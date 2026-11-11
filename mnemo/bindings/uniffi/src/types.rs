//! The records and enums that cross the FFI. Metadata travels as a JSON
//! object string (UniFFI has no dynamic JSON type); vectors as `[f32]`;
//! keys as bytes, never strings.

use crate::error::MnemoError;
use serde_json::{Map, Value};

/// The four memory types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MemoryType {
    /// What happened: conversation turns, events.
    Episodic,
    /// What is known: facts, entities, beliefs.
    Semantic,
    /// What to do: preferences, rules.
    Procedural,
    /// Right now: scratchpad context.
    Working,
}

impl From<MemoryType> for mnemo_engine::MemoryType {
    fn from(t: MemoryType) -> Self {
        match t {
            MemoryType::Episodic => mnemo_engine::MemoryType::Episodic,
            MemoryType::Semantic => mnemo_engine::MemoryType::Semantic,
            MemoryType::Procedural => mnemo_engine::MemoryType::Procedural,
            MemoryType::Working => mnemo_engine::MemoryType::Working,
        }
    }
}

impl From<mnemo_engine::MemoryType> for MemoryType {
    fn from(t: mnemo_engine::MemoryType) -> Self {
        match t {
            mnemo_engine::MemoryType::Episodic => MemoryType::Episodic,
            mnemo_engine::MemoryType::Semantic => MemoryType::Semantic,
            mnemo_engine::MemoryType::Procedural => MemoryType::Procedural,
            mnemo_engine::MemoryType::Working => MemoryType::Working,
        }
    }
}

/// The binding's mobile defaults: a 16 MiB vector column budget (10K
/// memories at 384 dims stay resident at about 7.7 MB) and a 2 MiB page
/// cache. The engine's own defaults (96 MiB and 8 MiB) are for desktops.
pub const MOBILE_VECTOR_CACHE_BUDGET: u64 = 16 * 1024 * 1024;
/// 256 pages of 8 KB = 2 MiB.
pub const MOBILE_CACHE_PAGES: u32 = 256;

/// Options for creating a file, and the tuning applied on open.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MnemoConfig {
    /// Embedding width. Every stored vector must match it.
    pub dimensions: u32,
    /// `0` (the default) or `9`: new files are v9. Since v0.13 a v8 file
    /// can't be created (the v8 sunset); `8` is refused with `Invalid`.
    #[uniffi(default = 0)]
    pub format_version: u16,
    /// The in-memory vector column budget (default 16 MiB).
    #[uniffi(default = 16777216)]
    pub vector_cache_budget_bytes: u64,
    /// The page cache, in 8 KB pages (default 256 = 2 MiB).
    #[uniffi(default = 256)]
    pub cache_pages: u32,
}

/// A stored memory, without its vector (mobile callers rarely need it, and
/// it's `4 × dimensions` bytes per record across the FFI).
#[derive(Debug, Clone, uniffi::Record)]
pub struct MemoryRecord {
    /// The ULID.
    pub id: String,
    pub content: String,
    pub memory_type: MemoryType,
    pub importance: f32,
    /// The metadata as a JSON object string (including reserved
    /// `_mnemo.*` keys).
    pub metadata_json: String,
    pub agent_id: String,
    /// Unix seconds.
    pub created_at: i64,
    pub accessed_at: i64,
    pub access_count: u32,
    pub ttl_secs: Option<i64>,
}

impl From<mnemo_engine::Memory> for MemoryRecord {
    fn from(m: mnemo_engine::Memory) -> Self {
        MemoryRecord {
            id: m.id.to_string(),
            content: m.content,
            memory_type: m.memory_type.into(),
            importance: m.importance,
            metadata_json: Value::Object(m.metadata).to_string(),
            agent_id: m.agent_id,
            created_at: m.created_at,
            accessed_at: m.accessed_at,
            access_count: m.access_count,
            ttl_secs: m.ttl_secs,
        }
    }
}

/// Options for `remember` and `remember_text`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RememberOptions {
    #[uniffi(default = None)]
    pub memory_type: Option<MemoryType>,
    #[uniffi(default = None)]
    pub importance: Option<f32>,
    /// A JSON object string of user metadata.
    #[uniffi(default = None)]
    pub metadata_json: Option<String>,
    /// A key slot: a later write with the same key supersedes this one
    /// (see `history`); an identical write is a no-op.
    #[uniffi(default = None)]
    pub key: Option<String>,
    #[uniffi(default = None)]
    pub ttl_secs: Option<i64>,
    #[uniffi(default = None)]
    pub agent_id: Option<String>,
}

/// A partial update: `None` leaves a field as it is.
#[derive(Debug, Clone, uniffi::Record)]
pub struct MemoryPatch {
    #[uniffi(default = None)]
    pub content: Option<String>,
    #[uniffi(default = None)]
    pub vector: Option<Vec<f32>>,
    #[uniffi(default = None)]
    pub importance: Option<f32>,
    #[uniffi(default = None)]
    pub memory_type: Option<MemoryType>,
    /// Replaces the user metadata (a JSON object string).
    #[uniffi(default = None)]
    pub metadata_json: Option<String>,
    /// Sets a TTL (`clear_ttl` removes it).
    #[uniffi(default = None)]
    pub ttl_secs: Option<i64>,
    #[uniffi(default = false)]
    pub clear_ttl: bool,
}

/// What a write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WriteOutcome {
    /// A new record.
    Added,
    /// Changed in place (same id).
    Updated,
    /// A keyed write replaced the slot's previous record (kept in
    /// `history`).
    Superseded,
    /// An identical write: nothing changed.
    Unchanged,
}

impl From<mnemo_engine::WriteOutcome> for WriteOutcome {
    fn from(o: mnemo_engine::WriteOutcome) -> Self {
        match o {
            mnemo_engine::WriteOutcome::Added => WriteOutcome::Added,
            mnemo_engine::WriteOutcome::Updated => WriteOutcome::Updated,
            mnemo_engine::WriteOutcome::Superseded => WriteOutcome::Superseded,
            mnemo_engine::WriteOutcome::Unchanged => WriteOutcome::Unchanged,
        }
    }
}

/// The result of a keyed or plain write.
#[derive(Debug, Clone, uniffi::Record)]
pub struct WriteResult {
    pub id: String,
    pub outcome: WriteOutcome,
}

/// One metadata condition of a recall filter. Values are JSON strings
/// (`"\"tea\""`, `"3"`, `"true"`), so any JSON type can be matched.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum FilterTerm {
    /// The key is present.
    Exists { key: String },
    /// The key equals this JSON value.
    Eq { key: String, value_json: String },
    /// The key equals one of these JSON values.
    In {
        key: String,
        values_json: Vec<String>,
    },
}

/// Options for `recall` and `recall_text`.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RecallOptions {
    #[uniffi(default = 10)]
    pub top_k: u32,
    /// Only these types (all when `None`).
    #[uniffi(default = None)]
    pub types: Option<Vec<MemoryType>>,
    /// v9 files: identifiers or exact terms to match (a version, a PR
    /// number, a function name), not the question.
    #[uniffi(default = None)]
    pub keywords: Option<String>,
    #[uniffi(default = None)]
    pub keyword_weight: Option<f32>,
    /// AND-ed metadata conditions.
    #[uniffi(default = [])]
    pub filter: Vec<FilterTerm>,
    /// Stop adding hits once their content would pass this many bytes.
    #[uniffi(default = None)]
    pub max_bytes: Option<u64>,
    /// Record the hits as accessed (persisted on the next commit). Off by
    /// default on phones since v0.12.2 (RB-001.5): with it on, every recall
    /// dirties the file, so a recall made in the iOS background ends in a
    /// full commit. Ranking doesn't need it. Turn it on per call when the
    /// access counts matter to the app.
    #[uniffi(default = false)]
    pub track_access: bool,
}

/// One recall hit.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RecallHit {
    pub memory: MemoryRecord,
    pub score: f32,
    pub similarity: f32,
}

/// A recall's hits, best first. `truncated` is true when `max_bytes` cut
/// the list short.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RecallPage {
    pub hits: Vec<RecallHit>,
    pub truncated: bool,
}

/// The essence render's scope.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum EssenceScope {
    /// Shared across every thread.
    Project,
    /// One thread's essence (plus the project's).
    Thread { thread_id: String },
}

/// A section of the essence that holds editable items. The timeline is
/// append-only and has its own call (`essence_append_timeline`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum EssenceSection {
    /// What the thread is trying to do.
    Intent,
    /// How the user works: conventions that survive across threads.
    Conventions,
    /// Decisions made and why.
    Decisions,
    /// The current state of the work.
    State,
    /// Open problems.
    Open,
    /// What to do next.
    Next,
    /// Lessons learned: mistakes caught and their fixes.
    Lessons,
}

impl From<EssenceSection> for mnemo_engine::EssenceSection {
    fn from(s: EssenceSection) -> Self {
        match s {
            EssenceSection::Intent => Self::Intent,
            EssenceSection::Conventions => Self::Conventions,
            EssenceSection::Decisions => Self::Decisions,
            EssenceSection::State => Self::State,
            EssenceSection::Open => Self::Open,
            EssenceSection::Next => Self::Next,
            EssenceSection::Lessons => Self::Lessons,
        }
    }
}

impl From<EssenceScope> for mnemo_engine::EssenceScope {
    fn from(s: EssenceScope) -> Self {
        match s {
            EssenceScope::Project => Self::Project,
            EssenceScope::Thread { thread_id } => Self::Thread(thread_id),
        }
    }
}

/// What `build_index` built: the IVF+PQ index's shape.
#[derive(Debug, Clone, uniffi::Record)]
pub struct IndexInfo {
    /// Vectors in the index.
    pub vectors: u64,
    /// IVF partitions.
    pub partitions: u32,
    /// PQ subspaces.
    pub subspaces: u32,
    /// Partitions probed per query by default.
    pub n_probe: u32,
    /// Candidates reranked per query by default.
    pub n_rerank: u32,
}

impl From<mnemo_engine::IndexInfo> for IndexInfo {
    fn from(i: mnemo_engine::IndexInfo) -> Self {
        Self {
            vectors: i.vectors as u64,
            partitions: i.partitions as u32,
            subspaces: i.subspaces as u32,
            n_probe: i.n_probe as u32,
            n_rerank: i.n_rerank as u32,
        }
    }
}

/// File statistics.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DbStats {
    pub memories: u64,
    pub deleted: u64,
    pub dimensions: u32,
    pub file_bytes: u64,
    /// `9`, or `8` for a v8 file opened read-only.
    pub format_version: u16,
    pub has_index: bool,
    pub wal_pages: u64,
    /// How recall scores vectors right now (see [`RecallMode`]).
    pub recall_mode: RecallMode,
    /// The bytes the vector column needs to stay in memory (live facts ×
    /// `2 × dimensions + 4`). Recall is `ExactResident` while the store's
    /// `vectorCacheBudgetBytes` is at least this. `0` on a v8 file.
    pub vector_column_bytes: u64,
}

/// How recall scores vectors, from `stats()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RecallMode {
    /// An exact scan over the vector column in memory: the live facts fit
    /// the vector cache budget (16 MiB by default on phones). The fast path.
    ExactResident,
    /// An exact scan that reads vectors from the file: the live facts are
    /// over the budget (or the file is v8). Slower as the file grows: raise
    /// `vector_cache_budget_bytes`, or build an index past 20,000 facts.
    ExactPaged,
    /// The IVF+PQ index shortlists the candidates.
    Indexed,
}

impl From<mnemo_engine::RecallMode> for RecallMode {
    fn from(m: mnemo_engine::RecallMode) -> Self {
        match m {
            mnemo_engine::RecallMode::ExactResident => RecallMode::ExactResident,
            mnemo_engine::RecallMode::ExactPaged => RecallMode::ExactPaged,
            mnemo_engine::RecallMode::Indexed => RecallMode::Indexed,
        }
    }
}

/// What `compact_file` did. Every field is a count, not a size: read the
/// file's length for bytes.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CompactReport {
    /// Live memories before compaction.
    pub before: u64,
    /// Live memories after it (expired ones dropped).
    pub after: u64,
    /// Live result-cache entries before compaction.
    pub cache_before: u64,
    /// Live result-cache entries after it.
    pub cache_after: u64,
}

/// One turn of a conversation to archive.
#[derive(Debug, Clone, uniffi::Record)]
pub struct ThreadTurn {
    /// The source's id for the turn; re-ingesting it is a no-op.
    pub turn_id: String,
    /// `user`, `assistant`, `system`, `tool`, …
    pub role: String,
    pub content: String,
    /// Unix seconds (the ingest time when `None`).
    #[uniffi(default = None)]
    pub timestamp: Option<i64>,
}

/// One archived turn, as `thread_export` returns it (chunks joined).
#[derive(Debug, Clone, uniffi::Record)]
pub struct ExportedTurn {
    pub turn_id: String,
    pub role: String,
    pub content: String,
    pub timestamp: i64,
    /// For a truncated tool result: the original's SHA-256 (hex) and length.
    pub sha256: Option<String>,
    pub original_length: Option<u64>,
}

/// What an ingest did.
#[derive(Debug, Clone, uniffi::Record)]
pub struct IngestReport {
    pub added_records: u64,
    pub skipped_existing_turns: u64,
    pub truncated_turns: u64,
    pub chunked_turns: u64,
}

/// An embedder's identity: checked against the file's manifest, so a file
/// is never mixed with vectors from another model.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct EmbedderIdentity {
    pub name: String,
    pub dimensions: u32,
    pub normalize: bool,
}

/// Parse a JSON object string into metadata.
pub(crate) fn metadata_from_json(s: &str) -> Result<Map<String, Value>, MnemoError> {
    match serde_json::from_str::<Value>(s) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(MnemoError::Invalid {
            message: "metadata_json must be a JSON object".into(),
        }),
        Err(e) => Err(MnemoError::Invalid {
            message: format!("metadata_json: {e}"),
        }),
    }
}

/// Parse one JSON value of a filter term.
pub(crate) fn json_value(s: &str) -> Result<Value, MnemoError> {
    serde_json::from_str(s).map_err(|e| MnemoError::Invalid {
        message: format!("filter value is not JSON: {e}"),
    })
}
