//! Hidden (doc(hidden)) test and benchmark hooks: not a stable API.

use super::*;

impl Mnemo {
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
}
