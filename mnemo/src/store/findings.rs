//! v0.18.0 (13a step 8b): review findings as records, `_mnemo.kind =
//! "finding"`, a coordination kind (`docs/design/13a-coordination.md`,
//! section 8, item 4). A Reviewer adds one under an id it gives
//! (`RB-014.1`), the Planner triages it, and a Reviewer verifies it. The
//! engine checks the transitions ([`FINDING_TRANSITIONS`]); who may make
//! each one is the protocol's (AGENTS.md, tmux rule 9), as for tasks.
//!
//! Review round 1 (RB-014): a finding changes only by triage and
//! verification. `update` and `delete` refuse its record, a wrong one is
//! `rejected`, and an id is unique by its prefix and numbers. A `SECURITY`
//! finding is written `private` until it's verified, so an older build's
//! `export --git`, `list` and recall leave it out, and it's verified only
//! against a release tag (`v1.2.3`): verified means shipped.

use super::*;
use crate::memory::{
    finding_status, FINDING_BODY_MAX, FINDING_CATEGORIES, FINDING_PRIORITIES, FINDING_TITLE_MAX,
    FINDING_TRANSITIONS, MNEMO_META_BATCH, MNEMO_META_CATEGORY, MNEMO_META_FINDING_ID,
    MNEMO_META_KIND_FINDING, MNEMO_META_PRIORITY, MNEMO_META_TRIAGE, MNEMO_META_TRIAGED_BY,
    MNEMO_META_VERIFIED_BY, MNEMO_META_VERIFIED_REF,
};

/// v0.18.0: a finding to add ([`Mnemo::finding_add`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindingSpec {
    /// Its id, `^[A-Z]+-[0-9]+\.[0-9]+$` (`RB-014.1`); unique.
    pub id: String,
    /// One of [`FINDING_CATEGORIES`].
    pub category: String,
    /// One of [`FINDING_PRIORITIES`].
    pub priority: String,
    /// One line, at most [`FINDING_TITLE_MAX`] bytes.
    pub title: String,
    /// At most [`FINDING_BODY_MAX`] bytes.
    pub body: String,
    /// What it points at: files and lines, commits, PRs.
    pub refs: Vec<String>,
}

/// v0.18.0: one finding.
#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// The record's id.
    pub record_id: Ulid,
    /// Its id (`RB-014.1`).
    pub id: String,
    /// Its batch, the id's prefix (`RB-014`).
    pub batch: String,
    /// Its category.
    pub category: String,
    /// Its priority.
    pub priority: String,
    /// One line.
    pub title: String,
    /// The body (may be empty).
    pub body: String,
    /// What it points at.
    pub refs: Vec<String>,
    /// Who added it (`_mnemo.author`).
    pub author: Option<String>,
    /// `open`, `accepted`, `deferred`, `rejected`, `needs-user` or
    /// `verified`.
    pub status: String,
    /// The last triage's text.
    pub triage: Option<String>,
    /// Who triaged it last.
    pub triaged_by: Option<String>,
    /// What verified it.
    pub verified_ref: Option<String>,
    /// Who verified it.
    pub verified_by: Option<String>,
    /// When it was added, in unix seconds.
    pub created_at: i64,
    /// Its status changes, oldest first (`from`, `to`, `at`, `by`, and
    /// `note` when there's one).
    pub history: Vec<Value>,
}

impl Finding {
    /// The finding a record holds, if it's one.
    pub fn from_memory(m: &Memory) -> Option<Finding> {
        if m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) != Some(MNEMO_META_KIND_FINDING)
        {
            return None;
        }
        let s = |k: &str| {
            m.metadata
                .get(k)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let (title, body) = match m.content.split_once("\n\n") {
            Some((a, b)) => (a.to_string(), b.to_string()),
            None => (m.content.clone(), String::new()),
        };
        Some(Finding {
            record_id: m.id,
            id: s(MNEMO_META_FINDING_ID).unwrap_or_default(),
            batch: s(MNEMO_META_BATCH).unwrap_or_default(),
            category: s(MNEMO_META_CATEGORY).unwrap_or_default(),
            priority: s(MNEMO_META_PRIORITY).unwrap_or_default(),
            title,
            body,
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
            author: s(crate::memory::MNEMO_META_AUTHOR),
            status: s(MNEMO_META_STATUS).unwrap_or_default(),
            triage: s(MNEMO_META_TRIAGE),
            triaged_by: s(MNEMO_META_TRIAGED_BY),
            verified_ref: s(MNEMO_META_VERIFIED_REF),
            verified_by: s(MNEMO_META_VERIFIED_BY),
            created_at: m.created_at,
            history: m
                .metadata
                .get(MNEMO_META_STATUS_HISTORY)
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        })
    }

    /// The order findings are listed in: by prefix, then batch and item
    /// numbers (`RB-9.1` before `RB-10.1`).
    pub fn sort_key(&self) -> (String, u64, u64) {
        parse_id(&self.id).unwrap_or((self.id.clone(), 0, 0))
    }
}

/// v0.18.0: which findings [`Mnemo::findings`] lists; each field left out
/// matches every finding.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FindingFilter {
    /// Its status.
    pub status: Option<String>,
    /// Its batch (`RB-014`).
    pub batch: Option<String>,
    /// Its category.
    pub category: Option<String>,
    /// Its priority.
    pub priority: Option<String>,
}

/// `RB-014.1` as (`RB`, 14, 1); `None` when it isn't a finding's id.
fn parse_id(id: &str) -> Option<(String, u64, u64)> {
    let (prefix, rest) = id.split_once('-')?;
    let (a, b) = rest.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit());
    if prefix.is_empty()
        || !prefix.bytes().all(|c| c.is_ascii_uppercase())
        || !digits(a)
        || !digits(b)
    {
        return None;
    }
    Some((prefix.to_string(), a.parse().ok()?, b.parse().ok()?))
}

/// v0.18.0: whether `id` is a finding's id: `^[A-Z]+-[0-9]+\.[0-9]+$`,
/// at most 64 bytes.
pub fn valid_finding_id(id: &str) -> bool {
    id.len() <= 64 && parse_id(id).is_some()
}

/// The category whose findings stay `private` until they're verified.
const SECURITY: &str = "SECURITY";

/// Whether `r` is a release tag, `^v[0-9]+\.[0-9]+\.[0-9]+$`.
fn is_release_tag(r: &str) -> bool {
    let Some(rest) = r.strip_prefix('v') else {
        return false;
    };
    let parts: Vec<&str> = rest.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
}

/// Whether a record is a finding (for the writes that refuse one).
pub(super) fn is_finding(m: &Memory) -> bool {
    m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) == Some(MNEMO_META_KIND_FINDING)
}

/// What `update` and `delete` say when asked to change a finding.
pub(super) fn finding_write_refused(what: &str) -> MnemoError {
    MnemoError::Invalid(format!(
        "a finding can't be {what}: it changes only by `finding triage` or `finding verify`; \
         a wrong one is rejected, with why, which keeps its history"
    ))
}

/// A finding's batch: its id without the item number (`RB-014`).
fn batch_of(id: &str) -> String {
    id.rsplit_once('.').map_or(id, |(b, _)| b).to_string()
}

impl Mnemo {
    fn finding_records(&mut self, id: Option<&str>) -> Result<Vec<Memory>> {
        self.coordination_records(MNEMO_META_KIND_FINDING, id)
    }

    fn finding_record(&mut self, id: &str) -> Result<Option<Memory>> {
        Ok(self
            .finding_records(Some(id))?
            .into_iter()
            .find(|m| Finding::from_memory(m).is_some_and(|f| f.id == id)))
    }

    /// v0.18.0: one finding by id. Reads only.
    pub fn finding(&mut self, id: &str) -> Result<Option<Finding>> {
        Ok(self
            .finding_record(id)?
            .as_ref()
            .and_then(Finding::from_memory))
    }

    /// v0.18.0: the findings `filter` matches, ordered by id. Reads only.
    pub fn findings(&mut self, filter: &FindingFilter) -> Result<Vec<Finding>> {
        let eq = |want: &Option<String>, have: &str| want.as_deref().is_none_or(|w| w == have);
        let mut v: Vec<Finding> = self
            .finding_records(None)?
            .iter()
            .filter_map(Finding::from_memory)
            .filter(|f| {
                eq(&filter.status, &f.status)
                    && eq(&filter.batch, &f.batch)
                    && eq(&filter.category, &f.category)
                    && eq(&filter.priority, &f.priority)
            })
            .collect();
        v.sort_by_key(Finding::sort_key);
        Ok(v)
    }

    /// v0.18.0: add a finding, `open`, by this handle's author, which it
    /// needs. Its id must be free, by its prefix and numbers (`RB-0900.1`
    /// is `RB-900.1`): a finding is changed only by triage and
    /// verification. A `SECURITY` finding is written `private` until it's
    /// verified.
    pub fn finding_add(&mut self, spec: &FindingSpec) -> Result<Finding> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if self.author.as_deref().is_none_or(|a| a.trim().is_empty()) {
            return Err(MnemoError::Invalid(
                "a finding needs its author: name the agent (MNEMO_AGENT, or --as on the CLI)"
                    .into(),
            ));
        }
        if !valid_finding_id(&spec.id) {
            return Err(MnemoError::Invalid(format!(
                "{:?} isn't a finding's id: capital letters, '-', a number, '.', a number \
                 (RB-014.1)",
                spec.id
            )));
        }
        if !FINDING_CATEGORIES.contains(&spec.category.as_str()) {
            return Err(MnemoError::Invalid(format!(
                "{:?} isn't a category: {}",
                spec.category,
                FINDING_CATEGORIES.join(", ")
            )));
        }
        if !FINDING_PRIORITIES.contains(&spec.priority.as_str()) {
            return Err(MnemoError::Invalid(format!(
                "{:?} isn't a priority: {}",
                spec.priority,
                FINDING_PRIORITIES.join(", ")
            )));
        }
        let title = spec.title.trim();
        if title.is_empty() || title.contains('\n') || title.len() > FINDING_TITLE_MAX {
            return Err(MnemoError::Invalid(format!(
                "a finding's title is one line, not empty, at most {FINDING_TITLE_MAX} bytes"
            )));
        }
        if spec.body.len() > FINDING_BODY_MAX {
            return Err(MnemoError::Invalid(format!(
                "a finding's body is at most {FINDING_BODY_MAX} bytes"
            )));
        }
        let want = parse_id(&spec.id);
        if let Some(taken) = self
            .finding_records(None)?
            .iter()
            .filter_map(Finding::from_memory)
            .find(|f| f.id == spec.id || parse_id(&f.id) == want)
        {
            let same = if taken.id == spec.id {
                String::new()
            } else {
                format!(" (as {})", taken.id)
            };
            return Err(MnemoError::Invalid(format!(
                "{} is taken{same}; give the next free id",
                spec.id
            )));
        }
        let content = if spec.body.is_empty() {
            title.to_string()
        } else {
            format!("{title}\n\n{}", spec.body)
        };
        let mut meta = Map::new();
        meta.insert(MNEMO_META_FINDING_ID.into(), Value::String(spec.id.clone()));
        meta.insert(MNEMO_META_BATCH.into(), Value::String(batch_of(&spec.id)));
        meta.insert(
            MNEMO_META_CATEGORY.into(),
            Value::String(spec.category.clone()),
        );
        meta.insert(
            MNEMO_META_PRIORITY.into(),
            Value::String(spec.priority.clone()),
        );
        meta.insert(
            MNEMO_META_STATUS.into(),
            Value::String(finding_status::OPEN.into()),
        );
        meta.insert(
            crate::memory::MNEMO_META_REFS.into(),
            Value::Array(spec.refs.iter().map(|r| Value::String(r.clone())).collect()),
        );
        if spec.category == SECURITY {
            meta.insert(
                crate::memory::MNEMO_META_VISIBILITY.into(),
                Value::String("private".into()),
            );
        }
        let id =
            self.put_coordination_with_id(Ulid::new(), MNEMO_META_KIND_FINDING, content, meta)?;
        Ok(Finding::from_memory(&self.get(&id)?).expect("a finding"))
    }

    /// v0.18.0: triage a finding: to `accepted`, `deferred`, `rejected` or
    /// `needs-user`, with `text` (why), as [`FINDING_TRANSITIONS`] allows.
    /// The record changes in place, with a history entry.
    pub fn finding_triage(&mut self, id: &str, to: &str, text: &str) -> Result<Finding> {
        if to == finding_status::VERIFIED {
            return Err(MnemoError::Invalid(
                "a finding is verified with `verify`, and a ref".into(),
            ));
        }
        let text = text.trim();
        if text.is_empty() || text.len() > FINDING_BODY_MAX {
            return Err(MnemoError::Invalid(format!(
                "a triage needs its text (why), at most {FINDING_BODY_MAX} bytes"
            )));
        }
        self.finding_move(id, to, text, |m, by| {
            m.metadata
                .insert(MNEMO_META_TRIAGE.into(), Value::String(text.to_string()));
            m.metadata
                .insert(MNEMO_META_TRIAGED_BY.into(), Value::String(by.to_string()));
        })
    }

    /// v0.18.0: verify a finding against `verified_ref` (a commit, a tag,
    /// a PR), from `accepted`, `deferred` or `needs-user`. A `SECURITY`
    /// finding is verified only against a published release tag
    /// (`v1.2.3`), and becomes `team` then.
    pub fn finding_verify(&mut self, id: &str, verified_ref: &str) -> Result<Finding> {
        let r = verified_ref.trim();
        if r.is_empty() || r.contains('\n') || r.len() > FINDING_TITLE_MAX {
            return Err(MnemoError::Invalid(format!(
                "a verification needs its ref (a commit, a tag, a PR), one line, at most \
                 {FINDING_TITLE_MAX} bytes"
            )));
        }
        let security = self.finding(id)?.is_some_and(|f| f.category == SECURITY);
        if security && !is_release_tag(r) {
            return Err(MnemoError::Invalid(format!(
                "a SECURITY finding is verified against a published release tag (v1.2.3), \
                 not {r:?}"
            )));
        }
        self.finding_move(id, finding_status::VERIFIED, r, |m, by| {
            m.metadata
                .insert(MNEMO_META_VERIFIED_REF.into(), Value::String(r.to_string()));
            m.metadata
                .insert(MNEMO_META_VERIFIED_BY.into(), Value::String(by.to_string()));
            if security {
                m.metadata.insert(
                    crate::memory::MNEMO_META_VISIBILITY.into(),
                    Value::String("team".into()),
                );
            }
        })
    }

    /// Move a finding to `to`, as [`FINDING_TRANSITIONS`] allows, with
    /// `set` filling in the fields that go with it.
    fn finding_move(
        &mut self,
        id: &str,
        to: &str,
        note: &str,
        set: impl FnOnce(&mut Memory, &str),
    ) -> Result<Finding> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let Some(mut m) = self.finding_record(id)? else {
            return Err(MnemoError::Invalid(format!("no finding {id:?}")));
        };
        let f = Finding::from_memory(&m).expect("a finding");
        if !FINDING_TRANSITIONS.contains(&(f.status.as_str(), to)) {
            return Err(MnemoError::Invalid(format!(
                "a finding can't go from {} to {to}",
                f.status
            )));
        }
        let by = self.author.clone().unwrap_or_else(|| "anonymous".into());
        let now = self.coordination_now();
        m.metadata
            .insert(MNEMO_META_STATUS.into(), Value::String(to.to_string()));
        set(&mut m, &by);
        Self::task_history(&mut m, &f.status, to, &by, now, &[("note", note)]);
        let rid = m.id;
        self.put(m)?;
        Ok(Finding::from_memory(&self.get(&rid)?).expect("a finding"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finding_ids() {
        for ok in ["RB-014.1", "R-011.2", "REV-1.10", "A-0.0"] {
            assert!(valid_finding_id(ok), "{ok}");
        }
        for bad in [
            "",
            "rb-014.1",
            "RB014.1",
            "RB-014",
            "RB-014.",
            "RB-.1",
            "RB-01a.1",
            "RB-014.1.2",
            "R B-1.1",
            "RB-1.1 ",
            "-1.1",
        ] {
            assert!(!valid_finding_id(bad), "{bad}");
        }
        assert_eq!(batch_of("RB-014.12"), "RB-014");
        assert!(parse_id("RB-9.1") < parse_id("RB-10.1"));
        assert_eq!(parse_id("RB-0900.1"), parse_id("RB-900.1"));
    }

    #[test]
    fn release_tags() {
        for ok in ["v0.18.0", "v1.2.3", "v10.20.30"] {
            assert!(is_release_tag(ok), "{ok}");
        }
        for bad in [
            "",
            "0.18.0",
            "v0.18",
            "v0.18.0-rc1",
            "v0.18.0.1",
            "abc1234",
            "v.1.2",
            "V1.2.3",
            "v1..3",
        ] {
            assert!(!is_release_tag(bad), "{bad}");
        }
    }
}
