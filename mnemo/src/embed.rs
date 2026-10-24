//! Embedder trait + text-side convenience paths (Phase 3, PR 10a).
//!
//! Mnemo stores vectors; **embedders** are the thing that turns text
//! into those vectors. This module adds a small trait that any Rust
//! embedder (hand-written, feature-gated `fastembed`, or a bound Python
//! callable) can implement, plus the two convenience paths every
//! downstream caller actually wants:
//!
//! - [`Mnemo::remember_text`] — embed a string and `remember` the result.
//! - [`RecallRequest::from_text`] — embed a query string and build a
//!   `RecallRequest`.
//!
//! Both validate the embedder's dimensionality against the database's
//! configured width ([`MnemoError::DimensionMismatch`]) and compare the
//! embedder's identity (name, dimensions, normalize) to the manifest
//! (`EmbedderIdentity::MISMATCH_MSG`). An identity mismatch is a loud
//! error — mixing two embedders in one file silently corrupts recall
//! quality, so we refuse rather than warn.
//!
//! The identity check lives on the handle via [`Mnemo::set_embedder`]:
//! one call registers the embedder, performs the identity check once,
//! and upserts the manifest's `embedder_name`/`embedder_dimensions`/
//! `embedder_normalize` metadata on first registration. Subsequent
//! `remember_text`/`from_text` calls on the handle are a plain
//! delegation.

use crate::error::{MnemoError, Result};

/// Describe an embedder's observable identity. Used both in-memory on
/// an open [`Mnemo`] handle and persisted as three `metadata.embedder_*`
/// keys on the scaffold manifest so that reopening with a different
/// embedder is caught at `set_embedder` time instead of silently
/// polluting the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmbedderIdentity {
    /// Model identifier the caller chose. Convention: a short, stable
    /// slug — `"all-MiniLM-L6-v2"`, `"text-embedding-3-small"`.
    pub name: String,
    /// Vector width this embedder emits. Validated against
    /// [`Mnemo::dimensions`] on registration.
    pub dimensions: usize,
    /// Whether the embedder returns unit-normalized vectors. Caller's
    /// claim — mnemo doesn't verify it, but records it on the manifest
    /// so downstream readers can tell whether a dot product is cosine
    /// equivalent.
    pub normalize: bool,
}

impl EmbedderIdentity {
    /// Message rendered inside [`MnemoError::Invalid`] on an identity
    /// mismatch. Public so tests + binding layers can assert on it.
    pub const MISMATCH_MSG: &'static str = "embedder identity mismatch";
}

/// Minimal trait any embedder implementation must satisfy. Returns
/// vectors in the same order as the input `texts`.
///
/// Implementations are expected to be cheap to clone / `Send + Sync`
/// when they wrap thread-safe state; the engine holds an `Arc<dyn
/// Embedder>` on the handle via [`Mnemo::set_embedder`]. See
/// [`Mnemo::remember_text`] for the full user-facing contract.
pub trait Embedder: Send + Sync {
    /// This embedder's observable identity (name, dims, normalize).
    /// Must be stable for the lifetime of the embedder — mnemo caches
    /// it on `set_embedder` and does not re-query per call.
    fn identity(&self) -> EmbedderIdentity;

    /// Embed `texts`, returning one vector per input in the same order.
    /// Caller-level errors (network failure for a hosted embedder, OOM
    /// in a local model) should bubble up as [`MnemoError::Invalid`]
    /// with a short message — the specific crypto / page errors in
    /// [`MnemoError`] are reserved for the engine's own concerns.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Returns `true` iff every component of `v` is exactly `0.0`. This is
/// the predicate `remember` uses to refuse all-zero vectors (which
/// score 0 on every cosine query and pollute recall).
pub(crate) fn is_zero_vector(v: &[f32]) -> bool {
    !v.is_empty() && v.iter().all(|x| *x == 0.0)
}

/// Metadata keys used on the scaffold manifest to record the embedder
/// identity. Kept as `pub(crate)` constants so a future binding or
/// migration can refer to them without re-typing the literals.
pub(crate) const MANIFEST_EMBEDDER_NAME: &str = "embedder_name";
pub(crate) const MANIFEST_EMBEDDER_DIMENSIONS: &str = "embedder_dimensions";
pub(crate) const MANIFEST_EMBEDDER_NORMALIZE: &str = "embedder_normalize";

/// Build a clear `MnemoError::Invalid` describing an identity mismatch,
/// shaped so a Python caller catching `RuntimeError` can grep the string.
pub(crate) fn identity_mismatch_err(want: &EmbedderIdentity, saw: &EmbedderIdentity) -> MnemoError {
    MnemoError::Invalid(format!(
        "{}: database manifest records embedder (name={:?}, dimensions={}, normalize={}), \
         but this handle was passed embedder (name={:?}, dimensions={}, normalize={}). \
         Mixing embedders silently corrupts recall quality — open the file with the \
         original embedder, or create a fresh database for the new one.",
        EmbedderIdentity::MISMATCH_MSG,
        want.name,
        want.dimensions,
        want.normalize,
        saw.name,
        saw.dimensions,
        saw.normalize,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_zero_vector_detects_all_zero() {
        assert!(is_zero_vector(&[0.0, 0.0, 0.0]));
        assert!(!is_zero_vector(&[0.0, 0.0, 1e-9]));
        assert!(!is_zero_vector(&[]));
    }

    #[test]
    fn mismatch_err_includes_both_identities() {
        let want = EmbedderIdentity {
            name: "A".into(),
            dimensions: 4,
            normalize: true,
        };
        let saw = EmbedderIdentity {
            name: "B".into(),
            dimensions: 4,
            normalize: false,
        };
        let msg = format!("{}", identity_mismatch_err(&want, &saw));
        assert!(msg.contains(EmbedderIdentity::MISMATCH_MSG));
        assert!(msg.contains("name=\"A\""));
        assert!(msg.contains("name=\"B\""));
    }
}
