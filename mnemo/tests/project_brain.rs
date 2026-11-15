//! 12.2 step 1: the project brain, found without a path
//! (`docs/design/12.2-onboard.md`, section 3). A brain argument left out is
//! `<git-common-dir>/mnemo/project.mnemo`, found from `-C`, else
//! `CLAUDE_PROJECT_DIR`, else the working directory.
//!
//! Every run has a temporary `HOME` and a `PATH` of a temporary directory
//! (holding at most a fake `claude`) plus git's own directory, so nothing
//! here touches this machine's client configs, the real `claude` CLI or the
//! keychain. Brains are made with `init --key-file` and opened through
//! `MNEMO_KEY_FILE`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;
use tempfile::TempDir;

const CLEARED: &[&str] = &[
    "MNEMO_PASSPHRASE",
    "MNEMO_KEY",
    "MNEMO_KEY_FILE",
    "MNEMO_KEY_COMMAND",
    "MNEMO_DEV_KEYSTORE",
    "XDG_CONFIG_HOME",
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_PROJECT_DIR",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
];

/// The directory holding `git` on this process's PATH.
fn git_dir() -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .find(|d| d.join("git").is_file() || d.join("git.exe").is_file())
        .expect("git on the PATH")
}

/// A 4-wide embed command: answers each JSON string with a vector.
const EMBED_PY: &str = r#"
import sys, json
for line in sys.stdin:
    s = json.loads(line)
    print(json.dumps([1.0, float(len(s)) + 1.0, 0.5, 0.25]), flush=True)
"#;

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

struct Env {
    dir: TempDir,
}

impl Env {
    /// A repo `main` with one commit, a linked worktree `wt`, a
    /// subdirectory `main/sub/deep`, and a folder `plain` outside git.
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        let e = Env { dir };
        for d in ["home", "bin", "main/sub/deep", "plain"] {
            std::fs::create_dir_all(e.p(d)).unwrap();
        }
        std::fs::write(e.p("embed.py"), EMBED_PY).unwrap();
        e.git(&e.p("main"), &["init", "-q"]);
        e.git(
            &e.p("main"),
            &["commit", "-q", "--allow-empty", "-m", "start"],
        );
        e.git(&e.p("main"), &["worktree", "add", "-q", "../wt"]);
        e
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    fn git(&self, cwd: &Path, args: &[&str]) {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(cwd)
            .env("HOME", self.p("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    /// The project brain's path, as git and the filesystem name it.
    fn brain(&self) -> PathBuf {
        std::fs::canonicalize(self.p("main/.git"))
            .unwrap()
            .join("mnemo")
            .join("project.mnemo")
    }
    fn key(&self) -> PathBuf {
        self.p("brain.key")
    }
    fn cmd(&self, cwd: &Path, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.args(args);
        for v in CLEARED {
            c.env_remove(v);
        }
        let path = std::env::join_paths([self.p("bin"), git_dir()]).unwrap();
        c.env("HOME", self.p("home"))
            .env("USERPROFILE", self.p("home"))
            .env("APPDATA", self.p("home"))
            .env("PATH", path)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("MNEMO_DEV_NO_KEYCHAIN", "1")
            .current_dir(cwd)
            .stdin(Stdio::null());
        if self.key().exists() {
            c.env("MNEMO_KEY_FILE", self.key());
        }
        c
    }
    fn run(&self, cwd: &Path, args: &[&str]) -> Output {
        self.cmd(cwd, args).output().unwrap()
    }
    fn ok(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.run(cwd, args);
        assert!(
            o.status.success(),
            "{args:?}: {}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        String::from_utf8_lossy(&o.stdout).into_owned()
    }
    fn fails(&self, cwd: &Path, args: &[&str]) -> String {
        let o = self.run(cwd, args);
        assert!(!o.status.success(), "{args:?} should fail");
        String::from_utf8_lossy(&o.stderr).into_owned()
    }
    /// The project brain, made with no path from `main/sub/deep`.
    fn init(&self) {
        let key = self.key();
        self.ok(
            &self.p("main/sub/deep"),
            &[
                "init",
                "--dimensions",
                "4",
                "--key-file",
                key.to_str().unwrap(),
            ],
        );
    }
    /// Store `text` from `cwd`, embedded by the 4-wide test command.
    fn remember(&self, cwd: &Path, extra: &[&str], text: &str) {
        let embed = format!("{} \"{}\"", python(), self.p("embed.py").display());
        let mut args = extra.to_vec();
        args.extend([
            "remember",
            "--text",
            text,
            "--type",
            "semantic",
            "--embed-cmd",
            &embed,
            "--embed-name",
            "test-4",
        ]);
        self.ok(cwd, &args);
    }
    /// The live memories' contents, through `list --format jsonl` from `cwd`.
    fn contents(&self, cwd: &Path, extra: &[&str]) -> Vec<String> {
        let mut args = extra.to_vec();
        args.extend(["list", "--format", "jsonl"]);
        self.ok(cwd, &args)
            .lines()
            .filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter_map(|v| v["content"].as_str().map(str::to_string))
            .collect()
    }
}

#[test]
fn init_with_no_path_makes_the_brain_under_the_git_common_dir() {
    let e = Env::new();
    e.init();
    let brain = e.brain();
    assert!(brain.is_file(), "{}", brain.display());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(brain.parent().unwrap())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "{mode:o}");
    }
    // Nothing in the working tree.
    assert!(!e.p("main/sub/deep/project.mnemo").exists());
    assert!(!e.p("main/mnemo").exists());
    // A second init refuses the existing brain, and writes no key.
    let other = e.p("other.key");
    let err = e.fails(
        &e.p("main"),
        &[
            "init",
            "--dimensions",
            "4",
            "--key-file",
            other.to_str().unwrap(),
        ],
    );
    assert!(err.contains("exists"), "{err}");
    assert!(!other.exists());
}

#[test]
fn every_worktree_and_subdirectory_finds_the_same_brain() {
    let e = Env::new();
    e.init();
    e.remember(&e.p("wt"), &[], "written from the linked worktree");
    e.remember(&e.p("main/sub/deep"), &[], "written from a subdirectory");
    for cwd in ["main", "main/sub/deep", "wt"] {
        let c = e.contents(&e.p(cwd), &[]);
        assert!(
            c.iter().any(|s| s == "written from the linked worktree")
                && c.iter().any(|s| s == "written from a subdirectory"),
            "{cwd}: {c:?}"
        );
    }
    // `about`, `info` and `essence render` take no path either.
    let about = e.ok(&e.p("wt"), &["about", "--manifest-only"]);
    assert!(about.contains("project.mnemo"), "{about}");
    e.ok(&e.p("wt"), &["info"]);
    e.ok(&e.p("wt"), &["essence", "render", "--project"]);
}

#[test]
fn dash_c_runs_as_if_started_there() {
    let e = Env::new();
    e.init();
    let wt = e.p("wt");
    let wt = wt.to_str().unwrap();
    e.remember(&e.p("plain"), &["-C", wt], "through -C");
    assert!(e
        .contents(&e.p("plain"), &["-C", wt])
        .contains(&"through -C".to_string()));
    // Global, so it may follow the subcommand too.
    let o = e.ok(&e.p("plain"), &["list", "-C", wt, "--format", "jsonl"]);
    assert!(o.contains("through -C"), "{o}");
    // A relative path is read from the -C directory.
    let rel = e.brain();
    let rel = rel
        .strip_prefix(std::fs::canonicalize(e.p("main")).unwrap())
        .unwrap();
    let main = e.p("main");
    let o = e.ok(
        &e.p("plain"),
        &["-C", main.to_str().unwrap(), "info", rel.to_str().unwrap()],
    );
    assert!(!o.is_empty());
    // A -C that doesn't exist is an error.
    let err = e.fails(&e.p("plain"), &["-C", "no-such-dir", "info"]);
    assert!(err.contains("-C no-such-dir"), "{err}");
}

#[test]
fn claude_project_dir_is_used_and_dash_c_wins() {
    let e = Env::new();
    e.init();
    let main = std::fs::canonicalize(e.p("main")).unwrap();
    // From outside git, CLAUDE_PROJECT_DIR finds the brain.
    let o = e
        .cmd(&e.p("plain"), &["list", "--format", "jsonl"])
        .env("CLAUDE_PROJECT_DIR", &main)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    // -C wins over it: -C at a non-git folder fails even with it set.
    let plain = e.p("plain");
    let o = e
        .cmd(&e.p("main"), &["-C", plain.to_str().unwrap(), "info"])
        .env("CLAUDE_PROJECT_DIR", &main)
        .output()
        .unwrap();
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("isn't in a git repository"), "{err}");
    // A relative CLAUDE_PROJECT_DIR is ignored: the working directory is
    // used, and `plain` isn't in git.
    let o = e
        .cmd(&e.p("plain"), &["info"])
        .env("CLAUDE_PROJECT_DIR", "main")
        .output()
        .unwrap();
    assert!(!o.status.success());
}

#[test]
fn outside_git_or_before_init_the_error_says_what_to_do() {
    let e = Env::new();
    let err = e.fails(&e.p("plain"), &["about"]);
    assert!(
        err.contains("no project brain here") && err.contains("isn't in a git repository"),
        "{err}"
    );
    // In a repo with no brain yet: no directory is made, and init is named.
    let err = e.fails(&e.p("main"), &["recall", "--text", "x"]);
    assert!(
        err.contains("no project brain at") && err.contains("mnemo init"),
        "{err}"
    );
    assert!(!e.p("main/.git/mnemo").exists());
    // An explicit path still works anywhere, git or not.
    let key = e.p("plain.key");
    e.ok(
        &e.p("plain"),
        &[
            "init",
            "here.mnemo",
            "--dimensions",
            "4",
            "--key-file",
            key.to_str().unwrap(),
        ],
    );
    assert!(e.p("plain/here.mnemo").is_file());
    // Commands with a positional argument after the brain keep it required.
    let err = e.fails(&e.p("main"), &["get", "01M7ESWBTNXMB0KEM5XF95YGX3"]);
    assert!(err.contains("<PATH>") || err.contains("required"), "{err}");
}

#[cfg(unix)]
#[test]
fn a_symlink_in_place_of_the_brains_directory_is_refused() {
    let e = Env::new();
    std::fs::create_dir_all(e.p("elsewhere")).unwrap();
    std::os::unix::fs::symlink(e.p("elsewhere"), e.p("main/.git/mnemo")).unwrap();
    let key = e.key();
    let err = e.fails(
        &e.p("main"),
        &[
            "init",
            "--dimensions",
            "4",
            "--key-file",
            key.to_str().unwrap(),
        ],
    );
    assert!(err.contains("symbolic link"), "{err}");
    assert!(std::fs::read_dir(e.p("elsewhere"))
        .unwrap()
        .next()
        .is_none());
}

#[test]
fn serve_with_no_path_opens_the_project_brain() {
    let e = Env::new();
    e.init();
    e.remember(&e.p("main"), &[], "served without a path");
    let main = std::fs::canonicalize(e.p("main")).unwrap();
    // As Claude Code starts it: CLAUDE_PROJECT_DIR set, any working dir.
    let mut child = e
        .cmd(&e.p("plain"), &["serve", "--mcp", "--shared"])
        .env("CLAUDE_PROJECT_DIR", &main)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        let mut stdin = child.stdin.take().unwrap();
        for req in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list","arguments":{}}}"#,
        ] {
            writeln!(stdin, "{req}").unwrap();
        }
    }
    let o = child.wait_with_output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("served without a path"),
        "{out}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// The entry `mcp install --print` shows: its `+ ` lines, as JSON.
fn printed_entry(out: &str) -> Value {
    let json: String = out
        .lines()
        .filter_map(|l| l.strip_prefix("+ "))
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("{e}: {out}"))
}

#[test]
fn mcp_install_with_no_path_writes_no_path() {
    let e = Env::new();
    e.init();
    // Cursor, project scope (the default in a git work tree).
    let out = e.ok(
        &e.p("main"),
        &["mcp", "install", "--client", "cursor", "--print"],
    );
    let entry = printed_entry(&out);
    let args: Vec<&str> = entry["mnemo"]["args"]
        .as_array()
        .unwrap_or_else(|| panic!("{out}"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert_eq!(
        args,
        ["-C", "${workspaceFolder}", "serve", "--mcp", "--shared"],
        "{out}"
    );
    assert!(!out.contains("project.mnemo"), "{out}");
    // From the linked worktree too: the brain is outside its root, and the
    // default is still the project scope.
    let out = e.ok(
        &e.p("wt"),
        &["mcp", "install", "--client", "cursor", "--print"],
    );
    assert!(out.contains(".cursor"), "{out}");
    assert!(!out.contains("project.mnemo"), "{out}");
    // A user scope keeps the absolute path.
    let out = e.ok(
        &e.p("main"),
        &[
            "mcp", "install", "--client", "cursor", "--scope", "user", "--print",
        ],
    );
    assert!(out.contains("project.mnemo"), "{out}");
}

#[cfg(unix)]
#[test]
fn mcp_install_for_claude_code_names_no_path() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    e.init();
    // A fake `claude`, first on the PATH, that records its arguments.
    let log = e.p("claude.log");
    let script = e.p("bin/claude");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    for scope in ["project", "local"] {
        let _ = std::fs::remove_file(&log);
        e.ok(
            &e.p("main/sub/deep"),
            &[
                "mcp",
                "install",
                "--client",
                "claude-code",
                "--scope",
                scope,
                "--yes",
            ],
        );
        let args = std::fs::read_to_string(&log).unwrap();
        let json = args
            .lines()
            .find(|l| l.starts_with('{'))
            .unwrap_or_else(|| panic!("{args}"));
        let entry: Value = serde_json::from_str(json).unwrap();
        let a: Vec<&str> = entry["args"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(a, ["serve", "--mcp", "--shared"], "{scope}: {args}");
        assert!(
            args.contains(&format!("--scope\n{scope}\n")),
            "{scope}: {args}"
        );
    }
}
