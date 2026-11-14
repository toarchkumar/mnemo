//! 12.3 step 1 (`docs/design/12.3-keys.md`, section 10): the salt rule, the
//! KDF rules, the recovery code, the rekey order with a key store, key
//! files and the key command. The CLI's chain, `init` and the display rule
//! are in `tests/cli_keys.rs`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use mnemo_engine::keys::{self, recovery, KeyStore, Secret, StoredSecret};
use mnemo_engine::{KdfParams, Memory, MemoryType, Mnemo, MnemoConfig, MnemoError, SharedOptions};
use tempfile::tempdir;
use zeroize::Zeroizing;

const PW: &str = "a passphrase for the keys tests";

fn cfg(kdf: KdfParams) -> MnemoConfig {
    MnemoConfig {
        dimensions: 2,
        kdf,
        ..Default::default()
    }
}

fn mem(i: usize) -> Memory {
    Memory::new(
        format!("fact {i}"),
        MemoryType::Semantic,
        vec![(i as f32) + 1.0, 1.0],
    )
}

fn salt(p: &Path) -> [u8; 16] {
    keys::file_info(p).unwrap().salt
}

/// Header bytes 102..162: the DEK nonce and the wrapped DEK.
fn wrap(p: &Path) -> Vec<u8> {
    std::fs::read(p).unwrap()[102..162].to_vec()
}

fn brain_with_key(p: &Path) -> [u8; 32] {
    let key = Mnemo::generate_key();
    let mut db = Mnemo::create_with_key(p, &key, cfg(KdfParams::raw_key())).unwrap();
    for i in 0..3 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
    key
}

fn brain_with_passphrase(p: &Path) {
    let mut db = Mnemo::create(p, PW, cfg(KdfParams::fast())).unwrap();
    for i in 0..3 {
        db.remember(mem(i)).unwrap();
    }
    db.close().unwrap();
}

// --- Test 1: a compaction keeps the salt and the KDF, with a new data key ---

#[test]
fn a_compaction_keeps_the_salt_and_kdf_and_writes_a_new_data_key() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let before = keys::file_info(&p).unwrap();
    let old_copy = dir.path().join("old.mnemo");
    std::fs::copy(&p, &old_copy).unwrap();
    let old_wrap = wrap(&p);

    Mnemo::compact_file_with_key(p.to_str().unwrap(), &key).unwrap();
    let after = keys::file_info(&p).unwrap();
    assert_eq!(after.salt, before.salt, "the salt is kept");
    assert_eq!(after.kdf, before.kdf, "the KDF parameters are kept");
    assert_ne!(wrap(&p), old_wrap, "a new data key under a new nonce");

    // Both open with the key.
    assert_eq!(Mnemo::open_with_key(&p, &key).unwrap().len(), 3);
    assert_eq!(Mnemo::open_with_key(&old_copy, &key).unwrap().len(), 3);

    // The old header on the new file unwraps the old data key, which
    // doesn't verify the new file's seal: the open fails.
    let mut new_bytes = std::fs::read(&p).unwrap();
    let old_bytes = std::fs::read(&old_copy).unwrap();
    new_bytes[..8192].copy_from_slice(&old_bytes[..8192]);
    let grafted = dir.path().join("grafted.mnemo");
    std::fs::write(&grafted, &new_bytes).unwrap();
    assert!(Mnemo::open_with_key(&grafted, &key).is_err());
}

#[test]
fn a_passphrase_compaction_keeps_the_salt_too() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("b.mnemo");
    brain_with_passphrase(&p);
    let s = salt(&p);
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    assert_eq!(salt(&p), s);
    assert_eq!(keys::file_info(&p).unwrap().kdf, KdfParams::fast());
}

// --- Test 2: a rekey draws a new salt; rekey then compact keeps it ---

#[test]
fn a_rekey_draws_a_new_salt_and_a_rotation_keeps_the_rekeys() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let s0 = salt(&p);
    let new_key = Mnemo::generate_key();
    let mut db = Mnemo::open_with_key(&p, &key).unwrap();
    db.rekey_with_key(&new_key).unwrap();
    db.close().unwrap();
    let s1 = salt(&p);
    assert_ne!(s1, s0, "a rekey draws a new salt");
    let w = wrap(&p);
    Mnemo::compact_file_with_key(p.to_str().unwrap(), &new_key).unwrap();
    assert_eq!(
        salt(&p),
        s1,
        "the rotation's compaction keeps the rekey's salt"
    );
    assert_ne!(wrap(&p), w, "and gives a new data key");
}

// --- Test 3: a v8 fixture compacted to v9 keeps its salt ---

#[test]
fn a_v8_fixture_compacted_to_v9_keeps_its_salt() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/v8");
    let dir = tempdir().unwrap();
    let p = dir.path().join("v8.mnemo");
    std::fs::copy(fixtures.join("v8_passphrase.mnemo"), &p).unwrap();
    let s = salt(&p);
    Mnemo::compact_file(p.to_str().unwrap(), "v8-fixture-passphrase").unwrap();
    assert_eq!(salt(&p), s);
}

// --- Test 4: a shared-mode compaction keeps the salt and is followed ---

#[test]
fn a_shared_compaction_keeps_the_salt_and_a_shared_handle_follows_it() {
    if mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        return;
    }
    let dir = tempdir().unwrap();
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let s = salt(&p);
    let mut shared = Mnemo::open_shared_with_key(&p, &key, SharedOptions::default()).unwrap();
    assert_eq!(shared.read(|db| Ok(db.len())).unwrap(), 3);
    Mnemo::compact_file_waiting_with_key(p.to_str().unwrap(), &key, SharedOptions::default())
        .unwrap();
    assert_eq!(salt(&p), s);
    assert_eq!(shared.read(|db| Ok(db.len())).unwrap(), 3);
}

// --- Test 5: the KDF rules ---

fn is_invalid(r: mnemo_engine::Result<impl Sized>) -> bool {
    matches!(r, Err(MnemoError::Invalid(_)))
}

#[test]
fn a_passphrase_is_refused_raw_key_parameters_on_every_path() {
    let dir = tempdir().unwrap();
    let raw = KdfParams::raw_key();

    // Mnemo::create, before anything is on disk.
    let p = dir.path().join("create.mnemo");
    assert!(is_invalid(Mnemo::create(&p, PW, cfg(raw))));
    assert!(!p.exists(), "a refused create leaves no file");

    // Mnemo::rekey with the file's own (raw-key) parameters.
    let p = dir.path().join("rekey.mnemo");
    let key = brain_with_key(&p);
    let mut db = Mnemo::open_with_key(&p, &key).unwrap();
    let kdf = db.kdf_params();
    assert!(is_invalid(db.rekey(PW, kdf)));
    // The parameter function lifts them to at least `secure()`.
    db.rekey(PW, kdf.for_passphrase()).unwrap();
    db.close().unwrap();
    let after = keys::file_info(&p).unwrap().kdf;
    assert!(after.m_cost >= KdfParams::secure().m_cost);
    assert!(after.t_cost >= KdfParams::secure().t_cost);

    // export_rekeyed.
    let mut db = Mnemo::open(&p, PW).unwrap();
    assert!(is_invalid(db.export_rekeyed(
        dir.path().join("x.mnemo"),
        PW,
        raw
    )));
    drop(db);

    // SharedMnemo::rekey, which calls the internal rekey directly.
    if !mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        let p = dir.path().join("shared.mnemo");
        let key = brain_with_key(&p);
        let mut s = Mnemo::open_shared_with_key(&p, &key, SharedOptions::default()).unwrap();
        assert!(is_invalid(s.rekey(PW, raw)));
        s.rekey(PW, KdfParams::raw_key().for_passphrase()).unwrap();
        drop(s);
        assert!(!keys::file_info(&p).unwrap().kdf.below_passphrase_floor());
    }

    // `fast()` is the named exception: tests use it.
    let p = dir.path().join("fast.mnemo");
    Mnemo::create(&p, PW, cfg(KdfParams::fast())).unwrap();
    assert!(KdfParams::fast().is_test_grade());
    assert!(!KdfParams::secure().is_test_grade());
}

#[test]
fn a_file_below_the_floor_is_reported_and_a_compaction_raises_it() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("low.mnemo");
    let mut db =
        Mnemo::__create_passphrase_below_floor_for_testing(&p, PW, cfg(KdfParams::fast())).unwrap();
    db.remember(mem(0)).unwrap();
    db.close().unwrap();
    let s = salt(&p);
    assert!(keys::file_info(&p).unwrap().kdf.below_passphrase_floor());

    // A passphrase open says so, with the fix; a read-only one too.
    let db = Mnemo::open(&p, PW).unwrap();
    assert_eq!(db.notices().len(), 1);
    assert!(
        db.notices()[0].contains("mnemo compact"),
        "{:?}",
        db.notices()
    );
    drop(db);
    assert_eq!(Mnemo::open_read_only(&p, PW).unwrap().notices().len(), 1);
    if !mnemo_engine::__shared::WINDOWS_OLD_RENAME {
        let s = Mnemo::open_shared(&p, PW, SharedOptions::default()).unwrap();
        assert!(
            s.notices().iter().any(|n| n.contains("raw-key KDF")),
            "{:?}",
            s.notices()
        );
    }
    assert!(keys::kdf_grade_note(
        keys::file_info(&p).unwrap().kdf,
        keys::SecretKind::Passphrase
    )
    .is_some());

    // A compaction given the passphrase raises the parameters and keeps the
    // salt; the notice is gone.
    Mnemo::compact_file(p.to_str().unwrap(), PW).unwrap();
    let info = keys::file_info(&p).unwrap();
    assert_eq!(info.salt, s);
    assert!(info.kdf.m_cost >= KdfParams::secure().m_cost);
    let db = Mnemo::open(&p, PW).unwrap();
    assert!(db.notices().is_empty());
    assert_eq!(db.len(), 1);
}

#[test]
fn a_key_open_of_a_key_file_has_no_notice() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("k.mnemo");
    let key = brain_with_key(&p);
    assert!(Mnemo::open_with_key(&p, &key).unwrap().notices().is_empty());
}

// --- Test 6: the recovery code ---

#[test]
fn recovery_codes_round_trip() {
    for _ in 0..200 {
        let key = Mnemo::generate_key();
        let code = recovery::encode(&key);
        assert_eq!(code.len(), 3 + 13 * 6);
        assert_eq!(*recovery::parse(&code).unwrap(), key);
        // Lower case, no hyphens, spaces: the same key.
        let loose: String = code.to_lowercase().replace('-', " ");
        assert_eq!(*recovery::parse(&loose).unwrap(), key);
    }
}

const SYMS: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn groups(code: &str) -> Vec<String> {
    code.split('-').skip(1).map(str::to_string).collect()
}

fn join(gs: &[String]) -> String {
    format!("MN1-{}", gs.join("-"))
}

#[test]
fn every_single_substitution_and_swap_is_caught_and_named() {
    let key = Mnemo::generate_key();
    let code = recovery::encode(&key);
    let gs = groups(&code);
    for (g, group) in gs.iter().enumerate() {
        let chars: Vec<char> = group.chars().collect();
        for pos in 0..5 {
            for c in SYMS.chars().filter(|&c| c != chars[pos]) {
                let mut changed = chars.clone();
                changed[pos] = c;
                let mut gs2 = gs.clone();
                gs2[g] = changed.iter().collect();
                let e = recovery::parse(&join(&gs2)).unwrap_err().to_string();
                assert!(e.contains(&format!("group {}", g + 1)), "{e}");
            }
            for q in pos + 1..5 {
                if chars[pos] == chars[q] {
                    continue;
                }
                let mut swapped = chars.clone();
                swapped.swap(pos, q);
                let mut gs2 = gs.clone();
                gs2[g] = swapped.iter().collect();
                let e = recovery::parse(&join(&gs2)).unwrap_err().to_string();
                assert!(e.contains(&format!("group {}", g + 1)), "{e}");
            }
        }
    }
    // A group in another group's place.
    for a in 0..13 {
        for b in a + 1..13 {
            if gs[a] == gs[b] {
                continue;
            }
            let mut gs2 = gs.clone();
            gs2.swap(a, b);
            assert!(recovery::parse(&join(&gs2)).is_err());
        }
    }
}

#[test]
fn the_parser_reads_aliases_and_refuses_what_it_should() {
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let code = recovery::encode(&key);
    // `O` for `0`, `I` and `L` for `1`, in the prefix too.
    let aliased = code.replacen('0', "O", 3).replacen("MN1", "mni", 1);
    assert_eq!(*recovery::parse(&aliased).unwrap(), key);
    assert_eq!(
        *recovery::parse(&code.replacen("MN1", "MNl", 1)).unwrap(),
        key
    );
    // `U` isn't in the alphabet.
    let gs = groups(&code);
    let mut bad = gs.clone();
    bad[2].replace_range(0..1, "U");
    assert!(recovery::parse(&join(&bad))
        .unwrap_err()
        .to_string()
        .contains("group 3"));
    // A wrong prefix.
    assert!(recovery::parse(&code.replacen("MN1", "MN2", 1)).is_err());
    // A last symbol other than 0 or G: the check is recomputed by changing
    // the bit and is still refused.
    let mut last = gs.clone();
    last[12].replace_range(3..4, "1");
    assert!(recovery::parse(&join(&last))
        .unwrap_err()
        .to_string()
        .contains("group 13"));
}

#[test]
fn a_wrong_length_names_the_group() {
    let key = Mnemo::generate_key();
    let code = recovery::encode(&key);
    let gs = groups(&code);
    // With hyphens and group 4 one short, group 4 is named.
    let mut short = gs.clone();
    short[3].pop();
    let e = recovery::parse(&join(&short)).unwrap_err().to_string();
    assert!(e.contains("group 4 has 4"), "{e}");
    // Without hyphens: "group N or the one before it".
    let flat: String = join(&short).replace('-', "");
    let e = recovery::parse(&flat).unwrap_err().to_string();
    assert!(e.contains("64 characters"), "{e}");
    assert!(
        e.contains("or the one before it") || e.contains("group 1"),
        "{e}"
    );
    // A character dropped from the last group: the last group is named.
    let mut last = gs.clone();
    last[12].pop();
    let flat: String = join(&last).replace('-', "");
    let e = recovery::parse(&flat).unwrap_err().to_string();
    assert!(e.contains("the last group (13)"), "{e}");
}

// --- Test 13: the rekey order with a key store ---

/// A key store in memory, which can be told to give back something else.
#[derive(Default)]
struct MemStore {
    entries: RefCell<HashMap<[u8; 16], String>>,
    corrupt_reads: std::cell::Cell<bool>,
}

impl KeyStore for MemStore {
    fn describe(&self) -> String {
        "the test's memory store".into()
    }
    fn get(&self, salt: &[u8; 16]) -> mnemo_engine::Result<Option<StoredSecret>> {
        if self.corrupt_reads.get() {
            return Ok(Some(StoredSecret::Passphrase(Zeroizing::new(
                "other".into(),
            ))));
        }
        self.entries
            .borrow()
            .get(salt)
            .map(|t| StoredSecret::decode(t))
            .transpose()
    }
    fn put(&self, salt: &[u8; 16], s: &StoredSecret, _: &str) -> mnemo_engine::Result<()> {
        self.entries
            .borrow_mut()
            .insert(*salt, s.encode().to_string());
        Ok(())
    }
    fn delete(&self, salt: &[u8; 16]) -> mnemo_engine::Result<()> {
        self.entries.borrow_mut().remove(salt);
        Ok(())
    }
}

fn stored_key(store: &MemStore, salt: &[u8; 16]) -> Option<[u8; 32]> {
    match store.get(salt).unwrap()? {
        StoredSecret::Key(k) => Some(*k),
        _ => None,
    }
}

#[test]
fn a_stored_rekey_keeps_both_entries_and_the_new_one_opens() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let store = MemStore::default();
    let s0 = salt(&p);
    store
        .put(&s0, &StoredSecret::Key(Zeroizing::new(key)), "b")
        .unwrap();
    let new = Secret::Key(Zeroizing::new(Mnemo::generate_key()));
    let mut shown = 0;
    let mut db = Mnemo::open_with_key(&p, &key).unwrap();
    keys::rekey_stored(&mut db, &new, &store, "b", &mut || {
        shown += 1;
        Ok(())
    })
    .unwrap();
    db.close().unwrap();
    assert_eq!(shown, 1, "the code is shown once, before the commit");
    let s1 = salt(&p);
    assert_ne!(s1, s0);
    assert_eq!(store.entries.borrow().len(), 2, "the old entry is kept");
    let k1 = stored_key(&store, &s1).unwrap();
    assert_eq!(Mnemo::open_with_key(&p, &k1).unwrap().len(), 3);
}

#[test]
fn a_read_back_that_doesnt_match_stops_before_the_commit() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let store = MemStore::default();
    store.corrupt_reads.set(true);
    let new = Secret::Key(Zeroizing::new(Mnemo::generate_key()));
    // Read before the open: Windows' lock refuses a second reader.
    let s0 = salt(&p);
    let mut db = Mnemo::open_with_key(&p, &key).unwrap();
    let e = keys::rekey_stored(&mut db, &new, &store, "b", &mut || Ok(())).unwrap_err();
    assert!(e.to_string().contains("didn't give back"), "{e}");
    drop(db);
    assert_eq!(salt(&p), s0, "nothing was committed");
    assert!(store.entries.borrow().is_empty(), "the entry was removed");
    assert_eq!(Mnemo::open_with_key(&p, &key).unwrap().len(), 3);
}

/// The number of the write that puts the new page 0 on disk in a key-to-key
/// rekey of a 3-memory file (as `tests/rekey_cut.rs` finds it).
#[cfg(feature = "failpoints")]
fn page0_write(dir: &Path) -> u64 {
    for n in 0..16 {
        let probe = dir.join(format!("probe-{n}.mnemo"));
        let key = brain_with_key(&probe);
        let mut db = Mnemo::open_with_key(&probe, &key).unwrap();
        mnemo_engine::__failpoints::set_writes_until_fail(n);
        let r = db.rekey_with_key(&Mnemo::generate_key());
        mnemo_engine::__failpoints::set_writes_until_fail(-1);
        if r.is_ok() {
            assert!(n >= 1);
            return (n - 1) as u64;
        }
    }
    panic!("no cut let the rekey succeed");
}

#[cfg(feature = "failpoints")]
#[test]
fn a_rekey_that_fails_before_page_0_removes_the_new_entry() {
    let dir = tempdir().unwrap();
    let w = page0_write(dir.path());
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let s0 = salt(&p);
    let store = MemStore::default();
    store
        .put(&s0, &StoredSecret::Key(Zeroizing::new(key)), "b")
        .unwrap();
    let new = Secret::Key(Zeroizing::new(Mnemo::generate_key()));
    let mut db = Mnemo::open_with_key(&p, &key).unwrap();
    mnemo_engine::__failpoints::set_writes_failing(&[w]);
    let e = keys::rekey_stored(&mut db, &new, &store, "b", &mut || Ok(())).unwrap_err();
    mnemo_engine::__failpoints::set_writes_failing(&[]);
    assert!(e.entries.contains("new entry was removed"), "{e}");
    drop(db);
    assert_eq!(salt(&p), s0);
    assert_eq!(store.entries.borrow().len(), 1);
    assert_eq!(Mnemo::open_with_key(&p, &key).unwrap().len(), 3);
}

#[cfg(feature = "failpoints")]
#[test]
fn a_failed_write_back_after_page_0_keeps_both_entries() {
    let dir = tempdir().unwrap();
    let w = page0_write(dir.path());
    let p = dir.path().join("b.mnemo");
    let key = brain_with_key(&p);
    let s0 = salt(&p);
    let store = MemStore::default();
    store
        .put(&s0, &StoredSecret::Key(Zeroizing::new(key)), "b")
        .unwrap();
    let new_key = Mnemo::generate_key();
    let new = Secret::Key(Zeroizing::new(new_key));
    let mut db = Mnemo::open_with_key(&p, &key).unwrap();
    // Page 0's sync fails, then writing the old one back fails too: REKEY-1's
    // "may open with either secret".
    mnemo_engine::__failpoints::set_writes_failing(&[w, w + 2]);
    let e = keys::rekey_stored(&mut db, &new, &store, "b", &mut || Ok(())).unwrap_err();
    mnemo_engine::__failpoints::set_writes_failing(&[]);
    let msg = e.to_string();
    assert!(msg.contains("may open with either secret"), "{msg}");
    assert!(e.entries.contains("both"), "{msg}");
    drop(db);
    assert_eq!(store.entries.borrow().len(), 2, "both entries are kept");
    // Whichever secret is in force, the store has it.
    let s = salt(&p);
    let k = stored_key(&store, &s).expect("an entry for the salt on disk");
    assert!(Mnemo::open_with_key(&p, &k).is_ok());
}

// --- Test 11: key files ---

#[cfg(unix)]
#[test]
fn a_key_file_open_to_others_is_refused_and_an_owner_only_one_is_read() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let f = dir.path().join("k.key");
    let key = Mnemo::generate_key();
    std::fs::write(&f, keys::hex(&key)).unwrap();
    std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
    let e = keys::read_key_file(&f).unwrap_err().to_string();
    assert!(e.contains("chmod 600"), "{e}");
    for mode in [0o600, 0o400] {
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(mode)).unwrap();
        assert_eq!(*keys::read_key_file(&f).unwrap(), key);
    }
    // A recovery code in a key file is read too.
    let g = dir.path().join("code.key");
    keys::write_new_key_file(&g, &key).unwrap();
    assert_eq!(
        std::fs::metadata(&g).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(
        keys::write_new_key_file(&g, &key).is_err(),
        "never over a name"
    );
    std::fs::write(&g, recovery::encode(&key).as_str()).unwrap();
    assert_eq!(*keys::read_key_file(&g).unwrap(), key);
}

// --- Test 14: the key command ---

#[cfg(unix)]
#[test]
fn the_key_command_fails_each_way_with_its_own_message() {
    use std::time::Duration;
    let t = Duration::from_secs(5);
    let key = Mnemo::generate_key();
    let ok = format!("echo {}", keys::hex(&key));
    assert_eq!(*keys::run_key_command(&ok, t).unwrap(), key);
    let code = format!("echo {}", recovery::encode(&key).as_str());
    assert_eq!(*keys::run_key_command(&code, t).unwrap(), key);

    let e = keys::run_key_command("sh -c 'exit 3'", t)
        .unwrap_err()
        .to_string();
    assert!(e.contains("failed"), "{e}");
    let e = keys::run_key_command("true", t).unwrap_err().to_string();
    assert!(e.contains("printed nothing"), "{e}");
    let e = keys::run_key_command("yes", t).unwrap_err().to_string();
    assert!(e.contains("more than 4 KiB"), "{e}");
    let e = keys::run_key_command("sleep 5", Duration::from_millis(300))
        .unwrap_err()
        .to_string();
    assert!(e.contains("didn't answer"), "{e}");
    let e = keys::run_key_command("echo not-a-key", t)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("neither 64 hex digits nor a recovery code"),
        "{e}"
    );
}

// --- The display rule's marker check ---

#[test]
fn an_agent_marker_refuses_the_display() {
    let set = |k: &str| (k == "CLAUDECODE").then(|| std::ffi::OsString::from("1"));
    assert!(keys::display::marker_reason(set)
        .unwrap()
        .contains("CLAUDECODE"));
    let empty = |k: &str| (k == "CLAUDECODE").then(std::ffi::OsString::new);
    assert!(
        keys::display::marker_reason(empty).is_none(),
        "an empty value isn't set"
    );
    assert!(keys::display::marker_reason(|_| None).is_none());
}
