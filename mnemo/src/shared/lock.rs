//! The per-call lock loop (12.4 design note, "The protocol" steps 2 to 4 and
//! 9, "Writer preference", "Long operations"): one deadline and a cancel per
//! call, jittered backoff, writer preference through the sidecar's intent
//! lock with a reader that holds it for one try only, and the identity check
//! once the main lock is held.
//!
//! Every lock here goes through [`LockedFile`], so it's unlocked explicitly
//! before its descriptor closes (FLAKE-1).

use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fs4::fs_std::FileExt;

use super::call::{Call, CancelToken};
use super::fscheck;
use super::sidecar::{self, Sidecar, SidecarState, INTENT};
use crate::error::{MnemoError, Result};
use crate::pager::LockedFile;
use crate::store::{is_file_at, is_lock_conflict};

/// Options for a shared-mode handle (12.4).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SharedOptions {
    /// How long one call may wait for the file's lock before it fails with
    /// [`MnemoError::Locked`]. One deadline covers both locks and the
    /// retries after a compaction. Default 10 s; hooks pass 2 s.
    pub deadline: Duration,
    /// Use shared mode on a network or synced filesystem anyway, where the
    /// lock may not reach every process that opens the file.
    ///
    /// The check refuses network filesystems (on Linux, anything but ext4,
    /// xfs, btrfs, tmpfs, f2fs, overlayfs and zfs; overlayfs assumes Linux
    /// 4.19 or later) and, on macOS, iCloud Drive and File Provider folders.
    /// On Windows it refuses UNC paths; a drive letter mapped to a network
    /// share may not be detected.
    pub allow_network_fs: bool,
    /// Cancels the handle's waiting calls from another thread.
    pub cancel: Option<CancelToken>,
}

impl Default for SharedOptions {
    fn default() -> Self {
        SharedOptions {
            deadline: Duration::from_secs(10),
            allow_network_fs: false,
            cancel: None,
        }
    }
}

/// Which lock a call takes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum LockKind {
    /// Reads.
    Shared,
    /// Writes.
    Exclusive,
}

/// The lock layer for one brain: where it is, its sidecar, and the notices
/// to give once.
#[derive(Debug)]
pub(crate) struct BrainLocks {
    /// The path as given (made absolute), which the locks open and the
    /// identity check compares.
    path: PathBuf,
    /// The path with symlinks resolved, where the sidecar directory lives.
    resolved: PathBuf,
    sidecar: Option<Arc<Sidecar>>,
    notices: Vec<String>,
}

/// The Windows refusal before Rust 1.85 (see `build.rs`).
pub(crate) const WINDOWS_OLD_RENAME: bool = cfg!(mnemo_windows_old_rename);

impl BrainLocks {
    /// Set up the lock layer for the brain at `path`. Refuses a Windows
    /// build before Rust 1.85, a missing file (never creates one), and a
    /// network or synced filesystem unless `allow_network_fs`.
    pub(crate) fn new(path: &Path, opts: &SharedOptions) -> Result<Self> {
        Self::with_sidecar(path, opts, true)
    }

    /// [`BrainLocks::new`] for a plain long operation (compaction, an
    /// import, a rekey): it uses the sidecar directory only if one is
    /// already there. A shared handle makes it when it opens, so without
    /// one there's no shared caller to queue against, and a brain nobody
    /// opens in shared mode gains no sibling.
    pub(crate) fn for_plain(path: &Path, opts: &SharedOptions) -> Result<Self> {
        Self::with_sidecar(path, opts, false)
    }

    fn with_sidecar(path: &Path, opts: &SharedOptions, create: bool) -> Result<Self> {
        if WINDOWS_OLD_RENAME {
            return Err(MnemoError::Unsupported(
                "shared mode on Windows needs a build with Rust 1.85 or later (this one was \
                 built with an older compiler, whose std::fs::rename can't replace an open \
                 file)"
                    .into(),
            ));
        }
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()?.join(path)
        };
        // Symlinks resolved, so two paths through a symlinked directory find
        // one sidecar. A missing file is an error here: shared mode never
        // creates the brain.
        let resolved = std::fs::canonicalize(&path)?;
        let mut notices = fscheck::check(&resolved, opts.allow_network_fs)?;
        let present = std::fs::symlink_metadata(sidecar::sidecar_path(&resolved)).is_ok();
        let sidecar = if !create && !present {
            None
        } else {
            match sidecar::open(&resolved) {
                SidecarState::Ready(s) => Some(Arc::new(s)),
                SidecarState::Unavailable(why) => {
                    notices.push(format!(
                        "not using the sidecar directory: {why}; each call takes the main \
                         lock alone, without writer preference or a lock-info note"
                    ));
                    None
                }
            }
        };
        Ok(BrainLocks {
            path,
            resolved,
            sidecar,
            notices,
        })
    }

    /// The warnings to show once: a sidecar that can't be used, a synced
    /// folder, an allowed network filesystem.
    pub(crate) fn notices(&self) -> &[String] {
        &self.notices
    }

    /// Whether the sidecar directory is in use.
    pub(crate) fn has_sidecar(&self) -> bool {
        self.sidecar.is_some()
    }

    /// The sidecar directory, when in use.
    pub(crate) fn sidecar_path(&self) -> Option<&Path> {
        self.sidecar.as_deref().map(Sidecar::path)
    }

    /// Take the main lock for one call, within the call's deadline, unless
    /// cancelled. Writers queue on the intent lock: a writer holds it
    /// exclusively while it waits for the main lock, so new readers queue
    /// behind it and busy readers can't starve it.
    ///
    /// A reader takes the intent lock shared and tries the main lock. When
    /// the main lock is free or held shared, it gets in (RB-006.4 rec. 1).
    /// When the main lock is held exclusively (a shared try fails only
    /// then), the reader **keeps** the intent lock shared while it waits
    /// for that writer (since step 5): the next writer can't take the intent
    /// lock and queues behind the waiting readers, which get in together
    /// when the commit ends. Writers and batches of readers alternate, so a
    /// writer committing back to back can't starve readers either.
    ///
    /// One [`Call`] covers every lock a shared handle's window takes, so a
    /// read that has to take the exclusive lock to heal the file still
    /// waits one deadline in all (RB-007.8 item 2).
    pub(crate) fn acquire(&mut self, kind: LockKind, call: &mut Call) -> Result<Held> {
        let mut replaced_once = false;
        'call: loop {
            call.check()?;
            let intent = match self.try_intent(kind, call)? {
                Intent::Held(f) => Some(f),
                Intent::NotUsed => None,
                Intent::Busy => {
                    self.wait(call)?;
                    continue;
                }
            };
            loop {
                #[cfg(any(test, feature = "failpoints"))]
                crate::__failpoints::count_shared_main_try();
                match self.try_main(kind)? {
                    Main::Got(file) => {
                        drop(intent);
                        return Ok(Held {
                            file: Some(file),
                            sidecar: self.sidecar.clone(),
                            announced: false,
                        });
                    }
                    // A compaction renamed a new file over the path between
                    // our open and our lock: open the new one, at once the
                    // first time, then with the backoff, so a path that
                    // keeps failing the check can't spin a core until the
                    // deadline. The backoff sleeps without the intent lock
                    // (RB-007.8 item 3): a reader holds it for one try only,
                    // and a writer queues for it again.
                    Main::Replaced => {
                        call.check()?;
                        if call.past_deadline() {
                            return Err(self.locked());
                        }
                        if replaced_once {
                            drop(intent);
                            self.wait(call)?;
                            continue 'call;
                        }
                        replaced_once = true;
                        continue;
                    }
                    Main::Busy => {}
                }
                if intent.is_none() {
                    break;
                }
                // A writer keeps the intent lock while it waits, so new
                // readers queue behind it. A reader keeps it too, shared,
                // while a writer holds the main lock (the only time its
                // shared try fails), so the next writer queues behind it
                // (step 5; before, readers let it go and starved).
                self.wait(call)?;
            }
            drop(intent);
            self.wait(call)?;
        }
    }

    /// One backoff; past the deadline, `Locked` with what holds the file.
    fn wait(&self, call: &mut Call) -> Result<()> {
        if call.back_off()? {
            Ok(())
        } else {
            Err(self.locked())
        }
    }

    fn locked(&self) -> MnemoError {
        MnemoError::Locked {
            path: self.path.clone(),
            holder: self.holder(),
        }
    }

    fn try_intent(&mut self, kind: LockKind, call: &mut Call) -> Result<Intent> {
        let Some(side) = &self.sidecar else {
            return Ok(Intent::NotUsed);
        };
        let f = match side.lock_file(INTENT) {
            Ok(f) => f,
            // The directory may have been removed (`git clean -fdx`) or
            // replaced since the handle opened it, and the handle's
            // descriptor still names the old one. Open it again by path,
            // with every check, once per call (RB-007.7).
            Err(_) if !call.sidecar_reopened => {
                call.sidecar_reopened = true;
                match sidecar::open(&self.resolved) {
                    SidecarState::Ready(s) => {
                        let s = Arc::new(s);
                        self.sidecar = Some(s.clone());
                        match s.lock_file(INTENT) {
                            Ok(f) => f,
                            Err(e) => {
                                self.notice(format!(
                                    "can't use {}: {e}; this call takes the main lock alone, \
                                     without writer preference",
                                    s.path().join(INTENT).display()
                                ));
                                return Ok(Intent::NotUsed);
                            }
                        }
                    }
                    SidecarState::Unavailable(why) => {
                        self.notice(format!(
                            "not using the sidecar directory: {why}; calls take the main \
                             lock alone, without writer preference or a lock-info note"
                        ));
                        return Ok(Intent::NotUsed);
                    }
                }
            }
            // A lock file that can't be opened safely (something else is at
            // the name) costs writer preference for this call, never the
            // call.
            Err(_) => return Ok(Intent::NotUsed),
        };
        match try_lock(&f, kind) {
            Ok(()) => Ok(Intent::Held(LockedFile::new(f))),
            Err(e) if is_lock_conflict(&e) => Ok(Intent::Busy),
            Err(e) => Err(MnemoError::Io(e)),
        }
    }

    /// Add `text` to the notices, once.
    fn notice(&mut self, text: String) {
        if !self.notices.contains(&text) {
            self.notices.push(text);
        }
    }

    fn try_main(&self, kind: LockKind) -> Result<Main> {
        let mut o = OpenOptions::new();
        o.read(true);
        if kind == LockKind::Exclusive {
            o.write(true);
        }
        // Never creates: a missing file is an error (`NotFound`).
        let f = match o.open(&self.path) {
            Ok(f) => f,
            // A file this process can only read (RB-007.8 item 5).
            Err(e) if kind == LockKind::Exclusive && is_permission_denied(&e) => {
                return Err(MnemoError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "{} isn't writable by this process ({e}), so a shared-mode write \
                         can't take its lock; reads still work",
                        self.path.display()
                    ),
                )))
            }
            Err(e) => return Err(MnemoError::Io(e)),
        };
        match try_lock(&f, kind) {
            Ok(()) => {
                let f = LockedFile::new(f);
                #[cfg(any(test, feature = "failpoints"))]
                if crate::__failpoints::take_shared_replaced() {
                    return Ok(Main::Replaced);
                }
                if is_file_at(&f, &self.path)? {
                    Ok(Main::Got(f))
                } else {
                    Ok(Main::Replaced)
                }
            }
            Err(e) if is_lock_conflict(&e) => Ok(Main::Busy),
            Err(e) => Err(MnemoError::Io(e)),
        }
    }

    /// The live holder's note, if there is one and its process still runs.
    fn holder(&self) -> Option<String> {
        let line = self.sidecar.as_ref()?.read_lock_info()?;
        let info = LockInfo::parse(&line)?;
        sidecar::pid_alive(info.pid).then(|| info.describe())
    }
}

enum Intent {
    Held(LockedFile),
    Busy,
    NotUsed,
}

enum Main {
    Got(LockedFile),
    Busy,
    Replaced,
}

/// Whether opening the brain for writing failed for lack of permission.
pub(crate) fn is_permission_denied(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::PermissionDenied
}

fn try_lock(f: &File, kind: LockKind) -> std::io::Result<()> {
    match kind {
        LockKind::Shared => FileExt::try_lock_shared(f),
        LockKind::Exclusive => FileExt::try_lock_exclusive(f),
    }
}

/// The main lock for one call. Dropping it removes this call's `lock-info`
/// note, if it wrote one, and then unlocks.
pub(crate) struct Held {
    file: Option<LockedFile>,
    sidecar: Option<Arc<Sidecar>>,
    announced: bool,
}

impl Held {
    /// The locked file.
    pub(crate) fn file(&self) -> &File {
        self.file.as_ref().expect("held until dropped")
    }

    /// Hand the locked file to the caller (a shared handle's pager), which
    /// then owns the lock. Drop this `Held` first, so its `lock-info` note
    /// goes before the caller's unlock.
    pub(crate) fn take_file(&mut self) -> LockedFile {
        self.file.take().expect("taken once")
    }

    /// Say what this exclusive call is doing, for callers that time out
    /// waiting: written after the lock is held, by temp name and rename,
    /// and removed before the unlock. A note that can't be written is
    /// skipped (it's a hint).
    pub(crate) fn announce(&mut self, op: &str) {
        if let Some(side) = &self.sidecar {
            let line = LockInfo::current(op).encode();
            if side.write_lock_info(&line).is_ok() {
                self.announced = true;
            }
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.announced {
            if let Some(side) = &self.sidecar {
                side.remove_lock_info();
            }
        }
        drop(self.file.take());
    }
}

/// The `lock-info` line: operation, pid, start time, tool, tab-separated.
struct LockInfo {
    op: String,
    pid: u32,
    since: u64,
    tool: String,
}

impl LockInfo {
    fn current(op: &str) -> Self {
        let tool = std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "mnemo".into());
        LockInfo {
            op: clean(op),
            pid: std::process::id(),
            since: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs()),
            tool: clean(&tool),
        }
    }

    fn encode(&self) -> String {
        format!("{}\t{}\t{}\t{}", self.op, self.pid, self.since, self.tool)
    }

    fn parse(line: &str) -> Option<Self> {
        let mut f = line.split('\t');
        Some(LockInfo {
            op: clean(f.next()?),
            pid: f.next()?.parse().ok()?,
            since: f.next()?.parse().ok()?,
            tool: clean(f.next()?),
        })
    }

    /// "compact since 12:01:05 UTC (pid 4242, mnemo)".
    fn describe(&self) -> String {
        let s = self.since % 86_400;
        format!(
            "{} since {:02}:{:02}:{:02} UTC (pid {}, {})",
            self.op,
            s / 3600,
            s % 3600 / 60,
            s % 60,
            self.pid,
            self.tool
        )
    }
}

/// Printable, short, no tabs or line breaks: the note is shown in error
/// messages, and another process wrote it.
fn clean(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(64).collect()
}
