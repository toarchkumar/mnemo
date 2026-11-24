//! Hidden (doc(hidden)) test and benchmark hooks: not a stable API.

use super::*;

impl Mnemo {
    /// **Not a stable API.** 13a step 1: write a coordination record of
    /// `kind` (message, task, claim, presence), so a test can check it's
    /// kept out of recall, the keyword index, the ANN and `about` before
    /// each kind has its own API.
    #[doc(hidden)]
    pub fn __put_coordination_for_testing(
        &mut self,
        kind: &str,
        content: &str,
        metadata: Map<String, Value>,
    ) -> Result<Ulid> {
        self.put_coordination(kind, content.to_string(), metadata)
    }

    /// **Not a stable API.** v0.21.0 (13.7): the merge's rules alone, on two
    /// inputs' live records and deleted-row ids, at `now_ms`: what
    /// [`Mnemo::merge_files`] writes, and its report.
    #[doc(hidden)]
    pub fn __merge_records_for_testing(
        a: Vec<Memory>,
        a_deleted: Vec<Ulid>,
        b: Vec<Memory>,
        b_deleted: Vec<Ulid>,
        now_ms: u64,
    ) -> (Vec<Memory>, MergeReport) {
        super::merge::merge_core(
            super::merge::MergeInput {
                records: a,
                deleted: a_deleted,
            },
            super::merge::MergeInput {
                records: b,
                deleted: b_deleted,
            },
            now_ms,
        )
    }

    /// **Not a stable API.** v0.21.0 (v0210-3b, REV-017.1 (C)): this
    /// thread's next [`Mnemo::merge_files`] meets a filesystem without hard
    /// links: `"no-links"` (the copy goes ahead), `"taken"` (a file appears
    /// at the output first) or `"copy-fails"`. Anything else disarms.
    #[doc(hidden)]
    pub fn __merge_without_hard_links_for_testing(what: &str) {
        use super::merge::link_hook::{arm, LinkHook};
        arm(match what {
            "no-links" => Some(LinkHook::Copies),
            "taken" => Some(LinkHook::Taken),
            "copy-fails" => Some(LinkHook::CopyFails),
            _ => None,
        });
    }

    /// **Not a stable API.** v0.21.0 (13.7): what a merge reads from this
    /// handle: its live records and the ids of its deleted rows.
    #[doc(hidden)]
    pub fn __merge_input_for_testing(&mut self) -> Result<(Vec<Memory>, Vec<Ulid>)> {
        let i = self.merge_input()?;
        Ok((i.records, i.deleted))
    }

    /// **Not a stable API.** v0.21.0 (13.7): the wrapped data key in the
    /// header of the file at `path`, read without a key, so a test can see
    /// that a merge's output has a key of its own.
    #[doc(hidden)]
    pub fn __wrapped_key_for_testing(path: impl AsRef<Path>) -> Result<Vec<u8>> {
        let mut file = File::open(path.as_ref())?;
        let mut page = [0u8; PAGE_SIZE];
        file.read_exact(&mut page)?;
        Ok(Header::from_page(&page)?.wrapped_dek.to_vec())
    }

    /// **Not a stable API.** v0.21.0 (RB-020.6): the data key of the file
    /// at `path`, unwrapped with `passphrase`, so a test can see that a
    /// merge's output draws a key of its own (the wrapped bytes differ
    /// anyway: each wrap takes a fresh nonce).
    #[doc(hidden)]
    pub fn __data_key_for_testing(path: impl AsRef<Path>, passphrase: &str) -> Result<Vec<u8>> {
        let mut file = File::open(path.as_ref())?;
        let mut page = [0u8; PAGE_SIZE];
        file.read_exact(&mut page)?;
        let h = Header::from_page(&page)?;
        let kdf = KdfParams {
            m_cost: h.m_cost,
            t_cost: h.t_cost,
            p_cost: h.p_cost,
        };
        let kek = crate::crypto::derive_kek(passphrase.as_bytes(), &h.salt, kdf)?;
        let dek = crate::crypto::unwrap_dek(&kek, &h.dek_nonce, &h.wrapped_dek)?;
        Ok(dek.to_vec())
    }

    /// **Not a stable API.** v0.21.0 (RB-019.8): move this handle's clock
    /// for stamps by `ms` (negative: behind), so a test can run one copy
    /// ahead of another. `MNEMO_DEV_HLC_OFFSET_MS` adds to it.
    #[doc(hidden)]
    pub fn __set_hlc_offset_for_testing(&mut self, ms: i64) {
        self.hlc_clock.offset_ms = ms;
    }

    /// **Not a stable API.** v0.21.0 (RB-019.8): hold this handle's wall
    /// clock for stamps at `at_ms` unix ms (`None`: the real clock again),
    /// so two copies can stamp the same millisecond. Each stamp still goes
    /// past the last, so a held clock gives `at_ms`, `at_ms + 1`, ...
    #[doc(hidden)]
    pub fn __freeze_hlc_clock_for_testing(&mut self, at_ms: Option<u64>) {
        self.hlc_clock.frozen_ms = at_ms;
    }

    /// **Not a stable API.** v0.21.0 (RB-020.4): this handle writes as a
    /// build before v0.21 did: no clock stamp, version or first stamp on a
    /// write (an in-place edit keeps what it found), and no tombstone for a
    /// delete, so a test can make the records an upgraded brain holds.
    #[doc(hidden)]
    pub fn __write_as_older_build_for_testing(&mut self, on: bool) {
        self.hlc_clock.older_build = on;
    }

    /// **Not a stable API.** PERF-2 (v0.19.0): load the ANN index and write
    /// it again as a new base run, in the current layout (or the one
    /// `__set_index_layout_for_testing` asked for), without retraining it.
    #[doc(hidden)]
    pub fn __rewrite_index_base_for_testing(&mut self) -> Result<()> {
        self.ensure_ann()?;
        if self.ann.is_some() {
            self.index_rebase = true;
            self.dirty_index = true;
        }
        self.flush()
    }

    /// **Not a stable API.** PERF-2 (v0.19.0): load the persisted ANN index
    /// the way `ensure_ann` does, timing each stage separately (ms): the
    /// run's pages read and decrypted, the msgpack decode, the `id ->
    /// partition` map, the delta chain, and the reconcile with the catalog.
    /// Also the run's length in bytes and the index's vector count. Returns
    /// an empty list when there's no unloaded v9 index.
    #[doc(hidden)]
    pub fn __index_load_split(&mut self) -> Result<Vec<(&'static str, f64)>> {
        use std::time::Instant;
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
        if !self.ann_unloaded || !self.is_v9() || self.header.index_pages == 0 {
            return Ok(Vec::new());
        }
        let (start, pages, len, gen) = (
            self.header.index_start,
            self.header.index_pages,
            self.header.index_len,
            self.header.index_gen,
        );
        let t = Instant::now();
        let buf = super::load::read_run_bytes(&mut self.pager, start, pages, len, gen)?;
        let read = ms(t);
        let t = Instant::now();
        let mut idx: crate::index::IvfPqIndex = match buf.first() {
            Some(&V9_INDEX_LAYOUT) => {
                crate::index::IvfPqIndex::read_layout2(&mut &buf[1..], buf.len() as u64 - 1)?
            }
            Some(&V9_INDEX_LAYOUT_MSGPACK) => rmp_serde::from_slice(&buf[1..])
                .map_err(|e| MnemoError::Serialize(e.to_string()))?,
            _ => return Ok(Vec::new()),
        };
        let decode = ms(t);
        let layout = buf[0] as f64;
        let t = Instant::now();
        let (chain, n) =
            super::load::apply_index_chain(&mut self.pager, &mut idx, self.header.index_delta)?;
        let chain_ms = ms(t);
        let vectors = idx.info().vectors as f64;
        // The `id -> partition` map isn't part of the load since PERF-2; it's
        // timed here on its own, after the rest.
        let mut probe = idx.clone();
        let t = Instant::now();
        probe.ensure_assignment();
        let assign = ms(t);
        drop(probe);
        idx.enable_log();
        self.ann = Some(idx);
        self.ann_unloaded = false;
        self.index_chain = chain;
        self.index_chain_ops = n;
        let t = Instant::now();
        let (index_start, index_gen) = (self.header.index_start, self.index_gen_now());
        self.reconcile_ann_with_catalog(index_start, index_gen)?;
        let reconcile = ms(t);
        Ok(vec![
            ("read_decrypt_ms", read),
            ("decode_ms", decode),
            ("assignment_ms", assign),
            ("delta_chain_ms", chain_ms),
            ("reconcile_ms", reconcile),
            ("run_bytes", buf.len() as f64),
            ("vectors", vectors),
            ("layout", layout),
        ])
    }

    /// **Not a stable API.** 12.3: create a file protected by a passphrase on
    /// raw-key KDF parameters, which the public paths refuse, so a test can
    /// check what an open, `info`, `verify` and a compaction do with one.
    #[doc(hidden)]
    pub fn __create_passphrase_below_floor_for_testing(
        path: impl Into<PathBuf>,
        passphrase: &str,
        config: MnemoConfig,
    ) -> Result<Mnemo> {
        let config = MnemoConfig {
            kdf: KdfParams::raw_key(),
            ..config
        };
        // Handed over as a key, so the floor check lets it through.
        Self::create_secret_as(
            path.into(),
            passphrase.as_bytes(),
            SecretKind::Key,
            config,
            Create::Truncate,
            None,
        )
    }

    /// **Not a stable API.** TOK-1: from now on this handle splits the
    /// documents it writes with tokenizer 1 and writes unmarked keyword
    /// segments, as every build before v0.15 does, so a test can make the
    /// mixed index an older build leaves without one. It stays on through
    /// later commits on this handle.
    #[doc(hidden)]
    pub fn __write_keywords_like_an_older_build_for_testing(&mut self) {
        self.bm25.like_older_build = true;
    }

    /// **Not a stable API.** TOK-1 (check 3): from now on this handle
    /// writes keyword segments without their mark, but still with the
    /// file's tokenizer, so a test can compare scores with and without it.
    #[doc(hidden)]
    pub fn __write_keyword_segments_without_the_mark_for_testing(&mut self) {
        self.bm25.unmarked = true;
    }

    /// **Not a stable API.** The raw BM25 scores of the live documents that
    /// match `query` (k1 1.2, b 0.75), by memory id, sorted by id: recall
    /// divides them by the best one, which would hide a shift in N, avgdl
    /// or a document's length.
    #[doc(hidden)]
    pub fn __bm25_scores_for_testing(&mut self, query: &str) -> Result<Vec<(Ulid, f32)>> {
        let scores = self.bm25_scores(query, 1.2, 0.75)?;
        let mut out: Vec<(Ulid, f32)> = scores
            .into_iter()
            .map(|(row, s)| (Ulid(self.catalog[row].id), s))
            .collect();
        out.sort_by_key(|(id, _)| *id);
        Ok(out)
    }

    /// **Not a stable API.** The file's on-disk format version.
    /// Writes `m` as a record already in a file would be copied: no
    /// zero-vector check (the dimension check stays). For the zero-vector
    /// fixture only (`examples/gen_v8_zero_vector_fixture.rs`): it stands in
    /// for a file written before the zero-vector rule.
    #[doc(hidden)]
    pub fn __put_unvalidated_for_testing(&mut self, m: Memory) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.put_existing(m)
    }

    #[doc(hidden)]
    pub fn __format_version(&self) -> u16 {
        self.header.version
    }

    /// **Not a stable API.** FMT-9 (a): the committed free list as
    /// `(freed_at_txn, first_page, page_count)` runs, plus the number of
    /// runs pending for the next commit.
    #[doc(hidden)]
    pub fn __free_list(&self) -> (Vec<(u64, u64, u64)>, usize) {
        (
            self.free
                .to_runs()
                .into_iter()
                .map(|r| (r.freed_at, r.page, r.count))
                .collect(),
            self.pending_free.len(),
        )
    }

    /// **Not a stable API.** FMT-9: where a memory's record lives:
    /// `(first_page, page_count, generation)`.
    #[doc(hidden)]
    pub fn __record_location(&self, id: &Ulid) -> Option<(u64, u32, u64)> {
        let idx = self.index.get(id.0, &self.catalog)?;
        let e = &self.catalog[idx];
        (!e.deleted).then_some((e.start_page, e.page_count, e.gen))
    }

    /// **Not a stable API.** v0200-3r: where a memory's vector entry lives:
    /// `(vec_page, vec_slot, vec_gen)` (v9).
    #[doc(hidden)]
    pub fn __vector_location(&self, id: &Ulid) -> Option<(u64, u32, u64)> {
        let idx = self.index.get(id.0, &self.catalog)?;
        let e = &self.catalog[idx];
        e.v9.filter(|_| !e.deleted)
            .map(|r| (r.vec_page, r.vec_slot as u32, r.vec_gen))
    }

    /// **Not a stable API.** FMT-9e: how many thread-hash buckets of turn
    /// records the handle caches hold undecoded (`None` until built).
    #[doc(hidden)]
    pub fn __cold_thread_buckets(&self) -> Option<usize> {
        self.layers.as_ref().map(|l| l.cold_turns.len())
    }

    /// **Not a stable API.** FMT-9e: a live memory's stored record length
    /// and whether the stored body is a zstd frame.
    #[doc(hidden)]
    pub fn __record_storage(&mut self, id: &Ulid) -> Result<(u32, bool)> {
        let idx = self
            .index
            .get(id.0, &self.catalog)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        let e = self.catalog[idx].clone();
        let bytes = self.read_record(&e)?;
        Ok((e.len, bytes.starts_with(&ZSTD_MAGIC)))
    }

    /// **Not a stable API.** FMT-9b: whether the vector column is resident,
    /// and how many vector pages hold live entries.
    #[doc(hidden)]
    pub fn __vector_column(&self) -> (bool, usize) {
        (self.col.resident, self.col.page_rows.len())
    }

    /// **Not a stable API.** FMT-9c.2: `(catalog rows incl. tombstones,
    /// delta runs on the chain, first page of the base run)`.
    #[doc(hidden)]
    pub fn __catalog_shape(&self) -> (usize, usize, u64) {
        (
            self.catalog.len(),
            self.delta_chain.len(),
            self.header.catalog_start,
        )
    }

    /// **Not a stable API.** FMT-9c.3: `(index delta runs on the chain,
    /// first page of the base index run)`.
    #[doc(hidden)]
    pub fn __index_shape(&self) -> (usize, u64) {
        (self.index_chain.len(), self.header.index_start)
    }

    /// **Not a stable API.** RB-015.1: `[(first page, pages)]` of the base
    /// index run and the newest index delta run (`(0, 0)` when absent), so
    /// a test can damage one on disk.
    #[doc(hidden)]
    pub fn __index_runs(&self) -> [(u64, u64); 2] {
        let h = &self.header;
        [
            (h.index_start, h.index_pages),
            (h.index_delta.start, h.index_delta.pages),
        ]
    }

    /// **Not a stable API.** FMT-9d: BM25 keyword scores for `query`, best
    /// first: `(id, score)`, with k1 1.2 and b 0.75.
    #[doc(hidden)]
    pub fn __bm25(&mut self, query: &str) -> Result<Vec<(Ulid, f32)>> {
        let scores = self.bm25_scores(query, 1.2, 0.75)?;
        let mut out: Vec<(Ulid, f32)> = scores
            .into_iter()
            .map(|(row, s)| (Ulid(self.catalog[row].id), s))
            .collect();
        out.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(out)
    }

    /// **Not a stable API.** FMT-9d: write the next whole index with this
    /// layout byte (an unknown one exercises the "treat as no index" path).
    #[doc(hidden)]
    pub fn __set_index_layout_for_testing(&mut self, layout: u8) {
        self.index_layout_override = Some(layout);
    }

    /// **Not a stable API.** FMT-9d: BM25 segments on disk.
    #[doc(hidden)]
    pub fn __bm25_segments(&self) -> usize {
        self.bm25.manifest.segments.len()
    }

    /// **Not a stable API.** The id of the last committed transaction (the
    /// header's `wal_seq`).
    #[doc(hidden)]
    pub fn __transaction_id(&self) -> u64 {
        self.header.wal_seq
    }

    /// **Not a stable API.** FMT-9: the header's next unallocated page.
    #[doc(hidden)]
    pub fn __next_page(&self) -> u64 {
        self.header.next_page
    }

    /// **Not a stable API.** FMT-9 (b): add a placeholder key slot (its
    /// wrapped DEK is random bytes, so it unlocks nothing) to exercise the
    /// key-slot run until Phase 13.2 adds real slots. v9 only.
    #[doc(hidden)]
    pub fn __add_key_slot_for_testing(&mut self, label: &str) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if !self.is_v9() {
            return Err(MnemoError::Invalid("key slots need a v9 file".into()));
        }
        let mut wrapped = vec![0u8; WRAPPED_DEK_LEN];
        wrapped[..crypto::KEY_LEN].copy_from_slice(&crypto::random_dek()[..]);
        self.key_slots.slots.push(KeySlotEntry {
            label: label.to_string(),
            role: 0,
            m_cost: KdfParams::raw_key().m_cost,
            t_cost: KdfParams::raw_key().t_cost,
            p_cost: KdfParams::raw_key().p_cost,
            salt: crypto::random_salt(),
            dek_nonce: crypto::random_nonce(),
            wrapped_dek: wrapped,
        });
        self.dirty_keys = true;
        Ok(())
    }

    /// **Not a stable API.** FMT-9 (b): the key-slot labels.
    #[doc(hidden)]
    pub fn __key_slot_labels(&self) -> Vec<String> {
        self.key_slots
            .slots
            .iter()
            .map(|s| s.label.clone())
            .collect()
    }

    /// **Not a stable API.** FMT-9 (b): the key-slot run's pointer
    /// `(first_page, page_count, generation)`.
    #[doc(hidden)]
    pub fn __key_slot_run(&self) -> (u64, u64, u64) {
        (
            self.header.keys.start,
            self.header.keys.pages,
            self.header.keys.gen,
        )
    }

    /// FMT-9a measurement hook: the ANN shortlist (candidate ids, nearest
    /// first by PQ distance) that recall would rerank, at the given or the
    /// index's effective `n_probe` / `n_rerank`. Empty without an index.
    /// **Unstable diagnostic.**
    #[doc(hidden)]
    pub fn __ann_shortlist(
        &mut self,
        query: &[f32],
        n_probe: Option<usize>,
        n_rerank: Option<usize>,
    ) -> Result<Vec<Ulid>> {
        self.ensure_ann()?;
        Ok(match &self.ann {
            Some(ann) => ann
                .query(query, n_probe, n_rerank)
                .into_iter()
                .map(Ulid)
                .collect(),
            None => Vec::new(),
        })
    }

    /// P-0 device budget breakdown of this file and handle. **Unstable
    /// diagnostic**; see [`BudgetReport`]. Reads the header, the catalog
    /// and file metadata only; nothing is decrypted.
    #[doc(hidden)]
    pub fn __budget_report(&self) -> Result<BudgetReport> {
        let h = &self.header;
        let ps = h.page_size as u64;
        let mut r = BudgetReport {
            file_bytes: std::fs::metadata(&self.path)?.len(),
            page_size: ps,
            header_bytes: ps,
            catalog_run_bytes: h.catalog_pages * ps,
            catalog_payload_bytes: h.catalog_len,
            index_run_bytes: h.index_pages * ps,
            index_payload_bytes: h.index_len,
            index_start_page: h.index_start,
            wal_bytes: h.wal_pages * ps,
            snapshot_manifest_bytes: h.manifest_pages * ps,
            cache_dir_run_bytes: h.cache_pages * ps,
            ..Default::default()
        };
        for e in &self.catalog {
            // FMT-9b.2: packed v9 records share pages (counted once below);
            // a v9 delete frees its pages, so there are no deleted ones.
            let packed = e.v9.is_some_and(|r| r.rec_slot != REC_SPAN);
            let pages = if packed { 0 } else { e.page_count as u64 * ps };
            if e.deleted {
                if e.v9.is_none() {
                    r.deleted_record_page_bytes += pages;
                }
            } else {
                r.record_page_bytes += pages;
                r.record_payload_bytes += e.len as u64;
                r.live_records += 1;
            }
        }
        r.record_page_bytes += self.recs.page_rows.len() as u64 * ps;
        // FMT-9b: the vector column, reported with the records.
        r.record_page_bytes += self.col.page_rows.len() as u64 * ps;
        let accounted = r.header_bytes
            + r.record_page_bytes
            + r.deleted_record_page_bytes
            + r.catalog_run_bytes
            + r.index_run_bytes
            + r.wal_bytes
            + r.snapshot_manifest_bytes
            + r.cache_dir_run_bytes;
        r.unreferenced_bytes = r.file_bytes.saturating_sub(accounted);

        r.ram_catalog = (self.catalog.capacity() * std::mem::size_of::<CatalogEntry>()) as u64;
        r.ram_id_map = self.index.heap_bytes() as u64;
        if let Some(ann) = &self.ann {
            let (c, cb, ids, codes, assign) = ann.heap_bytes();
            r.ram_ann_centroids = c as u64;
            r.ram_ann_codebook = cb as u64;
            r.ram_ann_posting_ids = ids as u64;
            r.ram_ann_posting_codes = codes as u64;
            r.ram_ann_assignment = assign as u64;
        }
        r.ram_cache_dir =
            (self.cache_dir.capacity() * std::mem::size_of::<CacheDirectoryEntry>()) as u64;
        let key_index = self.key_index.as_ref().map_or(0, |k| {
            crate::index::hash_map_bytes::<KeySlot, u128>(k.capacity())
                + k.keys()
                    .map(|s| s.key.capacity() + s.scope_owner.as_ref().map_or(0, |o| o.capacity()))
                    .sum::<usize>()
        });
        let superseded = self.superseded.as_ref().map_or(0, |s| {
            crate::index::hash_map_bytes::<u128, ()>(s.capacity())
        });
        r.ram_key_caches = (key_index + superseded) as u64;
        r.ram_vector_column = (self.col.data.capacity() * 2
            + self.col.norms.capacity() * 4
            + self.col.have.capacity()) as u64;
        r.ram_total = r.ram_catalog
            + r.ram_id_map
            + r.ram_ann_centroids
            + r.ram_ann_codebook
            + r.ram_ann_posting_ids
            + r.ram_ann_posting_codes
            + r.ram_ann_assignment
            + r.ram_cache_dir
            + r.ram_key_caches
            + r.ram_vector_column;
        Ok(r)
    }

    /// **Not a stable API.** v0200-3: the parts of the handle's state that
    /// an incremental catch-up must leave as a fresh open of the file
    /// builds them, as `(name, value)` to compare. Lazily built parts (the
    /// BM25 stats and dicts, the ANN index) are listed only when built;
    /// [`Mnemo::__warm_for_testing`] builds them.
    #[doc(hidden)]
    pub fn __catch_up_parts(&self) -> Vec<(String, String)> {
        fn sorted(m: &std::collections::HashMap<u64, Vec<usize>>) -> String {
            let mut v: Vec<(u64, Vec<usize>)> = m
                .iter()
                .map(|(&p, rows)| {
                    let mut rows = rows.clone();
                    rows.sort_unstable();
                    (p, rows)
                })
                .collect();
            v.sort();
            format!("{v:?}")
        }
        let bad_ids: Vec<usize> = (0..self.catalog.len())
            .filter(|&r| self.index.get(self.catalog[r].id, &self.catalog) != Some(r))
            .collect();
        let mut parts = vec![
            ("header".to_string(), format!("{:?}", self.header)),
            ("catalog".to_string(), format!("{:?}", self.catalog)),
            (
                "id_index".to_string(),
                format!("{} entries, rows not found: {bad_ids:?}", self.index.len),
            ),
            ("record_pages".to_string(), sorted(&self.recs.page_rows)),
            ("vector_pages".to_string(), sorted(&self.col.page_rows)),
            ("manifest".to_string(), format!("{:?}", self.manifest)),
            ("free".to_string(), format!("{:?}", self.free)),
            ("key_slots".to_string(), format!("{:?}", self.key_slots)),
            ("cache_dir".to_string(), format!("{:?}", self.cache_dir)),
            (
                "bm25_manifest".to_string(),
                format!("{:?}", self.bm25.manifest),
            ),
            (
                "delta_chain".to_string(),
                format!("{:?} ({} rows)", self.delta_chain, self.delta_rows),
            ),
        ];
        if let Some(s) = &self.bm25.stats {
            let mut docs: Vec<_> = s.docs.iter().map(|(&d, &v)| (d, v)).collect();
            docs.sort_unstable();
            parts.push((
                "bm25_stats".to_string(),
                format!("{docs:?} total {} avgdl {}", s.total, s.avgdl),
            ));
        }
        if let Some(d) = &self.bm25.dicts {
            parts.push(("bm25_dicts".to_string(), format!("{d:?}")));
        }
        if let Some(a) = &self.ann {
            parts.push((
                "ann".to_string(),
                format!(
                    "{:?} chain {:?} ({} ops)",
                    a.sorted_entries(),
                    self.index_chain,
                    self.index_chain_ops
                ),
            ));
        }
        parts
    }

    /// **Not a stable API.** v0200-3: build what a recall builds lazily:
    /// the ANN index, the resident vector column (every scored row filled)
    /// and the BM25 stats and dicts.
    #[doc(hidden)]
    pub fn __warm_for_testing(&mut self) -> Result<()> {
        self.ensure_ann()?;
        self.ensure_column()?;
        let rows: Vec<usize> = (0..self.catalog.len()).collect();
        self.fill_column_rows(&rows)?;
        self.ensure_bm25()
    }

    /// **Not a stable API.** v0200-3: the resident column's filled rows,
    /// as `(row, fp16 values, norm bits)`.
    #[doc(hidden)]
    pub fn __column_filled(&self) -> Vec<(usize, Vec<u16>, u32)> {
        let d = self.dimensions;
        (0..self.col.have.len())
            .filter(|&i| self.col.have[i])
            .map(|i| {
                (
                    i,
                    self.col.data[i * d..(i + 1) * d].to_vec(),
                    self.col.norms[i].to_bits(),
                )
            })
            .collect()
    }

    /// **Not a stable API.** v0200-3: each live row's id, record page and
    /// vector page, so a test can see a page freed and reused.
    #[doc(hidden)]
    pub fn __row_pages(&self) -> Vec<(Ulid, u64, u64)> {
        self.catalog
            .iter()
            .filter(|e| !e.deleted)
            .map(|e| {
                (
                    Ulid::from(e.id),
                    e.start_page,
                    e.v9.map_or(0, |r| r.vec_page),
                )
            })
            .collect()
    }

    /// **Not a stable API.** v0200-3 (addition (d)): drop one lazily built
    /// part, as a catch-up that changed rows would, so a bench can time
    /// what the next recall pays to build it again. `which`: `key_index`,
    /// `superseded`, `layers`, `agent_set`, `bm25_stats`, `bm25_dicts`,
    /// `ann` or `column`.
    #[doc(hidden)]
    pub fn __drop_lazy_for_testing(&mut self, which: &str) -> Result<()> {
        match which {
            "key_index" => self.key_index = None,
            "superseded" => self.superseded = None,
            "layers" => self.layers = None,
            "agent_set" => self.agent_set = None,
            "bm25_stats" => self.bm25.stats = None,
            "bm25_dicts" => self.bm25.dicts = None,
            "ann" => {
                self.ann = None;
                self.ann_unloaded = self.header.index_pages > 0;
                self.dirty_index = false;
                self.index_persist = IndexPersist::default();
                self.index_chain = Vec::new();
                self.index_chain_ops = 0;
            }
            "column" => self.col = VecColumn::from_catalog(&self.catalog),
            _ => return Err(MnemoError::Invalid(format!("no lazy part {which:?}"))),
        }
        Ok(())
    }
}
