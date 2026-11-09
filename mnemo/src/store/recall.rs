//! Retrieval: memories(), about(), exact search, recall (scoring, filters, budgets, the ANN and BM25 candidate paths).

use super::*;

impl Mnemo {
    /// Return every live memory (used by tooling and compaction).
    pub fn memories(&mut self) -> Result<Vec<Memory>> {
        // FMT-9b: one pass over the vector pages instead of one per record.
        self.ensure_column()?;
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut out = Vec::with_capacity(entries.len());
        for e in &entries {
            out.push(self.read_memory(e)?);
        }
        Ok(out)
    }

    /// Return the database's **self-describing onboarding memories** — the
    /// ones tagged `metadata.area = "onboarding"`. This is the engine-level
    /// surface for the *single-file philosophy*: an agent who receives a
    /// `.mnemo` file (and its passphrase) can call this to learn what the
    /// file is, which embedder it expects, the recommended agent id, and any
    /// other conventions the file's author chose to record — all without
    /// needing any external documentation.
    ///
    /// Ordering: the canonical manifest (tag `metadata.topic = "manifest"`)
    /// always comes first; everything else follows in `importance` descending,
    /// then `created_at` ascending for deterministic results.
    pub fn about(&mut self) -> Result<Vec<Memory>> {
        // Phase 12.1: drop records that have been superseded by a newer
        // keyed write. They still live in snapshot history and
        // `history(key)` — just not here.
        // R-004.2: read only the onboarding records, from the handle caches.
        self.ensure_key_caches()?;
        let ids: Vec<u128> = self
            .layers
            .as_ref()
            .map(|l| l.onboarding.iter().copied().collect())
            .unwrap_or_default();
        let mut out: Vec<Memory> = Vec::new();
        for e in self.live_entries(ids) {
            let m = self.read_memory(&e)?;
            if !m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) && is_onboarding(&m) {
                out.push(m);
            }
        }
        out.sort_by(|a, b| {
            let a_manifest = is_manifest(a);
            let b_manifest = is_manifest(b);
            // Manifest topic wins first; then importance desc; then created_at asc.
            b_manifest
                .cmp(&a_manifest)
                .then_with(|| b.importance.total_cmp(&a.importance))
                .then_with(|| a.created_at.cmp(&b.created_at))
        });
        Ok(out)
    }

    /// The project essence for the `about` briefing (Phase 12.7d): the
    /// [`EssenceScope::Project`] render at the default budget
    /// ([`ESSENCE_DEFAULT_RENDER_BYTES`]), or `None` when the file has no
    /// open project-scope item or timeline entry. The CLI and MCP `about`
    /// append it after the manifest and onboarding memories; [`Mnemo::about`]
    /// itself keeps returning records.
    pub fn about_essence(&mut self) -> Result<Option<String>> {
        let entries = self.essence_entries()?;
        let mut any = false;
        for e in &entries {
            let m = self.read_memory(e)?;
            let is_project_essence = m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str())
                == Some(MNEMO_META_KIND_ESSENCE)
                && !m.metadata.contains_key(MNEMO_META_THREAD);
            if is_project_essence && status_of(&m) != Some(essence_status::RESOLVED) {
                any = true;
                break;
            }
        }
        if !any {
            return Ok(None);
        }
        self.essence(EssenceScope::Project, ESSENCE_DEFAULT_RENDER_BYTES)
            .map(Some)
    }

    // --- retrieval --------------------------------------------------------

    /// Phase-1 brute-force search: rank live memories by raw similarity only.
    /// Read-only — does not update access statistics.
    pub fn search(
        &mut self,
        query: &[f32],
        top_k: usize,
        metric: Metric,
    ) -> Result<Vec<(Memory, f32)>> {
        if query.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: query.len(),
            });
        }
        let now = memory::now_secs();
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();

        let mut scored: Vec<(Memory, f32)> = Vec::new();
        for e in &entries {
            let m = self.read_memory(e)?;
            if m.is_expired(now) {
                continue;
            }
            let sim = memory::similarity(metric, query, &m.vector);
            scored.push((m, sim));
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored.truncate(top_k);
        Ok(scored)
    }

    /// Phase-5 multi-signal recall: rank by
    /// `α·similarity + β·recency + γ·importance + δ·ln(freq)`, with type and
    /// agent filtering. Updates `accessed_at` / `access_count` on the
    /// returned memories (persisted on the next `flush`).
    ///
    /// When an ANN index has been built ([`Mnemo::build_index`]), recall runs
    /// the tiered IVF→PQ→rerank pipeline: it scores only the similarity-nearest
    /// `n_rerank` candidates rather than every memory. Without an index it
    /// scores every live memory exactly.
    /// One pass of filter + score for [`Mnemo::recall`]. Factored out of
    /// `recall` so the outer path can retry with a wider shortlist when
    /// the type/agent/supersession/TTL filters starve the ANN-narrowed
    /// result below `top_k` (Phase 12.1 short-results-on-indexed-path
    /// fix).
    ///
    /// * `force_exact = false` → use the ANN index when one exists; fall
    ///   back to a full scan only if none is built.
    /// * `force_exact = true` → skip the ANN index and scan every live
    ///   non-superseded record. Used by the outer path as the last-resort
    ///   attempt when widening the shortlist would already touch half or
    ///   more of the live non-superseded set.
    ///
    /// Returns an unsorted, un-truncated `Vec<RecallResult>`; the outer
    /// `recall` sorts and truncates once.
    pub(super) fn score_once(
        &mut self,
        req: &RecallRequest,
        now: i64,
        force_exact: bool,
    ) -> Result<Vec<RecallResult>> {
        // Candidate set: ANN-narrowed when an index exists and the
        // caller hasn't forced an exact scan, else everything live.
        // R-004.2: a filter pinning `_mnemo.thread` scans just that
        // thread's records (from the handle caches); one that can only
        // match non-fact kinds skips the ANN, which doesn't hold them.
        // FMT-9d: keyword matches, normalised by the best one.
        self.kw = None;
        if let Some(text) = &req.keywords {
            let scores = self.bm25_scores(text, req.bm25_k1, req.bm25_b)?;
            let max = scores.values().copied().fold(0.0f32, f32::max);
            if max > 0.0 {
                self.kw = Some(
                    scores
                        .into_iter()
                        .map(|(row, s)| (self.catalog[row].id, s / max))
                        .collect(),
                );
            } else {
                self.kw = Some(std::collections::HashMap::new());
            }
        }
        if let Some(thread) = filter_thread_pin(&req.filter) {
            let thread = thread.to_string();
            let entries = self.thread_entries(&thread)?;
            return self.score_entries(req, now, &entries);
        }
        // Keyword-only: the candidates are the matches.
        if req.query.is_empty() {
            let ids: Vec<u128> = self
                .kw
                .as_ref()
                .map_or(Vec::new(), |k| k.keys().copied().collect());
            let entries: Vec<CatalogEntry> = ids
                .into_iter()
                .filter_map(|id| self.index.get(id, &self.catalog))
                .map(|i| self.catalog[i].clone())
                .filter(|e| !e.deleted)
                .collect();
            return self.score_entries(req, now, &entries);
        }
        let force_exact = force_exact || filter_bypasses_ann(&req.filter);
        let mut entries: Vec<CatalogEntry> = match (&self.ann, force_exact) {
            (Some(ann), false) => {
                let n_rerank = self.rerank_for(ann, req);
                let ids = ann.query(&req.query, req.n_probe, n_rerank);
                ids.iter()
                    .filter_map(|&id| self.index.get(id, &self.catalog))
                    .map(|i| self.catalog[i].clone())
                    .filter(|e| !e.deleted)
                    .collect()
            }
            _ => self
                .catalog
                .iter()
                .filter(|e| !e.deleted)
                .cloned()
                .collect(),
        };
        // FMT-9d: an ANN shortlist plus the keyword matches it missed.
        if let Some(kw) = &self.kw {
            if entries.len() < self.catalog.len() {
                let have: std::collections::HashSet<u128> = entries.iter().map(|e| e.id).collect();
                let extra: Vec<u128> = kw.keys().copied().filter(|id| !have.contains(id)).collect();
                for id in extra {
                    if let Some(i) = self.index.get(id, &self.catalog) {
                        if !self.catalog[i].deleted {
                            entries.push(self.catalog[i].clone());
                        }
                    }
                }
            }
        }

        self.score_entries(req, now, &entries)
    }

    /// FMT-9d: a candidate's relevance: its vector similarity plus ε times
    /// its normalised keyword score (keyword-only requests: just the
    /// keyword score). Returns `(relevance, reported similarity)`.
    pub(super) fn relevance(&self, req: &RecallRequest, id: u128, sim: f32) -> (f32, f32) {
        let kw = self
            .kw
            .as_ref()
            .and_then(|k| k.get(&id).copied())
            .unwrap_or(0.0);
        if req.query.is_empty() {
            (kw, kw)
        } else {
            (sim + req.keyword_weight * kw, sim)
        }
    }

    /// Score one candidate set for [`Mnemo::score_once`].
    pub(super) fn score_entries(
        &mut self,
        req: &RecallRequest,
        now: i64,
        entries: &[CatalogEntry],
    ) -> Result<Vec<RecallResult>> {
        // FMT-9b: score v9 candidates from their rows and the fp16 column,
        // decrypting only what makes `top_k`. A metadata filter needs the
        // record body, so it takes the general path below.
        if self.is_v9() && req.filter.is_empty() {
            return self.score_rows(req, now, entries);
        }
        let mut scored: Vec<RecallResult> = Vec::with_capacity(entries.len());
        for e in entries {
            // Phase 12.1: drop superseded records.
            //
            // Fast path: if the lazy `superseded` set has already been
            // built by a prior keyed op on this handle, check it before
            // decrypt — O(1), no AES work. On a fresh read-only handle
            // it is typically `None`, so we fall through to the slow
            // path (one `_mnemo.superseded_by` lookup per surviving
            // record, O(k) over user metadata), and never trigger the
            // full-file scan that `ensure_key_caches()` would do.
            if self.superseded.as_ref().is_some_and(|s| s.contains(&e.id)) {
                continue;
            }
            let m = self.read_memory(e)?;
            if m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) {
                continue;
            }
            if m.is_expired(now) {
                continue;
            }
            if let Some(types) = &req.memory_types {
                if !types.contains(&m.memory_type) {
                    continue;
                }
            }
            if let Some(agent) = &req.agent_id {
                let visible = m.agent_id == *agent || m.scope == Scope::Shared;
                if !visible {
                    continue;
                }
            }
            // Phase 12.6: pre-scoring filter (session_id + metadata
            // conditions). Cheap when `req.filter.is_empty()`; the
            // filter struct's `matches` is a short-circuit AND.
            if !req.filter.is_empty() && !req.filter.matches(&m) {
                continue;
            }
            // Phase 12.7: default-exclude the thread layers from fact
            // recall: archive turns (12.7a), essence items (12.7b; read
            // by scope through the render, not by similarity) and
            // thread-link markers (12.7c; they carry an all-equal
            // sentinel vector and would surface as junk hits). A caller
            // that wants them opts in by placing any `_mnemo.*` term in
            // the filter (equality, `$in`, `$exists`, or a range on
            // `_mnemo.thread` / `_mnemo.kind` / `_mnemo.seq`).
            if !req.filter.mentions_mnemo_meta()
                && matches!(
                    m.metadata.get(MNEMO_META_KIND).and_then(|v| v.as_str()),
                    Some(MNEMO_META_KIND_TURN)
                        | Some(MNEMO_META_KIND_ESSENCE)
                        | Some(MNEMO_META_KIND_THREAD_LINK)
                )
            {
                continue;
            }
            let sim = if req.query.is_empty() {
                0.0
            } else {
                memory::similarity(req.metric, &req.query, &m.vector)
            };
            let (rel, sim) = self.relevance(req, e.id, sim);
            let age_secs = (now - m.created_at) as f32;
            let score = req
                .weights
                .score(rel, age_secs, m.importance, m.access_count);
            scored.push(RecallResult {
                memory: m,
                score,
                similarity: sim,
            });
        }
        Ok(scored)
    }

    /// FMT-9b: [`Mnemo::score_entries`] for a v9 file with no metadata
    /// filter. Every check the general path makes on the decrypted record
    /// is made on the row instead (supersession, TTL, type, agent
    /// visibility, the thread-layer kinds), the similarity comes from the
    /// fp16 column, and the score from the row's `created_at` and
    /// `importance`. Candidates are then decrypted best-first until `top_k`
    /// survive; the agent check is repeated on the record, since the row
    /// only has a hash of the agent id.
    pub(super) fn score_rows(
        &mut self,
        req: &RecallRequest,
        now: i64,
        entries: &[CatalogEntry],
    ) -> Result<Vec<RecallResult>> {
        self.ensure_column()?;
        let d = self.dimensions;
        let q = &req.query;
        let q_norm = q.iter().map(|x| x * x).sum::<f32>().sqrt();
        let agent_h = req.agent_id.as_deref().map(|a| fnv1a32(&[a.as_bytes()]));
        let types: Option<Vec<u8>> = req
            .memory_types
            .as_ref()
            .map(|ts| ts.iter().map(|&t| memory_type_code(t)).collect());
        let mut cand: Vec<(f32, f32, usize)> = Vec::with_capacity(entries.len());
        for (pos, e) in entries.iter().enumerate() {
            let Some(r) = e.v9 else { continue };
            if r.flags & ROW_SUPERSEDED != 0 {
                continue;
            }
            if self.superseded.as_ref().is_some_and(|s| s.contains(&e.id)) {
                continue;
            }
            if r.flags & ROW_HAS_TTL != 0 && now >= r.expires_at {
                continue;
            }
            if let Some(ts) = &types {
                if !ts.contains(&r.memory_type) {
                    continue;
                }
            }
            if let Some(ah) = agent_h {
                if r.flags & ROW_SHARED == 0 && r.agent_hash != ah {
                    continue;
                }
            }
            if matches!(
                r.kind,
                ROW_KIND_TURN | ROW_KIND_ESSENCE | ROW_KIND_THREAD_LINK
            ) {
                continue;
            }
            let Some(idx) = self.index.get(e.id, &self.catalog) else {
                continue;
            };
            if !req.query.is_empty()
                && self.col.resident
                && idx < self.col.have.len()
                && !self.col.have[idx]
                && !self.col.hot.contains_key(&idx)
            {
                self.fill_column_page(idx)?;
            }
            let sim = if req.query.is_empty() {
                0.0
            } else if let Some((h, n)) = self.col.hot.get(&idx) {
                veccol::similarity_f16(req.metric, q, q_norm, h, *n)
            } else if self.col.resident && self.col.have.get(idx).copied().unwrap_or(false) {
                veccol::similarity_f16(
                    req.metric,
                    q,
                    q_norm,
                    &self.col.data[idx * d..(idx + 1) * d],
                    self.col.norms[idx],
                )
            } else {
                let (h, n) = self.column_entry(e)?;
                veccol::similarity_f16(req.metric, q, q_norm, &h, n)
            };
            let (rel, sim) = self.relevance(req, e.id, sim);
            let score = req.weights.score(
                rel,
                (now - r.created_at) as f32,
                r.importance,
                e.access_count,
            );
            cand.push((score, sim, pos));
        }
        cand.sort_by(|a, b| b.0.total_cmp(&a.0));
        let mut out: Vec<RecallResult> = Vec::with_capacity(req.top_k.min(cand.len()));
        for (score, sim, pos) in cand {
            if out.len() >= req.top_k {
                break;
            }
            let m = self.read_memory(&entries[pos])?;
            if let Some(agent) = &req.agent_id {
                if !(m.agent_id == *agent || m.scope == Scope::Shared) {
                    continue;
                }
            }
            out.push(RecallResult {
                memory: m,
                score,
                similarity: sim,
            });
        }
        Ok(out)
    }

    /// Phase-5 multi-signal recall under the v0.7.0 scoring-v2 formula
    /// (see [`crate::ScoreWeights`]). Thin wrapper over
    /// [`Mnemo::recall_budgeted`] that drops the `truncated` flag;
    /// use the budgeted form if you need to know whether
    /// [`RecallRequest::max_bytes`] cut anything.
    pub fn recall(&mut self, req: &RecallRequest) -> Result<Vec<RecallResult>> {
        Ok(self.recall_budgeted(req)?.hits)
    }

    /// Phase 12.8 budgeted recall: scoring v2 (see [`crate::ScoreWeights`])
    /// with an optional UTF-8-content byte cap. Ranks by
    /// `sim + max(sim, 0) · (β·recency + γ·(importance − 0.5) + δ·tanh(uses/5))`,
    /// with type / agent / supersession / TTL filters and the 12.6
    /// `Filter` applied before the scorer. Updates `accessed_at` /
    /// `access_count` on the returned memories (persisted on the next
    /// `flush`).
    ///
    /// When an ANN index has been built ([`Mnemo::build_index`]),
    /// recall runs the tiered IVF→PQ→rerank pipeline. If the filters
    /// (type / agent / supersession / TTL / 12.6) reduce the ANN
    /// shortlist below `top_k`, Phase 12.1's short-results-on-indexed-
    /// path fix kicks in: the query is retried with `n_probe` and
    /// `n_rerank` doubled, and if that would already touch half or
    /// more of the live non-superseded records, the ANN pass is
    /// skipped for an exact scan. Without an index, recall scores
    /// every live non-superseded memory.
    ///
    /// Phase 12.8: after `top_k` is applied, if
    /// [`RecallRequest::max_bytes`] is set, hits are dropped in rank
    /// order once the running sum of `content` UTF-8 bytes would
    /// exceed the budget. The top-1 hit is always kept even if it
    /// alone exceeds the budget. [`RecallResponse::truncated`] is
    /// `true` iff at least one hit was dropped by the budget.
    pub fn recall_budgeted(&mut self, req: &RecallRequest) -> Result<RecallResponse> {
        // FMT-9d: keyword recall needs a v9 file's BM25 index; a
        // keyword-only request has no vector.
        if req.keywords.is_some() && !self.is_v9() {
            return Err(MnemoError::Invalid(
                "keyword recall (keywords) needs a v9 file; run `mnemo compact` to upgrade".into(),
            ));
        }
        let keyword_only = req.query.is_empty() && req.keywords.is_some();
        if !keyword_only && req.query.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: req.query.len(),
            });
        }
        // PERF-1 step 3: the ANN index loads on the first recall.
        self.ensure_ann()?;
        // Access-tracking bumps `accessed_at` / `access_count` in the
        // catalog, which is a mutation. On a read-only handle the caller
        // must pass `track_access(false)` (via CLI `--no-track`).
        if self.read_only && req.track_access {
            return Err(MnemoError::ReadOnly);
        }
        // Phase 12.1 **fix** (lane A v0.8.0 REVIEW): do NOT call
        // `ensure_key_caches()` here. A fresh read-only handle that
        // only ever recalls would otherwise pay a full-file decrypt
        // on its first query to build the lazy superseded set. Instead
        // `score_once` checks `_mnemo.superseded_by` per-record after
        // decrypt (slow path) and consults the set only when it has
        // already been built by a prior keyed op on this handle (fast
        // path). `live_non_superseded` is not knowable without a
        // decrypt pass either, so we approximate by `live_count`; the
        // exact-scan gate uses this as a conservative upper bound,
        // which errs toward widening (cheaper) rather than skipping
        // straight to a full scan.
        let live_count = self.catalog.iter().filter(|e| !e.deleted).count();
        let live_non_superseded = live_count;
        let now = memory::now_secs();

        // Scoring v2 (shipped v0.7.0). Single-pass filter + score:
        //   * `age_secs = now - m.created_at`, so a recall doesn't bump
        //     its own results' recency on later queries.
        //   * `ScoreWeights::score` applies the similarity-gated,
        //     importance-centered, saturating-usage formula documented
        //     on `ScoreWeights`.
        //
        // Phase 12.1 + 12.6 short-results-on-indexed-path fix: when the
        // ANN shortlist is narrower than the live count, the filters
        // (type / agent / supersession / TTL / 12.6) can reduce the
        // result below `top_k`. If that happens, re-query the index
        // with `n_probe` / `n_rerank` doubled from their *effective*
        // base; if that would already cover ≥50% of the live set,
        // skip the index and do an exact scan.
        //
        // Lane A v0.8.0 REVIEW fix: widen from
        // `IvfPqIndex::effective_defaults()` when the caller didn't
        // pin the knobs. The pre-fix code only doubled when `req.n_*`
        // was already `Some(_)`, which skipped the widen step for the
        // default (unpinned) recall path and jumped straight to an
        // exact scan on moderately selective filters.
        let mut scored = self.score_once(req, now, false)?;
        let mut used_exact_scan = self.ann.is_none() || filter_bypasses_ann(&req.filter);
        if !used_exact_scan && scored.len() < req.top_k && live_non_superseded > req.top_k {
            if let Some(ann) = &self.ann {
                let (default_probe, default_rerank) = ann.effective_defaults();
                let base_probe = req.n_probe.unwrap_or(default_probe);
                let base_rerank = self.rerank_for(ann, req).unwrap_or(default_rerank);
                let widen_probe = base_probe.saturating_mul(2);
                let widen_rerank = base_rerank.saturating_mul(2);
                // Attempt 2: widen once, 2× from the effective base.
                // Only worth another ANN pass if either knob actually
                // grew (saturating_mul on usize::MAX would cap).
                if widen_probe > base_probe || widen_rerank > base_rerank {
                    let widened = RecallRequest {
                        n_probe: Some(widen_probe),
                        n_rerank: Some(widen_rerank),
                        ..req.clone()
                    };
                    scored = self.score_once(&widened, now, false)?;
                }
                // Attempt 3: if still short after widening, pay the
                // exact scan — it's the only path that guarantees
                // `top_k` correct hits when the filter is more
                // selective than `widen_rerank / live_count`. Before
                // the fix this was gated on `widened_rerank * 2 >=
                // live_non_superseded`, which silently returned fewer
                // than `top_k` hits on selective filters that fell in
                // the gap. Exact scan is bounded (one decrypt per
                // live record); the previous widen is an optimization.
                if scored.len() < req.top_k {
                    scored = self.score_once(req, now, true)?;
                    used_exact_scan = true;
                }
            }
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        scored.truncate(req.top_k);

        // Update access statistics for everything we surfaced — but only
        // mutate the catalog entry, not the full record on disk. Pre-v5
        // this loop called `self.put(r.memory.clone())`, which serialized
        // and rewrote every result (vector included) to fresh pages,
        // turning a top-K recall into K full record rewrites. Now the
        // catalog is the source of truth for these two fields and the
        // record body stays untouched until the next real edit.
        if req.track_access {
            for r in &mut scored {
                if let Some(idx) = self.index.get(r.memory.id.0, &self.catalog) {
                    self.touch(idx);
                    let entry = &mut self.catalog[idx];
                    entry.accessed_at = now;
                    entry.access_count = entry.access_count.saturating_add(1);
                    // Propagate the just-updated values onto the returned
                    // Memory so the caller sees the post-recall state.
                    r.memory.accessed_at = entry.accessed_at;
                    r.memory.access_count = entry.access_count;
                }
            }
            // Mark the catalog dirty so the next flush persists these
            // bumps. The records themselves are not dirty.
            if !scored.is_empty() {
                self.dirty_catalog = true;
            }
        }

        // Phase 12.8: apply the `max_bytes` budget *after* tracking has
        // seen the full top-k list. Access stats record what the scorer
        // chose; the byte budget shouldn't suppress the signal.
        let truncated = if let Some(budget) = req.max_bytes {
            let orig_len = scored.len();
            let mut running = 0usize;
            let mut cut_at: Option<usize> = None;
            for (i, r) in scored.iter().enumerate() {
                let w = r.memory.content.len();
                if i == 0 {
                    // Always keep the top hit, even if it's over budget
                    // on its own. Avoid "I asked for top_k results and
                    // got zero" when a single memory exceeds the cap.
                    running += w;
                    continue;
                }
                if running.saturating_add(w) > budget {
                    cut_at = Some(i);
                    break;
                }
                running += w;
            }
            if let Some(idx) = cut_at {
                scored.truncate(idx);
                scored.len() < orig_len
            } else {
                false
            }
        } else {
            false
        };

        self.kw = None;
        Ok(RecallResponse {
            hits: scored,
            truncated,
            used_exact_scan,
        })
    }

    // --- approximate index ------------------------------------------------
}
