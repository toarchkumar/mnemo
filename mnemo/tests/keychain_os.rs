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

/// Step 5a (MSRV-2): an entry the `keyring` 3.6.3 build wrote reads under
/// the `keyring` 4 stores. `MNEMO_KEYRING3_BIN` names a CLI built at
/// `824ff51` (the last commit on 3.6.3); CI's macOS job builds it. The old
/// CLI makes a brain with a generated key, kept in this user's keychain;
/// this build must find that key, open the brain with it, and remove it.
#[test]
fn an_entry_the_keyring_3_build_wrote_reads_under_keyring_4() {
    if !wanted() {
        eprintln!("MNEMO_TEST_OS_KEYCHAIN isn't 1; the OS keychain isn't touched");
        return;
    }
    let Some(old) = std::env::var_os("MNEMO_KEYRING3_BIN") else {
        eprintln!("MNEMO_KEYRING3_BIN isn't set; skipped");
        return;
    };
    let store = keys::default_store().expect("a keychain build has a store");
    if let Err(e) = store.check_available() {
        eprintln!("no keychain here ({e}); skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let b = dir.path().join("kr3.mnemo");
    let o = Command::new(&old)
        .args(["init", "--dimensions", "4", b.to_str().unwrap()])
        .env_remove("MNEMO_PASSPHRASE")
        .env_remove("MNEMO_KEY")
        .env_remove("MNEMO_KEY_FILE")
        .env_remove("MNEMO_KEY_COMMAND")
        .env_remove("MNEMO_DEV_KEYSTORE")
        .env_remove("MNEMO_DEV_NO_KEYCHAIN")
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_CHILD_SESSION")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "the 3.6.3 build's init failed: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let salt = keys::file_info(&b).unwrap().salt;
    let found = store.get(&salt);
    // Remove the entry before asserting, so a failure leaves nothing behind.
    let _ = store.delete(&salt);
    match found.unwrap() {
        Some(StoredSecret::Key(k)) => {
            let db = Mnemo::open_with_key(&b, &k).unwrap();
            assert_eq!(db.dimensions(), 4);
        }
        Some(_) => panic!("the 3.6.3 build stored a passphrase, not a key"),
        None => panic!("the 3.6.3 build's entry wasn't found under keyring 4"),
    }
    assert!(store.get(&salt).unwrap().is_none());
}
