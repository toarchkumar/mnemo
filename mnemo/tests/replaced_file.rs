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
            let mut compactions = 0u32;
            while !done.load(Ordering::Relaxed) || compactions < 5 {
                retry_locked(|| Mnemo::compact_file(&p, PW)).expect("compaction failed");
                compactions += 1;
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
    std::fs::rename(&other, &path).unwrap();
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
    let before = std::fs::read(&path).unwrap();

    let err = Mnemo::create(&path, "other-pw", cfg())
        .err()
        .expect("create must be refused");
    assert!(matches!(err, MnemoError::Locked { .. }), "{err}");
    assert_eq!(
        std::fs::read(&path).unwrap(),
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
