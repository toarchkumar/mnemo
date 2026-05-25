//! Session lifecycle wrapper (Phase 5.5 of the build plan).
//!
//! A [`Session`] is a thin, scoped handle over a [`Mnemo`] database for the
//! duration of one conversation. It bundles an agent id with a freshly
//! generated session id, records conversation turns as `Working` memories
//! tagged with that session, and — on [`Session::close`] — **consolidates**
//! those turns into durable `Episodic` memory, exactly the working-memory →
//! episodic promotion the build plan calls for.
//!
//! The session borrows the database mutably for its lifetime, so the
//! single-writer discipline is enforced by the compiler: while a session is
//! open, the database is reached only through it.
//!
//! ```no_run
//! # use mnemo::{Mnemo, MnemoConfig, Role, Turn, Result};
//! # fn main() -> Result<()> {
//! let mut db = Mnemo::open("agent.mnemo", "passphrase")?;
//! let mut session = db.session("assistant-1");
//!
//! session.add_turn(Turn::user("what's the weather?", vec![0.1, 0.2, 0.3]))?;
//! session.add_turn(Turn::assistant("clear and mild", vec![0.2, 0.1, 0.4]))?;
//!
//! // End the session — its turns are promoted to episodic memory.
//! let promoted = session.close()?;
//! assert_eq!(promoted, 2);
//! # Ok(())
//! # }
//! ```

use serde_json::Value;
use ulid::Ulid;

use crate::error::Result;
use crate::memory::{Memory, MemoryType};
use crate::store::{Mnemo, RecallRequest, RecallResult};

/// Who produced a conversation turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The end user.
    User,
    /// The agent / assistant.
    Assistant,
    /// A system or tool message.
    System,
}

impl Role {
    /// Lowercase string form — how the role is stored in turn metadata.
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        }
    }
}

/// One conversation turn, written to working memory by [`Session::add_turn`].
///
/// The `vector` is the caller-supplied embedding of `content`; Mnemo does not
/// embed text itself.
#[derive(Clone, Debug)]
pub struct Turn {
    /// Who produced the turn.
    pub role: Role,
    /// The turn's text.
    pub content: String,
    /// Embedding of `content`.
    pub vector: Vec<f32>,
}

impl Turn {
    /// A turn with an explicit role.
    pub fn new(role: Role, content: impl Into<String>, vector: Vec<f32>) -> Self {
        Self { role, content: content.into(), vector }
    }
    /// Shorthand for a [`Role::User`] turn.
    pub fn user(content: impl Into<String>, vector: Vec<f32>) -> Self {
        Self::new(Role::User, content, vector)
    }
    /// Shorthand for a [`Role::Assistant`] turn.
    pub fn assistant(content: impl Into<String>, vector: Vec<f32>) -> Self {
        Self::new(Role::Assistant, content, vector)
    }
    /// Shorthand for a [`Role::System`] turn.
    pub fn system(content: impl Into<String>, vector: Vec<f32>) -> Self {
        Self::new(Role::System, content, vector)
    }
}

/// A scoped conversation session over a [`Mnemo`] database.
///
/// Created by [`Mnemo::session`]. Holds the database mutably until it is
/// closed (or dropped), so all writes for the conversation flow through it.
pub struct Session<'db> {
    db: &'db mut Mnemo,
    agent_id: String,
    session_id: String,
    turns: Vec<Ulid>,
}

impl<'db> Session<'db> {
    /// Begin a session for `agent_id` with a fresh, time-sortable session id.
    pub(crate) fn new(db: &'db mut Mnemo, agent_id: String) -> Self {
        Self {
            db,
            agent_id,
            session_id: Ulid::new().to_string(),
            turns: Vec::new(),
        }
    }

    /// This session's unique id (a ULID string), stamped on every turn.
    pub fn id(&self) -> &str {
        &self.session_id
    }

    /// The agent this session belongs to.
    pub fn agent(&self) -> &str {
        &self.agent_id
    }

    /// Ids of the turns recorded so far, in order.
    pub fn turn_ids(&self) -> &[Ulid] {
        &self.turns
    }

    /// How many turns have been recorded.
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }

    /// Record a conversation turn as a `Working` memory tagged with this
    /// session and agent. The turn's role is kept in the memory's metadata.
    /// Writes are staged; they reach disk on [`Session::close`] (or any later
    /// flush of the database).
    pub fn add_turn(&mut self, turn: Turn) -> Result<Ulid> {
        let memory = Memory::new(turn.content, MemoryType::Working, turn.vector)
            .with_agent(&self.agent_id)
            .with_session(&self.session_id)
            .with_meta("role", Value::String(turn.role.as_str().to_string()));
        let id = self.db.remember(memory)?;
        self.turns.push(id);
        Ok(id)
    }

    /// Retrieve memories for context injection, scoped to this session's
    /// agent. The agent filter on `request` is overridden — a session always
    /// recalls within its own agent's view (its private memories plus shared
    /// ones). All other request fields (type filter, weights, top-k) apply.
    pub fn recall(&mut self, mut request: RecallRequest) -> Result<Vec<RecallResult>> {
        request.agent_id = Some(self.agent_id.clone());
        self.db.recall(&request)
    }

    /// End the session, **consolidating** its working turns into episodic
    /// memory: each turn still typed `Working` is promoted to `Episodic`, the
    /// store of "what happened". Returns the number of turns promoted; the
    /// database is flushed before returning.
    pub fn close(self) -> Result<usize> {
        let Session { db, turns, .. } = self;
        let mut promoted = 0;
        for id in &turns {
            // A turn could have been deleted elsewhere — skip if it is gone.
            let mut memory = match db.get(id) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if memory.memory_type == MemoryType::Working {
                memory.memory_type = MemoryType::Episodic;
                db.remember(memory)?;
                promoted += 1;
            }
        }
        db.flush()?;
        Ok(promoted)
    }

    /// End the session, **discarding** its working turns instead of
    /// consolidating them — every turn this session recorded is deleted.
    /// Returns the number removed; the database is flushed before returning.
    pub fn discard(self) -> Result<usize> {
        let Session { db, turns, .. } = self;
        let mut removed = 0;
        for id in &turns {
            if db.delete(id).is_ok() {
                removed += 1;
            }
        }
        db.flush()?;
        Ok(removed)
    }
}
