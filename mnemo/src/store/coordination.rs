//! 13a (v0.17): coordination records, `message`, `task`, `claim` and
//! `presence` (`docs/design/13a-coordination.md`). Step 1 is the plumbing:
//! the kinds are stored as ordinary records in the shared scope with the
//! sentinel vector, and kept out of recall, the keyword index, the ANN and
//! `about` (see `is_coordination`). Each kind's API comes in its own step.

use super::*;

impl Mnemo {
    /// Write a coordination record of `kind` (one of
    /// [`crate::memory::COORDINATION_KINDS`]) holding `content` and
    /// `metadata` (whose `_mnemo.*` keys the caller of this internal path
    /// sets): shared scope, the sentinel vector, never embedded. The
    /// handle's author is stamped like any new record's.
    // The test hook's; the APIs use `put_coordination_with_id`.
    #[cfg_attr(not(feature = "dev-hooks"), allow(dead_code))]
    pub(super) fn put_coordination(
        &mut self,
        kind: &str,
        content: String,
        metadata: Map<String, Value>,
    ) -> Result<Ulid> {
        self.put_coordination_with_id(Ulid::new(), kind, content, metadata)
    }

    /// [`Mnemo::put_coordination`] with the record's id chosen first (a
    /// message's id names its conversation).
    pub(super) fn put_coordination_with_id(
        &mut self,
        id: Ulid,
        kind: &str,
        content: String,
        mut metadata: Map<String, Value>,
    ) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if !crate::memory::COORDINATION_KINDS.contains(&kind) {
            return Err(MnemoError::Invalid(format!(
                "{kind:?} isn't a coordination kind"
            )));
        }
        metadata.insert(MNEMO_META_KIND.into(), Value::String(kind.to_string()));
        let mut m = Memory::new(content, MemoryType::Episodic, self.sentinel_vector())
            .with_scope(Scope::Shared);
        m.id = id;
        m.metadata = metadata;
        self.put(m)
    }
}

/// 13a (13.1): one message between agents, as [`Mnemo::inbox`] returns it.
#[derive(Debug, Clone, PartialEq)]
pub struct Message {
    /// The record's id.
    pub id: Ulid,
    /// An agent id, `role:<role>`, or `all`.
    pub to: String,
    /// The sender as it said, unproven until 13b.
    pub from: String,
    /// One line.
    pub subject: String,
    /// The body (may be empty).
    pub body: String,
    /// Record ids, a PR number, a ledger entry title.
    pub refs: Vec<String>,
    /// The conversation it belongs to.
    pub conversation: String,
    /// Its hop count (13.1b's hop cap).
    pub hops: u64,
    /// `unread` or `acked`.
    pub status: String,
    /// `sent`, `delivered`, `seen` or `done`.
    pub delivery: String,
    /// When it was posted, in unix seconds.
    pub created_at: i64,
    /// When it was acked, if it was.
    pub acked_at: Option<i64>,
}

impl Message {
    /// The message a record holds, if it's one.
    pub fn from_memory(m: &Memory) -> Option<Message> {
        if m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str)
            != Some(crate::memory::MNEMO_META_KIND_MESSAGE)
        {
            return None;
        }
        let s = |k: &str| {
            m.metadata
                .get(k)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        let (subject, body) = match m.content.split_once("\n\n") {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (m.content.clone(), String::new()),
        };
        Some(Message {
            id: m.id,
            to: s(MNEMO_META_TO),
            from: s(MNEMO_META_FROM),
            subject,
            body,
            refs: m
                .metadata
                .get(MNEMO_META_REFS)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            conversation: s(MNEMO_META_CONVERSATION),
            hops: m
                .metadata
                .get(MNEMO_META_HOPS)
                .and_then(Value::as_u64)
                .unwrap_or(0),
            status: s(MNEMO_META_STATUS),
            delivery: s(MNEMO_META_DELIVERY),
            created_at: m.created_at,
            acked_at: m.metadata.get(MNEMO_META_ACKED_AT).and_then(Value::as_i64),
        })
    }

    /// Whether it's addressed to `agent` (by id), to `role`, or to everyone,
    /// and isn't from `agent` itself.
    pub fn is_for(&self, agent: Option<&str>, role: Option<&str>) -> bool {
        if agent.is_some_and(|a| a == self.from) {
            return false;
        }
        self.to == "all"
            || agent.is_some_and(|a| a == self.to)
            || role.is_some_and(|r| self.to.strip_prefix("role:") == Some(r))
    }
}

/// Whether `to` is an address: an agent id, `role:<role>`, or `all`.
fn valid_address(to: &str) -> bool {
    to == "all"
        || crate::memory::valid_agent_id(to) && !to.starts_with("role:")
        || to
            .strip_prefix("role:")
            .is_some_and(crate::memory::valid_agent_id)
}

impl Mnemo {
    /// Every live message, oldest first.
    fn messages(&mut self) -> Result<Vec<Memory>> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .filter(|e| e.v9.is_none_or(|r| r.kind == ROW_KIND_COORDINATION))
            .cloned()
            .collect();
        let mut out = Vec::new();
        for e in entries {
            let m = self.read_memory(&e)?;
            if Message::from_memory(&m).is_some() {
                out.push(m);
            }
        }
        // Stable: within one second (`created_at`'s grain) the catalog's
        // order, which is the order they were written.
        out.sort_by_key(|m| m.created_at);
        Ok(out)
    }

    /// 13a (13.1): post a message to `to` (an agent id, `role:<role>`, or
    /// `all`) with a one-line `subject` and an optional `body` (at most
    /// [`crate::memory::MESSAGE_BODY_MAX`] bytes), from `from` (else the
    /// handle's author, else `anonymous`), in `from_role` when given.
    ///
    /// A reply names its `conversation`, and its hop count is one more than
    /// the conversation's highest. A new message between the same two
    /// parties within [`crate::memory::MESSAGE_PAIR_CARRY_SECS`] starts one
    /// above the highest count between them in that time, so agents trading
    /// fresh messages still reach the hop cap. Its delivery is `sent` and its status `unread`.
    // Each is a message's own field; a request struct would only rename them.
    #[allow(clippy::too_many_arguments)]
    pub fn post(
        &mut self,
        to: &str,
        subject: &str,
        body: &str,
        refs: &[String],
        conversation: Option<&str>,
        from: Option<&str>,
        from_role: Option<&str>,
    ) -> Result<Message> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if !valid_address(to) {
            return Err(MnemoError::Invalid(format!(
                "{to:?} isn't an address: an agent id, role:<role>, or all"
            )));
        }
        let subject = subject.trim();
        if subject.is_empty() || subject.contains('\n') {
            return Err(MnemoError::Invalid(
                "a message's subject is one line, and not empty".into(),
            ));
        }
        if subject.len() > crate::memory::MESSAGE_SUBJECT_MAX {
            return Err(MnemoError::Invalid(format!(
                "a message's subject is at most {} bytes",
                crate::memory::MESSAGE_SUBJECT_MAX
            )));
        }
        if body.len() > crate::memory::MESSAGE_BODY_MAX {
            return Err(MnemoError::Invalid(format!(
                "a message's body is at most {} bytes; put a longer one in a fact and send its id \
                 in refs",
                crate::memory::MESSAGE_BODY_MAX
            )));
        }
        let from = from
            .map(str::to_string)
            .or_else(|| self.author.clone())
            .unwrap_or_else(|| "anonymous".to_string());
        let now = memory::now_secs();
        let earlier: Vec<Message> = self
            .messages()?
            .iter()
            .filter_map(Message::from_memory)
            .collect();
        let mut hops = 0u64;
        if let Some(c) = conversation {
            if let Some(h) = earlier
                .iter()
                .filter(|m| m.conversation == c)
                .map(|m| m.hops)
                .max()
            {
                hops = h + 1;
            }
        }
        // The pair carry: one more than the highest count between these two,
        // either way, in the window.
        if let Some(h) = earlier
            .iter()
            .filter(|m| now - m.created_at <= crate::memory::MESSAGE_PAIR_CARRY_SECS)
            .filter(|m| (m.from == from && m.to == to) || (m.from == to && m.to == from))
            .map(|m| m.hops)
            .max()
        {
            hops = hops.max(h + 1);
        }
        let id = Ulid::new();
        let conversation = conversation
            .map(str::to_string)
            .unwrap_or_else(|| id.to_string());
        let mut meta = Map::new();
        meta.insert(MNEMO_META_TO.into(), Value::String(to.into()));
        meta.insert(MNEMO_META_FROM.into(), Value::String(from));
        if let Some(r) = from_role {
            meta.insert("_mnemo.from_role".into(), Value::String(r.to_string()));
        }
        meta.insert(MNEMO_META_CONVERSATION.into(), Value::String(conversation));
        meta.insert(MNEMO_META_HOPS.into(), Value::from(hops));
        meta.insert(
            MNEMO_META_REFS.into(),
            Value::Array(refs.iter().map(|r| Value::String(r.clone())).collect()),
        );
        meta.insert(
            MNEMO_META_STATUS.into(),
            Value::String(crate::memory::MESSAGE_UNREAD.into()),
        );
        meta.insert(
            MNEMO_META_DELIVERY.into(),
            Value::String(crate::memory::delivery::SENT.into()),
        );
        let content = if body.is_empty() {
            subject.to_string()
        } else {
            format!("{subject}\n\n{body}")
        };
        let id = self.put_coordination_with_id(
            id,
            crate::memory::MNEMO_META_KIND_MESSAGE,
            content,
            meta,
        )?;
        let m = self.get(&id)?;
        Ok(Message::from_memory(&m).expect("just written"))
    }

    /// 13a (13.1): the messages for `agent` (by id), its `role`, or
    /// everyone, oldest first, leaving out its own. With `unread_only`,
    /// only those not yet acked. Reads only; [`Mnemo::mark_seen`] records
    /// that they were shown.
    pub fn inbox(
        &mut self,
        agent: Option<&str>,
        role: Option<&str>,
        unread_only: bool,
    ) -> Result<Vec<Message>> {
        Ok(self
            .messages()?
            .iter()
            .filter_map(Message::from_memory)
            .filter(|m| m.is_for(agent, role))
            .filter(|m| !unread_only || m.status == crate::memory::MESSAGE_UNREAD)
            .collect())
    }

    /// 13a (13.1b): mark these messages `seen` (from `sent` or `delivered`),
    /// in place. Returns how many changed.
    pub fn mark_seen(&mut self, ids: &[Ulid]) -> Result<usize> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let mut changed = 0;
        for id in ids {
            let mut m = self.get(id)?;
            let Some(msg) = Message::from_memory(&m) else {
                continue;
            };
            if msg.delivery == crate::memory::delivery::SENT
                || msg.delivery == crate::memory::delivery::DELIVERED
            {
                m.metadata.insert(
                    MNEMO_META_DELIVERY.into(),
                    Value::String(crate::memory::delivery::SEEN.into()),
                );
                self.put(m)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    /// 13a (13.1): ack a message, in place: `unread → acked`, its delivery
    /// `done`, and a `status_history` entry with `by` (else the handle's
    /// author) and the optional `note`. `Unchanged` when it's already
    /// acked; an id that isn't a message is refused.
    pub fn ack(&mut self, id: &Ulid, note: Option<&str>, by: Option<&str>) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let mut m = self.get(id)?;
        let Some(msg) = Message::from_memory(&m) else {
            return Err(MnemoError::Invalid(format!("{id} isn't a message")));
        };
        if msg.status == crate::memory::MESSAGE_ACKED {
            return Ok(WriteOutcome::Unchanged);
        }
        let now = memory::now_secs();
        let by = by
            .map(str::to_string)
            .or_else(|| self.author.clone())
            .unwrap_or_else(|| "anonymous".to_string());
        let mut entry = Map::new();
        entry.insert("from".into(), Value::String(msg.status.clone()));
        entry.insert(
            "to".into(),
            Value::String(crate::memory::MESSAGE_ACKED.into()),
        );
        entry.insert("at".into(), Value::from(now));
        entry.insert("by".into(), Value::String(by));
        if let Some(n) = note {
            entry.insert("note".into(), Value::String(n.to_string()));
        }
        let mut history = m
            .metadata
            .get(MNEMO_META_STATUS_HISTORY)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        history.push(Value::Object(entry));
        m.metadata
            .insert(MNEMO_META_STATUS_HISTORY.into(), Value::Array(history));
        m.metadata.insert(
            MNEMO_META_STATUS.into(),
            Value::String(crate::memory::MESSAGE_ACKED.into()),
        );
        m.metadata.insert(
            MNEMO_META_DELIVERY.into(),
            Value::String(crate::memory::delivery::DONE.into()),
        );
        m.metadata
            .insert(MNEMO_META_ACKED_AT.into(), Value::from(now));
        self.put(m)?;
        Ok(WriteOutcome::Updated)
    }

    /// 13a (R-010.8): delete messages acked more than `older_than_secs` ago
    /// ([`crate::memory::MESSAGE_ACKED_KEEP_SECS`] by default in the CLI).
    /// Returns how many went. `compact` drops the same ones.
    pub fn prune_messages(&mut self, older_than_secs: i64) -> Result<usize> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = memory::now_secs();
        let old: Vec<Ulid> = self
            .messages()?
            .iter()
            .filter_map(Message::from_memory)
            .filter(|m| prunable(m, now, older_than_secs))
            .map(|m| m.id)
            .collect();
        for id in &old {
            self.delete(id)?;
        }
        Ok(old.len())
    }
}

/// Whether a message is acked and older than `keep` seconds.
fn prunable(m: &Message, now: i64, keep: i64) -> bool {
    m.status == crate::memory::MESSAGE_ACKED && m.acked_at.is_some_and(|t| now - t > keep)
}

/// 13a (R-010.8): whether `compact` leaves this record out: a message acked
/// more than [`crate::memory::MESSAGE_ACKED_KEEP_SECS`] ago.
pub(super) fn compact_drops(m: &Memory, now: i64) -> bool {
    Message::from_memory(m)
        .is_some_and(|msg| prunable(&msg, now, crate::memory::MESSAGE_ACKED_KEEP_SECS))
}
