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
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ulid::Ulid;

use crate::crypto::{self, KdfParams};
use crate::embed::{self, Embedder};
use crate::error::{MnemoError, Result};
use crate::format::{Header, FLAG_ENCRYPTED, PAGE_SIZE, PAYLOAD, VERSION, WRAPPED_DEK_LEN};
use crate::index::{IndexConfig, IndexInfo, IvfPqIndex};
use crate::memory::{
    self, status as essence_status, CommitBatch, CommitReport, EssenceOpKind, EssenceScope,
    EssenceSection, Filter, IngestReport, KeyScope, Memory, MemoryPatch, MemoryType, Metric, Scope,
    ScoreWeights, ThreadTurn, TurnPolicy, WriteOutcome, CONTINUATION_TIMELINE_ENTRIES,
    ESSENCE_DEFAULT_RENDER_BYTES, ESSENCE_TRANSITIONS, MNEMO_META_CONTINUES, MNEMO_META_INVALID_AT,
    MNEMO_META_KEY, MNEMO_META_KIND, MNEMO_META_KIND_ESSENCE, MNEMO_META_KIND_THREAD_LINK,
    MNEMO_META_KIND_TURN, MNEMO_META_LENGTH, MNEMO_META_PART, MNEMO_META_REFS, MNEMO_META_ROLE,
    MNEMO_META_SECTION, MNEMO_META_SEQ, MNEMO_META_SHA256, MNEMO_META_STATUS,
    MNEMO_META_STATUS_HISTORY, MNEMO_META_SUPERSEDED_BY, MNEMO_META_SUPERSEDES, MNEMO_META_THREAD,
    MNEMO_META_TURN_ID, MNEMO_META_UPDATED_AT, THREAD_TOOL_RESULT_TRUNCATE_BYTES,
    THREAD_TURN_CHUNK_BYTES,
};
use crate::pager::Pager;
use crate::result_cache::{
    self, BatchState, CacheBudget, CacheDirectoryEntry, CacheEntry, CacheFlushPolicy, CachePutOpts,
    CacheStats, CachedValue, CounterState, DirectoryIndex, SemanticCachePutOpts,
};
use crate::wal;

/// Default initial size of the write-ahead log region, in pages (64 KiB).
/// The region grows automatically when a transaction's control plane outgrows
/// it, so this is a *floor* on the fresh-file footprint, not a cap on
/// transaction size. Was 64 pages (512 KiB) in v0.1.0 — lowered because that
/// reservation dominated small-file size (~62% of a 31-memory dogfood file).
const DEFAULT_WAL_PAGES: u64 = 8;

/// Hard floor on the WAL reservation. A single header-page flush already
/// needs one full DATA frame plus a COMMIT frame; two pages are the bare
/// minimum that lets the first transaction commit without immediate growth.
const MIN_WAL_PAGES: u64 = 2;

/// Default cap on retained snapshot manifest entries. Each `flush` appends
/// one entry; without a cap, long-running processes accumulate entries
/// forever, growing per-flush manifest-serialize cost as O(total flushes)
/// and the manifest run size on disk in lockstep. 256 keeps roughly a
/// week of hourly snapshots while staying tiny (256 × ~82 bytes ≈ 21 KiB
/// of manifest). Set `MnemoConfig::max_snapshots = 0` to disable the cap.
pub const DEFAULT_MAX_SNAPSHOTS: usize = 256;

/// Chunk size for the torn-header scan's streaming read of the file.
/// We walk the file 1 MiB at a time looking for 8 KiB-aligned `MWAL`
/// magic. There is **no** cap on how far a given candidate's frame
/// chain may extend — a 1M-vector IVF+PQ relocation can push a single
/// transaction well past 16 MiB — so each candidate is read frame by
/// frame via [`wal::scan_committed_txn`] rather than slurped into a
/// bounded buffer. (DUR-1 R-002.1 part 2.)
const TORN_HEADER_SCAN_CHUNK: usize = 1 << 20; // 1 MiB

/// Walk the file for every 8 KiB-aligned page whose first four bytes are
/// `MWAL`, treating each as a candidate WAL-region start, and return the
/// frames of the highest-`txn_id` committed transaction whose header
/// frame fully validates under `passphrase`.
///
/// A candidate is accepted only if:
/// * `wal::scan_committed_txn` returns a committed transaction (chained
///   CRC verifies, single `txn_id`, `KIND_COMMIT` closes it);
/// * that transaction contains a `(0, header_page)` frame;
/// * the header parses with `wal_start == candidate_page_no` and
///   `wal_seq == txn_id` (so a frame cannot forge a region it was not
///   written into);
/// * deriving the KEK from `passphrase + header.salt`, unwrapping the
///   wrapped DEK, and validating the header seal all succeed.
///
/// Returns `Ok(None)` when the scan finds no valid candidate — the
/// caller is expected to propagate the original parse error so
/// non-mnemo files still report `BadMagic`.
///
/// DUR-1 (R-002.1 part 2): there is no fixed scan window — the whole
/// file is scanned. Reading at page granularity (not byte granularity)
/// is sufficient because every WAL region in the format is page-aligned
/// and every WAL commit writes its first frame at the region's start.
fn scan_header_recovery_candidates(
    file: &mut File,
    passphrase: &str,
) -> Result<Option<Vec<wal::Frame>>> {
    let file_len = file.seek(SeekFrom::End(0))?;
    if file_len < PAGE_SIZE as u64 * 2 {
        return Ok(None); // No room for even one WAL page past the header.
    }

    let mut best: Option<(u64, Vec<wal::Frame>)> = None;
    let page_sz = PAGE_SIZE as u64;
    let mut offset = page_sz; // Skip page 0.

    // Stream through the file in 1 MiB chunks, scanning each chunk for
    // page-aligned `MWAL` candidates. Each candidate's frame chain is
    // then read incrementally via `wal::scan_committed_txn`, which has
    // no window cap — a 1M-vector IVF+PQ relocation can span tens of
    // MiB in one committed transaction.
    let mut chunk = vec![0u8; TORN_HEADER_SCAN_CHUNK];
    while offset < file_len {
        let want = (TORN_HEADER_SCAN_CHUNK as u64).min(file_len - offset) as usize;
        chunk.resize(want, 0);
        file.seek(SeekFrom::Start(offset))?;
        let mut filled = 0;
        while filled < want {
            match file.read(&mut chunk[filled..])? {
                0 => break,
                n => filled += n,
            }
        }
        if filled == 0 {
            break;
        }
        let mut inner = 0usize;
        while inner + 4 <= filled {
            if &chunk[inner..inner + 4] == wal::FRAME_MAGIC_BYTES {
                let page_no = (offset + inner as u64) / page_sz;
                if let Some((txn_id, frames)) = try_recover_candidate(file, page_no, passphrase)? {
                    let keep = match &best {
                        None => true,
                        Some((id, _)) => txn_id > *id,
                    };
                    if keep {
                        best = Some((txn_id, frames));
                    }
                }
            }
            inner += PAGE_SIZE;
        }
        offset += filled as u64;
    }

    Ok(best.map(|(_, f)| f))
}

/// Try to recover a committed transaction from `candidate_page_no`,
/// returning `(txn_id, frames)` only if the candidate's
/// `(0, header_page)` frame fully validates: the frame chain commits
/// (via [`wal::scan_committed_txn`]'s streaming reader), the header
/// parses with `wal_start == candidate_page_no` **and**
/// `wal_seq == txn_id`, the DEK unwraps under `passphrase`, and the
/// AES-GCM seal validates. DUR-1 (R-002.1 item 2) requires the
/// `wal_seq == txn_id` check and no fixed scan window — the streaming
/// reader handles transactions of arbitrary size.
fn try_recover_candidate(
    file: &mut File,
    candidate_page_no: u64,
    passphrase: &str,
) -> Result<Option<(u64, Vec<wal::Frame>)>> {
    let Some((txn_id, frames)) = wal::scan_committed_txn(file, candidate_page_no)? else {
        return Ok(None);
    };

    // Must contain a header frame at page 0.
    let header_bytes = match frames.iter().find(|(p, _)| *p == 0) {
        Some((_, b)) => b,
        None => return Ok(None),
    };
    if header_bytes.len() != PAGE_SIZE {
        return Ok(None);
    }
    let mut hbuf = [0u8; PAGE_SIZE];
    hbuf.copy_from_slice(header_bytes);
    let candidate = match Header::from_page(&hbuf) {
        Ok(h) => h,
        Err(_) => return Ok(None),
    };

    // The frame's claimed region must match where we found it, and the
    // header frame's `wal_seq` must match the committed transaction's
    // `txn_id` — this is what pins the frame to the region it was
    // written into and keeps a stale checkpointed-and-copied frame
    // from being accepted as a new region's root.
    if candidate.wal_start != candidate_page_no || candidate.wal_seq != txn_id {
        return Ok(None);
    }
    // DEK unwrap under this candidate's own salt + KDF params.
    let kdf = KdfParams {
        m_cost: candidate.m_cost,
        t_cost: candidate.t_cost,
        p_cost: candidate.p_cost,
    };
    let kek = match crypto::derive_kek(passphrase.as_bytes(), &candidate.salt, kdf) {
        Ok(k) => k,
        Err(_) => return Ok(None),
    };
    let dek = match crypto::unwrap_dek(&kek, &candidate.dek_nonce, &candidate.wrapped_dek) {
        Ok(d) => d,
        Err(_) => return Ok(None),
    };
    if candidate.validate_seal(&dek).is_err() {
        return Ok(None);
    }

    Ok(Some((txn_id, frames)))
}

/// Configuration for creating a new database.
#[derive(Clone, Copy, Debug)]
pub struct MnemoConfig {
    /// Embedding dimensionality. Every stored vector must match this.
    pub dimensions: usize,
    /// Key-derivation parameters.
    pub kdf: KdfParams,
    /// Initial size of the WAL region in 8 KiB pages. Default is
    /// [`DEFAULT_WAL_PAGES`] (8 pages = 64 KiB), which keeps fresh files
    /// small; the region auto-grows when a transaction's control plane needs
    /// more space, so raising this is only a hint for the expected steady-state
    /// transaction size. Write-heavy databases that routinely flush large
    /// catalogs or ANN indexes can set this higher (e.g. 64 = 512 KiB) to
    /// avoid early grow events; tiny embedded uses can leave it at the
    /// default. Clamped to a minimum of [`MIN_WAL_PAGES`] (2 pages).
    pub wal_pages_initial: u64,
    /// Maximum number of snapshot manifest entries to retain. Each `flush`
    /// appends one entry; once the cap is hit, the *oldest* entry is dropped
    /// from the in-memory manifest before the new one is added, so the
    /// retained set is always the most-recent N. Defaults to
    /// [`DEFAULT_MAX_SNAPSHOTS`] (256). Set to `0` to disable the cap and
    /// retain every snapshot forever (the pre-v0.3 behavior).
    ///
    /// Pages referenced *only* by pruned snapshots stay on disk until the
    /// next [`Mnemo::compact_file`], which reclaims them. Point-in-time
    /// recovery via [`Mnemo::restore_to`] into a pruned `txn_id` returns
    /// [`MnemoError::NotFound`].
    ///
    /// Apply to an already-open database with [`Mnemo::set_max_snapshots`].
    pub max_snapshots: usize,
}

impl Default for MnemoConfig {
    fn default() -> Self {
        Self {
            dimensions: 768,
            kdf: KdfParams::secure(),
            wal_pages_initial: DEFAULT_WAL_PAGES,
            max_snapshots: DEFAULT_MAX_SNAPSHOTS,
        }
    }
}

/// Catalog entry: maps a memory ID to its page run on disk.
///
/// **Schema (v5+):** carries `accessed_at` and `access_count` so `recall`
/// can update access stats by mutating the catalog entry rather than
/// rewriting the entire record (Phase 2.1 of the improvement plan). The
/// values in a memory's serialized record body become a stale snapshot
/// after the first recall; [`Mnemo::read_memory`] re-populates them from
/// the catalog so consumers always see the live values.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct CatalogEntry {
    /// ULID as a raw `u128`.
    id: u128,
    start_page: u64,
    page_count: u32,
    /// Exact serialized byte length of the record.
    len: u32,
    deleted: bool,
    /// Unix seconds of the most recent recall hit (or write, for fresh entries).
    accessed_at: i64,
    /// How many times `recall` has surfaced this memory.
    access_count: u32,
}

/// Frozen v4 catalog entry shape, used only by the v4→v5 migration path in
/// [`Mnemo::open`]. Its layout matches the catalog encoding written by
/// pre-v5 builds (5 positional fields). Do not change this struct.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct CatalogEntryV4 {
    id: u128,
    start_page: u64,
    page_count: u32,
    len: u32,
    deleted: bool,
}

/// Returned by [`Mnemo::prepare_for_flush`] and consumed by [`Mnemo::flush`].
/// Carries the already-serialized control-plane bytes through the prelude
/// so we don't re-serialize after the lease has been persisted.
struct FlushPrelude {
    cat_bytes: Option<Vec<u8>>,
    idx_bytes: Option<Vec<u8>>,
    /// Serialized result-cache directory bytes when `dirty_cache` is
    /// set (Phase 10.1). `None` if the cache didn't change since the
    /// last flush — in that case the header keeps its current
    /// `cache_start` / `cache_pages` / `cache_len` and no fresh cache
    /// run is written.
    cache_bytes: Option<Vec<u8>>,
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
    /// Whether to update each result's `accessed_at` and `access_count` in
    /// the catalog. Defaults to `true`, matching pre-v5 behavior.
    ///
    /// Set to `false` for **fully read-only recall** — useful for batch
    /// scoring, dry-run scoring, or recalls run by introspection tooling
    /// (e.g. `mnemo about` consumers) where you don't want the score's own
    /// observation to perturb the database. With `false`, recall does not
    /// dirty the catalog and the next `flush` is a no-op.
    pub track_access: bool,
    /// Phase 12.6 pre-scoring filter: `session_id` + AND-ed metadata
    /// conditions applied before the candidate is scored. Default is
    /// [`Filter::default`] (no constraints). When the ANN index is on
    /// and this filter is selective enough to starve the shortlist
    /// below `top_k`, Phase 12.1's widen-then-exact-scan logic kicks
    /// in so the result still carries up to `top_k` correct hits.
    pub filter: Filter,
    /// Phase 12.8 post-scoring budget: stop adding hits once the UTF-8
    /// bytes of their `content` fields (not the serialized record
    /// body) would exceed this many bytes. `None` (default) means no
    /// budget and every hit through `top_k` is returned. If the top-1
    /// hit is already over budget it is still returned — callers that
    /// need an absolute cap should also pre-size `top_k`.
    ///
    /// Counts only the UTF-8 `content` bytes so the number is stable
    /// across the Rust / Python / CLI / MCP surfaces; the JSON or
    /// MessagePack envelope is excluded. Roughly 4 bytes per BPE
    /// token for English text — a 4 KB budget comfortably holds ~1K
    /// tokens.
    ///
    /// Observable through [`RecallResponse::truncated`]
    /// ([`Mnemo::recall_budgeted`]): `true` iff at least one scored
    /// hit was dropped by this budget, `false` otherwise.
    pub max_bytes: Option<usize>,
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
            track_access: true,
            filter: Filter::default(),
            max_bytes: None,
        }
    }

    /// Phase 12.6: attach a pre-scoring filter. See [`Filter`] and
    /// [`FilterTerm`] for term shapes.
    pub fn filter(mut self, filter: Filter) -> Self {
        self.filter = filter;
        self
    }

    /// Phase 12.8: cap the total UTF-8 bytes of returned `content`
    /// fields. See [`RecallRequest::max_bytes`].
    pub fn max_bytes(mut self, n: usize) -> Self {
        self.max_bytes = Some(n);
        self
    }

    /// Build a request by embedding `query_text` with the supplied
    /// embedder (Phase 3). Shorthand for `embedder.embed(&[text])` +
    /// [`RecallRequest::new`]; same defaults as `new`. Fails if the
    /// embedder returns an empty vector list or an embedding failure.
    pub fn from_text(embedder: &dyn Embedder, query_text: &str) -> Result<Self> {
        let mut vectors = embedder.embed(&[query_text])?;
        let v = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the query text".into())
        })?;
        Ok(Self::new(v))
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
    /// Toggle whether recall updates `accessed_at` / `access_count` on
    /// the returned memories' catalog entries (default `true`). Pass
    /// `false` for a fully read-only recall — see the field doc.
    pub fn track_access(mut self, track: bool) -> Self {
        self.track_access = track;
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

/// Phase 12.8 budgeted-recall envelope returned by
/// [`Mnemo::recall_budgeted`]. Wraps the `Vec<RecallResult>` the
/// legacy [`Mnemo::recall`] returns so callers can tell whether
/// [`RecallRequest::max_bytes`] cut anything.
#[derive(Clone, Debug)]
pub struct RecallResponse {
    /// Scored hits, best first, truncated to at most `top_k` and then
    /// to fit the `max_bytes` budget if set.
    pub hits: Vec<RecallResult>,
    /// `true` iff the `max_bytes` budget dropped at least one hit
    /// that would otherwise have been returned (i.e. the result is
    /// shorter than what `top_k` alone would produce).
    pub truncated: bool,
    /// Phase 12.1 + 12.6 observability: `true` iff recall fell
    /// through to an exact scan (ANN shortlist too narrow after
    /// filtering, and the widen pass also came up short, so the
    /// index was skipped for one full-corpus pass). Monitoring on
    /// this flag is how callers detect selective-filter regressions
    /// or supersession chains outgrowing the ANN shortlist.
    pub used_exact_scan: bool,
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
    /// Fsyncs issued by the most recent successful [`Mnemo::flush`] on
    /// this handle. `0` on a fresh handle before any flush, or when
    /// `flush()` short-circuited (nothing dirty). Phase 6 instrumentation
    /// for the Phase 11.1 write-latency work — the planner wants this
    /// before deciding whether cutting fsync count is a viable
    /// durability-preserving fix.
    pub fsyncs_last_flush: u32,
}

/// Result of a [`Mnemo::compact_file`] run.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompactReport {
    /// Live memories before compaction.
    pub before: usize,
    /// Live memories after compaction (expired ones dropped).
    pub after: usize,
    /// Live, non-expired cache entries before compaction. FIX-1
    /// R-001.1: compaction now preserves the result cache instead of
    /// silently dropping it.
    pub cache_before: usize,
    /// Live, non-expired cache entries after compaction (tombstoned
    /// and TTL-expired ones dropped).
    pub cache_after: usize,
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
    /// Append-only manifest of committed snapshots, oldest first. Capped
    /// at [`Mnemo::max_snapshots`] entries on every flush — the oldest
    /// entries are pruned first.
    manifest: Vec<Snapshot>,
    /// Maximum manifest entries to retain across flushes. `0` disables
    /// the cap (retain forever). Defaults to [`DEFAULT_MAX_SNAPSHOTS`].
    max_snapshots: usize,
    /// If `true`, this handle was opened via [`Mnemo::open_read_only`] and
    /// every mutating API returns [`MnemoError::ReadOnly`]. The underlying
    /// file was opened without write permission and holds an OS shared
    /// lock; the OS would reject a write() anyway, but gating in Rust
    /// gives callers a clean, typed error.
    read_only: bool,
    /// Result-cache directory (Phase 10.1). See [`crate::result_cache`].
    /// Loaded from `header.cache_start` on open; persisted on `flush`
    /// via the same seal_run pipeline as catalog/index/manifest.
    cache_dir: Vec<CacheDirectoryEntry>,
    /// In-memory index over `cache_dir` for O(1) `(namespace, key_hash)`
    /// lookups. Rebuilt on open + kept in sync on `cache_put`/`delete`.
    cache_idx: DirectoryIndex,
    /// Set whenever `cache_dir` changes; drives whether `flush` rewrites
    /// the cache directory run.
    dirty_cache: bool,
    /// Per-namespace eviction budgets. Missing entries fall back to
    /// [`CacheBudget::default`]. Not persisted (per-handle policy).
    cache_budgets: std::collections::HashMap<String, CacheBudget>,
    /// Flush policy for cache mutations. Memory writes ignore this and
    /// always behave as `Strict`.
    cache_flush_policy: CacheFlushPolicy,
    /// Bookkeeping for `CacheFlushPolicy::Batched` — pending mutation
    /// count and first-dirty timestamp.
    cache_batch: BatchState,
    /// In-memory hit/miss/eviction counters (reset on reopen).
    cache_counters: CounterState,
    /// Optional handle-scoped embedder (Phase 3, PR 10a). Registered
    /// via [`Mnemo::set_embedder`]; consulted by [`Mnemo::remember_text`]
    /// and [`RecallRequest::from_text`]. `None` means text-side APIs
    /// return [`MnemoError::Invalid`] with a "no embedder" message.
    /// Not persisted — the handle is re-registered after every open.
    embedder: Option<std::sync::Arc<dyn Embedder>>,
    /// Fsyncs issued by the most recent `flush()` call. Snapshotted
    /// from the thread-local fsync counter in `lib.rs::fsync_counter`:
    /// `flush()` reads it on entry, does its work (each `sync_all` the
    /// pager / wal / recovery path makes bumps the counter), and
    /// stores the delta here. `0` on a fresh handle before any flush,
    /// or on a read-only handle where `flush()` short-circuits.
    /// Exposed through [`Stats::fsyncs_last_flush`] for diagnostics —
    /// the Phase 6 planner wants this before deciding whether cutting
    /// fsync count is a viable write-latency win.
    fsyncs_last_flush: u32,
    /// FIX-1 R-001.8 part 2: lazy cache of the distinct agent IDs
    /// across every live memory, built once on first `stats()` call
    /// per handle. `remember()` adds to it in O(1); `delete()` can't
    /// cheaply tell whether the removed memory was the last of its
    /// agent (the agent lives in the encrypted body, not the catalog
    /// — pre-v9 format), so it just invalidates the set and the next
    /// `stats()` call rebuilds. Pre-FIX-1 every `stats()` call
    /// decrypted every live memory to collect agents, turning an
    /// otherwise-cheap diagnostic into an O(N) decrypt.
    agent_set: Option<std::collections::BTreeSet<String>>,
    /// Phase 12.1 lazy caches, built together on the first keyed op this
    /// handle sees (`remember_keyed`, `update` that touches `_mnemo.key`,
    /// or any `history(...)` call). Both are maintained incrementally
    /// after that. `None` → not built yet; `Some(…)` → authoritative.
    ///
    /// * `key_index`: `(scope_owner, key) → ULID bytes` of the **live**
    ///   record for each keyed slot. `scope_owner` is `Some(agent_id)`
    ///   for a `Private` scope slot and `None` for a `Shared` one, so a
    ///   private write never collides with a shared one.
    /// * `superseded`: the ULID bytes of every record that has been
    ///   superseded (`_mnemo.superseded_by` set on it). `recall` and
    ///   `about` skip these before any decrypt.
    ///
    /// Building both costs one scan over live records (~1s at 100K with
    /// hardware AES) and happens at most once per handle. A format bump
    /// to v9 moves the key into the catalog row so the scan goes away.
    key_index: Option<std::collections::HashMap<KeySlot, u128>>,
    superseded: Option<std::collections::HashSet<u128>>,
}

/// Composite key for the lazy `key_index` on `Mnemo`. A slot is identified
/// by `(scope_owner, key)`:
/// * `Scope::Private` writes carry the owning agent → `Some(agent_id)`.
/// * `Scope::Shared` writes are per-file → `None`.
///
/// A private write never supersedes a shared one, and the reverse is also
/// true. Scope lives on `Memory.scope`, agent on `Memory.agent_id`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct KeySlot {
    /// `Some(agent_id)` for `Scope::Private`, `None` for `Scope::Shared`.
    pub(crate) scope_owner: Option<String>,
    /// The caller-supplied key string, exactly as given (no case-fold).
    pub(crate) key: String,
}

/// Open a file and take an OS advisory lock, returning `Locked { path }`
/// if the requested lock cannot be acquired. On drop of the returned
/// [`File`], the OS releases the lock. `flock`/`LockFileEx` semantics
/// (per file-descriptor on Unix, per-open-file-handle on Windows) mean
/// the lock is scoped to this handle: cloning the file (or the returned
/// `File`) shares the lock, but a fresh `open()` from any process gets a
/// distinct handle and will collide.
fn open_with_lock(path: &Path, write: bool, create: bool) -> Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    if write {
        opts.write(true);
    }
    if create {
        opts.create(true).truncate(true);
    }
    let file = opts.open(path)?;
    // On success the returned `Result<()>` is `Ok(())`. On conflict:
    //   * Unix `flock(2)` → `ErrorKind::WouldBlock` (EWOULDBLOCK / EAGAIN)
    //   * Windows `LockFileEx` → raw OS code 33 (ERROR_LOCK_VIOLATION)
    //     or 32 (ERROR_SHARING_VIOLATION), both surfaced by Rust as
    //     `ErrorKind::Uncategorized` because `std::io::ErrorKind`
    //     doesn't carry a portable "locked" variant.
    // Anything else is a real I/O failure (permissions, invalid handle,
    // ...) and gets bubbled up as `MnemoError::Io`.
    let result = if write {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    };
    match result {
        Ok(()) => Ok(file),
        Err(e) if is_lock_conflict(&e) => Err(MnemoError::Locked {
            path: path.to_path_buf(),
        }),
        Err(e) => Err(MnemoError::Io(e)),
    }
}

/// Do two paths resolve to the same on-disk file? Used by
/// [`Mnemo::export_encrypted`] to refuse overwriting the live
/// database. On Unix we compare `(dev, ino)` via `MetadataExt`, which
/// catches hard links and symlinks. On Windows we compare canonicalized
/// paths, since there is no stable device+inode equivalent in `std`.
/// Returns `None` if either side can't be stat'd / canonicalized —
/// the caller treats that as "not the same file" so a missing `dest`
/// (the normal export case) proceeds.
fn paths_refer_to_same_file(a: &Path, b: &Path) -> Option<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let ma = std::fs::metadata(a).ok()?;
        let mb = std::fs::metadata(b).ok()?;
        Some(ma.dev() == mb.dev() && ma.ino() == mb.ino())
    }
    #[cfg(not(unix))]
    {
        let ca = std::fs::canonicalize(a).ok()?;
        let cb = std::fs::canonicalize(b).ok()?;
        Some(ca == cb)
    }
}

/// A sibling temp-file path next to `dest`, suffixed with
/// `.export-tmp`. Lives in `dest`'s own parent directory so the final
/// [`std::fs::rename`] is same-filesystem (and therefore atomic on
/// every platform mnemo supports).
fn temp_sibling(dest: &Path) -> PathBuf {
    let mut buf = dest.as_os_str().to_os_string();
    buf.push(".export-tmp");
    PathBuf::from(buf)
}

/// Was this I/O error the OS reporting a conflicting file lock, or a
/// genuine I/O failure? Platform-conditional because
/// [`std::io::ErrorKind`] carries no portable "resource-locked" variant
/// as of Rust 1.75 — Windows surfaces `ERROR_LOCK_VIOLATION` /
/// `ERROR_SHARING_VIOLATION` as `Uncategorized`.
fn is_lock_conflict(e: &std::io::Error) -> bool {
    if e.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if let Some(code) = e.raw_os_error() {
        // 33 = ERROR_LOCK_VIOLATION, 32 = ERROR_SHARING_VIOLATION —
        // both mean "someone else holds an incompatible lock on this
        // region of the file". See
        // https://learn.microsoft.com/en-us/windows/win32/debug/system-error-codes--0-499-
        if code == 33 || code == 32 {
            return true;
        }
    }
    false
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

/// Load a v5+ catalog run (an empty `pages` yields an empty catalog).
fn load_catalog(pager: &mut Pager, start: u64, pages: u64, len: u64) -> Result<Vec<CatalogEntry>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load a **v4** catalog run using the frozen [`CatalogEntryV4`] shape.
/// Used only by the v4→v5 migration path in [`Mnemo::open`].
fn load_catalog_v4(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
) -> Result<Vec<CatalogEntryV4>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Walk a v4 catalog and synthesize the v5 shape. For each live entry,
/// reads the memory's serialized body and copies its `accessed_at` and
/// `access_count` into the catalog row (the v4 record body still carries
/// them — they're moving *from* the body *to* the catalog). Deleted
/// entries skip the read and zero the fields.
fn migrate_v4_catalog(pager: &mut Pager, v4: Vec<CatalogEntryV4>) -> Result<Vec<CatalogEntry>> {
    let mut out = Vec::with_capacity(v4.len());
    for e in &v4 {
        let (accessed_at, access_count) = if e.deleted {
            (0i64, 0u32)
        } else {
            let mut buf = Vec::with_capacity(e.len as usize);
            for i in 0..e.page_count as u64 {
                buf.extend_from_slice(&pager.read_page(e.start_page + i)?);
            }
            buf.truncate(e.len as usize);
            let mem: Memory = rmp_serde::from_slice(&buf)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
            (mem.accessed_at, mem.access_count)
        };
        out.push(CatalogEntry {
            id: e.id,
            start_page: e.start_page,
            page_count: e.page_count,
            len: e.len,
            deleted: e.deleted,
            accessed_at,
            access_count,
        });
    }
    Ok(out)
}

/// Load the snapshot manifest (an empty `pages` yields an empty manifest).
fn load_manifest(pager: &mut Pager, start: u64, pages: u64, len: u64) -> Result<Vec<Snapshot>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load the result-cache directory (v8+). An empty `pages` yields an
/// empty directory, which is the natural state for v7 files migrated
/// to v8 (no cache entries yet) and for freshly created v8 files.
fn load_cache_directory(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
) -> Result<Vec<CacheDirectoryEntry>> {
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

// ---------------------------------------------------------------------------
// Phase 12.7a helpers used by `ingest_thread`.
// ---------------------------------------------------------------------------

/// Hex-encode a byte slice. Low-dependency alternative to pulling in a
/// hex crate for the lone call site in `ingest_thread`'s SHA-256
/// recording.
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

/// Return the longest UTF-8 prefix of `s` whose byte length is `<=
/// max_bytes`. Always lands on a `char` boundary.
fn take_utf8_prefix(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // Walk forward through char indices; the last index whose byte
    // offset fits inside `max_bytes` tells us where to cut.
    let mut cut = 0usize;
    for (idx, _) in s.char_indices() {
        if idx > max_bytes {
            break;
        }
        cut = idx;
    }
    s[..cut].to_string()
}

/// Split `s` into chunks, each with `<= max_bytes` of UTF-8 content,
/// landing on `char` boundaries. Preserves original ordering; a trailing
/// chunk may be shorter than `max_bytes`.
/// Phase 12.7b helper: pull the `_mnemo.status` string from an
/// essence record. Returns `None` for timeline entries (they have no
/// status) and for malformed records.
fn status_of(m: &Memory) -> Option<&str> {
    m.metadata.get(MNEMO_META_STATUS).and_then(|v| v.as_str())
}

/// Phase 12.7b helper: render one essence item as a markdown bullet.
/// Timeline entries get a date tag; section items get `[open]` /
/// `[resolved]` so the reader can scan active work quickly.
fn render_essence_item(section: EssenceSection, m: &Memory) -> String {
    if section == EssenceSection::Timeline {
        let date = format_unix_date(m.created_at);
        format!("- [{date}] {}\n", m.content)
    } else {
        let status = status_of(m).unwrap_or(essence_status::OPEN);
        format!("- [{status}] {}\n", m.content)
    }
}

/// Minimal unix-seconds → `YYYY-MM-DD` formatter (no chrono/time
/// dependency; mirrors the ISO 8601 parser in `transcript.rs`).
fn format_unix_date(secs: i64) -> String {
    let days_total = secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days_total);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's `civil_from_days` — inverse of `days_from_civil`,
/// adequate for Gregorian dates since 1970. Pure arithmetic, no
/// dependencies.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn chunk_utf8(s: &str, max_bytes: usize) -> Vec<String> {
    if s.len() <= max_bytes {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut rest = s;
    while rest.len() > max_bytes {
        let head = take_utf8_prefix(rest, max_bytes);
        let head_bytes = head.len();
        out.push(head);
        rest = &rest[head_bytes..];
    }
    if !rest.is_empty() {
        out.push(rest.to_string());
    }
    out
}

impl Mnemo {
    /// Create a brand-new encrypted database at `path`.
    pub fn create(
        path: impl Into<PathBuf>,
        passphrase: &str,
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        if config.dimensions == 0 {
            return Err(MnemoError::Invalid("dimensions must be > 0".into()));
        }
        // Exclusive lock covers the write path; second writer sees `Locked`.
        let file = open_with_lock(&path, /*write=*/ true, /*create=*/ true)?;

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

        let mut header = Header {
            version: VERSION,
            page_size: PAGE_SIZE as u32,
            flags: FLAG_ENCRYPTED,
            dimensions: config.dimensions as u32,
            created_at: memory::now_secs(),
            write_counter: 0,
            // Page 0 is the header; pages 1..=wal_pages are the WAL;
            // record/catalog/index pages start after it. Initial WAL size is
            // chosen per-config (default 8 pages = 64 KiB) and clamped to a
            // sane floor so the first transaction can always commit.
            next_page: 1 + config.wal_pages_initial.max(MIN_WAL_PAGES),
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
            wal_pages: config.wal_pages_initial.max(MIN_WAL_PAGES),
            wal_seq: 0,
            manifest_start: 0,
            manifest_pages: 0,
            manifest_len: 0,
            // Regenerated per write by `apply_seal`; `None` until we seal.
            seal_nonce: None,
            seal_tag: None,
            // v8+ result-cache pointer — zero-initialized on create,
            // populated only when `cache_put` runs at least once and
            // the next `flush` seals a directory run.
            cache_start: 0,
            cache_pages: 0,
            cache_len: 0,
        };

        let mut pager = Pager::new(file, dek, 0, VERSION);
        let mut page = header.to_page();
        header.apply_seal(&mut page, pager.dek())?;
        pager.write_raw(0, &page)?;
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
            max_snapshots: config.max_snapshots,
            read_only: false,
            cache_dir: Vec::new(),
            cache_idx: DirectoryIndex::new(),
            dirty_cache: false,
            cache_budgets: std::collections::HashMap::new(),
            cache_flush_policy: CacheFlushPolicy::default(),
            cache_batch: BatchState::default(),
            cache_counters: CounterState::default(),
            embedder: None,
            fsyncs_last_flush: 0,
            agent_set: None,
            key_index: None,
            superseded: None,
        })
    }

    /// Open an existing database. A wrong passphrase fails cleanly with
    /// [`MnemoError::WrongPassphrase`]. Takes an OS exclusive lock for
    /// the lifetime of the returned handle; a second concurrent open
    /// (from any process) returns [`MnemoError::Locked`].
    ///
    /// For a caller that only needs to read (and can tolerate the
    /// pending-recovery / pending-migration cases documented on
    /// [`Mnemo::open_read_only`]), prefer that entry point — it takes a
    /// shared lock and coexists with other readers.
    pub fn open(path: impl Into<PathBuf>, passphrase: &str) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        let mut file = open_with_lock(&path, /*write=*/ true, /*create=*/ false)?;

        let mut hbuf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        let mut header = match Header::from_page(&hbuf) {
            Ok(h) => h,
            Err(e) => {
                // DUR-1 (R-002.1 part 2+3): page 0 did not parse — any
                // error is treated as a torn-header candidate, not just
                // `HeaderChecksum`. The whole 294-byte header lives in
                // the first 512-byte sector, so the realistic failure is
                // the sector zeroed or garbled, which returns `BadMagic`
                // or `UnsupportedVersion` *before* the CRC gate.
                //
                // Scan the file for valid committed header-frame WAL
                // regions, pick the highest `txn_id`, and replay it. If
                // nothing valid is found, return the *original* error so
                // non-mnemo files still report `BadMagic`.
                let scan = scan_header_recovery_candidates(&mut file, passphrase)?;
                let frames = scan.ok_or(e)?;
                for (page_no, bytes) in &frames {
                    if bytes.len() != PAGE_SIZE {
                        return Err(MnemoError::Invalid("WAL frame is not page-sized".into()));
                    }
                    file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                    file.write_all(bytes)?;
                }
                file.sync_all()?;
                crate::bump_fsync();
                file.seek(SeekFrom::Start(0))?;
                file.read_exact(&mut hbuf)?;
                Header::from_page(&hbuf)?
            }
        };

        // Crash recovery: replay a committed-but-uncheckpointed transaction.
        // Each frame is a finished page image; the header frame, replayed to
        // page 0, supersedes the header just read.
        if let Some(frames) = wal::recover(
            &mut file,
            header.wal_start,
            header.wal_pages,
            header.wal_seq,
        )? {
            for (page_no, bytes) in &frames {
                if bytes.len() != PAGE_SIZE {
                    return Err(MnemoError::Invalid("WAL frame is not page-sized".into()));
                }
                file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                file.write_all(bytes)?;
            }
            file.sync_all()?;
            crate::bump_fsync();
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

        // Validate the v7 header seal now that we have the DEK. Returns
        // `MnemoError::HeaderTampered` if any of the sealed mutable fields
        // were rewritten after the last legitimate flush. No-op for v6 and
        // older — pre-v7 files don't carry a seal.
        //
        // DUR-1 (Planner decision 2026-11-01, Option B): a seal failure is
        // **always** reported as tampering, never auto-healed. R-002.1
        // part 3's seal-mismatch WAL fallback is withdrawn: the seal
        // exists for tamper-evidence; silently loading a stale committed
        // header from the WAL would erase that evidence. A real crash
        // cannot produce "CRC valid + seal invalid" either, because the
        // whole 294-byte header lies inside sector 0 which storage
        // writes atomically — a torn sector breaks magic, version or
        // CRC first, and *those* are healed by the scan above.
        header.validate_seal(&dek)?;

        // Build the pager at the file's *current* version so initial reads
        // use the matching AAD scheme — `Pager::page_aad` returns empty AAD
        // for v4/v5 (no page binding) and `page_no.to_le_bytes()` for v6+.
        let on_disk_version = header.version;
        let mut pager = Pager::new(file, dek, header.write_counter, on_disk_version);
        let dimensions = header.dimensions as usize;

        // === v4→v5 catalog migration =====================================
        // v6 reads directly; v5 reads directly; v4 replays the catalog into
        // the v5 shape by reading each memory's body to populate the new
        // `accessed_at` / `access_count` fields. All page reads here still
        // use the v4/v5 AAD scheme (none).
        let (catalog, migrated_from_v4) = if on_disk_version >= 5 {
            (
                load_catalog(
                    &mut pager,
                    header.catalog_start,
                    header.catalog_pages,
                    header.catalog_len,
                )?,
                false,
            )
        } else if on_disk_version == 4 {
            let v4 = load_catalog_v4(
                &mut pager,
                header.catalog_start,
                header.catalog_pages,
                header.catalog_len,
            )?;
            (migrate_v4_catalog(&mut pager, v4)?, true)
        } else {
            return Err(MnemoError::UnsupportedVersion(on_disk_version));
        };
        let index = build_id_index(&catalog);
        let ann = load_index(
            &mut pager,
            header.index_start,
            header.index_pages,
            header.index_len,
            dimensions,
        )?;
        let mut manifest = load_manifest(
            &mut pager,
            header.manifest_start,
            header.manifest_pages,
            header.manifest_len,
        )?;

        // === v5→v6 page-encryption migration =============================
        // Pre-v6 data pages were encrypted under no AAD; v6 binds the home
        // `page_no` as AAD so a page-transplant attack fails authentication
        // on read. Re-stage every live record page through `write_page` so
        // the next flush re-encrypts it under v6 AAD at the same home slot.
        // Tombstoned entries skip — they're reclaimed by `compact_file`.
        // Old catalog / ANN / manifest pages stay as orphan ciphertext;
        // the next flush writes the new runs at fresh slots.
        let migrated_pages_to_v6 = on_disk_version < 6;
        if migrated_pages_to_v6 {
            for e in &catalog {
                if e.deleted {
                    continue;
                }
                for i in 0..e.page_count as u64 {
                    let payload = pager.read_page(e.start_page + i)?;
                    pager.write_page(e.start_page + i, &payload)?;
                }
            }
            pager.set_version(6);
        }

        // === v6→v7 header-seal migration =================================
        // v7 adds an AES-GCM seal at the tail of the header page that
        // authenticates every mutable header field. Pre-v7 files have no
        // seal — we just bump the version in memory and mark `dirty_catalog`
        // so the next flush writes a sealed v7 header. Pages are untouched
        // (page crypto is unchanged from v6).
        let migrated_header_to_v7 = on_disk_version < 7;
        if migrated_header_to_v7 {
            pager.set_version(7);
        }

        // === v7→v8 cache-directory migration =============================
        // v8 adds three u64 header fields (cache_start/pages/len) for the
        // result-cache directory (Phase 10.1). Pre-v8 files have those
        // three bytes as zero (the on-disk region beyond the v7 seal is
        // zero-initialized), which is the legal "empty cache directory"
        // state — no data migration required. We bump the version in
        // memory and mark dirty_catalog so the next flush writes a
        // v8-sealed header with the new AAD shape covering the cache
        // pointer. Pages are untouched.
        let migrated_header_to_v8 = on_disk_version < 8;
        if migrated_header_to_v8 {
            pager.set_version(VERSION);
        }

        let migrated = migrated_from_v4
            || migrated_pages_to_v6
            || migrated_header_to_v7
            || migrated_header_to_v8;
        if migrated {
            // Stamp the version forward in memory; the next flush persists
            // it via the WAL-committed header frame (and the v7 seal).
            header.version = VERSION;
            // Snapshots written under an older format reference page runs
            // encrypted under the old AAD scheme; this build can't decrypt
            // them under the new pager. Drop the manifest so the next
            // flush records a fresh snapshot of the migrated state.
            // Point-in-time recovery into the pre-migration past is
            // sacrificed for migration simplicity. Old pages remain on
            // disk until `compact_file`. (For v6→v7 alone — which doesn't
            // change page crypto — the old snapshots would technically
            // still be readable, but dropping them uniformly keeps the
            // migration policy simple.)
            manifest.clear();
        }

        // The ANN index pages on disk were sealed under the *old* AAD scheme
        // if we migrated pages to v6; force a rewrite at fresh v6-encrypted
        // slots on the next flush so a future reopen doesn't try to read
        // them under the new AAD and fail. (Catalog and manifest already
        // get rewritten via dirty_catalog and the manifest.clear() above.)
        let dirty_index = migrated_pages_to_v6 && ann.is_some();

        // Load the cache directory (v8+ only). Pre-v8 files have
        // cache_pages == 0 by header layout, so this trivially yields
        // an empty directory without touching the disk.
        let cache_dir = load_cache_directory(
            &mut pager,
            header.cache_start,
            header.cache_pages,
            header.cache_len,
        )?;
        let cache_idx = DirectoryIndex::build(&cache_dir);

        Ok(Mnemo {
            pager,
            header,
            catalog,
            index,
            path,
            dimensions,
            kdf,
            dirty_catalog: migrated,
            ann,
            dirty_index,
            manifest,
            // `Mnemo::open` has no [`MnemoConfig`] to read; default the cap
            // to [`DEFAULT_MAX_SNAPSHOTS`]. Override with
            // [`Mnemo::set_max_snapshots`] right after open if you need
            // unlimited retention or a different cap for this handle.
            max_snapshots: DEFAULT_MAX_SNAPSHOTS,
            read_only: false,
            cache_dir,
            cache_idx,
            dirty_cache: false,
            cache_budgets: std::collections::HashMap::new(),
            cache_flush_policy: CacheFlushPolicy::default(),
            cache_batch: BatchState::default(),
            cache_counters: CounterState::default(),
            embedder: None,
            fsyncs_last_flush: 0,
            agent_set: None,
            key_index: None,
            superseded: None,
        })
    }

    /// Open an existing database in **read-only** mode. Takes an OS shared
    /// lock; multiple read-only handles can coexist, but any attempt to
    /// open the same file read-write elsewhere returns
    /// [`MnemoError::Locked`]. All mutating methods on the returned handle
    /// return [`MnemoError::ReadOnly`], including `recall` with
    /// `track_access(true)`.
    ///
    /// Two conditions block a read-only open and return
    /// [`MnemoError::NeedsWriteOpen`]:
    ///
    /// 1. The file has a committed-but-uncheckpointed WAL transaction that
    ///    would ordinarily be replayed on open. A read-only handle
    ///    cannot write the recovered pages back.
    /// 2. The on-disk format version is older than the current
    ///    `format::VERSION`, requiring an on-disk migration.
    ///
    /// In either case, open the file read-write once (which recovers /
    /// migrates in place), close the handle cleanly, and reopen
    /// read-only. This is documented in the README's Durability section.
    pub fn open_read_only(path: impl Into<PathBuf>, passphrase: &str) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        let mut file = open_with_lock(&path, /*write=*/ false, /*create=*/ false)?;

        let mut hbuf = [0u8; PAGE_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hbuf)?;
        // A torn header on a read-only open cannot be healed (that requires
        // writing recovered frames back). Surface as `NeedsWriteOpen` so the
        // caller knows to reopen read-write once. DUR-1 (R-002.1 part 4):
        // any `Header::from_page` failure (`BadMagic`,
        // `UnsupportedVersion`, `HeaderChecksum`) qualifies — but only
        // when the file-wide scan actually finds a valid committed
        // header frame to recover from, so a non-mnemo file still
        // reports `BadMagic` cleanly. A `validate_seal` failure is NOT
        // routed here (Option B: tampering is never auto-healed).
        let header = match Header::from_page(&hbuf) {
            Ok(h) => h,
            Err(parse_err) => {
                let recoverable = scan_header_recovery_candidates(&mut file, passphrase)?;
                return Err(match (parse_err, recoverable) {
                    (_, Some(_)) => MnemoError::NeedsWriteOpen {
                        reason: "torn header page needs recovery from WAL",
                    },
                    (e, None) => e,
                });
            }
        };

        // Refuse to migrate on read-only opens. v6→v7 is header-only
        // (no page writes required), but even that bumps `version` on
        // disk and re-seals the header — which needs write access. Keep
        // the policy uniform across all sub-version bumps.
        if header.version < VERSION {
            return Err(MnemoError::NeedsWriteOpen {
                reason: "pending format-version migration",
            });
        }

        // Refuse if there is a committed-but-uncheckpointed transaction
        // waiting for `wal::recover` to fold it into home pages.
        if wal::recover(
            &mut file,
            header.wal_start,
            header.wal_pages,
            header.wal_seq,
        )?
        .is_some()
        {
            return Err(MnemoError::NeedsWriteOpen {
                reason: "pending WAL recovery",
            });
        }

        let kdf = KdfParams {
            m_cost: header.m_cost,
            t_cost: header.t_cost,
            p_cost: header.p_cost,
        };
        let kek = crypto::derive_kek(passphrase.as_bytes(), &header.salt, kdf)?;
        let dek = crypto::unwrap_dek(&kek, &header.dek_nonce, &header.wrapped_dek)?;
        header.validate_seal(&dek)?;

        let mut pager = Pager::new(file, dek, header.write_counter, header.version);
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
        let cache_dir = load_cache_directory(
            &mut pager,
            header.cache_start,
            header.cache_pages,
            header.cache_len,
        )?;
        let cache_idx = DirectoryIndex::build(&cache_dir);

        Ok(Mnemo {
            pager,
            header,
            catalog,
            index,
            path,
            dimensions,
            kdf,
            // No mutation is possible; leave both dirty flags false so
            // `close()` is a genuine no-op even if we later add an
            // auto-flush on drop.
            dirty_catalog: false,
            ann,
            dirty_index: false,
            manifest,
            max_snapshots: DEFAULT_MAX_SNAPSHOTS,
            read_only: true,
            cache_dir,
            cache_idx,
            dirty_cache: false,
            cache_budgets: std::collections::HashMap::new(),
            cache_flush_policy: CacheFlushPolicy::default(),
            cache_batch: BatchState::default(),
            cache_counters: CounterState::default(),
            embedder: None,
            fsyncs_last_flush: 0,
            agent_set: None,
            key_index: None,
            superseded: None,
        })
    }

    /// Override the manifest snapshot cap on this open handle. `0` disables
    /// the cap; any positive value keeps the most-recent `max` snapshots and
    /// drops the rest on the next flush. See [`MnemoConfig::max_snapshots`].
    pub fn set_max_snapshots(&mut self, max: usize) {
        self.max_snapshots = max;
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
        let mut m: Memory =
            rmp_serde::from_slice(&bytes).map_err(|err| MnemoError::Serialize(err.to_string()))?;
        // The catalog is the source of truth for access stats — the values
        // in the record body are a stale snapshot from when the record was
        // last written. Overwrite them here so consumers always see live
        // values regardless of when the record was last serialized.
        m.accessed_at = e.accessed_at;
        m.access_count = e.access_count;
        Ok(m)
    }

    /// Serialize and store a memory (insert or overwrite by ID).
    fn put(&mut self, mut m: Memory) -> Result<Ulid> {
        if m.vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: m.vector.len(),
            });
        }
        // Reject all-zero vectors (Phase 3 PR 10a; breaking vs 0.4.x).
        // A zero vector scores exactly 0.0 on every cosine query — it
        // can't contribute signal, but recency / importance can still
        // pull it into top-k as noise. We'd rather fail loudly at
        // `remember` than let a mis-embedded entry pollute recall
        // quality for the life of the database. Scaffold manifests now
        // use a sentinel unit vector so this check doesn't trip them.
        if embed::is_zero_vector(&m.vector) {
            return Err(MnemoError::Invalid(
                "all-zero vector refused: it scores 0.0 on every cosine query and \
                 pollutes recall; check your embedder returned a real embedding"
                    .into(),
            ));
        }
        if m.id == Ulid::nil() {
            m.id = Ulid::new();
        }
        let id_u: u128 = m.id.0;
        let bytes = rmp_serde::to_vec(&m).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (start, pc) = self.write_record(&bytes)?;
        // For overwrites preserve the existing access stats; for inserts
        // seed them from the memory (which carries them in its body).
        let (prev_accessed, prev_count) = match self.index.get(&id_u).copied() {
            Some(idx) => (
                self.catalog[idx].accessed_at,
                self.catalog[idx].access_count,
            ),
            None => (m.accessed_at, m.access_count),
        };
        let entry = CatalogEntry {
            id: id_u,
            start_page: start,
            page_count: pc,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: prev_accessed,
            access_count: prev_count,
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

        // FIX-1 R-001.8 part 2: keep the lazy agent set in sync. If
        // it hasn't been built yet (no `stats()` call this session),
        // skip — the first `stats()` will build it from scratch. If
        // it has, add this memory's agent_id. An overwrite with the
        // same id and same agent is a no-op (BTreeSet::insert returns
        // false); an overwrite with a *changed* agent_id adds the new
        // agent but doesn't drop the old (we can't tell without a
        // decrypt whether the old agent still has live memories).
        // That's conservative in the right direction — `stats()` may
        // over-report by one agent until the next delete invalidates.
        if let Some(set) = &mut self.agent_set {
            set.insert(m.agent_id.clone());
        }
        Ok(m.id)
    }

    // --- public CRUD ------------------------------------------------------

    /// Register a text embedder for this handle. Enables
    /// [`Mnemo::remember_text`] and the text-side `RecallRequest`
    /// convenience path. Does two checks before storing the embedder:
    ///
    /// 1. **Dimension check.** The embedder's `identity().dimensions`
    ///    must equal [`Mnemo::dimensions`] (the width this database
    ///    was created with). Mismatch returns
    ///    [`MnemoError::DimensionMismatch`].
    /// 2. **Manifest identity check.** If the scaffold / onboarding
    ///    manifest already records an embedder — the canonical shape
    ///    is `metadata.embedder = { name, dimensions, normalize }`,
    ///    with the legacy flat `embedder_name` / `embedder_dimensions`
    ///    / `embedder_normalize` keys accepted as an alias — the new
    ///    embedder's identity must match exactly. A different name —
    ///    even at the same dimensions — returns
    ///    [`MnemoError::Invalid`] with
    ///    [`EmbedderIdentity::MISMATCH_MSG`] in the message. If the
    ///    manifest has no embedder metadata yet, this call upserts
    ///    the nested object into the first onboarding/manifest memory
    ///    (write + flush so a subsequent reopen sees the identity).
    ///
    /// **Read-only handles.** `set_embedder` on an `open_read_only`
    /// handle performs both checks and adopts the embedder in memory;
    /// it never writes. Readers (CLI `--text`, agent-side `recall_text`)
    /// can therefore use the text path next to a writer that holds
    /// the exclusive lock.
    ///
    /// On both failure paths the handle's embedder slot is left
    /// unchanged. On success the embedder is wrapped in an `Arc<dyn
    /// Embedder>` and consulted by every text-side call made through
    /// this handle. The slot is per-handle and not persisted — a
    /// fresh open must re-register (same identity, so the manifest
    /// check round-trips cleanly).
    pub fn set_embedder<E: Embedder + 'static>(&mut self, embedder: E) -> Result<()> {
        let identity = embedder.identity();
        if identity.dimensions != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: identity.dimensions,
            });
        }

        // Find a scaffold/onboarding manifest to inspect (or upsert
        // into). `about()` already filters + sorts the onboarding set.
        let onboarding = self.about()?;
        let mut upsert_target: Option<usize> = None;
        for (i, m) in onboarding.iter().enumerate() {
            if let Some(saw) = embed::read_identity_from_metadata(&m.metadata) {
                if saw != identity {
                    return Err(embed::identity_mismatch_err(&saw, &identity));
                }
                // Match — nothing to persist; adopt + done. Works the
                // same on read-only and read-write handles.
                self.embedder = Some(std::sync::Arc::new(embedder));
                return Ok(());
            }
            if upsert_target.is_none() {
                upsert_target = Some(i);
            }
        }

        // No existing identity on the manifest. On a read-write handle
        // upsert into the first onboarding memory so later opens see
        // it; on a read-only handle skip the write (readers must not
        // mutate the file), but still adopt the embedder in memory.
        if !self.read_only {
            if let Some(idx) = upsert_target {
                let mut m = onboarding[idx].clone();
                embed::write_identity_into_metadata(&mut m.metadata, &identity);
                // Reuse the memory's existing id so this is an in-place
                // update of the scaffold manifest, not a new entry.
                self.put(m)?;
                self.flush()?;
            }
        }

        self.embedder = Some(std::sync::Arc::new(embedder));
        Ok(())
    }

    /// Borrow the currently-registered embedder, if any. Returns
    /// `None` when no embedder has been set via
    /// [`Mnemo::set_embedder`]. Bindings use this to drive their own
    /// text-side convenience methods without duplicating the identity
    /// check.
    pub fn embedder(&self) -> Option<&dyn Embedder> {
        self.embedder.as_deref()
    }

    /// Shared handle to the registered embedder, cloned so the caller
    /// can drop the borrow on this `Mnemo` before invoking `embed()`.
    /// FIX-1 / PY-1 (R-001.11): the Python binding needs this so an
    /// embedder callback that re-enters `Mnemo` (e.g. a Python
    /// embedder that calls `db.dimensions()` or `db.cache_get(...)`)
    /// doesn't hit PyO3's "Already borrowed" — we clone the Arc under
    /// a short `&self` borrow, release that borrow, then invoke
    /// `embed()` with no `&mut Mnemo` held.
    pub fn embedder_arc(&self) -> Option<std::sync::Arc<dyn Embedder>> {
        self.embedder.clone()
    }

    /// Embed `content` with the registered embedder and `remember` the
    /// result. Returns the new memory's ULID. Convenience path for
    /// callers who have a string, want it stored, and don't need any
    /// of the per-memory options [`Memory`] exposes (agent, importance,
    /// metadata, ttl, scope) — build a [`Memory`] by hand and call
    /// [`Mnemo::remember`] for those.
    ///
    /// Fails with [`MnemoError::Invalid`] if no embedder has been
    /// registered on this handle (call [`Mnemo::set_embedder`] first);
    /// with [`MnemoError::DimensionMismatch`] if the embedder returned
    /// the wrong width for this database.
    pub fn remember_text(&mut self, content: &str, memory_type: MemoryType) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[content])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let memory = Memory::new(content.to_string(), memory_type, vector);
        self.remember(memory)
    }

    /// Store a memory. Returns its ULID. If the memory's ID is nil a fresh
    /// one is assigned; an existing ID overwrites in place.
    pub fn remember(&mut self, memory: Memory) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
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
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
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
        // FIX-1 R-001.8 part 2: we can't tell whether this delete
        // removed the last memory of its agent without a decrypt
        // (the catalog entry doesn't carry `agent_id` in format v8),
        // so just invalidate the lazy set. The next `stats()` call
        // rebuilds from live memories.
        self.agent_set = None;
        // Phase 12.1: delete of a keyed record removes it from the slot,
        // and delete of a superseded record frees a `history` entry. In
        // both cases the exact (key, scope_owner) needs a decrypt we're
        // not willing to pay here, so drop the lazy caches and rebuild
        // on next access. Format v9 moves the key into the catalog row
        // and this becomes O(1).
        self.key_index = None;
        self.superseded = None;
        Ok(())
    }

    // --- Phase 12.1: edit in place + keyed supersession -----------------

    /// Scope-aware key-slot identifier for `memory` and a caller key.
    ///
    /// * [`Scope::Private`] → owned by the memory's `agent_id`.
    /// * [`Scope::Shared`]  → one slot across the whole file.
    fn key_slot_for(memory: &Memory, key: &str) -> crate::store::KeySlot {
        let scope_owner = match memory.scope {
            Scope::Private => Some(memory.agent_id.clone()),
            Scope::Shared => None,
        };
        crate::store::KeySlot {
            scope_owner,
            key: key.to_string(),
        }
    }

    /// Build the `key_index` + `superseded` lazy caches from live
    /// records. Called by every 12.1 entrypoint on first use. Decrypts
    /// every live record once, so a fresh open pays one scan the first
    /// time a keyed op touches it; subsequent ops are O(1).
    fn ensure_key_caches(&mut self) -> Result<()> {
        if self.key_index.is_some() && self.superseded.is_some() {
            return Ok(());
        }
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut keys: std::collections::HashMap<crate::store::KeySlot, u128> =
            std::collections::HashMap::new();
        let mut superseded: std::collections::HashSet<u128> = std::collections::HashSet::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_SUPERSEDED_BY).is_some() {
                superseded.insert(e.id);
                continue;
            }
            if let Some(Value::String(k)) = m.metadata.get(MNEMO_META_KEY) {
                let slot = Self::key_slot_for(&m, k);
                // Catalog order is insertion order; a later live record
                // with the same slot would only exist if a prior build
                // of this code allowed it. Keep the later id (newer
                // ULID), in case of a tie.
                keys.entry(slot)
                    .and_modify(|cur| {
                        if *cur < e.id {
                            *cur = e.id;
                        }
                    })
                    .or_insert(e.id);
            }
        }
        self.key_index = Some(keys);
        self.superseded = Some(superseded);
        Ok(())
    }

    /// Stage-identical check for a 12.1 keyed no-op. The incoming write
    /// is byte-equal to the live record iff every user-visible field
    /// (content, type, importance, vector, user metadata) matches. The
    /// `_mnemo.*` reserved keys on the live record are **ignored** for
    /// this comparison — a repeat of an identical write re-issued by an
    /// extractor must be a NOOP even if the live record already carries
    /// supersession bookkeeping from a prior op.
    fn is_identical_keyed_write(incoming: &Memory, live: &Memory) -> bool {
        if incoming.content != live.content {
            return false;
        }
        if incoming.memory_type != live.memory_type {
            return false;
        }
        if (incoming.importance - live.importance).abs() > f32::EPSILON {
            return false;
        }
        if incoming.vector != live.vector {
            return false;
        }
        // Strip `_mnemo.*` from both sides before comparing user metadata.
        let strip = |m: &Map<String, Value>| -> Map<String, Value> {
            m.iter()
                .filter(|(k, _)| !k.starts_with(crate::memory::MNEMO_META_NS))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        strip(&incoming.metadata) == strip(&live.metadata)
    }

    /// Store a memory under a keyed upsert slot. Returns the live record's
    /// id and a [`WriteOutcome`] saying whether this write created a new
    /// slot (`Added`), replaced a prior record (`Superseded`), or matched
    /// the existing live record byte-for-byte (`Unchanged`, no file write).
    ///
    /// The slot is identified by `(scope_owner, key)`:
    /// * `Scope::Private` memories share a slot per `(agent_id, key)`.
    /// * `Scope::Shared` memories share one slot per `key` across the file.
    ///
    /// On supersession the prior record is **not** deleted — it remains
    /// retrievable through [`Mnemo::history`] — but it is excluded from
    /// default [`Mnemo::recall`] and [`Mnemo::about`]. A caller that wants
    /// the full lineage of a key walks `history(key)`.
    ///
    /// This is the atomic 12.1 "supersede-or-noop" write path. Plain
    /// [`Mnemo::remember`] without a key is unchanged.
    pub fn remember_keyed(
        &mut self,
        mut memory: Memory,
        key: &str,
    ) -> Result<(Ulid, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if key.is_empty() {
            return Err(MnemoError::Invalid(
                "remember_keyed: 'key' must not be empty".into(),
            ));
        }
        self.ensure_key_caches()?;
        let slot = Self::key_slot_for(&memory, key);
        let existing = self.key_index.as_ref().and_then(|k| k.get(&slot).copied());

        // Caller may already have set `_mnemo.key` on the memory; rewrite
        // it to our canonical value regardless so the record is
        // self-identifying.
        memory
            .metadata
            .insert(MNEMO_META_KEY.into(), Value::String(key.to_string()));

        // NOOP check: if a live record exists for this slot and its
        // user-visible fields match, return the existing id without a
        // write.
        if let Some(prev_id_u) = existing {
            let prev_idx = *self
                .index
                .get(&prev_id_u)
                .ok_or_else(|| MnemoError::Invalid("key_index drift".into()))?;
            let prev_entry = self.catalog[prev_idx].clone();
            let prev_mem = self.read_memory(&prev_entry)?;
            if Self::is_identical_keyed_write(&memory, &prev_mem) {
                return Ok((Ulid(prev_id_u), WriteOutcome::Unchanged));
            }

            // SUPERSESSION: insert the new record, then rewrite the old
            // with `_mnemo.superseded_by` + `_mnemo.invalid_at`.
            let now = memory::now_secs();
            memory.id = Ulid::nil(); // force a fresh ULID in `put`
            memory.metadata.insert(
                MNEMO_META_SUPERSEDES.into(),
                Value::String(Ulid(prev_id_u).to_string()),
            );
            let new_id = self.put(memory)?;
            // Mark the prior record.
            let mut prev_mut = prev_mem;
            prev_mut.metadata.insert(
                MNEMO_META_SUPERSEDED_BY.into(),
                Value::String(new_id.to_string()),
            );
            prev_mut
                .metadata
                .insert(MNEMO_META_INVALID_AT.into(), Value::Number(now.into()));
            // keep prev_mut.id so `put` overwrites in place.
            self.put(prev_mut)?;
            // Update caches incrementally.
            if let Some(k) = self.key_index.as_mut() {
                k.insert(slot, new_id.0);
            }
            if let Some(s) = self.superseded.as_mut() {
                s.insert(prev_id_u);
            }
            return Ok((new_id, WriteOutcome::Superseded));
        }

        // ADDED: no prior record for this slot.
        memory.id = Ulid::nil();
        let new_id = self.put(memory)?;
        if let Some(k) = self.key_index.as_mut() {
            k.insert(slot, new_id.0);
        }
        Ok((new_id, WriteOutcome::Added))
    }

    /// Text-side keyed write: embed `content` through the registered
    /// embedder, then [`Mnemo::remember_keyed`]. Same returns.
    pub fn remember_keyed_text(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        key: &str,
    ) -> Result<(Ulid, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[content])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let memory = Memory::new(content.to_string(), memory_type, vector);
        self.remember_keyed(memory, key)
    }

    /// Update an existing memory in place, preserving its ULID,
    /// `created_at` and access stats. Returns [`WriteOutcome::Updated`]
    /// on a real change, [`WriteOutcome::Unchanged`] when every field in
    /// the patch already matches the live record.
    ///
    /// Reserved `_mnemo.*` metadata keys on the existing record are
    /// preserved across a `metadata` replacement; a `metadata` of
    /// `Some(map)` replaces every **user-visible** key with `map`'s
    /// entries. A `content` change requires a matching `vector`
    /// (use [`Mnemo::update_text`] to re-embed) — a bare `content` patch
    /// is rejected to stop text and embedding drifting apart silently.
    pub fn update(&mut self, id: &Ulid, patch: MemoryPatch) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if patch.content.is_some() && patch.vector.is_none() {
            return Err(MnemoError::Invalid(
                "update: a content change needs a matching vector (or call update_text \
                 to re-embed through the registered embedder)"
                    .into(),
            ));
        }
        let idx = *self
            .index
            .get(&id.0)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        if self.catalog[idx].deleted {
            return Err(MnemoError::NotFound(id.to_string()));
        }
        let entry = self.catalog[idx].clone();
        let existing = self.read_memory(&entry)?;

        // Apply patch into a candidate, detect no-op after the fact.
        let mut cand = existing.clone();
        if let Some(c) = &patch.content {
            cand.content = c.clone();
        }
        if let Some(v) = &patch.vector {
            if v.len() != self.dimensions {
                return Err(MnemoError::DimensionMismatch {
                    expected: self.dimensions,
                    got: v.len(),
                });
            }
            cand.vector = v.clone();
        }
        if let Some(i) = patch.importance {
            cand.importance = i.clamp(0.0, 1.0);
        }
        if let Some(mt) = patch.memory_type {
            cand.memory_type = mt;
        }
        if let Some(new_meta) = patch.metadata {
            // Keep reserved `_mnemo.*` keys from the existing record.
            let reserved: Vec<(String, Value)> = existing
                .metadata
                .iter()
                .filter(|(k, _)| k.starts_with(crate::memory::MNEMO_META_NS))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            cand.metadata = new_meta;
            for (k, v) in reserved {
                cand.metadata.insert(k, v);
            }
        }
        if let Some(ttl) = patch.ttl_secs {
            cand.ttl_secs = ttl;
        }

        // No-op detection: compare all user-visible fields.
        if cand.content == existing.content
            && cand.vector == existing.vector
            && (cand.importance - existing.importance).abs() <= f32::EPSILON
            && cand.memory_type == existing.memory_type
            && cand.metadata == existing.metadata
            && cand.ttl_secs == existing.ttl_secs
        {
            return Ok(WriteOutcome::Unchanged);
        }

        // Stamp `_mnemo.updated_at` and preserve id + created_at.
        let now = memory::now_secs();
        cand.metadata
            .insert(MNEMO_META_UPDATED_AT.into(), Value::Number(now.into()));
        cand.id = existing.id;
        cand.created_at = existing.created_at;
        // put() preserves accessed_at/access_count via the index lookup.
        self.put(cand)?;
        // key_index may need maintenance if this update added or removed
        // `_mnemo.key`; cheapest safe path is to invalidate.
        self.key_index = None;
        Ok(WriteOutcome::Updated)
    }

    /// In-place update that re-embeds `new_text` through the registered
    /// embedder. Shorthand for `update(id, patch.with_content_and_vector)`.
    pub fn update_text(
        &mut self,
        id: &Ulid,
        new_text: &str,
        mut patch: MemoryPatch,
    ) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[new_text])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        patch.content = Some(new_text.to_string());
        patch.vector = Some(vector);
        self.update(id, patch)
    }

    /// Walk the full chain for a keyed slot: every record this key has
    /// ever held, newest first. Entry 0 is the **live** record (unless
    /// the slot is empty, in which case the vector is empty); subsequent
    /// entries are the superseded history in reverse-supersession order.
    ///
    /// **Slot resolution (v0.8.1 bugfix, lane A).** The lookup slot
    /// matches a keyed `remember` with the same arguments, so the
    /// obvious `history(key, None)` walks the private slot of the
    /// default agent — the same slot
    /// `remember_keyed(Memory::new(...), key)` writes to when the
    /// caller omits an explicit `agent_id` / `scope`. Pre-v0.8.1
    /// `agent_id = None` incorrectly looked up the shared slot and
    /// returned an empty chain for the common case; the old behavior
    /// is still reachable via `history_in(key, KeyScope::Shared)`.
    ///
    /// * `agent_id = Some(a)` → private slot for that agent.
    /// * `agent_id = None` → private slot for the default agent (same
    ///   as [`KeyScope::private_default`]).
    ///
    /// Use [`Mnemo::history_in`] with [`KeyScope::Shared`] for the
    /// shared slot.
    pub fn history(&mut self, key: &str, agent_id: Option<&str>) -> Result<Vec<Memory>> {
        let scope = match agent_id {
            Some(a) => KeyScope::Private(a.to_string()),
            None => KeyScope::private_default(),
        };
        self.history_in(key, scope)
    }

    /// Walk the full chain for a keyed slot identified by an explicit
    /// [`KeyScope`]. Returns the same shape as [`Mnemo::history`] —
    /// live record first, then the superseded chain newest-first.
    ///
    /// Use this when you need the **shared** slot (`KeyScope::Shared`),
    /// which `history(key, None)` no longer reaches as of v0.8.1.
    pub fn history_in(&mut self, key: &str, scope: KeyScope) -> Result<Vec<Memory>> {
        self.ensure_key_caches()?;
        let slot = crate::store::KeySlot {
            scope_owner: match scope {
                KeyScope::Private(a) => Some(a),
                KeyScope::Shared => None,
            },
            key: key.to_string(),
        };
        let live_id = self.key_index.as_ref().and_then(|k| k.get(&slot).copied());
        let mut out = Vec::new();
        let mut cursor = live_id;
        while let Some(id_u) = cursor {
            let idx = *self.index.get(&id_u).ok_or_else(|| {
                MnemoError::Invalid("history: superseded chain references missing id".into())
            })?;
            let entry = self.catalog[idx].clone();
            if entry.deleted {
                break;
            }
            let m = self.read_memory(&entry)?;
            let next = m
                .metadata
                .get(MNEMO_META_SUPERSEDES)
                .and_then(|v| v.as_str())
                .and_then(|s| Ulid::from_string(s).ok())
                .map(|u| u.0);
            out.push(m);
            cursor = next;
        }
        Ok(out)
    }

    // --- Phase 12.7a: thread archive --------------------------------

    /// Ingest a batch of turns into the archive layer of `thread_id`.
    ///
    /// Each [`ThreadTurn`] becomes one or more `Episodic` records
    /// tagged with the usual thread-archive reserved-metadata set
    /// (`_mnemo.kind = "turn"`, `_mnemo.thread`, `_mnemo.turn_id`,
    /// `_mnemo.seq`, `_mnemo.role`). The ingest path is
    /// **idempotent** per `turn_id`: a turn whose `turn_id` already
    /// appears on a record in this thread is skipped, so re-running
    /// `ingest_thread` with an extended transcript appends only
    /// what's new.
    ///
    /// **Chunking.** A non-tool turn whose UTF-8 content exceeds
    /// [`THREAD_TURN_CHUNK_BYTES`] (8 KB) is split into chunks at
    /// char boundaries, each stored as its own record with
    /// `_mnemo.part = 0, 1, …` under the same `turn_id`.
    ///
    /// **Truncation.** A `role = "tool"` turn whose content exceeds
    /// [`THREAD_TOOL_RESULT_TRUNCATE_BYTES`] (16 KB) is stored
    /// truncated — the first 16 KB of UTF-8 goes into the record,
    /// and `_mnemo.sha256` + `_mnemo.length` capture the original
    /// hash and byte length so the full content can be recovered
    /// (or verified) out-of-band.
    ///
    /// **Embedding.** An embedder must be registered
    /// ([`Mnemo::set_embedder`]); the content strings (one per
    /// resulting chunk / truncated record) are batched into a single
    /// `embed()` call.
    ///
    /// Call `self.flush()` after a successful ingest to persist.
    pub fn ingest_thread(&mut self, thread_id: &str, turns: &[ThreadTurn]) -> Result<IngestReport> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if thread_id.is_empty() {
            return Err(MnemoError::Invalid(
                "ingest_thread: 'thread_id' must not be empty".into(),
            ));
        }
        let mut report = IngestReport::default();
        if turns.is_empty() {
            return Ok(report);
        }

        // Build the set of turn_ids already in this thread so the
        // idempotence check is O(1) per incoming turn. First-pass
        // cost: one decrypt per existing thread record. A future v9
        // format bump can move `_mnemo.turn_id` into the catalog row
        // and drop this scan.
        let existing_turn_ids = self.collect_turn_ids_in_thread(thread_id)?;
        // Also figure out the next `_mnemo.seq` value so a repeat
        // ingest continues the sequence instead of restarting at 0.
        let mut next_seq = self.max_seq_in_thread(thread_id)?.map_or(0, |n| n + 1);

        // Determine each incoming turn's final text pieces (post-
        // chunking / truncation) before any writes. We embed the
        // whole batch in a single `embed()` call so the engine side
        // is one round trip to the embedder regardless of turn count.
        struct Pending {
            turn_id: String,
            role: String,
            created_at: i64,
            parts: Vec<String>,
            /// Hex sha256 + original length, set only for a
            /// truncated tool-result turn. Applied to the single
            /// chunk the truncation yielded.
            truncation: Option<(String, u64)>,
        }
        let mut pending: Vec<Pending> = Vec::new();
        let mut all_parts: Vec<String> = Vec::new();
        let now = memory::now_secs();
        for t in turns {
            if t.turn_id.is_empty() {
                return Err(MnemoError::Invalid(
                    "ingest_thread: turn_id must not be empty".into(),
                ));
            }
            if existing_turn_ids.contains(&t.turn_id) {
                report.skipped_existing_turns += 1;
                continue;
            }
            let content_bytes = t.content.len();
            let is_tool = t.role == "tool";
            let (parts, truncation) =
                if is_tool && content_bytes > THREAD_TOOL_RESULT_TRUNCATE_BYTES {
                    report.truncated_turns += 1;
                    let hash = {
                        use sha2::{Digest, Sha256};
                        let h = Sha256::digest(t.content.as_bytes());
                        hex_encode(&h)
                    };
                    let truncated = take_utf8_prefix(&t.content, THREAD_TOOL_RESULT_TRUNCATE_BYTES);
                    (vec![truncated], Some((hash, content_bytes as u64)))
                } else if content_bytes > THREAD_TURN_CHUNK_BYTES {
                    report.chunked_turns += 1;
                    (chunk_utf8(&t.content, THREAD_TURN_CHUNK_BYTES), None)
                } else {
                    (vec![t.content.clone()], None)
                };
            for p in &parts {
                all_parts.push(p.clone());
            }
            pending.push(Pending {
                turn_id: t.turn_id.clone(),
                role: t.role.clone(),
                created_at: t.timestamp.unwrap_or(now),
                parts,
                truncation,
            });
        }

        if pending.is_empty() {
            return Ok(report);
        }

        // Batched embed — one round trip to the embedder for the
        // whole ingest.
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "ingest_thread: no embedder registered on this handle; call set_embedder first"
                    .into(),
            )
        })?;
        let as_refs: Vec<&str> = all_parts.iter().map(|s| s.as_str()).collect();
        let vectors = embedder.embed(&as_refs)?;
        if vectors.len() != all_parts.len() {
            return Err(MnemoError::Invalid(format!(
                "ingest_thread: embedder returned {} vectors for {} parts",
                vectors.len(),
                all_parts.len()
            )));
        }
        for v in &vectors {
            if v.len() != self.dimensions {
                return Err(MnemoError::DimensionMismatch {
                    expected: self.dimensions,
                    got: v.len(),
                });
            }
        }

        // Write records. Order: each pending turn's chunks in part
        // order; each turn gets a single `_mnemo.seq` slot, so
        // `_mnemo.part` disambiguates chunks of the same turn.
        let mut vec_cursor = 0usize;
        for p in &pending {
            let this_seq = next_seq;
            next_seq += 1;
            for (part_idx, part_text) in p.parts.iter().enumerate() {
                let vec = vectors[vec_cursor].clone();
                vec_cursor += 1;
                let mut m =
                    Memory::new(part_text.clone(), MemoryType::Episodic, vec).with_importance(0.5);
                m.created_at = p.created_at;
                m.accessed_at = p.created_at;
                m.metadata.insert(
                    MNEMO_META_KIND.into(),
                    Value::String(MNEMO_META_KIND_TURN.into()),
                );
                m.metadata.insert(
                    MNEMO_META_THREAD.into(),
                    Value::String(thread_id.to_string()),
                );
                m.metadata
                    .insert(MNEMO_META_TURN_ID.into(), Value::String(p.turn_id.clone()));
                m.metadata
                    .insert(MNEMO_META_SEQ.into(), Value::Number(this_seq.into()));
                m.metadata
                    .insert(MNEMO_META_ROLE.into(), Value::String(p.role.clone()));
                if p.parts.len() > 1 {
                    m.metadata.insert(
                        MNEMO_META_PART.into(),
                        Value::Number((part_idx as u64).into()),
                    );
                }
                if let Some((sha, len)) = &p.truncation {
                    m.metadata
                        .insert(MNEMO_META_SHA256.into(), Value::String(sha.clone()));
                    m.metadata
                        .insert(MNEMO_META_LENGTH.into(), Value::Number((*len).into()));
                }
                self.put(m)?;
                report.added_records += 1;
            }
        }
        Ok(report)
    }

    /// Phase 12.7a helper: collect every `_mnemo.turn_id` already
    /// stored in `thread_id`. One decrypt per record in the thread.
    fn collect_turn_ids_in_thread(
        &mut self,
        thread_id: &str,
    ) -> Result<std::collections::HashSet<String>> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut out = std::collections::HashSet::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(thread_id) {
                continue;
            }
            if let Some(tid) = m.metadata.get(MNEMO_META_TURN_ID).and_then(|v| v.as_str()) {
                out.insert(tid.to_string());
            }
        }
        Ok(out)
    }

    /// Phase 12.7a helper: find the largest `_mnemo.seq` in a thread,
    /// so a resumed ingest continues the sequence.
    fn max_seq_in_thread(&mut self, thread_id: &str) -> Result<Option<u64>> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut best: Option<u64> = None;
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(thread_id) {
                continue;
            }
            if let Some(n) = m.metadata.get(MNEMO_META_SEQ).and_then(|v| v.as_u64()) {
                best = Some(best.map_or(n, |cur| cur.max(n)));
            }
        }
        Ok(best)
    }

    /// Export the archive layer for `thread_id` in `_mnemo.seq`
    /// order. Each entry is the Memory as stored; chunks of a
    /// multi-part turn come out in `_mnemo.part` order within their
    /// `_mnemo.seq`. The record `.content` is the text that was
    /// stored (post-chunking / post-truncation); callers that need
    /// the pre-truncation original should round-trip via
    /// `_mnemo.sha256` + `_mnemo.length`.
    pub fn thread_export(&mut self, thread_id: &str) -> Result<Vec<Memory>> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut out: Vec<Memory> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(thread_id) {
                continue;
            }
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_TURN)
            {
                continue;
            }
            out.push(m);
        }
        out.sort_by(|a, b| {
            let seq_a = a
                .metadata
                .get(MNEMO_META_SEQ)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let seq_b = b
                .metadata
                .get(MNEMO_META_SEQ)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let part_a = a
                .metadata
                .get(MNEMO_META_PART)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let part_b = b
                .metadata
                .get(MNEMO_META_PART)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            seq_a.cmp(&seq_b).then_with(|| part_a.cmp(&part_b))
        });
        Ok(out)
    }

    /// Soft-delete every record in the archive layer of `thread_id`.
    /// Returns the number of records marked deleted. Permanent
    /// removal still requires a subsequent `compact_file`, same as
    /// any `Mnemo::delete`.
    ///
    /// Does **not** touch `_mnemo.kind = "essence"` records or other
    /// non-turn metadata the thread may carry — only `_mnemo.kind =
    /// "turn"` records matching the thread id.
    pub fn thread_forget(&mut self, thread_id: &str) -> Result<usize> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // Collect first, then delete — can't hold catalog borrows
        // across `self.delete`.
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut victims: Vec<Ulid> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(thread_id) {
                continue;
            }
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_TURN)
            {
                continue;
            }
            victims.push(m.id);
        }
        let n = victims.len();
        for id in &victims {
            self.delete(id)?;
        }
        Ok(n)
    }

    // --- Phase 12.7b: essence (timeline + sections) -----------------

    /// Phase 12.7b: add a new essence item to a section (any section
    /// except [`EssenceSection::Timeline`]). Content ≤
    /// [`memory::ESSENCE_SECTION_MAX_BYTES`] (280 bytes); items over
    /// that are rejected. Starting status is `"open"`.
    ///
    /// Use [`Mnemo::essence_append_timeline`] for timeline entries —
    /// they have a different cap (600 bytes), are append-only, and
    /// carry no status lifecycle.
    pub fn essence_add(
        &mut self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if section == EssenceSection::Timeline {
            return Err(MnemoError::Invalid(
                "essence_add: use essence_append_timeline for the timeline section".into(),
            ));
        }
        if text.is_empty() {
            return Err(MnemoError::Invalid(
                "essence_add: text must not be empty".into(),
            ));
        }
        if text.len() > section.item_max_bytes() {
            return Err(MnemoError::Invalid(format!(
                "essence_add: '{}' items must be <= {} bytes; got {}",
                section.as_str(),
                section.item_max_bytes(),
                text.len()
            )));
        }
        let now = memory::now_secs();
        let mem = self.build_essence_record(scope, section, text, refs, Some(now))?;
        let id = self.put(mem)?;
        Ok(id)
    }

    /// Phase 12.7b: append a new timeline entry (append-only
    /// milestone). Content ≤ [`memory::ESSENCE_TIMELINE_MAX_BYTES`]
    /// (600 bytes); entries over that are rejected (not truncated —
    /// truncating a milestone silently loses the narrative).
    pub fn essence_append_timeline(
        &mut self,
        scope: EssenceScope,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if text.is_empty() {
            return Err(MnemoError::Invalid(
                "essence_append_timeline: text must not be empty".into(),
            ));
        }
        if text.len() > EssenceSection::Timeline.item_max_bytes() {
            return Err(MnemoError::Invalid(format!(
                "essence_append_timeline: entries must be <= {} bytes; got {}",
                EssenceSection::Timeline.item_max_bytes(),
                text.len()
            )));
        }
        let mem = self.build_essence_record(scope, EssenceSection::Timeline, text, refs, None)?;
        let id = self.put(mem)?;
        Ok(id)
    }

    /// Phase 12.7b: update an existing essence section item in place.
    /// Returns [`WriteOutcome::Updated`] / [`WriteOutcome::Unchanged`].
    /// Timeline entries are append-only — calling this on a timeline
    /// record is an error.
    pub fn essence_update(
        &mut self,
        id: &Ulid,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let existing = self.load_essence_record(id)?;
        let section = self.section_of(&existing)?;
        if section == EssenceSection::Timeline {
            return Err(MnemoError::Invalid(
                "essence_update: timeline entries are append-only; append a new one instead".into(),
            ));
        }
        if text.is_empty() {
            return Err(MnemoError::Invalid(
                "essence_update: text must not be empty".into(),
            ));
        }
        if text.len() > section.item_max_bytes() {
            return Err(MnemoError::Invalid(format!(
                "essence_update: '{}' items must be <= {} bytes; got {}",
                section.as_str(),
                section.item_max_bytes(),
                text.len()
            )));
        }
        let mut patch = MemoryPatch {
            content: Some(text.to_string()),
            ..Default::default()
        };
        // We don't have a usable embedder-free path for an in-place
        // content update, so provide the same vector as the existing
        // record (the text-vector drift guard in `update` will accept
        // this).
        patch.vector = Some(existing.vector.clone());
        if let Some(r) = refs {
            let mut meta = Map::new();
            // Preserve the user-visible metadata (none for essence —
            // everything lives under `_mnemo.*`) and let `update`'s
            // metadata-replacement path keep the reserved keys.
            meta.insert(
                MNEMO_META_REFS.into(),
                Value::Array(r.into_iter().map(Value::String).collect()),
            );
            patch.metadata = Some(meta);
        }
        self.update(id, patch)
    }

    /// Phase 12.7b: mark a section item as resolved (open → resolved
    /// transition). A new history entry lands on
    /// `_mnemo.status_history`. Idempotent — resolving a record that
    /// is already resolved is a no-op.
    pub fn essence_resolve(&mut self, id: &Ulid) -> Result<WriteOutcome> {
        self.essence_transition_status(id, essence_status::RESOLVED)
    }

    /// Phase 12.7b: soft-delete a section item. Same semantics as
    /// [`Mnemo::delete`] on any record — space reclaimed by a later
    /// compact.
    pub fn essence_remove(&mut self, id: &Ulid) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.delete(id)
    }

    /// Phase 12.7b: deterministic, budgeted markdown render of the
    /// essence layer for a thread (or the project layer when
    /// `scope = EssenceScope::Project`). `max_bytes` caps the output;
    /// sections are emitted in the fixed [`EssenceSection::render_order`],
    /// and each section's own budget is proportional to its position
    /// in the order. Oldest timeline entries fold into a
    /// `+N earlier milestones` tail when the budget runs out.
    ///
    /// The render always ends with an archive pointer so the reader
    /// knows how to drill in deeper (`mnemo thread export …` or
    /// `recall --thread …`).
    pub fn essence(&mut self, scope: EssenceScope, max_bytes: usize) -> Result<String> {
        let max_bytes = if max_bytes == 0 {
            ESSENCE_DEFAULT_RENDER_BYTES
        } else {
            max_bytes
        };
        // Collect every live essence record for this scope.
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut by_section: std::collections::BTreeMap<&'static str, Vec<Memory>> =
            std::collections::BTreeMap::new();
        for section in EssenceSection::render_order() {
            by_section.insert(section.as_str(), Vec::new());
        }
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_ESSENCE)
            {
                continue;
            }
            // Scope filter. Project-scope records are shown in every
            // thread's render (and in the project render); thread-scope
            // records only match when scope == same thread.
            let rec_thread = m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str());
            let matches_scope = match (&scope, rec_thread) {
                (EssenceScope::Project, None) => true,
                (EssenceScope::Thread(_), None) => true, // project-scope always shows
                (EssenceScope::Thread(t), Some(rt)) => rt == t,
                (EssenceScope::Project, Some(_)) => false,
            };
            if !matches_scope {
                continue;
            }
            let section_str = m
                .metadata
                .get(MNEMO_META_SECTION)
                .and_then(|v| v.as_str())
                .unwrap_or("timeline");
            by_section
                .entry(
                    EssenceSection::parse(section_str)
                        .map(|s| s.as_str())
                        .unwrap_or("timeline"),
                )
                .or_default()
                .push(m);
        }

        // Sort each section's records:
        // - Decisions + Timeline: newest-first (by created_at desc).
        // - Everything else: sort open items before resolved, then
        //   newest-first. Cheap, deterministic.
        for (name, items) in by_section.iter_mut() {
            let newest_first = matches!(*name, "decisions" | "timeline");
            items.sort_by(|a, b| {
                let a_resolved = status_of(a) == Some(essence_status::RESOLVED);
                let b_resolved = status_of(b) == Some(essence_status::RESOLVED);
                match (a_resolved, b_resolved) {
                    (false, true) => std::cmp::Ordering::Less,
                    (true, false) => std::cmp::Ordering::Greater,
                    _ => {
                        if newest_first {
                            b.created_at.cmp(&a.created_at)
                        } else {
                            a.created_at.cmp(&b.created_at)
                        }
                    }
                }
            });
        }

        // Per-section budget: give intent / conventions / decisions a
        // larger share (they frame the thread), timeline a smaller
        // share (it can fold). This is deterministic because it
        // depends only on `max_bytes`.
        let header_reserve = 256usize; // header + archive pointer
        let body_budget = max_bytes.saturating_sub(header_reserve);
        let per_section_budget = |section: EssenceSection| -> usize {
            match section {
                EssenceSection::Intent => body_budget / 8,
                EssenceSection::Conventions => body_budget / 6,
                EssenceSection::Decisions => body_budget / 4,
                EssenceSection::State => body_budget / 8,
                EssenceSection::Open => body_budget / 8,
                EssenceSection::Next => body_budget / 12,
                EssenceSection::Lessons => body_budget / 12,
                EssenceSection::Timeline => body_budget / 4,
            }
        };

        let mut out = String::with_capacity(max_bytes);
        let scope_label = match &scope {
            EssenceScope::Thread(t) => format!("thread `{t}`"),
            EssenceScope::Project => "project".to_string(),
        };
        out.push_str(&format!("# essence — {scope_label}\n\n"));

        // Phase 12.7c: continuation. If this is a thread render AND
        // the thread has a `_mnemo.continues` link, pull the prior
        // thread's open section items, its `next`, and its last-N
        // timeline entries in ahead of the new thread's own essence.
        if let EssenceScope::Thread(new_id) = &scope {
            if let Some(prev_id) = self.predecessor_thread(new_id)? {
                let continuation = self
                    .render_continuation_prelude(&prev_id, max_bytes.saturating_sub(out.len()))?;
                if !continuation.is_empty() && out.len() + continuation.len() <= max_bytes {
                    out.push_str(&continuation);
                }
            }
        }

        for section in EssenceSection::render_order() {
            let items = match by_section.get(section.as_str()) {
                Some(v) if !v.is_empty() => v,
                _ => continue,
            };
            let budget = per_section_budget(*section);
            let section_header = format!("## {}\n", section.as_str());
            if out.len() + section_header.len() > max_bytes {
                break;
            }
            out.push_str(&section_header);
            let mut used = 0usize;
            let mut rendered = 0usize;
            for item in items {
                let line = render_essence_item(*section, item);
                if used + line.len() > budget {
                    break;
                }
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
                used += line.len();
                rendered += 1;
            }
            // Fold the remainder for timeline.
            if *section == EssenceSection::Timeline && rendered < items.len() {
                let fold = format!("- +{} earlier milestones\n", items.len() - rendered);
                if out.len() + fold.len() <= max_bytes {
                    out.push_str(&fold);
                }
            }
            if out.len() < max_bytes {
                out.push('\n');
            }
        }

        // Archive pointer.
        let pointer = match &scope {
            EssenceScope::Thread(t) => format!(
                "> full history: `mnemo thread export <file> --thread {t}` or `recall --thread {t}`\n"
            ),
            EssenceScope::Project => String::from(
                "> full history: `mnemo thread export <file> --thread <id>` or `recall --thread <id>`\n",
            ),
        };
        if out.len() + pointer.len() <= max_bytes {
            out.push_str(&pointer);
        }
        Ok(out)
    }

    // --- essence internal helpers ----------------------------------

    fn build_essence_record(
        &self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
        status_at: Option<i64>,
    ) -> Result<Memory> {
        if let Some(e) = self.embedder.as_ref() {
            let mut vs = e.embed(&[text])?;
            let v = vs.pop().ok_or_else(|| {
                MnemoError::Invalid("essence: embedder returned no vector".into())
            })?;
            let mut m = Memory::new(text.to_string(), MemoryType::Semantic, v).with_importance(0.5);
            m.metadata.insert(
                MNEMO_META_KIND.into(),
                Value::String(MNEMO_META_KIND_ESSENCE.into()),
            );
            m.metadata.insert(
                MNEMO_META_SECTION.into(),
                Value::String(section.as_str().into()),
            );
            if let EssenceScope::Thread(t) = &scope {
                m.metadata
                    .insert(MNEMO_META_THREAD.into(), Value::String(t.clone()));
            }
            if let Some(r) = refs {
                m.metadata.insert(
                    MNEMO_META_REFS.into(),
                    Value::Array(r.into_iter().map(Value::String).collect()),
                );
            }
            if let Some(at) = status_at {
                m.metadata.insert(
                    MNEMO_META_STATUS.into(),
                    Value::String(essence_status::OPEN.into()),
                );
                m.metadata.insert(
                    MNEMO_META_STATUS_HISTORY.into(),
                    Value::Array(vec![serde_json::json!({
                        "status": essence_status::OPEN,
                        "at": at,
                    })]),
                );
            }
            Ok(m)
        } else {
            // No embedder: use an axis-neutral all-equal vector
            // scaled to unit magnitude. Essence records are retrieved
            // by `_mnemo.kind` filter, not by similarity, so the
            // concrete vector doesn't matter for retrieval; but we
            // need something non-zero to pass `put`'s guard.
            let dims = self.dimensions.max(1);
            let scale = 1.0 / (dims as f32).sqrt();
            let v = vec![scale; dims];
            let mut m = Memory::new(text.to_string(), MemoryType::Semantic, v).with_importance(0.5);
            m.metadata.insert(
                MNEMO_META_KIND.into(),
                Value::String(MNEMO_META_KIND_ESSENCE.into()),
            );
            m.metadata.insert(
                MNEMO_META_SECTION.into(),
                Value::String(section.as_str().into()),
            );
            if let EssenceScope::Thread(t) = &scope {
                m.metadata
                    .insert(MNEMO_META_THREAD.into(), Value::String(t.clone()));
            }
            if let Some(r) = refs {
                m.metadata.insert(
                    MNEMO_META_REFS.into(),
                    Value::Array(r.into_iter().map(Value::String).collect()),
                );
            }
            if let Some(at) = status_at {
                m.metadata.insert(
                    MNEMO_META_STATUS.into(),
                    Value::String(essence_status::OPEN.into()),
                );
                m.metadata.insert(
                    MNEMO_META_STATUS_HISTORY.into(),
                    Value::Array(vec![serde_json::json!({
                        "status": essence_status::OPEN,
                        "at": at,
                    })]),
                );
            }
            Ok(m)
        }
    }

    fn load_essence_record(&mut self, id: &Ulid) -> Result<Memory> {
        let m = self.get(id)?;
        if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()) != Some(MNEMO_META_KIND_ESSENCE)
        {
            return Err(MnemoError::Invalid(format!(
                "essence: record {id} is not an essence record (missing `_mnemo.kind = essence`)"
            )));
        }
        Ok(m)
    }

    fn section_of(&self, m: &Memory) -> Result<EssenceSection> {
        let s = m
            .metadata
            .get(MNEMO_META_SECTION)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                MnemoError::Invalid("essence: record missing `_mnemo.section`".into())
            })?;
        EssenceSection::parse(s)
            .ok_or_else(|| MnemoError::Invalid(format!("essence: unknown section '{s}' on record")))
    }

    fn essence_transition_status(&mut self, id: &Ulid, to: &str) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let existing = self.load_essence_record(id)?;
        let section = self.section_of(&existing)?;
        if !section.has_status_lifecycle() {
            return Err(MnemoError::Invalid(format!(
                "essence_transition: '{}' section has no status lifecycle",
                section.as_str()
            )));
        }
        let from = existing
            .metadata
            .get(MNEMO_META_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or(essence_status::OPEN);
        if from == to {
            return Ok(WriteOutcome::Unchanged);
        }
        let allowed = ESSENCE_TRANSITIONS
            .iter()
            .any(|(f, t)| *f == from && *t == to);
        if !allowed {
            return Err(MnemoError::Invalid(format!(
                "essence_transition: '{from}' → '{to}' is not a valid transition for essence"
            )));
        }
        // Mutate metadata in place and `put` back under the same id
        // so `_mnemo.updated_at` gets stamped and access stats are
        // preserved.
        let mut new = existing.clone();
        new.metadata
            .insert(MNEMO_META_STATUS.into(), Value::String(to.to_string()));
        let now = memory::now_secs();
        let history_entry = serde_json::json!({ "status": to, "at": now });
        let history = new
            .metadata
            .entry(MNEMO_META_STATUS_HISTORY.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(arr) = history {
            arr.push(history_entry);
        }
        new.metadata
            .insert(MNEMO_META_UPDATED_AT.into(), Value::Number(now.into()));
        new.id = existing.id;
        new.created_at = existing.created_at;
        self.put(new)?;
        Ok(WriteOutcome::Updated)
    }

    // --- Phase 12.7c: continuation + commit_deltas ------------------

    /// Start a new thread that continues from a previous one
    /// (Phase 12.7c). Writes a thread-link marker record with
    /// `_mnemo.kind = "thread_link"` + `_mnemo.thread = <new_id>` +
    /// `_mnemo.continues = <from_id>`. The marker holds no user
    /// content; its purpose is to let `essence(Thread(new_id))`
    /// discover the continuation chain.
    ///
    /// The render for the new thread pulls the previous thread's
    /// open section items, its `next`, and its last
    /// [`CONTINUATION_TIMELINE_ENTRIES`] timeline entries ahead of
    /// the new thread's own essence — the "new thread, same
    /// experience" path.
    pub fn thread_start(&mut self, new_thread_id: &str, from_thread_id: &str) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if new_thread_id.is_empty() || from_thread_id.is_empty() {
            return Err(MnemoError::Invalid(
                "thread_start: 'new_thread_id' and 'from_thread_id' must not be empty".into(),
            ));
        }
        if new_thread_id == from_thread_id {
            return Err(MnemoError::Invalid(
                "thread_start: 'new_thread_id' must differ from 'from_thread_id'".into(),
            ));
        }
        // Store a tiny marker record so the continuation chain is
        // queryable from a fresh handle. Vector is the sentinel
        // all-equal-axis unit spike (same as the scaffold manifest
        // path) so `put`'s zero-vector guard accepts it.
        let dims = self.dimensions.max(1);
        let scale = 1.0 / (dims as f32).sqrt();
        let v = vec![scale; dims];
        let content = format!(
            "thread link: {new_thread_id} continues from {from_thread_id}"
        );
        let mut m = Memory::new(content, MemoryType::Semantic, v).with_importance(0.1);
        m.metadata.insert(
            MNEMO_META_KIND.into(),
            Value::String(MNEMO_META_KIND_THREAD_LINK.into()),
        );
        m.metadata.insert(
            MNEMO_META_THREAD.into(),
            Value::String(new_thread_id.to_string()),
        );
        m.metadata.insert(
            MNEMO_META_CONTINUES.into(),
            Value::String(from_thread_id.to_string()),
        );
        let id = self.put(m)?;
        Ok(id)
    }

    /// Phase 12.7c helper: render the predecessor thread's open
    /// items + `next` + last-N timeline entries as a "continued from"
    /// prelude. Used by [`Mnemo::essence`] to carry forward the
    /// prior thread's live state into a fresh thread.
    fn render_continuation_prelude(
        &mut self,
        prev_thread: &str,
        max_bytes: usize,
    ) -> Result<String> {
        if max_bytes == 0 {
            return Ok(String::new());
        }
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut open_items: Vec<Memory> = Vec::new();
        let mut next_items: Vec<Memory> = Vec::new();
        let mut timeline_items: Vec<Memory> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_ESSENCE)
            {
                continue;
            }
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(prev_thread)
            {
                continue;
            }
            let section = match m
                .metadata
                .get(MNEMO_META_SECTION)
                .and_then(|v| v.as_str())
                .and_then(EssenceSection::parse)
            {
                Some(s) => s,
                None => continue,
            };
            let status = m
                .metadata
                .get(MNEMO_META_STATUS)
                .and_then(|v| v.as_str())
                .unwrap_or(essence_status::OPEN);
            match section {
                EssenceSection::Open if status == essence_status::OPEN => {
                    open_items.push(m);
                }
                EssenceSection::Next if status == essence_status::OPEN => {
                    next_items.push(m);
                }
                EssenceSection::Timeline => timeline_items.push(m),
                _ => {}
            }
        }
        // Newest-first timeline, cap to last N.
        timeline_items.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        timeline_items.truncate(CONTINUATION_TIMELINE_ENTRIES);
        // Oldest-first for stable chronological read in the render.
        timeline_items.reverse();

        let mut out = String::new();
        let header = format!("## continued from `{prev_thread}`\n");
        if header.len() > max_bytes {
            return Ok(String::new());
        }
        out.push_str(&header);
        if !open_items.is_empty() {
            let sub = "### open\n";
            if out.len() + sub.len() <= max_bytes {
                out.push_str(sub);
            }
            for m in &open_items {
                let line = format!("- {}\n", m.content);
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
            }
        }
        if !next_items.is_empty() {
            let sub = "### next\n";
            if out.len() + sub.len() <= max_bytes {
                out.push_str(sub);
            }
            for m in &next_items {
                let line = format!("- {}\n", m.content);
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
            }
        }
        if !timeline_items.is_empty() {
            let sub = "### recent milestones\n";
            if out.len() + sub.len() <= max_bytes {
                out.push_str(sub);
            }
            for m in &timeline_items {
                let line = format!("- {}\n", m.content);
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
            }
        }
        if out.len() < max_bytes {
            out.push('\n');
        }
        Ok(out)
    }

    /// Phase 12.7c helper: find the predecessor thread id for
    /// `thread_id` by reading its `_mnemo.kind = "thread_link"`
    /// marker. Returns `None` if the thread has no predecessor
    /// (fresh thread, no `thread_start` call).
    fn predecessor_thread(&mut self, thread_id: &str) -> Result<Option<String>> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_THREAD_LINK)
            {
                continue;
            }
            let this = m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str());
            if this != Some(thread_id) {
                continue;
            }
            return Ok(m
                .metadata
                .get(MNEMO_META_CONTINUES)
                .and_then(|v| v.as_str())
                .map(String::from));
        }
        Ok(None)
    }

    /// Atomic batch write combining 12.1 facts, 12.7b essence ops,
    /// and 12.7a archive turns in one flush (Phase 12.7c).
    ///
    /// **Validation up front.** Every op in the batch is validated
    /// before any write runs: missing required fields in an
    /// [`EssenceOp`], an invalid `thread_id` for turns, a fact
    /// whose vector has the wrong dimensionality, a timeline entry
    /// over the 600-byte cap, etc. Any validation error returns
    /// `Err(MnemoError::Invalid)` and the file is untouched.
    ///
    /// **One flush.** On success, every sub-write runs in order
    /// (facts → essence ops → turns per `turn_policy`) and the
    /// method calls `self.flush()` exactly once on exit. On a
    /// failure that lands past validation (e.g. an I/O error mid-
    /// batch), the file reflects whatever had already been written
    /// — the batch is atomic at the "validation gate" but not
    /// crash-atomic. Callers that need crash atomicity should use
    /// snapshots.
    pub fn commit_deltas(&mut self, mut batch: CommitBatch) -> Result<CommitReport> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // --- Validation gate (no writes yet) ----------------------
        // Facts: vectors must match the handle's dimensionality.
        for (i, f) in batch.facts.iter().enumerate() {
            if f.memory.vector.len() != self.dimensions {
                return Err(MnemoError::Invalid(format!(
                    "commit_deltas: facts[{i}] vector has {} dims; this file is {}",
                    f.memory.vector.len(),
                    self.dimensions
                )));
            }
            if let Some(k) = &f.key {
                if k.is_empty() {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] key is empty"
                    )));
                }
            }
        }
        // Essence ops: per-kind field presence + text size caps.
        for (i, op) in batch.essence_ops.iter().enumerate() {
            match op.kind {
                EssenceOpKind::Add => {
                    if op.scope.is_none() {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] Add missing 'scope'"
                        )));
                    }
                    let section = op.section.ok_or_else(|| {
                        MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] Add missing 'section'"
                        ))
                    })?;
                    if section == EssenceSection::Timeline {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] Add on Timeline; use AppendTimeline"
                        )));
                    }
                    let text = op.text.as_deref().ok_or_else(|| {
                        MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] Add missing 'text'"
                        ))
                    })?;
                    if text.is_empty() || text.len() > section.item_max_bytes() {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] Add text length invalid (0 or > {})",
                            section.item_max_bytes()
                        )));
                    }
                }
                EssenceOpKind::AppendTimeline => {
                    if op.scope.is_none() {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] AppendTimeline missing 'scope'"
                        )));
                    }
                    let text = op.text.as_deref().ok_or_else(|| {
                        MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] AppendTimeline missing 'text'"
                        ))
                    })?;
                    if text.is_empty()
                        || text.len() > EssenceSection::Timeline.item_max_bytes()
                    {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] AppendTimeline text length invalid (0 or > {})",
                            EssenceSection::Timeline.item_max_bytes()
                        )));
                    }
                }
                EssenceOpKind::Update => {
                    if op.id.is_none() || op.text.is_none() {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] Update needs 'id' and 'text'"
                        )));
                    }
                }
                EssenceOpKind::Resolve | EssenceOpKind::Remove => {
                    if op.id.is_none() {
                        return Err(MnemoError::Invalid(format!(
                            "commit_deltas: essence_ops[{i}] needs 'id'"
                        )));
                    }
                }
            }
        }
        // Turns: thread_id required when non-empty.
        if !batch.turns.is_empty() && batch.thread_id.is_none() {
            return Err(MnemoError::Invalid(
                "commit_deltas: 'turns' non-empty but 'thread_id' missing".into(),
            ));
        }

        // --- Apply, in planner-specified order --------------------
        let mut report = CommitReport::default();

        // 1) facts
        for f in batch.facts.drain(..) {
            match f.key {
                Some(k) => {
                    let (_id, outcome) = self.remember_keyed(f.memory, &k)?;
                    match outcome {
                        WriteOutcome::Added => report.facts_added += 1,
                        WriteOutcome::Superseded => report.facts_superseded += 1,
                        WriteOutcome::Unchanged => report.facts_unchanged += 1,
                        WriteOutcome::Updated => report.facts_superseded += 1,
                    }
                }
                None => {
                    self.remember(f.memory)?;
                    report.facts_added += 1;
                }
            }
        }

        // 2) essence ops
        for op in batch.essence_ops.drain(..) {
            match op.kind {
                EssenceOpKind::Add => {
                    self.essence_add(
                        op.scope.expect("validated"),
                        op.section.expect("validated"),
                        op.text.as_deref().expect("validated"),
                        op.refs,
                    )?;
                    report.essence_ops_applied += 1;
                }
                EssenceOpKind::AppendTimeline => {
                    self.essence_append_timeline(
                        op.scope.expect("validated"),
                        op.text.as_deref().expect("validated"),
                        op.refs,
                    )?;
                    report.essence_ops_applied += 1;
                }
                EssenceOpKind::Update => {
                    let outcome = self.essence_update(
                        op.id.as_ref().expect("validated"),
                        op.text.as_deref().expect("validated"),
                        op.refs,
                    )?;
                    match outcome {
                        WriteOutcome::Updated | WriteOutcome::Added | WriteOutcome::Superseded => {
                            report.essence_ops_applied += 1
                        }
                        WriteOutcome::Unchanged => report.essence_ops_unchanged += 1,
                    }
                }
                EssenceOpKind::Resolve => {
                    let outcome = self.essence_resolve(op.id.as_ref().expect("validated"))?;
                    match outcome {
                        WriteOutcome::Updated | WriteOutcome::Added | WriteOutcome::Superseded => {
                            report.essence_ops_applied += 1
                        }
                        WriteOutcome::Unchanged => report.essence_ops_unchanged += 1,
                    }
                }
                EssenceOpKind::Remove => {
                    self.essence_remove(op.id.as_ref().expect("validated"))?;
                    report.essence_ops_applied += 1;
                }
            }
        }

        // 3) turns per turn_policy
        match batch.turn_policy {
            TurnPolicy::Archive => {
                if !batch.turns.is_empty() {
                    let thread_id = batch
                        .thread_id
                        .as_deref()
                        .expect("validated above");
                    let r = self.ingest_thread(thread_id, &batch.turns)?;
                    report.turns_added += r.added_records;
                    report.turns_skipped += r.skipped_existing_turns;
                    report.turns_truncated += r.truncated_turns;
                    report.turns_chunked += r.chunked_turns;
                }
            }
            TurnPolicy::ExpireAfter(ttl) => {
                if !batch.turns.is_empty() {
                    let thread_id = batch
                        .thread_id
                        .as_deref()
                        .expect("validated above");
                    // Reuse ingest, then apply TTL via `update` on
                    // the newly-written records. Simpler than a
                    // dedicated path — the TTL is advisory anyway
                    // (compact + is_expired do the work).
                    let r = self.ingest_thread(thread_id, &batch.turns)?;
                    report.turns_added += r.added_records;
                    report.turns_skipped += r.skipped_existing_turns;
                    report.turns_truncated += r.truncated_turns;
                    report.turns_chunked += r.chunked_turns;
                    // Walk the just-ingested records and stamp TTL.
                    // We can identify them by `(thread, seq >=
                    // next_seq_before)`; simplest: pull the whole
                    // thread, and for turns whose turn_id is in the
                    // input set but wasn't already there (handled
                    // by ingest_thread's idempotence check), set TTL.
                    let want_ids: std::collections::HashSet<String> =
                        batch.turns.iter().map(|t| t.turn_id.clone()).collect();
                    let records = self.thread_export(thread_id)?;
                    for rec in records {
                        if let Some(tid) = rec
                            .metadata
                            .get(MNEMO_META_TURN_ID)
                            .and_then(|v| v.as_str())
                        {
                            if want_ids.contains(tid) && rec.ttl_secs.is_none() {
                                let _ = self.update(
                                    &rec.id,
                                    MemoryPatch {
                                        ttl_secs: Some(Some(ttl)),
                                        ..Default::default()
                                    },
                                )?;
                            }
                        }
                    }
                }
            }
            TurnPolicy::Drop => {
                // No-op: nothing goes to the archive.
            }
        }

        self.flush()?;
        Ok(report)
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
        // Phase 12.1: drop records that have been superseded by a newer
        // keyed write. They still live in snapshot history and
        // `history(key)` — just not here.
        let mut out: Vec<Memory> = self
            .memories()?
            .into_iter()
            .filter(|m| !m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY))
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
    /// One pass of filter + score for [`Mnemo::recall`]. Factored out of
    /// `recall` so the outer path can retry with a wider shortlist when
    /// the type/agent/supersession/TTL filters starve the ANN-narrowed
    /// result below `top_k` (Phase 12.1 short-results-on-indexed-path
    /// fix).
    ///
    /// * `force_exact = false` → use the ANN index when one exists; fall
    ///   back to a full scan only if none is built.
    /// * `force_exact = true` → skip the ANN index and scan every live
    ///   non-superseded record. Used by the outer path as the last-resort
    ///   attempt when widening the shortlist would already touch half or
    ///   more of the live non-superseded set.
    ///
    /// Returns an unsorted, un-truncated `Vec<RecallResult>`; the outer
    /// `recall` sorts and truncates once.
    fn score_once(
        &mut self,
        req: &RecallRequest,
        now: i64,
        force_exact: bool,
    ) -> Result<Vec<RecallResult>> {
        // Candidate set: ANN-narrowed when an index exists and the
        // caller hasn't forced an exact scan, else everything live.
        let entries: Vec<CatalogEntry> = match (&self.ann, force_exact) {
            (Some(ann), false) => {
                let ids = ann.query(&req.query, req.n_probe, req.n_rerank);
                ids.iter()
                    .filter_map(|id| self.index.get(id).copied())
                    .map(|i| self.catalog[i].clone())
                    .filter(|e| !e.deleted)
                    .collect()
            }
            _ => self
                .catalog
                .iter()
                .filter(|e| !e.deleted)
                .cloned()
                .collect(),
        };

        let mut scored: Vec<RecallResult> = Vec::with_capacity(entries.len());
        for e in &entries {
            // Phase 12.1: drop superseded records.
            //
            // Fast path: if the lazy `superseded` set has already been
            // built by a prior keyed op on this handle, check it before
            // decrypt — O(1), no AES work. On a fresh read-only handle
            // it is typically `None`, so we fall through to the slow
            // path (one `_mnemo.superseded_by` lookup per surviving
            // record, O(k) over user metadata), and never trigger the
            // full-file scan that `ensure_key_caches()` would do.
            if self.superseded.as_ref().is_some_and(|s| s.contains(&e.id)) {
                continue;
            }
            let m = self.read_memory(e)?;
            if m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) {
                continue;
            }
            if m.is_expired(now) {
                continue;
            }
            if let Some(types) = &req.memory_types {
                if !types.contains(&m.memory_type) {
                    continue;
                }
            }
            if let Some(agent) = &req.agent_id {
                let visible = m.agent_id == *agent || m.scope == Scope::Shared;
                if !visible {
                    continue;
                }
            }
            // Phase 12.6: pre-scoring filter (session_id + metadata
            // conditions). Cheap when `req.filter.is_empty()`; the
            // filter struct's `matches` is a short-circuit AND.
            if !req.filter.is_empty() && !req.filter.matches(&m) {
                continue;
            }
            // Phase 12.7a: default-exclude thread-archive turns. A
            // caller that wants turns opts in by placing any
            // `_mnemo.*` term in the filter (equality, `$in`,
            // `$exists`, or a range on `_mnemo.thread` /
            // `_mnemo.kind` / `_mnemo.seq`). Without such a term, we
            // skip `_mnemo.kind = "turn"` records so the archive
            // layer doesn't pollute default recall.
            if !req.filter.mentions_mnemo_meta()
                && m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                    == Some(MNEMO_META_KIND_TURN)
            {
                continue;
            }
            let sim = memory::similarity(req.metric, &req.query, &m.vector);
            let age_secs = (now - m.created_at) as f32;
            let score = req
                .weights
                .score(sim, age_secs, m.importance, m.access_count);
            scored.push(RecallResult {
                memory: m,
                score,
                similarity: sim,
            });
        }
        Ok(scored)
    }

    /// Phase-5 multi-signal recall under the v0.7.0 scoring-v2 formula
    /// (see [`crate::ScoreWeights`]). Thin wrapper over
    /// [`Mnemo::recall_budgeted`] that drops the `truncated` flag;
    /// use the budgeted form if you need to know whether
    /// [`RecallRequest::max_bytes`] cut anything.
    pub fn recall(&mut self, req: &RecallRequest) -> Result<Vec<RecallResult>> {
        Ok(self.recall_budgeted(req)?.hits)
    }

    /// Phase 12.8 budgeted recall: scoring v2 (see [`crate::ScoreWeights`])
    /// with an optional UTF-8-content byte cap. Ranks by
    /// `sim + max(sim, 0) · (β·recency + γ·(importance − 0.5) + δ·tanh(uses/5))`,
    /// with type / agent / supersession / TTL filters and the 12.6
    /// `Filter` applied before the scorer. Updates `accessed_at` /
    /// `access_count` on the returned memories (persisted on the next
    /// `flush`).
    ///
    /// When an ANN index has been built ([`Mnemo::build_index`]),
    /// recall runs the tiered IVF→PQ→rerank pipeline. If the filters
    /// (type / agent / supersession / TTL / 12.6) reduce the ANN
    /// shortlist below `top_k`, Phase 12.1's short-results-on-indexed-
    /// path fix kicks in: the query is retried with `n_probe` and
    /// `n_rerank` doubled, and if that would already touch half or
    /// more of the live non-superseded records, the ANN pass is
    /// skipped for an exact scan. Without an index, recall scores
    /// every live non-superseded memory.
    ///
    /// Phase 12.8: after `top_k` is applied, if
    /// [`RecallRequest::max_bytes`] is set, hits are dropped in rank
    /// order once the running sum of `content` UTF-8 bytes would
    /// exceed the budget. The top-1 hit is always kept even if it
    /// alone exceeds the budget. [`RecallResponse::truncated`] is
    /// `true` iff at least one hit was dropped by the budget.
    pub fn recall_budgeted(&mut self, req: &RecallRequest) -> Result<RecallResponse> {
        if req.query.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: req.query.len(),
            });
        }
        // Access-tracking bumps `accessed_at` / `access_count` in the
        // catalog, which is a mutation. On a read-only handle the caller
        // must pass `track_access(false)` (via CLI `--no-track`).
        if self.read_only && req.track_access {
            return Err(MnemoError::ReadOnly);
        }
        // Phase 12.1 **fix** (lane A v0.8.0 REVIEW): do NOT call
        // `ensure_key_caches()` here. A fresh read-only handle that
        // only ever recalls would otherwise pay a full-file decrypt
        // on its first query to build the lazy superseded set. Instead
        // `score_once` checks `_mnemo.superseded_by` per-record after
        // decrypt (slow path) and consults the set only when it has
        // already been built by a prior keyed op on this handle (fast
        // path). `live_non_superseded` is not knowable without a
        // decrypt pass either, so we approximate by `live_count`; the
        // exact-scan gate uses this as a conservative upper bound,
        // which errs toward widening (cheaper) rather than skipping
        // straight to a full scan.
        let live_count = self.catalog.iter().filter(|e| !e.deleted).count();
        let live_non_superseded = live_count;
        let now = memory::now_secs();

        // Scoring v2 (shipped v0.7.0). Single-pass filter + score:
        //   * `age_secs = now - m.created_at`, so a recall doesn't bump
        //     its own results' recency on later queries.
        //   * `ScoreWeights::score` applies the similarity-gated,
        //     importance-centered, saturating-usage formula documented
        //     on `ScoreWeights`.
        //
        // Phase 12.1 + 12.6 short-results-on-indexed-path fix: when the
        // ANN shortlist is narrower than the live count, the filters
        // (type / agent / supersession / TTL / 12.6) can reduce the
        // result below `top_k`. If that happens, re-query the index
        // with `n_probe` / `n_rerank` doubled from their *effective*
        // base; if that would already cover ≥50% of the live set,
        // skip the index and do an exact scan.
        //
        // Lane A v0.8.0 REVIEW fix: widen from
        // `IvfPqIndex::effective_defaults()` when the caller didn't
        // pin the knobs. The pre-fix code only doubled when `req.n_*`
        // was already `Some(_)`, which skipped the widen step for the
        // default (unpinned) recall path and jumped straight to an
        // exact scan on moderately selective filters.
        let mut scored = self.score_once(req, now, false)?;
        let mut used_exact_scan = self.ann.is_none();
        if scored.len() < req.top_k && live_non_superseded > req.top_k {
            if let Some(ann) = &self.ann {
                let (default_probe, default_rerank) = ann.effective_defaults();
                let base_probe = req.n_probe.unwrap_or(default_probe);
                let base_rerank = req.n_rerank.unwrap_or(default_rerank);
                let widen_probe = base_probe.saturating_mul(2);
                let widen_rerank = base_rerank.saturating_mul(2);
                // Attempt 2: widen once, 2× from the effective base.
                // Only worth another ANN pass if either knob actually
                // grew (saturating_mul on usize::MAX would cap).
                if widen_probe > base_probe || widen_rerank > base_rerank {
                    let widened = RecallRequest {
                        n_probe: Some(widen_probe),
                        n_rerank: Some(widen_rerank),
                        ..req.clone()
                    };
                    scored = self.score_once(&widened, now, false)?;
                }
                // Attempt 3: if still short after widening, pay the
                // exact scan — it's the only path that guarantees
                // `top_k` correct hits when the filter is more
                // selective than `widen_rerank / live_count`. Before
                // the fix this was gated on `widened_rerank * 2 >=
                // live_non_superseded`, which silently returned fewer
                // than `top_k` hits on selective filters that fell in
                // the gap. Exact scan is bounded (one decrypt per
                // live record); the previous widen is an optimization.
                if scored.len() < req.top_k {
                    scored = self.score_once(req, now, true)?;
                    used_exact_scan = true;
                }
            }
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(req.top_k);

        // Update access statistics for everything we surfaced — but only
        // mutate the catalog entry, not the full record on disk. Pre-v5
        // this loop called `self.put(r.memory.clone())`, which serialized
        // and rewrote every result (vector included) to fresh pages,
        // turning a top-K recall into K full record rewrites. Now the
        // catalog is the source of truth for these two fields and the
        // record body stays untouched until the next real edit.
        if req.track_access {
            for r in &mut scored {
                if let Some(&idx) = self.index.get(&r.memory.id.0) {
                    let entry = &mut self.catalog[idx];
                    entry.accessed_at = now;
                    entry.access_count = entry.access_count.saturating_add(1);
                    // Propagate the just-updated values onto the returned
                    // Memory so the caller sees the post-recall state.
                    r.memory.accessed_at = entry.accessed_at;
                    r.memory.access_count = entry.access_count;
                }
            }
            // Mark the catalog dirty so the next flush persists these
            // bumps. The records themselves are not dirty.
            if !scored.is_empty() {
                self.dirty_catalog = true;
            }
        }

        // Phase 12.8: apply the `max_bytes` budget *after* tracking has
        // seen the full top-k list. Access stats record what the scorer
        // chose; the byte budget shouldn't suppress the signal.
        let truncated = if let Some(budget) = req.max_bytes {
            let orig_len = scored.len();
            let mut running = 0usize;
            let mut cut_at: Option<usize> = None;
            for (i, r) in scored.iter().enumerate() {
                let w = r.memory.content.len();
                if i == 0 {
                    // Always keep the top hit, even if it's over budget
                    // on its own. Avoid "I asked for top_k results and
                    // got zero" when a single memory exceeds the cap.
                    running += w;
                    continue;
                }
                if running.saturating_add(w) > budget {
                    cut_at = Some(i);
                    break;
                }
                running += w;
            }
            if let Some(idx) = cut_at {
                scored.truncate(idx);
                scored.len() < orig_len
            } else {
                false
            }
        } else {
            false
        };

        Ok(RecallResponse {
            hits: scored,
            truncated,
            used_exact_scan,
        })
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
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
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
        // ANN-1.1: re-derive every parameter (partitions, subspaces,
        // n_probe, n_rerank) for the current corpus size by passing
        // `IndexConfig::default()`. Previously this freezed the
        // stored `(n_probe, n_rerank)` from the current index,
        // defeating the point of rebuilding on a grown corpus. If
        // you really want to pin specific values, call
        // `build_index_with(IndexConfig { … })` directly.
        self.build_index_with(IndexConfig::default())
    }

    /// Drop the ANN index; recall reverts to exact scans. Persisted on flush.
    /// No-op on a read-only handle (signature is infallible — callers of
    /// `build_index` / `flush` on a read-only handle already get a typed
    /// `ReadOnly` error, which is the enforceable boundary).
    pub fn drop_index(&mut self) {
        if self.read_only {
            return;
        }
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
        let old_wal_start = self.header.wal_start;
        let new_start = self.header.next_page;
        self.header.next_page += new_pages;
        self.header.wal_start = new_start;
        self.header.wal_pages = new_pages;
        self.header.wal_seq += 1;
        // DUR-1 (R-002.1): WAL-log the header change into the *new* region
        // first, checkpoint page 0, then invalidate the old region. A crash
        // anywhere here either keeps the old state (if page 0 wasn't yet
        // overwritten) or recovers to the new state via the torn-header
        // scan in `Mnemo::open`, which prefers the highest-txn-id valid
        // committed header frame.
        self.relocate_header_through_wal(old_wal_start)
    }

    /// Shared helper for every post-`create` page-0 commit: WAL-log the
    /// new header into the current WAL region, checkpoint page 0, and
    /// invalidate `old_wal_start` (the previous region, or the current
    /// region when it hasn't moved). The caller is expected to have
    /// mutated `self.header` to its target values *including a bumped
    /// `wal_seq`* (one more than the last on-disk `wal_seq`); this
    /// routine applies the seal, writes the frame + commit, folds page 0,
    /// then zeroes the old region's first frame header.
    ///
    /// If `old_wal_start == self.header.wal_start`, the invalidation is
    /// skipped — the region didn't move, so overwriting its first frame
    /// would destroy the just-committed one.
    fn relocate_header_through_wal(&mut self, old_wal_start: u64) -> Result<()> {
        self.header.write_counter = self.pager.write_counter;
        let mut page = self.header.to_page();
        self.header.apply_seal(&mut page, self.pager.dek())?;
        let txn_id = self.header.wal_seq;
        let frames = vec![(0u64, page.to_vec())];
        wal::commit(
            self.pager.file_mut(),
            self.header.wal_start,
            self.header.wal_pages,
            txn_id,
            &frames,
        )?;
        self.pager.write_raw(0, &page)?;
        self.pager.sync()?;
        if old_wal_start != self.header.wal_start {
            wal::invalidate_region(self.pager.file_mut(), old_wal_start)?;
        }
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
    /// Sequence:
    ///
    /// 1. **Prepare.** Serialize the control plane (catalog, ANN index),
    ///    grow the WAL if needed, and **lease** counter and page slots —
    ///    bump `header.write_counter` and `header.next_page` by upper
    ///    bounds on this transaction's consumption and persist that
    ///    *leased* header to disk before any encrypted page is written.
    /// 2. **Data pages.** [`Pager::flush`] writes dirty record pages
    ///    copy-on-write under fresh nonces and fsyncs them. The orphan-page
    ///    nonce-reuse window (see [Phase 1.1 of the improvement plan])
    ///    is closed because the leased header on disk already records a
    ///    `write_counter` past anything this step can produce.
    /// 3. **Control plane.** Seal the new catalog / ANN index / snapshot
    ///    manifest into fresh home page runs.
    /// 4. **Commit.** Log all of step 3's frames plus the final header
    ///    frame into the WAL and fsync — the durability point.
    /// 5. **Checkpoint.** Fold the WAL into the home pages.
    ///
    /// A crash before step 4 leaves the previous state intact. A crash
    /// after step 4 is repaired by [`Mnemo::open`] replaying the WAL.
    /// Safe to call repeatedly.
    pub fn flush(&mut self) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // Snapshot the thread-local fsync counter. Every `sync_all` the
        // pager / wal / checkpoint path makes between here and the end
        // of this function bumps the same counter; `delta` lands on
        // `self.fsyncs_last_flush` so `Stats::fsyncs_last_flush` can
        // report it. The planner wants this to decide whether cutting
        // fsync count is a viable write-latency win before we touch
        // the write path's durability model.
        let fsync_before = crate::fsync_count();
        if !self.dirty_catalog && !self.dirty_index && !self.dirty_cache {
            // No control-plane changes pending. Because every `put` that
            // dirties a data page also dirties the catalog, and every
            // `cache_put` that dirties a data page also dirties
            // `dirty_cache`, there can't be dirty data pages either —
            // nothing to do.
            self.fsyncs_last_flush = 0;
            return Ok(());
        }

        // FIX-1 R-001.8 part 3: snapshot the memory/index-dirty flags
        // before step 3b decides whether to push a restore point.
        // Only flushes that change the memory catalog or ANN index
        // need a snapshot — a cache-only flush (`dirty_cache` set,
        // `dirty_catalog` + `dirty_index` both false) must not push,
        // because snapshots don't capture cache state and every push
        // counts against `max_snapshots` (default 256), so a cache-
        // heavy workload would otherwise evict every real memory
        // restore point within ~256 cache writes.
        let is_memory_or_index_flush = self.dirty_catalog || self.dirty_index;

        // 1. Serialize control plane, ensure WAL capacity, lease counter +
        //    page slots, and persist the leased header *before* any data
        //    page hits the disk.
        let ctx = self.prepare_for_flush()?;

        // 2. Record (vector) data pages: copy-on-write to fresh pages, fsynced.
        //    Safe: the lease guarantees that even if we crash here, reopen
        //    will see a `write_counter` past every nonce this step uses.
        self.pager.flush()?;

        // 3. Seal the catalog / index control pages into fresh home runs.
        let mut frames: Vec<wal::Frame> = Vec::new();
        if let Some(bytes) = &ctx.cat_bytes {
            let (start, pages) = self.seal_run(bytes, &mut frames)?;
            self.header.catalog_start = start;
            self.header.catalog_pages = pages as u64;
            self.header.catalog_len = bytes.len() as u64;
        }
        if self.dirty_index {
            match &ctx.idx_bytes {
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
        // v8+ result-cache directory (Phase 10.1). Seal the serialized
        // directory into a fresh page run and update the pointer in the
        // header. The seal AAD already covers cache_start/pages/len for
        // v8+ (see `header_seal_aad`), so tampering with the pointer
        // trips the header seal on the next open.
        if self.dirty_cache {
            match &ctx.cache_bytes {
                Some(bytes) if !self.cache_dir.is_empty() => {
                    let (start, pages) = self.seal_run(bytes, &mut frames)?;
                    self.header.cache_start = start;
                    self.header.cache_pages = pages as u64;
                    self.header.cache_len = bytes.len() as u64;
                }
                _ => {
                    // Either the directory serialized to nothing (all
                    // entries deleted after tombstone-only migration) or
                    // it's empty — collapse the pointer to zero.
                    self.header.cache_start = 0;
                    self.header.cache_pages = 0;
                    self.header.cache_len = 0;
                }
            }
        }

        // 3b. Record this transaction as a restorable snapshot. The manifest
        //     update is staged in a local and adopted only once the commit
        //     below succeeds, so a failed flush leaves no phantom entry.
        //
        //     The cap (`self.max_snapshots`) is applied to the staged local
        //     before the new entry lands, so after appending the manifest
        //     holds at most `max_snapshots` entries — the most-recent N.
        //     `max_snapshots == 0` disables the cap. Pages referenced only
        //     by pruned snapshots stay on disk until `compact_file`.
        //
        //     FIX-1 R-001.8 part 3: skip the snapshot push on a
        //     cache-only flush. Snapshots don't capture the cache
        //     anyway, and every push counts against `max_snapshots`
        //     (default 256) + rewrites the manifest run — so a
        //     cache-heavy workload would otherwise evict every real
        //     memory restore point within ~256 cache writes. On a
        //     cache-only flush we reuse the current manifest
        //     verbatim; the manifest run already lives at
        //     `self.header.manifest_{start,pages,len}` (set by the
        //     last memory-touching flush, or at `create` time).
        let live = self.len() as u64;
        let txn_id = self.header.wal_seq + 1;
        let mut manifest = self.manifest.clone();
        if is_memory_or_index_flush {
            if self.max_snapshots > 0 {
                // Drop the oldest entries so there's room for the new one.
                while manifest.len() >= self.max_snapshots {
                    manifest.remove(0);
                }
            }
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
        }

        // 3c. The header is the transaction's final frame; stamp the new id
        //     and the *actual* (post-flush) write counter, which is <= the
        //     leased value persisted in `prepare_for_flush`. This is the
        //     value that ultimately replaces the leased header on checkpoint.
        self.header.vector_count = live;
        self.header.write_counter = self.pager.write_counter;
        self.header.wal_seq = txn_id;
        let mut hpage = self.header.to_page();
        self.header.apply_seal(&mut hpage, self.pager.dek())?;
        frames.push((0, hpage.to_vec()));

        // 4. COMMIT — log the transaction and fsync. Nothing at a home page
        //    other than the leased header has changed yet; this single
        //    fsync is what makes the rest durable.
        let (wal_start, wal_pages) = (self.header.wal_start, self.header.wal_pages);
        wal::commit(self.pager.file_mut(), wal_start, wal_pages, txn_id, &frames)?;

        // 5. Checkpoint — fold the WAL into the home pages.
        self.checkpoint(&frames)?;

        self.manifest = manifest;
        self.dirty_catalog = false;
        self.dirty_index = false;
        self.dirty_cache = false;
        // Batched policy: successful flush consumes the pending batch.
        self.cache_batch.reset();
        // Record how many fsyncs this flush issued, for Stats reporting.
        // Thread-local counter guarantees we see only this handle's own
        // sync_all calls between the snapshot above and this read.
        self.fsyncs_last_flush =
            (crate::fsync_count().wrapping_sub(fsync_before)).min(u32::MAX as u64) as u32;
        Ok(())
    }

    /// Pre-flush prelude shared by [`Mnemo::flush`] and the
    /// `__crash_partial_flush_for_testing` hook.
    ///
    /// Serializes the control plane (catalog, ANN index, cache directory)
    /// and grows the WAL if needed. That's it: this used to persist a
    /// leased header too (SEC-1 obsoleted it; see DUR-1 R-002.1 note in
    /// the body for the full rationale), but after DUR-1 the only page-0
    /// writes outside `create` go through the WAL via
    /// [`Mnemo::relocate_header_through_wal`].
    ///
    /// The historical pre-stamping used to close the nonce-reuse window
    /// flagged in Phase 1.1 of the improvement plan: a crash anywhere
    /// between here and the WAL commit left a file whose on-disk header
    /// recorded a `write_counter` past every nonce the rest of this
    /// transaction could produce, so the next reopen started from a
    /// counter that guaranteed uniqueness against any orphan data pages
    /// still on disk. SEC-1's per-session random nonce prefix gives the
    /// same guarantee without needing a persisted counter, which is why
    /// this routine no longer writes page 0.
    fn prepare_for_flush(&mut self) -> Result<FlushPrelude> {
        // Serialize.
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
                Some(ann) => {
                    Some(rmp_serde::to_vec(ann).map_err(|e| MnemoError::Serialize(e.to_string()))?)
                }
                None => None,
            }
        } else {
            None
        };
        // Serialize the result-cache directory when it's dirty. An
        // empty directory serializes to a small header + 0-element
        // array; still cheap enough that we don't special-case it.
        let cache_bytes: Option<Vec<u8>> = if self.dirty_cache {
            Some(
                rmp_serde::to_vec(&self.cache_dir)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?,
            )
        } else {
            None
        };

        let cat_pc = cat_bytes
            .as_ref()
            .map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        let idx_pc = idx_bytes
            .as_ref()
            .map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        let cache_pc = cache_bytes
            .as_ref()
            .map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        // Upper bound on the manifest run: one extra entry, <=82 bytes each
        // (rmp_serde encodes a Snapshot as a 9-element fixarray of i64/u64,
        // worst case 81 bytes plus a 1-byte array header).
        let man_upper = 9 + (self.manifest.len() + 1) * 82;
        let man_pc_est = man_upper.div_ceil(PAYLOAD).max(1);

        // Grow WAL if needed (DUR-1: relocation is WAL-logged and does
        // its own fsyncs). The cache directory rides in the same
        // transaction so its page count contributes to the reservation.
        self.ensure_wal_capacity(cat_pc + idx_pc + cache_pc + man_pc_est + 1)?;

        // DUR-1 (R-002.1): the pre-flush "leased header" write that used
        // to live here is gone. It existed to close the Phase 1.1 nonce
        // reuse window (reopen must see a `write_counter` past any nonce
        // this flush could produce before data pages hit disk). SEC-1
        // superseded that: nonces now carry a per-session random prefix
        // and `write_counter` is no longer a nonce input, so the leased
        // write bought us nothing but an extra fsync per flush. Orphan
        // data pages past `header.next_page` on a crashed flush are
        // still safe — the next session's random prefix makes any later
        // write at the same page_no uniquely nonced regardless of
        // `write_counter`.

        Ok(FlushPrelude {
            cat_bytes,
            idx_bytes,
            cache_bytes,
        })
    }

    /// Flush and close. Equivalent to `flush()` on a read-write handle; on
    /// a read-only handle it's a no-op (no dirty state to persist). The
    /// file is released on drop, which also drops the OS advisory lock.
    pub fn close(&mut self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        self.flush()
    }

    /// **TEST ONLY — do not use.** Reproduces a crash inside
    /// [`Mnemo::flush`] *after* the prelude has leased counter+page slots
    /// and persisted the leased header, but *before* the WAL is committed.
    /// Runs `prepare_for_flush` + `pager.flush` and returns — leaving
    /// data pages and a leased header on disk, with no commit frame.
    ///
    /// Used by `tests/integration.rs::nonce_unique_after_crashed_data_flush`
    /// to verify that this window (Phase 1.1 of the improvement plan) does
    /// not enable AES-GCM nonce reuse. Calling this in production code
    /// strands data pages; never expose it from a binding.
    #[doc(hidden)]
    pub fn __crash_partial_flush_for_testing(&mut self) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if !self.dirty_catalog && !self.dirty_index {
            return Ok(());
        }
        let _prelude = self.prepare_for_flush()?;
        self.pager.flush()
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

    /// Page-cache occupancy: `(pages_cached, capacity)`. Renamed from
    /// `cache_stats` in v0.4.0: the result-cache API (Phase 10.1) took
    /// that name for its per-namespace [`CacheStats`] summary, and the
    /// page-cache pair is the older/less-user-facing of the two, so it
    /// gets the more-specific name.
    pub fn page_cache_stats(&self) -> (usize, usize) {
        self.pager.cache_stats()
    }

    // === Result cache (Phase 10.1 + 10.3) ============================

    /// Insert or overwrite a cache entry at `(namespace, key)`.
    ///
    /// The engine SHA-256-hashes `key` — callers pass the raw input
    /// string. Persists to disk on the next `flush()` under the current
    /// flush policy (see [`Mnemo::set_cache_flush_policy`]); in
    /// [`CacheFlushPolicy::Batched`] the engine may call `flush()`
    /// internally when a threshold trips.
    ///
    /// On overflow of the namespace's [`CacheBudget`] (default 10,000
    /// entries / 64 MiB) the LRU entry is tombstoned before the new
    /// insert. Tombstones are reclaimed by
    /// [`Mnemo::compact_file`], not the runtime flush.
    ///
    /// Returns [`MnemoError::ReadOnly`] on a read-only handle.
    pub fn cache_put(
        &mut self,
        namespace: &str,
        key: &str,
        value: &[u8],
        opts: CachePutOpts,
    ) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let key_hash = result_cache::hash_key(key);
        let key_preview = result_cache::key_preview(key);
        let now = memory::now_secs();
        let size_bytes = value.len() as u64;

        let entry = CacheEntry {
            namespace: namespace.to_string(),
            key_hash,
            key_preview: key_preview.clone(),
            value: value.to_vec(),
            content_type: opts.content_type.clone(),
            created_at: now,
            accessed_at: now,
            access_count: 0,
            ttl_secs: opts.ttl_secs,
            cost_hint_ms: opts.cost_hint_ms,
            size_bytes,
            vector: None,
            model: None,
        };
        let bytes = rmp_serde::to_vec(&entry).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (start_page, page_count) = self.write_record(&bytes)?;

        // Tombstone any prior entry for this key. Its pages remain on
        // disk until `compact_file`, exactly like memory-record
        // overwrites — the tombstone just hides it from lookups.
        if let Some(prev_pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[prev_pos].deleted = true;
            self.cache_idx.remove(namespace, &key_hash);
        }

        let dir_entry = CacheDirectoryEntry {
            namespace: namespace.to_string(),
            key_hash,
            start_page,
            page_count,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: now,
            access_count: 0,
            size_bytes,
            ttl_secs: opts.ttl_secs,
            created_at: now,
            vector: None,
            model: None,
        };
        let pos = self.cache_dir.len();
        self.cache_dir.push(dir_entry);
        self.cache_idx.insert(namespace, key_hash, pos);

        // Enforce budget — evict LRU tombstones until the namespace
        // fits under both caps.
        self.evict_to_budget(namespace)?;

        self.dirty_cache = true;
        self.cache_batch.record_dirty();

        // Batched policy: auto-flush if a threshold tripped. Successful
        // `flush()` calls `cache_batch.reset()` for us.
        if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        Ok(())
    }

    /// Look up `(namespace, key)`. Returns `None` on miss (either the
    /// key was never inserted or the entry is TTL-expired). On hit
    /// updates catalog-side access stats (mirrors the recall v5-trick);
    /// on a read-only handle the stats update is silently skipped.
    ///
    /// A miss is recorded as a miss in this handle's counters even if
    /// the key was TTL-expired — from the caller's perspective the two
    /// are indistinguishable.
    pub fn cache_get(&mut self, namespace: &str, key: &str) -> Result<Option<CachedValue>> {
        // FIX-1 R-001.8 part 4: under `Batched`, honor `max_age` on
        // reads too. Pre-FIX-1 the age check only fired on `cache_put`,
        // so a workload that wrote a few entries and then switched to
        // pure reads could sit with its pending batch past the
        // nominal max_age indefinitely. `max_dirty` is already
        // maintained per-write; this adds the symmetric time-bound
        // check on the read path. Only runs on read-write handles —
        // flush() returns ReadOnly for shared-lock readers.
        if !self.read_only && self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        let key_hash = result_cache::hash_key(key);
        let pos = match self.cache_idx.get(namespace, &key_hash) {
            Some(p) => p,
            None => {
                self.cache_counters.misses += 1;
                return Ok(None);
            }
        };
        let now = memory::now_secs();
        // Snapshot the directory entry fields we need before any
        // mutation — the borrow checker gets grumpy otherwise.
        let (start_page, page_count, len, ttl_secs, expired, created_at) = {
            let e = &self.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.ttl_secs,
                e.is_expired(now),
                e.created_at,
            )
        };
        if expired {
            // Treat expired as a miss for the caller. Don't tombstone
            // here — cache_purge does that. Just skip.
            self.cache_counters.misses += 1;
            return Ok(None);
        }
        // Decrypt and decode the record body.
        let mut buf = Vec::with_capacity(len as usize);
        for i in 0..page_count as u64 {
            buf.extend_from_slice(&self.pager.read_page(start_page + i)?);
        }
        buf.truncate(len as usize);
        let entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;

        // Bump access stats catalog-only (v5 trick — no full record
        // rewrite on a hit). Skipped on read-only handles so a shared-lock
        // reader can't dirty the catalog.
        if !self.read_only {
            self.cache_dir[pos].accessed_at = now;
            self.cache_dir[pos].access_count = self.cache_dir[pos].access_count.saturating_add(1);
            self.dirty_cache = true;
        }
        self.cache_counters.hits += 1;
        // On a mutating handle: access_count reflects the just-bumped
        // value. On a read-only handle: the on-disk directory value is
        // unchanged, so returning it verbatim is honest (the caller's
        // reads did not, and cannot, alter the persisted counter).
        //
        // FIX-1 R-002.2 part 1: `accessed_at` must also come from the
        // directory on a read-only handle, not from `entry.accessed_at`
        // in the record body. The record body is written once at insert
        // time and never updated on a hit — every live `accessed_at`
        // update lands in the directory (see the `!self.read_only`
        // branch above). The pre-FIX-1 code returned
        // `entry.accessed_at`, which is really the entry's **creation
        // time** by then, so a read-only reader saw "last accessed =
        // when it was inserted" even after many successful hits by
        // concurrent writers.
        let access_count = self.cache_dir[pos].access_count;
        let accessed_at = if self.read_only {
            self.cache_dir[pos].accessed_at
        } else {
            now
        };
        Ok(Some(CachedValue {
            value: entry.value,
            content_type: entry.content_type,
            created_at,
            accessed_at,
            access_count,
            ttl_secs,
        }))
    }

    /// Tombstone the entry at `(namespace, key)`. Silently succeeds if
    /// no such entry exists. Space is reclaimed by `compact_file`.
    pub fn cache_delete(&mut self, namespace: &str, key: &str) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let key_hash = result_cache::hash_key(key);
        if let Some(pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[pos].deleted = true;
            self.cache_idx.remove(namespace, &key_hash);
            self.dirty_cache = true;
            self.cache_batch.record_dirty();
            if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Purge cache entries. If `namespace` is `Some`, restrict to that
    /// namespace; otherwise purge across the whole cache. If
    /// `expired_only` is `true`, only TTL-expired entries are removed;
    /// otherwise every live entry in the target scope is tombstoned.
    /// Returns the number of entries newly tombstoned.
    pub fn cache_purge(&mut self, namespace: Option<&str>, expired_only: bool) -> Result<usize> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = memory::now_secs();
        let mut purged = 0usize;
        // Iterate the directory by index; mark matching entries as
        // deleted and pull them out of the in-memory index.
        for pos in 0..self.cache_dir.len() {
            if self.cache_dir[pos].deleted {
                continue;
            }
            if let Some(ns) = namespace {
                if self.cache_dir[pos].namespace != ns {
                    continue;
                }
            }
            if expired_only && !self.cache_dir[pos].is_expired(now) {
                continue;
            }
            let key_hash = self.cache_dir[pos].key_hash;
            let ns = self.cache_dir[pos].namespace.clone();
            self.cache_dir[pos].deleted = true;
            self.cache_idx.remove(&ns, &key_hash);
            purged += 1;
        }
        if purged > 0 {
            self.dirty_cache = true;
            self.cache_batch.record_dirty();
            if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
                self.flush()?;
            }
        }
        Ok(purged)
    }

    /// Statistics for `namespace` (or the whole cache if `None`).
    /// `hits`/`misses`/`evictions` are per-handle in-memory counters
    /// that reset on reopen; `entries`/`bytes` reflect the durable
    /// on-disk state (live entries only, tombstones excluded).
    pub fn cache_stats(&self, namespace: Option<&str>) -> CacheStats {
        let mut entries = 0usize;
        let mut bytes = 0u64;
        for e in &self.cache_dir {
            if e.deleted {
                continue;
            }
            if let Some(ns) = namespace {
                if e.namespace != ns {
                    continue;
                }
            }
            entries += 1;
            bytes += e.size_bytes;
        }
        let hits = self.cache_counters.hits;
        let misses = self.cache_counters.misses;
        let total = hits + misses;
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };
        CacheStats {
            entries,
            bytes,
            hits,
            misses,
            hit_rate,
            evictions: self.cache_counters.evictions,
        }
    }

    /// Set (or update) the eviction budget for `namespace`. Applies to
    /// subsequent `cache_put` calls; does not retroactively evict
    /// entries that already sit under a looser cap.
    pub fn set_cache_budget(&mut self, namespace: &str, budget: CacheBudget) {
        self.cache_budgets.insert(namespace.to_string(), budget);
    }

    /// Set the per-handle cache flush policy. Defaults to
    /// [`CacheFlushPolicy::Strict`]. Memory writes are never batched;
    /// this affects only cache mutations.
    pub fn set_cache_flush_policy(&mut self, policy: CacheFlushPolicy) {
        self.cache_flush_policy = policy;
        // Switching mid-run: if we're moving to Strict and the batch is
        // non-empty, the pending count still counts toward dirty_cache
        // which the next flush() will consume.
    }

    /// Evict LRU live entries in `namespace` until it fits under both
    /// budget caps. Called from `cache_put` after the insert; the new
    /// entry is included in the count so we always leave room.
    fn evict_to_budget(&mut self, namespace: &str) -> Result<()> {
        let budget = self
            .cache_budgets
            .get(namespace)
            .copied()
            .unwrap_or_default();
        loop {
            // Sum current live count + bytes in the namespace.
            let mut live_count = 0usize;
            let mut live_bytes = 0u64;
            for e in &self.cache_dir {
                if !e.deleted && e.namespace == namespace {
                    live_count += 1;
                    live_bytes += e.size_bytes;
                }
            }
            if live_count <= budget.max_entries && live_bytes <= budget.max_bytes {
                return Ok(());
            }
            // Find the LRU (oldest accessed_at) live entry.
            let victim_pos = self
                .cache_dir
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.deleted && e.namespace == namespace)
                .min_by_key(|(_, e)| e.accessed_at)
                .map(|(i, _)| i);
            let Some(pos) = victim_pos else {
                // Nothing to evict but still over budget — the single
                // just-inserted entry itself is bigger than
                // max_bytes. Nothing we can safely do here without
                // rejecting the put; leave it and return.
                return Ok(());
            };
            let key_hash = self.cache_dir[pos].key_hash;
            let ns = self.cache_dir[pos].namespace.clone();
            self.cache_dir[pos].deleted = true;
            self.cache_idx.remove(&ns, &key_hash);
            self.cache_counters.evictions += 1;
        }
    }

    // === Semantic cache (Phase 10.2) ==================================

    /// Insert or overwrite a **semantic** cache entry — same as
    /// [`Mnemo::cache_put`] but with a required embedding vector and
    /// model identifier. The vector is stored on the directory entry
    /// so [`Mnemo::cache_get_semantic`] can score without decrypting
    /// record bodies.
    ///
    /// Vector dimensionality is validated against the database's
    /// configured `dimensions` — a mismatch returns
    /// [`MnemoError::DimensionMismatch`], the same signal `remember`
    /// gives for the memory catalog.
    ///
    /// The `model` string is matched **exactly** on lookup — a
    /// different model's cache is invisible to a given query. Pick
    /// canonical identifiers (`"text-embedding-3-small"`,
    /// `"bge-large-en-v1.5"`, ...) and stick to them.
    pub fn cache_put_semantic(
        &mut self,
        namespace: &str,
        key: &str,
        vector: Vec<f32>,
        value: &[u8],
        opts: SemanticCachePutOpts,
    ) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let key_hash = result_cache::hash_key(key);
        let key_preview = result_cache::key_preview(key);
        let now = memory::now_secs();
        let size_bytes = value.len() as u64;

        let entry = CacheEntry {
            namespace: namespace.to_string(),
            key_hash,
            key_preview: key_preview.clone(),
            value: value.to_vec(),
            content_type: opts.content_type.clone(),
            created_at: now,
            accessed_at: now,
            access_count: 0,
            ttl_secs: opts.ttl_secs,
            cost_hint_ms: opts.cost_hint_ms,
            size_bytes,
            vector: Some(vector.clone()),
            model: Some(opts.model.clone()),
        };
        let bytes = rmp_serde::to_vec(&entry).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (start_page, page_count) = self.write_record(&bytes)?;

        if let Some(prev_pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[prev_pos].deleted = true;
            self.cache_idx.remove(namespace, &key_hash);
        }

        let dir_entry = CacheDirectoryEntry {
            namespace: namespace.to_string(),
            key_hash,
            start_page,
            page_count,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: now,
            access_count: 0,
            size_bytes,
            ttl_secs: opts.ttl_secs,
            created_at: now,
            vector: Some(vector),
            model: Some(opts.model),
        };
        let pos = self.cache_dir.len();
        self.cache_dir.push(dir_entry);
        self.cache_idx.insert(namespace, key_hash, pos);

        self.evict_to_budget(namespace)?;

        self.dirty_cache = true;
        self.cache_batch.record_dirty();
        if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        Ok(())
    }

    /// Semantic recall over the cache: score every live entry in
    /// `namespace` whose `model` matches against `query_vector` using
    /// cosine similarity; return the top-1 hit and its similarity iff
    /// `sim >= threshold`. `None` otherwise.
    ///
    /// Uses [`crate::result_cache::DEFAULT_SEMANTIC_THRESHOLD`] if you
    /// want a sensible default — pass it explicitly (`0.97` today).
    ///
    /// Exact-key entries (those without a vector) are transparently
    /// skipped, as are entries whose model doesn't match — a
    /// different-model hit would be a semantic bug, not a win.
    /// TTL-expired entries are also skipped.
    ///
    /// Access-stat updates on hit follow the same policy as
    /// [`Mnemo::cache_get`]: bumped on read-write handles, silently
    /// skipped on read-only. Hits and misses are recorded in the
    /// per-handle counters just like exact-key lookups.
    pub fn cache_get_semantic(
        &mut self,
        namespace: &str,
        query_vector: &[f32],
        threshold: f32,
        model: &str,
    ) -> Result<Option<(CachedValue, f32)>> {
        if query_vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: query_vector.len(),
            });
        }
        // FIX-1 R-001.8 part 4: same Batched max_age check as
        // `cache_get`. See the comment there.
        if !self.read_only && self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        let now = memory::now_secs();

        // Scan namespace live entries, filter by model, compute
        // cosine, keep the best. Exact-key entries have `vector: None`
        // and are skipped by the filter.
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.cache_dir.iter().enumerate() {
            if e.deleted || e.namespace != namespace || e.is_expired(now) {
                continue;
            }
            let Some(ref v) = e.vector else { continue };
            let Some(ref m) = e.model else { continue };
            if m != model {
                continue;
            }
            let sim = memory::cosine(query_vector, v);
            if best.map(|(_, s)| sim > s).unwrap_or(true) {
                best = Some((i, sim));
            }
        }

        let Some((pos, sim)) = best else {
            self.cache_counters.misses += 1;
            return Ok(None);
        };
        if sim < threshold {
            self.cache_counters.misses += 1;
            return Ok(None);
        }

        // Extract fields before any mutation to keep the borrow checker
        // happy.
        let (start_page, page_count, len, ttl_secs, created_at) = {
            let e = &self.cache_dir[pos];
            (e.start_page, e.page_count, e.len, e.ttl_secs, e.created_at)
        };

        // Decrypt and decode the record body.
        let mut buf = Vec::with_capacity(len as usize);
        for i in 0..page_count as u64 {
            buf.extend_from_slice(&self.pager.read_page(start_page + i)?);
        }
        buf.truncate(len as usize);
        let entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;

        if !self.read_only {
            self.cache_dir[pos].accessed_at = now;
            self.cache_dir[pos].access_count = self.cache_dir[pos].access_count.saturating_add(1);
            self.dirty_cache = true;
        }
        self.cache_counters.hits += 1;
        let access_count = self.cache_dir[pos].access_count;
        // FIX-1 R-002.2 part 1: same read-only `accessed_at` fix as
        // `cache_get`. The directory carries the live value; the record
        // body only holds the insert-time snapshot.
        let accessed_at = if self.read_only {
            self.cache_dir[pos].accessed_at
        } else {
            now
        };
        Ok(Some((
            CachedValue {
                value: entry.value,
                content_type: entry.content_type,
                created_at,
                accessed_at,
                access_count,
                ttl_secs,
            },
            sim,
        )))
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
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
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
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
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

    /// Return the KDF (Argon2id) parameters this file currently uses to
    /// wrap its DEK. Needed by callers that want to [`rekey`] while
    /// keeping the file's original protection level (e.g. the Python
    /// binding's `rekey(fast=False)`, whose docstring promises exactly
    /// that). FIX-1 R-001.5.
    pub fn kdf_params(&self) -> KdfParams {
        self.kdf
    }

    /// Flush any pending writes, then stream the (encrypted) file to
    /// `dest` atomically: writes to a sibling temp file in `dest`'s
    /// parent directory, `sync_all`s it, renames onto `dest`, and
    /// fsyncs the parent directory on Unix so a crash never leaves a
    /// partial export.
    ///
    /// Refuses when `dest` resolves to the file this handle has open
    /// — that case would otherwise `File::create`-truncate the live
    /// database to 0 bytes (`fs::copy` on Unix reads *after* the
    /// `O_TRUNC`, so a naive export-to-self destroys the source).
    /// Unix: compares `(dev, ino)` of both paths via `MetadataExt`,
    /// catching symlinks and hard links. Windows: compares
    /// canonicalized paths; a rename onto an open file also fails on
    /// Windows because of the mandatory lock the handle holds, so the
    /// rename step is a second line of defense.
    ///
    /// Reads the bytes from the already-open file handle, not by
    /// `fs::copy`. On Windows that matters: `LockFileEx` locks are
    /// mandatory, so a second handle opened by `fs::copy` would fail
    /// while this writer still holds the lock.
    ///
    /// FIX-1 R-001.5 + R-002.2 part 3.
    pub fn export_encrypted(&mut self, dest: impl AsRef<Path>) -> Result<()> {
        let dest = dest.as_ref();

        // Step 1 — refuse export to the open file itself. We compare
        // by identity, not string, so symlinks and hard links to
        // `self.path` are caught too. The check runs before any flush
        // so a self-targeted export is a pure no-op.
        if paths_refer_to_same_file(&self.path, dest).unwrap_or(false) {
            return Err(MnemoError::Invalid(format!(
                "export_encrypted refuses to overwrite the open database at {}",
                dest.display()
            )));
        }

        // Step 2 — flush, so on-disk bytes match the live state.
        if !self.read_only {
            self.flush()?;
        }

        // Step 3 — stream from the open handle into a sibling temp.
        // Using the open handle (instead of a second open on `self.path`)
        // means a Windows mandatory lock can't block the copy.
        let parent = dest.parent().ok_or_else(|| {
            MnemoError::Invalid(format!(
                "export_encrypted: dest {} has no parent directory",
                dest.display()
            ))
        })?;
        let tmp = temp_sibling(dest);
        // Scope the temp handle so it closes before the rename on
        // Windows (which refuses to rename over an open file).
        {
            let src = self.pager.file_mut();
            src.seek(SeekFrom::Start(0))?;
            let mut out = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            let mut buf = vec![0u8; 1 << 20]; // 1 MiB
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
            }
            out.sync_all()?;
            crate::bump_fsync();
        }

        // Step 4 — atomic rename onto `dest`.
        if let Err(e) = std::fs::rename(&tmp, dest) {
            // Best-effort cleanup; propagate the rename error.
            let _ = std::fs::remove_file(&tmp);
            return Err(MnemoError::Io(e));
        }

        // Step 5 — fsync the parent directory on Unix so the rename
        // itself is durable. On Windows directory syncs aren't
        // supported; the rename above is atomic enough on NTFS.
        #[cfg(unix)]
        {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
                crate::bump_fsync();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = parent;
        }

        Ok(())
    }

    /// Change the passphrase. Cheap: re-derives the KEK and re-wraps the DEK;
    /// the encrypted pages are never rewritten.
    pub fn rekey(&mut self, new_passphrase: &str, kdf: KdfParams) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
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
        self.kdf = kdf;

        // DUR-1 (R-002.1): WAL-log the new page 0 before touching it.
        // Allocate a fresh WAL region at the tail so that a crash during
        // the rekey leaves either the old region (old passphrase works)
        // or the new region (new passphrase works) fully committed —
        // overwriting the existing region would risk destroying both.
        let old_wal_start = self.header.wal_start;
        let new_pages = self.header.wal_pages;
        let new_start = self.header.next_page;
        self.header.next_page += new_pages;
        self.header.wal_start = new_start;
        self.header.wal_pages = new_pages;
        self.header.wal_seq += 1;
        self.relocate_header_through_wal(old_wal_start)
    }

    /// Decrypt and validate every live record. Returns the count verified
    /// across memories **and** cache entries (FIX-1 R-001.1: now that the
    /// cache survives compaction, `verify()` has to walk it too — the
    /// pre-FIX-1 implementation only checked `self.catalog`, so a
    /// corrupted cache record went unnoticed).
    pub fn verify(&mut self) -> Result<usize> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut n = entries.len();
        for e in &entries {
            let bytes = self.read_record(e)?;
            let _m: Memory = rmp_serde::from_slice(&bytes)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
        }

        // Also decode every live cache entry. Tombstoned entries are
        // skipped — their pages are orphan ciphertext awaiting
        // `compact_file`, same as deleted memories. Expired entries
        // are *still* on disk until compaction, so we verify them too.
        let cache_positions: Vec<(u64, u32, u32)> = self
            .cache_dir
            .iter()
            .filter(|e| !e.deleted)
            .map(|e| (e.start_page, e.page_count, e.len))
            .collect();
        for (start_page, page_count, len) in cache_positions {
            let mut buf = Vec::with_capacity(len as usize);
            for i in 0..page_count as u64 {
                buf.extend_from_slice(&self.pager.read_page(start_page + i)?);
            }
            buf.truncate(len as usize);
            let _c: CacheEntry = rmp_serde::from_slice(&buf)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
            n += 1;
        }
        Ok(n)
    }

    /// Summary statistics for the open database.
    pub fn stats(&mut self) -> Result<Stats> {
        let deleted = self.catalog.iter().filter(|e| e.deleted).count();
        // FIX-1 R-001.8 part 2: build the agent set lazily on first
        // call per handle, then maintain in `put` / invalidate in
        // `delete`. Pre-FIX-1 every `stats()` call decrypted every
        // live memory (O(N) AES-GCM decrypts just to list a usually-
        // tiny set of distinct agent IDs). On a 1M-memory database
        // this was in the multi-second range on hardware AES.
        if self.agent_set.is_none() {
            let built: std::collections::BTreeSet<String> =
                self.memories()?.into_iter().map(|m| m.agent_id).collect();
            self.agent_set = Some(built);
        }
        let agents: Vec<String> = self
            .agent_set
            .as_ref()
            .expect("agent_set just filled above")
            .iter()
            .cloned()
            .collect();
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
            fsyncs_last_flush: self.fsyncs_last_flush,
        })
    }

    /// Rewrite the file, dropping tombstoned and expired memories and
    /// reclaiming stale pages left by updates. Done out-of-place via a temp
    /// file and an atomic rename.
    ///
    /// FIX-1 R-001.1 + R-002.2 part 2: the result cache now survives
    /// compaction. Every live, non-expired directory entry is copied
    /// into the new file verbatim — `created_at`, `ttl_secs`,
    /// `cost_hint_ms`, `key_preview`, `size_bytes`, `content_type`,
    /// `vector` and `model` come from the record body; `accessed_at`
    /// and `access_count` come from the *directory* (the record body
    /// only holds the insert-time snapshot — the directory is the
    /// source of truth for the live counts, as of v5's recall trick).
    /// Tombstoned and TTL-expired entries are dropped. Pre-FIX-1
    /// compaction kept only the memory catalog and silently emptied
    /// the cache; the Planner's interim README caveat (c417d2e) called
    /// that out — this PR removes the surprise.
    pub fn compact_file(path: &str, passphrase: &str) -> Result<CompactReport> {
        let mut old = Mnemo::open(path, passphrase)?;
        let dims = old.dimensions;
        let kdf = old.kdf;
        let tmp = format!("{path}.compact-tmp");

        let mut new = Mnemo::create(
            &tmp,
            passphrase,
            MnemoConfig {
                dimensions: dims,
                kdf,
                ..Default::default()
            },
        )?;
        // ANN-1.1: compaction is "re-tune for the current size" by
        // default. Pass IndexConfig::default() so partitions,
        // subspaces, n_probe and n_rerank are all re-derived from the
        // compacted vector count. Previously we froze the pre-compact
        // (n_probe, n_rerank), which left the new index on stale
        // numbers for the smaller post-compact corpus.
        let want_index = old.ann.is_some();
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
        if want_index && after > 0 {
            new.build_index_with(IndexConfig::default())?;
        }

        // Count + copy cache entries. We walk `old.cache_dir` by index
        // so each hop reads the directory's live `accessed_at` /
        // `access_count` directly rather than going through
        // `cache_get` (which would also treat read-only semantics,
        // tombstones, etc. — we want the raw, authoritative values).
        let mut cache_before = 0usize;
        let mut cache_after = 0usize;
        let positions: Vec<usize> = (0..old.cache_dir.len()).collect();
        for pos in positions {
            let keep = {
                let e = &old.cache_dir[pos];
                !e.deleted && !e.is_expired(now)
            };
            if keep {
                cache_before += 1;
                new.copy_cache_entry_from(&mut old, pos)?;
                cache_after += 1;
            }
        }

        new.flush()?;
        drop(new);
        drop(old);

        std::fs::rename(&tmp, path)?;
        Ok(CompactReport {
            before,
            after,
            cache_before,
            cache_after,
        })
    }

    /// Copy one live cache entry from `old` at directory position
    /// `pos` into `self` (a freshly-created compaction target).
    /// Decrypts the old record body, substitutes the old *directory*'s
    /// `accessed_at` + `access_count` (the record body's copies are
    /// stale as of v5's directory-only hit path), re-encodes, writes
    /// a fresh record run, and pushes a matching directory entry.
    ///
    /// Caller is expected to have already filtered out tombstoned /
    /// expired entries and to call this on a read-write handle.
    pub(crate) fn copy_cache_entry_from(&mut self, old: &mut Mnemo, pos: usize) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // Snapshot the directory fields we need, so the borrow on
        // `old.cache_dir` ends before we touch `old.pager`.
        let (start_page, page_count, len, dir_accessed_at, dir_access_count) = {
            let e = &old.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.accessed_at,
                e.access_count,
            )
        };

        // Read + decrypt the old record body.
        let mut buf = Vec::with_capacity(len as usize);
        for i in 0..page_count as u64 {
            buf.extend_from_slice(&old.pager.read_page(start_page + i)?);
        }
        buf.truncate(len as usize);
        let mut entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;

        // Overwrite the stale body copies with the directory's live
        // values before persisting. Everything else — `created_at`,
        // `ttl_secs`, `cost_hint_ms`, `key_preview`, `size_bytes`,
        // `content_type`, `value`, `vector`, `model` — stays as the
        // body already recorded it.
        entry.accessed_at = dir_accessed_at;
        entry.access_count = dir_access_count;

        let bytes =
            rmp_serde::to_vec(&entry).map_err(|err| MnemoError::Serialize(err.to_string()))?;
        let (new_start_page, new_page_count) = self.write_record(&bytes)?;

        let dir_entry = CacheDirectoryEntry {
            namespace: entry.namespace.clone(),
            key_hash: entry.key_hash,
            start_page: new_start_page,
            page_count: new_page_count,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: dir_accessed_at,
            access_count: dir_access_count,
            size_bytes: entry.size_bytes,
            ttl_secs: entry.ttl_secs,
            created_at: entry.created_at,
            vector: entry.vector.clone(),
            model: entry.model.clone(),
        };
        let new_pos = self.cache_dir.len();
        self.cache_idx
            .insert(&entry.namespace, entry.key_hash, new_pos);
        self.cache_dir.push(dir_entry);
        self.dirty_cache = true;
        Ok(())
    }
}
