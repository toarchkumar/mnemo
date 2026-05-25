//! Node.js bindings for Mnemo — the encrypted, single-file agent-memory engine.
//!
//! Built with [napi-rs]. The compiled addon exposes a `Database` class whose
//! methods mirror the Rust API; structured results come back as plain
//! JavaScript objects with camelCase fields.
//!
//! ```js
//! const { Database } = require('./mnemo.node');
//! const db = Database.create('agent.mnemo', 'passphrase', 3);
//! db.remember('the user prefers dark mode', [0.1, 0.2, 0.9], 'procedural', 0.8);
//! db.flush();
//! for (const hit of db.recall([0.1, 0.2, 0.9], 5)) {
//!   console.log(hit.score, hit.content);
//! }
//! ```
//!
//! [napi-rs]: https://napi.rs

use mnemo::{Memory, MemoryType, Mnemo, MnemoConfig, RecallRequest, Ulid};
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// Translate a Mnemo error into a JavaScript error.
fn to_napi(e: mnemo::MnemoError) -> Error {
    Error::from_reason(e.to_string())
}

/// Parse a memory-type string, erroring if unknown.
fn parse_type(s: &str) -> Result<MemoryType> {
    MemoryType::parse(s)
        .ok_or_else(|| Error::from_reason(format!("unknown memory type '{s}'")))
}

/// One scored result from [`Database::recall`].
#[napi(object)]
pub struct RecallHit {
    pub id: String,
    pub content: String,
    pub memory_type: String,
    pub score: f64,
    pub similarity: f64,
    pub importance: f64,
    pub agent: String,
}

/// A memory returned by [`Database::get`].
#[napi(object)]
pub struct MemoryObject {
    pub id: String,
    pub content: String,
    pub memory_type: String,
    pub vector: Vec<f64>,
    pub importance: f64,
    pub agent: String,
    pub session: Option<String>,
    pub created_at: i64,
    pub access_count: u32,
}

/// Summary statistics from [`Database::stats`].
#[napi(object)]
pub struct DbStats {
    pub memories: u32,
    pub deleted: u32,
    pub dimensions: u32,
    pub file_bytes: f64,
    pub encrypted: bool,
    pub wal_pages: u32,
}

/// One restorable snapshot from [`Database::snapshots`].
#[napi(object)]
pub struct SnapshotObject {
    pub txn_id: i64,
    pub created_at: i64,
    pub memory_count: i64,
}

/// An encrypted, single-file agent-memory database.
#[napi(js_name = "Database")]
pub struct Database {
    inner: Mnemo,
}

#[napi]
impl Database {
    /// Create a brand-new encrypted database at `path`.
    #[napi(factory)]
    pub fn create(path: String, passphrase: String, dimensions: u32) -> Result<Database> {
        let cfg = MnemoConfig {
            dimensions: dimensions as usize,
            ..Default::default()
        };
        Mnemo::create(&path, &passphrase, cfg)
            .map(|inner| Database { inner })
            .map_err(to_napi)
    }

    /// Open an existing database. A wrong passphrase rejects with an error.
    #[napi(factory)]
    pub fn open(path: String, passphrase: String) -> Result<Database> {
        Mnemo::open(&path, &passphrase)
            .map(|inner| Database { inner })
            .map_err(to_napi)
    }

    /// Store a memory and return its id. `memoryType` is one of
    /// `episodic`, `semantic`, `procedural`, `working`.
    #[napi]
    pub fn remember(
        &mut self,
        content: String,
        vector: Vec<f64>,
        memory_type: String,
        importance: Option<f64>,
        agent: Option<String>,
    ) -> Result<String> {
        let v: Vec<f32> = vector.into_iter().map(|x| x as f32).collect();
        let mut m = Memory::new(content, parse_type(&memory_type)?, v);
        if let Some(i) = importance {
            m = m.with_importance(i as f32);
        }
        if let Some(a) = agent {
            m = m.with_agent(a);
        }
        self.inner
            .remember(m)
            .map(|id| id.to_string())
            .map_err(to_napi)
    }

    /// Multi-signal recall. Returns hits ordered by score.
    #[napi]
    pub fn recall(
        &mut self,
        vector: Vec<f64>,
        top_k: Option<u32>,
        memory_types: Option<Vec<String>>,
    ) -> Result<Vec<RecallHit>> {
        let v: Vec<f32> = vector.into_iter().map(|x| x as f32).collect();
        let mut req = RecallRequest::new(v).top_k(top_k.unwrap_or(10) as usize);
        if let Some(types) = memory_types {
            let mut mts = Vec::with_capacity(types.len());
            for t in &types {
                mts.push(parse_type(t)?);
            }
            req = req.types(mts);
        }
        let hits = self.inner.recall(&req).map_err(to_napi)?;
        Ok(hits
            .into_iter()
            .map(|h| RecallHit {
                id: h.memory.id.to_string(),
                content: h.memory.content,
                memory_type: h.memory.memory_type.as_str().to_string(),
                score: h.score as f64,
                similarity: h.similarity as f64,
                importance: h.memory.importance as f64,
                agent: h.memory.agent_id,
            })
            .collect())
    }

    /// Fetch one memory by id.
    #[napi]
    pub fn get(&mut self, id: String) -> Result<MemoryObject> {
        let uid = Ulid::from_string(&id)
            .map_err(|_| Error::from_reason(format!("invalid memory id '{id}'")))?;
        let m = self.inner.get(&uid).map_err(to_napi)?;
        Ok(MemoryObject {
            id: m.id.to_string(),
            content: m.content,
            memory_type: m.memory_type.as_str().to_string(),
            vector: m.vector.into_iter().map(|x| x as f64).collect(),
            importance: m.importance as f64,
            agent: m.agent_id,
            session: m.session_id,
            created_at: m.created_at,
            access_count: m.access_count,
        })
    }

    /// Soft-delete a memory by id (reclaimed on the next compaction).
    #[napi]
    pub fn delete(&mut self, id: String) -> Result<()> {
        let uid = Ulid::from_string(&id)
            .map_err(|_| Error::from_reason(format!("invalid memory id '{id}'")))?;
        self.inner.delete(&uid).map_err(to_napi)
    }

    /// Persist pending changes as one atomic, write-ahead-logged transaction.
    #[napi]
    pub fn flush(&mut self) -> Result<()> {
        self.inner.flush().map_err(to_napi)
    }

    /// Flush and finish using the database.
    #[napi]
    pub fn close(&mut self) -> Result<()> {
        self.inner.close().map_err(to_napi)
    }

    /// Live (non-deleted) memory count.
    #[napi]
    pub fn len(&self) -> u32 {
        self.inner.len() as u32
    }

    /// Summary statistics.
    #[napi]
    pub fn stats(&mut self) -> Result<DbStats> {
        let s = self.inner.stats().map_err(to_napi)?;
        Ok(DbStats {
            memories: s.memories as u32,
            deleted: s.deleted as u32,
            dimensions: s.dimensions as u32,
            file_bytes: s.file_bytes as f64,
            encrypted: s.encrypted,
            wal_pages: s.wal_pages as u32,
        })
    }

    /// List restorable snapshots (point-in-time recovery), oldest first.
    #[napi]
    pub fn snapshots(&self) -> Vec<SnapshotObject> {
        self.inner
            .snapshots()
            .into_iter()
            .map(|s| SnapshotObject {
                txn_id: s.txn_id as i64,
                created_at: s.created_at,
                memory_count: s.memory_count as i64,
            })
            .collect()
    }

    /// Restore the database to the snapshot from transaction `txnId`.
    /// Returns the restored memory count.
    #[napi]
    pub fn restore_to(&mut self, txn_id: i64) -> Result<i64> {
        self.inner
            .restore_to(txn_id as u64)
            .map(|s| s.memory_count as i64)
            .map_err(to_napi)
    }
}
