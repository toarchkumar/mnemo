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
