//! Exact-key result cache (Phase 10.1 of the level-up plan).
//!
//! **This is NOT the page cache** (that's `mnemo/src/cache.rs`, which
//! caches decrypted pages inside the pager). This module implements a
//! *user-facing key/value cache* backed by the same encrypted-page
//! storage engine as the memory catalog — a `.mnemo` file becomes both a
//! durable agent memory store AND a durable exact-key cache for LLM
//! tool-call results, prompt→completion pairs, HTTP responses, and
//! anything else an agent wants to memoize.
//!
//! ## Model
//!
//! Each cache entry lives at a `(namespace, key)` pair. The engine
//! SHA-256-hashes `key` — callers pass the raw input string (a prompt,
//! a tool-call JSON, a URL, whatever), the engine handles the hashing.
//! Namespaces isolate independent caches inside one file so `llm` and
//! `http` and `tool` never collide.
//!
//! On-disk layout mirrors the memory catalog + record split:
//!
//! - **Cache directory** — a serialized `Vec<CacheDirectoryEntry>` in
//!   its own encrypted page run, pointed at by
//!   [`crate::format::Header`]'s `cache_start` / `cache_pages` /
//!   `cache_len` fields (added in format v8). Small per-entry
//!   overhead; the directory is what a lookup scans.
//! - **Cache records** — the payloads. One [`CacheEntry`] per record,
//!   serialized as MessagePack and stored in the same encrypted-page
//!   append-only region the memory records use. Never rewritten on
//!   read; access stats (`accessed_at`, `access_count`) live on the
//!   directory entry only (the v5 recall-side-trick).
//!
//! ## Flush policy
//!
//! [`CacheFlushPolicy`] is per-handle:
//!
//! - `Strict` (default): every `cache_put` dirties the directory; the
//!   next call to `Mnemo::flush()` persists everything in one
//!   WAL-committed transaction.
//! - `Batched { max_dirty, max_age }`: `cache_put` still dirties the
//!   directory in memory, but the engine tracks pending-dirty count
//!   and first-dirty timestamp. When either threshold trips, the
//!   engine internally calls `flush()` — persisting the whole
//!   transaction (including any pending memory writes, which is safe
//!   because they were queued for a flush anyway; the only effect is
//!   earlier-than-user-expected durability).
//!
//! **Memory writes are never batched** — `remember` behaves exactly as
//! it always did, requiring a user-driven `flush()` to persist. Only
//! cache mutations get the relaxed lane.
//!
//! ## Crash semantics
//!
//! Losing unflushed (Batched-lane) cache entries on crash is a cache
//! *miss*, never corruption. Every commit — whether Strict or Batched —
//! goes through the same single-fsync WAL transaction that already
//! backs memory writes. The append-only-snapshot invariant is untouched.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::time::{Duration, Instant};

// --- Public tunables and types -------------------------------------------

/// Default per-namespace entry-count budget. Once a namespace holds this
/// many live entries, `cache_put` evicts the LRU entry before inserting.
pub const DEFAULT_MAX_ENTRIES: usize = 10_000;

/// Default per-namespace byte budget (values + directory overhead).
/// Once exceeded, `cache_put` evicts LRU until the total falls back
/// under the cap.
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Default `max_dirty` for [`CacheFlushPolicy::Batched`].
pub const DEFAULT_BATCH_MAX_DIRTY: usize = 128;

/// Default `max_age` for [`CacheFlushPolicy::Batched`].
pub const DEFAULT_BATCH_MAX_AGE: Duration = Duration::from_secs(5);

/// Default similarity threshold for [`crate::Mnemo::cache_get_semantic`].
/// Lookups below this are treated as misses. Chosen conservatively to
/// avoid false hits on subtly-different prompts; callers can override
/// per-lookup. Model-dependent — text-embedding-ada-002 and BGE at
/// this threshold rarely surface unrelated content, but if you're
/// working with a lossier embedder consider raising it.
pub const DEFAULT_SEMANTIC_THRESHOLD: f32 = 0.97;

/// Length in chars of the human-readable key preview stored on every
/// directory entry. Long enough to be recognizable, short enough that
/// the directory scan stays fast on large caches.
pub const KEY_PREVIEW_MAX_CHARS: usize = 120;

/// Options accepted by `Mnemo::cache_put`. Defaults describe an
/// untyped, never-expiring, non-cost-hinted entry.
#[derive(Clone, Debug)]
pub struct CachePutOpts {
    /// Free-form content-type label. Convention: `"json"`, `"text"`,
    /// `"bytes"`, or a MIME string. Not interpreted by the engine —
    /// preserved verbatim on `cache_get` so callers can round-trip
    /// their own type discipline.
    pub content_type: String,
    /// Time-to-live in seconds from the moment of `cache_put`. `None`
    /// means "never expires by itself" (still evictable via LRU or
    /// namespace purge).
    pub ttl_secs: Option<u64>,
    /// Optional hint of how expensive the miss-side computation was,
    /// in milliseconds. Recorded per-entry; today used only as a
    /// tie-breaker for future cost-aware eviction (see the GDSF
    /// follow-up note in the level-up plan).
    pub cost_hint_ms: Option<u32>,
}

impl Default for CachePutOpts {
    fn default() -> Self {
        Self {
            content_type: "text".into(),
            ttl_secs: None,
            cost_hint_ms: None,
        }
    }
}

/// Options for [`crate::Mnemo::cache_put_semantic`]. Same as
/// [`CachePutOpts`] plus a **required** `model` string — a hit from a
/// different embedding model's cache is a semantics bug, not a win, so
/// the engine forces callers to declare the model and matches it on
/// lookup.
///
/// The vector itself is a separate argument to `cache_put_semantic`
/// (not inside opts) so callers who already own the vector don't have
/// to `.clone()` it into the struct.
#[derive(Clone, Debug)]
pub struct SemanticCachePutOpts {
    /// Free-form content-type label recorded on the entry.
    pub content_type: String,
    /// TTL in seconds. Omit for no expiry.
    pub ttl_secs: Option<u64>,
    /// Cost hint recorded on the entry (see [`CachePutOpts::cost_hint_ms`]).
    pub cost_hint_ms: Option<u32>,
    /// Embedding model identifier. Matched exactly on lookup; a
    /// different model's cache is invisible to this query. Convention:
    /// `"text-embedding-3-small"`, `"bge-large-en-v1.5"`, etc.
    pub model: String,
}

impl SemanticCachePutOpts {
    /// Construct with just the required `model`; all other fields
    /// default (content_type = "text", no TTL, no cost hint).
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            content_type: "text".into(),
            ttl_secs: None,
            cost_hint_ms: None,
            model: model.into(),
        }
    }
}

/// A returned cache hit. `accessed_at` / `access_count` are updated on
/// the directory *before* this is constructed, so the values here
/// reflect the post-touch state (mirroring the recall-time convention).
#[derive(Clone, Debug)]
pub struct CachedValue {
    /// Value bytes as stored by `cache_put`.
    pub value: Vec<u8>,
    /// Content-type label as stored by `cache_put`.
    pub content_type: String,
    /// Unix seconds when this entry was created.
    pub created_at: i64,
    /// Unix seconds when this entry was last accessed (this hit).
    pub accessed_at: i64,
    /// Number of hits recorded on this entry (including the current one).
    pub access_count: u32,
    /// TTL configured on `cache_put`, if any.
    pub ttl_secs: Option<u64>,
}

/// Per-namespace eviction budget. `cache_put` enforces both caps: on
/// insert, the LRU is evicted until *both* live-entry count and live
/// byte total fit under the caps.
#[derive(Clone, Copy, Debug)]
pub struct CacheBudget {
    /// Hard cap on live directory entries for this namespace.
    pub max_entries: usize,
    /// Hard cap on live payload bytes for this namespace.
    pub max_bytes: u64,
}

impl Default for CacheBudget {
    fn default() -> Self {
        Self {
            max_entries: DEFAULT_MAX_ENTRIES,
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

/// Per-handle flush policy for cache writes. Memory writes ignore this
/// and always behave as `Strict`.
#[derive(Clone, Copy, Debug, Default)]
pub enum CacheFlushPolicy {
    /// Every `cache_put` dirties the directory; the caller's next
    /// `Mnemo::flush()` persists everything.
    #[default]
    Strict,
    /// The engine internally calls `flush()` when either the pending
    /// dirty count reaches `max_dirty` OR the first pending dirty
    /// entry is older than `max_age`.
    Batched {
        /// Auto-flush after this many pending cache mutations.
        max_dirty: usize,
        /// Auto-flush this long after the first pending mutation.
        max_age: Duration,
    },
}

/// Summary statistics for a namespace (or the whole cache).
#[derive(Clone, Copy, Debug, Default)]
pub struct CacheStats {
    /// Live (non-deleted, non-expired) entries.
    pub entries: usize,
    /// Live payload bytes.
    pub bytes: u64,
    /// Hits recorded on this handle since open.
    pub hits: u64,
    /// Misses recorded on this handle since open (includes TTL-expired
    /// and non-existent keys — both look the same to the caller).
    pub misses: u64,
    /// Hits divided by (hits + misses); `0.0` if no lookups.
    pub hit_rate: f64,
    /// Directory entries evicted (by LRU-under-budget) since open.
    pub evictions: u64,
}

// --- On-disk types (crate-internal) --------------------------------------

/// The MessagePack record body of a cache entry. Stored in the same
/// encrypted-page region as memory records.
///
/// `vector` and `model` are the semantic-cache extension (Phase 10.2).
/// `#[serde(default)]` keeps pre-semantic v8 entries — where the
/// on-disk record has no vector / no model — round-trip-safe under
/// the extended schema: rmp-serde decodes missing tail fields as
/// their `Default::default()`, so old entries surface as `None`/`None`
/// (exactly the "exact-key only, no vector" state we want).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct CacheEntry {
    pub namespace: String,
    pub key_hash: [u8; 32],
    pub key_preview: String,
    pub value: Vec<u8>,
    pub content_type: String,
    pub created_at: i64,
    pub accessed_at: i64,
    pub access_count: u32,
    pub ttl_secs: Option<u64>,
    pub cost_hint_ms: Option<u32>,
    pub size_bytes: u64,
    /// Semantic-cache: embedding vector for this entry, or `None` for
    /// pure exact-key entries.
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    /// Semantic-cache: embedding model identifier this vector was
    /// computed under, or `None` for pure exact-key entries. Matched
    /// exactly on `cache_get_semantic` — different-model hits are bugs.
    #[serde(default)]
    pub model: Option<String>,
}

/// Directory entry — the "catalog row" for a cache record. Access stats
/// live here (not on the [`CacheEntry`]) so a hit doesn't require a full
/// record rewrite (mirrors the v5 recall trick).
///
/// `size_bytes` is duplicated from the entry body so budget enforcement
/// runs against the directory alone without decrypting record pages.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct CacheDirectoryEntry {
    pub namespace: String,
    pub key_hash: [u8; 32],
    pub start_page: u64,
    pub page_count: u32,
    pub len: u32,
    pub deleted: bool,
    pub accessed_at: i64,
    pub access_count: u32,
    pub size_bytes: u64,
    pub ttl_secs: Option<u64>,
    pub created_at: i64,
    /// Semantic-cache: embedding vector duplicated onto the directory
    /// entry so lookup can scan without reading (and decrypting) every
    /// record body. `None` on pure exact-key entries. `#[serde(default)]`
    /// keeps pre-semantic v8 directories decoding under the extended
    /// schema.
    #[serde(default)]
    pub vector: Option<Vec<f32>>,
    /// Semantic-cache: model identifier. `None` on pure exact-key entries.
    #[serde(default)]
    pub model: Option<String>,
}

impl CacheDirectoryEntry {
    /// Has the entry passed its TTL at `now`? An entry with no TTL is
    /// never expired.
    pub(crate) fn is_expired(&self, now: i64) -> bool {
        match self.ttl_secs {
            Some(ttl) => now.saturating_sub(self.created_at) as u64 >= ttl,
            None => false,
        }
    }
}

// --- Key hashing ---------------------------------------------------------

/// SHA-256 hash a caller-supplied cache key. Deterministic and platform-
/// independent (endianness-safe because the input is a byte string, not
/// a typed value).
pub fn hash_key(key: &str) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(key.as_bytes());
    h.finalize().into()
}

/// Truncate a key to at most [`KEY_PREVIEW_MAX_CHARS`] chars (not
/// bytes), suffixing `…` when cut. Preserves char-boundary safety for
/// UTF-8 previews.
pub fn key_preview(key: &str) -> String {
    if key.chars().count() <= KEY_PREVIEW_MAX_CHARS {
        return key.to_string();
    }
    let mut out: String = key.chars().take(KEY_PREVIEW_MAX_CHARS - 1).collect();
    out.push('…');
    out
}

// --- In-memory directory index -------------------------------------------

/// In-memory index over the on-disk directory: given `(namespace,
/// key_hash)`, find the position of the live entry in the directory
/// `Vec`. Absent entries and deleted tombstones are not indexed.
#[derive(Debug, Default)]
pub(crate) struct DirectoryIndex {
    // Namespace name → key_hash → index into the directory Vec.
    by_ns: HashMap<String, HashMap<[u8; 32], usize>>,
}

impl DirectoryIndex {
    pub(crate) fn new() -> Self {
        Self { by_ns: HashMap::new() }
    }

    /// Build an index over a directory `Vec`, skipping deleted entries.
    pub(crate) fn build(entries: &[CacheDirectoryEntry]) -> Self {
        let mut idx = Self::new();
        for (i, e) in entries.iter().enumerate() {
            if !e.deleted {
                idx.insert(&e.namespace, e.key_hash, i);
            }
        }
        idx
    }

    pub(crate) fn insert(&mut self, namespace: &str, key_hash: [u8; 32], pos: usize) {
        self.by_ns
            .entry(namespace.to_string())
            .or_default()
            .insert(key_hash, pos);
    }

    pub(crate) fn remove(&mut self, namespace: &str, key_hash: &[u8; 32]) {
        if let Some(ns) = self.by_ns.get_mut(namespace) {
            ns.remove(key_hash);
        }
    }

    pub(crate) fn get(&self, namespace: &str, key_hash: &[u8; 32]) -> Option<usize> {
        self.by_ns.get(namespace)?.get(key_hash).copied()
    }
}

// --- In-memory hit/miss counters -----------------------------------------

/// Per-handle hit/miss/eviction counters. Persisted portion is aggregate
/// evictions (via the directory tombstones themselves); hits and misses
/// are in-memory only and reset on reopen.
#[derive(Debug, Default)]
pub(crate) struct CounterState {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

// --- Batched-lane bookkeeping --------------------------------------------

/// Tracks pending-batch state for [`CacheFlushPolicy::Batched`]. In
/// `Strict` mode all fields stay zero; the store consults this before
/// deciding whether a `cache_put` should trigger an auto-flush.
#[derive(Debug, Default)]
pub(crate) struct BatchState {
    /// Count of cache mutations since the last commit.
    pub dirty_count: usize,
    /// Wallclock of the first mutation since the last commit — used to
    /// evaluate `max_age`.
    pub first_dirty_at: Option<Instant>,
}

impl BatchState {
    pub(crate) fn record_dirty(&mut self) {
        self.dirty_count += 1;
        if self.first_dirty_at.is_none() {
            self.first_dirty_at = Some(Instant::now());
        }
    }

    pub(crate) fn reset(&mut self) {
        self.dirty_count = 0;
        self.first_dirty_at = None;
    }

    /// Should the store auto-flush now, given the current batch state
    /// and policy? Always `false` for `Strict`.
    pub(crate) fn should_auto_flush(&self, policy: &CacheFlushPolicy) -> bool {
        match policy {
            CacheFlushPolicy::Strict => false,
            CacheFlushPolicy::Batched { max_dirty, max_age } => {
                if self.dirty_count >= *max_dirty {
                    return true;
                }
                if let Some(t0) = self.first_dirty_at {
                    if t0.elapsed() >= *max_age {
                        return true;
                    }
                }
                false
            }
        }
    }
}

// --- Unit tests ---------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_key_is_deterministic_and_32_bytes() {
        let a = hash_key("hello");
        let b = hash_key("hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 32);
        // Different input → different hash (SHA-256 collision would be a
        // major cryptographic event; this just guards typo bugs).
        assert_ne!(a, hash_key("hello!"));
    }

    #[test]
    fn key_preview_truncates_with_ellipsis() {
        let short = "hi there";
        assert_eq!(key_preview(short), short);
        let long = "x".repeat(200);
        let out = key_preview(&long);
        assert_eq!(out.chars().count(), KEY_PREVIEW_MAX_CHARS);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn directory_index_locates_live_entries() {
        let entries = vec![
            CacheDirectoryEntry {
                namespace: "llm".into(),
                key_hash: [1u8; 32],
                start_page: 100,
                page_count: 1,
                len: 42,
                deleted: false,
                accessed_at: 0,
                access_count: 0,
                size_bytes: 42,
                ttl_secs: None,
                created_at: 0,
                vector: None,
                model: None,
            },
            CacheDirectoryEntry {
                namespace: "http".into(),
                key_hash: [2u8; 32],
                start_page: 101,
                page_count: 1,
                len: 42,
                deleted: true, // tombstone — should be skipped
                accessed_at: 0,
                access_count: 0,
                size_bytes: 42,
                ttl_secs: None,
                created_at: 0,
                vector: None,
                model: None,
            },
        ];
        let idx = DirectoryIndex::build(&entries);
        assert_eq!(idx.get("llm", &[1u8; 32]), Some(0));
        assert_eq!(idx.get("http", &[2u8; 32]), None, "tombstones not indexed");
    }

    #[test]
    fn batch_state_triggers_on_count() {
        let mut b = BatchState::default();
        let p = CacheFlushPolicy::Batched {
            max_dirty: 3,
            max_age: Duration::from_secs(3600),
        };
        b.record_dirty();
        b.record_dirty();
        assert!(!b.should_auto_flush(&p));
        b.record_dirty();
        assert!(b.should_auto_flush(&p));
    }

    #[test]
    fn ttl_expiry_matches_created_at() {
        let now = 1_000_000_i64;
        let fresh = CacheDirectoryEntry {
            namespace: "".into(),
            key_hash: [0u8; 32],
            start_page: 0,
            page_count: 0,
            len: 0,
            deleted: false,
            accessed_at: 0,
            access_count: 0,
            size_bytes: 0,
            ttl_secs: Some(60),
            created_at: now - 30,
            vector: None,
            model: None,
        };
        assert!(!fresh.is_expired(now), "30s old with 60s TTL is fresh");
        let stale = CacheDirectoryEntry {
            created_at: now - 120,
            ..fresh.clone()
        };
        assert!(stale.is_expired(now), "120s old with 60s TTL is expired");
        let no_ttl = CacheDirectoryEntry {
            ttl_secs: None,
            ..fresh
        };
        assert!(!no_ttl.is_expired(now + 1_000_000_000));
    }
}
