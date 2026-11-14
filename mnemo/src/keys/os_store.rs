//! 12.3 step 2: the OS keychain as a [`KeyStore`], through `keyring` 3.6.3:
//! the macOS login keychain, Windows Credential Manager, and Secret Service
//! on Linux. Service [`KEYCHAIN_SERVICE`], account the brain's salt in hex,
//! value `key:…` or `passphrase:…` (the keys note, section 4).
//!
//! What it protects and what it doesn't (section 7): it keeps the key out of
//! files and away from other users; any program running as this user can
//! read an entry (macOS asks once per binary, and "Always Allow" then covers
//! anything that can run that binary).

use zeroize::Zeroizing;

use super::{hex, KeyStore, StoredSecret, KEYCHAIN_SERVICE};
use crate::crypto::SALT_LEN;
use crate::error::{MnemoError, Result};

/// The OS keychain.
pub(super) struct OsKeyStore;

/// The account name that never holds an entry, for [`KeyStore::check_available`].
const PROBE_ACCOUNT: &str = "mnemo-availability-probe";

fn entry(account: &str) -> Result<keyring::Entry> {
    keyring::Entry::new(KEYCHAIN_SERVICE, account).map_err(|e| store_error("opening an entry", e))
}

fn store_error(what: &str, e: keyring::Error) -> MnemoError {
    MnemoError::Io(std::io::Error::other(format!(
        "{} ({what}): {e}",
        OsKeyStore.describe()
    )))
}

impl KeyStore for OsKeyStore {
    fn describe(&self) -> String {
        if cfg!(target_os = "macos") {
            "the macOS keychain".into()
        } else if cfg!(windows) {
            "Windows Credential Manager".into()
        } else {
            "the Secret Service keyring".into()
        }
    }

    fn get(&self, salt: &[u8; SALT_LEN]) -> Result<Option<StoredSecret>> {
        match entry(&hex(salt))?.get_password() {
            Ok(text) => StoredSecret::decode(&Zeroizing::new(text)).map(Some),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(store_error("reading", e)),
        }
    }

    fn put(&self, salt: &[u8; SALT_LEN], secret: &StoredSecret, _label: &str) -> Result<()> {
        // `keyring` 3 sets no label of its own choosing; the service and the
        // account name the entry.
        entry(&hex(salt))?
            .set_password(&secret.encode())
            .map_err(|e| store_error("writing", e))
    }

    fn delete(&self, salt: &[u8; SALT_LEN]) -> Result<()> {
        match entry(&hex(salt))?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error("removing", e)),
        }
    }

    fn check_available(&self) -> Result<()> {
        match entry(PROBE_ACCOUNT)?.get_password() {
            Ok(_) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(store_error("checking it can be used", e)),
        }
    }
}
