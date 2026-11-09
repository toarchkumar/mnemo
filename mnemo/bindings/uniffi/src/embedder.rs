//! The app's embedder, implemented in Swift or Kotlin (the OS embedding
//! API, or a bundled model).

use std::sync::Arc;

use crate::error::{EmbedderError, MnemoError};
use crate::types::EmbedderIdentity;

/// Implemented by the app. `embed` is called synchronously on the calling
/// thread, **never while the handle's lock is held**, so it may call back
/// into the same database. `identity` is read once, when the embedder is
/// set.
#[uniffi::export(foreign)]
pub trait MnemoEmbedder: Send + Sync {
    /// The model's identity, checked against the file's manifest.
    fn identity(&self) -> EmbedderIdentity;
    /// One vector per text, in order.
    fn embed(&self, texts: Vec<String>) -> Result<Vec<Vec<f32>>, EmbedderError>;
}

/// Run the app's embedder and check the shape of what it returns. A throw
/// becomes [`MnemoError::Embedder`].
pub(crate) fn embed_checked(
    embedder: &Arc<dyn MnemoEmbedder>,
    texts: Vec<String>,
    dimensions: usize,
) -> Result<Vec<Vec<f32>>, MnemoError> {
    let n = texts.len();
    let vectors = embedder.embed(texts).map_err(|e| MnemoError::Embedder {
        message: e.to_string(),
    })?;
    if vectors.len() != n {
        return Err(MnemoError::Embedder {
            message: format!("returned {} vectors for {n} texts", vectors.len()),
        });
    }
    if let Some(v) = vectors.iter().find(|v| v.len() != dimensions) {
        return Err(MnemoError::DimensionMismatch {
            expected: dimensions as u64,
            got: v.len() as u64,
        });
    }
    Ok(vectors)
}

/// The engine-side view of the app's embedder. The engine's `set_embedder`
/// uses it for the identity and manifest checks. The binding always embeds
/// itself, outside the handle lock, before calling the engine, so the
/// engine never runs `embed` through this adapter on any binding path; if
/// it ever did, a throw would surface as an engine `Invalid` error.
pub(crate) struct EngineEmbedder {
    pub(crate) inner: Arc<dyn MnemoEmbedder>,
    pub(crate) identity: mnemo_engine::EmbedderIdentity,
}

impl mnemo_engine::Embedder for EngineEmbedder {
    fn identity(&self) -> mnemo_engine::EmbedderIdentity {
        self.identity.clone()
    }

    fn embed(&self, texts: &[&str]) -> mnemo_engine::Result<Vec<Vec<f32>>> {
        self.inner
            .embed(texts.iter().map(|t| t.to_string()).collect())
            .map_err(|e| mnemo_engine::MnemoError::Invalid(format!("embedder failed: {e}")))
    }
}
