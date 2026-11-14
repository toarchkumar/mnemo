//! 12.3 step 2: the OS keychain as a [`KeyStore`]: the macOS login
//! keychain, Windows Credential Manager, and Secret Service on Linux.
//! Service [`KEYCHAIN_SERVICE`], account the brain's salt in hex, value
//! `key:…` or `passphrase:…` (the keys note, section 4).
//!
//! Since step 5a (MSRV-2) it goes through the `keyring` 4 family:
//! `keyring-core` and one store crate per platform, built once per process
//! and used directly (the process-wide default store `keyring-core` offers
//! is left alone, since this is a library). The stores name an entry as
//! `keyring` 3.6.3 did, so entries written before keep working.
//!
//! What it protects and what it doesn't (section 7): it keeps the key out of
//! files and away from other users; any program running as this user can
//! read an entry (macOS asks once per binary, and "Always Allow" then covers
//! anything that can run that binary).

use std::sync::{Arc, OnceLock};

use keyring_core::{CredentialStore, Entry, Error as KeyringError};
use zeroize::Zeroizing;

use super::{hex, KeyStore, StoredSecret, KEYCHAIN_SERVICE};
use crate::crypto::SALT_LEN;
use crate::error::{MnemoError, Result};

/// The OS keychain.
pub(super) struct OsKeyStore;

/// The account name that never holds an entry, for [`KeyStore::check_available`].
const PROBE_ACCOUNT: &str = "mnemo-availability-probe";

/// The platform's store, made on first use. On Linux that connects to the
/// session bus, which can fail (no Secret Service); the error is kept and
/// reported by every call.
fn platform_store() -> std::result::Result<&'static Arc<CredentialStore>, &'static str> {
    static STORE: OnceLock<std::result::Result<Arc<CredentialStore>, String>> = OnceLock::new();
    STORE
        .get_or_init(|| {
            let made: keyring_core::Result<Arc<CredentialStore>> = {
                #[cfg(target_os = "macos")]
                {
                    apple_native_keyring_store::keychain::Store::new()
                        .map(|s| s as Arc<CredentialStore>)
                }
                #[cfg(windows)]
                {
                    windows_native_keyring_store::Store::new().map(|s| s as Arc<CredentialStore>)
                }
                #[cfg(target_os = "linux")]
                {
                    dbus_secret_service_keyring_store::Store::new()
                        .map(|s| s as Arc<CredentialStore>)
                }
            };
            made.map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(String::as_str)
}

fn entry(account: &str) -> Result<Entry> {
    let store = platform_store().map_err(|e| {
        MnemoError::Io(std::io::Error::other(format!(
            "{} (opening it): {e}",
            OsKeyStore.describe()
        )))
    })?;
    store
        .build(KEYCHAIN_SERVICE, account, None)
        .map_err(|e| store_error("opening an entry", e))
}

fn store_error(what: &str, e: KeyringError) -> MnemoError {
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
            Err(KeyringError::NoEntry) => Ok(None),
            Err(e) => Err(store_error("reading", e)),
        }
    }

    fn put(&self, salt: &[u8; SALT_LEN], secret: &StoredSecret, _label: &str) -> Result<()> {
        // The stores set no label of our choosing; the service and the
        // account name the entry.
        entry(&hex(salt))?
            .set_password(&secret.encode())
            .map_err(|e| store_error("writing", e))
    }

    fn delete(&self, salt: &[u8; SALT_LEN]) -> Result<()> {
        match entry(&hex(salt))?.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(store_error("removing", e)),
        }
    }

    fn check_available(&self) -> Result<()> {
        match entry(PROBE_ACCOUNT)?.get_password() {
            Ok(_) | Err(KeyringError::NoEntry) => Ok(()),
            Err(e) => Err(store_error("checking it can be used", e)),
        }
    }
}
