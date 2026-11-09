//! Phase 12.7b: essence items (add, append, update, resolve, remove) and the essence render.

use super::*;

impl Mnemo {
    /// Phase 12.7b: add a new essence item to a section (any section
    /// except [`EssenceSection::Timeline`]). Content ≤
    /// [`memory::ESSENCE_SECTION_MAX_BYTES`] (280 bytes); items over
    /// that are rejected. Starting status is `"open"`.
    ///
    /// Use [`Mnemo::essence_append_timeline`] for timeline entries —
    /// they have a different cap (600 bytes), are append-only, and
    /// carry no status lifecycle.
    pub fn essence_add(
        &mut self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        Self::check_essence_add(section, text)?;
        self.essence_add_with(scope, section, text, refs, None)
    }

    /// Validation shared by [`Mnemo::essence_add`] and `commit_deltas`.
    pub(super) fn check_essence_add(section: EssenceSection, text: &str) -> Result<()> {
        if section == EssenceSection::Timeline {
            return Err(MnemoError::Invalid(
                "essence_add: use essence_append_timeline for the timeline section".into(),
            ));
        }
        if text.is_empty() {
            return Err(MnemoError::Invalid(
                "essence_add: text must not be empty".into(),
            ));
        }
        if text.len() > section.item_max_bytes() {
            return Err(MnemoError::Invalid(format!(
                "essence_add: '{}' items must be <= {} bytes; got {}",
                section.as_str(),
                section.item_max_bytes(),
                text.len()
            )));
        }
        Ok(())
    }

    /// Write half of `essence_add`, with an optional precomputed vector.
    pub(super) fn essence_add_with(
        &mut self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
        vector: Option<Vec<f32>>,
    ) -> Result<Ulid> {
        let now = memory::now_secs();
        let mem = self.build_essence_record(scope, section, text, refs, Some(now), vector)?;
        self.put(mem)
    }

    /// Phase 12.7b: append a new timeline entry (append-only
    /// milestone). Content ≤ [`memory::ESSENCE_TIMELINE_MAX_BYTES`]
    /// (600 bytes); entries over that are rejected (not truncated —
    /// truncating a milestone silently loses the narrative).
    pub fn essence_append_timeline(
        &mut self,
        scope: EssenceScope,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        Self::check_essence_append(text)?;
        self.essence_append_with(scope, text, refs, None)
    }

    /// Validation shared by [`Mnemo::essence_append_timeline`] and
    /// `commit_deltas`.
    pub(super) fn check_essence_append(text: &str) -> Result<()> {
        if text.is_empty() {
            return Err(MnemoError::Invalid(
                "essence_append_timeline: text must not be empty".into(),
            ));
        }
        if text.len() > EssenceSection::Timeline.item_max_bytes() {
            return Err(MnemoError::Invalid(format!(
                "essence_append_timeline: entries must be <= {} bytes; got {}",
                EssenceSection::Timeline.item_max_bytes(),
                text.len()
            )));
        }
        Ok(())
    }

    /// Write half of `essence_append_timeline`, with an optional
    /// precomputed vector.
    pub(super) fn essence_append_with(
        &mut self,
        scope: EssenceScope,
        text: &str,
        refs: Option<Vec<String>>,
        vector: Option<Vec<f32>>,
    ) -> Result<Ulid> {
        let mem =
            self.build_essence_record(scope, EssenceSection::Timeline, text, refs, None, vector)?;
        self.put(mem)
    }

    /// Phase 12.7b: update an existing essence section item in place.
    /// Returns [`WriteOutcome::Updated`] / [`WriteOutcome::Unchanged`].
    /// Timeline entries are append-only — calling this on a timeline
    /// record is an error.
    pub fn essence_update(
        &mut self,
        id: &Ulid,
        text: &str,
        refs: Option<Vec<String>>,
    ) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let existing = self.load_essence_record(id)?;
        let section = self.section_of(&existing)?;
        if section == EssenceSection::Timeline {
            return Err(MnemoError::Invalid(
                "essence_update: timeline entries are append-only; append a new one instead".into(),
            ));
        }
        if text.is_empty() {
            return Err(MnemoError::Invalid(
                "essence_update: text must not be empty".into(),
            ));
        }
        if text.len() > section.item_max_bytes() {
            return Err(MnemoError::Invalid(format!(
                "essence_update: '{}' items must be <= {} bytes; got {}",
                section.as_str(),
                section.item_max_bytes(),
                text.len()
            )));
        }
        let mut patch = MemoryPatch {
            content: Some(text.to_string()),
            ..Default::default()
        };
        // We don't have a usable embedder-free path for an in-place
        // content update, so provide the same vector as the existing
        // record (the text-vector drift guard in `update` will accept
        // this).
        patch.vector = Some(existing.vector.clone());
        if let Some(r) = refs {
            let mut meta = Map::new();
            // Preserve the user-visible metadata (none for essence —
            // everything lives under `_mnemo.*`) and let `update`'s
            // metadata-replacement path keep the reserved keys.
            meta.insert(
                MNEMO_META_REFS.into(),
                Value::Array(r.into_iter().map(Value::String).collect()),
            );
            patch.metadata = Some(meta);
        }
        self.update(id, patch)
    }

    /// Phase 12.7b: mark a section item as resolved (open → resolved
    /// transition). A new history entry lands on
    /// `_mnemo.status_history`. Idempotent — resolving a record that
    /// is already resolved is a no-op.
    pub fn essence_resolve(&mut self, id: &Ulid) -> Result<WriteOutcome> {
        self.essence_transition_status(id, essence_status::RESOLVED)
    }

    /// Phase 12.7b: soft-delete a section item. Same semantics as
    /// [`Mnemo::delete`] on any record — space reclaimed by a later
    /// compact.
    pub fn essence_remove(&mut self, id: &Ulid) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.delete(id)
    }

    /// Phase 12.7b: deterministic, budgeted markdown render of the
    /// essence layer for a thread (or the project layer when
    /// `scope = EssenceScope::Project`). `max_bytes` caps the output;
    /// sections are emitted in the fixed [`EssenceSection::render_order`],
    /// and each section's own budget is proportional to its position
    /// in the order. Oldest timeline entries fold into a
    /// `+N earlier milestones` tail when the budget runs out.
    ///
    /// The render always ends with an archive pointer so the reader
    /// knows how to drill in deeper (`mnemo thread export …` or
    /// `recall --thread …`).
    pub fn essence(&mut self, scope: EssenceScope, max_bytes: usize) -> Result<String> {
        let max_bytes = if max_bytes == 0 {
            ESSENCE_DEFAULT_RENDER_BYTES
        } else {
            max_bytes
        };
        // Collect every live essence record for this scope.
        // R-004.2: only the essence items, from the handle caches.
        let entries = self.essence_entries()?;
        let mut by_section: std::collections::BTreeMap<&'static str, Vec<Memory>> =
            std::collections::BTreeMap::new();
        for section in EssenceSection::render_order() {
            by_section.insert(section.as_str(), Vec::new());
        }
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_ESSENCE)
            {
                continue;
            }
            // Scope filter. Project-scope records are shown in every
            // thread's render (and in the project render); thread-scope
            // records only match when scope == same thread.
            let rec_thread = m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str());
            let matches_scope = match (&scope, rec_thread) {
                (EssenceScope::Project, None) => true,
                (EssenceScope::Thread(_), None) => true, // project-scope always shows
                (EssenceScope::Thread(t), Some(rt)) => rt == t,
                (EssenceScope::Project, Some(_)) => false,
            };
            if !matches_scope {
                continue;
            }
            let section_str = m
                .metadata
                .get(MNEMO_META_SECTION)
                .and_then(|v| v.as_str())
                .unwrap_or("timeline");
            by_section
                .entry(
                    EssenceSection::parse(section_str)
                        .map(|s| s.as_str())
                        .unwrap_or("timeline"),
                )
                .or_default()
                .push(m);
        }

        // Sort each section's records:
        // - Decisions + Timeline: newest-first (by created_at desc).
        // - Everything else: sort open items before resolved, then
        //   newest-first. Cheap, deterministic.
        for (name, items) in by_section.iter_mut() {
            let newest_first = matches!(*name, "decisions" | "timeline");
            items.sort_by(|a, b| {
                let a_resolved = status_of(a) == Some(essence_status::RESOLVED);
                let b_resolved = status_of(b) == Some(essence_status::RESOLVED);
                match (a_resolved, b_resolved) {
                    (false, true) => std::cmp::Ordering::Less,
                    (true, false) => std::cmp::Ordering::Greater,
                    _ => {
                        if newest_first {
                            b.created_at.cmp(&a.created_at)
                        } else {
                            a.created_at.cmp(&b.created_at)
                        }
                    }
                }
            });
        }

        // Per-section budget: give intent / conventions / decisions a
        // larger share (they frame the thread), timeline a smaller
        // share (it can fold). This is deterministic because it
        // depends only on `max_bytes`.
        let header_reserve = 256usize; // header + archive pointer
        let body_budget = max_bytes.saturating_sub(header_reserve);
        let per_section_budget = |section: EssenceSection| -> usize {
            match section {
                EssenceSection::Intent => body_budget / 8,
                EssenceSection::Conventions => body_budget / 6,
                EssenceSection::Decisions => body_budget / 4,
                EssenceSection::State => body_budget / 8,
                EssenceSection::Open => body_budget / 8,
                EssenceSection::Next => body_budget / 12,
                EssenceSection::Lessons => body_budget / 12,
                EssenceSection::Timeline => body_budget / 4,
            }
        };

        let mut out = String::with_capacity(max_bytes);
        let scope_label = match &scope {
            EssenceScope::Thread(t) => format!("thread `{t}`"),
            EssenceScope::Project => "project".to_string(),
        };
        out.push_str(&format!("# essence — {scope_label}\n\n"));

        // Phase 12.7c: continuation. If this is a thread render AND
        // the thread has a `_mnemo.continues` link, pull the prior
        // thread's open section items, its `next`, and its last-N
        // timeline entries in ahead of the new thread's own essence.
        if let EssenceScope::Thread(new_id) = &scope {
            if let Some(prev_id) = self.predecessor_thread(new_id)? {
                let continuation = self
                    .render_continuation_prelude(&prev_id, max_bytes.saturating_sub(out.len()))?;
                if !continuation.is_empty() && out.len() + continuation.len() <= max_bytes {
                    out.push_str(&continuation);
                }
            }
        }

        for section in EssenceSection::render_order() {
            let items = match by_section.get(section.as_str()) {
                Some(v) if !v.is_empty() => v,
                _ => continue,
            };
            let budget = per_section_budget(*section);
            let section_header = format!("## {}\n", section.as_str());
            if out.len() + section_header.len() > max_bytes {
                break;
            }
            out.push_str(&section_header);
            let mut used = 0usize;
            let mut rendered = 0usize;
            for item in items {
                let line = render_essence_item(*section, item);
                if used + line.len() > budget {
                    break;
                }
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
                used += line.len();
                rendered += 1;
            }
            // Fold the remainder for timeline.
            if *section == EssenceSection::Timeline && rendered < items.len() {
                let fold = format!("- +{} earlier milestones\n", items.len() - rendered);
                if out.len() + fold.len() <= max_bytes {
                    out.push_str(&fold);
                }
            }
            if out.len() < max_bytes {
                out.push('\n');
            }
        }

        // Archive pointer.
        let pointer = match &scope {
            EssenceScope::Thread(t) => format!(
                "> full history: `mnemo thread export <file> --thread {t}` or `recall --thread {t}`\n"
            ),
            EssenceScope::Project => String::from(
                "> full history: `mnemo thread export <file> --thread <id>` or `recall --thread <id>`\n",
            ),
        };
        if out.len() + pointer.len() <= max_bytes {
            out.push_str(&pointer);
        }
        Ok(out)
    }

    // --- essence internal helpers ----------------------------------

    /// Build an essence record. `vector` is a precomputed embedding (the
    /// 12.7d `commit_deltas` batch path); `None` embeds `text` with the
    /// registered embedder, or falls back to an axis-neutral unit vector
    /// when there is none. Essence records are read by `_mnemo.kind` and
    /// scope, not by similarity, so the fallback costs no retrieval
    /// quality; it only has to pass `put`'s non-zero guard.
    pub(super) fn build_essence_record(
        &self,
        scope: EssenceScope,
        section: EssenceSection,
        text: &str,
        refs: Option<Vec<String>>,
        status_at: Option<i64>,
        vector: Option<Vec<f32>>,
    ) -> Result<Memory> {
        let v = match (vector, self.embedder.as_ref()) {
            (Some(v), _) => v,
            (None, Some(e)) => e.embed(&[text])?.pop().ok_or_else(|| {
                MnemoError::Invalid("essence: embedder returned no vector".into())
            })?,
            (None, None) => self.sentinel_vector(),
        };
        let mut m = Memory::new(text.to_string(), MemoryType::Semantic, v).with_importance(0.5);
        m.metadata.insert(
            MNEMO_META_KIND.into(),
            Value::String(MNEMO_META_KIND_ESSENCE.into()),
        );
        m.metadata.insert(
            MNEMO_META_SECTION.into(),
            Value::String(section.as_str().into()),
        );
        if let EssenceScope::Thread(t) = &scope {
            m.metadata
                .insert(MNEMO_META_THREAD.into(), Value::String(t.clone()));
        }
        if let Some(r) = refs {
            m.metadata.insert(
                MNEMO_META_REFS.into(),
                Value::Array(r.into_iter().map(Value::String).collect()),
            );
        }
        if let Some(at) = status_at {
            m.metadata.insert(
                MNEMO_META_STATUS.into(),
                Value::String(essence_status::OPEN.into()),
            );
            m.metadata.insert(
                MNEMO_META_STATUS_HISTORY.into(),
                Value::Array(vec![serde_json::json!({
                    "status": essence_status::OPEN,
                    "at": at,
                })]),
            );
        }
        Ok(m)
    }

    /// Axis-neutral unit vector (every component `1/sqrt(dims)`), used by
    /// essence records without an embedder and by thread-link markers.
    pub(super) fn sentinel_vector(&self) -> Vec<f32> {
        let dims = self.dimensions.max(1);
        vec![1.0 / (dims as f32).sqrt(); dims]
    }

    pub(super) fn load_essence_record(&mut self, id: &Ulid) -> Result<Memory> {
        let m = self.get(id)?;
        if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()) != Some(MNEMO_META_KIND_ESSENCE)
        {
            return Err(MnemoError::Invalid(format!(
                "essence: record {id} is not an essence record (missing `_mnemo.kind = essence`)"
            )));
        }
        Ok(m)
    }

    pub(super) fn section_of(&self, m: &Memory) -> Result<EssenceSection> {
        let s = m
            .metadata
            .get(MNEMO_META_SECTION)
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                MnemoError::Invalid("essence: record missing `_mnemo.section`".into())
            })?;
        EssenceSection::parse(s)
            .ok_or_else(|| MnemoError::Invalid(format!("essence: unknown section '{s}' on record")))
    }

    pub(super) fn essence_transition_status(
        &mut self,
        id: &Ulid,
        to: &str,
    ) -> Result<WriteOutcome> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let existing = self.load_essence_record(id)?;
        let section = self.section_of(&existing)?;
        if !section.has_status_lifecycle() {
            return Err(MnemoError::Invalid(format!(
                "essence_transition: '{}' section has no status lifecycle",
                section.as_str()
            )));
        }
        let from = existing
            .metadata
            .get(MNEMO_META_STATUS)
            .and_then(|v| v.as_str())
            .unwrap_or(essence_status::OPEN);
        if from == to {
            return Ok(WriteOutcome::Unchanged);
        }
        let allowed = ESSENCE_TRANSITIONS
            .iter()
            .any(|(f, t)| *f == from && *t == to);
        if !allowed {
            return Err(MnemoError::Invalid(format!(
                "essence_transition: '{from}' → '{to}' is not a valid transition for essence"
            )));
        }
        // Mutate metadata in place and `put` back under the same id
        // so `_mnemo.updated_at` gets stamped and access stats are
        // preserved.
        let mut new = existing.clone();
        new.metadata
            .insert(MNEMO_META_STATUS.into(), Value::String(to.to_string()));
        let now = memory::now_secs();
        let history_entry = serde_json::json!({ "status": to, "at": now });
        let history = new
            .metadata
            .entry(MNEMO_META_STATUS_HISTORY.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        if let Value::Array(arr) = history {
            arr.push(history_entry);
        }
        new.metadata
            .insert(MNEMO_META_UPDATED_AT.into(), Value::Number(now.into()));
        new.id = existing.id;
        new.created_at = existing.created_at;
        self.put(new)?;
        Ok(WriteOutcome::Updated)
    }

    // --- Phase 12.7c: continuation + commit_deltas ------------------
}
