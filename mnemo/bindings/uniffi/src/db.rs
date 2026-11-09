//! `MnemoDb`, the one object the Swift and Kotlin packages wrap.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use mnemo_engine::{KdfParams, Mnemo, RecallRequest, Ulid};
use zeroize::{Zeroize, Zeroizing};

use crate::embedder::{embed_checked, EngineEmbedder, MnemoEmbedder};
use crate::error::{MnemoError, POISONED};
use crate::types::*;

/// How the handle was opened, kept (zeroized on drop) so
/// [`MnemoDb::export_with_passphrase`] can reopen its copy.
pub(crate) enum Secret {
    Passphrase(Zeroizing<String>),
    Key(Zeroizing<[u8; 32]>),
}

/// An open `.mnemo` file. Safe to share across threads: calls are
/// serialized on the handle's lock and block until done, so call them off
/// the main thread (the Swift and Kotlin layers add async wrappers).
#[derive(uniffi::Object)]
pub struct MnemoDb {
    pub(crate) inner: Mutex<Option<Mnemo>>,
    embedder: RwLock<Option<Arc<dyn MnemoEmbedder>>>,
    secret: Secret,
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
    fn wrap(db: Mnemo, secret: Secret, path: PathBuf, tuning: Option<&MnemoConfig>) -> Arc<Self> {
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
            secret,
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

    /// Reopen `copy` with this handle's own secret (the copy is a byte copy
    /// under the same key).
    fn open_copy(&self, copy: &Path) -> Result<Mnemo, MnemoError> {
        match &self.secret {
            Secret::Passphrase(p) => Mnemo::open(copy, p),
            Secret::Key(k) => Mnemo::open_with_key(copy, k),
        }
        .map_err(MnemoError::from)
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
        let db = Mnemo::create(
            &path,
            &passphrase,
            engine_config(&config, KdfParams::secure()),
        )?;
        Ok(Self::wrap(
            db,
            Secret::Passphrase(Zeroizing::new(passphrase)),
            path.into(),
            Some(&config),
        ))
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
        Ok(Self::wrap(db, Secret::Key(key), path.into(), Some(&config)))
    }

    /// Open a passphrase file, read-write (takes the file's exclusive lock).
    #[uniffi::constructor]
    pub fn open(path: String, passphrase: String) -> Result<Arc<Self>, MnemoError> {
        let db = Mnemo::open(&path, &passphrase)?;
        Ok(Self::wrap(
            db,
            Secret::Passphrase(Zeroizing::new(passphrase)),
            path.into(),
            None,
        ))
    }

    /// Open a raw-key file, read-write.
    #[uniffi::constructor]
    pub fn open_with_key(path: String, key: Vec<u8>) -> Result<Arc<Self>, MnemoError> {
        let key = key_from(key)?;
        let db = Mnemo::open_with_key(&path, &key)?;
        Ok(Self::wrap(db, Secret::Key(key), path.into(), None))
    }

    /// Open a passphrase file read-only (a shared lock; writes return
    /// `ReadOnly`).
    #[uniffi::constructor]
    pub fn open_read_only(path: String, passphrase: String) -> Result<Arc<Self>, MnemoError> {
        let db = Mnemo::open_read_only(&path, &passphrase)?;
        Ok(Self::wrap(
            db,
            Secret::Passphrase(Zeroizing::new(passphrase)),
            path.into(),
            None,
        ))
    }

    /// Open a raw-key file read-only.
    #[uniffi::constructor]
    pub fn open_read_only_with_key(path: String, key: Vec<u8>) -> Result<Arc<Self>, MnemoError> {
        let key = key_from(key)?;
        let db = Mnemo::open_read_only_with_key(&path, &key)?;
        Ok(Self::wrap(db, Secret::Key(key), path.into(), None))
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
        let scope = match scope {
            EssenceScope::Project => mnemo_engine::EssenceScope::Project,
            EssenceScope::Thread { thread_id } => mnemo_engine::EssenceScope::Thread(thread_id),
        };
        self.with(|db| db.essence(scope, max_bytes as usize))
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
        let turns: Vec<mnemo_engine::ThreadTurn> = turns
            .into_iter()
            .map(|t| mnemo_engine::ThreadTurn {
                turn_id: t.turn_id,
                role: t.role,
                content: t.content,
                timestamp: t.timestamp,
            })
            .collect();
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
                return Ok(IngestReport {
                    added_records: r.added_records as u64,
                    skipped_existing_turns: r.skipped_existing_turns as u64,
                    truncated_turns: r.truncated_turns as u64,
                    chunked_turns: r.chunked_turns as u64,
                });
            }
        }
        Err(MnemoError::Invalid {
            message: "the thread kept changing during ingest; try again".into(),
        })
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
    /// to a new phone. The live file is flushed first and keeps its own
    /// key; `dest` must not exist.
    pub fn export_with_passphrase(
        &self,
        dest: String,
        passphrase: String,
    ) -> Result<(), MnemoError> {
        let passphrase = Zeroizing::new(passphrase);
        let dest = PathBuf::from(dest);
        if dest.exists() {
            return Err(MnemoError::Invalid {
                message: format!("{} already exists", dest.display()),
            });
        }
        let tmp = sibling(&dest, "export-tmp");
        let _ = std::fs::remove_file(&tmp);
        self.with(|db| db.export_encrypted(&tmp))?;
        let result = (|| {
            let mut copy = self.open_copy(&tmp)?;
            copy.rekey(&passphrase, KdfParams::secure())?;
            copy.close()?;
            std::fs::rename(&tmp, &dest).map_err(|e| MnemoError::Io {
                message: e.to_string(),
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }
}

/// `dest` with `.<suffix>` appended to its file name.
fn sibling(dest: &Path, suffix: &str) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{suffix}"));
    dest.with_file_name(name)
}

/// A fresh random 32-byte key for `create_with_key`. Store it in the
/// Keychain or Keystore: it can't be recovered from the file.
#[uniffi::export]
pub fn generate_key() -> Vec<u8> {
    let key = Zeroizing::new(Mnemo::generate_key());
    key.to_vec()
}

/// Copy a passphrase file (from `export_with_passphrase`, on another
/// device) to `dest`, re-protected by a new device key, and return that
/// key for the app to store in the Keychain or Keystore. `src` is left as
/// it is; `dest` must not exist.
#[uniffi::export]
pub fn import_with_passphrase(
    src: String,
    passphrase: String,
    dest: String,
) -> Result<Vec<u8>, MnemoError> {
    let passphrase = Zeroizing::new(passphrase);
    let dest = PathBuf::from(dest);
    if dest.exists() {
        return Err(MnemoError::Invalid {
            message: format!("{} already exists", dest.display()),
        });
    }
    let tmp = sibling(&dest, "import-tmp");
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        std::fs::copy(&src, &tmp).map_err(|e| MnemoError::Io {
            message: e.to_string(),
        })?;
        let mut db = Mnemo::open(&tmp, &passphrase)?;
        let key = Zeroizing::new(Mnemo::generate_key());
        db.rekey_with_key(&key)?;
        db.close()?;
        std::fs::rename(&tmp, &dest).map_err(|e| MnemoError::Io {
            message: e.to_string(),
        })?;
        Ok(key.to_vec())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
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
