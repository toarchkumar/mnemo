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
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
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

/// 13a (RB-012.6 item 4): the agents a broadcast message or a role's task
/// has been shown to (`_mnemo.seen_by`).
fn seen_by(m: &Memory) -> Vec<String> {
    m.metadata
        .get(crate::memory::MNEMO_META_SEEN_BY)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// COORD-LEGACY-SCAN: whether a stored (fp16) vector is the sentinel every
/// coordination record carries: all its values equal, at 1/sqrt(dims).
fn is_sentinel(h: &[u16]) -> bool {
    let Some(&first) = h.first() else {
        return false;
    };
    let want = 1.0 / (h.len() as f32).sqrt();
    h.iter().all(|&x| x == first) && (crate::veccol::f16_to_f32(first) - want).abs() < 1e-3
}

/// READY-NOTICE: the agents a `ready` task was shown to as theirs to
/// close (`_mnemo.ready_seen_by`).
fn ready_seen_by(m: &Memory) -> Vec<String> {
    m.metadata
        .get(crate::memory::MNEMO_META_READY_SEEN_BY)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Add `agent` to a record's `_mnemo.seen_by`; `false` when it was there.
fn add_seen_by(m: &mut Memory, agent: &str) -> bool {
    let mut v = seen_by(m);
    if v.iter().any(|a| a == agent) {
        return false;
    }
    v.push(agent.to_string());
    m.metadata.insert(
        crate::memory::MNEMO_META_SEEN_BY.into(),
        Value::Array(v.into_iter().map(Value::String).collect()),
    );
    true
}

impl Mnemo {
    /// Every live coordination record of `kind`, in catalog order; with a
    /// `locator` (a claim's topic, a presence record's agent), only that
    /// one's, found through its row's hash without decrypting the rest.
    pub(super) fn coordination_records(
        &mut self,
        kind: &str,
        locator: Option<&str>,
    ) -> Result<Vec<Memory>> {
        let want = locator.map(|l| locator_hash_of(kind, l));
        let legacy = self.legacy_coordination_ids()?;
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .filter(|e| {
                legacy.contains(&e.id)
                    || e.v9.is_none_or(|r| {
                        r.kind == ROW_KIND_COORDINATION && want.is_none_or(|h| r.thread_hash == h)
                    })
            })
            .cloned()
            .collect();
        let mut out = Vec::new();
        for e in entries {
            let m = self.read_memory(&e)?;
            if m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) != Some(kind) {
                continue;
            }
            if let Some(l) = locator {
                if locator_of(&m) != Some(l) {
                    continue;
                }
            }
            out.push(m);
        }
        Ok(out)
    }

    /// RB-013.1: the coordination records an older build re-tagged as facts
    /// (its `compact` writes every row again, and it computes "fact" for a
    /// kind it doesn't know). Found once per handle: such a record keeps
    /// its `episodic` type in the row and its sentinel vector in the column,
    /// so only `episodic` fact rows whose vector is the sentinel are
    /// decrypted (COORD-LEGACY-SCAN, v0.17.2); a row whose vector can't be
    /// read is decrypted, as before. A write of one re-tags its row.
    fn legacy_coordination_ids(&mut self) -> Result<std::collections::HashSet<u128>> {
        if let Some(ids) = &self.legacy_coordination {
            return Ok(ids.clone());
        }
        let episodic = super::records::memory_type_code(MemoryType::Episodic);
        let candidates: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .filter(|e| {
                e.v9.is_some_and(|r| {
                    r.kind == super::records::ROW_KIND_FACT && r.memory_type == episodic
                })
            })
            .cloned()
            .collect();
        let mut ids = std::collections::HashSet::new();
        // Each vector page read once, for every candidate row on it.
        let mut pages: std::collections::HashMap<(u64, u64), Option<Vec<u8>>> =
            std::collections::HashMap::new();
        for e in candidates {
            let vector = match e.v9 {
                Some(r) if r.vec_page != 0 && !self.has_hot_vector(&e) => {
                    let page = pages
                        .entry((r.vec_page, r.vec_gen))
                        .or_insert_with(|| self.pager.read_page(r.vec_page, r.vec_gen).ok());
                    page.as_ref().and_then(|p| {
                        crate::veccol::decode_entry(p, self.dimensions, r.vec_slot as usize).ok()
                    })
                }
                _ => self.column_entry(&e).ok(),
            };
            if matches!(vector, Some((h, _)) if !is_sentinel(&h)) {
                continue;
            }
            if is_coordination(&self.read_memory(&e)?) {
                ids.insert(e.id);
            }
        }
        self.legacy_coordination = Some(ids.clone());
        Ok(ids)
    }

    /// RB-013.1: how many coordination records are tagged as facts in their
    /// rows (an older mnemo rewrote this brain); `mnemo compact` re-tags
    /// them. They're found and used all the same.
    pub fn mistagged_coordination(&mut self) -> Result<usize> {
        let ids = self.legacy_coordination_ids()?;
        Ok(self
            .catalog
            .iter()
            .filter(|e| !e.deleted && ids.contains(&e.id))
            .filter(|e| {
                e.v9.is_some_and(|r| r.kind == super::records::ROW_KIND_FACT)
            })
            .count())
    }

    /// Every live message, oldest first.
    fn messages(&mut self) -> Result<Vec<Memory>> {
        let mut out = self.coordination_records(crate::memory::MNEMO_META_KIND_MESSAGE, None)?;
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
        self.inbox_in(
            agent,
            role,
            unread_only,
            crate::memory::VisibilitySet::DEFAULT,
        )
    }

    /// 13a (13.5): [`Mnemo::inbox`] with the visibility levels it shows
    /// (`inbox` itself shows public and team).
    pub fn inbox_in(
        &mut self,
        agent: Option<&str>,
        role: Option<&str>,
        unread_only: bool,
        visibility: crate::memory::VisibilitySet,
    ) -> Result<Vec<Message>> {
        Ok(self
            .messages()?
            .iter()
            .filter(|m| visibility.shows(m))
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
            let mut dirty = false;
            if msg.delivery == crate::memory::delivery::SENT
                || msg.delivery == crate::memory::delivery::DELIVERED
            {
                m.metadata.insert(
                    MNEMO_META_DELIVERY.into(),
                    Value::String(crate::memory::delivery::SEEN.into()),
                );
                dirty = true;
            }
            // RB-012.6 item 4: a message to a role or everyone is seen per
            // agent (this handle's author).
            if msg.to == "all" || msg.to.starts_with("role:") {
                if let Some(a) = self.author.clone() {
                    dirty |= add_seen_by(&mut m, &a);
                }
            }
            if dirty {
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

// --- 13a step 3: claims and presence (13.3) ---------------------------------

use crate::memory::{
    CLAIM_LAPSE_GRACE_SECS, CLAIM_TOPIC_MAX, CLAIM_TTL_DEFAULT, CLAIM_TTL_MAX, CLAIM_TTL_MIN,
    MNEMO_META_AGENT, MNEMO_META_CLAIM_TOKEN, MNEMO_META_DEAD_SEEN_AT, MNEMO_META_EXPIRES_AT,
    MNEMO_META_HOLDER, MNEMO_META_HOLDER_LOCK, MNEMO_META_KIND_CLAIM, MNEMO_META_KIND_PRESENCE,
    MNEMO_META_LAST_ACTIVE, MNEMO_META_LEASE, MNEMO_META_PRESENCE_LOCK, MNEMO_META_TOOL,
    MNEMO_META_TOPIC, PRESENCE_EVERY_SECS,
};

/// A claim's or presence record's locator: its topic or its agent.
fn locator_of(m: &Memory) -> Option<&str> {
    let key = match m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str)? {
        MNEMO_META_KIND_CLAIM => MNEMO_META_TOPIC,
        MNEMO_META_KIND_PRESENCE => MNEMO_META_AGENT,
        crate::memory::MNEMO_META_KIND_TASK => crate::memory::MNEMO_META_SLUG,
        crate::memory::MNEMO_META_KIND_FINDING => crate::memory::MNEMO_META_FINDING_ID,
        _ => return None,
    };
    m.metadata.get(key).and_then(Value::as_str)
}

fn locator_hash_of(kind: &str, locator: &str) -> u32 {
    super::records::fnv1a32(&[b"_mnemo.coordination", kind.as_bytes(), locator.as_bytes()])
}

/// The hash a coordination record's v9 row carries in place of a thread
/// hash: of its kind and topic (a claim) or agent (presence); `0` for the
/// other kinds.
pub(super) fn locator_hash(m: &Memory) -> u32 {
    match (
        m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str),
        locator_of(m),
    ) {
        (Some(kind), Some(l)) => locator_hash_of(kind, l),
        _ => 0,
    }
}

/// 13a (13.3): what [`Mnemo::claim`] got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// The claim is this agent's. Keep the token: `renew`, `release` and
    /// `check_claim` need it.
    Claimed {
        /// The claim's token.
        token: String,
        /// When it lapses unless renewed, in unix seconds.
        expires_at: i64,
    },
    /// Another agent (or this one, in another process or session) holds it.
    HeldBy {
        /// The holder.
        agent: String,
        /// When it lapses unless renewed.
        expires_at: i64,
    },
}

/// 13a (13.3): what `renew`, `release` and `check_claim` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimStatus {
    /// The claim is still the token's, until `expires_at`.
    Held {
        /// When it lapses unless renewed, in unix seconds.
        expires_at: i64,
    },
    /// Released.
    Released,
    /// The claim lapsed or another agent holds it now: whatever it guarded
    /// may have been done by someone else. Check before an irreversible
    /// step.
    Lost,
}

/// 13a (13.3): a live claim, as [`Mnemo::claims`] lists it (no token).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Claim {
    /// What's claimed.
    pub topic: String,
    /// Who holds it.
    pub holder: String,
    /// When it lapses unless renewed.
    pub expires_at: i64,
    /// Its lease, in seconds.
    pub lease: i64,
}

/// 13a (13.3): whether an agent's process is running, by its presence lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Alive {
    /// Its long-lived process holds its presence lock.
    Yes,
    /// It registered a lock, and the lock is free: its process is gone.
    No,
    /// It has no long-lived process (the CLI, a session without the MCP
    /// server), or the probe couldn't tell.
    Unknown,
}

/// 13a (13.3): one agent, as [`Mnemo::who`] lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Presence {
    /// The agent id.
    pub agent: String,
    /// Its role, if it gave one.
    pub role: Option<String>,
    /// The tool it last wrote through (`mcp`, `cli`, ...).
    pub tool: Option<String>,
    /// When it last wrote, in unix seconds (updated at most once a minute).
    pub last_active: i64,
    /// Whether its process is running.
    pub alive: Alive,
    /// The topics it holds live claims on.
    pub claims: Vec<String>,
}

fn meta_i64(m: &Memory, k: &str) -> Option<i64> {
    m.metadata.get(k).and_then(Value::as_i64)
}

fn meta_string(m: &Memory, k: &str) -> Option<String> {
    m.metadata
        .get(k)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A claim record's expiry, bounded by one lease from `now`: a clock that
/// stepped back can't stretch a claim past its lease.
fn effective_expiry(m: &Memory, now: i64) -> Option<i64> {
    let token = m
        .metadata
        .get(MNEMO_META_CLAIM_TOKEN)
        .and_then(Value::as_str)?;
    if token.is_empty() {
        return None;
    }
    let expires = meta_i64(m, MNEMO_META_EXPIRES_AT)?;
    let lease = meta_i64(m, MNEMO_META_LEASE).unwrap_or(CLAIM_TTL_DEFAULT);
    Some(expires.min(now.saturating_add(lease)))
}

/// Whether a claim record is held at `now`.
fn is_live(m: &Memory, now: i64) -> bool {
    effective_expiry(m, now).is_some_and(|e| now < e)
}

fn new_token() -> String {
    format!(
        "{:016x}{:016x}",
        crate::crypto::random_u64(),
        crate::crypto::random_u64()
    )
}

fn check_topic(topic: &str) -> Result<()> {
    if topic.trim().is_empty() || topic.contains('\n') || topic.len() > CLAIM_TOPIC_MAX {
        return Err(MnemoError::Invalid(format!(
            "a claim's topic is one line, not empty, at most {CLAIM_TOPIC_MAX} bytes"
        )));
    }
    Ok(())
}

fn check_ttl(ttl: Option<i64>) -> Result<i64> {
    let t = ttl.unwrap_or(CLAIM_TTL_DEFAULT);
    if !(CLAIM_TTL_MIN..=CLAIM_TTL_MAX).contains(&t) {
        return Err(MnemoError::Invalid(format!(
            "a claim's ttl is {CLAIM_TTL_MIN} s to {CLAIM_TTL_MAX} s (24 h); got {t}"
        )));
    }
    Ok(t)
}

impl Mnemo {
    /// 13a: the clock claims, leases and presence run on, in unix seconds:
    /// the wall clock (dev-hooks builds let tests shift it).
    pub fn coordination_now(&self) -> i64 {
        #[cfg(feature = "dev-hooks")]
        let env: i64 = std::env::var("MNEMO_DEV_CLOCK_OFFSET")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        #[cfg(not(feature = "dev-hooks"))]
        let env: i64 = 0;
        memory::now_secs() + self.clock_offset + env
    }

    /// **Not a stable API** (dev-hooks builds only): shift this handle's
    /// coordination clock by `secs`.
    #[cfg(feature = "dev-hooks")]
    #[doc(hidden)]
    pub fn __set_clock_offset_for_testing(&mut self, secs: i64) {
        self.clock_offset = secs;
    }

    fn claim_agent(&self) -> Result<String> {
        self.author.clone().ok_or_else(|| {
            MnemoError::Invalid(
                "a claim needs an agent: set MNEMO_AGENT (or --as), or call set_author".into(),
            )
        })
    }

    fn find_claim(&mut self, topic: &str) -> Result<Option<Memory>> {
        Ok(self
            .coordination_records(MNEMO_META_KIND_CLAIM, Some(topic))?
            .into_iter()
            .next())
    }

    /// 13a (13.3): claim `topic` for this handle's author for `ttl` seconds
    /// ([`crate::memory::CLAIM_TTL_DEFAULT`], 15 minutes, by default; 30 s
    /// to 24 h).
    ///
    /// A claim is a record, not a lock, and it's advisory: nothing refuses
    /// a write because of one. In a shared handle's write window it's a
    /// compare-and-set: two processes can't both win. The topic is free
    /// when nobody holds it, its claim lapsed at `expires_at`, or its
    /// holder registered a presence lock when it claimed and has been seen
    /// not alive twice, [`crate::memory::CLAIM_LAPSE_GRACE_SECS`] apart
    /// (each look is recorded on the claim). A topic this agent already
    /// holds is [`ClaimOutcome::HeldBy`] too: a token can't be recovered.
    /// One record per topic, changed in place: a new holder gets a new
    /// token.
    pub fn claim(&mut self, topic: &str, ttl: Option<i64>) -> Result<ClaimOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        check_topic(topic)?;
        let ttl = check_ttl(ttl)?;
        let agent = self.claim_agent()?;
        let now = self.coordination_now();
        let existing = self.find_claim(topic)?;
        if let Some(mut m) = existing.clone() {
            if is_live(&m, now) {
                let holder = meta_string(&m, MNEMO_META_HOLDER).unwrap_or_default();
                let expires = effective_expiry(&m, now).unwrap_or(now);
                let mut changed = false;
                let mut lapse = false;
                let had_lock = m
                    .metadata
                    .get(MNEMO_META_HOLDER_LOCK)
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                if had_lock && holder != agent {
                    match crate::shared::probe_presence_once(&self.path, &holder) {
                        Some(false) => match meta_i64(&m, MNEMO_META_DEAD_SEEN_AT) {
                            Some(t) if now - t >= CLAIM_LAPSE_GRACE_SECS => lapse = true,
                            Some(_) => {}
                            None => {
                                m.metadata
                                    .insert(MNEMO_META_DEAD_SEEN_AT.into(), Value::from(now));
                                changed = true;
                            }
                        },
                        _ => {
                            changed |= m.metadata.remove(MNEMO_META_DEAD_SEEN_AT).is_some();
                        }
                    }
                }
                if !lapse {
                    // A clock that stepped back: bring the stored expiry
                    // within one lease.
                    if meta_i64(&m, MNEMO_META_EXPIRES_AT) != Some(expires) {
                        m.metadata
                            .insert(MNEMO_META_EXPIRES_AT.into(), Value::from(expires));
                        changed = true;
                    }
                    if changed {
                        self.put(m)?;
                    }
                    return Ok(ClaimOutcome::HeldBy {
                        agent: holder,
                        expires_at: expires,
                    });
                }
            }
        }
        let token = new_token();
        let expires_at = now + ttl;
        let mut meta = existing
            .as_ref()
            .map(|m| m.metadata.clone())
            .unwrap_or_default();
        meta.remove(MNEMO_META_DEAD_SEEN_AT);
        meta.insert(MNEMO_META_TOPIC.into(), Value::String(topic.to_string()));
        meta.insert(MNEMO_META_HOLDER.into(), Value::String(agent));
        meta.insert(MNEMO_META_CLAIM_TOKEN.into(), Value::String(token.clone()));
        meta.insert(MNEMO_META_EXPIRES_AT.into(), Value::from(expires_at));
        meta.insert(MNEMO_META_LEASE.into(), Value::from(ttl));
        meta.insert(
            MNEMO_META_HOLDER_LOCK.into(),
            Value::Bool(self.presence_held),
        );
        match existing {
            Some(mut m) => {
                m.metadata = meta;
                self.put(m)?;
            }
            None => {
                self.put_coordination_with_id(
                    Ulid::new(),
                    MNEMO_META_KIND_CLAIM,
                    format!("claim {topic}"),
                    meta,
                )?;
            }
        }
        Ok(ClaimOutcome::Claimed { token, expires_at })
    }

    /// 13a (13.3): renew a claim for `ttl` seconds from now (its lease by
    /// default). [`ClaimStatus::Lost`] when it lapsed or isn't the token's.
    pub fn renew(&mut self, topic: &str, token: &str, ttl: Option<i64>) -> Result<ClaimStatus> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = self.coordination_now();
        let Some(mut m) = self.find_claim(topic)? else {
            return Ok(ClaimStatus::Lost);
        };
        if !is_live(&m, now) || meta_string(&m, MNEMO_META_CLAIM_TOKEN).as_deref() != Some(token) {
            return Ok(ClaimStatus::Lost);
        }
        let lease = match ttl {
            Some(t) => check_ttl(Some(t))?,
            None => meta_i64(&m, MNEMO_META_LEASE).unwrap_or(CLAIM_TTL_DEFAULT),
        };
        let expires_at = now + lease;
        m.metadata
            .insert(MNEMO_META_EXPIRES_AT.into(), Value::from(expires_at));
        m.metadata
            .insert(MNEMO_META_LEASE.into(), Value::from(lease));
        m.metadata.remove(MNEMO_META_DEAD_SEEN_AT);
        self.put(m)?;
        Ok(ClaimStatus::Held { expires_at })
    }

    /// 13a (13.3): release a claim. [`ClaimStatus::Lost`] when it already
    /// lapsed or isn't the token's (then nothing changes).
    pub fn release(&mut self, topic: &str, token: &str) -> Result<ClaimStatus> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = self.coordination_now();
        let Some(mut m) = self.find_claim(topic)? else {
            return Ok(ClaimStatus::Lost);
        };
        if !is_live(&m, now) || meta_string(&m, MNEMO_META_CLAIM_TOKEN).as_deref() != Some(token) {
            return Ok(ClaimStatus::Lost);
        }
        m.metadata.remove(MNEMO_META_CLAIM_TOKEN);
        m.metadata.remove(MNEMO_META_DEAD_SEEN_AT);
        m.metadata
            .insert(MNEMO_META_EXPIRES_AT.into(), Value::from(now));
        self.put(m)?;
        Ok(ClaimStatus::Released)
    }

    /// 13a (13.3): whether `token` still holds `topic`. Call it before an
    /// irreversible step. Reads only.
    pub fn check_claim(&mut self, topic: &str, token: &str) -> Result<ClaimStatus> {
        let now = self.coordination_now();
        let Some(m) = self.find_claim(topic)? else {
            return Ok(ClaimStatus::Lost);
        };
        if meta_string(&m, MNEMO_META_CLAIM_TOKEN).as_deref() != Some(token) {
            return Ok(ClaimStatus::Lost);
        }
        match effective_expiry(&m, now) {
            Some(e) if now < e => Ok(ClaimStatus::Held { expires_at: e }),
            _ => Ok(ClaimStatus::Lost),
        }
    }

    /// 13a (13.3): the live claims, by topic. Reads only.
    pub fn claims(&mut self) -> Result<Vec<Claim>> {
        let now = self.coordination_now();
        let mut out: Vec<Claim> = self
            .coordination_records(MNEMO_META_KIND_CLAIM, None)?
            .iter()
            .filter(|m| is_live(m, now))
            .map(|m| Claim {
                topic: meta_string(m, MNEMO_META_TOPIC).unwrap_or_default(),
                holder: meta_string(m, MNEMO_META_HOLDER).unwrap_or_default(),
                expires_at: effective_expiry(m, now).unwrap_or(now),
                lease: meta_i64(m, MNEMO_META_LEASE).unwrap_or(CLAIM_TTL_DEFAULT),
            })
            .collect();
        out.sort_by(|a, b| a.topic.cmp(&b.topic));
        Ok(out)
    }

    /// 13a (13.3): record that this handle's author is active, with its
    /// `role` and `tool`: its presence record's `last_active`, changed in
    /// place, at most once a minute (sooner when the role, the tool or the
    /// presence lock changed). Call it before a write the agent makes
    /// anyway, so it rides that commit. `false` when nothing was written
    /// (too soon, or no author).
    pub fn touch_presence(&mut self, role: Option<&str>, tool: Option<&str>) -> Result<bool> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let Some(agent) = self.author.clone() else {
            return Ok(false);
        };
        let now = self.coordination_now();
        let existing = self
            .coordination_records(MNEMO_META_KIND_PRESENCE, Some(&agent))?
            .into_iter()
            .next();
        let role_v = role.map(|r| Value::String(r.to_string()));
        let tool_v = tool.map(|t| Value::String(t.to_string()));
        // RB-012.4: only a process holding the presence lock writes the
        // lock flag and the tool; another (the CLI, beside a server that
        // holds it) moves `last_active` alone.
        let defer = !self.presence_held
            && existing.as_ref().is_some_and(|m| {
                m.metadata
                    .get(MNEMO_META_PRESENCE_LOCK)
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        if let Some(m) = &existing {
            let fresh = meta_i64(m, MNEMO_META_LAST_ACTIVE)
                .is_some_and(|t| now >= t && now - t < PRESENCE_EVERY_SECS);
            let same = defer
                || (m.metadata.get(crate::memory::MNEMO_META_ROLE) == role_v.as_ref()
                    && m.metadata.get(MNEMO_META_TOOL) == tool_v.as_ref()
                    && m.metadata
                        .get(MNEMO_META_PRESENCE_LOCK)
                        .and_then(Value::as_bool)
                        == Some(self.presence_held));
            if fresh && same {
                return Ok(false);
            }
        }
        if defer {
            let mut m = existing.expect("checked");
            m.metadata
                .insert(MNEMO_META_LAST_ACTIVE.into(), Value::from(now));
            self.put(m)?;
            return Ok(true);
        }
        let mut meta = existing
            .as_ref()
            .map(|m| m.metadata.clone())
            .unwrap_or_default();
        meta.insert(MNEMO_META_AGENT.into(), Value::String(agent.clone()));
        for (k, v) in [
            (crate::memory::MNEMO_META_ROLE, role_v),
            (MNEMO_META_TOOL, tool_v),
        ] {
            match v {
                Some(v) => {
                    meta.insert(k.into(), v);
                }
                None => {
                    meta.remove(k);
                }
            }
        }
        meta.insert(MNEMO_META_LAST_ACTIVE.into(), Value::from(now));
        meta.insert(
            MNEMO_META_PRESENCE_LOCK.into(),
            Value::Bool(self.presence_held),
        );
        match existing {
            Some(mut m) => {
                m.metadata = meta;
                self.put(m)?;
            }
            None => {
                self.put_coordination_with_id(
                    Ulid::new(),
                    MNEMO_META_KIND_PRESENCE,
                    format!("presence {agent}"),
                    meta,
                )?;
            }
        }
        Ok(true)
    }

    /// 13a (13.3): every agent with a presence record, most recently active
    /// first: its role, tool, `last_active`, whether its process is alive
    /// (by its presence lock; [`Alive::Unknown`] for an agent with no
    /// long-lived process), and the topics it holds. Reads only.
    pub fn who(&mut self) -> Result<Vec<Presence>> {
        let claims = self.claims()?;
        let records = self.coordination_records(MNEMO_META_KIND_PRESENCE, None)?;
        let mut out: Vec<Presence> = records
            .iter()
            .map(|m| {
                let agent = meta_string(m, MNEMO_META_AGENT).unwrap_or_default();
                let locked = m
                    .metadata
                    .get(MNEMO_META_PRESENCE_LOCK)
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let alive = if !locked {
                    Alive::Unknown
                } else {
                    match crate::shared::probe_presence(&self.path, &agent) {
                        Some(true) => Alive::Yes,
                        Some(false) => Alive::No,
                        None => Alive::Unknown,
                    }
                };
                Presence {
                    claims: claims
                        .iter()
                        .filter(|c| c.holder == agent)
                        .map(|c| c.topic.clone())
                        .collect(),
                    role: meta_string(m, crate::memory::MNEMO_META_ROLE),
                    tool: meta_string(m, MNEMO_META_TOOL),
                    last_active: meta_i64(m, MNEMO_META_LAST_ACTIVE).unwrap_or(0),
                    alive,
                    agent,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            b.last_active
                .cmp(&a.last_active)
                .then(a.agent.cmp(&b.agent))
        });
        Ok(out)
    }
}

// --- 13a step 4: tasks (13.1) ------------------------------------------------

use crate::memory::{
    task_status, MNEMO_META_AFTER, MNEMO_META_CREATOR, MNEMO_META_KIND_TASK, MNEMO_META_OWNER,
    MNEMO_META_PRE_APPROVED, MNEMO_META_SLUG, TASK_CLAIM_TTL, TASK_SLUG_MAX, TASK_TRANSITIONS,
};

/// 13a (13.1): a task to add ([`Mnemo::task_add`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct TaskSpec {
    /// One line, at most 200 bytes.
    pub title: String,
    /// At most 8 KB; put more in a fact and name it.
    pub body: String,
    /// The role that may take it (`worker`); `None`, any.
    pub role: Option<String>,
    /// The slugs of the tasks it waits for; each must exist.
    pub after: Vec<String>,
    /// Its owner may go from `ready` to `done` without the creator.
    pub pre_approved: bool,
    /// Its name; made from the title when left out.
    pub slug: Option<String>,
    /// v0.18.1: what it's about: finding ids (`RB-014.1`), files, PRs. Each
    /// is one line, at most 200 bytes, and at most 32 of them.
    pub refs: Vec<String>,
}

impl TaskSpec {
    /// v0.19.0: a task titled `title`, for any role, waiting for nothing.
    /// The type is `#[non_exhaustive]`, so outside this crate set the other
    /// fields after this (`spec.role = Some("worker".into())`).
    pub fn new(title: impl Into<String>) -> Self {
        TaskSpec {
            title: title.into(),
            ..Default::default()
        }
    }
}

/// v0.18.1: the most refs a task carries, and the longest one, in bytes.
const TASK_REFS_MAX: usize = 32;
const TASK_REF_MAX: usize = 200;

/// 13a (13.1): one task.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Task {
    /// The record's id.
    pub id: Ulid,
    /// Its name.
    pub slug: String,
    /// One line.
    pub title: String,
    /// The body (may be empty).
    pub body: String,
    /// The role that may take it.
    pub role: Option<String>,
    /// `queued`, `active`, `ready`, `done` or `blocked`.
    pub status: String,
    /// The agent that took it.
    pub owner: Option<String>,
    /// The agent that added it.
    pub creator: String,
    /// The slugs it waits for.
    pub after: Vec<String>,
    /// Whether its owner may set `done`.
    pub pre_approved: bool,
    /// v0.18.1: what it's about (`_mnemo.refs`); empty when it names none.
    pub refs: Vec<String>,
    /// When it was added, in unix seconds.
    pub created_at: i64,
    /// Its status changes and notes, oldest first (`from`, `to`, `at`,
    /// `by`, and `note` or `reclaimed_from` when there's one).
    pub history: Vec<Value>,
}

impl Task {
    /// The task a record holds, if it's one.
    pub fn from_memory(m: &Memory) -> Option<Task> {
        if m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) != Some(MNEMO_META_KIND_TASK) {
            return None;
        }
        let (title, body) = match m.content.split_once("\n\n") {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (m.content.clone(), String::new()),
        };
        Some(Task {
            id: m.id,
            slug: meta_string(m, MNEMO_META_SLUG).unwrap_or_default(),
            title,
            body,
            role: meta_string(m, crate::memory::MNEMO_META_ROLE),
            status: meta_string(m, MNEMO_META_STATUS).unwrap_or_default(),
            owner: meta_string(m, MNEMO_META_OWNER),
            creator: meta_string(m, MNEMO_META_CREATOR).unwrap_or_default(),
            after: m
                .metadata
                .get(MNEMO_META_AFTER)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            pre_approved: m
                .metadata
                .get(MNEMO_META_PRE_APPROVED)
                .and_then(Value::as_bool)
                .unwrap_or(false),
            refs: m
                .metadata
                .get(crate::memory::MNEMO_META_REFS)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            created_at: m.created_at,
            history: m
                .metadata
                .get(MNEMO_META_STATUS_HISTORY)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// Whether an agent of `role` may take it (a task with no role is any
    /// role's).
    fn for_role(&self, role: Option<&str>) -> bool {
        self.role.is_none() || self.role.as_deref() == role
    }
}

/// 13a (13.1): a task [`Mnemo::task_next`] took, with its claim's token.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct TaskTaken {
    /// The task, now `active` and this agent's.
    pub task: Task,
    /// The `task:<slug>` claim's token (give it to `task_update` for the
    /// strict check).
    pub token: String,
    /// When the claim lapses unless an update renews it.
    pub expires_at: i64,
    /// The owner whose claim lapsed, when this took an abandoned task back.
    pub reclaimed_from: Option<String>,
}

/// 13a (13.1): the claim an owner holds after a [`Mnemo::task_update`]
/// (RB-012.1), so it always has the current token.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TaskClaim {
    /// The `task:<slug>` claim's token.
    pub token: String,
    /// When it lapses unless an update renews it.
    pub expires_at: i64,
    /// The claim had lapsed and nobody took the task, so the owner took
    /// the claim again (with a new token).
    pub retaken: bool,
}

/// 13a (13.1): why a [`Mnemo::task_update`] changed nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskLost {
    /// Another agent took the task back after the caller's claim lapsed.
    TakenBack,
    /// The token isn't the task's claim's (another session of the same
    /// agent may hold it).
    OtherToken,
}

/// 13a (13.1): what [`Mnemo::task_update`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
    /// Changed (or a note added). For the owner, the claim it now holds:
    /// renewed, or taken again after a lapse; `None` when the change
    /// released it (`ready`, `done`, `queued`) or wasn't the owner's.
    Updated(Box<Task>, Option<TaskClaim>),
    /// Nothing changed: don't carry on with it.
    Lost(TaskLost),
}

/// The slug a title gives: lower case, runs of anything but `a-z0-9`
/// as one `-`, at most [`TASK_SLUG_MAX`] bytes.
pub fn slug_of(title: &str) -> String {
    let mut out = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= TASK_SLUG_MAX {
            break;
        }
    }
    out.trim_end_matches('-').to_string()
}

fn valid_slug(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= TASK_SLUG_MAX
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
}

fn task_topic(slug: &str) -> String {
    format!("task:{slug}")
}

impl Mnemo {
    fn task_records(&mut self, slug: Option<&str>) -> Result<Vec<Memory>> {
        let mut v = self.coordination_records(MNEMO_META_KIND_TASK, slug)?;
        v.sort_by_key(|m| m.created_at);
        Ok(v)
    }

    fn task_record(&mut self, slug: &str) -> Result<Option<Memory>> {
        Ok(self.task_records(Some(slug))?.into_iter().next())
    }

    /// 13a (13.1): one task by slug. Reads only.
    pub fn task(&mut self, slug: &str) -> Result<Option<Task>> {
        Ok(self.task_record(slug)?.as_ref().and_then(Task::from_memory))
    }

    /// 13a (13.1): the tasks, oldest first, with `status` and for `role`
    /// when given (a task with no role matches any). Reads only.
    pub fn tasks(&mut self, status: Option<&str>, role: Option<&str>) -> Result<Vec<Task>> {
        Ok(self
            .task_records(None)?
            .iter()
            .filter_map(Task::from_memory)
            .filter(|t| status.is_none_or(|s| t.status == s))
            .filter(|t| role.is_none() || t.for_role(role))
            .collect())
    }

    /// 13a (13.1): the `queued` tasks an agent of `role` may take now (all
    /// their `after` tasks `done`), oldest first. Reads only.
    pub fn ready_tasks(&mut self, role: Option<&str>) -> Result<Vec<Task>> {
        let all: Vec<Task> = self
            .task_records(None)?
            .iter()
            .filter_map(Task::from_memory)
            .collect();
        let done: std::collections::HashSet<&str> = all
            .iter()
            .filter(|t| t.status == task_status::DONE)
            .map(|t| t.slug.as_str())
            .collect();
        Ok(all
            .iter()
            .filter(|t| t.status == task_status::QUEUED && t.for_role(role))
            .filter(|t| t.after.iter().all(|a| done.contains(a.as_str())))
            .cloned()
            .collect())
    }

    /// 13a (13.1): add a task, `queued`, created by this handle's author.
    /// A slug that already names a task updates its title, body, role,
    /// `after` and `pre_approved` in place (its status, owner and history
    /// stay); `Unchanged` when nothing differs. `after` must name existing
    /// tasks, never itself.
    pub fn task_add(&mut self, spec: &TaskSpec) -> Result<(Task, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let title = spec.title.trim();
        if title.is_empty()
            || title.contains('\n')
            || title.len() > crate::memory::MESSAGE_SUBJECT_MAX
        {
            return Err(MnemoError::Invalid(format!(
                "a task's title is one line, not empty, at most {} bytes",
                crate::memory::MESSAGE_SUBJECT_MAX
            )));
        }
        if spec.body.len() > crate::memory::MESSAGE_BODY_MAX {
            return Err(MnemoError::Invalid(format!(
                "a task's body is at most {} bytes; put more in a fact and name it",
                crate::memory::MESSAGE_BODY_MAX
            )));
        }
        let slug = match &spec.slug {
            Some(s) => s.clone(),
            None => slug_of(title),
        };
        if !valid_slug(&slug) {
            return Err(MnemoError::Invalid(format!(
                "{slug:?} isn't a slug: 1 to {TASK_SLUG_MAX} of a-z, 0-9, '.', '_' and '-' \
                 (give one with --slug)"
            )));
        }
        if let Some(r) = &spec.role {
            if !crate::memory::valid_agent_id(r) {
                return Err(MnemoError::Invalid(format!("{r:?} isn't a role")));
            }
        }
        if spec.refs.len() > TASK_REFS_MAX
            || spec
                .refs
                .iter()
                .any(|r| r.trim().is_empty() || r.contains('\n') || r.len() > TASK_REF_MAX)
        {
            return Err(MnemoError::Invalid(format!(
                "a task's refs: at most {TASK_REFS_MAX}, each one line, not empty, at most \
                 {TASK_REF_MAX} bytes"
            )));
        }
        for a in &spec.after {
            if a == &slug {
                return Err(MnemoError::Invalid("a task can't wait for itself".into()));
            }
            if self.task_record(a)?.is_none() {
                return Err(MnemoError::Invalid(format!("after: no task {a:?}")));
            }
        }
        let content = if spec.body.is_empty() {
            title.to_string()
        } else {
            format!("{title}\n\n{}", spec.body)
        };
        let set = |meta: &mut Map<String, Value>| {
            match &spec.role {
                Some(r) => {
                    meta.insert(
                        crate::memory::MNEMO_META_ROLE.into(),
                        Value::String(r.clone()),
                    );
                }
                None => {
                    meta.remove(crate::memory::MNEMO_META_ROLE);
                }
            }
            meta.insert(
                MNEMO_META_AFTER.into(),
                Value::Array(
                    spec.after
                        .iter()
                        .map(|a| Value::String(a.clone()))
                        .collect(),
                ),
            );
            meta.insert(
                MNEMO_META_PRE_APPROVED.into(),
                Value::Bool(spec.pre_approved),
            );
            // v0.18.1: only when there are refs, so re-adding an older task
            // as it was is still a no-op.
            if spec.refs.is_empty() {
                meta.remove(crate::memory::MNEMO_META_REFS);
            } else {
                meta.insert(
                    crate::memory::MNEMO_META_REFS.into(),
                    Value::Array(
                        spec.refs
                            .iter()
                            .map(|r| Value::String(r.trim().to_string()))
                            .collect(),
                    ),
                );
            }
        };
        if let Some(mut m) = self.task_record(&slug)? {
            let t = Task::from_memory(&m).expect("a task");
            let mut meta = m.metadata.clone();
            set(&mut meta);
            if meta == m.metadata && m.content == content {
                return Ok((t, WriteOutcome::Unchanged));
            }
            // RB-012.2: a finished task is never reopened by a re-add, and
            // only its creator changes what a task is.
            if t.status == task_status::DONE {
                let mut n = 2;
                while self.task_record(&format!("{slug}-{n}"))?.is_some() {
                    n += 1;
                }
                return Err(MnemoError::Invalid(format!(
                    "{slug} is done; give a new slug ({slug}-{n}?)"
                )));
            }
            let caller = self.author.clone().unwrap_or_else(|| "anonymous".into());
            if caller != t.creator {
                return Err(MnemoError::Invalid(format!(
                    "that slug is {}'s task; it wasn't changed",
                    t.creator
                )));
            }
            // RB-012.6 item 5: an `after` that closes a cycle.
            if self.closes_cycle(&slug, &spec.after)? {
                return Err(MnemoError::Invalid(format!(
                    "after: that would make {slug} wait for itself, through the tasks it waits for"
                )));
            }
            m.metadata = meta;
            m.content = content;
            let id = m.id;
            self.put(m)?;
            let t = Task::from_memory(&self.get(&id)?).expect("a task");
            return Ok((t, WriteOutcome::Updated));
        }
        let mut meta = Map::new();
        meta.insert(MNEMO_META_SLUG.into(), Value::String(slug));
        meta.insert(
            MNEMO_META_STATUS.into(),
            Value::String(task_status::QUEUED.into()),
        );
        meta.insert(
            MNEMO_META_CREATOR.into(),
            Value::String(self.author.clone().unwrap_or_else(|| "anonymous".into())),
        );
        meta.insert(
            MNEMO_META_DELIVERY.into(),
            Value::String(crate::memory::delivery::SENT.into()),
        );
        set(&mut meta);
        let id = self.put_coordination_with_id(Ulid::new(), MNEMO_META_KIND_TASK, content, meta)?;
        let t = Task::from_memory(&self.get(&id)?).expect("a task");
        Ok((t, WriteOutcome::Added))
    }

    /// Whether `slug` waiting for `after` closes a cycle: a task in `after`
    /// that waits (through its own `after`, transitively) for `slug`.
    fn closes_cycle(&mut self, slug: &str, after: &[String]) -> Result<bool> {
        let all: std::collections::HashMap<String, Vec<String>> = self
            .task_records(None)?
            .iter()
            .filter_map(Task::from_memory)
            .map(|t| (t.slug, t.after))
            .collect();
        let mut stack: Vec<&str> = after.iter().map(String::as_str).collect();
        let mut seen = std::collections::HashSet::new();
        while let Some(s) = stack.pop() {
            if s == slug {
                return Ok(true);
            }
            if seen.insert(s.to_string()) {
                if let Some(next) = all.get(s) {
                    stack.extend(next.iter().map(String::as_str));
                }
            }
        }
        Ok(false)
    }

    /// Append a `status_history` entry.
    pub(super) fn task_history(
        m: &mut Memory,
        from: &str,
        to: &str,
        by: &str,
        at: i64,
        extra: &[(&str, &str)],
    ) {
        let mut e = Map::new();
        e.insert("from".into(), Value::String(from.into()));
        e.insert("to".into(), Value::String(to.into()));
        e.insert("at".into(), Value::from(at));
        e.insert("by".into(), Value::String(by.into()));
        for (k, v) in extra {
            e.insert((*k).into(), Value::String((*v).into()));
        }
        let mut h = m
            .metadata
            .get(MNEMO_META_STATUS_HISTORY)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        h.push(Value::Object(e));
        m.metadata
            .insert(MNEMO_META_STATUS_HISTORY.into(), Value::Array(h));
    }

    /// 13a (13.1): take the next task for `role`, for this handle's author,
    /// all in one exclusive window in a shared handle: the oldest `queued`
    /// task for the role whose `after` tasks are all `done`; failing that
    /// (R-010.7), an `active` one whose `task:<slug>` claim has lapsed,
    /// taken back from its owner with a `status_history` entry naming it.
    /// It claims `task:<slug>` for [`crate::memory::TASK_CLAIM_TTL`] and
    /// sets `active` and the owner. `None` when there's nothing to take.
    pub fn task_next(&mut self, role: Option<&str>) -> Result<Option<TaskTaken>> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let agent = self.claim_agent()?;
        let now = self.coordination_now();
        let ready: Vec<Ulid> = self.ready_tasks(role)?.iter().map(|t| t.id).collect();
        // Taken tasks (`active`, or `blocked`: RB-012.3) for the role. The
        // caller's own come first (RB-012.1): a worker whose claim lapsed
        // gets its own task back before anything new. `claim()` decides
        // which are free.
        let taken: Vec<Task> = self
            .task_records(None)?
            .iter()
            .filter_map(Task::from_memory)
            .filter(|t| t.status == task_status::ACTIVE || t.status == task_status::BLOCKED)
            .filter(|t| t.for_role(role))
            .collect();
        let (own, others): (Vec<&Task>, Vec<&Task>) = taken
            .iter()
            .partition(|t| t.owner.as_deref() == Some(agent.as_str()));
        for (id, back) in own
            .iter()
            .map(|t| (t.id, true))
            .chain(ready.into_iter().map(|i| (i, false)))
            .chain(others.iter().map(|t| (t.id, true)))
        {
            let mut m = self.get(&id)?;
            let t = Task::from_memory(&m).expect("a task");
            let (token, expires_at) =
                match self.claim(&task_topic(&t.slug), Some(TASK_CLAIM_TTL))? {
                    ClaimOutcome::Claimed { token, expires_at } => (token, expires_at),
                    ClaimOutcome::HeldBy { .. } => continue,
                };
            // The claim's write may have changed the catalog: read the task
            // again before changing it.
            m = self.get(&id)?;
            let old = t.owner.clone();
            let own = old.as_deref() == Some(agent.as_str());
            if back && own {
                Self::task_history(
                    &mut m,
                    &t.status,
                    task_status::ACTIVE,
                    &agent,
                    now,
                    &[("note", "taken again after its claim lapsed")],
                );
            } else if back {
                let from = old.clone().unwrap_or_default();
                Self::task_history(
                    &mut m,
                    &t.status,
                    task_status::ACTIVE,
                    &agent,
                    now,
                    &[("reclaimed_from", from.as_str())],
                );
            } else {
                Self::task_history(
                    &mut m,
                    task_status::QUEUED,
                    task_status::ACTIVE,
                    &agent,
                    now,
                    &[],
                );
            }
            m.metadata.insert(
                MNEMO_META_STATUS.into(),
                Value::String(task_status::ACTIVE.into()),
            );
            m.metadata
                .insert(MNEMO_META_OWNER.into(), Value::String(agent.clone()));
            self.put(m)?;
            let task = Task::from_memory(&self.get(&id)?).expect("a task");
            return Ok(Some(TaskTaken {
                task,
                token,
                expires_at,
                reclaimed_from: if back && !own { old } else { None },
            }));
        }
        Ok(None)
    }

    /// 13a (13.1): change a task's status (along [`TASK_TRANSITIONS`]), or
    /// add a `note` with none, as this handle's author.
    /// - **Its owner** moves it from `active` to `ready`, `blocked` or
    ///   `queued` (handing it back) and from `blocked` to `active`, and may
    ///   add notes; each renews its `task:<slug>` claim. With a `token`,
    ///   the claim must be that token's. An owner whose task was taken back
    ///   (or whose token isn't the claim's) gets [`TaskOutcome::Lost`].
    /// - **`ready → done`** is its creator's, or its owner's when
    ///   `pre_approved`.
    /// - **`active → queued`** is anyone's once the owner's claim has
    ///   lapsed (an abandoned task).
    /// - `queued → active` happens only through [`Mnemo::task_next`].
    ///
    /// `ready`, `done` and `queued` release the claim.
    pub fn task_update(
        &mut self,
        slug: &str,
        status: Option<&str>,
        note: Option<&str>,
        token: Option<&str>,
    ) -> Result<TaskOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let agent = self.claim_agent()?;
        let now = self.coordination_now();
        let Some(m) = self.task_record(slug)? else {
            return Err(MnemoError::Invalid(format!("no task {slug:?}")));
        };
        let t = Task::from_memory(&m).expect("a task");
        let to = status.unwrap_or(&t.status).to_string();
        if status.is_none() && note.is_none() {
            return Err(MnemoError::Invalid("give a status or a note".into()));
        }
        // A token says the caller thinks the task is its own: when it isn't
        // any more, that's the answer, whatever the change asked for.
        if token.is_some() && t.owner.as_deref() != Some(agent.as_str()) {
            return Ok(TaskOutcome::Lost(TaskLost::TakenBack));
        }
        let changing = to != t.status;
        if changing && !TASK_TRANSITIONS.contains(&(t.status.as_str(), to.as_str())) {
            return Err(MnemoError::Invalid(format!(
                "a task can't go from {} to {to}",
                t.status
            )));
        }
        if changing && t.status == task_status::QUEUED {
            return Err(MnemoError::Invalid(
                "a queued task is taken with `task next`".into(),
            ));
        }
        let topic = task_topic(slug);
        let claim = self.find_claim(&topic)?;
        let live = claim.as_ref().filter(|c| is_live(c, now));
        let claim_token = claim
            .as_ref()
            .and_then(|c| meta_string(c, MNEMO_META_CLAIM_TOKEN));
        let held_by_owner = live.is_some_and(|c| meta_string(c, MNEMO_META_HOLDER) == t.owner);
        let is_owner = t.owner.as_deref() == Some(agent.as_str());
        let is_creator = t.creator == agent;
        // Who may do this.
        let by_owner = if changing && t.status == task_status::READY {
            // ready -> done
            if !(is_creator || (is_owner && t.pre_approved)) {
                return Err(MnemoError::Invalid(format!(
                    "{slug} waits for its creator ({}) to set done",
                    t.creator
                )));
            }
            false
        } else if changing && to == task_status::QUEUED && !is_owner {
            // Requeueing an abandoned task: anyone's, once its claim lapsed.
            if let Some(c) = live {
                return Err(MnemoError::Invalid(format!(
                    "{slug} is {}'s, and its claim is live until {}; only its owner can hand \
                     it back before then",
                    t.owner.as_deref().unwrap_or("its owner"),
                    crate::memory::date_time_of(effective_expiry(c, now).unwrap_or(now))
                )));
            }
            false
        } else if t.status == task_status::DONE || (!changing && t.status == task_status::READY) {
            // A note on a finished task: its creator's or owner's.
            if !(is_creator || is_owner) {
                return Err(MnemoError::Invalid(format!("{slug} isn't yours")));
            }
            false
        } else if !changing && is_creator && !is_owner {
            false // its creator's note, at any status
        } else if t.status == task_status::QUEUED {
            // A note on a queued task: its creator's.
            if !is_creator {
                return Err(MnemoError::Invalid(format!("{slug} isn't yours")));
            }
            false
        } else {
            if !is_owner {
                return Ok(TaskOutcome::Lost(TaskLost::TakenBack));
            }
            true
        };
        // The owner's claim (RB-012.1):
        // - live and its own: a given token must be the claim's;
        // - live and someone else's: taken back;
        // - lapsed: a given token must be the lapsed claim's (the record
        //   keeps it), else another session took it in between; then the
        //   owner carries on and takes the claim again.
        let mut renew_token = None;
        if by_owner {
            let token_ok = token.is_none_or(|tk| claim_token.as_deref() == Some(tk));
            if live.is_some() && !held_by_owner {
                return Ok(TaskOutcome::Lost(TaskLost::TakenBack));
            }
            if !token_ok {
                return Ok(TaskOutcome::Lost(TaskLost::OtherToken));
            }
            if held_by_owner {
                renew_token = claim_token.clone();
            }
        }
        let mut m = m;
        let mut extra: Vec<(&str, &str)> = Vec::new();
        if let Some(n) = note {
            extra.push(("note", n));
        }
        Self::task_history(&mut m, &t.status, &to, &agent, now, &extra);
        m.metadata
            .insert(MNEMO_META_STATUS.into(), Value::String(to.clone()));
        if to == task_status::QUEUED {
            m.metadata.remove(MNEMO_META_OWNER);
        }
        let id = m.id;
        self.put(m)?;
        let releases =
            [task_status::READY, task_status::DONE, task_status::QUEUED].contains(&to.as_str());
        let mut held: Option<TaskClaim> = None;
        match (releases, by_owner, renew_token) {
            (true, _, Some(tk)) => {
                self.release(&topic, &tk)?;
            }
            (true, _, None) => {
                // A lapsed or foreign-free claim: let it lapse.
                if let Some(mut c) = self.find_claim(&topic)? {
                    if is_live(&c, now)
                        && meta_string(&c, MNEMO_META_HOLDER) == t.owner
                        && to == task_status::DONE
                    {
                        c.metadata.remove(MNEMO_META_CLAIM_TOKEN);
                        c.metadata
                            .insert(MNEMO_META_EXPIRES_AT.into(), Value::from(now));
                        self.put(c)?;
                    }
                }
            }
            (false, true, Some(tk)) => {
                if let ClaimStatus::Held { expires_at } =
                    self.renew(&topic, &tk, Some(TASK_CLAIM_TTL))?
                {
                    held = Some(TaskClaim {
                        token: tk,
                        expires_at,
                        retaken: false,
                    });
                }
            }
            (false, true, None) => {
                // Lapsed, and nobody else took it: the owner takes it again,
                // and gets the new token.
                if let ClaimOutcome::Claimed { token, expires_at } =
                    self.claim(&topic, Some(TASK_CLAIM_TTL))?
                {
                    held = Some(TaskClaim {
                        token,
                        expires_at,
                        retaken: true,
                    });
                }
            }
            (false, false, _) => {}
        }
        Ok(TaskOutcome::Updated(
            Box::new(Task::from_memory(&self.get(&id)?).expect("a task")),
            held,
        ))
    }
}

// --- 13a step 5: what's new for an agent, and the wake-up guardrails ---------

use crate::memory::{
    HOP_CAP, MNEMO_META_WAKES, MNEMO_META_WAKE_PAUSED, MNEMO_META_WARNED_EXPIRY, PAIR_BURST,
    PAIR_BURST_SECS, TASK_CLAIM_WARN_SECS, WAKE_COOLDOWN_SECS, WAKE_PER_HOUR,
};

/// 13a (13.1b): what's new for an agent since it was last shown
/// ([`Mnemo::new_work`]), for the per-turn hook.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct NewWork {
    /// Unread messages to it, its role or everyone, not yet `seen`.
    pub messages: Vec<Message>,
    /// Tasks ready for its role, not yet `seen`.
    pub tasks: Vec<Task>,
    /// Its own task claims lapsing within
    /// [`crate::memory::TASK_CLAIM_WARN_SECS`], not yet warned about at this
    /// expiry.
    pub lapsing: Vec<Claim>,
    /// READY-NOTICE (v0.17.2): tasks it created that are `ready`, waiting
    /// for its `done`, not yet shown to it as such. They wait for a
    /// review, so they never wake a session.
    pub ready_for_done: Vec<Task>,
    /// v0.18.0: for an agent of role `planner`, the `open` findings not
    /// yet shown to it (`_mnemo.seen_by`). They wait for its triage, so
    /// they never wake a session.
    pub findings: Vec<super::Finding>,
    /// The agent it was read for: [`Mnemo::mark_shown`] records this agent
    /// as having seen the broadcasts and the role's tasks.
    pub agent: Option<String>,
}

impl NewWork {
    /// Whether there's nothing.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
            && self.tasks.is_empty()
            && self.lapsing.is_empty()
            && self.ready_for_done.is_empty()
            && self.findings.is_empty()
    }
}

/// 13a (13.1b): what [`Mnemo::wake`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wake {
    /// Wake the agent for these, now marked `delivered`: how many
    /// pre-approved tasks and messages addressed to it by id.
    Now {
        /// Pre-approved tasks ready for its role.
        tasks: usize,
        /// Messages to it by id.
        messages: usize,
    },
    /// Nothing that may wake it.
    Nothing,
    /// Something waits, but a guardrail holds it (its items stay `sent`,
    /// and are seen at the next turn or session start).
    Held(&'static str),
}

/// v0.20.1 (RB-017.1): what [`Mnemo::wake_items`] decided, with the items
/// it marked `delivered`, so a caller can name them (`mnemo wait`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WakeItems {
    /// Wake the agent for these, now marked `delivered`.
    Now {
        /// Pre-approved tasks ready for its role.
        tasks: Vec<Task>,
        /// Messages to it by id.
        messages: Vec<Message>,
    },
    /// Nothing that may wake it.
    Nothing,
    /// Something waits, but a guardrail holds it (as [`Wake::Held`]).
    Held(&'static str),
}

/// The items that may wake an agent, before the guardrails: its role's
/// pre-approved tasks and the messages to it by id, still `sent`; and every
/// message, for the pair-burst count.
struct WakeCandidates {
    tasks: Vec<Task>,
    messages: Vec<Message>,
    all_msgs: Vec<Message>,
}

impl Mnemo {
    /// 13a (13.1b): what's new for `agent` (and `role`) since it was last
    /// shown. Reads only; [`Mnemo::mark_shown`] records that it was.
    pub fn new_work(&mut self, agent: Option<&str>, role: Option<&str>) -> Result<NewWork> {
        let now = self.coordination_now();
        let unseen =
            |d: &str| d == crate::memory::delivery::SENT || d == crate::memory::delivery::DELIVERED;
        // RB-012.6 item 4: a message to the agent by id is new until it's
        // seen; one to a role or everyone, and a role's task, until this
        // agent has been shown it (`_mnemo.seen_by`).
        let mut messages = Vec::new();
        for msg in self.inbox(agent, role, true)? {
            let broadcast = msg.to == "all" || msg.to.starts_with("role:");
            let new = match (broadcast, agent) {
                (true, Some(a)) => !seen_by(&self.get(&msg.id)?).iter().any(|s| s == a),
                _ => unseen(&msg.delivery),
            };
            if new {
                messages.push(msg);
            }
        }
        let mut tasks = Vec::new();
        for t in self.ready_tasks(role)? {
            let m = self.get(&t.id)?;
            let new = match agent {
                Some(a) => !seen_by(&m).iter().any(|s| s == a),
                None => meta_string(&m, MNEMO_META_DELIVERY).is_none_or(|d| unseen(&d)),
            };
            if new {
                tasks.push(t);
            }
        }
        // READY-NOTICE: its own tasks waiting for its `done`, not yet
        // shown to it as such.
        let mut ready_for_done = Vec::new();
        if let Some(a) = agent {
            for t in self.tasks(Some(crate::memory::task_status::READY), None)? {
                if t.creator == a && !ready_seen_by(&self.get(&t.id)?).iter().any(|s| s == a) {
                    ready_for_done.push(t);
                }
            }
        }
        // v0.18.0: the findings waiting for a Planner's triage.
        let mut findings = Vec::new();
        if let (Some(a), Some("planner")) = (agent, role) {
            let open = super::FindingFilter {
                status: Some(crate::memory::finding_status::OPEN.into()),
                ..Default::default()
            };
            for f in self.findings(&open)? {
                if !seen_by(&self.get(&f.record_id)?).iter().any(|s| s == a) {
                    findings.push(f);
                }
            }
        }
        let mut lapsing = Vec::new();
        if let Some(a) = agent {
            for m in self.coordination_records(MNEMO_META_KIND_CLAIM, None)? {
                let topic = meta_string(&m, MNEMO_META_TOPIC).unwrap_or_default();
                if !topic.starts_with("task:")
                    || meta_string(&m, MNEMO_META_HOLDER).as_deref() != Some(a)
                    || !is_live(&m, now)
                {
                    continue;
                }
                let e = effective_expiry(&m, now).unwrap_or(now);
                if e - now <= TASK_CLAIM_WARN_SECS
                    && meta_i64(&m, MNEMO_META_WARNED_EXPIRY) != Some(e)
                {
                    lapsing.push(Claim {
                        topic,
                        holder: a.to_string(),
                        expires_at: e,
                        lease: meta_i64(&m, MNEMO_META_LEASE).unwrap_or(CLAIM_TTL_DEFAULT),
                    });
                }
            }
        }
        Ok(NewWork {
            messages,
            tasks,
            lapsing,
            ready_for_done,
            findings,
            agent: agent.map(str::to_string),
        })
    }

    /// 13a (13.1b): record that `work` was shown: its messages and tasks
    /// `seen`, its tasks waiting for a `done` shown to their creator
    /// (READY-NOTICE), its lapsing claims warned about at their expiry. In
    /// place.
    pub fn mark_shown(&mut self, work: &NewWork) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let ids: Vec<Ulid> = work.messages.iter().map(|m| m.id).collect();
        // Seen by the agent the work was read for.
        let who = work.agent.clone().or_else(|| self.author.clone());
        let author = std::mem::replace(&mut self.author, who.clone());
        let r = self.mark_seen(&ids);
        self.author = author;
        r?;
        for t in &work.tasks {
            let mut m = self.get(&t.id)?;
            match who.clone() {
                // Seen by this agent; the task stays new to the others.
                Some(a) => {
                    if add_seen_by(&mut m, &a) {
                        self.put(m)?;
                    }
                }
                None => {
                    m.metadata.insert(
                        MNEMO_META_DELIVERY.into(),
                        Value::String(crate::memory::delivery::SEEN.into()),
                    );
                    self.put(m)?;
                }
            }
        }
        // v0.18.0: the findings shown to the Planner that read them.
        if let Some(a) = &who {
            for f in &work.findings {
                let mut m = self.get(&f.record_id)?;
                if add_seen_by(&mut m, a) {
                    self.put(m)?;
                }
            }
        }
        // READY-NOTICE: shown to its creator as waiting for its `done`.
        if let Some(a) = &who {
            for t in &work.ready_for_done {
                let mut m = self.get(&t.id)?;
                let mut v = ready_seen_by(&m);
                if v.iter().any(|s| s == a) {
                    continue;
                }
                v.push(a.clone());
                m.metadata.insert(
                    crate::memory::MNEMO_META_READY_SEEN_BY.into(),
                    Value::Array(v.into_iter().map(Value::String).collect()),
                );
                self.put(m)?;
            }
        }
        for c in &work.lapsing {
            if let Some(mut m) = self.find_claim(&c.topic)? {
                m.metadata
                    .insert(MNEMO_META_WARNED_EXPIRY.into(), Value::from(c.expires_at));
                self.put(m)?;
            }
        }
        Ok(())
    }

    /// 13a (13.1b): whether wake-ups are paused (the kill switch): who
    /// paused them and when.
    pub fn wake_paused(&mut self) -> Result<Option<(String, i64)>> {
        let Some(m) = self
            .about_in(crate::memory::VisibilitySet::ALL)?
            .into_iter()
            .find(is_manifest)
        else {
            return Ok(None);
        };
        Ok(m.metadata.get(MNEMO_META_WAKE_PAUSED).map(|v| {
            (
                v.get("by")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                v.get("at").and_then(Value::as_i64).unwrap_or(0),
            )
        }))
    }

    /// 13a (13.1b): pause (`true`) or resume wake-ups for every session
    /// of this brain, on the manifest. Paused, nothing wakes anyone: items
    /// stay `sent`, and the per-turn hook and the brief still show them.
    pub fn set_wake_paused(&mut self, paused: bool) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let Some(mut m) = self
            .about_in(crate::memory::VisibilitySet::ALL)?
            .into_iter()
            .find(is_manifest)
        else {
            return Err(MnemoError::Invalid(
                "this brain has no manifest to record the pause on".into(),
            ));
        };
        if m.metadata.contains_key(MNEMO_META_WAKE_PAUSED) == paused {
            return Ok(WriteOutcome::Unchanged);
        }
        if paused {
            let mut v = Map::new();
            v.insert(
                "by".into(),
                Value::String(self.author.clone().unwrap_or_else(|| "anonymous".into())),
            );
            v.insert("at".into(), Value::from(self.coordination_now()));
            m.metadata
                .insert(MNEMO_META_WAKE_PAUSED.into(), Value::Object(v));
        } else {
            m.metadata.remove(MNEMO_META_WAKE_PAUSED);
        }
        self.put(m)?;
        Ok(WriteOutcome::Updated)
    }

    /// 13a (13.1b): the wake-up guardrails, in the one place that marks an
    /// item `delivered`. For `agent` (and `role`), the items that may wake
    /// it: pre-approved tasks ready for its role, and messages addressed to
    /// it by id (never to its role or `all`), still `sent`. Then:
    /// - **paused** (the kill switch): [`Wake::Held`];
    /// - **an explicit reply past the hop cap** ([`crate::memory::HOP_CAP`])
    ///   or **a pair in a burst** ([`crate::memory::PAIR_BURST`] messages
    ///   either way in [`crate::memory::PAIR_BURST_SECS`]) wakes nobody;
    /// - **the recipient's rate** ([`crate::memory::WAKE_PER_HOUR`]) and
    ///   **cooldown** ([`crate::memory::WAKE_COOLDOWN_SECS`]), from its
    ///   presence record's `_mnemo.wakes`.
    ///
    /// On [`Wake::Now`] the items are marked `delivered` and the wake-up is
    /// recorded on the recipient's presence record, in place.
    /// [`Mnemo::wake_items`] does the same and returns the items.
    pub fn wake(&mut self, agent: &str, role: Option<&str>) -> Result<Wake> {
        Ok(match self.wake_items(agent, role)? {
            WakeItems::Now { tasks, messages } => Wake::Now {
                tasks: tasks.len(),
                messages: messages.len(),
            },
            WakeItems::Nothing => Wake::Nothing,
            WakeItems::Held(why) => Wake::Held(why),
        })
    }

    /// v0.20.1 (RB-017.4): whether anything may wake `agent` (and `role`):
    /// a pre-approved task or a message to it by id, still `sent`, before
    /// the guardrails. Reads only, so it runs on a read-only handle; when
    /// it's `false`, [`Mnemo::wake`] would return [`Wake::Nothing`].
    pub fn may_wake(&mut self, agent: &str, role: Option<&str>) -> Result<bool> {
        let c = self.wake_candidates(agent, role)?;
        Ok(!c.tasks.is_empty() || !c.messages.is_empty())
    }

    fn wake_candidates(&mut self, agent: &str, role: Option<&str>) -> Result<WakeCandidates> {
        let sent = crate::memory::delivery::SENT;
        let mut tasks = Vec::new();
        for t in self.ready_tasks(role)? {
            let m = self.get(&t.id)?;
            if t.pre_approved
                && meta_string(&m, MNEMO_META_DELIVERY).as_deref() == Some(sent)
                && !seen_by(&m).iter().any(|s| s == agent)
            {
                tasks.push(t);
            }
        }
        let all_msgs: Vec<Message> = self
            .messages()?
            .iter()
            .filter_map(Message::from_memory)
            .collect();
        let messages = all_msgs
            .iter()
            .filter(|m| m.to == agent && m.from != agent)
            .filter(|m| m.status == crate::memory::MESSAGE_UNREAD && m.delivery == sent)
            .cloned()
            .collect();
        Ok(WakeCandidates {
            tasks,
            messages,
            all_msgs,
        })
    }

    /// v0.20.1 (RB-017.1): [`Mnemo::wake`], returning the items it marked
    /// `delivered` instead of their counts. The same guardrails, the same
    /// writes, in the same window.
    pub fn wake_items(&mut self, agent: &str, role: Option<&str>) -> Result<WakeItems> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = self.coordination_now();
        let WakeCandidates {
            tasks,
            messages: candidates,
            all_msgs,
        } = self.wake_candidates(agent, role)?;
        if tasks.is_empty() && candidates.is_empty() {
            return Ok(WakeItems::Nothing);
        }
        if self.wake_paused()?.is_some() {
            return Ok(WakeItems::Held("paused"));
        }
        let burst = |a: &str, b: &str| {
            all_msgs
                .iter()
                .filter(|m| now - m.created_at <= PAIR_BURST_SECS)
                .filter(|m| (m.from == a && m.to == b) || (m.from == b && m.to == a))
                .count()
                >= PAIR_BURST
        };
        let mut reason = None;
        let messages: Vec<Message> = candidates
            .into_iter()
            .filter(|m| {
                let reply = m.conversation != m.id.to_string();
                if reply && m.hops > HOP_CAP {
                    reason = Some("the thread reached its hop cap");
                    return false;
                }
                if burst(&m.from, &m.to) {
                    reason = Some("a burst of messages between two agents");
                    return false;
                }
                true
            })
            .collect();
        if tasks.is_empty() && messages.is_empty() {
            return Ok(WakeItems::Held(reason.unwrap_or("a guardrail")));
        }
        // The recipient's rate and cooldown, from its presence record.
        let presence = self
            .coordination_records(MNEMO_META_KIND_PRESENCE, Some(agent))?
            .into_iter()
            .next();
        let mut wakes: Vec<i64> = presence
            .as_ref()
            .and_then(|m| m.metadata.get(MNEMO_META_WAKES))
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        wakes.retain(|t| now - t < 3600 && *t <= now);
        if wakes.len() >= WAKE_PER_HOUR {
            return Ok(WakeItems::Held("the hourly limit of wake-ups"));
        }
        if wakes.iter().any(|t| now - t < WAKE_COOLDOWN_SECS) {
            return Ok(WakeItems::Held("the cooldown between wake-ups"));
        }
        wakes.push(now);
        let delivered = Value::String(crate::memory::delivery::DELIVERED.into());
        let ids: Vec<Ulid> = tasks
            .iter()
            .map(|t| t.id)
            .chain(messages.iter().map(|m| m.id))
            .collect();
        for id in &ids {
            let mut m = self.get(id)?;
            m.metadata
                .insert(MNEMO_META_DELIVERY.into(), delivered.clone());
            self.put(m)?;
        }
        let w = Value::Array(wakes.into_iter().map(Value::from).collect());
        match presence {
            Some(mut m) => {
                m.metadata.insert(MNEMO_META_WAKES.into(), w);
                self.put(m)?;
            }
            None => {
                let mut meta = Map::new();
                meta.insert(MNEMO_META_AGENT.into(), Value::String(agent.to_string()));
                meta.insert(MNEMO_META_LAST_ACTIVE.into(), Value::from(0));
                meta.insert(MNEMO_META_PRESENCE_LOCK.into(), Value::Bool(false));
                meta.insert(MNEMO_META_WAKES.into(), w);
                self.put_coordination_with_id(
                    Ulid::new(),
                    MNEMO_META_KIND_PRESENCE,
                    format!("presence {agent}"),
                    meta,
                )?;
            }
        }
        Ok(WakeItems::Now { tasks, messages })
    }
}

// --- 13a step 7: freshness (13.6) --------------------------------------------

/// 13a (13.6): one stale record ([`Mnemo::stale`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Stale {
    /// The record's id.
    pub id: Ulid,
    /// What it is: `essence state`, `essence next`, `essence open`, `task
    /// active`, `task blocked`, `message unread`, or `fact` (past its
    /// `review_by`).
    pub what: String,
    /// When it went stale, in unix seconds.
    pub since: i64,
    /// Its first line (a task's title, a message's subject).
    pub text: String,
    /// Whose it is: a task's owner, a message's recipient, else its author.
    pub whose: Option<String>,
    /// Its visibility level (`public`, `team` or `private`).
    pub visibility: String,
}

impl Mnemo {
    /// 13a (13.6): every record that's stale now
    /// ([`crate::memory::stale_since`]), oldest first: past its `review_by`,
    /// or older than its kind's default (essence `state` and `next` 14
    /// days, `open` 30, an `active` task with no update 3, a `blocked` one
    /// 7, an unread message 3). Reads only the rows that can be (essence,
    /// coordination, and those with a `review_by`).
    pub fn stale(&mut self) -> Result<Vec<Stale>> {
        use super::records::{ROW_KIND_ESSENCE, ROW_REVIEW};
        let now = self.coordination_now();
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .filter(|e| {
                e.v9.is_none_or(|r| {
                    r.kind == ROW_KIND_ESSENCE
                        || r.kind == ROW_KIND_COORDINATION
                        || r.flags & ROW_REVIEW != 0
                })
            })
            .cloned()
            .collect();
        let mut out = Vec::new();
        for e in entries {
            let m = self.read_memory(&e)?;
            let Some(since) = crate::memory::stale_since(&m, now) else {
                continue;
            };
            let s = |k: &str| meta_string(&m, k);
            let kind = s(MNEMO_META_KIND);
            let what = match kind.as_deref() {
                Some("essence") => format!(
                    "essence {}",
                    s(crate::memory::MNEMO_META_SECTION).unwrap_or_default()
                ),
                Some(k @ ("task" | "message")) => {
                    format!("{k} {}", s(MNEMO_META_STATUS).unwrap_or_default())
                }
                Some(k) => k.to_string(),
                None => "fact".to_string(),
            };
            let whose = match kind.as_deref() {
                Some("task") => s(MNEMO_META_OWNER),
                Some("message") => s(MNEMO_META_TO),
                _ => s(crate::memory::MNEMO_META_AUTHOR),
            };
            out.push(Stale {
                id: m.id,
                what,
                since,
                text: m.content.lines().next().unwrap_or("").to_string(),
                whose,
                visibility: crate::memory::visibility_of(&m).to_string(),
            });
        }
        out.sort_by_key(|x| (x.since, x.id.0));
        Ok(out)
    }
}

/// 13a (ABOUT-COUNT): the live records by what they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct KindCounts {
    /// Facts and onboarding records (what recall and `about` read).
    pub facts: usize,
    /// Thread turns and thread links.
    pub turns: usize,
    /// Essence items.
    pub essence: usize,
    /// Messages, tasks, claims and presence records.
    pub coordination: usize,
}

impl Mnemo {
    /// 13a (ABOUT-COUNT): how many live records of each kind, from the
    /// catalog's rows (a v8 file's records have none and count as facts).
    /// Superseded versions count too, as in [`Mnemo::len`].
    pub fn kind_counts(&self) -> KindCounts {
        use super::records::{ROW_KIND_ESSENCE, ROW_KIND_THREAD_LINK, ROW_KIND_TURN};
        let mut c = KindCounts::default();
        for e in self.catalog.iter().filter(|e| !e.deleted) {
            match e.v9.map(|r| r.kind) {
                Some(ROW_KIND_TURN | ROW_KIND_THREAD_LINK) => c.turns += 1,
                Some(ROW_KIND_ESSENCE) => c.essence += 1,
                Some(ROW_KIND_COORDINATION) => c.coordination += 1,
                _ => c.facts += 1,
            }
        }
        c
    }
}

#[cfg(test)]
mod legacy_scan_tests {
    use super::is_sentinel;

    /// COORD-LEGACY-SCAN: the sentinel, quantised as the column stores it,
    /// is recognised at every width; an ordinary vector isn't.
    #[test]
    fn the_sentinel_is_told_from_an_ordinary_vector() {
        for dims in [4, 32, 384, 768] {
            let s = vec![1.0 / (dims as f32).sqrt(); dims];
            let (h, _) = crate::veccol::quantize(&s);
            assert!(is_sentinel(&h), "{dims}");
            let mut v = s.clone();
            v[dims / 2] += 0.05;
            assert!(!is_sentinel(&crate::veccol::quantize(&v).0), "{dims}");
            let flat = vec![0.5; dims];
            assert_eq!(
                is_sentinel(&crate::veccol::quantize(&flat).0),
                dims == 4,
                "{dims}"
            );
        }
        assert!(!is_sentinel(&[]));
    }
}
