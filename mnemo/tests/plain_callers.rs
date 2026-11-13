//! v0.14.0 step 4a (RB-007.4, RB-007.8 item 8): the engine's long plain
//! operations (compaction, and so `rekey --rotate`; `Mnemo::open_exclusive`
//! for the CLI's `import` and `rekey`) take the lock through 12.4's lock
//! layer: they wait for shared callers with writer preference, and leave a
//! `lock-info` note while they hold the file. Compaction also removes the
//! temp files dead compactions left. Failpoints builds only (the test
//! hooks).
#![cfg(feature = "failpoints")]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mnemo_engine::__shared;
use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, SharedOptions};
use tempfile::tempdir;

const PW: &str = "plain-pw";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 2,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(text: &str) -> Memory {
    Memory::new(text, MemoryType::Semantic, vec![1.0, 0.0])
}

fn brain(dir: &Path, n: usize) -> PathBuf {
    let p = dir.join("b.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(&format!("seed {i}"))).unwrap();
    }
    db.close().unwrap();
    p
}

fn opts(deadline_ms: u64) -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_millis(deadline_ms);
    o
}

fn refused() -> bool {
    if __shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode");
    }
    __shared::WINDOWS_OLD_RENAME
}

fn count(p: &Path) -> usize {
    Mnemo::open_read_only(p, PW).unwrap().len()
}

/// A shared handle in another thread holds a read window for `hold`.
/// Returns once the window is open.
fn hold_a_read(p: &Path, hold: Duration) -> std::thread::JoinHandle<()> {
    let p = p.to_path_buf();
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let mut s = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
        s.read(|_| {
            tx.send(()).unwrap();
            std::thread::sleep(hold);
            Ok(())
        })
        .unwrap();
    });
    rx.recv().unwrap();
    t
}

#[test]
fn compaction_waits_for_a_shared_read_instead_of_failing() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 3);
    let reader = hold_a_read(&p, Duration::from_millis(400));
    // A plain open still fails at once, as the note says.
    assert!(matches!(
        Mnemo::open(&p, PW).err(),
        Some(MnemoError::Locked { .. })
    ));
    let t = Instant::now();
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    assert!(t.elapsed() >= Duration::from_millis(200), "it waited");
    reader.join().unwrap();
    assert_eq!(count(&p), 3);
}

#[test]
fn an_exclusive_open_names_itself_to_shared_callers_and_cleans_up() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let mut s = Mnemo::open_shared(&p, PW, opts(200)).unwrap();
    let lock_info =
        __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap()).join(__shared::LOCK_INFO);

    let mut db = Mnemo::open_exclusive(&p, PW, "import", SharedOptions::default()).unwrap();
    assert!(lock_info.is_file());
    let e = s.write(|db| db.remember(mem("waits"))).unwrap_err();
    match &e {
        MnemoError::Locked { holder, .. } => {
            let h = holder.as_deref().unwrap_or("");
            assert!(h.starts_with("import since "), "{h}");
            assert!(h.contains(&format!("pid {}", std::process::id())), "{h}");
        }
        other => panic!("{other}"),
    }
    db.remember(mem("imported")).unwrap();
    db.close().unwrap();
    assert!(!lock_info.exists(), "the note goes with the lock");
    s.write(|db| db.remember(mem("after"))).unwrap();
    assert_eq!(count(&p), 3);
}

/// Four shared readers in tight loops: the exclusive open and a compaction
/// still get in (writer preference), where a plain open's single try
/// usually doesn't.
#[test]
fn busy_shared_readers_dont_keep_long_operations_out() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 50);
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut s = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
                while !stop.load(Ordering::Relaxed) {
                    match s.read(|db| db.memories()) {
                        Ok(_) | Err(MnemoError::Locked { .. }) => {}
                        Err(e) => panic!("{e}"),
                    }
                }
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(50));
    for _ in 0..3 {
        let mut db = Mnemo::open_exclusive(&p, PW, "import", opts(5000)).unwrap();
        db.remember(mem("from the exclusive open")).unwrap();
        db.close().unwrap();
        Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().unwrap();
    }
    assert_eq!(count(&p), 53);
}

/// The CLI's `import`, `rekey` and `compact` (and so `rekey --rotate`) wait
/// for a shared read instead of failing.
#[test]
fn the_clis_long_commands_wait_for_a_shared_read() {
    if refused() {
        return;
    }
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let jsonl = dir.path().join("in.jsonl");
    std::fs::write(
        &jsonl,
        "{\"content\":\"from the import\",\"memory_type\":\"semantic\",\"vector\":[0.5,0.5]}\n",
    )
    .unwrap();
    let mnemo = |args: &[&str], secret: &str| {
        // Long enough that the command starts while the read holds the
        // file, so it has to wait.
        let reader = hold_a_read_with(&p, secret, Duration::from_millis(1500));
        let t = Instant::now();
        let out = Command::new(env!("CARGO_BIN_EXE_mnemo"))
            .args(args)
            .env("MNEMO_PASSPHRASE", secret)
            .env("MNEMO_NEW_PASSPHRASE", "new-pw")
            .output()
            .unwrap();
        let took = t.elapsed();
        reader.join().unwrap();
        assert!(
            out.status.success(),
            "mnemo {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            took >= Duration::from_millis(1000),
            "mnemo {args:?} took {took:?}"
        );
    };
    let path = p.to_str().unwrap();
    mnemo(&["import", path, jsonl.to_str().unwrap()], PW);
    mnemo(&["compact", path], PW);
    mnemo(&["rekey", path, "--rotate"], PW);
    let mut db = Mnemo::open(&p, "new-pw").unwrap();
    assert_eq!(db.len(), 2);
    assert!(db
        .memories()
        .unwrap()
        .iter()
        .any(|m| m.content == "from the import"));
}

fn hold_a_read_with(p: &Path, secret: &str, hold: Duration) -> std::thread::JoinHandle<()> {
    let (p, secret) = (p.to_path_buf(), secret.to_string());
    let (tx, rx) = std::sync::mpsc::channel();
    let t = std::thread::spawn(move || {
        let mut s = Mnemo::open_shared(&p, &secret, SharedOptions::default()).unwrap();
        s.read(|_| {
            tx.send(()).unwrap();
            std::thread::sleep(hold);
            Ok(())
        })
        .unwrap();
    });
    rx.recv().unwrap();
    t
}

/// RB-007.8 item 8: a compaction removes the temp files dead compactions of
/// the same brain left, and nothing else.
#[cfg(unix)]
#[test]
fn compaction_removes_only_dead_compactors_temp_files() {
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 2);
    // A pid that has exited, and one that's running.
    let mut dead = Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();
    let mut live = Command::new("sleep").arg("30").spawn().unwrap();
    let live_pid = live.id();
    let me = std::process::id();

    let gone = d.join(format!("b.mnemo.compact-{dead_pid}-0-0000abcd.tmp"));
    let kept_names = [
        format!("b.mnemo.compact-{live_pid}-0-0000abcd.tmp"),
        format!("b.mnemo.compact-{me}-0-0000abcd.tmp"),
        format!("b.mnemo.compact-{dead_pid}-0-abcd.tmp"),
        format!("b.mnemo.compact-{dead_pid}-x-0000abcd.tmp"),
        format!("b.mnemo.compact-{dead_pid}-0-0000abcd.tmp.keep"),
        format!("b.mnemo.export-{dead_pid}-0-0000abcd.tmp"),
        format!("other.mnemo.compact-{dead_pid}-0-0000abcd.tmp"),
    ];
    std::fs::write(&gone, b"a dead compactor's copy").unwrap();
    for n in &kept_names {
        std::fs::write(d.join(n), b"keep").unwrap();
    }
    // A directory and a symlink at matching names are left alone too.
    let as_dir = d.join(format!("b.mnemo.compact-{dead_pid}-1-0000abcd.tmp"));
    std::fs::create_dir(&as_dir).unwrap();
    let target = d.join("target.txt");
    std::fs::write(&target, b"target").unwrap();
    let as_link = d.join(format!("b.mnemo.compact-{dead_pid}-2-0000abcd.tmp"));
    std::os::unix::fs::symlink(&target, &as_link).unwrap();

    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();

    assert!(!gone.exists(), "the dead compactor's temp file is removed");
    for n in &kept_names {
        assert!(d.join(n).exists(), "{n} is kept");
    }
    assert!(as_dir.is_dir());
    assert!(std::fs::symlink_metadata(&as_link).is_ok());
    assert_eq!(std::fs::read(&target).unwrap(), b"target");
    assert_eq!(count(&p), 2);
    live.kill().unwrap();
    live.wait().unwrap();
}

/// The cleanup also needs the file to be this user's (Unix): in a shared
/// directory, another user's file that happens to match the name stays.
#[cfg(unix)]
#[test]
fn compaction_keeps_a_matching_temp_file_another_user_owns() {
    use std::os::unix::fs::MetadataExt;
    let dir = tempdir().unwrap();
    let d = dir.path();
    let p = brain(d, 1);
    let me = std::fs::metadata(&p).unwrap().uid();
    let mut dead = Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();
    let left = d.join(format!("b.mnemo.compact-{dead_pid}-0-0000abcd.tmp"));
    std::fs::write(&left, b"a dead compactor's copy").unwrap();

    // Seen as another user's (the test override of the effective uid).
    mnemo_engine::__failpoints::set_euid_override(Some(me.wrapping_add(1)));
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    mnemo_engine::__failpoints::set_euid_override(None);
    assert!(left.exists(), "another user's file is kept");
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    assert!(!left.exists(), "this user's is removed");

    // Really another user's, where the test can make one (root only).
    if me != 0 {
        eprintln!("skipped the chown case: not running as root");
        return;
    }
    let theirs = d.join(format!("b.mnemo.compact-{dead_pid}-1-0000abcd.tmp"));
    std::fs::write(&theirs, b"someone else's").unwrap();
    std::os::unix::fs::chown(&theirs, Some(65534), Some(65534)).unwrap();
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    assert!(theirs.exists(), "a file owned by uid 65534 is kept");
}
