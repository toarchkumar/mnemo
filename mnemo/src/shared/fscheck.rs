//! Where shared mode refuses to run (12.4 design note, "Things (A) refuses"):
//! a filesystem where `flock` may not reach every process that opens the
//! file, and, on macOS, a folder a sync client manages.
//!
//! Each refusal names its cause and can be overridden with
//! `allow_network_fs` (`SharedOptions::allow_network_fs` in Rust; the same
//! name on every surface).

use std::path::Path;

use crate::error::{MnemoError, Result};

/// Linux and Android filesystems shared mode runs on: an allow-list of
/// `statfs` magic numbers, so NFS, SMB, FUSE, virtiofs and 9p (a host and
/// a container, or WSL, sharing a file) are refused. overlayfs is allowed:
/// it's how a container sees its own disk, and its locks are local.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
pub(crate) const LINUX_ALLOWED: &[(u32, &str)] = &[
    (0xEF53, "ext2/3/4"),
    (0x5846_5342, "xfs"),
    (0x9123_683E, "btrfs"),
    (0x0102_1994, "tmpfs"),
    (0xF2F5_2010, "f2fs"),
    (0x794C_7630, "overlayfs"),
    (0x2FC1_2FC1, "zfs"),
];

/// Whether a Linux `statfs` magic number is on the allow-list.
pub(crate) fn linux_fs_allowed(magic: u32) -> bool {
    LINUX_ALLOWED.iter().any(|(m, _)| *m == magic)
}

/// macOS folders a sync client manages, for a path with symlinks resolved.
/// `Some("refuse: ...")` for iCloud Drive and File Provider folders;
/// `Some("warn: ...")` for `~/Desktop` and `~/Documents` (which "Desktop &
/// Documents in iCloud" syncs), unless a folder on the way is named
/// `*.nosync`; `None` otherwise.
pub(crate) fn classify_synced(path: &Path, home: &Path) -> Option<String> {
    let refuse = [
        ("Library/Mobile Documents", "iCloud Drive"),
        (
            "Library/CloudStorage",
            "a File Provider sync client (Dropbox, OneDrive, Google Drive, ...)",
        ),
    ];
    for (dir, what) in refuse {
        if path.starts_with(home.join(dir)) {
            return Some(format!(
                "refuse: {} is in a folder {what} syncs",
                path.display()
            ));
        }
    }
    let nosync = path
        .components()
        .any(|c| c.as_os_str().to_string_lossy().ends_with(".nosync"));
    for dir in ["Desktop", "Documents"] {
        if path.starts_with(home.join(dir)) && !nosync {
            return Some(format!(
                "warn: {} is under ~/{dir}, which \"Desktop & Documents in iCloud\" syncs if \
                 it's on; a synced copy isn't covered by the lock (put the brain in a folder \
                 named *.nosync to keep it local)",
                path.display()
            ));
        }
    }
    None
}

fn refused(why: String) -> MnemoError {
    MnemoError::Unsupported(format!(
        "{why}; shared mode needs a local disk where every process sees the same lock \
         (pass allow_network_fs to use it anyway)"
    ))
}

/// Check the brain at `resolved` (symlinks followed). Returns the warnings
/// to show once, or the refusal.
pub(crate) fn check(resolved: &Path, allow_network_fs: bool) -> Result<Vec<String>> {
    let mut notices = Vec::new();
    if let Some(why) = filesystem_problem(resolved)? {
        if allow_network_fs {
            notices.push(format!("{why} (allowed by allow_network_fs)"));
        } else {
            return Err(refused(why));
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        if let Some(c) = classify_synced(resolved, Path::new(&home)) {
            if let Some(why) = c.strip_prefix("refuse: ") {
                if allow_network_fs {
                    notices.push(format!("{why} (allowed by allow_network_fs)"));
                } else {
                    return Err(refused(why.to_string()));
                }
            } else if let Some(warning) = c.strip_prefix("warn: ") {
                notices.push(warning.to_string());
            }
        }
    }
    Ok(notices)
}

/// Why the filesystem holding `path` can't be trusted with the lock, if it
/// can't.
#[cfg(any(target_os = "macos", target_os = "ios"))]
fn filesystem_problem(path: &Path) -> Result<Option<String>> {
    const MNT_LOCAL: u32 = 0x0000_1000;
    let st = rustix::fs::statfs(path).map_err(std::io::Error::from)?;
    if st.f_flags & MNT_LOCAL == 0 {
        let kind: String = st
            .f_fstypename
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8 as char)
            .collect();
        return Ok(Some(format!(
            "{} is on a network filesystem ({kind})",
            path.display()
        )));
    }
    Ok(None)
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn filesystem_problem(path: &Path) -> Result<Option<String>> {
    let st = rustix::fs::statfs(path).map_err(std::io::Error::from)?;
    #[allow(clippy::unnecessary_cast)]
    let magic = st.f_type as u32;
    if linux_fs_allowed(magic) {
        Ok(None)
    } else {
        Ok(Some(format!(
            "{} is on a filesystem shared mode doesn't run on (statfs type {magic:#x}; \
             it runs on {})",
            path.display(),
            LINUX_ALLOWED
                .iter()
                .map(|(_, n)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        )))
    }
}

/// Windows: a UNC path, which is what a network share or a mapped network
/// drive resolves to. (`GetDriveType` would also see a drive letter mapped
/// to a share before it resolves; it needs `unsafe`, which this crate
/// forbids, and canonicalizing the path covers the common case.)
#[cfg(windows)]
fn filesystem_problem(path: &Path) -> Result<Option<String>> {
    let s = path.as_os_str().to_string_lossy();
    if s.starts_with(r"\\?\UNC\") || (s.starts_with(r"\\") && !s.starts_with(r"\\?\")) {
        return Ok(Some(format!("{} is on a network share", path.display())));
    }
    Ok(None)
}

/// Other platforms: no check is available, so nothing is refused.
#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn filesystem_problem(_path: &Path) -> Result<Option<String>> {
    Ok(None)
}
