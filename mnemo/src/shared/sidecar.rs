//! The sidecar directory next to a shared brain, `<brain>.mnemo-shared/`
//! (12.4 design note, "Sidecar files"): the writer-preference lock
//! (`intent.lock`) and the long-operation line (`lock-info`); step 7 adds the
//! presence locks.
//!
//! It sits where others may be able to create names, so it's never trusted
//! by name:
//! - it's created with `mkdir`, never "create if missing, else use";
//! - one that's already there is opened with `O_NOFOLLOW | O_DIRECTORY` and
//!   checked through its descriptor: a real directory, owned by this user,
//!   with no group or other access (and given 0700 if a umask left the
//!   owner without full access);
//! - files in it are opened relative to that descriptor (`openat`), with
//!   `O_NOFOLLOW`, and with `O_EXCL` when they're new;
//! - `lock-info` is written to a new temp name and renamed into place, never
//!   truncated in place.
//!
//! If the directory can't be created (the caller can't write there) or
//! fails a check, the handle doesn't use it and takes the main lock alone,
//! and says why once ([`SidecarState::Unavailable`]).
//!
//! On Windows the checks are what std offers safely: the directory must be
//! a real directory (not a symlink or junction), and files are opened
//! without following reparse points. There's no owner or mode check there.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// The sidecar directory's suffix, after the brain's full file name. One
/// `.gitignore` pattern matches it wherever the brain lives:
/// `*.mnemo-shared/`.
pub const SUFFIX: &str = ".mnemo-shared";

/// The writer-preference lock's file name.
pub const INTENT: &str = "intent.lock";

/// The long-operation line's file name.
pub const LOCK_INFO: &str = "lock-info";

/// LOCK-2's valve: the lock a writer that has waited past the threshold
/// holds, which readers try (shared, without waiting) before the intent
/// lock and back off from while it's held. Only writers ever hold it. Made
/// up front by a v0.15 shared handle's sidecar, or by the first writer that
/// needs it; never by a reader (12.4 design note, "LOCK-2").
pub const VALVE: &str = "writer.lock";

/// `<brain>.mnemo-shared` for an already resolved brain path.
pub(crate) fn sidecar_path(brain: &Path) -> PathBuf {
    let mut p = brain.as_os_str().to_os_string();
    p.push(SUFFIX);
    PathBuf::from(p)
}

/// Whether a brain can use its sidecar directory.
pub(crate) enum SidecarState {
    Ready(Sidecar),
    /// Why not, for the one notice the handle gives.
    Unavailable(String),
}

/// An opened, checked sidecar directory.
pub(crate) struct Sidecar {
    path: PathBuf,
    #[cfg(unix)]
    dir: rustix::fd::OwnedFd,
    /// LOCK-2: the valve file, kept open once a reader has found it, so a
    /// reader's check is a lock try and an unlock, not an `openat` too. It
    /// holds no lock between calls. A replaced directory gets a new
    /// `Sidecar`, and with it a new descriptor.
    valve: std::sync::OnceLock<File>,
}

impl std::fmt::Debug for Sidecar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sidecar").field("path", &self.path).finish()
    }
}

impl Sidecar {
    /// The directory's path.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Read `lock-info`, if there is one: at most 512 bytes, one line.
    pub(crate) fn read_lock_info(&self) -> Option<String> {
        let mut f = self.open_existing(LOCK_INFO, false).ok()?;
        let mut buf = vec![0u8; 512];
        let n = f.read(&mut buf).ok()?;
        buf.truncate(n);
        let text = String::from_utf8_lossy(&buf).into_owned();
        Some(text.lines().next().unwrap_or("").to_string())
    }

    /// Replace `lock-info` with `line`: written to a new exclusive temp name
    /// in the directory, then renamed into place.
    pub(crate) fn write_lock_info(&self, line: &str) -> std::io::Result<()> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let tmp = format!(
            "{LOCK_INFO}.{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let result = (|| {
            let mut f = self.create_new(&tmp)?;
            f.write_all(line.as_bytes())?;
            f.write_all(b"\n")?;
            drop(f);
            self.rename(&tmp, LOCK_INFO)
        })();
        if result.is_err() {
            let _ = self.remove(&tmp);
        }
        result
    }

    /// Remove `lock-info` (best effort: a hint, not a lock).
    pub(crate) fn remove_lock_info(&self) {
        let _ = self.remove(LOCK_INFO);
    }

    /// LOCK-2: the valve file for a reader's check, opened read-only and kept
    /// once it exists. `None` while it doesn't (a sidecar made before v0.15,
    /// until a writer needs it): no valve, the reader goes on.
    pub(crate) fn valve_for_reading(&self) -> Option<&File> {
        if let Some(f) = self.valve.get() {
            return Some(f);
        }
        let f = self.open_existing(VALVE, false).ok()?;
        Some(self.valve.get_or_init(|| f))
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use rustix::fs::{self as rfs, AtFlags, FileType, Mode, OFlags};
    use rustix::io::Errno;

    /// Create (or find) and check the sidecar directory for `brain`, which
    /// is already resolved (symlinks followed).
    pub(crate) fn open(brain: &Path) -> SidecarState {
        let path = sidecar_path(brain);
        match rfs::mkdir(&path, Mode::RWXU) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(Errno::ACCESS | Errno::PERM | Errno::ROFS) => {
                return SidecarState::Unavailable(format!(
                    "can't create {} (the directory isn't writable)",
                    path.display()
                ))
            }
            Err(e) => {
                return SidecarState::Unavailable(format!("can't create {}: {e}", path.display()))
            }
        }
        let dir = match rfs::open(
            &path,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(e) => {
                return SidecarState::Unavailable(format!(
                    "{} isn't a real directory ({e})",
                    path.display()
                ))
            }
        };
        let st = match rfs::fstat(&dir) {
            Ok(st) => st,
            Err(e) => return SidecarState::Unavailable(format!("{}: {e}", path.display())),
        };
        if FileType::from_raw_mode(st.st_mode) != FileType::Directory {
            return SidecarState::Unavailable(format!("{} isn't a directory", path.display()));
        }
        if st.st_uid != rustix::process::geteuid().as_raw() {
            return SidecarState::Unavailable(format!(
                "{} belongs to another user",
                path.display()
            ));
        }
        if st.st_mode & 0o077 != 0 {
            return SidecarState::Unavailable(format!(
                "{} is open to other users (mode {:o}; it must be 0700)",
                path.display(),
                st.st_mode & 0o777
            ));
        }
        // A umask that clears owner bits (0277, say) makes a directory
        // nothing can be created in, and every call would then skip the
        // intent lock without a word. It's this user's own directory with
        // no access for others, so give it 0700.
        if st.st_mode & 0o700 != 0o700 {
            if let Err(e) = rfs::fchmod(&dir, Mode::RWXU) {
                return SidecarState::Unavailable(format!(
                    "{} has mode {:o} and can't be given 0700: {e}",
                    path.display(),
                    st.st_mode & 0o777
                ));
            }
        }
        SidecarState::Ready(Sidecar {
            path,
            dir,
            valve: std::sync::OnceLock::new(),
        })
    }

    fn check_regular(f: &File) -> std::io::Result<()> {
        let st = rfs::fstat(f)?;
        if FileType::from_raw_mode(st.st_mode) != FileType::RegularFile
            || st.st_uid != rustix::process::geteuid().as_raw()
        {
            return Err(std::io::Error::other("not a regular file of this user's"));
        }
        Ok(())
    }

    impl Sidecar {
        /// Open (creating if new) one of the directory's lock files.
        pub(crate) fn lock_file(&self, name: &str) -> std::io::Result<File> {
            let base = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
            match rfs::openat(
                &self.dir,
                name,
                base | OFlags::CREATE | OFlags::EXCL,
                Mode::RUSR | Mode::WUSR,
            ) {
                Ok(fd) => Ok(File::from(fd)),
                Err(Errno::EXIST) => self.open_existing(name, true),
                Err(e) => Err(e.into()),
            }
        }

        pub(super) fn open_existing(&self, name: &str, write: bool) -> std::io::Result<File> {
            let mode = if write { OFlags::RDWR } else { OFlags::RDONLY };
            let fd = rfs::openat(
                &self.dir,
                name,
                mode | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
                Mode::empty(),
            )?;
            let f = File::from(fd);
            check_regular(&f)?;
            Ok(f)
        }

        pub(super) fn create_new(&self, name: &str) -> std::io::Result<File> {
            let fd = rfs::openat(
                &self.dir,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
            )?;
            Ok(File::from(fd))
        }

        pub(super) fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
            Ok(rfs::renameat(&self.dir, from, &self.dir, to)?)
        }

        pub(super) fn remove(&self, name: &str) -> std::io::Result<()> {
            Ok(rfs::unlinkat(&self.dir, name, AtFlags::empty())?)
        }
    }

    /// Whether `pid` names a running process (signal 0). A process of
    /// another user counts as running.
    pub(crate) fn pid_alive(pid: u32) -> bool {
        let Some(pid) = i32::try_from(pid)
            .ok()
            .and_then(rustix::process::Pid::from_raw)
        else {
            return false;
        };
        !matches!(rustix::process::test_kill_process(pid), Err(Errno::SRCH))
    }
}

#[cfg(not(unix))]
mod imp {
    use super::*;
    use std::fs::OpenOptions;

    /// Don't follow a reparse point (symlink or junction) when opening.
    #[cfg(windows)]
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

    fn no_follow(o: &mut OpenOptions) -> &mut OpenOptions {
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            o.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        o
    }

    pub(crate) fn open(brain: &Path) -> SidecarState {
        let path = sidecar_path(brain);
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                return SidecarState::Unavailable(format!(
                    "can't create {} (the directory isn't writable)",
                    path.display()
                ))
            }
            Err(e) => {
                return SidecarState::Unavailable(format!("can't create {}: {e}", path.display()))
            }
        }
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => SidecarState::Ready(Sidecar {
                path,
                valve: std::sync::OnceLock::new(),
            }),
            _ => SidecarState::Unavailable(format!("{} isn't a real directory", path.display())),
        }
    }

    fn check_regular(f: &File) -> std::io::Result<()> {
        let m = f.metadata()?;
        if !m.is_file() || m.file_type().is_symlink() {
            return Err(std::io::Error::other("not a regular file"));
        }
        Ok(())
    }

    impl Sidecar {
        pub(crate) fn lock_file(&self, name: &str) -> std::io::Result<File> {
            let p = self.path.join(name);
            match no_follow(OpenOptions::new().read(true).write(true).create_new(true)).open(&p) {
                Ok(f) => Ok(f),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    self.open_existing(name, true)
                }
                Err(e) => Err(e),
            }
        }

        pub(super) fn open_existing(&self, name: &str, write: bool) -> std::io::Result<File> {
            let f =
                no_follow(OpenOptions::new().read(true).write(write)).open(self.path.join(name))?;
            check_regular(&f)?;
            Ok(f)
        }

        pub(super) fn create_new(&self, name: &str) -> std::io::Result<File> {
            no_follow(OpenOptions::new().write(true).create_new(true)).open(self.path.join(name))
        }

        pub(super) fn rename(&self, from: &str, to: &str) -> std::io::Result<()> {
            std::fs::rename(self.path.join(from), self.path.join(to))
        }

        pub(super) fn remove(&self, name: &str) -> std::io::Result<()> {
            std::fs::remove_file(self.path.join(name))
        }
    }

    /// No safe liveness check off Unix: a note is shown as it is.
    pub(crate) fn pid_alive(_pid: u32) -> bool {
        true
    }
}

pub(crate) use imp::{open, pid_alive};
