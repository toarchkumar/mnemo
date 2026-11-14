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
//! Every page gets a 96-bit nonce minted at write time from a per-session
//! random prefix plus a per-session monotonic counter (SEC-1). The random
//! prefix is drawn fresh when a `Mnemo` handle opens the file — it never
//! touches the disk — and the counter resets to zero in the same step.
//! Together they make an AES-GCM nonce reuse infeasible across the sessions
//! that could ever share one DEK, including the "two forks of the same file
//! after a backup" case that an on-disk `write_counter`-based scheme leaks.

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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        Self {
            m_cost: 19_456,
            t_cost: 2,
            p_cost: 1,
        }
    }

    /// PERF-1 step 7 (R-004.5): the Argon2id floor (8 KiB, one pass), for
    /// databases unlocked by a raw random 256-bit key
    /// (`Mnemo::create_with_key`). A uniformly random key has full entropy
    /// already, so stretching it buys nothing; these are recorded in the
    /// header so the normal open path runs in microseconds. **Never** use
    /// them with a human passphrase.
    pub fn raw_key() -> Self {
        Self {
            m_cost: 8,
            t_cost: 1,
            p_cost: 1,
        }
    }

    /// Deliberately weak parameters — **only** for fast unit tests.
    #[doc(hidden)]
    pub fn fast() -> Self {
        Self {
            m_cost: 512,
            t_cost: 1,
            p_cost: 1,
        }
    }
}

impl Default for KdfParams {
    fn default() -> Self {
        Self::secure()
    }
}

/// 12.3 (the keys note, section 1): the memory cost below which parameters
/// are the raw-key kind. [`KdfParams::raw_key`] is below it, and
/// [`KdfParams::fast`] sits exactly on it, so the tests' parameters stay
/// allowed with a passphrase.
const PASSPHRASE_FLOOR_M_COST: u32 = 512;

impl KdfParams {
    /// 12.3: the parameters a passphrase gets when a file had `self`: for
    /// each cost, the larger of `self`'s and [`KdfParams::secure`]'s. Every
    /// surface that rekeys to a passphrase uses it, so a passphrase never
    /// lands on weaker parameters than a file already had, nor below
    /// `secure()`.
    pub fn for_passphrase(self) -> Self {
        let s = Self::secure();
        Self {
            m_cost: self.m_cost.max(s.m_cost),
            t_cost: self.t_cost.max(s.t_cost),
            p_cost: self.p_cost.max(s.p_cost),
        }
    }

    /// 12.3: whether these are raw-key parameters, too light for a human
    /// passphrase ([`KdfParams::raw_key`] is; [`KdfParams::fast`] isn't).
    /// The engine refuses a passphrase with them, and a file a passphrase
    /// opens with them is reported.
    pub fn below_passphrase_floor(&self) -> bool {
        self.m_cost < PASSPHRASE_FLOOR_M_COST
    }

    /// 12.3: at or above the floor but below [`KdfParams::secure`]'s memory
    /// cost: test-grade parameters, such as [`KdfParams::fast`].
    pub fn is_test_grade(&self) -> bool {
        !self.below_passphrase_floor() && self.m_cost < Self::secure().m_cost
    }
}

/// 12.3: a rekey's hook, given the new salt before anything is written.
pub(crate) type SaltHook<'a> = dyn FnMut(&[u8; SALT_LEN]) -> Result<()> + 'a;

/// 12.3: what kind of secret a caller handed the engine. A passphrase is a
/// person's; a key is a uniformly random 256-bit value a machine made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    /// A passphrase a person chose.
    Passphrase,
    /// A raw 256-bit key.
    Key,
}

/// 12.3 (the keys note, section 1, rule 2): refuse a passphrase with
/// raw-key parameters. Called where every create and rekey path meets.
pub(crate) fn check_kdf_for(kind: SecretKind, kdf: KdfParams) -> Result<()> {
    if kind == SecretKind::Passphrase && kdf.below_passphrase_floor() {
        return Err(MnemoError::Invalid(format!(
            "these KDF parameters ({} KiB, {} pass) are for a raw 256-bit key, not a \
             passphrase; use create_with_key or rekey_with_key for a key, or \
             KdfParams::secure() for a passphrase",
            kdf.m_cost, kdf.t_cost
        )));
    }
    Ok(())
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
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
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
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
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

/// Generate a random `u64`, used by the pager to pick a per-session
/// nonce prefix. See [`session_page_nonce`] for the full construction.
pub fn random_u64() -> u64 {
    use rand::RngCore;
    rand::thread_rng().next_u64()
}

/// Build a per-session page nonce (PR SEC-1, v0.5.2).
///
/// Structure, following NIST SP 800-38D §8.2.1 (fixed field + invocation
/// field) for AES-GCM:
///
/// - **Bytes 0..8** — `session_random`: a 64-bit random value drawn
///   fresh on every writable [`crate::Mnemo::open`] / [`crate::Mnemo::create`]
///   (and regenerated on counter wrap). This is the "fixed field".
/// - **Bytes 8..12** — `session_counter`: a 32-bit per-session monotonic
///   counter that advances once per encrypted page write. This is the
///   "invocation field".
///
/// `page_no` is **not** encoded in the nonce. Pages were bound to their
/// home slot through AAD since v6 (`page_no.to_le_bytes()`), so an
/// attacker who swapped a valid page to a different slot already failed
/// authentication — the AAD binding stays in place and does the job the
/// `page_no` component of the pre-SEC-1 nonce was helping with.
///
/// # Why not deterministic any more
///
/// Before v0.5.2 the nonce was `page_no as u32 || write_counter as u64`,
/// which was unique **within a file** but not **across copies of a file**.
/// If you copied a `.mnemo` file to another host and both the original and
/// the copy got written to (two laptops syncing a Dropbox file; a
/// restored older backup resuming operation), each copy's writer produced
/// pages whose nonces collided with the other's at the same `(page_no,
/// write_counter)` coordinates, under the same DEK (`random_dek()` only
/// at create; `rekey` keeps it). That breaks AES-GCM's confidentiality
/// and tamper-evidence guarantee: whoever holds both ciphertexts can
/// recover the XOR of the plaintexts and forge pages with a valid tag.
///
/// A fresh random 64-bit `session_random` per writable open closes that:
/// cross-copy collision now requires two sessions to independently draw
/// the same 64-bit value (birthday bound ~2^32 sessions, i.e.
/// astronomically unlikely for a human-scale user). The 32-bit
/// `session_counter` guarantees uniqueness *within* a session; the pager
/// regenerates `session_random` and resets the counter if it would wrap.
///
/// # Decrypt is unaffected
///
/// Each page already stores its own nonce in bytes `[0..12]` of the
/// on-disk image (see `mnemo/src/pager.rs::read_page`). Decrypt reads the
/// stored nonce; it never recomputes one. **No format bump — a v0.5.1
/// file opens fine on v0.5.2 and vice versa.**
pub fn session_page_nonce(session_random: u64, session_counter: u32) -> [u8; NONCE_LEN] {
    let mut n = [0u8; NONCE_LEN];
    n[0..8].copy_from_slice(&session_random.to_le_bytes());
    n[8..12].copy_from_slice(&session_counter.to_le_bytes());
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
            Payload {
                msg: wrapped,
                aad: &[],
            },
        )
        .map_err(|_| MnemoError::WrongPassphrase)?;
    if plain.len() != KEY_LEN {
        return Err(MnemoError::WrongPassphrase);
    }
    let mut dek = Zeroizing::new([0u8; KEY_LEN]);
    dek.copy_from_slice(&plain);
    Ok(dek)
}
