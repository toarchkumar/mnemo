//! Commits and durability: sealed runs, the WAL and checkpoint, flush, close, and the page-cache and vector-budget settings.

use super::*;

impl Mnemo {
    /// Seal a serialized buffer into a fresh run of encrypted home pages and
    /// append a WAL frame for each. Returns the run's `(start_page, pages)`.
    /// The pages are *not* written to their home locations here — they go to
    /// the WAL and are folded in later by [`Mnemo::checkpoint`].
    ///
    /// `gen` is the run's generation (v9; `0` on v8). On v9 the pages come
    /// from the free list when an eligible run fits.
    pub(super) fn seal_run(
        &mut self,
        bytes: &[u8],
        gen: u64,
        frames: &mut Vec<wal::Frame>,
    ) -> Result<(u64, u32)> {
        let pc = bytes.len().div_ceil(PAYLOAD).max(1);
        let start = self.alloc_pages(pc as u64);
        self.seal_run_at(start, pc as u64, bytes, gen, frames)?;
        Ok((start, pc as u32))
    }

    /// Seal `bytes` into the `pages` already-allocated pages at `start`
    /// (pages past the data are sealed empty) and append their WAL frames.
    pub(super) fn seal_run_at(
        &mut self,
        start: u64,
        pages: u64,
        bytes: &[u8],
        gen: u64,
        frames: &mut Vec<wal::Frame>,
    ) -> Result<()> {
        let direct = self.is_v9() && pages > V9_WAL_RUN_MAX_PAGES;
        for i in 0..pages as usize {
            let lo = (i * PAYLOAD).min(bytes.len());
            let hi = ((i + 1) * PAYLOAD).min(bytes.len());
            let page_no = start + i as u64;
            let sealed = self.pager.seal_page(page_no, gen, &bytes[lo..hi])?;
            if direct {
                self.pager.write_sealed(page_no, &sealed)?;
            } else {
                frames.push((page_no, sealed.to_vec()));
            }
        }
        if direct {
            self.direct_writes = true;
        }
        Ok(())
    }

    /// Grow the WAL region if `frame_pages` worth of frames would not fit.
    /// Called at the top of `flush`, where the WAL is always spent (every
    /// flush ends with a checkpoint), so relocating it cannot strand a
    /// committed transaction.
    pub(super) fn ensure_wal_capacity(&mut self, frame_pages: usize) -> Result<()> {
        let need = wal::txn_byte_len(frame_pages, PAGE_SIZE);
        if need <= self.header.wal_pages * PAGE_SIZE as u64 {
            return Ok(());
        }
        // Allocate a new, larger region at the file tail (~1.5x headroom).
        let req = need.div_ceil(PAGE_SIZE as u64);
        let new_pages = (req + req / 2 + 4).max(DEFAULT_WAL_PAGES);
        let old_wal_start = self.header.wal_start;
        let old_wal_pages = self.header.wal_pages;
        // FMT-9 (a): the new region always comes from the file's tail: the
        // relocation commits a header-only transaction, so a region taken
        // from the free list would still be listed as free in the
        // committed free-list run.
        let new_start = self.header.next_page;
        self.header.next_page += new_pages;
        self.header.wal_start = new_start;
        self.header.wal_pages = new_pages;
        self.header.wal_seq += 1;
        // DUR-1 (R-002.1): WAL-log the header change into the *new* region
        // first, checkpoint page 0, then invalidate the old region. A crash
        // anywhere here either keeps the old state (if page 0 wasn't yet
        // overwritten) or recovers to the new state via the torn-header
        // scan in `Mnemo::open`, which prefers the highest-txn-id valid
        // committed header frame.
        self.relocate_header_through_wal(old_wal_start)?;
        // FMT-9 (a): once the relocation has committed, the old region is
        // unreferenced; the next commit frees it.
        self.release_pages(old_wal_start, old_wal_pages);
        Ok(())
    }

    /// Shared helper for every post-`create` page-0 commit: WAL-log the
    /// new header into the current WAL region, checkpoint page 0, and
    /// invalidate `old_wal_start` (the previous region, or the current
    /// region when it hasn't moved). The caller is expected to have
    /// mutated `self.header` to its target values *including a bumped
    /// `wal_seq`* (one more than the last on-disk `wal_seq`); this
    /// routine applies the seal, writes the frame + commit, folds page 0,
    /// then zeroes the old region's first frame header.
    ///
    /// If `old_wal_start == self.header.wal_start`, the invalidation is
    /// skipped — the region didn't move, so overwriting its first frame
    /// would destroy the just-committed one.
    pub(super) fn relocate_header_through_wal(&mut self, old_wal_start: u64) -> Result<()> {
        self.header.write_counter = self.pager.write_counter;
        let mut page = self.header.to_page();
        self.header.apply_seal(&mut page, self.pager.dek())?;
        let txn_id = self.header.wal_seq;
        let frames = vec![(0u64, page.to_vec())];
        wal::commit(
            self.pager.file_mut(),
            self.header.wal_start,
            self.header.wal_pages,
            txn_id,
            &frames,
        )?;
        self.pager.write_raw(0, &page)?;
        self.pager.sync()?;
        if old_wal_start != self.header.wal_start {
            wal::invalidate_region(self.pager.file_mut(), old_wal_start)?;
        }
        Ok(())
    }

    /// Fold a committed transaction's WAL frames into their home pages.
    pub(super) fn checkpoint(&mut self, frames: &[wal::Frame]) -> Result<()> {
        for (page_no, bytes) in frames {
            let mut img = [0u8; PAGE_SIZE];
            img.copy_from_slice(bytes);
            self.pager.write_sealed(*page_no, &img)?;
        }
        self.pager.sync()?;
        Ok(())
    }

    /// Persist all pending changes as one **write-ahead-logged transaction**.
    ///
    /// Sequence:
    ///
    /// 1. **Prepare.** Serialize the control plane (catalog, ANN index),
    ///    grow the WAL if needed, and **lease** counter and page slots —
    ///    bump `header.write_counter` and `header.next_page` by upper
    ///    bounds on this transaction's consumption and persist that
    ///    *leased* header to disk before any encrypted page is written.
    /// 2. **Data pages.** [`Pager::flush`] writes dirty record pages
    ///    copy-on-write under fresh nonces and fsyncs them. The orphan-page
    ///    nonce-reuse window (see [Phase 1.1 of the improvement plan])
    ///    is closed because the leased header on disk already records a
    ///    `write_counter` past anything this step can produce.
    /// 3. **Control plane.** Seal the new catalog / ANN index / snapshot
    ///    manifest into fresh home page runs.
    /// 4. **Commit.** Log all of step 3's frames plus the final header
    ///    frame into the WAL and fsync — the durability point.
    /// 5. **Checkpoint.** Fold the WAL into the home pages.
    ///
    /// A crash before step 4 leaves the previous state intact. A crash
    /// after step 4 is repaired by [`Mnemo::open`] replaying the WAL.
    /// Safe to call repeatedly.
    pub fn flush(&mut self) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // Snapshot the thread-local fsync counter. Every `sync_all` the
        // pager / wal / checkpoint path makes between here and the end
        // of this function bumps the same counter; `delta` lands on
        // `self.fsyncs_last_flush` so `Stats::fsyncs_last_flush` can
        // report it. The planner wants this to decide whether cutting
        // fsync count is a viable write-latency win before we touch
        // the write path's durability model.
        let fsync_before = crate::fsync_count();
        // PERF-1 step 3: while the index is unloaded, records written since
        // its run pile up for the next load to re-assign. Once there are
        // INDEX_PERSIST_EVERY_CHANGES of them, load it now so this flush
        // persists a current index and keeps that work bounded.
        if self.ann_unloaded && self.dirty_catalog {
            let (index_start, index_gen) = (self.header.index_start, self.index_gen_now());
            let v9 = self.is_v9();
            let pending = self
                .catalog
                .iter()
                .filter(|e| !e.deleted && written_after_index(e, v9, index_start, index_gen))
                .count();
            if pending >= INDEX_PERSIST_EVERY_CHANGES {
                self.ensure_ann()?;
            }
        }
        // PERF-1: persist the (derived) ANN index only when forced or due.
        // Otherwise the header keeps pointing at the previous index run, and
        // records written since then are re-assigned when it's next loaded
        // (`reconcile_ann_with_catalog`). `ann == None` with `dirty_index` set is a
        // dropped index, which is persisted at once.
        let persist_index = if self.is_v9() {
            self.plan_index_v9()
        } else {
            self.dirty_index
                && (self.index_persist.forced
                    || self.ann.is_none()
                    || self.index_persist.commits + 1 >= INDEX_PERSIST_EVERY_COMMITS
                    || self.index_persist.changes >= INDEX_PERSIST_EVERY_CHANGES)
        };
        let free_work =
            self.is_v9() && (self.free_dirty || !self.pending_free.is_empty() || self.dirty_keys);
        if !self.dirty_catalog && !persist_index && !self.dirty_cache && !free_work {
            // No control-plane changes pending. Because every `put` that
            // dirties a data page also dirties the catalog, and every
            // `cache_put` that dirties a data page also dirties
            // `dirty_cache`, there can't be dirty data pages either —
            // nothing to do.
            self.fsyncs_last_flush = 0;
            return Ok(());
        }

        // FIX-1 R-001.8 part 3: snapshot the memory/index-dirty flags
        // before step 3b decides whether to push a restore point.
        // Only flushes that change the memory catalog or ANN index
        // need a snapshot — a cache-only flush (`dirty_cache` set,
        // `dirty_catalog` + `dirty_index` both false) must not push,
        // because snapshots don't capture cache state and every push
        // counts against `max_snapshots` (default 256), so a cache-
        // heavy workload would otherwise evict every real memory
        // restore point within ~256 cache writes.
        let is_memory_or_index_flush = self.dirty_catalog || persist_index;

        // FMT-9: a v9 session's first sealed page needs the generation
        // lease (normally taken by its first record write already).
        self.ensure_gen_lease()?;
        // FMT-9e: turns that aged past the cutoff are rewritten compressed
        // (bounded per commit), then packed with the other waiting records.
        self.settle_cold_turns()?;
        // FMT-9b: column entries first; they move rows' vector locations,
        // so they must land before the catalog is serialized.
        self.pack_records()?;
        self.pack_vectors()?;

        // 1. Serialize control plane, ensure WAL capacity, lease counter +
        //    page slots, and persist the leased header *before* any data
        //    page hits the disk.
        let ctx = self.prepare_for_flush(persist_index)?;

        // 2. Record (vector) data pages: copy-on-write to fresh pages, fsynced.
        //    Safe: the lease guarantees that even if we crash here, reopen
        //    will see a `write_counter` past every nonce this step uses.
        self.pager.flush()?;

        // 3. Seal the catalog / index control pages into fresh home runs.
        //    FMT-9: `txn_id` is this transaction's id and the generation
        //    of every run it seals. A v9 run replaced here is freed by this
        //    same commit (`superseded`); v8 never reuses pages.
        let txn_id = self.header.wal_seq + 1;
        let gen = if self.is_v9() { txn_id } else { 0 };
        let v9 = self.is_v9();
        let mut superseded: Vec<(u64, u64)> = Vec::new();
        let mut frames: Vec<wal::Frame> = Vec::new();
        // FMT-9c.2: a delta run goes on the chain; a new base replaces the
        // old base and the whole chain (all freed by this commit). The
        // chain is adopted once the commit succeeds.
        let mut new_chain: Option<(Vec<RunPtr>, usize)> = None;
        if let Some(bytes) = &ctx.cat_bytes {
            if ctx.cat_delta {
                let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                let ptr = RunPtr {
                    start,
                    pages: pages as u64,
                    len: bytes.len() as u64,
                    gen,
                };
                self.header.catalog_delta = ptr;
                let mut chain = self.delta_chain.clone();
                chain.push(ptr);
                new_chain = Some((chain, self.delta_rows + self.dirty_rows.len()));
            } else {
                if v9 && self.header.catalog_pages > 0 {
                    superseded.push((self.header.catalog_start, self.header.catalog_pages));
                }
                if v9 {
                    for r in &self.delta_chain {
                        superseded.push((r.start, r.pages));
                    }
                    self.header.catalog_delta = RunPtr::default();
                    new_chain = Some((Vec::new(), 0));
                }
                let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                self.header.catalog_start = start;
                self.header.catalog_pages = pages as u64;
                self.header.catalog_len = bytes.len() as u64;
                self.header.catalog_gen = gen;
            }
        }
        // FMT-9d: the BM25 segment, then the manifest that lists it.
        let mut new_bm25: Option<Bm25Segments> = None;
        if let Some((bytes, replace_from)) = &ctx.bm25_seg {
            let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
            let mut segs = self.bm25.manifest.segments.clone();
            let mut seg_docs = self.bm25.manifest.seg_docs.clone();
            seg_docs.resize(segs.len(), 0);
            let docs = if let Some(from) = *replace_from {
                for t in segs.drain(from..) {
                    superseded.push((t.0, t.1));
                }
                seg_docs.truncate(from);
                self.bm25.merged_docs
            } else {
                self.bm25.pending.len() as u32
            };
            segs.push((start, pages as u64, bytes.len() as u64, gen));
            seg_docs.push(docs);
            let man = Bm25Manifest {
                tokenizer: text::TOKENIZER_V1,
                next_doc: self.bm25.manifest.next_doc,
                segments: segs.clone(),
                seg_docs: seg_docs.clone(),
            };
            let mbytes =
                rmp_serde::to_vec(&man).map_err(|e| MnemoError::Serialize(e.to_string()))?;
            if !self.header.bm25.is_empty() {
                superseded.push((self.header.bm25.start, self.header.bm25.pages));
            }
            let (ms, mp) = self.seal_run(&mbytes, gen, &mut frames)?;
            self.header.bm25 = RunPtr {
                start: ms,
                pages: mp as u64,
                len: mbytes.len() as u64,
                gen,
            };
            new_bm25 = Some((segs, seg_docs));
        }
        let mut new_ichain: Option<(Vec<RunPtr>, usize)> = None;
        if persist_index && ctx.idx_delta {
            if let Some(bytes) = &ctx.idx_bytes {
                let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                let ptr = RunPtr {
                    start,
                    pages: pages as u64,
                    len: bytes.len() as u64,
                    gen,
                };
                self.header.index_delta = ptr;
                let mut chain = self.index_chain.clone();
                chain.push(ptr);
                let n = self.index_delta_ops.as_ref().map_or(0, |o| o.len());
                new_ichain = Some((chain, self.index_chain_ops + n));
            }
        } else if persist_index {
            if v9 && self.header.index_pages > 0 {
                superseded.push((self.header.index_start, self.header.index_pages));
            }
            if v9 {
                for r in &self.index_chain {
                    superseded.push((r.start, r.pages));
                }
                self.header.index_delta = RunPtr::default();
                new_ichain = Some((Vec::new(), 0));
            }
            match &ctx.idx_bytes {
                Some(bytes) => {
                    let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                    self.header.index_start = start;
                    self.header.index_pages = pages as u64;
                    self.header.index_len = bytes.len() as u64;
                    self.header.index_gen = gen;
                }
                None => {
                    self.header.index_start = 0;
                    self.header.index_pages = 0;
                    self.header.index_len = 0;
                    self.header.index_gen = 0;
                }
            }
        }
        // v8+ result-cache directory (Phase 10.1). Seal the serialized
        // directory into a fresh page run and update the pointer in the
        // header. The seal AAD already covers cache_start/pages/len for
        // v8+ (see `header_seal_aad`), so tampering with the pointer
        // trips the header seal on the next open.
        if self.dirty_cache {
            if v9 && self.header.cache_pages > 0 {
                superseded.push((self.header.cache_start, self.header.cache_pages));
            }
            match &ctx.cache_bytes {
                Some(bytes) if !self.cache_dir.is_empty() => {
                    let (start, pages) = self.seal_run(bytes, gen, &mut frames)?;
                    self.header.cache_start = start;
                    self.header.cache_pages = pages as u64;
                    self.header.cache_len = bytes.len() as u64;
                    self.header.cache_gen = gen;
                }
                _ => {
                    // Either the directory serialized to nothing (all
                    // entries deleted after tombstone-only migration) or
                    // it's empty — collapse the pointer to zero.
                    self.header.cache_start = 0;
                    self.header.cache_pages = 0;
                    self.header.cache_len = 0;
                    self.header.cache_gen = 0;
                }
            }
        }

        // 3b. Record this transaction as a restorable snapshot. The manifest
        //     update is staged in a local and adopted only once the commit
        //     below succeeds, so a failed flush leaves no phantom entry.
        //
        //     The cap (`self.max_snapshots`) is applied to the staged local
        //     before the new entry lands, so after appending the manifest
        //     holds at most `max_snapshots` entries — the most-recent N.
        //     `max_snapshots == 0` disables the cap. On v8, pages
        //     referenced only by pruned snapshots stay on disk until
        //     `compact_file`; on v9 they become reusable once no retained
        //     snapshot is older than the transaction that freed them.
        //
        //     FIX-1 R-001.8 part 3: skip the snapshot push on a
        //     cache-only flush. Snapshots don't capture the cache
        //     anyway, and every push counts against `max_snapshots`
        //     (default 256) + rewrites the manifest run — so a
        //     cache-heavy workload would otherwise evict every real
        //     memory restore point within ~256 cache writes. On a
        //     cache-only flush we reuse the current manifest
        //     verbatim; the manifest run already lives at
        //     `self.header.manifest_{start,pages,len}` (set by the
        //     last memory-touching flush, or at `create` time).
        let live = self.len() as u64;
        let mut manifest = self.manifest.clone();
        if is_memory_or_index_flush {
            if self.max_snapshots > 0 && !v9 {
                // Drop the oldest entries so there's room for the new one.
                while manifest.len() >= self.max_snapshots {
                    manifest.remove(0);
                }
            }
            manifest.push(Snapshot {
                txn_id,
                created_at: memory::now_secs(),
                catalog_start: self.header.catalog_start,
                catalog_pages: self.header.catalog_pages,
                catalog_len: self.header.catalog_len,
                index_start: self.header.index_start,
                index_pages: self.header.index_pages,
                index_len: self.header.index_len,
                memory_count: live,
                gens: v9.then_some(SnapshotGens {
                    catalog: self.header.catalog_gen,
                    index: self.header.index_gen,
                    delta: ptr_tuple(self.header.catalog_delta),
                    index_delta: ptr_tuple(self.header.index_delta),
                    bm25: ptr_tuple(self.header.bm25),
                }),
            });
            if v9 {
                let extra = self.pending_free.iter().map(|r| r.1).sum::<u64>()
                    + superseded.iter().map(|r| r.1).sum::<u64>()
                    + self.header.manifest_pages;
                self.apply_retention(&mut manifest, extra);
            }
            let man_bytes =
                rmp_serde::to_vec(&manifest).map_err(|e| MnemoError::Serialize(e.to_string()))?;
            if v9 && self.header.manifest_pages > 0 {
                superseded.push((self.header.manifest_start, self.header.manifest_pages));
            }
            let (m_start, m_pages) = self.seal_run(&man_bytes, gen, &mut frames)?;
            self.header.manifest_start = m_start;
            self.header.manifest_pages = m_pages as u64;
            self.header.manifest_len = man_bytes.len() as u64;
            self.header.manifest_gen = gen;
        }

        // 3c. FMT-9 (b): the key-slot run, rewritten copy-on-write when a
        //     slot changed. The header seal covers its pointer.
        if v9 && self.dirty_keys {
            if !self.header.keys.is_empty() {
                superseded.push((self.header.keys.start, self.header.keys.pages));
            }
            if self.key_slots.slots.is_empty() {
                self.header.keys = RunPtr::default();
            } else {
                let bytes = rmp_serde::to_vec(&self.key_slots)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?;
                let (start, pages) = self.seal_run(&bytes, gen, &mut frames)?;
                self.header.keys = RunPtr {
                    start,
                    pages: pages as u64,
                    len: bytes.len() as u64,
                    gen,
                };
            }
        }

        // 3d. FMT-9 (a): the free list. Everything this transaction
        //     stopped referencing (`pending_free`, the runs replaced above,
        //     and the previous free-list run itself) is freed *at this
        //     transaction's id*, in this transaction's own commit, so it
        //     can't be reused until this commit is durable and no retained
        //     snapshot predates it. The list is staged in a local and
        //     adopted only once the commit succeeds (a failed flush that
        //     is retried must not free the same runs twice).
        let mut shrink = false;
        let staged_free =
            if v9 && (self.free_dirty || !self.pending_free.is_empty() || !superseded.is_empty()) {
                let mut staged = self.free.clone();
                for &(p, c) in self.pending_free.iter().chain(superseded.iter()) {
                    staged.free(txn_id, p, c)?;
                }
                if !self.header.free.is_empty() {
                    staged.free(txn_id, self.header.free.start, self.header.free.pages)?;
                }
                // FMT-9c: give eligible free runs at the file's tail back
                // to the filesystem: the header's `next_page` moves down in
                // this commit, and the file is cut after it.
                let min_snap = self.min_snapshot_txn();
                while let Some(p) =
                    staged.take_tail(self.header.next_page, self.header.wal_seq, min_snap)
                {
                    self.header.next_page = p;
                    shrink = true;
                }
                // Size the run with room for the one extra entry that
                // allocating it out of the list can create (a split), then
                // allocate it from the list (eligible runs only) or the tail.
                let est = rmp_serde::to_vec(&staged.to_runs())
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?
                    .len()
                    + 64;
                let pc = est.div_ceil(PAYLOAD).max(1) as u64;
                let start = match staged.alloc(pc, self.header.wal_seq, self.min_snapshot_txn()) {
                    Some(p) => p,
                    None => {
                        let p = self.header.next_page;
                        self.header.next_page += pc;
                        p
                    }
                };
                let bytes = rmp_serde::to_vec(&staged.to_runs())
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?;
                if bytes.len() as u64 > pc * PAYLOAD as u64 {
                    return Err(MnemoError::Invalid(
                        "free list outgrew its reserved run".into(),
                    ));
                }
                self.seal_run_at(start, pc, &bytes, gen, &mut frames)?;
                self.header.free = RunPtr {
                    start,
                    pages: pc,
                    len: bytes.len() as u64,
                    gen,
                };
                Some(staged)
            } else {
                None
            };

        // 3e. The header is the transaction's final frame; stamp the new id
        //     and the *actual* (post-flush) write counter, which is <= the
        //     leased value persisted in `prepare_for_flush`. This is the
        //     value that ultimately replaces the leased header on checkpoint.
        self.header.vector_count = live;
        self.header.write_counter = self.pager.write_counter;
        self.header.wal_seq = txn_id;
        let mut hpage = self.header.to_page();
        self.header.apply_seal(&mut hpage, self.pager.dek())?;
        frames.push((0, hpage.to_vec()));

        // 4. COMMIT — log the transaction and fsync. Nothing at a home page
        //    other than the leased header has changed yet; this single
        //    fsync is what makes the rest durable.
        // FMT-9c: big runs went straight to their home pages; make them
        // durable before the header that references them can be.
        if self.direct_writes {
            self.pager.sync()?;
            self.direct_writes = false;
        }
        let (wal_start, wal_pages) = (self.header.wal_start, self.header.wal_pages);
        wal::commit(self.pager.file_mut(), wal_start, wal_pages, txn_id, &frames)?;

        // 5. Checkpoint — fold the WAL into the home pages.
        self.checkpoint(&frames)?;
        // FMT-9c: the committed header no longer covers the tail; cut it.
        // A crash before this leaves a longer file, which is harmless.
        if shrink {
            let len = self.header.next_page * PAGE_SIZE as u64;
            self.pager.file_mut().set_len(len)?;
        }

        self.manifest = manifest;
        if let Some(staged) = staged_free {
            self.free = staged;
        }
        self.pending_free.clear();
        self.free_dirty = false;
        self.dirty_keys = false;
        if let Some((chain, rows)) = new_chain {
            self.delta_chain = chain;
            self.delta_rows = rows;
        }
        if let Some((chain, n)) = new_ichain {
            self.index_chain = chain;
            self.index_chain_ops = n;
        }
        if let Some((segs, seg_docs)) = new_bm25 {
            // FMT-9f: keep the loaded dicts and read only the new segment's
            // (a merge replaced the segments from `kept` on), instead of
            // dropping them all and reloading every dict on next use.
            let kept = self
                .bm25
                .manifest
                .segments
                .iter()
                .zip(&segs)
                .take_while(|(a, b)| a == b)
                .count();
            if let Some(mut dicts) = self.bm25.dicts.take() {
                dicts.truncate(kept);
                let mut ok = dicts.len() == kept;
                for &t in &segs[kept..] {
                    match load_segment_dict(&mut self.pager, tuple_ptr(t)) {
                        Ok(d) if ok => dicts.push(d),
                        _ => ok = false,
                    }
                }
                if ok {
                    self.bm25.dicts = Some(dicts);
                }
            }
            self.bm25.manifest.segments = segs;
            self.bm25.manifest.seg_docs = seg_docs;
            self.bm25.pending.clear();
        }
        if persist_index {
            self.index_rebase = false;
            self.index_delta_ops = None;
        }
        if ctx.cat_bytes.is_some() {
            self.dirty_rows.clear();
            self.catalog_rewrite = false;
        }
        self.dirty_catalog = false;
        if persist_index {
            self.dirty_index = false;
            self.index_persist = IndexPersist::default();
        } else if self.dirty_index {
            self.index_persist.commits += 1;
        }
        self.dirty_cache = false;
        // Batched policy: successful flush consumes the pending batch.
        self.cache_batch.reset();
        // Record how many fsyncs this flush issued, for Stats reporting.
        // Thread-local counter guarantees we see only this handle's own
        // sync_all calls between the snapshot above and this read.
        self.fsyncs_last_flush =
            (crate::fsync_count().wrapping_sub(fsync_before)).min(u32::MAX as u64) as u32;
        Ok(())
    }

    /// Pre-flush prelude shared by [`Mnemo::flush`] and the
    /// `__crash_partial_flush_for_testing` hook.
    ///
    /// Serializes the control plane (catalog, ANN index, cache directory)
    /// and grows the WAL if needed. That's it: this used to persist a
    /// leased header too (SEC-1 obsoleted it; see DUR-1 R-002.1 note in
    /// the body for the full rationale), but after DUR-1 the only page-0
    /// writes outside `create` go through the WAL via
    /// [`Mnemo::relocate_header_through_wal`].
    ///
    /// The historical pre-stamping used to close the nonce-reuse window
    /// flagged in Phase 1.1 of the improvement plan: a crash anywhere
    /// between here and the WAL commit left a file whose on-disk header
    /// recorded a `write_counter` past every nonce the rest of this
    /// transaction could produce, so the next reopen started from a
    /// counter that guaranteed uniqueness against any orphan data pages
    /// still on disk. SEC-1's per-session random nonce prefix gives the
    /// same guarantee without needing a persisted counter, which is why
    /// this routine no longer writes page 0.
    pub(super) fn prepare_for_flush(&mut self, persist_index: bool) -> Result<FlushPrelude> {
        // Serialize.
        // FMT-9c.2: on v9 usually just the changed rows, as a delta run.
        let cat_delta = self.dirty_catalog && self.catalog_as_delta();
        let cat_bytes = if !self.dirty_catalog {
            None
        } else if cat_delta {
            let rows: Vec<(u32, CatalogEntry)> = self
                .dirty_rows
                .iter()
                .filter(|&&i| i < self.catalog.len())
                .map(|&i| (i as u32, self.catalog[i].clone()))
                .collect();
            let d = CatalogDelta {
                prev: ptr_tuple(self.header.catalog_delta),
                rows,
            };
            Some(rmp_serde::to_vec(&d).map_err(|e| MnemoError::Serialize(e.to_string()))?)
        } else {
            if self.is_v9() {
                self.compact_tombstones();
            }
            Some(
                rmp_serde::to_vec(&self.catalog)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?,
            )
        };
        // FMT-9c.3: a v9 commit usually writes just its index changes.
        let idx_delta = persist_index && self.index_delta_ops.is_some();
        let idx_bytes: Option<Vec<u8>> = if !persist_index {
            None
        } else if let Some(ops) = &self.index_delta_ops {
            let d = IndexDeltaRef {
                prev: ptr_tuple(self.header.index_delta),
                ops,
            };
            Some(rmp_serde::to_vec(&d).map_err(|e| MnemoError::Serialize(e.to_string()))?)
        } else {
            match &self.ann {
                Some(ann) => {
                    let mut b =
                        rmp_serde::to_vec(ann).map_err(|e| MnemoError::Serialize(e.to_string()))?;
                    // FMT-9: v9 index runs carry a layout byte first.
                    if self.is_v9() {
                        b.insert(0, self.index_layout_override.unwrap_or(V9_INDEX_LAYOUT));
                    }
                    Some(b)
                }
                None => None,
            }
        };
        // Serialize the result-cache directory when it's dirty. An
        // empty directory serializes to a small header + 0-element
        // array; still cheap enough that we don't special-case it.
        let cache_bytes: Option<Vec<u8>> = if self.dirty_cache {
            Some(
                rmp_serde::to_vec(&self.cache_dir)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?,
            )
        } else {
            None
        };

        let cat_pc = cat_bytes
            .as_ref()
            .map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        let idx_pc = idx_bytes
            .as_ref()
            .map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        let cache_pc = cache_bytes
            .as_ref()
            .map_or(0, |b| b.len().div_ceil(PAYLOAD).max(1));
        // Upper bound on the manifest run: one extra entry, <=82 bytes each
        // (rmp_serde encodes a Snapshot as a 9-element fixarray of i64/u64,
        // worst case 81 bytes plus a 1-byte array header).
        // v9 snapshots carry two more u64s (≤ 20 bytes with the array header).
        let per_snapshot = if self.is_v9() { 102 } else { 82 };
        let man_upper = 9 + (self.manifest.len() + 1) * per_snapshot;
        let man_pc_est = man_upper.div_ceil(PAYLOAD).max(1);
        // FMT-9: the free-list run (≤ 28 bytes per run: every pending and
        // superseded run plus a split and a relocated WAL region) and the
        // key-slot run.
        let (free_pc_est, keys_pc) = if self.is_v9() {
            let runs = self.free.len() + self.pending_free.len() + 12;
            let free_pc = (runs * 28 + 64).div_ceil(PAYLOAD).max(1);
            let keys_pc = if self.dirty_keys {
                rmp_serde::to_vec(&self.key_slots)
                    .map_err(|e| MnemoError::Serialize(e.to_string()))?
                    .len()
                    .div_ceil(PAYLOAD)
                    .max(1)
            } else {
                0
            };
            (free_pc, keys_pc)
        } else {
            (0, 0)
        };

        // Grow WAL if needed (DUR-1: relocation is WAL-logged and does
        // its own fsyncs). The cache directory rides in the same
        // transaction so its page count contributes to the reservation.
        // FMT-9d: the BM25 segment (and its manifest, one page).
        let bm25_seg = self.bm25_next_segment()?;
        let bm25_pc = bm25_seg
            .as_ref()
            .map_or(0, |(b, _)| b.len().div_ceil(PAYLOAD).max(1));
        // FMT-9c: on v9 only runs of up to V9_WAL_RUN_MAX_PAGES go through
        // the WAL; longer ones are written in place.
        let v9 = self.is_v9();
        let in_wal = |pc: usize| {
            if v9 && pc as u64 > V9_WAL_RUN_MAX_PAGES {
                0
            } else {
                pc
            }
        };
        self.ensure_wal_capacity(
            in_wal(cat_pc)
                + in_wal(idx_pc)
                + in_wal(cache_pc)
                + in_wal(man_pc_est)
                + in_wal(free_pc_est)
                + in_wal(keys_pc)
                + in_wal(bm25_pc)
                + usize::from(bm25_pc > 0)
                + 1,
        )?;

        // DUR-1 (R-002.1): the pre-flush "leased header" write that used
        // to live here is gone. It existed to close the Phase 1.1 nonce
        // reuse window (reopen must see a `write_counter` past any nonce
        // this flush could produce before data pages hit disk). SEC-1
        // superseded that: nonces now carry a per-session random prefix
        // and `write_counter` is no longer a nonce input, so the leased
        // write bought us nothing but an extra fsync per flush. Orphan
        // data pages past `header.next_page` on a crashed flush are
        // still safe — the next session's random prefix makes any later
        // write at the same page_no uniquely nonced regardless of
        // `write_counter`.

        Ok(FlushPrelude {
            bm25_seg,
            cat_bytes,
            cat_delta,
            idx_delta,
            idx_bytes,
            cache_bytes,
        })
    }

    /// Flush and close: consumes the handle and releases the file and its
    /// OS advisory lock before returning, so the same file can be reopened
    /// right away (by this process or another). On a read-write handle it
    /// flushes first, persisting a dirty ANN index too; on a read-only
    /// handle there's nothing to persist.
    ///
    /// Using the handle after `close` is a compile error. If the flush
    /// fails, the error is returned and the handle is dropped all the same:
    /// changes since the last successful [`Mnemo::flush`] are lost, as
    /// they would be on a plain `drop` (which never flushes).
    ///
    /// A struct that owns a handle and closes it from `&mut self` keeps it
    /// in an `Option` and `take()`s it:
    ///
    /// ```no_run
    /// use mnemo_engine::{Mnemo, Result};
    ///
    /// struct App {
    ///     db: Option<Mnemo>,
    /// }
    ///
    /// impl App {
    ///     fn shutdown(&mut self) -> Result<()> {
    ///         match self.db.take() {
    ///             Some(db) => db.close(),
    ///             None => Ok(()), // already closed
    ///         }
    ///     }
    /// }
    /// ```
    ///
    /// Changed in v0.12.0: `close` took `&mut self` and left the file and
    /// lock held until the handle was dropped.
    ///
    /// **If the final flush fails, the handle is dropped with it**, and the
    /// writes that weren't flushed are lost (a full disk on a phone, say).
    /// [`Mnemo::try_close`] gives the handle back on that path, so the caller
    /// can free space and retry.
    pub fn close(mut self) -> Result<()> {
        if !self.read_only {
            // PERF-1: closing persists a dirty ANN index too, so the next
            // open has nothing to re-assign.
            self.index_persist.forced = true;
            self.flush()?;
        }
        // Dropping the handle closes the file, which releases the lock.
        drop(self);
        Ok(())
    }

    /// [`Mnemo::close`] that keeps the data when the final flush fails: the
    /// error carries the handle back, with every unflushed write still in
    /// it, as [`std::io::BufWriter::into_inner`] does. Retry with
    /// [`Mnemo::flush`] or another `try_close` once the cause is fixed.
    /// A read-only handle never flushes, so it always closes. Added in
    /// v0.12.2 (RB-001.22).
    ///
    /// ```no_run
    /// # use mnemo_engine::Mnemo;
    /// # fn run(db: Mnemo) -> mnemo_engine::Result<()> {
    /// match db.try_close() {
    ///     Ok(()) => {}
    ///     Err(e) => {
    ///         eprintln!("close failed: {}", e.error());
    ///         let mut db = e.into_inner(); // still open, still holding the writes
    ///         db.flush()?;                  // e.g. after freeing space
    ///         db.close()?;
    ///     }
    /// }
    /// # Ok(()) }
    /// ```
    pub fn try_close(mut self) -> std::result::Result<(), CloseError> {
        if !self.read_only {
            self.index_persist.forced = true;
            if let Err(error) = self.flush() {
                return Err(CloseError {
                    handle: Box::new(self),
                    error,
                });
            }
        }
        drop(self);
        Ok(())
    }

    /// PERF-1: flush, persisting the ANN index even if the
    /// [`INDEX_PERSIST_EVERY_COMMITS`] / [`INDEX_PERSIST_EVERY_CHANGES`]
    /// policy wouldn't yet. Plain [`Mnemo::flush`] writes only the catalog
    /// between persists; the index is derived, so a crash costs nothing but
    /// re-assigning the newer records on the next load.
    pub fn flush_index(&mut self) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.index_persist.forced = true;
        self.flush()
    }

    /// **TEST ONLY — do not use.** Reproduces a crash inside
    /// [`Mnemo::flush`] *after* the prelude has leased counter+page slots
    /// and persisted the leased header, but *before* the WAL is committed.
    /// Runs `prepare_for_flush` + `pager.flush` and returns — leaving
    /// data pages and a leased header on disk, with no commit frame.
    ///
    /// Used by `tests/integration.rs::nonce_unique_after_crashed_data_flush`
    /// to verify that this window (Phase 1.1 of the improvement plan) does
    /// not enable AES-GCM nonce reuse. Calling this in production code
    /// strands data pages; never expose it from a binding.
    #[doc(hidden)]
    pub fn __crash_partial_flush_for_testing(&mut self) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if !self.dirty_catalog && !self.dirty_index {
            return Ok(());
        }
        // FMT-9b: v9 records and vectors reach their pages at the top of a
        // flush; do that part too.
        self.ensure_gen_lease()?;
        self.pack_records()?;
        self.pack_vectors()?;
        let _prelude = self.prepare_for_flush(self.dirty_index)?;
        self.pager.flush()
    }

    /// Bound the in-memory page cache to `pages` decrypted pages.
    ///
    /// The cache holds decrypted page payloads to speed repeated reads. By
    /// default it is capped at 1024 pages (~8 MiB; 8192 before v0.10.0).
    /// Lower the cap to trade hit rate for a smaller footprint, or raise
    /// it for a hotter cache. The
    /// cap governs *clean* pages — pages with un-flushed writes are always
    /// retained until [`Mnemo::flush`], regardless of the cap.
    pub fn set_cache_capacity(&mut self, pages: usize) {
        self.pager.set_cache_capacity(pages);
    }

    /// FMT-9b (v9 files): keep the fp16 vector column in memory while it
    /// takes at most `bytes` (default [`DEFAULT_VECTOR_CACHE_BUDGET`],
    /// 96 MiB, about 125K vectors at 384 dims). Above it, recall reads
    /// vector entries through the page cache instead: less memory, slower
    /// recall. Lowering it below the resident size drops the resident copy.
    pub fn set_vector_cache_budget(&mut self, bytes: usize) {
        self.vector_cache_budget = bytes;
        let need = self.col.scored_rows * veccol::entry_len(self.dimensions);
        if self.col.resident && need > bytes {
            self.col.data = Vec::new();
            self.col.norms = Vec::new();
            self.col.have = Vec::new();
            self.col.resident = false;
        }
    }

    /// Page-cache occupancy: `(pages_cached, capacity)`. Renamed from
    /// `cache_stats` in v0.4.0: the result-cache API (Phase 10.1) took
    /// that name for its per-namespace [`CacheStats`] summary, and the
    /// page-cache pair is the older/less-user-facing of the two, so it
    /// gets the more-specific name.
    pub fn page_cache_stats(&self) -> (usize, usize) {
        self.pager.cache_stats()
    }

    // === Result cache (Phase 10.1 + 10.3) ============================
}
