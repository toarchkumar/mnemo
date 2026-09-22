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

use mnemo::{
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
        mnemo::__failpoints::set_writes_until_fail(fail_at);
        let _ = db.flush();
        // Disarm defensively — flush may have consumed the counter,
        // but we don't want a subsequent drop/reopen path (which
        // performs its own no-op writes) to trip a stray failpoint.
        mnemo::__failpoints::set_writes_until_fail(-1);

        let mut db = reopen(db, &path);
        let observed = observed_set(&mut db);
        prop_assert!(
            observed == pre_set || observed == post_set,
            "torn flush surfaced a third state: observed={:?} pre={:?} post={:?}",
            observed, pre_set, post_set,
        );
    }
}
