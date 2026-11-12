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
