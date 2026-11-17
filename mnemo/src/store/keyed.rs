//! Phase 12.1: keyed upserts, edit in place (update), and supersession history.

use super::*;

impl Mnemo {
    /// Stage-identical check for a 12.1 keyed no-op. The incoming write
    /// is byte-equal to the live record iff every user-visible field
    /// (content, type, importance, vector, user metadata) matches. The
    /// `_mnemo.*` reserved keys on the live record are **ignored** for
    /// this comparison — a repeat of an identical write re-issued by an
    /// extractor must be a NOOP even if the live record already carries
    /// supersession bookkeeping from a prior op.
    ///
    /// `rounded` says the live vector was read from a v9 file's fp16 column
    /// (FMT-9b), so it's compared with the incoming vector after the same
    /// rounding: a real embedder's value never survives it exactly, and
    /// before v0.16 every such re-write added a version.
    pub(super) fn is_identical_keyed_write(
        incoming: &Memory,
        live: &Memory,
        rounded: bool,
    ) -> bool {
        if incoming.content != live.content {
            return false;
        }
        if incoming.memory_type != live.memory_type {
            return false;
        }
        if (incoming.importance - live.importance).abs() > f32::EPSILON {
            return false;
        }
        let same_vector = if rounded {
            crate::veccol::dequantize(&crate::veccol::quantize(&incoming.vector).0) == live.vector
        } else {
            incoming.vector == live.vector
        };
        if !same_vector {
            return false;
        }
        // Strip `_mnemo.*` from both sides before comparing user metadata,
        // all but the caller's own (13a: visibility and freshness count).
        let strip = |m: &Map<String, Value>| -> Map<String, Value> {
            m.iter()
                .filter(|(k, _)| !k.starts_with(crate::memory::MNEMO_META_NS) || is_caller_key(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        };
        strip(&incoming.metadata) == strip(&live.metadata)
    }

    /// 13a (AGENT-DEFAULT): the agent the manifest names for a write that
    /// names none (`_mnemo.default_agent`), if it names one. The CLI and the
    /// MCP server use it in place of `default`; the engine's own default is
    /// unchanged.
    pub fn default_agent(&mut self) -> Result<Option<String>> {
        Ok(self
            .about_in(crate::memory::VisibilitySet::ALL)?
            .into_iter()
            .find(is_manifest)
            .and_then(|m| {
                m.metadata
                    .get(MNEMO_META_DEFAULT_AGENT)
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }))
    }

    /// 13a (AGENT-DEFAULT): set (or with `None`, clear) the manifest's
    /// `_mnemo.default_agent`, in place: the manifest keeps its id, content
    /// and vector. `agent` must be 1 to 64 of `[A-Za-z0-9._-]`. Returns
    /// `Unchanged` when it already says so. A brain with no manifest is
    /// refused.
    pub fn set_default_agent(&mut self, agent: Option<&str>) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if let Some(a) = agent {
            if !crate::memory::valid_agent_id(a) {
                return Err(MnemoError::Invalid(format!(
                    "{a:?} isn't an agent id: 1 to 64 of letters, digits, '.', '_' and '-'"
                )));
            }
        }
        let Some(mut m) = self
            .about_in(crate::memory::VisibilitySet::ALL)?
            .into_iter()
            .find(is_manifest)
        else {
            return Err(MnemoError::Invalid(
                "this brain has no manifest to record a default agent on".into(),
            ));
        };
        let want = agent.map(|a| Value::String(a.to_string()));
        if m.metadata.get(MNEMO_META_DEFAULT_AGENT) == want.as_ref() {
            return Ok(WriteOutcome::Unchanged);
        }
        match want {
            Some(v) => m.metadata.insert(MNEMO_META_DEFAULT_AGENT.into(), v),
            None => m.metadata.remove(MNEMO_META_DEFAULT_AGENT),
        };
        m.metadata.insert(
            crate::memory::MNEMO_META_UPDATED_AT.into(),
            Value::Number(memory::now_secs().into()),
        );
        self.put(m)?;
        Ok(WriteOutcome::Updated)
    }

    /// Store a memory under a keyed upsert slot. Returns the live record's
    /// id and a [`WriteOutcome`] saying whether this write created a new
    /// slot (`Added`), replaced a prior record (`Superseded`), or matched
    /// the existing live record byte-for-byte (`Unchanged`, no file write).
    ///
    /// The slot is identified by `(scope_owner, key)`:
    /// * `Scope::Private` memories share a slot per `(agent_id, key)`.
    /// * `Scope::Shared` memories share one slot per `key` across the file.
    ///
    /// On supersession the prior record is **not** deleted — it remains
    /// retrievable through [`Mnemo::history`] — but it is excluded from
    /// default [`Mnemo::recall`] and [`Mnemo::about`]. A caller that wants
    /// the full lineage of a key walks `history(key)`.
    ///
    /// This is the atomic 12.1 "supersede-or-noop" write path. Plain
    /// [`Mnemo::remember`] without a key is unchanged.
    pub fn remember_keyed(
        &mut self,
        mut memory: Memory,
        key: &str,
    ) -> Result<(Ulid, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if key.is_empty() {
            return Err(MnemoError::Invalid(
                "remember_keyed: 'key' must not be empty".into(),
            ));
        }
        // RB-001.24: a keyed write is always a new record, so no caller
        // `_mnemo.*` key is accepted (the engine sets `_mnemo.key` below).
        // 13a: visibility and freshness are the caller's (checked); no other
        // `_mnemo.*` key is.
        check_caller_reserved(&memory.metadata)?;
        self.ensure_key_caches()?;
        let slot = Self::key_slot_for(&memory, key);
        let existing = self.key_index.as_ref().and_then(|k| k.get(&slot).copied());

        // The canonical `_mnemo.key`, so the record is self-identifying.
        memory
            .metadata
            .insert(MNEMO_META_KEY.into(), Value::String(key.to_string()));

        // NOOP check: if a live record exists for this slot and its
        // user-visible fields match, return the existing id without a
        // write.
        if let Some(prev_id_u) = existing {
            let prev_idx = self
                .index
                .get(prev_id_u, &self.catalog)
                .ok_or_else(|| MnemoError::Invalid("key_index drift".into()))?;
            let prev_entry = self.catalog[prev_idx].clone();
            let prev_mem = self.read_memory(&prev_entry)?;
            inherit_levels(&mut memory, &prev_mem);
            let rounded = prev_entry.v9.is_some() && !self.float_vectors();
            if Self::is_identical_keyed_write(&memory, &prev_mem, rounded) {
                return Ok((Ulid(prev_id_u), WriteOutcome::Unchanged));
            }

            // SUPERSESSION: insert the new record, then rewrite the old
            // with `_mnemo.superseded_by` + `_mnemo.invalid_at`.
            let now = memory::now_secs();
            memory.id = Ulid::nil(); // force a fresh ULID in `put`
            memory.metadata.insert(
                MNEMO_META_SUPERSEDES.into(),
                Value::String(Ulid(prev_id_u).to_string()),
            );
            let new_id = self.put(memory)?;
            // Mark the prior record.
            let mut prev_mut = prev_mem;
            prev_mut.metadata.insert(
                MNEMO_META_SUPERSEDED_BY.into(),
                Value::String(new_id.to_string()),
            );
            prev_mut
                .metadata
                .insert(MNEMO_META_INVALID_AT.into(), Value::Number(now.into()));
            // keep prev_mut.id so `put` overwrites in place.
            self.put(prev_mut)?;
            // Update caches incrementally.
            if let Some(k) = self.key_index.as_mut() {
                k.insert(slot, new_id.0);
            }
            if let Some(s) = self.superseded.as_mut() {
                s.insert(prev_id_u);
            }
            return Ok((new_id, WriteOutcome::Superseded));
        }

        // ADDED: no prior record for this slot.
        memory.id = Ulid::nil();
        let new_id = self.put(memory)?;
        if let Some(k) = self.key_index.as_mut() {
            k.insert(slot, new_id.0);
        }
        Ok((new_id, WriteOutcome::Added))
    }

    /// 12.3 step 5b (v0.15): a keyed write that **adopts a named, unkeyed
    /// memory as the key's first version, then supersedes it**. Use it to
    /// put a fact that was written without a key under one, so the old
    /// version stops showing in recall without being deleted.
    ///
    /// The old record (`supersedes`) takes the key (`_mnemo.key`) and is
    /// marked `_mnemo.superseded_by` and `_mnemo.invalid_at`; the new record
    /// gets `_mnemo.supersedes = <old id>`. [`Mnemo::history`] then lists the
    /// old record as the oldest version, and recall and `about` leave it out,
    /// as for any superseded record. No format change: these are the keyed
    /// path's own `_mnemo.*` keys, which older builds carry through.
    ///
    /// **When the new content matches the old record's** user-visible fields
    /// (the identical-write rule of [`Mnemo::remember_keyed`]), no new record
    /// is written: the old one takes the key and becomes the slot's live
    /// record, and the outcome is [`WriteOutcome::Updated`] with its id.
    ///
    /// Refused with [`MnemoError::Invalid`] when the old id doesn't exist or
    /// is deleted; is already superseded; already has a key (use the plain
    /// keyed write); is an engine record (any `_mnemo.kind`, or the
    /// manifest); isn't in the key's slot scope (the same agent's Private
    /// memory for a Private write, a Shared one for a Shared write); or when
    /// the slot already has a live record. [`MnemoError::ReadOnly`] on a
    /// read-only handle.
    pub fn remember_keyed_superseding(
        &mut self,
        mut memory: Memory,
        key: &str,
        supersedes: &Ulid,
    ) -> Result<(Ulid, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if key.is_empty() {
            return Err(MnemoError::Invalid(
                "remember_keyed_superseding: 'key' must not be empty".into(),
            ));
        }
        // 13a: visibility and freshness are the caller's (checked); no other
        // `_mnemo.*` key is.
        check_caller_reserved(&memory.metadata)?;
        self.ensure_key_caches()?;
        let slot = Self::key_slot_for(&memory, key);
        if self
            .key_index
            .as_ref()
            .is_some_and(|k| k.contains_key(&slot))
        {
            return Err(MnemoError::Invalid(format!(
                "supersedes: key {key:?} already has a live record; a named memory can only \
                 become a key's first version (use the plain keyed write)"
            )));
        }
        let bad = |why: &str| MnemoError::Invalid(format!("supersedes {supersedes}: {why}"));
        let old_idx = self
            .index
            .get(supersedes.0, &self.catalog)
            .ok_or_else(|| bad("no such memory"))?;
        let old_entry = self.catalog[old_idx].clone();
        if old_entry.deleted {
            return Err(bad("the memory is deleted"));
        }
        let mut old = self.read_memory(&old_entry)?;
        if old.metadata.contains_key(MNEMO_META_SUPERSEDED_BY)
            || self
                .superseded
                .as_ref()
                .is_some_and(|s| s.contains(&supersedes.0))
        {
            return Err(bad("it is already superseded"));
        }
        if old.metadata.contains_key(MNEMO_META_KEY) {
            return Err(bad(
                "it already has a key; use the plain keyed write with that key",
            ));
        }
        if old.metadata.contains_key(MNEMO_META_KIND) || is_manifest(&old) {
            return Err(bad(
                "it is an engine record (a thread, essence or the manifest)",
            ));
        }
        if Self::key_slot_for(&old, key) != slot {
            return Err(bad(
                "it isn't in this key's scope (a Private write takes this agent's Private \
                 memory, a Shared write a Shared one)",
            ));
        }
        memory
            .metadata
            .insert(MNEMO_META_KEY.into(), Value::String(key.to_string()));
        old.metadata
            .insert(MNEMO_META_KEY.into(), Value::String(key.to_string()));
        inherit_levels(&mut memory, &old);
        let rounded = old_entry.v9.is_some() && !self.float_vectors();
        if Self::is_identical_keyed_write(&memory, &old, rounded) {
            // Adopt the old record as the slot's live record.
            self.put(old)?;
            if let Some(k) = self.key_index.as_mut() {
                k.insert(slot, supersedes.0);
            }
            return Ok((*supersedes, WriteOutcome::Updated));
        }
        let now = memory::now_secs();
        memory.id = Ulid::nil();
        memory.metadata.insert(
            MNEMO_META_SUPERSEDES.into(),
            Value::String(supersedes.to_string()),
        );
        let new_id = self.put(memory)?;
        old.metadata.insert(
            MNEMO_META_SUPERSEDED_BY.into(),
            Value::String(new_id.to_string()),
        );
        old.metadata
            .insert(MNEMO_META_INVALID_AT.into(), Value::Number(now.into()));
        self.put(old)?;
        if let Some(k) = self.key_index.as_mut() {
            k.insert(slot, new_id.0);
        }
        if let Some(s) = self.superseded.as_mut() {
            s.insert(supersedes.0);
        }
        Ok((new_id, WriteOutcome::Superseded))
    }

    /// [`Mnemo::remember_keyed_superseding`] with `content` embedded by the
    /// registered embedder (the document side).
    pub fn remember_keyed_text_superseding(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        key: &str,
        supersedes: &Ulid,
    ) -> Result<(Ulid, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[content])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let memory = Memory::new(content.to_string(), memory_type, vector);
        self.remember_keyed_superseding(memory, key, supersedes)
    }

    /// Text-side keyed write: embed `content` through the registered
    /// embedder, then [`Mnemo::remember_keyed`]. Same returns.
    pub fn remember_keyed_text(
        &mut self,
        content: &str,
        memory_type: MemoryType,
        key: &str,
    ) -> Result<(Ulid, WriteOutcome)> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[content])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let memory = Memory::new(content.to_string(), memory_type, vector);
        self.remember_keyed(memory, key)
    }

    /// Update an existing memory in place, preserving its ULID,
    /// `created_at` and access stats. Returns [`WriteOutcome::Updated`]
    /// on a real change, [`WriteOutcome::Unchanged`] when every field in
    /// the patch already matches the live record.
    ///
    /// Reserved `_mnemo.*` metadata keys on the existing record are
    /// preserved across a `metadata` replacement; a `metadata` of
    /// `Some(map)` replaces every **user-visible** key with `map`'s
    /// entries. A `content` change requires a matching `vector`
    /// (use [`Mnemo::update_text`] to re-embed) — a bare `content` patch
    /// is rejected to stop text and embedding drifting apart silently.
    ///
    /// A `metadata` map with a key starting with `_mnemo.` is refused with
    /// [`MnemoError::Invalid`]: those keys are the engine's, and the
    /// record's own are kept as described above.
    pub fn update(&mut self, id: &Ulid, patch: MemoryPatch) -> Result<WriteOutcome> {
        if let Some(m) = patch.metadata.as_ref() {
            check_caller_reserved(m)?;
        }
        if let Some(m) = patch.caller_keys.as_ref() {
            for (k, v) in m {
                if !is_caller_key(k) {
                    return Err(super::crud::reserved_key_error(k));
                }
                if !v.is_null() {
                    crate::memory::check_caller_key(k, v).map_err(MnemoError::Invalid)?;
                }
            }
        }
        self.update_unchecked(id, patch)
    }

    /// [`Mnemo::update`] without the reserved-key check, for the engine's own
    /// updates (essence items carry `_mnemo.refs`).
    pub(super) fn update_unchecked(
        &mut self,
        id: &Ulid,
        patch: MemoryPatch,
    ) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if patch.content.is_some() && patch.vector.is_none() {
            return Err(MnemoError::Invalid(
                "update: a content change needs a matching vector (or call update_text \
                 to re-embed through the registered embedder)"
                    .into(),
            ));
        }
        let idx = self
            .index
            .get(id.0, &self.catalog)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        if self.catalog[idx].deleted {
            return Err(MnemoError::NotFound(id.to_string()));
        }
        let entry = self.catalog[idx].clone();
        let existing = self.read_memory(&entry)?;
        // RB-014: a finding changes only by triage and verification.
        if super::findings::is_finding(&existing) {
            return Err(super::findings::finding_write_refused("updated"));
        }

        // Apply patch into a candidate, detect no-op after the fact.
        let mut cand = existing.clone();
        if let Some(c) = &patch.content {
            cand.content = c.clone();
        }
        if let Some(v) = &patch.vector {
            if v.len() != self.dimensions {
                return Err(MnemoError::DimensionMismatch {
                    expected: self.dimensions,
                    got: v.len(),
                });
            }
            cand.vector = v.clone();
        }
        if let Some(i) = patch.importance {
            cand.importance = i.clamp(0.0, 1.0);
        }
        if let Some(mt) = patch.memory_type {
            cand.memory_type = mt;
        }
        if let Some(new_meta) = patch.metadata {
            // Keep reserved `_mnemo.*` keys from the existing record.
            // 13a: but a caller key the patch sets (visibility, freshness)
            // is the patch's.
            let reserved: Vec<(String, Value)> = existing
                .metadata
                .iter()
                .filter(|(k, _)| k.starts_with(crate::memory::MNEMO_META_NS))
                .filter(|(k, _)| !(is_caller_key(k) && new_meta.contains_key(k.as_str())))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            cand.metadata = new_meta;
            for (k, v) in reserved {
                cand.metadata.insert(k, v);
            }
        }
        if let Some(keys) = patch.caller_keys {
            for (k, v) in keys {
                if v.is_null() {
                    cand.metadata.remove(&k);
                } else {
                    cand.metadata.insert(k, v);
                }
            }
        }
        if let Some(ttl) = patch.ttl_secs {
            cand.ttl_secs = ttl;
        }

        // No-op detection: compare all user-visible fields.
        if cand.content == existing.content
            && cand.vector == existing.vector
            && (cand.importance - existing.importance).abs() <= f32::EPSILON
            && cand.memory_type == existing.memory_type
            && cand.metadata == existing.metadata
            && cand.ttl_secs == existing.ttl_secs
        {
            return Ok(WriteOutcome::Unchanged);
        }

        // Stamp `_mnemo.updated_at` and preserve id + created_at.
        let now = memory::now_secs();
        cand.metadata
            .insert(MNEMO_META_UPDATED_AT.into(), Value::Number(now.into()));
        cand.id = existing.id;
        cand.created_at = existing.created_at;
        // put() preserves accessed_at/access_count via the index lookup.
        self.put(cand)?;
        // `put` kept the key slot and the other handle caches current.
        Ok(WriteOutcome::Updated)
    }

    /// In-place update that re-embeds `new_text` through the registered
    /// embedder. Shorthand for `update(id, patch.with_content_and_vector)`.
    pub fn update_text(
        &mut self,
        id: &Ulid,
        new_text: &str,
        mut patch: MemoryPatch,
    ) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[new_text])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        patch.content = Some(new_text.to_string());
        patch.vector = Some(vector);
        self.update(id, patch)
    }

    /// Walk the full chain for a keyed slot: every record this key has
    /// ever held, newest first. Entry 0 is the **live** record (unless
    /// the slot is empty, in which case the vector is empty); subsequent
    /// entries are the superseded history in reverse-supersession order.
    ///
    /// **Slot resolution (v0.8.1 bugfix, lane A).** The lookup slot
    /// matches a keyed `remember` with the same arguments, so the
    /// obvious `history(key, None)` walks the private slot of the
    /// default agent — the same slot
    /// `remember_keyed(Memory::new(...), key)` writes to when the
    /// caller omits an explicit `agent_id` / `scope`. Pre-v0.8.1
    /// `agent_id = None` incorrectly looked up the shared slot and
    /// returned an empty chain for the common case; the old behavior
    /// is still reachable via `history_in(key, KeyScope::Shared)`.
    ///
    /// * `agent_id = Some(a)` → private slot for that agent.
    /// * `agent_id = None` → private slot for the default agent (same
    ///   as [`KeyScope::private_default`]).
    ///
    /// Use [`Mnemo::history_in`] with [`KeyScope::Shared`] for the
    /// shared slot.
    pub fn history(&mut self, key: &str, agent_id: Option<&str>) -> Result<Vec<Memory>> {
        let scope = match agent_id {
            Some(a) => KeyScope::Private(a.to_string()),
            None => KeyScope::private_default(),
        };
        self.history_in(key, scope)
    }

    /// Walk the full chain for a keyed slot identified by an explicit
    /// [`KeyScope`]. Returns the same shape as [`Mnemo::history`] —
    /// live record first, then the superseded chain newest-first.
    ///
    /// Use this when you need the **shared** slot (`KeyScope::Shared`),
    /// which `history(key, None)` no longer reaches as of v0.8.1.
    pub fn history_in(&mut self, key: &str, scope: KeyScope) -> Result<Vec<Memory>> {
        self.ensure_key_caches()?;
        let slot = crate::store::KeySlot {
            scope_owner: match scope {
                KeyScope::Private(a) => Some(a),
                KeyScope::Shared => None,
            },
            key: key.to_string(),
        };
        let live_id = self.key_index.as_ref().and_then(|k| k.get(&slot).copied());
        let mut out = Vec::new();
        let mut cursor = live_id;
        while let Some(id_u) = cursor {
            let idx = self.index.get(id_u, &self.catalog).ok_or_else(|| {
                MnemoError::Invalid("history: superseded chain references missing id".into())
            })?;
            let entry = self.catalog[idx].clone();
            if entry.deleted {
                break;
            }
            let m = self.read_memory(&entry)?;
            let next = m
                .metadata
                .get(MNEMO_META_SUPERSEDES)
                .and_then(|v| v.as_str())
                .and_then(|s| Ulid::from_string(s).ok())
                .map(|u| u.0);
            out.push(m);
            cursor = next;
        }
        Ok(out)
    }

    // --- Phase 12.7a: thread archive --------------------------------
}

/// RB-013.2: a keyed write that gives no visibility (or no review date)
/// takes the live version's, so revising a private fact by its key keeps it
/// private. A caller who wants another level says so.
fn inherit_levels(new: &mut Memory, prev: &Memory) {
    for k in [
        crate::memory::MNEMO_META_VISIBILITY,
        crate::memory::MNEMO_META_REVIEW_BY,
    ] {
        if !new.metadata.contains_key(k) {
            if let Some(v) = prev.metadata.get(k) {
                new.metadata.insert(k.into(), v.clone());
            }
        }
    }
}

impl Mnemo {
    /// RB-013.2: when the keyed version `id` changed the level of the
    /// version it superseded, the two levels (`(old, new)`), so a caller can
    /// say so (`superseded (private → team)`).
    pub fn superseded_level_change(&mut self, id: &Ulid) -> Result<Option<(String, String)>> {
        let new = self.get(id)?;
        let Some(old) = new
            .metadata
            .get(MNEMO_META_SUPERSEDES)
            .and_then(Value::as_str)
            .and_then(|s| Ulid::from_string(s).ok())
        else {
            return Ok(None);
        };
        let old = self.get(&old)?;
        let (a, b) = (
            crate::memory::visibility_of(&old).to_string(),
            crate::memory::visibility_of(&new).to_string(),
        );
        Ok((a != b).then_some((a, b)))
    }
}
