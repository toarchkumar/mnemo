//! Integration tests for Mnemo.
//!
//! These exercise the public API end-to-end. `KdfParams::fast()` keeps Argon2
//! cheap so the suite runs quickly; production code should use the default
//! (secure) parameters.

use mnemo::{
    KdfParams, Memory, MemoryType, Metric, Mnemo, MnemoConfig, RecallRequest,
    ScoreWeights, Turn,
};
use tempfile::tempdir;

/// Config with cheap KDF params for fast tests.
fn fast_cfg(dimensions: usize) -> MnemoConfig {
    MnemoConfig { dimensions, kdf: KdfParams::fast(), ..Default::default() }
}

fn vec4(a: f32, b: f32, c: f32, d: f32) -> Vec<f32> {
    vec![a, b, c, d]
}

#[test]
fn create_insert_search_reopen_recall() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    assert!(db.is_empty());

    let id = db
        .remember(Memory::new("hello world", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.remember(Memory::new("goodbye", MemoryType::Episodic, vec4(0.0, 1.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();
    assert_eq!(db.len(), 2);

    // Exact search finds the nearest vector first.
    let hits = db.search(&vec4(0.9, 0.1, 0.0, 0.0), 1, Metric::Cosine).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.content, "hello world");

    // get by id
    let got = db.get(&id).unwrap();
    assert_eq!(got.content, "hello world");
    db.close().unwrap();

    // Reopen and recall.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 2);
    let res = db.recall(&RecallRequest::new(vec4(0.9, 0.1, 0.0, 0.0)).top_k(2)).unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].memory.content, "hello world");
}

/// Regression test for v7's header AEAD seal. Pre-v7, the only integrity
/// check on the mutable header fields was an unkeyed CRC-32 — an attacker
/// with write access could rewrite, say, `catalog_start` to point at an
/// older catalog run and reopen the database against stale data, since
/// the CRC is trivially recomputed without the DEK. v7 appends an
/// AES-GCM tag whose AAD covers every mutable header field; flipping
/// any of them (or any of the seal bytes themselves) invalidates the
/// tag, so the next open errors with `HeaderTampered`.
///
/// The test flips a single bit in the seal tag — which lives at byte
/// 254 of the header, well past the byte-238 CRC region — and confirms
/// the open fails. Tampering with bytes inside the CRC range would
/// instead trip the prefix-CRC torn-write check, so that case is not
/// tested here; the keyed seal is what makes the rollback attack loud.
#[test]
fn header_tamper_is_detected_by_v7_seal() {
    use mnemo::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("tamper.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new("x", MemoryType::Working, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();
    db.close().unwrap();

    // Seal tag offset matches `format::HEADER_SEAL_TAG_OFF`.
    const SEAL_TAG_OFF: usize = 254;
    let mut bytes = read_bytes(&path);
    bytes[SEAL_TAG_OFF] ^= 0x01;
    write_bytes(&path, &bytes);

    // `unwrap_err` would require `Mnemo: Debug`; match the Result instead.
    match Mnemo::open(&path, "pw") {
        Err(MnemoError::HeaderTampered) => {}
        Err(other) => panic!("expected HeaderTampered, got {other:?}"),
        Ok(_) => panic!("expected Mnemo::open to fail after seal tamper, but it succeeded"),
    }
}

/// Regression test for v6's page-binding AAD. Before v6, page encryption
/// passed no AAD to AES-GCM, so an attacker with write access could swap
/// two valid encrypted pages between slots and the database would happily
/// decrypt them at the wrong addresses. v6 binds `page_no.to_le_bytes()`
/// as AAD on every page encrypt/decrypt, so a swap fails authentication
/// on read.
///
/// The test creates two memories, locates their on-disk page slots from
/// the catalog, byte-swaps the two record pages, and asserts that any
/// subsequent read on either swapped page errors with PageAuthFailed.
#[test]
fn page_swap_attack_is_detected_by_aad() {
    use mnemo::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("swap.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let id_a = db
        .remember(Memory::new("ALPHA_AAA", MemoryType::Working, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    let id_b = db
        .remember(Memory::new("BETA_BBBB", MemoryType::Working, vec4(0.0, 1.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();
    // Pull the page slots from a successful get before swapping.
    let _ = db.get(&id_a).unwrap();
    let _ = db.get(&id_b).unwrap();
    db.close().unwrap();

    // With the default 8-page WAL reservation and 4-dim records, each
    // record fits in a single page; record A lands at page 9 and record
    // B lands at page 10 (page 0 is the header, pages 1..=8 are the WAL).
    // If the defaults ever shift this test fails visibly with a clear
    // page-number message — adjust the two constants below.
    const PAGE_SIZE: usize = 8192;
    const PAGE_A: usize = 9;
    const PAGE_B: usize = 10;
    let mut bytes = read_bytes(&path);
    let off_a = PAGE_A * PAGE_SIZE;
    let off_b = PAGE_B * PAGE_SIZE;
    assert!(off_b + PAGE_SIZE <= bytes.len(), "file too short for expected layout");
    let nonce_a: [u8; 12] = bytes[off_a..off_a + 12].try_into().unwrap();
    let nonce_b: [u8; 12] = bytes[off_b..off_b + 12].try_into().unwrap();
    assert_ne!(
        nonce_a, nonce_b,
        "pages {PAGE_A} and {PAGE_B} should have distinct nonces — defaults moved?"
    );

    // Swap the full page images on disk.
    let (left, right) = bytes.split_at_mut(off_b);
    left[off_a..off_a + PAGE_SIZE].swap_with_slice(&mut right[..PAGE_SIZE]);
    write_bytes(&path, &bytes);

    // Reopen and try to use the database. Any read that hits one of the
    // swapped pages must fail with PageAuthFailed — the v5 AAD binding
    // (none) made this attack silent; v6 binds page_no so the GCM tag
    // refuses the wrong slot.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let err_a = db.get(&id_a).unwrap_err();
    let err_b = db.get(&id_b).unwrap_err();
    let is_auth_fail = |e: &MnemoError| matches!(e, MnemoError::PageAuthFailed(_));
    assert!(
        is_auth_fail(&err_a) || is_auth_fail(&err_b),
        "at least one of the swapped pages must fail auth; got {err_a:?} / {err_b:?}"
    );
}

/// Regression: v5 moved `accessed_at` and `access_count` from the record body
/// into the catalog entry, so `recall` no longer rewrites the full record
/// (vector included) on every hit. Pre-v5 this loop did ~K × ~1.5 KB of
/// vector churn per recall plus a catalog rewrite at the next flush; v5
/// only dirties the catalog, and `track_access(false)` skips even that.
#[test]
fn recall_does_not_rewrite_records() {
    use std::fs;
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro.mnemo");

    // Seed two memories and flush.
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new("a", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.remember(Memory::new("b", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();
    let baseline = fs::metadata(&path).unwrap().len();

    // Many recalls with `track_access(false)` must not dirty anything —
    // the next flush is a no-op and the file size stays put.
    for _ in 0..50 {
        let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
            .top_k(2)
            .track_access(false);
        let _ = db.recall(&req).unwrap();
    }
    db.flush().unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().len(),
        baseline,
        "track_access=false recalls should not grow the file at all"
    );

    // With default `track_access(true)`, each recall+flush rewrites only the
    // catalog (one small page run) plus the manifest. Per-flush growth must
    // stay well under one full-record rewrite (vector + content), which at
    // 4-dim is small but pre-v5 would still scale with K results.
    let mut prev = baseline;
    for i in 0..20 {
        let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(2);
        let _ = db.recall(&req).unwrap();
        db.flush().unwrap();
        let now = fs::metadata(&path).unwrap().len();
        let growth = now - prev;
        // One catalog rewrite (~1 page) + one manifest entry (~1 page) +
        // some WAL frame overhead. Far below 50 KB per recall.
        assert!(
            growth < 50_000,
            "iter {i}: per-recall+flush growth {growth} bytes — pre-v5 catalog rewrite path?"
        );
        prev = now;
    }

    // Sanity: access stats actually got bumped despite no record rewrites.
    let hits = db.recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(1).track_access(false)).unwrap();
    assert!(
        hits[0].memory.access_count > 0,
        "track_access(true) recalls should have bumped access_count via the catalog"
    );
}

#[test]
fn file_is_encrypted_at_rest() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("enc.mnemo");

    let secret = "TOP-SECRET-PLAINTEXT-MARKER";
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new(secret, MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();
    db.close().unwrap();

    let raw = std::fs::read(&path).unwrap();
    let needle = secret.as_bytes();
    let found = raw.windows(needle.len()).any(|w| w == needle);
    assert!(!found, "plaintext content must not appear in the on-disk file");
}

#[test]
fn wrong_passphrase_is_rejected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wp.mnemo");

    let mut db = Mnemo::create(&path, "correct", fast_cfg(4)).unwrap();
    db.remember(Memory::new("x", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.close().unwrap();

    assert!(Mnemo::open(&path, "wrong").is_err());
    assert!(Mnemo::open(&path, "correct").is_ok());
}

#[test]
fn rekey_changes_the_passphrase() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rk.mnemo");

    let mut db = Mnemo::create(&path, "old-pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new("durable", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();
    db.rekey("new-pw", KdfParams::fast()).unwrap();
    db.close().unwrap();

    assert!(Mnemo::open(&path, "old-pw").is_err(), "old passphrase must fail");
    let mut db = Mnemo::open(&path, "new-pw").unwrap();
    assert_eq!(db.len(), 1);
    let id = db.memories().unwrap()[0].id;
    assert_eq!(db.get(&id).unwrap().content, "durable");
}

#[test]
fn copy_on_write_crash_safety() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cow.mnemo");

    // State A: one memory, flushed.
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new("state-a", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();

    // Write more WITHOUT flushing, then simulate a crash by dropping.
    db.remember(Memory::new("uncommitted", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0)))
        .unwrap();
    drop(db); // no flush — header still points at state A

    // Reopen: must see the last consistent state, uncorrupted.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1, "only the flushed state should survive");
    assert_eq!(db.memories().unwrap()[0].content, "state-a");
    assert_eq!(db.verify().unwrap(), 1, "surviving record must decrypt and decode");
}

#[test]
fn ttl_expiry_hides_memories() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ttl.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // ttl of 0 seconds => expired immediately (now - created_at >= 0).
    db.remember(
        Memory::new("ephemeral", MemoryType::Working, vec4(1.0, 0.0, 0.0, 0.0)).with_ttl(0),
    )
    .unwrap();
    db.remember(Memory::new("permanent", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();

    // Both are catalog-live, but recall/search skip the expired one.
    let res = db.recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(10)).unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].memory.content, "permanent");
}

#[test]
fn delete_then_compact_drops_tombstones() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cmp.mnemo");
    let path_str = path.to_str().unwrap();

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let keep =
        db.remember(Memory::new("keep", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))).unwrap();
    let drop_id =
        db.remember(Memory::new("drop", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0))).unwrap();
    db.delete(&drop_id).unwrap();
    db.flush().unwrap();
    assert_eq!(db.len(), 1);
    db.close().unwrap();

    let report = Mnemo::compact_file(path_str, "pw").unwrap();
    assert_eq!(report.before, 1);
    assert_eq!(report.after, 1);

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1);
    assert!(db.get(&keep).is_ok());
}

#[test]
fn recall_ranking_uses_importance() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rank.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // Two memories with identical vectors but different importance.
    db.remember(
        Memory::new("low importance", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
            .with_importance(0.0),
    )
    .unwrap();
    db.remember(
        Memory::new("high importance", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
            .with_importance(1.0),
    )
    .unwrap();
    db.flush().unwrap();

    let res = db.recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(2)).unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(
        res[0].memory.content, "high importance",
        "with equal similarity, importance should break the tie"
    );
    assert!(res[0].score >= res[1].score);
}

#[test]
fn agent_scoping_filters_recall() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("agent.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(
        Memory::new("alice private", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
            .with_agent("alice"),
    )
    .unwrap();
    db.remember(
        Memory::new("bob private", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
            .with_agent("bob"),
    )
    .unwrap();
    db.remember(
        Memory::new("shared note", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
            .with_agent("bob")
            .with_scope(mnemo::Scope::Shared),
    )
    .unwrap();
    db.flush().unwrap();

    let res = db
        .recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(10).agent("alice"))
        .unwrap();
    let contents: Vec<&str> = res.iter().map(|r| r.memory.content.as_str()).collect();
    assert!(contents.contains(&"alice private"));
    assert!(contents.contains(&"shared note"));
    assert!(!contents.contains(&"bob private"), "alice must not see bob's private memory");
}

#[test]
fn dimension_mismatch_is_rejected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("dim.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let bad = Memory::new("wrong size", MemoryType::Semantic, vec![1.0, 2.0]);
    assert!(db.remember(bad).is_err());
}

#[test]
fn type_filter_restricts_recall() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("type.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new("a fact", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.remember(Memory::new("an event", MemoryType::Episodic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap();

    let res = db
        .recall(
            &RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
                .top_k(10)
                .types(vec![MemoryType::Episodic]),
        )
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].memory.memory_type, MemoryType::Episodic);
}

// ---------------------------------------------------------------------------
// IVF + PQ approximate-nearest-neighbour index (Phase 2)
// ---------------------------------------------------------------------------

/// Tiny deterministic PRNG (xorshift64*) — keeps test data reproducible
/// without pulling `rand` into dev-dependencies.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `[0, 1)`.
    fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Roughly standard-normal (sum of uniforms, centred).
    fn normalish(&mut self) -> f32 {
        self.unit() + self.unit() + self.unit() + self.unit() - 2.0
    }
}

/// Score weights that depend on similarity only — makes recall ranking
/// directly comparable to exact search.
fn sim_only() -> ScoreWeights {
    ScoreWeights { alpha: 1.0, beta: 0.0, gamma: 0.0, delta: 0.0, half_life_secs: 1.0 }
}

/// Build a clustered dataset (like real embeddings) and verify the ANN index
/// recovers the exact top-10 with recall ≥ 0.95.
#[test]
fn ann_index_recall_quality() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ann.mnemo");
    let dims = 32;
    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();

    let mut rng = Rng::new(0xC0FFEE);
    let n_clusters = 20;
    let per_cluster = 30;

    let centers: Vec<Vec<f32>> = (0..n_clusters)
        .map(|_| (0..dims).map(|_| rng.normalish() * 5.0).collect())
        .collect();
    for c in &centers {
        for _ in 0..per_cluster {
            let v: Vec<f32> = c.iter().map(|&x| x + rng.normalish() * 0.3).collect();
            db.remember(Memory::new("vec", MemoryType::Semantic, v)).unwrap();
        }
    }
    db.flush().unwrap();
    let n = db.len();
    assert_eq!(n, n_clusters * per_cluster);

    // Queries near random cluster centres.
    let queries: Vec<Vec<f32>> = (0..30)
        .map(|_| {
            let ci = (rng.next_u64() as usize) % n_clusters;
            centers[ci].iter().map(|&x| x + rng.normalish() * 0.3).collect()
        })
        .collect();

    // Ground truth: exact top-10 via brute-force search.
    let exact: Vec<std::collections::HashSet<u128>> = queries
        .iter()
        .map(|q| {
            db.search(q, 10, Metric::L2)
                .unwrap()
                .iter()
                .map(|(m, _)| m.id.0)
                .collect()
        })
        .collect();

    // Build the index; recall should now use the tiered pipeline.
    let info = db.build_index().unwrap();
    db.flush().unwrap();
    assert!(db.has_index());
    assert_eq!(info.vectors, n);
    assert!(info.partitions >= 1 && info.subspaces >= 1);

    let mut overlap = 0usize;
    let mut total = 0usize;
    for (q, want) in queries.iter().zip(&exact) {
        let mut req = RecallRequest::new(q.clone()).top_k(10).n_probe(20).n_rerank(200);
        req.metric = Metric::L2;
        req.weights = sim_only();
        for h in db.recall(&req).unwrap() {
            if want.contains(&h.memory.id.0) {
                overlap += 1;
            }
        }
        total += want.len();
    }
    let recall = overlap as f64 / total as f64;
    assert!(recall >= 0.95, "ANN recall@10 was {recall:.3}, expected >= 0.95");
}

/// The index must persist across a close/reopen cycle.
#[test]
fn index_survives_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("persist-idx.mnemo");
    let dims = 16;

    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();
    let mut rng = Rng::new(7);
    for _ in 0..120 {
        let v: Vec<f32> = (0..dims).map(|_| rng.normalish()).collect();
        db.remember(Memory::new("m", MemoryType::Semantic, v)).unwrap();
    }
    db.build_index().unwrap();
    db.close().unwrap();

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(db.has_index(), "index must survive reopen");
    let q: Vec<f32> = (0..dims).map(|_| rng.normalish()).collect();
    let hits = db.recall(&RecallRequest::new(q).top_k(5)).unwrap();
    assert!(!hits.is_empty());
}

/// Memories inserted *after* an index is built are still retrievable — `put`
/// assigns them to their nearest partition incrementally.
#[test]
fn index_incremental_insert_is_searchable() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("incremental.mnemo");
    let dims = 16;

    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();
    let mut rng = Rng::new(99);
    for _ in 0..100 {
        let v: Vec<f32> = (0..dims).map(|_| rng.normalish()).collect();
        db.remember(Memory::new("old", MemoryType::Semantic, v)).unwrap();
    }
    db.build_index().unwrap();

    // Insert a distinctive vector after the index exists.
    let needle = vec![9.0f32; dims];
    let needle_id =
        db.remember(Memory::new("needle", MemoryType::Semantic, needle.clone())).unwrap();
    db.flush().unwrap();

    let mut req = RecallRequest::new(needle).top_k(1).n_probe(8).n_rerank(64);
    req.metric = Metric::L2;
    req.weights = sim_only();
    let hits = db.recall(&req).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].memory.id, needle_id, "post-build insert must be indexed");
}

/// Dropping the index reverts recall to exact scans; compaction rebuilds it.
#[test]
fn index_drop_and_compact_rebuild() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("idx-lifecycle.mnemo");
    let path_str = path.to_str().unwrap();
    let dims = 16;

    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();
    let mut rng = Rng::new(55);
    for _ in 0..80 {
        let v: Vec<f32> = (0..dims).map(|_| rng.normalish()).collect();
        db.remember(Memory::new("m", MemoryType::Semantic, v)).unwrap();
    }
    db.build_index().unwrap();
    db.drop_index();
    db.close().unwrap();

    // Index was dropped before closing — must be absent on reopen.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(!db.has_index());

    // Rebuild, then compact: the compacted file should still carry an index.
    db.build_index().unwrap();
    db.close().unwrap();
    Mnemo::compact_file(path_str, "pw").unwrap();

    let db = Mnemo::open(&path, "pw").unwrap();
    assert!(db.has_index(), "compaction must rebuild the index");
}

// --- write-ahead log -----------------------------------------------------

fn read_bytes(p: &std::path::Path) -> Vec<u8> {
    std::fs::read(p).unwrap()
}
fn write_bytes(p: &std::path::Path, b: &[u8]) {
    std::fs::write(p, b).unwrap()
}

/// A transaction committed to the WAL but not yet checkpointed (a crash
/// between the WAL fsync and the home-page writes) is replayed on open.
#[test]
fn wal_crash_recovery_replays_committed_txn() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    // State A: five memories, cleanly flushed.
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..5 {
        db.remember(Memory::new(format!("mem-{i}"), MemoryType::Semantic, vec4(i as f32, 0.0, 0.0, 0.0)))
            .unwrap();
    }
    db.close().unwrap();
    let snapshot = read_bytes(&path); // page 0 here points at the 5-memory state

    // State B: three more memories, cleanly flushed.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    for i in 5..8 {
        db.remember(Memory::new(format!("mem-{i}"), MemoryType::Semantic, vec4(i as f32, 0.0, 0.0, 0.0)))
            .unwrap();
    }
    db.close().unwrap();
    let file_b = read_bytes(&path);

    // Frankenfile: state B's file with page 0 rewound to state A. This is
    // exactly a crash after B's WAL commit but before its checkpoint — the
    // WAL holds committed txn B, yet the header still describes A.
    let franken = dir.path().join("franken.mnemo");
    let mut bytes = file_b.clone();
    bytes[0..8192].copy_from_slice(&snapshot[0..8192]);
    write_bytes(&franken, &bytes);

    // Opening it must replay the WAL and surface all eight memories.
    let mut db = Mnemo::open(&franken, "pw").unwrap();
    assert_eq!(db.len(), 8, "WAL recovery must restore the committed txn");
    let contents: Vec<String> = db.memories().unwrap().into_iter().map(|m| m.content).collect();
    assert!(contents.contains(&"mem-7".to_string()));
}

/// A torn page-0 write is detected by the header CRC and healed from the WAL.
#[test]
fn wal_heals_torn_header() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..6 {
        db.remember(Memory::new(format!("k-{i}"), MemoryType::Episodic, vec4(i as f32, 1.0, 2.0, 3.0)))
            .unwrap();
    }
    db.close().unwrap();

    // Corrupt the header page — simulate a torn write to page 0.
    let mut bytes = read_bytes(&path);
    for b in bytes.iter_mut().take(260).skip(120) {
        *b ^= 0xFF;
    }
    write_bytes(&path, &bytes);

    // Open must notice the bad CRC and rebuild page 0 from the WAL.
    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 6, "torn header must self-heal from the WAL");
}

/// A cleanly checkpointed database opens correctly even if the WAL region is
/// full of garbage: with no valid commit frame, recovery replays nothing.
#[test]
fn wal_discards_uncommitted_garbage() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..5 {
        db.remember(Memory::new(format!("g-{i}"), MemoryType::Working, vec4(i as f32, 0.0, 0.0, 0.0)))
            .unwrap();
    }
    db.close().unwrap();

    // Overwrite exactly the WAL region with 0xFF. The actual size lives in
    // the header (offset 194), so we read it rather than hardcoding — the
    // default WAL size has shrunk over the project's lifetime and may again.
    let mut bytes = read_bytes(&path);
    let wal_pages = u64::from_le_bytes(bytes[194..202].try_into().unwrap()) as usize;
    let region_end = (1 + wal_pages) * 8192;
    for b in bytes.iter_mut().take(region_end).skip(8192) {
        *b = 0xFF;
    }
    write_bytes(&path, &bytes);

    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 5, "a garbage WAL must not disturb a checkpointed db");
}

/// The WAL region grows automatically once a transaction's control plane
/// (here, a large catalog) outgrows the initial reservation.
#[test]
fn wal_region_grows_for_large_catalog() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("big.mnemo");

    // Enough memories that the serialized catalog dwarfs any sensible
    // initial WAL — the region must grow far past whatever the default is.
    let n = 22_000;
    let mut db = Mnemo::create(&path, "pw", fast_cfg(2)).unwrap();
    let mut rng = Rng::new(7);
    for _ in 0..n {
        let v = vec![rng.unit(), rng.unit()];
        db.remember(Memory::new("x", MemoryType::Semantic, v)).unwrap();
    }
    db.close().unwrap();

    // wal_pages lives at byte offset 194 of the (plaintext) header page.
    // 64 was the v0.1.0 default; even after dropping the default to 8, a
    // 22k-memory catalog still forces growth well past 64.
    let bytes = read_bytes(&path);
    let wal_pages = u64::from_le_bytes(bytes[194..202].try_into().unwrap());
    assert!(wal_pages > 64, "WAL should have grown well past the default (got {wal_pages})");

    // The grown/relocated WAL must reopen cleanly with every memory intact.
    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), n);
}

/// The default initial WAL is small (8 pages = 64 KiB), so a populated file
/// is well under what the v0.1.0 default (64 pages = 512 KiB) produced.
///
/// `Mnemo::create` only physically writes page 0; the WAL region and any
/// data pages past it are only allocated once writes force the file to
/// extend (sparsely, on most filesystems). To compare the *effective*
/// footprint each config produces, we write a single memory and flush —
/// that pushes the file out past `next_page = 1 + wal_pages`, so its
/// reported length now reflects the WAL reservation honestly.
#[test]
fn fresh_file_uses_small_wal_by_default() {
    use std::fs;
    let dir = tempdir().unwrap();

    // Helper: create with `cfg`, write one memory + flush, return
    // (file length in bytes, the wal_pages value recorded in the header).
    let make = |path: &std::path::Path, cfg: MnemoConfig| -> (u64, u64) {
        let mut db = Mnemo::create(path, "pw", cfg).unwrap();
        db.remember(Memory::new("seed", MemoryType::Working, vec4(1.0, 0.0, 0.0, 0.0)))
            .unwrap();
        db.flush().unwrap();
        db.close().unwrap();
        let bytes = fs::metadata(path).unwrap().len();
        let raw = read_bytes(path);
        let wal_pages = u64::from_le_bytes(raw[194..202].try_into().unwrap());
        (bytes, wal_pages)
    };

    // 1. Default config: 8 WAL pages (64 KiB). After one write+flush the
    //    file extends through the WAL reservation; it must stay well under
    //    the v0.1.0 footprint (which started at 65 pages = 532 KiB before
    //    any data was written).
    let (small_bytes, small_wal) = make(&dir.path().join("small.mnemo"), fast_cfg(4));
    assert_eq!(small_wal, 8, "default wal_pages_initial should be 8");
    assert!(
        small_bytes < 200_000,
        "fresh-default file ballooned: {small_bytes} bytes (expected < 200 KB)"
    );

    // 2. Explicit override: a 64-page initial WAL sticks in the header and
    //    produces a meaningfully larger file (the extra 56 reserved WAL
    //    pages alone are 56 * 8 KiB = 458 KiB).
    let big_cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        wal_pages_initial: 64,
        ..Default::default()
    };
    let (big_bytes, big_wal) = make(&dir.path().join("big.mnemo"), big_cfg);
    assert_eq!(big_wal, 64, "explicit wal_pages_initial=64 should stick");
    let delta = big_bytes.saturating_sub(small_bytes);
    assert!(
        delta >= 56 * 8192 - 8192, // allow one page slack for transaction layout differences
        "explicit large WAL should add ~56 pages; default={small_bytes} big={big_bytes} (delta={delta})"
    );

    // 3. Floor enforcement: 0 is clamped up to MIN_WAL_PAGES (2).
    let tiny_cfg = MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        wal_pages_initial: 0,
        ..Default::default()
    };
    let (_tiny_bytes, tiny_wal) = make(&dir.path().join("tiny.mnemo"), tiny_cfg);
    assert!(
        tiny_wal >= 2,
        "wal_pages_initial=0 should be clamped to the MIN_WAL_PAGES floor (got {tiny_wal})"
    );
}

// --- crash-recovery: nonce uniqueness across an aborted transaction ------

/// Regression test for the **AES-GCM nonce-reuse window** documented in
/// Phase 1.1 of the improvement plan.
///
/// Before the leasing fix, `Mnemo::flush` wrote data pages with advanced
/// `write_counter` values *before* committing the WAL. A crash in that
/// window left:
///
/// 1. Home pages on disk encrypted under nonces derived from the bumped
///    counter values.
/// 2. An on-disk header still recording the **old** `write_counter` and
///    **old** `next_page`, because the transaction never committed.
///
/// On reopen the in-memory counter was restored from the stale header, so
/// the next `remember + flush` re-allocated the same page slots and
/// re-derived the same `(page_no, write_counter)` nonces — encrypting a
/// *different* plaintext under the same DEK with the same nonce.
///
/// The fix is **counter+page leasing in `prepare_for_flush`**: bump a
/// cloned header by upper-bound counter/page advances and persist it
/// with `pager.write_raw + sync` *before* any data page is encrypted.
/// A crash anywhere from the lease to the WAL commit then leaves a file
/// whose on-disk header records a counter past every nonce the orphan
/// transaction could have used.
///
/// This test takes a `prepare_for_flush + pager.flush` snapshot via
/// `__crash_partial_flush_for_testing`, drops the handle, reopens, writes
/// *different* content, and asserts no byte position in the file shows
/// the same 12-byte nonce prefix with different ciphertexts. With the
/// leasing fix in place it passes; without it (a previous commit), it
/// fails with an explicit `NONCE REUSE confirmed at page N` message.
#[test]
fn nonce_unique_after_crashed_data_flush() {
    use std::collections::HashMap;
    const PAGE_SIZE: usize = 8192;

    let dir = tempdir().unwrap();
    let path = dir.path().join("crash.mnemo");

    // --- Phase 1: orphan transaction (data pages on disk, no WAL commit) -
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.remember(Memory::new(
            "ORPHAN_TRANSACTION_AAA",
            MemoryType::Working,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        // Run the data-page write portion of flush only — no WAL commit.
        // Then drop the handle so the on-disk header still records the
        // stale write_counter and next_page.
        db.__crash_partial_flush_for_testing().unwrap();
    }

    // Snapshot every encrypted page's (nonce, ciphertext) after Phase 1.
    let snap1 = read_bytes(&path);
    assert!(snap1.len() % PAGE_SIZE == 0, "file must be page-aligned");
    let page_count = snap1.len() / PAGE_SIZE;
    let mut phase1: HashMap<u64, [u8; 12]> = HashMap::new();
    for page_no in 1..page_count as u64 {
        let off = page_no as usize * PAGE_SIZE;
        let nonce: [u8; 12] = snap1[off..off + 12].try_into().unwrap();
        // Skip pages that were never touched (all-zero nonces in the
        // sparse WAL region or beyond the high-water mark).
        if nonce != [0u8; 12] {
            phase1.insert(page_no, nonce);
        }
    }
    assert!(
        !phase1.is_empty(),
        "Phase 1 should have written at least one encrypted data page; \
         did __crash_partial_flush_for_testing actually flush?"
    );

    // --- Phase 2: reopen, write DIFFERENT content, full flush -----------
    {
        let mut db = Mnemo::open(&path, "pw").unwrap();
        db.remember(Memory::new(
            "REUSE_ATTEMPT_DIFFERENT_BBB",
            MemoryType::Working,
            vec4(0.0, 1.0, 0.0, 0.0),
        ))
        .unwrap();
        db.flush().unwrap();
    }

    // --- Detection: same nonce at the same page slot, different ct. -----
    let snap2 = read_bytes(&path);
    for (&page_no, &nonce_phase1) in &phase1 {
        let off = page_no as usize * PAGE_SIZE;
        if off + PAGE_SIZE > snap2.len() {
            continue;
        }
        let nonce_phase2: [u8; 12] = snap2[off..off + 12].try_into().unwrap();
        if nonce_phase2 == nonce_phase1 {
            let ct_phase1 = &snap1[off + 12..off + PAGE_SIZE];
            let ct_phase2 = &snap2[off + 12..off + PAGE_SIZE];
            if ct_phase1 != ct_phase2 {
                panic!(
                    "NONCE REUSE confirmed at page {page_no}: \
                     nonce {:02x?} was used to encrypt two different \
                     ciphertexts under the same DEK.\n\
                     This leaks keystream — anyone with both ciphertexts \
                     can compute plaintext_phase1 XOR plaintext_phase2 \
                     and breaks AES-GCM's authentication guarantees.\n\
                     (Phase 1.1 leasing fix not applied yet.)",
                    nonce_phase1
                );
            }
        }
    }
}

// --- bounded page cache --------------------------------------------------

/// With the page cache capped well below the working set, every lookup must
/// still return correct data (eviction forces a re-decrypt from disk), and
/// the cache must stay within its bound.
#[test]
fn bounded_cache_survives_eviction() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let mut ids = Vec::new();
    for i in 0..300 {
        let id = db
            .remember(Memory::new(
                format!("content-{i}"),
                MemoryType::Semantic,
                vec4(i as f32, 1.0, 2.0, 3.0),
            ))
            .unwrap();
        ids.push((id, i));
    }
    db.flush().unwrap();

    // Shrink the cache far below the 300-page working set.
    db.set_cache_capacity(8);

    // Every lookup must round-trip correctly despite constant eviction.
    for (id, i) in &ids {
        let m = db.get(id).unwrap();
        assert_eq!(m.content, format!("content-{i}"));
        assert_eq!(m.vector[0], *i as f32);
    }

    let (used, cap) = db.cache_stats();
    assert_eq!(cap, 8);
    assert!(used <= 8, "cache held {used} clean pages, cap {cap}");

    // The bound survives a reopen + re-shrink too.
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.set_cache_capacity(4);
    for (id, i) in &ids {
        assert_eq!(db.get(id).unwrap().content, format!("content-{i}"));
    }
    assert!(db.cache_stats().0 <= 4);
}

// --- snapshots & point-in-time recovery ----------------------------------

/// Every flush appends exactly one restorable snapshot, and the manifest
/// survives a reopen.
#[test]
fn snapshots_record_every_flush() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..3 {
        db.remember(Memory::new(format!("m{i}"), MemoryType::Semantic,
            vec4(i as f32, 0.0, 0.0, 0.0))).unwrap();
        db.flush().unwrap();
    }
    db.close().unwrap();

    let db = Mnemo::open(&path, "pw").unwrap();
    let snaps = db.snapshots();
    assert_eq!(snaps.len(), 3);
    for (i, s) in snaps.iter().enumerate() {
        assert_eq!(s.txn_id, (i + 1) as u64);
        assert_eq!(s.memory_count, (i + 1) as u64);
    }
}

/// `MnemoConfig::max_snapshots` (Phase 2.3) caps the retained manifest
/// length. The newest N entries are kept; the rest get pruned at flush
/// time. `restore_to` a pruned txn_id returns `NotFound`. Setting the
/// cap to 0 disables it entirely.
#[test]
fn max_snapshots_prunes_oldest_entries() {
    use mnemo::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("cap.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.set_max_snapshots(10);
    for i in 0..15 {
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Semantic,
            vec4(i as f32, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        db.flush().unwrap();
    }
    let snaps = db.snapshots();
    assert_eq!(snaps.len(), 10, "cap=10 should keep exactly 10 snapshots");
    // The retained txn_ids must be the most-recent ten (6..=15 for
    // 15 flushes), not the oldest ten.
    let kept_ids: Vec<u64> = snaps.iter().map(|s| s.txn_id).collect();
    assert_eq!(kept_ids, (6..=15).collect::<Vec<_>>());

    // restore_to one of the pruned txn_ids must fail with NotFound; the
    // pages those snapshots reference are still on disk but the manifest
    // no longer knows where they are.
    let err = db.restore_to(3).expect_err("pruned txn must not resolve");
    assert!(
        matches!(err, MnemoError::NotFound(_)),
        "expected NotFound for pruned txn_id 3; got {err:?}"
    );

    // The most-recent retained txn_id IS still restorable.
    let restored = db.restore_to(10).expect("retained txn 10 must restore");
    assert_eq!(restored.txn_id, 10);

    // After restore_to, one more flush appends a new snapshot — verify
    // the cap holds at 10 (one pruned to make room for the new one).
    assert_eq!(db.snapshots().len(), 10);

    // Disabling the cap mid-life: cap=0 lets the manifest grow again.
    db.set_max_snapshots(0);
    for i in 0..5 {
        db.remember(Memory::new(
            format!("more-{i}"),
            MemoryType::Working,
            vec4(0.0, i as f32, 0.0, 0.0),
        ))
        .unwrap();
        db.flush().unwrap();
    }
    assert!(
        db.snapshots().len() > 10,
        "cap=0 should let the manifest grow past the old cap"
    );
}

/// Restoring to a past transaction reinstates exactly that state, and the
/// restore is itself reversible — you can roll forward again.
#[test]
fn restore_rewinds_and_rolls_forward() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let mut ids = Vec::new();
    for i in 0..3 {
        let id = db.remember(Memory::new(format!("content-{i}"), MemoryType::Semantic,
            vec4(i as f32, 0.0, 0.0, 0.0))).unwrap();
        db.flush().unwrap(); // snapshot txn i+1 holds i+1 memories
        ids.push(id);
    }
    assert_eq!(db.len(), 3);

    // Rewind to the one-memory snapshot.
    let info = db.restore_to(1).unwrap();
    assert_eq!(info.memory_count, 1);
    assert_eq!(db.len(), 1);
    assert_eq!(db.get(&ids[0]).unwrap().content, "content-0");
    assert!(db.get(&ids[2]).is_err(), "later memories must be gone");

    // The rewind persists across a reopen.
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1);

    // Roll forward to the three-memory snapshot — history is still intact.
    db.restore_to(3).unwrap();
    assert_eq!(db.len(), 3);
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(db.get(id).unwrap().content, format!("content-{i}"));
    }

    // Each restore was itself recorded: 3 flushes + 2 restores = 5 snapshots.
    assert_eq!(db.snapshots().len(), 5);
}

/// Time-based recovery selects the latest snapshot at or before an instant.
#[test]
fn restore_to_a_past_instant() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new("first", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap(); // snapshot 1

    // Ensure the next snapshot lands in a strictly later second.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    db.remember(Memory::new("second", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0)))
        .unwrap();
    db.flush().unwrap(); // snapshot 2

    let snaps = db.snapshots();
    let (t1, t2) = (snaps[0].created_at, snaps[1].created_at);
    assert!(t2 > t1, "snapshots should differ in time");

    // As of t2, both memories existed (snapshot 2 is the latest <= t2).
    let info = db.restore_to_time(t2).unwrap();
    assert_eq!(info.txn_id, 2);
    assert_eq!(db.len(), 2);

    // As of t1, only the first did — snapshot 1 is the only one that old.
    let info = db.restore_to_time(t1).unwrap();
    assert_eq!(info.txn_id, 1);
    assert_eq!(db.len(), 1);

    // Before any snapshot existed: nothing to restore to.
    assert!(db.restore_to_time(t1 - 3600).is_err());
}

/// Compaction reclaims space by collapsing history to a single snapshot —
/// the documented boundary of point-in-time recovery.
#[test]
fn compaction_collapses_history() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let path_str = path.to_str().unwrap();

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..4 {
        db.remember(Memory::new(format!("m{i}"), MemoryType::Episodic,
            vec4(i as f32, 0.0, 0.0, 0.0))).unwrap();
        db.flush().unwrap();
    }
    assert_eq!(db.snapshots().len(), 4);
    db.close().unwrap();

    Mnemo::compact_file(path_str, "pw").unwrap();

    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 4, "compaction keeps the live data");
    assert_eq!(db.snapshots().len(), 1, "but resets the history");
}

// --- session lifecycle ---------------------------------------------------

/// Turns added during a session are written as working memory, tagged with
/// the session id, agent, and role.
#[test]
fn session_turns_written_as_working() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(3)).unwrap();

    let (ids, sid) = {
        let mut s = db.session("assistant-7");
        let sid = s.id().to_string();
        let a = s.add_turn(Turn::user("hello", vec![1.0, 0.0, 0.0])).unwrap();
        let b = s.add_turn(Turn::assistant("hi there", vec![0.0, 1.0, 0.0])).unwrap();
        assert_eq!(s.turn_count(), 2);
        assert_eq!(s.agent(), "assistant-7");
        (vec![a, b], sid)
        // session dropped here without close — no consolidation
    };
    db.flush().unwrap();

    for id in &ids {
        let m = db.get(id).unwrap();
        assert_eq!(m.memory_type, MemoryType::Working);
        assert_eq!(m.agent_id, "assistant-7");
        assert_eq!(m.session_id.as_deref(), Some(sid.as_str()));
    }
    let first = db.get(&ids[0]).unwrap();
    assert_eq!(first.metadata.get("role").and_then(|v| v.as_str()), Some("user"));
}

/// Closing a session consolidates its working turns into episodic memory,
/// and that promotion survives a reopen.
#[test]
fn session_close_consolidates_to_episodic() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(3)).unwrap();

    let ids = {
        let mut s = db.session("agent-x");
        let mut v = Vec::new();
        for i in 0..3 {
            v.push(
                s.add_turn(Turn::user(format!("turn-{i}"), vec![i as f32, 0.0, 0.0]))
                    .unwrap(),
            );
        }
        let promoted = s.close().unwrap();
        assert_eq!(promoted, 3, "every working turn should be promoted");
        v
    };
    for id in &ids {
        assert_eq!(db.get(id).unwrap().memory_type, MemoryType::Episodic);
    }

    // The consolidation is durable.
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    for id in &ids {
        assert_eq!(db.get(id).unwrap().memory_type, MemoryType::Episodic);
    }
}

/// Discarding a session deletes its working turns instead of consolidating.
#[test]
fn session_discard_deletes_turns() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(3)).unwrap();

    let ids = {
        let mut s = db.session("agent-x");
        let a = s.add_turn(Turn::user("scratch", vec![1.0, 0.0, 0.0])).unwrap();
        let b = s.add_turn(Turn::assistant("noise", vec![0.0, 1.0, 0.0])).unwrap();
        let removed = s.discard().unwrap();
        assert_eq!(removed, 2);
        vec![a, b]
    };
    for id in &ids {
        assert!(db.get(id).is_err(), "discarded turns must be gone");
    }
    assert_eq!(db.len(), 0);
}

/// A session's recall is scoped to its own agent — another agent's private
/// memories never leak into the results.
#[test]
fn session_recall_is_agent_scoped() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(3)).unwrap();

    db.remember(
        Memory::new("bob-private-secret", MemoryType::Semantic, vec![1.0, 0.0, 0.0])
            .with_agent("bob"),
    )
    .unwrap();
    db.remember(
        Memory::new("alice-fact", MemoryType::Semantic, vec![1.0, 0.0, 0.0])
            .with_agent("alice"),
    )
    .unwrap();
    db.flush().unwrap();

    let contents: Vec<String> = {
        let mut s = db.session("alice");
        let hits = s.recall(RecallRequest::new(vec![1.0, 0.0, 0.0]).top_k(10)).unwrap();
        hits.into_iter().map(|h| h.memory.content).collect()
    };
    assert!(contents.contains(&"alice-fact".to_string()));
    assert!(
        !contents.contains(&"bob-private-secret".to_string()),
        "another agent's private memory must not leak into a scoped recall"
    );
}
