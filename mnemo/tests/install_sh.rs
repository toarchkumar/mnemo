//! 12.2 step 6: the repository's `install.sh`, run against a local
//! `MNEMO_INSTALL_BASE` (as the release workflow's `check-artifacts` does):
//! a good binary, a checksum mismatch, a bad `MNEMO_VERSION`, a URL or a
//! relative path as the base, and a directory that isn't on the PATH. The
//! "binary" is this build's own `mnemo`, so `--version` runs. Unix only (the
//! script is POSIX `sh`; `install.ps1` is checked in the release workflow).
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

/// This build's own version, which its `--version` says (the script checks
/// it).
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The release target `install.sh` picks for this machine.
fn target() -> String {
    let os = String::from_utf8(Command::new("uname").arg("-s").output().unwrap().stdout).unwrap();
    let arch = String::from_utf8(Command::new("uname").arg("-m").output().unwrap().stdout).unwrap();
    match (os.trim(), arch.trim()) {
        ("Linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("Linux", "aarch64") | ("Linux", "arm64") => "aarch64-unknown-linux-gnu",
        ("Darwin", "x86_64") => "x86_64-apple-darwin",
        ("Darwin", "arm64") => "aarch64-apple-darwin",
        other => panic!("no target for {other:?}"),
    }
    .to_string()
}

fn script() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../install.sh")
}

struct Env {
    dir: tempfile::TempDir,
}

impl Env {
    /// A base directory with this build's binary under its release name,
    /// and a SHA256SUMS that lists it (or `bad_sum`'s hash instead).
    fn new(bad_sum: bool) -> Env {
        Env::named(bad_sum, VERSION)
    }
    /// The same, with the binary under `version`'s release name.
    fn named(bad_sum: bool, version: &str) -> Env {
        let dir = tempfile::tempdir().unwrap();
        let e = Env { dir };
        std::fs::create_dir_all(e.p("base")).unwrap();
        std::fs::create_dir_all(e.p("home")).unwrap();
        let name = format!("mnemo-v{version}-{}", target());
        let bin = std::fs::read(env!("CARGO_BIN_EXE_mnemo")).unwrap();
        std::fs::write(e.p("base").join(&name), &bin).unwrap();
        let mut h = Sha256::new();
        h.update(if bad_sum {
            b"something else".as_slice()
        } else {
            &bin
        });
        let hex: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
        std::fs::write(
            e.p("base/SHA256SUMS"),
            format!("{hex}  {name}\n0000  mnemo-v{version}-other-target\n"),
        )
        .unwrap();
        e
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn run(&self, base: &str, version: &str, path_has_bin: bool) -> Output {
        self.run_with(base, version, path_has_bin, None)
    }
    /// [`Env::run`], with `extra` first on the PATH.
    fn run_with(
        &self,
        base: &str,
        version: &str,
        path_has_bin: bool,
        extra: Option<&Path>,
    ) -> Output {
        let mut path = String::from("/usr/bin:/bin:/usr/sbin:/sbin");
        if path_has_bin {
            path = format!("{}:{path}", self.p("home/.local/bin").display());
        }
        if let Some(x) = extra {
            path = format!("{}:{path}", x.display());
        }
        Command::new("sh")
            .arg(script())
            .env_clear()
            .env("HOME", self.p("home"))
            .env("PATH", path)
            .env("MNEMO_INSTALL_BASE", base)
            .env("MNEMO_VERSION", version)
            .output()
            .unwrap()
    }
    fn installed(&self) -> PathBuf {
        self.p("home/.local/bin/mnemo")
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn a_good_binary_is_installed_and_the_next_line_works_before_the_path_is_fixed() {
    let e = Env::new(false);
    let base = e.p("base");
    let o = e.run(base.to_str().unwrap(), VERSION, false);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(e.installed().is_file());
    assert!(
        t.contains("checked the SHA-256 (a local MNEMO_INSTALL_BASE: the attestation was skipped)"),
        "{t}"
    );
    assert!(t.contains(env!("CARGO_PKG_VERSION")), "--version ran: {t}");
    // Not on the PATH: the line to add, and the full path in "Next".
    let full = e.installed();
    assert!(t.contains("export PATH=\"$HOME/.local/bin:$PATH\""), "{t}");
    assert!(
        t.contains(&format!("Next: {} onboard", full.display())),
        "{t}"
    );
    // A first install: no keychain note.
    assert!(!t.contains("Always Allow"), "{t}");
    // Again, with the directory on the PATH: replaced in place, "Next:
    // mnemo onboard".
    let o = e.run(base.to_str().unwrap(), VERSION, true);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("Next: mnemo onboard"), "{t}");
    // An upgrade: on macOS, the keychain asks once (KEYCHAIN-REPROMPT).
    assert_eq!(
        t.contains("macOS asks once.\nChoose Always Allow"),
        cfg!(target_os = "macos"),
        "{t}"
    );
    // Nothing left beside it (the staged copy was moved into place).
    let left: Vec<_> = std::fs::read_dir(e.p("home/.local/bin"))
        .unwrap()
        .map(|d| d.unwrap().file_name())
        .collect();
    assert_eq!(left, ["mnemo"], "{left:?}");
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let e = Env::new(true);
    let o = e.run(e.p("base").to_str().unwrap(), VERSION, false);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains("doesn't match SHA256SUMS") && t.contains("nothing was installed"),
        "{t}"
    );
    assert!(!e.installed().exists());
}

#[test]
fn a_bad_version_or_base_is_refused() {
    let e = Env::new(false);
    let base = e.p("base");
    for v in ["1.0", "1.0.0;rm -rf x", "v1.0.0", "../1.0.0", ""] {
        let o = e.run(base.to_str().unwrap(), v, false);
        let t = text(&o);
        assert!(!o.status.success(), "{v:?}: {t}");
        assert!(
            t.contains("not a version") || t.contains("set MNEMO_VERSION too"),
            "{v:?}: {t}"
        );
    }
    for b in [
        "https://example.com/releases",
        "file:///tmp",
        "relative/dir",
        "/no/such/directory",
    ] {
        let o = e.run(b, VERSION, false);
        let t = text(&o);
        assert!(!o.status.success(), "{b}: {t}");
        assert!(t.contains("MNEMO_INSTALL_BASE"), "{b}: {t}");
    }
    // A version the base doesn't have.
    let o = e.run(base.to_str().unwrap(), "1.2.3", false);
    assert!(!o.status.success() && text(&o).contains("no mnemo-v1.2.3-"));
    assert!(!e.installed().exists());
}

/// RB-011.2: a binary whose `--version` isn't the version asked for (an
/// older release's file under a newer name) installs nothing.
#[test]
fn a_binary_of_another_version_installs_nothing() {
    let e = Env::named(false, "9.8.7");
    let o = e.run(e.p("base").to_str().unwrap(), "9.8.7", false);
    let t = text(&o);
    assert!(!o.status.success(), "{t}");
    assert!(
        t.contains(&format!(
            "says 'mnemo {}', not 'mnemo 9.8.7'; nothing was installed",
            env!("CARGO_PKG_VERSION")
        )),
        "{t}"
    );
    assert!(!e.installed().exists());
    let left: Vec<_> = std::fs::read_dir(e.p("home/.local/bin"))
        .unwrap()
        .map(|d| d.unwrap().file_name())
        .collect();
    assert!(left.is_empty(), "the staged copy is removed: {left:?}");
}

/// v0.16.1: another `mnemo` on the PATH (an older install a shell may
/// still run from its command hash, or one that comes first) gets a line
/// naming it, with what to do; with none, there's no such line.
#[test]
fn another_mnemo_on_the_path_is_named() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new(false);
    let old = e.p("old-bin");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("mnemo"), "#!/bin/sh\necho mnemo 0.12.3\n").unwrap();
    std::fs::set_permissions(old.join("mnemo"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let base = e.p("base");
    let o = e.run_with(base.to_str().unwrap(), VERSION, true, Some(&old));
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains(&format!(
            "another mnemo is on your PATH: {}/mnemo\n  If `mnemo --version` doesn't say mnemo {VERSION}, run `hash -r`\n  (zsh: `rehash`) or open a new terminal.\n  If it still doesn't, remove that one, or put ",
            old.display()
        )),
        "{t}"
    );
    // What to do is on short lines of its own (ONBOARD-UX); only a path
    // makes one longer.
    for l in t
        .lines()
        .filter(|l| l.starts_with("  ") && !l.contains('/'))
    {
        assert!(l.chars().count() <= 72, "{l}");
    }
    // The installed one itself isn't named.
    assert_eq!(t.matches("another mnemo").count(), 1, "{t}");
    let o = e.run(base.to_str().unwrap(), VERSION, true);
    assert!(!text(&o).contains("another mnemo"), "{}", text(&o));
}
