//! v0.21.0 (13.7, spec.v0210 step 2): `Mnemo::merge_files`, which combines
//! two diverged copies of one brain into a new file.
//!
//! The rules ([`merge_core`]) are a pure function of the two inputs' records
//! and one `now`: they never read the clock and never make a random id, so
//! `merge(a, b)` and `merge(b, a)` give the same records (claims and
//! presence come from the first input only), and merging an output again
//! with an input changes nothing. In order:
//!
//! 0. Every record is classified by its decoded `_mnemo.kind`, never by its
//!    row. What compaction drops (expired records, old acked messages) is
//!    dropped from both inputs.
//! 1. Deletes first, by id, with no clock: a tombstone, or a deleted row
//!    with no live record behind it, removes the id it names, unless the
//!    same input holds that id live again. A turn's tombstone removes the
//!    turn under every id. A conflict copy goes with its source.
//! 2. One number per record for every comparison: the larger of
//!    `_mnemo.hlc` and `_mnemo.updated_at` in ms, else `created_at` in ms.
//! 3. The same id in both inputs is resolved by its kind (facts get a
//!    conflict copy; a key slot becomes one chain; essence, tasks, findings
//!    and messages merge their status; engine records take the larger
//!    number), then names, turns and tombstones are made unique.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::*;
use crate::memory::{
    finding_status, task_status, MNEMO_META_FINDING_ID, MNEMO_META_FIRST_HLC,
    MNEMO_META_MERGE_CONFLICT, MNEMO_META_MERGE_HEADS, MNEMO_META_MERGE_LINKED,
    MNEMO_META_MERGE_NAME, MNEMO_META_MERGE_SOURCE, MNEMO_META_OWNER, MNEMO_META_READY_SEEN_BY,
    MNEMO_META_SEEN_BY, MNEMO_META_SLUG, MNEMO_META_STATUS, MNEMO_META_STATUS_HISTORY,
    MNEMO_META_TOMBSTONE_SUPERSEDES,
};

/// One input of the rules: its live records (with vectors and access
/// stats) and the ids it deleted with no tombstone (deleted catalog rows).
#[derive(Clone, Debug, Default)]
pub(crate) struct MergeInput {
    pub(crate) records: Vec<Memory>,
    pub(crate) deleted: Vec<Ulid>,
}

/// What a record is, from its decoded `_mnemo.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Fact,
    Manifest,
    Turn,
    Essence,
    Task,
    Finding,
    Message,
    /// Claims and presence: the first input's only.
    Local,
    Tombstone,
    /// Thread links and any other engine record.
    Engine,
}

fn kind(m: &Memory) -> Kind {
    match meta_str(m, MNEMO_META_KIND) {
        Some(MNEMO_META_KIND_TURN) => Kind::Turn,
        Some(MNEMO_META_KIND_ESSENCE) => Kind::Essence,
        Some(crate::memory::MNEMO_META_KIND_TASK) => Kind::Task,
        Some(crate::memory::MNEMO_META_KIND_FINDING) => Kind::Finding,
        Some(crate::memory::MNEMO_META_KIND_MESSAGE) => Kind::Message,
        Some(crate::memory::MNEMO_META_KIND_CLAIM | crate::memory::MNEMO_META_KIND_PRESENCE) => {
            Kind::Local
        }
        Some(MNEMO_META_KIND_TOMBSTONE) => Kind::Tombstone,
        Some(_) => Kind::Engine,
        None if is_manifest(m) => Kind::Manifest,
        None => Kind::Fact,
    }
}

fn ulid_at(m: &Memory, key: &str) -> Option<u128> {
    meta_str(m, key)
        .and_then(|s| Ulid::from_string(s).ok())
        .map(|u| u.0)
}

/// Rule (2): the one number every comparison uses.
pub(crate) fn number(m: &Memory) -> u64 {
    let hlc = m.metadata.get(MNEMO_META_HLC).and_then(Value::as_u64);
    let updated = m
        .metadata
        .get(MNEMO_META_UPDATED_AT)
        .and_then(Value::as_i64)
        .map(|s| s.max(0) as u64 * 1000);
    match (hlc, updated) {
        (None, None) => m.created_at.max(0) as u64 * 1000,
        (h, u) => h.unwrap_or(0).max(u.unwrap_or(0)),
    }
}

fn rounded(v: &[f32]) -> Vec<f32> {
    crate::veccol::dequantize(&crate::veccol::quantize(v).0)
}

/// The user-visible metadata: all but the engine's `_mnemo.*` keys (the
/// caller's own, visibility and freshness, count).
fn visible_metadata(m: &Memory) -> Map<String, Value> {
    m.metadata
        .iter()
        .filter(|(k, _)| !k.starts_with(crate::memory::MNEMO_META_NS) || is_caller_key(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// A hash of what a user sees of a record (the keyed write's comparison:
/// content, type, importance, the vector after fp16 rounding, user
/// metadata), with its scope, agent and time to live.
fn content_hash(m: &Memory) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let put = |h: &mut Sha256, b: &[u8]| {
        h.update((b.len() as u64).to_le_bytes());
        h.update(b);
    };
    put(&mut h, m.content.as_bytes());
    put(&mut h, m.memory_type.as_str().as_bytes());
    put(&mut h, &m.importance.to_bits().to_le_bytes());
    let v: Vec<u8> = rounded(&m.vector)
        .iter()
        .flat_map(|x| x.to_bits().to_le_bytes())
        .collect();
    put(&mut h, &v);
    put(
        &mut h,
        &serde_json::to_vec(&visible_metadata(m)).unwrap_or_default(),
    );
    put(&mut h, format!("{:?}", m.scope).as_bytes());
    put(&mut h, m.agent_id.as_bytes());
    put(&mut h, format!("{:?}", m.ttl_secs).as_bytes());
    h.finalize().into()
}

/// The whole record's bytes, as the comparison of last resort: vector
/// rounded, access stats left out.
fn record_bytes(m: &Memory) -> Vec<u8> {
    let mut c = m.clone();
    c.vector = rounded(&c.vector);
    c.accessed_at = 0;
    c.access_count = 0;
    rmp_serde::to_vec(&c).unwrap_or_default()
}

fn same_visible(a: &Memory, b: &Memory) -> bool {
    content_hash(a) == content_hash(b)
}

/// A version's bytes without the lists a merge changes (`_mnemo.ver` and
/// `_mnemo.hlc_prev`): the same version reads the same in every grouping.
fn core_bytes(m: &Memory) -> Vec<u8> {
    let mut c = m.clone();
    c.metadata.remove(MNEMO_META_VER);
    c.metadata.remove(MNEMO_META_HLC_PREV);
    record_bytes(&c)
}

/// Whether `a` wins over `b` (the same id): the larger number, then the
/// larger content hash, then the larger bytes of the version itself.
fn wins(a: &Memory, b: &Memory) -> bool {
    (number(a), content_hash(a), core_bytes(a)) >= (number(b), content_hash(b), core_bytes(b))
}

/// A conflict copy's id: the source's time part (so it sorts beside it) and
/// 80 bits of a hash of the source id and the content it holds, the same in
/// every run.
fn copy_id(source: u128, content: &[u8; 32]) -> u128 {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(b"mnemo merge copy");
    h.update(source.to_be_bytes());
    h.update(content);
    let d = h.finalize();
    let mut low = [0u8; 16];
    low[6..].copy_from_slice(&d[..10]);
    let low = u128::from_be_bytes(low);
    (source & !((1u128 << 80) - 1)) | low
}

fn history_of(m: &Memory) -> Vec<Value> {
    m.metadata
        .get(MNEMO_META_STATUS_HISTORY)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Two status histories as one: each entry once, by `at`, then its bytes.
fn union_history(a: &Memory, b: &Memory) -> Vec<Value> {
    let mut seen: BTreeMap<(i64, Vec<u8>), Value> = BTreeMap::new();
    for e in history_of(a).into_iter().chain(history_of(b)) {
        let at = e.get("at").and_then(Value::as_i64).unwrap_or(0);
        let bytes = serde_json::to_vec(&e).unwrap_or_default();
        seen.entry((at, bytes)).or_insert(e);
    }
    seen.into_values().collect()
}

/// Write the two histories' union on `body`, leaving the field out when
/// neither input had it and there's nothing in it.
fn put_history(body: &mut Memory, a: &Memory, b: &Memory, history: Vec<Value>) {
    let had = [a, b]
        .iter()
        .any(|m| m.metadata.contains_key(MNEMO_META_STATUS_HISTORY));
    let v = (had || !history.is_empty()).then_some(Value::Array(history));
    set_or_remove(body, MNEMO_META_STATUS_HISTORY, v);
}

fn string_list(m: &Memory, key: &str) -> Vec<String> {
    m.metadata
        .get(key)
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn set_or_remove(m: &mut Memory, key: &str, v: Option<Value>) {
    match v {
        Some(v) => {
            m.metadata.insert(key.into(), v);
        }
        None => {
            m.metadata.remove(key);
        }
    }
}

fn delivery_rank(s: Option<&str>) -> u8 {
    match s {
        Some(crate::memory::delivery::DONE) => 3,
        Some(crate::memory::delivery::SEEN) => 2,
        Some(crate::memory::delivery::DELIVERED) => 1,
        _ => 0,
    }
}

fn stamp(m: &Memory) -> Option<u64> {
    m.metadata.get(MNEMO_META_HLC).and_then(Value::as_u64)
}

fn version(m: &Memory) -> Option<(u64, u64)> {
    Some((
        stamp(m)?,
        m.metadata.get(MNEMO_META_VER).and_then(Value::as_u64)?,
    ))
}

/// decision.v0210-conflict-flags: one of the concurrent versions a record
/// combines: its stamp, its version hash, and a hash of the text a merge
/// compares for its kind (an essence item's, a task's or a finding's text;
/// all a user sees of anything else).
type Head = (u64, u64, u64);

fn group(m: &Memory) -> u64 {
    let h: [u8; 32] = match kind(m) {
        Kind::Essence | Kind::Task | Kind::Finding => {
            use sha2::{Digest, Sha256};
            Sha256::digest(m.content.as_bytes()).into()
        }
        _ => content_hash(m),
    };
    u64::from_be_bytes(h[..8].try_into().expect("8 bytes")) >> 2
}

/// A version as one head: an older build's record (no stamp, or no
/// version hash) by its number and the hash of what it holds.
fn own_head(m: &Memory) -> Head {
    (
        stamp(m).unwrap_or_else(|| number(m)),
        m.metadata
            .get(MNEMO_META_VER)
            .and_then(Value::as_u64)
            .unwrap_or_else(|| super::records::version_hash(m)),
        group(m),
    )
}

/// The versions a record combines: the list a merge left on it; for a fact
/// a merge only relinked or flagged, the write it holds (its earlier
/// version at its own stamp), so a later edit of that write covers it;
/// else the record's own.
fn heads(m: &Memory) -> BTreeSet<Head> {
    let listed: BTreeSet<Head> = m
        .metadata
        .get(MNEMO_META_MERGE_HEADS)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let e = e.as_array()?;
            Some((
                e.first()?.as_u64()?,
                e.get(1)?.as_u64()?,
                e.get(2)?.as_u64()?,
            ))
        })
        .collect();
    if !listed.is_empty() {
        return listed;
    }
    let own = own_head(m);
    if kind(m) == Kind::Fact && own.1 & super::records::VER_MERGED != 0 {
        let held: BTreeSet<Head> = super::records::hlc_prev(m)
            .into_iter()
            .filter(|p| p.0 == own.0 && p.1 & super::records::VER_MERGED == 0)
            .map(|p| (p.0, p.1, own.2))
            .collect();
        if !held.is_empty() {
            return held;
        }
    }
    BTreeSet::from([own])
}

fn heads_value(hs: &BTreeSet<Head>) -> Value {
    Value::Array(
        hs.iter()
            .map(|h| Value::Array(vec![h.0.into(), h.1.into(), h.2.into()]))
            .collect(),
    )
}

fn pair(h: &Head) -> (u64, u64) {
    (h.0, h.1)
}

/// decision.v0210-ancestry, with decision.v0210-conflict-flags: `y`
/// covers `x` when every version `x` combines is one `y` combines too or
/// one of `y`'s earlier versions. For two single versions, that's `x` being
/// `y`'s ancestor. Equal stamps with different content (an older build's
/// edit of both, or two copies in one millisecond) are never ancestry.
fn covers(y: &Memory, hy: &BTreeSet<Head>, hx: &BTreeSet<Head>) -> bool {
    let prev = super::records::hlc_prev(y);
    hx.iter()
        .all(|h| prev.contains(&pair(h)) || hy.iter().any(|k| pair(k) == pair(h)))
}

/// Two inputs' heads as one set, less each one the other input was edited
/// from (among its earlier versions, and not one of its own heads).
fn joined_heads(
    a: &Memory,
    ha: &BTreeSet<Head>,
    b: &Memory,
    hb: &BTreeSet<Head>,
) -> BTreeSet<Head> {
    let edited_from = |m: &Memory, hm: &BTreeSet<Head>| -> BTreeSet<(u64, u64)> {
        let own: BTreeSet<(u64, u64)> = hm.iter().map(pair).collect();
        super::records::hlc_prev(m)
            .into_iter()
            .filter(|p| !own.contains(p))
            .collect()
    };
    let (ea, eb) = (edited_from(a, ha), edited_from(b, hb));
    ha.iter()
        .filter(|h| !eb.contains(&pair(h)))
        .chain(hb.iter().filter(|h| !ea.contains(&pair(h))))
        .copied()
        .collect()
}

/// Whether the merge's flag on this kind follows its heads (two of them
/// with different text) rather than staying once set.
fn flag_follows_heads(m: &Memory) -> bool {
    matches!(kind(m), Kind::Essence | Kind::Task | Kind::Finding)
}

/// An essence item's, a task's or a finding's flag from the versions it
/// combines: set while two hold different text, or while a merge's rename
/// stands; else cleared. Returns whether it's set.
fn set_flag(body: &mut Memory, hs: &BTreeSet<Head>) -> bool {
    let texts: BTreeSet<u64> = hs.iter().map(|h| h.2).collect();
    let on = texts.len() >= 2 || body.metadata.contains_key(MNEMO_META_MERGE_NAME);
    set_or_remove(
        body,
        MNEMO_META_MERGE_CONFLICT,
        on.then_some(Value::Bool(true)),
    );
    on
}

/// The merged record's version: an input's when the merge left it as that
/// input had it; else one of the merge's own, named by its hash and marked
/// as made by a merge.
fn name_version(a: &Memory, b: &Memory, body: &mut Memory) {
    if stamp(body).is_none() {
        return;
    }
    let same = [a, b]
        .into_iter()
        .find(|m| core_bytes(m) == core_bytes(body));
    let v = match same {
        Some(m) => m.metadata.get(MNEMO_META_VER).cloned(),
        None => Some(Value::from(
            super::records::version_hash(body) | super::records::VER_MERGED,
        )),
    };
    set_or_remove(body, MNEMO_META_VER, v);
}

/// A conflict copy of `source` holding `cand`'s text, rebuilt from what a
/// user sees of it and the versions it holds (`list`), so it reads the same
/// whichever merge made it.
fn build_copy(source: u128, cand: &Memory, list: &BTreeSet<Head>, src: Option<&Memory>) -> Memory {
    let mut c = cand.clone();
    c.id = Ulid(copy_id(source, &content_hash(cand)));
    c.vector = rounded(&cand.vector);
    c.metadata = visible_metadata(cand);
    let top = list.iter().max_by_key(|h| pair(h)).expect("one version");
    c.metadata.insert(MNEMO_META_HLC.into(), Value::from(top.0));
    c.metadata.insert(MNEMO_META_VER.into(), Value::from(top.1));
    c.metadata.insert(
        MNEMO_META_UPDATED_AT.into(),
        Value::from((top.0 / 1000) as i64),
    );
    c.metadata
        .insert(MNEMO_META_MERGE_CONFLICT.into(), Value::Bool(true));
    c.metadata.insert(
        MNEMO_META_MERGE_SOURCE.into(),
        Value::String(Ulid(source).to_string()),
    );
    c.metadata
        .insert(MNEMO_META_MERGE_HEADS.into(), heads_value(list));
    if let Some(s) = src {
        for k in [MNEMO_META_SUPERSEDED_BY, MNEMO_META_INVALID_AT] {
            set_or_remove(&mut c, k, s.metadata.get(k).cloned());
        }
    }
    c.accessed_at = c.created_at;
    c.access_count = 0;
    c
}

/// A conflict copy as a merge made it (its id still names its text).
fn pristine_copy(m: &Memory) -> bool {
    kind(m) == Kind::Fact
        && ulid_at(m, MNEMO_META_MERGE_SOURCE)
            .is_some_and(|s| m.id.0 == copy_id(s, &content_hash(m)))
}

/// A conflict flag stays once set, whichever version wins.
fn keep_flag(a: &Memory, b: &Memory, body: &mut Memory) {
    if [a, b]
        .iter()
        .any(|m| m.metadata.contains_key(MNEMO_META_MERGE_CONFLICT))
    {
        body.metadata
            .insert(MNEMO_META_MERGE_CONFLICT.into(), Value::Bool(true));
    }
}

/// A merged record's earlier versions: the [`HLC_PREV_KEEP`] largest of
/// both inputs' versions and lists, less its own.
fn merged_prev(a: &Memory, b: &Memory, body: &mut Memory) {
    let own = version(body);
    let mut all = super::records::hlc_prev(a);
    all.extend(super::records::hlc_prev(b));
    for m in [a, b] {
        all.extend(version(m).filter(|(_, v)| v & super::records::VER_MERGED == 0));
    }
    all.retain(|v| Some(*v) != own);
    super::records::set_hlc_prev(body, all);
}

/// What resolving one id found.
struct Resolved {
    body: Memory,
    copy: Option<Memory>,
    conflict: Option<&'static str>,
}

/// Rule (3): the same id in both inputs.
fn resolve(a: &Memory, b: &Memory) -> Resolved {
    let a_wins = wins(a, b);
    let (w, l) = if a_wins { (a, b) } else { (b, a) };
    let mut body = w.clone();
    body.accessed_at = a.accessed_at.max(b.accessed_at);
    body.access_count = a.access_count.max(b.access_count);
    if record_bytes(a) == record_bytes(b) {
        return Resolved {
            body,
            copy: None,
            conflict: None,
        };
    }
    // One covers the other (a one-sided edit or move, or a record edited
    // from every version the other combines): it wins whole, with no copy
    // and no new flag. The other's flag goes with it, since the winner
    // descends from every version that one combines.
    let (ha, hb) = (heads(a), heads(b));
    let (a_covers, b_covers) = (covers(a, &ha, &hb), covers(b, &hb, &ha));
    if a_covers != b_covers {
        let d = if b_covers { b } else { a };
        let mut body = d.clone();
        body.accessed_at = a.accessed_at.max(b.accessed_at);
        body.access_count = a.access_count.max(b.access_count);
        if flag_follows_heads(d) {
            set_flag(&mut body, &heads(d));
        }
        name_version(a, b, &mut body);
        merged_prev(a, b, &mut body);
        return Resolved {
            body,
            copy: None,
            conflict: None,
        };
    }
    // A record the merge changes keeps the larger of its two stamps.
    let hlc = [a, b]
        .iter()
        .filter_map(|m| m.metadata.get(MNEMO_META_HLC).and_then(Value::as_u64))
        .max();
    if let Some(h) = hlc {
        body.metadata.insert(MNEMO_META_HLC.into(), Value::from(h));
    }
    // And the later of its two update times, whichever body it took (two
    // with one number and one text tie on bytes the merge itself changes).
    let updated = [a, b]
        .iter()
        .filter_map(|m| {
            m.metadata
                .get(MNEMO_META_UPDATED_AT)
                .and_then(Value::as_i64)
        })
        .max();
    if let Some(u) = updated {
        body.metadata
            .insert(MNEMO_META_UPDATED_AT.into(), Value::from(u));
    }
    let mut copy = None;
    let mut conflict = None;
    match kind(w) {
        Kind::Fact => {
            // Unkeyed supersession: a mark from either input stays; two
            // different marks: the one on the larger number's version.
            if !w.metadata.contains_key(MNEMO_META_KEY) {
                let mark = |m: &Memory| {
                    m.metadata
                        .get(MNEMO_META_SUPERSEDED_BY)
                        .cloned()
                        .map(|v| (v, m.metadata.get(MNEMO_META_INVALID_AT).cloned()))
                };
                if let Some((by, at)) = mark(w).or_else(|| mark(l)) {
                    body.metadata.insert(MNEMO_META_SUPERSEDED_BY.into(), by);
                    set_or_remove(&mut body, MNEMO_META_INVALID_AT, at);
                }
            }
            if !same_visible(a, b) {
                let mut c = l.clone();
                c.id = Ulid(copy_id(l.id.0, &content_hash(l)));
                // A copy is a new record: the versions its source had
                // before aren't its own (and differ by grouping).
                for k in [MNEMO_META_KEY, MNEMO_META_SUPERSEDES, MNEMO_META_HLC_PREV] {
                    c.metadata.remove(k);
                }
                for k in [MNEMO_META_SUPERSEDED_BY, MNEMO_META_INVALID_AT] {
                    set_or_remove(&mut c, k, body.metadata.get(k).cloned());
                }
                c.metadata
                    .insert(MNEMO_META_MERGE_CONFLICT.into(), Value::Bool(true));
                c.metadata.insert(
                    MNEMO_META_MERGE_SOURCE.into(),
                    Value::String(l.id.to_string()),
                );
                copy = Some(c);
                conflict = Some("fact");
            }
        }
        Kind::Essence => {
            let resolved = crate::memory::status::RESOLVED;
            let st = |m: &Memory| meta_str(m, MNEMO_META_STATUS).map(str::to_string);
            let status = if st(a).as_deref() == Some(resolved) || st(b).as_deref() == Some(resolved)
            {
                Some(Value::String(resolved.into()))
            } else {
                st(w).map(Value::String)
            };
            set_or_remove(&mut body, MNEMO_META_STATUS, status);
            put_history(&mut body, a, b, union_history(a, b));
            conflict = Some("essence");
        }
        Kind::Task | Kind::Finding => {
            let finding = kind(w) == Kind::Finding;
            let fin = if finding {
                finding_status::VERIFIED
            } else {
                task_status::DONE
            };
            let st = |m: &Memory| meta_str(m, MNEMO_META_STATUS).map(str::to_string);
            let history = union_history(a, b);
            let status = if st(a).as_deref() == Some(fin) || st(b).as_deref() == Some(fin) {
                Some(fin.to_string())
            } else {
                history
                    .iter()
                    .rev()
                    .find_map(|e| e.get("to").and_then(Value::as_str).map(str::to_string))
                    .or_else(|| st(w))
            };
            if !finding {
                let owner = if status.as_deref() == Some(task_status::QUEUED) {
                    None
                } else {
                    history
                        .iter()
                        .rev()
                        .find(|e| e.get("to").and_then(Value::as_str) == Some(task_status::ACTIVE))
                        .and_then(|e| e.get("by").cloned())
                        .or_else(|| w.metadata.get(MNEMO_META_OWNER).cloned())
                };
                set_or_remove(&mut body, MNEMO_META_OWNER, owner);
            }
            if finding && meta_str(w, crate::memory::MNEMO_META_CATEGORY) == Some("SECURITY") {
                let vis = if status.as_deref() == Some(finding_status::VERIFIED) {
                    "team"
                } else {
                    "private"
                };
                body.metadata.insert(
                    crate::memory::MNEMO_META_VISIBILITY.into(),
                    Value::String(vis.into()),
                );
            }
            set_or_remove(&mut body, MNEMO_META_STATUS, status.map(Value::String));
            put_history(&mut body, a, b, history);
            conflict = Some(if finding { "finding" } else { "task" });
        }
        Kind::Message => {
            let d = [a, b]
                .iter()
                .map(|m| meta_str(m, MNEMO_META_DELIVERY))
                .max_by_key(|d| delivery_rank(*d))
                .flatten()
                .map(|s| Value::String(s.to_string()));
            set_or_remove(&mut body, MNEMO_META_DELIVERY, d);
            for key in [MNEMO_META_SEEN_BY, MNEMO_META_READY_SEEN_BY] {
                let all: BTreeSet<String> = string_list(a, key)
                    .into_iter()
                    .chain(string_list(b, key))
                    .collect();
                let v = (!all.is_empty())
                    .then(|| Value::Array(all.into_iter().map(Value::String).collect()));
                set_or_remove(&mut body, key, v);
            }
            let acked = crate::memory::MESSAGE_ACKED;
            let acked_at = [a, b]
                .iter()
                .filter(|m| meta_str(m, MNEMO_META_STATUS) == Some(acked))
                .filter_map(|m| m.metadata.get(MNEMO_META_ACKED_AT).and_then(Value::as_i64))
                .min();
            if [a, b]
                .iter()
                .any(|m| meta_str(m, MNEMO_META_STATUS) == Some(acked))
            {
                body.metadata
                    .insert(MNEMO_META_STATUS.into(), Value::String(acked.into()));
                set_or_remove(&mut body, MNEMO_META_ACKED_AT, acked_at.map(Value::from));
            }
            put_history(&mut body, a, b, union_history(a, b));
        }
        Kind::Manifest => {
            // A later merge mustn't claim to know deletes neither input
            // knew: the later first stamp, and none when either lacks one.
            let since = |m: &Memory| m.metadata.get(MNEMO_META_HLC_SINCE).and_then(Value::as_u64);
            let v = since(a).zip(since(b)).map(|(x, y)| Value::from(x.max(y)));
            set_or_remove(&mut body, MNEMO_META_HLC_SINCE, v);
        }
        Kind::Turn | Kind::Local | Kind::Tombstone | Kind::Engine => {}
    }
    // The concurrent versions this one combines. An essence item's, a
    // task's or a finding's flag follows them: set while two hold different
    // text (decision.v0210-conflict-flags). Others' flags stay once set.
    let hs = joined_heads(a, &ha, b, &hb);
    if flag_follows_heads(w) {
        if !set_flag(&mut body, &hs) {
            conflict = None;
        }
    } else {
        keep_flag(a, b, &mut body);
    }
    set_or_remove(
        &mut body,
        MNEMO_META_MERGE_HEADS,
        (hs.len() >= 2).then(|| heads_value(&hs)),
    );
    name_version(a, b, &mut body);
    merged_prev(a, b, &mut body);
    Resolved {
        body,
        copy,
        conflict,
    }
}

/// One input after rule (0) and the delete rule.
struct Side {
    by_id: BTreeMap<u128, Memory>,
    /// Ids this input ever held: live, deleted, or named by its tombstones.
    known: HashSet<u128>,
}

fn turn_of(m: &Memory) -> Option<(String, String)> {
    Some((
        meta_str(m, MNEMO_META_THREAD)?.to_string(),
        meta_str(m, MNEMO_META_TURN_ID)?.to_string(),
    ))
}

/// The pure rules: the output's records and the report (its `lead_ms` and
/// `hlc_since` from `now_ms` and the inputs' manifests).
pub(crate) fn merge_core(a: MergeInput, b: MergeInput, now_ms: u64) -> (Vec<Memory>, MergeReport) {
    let now_s = (now_ms / 1000) as i64;
    let mut report = MergeReport::default();
    let inputs = [a, b];
    for (i, input) in inputs.iter().enumerate() {
        report.lead_ms[i] = input
            .records
            .iter()
            .map(number)
            .max()
            .map_or(i64::MIN, |n| n as i64 - now_ms as i64);
        if report.lead_ms[i] == i64::MIN {
            report.lead_ms[i] = 0;
        }
        report.hlc_since[i] = input
            .records
            .iter()
            .filter(|m| {
                kind(m) == Kind::Manifest && !m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY)
            })
            .filter_map(|m| m.metadata.get(MNEMO_META_HLC_SINCE).and_then(Value::as_u64))
            .min();
    }

    // (0) What compaction drops.
    let mut kept: [Vec<Memory>; 2] = [Vec::new(), Vec::new()];
    let mut dropped: BTreeSet<u128> = BTreeSet::new();
    for (i, input) in inputs.iter().enumerate() {
        for m in &input.records {
            if m.is_expired(now_s) || super::coordination::compact_drops(m, now_s) {
                dropped.insert(m.id.0);
            } else {
                kept[i].push(m.clone());
            }
        }
    }
    report.dropped = dropped.len();

    // (1) Deletes first, by id, with no clock.
    let mut removed: BTreeSet<u128> = BTreeSet::new();
    let mut removed_turns: BTreeSet<(String, String)> = BTreeSet::new();
    // Turns an input holds beside a tombstone of their turn: saved again
    // after the forget, which named only what it saw.
    let mut saved_again: HashSet<u128> = HashSet::new();
    let mut known: [HashSet<u128>; 2] = [HashSet::new(), HashSet::new()];
    let mut named: [Vec<&Memory>; 2] = [Vec::new(), Vec::new()];
    for i in 0..2 {
        let live: HashSet<u128> = kept[i].iter().map(|m| m.id.0).collect();
        let live_turns: HashSet<(String, String)> = kept[i]
            .iter()
            .filter(|m| kind(m) == Kind::Turn)
            .filter_map(turn_of)
            .collect();
        known[i].extend(live.iter().copied());
        for d in &inputs[i].deleted {
            known[i].insert(d.0);
            if !live.contains(&d.0) {
                removed.insert(d.0);
            }
        }
        for t in kept[i].iter().filter(|m| kind(m) == Kind::Tombstone) {
            let Some(x) = ulid_at(t, MNEMO_META_TOMBSTONE_OF) else {
                continue;
            };
            known[i].insert(x);
            if live.contains(&x) {
                continue; // written again after the delete: void
            }
            removed.insert(x);
            named[i].push(t);
            if let (Some(th), Some(tu)) = (
                meta_str(t, MNEMO_META_TOMBSTONE_THREAD),
                meta_str(t, MNEMO_META_TOMBSTONE_TURN),
            ) {
                // The forget takes every copy of the turn, in any input;
                // only the copies this input holds beside its tombstone,
                // saved after the forget, stay.
                let turn = (th.to_string(), tu.to_string());
                if live_turns.contains(&turn) {
                    saved_again.extend(
                        kept[i]
                            .iter()
                            .filter(|m| kind(m) == Kind::Turn && turn_of(m).as_ref() == Some(&turn))
                            .map(|m| m.id.0),
                    );
                }
                removed_turns.insert(turn);
            }
        }
    }
    let gone = |m: &Memory| {
        removed.contains(&m.id.0)
            || ulid_at(m, MNEMO_META_MERGE_SOURCE).is_some_and(|s| removed.contains(&s))
            || (kind(m) == Kind::Turn
                && !saved_again.contains(&m.id.0)
                && turn_of(m).is_some_and(|t| removed_turns.contains(&t)))
    };
    let mut gone_ids: BTreeSet<u128> = BTreeSet::new();
    let mut sides: Vec<Side> = Vec::new();
    for (i, records) in kept.iter().enumerate() {
        let mut by_id = BTreeMap::new();
        for m in records {
            if kind(m) != Kind::Tombstone && gone(m) {
                gone_ids.insert(m.id.0);
            } else {
                by_id.insert(m.id.0, m.clone());
            }
        }
        sides.push(Side {
            by_id,
            known: known[i].clone(),
        });
    }
    report.removed = gone_ids.iter().map(|&u| Ulid(u)).collect();

    // Keys deleted in one input and written in the other, in key order
    // (so the report reads alike whichever input is first).
    let mut kept_keys: BTreeMap<(Option<String>, String, MergeSide), BTreeSet<u128>> =
        BTreeMap::new();
    for (i, side) in [MergeSide::A, MergeSide::B].into_iter().enumerate() {
        let other = &sides[1 - i];
        for t in &named[i] {
            let Some(key) = meta_str(t, MNEMO_META_TOMBSTONE_KEY) else {
                continue;
            };
            let owner = meta_str(t, MNEMO_META_TOMBSTONE_OWNER).map(str::to_string);
            for m in other.by_id.values() {
                let Some(k) = meta_str(m, MNEMO_META_KEY) else {
                    continue;
                };
                let slot = Mnemo::key_slot_for(m, k);
                if slot.key == key && slot.scope_owner == owner && !sides[i].known.contains(&m.id.0)
                {
                    kept_keys
                        .entry((owner.clone(), key.to_string(), side))
                        .or_default()
                        .insert(m.id.0);
                }
            }
        }
    }
    report.kept_keys = kept_keys
        .into_iter()
        .map(|((owner, key, deleted_in), ids)| MergeKeptKey {
            key,
            owner,
            deleted_in,
            kept: ids.into_iter().map(Ulid).collect(),
        })
        .collect();

    // (3) One record per id.
    let mut out: BTreeMap<u128, Memory> = BTreeMap::new();
    let mut conflicts: BTreeMap<String, usize> = BTreeMap::new();
    let mut conflict_ids: BTreeSet<u128> = BTreeSet::new();
    let mut copies: Vec<Memory> = Vec::new();
    let ids: BTreeSet<u128> = sides[0]
        .by_id
        .keys()
        .chain(sides[1].by_id.keys())
        .copied()
        .collect();
    for id in ids {
        match (sides[0].by_id.get(&id), sides[1].by_id.get(&id)) {
            (Some(x), Some(y)) => {
                report.both += 1;
                if pristine_copy(x) && pristine_copy(y) {
                    copies.extend([x.clone(), y.clone()]);
                    continue;
                }
                if kind(x) == Kind::Local {
                    out.insert(id, x.clone());
                    continue;
                }
                let r = resolve(x, y);
                if let Some(c) = r.conflict {
                    *conflicts.entry(c.to_string()).or_default() += 1;
                    match &r.copy {
                        Some(cp) => conflict_ids.insert(cp.id.0),
                        None => conflict_ids.insert(id),
                    };
                }
                if let Some(c) = r.copy {
                    copies.push(c);
                }
                out.insert(id, r.body);
            }
            (Some(x), None) => {
                if kind(x) != Kind::Local {
                    report.a_only.push(Ulid(id));
                }
                if pristine_copy(x) {
                    copies.push(x.clone());
                    continue;
                }
                out.insert(id, x.clone());
            }
            (None, Some(y)) => {
                // Claims and presence: the first input's only.
                if kind(y) == Kind::Local {
                    continue;
                }
                report.b_only.push(Ulid(id));
                if pristine_copy(y) {
                    copies.push(y.clone());
                    continue;
                }
                out.insert(id, y.clone());
            }
            (None, None) => {}
        }
    }
    // Conflict copies (decision.v0210-conflict-flags): for each source,
    // one per text among the versions it combines, other than its own, and
    // each holding those versions; a text whose versions the source was
    // edited from has none. The inputs' copies and this merge's new ones
    // are only where the text comes from.
    let mut cands: BTreeMap<u128, BTreeMap<u64, Vec<Memory>>> = BTreeMap::new();
    for c in copies {
        if let Some(s) = ulid_at(&c, MNEMO_META_MERGE_SOURCE) {
            cands
                .entry(s)
                .or_default()
                .entry(group(&c))
                .or_default()
                .push(c);
        }
    }
    for (s, by_text) in cands {
        let src = out.get(&s).cloned();
        let hs = src.as_ref().map(heads);
        let own = src.as_ref().map(group);
        for (g, cs) in by_text {
            let list: BTreeSet<Head> = match &hs {
                Some(hs) => hs.iter().filter(|h| h.2 == g).copied().collect(),
                // A copy whose source isn't here: what its copies hold.
                None => cs.iter().flat_map(heads).collect(),
            };
            if own == Some(g) || list.is_empty() {
                continue;
            }
            let c = build_copy(s, &cs[0], &list, src.as_ref());
            if !removed.contains(&c.id.0) {
                out.insert(c.id.0, c);
            }
        }
    }

    relink_slots(&mut out, &sides, &mut conflicts, &mut conflict_ids);
    report.renamed = rename_names(&mut out, &sides, &mut conflict_ids);
    report.after_cycles = after_cycles(&out);
    keep_one_turn_copy(&mut out, &sides);
    keep_one_tombstone(&mut out);
    name_final_versions(&mut out, &sides);

    report.a_only.retain(|u| out.contains_key(&u.0));
    report.b_only.retain(|u| out.contains_key(&u.0));
    report.merged = out.values().filter(|m| kind(m) != Kind::Local).count();
    report.conflicts = conflicts;
    report.conflict_ids = conflict_ids
        .into_iter()
        .filter(|u| out.contains_key(u))
        .map(Ulid)
        .collect();
    (out.into_values().collect(), report)
}

/// The version a key's version was written over: its own link, unless a
/// merge set that one (decision.v0210-chain-rule).
fn written_over(m: &Memory) -> Option<u128> {
    match m.metadata.get(MNEMO_META_MERGE_LINKED) {
        Some(v) => v
            .as_str()
            .and_then(|s| Ulid::from_string(s).ok())
            .map(|u| u.0),
        None => ulid_at(m, MNEMO_META_SUPERSEDES),
    }
}

/// A key slot's versions as one chain, from what was written, never from
/// links a merge made (decision.v0210-chain-rule, as Git recomputes from
/// its commits): so the chain is the same whichever copies were merged
/// first. The versions go by the number each was first written with (a
/// write's stamp is past the one it supersedes), then created_at, then id.
/// Each keeps the link its write made, to the version before it or to a
/// deleted one between them; where the version before ended in a forgotten
/// version and nothing written follows it, the chain stays broken there, as
/// the engine leaves it; anywhere else the merge links the two and marks
/// the later with what it was written over (`_mnemo.merge_linked`). A
/// deleted version's own link is on its tombstone.
fn relink_slots(
    out: &mut BTreeMap<u128, Memory>,
    sides: &[Side],
    conflicts: &mut BTreeMap<String, usize>,
    conflict_ids: &mut BTreeSet<u128>,
) {
    let mut slots: BTreeMap<(Option<String>, String), Vec<u128>> = BTreeMap::new();
    for (id, m) in out.iter() {
        if kind(m) != Kind::Fact || m.metadata.contains_key(MNEMO_META_MERGE_SOURCE) {
            continue;
        }
        if let Some(k) = meta_str(m, MNEMO_META_KEY) {
            let s = Mnemo::key_slot_for(m, k);
            slots.entry((s.scope_owner, s.key)).or_default().push(*id);
        }
    }
    // What each deleted version was written over: its tombstone's record of
    // it; for an older tombstone, an input's copy of the version; else the
    // version an input links to it from.
    let mut gone_over: BTreeMap<u128, Option<u128>> = BTreeMap::new();
    for m in out.values() {
        if kind(m) != Kind::Tombstone {
            continue;
        }
        let (Some(t), Some(v)) = (
            ulid_at(m, MNEMO_META_TOMBSTONE_OF),
            m.metadata.get(MNEMO_META_TOMBSTONE_SUPERSEDES),
        ) else {
            continue;
        };
        if !out.contains_key(&t) {
            let over = v
                .as_str()
                .and_then(|s| Ulid::from_string(s).ok())
                .map(|u| u.0);
            gone_over.insert(t, over);
        }
    }
    let mut linked_from: BTreeMap<u128, u128> = BTreeMap::new();
    for side in sides {
        for m in side.by_id.values() {
            if let Some(t) = ulid_at(m, MNEMO_META_SUPERSEDED_BY) {
                if !out.contains_key(&t) {
                    let e = linked_from.entry(t).or_insert(m.id.0);
                    *e = (*e).min(m.id.0);
                }
            }
        }
    }
    for (&t, &p) in &linked_from {
        if let std::collections::btree_map::Entry::Vacant(e) = gone_over.entry(t) {
            e.insert(
                sides
                    .iter()
                    .find_map(|s| s.by_id.get(&t))
                    .map_or(Some(p), written_over),
            );
        }
    }
    for members in slots.into_values() {
        let member_set: BTreeSet<u128> = members.iter().copied().collect();
        type Links = BTreeMap<u128, (Option<u128>, Option<u128>, Option<u128>, i64)>;
        let (order, links, heads): (Vec<u128>, Links, Vec<u128>) = {
            let gone = |n: &u128| !out.contains_key(n);
            // What a version was written over, within the slot: a link to a
            // record of another kind or slot is left out.
            let over_of = |n: u128| -> Option<u128> {
                let w = match out.get(&n) {
                    Some(m) => written_over(m),
                    None => gone_over.get(&n).copied().flatten(),
                };
                w.filter(|t| member_set.contains(t) || gone(t))
            };
            let gone_after = |p: u128| -> Vec<u128> {
                gone_over
                    .iter()
                    .filter(|(t, o)| **o == Some(p) && gone(t))
                    .map(|(t, _)| *t)
                    .collect()
            };
            // Whether a version written after `p` (through deleted ones) is in
            // the output.
            let continues = |p: u128| -> bool {
                let mut todo = vec![p];
                let mut seen = BTreeSet::new();
                while let Some(x) = todo.pop() {
                    if !seen.insert(x) {
                        continue;
                    }
                    if members.iter().any(|&m| m != p && over_of(m) == Some(x)) {
                        return true;
                    }
                    todo.extend(gone_after(x));
                }
                false
            };
            // The deleted version right after `p` on the way back from `t`,
            // when `t` was written after `p` through deleted versions only.
            let gone_path = |p: u128, t: u128| -> Option<u128> {
                let mut x = t;
                for _ in 0..=gone_over.len() {
                    match over_of(x) {
                        Some(y) if y == p => return Some(x),
                        Some(y) if gone(&y) => x = y,
                        _ => return None,
                    }
                }
                None
            };
            let ended = |p: u128| -> Option<u128> {
                let after = gone_after(p);
                if after.is_empty() || continues(p) {
                    None
                } else {
                    after.into_iter().min()
                }
            };
            // A version is placed by the number it was first written with,
            // which a later write superseding it doesn't change.
            let key_of = |n: u128| -> (u64, i64, u128) {
                let m = &out[&n];
                let first = m
                    .metadata
                    .get(MNEMO_META_FIRST_HLC)
                    .and_then(Value::as_u64)
                    .unwrap_or_else(|| {
                        super::records::hlc_prev(m)
                            .into_iter()
                            .map(|p| p.0)
                            .fold(number(m), u64::min)
                    });
                (first, m.created_at, n)
            };
            let mut order = members.clone();
            order.sort_by_key(|&n| key_of(n));
            // Each version's two links.
            let mut before: BTreeMap<u128, Option<u128>> = BTreeMap::new();
            let mut after: BTreeMap<u128, Option<u128>> = BTreeMap::new();
            for (pos, &n) in order.iter().enumerate() {
                let w = over_of(n);
                let w_gone = w.filter(|t| gone(t));
                let Some(p) = pos.checked_sub(1).map(|i| order[i]) else {
                    before.insert(n, w_gone);
                    continue;
                };
                if w == Some(p) {
                    before.insert(n, Some(p));
                    after.insert(p, Some(n));
                } else if let Some(x) = w_gone.and_then(|t| gone_path(p, t)) {
                    before.insert(n, w_gone);
                    after.insert(p, Some(x));
                } else if let Some(t) = ended(p) {
                    before.insert(n, w_gone);
                    after.insert(p, Some(t));
                } else {
                    before.insert(n, Some(p));
                    after.insert(p, Some(n));
                }
            }
            if let Some(&last) = order.last() {
                after.insert(last, ended(last));
            }
            let created_secs = |n: u128| -> i64 {
                out.get(&n)
                    .map_or_else(|| ((n >> 80) as u64 / 1000) as i64, |m| m.created_at)
            };
            let links = order
                .iter()
                .map(|&n| {
                    let next = after.get(&n).copied().flatten();
                    (
                        n,
                        (
                            over_of(n),
                            before.get(&n).copied().flatten(),
                            next,
                            next.map_or(0, created_secs),
                        ),
                    )
                })
                .collect();
            // The versions nothing written follows (no version, kept or
            // deleted, was written over them).
            let heads = order
                .iter()
                .copied()
                .filter(|&n| {
                    gone_after(n).is_empty() && !members.iter().any(|&m| over_of(m) == Some(n))
                })
                .collect();
            (order, links, heads)
        };
        for &n in &order {
            let (w, prev, next, next_created) = links[&n];
            let m = out.get_mut(&n).expect("a member");
            set_or_remove(
                m,
                MNEMO_META_SUPERSEDES,
                prev.map(|p| Value::String(Ulid(p).to_string())),
            );
            let mark =
                (prev != w).then(|| w.map_or(Value::Null, |t| Value::String(Ulid(t).to_string())));
            set_or_remove(m, MNEMO_META_MERGE_LINKED, mark);
            match next {
                None => {
                    m.metadata.remove(MNEMO_META_SUPERSEDED_BY);
                    m.metadata.remove(MNEMO_META_INVALID_AT);
                }
                Some(x) => {
                    let same = ulid_at(m, MNEMO_META_SUPERSEDED_BY) == Some(x)
                        && m.metadata.contains_key(MNEMO_META_INVALID_AT);
                    m.metadata.insert(
                        MNEMO_META_SUPERSEDED_BY.into(),
                        Value::String(Ulid(x).to_string()),
                    );
                    if !same {
                        m.metadata
                            .insert(MNEMO_META_INVALID_AT.into(), Value::from(next_created));
                    }
                }
            }
        }
        // REV-017.1 (A): the key's conflict flags come from what was
        // written, so every grouping flags alike. When the versions nothing
        // written follows hold different text, each of them but the chain's
        // last is flagged; no other version of the key is.
        let tail = order.last().copied();
        let differ = heads
            .iter()
            .any(|x| heads.iter().any(|y| !same_visible(&out[x], &out[y])));
        let flagged: BTreeSet<u128> = if differ {
            heads.iter().copied().filter(|h| Some(*h) != tail).collect()
        } else {
            BTreeSet::new()
        };
        let mut new_flags = Vec::new();
        for &n in &order {
            let m = out.get_mut(&n).expect("a member");
            if flagged.contains(&n) {
                m.metadata
                    .insert(MNEMO_META_MERGE_CONFLICT.into(), Value::Bool(true));
                let had = sides.iter().any(|s| {
                    s.by_id
                        .get(&n)
                        .is_some_and(|v| v.metadata.contains_key(MNEMO_META_MERGE_CONFLICT))
                });
                if !had {
                    new_flags.push(n);
                }
            } else {
                m.metadata.remove(MNEMO_META_MERGE_CONFLICT);
            }
        }
        // The report counts a key this merge flagged anew.
        if !new_flags.is_empty() {
            *conflicts.entry("key".into()).or_default() += 1;
            conflict_ids.extend(new_flags);
        }
    }
}

/// One task per slug and one finding per id: the earliest id keeps the
/// name; any other is renamed `<name>-merge-2` (then -3, ...), flagged, and
/// remembers the name in `_mnemo.merge_name`.
fn rename_names(
    out: &mut BTreeMap<u128, Memory>,
    sides: &[Side],
    conflict_ids: &mut BTreeSet<u128>,
) -> Vec<MergeRename> {
    let mut renamed = Vec::new();
    for (k, field, label) in [
        (Kind::Task, MNEMO_META_SLUG, "task"),
        (Kind::Finding, MNEMO_META_FINDING_ID, "finding"),
    ] {
        let mut groups: BTreeMap<String, Vec<u128>> = BTreeMap::new();
        for (id, m) in out.iter() {
            if kind(m) != k {
                continue;
            }
            let canon = meta_str(m, MNEMO_META_MERGE_NAME)
                .or_else(|| meta_str(m, field))
                .unwrap_or("")
                .to_string();
            groups.entry(canon).or_default().push(*id);
        }
        let mut used: BTreeSet<String> = groups.keys().cloned().collect();
        for (canon, ids) in &groups {
            let mut n = 2;
            for (i, id) in ids.iter().enumerate() {
                let m = out.get_mut(id).expect("a record");
                if i == 0 {
                    m.metadata
                        .insert(field.into(), Value::String(canon.clone()));
                    // A record a merge renamed takes its name back: the
                    // rename no longer stands, so its flag follows only
                    // the versions it combines, as a later merge sees it.
                    if m.metadata.remove(MNEMO_META_MERGE_NAME).is_some() {
                        let hs = heads(m);
                        if !set_flag(m, &hs) {
                            conflict_ids.remove(id);
                        }
                    }
                    continue;
                }
                let name = loop {
                    let c = format!("{canon}-merge-{n}");
                    n += 1;
                    if !used.contains(&c) {
                        break c;
                    }
                };
                used.insert(name.clone());
                let was: BTreeSet<String> = sides
                    .iter()
                    .filter_map(|s| s.by_id.get(id))
                    .filter_map(|v| meta_str(v, field).map(str::to_string))
                    .collect();
                m.metadata.insert(field.into(), Value::String(name.clone()));
                m.metadata
                    .insert(MNEMO_META_MERGE_NAME.into(), Value::String(canon.clone()));
                m.metadata
                    .insert(MNEMO_META_MERGE_CONFLICT.into(), Value::Bool(true));
                conflict_ids.insert(*id);
                if was.iter().any(|w| *w != name) {
                    renamed.push(MergeRename {
                        kind: label.into(),
                        from: canon.clone(),
                        to: name,
                        id: Ulid(*id),
                    });
                }
            }
        }
    }
    renamed.sort();
    renamed
}

/// The tasks' `after` cycles, each as its slugs from the smallest.
fn after_cycles(out: &BTreeMap<u128, Memory>) -> Vec<Vec<String>> {
    let mut after: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for m in out.values().filter(|m| kind(m) == Kind::Task) {
        if let Some(slug) = meta_str(m, MNEMO_META_SLUG) {
            after.insert(
                slug.to_string(),
                string_list(m, crate::memory::MNEMO_META_AFTER),
            );
        }
    }
    // Tarjan's strongly connected components; a component of two or more,
    // or one that names itself, is a cycle.
    struct T<'a> {
        g: &'a BTreeMap<String, Vec<String>>,
        index: BTreeMap<&'a str, usize>,
        low: BTreeMap<&'a str, usize>,
        stack: Vec<&'a str>,
        on: BTreeSet<&'a str>,
        n: usize,
        out: Vec<Vec<String>>,
    }
    fn visit<'a>(t: &mut T<'a>, v: &'a str) {
        t.index.insert(v, t.n);
        t.low.insert(v, t.n);
        t.n += 1;
        t.stack.push(v);
        t.on.insert(v);
        let next: Vec<&'a str> =
            t.g.get(v)
                .map(|n| {
                    n.iter()
                        .map(String::as_str)
                        .filter(|w| t.g.contains_key(*w))
                        .collect()
                })
                .unwrap_or_default();
        for w in next {
            if !t.index.contains_key(w) {
                visit(t, w);
                let lw = t.low[w];
                let lv = t.low.get_mut(v).expect("visited");
                *lv = (*lv).min(lw);
            } else if t.on.contains(w) {
                let iw = t.index[w];
                let lv = t.low.get_mut(v).expect("visited");
                *lv = (*lv).min(iw);
            }
        }
        if t.low[v] == t.index[v] {
            let mut comp = Vec::new();
            while let Some(w) = t.stack.pop() {
                t.on.remove(w);
                comp.push(w.to_string());
                if w == v {
                    break;
                }
            }
            let self_loop = t.g.get(v).is_some_and(|n| n.iter().any(|x| x == v));
            if comp.len() > 1 || self_loop {
                comp.sort();
                t.out.push(comp);
            }
        }
    }
    let mut t = T {
        g: &after,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        stack: Vec::new(),
        on: BTreeSet::new(),
        n: 0,
        out: Vec::new(),
    };
    for v in after.keys() {
        if !t.index.contains_key(v.as_str()) {
            visit(&mut t, v);
        }
    }
    let mut cycles = t.out;
    cycles.sort();
    cycles
}

/// A turn saved in both inputs under different ids: every part from the
/// input whose first part has the earlier id.
fn keep_one_turn_copy(out: &mut BTreeMap<u128, Memory>, sides: &[Side]) {
    let mut turns: BTreeMap<(String, String), Vec<u128>> = BTreeMap::new();
    for (id, m) in out.iter() {
        if kind(m) == Kind::Turn {
            if let Some(t) = turn_of(m) {
                turns.entry(t).or_default().push(*id);
            }
        }
    }
    for ids in turns.into_values() {
        let only = |s: usize| -> Vec<u128> {
            ids.iter()
                .copied()
                .filter(|id| {
                    sides[s].by_id.contains_key(id) && !sides[1 - s].by_id.contains_key(id)
                })
                .collect()
        };
        let (oa, ob) = (only(0), only(1));
        if oa.is_empty() || ob.is_empty() {
            continue;
        }
        let first = |s: usize| -> Option<u128> {
            ids.iter()
                .copied()
                .filter(|id| sides[s].by_id.contains_key(id))
                .min_by_key(|id| {
                    let part = out[id]
                        .metadata
                        .get(crate::memory::MNEMO_META_PART)
                        .and_then(Value::as_u64)
                        .unwrap_or(0);
                    (part, *id)
                })
        };
        let drop = if first(0) <= first(1) { ob } else { oa };
        for id in drop {
            out.remove(&id);
        }
    }
}

/// Each output version's `_mnemo.ver`, from what it finally holds (after
/// relinking): an input's version it equals keeps that one's; any other
/// is the hash of its bytes, which is a write's own `_mnemo.ver` when it
/// holds exactly what that write did, so a version it already lists (an
/// earlier one) is named so again, and the rest are marked a merge's. Naming it before the links settle would name
/// one record differently in different groupings of three or more copies.
/// A conflict copy keeps its source's version
/// (decision.v0210-conflict-flags).
fn name_final_versions(out: &mut BTreeMap<u128, Memory>, sides: &[Side]) {
    use super::records::{hlc_prev, set_hlc_prev, version_hash, VER_MERGED};
    for (id, m) in out.iter_mut() {
        if m.metadata.contains_key(MNEMO_META_MERGE_SOURCE) {
            continue;
        }
        let Some(st) = stamp(m) else {
            continue;
        };
        let inputs: Vec<&Memory> = sides.iter().filter_map(|s| s.by_id.get(id)).collect();
        let core = core_bytes(m);
        let mut prev = hlc_prev(m);
        let v = match inputs.iter().find(|x| core_bytes(x) == core) {
            Some(x) => match x.metadata.get(MNEMO_META_VER).and_then(Value::as_u64) {
                Some(v) => v,
                None => continue, // an older build's version, as it was
            },
            None => {
                let h = version_hash(m);
                let real =
                    prev.contains(&(st, h)) || inputs.iter().any(|x| version(x) == Some((st, h)));
                if real {
                    h
                } else {
                    h | VER_MERGED
                }
            }
        };
        let old = version(m);
        if old.map(|o| o.1) == Some(v) {
            continue;
        }
        prev.extend(old.filter(|(_, x)| x & VER_MERGED == 0));
        prev.remove(&(st, v));
        m.metadata.insert(MNEMO_META_VER.into(), Value::from(v));
        set_hlc_prev(m, prev);
    }
}

/// One tombstone per deleted id: the larger number, then the larger id.
fn keep_one_tombstone(out: &mut BTreeMap<u128, Memory>) {
    let mut by_target: BTreeMap<u128, Vec<u128>> = BTreeMap::new();
    for (id, m) in out.iter() {
        if kind(m) == Kind::Tombstone {
            if let Some(x) = ulid_at(m, MNEMO_META_TOMBSTONE_OF) {
                by_target.entry(x).or_default().push(*id);
            }
        }
    }
    for ids in by_target.into_values() {
        if ids.len() < 2 {
            continue;
        }
        let keep = *ids
            .iter()
            .max_by_key(|id| (number(&out[*id]), **id))
            .expect("two or more");
        for id in ids {
            if id != keep {
                out.remove(&id);
            }
        }
    }
}

impl Mnemo {
    /// Rule (0)'s inputs from an open handle: every live record, and the
    /// ids of deleted rows with no live record.
    pub(super) fn merge_input(&mut self) -> Result<MergeInput> {
        let records = self.memories()?;
        let live: HashSet<u128> = records.iter().map(|m| m.id.0).collect();
        let deleted: BTreeSet<u128> = self
            .catalog
            .iter()
            .filter(|e| e.deleted && !live.contains(&e.id))
            .map(|e| e.id)
            .collect();
        Ok(MergeInput {
            records,
            deleted: deleted.into_iter().map(Ulid).collect(),
        })
    }

    /// The live manifest's id: the one `about` shows (the most important,
    /// then the oldest).
    fn manifest_id(records: &[Memory]) -> Option<u128> {
        records
            .iter()
            .filter(|m| {
                kind(m) == Kind::Manifest && !m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY)
            })
            .max_by(|x, y| {
                x.importance
                    .total_cmp(&y.importance)
                    .then_with(|| y.created_at.cmp(&x.created_at))
                    .then_with(|| y.id.0.cmp(&x.id.0))
            })
            .map(|m| m.id.0)
    }

    /// v0.21.0 (13.7): **merge two diverged copies of one brain into a new
    /// file at `out`**. Neither input changes: each is opened read-only,
    /// with its own secret, and held for readers for the whole merge (so a
    /// live brain's writers wait; merge copies).
    ///
    /// The inputs are one brain when their salts match (a copy of a copy),
    /// or when both manifests have the same id (a copy rekeyed since). The
    /// output follows compaction: the first input's secret, kind, salt and
    /// KDF settings (never lower), a new data key, the engine's own cache
    /// entries only (no cached results), one snapshot, and the index when
    /// either input has one. It's written to a temp file beside `out` and
    /// put in place with a call that fails if `out` exists; on any error
    /// nothing is left behind.
    ///
    /// Refused, with the reason, when the inputs aren't one brain; when
    /// their embedder identities, dimensions or float-vector settings
    /// differ; when an input is older than format 9 or needs a read-write
    /// open first; and when `out` exists (a link included) or is an input.
    ///
    /// The rules are in the module's documentation; [`MergeReport`] says
    /// what they did. A delete made before an input's `_mnemo.hlc_since`,
    /// or by an older build, isn't known to a merge, and the inputs still
    /// hold the records the merge removed.
    pub fn merge_files(
        a: impl AsRef<Path>,
        a_secret: MergeSecret<'_>,
        b: impl AsRef<Path>,
        b_secret: MergeSecret<'_>,
        out: impl AsRef<Path>,
        opts: &MergeOptions,
    ) -> Result<MergeReport> {
        let (a, b, out) = (a.as_ref(), b.as_ref(), out.as_ref());
        let now_ms = opts.now_ms.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64)
        });
        let refuse = |why: String| MnemoError::Invalid(format!("merge refused: {why}"));
        // The output: nothing at the name (a link included), never an input.
        if std::fs::symlink_metadata(out).is_ok() {
            let is_input = [a, b]
                .iter()
                .any(|p| paths_refer_to_same_file(p, out).unwrap_or(false));
            return Err(refuse(if is_input {
                format!("the output {} is one of the inputs", out.display())
            } else {
                format!("the output {} already exists", out.display())
            }));
        }
        let open = |p: &Path, s: MergeSecret<'_>, label: &str| -> Result<Mnemo> {
            let r = match s {
                MergeSecret::Passphrase(pp) => Mnemo::open_read_only(p, pp),
                MergeSecret::Key(k) => Mnemo::open_read_only_with_key(p, k),
            };
            match r {
                Err(MnemoError::NeedsWriteOpen { reason }) => Err(refuse(format!(
                    "input {label} ({}) needs a read-write open first ({reason}): \
                     `mnemo index {}` opens it read-write, which recovers it and leaves its \
                     records as they are; then merge again",
                    p.display(),
                    p.display()
                ))),
                Err(e) => Err(e),
                Ok(db) => {
                    if !db.is_v9() {
                        return Err(refuse(format!(
                            "input {label} ({}) is older than format 9: run `mnemo compact {}` \
                             on it first",
                            p.display(),
                            p.display()
                        )));
                    }
                    Ok(db)
                }
            }
        };
        let mut da = open(a, a_secret, "a")?;
        let mut db = open(b, b_secret, "b")?;
        if da.dimensions != db.dimensions {
            return Err(refuse(format!(
                "the inputs have different dimensions ({} and {})",
                da.dimensions, db.dimensions
            )));
        }
        if da.float_vectors() != db.float_vectors() {
            return Err(refuse(
                "one input keeps float vectors and the other doesn't".into(),
            ));
        }
        let (ia, ib) = (da.embedder_identity()?, db.embedder_identity()?);
        if ia != ib {
            let d = |i: &Option<crate::embed::EmbedderIdentity>| {
                i.as_ref().map_or("none".to_string(), |i| i.describe())
            };
            return Err(refuse(format!(
                "the inputs' embedders differ: a {}, b {}",
                d(&ia),
                d(&ib)
            )));
        }
        let input_a = da.merge_input()?;
        let input_b = db.merge_input()?;
        let same_salt = da.header.salt == db.header.salt;
        let (ma, mb) = (
            Self::manifest_id(&input_a.records),
            Self::manifest_id(&input_b.records),
        );
        let same_manifest = ma.is_some() && ma == mb;
        if !same_salt && !same_manifest {
            let m = |x: Option<u128>| x.map_or("no manifest".into(), |u| Ulid(u).to_string());
            return Err(refuse(format!(
                "{} and {} aren't copies of one brain: their salts differ, and their manifests \
                 are {} and {}",
                a.display(),
                b.display(),
                m(ma),
                m(mb)
            )));
        }
        let want_index = da.has_index() || db.has_index();
        drop(db);
        let (records, report) = merge_core(input_a, input_b, now_ms);

        // The output, as compaction writes one.
        let secret_kind = a_secret.kind();
        let kdf = if secret_kind == SecretKind::Passphrase && da.kdf.below_passphrase_floor() {
            da.kdf.for_passphrase()
        } else {
            da.kdf
        };
        let config = MnemoConfig {
            dimensions: da.dimensions,
            kdf,
            store_float_vectors: da.float_vectors(),
            ..Default::default()
        };
        let salt = da.header.salt;
        let (tmp, mut new) = create_temp(
            out,
            "merge",
            a_secret.bytes(),
            secret_kind,
            config,
            Some(salt),
        )?;
        let result = (|| {
            for m in records {
                new.put_existing(m)?;
            }
            if want_index && !new.is_empty() {
                new.build_index_with(IndexConfig::default())?;
            }
            let positions: Vec<usize> = (0..da.cache_dir.len()).collect();
            for pos in positions {
                let e = &da.cache_dir[pos];
                if !e.deleted && embed::is_reserved_namespace(&e.namespace) {
                    new.copy_cache_entry_from(&mut da, pos)?;
                }
            }
            new.flush()?;
            drop(new);
            // In place only if nothing has the name: a link fails if it does.
            std::fs::hard_link(&tmp, out)?;
            let _ = std::fs::remove_file(&tmp);
            #[cfg(unix)]
            if let Some(parent) = out.parent() {
                let parent = if parent.as_os_str().is_empty() {
                    Path::new(".")
                } else {
                    parent
                };
                if let Ok(dir) = File::open(parent) {
                    let _ = dir.sync_all();
                    crate::bump_fsync();
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(match e {
                MnemoError::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists => {
                    refuse(format!("the output {} already exists", out.display()))
                }
                e => e,
            });
        }
        Ok(report)
    }

    /// v0.21.0 (13.7): the live records a merge flagged
    /// ([`crate::is_merge_conflict`]), conflict copies and superseded
    /// versions included, in id order. Reads every live record but thread
    /// turns, which a merge never flags.
    pub fn merge_conflicts(&mut self) -> Result<Vec<Memory>> {
        let entries: Vec<_> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .filter(|e| e.v9.is_none_or(|r| r.kind != super::records::ROW_KIND_TURN))
            .cloned()
            .collect();
        let mut out = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if crate::memory::is_merge_conflict(&m) {
                out.push(m);
            }
        }
        out.sort_by_key(|m| m.id);
        Ok(out)
    }
}
