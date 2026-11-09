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
pub const DEFAULT_TEXT_WEIGHT: f32 = 0.5;

/// FMT-9b: default [`Mnemo::set_vector_cache_budget`]: the fp16 vector
/// column stays resident while it fits (100K × 384 dims is about 77 MB) and
/// is read through the page cache above it. PLANNER-A, 2026-11-07.
pub const DEFAULT_VECTOR_CACHE_BUDGET: usize = 96 * 1024 * 1024;

/// FMT-9b: below this many facts a v9 file has no ANN index: an exact
/// fp16 scan of 20K × 384 dims is about a millisecond, and the IVF+PQ
/// codebook alone would cost more disk than it saves time.
pub const V9_ANN_MIN_FACTS: usize = 20_000;

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
    secret: &[u8],
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
                if let Some((txn_id, frames)) = try_recover_candidate(file, page_no, secret)? {
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
    secret: &[u8],
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
    let kek = match crypto::derive_kek(secret, &candidate.salt, kdf) {
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
    /// FMT-9 development switch: the on-disk format a new file is created
    /// in. `0` (the default) means the current default, v8, unless the
    /// `MNEMO_DEV_FORMAT=9` environment variable is set; `8` or `9` pick
    /// one explicitly. **Not a stable API:** v9 is under construction until
    /// PR 9f, its layout can change between the 9a–9f PRs, and 9f removes
    /// this field when v9 becomes the default.
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
        0 => Ok(match std::env::var("MNEMO_DEV_FORMAT").as_deref() {
            Ok("9") => VERSION_V9,
            _ => VERSION,
        }),
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
    /// FMT-9d (Phase 2, v9 files): keyword query for hybrid recall. Its
    /// BM25 scores, divided by the best one for this query, are added to
    /// each candidate's similarity with weight [`RecallRequest::text_weight`]
    /// before the usual scoring. With an empty `query` vector
    /// ([`RecallRequest::text`]), recall is keyword-only. `None` (the
    /// default) is plain vector recall.
    pub text_query: Option<String>,
    /// FMT-9d: weight ε of the normalised BM25 score in hybrid recall
    /// (default [`DEFAULT_TEXT_WEIGHT`]).
    pub text_weight: f32,
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
            text_query: None,
            text_weight: DEFAULT_TEXT_WEIGHT,
            bm25_k1: 1.2,
            bm25_b: 0.75,
        }
    }

    /// FMT-9d (v9 files): a keyword-only request: BM25 over `text`, no
    /// query vector, so it works without an embedder.
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(Vec::new()).text_query(text)
    }

    /// FMT-9d (v9 files): add a keyword query (hybrid recall when the
    /// request also has a vector). See [`RecallRequest::text_query`].
    pub fn text_query(mut self, text: impl Into<String>) -> Self {
        self.text_query = Some(text.into());
        self
    }

    /// FMT-9d: the weight ε of the keyword score in hybrid recall.
    pub fn text_weight(mut self, w: f32) -> Self {
        self.text_weight = w;
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
/// `gen` is the generation the run's pointer expects (v9; `0` before).
fn read_run_bytes(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(len as usize);
    for i in 0..pages {
        buf.extend_from_slice(&pager.read_page(start + i, gen)?);
    }
    buf.truncate(len as usize);
    Ok(buf)
}

/// FMT-9b: a page run read as a byte stream, one page at a time and
/// without caching it, so loading a large run (a 100K catalog is several
/// MB, its index about 17 MB) never holds the whole serialized run in
/// memory next to the structure being decoded from it.
struct RunReader<'a> {
    pager: &'a mut Pager,
    /// The first page error, kept so the caller reports it as itself
    /// (e.g. `PageAuthFailed`) rather than as a decode error.
    err: &'a std::cell::RefCell<Option<MnemoError>>,
    next_page: u64,
    end_page: u64,
    gen: u64,
    remaining: u64,
    buf: Vec<u8>,
    pos: usize,
}

impl<'a> RunReader<'a> {
    fn new(
        pager: &'a mut Pager,
        err: &'a std::cell::RefCell<Option<MnemoError>>,
        start: u64,
        pages: u64,
        len: u64,
        gen: u64,
    ) -> Self {
        RunReader {
            pager,
            err,
            next_page: start,
            end_page: start + pages,
            gen,
            remaining: len,
            buf: Vec::new(),
            pos: 0,
        }
    }
}

impl Read for RunReader<'_> {
    /// The decoder reads a byte or a few at a time; serve those straight
    /// from the current page.
    #[inline]
    fn read_exact(&mut self, out: &mut [u8]) -> std::io::Result<()> {
        if self.buf.len() - self.pos >= out.len() {
            out.copy_from_slice(&self.buf[self.pos..self.pos + out.len()]);
            self.pos += out.len();
            return Ok(());
        }
        let mut done = 0;
        while done < out.len() {
            match self.read(&mut out[done..])? {
                0 => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                n => done += n,
            }
        }
        Ok(())
    }

    #[inline]
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.buf.len() {
            if self.remaining == 0 || self.next_page >= self.end_page {
                return Ok(0);
            }
            let mut page = match self.pager.read_page_uncached(self.next_page, self.gen) {
                Ok(p) => p,
                Err(e) => {
                    let msg = e.to_string();
                    *self.err.borrow_mut() = Some(e);
                    return Err(std::io::Error::other(msg));
                }
            };
            self.next_page += 1;
            page.truncate((self.remaining as usize).min(page.len()));
            self.remaining -= page.len() as u64;
            self.buf = page;
            self.pos = 0;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// FMT-9b: deserialize a sequence into a `Vec` sized from the encoded
/// length up front (bounded by `cap`, so a hostile length can't force a
/// huge allocation). serde's own `Vec` impl pre-sizes only up to 1 MiB and
/// then doubles, which for a 100K catalog leaves a quarter of the capacity
/// unused and a freed half-size copy behind in the allocator.
struct ExactVec<T> {
    cap: usize,
    _t: std::marker::PhantomData<T>,
}

impl<'de, T: Deserialize<'de>> serde::de::DeserializeSeed<'de> for ExactVec<T> {
    type Value = Vec<T>;
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        d: D,
    ) -> std::result::Result<Vec<T>, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for ExactVec<T> {
    type Value = Vec<T>;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a sequence")
    }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<Vec<T>, A::Error> {
        let mut v = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(self.cap));
        while let Some(x) = seq.next_element()? {
            v.push(x);
        }
        Ok(v)
    }
}

/// Load a v5+ catalog run (an empty `pages` yields an empty catalog).
fn load_catalog(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<CatalogEntry>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    // Sized exactly (every row encodes to at least 8 bytes, which bounds
    // the up-front allocation by the run's length). v8 decodes from one
    // buffer, as before; v9 streams the run (it's larger, and it's paired
    // with a resident vector column, so the transient buffer costs RSS).
    let cap = (len / 8) as usize;
    if pager.version() < VERSION_V9 {
        let buf = read_run_bytes(pager, start, pages, len, gen)?;
        let mut de = rmp_serde::Deserializer::from_read_ref(&buf);
        return serde::de::DeserializeSeed::deserialize(
            ExactVec::<CatalogEntry> {
                cap,
                _t: std::marker::PhantomData,
            },
            &mut de,
        )
        .map_err(|e| MnemoError::Serialize(e.to_string()));
    }
    let err = std::cell::RefCell::new(None);
    let mut de = rmp_serde::Deserializer::new(RunReader::new(pager, &err, start, pages, len, gen));
    let res = serde::de::DeserializeSeed::deserialize(
        ExactVec::<CatalogEntry> {
            cap,
            _t: std::marker::PhantomData,
        },
        &mut de,
    );
    drop(de);
    match (res, err.into_inner()) {
        (Ok(v), _) => Ok(v),
        (Err(_), Some(page_err)) => Err(page_err),
        (Err(e), None) => Err(MnemoError::Serialize(e.to_string())),
    }
}

/// FMT-9c.2: one delta run of a v9 catalog: the rows a commit changed or
/// added, by position, and the previous delta in the chain.
#[derive(Serialize, Deserialize)]
struct CatalogDelta {
    prev: (u64, u64, u64, u64),
    rows: Vec<(u32, CatalogEntry)>,
}

fn ptr_tuple(p: RunPtr) -> (u64, u64, u64, u64) {
    (p.start, p.pages, p.len, p.gen)
}

fn tuple_ptr(t: (u64, u64, u64, u64)) -> RunPtr {
    RunPtr {
        start: t.0,
        pages: t.1,
        len: t.2,
        gen: t.3,
    }
}

/// FMT-9c.2: walk the delta chain from `head` back to the base and replay
/// it onto `catalog`, oldest first. Returns the catalog, the chain (oldest
/// first) and its total row count. An empty `head` is a no-op (and v8).
fn apply_delta_chain(
    pager: &mut Pager,
    mut catalog: Vec<CatalogEntry>,
    head: RunPtr,
) -> Result<(Vec<CatalogEntry>, Vec<RunPtr>, usize)> {
    let mut deltas: Vec<(RunPtr, Vec<(u32, CatalogEntry)>)> = Vec::new();
    let mut at = head;
    while !at.is_empty() {
        if deltas.len() > 4096 {
            return Err(MnemoError::Invalid("catalog delta chain too long".into()));
        }
        let buf = read_run_bytes(pager, at.start, at.pages, at.len, at.gen)?;
        let d: CatalogDelta =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        deltas.push((at, d.rows));
        at = tuple_ptr(d.prev);
    }
    deltas.reverse();
    let mut rows = 0usize;
    let mut chain = Vec::with_capacity(deltas.len());
    for (ptr, d) in deltas {
        rows += d.len();
        chain.push(ptr);
        for (i, row) in d {
            let i = i as usize;
            match i.cmp(&catalog.len()) {
                std::cmp::Ordering::Less => catalog[i] = row,
                std::cmp::Ordering::Equal => catalog.push(row),
                std::cmp::Ordering::Greater => {
                    return Err(MnemoError::Invalid("catalog delta skips a row".into()))
                }
            }
        }
    }
    Ok((catalog, chain, rows))
}

/// FMT-9c.3: one delta run of a v9 ANN index: the changes a commit made,
/// and the previous delta in the chain.
#[derive(Serialize)]
struct IndexDeltaRef<'a> {
    prev: (u64, u64, u64, u64),
    ops: &'a [IndexOp],
}

#[derive(Deserialize)]
struct IndexDeltaOwned {
    prev: (u64, u64, u64, u64),
    ops: Vec<IndexOp>,
}

/// FMT-9c.3: walk the index delta chain from `head` and replay it onto a
/// just-loaded base index, oldest first. Returns the chain and its change
/// count.
fn apply_index_chain(
    pager: &mut Pager,
    ann: &mut IvfPqIndex,
    head: RunPtr,
) -> Result<(Vec<RunPtr>, usize)> {
    let mut deltas: Vec<(RunPtr, Vec<IndexOp>)> = Vec::new();
    let mut at = head;
    while !at.is_empty() {
        if deltas.len() > 4096 {
            return Err(MnemoError::Invalid("index delta chain too long".into()));
        }
        let buf = read_run_bytes(pager, at.start, at.pages, at.len, at.gen)?;
        let d: IndexDeltaOwned =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        deltas.push((at, d.ops));
        at = tuple_ptr(d.prev);
    }
    deltas.reverse();
    let mut chain = Vec::with_capacity(deltas.len());
    let mut n = 0usize;
    for (ptr, ops) in deltas {
        n += ops.len();
        chain.push(ptr);
        ann.apply_ops(ops)?;
    }
    Ok((chain, n))
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
    let buf = read_run_bytes(pager, start, pages, len, 0)?;
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
                buf.extend_from_slice(&pager.read_page(e.start_page + i, 0)?);
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
            gen: 0,
            v9: None,
        });
    }
    Ok(out)
}

/// Load the snapshot manifest (an empty `pages` yields an empty manifest).
fn load_manifest(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
) -> Result<Vec<Snapshot>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len, gen)?;
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
    gen: u64,
) -> Result<Vec<CacheDirectoryEntry>> {
    if pages == 0 {
        return Ok(Vec::new());
    }
    let buf = read_run_bytes(pager, start, pages, len, gen)?;
    rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))
}

/// Load an ANN index run, validating its dimensionality.
fn load_index(
    pager: &mut Pager,
    start: u64,
    pages: u64,
    len: u64,
    gen: u64,
    dims: usize,
) -> Result<Option<IvfPqIndex>> {
    if pages == 0 {
        return Ok(None);
    }
    // v8 decodes from one buffer, as before. v9 streams the run (a 100K
    // index is ~17 MB), trading decode speed for not holding it twice.
    if pager.version() < VERSION_V9 {
        let buf = read_run_bytes(pager, start, pages, len, gen)?;
        let mut idx: IvfPqIndex =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        if idx.dims() != dims {
            return Err(MnemoError::Invalid(
                "ANN index dimensionality does not match the database".into(),
            ));
        }
        idx.rebuild_assignment();
        return Ok(Some(idx));
    }
    let err = std::cell::RefCell::new(None);
    let res: std::result::Result<IvfPqIndex, _> =
        rmp_serde::from_read(RunReader::new(pager, &err, start, pages, len, gen));
    let mut idx = match (res, err.into_inner()) {
        (Ok(i), _) => i,
        (Err(_), Some(page_err)) => return Err(page_err),
        (Err(e), None) => return Err(MnemoError::Serialize(e.to_string())),
    };
    if idx.dims() != dims {
        return Err(MnemoError::Invalid(
            "ANN index dimensionality does not match the database".into(),
        ));
    }
    idx.rebuild_assignment();
    Ok(Some(idx))
}

/// FMT-9 (a): load the v9 free list (an empty pointer yields an empty list).
fn load_free_list(pager: &mut Pager, ptr: RunPtr) -> Result<FreeList> {
    if ptr.is_empty() {
        return Ok(FreeList::default());
    }
    let buf = read_run_bytes(pager, ptr.start, ptr.pages, ptr.len, ptr.gen)?;
    let runs: Vec<crate::freelist::FreeRun> =
        rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    FreeList::from_runs(runs)
}

/// FMT-9 (b): load the v9 key-slot run (an empty pointer yields no slots).
fn load_key_slots(pager: &mut Pager, ptr: RunPtr) -> Result<KeySlotRun> {
    if ptr.is_empty() {
        return Ok(KeySlotRun::default());
    }
    let buf = read_run_bytes(pager, ptr.start, ptr.pages, ptr.len, ptr.gen)?;
    let run: KeySlotRun =
        rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
    run.validate()?;
    Ok(run)
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

impl Mnemo {
    /// Create a brand-new encrypted database at `path`.
    pub fn create(
        path: impl Into<PathBuf>,
        passphrase: &str,
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        Self::create_secret(path, passphrase.as_bytes(), config)
    }

    /// PERF-1 step 7 (R-004.5): create a database unlocked by a **raw
    /// 256-bit key** instead of a passphrase, for keys held by an OS
    /// keychain or keystore. The key must be uniformly random (see
    /// [`Mnemo::generate_key`]); it gets no stretching. The header records
    /// minimal Argon2id parameters ([`KdfParams::raw_key`]), so opening
    /// costs microseconds instead of tens of milliseconds. `config.kdf` is
    /// ignored. No format change: the key goes through the same KEK
    /// derivation a passphrase does.
    pub fn create_with_key(
        path: impl Into<PathBuf>,
        key: &[u8; crypto::KEY_LEN],
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        let config = MnemoConfig {
            kdf: KdfParams::raw_key(),
            ..config
        };
        Self::create_secret(path, key, config)
    }

    /// A fresh random 256-bit key for [`Mnemo::create_with_key`], from the
    /// thread-local CSPRNG (seeded from the OS). Store it in a keychain;
    /// it can't be recovered from the file.
    pub fn generate_key() -> [u8; crypto::KEY_LEN] {
        *crypto::random_dek()
    }

    fn create_secret(
        path: impl Into<PathBuf>,
        secret: &[u8],
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        let path: PathBuf = path.into();
        if config.dimensions == 0 {
            return Err(MnemoError::Invalid("dimensions must be > 0".into()));
        }
        let version = resolve_format_version(config.format_version)?;
        if version >= VERSION_V9 && veccol::per_page(config.dimensions) == 0 {
            return Err(MnemoError::Invalid(format!(
                "a v9 file supports up to {} dimensions for now (one fp16 vector \
                 must fit a page); vector entries that span pages are not supported yet",
                (PAYLOAD - veccol::PAGE_HEADER - 4) / 2
            )));
        }
        let flags = if version >= VERSION_V9 && config.store_float_vectors {
            FLAG_ENCRYPTED | FLAG_FLOAT_VECTORS
        } else {
            FLAG_ENCRYPTED
        };
        // Exclusive lock covers the write path; second writer sees `Locked`.
        let file = open_with_lock(&path, /*write=*/ true, /*create=*/ true)?;

        let salt = crypto::random_salt();
        let dek = crypto::random_dek();
        let kek = crypto::derive_kek(secret, &salt, config.kdf)?;
        let dek_nonce = crypto::random_nonce();
        let wrapped = crypto::wrap_dek(&kek, &dek_nonce, &dek)?;
        if wrapped.len() != WRAPPED_DEK_LEN {
            return Err(MnemoError::Crypto("unexpected wrapped DEK length".into()));
        }
        let mut wrapped_dek = [0u8; WRAPPED_DEK_LEN];
        wrapped_dek.copy_from_slice(&wrapped);

        let mut header = Header {
            version,
            page_size: PAGE_SIZE as u32,
            flags,
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
            // FMT-9 v9 fields; all zero on a fresh file (and unused on v8).
            catalog_gen: 0,
            index_gen: 0,
            manifest_gen: 0,
            cache_gen: 0,
            free: RunPtr::default(),
            keys: RunPtr::default(),
            bm25: RunPtr::default(),
            index_delta: RunPtr::default(),
            catalog_delta: RunPtr::default(),
        };

        let mut pager = Pager::new(file, dek, 0, version);
        let mut page = header.to_page();
        header.apply_seal(&mut page, pager.dek())?;
        pager.write_raw(0, &page)?;
        pager.sync()?;

        Ok(Mnemo {
            pager,
            header,
            catalog: Vec::new(),
            index: IdIndex::default(),
            path,
            dimensions: config.dimensions,
            kdf: config.kdf,
            dirty_catalog: false,
            ann: None,
            dirty_index: false,
            index_persist: IndexPersist::default(),
            ann_unloaded: false,
            manifest: Vec::new(),
            // FMT-9c: v9 keeps fewer snapshots by default: with page reuse,
            // retention is what pins pages.
            max_snapshots: if version >= VERSION_V9 && config.max_snapshots == DEFAULT_MAX_SNAPSHOTS
            {
                V9_DEFAULT_MAX_SNAPSHOTS
            } else {
                config.max_snapshots
            },
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
            layers: None,
            free: FreeList::default(),
            pending_free: Vec::new(),
            free_dirty: false,
            key_slots: KeySlotRun::default(),
            dirty_keys: false,
            gen_leased: false,
            col: VecColumn::default(),
            recs: RecPages::default(),
            bm25: Bm25State::default(),
            kw: None,
            dirty_rows: std::collections::BTreeSet::new(),
            delta_chain: Vec::new(),
            delta_rows: 0,
            catalog_rewrite: false,
            index_chain: Vec::new(),
            index_chain_ops: 0,
            index_rebase: false,
            index_delta_ops: None,
            snapshot_cap_explicit: config.max_snapshots != DEFAULT_MAX_SNAPSHOTS,
            direct_writes: false,
            vector_cache_budget: DEFAULT_VECTOR_CACHE_BUDGET,
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
        Self::open_secret(path, passphrase.as_bytes())
    }

    /// PERF-1 step 7: open a database created by [`Mnemo::create_with_key`]
    /// (or rekeyed with [`Mnemo::rekey_with_key`]) with its raw key.
    pub fn open_with_key(path: impl Into<PathBuf>, key: &[u8; crypto::KEY_LEN]) -> Result<Mnemo> {
        Self::open_secret(path, key)
    }

    fn open_secret(path: impl Into<PathBuf>, secret: &[u8]) -> Result<Mnemo> {
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
                let scan = scan_header_recovery_candidates(&mut file, secret)?;
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
        let kek = crypto::derive_kek(secret, &header.salt, kdf)?;
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
                    header.catalog_gen,
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
        // FMT-9c.2: replay the v9 delta chain onto the base catalog.
        let (catalog, delta_chain, delta_rows) =
            apply_delta_chain(&mut pager, catalog, header.catalog_delta)?;
        let index = build_id_index(&catalog);
        // PERF-1 step 3: the ANN index loads on first use, not at open. A
        // v5→v6 page migration is the exception: it must read the index
        // now, under the old page AAD, to rewrite it.
        let ann = if on_disk_version < 6 {
            load_index(
                &mut pager,
                header.index_start,
                header.index_pages,
                header.index_len,
                header.index_gen,
                dimensions,
            )?
        } else {
            None
        };
        let ann_unloaded = ann.is_none() && header.index_pages > 0;
        let mut manifest = load_manifest(
            &mut pager,
            header.manifest_start,
            header.manifest_pages,
            header.manifest_len,
            header.manifest_gen,
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
                    let payload = pager.read_page(e.start_page + i, 0)?;
                    pager.write_page(e.start_page + i, 0, &payload)?;
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
            header.cache_gen,
        )?;
        let cache_idx = DirectoryIndex::build(&cache_dir);
        // FMT-9 (a)/(b): the free list and key-slot run (empty pointers on
        // v8, so these don't touch the disk there).
        let free = load_free_list(&mut pager, header.free)?;
        let key_slots = load_key_slots(&mut pager, header.keys)?;
        let col = VecColumn::from_catalog(&catalog);
        let recs = RecPages::from_catalog(&catalog);
        let bm25_manifest = load_bm25_manifest(&mut pager, header.bm25)?;
        let max_snapshots = if header.version >= VERSION_V9 {
            V9_DEFAULT_MAX_SNAPSHOTS
        } else {
            DEFAULT_MAX_SNAPSHOTS
        };

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
            // A v5→v6 page migration leaves the index sealed under the old
            // page AAD, so it must be rewritten on the next flush.
            index_persist: IndexPersist {
                forced: dirty_index,
                ..IndexPersist::default()
            },
            ann_unloaded,
            manifest,
            // `Mnemo::open` has no [`MnemoConfig`] to read; default the cap
            // to [`DEFAULT_MAX_SNAPSHOTS`]. Override with
            // [`Mnemo::set_max_snapshots`] right after open if you need
            // unlimited retention or a different cap for this handle.
            max_snapshots,
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
            layers: None,
            free,
            pending_free: Vec::new(),
            free_dirty: false,
            key_slots,
            dirty_keys: false,
            gen_leased: false,
            col,
            recs,
            bm25: Bm25State {
                manifest: bm25_manifest,
                ..Bm25State::default()
            },
            kw: None,
            dirty_rows: std::collections::BTreeSet::new(),
            delta_chain,
            delta_rows,
            catalog_rewrite: false,
            index_chain: Vec::new(),
            index_chain_ops: 0,
            index_rebase: false,
            index_delta_ops: None,
            snapshot_cap_explicit: false,
            direct_writes: false,
            vector_cache_budget: DEFAULT_VECTOR_CACHE_BUDGET,
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
        Self::open_read_only_secret(path, passphrase.as_bytes())
    }

    /// PERF-1 step 7: [`Mnemo::open_read_only`] for a raw-key database.
    pub fn open_read_only_with_key(
        path: impl Into<PathBuf>,
        key: &[u8; crypto::KEY_LEN],
    ) -> Result<Mnemo> {
        Self::open_read_only_secret(path, key)
    }

    fn open_read_only_secret(path: impl Into<PathBuf>, secret: &[u8]) -> Result<Mnemo> {
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
                let recoverable = scan_header_recovery_candidates(&mut file, secret)?;
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
        let kek = crypto::derive_kek(secret, &header.salt, kdf)?;
        let dek = crypto::unwrap_dek(&kek, &header.dek_nonce, &header.wrapped_dek)?;
        header.validate_seal(&dek)?;

        let mut pager = Pager::new(file, dek, header.write_counter, header.version);
        let dimensions = header.dimensions as usize;

        let catalog = load_catalog(
            &mut pager,
            header.catalog_start,
            header.catalog_pages,
            header.catalog_len,
            header.catalog_gen,
        )?;
        // FMT-9c.2: replay the v9 delta chain onto the base catalog.
        let (catalog, delta_chain, delta_rows) =
            apply_delta_chain(&mut pager, catalog, header.catalog_delta)?;
        let index = build_id_index(&catalog);
        // PERF-1 step 3: the ANN index loads on first use, not at open.
        let ann: Option<IvfPqIndex> = None;
        let ann_unloaded = header.index_pages > 0;
        let manifest = load_manifest(
            &mut pager,
            header.manifest_start,
            header.manifest_pages,
            header.manifest_len,
            header.manifest_gen,
        )?;
        let cache_dir = load_cache_directory(
            &mut pager,
            header.cache_start,
            header.cache_pages,
            header.cache_len,
            header.cache_gen,
        )?;
        let cache_idx = DirectoryIndex::build(&cache_dir);
        let free = load_free_list(&mut pager, header.free)?;
        let key_slots = load_key_slots(&mut pager, header.keys)?;
        let col = VecColumn::from_catalog(&catalog);
        let recs = RecPages::from_catalog(&catalog);
        let bm25_manifest = load_bm25_manifest(&mut pager, header.bm25)?;
        let max_snapshots = if header.version >= VERSION_V9 {
            V9_DEFAULT_MAX_SNAPSHOTS
        } else {
            DEFAULT_MAX_SNAPSHOTS
        };

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
            index_persist: IndexPersist::default(),
            ann_unloaded,
            manifest,
            max_snapshots,
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
            layers: None,
            free,
            pending_free: Vec::new(),
            free_dirty: false,
            key_slots,
            dirty_keys: false,
            gen_leased: false,
            col,
            recs,
            bm25: Bm25State {
                manifest: bm25_manifest,
                ..Bm25State::default()
            },
            kw: None,
            dirty_rows: std::collections::BTreeSet::new(),
            delta_chain,
            delta_rows,
            catalog_rewrite: false,
            index_chain: Vec::new(),
            index_chain_ops: 0,
            index_rebase: false,
            index_delta_ops: None,
            snapshot_cap_explicit: false,
            direct_writes: false,
            vector_cache_budget: DEFAULT_VECTOR_CACHE_BUDGET,
        })
    }

    /// Override the manifest snapshot cap on this open handle. `0` disables
    /// the cap; any positive value keeps the most-recent `max` snapshots and
    /// drops the rest on the next flush. See [`MnemoConfig::max_snapshots`].
    pub fn set_max_snapshots(&mut self, max: usize) {
        self.max_snapshots = max;
        // FMT-9c: an explicit count is a hard cap on v9 (see
        // `apply_retention`).
        self.snapshot_cap_explicit = true;
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

    /// FMT-9: true for a v9 file (page reuse, generations in the AAD).
    fn is_v9(&self) -> bool {
        self.header.version >= VERSION_V9
    }

    /// FMT-9: the generation for pages written now: the id of the
    /// transaction that will commit them, or a lower bound on it (a WAL
    /// relocation inside the next flush can push the commit id higher).
    /// Either way it's strictly greater than the generation any earlier
    /// write to a reused page carried, which is all rollback protection
    /// needs: a page is reused only after the transaction that freed it
    /// committed, and that transaction came after the one that wrote it.
    /// `0` on v8, whose page AAD has no generation.
    fn write_gen(&self) -> u64 {
        if self.is_v9() {
            self.header.wal_seq + 1
        } else {
            0
        }
    }

    /// FMT-9 (a): the oldest retained snapshot's transaction id
    /// (`u64::MAX` when none is retained). Only pages freed at or before it
    /// may be reused.
    fn min_snapshot_txn(&self) -> u64 {
        self.manifest
            .iter()
            .map(|s| s.txn_id)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Allocate `n` contiguous pages. v9 takes them from the free list when
    /// an eligible run fits (freed by a committed transaction, and not
    /// referenced by a retained snapshot); otherwise, and always on v8, the
    /// file grows.
    fn alloc_pages(&mut self, n: u64) -> u64 {
        if self.is_v9() {
            let committed = self.header.wal_seq;
            let min_snap = self.min_snapshot_txn();
            if let Some(p) = self.free.alloc(n, committed, min_snap) {
                self.free_dirty = true;
                return p;
            }
        }
        let p = self.header.next_page;
        self.header.next_page += n;
        p
    }

    /// FMT-9 (a): record that the current transaction stopped referencing
    /// `n` pages from `page`. The next commit frees them. No-op on v8,
    /// which never reuses a page.
    fn release_pages(&mut self, page: u64, n: u64) {
        if self.is_v9() && n > 0 && page != 0 {
            self.pending_free.push((page, n));
        }
    }

    /// FMT-9d: make sure the live-document stats and the segment dicts are
    /// loaded. Stats come from the rows; dicts from each segment's head.
    fn ensure_bm25(&mut self) -> Result<()> {
        if self.bm25.stats.is_none() {
            let mut docs = std::collections::HashMap::new();
            let mut total = 0u64;
            for (i, e) in self.catalog.iter().enumerate() {
                if e.deleted {
                    continue;
                }
                if let Some(r) = e.v9 {
                    if r.doc != 0 && bm25_kind(r.kind) {
                        docs.insert(r.doc, (i, r.doc_len));
                        total += r.doc_len as u64;
                    }
                }
            }
            let avgdl = if docs.is_empty() {
                1.0
            } else {
                total as f32 / docs.len() as f32
            };
            self.bm25.stats = Some(DocStats { docs, avgdl });
        }
        if self.bm25.dicts.is_none() {
            let mut dicts = Vec::with_capacity(self.bm25.manifest.segments.len());
            for &t in &self.bm25.manifest.segments {
                dicts.push(load_segment_dict(&mut self.pager, tuple_ptr(t))?);
            }
            self.bm25.dicts = Some(dicts);
        }
        Ok(())
    }

    /// FMT-9d: BM25 scores of the live keyword-indexed rows that match
    /// `query`: catalog row → score. Empty on v8 (no keyword index).
    fn bm25_scores(
        &mut self,
        query: &str,
        k1: f32,
        b: f32,
    ) -> Result<std::collections::HashMap<usize, f32>> {
        let mut scores: std::collections::HashMap<usize, f32> = std::collections::HashMap::new();
        if !self.is_v9() {
            return Ok(scores);
        }
        self.ensure_bm25()?;
        let terms: std::collections::BTreeSet<String> = text::tokenize(query).into_iter().collect();
        let segs: Vec<RunPtr> = self
            .bm25
            .manifest
            .segments
            .iter()
            .map(|&t| tuple_ptr(t))
            .collect();
        let stats = self.bm25.stats.as_ref().expect("loaded above");
        let dicts = self.bm25.dicts.as_ref().expect("loaded above");
        let n = stats.docs.len();
        for term in &terms {
            let mut posts: Vec<(u32, u8)> = Vec::new();
            for (si, seg) in segs.iter().enumerate() {
                posts.extend(segment_postings(&mut self.pager, *seg, &dicts[si], term)?);
            }
            for (doc, tfs) in &self.bm25.pending {
                if let Ok(i) = tfs.binary_search_by(|(t, _)| t.as_str().cmp(term)) {
                    posts.push((*doc, tfs[i].1));
                }
            }
            posts.retain(|(d, _)| stats.docs.contains_key(d));
            if posts.is_empty() {
                continue;
            }
            let idf = text::idf(n, posts.len());
            for (d, tf) in posts {
                let (row, len) = stats.docs[&d];
                *scores.entry(row).or_default() += text::bm25(tf, idf, len, stats.avgdl, k1, b);
            }
        }
        Ok(scores)
    }

    /// FMT-9d: the next commit's BM25 segment: the pending documents, plus,
    /// past [`BM25_MAX_SEGMENTS`], the small segments (all but the first),
    /// or all of them once the small ones outgrow a tenth of the first.
    /// Dead documents are dropped from merged segments. Returns the segment
    /// bytes and, for a merge, the index from which it replaces the
    /// segments (`None`: it's appended).
    fn bm25_next_segment(&mut self) -> Result<Option<(Vec<u8>, Option<usize>)>> {
        if !self.is_v9() || self.bm25.pending.is_empty() {
            return Ok(None);
        }
        let segs: Vec<RunPtr> = self
            .bm25
            .manifest
            .segments
            .iter()
            .map(|&t| tuple_ptr(t))
            .collect();
        // Mostly dead (a mass delete): merge everything, so the dead
        // postings go and the old segments' pages can be reused or cut off.
        self.ensure_bm25()?;
        let written: u64 = self.bm25.manifest.seg_docs.iter().map(|&d| d as u64).sum();
        let live_docs = self.bm25.stats.as_ref().map_or(0, |s| s.docs.len()) as u64;
        let mostly_dead = !segs.is_empty() && written > 64 && live_docs * 2 < written;
        if segs.len() < BM25_MAX_SEGMENTS && !mostly_dead {
            return Ok(Some((text::build_segment(&self.bm25.pending)?, None)));
        }
        let small: u64 = segs.iter().skip(1).map(|p| p.len).sum();
        let from = if mostly_dead || small * 10 > segs[0].len {
            0
        } else {
            1
        };
        let live: std::collections::HashSet<u32> = self
            .bm25
            .stats
            .as_ref()
            .expect("loaded above")
            .docs
            .keys()
            .copied()
            .collect();
        let mut by_doc: std::collections::BTreeMap<u32, Vec<(String, u8)>> =
            std::collections::BTreeMap::new();
        for seg in &segs[from..] {
            let bytes = read_run_bytes(&mut self.pager, seg.start, seg.pages, seg.len, seg.gen)?;
            let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            let dict: Vec<text::DictEntry> = rmp_serde::from_slice(&bytes[4..4 + n])
                .map_err(|e| MnemoError::Serialize(e.to_string()))?;
            let post = &bytes[4 + n..];
            for e in dict {
                let range = e.off as usize..(e.off + e.len) as usize;
                let list = text::decode_postings(post.get(range).ok_or_else(|| {
                    MnemoError::Invalid("BM25 dict entry past its postings".into())
                })?)?;
                for (d, tf) in list {
                    if live.contains(&d) {
                        by_doc.entry(d).or_default().push((e.term.clone(), tf));
                    }
                }
            }
        }
        for (d, tfs) in &self.bm25.pending {
            if live.contains(d) {
                by_doc.insert(*d, tfs.clone());
            }
        }
        let docs: Vec<(u32, Vec<(String, u8)>)> = by_doc.into_iter().collect();
        self.bm25.merged_docs = docs.len() as u32;
        Ok(Some((text::build_segment(&docs)?, Some(from))))
    }

    /// FMT-9c.3: the generation the persisted index is current to: its
    /// base run's, or its newest delta run's.
    fn index_gen_now(&self) -> u64 {
        self.header.index_gen.max(self.header.index_delta.gen)
    }

    /// FMT-9c.3: should this v9 flush persist the index, and how? Every
    /// commit that changed it writes those changes as a delta run (so the
    /// index is never stale after a crash); a build, rebuild, drop or
    /// restore, or a chain past 64 runs or a quarter of the index, writes
    /// the whole index instead. Sets `index_delta_ops` for a delta.
    fn plan_index_v9(&mut self) -> bool {
        self.index_delta_ops = None;
        let Some(ann) = self.ann.as_mut() else {
            // Dropped (persisted as "no index"), or not loaded (unchanged).
            return self.dirty_index;
        };
        let ops = ann.take_log();
        let n = ann.len();
        if self.index_rebase {
            return true;
        }
        if ops.is_empty() {
            return false;
        }
        if self.index_chain.len() >= 64 || (self.index_chain_ops + ops.len()) * 4 > n.max(256) {
            return true;
        }
        self.index_delta_ops = Some(ops);
        true
    }

    /// FMT-9c.2: catalog row `idx` changed (v9 writes it in the next
    /// commit's delta run). No-op on v8, which rewrites the whole catalog.
    fn touch(&mut self, idx: usize) {
        if self.is_v9() {
            self.dirty_rows.insert(idx);
        }
    }

    /// FMT-9c.2: write this commit's catalog change as a delta run on the
    /// chain, rather than a new base? Not after a restore, not before the
    /// first base, and not once the chain has 64 runs or holds more than a
    /// quarter of the base's rows (the next base then folds it in).
    fn catalog_as_delta(&self) -> bool {
        self.is_v9()
            && !self.catalog_rewrite
            && self.header.catalog_pages > 0
            && self.delta_chain.len() < 64
            && (self.delta_rows + self.dirty_rows.len()) * 4 <= self.catalog.len()
    }

    /// FMT-9c.2: before writing a new base, drop deleted rows once they're
    /// more than a quarter of the catalog. Rows move, so the id index and
    /// the page maps are rebuilt, and the resident column reloads lazily.
    /// Runs inside a flush, after packing, when nothing is waiting.
    fn compact_tombstones(&mut self) {
        let dead = self.catalog.iter().filter(|e| e.deleted).count();
        if dead == 0 || dead * 4 <= self.catalog.len() {
            return;
        }
        self.catalog.retain(|e| !e.deleted);
        self.catalog.shrink_to_fit();
        self.index = build_id_index(&self.catalog);
        self.col = VecColumn::from_catalog(&self.catalog);
        self.recs = RecPages::from_catalog(&self.catalog);
        self.dirty_rows.clear();
        self.bm25.stats = None;
    }

    /// FMT-9b: the file keeps float vectors in its records.
    fn float_vectors(&self) -> bool {
        self.is_v9() && self.header.flags & FLAG_FLOAT_VECTORS != 0
    }

    /// FMT-9b: quantize `v` into the column for catalog row `idx` (written
    /// by the next flush).
    fn stage_vector(&mut self, idx: usize, v: &[f32]) {
        let (h, norm) = veccol::quantize(v);
        let d = self.dimensions;
        if self.col.resident {
            let rows = self.catalog.len();
            let fits = rows * veccol::entry_len(d) <= self.vector_cache_budget;
            if fits {
                if self.col.data.len() < rows * d {
                    self.col.data.resize(rows * d, 0);
                    self.col.norms.resize(rows, 0.0);
                    self.col.have.resize(rows, false);
                }
                self.col.data[idx * d..(idx + 1) * d].copy_from_slice(&h);
                self.col.norms[idx] = norm;
                self.col.have[idx] = true;
            } else {
                // Grown past the budget: page the column from now on.
                self.col.data = Vec::new();
                self.col.norms = Vec::new();
                self.col.have = Vec::new();
                self.col.resident = false;
            }
        }
        self.col.hot.insert(idx, (h, norm));
        self.col.pending.push(idx);
        self.touch(idx);
        if let Some(r) = self.catalog[idx].v9.as_mut() {
            r.vec_page = 0;
            r.vec_slot = 0;
            r.vec_gen = 0;
        }
    }

    /// FMT-9b: row `idx` no longer needs its column entry (`row` is the row
    /// as it was): drop a staged one, or take it off its page and free the
    /// page once nothing live is left on it.
    fn kill_vector(&mut self, idx: usize, row: &CatalogEntry) {
        self.col.hot.remove(&idx);
        let Some(r) = row.v9 else { return };
        if r.vec_page == 0 {
            return;
        }
        let empty = match self.col.page_rows.get_mut(&r.vec_page) {
            Some(rows) => {
                rows.retain(|&i| i != idx);
                rows.is_empty()
            }
            None => false,
        };
        if empty {
            self.col.page_rows.remove(&r.vec_page);
            self.release_pages(r.vec_page, 1);
        }
    }

    /// FMT-9b.2: row `idx` no longer needs its record (`row` is the row as
    /// it was): drop a waiting one, free a span, or take it off its packed
    /// page and free the page once nothing live is left on it.
    fn kill_record(&mut self, idx: usize, row: &CatalogEntry) {
        self.recs.hot.remove(&idx);
        let Some(r) = row.v9 else { return };
        if r.rec_slot == REC_SPAN {
            self.release_pages(row.start_page, row.page_count as u64);
            return;
        }
        if row.start_page == 0 {
            return;
        }
        let empty = match self.recs.page_rows.get_mut(&row.start_page) {
            Some(rows) => {
                rows.retain(|&i| i != idx);
                rows.is_empty()
            }
            None => false,
        };
        if empty {
            self.recs.page_rows.remove(&row.start_page);
            self.release_pages(row.start_page, 1);
        }
    }

    /// FMT-9b.2: pack the waiting records, plus the live records of up to
    /// [`VACUUM_PAGES_PER_COMMIT`] sparse pages (under 3/4 full), into
    /// fresh record pages, filling each before starting the next. Runs at
    /// the top of a flush, before the catalog is serialized (it moves rows'
    /// record locations), and only when something is waiting.
    fn pack_records(&mut self) -> Result<()> {
        if !self.is_v9() || self.recs.pending.is_empty() {
            return Ok(());
        }
        let mut sparse: Vec<(u64, usize)> = self
            .recs
            .page_rows
            .iter()
            .filter(|(_, rows)| {
                let live: usize = rows
                    .iter()
                    .map(|&i| self.catalog[i].len as usize + REC_SLOT_BYTES)
                    .sum();
                live * 4 < REC_PAGE_CAP * 3
            })
            .map(|(&p, rows)| (p, rows.len()))
            .collect();
        sparse.sort_unstable();
        let sparse = vacuum_pick(sparse);
        let mut order: Vec<usize> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for p in sparse {
            let rows = self.recs.page_rows.remove(&p).unwrap_or_default();
            if let Some(&first) = rows.first() {
                let page = self.pager.read_page_uncached(p, self.catalog[first].gen)?;
                for idx in rows {
                    let slot = self.catalog[idx].v9.map_or(0, |r| r.rec_slot);
                    let b = record_slot(&page, slot)?;
                    self.recs.hot.insert(idx, b);
                    if seen.insert(idx) {
                        order.push(idx);
                    }
                }
            }
            self.release_pages(p, 1);
        }
        let pending = std::mem::take(&mut self.recs.pending);
        for idx in pending {
            if self.recs.hot.contains_key(&idx) && seen.insert(idx) {
                order.push(idx);
            }
        }
        let gen = self.write_gen();
        let mut i = 0;
        while i < order.len() {
            let mut used = 0usize;
            let mut j = i;
            while j < order.len() {
                let l = self.recs.hot[&order[j]].len() + REC_SLOT_BYTES;
                if j > i && used + l > REC_PAGE_CAP {
                    break;
                }
                used += l;
                j += 1;
            }
            let page = self.alloc_pages(1);
            let bytes = {
                let recs: Vec<&[u8]> = order[i..j]
                    .iter()
                    .map(|k| self.recs.hot[k].as_slice())
                    .collect();
                encode_record_page(gen, &recs)
            };
            self.pager.write_page(page, gen, &bytes)?;
            for (slot, &k) in order[i..j].iter().enumerate() {
                self.touch(k);
                let e = &mut self.catalog[k];
                e.start_page = page;
                e.page_count = 1;
                e.gen = gen;
                if let Some(r) = e.v9.as_mut() {
                    r.rec_slot = slot as u16;
                }
            }
            self.recs.page_rows.insert(page, order[i..j].to_vec());
            i = j;
        }
        for k in order {
            self.recs.hot.remove(&k);
        }
        self.dirty_catalog = true;
        Ok(())
    }

    /// FMT-9b: row `e`'s column entry (fp16 values and norm), from the
    /// staged set, the resident column, or its page.
    fn column_entry(&mut self, e: &CatalogEntry) -> Result<(Vec<u16>, f32)> {
        let d = self.dimensions;
        if let Some(idx) = self.index.get(e.id, &self.catalog) {
            if let Some((h, n)) = self.col.hot.get(&idx) {
                return Ok((h.clone(), *n));
            }
            if self.col.resident && self.col.have.get(idx).copied().unwrap_or(false) {
                return Ok((
                    self.col.data[idx * d..(idx + 1) * d].to_vec(),
                    self.col.norms[idx],
                ));
            }
        }
        let r =
            e.v9.ok_or_else(|| MnemoError::Invalid("v9 row without a vector entry".into()))?;
        if r.vec_page == 0 {
            return Err(MnemoError::Invalid("vector entry not written yet".into()));
        }
        let page = self.pager.read_page(r.vec_page, r.vec_gen)?;
        veccol::decode_entry(&page, d, r.vec_slot as usize)
    }

    /// FMT-9b: make the column resident if it fits the budget: allocate it
    /// (zeroed, so untouched rows cost no memory) and copy in the staged
    /// entries. Rows fill a vector page at a time on first use
    /// ([`Mnemo::fill_column_page`]). A no-op once resident, and on v8.
    fn ensure_column(&mut self) -> Result<()> {
        if !self.is_v9() || self.col.resident {
            return Ok(());
        }
        let d = self.dimensions;
        let rows = self.catalog.len();
        if rows * veccol::entry_len(d) > self.vector_cache_budget {
            return Ok(());
        }
        self.col.data = vec![0u16; rows * d];
        self.col.norms = vec![0f32; rows];
        self.col.have = vec![false; rows];
        let hot: Vec<(usize, Vec<u16>, f32)> = self
            .col
            .hot
            .iter()
            .filter(|(&i, _)| i < rows)
            .map(|(&i, (h, n))| (i, h.clone(), *n))
            .collect();
        for (i, h, n) in hot {
            self.col.data[i * d..(i + 1) * d].copy_from_slice(&h);
            self.col.norms[i] = n;
            self.col.have[i] = true;
        }
        self.col.resident = true;
        Ok(())
    }

    /// FMT-9b: fill the resident column from row `idx`'s vector page: every
    /// live row on that page, in one decrypt that bypasses the page cache.
    fn fill_column_page(&mut self, idx: usize) -> Result<()> {
        let Some(r) = self.catalog[idx].v9 else {
            return Ok(());
        };
        if r.vec_page == 0 {
            return Ok(());
        }
        let d = self.dimensions;
        let bytes = self.pager.read_page_uncached(r.vec_page, r.vec_gen)?;
        let rows = self
            .col
            .page_rows
            .get(&r.vec_page)
            .cloned()
            .unwrap_or_default();
        for i in rows.into_iter().chain(std::iter::once(idx)) {
            if i >= self.col.have.len() || self.col.have[i] {
                continue;
            }
            let Some(ri) = self.catalog[i].v9 else {
                continue;
            };
            if ri.vec_page != r.vec_page {
                continue;
            }
            let n = veccol::decode_entry_into(
                &bytes,
                d,
                ri.vec_slot as usize,
                &mut self.col.data[i * d..(i + 1) * d],
            )?;
            self.col.norms[i] = n;
            self.col.have[i] = true;
        }
        Ok(())
    }

    /// FMT-9b: write the staged column entries, plus the live entries of up
    /// to [`VACUUM_PAGES_PER_COMMIT`] sparse pages (under 3/4 full), into
    /// fresh vector pages. Runs at the top of a flush, before the catalog
    /// is serialized, since it moves rows' vector locations. Only when
    /// something is staged, so a flush with no new vectors writes none.
    fn pack_vectors(&mut self) -> Result<()> {
        if !self.is_v9() || self.col.pending.is_empty() {
            return Ok(());
        }
        let d = self.dimensions;
        let per = veccol::per_page(d);
        let mut sparse: Vec<(u64, usize)> = self
            .col
            .page_rows
            .iter()
            .filter(|(_, rows)| rows.len() * 4 < per * 3)
            .map(|(&p, rows)| (p, rows.len()))
            .collect();
        sparse.sort_unstable();
        let sparse = vacuum_pick(sparse);
        let mut order: Vec<usize> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for p in sparse {
            let rows = self.col.page_rows.remove(&p).unwrap_or_default();
            for idx in rows {
                let e = self.catalog[idx].clone();
                let v = self.column_entry(&e)?;
                self.col.hot.insert(idx, v);
                if seen.insert(idx) {
                    order.push(idx);
                }
            }
            self.release_pages(p, 1);
        }
        let pending = std::mem::take(&mut self.col.pending);
        for idx in pending {
            if self.col.hot.contains_key(&idx) && seen.insert(idx) {
                order.push(idx);
            }
        }
        let gen = self.write_gen();
        for chunk in order.chunks(per) {
            let page = self.alloc_pages(1);
            let bytes = {
                let entries: Vec<(&[u16], f32)> = chunk
                    .iter()
                    .map(|i| {
                        let (h, n) = &self.col.hot[i];
                        (h.as_slice(), *n)
                    })
                    .collect();
                veccol::encode_page(d, gen, &entries)
            };
            self.pager.write_page(page, gen, &bytes)?;
            for (slot, &i) in chunk.iter().enumerate() {
                self.touch(i);
                if let Some(r) = self.catalog[i].v9.as_mut() {
                    r.vec_page = page;
                    r.vec_slot = slot as u16;
                    r.vec_gen = gen;
                }
            }
            self.col.page_rows.insert(page, chunk.to_vec());
        }
        for i in order {
            self.col.hot.remove(&i);
        }
        self.dirty_catalog = true;
        Ok(())
    }

    /// FMT-9: before this session seals its first v9 page, commit a
    /// header-only transaction that bumps `wal_seq`.
    ///
    /// A page's generation is the id of the transaction that writes it,
    /// and that id is `wal_seq + 1`, which is only made durable by the
    /// commit. If a session crashes before committing, its pages (orphan
    /// data pages, or uncommitted WAL frames) carry an id the next session
    /// would hand out again, for the same pages (allocation is
    /// deterministic). An attacker holding a copy of the crashed file could
    /// then swap an orphan back in under a matching generation. Burning
    /// one id durably per writable session rules that out: every id this
    /// session uses is above every id any earlier session used. Costs one
    /// small commit per session, on its first write only.
    fn ensure_gen_lease(&mut self) -> Result<()> {
        if !self.is_v9() || self.gen_leased || self.read_only {
            return Ok(());
        }
        self.header.wal_seq += 1;
        let wal_start = self.header.wal_start;
        self.relocate_header_through_wal(wal_start)?;
        self.gen_leased = true;
        Ok(())
    }

    /// Write a serialized record (memory or cache entry) to fresh pages.
    /// Returns `(start_page, page_count, generation)`.
    fn write_record(&mut self, bytes: &[u8]) -> Result<(u64, u32, u64)> {
        self.ensure_gen_lease()?;
        let pc = bytes.len().div_ceil(PAYLOAD).max(1);
        let start = self.alloc_pages(pc as u64);
        let gen = self.write_gen();
        for i in 0..pc {
            let lo = i * PAYLOAD;
            let hi = ((i + 1) * PAYLOAD).min(bytes.len());
            self.pager
                .write_page(start + i as u64, gen, &bytes[lo..hi])?;
        }
        Ok((start, pc as u32, gen))
    }

    /// Read a record run of `pages` pages at `start`, sealed under `gen`.
    fn read_record_run(&mut self, start: u64, pages: u32, len: u32, gen: u64) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(len as usize);
        for i in 0..pages as u64 {
            buf.extend_from_slice(&self.pager.read_page(start + i, gen)?);
        }
        buf.truncate(len as usize);
        Ok(buf)
    }

    fn read_record(&mut self, e: &CatalogEntry) -> Result<Vec<u8>> {
        // FMT-9b.2: a packed v9 record is one slot of a shared page, or
        // still waiting for the next flush.
        if let Some(r) = e.v9 {
            if r.rec_slot != REC_SPAN {
                if e.start_page == 0 {
                    return self
                        .index
                        .get(e.id, &self.catalog)
                        .and_then(|idx| self.recs.hot.get(&idx).cloned())
                        .ok_or_else(|| MnemoError::Invalid("record not written yet".into()));
                }
                let page = self.pager.read_page(e.start_page, e.gen)?;
                return record_slot(&page, r.rec_slot);
            }
        }
        let mut buf = Vec::with_capacity(e.len as usize);
        for i in 0..e.page_count as u64 {
            buf.extend_from_slice(&self.pager.read_page(e.start_page + i, e.gen)?);
        }
        buf.truncate(e.len as usize);
        Ok(buf)
    }

    fn read_memory(&mut self, e: &CatalogEntry) -> Result<Memory> {
        let bytes = self.read_record(e)?;
        let mut m: Memory =
            rmp_serde::from_slice(&bytes).map_err(|err| MnemoError::Serialize(err.to_string()))?;
        // FMT-9b: a v9 record carries no vector (unless the file keeps
        // float vectors); it comes from the column, fp16-rounded.
        if e.v9.is_some() && !self.float_vectors() {
            let (h, _) = self.column_entry(e)?;
            m.vector = veccol::dequantize(&h);
        }
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
        // R-004.2: an overwrite with the handle caches built reads the
        // stored version first (one decrypt), so the caches drop exactly
        // what it contributed.
        if self.layers.is_some() {
            if let Some(idx) = self.index.get(id_u, &self.catalog) {
                let old_entry = self.catalog[idx].clone();
                if !old_entry.deleted {
                    let old = self.read_memory(&old_entry)?;
                    self.note_removed(&old);
                }
            }
        }
        // FMT-9b: a v9 record is stored without its vector (unless the file
        // keeps float vectors); the vector goes to the fp16 column.
        let v9 = self.is_v9();
        let bytes = if v9 && !self.float_vectors() {
            let vector = std::mem::take(&mut m.vector);
            let b = rmp_serde::to_vec(&m);
            m.vector = vector;
            b
        } else {
            rmp_serde::to_vec(&m)
        }
        .map_err(|e| MnemoError::Serialize(e.to_string()))?;
        // FMT-9b.2: a v9 record that fits a packed page waits for the next
        // flush, which packs it with others; a bigger one owns a run of
        // whole pages, written now as on v8.
        let packed = v9 && bytes.len() + REC_SLOT_BYTES <= REC_PAGE_CAP;
        // A waiting record has `start_page` 0. Its generation is never 0 on
        // v9: the row encoding leaves out a zero `gen`, which would shift
        // the `v9` field into its place (a deleted row can stay unpacked).
        let (start, pc, gen) = if packed {
            self.ensure_gen_lease()?;
            (0, 1, self.write_gen())
        } else {
            self.write_record(&bytes)?
        };
        let rec_len = bytes.len();
        // FMT-9d: a keyword-indexed v9 row gets a new BM25 document.
        let mut tags = v9.then(|| row_tags(&m));
        if let Some(r) = tags.as_mut() {
            if bm25_kind(r.kind) {
                let (terms, len) = text::term_freqs(&m.content);
                r.doc = self.bm25.manifest.next_doc;
                r.doc_len = len;
                self.bm25.manifest.next_doc += 1;
                self.bm25.pending.push((r.doc, terms));
            }
        }
        self.bm25.stats = None;
        // For overwrites preserve the existing access stats; for inserts
        // seed them from the memory (which carries them in its body).
        let (prev_accessed, prev_count) = match self.index.get(id_u, &self.catalog) {
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
            len: rec_len as u32,
            deleted: false,
            accessed_at: prev_accessed,
            access_count: prev_count,
            gen,
            v9: tags.map(|mut r| {
                r.rec_slot = if packed { 0 } else { REC_SPAN };
                r
            }),
        };
        match self.index.get(id_u, &self.catalog) {
            Some(idx) => {
                // FMT-9 (a): the overwritten version's pages are freed by
                // this transaction's commit. A tombstone's pages were
                // already freed when its delete committed.
                if !self.catalog[idx].deleted {
                    let old = self.catalog[idx].clone();
                    if v9 {
                        self.kill_vector(idx, &old);
                        self.kill_record(idx, &old);
                    } else {
                        self.release_pages(old.start_page, old.page_count as u64);
                    }
                }
                self.catalog[idx] = entry;
                self.touch(idx);
            }
            None => {
                self.index.insert(id_u, self.catalog.len(), &self.catalog);
                self.touch(self.catalog.len());
                self.catalog.push(entry);
            }
        }
        self.dirty_catalog = true;
        if v9 {
            if let Some(idx) = self.index.get(id_u, &self.catalog) {
                self.stage_vector(idx, &m.vector);
                if packed {
                    self.recs.hot.insert(idx, bytes);
                    self.recs.pending.push(idx);
                }
            }
        }

        // Keep the ANN index complete: assign the (new or changed) vector to
        // its nearest partition. Centroids/codebook stay fixed until rebuild.
        // R-004.2: only facts go in; turns, essence items and thread links
        // are read by kind and scope.
        if let Some(ann) = &mut self.ann {
            if is_ann_kind(&m) {
                ann.add(id_u, &m.vector);
                self.dirty_index = true;
                self.index_persist.changes += 1;
            } else if ann.contains(id_u) {
                ann.remove(id_u);
                self.dirty_index = true;
                self.index_persist.changes += 1;
            }
        }
        self.note_added(&m);

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
        let idx = self
            .index
            .get(id.0, &self.catalog)
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
        let idx = self
            .index
            .get(id.0, &self.catalog)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        // R-004.2 / 12.1: with the handle caches built, read the record
        // being deleted (one decrypt) so the key slot, superseded set and
        // thread layers drop exactly what it contributed.
        let entry = self.catalog[idx].clone();
        if self.layers.is_some() && !entry.deleted {
            let old = self.read_memory(&entry)?;
            self.note_removed(&old);
        }
        if !self.catalog[idx].deleted {
            self.catalog[idx].deleted = true;
            self.dirty_catalog = true;
            self.touch(idx);
            self.bm25.stats = None;
            if self.is_v9() {
                self.kill_record(idx, &entry);
                self.kill_vector(idx, &entry);
            } else {
                self.release_pages(entry.start_page, entry.page_count as u64);
            }
        }
        if let Some(ann) = &mut self.ann {
            if ann.contains(id.0) {
                ann.remove(id.0);
                self.dirty_index = true;
                self.index_persist.changes += 1;
            }
        }
        // FIX-1 R-001.8 part 2: we can't tell whether this delete
        // removed the last memory of its agent without a decrypt
        // (the catalog entry doesn't carry `agent_id` in format v8),
        // so just invalidate the lazy set. The next `stats()` call
        // rebuilds from live memories.
        self.agent_set = None;
        // Phase 12.1: delete of a keyed record removes it from the slot,
        // and delete of a superseded record frees a `history` entry. When
        // the caches are built, `note_removed` above did that exactly;
        // otherwise they're built from scratch on first use anyway.
        Ok(())
    }

    /// Keep the handle caches (12.1 key slots, superseded set, R-004.2
    /// layers) current after `put` writes `m`. No-op until they're built.
    fn note_added(&mut self, m: &Memory) {
        let id = m.id.0;
        if let (Some(keys), Some(sup)) = (self.key_index.as_mut(), self.superseded.as_mut()) {
            if m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) {
                sup.insert(id);
            } else if let Some(Value::String(k)) = m.metadata.get(MNEMO_META_KEY) {
                keys.insert(Self::key_slot_for(m, k), id);
            }
        }
        if let Some(layers) = self.layers.as_mut() {
            layers.add(m);
        }
    }

    /// Undo [`Mnemo::note_added`] for the stored version of a record that's
    /// about to be overwritten or deleted.
    fn note_removed(&mut self, m: &Memory) {
        let id = m.id.0;
        if let (Some(keys), Some(sup)) = (self.key_index.as_mut(), self.superseded.as_mut()) {
            sup.remove(&id);
            if let Some(Value::String(k)) = m.metadata.get(MNEMO_META_KEY) {
                let slot = Self::key_slot_for(m, k);
                if keys.get(&slot) == Some(&id) {
                    keys.remove(&slot);
                }
            }
        }
        if let Some(layers) = self.layers.as_mut() {
            layers.remove(m);
        }
    }

    /// The live catalog entries for `ids` (ids that are gone are skipped).
    fn live_entries(&self, ids: impl IntoIterator<Item = u128>) -> Vec<CatalogEntry> {
        ids.into_iter()
            .filter_map(|id| {
                self.index
                    .get(id, &self.catalog)
                    .map(|i| self.catalog[i].clone())
            })
            .filter(|e| !e.deleted)
            .collect()
    }

    /// Catalog entries of the essence items, from the handle caches.
    fn essence_entries(&mut self) -> Result<Vec<CatalogEntry>> {
        self.ensure_key_caches()?;
        let ids: Vec<u128> = self
            .layers
            .as_ref()
            .map(|l| l.essence.iter().copied().collect())
            .unwrap_or_default();
        Ok(self.live_entries(ids))
    }

    /// Catalog entries of every record filed under `thread`, from the
    /// handle caches.
    fn thread_entries(&mut self, thread: &str) -> Result<Vec<CatalogEntry>> {
        self.ensure_key_caches()?;
        let ids: Vec<u128> = self
            .layers
            .as_ref()
            .and_then(|l| l.threads.get(thread))
            .map(|tc| tc.records.iter().copied().collect())
            .unwrap_or_default();
        Ok(self.live_entries(ids))
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

    /// The handle's one lazy scan: builds the 12.1 `key_index` +
    /// `superseded` caches and the R-004.2 layer cache (essence ids,
    /// onboarding ids, threads, links) from live records, and prunes
    /// non-fact ids from a loaded ANN index (files written before v0.10.0
    /// indexed every kind). Decrypts every live record once per handle;
    /// `put` and `delete` keep the result current after that.
    fn ensure_key_caches(&mut self) -> Result<()> {
        if self.key_index.is_some() && self.superseded.is_some() && self.layers.is_some() {
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
        let mut layers = LayerCache::default();
        let mut non_facts: Vec<u128> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            layers.add(&m);
            if !is_ann_kind(&m) {
                non_facts.push(e.id);
            }
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
        self.layers = Some(layers);
        if let Some(ann) = &mut self.ann {
            let mut pruned = 0usize;
            for id in non_facts {
                if ann.contains(id) {
                    ann.remove(id);
                    pruned += 1;
                }
            }
            if pruned > 0 {
                self.dirty_index = true;
                self.index_persist.changes += pruned;
            }
        }
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
            let prev_idx = self
                .index
                .get(prev_id_u, &self.catalog)
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
        let idx = self
            .index
            .get(id.0, &self.catalog)
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
        // `put` kept the key slot and the other handle caches current.
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
            let idx = self.index.get(id_u, &self.catalog).ok_or_else(|| {
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
        let plan = self.plan_ingest(thread_id, turns)?;
        if plan.pending.is_empty() {
            return Ok(plan.report);
        }
        // Batched embed: one round trip to the embedder for the whole
        // ingest, whatever the turn count.
        let vectors = {
            let embedder = self.embedder.as_ref().ok_or_else(|| {
                MnemoError::Invalid(
                    "ingest_thread: no embedder registered on this handle; call set_embedder first"
                        .into(),
                )
            })?;
            embedder.embed(&plan.texts())?
        };
        self.check_vectors("ingest_thread", &vectors, plan.text_count())?;
        self.apply_ingest(thread_id, plan, vectors, None)
    }

    /// Check a batch of embedder output before anything is written:
    /// one vector per input, the file's width, finite and non-zero.
    fn check_vectors(&self, ctx: &str, vectors: &[Vec<f32>], expected: usize) -> Result<()> {
        if vectors.len() != expected {
            return Err(MnemoError::Invalid(format!(
                "{ctx}: embedder returned {} vectors for {expected} texts",
                vectors.len()
            )));
        }
        for v in vectors {
            self.check_vector(ctx, v)?;
        }
        Ok(())
    }

    /// The checks `put` applies to one vector, run up front so a batch
    /// can't fail half-way through its writes.
    fn check_vector(&self, ctx: &str, v: &[f32]) -> Result<()> {
        if v.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: v.len(),
            });
        }
        if embed::is_zero_vector(v) {
            return Err(MnemoError::Invalid(format!(
                "{ctx}: all-zero vector refused"
            )));
        }
        if v.iter().any(|x| !x.is_finite()) {
            return Err(MnemoError::Invalid(format!(
                "{ctx}: vector contains a non-finite value"
            )));
        }
        Ok(())
    }

    /// Planning half of [`Mnemo::ingest_thread`]: validate the turns, skip
    /// the ones already archived, and chunk or truncate the rest. Reads
    /// only; nothing is written.
    fn plan_ingest(&mut self, thread_id: &str, turns: &[ThreadTurn]) -> Result<IngestPlan> {
        if thread_id.is_empty() {
            return Err(MnemoError::Invalid(
                "ingest_thread: 'thread_id' must not be empty".into(),
            ));
        }
        // FMT-9b.2: v9 chunks fit one packed page with their metadata.
        let chunk_bytes = if self.is_v9() {
            V9_TURN_CHUNK_BYTES
        } else {
            THREAD_TURN_CHUNK_BYTES
        };
        let mut report = IngestReport::default();
        if turns.is_empty() {
            return Ok(IngestPlan {
                pending: Vec::new(),
                next_seq: 0,
                report,
            });
        }

        // Build the set of turn_ids already in this thread so the
        // idempotence check is O(1) per incoming turn. First-pass
        // cost: one decrypt per existing thread record. A future v9
        // format bump can move `_mnemo.turn_id` into the catalog row
        // and drop this scan.
        // R-004.2: the turn ids already archived and the next `_mnemo.seq`
        // come from the handle caches: one lazy scan per handle, then O(1)
        // per ingest instead of two full decrypt scans each time.
        self.ensure_key_caches()?;
        let cached = self.layers.as_ref().and_then(|l| l.threads.get(thread_id));
        let existing_turn_ids: std::collections::HashSet<&str> = cached
            .map(|tc| tc.turns.keys().map(String::as_str).collect())
            .unwrap_or_default();
        // A repeat ingest continues the sequence instead of restarting at 0.
        let next_seq = cached.and_then(|tc| tc.max_seq).map_or(0, |n| n + 1);

        // Determine each incoming turn's final text pieces (post-
        // chunking / truncation) before any writes, so the caller can
        // embed the whole batch in one `embed()` call.
        let mut pending: Vec<PendingTurn> = Vec::new();
        let now = memory::now_secs();
        for t in turns {
            if t.turn_id.is_empty() {
                return Err(MnemoError::Invalid(
                    "ingest_thread: turn_id must not be empty".into(),
                ));
            }
            if existing_turn_ids.contains(t.turn_id.as_str()) {
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
                } else if content_bytes > chunk_bytes {
                    report.chunked_turns += 1;
                    (chunk_utf8(&t.content, chunk_bytes), None)
                } else {
                    (vec![t.content.clone()], None)
                };
            pending.push(PendingTurn {
                turn_id: t.turn_id.clone(),
                role: t.role.clone(),
                created_at: t.timestamp.unwrap_or(now),
                parts,
                truncation,
            });
        }
        Ok(IngestPlan {
            pending,
            next_seq,
            report,
        })
    }

    /// Write half of [`Mnemo::ingest_thread`]. `vectors` holds one vector
    /// per text of `plan.texts()`, already checked. `ttl` stamps every
    /// new record (the `commit_deltas` `ExpireAfter` policy).
    fn apply_ingest(
        &mut self,
        thread_id: &str,
        plan: IngestPlan,
        vectors: Vec<Vec<f32>>,
        ttl: Option<i64>,
    ) -> Result<IngestReport> {
        let IngestPlan {
            pending,
            mut next_seq,
            mut report,
        } = plan;
        // Order: each pending turn's chunks in part order; each turn
        // gets a single `_mnemo.seq` slot, so `_mnemo.part`
        // disambiguates chunks of the same turn.
        let mut vectors = vectors.into_iter();
        for p in &pending {
            let this_seq = next_seq;
            next_seq += 1;
            for (part_idx, part_text) in p.parts.iter().enumerate() {
                let vec = vectors.next().ok_or_else(|| {
                    MnemoError::Invalid("ingest_thread: ran out of vectors".into())
                })?;
                let mut m =
                    Memory::new(part_text.clone(), MemoryType::Episodic, vec).with_importance(0.5);
                if let Some(t) = ttl {
                    m = m.with_ttl(t);
                }
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

    /// Export the archive layer for `thread_id` in `_mnemo.seq`
    /// order. Each entry is the Memory as stored; chunks of a
    /// multi-part turn come out in `_mnemo.part` order within their
    /// `_mnemo.seq`. The record `.content` is the text that was
    /// stored (post-chunking / post-truncation); callers that need
    /// the pre-truncation original should round-trip via
    /// `_mnemo.sha256` + `_mnemo.length`.
    pub fn thread_export(&mut self, thread_id: &str) -> Result<Vec<Memory>> {
        // R-004.2: the thread's own records, from the handle caches.
        let entries = self.thread_entries(thread_id)?;
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

    /// Export a thread's archive in the **ingest shape** (Phase 12.7d):
    /// one [`ExportedTurn`] per source turn, in `_mnemo.seq` order, with
    /// a chunked turn's parts joined back into one `content`. Passing
    /// the result back to [`Mnemo::ingest_thread`] for the same thread
    /// skips every turn; for a new thread it re-creates the same records.
    /// This is what Python `db.thread_export`, MCP `thread_export` and
    /// `mnemo thread export` return; [`Mnemo::thread_export`] still
    /// returns the raw records.
    pub fn thread_export_turns(&mut self, thread_id: &str) -> Result<Vec<ExportedTurn>> {
        let records = self.thread_export(thread_id)?;
        let mut out: Vec<ExportedTurn> = Vec::new();
        let mut last_seq: Option<u64> = None;
        for m in records {
            let meta_str = |k: &str| {
                m.metadata
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string()
            };
            let seq = m.metadata.get(MNEMO_META_SEQ).and_then(|v| v.as_u64());
            // Records are sorted by (seq, part), so the parts of one turn
            // are adjacent and share a seq.
            if seq.is_some() && seq == last_seq {
                if let Some(prev) = out.last_mut() {
                    prev.content.push_str(&m.content);
                    continue;
                }
            }
            last_seq = seq;
            out.push(ExportedTurn {
                turn_id: meta_str(MNEMO_META_TURN_ID),
                role: meta_str(MNEMO_META_ROLE),
                timestamp: m.created_at,
                sha256: m
                    .metadata
                    .get(MNEMO_META_SHA256)
                    .and_then(|v| v.as_str())
                    .map(String::from),
                original_length: m.metadata.get(MNEMO_META_LENGTH).and_then(|v| v.as_u64()),
                content: m.content,
            });
        }
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
        let entries = self.thread_entries(thread_id)?;
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
        Self::check_essence_add(section, text)?;
        self.essence_add_with(scope, section, text, refs, None)
    }

    /// Validation shared by [`Mnemo::essence_add`] and `commit_deltas`.
    fn check_essence_add(section: EssenceSection, text: &str) -> Result<()> {
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
        Ok(())
    }

    /// Write half of `essence_add`, with an optional precomputed vector.
    fn essence_add_with(
        &mut self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
        vector: Option<Vec<f32>>,
    ) -> Result<Ulid> {
        let now = memory::now_secs();
        let mem = self.build_essence_record(scope, section, text, refs, Some(now), vector)?;
        self.put(mem)
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
        Self::check_essence_append(text)?;
        self.essence_append_with(scope, text, refs, None)
    }

    /// Validation shared by [`Mnemo::essence_append_timeline`] and
    /// `commit_deltas`.
    fn check_essence_append(text: &str) -> Result<()> {
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
        Ok(())
    }

    /// Write half of `essence_append_timeline`, with an optional
    /// precomputed vector.
    fn essence_append_with(
        &mut self,
        scope: EssenceScope,
        text: &str,
        refs: Option<Vec<String>>,
        vector: Option<Vec<f32>>,
    ) -> Result<Ulid> {
        let mem =
            self.build_essence_record(scope, EssenceSection::Timeline, text, refs, None, vector)?;
        self.put(mem)
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
        // R-004.2: only the essence items, from the handle caches.
        let entries = self.essence_entries()?;
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

    /// Build an essence record. `vector` is a precomputed embedding (the
    /// 12.7d `commit_deltas` batch path); `None` embeds `text` with the
    /// registered embedder, or falls back to an axis-neutral unit vector
    /// when there is none. Essence records are read by `_mnemo.kind` and
    /// scope, not by similarity, so the fallback costs no retrieval
    /// quality; it only has to pass `put`'s non-zero guard.
    fn build_essence_record(
        &self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
        status_at: Option<i64>,
        vector: Option<Vec<f32>>,
    ) -> Result<Memory> {
        let v = match (vector, self.embedder.as_ref()) {
            (Some(v), _) => v,
            (None, Some(e)) => e.embed(&[text])?.pop().ok_or_else(|| {
                MnemoError::Invalid("essence: embedder returned no vector".into())
            })?,
            (None, None) => self.sentinel_vector(),
        };
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

    /// Axis-neutral unit vector (every component `1/sqrt(dims)`), used by
    /// essence records without an embedder and by thread-link markers.
    fn sentinel_vector(&self) -> Vec<f32> {
        let dims = self.dimensions.max(1);
        vec![1.0 / (dims as f32).sqrt(); dims]
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
        // queryable from a fresh handle. The sentinel vector passes
        // `put`'s zero-vector guard; default recall skips link records.
        let v = self.sentinel_vector();
        let content = format!("thread link: {new_thread_id} continues from {from_thread_id}");
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
        let entries = self.essence_entries()?;
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
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(prev_thread) {
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
        self.ensure_key_caches()?;
        Ok(self
            .layers
            .as_ref()
            .and_then(|l| l.links.get(thread_id))
            .map(|(from, _)| from.clone()))
    }

    /// Atomic batch write combining 12.1 facts, 12.7b essence ops,
    /// and 12.7a archive turns in one flush (Phase 12.7c; batch
    /// embedding added in 12.7d).
    ///
    /// The batch runs in four steps:
    ///
    /// 1. **Structural checks** on every item: required fields, size
    ///    caps, empty keys, `thread_id` for turns, and that every
    ///    `Update` / `Resolve` / `Remove` targets a live essence record
    ///    the op can apply to.
    /// 2. **One `Embedder::embed` call** for every text that needs a
    ///    vector: facts whose `vector` is empty, essence `Add` /
    ///    `AppendTimeline` texts (when an embedder is registered;
    ///    without one, essence keeps its sentinel vector), and the
    ///    archive turns' chunks.
    /// 3. **Vector checks** (width, non-zero, finite) on supplied and
    ///    embedded vectors alike.
    /// 4. **Writes**: facts, then essence ops, then turns per
    ///    `turn_policy`, then exactly one `flush()`.
    ///
    /// A failure in steps 1–3 returns an error with nothing staged. A
    /// text-only fact, or any turn to archive, with no embedder
    /// registered is a step-1 error. `put` has no rollback, so an I/O
    /// error during step 4 can leave the batch's earlier writes staged
    /// on the handle; steps 1–3 exist so nothing else can fail that
    /// late.
    pub fn commit_deltas(&mut self, batch: CommitBatch) -> Result<CommitReport> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let CommitBatch {
            mut facts,
            essence_ops,
            turns,
            thread_id,
            turn_policy,
        } = batch;
        let has_embedder = self.embedder.is_some();
        let in_ctx = |ctx: &str, e: MnemoError| MnemoError::Invalid(format!("{ctx}: {e}"));

        // --- 1. Structural checks (reads only) ----------------------
        for (i, f) in facts.iter().enumerate() {
            if let Some(k) = &f.key {
                if k.is_empty() {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] key is empty"
                    )));
                }
            }
            if f.memory.vector.is_empty() {
                if f.memory.content.is_empty() {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] has neither a vector nor content to embed"
                    )));
                }
                if !has_embedder {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] has no vector and no embedder is registered \
                         on this handle; pass a vector or call set_embedder first"
                    )));
                }
            }
        }
        let mut removed_in_batch: std::collections::HashSet<u128> =
            std::collections::HashSet::new();
        for (i, op) in essence_ops.iter().enumerate() {
            let ctx = format!("commit_deltas: essence_ops[{i}]");
            match op.kind {
                EssenceOpKind::Add => {
                    if op.scope.is_none() {
                        return Err(MnemoError::Invalid(format!("{ctx} Add missing 'scope'")));
                    }
                    let section = op.section.ok_or_else(|| {
                        MnemoError::Invalid(format!("{ctx} Add missing 'section'"))
                    })?;
                    let text = op
                        .text
                        .as_deref()
                        .ok_or_else(|| MnemoError::Invalid(format!("{ctx} Add missing 'text'")))?;
                    Self::check_essence_add(section, text).map_err(|e| in_ctx(&ctx, e))?;
                }
                EssenceOpKind::AppendTimeline => {
                    if op.scope.is_none() {
                        return Err(MnemoError::Invalid(format!(
                            "{ctx} AppendTimeline missing 'scope'"
                        )));
                    }
                    let text = op.text.as_deref().ok_or_else(|| {
                        MnemoError::Invalid(format!("{ctx} AppendTimeline missing 'text'"))
                    })?;
                    Self::check_essence_append(text).map_err(|e| in_ctx(&ctx, e))?;
                }
                EssenceOpKind::Update | EssenceOpKind::Resolve | EssenceOpKind::Remove => {
                    let id = op
                        .id
                        .ok_or_else(|| MnemoError::Invalid(format!("{ctx} needs 'id'")))?;
                    if removed_in_batch.contains(&id.0) {
                        return Err(MnemoError::Invalid(format!(
                            "{ctx}: record {id} is removed earlier in this batch"
                        )));
                    }
                    let existing = self.load_essence_record(&id).map_err(|e| in_ctx(&ctx, e))?;
                    let section = self.section_of(&existing).map_err(|e| in_ctx(&ctx, e))?;
                    match op.kind {
                        EssenceOpKind::Update => {
                            let text = op.text.as_deref().ok_or_else(|| {
                                MnemoError::Invalid(format!("{ctx} Update needs 'text'"))
                            })?;
                            if section == EssenceSection::Timeline {
                                return Err(MnemoError::Invalid(format!(
                                    "{ctx}: timeline entries are append-only"
                                )));
                            }
                            if text.is_empty() || text.len() > section.item_max_bytes() {
                                return Err(MnemoError::Invalid(format!(
                                    "{ctx}: '{}' items must be 1..={} bytes; got {}",
                                    section.as_str(),
                                    section.item_max_bytes(),
                                    text.len()
                                )));
                            }
                        }
                        EssenceOpKind::Resolve => {
                            if !section.has_status_lifecycle() {
                                return Err(MnemoError::Invalid(format!(
                                    "{ctx}: '{}' section has no status lifecycle",
                                    section.as_str()
                                )));
                            }
                        }
                        _ => {
                            removed_in_batch.insert(id.0);
                        }
                    }
                }
            }
        }
        if !turns.is_empty() && thread_id.is_none() {
            return Err(MnemoError::Invalid(
                "commit_deltas: 'turns' non-empty but 'thread_id' missing".into(),
            ));
        }
        let archive_turns = !turns.is_empty() && !matches!(turn_policy, TurnPolicy::Drop);
        if archive_turns && !has_embedder {
            return Err(MnemoError::Invalid(
                "commit_deltas: archiving 'turns' needs an embedder registered on this handle; \
                 call set_embedder first or use turn_policy Drop"
                    .into(),
            ));
        }
        let plan = match (&thread_id, archive_turns) {
            (Some(t), true) => Some(self.plan_ingest(t, &turns)?),
            _ => None,
        };

        // --- 2. One batched embed -----------------------------------
        let embeds_essence = |op: &EssenceOp| {
            has_embedder && matches!(op.kind, EssenceOpKind::Add | EssenceOpKind::AppendTimeline)
        };
        let (vectors, n_texts) = {
            let mut texts: Vec<&str> = Vec::new();
            for f in &facts {
                if f.memory.vector.is_empty() {
                    texts.push(f.memory.content.as_str());
                }
            }
            for op in essence_ops.iter().filter(|op| embeds_essence(op)) {
                texts.push(op.text.as_deref().expect("checked in step 1"));
            }
            if let Some(p) = &plan {
                texts.extend(p.texts());
            }
            let vectors = match (texts.is_empty(), self.embedder.as_ref()) {
                (true, _) => Vec::new(),
                (false, Some(e)) => e.embed(&texts)?,
                (false, None) => unreachable!("texts need an embedder; checked in step 1"),
            };
            (vectors, texts.len())
        };

        // --- 3. Vector checks ---------------------------------------
        self.check_vectors("commit_deltas", &vectors, n_texts)?;
        for (i, f) in facts.iter().enumerate() {
            let v = &f.memory.vector;
            if v.is_empty() {
                continue;
            }
            if v.len() != self.dimensions {
                return Err(MnemoError::Invalid(format!(
                    "commit_deltas: facts[{i}] vector has {} dims; this file is {}",
                    v.len(),
                    self.dimensions
                )));
            }
            let ctx = format!("commit_deltas: facts[{i}]");
            self.check_vector(&ctx, v)?;
        }

        // Hand the embedded vectors back out, in the order they were
        // requested: facts, then essence ops, then turn chunks.
        let mut vectors = vectors.into_iter();
        for f in facts.iter_mut() {
            if f.memory.vector.is_empty() {
                f.memory.vector = vectors.next().expect("one vector per text");
            }
        }
        let essence_vectors: Vec<Option<Vec<f32>>> = essence_ops
            .iter()
            .map(|op| embeds_essence(op).then(|| vectors.next().expect("one vector per text")))
            .collect();
        let turn_vectors: Vec<Vec<f32>> = vectors.collect();

        // --- 4. Writes ----------------------------------------------
        let mut report = CommitReport::default();

        for f in facts {
            match f.key {
                Some(k) => {
                    let (_id, outcome) = self.remember_keyed(f.memory, &k)?;
                    match outcome {
                        WriteOutcome::Added => report.facts_added += 1,
                        WriteOutcome::Superseded | WriteOutcome::Updated => {
                            report.facts_superseded += 1
                        }
                        WriteOutcome::Unchanged => report.facts_unchanged += 1,
                    }
                }
                None => {
                    self.remember(f.memory)?;
                    report.facts_added += 1;
                }
            }
        }

        for (op, vector) in essence_ops.into_iter().zip(essence_vectors) {
            let outcome = match op.kind {
                EssenceOpKind::Add => {
                    self.essence_add_with(
                        op.scope.expect("checked in step 1"),
                        op.section.expect("checked in step 1"),
                        op.text.as_deref().expect("checked in step 1"),
                        op.refs,
                        vector,
                    )?;
                    WriteOutcome::Added
                }
                EssenceOpKind::AppendTimeline => {
                    self.essence_append_with(
                        op.scope.expect("checked in step 1"),
                        op.text.as_deref().expect("checked in step 1"),
                        op.refs,
                        vector,
                    )?;
                    WriteOutcome::Added
                }
                EssenceOpKind::Update => self.essence_update(
                    op.id.as_ref().expect("checked in step 1"),
                    op.text.as_deref().expect("checked in step 1"),
                    op.refs,
                )?,
                EssenceOpKind::Resolve => {
                    self.essence_resolve(op.id.as_ref().expect("checked in step 1"))?
                }
                EssenceOpKind::Remove => {
                    self.essence_remove(op.id.as_ref().expect("checked in step 1"))?;
                    WriteOutcome::Updated
                }
            };
            match outcome {
                WriteOutcome::Unchanged => report.essence_ops_unchanged += 1,
                _ => report.essence_ops_applied += 1,
            }
        }

        if let (Some(plan), Some(t)) = (plan, thread_id.as_deref()) {
            let ttl = match turn_policy {
                TurnPolicy::ExpireAfter(secs) => Some(secs),
                _ => None,
            };
            let r = self.apply_ingest(t, plan, turn_vectors, ttl)?;
            report.turns_added += r.added_records;
            report.turns_skipped += r.skipped_existing_turns;
            report.turns_truncated += r.truncated_turns;
            report.turns_chunked += r.chunked_turns;
        }

        self.flush()?;
        Ok(report)
    }

    /// **Not a stable API.** The file's on-disk format version.
    #[doc(hidden)]
    pub fn __format_version(&self) -> u16 {
        self.header.version
    }

    /// **Not a stable API.** FMT-9 (a): the committed free list as
    /// `(freed_at_txn, first_page, page_count)` runs, plus the number of
    /// runs pending for the next commit.
    #[doc(hidden)]
    pub fn __free_list(&self) -> (Vec<(u64, u64, u64)>, usize) {
        (
            self.free
                .to_runs()
                .into_iter()
                .map(|r| (r.freed_at, r.page, r.count))
                .collect(),
            self.pending_free.len(),
        )
    }

    /// **Not a stable API.** FMT-9: where a memory's record lives:
    /// `(first_page, page_count, generation)`.
    #[doc(hidden)]
    pub fn __record_location(&self, id: &Ulid) -> Option<(u64, u32, u64)> {
        let idx = self.index.get(id.0, &self.catalog)?;
        let e = &self.catalog[idx];
        (!e.deleted).then_some((e.start_page, e.page_count, e.gen))
    }

    /// **Not a stable API.** FMT-9b: whether the vector column is resident,
    /// and how many vector pages hold live entries.
    #[doc(hidden)]
    pub fn __vector_column(&self) -> (bool, usize) {
        (self.col.resident, self.col.page_rows.len())
    }

    /// **Not a stable API.** FMT-9c.2: `(catalog rows incl. tombstones,
    /// delta runs on the chain, first page of the base run)`.
    #[doc(hidden)]
    pub fn __catalog_shape(&self) -> (usize, usize, u64) {
        (
            self.catalog.len(),
            self.delta_chain.len(),
            self.header.catalog_start,
        )
    }

    /// **Not a stable API.** FMT-9c.3: `(index delta runs on the chain,
    /// first page of the base index run)`.
    #[doc(hidden)]
    pub fn __index_shape(&self) -> (usize, u64) {
        (self.index_chain.len(), self.header.index_start)
    }

    /// **Not a stable API.** FMT-9d: BM25 keyword scores for `query`, best
    /// first: `(id, score)`, with k1 1.2 and b 0.75.
    #[doc(hidden)]
    pub fn __bm25(&mut self, query: &str) -> Result<Vec<(Ulid, f32)>> {
        let scores = self.bm25_scores(query, 1.2, 0.75)?;
        let mut out: Vec<(Ulid, f32)> = scores
            .into_iter()
            .map(|(row, s)| (Ulid(self.catalog[row].id), s))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(out)
    }

    /// **Not a stable API.** FMT-9d: BM25 segments on disk.
    #[doc(hidden)]
    pub fn __bm25_segments(&self) -> usize {
        self.bm25.manifest.segments.len()
    }

    /// **Not a stable API.** FMT-9: the header's next unallocated page.
    #[doc(hidden)]
    pub fn __next_page(&self) -> u64 {
        self.header.next_page
    }

    /// **Not a stable API.** FMT-9 (b): add a placeholder key slot (its
    /// wrapped DEK is random bytes, so it unlocks nothing) to exercise the
    /// key-slot run until Phase 13.2 adds real slots. v9 only.
    #[doc(hidden)]
    pub fn __add_key_slot_for_testing(&mut self, label: &str) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if !self.is_v9() {
            return Err(MnemoError::Invalid("key slots need a v9 file".into()));
        }
        let mut wrapped = vec![0u8; WRAPPED_DEK_LEN];
        wrapped[..crypto::KEY_LEN].copy_from_slice(&crypto::random_dek()[..]);
        self.key_slots.slots.push(KeySlotEntry {
            label: label.to_string(),
            role: 0,
            m_cost: KdfParams::raw_key().m_cost,
            t_cost: KdfParams::raw_key().t_cost,
            p_cost: KdfParams::raw_key().p_cost,
            salt: crypto::random_salt(),
            dek_nonce: crypto::random_nonce(),
            wrapped_dek: wrapped,
        });
        self.dirty_keys = true;
        Ok(())
    }

    /// **Not a stable API.** FMT-9 (b): the key-slot labels.
    #[doc(hidden)]
    pub fn __key_slot_labels(&self) -> Vec<String> {
        self.key_slots
            .slots
            .iter()
            .map(|s| s.label.clone())
            .collect()
    }

    /// **Not a stable API.** FMT-9 (b): the key-slot run's pointer
    /// `(first_page, page_count, generation)`.
    #[doc(hidden)]
    pub fn __key_slot_run(&self) -> (u64, u64, u64) {
        (
            self.header.keys.start,
            self.header.keys.pages,
            self.header.keys.gen,
        )
    }

    /// FMT-9a measurement hook: the ANN shortlist (candidate ids, nearest
    /// first by PQ distance) that recall would rerank, at the given or the
    /// index's effective `n_probe` / `n_rerank`. Empty without an index.
    /// **Unstable diagnostic.**
    #[doc(hidden)]
    pub fn __ann_shortlist(
        &mut self,
        query: &[f32],
        n_probe: Option<usize>,
        n_rerank: Option<usize>,
    ) -> Result<Vec<Ulid>> {
        self.ensure_ann()?;
        Ok(match &self.ann {
            Some(ann) => ann
                .query(query, n_probe, n_rerank)
                .into_iter()
                .map(Ulid)
                .collect(),
            None => Vec::new(),
        })
    }

    /// P-0 device budget breakdown of this file and handle. **Unstable
    /// diagnostic**; see [`BudgetReport`]. Reads the header, the catalog
    /// and file metadata only; nothing is decrypted.
    #[doc(hidden)]
    pub fn __budget_report(&self) -> Result<BudgetReport> {
        let h = &self.header;
        let ps = h.page_size as u64;
        let mut r = BudgetReport {
            file_bytes: std::fs::metadata(&self.path)?.len(),
            page_size: ps,
            header_bytes: ps,
            catalog_run_bytes: h.catalog_pages * ps,
            catalog_payload_bytes: h.catalog_len,
            index_run_bytes: h.index_pages * ps,
            index_payload_bytes: h.index_len,
            index_start_page: h.index_start,
            wal_bytes: h.wal_pages * ps,
            snapshot_manifest_bytes: h.manifest_pages * ps,
            cache_dir_run_bytes: h.cache_pages * ps,
            ..Default::default()
        };
        for e in &self.catalog {
            // FMT-9b.2: packed v9 records share pages (counted once below);
            // a v9 delete frees its pages, so there are no deleted ones.
            let packed = e.v9.is_some_and(|r| r.rec_slot != REC_SPAN);
            let pages = if packed { 0 } else { e.page_count as u64 * ps };
            if e.deleted {
                if e.v9.is_none() {
                    r.deleted_record_page_bytes += pages;
                }
            } else {
                r.record_page_bytes += pages;
                r.record_payload_bytes += e.len as u64;
                r.live_records += 1;
            }
        }
        r.record_page_bytes += self.recs.page_rows.len() as u64 * ps;
        // FMT-9b: the vector column, reported with the records.
        r.record_page_bytes += self.col.page_rows.len() as u64 * ps;
        let accounted = r.header_bytes
            + r.record_page_bytes
            + r.deleted_record_page_bytes
            + r.catalog_run_bytes
            + r.index_run_bytes
            + r.wal_bytes
            + r.snapshot_manifest_bytes
            + r.cache_dir_run_bytes;
        r.unreferenced_bytes = r.file_bytes.saturating_sub(accounted);

        r.ram_catalog = (self.catalog.capacity() * std::mem::size_of::<CatalogEntry>()) as u64;
        r.ram_id_map = self.index.heap_bytes() as u64;
        if let Some(ann) = &self.ann {
            let (c, cb, ids, codes, assign) = ann.heap_bytes();
            r.ram_ann_centroids = c as u64;
            r.ram_ann_codebook = cb as u64;
            r.ram_ann_posting_ids = ids as u64;
            r.ram_ann_posting_codes = codes as u64;
            r.ram_ann_assignment = assign as u64;
        }
        r.ram_cache_dir =
            (self.cache_dir.capacity() * std::mem::size_of::<CacheDirectoryEntry>()) as u64;
        let key_index = self.key_index.as_ref().map_or(0, |k| {
            crate::index::hash_map_bytes::<KeySlot, u128>(k.capacity())
                + k.keys()
                    .map(|s| s.key.capacity() + s.scope_owner.as_ref().map_or(0, |o| o.capacity()))
                    .sum::<usize>()
        });
        let superseded = self.superseded.as_ref().map_or(0, |s| {
            crate::index::hash_map_bytes::<u128, ()>(s.capacity())
        });
        r.ram_key_caches = (key_index + superseded) as u64;
        r.ram_vector_column = (self.col.data.capacity() * 2
            + self.col.norms.capacity() * 4
            + self.col.have.capacity()) as u64;
        r.ram_total = r.ram_catalog
            + r.ram_id_map
            + r.ram_ann_centroids
            + r.ram_ann_codebook
            + r.ram_ann_posting_ids
            + r.ram_ann_posting_codes
            + r.ram_ann_assignment
            + r.ram_cache_dir
            + r.ram_key_caches
            + r.ram_vector_column;
        Ok(r)
    }

    /// Return every live memory (used by tooling and compaction).
    pub fn memories(&mut self) -> Result<Vec<Memory>> {
        // FMT-9b: one pass over the vector pages instead of one per record.
        self.ensure_column()?;
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
        // R-004.2: read only the onboarding records, from the handle caches.
        self.ensure_key_caches()?;
        let ids: Vec<u128> = self
            .layers
            .as_ref()
            .map(|l| l.onboarding.iter().copied().collect())
            .unwrap_or_default();
        let mut out: Vec<Memory> = Vec::new();
        for e in self.live_entries(ids) {
            let m = self.read_memory(&e)?;
            if !m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) && is_onboarding(&m) {
                out.push(m);
            }
        }
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

    /// The project essence for the `about` briefing (Phase 12.7d): the
    /// [`EssenceScope::Project`] render at the default budget
    /// ([`ESSENCE_DEFAULT_RENDER_BYTES`]), or `None` when the file has no
    /// open project-scope item or timeline entry. The CLI and MCP `about`
    /// append it after the manifest and onboarding memories; [`Mnemo::about`]
    /// itself keeps returning records.
    pub fn about_essence(&mut self) -> Result<Option<String>> {
        let entries = self.essence_entries()?;
        let mut any = false;
        for e in &entries {
            let m = self.read_memory(e)?;
            let is_project_essence = m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                == Some(MNEMO_META_KIND_ESSENCE)
                && !m.metadata.contains_key(MNEMO_META_THREAD);
            if is_project_essence && status_of(&m) != Some(essence_status::RESOLVED) {
                any = true;
                break;
            }
        }
        if !any {
            return Ok(None);
        }
        self.essence(EssenceScope::Project, ESSENCE_DEFAULT_RENDER_BYTES)
            .map(Some)
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
        // R-004.2: a filter pinning `_mnemo.thread` scans just that
        // thread's records (from the handle caches); one that can only
        // match non-fact kinds skips the ANN, which doesn't hold them.
        // FMT-9d: keyword matches, normalised by the best one.
        self.kw = None;
        if let Some(text) = &req.text_query {
            let scores = self.bm25_scores(text, req.bm25_k1, req.bm25_b)?;
            let max = scores.values().copied().fold(0.0f32, f32::max);
            if max > 0.0 {
                self.kw = Some(
                    scores
                        .into_iter()
                        .map(|(row, s)| (self.catalog[row].id, s / max))
                        .collect(),
                );
            } else {
                self.kw = Some(std::collections::HashMap::new());
            }
        }
        if let Some(thread) = filter_thread_pin(&req.filter) {
            let thread = thread.to_string();
            let entries = self.thread_entries(&thread)?;
            return self.score_entries(req, now, &entries);
        }
        // Keyword-only: the candidates are the matches.
        if req.query.is_empty() {
            let ids: Vec<u128> = self
                .kw
                .as_ref()
                .map_or(Vec::new(), |k| k.keys().copied().collect());
            let entries: Vec<CatalogEntry> = ids
                .into_iter()
                .filter_map(|id| self.index.get(id, &self.catalog))
                .map(|i| self.catalog[i].clone())
                .filter(|e| !e.deleted)
                .collect();
            return self.score_entries(req, now, &entries);
        }
        let force_exact = force_exact || filter_bypasses_ann(&req.filter);
        let mut entries: Vec<CatalogEntry> = match (&self.ann, force_exact) {
            (Some(ann), false) => {
                let ids = ann.query(&req.query, req.n_probe, req.n_rerank);
                ids.iter()
                    .filter_map(|&id| self.index.get(id, &self.catalog))
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
        // FMT-9d: an ANN shortlist plus the keyword matches it missed.
        if let Some(kw) = &self.kw {
            if entries.len() < self.catalog.len() {
                let have: std::collections::HashSet<u128> = entries.iter().map(|e| e.id).collect();
                let extra: Vec<u128> = kw.keys().copied().filter(|id| !have.contains(id)).collect();
                for id in extra {
                    if let Some(i) = self.index.get(id, &self.catalog) {
                        if !self.catalog[i].deleted {
                            entries.push(self.catalog[i].clone());
                        }
                    }
                }
            }
        }

        self.score_entries(req, now, &entries)
    }

    /// FMT-9d: a candidate's relevance: its vector similarity plus ε times
    /// its normalised keyword score (keyword-only requests: just the
    /// keyword score). Returns `(relevance, reported similarity)`.
    fn relevance(&self, req: &RecallRequest, id: u128, sim: f32) -> (f32, f32) {
        let kw = self
            .kw
            .as_ref()
            .and_then(|k| k.get(&id).copied())
            .unwrap_or(0.0);
        if req.query.is_empty() {
            (kw, kw)
        } else {
            (sim + req.text_weight * kw, sim)
        }
    }

    /// Score one candidate set for [`Mnemo::score_once`].
    fn score_entries(
        &mut self,
        req: &RecallRequest,
        now: i64,
        entries: &[CatalogEntry],
    ) -> Result<Vec<RecallResult>> {
        // FMT-9b: score v9 candidates from their rows and the fp16 column,
        // decrypting only what makes `top_k`. A metadata filter needs the
        // record body, so it takes the general path below.
        if self.is_v9() && req.filter.is_empty() {
            return self.score_rows(req, now, entries);
        }
        let mut scored: Vec<RecallResult> = Vec::with_capacity(entries.len());
        for e in entries {
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
            // Phase 12.7: default-exclude the thread layers from fact
            // recall: archive turns (12.7a), essence items (12.7b; read
            // by scope through the render, not by similarity) and
            // thread-link markers (12.7c; they carry an all-equal
            // sentinel vector and would surface as junk hits). A caller
            // that wants them opts in by placing any `_mnemo.*` term in
            // the filter (equality, `$in`, `$exists`, or a range on
            // `_mnemo.thread` / `_mnemo.kind` / `_mnemo.seq`).
            if !req.filter.mentions_mnemo_meta()
                && matches!(
                    m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()),
                    Some(MNEMO_META_KIND_TURN)
                        | Some(MNEMO_META_KIND_ESSENCE)
                        | Some(MNEMO_META_KIND_THREAD_LINK)
                )
            {
                continue;
            }
            let sim = if req.query.is_empty() {
                0.0
            } else {
                memory::similarity(req.metric, &req.query, &m.vector)
            };
            let (rel, sim) = self.relevance(req, e.id, sim);
            let age_secs = (now - m.created_at) as f32;
            let score = req
                .weights
                .score(rel, age_secs, m.importance, m.access_count);
            scored.push(RecallResult {
                memory: m,
                score,
                similarity: sim,
            });
        }
        Ok(scored)
    }

    /// FMT-9b: [`Mnemo::score_entries`] for a v9 file with no metadata
    /// filter. Every check the general path makes on the decrypted record
    /// is made on the row instead (supersession, TTL, type, agent
    /// visibility, the thread-layer kinds), the similarity comes from the
    /// fp16 column, and the score from the row's `created_at` and
    /// `importance`. Candidates are then decrypted best-first until `top_k`
    /// survive; the agent check is repeated on the record, since the row
    /// only has a hash of the agent id.
    fn score_rows(
        &mut self,
        req: &RecallRequest,
        now: i64,
        entries: &[CatalogEntry],
    ) -> Result<Vec<RecallResult>> {
        self.ensure_column()?;
        let d = self.dimensions;
        let q = &req.query;
        let q_norm = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        let agent_h = req.agent_id.as_deref().map(|a| fnv1a32(&[a.as_bytes()]));
        let types: Option<Vec<u8>> = req
            .memory_types
            .as_ref()
            .map(|ts| ts.iter().map(|&t| memory_type_code(t)).collect());
        let mut cand: Vec<(f32, f32, usize)> = Vec::with_capacity(entries.len());
        for (pos, e) in entries.iter().enumerate() {
            let Some(r) = e.v9 else { continue };
            if r.flags & ROW_SUPERSEDED != 0 {
                continue;
            }
            if self.superseded.as_ref().is_some_and(|s| s.contains(&e.id)) {
                continue;
            }
            if r.flags & ROW_HAS_TTL != 0 && now >= r.expires_at {
                continue;
            }
            if let Some(ts) = &types {
                if !ts.contains(&r.memory_type) {
                    continue;
                }
            }
            if let Some(ah) = agent_h {
                if r.flags & ROW_SHARED == 0 && r.agent_hash != ah {
                    continue;
                }
            }
            if matches!(
                r.kind,
                ROW_KIND_TURN | ROW_KIND_ESSENCE | ROW_KIND_THREAD_LINK
            ) {
                continue;
            }
            let Some(idx) = self.index.get(e.id, &self.catalog) else {
                continue;
            };
            if !req.query.is_empty()
                && self.col.resident
                && idx < self.col.have.len()
                && !self.col.have[idx]
                && !self.col.hot.contains_key(&idx)
            {
                self.fill_column_page(idx)?;
            }
            let sim = if req.query.is_empty() {
                0.0
            } else if let Some((h, n)) = self.col.hot.get(&idx) {
                veccol::similarity_f16(req.metric, q, q_norm, h, *n)
            } else if self.col.resident && self.col.have.get(idx).copied().unwrap_or(false) {
                veccol::similarity_f16(
                    req.metric,
                    q,
                    q_norm,
                    &self.col.data[idx * d..(idx + 1) * d],
                    self.col.norms[idx],
                )
            } else {
                let (h, n) = self.column_entry(e)?;
                veccol::similarity_f16(req.metric, q, q_norm, &h, n)
            };
            let (rel, sim) = self.relevance(req, e.id, sim);
            let score = req.weights.score(
                rel,
                (now - r.created_at) as f32,
                r.importance,
                e.access_count,
            );
            cand.push((score, sim, pos));
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out: Vec<RecallResult> = Vec::with_capacity(req.top_k.min(cand.len()));
        for (score, sim, pos) in cand {
            if out.len() >= req.top_k {
                break;
            }
            let m = self.read_memory(&entries[pos])?;
            if let Some(agent) = &req.agent_id {
                if !(m.agent_id == *agent || m.scope == Scope::Shared) {
                    continue;
                }
            }
            out.push(RecallResult {
                memory: m,
                score,
                similarity: sim,
            });
        }
        Ok(out)
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
        // FMT-9d: keyword recall needs a v9 file's BM25 index; a
        // keyword-only request has no vector.
        if req.text_query.is_some() && !self.is_v9() {
            return Err(MnemoError::Invalid(
                "keyword recall (text_query) needs a v9 file; run `mnemo compact` to upgrade"
                    .into(),
            ));
        }
        let keyword_only = req.query.is_empty() && req.text_query.is_some();
        if !keyword_only && req.query.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: req.query.len(),
            });
        }
        // PERF-1 step 3: the ANN index loads on the first recall.
        self.ensure_ann()?;
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
        let mut used_exact_scan = self.ann.is_none() || filter_bypasses_ann(&req.filter);
        if !used_exact_scan && scored.len() < req.top_k && live_non_superseded > req.top_k {
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
                if let Some(idx) = self.index.get(r.memory.id.0, &self.catalog) {
                    self.touch(idx);
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

        self.kw = None;
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
        // R-004.2: facts only; turns, essence items and thread links stay
        // out of the ANN.
        let mems: Vec<Memory> = self.memories()?.into_iter().filter(is_ann_kind).collect();
        if mems.is_empty() {
            return Err(MnemoError::Invalid(
                "cannot build an index over an empty database".into(),
            ));
        }
        // FMT-9b: a v9 file below V9_ANN_MIN_FACTS recalls by an exact fp16
        // scan, which is faster than the index there and costs no disk.
        // Report it as an index with no partitions, and drop a stale one.
        // `MNEMO_DEV_ANN_MIN_FACTS` overrides the threshold so the v8 index
        // suites can run on v9 files during FMT-9 (removed in 9f).
        let min_facts = std::env::var("MNEMO_DEV_ANN_MIN_FACTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(V9_ANN_MIN_FACTS);
        if self.is_v9() && mems.len() < min_facts {
            if self.ann.is_some() || self.ann_unloaded {
                self.ann = None;
                self.ann_unloaded = false;
                self.dirty_index = true;
                self.index_persist.forced = true;
            }
            return Ok(IndexInfo {
                vectors: mems.len(),
                partitions: 0,
                subspaces: 0,
                n_probe: 0,
                n_rerank: 0,
            });
        }
        let items: Vec<(u128, &[f32])> =
            mems.iter().map(|m| (m.id.0, m.vector.as_slice())).collect();
        let mut idx = IvfPqIndex::build(self.dimensions, &items, cfg)?;
        let info = idx.info();
        if self.is_v9() {
            idx.enable_log();
            self.index_rebase = true;
        }
        self.ann = Some(idx);
        self.ann_unloaded = false;
        self.dirty_index = true;
        self.index_persist.forced = true;
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
        if self.ann.is_some() || self.ann_unloaded {
            self.ann = None;
            self.ann_unloaded = false;
            self.dirty_index = true;
            self.index_persist.forced = true;
        }
    }

    /// PERF-1 (R-004.1): bring a just-loaded ANN index up to date with the
    /// catalog. Called wherever an index run is loaded: on first use after
    /// `open` / `open_read_only` (`ensure_ann`, PERF-1 step 3) and by
    /// `restore_to`, with the first page of that run.
    ///
    /// `flush` persists the index only every so often, so that run can lag
    /// the catalog. It does two things:
    ///
    /// 1. **Prune** every indexed id the catalog no longer has live
    ///    (missing or deleted).
    /// 2. **Re-add** every live record written after the index run. Pages
    ///    are allocated monotonically in v8, and every write, including an
    ///    in-place `update` or `essence_update` (they rewrite the record to
    ///    fresh pages), lands at a page `> index_start`. Those records are
    ///    read and re-assigned. A record of a non-fact kind (turn, essence,
    ///    thread link) is removed instead: those never belong in the ANN.
    ///
    /// The work is bounded by [`INDEX_PERSIST_EVERY_CHANGES`] in the common
    /// case, since a writer persists the index at least that often.
    ///
    /// **v9:** pages are reused, so "a page past the run" no longer means
    /// "written after it". A v9 record is newer than the index run when its
    /// generation is greater than the run's (`index_gen`).
    fn reconcile_ann_with_catalog(&mut self, index_start: u64, index_gen: u64) -> Result<()> {
        if self.ann.is_none() {
            return Ok(());
        }
        let mut changed = 0usize;
        // 1. Prune ids that are no longer live.
        let stale: Vec<u128> = match &self.ann {
            Some(ann) => ann
                .ids()
                .filter(|&id| !matches!(self.index.get(id, &self.catalog), Some(i) if !self.catalog[i].deleted))
                .collect(),
            None => Vec::new(),
        };
        if let Some(ann) = &mut self.ann {
            for id in &stale {
                ann.remove(*id);
                changed += 1;
            }
        }
        // 2. Re-add records written after the index run.
        let v9 = self.is_v9();
        let pending: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted && written_after_index(e, v9, index_start, index_gen))
            .cloned()
            .collect();
        for e in &pending {
            let m = self.read_memory(e)?;
            if let Some(ann) = &mut self.ann {
                if is_ann_kind(&m) {
                    ann.add(e.id, &m.vector);
                } else {
                    ann.remove(e.id);
                }
                changed += 1;
            }
        }
        if changed > 0 {
            self.dirty_index = true;
            self.index_persist.changes += changed;
        }
        Ok(())
    }

    /// Whether the database has an ANN index (loaded, or persisted and
    /// loading on first use).
    pub fn has_index(&self) -> bool {
        self.ann.is_some() || self.ann_unloaded
    }

    /// PERF-1 step 3: load the persisted ANN index on first use, then bring
    /// it up to date with the catalog (`reconcile_ann_with_catalog`) and,
    /// when the handle caches are already built, prune non-fact ids an
    /// older persisted index may hold.
    fn ensure_ann(&mut self) -> Result<()> {
        if !self.ann_unloaded {
            return Ok(());
        }
        self.ann_unloaded = false;
        self.ann = load_index(
            &mut self.pager,
            self.header.index_start,
            self.header.index_pages,
            self.header.index_len,
            self.header.index_gen,
            self.dimensions,
        )?;
        // FMT-9c.3: replay the v9 index delta chain, then record changes.
        if self.is_v9() {
            if let Some(ann) = self.ann.as_mut() {
                let (chain, n) = apply_index_chain(&mut self.pager, ann, self.header.index_delta)?;
                self.index_chain = chain;
                self.index_chain_ops = n;
                ann.enable_log();
            }
        }
        let (index_start, index_gen) = (self.header.index_start, self.index_gen_now());
        self.reconcile_ann_with_catalog(index_start, index_gen)?;
        let non_facts: Vec<u128> = match &self.layers {
            Some(l) => l
                .essence
                .iter()
                .copied()
                .chain(l.threads.values().flat_map(|t| t.records.iter().copied()))
                .collect(),
            None => Vec::new(),
        };
        if let Some(ann) = &mut self.ann {
            let mut pruned = 0usize;
            for id in non_facts {
                if ann.contains(id) {
                    ann.remove(id);
                    pruned += 1;
                }
            }
            if pruned > 0 {
                self.dirty_index = true;
                self.index_persist.changes += pruned;
            }
        }
        Ok(())
    }

    // --- durability & maintenance ----------------------------------------

    /// Seal a serialized buffer into a fresh run of encrypted home pages and
    /// append a WAL frame for each. Returns the run's `(start_page, pages)`.
    /// The pages are *not* written to their home locations here — they go to
    /// the WAL and are folded in later by [`Mnemo::checkpoint`].
    ///
    /// `gen` is the run's generation (v9; `0` on v8). On v9 the pages come
    /// from the free list when an eligible run fits.
    fn seal_run(
        &mut self,
        bytes: &[u8],
        gen: u64,
        frames: &mut Vec<wal::Frame>,
    ) -> Result<(u64, u32)> {
        let pc = bytes.len().div_ceil(PAYLOAD).max(1);
        let start = self.alloc_pages(pc as u64);
        self.seal_run_at(start, pc as u64, bytes, gen, frames)?;
        Ok((start, pc as u32))
    }

    /// Seal `bytes` into the `pages` already-allocated pages at `start`
    /// (pages past the data are sealed empty) and append their WAL frames.
    fn seal_run_at(
        &mut self,
        start: u64,
        pages: u64,
        bytes: &[u8],
        gen: u64,
        frames: &mut Vec<wal::Frame>,
    ) -> Result<()> {
        let direct = self.is_v9() && pages > V9_WAL_RUN_MAX_PAGES;
        for i in 0..pages as usize {
            let lo = (i * PAYLOAD).min(bytes.len());
            let hi = ((i + 1) * PAYLOAD).min(bytes.len());
            let page_no = start + i as u64;
            let sealed = self.pager.seal_page(page_no, gen, &bytes[lo..hi])?;
            if direct {
                self.pager.write_sealed(page_no, &sealed)?;
            } else {
                frames.push((page_no, sealed.to_vec()));
            }
        }
        if direct {
            self.direct_writes = true;
        }
        Ok(())
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
        let old_wal_pages = self.header.wal_pages;
        // FMT-9 (a): the new region always comes from the file's tail: the
        // relocation commits a header-only transaction, so a region taken
        // from the free list would still be listed as free in the
        // committed free-list run.
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
        self.relocate_header_through_wal(old_wal_start)?;
        // FMT-9 (a): once the relocation has committed, the old region is
        // unreferenced; the next commit frees it.
        self.release_pages(old_wal_start, old_wal_pages);
        Ok(())
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
        // PERF-1 step 3: while the index is unloaded, records written since
        // its run pile up for the next load to re-assign. Once there are
        // INDEX_PERSIST_EVERY_CHANGES of them, load it now so this flush
        // persists a current index and keeps that work bounded.
        if self.ann_unloaded && self.dirty_catalog {
            let (index_start, index_gen) = (self.header.index_start, self.index_gen_now());
            let v9 = self.is_v9();
            let pending = self
                .catalog
                .iter()
                .filter(|e| !e.deleted && written_after_index(e, v9, index_start, index_gen))
                .count();
            if pending >= INDEX_PERSIST_EVERY_CHANGES {
                self.ensure_ann()?;
            }
        }
        // PERF-1: persist the (derived) ANN index only when forced or due.
        // Otherwise the header keeps pointing at the previous index run, and
        // records written since then are re-assigned when it's next loaded
        // (`reconcile_ann_with_catalog`). `ann == None` with `dirty_index` set is a
        // dropped index, which is persisted at once.
        let persist_index = if self.is_v9() {
            self.plan_index_v9()
        } else {
            self.dirty_index
                && (self.index_persist.forced
                    || self.ann.is_none()
                    || self.index_persist.commits + 1 >= INDEX_PERSIST_EVERY_COMMITS
                    || self.index_persist.changes >= INDEX_PERSIST_EVERY_CHANGES)
        };
        let free_work =
            self.is_v9() && (self.free_dirty || !self.pending_free.is_empty() || self.dirty_keys);
        if !self.dirty_catalog && !persist_index && !self.dirty_cache && !free_work {
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
        let is_memory_or_index_flush = self.dirty_catalog || persist_index;

        // FMT-9: a v9 session's first sealed page needs the generation
        // lease (normally taken by its first record write already).
        self.ensure_gen_lease()?;
        // FMT-9b: column entries first; they move rows' vector locations,
        // so they must land before the catalog is serialized.
        self.pack_records()?;
        self.pack_vectors()?;

        // 1. Serialize control plane, ensure WAL capacity, lease counter +
        //    page slots, and persist the leased header *before* any data
        //    page hits the disk.
        let ctx = self.prepare_for_flush(persist_index)?;

        // 2. Record (vector) data pages: copy-on-write to fresh pages, fsynced.
        //    Safe: the lease guarantees that even if we crash here, reopen
        //    will see a `write_counter` past every nonce this step uses.
        self.pager.flush()?;

        // 3. Seal the catalog / index control pages into fresh home runs.
        //    FMT-9: `txn_id` is this transaction's id and the generation
        //    of every run it seals. A v9 run replaced here is freed by this
        //    same commit (`superseded`); v8 never reuses pages.
        let txn_id = self.header.wal_seq + 1;
        let gen = if self.is_v9() { txn_id } else { 0 };
        let v9 = self.is_v9();
        let mut superseded: Vec<(u64, u64)> = Vec::new();
        let mut frames: Vec<wal::Frame> = Vec::new();
        // FMT-9c.2: a delta run goes on the chain; a new base replaces the
        // old base and the whole chain (all freed by this commit). The
        // chain is adopted once the commit succeeds.
        let mut new_chain: Option<(Vec<RunPtr>, usize)> = None;
        if let Some(bytes) = &ctx.cat_bytes {
            if ctx.cat_delta {
                let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                let ptr = RunPtr {
                    start,
                    pages: pages as u64,
                    len: bytes.len() as u64,
                    gen,
                };
                self.header.catalog_delta = ptr;
                let mut chain = self.delta_chain.clone();
                chain.push(ptr);
                new_chain = Some((chain, self.delta_rows + self.dirty_rows.len()));
            } else {
                if v9 && self.header.catalog_pages > 0 {
                    superseded.push((self.header.catalog_start, self.header.catalog_pages));
                }
                if v9 {
                    for r in &self.delta_chain {
                        superseded.push((r.start, r.pages));
                    }
                    self.header.catalog_delta = RunPtr::default();
                    new_chain = Some((Vec::new(), 0));
                }
                let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                self.header.catalog_start = start;
                self.header.catalog_pages = pages as u64;
                self.header.catalog_len = bytes.len() as u64;
                self.header.catalog_gen = gen;
            }
        }
        // FMT-9d: the BM25 segment, then the manifest that lists it.
        let mut new_bm25: Option<Bm25Segments> = None;
        if let Some((bytes, replace_from)) = &ctx.bm25_seg {
            let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
            let mut segs = self.bm25.manifest.segments.clone();
            let mut seg_docs = self.bm25.manifest.seg_docs.clone();
            seg_docs.resize(segs.len(), 0);
            let docs = if let Some(from) = *replace_from {
                for t in segs.drain(from..) {
                    superseded.push((t.0, t.1));
                }
                seg_docs.truncate(from);
                self.bm25.merged_docs
            } else {
                self.bm25.pending.len() as u32
            };
            segs.push((start, pages as u64, bytes.len() as u64, gen));
            seg_docs.push(docs);
            let man = Bm25Manifest {
                tokenizer: text::TOKENIZER_V1,
                next_doc: self.bm25.manifest.next_doc,
                segments: segs.clone(),
                seg_docs: seg_docs.clone(),
            };
            let mbytes =
                rmp_serde::to_vec(&man).map_err(|e| MnemoError::Serialize(e.to_string()))?;
            if !self.header.bm25.is_empty() {
                superseded.push((self.header.bm25.start, self.header.bm25.pages));
            }
            let (ms, mp) = self.seal_run(&mbytes, gen, &mut frames)?;
            self.header.bm25 = RunPtr {
                start: ms,
                pages: mp as u64,
                len: mbytes.len() as u64,
                gen,
            };
            new_bm25 = Some((segs, seg_docs));
        }
        let mut new_ichain: Option<(Vec<RunPtr>, usize)> = None;
        if persist_index && ctx.idx_delta {
            if let Some(bytes) = &ctx.idx_bytes {
                let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                let ptr = RunPtr {
                    start,
                    pages: pages as u64,
                    len: bytes.len() as u64,
                    gen,
                };
                self.header.index_delta = ptr;
                let mut chain = self.index_chain.clone();
                chain.push(ptr);
                let n = self.index_delta_ops.as_ref().map_or(0, |o| o.len());
                new_ichain = Some((chain, self.index_chain_ops + n));
            }
        } else if persist_index {
            if v9 && self.header.index_pages > 0 {
                superseded.push((self.header.index_start, self.header.index_pages));
            }
            if v9 {
                for r in &self.index_chain {
                    superseded.push((r.start, r.pages));
                }
                self.header.index_delta = RunPtr::default();
                new_ichain = Some((Vec::new(), 0));
            }
            match &ctx.idx_bytes {
                Some(bytes) => {
                    let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                    self.header.index_start = start;
                    self.header.index_pages = pages as u64;
                    self.header.index_len = bytes.len() as u64;
                    self.header.index_gen = gen;
                }
                None => {
                    self.header.index_start = 0;
                    self.header.index_pages = 0;
                    self.header.index_len = 0;
                    self.header.index_gen = 0;
                }
            }
        }
        // v8+ result-cache directory (Phase 10.1). Seal the serialized
        // directory into a fresh page run and update the pointer in the
        // header. The seal AAD already covers cache_start/pages/len for
        // v8+ (see `header_seal_aad`), so tampering with the pointer
        // trips the header seal on the next open.
        if self.dirty_cache {
            if v9 && self.header.cache_pages > 0 {
                superseded.push((self.header.cache_start, self.header.cache_pages));
            }
            match &ctx.cache_bytes {
                Some(bytes) if !self.cache_dir.is_empty() => {
                    let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                    self.header.cache_start = start;
                    self.header.cache_pages = pages as u64;
                    self.header.cache_len = bytes.len() as u64;
                    self.header.cache_gen = gen;
                }
                _ => {
                    // Either the directory serialized to nothing (all
                    // entries deleted after tombstone-only migration) or
                    // it's empty — collapse the pointer to zero.
                    self.header.cache_start = 0;
                    self.header.cache_pages = 0;
                    self.header.cache_len = 0;
                    self.header.cache_gen = 0;
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
        //     `max_snapshots == 0` disables the cap. On v8, pages
        //     referenced only by pruned snapshots stay on disk until
        //     `compact_file`; on v9 they become reusable once no retained
        //     snapshot is older than the transaction that freed them.
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
        let mut manifest = self.manifest.clone();
        if is_memory_or_index_flush {
            if self.max_snapshots > 0 && !v9 {
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
                gens: v9.then_some(SnapshotGens {
                    catalog: self.header.catalog_gen,
                    index: self.header.index_gen,
                    delta: ptr_tuple(self.header.catalog_delta),
                    index_delta: ptr_tuple(self.header.index_delta),
                    bm25: ptr_tuple(self.header.bm25),
                }),
            });
            if v9 {
                let extra = self.pending_free.iter().map(|r| r.1).sum::<u64>()
                    + superseded.iter().map(|r| r.1).sum::<u64>()
                    + self.header.manifest_pages;
                self.apply_retention(&mut manifest, extra);
            }
            let man_bytes =
                rmp_serde::to_vec(&manifest).map_err(|e| MnemoError::Serialize(e.to_string()))?;
            if v9 && self.header.manifest_pages > 0 {
                superseded.push((self.header.manifest_start, self.header.manifest_pages));
            }
            let (m_start, m_pages) = self.seal_run(&man_bytes, gen, &mut frames)?;
            self.header.manifest_start = m_start;
            self.header.manifest_pages = m_pages as u64;
            self.header.manifest_len = man_bytes.len() as u64;
            self.header.manifest_gen = gen;
        }

        // 3c. FMT-9 (b): the key-slot run, rewritten copy-on-write when a
        //     slot changed. The header seal covers its pointer.
        if v9 && self.dirty_keys {
            if !self.header.keys.is_empty() {
                superseded.push((self.header.keys.start, self.header.keys.pages));
            }
            if self.key_slots.slots.is_empty() {
                self.header.keys = RunPtr::default();
            } else {
                let bytes = rmp_serde::to_vec(&self.key_slots)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?;
                let (start, pages) = self.seal_run(&bytes, gen, &mut frames)?;
                self.header.keys = RunPtr {
                    start,
                    pages: pages as u64,
                    len: bytes.len() as u64,
                    gen,
                };
            }
        }

        // 3d. FMT-9 (a): the free list. Everything this transaction
        //     stopped referencing (`pending_free`, the runs replaced above,
        //     and the previous free-list run itself) is freed *at this
        //     transaction's id*, in this transaction's own commit, so it
        //     can't be reused until this commit is durable and no retained
        //     snapshot predates it. The list is staged in a local and
        //     adopted only once the commit succeeds (a failed flush that
        //     is retried must not free the same runs twice).
        let mut shrink = false;
        let staged_free =
            if v9 && (self.free_dirty || !self.pending_free.is_empty() || !superseded.is_empty()) {
                let mut staged = self.free.clone();
                for &(p, c) in self.pending_free.iter().chain(superseded.iter()) {
                    staged.free(txn_id, p, c)?;
                }
                if !self.header.free.is_empty() {
                    staged.free(txn_id, self.header.free.start, self.header.free.pages)?;
                }
                // FMT-9c: give eligible free runs at the file's tail back
                // to the filesystem: the header's `next_page` moves down in
                // this commit, and the file is cut after it.
                let min_snap = self.min_snapshot_txn();
                while let Some(p) =
                    staged.take_tail(self.header.next_page, self.header.wal_seq, min_snap)
                {
                    self.header.next_page = p;
                    shrink = true;
                }
                // Size the run with room for the one extra entry that
                // allocating it out of the list can create (a split), then
                // allocate it from the list (eligible runs only) or the tail.
                let est = rmp_serde::to_vec(&staged.to_runs())
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?
                    .len()
                    + 64;
                let pc = est.div_ceil(PAYLOAD).max(1) as u64;
                let start = match staged.alloc(pc, self.header.wal_seq, self.min_snapshot_txn()) {
                    Some(p) => p,
                    None => {
                        let p = self.header.next_page;
                        self.header.next_page += pc;
                        p
                    }
                };
                let bytes = rmp_serde::to_vec(&staged.to_runs())
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?;
                if bytes.len() as u64 > pc * PAYLOAD as u64 {
                    return Err(MnemoError::Invalid(
                        "free list outgrew its reserved run".into(),
                    ));
                }
                self.seal_run_at(start, pc, &bytes, gen, &mut frames)?;
                self.header.free = RunPtr {
                    start,
                    pages: pc,
                    len: bytes.len() as u64,
                    gen,
                };
                Some(staged)
            } else {
                None
            };

        // 3e. The header is the transaction's final frame; stamp the new id
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
        // FMT-9c: big runs went straight to their home pages; make them
        // durable before the header that references them can be.
        if self.direct_writes {
            self.pager.sync()?;
            self.direct_writes = false;
        }
        let (wal_start, wal_pages) = (self.header.wal_start, self.header.wal_pages);
        wal::commit(self.pager.file_mut(), wal_start, wal_pages, txn_id, &frames)?;

        // 5. Checkpoint — fold the WAL into the home pages.
        self.checkpoint(&frames)?;
        // FMT-9c: the committed header no longer covers the tail; cut it.
        // A crash before this leaves a longer file, which is harmless.
        if shrink {
            let len = self.header.next_page * PAGE_SIZE as u64;
            self.pager.file_mut().set_len(len)?;
        }

        self.manifest = manifest;
        if let Some(staged) = staged_free {
            self.free = staged;
        }
        self.pending_free.clear();
        self.free_dirty = false;
        self.dirty_keys = false;
        if let Some((chain, rows)) = new_chain {
            self.delta_chain = chain;
            self.delta_rows = rows;
        }
        if let Some((chain, n)) = new_ichain {
            self.index_chain = chain;
            self.index_chain_ops = n;
        }
        if let Some((segs, seg_docs)) = new_bm25 {
            self.bm25.manifest.segments = segs;
            self.bm25.manifest.seg_docs = seg_docs;
            self.bm25.pending.clear();
            self.bm25.dicts = None;
        }
        if persist_index {
            self.index_rebase = false;
            self.index_delta_ops = None;
        }
        if ctx.cat_bytes.is_some() {
            self.dirty_rows.clear();
            self.catalog_rewrite = false;
        }
        self.dirty_catalog = false;
        if persist_index {
            self.dirty_index = false;
            self.index_persist = IndexPersist::default();
        } else if self.dirty_index {
            self.index_persist.commits += 1;
        }
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
    fn prepare_for_flush(&mut self, persist_index: bool) -> Result<FlushPrelude> {
        // Serialize.
        // FMT-9c.2: on v9 usually just the changed rows, as a delta run.
        let cat_delta = self.dirty_catalog && self.catalog_as_delta();
        let cat_bytes = if !self.dirty_catalog {
            None
        } else if cat_delta {
            let rows: Vec<(u32, CatalogEntry)> = self
                .dirty_rows
                .iter()
                .filter(|&&i| i < self.catalog.len())
                .map(|&i| (i as u32, self.catalog[i].clone()))
                .collect();
            let d = CatalogDelta {
                prev: ptr_tuple(self.header.catalog_delta),
                rows,
            };
            Some(rmp_serde::to_vec(&d).map_err(|e| MnemoError::Serialize(e.to_string()))?)
        } else {
            if self.is_v9() {
                self.compact_tombstones();
            }
            Some(
                rmp_serde::to_vec(&self.catalog)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?,
            )
        };
        // FMT-9c.3: a v9 commit usually writes just its index changes.
        let idx_delta = persist_index && self.index_delta_ops.is_some();
        let idx_bytes: Option<Vec<u8>> = if !persist_index {
            None
        } else if let Some(ops) = &self.index_delta_ops {
            let d = IndexDeltaRef {
                prev: ptr_tuple(self.header.index_delta),
                ops,
            };
            Some(rmp_serde::to_vec(&d).map_err(|e| MnemoError::Serialize(e.to_string()))?)
        } else {
            match &self.ann {
                Some(ann) => {
                    Some(rmp_serde::to_vec(ann).map_err(|e| MnemoError::Serialize(e.to_string()))?)
                }
                None => None,
            }
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
        // v9 snapshots carry two more u64s (≤ 20 bytes with the array header).
        let per_snapshot = if self.is_v9() { 102 } else { 82 };
        let man_upper = 9 + (self.manifest.len() + 1) * per_snapshot;
        let man_pc_est = man_upper.div_ceil(PAYLOAD).max(1);
        // FMT-9: the free-list run (≤ 28 bytes per run: every pending and
        // superseded run plus a split and a relocated WAL region) and the
        // key-slot run.
        let (free_pc_est, keys_pc) = if self.is_v9() {
            let runs = self.free.len() + self.pending_free.len() + 12;
            let free_pc = (runs * 28 + 64).div_ceil(PAYLOAD).max(1);
            let keys_pc = if self.dirty_keys {
                rmp_serde::to_vec(&self.key_slots)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?
                    .len()
                    .div_ceil(PAYLOAD)
                    .max(1)
            } else {
                0
            };
            (free_pc, keys_pc)
        } else {
            (0, 0)
        };

        // Grow WAL if needed (DUR-1: relocation is WAL-logged and does
        // its own fsyncs). The cache directory rides in the same
        // transaction so its page count contributes to the reservation.
        // FMT-9d: the BM25 segment (and its manifest, one page).
        let bm25_seg = self.bm25_next_segment()?;
        let bm25_pc = bm25_seg
            .as_ref()
            .map_or(0, |(b, _)| b.len().div_ceil(PAYLOAD).max(1));
        // FMT-9c: on v9 only runs of up to V9_WAL_RUN_MAX_PAGES go through
        // the WAL; longer ones are written in place.
        let v9 = self.is_v9();
        let in_wal = |pc: usize| {
            if v9 && pc as u64 > V9_WAL_RUN_MAX_PAGES {
                0
            } else {
                pc
            }
        };
        self.ensure_wal_capacity(
            in_wal(cat_pc)
                + in_wal(idx_pc)
                + in_wal(cache_pc)
                + in_wal(man_pc_est)
                + in_wal(free_pc_est)
                + in_wal(keys_pc)
                + in_wal(bm25_pc)
                + usize::from(bm25_pc > 0)
                + 1,
        )?;

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
            bm25_seg,
            cat_bytes,
            cat_delta,
            idx_delta,
            idx_bytes,
            cache_bytes,
        })
    }

    /// Flush and close. Equivalent to `flush()` on a read-write handle; on
    /// a read-only handle it's a no-op (no dirty state to persist). The
    /// file is released on drop, which also drops the OS advisory lock.
    /// **Caution:** the handle keeps the file and its lock until it is
    /// dropped, so `drop` it before reopening the same file (or the open
    /// fails with [`MnemoError::Locked`]).
    pub fn close(&mut self) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        // PERF-1: closing persists a dirty ANN index too, so the next open
        // has nothing to re-assign.
        self.index_persist.forced = true;
        self.flush()
    }

    /// PERF-1: flush, persisting the ANN index even if the
    /// [`INDEX_PERSIST_EVERY_COMMITS`] / [`INDEX_PERSIST_EVERY_CHANGES`]
    /// policy wouldn't yet. Plain [`Mnemo::flush`] writes only the catalog
    /// between persists; the index is derived, so a crash costs nothing but
    /// re-assigning the newer records on the next load.
    pub fn flush_index(&mut self) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.index_persist.forced = true;
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
        // FMT-9b: v9 records and vectors reach their pages at the top of a
        // flush; do that part too.
        self.ensure_gen_lease()?;
        self.pack_records()?;
        self.pack_vectors()?;
        let _prelude = self.prepare_for_flush(self.dirty_index)?;
        self.pager.flush()
    }

    /// Bound the in-memory page cache to `pages` decrypted pages.
    ///
    /// The cache holds decrypted page payloads to speed repeated reads. By
    /// default it is capped at 1024 pages (~8 MiB; 8192 before v0.10.0).
    /// Lower the cap to trade hit rate for a smaller footprint, or raise
    /// it for a hotter cache. The
    /// cap governs *clean* pages — pages with un-flushed writes are always
    /// retained until [`Mnemo::flush`], regardless of the cap.
    pub fn set_cache_capacity(&mut self, pages: usize) {
        self.pager.set_cache_capacity(pages);
    }

    /// FMT-9b (v9 files): keep the fp16 vector column in memory while it
    /// takes at most `bytes` (default [`DEFAULT_VECTOR_CACHE_BUDGET`],
    /// 96 MiB, about 125K vectors at 384 dims). Above it, recall reads
    /// vector entries through the page cache instead: less memory, slower
    /// recall. Lowering it below the resident size drops the resident copy.
    pub fn set_vector_cache_budget(&mut self, bytes: usize) {
        self.vector_cache_budget = bytes;
        let need = self.catalog.len() * veccol::entry_len(self.dimensions);
        if self.col.resident && need > bytes {
            self.col.data = Vec::new();
            self.col.norms = Vec::new();
            self.col.have = Vec::new();
            self.col.resident = false;
        }
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
        let (start_page, page_count, gen) = self.write_record(&bytes)?;

        // Tombstone any prior entry for this key. Its pages remain on
        // disk until `compact_file`, exactly like memory-record
        // overwrites — the tombstone just hides it from lookups.
        if let Some(prev_pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[prev_pos].deleted = true;
            self.release_cache_pages(prev_pos);
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
            gen,
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
        let (start_page, page_count, len, ttl_secs, expired, created_at, gen) = {
            let e = &self.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.ttl_secs,
                e.is_expired(now),
                e.created_at,
                e.gen,
            )
        };
        if expired {
            // Treat expired as a miss for the caller. Don't tombstone
            // here — cache_purge does that. Just skip.
            self.cache_counters.misses += 1;
            return Ok(None);
        }
        // Decrypt and decode the record body.
        let buf = self.read_record_run(start_page, page_count, len, gen)?;
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
            self.release_cache_pages(pos);
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
            self.release_cache_pages(pos);
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

    /// FMT-9 (a): free a just-tombstoned cache entry's record pages at the
    /// next commit (v9; no-op on v8).
    fn release_cache_pages(&mut self, pos: usize) {
        let (p, c) = (
            self.cache_dir[pos].start_page,
            self.cache_dir[pos].page_count as u64,
        );
        self.release_pages(p, c);
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
            self.release_cache_pages(pos);
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
        let (start_page, page_count, gen) = self.write_record(&bytes)?;

        if let Some(prev_pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[prev_pos].deleted = true;
            self.release_cache_pages(prev_pos);
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
            gen,
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
        let (start_page, page_count, len, ttl_secs, created_at, gen) = {
            let e = &self.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.ttl_secs,
                e.created_at,
                e.gen,
            )
        };

        // Decrypt and decode the record body.
        let buf = self.read_record_run(start_page, page_count, len, gen)?;
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
            snap.catalog_gen(),
        )?;
        let (catalog, _, _) = apply_delta_chain(&mut self.pager, catalog, snap.delta_head())?;
        // FMT-9c.2: the restored catalog is written whole.
        self.catalog_rewrite = true;
        let ann = load_index(
            &mut self.pager,
            snap.index_start,
            snap.index_pages,
            snap.index_len,
            snap.index_gen(),
            self.dimensions,
        )?;
        let mut ann = ann;
        if self.is_v9() {
            self.reclaim_for_restore(&catalog);
            if let Some(a) = ann.as_mut() {
                apply_index_chain(&mut self.pager, a, snap.index_delta_head())?;
                a.enable_log();
            }
            self.index_rebase = true;
            // FMT-9d: the snapshot's BM25 segments (quarantined while it's
            // retained) describe its documents.
            let bptr = snap.gens.map_or(RunPtr::default(), |g| tuple_ptr(g.bm25));
            let manifest = load_bm25_manifest(&mut self.pager, bptr)?;
            // Its runs come back off the free list; the current ones it
            // doesn't share are freed, and the header points at its run.
            let runs = |ptr: RunPtr, m: &Bm25Manifest| {
                let mut v: Vec<(u64, u64)> = m.segments.iter().map(|t| (t.0, t.1)).collect();
                if !ptr.is_empty() {
                    v.push((ptr.start, ptr.pages));
                }
                v
            };
            let then = runs(bptr, &manifest);
            let now = runs(self.header.bm25, &self.bm25.manifest);
            self.pending_free.retain(|r| !then.contains(r));
            for &(p, c) in &then {
                self.free.unfree(p, c);
            }
            for r in now {
                if !then.contains(&r) {
                    self.release_pages(r.0, r.1);
                }
            }
            self.header.bm25 = bptr;
            self.free_dirty = true;
            self.bm25 = Bm25State {
                manifest,
                ..Bm25State::default()
            };
        }
        self.index = build_id_index(&catalog);
        self.catalog = catalog;
        // FMT-9b: the column and the packed pages described the catalog
        // being replaced.
        self.col = VecColumn::from_catalog(&self.catalog);
        self.recs = RecPages::from_catalog(&self.catalog);
        self.ann = ann;
        self.ann_unloaded = false;
        // The handle caches describe the catalog being replaced.
        self.key_index = None;
        self.superseded = None;
        self.layers = None;
        self.agent_set = None;
        // PERF-1: the snapshot's index may predate its catalog; bring it up
        // to date before it's persisted again.
        let snap_index_gen = snap.index_gen().max(snap.index_delta_head().gen);
        self.reconcile_ann_with_catalog(snap.index_start, snap_index_gen)?;
        // Re-commit as a fresh transaction: crash-safe, and itself recorded
        // as a new snapshot so a restore can always be undone.
        self.dirty_catalog = true;
        self.dirty_index = true;
        self.index_persist.forced = true;
        self.flush()
    }

    /// FMT-9c: v9 snapshot retention, applied to the staged manifest (the
    /// new snapshot already appended): keep the newest `max_snapshots`
    /// plus any younger than 24 hours, then drop the oldest while retained
    /// snapshots pin more than a quarter of the file's live pages. The
    /// newest snapshot always stays.
    ///
    /// The pin cap is a quarter of the live pages or [`V9_PIN_FLOOR_PAGES`]
    /// (1 MiB), whichever is larger, so tiny files keep their history.
    /// `extra` is what this commit frees (pinned by every older snapshot).
    ///
    /// An explicit count (`set_max_snapshots`, or a non-default
    /// `MnemoConfig::max_snapshots`) replaces the policy: exactly the
    /// newest `n` are kept (`0` keeps all), whatever they pin.
    fn apply_retention(&self, manifest: &mut Vec<Snapshot>, extra: u64) {
        if self.snapshot_cap_explicit {
            if self.max_snapshots > 0 {
                while manifest.len() > self.max_snapshots {
                    manifest.remove(0);
                }
            }
            return;
        }
        let now = memory::now_secs();
        if self.max_snapshots > 0 {
            while manifest.len() > self.max_snapshots
                && now - manifest[0].created_at > V9_SNAPSHOT_KEEP_SECS
            {
                manifest.remove(0);
            }
        }
        let live_pages = self.header.next_page.saturating_sub(self.free.pages());
        let cap = (live_pages / 4).max(V9_PIN_FLOOR_PAGES);
        while manifest.len() > 1 {
            let pinned = self.free.pinned_pages(manifest[0].txn_id) + extra;
            if pinned <= cap {
                break;
            }
            manifest.remove(0);
        }
    }

    /// FMT-9 (a): a restore makes the snapshot's records live again and the
    /// current ones (those the snapshot doesn't have) dead. The snapshot's
    /// record runs may be on the free list (freed after the snapshot, and
    /// quarantined while it's retained), so they come back off it, and off
    /// this transaction's pending frees; the current runs it doesn't share
    /// are freed by the restoring commit.
    fn reclaim_for_restore(&mut self, restored: &[CatalogEntry]) {
        // Record runs, deduplicated (FMT-9b.2: packed records share a page);
        // rows still waiting to be packed own no page yet.
        let runs = |cat: &[CatalogEntry]| -> std::collections::BTreeMap<u64, u64> {
            cat.iter()
                .filter(|e| !e.deleted && e.start_page != 0)
                .map(|e| (e.start_page, e.page_count as u64))
                .collect()
        };
        let live_then = runs(restored);
        let live_now = runs(&self.catalog);
        let overlaps = |p: u64, c: u64| {
            live_then
                .range(..p + c)
                .next_back()
                .is_some_and(|(&q, &n)| q + n > p)
        };
        let dying: Vec<(u64, u64)> = live_now
            .iter()
            .filter(|(p, _)| !live_then.contains_key(p))
            .map(|(&p, &c)| (p, c))
            .collect();
        // FMT-9b: vector pages hold many rows, so they're compared as a
        // set: the snapshot's come back, the current ones it lacks go.
        let vec_then: std::collections::BTreeSet<u64> = restored
            .iter()
            .filter(|e| !e.deleted)
            .filter_map(|e| e.v9.map(|r| r.vec_page))
            .filter(|&p| p != 0)
            .collect();
        let vec_dying: Vec<u64> = self
            .col
            .page_rows
            .keys()
            .copied()
            .filter(|p| !vec_then.contains(p))
            .collect();
        self.pending_free
            .retain(|&(p, c)| !(overlaps(p, c) || (c == 1 && vec_then.contains(&p))));
        for (&p, &c) in &live_then {
            self.free.unfree(p, c);
        }
        for &p in &vec_then {
            self.free.unfree(p, 1);
        }
        for (p, c) in dying {
            self.release_pages(p, c);
        }
        for p in vec_dying {
            self.release_pages(p, 1);
        }
        self.free_dirty = true;
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
            .ok_or_else(|| {
                let oldest = self.manifest.iter().map(|s| s.txn_id).min();
                match oldest {
                    Some(o) if self.is_v9() && txn_id < o => MnemoError::NotFound(format!(
                        "snapshot for transaction {txn_id}: older than the retention window \
                         (the newest {} snapshots and any from the last 24 hours, with at \
                         most a quarter of the file pinned); the oldest kept is {o}",
                        self.max_snapshots
                    )),
                    _ => MnemoError::NotFound(format!("snapshot for transaction {txn_id}")),
                }
            })?;
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
        self.rekey_secret(new_passphrase.as_bytes(), kdf)
    }

    /// PERF-1 step 7: switch to (or rotate) a raw 256-bit key, recording
    /// [`KdfParams::raw_key`] in the header. Like [`Mnemo::rekey`], only the
    /// wrapped DEK changes; pages aren't rewritten.
    pub fn rekey_with_key(&mut self, new_key: &[u8; crypto::KEY_LEN]) -> Result<()> {
        self.rekey_secret(new_key, KdfParams::raw_key())
    }

    fn rekey_secret(&mut self, secret: &[u8], kdf: KdfParams) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.flush()?;
        let new_salt = crypto::random_salt();
        let new_kek = crypto::derive_kek(secret, &new_salt, kdf)?;
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
        self.relocate_header_through_wal(old_wal_start)?;
        // FMT-9 (a): the old region is freed by the next commit (see
        // `ensure_wal_capacity`).
        self.release_pages(old_wal_start, new_pages);
        Ok(())
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
            // FMT-9b: and the row's vector column entry.
            if e.v9.is_some() {
                let (h, _) = self.column_entry(e)?;
                if h.len() != self.dimensions {
                    return Err(MnemoError::Invalid(
                        "vector entry has the wrong width".into(),
                    ));
                }
            }
        }

        // Also decode every live cache entry. Tombstoned entries are
        // skipped — their pages are orphan ciphertext awaiting
        // `compact_file`, same as deleted memories. Expired entries
        // are *still* on disk until compaction, so we verify them too.
        let cache_positions: Vec<(u64, u32, u32, u64)> = self
            .cache_dir
            .iter()
            .filter(|e| !e.deleted)
            .map(|e| (e.start_page, e.page_count, e.len, e.gen))
            .collect();
        for (start_page, page_count, len, gen) in cache_positions {
            let buf = self.read_record_run(start_page, page_count, len, gen)?;
            let _c: CacheEntry = rmp_serde::from_slice(&buf)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
            n += 1;
        }
        // FMT-9 (a): no free run may overlap a page the committed state
        // references (records, cache entries, control runs, the WAL).
        if self.is_v9() {
            self.check_free_list_disjoint()?;
        }
        Ok(n)
    }

    /// FMT-9 (a): fail if the free list (or a pending free) overlaps any
    /// page the current state references.
    fn check_free_list_disjoint(&self) -> Result<()> {
        let h = &self.header;
        let mut live: Vec<(u64, u64, &str)> = vec![
            (h.wal_start, h.wal_pages, "WAL region"),
            (h.catalog_start, h.catalog_pages, "catalog run"),
            (h.index_start, h.index_pages, "index run"),
            (h.manifest_start, h.manifest_pages, "manifest run"),
            (h.cache_start, h.cache_pages, "cache directory run"),
            (h.free.start, h.free.pages, "free-list run"),
            (h.keys.start, h.keys.pages, "key-slot run"),
        ];
        live.extend(
            self.catalog
                .iter()
                .filter(|e| !e.deleted)
                .map(|e| (e.start_page, e.page_count as u64, "record")),
        );
        live.extend(
            self.cache_dir
                .iter()
                .filter(|e| !e.deleted)
                .map(|e| (e.start_page, e.page_count as u64, "cache entry")),
        );
        live.extend(self.col.page_rows.keys().map(|&p| (p, 1, "vector page")));
        live.extend(
            self.delta_chain
                .iter()
                .map(|r| (r.start, r.pages, "catalog delta run")),
        );
        live.extend(
            self.index_chain
                .iter()
                .map(|r| (r.start, r.pages, "index delta run")),
        );
        live.push((
            self.header.bm25.start,
            self.header.bm25.pages,
            "BM25 manifest",
        ));
        live.extend(
            self.bm25
                .manifest
                .segments
                .iter()
                .map(|t| (t.0, t.1, "BM25 segment")),
        );
        for (p, c, what) in live {
            if c == 0 {
                continue;
            }
            if self.free.overlaps(p, c) {
                return Err(MnemoError::Invalid(format!(
                    "free list overlaps a live {what} at page {p} (+{c})"
                )));
            }
            if self
                .pending_free
                .iter()
                .any(|&(q, n)| q < p + c && p < q + n)
            {
                return Err(MnemoError::Invalid(format!(
                    "a pending free overlaps a live {what} at page {p} (+{c})"
                )));
            }
        }
        Ok(())
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
            index: {
                self.ensure_ann()?;
                self.ann.as_ref().map(|a| a.info())
            },
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
        Self::compact_file_secret(path, passphrase.as_bytes())
    }

    /// PERF-1 step 7: [`Mnemo::compact_file`] for a raw-key database.
    pub fn compact_file_with_key(path: &str, key: &[u8; crypto::KEY_LEN]) -> Result<CompactReport> {
        Self::compact_file_secret(path, key)
    }

    fn compact_file_secret(path: &str, secret: &[u8]) -> Result<CompactReport> {
        let mut old = Mnemo::open_secret(path, secret)?;
        let dims = old.dimensions;
        let kdf = old.kdf;
        let tmp = format!("{path}.compact-tmp");

        let mut new = Mnemo::create_secret(
            &tmp,
            secret,
            MnemoConfig {
                dimensions: dims,
                kdf,
                // FMT-9a: compaction keeps the source's format (v8 stays
                // v8, v9 stays v9). 9f makes it write v9.
                format_version: old.header.version.max(VERSION),
                ..Default::default()
            },
        )?;
        // ANN-1.1: compaction is "re-tune for the current size" by
        // default. Pass IndexConfig::default() so partitions,
        // subspaces, n_probe and n_rerank are all re-derived from the
        // compacted vector count. Previously we froze the pre-compact
        // (n_probe, n_rerank), which left the new index on stale
        // numbers for the smaller post-compact corpus.
        let want_index = old.has_index();
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
        let (start_page, page_count, len, dir_accessed_at, dir_access_count, old_gen) = {
            let e = &old.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.accessed_at,
                e.access_count,
                e.gen,
            )
        };

        // Read + decrypt the old record body.
        let buf = old.read_record_run(start_page, page_count, len, old_gen)?;
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
        let (new_start_page, new_page_count, gen) = self.write_record(&bytes)?;

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
            gen,
        };
        let new_pos = self.cache_dir.len();
        self.cache_idx
            .insert(&entry.namespace, entry.key_hash, new_pos);
        self.cache_dir.push(dir_entry);
        self.dirty_cache = true;
        Ok(())
    }
}
