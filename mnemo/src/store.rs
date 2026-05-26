//! The Mnemo database — ties the storage engine and the memory model together.
//!
//! Layout of a populated file:
//!
//! ```text
//!   page 0           : Header (unencrypted)
//!   pages 1..W       : write-ahead log region
//!   pages W..        : encrypted record runs + catalog runs + index +
//!                      snapshot manifest (append-only)
//! ```
//!
//! Durability is provided by a **write-ahead log** ([`crate::wal`]). A `flush`
//! is one transaction: record data pages are written copy-on-write to fresh
//! pages, then the new catalog, ANN index, and header are logged to the WAL
//! and committed with a single fsync. A checkpoint then folds the WAL into the
//! home pages. A crash before the commit leaves the previous state intact; a
//! crash after it is repaired by replaying the WAL on open.
//!
//! Because pages are only ever appended, every past transaction's runs survive
//! on disk. Each flush also appends an entry to a **snapshot manifest**, so
//! [`Mnemo::restore_to`] can reinstate any past state. Stale pages — and the
//! history along with them — are reclaimed by [`Mnemo::compact_file`].

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::crypto::{self, KdfParams};
use crate::error::{MnemoError, Result};
use crate::format::{Header, FLAG_ENCRYPTED, PAGE_SIZE, PAYLOAD, VERSION, WRAPPED_DEK_LEN};
use crate::index::{IndexConfig, IndexInfo, IvfPqIndex};
use crate::memory::{self, Memory, MemoryType, Metric, Scope, ScoreWeights};
use crate::pager::Pager;
use crate::wal;

/// Initial size of the write-ahead log region, in pages (512 KiB). The region
/// grows automatically when a transaction's control plane outgrows it.
const DEFAULT_WAL_PAGES: u64 = 64;

/// Configuration for creating a new database.
#[derive(Clone, Copy, Debug)]
pub struct MnemoConfig {
    /// Embedding dimensionality. Every stored vector must match this.
    pub dimensions: usize,
    /// Key-derivation parameters.
    pub kdf: KdfParams,
}

impl Default for MnemoConfig {
    fn default() -> Self {
        Self { dimensions: 768, kdf: KdfParams::secure() }
    }
}

/// Catalog entry: maps a memory ID to its page run on disk.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct CatalogEntry {
    /// ULID as a raw `u128`.
    id: u128,
    start_page: u64,
    page_count: u32,
    /// Exact serialized byte length of the record.
    len: u32,
    deleted: bool,
}

/// One entry in the append-only snapshot manifest. Because record, catalog,
/// and index pages are only ever *appended*, the runs a past flush wrote are
/// still on disk; a `Snapshot` is the set of pointers needed to reconstruct
/// the database exactly as that flush left it.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Snapshot {
    txn_id: u64,
    created_at: i64,
    catalog_start: u64,
    catalog_pages: u64,
    catalog_len: u64,
    index_start: u64,
    index_pages: u64,
    index_len: u64,
    memory_count: u64,
}

/// A restorable point in a database's history — see [`Mnemo::snapshots`].
#[derive(Clone, Copy, Debug)]
pub struct SnapshotInfo {
    /// Id of the transaction that produced this snapshot (monotonic from 1).
    pub txn_id: u64,
    /// When the snapshot was committed (unix seconds).
    pub created_at: i64,
    /// Live memory count captured in the snapshot.
    pub memory_count: u64,
}

/// A retrieval request for [`Mnemo::recall`].
#[derive(Clone, Debug)]
pub struct RecallRequest {
    /// Query embedding.
    pub query: Vec<f32>,
    /// Maximum results to return.
    pub top_k: usize,
    /// Restrict to these memory types (`None` = all types).
    pub memory_types: Option<Vec<MemoryType>>,
    /// Restrict to a single agent's view: its own memories plus shared ones.
    pub agent_id: Option<String>,
    /// Similarity metric.
    pub metric: Metric,
    /// Multi-signal score weights.
    pub weights: ScoreWeights,
    /// Index override: partitions to probe (`None` = index default). Ignored
    /// when no ANN index is present.
    pub n_probe: Option<usize>,
    /// Index override: candidates to rerank (`None` = index default). Ignored
    /// when no ANN index is present.
    pub n_rerank: Option<usize>,
}

impl RecallRequest {
    /// A request with sensible defaults for a query embedding.
    pub fn new(query: Vec<f32>) -> Self {
        Self {
            query,
            top_k: 10,
            memory_types: None,
            agent_id: None,
            metric: Metric::Cosine,
            weights: ScoreWeights::default(),
            n_probe: None,
            n_rerank: None,
        }
    }
    /// Set the result cap.
    pub fn top_k(mut self, k: usize) -> Self {
        self.top_k = k;
        self
    }
    /// Restrict to specific memory types.
    pub fn types(mut self, t: Vec<MemoryType>) -> Self {
        self.memory_types = Some(t);
        self
    }
    /// Restrict to one agent's view.
    pub fn agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = Some(agent_id.into());
        self
    }
    /// Set the similarity metric (default: cosine).
    pub fn metric(mut self, metric: Metric) -> Self {
        self.metric = metric;
        self
    }
    /// Replace the multi-signal score weights.
    pub fn weights(mut self, weights: ScoreWeights) -> Self {
        self.weights = weights;
        self
    }
    /// Override the number of IVF partitions probed (accuracy/speed dial).
    pub fn n_probe(mut self, n: usize) -> Self {
        self.n_probe = Some(n);
        self
    }
    /// Override the number of candidates reranked exactly (accuracy dial).
    pub fn n_rerank(mut self, n: usize) -> Self {
        self.n_rerank = Some(n);
        self
    }
}

/// One scored result from [`Mnemo::recall`].
#[derive(Clone, Debug)]
pub struct RecallResult {
    /// The retrieved memory.
    pub memory: Memory,
    /// Combined multi-signal score.
    pub score: f32,
    /// The bare similarity component (before other signals).
    pub similarity: f32,
}

/// Summary statistics for a database.
#[derive(Clone, Debug)]
pub struct Stats {
    /// Live (non-deleted) memory count.
    pub memories: usize,
    /// Tombstoned entries awaiting compaction.
    pub deleted: usize,
    /// Embedding dimensionality.
    pub dimensions: usize,
    /// File size in bytes.
    pub file_bytes: u64,
    /// Distinct agent IDs present.
    pub agents: Vec<String>,
    /// Whether pages are encrypted (always true in v1).
    pub encrypted: bool,
    /// Creation time (unix seconds).
    pub created_at: i64,
    /// ANN index shape, if an index has been built.
    pub index: Option<IndexInfo>,
    /// Current size of the write-ahead log region, in 8 KiB pages.
    pub wal_pages: u64,
}

/// Result of a [`Mnemo::compact_file`] run.
#[derive(Clone, Copy, Debug)]
pub struct CompactReport {
    /// Live memories before compaction.
    pub before: usize,
    /// Live memories after compaction (expired ones dropped).
    pub after: usize,
}

/// An encrypted, single-file agent memory database.
pub struct Mnemo {
    pager: Pager,
    header: Header,
    catalog: Vec<CatalogEntry>,
    index: HashMap<u128, usize>,
    #[allow(dead_code)]
    path: PathBuf,
    dimensions: usize,
    kdf: KdfParams,
    /// Set whenever the catalog changes; drives whether `flush` rewrites it.
    dirty_catalog: bool,
    /// Optional IVF+PQ approximate-nearest-neighbour index.
    ann: Option<IvfPqIndex>,
    /// Set whenever the ANN index changes; drives whether `flush` rewrites it.
    dirty_index: bool,
    /// Append-only manifest of committed snapshots, oldest first.
    manifest: Vec<Snapshot>,
}

/// Read a run of consecutive encrypted pages and concatenate their plaintext.
fn read_run_bytes(pager: &mut Pager, start: u64, pages: u64, len: u64) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len as usize);
    for i in 0..pages {
        buf.extend_from_slice(&pager.read_page(start + i)?);
    }
    buf.truncate(len as usize);
    Ok(buf)
}

/// Load a catalog run (an empty `pages` yields an empty catalog).
fn load_catalog(pager: &mut Pager, start: u64, pages: u64, len: u64) -> Result<Vec<CatalogEntry>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load the snapshot manifest (an empty `pages` yields an empty manifest).
fn load_manifest(pager: &mut Pager, start: u64, pages: u64, len: u64) -> Result<Vec<Snapshot>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load an ANN index run, validating its dimensionality.
fn load_index(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    dims: usize,
) -> Result<Option<IvfPqIndex>> {
    if pages == 0 {
        return Ok(None);
    }
    let buf = read_run_bytes(pager, start, pages, len)?;
    let mut idx: IvfPqIndex =
        rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    if idx.dims() != dims {
        return Err(MnemoError::Invalid(
            "ANN index dimensionality does not match the database".into(),
        ));
    }
    idx.rebuild_assignment();
    Ok(Some(idx))
}

/// True if a memory's metadata marks it as the canonical onboarding manifest
/// (`metadata.topic == "manifest"`). Used by [`Mnemo::about`] to hoist the
/// manifest to the top of the briefing regardless of importance ordering.
fn is_manifest(m: &Memory) -> bool {
    m.metadata
        .get("topic")
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("manifest"))
        .unwrap_or(false)
}

/// Build the ULID → catalog-slot lookup map.
fn build_id_index(catalog: &[CatalogEntry]) -> HashMap<u128, usize> {
    let mut m = HashMap::with_capacity(catalog.len());
    for (i, e) in catalog.iter().enumerate() {
        m.insert(e.id, i);
    }
    m
}

impl Mnemo {
    /// Create a brand-new encrypted database at `path`.
    pub fn create(path: impl Into<PathBuf>, passphrase: &str, config: MnemoConfig) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        if config.dimensions == 0 {
            return Err(MnemoError::Invalid("dimensions must be > 0".into()));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;

        let salt = crypto::random_salt();
        let dek = crypto::random_dek();
        let kek = crypto::derive_kek(passphrase.as_bytes(), &salt, config.kdf)?;
        let dek_nonce = crypto::random_nonce();
        let wrapped = crypto::wrap_dek(&kek, &dek_nonce, &dek)?;
        if wrapped.len() != WRAPPED_DEK_LEN {
            return Err(MnemoError::Crypto("unexpected wrapped DEK length".into()));
        }
        let mut wrapped_dek = [0u8; WRAPPED_DEK_LEN];
        wrapped_dek.copy_from_slice(&wrapped);

        let header = Header {
            version: VERSION,
            page_size: PAGE_SIZE as u32,
            flags: FLAG_ENCRYPTED,
            dimensions: config.dimensions as u32,
            created_at: memory::now_secs(),
            write_counter: 0,
            // Page 0 is the header; pages 1..=DEFAULT_WAL_PAGES are the WAL;
            // record/catalog/index pages start after it.
            next_page: 1 + DEFAULT_WAL_PAGES,
            catalog_start: 0,
            catalog_pages: 0,
            catalog_len: 0,
            vector_count: 0,
            m_cost: config.kdf.m_cost,
            t_cost: config.kdf.t_cost,
            p_cost: config.kdf.p_cost,
            salt,
            dek_nonce,
            wrapped_dek,
            index_start: 0,
            index_pages: 0,
            index_len: 0,
            wal_start: 1,
            wal_pages: DEFAULT_WAL_PAGES,
            wal_seq: 0,
            manifest_start: 0,
            manifest_pages: 0,
            manifest_len: 0,
        };

        let mut pager = Pager::new(file, dek, 0);
        pager.write_raw(0, &header.to_page())?;
        pager.sync()?;

        Ok(Mnemo {
            pager,
            header,
            catalog: Vec::new(),
            index: HashMap::new(),
            path,
            dimensions: config.dimensions,
            kdf: config.kdf,
            dirty_catalog: false,
            ann: None,
            dirty_index: false,
            manifest: Vec::new(),
        })
    }

    /// Open an existing database. A wrong passphrase fails cleanly with
    /// [`MnemoError::WrongPassphrase`].
    pub fn open(path: impl Into<PathBuf>, passphrase: &str) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;

        let mut hbuf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        let mut header = match Header::from_page(&hbuf) {
            Ok(h) => h,
            Err(MnemoError::HeaderChecksum) => {
                // Page 0 is torn — most likely a crash during a checkpoint's
                // header write. Try to heal from the WAL at its default site
                // (a never-grown WAL never moves from page 1).
                let healed = wal::recover(&mut file, 1, DEFAULT_WAL_PAGES, 0)?;
                let frames = healed.ok_or(MnemoError::HeaderChecksum)?;
                for (page_no, bytes) in &frames {
                    if bytes.len() != PAGE_SIZE {
                        return Err(MnemoError::Invalid("WAL frame is not page-sized".into()));
                    }
                    file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                    file.write_all(bytes)?;
                }
                file.sync_all()?;
                file.seek(SeekFrom::Start(0))?;
                file.read_exact(&mut hbuf)?;
                Header::from_page(&hbuf)?
            }
            Err(e) => return Err(e),
        };

        // Crash recovery: replay a committed-but-uncheckpointed transaction.
        // Each frame is a finished page image; the header frame, replayed to
        // page 0, supersedes the header just read.
        if let Some(frames) =
            wal::recover(&mut file, header.wal_start, header.wal_pages, header.wal_seq)?
        {
            for (page_no, bytes) in &frames {
                if bytes.len() != PAGE_SIZE {
                    return Err(MnemoError::Invalid("WAL frame is not page-sized".into()));
                }
                file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                file.write_all(bytes)?;
            }
            file.sync_all()?;
            // Re-read the now-current header from the replayed page 0.
            file.seek(SeekFrom::Start(0))?;
            file.read_exact(&mut hbuf)?;
            header = Header::from_page(&hbuf)?;
        }

        let kdf = KdfParams {
            m_cost: header.m_cost,
            t_cost: header.t_cost,
            p_cost: header.p_cost,
        };
        let kek = crypto::derive_kek(passphrase.as_bytes(), &header.salt, kdf)?;
        let dek = crypto::unwrap_dek(&kek, &header.dek_nonce, &header.wrapped_dek)?;

        let mut pager = Pager::new(file, dek, header.write_counter);
        let dimensions = header.dimensions as usize;

        let catalog = load_catalog(
            &mut pager,
            header.catalog_start,
            header.catalog_pages,
            header.catalog_len,
        )?;
        let index = build_id_index(&catalog);
        let ann = load_index(
            &mut pager,
            header.index_start,
            header.index_pages,
            header.index_len,
            dimensions,
        )?;
        let manifest = load_manifest(
            &mut pager,
            header.manifest_start,
            header.manifest_pages,
            header.manifest_len,
        )?;

        Ok(Mnemo {
            pager,
            header,
            catalog,
            index,
            path,
            dimensions,
            kdf,
            dirty_catalog: false,
            ann,
            dirty_index: false,
            manifest,
        })
    }

    /// Embedding dimensionality this database expects.
    pub fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Number of live (non-deleted) memories.
    pub fn len(&self) -> usize {
        self.catalog.iter().filter(|e| !e.deleted).count()
    }

    /// True if the database holds no live memories.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    // --- internal page/record helpers -------------------------------------

    fn write_record(&mut self, bytes: &[u8]) -> Result<(u64, u32)> {
        let pc = bytes.len().div_ceil(PAYLOAD).max(1);
        let start = self.header.next_page;
        self.header.next_page += pc as u64;
        for i in 0..pc {
            let lo = i * PAYLOAD;
            let hi = ((i + 1) * PAYLOAD).min(bytes.len());
            self.pager.write_page(start + i as u64, &bytes[lo..hi])?;
        }
        Ok((start, pc as u32))
    }

    fn read_record(&mut self, e: &CatalogEntry) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(e.len as usize);
        for i in 0..e.page_count as u64 {
            buf.extend_from_slice(&self.pager.read_page(e.start_page + i)?);
        }
        buf.truncate(e.len as usize);
        Ok(buf)
    }

    fn read_memory(&mut self, e: &CatalogEntry) -> Result<Memory> {
        let bytes = self.read_record(e)?;
        rmp_serde::from_slice(&bytes).map_err(|err| MnemoError::Serialize(err.to_string()))
    }

    /// Serialize and store a memory (insert or overwrite by ID).
    fn put(&mut self, mut m: Memory) -> Result<Ulid> {
        if m.vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: m.vector.len(),
            });
        }
        if m.id == Ulid::nil() {
            m.id = Ulid::new();
        }
        let id_u: u128 = m.id.0;
        let bytes = rmp_serde::to_vec(&m).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (start, pc) = self.write_record(&bytes)?;
        let entry = CatalogEntry {
            id: id_u,
            start_page: start,
            page_count: pc,
            len: bytes.len() as u32,
            deleted: false,
        };
        match self.index.get(&id_u).copied() {
            Some(idx) => self.catalog[idx] = entry,
            None => {
                self.index.insert(id_u, self.catalog.len());
                self.catalog.push(entry);
            }
        }
        self.dirty_catalog = true;

        // Keep the ANN index complete: assign the (new or changed) vector to
        // its nearest partition. Centroids/codebook stay fixed until rebuild.
        if let Some(ann) = &mut self.ann {
            ann.add(id_u, &m.vector);
            self.dirty_index = true;
        }
        Ok(m.id)
    }

    // --- public CRUD ------------------------------------------------------

    /// Store a memory. Returns its ULID. If the memory's ID is nil a fresh
    /// one is assigned; an existing ID overwrites in place.
    pub fn remember(&mut self, memory: Memory) -> Result<Ulid> {
        self.put(memory)
    }

    /// Fetch a memory by ID.
    pub fn get(&mut self, id: &Ulid) -> Result<Memory> {
        let idx = *self
            .index
            .get(&id.0)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        let entry = self.catalog[idx].clone();
        if entry.deleted {
            return Err(MnemoError::NotFound(id.to_string()));
        }
        self.read_memory(&entry)
    }

    /// Soft-delete a memory (tombstoned; space reclaimed by `compact`).
    pub fn delete(&mut self, id: &Ulid) -> Result<()> {
        let idx = *self
            .index
            .get(&id.0)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        if !self.catalog[idx].deleted {
            self.catalog[idx].deleted = true;
            self.dirty_catalog = true;
        }
        if let Some(ann) = &mut self.ann {
            ann.remove(id.0);
            self.dirty_index = true;
        }
        Ok(())
    }

    /// Return every live memory (used by tooling and compaction).
    pub fn memories(&mut self) -> Result<Vec<Memory>> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(entries.len());
        for e in &entries {
            out.push(self.read_memory(e)?);
        }
        Ok(out)
    }

    /// Return the database's **self-describing onboarding memories** — the
    /// ones tagged `metadata.area = "onboarding"`. This is the engine-level
    /// surface for the *single-file philosophy*: an agent who receives a
    /// `.mnemo` file (and its passphrase) can call this to learn what the
    /// file is, which embedder it expects, the recommended agent id, and any
    /// other conventions the file's author chose to record — all without
    /// needing any external documentation.
    ///
    /// Ordering: the canonical manifest (tag `metadata.topic = "manifest"`)
    /// always comes first; everything else follows in `importance` descending,
    /// then `created_at` ascending for deterministic results.
    pub fn about(&mut self) -> Result<Vec<Memory>> {
        let mut out: Vec<Memory> = self
            .memories()?
            .into_iter()
            .filter(|m| {
                m.metadata
                    .get("area")
                    .and_then(|v| v.as_str())
                    .map(|s| s.eq_ignore_ascii_case("onboarding"))
                    .unwrap_or(false)
            })
            .collect();
        out.sort_by(|a, b| {
            let a_manifest = is_manifest(a);
            let b_manifest = is_manifest(b);
            // Manifest topic wins first; then importance desc; then created_at asc.
            b_manifest
                .cmp(&a_manifest)
                .then_with(|| b.importance.total_cmp(&a.importance))
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        Ok(out)
    }

    // --- retrieval --------------------------------------------------------

    /// Phase-1 brute-force search: rank live memories by raw similarity only.
    /// Read-only — does not update access statistics.
    pub fn search(
        &mut self,
        query: &[f32],
        top_k: usize,
        metric: Metric,
    ) -> Result<Vec<(Memory, f32)>> {
        if query.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: query.len(),
            });
        }
        let now = memory::now_secs();
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();

        let mut scored: Vec<(Memory, f32)> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.is_expired(now) {
                continue;
            }
            let sim = memory::similarity(metric, query, &m.vector);
            scored.push((m, sim));
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Phase-5 multi-signal recall: rank by
    /// `α·similarity + β·recency + γ·importance + δ·ln(freq)`, with type and
    /// agent filtering. Updates `accessed_at` / `access_count` on the
    /// returned memories (persisted on the next `flush`).
    ///
    /// When an ANN index has been built ([`Mnemo::build_index`]), recall runs
    /// the tiered IVF→PQ→rerank pipeline: it scores only the similarity-nearest
    /// `n_rerank` candidates rather than every memory. Without an index it
    /// scores every live memory exactly.
    pub fn recall(&mut self, req: &RecallRequest) -> Result<Vec<RecallResult>> {
        if req.query.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: req.query.len(),
            });
        }
        let now = memory::now_secs();

        // Candidate set: ANN-narrowed when an index exists, else everything.
        let entries: Vec<CatalogEntry> = if let Some(ann) = &self.ann {
            let ids = ann.query(&req.query, req.n_probe, req.n_rerank);
            ids.iter()
                .filter_map(|id| self.index.get(id).copied())
                .map(|i| self.catalog[i].clone())
                .filter(|e| !e.deleted)
                .collect()
        } else {
            self.catalog
                .iter()
                .filter(|e| !e.deleted)
                .cloned()
                .collect()
        };

        let mut scored: Vec<RecallResult> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.is_expired(now) {
                continue;
            }
            // Type filter.
            if let Some(types) = &req.memory_types {
                if !types.contains(&m.memory_type) {
                    continue;
                }
            }
            // Agent-scoping: an agent sees its own memories plus shared ones.
            if let Some(agent) = &req.agent_id {
                let visible = m.agent_id == *agent || m.scope == Scope::Shared;
                if !visible {
                    continue;
                }
            }
            let sim = memory::similarity(req.metric, &req.query, &m.vector);
            let age = (now - m.accessed_at) as f32;
            let score = req.weights.score(sim, age, m.importance, m.access_count);
            scored.push(RecallResult { memory: m, score, similarity: sim });
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(req.top_k);

        // Update access statistics for everything we surfaced.
        for r in &mut scored {
            r.memory.accessed_at = now;
            r.memory.access_count = r.memory.access_count.saturating_add(1);
            self.put(r.memory.clone())?;
        }
        Ok(scored)
    }

    // --- approximate index ------------------------------------------------

    /// Build an IVF+PQ approximate-nearest-neighbour index over every live
    /// memory, using default tuning. After this, [`Mnemo::recall`] runs the
    /// tiered pipeline instead of an exact scan. Persisted on the next
    /// `flush`. Returns a snapshot of the index shape.
    pub fn build_index(&mut self) -> Result<IndexInfo> {
        self.build_index_with(IndexConfig::default())
    }

    /// Build the ANN index with explicit tuning.
    pub fn build_index_with(&mut self, cfg: IndexConfig) -> Result<IndexInfo> {
        let mems = self.memories()?;
        if mems.is_empty() {
            return Err(MnemoError::Invalid(
                "cannot build an index over an empty database".into(),
            ));
        }
        let items: Vec<(u128, &[f32])> =
            mems.iter().map(|m| (m.id.0, m.vector.as_slice())).collect();
        let idx = IvfPqIndex::build(self.dimensions, &items, cfg)?;
        let info = idx.info();
        self.ann = Some(idx);
        self.dirty_index = true;
        Ok(info)
    }

    /// Rebuild the ANN index from scratch — re-clusters centroids and retrains
    /// the PQ codebook, undoing the cluster drift that accumulates as memories
    /// are inserted against fixed centroids.
    pub fn rebuild_index(&mut self) -> Result<IndexInfo> {
        let cfg = match &self.ann {
            Some(a) => IndexConfig {
                n_probe: a.n_probe(),
                n_rerank: a.n_rerank(),
                ..Default::default()
            },
            None => IndexConfig::default(),
        };
        self.build_index_with(cfg)
    }

    /// Drop the ANN index; recall reverts to exact scans. Persisted on flush.
    pub fn drop_index(&mut self) {
        if self.ann.is_some() {
            self.ann = None;
            self.dirty_index = true;
        }
    }

    /// Whether an ANN index is currently loaded.
    pub fn has_index(&self) -> bool {
        self.ann.is_some()
    }

    // --- durability & maintenance ----------------------------------------

    /// Seal a serialized buffer into a fresh run of encrypted home pages and
    /// append a WAL frame for each. Returns the run's `(start_page, pages)`.
    /// The pages are *not* written to their home locations here — they go to
    /// the WAL and are folded in later by [`Mnemo::checkpoint`].
    fn seal_run(&mut self, bytes: &[u8], frames: &mut Vec<wal::Frame>) -> Result<(u64, u32)> {
        let pc = bytes.len().div_ceil(PAYLOAD).max(1);
        let start = self.header.next_page;
        self.header.next_page += pc as u64;
        for i in 0..pc {
            let lo = i * PAYLOAD;
            let hi = ((i + 1) * PAYLOAD).min(bytes.len());
            let page_no = start + i as u64;
            let sealed = self.pager.seal_page(page_no, &bytes[lo..hi])?;
            frames.push((page_no, sealed.to_vec()));
        }
        Ok((start, pc as u32))
    }

    /// Grow the WAL region if `frame_pages` worth of frames would not fit.
    /// Called at the top of `flush`, where the WAL is always spent (every
    /// flush ends with a checkpoint), so relocating it cannot strand a
    /// committed transaction.
    fn ensure_wal_capacity(&mut self, frame_pages: usize) -> Result<()> {
        let need = wal::txn_byte_len(frame_pages, PAGE_SIZE);
        if need <= self.header.wal_pages * PAGE_SIZE as u64 {
            return Ok(());
        }
        // Allocate a new, larger region at the file tail (~1.5x headroom).
        let req = need.div_ceil(PAGE_SIZE as u64);
        let new_pages = (req + req / 2 + 4).max(DEFAULT_WAL_PAGES);
        let new_start = self.header.next_page;
        self.header.next_page += new_pages;
        self.header.wal_start = new_start;
        self.header.wal_pages = new_pages;
        // Persist the relocation now: an isolated header write over an empty
        // WAL. A crash here leaves a consistent state with a bigger WAL.
        self.header.write_counter = self.pager.write_counter;
        self.pager.write_raw(0, &self.header.to_page())?;
        self.pager.sync()?;
        Ok(())
    }

    /// Fold a committed transaction's WAL frames into their home pages.
    fn checkpoint(&mut self, frames: &[wal::Frame]) -> Result<()> {
        for (page_no, bytes) in frames {
            let mut img = [0u8; PAGE_SIZE];
            img.copy_from_slice(bytes);
            self.pager.write_sealed(*page_no, &img)?;
        }
        self.pager.sync()?;
        Ok(())
    }

    /// Persist all pending changes as one **write-ahead-logged transaction**.
    ///
    /// Record data pages are written copy-on-write to fresh pages and fsynced;
    /// the new catalog, ANN index, and header are then logged to the WAL and
    /// committed with a single fsync — the durability point. A checkpoint
    /// folds the WAL into the home pages. A crash before the commit leaves the
    /// previous state intact; a crash after it is repaired by [`Mnemo::open`]
    /// replaying the WAL. Safe to call repeatedly.
    pub fn flush(&mut self) -> Result<()> {
        // 1. Record (vector) data pages: copy-on-write to fresh pages, fsynced.
        //    They are unreferenced until the catalog below commits.
        self.pager.flush()?;

        if !self.dirty_catalog && !self.dirty_index {
            return Ok(());
        }

        // 2. Serialize the control plane this transaction will commit.
        let cat_bytes = if self.dirty_catalog {
            Some(
                rmp_serde::to_vec(&self.catalog)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?,
            )
        } else {
            None
        };
        let idx_bytes: Option<Vec<u8>> = if self.dirty_index {
            match &self.ann {
                Some(ann) => Some(
                    rmp_serde::to_vec(ann).map_err(|e| MnemoError::Serialize(e.to_string()))?,
                ),
                None => None,
            }
        } else {
            None
        };

        // 3. Size the WAL for the catalog, index, manifest and header pages.
        let cat_pc = cat_bytes.as_ref().map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        let idx_pc = idx_bytes.as_ref().map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        // Upper bound on the manifest run: one extra entry, <=82 bytes each.
        let man_upper = 9 + (self.manifest.len() + 1) * 82;
        let man_pc_est = man_upper.div_ceil(PAYLOAD).max(1);
        self.ensure_wal_capacity(cat_pc + idx_pc + man_pc_est + 1)?;

        // 4. Seal the catalog / index control pages into fresh home runs.
        let mut frames: Vec<wal::Frame> = Vec::new();
        if let Some(bytes) = &cat_bytes {
            let (start, pages) = self.seal_run(bytes, &mut frames)?;
            self.header.catalog_start = start;
            self.header.catalog_pages = pages as u64;
            self.header.catalog_len = bytes.len() as u64;
        }
        if self.dirty_index {
            match &idx_bytes {
                Some(bytes) => {
                    let (start, pages) = self.seal_run(bytes, &mut frames)?;
                    self.header.index_start = start;
                    self.header.index_pages = pages as u64;
                    self.header.index_len = bytes.len() as u64;
                }
                None => {
                    self.header.index_start = 0;
                    self.header.index_pages = 0;
                    self.header.index_len = 0;
                }
            }
        }

        // 5. Record this transaction as a restorable snapshot. The manifest
        //    update is staged in a local and adopted only once the commit
        //    below succeeds, so a failed flush leaves no phantom entry.
        let live = self.len() as u64;
        let txn_id = self.header.wal_seq + 1;
        let mut manifest = self.manifest.clone();
        manifest.push(Snapshot {
            txn_id,
            created_at: memory::now_secs(),
            catalog_start: self.header.catalog_start,
            catalog_pages: self.header.catalog_pages,
            catalog_len: self.header.catalog_len,
            index_start: self.header.index_start,
            index_pages: self.header.index_pages,
            index_len: self.header.index_len,
            memory_count: live,
        });
        let man_bytes =
            rmp_serde::to_vec(&manifest).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (m_start, m_pages) = self.seal_run(&man_bytes, &mut frames)?;
        self.header.manifest_start = m_start;
        self.header.manifest_pages = m_pages as u64;
        self.header.manifest_len = man_bytes.len() as u64;

        // 6. The header is the transaction's final frame; stamp the new id.
        self.header.vector_count = live;
        self.header.write_counter = self.pager.write_counter;
        self.header.wal_seq = txn_id;
        frames.push((0, self.header.to_page().to_vec()));

        // 7. COMMIT — log the transaction and fsync. Nothing at a home page
        //    has changed yet; this single fsync is what makes it durable.
        let (wal_start, wal_pages) = (self.header.wal_start, self.header.wal_pages);
        wal::commit(self.pager.file_mut(), wal_start, wal_pages, txn_id, &frames)?;

        // 8. Checkpoint — fold the WAL into the home pages.
        self.checkpoint(&frames)?;

        self.manifest = manifest;
        self.dirty_catalog = false;
        self.dirty_index = false;
        Ok(())
    }

    /// Flush and close. Equivalent to `flush()`; the file is released on drop.
    pub fn close(&mut self) -> Result<()> {
        self.flush()
    }

    /// Bound the in-memory page cache to `pages` decrypted pages.
    ///
    /// The cache holds decrypted page payloads to speed repeated reads. By
    /// default it is capped at 8192 pages (~64 MiB); lower the cap to trade
    /// hit rate for a smaller footprint, or raise it for a hotter cache. The
    /// cap governs *clean* pages — pages with un-flushed writes are always
    /// retained until [`Mnemo::flush`], regardless of the cap.
    pub fn set_cache_capacity(&mut self, pages: usize) {
        self.pager.set_cache_capacity(pages);
    }

    /// Page-cache occupancy: `(pages_cached, capacity)`.
    pub fn cache_stats(&self) -> (usize, usize) {
        self.pager.cache_stats()
    }

    /// Begin a conversation [`Session`](crate::Session) for `agent_id`.
    ///
    /// The session borrows the database for its lifetime, records turns as
    /// working memory, and consolidates them into episodic memory when closed.
    pub fn session(&mut self, agent_id: impl Into<String>) -> crate::session::Session<'_> {
        crate::session::Session::new(self, agent_id.into())
    }

    // --- snapshots & point-in-time recovery ------------------------------

    /// Every committed transaction, oldest first — the restore points
    /// available to [`Mnemo::restore_to`] and [`Mnemo::restore_to_time`].
    ///
    /// Each `flush` appends one snapshot. Because the storage engine is
    /// append-only, the pages a past flush wrote are still on disk, so any
    /// listed snapshot can be reinstated exactly. The history reaches back to
    /// the last [`Mnemo::compact_file`], which reclaims space by collapsing
    /// it.
    pub fn snapshots(&self) -> Vec<SnapshotInfo> {
        let mut v: Vec<SnapshotInfo> = self
            .manifest
            .iter()
            .map(|s| SnapshotInfo {
                txn_id: s.txn_id,
                created_at: s.created_at,
                memory_count: s.memory_count,
            })
            .collect();
        v.sort_by_key(|s| s.txn_id);
        v
    }

    /// Load a past snapshot's state and commit it as a new transaction.
    fn apply_snapshot(&mut self, snap: &Snapshot) -> Result<()> {
        let catalog = load_catalog(
            &mut self.pager,
            snap.catalog_start,
            snap.catalog_pages,
            snap.catalog_len,
        )?;
        let ann = load_index(
            &mut self.pager,
            snap.index_start,
            snap.index_pages,
            snap.index_len,
            self.dimensions,
        )?;
        self.index = build_id_index(&catalog);
        self.catalog = catalog;
        self.ann = ann;
        // Re-commit as a fresh transaction: crash-safe, and itself recorded
        // as a new snapshot so a restore can always be undone.
        self.dirty_catalog = true;
        self.dirty_index = true;
        self.flush()
    }

    /// Restore the database to the snapshot produced by transaction `txn_id`.
    ///
    /// The restore is itself a new committed transaction (and a new
    /// snapshot), so it is crash-safe and reversible — restoring forward to a
    /// later snapshot afterwards works just as well.
    pub fn restore_to(&mut self, txn_id: u64) -> Result<SnapshotInfo> {
        let snap = self
            .manifest
            .iter()
            .find(|s| s.txn_id == txn_id)
            .cloned()
            .ok_or_else(|| MnemoError::NotFound(format!("snapshot for transaction {txn_id}")))?;
        self.apply_snapshot(&snap)?;
        Ok(SnapshotInfo {
            txn_id: snap.txn_id,
            created_at: snap.created_at,
            memory_count: snap.memory_count,
        })
    }

    /// Restore the database to the latest snapshot committed at or before
    /// `unix_secs`. Returns [`MnemoError::NotFound`] if no snapshot is that
    /// old. Like [`Mnemo::restore_to`], the restore is a new transaction.
    pub fn restore_to_time(&mut self, unix_secs: i64) -> Result<SnapshotInfo> {
        let snap = self
            .manifest
            .iter()
            .filter(|s| s.created_at <= unix_secs)
            .max_by_key(|s| s.txn_id)
            .cloned()
            .ok_or_else(|| {
                MnemoError::NotFound(format!("no snapshot at or before time {unix_secs}"))
            })?;
        self.apply_snapshot(&snap)?;
        Ok(SnapshotInfo {
            txn_id: snap.txn_id,
            created_at: snap.created_at,
            memory_count: snap.memory_count,
        })
    }

    /// Change the passphrase. Cheap: re-derives the KEK and re-wraps the DEK;
    /// the encrypted pages are never rewritten.
    pub fn rekey(&mut self, new_passphrase: &str, kdf: KdfParams) -> Result<()> {
        self.flush()?;
        let new_salt = crypto::random_salt();
        let new_kek = crypto::derive_kek(new_passphrase.as_bytes(), &new_salt, kdf)?;
        let new_nonce = crypto::random_nonce();
        let wrapped = crypto::wrap_dek(&new_kek, &new_nonce, self.pager.dek())?;
        if wrapped.len() != WRAPPED_DEK_LEN {
            return Err(MnemoError::Crypto("unexpected wrapped DEK length".into()));
        }
        let mut wrapped_dek = [0u8; WRAPPED_DEK_LEN];
        wrapped_dek.copy_from_slice(&wrapped);

        self.header.salt = new_salt;
        self.header.dek_nonce = new_nonce;
        self.header.wrapped_dek = wrapped_dek;
        self.header.m_cost = kdf.m_cost;
        self.header.t_cost = kdf.t_cost;
        self.header.p_cost = kdf.p_cost;
        self.header.write_counter = self.pager.write_counter;
        self.kdf = kdf;

        self.pager.write_raw(0, &self.header.to_page())?;
        self.pager.sync()?;
        Ok(())
    }

    /// Decrypt and validate every live record. Returns the count verified.
    pub fn verify(&mut self) -> Result<usize> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let n = entries.len();
        for e in &entries {
            let bytes = self.read_record(e)?;
            let _m: Memory = rmp_serde::from_slice(&bytes)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
        }
        Ok(n)
    }

    /// Summary statistics for the open database.
    pub fn stats(&mut self) -> Result<Stats> {
        let deleted = self.catalog.iter().filter(|e| e.deleted).count();
        let mut agents: Vec<String> = self
            .memories()?
            .into_iter()
            .map(|m| m.agent_id)
            .collect();
        agents.sort();
        agents.dedup();
        let file_bytes = self.header.next_page * PAGE_SIZE as u64;
        Ok(Stats {
            memories: self.len(),
            deleted,
            dimensions: self.dimensions,
            file_bytes,
            agents,
            encrypted: self.header.flags & FLAG_ENCRYPTED != 0,
            created_at: self.header.created_at,
            index: self.ann.as_ref().map(|a| a.info()),
            wal_pages: self.header.wal_pages,
        })
    }

    /// Rewrite the file, dropping tombstoned and expired memories and
    /// reclaiming stale pages left by updates. Done out-of-place via a temp
    /// file and an atomic rename.
    pub fn compact_file(path: &str, passphrase: &str) -> Result<CompactReport> {
        let mut old = Mnemo::open(path, passphrase)?;
        let dims = old.dimensions;
        let kdf = old.kdf;
        let tmp = format!("{path}.compact-tmp");

        let mut new = Mnemo::create(&tmp, passphrase, MnemoConfig { dimensions: dims, kdf })?;
        let want_index = old.ann.as_ref().map(|a| (a.n_probe(), a.n_rerank()));
        let now = memory::now_secs();
        let all = old.memories()?;
        let before = all.len();
        let mut after = 0;
        for m in all {
            if m.is_expired(now) {
                continue;
            }
            new.remember(m)?;
            after += 1;
        }
        // Rebuild the index fresh (re-clustered) when the source had one.
        if let Some((n_probe, n_rerank)) = want_index {
            if after > 0 {
                new.build_index_with(IndexConfig {
                    n_probe,
                    n_rerank,
                    ..Default::default()
                })?;
            }
        }
        new.flush()?;
        drop(new);
        drop(old);

        std::fs::rename(&tmp, path)?;
        Ok(CompactReport { before, after })
    }
}
