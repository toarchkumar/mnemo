//! Phase 12.7c: thread continuation (thread_start and its prelude) and commit_deltas.

use super::*;

impl Mnemo {
    /// Start a new thread that continues from a previous one
    /// (Phase 12.7c). Writes a thread-link marker record with
    /// `_mnemo.kind = "thread_link"` + `_mnemo.thread = <new_id>` +
    /// `_mnemo.continues = <from_id>`. The marker holds no user
    /// content; its purpose is to let `essence(Thread(new_id))`
    /// discover the continuation chain.
    ///
    /// The render for the new thread pulls the previous thread's
    /// open section items, its `next`, and its last
    /// [`CONTINUATION_TIMELINE_ENTRIES`] timeline entries ahead of
    /// the new thread's own essence — the "new thread, same
    /// experience" path.
    pub fn thread_start(&mut self, new_thread_id: &str, from_thread_id: &str) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if new_thread_id.is_empty() || from_thread_id.is_empty() {
            return Err(MnemoError::Invalid(
                "thread_start: 'new_thread_id' and 'from_thread_id' must not be empty".into(),
            ));
        }
        if new_thread_id == from_thread_id {
            return Err(MnemoError::Invalid(
                "thread_start: 'new_thread_id' must differ from 'from_thread_id'".into(),
            ));
        }
        // Store a tiny marker record so the continuation chain is
        // queryable from a fresh handle. The sentinel vector passes
        // `put`'s zero-vector guard; default recall skips link records.
        let v = self.sentinel_vector();
        let content = format!("thread link: {new_thread_id} continues from {from_thread_id}");
        let mut m = Memory::new(content, MemoryType::Semantic, v).with_importance(0.1);
        m.metadata.insert(
            MNEMO_META_KIND.into(),
            Value::String(MNEMO_META_KIND_THREAD_LINK.into()),
        );
        m.metadata.insert(
            MNEMO_META_THREAD.into(),
            Value::String(new_thread_id.to_string()),
        );
        m.metadata.insert(
            MNEMO_META_CONTINUES.into(),
            Value::String(from_thread_id.to_string()),
        );
        let id = self.put(m)?;
        Ok(id)
    }

    /// Phase 12.7c helper: render the predecessor thread's open
    /// items + `next` + last-N timeline entries as a "continued from"
    /// prelude. Used by [`Mnemo::essence`] to carry forward the
    /// prior thread's live state into a fresh thread.
    pub(super) fn render_continuation_prelude(
        &mut self,
        prev_thread: &str,
        max_bytes: usize,
    ) -> Result<String> {
        if max_bytes == 0 {
            return Ok(String::new());
        }
        let entries = self.essence_entries()?;
        let mut open_items: Vec<Memory> = Vec::new();
        let mut next_items: Vec<Memory> = Vec::new();
        let mut timeline_items: Vec<Memory> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                != Some(MNEMO_META_KIND_ESSENCE)
            {
                continue;
            }
            if m.metadata.get(MNEMO_META_THREAD).and_then(|v| v.as_str()) != Some(prev_thread) {
                continue;
            }
            let section = match m
                .metadata
                .get(MNEMO_META_SECTION)
                .and_then(|v| v.as_str())
                .and_then(EssenceSection::parse)
            {
                Some(s) => s,
                None => continue,
            };
            let status = m
                .metadata
                .get(MNEMO_META_STATUS)
                .and_then(|v| v.as_str())
                .unwrap_or(essence_status::OPEN);
            match section {
                EssenceSection::Open if status == essence_status::OPEN => {
                    open_items.push(m);
                }
                EssenceSection::Next if status == essence_status::OPEN => {
                    next_items.push(m);
                }
                EssenceSection::Timeline => timeline_items.push(m),
                _ => {}
            }
        }
        // Newest-first timeline, cap to last N.
        timeline_items.sort_by_key(|m| std::cmp::Reverse(m.created_at));
        timeline_items.truncate(CONTINUATION_TIMELINE_ENTRIES);
        // Oldest-first for stable chronological read in the render.
        timeline_items.reverse();

        let mut out = String::new();
        let header = format!("## continued from `{prev_thread}`\n");
        if header.len() > max_bytes {
            return Ok(String::new());
        }
        out.push_str(&header);
        if !open_items.is_empty() {
            let sub = "### open\n";
            if out.len() + sub.len() <= max_bytes {
                out.push_str(sub);
            }
            for m in &open_items {
                let line = format!("- {}\n", m.content);
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
            }
        }
        if !next_items.is_empty() {
            let sub = "### next\n";
            if out.len() + sub.len() <= max_bytes {
                out.push_str(sub);
            }
            for m in &next_items {
                let line = format!("- {}\n", m.content);
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
            }
        }
        if !timeline_items.is_empty() {
            let sub = "### recent milestones\n";
            if out.len() + sub.len() <= max_bytes {
                out.push_str(sub);
            }
            for m in &timeline_items {
                let line = format!("- {}\n", m.content);
                if out.len() + line.len() > max_bytes {
                    break;
                }
                out.push_str(&line);
            }
        }
        if out.len() < max_bytes {
            out.push('\n');
        }
        Ok(out)
    }

    /// Phase 12.7c helper: find the predecessor thread id for
    /// `thread_id` by reading its `_mnemo.kind = "thread_link"`
    /// marker. Returns `None` if the thread has no predecessor
    /// (fresh thread, no `thread_start` call).
    pub(super) fn predecessor_thread(&mut self, thread_id: &str) -> Result<Option<String>> {
        self.ensure_key_caches()?;
        Ok(self
            .layers
            .as_ref()
            .and_then(|l| l.links.get(thread_id))
            .map(|(from, _)| from.clone()))
    }

    /// Atomic batch write combining 12.1 facts, 12.7b essence ops,
    /// and 12.7a archive turns in one flush (Phase 12.7c; batch
    /// embedding added in 12.7d).
    ///
    /// The batch runs in four steps:
    ///
    /// 1. **Structural checks** on every item: required fields, size
    ///    caps, empty keys, `thread_id` for turns, and that every
    ///    `Update` / `Resolve` / `Remove` targets a live essence record
    ///    the op can apply to.
    /// 2. **One `Embedder::embed` call** for every text that needs a
    ///    vector: facts whose `vector` is empty, essence `Add` /
    ///    `AppendTimeline` texts (when an embedder is registered;
    ///    without one, essence keeps its sentinel vector), and the
    ///    archive turns' chunks.
    /// 3. **Vector checks** (width, non-zero, finite) on supplied and
    ///    embedded vectors alike.
    /// 4. **Writes**: facts, then essence ops, then turns per
    ///    `turn_policy`, then exactly one `flush()`.
    ///
    /// A failure in steps 1–3 returns an error with nothing staged. A
    /// text-only fact, or any turn to archive, with no embedder
    /// registered is a step-1 error. `put` has no rollback, so an I/O
    /// error during step 4 can leave the batch's earlier writes staged
    /// on the handle; steps 1–3 exist so nothing else can fail that
    /// late.
    pub fn commit_deltas(&mut self, batch: CommitBatch) -> Result<CommitReport> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let CommitBatch {
            mut facts,
            essence_ops,
            turns,
            thread_id,
            turn_policy,
        } = batch;
        let has_embedder = self.embedder.is_some();
        let in_ctx = |ctx: &str, e: MnemoError| MnemoError::Invalid(format!("{ctx}: {e}"));

        // --- 1. Structural checks (reads only) ----------------------
        for (i, f) in facts.iter().enumerate() {
            if let Some(k) = &f.key {
                if k.is_empty() {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] key is empty"
                    )));
                }
            }
            if f.memory.vector.is_empty() {
                if f.memory.content.is_empty() {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] has neither a vector nor content to embed"
                    )));
                }
                if !has_embedder {
                    return Err(MnemoError::Invalid(format!(
                        "commit_deltas: facts[{i}] has no vector and no embedder is registered \
                         on this handle; pass a vector or call set_embedder first"
                    )));
                }
            }
        }
        let mut removed_in_batch: std::collections::HashSet<u128> =
            std::collections::HashSet::new();
        for (i, op) in essence_ops.iter().enumerate() {
            let ctx = format!("commit_deltas: essence_ops[{i}]");
            match op.kind {
                EssenceOpKind::Add => {
                    if op.scope.is_none() {
                        return Err(MnemoError::Invalid(format!("{ctx} Add missing 'scope'")));
                    }
                    let section = op.section.ok_or_else(|| {
                        MnemoError::Invalid(format!("{ctx} Add missing 'section'"))
                    })?;
                    let text = op
                        .text
                        .as_deref()
                        .ok_or_else(|| MnemoError::Invalid(format!("{ctx} Add missing 'text'")))?;
                    Self::check_essence_add(section, text).map_err(|e| in_ctx(&ctx, e))?;
                }
                EssenceOpKind::AppendTimeline => {
                    if op.scope.is_none() {
                        return Err(MnemoError::Invalid(format!(
                            "{ctx} AppendTimeline missing 'scope'"
                        )));
                    }
                    let text = op.text.as_deref().ok_or_else(|| {
                        MnemoError::Invalid(format!("{ctx} AppendTimeline missing 'text'"))
                    })?;
                    Self::check_essence_append(text).map_err(|e| in_ctx(&ctx, e))?;
                }
                EssenceOpKind::Update | EssenceOpKind::Resolve | EssenceOpKind::Remove => {
                    let id = op
                        .id
                        .ok_or_else(|| MnemoError::Invalid(format!("{ctx} needs 'id'")))?;
                    if removed_in_batch.contains(&id.0) {
                        return Err(MnemoError::Invalid(format!(
                            "{ctx}: record {id} is removed earlier in this batch"
                        )));
                    }
                    let existing = self.load_essence_record(&id).map_err(|e| in_ctx(&ctx, e))?;
                    let section = self.section_of(&existing).map_err(|e| in_ctx(&ctx, e))?;
                    match op.kind {
                        EssenceOpKind::Update => {
                            let text = op.text.as_deref().ok_or_else(|| {
                                MnemoError::Invalid(format!("{ctx} Update needs 'text'"))
                            })?;
                            if section == EssenceSection::Timeline {
                                return Err(MnemoError::Invalid(format!(
                                    "{ctx}: timeline entries are append-only"
                                )));
                            }
                            if text.is_empty() || text.len() > section.item_max_bytes() {
                                return Err(MnemoError::Invalid(format!(
                                    "{ctx}: '{}' items must be 1..={} bytes; got {}",
                                    section.as_str(),
                                    section.item_max_bytes(),
                                    text.len()
                                )));
                            }
                        }
                        EssenceOpKind::Resolve => {
                            if !section.has_status_lifecycle() {
                                return Err(MnemoError::Invalid(format!(
                                    "{ctx}: '{}' section has no status lifecycle",
                                    section.as_str()
                                )));
                            }
                        }
                        _ => {
                            removed_in_batch.insert(id.0);
                        }
                    }
                }
            }
        }
        if !turns.is_empty() && thread_id.is_none() {
            return Err(MnemoError::Invalid(
                "commit_deltas: 'turns' non-empty but 'thread_id' missing".into(),
            ));
        }
        let archive_turns = !turns.is_empty() && !matches!(turn_policy, TurnPolicy::Drop);
        if archive_turns && !has_embedder {
            return Err(MnemoError::Invalid(
                "commit_deltas: archiving 'turns' needs an embedder registered on this handle; \
                 call set_embedder first or use turn_policy Drop"
                    .into(),
            ));
        }
        let plan = match (&thread_id, archive_turns) {
            (Some(t), true) => Some(self.plan_ingest(t, &turns)?),
            _ => None,
        };

        // --- 2. One batched embed -----------------------------------
        let embeds_essence = |op: &EssenceOp| {
            has_embedder && matches!(op.kind, EssenceOpKind::Add | EssenceOpKind::AppendTimeline)
        };
        let (vectors, n_texts) = {
            let mut texts: Vec<&str> = Vec::new();
            for f in &facts {
                if f.memory.vector.is_empty() {
                    texts.push(f.memory.content.as_str());
                }
            }
            for op in essence_ops.iter().filter(|op| embeds_essence(op)) {
                texts.push(op.text.as_deref().expect("checked in step 1"));
            }
            if let Some(p) = &plan {
                texts.extend(p.texts());
            }
            let vectors = match (texts.is_empty(), self.embedder.as_ref()) {
                (true, _) => Vec::new(),
                (false, Some(e)) => e.embed(&texts)?,
                (false, None) => unreachable!("texts need an embedder; checked in step 1"),
            };
            (vectors, texts.len())
        };

        // --- 3. Vector checks ---------------------------------------
        self.check_vectors("commit_deltas", &vectors, n_texts)?;
        for (i, f) in facts.iter().enumerate() {
            let v = &f.memory.vector;
            if v.is_empty() {
                continue;
            }
            if v.len() != self.dimensions {
                return Err(MnemoError::Invalid(format!(
                    "commit_deltas: facts[{i}] vector has {} dims; this file is {}",
                    v.len(),
                    self.dimensions
                )));
            }
            let ctx = format!("commit_deltas: facts[{i}]");
            self.check_vector(&ctx, v)?;
        }

        // Hand the embedded vectors back out, in the order they were
        // requested: facts, then essence ops, then turn chunks.
        let mut vectors = vectors.into_iter();
        for f in facts.iter_mut() {
            if f.memory.vector.is_empty() {
                f.memory.vector = vectors.next().expect("one vector per text");
            }
        }
        let essence_vectors: Vec<Option<Vec<f32>>> = essence_ops
            .iter()
            .map(|op| embeds_essence(op).then(|| vectors.next().expect("one vector per text")))
            .collect();
        let turn_vectors: Vec<Vec<f32>> = vectors.collect();

        // --- 4. Writes ----------------------------------------------
        let mut report = CommitReport::default();

        for f in facts {
            match f.key {
                Some(k) => {
                    let (_id, outcome) = self.remember_keyed(f.memory, &k)?;
                    match outcome {
                        WriteOutcome::Added => report.facts_added += 1,
                        WriteOutcome::Superseded | WriteOutcome::Updated => {
                            report.facts_superseded += 1
                        }
                        WriteOutcome::Unchanged => report.facts_unchanged += 1,
                    }
                }
                None => {
                    self.remember(f.memory)?;
                    report.facts_added += 1;
                }
            }
        }

        for (op, vector) in essence_ops.into_iter().zip(essence_vectors) {
            let outcome = match op.kind {
                EssenceOpKind::Add => {
                    self.essence_add_with(
                        op.scope.expect("checked in step 1"),
                        op.section.expect("checked in step 1"),
                        op.text.as_deref().expect("checked in step 1"),
                        op.refs,
                        vector,
                    )?;
                    WriteOutcome::Added
                }
                EssenceOpKind::AppendTimeline => {
                    self.essence_append_with(
                        op.scope.expect("checked in step 1"),
                        op.text.as_deref().expect("checked in step 1"),
                        op.refs,
                        vector,
                    )?;
                    WriteOutcome::Added
                }
                EssenceOpKind::Update => self.essence_update(
                    op.id.as_ref().expect("checked in step 1"),
                    op.text.as_deref().expect("checked in step 1"),
                    op.refs,
                )?,
                EssenceOpKind::Resolve => {
                    self.essence_resolve(op.id.as_ref().expect("checked in step 1"))?
                }
                EssenceOpKind::Remove => {
                    self.essence_remove(op.id.as_ref().expect("checked in step 1"))?;
                    WriteOutcome::Updated
                }
            };
            match outcome {
                WriteOutcome::Unchanged => report.essence_ops_unchanged += 1,
                _ => report.essence_ops_applied += 1,
            }
        }

        if let (Some(plan), Some(t)) = (plan, thread_id.as_deref()) {
            let ttl = match turn_policy {
                TurnPolicy::ExpireAfter(secs) => Some(secs),
                _ => None,
            };
            let r = self.apply_ingest(t, plan, turn_vectors, ttl)?;
            report.turns_added += r.added_records;
            report.turns_skipped += r.skipped_existing_turns;
            report.turns_truncated += r.truncated_turns;
            report.turns_chunked += r.chunked_turns;
        }

        self.flush()?;
        Ok(report)
    }
}
