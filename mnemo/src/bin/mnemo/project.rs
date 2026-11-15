//! 12.2 step 1: the project brain, found without a path
//! (`docs/design/12.2-onboard.md`, section 3).
//!
//! A project's brain is `<git-common-dir>/mnemo/project.mnemo`. The common
//! dir is the same from every worktree of a repository, git never tracks
//! it, and the brain's shared-mode sidecar stays inside it too. Commands
//! whose brain argument may be left out find it from where they start:
//! `-C <dir>` when given (the process has already moved there), else
//! `CLAUDE_PROJECT_DIR` when Claude Code set it (absolute), else the working
//! directory.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The brain's directory inside the common dir.
pub const DIR: &str = "mnemo";
/// The brain's file name.
pub const FILE: &str = "project.mnemo";

/// Where the lookup starts: the working directory when `-C` moved the
/// process there, else `CLAUDE_PROJECT_DIR` when it's set and absolute,
/// else the working directory.
pub fn start_dir(moved: bool) -> Result<PathBuf, String> {
    if !moved {
        if let Some(d) = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from) {
            if d.is_absolute() {
                return Ok(d);
            }
        }
    }
    std::env::current_dir().map_err(|e| format!("can't read the working directory: {e}"))
}

/// The git common dir of the repository `start` is in, absolute.
pub fn common_dir(start: &Path) -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-common-dir"])
        .current_dir(start)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "no project brain here: finding it needs git, which isn't on the PATH; \
                     give the brain's path ({e})"
                )
            } else {
                format!(
                    "no project brain here: git didn't run in {}: {e}",
                    start.display()
                )
            }
        })?;
    if !out.status.success() {
        return Err(format!(
            "no project brain here: {} isn't in a git repository; run this in one, or give \
             the brain's path",
            start.display()
        ));
    }
    let text = String::from_utf8(out.stdout)
        .map_err(|_| "no project brain here: git printed a path that isn't UTF-8".to_string())?;
    let line = text.trim_end_matches(['\n', '\r']);
    if line.is_empty() {
        return Err("no project brain here: git printed no common dir".into());
    }
    // Older git prints it relative to where it ran (`.git`, `../../.git`).
    let dir = start.join(line);
    std::fs::canonicalize(&dir)
        .map_err(|e| format!("no project brain here: {}: {e}", dir.display()))
}

/// The project brain's path from `start` (it may not exist yet).
pub fn brain_path(start: &Path) -> Result<PathBuf, String> {
    Ok(common_dir(start)?.join(DIR).join(FILE))
}

/// Create the brain's directory, mode 0700 on Unix, if it isn't there. A
/// symlink in its place is refused, so the brain can't be steered out of
/// the repository.
pub fn create_dir(brain: &Path) -> Result<(), String> {
    let dir = brain
        .parent()
        .ok_or_else(|| format!("{}: no parent directory", brain.display()))?;
    match std::fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(format!(
                "{} is a symbolic link; the project brain's directory must be a real one",
                dir.display()
            ))
        }
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => return Err(format!("{} exists and isn't a directory", dir.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    }
    // Only Unix sets a mode, so elsewhere `b` is never changed.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        b.mode(0o700);
    }
    match b.create(dir) {
        Ok(()) => Ok(()),
        // Made by another process since the check: check it again.
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            match std::fs::symlink_metadata(dir) {
                Ok(m) if m.is_dir() => Ok(()),
                _ => Err(format!(
                    "{} appeared while it was being created, and isn't a directory",
                    dir.display()
                )),
            }
        }
        Err(e) => Err(format!("can't create {}: {e}", dir.display())),
    }
}
