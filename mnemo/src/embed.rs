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
//! and upserts the manifest's `metadata.embedder = { name, dimensions,
//! normalize }` nested object on first registration. Subsequent
//! `remember_text`/`from_text` calls on the handle are a plain
//! delegation.
//!
//! **Manifest schema (canonical, docs-matching):** the embedder
//! identity lives at `metadata.embedder.{name,dimensions,normalize}`
//! on any onboarding/manifest memory — the shape `test/scripts/seed.json`
//! writes and `mnemo/README.md` documents. For back-compat with the
//! very first PR 10a dev builds that wrote flat
//! `embedder_name`/`embedder_dimensions`/`embedder_normalize` keys,
//! those are still accepted on read as a legacy alias; writes are
//! always the nested form.

use crate::error::{MnemoError, Result};
use serde_json::{Map, Value};

/// Describe an embedder's observable identity. Used both in-memory on
/// an open [`Mnemo`] handle and persisted at `metadata.embedder` on the
/// scaffold manifest so that reopening with a different embedder is
/// caught at `set_embedder` time instead of silently polluting the
/// index.
///
/// Build one with a struct literal ending in `..Default::default()`, so a
/// field added later (as `query_convention` was in v0.15) doesn't break it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
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
    /// 12.3 step 4 (v0.15): how [`Embedder::embed_query`] differs from
    /// [`Embedder::embed`]. Empty (the default) when queries are embedded
    /// like documents; otherwise a short, stable name for the convention,
    /// such as `"bge-query-instruction"` for a model that prefixes queries
    /// with an instruction. It's part of the identity: a file embedded one
    /// way refuses an embedder that queries another way
    /// ([`MnemoError::EmbedderMismatch`]); `mnemo reembed` moves a file to a
    /// new identity. Stored only when it isn't empty, so a symmetric
    /// embedder records what it did before v0.15. Builds before v0.15
    /// ignore it.
    pub query_convention: String,
}

impl EmbedderIdentity {
    /// Message rendered inside [`MnemoError::Invalid`] on an identity
    /// mismatch. Public so tests + binding layers can assert on it.
    pub const MISMATCH_MSG: &'static str = "embedder identity mismatch";

    /// The identity as `(name=…, dimensions=…, normalize=…)`, with
    /// `query_convention=…` when it isn't empty.
    pub(crate) fn describe(&self) -> String {
        let mut s = format!(
            "(name={:?}, dimensions={}, normalize={}",
            self.name, self.dimensions, self.normalize
        );
        if !self.query_convention.is_empty() {
            s.push_str(&format!(", query_convention={:?}", self.query_convention));
        }
        s.push(')');
        s
    }
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
    ///
    /// This is the **document side**: everything stored is embedded with it.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;

    /// 12.3 step 4 (v0.15): embed `texts` as **queries**. Every recall by
    /// text uses it ([`crate::RecallRequest::from_text`], the CLI's
    /// `recall --text`, the MCP `recall` tool, Python's `recall_text`). It
    /// defaults to [`Embedder::embed`]. A model that embeds queries
    /// differently (an instruction prefix, a separate query encoder)
    /// overrides it and names the difference in
    /// [`EmbedderIdentity::query_convention`].
    fn embed_query(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.embed(texts)
    }
}

/// The JSON key of [`EmbedderIdentity::query_convention`] in both stored
/// identities, written only when it isn't empty.
pub(crate) const QUERY_CONVENTION_KEY: &str = "query_convention";

/// The query convention from a stored identity object: empty when absent.
/// A present key that isn't a string makes the identity malformed.
fn convention_from(obj: &Map<String, Value>) -> Option<String> {
    match obj.get(QUERY_CONVENTION_KEY) {
        None => Some(String::new()),
        Some(v) => v.as_str().map(str::to_string),
    }
}

/// Returns `true` iff every component of `v` is exactly `0.0`. This is
/// the predicate `remember` uses to refuse all-zero vectors (which
/// score 0 on every cosine query and pollute recall).
pub(crate) fn is_zero_vector(v: &[f32]) -> bool {
    !v.is_empty() && v.iter().all(|x| *x == 0.0)
}

/// Canonical nested metadata key: `metadata.embedder = { name,
/// dimensions, normalize }`. This is the shape `test/scripts/seed.json`
/// writes and `mnemo/README.md` documents.
pub(crate) const MANIFEST_EMBEDDER_KEY: &str = "embedder";

/// Legacy flat keys written by the very first PR 10a dev builds
/// (never released). Still accepted on read so a locally-bootstrapped
/// repo doesn't trip the new check; writes are always the nested form
/// above.
pub(crate) const LEGACY_FLAT_NAME: &str = "embedder_name";
pub(crate) const LEGACY_FLAT_DIMENSIONS: &str = "embedder_dimensions";
pub(crate) const LEGACY_FLAT_NORMALIZE: &str = "embedder_normalize";

/// Read the recorded embedder identity off a memory's metadata map.
/// Returns `None` if neither the canonical nested form nor the legacy
/// flat form is present (or either is present but malformed — missing
/// a required field, wrong type).
///
/// Nested form (canonical) is tried first; legacy flat form is only
/// consulted when nothing nested is found. This keeps a seed.json-
/// shaped file hitting the canonical path even if a bugged earlier
/// `set_embedder` call left stale flat keys alongside.
pub(crate) fn read_identity_from_metadata(meta: &Map<String, Value>) -> Option<EmbedderIdentity> {
    // Canonical nested form.
    if let Some(obj) = meta.get(MANIFEST_EMBEDDER_KEY).and_then(Value::as_object) {
        let name = obj.get("name").and_then(Value::as_str)?;
        let dims = obj.get("dimensions").and_then(Value::as_u64)?;
        let norm = obj.get("normalize").and_then(Value::as_bool)?;
        return Some(EmbedderIdentity {
            name: name.to_string(),
            dimensions: dims as usize,
            normalize: norm,
            query_convention: convention_from(obj)?,
        });
    }
    // Legacy flat fallback.
    let name = meta.get(LEGACY_FLAT_NAME).and_then(Value::as_str)?;
    let dims = meta.get(LEGACY_FLAT_DIMENSIONS).and_then(Value::as_u64)?;
    let norm = meta.get(LEGACY_FLAT_NORMALIZE).and_then(Value::as_bool)?;
    Some(EmbedderIdentity {
        name: name.to_string(),
        dimensions: dims as usize,
        normalize: norm,
        query_convention: String::new(),
    })
}

/// Write (or overwrite) the embedder identity on a memory's metadata
/// map, in the canonical nested form. Also removes any legacy flat
/// keys from the same map so the upsert doesn't leave duplicates
/// behind.
pub(crate) fn write_identity_into_metadata(
    meta: &mut Map<String, Value>,
    identity: &EmbedderIdentity,
) {
    let mut obj = Map::new();
    obj.insert("name".into(), Value::String(identity.name.clone()));
    obj.insert(
        "dimensions".into(),
        Value::Number(serde_json::Number::from(identity.dimensions as u64)),
    );
    obj.insert("normalize".into(), Value::Bool(identity.normalize));
    if !identity.query_convention.is_empty() {
        obj.insert(
            QUERY_CONVENTION_KEY.into(),
            Value::String(identity.query_convention.clone()),
        );
    }
    meta.insert(MANIFEST_EMBEDDER_KEY.into(), Value::Object(obj));
    // Drop legacy flat keys if present so the manifest canonicalizes
    // on the next flush. Harmless no-op when they're absent.
    meta.remove(LEGACY_FLAT_NAME);
    meta.remove(LEGACY_FLAT_DIMENSIONS);
    meta.remove(LEGACY_FLAT_NORMALIZE);
}

/// The error for an embedder that isn't the one the file records
/// ([`MnemoError::EmbedderMismatch`]). Its message starts with
/// [`EmbedderIdentity::MISMATCH_MSG`], so callers that grep the string
/// (Python's `RuntimeError`) keep working.
pub(crate) fn identity_mismatch_err(
    stored: &EmbedderIdentity,
    given: &EmbedderIdentity,
) -> MnemoError {
    MnemoError::EmbedderMismatch {
        stored: Box::new(stored.clone()),
        given: Box::new(given.clone()),
    }
}

/// EMBED-ID-1: the reserved result-cache namespace that holds a file's
/// embedder identity. Every file (manifest or not) records the first
/// embedder attached to it here. The public cache API refuses
/// [`RESERVED_CACHE_PREFIX`] namespaces and leaves them out of its stats
/// and purges; compaction, the rekeyed exports and the import copy the
/// entry like any other cache entry, so the identity follows the file.
pub(crate) const IDENTITY_NAMESPACE: &str = "_mnemo.embedder";
/// The key of the identity entry in [`IDENTITY_NAMESPACE`].
pub(crate) const IDENTITY_KEY: &str = "identity";
/// Cache namespaces with this prefix belong to the engine.
pub(crate) const RESERVED_CACHE_PREFIX: &str = "_mnemo.";

/// Whether `namespace` is one of the engine's own cache namespaces.
pub(crate) fn is_reserved_namespace(namespace: &str) -> bool {
    namespace.starts_with(RESERVED_CACHE_PREFIX)
}

/// The identity as the JSON the identity entry stores.
pub(crate) fn identity_to_json(identity: &EmbedderIdentity) -> Vec<u8> {
    let mut v = serde_json::json!({
        "name": identity.name,
        "dimensions": identity.dimensions as u64,
        "normalize": identity.normalize,
    });
    if !identity.query_convention.is_empty() {
        v[QUERY_CONVENTION_KEY] = Value::String(identity.query_convention.clone());
    }
    serde_json::to_vec(&v).expect("a JSON object of strings, a number and a bool serializes")
}

/// The identity from the identity entry's JSON, or `None` if it's
/// malformed.
pub(crate) fn identity_from_json(bytes: &[u8]) -> Option<EmbedderIdentity> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let obj = v.as_object()?;
    Some(EmbedderIdentity {
        name: obj.get("name")?.as_str()?.to_string(),
        dimensions: obj.get("dimensions")?.as_u64()? as usize,
        normalize: obj.get("normalize")?.as_bool()?,
        query_convention: convention_from(obj)?,
    })
}

/// The error for a file whose manifest and identity entry disagree (a
/// file changed by hand, or by two builds that saw different embedders).
/// It names both; neither embedder is attached.
pub(crate) fn identity_conflict_err(
    manifest: &EmbedderIdentity,
    record: &EmbedderIdentity,
) -> MnemoError {
    MnemoError::Invalid(format!(
        "{}: this file's manifest records embedder {}, but its identity record holds {}; \
         fix the manifest (`mnemo update`) to match the embedder its vectors came from",
        EmbedderIdentity::MISMATCH_MSG,
        manifest.describe(),
        record.describe(),
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
    fn read_identity_prefers_nested_over_legacy() {
        // Canonical nested form.
        let json = serde_json::json!({
            "area": "onboarding",
            "embedder": { "name": "nested", "dimensions": 384, "normalize": true },
            // Garbage legacy keys — must be ignored while the nested
            // form is present.
            "embedder_name": "legacy",
            "embedder_dimensions": 999,
            "embedder_normalize": false,
        });
        let obj = json.as_object().unwrap().clone();
        let id = read_identity_from_metadata(&obj).expect("nested identity read");
        assert_eq!(id.name, "nested");
        assert_eq!(id.dimensions, 384);
        assert!(id.normalize);
    }

    #[test]
    fn read_identity_falls_back_to_legacy_flat() {
        let json = serde_json::json!({
            "embedder_name": "legacy",
            "embedder_dimensions": 128,
            "embedder_normalize": false,
        });
        let obj = json.as_object().unwrap().clone();
        let id = read_identity_from_metadata(&obj).expect("legacy identity read");
        assert_eq!(id.name, "legacy");
        assert_eq!(id.dimensions, 128);
        assert!(!id.normalize);
    }

    #[test]
    fn read_identity_none_when_absent_or_malformed() {
        // Empty.
        assert!(read_identity_from_metadata(&Map::new()).is_none());
        // Nested missing a field.
        let json = serde_json::json!({ "embedder": { "name": "x", "dimensions": 4 } });
        let obj = json.as_object().unwrap().clone();
        assert!(read_identity_from_metadata(&obj).is_none());
    }

    #[test]
    fn write_identity_canonicalizes_and_drops_legacy() {
        let json = serde_json::json!({
            "embedder_name": "old",
            "embedder_dimensions": 128,
            "embedder_normalize": false,
            "unrelated": "keep-me",
        });
        let mut obj = json.as_object().unwrap().clone();
        write_identity_into_metadata(
            &mut obj,
            &EmbedderIdentity {
                name: "new".into(),
                dimensions: 384,
                normalize: true,
                ..Default::default()
            },
        );
        // Legacy keys gone.
        assert!(!obj.contains_key(LEGACY_FLAT_NAME));
        assert!(!obj.contains_key(LEGACY_FLAT_DIMENSIONS));
        assert!(!obj.contains_key(LEGACY_FLAT_NORMALIZE));
        // Unrelated metadata preserved.
        assert_eq!(obj["unrelated"], serde_json::json!("keep-me"));
        // Nested identity written correctly.
        let nested = obj["embedder"].as_object().unwrap();
        assert_eq!(nested["name"], serde_json::json!("new"));
        assert_eq!(nested["dimensions"], serde_json::json!(384));
        assert_eq!(nested["normalize"], serde_json::json!(true));
    }

    #[test]
    fn mismatch_err_includes_both_identities() {
        let want = EmbedderIdentity {
            name: "A".into(),
            dimensions: 4,
            normalize: true,
            ..Default::default()
        };
        let saw = EmbedderIdentity {
            name: "B".into(),
            dimensions: 4,
            normalize: false,
            ..Default::default()
        };
        let msg = format!("{}", identity_mismatch_err(&want, &saw));
        assert!(msg.contains(EmbedderIdentity::MISMATCH_MSG));
        assert!(msg.contains("name=\"A\""));
        assert!(msg.contains("name=\"B\""));
    }
}
