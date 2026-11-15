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
    // Step 2 (messages) is its first caller outside the test hook.
    #[cfg_attr(not(feature = "dev-hooks"), allow(dead_code))]
    pub(super) fn put_coordination(
        &mut self,
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
        m.metadata = metadata;
        self.put(m)
    }
}
