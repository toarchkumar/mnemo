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
    #[error(
        "unsupported .mnemo format version {0}: this build opens v4 to v9; \
         a newer file needs a newer mnemo-engine"
    )]
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
    ///
    /// On a file older than v9 (which no build opens read-write since the
    /// v8 sunset) the reason itself says to run `mnemo compact`, which
    /// completes the work and converts the file, and the generic advice is
    /// left off.
    #[error("read-only open blocked: {reason}{}", needs_write_open_hint(reason))]
    NeedsWriteOpen {
        /// Human-readable reason (e.g. `"pending WAL recovery"`).
        reason: &'static str,
    },

    /// The v8 sunset (v0.13): the file's format is older than v9, so it
    /// can't be opened read-write. A v8 file still opens read-only
    /// ([`crate::Mnemo::open_read_only`]); `mnemo compact` (or
    /// [`crate::Mnemo::compact_file`]) converts any v4–v8 file to v9 and
    /// keeps every live memory.
    #[error(
        "this file is format v{version}, which this build {}; run `mnemo compact <file>` \
         (with `--key-file <key>` for a raw-key file) to convert it to v9 (every live \
         memory is kept)",
        if *version == 8 { "opens read-only" } else { "no longer opens" }
    )]
    FormatTooOld {
        /// The file's on-disk format version.
        version: u16,
    },

    /// EMBED-ID-1: the embedder passed to [`crate::Mnemo::set_embedder`]
    /// isn't the one this file's vectors came from. The file records the
    /// identity of the first embedder attached to it; a different name,
    /// width or `normalize` flag is refused, because mixing two models'
    /// vectors in one file silently breaks recall. Use the original
    /// embedder, or create a new store for the new model.
    #[error(
        "embedder identity mismatch: this file's vectors come from embedder \
         (name={:?}, dimensions={}, normalize={}), but the embedder given is \
         (name={:?}, dimensions={}, normalize={}). Open the file with its \
         original embedder, or create a new store for the new model.",
        stored.name, stored.dimensions, stored.normalize,
        given.name, given.dimensions, given.normalize
    )]
    EmbedderMismatch {
        /// The identity the file records.
        stored: crate::embed::EmbedderIdentity,
        /// The identity of the embedder that was passed.
        given: crate::embed::EmbedderIdentity,
    },
}

/// Convenience result alias used throughout the crate.
pub type Result<T> = std::result::Result<T, MnemoError>;

/// The advice after a [`MnemoError::NeedsWriteOpen`] reason. A reason that
/// names its own fix (`mnemo compact`, for a pre-v9 file) needs no other.
pub(crate) fn needs_write_open_hint(reason: &str) -> &'static str {
    if reason.contains("mnemo compact") {
        ""
    } else {
        " — open read-write once to fix"
    }
}
