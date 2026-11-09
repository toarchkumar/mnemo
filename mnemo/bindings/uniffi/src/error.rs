//! The binding's one error type, `MnemoError` in Swift and
//! `MnemoException` in Kotlin.

/// Every error a binding call can return. Engine errors map onto these;
/// `Closed` and `Embedder` are the binding's own.
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
    /// The file's format is newer than this build reads.
    #[error("unsupported .mnemo format version {found}")]
    UnsupportedVersion { found: u16 },
    /// No such memory, key, thread or snapshot.
    #[error("not found: {message}")]
    NotFound { message: String },
    /// A write on a read-only handle.
    #[error("operation requires a read-write handle")]
    ReadOnly,
    /// A vector's width doesn't match the file's.
    #[error("vector dimension mismatch: expected {expected}, got {got}")]
    DimensionMismatch { expected: u64, got: u64 },
    /// A bad argument (an empty key, malformed metadata JSON, …).
    #[error("invalid argument: {message}")]
    Invalid { message: String },
    /// An I/O error from the file system.
    #[error("i/o error: {message}")]
    Io { message: String },
    /// The file failed an integrity check (magic, CRC, page
    /// authentication, decoding), or this handle can no longer be used
    /// (a poisoned lock after a panic: reopen the file).
    #[error("corrupt: {message}")]
    Corrupt { message: String },
    /// The handle was closed; open the file again.
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
            E::UnsupportedVersion(found) => MnemoError::UnsupportedVersion { found },
            E::NotFound(message) => MnemoError::NotFound { message },
            E::ReadOnly => MnemoError::ReadOnly,
            E::DimensionMismatch { expected, got } => MnemoError::DimensionMismatch {
                expected: expected as u64,
                got: got as u64,
            },
            E::Invalid(message) => MnemoError::Invalid { message },
            other @ E::NeedsWriteOpen { .. } => MnemoError::Invalid {
                message: other.to_string(),
            },
            other => MnemoError::Corrupt {
                message: other.to_string(),
            },
        }
    }
}

/// The message a poisoned handle lock reports: a panic happened while the
/// handle was in use, so its state can't be trusted any more.
pub(crate) const POISONED: &str = "handle poisoned by an earlier panic; reopen the file";

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
