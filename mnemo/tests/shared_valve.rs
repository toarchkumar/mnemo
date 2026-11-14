//! LOCK-2, the writer safety valve (12.4 design note, "LOCK-2"), through the
//! lock layer's test hooks: a writer that has waited past the threshold
//! takes `writer.lock`, and readers back off while it's held. The writer
//! killed while holding the valve is in `tests/cross_process.rs`.
#![cfg(feature = "failpoints")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mnemo_engine::__shared::{self, Locks};
use mnemo_engine::{MnemoError, SharedOptions};
use tempfile::tempdir;

fn opts(deadline_ms: u64) -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_millis(deadline_ms);
    o
}

fn brain(dir: &Path) -> PathBuf {
    let p = dir.join("b.mnemo");
    std::fs::write(&p, b"brain").unwrap();
    p
}

fn refused() -> bool {
    if __shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode (Windows, Rust < 1.85)");
    }
    __shared::WINDOWS_OLD_RENAME
}

fn valve_path(p: &Path) -> PathBuf {
    __shared::sidecar_path(&std::fs::canonicalize(p).unwrap()).join(__shared::VALVE)
}

/// `n` reader threads, each looping on the shared lock (holding it `hold`),
/// until `stop`. Returns their handles and a shared read count.
fn readers(
    p: &Path,
    n: usize,
    hold: Duration,
    stop: &Arc<AtomicBool>,
) -> (Vec<std::thread::JoinHandle<Duration>>, Arc<AtomicU64>) {
    let count = Arc::new(AtomicU64::new(0));
    let handles = (0..n)
        .map(|_| {
            let (p, stop, count) = (p.to_path_buf(), stop.clone(), count.clone());
            std::thread::spawn(move || {
                let mut r = Locks::new(&p, &opts(10_000)).unwrap();
                let mut longest = Duration::ZERO;
                while !stop.load(Ordering::Relaxed) {
                    let t = Instant::now();
                    let held = r.shared(&opts(10_000)).unwrap();
                    longest = longest.max(t.elapsed());
                    if !hold.is_zero() {
                        std::thread::sleep(hold);
                    }
                    drop(held);
                    count.fetch_add(1, Ordering::Relaxed);
                }
                longest
            })
        })
        .collect();
    (handles, count)
}

/// Test 2: sixteen readers on cheap reads (no pause between them): a writer's
/// 20 writes all commit within the 10 s deadline.
#[test]
fn sixteen_cheap_readers_cant_lock_out_a_writer() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let mut w = Locks::new(&p, &opts(10_000)).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let (rs, reads) = readers(&p, 16, Duration::ZERO, &stop);
    std::thread::sleep(Duration::from_millis(100));
    let mut worst = Duration::ZERO;
    for _ in 0..20 {
        let t = Instant::now();
        let held = match w.exclusive(&opts(10_000)) {
            Ok(h) => h,
            Err(e) => panic!("a write was locked out by cheap readers: {e}"),
        };
        worst = worst.max(t.elapsed());
        std::thread::sleep(Duration::from_millis(1));
        drop(held);
    }
    stop.store(true, Ordering::Relaxed);
    for r in rs {
        r.join().unwrap();
    }
    eprintln!(
        "20 writes against 16 cheap readers: longest wait {worst:?}, valve taken {} times, {} reads",
        w.valve_takes(),
        reads.load(Ordering::Relaxed)
    );
    assert!(worst < Duration::from_secs(5), "a write waited {worst:?}");
}

/// Test 3: with nobody in the way, a writer never takes the valve.
#[test]
fn a_writer_that_gets_in_before_the_threshold_never_takes_the_valve() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let mut w = Locks::new(&p, &opts(10_000)).unwrap();
    for _ in 0..50 {
        drop(w.exclusive(&opts(10_000)).unwrap());
    }
    assert_eq!(w.valve_takes(), 0);
    // A v0.15 sidecar makes the valve file up front.
    assert!(valve_path(&p).exists());
}

/// While a writer holds the valve, a new reader backs off (here: waits out a
/// short deadline); once it's free, the reader gets in at once.
#[test]
fn a_reader_backs_off_while_the_valve_is_held() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let mut r = Locks::new(&p, &opts(10_000)).unwrap();
    // Stand in for a writer past the threshold: hold the valve exclusively.
    let valve = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(valve_path(&p))
        .unwrap();
    fs4::fs_std::FileExt::lock_exclusive(&valve).unwrap();
    match r.shared(&opts(200)) {
        Err(MnemoError::Locked { .. }) => {}
        Ok(_) => panic!("a reader got past a held valve"),
        Err(e) => panic!("{e}"),
    }
    drop(valve); // A writer that dies: the kernel releases its lock.
    let t = Instant::now();
    drop(r.shared(&opts(10_000)).unwrap());
    assert!(t.elapsed() < Duration::from_millis(500));
}

/// Test 5: two writers and four readers looping. Both writers commit, and
/// readers keep getting in throughout.
#[test]
fn two_writers_and_four_readers_all_get_their_turns() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let stop = Arc::new(AtomicBool::new(false));
    let (rs, reads) = readers(&p, 4, Duration::from_millis(1), &stop);
    let writers: Vec<_> = (0..2)
        .map(|_| {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut w = Locks::new(&p, &opts(10_000)).unwrap();
                let mut commits = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let held = w.exclusive(&opts(10_000)).unwrap();
                    std::thread::sleep(Duration::from_millis(5));
                    drop(held);
                    commits += 1;
                }
                commits
            })
        })
        .collect();
    let mut samples = Vec::new();
    for _ in 0..6 {
        let before = reads.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(500));
        samples.push(reads.load(Ordering::Relaxed) - before);
    }
    stop.store(true, Ordering::Relaxed);
    let commits: Vec<u64> = writers.into_iter().map(|w| w.join().unwrap()).collect();
    let longest_read: Duration = rs.into_iter().map(|r| r.join().unwrap()).max().unwrap();
    eprintln!(
        "commits {commits:?}; reads per 500 ms {samples:?}; longest read wait {longest_read:?}"
    );
    assert!(commits.iter().all(|&c| c > 0), "a writer never committed");
    assert!(samples.iter().all(|&n| n > 0), "readers stopped getting in");
    assert!(
        longest_read < Duration::from_secs(5),
        "a read waited {longest_read:?}"
    );
}

/// Test 6 (RB-008.3 item 5): a plain long operation that started with no
/// sidecar picks one up while it waits, and gets in against busy readers.
#[test]
fn a_plain_operation_picks_up_the_sidecar_while_it_waits() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let mut plain = Locks::for_plain(&p, &opts(10_000)).unwrap();
    assert!(!plain.has_sidecar());
    let stop = Arc::new(AtomicBool::new(false));
    let (rs, _) = readers(&p, 16, Duration::ZERO, &stop);
    std::thread::sleep(Duration::from_millis(100));
    let t = Instant::now();
    let held = plain.exclusive(&opts(10_000)).map(drop);
    let waited = t.elapsed();
    stop.store(true, Ordering::Relaxed);
    for r in rs {
        r.join().unwrap();
    }
    assert!(
        held.is_ok(),
        "the plain operation was locked out: {:?}",
        held.err()
    );
    assert!(plain.has_sidecar(), "it didn't pick up the sidecar");
    eprintln!("the plain operation waited {waited:?}");
}

/// Hold the main lock shared for `d` in another thread, so a writer waits
/// past the threshold.
fn hold_shared(p: &Path, d: Duration) -> std::thread::JoinHandle<()> {
    let p = p.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let h = std::thread::spawn(move || {
        let mut r = Locks::new(&p, &opts(10_000)).unwrap();
        let held = r.shared(&opts(10_000)).unwrap();
        tx.send(()).unwrap();
        std::thread::sleep(d);
        drop(held);
    });
    rx.recv().unwrap();
    h
}

/// Test 8: a sidecar made before v0.15 has no `writer.lock`. Readers go on at
/// once, and the first writer past the threshold creates it.
#[test]
fn a_missing_valve_file_is_made_by_the_first_writer_that_needs_it() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let mut w = Locks::new(&p, &opts(10_000)).unwrap();
    std::fs::remove_file(valve_path(&p)).unwrap();
    let mut r = Locks::new(&p, &opts(10_000)).unwrap();
    std::fs::remove_file(valve_path(&p)).unwrap();
    drop(r.shared(&opts(10_000)).unwrap());
    assert!(!valve_path(&p).exists(), "a reader made the valve file");
    let h = hold_shared(&p, Duration::from_millis(900));
    drop(w.exclusive(&opts(10_000)).unwrap());
    h.join().unwrap();
    assert!(valve_path(&p).exists());
    assert_eq!(w.valve_takes(), 1);
}

/// Test 7: the valve can't be used (here, something else is at its name;
/// a sidecar directory this user owns can't stay unwritable, since opening
/// it gives it 0700 again): the write goes on without the valve, the call
/// isn't failed, and it's said once.
#[test]
fn no_valve_available_costs_the_valve_not_the_write() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let mut w = Locks::new(&p, &opts(10_000)).unwrap();
    let side = w.sidecar_path().unwrap();
    std::fs::remove_file(side.join(__shared::VALVE)).unwrap();
    std::fs::create_dir(side.join(__shared::VALVE)).unwrap();
    let h = hold_shared(&p, Duration::from_millis(900));
    let r = w.exclusive(&opts(10_000));
    h.join().unwrap();
    assert!(r.is_ok(), "{:?}", r.err());
    drop(r);
    let h = hold_shared(&p, Duration::from_millis(900));
    drop(w.exclusive(&opts(10_000)).unwrap());
    h.join().unwrap();
    let notices = w.notices();
    assert_eq!(
        notices
            .iter()
            .filter(|n| n.contains("writer safety valve"))
            .count(),
        1,
        "{notices:?}"
    );
    assert_eq!(w.valve_takes(), 0);
}

/// Eight writers past the threshold hand the valve on to each other; a
/// reader that has waited past the threshold stops deferring to it, so
/// readers still get in (the bench's 8-writer, 2-reader mix starved readers
/// before that rule).
#[test]
fn eight_writers_handing_on_the_valve_dont_starve_readers() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let stop = Arc::new(AtomicBool::new(false));
    let writers: Vec<_> = (0..8)
        .map(|_| {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut w = Locks::new(&p, &opts(10_000)).unwrap();
                while !stop.load(Ordering::Relaxed) {
                    let held = w.exclusive(&opts(10_000)).unwrap();
                    std::thread::sleep(Duration::from_millis(20));
                    drop(held);
                }
                w.valve_takes()
            })
        })
        .collect();
    let (rs, reads) = readers(&p, 2, Duration::ZERO, &stop);
    std::thread::sleep(Duration::from_secs(4));
    stop.store(true, Ordering::Relaxed);
    let takes: u64 = writers.into_iter().map(|w| w.join().unwrap()).sum();
    let longest_read: Duration = rs.into_iter().map(|r| r.join().unwrap()).max().unwrap();
    let n = reads.load(Ordering::Relaxed);
    eprintln!(
        "8 writers: valve taken {takes} times; {n} reads; longest read wait {longest_read:?}"
    );
    // Readers aren't starved, but a read can wait a second or two in this
    // saturating mix: the trade LOCK-2 makes (12.4 design note, "LOCK-2",
    // the measurements). 5 s still catches starvation.
    assert!(n > 20, "readers starved: {n} reads in 4 s");
    assert!(
        longest_read < Duration::from_secs(5),
        "a read waited {longest_read:?}"
    );
}
