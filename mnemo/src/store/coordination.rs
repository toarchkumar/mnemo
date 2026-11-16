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
    /// The clock coordination runs on (the wall clock; tests shift it).
    fn coordination_now(&self) -> i64 {
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
