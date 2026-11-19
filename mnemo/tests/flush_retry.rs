//! v0.20.0 (v0200-1r): a flush that fails, retried on the same handle,
//! either commits soundly or returns an error, and the file always reopens
//! with the state before the commit or after it.
//!
//! Before the commit point (the WAL commit write) a failure stores nothing,
//! and `flush` puts back the header and free list it changed, so the retry
//! commits. At or after it, the commit may be in the file and the handle
//! can't tell, so the retry returns `MaybeStored` and the open decides: it
//! replays a committed WAL or drops a torn one. Before v0.20.0 the retry
//! returned `Ok` over a header that linked to the failed attempt's runs,
//! and the reopen failed with `PageAuthFailed`.
//!
//! Needs the `failpoints` feature (CI's `cargo test --features failpoints`).
#![cfg(feature = "failpoints")]

use mnemo_engine::__failpoints::{self as fp, IoEvent};
use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError};
use std::path::{Path, PathBuf};
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
fn base(dir: &Path, n: usize) -> PathBuf {
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

/// The commit under test's writes, before its flush. The first one takes
/// the session's generation lease, a commit of its own, so the failpoints
/// are armed after this.
fn stage(db: &mut Mnemo, n: usize) {
    for i in 0..n {
        db.remember(mem(100_000 + i)).unwrap();
    }
}

/// One clean run of the commit, on a copy: its I/O trace.
fn trace_of(dir: &Path, b: &Path, n: usize) -> Vec<IoEvent> {
    let p = dir.join("trace.mnemo");
    std::fs::copy(b, &p).unwrap();
    let mut db = Mnemo::open(&p, "pw").unwrap();
    stage(&mut db, n);
    fp::set_io_trace(true);
    db.flush().unwrap();
    let trace = fp::io_trace();
    fp::set_io_trace(false);
    db.close().unwrap();
    std::fs::remove_file(&p).unwrap();
    trace
}

enum Fail {
    Write(u64),
    Sync(i64),
}

/// Fail one write or sync of the commit, retry on the same handle, close,
/// and return the retry's result and the reopened state.
fn fail_and_retry(
    dir: &Path,
    b: &Path,
    n: usize,
    fail: &Fail,
    tag: &str,
) -> (bool, (usize, usize)) {
    let p = dir.join(format!("{tag}.mnemo"));
    std::fs::copy(b, &p).unwrap();
    let mut db = Mnemo::open(&p, "pw").unwrap();
    stage(&mut db, n);
    match fail {
        Fail::Write(k) => fp::set_writes_failing(&[*k]),
        Fail::Sync(k) => fp::set_syncs_until_fail(*k),
    }
    let first = db.flush();
    fp::set_writes_failing(&[]);
    fp::set_syncs_until_fail(-1);
    assert!(
        first.is_err(),
        "{tag}: the failpoint fired, so the flush failed"
    );
    let retry = db.flush();
    let retried = match &retry {
        Ok(()) => {
            db.close().unwrap();
            true
        }
        Err(MnemoError::MaybeStored(_)) => {
            // Refused for good on this handle: a second try and the close
            // say the same.
            assert!(
                matches!(db.flush(), Err(MnemoError::MaybeStored(_))),
                "{tag}"
            );
            assert!(
                matches!(db.close(), Err(MnemoError::MaybeStored(_))),
                "{tag}"
            );
            false
        }
        Err(e) => panic!("{tag}: the retry failed with {e:?}"),
    };
    let got = state(&p);
    std::fs::remove_file(&p).unwrap();
    (retried, got)
}

/// Every write and every sync of one commit of `n` memories over a base of
/// `base_n`: a failure before the commit point is retried into the post
/// state; one at or after it is refused, and the open finds pre (the commit
/// write itself failed) or post (the WAL holds the commit).
fn every_failure_then_retry(base_n: usize, n: usize) {
    let dir = tempdir().unwrap();
    let b = base(dir.path(), base_n);
    let pre = state(&b);
    let post = (pre.0 + n, pre.1 + n);
    let trace = trace_of(dir.path(), &b, n);
    let writes: Vec<&IoEvent> = trace.iter().filter(|e| **e != IoEvent::Sync).collect();
    let commit = writes
        .iter()
        .position(|e| **e == IoEvent::WalCommit)
        .unwrap() as u64;
    let syncs = trace.iter().filter(|e| **e == IoEvent::Sync).count() as i64;
    assert_eq!(syncs, 3, "{trace:?}");
    for k in 0..writes.len() as u64 {
        let (retried, got) = fail_and_retry(dir.path(), &b, n, &Fail::Write(k), &format!("w{k}"));
        if k < commit {
            assert!(retried, "write {k}, before the commit point, is retried");
            assert_eq!(got, post, "write {k}");
        } else {
            assert!(
                !retried,
                "write {k}, at or after the commit point, is refused"
            );
            let want = if k == commit { pre } else { post };
            assert_eq!(got, want, "write {k} (the commit write is {commit})");
        }
    }
    // Sync 0 is the barrier, before the commit point; 1 is the commit's, 2
    // the checkpoint's. The commit frame was written before either fails,
    // so the open replays it.
    for k in 0..syncs {
        let (retried, got) = fail_and_retry(dir.path(), &b, n, &Fail::Sync(k), &format!("s{k}"));
        assert_eq!(retried, k == 0, "sync {k}");
        assert_eq!(got, post, "sync {k}");
    }
}

#[test]
fn a_small_commit_retried_after_any_failure_is_sound() {
    every_failure_then_retry(200, 1);
}

/// A batch with a big catalog run, written outside the WAL (FMT-9c).
#[test]
fn a_big_commit_retried_after_any_failure_is_sound() {
    every_failure_then_retry(3000, 1500);
}

/// The refusal is per handle: reopening the file clears it, and the next
/// commit works.
#[test]
fn a_reopened_handle_commits_again() {
    let dir = tempdir().unwrap();
    let b = base(dir.path(), 200);
    let trace = trace_of(dir.path(), &b, 1);
    let commit = trace
        .iter()
        .filter(|e| **e != IoEvent::Sync)
        .position(|e| *e == IoEvent::WalCommit)
        .unwrap() as u64;
    let mut db = Mnemo::open(&b, "pw").unwrap();
    stage(&mut db, 1);
    fp::set_writes_failing(&[commit]);
    assert!(db.flush().is_err());
    fp::set_writes_failing(&[]);
    assert!(matches!(db.flush(), Err(MnemoError::MaybeStored(_))));
    drop(db);
    let mut db = Mnemo::open(&b, "pw").unwrap();
    assert_eq!(
        db.len(),
        200,
        "the commit write failed, so nothing was stored"
    );
    db.remember(mem(50_000)).unwrap();
    db.close().unwrap();
    assert_eq!(state(&b), (201, 201));
}
