//! 12.4 shared access, the lock layer (v0.14.0 step 2a). The design is in
//! `docs/design/12.4-shared-access.md`: "The protocol", "Writer
//! preference", "Long operations", "Sidecar files" and "Things (A)
//! refuses". `Mnemo::open_shared` builds on it in step 2b.

// Until step 2b's `open_shared` calls it, only the tests (and the
// `failpoints` test hooks) reach this module.
#![cfg_attr(not(any(test, feature = "failpoints")), allow(dead_code))]

mod call;
mod fscheck;
mod lock;
mod sidecar;

#[allow(unused_imports)]
pub(crate) use call::Call;
pub use call::CancelToken;
pub use lock::SharedOptions;
#[allow(unused_imports)]
pub(crate) use lock::{BrainLocks, Held, LockKind, WINDOWS_OLD_RENAME};
pub(crate) use sidecar::pid_alive;

/// Test hooks: not a stable API.
#[cfg(any(test, feature = "failpoints"))]
pub(crate) mod testing {
    pub(crate) use super::fscheck::{classify_synced, linux_fs_allowed};
    pub(crate) use super::sidecar::{sidecar_path, INTENT, LOCK_INFO, SUFFIX};
}
