//! 12.3 step 2: the OS keychain's round trip, with the `keychain` feature.
//!
//! It writes to the real keychain of the user running it, so it runs only
//! when `MNEMO_TEST_OS_KEYCHAIN=1` is set (CI's macOS, Windows and Linux
//! jobs set it); anywhere else it returns at once. On a machine with no
//! keychain (Linux CI has no Secret Service on a session bus) it checks
//! that "no keychain" is reported, and that `init` then fails, leads with
//! `--ask-passphrase` and creates nothing.
#![cfg(feature = "keychain")]

use std::process::{Command, Stdio};

use mnemo_engine::keys::{self, StoredSecret};
use mnemo_engine::Mnemo;
use zeroize::Zeroizing;

fn wanted() -> bool {
    std::env::var_os("MNEMO_TEST_OS_KEYCHAIN").is_some_and(|v| v == "1")
        && std::env::var_os("MNEMO_DEV_KEYSTORE").is_none()
        && std::env::var_os("MNEMO_DEV_NO_KEYCHAIN").is_none()
}

#[test]
fn the_os_keychain_keeps_and_gives_back_an_entry_or_says_it_isnt_there() {
    if !wanted() {
        eprintln!("MNEMO_TEST_OS_KEYCHAIN isn't 1; the OS keychain isn't touched");
        return;
    }
    let store = keys::default_store().expect("a keychain build has a store");
    eprintln!("store: {}", store.describe());
    match store.check_available() {
        Ok(()) => {
            // A salt no brain has, so the test never touches a real entry.
            let mut salt = [0u8; 16];
            salt.copy_from_slice(&Mnemo::generate_key()[..16]);
            let key = Mnemo::generate_key();
            assert!(store.get(&salt).unwrap().is_none());
            keys::store_and_check(
                &*store,
                &salt,
                &StoredSecret::Key(Zeroizing::new(key)),
                "test",
            )
            .unwrap();
            match store.get(&salt).unwrap() {
                Some(StoredSecret::Key(k)) => assert_eq!(*k, key),
                _ => panic!("the key didn't come back"),
            }
            store.delete(&salt).unwrap();
            assert!(store.get(&salt).unwrap().is_none());
            assert!(!store.durable(), "no OS store promises a durable write");
        }
        Err(e) => {
            eprintln!("no keychain here: {e}");
            let dir = tempfile::tempdir().unwrap();
            let b = dir.path().join("b.mnemo");
            let o = Command::new(env!("CARGO_BIN_EXE_mnemo"))
                .args(["init", "--dimensions", "4", b.to_str().unwrap()])
                .env_remove("MNEMO_PASSPHRASE")
                .env_remove("MNEMO_DEV_KEYSTORE")
                .env_remove("MNEMO_DEV_NO_KEYCHAIN")
                .stdin(Stdio::null())
                .output()
                .unwrap();
            let err = String::from_utf8_lossy(&o.stderr);
            assert!(!o.status.success());
            assert!(err.contains("--ask-passphrase"), "{err}");
            assert!(!b.exists());
        }
    }
}

/// Step 5a (MSRV-2): an entry written through `keyring` 3.6.3 (the
/// keychain's crate before the `keyring` 4 stores) reads through the new
/// store, so a key an earlier build kept is still found. One test binary
/// writes and reads, so macOS asks no second binary for access. macOS and
/// Windows (Linux CI has no Secret Service); a salt no brain has.
#[cfg(any(target_os = "macos", windows))]
#[test]
fn an_entry_keyring_3_wrote_reads_under_the_keyring_4_store() {
    if !wanted() {
        eprintln!("MNEMO_TEST_OS_KEYCHAIN isn't 1; the OS keychain isn't touched");
        return;
    }
    let store = keys::default_store().expect("a keychain build has a store");
    if let Err(e) = store.check_available() {
        eprintln!("no keychain here ({e}); skipped");
        return;
    }
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&Mnemo::generate_key()[..16]);
    let key = Mnemo::generate_key();
    let value = StoredSecret::Key(Zeroizing::new(key)).encode();
    let old = keyring::Entry::new(keys::KEYCHAIN_SERVICE, &keys::hex(&salt)).unwrap();
    old.set_password(&value).unwrap();
    let found = store.get(&salt);
    // Remove the entry before asserting, so a failure leaves nothing behind.
    let _ = old.delete_credential();
    match found.unwrap() {
        Some(StoredSecret::Key(k)) => assert_eq!(*k, key),
        Some(_) => panic!("the entry came back as a passphrase"),
        None => panic!("the keyring 3.6.3 entry wasn't found by the keyring 4 store"),
    }
    // And the other way: the new store's entry reads through 3.6.3.
    keys::store_and_check(
        &*store,
        &salt,
        &StoredSecret::Key(Zeroizing::new(key)),
        "test",
    )
    .unwrap();
    let back = old.get_password();
    store.delete(&salt).unwrap();
    assert_eq!(*value, back.unwrap());
    assert!(store.get(&salt).unwrap().is_none());
}
