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
use crate::format::{
    Header, RunPtr, FLAG_ENCRYPTED, PAGE_SIZE, PAYLOAD, VERSION, VERSION_V9, WRAPPED_DEK_LEN,
};
use crate::freelist::FreeList;
use crate::index::{IndexConfig, IndexInfo, IndexOp, IvfPqIndex};
use crate::keyslot::{KeySlot as KeySlotEntry, KeySlotRun};
use crate::memory::{
    self, status as essence_status, CommitBatch, CommitReport, EssenceOp, EssenceOpKind,
    EssenceScope, EssenceSection, ExportedTurn, Filter, FilterTerm, IngestReport, KeyScope, Memory,
    MemoryPatch, MemoryType, Metric, Scope, ScoreWeights, ThreadTurn, TurnPolicy, WriteOutcome,
    CONTINUATION_TIMELINE_ENTRIES, ESSENCE_DEFAULT_RENDER_BYTES, ESSENCE_TRANSITIONS,
    MNEMO_META_CONTINUES, MNEMO_META_INVALID_AT, MNEMO_META_KEY, MNEMO_META_KIND,
    MNEMO_META_KIND_ESSENCE, MNEMO_META_KIND_THREAD_LINK, MNEMO_META_KIND_TURN, MNEMO_META_LENGTH,
    MNEMO_META_PART, MNEMO_META_REFS, MNEMO_META_ROLE, MNEMO_META_SECTION, MNEMO_META_SEQ,
    MNEMO_META_SHA256, MNEMO_META_STATUS, MNEMO_META_STATUS_HISTORY, MNEMO_META_SUPERSEDED_BY,
    MNEMO_META_SUPERSEDES, MNEMO_META_THREAD, MNEMO_META_TURN_ID, MNEMO_META_UPDATED_AT,
    THREAD_TOOL_RESULT_TRUNCATE_BYTES, THREAD_TURN_CHUNK_BYTES,
};
use crate::pager::Pager;
use crate::result_cache::{
    self, BatchState, CacheBudget, CacheDirectoryEntry, CacheEntry, CacheFlushPolicy, CachePutOpts,
    CacheStats, CachedValue, CounterState, DirectoryIndex, SemanticCachePutOpts,
};
use crate::text;
use crate::veccol;
use crate::wal;

mod ann;
mod cache_api;
mod continuation;
mod crud;
mod essence;
mod flush;
mod hooks;
mod keyed;
mod load;
mod maintenance;
mod open;
mod recall;
mod records;
mod snapshots;
mod threads;
use load::*;

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

/// PERF-1: the ANN index is derived data, so `flush` persists it only every
/// this many commits that changed it (or see
/// [`INDEX_PERSIST_EVERY_CHANGES`], [`Mnemo::flush_index`], [`Mnemo::close`]).
/// Between persists, only the catalog is rewritten.
pub const INDEX_PERSIST_EVERY_COMMITS: u32 = 64;

/// PERF-1: …or once this many vectors were added, changed or removed since
/// the last persist. Records written after the persisted index are
/// re-assigned when the index is next loaded, so this bounds that work.
pub const INDEX_PERSIST_EVERY_CHANGES: usize = 1_000;

/// FMT-9d: default weight ε of the normalised BM25 score in hybrid recall
/// (`relevance = similarity + ε · bm25 / max bm25`).
pub const DEFAULT_KEYWORD_WEIGHT: f32 = 0.5;

/// FMT-9b: default [`Mnemo::set_vector_cache_budget`]: the fp16 vector
/// column stays resident while it fits (100K × 384 dims is about 77 MB) and
/// is read through the page cache above it. PLANNER-A, 2026-11-07.
pub const DEFAULT_VECTOR_CACHE_BUDGET: usize = 96 * 1024 * 1024;

/// FMT-9b: below this many facts a v9 file has no ANN index: an exact
/// fp16 scan of 20K × 384 dims is about a millisecond, and the IVF+PQ
/// codebook alone would cost more disk than it saves time.
pub const V9_ANN_MIN_FACTS: usize = 20_000;

/// FMT-9f (PLANNER-A, 2026-11-08): on v9 an unpinned recall reranks at
/// least this many candidates per requested hit (500 for `top_k` 10), or
/// the index's own auto depth when that's deeper (above 100K). The v9
/// index is trained on fp16-rounded vectors, and at 20K–50K the auto
/// depth (×10–×25) lost up to 0.03 recall@10 against v8; ×50 is within
/// 0.005 at every measured tier. v8 is unchanged.
pub const V9_RERANK_PER_HIT: usize = 50;

/// FMT-9c: page runs longer than this are written copy-on-write straight
/// to their home pages (and fsynced) before the commit, instead of through
/// the WAL, on v9. The WAL then carries only the header and small runs, so
/// it stays at its 64 KiB default instead of growing to the largest
/// catalog or index (26 MB at 100K on v8). Safe for the same reason data
/// pages are: the pages come from the tail or from quarantined free runs,
/// so nothing committed references them until the header flips.
const V9_WAL_RUN_MAX_PAGES: u64 = 2;

/// FMT-9c: snapshot retention on v9: the newest 16 snapshots, plus any
/// from the last 24 hours, with retained snapshots never pinning more than
/// a quarter of the file (past that, the oldest go first, whatever their
/// age). `set_max_snapshots` changes the count.
pub const V9_DEFAULT_MAX_SNAPSHOTS: usize = 16;
const V9_SNAPSHOT_KEEP_SECS: i64 = 24 * 3600;
/// FMT-9c: the pin cap never drops below 128 pages (1 MiB).
const V9_PIN_FLOOR_PAGES: u64 = 128;

/// FMT-9d: the layout byte at the start of a v9 ANN index run (1: the
/// whole IVF+PQ index, followed by delta runs). A reader treats an unknown
/// layout as no index, so a later layout (partition-major, PERF-2) ships
/// without a format bump.
const V9_INDEX_LAYOUT: u8 = 1;

/// FMT-9b: how many sparse vector pages one commit folds into its new
/// pages (the vacuum). Bounded so a commit's cost stays bounded.
const VACUUM_PAGES_PER_COMMIT: usize = 4;

/// FMT-9c.2: and at most this many rows moved by the vacuum per commit (a
/// moved row is a changed catalog row, so it lands in the commit's delta).
/// Pages with more live rows than this stay as they are until deletes thin
/// them out.
const VACUUM_MAX_ROWS: usize = 64;

/// FMT-9b: header flag: the file also keeps each record's float vector
/// (`MnemoConfig::store_float_vectors`).
const FLAG_FLOAT_VECTORS: u32 = 2;

/// PERF-1: when the in-memory ANN index was last persisted, and whether the
/// next flush must persist it.
#[derive(Debug, Default, Clone, Copy)]
struct IndexPersist {
    /// Persist on the next flush (build, rebuild, drop, `flush_index`,
    /// `close`, snapshot restore).
    forced: bool,
    /// Commits since the last persist that left the index dirty.
    commits: u32,
    /// Vectors added, re-assigned or removed since the last persist.
    changes: usize,
}

/// R-004.2: where the thread layers and the onboarding briefing live,
/// built by the handle's one lazy scan (`ensure_key_caches`, together with
/// the 12.1 key caches) and then kept current by `put` and `delete`. It
/// lets `about`, `essence`, `ingest_thread`, `thread_export` and
/// thread-pinned recall read just their own records instead of decrypting
/// the whole file each time.
#[derive(Debug, Default)]
struct LayerCache {
    /// Live essence item ids.
    essence: std::collections::HashSet<u128>,
    /// Live ids tagged `metadata.area = "onboarding"` (superseded versions
    /// included; `about` drops those when it reads them).
    onboarding: std::collections::HashSet<u128>,
    /// Per `_mnemo.thread`: its records and archive state.
    threads: std::collections::HashMap<String, ThreadCache>,
    /// Thread → (the thread it continues, the link record's id).
    links: std::collections::HashMap<String, (String, u128)>,
    /// FMT-9e: on v9, thread-turn records the lazy scan didn't decode,
    /// by their row's `thread_hash`. A thread's bucket is decoded into
    /// `threads` the first time that thread is used
    /// ([`Mnemo::warm_thread`]), so a cold handle never decodes (or
    /// decompresses) the turns of threads it doesn't touch.
    cold_turns: std::collections::HashMap<u32, Vec<u128>>,
}

/// One thread's records (turns, essence items, link), the turn ids already
/// archived (with how many records each has: long turns are chunked), and
/// the highest `_mnemo.seq` seen.
#[derive(Debug, Default)]
struct ThreadCache {
    records: std::collections::HashSet<u128>,
    turns: std::collections::HashMap<String, u32>,
    max_seq: Option<u64>,
}

fn meta_str<'a>(m: &'a Memory, key: &str) -> Option<&'a str> {
    m.metadata.get(key).and_then(|v| v.as_str())
}

fn is_onboarding(m: &Memory) -> bool {
    meta_str(m, "area").is_some_and(|s| s.eq_ignore_ascii_case("onboarding"))
}

impl LayerCache {
    fn add(&mut self, m: &Memory) {
        let id = m.id.0;
        let kind = meta_str(m, MNEMO_META_KIND);
        if kind == Some(MNEMO_META_KIND_ESSENCE) {
            self.essence.insert(id);
        }
        if is_onboarding(m) {
            self.onboarding.insert(id);
        }
        let Some(thread) = meta_str(m, MNEMO_META_THREAD) else {
            return;
        };
        let tc = self.threads.entry(thread.to_string()).or_default();
        tc.records.insert(id);
        if kind == Some(MNEMO_META_KIND_TURN) {
            if let Some(turn_id) = meta_str(m, MNEMO_META_TURN_ID) {
                *tc.turns.entry(turn_id.to_string()).or_insert(0) += 1;
            }
            if let Some(seq) = m.metadata.get(MNEMO_META_SEQ).and_then(|v| v.as_u64()) {
                tc.max_seq = Some(tc.max_seq.map_or(seq, |s| s.max(seq)));
            }
        }
        if kind == Some(MNEMO_META_KIND_THREAD_LINK) {
            if let Some(from) = meta_str(m, MNEMO_META_CONTINUES) {
                self.links
                    .entry(thread.to_string())
                    .or_insert_with(|| (from.to_string(), id));
            }
        }
    }

    /// Undo [`LayerCache::add`] for the stored version of a record that is
    /// being overwritten or deleted. A thread with no records left is
    /// dropped, so a forgotten thread's `_mnemo.seq` restarts at 0. Partial
    /// deletes keep the old maximum (sequence gaps are harmless).
    fn remove(&mut self, m: &Memory) {
        let id = m.id.0;
        self.essence.remove(&id);
        self.onboarding.remove(&id);
        let Some(thread) = meta_str(m, MNEMO_META_THREAD) else {
            return;
        };
        if self.links.get(thread).is_some_and(|(_, link)| *link == id) {
            self.links.remove(thread);
        }
        let Some(tc) = self.threads.get_mut(thread) else {
            return;
        };
        tc.records.remove(&id);
        if meta_str(m, MNEMO_META_KIND) == Some(MNEMO_META_KIND_TURN) {
            if let Some(turn_id) = meta_str(m, MNEMO_META_TURN_ID) {
                if let Some(n) = tc.turns.get_mut(turn_id) {
                    *n -= 1;
                    if *n == 0 {
                        tc.turns.remove(turn_id);
                    }
                }
            }
        }
        if tc.records.is_empty() {
            self.threads.remove(thread);
        }
    }
}

/// The thread a recall filter pins with `_mnemo.thread` equality, if any.
fn filter_thread_pin(f: &Filter) -> Option<&str> {
    f.terms.iter().find_map(|t| match t {
        FilterTerm::Eq(k, Value::String(s)) if k == MNEMO_META_THREAD => Some(s.as_str()),
        _ => None,
    })
}

/// Whether a recall filter can only match records that aren't in the ANN
/// (R-004.2): it pins `_mnemo.thread`, or restricts `_mnemo.kind` to turns,
/// essence items or thread links. Such recalls scan exactly instead.
fn filter_bypasses_ann(f: &Filter) -> bool {
    let non_ann = |v: &Value| {
        matches!(
            v.as_str(),
            Some(MNEMO_META_KIND_TURN)
                | Some(MNEMO_META_KIND_ESSENCE)
                | Some(MNEMO_META_KIND_THREAD_LINK)
        )
    };
    filter_thread_pin(f).is_some()
        || f.terms.iter().any(|t| match t {
            FilterTerm::Eq(k, v) => k == MNEMO_META_KIND && non_ann(v),
            FilterTerm::In(k, vs) => {
                k == MNEMO_META_KIND && !vs.is_empty() && vs.iter().all(non_ann)
            }
            _ => false,
        })
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
    /// The on-disk format a new file is created in. `0` (the default)
    /// means v9, the current format. `8` creates a v8 file, for tools that
    /// still need to read it with v0.10 or earlier; it's kept for one
    /// release (v0.12 makes v8 read-only). **Not a stable API.**
    #[doc(hidden)]
    pub format_version: u16,
    /// FMT-9b (v9 files only; ignored for v8): also keep each memory's
    /// full-precision vector in its record, so `get()` and `recall()`
    /// return it exactly. Off by default: a v9 file stores vectors as
    /// fp16 (about 1e-3 relative error) in the vector column, which recall
    /// scores from either way. Costs `4 × dimensions` bytes per memory.
    pub store_float_vectors: bool,
}

impl Default for MnemoConfig {
    fn default() -> Self {
        Self {
            dimensions: 768,
            kdf: KdfParams::secure(),
            wal_pages_initial: DEFAULT_WAL_PAGES,
            max_snapshots: DEFAULT_MAX_SNAPSHOTS,
            format_version: 0,
            store_float_vectors: false,
        }
    }
}

/// FMT-9: resolve [`MnemoConfig::format_version`] for a new file.
fn resolve_format_version(requested: u16) -> Result<u16> {
    match requested {
        // FMT-9f: new files are v9; the `MNEMO_DEV_FORMAT` switch is gone.
        0 => Ok(crate::format::DEFAULT_NEW_VERSION),
        VERSION | VERSION_V9 => Ok(requested),
        v => Err(MnemoError::Invalid(format!(
            "format_version {v} is not supported for new files (use 8 or 9)"
        ))),
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
    /// FMT-9: the generation the record's pages were sealed under (v9 page
    /// AAD). `0`, and left out of the encoding, on v8 files, so a v8
    /// catalog is byte-for-byte what v0.10 writes.
    #[serde(default, skip_serializing_if = "crate::format::is_zero_u64")]
    gen: u64,
    /// FMT-9b: the v9 row's tags, scoring inputs and vector location
    /// (`None`, and left out of the encoding, on v8 files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v9: Option<RowV9>,
}

/// FMT-9b: what a v9 catalog row carries beyond the v8 fields, so recall
/// can filter and score a candidate without decrypting its record, and a
/// cold handle can select records by kind or thread.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
struct RowV9 {
    /// The vector column entry: page (`0` while it's waiting for the next
    /// flush), slot, and the page's generation.
    vec_page: u64,
    vec_slot: u16,
    vec_gen: u64,
    /// FMT-9b.2: the record's slot on its packed page (`start_page`), or
    /// [`REC_SPAN`] when the record owns a run of whole pages.
    rec_slot: u16,
    /// `ROW_KIND_*`.
    kind: u8,
    /// `ROW_SUPERSEDED | ROW_HAS_TTL | ROW_SHARED`.
    flags: u8,
    /// [`memory_type_code`].
    memory_type: u8,
    /// FNV-1a of `_mnemo.thread`, of the 12.1 key slot, and of `agent_id`
    /// (`0` = none). A collision only lets a non-match through to the
    /// decrypted check.
    thread_hash: u32,
    key_hash: u32,
    agent_hash: u32,
    created_at: i64,
    importance: f32,
    /// `created_at + ttl_secs` when `ROW_HAS_TTL` is set.
    expires_at: i64,
    /// FMT-9d: the row's BM25 document id (`0`: not keyword-indexed) and
    /// its token count. Live rows define which documents are live, and
    /// N and avgdl, so segments never hold lengths or tombstones.
    doc: u32,
    doc_len: u16,
}

/// FMT-9b.2: `RowV9::rec_slot` of a record too big for a packed page; it
/// owns `page_count` whole pages from `start_page`, as in v8.
const REC_SPAN: u16 = u16::MAX;
/// FMT-9b.2: `kind` byte of a packed record page.
const PAGE_KIND_RECORD: u8 = 1;
/// FMT-9b.2: bytes of a packed record page available to slots and data.
const REC_PAGE_CAP: usize = PAYLOAD - veccol::PAGE_HEADER;
/// FMT-9b.2: bytes per slot-directory entry (offset u16, length u16).
const REC_SLOT_BYTES: usize = 4;
/// FMT-9b.2: thread-archive chunk size on v9. A chunk plus its record's
/// metadata (~200 B) then fits one packed page (the FMT-9a measurement:
/// 8 KB chunks spill into a second page almost every time, 300 MB vs
/// 212 MB for the 50K-turn profile).
const V9_TURN_CHUNK_BYTES: usize = 7_900;

/// FMT-9e: the zstd frame magic. A v9 record body is either a MessagePack
/// `Memory` (which always starts with an array marker, `0x9_` or
/// `0xdc`/`0xdd`) or, for a compressed thread turn, a zstd frame, so the
/// first four bytes tell them apart without a row flag.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];
/// FMT-9e: zstd level for turn records (the FMT-9a measurement: 2.15× on
/// this repo's Markdown at level 3).
const TURN_ZSTD_LEVEL: i32 = 3;
/// FMT-9e: turn records shorter than this stay uncompressed (a frame's
/// overhead eats the gain on a one-line turn).
const TURN_ZSTD_MIN_BYTES: usize = 256;
/// FMT-9e: the largest decompressed record accepted. A turn record is a
/// chunk of at most [`V9_TURN_CHUNK_BYTES`] plus its metadata, so this is
/// a generous bound, not a size limit anyone meets; it stops a damaged
/// (but authenticated) frame from asking for an unbounded allocation.
const RECORD_PLAIN_MAX: u64 = 64 << 20;
/// FMT-9e: by default a thread turn is written plain and compressed once
/// it's a week old (by its `created_at`), so a live session's recent turns
/// read at full speed and archived history is small.
pub const DEFAULT_TURN_COMPRESS_AFTER_SECS: i64 = 7 * 24 * 3600;
/// FMT-9e: at most this many aged turns are compressed per commit, so a
/// commit's extra work stays bounded (like the vacuum).
const COLD_TURNS_PER_COMMIT: usize = 256;

/// FMT-9e: is turn-record compression on? Always, unless the dev switch
/// `MNEMO_DEV_TURN_ZSTD=0` turns it off for an A/B measurement (read once
/// per process). Reading never depends on it.
fn turn_zstd_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("MNEMO_DEV_TURN_ZSTD").as_deref() != Ok("0"))
}

/// FMT-9e: compress a v9 thread-turn record body, before the page
/// encryption. Each record is its own zstd frame with no dictionary and no
/// compression history from any other record, so how densely one record
/// compresses says nothing about another's content (design note (d)).
/// Kept only when it saves at least an eighth; otherwise the plain body is
/// returned.
fn compress_turn_record(bytes: Vec<u8>) -> Vec<u8> {
    if bytes.len() < TURN_ZSTD_MIN_BYTES || !turn_zstd_enabled() {
        return bytes;
    }
    let c = ZSTD_COMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = zstd::bulk::Compressor::new(TURN_ZSTD_LEVEL).ok();
        }
        let out = slot.as_mut().and_then(|z| z.compress(&bytes).ok());
        if out.is_none() {
            // Never reuse a context a call failed on.
            *slot = None;
        }
        out
    });
    match c {
        Some(c) if c.len() * 8 <= bytes.len() * 7 => c,
        _ => bytes,
    }
}

// FMT-9e: one zstd context of each kind per thread, made on first use and
// reused for every record (a fresh context per record made reads about 15%
// slower). Only the one-shot (`bulk`) calls use them, and each call starts
// a new frame from a reset session, so no state crosses records; a context
// a call failed on is dropped, not reused (cf. RUSTSEC-2026-0391, which
// affected the stream API's `with_context`; zstd 0.13.4 carries its fix).
// One-shot calls size their tables to the record, so a context stays small.
thread_local! {
    static ZSTD_COMPRESSOR: std::cell::RefCell<Option<zstd::bulk::Compressor<'static>>> =
        const { std::cell::RefCell::new(None) };
    static ZSTD_DECOMPRESSOR: std::cell::RefCell<Option<zstd::bulk::Decompressor<'static>>> =
        const { std::cell::RefCell::new(None) };
}

/// FMT-9e: the plain body of a v9 record: decompressed when it's a zstd
/// frame, borrowed as is otherwise. The frame must state its content size,
/// and that size must be at most [`RECORD_PLAIN_MAX`].
pub(crate) fn plain_record(bytes: &[u8]) -> Result<std::borrow::Cow<'_, [u8]>> {
    if !bytes.starts_with(&ZSTD_MAGIC) {
        return Ok(std::borrow::Cow::Borrowed(bytes));
    }
    let bad = |why: &str| MnemoError::Invalid(format!("compressed record: {why}"));
    let size = match zstd::zstd_safe::get_frame_content_size(bytes) {
        Ok(Some(n)) if n <= RECORD_PLAIN_MAX => n as usize,
        Ok(Some(_)) => return Err(bad("too large")),
        _ => return Err(bad("no content size")),
    };
    let out = ZSTD_DECOMPRESSOR.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = Some(zstd::bulk::Decompressor::new().map_err(|e| bad(&e.to_string()))?);
        }
        let out = match slot.as_mut() {
            Some(z) => z.decompress(bytes, size).map_err(|e| bad(&e.to_string())),
            None => Err(bad("no context")),
        };
        if out.is_err() {
            // Never reuse a context a call failed on.
            *slot = None;
        }
        out
    })?;
    if out.len() != size {
        return Err(bad("length mismatch"));
    }
    Ok(std::borrow::Cow::Owned(out))
}

/// FMT-9c.2: pick the sparse pages (`(page, live rows)`, in page order) one
/// commit's vacuum folds in: at most [`VACUUM_PAGES_PER_COMMIT`] pages and
/// [`VACUUM_MAX_ROWS`] rows.
fn vacuum_pick(sparse: Vec<(u64, usize)>) -> Vec<u64> {
    let mut rows = 0usize;
    let mut out = Vec::new();
    for (p, n) in sparse {
        if out.len() == VACUUM_PAGES_PER_COMMIT {
            break;
        }
        if rows + n > VACUUM_MAX_ROWS {
            continue;
        }
        rows += n;
        out.push(p);
    }
    out
}

/// FMT-9b.2: encode a packed record page: the header, the slot directory,
/// then the records in slot order.
fn encode_record_page(gen: u64, recs: &[&[u8]]) -> Vec<u8> {
    let dir = recs.len() * REC_SLOT_BYTES;
    let data: usize = recs.iter().map(|r| r.len()).sum();
    let mut out = Vec::with_capacity(veccol::PAGE_HEADER + dir + data);
    out.extend_from_slice(&[PAGE_KIND_RECORD, 0, 0, 0]);
    out.extend_from_slice(&(recs.len() as u16).to_le_bytes());
    out.extend_from_slice(&((dir + data) as u16).to_le_bytes());
    out.extend_from_slice(&gen.to_le_bytes());
    let mut off = veccol::PAGE_HEADER + dir;
    for r in recs {
        out.extend_from_slice(&(off as u16).to_le_bytes());
        out.extend_from_slice(&(r.len() as u16).to_le_bytes());
        off += r.len();
    }
    for r in recs {
        out.extend_from_slice(r);
    }
    out
}

/// FMT-9b.2: the bytes of slot `slot` of a packed record page.
fn record_slot(page: &[u8], slot: u16) -> Result<Vec<u8>> {
    let bad = || MnemoError::Invalid("malformed record page".into());
    if page.len() < veccol::PAGE_HEADER || page[0] != PAGE_KIND_RECORD {
        return Err(bad());
    }
    let n = u16::from_le_bytes([page[4], page[5]]);
    if slot >= n {
        return Err(bad());
    }
    let d = veccol::PAGE_HEADER + slot as usize * REC_SLOT_BYTES;
    let dir = page.get(d..d + REC_SLOT_BYTES).ok_or_else(bad)?;
    let off = u16::from_le_bytes([dir[0], dir[1]]) as usize;
    let len = u16::from_le_bytes([dir[2], dir[3]]) as usize;
    page.get(off..off + len).map(|b| b.to_vec()).ok_or_else(bad)
}

/// FMT-9d: the BM25 manifest run: tokenizer, the next document id, and the
/// segment runs (oldest first).
#[derive(Serialize, Deserialize, Clone, Debug)]
struct Bm25Manifest {
    tokenizer: u8,
    next_doc: u32,
    segments: Vec<(u64, u64, u64, u64)>,
    /// Documents each segment was written with (live or not since).
    #[serde(default)]
    seg_docs: Vec<u32>,
}

impl Default for Bm25Manifest {
    fn default() -> Self {
        Bm25Manifest {
            tokenizer: text::TOKENIZER_V1,
            next_doc: 1,
            segments: Vec::new(),
            seg_docs: Vec::new(),
        }
    }
}

/// FMT-9d: live documents: `doc -> (row, length)`, their count and mean
/// length. Rebuilt from the rows after any write.
#[derive(Debug, Default)]
struct DocStats {
    docs: std::collections::HashMap<u32, (usize, u16)>,
    avgdl: f32,
    /// FMT-9f: the sum of the live documents' lengths, so `put` and
    /// `delete` keep `avgdl` current without a rescan of the rows.
    total: u64,
}

impl DocStats {
    /// FMT-9f: drop document `doc` (if live) and keep `avgdl` current.
    fn remove(&mut self, doc: u32) {
        if let Some((_, len)) = self.docs.remove(&doc) {
            self.total = self.total.saturating_sub(len as u64);
            self.refresh();
        }
    }

    /// FMT-9f: add document `doc` at catalog row `row`.
    fn insert(&mut self, doc: u32, row: usize, len: u16) {
        if let Some((_, old)) = self.docs.insert(doc, (row, len)) {
            self.total = self.total.saturating_sub(old as u64);
        }
        self.total += len as u64;
        self.refresh();
    }

    fn refresh(&mut self) {
        self.avgdl = if self.docs.is_empty() {
            1.0
        } else {
            self.total as f32 / self.docs.len() as f32
        };
    }
}

/// FMT-9d: the handle's BM25 state.
#[derive(Debug, Default)]
struct Bm25State {
    manifest: Bm25Manifest,
    /// Per segment, its dict, once loaded (resident; postings stay on disk).
    dicts: Option<Vec<Vec<text::DictEntry>>>,
    /// Documents written since the last commit, which writes them as a
    /// new segment.
    pending: Vec<(u32, Vec<(String, u8)>)>,
    stats: Option<DocStats>,
    /// Documents in the segment the current flush is merging.
    merged_docs: u32,
}

/// FMT-9d: a manifest's segment runs and their document counts.
type Bm25Segments = (Vec<(u64, u64, u64, u64)>, Vec<u32>);

/// FMT-9d: segments past this many are merged into one by the next commit.
const BM25_MAX_SEGMENTS: usize = 8;

/// FMT-9d: is this row keyword-indexed? Facts and onboarding records;
/// thread turns, essence items and links stay out (FMT-9a: turn postings
/// would be 3× all the fact data).
fn bm25_kind(kind: u8) -> bool {
    kind == ROW_KIND_FACT || kind == ROW_KIND_ONBOARDING
}

/// FMT-9d: read `len` bytes at byte `off` of a page run.
fn read_run_range(pager: &mut Pager, ptr: RunPtr, off: u64, len: u64) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(len as usize);
    if len == 0 {
        return Ok(out);
    }
    if off + len > ptr.len {
        return Err(MnemoError::Invalid("BM25 range past its run".into()));
    }
    let p = PAYLOAD as u64;
    let (first, last) = (off / p, (off + len - 1) / p);
    for i in first..=last {
        let page = pager.read_page(ptr.start + i, ptr.gen)?;
        let lo = if i == first { (off % p) as usize } else { 0 };
        let hi = if i == last {
            ((off + len - 1) % p) as usize + 1
        } else {
            PAYLOAD
        };
        out.extend_from_slice(&page[lo..hi]);
    }
    Ok(out)
}

/// FMT-9d: a segment's dict (the `[u32 len][dict]` head of its run).
fn load_segment_dict(pager: &mut Pager, ptr: RunPtr) -> Result<Vec<text::DictEntry>> {
    let head = read_run_range(pager, ptr, 0, 4)?;
    let n = u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as u64;
    let bytes = read_run_range(pager, ptr, 4, n)?;
    rmp_serde::from_slice(&bytes).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// FMT-9d: a term's postings in one segment (empty when absent).
fn segment_postings(
    pager: &mut Pager,
    ptr: RunPtr,
    dict: &[text::DictEntry],
    term: &str,
) -> Result<Vec<(u32, u8)>> {
    let Ok(i) = dict.binary_search_by(|e| e.term.as_str().cmp(term)) else {
        return Ok(Vec::new());
    };
    let head = read_run_range(pager, ptr, 0, 4)?;
    let base = 4 + u32::from_le_bytes([head[0], head[1], head[2], head[3]]) as u64;
    let e = &dict[i];
    let bytes = read_run_range(pager, ptr, base + e.off as u64, e.len as u64)?;
    text::decode_postings(&bytes)
}

/// FMT-9d: load the BM25 manifest (an empty pointer yields a fresh one).
fn load_bm25_manifest(pager: &mut Pager, ptr: RunPtr) -> Result<Bm25Manifest> {
    if ptr.is_empty() {
        return Ok(Bm25Manifest::default());
    }
    let buf = read_run_bytes(pager, ptr.start, ptr.pages, ptr.len, ptr.gen)?;
    let m: Bm25Manifest =
        rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    if m.tokenizer != text::TOKENIZER_V1 {
        return Err(MnemoError::Invalid(format!(
            "BM25 tokenizer {} is newer than this build understands",
            m.tokenizer
        )));
    }
    Ok(m)
}

/// FMT-9b.2: the handle's view of packed record pages.
#[derive(Debug, Default)]
struct RecPages {
    /// Rows whose records aren't on a page yet; the next flush packs them.
    hot: std::collections::HashMap<usize, Vec<u8>>,
    /// Rows to pack, in order (may repeat; the flush skips repeats).
    pending: Vec<usize>,
    /// Packed page → the live rows on it.
    page_rows: std::collections::HashMap<u64, Vec<usize>>,
}

impl RecPages {
    fn from_catalog(catalog: &[CatalogEntry]) -> RecPages {
        let mut recs = RecPages::default();
        for (i, e) in catalog.iter().enumerate() {
            if e.deleted || e.start_page == 0 {
                continue;
            }
            if let Some(r) = e.v9 {
                if r.rec_slot != REC_SPAN {
                    recs.page_rows.entry(e.start_page).or_default().push(i);
                }
            }
        }
        recs
    }
}

const ROW_SUPERSEDED: u8 = 1;
const ROW_HAS_TTL: u8 = 2;
const ROW_SHARED: u8 = 4;
/// FMT-9e: a thread turn whose record compression is settled: stored as
/// a zstd frame, or tried and not worth it. Unsettled turns are plain and
/// get compressed once they're older than the handle's
/// `turn_compress_after` (see [`Mnemo::set_turn_compress_after`]).
const ROW_SETTLED: u8 = 8;
const ROW_KIND_FACT: u8 = 0;
const ROW_KIND_TURN: u8 = 1;
const ROW_KIND_ESSENCE: u8 = 2;
const ROW_KIND_THREAD_LINK: u8 = 3;
const ROW_KIND_ONBOARDING: u8 = 4;

/// 32-bit FNV-1a over `parts` (separated), never `0` ("none" in a row).
fn fnv1a32(parts: &[&[u8]]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h ^= 0xff;
            h = h.wrapping_mul(0x0100_0193);
        }
        for &b in p.iter() {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    h.max(1)
}

fn memory_type_code(t: MemoryType) -> u8 {
    match t {
        MemoryType::Episodic => 0,
        MemoryType::Semantic => 1,
        MemoryType::Procedural => 2,
        MemoryType::Working => 3,
    }
}

/// FMT-9b: a memory's row tags and scoring inputs (the vector location is
/// filled in when its column entry is written).
fn row_tags(m: &Memory) -> RowV9 {
    let kind = match meta_str(m, MNEMO_META_KIND) {
        Some(MNEMO_META_KIND_TURN) => ROW_KIND_TURN,
        Some(MNEMO_META_KIND_ESSENCE) => ROW_KIND_ESSENCE,
        Some(MNEMO_META_KIND_THREAD_LINK) => ROW_KIND_THREAD_LINK,
        _ if is_onboarding(m) => ROW_KIND_ONBOARDING,
        _ => ROW_KIND_FACT,
    };
    let mut flags = 0u8;
    if m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) {
        flags |= ROW_SUPERSEDED;
    }
    if m.ttl_secs.is_some() {
        flags |= ROW_HAS_TTL;
    }
    if m.scope == Scope::Shared {
        flags |= ROW_SHARED;
    }
    let key_hash = meta_str(m, MNEMO_META_KEY).map_or(0, |k| {
        let slot = Mnemo::key_slot_for(m, k);
        fnv1a32(&[
            slot.scope_owner.as_deref().unwrap_or("").as_bytes(),
            slot.key.as_bytes(),
        ])
    });
    RowV9 {
        kind,
        flags,
        memory_type: memory_type_code(m.memory_type),
        thread_hash: meta_str(m, MNEMO_META_THREAD).map_or(0, |t| fnv1a32(&[t.as_bytes()])),
        key_hash,
        agent_hash: fnv1a32(&[m.agent_id.as_bytes()]),
        created_at: m.created_at,
        importance: m.importance,
        expires_at: m.ttl_secs.map_or(0, |t| m.created_at.saturating_add(t)),
        ..RowV9::default()
    }
}

/// FMT-9b: the handle's view of the fp16 vector column.
#[derive(Debug, Default)]
struct VecColumn {
    /// Resident fp16 values by catalog row (`rows × dims`), once loaded.
    data: Vec<u16>,
    norms: Vec<f32>,
    /// Which rows of `data` are filled. The column is allocated zeroed for
    /// every row (the OS maps those pages lazily) and filled a vector page
    /// at a time as recall touches rows, so the first recall reads only the
    /// pages its candidates live on.
    have: Vec<bool>,
    resident: bool,
    /// Rows whose entries aren't on a page yet; the next flush writes them.
    /// A row here has no page entry (its old one was released).
    hot: std::collections::HashMap<usize, (Vec<u16>, f32)>,
    /// Rows to write, in order (may repeat; the flush skips repeats).
    pending: Vec<usize>,
    /// Vector page → the live rows whose entries it holds.
    page_rows: std::collections::HashMap<u64, Vec<usize>>,
}

impl VecColumn {
    fn from_catalog(catalog: &[CatalogEntry]) -> VecColumn {
        let mut col = VecColumn::default();
        for (i, e) in catalog.iter().enumerate() {
            if e.deleted {
                continue;
            }
            if let Some(r) = e.v9 {
                if r.vec_page != 0 {
                    col.page_rows.entry(r.vec_page).or_default().push(i);
                }
            }
        }
        col
    }
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
    /// FMT-9c.2: `cat_bytes` is a delta run, not a base catalog.
    cat_delta: bool,
    /// FMT-9c.3: `idx_bytes` is an index delta run, not a whole index.
    idx_delta: bool,
    /// FMT-9d: this commit's BM25 segment, and the segments it replaces.
    bm25_seg: Option<(Vec<u8>, Option<usize>)>,
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
    /// FMT-9: generations of the catalog and index runs (v9 only; `None`,
    /// and left out of the encoding, on v8 files).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gens: Option<SnapshotGens>,
}

/// FMT-9: the generations a v9 snapshot's runs were sealed under.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default)]
struct SnapshotGens {
    catalog: u64,
    index: u64,
    /// FMT-9c.2: the catalog's delta-chain head at this snapshot.
    #[serde(default)]
    delta: (u64, u64, u64, u64),
    /// FMT-9c.3: the index's delta-chain head at this snapshot.
    #[serde(default)]
    index_delta: (u64, u64, u64, u64),
    /// FMT-9d: the BM25 manifest at this snapshot.
    #[serde(default)]
    bm25: (u64, u64, u64, u64),
}

impl Snapshot {
    fn catalog_gen(&self) -> u64 {
        self.gens.map_or(0, |g| g.catalog)
    }
    fn index_gen(&self) -> u64 {
        self.gens.map_or(0, |g| g.index)
    }
    fn delta_head(&self) -> RunPtr {
        self.gens.map_or(RunPtr::default(), |g| tuple_ptr(g.delta))
    }
    fn index_delta_head(&self) -> RunPtr {
        self.gens
            .map_or(RunPtr::default(), |g| tuple_ptr(g.index_delta))
    }
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
    /// FMT-9d (Phase 2, v9 files): **identifiers or exact terms to match**
    /// (a version, a PR number, a function name) for hybrid recall. Don't
    /// paste the question: the vector covers its meaning, and its common
    /// words would match the wrong records. BM25 scores, divided by this
    /// query's best, are added to each candidate's similarity with weight
    /// [`RecallRequest::keyword_weight`] before the usual scoring. With an
    /// empty `query` vector ([`RecallRequest::keywords_only`]), recall is
    /// keyword-only. `None` (the default) is plain vector recall.
    pub keywords: Option<String>,
    /// FMT-9d: weight ε of the normalised BM25 score in hybrid recall
    /// (default [`DEFAULT_KEYWORD_WEIGHT`]).
    pub keyword_weight: f32,
    /// FMT-9d: BM25 term-frequency saturation `k1` (default 1.2).
    pub bm25_k1: f32,
    /// FMT-9d: BM25 length normalisation `b` (default 0.75).
    pub bm25_b: f32,
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
            keywords: None,
            keyword_weight: DEFAULT_KEYWORD_WEIGHT,
            bm25_k1: 1.2,
            bm25_b: 0.75,
        }
    }

    /// FMT-9d (v9 files): a keyword-only request: BM25 over `keywords`
    /// (identifiers or exact terms), no query vector, so it works without
    /// an embedder.
    pub fn keywords_only(keywords: impl Into<String>) -> Self {
        Self::new(Vec::new()).keywords(keywords)
    }

    /// FMT-9d (v9 files): add identifiers or exact terms to match (hybrid
    /// recall). Pass identifiers, not the question. See the `keywords`
    /// field.
    pub fn keywords(mut self, text: impl Into<String>) -> Self {
        self.keywords = Some(text.into());
        self
    }

    /// FMT-9d: the weight ε of the keyword score in hybrid recall.
    pub fn keyword_weight(mut self, w: f32) -> Self {
        self.keyword_weight = w;
        self
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

/// Where a file's bytes and an open handle's heap go (P-0 device budget,
/// `examples/device_budget.rs`). **Unstable diagnostic**: fields may
/// change in any release. File figures come from the header and the
/// catalog. RAM figures are buffer capacities, with hash maps estimated
/// from their capacity; they cover the engine's main structures, not
/// every allocation, and not allocator overhead.
#[doc(hidden)]
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct BudgetReport {
    pub file_bytes: u64,
    pub page_size: u64,
    /// Page 0.
    pub header_bytes: u64,
    /// Whole pages holding live records (superseded versions included).
    pub record_page_bytes: u64,
    /// Serialized bytes of those records; the rest of their pages is
    /// slack.
    pub record_payload_bytes: u64,
    /// Pages still holding soft-deleted records (reclaimed by compact).
    pub deleted_record_page_bytes: u64,
    pub catalog_run_bytes: u64,
    pub catalog_payload_bytes: u64,
    pub index_run_bytes: u64,
    pub index_payload_bytes: u64,
    /// First page of the persisted index run (0 = none).
    pub index_start_page: u64,
    pub wal_bytes: u64,
    pub snapshot_manifest_bytes: u64,
    pub cache_dir_run_bytes: u64,
    /// Everything else: superseded catalog, index and directory runs from
    /// earlier flushes, and freed pages, until the next compact.
    pub unreferenced_bytes: u64,
    pub live_records: u64,
    pub ram_catalog: u64,
    pub ram_id_map: u64,
    pub ram_ann_centroids: u64,
    pub ram_ann_codebook: u64,
    pub ram_ann_posting_ids: u64,
    pub ram_ann_posting_codes: u64,
    pub ram_ann_assignment: u64,
    pub ram_cache_dir: u64,
    pub ram_key_caches: u64,
    /// FMT-9b: the resident fp16 vector column as allocated (rows not yet
    /// touched by recall cost no physical memory).
    pub ram_vector_column: u64,
    /// Sum of the `ram_*` fields.
    pub ram_total: u64,
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
    /// FMT-9f: the file's on-disk format (`8` or `9`). A v8 file still
    /// opens and writes as v8; `mnemo compact` upgrades it to v9.
    pub format_version: u16,
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
    index: IdIndex,
    #[allow(dead_code)]
    path: PathBuf,
    dimensions: usize,
    kdf: KdfParams,
    /// Set whenever the catalog changes; drives whether `flush` rewrites it.
    dirty_catalog: bool,
    /// Optional IVF+PQ approximate-nearest-neighbour index.
    ann: Option<IvfPqIndex>,
    /// Set whenever the in-memory ANN index differs from the persisted one.
    /// PERF-1: `flush` persists it only per [`IndexPersist`]'s policy.
    dirty_index: bool,
    /// PERF-1: index persistence policy state.
    index_persist: IndexPersist,
    /// PERF-1 step 3: the header points at a persisted ANN index that
    /// hasn't been loaded yet. It loads on first use (`ensure_ann`).
    ann_unloaded: bool,
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
    /// R-004.2: thread layers + onboarding ids, built by the same lazy scan.
    layers: Option<LayerCache>,
    /// FMT-9e: thread turns are compressed once older than this many
    /// seconds (`0`: at write). Per handle, not persisted.
    turn_compress_after: i64,
    /// FMT-9 (a): the committed free list (v9; always empty on v8). Pages
    /// allocated from it since the last commit are already removed.
    free: FreeList,
    /// FMT-9 (a): runs this transaction stopped referencing (overwritten
    /// or deleted records, tombstoned cache entries, a relocated WAL
    /// region). The next commit adds them to the free list, freed at its
    /// own transaction id.
    pending_free: Vec<(u64, u64)>,
    /// FMT-9 (a): the free list changed (an allocation took pages from it)
    /// and must be rewritten by the next commit.
    free_dirty: bool,
    /// FMT-9 (b): the key-slot run (v9; empty until Phase 13.2 adds slots).
    key_slots: KeySlotRun,
    /// FMT-9 (b): `key_slots` changed since the last commit.
    dirty_keys: bool,
    /// FMT-9: this writable session has burned a transaction id with a
    /// durable header-only commit (`ensure_gen_lease`), so the generations
    /// its pages carry can't repeat ones a crashed earlier session used.
    gen_leased: bool,
    /// FMT-9b: the fp16 vector column (v9; unused on v8).
    col: VecColumn,
    /// FMT-9b.2: packed record pages (v9; unused on v8).
    recs: RecPages,
    /// FMT-9d: the BM25 keyword index (v9; unused on v8).
    bm25: Bm25State,
    /// FMT-9d: test hook: write this layout byte instead of the current
    /// one (to exercise the unknown-layout path).
    index_layout_override: Option<u8>,
    /// FMT-9d: during one recall, each keyword match's normalised BM25
    /// score, by id.
    kw: Option<std::collections::HashMap<u128, f32>>,
    /// FMT-9c.2: catalog rows changed since the last commit (v9 writes
    /// them as a delta run instead of the whole catalog).
    dirty_rows: std::collections::BTreeSet<usize>,
    /// FMT-9c.2: the delta runs on top of the base catalog, oldest first,
    /// and how many rows they hold in all.
    delta_chain: Vec<RunPtr>,
    delta_rows: usize,
    /// FMT-9c.3: the ANN index's delta runs on top of its base run, oldest
    /// first, and how many changes they hold in all.
    index_chain: Vec<RunPtr>,
    index_chain_ops: usize,
    /// FMT-9c.3: the next commit writes the whole index (after a build,
    /// rebuild or restore).
    index_rebase: bool,
    /// FMT-9c.3: this flush's index changes, when it writes them as a
    /// delta run.
    index_delta_ops: Option<Vec<IndexOp>>,
    /// FMT-9c.2: the next commit writes a whole base catalog (after a
    /// restore, or when the chain has grown too long).
    catalog_rewrite: bool,
    /// FMT-9c: the snapshot count was set explicitly (config or
    /// `set_max_snapshots`), which replaces v9's default retention policy.
    snapshot_cap_explicit: bool,
    /// FMT-9c: this flush wrote big runs straight to their home pages,
    /// which must be fsynced before the commit.
    direct_writes: bool,
    /// FMT-9b: keep the column resident while it fits this many bytes.
    vector_cache_budget: usize,
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

/// PERF-1 step 4: ULID → catalog row, as an open-addressing table of `u32`
/// row numbers: 4 bytes a slot, about 5 bytes an entry, against about 43
/// for the `HashMap<u128, usize>` it replaces. A probe compares against the
/// catalog's own id at that row, so the ids aren't stored twice. Rows are
/// never removed (a delete only tombstones the catalog entry), so there's
/// no delete path.
#[derive(Debug, Clone, Default)]
struct IdIndex {
    slots: Vec<u32>,
    len: usize,
}

const NO_ROW: u32 = u32::MAX;

impl IdIndex {
    fn build(catalog: &[CatalogEntry]) -> IdIndex {
        let mut t = IdIndex {
            slots: vec![NO_ROW; crate::index::table_capacity_for(catalog.len())],
            len: 0,
        };
        for (row, e) in catalog.iter().enumerate() {
            t.insert(e.id, row, catalog);
        }
        t
    }

    /// The catalog row holding `id`, if any.
    fn get(&self, id: u128, catalog: &[CatalogEntry]) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        let cap = self.slots.len();
        let mut i = crate::index::slot_home(id, cap);
        loop {
            let row = self.slots[i];
            if row == NO_ROW {
                return None;
            }
            if catalog[row as usize].id == id {
                return Some(row as usize);
            }
            i = crate::index::next_slot(i, cap);
        }
    }

    /// Map `id` to `row`. `catalog` must hold every row already in the
    /// table (the new row may not be pushed yet).
    fn insert(&mut self, id: u128, row: usize, catalog: &[CatalogEntry]) {
        if (self.len + 1) * 4 > self.slots.len() * 3 {
            let cap = (self.slots.len() * 2).max(crate::index::table_capacity_for(self.len + 1));
            let old = std::mem::replace(&mut self.slots, vec![NO_ROW; cap]);
            self.len = 0;
            for r in old.into_iter().filter(|&r| r != NO_ROW) {
                self.insert(catalog[r as usize].id, r as usize, catalog);
            }
        }
        let cap = self.slots.len();
        let mut i = crate::index::slot_home(id, cap);
        loop {
            let r = self.slots[i];
            if r == NO_ROW {
                self.slots[i] = row as u32;
                self.len += 1;
                return;
            }
            if catalog[r as usize].id == id {
                self.slots[i] = row as u32;
                return;
            }
            i = crate::index::next_slot(i, cap);
        }
    }

    fn heap_bytes(&self) -> usize {
        self.slots.capacity() * std::mem::size_of::<u32>()
    }
}

/// Build the ULID → catalog-row lookup.
fn build_id_index(catalog: &[CatalogEntry]) -> IdIndex {
    IdIndex::build(catalog)
}

// ---------------------------------------------------------------------------
// Phase 12.7a helpers used by `ingest_thread`.
// ---------------------------------------------------------------------------

/// One incoming turn after chunking or truncation, ready to embed and
/// write (Phase 12.7a, split out for 12.7d's batch embed).
struct PendingTurn {
    turn_id: String,
    role: String,
    created_at: i64,
    parts: Vec<String>,
    /// Hex sha256 + original length, set only for a truncated
    /// tool-result turn. Applied to the single chunk the truncation
    /// yielded.
    truncation: Option<(String, u64)>,
}

/// Output of `Mnemo::plan_ingest`: the turns to write, the first free
/// `_mnemo.seq`, and the counts gathered so far (skips, chunking,
/// truncation).
struct IngestPlan {
    pending: Vec<PendingTurn>,
    next_seq: u64,
    report: IngestReport,
}

impl IngestPlan {
    /// Every text to embed, in write order (turns, then parts).
    fn texts(&self) -> Vec<&str> {
        self.pending
            .iter()
            .flat_map(|p| p.parts.iter().map(String::as_str))
            .collect()
    }

    fn text_count(&self) -> usize {
        self.pending.iter().map(|p| p.parts.len()).sum()
    }
}

/// PERF-1 / FMT-9: was this catalog row written after the persisted index
/// run? v8 allocates pages monotonically, so a record past the run's first
/// page is newer. v9 reuses pages, so it compares generations instead.
fn written_after_index(e: &CatalogEntry, v9: bool, index_start: u64, index_gen: u64) -> bool {
    if v9 {
        // `start_page` 0: packed at the next flush, so certainly newer.
        e.start_page == 0 || e.gen > index_gen
    } else {
        e.start_page > index_start
    }
}

/// Whether a record belongs in the ANN index: facts only. Thread-archive
/// turns, essence items and thread links are read by kind and scope, never
/// by similarity, so they stay out (R-004.2).
fn is_ann_kind(m: &Memory) -> bool {
    !matches!(
        m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()),
        Some(MNEMO_META_KIND_TURN)
            | Some(MNEMO_META_KIND_ESSENCE)
            | Some(MNEMO_META_KIND_THREAD_LINK)
    )
}

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
