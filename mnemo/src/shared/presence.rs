//! 13a (13.3, 12.4's step 7): presence locks, the free half of presence.
//!
//! A long-lived process (the MCP server) that declares an agent holds a
//! shared lock on `presence.<hash of agent>.lock` in the brain's sidecar
//! directory for its life. "Is X alive?" is an exclusive lock try on X's
//! file, without waiting: refused means X's process runs; acquired means it
//! is gone (the kernel dropped its lock, even after `kill -9`). Nothing is
//! written to the brain, and the brain's own lock is never touched.
//!
//! The step 7 rework (12.4 design note, "Claims and presence"):
//! - **the holder re-checks its file by identity on every call**
//!   ([`PresenceLock::refresh`]): if the name no longer names the file it
//!   locked (a deleted or replaced sidecar directory), it takes it again;
//! - **the probe retries once before it says "alive"**, since two probes at
//!   once refuse each other for an instant;
//! - **an acquired lock means "not alive" only if the name still names the
//!   file the probe locked;** otherwise, and for a missing file or
//!   directory, the answer is "unknown", never "not alive";
//! - every lock goes through `LockedFile` (FLAKE-1).
//!
//! The result is only a hint: a claim lapses early on it only when its
//! holder registered a lock when it claimed, and was seen not alive twice,
//! some seconds apart (`store/coordination.rs`).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};

use super::sidecar::{self, Sidecar, SidecarState};
use crate::pager::LockedFile;

/// The presence lock's file name for `agent`: a hash, so the name is the
/// same length and alphabet whatever the id.
pub(crate) fn file_name(agent: &str) -> String {
    let h = Sha256::digest(agent.as_bytes());
    let hex: String = h[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("presence.{hex}.lock")
}

#[cfg(unix)]
fn identity(f: &File) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    f.metadata().ok().map(|m| (m.dev(), m.ino()))
}

/// Off Unix std has no stable file index: a name is trusted to be its file.
#[cfg(not(unix))]
fn identity(_f: &File) -> Option<(u64, u64)> {
    Some((0, 0))
}

/// A held presence lock.
pub(crate) struct PresenceLock {
    brain: PathBuf,
    name: String,
    side: Sidecar,
    file: Option<LockedFile>,
    id: Option<(u64, u64)>,
}

impl std::fmt::Debug for PresenceLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresenceLock")
            .field("name", &self.name)
            .field("held", &self.file.is_some())
            .finish()
    }
}

impl PresenceLock {
    /// Take `agent`'s presence lock for the brain at `brain`.
    pub(crate) fn take(brain: &Path, agent: &str) -> std::io::Result<PresenceLock> {
        let brain = std::fs::canonicalize(brain)?;
        let side = match sidecar::open(&brain) {
            SidecarState::Ready(s) => s,
            SidecarState::Unavailable(why) => return Err(std::io::Error::other(why)),
        };
        let mut p = PresenceLock {
            brain,
            name: file_name(agent),
            side,
            file: None,
            id: None,
        };
        p.lock()?;
        Ok(p)
    }

    fn lock(&mut self) -> std::io::Result<()> {
        let f = self.side.lock_file(&self.name)?;
        // A probe holds it exclusively for an instant; wait that out.
        let mut tries = 0;
        loop {
            match FileExt::try_lock_shared(&f) {
                Ok(()) => break,
                Err(e) if tries >= 50 => return Err(e),
                Err(_) => {
                    tries += 1;
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
        self.id = identity(&f);
        self.file = Some(LockedFile::new(f));
        Ok(())
    }

    /// Re-check that the name still names the file this lock holds, and take
    /// it again if it doesn't (the sidecar directory was deleted or
    /// replaced). Best effort: a failure leaves the agent `unknown` to
    /// probes, never "not alive".
    pub(crate) fn refresh(&mut self) {
        let now = self
            .side
            .open_existing(&self.name, false)
            .ok()
            .and_then(|f| identity(&f));
        if self.file.is_some() && now.is_some() && now == self.id {
            return;
        }
        self.file = None;
        if let SidecarState::Ready(s) = sidecar::open(&self.brain) {
            self.side = s;
        }
        let _ = self.lock();
    }
}

/// Whether `agent`'s process is alive, by its presence lock on the brain at
/// `brain`: `Some(true)` alive, `Some(false)` gone, `None` unknown (no lock
/// file, no sidecar directory, or a file that changed under the probe).
/// Never creates anything.
pub(crate) fn probe(brain: &Path, agent: &str) -> Option<bool> {
    probe_with(brain, agent, true)
}

/// [`probe`] without the retry: a refused lock is "alive" at once. For a
/// claim's early lapse inside a write window (RB-012.6 item 3): there a
/// wrong "alive" only delays a lapse that already needs two looks 10 s
/// apart, and the retry's 20 ms sleep would hold the brain's lock once per
/// live holder.
pub(crate) fn probe_once(brain: &Path, agent: &str) -> Option<bool> {
    probe_with(brain, agent, false)
}

fn probe_with(brain: &Path, agent: &str, retry: bool) -> Option<bool> {
    let brain = std::fs::canonicalize(brain).ok()?;
    let side = sidecar::open_if_present(&brain)?;
    let name = file_name(agent);
    for attempt in 0..(1 + usize::from(retry)) {
        let f = side.open_existing(&name, true).ok()?;
        match FileExt::try_lock_exclusive(&f) {
            Ok(()) => {
                let held = LockedFile::new(f);
                let mine = identity(&held);
                let named = side
                    .open_existing(&name, false)
                    .ok()
                    .and_then(|g| identity(&g));
                drop(held);
                return (mine.is_some() && mine == named).then_some(false);
            }
            Err(_) if attempt == 0 && retry => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return Some(true),
        }
    }
    None
}
