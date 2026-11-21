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
#[cfg(unix)]
use mnemo_engine::MnemoError;
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
/// after its barrier failed syncs before its commit. And the failed
/// attempt's header and free list are put back, so the retry commits over
/// the committed state, not over the runs the failed attempt staged.
///
/// RB-016.1: the retry writes the data pages the failed attempt wrote again,
/// since the OS may have dropped them. RB-016.2: it takes a new generation
/// lease first (one header-only commit), so its runs aren't sealed under the
/// failed attempt's generation.
#[test]
fn a_retry_after_a_failed_barrier_syncs_before_its_commit() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 3000);
    let mut db = Mnemo::open(&p, "pw").unwrap();
    db.remember(mem(60_000)).unwrap();
    fp::set_syncs_until_fail(0);
    fp::set_io_trace(true);
    assert!(db.flush().is_err());
    let first = pages_written(&fp::io_trace());
    fp::set_syncs_until_fail(-1);
    fp::set_io_trace(true);
    db.flush().unwrap();
    let trace = fp::io_trace();
    fp::set_io_trace(false);
    let commits: Vec<usize> = (0..trace.len())
        .filter(|&i| trace[i] == IoEvent::WalCommit)
        .collect();
    assert_eq!(commits.len(), 2, "the lease, then the commit: {trace:?}");
    // The lease: its WAL commit, the header folded in, then the commit
    // under test.
    assert_eq!(
        trace[commits[0]..commits[0] + 4],
        [
            IoEvent::WalCommit,
            IoEvent::Sync,
            IoEvent::Page(0),
            IoEvent::Sync
        ],
        "{trace:?}"
    );
    let retry = &trace[commits[0] + 4..];
    assert_one_barrier(retry);
    let again = pages_written(retry);
    for page in &first {
        assert!(
            again.contains(page),
            "page {page} not written again: {trace:?}"
        );
    }
    db.close().unwrap();
    assert_eq!(state(&p), (3001, 3001));
}

/// The pages a trace wrote, page 0 (the header) aside.
fn pages_written(trace: &[IoEvent]) -> Vec<u64> {
    let mut v: Vec<u64> = trace
        .iter()
        .filter_map(|e| match e {
            IoEvent::Page(n) if *n != 0 => Some(*n),
            _ => None,
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Put `pages` of `p` back to their bytes in `earlier` (zeros past its
/// end): a stand-in for a kernel that dropped those writes after a failed
/// fsync (RB-016.1).
///
/// It writes under the open handle's lock, which Windows enforces (a
/// byte-range lock there refuses other handles' reads and writes), so it
/// and the two tests that use it run on Unix only.
#[cfg(unix)]
fn put_back(p: &Path, earlier: &[u8], pages: &[u64]) {
    use std::io::{Seek, SeekFrom, Write};
    const PAGE: usize = mnemo_engine::__wal::PAGE_SIZE;
    let mut f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
    for &n in pages {
        let at = n as usize * PAGE;
        let mut img = vec![0u8; PAGE];
        if at < earlier.len() {
            let end = (at + PAGE).min(earlier.len());
            img[..end - at].copy_from_slice(&earlier[at..end]);
        }
        f.seek(SeekFrom::Start(at as u64)).unwrap();
        f.write_all(&img).unwrap();
    }
    f.sync_all().unwrap();
}

/// RB-016.1: after a failed barrier the OS may have dropped the pages the
/// attempt wrote, and a later fsync can still succeed. The retry writes
/// them again, so the file it commits reads back whole, earlier memories
/// included (a commit repacks its tail record page). At the finding's
/// three sizes. Unix only: see `put_back`.
#[cfg(unix)]
#[test]
fn a_retry_after_a_failed_barrier_writes_dropped_pages_again() {
    for (n, k) in [(200, 1), (3000, 1), (3000, 1500)] {
        let dir = tempdir().unwrap();
        let p = base(dir.path(), n);
        let mut db = Mnemo::open(&p, "pw").unwrap();
        stage(&mut db, k).unwrap();
        let earlier = std::fs::read(&p).unwrap();
        fp::set_syncs_until_fail(0);
        fp::set_io_trace(true);
        assert!(db.flush().is_err(), "{n} + {k}: the barrier failed");
        fp::set_syncs_until_fail(-1);
        let wrote = pages_written(&fp::io_trace());
        fp::set_io_trace(false);
        assert!(!wrote.is_empty(), "{n} + {k}");
        put_back(&p, &earlier, &wrote);
        db.flush().unwrap();
        db.close().unwrap();

        let mut db = Mnemo::open(&p, "pw").unwrap();
        assert_eq!(db.len(), n + k, "{n} + {k}");
        assert_eq!(db.verify().unwrap(), n + k, "{n} + {k}");
        let all = db.memories().unwrap();
        assert_eq!(all.len(), n + k, "{n} + {k}");
        for m in &all {
            assert_eq!(db.get(&m.id).unwrap().content, m.content, "{n} + {k}");
        }
        let mut want: Vec<String> = (0..n)
            .chain(100_000..100_000 + k)
            .map(|i| format!("memory {i}"))
            .collect();
        let mut got: Vec<String> = all.into_iter().map(|m| m.content).collect();
        want.sort();
        got.sort();
        assert_eq!(got, want, "{n} + {k}");
    }
}

/// RB-016.2: a retry after a failed barrier seals its runs under a new
/// generation. Change something between the attempt and the retry, then
/// put the failed attempt's images back: they either fail to authenticate
/// or aren't referenced, so the change holds. Unix only: see `put_back`.
#[cfg(unix)]
#[test]
fn a_retry_after_a_failed_barrier_seals_under_a_new_generation() {
    let dir = tempdir().unwrap();
    let p = base(dir.path(), 3000);
    let mut db = Mnemo::open(&p, "pw").unwrap();
    let ids: Vec<_> = (0..1500)
        .map(|i| db.remember(mem(100_000 + i)).unwrap())
        .collect();
    fp::set_syncs_until_fail(0);
    fp::set_io_trace(true);
    assert!(db.flush().is_err());
    fp::set_syncs_until_fail(-1);
    let first = pages_written(&fp::io_trace());
    fp::set_io_trace(false);
    let copy = std::fs::read(&p).unwrap();
    db.delete(&ids[7]).unwrap();
    db.flush().unwrap();
    db.close().unwrap();
    // 4,499 records and the delete's tombstone.
    assert_eq!(state(&p), (4500, 4500));

    // Every page the failed attempt wrote, back to its image.
    assert!(first.len() > 2, "{first:?}");
    put_back(&p, &copy, &first);
    match Mnemo::open(&p, "pw") {
        Err(MnemoError::PageAuthFailed(_)) => {}
        Err(e) => panic!("{e}"),
        Ok(mut db) => {
            assert_eq!(db.len(), 4500, "the delete was undone");
            match db.get(&ids[7]) {
                Err(MnemoError::NotFound(_)) | Err(MnemoError::PageAuthFailed(_)) => {}
                other => panic!("the deleted memory came back: {other:?}"),
            }
            match db.verify() {
                Ok(n) => assert_eq!(n, 4500),
                Err(MnemoError::PageAuthFailed(_)) => {}
                Err(e) => panic!("{e}"),
            }
        }
    }
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

/// RB-018.1: a failed direct-run write, then a retry whose new lease fails
/// at its WAL fsync. That fsync isn't `Pager::sync`, so the pager isn't
/// told; the restore after the first failure is what put attempt 1's data
/// pages back on the list. Model the kernel dropping every write since the
/// last sync (none of them was synced) by putting the whole file back,
/// then a third flush. Unix only: see `put_back`.
#[cfg(unix)]
#[test]
fn a_failed_run_write_then_a_failed_lease_fsync_writes_the_pages_again() {
    let dir = tempdir().unwrap();
    let b = base(dir.path(), 3000);
    // The last write before the barrier is a direct run's, after
    // `write_dirty` has written every data page.
    let last_before_barrier = {
        let p = dir.path().join("count.mnemo");
        std::fs::copy(&b, &p).unwrap();
        let mut db = Mnemo::open(&p, "pw").unwrap();
        stage(&mut db, 1500).unwrap();
        fp::set_io_trace(true);
        db.flush().unwrap();
        let trace = fp::io_trace();
        fp::set_io_trace(false);
        db.close().unwrap();
        let before = &trace[..trace.iter().position(|e| *e == IoEvent::Sync).unwrap()];
        assert!(
            before
                .iter()
                .filter(|e| matches!(e, IoEvent::Page(_)))
                .count()
                > 2,
            "{before:?}"
        );
        before.len() as u64 - 1
    };

    let p = dir.path().join("r.mnemo");
    std::fs::copy(&b, &p).unwrap();
    let mut db = Mnemo::open(&p, "pw").unwrap();
    stage(&mut db, 1500).unwrap();
    let earlier = std::fs::read(&p).unwrap();
    fp::set_writes_failing(&[last_before_barrier]);
    assert!(db.flush().is_err(), "the direct run's write failed");
    fp::set_writes_failing(&[]);
    fp::set_syncs_until_fail(0);
    assert!(db.flush().is_err(), "the new lease's fsync failed");
    fp::set_syncs_until_fail(-1);
    // Nothing was synced since `earlier`: drop all of it.
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        f.set_len(earlier.len() as u64).unwrap();
        f.write_all(&earlier).unwrap();
        f.sync_all().unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();

    let mut db = Mnemo::open(&p, "pw").unwrap();
    assert_eq!(db.len(), 4500);
    assert_eq!(db.verify().unwrap(), 4500);
    let all = db.memories().unwrap();
    for m in &all {
        assert_eq!(db.get(&m.id).unwrap().content, m.content);
    }
    let mut want: Vec<String> = (0..3000)
        .chain(100_000..101_500)
        .map(|i| format!("memory {i}"))
        .collect();
    let mut got: Vec<String> = all.into_iter().map(|m| m.content).collect();
    want.sort();
    got.sort();
    assert_eq!(got, want);
}
