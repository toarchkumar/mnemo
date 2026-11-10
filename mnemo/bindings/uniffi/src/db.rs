//! `MnemoDb`, the one object the Swift and Kotlin packages wrap.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

use mnemo_engine::{KdfParams, Mnemo, RecallRequest, Ulid};
use zeroize::{Zeroize, Zeroizing};

use crate::embedder::{embed_checked, EngineEmbedder, MnemoEmbedder};
use crate::error::{MnemoError, POISONED};
use crate::types::*;

/// An open `.mnemo` file. Safe to share across threads: calls are
/// serialized on the handle's lock and block until done, so call them off
/// the main thread (the Swift and Kotlin layers add async wrappers).
#[derive(uniffi::Object)]
pub struct MnemoDb {
    pub(crate) inner: Mutex<Option<Mnemo>>,
    embedder: RwLock<Option<Arc<dyn MnemoEmbedder>>>,
    path: PathBuf,
    dimensions: usize,
}

/// A 32-byte key from the app, as bytes (never a string).
fn key_from(mut bytes: Vec<u8>) -> Result<Zeroizing<[u8; 32]>, MnemoError> {
    let mut key = Zeroizing::new([0u8; 32]);
    let ok = bytes.len() == 32;
    if ok {
        key.copy_from_slice(&bytes);
    }
    bytes.zeroize();
    if ok {
        Ok(key)
    } else {
        Err(MnemoError::Invalid {
            message: "a key must be 32 bytes".into(),
        })
    }
}

fn engine_config(c: &MnemoConfig, kdf: KdfParams) -> mnemo_engine::MnemoConfig {
    mnemo_engine::MnemoConfig {
        dimensions: c.dimensions as usize,
        kdf,
        format_version: c.format_version,
        ..Default::default()
    }
}

fn parse_id(id: &str) -> Result<Ulid, MnemoError> {
    Ulid::from_string(id).map_err(|e| MnemoError::Invalid {
        message: format!("not a memory id: {e}"),
    })
}

fn engine_filter(terms: &[FilterTerm]) -> Result<mnemo_engine::Filter, MnemoError> {
    let mut f = mnemo_engine::Filter::default();
    for t in terms {
        f = match t {
            FilterTerm::Exists { key } => f.metadata_exists(key.clone()),
            FilterTerm::Eq { key, value_json } => {
                f.metadata_eq(key.clone(), json_value(value_json)?)
            }
            FilterTerm::In { key, values_json } => f.metadata_in(
                key.clone(),
                values_json
                    .iter()
                    .map(|v| json_value(v))
                    .collect::<Result<_, _>>()?,
            ),
        };
    }
    Ok(f)
}

impl MnemoDb {
    fn wrap(db: Mnemo, path: PathBuf, tuning: Option<&MnemoConfig>) -> Arc<Self> {
        let mut db = db;
        let (budget, pages) = tuning
            .map_or((MOBILE_VECTOR_CACHE_BUDGET, MOBILE_CACHE_PAGES), |c| {
                (c.vector_cache_budget_bytes, c.cache_pages)
            });
        db.set_vector_cache_budget(budget as usize);
        db.set_cache_capacity(pages as usize);
        let dimensions = db.dimensions();
        Arc::new(MnemoDb {
            inner: Mutex::new(Some(db)),
            embedder: RwLock::new(None),
            path,
            dimensions,
        })
    }

    /// Run `f` on the open handle, under its lock. A poisoned lock (a panic
    /// while the handle was in use) is `Corrupt`, never a panic.
    fn with<T>(
        &self,
        f: impl FnOnce(&mut Mnemo) -> Result<T, mnemo_engine::MnemoError>,
    ) -> Result<T, MnemoError> {
        let mut guard = self.inner.lock().map_err(|_| MnemoError::Corrupt {
            message: POISONED.into(),
        })?;
        let db = guard.as_mut().ok_or(MnemoError::Closed)?;
        f(db).map_err(MnemoError::from)
    }

    /// The registered embedder, cloned out so the caller holds no lock
    /// while it runs.
    fn embedder(&self) -> Result<Arc<dyn MnemoEmbedder>, MnemoError> {
        self.embedder
            .read()
            .map_err(|_| MnemoError::Corrupt {
                message: POISONED.into(),
            })?
            .clone()
            .ok_or_else(|| MnemoError::Invalid {
                message: "no embedder set; call set_embedder first".into(),
            })
    }

    /// Embed one text with no lock held.
    /// `text`'s vector when an embedder is set (embedded with no lock
    /// held), `None` otherwise.
    fn embed_if_set(&self, text: &str) -> Result<Option<Vec<f32>>, MnemoError> {
        let set = self
            .embedder
            .read()
            .map_err(|_| MnemoError::Corrupt {
                message: POISONED.into(),
            })?
            .is_some();
        if set {
            self.embed_one(text).map(Some)
        } else {
            Ok(None)
        }
    }

    fn embed_one(&self, text: &str) -> Result<Vec<f32>, MnemoError> {
        let e = self.embedder()?;
        let mut v = embed_checked(&e, vec![text.to_string()], self.dimensions)?;
        Ok(v.pop().unwrap_or_default())
    }

    fn build_memory(
        &self,
        content: String,
        vector: Vec<f32>,
        opts: &RememberOptions,
    ) -> Result<mnemo_engine::Memory, MnemoError> {
        let mut m = mnemo_engine::Memory::new(
            content,
            opts.memory_type.unwrap_or(MemoryType::Semantic).into(),
            vector,
        );
        if let Some(i) = opts.importance {
            m = m.with_importance(i);
        }
        if let Some(a) = &opts.agent_id {
            m = m.with_agent(a.clone());
        }
        if let Some(t) = opts.ttl_secs {
            m.ttl_secs = Some(t);
        }
        if let Some(j) = &opts.metadata_json {
            for (k, v) in metadata_from_json(j)? {
                m.metadata.insert(k, v);
            }
        }
        Ok(m)
    }

    fn write(&self, m: mnemo_engine::Memory, key: Option<&str>) -> Result<WriteResult, MnemoError> {
        self.with(|db| match key {
            Some(k) => db
                .remember_keyed(m, k)
                .map(|(id, o)| (id.to_string(), o.into())),
            None => db
                .remember(m)
                .map(|id| (id.to_string(), WriteOutcome::Added)),
        })
        .map(|(id, outcome)| WriteResult { id, outcome })
    }

    fn recall_vector(
        &self,
        vector: Vec<f32>,
        opts: RecallOptions,
    ) -> Result<RecallPage, MnemoError> {
        let mut req = if vector.is_empty() {
            match &opts.keywords {
                Some(k) => RecallRequest::keywords_only(k.clone()),
                None => {
                    return Err(MnemoError::Invalid {
                        message: "recall needs a vector or keywords".into(),
                    })
                }
            }
        } else {
            let mut r = RecallRequest::new(vector);
            if let Some(k) = &opts.keywords {
                r = r.keywords(k.clone());
            }
            r
        };
        req = req
            .top_k(opts.top_k as usize)
            .track_access(opts.track_access);
        if let Some(w) = opts.keyword_weight {
            req = req.keyword_weight(w);
        }
        if let Some(t) = &opts.types {
            req = req.types(t.iter().map(|x| (*x).into()).collect());
        }
        if !opts.filter.is_empty() {
            req = req.filter(engine_filter(&opts.filter)?);
        }
        if let Some(b) = opts.max_bytes {
            req = req.max_bytes(b as usize);
        }
        let resp = self.with(|db| db.recall_budgeted(&req))?;
        Ok(RecallPage {
            hits: resp
                .hits
                .into_iter()
                .map(|h| RecallHit {
                    memory: h.memory.into(),
                    score: h.score,
                    similarity: h.similarity,
                })
                .collect(),
            truncated: resp.truncated,
        })
    }

    fn engine_turns(turns: Vec<ThreadTurn>) -> Vec<mnemo_engine::ThreadTurn> {
        turns
            .into_iter()
            .map(|t| mnemo_engine::ThreadTurn {
                turn_id: t.turn_id,
                role: t.role,
                content: t.content,
                timestamp: t.timestamp,
            })
            .collect()
    }

    fn report(r: mnemo_engine::IngestReport) -> IngestReport {
        IngestReport {
            added_records: r.added_records as u64,
            skipped_existing_turns: r.skipped_existing_turns as u64,
            truncated_turns: r.truncated_turns as u64,
            chunked_turns: r.chunked_turns as u64,
        }
    }
}

#[uniffi::export]
impl MnemoDb {
    // --- open and lifecycle ---------------------------------------------

    /// Create a new file protected by a passphrase (the secure KDF).
    #[uniffi::constructor]
    pub fn create(
        path: String,
        passphrase: String,
        config: MnemoConfig,
    ) -> Result<Arc<Self>, MnemoError> {
        // Wrapped first, so it's zeroized on every path, failures included.
        let passphrase = Zeroizing::new(passphrase);
        let db = Mnemo::create(
            &path,
            &passphrase,
            engine_config(&config, KdfParams::secure()),
        )?;
        Ok(Self::wrap(db, path.into(), Some(&config)))
    }

    /// Create a new file protected by a raw 32-byte key (from
    /// `generate_key`, kept in the Keychain or Keystore). Opening it costs
    /// microseconds, not a KDF.
    #[uniffi::constructor]
    pub fn create_with_key(
        path: String,
        key: Vec<u8>,
        config: MnemoConfig,
    ) -> Result<Arc<Self>, MnemoError> {
        let key = key_from(key)?;
        let db = Mnemo::create_with_key(&path, &key, engine_config(&config, KdfParams::raw_key()))?;
        Ok(Self::wrap(db, path.into(), Some(&config)))
    }

    /// Open a passphrase file, read-write (takes the file's exclusive lock).
    #[uniffi::constructor]
    pub fn open(path: String, passphrase: String) -> Result<Arc<Self>, MnemoError> {
        let passphrase = Zeroizing::new(passphrase);
        let db = Mnemo::open(&path, &passphrase)?;
        Ok(Self::wrap(db, path.into(), None))
    }

    /// Open a raw-key file, read-write.
    #[uniffi::constructor]
    pub fn open_with_key(path: String, key: Vec<u8>) -> Result<Arc<Self>, MnemoError> {
        let key = key_from(key)?;
        let db = Mnemo::open_with_key(&path, &key)?;
        Ok(Self::wrap(db, path.into(), None))
    }

    /// Open a passphrase file read-only (a shared lock; writes return
    /// `ReadOnly`).
    #[uniffi::constructor]
    pub fn open_read_only(path: String, passphrase: String) -> Result<Arc<Self>, MnemoError> {
        let passphrase = Zeroizing::new(passphrase);
        let db = Mnemo::open_read_only(&path, &passphrase)?;
        Ok(Self::wrap(db, path.into(), None))
    }

    /// Open a raw-key file read-only.
    #[uniffi::constructor]
    pub fn open_read_only_with_key(path: String, key: Vec<u8>) -> Result<Arc<Self>, MnemoError> {
        let key = key_from(key)?;
        let db = Mnemo::open_read_only_with_key(&path, &key)?;
        Ok(Self::wrap(db, path.into(), None))
    }

    /// Flush, then release the file and its lock. Later calls return
    /// `Closed`; closing twice is fine.
    pub fn close(&self) -> Result<(), MnemoError> {
        let mut guard = self.inner.lock().map_err(|_| MnemoError::Corrupt {
            message: POISONED.into(),
        })?;
        match guard.take() {
            Some(db) => db.close().map_err(MnemoError::from),
            None => Ok(()),
        }
    }

    /// Make every change so far durable (one atomic commit).
    pub fn flush(&self) -> Result<(), MnemoError> {
        self.with(|db| db.flush())
    }

    /// Live memory count.
    pub fn len(&self) -> Result<u64, MnemoError> {
        self.with(|db| Ok(db.len() as u64))
    }

    /// No live memories.
    pub fn is_empty(&self) -> Result<bool, MnemoError> {
        self.with(|db| Ok(db.is_empty()))
    }

    /// The file's embedding width.
    pub fn dimensions(&self) -> u32 {
        self.dimensions as u32
    }

    /// The file's path.
    pub fn path(&self) -> String {
        self.path.display().to_string()
    }

    // --- embedder -------------------------------------------------------

    /// Register the app's embedder. Its identity must match the file's
    /// width, and the manifest's embedder if one is recorded (a new file
    /// records it). `embed` is never called while the handle is locked.
    pub fn set_embedder(&self, embedder: Arc<dyn MnemoEmbedder>) -> Result<(), MnemoError> {
        let id = embedder.identity();
        let identity = mnemo_engine::EmbedderIdentity {
            name: id.name,
            dimensions: id.dimensions as usize,
            normalize: id.normalize,
        };
        let adapter = EngineEmbedder {
            inner: embedder.clone(),
            identity,
        };
        self.with(|db| db.set_embedder(adapter))?;
        *self.embedder.write().map_err(|_| MnemoError::Corrupt {
            message: POISONED.into(),
        })? = Some(embedder);
        Ok(())
    }

    // --- writes ---------------------------------------------------------

    /// Store a memory with its vector. A `key` in the options makes it a
    /// keyed write (supersede-or-no-op).
    pub fn remember(
        &self,
        content: String,
        vector: Vec<f32>,
        options: RememberOptions,
    ) -> Result<WriteResult, MnemoError> {
        let m = self.build_memory(content, vector, &options)?;
        self.write(m, options.key.as_deref())
    }

    /// Embed `content` with the registered embedder (no lock held), then
    /// store it.
    pub fn remember_text(
        &self,
        content: String,
        options: RememberOptions,
    ) -> Result<WriteResult, MnemoError> {
        let vector = self.embed_one(&content)?;
        let m = self.build_memory(content, vector, &options)?;
        self.write(m, options.key.as_deref())
    }

    /// Change a memory in place (same id). A content change needs a new
    /// vector: pass one in the patch, or leave it out and the registered
    /// embedder embeds the new content (no lock held).
    pub fn update(&self, id: String, patch: MemoryPatch) -> Result<WriteOutcome, MnemoError> {
        let id = parse_id(&id)?;
        let have_embedder = self.embedder.read().map(|e| e.is_some()).unwrap_or(false);
        let vector = match (&patch.content, patch.vector) {
            (Some(c), None) if have_embedder => Some(self.embed_one(c)?),
            (_, v) => v,
        };
        let p = mnemo_engine::MemoryPatch {
            content: patch.content,
            vector,
            importance: patch.importance,
            memory_type: patch.memory_type.map(Into::into),
            metadata: patch
                .metadata_json
                .as_deref()
                .map(metadata_from_json)
                .transpose()?,
            ttl_secs: if patch.clear_ttl {
                Some(None)
            } else {
                patch.ttl_secs.map(Some)
            },
        };
        self.with(|db| db.update(&id, p)).map(Into::into)
    }

    /// Delete a memory.
    pub fn delete(&self, id: String) -> Result<(), MnemoError> {
        let id = parse_id(&id)?;
        self.with(|db| db.delete(&id))
    }

    // --- reads ----------------------------------------------------------

    /// One memory by id.
    pub fn get(&self, id: String) -> Result<MemoryRecord, MnemoError> {
        let id = parse_id(&id)?;
        self.with(|db| db.get(&id)).map(Into::into)
    }

    /// Every version written under `key` (the default slot), newest first.
    pub fn history(&self, key: String) -> Result<Vec<MemoryRecord>, MnemoError> {
        self.with(|db| db.history(&key, None))
            .map(|v| v.into_iter().map(Into::into).collect())
    }

    /// Recall by vector (or keywords only, with an empty vector).
    pub fn recall(
        &self,
        vector: Vec<f32>,
        options: RecallOptions,
    ) -> Result<RecallPage, MnemoError> {
        self.recall_vector(vector, options)
    }

    /// Embed `query` with the registered embedder (no lock held), then
    /// recall.
    pub fn recall_text(
        &self,
        query: String,
        options: RecallOptions,
    ) -> Result<RecallPage, MnemoError> {
        let vector = self.embed_one(&query)?;
        self.recall_vector(vector, options)
    }

    /// The onboarding briefing: the manifest and onboarding memories.
    pub fn about(&self) -> Result<Vec<MemoryRecord>, MnemoError> {
        self.with(|db| db.about())
            .map(|v| v.into_iter().map(Into::into).collect())
    }

    /// The essence render for a scope, within `max_bytes`.
    pub fn essence(&self, scope: EssenceScope, max_bytes: u64) -> Result<String, MnemoError> {
        self.with(|db| db.essence(scope.into(), max_bytes as usize))
    }

    // --- essence writes -----------------------------------------------------

    /// Add an item to an essence section (at most 280 bytes; status
    /// `open`). Returns its id. With an embedder set, `text` is embedded
    /// before the handle is locked.
    pub fn essence_add(
        &self,
        scope: EssenceScope,
        section: EssenceSection,
        text: String,
        refs: Option<Vec<String>>,
    ) -> Result<String, MnemoError> {
        let vector = self.embed_if_set(&text)?;
        self.with(|db| match vector {
            Some(v) => db.essence_add_with_vector(scope.into(), section.into(), &text, refs, v),
            None => db.essence_add(scope.into(), section.into(), &text, refs),
        })
        .map(|id| id.to_string())
    }

    /// Append a timeline entry (at most 600 bytes, append-only). Returns its
    /// id. Embeds like `essence_add`.
    pub fn essence_append_timeline(
        &self,
        scope: EssenceScope,
        text: String,
        refs: Option<Vec<String>>,
    ) -> Result<String, MnemoError> {
        let vector = self.embed_if_set(&text)?;
        self.with(|db| match vector {
            Some(v) => db.essence_append_timeline_with_vector(scope.into(), &text, refs, v),
            None => db.essence_append_timeline(scope.into(), &text, refs),
        })
        .map(|id| id.to_string())
    }

    /// `essence_add` with the caller's vector, embedded before the call so
    /// no embedder runs under the lock. With `None` it's `essence_add`: the
    /// registered embedder, if any, or else a neutral vector (essence is
    /// read by section, not similarity). The Swift and Kotlin stores pass a
    /// vector whenever they have an embedder.
    pub fn essence_add_with_vector(
        &self,
        scope: EssenceScope,
        section: EssenceSection,
        text: String,
        refs: Option<Vec<String>>,
        vector: Option<Vec<f32>>,
    ) -> Result<String, MnemoError> {
        self.with(|db| match vector {
            Some(v) => db.essence_add_with_vector(scope.into(), section.into(), &text, refs, v),
            None => db.essence_add(scope.into(), section.into(), &text, refs),
        })
        .map(|id| id.to_string())
    }

    /// `essence_append_timeline` with the caller's vector, like
    /// [`MnemoDb::essence_add_with_vector`].
    pub fn essence_append_timeline_with_vector(
        &self,
        scope: EssenceScope,
        text: String,
        refs: Option<Vec<String>>,
        vector: Option<Vec<f32>>,
    ) -> Result<String, MnemoError> {
        self.with(|db| match vector {
            Some(v) => db.essence_append_timeline_with_vector(scope.into(), &text, refs, v),
            None => db.essence_append_timeline(scope.into(), &text, refs),
        })
        .map(|id| id.to_string())
    }

    /// Replace an item's text (and refs). Timeline entries can't be edited.
    /// `Unchanged` when nothing differs.
    pub fn essence_update(
        &self,
        id: String,
        text: String,
        refs: Option<Vec<String>>,
    ) -> Result<WriteOutcome, MnemoError> {
        let id = parse_id(&id)?;
        self.with(|db| db.essence_update(&id, &text, refs))
            .map(Into::into)
    }

    /// Mark an item resolved (it stays in the file and in history).
    pub fn essence_resolve(&self, id: String) -> Result<WriteOutcome, MnemoError> {
        let id = parse_id(&id)?;
        self.with(|db| db.essence_resolve(&id)).map(Into::into)
    }

    /// Remove an item from the render (a soft delete).
    pub fn essence_remove(&self, id: String) -> Result<(), MnemoError> {
        let id = parse_id(&id)?;
        self.with(|db| db.essence_remove(&id))
    }

    // --- the ANN index ------------------------------------------------------

    /// Build (or rebuild) the IVF+PQ index over every fact. On a v9 file
    /// below 20,000 facts (`V9_ANN_MIN_FACTS`) it builds nothing and
    /// reports `partitions: 0`: an exact scan is faster there, and it drops
    /// a stale index. v8 files build one at any size. The handle is locked
    /// for the build.
    pub fn build_index(&self) -> Result<IndexInfo, MnemoError> {
        self.with(|db| db.build_index()).map(Into::into)
    }

    /// Whether the file has an ANN index.
    pub fn has_index(&self) -> Result<bool, MnemoError> {
        self.with(|db| Ok(db.has_index()))
    }

    /// File statistics.
    pub fn stats(&self) -> Result<DbStats, MnemoError> {
        self.with(|db| db.stats()).map(|s| DbStats {
            memories: s.memories as u64,
            deleted: s.deleted as u64,
            dimensions: s.dimensions as u32,
            file_bytes: s.file_bytes,
            format_version: s.format_version,
            has_index: s.index.is_some(),
            wal_pages: s.wal_pages,
            recall_mode: s.recall_mode.into(),
        })
    }

    // --- threads ----------------------------------------------------------

    /// Archive a conversation's turns (re-ingesting a turn id is a no-op).
    /// The turns are embedded by the registered embedder with no lock held:
    /// the binding asks the engine which texts are new, embeds them, then
    /// applies them under the lock, and plans again if the thread changed
    /// in between.
    pub fn ingest_thread(
        &self,
        thread_id: String,
        turns: Vec<ThreadTurn>,
    ) -> Result<IngestReport, MnemoError> {
        let turns = Self::engine_turns(turns);
        for _ in 0..3 {
            let texts = self.with(|db| db.ingest_thread_texts(&thread_id, &turns))?;
            let vectors = if texts.is_empty() {
                Vec::new()
            } else {
                let e = self.embedder()?;
                embed_checked(&e, texts.clone(), self.dimensions)?
            };
            // Under one lock: check the plan is unchanged, then apply.
            let applied = self.with(|db| {
                if db.ingest_thread_texts(&thread_id, &turns)? != texts {
                    return Ok(None);
                }
                db.ingest_thread_with_vectors(&thread_id, &turns, &vectors)
                    .map(Some)
            })?;
            if let Some(r) = applied {
                return Ok(Self::report(r));
            }
        }
        Err(MnemoError::Invalid {
            message: "the thread kept changing during ingest; try again".into(),
        })
    }

    /// The first half of an ingest that the caller embeds itself (the
    /// Swift and Kotlin stores, which embed before taking their own lock):
    /// the texts to embed for these turns, in order. Empty when every turn
    /// is already stored.
    pub fn ingest_thread_plan(
        &self,
        thread_id: String,
        turns: Vec<ThreadTurn>,
    ) -> Result<Vec<String>, MnemoError> {
        let turns = Self::engine_turns(turns);
        self.with(|db| db.ingest_thread_texts(&thread_id, &turns))
    }

    /// The second half: `texts` is what [`MnemoDb::ingest_thread_plan`]
    /// returned and `vectors` one vector per text. Under one lock, the plan
    /// is checked and applied. `None` when the thread changed since the
    /// plan (plan and embed again); nothing is written then.
    pub fn ingest_thread_with_vectors(
        &self,
        thread_id: String,
        turns: Vec<ThreadTurn>,
        texts: Vec<String>,
        vectors: Vec<Vec<f32>>,
    ) -> Result<Option<IngestReport>, MnemoError> {
        if vectors.len() != texts.len() {
            return Err(MnemoError::Invalid {
                message: format!("{} vectors for {} texts", vectors.len(), texts.len()),
            });
        }
        if let Some(v) = vectors.iter().find(|v| v.len() != self.dimensions) {
            return Err(MnemoError::DimensionMismatch {
                expected: self.dimensions as u64,
                got: v.len() as u64,
            });
        }
        let turns = Self::engine_turns(turns);
        self.with(|db| {
            if db.ingest_thread_texts(&thread_id, &turns)? != texts {
                return Ok(None);
            }
            db.ingest_thread_with_vectors(&thread_id, &turns, &vectors)
                .map(Some)
        })
        .map(|r| r.map(Self::report))
    }

    /// The archived turns of a thread, in order (chunks joined).
    pub fn thread_export(&self, thread_id: String) -> Result<Vec<ExportedTurn>, MnemoError> {
        self.with(|db| db.thread_export_turns(&thread_id)).map(|v| {
            v.into_iter()
                .map(|t| ExportedTurn {
                    turn_id: t.turn_id,
                    role: t.role,
                    content: t.content,
                    timestamp: t.timestamp,
                    sha256: t.sha256,
                    original_length: t.original_length,
                })
                .collect()
        })
    }

    /// Delete a thread's archive; returns the records removed.
    pub fn thread_forget(&self, thread_id: String) -> Result<u64, MnemoError> {
        self.with(|db| db.thread_forget(&thread_id))
            .map(|n| n as u64)
    }

    // --- result cache -----------------------------------------------------

    /// Store `value` under `(namespace, key)`.
    pub fn cache_put(
        &self,
        namespace: String,
        key: String,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    ) -> Result<(), MnemoError> {
        let opts = mnemo_engine::CachePutOpts {
            ttl_secs,
            ..Default::default()
        };
        self.with(|db| db.cache_put(&namespace, &key, &value, opts))
    }

    /// The value under `(namespace, key)`, if present and not expired.
    pub fn cache_get(&self, namespace: String, key: String) -> Result<Option<Vec<u8>>, MnemoError> {
        self.with(|db| db.cache_get(&namespace, &key))
            .map(|v| v.map(|c| c.value))
    }

    // --- tuning -----------------------------------------------------------

    /// The in-memory vector column budget, in bytes.
    pub fn set_vector_cache_budget(&self, bytes: u64) -> Result<(), MnemoError> {
        self.with(|db| {
            db.set_vector_cache_budget(bytes as usize);
            Ok(())
        })
    }

    /// The page cache, in 8 KB pages.
    pub fn set_cache_capacity(&self, pages: u32) -> Result<(), MnemoError> {
        self.with(|db| {
            db.set_cache_capacity(pages as usize);
            Ok(())
        })
    }

    // --- migration --------------------------------------------------------

    /// Write a copy of this file at `dest` that opens with `passphrase`
    /// alone (the secure KDF), on any device: for moving a key-bound file
    /// to a new phone. The copy is written under a fresh data key (through
    /// the compaction path: live memories, the result cache and the index,
    /// no history), so it shares no key material with this file. The live
    /// file is flushed first and keeps its own key; `dest` must not exist.
    /// The handle is locked for the copy and the KDF.
    pub fn export_with_passphrase(
        &self,
        dest: String,
        passphrase: String,
    ) -> Result<(), MnemoError> {
        let passphrase = Zeroizing::new(passphrase);
        let dest = PathBuf::from(dest);
        self.with(|db| db.export_rekeyed(&dest, &passphrase, KdfParams::secure()))
            .map(|_| ())
    }
}

/// Run the app's embedder on `texts`, checking the shape like the binding's
/// own text calls: a throw is [`MnemoError::Embedder`], a wrong count is
/// `Embedder`, and a wrong width is `DimensionMismatch`. The Swift and
/// Kotlin stores call it before taking their lock, then pass the vectors
/// to the vector APIs, so the embedder may call back into the same store.
#[uniffi::export]
pub fn embed_texts(
    embedder: Arc<dyn MnemoEmbedder>,
    texts: Vec<String>,
    dimensions: u32,
) -> Result<Vec<Vec<f32>>, MnemoError> {
    embed_checked(&embedder, texts, dimensions as usize)
}

/// A fresh random 32-byte key for `create_with_key`. Store it in the
/// Keychain or Keystore: it can't be recovered from the file.
///
/// Zeroizing here is best effort: the key is made in a zeroized array and
/// copied once into the returned `Vec`, which UniFFI copies into the
/// foreign buffer and frees without zeroing (a lowering that wiped it would
/// need unsafe code). The app's copy is the one to look after.
#[uniffi::export]
pub fn generate_key() -> Vec<u8> {
    let key = Zeroizing::new(Mnemo::generate_key());
    key.to_vec()
}

/// Copy a passphrase file (from `export_with_passphrase`, on another
/// device) to `dest` under a fresh data key, protected by a new device key,
/// and return that key for the app to store in the Keychain or Keystore.
/// The copy shares no key material with `src`: the transfer passphrase
/// doesn't open it, even with `src`'s header. `src` is opened read-only and
/// left as it is; `dest` must not exist.
#[uniffi::export]
pub fn import_with_passphrase(
    src: String,
    passphrase: String,
    dest: String,
) -> Result<Vec<u8>, MnemoError> {
    let passphrase = Zeroizing::new(passphrase);
    let dest = PathBuf::from(dest);
    // Read-only: the source is left exactly as it is. The copy is written
    // under a fresh data key and the new device key, so neither the
    // transfer passphrase nor the source's data key opens it.
    let mut source = Mnemo::open_read_only(&src, &passphrase)?;
    let key = Zeroizing::new(Mnemo::generate_key());
    source.export_rekeyed_with_key(&dest, &key)?;
    drop(source);
    Ok(key.to_vec())
}

/// Compact a passphrase file in place (also upgrades a v8 file to v9). The
/// file must not be open.
#[uniffi::export]
pub fn compact_file(path: String, passphrase: String) -> Result<CompactReport, MnemoError> {
    let passphrase = Zeroizing::new(passphrase);
    Mnemo::compact_file(&path, &passphrase)
        .map(report)
        .map_err(MnemoError::from)
}

/// Compact a raw-key file in place. The file must not be open.
#[uniffi::export]
pub fn compact_file_with_key(path: String, key: Vec<u8>) -> Result<CompactReport, MnemoError> {
    let key = key_from(key)?;
    Mnemo::compact_file_with_key(&path, &key)
        .map(report)
        .map_err(MnemoError::from)
}

fn report(r: mnemo_engine::CompactReport) -> CompactReport {
    CompactReport {
        before: r.before as u64,
        after: r.after as u64,
        cache_before: r.cache_before as u64,
        cache_after: r.cache_after as u64,
    }
}
