//! v0.20.0 (v0200-3): incremental catch-up for a shared handle, the design
//! in `docs/design/catch-up.md` (12.4's implementation order, step 6).
//!
//! A shared handle at transaction `t` that finds another process's commit
//! applies only what changed since `t`: the catalog and index deltas newer
//! than `t`, and the small runs whose pointers moved. Everything it can't
//! prove (a new catalog base, a chain that doesn't reach back to `t`, a run
//! that doesn't read) leaves the handle untouched and returns `false`, and
//! the caller does the full reload. The page cache stays valid across a
//! catch-up because it's keyed by page and generation (`cache.rs`).

use super::*;
use std::collections::{BTreeMap, HashMap};

/// What a catch-up reads before it changes anything, so that "can't" is
/// decided with the handle untouched.
struct Plan {
    /// The catalog deltas newer than `t`, oldest first.
    catalog: Vec<(RunPtr, Vec<(u32, CatalogEntry)>)>,
    /// The index deltas newer than `t`, oldest first, when the handle's
    /// loaded index can take them; `None` unloads it instead.
    index: Option<Vec<(RunPtr, Vec<IndexOp>)>>,
    manifest: Option<Vec<Snapshot>>,
    free: Option<FreeList>,
    keys: Option<KeySlotRun>,
    cache_dir: Option<Vec<CacheDirectoryEntry>>,
    bm25: Option<Bm25Manifest>,
}

/// Walk a delta chain back from `head` to the first pointer at or below
/// transaction `t`, which must be `held` exactly (or the chain's empty end
/// when `held` is empty). Returns the newer deltas, oldest first, or `None`
/// when the chain doesn't reach back to `held`.
fn walk_back<T>(
    pager: &mut Pager,
    head: RunPtr,
    held: RunPtr,
    t: u64,
    decode: impl Fn(&[u8]) -> Result<((u64, u64, u64, u64), T)>,
) -> Result<Option<Vec<(RunPtr, T)>>> {
    let mut out = Vec::new();
    let mut at = head;
    loop {
        if at.is_empty() {
            if held.is_empty() {
                break;
            }
            return Ok(None);
        }
        // Every run committed after `t` was sealed under a generation above
        // it: a writer takes its lease (a header-only commit) only after it
        // has caught up past `t`.
        if at.gen <= t {
            if at == held {
                break;
            }
            return Ok(None);
        }
        if out.len() >= 4096 {
            return Ok(None);
        }
        let buf = read_run_bytes(pager, at.start, at.pages, at.len, at.gen)?;
        let (prev, payload) = decode(&buf)?;
        out.push((at, payload));
        at = tuple_ptr(prev);
    }
    out.reverse();
    Ok(Some(out))
}

/// The packed-record page a row lives on, as [`RecPages::from_catalog`]
/// files it.
fn rec_page(e: &CatalogEntry) -> Option<u64> {
    if e.deleted || e.start_page == 0 {
        return None;
    }
    e.v9.filter(|r| r.rec_slot != REC_SPAN)
        .map(|_| e.start_page)
}

/// The vector page a row's column entry lives on, as
/// [`VecColumn::from_catalog`] files it.
fn vec_page(e: &CatalogEntry) -> Option<u64> {
    if e.deleted {
        return None;
    }
    e.v9.map(|r| r.vec_page).filter(|&p| p != 0)
}

/// Move `row` from `old`'s page list to `new`'s, keeping each list sorted
/// (as a fresh build makes it).
fn move_row(
    map: &mut std::collections::HashMap<u64, Vec<usize>>,
    row: usize,
    old: Option<u64>,
    new: Option<u64>,
) {
    if old == new {
        return;
    }
    if let Some(p) = old {
        if let Some(rows) = map.get_mut(&p) {
            rows.retain(|&r| r != row);
            if rows.is_empty() {
                map.remove(&p);
            }
        }
    }
    if let Some(p) = new {
        let rows = map.entry(p).or_default();
        if let Err(at) = rows.binary_search(&row) {
            rows.insert(at, row);
        }
    }
}

/// A BM25 document of a row (`(doc, len)`), as `ensure_bm25_stats` counts
/// them.
fn bm25_doc(e: &CatalogEntry) -> Option<(u32, u16)> {
    if e.deleted {
        return None;
    }
    e.v9.filter(|r| r.doc != 0 && bm25_kind(r.kind))
        .map(|r| (r.doc, r.doc_len))
}

impl Mnemo {
    /// Nothing waits to be committed, so the handle's state is exactly the
    /// file's as of its header: the only state a catch-up may start from.
    fn clean_for_catch_up(&self) -> bool {
        !self.dirty_catalog
            && self.dirty_rows.is_empty()
            && !self.dirty_cache
            && !self.free_dirty
            && self.pending_free.is_empty()
            && !self.dirty_keys
            && !self.catalog_rewrite
            && !self.index_rebase
            && self.index_delta_ops.is_none()
            && self.commit_failed.is_none()
            && self.col.hot.is_empty()
            && self.col.pending.is_empty()
            && self.recs.hot.is_empty()
            && self.recs.pending.is_empty()
            && self.bm25.pending.is_empty()
            && !self.pager.has_dirty_pages()
    }

    /// Bring the handle from its header to `new` by applying only what
    /// changed. `Ok(false)`: it can't (the handle is untouched; do the full
    /// reload). An error after it started changing the handle leaves it
    /// unusable, so the caller reloads on any error too.
    ///
    /// The caller has checked that `new`'s seal verifies under the handle's
    /// data key and that the file is the one the handle was built from.
    ///
    /// `trusted_gen` (v0200-3r): the newest generation the caller proved is
    /// on this handle's timeline (`anchors_hold` in `shared.rs`). Held data
    /// that a new pointer names again, and that is newer than it, is read
    /// again instead of kept.
    pub(super) fn catch_up_to(&mut self, new: &Header, trusted_gen: u64) -> Result<bool> {
        let old = &self.header;
        if !self.is_v9()
            || new.version != old.version
            || new.dimensions != old.dimensions
            || (
                new.catalog_start,
                new.catalog_pages,
                new.catalog_len,
                new.catalog_gen,
            ) != (
                old.catalog_start,
                old.catalog_pages,
                old.catalog_len,
                old.catalog_gen,
            )
            || new.wal_seq < old.wal_seq
            || !self.clean_for_catch_up()
        {
            return Ok(false);
        }
        let Some(plan) = self.plan_catch_up(new)? else {
            return Ok(false);
        };
        self.apply_catch_up(new, plan, trusted_gen)?;
        Ok(true)
    }

    /// A full reload of the same file (a new catalog base, say): take the
    /// vector column of `old`, the state before it, instead of building a
    /// new one, so a fallback neither allocates a second column nor refills
    /// it. A row is kept when it's the same row with its entry where it
    /// was: v9 pages are never rewritten under the same generation, so the
    /// entry is the one a fresh fill would read. The caller has checked
    /// that `old` is this file's state as of its stamp.
    pub(super) fn adopt_column(&mut self, old: &mut Mnemo) {
        let d = self.dimensions;
        if !self.is_v9()
            || self.col.resident
            || !old.col.resident
            || old.dimensions != d
            || !old.clean_for_catch_up()
        {
            return;
        }
        let rows = self.catalog.len();
        let scored = self.scored_rows();
        if scored * veccol::entry_len(d) > self.vector_cache_budget {
            return;
        }
        let mut col = std::mem::take(&mut old.col);
        col.data.resize(rows * d, 0);
        col.norms.resize(rows, 0.0);
        col.have.resize(rows, false);
        let at = |e: &CatalogEntry| e.v9.map(|r| (r.vec_page, r.vec_slot, r.vec_gen));
        for i in 0..rows {
            let now = &self.catalog[i];
            let keep = col.have[i]
                && old
                    .catalog
                    .get(i)
                    .is_some_and(|w| w.id == now.id && at(w) == at(now))
                && now.v9.is_some_and(|r| r.vec_page != 0)
                && scored_row(now);
            if !keep {
                col.have[i] = false;
            }
        }
        self.col.data = col.data;
        self.col.norms = col.norms;
        self.col.have = col.have;
        self.col.scored_rows = scored;
        self.col.resident = true;
    }

    /// Read everything the catch-up needs, changing nothing.
    fn plan_catch_up(&mut self, new: &Header) -> Result<Option<Plan>> {
        let old = self.header.clone();
        let t = old.wal_seq;
        let Some(catalog) = walk_back(
            &mut self.pager,
            new.catalog_delta,
            old.catalog_delta,
            t,
            |buf| {
                let d: CatalogDelta =
                    rmp_serde::from_slice(buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
                Ok((d.prev, d.rows))
            },
        )?
        else {
            return Ok(None);
        };
        // The index: only a loaded one is carried forward, and only when its
        // base is the same and its chain reaches back.
        let same_index_base = (
            new.index_start,
            new.index_pages,
            new.index_len,
            new.index_gen,
        ) == (
            old.index_start,
            old.index_pages,
            old.index_len,
            old.index_gen,
        );
        let index = if self.ann.is_some() && same_index_base {
            walk_back(
                &mut self.pager,
                new.index_delta,
                old.index_delta,
                t,
                |buf| {
                    let d: IndexDeltaOwned = rmp_serde::from_slice(buf)
                        .map_err(|e| MnemoError::Serialize(e.to_string()))?;
                    Ok((d.prev, d.ops))
                },
            )?
        } else {
            None
        };
        let manifest = if (
            new.manifest_start,
            new.manifest_pages,
            new.manifest_len,
            new.manifest_gen,
        ) != (
            old.manifest_start,
            old.manifest_pages,
            old.manifest_len,
            old.manifest_gen,
        ) {
            Some(load_manifest(
                &mut self.pager,
                new.manifest_start,
                new.manifest_pages,
                new.manifest_len,
                new.manifest_gen,
            )?)
        } else {
            None
        };
        let free = if new.free != old.free {
            Some(load_free_list(&mut self.pager, new.free)?)
        } else {
            None
        };
        let keys = if new.keys != old.keys {
            Some(load_key_slots(&mut self.pager, new.keys)?)
        } else {
            None
        };
        let cache_dir = if (
            new.cache_start,
            new.cache_pages,
            new.cache_len,
            new.cache_gen,
        ) != (
            old.cache_start,
            old.cache_pages,
            old.cache_len,
            old.cache_gen,
        ) {
            Some(load_cache_directory(
                &mut self.pager,
                new.cache_start,
                new.cache_pages,
                new.cache_len,
                new.cache_gen,
            )?)
        } else {
            None
        };
        let bm25 = if new.bm25 != old.bm25 {
            Some(load_bm25_manifest(&mut self.pager, new.bm25)?)
        } else {
            None
        };
        Ok(Some(Plan {
            catalog,
            index,
            manifest,
            free,
            keys,
            cache_dir,
            bm25,
        }))
    }

    /// Apply a plan: from here an error leaves the handle half caught up,
    /// and the caller reloads.
    fn apply_catch_up(&mut self, new: &Header, plan: Plan, trusted_gen: u64) -> Result<()> {
        // 1. The catalog rows, keeping each changed row's first old entry.
        // The index generation the handle's entries were reconciled under.
        let old_index_gen = self.index_gen_now();
        let mut changed: BTreeMap<usize, Option<CatalogEntry>> = BTreeMap::new();
        let mut new_rows = 0usize;
        for (ptr, rows) in plan.catalog {
            new_rows += rows.len();
            self.delta_chain.push(ptr);
            for (i, row) in rows {
                let i = i as usize;
                match i.cmp(&self.catalog.len()) {
                    std::cmp::Ordering::Less => {
                        if self.catalog[i].id != row.id {
                            return Err(MnemoError::Invalid(
                                "catch-up: a delta changes a row's id".into(),
                            ));
                        }
                        let was = std::mem::replace(&mut self.catalog[i], row);
                        changed.entry(i).or_insert(Some(was));
                    }
                    std::cmp::Ordering::Equal => {
                        let id = row.id;
                        self.catalog.push(row);
                        self.index.insert(id, i, &self.catalog);
                        changed.entry(i).or_insert(None);
                    }
                    std::cmp::Ordering::Greater => {
                        return Err(MnemoError::Invalid("catalog delta skips a row".into()))
                    }
                }
            }
        }
        self.delta_rows += new_rows;

        // 2. What's built from the catalog, per changed row: the packed
        // record pages, the vector column and the BM25 stats.
        let rows = self.catalog.len();
        let d = self.dimensions;
        if self.col.resident && self.col.have.len() < rows {
            self.col.data.resize(rows * d, 0);
            self.col.norms.resize(rows, 0.0);
            self.col.have.resize(rows, false);
        }
        let mut scored_delta: isize = 0;
        let mut old_vecs: HashMap<usize, (Vec<u16>, f32)> = HashMap::new();
        for (&i, was) in &changed {
            let now = &self.catalog[i];
            let (old_rec, old_vec) = match was {
                Some(w) => (rec_page(w), vec_page(w)),
                None => (None, None),
            };
            move_row(&mut self.recs.page_rows, i, old_rec, rec_page(now));
            move_row(&mut self.col.page_rows, i, old_vec, vec_page(now));
            let moved = match was {
                Some(w) => {
                    w.v9.map(|r| (r.vec_page, r.vec_slot, r.vec_gen))
                        != now.v9.map(|r| (r.vec_page, r.vec_slot, r.vec_gen))
                }
                None => true,
            };
            if moved && i < self.col.have.len() {
                // The old entry, for the index's check below: a commit
                // rewrites its tail vector page, moving rows whose vectors
                // didn't change.
                if self.col.have[i] && self.ann.is_some() && was.is_some() {
                    old_vecs.insert(
                        i,
                        (
                            self.col.data[i * d..(i + 1) * d].to_vec(),
                            self.col.norms[i],
                        ),
                    );
                }
                self.col.have[i] = false;
            }
            scored_delta +=
                isize::from(scored_row(now)) - isize::from(was.as_ref().is_some_and(scored_row));
            if let Some(stats) = &mut self.bm25.stats {
                if let Some((doc, _)) = was.as_ref().and_then(bm25_doc) {
                    stats.remove(doc);
                }
                if let Some((doc, len)) = bm25_doc(now) {
                    stats.insert(doc, i, len);
                }
            }
        }
        if self.col.resident {
            self.col.scored_rows = (self.col.scored_rows as isize + scored_delta).max(0) as usize;
            if self.col.scored_rows * veccol::entry_len(d) > self.vector_cache_budget {
                // Over budget now: a fresh open wouldn't build it either.
                self.col = VecColumn::from_catalog(&self.catalog);
            }
        }

        // 3. The small runs whose pointers moved.
        if let Some(m) = plan.manifest {
            self.manifest = m;
        }
        if let Some(f) = plan.free {
            self.free = f;
        }
        if let Some(k) = plan.keys {
            self.key_slots = k;
        }
        if let Some(c) = plan.cache_dir {
            self.cache_idx = DirectoryIndex::build(&c);
            self.cache_dir = c;
        }
        if let Some(m) = plan.bm25 {
            // Keep the dicts of the segments that are still there; load the
            // new ones.
            if let Some(old_dicts) = self.bm25.dicts.take() {
                let mut old_dicts: Vec<Option<_>> = old_dicts.into_iter().map(Some).collect();
                let mut dicts = Vec::with_capacity(m.segments.len());
                for &seg in &m.segments {
                    let kept = self
                        .bm25
                        .manifest
                        .segments
                        .iter()
                        .position(|&s| s == seg && s.3 <= trusted_gen)
                        .and_then(|j| old_dicts[j].take());
                    match kept {
                        Some(d) => dicts.push(d),
                        None => dicts.push(load_segment_dict(&mut self.pager, tuple_ptr(seg))?),
                    }
                }
                self.bm25.dicts = Some(dicts);
            }
            self.bm25.manifest = m;
        }

        // 4. The header, and the handle caches built from the catalog.
        self.header = new.clone();
        self.pager.write_counter = self.pager.write_counter.max(new.write_counter);
        self.kdf = KdfParams {
            m_cost: new.m_cost,
            t_cost: new.t_cost,
            p_cost: new.p_cost,
        };
        self.rekey_cleanup = None;
        if !changed.is_empty() {
            self.key_index = None;
            self.superseded = None;
            self.layers = None;
            self.agent_set = None;
            self.legacy_coordination = None;
        }

        // 5. The index: the new deltas onto a loaded one, then the changed
        // rows as `reconcile_ann_with_catalog` would treat them; otherwise
        // back to "not loaded".
        match plan.index {
            Some(deltas) if self.ann.is_some() => {
                for (ptr, ops) in deltas {
                    self.index_chain_ops += ops.len();
                    self.index_chain.push(ptr);
                    if let Some(ann) = &mut self.ann {
                        ann.apply_ops(ops)?;
                    }
                }
                self.reconcile_ann_rows(&changed, &old_vecs, old_index_gen)?;
            }
            _ => {
                // As a fresh open: nothing of the old index is pending
                // (a dirty flag with no index would persist "no index").
                self.ann = None;
                self.ann_unloaded = new.index_pages > 0;
                self.dirty_index = false;
                self.index_persist = IndexPersist::default();
                self.index_repair = false;
                self.index_chain = Vec::new();
                self.index_chain_ops = 0;
            }
        }
        Ok(())
    }

    /// [`Mnemo::reconcile_ann_with_catalog`] for the rows a catch-up
    /// changed: a deleted row leaves the index, a live one written after
    /// the index run is read and re-added (or removed, if it's not a fact).
    ///
    /// A row whose index entry can't change is skipped: one that a fresh
    /// open re-adds (written after the index under the new header) and the
    /// handle had re-added too (written after it under the old one), still
    /// live and of the same kind, with the same fp16 column entry. `add`
    /// of the same input gives the same entry; in v9 that input is the
    /// column entry (`finish_memory`), and the kind is `row_tags`' code,
    /// which carries exactly what `is_ann_kind` reads. A row that a new
    /// index delta touched isn't written after the index under the new
    /// header, so it's never skipped.
    ///
    /// A commit repacks every row on its tail record page and rewrites its
    /// tail vector page (FMT-9b.2), so most changed rows are these, and
    /// re-reading them cost about 4 ms per catch-up at 100K. The entry is
    /// the same when the row's vector didn't move, or when the column held
    /// the old entry (`old_vecs`) and the new one is bit for bit the same
    /// (then it's refilled).
    fn reconcile_ann_rows(
        &mut self,
        rows: &BTreeMap<usize, Option<CatalogEntry>>,
        old_vecs: &HashMap<usize, (Vec<u16>, f32)>,
        old_index_gen: u64,
    ) -> Result<()> {
        let d = self.dimensions;
        let float = self.float_vectors();
        let index_gen = self.index_gen_now();
        let index_start = self.header.index_start;
        let mut changed = 0usize;
        for (&i, was) in rows {
            let e = self.catalog[i].clone();
            if let Some(w) = was {
                let re_added = !w.deleted
                    && !e.deleted
                    && written_after_index(w, true, index_start, old_index_gen)
                    && written_after_index(&e, true, index_start, index_gen);
                let at =
                    |x: &CatalogEntry| x.v9.map(|r| (r.kind, r.vec_page, r.vec_slot, r.vec_gen));
                let kind = |x: &CatalogEntry| x.v9.map(|r| r.kind);
                if re_added && e.v9.is_some() && kind(w) == kind(&e) {
                    if at(w) == at(&e) {
                        continue;
                    }
                    if let (false, true, Some((oh, on))) = (
                        float,
                        e.v9.is_some_and(|r| r.vec_page != 0),
                        old_vecs.get(&i),
                    ) {
                        let (h, n) = self.column_entry(&e)?;
                        if h == *oh && n.to_bits() == on.to_bits() {
                            if self.col.resident && i < self.col.have.len() {
                                self.col.data[i * d..(i + 1) * d].copy_from_slice(&h);
                                self.col.norms[i] = n;
                                self.col.have[i] = true;
                            }
                            continue;
                        }
                    }
                }
            }
            if e.deleted {
                if let Some(ann) = &mut self.ann {
                    if ann.contains(e.id) {
                        ann.remove(e.id);
                        changed += 1;
                    }
                }
                continue;
            }
            if !written_after_index(&e, true, index_start, index_gen) {
                continue;
            }
            // Fill the row's column entry, which the index reads below: the
            // next commit's repack then finds it in `old_vecs`. Only a row
            // recall scores, as a fresh open fills.
            if !float
                && self.col.resident
                && i < self.col.have.len()
                && !self.col.have[i]
                && scored_row(&e)
            {
                if let Some(r) = e.v9.filter(|r| r.vec_page != 0) {
                    let page = self.pager.read_page(r.vec_page, r.vec_gen)?;
                    let (h, n) = veccol::decode_entry(&page, d, r.vec_slot as usize)?;
                    self.col.data[i * d..(i + 1) * d].copy_from_slice(&h);
                    self.col.norms[i] = n;
                    self.col.have[i] = true;
                }
            }
            let m = self.read_memory(&e)?;
            if let Some(ann) = &mut self.ann {
                if is_ann_kind(&m) {
                    ann.add(e.id, &m.vector);
                } else {
                    ann.remove(e.id);
                }
                changed += 1;
            }
        }
        if changed > 0 {
            self.dirty_index = true;
            self.index_persist.changes += changed;
        }
        Ok(())
    }
}
