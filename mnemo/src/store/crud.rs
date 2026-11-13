//! Public CRUD (remember, get, delete, embedder), and the handle caches (key slots, superseded set, thread layers).

use super::*;

/// R-004.2: where the thread layers and the onboarding briefing live,
/// built by the handle's one lazy scan (`ensure_key_caches`, together with
/// the 12.1 key caches) and then kept current by `put` and `delete`. It
/// lets `about`, `essence`, `ingest_thread`, `thread_export` and
/// thread-pinned recall read just their own records instead of decrypting
/// the whole file each time.
#[derive(Debug, Default)]
pub(super) struct LayerCache {
    /// Live essence item ids.
    pub(super) essence: std::collections::HashSet<u128>,
    /// Live ids tagged `metadata.area = "onboarding"` (superseded versions
    /// included; `about` drops those when it reads them).
    pub(super) onboarding: std::collections::HashSet<u128>,
    /// Per `_mnemo.thread`: its records and archive state.
    pub(super) threads: std::collections::HashMap<String, ThreadCache>,
    /// Thread → (the thread it continues, the link record's id).
    pub(super) links: std::collections::HashMap<String, (String, u128)>,
    /// FMT-9e: on v9, thread-turn records the lazy scan didn't decode,
    /// by their row's `thread_hash`. A thread's bucket is decoded into
    /// `threads` the first time that thread is used
    /// ([`Mnemo::warm_thread`]), so a cold handle never decodes (or
    /// decompresses) the turns of threads it doesn't touch.
    pub(super) cold_turns: std::collections::HashMap<u32, Vec<u128>>,
}

/// One thread's records (turns, essence items, link), the turn ids already
/// archived (with how many records each has: long turns are chunked), and
/// the highest `_mnemo.seq` seen.
#[derive(Debug, Default)]
pub(super) struct ThreadCache {
    pub(super) records: std::collections::HashSet<u128>,
    pub(super) turns: std::collections::HashMap<String, u32>,
    pub(super) max_seq: Option<u64>,
}

pub(super) fn meta_str<'a>(m: &'a Memory, key: &str) -> Option<&'a str> {
    m.metadata.get(key).and_then(|v| v.as_str())
}

pub(super) fn is_onboarding(m: &Memory) -> bool {
    meta_str(m, "area").is_some_and(|s| s.eq_ignore_ascii_case("onboarding"))
}

impl LayerCache {
    pub(super) fn add(&mut self, m: &Memory) {
        let id = m.id.0;
        let kind = meta_str(m, MNEMO_META_KIND);
        if kind == Some(MNEMO_META_KIND_ESSENCE) {
            self.essence.insert(id);
        }
        if is_onboarding(m) {
            self.onboarding.insert(id);
        }
        let Some(thread) = meta_str(m, MNEMO_META_THREAD) else {
            return;
        };
        let tc = self.threads.entry(thread.to_string()).or_default();
        tc.records.insert(id);
        if kind == Some(MNEMO_META_KIND_TURN) {
            if let Some(turn_id) = meta_str(m, MNEMO_META_TURN_ID) {
                *tc.turns.entry(turn_id.to_string()).or_insert(0) += 1;
            }
            if let Some(seq) = m.metadata.get(MNEMO_META_SEQ).and_then(|v| v.as_u64()) {
                tc.max_seq = Some(tc.max_seq.map_or(seq, |s| s.max(seq)));
            }
        }
        if kind == Some(MNEMO_META_KIND_THREAD_LINK) {
            if let Some(from) = meta_str(m, MNEMO_META_CONTINUES) {
                self.links
                    .entry(thread.to_string())
                    .or_insert_with(|| (from.to_string(), id));
            }
        }
    }

    /// Undo [`LayerCache::add`] for the stored version of a record that is
    /// being overwritten or deleted. A thread with no records left is
    /// dropped, so a forgotten thread's `_mnemo.seq` restarts at 0. Partial
    /// deletes keep the old maximum (sequence gaps are harmless).
    pub(super) fn remove(&mut self, m: &Memory) {
        let id = m.id.0;
        self.essence.remove(&id);
        self.onboarding.remove(&id);
        let Some(thread) = meta_str(m, MNEMO_META_THREAD) else {
            return;
        };
        if self.links.get(thread).is_some_and(|(_, link)| *link == id) {
            self.links.remove(thread);
        }
        let Some(tc) = self.threads.get_mut(thread) else {
            return;
        };
        tc.records.remove(&id);
        if meta_str(m, MNEMO_META_KIND) == Some(MNEMO_META_KIND_TURN) {
            if let Some(turn_id) = meta_str(m, MNEMO_META_TURN_ID) {
                if let Some(n) = tc.turns.get_mut(turn_id) {
                    *n -= 1;
                    if *n == 0 {
                        tc.turns.remove(turn_id);
                    }
                }
            }
        }
        if tc.records.is_empty() {
            self.threads.remove(thread);
        }
    }
}

impl Mnemo {
    /// Register a text embedder for this handle. Enables
    /// [`Mnemo::remember_text`] and the text-side `RecallRequest`
    /// convenience path. Does two checks before storing the embedder:
    ///
    /// 1. **Dimension check.** The embedder's `identity().dimensions`
    ///    must equal [`Mnemo::dimensions`] (the width this database
    ///    was created with). Mismatch returns
    ///    [`MnemoError::DimensionMismatch`].
    /// 2. **Identity check (EMBED-ID-1).** Every file records the
    ///    identity (name, dimensions, normalize) of the first embedder
    ///    attached to it, in a reserved, encrypted record that isn't a
    ///    memory: `len`, `count`, `stats`, recall, `about` and `get`
    ///    never see it, and compaction, the rekeyed exports and the
    ///    passphrase import carry it. After that, any other identity,
    ///    even at the same width (another model, or another revision of
    ///    the same one), returns [`MnemoError::EmbedderMismatch`]: a
    ///    store is bound to the model that first wrote to it, so a new
    ///    model needs a new store.
    ///
    /// **Files with a manifest** (`mnemo init`, `seed.json`) also carry
    /// the identity on the manifest at `metadata.embedder = { name,
    /// dimensions, normalize }` (the legacy flat `embedder_name` /
    /// `embedder_dimensions` / `embedder_normalize` keys are read as an
    /// alias). The first registration upserts it there too, as before.
    /// If the manifest and the record disagree, this returns
    /// [`MnemoError::Invalid`] naming both.
    ///
    /// **Files from before EMBED-ID-1** have no record: the first
    /// embedder attached after the upgrade is recorded. The check
    /// protects from then on; it can't tell which model made the older
    /// vectors.
    ///
    /// **Read-only handles.** `set_embedder` on an `open_read_only`
    /// handle performs both checks and adopts the embedder in memory;
    /// it never writes. Readers (CLI `--text`, agent-side `recall_text`)
    /// can therefore use the text path next to a writer that holds
    /// the exclusive lock.
    ///
    /// On every failure path the handle's embedder slot is left
    /// unchanged. On success the embedder is wrapped in an `Arc<dyn
    /// Embedder>` and consulted by every text-side call made through
    /// this handle. The slot is per-handle and not persisted — a
    /// fresh open must re-register (same identity, so the check
    /// round-trips cleanly).
    pub fn set_embedder<E: Embedder + 'static>(&mut self, embedder: E) -> Result<()> {
        let identity = embedder.identity();
        if identity.dimensions != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: identity.dimensions,
            });
        }

        let (onboarding, on_manifest, upsert_target, on_record) = self.embedder_records()?;

        if let (Some(manifest), Some(record)) = (&on_manifest, &on_record) {
            if manifest != record {
                return Err(embed::identity_conflict_err(manifest, record));
            }
        }
        if let Some(stored) = on_record.as_ref().or(on_manifest.as_ref()) {
            if *stored != identity {
                return Err(embed::identity_mismatch_err(stored, &identity));
            }
        }

        // Record what's missing. A read-only handle writes nothing, but
        // still adopts the embedder in memory.
        if !self.read_only {
            let mut wrote = false;
            if on_manifest.is_none() {
                if let Some(idx) = upsert_target {
                    let mut m = onboarding[idx].clone();
                    embed::write_identity_into_metadata(&mut m.metadata, &identity);
                    // Reuse the memory's existing id so this is an in-place
                    // update of the scaffold manifest, not a new entry.
                    self.put(m)?;
                    wrote = true;
                }
            }
            if on_record.is_none() {
                self.record_identity(&identity)?;
                wrote = true;
            }
            if wrote {
                self.flush()?;
            }
        }

        self.embedder = Some(std::sync::Arc::new(embedder));
        Ok(())
    }

    /// Where the file records an embedder identity (EMBED-ID-1): the
    /// onboarding memories, the manifest's identity if it has one, the first
    /// onboarding memory without one (the upsert target), and the reserved
    /// record's identity.
    #[allow(clippy::type_complexity)]
    fn embedder_records(
        &mut self,
    ) -> Result<(
        Vec<Memory>,
        Option<embed::EmbedderIdentity>,
        Option<usize>,
        Option<embed::EmbedderIdentity>,
    )> {
        let onboarding = self.about()?;
        let mut on_manifest: Option<embed::EmbedderIdentity> = None;
        let mut upsert_target: Option<usize> = None;
        for (i, m) in onboarding.iter().enumerate() {
            if let Some(saw) = embed::read_identity_from_metadata(&m.metadata) {
                on_manifest = Some(saw);
                break;
            }
            if upsert_target.is_none() {
                upsert_target = Some(i);
            }
        }
        let on_record = self.recorded_identity()?;
        Ok((onboarding, on_manifest, upsert_target, on_record))
    }

    /// Whether [`Mnemo::set_embedder`] with this identity would write to the
    /// file (to record the identity where it's missing). A shared handle
    /// attaches an already recorded embedder in a read window (RB-007.6).
    pub(super) fn embedder_would_record(
        &mut self,
        identity: &embed::EmbedderIdentity,
    ) -> Result<bool> {
        if identity.dimensions != self.dimensions {
            // `set_embedder` refuses it without writing.
            return Ok(false);
        }
        let (_, on_manifest, upsert_target, on_record) = self.embedder_records()?;
        Ok((on_manifest.is_none() && upsert_target.is_some()) || on_record.is_none())
    }

    /// The embedder identity this file records (EMBED-ID-1), or `None`
    /// before the first `set_embedder` on a read-write handle. Reads the
    /// reserved record without touching any counter.
    pub fn embedder_identity(&mut self) -> Result<Option<embed::EmbedderIdentity>> {
        self.recorded_identity()
    }

    /// Borrow the currently-registered embedder, if any. Returns
    /// `None` when no embedder has been set via
    /// [`Mnemo::set_embedder`]. Bindings use this to drive their own
    /// text-side convenience methods without duplicating the identity
    /// check.
    pub fn embedder(&self) -> Option<&dyn Embedder> {
        self.embedder.as_deref()
    }

    /// Shared handle to the registered embedder, cloned so the caller
    /// can drop the borrow on this `Mnemo` before invoking `embed()`.
    /// FIX-1 / PY-1 (R-001.11): the Python binding needs this so an
    /// embedder callback that re-enters `Mnemo` (e.g. a Python
    /// embedder that calls `db.dimensions()` or `db.cache_get(...)`)
    /// doesn't hit PyO3's "Already borrowed" — we clone the Arc under
    /// a short `&self` borrow, release that borrow, then invoke
    /// `embed()` with no `&mut Mnemo` held.
    pub fn embedder_arc(&self) -> Option<std::sync::Arc<dyn Embedder>> {
        self.embedder.clone()
    }

    /// Embed `content` with the registered embedder and `remember` the
    /// result. Returns the new memory's ULID. Convenience path for
    /// callers who have a string, want it stored, and don't need any
    /// of the per-memory options [`Memory`] exposes (agent, importance,
    /// metadata, ttl, scope) — build a [`Memory`] by hand and call
    /// [`Mnemo::remember`] for those.
    ///
    /// Fails with [`MnemoError::Invalid`] if no embedder has been
    /// registered on this handle (call [`Mnemo::set_embedder`] first);
    /// with [`MnemoError::DimensionMismatch`] if the embedder returned
    /// the wrong width for this database.
    pub fn remember_text(&mut self, content: &str, memory_type: MemoryType) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let embedder = self.embedder.as_ref().ok_or_else(|| {
            MnemoError::Invalid(
                "no embedder registered on this handle; call set_embedder first".into(),
            )
        })?;
        let mut vectors = embedder.embed(&[content])?;
        let vector = vectors.pop().ok_or_else(|| {
            MnemoError::Invalid("embedder returned no vector for the input text".into())
        })?;
        if vector.len() != self.dimensions {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions,
                got: vector.len(),
            });
        }
        let memory = Memory::new(content.to_string(), memory_type, vector);
        self.remember(memory)
    }

    /// Store a memory. Returns its ULID. If the memory's ID is nil a fresh
    /// one is assigned; an existing ID overwrites in place.
    ///
    /// Metadata keys starting with `_mnemo.` are the engine's (supersession,
    /// keys, threads, kinds): a new memory carrying one is refused with
    /// [`MnemoError::Invalid`]. An in-place overwrite may carry the stored
    /// record's own `_mnemo.*` keys through unchanged (the usual `get`,
    /// change a field, `remember`), but can't add, change or drop one.
    pub fn remember(&mut self, memory: Memory) -> Result<Ulid> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        self.check_caller_metadata(&memory)?;
        self.put(memory)
    }

    /// RB-001.24: the caller's metadata may not set engine state. For a
    /// memory with a nil or unknown id, any `_mnemo.*` key is refused; for
    /// an overwrite in place, the `_mnemo.*` keys must equal the stored
    /// record's.
    pub(super) fn check_caller_metadata(&mut self, memory: &Memory) -> Result<()> {
        let reserved: Vec<(&String, &Value)> = memory
            .metadata
            .iter()
            .filter(|(k, _)| k.starts_with(crate::memory::MNEMO_META_NS))
            .collect();
        if reserved.is_empty() {
            return Ok(());
        }
        let stored = if memory.id.is_nil() {
            None
        } else {
            match self.index.get(memory.id.0, &self.catalog) {
                Some(idx) if !self.catalog[idx].deleted => {
                    let entry = self.catalog[idx].clone();
                    Some(self.read_memory(&entry)?)
                }
                _ => None,
            }
        };
        if let Some(old) = stored {
            let old_reserved: Vec<(&String, &Value)> = old
                .metadata
                .iter()
                .filter(|(k, _)| k.starts_with(crate::memory::MNEMO_META_NS))
                .collect();
            if old_reserved == reserved {
                return Ok(());
            }
        }
        Err(reserved_key_error(reserved[0].0))
    }

    /// Fetch a memory by ID.
    pub fn get(&mut self, id: &Ulid) -> Result<Memory> {
        let idx = self
            .index
            .get(id.0, &self.catalog)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        let entry = self.catalog[idx].clone();
        if entry.deleted {
            return Err(MnemoError::NotFound(id.to_string()));
        }
        self.read_memory(&entry)
    }

    /// Soft-delete a memory (tombstoned; space reclaimed by `compact`).
    pub fn delete(&mut self, id: &Ulid) -> Result<()> {
        if self.read_only {
            return Err(MnemoError::ReadOnly);
        }
        let idx = self
            .index
            .get(id.0, &self.catalog)
            .ok_or_else(|| MnemoError::NotFound(id.to_string()))?;
        // R-004.2 / 12.1: with the handle caches built, read the record
        // being deleted (one decrypt) so the key slot, superseded set and
        // thread layers drop exactly what it contributed.
        let entry = self.catalog[idx].clone();
        if self.layers.is_some() && !entry.deleted {
            let old = self.read_memory(&entry)?;
            self.note_removed(&old)?;
        }
        if !self.catalog[idx].deleted {
            self.catalog[idx].deleted = true;
            self.dirty_catalog = true;
            self.touch(idx);
            // FMT-9f: drop its document from the stats in place.
            if let (Some(stats), Some(r)) = (self.bm25.stats.as_mut(), entry.v9) {
                if r.doc != 0 {
                    stats.remove(r.doc);
                }
            }
            if self.is_v9() {
                self.kill_record(idx, &entry);
                self.kill_vector(idx, &entry);
            } else {
                self.release_pages(entry.start_page, entry.page_count as u64);
            }
        }
        if let Some(ann) = &mut self.ann {
            if ann.contains(id.0) {
                ann.remove(id.0);
                self.dirty_index = true;
                self.index_persist.changes += 1;
            }
        }
        // FIX-1 R-001.8 part 2: we can't tell whether this delete
        // removed the last memory of its agent without a decrypt
        // (the catalog entry doesn't carry `agent_id` in format v8),
        // so just invalidate the lazy set. The next `stats()` call
        // rebuilds from live memories.
        self.agent_set = None;
        // Phase 12.1: delete of a keyed record removes it from the slot,
        // and delete of a superseded record frees a `history` entry. When
        // the caches are built, `note_removed` above did that exactly;
        // otherwise they're built from scratch on first use anyway.
        Ok(())
    }

    /// Keep the handle caches (12.1 key slots, superseded set, R-004.2
    /// layers) current after `put` writes `m`. No-op until they're built.
    /// FMT-9e: a cold thread is warmed first, so its layer entry is whole.
    pub(super) fn note_added(&mut self, m: &Memory) -> Result<()> {
        if let Some(thread) = meta_str(m, MNEMO_META_THREAD) {
            self.warm_thread(thread)?;
        }
        let id = m.id.0;
        if let (Some(keys), Some(sup)) = (self.key_index.as_mut(), self.superseded.as_mut()) {
            if m.metadata.contains_key(MNEMO_META_SUPERSEDED_BY) {
                sup.insert(id);
            } else if let Some(Value::String(k)) = m.metadata.get(MNEMO_META_KEY) {
                keys.insert(Self::key_slot_for(m, k), id);
            }
        }
        if let Some(layers) = self.layers.as_mut() {
            layers.add(m);
        }
        Ok(())
    }

    /// Undo [`Mnemo::note_added`] for the stored version of a record that's
    /// about to be overwritten or deleted.
    pub(super) fn note_removed(&mut self, m: &Memory) -> Result<()> {
        if let Some(thread) = meta_str(m, MNEMO_META_THREAD) {
            self.warm_thread(thread)?;
        }
        let id = m.id.0;
        if let (Some(keys), Some(sup)) = (self.key_index.as_mut(), self.superseded.as_mut()) {
            sup.remove(&id);
            if let Some(Value::String(k)) = m.metadata.get(MNEMO_META_KEY) {
                let slot = Self::key_slot_for(m, k);
                if keys.get(&slot) == Some(&id) {
                    keys.remove(&slot);
                }
            }
        }
        if let Some(layers) = self.layers.as_mut() {
            layers.remove(m);
        }
        Ok(())
    }

    /// FMT-9e: decode `thread`'s cold turn records (see
    /// [`LayerCache::cold_turns`]) into the layer cache. Every record in
    /// the thread's hash bucket is decoded and filed under its own thread,
    /// so a hash collision just warms both threads. A no-op when the
    /// caches aren't built or the bucket is already warm.
    pub(super) fn warm_thread(&mut self, thread: &str) -> Result<()> {
        self.warm_thread_decoded(thread).map(|_| ())
    }

    /// [`Mnemo::warm_thread`], returning the records it decoded (without
    /// vectors), so a caller about to read the thread decodes each once.
    pub(super) fn warm_thread_decoded(
        &mut self,
        thread: &str,
    ) -> Result<std::collections::HashMap<u128, Memory>> {
        let mut decoded = std::collections::HashMap::new();
        let h = fnv1a32(&[thread.as_bytes()]);
        let Some(ids) = self.layers.as_mut().and_then(|l| l.cold_turns.remove(&h)) else {
            return Ok(decoded);
        };
        for e in self.live_entries(ids) {
            let m = self.read_memory_meta(&e)?;
            if let Some(layers) = self.layers.as_mut() {
                layers.add(&m);
            }
            decoded.insert(e.id, m);
        }
        Ok(decoded)
    }

    /// Decode a record's memory without its vector or live access stats:
    /// what the handle caches need (metadata, scope, agent), without the
    /// v9 vector-column read [`Mnemo::read_memory`] adds.
    pub(super) fn read_memory_meta(&mut self, e: &CatalogEntry) -> Result<Memory> {
        let bytes = self.read_record(e)?;
        self.decode_memory(&bytes)
    }

    /// The live catalog entries for `ids` (ids that are gone are skipped).
    pub(super) fn live_entries(&self, ids: impl IntoIterator<Item = u128>) -> Vec<CatalogEntry> {
        ids.into_iter()
            .filter_map(|id| {
                self.index
                    .get(id, &self.catalog)
                    .map(|i| self.catalog[i].clone())
            })
            .filter(|e| !e.deleted)
            .collect()
    }

    /// Catalog entries of the essence items, from the handle caches.
    pub(super) fn essence_entries(&mut self) -> Result<Vec<CatalogEntry>> {
        self.ensure_key_caches()?;
        let ids: Vec<u128> = self
            .layers
            .as_ref()
            .map(|l| l.essence.iter().copied().collect())
            .unwrap_or_default();
        Ok(self.live_entries(ids))
    }

    /// Catalog entries of every record filed under `thread`, from the
    /// handle caches.
    pub(super) fn thread_entries(&mut self, thread: &str) -> Result<Vec<CatalogEntry>> {
        Ok(self.thread_entries_decoded(thread)?.0)
    }

    /// [`Mnemo::thread_entries`], plus the records that warming a cold
    /// thread just decoded (see [`Mnemo::warm_thread_decoded`]).
    pub(super) fn thread_entries_decoded(
        &mut self,
        thread: &str,
    ) -> Result<(Vec<CatalogEntry>, std::collections::HashMap<u128, Memory>)> {
        self.ensure_key_caches()?;
        let decoded = self.warm_thread_decoded(thread)?;
        let ids: Vec<u128> = self
            .layers
            .as_ref()
            .and_then(|l| l.threads.get(thread))
            .map(|tc| tc.records.iter().copied().collect())
            .unwrap_or_default();
        Ok((self.live_entries(ids), decoded))
    }

    // --- Phase 12.1: edit in place + keyed supersession -----------------

    /// Scope-aware key-slot identifier for `memory` and a caller key.
    ///
    /// * [`Scope::Private`] → owned by the memory's `agent_id`.
    /// * [`Scope::Shared`]  → one slot across the whole file.
    pub(super) fn key_slot_for(memory: &Memory, key: &str) -> crate::store::KeySlot {
        let scope_owner = match memory.scope {
            Scope::Private => Some(memory.agent_id.clone()),
            Scope::Shared => None,
        };
        crate::store::KeySlot {
            scope_owner,
            key: key.to_string(),
        }
    }

    /// The handle's one lazy scan: builds the 12.1 `key_index` +
    /// `superseded` caches and the R-004.2 layer cache (essence ids,
    /// onboarding ids, threads, links) from live records, and prunes
    /// non-fact ids from a loaded ANN index (files written before v0.10.0
    /// indexed every kind). Decrypts every live record once per handle;
    /// `put` and `delete` keep the result current after that.
    pub(super) fn ensure_key_caches(&mut self) -> Result<()> {
        if self.key_index.is_some() && self.superseded.is_some() && self.layers.is_some() {
            return Ok(());
        }
        let entries: Vec<CatalogEntry> = self
            .catalog
            .iter()
            .filter(|e| !e.deleted)
            .cloned()
            .collect();
        let mut keys: std::collections::HashMap<crate::store::KeySlot, u128> =
            std::collections::HashMap::new();
        let mut superseded: std::collections::HashSet<u128> = std::collections::HashSet::new();
        let mut layers = LayerCache::default();
        let mut non_facts: Vec<u128> = Vec::new();
        for e in &entries {
            // FMT-9e: a v9 turn row with no key slot and no supersession
            // contributes only to its thread's layer entry, so it's left
            // undecoded in the thread's cold bucket (most of a
            // thread-heavy file). Its row says it isn't a fact.
            if let Some(r) = e.v9 {
                if r.kind == ROW_KIND_TURN
                    && r.thread_hash != 0
                    && r.key_hash == 0
                    && r.flags & ROW_SUPERSEDED == 0
                {
                    layers
                        .cold_turns
                        .entry(r.thread_hash)
                        .or_default()
                        .push(e.id);
                    non_facts.push(e.id);
                    continue;
                }
            }
            let m = self.read_memory_meta(e)?;
            layers.add(&m);
            if !is_ann_kind(&m) {
                non_facts.push(e.id);
            }
            if m.metadata.get(MNEMO_META_SUPERSEDED_BY).is_some() {
                superseded.insert(e.id);
                continue;
            }
            if let Some(Value::String(k)) = m.metadata.get(MNEMO_META_KEY) {
                let slot = Self::key_slot_for(&m, k);
                // Catalog order is insertion order; a later live record
                // with the same slot would only exist if a prior build
                // of this code allowed it. Keep the later id (newer
                // ULID), in case of a tie.
                keys.entry(slot)
                    .and_modify(|cur| {
                        if *cur < e.id {
                            *cur = e.id;
                        }
                    })
                    .or_insert(e.id);
            }
        }
        self.key_index = Some(keys);
        self.superseded = Some(superseded);
        self.layers = Some(layers);
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
}

/// RB-001.24: the error for caller metadata that sets an engine key.
pub(super) fn reserved_key_error(key: &str) -> MnemoError {
    MnemoError::Invalid(format!(
        "metadata key \"{key}\" is reserved: keys starting with \"{}\" are set by the engine",
        crate::memory::MNEMO_META_NS
    ))
}
