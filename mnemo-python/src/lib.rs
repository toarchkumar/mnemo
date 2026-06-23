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
    Memory, MemoryType, Metric, Mnemo as Core, MnemoConfig, RecallRequest, Scope, Ulid,
};

// --- error / value conversion -------------------------------------------

/// Map a core error onto a Python exception.
fn to_py(e: mnemo_core::MnemoError) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// Parse a memory-type string, raising `ValueError` on an unknown value.
fn parse_type(s: &str) -> PyResult<MemoryType> {
    MemoryType::parse(s)
        .ok_or_else(|| PyValueError::new_err(format!("unknown memory_type '{s}'")))
}

/// Parse a ULID string, raising `ValueError` on a malformed id.
fn parse_id(s: &str) -> PyResult<Ulid> {
    Ulid::from_string(s).map_err(|_| PyValueError::new_err(format!("invalid memory id '{s}'")))
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

// --- the Mnemo class -----------------------------------------------------

/// An open, encrypted Mnemo database.
#[pyclass]
struct Mnemo {
    inner: Core,
    path: String,
}

#[pymethods]
impl Mnemo {
    /// Store a memory; returns its ULID as a string.
    #[pyo3(signature = (content, memory_type, vector, *, agent_id=None,
                        importance=None, session_id=None, ttl_secs=None,
                        shared=false, metadata=None))]
    #[allow(clippy::too_many_arguments)]
    fn remember(
        &mut self,
        content: String,
        memory_type: &str,
        vector: Vec<f32>,
        agent_id: Option<String>,
        importance: Option<f32>,
        session_id: Option<String>,
        ttl_secs: Option<i64>,
        shared: bool,
        metadata: Option<Bound<'_, PyDict>>,
    ) -> PyResult<String> {
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
                memory.metadata.insert(k.extract::<String>()?, py_to_json(&v)?);
            }
        }
        let id = self.inner.remember(memory).map_err(to_py)?;
        Ok(id.to_string())
    }

    /// Multi-signal recall. Returns a list of result dicts, each a memory with
    /// added `score` and `similarity` keys, best first.
    ///
    /// `track_access` (default `True`) controls whether the catalog's
    /// `accessed_at` / `access_count` get bumped on the returned memories
    /// — pass `False` for a fully read-only recall that doesn't dirty the
    /// catalog, useful for batch scoring or dry-runs.
    #[pyo3(signature = (query, top_k=10, memory_types=None, agent_id=None, track_access=true))]
    fn recall(
        &mut self,
        py: Python<'_>,
        query: Vec<f32>,
        top_k: usize,
        memory_types: Option<Vec<String>>,
        agent_id: Option<String>,
        track_access: bool,
    ) -> PyResult<Vec<PyObject>> {
        let mut req = RecallRequest::new(query).top_k(top_k).track_access(track_access);
        if let Some(types) = memory_types {
            let parsed: PyResult<Vec<MemoryType>> =
                types.iter().map(|t| parse_type(t)).collect();
            req = req.types(parsed?);
        }
        if let Some(a) = agent_id {
            req = req.agent(a);
        }
        let hits = self.inner.recall(&req).map_err(to_py)?;
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

    /// Exact nearest-neighbour search; returns `(memory_dict, similarity)`
    /// pairs. Always exact — the ground-truth baseline for `recall`.
    #[pyo3(signature = (query, top_k=10))]
    fn search(
        &mut self,
        py: Python<'_>,
        query: Vec<f32>,
        top_k: usize,
    ) -> PyResult<Vec<(PyObject, f32)>> {
        let hits = self
            .inner
            .search(&query, top_k, Metric::Cosine)
            .map_err(to_py)?;
        let mut out = Vec::with_capacity(hits.len());
        for (m, sim) in hits {
            out.push((memory_to_dict(py, &m)?, sim));
        }
        Ok(out)
    }

    /// Fetch one memory by id; raises `RuntimeError` if it does not exist.
    fn get(&mut self, py: Python<'_>, id: &str) -> PyResult<PyObject> {
        let m = self.inner.get(&parse_id(id)?).map_err(to_py)?;
        memory_to_dict(py, &m)
    }

    /// Soft-delete a memory by id.
    fn delete(&mut self, id: &str) -> PyResult<()> {
        self.inner.delete(&parse_id(id)?).map_err(to_py)
    }

    /// Number of live (non-deleted) memories.
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    /// Persist all pending changes as one atomic transaction.
    fn flush(&mut self) -> PyResult<()> {
        self.inner.flush().map_err(to_py)
    }

    /// Flush and release; the object should not be used afterwards.
    fn close(&mut self) -> PyResult<()> {
        self.inner.flush().map_err(to_py)
    }

    /// Decrypt and re-validate every live record; returns the count checked.
    fn verify(&mut self) -> PyResult<usize> {
        self.inner.verify().map_err(to_py)
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
        for m in self.inner.about().map_err(to_py)? {
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
        let dims = self.inner.stats().map_err(to_py)?.dimensions;
        let manifest = Memory::scaffold_manifest(dims);
        let id = self.inner.remember(manifest).map_err(to_py)?;
        Ok(id.to_string())
    }

    /// Build the IVF+PQ approximate index; `recall` then runs sub-linearly.
    fn build_index(&mut self) -> PyResult<()> {
        self.inner.build_index().map(|_| ()).map_err(to_py)
    }

    /// Drop the approximate index; recall reverts to exact scan.
    fn drop_index(&mut self) {
        self.inner.drop_index();
    }

    /// Whether an approximate index is currently loaded.
    fn has_index(&self) -> bool {
        self.inner.has_index()
    }

    /// List restorable snapshots — one dict per committed transaction.
    fn snapshots(&self, py: Python<'_>) -> PyResult<Vec<PyObject>> {
        let mut out = Vec::new();
        for s in self.inner.snapshots() {
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
        let info = self.inner.restore_to(txn_id).map_err(to_py)?;
        let d = PyDict::new_bound(py);
        d.set_item("txn_id", info.txn_id)?;
        d.set_item("created_at", info.created_at)?;
        d.set_item("memory_count", info.memory_count)?;
        Ok(d.into_any().unbind())
    }

    /// Restore to the latest snapshot committed at or before `unix_secs`.
    fn restore_to_time(&mut self, py: Python<'_>, unix_secs: i64) -> PyResult<PyObject> {
        let info = self.inner.restore_to_time(unix_secs).map_err(to_py)?;
        let d = PyDict::new_bound(py);
        d.set_item("txn_id", info.txn_id)?;
        d.set_item("created_at", info.created_at)?;
        d.set_item("memory_count", info.memory_count)?;
        Ok(d.into_any().unbind())
    }

    /// Bound the page cache to `pages` decrypted pages.
    fn set_cache_capacity(&mut self, pages: usize) {
        self.inner.set_cache_capacity(pages);
    }

    /// Page-cache occupancy as `(pages_cached, capacity)`.
    fn cache_stats(&self) -> (usize, usize) {
        self.inner.cache_stats()
    }

    /// Override the snapshot-manifest retention cap on this open handle.
    /// `0` disables the cap (retain every snapshot forever); any positive
    /// value keeps the most-recent N and prunes the rest on the next
    /// `flush()`. Defaults at open are inherited from
    /// `MnemoConfig::max_snapshots` (256). See the Rust-side
    /// `Mnemo::set_max_snapshots` docs.
    fn set_max_snapshots(&mut self, max: usize) {
        self.inner.set_max_snapshots(max);
    }

    /// Summary statistics as a dict.
    fn stats(&mut self, py: Python<'_>) -> PyResult<PyObject> {
        let s = self.inner.stats().map_err(to_py)?;
        let d = PyDict::new_bound(py);
        d.set_item("memories", s.memories)?;
        d.set_item("deleted", s.deleted)?;
        d.set_item("dimensions", s.dimensions)?;
        d.set_item("file_bytes", s.file_bytes)?;
        d.set_item("agents", s.agents)?;
        d.set_item("encrypted", s.encrypted)?;
        d.set_item("created_at", s.created_at)?;
        d.set_item("wal_pages", s.wal_pages)?;
        d.set_item("snapshots", self.inner.snapshots().len())?;
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

    /// Flush, then copy the encrypted file to `dest`. The file is already
    /// encrypted, so the export is a plain byte-for-byte copy.
    fn export_encrypted(&mut self, dest: &str) -> PyResult<()> {
        self.inner.flush().map_err(to_py)?;
        std::fs::copy(&self.path, dest)
            .map_err(|e| PyRuntimeError::new_err(format!("export failed: {e}")))?;
        Ok(())
    }

    /// Filesystem path of this database.
    #[getter]
    fn path(&self) -> &str {
        &self.path
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
        self.inner.flush().map_err(to_py)?;
        Ok(false) // do not suppress exceptions
    }

    fn __repr__(&self) -> String {
        format!(
            "<mnemo.Mnemo path='{}' memories={}>",
            self.path,
            self.inner.len()
        )
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
        Ok(Turn { role: check_role(role)?, content, vector })
    }

    /// Shorthand for a user turn.
    #[staticmethod]
    fn user(content: String, vector: Vec<f32>) -> Turn {
        Turn { role: "user".into(), content, vector }
    }

    /// Shorthand for an assistant turn.
    #[staticmethod]
    fn assistant(content: String, vector: Vec<f32>) -> Turn {
        Turn { role: "assistant".into(), content, vector }
    }

    /// Shorthand for a system turn.
    #[staticmethod]
    fn system(content: String, vector: Vec<f32>) -> Turn {
        Turn { role: "system".into(), content, vector }
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
        format!("<mnemo.Turn role='{}' content='{}'>", self.role, self.content)
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
        let id = db.inner.remember(memory).map_err(to_py)?;
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
            let parsed: PyResult<Vec<MemoryType>> =
                types.iter().map(|t| parse_type(t)).collect();
            req = req.types(parsed?);
        }
        let mut db = self.db.borrow_mut(py);
        let hits = db.inner.recall(&req).map_err(to_py)?;
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
            let mut memory = match db.inner.get(&id) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if memory.memory_type == MemoryType::Working {
                memory.memory_type = MemoryType::Episodic;
                db.inner.remember(memory).map_err(to_py)?;
                promoted += 1;
            }
        }
        db.inner.flush().map_err(to_py)?;
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
            if db.inner.delete(&id).is_ok() {
                removed += 1;
            }
        }
        db.inner.flush().map_err(to_py)?;
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
#[pyfunction]
#[pyo3(signature = (path, passphrase, dimensions=None))]
fn open(path: &str, passphrase: &str, dimensions: Option<usize>) -> PyResult<Mnemo> {
    let inner = if std::path::Path::new(path).exists() {
        Core::open(path, passphrase).map_err(to_py)?
    } else {
        let dims = dimensions.ok_or_else(|| {
            PyValueError::new_err("dimensions is required when creating a new database")
        })?;
        let cfg = MnemoConfig { dimensions: dims, ..Default::default() };
        Core::create(path, passphrase, cfg).map_err(to_py)?
    };
    Ok(Mnemo { inner, path: path.to_string() })
}

/// The `mnemo` Python extension module.
#[pymodule]
fn mnemo(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Mnemo>()?;
    m.add_class::<Session>()?;
    m.add_class::<Turn>()?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
