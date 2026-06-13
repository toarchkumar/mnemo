//! Cryptography layer for Mnemo (Phase 4 of the build plan).
//!
//! Mnemo uses a **two-tier key hierarchy**:
//!
//! ```text
//!   passphrase ──Argon2id──▶ KEK (key-encryption-key)
//!                              │  AES-256-GCM
//!                              ▼
//!                            DEK (data-encryption-key, random 256-bit)
//!                              │  AES-256-GCM, per-page
//!                              ▼
//!                            encrypted pages on disk
//! ```
//!
//! The DEK encrypts every page. The KEK only ever encrypts ("wraps") the DEK.
//! This is what makes `rekey` cheap: changing the passphrase re-derives the KEK
//! and re-wraps the DEK — the pages themselves never need re-encryption.
//!
//! Every page gets a unique nonce derived from `(page_number, write_counter)`,
//! where `write_counter` is a monotonic counter persisted in the file header.
//! Because the counter never repeats, an AES-GCM nonce is never reused.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use crate::error::{MnemoError, Result};

/// Length of a symmetric key in bytes (256-bit).
pub const KEY_LEN: usize = 32;
/// Length of an AES-GCM nonce in bytes (96-bit).
pub const NONCE_LEN: usize = 12;
/// Length of an AES-GCM authentication tag in bytes.
pub const TAG_LEN: usize = 16;
/// Length of the Argon2 salt in bytes.
pub const SALT_LEN: usize = 16;

/// Argon2id cost parameters. Stored (unencrypted) in the file header so the
/// KEK can be re-derived on open.
#[derive(Clone, Copy, Debug)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_cost: u32,
    /// Time cost (number of iterations).
    pub t_cost: u32,
    /// Parallelism (lanes).
    pub p_cost: u32,
}

impl KdfParams {
    /// Sensible interactive-use defaults (~19 MiB, 2 passes).
    pub fn secure() -> Self {
        Self { m_cost: 19_456, t_cost: 2, p_cost: 1 }
    }

    /// Deliberately weak parameters — **only** for fast unit tests.
    #[doc(hidden)]
    pub fn fast() -> Self {
        Self { m_cost: 512, t_cost: 1, p_cost: 1 }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::secure()
    }
}

/// Derive the key-encryption-key (KEK) from a passphrase using Argon2id.
pub fn derive_kek(
    passphrase: &[u8],
    salt: &[u8],
    params: KdfParams,
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let p = Params::new(params.m_cost, params.t_cost, params.p_cost, Some(KEY_LEN))
        .map_err(|e| MnemoError::Kdf(e.to_string()))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, p);
    let mut key = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(passphrase, salt, &mut key[..])
        .map_err(|e| MnemoError::Kdf(e.to_string()))?;
    Ok(key)
}

fn cipher(key: &[u8; KEY_LEN]) -> Aes256Gcm {
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
}

/// AES-256-GCM encrypt with **Associated Authenticated Data**. The AAD is not
/// part of the ciphertext but is bound into the authentication tag; decryption
/// fails unless the caller supplies the same AAD. Output is
/// `ciphertext || 16-byte tag`.
///
/// Pass `&[]` to encrypt without AAD — this matches the format before v6, used
/// for wrapping the DEK and for v4/v5 page encryption. From v6 onwards, page
/// encryption passes `page_no.to_le_bytes()` as AAD so an attacker can't
/// transplant a valid encrypted page to a different page slot — the
/// authentication tag binds the page image to its home page number.
pub fn aead_encrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    cipher(key)
        .encrypt(Nonce::from_slice(nonce), Payload { msg: plaintext, aad })
        .map_err(|e| MnemoError::Crypto(e.to_string()))
}

/// AES-256-GCM decrypt with AAD. Input must be `ciphertext || 16-byte tag`.
/// Returns an error if authentication fails — including when the supplied
/// AAD doesn't match what was used during encryption.
pub fn aead_decrypt(
    key: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    cipher(key)
        .decrypt(Nonce::from_slice(nonce), Payload { msg: ciphertext, aad })
        .map_err(|_| MnemoError::Crypto("authentication failed".into()))
}

/// Generate a fresh random 256-bit data-encryption-key.
pub fn random_dek() -> Zeroizing<[u8; KEY_LEN]> {
    use rand::RngCore;
    let mut dek = Zeroizing::new([0u8; KEY_LEN]);
    rand::thread_rng().fill_bytes(&mut dek[..]);
    dek
}

/// Generate a random Argon2 salt.
pub fn random_salt() -> [u8; SALT_LEN] {
    use rand::RngCore;
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    salt
}

/// Generate a random nonce (used for wrapping the DEK).
pub fn random_nonce() -> [u8; NONCE_LEN] {
    use rand::RngCore;
    let mut n = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut n);
    n
}

/// Build a deterministic, unique page nonce from a page number and the
/// monotonic write counter. `(page, counter)` never repeats, so neither does
/// the nonce — the central safety property of the encryption scheme.
pub fn page_nonce(page_no: u64, write_counter: u64) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    n[0..4].copy_from_slice(&(page_no as u32).to_le_bytes());
    n[4..12].copy_from_slice(&write_counter.to_le_bytes());
    n
}

/// Wrap (encrypt) the DEK with the KEK. The wrap uses **empty AAD** in every
/// version, so v5 wrapped-DEK bytes on disk round-trip unchanged through a v6
/// build — only page encryption changed in v6.
pub fn wrap_dek(
    kek: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    dek: &[u8; KEY_LEN],
) -> Result<Vec<u8>> {
    aead_encrypt(kek, nonce, dek, &[])
}

/// Unwrap (decrypt) the DEK with the KEK. A wrong KEK (wrong passphrase)
/// fails authentication and is reported as [`MnemoError::WrongPassphrase`].
pub fn unwrap_dek(
    kek: &[u8; KEY_LEN],
    nonce: &[u8; NONCE_LEN],
    wrapped: &[u8],
) -> Result<Zeroizing<[u8; KEY_LEN]>> {
    let plain = cipher(kek)
        .decrypt(
            Nonce::from_slice(nonce),
            Payload { msg: wrapped, aad: &[] },
        )
        .map_err(|_| MnemoError::WrongPassphrase)?;
    if plain.len() != KEY_LEN {
        return Err(MnemoError::WrongPassphrase);
    }
    let mut dek = Zeroizing::new([0u8; KEY_LEN]);
    dek.copy_from_slice(&plain);
    Ok(dek)
}
