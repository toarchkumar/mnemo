//! The binding's one error type, `MnemoError` in Swift and
//! `MnemoException` in Kotlin.

/// Every error a binding call can return. Engine errors map onto these;
/// `Closed` and `Embedder` are the binding's own.
///
/// Three classes (`is_retryable` tells the first apart):
/// - **retry later:** `Locked`, `Io`, `Kdf` (the KDF couldn't get its
///   memory), `Closed` (reopen);
/// - **fix the call:** `Invalid`, `DimensionMismatch`, `EmbedderMismatch`,
///   `ReadOnly`, `NeedsWriteOpen`, `NotFound`, `WrongPassphrase`, `Embedder`;
/// - **the data is unreadable:** `Corrupt`, `HeaderTampered`, `Crypto`,
///   `NotAMnemoFile`, `UnsupportedVersion`.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum MnemoError {
    /// Another handle (in this process or another) holds the file's lock.
    #[error("file already locked by another handle: {path}")]
    Locked { path: String },
    /// The passphrase or key doesn't open this file.
    #[error("wrong passphrase or key")]
    WrongPassphrase,
    /// The header's keyed seal failed: the file was modified outside MNemo.
    #[error("header tampered: the file was modified outside MNemo")]
    HeaderTampered,
    /// The file's format is one this build doesn't open. `message` says which
    /// versions it does (v0.12.2).
    #[error("{message}")]
    UnsupportedVersion { found: u16, message: String },
    /// The file isn't a `.mnemo` database (its magic bytes are wrong).
    #[error("not a .mnemo file: {message}")]
    NotAMnemoFile { message: String },
    /// Deriving the key from the passphrase failed, most often because
    /// Argon2 couldn't allocate its memory on a phone short of RAM.
    /// Retryable.
    #[error("key derivation failed: {message}")]
    Kdf { message: String },
    /// A cryptographic operation failed.
    #[error("cryptographic operation failed: {message}")]
    Crypto { message: String },
    /// A read-only open needs a read-write open first: the file has a WAL
    /// transaction to recover, or a format migration to apply. Open it
    /// read-write once, then read-only again.
    #[error("read-only open blocked: {reason}; open read-write once to fix")]
    NeedsWriteOpen { reason: String },
    /// No such memory, key, thread or snapshot.
    #[error("not found: {message}")]
    NotFound { message: String },
    /// A write on a read-only handle.
    #[error("operation requires a read-write handle")]
    ReadOnly,
    /// A vector's width doesn't match the file's.
    #[error("vector dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: u64, got: u64 },
    /// The embedder isn't the one this file's vectors came from (another
    /// model, or another revision of the same one, even at the same width).
    /// A file is bound to the first embedder attached to it: use that one,
    /// or create a new store for the new model (EMBED-ID-1).
    #[error(
        "embedder identity mismatch: this file's vectors come from {} (dimensions {}, normalize {}), \
         but the embedder given is {} (dimensions {}, normalize {}); create a new store for the new model",
        stored.name, stored.dimensions, stored.normalize, given.name, given.dimensions, given.normalize
    )]
    EmbedderMismatch {
        stored: crate::types::EmbedderIdentity,
        given: crate::types::EmbedderIdentity,
    },
    /// A bad argument (an empty key, malformed metadata JSON, …).
    #[error("invalid argument: {message}")]
    Invalid { message: String },
    /// An I/O error from the file system.
    #[error("i/o error: {message}")]
    Io { message: String },
    /// The file failed an integrity check (a CRC, page authentication,
    /// decoding). Integrity failures only (v0.12.2).
    #[error("corrupt: {message}")]
    Corrupt { message: String },
    /// The handle was closed, or dropped after a panic poisoned it; open the
    /// file again (the Swift and Kotlin stores do that themselves).
    #[error("database is closed")]
    Closed,
    /// The app's embedder callback threw or returned the wrong shape.
    /// Nothing was written.
    #[error("embedder failed: {message}")]
    Embedder { message: String },
}

impl From<mnemo_engine::MnemoError> for MnemoError {
    fn from(e: mnemo_engine::MnemoError) -> Self {
        use mnemo_engine::MnemoError as E;
        match e {
            E::Io(err) => MnemoError::Io {
                message: err.to_string(),
            },
            E::Locked { path } => MnemoError::Locked {
                path: path.display().to_string(),
            },
            E::WrongPassphrase => MnemoError::WrongPassphrase,
            E::HeaderTampered => MnemoError::HeaderTampered,
            other @ E::UnsupportedVersion(_) => {
                let message = other.to_string();
                let E::UnsupportedVersion(found) = other else {
                    unreachable!()
                };
                MnemoError::UnsupportedVersion { found, message }
            }
            other @ E::BadMagic => MnemoError::NotAMnemoFile {
                message: other.to_string(),
            },
            E::Kdf(message) => MnemoError::Kdf { message },
            E::Crypto(message) => MnemoError::Crypto { message },
            E::NotFound(message) => MnemoError::NotFound { message },
            E::ReadOnly => MnemoError::ReadOnly,
            E::DimensionMismatch { expected, got } => MnemoError::DimensionMismatch {
                expected: expected as u64,
                got: got as u64,
            },
            E::Invalid(message) => MnemoError::Invalid { message },
            E::EmbedderMismatch { stored, given } => MnemoError::EmbedderMismatch {
                stored: identity(stored),
                given: identity(given),
            },
            E::NeedsWriteOpen { reason } => MnemoError::NeedsWriteOpen {
                reason: reason.to_string(),
            },
            // What's left is integrity: a header CRC, page authentication,
            // decoding.
            other => MnemoError::Corrupt {
                message: other.to_string(),
            },
        }
    }
}

fn identity(i: mnemo_engine::EmbedderIdentity) -> crate::types::EmbedderIdentity {
    crate::types::EmbedderIdentity {
        name: i.name,
        dimensions: i.dimensions as u32,
        normalize: i.normalize,
    }
}

#[uniffi::export]
impl MnemoError {
    /// Whether the same call may succeed later with nothing changed: `Locked`,
    /// `Io`, `Kdf` and `Closed` (reopen first). Everything else needs a
    /// different call, or means the data can't be read (v0.12.2).
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            MnemoError::Locked { .. }
                | MnemoError::Io { .. }
                | MnemoError::Kdf { .. }
                | MnemoError::Closed
        )
    }
}

/// What the app's embedder callback throws (`EmbedderError` in Swift and
/// Kotlin).
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum EmbedderError {
    /// The embedder couldn't produce vectors.
    #[error("{message}")]
    Failed { message: String },
}

impl From<uniffi::UnexpectedUniFFICallbackError> for EmbedderError {
    fn from(e: uniffi::UnexpectedUniFFICallbackError) -> Self {
        EmbedderError::Failed { message: e.reason }
    }
}
