//! 12.4 shared mode's handle (v0.14.0 step 2b): [`SharedMnemo`], from
//! [`Mnemo::open_shared`]. The design is `docs/design/12.4-shared-access.md`,
//! "The protocol" steps 4 to 8, "Catching up" (the full reload) and "Writes
//! that only this handle makes".
//!
//! A shared handle holds no lock and no descriptor on the brain between
//! calls. Each call is a **window** on the file:
//! 1. take the lock through the lock layer (`crate::shared`): shared for
//!    [`SharedMnemo::read`], exclusive for [`SharedMnemo::write`];
//! 2. catch up: compare the header's seal (and, on Unix, the file's
//!    identity) with the stamp of the state the handle holds, and reload
//!    if they differ. A pending WAL or a torn page 0 takes the exclusive
//!    lock first, then replays or heals, then reloads;
//! 3. run the caller's closure on the handle: read-only for a read, with
//!    access statistics kept as a delta; for a write, the stamp is cleared
//!    first, a fresh generation lease is taken, the delta is applied, and
//!    the window commits before it lets go;
//! 4. unlock and close.

use std::collections::HashMap;
use std::sync::Arc;

use zeroize::Zeroizing;

use super::*;
use crate::shared::{BrainLocks, LockKind, SharedOptions};

/// Access statistics a shared handle saw while it held only the shared lock
/// (recall hits, cache hits): applied with the handle's next commit, and
/// dropped at close (12.4 design note, "Writes that only this handle
/// makes").
#[derive(Clone, Debug, Default)]
pub(crate) struct AccessDelta {
    /// Memory id → (hits, latest hit).
    memories: HashMap<u128, (u32, i64)>,
    /// (namespace, key hash) → (hits, latest hit).
    cache: HashMap<(String, [u8; 32]), (u32, i64)>,
}

impl AccessDelta {
    pub(crate) fn memory_hit(&mut self, id: u128, at: i64) {
        let e = self.memories.entry(id).or_insert((0, at));
        e.0 = e.0.saturating_add(1);
        e.1 = e.1.max(at);
    }

    pub(crate) fn cache_hit(&mut self, namespace: &str, key_hash: [u8; 32], at: i64) {
        let e = self
            .cache
            .entry((namespace.to_string(), key_hash))
            .or_insert((0, at));
        e.0 = e.0.saturating_add(1);
        e.1 = e.1.max(at);
    }
}

impl Mnemo {
    /// Open the brain at `path` in **shared mode** (12.4): several
    /// processes, sessions and tools can use one file at once. The handle
    /// takes the file's lock for each call and holds nothing between calls.
    ///
    /// The file must exist and be v9; shared mode never creates one. It's
    /// refused on a network or synced filesystem unless
    /// [`SharedOptions::allow_network_fs`], and on Windows builds made with
    /// Rust before 1.85 ([`MnemoError::Unsupported`]).
    ///
    /// The handle **keeps its secret** for its whole life, because every
    /// compaction gives the file a new data key: without the secret, a
    /// handle couldn't follow a `mnemo compact`. The passphrase is zeroized
    /// when the handle drops.
    ///
    /// Use [`SharedMnemo::read`] and [`SharedMnemo::write`] to call the
    /// handle's methods. Rust API only for now; the Python, CLI and MCP
    /// surfaces come later.
    pub fn open_shared(
        path: impl AsRef<Path>,
        passphrase: &str,
        opts: SharedOptions,
    ) -> Result<SharedMnemo> {
        SharedMnemo::open(path.as_ref(), passphrase.as_bytes(), opts)
    }

    /// [`Mnemo::open_shared`] for a raw-key file.
    pub fn open_shared_with_key(
        path: impl AsRef<Path>,
        key: &[u8; crypto::KEY_LEN],
        opts: SharedOptions,
    ) -> Result<SharedMnemo> {
        SharedMnemo::open(path.as_ref(), key, opts)
    }

    /// Apply a shared handle's statistics delta to the catalog and the cache
    /// directory, marking them dirty for this window's commit.
    fn apply_access_delta(&mut self, d: &AccessDelta) {
        for (id, (n, at)) in &d.memories {
            if let Some(idx) = self.index.get(*id, &self.catalog) {
                self.touch(idx);
                let e = &mut self.catalog[idx];
                e.access_count = e.access_count.saturating_add(*n);
                e.accessed_at = e.accessed_at.max(*at);
                self.dirty_catalog = true;
            }
        }
        for ((ns, key), (n, at)) in &d.cache {
            for e in self.cache_dir.iter_mut() {
                if !e.deleted && e.namespace == *ns && e.key_hash == *key {
                    e.access_count = e.access_count.saturating_add(*n);
                    e.accessed_at = e.accessed_at.max(*at);
                    self.dirty_cache = true;
                }
            }
        }
    }
}

/// The settings that decide what a commit does to the whole file. In v0.14
/// a shared handle keeps the defaults and refuses a change.
#[derive(PartialEq, Debug)]
struct FileSettings {
    max_snapshots: usize,
    snapshot_cap_explicit: bool,
    turn_compress_after: i64,
    cache_budgets: usize,
    cache_flush_policy: String,
}

impl FileSettings {
    fn of(db: &Mnemo) -> Self {
        FileSettings {
            max_snapshots: db.max_snapshots,
            snapshot_cap_explicit: db.snapshot_cap_explicit,
            turn_compress_after: db.turn_compress_after,
            cache_budgets: db.cache_budgets.len(),
            cache_flush_policy: format!("{:?}", db.cache_flush_policy),
        }
    }
}

fn settings_refused() -> MnemoError {
    MnemoError::Unsupported(
        "a shared-mode handle keeps the file's default settings in v0.14 (snapshot \
         retention, cache budgets, the cache flush policy, turn compression): every \
         process's commits would apply one handle's setting to the whole file. Nothing \
         was changed"
            .into(),
    )
}

/// What tells a shared handle that the file changed (12.4 protocol step 5):
/// the header's seal, which gets a fresh random nonce on every header
/// write, with the file's identity as the quick first test on Unix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    identity: Option<(u64, u64)>,
    seal: Option<([u8; crypto::NONCE_LEN], [u8; crypto::TAG_LEN])>,
}

impl Stamp {
    fn of(file: &File, header: &Header) -> Self {
        Stamp {
            identity: identity(file),
            seal: header.seal_nonce.zip(header.seal_tag),
        }
    }
}

#[cfg(unix)]
fn identity(file: &File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    file.metadata().ok().map(|m| (m.dev(), m.ino()))
}

/// Off Unix the seal alone tells (std has no stable file index there).
#[cfg(not(unix))]
fn identity(_file: &File) -> Option<(u64, u64)> {
    None
}

fn read_page0(file: &mut File) -> std::io::Result<[u8; PAGE_SIZE]> {
    let mut page = [0u8; PAGE_SIZE];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut page)?;
    Ok(page)
}

/// After catching up: go on, or take the exclusive lock first.
enum Next {
    Ready,
    NeedExclusive,
}

/// A brain opened in shared mode (12.4): see [`Mnemo::open_shared`].
///
/// Every call goes through [`SharedMnemo::read`] or [`SharedMnemo::write`],
/// which give the closure the handle for one window on the file:
///
/// ```no_run
/// # use mnemo_engine::{Memory, MemoryType, Mnemo, SharedOptions, RecallRequest};
/// # fn main() -> mnemo_engine::Result<()> {
/// let mut brain = Mnemo::open_shared("team.mnemo", "passphrase", SharedOptions::default())?;
/// let id = brain.write(|db| db.remember(Memory::new("we ship on Fridays", MemoryType::Semantic, vec![0.1, 0.2])))?;
/// let hits = brain.read(|db| db.recall(&RecallRequest::new(vec![0.1, 0.2]).top_k(5)))?;
/// # let _ = (id, hits); Ok(()) }
/// ```
///
/// - **A read** runs under the shared lock, on a read-only handle. A
///   recall's or a cache hit's access statistics are kept and applied with
///   this handle's next write; they're dropped at [`SharedMnemo::close`].
/// - **A write** runs under the exclusive lock and commits before the
///   window ends. If it fails before the commit point, nothing was stored.
///   If it fails after, the handle finishes the commit itself and returns
///   `Ok`, or returns [`MnemoError::MaybeStored`]. If the file was renamed
///   away during the commit, [`MnemoError::ReplacedDuringWrite`].
/// - **Settings** that act on the whole file (snapshot retention, cache
///   budgets, the cache flush policy, turn compression) are refused in
///   v0.14 with [`MnemoError::Unsupported`]. The page cache's size, the
///   vector cache budget and the embedder are kept across reloads.
pub struct SharedMnemo {
    locks: BrainLocks,
    opts: SharedOptions,
    path: PathBuf,
    secret: Zeroizing<Vec<u8>>,
    db: Option<Mnemo>,
    /// `None` means "reload at the next call" (12.4 protocol step 7).
    stamp: Option<Stamp>,
    delta: AccessDelta,
    embedder: Option<Arc<dyn Embedder>>,
    reloads: u64,
}

impl std::fmt::Debug for SharedMnemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedMnemo")
            .field("path", &self.path)
            .field("loaded", &self.db.is_some())
            .field("reloads", &self.reloads)
            .finish()
    }
}

impl SharedMnemo {
    fn open(path: &Path, secret: &[u8], opts: SharedOptions) -> Result<SharedMnemo> {
        let locks = BrainLocks::new(path, &opts)?;
        let mut s = SharedMnemo {
            locks,
            opts,
            path: absolute(path.to_path_buf()),
            secret: Zeroizing::new(secret.to_vec()),
            db: None,
            stamp: None,
            delta: AccessDelta::default(),
            embedder: None,
            reloads: 0,
        };
        // Load now, so a wrong secret or an old format fails at open.
        s.read(|_| Ok(()))?;
        Ok(s)
    }

    /// Run `f` on the handle under the file's shared lock, after catching up
    /// with any other process's commits. The handle is read-only inside;
    /// recall's default access tracking works and is applied with this
    /// handle's next write.
    pub fn read<R>(&mut self, f: impl FnOnce(&mut Mnemo) -> Result<R>) -> Result<R> {
        self.window(LockKind::Shared, |s| s.run_read(f))
    }

    /// Run `f` on the handle under the file's exclusive lock, after catching
    /// up, and commit before the lock is released. See [`SharedMnemo`] for
    /// what each failure means.
    pub fn write<R>(&mut self, f: impl FnOnce(&mut Mnemo) -> Result<R>) -> Result<R> {
        self.window(LockKind::Exclusive, |s| s.run_write(f))
    }

    /// Attach an embedder (as [`Mnemo::set_embedder`], in a write window,
    /// since a first embedder's identity is recorded in the file). It's kept
    /// across reloads.
    pub fn set_embedder<E: Embedder + 'static>(&mut self, embedder: E) -> Result<()> {
        self.write(|db| db.set_embedder(embedder))?;
        self.embedder = self.db.as_ref().and_then(|d| d.embedder.clone());
        Ok(())
    }

    /// The warnings to show once: a sidecar directory that can't be used, a
    /// synced folder, an allowed network filesystem.
    pub fn notices(&self) -> &[String] {
        self.locks.notices()
    }

    /// The options the handle was opened with.
    pub fn options(&self) -> &SharedOptions {
        &self.opts
    }

    /// Close the handle. It holds no lock between calls, so this only drops
    /// what it kept: the secret (zeroized) and the access statistics no
    /// write has applied yet.
    pub fn close(self) -> Result<()> {
        Ok(())
    }

    /// **Not a stable API.** How many times the handle has rebuilt its
    /// state from the file.
    #[doc(hidden)]
    pub fn __reloads(&self) -> u64 {
        self.reloads
    }

    /// **Not a stable API.** The page cache's occupancy and capacity.
    #[doc(hidden)]
    pub fn __page_cache(&self) -> (usize, usize) {
        self.db.as_ref().map_or((0, 0), |d| d.pager.cache_stats())
    }

    /// **Not a stable API.** The handle's state as it stands, without a
    /// window (no lock, no catch-up): for tests that look at what a failed
    /// call left in memory. Calls that reach the file fail here.
    #[doc(hidden)]
    pub fn __state(&mut self) -> Option<&mut Mnemo> {
        self.db.as_mut()
    }

    /// One window: lock, catch up, run, unlock.
    fn window<R>(&mut self, kind: LockKind, run: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let mut lock = kind;
        let held = loop {
            let mut held = self.locks.acquire(lock, &self.opts)?;
            let file = held.take_file();
            match self.catch_up(lock, file)? {
                Next::Ready => break held,
                // A pending WAL or a torn page 0 under the shared lock:
                // heal it under the exclusive one, then carry on.
                Next::NeedExclusive => lock = LockKind::Exclusive,
            }
        };
        let r = run(self);
        // The lock-info note (if any) goes before the unlock.
        drop(held);
        if let Some(db) = self.db.as_mut() {
            drop(db.pager.detach_file());
        }
        r
    }

    /// 12.4 protocol steps 5 and 6 (the identity check of step 4 ran in the
    /// lock layer): leave `self.db` current, holding `file`.
    fn catch_up(&mut self, lock: LockKind, mut file: LockedFile) -> Result<Next> {
        let header = match read_page0(&mut file)
            .map_err(MnemoError::Io)
            .and_then(|p| Header::from_page(&p))
        {
            Ok(h) => h,
            // A torn page 0: the scan that heals it reads the whole file and
            // runs the KDF per candidate, so never under the shared lock.
            Err(_) if lock == LockKind::Shared => return Ok(Next::NeedExclusive),
            Err(_) => return self.load(file),
        };
        let pending = wal::recover(
            &mut file,
            header.wal_start,
            header.wal_pages,
            header.wal_seq,
        )?
        .is_some();
        if pending {
            if lock == LockKind::Shared {
                return Ok(Next::NeedExclusive);
            }
            // Replay, then always reload: the replay moved page 0 past the
            // handle's state.
            return self.load(file);
        }
        let stamp = Stamp::of(&file, &header);
        if let (Some(db), Some(held)) = (self.db.as_mut(), self.stamp) {
            if held == stamp {
                db.pager.attach_file(file);
                return Ok(Next::Ready);
            }
        }
        // Another commit (the seal verifies under the key the handle
        // holds): rebuild from the file without the KDF.
        if let Some(old) = &self.db {
            if header.validate_seal(old.pager.dek()).is_ok() {
                let dek = Zeroizing::new(*old.pager.dek());
                let kdf = KdfParams {
                    m_cost: header.m_cost,
                    t_cost: header.t_cost,
                    p_cost: header.p_cost,
                };
                let new = Mnemo::from_locked(self.path.clone(), file, header, dek, kdf, false)?;
                self.install(new);
                return Ok(Next::Ready);
            }
        }
        // The seal doesn't verify under the held key (a compaction's new
        // data key, a file copied over this one) or nothing is loaded yet:
        // open it with the kept secret, which returns the specific error
        // (`WrongPassphrase`, `HeaderTampered`) if that fails too.
        self.load(file)
    }

    /// The full open on the call's locked file (heal, replay, KDF), then
    /// install it.
    fn load(&mut self, file: LockedFile) -> Result<Next> {
        let new = Mnemo::load_locked(self.path.clone(), file, &self.secret, false)?;
        self.install(new);
        Ok(Next::Ready)
    }

    /// Take `new` as the handle's state: carry over what is per-process (the
    /// page cache's size, the vector cache budget, the embedder), and stamp
    /// it.
    fn install(&mut self, mut new: Mnemo) {
        if let Some(old) = &self.db {
            new.pager.set_cache_capacity(old.pager.cache_stats().1);
            new.set_vector_cache_budget(old.vector_cache_budget);
        }
        if let Some(e) = &self.embedder {
            new.embedder = Some(e.clone());
        }
        self.stamp = Some(Stamp::of(new.pager.file(), &new.header));
        self.db = Some(new);
        self.reloads += 1;
    }

    fn run_read<R>(&mut self, f: impl FnOnce(&mut Mnemo) -> Result<R>) -> Result<R> {
        let db = self.db.as_mut().expect("loaded by the window");
        db.read_only = true;
        db.access_delta = Some(std::mem::take(&mut self.delta));
        let before = FileSettings::of(db);
        let r = f(db);
        db.read_only = false;
        self.delta = db.access_delta.take().unwrap_or_default();
        if FileSettings::of(db) != before {
            self.stamp = None;
            return Err(settings_refused());
        }
        r
    }

    fn run_write<R>(&mut self, f: impl FnOnce(&mut Mnemo) -> Result<R>) -> Result<R> {
        // Cleared before anything touches the handle's memory, set again
        // only from a successful commit: an error, or a panic, leaves the
        // next call to reload (protocol step 7).
        self.stamp = None;
        let db = self.db.as_mut().expect("loaded by the window");
        db.read_only = false;
        db.access_delta = None;
        db.commit_point = None;
        // A fresh generation lease for this window, committed before any
        // page is sealed: every page this window seals carries a generation
        // no other writer has used, even when windows interleave and one
        // fails before its commit.
        db.gen_leased = false;
        db.ensure_gen_lease()?;
        let delta = std::mem::take(&mut self.delta);
        db.apply_access_delta(&delta);
        let before = FileSettings::of(db);
        let r = f(db);
        let db = self.db.as_mut().expect("loaded by the window");
        let r = match r {
            Ok(r) => r,
            // A commit inside the closure got past its commit point before
            // the error: it may be in the file.
            Err(e) if db.commit_point.is_some() => {
                return Err(MnemoError::MaybeStored(e.to_string()))
            }
            Err(e) => {
                self.delta = delta;
                return Err(e);
            }
        };
        if FileSettings::of(db) != before {
            self.delta = delta;
            return Err(settings_refused());
        }
        if let Err(e) = db.flush() {
            self.finish_failed_commit(e)?;
        }
        #[cfg(any(test, feature = "failpoints"))]
        crate::__failpoints::run_after_shared_commit();
        // Protocol step 8: the path must still name the file just written.
        let db = self.db.as_ref().expect("loaded");
        if !is_file_at(db.pager.file(), &db.path)? {
            return Err(MnemoError::ReplacedDuringWrite {
                path: db.path.clone(),
            });
        }
        self.stamp = Some(Stamp::of(db.pager.file(), &db.header));
        Ok(r)
    }

    /// A commit failed (protocol step 7). Before the WAL's commit point,
    /// nothing was stored: return the error. After it, finish the commit
    /// from the WAL (the same replay an open does) and return `Ok` if the
    /// transaction is in the file, or [`MnemoError::MaybeStored`] if that
    /// can't be found out.
    fn finish_failed_commit(&mut self, e: MnemoError) -> Result<()> {
        let db = self.db.as_mut().expect("loaded");
        let Some(txn) = db.commit_point.take() else {
            return Err(e);
        };
        let Some(mut file) = db.pager.detach_file() else {
            return Err(MnemoError::MaybeStored(e.to_string()));
        };
        // Replay a pending transaction here, through the write failpoint,
        // so a test can fail this step too; `load_locked` then finds the
        // file current (or heals a torn page 0 itself).
        let replay = (|| -> Result<()> {
            let Ok(header) = read_page0(&mut file)
                .map_err(MnemoError::Io)
                .and_then(|p| Header::from_page(&p))
            else {
                return Ok(());
            };
            if let Some(frames) = wal::recover(
                &mut file,
                header.wal_start,
                header.wal_pages,
                header.wal_seq,
            )? {
                for (page_no, bytes) in &frames {
                    crate::pager::__failpoint_check()?;
                    file.seek(SeekFrom::Start(page_no * PAGE_SIZE as u64))?;
                    file.write_all(bytes)?;
                }
                file.sync_all()?;
                crate::bump_fsync();
            }
            Ok(())
        })();
        if let Err(e2) = replay {
            return Err(MnemoError::MaybeStored(format!(
                "{e}; finishing the commit failed too: {e2}"
            )));
        }
        match Mnemo::load_locked(self.path.clone(), file, &self.secret, false) {
            Ok(new) => {
                let stored = new.header.wal_seq >= txn;
                self.install(new);
                if stored {
                    Ok(())
                } else {
                    // The commit never became durable: nothing was stored,
                    // and the handle now holds the file as it is.
                    self.stamp = None;
                    Err(e)
                }
            }
            Err(e2) => Err(MnemoError::MaybeStored(format!(
                "{e}; reading the file back failed: {e2}"
            ))),
        }
    }
}
