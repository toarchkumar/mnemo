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
//!    first, the generation lease is reset (the first page the window seals
//!    takes a fresh one), the delta is applied, and the window commits
//!    before it lets go;
//! 4. unlock and close.

use std::collections::HashMap;
use std::sync::Arc;

use zeroize::Zeroizing;

use super::*;
use crate::shared::{BrainLocks, Call, CancelToken, LockKind, SharedOptions};

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
    /// The per-namespace budgets themselves, sorted: a changed value is a
    /// change, not only an added or removed namespace.
    cache_budgets: Vec<(String, usize, u64)>,
    cache_flush_policy: String,
}

impl FileSettings {
    fn of(db: &Mnemo) -> Self {
        FileSettings {
            max_snapshots: db.max_snapshots,
            snapshot_cap_explicit: db.snapshot_cap_explicit,
            turn_compress_after: db.turn_compress_after,
            cache_budgets: {
                let mut b: Vec<_> = db
                    .cache_budgets
                    .iter()
                    .map(|(ns, b)| (ns.clone(), b.max_entries, b.max_bytes))
                    .collect();
                b.sort();
                b
            },
            cache_flush_policy: format!("{:?}", db.cache_flush_policy),
        }
    }
}

const SETTINGS_KEPT: &str = "a shared-mode handle keeps the file's default settings in v0.14 \
     (snapshot retention, cache budgets, the cache flush policy, turn compression): every \
     process's commits would apply one handle's setting to the whole file";

fn settings_refused() -> MnemoError {
    MnemoError::Unsupported(format!("{SETTINGS_KEPT}. Nothing was changed"))
}

/// RB-007.5: the window's closure committed part of its work itself (a
/// `flush`, or a method that commits, such as `commit_deltas`) before `e`.
fn part_stored(e: impl std::fmt::Display) -> MnemoError {
    MnemoError::MaybeStored(format!(
        "{e}; an earlier commit in this window is in the file, so part of the window \
         was stored"
    ))
}

/// A read had to heal the file (a pending WAL, a torn page 0) and this
/// process can't open it for writing (RB-007.8 item 5).
const HEAL_NEEDS_WRITE: &str = "the file has a pending WAL or a torn header to recover, \
     and this process can't write to it";

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
///   window ends. If it fails before the commit point, nothing was stored,
///   for a closure that doesn't commit by itself. A closure that does (it
///   calls `flush`, or a method that commits, such as `commit_deltas` or
///   `restore_to`) and then fails gets [`MnemoError::MaybeStored`], saying
///   part of the window is in the file. If the window's own commit fails
///   after its commit point, the handle finishes the commit itself and
///   returns `Ok`, or returns [`MnemoError::MaybeStored`]. If the file was
///   renamed away during the commit, [`MnemoError::ReplacedDuringWrite`].
///   Keep to one engine call per window where you can.
/// - **Access statistics** a failed write window applied are dropped when
///   its commit fails; they're statistics, and the next hits count again.
/// - **Settings** that act on the whole file (snapshot retention, cache
///   budgets, the cache flush policy, turn compression) are refused in
///   v0.14 with [`MnemoError::Unsupported`], and so is `rekey` inside a
///   closure (use [`SharedMnemo::rekey`], which also updates the secret the
///   handle keeps). The page cache's size, the vector cache budget and the
///   embedder (however it was set) are kept across reloads.
/// - **A cancel** ([`SharedMnemo::set_cancel`]) reaches only a call that is
///   waiting for a lock; once a call holds the lock it runs to its end.
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
    /// What the last [`SharedMnemo::rekey`] couldn't scrub.
    rekey_cleanup: Option<String>,
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
            rekey_cleanup: None,
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
    ///
    /// A closure that commits part of its work itself and then fails, or
    /// then changes a setting, gets [`MnemoError::MaybeStored`]: the earlier
    /// commit is in the file.
    pub fn write<R>(&mut self, f: impl FnOnce(&mut Mnemo) -> Result<R>) -> Result<R> {
        self.window(LockKind::Exclusive, |s| s.run_write(f))
    }

    /// Attach an embedder (as [`Mnemo::set_embedder`]). It's kept across
    /// reloads. When the file already records this embedder's identity,
    /// this is a read window and writes nothing; otherwise the identity is
    /// recorded in a write window.
    pub fn set_embedder<E: Embedder + 'static>(&mut self, embedder: E) -> Result<()> {
        let identity = embedder.identity();
        let needs_write = self.read(move |db| {
            if db.embedder_would_record(&identity)? {
                return Ok(Some(embedder));
            }
            db.set_embedder(embedder)?;
            Ok(None)
        })?;
        if let Some(embedder) = needs_write {
            self.write(|db| db.set_embedder(embedder))?;
        }
        Ok(())
    }

    /// Change the passphrase (as [`Mnemo::rekey`], in a write window), and
    /// replace the secret the handle keeps once the new one is in force, so
    /// the handle can still follow a compaction or recover the file
    /// afterwards. An error leaves the old secret in force and kept, unless
    /// it says the file may open with either.
    pub fn rekey(&mut self, new_passphrase: &str, kdf: KdfParams) -> Result<()> {
        self.rekey_secret(new_passphrase.as_bytes(), kdf)
    }

    /// [`SharedMnemo::rekey`] to a raw 256-bit key (as
    /// [`Mnemo::rekey_with_key`]).
    pub fn rekey_with_key(&mut self, new_key: &[u8; crypto::KEY_LEN]) -> Result<()> {
        self.rekey_secret(new_key, KdfParams::raw_key())
    }

    /// After a successful [`SharedMnemo::rekey`], what couldn't be scrubbed
    /// of the old key wrap (see [`Mnemo::rekey_cleanup_error`]).
    pub fn rekey_cleanup_error(&self) -> Option<&str> {
        self.rekey_cleanup.as_deref()
    }

    fn rekey_secret(&mut self, secret: &[u8], kdf: KdfParams) -> Result<()> {
        // The rekey window applies no statistics, so its only commit is the
        // rekey's and its errors are the rekey's own; the next write applies
        // them.
        let held_back = std::mem::take(&mut self.delta);
        let mut in_force = None;
        let r = self.write(|db| {
            db.rekey_unchecked(secret, kdf)?;
            in_force = Some(db.rekey_cleanup.clone());
            Ok(())
        });
        self.delta = held_back;
        if let Some(cleanup) = in_force {
            self.secret = Zeroizing::new(secret.to_vec());
            self.rekey_cleanup = cleanup;
        }
        r
    }

    /// Give the handle a new cancel token for its later calls, or none
    /// (RB-007.1). A cancelled token stays cancelled, so after a cancel the
    /// handle needs a new one (or `None`) to wait for a lock again. A cancel
    /// reaches only a call that is waiting for a lock.
    pub fn set_cancel(&mut self, token: Option<CancelToken>) {
        self.opts.cancel = token;
    }

    /// The embedder attached to the handle, if any (shared, so a binding can
    /// embed outside a window and then call [`SharedMnemo::write`]).
    pub fn embedder_arc(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.clone()
    }

    /// The warnings to show once: a sidecar directory that can't be used, a
    /// synced folder, an allowed network filesystem.
    pub fn notices(&self) -> &[String] {
        self.locks.notices()
    }

    /// The handle's options: those it was opened with, and the cancel token
    /// [`SharedMnemo::set_cancel`] gave it last.
    pub fn options(&self) -> &SharedOptions {
        &self.opts
    }

    /// Close the handle. It holds no lock between calls, so this only drops
    /// what it kept: the secret (zeroized) and the access statistics no
    /// write has applied yet.
    pub fn close(self) -> Result<()> {
        Ok(())
    }

    /// **Not a stable API** (failpoints builds only). How many times the
    /// handle has rebuilt its state from the file.
    #[cfg(any(test, feature = "failpoints"))]
    #[doc(hidden)]
    pub fn __reloads(&self) -> u64 {
        self.reloads
    }

    /// **Not a stable API** (failpoints builds only). The page cache's
    /// occupancy and capacity.
    #[cfg(any(test, feature = "failpoints"))]
    #[doc(hidden)]
    pub fn __page_cache(&self) -> (usize, usize) {
        self.db.as_ref().map_or((0, 0), |d| d.pager.cache_stats())
    }

    /// **Not a stable API** (failpoints builds only). The handle's state as
    /// it stands, without a window (no lock, no catch-up): for tests that
    /// look at what a failed call left in memory. No file is attached, so
    /// only calls that stay in memory may be made; one that reads or
    /// writes the file panics.
    #[cfg(any(test, feature = "failpoints"))]
    #[doc(hidden)]
    pub fn __state(&mut self) -> Option<&mut Mnemo> {
        self.db.as_mut()
    }

    /// One window: lock, catch up, run, unlock. One [`Call`], so one
    /// deadline and one cancel token, covers every lock it takes.
    fn window<R>(&mut self, kind: LockKind, run: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let mut call = Call::new(self.opts.deadline, self.opts.cancel.as_ref());
        let mut lock = kind;
        let held = loop {
            let mut held = match self.locks.acquire(lock, &mut call) {
                Ok(h) => h,
                // A read that has to heal the file, which this process can
                // only read.
                Err(MnemoError::Io(e))
                    if lock != kind && e.kind() == std::io::ErrorKind::PermissionDenied =>
                {
                    return Err(MnemoError::NeedsWriteOpen {
                        reason: HEAL_NEEDS_WRITE,
                    })
                }
                Err(e) => return Err(e),
            };
            let file = held.take_file();
            match self.catch_up(lock, file)? {
                Next::Ready => break held,
                // A pending WAL or a torn page 0 under the shared lock:
                // heal it under the exclusive one, then carry on.
                Next::NeedExclusive => lock = LockKind::Exclusive,
            }
        };
        // The pager now holds the call's locked file. Release it however
        // `run` ends: a closure that panics, caught by the caller (UniFFI
        // catches panics at its boundary, and so do thread pools and
        // `catch_unwind`), must not leave the handle holding the lock. So
        // the unwind is caught here, the lock released, and the panic
        // resumed. A write window has cleared its stamp before running the
        // closure, so the handle's next call reloads.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self)));
        // The lock-info note (if any) goes before the unlock.
        drop(held);
        if let Some(db) = self.db.as_mut() {
            drop(db.pager.detach_file());
            // An embedder set inside a closure is kept across reloads too
            // (RB-007.8 item 4).
            self.embedder = db.embedder.clone();
        }
        match r {
            Ok(r) => r,
            Err(panic) => {
                // Whatever the closure left half done in memory, read or
                // write, is reloaded by the next call (RB-007.8 item 6).
                self.stamp = None;
                std::panic::resume_unwind(panic)
            }
        }
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

    /// The full open on the call's locked file (heal, replay), then install
    /// it.
    fn load(&mut self, file: LockedFile) -> Result<Next> {
        let new = self.reopen(file)?;
        self.install(new);
        Ok(Next::Ready)
    }

    /// Open the call's locked file as `open` does (heal a torn page 0,
    /// replay a pending WAL), with the data key the handle holds when the
    /// header it would recover to verifies under it (no KDF), and the kept
    /// secret otherwise (RB-007.2): a rekey by another process changes the
    /// secret but not the data key.
    fn reopen(&self, file: LockedFile) -> Result<Mnemo> {
        let held = self.db.as_ref().map(|d| Zeroizing::new(*d.pager.dek()));
        Mnemo::load_locked_with(
            self.path.clone(),
            file,
            &self.secret,
            held.as_deref(),
            false,
        )
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
        new.shared = true;
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
        db.window_committed = false;
        // No lease carries over from an earlier window. Every place that
        // seals a page takes the lease first when it's missing
        // (`ensure_gen_lease`), so every page this window seals carries a
        // generation no other writer has used, even when windows interleave
        // and one fails before its commit; and a window that seals nothing
        // commits nothing (RB-007.6).
        db.gen_leased = false;
        let delta = std::mem::take(&mut self.delta);
        db.apply_access_delta(&delta);
        let before = FileSettings::of(db);
        let r = f(db);
        let db = self.db.as_mut().expect("loaded by the window");
        let r = match r {
            Ok(r) => r,
            // A commit inside the closure finished (RB-007.5), or got past
            // its commit point, before the error: it may be in the file.
            Err(e) if db.window_committed => return Err(part_stored(e)),
            Err(e) if db.commit_point.is_some() => {
                return Err(MnemoError::MaybeStored(e.to_string()))
            }
            Err(e) => {
                self.delta = delta;
                return Err(e);
            }
        };
        if FileSettings::of(db) != before {
            if db.window_committed || db.commit_point.is_some() {
                return Err(part_stored(format!(
                    "{SETTINGS_KEPT}, so the setting wasn't applied"
                )));
            }
            self.delta = delta;
            return Err(settings_refused());
        }
        let committed_before = db.window_committed;
        if let Err(e) = db.flush() {
            match self.finish_failed_commit(e) {
                Ok(()) => {}
                Err(e @ MnemoError::MaybeStored(_)) => return Err(e),
                Err(e) if committed_before => return Err(part_stored(e)),
                Err(e) => return Err(e),
            }
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
        match self.reopen(file) {
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
