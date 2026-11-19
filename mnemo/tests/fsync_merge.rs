//! PERF-2 3f (v0.20.0): one fsync, the barrier, makes a commit's data pages
//! and its big runs (FMT-9c's direct writes) durable before the WAL commit,
//! where there were two. The barrier stays a sync of its own, never merged
//! with the commit's.
//!
//! The write and sync failpoints and the I/O trace are per thread, so these
//! tests can share a binary. Needs the `failpoints` feature (CI's `cargo
//! test --features failpoints`).
#![cfg(feature = "failpoints")]

use mnemo_engine::__failpoints::{self as fp, IoEvent};
use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig};
use std::path::Path;
use tempfile::tempdir;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 8,
        kdf: KdfParams::fast(),
        format_version: 9,
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    let v: Vec<f32> = (0..8)
        .map(|k| ((i * 7 + k * 3) % 11) as f32 + 0.5)
        .collect();
    Memory::new(format!("memory {i}"), MemoryType::Semantic, v)
}

/// A closed v9 file of `n` memories.
fn base(dir: &Path, n: usize) -> std::path::PathBuf {
    let p = dir.join("base.mnemo");
    let mut db = Mnemo::create(&p, "pw", cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    p
}

fn state(p: &Path) -> (usize, usize) {
    let mut db = Mnemo::open(p, "pw").unwrap();
    (db.len(), db.verify().unwrap())
}

/// The commit under test: `n` new memories in one flush, enough that the
/// catalog delta is a big run written outside the WAL.
fn batch(db: &mut Mnemo, n: usize) -> mnemo_engine::Result<()> {
    stage(db, n)?;
    db.flush()
}

/// The batch's writes before its flush. The first one takes the session's
/// generation lease, a commit of its own, so a trace starts after this.
fn stage(db: &mut Mnemo, n: usize) -> mnemo_engine::Result<()> {
    for i in 0..n {
        db.remember(mem(100_000 + i))?;
    }
    Ok(())
}

/// The trace's shape for one commit: every page written before the WAL
/// commit, then exactly one sync (the barrier), then the commit and its own
/// sync. Returns the number of pages written before the commit.
fn assert_one_barrier(trace: &[IoEvent]) -> usize {
    let commit = trace
        .iter()
        .position(|e| *e == IoEvent::WalCommit)
        .unwrap_or_else(|| panic!("no WAL commit in {trace:?}"));
    let before = &trace[..commit];
    let syncs: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] == IoEvent::Sync)
        .collect();
    assert_eq!(syncs.len(), 1, "one barrier before the commit: {before:?}");
    let barrier = syncs[0];
    assert!(
        before[barrier + 1..].is_empty(),
        "nothing is written between the barrier and the commit: {before:?}"
    );
    // The commit's own fsync follows it at once: never the same sync.
    assert_eq!(trace.get(commit + 1), Some(&IoEvent::Sync), "{trace:?}");
    before
        .iter()
        .filter(|e| matches!(e, IoEvent::Page(_)))
        .count()
}

#[test]
fn a_batch_with_a_big_run_makes_three_fsyncs_with_one_barrier() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 3000);
    let mut db = Mnemo::open(&p, "pw").unwrap();
    stage(&mut db, 1500).unwrap();
    fp::set_io_trace(true);
    db.flush().unwrap();
    let trace = fp::io_trace();
    fp::set_io_trace(false);
    // Data pages and the big catalog run: more pages than the WAL takes
    // for a run (FMT-9c's limit is 2).
    let pages = assert_one_barrier(&trace);
    assert!(pages > 2, "{pages} pages before the commit: {trace:?}");
    assert_eq!(db.stats().unwrap().fsyncs_last_flush, 3);
    let syncs = trace.iter().filter(|e| **e == IoEvent::Sync).count();
    assert_eq!(syncs, 3, "{trace:?}");
    db.close().unwrap();
    assert_eq!(state(&p), (4500, 4500));
}

#[test]
fn a_single_write_still_makes_three_fsyncs() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 3000);
    let mut db = Mnemo::open(&p, "pw").unwrap();
    db.remember(mem(50_000)).unwrap();
    fp::set_io_trace(true);
    db.flush().unwrap();
    let trace = fp::io_trace();
    fp::set_io_trace(false);
    assert!(assert_one_barrier(&trace) >= 1);
    assert_eq!(db.stats().unwrap().fsyncs_last_flush, 3);
    db.close().unwrap();
    assert_eq!(state(&p), (3001, 3001));
}

/// A write that fails, at each write of a big commit in turn, leaves the
/// file before the commit or after it: no committed memory lost, no
/// uncommitted one visible.
#[test]
fn a_crash_at_every_write_of_a_big_commit_is_pre_or_post() {
    let dir = tempdir().unwrap();
    let b = base(dir.path(), 3000);
    let pre = state(&b);
    let post = (pre.0 + 1500, pre.1 + 1500);
    // How many writes the commit makes, counted on a copy.
    let writes = {
        let p = dir.path().join("count.mnemo");
        std::fs::copy(&b, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        fp::set_writes_failing(&[]);
        batch(&mut db, 1500).unwrap();
        let n = fp::writes_seen();
        db.close().unwrap();
        n
    };
    assert!(writes > 10, "{writes} writes");
    let (mut saw_pre, mut saw_post) = (false, false);
    for k in 0..writes {
        let p = dir.path().join(format!("w{k}.mnemo"));
        std::fs::copy(&b, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        fp::set_writes_failing(&[k]);
        let res = batch(&mut db, 1500);
        fp::set_writes_failing(&[]);
        drop(db);
        let got = state(&p);
        assert!(got == pre || got == post, "write {k}: {got:?} ({res:?})");
        saw_pre |= got == pre;
        saw_post |= got == post;
        std::fs::remove_file(&p).unwrap();
    }
    assert!(saw_pre && saw_post);
}

/// A sync that fails, at each of the commit's three, leaves the file before
/// or after the commit; a failed barrier always leaves it before, since the
/// commit frame isn't written.
#[test]
fn a_failed_sync_is_pre_or_post_and_a_failed_barrier_is_pre() {
    let dir = tempdir().unwrap();
    let b = base(dir.path(), 3000);
    let pre = state(&b);
    let post = (pre.0 + 1500, pre.1 + 1500);
    for k in 0..3 {
        let p = dir.path().join(format!("s{k}.mnemo"));
        std::fs::copy(&b, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        fp::set_syncs_until_fail(k);
        let res = batch(&mut db, 1500);
        fp::set_syncs_until_fail(-1);
        assert!(res.is_err(), "sync {k} failed, so the flush did");
        drop(db);
        let got = state(&p);
        if k == 0 {
            assert_eq!(got, pre, "a failed barrier commits nothing");
        } else {
            assert!(got == pre || got == post, "sync {k}: {got:?}");
        }
    }
}

/// The barrier's flag stays set until a sync succeeds, so a commit retried
/// after its barrier failed syncs the pages the first attempt wrote, even
/// when the retry writes no page of its own before the commit. And the
/// failed attempt's header and free list are put back, so the retry commits
/// over the committed state, not over the runs the failed attempt staged.
#[test]
fn a_retry_after_a_failed_barrier_syncs_before_its_commit() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 3000);
    let mut db = Mnemo::open(&p, "pw").unwrap();
    db.remember(mem(60_000)).unwrap();
    fp::set_syncs_until_fail(0);
    assert!(db.flush().is_err());
    fp::set_syncs_until_fail(-1);
    fp::set_io_trace(true);
    db.flush().unwrap();
    let trace = fp::io_trace();
    fp::set_io_trace(false);
    assert_one_barrier(&trace);
    db.close().unwrap();
    assert_eq!(state(&p), (3001, 3001));
}

/// A big commit that fails at a write before the barrier (its data pages,
/// or a direct run), retried on the same handle, commits once and reopens.
#[test]
fn a_retry_after_a_failed_write_before_the_barrier_commits_cleanly() {
    let dir = tempdir().unwrap();
    let b = base(dir.path(), 3000);
    // The commit's writes before the barrier, counted on a copy.
    let before_barrier = {
        let p = dir.path().join("count.mnemo");
        std::fs::copy(&b, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        stage(&mut db, 1500).unwrap();
        fp::set_io_trace(true);
        db.flush().unwrap();
        let trace = fp::io_trace();
        fp::set_io_trace(false);
        db.close().unwrap();
        trace.iter().take_while(|e| **e != IoEvent::Sync).count() as u64
    };
    assert!(before_barrier > 2, "{before_barrier}");
    // Every one: the data pages' writes come first, then the direct runs',
    // which fail after step 3 has changed the header.
    for k in 0..before_barrier {
        let p = dir.path().join(format!("r{k}.mnemo"));
        std::fs::copy(&b, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        stage(&mut db, 1500).unwrap();
        fp::set_writes_failing(&[k]);
        assert!(db.flush().is_err(), "write {k} failed, so the flush did");
        fp::set_writes_failing(&[]);
        db.flush().unwrap();
        db.close().unwrap();
        assert_eq!(state(&p), (4500, 4500), "write {k}");
    }
}
