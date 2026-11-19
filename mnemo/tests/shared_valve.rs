//! LOCK-2, the writer safety valve (12.4 design note, "LOCK-2"), through the
//! lock layer's test hooks: a writer that has waited past the threshold
//! takes `writer.lock`, and readers back off while it's held. The writer
//! killed while holding the valve is in `tests/cross_process.rs`.
#![cfg(feature = "failpoints")]

use std::io::Write;
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
/// no reader waits much longer than the design lets it (v0200-t1).
///
/// A read can wait for two reasons. Behind a write window that holds the
/// lock: that's the commit, not starvation, so it doesn't count. And while
/// writers hand the lock on to each other through the intent lock or the
/// valve: a reader defers for up to `VALVE_AFTER` of its own wait, then
/// hurries and queues (LOCK-2 round 3). So each read's *idle wait*, its
/// wait less the time write windows held the lock during it, must stay
/// under `VALVE_AFTER` plus 500 ms of scheduler slack. A fixed 500 ms
/// sample with no reads isn't a failure: the design allows that much.
///
/// That hurry caps any one read's idle wait, so readers starved by a
/// regression show in throughput instead: over the run, readers must get
/// in at least as often as writers commit. Healthy runs have about 30
/// times as many reads as write windows. With the pre-step-5 behaviour
/// back (a reader drops the intent lock while the main lock is busy),
/// there were 0.15 to 0.21 reads per window, with every idle wait under
/// the bound (v0200-t1).
#[test]
fn two_writers_and_four_readers_all_get_their_turns() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let stop = Arc::new(AtomicBool::new(false));
    let t0 = Instant::now();
    // Each read's (started waiting, got the lock), from t0.
    let rs: Vec<_> = (0..4)
        .map(|_| {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut r = Locks::new(&p, &opts(10_000)).unwrap();
                let mut reads = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let asked = t0.elapsed();
                    let held = r.shared(&opts(10_000)).unwrap();
                    reads.push((asked, t0.elapsed()));
                    std::thread::sleep(Duration::from_millis(1));
                    drop(held);
                }
                reads
            })
        })
        .collect();
    // Each write window's (got the lock, let it go), from t0.
    let ws: Vec<_> = (0..2)
        .map(|_| {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut w = Locks::new(&p, &opts(10_000)).unwrap();
                let mut windows = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let held = w.exclusive(&opts(10_000)).unwrap();
                    let got = t0.elapsed();
                    std::thread::sleep(Duration::from_millis(5));
                    windows.push((got, t0.elapsed()));
                    drop(held);
                }
                windows
            })
        })
        .collect();
    std::thread::sleep(Duration::from_secs(3));
    stop.store(true, Ordering::Relaxed);
    let windows: Vec<Vec<(Duration, Duration)>> =
        ws.into_iter().map(|w| w.join().unwrap()).collect();
    let reads: Vec<(Duration, Duration)> = rs.into_iter().flat_map(|r| r.join().unwrap()).collect();
    let all: Vec<(Duration, Duration)> = windows.iter().flatten().copied().collect();

    // Per read: its wait, and the part of it no write window held the lock.
    let (mut longest, mut idlest) = (Duration::ZERO, (Duration::ZERO, Duration::ZERO));
    for &(asked, got) in &reads {
        let waited = got - asked;
        let held: Duration = all
            .iter()
            .filter(|&&(a, b)| a < got && b > asked)
            .map(|&(a, b)| b.min(got) - a.max(asked))
            .sum();
        let idle = waited.saturating_sub(held);
        longest = longest.max(waited);
        if idle > idlest.0 {
            idlest = (idle, waited);
        }
    }
    let longest_window = all.iter().map(|&(a, b)| b - a).max().unwrap_or_default();
    eprintln!(
        "commits {:?}; {} reads; longest read wait {longest:?}; longest idle wait {:?} \
         (of a {:?} wait); longest write window {longest_window:?}",
        windows.iter().map(Vec::len).collect::<Vec<_>>(),
        reads.len(),
        idlest.0,
        idlest.1,
    );
    assert!(
        windows.iter().all(|w| !w.is_empty()),
        "a writer never committed"
    );
    assert!(
        reads.len() >= all.len(),
        "readers got in {} times while writers committed {} times",
        reads.len(),
        all.len()
    );
    let bound = __shared::VALVE_AFTER + Duration::from_millis(500);
    assert!(
        idlest.0 < bound,
        "a read waited {:?} while no writer held the lock (bound {bound:?})",
        idlest.0
    );
    assert!(
        longest < Duration::from_secs(5),
        "a read waited {longest:?}"
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
    let (rs, reads) = readers(&p, 16, Duration::ZERO, &stop);
    // The readers are running (so the sidecar exists), and a shared lock
    // is held as the plain operation starts, so it has to wait: on a loaded
    // machine the readers alone could leave it a gap, and an operation
    // that gets in at once never looks for the sidecar.
    while reads.load(Ordering::Relaxed) == 0 {
        std::thread::sleep(Duration::from_millis(5));
    }
    let holder = hold_shared(&p, Duration::from_millis(300));
    let t = Instant::now();
    let held = plain.exclusive(&opts(10_000)).map(drop);
    let waited = t.elapsed();
    holder.join().unwrap();
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
    // Written to stderr directly, past the test harness's capture, so the
    // counts show in CI's log on a pass too.
    let _ = writeln!(
        std::io::stderr(),
        "8 writers: valve taken {takes} times; {n} reads; longest read wait {longest_read:?}"
    );
    // What LOCK-2 guarantees here is that reads get through and none waits
    // without bound, not a rate: with eight writers saving back to back on
    // a slow machine a read can wait up to about 2 s (12.4 design note,
    // "LOCK-2", the measurements). 5 s still catches starvation.
    assert!(n >= 3, "readers starved: {n} reads in 4 s");
    assert!(
        longest_read < Duration::from_secs(5),
        "a read waited {longest_read:?}"
    );
}
