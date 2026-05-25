//! Python bindings for Mnemo — the encrypted, single-file agent-memory engine.
//!
//! Built with [PyO3]. The compiled extension exposes one class, `Mnemo`,
//! whose methods mirror the Rust API; results come back as plain dicts and
//! lists so the surface feels native to Python.
//!
//! ```python
//! import mnemo
//! db = mnemo.Mnemo.create("agent.mnemo", "passphrase", dimensions=3)
//! db.remember("the user prefers dark mode", [0.1, 0.2, 0.9],
//!             memory_type="procedural", importance=0.8)
//! db.flush()
//! for hit in db.recall([0.1, 0.2, 0.9], top_k=5):
//!     print(hit["score"], hit["content"])
//! ```
//!
//! [PyO3]: https://pyo3.rs

use pyo3::exceptions::{PyIOError, PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyDict;

use mnemo::{Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, Ulid};

/// Translate a Mnemo error into the most fitting Python exception.
fn to_pyerr(e: mnemo::MnemoError) -> PyErr {
    use mnemo::MnemoError as E;
    match e {
        E::NotFound(s) => PyKeyError::new_err(s),
        E::Io(io) => PyIOError::new_err(io.to_string()),
        other => PyValueError::new_err(other.to_string()),
    }
}

/// Parse a memory-type string, raising `ValueError` if unknown.
fn parse_type(s: &str) -> PyResult<MemoryType> {
    MemoryType::parse(s)
        .ok_or_else(|| PyValueError::new_err(format!("unknown memory_type '{s}'")))
}

/// Parse a ULID string, raising `ValueError` if malformed.
fn parse_id(s: &str) -> PyResult<Ulid> {
    Ulid::from_string(s)
        .map_err(|_| PyValueError::new_err(format!("invalid memory id '{s}'")))
}

/// An encrypted, single-file agent-memory database.
#[pyclass(name = "Mnemo")]
struct PyMnemo {
    inner: Mnemo,
}

#[pymethods]
impl PyMnemo {
    /// Create a brand-new encrypted database at `path`.
    #[staticmethod]
    #[pyo3(signature = (path, passphrase, dimensions = 768))]
    fn create(path: &str, passphrase: &str, dimensions: usize) -> PyResult<Self> {
        let cfg = MnemoConfig { dimensions, ..Default::default() };
        Mnemo::create(path, passphrase, cfg)
            .map(|inner| Self { inner })
            .map_err(to_pyerr)
    }

    /// Open an existing database. A wrong passphrase raises `ValueError`.
    #[staticmethod]
    fn open(path: &str, passphrase: &str) -> PyResult<Self> {
        Mnemo::open(path, passphrase)
            .map(|inner| Self { inner })
            .map_err(to_pyerr)
    }

    /// Store a memory and return its id. `memory_type` is one of
    /// `episodic`, `semantic`, `procedural`, `working`.
    #[pyo3(signature = (content, vector, memory_type = "semantic", importance = 0.5, agent = None, session = None))]
    fn remember(
        &mut self,
        content: &str,
        vector: Vec<f32>,
        memory_type: &str,
        importance: f32,
        agent: Option<&str>,
        session: Option<&str>,
    ) -> PyResult<String> {
        let mut m =
            Memory::new(content, parse_type(memory_type)?, vector).with_importance(importance);
        if let Some(a) = agent {
            m = m.with_agent(a);
        }
        if let Some(s) = session {
            m = m.with_session(s);
        }
        self.inner
            .remember(m)
            .map(|id| id.to_string())
            .map_err(to_pyerr)
    }

    /// Multi-signal recall. Returns a list of dicts ordered by score.
    #[pyo3(signature = (vector, top_k = 10, memory_types = None, agent = None))]
    fn recall(
        &mut self,
        py: Python<'_>,
        vector: Vec<f32>,
        top_k: usize,
        memory_types: Option<Vec<String>>,
        agent: Option<&str>,
    ) -> PyResult<Vec<PyObject>> {
        let mut req = RecallRequest::new(vector).top_k(top_k);
        if let Some(types) = memory_types {
            let parsed: PyResult<Vec<MemoryType>> =
                types.iter().map(|t| parse_type(t)).collect();
            req = req.types(parsed?);
        }
        if let Some(a) = agent {
            req = req.agent(a);
        }
        let hits = self.inner.recall(&req).map_err(to_pyerr)?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let d = PyDict::new_bound(py);
            d.set_item("id", h.memory.id.to_string())?;
            d.set_item("content", h.memory.content)?;
            d.set_item("memory_type", h.memory.memory_type.as_str())?;
            d.set_item("score", h.score)?;
            d.set_item("similarity", h.similarity)?;
            d.set_item("importance", h.memory.importance)?;
            d.set_item("agent", h.memory.agent_id)?;
            out.push(d.into_any().unbind());
        }
        Ok(out)
    }

    /// Fetch one memory by id, as a dict.
    fn get(&mut self, py: Python<'_>, id: &str) -> PyResult<PyObject> {
        let m = self.inner.get(&parse_id(id)?).map_err(to_pyerr)?;
        let d = PyDict::new_bound(py);
        d.set_item("id", m.id.to_string())?;
        d.set_item("content", m.content)?;
        d.set_item("memory_type", m.memory_type.as_str())?;
        d.set_item("vector", m.vector)?;
        d.set_item("importance", m.importance)?;
        d.set_item("agent", m.agent_id)?;
        d.set_item("session", m.session_id)?;
        d.set_item("created_at", m.created_at)?;
        d.set_item("access_count", m.access_count)?;
        Ok(d.into_any().unbind())
    }

    /// Soft-delete a memory by id (reclaimed on the next compaction).
    fn delete(&mut self, id: &str) -> PyResult<()> {
        self.inner.delete(&parse_id(id)?).map_err(to_pyerr)
    }

    /// Persist pending changes as one atomic, write-ahead-logged transaction.
    fn flush(&mut self) -> PyResult<()> {
        self.inner.flush().map_err(to_pyerr)
    }

    /// Flush and finish using the database.
    fn close(&mut self) -> PyResult<()> {
        self.inner.close().map_err(to_pyerr)
    }

    /// Summary statistics as a dict.
    fn stats(&mut self, py: Python<'_>) -> PyResult<PyObject> {
        let s = self.inner.stats().map_err(to_pyerr)?;
        let d = PyDict::new_bound(py);
        d.set_item("memories", s.memories)?;
        d.set_item("deleted", s.deleted)?;
        d.set_item("dimensions", s.dimensions)?;
        d.set_item("file_bytes", s.file_bytes)?;
        d.set_item("encrypted", s.encrypted)?;
        d.set_item("wal_pages", s.wal_pages)?;
        Ok(d.into_any().unbind())
    }

    /// List restorable snapshots (point-in-time recovery), oldest first.
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
    /// Returns the restored memory count.
    fn restore_to(&mut self, txn_id: u64) -> PyResult<u64> {
        self.inner
            .restore_to(txn_id)
            .map(|s| s.memory_count)
            .map_err(to_pyerr)
    }

    /// Live (non-deleted) memory count — also `len(db)`.
    fn __len__(&self) -> usize {
        self.inner.len()
    }

    fn __repr__(&self) -> String {
        format!("<Mnemo: {} memories>", self.inner.len())
    }
}

/// The `mnemo` Python module.
#[pymodule]
#[pyo3(name = "mnemo")]
fn mnemo_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyMnemo>()?;
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    Ok(())
}
