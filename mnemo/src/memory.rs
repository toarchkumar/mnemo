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
        let now = now_secs();
        Memory {
            id: Ulid::new(),
            memory_type: MemoryType::Semantic,
            vector: vec![0.0; dimensions],
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

fn cosine(a: &[f32], b: &[f32]) -> f32 {
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

/// Weights for the multi-signal recall score.
///
/// `score = α·similarity + β·recency + γ·importance + δ·ln(access_count + 1)`
#[derive(Clone, Copy, Debug)]
pub struct ScoreWeights {
    /// Weight on semantic similarity.
    pub alpha: f32,
    /// Weight on recency decay.
    pub beta: f32,
    /// Weight on the importance score.
    pub gamma: f32,
    /// Weight on access frequency.
    pub delta: f32,
    /// Half-life of the recency term, in seconds.
    pub half_life_secs: f32,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        // Tuned for conversational agents: similarity dominates, with a
        // meaningful recency nudge. Half-life of 7 days.
        Self {
            alpha: 1.0,
            beta: 0.3,
            gamma: 0.2,
            delta: 0.1,
            half_life_secs: 7.0 * 24.0 * 3600.0,
        }
    }
}

impl ScoreWeights {
    /// Combine the four signals into a single score.
    pub fn score(&self, similarity: f32, age_secs: f32, importance: f32, access_count: u32) -> f32 {
        let recency = (-std::f32::consts::LN_2 * age_secs.max(0.0) / self.half_life_secs).exp();
        let frequency = ((access_count as f32) + 1.0).ln();
        self.alpha * similarity
            + self.beta * recency
            + self.gamma * importance
            + self.delta * frequency
    }
}
