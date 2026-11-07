//! The agent memory model (Phase 5 of the build plan).
//!
//! This is what makes Mnemo a *memory engine* rather than a generic vector
//! store. Memories are typed (episodic / semantic / procedural / working),
//! carry lifecycle metadata (importance, access stats, optional TTL), and are
//! retrieved with a **multi-signal score** that blends semantic similarity
//! with recency, importance, and access frequency.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use ulid::Ulid;

/// Current unix time in whole seconds.
pub fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The four kinds of agent memory.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum MemoryType {
    /// What happened — conversation turns, events, interactions.
    Episodic,
    /// What is known — facts, entities, relationships, beliefs.
    Semantic,
    /// What to do — preferences, behavioral patterns, rules.
    Procedural,
    /// Right now — current-session context and scratchpad.
    Working,
}

impl MemoryType {
    /// Parse from a lowercase string (used by the CLI / SDK surface).
    pub fn parse(s: &str) -> Option<MemoryType> {
        match s.to_ascii_lowercase().as_str() {
            "episodic" => Some(MemoryType::Episodic),
            "semantic" => Some(MemoryType::Semantic),
            "procedural" => Some(MemoryType::Procedural),
            "working" => Some(MemoryType::Working),
            _ => None,
        }
    }

    /// Lowercase string form.
    pub fn as_str(&self) -> &'static str {
        match self {
            MemoryType::Episodic => "episodic",
            MemoryType::Semantic => "semantic",
            MemoryType::Procedural => "procedural",
            MemoryType::Working => "working",
        }
    }
}

/// Visibility of a memory across agents sharing one `.mnemo` file.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scope {
    /// Visible only to the agent that created it.
    Private,
    /// Visible to every agent with read access.
    Shared,
}

/// A single memory entry.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Memory {
    /// Time-sortable unique ID.
    pub id: Ulid,
    /// Cognitive category of this memory.
    pub memory_type: MemoryType,
    /// Embedding of `content`.
    pub vector: Vec<f32>,
    /// The actual text/data of the memory.
    pub content: String,
    /// Free-form structured metadata.
    pub metadata: Map<String, Value>,
    /// Which agent created this memory.
    pub agent_id: String,
    /// Which session produced it (relevant for episodic memory).
    pub session_id: Option<String>,
    /// Visibility scope (private to its agent, or shared).
    pub scope: Scope,
    /// Unix seconds at creation.
    pub created_at: i64,
    /// Unix seconds of the most recent retrieval. Updated by `recall`.
    pub accessed_at: i64,
    /// How many times this memory has been retrieved.
    pub access_count: u32,
    /// Importance in `[0.0, 1.0]`; higher importance resists decay.
    pub importance: f32,
    /// Optional lifetime in seconds. After expiry the memory is skipped by
    /// `recall` and removed by `compact`.
    pub ttl_secs: Option<i64>,
}

impl Memory {
    /// Create a new memory with sensible defaults. The ID and timestamps are
    /// assigned now; tune the rest with the `with_*` builders.
    pub fn new(content: impl Into<String>, memory_type: MemoryType, vector: Vec<f32>) -> Self {
        let now = now_secs();
        Memory {
            id: Ulid::new(),
            memory_type,
            vector,
            content: content.into(),
            metadata: Map::new(),
            agent_id: "default".to_string(),
            session_id: None,
            scope: Scope::Private,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            importance: 0.5,
            ttl_secs: None,
        }
    }

    /// Set the owning agent.
    pub fn with_agent(mut self, agent_id: impl Into<String>) -> Self {
        self.agent_id = agent_id.into();
        self
    }
    /// Set the session ID.
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }
    /// Set the importance score (clamped to `[0,1]`).
    pub fn with_importance(mut self, importance: f32) -> Self {
        self.importance = importance.clamp(0.0, 1.0);
        self
    }
    /// Set the visibility scope.
    pub fn with_scope(mut self, scope: Scope) -> Self {
        self.scope = scope;
        self
    }
    /// Attach a TTL in seconds.
    pub fn with_ttl(mut self, ttl_secs: i64) -> Self {
        self.ttl_secs = Some(ttl_secs);
        self
    }
    /// Attach a metadata key/value pair.
    pub fn with_meta(mut self, key: impl Into<String>, value: Value) -> Self {
        self.metadata.insert(key.into(), value);
        self
    }

    /// True if the memory's TTL has elapsed as of `now`.
    pub fn is_expired(&self, now: i64) -> bool {
        match self.ttl_secs {
            Some(ttl) => now - self.created_at >= ttl,
            None => false,
        }
    }

    /// Build the **canonical scaffold manifest** for a freshly-initialised
    /// database. This is the default "I am a brand-new MNemo file" memory
    /// inserted by `mnemo init` (and surfaced by `mnemo about`) so every new
    /// database is self-describing from birth — no opt-in step required.
    ///
    /// The scaffold is a placeholder: the vector is all-zeros (it doesn't
    /// pretend to carry semantic content), and `metadata.scaffold = true`
    /// marks it as auto-generated so tooling and humans can tell it apart
    /// from a hand-written manifest. The expected workflow is for the file's
    /// author to replace this memory with one that records the project's
    /// actual embedder, agent_id default, and conventions.
    ///
    /// See [`crate::Mnemo::about`] for the rest of the convention.
    pub fn scaffold_manifest(dimensions: usize) -> Self {
        let content = format!(
            "MNEMO MANIFEST (scaffold) — Fresh {}-dimensional database created \
             by `mnemo init`. This is a placeholder. Replace it with a memory \
             that records your project's embedder (name, dimensions, normalize), \
             default agent_id, and metadata conventions; keep the metadata keys \
             `area=\"onboarding\"` and `topic=\"manifest\"` so `mnemo about` \
             continues to surface it as the headline orientation point. Drop \
             `metadata.scaffold` once you've replaced this entry so tooling \
             knows the manifest has been curated.",
            dimensions
        );
        let mut metadata = Map::new();
        metadata.insert("area".into(), Value::String("onboarding".into()));
        metadata.insert("topic".into(), Value::String("manifest".into()));
        metadata.insert("scaffold".into(), Value::Bool(true));
        metadata.insert(
            "engine_version".into(),
            Value::String(env!("CARGO_PKG_VERSION").into()),
        );
        metadata.insert(
            "dimensions".into(),
            Value::Number(serde_json::Number::from(dimensions as u64)),
        );
        // Sentinel vector: a single-axis unit spike `[1, 0, 0, …]`.
        // A zero vector would now trip the Phase 3 `put` guard
        // (all-zero refused — see store.rs). This spike is non-zero
        // (so it passes), unit-normalized (so it reads cleanly to any
        // downstream inspector), and uncorrelated with any real
        // embedder's output (so it ranks near-last on any user query —
        // appropriate for a placeholder the user is expected to
        // replace).
        let mut vector = vec![0.0_f32; dimensions];
        if dimensions > 0 {
            vector[0] = 1.0;
        }
        let now = now_secs();
        Memory {
            id: Ulid::new(),
            memory_type: MemoryType::Semantic,
            vector,
            content,
            metadata,
            agent_id: "mnemo".to_string(),
            session_id: None,
            scope: Scope::Shared,
            created_at: now,
            accessed_at: now,
            access_count: 0,
            importance: 1.0,
            ttl_secs: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase 12.1: reserved `_mnemo.*` metadata keys for in-place updates + keyed
// supersession. These live in `Memory.metadata` (which is already
// `Map<String, Value>` on the record), so nothing in the on-disk record
// layout changes — v0.7.x readers see them as ordinary user metadata and
// ignore them. Format bump to v9 (post-12.1) will promote the ones that
// pay for a typed catalog column.
// ---------------------------------------------------------------------------

/// Namespace prefix used for every v0.8.0+ reserved metadata key (R-001.10).
/// Keys outside this namespace are untouched user metadata.
pub const MNEMO_META_NS: &str = "_mnemo.";

/// Upsert slot name on a `remember(…, key = K)` write (12.1). Combined with
/// the memory's `scope` (and `agent_id` for `Private` scope) to identify a
/// single live memory per slot: a newer `remember(key=K)` supersedes the
/// prior one.
pub const MNEMO_META_KEY: &str = "_mnemo.key";

/// Set on the newly-inserted record when a `remember(key=K)` supersedes an
/// earlier record. Value: the superseded record's ULID string.
pub const MNEMO_META_SUPERSEDES: &str = "_mnemo.supersedes";

/// Set on the earlier record when a newer `remember(key=K)` has replaced
/// it. Value: the new record's ULID string. Together with `_mnemo.invalid_at`
/// this marks the record as "superseded, not current" — default `recall`,
/// `about` and the key index skip it; `history(key)` walks the chain.
pub const MNEMO_META_SUPERSEDED_BY: &str = "_mnemo.superseded_by";

/// Set on the earlier record when superseded. Value: unix seconds (i64).
/// Not interpreted by the engine's scorer — purely for audit / `history`.
pub const MNEMO_META_INVALID_AT: &str = "_mnemo.invalid_at";

/// Set on a record each time `Mnemo::update` succeeds. Value: unix seconds.
/// Lets callers distinguish an unchanged-since-creation record from one
/// that has been edited in place. Not interpreted by the scorer.
pub const MNEMO_META_UPDATED_AT: &str = "_mnemo.updated_at";

// ---------------------------------------------------------------------------
// Phase 12.7a: thread archive (ingest / export / forget). Reserved
// `_mnemo.*` keys that identify thread-turn records. All records with
// `_mnemo.kind = "turn"` are:
//   * excluded from default recall (opt in with a 12.6
//     `_mnemo.kind = "turn"` filter or the `--thread` CLI flag),
//   * written by `Mnemo::ingest_thread` + the `mnemo thread ingest`
//     CLI from a transcript file in either a generic JSONL shape or
//     Claude Code's JSONL (`~/.claude/projects/<p>/<s>.jsonl`),
//   * grouped by `_mnemo.thread = <id>` for `thread export` /
//     `thread forget` and the 12.7b essence render.
// ---------------------------------------------------------------------------

/// Record kind for thread-archive turns (Phase 12.7a). Value: `"turn"`.
pub const MNEMO_META_KIND: &str = "_mnemo.kind";

/// Thread identifier the record belongs to (Phase 12.7a). Value: a
/// caller-supplied string, typically the Claude Code session uuid or
/// the planner-chosen thread slug.
pub const MNEMO_META_THREAD: &str = "_mnemo.thread";

/// Monotonically increasing sequence number within the thread
/// (Phase 12.7a). Value: unsigned integer, 0-based. Lets
/// `thread export` reconstruct the transcript in order.
pub const MNEMO_META_SEQ: &str = "_mnemo.seq";

/// Role of the turn's author (Phase 12.7a). Value: `"user"`,
/// `"assistant"`, `"system"`, `"tool"`, or any other string the
/// source transcript used.
pub const MNEMO_META_ROLE: &str = "_mnemo.role";

/// Opaque per-turn identifier from the source transcript
/// (Phase 12.7a). Value: typically a uuid4. Used for idempotence —
/// `ingest_thread` skips any turn whose `turn_id` already exists on a
/// record in the target thread.
pub const MNEMO_META_TURN_ID: &str = "_mnemo.turn_id";

/// Zero-based part number for a turn that was chunked at the 8 KB
/// boundary during ingest (Phase 12.7a). Absent → the turn fits in a
/// single record.
pub const MNEMO_META_PART: &str = "_mnemo.part";

/// Hex-encoded SHA-256 of the original tool-result content
/// (Phase 12.7a). Set when the content exceeded the 16 KB truncation
/// cap during ingest; absent otherwise.
pub const MNEMO_META_SHA256: &str = "_mnemo.sha256";

/// Byte length of the original tool-result content (Phase 12.7a). Set
/// alongside `_mnemo.sha256` when truncation happened; absent
/// otherwise.
pub const MNEMO_META_LENGTH: &str = "_mnemo.length";

/// Marker value for `_mnemo.kind` on thread-turn records.
pub const MNEMO_META_KIND_TURN: &str = "turn";

/// Maximum content size for a single turn record before chunking kicks
/// in (Phase 12.7a). 8 KB per chunk keeps each record small enough
/// that the per-record page cost dominates — and so that a thread of
/// mostly short messages ingests as one record each.
pub const THREAD_TURN_CHUNK_BYTES: usize = 8 * 1024;

/// Maximum content size for a tool-result turn before truncation
/// (Phase 12.7a). Tool results frequently run tens of kilobytes;
/// stored truncated with `_mnemo.sha256` + `_mnemo.length` so the
/// original can be recovered (or verified) out-of-band.
pub const THREAD_TOOL_RESULT_TRUNCATE_BYTES: usize = 16 * 1024;

// ---------------------------------------------------------------------------
// Phase 12.7b: status-record core + essence (timeline + sections). The
// essence layer rides on the same reserved-metadata namespace as the
// thread archive, so a file keeps the single `_mnemo.*` convention:
//
// - `_mnemo.kind = "essence"` identifies an essence record. Section
//   is in `_mnemo.section`. Scope — thread or project — is in
//   `_mnemo.thread` (string) or absent (`None` means project scope).
// - `_mnemo.status` carries the lifecycle for section items
//   (`"open"` → `"resolved"`). Timeline entries never get a status.
// - `_mnemo.status_history` records the status transitions. One
//   element per transition: `{"status": "...", "at": <unix_secs>}`.
// - `_mnemo.refs` is an optional array of free-form reference
//   strings (turn ids, memory ids, commits, paths).
// ---------------------------------------------------------------------------

/// Reserved `_mnemo.section` key — which essence section an item
/// belongs to (Phase 12.7b). Values: `"timeline"`, `"intent"`,
/// `"conventions"`, `"decisions"`, `"state"`, `"open"`, `"next"`,
/// `"lessons"`. Timeline entries are append-only.
pub const MNEMO_META_SECTION: &str = "_mnemo.section";

/// Reserved `_mnemo.status` key — current status of a status-record.
/// Values are per-kind (Phase 12.7b: essence uses `"open"` and
/// `"resolved"`). Timeline entries never carry this.
pub const MNEMO_META_STATUS: &str = "_mnemo.status";

/// Reserved `_mnemo.status_history` key — array of status transitions,
/// each `{"status": <str>, "at": <unix_secs>}`. Append-only — the
/// first entry records creation, each later entry records a
/// transition.
pub const MNEMO_META_STATUS_HISTORY: &str = "_mnemo.status_history";

/// Reserved `_mnemo.refs` key — optional array of free-form reference
/// strings (turn ids, memory ids, commits, paths) attached to an
/// essence record (Phase 12.7b).
pub const MNEMO_META_REFS: &str = "_mnemo.refs";

/// Marker value for `_mnemo.kind` on essence records (Phase 12.7b).
pub const MNEMO_META_KIND_ESSENCE: &str = "essence";

/// Maximum content length for a timeline entry (Phase 12.7b).
/// Entries over this are rejected, not truncated — the planner's
/// spec is explicit about this because truncation would lose the
/// narrative, and splitting across records would lose ordering.
pub const ESSENCE_TIMELINE_MAX_BYTES: usize = 600;

/// Maximum content length for a section item (Phase 12.7b).
/// intent / conventions / decisions / state / open / next / lessons
/// all share the same single-line cap. Items over this are rejected.
pub const ESSENCE_SECTION_MAX_BYTES: usize = 280;

/// Default byte budget for [`crate::Mnemo::essence`] rendering
/// (Phase 12.7b). Matches the planner's "max_bytes = 6144" spec,
/// which sits comfortably below a 4K-token context window.
pub const ESSENCE_DEFAULT_RENDER_BYTES: usize = 6144;

/// Which essence section an item belongs to.
///
/// Rendering order is fixed at this type's declaration order:
/// intent → conventions → decisions → state → open → next → lessons
/// → timeline. See [`EssenceSection::render_order`] for the explicit
/// list and the [`crate::Mnemo::essence`] docs for per-section rules
/// (timeline is append-only, every other section is item-edited).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EssenceSection {
    /// What the thread is trying to do.
    Intent,
    /// How the user works — conventions that survive across threads.
    Conventions,
    /// Decisions made and why (newest first on render).
    Decisions,
    /// The current state of the work.
    State,
    /// Open problems.
    Open,
    /// What to do next.
    Next,
    /// Lessons learned — mistakes caught and their fixes.
    Lessons,
    /// Append-only milestone timeline. Entries capped at
    /// [`ESSENCE_TIMELINE_MAX_BYTES`] and never editable.
    Timeline,
}

impl EssenceSection {
    /// Section → stable `_mnemo.section` string.
    pub fn as_str(&self) -> &'static str {
        match self {
            EssenceSection::Intent => "intent",
            EssenceSection::Conventions => "conventions",
            EssenceSection::Decisions => "decisions",
            EssenceSection::State => "state",
            EssenceSection::Open => "open",
            EssenceSection::Next => "next",
            EssenceSection::Lessons => "lessons",
            EssenceSection::Timeline => "timeline",
        }
    }

    /// Parse a string from `_mnemo.section` into the typed enum.
    /// Case-insensitive; returns `None` on unknown input.
    pub fn parse(s: &str) -> Option<EssenceSection> {
        match s.to_ascii_lowercase().as_str() {
            "intent" => Some(EssenceSection::Intent),
            "conventions" => Some(EssenceSection::Conventions),
            "decisions" => Some(EssenceSection::Decisions),
            "state" => Some(EssenceSection::State),
            "open" => Some(EssenceSection::Open),
            "next" => Some(EssenceSection::Next),
            "lessons" => Some(EssenceSection::Lessons),
            "timeline" => Some(EssenceSection::Timeline),
            _ => None,
        }
    }

    /// Fixed render order (intent → conventions → decisions → state
    /// → open → next → lessons → timeline). The [`crate::Mnemo::essence`]
    /// render walks this list, allocating a per-section byte budget
    /// proportional to section importance in the user-centric order.
    pub fn render_order() -> &'static [EssenceSection] {
        &[
            EssenceSection::Intent,
            EssenceSection::Conventions,
            EssenceSection::Decisions,
            EssenceSection::State,
            EssenceSection::Open,
            EssenceSection::Next,
            EssenceSection::Lessons,
            EssenceSection::Timeline,
        ]
    }

    /// `true` for sections whose items carry an `open → resolved`
    /// status lifecycle (every section except [`EssenceSection::Timeline`]).
    pub fn has_status_lifecycle(&self) -> bool {
        !matches!(self, EssenceSection::Timeline)
    }

    /// Per-section byte cap for new items. Timeline: 600 bytes.
    /// Every other section: 280 bytes.
    pub fn item_max_bytes(&self) -> usize {
        match self {
            EssenceSection::Timeline => ESSENCE_TIMELINE_MAX_BYTES,
            _ => ESSENCE_SECTION_MAX_BYTES,
        }
    }
}

/// Scope selector for an essence item (Phase 12.7b). Thread-scoped
/// items belong to one `_mnemo.thread`; project-scoped items are
/// shared across every thread in the file.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum EssenceScope {
    /// Belongs to one thread (the string). Shows up in that
    /// thread's render only.
    Thread(String),
    /// Shared across every thread in the file. Shows up in every
    /// thread's render, and in the `about` briefing: the CLI and MCP
    /// `about` append the project render after the manifest, from
    /// `Mnemo::about_essence()` (12.7d).
    Project,
}

/// Status values the essence section supports (Phase 12.7b). More
/// kinds (tasks: `queued → active → ready → done | blocked`;
/// findings: `open → accepted | deferred | rejected | needs-user →
/// verified`) ship with Phase 13 reusing the same status-record
/// core; the transition tables for those are declared as constants
/// in this crate but currently unused.
pub mod status {
    /// Freshly-added section item.
    pub const OPEN: &str = "open";
    /// The user-facing concern has been addressed.
    pub const RESOLVED: &str = "resolved";
}

/// Task transition table (declared for Phase 13; currently unused
/// in the engine). `queued → active → ready → done | blocked`,
/// with `blocked → active` as the only back-edge.
pub const TASK_TRANSITIONS: &[(&str, &str)] = &[
    ("queued", "active"),
    ("active", "ready"),
    ("active", "blocked"),
    ("blocked", "active"),
    ("ready", "done"),
];

/// Finding transition table (declared for Phase 13; currently unused
/// in the engine). `open → {accepted | deferred | rejected |
/// needs-user} → verified`.
pub const FINDING_TRANSITIONS: &[(&str, &str)] = &[
    ("open", "accepted"),
    ("open", "deferred"),
    ("open", "rejected"),
    ("open", "needs-user"),
    ("accepted", "verified"),
    ("deferred", "verified"),
    ("needs-user", "verified"),
];

/// Essence transition table for Phase 12.7b. Only `open → resolved`
/// is allowed. Timeline entries don't use this (they're append-only
/// and have no status).
pub const ESSENCE_TRANSITIONS: &[(&str, &str)] = &[(status::OPEN, status::RESOLVED)];

// ---------------------------------------------------------------------------
// Phase 12.7c: continuation + commit_deltas
// ---------------------------------------------------------------------------

/// Reserved `_mnemo.continues` key (Phase 12.7c). Set on a
/// thread-start marker record to point at the previous thread whose
/// context the new thread continues from. Value: the prior thread id
/// (string). Rendering a continued thread pulls the prior thread's
/// open items, `next` and last-N timeline entries ahead of the new
/// thread's own essence.
pub const MNEMO_META_CONTINUES: &str = "_mnemo.continues";

/// Marker value for `_mnemo.kind` on a thread-start record (one per
/// `thread_start(new, --from old)` call, carrying the link in
/// `_mnemo.continues`).
pub const MNEMO_META_KIND_THREAD_LINK: &str = "thread_link";

/// How many timeline entries from the predecessor thread the
/// continuation render pulls in (Phase 12.7c). Small enough to keep
/// the render under the default 6 KB budget, large enough to carry a
/// handful of milestones.
pub const CONTINUATION_TIMELINE_ENTRIES: usize = 10;

/// What [`crate::Mnemo::commit_deltas`] does with the `turns` carried
/// in the batch (Phase 12.7c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TurnPolicy {
    /// Archive every turn in the batch via
    /// [`crate::Mnemo::ingest_thread`]. Default — matches the
    /// "lossless flow preservation" story of 12.7.
    #[default]
    Archive,
    /// Store turns with a TTL (unix-seconds expiry).
    ExpireAfter(i64),
    /// Drop turns on the floor (no archive record written).
    Drop,
}

/// Which op a [`EssenceOp`] carries. Mirrors the per-method
/// `Mnemo::essence_*` surface so one batch can mix section + timeline
/// edits in one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EssenceOpKind {
    /// Section item add (any section except timeline).
    Add,
    /// Append-only timeline entry.
    AppendTimeline,
    /// In-place update of an existing section item.
    Update,
    /// Transition an item `open → resolved`.
    Resolve,
    /// Soft-delete an item.
    Remove,
}

/// One essence op inside a [`CommitBatch`] (Phase 12.7c). Validation
/// (missing/extra fields per kind) runs up front in
/// [`crate::Mnemo::commit_deltas`] — a single bad op aborts the
/// whole batch before any write lands.
#[derive(Debug, Clone)]
pub struct EssenceOp {
    /// Which op to apply.
    pub kind: EssenceOpKind,
    /// Scope — required for [`EssenceOpKind::Add`] and
    /// [`EssenceOpKind::AppendTimeline`].
    pub scope: Option<EssenceScope>,
    /// Section — required for [`EssenceOpKind::Add`].
    pub section: Option<EssenceSection>,
    /// Target record id — required for [`EssenceOpKind::Update`],
    /// [`EssenceOpKind::Resolve`], [`EssenceOpKind::Remove`].
    pub id: Option<Ulid>,
    /// Content text — required for [`EssenceOpKind::Add`],
    /// [`EssenceOpKind::AppendTimeline`], [`EssenceOpKind::Update`].
    pub text: Option<String>,
    /// Optional refs for ops that carry text.
    pub refs: Option<Vec<String>>,
}

/// One keyed fact inside a [`CommitBatch`] (Phase 12.7c).
#[derive(Debug, Clone)]
pub struct KeyedMemory {
    /// The memory to store. An **empty `vector`** means "embed
    /// `content` with the registered embedder" (12.7d): every such fact
    /// in a batch is embedded together in one `Embedder::embed` call.
    pub memory: Memory,
    /// Optional 12.1 upsert key.
    pub key: Option<String>,
}

/// Atomic write batch for [`crate::Mnemo::commit_deltas`]
/// (Phase 12.7c). The planner's "one call that composes facts +
/// essence + turns" shape: everything validates up front, then every
/// write runs under a single `flush()`.
#[derive(Debug, Clone, Default)]
pub struct CommitBatch {
    /// Keyed-or-not facts to remember (12.1).
    pub facts: Vec<KeyedMemory>,
    /// Essence ops to apply (12.7b).
    pub essence_ops: Vec<EssenceOp>,
    /// Thread archive turns to ingest (12.7a).
    pub turns: Vec<ThreadTurn>,
    /// Destination thread id for `turns`. Required when `turns` is
    /// non-empty.
    pub thread_id: Option<String>,
    /// What to do with `turns` — defaults to `Archive`.
    pub turn_policy: TurnPolicy,
}

/// Result of [`crate::Mnemo::commit_deltas`] (Phase 12.7c).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitReport {
    /// Facts added (plain or `WriteOutcome::Added`).
    pub facts_added: usize,
    /// Facts whose keyed write superseded a prior record.
    pub facts_superseded: usize,
    /// Keyed facts whose write was a no-op (matched live record).
    pub facts_unchanged: usize,
    /// Essence ops that produced an Added/Updated result.
    pub essence_ops_applied: usize,
    /// Essence ops that were no-ops (e.g. `resolve` on resolved).
    pub essence_ops_unchanged: usize,
    /// Turn records written to the archive.
    pub turns_added: usize,
    /// Source turns skipped by the archive's idempotence check.
    pub turns_skipped: usize,
    /// Source turns whose content was truncated at the 16 KB cap.
    pub turns_truncated: usize,
    /// Source turns split into multiple 8 KB chunks.
    pub turns_chunked: usize,
}

/// Patch body for [`crate::Mnemo::update`] (Phase 12.1): every field is
/// optional, and only the `Some(...)` fields overwrite the corresponding
/// field on the target memory. All fields can be set independently.
///
/// `content` paired with `vector` updates both at once — the engine does
/// not re-embed under `update` (use `update_text` for that). A `content`
/// change without a new `vector` is rejected to prevent the embedding
/// drifting silently out of sync with the text.
///
/// `metadata` **replaces** the full user-metadata map — the engine's own
/// `_mnemo.*` reserved keys on the target record are preserved across
/// the replacement (updated-at timestamp, supersession markers, etc.).
/// Passing an empty map drops every user-visible metadata key but keeps
/// the reserved ones.
///
/// `ttl_secs` is `Option<Option<i64>>` so a caller can distinguish
/// "leave it alone" (`None`) from "set it to a value" (`Some(Some(s))`)
/// and "clear it" (`Some(None)`).
#[derive(Debug, Clone, Default)]
pub struct MemoryPatch {
    /// New text content. If set, a matching `vector` must also be set
    /// (or call `update_text`, which re-embeds through the registered
    /// embedder).
    pub content: Option<String>,
    /// New embedding. Must match the database's dimensionality.
    pub vector: Option<Vec<f32>>,
    /// New importance in `[0, 1]`.
    pub importance: Option<f32>,
    /// Replace the memory type.
    pub memory_type: Option<MemoryType>,
    /// Replace the user-visible metadata (preserves `_mnemo.*` reserved
    /// keys).
    pub metadata: Option<Map<String, Value>>,
    /// Change the TTL. Outer `None` → leave unchanged; `Some(Some(s))` →
    /// set TTL to `s`; `Some(None)` → clear any existing TTL.
    pub ttl_secs: Option<Option<i64>>,
}

/// One turn fed into [`crate::Mnemo::ingest_thread`] (Phase 12.7a).
///
/// The caller supplies `turn_id` (opaque, for idempotence), `role`
/// (the transcript's author tag; no interpretation), and `content`
/// (UTF-8 text; chunked at 8 KB, or truncated at 16 KB when `role ==
/// "tool"`). The ingest path assigns `_mnemo.seq` monotonically within
/// the destination thread and embeds through the registered embedder.
///
/// Tool messages whose content exceeds [`THREAD_TOOL_RESULT_TRUNCATE_BYTES`]
/// are stored truncated. Other roles over [`THREAD_TURN_CHUNK_BYTES`]
/// are chunked.
#[derive(Debug, Clone)]
pub struct ThreadTurn {
    /// Opaque per-turn id from the source transcript (e.g. a Claude
    /// Code message uuid). Used for `ingest_thread`'s idempotence
    /// check: a turn whose `turn_id` already exists in the target
    /// thread is skipped.
    pub turn_id: String,
    /// Author role verbatim from the transcript (`"user"`,
    /// `"assistant"`, `"system"`, `"tool"`, …). `"tool"` triggers
    /// the 16 KB truncation branch; all other roles go through the
    /// 8 KB chunking branch.
    pub role: String,
    /// UTF-8 message content.
    pub content: String,
    /// Optional caller-supplied creation timestamp (unix seconds).
    /// If `None`, the ingest path uses `now_secs()` so the record's
    /// `created_at` still sorts the thread in order. Transcripts
    /// that carry a per-turn timestamp should pass it through so a
    /// future `thread export --by-time` can honour it.
    pub timestamp: Option<i64>,
}

/// One turn of a thread's archive in the **ingest shape** (Phase 12.7d),
/// as returned by [`crate::Mnemo::thread_export_turns`] and by Python
/// `db.thread_export`, MCP `thread_export` and `mnemo thread export`.
///
/// Turns come back in `_mnemo.seq` order, one entry per source turn: the
/// 8 KB chunks of a long turn are joined back into one `content`, so
/// feeding the list to `ingest_thread` (same thread: every turn is
/// skipped; a new thread: the turns are re-chunked identically) round
/// trips. A tool result that was truncated at 16 KB carries `sha256` and
/// `original_length` of the original content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportedTurn {
    /// Source `turn_id` (`_mnemo.turn_id`).
    pub turn_id: String,
    /// Author role (`_mnemo.role`).
    pub role: String,
    /// Stored text, with chunks joined in `_mnemo.part` order.
    pub content: String,
    /// The turn's timestamp (unix seconds; the record's `created_at`).
    pub timestamp: i64,
    /// Hex SHA-256 of the original content, set only for a truncated
    /// tool result.
    pub sha256: Option<String>,
    /// Byte length of the original content, set only for a truncated
    /// tool result.
    pub original_length: Option<u64>,
}

impl ExportedTurn {
    /// The JSON object shared by the MCP tool and the CLI JSONL export:
    /// `{turn_id, role, content, timestamp}` plus `sha256` and
    /// `original_length` when the turn was truncated.
    pub fn to_json(&self) -> serde_json::Value {
        let mut obj = serde_json::json!({
            "turn_id": self.turn_id,
            "role": self.role,
            "content": self.content,
            "timestamp": self.timestamp,
        });
        if let Some(sha) = &self.sha256 {
            obj["sha256"] = serde_json::Value::String(sha.clone());
        }
        if let Some(len) = self.original_length {
            obj["original_length"] = serde_json::Value::Number(len.into());
        }
        obj
    }
}

/// Result of one [`crate::Mnemo::ingest_thread`] call (Phase 12.7a).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IngestReport {
    /// Number of **new** records written (turn chunks after chunking
    /// / truncation). A long turn counts as however many chunks it
    /// produced.
    pub added_records: usize,
    /// Number of **source turns** that already had a record in the
    /// target thread and were skipped by the idempotence check.
    pub skipped_existing_turns: usize,
    /// Number of turns whose content was truncated at the 16 KB cap.
    /// Only `role == "tool"` turns can trigger this branch.
    pub truncated_turns: usize,
    /// Number of turns that were split into multiple chunks at the
    /// 8 KB boundary. A truncated turn counts as 0 here even if its
    /// pre-truncation content would have chunked.
    pub chunked_turns: usize,
}

/// Scope selector for a keyed slot (Phase 12.1.1). A keyed
/// `remember`'s slot is always identified by `(scope_owner, key)`:
/// `Private(agent_id)` slots are per-agent, `Shared` slots are per-file.
/// `history_in(key, scope)` uses the same selector so the lookup matches
/// the write exactly.
///
/// The convenience [`crate::Mnemo::history`] is equivalent to
/// `history_in(key, KeyScope::Private(agent.unwrap_or("default")))` —
/// i.e. `history(key, None)` resolves to the default agent's private
/// slot, matching the `remember(…, agent_id=None, scope=Private, key=K)`
/// write path. Use [`crate::Mnemo::history_in`] with
/// [`KeyScope::Shared`] when you need the shared slot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyScope {
    /// Per-agent slot. The agent id is the one carried on `Memory.agent_id`
    /// at write time; `remember` with no explicit agent uses `"default"`.
    Private(String),
    /// Per-file slot shared across every agent.
    Shared,
}

impl KeyScope {
    /// Default agent id used when `Memory::new(...)` is called without
    /// `.with_agent(...)` — same `"default"` the [`Memory::new`]
    /// constructor seeds into `Memory.agent_id`.
    pub const DEFAULT_AGENT: &'static str = "default";

    /// Build the private slot for the default agent. Shorthand for
    /// `KeyScope::Private(KeyScope::DEFAULT_AGENT.to_string())`.
    pub fn private_default() -> Self {
        KeyScope::Private(Self::DEFAULT_AGENT.to_string())
    }
}

/// Outcome of a 12.1 write operation (`remember` with `key`, or `update`).
///
/// Plain `remember` without a key always returns a ULID, not a
/// `WriteOutcome`, because supersession and NOOP detection are tied to
/// the key slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    /// Keyed write with no prior record for this slot. A brand-new record
    /// was inserted, no record was superseded.
    Added,
    /// Keyed write whose content / type / importance / user metadata
    /// differs from the live record for this slot. A new record was
    /// inserted and the prior one was marked
    /// `_mnemo.superseded_by` / `_mnemo.invalid_at`.
    Superseded,
    /// No-op: the incoming keyed write is byte-equal to the live record
    /// for this slot (content, memory type, importance, user metadata).
    /// No record was written and no catalog page was dirtied. The
    /// returned id is the existing record's id.
    Unchanged,
    /// In-place `update`: fields changed on the target record. The record
    /// was rewritten copy-on-write so the previous version remains
    /// visible through snapshots.
    Updated,
}

// ---------------------------------------------------------------------------
// Phase 12.6: recall filters (session_id + per-metadata-key conditions)
// ---------------------------------------------------------------------------

/// One AND-ed condition applied to a candidate memory's metadata during
/// filtered recall (Phase 12.6). Terms are combined conjunctively — a
/// memory passes the filter only if *every* term evaluates true against
/// its `metadata` map. See [`Filter`] for the surface API.
///
/// Values use `serde_json::Value` so the same term shape serializes
/// cleanly through the MCP / Python / CLI bridges without a parallel
/// enum per surface.
#[derive(Debug, Clone)]
pub enum FilterTerm {
    /// The memory's `metadata[key]` must be set to any value.
    Exists(String),
    /// `metadata[key]` must equal this JSON value (strict JSON equality:
    /// strings compare case-sensitively, numbers compare as f64).
    Eq(String, Value),
    /// `metadata[key]` must equal one of these JSON values.
    In(String, Vec<Value>),
    /// `metadata[key]` must be a number in `[min, max]` (inclusive on
    /// both ends; `None` on either bound means "no bound on that side").
    /// A non-numeric metadata value fails the term.
    Range {
        /// Metadata key.
        key: String,
        /// Lower bound (inclusive) — `None` means unbounded below.
        min: Option<f64>,
        /// Upper bound (inclusive) — `None` means unbounded above.
        max: Option<f64>,
    },
}

/// Pre-scoring filter applied by [`crate::Mnemo::recall`] (Phase 12.6).
///
/// `session_id` restricts candidates to a single session (when the
/// memory's `session_id` is set). The `terms` are AND-ed metadata
/// conditions evaluated against each candidate's `metadata` map; see
/// [`FilterTerm`] for the shapes.
///
/// When an ANN index is present, the filter is applied *after* the ANN
/// shortlist step, so a selective filter can starve the result below
/// `top_k`. Recall handles that automatically via the Phase 12.1
/// short-results-on-indexed-path fix (widen 2×, then exact scan).
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Only candidates whose `Memory.session_id` equals this value.
    /// Candidates without a session_id are excluded when this is set.
    pub session_id: Option<String>,
    /// AND-ed metadata conditions. Empty → no metadata constraints.
    pub terms: Vec<FilterTerm>,
}

impl Filter {
    /// Builder helper: a filter that keeps only memories in `session`.
    pub fn new_session(session: impl Into<String>) -> Self {
        Self {
            session_id: Some(session.into()),
            terms: Vec::new(),
        }
    }

    /// Restrict to one session_id (builder form).
    pub fn with_session(mut self, session: impl Into<String>) -> Self {
        self.session_id = Some(session.into());
        self
    }

    /// Require `metadata[key]` to exist (any value).
    pub fn metadata_exists(mut self, key: impl Into<String>) -> Self {
        self.terms.push(FilterTerm::Exists(key.into()));
        self
    }

    /// Require `metadata[key]` to equal `value`.
    pub fn metadata_eq(mut self, key: impl Into<String>, value: Value) -> Self {
        self.terms.push(FilterTerm::Eq(key.into(), value));
        self
    }

    /// Require `metadata[key]` to be one of `values`.
    pub fn metadata_in(mut self, key: impl Into<String>, values: Vec<Value>) -> Self {
        self.terms.push(FilterTerm::In(key.into(), values));
        self
    }

    /// Require `metadata[key]` to be a number in `[min, max]`.
    pub fn metadata_range(
        mut self,
        key: impl Into<String>,
        min: Option<f64>,
        max: Option<f64>,
    ) -> Self {
        self.terms.push(FilterTerm::Range {
            key: key.into(),
            min,
            max,
        });
        self
    }

    /// `true` iff the filter has no terms and no `session_id`. The
    /// recall hot path uses this to skip the per-candidate filter
    /// check entirely.
    pub fn is_empty(&self) -> bool {
        self.session_id.is_none() && self.terms.is_empty()
    }

    /// Phase 12.7a: `true` iff the filter mentions any reserved
    /// `_mnemo.*` metadata key. Used by `recall` to decide whether
    /// to default-exclude thread-archive turns
    /// (`_mnemo.kind = "turn"`). A caller that wants turns in the
    /// result opts in by adding any `_mnemo.*` term to the filter
    /// — equality, `$in`, `$exists`, or range — because those are the
    /// only queries that can usefully target turn records.
    pub fn mentions_mnemo_meta(&self) -> bool {
        self.terms.iter().any(|term| {
            let key = match term {
                FilterTerm::Exists(k) => k,
                FilterTerm::Eq(k, _) => k,
                FilterTerm::In(k, _) => k,
                FilterTerm::Range { key, .. } => key,
            };
            key.starts_with(MNEMO_META_NS)
        })
    }

    /// Evaluate the filter against one memory. Returns `true` if the
    /// memory passes every term.
    pub fn matches(&self, m: &Memory) -> bool {
        if let Some(s) = &self.session_id {
            match &m.session_id {
                Some(ms) if ms == s => {}
                _ => return false,
            }
        }
        for term in &self.terms {
            if !term_matches(term, &m.metadata) {
                return false;
            }
        }
        true
    }
}

fn term_matches(term: &FilterTerm, metadata: &Map<String, Value>) -> bool {
    match term {
        FilterTerm::Exists(key) => metadata.contains_key(key),
        FilterTerm::Eq(key, v) => metadata.get(key).is_some_and(|got| values_equal(got, v)),
        FilterTerm::In(key, options) => metadata
            .get(key)
            .is_some_and(|got| options.iter().any(|opt| values_equal(got, opt))),
        FilterTerm::Range { key, min, max } => metadata
            .get(key)
            .and_then(|v| v.as_f64())
            .is_some_and(|n| min.map_or(true, |m_| n >= m_) && max.map_or(true, |m_| n <= m_)),
    }
}

/// JSON value equality for filter terms. serde_json's `Value: PartialEq`
/// compares numbers by exact representation (integer vs float). The
/// filter surface should treat `5` and `5.0` as equal, so we compare
/// numbers through `as_f64` when both sides are numbers; everything
/// else falls back to structural equality.
fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => match (x.as_f64(), y.as_f64()) {
            (Some(xf), Some(yf)) => (xf - yf).abs() < f64::EPSILON,
            _ => x == y,
        },
        (x, y) => x == y,
    }
}

/// Distance/similarity metric for vector comparison.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Metric {
    /// Cosine similarity (orientation only).
    Cosine,
    /// Negative Euclidean distance, mapped into a `(0,1]` similarity.
    L2,
    /// Raw dot product.
    Dot,
}

/// Compute a *similarity* (higher = more relevant) under the given metric.
pub fn similarity(metric: Metric, a: &[f32], b: &[f32]) -> f32 {
    match metric {
        Metric::Cosine => cosine(a, b),
        Metric::Dot => dot(a, b),
        Metric::L2 => {
            let d = euclidean(a, b);
            1.0 / (1.0 + d)
        }
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Cosine similarity in `[-1.0, 1.0]`; returns `0.0` if either vector
/// has zero magnitude. `pub(crate)` so [`crate::Mnemo::cache_get_semantic`]
/// (Phase 10.2) can reuse the same scoring routine that recall uses,
/// keeping the two paths semantically identical.
pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let na = dot(a, a).sqrt();
    let nb = dot(b, b).sqrt();
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot(a, b) / (na * nb)
    }
}

fn euclidean(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

/// Weights for the multi-signal recall score (scoring v2, shipped in
/// v0.7.0).
///
/// ```text
/// score = similarity
///       + max(similarity, 0)
///         · ( β · recency
///           + γ · (importance − 0.5)
///           + δ · tanh(access_count / 5) )
///
/// recency = exp( -ln(2) · age_secs / half_life_secs )
/// age_secs = now - memory.created_at
/// ```
///
/// Design notes (from the 2026-11-06 scoring-v2 bake-off; see the
/// `ranking_v2` harness at `mnemo/examples/ranking_v2.rs`):
/// * **Similarity-gated.** The recency / importance / usage terms are
///   multiplied by `max(similarity, 0)`, so a stale-but-semantically-off
///   memory can't ride recency or importance past a semantically-on-point
///   one. Pre-v0.7.0 scoring was purely additive.
/// * **Importance is centered at 0.5.** A default-importance memory
///   contributes zero importance bonus; importance only pushes a score up
///   or down from that reference point.
/// * **Usage is saturating.** `tanh(uses/5)` caps the feedback bonus so
///   a heavily-returned memory can't keep accumulating weight past a
///   plateau. At 5 uses the bonus is ~76% of its asymptote; at 10, ~96%.
/// * **Recency is decoupled from the feedback loop.** Age is measured
///   from `created_at`, not `accessed_at`, so a `recall()` call does not
///   bump its own results' recency scores on later queries.
///
/// The output of this scorer is on a **different scale** from the
/// pre-v0.7.0 formula. Callers that compared raw scores to a fixed
/// threshold should re-pick the threshold against the new scale.
#[derive(Clone, Copy, Debug)]
pub struct ScoreWeights {
    /// Weight on semantic similarity. Kept in the struct for calibration
    /// hooks and symmetry with the other weights; scoring v2 always uses
    /// it as `1.0` because the leading `similarity` term is ungated and
    /// the whole score sits on the cosine-similarity scale by design.
    pub alpha: f32,
    /// Weight on the recency bonus (multiplied by `max(sim, 0)`).
    pub beta: f32,
    /// Weight on the importance bonus `(importance − 0.5)`, centered at
    /// the default-importance 0.5 (multiplied by `max(sim, 0)`).
    pub gamma: f32,
    /// Weight on the saturating usage bonus `tanh(access_count / 5)`
    /// (multiplied by `max(sim, 0)`).
    pub delta: f32,
    /// Half-life of the recency term, in seconds.
    pub half_life_secs: f32,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        // Tuned on the Phase 6 corpus + the ranking_v2 age-spread
        // fixture; see `ranking_v2.checks` in perf_v0.5.0.json.
        Self {
            alpha: 1.0,
            beta: 0.3,
            gamma: 0.2,
            delta: 0.1,
            half_life_secs: 7.0 * 24.0 * 3600.0,
        }
    }
}

/// Saturation constant K for the usage term `tanh(access_count / K)`.
/// K = 5 means ~76% of the asymptote at 5 uses and ~96% at 10 — matches
/// the "a few repeats is signal; past that it's a habit" intuition.
const USAGE_SAT_K: f32 = 5.0;

impl ScoreWeights {
    /// Combine the four signals into a single score. See the struct docs
    /// for the full formula and the scoring-v2 rationale.
    pub fn score(&self, similarity: f32, age_secs: f32, importance: f32, access_count: u32) -> f32 {
        // Recency is in [0, 1]: 1.0 at age 0, 0.5 at one half-life, etc.
        let recency = (-std::f32::consts::LN_2 * age_secs.max(0.0) / self.half_life_secs).exp();
        // Saturating usage in [0, 1); tanh caps feedback-loop drift.
        let usage = (access_count as f32 / USAGE_SAT_K).tanh();
        // Gate the non-similarity bonuses by `max(sim, 0)` so a
        // negative-cosine candidate (should be rare after the metric
        // layer, but possible with `Metric::Dot` or short-text L2)
        // can't collect positive bonuses, and a zero-similarity
        // candidate collects no bonus either.
        let gate = similarity.max(0.0);
        self.alpha * similarity
            + gate * (self.beta * recency + self.gamma * (importance - 0.5) + self.delta * usage)
    }
}
