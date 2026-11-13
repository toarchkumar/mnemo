//! The Phase 10 result cache API: exact-key and semantic put/get, purge, budgets, flush policy and eviction.

use super::*;

impl Mnemo {
    /// Insert or overwrite a cache entry at `(namespace, key)`.
    ///
    /// The engine SHA-256-hashes `key` — callers pass the raw input
    /// string. Persists to disk on the next `flush()` under the current
    /// flush policy (see [`Mnemo::set_cache_flush_policy`]); in
    /// [`CacheFlushPolicy::Batched`] the engine may call `flush()`
    /// internally when a threshold trips.
    ///
    /// On overflow of the namespace's [`CacheBudget`] (default 10,000
    /// entries / 64 MiB) the LRU entry is tombstoned before the new
    /// insert. Tombstones are reclaimed by
    /// [`Mnemo::compact_file`], not the runtime flush.
    ///
    /// Returns [`MnemoError::ReadOnly`] on a read-only handle.
    pub fn cache_put(
        &mut self,
        namespace: &str,
        key: &str,
        value: &[u8],
        opts: CachePutOpts,
    ) -> Result<()> {
        reject_reserved(namespace)?;
        self.cache_put_inner(namespace, key, value, opts)
    }

    /// [`Mnemo::cache_put`] without the reserved-namespace check: the
    /// engine's own entries (EMBED-ID-1's identity) go through here.
    pub(super) fn cache_put_inner(
        &mut self,
        namespace: &str,
        key: &str,
        value: &[u8],
        opts: CachePutOpts,
    ) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let key_hash = result_cache::hash_key(key);
        let key_preview = result_cache::key_preview(key);
        let now = memory::now_secs();
        let size_bytes = value.len() as u64;

        let entry = CacheEntry {
            namespace: namespace.to_string(),
            key_hash,
            key_preview: key_preview.clone(),
            value: value.to_vec(),
            content_type: opts.content_type.clone(),
            created_at: now,
            accessed_at: now,
            access_count: 0,
            ttl_secs: opts.ttl_secs,
            cost_hint_ms: opts.cost_hint_ms,
            size_bytes,
            vector: None,
            model: None,
        };
        let bytes = rmp_serde::to_vec(&entry).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (start_page, page_count, gen) = self.write_record(&bytes)?;

        // Tombstone any prior entry for this key. Its pages remain on
        // disk until `compact_file`, exactly like memory-record
        // overwrites — the tombstone just hides it from lookups.
        if let Some(prev_pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[prev_pos].deleted = true;
            self.release_cache_pages(prev_pos);
            self.cache_idx.remove(namespace, &key_hash);
        }

        let dir_entry = CacheDirectoryEntry {
            namespace: namespace.to_string(),
            key_hash,
            start_page,
            page_count,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: now,
            access_count: 0,
            size_bytes,
            ttl_secs: opts.ttl_secs,
            created_at: now,
            vector: None,
            model: None,
            gen,
        };
        let pos = self.cache_dir.len();
        self.cache_dir.push(dir_entry);
        self.cache_idx.insert(namespace, key_hash, pos);

        // Enforce budget — evict LRU tombstones until the namespace
        // fits under both caps.
        self.evict_to_budget(namespace)?;

        self.dirty_cache = true;
        self.cache_batch.record_dirty();

        // Batched policy: auto-flush if a threshold tripped. Successful
        // `flush()` calls `cache_batch.reset()` for us.
        if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        Ok(())
    }

    /// Look up `(namespace, key)`. Returns `None` on miss (either the
    /// key was never inserted or the entry is TTL-expired). On hit
    /// updates catalog-side access stats (mirrors the recall v5-trick);
    /// on a read-only handle the stats update is silently skipped.
    ///
    /// A miss is recorded as a miss in this handle's counters even if
    /// the key was TTL-expired — from the caller's perspective the two
    /// are indistinguishable.
    pub fn cache_get(&mut self, namespace: &str, key: &str) -> Result<Option<CachedValue>> {
        reject_reserved(namespace)?;
        // FIX-1 R-001.8 part 4: under `Batched`, honor `max_age` on
        // reads too. Pre-FIX-1 the age check only fired on `cache_put`,
        // so a workload that wrote a few entries and then switched to
        // pure reads could sit with its pending batch past the
        // nominal max_age indefinitely. `max_dirty` is already
        // maintained per-write; this adds the symmetric time-bound
        // check on the read path. Only runs on read-write handles —
        // flush() returns ReadOnly for shared-lock readers.
        if !self.read_only && self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        let key_hash = result_cache::hash_key(key);
        let pos = match self.cache_idx.get(namespace, &key_hash) {
            Some(p) => p,
            None => {
                self.cache_counters.misses += 1;
                return Ok(None);
            }
        };
        let now = memory::now_secs();
        // Snapshot the directory entry fields we need before any
        // mutation — the borrow checker gets grumpy otherwise.
        let (start_page, page_count, len, ttl_secs, expired, created_at, gen) = {
            let e = &self.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.ttl_secs,
                e.is_expired(now),
                e.created_at,
                e.gen,
            )
        };
        if expired {
            // Treat expired as a miss for the caller. Don't tombstone
            // here — cache_purge does that. Just skip.
            self.cache_counters.misses += 1;
            return Ok(None);
        }
        // Decrypt and decode the record body.
        let buf = self.read_record_run(start_page, page_count, len, gen)?;
        let entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;

        // Bump access stats catalog-only (v5 trick — no full record
        // rewrite on a hit). Skipped on read-only handles so a shared-lock
        // reader can't dirty the catalog.
        if !self.read_only {
            self.cache_dir[pos].accessed_at = now;
            self.cache_dir[pos].access_count = self.cache_dir[pos].access_count.saturating_add(1);
            self.dirty_cache = true;
        } else if let Some(delta) = self.access_delta.as_mut() {
            // 12.4: a shared read keeps the hit for the next commit.
            let e = &self.cache_dir[pos];
            delta.cache_hit(&e.namespace, e.key_hash, now);
        }
        self.cache_counters.hits += 1;
        // On a mutating handle: access_count reflects the just-bumped
        // value. On a read-only handle: the on-disk directory value is
        // unchanged, so returning it verbatim is honest (the caller's
        // reads did not, and cannot, alter the persisted counter).
        //
        // FIX-1 R-002.2 part 1: `accessed_at` must also come from the
        // directory on a read-only handle, not from `entry.accessed_at`
        // in the record body. The record body is written once at insert
        // time and never updated on a hit — every live `accessed_at`
        // update lands in the directory (see the `!self.read_only`
        // branch above). The pre-FIX-1 code returned
        // `entry.accessed_at`, which is really the entry's **creation
        // time** by then, so a read-only reader saw "last accessed =
        // when it was inserted" even after many successful hits by
        // concurrent writers.
        let access_count = self.cache_dir[pos].access_count;
        let accessed_at = if self.read_only {
            self.cache_dir[pos].accessed_at
        } else {
            now
        };
        Ok(Some(CachedValue {
            value: entry.value,
            content_type: entry.content_type,
            created_at,
            accessed_at,
            access_count,
            ttl_secs,
        }))
    }

    /// Tombstone the entry at `(namespace, key)`. Silently succeeds if
    /// no such entry exists. Space is reclaimed by `compact_file`.
    pub fn cache_delete(&mut self, namespace: &str, key: &str) -> Result<()> {
        reject_reserved(namespace)?;
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let key_hash = result_cache::hash_key(key);
        if let Some(pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[pos].deleted = true;
            self.release_cache_pages(pos);
            self.cache_idx.remove(namespace, &key_hash);
            self.dirty_cache = true;
            self.cache_batch.record_dirty();
            if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
                self.flush()?;
            }
        }
        Ok(())
    }

    /// Purge cache entries. If `namespace` is `Some`, restrict to that
    /// namespace; otherwise purge across the whole cache. If
    /// `expired_only` is `true`, only TTL-expired entries are removed;
    /// otherwise every live entry in the target scope is tombstoned.
    /// Returns the number of entries newly tombstoned.
    pub fn cache_purge(&mut self, namespace: Option<&str>, expired_only: bool) -> Result<usize> {
        if let Some(ns) = namespace {
            reject_reserved(ns)?;
        }
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let now = memory::now_secs();
        let mut purged = 0usize;
        // Iterate the directory by index; mark matching entries as
        // deleted and pull them out of the in-memory index.
        for pos in 0..self.cache_dir.len() {
            if self.cache_dir[pos].deleted
                || embed::is_reserved_namespace(&self.cache_dir[pos].namespace)
            {
                continue;
            }
            if let Some(ns) = namespace {
                if self.cache_dir[pos].namespace != ns {
                    continue;
                }
            }
            if expired_only && !self.cache_dir[pos].is_expired(now) {
                continue;
            }
            let key_hash = self.cache_dir[pos].key_hash;
            let ns = self.cache_dir[pos].namespace.clone();
            self.cache_dir[pos].deleted = true;
            self.release_cache_pages(pos);
            self.cache_idx.remove(&ns, &key_hash);
            purged += 1;
        }
        if purged > 0 {
            self.dirty_cache = true;
            self.cache_batch.record_dirty();
            if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
                self.flush()?;
            }
        }
        Ok(purged)
    }

    /// Statistics for `namespace` (or the whole cache if `None`).
    /// `hits`/`misses`/`evictions` are per-handle in-memory counters
    /// that reset on reopen; `entries`/`bytes` reflect the durable
    /// on-disk state (live entries only, tombstones excluded).
    pub fn cache_stats(&self, namespace: Option<&str>) -> CacheStats {
        let mut entries = 0usize;
        let mut bytes = 0u64;
        for e in &self.cache_dir {
            if e.deleted || embed::is_reserved_namespace(&e.namespace) {
                continue;
            }
            if let Some(ns) = namespace {
                if e.namespace != ns {
                    continue;
                }
            }
            entries += 1;
            bytes += e.size_bytes;
        }
        let hits = self.cache_counters.hits;
        let misses = self.cache_counters.misses;
        let total = hits + misses;
        let hit_rate = if total > 0 {
            hits as f64 / total as f64
        } else {
            0.0
        };
        CacheStats {
            entries,
            bytes,
            hits,
            misses,
            hit_rate,
            evictions: self.cache_counters.evictions,
        }
    }

    /// Set (or update) the eviction budget for `namespace`. Applies to
    /// subsequent `cache_put` calls; does not retroactively evict
    /// entries that already sit under a looser cap.
    pub fn set_cache_budget(&mut self, namespace: &str, budget: CacheBudget) {
        self.cache_budgets.insert(namespace.to_string(), budget);
    }

    /// Set the per-handle cache flush policy. Defaults to
    /// [`CacheFlushPolicy::Strict`]. Memory writes are never batched;
    /// this affects only cache mutations.
    pub fn set_cache_flush_policy(&mut self, policy: CacheFlushPolicy) {
        self.cache_flush_policy = policy;
        // Switching mid-run: if we're moving to Strict and the batch is
        // non-empty, the pending count still counts toward dirty_cache
        // which the next flush() will consume.
    }

    /// FMT-9 (a): free a just-tombstoned cache entry's record pages at the
    /// next commit (v9; no-op on v8).
    pub(super) fn release_cache_pages(&mut self, pos: usize) {
        let (p, c) = (
            self.cache_dir[pos].start_page,
            self.cache_dir[pos].page_count as u64,
        );
        self.release_pages(p, c);
    }

    /// Evict LRU live entries in `namespace` until it fits under both
    /// budget caps. Called from `cache_put` after the insert; the new
    /// entry is included in the count so we always leave room.
    pub(super) fn evict_to_budget(&mut self, namespace: &str) -> Result<()> {
        let budget = self
            .cache_budgets
            .get(namespace)
            .copied()
            .unwrap_or_default();
        loop {
            // Sum current live count + bytes in the namespace.
            let mut live_count = 0usize;
            let mut live_bytes = 0u64;
            for e in &self.cache_dir {
                if !e.deleted && e.namespace == namespace {
                    live_count += 1;
                    live_bytes += e.size_bytes;
                }
            }
            if live_count <= budget.max_entries && live_bytes <= budget.max_bytes {
                return Ok(());
            }
            // Find the LRU (oldest accessed_at) live entry.
            let victim_pos = self
                .cache_dir
                .iter()
                .enumerate()
                .filter(|(_, e)| !e.deleted && e.namespace == namespace)
                .min_by_key(|(_, e)| e.accessed_at)
                .map(|(i, _)| i);
            let Some(pos) = victim_pos else {
                // Nothing to evict but still over budget — the single
                // just-inserted entry itself is bigger than
                // max_bytes. Nothing we can safely do here without
                // rejecting the put; leave it and return.
                return Ok(());
            };
            let key_hash = self.cache_dir[pos].key_hash;
            let ns = self.cache_dir[pos].namespace.clone();
            self.cache_dir[pos].deleted = true;
            self.release_cache_pages(pos);
            self.cache_idx.remove(&ns, &key_hash);
            self.cache_counters.evictions += 1;
        }
    }

    // === Semantic cache (Phase 10.2) ==================================

    /// Insert or overwrite a **semantic** cache entry — same as
    /// [`Mnemo::cache_put`] but with a required embedding vector and
    /// model identifier. The vector is stored on the directory entry
    /// so [`Mnemo::cache_get_semantic`] can score without decrypting
    /// record bodies.
    ///
    /// Vector dimensionality is validated against the database's
    /// configured `dimensions` — a mismatch returns
    /// [`MnemoError::DimensionMismatch`], the same signal `remember`
    /// gives for the memory catalog.
    ///
    /// The `model` string is matched **exactly** on lookup — a
    /// different model's cache is invisible to a given query. Pick
    /// canonical identifiers (`"text-embedding-3-small"`,
    /// `"bge-large-en-v1.5"`, ...) and stick to them.
    pub fn cache_put_semantic(
        &mut self,
        namespace: &str,
        key: &str,
        vector: Vec<f32>,
        value: &[u8],
        opts: SemanticCachePutOpts,
    ) -> Result<()> {
        reject_reserved(namespace)?;
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let key_hash = result_cache::hash_key(key);
        let key_preview = result_cache::key_preview(key);
        let now = memory::now_secs();
        let size_bytes = value.len() as u64;

        let entry = CacheEntry {
            namespace: namespace.to_string(),
            key_hash,
            key_preview: key_preview.clone(),
            value: value.to_vec(),
            content_type: opts.content_type.clone(),
            created_at: now,
            accessed_at: now,
            access_count: 0,
            ttl_secs: opts.ttl_secs,
            cost_hint_ms: opts.cost_hint_ms,
            size_bytes,
            vector: Some(vector.clone()),
            model: Some(opts.model.clone()),
        };
        let bytes = rmp_serde::to_vec(&entry).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        let (start_page, page_count, gen) = self.write_record(&bytes)?;

        if let Some(prev_pos) = self.cache_idx.get(namespace, &key_hash) {
            self.cache_dir[prev_pos].deleted = true;
            self.release_cache_pages(prev_pos);
            self.cache_idx.remove(namespace, &key_hash);
        }

        let dir_entry = CacheDirectoryEntry {
            namespace: namespace.to_string(),
            key_hash,
            start_page,
            page_count,
            len: bytes.len() as u32,
            deleted: false,
            accessed_at: now,
            access_count: 0,
            size_bytes,
            ttl_secs: opts.ttl_secs,
            created_at: now,
            vector: Some(vector),
            model: Some(opts.model),
            gen,
        };
        let pos = self.cache_dir.len();
        self.cache_dir.push(dir_entry);
        self.cache_idx.insert(namespace, key_hash, pos);

        self.evict_to_budget(namespace)?;

        self.dirty_cache = true;
        self.cache_batch.record_dirty();
        if self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        Ok(())
    }

    /// Semantic recall over the cache: score every live entry in
    /// `namespace` whose `model` matches against `query_vector` using
    /// cosine similarity; return the top-1 hit and its similarity iff
    /// `sim >= threshold`. `None` otherwise.
    ///
    /// Uses [`crate::result_cache::DEFAULT_SEMANTIC_THRESHOLD`] if you
    /// want a sensible default — pass it explicitly (`0.97` today).
    ///
    /// Exact-key entries (those without a vector) are transparently
    /// skipped, as are entries whose model doesn't match — a
    /// different-model hit would be a semantic bug, not a win.
    /// TTL-expired entries are also skipped.
    ///
    /// Access-stat updates on hit follow the same policy as
    /// [`Mnemo::cache_get`]: bumped on read-write handles, silently
    /// skipped on read-only. Hits and misses are recorded in the
    /// per-handle counters just like exact-key lookups.
    pub fn cache_get_semantic(
        &mut self,
        namespace: &str,
        query_vector: &[f32],
        threshold: f32,
        model: &str,
    ) -> Result<Option<(CachedValue, f32)>> {
        reject_reserved(namespace)?;
        if query_vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: query_vector.len(),
            });
        }
        // FIX-1 R-001.8 part 4: same Batched max_age check as
        // `cache_get`. See the comment there.
        if !self.read_only && self.cache_batch.should_auto_flush(&self.cache_flush_policy) {
            self.flush()?;
        }
        let now = memory::now_secs();

        // Scan namespace live entries, filter by model, compute
        // cosine, keep the best. Exact-key entries have `vector: None`
        // and are skipped by the filter.
        let mut best: Option<(usize, f32)> = None;
        for (i, e) in self.cache_dir.iter().enumerate() {
            if e.deleted || e.namespace != namespace || e.is_expired(now) {
                continue;
            }
            let Some(ref v) = e.vector else { continue };
            let Some(ref m) = e.model else { continue };
            if m != model {
                continue;
            }
            let sim = memory::cosine(query_vector, v);
            if best.map(|(_, s)| sim > s).unwrap_or(true) {
                best = Some((i, sim));
            }
        }

        let Some((pos, sim)) = best else {
            self.cache_counters.misses += 1;
            return Ok(None);
        };
        if sim < threshold {
            self.cache_counters.misses += 1;
            return Ok(None);
        }

        // Extract fields before any mutation to keep the borrow checker
        // happy.
        let (start_page, page_count, len, ttl_secs, created_at, gen) = {
            let e = &self.cache_dir[pos];
            (
                e.start_page,
                e.page_count,
                e.len,
                e.ttl_secs,
                e.created_at,
                e.gen,
            )
        };

        // Decrypt and decode the record body.
        let buf = self.read_record_run(start_page, page_count, len, gen)?;
        let entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;

        if !self.read_only {
            self.cache_dir[pos].accessed_at = now;
            self.cache_dir[pos].access_count = self.cache_dir[pos].access_count.saturating_add(1);
            self.dirty_cache = true;
        } else if let Some(delta) = self.access_delta.as_mut() {
            // 12.4: a shared read keeps the hit for the next commit.
            let e = &self.cache_dir[pos];
            delta.cache_hit(&e.namespace, e.key_hash, now);
        }
        self.cache_counters.hits += 1;
        let access_count = self.cache_dir[pos].access_count;
        // FIX-1 R-002.2 part 1: same read-only `accessed_at` fix as
        // `cache_get`. The directory carries the live value; the record
        // body only holds the insert-time snapshot.
        let accessed_at = if self.read_only {
            self.cache_dir[pos].accessed_at
        } else {
            now
        };
        Ok(Some((
            CachedValue {
                value: entry.value,
                content_type: entry.content_type,
                created_at,
                accessed_at,
                access_count,
                ttl_secs,
            },
            sim,
        )))
    }
}

impl Mnemo {
    /// EMBED-ID-1: the embedder identity in the reserved record, or `None`
    /// when the file has none yet. Touches no counter and writes nothing,
    /// so it's safe on a read-only handle.
    pub(super) fn recorded_identity(&mut self) -> Result<Option<embed::EmbedderIdentity>> {
        let key_hash = result_cache::hash_key(embed::IDENTITY_KEY);
        let Some(pos) = self.cache_idx.get(embed::IDENTITY_NAMESPACE, &key_hash) else {
            return Ok(None);
        };
        let (start_page, page_count, len, gen) = {
            let e = &self.cache_dir[pos];
            (e.start_page, e.page_count, e.len, e.gen)
        };
        let buf = self.read_record_run(start_page, page_count, len, gen)?;
        let entry: CacheEntry =
            rmp_serde::from_slice(&buf).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        embed::identity_from_json(&entry.value)
            .map(Some)
            .ok_or_else(|| {
                MnemoError::Serialize("the embedder identity record is malformed".into())
            })
    }

    /// EMBED-ID-1: write the reserved identity record (read-write handles
    /// only; the caller flushes).
    pub(super) fn record_identity(&mut self, identity: &embed::EmbedderIdentity) -> Result<()> {
        self.cache_put_inner(
            embed::IDENTITY_NAMESPACE,
            embed::IDENTITY_KEY,
            &embed::identity_to_json(identity),
            CachePutOpts {
                content_type: "json".into(),
                ttl_secs: None,
                cost_hint_ms: None,
            },
        )
    }
}

/// The public cache API refuses the engine's own namespaces.
fn reject_reserved(namespace: &str) -> Result<()> {
    if embed::is_reserved_namespace(namespace) {
        return Err(MnemoError::Invalid(format!(
            "cache namespace {namespace:?} is reserved (namespaces starting with {:?} belong to the engine)",
            embed::RESERVED_CACHE_PREFIX
        )));
    }
    Ok(())
}
