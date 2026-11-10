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
    pub fn rekey(&mut self, new_passphrase: &str, kdf: KdfParams) -> Result<()> {
        self.rekey_secret(new_passphrase.as_bytes(), kdf)
    }

    /// PERF-1 step 7: switch to (or rotate) a raw 256-bit key, recording
    /// [`KdfParams::raw_key`] in the header. Like [`Mnemo::rekey`], only the
    /// wrapped DEK changes; pages aren't rewritten.
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
        // FMT-9 (a): the old region is freed by the next commit (see
        // `ensure_wal_capacity`).
        self.release_pages(old_wal_start, new_pages);
        Ok(())
    }

    /// Decrypt and validate every live record. Returns the count verified
    /// across memories **and** cache entries (FIX-1 R-001.1: now that the
    /// cache survives compaction, `verify()` has to walk it too — the
    /// pre-FIX-1 implementation only checked `self.catalog`, so a
    /// corrupted cache record went unnoticed).
    pub fn verify(&mut self) -> Result<usize> {
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut n = entries.len();
        for e in &entries {
            let bytes = self.read_record(e)?;
            let _m: Memory = self.decode_memory(&bytes)?;
            // FMT-9b: and the row's vector column entry.
            if e.v9.is_some() {
                let (h, _) = self.column_entry(e)?;
                if h.len() != self.dimensions {
                    return Err(MnemoError::Invalid(
                        "vector entry has the wrong width".into(),
                    ));
                }
            }
        }

        // Also decode every live cache entry. Tombstoned entries are
        // skipped — their pages are orphan ciphertext awaiting
        // `compact_file`, same as deleted memories. Expired entries
        // are *still* on disk until compaction, so we verify them too.
        let cache_positions: Vec<(u64, u32, u32, u64)> = self
            .cache_dir
            .iter()
            .filter(|e| !e.deleted)
            .map(|e| (e.start_page, e.page_count, e.len, e.gen))
            .collect();
        for (start_page, page_count, len, gen) in cache_positions {
            let buf = self.read_record_run(start_page, page_count, len, gen)?;
            let _c: CacheEntry = rmp_serde::from_slice(&buf)
                .map_err(|err| MnemoError::Serialize(err.to_string()))?;
            n += 1;
        }
        // FMT-9 (a): no free run may overlap a page the committed state
        // references (records, cache entries, control runs, the WAL).
        if self.is_v9() {
            self.check_free_list_disjoint()?;
        }
        Ok(n)
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

    /// Summary statistics for the open database.
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
    pub fn compact_file(path: &str, passphrase: &str) -> Result<CompactReport> {
        Self::compact_file_secret(path, passphrase.as_bytes(), 0)
    }

    /// PERF-1 step 7: [`Mnemo::compact_file`] for a raw-key database.
    pub fn compact_file_with_key(path: &str, key: &[u8; crypto::KEY_LEN]) -> Result<CompactReport> {
        Self::compact_file_secret(path, key, 0)
    }

    /// FMT-9f: [`Mnemo::compact_file`] into an explicit format: `9` (what
    /// `compact_file` does) or `8`, which keeps a v8 file readable by
    /// v0.10 and earlier. The v8 target is kept for one release.
    pub fn compact_file_to(
        path: &str,
        passphrase: &str,
        format_version: u16,
    ) -> Result<CompactReport> {
        if format_version != VERSION && format_version != VERSION_V9 {
            return Err(MnemoError::Invalid(format!(
                "compact target format {format_version} isn't supported (use 8 or 9)"
            )));
        }
        Self::compact_file_secret(path, passphrase.as_bytes(), format_version)
    }

    pub(super) fn compact_file_secret(
        path: &str,
        secret: &[u8],
        format_version: u16,
    ) -> Result<CompactReport> {
        let mut old = Mnemo::open_secret(path, secret)?;
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
        // FMT-9e: compaction rewrites everything, so it compresses every
        // thread turn, recent or not.
        new.turn_compress_after = 0;
        // ANN-1.1: compaction is "re-tune for the current size" by
        // default. Pass IndexConfig::default() so partitions,
        // subspaces, n_probe and n_rerank are all re-derived from the
        // compacted vector count. Previously we froze the pre-compact
        // (n_probe, n_rerank), which left the new index on stale
        // numbers for the smaller post-compact corpus.
        let want_index = old.has_index();
        let now = memory::now_secs();
        let all = old.memories()?;
        let before = all.len();
        let mut after = 0;
        for m in all {
            if m.is_expired(now) {
                continue;
            }
            new.remember(m)?;
            after += 1;
        }
        if want_index && after > 0 {
            new.build_index_with(IndexConfig::default())?;
        }

        // Count + copy cache entries. We walk `old.cache_dir` by index
        // so each hop reads the directory's live `accessed_at` /
        // `access_count` directly rather than going through
        // `cache_get` (which would also treat read-only semantics,
        // tombstones, etc. — we want the raw, authoritative values).
        let mut cache_before = 0usize;
        let mut cache_after = 0usize;
        let positions: Vec<usize> = (0..old.cache_dir.len()).collect();
        for pos in positions {
            let keep = {
                let e = &old.cache_dir[pos];
                !e.deleted && !e.is_expired(now)
            };
            if keep {
                cache_before += 1;
                new.copy_cache_entry_from(&mut old, pos)?;
                cache_after += 1;
            }
        }

        new.flush()?;
        drop(new);
        drop(old);

        std::fs::rename(&tmp, path)?;
        Ok(CompactReport {
            before,
            after,
            cache_before,
            cache_after,
        })
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
