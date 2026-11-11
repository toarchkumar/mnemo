//! FLAKE-1 (v0.12.3): a handle that's dropped or closed releases its file
//! lock at once, even while other threads of the process spawn child
//! processes.
//!
//! On Linux, `std::process::Command` spawns through `posix_spawn`, whose
//! child holds a copy of the parent's file table until it `exec`s. A lock
//! taken with `flock` belongs to the open file description, so a handle
//! dropped inside that window left the lock held by the child's copy, and
//! the next open failed with `Locked`. That's what
//! `cli_smoke::recall_text_falls_back_to_read_only_when_write_lock_held`
//! hit on CI. The engine now unlocks explicitly before it closes the file.

#![cfg(unix)]

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError};
use tempfile::tempdir;

const PASS: &str = "lock-release";

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

/// Opens the file one way, drops or closes it, and opens it the other way at
/// once, 400 times (with a wrong-key open first every tenth time), while
/// four threads spawn `true` as fast as they can. Every open must succeed.
#[test]
fn a_dropped_handle_releases_its_lock_while_other_threads_spawn() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("lock.mnemo");
    {
        let mut db = Mnemo::create(&path, PASS, cfg()).unwrap();
        db.remember(Memory::new(
            "x",
            MemoryType::Semantic,
            vec![1.0, 0.0, 0.5, 1.0],
        ))
        .unwrap();
        db.flush().unwrap();
    }

    let stop = Arc::new(AtomicBool::new(false));
    let spawners: Vec<_> = (0..4)
        .map(|_| {
            let stop = Arc::clone(&stop);
            thread::spawn(move || {
                let mut spawned = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let status = Command::new("true")
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status();
                    if status.is_ok() {
                        spawned += 1;
                    }
                }
                spawned
            })
        })
        .collect();

    let mut locked_write = 0;
    let mut locked_read = 0;
    let mut wrong_key = 0;
    for i in 0..400 {
        // A failed open (a wrong key) must release the lock as promptly.
        if i % 10 == 0 {
            match Mnemo::open(&path, "not-the-passphrase") {
                Err(MnemoError::WrongPassphrase) => wrong_key += 1,
                Err(MnemoError::Locked { .. }) => locked_write += 1,
                other => panic!("open with a wrong key: {:?}", other.map(|_| ())),
            }
        }
        match Mnemo::open(&path, PASS) {
            Ok(db) => {
                if i % 2 == 0 {
                    drop(db);
                } else {
                    db.close().unwrap();
                }
            }
            Err(MnemoError::Locked { .. }) => locked_write += 1,
            Err(e) => panic!("open: {e}"),
        }
        match Mnemo::open_read_only(&path, PASS) {
            Ok(reader) => drop(reader),
            Err(MnemoError::Locked { .. }) => locked_read += 1,
            Err(e) => panic!("open_read_only: {e}"),
        }
    }

    stop.store(true, Ordering::Relaxed);
    let spawned: u64 = spawners.into_iter().map(|t| t.join().unwrap()).sum();
    assert!(spawned > 0, "no child process was spawned");
    assert_eq!(
        wrong_key, 40,
        "every wrong-key open fails as WrongPassphrase"
    );
    assert_eq!(
        (locked_write, locked_read),
        (0, 0),
        "opens that found the lock still held right after a drop or close \
         ({spawned} children spawned meanwhile)"
    );
}
