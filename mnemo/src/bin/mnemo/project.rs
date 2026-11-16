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
/// else the working directory. The flag says it was `CLAUDE_PROJECT_DIR`.
pub fn start_dir(moved: bool) -> Result<(PathBuf, bool), String> {
    if !moved {
        if let Some(d) = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from) {
            if d.is_absolute() {
                return Ok((d, true));
            }
        }
    }
    std::env::current_dir()
        .map(|d| (d, false))
        .map_err(|e| format!("can't read the working directory: {e}"))
}

/// The project brain from where the lookup starts. When that was
/// `CLAUDE_PROJECT_DIR` and the working directory is in a git repository
/// with another common dir, one line on stderr says which brain is used
/// (12.2 step 1's review: an agent that `cd`s to another repository in a
/// Claude Code session would otherwise get the session's brain silently).
/// `quiet` skips the line, for a hook, whose stderr isn't the user's.
pub fn find(moved: bool, quiet: bool) -> Result<PathBuf, String> {
    let (start, from_env) = start_dir(moved)?;
    let common = common_dir(&start)?;
    if from_env && !quiet {
        if let Ok(cwd) = std::env::current_dir() {
            let here = std::fs::canonicalize(&cwd).unwrap_or(cwd);
            let there = std::fs::canonicalize(&start).unwrap_or_else(|_| start.clone());
            // The second `git rev-parse` runs only when the two differ (a
            // subdirectory may be another repository, so it isn't skipped).
            if here != there {
                if let Ok(other) = common_dir(&here) {
                    if other != common {
                        eprintln!(
                            "mnemo: using the project brain of $CLAUDE_PROJECT_DIR ({}); \
                             -C . uses this directory's",
                            common.join(DIR).join(FILE).display()
                        );
                    }
                }
            }
        }
    }
    let brain = common.join(DIR).join(FILE);
    refuse_link(&brain)?;
    Ok(brain)
}

/// RB-011.2: a project brain found without a path must be a real file in a
/// real directory. A symbolic link in either place (one a cloned or
/// unpacked repository can carry) could lead to another project's brain, so
/// it's refused; a brain named by its path may still be a link.
pub fn refuse_link(brain: &Path) -> Result<(), String> {
    let dir = brain.parent().unwrap_or(brain);
    for p in [dir, brain] {
        if std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(format!(
                "{} is a symbolic link; a project brain found without a path must be a real \
                 file in a real directory (give the brain's path to use it)",
                p.display()
            ));
        }
    }
    Ok(())
}

/// The git common dir of the repository `start` is in, absolute.
/// 13a (R-010.6): the git common directory found without running git, for
/// the per-turn hook, which runs on every turn: the nearest `.git` above
/// `start` that's a directory (a repository), or a `gitdir: ` file (a
/// worktree or a submodule), following a worktree's `commondir`. `None`
/// for anything else (a `GIT_DIR` or `GIT_COMMON_DIR` in the environment,
/// a `.git` it can't read): then [`common_dir`] asks git.
pub fn common_dir_fast(start: &Path) -> Option<PathBuf> {
    if std::env::var_os("GIT_DIR").is_some() || std::env::var_os("GIT_COMMON_DIR").is_some() {
        return None;
    }
    let mut d = Some(start);
    while let Some(dir) = d {
        let dot = dir.join(".git");
        match std::fs::symlink_metadata(&dot) {
            Ok(m) if m.is_dir() => return Some(dot),
            Ok(m) if m.is_file() => {
                let text = std::fs::read_to_string(&dot).ok()?;
                let gitdir = text.trim_end().strip_prefix("gitdir: ")?;
                let gitdir = dir.join(gitdir);
                return match std::fs::read_to_string(gitdir.join("commondir")) {
                    Ok(c) => Some(gitdir.join(c.trim_end())),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(gitdir),
                    Err(_) => None,
                };
            }
            Ok(_) => return None,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => d = dir.parent(),
            Err(_) => return None,
        }
    }
    None
}

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
        .map(plain)
        .map_err(|e| format!("no project brain here: {}: {e}", dir.display()))
}

/// `p` without Windows' verbatim prefix (`\\?\C:\…` from `canonicalize`),
/// which a client may not run a command from, and a person doesn't expect
/// to read. A path the prefix is needed for (over 260 characters) keeps it.
pub fn plain(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match s.strip_prefix(r"\\?\") {
        Some(rest) if rest.len() < 260 && rest.as_bytes().get(1) == Some(&b':') => {
            PathBuf::from(rest)
        }
        _ => p,
    }
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

/// The marked block `onboard` keeps in `<git-common-dir>/info/exclude`: the
/// files it wrote for this user inside the work tree, so git doesn't offer
/// to commit them. `onboard --undo` removes the block.
pub const EXCLUDE_OPEN: &str =
    "# >>> mnemo onboard: files it wrote for this user (mnemo onboard --undo removes this block)";
pub const EXCLUDE_CLOSE: &str = "# <<< mnemo onboard";

/// The exclude file's path.
pub fn exclude_path(common: &Path) -> PathBuf {
    common.join("info").join("exclude")
}

/// `text` (the exclude file's) with `lines` in the marked block, added once;
/// the rest of the file as it was. `lines` empty removes the block.
pub fn with_excluded(text: &str, lines: &[String]) -> String {
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut inside: Vec<String> = Vec::new();
    let mut state = 0; // 0 before, 1 inside, 2 after
    for l in text.lines() {
        match state {
            0 if l == EXCLUDE_OPEN => state = 1,
            1 if l == EXCLUDE_CLOSE => state = 2,
            // A block whose closing line was edited away ends at the first
            // line that isn't one of ours (RB-011.5 item 5).
            1 if !l.starts_with('/') => {
                state = 2;
                after.push(l.to_string());
            }
            0 => before.push(l.to_string()),
            1 => inside.push(l.to_string()),
            _ => after.push(l.to_string()),
        }
    }
    let mut keep: Vec<String> = Vec::new();
    if !lines.is_empty() {
        for l in inside.iter().chain(lines) {
            if !keep.contains(l) {
                keep.push(l.clone());
            }
        }
    }
    let mut out: Vec<String> = before;
    if !keep.is_empty() {
        out.push(EXCLUDE_OPEN.to_string());
        out.extend(keep);
        out.push(EXCLUDE_CLOSE.to_string());
    }
    out.extend(after);
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// A work-tree file as an exclude pattern: `/` and its path from the root.
pub fn exclude_line(root: &Path, file: &Path) -> Option<String> {
    let rel = file.strip_prefix(root).ok()?;
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    Some(format!("/{}", parts.join("/")))
}

/// The work tree's root from `start` (`git rev-parse --show-toplevel`).
pub fn toplevel(start: &Path) -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(start)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git didn't run: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{} isn't in a git work tree; run this in one",
            start.display()
        ));
    }
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    std::fs::canonicalize(&p)
        .map(plain)
        .map_err(|e| format!("{}: {e}", p.display()))
}

/// Whether git tracks `file` (then an exclude line has no effect).
pub fn tracked(root: &Path, file: &Path) -> bool {
    Command::new("git")
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(file)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_drops_the_verbatim_prefix() {
        assert_eq!(
            plain(PathBuf::from(r"\\?\C:\x\mnemo.exe")),
            PathBuf::from(r"C:\x\mnemo.exe")
        );
        assert_eq!(
            plain(PathBuf::from(r"\\?\UNC\srv\share\a")),
            PathBuf::from(r"\\srv\share\a")
        );
        assert_eq!(
            plain(PathBuf::from("/usr/bin/mnemo")),
            PathBuf::from("/usr/bin/mnemo")
        );
    }

    #[test]
    fn the_exclude_block_is_added_once_and_removed_whole() {
        let base = "# git's own\n*.log\n";
        let a = with_excluded(base, &["/.cursor/mcp.json".into()]);
        assert!(a.starts_with(base), "{a}");
        assert_eq!(a.matches(EXCLUDE_OPEN).count(), 1);
        let b = with_excluded(&a, &["/.cursor/mcp.json".into(), "/.mcp.json".into()]);
        assert_eq!(b.matches("/.cursor/mcp.json").count(), 1, "{b}");
        assert!(b.contains("/.mcp.json"));
        assert_eq!(with_excluded(&b, &[]), base);
        assert_eq!(with_excluded("", &[]), "");
        // No closing line: the block ends at the first line not ours.
        let open = format!("{base}{EXCLUDE_OPEN}\n/.mcp.json\n\n# mine\n*.tmp\n");
        assert_eq!(
            with_excluded(&open, &[]),
            format!("{base}\n# mine\n*.tmp\n")
        );
    }
}
