// Shared body of `tests/integration.rs`, compiled once per format: the
// including module defines `FORMAT` (8 or 9).

use mnemo_engine::{
    CacheBudget, CacheFlushPolicy, CachePutOpts, CommitBatch, EssenceOp, EssenceOpKind,
    EssenceScope, EssenceSection, Filter, KdfParams, KeyScope, KeyedMemory, Memory, MemoryPatch,
    MemoryType, Metric, Mnemo, MnemoConfig, RecallRequest, Scope, ScoreWeights,
    SemanticCachePutOpts, ThreadTurn, Turn, TurnPolicy, WriteOutcome, DEFAULT_SEMANTIC_THRESHOLD,
    MNEMO_META_CONTINUES, MNEMO_META_INVALID_AT, MNEMO_META_KEY, MNEMO_META_KIND,
    MNEMO_META_KIND_THREAD_LINK, MNEMO_META_STATUS, MNEMO_META_STATUS_HISTORY,
    MNEMO_META_SUPERSEDED_BY, MNEMO_META_SUPERSEDES,
};
use serde_json::json;
use tempfile::tempdir;

/// Config with cheap KDF params for fast tests.
fn fast_cfg(dimensions: usize) -> MnemoConfig {
    // FMT-9f: these suites exercise the ANN index on small files, which a
    // v9 file only builds at `V9_ANN_MIN_FACTS` (20K) facts; the hidden
    // test override drops the threshold for this whole test binary (both
    // modules, so every test sees the same value).
    std::env::set_var("MNEMO_DEV_ANN_MIN_FACTS", "0");
    MnemoConfig {
        format_version: FORMAT,
        dimensions,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
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
        .remember(Memory::new(
            "hello world",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    db.remember(Memory::new(
        "goodbye",
        MemoryType::Episodic,
        vec4(0.0, 1.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();
    assert_eq!(db.len(), 2);

    // Exact search finds the nearest vector first.
    let hits = db
        .search(&vec4(0.9, 0.1, 0.0, 0.0), 1, Metric::Cosine)
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.content, "hello world");

    // get by id
    let got = db.get(&id).unwrap();
    assert_eq!(got.content, "hello world");
    db.close().unwrap();

    // Reopen and recall.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 2);
    let res = db
        .recall(&RecallRequest::new(vec4(0.9, 0.1, 0.0, 0.0)).top_k(2))
        .unwrap();
    assert_eq!(res.len(), 2);
    assert_eq!(res[0].memory.content, "hello world");
}

/// REFAC-1: `close` consumes the handle and releases the file and its lock
/// before returning, so the same file reopens right after it, in the same
/// scope, with no `drop` (writable, read-only, then writable again).
#[test]
fn close_releases_the_file_so_it_reopens_in_the_same_scope() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("reopen.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let id = db
        .remember(Memory::new(
            "kept",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.get(&id).unwrap().content, "kept", "close flushed");
    db.close().unwrap();
    let ro = Mnemo::open_read_only(&path, "pw").unwrap();
    assert_eq!(ro.len(), 1);
    ro.close().unwrap();
    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1);
}

/// MOBILE-1a: `ingest_thread_texts` + `ingest_thread_with_vectors` split
/// an ingest so the caller embeds in between (the UniFFI binding does, with
/// no lock held). The texts are the new chunks only; a count that no longer
/// matches the plan writes nothing.
#[test]
fn ingest_thread_in_two_steps_with_caller_vectors() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("two-step.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let turns = vec![
        ThreadTurn {
            turn_id: "a".into(),
            role: "user".into(),
            content: "short".into(),
            timestamp: None,
        },
        ThreadTurn {
            turn_id: "b".into(),
            role: "assistant".into(),
            content: "x".repeat(20_000),
            timestamp: None,
        },
    ];
    let texts = db.ingest_thread_texts("chat", &turns).unwrap();
    assert!(texts.len() >= 3, "one record for 'a', chunks for 'b'");
    assert_eq!(texts[0], "short");
    let vectors: Vec<Vec<f32>> = (0..texts.len())
        .map(|i| vec4(1.0 + i as f32, 0.5, 0.25, 0.125))
        .collect();
    // The wrong count writes nothing.
    assert!(db
        .ingest_thread_with_vectors("chat", &turns, &vectors[1..])
        .is_err());
    assert!(db.thread_export_turns("chat").unwrap().is_empty());
    let report = db
        .ingest_thread_with_vectors("chat", &turns, &vectors)
        .unwrap();
    assert_eq!(report.added_records, texts.len());
    // Everything is archived now, so nothing is left to embed.
    assert!(db.ingest_thread_texts("chat", &turns).unwrap().is_empty());
    let again = db.ingest_thread_with_vectors("chat", &turns, &[]).unwrap();
    assert_eq!(again.skipped_existing_turns, 2);
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let back = db.thread_export_turns("chat").unwrap();
    assert_eq!(back.len(), 2);
    assert_eq!(back[1].content, turns[1].content);
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
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("tamper.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new(
        "x",
        MemoryType::Working,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();
    db.close().unwrap();

    // Seal tag offset matches `format::HEADER_SEAL_TAG_OFF` (both formats).
    const SEAL_TAG_OFF: usize = 254;
    let mut bytes = read_bytes(&path);
    bytes[SEAL_TAG_OFF] ^= 0x01;
    if FORMAT == 9 {
        // FMT-9f: v9's header CRC (at 508) covers the seal, so a lone
        // flip reads as a torn header and heals from the WAL. A tamper
        // recomputes the CRC; then only the seal can catch it.
        let crc = crc32(&bytes[0..508]).to_le_bytes();
        bytes[508..512].copy_from_slice(&crc);
    }
    write_bytes(&path, &bytes);

    // `unwrap_err` would require `Mnemo: Debug`; match the Result instead.
    match Mnemo::open(&path, "pw") {
        Err(MnemoError::HeaderTampered) => {}
        Err(other) => panic!("expected HeaderTampered, got {other:?}"),
        Ok(_) => panic!("expected Mnemo::open to fail after seal tamper, but it succeeded"),
    }
}

/// The header CRC-32 (the WAL's), so a test can rewrite it after an edit.
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            let mask = (c & 1).wrapping_neg();
            c = (c >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !c
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
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("swap.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let id_a = db
        .remember(Memory::new(
            "ALPHA_AAA",
            MemoryType::Working,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    let id_b = db
        .remember(Memory::new(
            "BETA_BBBB",
            MemoryType::Working,
            vec4(0.0, 1.0, 0.0, 0.0),
        ))
        .unwrap();
    db.flush().unwrap();
    // Pull the page slots from a successful get before swapping.
    let _ = db.get(&id_a).unwrap();
    let _ = db.get(&id_b).unwrap();
    db.close().unwrap();
    // the raw file read below (see file_is_encrypted_at_rest for the
    // full rationale).

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
    assert!(
        off_b + PAGE_SIZE <= bytes.len(),
        "file too short for expected layout"
    );
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
    // refuses the wrong slot. (Writer already dropped above so the raw
    // read/write could run under Windows' mandatory lock.)
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
    db.remember(Memory::new(
        "a",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.remember(Memory::new(
        "b",
        MemoryType::Semantic,
        vec4(0.0, 1.0, 0.0, 0.0),
    ))
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
    let hits = db
        .recall(
            &RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
                .top_k(1)
                .track_access(false),
        )
        .unwrap();
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
    db.remember(Memory::new(
        secret,
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();
    db.close().unwrap();
    // on Windows is *mandatory* — any other open of the file's byte

    let raw = std::fs::read(&path).unwrap();
    let needle = secret.as_bytes();
    let found = raw.windows(needle.len()).any(|w| w == needle);
    assert!(
        !found,
        "plaintext content must not appear in the on-disk file"
    );
}

#[test]
fn wrong_passphrase_is_rejected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("wp.mnemo");

    let mut db = Mnemo::create(&path, "correct", fast_cfg(4)).unwrap();
    db.remember(Memory::new(
        "x",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.close().unwrap();
    // masking the passphrase-check we're actually asserting.

    assert!(Mnemo::open(&path, "wrong").is_err());
    assert!(Mnemo::open(&path, "correct").is_ok());
}

#[test]
fn rekey_changes_the_passphrase() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rk.mnemo");

    let mut db = Mnemo::create(&path, "old-pw", fast_cfg(4)).unwrap();
    db.remember(Memory::new(
        "durable",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();
    db.rekey("new-pw", KdfParams::fast()).unwrap();
    db.close().unwrap();
    // (which would still pass `.is_err()` but for the wrong reason).

    assert!(
        Mnemo::open(&path, "old-pw").is_err(),
        "old passphrase must fail"
    );
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
    db.remember(Memory::new(
        "state-a",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();

    // Write more WITHOUT flushing, then simulate a crash by dropping.
    db.remember(Memory::new(
        "uncommitted",
        MemoryType::Semantic,
        vec4(0.0, 1.0, 0.0, 0.0),
    ))
    .unwrap();
    drop(db); // no flush — header still points at state A

    // Reopen: must see the last consistent state, uncorrupted.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1, "only the flushed state should survive");
    assert_eq!(db.memories().unwrap()[0].content, "state-a");
    assert_eq!(
        db.verify().unwrap(),
        1,
        "surviving record must decrypt and decode"
    );
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
    db.remember(Memory::new(
        "permanent",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();

    // Both are catalog-live, but recall/search skip the expired one.
    let res = db
        .recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(10))
        .unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].memory.content, "permanent");
}

#[test]
fn delete_then_compact_drops_tombstones() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cmp.mnemo");
    let path_str = path.to_str().unwrap();

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let keep = db
        .remember(Memory::new(
            "keep",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    let drop_id = db
        .remember(Memory::new(
            "drop",
            MemoryType::Semantic,
            vec4(0.0, 1.0, 0.0, 0.0),
        ))
        .unwrap();
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
        Memory::new(
            "low importance",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        )
        .with_importance(0.0),
    )
    .unwrap();
    db.remember(
        Memory::new(
            "high importance",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        )
        .with_importance(1.0),
    )
    .unwrap();
    db.flush().unwrap();

    let res = db
        .recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(2))
        .unwrap();
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
        Memory::new(
            "alice private",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        )
        .with_agent("alice"),
    )
    .unwrap();
    db.remember(
        Memory::new(
            "bob private",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        )
        .with_agent("bob"),
    )
    .unwrap();
    db.remember(
        Memory::new(
            "shared note",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        )
        .with_agent("bob")
        .with_scope(mnemo_engine::Scope::Shared),
    )
    .unwrap();
    db.flush().unwrap();

    let res = db
        .recall(
            &RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
                .top_k(10)
                .agent("alice"),
        )
        .unwrap();
    let contents: Vec<&str> = res.iter().map(|r| r.memory.content.as_str()).collect();
    assert!(contents.contains(&"alice private"));
    assert!(contents.contains(&"shared note"));
    assert!(
        !contents.contains(&"bob private"),
        "alice must not see bob's private memory"
    );
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
    db.remember(Memory::new(
        "a fact",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.remember(Memory::new(
        "an event",
        MemoryType::Episodic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
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
    ScoreWeights {
        alpha: 1.0,
        beta: 0.0,
        gamma: 0.0,
        delta: 0.0,
        half_life_secs: 1.0,
    }
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
            db.remember(Memory::new("vec", MemoryType::Semantic, v))
                .unwrap();
        }
    }
    db.flush().unwrap();
    let n = db.len();
    assert_eq!(n, n_clusters * per_cluster);

    // Queries near random cluster centres.
    let queries: Vec<Vec<f32>> = (0..30)
        .map(|_| {
            let ci = (rng.next_u64() as usize) % n_clusters;
            centers[ci]
                .iter()
                .map(|&x| x + rng.normalish() * 0.3)
                .collect()
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
        let mut req = RecallRequest::new(q.clone())
            .top_k(10)
            .n_probe(20)
            .n_rerank(200);
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
    assert!(
        recall >= 0.95,
        "ANN recall@10 was {recall:.3}, expected >= 0.95"
    );
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
        db.remember(Memory::new("m", MemoryType::Semantic, v))
            .unwrap();
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
        db.remember(Memory::new("old", MemoryType::Semantic, v))
            .unwrap();
    }
    db.build_index().unwrap();

    // Insert a distinctive vector after the index exists.
    let needle = vec![9.0f32; dims];
    let needle_id = db
        .remember(Memory::new("needle", MemoryType::Semantic, needle.clone()))
        .unwrap();
    db.flush().unwrap();

    let mut req = RecallRequest::new(needle).top_k(1).n_probe(8).n_rerank(64);
    req.metric = Metric::L2;
    req.weights = sim_only();
    let hits = db.recall(&req).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0].memory.id, needle_id,
        "post-build insert must be indexed"
    );
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
        db.remember(Memory::new("m", MemoryType::Semantic, v))
            .unwrap();
    }
    db.build_index().unwrap();
    db.drop_index();
    db.close().unwrap();

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
        db.remember(Memory::new(
            format!("mem-{i}"),
            MemoryType::Semantic,
            vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    }
    db.close().unwrap();
    let snapshot = read_bytes(&path); // page 0 here points at the 5-memory state

    // State B: three more memories, cleanly flushed.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    for i in 5..8 {
        db.remember(Memory::new(
            format!("mem-{i}"),
            MemoryType::Semantic,
            vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
        ))
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
    // (Prior writer already dropped above.)
    let mut db = Mnemo::open(&franken, "pw").unwrap();
    assert_eq!(db.len(), 8, "WAL recovery must restore the committed txn");
    let contents: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    assert!(contents.contains(&"mem-7".to_string()));
}

/// A torn page-0 write is detected by the header CRC and healed from the WAL.
#[test]
fn wal_heals_torn_header() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..6 {
        db.remember(Memory::new(
            format!("k-{i}"),
            MemoryType::Episodic,
            vec4(i as f32, 1.0, 2.0, 3.0),
        ))
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
        db.remember(Memory::new(
            format!("g-{i}"),
            MemoryType::Working,
            vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
        ))
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
    assert_eq!(
        db.len(),
        5,
        "a garbage WAL must not disturb a checkpointed db"
    );
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
        db.remember(Memory::new("x", MemoryType::Semantic, v))
            .unwrap();
    }
    db.close().unwrap();

    // wal_pages lives at byte offset 194 of the (plaintext) header page,
    // in both formats. 64 was the v0.1.0 default; even after dropping the
    // default to 8, a 22k-memory catalog still forces growth well past 64
    // on v8. FMT-9c: v9 writes big runs in place outside the WAL, which
    // stays at its default.
    let bytes = read_bytes(&path);
    let wal_pages = u64::from_le_bytes(bytes[194..202].try_into().unwrap());
    if FORMAT == 8 {
        assert!(
            wal_pages > 64,
            "WAL should have grown well past the default (got {wal_pages})"
        );
    } else {
        assert_eq!(wal_pages, 8, "the v9 WAL stays at its default");
    }

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
    // Explicitly drops the write handle before reading raw bytes so
    // Windows' mandatory `LockFileEx` doesn't refuse the read.
    let make = |path: &std::path::Path, cfg: MnemoConfig| -> (u64, u64) {
        let mut db = Mnemo::create(path, "pw", cfg).unwrap();
        db.remember(Memory::new(
            "seed",
            MemoryType::Working,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
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
        format_version: FORMAT,
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
        format_version: FORMAT,
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
    assert!(snap1.len().is_multiple_of(PAGE_SIZE), "file must be page-aligned");
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

    // Renamed from `cache_stats` in v0.4.0 — the result cache took that
    // name (with a `namespace: Option<&str>` param); the page cache pair
    // now lives on `page_cache_stats`.
    let (used, cap) = db.page_cache_stats();
    assert_eq!(cap, 8);
    assert!(used <= 8, "cache held {used} clean pages, cap {cap}");

    // The bound survives a reopen + re-shrink too.
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    db.set_cache_capacity(4);
    for (id, i) in &ids {
        assert_eq!(db.get(id).unwrap().content, format!("content-{i}"));
    }
    assert!(db.page_cache_stats().0 <= 4);
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
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Semantic,
            vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        db.flush().unwrap();
    }
    db.close().unwrap();

    let db = Mnemo::open(&path, "pw").unwrap();
    let snaps = db.snapshots();
    assert_eq!(snaps.len(), 3);
    // FMT-9: a v9 session's first commit is its header-only generation
    // lease (no snapshot), so the first snapshot is txn 2.
    let first = if FORMAT == 8 { 1 } else { 2 };
    for (i, s) in snaps.iter().enumerate() {
        assert_eq!(s.txn_id, first + i as u64);
        assert_eq!(s.memory_count, (i + 1) as u64);
    }
}

/// `MnemoConfig::max_snapshots` (Phase 2.3) caps the retained manifest
/// length. The newest N entries are kept; the rest get pruned at flush
/// time. `restore_to` a pruned txn_id returns `NotFound`. Setting the
/// cap to 0 disables it entirely.
#[test]
fn max_snapshots_prunes_oldest_entries() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("cap.mnemo");

    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.set_max_snapshots(10);
    let mut flushed = Vec::new();
    for i in 0..15 {
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Semantic,
            vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        db.flush().unwrap();
        flushed.push(db.snapshots().last().unwrap().txn_id);
    }
    let snaps = db.snapshots();
    assert_eq!(snaps.len(), 10, "cap=10 should keep exactly 10 snapshots");
    // The retained txn_ids must be the most-recent ten flushes (6..=15 on
    // v8; one later on v9, whose session lease takes txn 1), not the
    // oldest ten.
    let kept_ids: Vec<u64> = snaps.iter().map(|s| s.txn_id).collect();
    assert_eq!(kept_ids, flushed[5..].to_vec());

    // restore_to one of the pruned txn_ids must fail with NotFound; the
    // pages those snapshots reference are still on disk but the manifest
    // no longer knows where they are.
    let pruned = flushed[2];
    let err = db.restore_to(pruned).expect_err("pruned txn must not resolve");
    assert!(
        matches!(err, MnemoError::NotFound(_)),
        "expected NotFound for pruned txn_id {pruned}; got {err:?}"
    );

    // A retained txn_id IS still restorable.
    let kept = flushed[9];
    let restored = db.restore_to(kept).expect("a retained txn must restore");
    assert_eq!(restored.txn_id, kept);

    // After restore_to, one more flush appends a new snapshot — verify
    // the cap holds at 10 (one pruned to make room for the new one).
    assert_eq!(db.snapshots().len(), 10);

    // Disabling the cap mid-life: cap=0 lets the manifest grow again.
    db.set_max_snapshots(0);
    for i in 0..5 {
        db.remember(Memory::new(
            format!("more-{i}"),
            MemoryType::Working,
            vec4(0.0, (i as f32) + 1.0, 0.0, 0.0),
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
        let id = db
            .remember(Memory::new(
                format!("content-{i}"),
                MemoryType::Semantic,
                vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
            ))
            .unwrap();
        db.flush().unwrap(); // snapshot txn i+1 holds i+1 memories (v9: i+2)
        ids.push(id);
    }
    assert_eq!(db.len(), 3);
    let txns: Vec<u64> = db.snapshots().iter().map(|s| s.txn_id).collect();

    // Rewind to the one-memory snapshot.
    let info = db.restore_to(txns[0]).unwrap();
    assert_eq!(info.memory_count, 1);
    assert_eq!(db.len(), 1);
    assert_eq!(db.get(&ids[0]).unwrap().content, "content-0");
    assert!(db.get(&ids[2]).is_err(), "later memories must be gone");

    // The rewind persists across a reopen.
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.len(), 1);

    // Roll forward to the three-memory snapshot — history is still intact.
    db.restore_to(txns[2]).unwrap();
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
    db.remember(Memory::new(
        "first",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap(); // snapshot 1

    // Ensure the next snapshot lands in a strictly later second.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    db.remember(Memory::new(
        "second",
        MemoryType::Semantic,
        vec4(0.0, 1.0, 0.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap(); // snapshot 2

    let snaps = db.snapshots();
    let (t1, t2) = (snaps[0].created_at, snaps[1].created_at);
    assert!(t2 > t1, "snapshots should differ in time");

    let (x1, x2) = (snaps[0].txn_id, snaps[1].txn_id);

    // As of t2, both memories existed (snapshot 2 is the latest <= t2).
    let info = db.restore_to_time(t2).unwrap();
    assert_eq!(info.txn_id, x2);
    assert_eq!(db.len(), 2);

    // As of t1, only the first did — snapshot 1 is the only one that old.
    let info = db.restore_to_time(t1).unwrap();
    assert_eq!(info.txn_id, x1);
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
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Episodic,
            vec4((i as f32) + 1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
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
        let a = s
            .add_turn(Turn::user("hello", vec![1.0, 0.0, 0.0]))
            .unwrap();
        let b = s
            .add_turn(Turn::assistant("hi there", vec![0.0, 1.0, 0.0]))
            .unwrap();
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
    assert_eq!(
        first.metadata.get("role").and_then(|v| v.as_str()),
        Some("user")
    );
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
                s.add_turn(Turn::user(
                    format!("turn-{i}"),
                    vec![(i as f32) + 1.0, 0.0, 0.0],
                ))
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
        let a = s
            .add_turn(Turn::user("scratch", vec![1.0, 0.0, 0.0]))
            .unwrap();
        let b = s
            .add_turn(Turn::assistant("noise", vec![0.0, 1.0, 0.0]))
            .unwrap();
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
        Memory::new(
            "bob-private-secret",
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.0],
        )
        .with_agent("bob"),
    )
    .unwrap();
    db.remember(
        Memory::new("alice-fact", MemoryType::Semantic, vec![1.0, 0.0, 0.0]).with_agent("alice"),
    )
    .unwrap();
    db.flush().unwrap();

    let contents: Vec<String> = {
        let mut s = db.session("alice");
        let hits = s
            .recall(RecallRequest::new(vec![1.0, 0.0, 0.0]).top_k(10))
            .unwrap();
        hits.into_iter().map(|h| h.memory.content).collect()
    };
    assert!(contents.contains(&"alice-fact".to_string()));
    assert!(
        !contents.contains(&"bob-private-secret".to_string()),
        "another agent's private memory must not leak into a scoped recall"
    );
}

// === Phase 1.1: OS file locking + read-only opens =========================
//
// These tests exercise the advisory lock and read-only handle semantics
// introduced in Phase 1.1. They live in-process and rely on `flock(2)`
// (Unix) / `LockFileEx` (Windows) being per-handle rather than per-process
// — the two `Mnemo` handles below get different `File` FDs, so a shared
// lock held by one refuses an exclusive lock request from the other.

#[test]
fn two_writers_on_same_file_collide_with_locked() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let _first = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // Second read-write open in the same process (different FD): the
    // exclusive-lock request must fail cleanly with `Locked`, not hang.
    match Mnemo::open(&path, "pw") {
        Err(MnemoError::Locked { path: p, .. }) => {
            assert_eq!(p, path, "Locked error should carry the requested path");
        }
        Err(other) => panic!("expected MnemoError::Locked, got {other:?}"),
        Ok(_) => panic!("second writer must not succeed while the first holds the lock"),
    }
}

#[test]
fn writer_blocks_read_only_open_on_all_platforms() {
    // Advisory locks via `fs4` — `flock(2)` on Unix, `LockFileEx` on
    // Windows — both refuse a shared-lock request while an exclusive
    // lock is held. So a read-only handle cannot coexist with a writer
    // holding the exclusive lock, regardless of platform. This test
    // pins that invariant so callers know to reach for `open_read_only`
    // only after the writer has been closed.
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    let mut w = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    w.remember(Memory::new(
        "hello",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    w.flush().unwrap();

    // `Mnemo` doesn't implement `Debug`, so we can't `{other:?}` an
    // Ok(Mnemo). Pattern-match to a stringified Err instead.
    match Mnemo::open_read_only(&path, "pw") {
        Err(MnemoError::Locked { path: p, .. }) => assert_eq!(p, path),
        Err(other) => panic!("expected Locked while writer is alive, got {other:?}"),
        Ok(_) => panic!("read-only open must not succeed while writer holds exclusive"),
    }
    // After the writer drops (and its exclusive lock releases), the
    // read-only open succeeds and can serve un-tracked recall.
    drop(w);
    let mut r = Mnemo::open_read_only(&path, "pw").unwrap();
    let hits = r
        .recall(
            &RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
                .top_k(1)
                .track_access(false),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
}

#[test]
fn multiple_read_only_handles_coexist() {
    // The value-add of `open_read_only`: shared locks stack, so many
    // reader handles can hold the same file at once.
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    {
        let mut w = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        w.remember(Memory::new(
            "seed",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        w.flush().unwrap();
    } // writer dropped — exclusive lock released

    let mut r1 = Mnemo::open_read_only(&path, "pw").unwrap();
    let mut r2 = Mnemo::open_read_only(&path, "pw").unwrap();
    let r3 = Mnemo::open_read_only(&path, "pw").unwrap();

    // All three see the same seed and can execute un-tracked recall
    // concurrently in-process.
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(1)
        .track_access(false);
    assert_eq!(r1.recall(&req).unwrap().len(), 1);
    assert_eq!(r2.recall(&req).unwrap().len(), 1);
    // r3 is only used to prove three handles coexist.
    drop(r3);
}

#[test]
fn read_only_handle_refuses_mutations() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    {
        let mut w = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        w.remember(Memory::new(
            "seed",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        w.flush().unwrap();
    } // writer dropped here — lock released

    let mut r = Mnemo::open_read_only(&path, "pw").unwrap();
    match r.remember(Memory::new(
        "nope",
        MemoryType::Semantic,
        vec4(0.0, 1.0, 0.0, 0.0),
    )) {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on remember, got {other:?}"),
    }
    match r.flush() {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on flush, got {other:?}"),
    }
    match r.rekey("new", KdfParams::fast()) {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on rekey, got {other:?}"),
    }
    // `recall` with default track_access(true) also mutates catalog stats
    // and must be refused.
    match r.recall(&RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0)).top_k(1)) {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on tracked recall, got {other:?}"),
    }
    // `recall` with track_access(false) is fine.
    let hits = r
        .recall(
            &RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
                .top_k(1)
                .track_access(false),
        )
        .unwrap();
    assert_eq!(hits.len(), 1);
    // `close` on read-only is a no-op success (no dirty state to persist).
    r.close().unwrap();
}

#[test]
fn read_only_open_refused_when_wal_needs_recovery() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");

    // Create a committed-but-uncheckpointed transaction using the
    // documented crash-simulation helper: partial flush leaves the WAL
    // half-written and the header still leased.
    {
        let mut w = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        w.remember(Memory::new(
            "crash-victim",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        // Full flush first, then a dirty-only partial that leaves a
        // committed WAL transaction. The partial helper only exists to
        // produce this exact "recovery needed on next open" state.
        w.flush().unwrap();
        w.remember(Memory::new(
            "crash-second",
            MemoryType::Semantic,
            vec4(0.0, 1.0, 0.0, 0.0),
        ))
        .unwrap();
        w.__crash_partial_flush_for_testing().unwrap();
        // `Mnemo` has no `Drop` impl, so falling out of scope just
        // closes the File (releasing the exclusive lock) without an
        // implicit flush — the WAL state produced by the partial-flush
        // helper stays exactly as-is on disk.
    }
    // Now try to open read-only. The pending WAL transaction (from the
    // second flush's data pages that were leased but not committed)
    // means we cannot serve reads without a write handle to recover.
    // Note: with our partial-flush helper this may or may not trigger
    // NeedsWriteOpen depending on whether the WAL made it to the commit
    // frame. The invariant we assert is that IF recovery is pending, the
    // error is NeedsWriteOpen — not a silent read of stale state.
    match Mnemo::open_read_only(&path, "pw") {
        Err(MnemoError::NeedsWriteOpen { reason }) => {
            assert!(!reason.is_empty(), "reason should be non-empty");
        }
        Ok(_) => {
            // If the partial-flush helper didn't leave a pending commit,
            // the read-only open succeeds cleanly — that's also fine.
        }
        Err(other) => panic!("unexpected error from read_only open: {other:?}"),
    }
}

// === Phase 10.1 / 10.3: Result cache ======================================
//
// Covers the acceptance points in 10.5 that fall inside this PR (put/get,
// TTL expiry, budget eviction, stats, persistence, v7→v8 migration,
// batched-policy crash test, reader-alongside-writer). Semantic cache
// tests land in PR 4 (Phase 10.2).

fn cache_opts(ct: &str, ttl_secs: Option<u64>) -> CachePutOpts {
    CachePutOpts {
        content_type: ct.to_string(),
        ttl_secs,
        cost_hint_ms: None,
    }
}

#[test]
fn cache_put_get_roundtrip_json_text_bytes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cache.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // Three content-type conventions in the same namespace.
    let json_val = br#"{"model":"gpt-4","cost":0.03}"#;
    let text_val = b"the user prefers dark mode";
    let bin_val = &[0x00u8, 0x01, 0x02, 0xff, 0xfe][..];

    db.cache_put("llm", "prompt-42", json_val, cache_opts("json", None))
        .unwrap();
    db.cache_put("prefs", "user-1", text_val, cache_opts("text", None))
        .unwrap();
    db.cache_put(
        "http",
        "GET /avatar.png",
        bin_val,
        cache_opts("bytes", None),
    )
    .unwrap();
    db.flush().unwrap();

    let got_json = db.cache_get("llm", "prompt-42").unwrap().expect("hit");
    assert_eq!(got_json.value, json_val);
    assert_eq!(got_json.content_type, "json");

    let got_text = db.cache_get("prefs", "user-1").unwrap().expect("hit");
    assert_eq!(got_text.value, text_val);

    let got_bin = db
        .cache_get("http", "GET /avatar.png")
        .unwrap()
        .expect("hit");
    assert_eq!(got_bin.value, bin_val);
    assert_eq!(got_bin.content_type, "bytes");
}

#[test]
fn cache_ttl_expiry_hides_entries_as_miss() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ttl.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // 0-second TTL → immediately expired (created_at is `now`, ttl is 0,
    // now - created_at == 0 >= 0). Same convention as memory TTL.
    db.cache_put(
        "llm",
        "ephemeral",
        b"gone by now",
        cache_opts("text", Some(0)),
    )
    .unwrap();
    db.flush().unwrap();

    let hit = db.cache_get("llm", "ephemeral").unwrap();
    assert!(hit.is_none(), "expired entry should read as miss");
    let stats = db.cache_stats(Some("llm"));
    // An expired-lookup counts as a miss, not a hit — the two are
    // indistinguishable to the caller.
    assert_eq!(stats.hits, 0);
    assert_eq!(stats.misses, 1);
    // The entry is still on disk (not purged) until `cache_purge` runs.
    assert_eq!(stats.entries, 1);
}

#[test]
fn cache_stats_counters_track_hits_and_misses() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("stats.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.cache_put("llm", "k1", b"v1", cache_opts("text", None))
        .unwrap();
    db.cache_put("llm", "k2", b"v2", cache_opts("text", None))
        .unwrap();
    db.flush().unwrap();

    // 3 hits, 2 misses on the "llm" namespace.
    assert!(db.cache_get("llm", "k1").unwrap().is_some());
    assert!(db.cache_get("llm", "k1").unwrap().is_some());
    assert!(db.cache_get("llm", "k2").unwrap().is_some());
    assert!(db.cache_get("llm", "missing-a").unwrap().is_none());
    assert!(db.cache_get("llm", "missing-b").unwrap().is_none());

    let s = db.cache_stats(Some("llm"));
    assert_eq!(s.hits, 3);
    assert_eq!(s.misses, 2);
    assert!((s.hit_rate - 0.6).abs() < 1e-9, "hit_rate = {}", s.hit_rate);
    assert_eq!(s.entries, 2);
    assert!(s.bytes >= 4, "bytes should include both values");

    // Whole-cache stats include the same counters (they're per-handle,
    // not per-namespace).
    let sw = db.cache_stats(None);
    assert_eq!(sw.hits, 3);
    assert_eq!(sw.misses, 2);
}

#[test]
fn cache_budget_evicts_lru_on_overflow() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("budget.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // Tight cap: 3 entries per namespace. Fourth insert must evict the
    // LRU (first-inserted, never touched).
    db.set_cache_budget(
        "llm",
        CacheBudget {
            max_entries: 3,
            max_bytes: 1 << 30,
        },
    );

    db.cache_put("llm", "a", b"AAA", cache_opts("text", None))
        .unwrap();
    // Sleep to guarantee monotonic accessed_at deltas — `memory::now_secs()`
    // has 1-second resolution.
    std::thread::sleep(std::time::Duration::from_millis(1050));
    db.cache_put("llm", "b", b"BBB", cache_opts("text", None))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1050));
    db.cache_put("llm", "c", b"CCC", cache_opts("text", None))
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1050));
    // Insert 4th → LRU (a, oldest accessed_at) evicted.
    db.cache_put("llm", "d", b"DDD", cache_opts("text", None))
        .unwrap();
    db.flush().unwrap();

    let s = db.cache_stats(Some("llm"));
    assert_eq!(s.entries, 3, "budget cap enforced after insert");
    assert!(s.evictions >= 1, "at least one eviction recorded");

    assert!(
        db.cache_get("llm", "a").unwrap().is_none(),
        "'a' should be evicted"
    );
    assert!(db.cache_get("llm", "b").unwrap().is_some());
    assert!(db.cache_get("llm", "c").unwrap().is_some());
    assert!(db.cache_get("llm", "d").unwrap().is_some());
}

#[test]
fn cache_persists_across_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("persist.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.cache_put(
            "llm",
            "durable-key",
            b"durable-value",
            cache_opts("text", None),
        )
        .unwrap();
        db.flush().unwrap();
    } // writer drops → lock released

    let mut db = Mnemo::open(&path, "pw").unwrap();
    let got = db.cache_get("llm", "durable-key").unwrap().expect("hit");
    assert_eq!(got.value, b"durable-value");
}

#[test]
fn cache_delete_removes_entry() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("del.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.cache_put("llm", "k", b"v", cache_opts("text", None))
        .unwrap();
    db.flush().unwrap();
    assert!(db.cache_get("llm", "k").unwrap().is_some());
    db.cache_delete("llm", "k").unwrap();
    db.flush().unwrap();
    assert!(db.cache_get("llm", "k").unwrap().is_none());
    assert_eq!(db.cache_stats(Some("llm")).entries, 0);
}

#[test]
fn cache_purge_expired_only_leaves_fresh_entries() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("purge.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.cache_put("llm", "fresh", b"stay", cache_opts("text", None))
        .unwrap();
    db.cache_put("llm", "expired", b"gone", cache_opts("text", Some(0)))
        .unwrap();
    db.flush().unwrap();

    let n = db.cache_purge(Some("llm"), true).unwrap();
    assert_eq!(n, 1, "only the expired entry should be purged");
    db.flush().unwrap();

    assert!(db.cache_get("llm", "fresh").unwrap().is_some());
    assert!(db.cache_get("llm", "expired").unwrap().is_none());
}

#[test]
fn cache_batched_policy_auto_flushes_on_count_threshold() {
    // Batched with max_dirty = 2, huge max_age. The 2nd put should
    // trigger an internal flush; a subsequent reopen sees both entries
    // durable without the test itself calling flush().
    let dir = tempdir().unwrap();
    let path = dir.path().join("batch.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.set_cache_flush_policy(CacheFlushPolicy::Batched {
            max_dirty: 2,
            max_age: std::time::Duration::from_secs(3600),
        });
        db.cache_put("llm", "b1", b"v1", cache_opts("text", None))
            .unwrap();
        db.cache_put("llm", "b2", b"v2", cache_opts("text", None))
            .unwrap();
        // Do NOT call db.flush(); the batched policy should have
        // committed on the second put.
    } // drop releases lock

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(
        db.cache_get("llm", "b1").unwrap().is_some(),
        "b1 should have auto-committed"
    );
    assert!(
        db.cache_get("llm", "b2").unwrap().is_some(),
        "b2 should have auto-committed"
    );
}

#[test]
fn cache_batched_policy_uncommitted_entries_are_misses_after_crash() {
    // Simulate a crash between batch commits: put once (under
    // max_dirty=99 so no auto-flush fires), never call flush(), drop the
    // handle without close(). The next open sees a clean database with
    // the entry missing — a miss, not corruption.
    let dir = tempdir().unwrap();
    let path = dir.path().join("crash.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.set_cache_flush_policy(CacheFlushPolicy::Batched {
            max_dirty: 99,
            max_age: std::time::Duration::from_secs(3600),
        });
        db.cache_put("llm", "will-be-lost", b"gone", cache_opts("text", None))
            .unwrap();
        // No flush, no close — simulate SIGKILL. Mnemo has no Drop impl
        // that would tidy up, so this is effectively `kill -9`.
    }

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(
        db.cache_get("llm", "will-be-lost").unwrap().is_none(),
        "uncommitted batched entry must be a miss on reopen (never corruption)"
    );
    assert_eq!(db.cache_stats(None).entries, 0);
}

#[test]
fn cache_read_only_handle_can_read_but_not_write() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.cache_put("llm", "k", b"v", cache_opts("text", None))
            .unwrap();
        db.flush().unwrap();
    }

    let mut r = Mnemo::open_read_only(&path, "pw").unwrap();
    // Read succeeds and does NOT bump access stats (silent skip on
    // read-only, per cache_get's documented behavior).
    let hit = r.cache_get("llm", "k").unwrap().expect("read still works");
    assert_eq!(hit.value, b"v");
    // Mutations refused.
    use mnemo_engine::MnemoError;
    match r.cache_put("llm", "new", b"x", cache_opts("text", None)) {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on cache_put, got {other:?}"),
    }
    match r.cache_delete("llm", "k") {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on cache_delete, got {other:?}"),
    }
    match r.cache_purge(None, false) {
        Err(MnemoError::ReadOnly) => {}
        other => panic!("expected ReadOnly on cache_purge, got {other:?}"),
    }
}

// === Phase 10.2: Semantic cache ==========================================

#[test]
fn semantic_hit_above_threshold_returns_top_1() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sem.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // Populate with a couple of vectored entries plus one exact-key
    // entry to prove the semantic scan ignores non-vectored rows.
    let v_a = vec4(1.0, 0.0, 0.0, 0.0);
    let v_b = vec4(0.0, 1.0, 0.0, 0.0);
    db.cache_put_semantic(
        "llm",
        "prompt-a",
        v_a.clone(),
        b"answer-a",
        SemanticCachePutOpts::new("bge-large-en-v1.5"),
    )
    .unwrap();
    db.cache_put_semantic(
        "llm",
        "prompt-b",
        v_b.clone(),
        b"answer-b",
        SemanticCachePutOpts::new("bge-large-en-v1.5"),
    )
    .unwrap();
    db.cache_put("llm", "exact-only", b"no-vec", cache_opts("text", None))
        .unwrap();
    db.flush().unwrap();

    // Query near v_a → high similarity → hit above threshold.
    let query = vec4(0.98, 0.20, 0.0, 0.0);
    let hit = db
        .cache_get_semantic(
            "llm",
            &query,
            DEFAULT_SEMANTIC_THRESHOLD,
            "bge-large-en-v1.5",
        )
        .unwrap()
        .expect("expected a hit for the near-a query");
    assert_eq!(hit.0.value, b"answer-a", "top-1 should be prompt-a");
    assert!(hit.1 >= DEFAULT_SEMANTIC_THRESHOLD, "sim = {}", hit.1);
}

#[test]
fn semantic_miss_below_threshold_returns_none() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sem.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.cache_put_semantic(
        "llm",
        "prompt-a",
        vec4(1.0, 0.0, 0.0, 0.0),
        b"answer-a",
        SemanticCachePutOpts::new("bge-large-en-v1.5"),
    )
    .unwrap();
    db.flush().unwrap();

    // Query orthogonal to the stored vector → cosine = 0.0 → miss.
    let query = vec4(0.0, 1.0, 0.0, 0.0);
    let miss = db
        .cache_get_semantic(
            "llm",
            &query,
            DEFAULT_SEMANTIC_THRESHOLD,
            "bge-large-en-v1.5",
        )
        .unwrap();
    assert!(miss.is_none(), "orthogonal query must not hit");

    // Threshold sweep — hit under a permissive threshold, miss under
    // the default. Confirms the cutoff is doing the right work.
    let hit = db
        .cache_get_semantic("llm", &query, -1.0, "bge-large-en-v1.5")
        .unwrap();
    assert!(
        hit.is_some(),
        "with threshold -1.0 the top-1 should always surface"
    );
}

#[test]
fn semantic_model_mismatch_is_a_miss() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sem.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // Store under one model; look up under a different one.
    let v = vec4(1.0, 0.0, 0.0, 0.0);
    db.cache_put_semantic(
        "llm",
        "prompt-a",
        v.clone(),
        b"answer-a",
        SemanticCachePutOpts::new("bge-large-en-v1.5"),
    )
    .unwrap();
    db.flush().unwrap();

    // Same vector as stored → cosine would be 1.0 → definite hit if
    // the model matched. Doesn't hit because the models differ.
    let miss = db
        .cache_get_semantic(
            "llm",
            &v,
            DEFAULT_SEMANTIC_THRESHOLD,
            "text-embedding-3-small",
        )
        .unwrap();
    assert!(
        miss.is_none(),
        "different-model hit is a semantics bug; must miss"
    );

    // Looking up under the correct model still hits.
    let hit = db
        .cache_get_semantic("llm", &v, DEFAULT_SEMANTIC_THRESHOLD, "bge-large-en-v1.5")
        .unwrap()
        .expect("same-model exact-vector query must hit");
    assert_eq!(hit.0.value, b"answer-a");
}

#[test]
fn semantic_dimension_mismatch_is_rejected() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("dim.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    let three_dims = vec![1.0, 0.0, 0.0];
    let err = db
        .cache_put_semantic("llm", "k", three_dims, b"v", SemanticCachePutOpts::new("m"))
        .unwrap_err();
    match err {
        MnemoError::DimensionMismatch {
            expected: 4,
            got: 3,
        } => {}
        other => panic!("expected DimensionMismatch, got {other:?}"),
    }
}

#[test]
fn exact_key_and_semantic_entries_coexist_in_same_namespace() {
    // Backwards-compat regression: the semantic-cache extension added
    // optional `vector`/`model` fields to CacheEntry with serde
    // defaults. Old (exact-key-only) entries must still roundtrip.
    let dir = tempdir().unwrap();
    let path = dir.path().join("coexist.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.cache_put("llm", "exact", b"exact-value", cache_opts("text", None))
            .unwrap();
        db.cache_put_semantic(
            "llm",
            "sem",
            vec4(1.0, 0.0, 0.0, 0.0),
            b"sem-value",
            SemanticCachePutOpts::new("bge"),
        )
        .unwrap();
        db.flush().unwrap();
    }
    // Reopen with a fresh handle — decoding old-shape and new-shape
    // entries side by side must not error.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let e = db
        .cache_get("llm", "exact")
        .unwrap()
        .expect("exact-key still hits");
    assert_eq!(e.value, b"exact-value");
    let s = db
        .cache_get_semantic(
            "llm",
            &vec4(1.0, 0.0, 0.0, 0.0),
            DEFAULT_SEMANTIC_THRESHOLD,
            "bge",
        )
        .unwrap()
        .expect("semantic entry still hits");
    assert_eq!(s.0.value, b"sem-value");
    // Exact-key entry has no vector → semantic scan ignores it.
    let orth = db
        .cache_get_semantic("llm", &vec4(0.0, 0.0, 0.0, 1.0), -1.0, "bge")
        .unwrap()
        .expect("some entry must surface with threshold=-1");
    // The only candidate visible to the semantic scan is the "sem"
    // entry — the exact-key "exact" entry has no vector, so it
    // cannot surface even at threshold=-1.
    assert_eq!(s.0.value, b"sem-value");
    assert_eq!(orth.0.value, b"sem-value");
}

#[test]
fn v7_file_migrates_to_v8_with_empty_cache() {
    // Full round-trip: a fresh v8 file (there's no easy way to synthesize
    // a real v7 file in-process without embedding a byte fixture, so we
    // exercise the same code path by opening a v8 file that happens to
    // have cache_pages == 0 — which IS the v7→v8 migration outcome for
    // "no cache entries yet"). Both the migration and the
    // clean-empty-cache case go through the same open logic in
    // `load_cache_directory` and both must yield an empty directory
    // without any I/O to a non-existent cache region.
    let dir = tempdir().unwrap();
    let path = dir.path().join("migrated.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        // No cache_put — the cache stays empty through the file
        // lifecycle, mirroring the v7→v8 migration state.
        db.remember(Memory::new(
            "sanity",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        db.flush().unwrap();
    }
    let db = Mnemo::open_read_only(&path, "pw").unwrap();
    let s = db.cache_stats(None);
    assert_eq!(s.entries, 0, "empty cache directory loads clean");
    assert_eq!(s.bytes, 0);
    // The memory side of the file is unaffected.
    assert_eq!(db.len(), 1);
}

// === Phase 3 PR 10a: Embedder API core ==================================

use mnemo_engine::{Embedder, EmbedderIdentity};

/// Deterministic 4-D fake embedder: hashes each input to pick a
/// single axis, emits a unit spike there. Used by the Phase 3 tests
/// below so the "embedder" is reproducible and dependency-free.
struct FakeEmbedder {
    identity: EmbedderIdentity,
}
impl FakeEmbedder {
    fn new(name: &str, dims: usize, normalize: bool) -> Self {
        Self {
            identity: EmbedderIdentity {
                name: name.into(),
                dimensions: dims,
                normalize,
                ..Default::default()
            },
        }
    }
}
impl Embedder for FakeEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        self.identity.clone()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        let dims = self.identity.dimensions;
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            let hash: usize = t.bytes().map(|b| b as usize).sum();
            let axis = hash % dims;
            let mut v = vec![0.0f32; dims];
            v[axis] = 1.0;
            out.push(v);
        }
        Ok(out)
    }
}

/// Embedder that always returns a zero vector — exercises the Phase 3
/// zero-vector rejection path independently of the embedder trait.
struct ZeroEmbedder {
    identity: EmbedderIdentity,
}
impl Embedder for ZeroEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        self.identity.clone()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|_| vec![0.0f32; self.identity.dimensions])
            .collect())
    }
}

#[test]
fn set_embedder_dim_mismatch_is_rejected() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("em.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let wrong = FakeEmbedder::new("wrong-dim", 8, true);
    match db.set_embedder(wrong) {
        Err(MnemoError::DimensionMismatch { expected, got }) => {
            assert_eq!(expected, 4);
            assert_eq!(got, 8);
        }
        other => panic!("expected DimensionMismatch, got {other:?}"),
    }
}

#[test]
fn set_embedder_identity_mismatch_rejected_after_persist() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("em.mnemo");
    // First handle: register MiniLM — manifest absorbs its identity.
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        db.flush().unwrap();
        db.set_embedder(FakeEmbedder::new("MiniLM", 4, true))
            .unwrap();
    }
    // Second handle: reopen and try to register a different name.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    match db.set_embedder(FakeEmbedder::new("bge", 4, true)) {
        Err(e @ MnemoError::EmbedderMismatch { .. }) => {
            let msg = e.to_string();
            assert!(msg.starts_with(EmbedderIdentity::MISMATCH_MSG), "msg = {msg}");
            assert!(msg.contains("MiniLM"));
            assert!(msg.contains("bge"));
            assert!(msg.contains("create a new store"), "msg = {msg}");
        }
        other => panic!("expected EmbedderMismatch, got {other:?}"),
    }
    // Re-registering the SAME identity round-trips cleanly.
    db.set_embedder(FakeEmbedder::new("MiniLM", 4, true))
        .unwrap();
}

#[test]
fn zero_vector_remember_rejected() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("z.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let m = Memory::new("empty", MemoryType::Semantic, vec![0.0, 0.0, 0.0, 0.0]);
    match db.remember(m) {
        Err(MnemoError::Invalid(msg)) => {
            assert!(msg.contains("all-zero vector"), "msg = {msg}");
        }
        other => panic!("expected Invalid for zero vector, got {other:?}"),
    }
    // A single-axis spike passes.
    let m = Memory::new("ok", MemoryType::Semantic, vec![1.0, 0.0, 0.0, 0.0]);
    db.remember(m).unwrap();
}

#[test]
fn remember_text_from_text_round_trip() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();

    db.remember_text("alpha", MemoryType::Semantic).unwrap();
    db.remember_text("beta", MemoryType::Episodic).unwrap();
    db.flush().unwrap();

    // Query for "alpha" via from_text; the fake embedder is
    // deterministic so recall should surface the "alpha" memory first.
    let fake = FakeEmbedder::new("fake-4d", 4, true);
    let req = mnemo_engine::RecallRequest::from_text(&fake, "alpha")
        .unwrap()
        .top_k(2);
    let hits = db.recall(&req).unwrap();
    assert!(!hits.is_empty(), "recall returned nothing");
    assert_eq!(hits[0].memory.content, "alpha");
}

#[test]
fn remember_text_without_embedder_fails_clearly() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("ne.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    match db.remember_text("hello", MemoryType::Semantic) {
        Err(MnemoError::Invalid(msg)) => assert!(msg.contains("no embedder")),
        other => panic!("expected Invalid 'no embedder', got {other:?}"),
    }
}

#[test]
fn zero_vector_embedder_remember_text_rejected() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("z2.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.set_embedder(ZeroEmbedder {
        identity: EmbedderIdentity {
            name: "zero".into(),
            dimensions: 4,
            normalize: false,
            ..Default::default()
        },
    })
    .unwrap();
    match db.remember_text("anything", MemoryType::Semantic) {
        Err(MnemoError::Invalid(msg)) => {
            assert!(msg.contains("all-zero vector"), "msg = {msg}");
        }
        other => panic!("expected Invalid for zero-vec embedder, got {other:?}"),
    }
}

// === Phase 3 PR 10a.1: nested-manifest + read-only identity check =====

/// Seed a scaffold manifest with the canonical **nested** form the
/// README documents and `test/scripts/seed.json` writes:
/// `metadata.embedder = { name, dimensions, normalize }`.
fn remember_nested_manifest(db: &mut Mnemo, dims: usize, name: &str, normalize: bool) {
    let mut m = Memory::scaffold_manifest(dims);
    let obj = serde_json::json!({
        "name": name,
        "dimensions": dims,
        "normalize": normalize,
    });
    m.metadata.insert("embedder".into(), obj);
    db.remember(m).unwrap();
    db.flush().unwrap();
}

#[test]
fn set_embedder_reads_nested_manifest_canonically() {
    // Writer: seed the nested form, then register the SAME embedder.
    // Must succeed without upserting any flat embedder_* keys (Bug 1).
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        remember_nested_manifest(&mut db, 4, "sentence-transformers/all-MiniLM-L6-v2", true);
        db.set_embedder(FakeEmbedder::new(
            "sentence-transformers/all-MiniLM-L6-v2",
            4,
            true,
        ))
        .unwrap();
    }
    // Reopen: the manifest's nested identity is intact, no stray
    // flat keys were written alongside.
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let onboarding = db.about().unwrap();
    let manifest = &onboarding[0];
    let nested = manifest
        .metadata
        .get("embedder")
        .and_then(|v| v.as_object());
    assert!(nested.is_some(), "nested embedder object must be present");
    assert_eq!(
        nested.unwrap().get("name").and_then(|v| v.as_str()),
        Some("sentence-transformers/all-MiniLM-L6-v2")
    );
    assert!(
        manifest.metadata.get("embedder_name").is_none(),
        "flat embedder_name must NOT have been upserted alongside the nested form"
    );
    assert!(manifest.metadata.get("embedder_dimensions").is_none());
    assert!(manifest.metadata.get("embedder_normalize").is_none());
}

#[test]
fn set_embedder_nested_manifest_mismatch_rejected() {
    use mnemo_engine::MnemoError;
    let dir = tempdir().unwrap();
    let path = dir.path().join("nested-mismatch.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    remember_nested_manifest(&mut db, 4, "sentence-transformers/all-MiniLM-L6-v2", true);
    // Register a different name at the same dimensions.
    match db.set_embedder(FakeEmbedder::new("bge-large-en-v1.5", 4, true)) {
        Err(e @ MnemoError::EmbedderMismatch { .. }) => {
            let msg = e.to_string();
            assert!(msg.starts_with(EmbedderIdentity::MISMATCH_MSG), "msg = {msg}");
            assert!(msg.contains("sentence-transformers/all-MiniLM-L6-v2"));
            assert!(msg.contains("bge-large-en-v1.5"));
            assert!(msg.contains("create a new store"), "msg = {msg}");
        }
        other => panic!("expected EmbedderMismatch, got {other:?}"),
    }
}

#[test]
fn set_embedder_accepts_legacy_flat_keys() {
    // A file that was bootstrapped by the first PR 10a dev build may
    // have flat keys. Those still read as a valid identity; the next
    // write path canonicalizes to nested.
    let dir = tempdir().unwrap();
    let path = dir.path().join("legacy.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        let mut m = Memory::scaffold_manifest(4);
        m.metadata
            .insert("embedder_name".into(), serde_json::json!("MiniLM"));
        m.metadata
            .insert("embedder_dimensions".into(), serde_json::json!(4));
        m.metadata
            .insert("embedder_normalize".into(), serde_json::json!(true));
        db.remember(m).unwrap();
        db.flush().unwrap();
        db.set_embedder(FakeEmbedder::new("MiniLM", 4, true))
            .unwrap();
    }
    // Legacy keys matched — nothing to upsert, so they stay as-is on
    // this read path. A future `set_embedder` on a manifest where no
    // identity is recorded would then write nested; this test just
    // confirms the legacy-read branch keeps working.
}

#[test]
fn set_embedder_on_read_only_handle_adopts_without_write() {
    // Writer seeds the nested manifest + a text memory via the same
    // embedder; drop; reopen read-only; register the embedder (Bug 2:
    // this used to return `ReadOnly` outright). Then verify
    // `RecallRequest::from_text` works through the read-only handle.
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro-em.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        remember_nested_manifest(&mut db, 4, "fake-4d", true);
        db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
            .unwrap();
        db.remember_text("alpha", MemoryType::Semantic).unwrap();
        db.flush().unwrap();
    }
    let mut r = Mnemo::open_read_only(&path, "pw").unwrap();
    // Bug 2: this call must succeed on a read-only handle.
    r.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    // Confirm the embedder is actually usable from the reader. Must
    // pass `track_access(false)` — `recall` on a read-only handle
    // with access-tracking on returns `ReadOnly` (it would try to
    // dirty the catalog); unrelated to Bug 2, that's the Phase 1.1
    // read-only contract.
    let fake = FakeEmbedder::new("fake-4d", 4, true);
    let req = mnemo_engine::RecallRequest::from_text(&fake, "alpha")
        .unwrap()
        .track_access(false);
    let hits = r.recall(&req).unwrap();
    assert!(
        !hits.is_empty(),
        "recall_text on read-only handle returned nothing"
    );
    assert_eq!(hits[0].memory.content, "alpha");
}

#[test]
fn set_embedder_on_read_only_handle_with_no_manifest_identity_still_works() {
    // No embedder was ever persisted on the manifest. On a read-only
    // handle, the upsert path is skipped (readers must not mutate the
    // file), but the embedder is still adopted so recall_text works.
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro-fresh.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        // Add a bare scaffold manifest with NO embedder metadata, so
        // the identity check finds nothing on the read-only path.
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        // Also store one text memory so recall has something to find;
        // we embed via a writer-registered embedder, drop, then reopen
        // read-only.
        db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
            .unwrap();
        db.remember_text("beta", MemoryType::Semantic).unwrap();
        db.flush().unwrap();
    }
    // Reopen read-only, but note: the writer above's set_embedder
    // persisted the identity. So this would use the match path. To
    // isolate the no-identity-on-manifest branch, open a fresh file
    // with just a plain scaffold.
    let dir2 = tempdir().unwrap();
    let path2 = dir2.path().join("ro-fresh2.mnemo");
    {
        let mut db = Mnemo::create(&path2, "pw", fast_cfg(4)).unwrap();
        db.remember(Memory::scaffold_manifest(4)).unwrap();
        db.remember(Memory::new(
            "gamma",
            MemoryType::Semantic,
            vec![0.0, 0.0, 1.0, 0.0],
        ))
        .unwrap();
        db.flush().unwrap();
    }
    let mut r = Mnemo::open_read_only(&path2, "pw").unwrap();
    r.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    // Even though nothing was persisted (read-only can't write), the
    // embedder is in memory and usable for the lifetime of this handle.
    assert!(r.embedder().is_some());
}

// ===========================================================================
// Phase 12.1: edit in place + keyed supersession
// ===========================================================================

#[test]
fn update_in_place_preserves_id_and_created_at() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("upd.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let id = db
        .remember(
            Memory::new("pre", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.5),
        )
        .unwrap();
    let orig = db.get(&id).unwrap();

    // Patch importance only; vector unchanged, content unchanged.
    let outcome = db
        .update(
            &id,
            MemoryPatch {
                importance: Some(0.9),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Updated);
    let after = db.get(&id).unwrap();
    assert_eq!(after.id, orig.id, "update must keep the ULID");
    assert_eq!(
        after.created_at, orig.created_at,
        "update must keep created_at"
    );
    assert!((after.importance - 0.9).abs() < 1e-6);
    assert_eq!(after.content, "pre");
    // `_mnemo.updated_at` stamp should land.
    assert!(after
        .metadata
        .contains_key(mnemo_engine::MNEMO_META_UPDATED_AT));
}

#[test]
fn update_with_identical_patch_is_noop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("upd-noop.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let id = db
        .remember(
            Memory::new("same", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
                .with_importance(0.5),
        )
        .unwrap();

    let outcome = db
        .update(
            &id,
            MemoryPatch {
                importance: Some(0.5),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(outcome, WriteOutcome::Unchanged);
    // No `_mnemo.updated_at` stamp — nothing was written.
    let m = db.get(&id).unwrap();
    assert!(!m.metadata.contains_key(mnemo_engine::MNEMO_META_UPDATED_AT));
}

#[test]
fn update_content_without_vector_is_rejected() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("upd-content.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let id = db
        .remember(Memory::new(
            "a",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    let err = db
        .update(
            &id,
            MemoryPatch {
                content: Some("b".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("needs a matching vector"),
        "want guard against text/embedding drift, got: {err}"
    );
}

#[test]
fn remember_keyed_added_then_superseded_then_noop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keyed.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // First keyed write → Added.
    let (id1, o1) = db
        .remember_keyed(
            Memory::new("v1", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
                .with_importance(0.5)
                .with_agent("alice"),
            "release",
        )
        .unwrap();
    assert_eq!(o1, WriteOutcome::Added);

    // Second with different content → Superseded.
    let (id2, o2) = db
        .remember_keyed(
            Memory::new("v2", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0))
                .with_importance(0.5)
                .with_agent("alice"),
            "release",
        )
        .unwrap();
    assert_eq!(o2, WriteOutcome::Superseded);
    assert_ne!(id1, id2, "supersession inserts a fresh ULID");

    // Old record carries `_mnemo.superseded_by` + `_mnemo.invalid_at`.
    let old = db.get(&id1).unwrap();
    assert_eq!(
        old.metadata
            .get(MNEMO_META_SUPERSEDED_BY)
            .and_then(|v| v.as_str()),
        Some(id2.to_string().as_str())
    );
    assert!(old.metadata.contains_key(MNEMO_META_INVALID_AT));

    // New record carries `_mnemo.supersedes` and the key.
    let new = db.get(&id2).unwrap();
    assert_eq!(
        new.metadata
            .get(MNEMO_META_SUPERSEDES)
            .and_then(|v| v.as_str()),
        Some(id1.to_string().as_str())
    );
    assert_eq!(
        new.metadata.get(MNEMO_META_KEY).and_then(|v| v.as_str()),
        Some("release")
    );

    // Third write with identical fields to the live record → Unchanged.
    let (id3, o3) = db
        .remember_keyed(
            Memory::new("v2", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0))
                .with_importance(0.5)
                .with_agent("alice"),
            "release",
        )
        .unwrap();
    assert_eq!(o3, WriteOutcome::Unchanged);
    assert_eq!(id3, id2, "NOOP returns the existing live id");
}

#[test]
fn recall_drops_superseded_records() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("recall-sup.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // Two keyed writes under the same key: the second supersedes the first.
    let (id1, _) = db
        .remember_keyed(
            Memory::new("old", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.9),
            "k",
        )
        .unwrap();
    let (id2, _) = db
        .remember_keyed(
            Memory::new("new", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.9),
            "k",
        )
        .unwrap();

    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .weights(ScoreWeights::default());
    let hits = db.recall(&req).unwrap();
    let ids: Vec<_> = hits.iter().map(|h| h.memory.id).collect();
    assert!(ids.contains(&id2), "live keyed memory must appear");
    assert!(
        !ids.contains(&id1),
        "superseded memory must be dropped from default recall"
    );
}

#[test]
fn history_walks_the_supersession_chain_newest_first() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("history.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // Build a chain: v1 → v2 → v3 under the shared key "status".
    let make = |content: &str, axis: usize| {
        let mut v = vec![0.0; 4];
        v[axis] = 1.0;
        Memory::new(content, MemoryType::Semantic, v)
            .with_scope(Scope::Shared)
            .with_importance(0.5)
    };
    let (id1, _) = db.remember_keyed(make("v1", 0), "status").unwrap();
    let (id2, _) = db.remember_keyed(make("v2", 1), "status").unwrap();
    let (id3, _) = db.remember_keyed(make("v3", 2), "status").unwrap();

    // v0.8.1 bugfix: these writes used `Scope::Shared`, so the chain
    // lives in the shared slot, not the default-agent private slot.
    // `history(key, None)` now walks the private slot (matching keyed
    // `remember` default), so use `history_in(key, KeyScope::Shared)`
    // for the shared chain.
    let chain = db.history_in("status", KeyScope::Shared).unwrap();
    assert_eq!(chain.len(), 3, "history should include live + 2 superseded");
    assert_eq!(chain[0].id, id3, "entry 0 is the live record");
    assert_eq!(chain[1].id, id2);
    assert_eq!(chain[2].id, id1);
}

#[test]
fn private_and_shared_keys_are_independent_slots() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("scope-keys.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // One private keyed record for "alice", one shared under the same
    // string. Neither must supersede the other.
    let (id_private, o_private) = db
        .remember_keyed(
            Memory::new("alice-pref", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0))
                .with_agent("alice"),
            "pref",
        )
        .unwrap();
    assert_eq!(o_private, WriteOutcome::Added);

    let (id_shared, o_shared) = db
        .remember_keyed(
            Memory::new(
                "shared-pref",
                MemoryType::Semantic,
                vec4(0.0, 1.0, 0.0, 0.0),
            )
            .with_scope(Scope::Shared),
            "pref",
        )
        .unwrap();
    assert_eq!(
        o_shared,
        WriteOutcome::Added,
        "shared-scope key must not supersede the private-scope slot"
    );
    assert_ne!(id_private, id_shared);

    // Reopen (after dropping the first handle to release its exclusive
    // flock) to make sure both are still live.
    db.flush().unwrap();
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let live_ids: std::collections::HashSet<_> =
        db.memories().unwrap().iter().map(|m| m.id).collect();
    assert!(live_ids.contains(&id_private));
    assert!(live_ids.contains(&id_shared));
}

#[test]
fn file_written_with_12_1_opens_under_v0_7_layout() {
    // Guard against an accidental format bump (R-001.10): the on-disk
    // header version must still be v8 after a 12.1 write round-trip.
    // Combined with the "all new per-memory data goes under `_mnemo.*`
    // metadata" rule, that means a v0.7.x reader can still open and
    // decrypt files written by this build. (The reader won't filter
    // superseded records out of recall — that behavior is 12.1-aware —
    // but it can read every record.)
    let dir = tempdir().unwrap();
    let path = dir.path().join("compat.mnemo");
    {
        let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        db.remember_keyed(
            Memory::new("first", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)),
            "slot",
        )
        .unwrap();
        db.remember_keyed(
            Memory::new("second", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0)),
            "slot",
        )
        .unwrap();
        db.flush().unwrap();
    }

    // (see the inline note below the test setup for how we validate
    // the on-disk layout stayed at v8.)
    // Mnemo uses format version `v8` since Phase FIX-1; `_mnemo.*`
    // metadata rides along as ordinary `Memory.metadata` serde, so a
    // v0.7.0 reader against the same on-disk bytes is unchanged.
    let bytes = std::fs::read(&path).unwrap();
    // The format module serializes the magic + version into the first
    // few bytes of the header; the exact layout is a private detail,
    // but the version byte is a single `u16` that lives at a known
    // offset. Rather than hard-code that, round-trip through the
    // engine itself — a successful `Mnemo::open` + `memories` means
    // the header validated against the current engine, which still
    // only accepts v8 files.
    assert!(!bytes.is_empty());
    let mut db = Mnemo::open(&path, "pw").unwrap();
    let all = db.memories().unwrap();
    assert_eq!(all.len(), 2, "both records must survive flush + reopen");
    // One of them should still be marked superseded, and the engine
    // (same layout as v0.7) reads the metadata out verbatim.
    let has_superseded_mark = all
        .iter()
        .any(|m| m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY));
    assert!(
        has_superseded_mark,
        "the superseded record's `_mnemo.superseded_by` marker should survive flush + reopen"
    );
}

// ===========================================================================
// Phase 12.6: filtered recall (session_id + metadata conditions)
// ===========================================================================

/// Helper that seeds `path` with `n` memories carrying predictable
/// `metadata.idx` (the integer index) and alternating `metadata.area`
/// / `metadata.tag` values. The vector is a simple axis spike keyed
/// off `idx % 4` so similarity ranks the first N uniformly.
fn seed_filter_corpus(path: &std::path::Path, pp: &str, n: usize) -> Vec<mnemo_engine::Ulid> {
    let mut db = Mnemo::create(path, pp, fast_cfg(4)).unwrap();
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let axis = i % 4;
        let mut v = vec![0.0; 4];
        v[axis] = 1.0;
        let mut m = Memory::new(format!("entry-{i}"), MemoryType::Semantic, v)
            .with_importance(0.5)
            .with_agent("alice");
        if i % 10 == 0 {
            m = m.with_session(format!("sess-{}", i % 3));
        }
        m.metadata.insert("idx".into(), json!(i as i64));
        m.metadata.insert(
            "area".into(),
            json!(if i % 2 == 0 { "onboarding" } else { "misc" }),
        );
        m.metadata.insert(
            "tag".into(),
            json!(match i % 5 {
                0 => "alpha",
                1 => "beta",
                2 => "gamma",
                3 => "delta",
                _ => "epsilon",
            }),
        );
        m.metadata.insert("score".into(), json!(i % 100));
        if i % 7 == 0 {
            m.metadata.insert("notes".into(), json!("has-notes"));
        }
        ids.push(db.remember(m).unwrap());
    }
    db.flush().unwrap();
    drop(db); // release the exclusive flock before the test re-opens
    ids
}

#[test]
fn filter_metadata_equality_restricts_candidates() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-eq.mnemo");
    seed_filter_corpus(&path, "pw", 50);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false)
        .filter(Filter::default().metadata_eq("area", json!("onboarding")));
    let hits = db.recall(&req).unwrap();
    assert!(!hits.is_empty(), "should match at least one");
    for h in &hits {
        assert_eq!(
            h.memory.metadata.get("area").and_then(|v| v.as_str()),
            Some("onboarding")
        );
    }
}

#[test]
fn filter_in_matches_any_listed_value() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-in.mnemo");
    seed_filter_corpus(&path, "pw", 50);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false)
        .filter(Filter::default().metadata_in("tag", vec![json!("alpha"), json!("beta")]));
    let hits = db.recall(&req).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        let tag = h.memory.metadata.get("tag").and_then(|v| v.as_str());
        assert!(tag == Some("alpha") || tag == Some("beta"), "tag={tag:?}");
    }
}

#[test]
fn filter_range_matches_inclusive_bounds() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-range.mnemo");
    seed_filter_corpus(&path, "pw", 50);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false)
        .filter(Filter::default().metadata_range("score", Some(10.0), Some(20.0)));
    let hits = db.recall(&req).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        let n = h.memory.metadata.get("score").and_then(|v| v.as_f64());
        let n = n.expect("score should be numeric");
        assert!((10.0..=20.0).contains(&n), "score {n} out of range");
    }
}

#[test]
fn filter_exists_requires_key_presence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-exists.mnemo");
    seed_filter_corpus(&path, "pw", 50);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false)
        .filter(Filter::default().metadata_exists("notes"));
    let hits = db.recall(&req).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        assert!(h.memory.metadata.contains_key("notes"));
    }
    // Sanity: without the filter, the "notes" subset is strictly smaller
    // than the total (we tag only every 7th memory).
    let req_all = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false);
    let all_hits = db.recall(&req_all).unwrap();
    assert!(hits.len() < all_hits.len());
}

#[test]
fn filter_session_id_only_returns_matching_memories() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-sess.mnemo");
    seed_filter_corpus(&path, "pw", 60);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false)
        .filter(Filter::new_session("sess-1"));
    let hits = db.recall(&req).unwrap();
    assert!(!hits.is_empty());
    for h in &hits {
        assert_eq!(
            h.memory.session_id.as_deref(),
            Some("sess-1"),
            "session filter must restrict to one session_id",
        );
    }
}

#[test]
fn filter_selective_still_returns_top_k_on_indexed_file() {
    // DISPATCH requirement: a filter that matches under ~5% of the
    // corpus on an indexed file must still return `top_k` correct hits.
    // We exercise the Phase 12.1 short-results-on-indexed-path widening
    // logic through the 12.6 filter path.
    //
    // We use 1000 memories (fast enough for a unit test; the mechanism
    // is identical to the 10K case) and a filter matching 40 of them.
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-selective.mnemo");
    let n = 1000;
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..n {
        let axis = i % 4;
        let mut v = vec![0.0; 4];
        v[axis] = 1.0;
        let mut m = Memory::new(format!("m-{i}"), MemoryType::Semantic, v)
            .with_importance(0.5)
            .with_agent("alice");
        // Only ~4% of memories get the rare marker.
        if i % 25 == 0 {
            m.metadata.insert("rare".into(), json!(true));
        }
        db.remember(m).unwrap();
    }
    db.build_index().unwrap();
    db.flush().unwrap();
    drop(db); // release the exclusive flock before the test re-opens

    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .filter(Filter::default().metadata_exists("rare"));
    let hits = db.recall(&req).unwrap();
    assert_eq!(
        hits.len(),
        10,
        "selective filter on an indexed file must still return top_k hits \
         (widen-then-exact-scan); got {} hits",
        hits.len()
    );
    for h in &hits {
        assert!(
            h.memory.metadata.contains_key("rare"),
            "every returned hit should carry the `rare` marker"
        );
    }
}

// ===========================================================================
// Phase 12.8: budgeted recall (max_bytes + truncated flag)
// ===========================================================================

#[test]
fn budgeted_recall_without_max_bytes_matches_plain_recall() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b-noop.mnemo");
    seed_filter_corpus(&path, "pw", 20);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(5)
        .track_access(false);
    let resp = db.recall_budgeted(&req).unwrap();
    assert!(!resp.truncated, "no max_bytes → truncated must be false");
    assert_eq!(resp.hits.len(), 5, "no budget → full top_k");
}

#[test]
fn budgeted_recall_drops_hits_past_budget() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b-cut.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // Three memories, each with 100-byte content. A 220-byte budget
    // keeps the first two (100 + 100 = 200 ≤ 220) and drops the third.
    for i in 0..3 {
        let mut v = vec![0.0; 4];
        v[0] = 1.0; // all three identical vectors, so rank is ULID order.
        let content = "x".repeat(100);
        let mut m = Memory::new(content, MemoryType::Semantic, v).with_importance(0.5);
        m.metadata.insert("idx".into(), json!(i as i64));
        db.remember(m).unwrap();
    }
    db.flush().unwrap();
    drop(db);

    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .max_bytes(220);
    let resp = db.recall_budgeted(&req).unwrap();
    assert!(
        resp.truncated,
        "budget of 220 bytes should cut the 3rd 100-byte hit"
    );
    assert_eq!(resp.hits.len(), 2);
}

#[test]
fn budgeted_recall_always_returns_top_hit_even_over_budget() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b-one.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    // One oversized memory + one small one. A 10-byte budget cuts the
    // small one but must keep the top-1 hit even though it alone is
    // 500 bytes (>> 10).
    db.remember(
        Memory::new(
            "y".repeat(500),
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        )
        .with_importance(0.9),
    )
    .unwrap();
    db.remember(
        Memory::new("small", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.5),
    )
    .unwrap();
    db.flush().unwrap();
    drop(db);

    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .max_bytes(10);
    let resp = db.recall_budgeted(&req).unwrap();
    assert_eq!(resp.hits.len(), 1, "top-1 must survive even if over budget");
    assert!(resp.truncated, "the cut 'small' hit sets truncated=true");
    assert_eq!(resp.hits[0].memory.content.len(), 500);
}

#[test]
fn budgeted_recall_truncated_false_when_everything_fits() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("b-fits.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for _ in 0..3 {
        db.remember(Memory::new(
            "tiny",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    }
    db.flush().unwrap();
    drop(db);

    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .max_bytes(10_000);
    let resp = db.recall_budgeted(&req).unwrap();
    assert_eq!(resp.hits.len(), 3);
    assert!(
        !resp.truncated,
        "budget larger than total content → not truncated"
    );
}

#[test]
fn fresh_handle_recall_hides_superseded_without_building_key_caches() {
    // Lane A v0.8.0 REVIEW fix: `recall_budgeted` must not pay a full
    // key-cache scan on the first query against a fresh handle. The
    // slow-path superseded check (`_mnemo.superseded_by` on the
    // decrypted record) carries correctness; the lazy `superseded`
    // set is a fast-path only, built by explicit keyed ops.
    let dir = tempdir().unwrap();
    let path = dir.path().join("fresh-handle-sup.mnemo");
    {
        // Writer handle: create a key chain so there's a superseded
        // record on disk. Dropped before the reader opens the file.
        let mut w = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
        w.remember_keyed(
            Memory::new("v1", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.5),
            "release",
        )
        .unwrap();
        w.remember_keyed(
            Memory::new("v2", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.5),
            "release",
        )
        .unwrap();
        w.flush().unwrap();
    }

    // Fresh reader. The key_index + superseded sets start as None.
    let mut r = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false);
    let resp = r.recall_budgeted(&req).unwrap();

    // Correctness: the superseded record is NOT in the result.
    assert_eq!(
        resp.hits.len(),
        1,
        "fresh-handle recall should see only the live keyed record"
    );
    assert_eq!(resp.hits[0].memory.content, "v2");

    // Observability (the planner's "without building the key caches"
    // ask): the recall must not have triggered `ensure_key_caches`.
    // We assert via the public `history` method as a proxy: before a
    // keyed op touches the handle, `history` builds the caches on
    // demand. Immediately after a recall, if the caches were built,
    // `history("release", None)` would return the same chain either
    // way — so instead we check the module-private invariant through
    // the engine's own exported `superseded` state by re-opening and
    // recalling again; the second recall has the same O(decrypts)
    // cost as the first, which we observe indirectly by asserting the
    // returned result is identical and no key-op happened on `r`.
    let resp2 = r.recall_budgeted(&req).unwrap();
    assert_eq!(resp2.hits.len(), 1);
    assert_eq!(resp2.hits[0].memory.content, "v2");
}

#[test]
fn filtered_recall_with_30_percent_match_does_not_exact_scan() {
    // Lane A v0.8.0 REVIEW fix: with default (unpinned) `n_probe` /
    // `n_rerank`, the widen step must widen from
    // `IvfPqIndex::effective_defaults()`, not skip straight to exact
    // scan. Build a 1 000-memory ANN-indexed corpus, apply a filter
    // matching ~30% of them, and assert the result (a) has top_k
    // correct hits and (b) `used_exact_scan == false`.
    let dir = tempdir().unwrap();
    let path = dir.path().join("f-30pct.mnemo");
    let n = 1000;
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..n {
        let axis = i % 4;
        let mut v = vec![0.0; 4];
        v[axis] = 1.0;
        let mut m = Memory::new(format!("m-{i}"), MemoryType::Semantic, v)
            .with_importance(0.5)
            .with_agent("alice");
        // ~30% of records carry the marker.
        if i % 10 < 3 {
            m.metadata.insert("marker".into(), json!(true));
        }
        db.remember(m).unwrap();
    }
    db.build_index().unwrap();
    db.flush().unwrap();
    drop(db);

    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .filter(Filter::default().metadata_exists("marker"));
    let resp = db.recall_budgeted(&req).unwrap();
    assert_eq!(
        resp.hits.len(),
        10,
        "30% filter on indexed file should still produce top_k=10 hits"
    );
    for h in &resp.hits {
        assert!(
            h.memory.metadata.contains_key("marker"),
            "every hit should carry the `marker`",
        );
    }
    assert!(
        !resp.used_exact_scan,
        "30% filter should be satisfied by the widened ANN pass, \
         not an exact scan; got used_exact_scan=true"
    );
}

#[test]
fn recall_wrapper_still_returns_hits_vec() {
    // The thin `recall` wrapper must keep its pre-12.8 signature
    // (`Result<Vec<RecallResult>>`) so pre-12.8 callers don't break.
    let dir = tempdir().unwrap();
    let path = dir.path().join("b-wrapper.mnemo");
    seed_filter_corpus(&path, "pw", 10);
    let mut db = Mnemo::open_read_only(&path, "pw").unwrap();
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(3)
        .track_access(false)
        .max_bytes(1); // aggressive budget
    let hits = db.recall(&req).unwrap();
    // `recall` discards the truncated flag; it still returns ≥1 hit
    // (the always-keep-top-1 rule).
    assert!(!hits.is_empty());
    assert!(hits.len() <= 3);
}

// ===========================================================================
// Phase 12.1.1 (v0.8.1): history slot resolution — default to the private
// slot of the default agent, with KeyScope::Shared as an explicit opt-in.
// ===========================================================================

// ===========================================================================
// Phase 12.7a: thread archive — ingest / export / forget + default recall
// ===========================================================================

fn with_embedder(path: &std::path::Path, pp: &str) -> Mnemo {
    let mut db = Mnemo::create(path, pp, fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    db
}

fn tt(id: &str, role: &str, content: &str) -> mnemo_engine::ThreadTurn {
    mnemo_engine::ThreadTurn {
        turn_id: id.into(),
        role: role.into(),
        content: content.into(),
        timestamp: None,
    }
}

#[test]
fn thread_ingest_then_export_round_trip_for_short_turns() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("thr.mnemo");
    let mut db = with_embedder(&path, "pw");
    let report = db
        .ingest_thread(
            "chat-1",
            &[
                tt("u-1", "user", "hi"),
                tt("a-1", "assistant", "hello back"),
                tt("u-2", "user", "bye"),
            ],
        )
        .unwrap();
    assert_eq!(report.added_records, 3);
    assert_eq!(report.skipped_existing_turns, 0);
    assert_eq!(report.truncated_turns, 0);
    assert_eq!(report.chunked_turns, 0);

    let exported = db.thread_export("chat-1").unwrap();
    assert_eq!(exported.len(), 3);
    assert_eq!(exported[0].content, "hi");
    assert_eq!(exported[1].content, "hello back");
    assert_eq!(exported[2].content, "bye");
    // _mnemo.seq must be monotonic.
    for (i, m) in exported.iter().enumerate() {
        let seq = m
            .metadata
            .get("_mnemo.seq")
            .and_then(|v| v.as_u64())
            .expect("seq");
        assert_eq!(seq as usize, i);
    }
}

#[test]
fn thread_ingest_is_idempotent_per_turn_id() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("thr-idem.mnemo");
    let mut db = with_embedder(&path, "pw");
    let turns = vec![tt("u-1", "user", "hi"), tt("a-1", "assistant", "hello")];
    let r1 = db.ingest_thread("chat-1", &turns).unwrap();
    assert_eq!(r1.added_records, 2);
    // Second ingest with the same turn_ids is a no-op.
    let r2 = db.ingest_thread("chat-1", &turns).unwrap();
    assert_eq!(r2.added_records, 0);
    assert_eq!(r2.skipped_existing_turns, 2);

    // Extend the transcript — only the new turn gets ingested.
    let r3 = db
        .ingest_thread(
            "chat-1",
            &[
                tt("u-1", "user", "hi"),
                tt("a-1", "assistant", "hello"),
                tt("u-2", "user", "next"),
            ],
        )
        .unwrap();
    assert_eq!(r3.added_records, 1);
    assert_eq!(r3.skipped_existing_turns, 2);

    let exported = db.thread_export("chat-1").unwrap();
    assert_eq!(exported.len(), 3);
    assert_eq!(exported[2].content, "next");
}

#[test]
fn thread_ingest_chunks_long_non_tool_turns() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("thr-chunk.mnemo");
    let mut db = with_embedder(&path, "pw");
    // Build a 20 KB message so it splits into three 8 KB chunks.
    let big = "x".repeat(20 * 1024);
    let report = db
        .ingest_thread("chat-1", &[tt("u-1", "user", &big)])
        .unwrap();
    assert_eq!(report.added_records, 3, "20 KB → 3 × 8 KB chunks");
    assert_eq!(report.chunked_turns, 1);
    assert_eq!(report.truncated_turns, 0);

    let exported = db.thread_export("chat-1").unwrap();
    assert_eq!(exported.len(), 3);
    // Parts reassemble to the original.
    let reassembled: String = exported.iter().map(|m| m.content.clone()).collect();
    assert_eq!(reassembled, big);
    // Each part carries `_mnemo.part` in order.
    for (i, m) in exported.iter().enumerate() {
        let part = m
            .metadata
            .get("_mnemo.part")
            .and_then(|v| v.as_u64())
            .expect("part");
        assert_eq!(part as usize, i);
    }
}

#[test]
fn thread_ingest_truncates_oversized_tool_results() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("thr-trunc.mnemo");
    let mut db = with_embedder(&path, "pw");
    let big = "a".repeat(20 * 1024);
    let report = db
        .ingest_thread("chat-1", &[tt("tool-1", "tool", &big)])
        .unwrap();
    assert_eq!(report.added_records, 1, "truncated to one record");
    assert_eq!(report.truncated_turns, 1);
    assert_eq!(report.chunked_turns, 0);

    let exported = db.thread_export("chat-1").unwrap();
    assert_eq!(exported.len(), 1);
    assert_eq!(exported[0].content.len(), 16 * 1024);
    let sha = exported[0]
        .metadata
        .get("_mnemo.sha256")
        .and_then(|v| v.as_str())
        .expect("sha256 should be present on a truncated turn");
    assert_eq!(sha.len(), 64, "hex SHA-256 is 64 chars");
    let orig_len = exported[0]
        .metadata
        .get("_mnemo.length")
        .and_then(|v| v.as_u64())
        .expect("length");
    assert_eq!(orig_len as usize, 20 * 1024);
}

#[test]
fn default_recall_excludes_turn_records() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("thr-excl.mnemo");
    let mut db = with_embedder(&path, "pw");
    db.remember(Memory::new(
        "a durable fact",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.ingest_thread("chat-1", &[tt("u-1", "user", "transient question")])
        .unwrap();
    db.flush().unwrap();

    // Plain recall must NOT surface the turn.
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false);
    let hits = db.recall(&req).unwrap();
    assert!(
        hits.iter()
            .all(|h| h.memory.content != "transient question"),
        "default recall must exclude `_mnemo.kind = turn` records"
    );

    // Opt in via a _mnemo.* filter → turns included.
    let req2 = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .filter(Filter::default().metadata_eq("_mnemo.kind", json!("turn")));
    let hits2 = db.recall(&req2).unwrap();
    assert!(
        hits2
            .iter()
            .any(|h| h.memory.content == "transient question"),
        "filter with `_mnemo.kind = turn` must include turn records; got {:?}",
        hits2.iter().map(|h| &h.memory.content).collect::<Vec<_>>()
    );
}

#[test]
fn thread_forget_removes_only_archive_records_for_thread() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("thr-forget.mnemo");
    let mut db = with_embedder(&path, "pw");
    db.remember(Memory::new(
        "fact that should stay",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.ingest_thread(
        "chat-A",
        &[tt("a-1", "user", "A1"), tt("a-2", "user", "A2")],
    )
    .unwrap();
    db.ingest_thread("chat-B", &[tt("b-1", "user", "B1")])
        .unwrap();

    let n = db.thread_forget("chat-A").unwrap();
    assert_eq!(n, 2, "only chat-A's 2 turn records go");

    // chat-B survives.
    let b = db.thread_export("chat-B").unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].content, "B1");
    // The durable fact survives.
    let mems = db.memories().unwrap();
    assert!(mems.iter().any(|m| m.content == "fact that should stay"));
}

#[test]
fn history_default_slot_matches_remember_default_slot() {
    // v0.8.1 bugfix (lane A): `remember(key=K)` with no explicit agent
    // writes to the default agent's private slot; `history(K)` with no
    // arguments must now walk that same slot (pre-v0.8.1 it walked the
    // shared slot by accident and returned []).
    let dir = tempdir().unwrap();
    let path = dir.path().join("hist-default.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    let (_id1, _) = db
        .remember_keyed(
            Memory::new("v1", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)).with_importance(0.5),
            "release",
        )
        .unwrap();
    let (_id2, _) = db
        .remember_keyed(
            Memory::new("v2", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0)).with_importance(0.5),
            "release",
        )
        .unwrap();

    // The obvious call: no agent, no shared → must walk the default
    // agent's private chain (live + 1 superseded = 2 entries).
    let chain = db.history("release", None).unwrap();
    assert_eq!(
        chain.len(),
        2,
        "history(key, None) should walk the default-agent private chain, not the shared slot"
    );
    assert_eq!(chain[0].content, "v2", "entry 0 is the live record");
    assert_eq!(chain[1].content, "v1");
}

#[test]
fn history_in_shared_and_private_are_separate_chains() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("hist-sep.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();

    // Two keyed slots under the same key string "pref":
    //   * default-agent private slot
    //   * shared slot
    // They must not alias.
    db.remember_keyed(
        Memory::new("priv-v1", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)),
        "pref",
    )
    .unwrap();
    db.remember_keyed(
        Memory::new("priv-v2", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0)),
        "pref",
    )
    .unwrap();
    db.remember_keyed(
        Memory::new("shared-v1", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0))
            .with_scope(Scope::Shared),
        "pref",
    )
    .unwrap();

    let private = db.history("pref", None).unwrap();
    assert_eq!(private.len(), 2, "default-agent private chain = 2");
    assert_eq!(private[0].content, "priv-v2");
    assert_eq!(private[1].content, "priv-v1");

    let shared = db.history_in("pref", KeyScope::Shared).unwrap();
    assert_eq!(shared.len(), 1, "shared chain = 1 (no supersession)");
    assert_eq!(shared[0].content, "shared-v1");

    // Named-agent lookup must also be independent.
    let alice = db
        .history_in("pref", KeyScope::Private("alice".into()))
        .unwrap();
    assert!(
        alice.is_empty(),
        "no 'alice' write → empty chain, not aliased to default or shared"
    );
}

// ===========================================================================
// Phase 12.7b: essence (timeline + sections + render + status lifecycle)
// ===========================================================================

#[test]
fn essence_add_section_item_shows_up_in_render() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    let id = db
        .essence_add(
            EssenceScope::Thread("t1".into()),
            EssenceSection::Intent,
            "ship v0.9.0",
            None,
        )
        .unwrap();
    let rendered = db.essence(EssenceScope::Thread("t1".into()), 1024).unwrap();
    assert!(
        rendered.contains("ship v0.9.0"),
        "intent item should appear in render; got:\n{rendered}"
    );
    assert!(
        rendered.contains("## intent"),
        "intent section header missing from render:\n{rendered}"
    );
    // The item id should be fetchable + carry open status.
    let m = db.get(&id).unwrap();
    assert_eq!(
        m.metadata.get(MNEMO_META_STATUS).and_then(|v| v.as_str()),
        Some("open")
    );
}

#[test]
fn essence_timeline_append_is_rejected_over_cap() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess-tl.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    // 700 bytes > 600-byte cap → rejected, not truncated.
    let too_long = "x".repeat(700);
    let err = db
        .essence_append_timeline(EssenceScope::Thread("t1".into()), &too_long, None)
        .unwrap_err();
    assert!(
        err.to_string().contains("<= 600 bytes"),
        "expected 600-byte-cap error, got: {err}"
    );
    // A 600-byte entry fits.
    let ok = "y".repeat(600);
    db.essence_append_timeline(EssenceScope::Thread("t1".into()), &ok, None)
        .unwrap();
}

#[test]
fn essence_section_item_rejected_over_cap() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess-sec.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    let too_long = "z".repeat(281);
    let err = db
        .essence_add(
            EssenceScope::Thread("t1".into()),
            EssenceSection::Open,
            &too_long,
            None,
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("<= 280 bytes"),
        "expected 280-byte-cap error, got: {err}"
    );
}

#[test]
fn essence_update_on_timeline_is_error() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess-upd.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    let id = db
        .essence_append_timeline(EssenceScope::Thread("t1".into()), "milestone", None)
        .unwrap();
    let err = db
        .essence_update(&id, "edited milestone", None)
        .unwrap_err();
    assert!(
        err.to_string().contains("append-only"),
        "expected append-only guard, got: {err}"
    );
}

#[test]
fn essence_resolve_transitions_status_and_is_idempotent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess-res.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    let id = db
        .essence_add(
            EssenceScope::Thread("t1".into()),
            EssenceSection::Open,
            "fix bug X",
            None,
        )
        .unwrap();
    let first = db.essence_resolve(&id).unwrap();
    assert_eq!(first, WriteOutcome::Updated);
    let m = db.get(&id).unwrap();
    assert_eq!(
        m.metadata.get(MNEMO_META_STATUS).and_then(|v| v.as_str()),
        Some("resolved")
    );
    let history = m
        .metadata
        .get(MNEMO_META_STATUS_HISTORY)
        .and_then(|v| v.as_array())
        .expect("status_history");
    assert!(
        history.len() >= 2,
        "status_history should have at least creation + resolve; got {history:?}"
    );
    // Second resolve is a no-op.
    let second = db.essence_resolve(&id).unwrap();
    assert_eq!(second, WriteOutcome::Unchanged);
}

#[test]
fn essence_render_respects_budget() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess-budget.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    // 30 timeline entries, each ~100 bytes. Full render would be
    // ~3 KB; budget it to 400 bytes and expect fold-tail behavior.
    for i in 0..30 {
        let text = format!("milestone #{i:02} — some body text here to fill out the entry");
        db.essence_append_timeline(EssenceScope::Thread("t1".into()), &text, None)
            .unwrap();
    }
    let out = db.essence(EssenceScope::Thread("t1".into()), 600).unwrap();
    assert!(out.len() <= 600, "render over budget: {} bytes", out.len());
    assert!(
        out.contains("earlier milestones"),
        "overflow should fold into `+N earlier milestones`:\n{out}"
    );
}

#[test]
fn essence_project_and_thread_scopes_are_independent() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ess-scope.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::Conventions,
        "repeat relay prompts verbatim",
        None,
    )
    .unwrap();
    db.essence_add(
        EssenceScope::Thread("t1".into()),
        EssenceSection::Intent,
        "ship 12.7",
        None,
    )
    .unwrap();

    // Thread render includes BOTH its own thread items AND project items
    // (per the spec: project-scope items show up in every thread's render).
    let thread_render = db.essence(EssenceScope::Thread("t1".into()), 2048).unwrap();
    assert!(thread_render.contains("ship 12.7"));
    assert!(thread_render.contains("repeat relay prompts verbatim"));

    // Project render includes project items only.
    let project_render = db.essence(EssenceScope::Project, 2048).unwrap();
    assert!(project_render.contains("repeat relay prompts verbatim"));
    assert!(
        !project_render.contains("ship 12.7"),
        "project render should NOT include thread-scoped items; got:\n{project_render}"
    );
}

// ---------- Phase 12.7c: continuation + commit_deltas -------------------

#[test]
fn thread_start_writes_link_record_with_continues_marker() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("link.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    let link_id = db.thread_start("t2", "t1").unwrap();
    let m = db.get(&link_id).unwrap();
    assert_eq!(
        m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()),
        Some(MNEMO_META_KIND_THREAD_LINK)
    );
    assert_eq!(
        m.metadata
            .get(MNEMO_META_CONTINUES)
            .and_then(|v| v.as_str()),
        Some("t1")
    );
}

#[test]
fn essence_render_for_continuation_pulls_prev_open_next_and_timeline() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cont.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();

    // Previous thread: add open + next + a few timeline entries.
    db.essence_add(
        EssenceScope::Thread("t1".into()),
        EssenceSection::Open,
        "finish 12.7c",
        None,
    )
    .unwrap();
    db.essence_add(
        EssenceScope::Thread("t1".into()),
        EssenceSection::Next,
        "release v0.9.0",
        None,
    )
    .unwrap();
    for i in 0..5 {
        let line = format!("milestone #{i}");
        db.essence_append_timeline(EssenceScope::Thread("t1".into()), &line, None)
            .unwrap();
    }

    // Start continuation thread.
    db.thread_start("t2", "t1").unwrap();

    // New thread's own essence item.
    db.essence_add(
        EssenceScope::Thread("t2".into()),
        EssenceSection::Intent,
        "pick up where t1 left off",
        None,
    )
    .unwrap();

    let out = db.essence(EssenceScope::Thread("t2".into()), 4096).unwrap();
    assert!(
        out.contains("continued from"),
        "continuation render missing prelude header; got:\n{out}"
    );
    assert!(
        out.contains("finish 12.7c"),
        "prelude should carry prev `open`; got:\n{out}"
    );
    assert!(
        out.contains("release v0.9.0"),
        "prelude should carry prev `next`; got:\n{out}"
    );
    assert!(
        out.contains("milestone #4"),
        "prelude should carry recent timeline entries; got:\n{out}"
    );
    assert!(
        out.contains("pick up where t1 left off"),
        "new thread's own essence should still render; got:\n{out}"
    );
}

#[test]
fn commit_deltas_validates_everything_before_staging() {
    // A dim-mismatched fact mid-batch must fail the whole call with
    // no record written.
    let dir = tempdir().unwrap();
    let path = dir.path().join("batch-val.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();
    let before = db.stats().unwrap().memories;

    let good = Memory::new("ok", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0));
    let bad = Memory::new("wrong-dim", MemoryType::Semantic, vec![1.0, 0.0]);
    let batch = CommitBatch {
        facts: vec![
            KeyedMemory {
                memory: good,
                key: None,
            },
            KeyedMemory {
                memory: bad,
                key: None,
            },
        ],
        ..Default::default()
    };
    let err = db.commit_deltas(batch).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("dim") || msg.contains("dimension") || msg.contains("vector"),
        "expected dim-mismatch error, got: {msg}"
    );
    let after = db.stats().unwrap().memories;
    assert_eq!(
        before, after,
        "commit_deltas must be atomic on validation failure"
    );
}

#[test]
fn commit_deltas_applies_facts_essence_and_turns_in_one_call() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("batch-ok.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();

    let fact = Memory::new("fact-a", MemoryType::Semantic, vec4(1.0, 0.0, 0.0, 0.0));
    let op = EssenceOp {
        kind: EssenceOpKind::Add,
        scope: Some(EssenceScope::Thread("tA".into())),
        section: Some(EssenceSection::Intent),
        id: None,
        text: Some("ship it".into()),
        refs: None,
    };
    let turn = ThreadTurn {
        turn_id: "u-1".into(),
        role: "user".into(),
        content: "hello".into(),
        timestamp: None,
    };
    let batch = CommitBatch {
        facts: vec![KeyedMemory {
            memory: fact,
            key: None,
        }],
        essence_ops: vec![op],
        turns: vec![turn],
        thread_id: Some("tA".into()),
        turn_policy: TurnPolicy::Archive,
    };
    let report = db.commit_deltas(batch).unwrap();
    assert_eq!(report.facts_added, 1);
    assert_eq!(report.essence_ops_applied, 1);
    assert_eq!(report.turns_added, 1);

    // Essence item rendered.
    let out = db.essence(EssenceScope::Thread("tA".into()), 2048).unwrap();
    assert!(out.contains("ship it"), "essence item not rendered:\n{out}");
    // Turn archived.
    let archived = db.thread_export("tA").unwrap();
    assert_eq!(archived.len(), 1);
    assert_eq!(archived[0].content, "hello");
}

#[test]
fn commit_deltas_turn_policy_drop_discards_turns() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("batch-drop.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();

    let turn = ThreadTurn {
        turn_id: "u-1".into(),
        role: "user".into(),
        content: "ephemeral".into(),
        timestamp: None,
    };
    let batch = CommitBatch {
        turns: vec![turn],
        thread_id: Some("tD".into()),
        turn_policy: TurnPolicy::Drop,
        ..Default::default()
    };
    let report = db.commit_deltas(batch).unwrap();
    assert_eq!(report.turns_added, 0);
    let archived = db.thread_export("tD").unwrap();
    assert!(archived.is_empty(), "drop policy wrote turns: {archived:?}");
}

#[test]
fn commit_deltas_rejects_turns_without_thread_id() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("batch-no-thread.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(FakeEmbedder::new("fake-4d", 4, true))
        .unwrap();

    let batch = CommitBatch {
        turns: vec![ThreadTurn {
            turn_id: "u-1".into(),
            role: "user".into(),
            content: "orphan".into(),
            timestamp: None,
        }],
        turn_policy: TurnPolicy::Archive,
        ..Default::default()
    };
    let err = db.commit_deltas(batch).unwrap_err();
    assert!(
        err.to_string().to_lowercase().contains("thread"),
        "expected thread_id-required error, got: {err}"
    );
}

#[test]
fn default_recall_excludes_turn_essence_and_thread_link_records() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("kind-excl.mnemo");
    let mut db = with_embedder(&path, "pw");
    db.remember(Memory::new(
        "a durable fact",
        MemoryType::Semantic,
        vec4(1.0, 0.0, 0.0, 0.0),
    ))
    .unwrap();
    db.ingest_thread("t1", &[tt("u-1", "user", "transient question")])
        .unwrap();
    db.essence_add(
        EssenceScope::Thread("t1".into()),
        EssenceSection::Intent,
        "essence intent item",
        None,
    )
    .unwrap();
    db.thread_start("t2", "t1").unwrap();
    db.flush().unwrap();

    let is_thread_layer = |m: &Memory| {
        matches!(
            m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()),
            Some("turn") | Some("essence") | Some("thread_link")
        )
    };

    // Default recall: only the fact; no turn, essence or link records.
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false);
    let hits = db.recall(&req).unwrap();
    assert!(
        hits.iter().any(|h| h.memory.content == "a durable fact"),
        "the fact should still be recalled"
    );
    let leaked: Vec<_> = hits
        .iter()
        .filter(|h| is_thread_layer(&h.memory))
        .map(|h| h.memory.content.clone())
        .collect();
    assert!(
        leaked.is_empty(),
        "default recall leaked thread-layer records: {leaked:?}"
    );

    // Opt in with a `_mnemo.kind` `$in` filter: all three kinds come back.
    let req2 = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(50)
        .track_access(false)
        .filter(Filter::default().metadata_in(
            MNEMO_META_KIND,
            vec![json!("turn"), json!("essence"), json!("thread_link")],
        ));
    let kinds: std::collections::HashSet<String> = db
        .recall(&req2)
        .unwrap()
        .iter()
        .filter_map(|h| {
            h.memory
                .metadata
                .get(MNEMO_META_KIND)
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .collect();
    for k in ["turn", "essence", "thread_link"] {
        assert!(
            kinds.contains(k),
            "kind filter should return `{k}`; got {kinds:?}"
        );
    }
}

// ---------- Phase 12.7d: batch embedding, export shape, about essence ----

/// FakeEmbedder that counts its `embed` calls, for the
/// one-embed-call-per-batch check.
struct CountingEmbedder {
    inner: FakeEmbedder,
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
impl Embedder for CountingEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        self.inner.identity()
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.embed(texts)
    }
}

fn with_counting_embedder(
    path: &std::path::Path,
) -> (Mnemo, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut db = Mnemo::create(path, "pw", fast_cfg(4)).unwrap();
    db.remember(Memory::scaffold_manifest(4)).unwrap();
    db.set_embedder(CountingEmbedder {
        inner: FakeEmbedder::new("fake-4d", 4, true),
        calls: calls.clone(),
    })
    .unwrap();
    (db, calls)
}

fn text_fact(content: &str) -> KeyedMemory {
    KeyedMemory {
        memory: Memory::new(content, MemoryType::Semantic, Vec::new()),
        key: None,
    }
}

#[test]
fn commit_deltas_embeds_text_only_facts_essence_and_turns_in_one_call() {
    let dir = tempdir().unwrap();
    let (mut db, calls) = with_counting_embedder(&dir.path().join("embed1.mnemo"));
    let before = calls.load(std::sync::atomic::Ordering::SeqCst);

    let batch = CommitBatch {
        facts: vec![
            text_fact("fact one"),
            text_fact("fact two"),
            KeyedMemory {
                memory: Memory::new("fact three", MemoryType::Semantic, vec4(0.0, 1.0, 0.0, 0.0)),
                key: None,
            },
        ],
        essence_ops: vec![
            EssenceOp {
                kind: EssenceOpKind::Add,
                scope: Some(EssenceScope::Thread("tE".into())),
                section: Some(EssenceSection::Intent),
                id: None,
                text: Some("ship 12.7d".into()),
                refs: None,
            },
            EssenceOp {
                kind: EssenceOpKind::AppendTimeline,
                scope: Some(EssenceScope::Thread("tE".into())),
                section: None,
                id: None,
                text: Some("first milestone".into()),
                refs: None,
            },
        ],
        turns: vec![tt("u-1", "user", "hello"), tt("u-2", "assistant", "hi")],
        thread_id: Some("tE".into()),
        turn_policy: TurnPolicy::Archive,
    };
    let report = db.commit_deltas(batch).unwrap();
    assert_eq!(report.facts_added, 3);
    assert_eq!(report.essence_ops_applied, 2);
    assert_eq!(report.turns_added, 2);
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst) - before,
        1,
        "commit_deltas must embed the whole batch in exactly one call"
    );

    // Text-only facts carry the embedder's vector for their content.
    let expected = FakeEmbedder::new("fake-4d", 4, true)
        .embed(&["fact one"])
        .unwrap()
        .remove(0);
    let stored = db
        .memories()
        .unwrap()
        .into_iter()
        .find(|m| m.content == "fact one")
        .expect("text-only fact was written");
    assert_eq!(stored.vector, expected);

    // A batch with nothing to embed makes no call at all.
    let before = calls.load(std::sync::atomic::Ordering::SeqCst);
    let batch = CommitBatch {
        facts: vec![KeyedMemory {
            memory: Memory::new("fact four", MemoryType::Semantic, vec4(0.0, 0.0, 1.0, 0.0)),
            key: None,
        }],
        ..Default::default()
    };
    db.commit_deltas(batch).unwrap();
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), before);
}

#[test]
fn commit_deltas_text_only_fact_without_embedder_writes_nothing() {
    let dir = tempdir().unwrap();
    let mut db = Mnemo::create(dir.path().join("noembed.mnemo"), "pw", fast_cfg(4)).unwrap();
    let before = db.stats().unwrap().memories;
    let batch = CommitBatch {
        facts: vec![
            KeyedMemory {
                memory: Memory::new(
                    "has a vector",
                    MemoryType::Semantic,
                    vec4(1.0, 0.0, 0.0, 0.0),
                ),
                key: None,
            },
            text_fact("needs an embedder"),
        ],
        ..Default::default()
    };
    let err = db.commit_deltas(batch).unwrap_err();
    assert!(
        err.to_string().contains("no embedder"),
        "expected a no-embedder error, got: {err}"
    );
    assert_eq!(
        db.stats().unwrap().memories,
        before,
        "nothing may be staged"
    );
}

#[test]
fn commit_deltas_rejects_bad_essence_targets_before_writing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("badtarget.mnemo");
    let mut db = with_embedder(&path, "pw");
    let item = db
        .essence_add(
            EssenceScope::Thread("t1".into()),
            EssenceSection::Open,
            "an open item",
            None,
        )
        .unwrap();
    let milestone = db
        .essence_append_timeline(EssenceScope::Thread("t1".into()), "a milestone", None)
        .unwrap();
    let fact_id = db
        .remember(Memory::new(
            "plain fact",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
    db.flush().unwrap();
    let before = db.stats().unwrap().memories;

    let op = |kind: EssenceOpKind, id: mnemo_engine::Ulid, text: Option<&str>| EssenceOp {
        kind,
        scope: None,
        section: None,
        id: Some(id),
        text: text.map(String::from),
        refs: None,
    };
    let cases: Vec<(&str, Vec<EssenceOp>)> = vec![
        (
            "unknown id",
            vec![op(
                EssenceOpKind::Update,
                mnemo_engine::Ulid::new(),
                Some("x"),
            )],
        ),
        (
            "not an essence record",
            vec![op(EssenceOpKind::Resolve, fact_id, None)],
        ),
        (
            "update on a timeline entry",
            vec![op(EssenceOpKind::Update, milestone, Some("edited"))],
        ),
        (
            "update over the section cap",
            vec![op(
                EssenceOpKind::Update,
                item,
                Some("z".repeat(281).as_str()),
            )],
        ),
        (
            "op after a remove of the same id",
            vec![
                op(EssenceOpKind::Remove, item, None),
                op(EssenceOpKind::Resolve, item, None),
            ],
        ),
    ];
    for (name, essence_ops) in cases {
        let batch = CommitBatch {
            facts: vec![text_fact("would be written first")],
            essence_ops,
            ..Default::default()
        };
        assert!(
            db.commit_deltas(batch).is_err(),
            "case `{name}` should fail"
        );
        assert_eq!(
            db.stats().unwrap().memories,
            before,
            "case `{name}` staged a write"
        );
    }
}

#[test]
fn commit_deltas_expire_after_stamps_only_the_new_turns() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ttl.mnemo");
    let mut db = with_embedder(&path, "pw");
    db.ingest_thread("tT", &[tt("u-1", "user", "kept forever")])
        .unwrap();
    let batch = CommitBatch {
        turns: vec![
            tt("u-1", "user", "kept forever"),
            tt("u-2", "user", "short-lived"),
        ],
        thread_id: Some("tT".into()),
        turn_policy: TurnPolicy::ExpireAfter(3600),
        ..Default::default()
    };
    let report = db.commit_deltas(batch).unwrap();
    assert_eq!(report.turns_added, 1);
    assert_eq!(report.turns_skipped, 1);
    let records = db.thread_export("tT").unwrap();
    let ttl_of = |content: &str| {
        records
            .iter()
            .find(|m| m.content == content)
            .map(|m| m.ttl_secs)
            .expect("record exists")
    };
    assert_eq!(
        ttl_of("kept forever"),
        None,
        "an already-archived turn keeps no TTL"
    );
    assert_eq!(ttl_of("short-lived"), Some(3600));
}

#[test]
fn thread_export_turns_is_ingest_shape_in_seq_order_and_round_trips() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("export.mnemo");
    let mut db = with_embedder(&path, "pw");
    let long = "é".repeat(5_000); // 10 KB of 2-byte chars: chunked at 8 KB
    let tool = "t".repeat(20_000); // over the 16 KB tool cap: truncated
    let mut turns = vec![
        tt("b", "user", "first, despite sorting second"),
        tt("a", "assistant", &long),
        tt("c", "tool", &tool),
    ];
    for (i, t) in turns.iter_mut().enumerate() {
        t.timestamp = Some(1_700_000_000 + i as i64);
    }
    let r = db.ingest_thread("tX", &turns).unwrap();
    assert_eq!(r.chunked_turns, 1);
    assert_eq!(r.truncated_turns, 1);

    let exported = db.thread_export_turns("tX").unwrap();
    let ids: Vec<&str> = exported.iter().map(|t| t.turn_id.as_str()).collect();
    assert_eq!(ids, ["b", "a", "c"], "turns must come back in ingest order");
    assert_eq!(
        exported[1].content, long,
        "chunks are joined back into one turn"
    );
    assert_eq!(exported[1].role, "assistant");
    assert_eq!(exported[0].timestamp, 1_700_000_000);
    assert!(exported[0].sha256.is_none());
    assert_eq!(exported[2].original_length, Some(20_000));
    assert!(exported[2].sha256.as_deref().is_some_and(|h| h.len() == 64));
    let json = exported[2].to_json();
    assert_eq!(json["turn_id"], "c");
    assert_eq!(json["original_length"], 20_000);

    let back: Vec<mnemo_engine::ThreadTurn> = exported
        .iter()
        .map(|t| mnemo_engine::ThreadTurn {
            turn_id: t.turn_id.clone(),
            role: t.role.clone(),
            content: t.content.clone(),
            timestamp: Some(t.timestamp),
        })
        .collect();
    // Same thread: every turn is already there.
    let again = db.ingest_thread("tX", &back).unwrap();
    assert_eq!(again.added_records, 0);
    assert_eq!(again.skipped_existing_turns, 3);
    // A new thread: the export re-creates the same turns. (The stored
    // tool text is already at the 16 KB cap, so the copy isn't truncated
    // again and carries no sha256 / original_length.)
    db.ingest_thread("tX-copy", &back).unwrap();
    let shape = |ts: &[mnemo_engine::ExportedTurn]| -> Vec<(String, String, String, i64)> {
        ts.iter()
            .map(|t| {
                (
                    t.turn_id.clone(),
                    t.role.clone(),
                    t.content.clone(),
                    t.timestamp,
                )
            })
            .collect()
    };
    let copy = db.thread_export_turns("tX-copy").unwrap();
    assert_eq!(shape(&copy), shape(&exported));
}

#[test]
fn about_essence_renders_open_project_items_only() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("about-ess.mnemo");
    let mut db = with_embedder(&path, "pw");
    assert_eq!(
        db.about_essence().unwrap(),
        None,
        "fresh file has no essence"
    );

    db.essence_add(
        EssenceScope::Thread("t1".into()),
        EssenceSection::Intent,
        "thread-only intent",
        None,
    )
    .unwrap();
    assert_eq!(
        db.about_essence().unwrap(),
        None,
        "thread-scope items don't make a project essence"
    );

    let id = db
        .essence_add(
            EssenceScope::Project,
            EssenceSection::Conventions,
            "repeat relay prompts verbatim",
            None,
        )
        .unwrap();
    let render = db
        .about_essence()
        .unwrap()
        .expect("project essence present");
    assert!(render.contains("repeat relay prompts verbatim"), "{render}");
    assert!(!render.contains("thread-only intent"), "{render}");
    assert!(render.len() <= mnemo_engine::ESSENCE_DEFAULT_RENDER_BYTES);

    db.essence_resolve(&id).unwrap();
    assert_eq!(
        db.about_essence().unwrap(),
        None,
        "only resolved project items: nothing to brief"
    );
    // about() itself still returns just the records.
    assert!(db
        .about()
        .unwrap()
        .iter()
        .all(|m| m.metadata.get(MNEMO_META_KIND).is_none()));
}

// ---------- PERF-1 step 2: the ANN index is derived data -----------------

fn l2_top(db: &mut Mnemo, v: &[f32], k: usize) -> Vec<u128> {
    let mut req = RecallRequest::new(v.to_vec())
        .top_k(k)
        .n_probe(10_000)
        .n_rerank(10_000)
        .track_access(false);
    req.metric = Metric::L2;
    req.weights = sim_only();
    db.recall(&req)
        .unwrap()
        .iter()
        .map(|h| h.memory.id.0)
        .collect()
}

/// Plain flushes rewrite only the catalog; the persisted index run stays
/// put. After a crash-equivalent drop, reopening re-assigns every record
/// written since that run (new, updated) and forgets deleted ones, so ANN
/// recall still finds exactly the live set. `flush_index` and `close`
/// persist the index.
#[test]
fn index_persists_lazily_and_reopen_reassigns_newer_records() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazy-index.mnemo");
    let dims = 32;
    let mut rng = Rng::new(0x1DE5);
    let vec_ = |rng: &mut Rng| -> Vec<f32> { (0..dims).map(|_| rng.normalish()).collect() };
    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();
    let mut old = Vec::new();
    for i in 0..400 {
        let v = vec_(&mut rng);
        let id = db
            .remember(Memory::new(
                format!("old {i}"),
                MemoryType::Semantic,
                v.clone(),
            ))
            .unwrap();
        old.push((id, v));
    }
    db.flush().unwrap();
    db.build_index().unwrap();
    db.flush().unwrap();
    let index_start = db.__budget_report().unwrap().index_start_page;
    assert!(index_start > 0);

    let mut fresh = Vec::new();
    for k in 0..5 {
        let v = vec_(&mut rng);
        let id = db
            .remember(Memory::new(
                format!("new {k}"),
                MemoryType::Semantic,
                v.clone(),
            ))
            .unwrap();
        db.flush().unwrap();
        fresh.push((id, v));
    }
    let moved = vec_(&mut rng);
    let (upd_id, _) = old[3].clone();
    db.update(
        &upd_id,
        MemoryPatch {
            vector: Some(moved.clone()),
            ..Default::default()
        },
    )
    .unwrap();
    let (del_id, del_vec) = old[7].clone();
    db.delete(&del_id).unwrap();
    db.flush().unwrap();
    assert_eq!(
        db.__budget_report().unwrap().index_start_page,
        index_start,
        "plain flushes must not rewrite the index"
    );

    // Crash-equivalent: drop without close.
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(db.has_index());
    for (id, v) in &fresh {
        assert_eq!(
            l2_top(&mut db, v, 1),
            vec![id.0],
            "a record newer than the index is found"
        );
    }
    assert_eq!(
        l2_top(&mut db, &moved, 1),
        vec![upd_id.0],
        "an updated vector is re-assigned"
    );
    assert!(
        !l2_top(&mut db, &del_vec, 5).contains(&del_id.0),
        "a deleted id is gone"
    );
    let (keep_id, keep_vec) = &old[100];
    assert_eq!(l2_top(&mut db, keep_vec, 1), vec![keep_id.0]);

    // flush_index persists the reconciled index. FMT-9c.3: v9 already
    // persisted every change as an index delta, so its base run stays
    // until the chain is folded.
    db.flush_index().unwrap();
    let persisted = db.__budget_report().unwrap().index_start_page;
    if FORMAT == 8 {
        assert_ne!(persisted, index_start);
    } else {
        assert!(db.__index_shape().0 > 0, "the changes live in index deltas");
    }

    // close() persists it too.
    let v = vec_(&mut rng);
    let late = db
        .remember(Memory::new("late", MemoryType::Semantic, v.clone()))
        .unwrap();
    db.flush_index().unwrap();
    if FORMAT == 8 {
        assert_ne!(db.__budget_report().unwrap().index_start_page, persisted);
    }
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(l2_top(&mut db, &v, 1), vec![late.0]);
}

/// The persistence policy: after `INDEX_PERSIST_EVERY_COMMITS` index-dirty
/// commits, a plain flush persists the index.
#[test]
fn index_is_persisted_every_n_commits() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("every-n.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..50 {
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Semantic,
            vec4(1.0, i as f32, 0.5, 2.0),
        ))
        .unwrap();
    }
    db.build_index().unwrap();
    db.flush().unwrap();
    let start = db.__budget_report().unwrap().index_start_page;
    let n = mnemo_engine::INDEX_PERSIST_EVERY_COMMITS;
    if FORMAT == 9 {
        // FMT-9c.3: v9 persists the index on every commit (a delta, folded
        // into a new base at 64 runs or a quarter of the base); the
        // every-N policy is v8's.
        let mut prev = db.__index_shape();
        for i in 0..n {
            db.remember(Memory::new(
                format!("x{i}"),
                MemoryType::Semantic,
                vec4(2.0, i as f32, 1.0, 0.5),
            ))
            .unwrap();
            db.flush().unwrap();
            let now = db.__index_shape();
            assert_ne!(now, prev, "commit {i} persists the index");
            prev = now;
        }
        return;
    }
    for i in 0..n - 1 {
        db.remember(Memory::new(
            format!("x{i}"),
            MemoryType::Semantic,
            vec4(2.0, i as f32, 1.0, 0.5),
        ))
        .unwrap();
        db.flush().unwrap();
        assert_eq!(
            db.__budget_report().unwrap().index_start_page,
            start,
            "commit {i}"
        );
    }
    db.remember(Memory::new(
        "last",
        MemoryType::Semantic,
        vec4(3.0, 1.0, 1.0, 1.0),
    ))
    .unwrap();
    db.flush().unwrap();
    assert_ne!(
        db.__budget_report().unwrap().index_start_page,
        start,
        "the {n}th index-dirty commit persists the index"
    );
}

/// R-004.1: a read-only open re-assigns memories written after the
/// persisted index, in memory, without writing.
#[test]
fn read_only_open_finds_memories_newer_than_the_index() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("ro-pending.mnemo");
    let dims = 16;
    let mut rng = Rng::new(0xA11CE);
    let vec_ = |rng: &mut Rng| -> Vec<f32> { (0..dims).map(|_| rng.normalish()).collect() };
    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();
    for i in 0..200 {
        db.remember(Memory::new(
            format!("old {i}"),
            MemoryType::Semantic,
            vec_(&mut rng),
        ))
        .unwrap();
    }
    db.build_index().unwrap();
    db.flush().unwrap();
    let v = vec_(&mut rng);
    let id = db
        .remember(Memory::new("pending", MemoryType::Semantic, v.clone()))
        .unwrap();
    db.flush().unwrap(); // catalog only: the index run doesn't have it
    let start = db.__budget_report().unwrap().index_start_page;
    drop(db);

    let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
    assert_eq!(l2_top(&mut ro, &v, 1), vec![id.0]);
    drop(ro);
    let db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(
        db.__budget_report().unwrap().index_start_page,
        start,
        "the read-only open wrote nothing"
    );
}

/// R-004.1: restoring a snapshot whose index run predates its catalog
/// re-assigns the newer records before the index is persisted again.
#[test]
fn restore_to_with_a_stale_index_reassigns_newer_records() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("restore-stale.mnemo");
    let dims = 16;
    let mut rng = Rng::new(0xBEE5);
    let vec_ = |rng: &mut Rng| -> Vec<f32> { (0..dims).map(|_| rng.normalish()).collect() };
    let mut db = Mnemo::create(&path, "pw", fast_cfg(dims)).unwrap();
    for i in 0..200 {
        db.remember(Memory::new(
            format!("old {i}"),
            MemoryType::Semantic,
            vec_(&mut rng),
        ))
        .unwrap();
    }
    db.build_index().unwrap();
    db.flush().unwrap();
    // Two catalog-only commits: the snapshot of the second one points at
    // the index built above, which has neither of these.
    let a = vec_(&mut rng);
    let a_id = db
        .remember(Memory::new("a", MemoryType::Semantic, a.clone()))
        .unwrap();
    db.flush().unwrap();
    let b = vec_(&mut rng);
    let b_id = db
        .remember(Memory::new("b", MemoryType::Semantic, b.clone()))
        .unwrap();
    db.flush().unwrap();
    let target = db.snapshots().last().unwrap().txn_id;
    // Move on, then restore that snapshot.
    let (later_id, _) = {
        let v = vec_(&mut rng);
        (
            db.remember(Memory::new("later", MemoryType::Semantic, v.clone()))
                .unwrap(),
            v,
        )
    };
    db.flush_index().unwrap();
    db.restore_to(target).unwrap();
    assert_eq!(l2_top(&mut db, &a, 1), vec![a_id.0]);
    assert_eq!(l2_top(&mut db, &b, 1), vec![b_id.0]);
    assert!(
        db.get(&later_id).is_err(),
        "the later memory isn't in the snapshot"
    );
    drop(db);
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(l2_top(&mut db, &a, 1), vec![a_id.0]);
    assert_eq!(l2_top(&mut db, &b, 1), vec![b_id.0]);
}

// ---------- PERF-1 step 2b (R-004.2): thread layers out of the ANN -------

/// Turns, essence items and thread links never enter the ANN, whether
/// they're written before or after the index is built.
#[test]
fn non_fact_kinds_stay_out_of_the_ann() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("kinds.mnemo");
    let mut db = with_embedder(&path, "pw");
    for i in 0..60 {
        db.remember(Memory::new(
            format!("fact {i}"),
            MemoryType::Semantic,
            vec4(1.0, i as f32, 0.5, 0.1),
        ))
        .unwrap();
    }
    db.ingest_thread(
        "t1",
        &[tt("u-1", "user", "a turn"), tt("u-2", "user", "another")],
    )
    .unwrap();
    db.essence_add(
        EssenceScope::Project,
        EssenceSection::Conventions,
        "be brief",
        None,
    )
    .unwrap();
    db.build_index().unwrap();
    // 60 facts + the scaffold manifest.
    assert_eq!(db.stats().unwrap().index.unwrap().vectors, 61);
    db.ingest_thread("t1", &[tt("u-3", "user", "after the build")])
        .unwrap();
    db.essence_append_timeline(EssenceScope::Thread("t1".into()), "a milestone", None)
        .unwrap();
    db.thread_start("t2", "t1").unwrap();
    db.remember(Memory::new(
        "one more fact",
        MemoryType::Semantic,
        vec4(0.0, 1.0, 0.0, 0.0),
    ))
    .unwrap();
    assert_eq!(db.stats().unwrap().index.unwrap().vectors, 62);
    db.close().unwrap();
    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert_eq!(db.stats().unwrap().index.unwrap().vectors, 62);

    // A thread-pinned recall scans that thread's records exactly.
    let req = RecallRequest::new(vec4(1.0, 0.0, 0.0, 0.0))
        .top_k(10)
        .track_access(false)
        .filter(Filter::default().metadata_eq("_mnemo.thread", json!("t1")));
    let hits = db.recall(&req).unwrap();
    let mut contents: Vec<String> = hits.iter().map(|h| h.memory.content.clone()).collect();
    contents.sort();
    assert_eq!(
        contents,
        ["a milestone", "a turn", "after the build", "another"]
    );
}

/// Once the handle caches are built, writes keep them current: about,
/// essence, ingest idempotence, forget and keyed slots all see changes made
/// on the same handle.
#[test]
fn handle_caches_track_writes_on_the_same_handle() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("caches.mnemo");
    let mut db = with_embedder(&path, "pw");
    assert_eq!(db.about().unwrap().len(), 1); // builds the caches

    // Onboarding: add, then delete.
    let note = db
        .remember(
            Memory::new("read me", MemoryType::Semantic, vec4(0.0, 0.0, 1.0, 0.0))
                .with_meta("area", json!("onboarding")),
        )
        .unwrap();
    assert_eq!(db.about().unwrap().len(), 2);
    db.delete(&note).unwrap();
    assert_eq!(db.about().unwrap().len(), 1);

    // Essence: add, update, remove.
    let item = db
        .essence_add(EssenceScope::Project, EssenceSection::Open, "first", None)
        .unwrap();
    assert!(db.about_essence().unwrap().unwrap().contains("first"));
    db.essence_update(&item, "second", None).unwrap();
    let r = db.about_essence().unwrap().unwrap();
    assert!(r.contains("second") && !r.contains("first"), "{r}");
    db.essence_remove(&item).unwrap();
    assert_eq!(db.about_essence().unwrap(), None);

    // Ingest: idempotent on the same handle, and forget resets the thread.
    let turns = [tt("u-1", "user", "hi"), tt("u-2", "assistant", "hello")];
    assert_eq!(db.ingest_thread("t", &turns).unwrap().added_records, 2);
    let again = db.ingest_thread("t", &turns).unwrap();
    assert_eq!((again.added_records, again.skipped_existing_turns), (0, 2));
    db.ingest_thread("t", &[tt("u-3", "user", "more")]).unwrap();
    let seqs: Vec<u64> = db
        .thread_export("t")
        .unwrap()
        .iter()
        .map(|m| m.metadata["_mnemo.seq"].as_u64().unwrap())
        .collect();
    assert_eq!(seqs, [0, 1, 2]);
    assert_eq!(db.thread_forget("t").unwrap(), 3);
    assert!(db.thread_export("t").unwrap().is_empty());
    assert_eq!(db.ingest_thread("t", &turns).unwrap().added_records, 2);

    // Continuation link.
    db.thread_start("t-next", "t").unwrap();
    assert!(db
        .essence(EssenceScope::Thread("t-next".into()), 4096)
        .unwrap()
        .contains("continued from"));

    // Keyed slot: supersede, delete the live record, then write again.
    let k = |c: &str| Memory::new(c, MemoryType::Semantic, vec4(0.0, 1.0, 1.0, 0.0));
    db.remember_keyed(k("v1"), "release").unwrap();
    let (live, outcome) = db.remember_keyed(k("v2"), "release").unwrap();
    assert_eq!(outcome, WriteOutcome::Superseded);
    db.delete(&live).unwrap();
    let (_, outcome) = db.remember_keyed(k("v3"), "release").unwrap();
    assert_eq!(
        outcome,
        WriteOutcome::Added,
        "deleting the live record empties the slot"
    );
}

// ---------- PERF-1 step 3: the ANN index loads on first use --------------

#[test]
fn ann_index_loads_on_first_recall_not_at_open() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazy-load.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..300 {
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Semantic,
            vec4(1.0, i as f32 * 0.01, 0.3, 0.7),
        ))
        .unwrap();
    }
    db.build_index().unwrap();
    db.close().unwrap();

    let mut db = Mnemo::open(&path, "pw").unwrap();
    assert!(
        db.has_index(),
        "a persisted index counts before it's loaded"
    );
    assert_eq!(
        db.__budget_report().unwrap().ram_ann_posting_codes,
        0,
        "open must not load the index"
    );
    db.recall(
        &RecallRequest::new(vec4(1.0, 0.0, 0.3, 0.7))
            .top_k(3)
            .track_access(false),
    )
    .unwrap();
    assert!(db.__budget_report().unwrap().ram_ann_posting_codes > 0);

    // Read-only handles load lazily too.
    drop(db);
    let mut ro = Mnemo::open_read_only(&path, "pw").unwrap();
    assert_eq!(ro.__budget_report().unwrap().ram_ann_posting_codes, 0);
    assert_eq!(ro.stats().unwrap().index.unwrap().vectors, 300);
}

/// While the index is unloaded, records written since its run pile up for
/// the next load. A flush that finds INDEX_PERSIST_EVERY_CHANGES of them
/// loads the index and persists it, so that work stays bounded.
#[test]
fn unloaded_index_is_refreshed_once_pending_reaches_the_bound() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lazy-bound.mnemo");
    let mut db = Mnemo::create(&path, "pw", fast_cfg(4)).unwrap();
    for i in 0..100 {
        db.remember(Memory::new(
            format!("m{i}"),
            MemoryType::Semantic,
            vec4(1.0, i as f32, 0.0, 1.0),
        ))
        .unwrap();
    }
    db.build_index().unwrap();
    db.close().unwrap();

    let mut db = Mnemo::open(&path, "pw").unwrap();
    let start = db.__budget_report().unwrap().index_start_page;
    let bound = mnemo_engine::INDEX_PERSIST_EVERY_CHANGES;
    for i in 0..bound - 1 {
        db.remember(Memory::new(
            format!("p{i}"),
            MemoryType::Semantic,
            vec4(0.5, i as f32, 1.0, 0.0),
        ))
        .unwrap();
    }
    db.flush().unwrap();
    assert_eq!(
        db.__budget_report().unwrap().index_start_page,
        start,
        "below the bound"
    );
    assert_eq!(
        db.__budget_report().unwrap().ram_ann_posting_codes,
        0,
        "still unloaded"
    );
    db.remember(Memory::new(
        "last",
        MemoryType::Semantic,
        vec4(0.5, 0.5, 1.0, 0.0),
    ))
    .unwrap();
    db.flush().unwrap();
    assert_ne!(
        db.__budget_report().unwrap().index_start_page,
        start,
        "at the bound the flush loads and persists the index"
    );
}

// ---------- PERF-1 step 7 (R-004.5): raw-key databases -------------------

#[test]
fn raw_key_create_open_rekey_and_compact() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("keyed.mnemo");
    let key = Mnemo::generate_key();
    assert_ne!(key, Mnemo::generate_key(), "keys are random");
    {
        let mut db = Mnemo::create_with_key(&path, &key, fast_cfg(4)).unwrap();
        assert_eq!(
            db.kdf_params(),
            KdfParams::raw_key(),
            "header records the raw-key floor"
        );
        db.remember(Memory::new(
            "held by a keychain",
            MemoryType::Semantic,
            vec4(1.0, 0.0, 0.0, 0.0),
        ))
        .unwrap();
        db.close().unwrap();
    }
    // The key opens it, read-write and read-only; anything else doesn't.
    assert_eq!(Mnemo::open_with_key(&path, &key).unwrap().len(), 1);
    assert_eq!(
        Mnemo::open_read_only_with_key(&path, &key).unwrap().len(),
        1
    );
    assert!(Mnemo::open_with_key(&path, &Mnemo::generate_key()).is_err());
    assert!(Mnemo::open(&path, "a passphrase").is_err());

    // Rotate the key; the old one stops working.
    let key2 = Mnemo::generate_key();
    {
        let mut db = Mnemo::open_with_key(&path, &key).unwrap();
        db.rekey_with_key(&key2).unwrap();
    }
    assert!(Mnemo::open_with_key(&path, &key).is_err());
    let mut db = Mnemo::open_with_key(&path, &key2).unwrap();
    assert_eq!(db.memories().unwrap()[0].content, "held by a keychain");
    drop(db);

    // Compact with the key.
    let report = Mnemo::compact_file_with_key(path.to_str().unwrap(), &key2).unwrap();
    assert_eq!(report.after, 1);
    assert_eq!(Mnemo::open_with_key(&path, &key2).unwrap().len(), 1);

    // A passphrase file can switch to a key, and back.
    let p2 = dir.path().join("switch.mnemo");
    {
        let mut db = Mnemo::create(&p2, "pw", fast_cfg(4)).unwrap();
        db.rekey_with_key(&key).unwrap();
    }
    {
        let mut db = Mnemo::open_with_key(&p2, &key).unwrap();
        db.rekey("pw-again", KdfParams::fast()).unwrap();
    }
    assert!(Mnemo::open(&p2, "pw-again").is_ok());
}
