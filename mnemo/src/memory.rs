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
    /// 13a (13.5): `public`, `team` or `private` (checked when it's
    /// written).
    pub fn with_visibility(mut self, level: &str) -> Self {
        self.metadata
            .insert(MNEMO_META_VISIBILITY.into(), Value::String(level.into()));
        self
    }
    /// 13a (13.6): when the content was true, in unix seconds.
    pub fn with_as_of(mut self, unix_secs: i64) -> Self {
        self.metadata
            .insert(MNEMO_META_AS_OF.into(), Value::Number(unix_secs.into()));
        self
    }
    /// 13a (13.6): when someone should look at it again, in unix seconds.
    pub fn with_review_by(mut self, unix_secs: i64) -> Self {
        self.metadata
            .insert(MNEMO_META_REVIEW_BY.into(), Value::Number(unix_secs.into()));
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
        // (all-zero refused — see store/records.rs). This spike is non-zero
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

/// 13a: `_mnemo.kind` of a message between agents (13.1).
pub const MNEMO_META_KIND_MESSAGE: &str = "message";
/// 13a: `_mnemo.kind` of a task in the queue (13.1).
pub const MNEMO_META_KIND_TASK: &str = "task";
/// 13a: `_mnemo.kind` of a claim on a topic (13.3).
pub const MNEMO_META_KIND_CLAIM: &str = "claim";
/// 13a: `_mnemo.kind` of an agent's presence record (13.3).
pub const MNEMO_META_KIND_PRESENCE: &str = "presence";
/// v0.18.0 (13a step 8b): `_mnemo.kind` of a review finding.
pub const MNEMO_META_KIND_FINDING: &str = "finding";
/// v0.21.0 (13.7): `_mnemo.kind` of a tombstone, the record a delete
/// leaves so a merge can tell a deleted record from one that never
/// existed. It holds the deleted id, its key slot if it had one, the
/// clock and the time, and never any of the deleted record's content.
pub const MNEMO_META_KIND_TOMBSTONE: &str = "tombstone";
/// v0.21.0: a tombstone's deleted id.
pub const MNEMO_META_TOMBSTONE_OF: &str = "_mnemo.tombstone_of";
/// v0.21.0: a tombstone's deleted record's key, when it had one.
pub const MNEMO_META_TOMBSTONE_KEY: &str = "_mnemo.tombstone_key";
/// v0.21.0: the agent whose private key slot the deleted record was in
/// (absent for a shared slot, or a record with no key).
pub const MNEMO_META_TOMBSTONE_OWNER: &str = "_mnemo.tombstone_owner";
/// v0.21.0: when a tombstone's record was deleted, in unix seconds.
pub const MNEMO_META_DELETED_AT: &str = "_mnemo.deleted_at";
/// v0.21.0 (RB-019.2): a deleted turn's thread id, on its tombstone, so a
/// merge removes every copy of the turn (a turn saved twice has two ids).
pub const MNEMO_META_TOMBSTONE_THREAD: &str = "_mnemo.tombstone_thread";
/// v0.21.0 (RB-019.2): a deleted turn's `_mnemo.turn_id`, on its tombstone.
pub const MNEMO_META_TOMBSTONE_TURN: &str = "_mnemo.tombstone_turn";
/// v0.21.0 (RB-019.1): on the manifest, the first clock stamp a v0.21 or
/// later build wrote in this file, set once. A merge can't know of a
/// delete made before it, or by an older build.
pub const MNEMO_META_HLC_SINCE: &str = "_mnemo.hlc_since";
/// v0.21.0 (decision.v0210-ancestry): the versions a record had before its
/// in-place rewrites, each `[stamp, version]` (its `_mnemo.hlc` and
/// `_mnemo.ver`), the [`HLC_PREV_KEEP`] largest, newest first. A merge
/// takes a version in another version's list as its ancestor: the
/// descendant wins, with no conflict. Two copies can stamp the same
/// millisecond, so a stamp alone doesn't name a version.
pub const MNEMO_META_HLC_PREV: &str = "_mnemo.hlc_prev";
/// v0.21.0 (decision.v0210-ancestry): a hash of the version a stamped
/// write wrote (its content and metadata, with its stamp), so the next
/// in-place rewrite can name it in `_mnemo.hlc_prev`.
pub const MNEMO_META_VER: &str = "_mnemo.ver";
/// v0.21.0: how many earlier stamps `_mnemo.hlc_prev` keeps.
pub const HLC_PREV_KEEP: usize = 8;
/// v0.21.0 (13.7): a record a merge found in conflict (`true`): a key both
/// inputs wrote, an essence item, task or finding whose text and status
/// both differ, a renamed task or finding, or a conflict copy.
pub const MNEMO_META_MERGE_CONFLICT: &str = "_mnemo.merge_conflict";
/// v0.21.0 (13.7): on a merge's conflict copy, the id of the record whose
/// other version it holds. A tombstone for that id removes the copy too.
pub const MNEMO_META_MERGE_SOURCE: &str = "_mnemo.merge_source";
/// v0.21.0 (13.7): on a task or finding a merge renamed, the name both
/// inputs used.
pub const MNEMO_META_MERGE_NAME: &str = "_mnemo.merge_name";
/// v0.21.0 (13.7): how far past the clock a handle's largest stamp seen
/// may run before it says so ([`crate::Mnemo::notices`]): one day, in ms.
pub const HLC_AHEAD_NOTICE_MS: u64 = 86_400_000;
/// v0.21.0 (13.7): a hybrid logical clock stamp, on every write and
/// in-place update: unix milliseconds, raised past the largest stamp the
/// writing handle has seen (the record's own earlier stamp included), so
/// a handle's stamps never go backwards. A merge orders concurrent
/// changes by it. Older builds carry it through.
pub const MNEMO_META_HLC: &str = "_mnemo.hlc";
/// 13a: the coordination kinds. Each is an ordinary record in the shared
/// scope with the sentinel vector, never embedded, and never a hit in
/// `recall`, the keyword index, the ANN or `about`: each is read through
/// its own API (`docs/design/13a-coordination.md`, section 1).
pub const COORDINATION_KINDS: [&str; 6] = [
    MNEMO_META_KIND_MESSAGE,
    MNEMO_META_KIND_TASK,
    MNEMO_META_KIND_CLAIM,
    MNEMO_META_KIND_PRESENCE,
    MNEMO_META_KIND_FINDING,
    MNEMO_META_KIND_TOMBSTONE,
];

/// 13a: who wrote a record, the writing handle's author
/// ([`crate::Mnemo::set_author`], from `MNEMO_AGENT` in the CLI, the MCP
/// server and the hooks). Stamped by the engine on new records, never set
/// by a caller, and unproven until 13.4 signs it.
pub const MNEMO_META_AUTHOR: &str = "_mnemo.author";
/// 13a (13.5): `public`, `team` (the default when absent) or `private`. A
/// filter, not a security boundary, until 13b. A caller may set it.
pub const MNEMO_META_VISIBILITY: &str = "_mnemo.visibility";
/// 13a (13.6): when the content was true, in unix seconds. A caller may
/// set it; absent, it's the record's `created_at` (or `_mnemo.updated_at`).
pub const MNEMO_META_AS_OF: &str = "_mnemo.as_of";
/// 13a (13.6): when someone should look at the record again, in unix
/// seconds. A caller may set it.
pub const MNEMO_META_REVIEW_BY: &str = "_mnemo.review_by";
/// 13a (AGENT-DEFAULT): on the manifest, the agent a write that names none
/// is made for, in place of `default`. Set with
/// [`crate::Mnemo::set_default_agent`]; a new brain's manifest has none.
pub const MNEMO_META_DEFAULT_AGENT: &str = "_mnemo.default_agent";

/// 13a (13.5): a record's visibility level: `_mnemo.visibility`, or `team`
/// when it has none. A filter, not a security boundary: everyone holding
/// the key reads every record.
pub fn visibility_of(m: &Memory) -> &str {
    match m
        .metadata
        .get(MNEMO_META_VISIBILITY)
        .and_then(|v| v.as_str())
    {
        Some(v @ ("public" | "team" | "private")) => v,
        _ => "team",
    }
}

/// 13a (13.5): which visibility levels a read shows. By default public and
/// team; private only when asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisibilitySet {
    /// Show `public` records.
    pub public: bool,
    /// Show `team` records (and those with no level).
    pub team: bool,
    /// Show `private` records.
    pub private: bool,
}

impl Default for VisibilitySet {
    fn default() -> Self {
        VisibilitySet::DEFAULT
    }
}

impl VisibilitySet {
    /// Public and team: what every read shows unless asked.
    pub const DEFAULT: VisibilitySet = VisibilitySet {
        public: true,
        team: true,
        private: false,
    };
    /// Every level.
    pub const ALL: VisibilitySet = VisibilitySet {
        public: true,
        team: true,
        private: true,
    };

    /// From a list of levels (`public`, `team`, `private`) or `all`, as a
    /// surface's `--visibility` or `"visibility"` names them.
    pub fn parse<S: AsRef<str>>(levels: &[S]) -> crate::Result<VisibilitySet> {
        let mut v = VisibilitySet {
            public: false,
            team: false,
            private: false,
        };
        for l in levels {
            for part in l
                .as_ref()
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
            {
                match part {
                    "public" => v.public = true,
                    "team" => v.team = true,
                    "private" => v.private = true,
                    "all" => v = VisibilitySet::ALL,
                    other => {
                        return Err(crate::MnemoError::Invalid(format!(
                            "{other:?} isn't a visibility level: public, team, private or all"
                        )))
                    }
                }
            }
        }
        if !(v.public || v.team || v.private) {
            return Ok(VisibilitySet::DEFAULT);
        }
        Ok(v)
    }

    /// Whether a record's level is in the set.
    pub fn shows(&self, m: &Memory) -> bool {
        self.shows_level(visibility_of(m))
    }

    /// Whether a level is in the set.
    pub fn shows_level(&self, level: &str) -> bool {
        match level {
            "public" => self.public,
            "private" => self.private,
            _ => self.team,
        }
    }
}

/// 13a: `YYYY-MM-DD` (UTC) for unix seconds.
pub fn date_of(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// 13a: `YYYY-MM-DD HH:MM UTC` for unix seconds.
pub fn date_time_of(secs: i64) -> String {
    let r = secs.rem_euclid(86_400);
    format!("{} {:02}:{:02} UTC", date_of(secs), r / 3600, r % 3600 / 60)
}

/// 13a (13.6): the per-kind freshness defaults, in seconds.
pub mod stale_after {
    /// An essence `state` or `next` item.
    pub const ESSENCE_STATE_NEXT: i64 = 14 * 86_400;
    /// An essence `open` item.
    pub const ESSENCE_OPEN: i64 = 30 * 86_400;
    /// An `active` task with no update.
    pub const TASK_ACTIVE: i64 = 3 * 86_400;
    /// A `blocked` task.
    pub const TASK_BLOCKED: i64 = 7 * 86_400;
    /// An `unread` message.
    pub const MESSAGE_UNREAD: i64 = 3 * 86_400;
}

/// 13a (13.6): when a record went stale at `now`, if it has: past its
/// `_mnemo.review_by` (which, when set, decides alone), or older than its
/// kind's default (`stale_after`) measured from `_mnemo.as_of`, else its
/// last edit, else `created_at`. A fact with no `review_by`, a claim, a
/// presence record and a resolved or superseded item are never stale.
pub fn stale_since(m: &Memory, now: i64) -> Option<i64> {
    if m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) {
        return None;
    }
    let num = |k: &str| m.metadata.get(k).and_then(|v| v.as_i64());
    if let Some(r) = num(MNEMO_META_REVIEW_BY) {
        return (now >= r).then_some(r);
    }
    let base = num(MNEMO_META_AS_OF)
        .or_else(|| num(MNEMO_META_UPDATED_AT))
        .unwrap_or(m.created_at);
    let s = |k: &str| m.metadata.get(k).and_then(|v| v.as_str());
    let after = match s(MNEMO_META_KIND) {
        Some(MNEMO_META_KIND_ESSENCE) => {
            if s(MNEMO_META_STATUS) == Some(status::RESOLVED) {
                return None;
            }
            match s(MNEMO_META_SECTION) {
                Some("state" | "next") => stale_after::ESSENCE_STATE_NEXT,
                Some("open") => stale_after::ESSENCE_OPEN,
                _ => return None,
            }
        }
        Some(MNEMO_META_KIND_TASK) => {
            let last = m
                .metadata
                .get(MNEMO_META_STATUS_HISTORY)
                .and_then(|h| h.as_array())
                .and_then(|h| h.last())
                .and_then(|e| e.get("at"))
                .and_then(|v| v.as_i64())
                .unwrap_or(m.created_at);
            let after = match s(MNEMO_META_STATUS) {
                Some("active") => stale_after::TASK_ACTIVE,
                Some("blocked") => stale_after::TASK_BLOCKED,
                _ => return None,
            };
            let t = last + after;
            return (now >= t).then_some(t);
        }
        Some(MNEMO_META_KIND_MESSAGE) => {
            if s(MNEMO_META_STATUS) != Some(MESSAGE_UNREAD) {
                return None;
            }
            let t = m.created_at + stale_after::MESSAGE_UNREAD;
            return (now >= t).then_some(t);
        }
        _ => return None,
    };
    let t = base + after;
    (now >= t).then_some(t)
}

/// 13a (13.1): a message's recipient: an agent id, `role:<role>`, or `all`.
pub const MNEMO_META_TO: &str = "_mnemo.to";
/// 13a (13.1): a message's sender, as it said (unproven until 13b).
pub const MNEMO_META_FROM: &str = "_mnemo.from";
/// 13a (13.1): the conversation a message belongs to, so replies share its
/// hop count. Not `_mnemo.thread`, which names a thread archive.
pub const MNEMO_META_CONVERSATION: &str = "_mnemo.conversation";
/// 13a (13.1b): a message's hop count within its conversation (and its
/// pair, for 10 minutes), for the wake-up hop cap.
pub const MNEMO_META_HOPS: &str = "_mnemo.hops";
/// 13a (13.1b): `sent`, `delivered`, `seen` or `done`.
pub const MNEMO_META_DELIVERY: &str = "_mnemo.delivery";
/// 13a: when a message was acked, in unix seconds (for pruning).
pub const MNEMO_META_ACKED_AT: &str = "_mnemo.acked_at";
/// 13a (13.3): a claim's topic.
pub const MNEMO_META_TOPIC: &str = "_mnemo.topic";
/// 13a (13.3): the agent holding a claim.
pub const MNEMO_META_HOLDER: &str = "_mnemo.holder";
/// 13a (13.3): a claim's token; `renew`, `release` and `check` need it.
pub const MNEMO_META_CLAIM_TOKEN: &str = "_mnemo.claim_token";
/// 13a (13.3): when a claim lapses, in unix seconds.
pub const MNEMO_META_EXPIRES_AT: &str = "_mnemo.expires_at";
/// 13a (13.3): a claim's lease, in seconds.
pub const MNEMO_META_LEASE: &str = "_mnemo.lease";
/// 13a (13.3): whether the holder had a presence lock when it claimed (the
/// early lapse applies only then).
pub const MNEMO_META_HOLDER_LOCK: &str = "_mnemo.holder_lock";
/// 13a (13.3): when a taker first found the holder not alive.
pub const MNEMO_META_DEAD_SEEN_AT: &str = "_mnemo.dead_seen_at";
/// 13a (13.3): a presence record's agent.
pub const MNEMO_META_AGENT: &str = "_mnemo.agent";
/// 13a (13.3): the tool a presence record's agent runs in (`mcp`, `cli`).
pub const MNEMO_META_TOOL: &str = "_mnemo.tool";
/// 13a (13.3): when the agent last wrote, in unix seconds.
pub const MNEMO_META_LAST_ACTIVE: &str = "_mnemo.last_active";
/// 13a (13.3): whether the agent's process holds a presence lock, so `who`
/// can probe it.
pub const MNEMO_META_PRESENCE_LOCK: &str = "_mnemo.presence_lock";
/// 13a (13.3): a claim's default lease (15 minutes).
pub const CLAIM_TTL_DEFAULT: i64 = 15 * 60;
/// 13a (13.3): a claim's shortest lease.
pub const CLAIM_TTL_MIN: i64 = 30;
/// 13a (13.3): a claim's longest lease (24 hours).
pub const CLAIM_TTL_MAX: i64 = 24 * 3600;
/// 13a (13.3): the most often `last_active` is written.
pub const PRESENCE_EVERY_SECS: i64 = 60;
/// 13a (13.3): how long a holder must be seen not alive, twice, before its
/// claim lapses early.
pub const CLAIM_LAPSE_GRACE_SECS: i64 = 10;
/// 13a (13.3): a topic's limit, in bytes (one line).
pub const CLAIM_TOPIC_MAX: usize = 200;

/// 13a (13.1): a task's slug, its name (`step-4-tasks`).
pub const MNEMO_META_SLUG: &str = "_mnemo.slug";
/// 13a (13.1): the agent that took a task (`Mnemo::task_next`).
pub const MNEMO_META_OWNER: &str = "_mnemo.owner";
/// 13a (13.1): the agent that added a task; `ready → done` is its call.
pub const MNEMO_META_CREATOR: &str = "_mnemo.creator";
/// 13a (13.1): the slugs of the tasks a task waits for.
pub const MNEMO_META_AFTER: &str = "_mnemo.after";
/// 13a (13.1): the owner may go from `ready` to `done` without a sign-off.
pub const MNEMO_META_PRE_APPROVED: &str = "_mnemo.pre_approved";
/// 13a (13.1): a task's claim lease (2 hours), renewed by its owner's
/// updates.
pub const TASK_CLAIM_TTL: i64 = 2 * 3600;
/// 13a: a slug's limit (it's 1 to 64 of `a-z`, `0-9`, `.`, `_`, `-`).
pub const TASK_SLUG_MAX: usize = 64;
/// 13a: the brief warns about an own task's claim that lapses within this.
pub const TASK_CLAIM_WARN_SECS: i64 = 15 * 60;
/// 13a: `export --git` leaves out tasks done more than this ago (30 days).
pub const TASK_DONE_EXPORT_KEEP_SECS: i64 = 30 * 86_400;

/// 13a (13.1b): the wake-up guardrails. At most this many wake-ups per
/// recipient per hour; past it, items stay `sent` and are seen at the next
/// turn or session start.
pub const WAKE_PER_HOUR: usize = 12;
/// 13a (13.1b): a recipient's cooldown between wake-ups, in seconds.
pub const WAKE_COOLDOWN_SECS: i64 = 60;
/// 13a (13.1b): a message wakes nobody when its two agents have exchanged
/// this many messages, either way, in the last [`PAIR_BURST_SECS`].
pub const PAIR_BURST: usize = 8;
/// 13a (13.1b): the pair burst's window (10 minutes).
pub const PAIR_BURST_SECS: i64 = 600;
/// 13a (13.1b): an explicit reply whose hop count passes this wakes nobody.
pub const HOP_CAP: u64 = 8;
/// 13a (13.1b): the kill switch, on the manifest: `{"by", "at"}` while
/// wake-ups are paused.
pub const MNEMO_META_WAKE_PAUSED: &str = "_mnemo.wake_paused";
/// 13a (13.1b): a presence record's recent wake-up times (the last hour).
pub const MNEMO_META_WAKES: &str = "_mnemo.wakes";
/// 13a (RB-012.6 item 4): the agents a message to a role or everyone, or a
/// role's task, has been shown to.
pub const MNEMO_META_SEEN_BY: &str = "_mnemo.seen_by";
/// 13a (READY-NOTICE, v0.17.2): the agents a `ready` task has been shown
/// to as waiting for their `done`. Apart from [`MNEMO_META_SEEN_BY`]: a
/// creator of the task's own role was shown it while it was queued.
pub const MNEMO_META_READY_SEEN_BY: &str = "_mnemo.ready_seen_by";
/// 13a (13.1b): the claim expiry its holder was last warned about.
pub const MNEMO_META_WARNED_EXPIRY: &str = "_mnemo.warned_expiry";

/// 13a (13.1b): the delivery states, in order.
pub mod delivery {
    /// Written, not yet delivered by a wake-up.
    pub const SENT: &str = "sent";
    /// A wake-up reached the recipient's session.
    pub const DELIVERED: &str = "delivered";
    /// Shown to the recipient (the inbox, the per-turn hook).
    pub const SEEN: &str = "seen";
    /// Acked.
    pub const DONE: &str = "done";
}
/// 13a: a message's status: `unread` until acked.
pub const MESSAGE_UNREAD: &str = "unread";
/// 13a: a message's status once acked.
pub const MESSAGE_ACKED: &str = "acked";
/// 13a: a message body's limit, in bytes; a bigger one belongs in a fact,
/// named in `refs`.
pub const MESSAGE_BODY_MAX: usize = 8 * 1024;
/// 13a: a subject's limit, in bytes (one line).
pub const MESSAGE_SUBJECT_MAX: usize = 200;
/// 13a: a new message between the same two parties within this many seconds
/// carries their last hop count on (the pair carry).
pub const MESSAGE_PAIR_CARRY_SECS: i64 = 600;
/// 13a: acked messages older than this are dropped by `compact` and
/// `prune_messages` (30 days).
pub const MESSAGE_ACKED_KEEP_SECS: i64 = 30 * 86_400;

/// The visibility levels (13.5).
pub const VISIBILITY_LEVELS: [&str; 3] = ["public", "team", "private"];

/// 13a: the reserved keys a caller may set on a write (visibility and
/// freshness), each checked by [`check_caller_key`]. Every other
/// `_mnemo.*` key stays the engine's.
pub const CALLER_KEYS: [&str; 3] = [
    MNEMO_META_VISIBILITY,
    MNEMO_META_AS_OF,
    MNEMO_META_REVIEW_BY,
];

/// 13a: an agent id (`MNEMO_AGENT`, `_mnemo.default_agent`, a message's
/// `to`): 1 to 64 of `[A-Za-z0-9._-]`. A role (`MNEMO_ROLE`) follows the
/// same rule.
pub fn valid_agent_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Whether `key` is one of [`CALLER_KEYS`].
pub fn is_caller_key(key: &str) -> bool {
    CALLER_KEYS.contains(&key)
}

/// Check a caller-settable reserved key's value: a visibility level, or a
/// whole number of unix seconds that isn't negative.
pub fn check_caller_key(key: &str, value: &Value) -> std::result::Result<(), String> {
    match key {
        MNEMO_META_VISIBILITY => match value.as_str() {
            Some(v) if VISIBILITY_LEVELS.contains(&v) => Ok(()),
            _ => Err(format!(
                "{key} must be one of public, team or private (got {value})"
            )),
        },
        MNEMO_META_AS_OF | MNEMO_META_REVIEW_BY => match value.as_i64() {
            Some(t) if t >= 0 => Ok(()),
            _ => Err(format!(
                "{key} must be a time in unix seconds, 0 or more (got {value})"
            )),
        },
        _ => Err(format!("{key} isn't a key a caller may set")),
    }
}

/// v0.21.0 (13.7): whether a record is a tombstone.
pub fn is_tombstone(m: &Memory) -> bool {
    m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()) == Some(MNEMO_META_KIND_TOMBSTONE)
}

/// Whether a record is one of the coordination kinds ([`COORDINATION_KINDS`]).
pub fn is_coordination(m: &Memory) -> bool {
    m.metadata
        .get(MNEMO_META_KIND)
        .and_then(Value::as_str)
        .is_some_and(|k| COORDINATION_KINDS.contains(&k))
}

/// v0.16.1: `true` on a turn whose content is only the markers a
/// transcript reader makes for blocks it doesn't keep (`[thinking]`,
/// `[tool_use name=…]`, `[tool_result ok, N bytes]`, `[image]`). Such a
/// turn is kept in the thread, so `thread export` shows the whole flow,
/// but it isn't embedded (it carries the sentinel vector) and `recall`
/// never returns it, since it would match any query weakly. `recall`
/// leaves out a marker-only turn written before v0.16.1 too, by its
/// content.
pub const MNEMO_META_MARKERS_ONLY: &str = "_mnemo.markers_only";

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

/// Task transition table (13a step 4, `Mnemo::task_update`).
/// `queued → active → ready → done`, or `active → blocked`; the back
/// edges are `blocked → active` and (R-010.7, RB-012.3) `active → queued`
/// and `blocked → queued`, for an owner handing a task back or anyone
/// requeueing an abandoned one.
/// `queued → active` happens only through `Mnemo::task_next`.
pub const TASK_TRANSITIONS: &[(&str, &str)] = &[
    ("queued", "active"),
    ("active", "ready"),
    ("active", "blocked"),
    ("blocked", "active"),
    ("active", "queued"),
    ("blocked", "queued"),
    ("ready", "done"),
];

/// 13a (13.1): the task statuses.
pub mod task_status {
    /// Waiting for an agent of its role to take it.
    pub const QUEUED: &str = "queued";
    /// Taken: its owner holds its claim.
    pub const ACTIVE: &str = "active";
    /// Its owner finished; waiting for its creator's `done`.
    pub const READY: &str = "ready";
    /// Done.
    pub const DONE: &str = "done";
    /// Its owner is stuck.
    pub const BLOCKED: &str = "blocked";
}

/// Finding transition table (v0.18.0, `Mnemo::finding_triage` and
/// `Mnemo::finding_verify`): `open → {accepted | deferred | rejected |
/// needs-user} → verified`, the user's way out of `needs-user` (to
/// `accepted`, `deferred` or `rejected`), and a deferral taken up later
/// (`deferred → accepted`).
pub const FINDING_TRANSITIONS: &[(&str, &str)] = &[
    ("open", "accepted"),
    ("open", "deferred"),
    ("open", "rejected"),
    ("open", "needs-user"),
    ("needs-user", "accepted"),
    ("needs-user", "deferred"),
    ("needs-user", "rejected"),
    ("deferred", "accepted"),
    ("accepted", "verified"),
    ("deferred", "verified"),
    ("needs-user", "verified"),
];

/// v0.18.0: the finding statuses.
pub mod finding_status {
    /// Added by a Reviewer; waits for triage.
    pub const OPEN: &str = "open";
    /// To be done.
    pub const ACCEPTED: &str = "accepted";
    /// Later.
    pub const DEFERRED: &str = "deferred";
    /// Not to be done.
    pub const REJECTED: &str = "rejected";
    /// The user decides.
    pub const NEEDS_USER: &str = "needs-user";
    /// Done, and confirmed in the code.
    pub const VERIFIED: &str = "verified";
}

/// v0.18.0: a finding's categories.
pub const FINDING_CATEGORIES: [&str; 6] = [
    "SECURITY",
    "CORRECTNESS",
    "PERF",
    "PLAN-REFINEMENT",
    "PRODUCT",
    "DX/DOCS",
];
/// v0.18.0: a finding's priorities.
pub const FINDING_PRIORITIES: [&str; 4] = ["HIGH", "MEDIUM", "LOW", "IDEA"];
/// v0.18.0: a finding's title, at most this many bytes (one line).
pub const FINDING_TITLE_MAX: usize = 200;
/// v0.18.0: a finding's body, at most this many bytes.
pub const FINDING_BODY_MAX: usize = 16 * 1024;
/// v0.18.0: a finding's id (`RB-014.1`), its locator.
pub const MNEMO_META_FINDING_ID: &str = "_mnemo.finding_id";
/// v0.18.0: a finding's batch, its id's prefix (`RB-014`).
pub const MNEMO_META_BATCH: &str = "_mnemo.batch";
/// v0.18.0: a finding's category ([`FINDING_CATEGORIES`]).
pub const MNEMO_META_CATEGORY: &str = "_mnemo.category";
/// v0.18.0: a finding's priority ([`FINDING_PRIORITIES`]).
pub const MNEMO_META_PRIORITY: &str = "_mnemo.priority";
/// v0.18.0: a finding's triage text, and who wrote it.
pub const MNEMO_META_TRIAGE: &str = "_mnemo.triage";
/// v0.18.0: who triaged a finding last.
pub const MNEMO_META_TRIAGED_BY: &str = "_mnemo.triaged_by";
/// v0.18.0: what verified a finding (a commit, a tag, a PR).
pub const MNEMO_META_VERIFIED_REF: &str = "_mnemo.verified_ref";
/// v0.18.0: who verified a finding.
pub const MNEMO_META_VERIFIED_BY: &str = "_mnemo.verified_by";

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
    /// 13a: set these caller keys ([`CALLER_KEYS`]: visibility, `as_of`,
    /// `review_by`), each checked, without replacing the user metadata. A
    /// `null` value removes the key.
    pub caller_keys: Option<Map<String, Value>>,
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
            .is_some_and(|n| min.is_none_or(|m_| n >= m_) && max.is_none_or(|m_| n <= m_)),
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
