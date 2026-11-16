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
    /// Every live coordination record of `kind`, in catalog order; with a
    /// `locator` (a claim's topic, a presence record's agent), only that
    /// one's, found through its row's hash without decrypting the rest.
    fn coordination_records(&mut self, kind: &str, locator: Option<&str>) -> Result<Vec<Memory>> {
        let want = locator.map(|l| locator_hash_of(kind, l));
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .filter(|e| {
                e.v9.is_none_or(|r| {
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
                    match crate::shared::probe_presence(&self.path, &holder) {
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
        if let Some(m) = &existing {
            let fresh = meta_i64(m, MNEMO_META_LAST_ACTIVE)
                .is_some_and(|t| now >= t && now - t < PRESENCE_EVERY_SECS);
            let same = m.metadata.get(crate::memory::MNEMO_META_ROLE) == role_v.as_ref()
                && m.metadata.get(MNEMO_META_TOOL) == tool_v.as_ref()
                && m.metadata
                    .get(MNEMO_META_PRESENCE_LOCK)
                    .and_then(Value::as_bool)
                    == Some(self.presence_held);
            if fresh && same {
                return Ok(false);
            }
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
}

/// 13a (13.1): one task.
#[derive(Debug, Clone, PartialEq)]
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

/// 13a (13.1): what [`Mnemo::task_update`] did.
#[derive(Debug, Clone, PartialEq)]
pub enum TaskOutcome {
    /// Changed (or a note added), and the owner's claim renewed.
    Updated(Box<Task>),
    /// The caller isn't its owner any more (another agent took it back
    /// after its claim lapsed), or the token isn't the claim's: nothing
    /// changed.
    Lost,
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
        };
        if let Some(mut m) = self.task_record(&slug)? {
            let mut meta = m.metadata.clone();
            set(&mut meta);
            if meta == m.metadata && m.content == content {
                let t = Task::from_memory(&m).expect("a task");
                return Ok((t, WriteOutcome::Unchanged));
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

    /// Append a `status_history` entry.
    fn task_history(
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
        let active: Vec<Ulid> = self
            .tasks(Some(task_status::ACTIVE), None)?
            .into_iter()
            .filter(|t| t.for_role(role) && t.owner.as_deref() != Some(agent.as_str()))
            .map(|t| t.id)
            .collect();
        for (id, back) in ready
            .into_iter()
            .map(|i| (i, false))
            .chain(active.into_iter().map(|i| (i, true)))
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
            if back {
                let from = old.clone().unwrap_or_default();
                Self::task_history(
                    &mut m,
                    task_status::ACTIVE,
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
                reclaimed_from: if back { old } else { None },
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
        let held_by_owner = live.is_some_and(|c| {
            meta_string(c, MNEMO_META_HOLDER) == t.owner
                && token
                    .is_none_or(|tk| meta_string(c, MNEMO_META_CLAIM_TOKEN).as_deref() == Some(tk))
        });
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
        } else if changing && to == task_status::QUEUED && !is_owner && live.is_none() {
            false // requeueing an abandoned task
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
                return Ok(TaskOutcome::Lost);
            }
            true
        };
        // The owner's claim: theirs and live, else lost to someone else, or
        // lapsed with nobody else holding it (then taken again).
        let mut renew_token = None;
        if by_owner {
            if held_by_owner {
                renew_token = live.and_then(|c| meta_string(c, MNEMO_META_CLAIM_TOKEN));
            } else if live.is_some() || token.is_some() {
                return Ok(TaskOutcome::Lost);
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
                self.renew(&topic, &tk, Some(TASK_CLAIM_TTL))?;
            }
            (false, true, None) => {
                // Lapsed, and nobody else took it: the owner takes it again.
                self.claim(&topic, Some(TASK_CLAIM_TTL))?;
            }
            (false, false, _) => {}
        }
        Ok(TaskOutcome::Updated(Box::new(
            Task::from_memory(&self.get(&id)?).expect("a task"),
        )))
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
pub struct NewWork {
    /// Unread messages to it, its role or everyone, not yet `seen`.
    pub messages: Vec<Message>,
    /// Tasks ready for its role, not yet `seen`.
    pub tasks: Vec<Task>,
    /// Its own task claims lapsing within
    /// [`crate::memory::TASK_CLAIM_WARN_SECS`], not yet warned about at this
    /// expiry.
    pub lapsing: Vec<Claim>,
}

impl NewWork {
    /// Whether there's nothing.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty() && self.tasks.is_empty() && self.lapsing.is_empty()
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

impl Mnemo {
    /// 13a (13.1b): what's new for `agent` (and `role`) since it was last
    /// shown. Reads only; [`Mnemo::mark_shown`] records that it was.
    pub fn new_work(&mut self, agent: Option<&str>, role: Option<&str>) -> Result<NewWork> {
        let now = self.coordination_now();
        let unseen =
            |d: &str| d == crate::memory::delivery::SENT || d == crate::memory::delivery::DELIVERED;
        let messages = self
            .inbox(agent, role, true)?
            .into_iter()
            .filter(|m| unseen(&m.delivery))
            .collect();
        let mut tasks = Vec::new();
        for t in self.ready_tasks(role)? {
            let m = self.get(&t.id)?;
            if meta_string(&m, MNEMO_META_DELIVERY).is_none_or(|d| unseen(&d)) {
                tasks.push(t);
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
        })
    }

    /// 13a (13.1b): record that `work` was shown: its messages and tasks
    /// `seen`, its lapsing claims warned about at their expiry. In place.
    pub fn mark_shown(&mut self, work: &NewWork) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let ids: Vec<Ulid> = work.messages.iter().map(|m| m.id).collect();
        self.mark_seen(&ids)?;
        for t in &work.tasks {
            let mut m = self.get(&t.id)?;
            m.metadata.insert(
                MNEMO_META_DELIVERY.into(),
                Value::String(crate::memory::delivery::SEEN.into()),
            );
            self.put(m)?;
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
    pub fn wake(&mut self, agent: &str, role: Option<&str>) -> Result<Wake> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = self.coordination_now();
        let sent = crate::memory::delivery::SENT;
        let mut tasks = Vec::new();
        for t in self.ready_tasks(role)? {
            let m = self.get(&t.id)?;
            if t.pre_approved && meta_string(&m, MNEMO_META_DELIVERY).as_deref() == Some(sent) {
                tasks.push(t.id);
            }
        }
        let all_msgs: Vec<Message> = self
            .messages()?
            .iter()
            .filter_map(Message::from_memory)
            .collect();
        let candidates: Vec<&Message> = all_msgs
            .iter()
            .filter(|m| m.to == agent && m.from != agent)
            .filter(|m| m.status == crate::memory::MESSAGE_UNREAD && m.delivery == sent)
            .collect();
        if tasks.is_empty() && candidates.is_empty() {
            return Ok(Wake::Nothing);
        }
        if self.wake_paused()?.is_some() {
            return Ok(Wake::Held("paused"));
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
        let messages: Vec<Ulid> = candidates
            .iter()
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
            .map(|m| m.id)
            .collect();
        if tasks.is_empty() && messages.is_empty() {
            return Ok(Wake::Held(reason.unwrap_or("a guardrail")));
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
            return Ok(Wake::Held("the hourly limit of wake-ups"));
        }
        if wakes.iter().any(|t| now - t < WAKE_COOLDOWN_SECS) {
            return Ok(Wake::Held("the cooldown between wake-ups"));
        }
        wakes.push(now);
        let delivered = Value::String(crate::memory::delivery::DELIVERED.into());
        for id in tasks.iter().chain(messages.iter()) {
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
        Ok(Wake::Now {
            tasks: tasks.len(),
            messages: messages.len(),
        })
    }
}

// --- 13a step 7: freshness (13.6) --------------------------------------------

/// 13a (13.6): one stale record ([`Mnemo::stale`]).
#[derive(Debug, Clone, PartialEq, Eq)]
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
            });
        }
        out.sort_by_key(|x| (x.since, x.id.0));
        Ok(out)
    }
}

/// 13a (ABOUT-COUNT): the live records by what they are.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
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
