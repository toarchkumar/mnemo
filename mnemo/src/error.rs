//! Error types for Mnemo.

use thiserror::Error;

/// All errors the Mnemo engine can produce.
#[derive(Error, Debug)]
pub enum MnemoError {
    /// An underlying I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The file does not begin with the Mnemo magic bytes.
    #[error("bad magic bytes — file is not a .mnemo database")]
    BadMagic,

    /// The on-disk format version is newer than this build understands.
    #[error("unsupported .mnemo format version {0} (this build supports v1)")]
    UnsupportedVersion(u16),

    /// The header page failed its CRC check — a torn page-0 write or
    /// corruption. [`crate::Mnemo::open`] tries to repair this from the WAL.
    #[error("header checksum mismatch — page 0 is torn or corrupt")]
    HeaderChecksum,

    /// Returned when the supplied passphrase fails to unwrap the data key.
    /// The failure is an authenticated-decryption failure, so it is
    /// indistinguishable from a tampered key blob — both are rejected cleanly.
    #[error("wrong passphrase, or the key material has been tampered with")]
    WrongPassphrase,

    /// AEAD authentication failed for a page: corruption or tampering.
    #[error("page {0} failed authentication — the file is corrupt or tampered")]
    PageAuthFailed(u64),

    /// The v7 header seal failed authentication — at least one mutable
    /// header field has been rewritten without the DEK. Open refuses to
    /// proceed so a silent rollback or pointer-rewrite attack can't
    /// surface stale data.
    #[error("header seal authentication failed — mutable fields tampered with")]
    HeaderTampered,

    /// A low-level cryptographic operation failed.
    #[error("cryptographic operation failed: {0}")]
    Crypto(String),

    /// Argon2id key derivation failed.
    #[error("key derivation failed: {0}")]
    Kdf(String),

    /// A record failed to (de)serialize.
    #[error("serialization error: {0}")]
    Serialize(String),

    /// No memory exists with the requested ID.
    #[error("memory '{0}' not found")]
    NotFound(String),

    /// A vector did not match the database's configured dimensionality.
    #[error("vector dimension mismatch: database expects {expected}, got {got}")]
    DimensionMismatch {
        /// Dimensionality the database was created with.
        expected: usize,
        /// Dimensionality of the offending vector.
        got: usize,
    },

    /// A caller-supplied argument was invalid.
    #[error("invalid argument: {0}")]
    Invalid(String),

    /// Another process (or another handle in this process) already holds a
    /// conflicting OS file lock on the target `.mnemo` file. Write handles
    /// take an exclusive lock; read-only handles take a shared lock. Two
    /// writers or a writer + reader will collide.
    #[error("file already locked by another handle: {path}")]
    Locked {
        /// The path whose lock was requested and refused.
        path: std::path::PathBuf,
    },

    /// A mutation was attempted on a handle opened via
    /// [`crate::Mnemo::open_read_only`]. Reopen read-write to mutate.
    #[error("operation requires a read-write handle (this one is read-only)")]
    ReadOnly,

    /// A read-only open was requested but the file has pending on-disk work
    /// that requires a write handle to complete — a committed-but-unchecked
    /// WAL transaction, or a format-version migration older than the current
    /// `VERSION`. Open read-write once, close cleanly, then reopen read-only.
    #[error("read-only open blocked: {reason} — open read-write once to fix")]
    NeedsWriteOpen {
        /// Human-readable reason (e.g. `"pending WAL recovery"`).
        reason: &'static str,
    },
}

/// Convenience result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, MnemoError>;
