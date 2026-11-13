//! REKEY-1 (RB-004.2, v0.13.2): a rekey cut off at every write it makes.
//!
//! After `Err`, the old secret is in force for the file, and stays so after
//! a drop, a close, or another write and a close; the new one opens
//! nothing. After `Ok`, the new secret is in force and the old one opens
//! nothing, even when part of the cleanup was cut off (which the handle
//! reports, instead of failing the rekey).
//!
//! The failpoint counter is thread-local, so each test arms its own.

use std::path::Path;

use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError};
use tempfile::tempdir;

fn cfg() -> MnemoConfig {
    MnemoConfig {
        dimensions: 4,
        kdf: KdfParams::fast(),
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    Memory::new(
        format!("memory {i}"),
        MemoryType::Semantic,
        vec![1.0 + i as f32, (i % 3) as f32, 0.5, 1.0],
    )
}

#[derive(Clone, Copy, Debug)]
enum Then {
    Drop,
    Close,
    WriteAndClose,
}

#[derive(Clone, Copy, Debug)]
enum Secret {
    Passphrase,
    Key,
}

const OLD_PW: &str = "old-pw";
const NEW_PW: &str = "new-pw";
const OLD_KEY: [u8; 32] = [7u8; 32];
const NEW_KEY: [u8; 32] = [9u8; 32];

fn create(path: &Path, s: Secret) -> Mnemo {
    match s {
        Secret::Passphrase => Mnemo::create(path, OLD_PW, cfg()).unwrap(),
        Secret::Key => Mnemo::create_with_key(path, &OLD_KEY, cfg()).unwrap(),
    }
}

fn rekey(db: &mut Mnemo, s: Secret) -> mnemo_engine::Result<()> {
    match s {
        Secret::Passphrase => db.rekey(NEW_PW, KdfParams::fast()),
        Secret::Key => db.rekey_with_key(&NEW_KEY),
    }
}

fn open(path: &Path, s: Secret, new: bool) -> mnemo_engine::Result<Mnemo> {
    match (s, new) {
        (Secret::Passphrase, false) => Mnemo::open(path, OLD_PW),
        (Secret::Passphrase, true) => Mnemo::open(path, NEW_PW),
        (Secret::Key, false) => Mnemo::open_with_key(path, &OLD_KEY),
        (Secret::Key, true) => Mnemo::open_with_key(path, &NEW_KEY),
    }
}

fn open_ro(path: &Path, s: Secret, new: bool) -> mnemo_engine::Result<Mnemo> {
    match (s, new) {
        (Secret::Passphrase, false) => Mnemo::open_read_only(path, OLD_PW),
        (Secret::Passphrase, true) => Mnemo::open_read_only(path, NEW_PW),
        (Secret::Key, false) => Mnemo::open_read_only_with_key(path, &OLD_KEY),
        (Secret::Key, true) => Mnemo::open_read_only_with_key(path, &NEW_KEY),
    }
}

/// What a rekey cut off at the `n`-th write returned.
#[derive(Debug, PartialEq)]
enum Outcome {
    Err,
    OkWithCleanupLeft,
    Ok,
}

/// Rekey a 3-memory file with its `n`-th write failing, continue as `then`
/// says, and check which secret opens the file. Returns what the rekey did.
fn cut_at(dir: &Path, s: Secret, n: i64, then: Then) -> Outcome {
    let path = dir.join(format!("{s:?}-{n}-{then:?}.mnemo"));
    let mut db = create(&path, s);
    for i in 0..3 {
        db.remember(mem(i)).unwrap();
    }
    db.flush().unwrap();

    mnemo_engine::__failpoints::set_writes_until_fail(n);
    let result = rekey(&mut db, s);
    mnemo_engine::__failpoints::set_writes_until_fail(-1);
    let outcome = match (&result, db.rekey_cleanup_error()) {
        (Err(_), _) => Outcome::Err,
        (Ok(()), Some(_)) => Outcome::OkWithCleanupLeft,
        (Ok(()), None) => Outcome::Ok,
    };
    let here = format!("{s:?}, write {n}, then {then:?}: {result:?}");

    let mut expect = 3;
    match then {
        Then::Drop => drop(db),
        Then::Close => db.close().unwrap(),
        Then::WriteAndClose => {
            db.remember(mem(3)).unwrap();
            db.close().unwrap();
            expect = 4;
        }
    }

    let in_force = outcome != Outcome::Err;
    let db = open(&path, s, in_force).unwrap_or_else(|e| panic!("{here}: the right secret: {e}"));
    assert_eq!(db.len(), expect, "{here}");
    db.close().unwrap();
    let ro = open_ro(&path, s, in_force).unwrap_or_else(|e| panic!("{here}: read-only: {e}"));
    assert_eq!(ro.len(), expect, "{here}");
    drop(ro);
    assert!(
        matches!(open(&path, s, !in_force), Err(MnemoError::WrongPassphrase)),
        "{here}: the other secret opened the file"
    );

    // After `Err`, the new region's commit was zeroed, so even a torn page
    // 0 gives the new secret nothing to recover with.
    if outcome == Outcome::Err {
        let torn = dir.join(format!("{s:?}-{n}-{then:?}-torn.mnemo"));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..16].fill(0);
        std::fs::write(&torn, &bytes).unwrap();
        assert!(
            open(&torn, s, true).is_err(),
            "{here}: the new secret recovered a torn header"
        );
    }
    outcome
}

fn every_cut_point(s: Secret) {
    let dir = tempdir().unwrap();
    for then in [Then::Drop, Then::Close, Then::WriteAndClose] {
        let mut seen = Vec::new();
        for n in 0..8 {
            seen.push(cut_at(dir.path(), s, n, then));
        }
        eprintln!("{s:?}, then {then:?}: {seen:?}");
        // The cut points run in order: `Err` while the new page 0 isn't
        // on disk, then `Ok` with part of the cleanup left, then `Ok`.
        let first_ok = seen.iter().position(|o| *o != Outcome::Err).unwrap();
        let first_clean = seen.iter().position(|o| *o == Outcome::Ok).unwrap();
        assert!(first_ok >= 1, "{s:?} {then:?}: {seen:?}");
        assert!(first_clean > first_ok, "{s:?} {then:?}: {seen:?}");
        assert!(
            seen[..first_ok].iter().all(|o| *o == Outcome::Err)
                && seen[first_ok..first_clean]
                    .iter()
                    .all(|o| *o == Outcome::OkWithCleanupLeft)
                && seen[first_clean..].iter().all(|o| *o == Outcome::Ok),
            "{s:?} {then:?}: {seen:?}"
        );
    }
}

#[cfg(feature = "failpoints")]
#[test]
fn a_cut_off_passphrase_rekey_leaves_exactly_one_secret_in_force() {
    every_cut_point(Secret::Passphrase);
}

#[cfg(feature = "failpoints")]
#[test]
fn a_cut_off_raw_key_rekey_leaves_exactly_one_secret_in_force() {
    every_cut_point(Secret::Key);
}

/// The read path on v8: a v8 file opens read-only, so a rekey is refused
/// before it writes anything, and the file still opens with its secret.
#[test]
fn a_v8_file_refuses_a_rekey_and_is_unchanged() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8");
    let dir = tempdir().unwrap();
    let path = dir.path().join("k.mnemo");
    std::fs::copy(fixtures.join("v8_key.mnemo"), &path).unwrap();
    let raw = std::fs::read_to_string(fixtures.join("v8_key.key")).unwrap();
    let key: [u8; 32] =
        std::array::from_fn(|i| u8::from_str_radix(&raw.trim()[2 * i..2 * i + 2], 16).unwrap());
    let before = std::fs::read(&path).unwrap();

    let mut db = Mnemo::open_read_only_with_key(&path, &key).unwrap();
    assert!(matches!(
        db.rekey_with_key(&NEW_KEY),
        Err(MnemoError::ReadOnly)
    ));
    assert!(db.rekey_cleanup_error().is_none());
    drop(db);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(Mnemo::open_read_only_with_key(&path, &key).is_ok());
}

// --- REKEY-2 (RB-005.2, RB-005.4 item 5): failures inside the undo --------

/// A 3-memory file, flushed, and the number of the write that puts the new
/// page 0 on disk: failing it is the last cut that leaves the old secret.
#[cfg(feature = "failpoints")]
fn page0_write(dir: &Path, s: Secret) -> u64 {
    for n in 0..16 {
        let probe = dir.join(format!("probe-{s:?}-{n}.mnemo"));
        let mut db = create(&probe, s);
        for i in 0..3 {
            db.remember(mem(i)).unwrap();
        }
        db.flush().unwrap();
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let r = rekey(&mut db, s);
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        if r.is_ok() {
            assert!(n >= 1);
            return (n - 1) as u64;
        }
    }
    panic!("no cut let the rekey succeed");
}

#[cfg(feature = "failpoints")]
fn fresh(dir: &Path, s: Secret, name: &str) -> (std::path::PathBuf, Mnemo) {
    let path = dir.join(name);
    let mut db = create(&path, s);
    for i in 0..3 {
        db.remember(mem(i)).unwrap();
    }
    db.flush().unwrap();
    (path, db)
}

/// Page 0's write fails, then the zeroing of the new region fails, then the
/// old page 0 is written back. The old secret is in force, and the error
/// says a copy of the new key wrap may be left until a later write clears
/// it, which it does.
#[cfg(feature = "failpoints")]
#[test]
fn a_failed_zeroing_in_the_undo_is_reported() {
    let dir = tempdir().unwrap();
    for s in [Secret::Passphrase, Secret::Key] {
        let p = page0_write(dir.path(), s);
        let (path, mut db) = fresh(dir.path(), s, &format!("zeroing-{s:?}.mnemo"));
        // p: the new page 0; p + 1: the zeroing; p + 2: the write-back.
        mnemo_engine::__failpoints::set_writes_failing(&[p, p + 1]);
        let e = rekey(&mut db, s).unwrap_err();
        mnemo_engine::__failpoints::set_writes_failing(&[]);
        let msg = e.to_string();
        assert!(
            msg.contains("zeroing the new key wrap's copy failed"),
            "{s:?}: {msg}"
        );
        assert!(
            msg.contains("the old secret is still in force"),
            "{s:?}: {msg}"
        );
        assert!(!msg.contains("may open with either secret"), "{s:?}: {msg}");
        // The copy is really there: a torn header taken now lets the new
        // secret recover the new page 0.
        let early = dir.path().join(format!("zeroing-{s:?}-early.mnemo"));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..16].fill(0);
        std::fs::write(&early, &bytes).unwrap();
        assert!(open(&early, s, true).is_ok(), "{s:?}: no copy was left");
        db.remember(mem(3)).unwrap();
        db.close().unwrap();
        assert_eq!(open(&path, s, false).unwrap().len(), 4, "{s:?}");
        assert!(matches!(
            open(&path, s, true),
            Err(MnemoError::WrongPassphrase)
        ));
        // After the later write, a torn header gives the new secret
        // nothing to recover with.
        let torn = dir.path().join(format!("zeroing-{s:?}-torn.mnemo"));
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..16].fill(0);
        std::fs::write(&torn, &bytes).unwrap();
        assert!(open(&torn, s, true).is_err(), "{s:?}: the copy was left");
    }
}

/// Page 0's write fails and the zeroing works, but writing the old page 0
/// back fails too: the file may open with either secret, and the message
/// says so (the CLI keys on the phrase).
#[cfg(feature = "failpoints")]
#[test]
fn a_failed_write_back_says_the_file_may_open_with_either_secret() {
    let dir = tempdir().unwrap();
    for s in [Secret::Passphrase, Secret::Key] {
        let p = page0_write(dir.path(), s);
        let (path, mut db) = fresh(dir.path(), s, &format!("writeback-{s:?}.mnemo"));
        mnemo_engine::__failpoints::set_writes_failing(&[p, p + 2]);
        let e = rekey(&mut db, s).unwrap_err();
        mnemo_engine::__failpoints::set_writes_failing(&[]);
        let msg = e.to_string();
        assert!(msg.contains("may open with either secret"), "{s:?}: {msg}");
        assert!(!msg.contains("zeroing"), "{s:?}: {msg}");
        drop(db);
        let old = open(&path, s, false).is_ok();
        let new = open(&path, s, true).is_ok();
        assert!(old || new, "{s:?}: neither secret opens the file");
    }
}

/// The failed zeroing through `SharedMnemo::rekey`: the same message, and
/// the handle keeps the old secret (a compaction under it is followed).
#[cfg(feature = "failpoints")]
#[test]
fn a_failed_zeroing_through_a_shared_handle_keeps_the_old_secret() {
    if mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        return;
    }
    let dir = tempdir().unwrap();
    let p = page0_write(dir.path(), Secret::Passphrase);
    let (path, db) = fresh(dir.path(), Secret::Passphrase, "shared.mnemo");
    db.close().unwrap();
    let mut s = Mnemo::open_shared(&path, OLD_PW, mnemo_engine::SharedOptions::default()).unwrap();
    mnemo_engine::__failpoints::set_writes_failing(&[p, p + 1]);
    let e = s.rekey(NEW_PW, KdfParams::fast()).unwrap_err();
    mnemo_engine::__failpoints::set_writes_failing(&[]);
    assert!(
        e.to_string()
            .contains("zeroing the new key wrap's copy failed"),
        "{e}"
    );
    Mnemo::compact_file(path.to_str().unwrap(), OLD_PW).unwrap();
    assert_eq!(s.read(|db| Ok(db.len())).unwrap(), 3);
}
