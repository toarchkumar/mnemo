//! The approximate (IVF+PQ) index: build, rebuild, drop, lazy load and reconcile.

use super::*;

impl Mnemo {
    /// Build an IVF+PQ approximate-nearest-neighbour index over every live
    /// memory, using default tuning. After this, [`Mnemo::recall`] runs the
    /// tiered pipeline instead of an exact scan. Persisted on the next
    /// `flush`. Returns a snapshot of the index shape.
    pub fn build_index(&mut self) -> Result<IndexInfo> {
        self.build_index_with(IndexConfig::default())
    }

    /// Build the ANN index with explicit tuning.
    pub fn build_index_with(&mut self, cfg: IndexConfig) -> Result<IndexInfo> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // R-004.2: facts only; turns, essence items and thread links stay
        // out of the ANN.
        let mems: Vec<Memory> = self.memories()?.into_iter().filter(is_ann_kind).collect();
        if mems.is_empty() {
            return Err(MnemoError::Invalid(
                "cannot build an index over an empty database".into(),
            ));
        }
        // FMT-9b: a v9 file below V9_ANN_MIN_FACTS recalls by an exact fp16
        // scan, which is faster than the index there and costs no disk.
        // Report it as an index with no partitions, and drop a stale one.
        // `MNEMO_DEV_ANN_MIN_FACTS` overrides the threshold. Unsupported,
        // for tests (and the recall-grid bench) only: it lets the index
        // suites exercise a v9 ANN on small files. Not documented.
        let min_facts = std::env::var("MNEMO_DEV_ANN_MIN_FACTS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(V9_ANN_MIN_FACTS);
        if self.is_v9() && mems.len() < min_facts {
            if self.ann.is_some() || self.ann_unloaded {
                self.ann = None;
                self.ann_unloaded = false;
                self.dirty_index = true;
                self.index_persist.forced = true;
            }
            return Ok(IndexInfo {
                vectors: mems.len(),
                partitions: 0,
                subspaces: 0,
                n_probe: 0,
                n_rerank: 0,
            });
        }
        let items: Vec<(u128, &[f32])> =
            mems.iter().map(|m| (m.id.0, m.vector.as_slice())).collect();
        let mut idx = IvfPqIndex::build(self.dimensions, &items, cfg)?;
        let info = idx.info();
        if self.is_v9() {
            idx.enable_log();
            self.index_rebase = true;
        }
        self.ann = Some(idx);
        self.ann_unloaded = false;
        self.dirty_index = true;
        self.index_persist.forced = true;
        Ok(info)
    }

    /// Rebuild the ANN index from scratch — re-clusters centroids and retrains
    /// the PQ codebook, undoing the cluster drift that accumulates as memories
    /// are inserted against fixed centroids.
    pub fn rebuild_index(&mut self) -> Result<IndexInfo> {
        // ANN-1.1: re-derive every parameter (partitions, subspaces,
        // n_probe, n_rerank) for the current corpus size by passing
        // `IndexConfig::default()`. Previously this freezed the
        // stored `(n_probe, n_rerank)` from the current index,
        // defeating the point of rebuilding on a grown corpus. If
        // you really want to pin specific values, call
        // `build_index_with(IndexConfig { … })` directly.
        self.build_index_with(IndexConfig::default())
    }

    /// Drop the ANN index; recall reverts to exact scans. Persisted on flush.
    /// No-op on a read-only handle (signature is infallible — callers of
    /// `build_index` / `flush` on a read-only handle already get a typed
    /// `ReadOnly` error, which is the enforceable boundary).
    pub fn drop_index(&mut self) {
        if self.read_only {
            return;
        }
        if self.ann.is_some() || self.ann_unloaded {
            self.ann = None;
            self.ann_unloaded = false;
            self.dirty_index = true;
            self.index_persist.forced = true;
        }
    }

    /// PERF-1 (R-004.1): bring a just-loaded ANN index up to date with the
    /// catalog. Called wherever an index run is loaded: on first use after
    /// `open` / `open_read_only` (`ensure_ann`, PERF-1 step 3) and by
    /// `restore_to`, with the first page of that run.
    ///
    /// `flush` persists the index only every so often, so that run can lag
    /// the catalog. It does two things:
    ///
    /// 1. **Prune** every indexed id the catalog no longer has live
    ///    (missing or deleted).
    /// 2. **Re-add** every live record written after the index run. Pages
    ///    are allocated monotonically in v8, and every write, including an
    ///    in-place `update` or `essence_update` (they rewrite the record to
    ///    fresh pages), lands at a page `> index_start`. Those records are
    ///    read and re-assigned. A record of a non-fact kind (turn, essence,
    ///    thread link) is removed instead: those never belong in the ANN.
    ///
    /// The work is bounded by [`INDEX_PERSIST_EVERY_CHANGES`] in the common
    /// case, since a writer persists the index at least that often.
    ///
    /// **v9:** pages are reused, so "a page past the run" no longer means
    /// "written after it". A v9 record is newer than the index run when its
    /// generation is greater than the run's (`index_gen`).
    pub(super) fn reconcile_ann_with_catalog(
        &mut self,
        index_start: u64,
        index_gen: u64,
    ) -> Result<()> {
        if self.ann.is_none() {
            return Ok(());
        }
        let mut changed = 0usize;
        // 1. Prune ids that are no longer live.
        let stale: Vec<u128> = match &self.ann {
            Some(ann) => ann
                .ids()
                .filter(|&id| !matches!(self.index.get(id, &self.catalog), Some(i) if !self.catalog[i].deleted))
                .collect(),
            None => Vec::new(),
        };
        if let Some(ann) = &mut self.ann {
            for id in &stale {
                ann.remove(*id);
                changed += 1;
            }
        }
        // 2. Re-add records written after the index run.
        let v9 = self.is_v9();
        let pending: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted && written_after_index(e, v9, index_start, index_gen))
            .cloned()
            .collect();
        for e in &pending {
            let m = self.read_memory(e)?;
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

    /// Whether the database has an ANN index (loaded, or persisted and
    /// loading on first use).
    pub fn has_index(&self) -> bool {
        self.ann.is_some() || self.ann_unloaded
    }

    /// PERF-1 step 3: load the persisted ANN index on first use, then bring
    /// it up to date with the catalog (`reconcile_ann_with_catalog`) and,
    /// when the handle caches are already built, prune non-fact ids an
    /// older persisted index may hold.
    pub(super) fn ensure_ann(&mut self) -> Result<()> {
        if !self.ann_unloaded {
            return Ok(());
        }
        self.ann_unloaded = false;
        self.ann = load_index(
            &mut self.pager,
            self.header.index_start,
            self.header.index_pages,
            self.header.index_len,
            self.header.index_gen,
            self.dimensions,
        )?;
        // FMT-9c.3: replay the v9 index delta chain, then record changes.
        if self.is_v9() {
            if let Some(ann) = self.ann.as_mut() {
                let (chain, n) = apply_index_chain(&mut self.pager, ann, self.header.index_delta)?;
                self.index_chain = chain;
                self.index_chain_ops = n;
                ann.enable_log();
            } else if self.header.index_pages > 0 {
                // FMT-9f (the 9d leak): an unknown-layout base loads as no
                // index; still track its delta runs, so rebuilding or
                // dropping the index frees them along with the base.
                self.index_chain = index_chain_runs(&mut self.pager, self.header.index_delta);
                self.index_chain_ops = 0;
            }
        }
        let (index_start, index_gen) = (self.header.index_start, self.index_gen_now());
        self.reconcile_ann_with_catalog(index_start, index_gen)?;
        let non_facts: Vec<u128> = match &self.layers {
            Some(l) => l
                .essence
                .iter()
                .copied()
                .chain(l.threads.values().flat_map(|t| t.records.iter().copied()))
                .chain(l.cold_turns.values().flatten().copied())
                .collect(),
            None => Vec::new(),
        };
        if let Some(ann) = &mut self.ann {
            let mut pruned = 0usize;
            for id in non_facts {
                if ann.contains(id) {
                    ann.remove(id);
                    pruned += 1;
                }
            }
            if pruned > 0 {
                self.dirty_index = true;
                self.index_persist.changes += pruned;
            }
        }
        Ok(())
    }

    // --- durability & maintenance ----------------------------------------
}
