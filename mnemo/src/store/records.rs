//! v9 internals and the record path: page allocation, packed record pages, the fp16 vector column, BM25 glue, turn compression, and put().

use super::*;

impl Mnemo {
    /// FMT-9: true for a v9 file (page reuse, generations in the AAD).
    pub(super) fn is_v9(&self) -> bool {
        self.header.version >= VERSION_V9
    }

    /// FMT-9: the generation for pages written now: the id of the
    /// transaction that will commit them, or a lower bound on it (a WAL
    /// relocation inside the next flush can push the commit id higher).
    /// Either way it's strictly greater than the generation any earlier
    /// write to a reused page carried, which is all rollback protection
    /// needs: a page is reused only after the transaction that freed it
    /// committed, and that transaction came after the one that wrote it.
    /// `0` on v8, whose page AAD has no generation.
    pub(super) fn write_gen(&self) -> u64 {
        if self.is_v9() {
            self.header.wal_seq + 1
        } else {
            0
        }
    }

    /// FMT-9 (a): the oldest retained snapshot's transaction id
    /// (`u64::MAX` when none is retained). Only pages freed at or before it
    /// may be reused.
    pub(super) fn min_snapshot_txn(&self) -> u64 {
        self.manifest
            .iter()
            .map(|s| s.txn_id)
            .min()
            .unwrap_or(u64::MAX)
    }

    /// Allocate `n` contiguous pages. v9 takes them from the free list when
    /// an eligible run fits (freed by a committed transaction, and not
    /// referenced by a retained snapshot); otherwise, and always on v8, the
    /// file grows.
    pub(super) fn alloc_pages(&mut self, n: u64) -> u64 {
        if self.is_v9() {
            let committed = self.header.wal_seq;
            let min_snap = self.min_snapshot_txn();
            if let Some(p) = self.free.alloc(n, committed, min_snap) {
                self.free_dirty = true;
                return p;
            }
        }
        let p = self.header.next_page;
        self.header.next_page += n;
        p
    }

    /// FMT-9 (a): record that the current transaction stopped referencing
    /// `n` pages from `page`. The next commit frees them. No-op on v8,
    /// which never reuses a page.
    pub(super) fn release_pages(&mut self, page: u64, n: u64) {
        if self.is_v9() && n > 0 && page != 0 {
            self.pending_free.push((page, n));
        }
    }

    /// FMT-9d: make sure the live-document stats and the segment dicts are
    /// loaded. Stats come from the rows; dicts from each segment's head.
    pub(super) fn ensure_bm25(&mut self) -> Result<()> {
        self.ensure_bm25_stats();
        self.ensure_bm25_dicts()
    }

    /// FMT-9d: the live-document stats, from the rows (once per handle, or
    /// after a catalog rebase moves rows; `put` and `delete` keep them
    /// current, FMT-9f).
    pub(super) fn ensure_bm25_stats(&mut self) {
        if self.bm25.stats.is_none() {
            let mut docs = std::collections::HashMap::new();
            let mut total = 0u64;
            for (i, e) in self.catalog.iter().enumerate() {
                if e.deleted {
                    continue;
                }
                if let Some(r) = e.v9 {
                    if r.doc != 0 && bm25_kind(r.kind) {
                        docs.insert(r.doc, (i, r.doc_len));
                        total += r.doc_len as u64;
                    }
                }
            }
            let mut stats = DocStats {
                docs,
                avgdl: 1.0,
                total,
            };
            stats.refresh();
            self.bm25.stats = Some(stats);
        }
    }

    /// FMT-9d: the segment dicts, read from each segment's head once; after
    /// that a commit adds or replaces only the segments it wrote (FMT-9f).
    pub(super) fn ensure_bm25_dicts(&mut self) -> Result<()> {
        if self.bm25.dicts.is_none() {
            let mut dicts = Vec::with_capacity(self.bm25.manifest.segments.len());
            for &t in &self.bm25.manifest.segments {
                dicts.push(load_segment_dict(&mut self.pager, tuple_ptr(t))?);
            }
            self.bm25.dicts = Some(dicts);
        }
        Ok(())
    }

    /// FMT-9d: BM25 scores of the live keyword-indexed rows that match
    /// `query`: catalog row → score. Empty on v8 (no keyword index).
    pub(super) fn bm25_scores(
        &mut self,
        query: &str,
        k1: f32,
        b: f32,
    ) -> Result<std::collections::HashMap<usize, f32>> {
        let mut scores: std::collections::HashMap<usize, f32> = std::collections::HashMap::new();
        if !self.is_v9() {
            return Ok(scores);
        }
        self.ensure_bm25()?;
        let terms: std::collections::BTreeSet<String> = text::tokenize(query).into_iter().collect();
        let segs: Vec<RunPtr> = self
            .bm25
            .manifest
            .segments
            .iter()
            .map(|&t| tuple_ptr(t))
            .collect();
        let stats = self.bm25.stats.as_ref().expect("loaded above");
        let dicts = self.bm25.dicts.as_ref().expect("loaded above");
        let n = stats.docs.len();
        for term in &terms {
            let mut posts: Vec<(u32, u8)> = Vec::new();
            for (si, seg) in segs.iter().enumerate() {
                posts.extend(segment_postings(&mut self.pager, *seg, &dicts[si], term)?);
            }
            for (doc, tfs) in &self.bm25.pending {
                if let Ok(i) = tfs.binary_search_by(|(t, _)| t.as_str().cmp(term)) {
                    posts.push((*doc, tfs[i].1));
                }
            }
            posts.retain(|(d, _)| stats.docs.contains_key(d));
            if posts.is_empty() {
                continue;
            }
            let idf = text::idf(n, posts.len());
            for (d, tf) in posts {
                let (row, len) = stats.docs[&d];
                *scores.entry(row).or_default() += text::bm25(tf, idf, len, stats.avgdl, k1, b);
            }
        }
        Ok(scores)
    }

    /// FMT-9d: the next commit's BM25 segment: the pending documents, plus,
    /// past [`BM25_MAX_SEGMENTS`], the small segments (all but the first),
    /// or all of them once the small ones outgrow a tenth of the first.
    /// Dead documents are dropped from merged segments. Returns the segment
    /// bytes and, for a merge, the index from which it replaces the
    /// segments (`None`: it's appended).
    pub(super) fn bm25_next_segment(&mut self) -> Result<Option<(Vec<u8>, Option<usize>)>> {
        if !self.is_v9() || self.bm25.pending.is_empty() {
            return Ok(None);
        }
        let segs: Vec<RunPtr> = self
            .bm25
            .manifest
            .segments
            .iter()
            .map(|&t| tuple_ptr(t))
            .collect();
        // Mostly dead (a mass delete): merge everything, so the dead
        // postings go and the old segments' pages can be reused or cut off.
        // FMT-9f: only the stats; the dicts aren't needed to write a segment.
        self.ensure_bm25_stats();
        let written: u64 = self.bm25.manifest.seg_docs.iter().map(|&d| d as u64).sum();
        let live_docs = self.bm25.stats.as_ref().map_or(0, |s| s.docs.len()) as u64;
        let mostly_dead = !segs.is_empty() && written > 64 && live_docs * 2 < written;
        if segs.len() < BM25_MAX_SEGMENTS && !mostly_dead {
            return Ok(Some((text::build_segment(&self.bm25.pending)?, None)));
        }
        let small: u64 = segs.iter().skip(1).map(|p| p.len).sum();
        let from = if mostly_dead || small * 10 > segs[0].len {
            0
        } else {
            1
        };
        let live: std::collections::HashSet<u32> = self
            .bm25
            .stats
            .as_ref()
            .expect("loaded above")
            .docs
            .keys()
            .copied()
            .collect();
        let mut by_doc: std::collections::BTreeMap<u32, Vec<(String, u8)>> =
            std::collections::BTreeMap::new();
        for seg in &segs[from..] {
            let bytes = read_run_bytes(&mut self.pager, seg.start, seg.pages, seg.len, seg.gen)?;
            let n = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
            let dict: Vec<text::DictEntry> = rmp_serde::from_slice(&bytes[4..4 + n])
                .map_err(|e| MnemoError::Serialize(e.to_string()))?;
            let post = &bytes[4 + n..];
            for e in dict {
                let range = e.off as usize..(e.off + e.len) as usize;
                let list = text::decode_postings(post.get(range).ok_or_else(|| {
                    MnemoError::Invalid("BM25 dict entry past its postings".into())
                })?)?;
                for (d, tf) in list {
                    if live.contains(&d) {
                        by_doc.entry(d).or_default().push((e.term.clone(), tf));
                    }
                }
            }
        }
        for (d, tfs) in &self.bm25.pending {
            if live.contains(d) {
                by_doc.insert(*d, tfs.clone());
            }
        }
        let docs: Vec<(u32, Vec<(String, u8)>)> = by_doc.into_iter().collect();
        self.bm25.merged_docs = docs.len() as u32;
        Ok(Some((text::build_segment(&docs)?, Some(from))))
    }

    /// FMT-9c.3: the generation the persisted index is current to: its
    /// base run's, or its newest delta run's.
    pub(super) fn index_gen_now(&self) -> u64 {
        self.header.index_gen.max(self.header.index_delta.gen)
    }

    /// FMT-9c.3: should this v9 flush persist the index, and how? Every
    /// commit that changed it writes those changes as a delta run (so the
    /// index is never stale after a crash); a build, rebuild, drop or
    /// restore, or a chain past 64 runs or a quarter of the index, writes
    /// the whole index instead. Sets `index_delta_ops` for a delta.
    pub(super) fn plan_index_v9(&mut self) -> bool {
        self.index_delta_ops = None;
        let Some(ann) = self.ann.as_mut() else {
            // Dropped (persisted as "no index"), or not loaded (unchanged).
            return self.dirty_index;
        };
        let ops = ann.take_log();
        let n = ann.len();
        if self.index_rebase {
            return true;
        }
        if ops.is_empty() {
            return false;
        }
        if self.index_chain.len() >= 64 || (self.index_chain_ops + ops.len()) * 4 > n.max(256) {
            return true;
        }
        self.index_delta_ops = Some(ops);
        true
    }

    /// FMT-9c.2: catalog row `idx` changed (v9 writes it in the next
    /// commit's delta run). No-op on v8, which rewrites the whole catalog.
    pub(super) fn touch(&mut self, idx: usize) {
        if self.is_v9() {
            self.dirty_rows.insert(idx);
        }
    }

    /// FMT-9c.2: write this commit's catalog change as a delta run on the
    /// chain, rather than a new base? Not after a restore, not before the
    /// first base, and not once the chain has 64 runs or holds more than a
    /// quarter of the base's rows (the next base then folds it in).
    pub(super) fn catalog_as_delta(&self) -> bool {
        self.is_v9()
            && !self.catalog_rewrite
            && self.header.catalog_pages > 0
            && self.delta_chain.len() < 64
            && (self.delta_rows + self.dirty_rows.len()) * 4 <= self.catalog.len()
    }

    /// FMT-9c.2: before writing a new base, drop deleted rows once they're
    /// more than a quarter of the catalog. Rows move, so the id index and
    /// the page maps are rebuilt, and the resident column reloads lazily.
    /// Runs inside a flush, after packing, when nothing is waiting.
    pub(super) fn compact_tombstones(&mut self) {
        let dead = self.catalog.iter().filter(|e| e.deleted).count();
        if dead == 0 || dead * 4 <= self.catalog.len() {
            return;
        }
        self.catalog.retain(|e| !e.deleted);
        self.catalog.shrink_to_fit();
        self.index = build_id_index(&self.catalog);
        self.col = VecColumn::from_catalog(&self.catalog);
        self.recs = RecPages::from_catalog(&self.catalog);
        self.dirty_rows.clear();
        self.bm25.stats = None;
    }

    /// FMT-9b: the file keeps float vectors in its records.
    pub(super) fn float_vectors(&self) -> bool {
        self.is_v9() && self.header.flags & FLAG_FLOAT_VECTORS != 0
    }

    /// FMT-9b: quantize `v` into the column for catalog row `idx` (written
    /// by the next flush).
    pub(super) fn stage_vector(&mut self, idx: usize, v: &[f32]) {
        let (h, norm) = veccol::quantize(v);
        let d = self.dimensions;
        if self.col.resident {
            let rows = self.catalog.len();
            let fits = rows * veccol::entry_len(d) <= self.vector_cache_budget;
            if fits {
                if self.col.data.len() < rows * d {
                    self.col.data.resize(rows * d, 0);
                    self.col.norms.resize(rows, 0.0);
                    self.col.have.resize(rows, false);
                }
                self.col.data[idx * d..(idx + 1) * d].copy_from_slice(&h);
                self.col.norms[idx] = norm;
                self.col.have[idx] = true;
            } else {
                // Grown past the budget: page the column from now on.
                self.col.data = Vec::new();
                self.col.norms = Vec::new();
                self.col.have = Vec::new();
                self.col.resident = false;
            }
        }
        self.col.hot.insert(idx, (h, norm));
        self.col.pending.push(idx);
        self.touch(idx);
        if let Some(r) = self.catalog[idx].v9.as_mut() {
            r.vec_page = 0;
            r.vec_slot = 0;
            r.vec_gen = 0;
        }
    }

    /// FMT-9b: row `idx` no longer needs its column entry (`row` is the row
    /// as it was): drop a staged one, or take it off its page and free the
    /// page once nothing live is left on it.
    pub(super) fn kill_vector(&mut self, idx: usize, row: &CatalogEntry) {
        self.col.hot.remove(&idx);
        let Some(r) = row.v9 else { return };
        if r.vec_page == 0 {
            return;
        }
        let empty = match self.col.page_rows.get_mut(&r.vec_page) {
            Some(rows) => {
                rows.retain(|&i| i != idx);
                rows.is_empty()
            }
            None => false,
        };
        if empty {
            self.col.page_rows.remove(&r.vec_page);
            self.release_pages(r.vec_page, 1);
        }
    }

    /// FMT-9b.2: row `idx` no longer needs its record (`row` is the row as
    /// it was): drop a waiting one, free a span, or take it off its packed
    /// page and free the page once nothing live is left on it.
    /// FMT-9f: the rerank depth for an ANN query: the caller's pin, else
    /// on v9 at least [`V9_RERANK_PER_HIT`] × `top_k`, else the index's
    /// own default (`None`).
    pub(super) fn rerank_for(&self, ann: &IvfPqIndex, req: &RecallRequest) -> Option<usize> {
        if req.n_rerank.is_some() || !self.is_v9() {
            return req.n_rerank;
        }
        let (_, auto) = ann.effective_defaults();
        Some(auto.max(V9_RERANK_PER_HIT.saturating_mul(req.top_k)))
    }

    /// FMT-9e: is a turn created at `created_at` past this handle's
    /// compression cutoff?
    pub(super) fn turn_is_cold(&self, created_at: i64) -> bool {
        self.turn_compress_after == 0
            || created_at <= memory::now_secs().saturating_sub(self.turn_compress_after)
    }

    /// FMT-9e: is row `idx` a live, unsettled v9 turn?
    pub(super) fn unsettled_turn(&self, idx: usize) -> bool {
        let e = &self.catalog[idx];
        !e.deleted
            && e.v9
                .is_some_and(|r| r.kind == ROW_KIND_TURN && r.flags & ROW_SETTLED == 0)
    }

    /// FMT-9e: settle row `idx`'s turn record, whose stored bytes are
    /// `bytes`: compress them if it pays, and mark the row settled.
    /// Returns the bytes to store (unchanged if not worth compressing);
    /// the caller writes them. Updates the row's length.
    pub(super) fn settle_turn(&mut self, idx: usize, bytes: Vec<u8>) -> Vec<u8> {
        let out = if bytes.starts_with(&ZSTD_MAGIC) {
            bytes
        } else {
            compress_turn_record(bytes)
        };
        let e = &mut self.catalog[idx];
        e.len = out.len() as u32;
        if let Some(r) = e.v9.as_mut() {
            r.flags |= ROW_SETTLED;
        }
        self.touch(idx);
        self.dirty_catalog = true;
        out
    }

    /// FMT-9e: compress up to [`COLD_TURNS_PER_COMMIT`] plain turns that
    /// have aged past the cutoff. Runs at the top of a flush that's
    /// already writing, before [`Mnemo::pack_records`], which packs the
    /// rewritten records.
    pub(super) fn settle_cold_turns(&mut self) -> Result<()> {
        if !self.is_v9() || !self.dirty_catalog {
            return Ok(());
        }
        let picked: Vec<usize> = (0..self.catalog.len())
            .filter(|&i| {
                self.unsettled_turn(i)
                    && self.catalog[i]
                        .v9
                        .is_some_and(|r| self.turn_is_cold(r.created_at))
            })
            .take(COLD_TURNS_PER_COMMIT)
            .collect();
        for idx in picked {
            let old = self.catalog[idx].clone();
            let bytes = self.read_record(&old)?;
            let before = bytes.len();
            let out = self.settle_turn(idx, bytes);
            if out.len() == before {
                continue;
            }
            self.kill_record(idx, &old);
            let gen = self.write_gen();
            let e = &mut self.catalog[idx];
            if out.len() + REC_SLOT_BYTES <= REC_PAGE_CAP {
                e.start_page = 0;
                e.page_count = 1;
                e.gen = gen;
                if let Some(r) = e.v9.as_mut() {
                    r.rec_slot = 0;
                }
                self.recs.hot.insert(idx, out);
                self.recs.pending.push(idx);
            } else {
                let (start, pc, gen) = self.write_record(&out)?;
                let e = &mut self.catalog[idx];
                e.start_page = start;
                e.page_count = pc;
                e.gen = gen;
                if let Some(r) = e.v9.as_mut() {
                    r.rec_slot = REC_SPAN;
                }
            }
        }
        Ok(())
    }

    pub(super) fn kill_record(&mut self, idx: usize, row: &CatalogEntry) {
        self.recs.hot.remove(&idx);
        let Some(r) = row.v9 else { return };
        if r.rec_slot == REC_SPAN {
            self.release_pages(row.start_page, row.page_count as u64);
            return;
        }
        if row.start_page == 0 {
            return;
        }
        let empty = match self.recs.page_rows.get_mut(&row.start_page) {
            Some(rows) => {
                rows.retain(|&i| i != idx);
                rows.is_empty()
            }
            None => false,
        };
        if empty {
            self.recs.page_rows.remove(&row.start_page);
            self.release_pages(row.start_page, 1);
        }
    }

    /// FMT-9b.2: pack the waiting records, plus the live records of up to
    /// [`VACUUM_PAGES_PER_COMMIT`] sparse pages (under 3/4 full), into
    /// fresh record pages, filling each before starting the next. Runs at
    /// the top of a flush, before the catalog is serialized (it moves rows'
    /// record locations), and only when something is waiting.
    pub(super) fn pack_records(&mut self) -> Result<()> {
        if !self.is_v9() || self.recs.pending.is_empty() {
            return Ok(());
        }
        let mut sparse: Vec<(u64, usize)> = self
            .recs
            .page_rows
            .iter()
            .filter(|(_, rows)| {
                let live: usize = rows
                    .iter()
                    .map(|&i| self.catalog[i].len as usize + REC_SLOT_BYTES)
                    .sum();
                live * 4 < REC_PAGE_CAP * 3
            })
            .map(|(&p, rows)| (p, rows.len()))
            .collect();
        sparse.sort_unstable();
        let sparse = vacuum_pick(sparse);
        let mut order: Vec<usize> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for p in sparse {
            let rows = self.recs.page_rows.remove(&p).unwrap_or_default();
            if let Some(&first) = rows.first() {
                let page = self.pager.read_page_uncached(p, self.catalog[first].gen)?;
                for idx in rows {
                    let slot = self.catalog[idx].v9.map_or(0, |r| r.rec_slot);
                    let mut b = record_slot(&page, slot)?;
                    // FMT-9e: a vacuum that moves a plain turn compresses it.
                    if self.unsettled_turn(idx) {
                        b = self.settle_turn(idx, b);
                    }
                    self.recs.hot.insert(idx, b);
                    if seen.insert(idx) {
                        order.push(idx);
                    }
                }
            }
            self.release_pages(p, 1);
        }
        let pending = std::mem::take(&mut self.recs.pending);
        for idx in pending {
            if self.recs.hot.contains_key(&idx) && seen.insert(idx) {
                order.push(idx);
            }
        }
        let gen = self.write_gen();
        let mut i = 0;
        while i < order.len() {
            let mut used = 0usize;
            let mut j = i;
            while j < order.len() {
                let l = self.recs.hot[&order[j]].len() + REC_SLOT_BYTES;
                if j > i && used + l > REC_PAGE_CAP {
                    break;
                }
                used += l;
                j += 1;
            }
            let page = self.alloc_pages(1);
            let bytes = {
                let recs: Vec<&[u8]> = order[i..j]
                    .iter()
                    .map(|k| self.recs.hot[k].as_slice())
                    .collect();
                encode_record_page(gen, &recs)
            };
            self.pager.write_page(page, gen, &bytes)?;
            for (slot, &k) in order[i..j].iter().enumerate() {
                self.touch(k);
                let e = &mut self.catalog[k];
                e.start_page = page;
                e.page_count = 1;
                e.gen = gen;
                if let Some(r) = e.v9.as_mut() {
                    r.rec_slot = slot as u16;
                }
            }
            self.recs.page_rows.insert(page, order[i..j].to_vec());
            i = j;
        }
        for k in order {
            self.recs.hot.remove(&k);
        }
        self.dirty_catalog = true;
        Ok(())
    }

    /// FMT-9b: row `e`'s column entry (fp16 values and norm), from the
    /// staged set, the resident column, or its page.
    pub(super) fn column_entry(&mut self, e: &CatalogEntry) -> Result<(Vec<u16>, f32)> {
        let d = self.dimensions;
        if let Some(idx) = self.index.get(e.id, &self.catalog) {
            if let Some((h, n)) = self.col.hot.get(&idx) {
                return Ok((h.clone(), *n));
            }
            if self.col.resident && self.col.have.get(idx).copied().unwrap_or(false) {
                return Ok((
                    self.col.data[idx * d..(idx + 1) * d].to_vec(),
                    self.col.norms[idx],
                ));
            }
        }
        let r =
            e.v9.ok_or_else(|| MnemoError::Invalid("v9 row without a vector entry".into()))?;
        if r.vec_page == 0 {
            return Err(MnemoError::Invalid("vector entry not written yet".into()));
        }
        let page = self.pager.read_page(r.vec_page, r.vec_gen)?;
        veccol::decode_entry(&page, d, r.vec_slot as usize)
    }

    /// FMT-9b: make the column resident if it fits the budget: allocate it
    /// (zeroed, so untouched rows cost no memory) and copy in the staged
    /// entries. Rows fill a vector page at a time on first use
    /// ([`Mnemo::fill_column_page`]). A no-op once resident, and on v8.
    pub(super) fn ensure_column(&mut self) -> Result<()> {
        if !self.is_v9() || self.col.resident {
            return Ok(());
        }
        let d = self.dimensions;
        let rows = self.catalog.len();
        if rows * veccol::entry_len(d) > self.vector_cache_budget {
            return Ok(());
        }
        self.col.data = vec![0u16; rows * d];
        self.col.norms = vec![0f32; rows];
        self.col.have = vec![false; rows];
        let hot: Vec<(usize, Vec<u16>, f32)> = self
            .col
            .hot
            .iter()
            .filter(|(&i, _)| i < rows)
            .map(|(&i, (h, n))| (i, h.clone(), *n))
            .collect();
        for (i, h, n) in hot {
            self.col.data[i * d..(i + 1) * d].copy_from_slice(&h);
            self.col.norms[i] = n;
            self.col.have[i] = true;
        }
        self.col.resident = true;
        Ok(())
    }

    /// FMT-9b: fill the resident column from row `idx`'s vector page: every
    /// live row on that page, in one decrypt that bypasses the page cache.
    pub(super) fn fill_column_page(&mut self, idx: usize) -> Result<()> {
        let Some(r) = self.catalog[idx].v9 else {
            return Ok(());
        };
        if r.vec_page == 0 {
            return Ok(());
        }
        let d = self.dimensions;
        let bytes = self.pager.read_page_uncached(r.vec_page, r.vec_gen)?;
        let rows = self
            .col
            .page_rows
            .get(&r.vec_page)
            .cloned()
            .unwrap_or_default();
        for i in rows.into_iter().chain(std::iter::once(idx)) {
            if i >= self.col.have.len() || self.col.have[i] {
                continue;
            }
            let Some(ri) = self.catalog[i].v9 else {
                continue;
            };
            if ri.vec_page != r.vec_page {
                continue;
            }
            let n = veccol::decode_entry_into(
                &bytes,
                d,
                ri.vec_slot as usize,
                &mut self.col.data[i * d..(i + 1) * d],
            )?;
            self.col.norms[i] = n;
            self.col.have[i] = true;
        }
        Ok(())
    }

    /// FMT-9b: write the staged column entries, plus the live entries of up
    /// to [`VACUUM_PAGES_PER_COMMIT`] sparse pages (under 3/4 full), into
    /// fresh vector pages. Runs at the top of a flush, before the catalog
    /// is serialized, since it moves rows' vector locations. Only when
    /// something is staged, so a flush with no new vectors writes none.
    pub(super) fn pack_vectors(&mut self) -> Result<()> {
        if !self.is_v9() || self.col.pending.is_empty() {
            return Ok(());
        }
        let d = self.dimensions;
        let per = veccol::per_page(d);
        let mut sparse: Vec<(u64, usize)> = self
            .col
            .page_rows
            .iter()
            .filter(|(_, rows)| rows.len() * 4 < per * 3)
            .map(|(&p, rows)| (p, rows.len()))
            .collect();
        sparse.sort_unstable();
        let sparse = vacuum_pick(sparse);
        let mut order: Vec<usize> = Vec::new();
        let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
        for p in sparse {
            let rows = self.col.page_rows.remove(&p).unwrap_or_default();
            for idx in rows {
                let e = self.catalog[idx].clone();
                let v = self.column_entry(&e)?;
                self.col.hot.insert(idx, v);
                if seen.insert(idx) {
                    order.push(idx);
                }
            }
            self.release_pages(p, 1);
        }
        let pending = std::mem::take(&mut self.col.pending);
        for idx in pending {
            if self.col.hot.contains_key(&idx) && seen.insert(idx) {
                order.push(idx);
            }
        }
        let gen = self.write_gen();
        for chunk in order.chunks(per) {
            let page = self.alloc_pages(1);
            let bytes = {
                let entries: Vec<(&[u16], f32)> = chunk
                    .iter()
                    .map(|i| {
                        let (h, n) = &self.col.hot[i];
                        (h.as_slice(), *n)
                    })
                    .collect();
                veccol::encode_page(d, gen, &entries)
            };
            self.pager.write_page(page, gen, &bytes)?;
            for (slot, &i) in chunk.iter().enumerate() {
                self.touch(i);
                if let Some(r) = self.catalog[i].v9.as_mut() {
                    r.vec_page = page;
                    r.vec_slot = slot as u16;
                    r.vec_gen = gen;
                }
            }
            self.col.page_rows.insert(page, chunk.to_vec());
        }
        for i in order {
            self.col.hot.remove(&i);
        }
        self.dirty_catalog = true;
        Ok(())
    }

    /// FMT-9: before this session seals its first v9 page, commit a
    /// header-only transaction that bumps `wal_seq`.
    ///
    /// A page's generation is the id of the transaction that writes it,
    /// and that id is `wal_seq + 1`, which is only made durable by the
    /// commit. If a session crashes before committing, its pages (orphan
    /// data pages, or uncommitted WAL frames) carry an id the next session
    /// would hand out again, for the same pages (allocation is
    /// deterministic). An attacker holding a copy of the crashed file could
    /// then swap an orphan back in under a matching generation. Burning
    /// one id durably per writable session rules that out: every id this
    /// session uses is above every id any earlier session used. Costs one
    /// small commit per session, on its first write only.
    pub(super) fn ensure_gen_lease(&mut self) -> Result<()> {
        if !self.is_v9() || self.gen_leased || self.read_only {
            return Ok(());
        }
        self.header.wal_seq += 1;
        let wal_start = self.header.wal_start;
        self.relocate_header_through_wal(wal_start)?;
        self.gen_leased = true;
        Ok(())
    }

    /// Write a serialized record (memory or cache entry) to fresh pages.
    /// Returns `(start_page, page_count, generation)`.
    pub(super) fn write_record(&mut self, bytes: &[u8]) -> Result<(u64, u32, u64)> {
        self.ensure_gen_lease()?;
        let pc = bytes.len().div_ceil(PAYLOAD).max(1);
        let start = self.alloc_pages(pc as u64);
        let gen = self.write_gen();
        for i in 0..pc {
            let lo = i * PAYLOAD;
            let hi = ((i + 1) * PAYLOAD).min(bytes.len());
            self.pager
                .write_page(start + i as u64, gen, &bytes[lo..hi])?;
        }
        Ok((start, pc as u32, gen))
    }

    /// Read a record run of `pages` pages at `start`, sealed under `gen`.
    pub(super) fn read_record_run(
        &mut self,
        start: u64,
        pages: u32,
        len: u32,
        gen: u64,
    ) -> Result<Vec<u8>> {
        let mut buf = Vec::with_capacity(len as usize);
        for i in 0..pages as u64 {
            buf.extend_from_slice(&self.pager.read_page(start + i, gen)?);
        }
        buf.truncate(len as usize);
        Ok(buf)
    }

    pub(super) fn read_record(&mut self, e: &CatalogEntry) -> Result<Vec<u8>> {
        // FMT-9b.2: a packed v9 record is one slot of a shared page, or
        // still waiting for the next flush.
        if let Some(r) = e.v9 {
            if r.rec_slot != REC_SPAN {
                if e.start_page == 0 {
                    return self
                        .index
                        .get(e.id, &self.catalog)
                        .and_then(|idx| self.recs.hot.get(&idx).cloned())
                        .ok_or_else(|| MnemoError::Invalid("record not written yet".into()));
                }
                let page = self.pager.read_page(e.start_page, e.gen)?;
                return record_slot(&page, r.rec_slot);
            }
        }
        let mut buf = Vec::with_capacity(e.len as usize);
        for i in 0..e.page_count as u64 {
            buf.extend_from_slice(&self.pager.read_page(e.start_page + i, e.gen)?);
        }
        buf.truncate(e.len as usize);
        Ok(buf)
    }

    /// Decode a stored memory record body. FMT-9e: on v9 it may be a zstd
    /// frame (a compressed turn); v8 records are always plain.
    pub(super) fn decode_memory(&self, bytes: &[u8]) -> Result<Memory> {
        let plain = if self.is_v9() {
            plain_record(bytes)?
        } else {
            std::borrow::Cow::Borrowed(bytes)
        };
        rmp_serde::from_slice(&plain).map_err(|err| MnemoError::Serialize(err.to_string()))
    }

    pub(super) fn read_memory(&mut self, e: &CatalogEntry) -> Result<Memory> {
        let m = self.read_memory_meta(e)?;
        self.finish_memory(e, m)
    }

    /// Complete a record decoded by [`Mnemo::read_memory_meta`]: its
    /// vector and the catalog's live access stats.
    pub(super) fn finish_memory(&mut self, e: &CatalogEntry, mut m: Memory) -> Result<Memory> {
        // FMT-9b: a v9 record carries no vector (unless the file keeps
        // float vectors); it comes from the column, fp16-rounded.
        if e.v9.is_some() && !self.float_vectors() {
            let (h, _) = self.column_entry(e)?;
            m.vector = veccol::dequantize(&h);
        }
        // The catalog is the source of truth for access stats — the values
        // in the record body are a stale snapshot from when the record was
        // last written. Overwrite them here so consumers always see live
        // values regardless of when the record was last serialized.
        m.accessed_at = e.accessed_at;
        m.access_count = e.access_count;
        Ok(m)
    }

    /// Serialize and store a memory (insert or overwrite by ID).
    pub(super) fn put(&mut self, mut m: Memory) -> Result<Ulid> {
        if m.vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: m.vector.len(),
            });
        }
        // Reject all-zero vectors (Phase 3 PR 10a; breaking vs 0.4.x).
        // A zero vector scores exactly 0.0 on every cosine query — it
        // can't contribute signal, but recency / importance can still
        // pull it into top-k as noise. We'd rather fail loudly at
        // `remember` than let a mis-embedded entry pollute recall
        // quality for the life of the database. Scaffold manifests now
        // use a sentinel unit vector so this check doesn't trip them.
        if embed::is_zero_vector(&m.vector) {
            return Err(MnemoError::Invalid(
                "all-zero vector refused: it scores 0.0 on every cosine query and \
                 pollutes recall; check your embedder returned a real embedding"
                    .into(),
            ));
        }
        if m.id == Ulid::nil() {
            m.id = Ulid::new();
        }
        let id_u: u128 = m.id.0;
        // R-004.2: an overwrite with the handle caches built reads the
        // stored version first (one decrypt), so the caches drop exactly
        // what it contributed.
        if self.layers.is_some() {
            if let Some(idx) = self.index.get(id_u, &self.catalog) {
                let old_entry = self.catalog[idx].clone();
                if !old_entry.deleted {
                    let old = self.read_memory(&old_entry)?;
                    self.note_removed(&old)?;
                }
            }
        }
        // FMT-9b: a v9 record is stored without its vector (unless the file
        // keeps float vectors); the vector goes to the fp16 column.
        let v9 = self.is_v9();
        let bytes = if v9 && !self.float_vectors() {
            let vector = std::mem::take(&mut m.vector);
            let b = rmp_serde::to_vec(&m);
            m.vector = vector;
            b
        } else {
            rmp_serde::to_vec(&m)
        }
        .map_err(|e| MnemoError::Serialize(e.to_string()))?;
        // FMT-9e: a v9 thread turn is stored zstd-compressed (turns are
        // most of a thread-heavy file). The catalog's `len` is the stored
        // length, so packing and vacuum move the frame as is.
        // FMT-9e (compress when cold): a turn is compressed at write only
        // when it's already past the handle's cutoff (an imported
        // archive, or `set_turn_compress_after(0)`); otherwise the commit
        // that sees it age compresses it.
        let is_turn = v9 && meta_str(&m, MNEMO_META_KIND) == Some(MNEMO_META_KIND_TURN);
        let settle =
            is_turn && (bytes.len() < TURN_ZSTD_MIN_BYTES || self.turn_is_cold(m.created_at));
        let bytes = if settle {
            compress_turn_record(bytes)
        } else {
            bytes
        };
        // FMT-9b.2: a v9 record that fits a packed page waits for the next
        // flush, which packs it with others; a bigger one owns a run of
        // whole pages, written now as on v8.
        let packed = v9 && bytes.len() + REC_SLOT_BYTES <= REC_PAGE_CAP;
        // A waiting record has `start_page` 0. Its generation is never 0 on
        // v9: the row encoding leaves out a zero `gen`, which would shift
        // the `v9` field into its place (a deleted row can stay unpacked).
        let (start, pc, gen) = if packed {
            self.ensure_gen_lease()?;
            (0, 1, self.write_gen())
        } else {
            self.write_record(&bytes)?
        };
        let rec_len = bytes.len();
        // FMT-9d: a keyword-indexed v9 row gets a new BM25 document.
        let mut tags = v9.then(|| row_tags(&m));
        if let Some(r) = tags.as_mut() {
            if bm25_kind(r.kind) {
                let (terms, len) = text::term_freqs(&m.content);
                r.doc = self.bm25.manifest.next_doc;
                r.doc_len = len;
                self.bm25.manifest.next_doc += 1;
                self.bm25.pending.push((r.doc, terms));
            }
        }
        // FMT-9f: keep the BM25 stats current (below, once the row is
        // placed) instead of rescanning every row at the next commit.
        let bm25_new = tags.filter(|r| r.doc != 0 && bm25_kind(r.kind));
        // For overwrites preserve the existing access stats; for inserts
        // seed them from the memory (which carries them in its body).
        let (prev_accessed, prev_count) = match self.index.get(id_u, &self.catalog) {
            Some(idx) => (
                self.catalog[idx].accessed_at,
                self.catalog[idx].access_count,
            ),
            None => (m.accessed_at, m.access_count),
        };
        let entry = CatalogEntry {
            id: id_u,
            start_page: start,
            page_count: pc,
            len: rec_len as u32,
            deleted: false,
            accessed_at: prev_accessed,
            access_count: prev_count,
            gen,
            v9: tags.map(|mut r| {
                r.rec_slot = if packed { 0 } else { REC_SPAN };
                if settle {
                    r.flags |= ROW_SETTLED;
                }
                r
            }),
        };
        match self.index.get(id_u, &self.catalog) {
            Some(idx) => {
                // FMT-9 (a): the overwritten version's pages are freed by
                // this transaction's commit. A tombstone's pages were
                // already freed when its delete committed.
                if !self.catalog[idx].deleted {
                    let old = self.catalog[idx].clone();
                    if v9 {
                        self.kill_vector(idx, &old);
                        self.kill_record(idx, &old);
                    } else {
                        self.release_pages(old.start_page, old.page_count as u64);
                    }
                }
                if let (Some(stats), Some(old)) = (self.bm25.stats.as_mut(), self.catalog[idx].v9) {
                    if !self.catalog[idx].deleted && old.doc != 0 {
                        stats.remove(old.doc);
                    }
                }
                self.catalog[idx] = entry;
                self.touch(idx);
            }
            None => {
                self.index.insert(id_u, self.catalog.len(), &self.catalog);
                self.touch(self.catalog.len());
                self.catalog.push(entry);
            }
        }
        if let (Some(stats), Some(r)) = (self.bm25.stats.as_mut(), bm25_new) {
            if let Some(idx) = self.index.get(id_u, &self.catalog) {
                stats.insert(r.doc, idx, r.doc_len);
            }
        }
        self.dirty_catalog = true;
        if v9 {
            if let Some(idx) = self.index.get(id_u, &self.catalog) {
                self.stage_vector(idx, &m.vector);
                if packed {
                    self.recs.hot.insert(idx, bytes);
                    self.recs.pending.push(idx);
                }
            }
        }

        // Keep the ANN index complete: assign the (new or changed) vector to
        // its nearest partition. Centroids/codebook stay fixed until rebuild.
        // R-004.2: only facts go in; turns, essence items and thread links
        // are read by kind and scope.
        if let Some(ann) = &mut self.ann {
            if is_ann_kind(&m) {
                ann.add(id_u, &m.vector);
                self.dirty_index = true;
                self.index_persist.changes += 1;
            } else if ann.contains(id_u) {
                ann.remove(id_u);
                self.dirty_index = true;
                self.index_persist.changes += 1;
            }
        }
        self.note_added(&m)?;

        // FIX-1 R-001.8 part 2: keep the lazy agent set in sync. If
        // it hasn't been built yet (no `stats()` call this session),
        // skip — the first `stats()` will build it from scratch. If
        // it has, add this memory's agent_id. An overwrite with the
        // same id and same agent is a no-op (BTreeSet::insert returns
        // false); an overwrite with a *changed* agent_id adds the new
        // agent but doesn't drop the old (we can't tell without a
        // decrypt whether the old agent still has live memories).
        // That's conservative in the right direction — `stats()` may
        // over-report by one agent until the next delete invalidates.
        if let Some(set) = &mut self.agent_set {
            set.insert(m.agent_id.clone());
        }
        Ok(m.id)
    }

    // --- public CRUD ------------------------------------------------------
}
