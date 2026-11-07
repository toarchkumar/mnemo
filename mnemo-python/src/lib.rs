//! Python bindings for Mnemo, the encrypted single-file agent-memory engine.
//!
//! This crate is a thin PyO3 adapter over the `mnemo` Rust core (imported here
//! as `mnemo_core`). It exposes one module-level function, [`open`], and one
//! class, [`Mnemo`]; everything else is plain method delegation plus
//! conversion between Rust and Python value types.
//!
//! From Python:
//!
//! ```python
//! import mnemo
//! db = mnemo.open("agent.mnemo", "passphrase", dimensions=4)
//! db.remember("the user likes tea", "semantic", [0.1, 0.2, 0.3, 0.4])
//! hits = db.recall([0.1, 0.2, 0.3, 0.4], top_k=5)
//! db.close()
//! ```

// Many methods chain `?` on helper calls that already return `PyResult<_>`
// (e.g. `parse_type(s)?`, `parse_id(s)?`, `key.extract::<String>()?`). When
// the inner and outer error types are both `PyErr`, `?` desugars to
// `<PyErr as From<PyErr>>::from`, the no-op blanket identity impl. Clippy
// ≥1.95 flags every such site as `useless_conversion` and reports the
// diagnostic on the function return type, which is misleading: the
// conversion is part of `?`'s sugar, not user code we can remove. Rewriting
// each call site to `match` would add noise without changing semantics.
#![allow(clippy::useless_conversion)]

use pyo3::exceptions::{PyRuntimeError, PyTypeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use mnemo_core::{
    CacheBudget, CacheFlushPolicy, Embedder, EmbedderIdentity, EssenceScope as CoreEssenceScope,
    EssenceSection, Filter as CoreFilter, KdfParams, Memory, MemoryPatch, MemoryType, Metric,
    Mnemo as Core, MnemoConfig, RecallRequest, Scope, ThreadTurn, Ulid, WriteOutcome,
};

// --- error / value conversion -------------------------------------------

/// Map a core error onto a Python exception.
fn to_py(e: mnemo_core::MnemoError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Phase 12.6: parse a Python filter dict into a core [`CoreFilter`].
///
/// Shape (same as the MCP tool):
/// ```python
/// filter={
///     "session_id": "sess-1",
///     "metadata": {
///         "area": "onboarding",                 # equality
///         "tag":  {"$in": ["a", "b"]},
///         "notes": {"$exists": True},
///         "score": {"$gte": 5, "$lte": 10},
///     },
/// }
/// ```
fn filter_from_py_dict(d: &Bound<'_, PyDict>) -> PyResult<CoreFilter> {
    let mut f = CoreFilter::default();
    if let Some(v) = d.get_item("session_id")? {
        let s: String = v.extract()?;
        f = f.with_session(s);
    }
    if let Some(meta) = d.get_item("metadata")? {
        let meta: Bound<'_, PyDict> = meta.downcast_into::<PyDict>().map_err(|_| {
            PyTypeError::new_err("filter.metadata must be a dict")
        })?;
        for (k, v) in meta.iter() {
            let key: String = k.extract()?;
            // Nested operator dict? Extract and dispatch.
            if let Ok(op_dict) = v.downcast::<PyDict>() {
                let mut saw_known = false;
                let mut min: Option<f64> = None;
                let mut max: Option<f64> = None;
                let mut range_pending = false;
                for (ok, ov) in op_dict.iter() {
                    let op: String = ok.extract()?;
                    match op.as_str() {
                        "$exists" => {
                            saw_known = true;
                            let b: bool = ov.extract()?;
                            if b {
                                f = f.metadata_exists(key.clone());
                            } else {
                                return Err(PyValueError::new_err(format!(
                                    "filter.metadata.{key}.$exists=False isn't supported; \
                                     drop the term instead"
                                )));
                            }
                        }
                        "$in" => {
                            saw_known = true;
                            let v_json = py_to_json(&ov)?;
                            let arr = match v_json {
                                serde_json::Value::Array(a) => a,
                                _ => {
                                    return Err(PyValueError::new_err(format!(
                                        "filter.metadata.{key}.$in must be a list"
                                    )));
                                }
                            };
                            f = f.metadata_in(key.clone(), arr);
                        }
                        "$gte" => {
                            saw_known = true;
                            range_pending = true;
                            min = Some(ov.extract()?);
                        }
                        "$lte" => {
                            saw_known = true;
                            range_pending = true;
                            max = Some(ov.extract()?);
                        }
                        other => {
                            return Err(PyValueError::new_err(format!(
                                "filter.metadata.{key}: unknown operator '{other}' \
                                 (want $exists, $in, $gte, $lte)"
                            )));
                        }
                    }
                }
                if range_pending {
                    f = f.metadata_range(key.clone(), min, max);
                }
                if !saw_known {
                    // Empty operator dict — fall back to equality against
                    // an empty dict (unlikely but well-defined).
                    f = f.metadata_eq(key.clone(), py_to_json(&v)?);
                }
            } else {
                // Plain scalar / list / string → equality.
                f = f.metadata_eq(key.clone(), py_to_json(&v)?);
            }
        }
    }
    Ok(f)
}

/// Phase 12.7b: map `(thread, project)` kwargs to a [`CoreEssenceScope`].
/// Exactly one of `thread` and `project` must be set.
fn essence_scope_from(
    thread: Option<String>,
    project: bool,
) -> PyResult<CoreEssenceScope> {
    match (thread, project) {
        (Some(t), false) => Ok(CoreEssenceScope::Thread(t)),
        (None, true) => Ok(CoreEssenceScope::Project),
        (None, false) => Err(PyValueError::new_err(
            "essence: pass either thread=<id> or project=True",
        )),
        (Some(_), true) => Err(PyValueError::new_err(
            "essence: `thread` and `project` are mutually exclusive",
        )),
    }
}

/// Phase 12.1: write-outcome enum → stable Python string.
fn write_outcome_str(o: WriteOutcome) -> &'static str {
    match o {
        WriteOutcome::Added => "added",
        WriteOutcome::Superseded => "superseded",
        WriteOutcome::Unchanged => "unchanged",
        WriteOutcome::Updated => "updated",
    }
}

/// Parse a memory-type string, raising `ValueError` on an unknown value.
fn parse_type(s: &str) -> PyResult<MemoryType> {
    MemoryType::parse(s).ok_or_else(|| PyValueError::new_err(format!("unknown memory_type '{s}'")))
}

/// Parse a ULID string, raising `ValueError` on a malformed id.
fn parse_id(s: &str) -> PyResult<Ulid> {
    Ulid::from_string(s).map_err(|_| PyValueError::new_err(format!("invalid memory id '{s}'")))
}

/// Accept either `str` (UTF-8 encoded) or `bytes` from Python and
/// return a `Vec<u8>`. Used by `cache_put` / `cache_put_semantic` so
/// callers don't have to pre-encode text values.
fn py_value_to_bytes(v: &pyo3::Bound<'_, pyo3::PyAny>) -> PyResult<Vec<u8>> {
    if let Ok(s) = v.extract::<String>() {
        return Ok(s.into_bytes());
    }
    if let Ok(b) = v.extract::<Vec<u8>>() {
        return Ok(b);
    }
    Err(PyTypeError::new_err("cache value must be str or bytes"))
}

/// Convert a `serde_json::Value` into the equivalent Python object.
fn json_to_py(py: Python<'_>, v: &serde_json::Value) -> PyResult<PyObject> {
    use serde_json::Value;
    Ok(match v {
        Value::Null => py.None(),
        Value::Bool(b) => b.into_py(py),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_py(py)
            } else if let Some(u) = n.as_u64() {
                u.into_py(py)
            } else {
                n.as_f64().unwrap_or(0.0).into_py(py)
            }
        }
        Value::String(s) => s.into_py(py),
        Value::Array(a) => {
            let list = PyList::empty_bound(py);
            for item in a {
                list.append(json_to_py(py, item)?)?;
            }
            list.into_any().unbind()
        }
        Value::Object(o) => {
            let d = PyDict::new_bound(py);
            for (k, val) in o {
                d.set_item(k, json_to_py(py, val)?)?;
            }
            d.into_any().unbind()
        }
    })
}

/// Convert a Python object into a `serde_json::Value` (used for metadata).
fn py_to_json(obj: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    use serde_json::Value;
    if obj.is_none() {
        return Ok(Value::Null);
    }
    // bool must be checked before int — Python's bool is an int subclass.
    if let Ok(b) = obj.extract::<bool>() {
        return Ok(Value::Bool(b));
    }
    if let Ok(i) = obj.extract::<i64>() {
        return Ok(Value::from(i));
    }
    if let Ok(f) = obj.extract::<f64>() {
        return Ok(Value::from(f));
    }
    if let Ok(s) = obj.extract::<String>() {
        return Ok(Value::String(s));
    }
    if let Ok(list) = obj.downcast::<PyList>() {
        let mut arr = Vec::with_capacity(list.len());
        for item in list.iter() {
            arr.push(py_to_json(&item)?);
        }
        return Ok(Value::Array(arr));
    }
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let mut map = serde_json::Map::new();
        for (k, val) in dict.iter() {
            map.insert(k.extract::<String>()?, py_to_json(&val)?);
        }
        return Ok(Value::Object(map));
    }
    Err(PyTypeError::new_err(
        "metadata values must be null, bool, int, float, str, list, or dict",
    ))
}

/// Render a [`Memory`] as a Python dict.
fn memory_to_dict(py: Python<'_>, m: &Memory) -> PyResult<PyObject> {
    let d = PyDict::new_bound(py);
    d.set_item("id", m.id.to_string())?;
    d.set_item("content", &m.content)?;
    d.set_item("memory_type", m.memory_type.as_str())?;
    d.set_item("vector", m.vector.clone())?;
    d.set_item("agent_id", &m.agent_id)?;
    d.set_item("session_id", m.session_id.clone())?;
    d.set_item(
        "scope",
        match m.scope {
            Scope::Private => "private",
            Scope::Shared => "shared",
        },
    )?;
    d.set_item("created_at", m.created_at)?;
    d.set_item("accessed_at", m.accessed_at)?;
    d.set_item("access_count", m.access_count)?;
    d.set_item("importance", m.importance)?;
    d.set_item("ttl_secs", m.ttl_secs)?;
    let meta = PyDict::new_bound(py);
    for (k, v) in &m.metadata {
        meta.set_item(k, json_to_py(py, v)?)?;
    }
    d.set_item("metadata", meta)?;
    Ok(d.into_any().unbind())
}

// --- Embedder bridge (Phase 3 PR 10a) -----------------------------------

/// Wraps a Python callable `fn(texts: list[str]) -> list[list[float]]`
/// so the Rust core can treat it as any other `Embedder`. The identity
/// is captured once at registration (so the Rust-side manifest-check
/// path doesn't re-enter Python unnecessarily) and the callable is
/// invoked under the GIL on each `embed`.
///
/// Error-path convention: a Python exception raised inside the
/// callback is converted into `MnemoError::Invalid` with the Python
/// error string — the engine then surfaces that as a `RuntimeError`
/// to the next Python frame via `to_py`.
struct PyFnEmbedder {
    identity: EmbedderIdentity,
    callback: Py<PyAny>,
}

impl Embedder for PyFnEmbedder {
    fn identity(&self) -> EmbedderIdentity {
        self.identity.clone()
    }

    fn embed(&self, texts: &[&str]) -> mnemo_core::Result<Vec<Vec<f32>>> {
        Python::with_gil(|py| {
            // Pass a Python list of strs; keeps the common
            // `sentence-transformers` / OpenAI shape natural.
            let arg = pyo3::types::PyList::new_bound(py, texts);
            let out = self
                .callback
                .call1(py, (arg,))
                .map_err(|e| mnemo_core::MnemoError::Invalid(format!("embedder callback raised: {e}")))?;
            let vecs: Vec<Vec<f32>> = out.extract(py).map_err(|e| {
                mnemo_core::MnemoError::Invalid(format!(
                    "embedder must return list[list[float]]; got {e}"
                ))
            })?;
            Ok(vecs)
        })
    }
}

// --- the Mnemo class -----------------------------------------------------

/// An open, encrypted Mnemo database.
#[pyclass]
struct Mnemo {
    /// The Rust core. Wrapped in `Option` so `close()` can `take()` and
    /// drop it — dropping releases the OS advisory file lock. Pre-PY-1
    /// this was a plain `Core`, which meant `close()` only flushed and
    /// the lock stayed held until Python's GC collected the object.
    /// A quick `db.close(); reopen(path)` in the same process raised
    /// `Locked`; only `del db` released the lock. PY-1 fixes that.
    inner: Option<Core>,
    path: String,
    /// Mirror of the current cache flush policy so `cache_put` and
    /// friends know whether to force an eager `flush()` (the historic
    /// Python behavior under `Strict`) or defer to the engine's
    /// batched auto-flush. Set by `set_cache_flush_policy`; defaults
    /// to `false` (i.e. `Strict`, eager flush) so existing callers
    /// see unchanged semantics.
    batched_cache_flush: bool,
}

impl Mnemo {
    /// Borrow the Rust core, raising a Python `RuntimeError: database
    /// is closed` if `close()` or `__exit__` has already released it.
    /// PY-1: every public method routes through here (or `core_mut`)
    /// so a post-close call gives a clear error instead of a panic or
    /// "already borrowed".
    fn core(&self) -> PyResult<&Core> {
        self.inner
            .as_ref()
            .ok_or_else(|| PyRuntimeError::new_err("database is closed"))
    }

    /// Mutable counterpart of [`core`].
    fn core_mut(&mut self) -> PyResult<&mut Core> {
        self.inner
            .as_mut()
            .ok_or_else(|| PyRuntimeError::new_err("database is closed"))
    }

    /// Flush the store unless the caller opted into the batched
    /// cache-flush lane (see `set_cache_flush_policy`). Called by
    /// every cache mutation (`cache_put`, `cache_delete`,
    /// `cache_purge`, `cache_put_semantic`) so those methods retain
    /// their historic "durable on return" semantics under the default
    /// `Strict` policy but let the engine's own auto-flush decide
    /// under `Batched`.
    fn maybe_flush_after_cache_write(&mut self) -> PyResult<()> {
        if self.batched_cache_flush {
            // Engine's `should_auto_flush` already fired inside the
            // preceding cache call if the batched thresholds tripped;
            // nothing to do here.
            Ok(())
        } else {
            self.core_mut()?.flush().map_err(to_py)
        }
    }
}

#[pymethods]
impl Mnemo {
    /// Store a memory; returns its ULID as a string.
    ///
    /// Phase 12.1: when `key` is set the write takes the keyed-upsert
    /// path (`remember_keyed`) and returns a `{"id": str, "outcome":
    /// str}` dict instead of a bare ULID string. `outcome` is one of
    /// `"added"`, `"superseded"`, `"unchanged"`. Without `key` the
    /// return value is still just the ULID string for backward
    /// compatibility.
    #[pyo3(signature = (content, memory_type, vector, *, agent_id=None,
                        importance=None, session_id=None, ttl_secs=None,
                        shared=false, metadata=None, key=None))]
    #[allow(clippy::too_many_arguments)]
    fn remember(
        &mut self,
        py: Python<'_>,
        content: String,
        memory_type: &str,
        vector: Vec<f32>,
        agent_id: Option<String>,
        importance: Option<f32>,
        session_id: Option<String>,
        ttl_secs: Option<i64>,
        shared: bool,
        metadata: Option<Bound<'_, PyDict>>,
        key: Option<String>,
    ) -> PyResult<PyObject> {
        let mut memory = Memory::new(content, parse_type(memory_type)?, vector);
        if let Some(a) = agent_id {
            memory = memory.with_agent(a);
        }
        if let Some(i) = importance {
            memory = memory.with_importance(i);
        }
        if let Some(s) = session_id {
            memory = memory.with_session(s);
        }
        if let Some(t) = ttl_secs {
            memory = memory.with_ttl(t);
        }
        if shared {
            memory = memory.with_scope(Scope::Shared);
        }
        if let Some(dict) = metadata {
            for (k, v) in dict.iter() {
                memory
                    .metadata
                    .insert(k.extract::<String>()?, py_to_json(&v)?);
            }
        }
        match key {
            None => {
                let id = self.core_mut()?.remember(memory).map_err(to_py)?;
                Ok(id.to_string().into_py(py))
            }
            Some(k) => {
                let (id, outcome) = self
                    .core_mut()?
                    .remember_keyed(memory, &k)
                    .map_err(to_py)?;
                let out = PyDict::new_bound(py);
                out.set_item("id", id.to_string())?;
                out.set_item("outcome", write_outcome_str(outcome))?;
                Ok(out.into_any().unbind())
            }
        }
    }

    /// Register a text embedder for this handle (Phase 3 PR 10a).
    ///
    /// `fn_` is any Python callable with the shape
    /// `embed(texts: list[str]) -> list[list[float]]`. `name`,
    /// `dimensions`, and `normalize` describe the embedder's identity;
    /// they're validated against the scaffold manifest on registration
    /// and persisted there on first registration so a subsequent open
    /// catches an embedder swap.
    ///
    /// Raises `RuntimeError` on dimension mismatch
    /// (`dimensions != db.dimensions()`) or on an identity mismatch
    /// with the manifest.
    #[pyo3(signature = (fn_, *, name, dimensions, normalize=true))]
    fn set_embedder(
        &mut self,
        fn_: Py<PyAny>,
        name: String,
        dimensions: usize,
        normalize: bool,
    ) -> PyResult<()> {
        let identity = EmbedderIdentity {
            name,
            dimensions,
            normalize,
        };
        let embedder = PyFnEmbedder {
            identity,
            callback: fn_,
        };
        self.core_mut()?.set_embedder(embedder).map_err(to_py)
    }

    /// Embed `content` with the registered embedder and `remember` the
    /// result. Accepts the same per-memory kwargs as `remember` so
    /// callers can migrate code wholesale: swap `remember(content,
    /// type, vector, …)` for `remember_text(content, type, …)` and
    /// drop the explicit vector.
    ///
    /// Raises `RuntimeError` if no embedder is registered (call
    /// `set_embedder` first), if the embedder returned the wrong
    /// vector width, or if the callback raised a Python exception.
    #[pyo3(signature = (content, memory_type, *, agent_id=None,
                        importance=None, session_id=None, ttl_secs=None,
                        shared=false, metadata=None, key=None))]
    #[allow(clippy::too_many_arguments)]
    fn remember_text(
        slf: Py<Self>,
        py: Python<'_>,
        content: String,
        memory_type: &str,
        agent_id: Option<String>,
        importance: Option<f32>,
        session_id: Option<String>,
        ttl_secs: Option<i64>,
        shared: bool,
        metadata: Option<Bound<'_, PyDict>>,
        key: Option<String>,
    ) -> PyResult<PyObject> {
        let mt = parse_type(memory_type)?;
        // PY-1 (R-001.11): take `slf: Py<Self>` instead of `&mut self` so
        // the PyO3-level `PyRefMut` does NOT live across the embedder
        // callback. We open a short `PyRef` to clone the embedder Arc,
        // drop it, invoke `embed()` with no borrow held, then open a
        // fresh `PyRefMut` to insert the memory. A Python embedder that
        // re-enters `db` (e.g. calls `db.dimensions()` for shape
        // probing or `db.cache_get(key)` for a dedupe check) now sees
        // a borrow-free Python object and succeeds; pre-PY-1 the
        // `&mut self` receiver kept the Python-level mutable borrow
        // alive for the whole call, and the re-entrant call raised
        // "Already mutably borrowed".
        let embedder = {
            let s = slf.borrow(py);
            s.core()?
                .embedder_arc()
                .ok_or_else(|| PyRuntimeError::new_err("no embedder registered on this handle"))?
        }; // PyRef drops here
        let vector = {
            let mut vs = embedder.embed(&[content.as_str()]).map_err(to_py)?;
            vs.pop().ok_or_else(|| {
                PyRuntimeError::new_err("embedder returned no vector for the input text")
            })?
        };
        let mut memory = Memory::new(content, mt, vector);
        if let Some(a) = agent_id {
            memory = memory.with_agent(a);
        }
        if let Some(i) = importance {
            memory = memory.with_importance(i);
        }
        if let Some(s) = session_id {
            memory = memory.with_session(s);
        }
        if let Some(t) = ttl_secs {
            memory = memory.with_ttl(t);
        }
        if shared {
            memory = memory.with_scope(Scope::Shared);
        }
        if let Some(dict) = metadata {
            for (k, v) in dict.iter() {
                memory
                    .metadata
                    .insert(k.extract::<String>()?, py_to_json(&v)?);
            }
        }
        let mut s = slf.borrow_mut(py);
        match key {
            None => {
                let id = s.core_mut()?.remember(memory).map_err(to_py)?;
                Ok(id.to_string().into_py(py))
            }
            Some(k) => {
                let (id, outcome) = s
                    .core_mut()?
                    .remember_keyed(memory, &k)
                    .map_err(to_py)?;
                let out = PyDict::new_bound(py);
                out.set_item("id", id.to_string())?;
                out.set_item("outcome", write_outcome_str(outcome))?;
                Ok(out.into_any().unbind())
            }
        }
    }

    /// Phase 12.1: edit a memory in place, preserving its ULID,
    /// `created_at` and access stats. Returns a dict
    /// `{"outcome": "updated"|"unchanged"}`. A patch whose fields all
    /// equal the live record is a no-op.
    ///
    /// A `content` change requires a matching `vector` (or use
    /// `update_text` to re-embed through the registered embedder).
    /// `metadata` fully replaces the user-visible metadata; reserved
    /// `_mnemo.*` keys on the record are preserved.
    ///
    /// `ttl_secs`:
    /// * `None` → leave the TTL unchanged,
    /// * an integer → set the TTL to that many seconds,
    /// * pass `clear_ttl=True` to drop any TTL.
    #[pyo3(signature = (id, *, content=None, vector=None, importance=None,
                        metadata=None, memory_type=None, ttl_secs=None, clear_ttl=false))]
    #[allow(clippy::too_many_arguments)]
    fn update(
        &mut self,
        py: Python<'_>,
        id: &str,
        content: Option<String>,
        vector: Option<Vec<f32>>,
        importance: Option<f32>,
        metadata: Option<Bound<'_, PyDict>>,
        memory_type: Option<&str>,
        ttl_secs: Option<i64>,
        clear_ttl: bool,
    ) -> PyResult<PyObject> {
        let ulid = Ulid::from_string(id)
            .map_err(|_| PyValueError::new_err(format!("invalid ULID '{id}'")))?;
        let mut patch = MemoryPatch {
            content,
            vector,
            importance,
            memory_type: memory_type.map(parse_type).transpose()?,
            ..Default::default()
        };
        if let Some(dict) = metadata {
            let mut map = serde_json::Map::new();
            for (k, v) in dict.iter() {
                map.insert(k.extract::<String>()?, py_to_json(&v)?);
            }
            patch.metadata = Some(map);
        }
        if clear_ttl {
            patch.ttl_secs = Some(None);
        } else if let Some(t) = ttl_secs {
            patch.ttl_secs = Some(Some(t));
        }
        let outcome = self.core_mut()?.update(&ulid, patch).map_err(to_py)?;
        let out = PyDict::new_bound(py);
        out.set_item("id", id.to_string())?;
        out.set_item("outcome", write_outcome_str(outcome))?;
        Ok(out.into_any().unbind())
    }

    /// Phase 12.1: in-place update that re-embeds `new_text` through
    /// the registered embedder. Shorthand for `update(id,
    /// content=new_text, vector=embedder.embed([new_text])[0], ...)`.
    /// Other kwargs behave as in [`Mnemo::update`].
    #[pyo3(signature = (id, new_text, *, importance=None, metadata=None,
                        memory_type=None, ttl_secs=None, clear_ttl=false))]
    #[allow(clippy::too_many_arguments)]
    fn update_text(
        slf: Py<Self>,
        py: Python<'_>,
        id: &str,
        new_text: String,
        importance: Option<f32>,
        metadata: Option<Bound<'_, PyDict>>,
        memory_type: Option<&str>,
        ttl_secs: Option<i64>,
        clear_ttl: bool,
    ) -> PyResult<PyObject> {
        let ulid = Ulid::from_string(id)
            .map_err(|_| PyValueError::new_err(format!("invalid ULID '{id}'")))?;
        // PY-1 (R-001.11): clone embedder Arc under a short PyRef, drop
        // the borrow, then take a fresh PyRefMut for the update call.
        let embedder = {
            let s = slf.borrow(py);
            s.core()?
                .embedder_arc()
                .ok_or_else(|| PyRuntimeError::new_err("no embedder registered on this handle"))?
        };
        let vector = {
            let mut vs = embedder.embed(&[new_text.as_str()]).map_err(to_py)?;
            vs.pop().ok_or_else(|| {
                PyRuntimeError::new_err("embedder returned no vector for the input text")
            })?
        };
        let mut patch = MemoryPatch {
            content: Some(new_text),
            vector: Some(vector),
            importance,
            memory_type: memory_type.map(parse_type).transpose()?,
            ..Default::default()
        };
        if let Some(dict) = metadata {
            let mut map = serde_json::Map::new();
            for (k, v) in dict.iter() {
                map.insert(k.extract::<String>()?, py_to_json(&v)?);
            }
            patch.metadata = Some(map);
        }
        if clear_ttl {
            patch.ttl_secs = Some(None);
        } else if let Some(t) = ttl_secs {
            patch.ttl_secs = Some(Some(t));
        }
        let mut s = slf.borrow_mut(py);
        let outcome = s.core_mut()?.update(&ulid, patch).map_err(to_py)?;
        let out = PyDict::new_bound(py);
        out.set_item("id", id.to_string())?;
        out.set_item("outcome", write_outcome_str(outcome))?;
        Ok(out.into_any().unbind())
    }

    /// Phase 12.1: walk the full chain for a keyed slot, newest first.
    /// Entry 0 is the live record (or an empty list if the slot is
    /// empty); subsequent entries are the superseded history.
    ///
    /// Slot resolution (v0.8.1 bugfix): matches keyed `remember` with
    /// the same arguments.
    ///
    /// * `agent_id=None, shared=False` → default agent's private slot
    ///   (same slot `remember(key=K)` writes to when `agent_id` is
    ///   omitted).
    /// * `agent_id="alice"` → alice's private slot.
    /// * `shared=True` → shared slot. Mutually exclusive with
    ///   `agent_id`.
    #[pyo3(signature = (key, *, agent_id=None, shared=false))]
    fn history(
        &mut self,
        py: Python<'_>,
        key: &str,
        agent_id: Option<String>,
        shared: bool,
    ) -> PyResult<PyObject> {
        use mnemo_core::KeyScope;
        if shared && agent_id.is_some() {
            return Err(PyValueError::new_err(
                "history: 'agent_id' and 'shared' are mutually exclusive",
            ));
        }
        let scope = if shared {
            KeyScope::Shared
        } else {
            match agent_id {
                Some(a) => KeyScope::Private(a),
                None => KeyScope::private_default(),
            }
        };
        let chain = self.core_mut()?.history_in(key, scope).map_err(to_py)?;
        let list = PyList::empty_bound(py);
        for (i, m) in chain.iter().enumerate() {
            let d_obj = memory_to_dict(py, m)?;
            let d = d_obj.downcast_bound::<PyDict>(py)?;
            d.set_item("state", if i == 0 { "live" } else { "superseded" })?;
            list.append(d)?;
        }
        Ok(list.into_any().unbind())
    }

    /// Phase 12.7a: ingest a batch of turns into `thread_id`'s archive.
    ///
    /// `turns` is a list of dicts; each dict needs `turn_id` (str),
    /// `role` (str), `content` (str) and may carry `timestamp` (int).
    /// Idempotent per `turn_id`. Returns a dict `{added_records,
    /// skipped_existing_turns, chunked_turns, truncated_turns}`.
    /// Requires `set_embedder` first.
    #[pyo3(signature = (thread_id, turns))]
    fn ingest_thread(
        &mut self,
        py: Python<'_>,
        thread_id: &str,
        turns: Vec<Bound<'_, PyDict>>,
    ) -> PyResult<PyObject> {
        let mut rust_turns: Vec<ThreadTurn> = Vec::with_capacity(turns.len());
        for (i, d) in turns.iter().enumerate() {
            let turn_id: String = d
                .get_item("turn_id")?
                .ok_or_else(|| {
                    PyValueError::new_err(format!(
                        "turns[{i}]: missing required key 'turn_id'"
                    ))
                })?
                .extract()?;
            let role: String = d
                .get_item("role")?
                .ok_or_else(|| {
                    PyValueError::new_err(format!("turns[{i}]: missing required key 'role'"))
                })?
                .extract()?;
            let content: String = d
                .get_item("content")?
                .ok_or_else(|| {
                    PyValueError::new_err(format!(
                        "turns[{i}]: missing required key 'content'"
                    ))
                })?
                .extract()?;
            let timestamp: Option<i64> =
                d.get_item("timestamp")?.and_then(|t| t.extract().ok());
            rust_turns.push(ThreadTurn {
                turn_id,
                role,
                content,
                timestamp,
            });
        }
        let report = self
            .core_mut()?
            .ingest_thread(thread_id, &rust_turns)
            .map_err(to_py)?;
        let out = PyDict::new_bound(py);
        out.set_item("added_records", report.added_records)?;
        out.set_item("skipped_existing_turns", report.skipped_existing_turns)?;
        out.set_item("chunked_turns", report.chunked_turns)?;
        out.set_item("truncated_turns", report.truncated_turns)?;
        Ok(out.into_any().unbind())
    }

    /// Phase 12.7a: export a thread's archive back as a list of
    /// `Memory` dicts, sorted by `_mnemo.seq` then `_mnemo.part`.
    /// Multi-part turns come out as separate entries — the CLI's
    /// `mnemo thread export` collapses them; Python callers are
    /// expected to walk `_mnemo.part` themselves when they need the
    /// per-chunk shape.
    #[pyo3(signature = (thread_id))]
    fn thread_export(&mut self, py: Python<'_>, thread_id: &str) -> PyResult<PyObject> {
        let records = self.core_mut()?.thread_export(thread_id).map_err(to_py)?;
        let list = PyList::empty_bound(py);
        for m in &records {
            list.append(memory_to_dict(py, m)?)?;
        }
        Ok(list.into_any().unbind())
    }

    /// Phase 12.7a: soft-delete every record in a thread's archive.
    /// Returns the number of records marked deleted. `db.compact()`
    /// (or `mnemo compact`) reclaims the space permanently.
    #[pyo3(signature = (thread_id))]
    fn thread_forget(&mut self, thread_id: &str) -> PyResult<usize> {
        self.core_mut()?.thread_forget(thread_id).map_err(to_py)
    }

    /// Phase 12.7b: add an essence section item.
    ///
    /// `section` is one of `intent`, `conventions`, `decisions`,
    /// `state`, `open`, `next`, `lessons`. Timeline entries use
    /// [`essence_append`]. Pass `thread=<id>` for a thread-scoped
    /// item, or `project=True` for a project-scoped one; exactly
    /// one of those is required. `refs` is an optional list of
    /// free-form strings. Returns the new record's ULID string.
    #[pyo3(signature = (section, text, *, thread=None, project=false, refs=None))]
    fn essence_add(
        &mut self,
        section: &str,
        text: &str,
        thread: Option<String>,
        project: bool,
        refs: Option<Vec<String>>,
    ) -> PyResult<String> {
        let section = EssenceSection::parse(section)
            .ok_or_else(|| PyValueError::new_err(format!("unknown section '{section}'")))?;
        if section == EssenceSection::Timeline {
            return Err(PyValueError::new_err(
                "essence_add: use essence_append for timeline entries",
            ));
        }
        let scope = essence_scope_from(thread, project)?;
        let id = self
            .core_mut()?
            .essence_add(scope, section, text, refs)
            .map_err(to_py)?;
        Ok(id.to_string())
    }

    /// Phase 12.7b: append a timeline entry (append-only, ≤ 600 bytes).
    #[pyo3(signature = (text, *, thread=None, project=false, refs=None))]
    fn essence_append(
        &mut self,
        text: &str,
        thread: Option<String>,
        project: bool,
        refs: Option<Vec<String>>,
    ) -> PyResult<String> {
        let scope = essence_scope_from(thread, project)?;
        let id = self
            .core_mut()?
            .essence_append_timeline(scope, text, refs)
            .map_err(to_py)?;
        Ok(id.to_string())
    }

    /// Phase 12.7b: update an essence section item's text (and optional
    /// refs). Returns a dict `{"id", "outcome"}` with outcome
    /// `"updated"` or `"unchanged"`.
    #[pyo3(signature = (id, text, *, refs=None))]
    fn essence_update(
        &mut self,
        py: Python<'_>,
        id: &str,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> PyResult<PyObject> {
        let ulid = Ulid::from_string(id)
            .map_err(|_| PyValueError::new_err(format!("invalid ULID '{id}'")))?;
        let outcome = self
            .core_mut()?
            .essence_update(&ulid, text, refs)
            .map_err(to_py)?;
        let out = PyDict::new_bound(py);
        out.set_item("id", id)?;
        out.set_item("outcome", write_outcome_str(outcome))?;
        Ok(out.into_any().unbind())
    }

    /// Phase 12.7b: resolve a section item (`open → resolved`).
    /// Idempotent: resolving an already-resolved item returns
    /// `{"outcome": "unchanged"}`.
    fn essence_resolve(&mut self, py: Python<'_>, id: &str) -> PyResult<PyObject> {
        let ulid = Ulid::from_string(id)
            .map_err(|_| PyValueError::new_err(format!("invalid ULID '{id}'")))?;
        let outcome = self.core_mut()?.essence_resolve(&ulid).map_err(to_py)?;
        let out = PyDict::new_bound(py);
        out.set_item("id", id)?;
        out.set_item("outcome", write_outcome_str(outcome))?;
        Ok(out.into_any().unbind())
    }

    /// Phase 12.7b: soft-delete an essence section item.
    fn essence_remove(&mut self, id: &str) -> PyResult<()> {
        let ulid = Ulid::from_string(id)
            .map_err(|_| PyValueError::new_err(format!("invalid ULID '{id}'")))?;
        self.core_mut()?.essence_remove(&ulid).map_err(to_py)
    }

    /// Phase 12.7b: render the essence layer as markdown. Pass
    /// `thread=<id>` or `project=True` (mutually exclusive). `budget`
    /// defaults to 6144 bytes.
    #[pyo3(signature = (*, thread=None, project=false, budget=6144))]
    fn essence(
        &mut self,
        thread: Option<String>,
        project: bool,
        budget: usize,
    ) -> PyResult<String> {
        let scope = essence_scope_from(thread, project)?;
        self.core_mut()?.essence(scope, budget).map_err(to_py)
    }

    /// Embed `query` with the registered embedder and run `recall`
    /// against the result. Mirrors the shape of `recall(query_vector,
    /// ...)` but takes a string. Raises if no embedder is registered.
    ///
    /// `n_probe` / `n_rerank` (PR ANN-1): per-query ANN index dials,
    /// matching the Rust `RecallRequest::n_probe` / `::n_rerank`
    /// builder methods. `None` (default) uses the defaults the index
    /// was built with (`IndexConfig::default` auto-formulas since
    /// v0.6.0: `n_probe = ceil(partitions / 2)`, `n_rerank = 500`;
    /// previously fixed `8` and `64`). Raise either dial for more
    /// recall at the cost of latency; drop them for the inverse. Only
    /// takes effect on a database that has an ANN index built;
    /// unindexed recall scans every live memory exactly.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (query, top_k=10, memory_types=None, agent_id=None,
                        track_access=true, n_probe=None, n_rerank=None,
                        filter=None, max_bytes=None))]
    fn recall_text(
        slf: Py<Self>,
        py: Python<'_>,
        query: &str,
        top_k: usize,
        memory_types: Option<Vec<String>>,
        agent_id: Option<String>,
        track_access: bool,
        n_probe: Option<usize>,
        n_rerank: Option<usize>,
        filter: Option<Bound<'_, PyDict>>,
        max_bytes: Option<usize>,
    ) -> PyResult<PyObject> {
        // PY-1 (R-001.11): take `slf: Py<Self>` so the Python-level
        // borrow drops before the embedder callback runs — same
        // pattern as `remember_text`, see the comment there.
        let embedder = {
            let s = slf.borrow(py);
            s.core()?
                .embedder_arc()
                .ok_or_else(|| PyRuntimeError::new_err("no embedder registered on this handle"))?
        }; // PyRef drops here
        let vector = {
            let mut vs = embedder.embed(&[query]).map_err(to_py)?;
            vs.pop().ok_or_else(|| {
                PyRuntimeError::new_err("embedder returned no vector for the query text")
            })?
        };
        let mut req = RecallRequest::new(vector)
            .top_k(top_k)
            .track_access(track_access);
        if let Some(types) = memory_types {
            let parsed: PyResult<Vec<MemoryType>> = types.iter().map(|t| parse_type(t)).collect();
            req = req.types(parsed?);
        }
        if let Some(a) = agent_id {
            req = req.agent(a);
        }
        if let Some(n) = n_probe {
            req = req.n_probe(n);
        }
        if let Some(n) = n_rerank {
            req = req.n_rerank(n);
        }
        if let Some(fd) = filter {
            req = req.filter(filter_from_py_dict(&fd)?);
        }
        let budgeted = max_bytes.is_some();
        if let Some(b) = max_bytes {
            req = req.max_bytes(b);
        }
        // PY-1 (R-001.11): release the GIL around the heavy recall
        // (ANN probe + rerank + per-hit decrypt). The subsequent
        // `memory_to_dict` loop re-enters Python so it stays outside
        // the `allow_threads` region.
        let mut s = slf.borrow_mut(py);
        let resp = {
            let core = s.core_mut()?;
            py.allow_threads(|| core.recall_budgeted(&req))
                .map_err(to_py)?
        };
        drop(s);
        let mut out = Vec::with_capacity(resp.hits.len());
        for h in resp.hits {
            let dict = memory_to_dict(py, &h.memory)?;
            let bound = dict.bind(py);
            bound.set_item("score", h.score)?;
            bound.set_item("similarity", h.similarity)?;
            out.push(dict);
        }
        // Phase 12.8: when `max_bytes` was set, return the wrapped
        // envelope `{hits, truncated}` so callers can tell whether
        // the budget cut anything; otherwise keep the pre-12.8 list
        // shape for backward compatibility.
        if budgeted {
            let wrap = PyDict::new_bound(py);
            let list = PyList::empty_bound(py);
            for h in out {
                list.append(h)?;
            }
            wrap.set_item("hits", list)?;
            wrap.set_item("truncated", resp.truncated)?;
            Ok(wrap.into_any().unbind())
        } else {
            let list = PyList::empty_bound(py);
            for h in out {
                list.append(h)?;
            }
            Ok(list.into_any().unbind())
        }
    }

    /// Multi-signal recall. Returns a list of result dicts, each a memory with
    /// added `score` and `similarity` keys, best first.
    ///
    /// `track_access` (default `True`) controls whether the catalog's
    /// `accessed_at` / `access_count` get bumped on the returned memories
    /// — pass `False` for a fully read-only recall that doesn't dirty the
    /// catalog, useful for batch scoring or dry-runs.
    ///
    /// `n_probe` / `n_rerank` (PR ANN-1): per-query ANN index dials.
    /// `None` (default) uses the defaults the index was built with.
    /// See [`recall_text`] for the full description.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (query, top_k=10, memory_types=None, agent_id=None,
                        track_access=true, n_probe=None, n_rerank=None,
                        filter=None, max_bytes=None))]
    fn recall(
        &mut self,
        py: Python<'_>,
        query: Vec<f32>,
        top_k: usize,
        memory_types: Option<Vec<String>>,
        agent_id: Option<String>,
        track_access: bool,
        n_probe: Option<usize>,
        n_rerank: Option<usize>,
        filter: Option<Bound<'_, PyDict>>,
        max_bytes: Option<usize>,
    ) -> PyResult<PyObject> {
        let mut req = RecallRequest::new(query)
            .top_k(top_k)
            .track_access(track_access);
        if let Some(types) = memory_types {
            let parsed: PyResult<Vec<MemoryType>> = types.iter().map(|t| parse_type(t)).collect();
            req = req.types(parsed?);
        }
        if let Some(a) = agent_id {
            req = req.agent(a);
        }
        if let Some(n) = n_probe {
            req = req.n_probe(n);
        }
        if let Some(n) = n_rerank {
            req = req.n_rerank(n);
        }
        if let Some(fd) = filter {
            req = req.filter(filter_from_py_dict(&fd)?);
        }
        let budgeted = max_bytes.is_some();
        if let Some(b) = max_bytes {
            req = req.max_bytes(b);
        }
        // PY-1 (R-001.11): same GIL release as `recall_text`.
        let resp = {
            let core = self.core_mut()?;
            py.allow_threads(|| core.recall_budgeted(&req))
                .map_err(to_py)?
        };
        let mut out = Vec::with_capacity(resp.hits.len());
        for h in resp.hits {
            let dict = memory_to_dict(py, &h.memory)?;
            let bound = dict.bind(py);
            bound.set_item("score", h.score)?;
            bound.set_item("similarity", h.similarity)?;
            out.push(dict);
        }
        if budgeted {
            let wrap = PyDict::new_bound(py);
            let list = PyList::empty_bound(py);
            for h in out {
                list.append(h)?;
            }
            wrap.set_item("hits", list)?;
            wrap.set_item("truncated", resp.truncated)?;
            Ok(wrap.into_any().unbind())
        } else {
            let list = PyList::empty_bound(py);
            for h in out {
                list.append(h)?;
            }
            Ok(list.into_any().unbind())
        }
    }

    /// Exact nearest-neighbour search; returns `(memory_dict, similarity)`
    /// pairs. Always exact — the ground-truth baseline for `recall`.
    #[pyo3(signature = (query, top_k=10))]
    fn search(
        &mut self,
        py: Python<'_>,
        query: Vec<f32>,
        top_k: usize,
    ) -> PyResult<Vec<(PyObject, f32)>> {
        // PY-1 (R-001.11): release the GIL around the exact scan.
        let hits = {
            let core = self.core_mut()?;
            py.allow_threads(|| core.search(&query, top_k, Metric::Cosine))
                .map_err(to_py)?
        };
        let mut out = Vec::with_capacity(hits.len());
        for (m, sim) in hits {
            out.push((memory_to_dict(py, &m)?, sim));
        }
        Ok(out)
    }

    /// Fetch one memory by id; raises `RuntimeError` if it does not exist.
    fn get(&mut self, py: Python<'_>, id: &str) -> PyResult<PyObject> {
        let m = self.core_mut()?.get(&parse_id(id)?).map_err(to_py)?;
        memory_to_dict(py, &m)
    }

    /// Soft-delete a memory by id.
    fn delete(&mut self, id: &str) -> PyResult<()> {
        self.core_mut()?.delete(&parse_id(id)?).map_err(to_py)
    }

    /// Enumerate every live (non-deleted, non-expired) memory as a
    /// list of dicts matching the shape of `get`/`recall` results.
    /// Order matches the on-disk catalog (roughly insertion order,
    /// minus deletions). Reads decrypt every record page — cheap
    /// for a small store, O(n) for a large one.
    fn memories(&mut self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let mut out = Vec::new();
        for m in self.core_mut()?.memories().map_err(to_py)? {
            out.push(memory_to_dict(py, &m)?);
        }
        Ok(out)
    }

    /// Number of live (non-deleted) memories.
    fn __len__(&self) -> PyResult<usize> {
        Ok(self.core()?.len())
    }

    /// Embedding dimensionality this database was created with. Also
    /// available via `stats()["dimensions"]`; exposed as its own
    /// method for callers who only need the number and want to skip
    /// the full stats dict.
    fn dimensions(&self) -> PyResult<usize> {
        Ok(self.core()?.dimensions())
    }

    /// Persist all pending changes as one atomic transaction.
    ///
    /// PY-1 (R-001.11): releases the GIL during the flush so another
    /// Python thread can make progress. A flush is the hot I/O path
    /// (WAL commit + checkpoint + 3 fsyncs since DUR-1), and under the
    /// pre-PY-1 "GIL held for the whole call" shape, a single
    /// background `db.flush()` would stall every other Python thread
    /// for the duration.
    fn flush(&mut self, py: Python<'_>) -> PyResult<()> {
        let core = self.core_mut()?;
        py.allow_threads(|| core.flush()).map_err(to_py)
    }

    /// Flush pending writes, then release the Rust core — which drops
    /// the OS file handle **and** the exclusive lock the writable
    /// handle holds. After `close()` the Python object is still alive
    /// but every method raises `RuntimeError: database is closed`;
    /// `close()` itself is idempotent.
    ///
    /// PY-1 (SEC-1 reproduction entry 2026-11-01 VERIFIED): pre-PY-1
    /// `close()` only called `Core::close` (which flushes but holds the
    /// handle), so `db.close()` followed by `reopen(path)` in the same
    /// process raised `Locked` until Python's GC collected the object.
    /// Now the core is wrapped in `Option` and `close` `take()`s +
    /// drops it, releasing the lock synchronously.
    fn close(&mut self) -> PyResult<()> {
        if let Some(mut core) = self.inner.take() {
            // Mirrors Core::close: flush on a writable handle, no-op on
            // a read-only one (close() itself does the mode check).
            core.close().map_err(to_py)?;
            // `core` drops here → File handle closes → OS advisory
            // lock releases synchronously.
        }
        Ok(())
    }

    /// Decrypt and re-validate every live record; returns the count checked.
    ///
    /// PY-1 (R-001.11): releases the GIL — `verify` decrypts every
    /// live memory and every live cache entry (FIX-1 R-001.1), which
    /// is O(N) AES-GCM on a potentially-large corpus.
    fn verify(&mut self, py: Python<'_>) -> PyResult<usize> {
        let core = self.core_mut()?;
        py.allow_threads(|| core.verify()).map_err(to_py)
    }

    /// Change the passphrase. Cheap: re-derives the KEK and re-wraps
    /// the DEK; the encrypted pages themselves are never rewritten,
    /// so this is O(1) in database size. Flushes any pending writes
    /// before rotating.
    ///
    /// `fast=True` uses `KdfParams::fast()` — the cheap Argon2 params
    /// intended for tests only. Production callers should leave it
    /// `False` so the new KEK inherits the same protection level as
    /// the file's original creation-time parameters.
    #[pyo3(signature = (new_passphrase, fast=false))]
    fn rekey(&mut self, py: Python<'_>, new_passphrase: &str, fast: bool) -> PyResult<()> {
        // FIX-1 R-001.5: `fast=False` now really does "inherit the
        // file's original protection level" as the docstring promises.
        // Pre-FIX-1 it reached for `KdfParams::default()` instead of
        // the file's actual params, so a file created with stronger
        // (or weaker) KDF cost silently reset to the engine defaults
        // on every rekey.
        let kdf = if fast {
            KdfParams::fast()
        } else {
            self.core()?.kdf_params()
        };
        // PY-1 (R-001.11): release the GIL — `rekey` runs Argon2id,
        // which on default parameters (`KdfParams::secure`) takes
        // ~50–100 ms. Holding the GIL for that stalls every other
        // Python thread for no reason.
        let core = self.core_mut()?;
        py.allow_threads(|| core.rekey(new_passphrase, kdf))
            .map_err(to_py)
    }

    /// Return the database's self-describing onboarding memories (those tagged
    /// `metadata.area = "onboarding"`), sorted by importance descending then
    /// created_at ascending. This is the engine-level entry point for a
    /// fresh agent to learn what the file is, which embedder it expects, and
    /// any other conventions the file's author chose to record — all without
    /// needing any external documentation. Each entry is returned as a dict
    /// matching the shape of `recall`/`get` results.
    fn about(&mut self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let mut out = Vec::new();
        for m in self.core_mut()?.about().map_err(to_py)? {
            out.push(memory_to_dict(py, &m)?);
        }
        Ok(out)
    }

    /// Insert the canonical scaffold manifest into this database. Mirrors what
    /// `mnemo init` does by default — useful when you create a database
    /// programmatically via `mnemo.open(path, pp, dimensions=N)` and want the
    /// same self-describing-by-default behavior. Returns the new memory's
    /// ULID as a string. Does not flush.
    fn insert_default_manifest(&mut self) -> PyResult<String> {
        // FIX-1 R-001.8 part 1: use the O(1) `dimensions()` accessor
        // instead of `stats().dimensions`. `stats()` historically
        // walked every live memory to build the agent list (fixed in
        // this PR too, but either way this call site never needed
        // more than the dimension field).
        let dims = self.core_mut()?.dimensions();
        let manifest = Memory::scaffold_manifest(dims);
        let id = self.core_mut()?.remember(manifest).map_err(to_py)?;
        Ok(id.to_string())
    }

    /// Build the IVF+PQ approximate index; `recall` then runs sub-linearly.
    ///
    /// PY-1 (R-001.11): releases the GIL — building an ANN index runs
    /// k-means clustering + PQ codebook training over every live
    /// vector, which is multi-second on 1M-vector corpora.
    fn build_index(&mut self, py: Python<'_>) -> PyResult<()> {
        let core = self.core_mut()?;
        py.allow_threads(|| core.build_index())
            .map(|_| ())
            .map_err(to_py)
    }

    /// Rebuild the ANN index from scratch using the auto-defaults for
    /// the current corpus size — re-clusters centroids and retrains
    /// the PQ codebook, undoing the cluster drift that accumulates as
    /// memories are inserted against fixed centroids. This is the
    /// command the v0.6.0 upgrade note tells users to run after
    /// upgrading from v0.5.x so an index built under the old
    /// `pq_subspaces=48` defaults picks up the ANN-1.1 formulas
    /// (`pq_subspaces = dims/4`, size-aware `n_probe` / `n_rerank`).
    ///
    /// BP.1: new in v0.6.1 — before, Python had no rebuild command and
    /// the CHANGELOG's `rebuild_index()` reference raised
    /// `AttributeError`. Works the same way as the CLI's
    /// `mnemo index <file> --rebuild`. Does not flush — call `flush()`
    /// yourself to persist the new index.
    ///
    /// Releases the GIL during the rebuild, same as `build_index`.
    fn rebuild_index(&mut self, py: Python<'_>) -> PyResult<()> {
        let core = self.core_mut()?;
        py.allow_threads(|| core.rebuild_index())
            .map(|_| ())
            .map_err(to_py)
    }

    /// Drop the approximate index; recall reverts to exact scan.
    fn drop_index(&mut self) -> PyResult<()> {
        self.core_mut()?.drop_index();
        Ok(())
    }

    /// Whether an approximate index is currently loaded.
    fn has_index(&self) -> PyResult<bool> {
        Ok(self.core()?.has_index())
    }

    /// List restorable snapshots — one dict per committed transaction.
    fn snapshots(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let mut out = Vec::new();
        for s in self.core()?.snapshots() {
            let d = PyDict::new_bound(py);
            d.set_item("txn_id", s.txn_id)?;
            d.set_item("created_at", s.created_at)?;
            d.set_item("memory_count", s.memory_count)?;
            out.push(d.into_any().unbind());
        }
        Ok(out)
    }

    /// Restore the database to the snapshot from transaction `txn_id`.
    fn restore_to(&mut self, py: Python<'_>, txn_id: u64) -> PyResult<PyObject> {
        let info = self.core_mut()?.restore_to(txn_id).map_err(to_py)?;
        let d = PyDict::new_bound(py);
        d.set_item("txn_id", info.txn_id)?;
        d.set_item("created_at", info.created_at)?;
        d.set_item("memory_count", info.memory_count)?;
        Ok(d.into_any().unbind())
    }

    /// Restore to the latest snapshot committed at or before `unix_secs`.
    fn restore_to_time(&mut self, py: Python<'_>, unix_secs: i64) -> PyResult<PyObject> {
        let info = self.core_mut()?.restore_to_time(unix_secs).map_err(to_py)?;
        let d = PyDict::new_bound(py);
        d.set_item("txn_id", info.txn_id)?;
        d.set_item("created_at", info.created_at)?;
        d.set_item("memory_count", info.memory_count)?;
        Ok(d.into_any().unbind())
    }

    /// Bound the page cache to `pages` decrypted pages.
    fn set_cache_capacity(&mut self, pages: usize) -> PyResult<()> {
        self.core_mut()?.set_cache_capacity(pages);
        Ok(())
    }

    /// Page-cache occupancy as `(pages_cached, capacity)`. Renamed from
    /// `cache_stats` in v0.4.0 to make room for the result-cache
    /// `cache_stats(namespace)` from Phase 10.1.
    fn page_cache_stats(&self) -> PyResult<(usize, usize)> {
        Ok(self.core()?.page_cache_stats())
    }

    /// Override the snapshot-manifest retention cap on this open handle.
    /// `0` disables the cap (retain every snapshot forever); any positive
    /// value keeps the most-recent N and prunes the rest on the next
    /// `flush()`. Defaults at open are inherited from
    /// `MnemoConfig::max_snapshots` (256). See the Rust-side
    /// `Mnemo::set_max_snapshots` docs.
    fn set_max_snapshots(&mut self, max: usize) -> PyResult<()> {
        self.core_mut()?.set_max_snapshots(max);
        Ok(())
    }

    /// Summary statistics as a dict.
    fn stats(&mut self, py: Python<'_>) -> PyResult<PyObject> {
        let s = self.core_mut()?.stats().map_err(to_py)?;
        let d = PyDict::new_bound(py);
        d.set_item("memories", s.memories)?;
        d.set_item("deleted", s.deleted)?;
        d.set_item("dimensions", s.dimensions)?;
        d.set_item("file_bytes", s.file_bytes)?;
        d.set_item("agents", s.agents)?;
        d.set_item("encrypted", s.encrypted)?;
        d.set_item("created_at", s.created_at)?;
        d.set_item("wal_pages", s.wal_pages)?;
        // Phase 6 PR 11.0: how many sync_all calls the most recent
        // flush on this handle issued. `0` on a fresh handle before
        // any flush. Useful for diagnosing write-latency work.
        d.set_item("fsyncs_last_flush", s.fsyncs_last_flush)?;
        d.set_item("snapshots", self.core_mut()?.snapshots().len())?;
        match s.index {
            Some(ix) => {
                let i = PyDict::new_bound(py);
                i.set_item("vectors", ix.vectors)?;
                i.set_item("partitions", ix.partitions)?;
                i.set_item("subspaces", ix.subspaces)?;
                d.set_item("index", i)?;
            }
            None => d.set_item("index", py.None())?,
        }
        Ok(d.into_any().unbind())
    }

    /// Flush any pending writes, then stream the (encrypted) file to
    /// `dest` atomically: writes a sibling temp file in `dest`'s
    /// parent directory, fsyncs it, renames onto `dest`, and fsyncs
    /// the parent directory on Unix. Refuses when `dest` resolves to
    /// the live database (would otherwise truncate the source).
    ///
    /// FIX-1 R-001.5 + R-002.2 part 3: delegates to the Rust core's
    /// `Mnemo::export_encrypted`, which uses the already-open file
    /// handle to copy bytes (so a Windows mandatory lock can't block
    /// the export) and compares `(dev, ino)` on Unix /
    /// canonicalized paths on Windows to catch symlinks + hard
    /// links + rename-onto-self. Pre-FIX-1 this was `close() +
    /// fs::copy()`, which left the handle unusable afterwards and
    /// happily truncated the database when `dest == path`.
    fn export_encrypted(&mut self, dest: &str) -> PyResult<()> {
        self.core_mut()?.export_encrypted(dest).map_err(to_py)
    }

    /// Filesystem path of this database.
    #[getter]
    fn path(&self) -> &str {
        &self.path
    }

    // === Result cache (Phase 10.1 / 10.2 / 10.4) ==========================

    /// Insert or overwrite an exact-key cache entry.
    ///
    /// `value` accepts either `str` (UTF-8 encoded to bytes) or `bytes`.
    /// `content_type` defaults to `"text"`; use `"json"`, `"bytes"`, or
    /// any MIME string you want to round-trip on `cache_get`.
    #[pyo3(signature = (namespace, key, value, content_type="text", ttl_secs=None))]
    fn cache_put(
        &mut self,
        namespace: &str,
        key: &str,
        value: &Bound<'_, PyAny>,
        content_type: &str,
        ttl_secs: Option<u64>,
    ) -> PyResult<()> {
        let bytes = py_value_to_bytes(value)?;
        let opts = mnemo_core::CachePutOpts {
            content_type: content_type.to_string(),
            ttl_secs,
            cost_hint_ms: None,
        };
        self.core_mut()?
            .cache_put(namespace, key, &bytes, opts)
            .map_err(to_py)?;
        self.maybe_flush_after_cache_write()
    }

    /// Look up an exact-key cache entry. Returns `None` on miss or a
    /// dict with `{value, content_type, created_at, accessed_at,
    /// access_count, ttl_secs}` on hit. `value` is always returned as
    /// `bytes`; decode with `.decode('utf-8')` if you stored text.
    fn cache_get(&mut self, py: Python<'_>, namespace: &str, key: &str) -> PyResult<PyObject> {
        match self.core_mut()?.cache_get(namespace, key).map_err(to_py)? {
            Some(hit) => {
                let d = PyDict::new_bound(py);
                d.set_item("value", pyo3::types::PyBytes::new_bound(py, &hit.value))?;
                d.set_item("content_type", hit.content_type)?;
                d.set_item("created_at", hit.created_at)?;
                d.set_item("accessed_at", hit.accessed_at)?;
                d.set_item("access_count", hit.access_count)?;
                if let Some(ttl) = hit.ttl_secs {
                    d.set_item("ttl_secs", ttl)?;
                }
                Ok(d.into())
            }
            None => Ok(py.None()),
        }
    }

    /// Tombstone a cache entry. Silently succeeds if the entry does
    /// not exist. Space reclaimed by `compact` (external CLI).
    fn cache_delete(&mut self, namespace: &str, key: &str) -> PyResult<()> {
        self.core_mut()?.cache_delete(namespace, key).map_err(to_py)?;
        self.maybe_flush_after_cache_write()
    }

    /// Purge cache entries. `namespace=None` targets the whole cache.
    /// `expired_only=True` restricts to TTL-expired entries.
    /// Returns the count of newly tombstoned entries.
    #[pyo3(signature = (namespace=None, expired_only=false))]
    fn cache_purge(&mut self, namespace: Option<&str>, expired_only: bool) -> PyResult<usize> {
        let n = self
            .core_mut()?
            .cache_purge(namespace, expired_only)
            .map_err(to_py)?;
        self.maybe_flush_after_cache_write()?;
        Ok(n)
    }

    /// Select the cache flush policy for this handle.
    ///
    /// - `policy="strict"` (default at open): every cache mutation
    ///   through this binding calls `flush()` before returning, so
    ///   entries are always durable on return. Matches the historic
    ///   Python behavior.
    /// - `policy="batched"`: the engine buffers dirty cache entries
    ///   and auto-flushes when either `max_dirty` mutations have
    ///   accumulated OR `max_age_secs` has elapsed since the first
    ///   pending write. `cache_put` and friends stop calling `flush()`
    ///   themselves — the caller can drop the handle at any time and
    ///   any unflushed entries appear as **misses** on reopen, never
    ///   as corruption (guarded by the Phase 1.3 proptest).
    ///
    /// `max_dirty` and `max_age_secs` are ignored under `strict`;
    /// under `batched` they default to `64` and `5.0` respectively.
    #[pyo3(signature = (policy, max_dirty=None, max_age_secs=None))]
    fn set_cache_flush_policy(
        &mut self,
        policy: &str,
        max_dirty: Option<usize>,
        max_age_secs: Option<f64>,
    ) -> PyResult<()> {
        let (rust_policy, batched) = match policy {
            "strict" => (CacheFlushPolicy::Strict, false),
            "batched" => {
                let md = max_dirty.unwrap_or(64);
                let ma = std::time::Duration::from_secs_f64(max_age_secs.unwrap_or(5.0));
                (
                    CacheFlushPolicy::Batched {
                        max_dirty: md,
                        max_age: ma,
                    },
                    true,
                )
            }
            other => {
                return Err(PyValueError::new_err(format!(
                    "unknown cache flush policy '{other}' (expected 'strict' or 'batched')"
                )))
            }
        };
        self.core_mut()?.set_cache_flush_policy(rust_policy);
        self.batched_cache_flush = batched;
        Ok(())
    }

    /// Set the per-namespace eviction budget. On the next `cache_put`
    /// (or `cache_put_semantic`) into `namespace`, the LRU is evicted
    /// until both the live-entry count is at or below `max_entries`
    /// AND the live payload byte total is at or below `max_bytes`.
    ///
    /// Defaults at open (`max_entries = 10_000`, `max_bytes = 64 MiB`)
    /// are per-namespace; call this only when you need a tighter or
    /// looser cap for a specific namespace.
    fn set_cache_budget(
        &mut self,
        namespace: &str,
        max_entries: usize,
        max_bytes: u64,
    ) -> PyResult<()> {
        self.core_mut()?.set_cache_budget(
            namespace,
            CacheBudget {
                max_entries,
                max_bytes,
            },
        );
        Ok(())
    }

    /// Cache statistics for `namespace` (or the whole cache if
    /// `None`). Returns a dict with `{entries, bytes, hits, misses,
    /// hit_rate, evictions}`.
    #[pyo3(signature = (namespace=None))]
    fn cache_stats(&self, py: Python<'_>, namespace: Option<&str>) -> PyResult<PyObject> {
        let s = self.core()?.cache_stats(namespace);
        let d = PyDict::new_bound(py);
        d.set_item("entries", s.entries)?;
        d.set_item("bytes", s.bytes)?;
        d.set_item("hits", s.hits)?;
        d.set_item("misses", s.misses)?;
        d.set_item("hit_rate", s.hit_rate)?;
        d.set_item("evictions", s.evictions)?;
        Ok(d.into())
    }

    /// Insert or overwrite a semantic-cache entry (Phase 10.2).
    /// `vector` is a list of floats matching the database's
    /// dimensionality. `model` is matched exactly on lookup —
    /// different-model queries won't see this entry.
    // PyO3 bindings for functions with many kwargs naturally exceed the
    // clippy limit — the shape mirrors what a Python caller expects.
    // Restructuring into a builder would hurt the Python ergonomics
    // (`db.cache_put_semantic(...)` is the natural call site) without
    // any Rust-side benefit.
    #[allow(clippy::too_many_arguments)]
    #[pyo3(signature = (namespace, key, vector, value, model, content_type="text", ttl_secs=None))]
    fn cache_put_semantic(
        &mut self,
        namespace: &str,
        key: &str,
        vector: Vec<f32>,
        value: &Bound<'_, PyAny>,
        model: String,
        content_type: &str,
        ttl_secs: Option<u64>,
    ) -> PyResult<()> {
        let bytes = py_value_to_bytes(value)?;
        let opts = mnemo_core::SemanticCachePutOpts {
            content_type: content_type.to_string(),
            ttl_secs,
            cost_hint_ms: None,
            model,
        };
        self.core_mut()?
            .cache_put_semantic(namespace, key, vector, &bytes, opts)
            .map_err(to_py)?;
        self.maybe_flush_after_cache_write()
    }

    /// Semantic recall over the cache. Returns `None` on miss or a
    /// dict with `{value, similarity, content_type, ...}` on hit.
    /// `threshold` defaults to `0.97`.
    #[pyo3(signature = (namespace, query, model, threshold=0.97))]
    fn cache_get_semantic(
        &mut self,
        py: Python<'_>,
        namespace: &str,
        query: Vec<f32>,
        model: &str,
        threshold: f32,
    ) -> PyResult<PyObject> {
        // PY-1 (R-001.11): release the GIL — semantic cache lookup
        // scans the directory + cosines every candidate; O(entries)
        // and decrypts the body of the winner.
        let hit = {
            let core = self.core_mut()?;
            py.allow_threads(|| core.cache_get_semantic(namespace, &query, threshold, model))
                .map_err(to_py)?
        };
        match hit {
            Some((hit, similarity)) => {
                let d = PyDict::new_bound(py);
                d.set_item("value", pyo3::types::PyBytes::new_bound(py, &hit.value))?;
                d.set_item("similarity", similarity)?;
                d.set_item("content_type", hit.content_type)?;
                d.set_item("created_at", hit.created_at)?;
                d.set_item("accessed_at", hit.accessed_at)?;
                d.set_item("access_count", hit.access_count)?;
                if let Some(ttl) = hit.ttl_secs {
                    d.set_item("ttl_secs", ttl)?;
                }
                Ok(d.into())
            }
            None => Ok(py.None()),
        }
    }

    /// Text-side semantic cache lookup (Phase 3 PR 10a). Embeds
    /// `text` with the registered embedder and runs
    /// `cache_get_semantic(namespace, vector, model=embedder.name,
    /// threshold)`. Raises if no embedder is registered.
    ///
    /// Returns the same dict shape as `cache_get_semantic` on hit, or
    /// `None` on miss. The `model` component of the lookup is taken
    /// from the registered embedder's identity — so a semantic entry
    /// put under a different embedder won't match (consistent with
    /// the Rust-core semantic-cache contract).
    #[pyo3(signature = (namespace, text, threshold=0.97))]
    fn cache_get_semantic_text(
        slf: Py<Self>,
        py: Python<'_>,
        namespace: &str,
        text: &str,
        threshold: f32,
    ) -> PyResult<PyObject> {
        // PY-1 (R-001.11): take `slf: Py<Self>` so the Python-level
        // borrow drops before the embedder callback runs — same
        // pattern as `remember_text` / `recall_text`.
        let embedder = {
            let s = slf.borrow(py);
            s.core()?
                .embedder_arc()
                .ok_or_else(|| PyRuntimeError::new_err("no embedder registered on this handle"))?
        }; // PyRef drops here
        let identity = embedder.identity();
        let mut vs = embedder.embed(&[text]).map_err(to_py)?;
        let vector = vs.pop().ok_or_else(|| {
            PyRuntimeError::new_err("embedder returned no vector for the query text")
        })?;
        let model = identity.name;
        // PY-1 (R-001.11): release the GIL around the semantic scan,
        // same as `cache_get_semantic`.
        let mut s = slf.borrow_mut(py);
        let hit = {
            let core = s.core_mut()?;
            py.allow_threads(|| core.cache_get_semantic(namespace, &vector, threshold, &model))
                .map_err(to_py)?
        };
        drop(s);
        match hit {
            Some((hit, similarity)) => {
                let d = PyDict::new_bound(py);
                d.set_item("value", pyo3::types::PyBytes::new_bound(py, &hit.value))?;
                d.set_item("similarity", similarity)?;
                d.set_item("content_type", hit.content_type)?;
                d.set_item("created_at", hit.created_at)?;
                d.set_item("accessed_at", hit.accessed_at)?;
                d.set_item("access_count", hit.access_count)?;
                if let Some(ttl) = hit.ttl_secs {
                    d.set_item("ttl_secs", ttl)?;
                }
                Ok(d.into())
            }
            None => Ok(py.None()),
        }
    }

    /// Begin a conversation `Session` for `agent_id`.
    ///
    /// The session records turns as working memory and, when closed,
    /// consolidates them into episodic memory.
    fn session(slf: Bound<'_, Self>, agent_id: String) -> Session {
        Session {
            db: slf.unbind(),
            agent_id,
            session_id: Ulid::new().to_string(),
            turns: Vec::new(),
            open: true,
        }
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    fn __exit__(
        &mut self,
        _exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        // PY-1: delegate to `close()` (not the core's `close` directly)
        // so the OS file lock actually releases on `with` exit. Pre-PY-1
        // this called `Core::close` on the still-held `Core`, which
        // flushed but left the exclusive lock in place until Python GC
        // collected the Mnemo object — so a `with mnemo.open(...) as
        // db:` block immediately followed by `mnemo.open(same_path)` in
        // the same process raised `Locked`. `close()` is idempotent and
        // no-ops on a read-only handle (keeping the v0.4.0-era fix that
        // read-only `with` blocks don't raise `ReadOnly` on scope exit).
        self.close()?;
        Ok(false) // do not suppress exceptions
    }

    /// `repr` on a closed handle should still succeed — it's often
    /// called by Python error handlers — so we fall back to the
    /// post-close shape instead of propagating the `database is
    /// closed` error through `__repr__`.
    fn __repr__(&self) -> String {
        match self.inner.as_ref() {
            Some(core) => format!(
                "<mnemo.Mnemo path='{}' memories={}>",
                self.path,
                core.len()
            ),
            None => format!("<mnemo.Mnemo path='{}' closed>", self.path),
        }
    }
}

// --- conversation sessions ----------------------------------------------

/// Validate a role string, raising `ValueError` on an unknown value.
fn check_role(s: &str) -> PyResult<String> {
    match s {
        "user" | "assistant" | "system" => Ok(s.to_string()),
        _ => Err(PyValueError::new_err(format!(
            "unknown role '{s}' (expected user, assistant, or system)"
        ))),
    }
}

/// One conversation turn, recorded into working memory by `Session.add_turn`.
///
/// The `vector` is the caller-supplied embedding of `content`; Mnemo does not
/// embed text itself.
#[pyclass]
#[derive(Clone)]
struct Turn {
    role: String,
    content: String,
    vector: Vec<f32>,
}

#[pymethods]
impl Turn {
    /// A turn with an explicit role (`"user"`, `"assistant"`, or `"system"`).
    #[new]
    fn new(role: &str, content: String, vector: Vec<f32>) -> PyResult<Self> {
        Ok(Turn {
            role: check_role(role)?,
            content,
            vector,
        })
    }

    /// Shorthand for a user turn.
    #[staticmethod]
    fn user(content: String, vector: Vec<f32>) -> Turn {
        Turn {
            role: "user".into(),
            content,
            vector,
        }
    }

    /// Shorthand for an assistant turn.
    #[staticmethod]
    fn assistant(content: String, vector: Vec<f32>) -> Turn {
        Turn {
            role: "assistant".into(),
            content,
            vector,
        }
    }

    /// Shorthand for a system turn.
    #[staticmethod]
    fn system(content: String, vector: Vec<f32>) -> Turn {
        Turn {
            role: "system".into(),
            content,
            vector,
        }
    }

    #[getter]
    fn role(&self) -> &str {
        &self.role
    }
    #[getter]
    fn content(&self) -> &str {
        &self.content
    }
    #[getter]
    fn vector(&self) -> Vec<f32> {
        self.vector.clone()
    }

    fn __repr__(&self) -> String {
        format!(
            "<mnemo.Turn role='{}' content='{}'>",
            self.role, self.content
        )
    }
}

/// A scoped conversation session over a [`Mnemo`] database.
///
/// Created by `Mnemo.session(agent_id)`. Holds a handle to the database,
/// records turns as `working` memory tagged with this session, and on `close`
/// **consolidates** them into durable `episodic` memory. Mirrors the Rust
/// core's `Session` type; because Python cannot hold the core's borrowing
/// `Session<'db>` directly, this re-borrows the database on each call.
#[pyclass]
struct Session {
    db: Py<Mnemo>,
    agent_id: String,
    session_id: String,
    turns: Vec<String>,
    open: bool,
}

impl Session {
    /// Error if the session has already been closed or discarded.
    fn ensure_open(&self) -> PyResult<()> {
        if self.open {
            Ok(())
        } else {
            Err(PyRuntimeError::new_err("session is already closed"))
        }
    }
}

#[pymethods]
impl Session {
    /// This session's unique id (a ULID string), stamped on every turn.
    fn id(&self) -> &str {
        &self.session_id
    }

    /// The agent this session belongs to.
    fn agent(&self) -> &str {
        &self.agent_id
    }

    /// Ids of the turns recorded so far, in order.
    fn turn_ids(&self) -> Vec<String> {
        self.turns.clone()
    }

    /// How many turns have been recorded.
    fn turn_count(&self) -> usize {
        self.turns.len()
    }

    /// Record a conversation turn as a `working` memory tagged with this
    /// session and agent. Writes are staged until `close` (or any flush).
    fn add_turn(&mut self, py: Python<'_>, turn: &Turn) -> PyResult<String> {
        self.ensure_open()?;
        let memory = Memory::new(
            turn.content.clone(),
            MemoryType::Working,
            turn.vector.clone(),
        )
        .with_agent(&self.agent_id)
        .with_session(&self.session_id)
        .with_meta("role", serde_json::Value::String(turn.role.clone()));

        let mut db = self.db.borrow_mut(py);
        let id = db.core_mut()?.remember(memory).map_err(to_py)?;
        self.turns.push(id.to_string());
        Ok(id.to_string())
    }

    /// Retrieve memories for context injection, scoped to this session's
    /// agent. Any `agent_id` argument is ignored — a session always recalls
    /// within its own agent's view (its private memories plus shared ones).
    #[pyo3(signature = (query, top_k=10, memory_types=None))]
    fn recall(
        &mut self,
        py: Python<'_>,
        query: Vec<f32>,
        top_k: usize,
        memory_types: Option<Vec<String>>,
    ) -> PyResult<Vec<PyObject>> {
        self.ensure_open()?;
        let mut req = RecallRequest::new(query).top_k(top_k).agent(&self.agent_id);
        if let Some(types) = memory_types {
            let parsed: PyResult<Vec<MemoryType>> = types.iter().map(|t| parse_type(t)).collect();
            req = req.types(parsed?);
        }
        let mut db = self.db.borrow_mut(py);
        let hits = db.core_mut()?.recall(&req).map_err(to_py)?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let dict = memory_to_dict(py, &h.memory)?;
            let bound = dict.bind(py);
            bound.set_item("score", h.score)?;
            bound.set_item("similarity", h.similarity)?;
            out.push(dict);
        }
        Ok(out)
    }

    /// End the session, **consolidating** its turns into episodic memory:
    /// each turn still typed `working` is promoted to `episodic`. Returns the
    /// number promoted; the database is flushed before returning.
    fn close(&mut self, py: Python<'_>) -> PyResult<usize> {
        self.ensure_open()?;
        let mut db = self.db.borrow_mut(py);
        let mut promoted = 0;
        for id_str in &self.turns {
            let id = parse_id(id_str)?;
            // A turn could have been deleted elsewhere — skip if it is gone.
            let mut memory = match db.core_mut()?.get(&id) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if memory.memory_type == MemoryType::Working {
                memory.memory_type = MemoryType::Episodic;
                db.core_mut()?.remember(memory).map_err(to_py)?;
                promoted += 1;
            }
        }
        db.core_mut()?.flush().map_err(to_py)?;
        self.open = false;
        Ok(promoted)
    }

    /// End the session, **discarding** its turns instead of consolidating —
    /// every turn this session recorded is deleted. Returns the number
    /// removed; the database is flushed before returning.
    fn discard(&mut self, py: Python<'_>) -> PyResult<usize> {
        self.ensure_open()?;
        let mut db = self.db.borrow_mut(py);
        let mut removed = 0;
        for id_str in &self.turns {
            let id = parse_id(id_str)?;
            if db.core_mut()?.delete(&id).is_ok() {
                removed += 1;
            }
        }
        db.core_mut()?.flush().map_err(to_py)?;
        self.open = false;
        Ok(removed)
    }

    fn __enter__(slf: Py<Self>) -> Py<Self> {
        slf
    }

    /// Exiting the `with` block consolidates the session (like `close`),
    /// unless it was already closed or discarded.
    fn __exit__(
        &mut self,
        py: Python<'_>,
        _exc_type: &Bound<'_, PyAny>,
        _exc_value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        if self.open {
            self.close(py)?;
        }
        Ok(false) // do not suppress exceptions
    }

    fn __repr__(&self) -> String {
        format!(
            "<mnemo.Session id='{}' agent='{}' turns={} open={}>",
            self.session_id,
            self.agent_id,
            self.turns.len(),
            self.open
        )
    }
}

// --- module-level entry point -------------------------------------------

/// Open an existing database, or create one if `path` does not yet exist.
///
/// `dimensions` is required only when creating; it is ignored for an existing
/// file (the stored dimensionality wins).
/// Open an existing database, or create one if `path` does not yet exist.
///
/// `dimensions` is required only when creating; it is ignored for an existing
/// file (the stored dimensionality wins).
///
/// `read_only=True` opens the file with a shared OS lock and blocks every
/// mutating call (raises `RuntimeError: operation requires a read-write
/// handle`). Read-only opens are refused if the file has a pending WAL
/// recovery or a pending format-version migration — open read-write once
/// to clear either, then reopen read-only. Cannot be combined with
/// creation: read-only + non-existent path raises `ValueError`.
#[pyfunction]
#[pyo3(signature = (path, passphrase, dimensions=None, read_only=false))]
fn open(
    py: Python<'_>,
    path: &str,
    passphrase: &str,
    dimensions: Option<usize>,
    read_only: bool,
) -> PyResult<Mnemo> {
    let exists = std::path::Path::new(path).exists();
    // PY-1 (R-001.11): release the GIL around the Argon2id KDF call
    // that `Core::open` / `Core::create` runs through its KEK
    // derivation. On default `KdfParams::secure` params the KDF is
    // ~50–100 ms; pre-PY-1 that stalled every other Python thread.
    let path_s = path.to_string();
    let pp_s = passphrase.to_string();
    let inner = py.allow_threads(move || -> mnemo_core::Result<Core> {
        if read_only {
            if !exists {
                return Err(mnemo_core::MnemoError::Invalid(
                    "read_only=True cannot create a new database".into(),
                ));
            }
            Core::open_read_only(&path_s, &pp_s)
        } else if exists {
            Core::open(&path_s, &pp_s)
        } else {
            let dims = dimensions.ok_or_else(|| {
                mnemo_core::MnemoError::Invalid(
                    "dimensions is required when creating a new database".into(),
                )
            })?;
            let cfg = MnemoConfig {
                dimensions: dims,
                ..Default::default()
            };
            Core::create(&path_s, &pp_s, cfg)
        }
    });
    let inner = match inner {
        Ok(c) => c,
        Err(mnemo_core::MnemoError::Invalid(msg)) if msg.contains("read_only=True cannot") => {
            return Err(PyValueError::new_err(msg));
        }
        Err(mnemo_core::MnemoError::Invalid(msg)) if msg.contains("dimensions is required") => {
            return Err(PyValueError::new_err(msg));
        }
        Err(e) => return Err(to_py(e)),
    };
    Ok(Mnemo {
        inner: Some(inner),
        path: path.to_string(),
        batched_cache_flush: false,
    })
}

/// Rewrite the file at `path` out-of-place, dropping tombstoned and
/// expired memories and reclaiming stale pages. The compacted file
/// atomically replaces the original via `rename`.
///
/// Callers must not have `path` open elsewhere — this function opens
/// it exclusively for reads and takes the file lock for writes.
///
/// Returns a dict `{before, after}` with the live-memory counts on
/// either side of the rewrite. `before - after` is how many
/// tombstones + expired entries were dropped.
#[pyfunction]
fn compact_file(py: Python<'_>, path: &str, passphrase: &str) -> PyResult<PyObject> {
    // PY-1 (R-001.11): release the GIL — compaction opens the source
    // (Argon2 KDF), creates the target (second Argon2), copies every
    // live memory + cache entry (FIX-1 R-001.1: now including the
    // cache), rebuilds the ANN index, flushes, renames. Multi-second
    // on realistic corpora.
    let path = path.to_string();
    let pp = passphrase.to_string();
    let report =
        py.allow_threads(move || Core::compact_file(&path, &pp)).map_err(to_py)?;
    let d = PyDict::new_bound(py);
    d.set_item("before", report.before)?;
    d.set_item("after", report.after)?;
    // PY-1 + FIX-1 R-001.1: report cache counts too.
    d.set_item("cache_before", report.cache_before)?;
    d.set_item("cache_after", report.cache_after)?;
    Ok(d.into_any().unbind())
}

/// Report this build's cryptographic backend configuration.
///
/// Returns a dict with the mnemo-engine version, the target triple
/// the wheel was built for, and the AES / PMULL backends in use. On
/// ARMv8 builds compiled with `--cfg aes_armv8 --cfg polyval_armv8`
/// (the default for mnemo's own CI + release wheels) the `aes` and
/// `polyval` crates emit hardware instructions; otherwise they run a
/// constant-time software fallback at ~5x the cost per page.
///
/// Call this to confirm a wheel installed from PyPI picked up the
/// hardware path — e.g. `mnemo.build_info()["aes_backend"]` should
/// read `"armv8 hardware"` on Apple Silicon.
#[pyfunction]
fn build_info(py: Python<'_>) -> PyResult<PyObject> {
    let bi = mnemo_core::build_info();
    let d = PyDict::new_bound(py);
    d.set_item("version", bi.version)?;
    d.set_item("target", bi.target)?;
    d.set_item("aes_backend", bi.aes_backend)?;
    d.set_item("polyval_backend", bi.polyval_backend)?;
    Ok(d.into_any().unbind())
}

/// The `mnemo_engine` Python extension module. Renamed in v0.7.0 by
/// PR NAME-1 — see the DECISION entry 2026-11-05 in PLAN.md for the
/// rationale. The old `import mnemo` works for the whole v0.7.x
/// series via a pure-Python shim that re-exports this module and
/// emits one `DeprecationWarning` on import; the alias is removed in
/// v0.8.0.
#[pymodule]
fn mnemo_engine(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Mnemo>()?;
    m.add_class::<Session>()?;
    m.add_class::<Turn>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(compact_file, m)?)?;
    m.add_function(wrap_pyfunction!(build_info, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
