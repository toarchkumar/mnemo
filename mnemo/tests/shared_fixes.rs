//! v0.14.0 step 2c: the fixes to shared mode's steps 2a and 2b (RB-007),
//! and two changes to compaction that shared mode relies on (it renames over
//! the file a symlink names, and keeps the file's mode). Failpoints builds
//! only, since several tests fail chosen writes or count lock tries.
#![cfg(feature = "failpoints")]

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use fs4::fs_std::FileExt;
use mnemo_engine::__failpoints::{set_shared_replaced, set_writes_until_fail, shared_main_tries};
use mnemo_engine::__shared::{self, Locks};
use mnemo_engine::{
    CancelToken, Embedder, EmbedderIdentity, KdfParams, Memory, MemoryType, Mnemo, MnemoConfig,
    MnemoError, Result, SharedMnemo, SharedOptions, Ulid,
};
use tempfile::tempdir;

const PW: &str = "fixes-pw";
const NEW: &str = "fixes-new-pw";

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

/// A v9 brain with `n` memories at `dir/b.mnemo`, closed.
fn brain(dir: &Path, n: usize) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join("b.mnemo");
    let mut db = Mnemo::create(&p, PW, cfg()).unwrap();
    for i in 0..n {
        db.remember(mem(&format!("seed {i}"))).unwrap();
    }
    db.flush().unwrap();
    db.close().unwrap();
    p
}

fn opts(deadline_ms: u64) -> SharedOptions {
    let mut o = SharedOptions::default();
    o.deadline = Duration::from_millis(deadline_ms);
    o
}

/// A shared handle, or `None` (the test skips) on a build that refuses
/// shared mode (Windows, Rust before 1.85).
fn open(p: &Path, secret: &str, o: SharedOptions) -> Option<SharedMnemo> {
    if __shared::WINDOWS_OLD_RENAME {
        eprintln!("skipped: this build refuses shared mode");
        return None;
    }
    Some(Mnemo::open_shared(p, secret, o).unwrap())
}

fn texts(s: &mut SharedMnemo) -> Vec<String> {
    let mut v: Vec<String> = s
        .read(|db| db.memories())
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    v.sort();
    v
}

/// What a fresh open of the file has, opened read-write in a copy (so a
/// pending WAL is replayed there, as any open would).
fn on_disk(p: &Path, secret: &str) -> Vec<String> {
    let copy = p.with_extension("view");
    std::fs::copy(p, &copy).unwrap();
    let mut db = Mnemo::open(&copy, secret).unwrap();
    let mut v: Vec<String> = db
        .memories()
        .unwrap()
        .into_iter()
        .map(|m| m.content)
        .collect();
    drop(db);
    std::fs::remove_file(&copy).unwrap();
    v.sort();
    v
}

fn remember(s: &mut SharedMnemo, text: &str) -> Result<Ulid> {
    let m = mem(text);
    s.write(move |db| db.remember(m))
}

/// A plain writer opens `p` with `secret`, writes `text`, and fails the
/// `n`th write of its flush. True if that left a pending WAL (it died after
/// its commit point).
fn die_after_commit(p: &Path, secret: &str, text: &str, n: i64) -> bool {
    let mut plain = Mnemo::open(p, secret).unwrap();
    plain.remember(mem(text)).unwrap();
    set_writes_until_fail(n);
    let r = plain.flush();
    set_writes_until_fail(-1);
    drop(plain);
    r.is_err()
        && matches!(
            Mnemo::open_read_only(p, secret).err(),
            Some(MnemoError::NeedsWriteOpen { .. })
        )
}

fn intent_path(p: &Path) -> PathBuf {
    __shared::sidecar_path(&std::fs::canonicalize(p).unwrap()).join(__shared::INTENT)
}

struct Fixed;

impl Embedder for Fixed {
    fn identity(&self) -> EmbedderIdentity {
        EmbedderIdentity {
            name: "fixed-test".into(),
            dimensions: 2,
            normalize: false,
        }
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|_| vec![0.0, 1.0]).collect())
    }
}

// --- RB-007.1: a cancel, then the handle's next calls ------------------

#[test]
fn a_cancelled_handle_takes_a_new_token_and_carries_on() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let token = CancelToken::new();
    let mut o = opts(10_000);
    o.cancel = Some(token.clone());
    let Some(mut a) = open(&p, PW, o) else { return };

    // A plain handle holds the file, so the read waits, and is cancelled.
    let plain = Mnemo::open(&p, PW).unwrap();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        token.cancel();
    });
    let t = Instant::now();
    let e = a.read(|db| db.memories()).unwrap_err();
    assert!(matches!(e, MnemoError::Cancelled), "{e}");
    assert!(t.elapsed() < Duration::from_secs(5));
    canceller.join().unwrap();
    drop(plain);

    // The token stays cancelled, so the handle needs a new one.
    let e = a.read(|db| db.memories()).unwrap_err();
    assert!(matches!(e, MnemoError::Cancelled), "{e}");
    let fresh = CancelToken::new();
    a.set_cancel(Some(fresh.clone()));
    assert!(a.options().cancel.is_some());
    assert_eq!(texts(&mut a), vec!["seed 0"]);
    remember(&mut a, "after the cancel").unwrap();
    a.set_cancel(None);
    assert_eq!(texts(&mut a), vec!["after the cancel", "seed 0"]);
}

// --- RB-007.2: the kept secret and a rekey -----------------------------

#[test]
fn a_rekey_through_the_handle_updates_the_kept_secret() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    a.rekey(NEW, KdfParams::fast()).unwrap();
    assert!(matches!(
        Mnemo::open(&p, PW).err(),
        Some(MnemoError::WrongPassphrase)
    ));
    assert!(a.rekey_cleanup_error().is_none());
    // A compaction under the new secret gives the file a new data key: the
    // handle follows it with the secret it now keeps.
    Mnemo::compact_file(p.to_str().unwrap(), NEW).unwrap();
    assert_eq!(texts(&mut a), vec!["seed 0"]);
    remember(&mut a, "after").unwrap();
    assert_eq!(on_disk(&p, NEW), vec!["after", "seed 0"]);

    // Back to a raw key, the same way.
    let key = Mnemo::generate_key();
    a.rekey_with_key(&key).unwrap();
    Mnemo::compact_file_with_key(p.to_str().unwrap(), &key).unwrap();
    assert_eq!(texts(&mut a), vec!["after", "seed 0"]);
}

#[test]
fn a_rekey_inside_a_window_is_refused() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    let e = a.write(|db| db.rekey(NEW, KdfParams::fast())).unwrap_err();
    assert!(matches!(e, MnemoError::Unsupported(_)), "{e}");
    assert!(e.to_string().contains("SharedMnemo::rekey"), "{e}");
    let e = a.write(|db| db.rekey_with_key(&[7u8; 32])).unwrap_err();
    assert!(matches!(e, MnemoError::Unsupported(_)), "{e}");
    // Nothing changed: the old secret opens the file.
    assert_eq!(on_disk(&p, PW), vec!["seed 0"]);
    remember(&mut a, "still works").unwrap();
}

/// Another process changes the passphrase, then a writer dies after its
/// commit point: the old-secret handle replays the WAL with the data key it
/// holds (a rekey keeps it), where it used to fail with `WrongPassphrase`.
#[test]
fn a_pending_wal_after_another_processs_rekey_is_replayed_with_the_held_key() {
    for n in 0..40 {
        let dir = tempdir().unwrap();
        let p = brain(dir.path(), 1);
        let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
            return;
        };
        assert_eq!(texts(&mut a), vec!["seed 0"]);
        let mut plain = Mnemo::open(&p, PW).unwrap();
        plain.rekey(NEW, KdfParams::fast()).unwrap();
        plain.close().unwrap();
        if !die_after_commit(&p, NEW, "dying", n) {
            continue;
        }
        assert_eq!(texts(&mut a), vec!["dying", "seed 0"], "n={n}");
        assert!(
            Mnemo::open_read_only(&p, NEW).is_ok(),
            "the WAL was replayed"
        );
        remember(&mut a, "after").unwrap();
        assert_eq!(on_disk(&p, NEW), vec!["after", "dying", "seed 0"]);
        return;
    }
    panic!("no write left a pending WAL");
}

/// The same for a torn page 0: the torn-header scan accepts the newest
/// header frame when its seal verifies under the held data key.
#[test]
fn a_torn_header_after_another_processs_rekey_is_healed_with_the_held_key() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    let mut plain = Mnemo::open(&p, PW).unwrap();
    plain.rekey(NEW, KdfParams::fast()).unwrap();
    plain.remember(mem("under the new secret")).unwrap();
    plain.close().unwrap();
    {
        let mut f = OpenOptions::new().write(true).open(&p).unwrap();
        f.seek(SeekFrom::Start(0)).unwrap();
        f.write_all(&[0u8; 512]).unwrap();
        f.sync_all().unwrap();
    }
    assert_eq!(texts(&mut a), vec!["seed 0", "under the new secret"]);
    assert_eq!(on_disk(&p, NEW), vec!["seed 0", "under the new secret"]);
}

// --- RB-007.3: two paths to one brain through a compaction --------------

#[cfg(unix)]
#[test]
fn a_compaction_through_a_symlink_replaces_the_real_file() {
    let dir = tempdir().unwrap();
    let real = brain(&dir.path().join("real"), 1);
    let link = dir.path().join("link.mnemo");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let Some(mut a) = open(&link, PW, SharedOptions::default()) else {
        return;
    };
    let mut b = open(&real, PW, SharedOptions::default()).unwrap();
    remember(&mut a, "through the link").unwrap();

    Mnemo::compact_file(link.to_str().unwrap(), PW).unwrap();
    assert!(
        std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink(),
        "the link is still a link"
    );
    remember(&mut a, "after, through the link").unwrap();
    let want = vec!["after, through the link", "seed 0", "through the link"];
    assert_eq!(texts(&mut b), want);
    assert_eq!(texts(&mut a), want);

    // Through the real path too, and a plain handle's `rekey --rotate`
    // shape (a rekey, then a compaction) through the link.
    Mnemo::compact_file(real.to_str().unwrap(), PW).unwrap();
    remember(&mut b, "after, real").unwrap();
    assert_eq!(texts(&mut a), texts(&mut b));
    let mut plain = Mnemo::open(&link, PW).unwrap();
    plain.rekey(NEW, KdfParams::fast()).unwrap();
    plain.close().unwrap();
    Mnemo::compact_file(link.to_str().unwrap(), NEW).unwrap();
    assert!(std::fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(on_disk(&real, NEW).len(), 4);
    // One sidecar, beside the real file.
    assert!(!__shared::sidecar_path(&link).exists());
}

// --- RB-007.5: a window that commits part-way ---------------------------

#[test]
fn a_window_that_commits_part_way_never_returns_a_plain_error_with_part_stored() {
    let (mut part, mut done) = (0, false);
    for n in 0..60 {
        let dir = tempdir().unwrap();
        let p = brain(dir.path(), 1);
        let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
            return;
        };
        set_writes_until_fail(n);
        let r = a.write(|db| {
            db.remember(mem("first"))?;
            db.flush()?;
            db.remember(mem("second"))?;
            Ok(())
        });
        set_writes_until_fail(-1);
        let disk = on_disk(&p, PW);
        let first = disk.contains(&"first".to_string());
        let second = disk.contains(&"second".to_string());
        match r {
            Ok(()) => {
                assert!(first && second, "n={n}: Ok with {disk:?}");
                done = true;
                break;
            }
            Err(MnemoError::MaybeStored(msg)) => {
                if msg.contains("part of the window") {
                    part += 1;
                }
            }
            Err(e) => assert!(
                !first && !second,
                "n={n}: a plain error ({e}) with {disk:?} in the file"
            ),
        }
        // The handle is usable after any outcome.
        remember(&mut a, "next").unwrap();
    }
    assert!(done, "no write window finished");
    assert!(part > 0, "no failure said part of the window was stored");
}

#[test]
fn a_setting_refused_after_a_commit_in_the_window_is_maybe_stored() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    let e = a
        .write(|db| {
            db.remember(mem("first"))?;
            db.flush()?;
            db.set_max_snapshots(3);
            Ok(())
        })
        .unwrap_err();
    assert!(matches!(e, MnemoError::MaybeStored(_)), "{e}");
    assert!(e.to_string().contains("setting wasn't applied"), "{e}");
    assert_eq!(on_disk(&p, PW), vec!["first", "seed 0"]);
    assert_eq!(texts(&mut a), vec!["first", "seed 0"]);
}

// --- RB-007.6: windows that write nothing commit nothing ----------------

#[test]
fn a_window_that_writes_nothing_leaves_the_transaction_id() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    let mut b = open(&p, PW, SharedOptions::default()).unwrap();
    let txn = |s: &mut SharedMnemo| s.read(|db| Ok(db.__transaction_id())).unwrap();
    let t0 = txn(&mut a);

    a.write(|_| Ok(())).unwrap();
    assert_eq!(txn(&mut a), t0, "an empty window");
    let e = a
        .write(|db| db.remember(Memory::new("zero", MemoryType::Semantic, vec![0.0, 0.0])))
        .unwrap_err();
    assert!(!matches!(e, MnemoError::MaybeStored(_)), "{e}");
    assert_eq!(txn(&mut a), t0, "a refused write");
    let reloads = b.__reloads();
    b.read(|_| Ok(())).unwrap();
    assert_eq!(
        b.__reloads(),
        reloads,
        "nothing for another handle to reload"
    );

    remember(&mut a, "one").unwrap();
    assert_eq!(txn(&mut a), t0 + 2, "the window's lease, then its commit");

    // The first `set_embedder` records the identity (a write); one on a
    // file that already records it is a read.
    a.set_embedder(Fixed).unwrap();
    let t1 = txn(&mut a);
    assert!(t1 > t0 + 2);
    // A reader holds the file throughout: a read window gets in, a write
    // window would wait out its deadline.
    let reader = File::open(&p).unwrap();
    FileExt::try_lock_shared(&reader).unwrap();
    let mut c = open(&p, PW, opts(300)).unwrap();
    c.set_embedder(Fixed).unwrap();
    FileExt::unlock(&reader).unwrap();
    drop(reader);
    b.set_embedder(Fixed).unwrap();
    assert_eq!(txn(&mut b), t1);
    let reloads = a.__reloads();
    a.read(|_| Ok(())).unwrap();
    assert_eq!(a.__reloads(), reloads, "b's set_embedder wrote nothing");
    b.write(|db| db.remember_text("by text", MemoryType::Semantic))
        .unwrap();
    c.write(|db| db.remember_text("by text from c", MemoryType::Semantic))
        .unwrap();
    assert!(on_disk(&p, PW).contains(&"by text from c".to_string()));
}

// --- RB-007.7: the sidecar directory removed under a live handle --------

#[test]
fn a_removed_sidecar_is_reopened_and_writer_preference_holds() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, opts(300)) else {
        return;
    };
    let intent = intent_path(&p);
    let side = intent.parent().unwrap().to_path_buf();
    assert!(side.is_dir());
    std::fs::remove_dir_all(&side).unwrap();

    // A new handle makes the directory again; its intent lock is held.
    let b = open(&p, PW, opts(300)).unwrap();
    assert!(intent.is_file());
    let held = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&intent)
        .unwrap();
    FileExt::try_lock_exclusive(&held).unwrap();
    let e = remember(&mut a, "waits").unwrap_err();
    assert!(matches!(e, MnemoError::Locked { .. }), "{e}");
    FileExt::unlock(&held).unwrap();
    drop(held);
    remember(&mut a, "now").unwrap();
    assert!(a.notices().is_empty(), "{:?}", a.notices());
    drop(b);

    // Something that isn't a directory at the name: the handle says so,
    // once, and takes the main lock alone.
    std::fs::remove_dir_all(&side).unwrap();
    std::fs::write(&side, b"not a directory").unwrap();
    remember(&mut a, "alone").unwrap();
    remember(&mut a, "alone again").unwrap();
    let said = a
        .notices()
        .iter()
        .filter(|n| n.contains("not using the sidecar directory"))
        .count();
    assert_eq!(said, 1, "{:?}", a.notices());
}

// --- RB-007.8 -------------------------------------------------------------

/// Item 1: a deadline too large for the clock is clamped.
#[test]
fn a_deadline_too_large_for_the_clock_is_clamped() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let mut o = SharedOptions::default();
    o.deadline = Duration::MAX;
    let Some(mut a) = open(&p, PW, o) else { return };
    remember(&mut a, "fine").unwrap();
    assert_eq!(texts(&mut a), vec!["fine", "seed 0"]);
}

/// Item 2: a read that has to take the exclusive lock to replay a WAL
/// waits one deadline in all, not one per lock.
#[test]
fn a_read_that_heals_waits_one_deadline_in_all() {
    for n in 0..40 {
        let dir = tempdir().unwrap();
        let p = brain(dir.path(), 1);
        let Some(mut a) = open(&p, PW, opts(500)) else {
            return;
        };
        if !die_after_commit(&p, PW, "dying", n) {
            continue;
        }
        // A writer queues on the intent lock for 300 ms, and a reader
        // holds the file shared throughout, so the replay can't start.
        let intent = OpenOptions::new()
            .read(true)
            .write(true)
            .open(intent_path(&p))
            .unwrap();
        FileExt::try_lock_exclusive(&intent).unwrap();
        let reader = File::open(&p).unwrap();
        FileExt::try_lock_shared(&reader).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            FileExt::unlock(&intent).unwrap();
        });
        let t = Instant::now();
        let e = a.read(|db| db.memories()).unwrap_err();
        let waited = t.elapsed();
        releaser.join().unwrap();
        assert!(matches!(e, MnemoError::Locked { .. }), "{e}");
        // One 500 ms deadline; two would be 300 + 500.
        assert!(waited < Duration::from_millis(700), "waited {waited:?}");
        FileExt::unlock(&reader).unwrap();
        drop(reader);
        assert_eq!(texts(&mut a), vec!["dying", "seed 0"]);
        return;
    }
    panic!("no write left a pending WAL");
}

/// Item 3 and the replaced-file backoff (RB-007's case 10): a path that
/// keeps failing the identity check is retried at once the first time, then
/// with the backoff, without spinning, until the deadline; and the reader
/// doesn't sleep holding the intent lock.
#[test]
fn a_path_that_keeps_being_replaced_backs_off_without_the_intent_lock() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    if __shared::WINDOWS_OLD_RENAME {
        return;
    }
    let path = p.clone();
    let (started, start_rx) = std::sync::mpsc::channel();
    let caller = std::thread::spawn(move || {
        let mut a = Mnemo::open_shared(&path, PW, opts(400)).unwrap();
        // Replaced once: one immediate retry.
        set_shared_replaced(1);
        let before = shared_main_tries();
        a.read(|_| Ok(())).unwrap();
        let once = shared_main_tries() - before;
        // Replaced every time.
        set_shared_replaced(u64::MAX);
        started.send(()).unwrap();
        let before = shared_main_tries();
        let t = Instant::now();
        let r = a.read(|_| Ok(()));
        let waited = t.elapsed();
        let tries = shared_main_tries() - before;
        set_shared_replaced(0);
        (once, r.err(), waited, tries)
    });

    // While the caller backs off, a writer can take the intent lock.
    start_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(20));
    let start = Instant::now();
    let intent_file = intent_path(&p);
    let intent = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&intent_file)
        .unwrap();
    let mut free = 0;
    while start.elapsed() < Duration::from_millis(1500) && !caller.is_finished() {
        if FileExt::try_lock_exclusive(&intent).is_ok() {
            free += 1;
            FileExt::unlock(&intent).unwrap();
        }
        std::thread::sleep(Duration::from_millis(3));
    }
    let (once, e, waited, tries) = caller.join().unwrap();
    assert_eq!(once, 2, "the first replaced file is retried at once");
    let e = e.expect("the call fails at its deadline");
    assert!(matches!(e, MnemoError::Locked { .. }), "{e}");
    assert!(waited >= Duration::from_millis(390), "waited {waited:?}");
    assert!(waited < Duration::from_secs(3), "waited {waited:?}");
    // The backoff doubles from 1 ms to 50 ms: about a dozen tries in
    // 400 ms. A spin would be thousands.
    assert!((3..60).contains(&tries), "{tries} tries");
    assert!(free > 0, "the intent lock was never free");
}

/// Item 4: an embedder set inside a closure survives a reload.
#[test]
fn an_embedder_set_inside_a_closure_survives_a_reload() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    a.write(|db| db.set_embedder(Fixed)).unwrap();
    let mut b = open(&p, PW, SharedOptions::default()).unwrap();
    remember(&mut b, "from b").unwrap();
    let reloads = a.__reloads();
    a.write(|db| db.remember_text("by text", MemoryType::Semantic))
        .unwrap();
    assert!(a.__reloads() > reloads);
    assert!(on_disk(&p, PW).contains(&"by text".to_string()));
}

/// Item 5: a brain this process can only read.
#[cfg(unix)]
#[test]
fn a_read_only_brain_gives_clear_errors() {
    use std::os::unix::fs::PermissionsExt;
    let read_only = |p: &Path| {
        std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o400)).unwrap();
        OpenOptions::new().write(true).open(p).is_err()
    };
    let dir = tempdir().unwrap();
    let p = brain(&dir.path().join("plain"), 1);
    let Some(mut a) = open(&p, PW, opts(500)) else {
        return;
    };
    if !read_only(&p) {
        eprintln!("skipped: this process can write a 0400 file (root)");
        return;
    }
    assert_eq!(texts(&mut a), vec!["seed 0"]);
    let e = remember(&mut a, "refused").unwrap_err();
    match &e {
        MnemoError::Io(io) => {
            assert_eq!(io.kind(), std::io::ErrorKind::PermissionDenied, "{e}");
            assert!(io.to_string().contains("isn't writable"), "{e}");
        }
        other => panic!("{other}"),
    }

    // A pending WAL on a file this process can't write.
    for n in 0..40 {
        let p = brain(&dir.path().join(format!("wal{n}")), 1);
        if !die_after_commit(&p, PW, "dying", n) {
            continue;
        }
        assert!(read_only(&p));
        let e = Mnemo::open_shared(&p, PW, opts(500)).unwrap_err();
        assert!(matches!(e, MnemoError::NeedsWriteOpen { .. }), "{e}");
        return;
    }
    panic!("no write left a pending WAL");
}

/// Item 6: a panic caught in a read window makes the next call reload.
#[test]
fn a_panic_in_a_read_window_makes_the_next_call_reload() {
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let Some(mut a) = open(&p, PW, SharedOptions::default()) else {
        return;
    };
    let reloads = a.__reloads();
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = a.read(|_| -> Result<()> { panic!("a read panics") });
    }));
    assert!(caught.is_err());
    assert_eq!(texts(&mut a), vec!["seed 0"]);
    assert_eq!(a.__reloads(), reloads + 1);
}

// --- New files' modes, and the sidecar's ---------------------------------

#[cfg(unix)]
#[test]
fn new_brains_are_owner_only_and_compaction_keeps_the_mode() {
    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    assert_eq!(mode(&p), 0o600);

    // A brain made group-readable on purpose stays so.
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    assert_eq!(mode(&p), 0o640);

    // An export is a new file.
    let copy = dir.path().join("copy.mnemo");
    let mut db = Mnemo::open(&p, PW).unwrap();
    db.export_encrypted(&copy).unwrap();
    db.close().unwrap();
    assert_eq!(mode(&copy), 0o600);

    // An existing file's mode is never changed.
    let q = dir.path().join("existing.mnemo");
    std::fs::write(&q, b"").unwrap();
    std::fs::set_permissions(&q, std::fs::Permissions::from_mode(0o644)).unwrap();
    Mnemo::create(&q, PW, cfg()).unwrap().close().unwrap();
    assert_eq!(mode(&q), 0o644);
}

/// A sidecar directory whose owner bits a umask cleared (here 0500) is the
/// user's own and closed to others: it's given 0700 and used.
#[cfg(unix)]
#[test]
fn a_sidecar_without_full_owner_access_is_given_0700() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let p = brain(dir.path(), 1);
    let side = __shared::sidecar_path(&std::fs::canonicalize(&p).unwrap());
    std::fs::create_dir(&side).unwrap();
    std::fs::set_permissions(&side, std::fs::Permissions::from_mode(0o500)).unwrap();
    if __shared::WINDOWS_OLD_RENAME {
        return;
    }
    let l = Locks::new(&p, &opts(200)).unwrap();
    assert!(l.has_sidecar(), "{:?}", l.notices());
    assert_eq!(
        std::fs::metadata(&side).unwrap().permissions().mode() & 0o777,
        0o700
    );
}
