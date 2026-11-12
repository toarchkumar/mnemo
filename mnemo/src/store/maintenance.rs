//! Maintenance: export, rekey, verify, stats, and compaction (which also upgrades v8 files to v9).

use super::*;

impl Mnemo {
    /// Return the KDF (Argon2id) parameters this file currently uses to
    /// wrap its DEK. Needed by callers that want to [`rekey`] while
    /// keeping the file's original protection level (e.g. the Python
    /// binding's `rekey(fast=False)`, whose docstring promises exactly
    /// that). FIX-1 R-001.5.
    pub fn kdf_params(&self) -> KdfParams {
        self.kdf
    }

    /// Flush any pending writes, then stream the (encrypted) file to
    /// `dest` atomically: writes to a sibling temp file in `dest`'s
    /// parent directory, `sync_all`s it, renames onto `dest`, and
    /// fsyncs the parent directory on Unix so a crash never leaves a
    /// partial export.
    ///
    /// Refuses when `dest` resolves to the file this handle has open
    /// — that case would otherwise `File::create`-truncate the live
    /// database to 0 bytes (`fs::copy` on Unix reads *after* the
    /// `O_TRUNC`, so a naive export-to-self destroys the source).
    /// Unix: compares `(dev, ino)` of both paths via `MetadataExt`,
    /// catching symlinks and hard links. Windows: compares
    /// canonicalized paths; a rename onto an open file also fails on
    /// Windows because of the mandatory lock the handle holds, so the
    /// rename step is a second line of defense.
    ///
    /// Reads the bytes from the already-open file handle, not by
    /// `fs::copy`. On Windows that matters: `LockFileEx` locks are
    /// mandatory, so a second handle opened by `fs::copy` would fail
    /// while this writer still holds the lock.
    ///
    /// FIX-1 R-001.5 + R-002.2 part 3.
    pub fn export_encrypted(&mut self, dest: impl AsRef<Path>) -> Result<()> {
        let dest = dest.as_ref();

        // Step 1 — refuse export to the open file itself. We compare
        // by identity, not string, so symlinks and hard links to
        // `self.path` are caught too. The check runs before any flush
        // so a self-targeted export is a pure no-op.
        if paths_refer_to_same_file(&self.path, dest).unwrap_or(false) {
            return Err(MnemoError::Invalid(format!(
                "export_encrypted refuses to overwrite the open database at {}",
                dest.display()
            )));
        }

        // Step 2 — flush, so on-disk bytes match the live state.
        if !self.read_only {
            self.flush()?;
        }

        // Step 3 — stream from the open handle into a sibling temp.
        // Using the open handle (instead of a second open on `self.path`)
        // means a Windows mandatory lock can't block the copy.
        let parent = dest.parent().ok_or_else(|| {
            MnemoError::Invalid(format!(
                "export_encrypted: dest {} has no parent directory",
                dest.display()
            ))
        })?;
        let tmp = temp_sibling(dest);
        // Scope the temp handle so it closes before the rename on
        // Windows (which refuses to rename over an open file).
        {
            let src = self.pager.file_mut();
            src.seek(SeekFrom::Start(0))?;
            let mut out = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp)?;
            let mut buf = vec![0u8; 1 << 20]; // 1 MiB
            loop {
                let n = src.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
            }
            out.sync_all()?;
            crate::bump_fsync();
        }

        // Step 4 — atomic rename onto `dest`.
        if let Err(e) = std::fs::rename(&tmp, dest) {
            // Best-effort cleanup; propagate the rename error.
            let _ = std::fs::remove_file(&tmp);
            return Err(MnemoError::Io(e));
        }

        // Step 5 — fsync the parent directory on Unix so the rename
        // itself is durable. On Windows directory syncs aren't
        // supported; the rename above is atomic enough on NTFS.
        #[cfg(unix)]
        {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
                crate::bump_fsync();
            }
        }
        #[cfg(not(unix))]
        {
            let _ = parent;
        }

        Ok(())
    }

    /// Change the passphrase. Cheap: re-derives the KEK and re-wraps the DEK;
    /// the encrypted pages are never rewritten.
    ///
    /// **This changes the secret that opens this file from now on; anyone who
    /// had the file and the old secret can still read it and later writes.**
    /// The data key stays the same, so an earlier copy of the file (or of
    /// just its header) and the old passphrase still unlock it. The old wrap
    /// is scrubbed from this file (the previous WAL region is zeroed, and any
    /// stale copy of the old header's key wrap is overwritten); that's
    /// hygiene, not revocation. **To revoke, rotate:** rekey, then
    /// [`Mnemo::compact_file`] (`mnemo rekey --rotate`), which rewrites the
    /// file under a fresh data key. Rotation can't erase old ciphertext the storage hardware still holds: flash keeps rewritten blocks until it reuses them, and that's out of the engine's hands.
    ///
    /// **If it returns an error, the file may already be under the new secret,
    /// or may move to it at the next write** (RB-004.2). Close the handle
    /// without writing anything more, then try both secrets.
    pub fn rekey(&mut self, new_passphrase: &str, kdf: KdfParams) -> Result<()> {
        self.rekey_secret(new_passphrase.as_bytes(), kdf)
    }

    /// PERF-1 step 7: switch to (or rotate) a raw 256-bit key, recording
    /// [`KdfParams::raw_key`] in the header. Like [`Mnemo::rekey`], only the
    /// wrapped DEK changes; pages aren't rewritten, and it doesn't revoke
    /// the old key (see [`Mnemo::rekey`]).
    ///
    /// **If it returns an error, the file may already be under the new secret,
    /// or may move to it at the next write** (RB-004.2). Close the handle
    /// without writing anything more, then try both secrets.
    pub fn rekey_with_key(&mut self, new_key: &[u8; crypto::KEY_LEN]) -> Result<()> {
        self.rekey_secret(new_key, KdfParams::raw_key())
    }

    pub(super) fn rekey_secret(&mut self, secret: &[u8], kdf: KdfParams) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.flush()?;
        let new_salt = crypto::random_salt();
        let new_kek = crypto::derive_kek(secret, &new_salt, kdf)?;
        let new_nonce = crypto::random_nonce();
        let wrapped = crypto::wrap_dek(&new_kek, &new_nonce, self.pager.dek())?;
        if wrapped.len() != WRAPPED_DEK_LEN {
            return Err(MnemoError::Crypto("unexpected wrapped DEK length".into()));
        }
        let mut wrapped_dek = [0u8; WRAPPED_DEK_LEN];
        wrapped_dek.copy_from_slice(&wrapped);

        // SEC-2: the old header's key wrap (salt, DEK nonce, wrapped DEK, as
        // laid out at header bytes 86..162), to scrub once the new header
        // is committed.
        let mut old_wrap =
            Vec::with_capacity(crypto::SALT_LEN + crypto::NONCE_LEN + WRAPPED_DEK_LEN);
        old_wrap.extend_from_slice(&self.header.salt);
        old_wrap.extend_from_slice(&self.header.dek_nonce);
        old_wrap.extend_from_slice(&self.header.wrapped_dek);

        self.header.salt = new_salt;
        self.header.dek_nonce = new_nonce;
        self.header.wrapped_dek = wrapped_dek;
        self.header.m_cost = kdf.m_cost;
        self.header.t_cost = kdf.t_cost;
        self.header.p_cost = kdf.p_cost;
        self.kdf = kdf;

        // DUR-1 (R-002.1): WAL-log the new page 0 before touching it.
        // Allocate a fresh WAL region at the tail so that a crash during
        // the rekey leaves either the old region (old passphrase works)
        // or the new region (new passphrase works) fully committed —
        // overwriting the existing region would risk destroying both.
        let old_wal_start = self.header.wal_start;
        let new_pages = self.header.wal_pages;
        let new_start = self.header.next_page;
        self.header.next_page += new_pages;
        self.header.wal_start = new_start;
        self.header.wal_pages = new_pages;
        self.header.wal_seq += 1;
        self.relocate_header_through_wal(old_wal_start)?;
        // SEC-2 (RB-001.30): the old region holds page-0 images with the old
        // wrap; `relocate_header_through_wal` only zeroed its first frame
        // header. Zero all of it, then overwrite any other stale copy of
        // the old wrap in the file (an earlier, grown or relocated region).
        // Hygiene only: a copy of the file taken before the rekey still
        // opens with the old secret (see `rekey`).
        wal::scrub_region(self.pager.file_mut(), old_wal_start, new_pages)?;
        scrub_bytes(self.pager.file_mut(), &old_wrap)?;
        // FMT-9 (a): the old region is freed by the next commit (see
        // `ensure_wal_capacity`).
        self.release_pages(old_wal_start, new_pages);
        Ok(())
    }

    /// SEC-2 (RB-001.30): write a copy of this database at `dest` under a
    /// **fresh data key**, protected by `passphrase` through `kdf`. It goes
    /// through the compaction path (live memories, the result cache and
    /// the index, no history), so the copy shares no key material with
    /// this file: neither this file's passphrase nor its data key opens
    /// it, and its passphrase opens nothing here. This file is flushed
    /// first (unless read-only) and is otherwise unchanged; `dest` must not
    /// exist. The copy is v9.
    pub fn export_rekeyed(
        &mut self,
        dest: impl AsRef<Path>,
        passphrase: &str,
        kdf: KdfParams,
    ) -> Result<CompactReport> {
        self.export_rekeyed_secret(dest.as_ref(), passphrase.as_bytes(), kdf)
    }

    /// [`Mnemo::export_rekeyed`] protected by a raw 256-bit key (no KDF).
    pub fn export_rekeyed_with_key(
        &mut self,
        dest: impl AsRef<Path>,
        key: &[u8; crypto::KEY_LEN],
    ) -> Result<CompactReport> {
        self.export_rekeyed_secret(dest.as_ref(), key, KdfParams::raw_key())
    }

    pub(super) fn export_rekeyed_secret(
        &mut self,
        dest: &Path,
        secret: &[u8],
        kdf: KdfParams,
    ) -> Result<CompactReport> {
        if paths_refer_to_same_file(&self.path, dest).unwrap_or(false) {
            return Err(MnemoError::Invalid(format!(
                "export_rekeyed refuses to overwrite the open database at {}",
                dest.display()
            )));
        }
        if dest.exists() {
            return Err(MnemoError::Invalid(format!(
                "{} already exists",
                dest.display()
            )));
        }
        if !self.read_only {
            self.flush()?;
        }
        let mut name = dest.file_name().unwrap_or_default().to_os_string();
        name.push(".export-tmp");
        let tmp = dest.with_file_name(name);
        let _ = std::fs::remove_file(&tmp);
        let result = (|| {
            let mut new = Mnemo::create_secret(
                &tmp,
                secret,
                MnemoConfig {
                    dimensions: self.dimensions,
                    kdf,
                    store_float_vectors: self.float_vectors(),
                    ..Default::default()
                },
            )?;
            let report = self.copy_live_into(&mut new)?;
            new.flush()?;
            drop(new);
            std::fs::rename(&tmp, dest)?;
            Ok(report)
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    /// The compaction copy: every live, unexpired memory and cache entry of
    /// `self` into `new` (a freshly created file), and the index if `self`
    /// has one. Counts as [`CompactReport`].
    pub(super) fn copy_live_into(&mut self, new: &mut Mnemo) -> Result<CompactReport> {
        // FMT-9e: compaction rewrites everything, so it compresses every
        // thread turn, recent or not.
        new.turn_compress_after = 0;
        // ANN-1.1: compaction is "re-tune for the current size" by
        // default. Pass IndexConfig::default() so partitions,
        // subspaces, n_probe and n_rerank are all re-derived from the
        // compacted vector count. Previously we froze the pre-compact
        // (n_probe, n_rerank), which left the new index on stale
        // numbers for the smaller post-compact corpus.
        let want_index = self.has_index();
        let now = memory::now_secs();
        let all = self.memories()?;
        let before = all.len();
        let mut after = 0;
        let mut zero_vector_manifests = 0;
        let mut zero_vector_memories = 0;
        for mut m in all {
            if m.is_expired(now) {
                continue;
            }
            // A file from before the zero-vector rule (Phase 3 PR 10a) can
            // hold all-zero vectors. A legacy onboarding or manifest memory
            // gets the sentinel unit vector a current manifest has, so it
            // behaves like a new one; any other is carried unchanged.
            if embed::is_zero_vector(&m.vector) {
                if crud::is_onboarding(&m) || is_manifest(&m) {
                    m.vector = Memory::scaffold_manifest(self.dimensions).vector;
                    zero_vector_manifests += 1;
                } else {
                    zero_vector_memories += 1;
                }
            }
            // The engine's own copy: `_mnemo.*` keys come through as they
            // are, and the write-time zero-vector check doesn't apply.
            new.put_existing(m)?;
            after += 1;
        }
        if want_index && after > 0 {
            new.build_index_with(IndexConfig::default())?;
        }

        // Count + copy cache entries. We walk `self.cache_dir` by index
        // so each hop reads the directory's live `accessed_at` /
        // `access_count` directly rather than going through
        // `cache_get` (which would also treat read-only semantics,
        // tombstones, etc. — we want the raw, authoritative values).
        let mut cache_before = 0usize;
        let mut cache_after = 0usize;
        let positions: Vec<usize> = (0..self.cache_dir.len()).collect();
        for pos in positions {
            let (keep, reserved) = {
                let e = &self.cache_dir[pos];
                (
                    !e.deleted && !e.is_expired(now),
                    embed::is_reserved_namespace(&e.namespace),
                )
            };
            if keep {
                // EMBED-ID-1: the engine's own entries (the embedder
                // identity) are copied but not counted.
                new.copy_cache_entry_from(self, pos)?;
                if !reserved {
                    cache_before += 1;
                    cache_after += 1;
                }
            }
        }
        Ok(CompactReport {
            before,
            after,
            cache_before,
            cache_after,
            zero_vector_manifests,
            zero_vector_memories,
        })
    }

    /// Decrypt and validate every live record. Returns the count verified
    /// across memories **and** cache entries (FIX-1 R-001.1: now that the
    /// cache survives compaction, `verify()` has to walk it too — the
    /// pre-FIX-1 implementation only checked `self.catalog`, so a
    /// corrupted cache record went unnoticed).
    pub fn verify(&mut self) -> Result<usize> {
        Ok(self.verify_report()?.records)
    }

    /// [`Mnemo::verify`], also counting the memories whose vector is all
    /// zeros. A file written before the zero-vector rule (Phase 3 PR 10a)
    /// can hold them; they decode fine but score 0.0 on every vector query.
    /// `mnemo verify` warns about them, and compaction carries them
    /// (giving a legacy manifest the sentinel vector).
    pub fn verify_report(&mut self) -> Result<VerifyReport> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut n = entries.len();
        let mut zero_vector_memories = 0;
        for e in &entries {
            let bytes = self.read_record(e)?;
            let m: Memory = self.decode_memory(&bytes)?;
            // FMT-9b: and the row's vector column entry.
            let zero = if e.v9.is_some() {
                let (h, _) = self.column_entry(e)?;
                if h.len() != self.dimensions {
                    return Err(MnemoError::Invalid(
                        "vector entry has the wrong width".into(),
                    ));
                }
                // fp16 +0.0 and -0.0.
                !h.is_empty() && h.iter().all(|b| b & 0x7fff == 0)
            } else {
                embed::is_zero_vector(&m.vector)
            };
            if zero {
                zero_vector_memories += 1;
            }
        }

        // Also decode every live cache entry. Tombstoned entries are
        // skipped — their pages are orphan ciphertext awaiting
        // `compact_file`, same as deleted memories. Expired entries
        // are *still* on disk until compaction, so we verify them too.
        let cache_positions: Vec<(u64, u32, u32, u64, bool)> = self
            .cache_dir
            .iter()
            .filter(|e| !e.deleted)
            .map(|e| {
                let reserved = embed::is_reserved_namespace(&e.namespace);
                (e.start_page, e.page_count, e.len, e.gen, reserved)
            })
            .collect();
        for (start_page, page_count, len, gen, reserved) in cache_positions {
            let buf = self.read_record_run(start_page, page_count, len, gen)?;
            let _c: CacheEntry = rmp_serde::from_slice(&buf)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
            // EMBED-ID-1: the engine's own entries are checked, not counted.
            if !reserved {
                n += 1;
            }
        }
        // FMT-9 (a): no free run may overlap a page the committed state
        // references (records, cache entries, control runs, the WAL).
        if self.is_v9() {
            self.check_free_list_disjoint()?;
        }
        Ok(VerifyReport {
            records: n,
            zero_vector_memories,
        })
    }

    /// FMT-9 (a): fail if the free list (or a pending free) overlaps any
    /// page the current state references.
    pub(super) fn check_free_list_disjoint(&self) -> Result<()> {
        let h = &self.header;
        let mut live: Vec<(u64, u64, &str)> = vec![
            (h.wal_start, h.wal_pages, "WAL region"),
            (h.catalog_start, h.catalog_pages, "catalog run"),
            (h.index_start, h.index_pages, "index run"),
            (h.manifest_start, h.manifest_pages, "manifest run"),
            (h.cache_start, h.cache_pages, "cache directory run"),
            (h.free.start, h.free.pages, "free-list run"),
            (h.keys.start, h.keys.pages, "key-slot run"),
        ];
        live.extend(
            self.catalog
                .iter()
                .filter(|e| !e.deleted)
                .map(|e| (e.start_page, e.page_count as u64, "record")),
        );
        live.extend(
            self.cache_dir
                .iter()
                .filter(|e| !e.deleted)
                .map(|e| (e.start_page, e.page_count as u64, "cache entry")),
        );
        live.extend(self.col.page_rows.keys().map(|&p| (p, 1, "vector page")));
        live.extend(
            self.delta_chain
                .iter()
                .map(|r| (r.start, r.pages, "catalog delta run")),
        );
        live.extend(
            self.index_chain
                .iter()
                .map(|r| (r.start, r.pages, "index delta run")),
        );
        live.push((
            self.header.bm25.start,
            self.header.bm25.pages,
            "BM25 manifest",
        ));
        live.extend(
            self.bm25
                .manifest
                .segments
                .iter()
                .map(|t| (t.0, t.1, "BM25 segment")),
        );
        for (p, c, what) in live {
            if c == 0 {
                continue;
            }
            if self.free.overlaps(p, c) {
                return Err(MnemoError::Invalid(format!(
                    "free list overlaps a live {what} at page {p} (+{c})"
                )));
            }
            if self
                .pending_free
                .iter()
                .any(|&(q, n)| q < p + c && p < q + n)
            {
                return Err(MnemoError::Invalid(format!(
                    "a pending free overlaps a live {what} at page {p} (+{c})"
                )));
            }
        }
        Ok(())
    }

    /// The statistics that need no record decrypts and no index load
    /// (v0.12.2, RB-002.3): counts, sizes, the format, whether an index
    /// exists, the recall mode and the bytes the vector column needs. The
    /// first [`Mnemo::stats`] on a handle decrypts every record (for its
    /// agent list) and loads a persisted index; this is what a phone app
    /// polling the recall mode should call.
    pub fn stats_summary(&self) -> StatsSummary {
        let deleted = self.catalog.iter().filter(|e| e.deleted).count();
        let v9 = self.is_v9();
        let column = if v9 {
            (self.scored_rows() * crate::veccol::entry_len(self.dimensions)) as u64
        } else {
            0
        };
        let recall_mode = if self.has_index() {
            RecallMode::Indexed
        } else if v9 && column <= self.vector_cache_budget as u64 {
            RecallMode::ExactResident
        } else {
            RecallMode::ExactPaged
        };
        let rp = self.restore_points();
        StatsSummary {
            memories: self.len(),
            deleted,
            dimensions: self.dimensions,
            file_bytes: self.header.next_page * PAGE_SIZE as u64,
            format_version: self.header.version,
            wal_pages: self.header.wal_pages,
            has_index: self.has_index(),
            recall_mode,
            vector_column_bytes: column,
            snapshots_retained: rp.retained,
            oldest_restorable_at: rp.oldest_at,
            snapshot_pinned_bytes: rp.pinned_bytes,
        }
    }

    /// Summary statistics for the open database. The first call on a
    /// handle decrypts every record (to list the agents) and loads a
    /// persisted index; [`Mnemo::stats_summary`] does neither.
    pub fn stats(&mut self) -> Result<Stats> {
        let deleted = self.catalog.iter().filter(|e| e.deleted).count();
        // FIX-1 R-001.8 part 2: build the agent set lazily on first
        // call per handle, then maintain in `put` / invalidate in
        // `delete`. Pre-FIX-1 every `stats()` call decrypted every
        // live memory (O(N) AES-GCM decrypts just to list a usually-
        // tiny set of distinct agent IDs). On a 1M-memory database
        // this was in the multi-second range on hardware AES.
        if self.agent_set.is_none() {
            let built: std::collections::BTreeSet<String> =
                self.memories()?.into_iter().map(|m| m.agent_id).collect();
            self.agent_set = Some(built);
        }
        let agents: Vec<String> = self
            .agent_set
            .as_ref()
            .expect("agent_set just filled above")
            .iter()
            .cloned()
            .collect();
        let file_bytes = self.header.next_page * PAGE_SIZE as u64;
        let recall_mode = self.recall_mode()?;
        let rp = self.restore_points();
        Ok(Stats {
            memories: self.len(),
            deleted,
            dimensions: self.dimensions,
            file_bytes,
            agents,
            encrypted: self.header.flags & FLAG_ENCRYPTED != 0,
            created_at: self.header.created_at,
            index: {
                self.ensure_ann()?;
                self.ann.as_ref().map(|a| a.info())
            },
            wal_pages: self.header.wal_pages,
            fsyncs_last_flush: self.fsyncs_last_flush,
            format_version: self.header.version,
            recall_mode,
            snapshots_retained: rp.retained,
            oldest_restorable_at: rp.oldest_at,
            snapshot_pinned_bytes: rp.pinned_bytes,
        })
    }

    /// Rewrite the file, dropping tombstoned and expired memories and
    /// reclaiming stale pages left by updates. Done out-of-place via a temp
    /// file and an atomic rename.
    ///
    /// FIX-1 R-001.1 + R-002.2 part 2: the result cache now survives
    /// compaction. Every live, non-expired directory entry is copied
    /// into the new file verbatim — `created_at`, `ttl_secs`,
    /// `cost_hint_ms`, `key_preview`, `size_bytes`, `content_type`,
    /// `vector` and `model` come from the record body; `accessed_at`
    /// and `access_count` come from the *directory* (the record body
    /// only holds the insert-time snapshot — the directory is the
    /// source of truth for the live counts, as of v5's recall trick).
    /// Tombstoned and TTL-expired entries are dropped. Pre-FIX-1
    /// compaction kept only the memory catalog and silently emptied
    /// the cache; the Planner's interim README caveat (c417d2e) called
    /// that out — this PR removes the surprise.
    ///
    /// The output is always v9, so this is also how a v4–v8 file is
    /// converted: the only read-write open that accepts one (the v8
    /// sunset, v0.13). It keeps every live memory.
    pub fn compact_file(path: &str, passphrase: &str) -> Result<CompactReport> {
        Self::compact_file_secret(path, passphrase.as_bytes(), 0)
    }

    /// PERF-1 step 7: [`Mnemo::compact_file`] for a raw-key database.
    pub fn compact_file_with_key(path: &str, key: &[u8; crypto::KEY_LEN]) -> Result<CompactReport> {
        Self::compact_file_secret(path, key, 0)
    }

    pub(super) fn compact_file_secret(
        path: &str,
        secret: &[u8],
        format_version: u16,
    ) -> Result<CompactReport> {
        // The one read-write open that takes a v4–v8 file (the v8 sunset).
        let mut old = Mnemo::open_secret_with(path, secret, true)?;
        let dims = old.dimensions;
        let kdf = old.kdf;
        let tmp = format!("{path}.compact-tmp");

        let mut new = Mnemo::create_secret(
            &tmp,
            secret,
            MnemoConfig {
                dimensions: dims,
                kdf,
                // FMT-9f: compaction writes v9 (upgrading a v8 file),
                // unless `compact_file_to` asked for v8. A v9 file that
                // keeps float vectors keeps them.
                format_version,
                store_float_vectors: old.float_vectors(),
                ..Default::default()
            },
        )?;
        let report = old.copy_live_into(&mut new)?;

        new.flush()?;
        drop(new);
        drop(old);

        std::fs::rename(&tmp, path)?;
        Ok(report)
    }

    /// Copy one live cache entry from `old` at directory position
    /// `pos` into `self` (a freshly-created compaction target).
    /// Decrypts the old record body, substitutes the old *directory*'s
    /// `accessed_at` + `access_count` (the record body's copies are
    /// stale as of v5's directory-only hit path), re-encodes, writes
    /// a fresh record run, and pushes a matching directory entry.
    ///
    /// Caller is expected to have already filtered out tombstoned /
    /// expired entries and to call this on a read-write handle.
    pub(crate) fn copy_cache_entry_from(&mut self, old: &mut Mnemo, pos: usize) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        // Snapshot the directory fields we need, so the borrow on
        // `old.cache_dir` ends before we touch `old.pager`.
        let (start_page, page_count, len, dir_accessed_at, dir_access_count, old_gen) = {
            let e = &old.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.accessed_at,
                e.access_count,
                e.gen,
            )
        };

        // Read + decrypt the old record body.
        let buf = old.read_record_run(start_page, page_count, len, old_gen)?;
        let mut entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;

        // Overwrite the stale body copies with the directory's live
        // values before persisting. Everything else — `created_at`,
        // `ttl_secs`, `cost_hint_ms`, `key_preview`, `size_bytes`,
        // `content_type`, `value`, `vector`, `model` — stays as the
        // body already recorded it.
        entry.accessed_at = dir_accessed_at;
        entry.access_count = dir_access_count;

        let bytes =
            rmp_serde::to_vec(&entry).map_err(|err| MnemoError::Serialize(err.to_string()))?;
        let (new_start_page, new_page_count, gen) = self.write_record(&bytes)?;

        let dir_entry = CacheDirectoryEntry {
            namespace: entry.namespace.clone(),
            key_hash: entry.key_hash,
            start_page: new_start_page,
            page_count: new_page_count,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: dir_accessed_at,
            access_count: dir_access_count,
            size_bytes: entry.size_bytes,
            ttl_secs: entry.ttl_secs,
            created_at: entry.created_at,
            vector: entry.vector.clone(),
            model: entry.model.clone(),
            gen,
        };
        let new_pos = self.cache_dir.len();
        self.cache_idx
            .insert(&entry.namespace, entry.key_hash, new_pos);
        self.cache_dir.push(dir_entry);
        self.dirty_cache = true;
        Ok(())
    }
}

/// SEC-2 (RB-001.30): overwrite every occurrence of `needle` in the file
/// with zeros, then sync. Rekey uses it for stale copies of the old header's
/// key wrap: they're plaintext (page 0 is sealed, not encrypted) and can sit
/// in WAL regions that were grown, relocated or freed. Every other page is
/// AES-GCM ciphertext, where a 76-byte random match doesn't happen.
pub(super) fn scrub_bytes(file: &mut std::fs::File, needle: &[u8]) -> Result<usize> {
    if needle.is_empty() {
        return Ok(0);
    }
    let len = file.seek(SeekFrom::End(0))?;
    let chunk = 1 << 20;
    let overlap = needle.len() - 1;
    let mut hits: Vec<u64> = Vec::new();
    let mut buf = vec![0u8; chunk + overlap];
    let mut pos: u64 = 0;
    while pos < len {
        let want = (chunk as u64 + overlap as u64).min(len - pos) as usize;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf[..want])?;
        let mut i = 0;
        while i + needle.len() <= want {
            if buf[i..i + needle.len()] == *needle {
                hits.push(pos + i as u64);
                i += needle.len();
            } else {
                i += 1;
            }
        }
        pos += chunk as u64;
    }
    hits.dedup();
    let zeros = vec![0u8; needle.len()];
    for off in &hits {
        file.seek(SeekFrom::Start(*off))?;
        file.write_all(&zeros)?;
    }
    if !hits.is_empty() {
        file.sync_all()?;
        crate::bump_fsync();
    }
    Ok(hits.len())
}
