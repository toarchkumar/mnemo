//! v0.14.0 step 1 (12.4's design note, "Gaps in today's code"): a file that
//! was replaced at a handle's path never takes that handle's writes, and a
//! write racing a compaction is never lost. Runs on every CI OS.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError};
use tempfile::tempdir;
use ulid::Ulid;

const PW: &str = "replaced-pw";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 2,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn seed(path: &Path, n: usize) {
    let mut db = Mnemo::create(path, PW, cfg()).unwrap();
    for i in 0..n {
        db.remember(Memory::new(
            format!("seed {i}"),
            MemoryType::Semantic,
            vec![1.0, 0.0],
        ))
        .unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();
}

fn count(path: &Path) -> usize {
    let mut db = Mnemo::open_read_only(path, PW).unwrap();
    let n = db.stats().unwrap().memories;
    drop(db);
    n
}

/// Retry `f` while the file is locked (a compaction is running), up to a
/// deadline, so no test can hang.
fn retry_locked<T>(mut f: impl FnMut() -> mnemo_engine::Result<T>) -> mnemo_engine::Result<T> {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        match f() {
            Err(MnemoError::Locked { .. }) if Instant::now() < deadline => std::thread::yield_now(),
            other => return other,
        }
    }
}

/// Windows built with Rust before 1.85: `std::fs::rename` refuses to
/// replace a file any handle has open ("Access is denied", os error 5).
/// A compaction then fails with that error, as in v0.13.2, and nothing is
/// lost; the caller may try again.
fn is_rename_refused(e: &MnemoError) -> bool {
    cfg!(windows)
        && matches!(e, MnemoError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied)
}

/// The race the step fixes. One thread compacts the file over and over;
/// another opens it, writes one memory, flushes and closes, as fast as it
/// can. Every write whose flush returned `Ok` must be in the final file.
///
/// Before the fix, `compact_file` released the old file's lock before
/// renaming the new one over it, and nothing checked that a freshly
/// locked file was still the one at the path, so a writer could lock the
/// file being replaced, commit to it, and lose the write.
#[test]
fn a_write_racing_a_compaction_is_never_lost() {
    let dir = tempdir().unwrap();
    let path: PathBuf = dir.path().join("race.mnemo");
    seed(&path, 20);

    let writes = 150;
    let done = Arc::new(AtomicBool::new(false));
    let acked: Arc<Mutex<Vec<Ulid>>> = Arc::default();

    let compactor = {
        let (path, done) = (path.clone(), done.clone());
        std::thread::spawn(move || {
            let p = path.to_str().unwrap().to_owned();
            let (mut compactions, mut refused) = (0u32, 0u32);
            while !done.load(Ordering::Relaxed) || compactions < 5 {
                match retry_locked(|| Mnemo::compact_file(&p, PW)) {
                    Ok(_) => compactions += 1,
                    Err(e) if is_rename_refused(&e) => refused += 1,
                    Err(e) => panic!("compaction failed: {e}"),
                }
                assert!(refused < 100_000, "compactions keep being refused");
            }
            if refused > 0 {
                eprintln!(
                    "{refused} compactions refused by the platform's rename (Windows, Rust < 1.85)"
                );
            }
            compactions
        })
    };

    let mut replaced_seen = 0u32;
    for i in 0..writes {
        let id = retry_locked(|| {
            let mut db = Mnemo::open(&path, PW)?;
            let id = db.remember(Memory::new(
                format!("racing write {i}"),
                MemoryType::Episodic,
                vec![0.0, 1.0],
            ))?;
            db.flush()?;
            db.close()?;
            Ok(id)
        });
        match id {
            Ok(id) => acked.lock().unwrap().push(id),
            // Refused before anything was written: not acknowledged.
            Err(MnemoError::Replaced { .. }) => replaced_seen += 1,
            Err(e) => panic!("write {i}: {e}"),
        }
    }
    done.store(true, Ordering::Relaxed);
    let compactions = compactor.join().unwrap();

    let acked = acked.lock().unwrap().clone();
    let mut db = Mnemo::open_read_only(&path, PW).unwrap();
    let missing: Vec<_> = acked.iter().filter(|id| db.get(id).is_err()).collect();
    assert!(
        missing.is_empty(),
        "{} of {} acknowledged writes are missing after {compactions} compactions \
         ({replaced_seen} refused as replaced): {missing:?}",
        missing.len(),
        acked.len()
    );
    assert_eq!(db.stats().unwrap().memories, 20 + acked.len());
    assert!(compactions >= 5, "the compactor ran {compactions} times");
    eprintln!(
        "{} writes kept across {compactions} compactions; {replaced_seen} refused as replaced",
        acked.len()
    );
}

/// A handle whose path now names another file refuses to write, and
/// leaves both files as they were.
#[test]
fn a_handle_refuses_to_write_to_a_file_replaced_at_its_path() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.mnemo");
    let other = dir.path().join("b.mnemo");
    seed(&path, 2);
    seed(&other, 5);

    let mut db = Mnemo::open(&path, PW).unwrap();
    db.remember(Memory::new("pending", MemoryType::Semantic, vec![1.0, 0.0]))
        .unwrap();
    // Another program moves a file over the path while the handle is open.
    // Windows with Rust before 1.85 can't do that while the file is open,
    // so the case can't arise there through `std`.
    if let Err(e) = std::fs::rename(&other, &path) {
        if is_rename_refused(&MnemoError::Io(e)) {
            eprintln!("skipped: this toolchain's rename can't replace an open file");
            return;
        }
        panic!("rename failed");
    }
    let before = std::fs::read(&path).unwrap();

    let err = db.flush().unwrap_err();
    assert!(matches!(err, MnemoError::Replaced { .. }), "{err}");
    let msg = err.to_string();
    assert!(
        msg.contains("was replaced") && msg.contains("nothing was written"),
        "{msg}"
    );
    // A rekey refuses too, before writing anything.
    assert!(matches!(
        db.rekey("new-pw", KdfParams::fast()).unwrap_err(),
        MnemoError::Replaced { .. }
    ));
    drop(db);

    assert_eq!(
        std::fs::read(&path).unwrap(),
        before,
        "the file now at the path is untouched"
    );
    assert_eq!(count(&path), 5);
}

/// What a file holds, read while a handle holds its lock: its bytes on
/// Unix, where the lock is advisory; its length on Windows, where the lock
/// is a mandatory byte-range lock and a read of the locked range fails
/// (os error 33).
fn snapshot(path: &Path) -> Vec<u8> {
    if cfg!(windows) {
        std::fs::metadata(path)
            .unwrap()
            .len()
            .to_le_bytes()
            .to_vec()
    } else {
        std::fs::read(path).unwrap()
    }
}

/// RB-008.2: a brain moved or removed under a live handle. Nothing is at
/// the path, so the message says the file was moved or removed (not
/// "replaced"), `try_close` hands the handle back, and once the file is
/// moved back the same handle commits what it held.
#[test]
fn a_moved_or_removed_brain_says_so_and_a_copy_says_replaced() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("a.mnemo");
    let moved = dir.path().join("moved.mnemo");
    seed(&path, 2);

    // Renamed away, in the same directory.
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.remember(Memory::new("pending", MemoryType::Semantic, vec![1.0, 0.0]))
        .unwrap();
    if let Err(e) = std::fs::rename(&path, &moved) {
        if is_rename_refused(&MnemoError::Io(e)) {
            eprintln!("skipped: this toolchain's rename can't move an open file");
            return;
        }
        panic!("rename failed");
    }
    let before = snapshot(&moved);
    let err = db.flush().unwrap_err();
    assert!(
        matches!(err, MnemoError::Replaced { missing: true, .. }),
        "{err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("was moved or removed")
            && msg.contains("no longer names it")
            && msg.contains("nothing was written"),
        "{msg}"
    );
    assert!(!msg.contains("was replaced"), "{msg}");
    let back = match db.try_close() {
        Ok(()) => panic!("a close on a moved brain must fail"),
        Err(e) => {
            assert!(
                matches!(e.error(), MnemoError::Replaced { missing: true, .. }),
                "{}",
                e.error()
            );
            e.into_inner()
        }
    };
    assert_eq!(
        snapshot(&moved),
        before,
        "nothing was written to the moved file"
    );
    std::fs::rename(&moved, &path).unwrap();
    back.close().unwrap();
    assert_eq!(count(&path), 3, "moved back, the handle's write commits");

    // Replaced by a copy of itself: another file is at the path.
    let mut db = Mnemo::open(&path, PW).unwrap();
    db.remember(Memory::new("lost", MemoryType::Semantic, vec![1.0, 0.0]))
        .unwrap();
    let copy = dir.path().join("copy.mnemo");
    std::fs::copy(&path, &copy).unwrap();
    if let Err(e) = std::fs::rename(&copy, &path) {
        if is_rename_refused(&MnemoError::Io(e)) {
            eprintln!("skipped: this toolchain's rename can't replace an open file");
            return;
        }
        panic!("rename failed");
    }
    let err = db.flush().unwrap_err();
    assert!(
        matches!(err, MnemoError::Replaced { missing: false, .. }),
        "{err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains("was replaced") && msg.contains("Reopen"),
        "{msg}"
    );
    assert!(!msg.contains("moved or removed"), "{msg}");
    drop(db);
    assert_eq!(count(&path), 3);

    // Removed. (Unix only: on Windows a removed file that a handle has
    // open can stay at its path, pending deletion, until the handle closes.)
    #[cfg(unix)]
    {
        let mut db = Mnemo::open(&path, PW).unwrap();
        db.remember(Memory::new("gone", MemoryType::Semantic, vec![1.0, 0.0]))
            .unwrap();
        std::fs::remove_file(&path).unwrap();
        let err = db.flush().unwrap_err();
        assert!(
            matches!(err, MnemoError::Replaced { missing: true, .. }),
            "{err}"
        );
        assert!(err.to_string().contains("was moved or removed"), "{err}");
        drop(db);
        assert!(!path.exists());
    }
}

/// The path is checked as an absolute path, so a handle opened with a
/// relative path keeps working after the process changes directory.
/// (The only test here that changes the current directory; it restores it.)
#[test]
fn a_relative_path_survives_a_change_of_directory() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("rel.mnemo");
    seed(&path, 1);
    let cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();
    let mut db = Mnemo::open("rel.mnemo", PW).unwrap();
    std::env::set_current_dir(&cwd).unwrap();
    db.remember(Memory::new(
        "after cd",
        MemoryType::Semantic,
        vec![1.0, 0.0],
    ))
    .unwrap();
    db.flush().unwrap();
    db.close().unwrap();
    assert_eq!(count(&path), 2);
}

/// `create` used to truncate the file before trying its lock, so a refused
/// `create` over a file another handle had open emptied it.
#[test]
fn a_refused_create_leaves_the_live_file_intact() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("live.mnemo");
    seed(&path, 3);
    let live = Mnemo::open(&path, PW).unwrap();
    // The size, not the bytes: on Windows the live handle's lock is
    // mandatory, so reading the file while it's open fails (os error 33).
    let before = std::fs::metadata(&path).unwrap().len();
    assert!(before > 0);

    let err = Mnemo::create(&path, "other-pw", cfg())
        .err()
        .expect("create must be refused");
    assert!(matches!(err, MnemoError::Locked { .. }), "{err}");
    assert_eq!(
        std::fs::metadata(&path).unwrap().len(),
        before,
        "the live file is untouched"
    );
    drop(live);
    assert_eq!(count(&path), 3);

    // With no other handle, `create` still replaces the file, as before.
    drop(Mnemo::create(&path, "other-pw", cfg()).unwrap());
    let fresh = Mnemo::open_read_only(&path, "other-pw").unwrap();
    assert_eq!(fresh.stats_summary().memories, 0);
}

/// Compaction's temp file has a name of its own and is gone afterwards.
#[test]
fn compaction_leaves_no_temp_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("c.mnemo");
    seed(&path, 4);
    Mnemo::compact_file(path.to_str().unwrap(), PW).unwrap();
    Mnemo::compact_file(path.to_str().unwrap(), PW).unwrap();
    let names: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["c.mnemo".to_string()], "{names:?}");
    assert_eq!(count(&path), 4);
}

/// Plant something at the temp name the engine is about to use: a symlink
/// to another file of the user's (Unix), or a plain file (everywhere).
/// Returns what to compare afterwards.
#[cfg(feature = "failpoints")]
fn plant(at: &Path, victim: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(victim, at).unwrap();
    #[cfg(not(unix))]
    std::fs::write(at, std::fs::read(victim).unwrap()).unwrap();
}

/// RB-006.10: compaction, `export_encrypted` and `export_rekeyed` create
/// their temp files with `create_new`, so a file or symlink already at the
/// name is never written through: the call takes another name, and the
/// other file is left byte-identical. The planted name is the exact one the
/// engine draws next (`__failpoints::set_next_temp_name`), plus the old
/// fixed names.
#[cfg(feature = "failpoints")]
#[test]
fn nothing_at_a_temp_name_is_ever_written_through() {
    use mnemo_engine::__failpoints::set_next_temp_name;
    let dir = tempdir().unwrap();
    let victim = dir.path().join("victim.txt");
    std::fs::write(&victim, b"someone else's file").unwrap();
    let brain = dir.path().join("brain.mnemo");
    seed(&brain, 3);
    let p = brain.to_str().unwrap();

    // The old fixed names (v0.13.2 and earlier).
    plant(&dir.path().join("brain.mnemo.compact-tmp"), &victim);
    plant(&dir.path().join("out.mnemo.export-tmp"), &victim);
    plant(&dir.path().join("re.mnemo.export-tmp"), &victim);

    let check = |what: &str| {
        assert_eq!(
            std::fs::read(&victim).unwrap(),
            b"someone else's file",
            "{what}: the other file was written through"
        );
    };

    // Compaction (and so `rekey --rotate`).
    let planted = dir.path().join("brain.mnemo.planted-compact.tmp");
    plant(&planted, &victim);
    set_next_temp_name(&planted);
    Mnemo::compact_file(p, PW).unwrap();
    check("compact_file");
    assert!(
        !std::fs::symlink_metadata(&brain)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the brain's path must stay a regular file"
    );
    assert_eq!(count(&brain), 3);

    // export_encrypted.
    let out = dir.path().join("out.mnemo");
    let planted = dir.path().join("out.mnemo.planted-export.tmp");
    plant(&planted, &victim);
    let mut db = Mnemo::open(&brain, PW).unwrap();
    set_next_temp_name(&planted);
    db.export_encrypted(&out).unwrap();
    check("export_encrypted");

    // export_rekeyed.
    let re = dir.path().join("re.mnemo");
    let planted = dir.path().join("re.mnemo.planted-rekeyed.tmp");
    plant(&planted, &victim);
    set_next_temp_name(&planted);
    db.export_rekeyed(&re, "other-pw", KdfParams::fast())
        .unwrap();
    check("export_rekeyed");
    drop(db);

    for f in [&out, &re] {
        assert!(!std::fs::symlink_metadata(f)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    assert_eq!(count(&out), 3);
    let mut r = Mnemo::open_read_only(&re, "other-pw").unwrap();
    assert_eq!(r.stats().unwrap().memories, 3);
}
