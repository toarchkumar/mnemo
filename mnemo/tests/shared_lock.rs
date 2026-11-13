//! v0.14.0 step 2a: the shared lock layer (12.4 design note, "The
//! protocol", "Writer preference", "Long operations", "Sidecar files",
//! "Things (A) refuses"), through its test hooks (`__shared`, failpoints
//! builds only). Runs on every CI OS, and on the `windows-msrv` job, where
//! shared mode refuses to start and only the refusal test runs.
#![cfg(feature = "failpoints")]

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;
use mnemo_engine::__shared::{self, Locks};
use mnemo_engine::{CancelToken, MnemoError, SharedOptions};
use tempfile::tempdir;

fn opts(deadline_ms: u64) -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_millis(deadline_ms);
    o
}

/// A brain file (the lock layer doesn't read it) in a fresh directory.
fn brain(dir: &Path) -> PathBuf {
    let p = dir.join("b.mnemo");
    std::fs::write(&p, b"brain").unwrap();
    p
}

/// The lock layer, or `None` (the test skips) on a build that refuses
/// shared mode: Windows before Rust 1.85.
fn locks(path: &Path, o: &SharedOptions) -> Option<Locks> {
    if __shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode (Windows, Rust < 1.85)");
        return None;
    }
    Some(Locks::new(path, o).unwrap())
}

/// Can a fresh open of `path` take an exclusive lock right now? Unlocks.
fn free(path: &Path) -> bool {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    let ok = FileExt::try_lock_exclusive(&f).is_ok();
    if ok {
        FileExt::unlock(&f).unwrap();
    }
    ok
}

#[test]
// It asserts on a constant on purpose: whether build.rs saw the compiler.
#[allow(clippy::assertions_on_constants)]
fn windows_before_rust_1_85_refuses_shared_mode() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let r = Locks::new(&p, &SharedOptions::default());
    // The windows-msrv CI job sets this, so a build.rs that failed to see
    // the old compiler fails here instead of passing the other branch.
    if std::env::var_os("MNEMO_EXPECT_SHARED_REFUSAL").is_some() {
        assert!(
            __shared::WINDOWS_OLD_RENAME,
            "this build was expected to refuse shared mode (Windows, Rust < 1.85)"
        );
    }
    if __shared::WINDOWS_OLD_RENAME {
        let e = r.unwrap_err();
        assert!(matches!(e, MnemoError::Unsupported(_)), "{e}");
        assert!(
            e.to_string()
                .contains("shared mode on Windows needs a build with Rust 1.85 or later"),
            "{e}"
        );
    } else {
        r.unwrap();
    }
}

#[test]
fn a_call_fails_with_locked_at_its_deadline() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(a) = locks(&p, &opts(1000)) else {
        return;
    };
    let b = Locks::new(&p, &opts(200)).unwrap();
    let held = a.exclusive(&opts(1000)).unwrap();
    let t = Instant::now();
    let e = b.shared(&opts(200)).err().expect("must time out");
    let waited = t.elapsed();
    assert!(matches!(e, MnemoError::Locked { holder: None, .. }), "{e}");
    assert!(
        waited >= Duration::from_millis(150) && waited < Duration::from_secs(3),
        "waited {waited:?}"
    );
    drop(held);
    b.shared(&opts(200)).unwrap();
}

#[test]
fn lock_info_names_the_holder_and_goes_with_the_lock() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(a) = locks(&p, &opts(1000)) else {
        return;
    };
    let b = Locks::new(&p, &opts(200)).unwrap();
    assert!(a.has_sidecar(), "{:?}", a.notices());
    let info = a.sidecar_path().unwrap().join(__shared::LOCK_INFO);

    let mut held = a.exclusive(&opts(1000)).unwrap();
    held.announce("compact");
    assert!(info.exists());
    let e = b.exclusive(&opts(200)).err().expect("must time out");
    let msg = e.to_string();
    match &e {
        MnemoError::Locked {
            holder: Some(h), ..
        } => {
            assert!(h.starts_with("compact since "), "{h}");
            assert!(h.contains(&format!("pid {}", std::process::id())), "{h}");
        }
        other => panic!("{other}"),
    }
    assert!(msg.contains("(held by compact since "), "{msg}");

    drop(held);
    assert!(!info.exists(), "lock-info must go before the unlock");
    b.exclusive(&opts(200)).unwrap();
}

#[test]
fn a_cancelled_wait_returns_at_once_and_holds_no_lock() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(a) = locks(&p, &opts(1000)) else {
        return;
    };
    let held = a.exclusive(&opts(1000)).unwrap();
    let intent = a.sidecar_path().unwrap().join(__shared::INTENT);

    let token = CancelToken::new();
    let waiter = {
        let (p, token) = (p.clone(), token.clone());
        std::thread::spawn(move || {
            let mut o = opts(30_000);
            o.cancel = Some(token);
            let b = Locks::new(&p, &o).unwrap();
            let t = Instant::now();
            let r = b.exclusive(&o).err();
            (r, t.elapsed())
        })
    };
    std::thread::sleep(Duration::from_millis(200));
    // The waiting writer holds the intent lock.
    assert!(!free(&intent), "a waiting writer holds the intent lock");
    token.cancel();
    let (r, waited) = waiter.join().unwrap();
    assert!(matches!(r, Some(MnemoError::Cancelled)), "{r:?}");
    assert!(waited < Duration::from_secs(3), "waited {waited:?}");
    assert!(free(&intent), "the cancelled call released the intent lock");
    drop(held);
    assert!(free(&p));
}

#[test]
fn a_writer_gets_in_against_eight_busy_readers() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(w) = locks(&p, &opts(10_000)) else {
        return;
    };
    let stop = Arc::new(AtomicBool::new(false));
    let readers: Vec<_> = (0..8)
        .map(|_| {
            let (p, stop) = (p.clone(), stop.clone());
            std::thread::spawn(move || {
                let r = Locks::new(&p, &opts(10_000)).unwrap();
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let held = r.shared(&opts(10_000)).unwrap();
                    std::thread::sleep(Duration::from_millis(2));
                    drop(held);
                    reads += 1;
                }
                reads
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    let mut worst = Duration::ZERO;
    for _ in 0..100 {
        let t = Instant::now();
        let held = w.exclusive(&opts(10_000)).unwrap();
        worst = worst.max(t.elapsed());
        std::thread::sleep(Duration::from_millis(1));
        drop(held);
    }
    stop.store(true, Ordering::Relaxed);
    let reads: u64 = readers.into_iter().map(|r| r.join().unwrap()).sum();
    eprintln!("100 writes, longest wait {worst:?}; {reads} reads meanwhile");
    assert!(worst < Duration::from_secs(1), "a write waited {worst:?}");
    assert!(reads > 0);
}

#[test]
fn a_reader_holds_the_intent_lock_for_one_try_only() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(a) = locks(&p, &opts(1000)) else {
        return;
    };
    let intent = a.sidecar_path().unwrap().join(__shared::INTENT);
    let held = a.exclusive(&opts(1000)).unwrap();
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let p = p.clone();
            std::thread::spawn(move || {
                let r = Locks::new(&p, &opts(10_000)).unwrap();
                r.shared(&opts(10_000)).map(|_| ())
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(100));
    // While the readers wait for the main lock, the intent lock is free
    // most of the time, so a second writer could queue.
    let mut free_samples = 0;
    for _ in 0..50 {
        if free(&intent) {
            free_samples += 1;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        free_samples > 25,
        "the intent lock was free {free_samples} times of 50"
    );
    drop(held);
    for r in readers {
        r.join().unwrap().unwrap();
    }
}

#[test]
fn shared_mode_never_creates_the_brain() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.mnemo");
    if __shared::WINDOWS_OLD_RENAME {
        return;
    }
    let e = Locks::new(&missing, &opts(200)).unwrap_err();
    assert!(
        matches!(&e, MnemoError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "{e}"
    );
    assert!(!missing.exists());

    // Deleted after the handle was set up: still an error, still not created.
    let p = brain(dir.path());
    let l = Locks::new(&p, &opts(200)).unwrap();
    std::fs::remove_file(&p).unwrap();
    let e = l.exclusive(&opts(200)).err().expect("must fail");
    assert!(
        matches!(&e, MnemoError::Io(io) if io.kind() == std::io::ErrorKind::NotFound),
        "{e}"
    );
    assert!(!p.exists());
}

#[test]
fn a_lock_is_always_on_the_file_at_the_path() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(l) = locks(&p, &opts(1000)) else {
        return;
    };
    let other = dir.path().join("new.mnemo");
    std::fs::write(&other, b"the new file").unwrap();
    std::fs::rename(&other, &p).unwrap();
    let held = l.exclusive(&opts(1000)).unwrap();
    let mut s = String::new();
    std::io::Read::read_to_string(&mut held.file(), &mut s).unwrap();
    assert_eq!(s, "the new file");
}

/// FLAKE-1's pattern: a thread spawns children in a loop while calls take
/// and release locks; after every call, a fresh open must find both the
/// brain and the intent lock free (no child's copy of a descriptor kept a
/// lock).
#[test]
fn locks_never_outlive_their_call_while_children_spawn() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path());
    let Some(l) = locks(&p, &opts(5000)) else {
        return;
    };
    let intent = l.sidecar_path().unwrap().join(__shared::INTENT);
    let stop = Arc::new(AtomicBool::new(false));
    let spawned = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let spawner = {
        let (stop, spawned) = (stop.clone(), spawned.clone());
        std::thread::spawn(move || {
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) {
                #[cfg(unix)]
                let child = std::process::Command::new("true").status();
                #[cfg(not(unix))]
                let child = std::process::Command::new("cmd")
                    .args(["/C", "exit"])
                    .status();
                child.unwrap();
                n += 1;
                spawned.store(n, Ordering::Relaxed);
            }
            n
        })
    };
    // At least 200 calls, and on until 200 children have been spawned.
    let mut i = 0u32;
    while i < 200 || spawned.load(Ordering::Relaxed) < 200 {
        assert!(i < 1_000_000, "the spawner made no progress");
        i += 1;
        let held = if i % 2 == 0 {
            l.exclusive(&opts(5000)).unwrap()
        } else {
            l.shared(&opts(5000)).unwrap()
        };
        drop(held);
        assert!(free(&p), "call {i}: the brain's lock outlived the call");
        assert!(free(&intent), "call {i}: the intent lock outlived the call");
    }
    stop.store(true, Ordering::Relaxed);
    let children = spawner.join().unwrap();
    eprintln!("{i} calls while {children} children were spawned");
}

#[test]
fn the_linux_filesystem_check_is_an_allow_list() {
    for magic in [
        0xEF53,
        0x5846_5342,
        0x9123_683E,
        0x0102_1994,
        0xF2F5_2010,
        0x794C_7630,
    ] {
        assert!(__shared::linux_fs_allowed(magic), "{magic:#x}");
    }
    // NFS, SMB (cifs, smb2), FUSE, virtiofs, 9p, an unknown type.
    for magic in [
        0x6969,
        0xFF53_4D42,
        0xFE53_4D42,
        0x6573_5546,
        0x6A65_6A63,
        0x0102_1997,
        0x1234,
    ] {
        assert!(!__shared::linux_fs_allowed(magic), "{magic:#x}");
    }
}

#[test]
fn synced_folders_on_macos_are_refused_or_warned() {
    let home = Path::new("/Users/someone");
    let c = |p: &str| __shared::classify_synced(Path::new(p), home);
    let r = c("/Users/someone/Library/Mobile Documents/com~apple~CloudDocs/b.mnemo").unwrap();
    assert!(
        r.starts_with("refuse: ") && r.contains("iCloud Drive"),
        "{r}"
    );
    let r = c("/Users/someone/Library/CloudStorage/Dropbox/b.mnemo").unwrap();
    assert!(r.starts_with("refuse: "), "{r}");
    let r = c("/Users/someone/Documents/proj/b.mnemo").unwrap();
    assert!(r.starts_with("warn: "), "{r}");
    let r = c("/Users/someone/Desktop/b.mnemo").unwrap();
    assert!(r.starts_with("warn: "), "{r}");
    assert_eq!(
        c("/Users/someone/Documents/GitHub/mnemo.nosync/test/b.mnemo"),
        None
    );
    assert_eq!(c("/Users/someone/code/b.mnemo"), None);
    assert_eq!(c("/tmp/b.mnemo"), None);
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    #[test]
    fn the_sidecar_follows_the_resolved_brain_path() {
        let dir = tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let p = brain(&real);
        let link = dir.path().join("link");
        symlink(&real, &link).unwrap();
        let Some(l) = locks(&link.join("b.mnemo"), &opts(200)) else {
            return;
        };
        let expected = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
        assert_eq!(l.sidecar_path().unwrap(), expected);
        let mode = std::fs::metadata(&expected).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "mode {mode:o}");
    }

    #[test]
    fn a_symlinked_sidecar_directory_is_not_used() {
        let dir = tempdir().unwrap();
        let p = brain(dir.path());
        let elsewhere = dir.path().join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::set_permissions(&elsewhere, std::fs::Permissions::from_mode(0o700)).unwrap();
        let side = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
        symlink(&elsewhere, &side).unwrap();
        let Some(l) = locks(&p, &opts(200)) else {
            return;
        };
        assert!(!l.has_sidecar());
        assert!(
            l.notices()
                .iter()
                .any(|n| n.contains("isn't a real directory")),
            "{:?}",
            l.notices()
        );
        let mut held = l.exclusive(&opts(200)).unwrap();
        held.announce("compact");
        drop(held);
        l.shared(&opts(200)).unwrap();
        assert_eq!(std::fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn a_sidecar_directory_open_to_others_is_not_used() {
        let dir = tempdir().unwrap();
        let p = brain(dir.path());
        let side = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
        std::fs::create_dir(&side).unwrap();
        std::fs::set_permissions(&side, std::fs::Permissions::from_mode(0o755)).unwrap();
        let Some(l) = locks(&p, &opts(200)) else {
            return;
        };
        assert!(!l.has_sidecar());
        assert!(
            l.notices().iter().any(|n| n.contains("0700")),
            "{:?}",
            l.notices()
        );
        l.exclusive(&opts(200)).unwrap();
        assert_eq!(std::fs::read_dir(&side).unwrap().count(), 0);
    }

    #[test]
    fn a_symlink_at_a_fixed_name_is_never_followed() {
        let dir = tempdir().unwrap();
        let p = brain(dir.path());
        let Some(l) = locks(&p, &opts(1000)) else {
            return;
        };
        let side = l.sidecar_path().unwrap();
        let victim = dir.path().join("victim.txt");
        std::fs::write(&victim, b"someone else's file").unwrap();
        symlink(&victim, side.join(__shared::INTENT)).unwrap();
        symlink(&victim, side.join(__shared::LOCK_INFO)).unwrap();

        let mut held = l.exclusive(&opts(1000)).unwrap();
        held.announce("compact");
        drop(held);
        l.shared(&opts(1000)).unwrap();
        let other = Locks::new(&p, &opts(200)).unwrap();
        let held = l.exclusive(&opts(1000)).unwrap();
        let _ = other.exclusive(&opts(200)).err().expect("must time out");
        drop(held);

        assert_eq!(std::fs::read(&victim).unwrap(), b"someone else's file");
        // `lock-info` was replaced by rename and removed, never written
        // through; the planted `intent.lock` is still a symlink.
        assert!(!side.join(__shared::LOCK_INFO).exists());
        assert!(std::fs::symlink_metadata(side.join(__shared::INTENT))
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[test]
    fn no_sidecar_where_the_directory_is_not_writable() {
        if rustix_euid_is_root() {
            eprintln!("skipped: root writes anywhere");
            return;
        }
        let dir = tempdir().unwrap();
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        let p = brain(&ro);
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = std::panic::catch_unwind(|| {
            let Some(l) = locks(&p, &opts(200)) else {
                return;
            };
            assert!(!l.has_sidecar());
            assert!(
                l.notices().iter().any(|n| n.contains("isn't writable")),
                "{:?}",
                l.notices()
            );
            l.shared(&opts(200)).unwrap();
        });
        std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
        result.unwrap();
    }

    fn rustix_euid_is_root() -> bool {
        // The test's own file is owned by its effective user.
        use std::os::unix::fs::MetadataExt;
        let f = tempfile::NamedTempFile::new().unwrap();
        f.as_file().metadata().unwrap().uid() == 0
    }
}
