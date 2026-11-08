//! Property tests for the Mnemo state machine (Phase 1.3).
//!
//! Four properties, each self-contained. They share a model helper
//! ([`ModelSet`]) that tracks the content-plus-type multi-set the
//! engine should surface — mnemo assigns ULIDs, so the model cannot
//! predict IDs and instead compares by observable fingerprint.
//!
//! 1. [`prop_flush_reopen_preserves_memories`] — random `Remember` /
//!    `Delete` interleavings followed by `Flush` + `Reopen`. The
//!    reopened set must equal the model set.
//! 2. [`prop_batched_cache_never_corrupts`] — random cache ops under
//!    the batched flush policy. `cache_get` returns either `None` or
//!    exactly the last value put for that (namespace, key); never a
//!    stale / wrong / partial value across the crash boundary.
//! 3. [`prop_partial_flush_crash_reverts_cleanly`] — after a
//!    partial-flush crash (via the pre-existing
//!    `__crash_partial_flush_for_testing`, which flushes data pages
//!    without the WAL commit), reopen recovers to the last committed
//!    state — the un-committed transaction is invisible.
//! 4. [`prop_failpoint_write_yields_pre_or_post`] — arms the
//!    `__failpoints` write counter to fail the Nth write during
//!    `flush()` and asserts the reopened live-memory set equals
//!    either the pre-flush snapshot or the post-flush snapshot,
//!    never a third state. This is the strict "torn write" invariant
//!    the Phase 1.3 spec calls out.
//!
//! Cases are small (≤8 ops per sequence) so a full `cargo test` run
//! stays under a few seconds even at the default `PROPTEST_CASES=256`.
//! Tempfiles + `fast_cfg()` (Argon2 fast params) keep each iteration
//! cheap; reopens follow the strict close → drop → `Mnemo::open`
//! sequence so `fs4`'s exclusive lock is released cleanly on Windows.

use mnemo_engine::{
    CacheFlushPolicy, CachePutOpts, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, Ulid,
};
use proptest::prelude::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

const DIMS: usize = 4;

fn fast_cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: DIMS,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn cache_put_opts() -> CachePutOpts {
    CachePutOpts {
        content_type: "text".into(),
        ttl_secs: None,
        cost_hint_ms: None,
    }
}

/// Deterministic 4-D vector from a seed byte. Two seeds produce two
/// distinct vectors iff they're distinct bytes.
fn seed_vec(seed: u8) -> Vec<f32> {
    let x = (seed as f32) / 255.0;
    vec![x, 1.0 - x, x * 0.5, (1.0 - x) * 0.5]
}

fn memory_type_of(byte: u8) -> MemoryType {
    match byte % 3 {
        0 => MemoryType::Semantic,
        1 => MemoryType::Episodic,
        _ => MemoryType::Working,
    }
}

/// Observable fingerprint of a memory. Content + type. ULID is engine-
/// assigned so the model can't track it; content + type are what the
/// caller supplied and what a reopen must round-trip.
type Fingerprint = (String, String);

fn fp(m: &Memory) -> Fingerprint {
    (m.content.clone(), format!("{:?}", m.memory_type))
}

fn make_memory(seed: u8, mtype_byte: u8) -> (Memory, Fingerprint) {
    let content = format!("m{seed:03}-{mtype_byte:03}");
    let mtype = memory_type_of(mtype_byte);
    let mtype_str = format!("{:?}", mtype);
    let mem = Memory::new(content.clone(), mtype, seed_vec(seed));
    (mem, (content, mtype_str))
}

/// Read the observable multiset from an open Mnemo handle. Uses
/// `memories()` (the enumeration API); returns a `BTreeSet` so
/// equality comparisons are order-insensitive.
fn observed_set(db: &mut Mnemo) -> BTreeSet<Fingerprint> {
    db.memories()
        .expect("memories() should never fail on a healthy handle")
        .iter()
        .map(fp)
        .collect()
}

/// Fresh `.mnemo` file in a scratch tempdir. Returns the dir so the
/// caller can keep it alive for the lifetime of the handles.
fn fresh_db() -> (TempDir, PathBuf, Mnemo) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.mnemo");
    let db = Mnemo::create(&path, "pw", fast_cfg()).unwrap();
    (dir, path, db)
}

/// Close + drop + reopen. Mirrors the "process exit, then reboot"
/// sequence tests should model. `path` is `&Path` (not `&PathBuf`)
/// to keep clippy's `ptr_arg` lint happy under `-D warnings`.
fn reopen(db: Mnemo, path: &Path) -> Mnemo {
    drop(db);
    Mnemo::open(path, "pw").unwrap()
}

// ---------------------------------------------------------------------
// Property 1: flush + reopen preserves the live memory set.
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
enum MemOp {
    Remember { seed: u8, mtype: u8 },
    Delete { idx: u8 },
}

fn arb_mem_op() -> impl Strategy<Value = MemOp> {
    prop_oneof![
        3 => (any::<u8>(), any::<u8>()).prop_map(|(s, t)| MemOp::Remember { seed: s, mtype: t }),
        1 => any::<u8>().prop_map(|i| MemOp::Delete { idx: i }),
    ]
}

proptest! {
    /// A sequence of remember/delete ops, followed by flush + reopen,
    /// preserves the observable content-plus-type set. Deletes on an
    /// empty catalog are no-ops (the model skips them and the engine
    /// path is unreached).
    #[test]
    fn prop_flush_reopen_preserves_memories(ops in prop::collection::vec(arb_mem_op(), 1..=8)) {
        let (_dir, path, mut db) = fresh_db();
        // The model is a Vec so `Delete { idx }` maps to a stable
        // index (modulo current length). The Vec doubles as our
        // "expected observable set" via .iter().collect().
        let mut model: Vec<Fingerprint> = Vec::new();
        let mut ulids: Vec<Ulid> = Vec::new();

        for op in ops {
            match op {
                MemOp::Remember { seed, mtype } => {
                    let (m, f) = make_memory(seed, mtype);
                    let id = db.remember(m).unwrap();
                    model.push(f);
                    ulids.push(id);
                }
                MemOp::Delete { idx } => {
                    if model.is_empty() { continue; }
                    let i = (idx as usize) % model.len();
                    db.delete(&ulids[i]).unwrap();
                    model.remove(i);
                    ulids.remove(i);
                }
            }
        }
        db.flush().unwrap();
        let expected: BTreeSet<Fingerprint> = model.iter().cloned().collect();

        // Round 1: same handle after flush.
        prop_assert_eq!(observed_set(&mut db), expected.clone(),
            "after flush, live handle disagrees with model");

        // Round 2: reopen (close + drop + open) must see the same set.
        let mut db = reopen(db, &path);
        prop_assert_eq!(observed_set(&mut db), expected,
            "after reopen, durable state disagrees with model");
    }
}

// ---------------------------------------------------------------------
// Property 2: batched cache never surfaces a wrong/stale value.
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
enum CacheOp {
    Put { ns: u8, key: u8, value: u8 },
    Get { ns: u8, key: u8 },
    Flush,
    Reopen,
}

fn arb_cache_op() -> impl Strategy<Value = CacheOp> {
    prop_oneof![
        4 => (any::<u8>(), any::<u8>(), any::<u8>()).prop_map(|(n, k, v)| CacheOp::Put {
            ns: n, key: k, value: v,
        }),
        3 => (any::<u8>(), any::<u8>()).prop_map(|(n, k)| CacheOp::Get { ns: n, key: k }),
        2 => Just(CacheOp::Flush),
        1 => Just(CacheOp::Reopen),
    ]
}

fn ns_of(byte: u8) -> String {
    format!("n{}", byte % 3)
}
fn key_of(byte: u8) -> String {
    format!("k{}", byte % 8)
}

/// Value the engine actually stored. Small deterministic byte-string
/// keyed off the seed so it's easy to recognize in an assertion.
fn value_of(byte: u8) -> Vec<u8> {
    vec![byte; 4]
}

const MAX_DIRTY: usize = 3;

proptest! {
    /// Under `Batched { max_dirty: 3, max_age: 1h }`, `cache_get` for
    /// any (ns, key) returns either `None` or exactly the value most
    /// recently `cache_put` for that (ns, key) *that the engine
    /// still knows about*. Two things drop a batched put from view:
    /// (1) a reopen before it flushed, (2) TTL / eviction (not
    /// exercised here). The property must model both to avoid false
    /// positives — the CI run of PR 8 caught one such false positive
    /// on the case `Put(v0) → Flush → Put(v1, batched) → Reopen → Get`
    /// where the model predicted v1 but the engine correctly served
    /// the durable v0.
    #[test]
    fn prop_batched_cache_never_corrupts(ops in prop::collection::vec(arb_cache_op(), 1..=10)) {
        let (_dir, path, mut db) = fresh_db();
        db.set_cache_flush_policy(CacheFlushPolicy::Batched {
            max_dirty: MAX_DIRTY,
            max_age: std::time::Duration::from_secs(3600),
        });
        // Two-layer model:
        //   `durable`: (ns, key) -> value that has flushed to disk.
        //              Survives a reopen. Updated only on Flush (and
        //              on Batched auto-flush).
        //   `pending`: (ns, key) -> value put since the last flush,
        //              in insertion order. On Batched, once the count
        //              of *distinct pending entries* hits max_dirty
        //              the engine auto-flushes; we mirror that here.
        //              Dropped on Reopen.
        // The observable value at any (ns, key) is:
        //   pending.get(k).or_else(|| durable.get(k))
        let mut durable: BTreeMap<(String, String), Vec<u8>> = BTreeMap::new();
        let mut pending: BTreeMap<(String, String), Vec<u8>> = BTreeMap::new();
        // Raw mutation counter (matches the engine's `BatchState.dirty_count`
        // — increments on every put, even when the same key is overwritten).
        // Reset on flush + auto-flush; NOT reset on reopen (pending is
        // simply dropped instead).
        let mut mutations: usize = 0;

        for op in ops {
            match op {
                CacheOp::Put { ns, key, value } => {
                    let (n, k, v) = (ns_of(ns), key_of(key), value_of(value));
                    db.cache_put(&n, &k, &v, cache_put_opts()).unwrap();
                    pending.insert((n, k), v);
                    mutations += 1;
                    // Store's `cache_put`: insert → record_dirty →
                    // should_auto_flush → flush. So the current put IS
                    // included in the auto-flushed batch.
                    if mutations >= MAX_DIRTY {
                        for (k, v) in std::mem::take(&mut pending) {
                            durable.insert(k, v);
                        }
                        mutations = 0;
                    }
                }
                CacheOp::Get { ns, key } => {
                    let (n, k) = (ns_of(ns), key_of(key));
                    let hit = db.cache_get(&n, &k).unwrap();
                    // Observable value = pending overrides durable.
                    let expected = pending
                        .get(&(n.clone(), k.clone()))
                        .or_else(|| durable.get(&(n.clone(), k.clone())))
                        .cloned();
                    match (hit, expected) {
                        (None, _) => {} // acceptable in all cases
                        (Some(v), Some(exp)) => {
                            prop_assert_eq!(v.value, exp,
                                "cache_get returned wrong value for ({}, {})", n, k);
                        }
                        (Some(_), None) => {
                            prop_assert!(false,
                                "cache_get hit on ({}, {}) that was never put", n, k);
                        }
                    }
                }
                CacheOp::Flush => {
                    db.flush().unwrap();
                    for (k, v) in std::mem::take(&mut pending) {
                        durable.insert(k, v);
                    }
                    mutations = 0;
                }
                CacheOp::Reopen => {
                    db = reopen(db, &path);
                    db.set_cache_flush_policy(CacheFlushPolicy::Batched {
                        max_dirty: MAX_DIRTY,
                        max_age: std::time::Duration::from_secs(3600),
                    });
                    // Reopen drops all pending writes on the floor.
                    pending.clear();
                    mutations = 0;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Property 3: partial-flush crash reverts to last-committed state.
// ---------------------------------------------------------------------

proptest! {
    /// After a flush of `pre` remembers, a batch of `post` remembers
    /// followed by a partial-flush crash — data pages hit disk, WAL
    /// commit does NOT — reopen must show exactly the `pre` set. The
    /// `post` batch is invisible; the un-committed WAL tail is
    /// discarded by recovery.
    #[test]
    fn prop_partial_flush_crash_reverts_cleanly(
        pre in prop::collection::vec(any::<u8>(), 1..=4),
        post in prop::collection::vec(any::<u8>(), 1..=4),
    ) {
        let (_dir, path, mut db) = fresh_db();
        let mut committed: BTreeSet<Fingerprint> = BTreeSet::new();
        for (i, seed) in pre.iter().enumerate() {
            let (m, f) = make_memory(*seed, i as u8);
            db.remember(m).unwrap();
            committed.insert(f);
        }
        db.flush().unwrap();

        // Post batch: remembered but never committed.
        for (i, seed) in post.iter().enumerate() {
            let (m, _) = make_memory(*seed, (pre.len() + i) as u8);
            db.remember(m).unwrap();
        }
        // Crash mid-flush: data pages persist, WAL commit does not.
        db.__crash_partial_flush_for_testing().unwrap();

        let mut db = reopen(db, &path);
        prop_assert_eq!(observed_set(&mut db), committed,
            "partial-flush crash surfaced un-committed remembers on reopen");
    }
}

// ---------------------------------------------------------------------
// Property 4: fail-after-N-writes yields pre-flush OR post-flush.
// ---------------------------------------------------------------------

// ---------------------------------------------------------------------
// Property 5: restore_to + compact_file preserve per-snapshot state.
// ---------------------------------------------------------------------

#[derive(Clone, Debug)]
enum SnapOp {
    Remember { seed: u8, mtype: u8 },
    Flush,
    RestoreToIdx { idx: u8 },
    Compact,
    Reopen,
}

fn arb_snap_op() -> impl Strategy<Value = SnapOp> {
    prop_oneof![
        3 => (any::<u8>(), any::<u8>()).prop_map(|(s, t)| SnapOp::Remember { seed: s, mtype: t }),
        3 => Just(SnapOp::Flush),
        2 => any::<u8>().prop_map(|i| SnapOp::RestoreToIdx { idx: i }),
        1 => Just(SnapOp::Compact),
        1 => Just(SnapOp::Reopen),
    ]
}

proptest! {
    /// A snapshot restored via `restore_to(txn_id)` reproduces exactly
    /// the memory set that was durable at that snapshot's flush.
    /// `compact_file` rewrites the file without changing the live set.
    /// Both survive a subsequent reopen. The model records one
    /// `(txn_id, live_set)` per completed flush and cross-checks after
    /// each op.
    #[test]
    fn prop_restore_and_compact_preserve_snapshots(
        ops in prop::collection::vec(arb_snap_op(), 2..=8),
    ) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.mnemo");
        let mut db = Mnemo::create(&path, "pw", fast_cfg()).unwrap();
        // `live`    — what the model expects the current handle to show.
        // `durable` — what a reopen (dropping the handle) would show.
        //             Updated only when the engine flushes (either via
        //             `Flush` or as part of `RestoreTo` / `Compact`).
        let mut live: BTreeSet<Fingerprint> = BTreeSet::new();
        let mut durable: BTreeSet<Fingerprint> = BTreeSet::new();
        // `model_snaps` — (txn_id, live_set) for every flush we've seen
        //                 the engine mint. Grows on Flush + Restore;
        //                 rebuilt on Compact (which rewrites the file).
        let mut model_snaps: Vec<(u64, BTreeSet<Fingerprint>)> = Vec::new();

        for op in ops {
            match op {
                SnapOp::Remember { seed, mtype } => {
                    let (m, f) = make_memory(seed, mtype);
                    db.remember(m).unwrap();
                    live.insert(f);
                }
                SnapOp::Flush => {
                    db.flush().unwrap();
                    durable = live.clone();
                    let all = db.snapshots();
                    if let Some(latest) = all.iter().max_by_key(|s| s.txn_id) {
                        let already = model_snaps.iter().any(|(t, _)| *t == latest.txn_id);
                        if !already {
                            model_snaps.push((latest.txn_id, live.clone()));
                        }
                    }
                }
                SnapOp::RestoreToIdx { idx } => {
                    if model_snaps.is_empty() { continue; }
                    let i = (idx as usize) % model_snaps.len();
                    let (txn, snap_live) = model_snaps[i].clone();
                    // `restore_to` is itself a committed transaction —
                    // it snapshots + flushes the restored state, so
                    // both `live` and `durable` shift together.
                    db.restore_to(txn).unwrap();
                    live = snap_live;
                    durable = live.clone();
                    prop_assert_eq!(observed_set(&mut db), live.clone(),
                        "restore_to({}) diverged from model", txn);
                    let all = db.snapshots();
                    if let Some(latest) = all.iter().max_by_key(|s| s.txn_id) {
                        let already = model_snaps.iter().any(|(t, _)| *t == latest.txn_id);
                        if !already {
                            model_snaps.push((latest.txn_id, live.clone()));
                        }
                    }
                }
                SnapOp::Compact => {
                    // `compact_file` requires the caller's own handle
                    // closed (it opens the file itself). Flush first so
                    // any pending writes make it into the compacted
                    // rewrite, then treat the reopen the same as a
                    // regular reopen — `live` and `durable` converge on
                    // the flushed state, and the manifest is fresh.
                    db.flush().unwrap();
                    durable = live.clone();
                    drop(db);
                    let _ = Mnemo::compact_file(path.to_str().unwrap(), "pw").unwrap();
                    db = Mnemo::open(&path, "pw").unwrap();
                    prop_assert_eq!(observed_set(&mut db), live.clone(),
                        "compact_file changed the observable live set");
                    // Compaction rebuilds the manifest fresh, so the
                    // model must drop prior snapshots and pick up the
                    // single post-compact one.
                    model_snaps.clear();
                    let all = db.snapshots();
                    if let Some(latest) = all.iter().max_by_key(|s| s.txn_id) {
                        model_snaps.push((latest.txn_id, live.clone()));
                    }
                }
                SnapOp::Reopen => {
                    db = reopen(db, &path);
                    live = durable.clone();
                    prop_assert_eq!(observed_set(&mut db), live.clone(),
                        "reopen diverged from durable live set");
                }
            }
        }
    }
}

proptest! {
    /// Arm the pager (and WAL commit) to fail its Nth subsequent
    /// write during `flush()`. The flush errors; the caller drops the
    /// handle; on reopen, the live memory set equals EITHER the
    /// pre-flush snapshot OR the post-flush snapshot — never a third
    /// state. This is the atomicity invariant Phase 1.3 exists to
    /// verify.
    #[test]
    fn prop_failpoint_write_yields_pre_or_post(
        pre in prop::collection::vec(any::<u8>(), 1..=3),
        post in prop::collection::vec(any::<u8>(), 1..=3),
        fail_at in 0i64..12,
    ) {
        let (_dir, path, mut db) = fresh_db();
        // Establish a durable "pre" state.
        let mut pre_set: BTreeSet<Fingerprint> = BTreeSet::new();
        for (i, seed) in pre.iter().enumerate() {
            let (m, f) = make_memory(*seed, i as u8);
            db.remember(m).unwrap();
            pre_set.insert(f);
        }
        db.flush().unwrap();

        // Stage the "post" additions.
        let mut post_set = pre_set.clone();
        for (i, seed) in post.iter().enumerate() {
            let (m, f) = make_memory(*seed, (pre.len() + i) as u8);
            db.remember(m).unwrap();
            post_set.insert(f);
        }

        // Arm the failpoint. Flush likely errors (torn write); if
        // `fail_at` happens to exceed the write count, flush succeeds
        // and we observe the post state — also acceptable.
        mnemo_engine::__failpoints::set_writes_until_fail(fail_at);
        let _ = db.flush();
        // Disarm defensively — flush may have consumed the counter,
        // but we don't want a subsequent drop/reopen path (which
        // performs its own no-op writes) to trip a stray failpoint.
        mnemo_engine::__failpoints::set_writes_until_fail(-1);

        let mut db = reopen(db, &path);
        let observed = observed_set(&mut db);
        prop_assert!(
            observed == pre_set || observed == post_set,
            "torn flush surfaced a third state: observed={:?} pre={:?} post={:?}",
            observed, pre_set, post_set,
        );
    }
}

// ---------------------------------------------------------------------
// DUR-1: torn writes under WAL relocation and rekey.
//
// Property 4 above sweeps `fail_at` over an ordinary flush. DUR-1 added
// two more code paths that now also commit through the WAL: WAL
// relocation (grown region at the file tail) and `rekey` (new wrapped
// DEK). This property stresses both. Each case:
//
//   1. Builds a durable "pre" state.
//   2. Stages either a very large transaction (forces relocation on
//      the next flush) or a `rekey` with a possibly-different kdf.
//   3. Arms the failpoint.
//   4. Reopens and asserts the live set equals pre or post, never a
//      third state, AND asserts the passphrase that opens the file is
//      consistent with what the file now stores (so a crashed rekey
//      can only land on old or new, never both-fail).
// ---------------------------------------------------------------------

proptest! {
    #[test]
    fn prop_failpoint_relocation_or_rekey_yields_pre_or_post(
        pre in prop::collection::vec(any::<u8>(), 1..=3),
        extra in prop::collection::vec(any::<u8>(), 1..=3),
        // 0 → relocation (large staged txn), 1 → rekey.
        choice in 0u8..=1,
        fail_at in 0i64..40,
    ) {
        let (_dir, path, mut db) = fresh_db();

        // Durable "pre" state.
        let mut pre_set: BTreeSet<Fingerprint> = BTreeSet::new();
        for (i, seed) in pre.iter().enumerate() {
            let (m, f) = make_memory(*seed, i as u8);
            db.remember(m).unwrap();
            pre_set.insert(f);
        }
        db.flush().unwrap();

        let (post_set, new_pw) = match choice {
            0 => {
                // Relocation path: stage a large transaction so flush
                // must grow the WAL. 4000 small vectors reliably
                // exceeds the default WAL even for pre.len() == 1.
                let mut set = pre_set.clone();
                for (i, seed) in extra.iter().cycle().take(4000).enumerate() {
                    let (m, f) = make_memory(*seed, (pre.len() + i) as u8);
                    db.remember(m).unwrap();
                    set.insert(f);
                }
                mnemo_engine::__failpoints::set_writes_until_fail(fail_at);
                let _ = db.flush();
                mnemo_engine::__failpoints::set_writes_until_fail(-1);
                (set, None)
            }
            _ => {
                // Rekey path: the live set stays pre_set either way.
                // We only care that at least one passphrase opens.
                mnemo_engine::__failpoints::set_writes_until_fail(fail_at);
                let _ = db.rekey("new-pw", KdfParams::fast());
                mnemo_engine::__failpoints::set_writes_until_fail(-1);
                (pre_set.clone(), Some("new-pw"))
            }
        };

        drop(db);

        // Reopen using whichever passphrase works. For the rekey case
        // either "pw" (old) or "new-pw" (new) must open — never
        // neither.
        let mut reopened = Mnemo::open(&path, "pw");
        if reopened.is_err() {
            if let Some(np) = new_pw {
                reopened = Mnemo::open(&path, np);
            }
        }
        let mut db = reopened.unwrap_or_else(|e| {
            panic!("reopen after choice={choice} fail_at={fail_at} failed: {e:?}")
        });
        let observed = observed_set(&mut db);
        prop_assert!(
            observed == pre_set || observed == post_set,
            "torn relocation/rekey surfaced a third state: \
             choice={} fail_at={} observed={:?} pre={:?} post={:?}",
            choice, fail_at, observed, pre_set, post_set,
        );
    }
}

// --- PR SEC-1: no page-nonce ever repeats within a file ---
//
// Fifth property, added for the v0.5.2 security hotfix. Random
// Remember/Flush/Reopen sequences on one `.mnemo` file; after the run,
// scan every encrypted page's 12-byte nonce and assert they're all
// distinct. Together with the targeted "no cross-copy nonce" and "fresh
// prefix per reopen" integration tests in sec1_nonce_uniqueness.rs,
// this covers the AES-GCM uniqueness invariant across the engine's
// actual write surface (not just a hand-rolled sequence).

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

const SEC1_PAGE_SIZE: usize = 8192;
const SEC1_NONCE_LEN: usize = 12;

fn sec1_collect_nonces(path: &Path) -> BTreeSet<[u8; SEC1_NONCE_LEN]> {
    let mut f = File::open(path).expect("open");
    let len = f.metadata().expect("stat").len();
    let page_count = (len / SEC1_PAGE_SIZE as u64) as usize;
    let mut out = BTreeSet::new();
    // Skip page 0 (plaintext header).
    for page_no in 1..page_count {
        f.seek(SeekFrom::Start((page_no as u64) * SEC1_PAGE_SIZE as u64))
            .expect("seek");
        let mut page = [0u8; SEC1_PAGE_SIZE];
        if f.read_exact(&mut page).is_err() {
            break;
        }
        let mut nonce = [0u8; SEC1_NONCE_LEN];
        nonce.copy_from_slice(&page[..SEC1_NONCE_LEN]);
        if nonce != [0u8; SEC1_NONCE_LEN] {
            out.insert(nonce);
        }
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 64,
        max_shrink_iters: 1024,
        ..Default::default()
    })]

    #[test]
    fn prop_no_page_nonce_repeats_within_file(
        ops in prop::collection::vec(
            (
                prop::collection::vec(any::<u8>(), 1..32), // content bytes
                any::<bool>(),                              // flush after?
                any::<bool>(),                              // reopen after?
            ),
            1..12,
        ),
    ) {
        let td = TempDir::new().unwrap();
        let path = td.path().join("prop-sec1.mnemo");
        let dims = 4;
        let mut db = Mnemo::create(&path, "pw", MnemoConfig {
            dimensions: dims,
            kdf: KdfParams::fast(),
            ..Default::default()
        }).unwrap();

        for (i, (content, should_flush, should_reopen)) in ops.iter().enumerate() {
            // Distinct per-op vector so the engine actually writes a page.
            let v: Vec<f32> = (0..dims)
                .map(|k| (i as f32 + k as f32 + 1.0) * 0.17)
                .collect();
            db.remember(Memory::new(
                format!("op-{i}:{}", content.iter().map(|b| *b as u32).sum::<u32>()),
                MemoryType::Semantic,
                v,
            ))
            .unwrap();
            if *should_flush || *should_reopen {
                db.flush().unwrap();
            }
            if *should_reopen {
                db.close().unwrap();
                drop(db);
                db = Mnemo::open(&path, "pw").unwrap();
            }
        }
        db.flush().unwrap();
        db.close().unwrap();
        drop(db);

        // Count stored (nonce) entries vs scanned pages. BTreeSet
        // deduplicates — a repeat would make the set shorter than the
        // page count. We can't know the exact page count up front
        // (compaction, WAL frames reused for later txns, etc.), so
        // compare set.len() to the count of non-empty page slots we
        // actually read.
        let mut f = File::open(&path).unwrap();
        let total_pages = (f.metadata().unwrap().len() / SEC1_PAGE_SIZE as u64) as usize;
        let mut non_empty = 0usize;
        for page_no in 1..total_pages {
            f.seek(SeekFrom::Start((page_no as u64) * SEC1_PAGE_SIZE as u64)).unwrap();
            let mut page = [0u8; SEC1_PAGE_SIZE];
            if f.read_exact(&mut page).is_err() {
                break;
            }
            let mut nonce = [0u8; SEC1_NONCE_LEN];
            nonce.copy_from_slice(&page[..SEC1_NONCE_LEN]);
            if nonce != [0u8; SEC1_NONCE_LEN] {
                non_empty += 1;
            }
        }
        let unique = sec1_collect_nonces(&path).len();
        prop_assert_eq!(
            unique, non_empty,
            "page-nonce repeat detected: {} unique nonces across {} non-empty pages",
            unique, non_empty,
        );
    }
}

// ---------------------------------------------------------------------
// Phase 12.7d: `commit_deltas` is all-or-nothing.
//
// Random batches mix valid items (facts with or without a vector,
// essence adds / timeline entries / a resolve of an existing item,
// archive turns) with invalid ones (a wrong-width vector, an oversized
// essence item, an update of a missing id, an empty turn id, turns
// without a thread id). The model predicts whether the batch is valid:
//
// * invalid → `commit_deltas` errors, the live handle shows exactly the
//   pre-batch set (nothing staged), and so does a reopen;
// * valid → the live handle and a reopen show pre ∪ the batch's records;
// * valid with a write failpoint armed → the single flush fails or not,
//   and a reopen shows either the pre set or the post set, never a mix.
// ---------------------------------------------------------------------

use mnemo_engine::{
    CommitBatch, Embedder, EmbedderIdentity, EssenceOp, EssenceOpKind, EssenceScope,
    EssenceSection, KeyedMemory, ThreadTurn, TurnPolicy,
};

/// Deterministic text embedder for the batch property: the byte sum of
/// the text picks a `seed_vec`, which is never all-zero.
struct SeedEmbedder;
impl Embedder for SeedEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "seed-4d".into(),
            dimensions: DIMS,
            normalize: false,
        }
    }
    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        Ok(texts
            .iter()
            .map(|t| seed_vec(t.bytes().fold(0u8, |a, b| a.wrapping_add(b))))
            .collect())
    }
}

#[derive(Clone, Debug)]
enum BatchItem {
    FactWithVector(u8),
    FactTextOnly(u8),
    FactWrongWidth,
    EssenceAdd(u8),
    EssenceAddOversized,
    TimelineEntry(u8),
    ResolveExisting,
    UpdateMissing,
    Turn(u8),
    TurnEmptyId,
}

fn arb_batch_item() -> impl Strategy<Value = BatchItem> {
    prop_oneof![
        3 => any::<u8>().prop_map(BatchItem::FactWithVector),
        3 => any::<u8>().prop_map(BatchItem::FactTextOnly),
        1 => Just(BatchItem::FactWrongWidth),
        2 => any::<u8>().prop_map(BatchItem::EssenceAdd),
        1 => Just(BatchItem::EssenceAddOversized),
        2 => any::<u8>().prop_map(BatchItem::TimelineEntry),
        1 => Just(BatchItem::ResolveExisting),
        1 => Just(BatchItem::UpdateMissing),
        3 => any::<u8>().prop_map(BatchItem::Turn),
        1 => Just(BatchItem::TurnEmptyId),
    ]
}

fn arb_turn_policy() -> impl Strategy<Value = TurnPolicy> {
    prop_oneof![
        3 => Just(TurnPolicy::Archive),
        1 => Just(TurnPolicy::ExpireAfter(3600)),
        1 => Just(TurnPolicy::Drop),
    ]
}

proptest! {
    #[test]
    fn prop_commit_deltas_is_all_or_nothing(
        items in prop::collection::vec(arb_batch_item(), 1..=8),
        policy in arb_turn_policy(),
        omit_thread_id in prop::bool::weighted(0.1),
        fail_at in prop::option::weighted(0.3, 0i64..40),
    ) {
        let (_dir, path, mut db) = fresh_db();
        db.remember(Memory::scaffold_manifest(DIMS)).unwrap();
        db.set_embedder(SeedEmbedder).unwrap();
        let (m, _) = make_memory(7, 0);
        db.remember(m).unwrap();
        let open_item = db
            .essence_add(EssenceScope::Project, EssenceSection::Open, "pre-existing open item", None)
            .unwrap();
        db.flush().unwrap();
        let pre_set = observed_set(&mut db);

        // Build the batch and the model's prediction side by side.
        let mut batch = CommitBatch {
            thread_id: (!omit_thread_id).then(|| "thread-p".to_string()),
            turn_policy: policy,
            ..Default::default()
        };
        let mut valid = true;
        let mut post_set = pre_set.clone();
        let archives = !matches!(policy, TurnPolicy::Drop);
        let essence_fp = |text: &str| (text.to_string(), format!("{:?}", MemoryType::Semantic));
        for (i, item) in items.iter().enumerate() {
            let essence_op = |kind, section, text: String| EssenceOp {
                kind,
                scope: Some(EssenceScope::Thread("thread-p".into())),
                section,
                id: None,
                text: Some(text),
                refs: None,
            };
            match item {
                BatchItem::FactWithVector(seed) => {
                    let (mut m, f) = make_memory(*seed, i as u8);
                    m.content = format!("{}-v{i}", m.content);
                    post_set.insert((m.content.clone(), f.1));
                    batch.facts.push(KeyedMemory { memory: m, key: None });
                }
                BatchItem::FactTextOnly(seed) => {
                    let content = format!("text-fact-{seed}-{i}");
                    post_set.insert((content.clone(), format!("{:?}", MemoryType::Semantic)));
                    batch.facts.push(KeyedMemory {
                        memory: Memory::new(content, MemoryType::Semantic, Vec::new()),
                        key: None,
                    });
                }
                BatchItem::FactWrongWidth => {
                    valid = false;
                    batch.facts.push(KeyedMemory {
                        memory: Memory::new(format!("bad-{i}"), MemoryType::Semantic, vec![1.0, 0.0]),
                        key: None,
                    });
                }
                BatchItem::EssenceAdd(seed) => {
                    let text = format!("essence-{seed}-{i}");
                    post_set.insert(essence_fp(&text));
                    batch.essence_ops.push(essence_op(EssenceOpKind::Add, Some(EssenceSection::Next), text));
                }
                BatchItem::EssenceAddOversized => {
                    valid = false;
                    batch.essence_ops.push(essence_op(EssenceOpKind::Add, Some(EssenceSection::Next), "x".repeat(300)));
                }
                BatchItem::TimelineEntry(seed) => {
                    let text = format!("milestone-{seed}-{i}");
                    post_set.insert(essence_fp(&text));
                    batch.essence_ops.push(essence_op(EssenceOpKind::AppendTimeline, None, text));
                }
                BatchItem::ResolveExisting => {
                    batch.essence_ops.push(EssenceOp {
                        kind: EssenceOpKind::Resolve,
                        scope: None,
                        section: None,
                        id: Some(open_item),
                        text: None,
                        refs: None,
                    });
                }
                BatchItem::UpdateMissing => {
                    valid = false;
                    batch.essence_ops.push(EssenceOp {
                        kind: EssenceOpKind::Update,
                        scope: None,
                        section: None,
                        id: Some(Ulid::new()),
                        text: Some("never applied".into()),
                        refs: None,
                    });
                }
                BatchItem::Turn(seed) => {
                    let content = format!("turn-{seed}-{i}");
                    if archives {
                        post_set.insert((content.clone(), format!("{:?}", MemoryType::Episodic)));
                    }
                    batch.turns.push(ThreadTurn {
                        turn_id: format!("t{i}"),
                        role: "user".into(),
                        content,
                        timestamp: None,
                    });
                }
                BatchItem::TurnEmptyId => {
                    // Only an archived turn reaches the turn-id check.
                    if archives {
                        valid = false;
                    }
                    batch.turns.push(ThreadTurn {
                        turn_id: String::new(),
                        role: "user".into(),
                        content: format!("orphan-{i}"),
                        timestamp: None,
                    });
                }
            }
        }
        if !batch.turns.is_empty() && omit_thread_id {
            valid = false;
        }

        if !valid {
            prop_assert!(db.commit_deltas(batch).is_err(), "invalid batch was accepted");
            prop_assert_eq!(observed_set(&mut db), pre_set.clone(),
                "a rejected batch left records staged on the handle");
            let mut db = reopen(db, &path);
            prop_assert_eq!(observed_set(&mut db), pre_set, "a rejected batch reached the file");
            return Ok(());
        }

        match fail_at {
            None => {
                db.commit_deltas(batch).unwrap();
                prop_assert_eq!(observed_set(&mut db), post_set.clone(),
                    "live handle disagrees with the model after a valid batch");
                let mut db = reopen(db, &path);
                prop_assert_eq!(observed_set(&mut db), post_set, "reopen disagrees with the model");
            }
            Some(n) => {
                mnemo_engine::__failpoints::set_writes_until_fail(n);
                let _ = db.commit_deltas(batch);
                mnemo_engine::__failpoints::set_writes_until_fail(-1);
                let mut db = reopen(db, &path);
                let observed = observed_set(&mut db);
                prop_assert!(
                    observed == pre_set || observed == post_set,
                    "torn commit_deltas surfaced a third state: fail_at={} observed={:?} pre={:?} post={:?}",
                    n, observed, pre_set, post_set,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// PERF-1 step 2: the ANN index is derived data.
//
// `flush` persists the index only every so often, so a reopen (after a
// clean close, or a crash-equivalent drop) must re-assign every record
// written since the persisted index run and drop deleted ids. The model
// tracks the durable live set; after the run, ANN recall with full probe
// and rerank must find each live vector's own id first, and never a
// deleted one.
// ---------------------------------------------------------------------

use mnemo_engine::{MemoryPatch, Metric, RecallRequest, ScoreWeights};

#[derive(Clone, Debug)]
enum IdxOp {
    Remember,
    Delete(u8),
    Update(u8),
    Flush,
    FlushIndex,
    Crash,
    Close,
}

fn arb_idx_op() -> impl Strategy<Value = IdxOp> {
    prop_oneof![
        4 => Just(IdxOp::Remember),
        2 => any::<u8>().prop_map(IdxOp::Delete),
        2 => any::<u8>().prop_map(IdxOp::Update),
        3 => Just(IdxOp::Flush),
        1 => Just(IdxOp::FlushIndex),
        2 => Just(IdxOp::Crash),
        1 => Just(IdxOp::Close),
    ]
}

/// A vector unique to counter `c`.
fn unique_vec(c: u32) -> Vec<f32> {
    let x = c as f32;
    vec![
        1.0 + x * 0.013,
        (x * 0.37).sin(),
        (x * 0.11).cos(),
        0.5 + x * 0.007,
    ]
}

fn l2_top(db: &mut Mnemo, v: &[f32], k: usize) -> Vec<u128> {
    let mut req = RecallRequest::new(v.to_vec())
        .top_k(k)
        .n_probe(10_000)
        .n_rerank(10_000)
        .track_access(false);
    req.metric = Metric::L2;
    req.weights = ScoreWeights {
        alpha: 1.0,
        beta: 0.0,
        gamma: 0.0,
        delta: 0.0,
        half_life_secs: 1.0,
    };
    db.recall(&req)
        .unwrap()
        .iter()
        .map(|h| h.memory.id.0)
        .collect()
}

proptest! {
    #[test]
    fn prop_index_is_derived_data(ops in prop::collection::vec(arb_idx_op(), 1..=10)) {
        let (_dir, path, mut db) = fresh_db();
        let mut counter = 0u32;
        // (id, current vector) of every live memory, staged and durable.
        let mut staged: Vec<(Ulid, Vec<f32>)> = Vec::new();
        for _ in 0..30 {
            let v = unique_vec(counter);
            counter += 1;
            let id = db.remember(Memory::new(format!("m{counter}"), MemoryType::Semantic, v.clone())).unwrap();
            staged.push((id, v));
        }
        db.build_index().unwrap();
        db.flush().unwrap();
        let mut durable = staged.clone();
        let mut gone: Vec<(Ulid, Vec<f32>)> = Vec::new();
        let mut gone_durable = gone.clone();

        for op in ops {
            match op {
                IdxOp::Remember => {
                    let v = unique_vec(counter);
                    counter += 1;
                    let id = db.remember(Memory::new(format!("m{counter}"), MemoryType::Semantic, v.clone())).unwrap();
                    staged.push((id, v));
                }
                IdxOp::Delete(i) => {
                    if staged.len() <= 2 { continue; }
                    let (id, v) = staged.remove(i as usize % staged.len());
                    db.delete(&id).unwrap();
                    gone.push((id, v));
                }
                IdxOp::Update(i) => {
                    if staged.is_empty() { continue; }
                    let j = i as usize % staged.len();
                    let v = unique_vec(counter);
                    counter += 1;
                    db.update(&staged[j].0, MemoryPatch { vector: Some(v.clone()), ..Default::default() }).unwrap();
                    staged[j].1 = v;
                }
                IdxOp::Flush | IdxOp::FlushIndex => {
                    if matches!(op, IdxOp::Flush) { db.flush().unwrap() } else { db.flush_index().unwrap() }
                    durable = staged.clone();
                    gone_durable = gone.clone();
                }
                IdxOp::Crash => {
                    db = reopen(db, &path);
                    staged = durable.clone();
                    gone = gone_durable.clone();
                }
                IdxOp::Close => {
                    db.close().unwrap();
                    durable = staged.clone();
                    gone_durable = gone.clone();
                    db = reopen(db, &path);
                }
            }
        }
        // Finish with a crash-equivalent reopen of the durable state.
        db = reopen(db, &path);
        prop_assert!(db.has_index());
        for (id, v) in &durable {
            prop_assert_eq!(l2_top(&mut db, v, 1), vec![id.0], "a live memory isn't its own nearest neighbour");
        }
        for (id, v) in &gone_durable {
            prop_assert!(!l2_top(&mut db, v, 3).contains(&id.0), "a deleted id came back");
        }
    }
}
