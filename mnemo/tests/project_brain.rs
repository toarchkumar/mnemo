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
    "MNEMO_AGENT",
    "MNEMO_ROLE",
];

/// The directory holding `name` on this process's PATH.
fn tool_dir(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").expect("PATH");
    std::env::split_paths(&path)
        .find(|d| d.join(name).is_file() || d.join(format!("{name}.exe")).is_file())
        .unwrap_or_else(|| panic!("{name} on the PATH"))
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
        // The test's own directory first (a fake `claude` shadows any
        // real one), then git and the embed command's python.
        let path =
            std::env::join_paths([self.p("bin"), tool_dir("git"), tool_dir(python())]).unwrap();
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
    // `get <ID>` looks for the project brain (v0.16.1)...
    let err = e.fails(&e.p("main"), &["get", "01M7ESWBTNXMB0KEM5XF95YGX3"]);
    assert!(err.contains("no project brain at"), "{err}");
    // ...as `update <ID>` does since v0.17; a command that still needs both
    // arguments (`import <PATH> <FILE>`) keeps them required.
    let err = e.fails(&e.p("main"), &["update", "01M7ESWBTNXMB0KEM5XF95YGX3"]);
    assert!(err.contains("no project brain at"), "{err}");
    let err = e.fails(&e.p("main"), &["import", "only-one-argument"]);
    assert!(err.contains("<FILE>") || err.contains("required"), "{err}");
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

/// 12.2 step 1's review: when `CLAUDE_PROJECT_DIR` picks the brain and the
/// working directory is in another repository, one stderr line says so.
#[test]
fn claude_project_dir_names_the_brain_when_the_working_directory_differs() {
    let e = Env::new();
    e.init();
    std::fs::create_dir_all(e.p("other")).unwrap();
    e.git(&e.p("other"), &["init", "-q"]);
    let main = std::fs::canonicalize(e.p("main")).unwrap();
    let note = "using the project brain of $CLAUDE_PROJECT_DIR";
    // From another repository: the session's brain is used, and named.
    let o = e
        .cmd(&e.p("other"), &["info"])
        .env("CLAUDE_PROJECT_DIR", &main)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "{err}");
    assert!(err.contains(note) && err.contains("-C ."), "{err}");
    assert!(err.contains("project.mnemo"), "{err}");
    // -C . uses the working directory's repository, which has no brain.
    let o = e
        .cmd(&e.p("other"), &["-C", ".", "info"])
        .env("CLAUDE_PROJECT_DIR", &main)
        .output()
        .unwrap();
    assert!(!o.status.success());
    // From inside the project, from its linked worktree (the same common
    // dir), or outside git: no line.
    for cwd in ["main/sub/deep", "wt", "plain"] {
        let o = e
            .cmd(&e.p(cwd), &["info"])
            .env("CLAUDE_PROJECT_DIR", &main)
            .output()
            .unwrap();
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(o.status.success(), "{cwd}: {err}");
        assert!(!err.contains(note), "{cwd}: {err}");
    }
}

/// 12.2 step 3 (from step 2's review): `recovery-code`, `unlock` and
/// `rekey` take no path either, so the guide's `mnemo recovery-code` works
/// from a subdirectory and from a linked worktree.
#[test]
fn recovery_code_unlock_and_rekey_find_the_project_brain() {
    let e = Env::new();
    e.init();
    for cwd in ["main/sub/deep", "wt"] {
        for args in [&["recovery-code"][..], &["unlock"][..], &["rekey"][..]] {
            let err = e.fails(&e.p(cwd), args);
            // Past the arguments: the keychain (there's none here) or the new
            // secret is what's missing, not the path.
            assert!(
                !err.contains("required") && !err.contains("no project brain"),
                "{cwd} {args:?}: {err}"
            );
            assert!(
                err.contains("keychain") || err.contains("passphrase"),
                "{cwd} {args:?}: {err}"
            );
        }
    }
    let err = e.fails(&e.p("plain"), &["recovery-code"]);
    assert!(err.contains("no project brain here"), "{err}");
}

/// 12.2 step 5 (from step 1's review): with the path left out, `mcp
/// install` defaults to Claude Code's local scope, and to Cursor's project
/// file, which it lists in `.git/info/exclude`.
#[cfg(unix)]
#[test]
fn mcp_install_with_no_path_defaults_to_this_users_scope() {
    use std::os::unix::fs::PermissionsExt;
    let e = Env::new();
    e.init();
    let log = e.p("claude.log");
    let script = e.p("bin/claude");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    e.ok(
        &e.p("main"),
        &["mcp", "install", "--client", "claude-code", "--yes"],
    );
    let args = std::fs::read_to_string(&log).unwrap();
    assert!(args.contains("--scope\nlocal\n"), "{args}");
    e.ok(
        &e.p("main"),
        &["mcp", "install", "--client", "cursor", "--yes"],
    );
    assert!(e.p("main/.cursor/mcp.json").is_file());
    let ex = std::fs::read_to_string(e.p("main/.git/info/exclude")).unwrap();
    assert!(ex.contains("/.cursor/mcp.json"), "{ex}");
}

/// v0.16.1: `get <ID>` finds the project brain (and `get <PATH> <ID>`
/// still works), printing the whole content; `search` and `snapshots`
/// take no path either.
#[test]
fn get_search_and_snapshots_take_no_path() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let long = format!("a long memory: {}", "word ".repeat(40));
    e.remember(&main, &[], &long);
    let listed: Vec<Value> = e
        .ok(&main, &["list", "--format", "jsonl"])
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let id = listed
        .iter()
        .find(|v| v["content"].as_str() == Some(long.as_str()))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    // One argument: the id, in the project brain; the table holds it all.
    let out = e.ok(&e.p("main/sub"), &["get", &id]);
    assert!(out.contains(long.trim_end()), "{out}");
    // Two: the path, then the id, as before.
    let out = e.ok(
        &main,
        &["get", e.brain().to_str().unwrap(), &id, "--format", "json"],
    );
    assert!(out.contains(&id), "{out}");
    // One argument that isn't an id says so.
    let err = e.fails(&main, &["get", e.brain().to_str().unwrap()]);
    assert!(err.contains("`get` takes `[PATH] <ID>`"), "{err}");
    let err = e.fails(&main, &["get"]);
    assert!(err.contains("needs a memory's id, or `--key"), "{err}");
    // `search` and `snapshots`, from a subdirectory.
    let out = e.ok(
        &e.p("main/sub"),
        &["search", "--query", "1,1,0.5,0.25", "--top-k", "1"],
    );
    assert!(
        out.contains(&id[..10]) || out.contains("a long memory"),
        "{out}"
    );
    e.ok(&e.p("main/sub"), &["snapshots"]);
    // v0.20.2 (G1): `--key` takes no path, or the path alone.
    let embed = format!("{} \"{}\"", python(), e.p("embed.py").display());
    for text in ["release: v1", "release: v2"] {
        e.ok(
            &main,
            &[
                "remember",
                "--text",
                text,
                "--type",
                "semantic",
                "--key",
                "process.release",
                "--embed-cmd",
                &embed,
                "--embed-name",
                "test-4",
            ],
        );
    }
    let out = e.ok(&e.p("main/sub"), &["get", "--key", "process.release"]);
    assert!(
        out.contains("release: v2") && !out.contains("release: v1"),
        "{out}"
    );
    let brain = e.brain().to_str().unwrap().to_string();
    let m: Value = serde_json::from_str(&e.ok(
        &main,
        &[
            "get",
            &brain,
            "--key",
            "process.release",
            "--format",
            "json",
        ],
    ))
    .unwrap();
    assert_eq!(m["content"], "release: v2", "{m}");
}

/// 13a step 1 (v0.17): `default-agent`; a write that names no agent goes to
/// it, and `history` reads it back; `MNEMO_AGENT` stamps `_mnemo.author`;
/// `--visibility`, `--review-by` and `--as-of`; and `update`, `thread start`
/// and `essence resolve` with no path (CLI-PATHLESS-2).
#[test]
fn default_agent_author_freshness_and_more_commands_without_a_path() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let embed = format!("{} \"{}\"", python(), e.p("embed.py").display());
    assert!(e.ok(&main, &["default-agent"]).contains("(none"));
    assert_eq!(
        e.ok(&main, &["default-agent", "--set", "cursor-agent"])
            .trim(),
        "cursor-agent\tupdated"
    );
    assert_eq!(
        e.ok(&main, &["default-agent", "--set", "cursor-agent"])
            .trim(),
        "cursor-agent\tunchanged"
    );
    assert_eq!(e.ok(&main, &["default-agent"]).trim(), "cursor-agent");
    assert!(e
        .fails(&main, &["default-agent", "--set", "no spaces"])
        .contains("isn't an agent id"));

    // A keyed write with no --agent, by worker-a, with freshness.
    let o = e
        .cmd(
            &main,
            &[
                "remember",
                "--text",
                "the deploy target is staging-7",
                "--key",
                "fact.deploy",
                "--visibility",
                "private",
                "--review-by",
                "30d",
                "--as-of",
                "2026-01-02",
                "--embed-cmd",
                &embed,
                "--embed-name",
                "test-4",
            ],
        )
        .env("MNEMO_AGENT", "worker-a")
        .env("MNEMO_ROLE", "worker")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let id = String::from_utf8_lossy(&o.stdout)
        .split('\t')
        .next()
        .unwrap()
        .trim()
        .to_string();
    let m: Value = serde_json::from_str(&e.ok(&main, &["get", &id, "--format", "json"])).unwrap();
    assert_eq!(m["agent_id"], "cursor-agent", "{m}");
    let meta = &m["metadata"];
    assert_eq!(meta["_mnemo.author"], "worker-a", "{m}");
    assert_eq!(meta["_mnemo.visibility"], "private");
    assert_eq!(
        meta["_mnemo.as_of"], 1_767_312_000,
        "2026-01-02 at midnight UTC"
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let review = meta["_mnemo.review_by"].as_i64().unwrap();
    assert!(
        (review - now - 30 * 86_400).abs() < 120,
        "{review} vs {now}"
    );
    // `history` with no --agent finds it under the default agent (it's
    // private, so it's asked for).
    let h = e.ok(
        &main,
        &[
            "history",
            "--key",
            "fact.deploy",
            "--visibility",
            "all",
            "--format",
            "json",
        ],
    );
    assert!(h.contains("staging-7"), "{h}");
    // Bad values are refused before anything is written.
    let err = e.fails(
        &main,
        &[
            "remember",
            "--text",
            "x",
            "--review-by",
            "soon",
            "--embed-cmd",
            &embed,
            "--embed-name",
            "test-4",
        ],
    );
    assert!(err.contains("--review-by"), "{err}");
    assert!(!e
        .fails(
            &main,
            &["remember", "--text", "x", "--visibility", "secret"]
        )
        .is_empty());
    // An invalid MNEMO_AGENT is ignored, with a note.
    let o = e
        .cmd(
            &main,
            &[
                "remember",
                "--text",
                "no author",
                "--embed-cmd",
                &embed,
                "--embed-name",
                "test-4",
            ],
        )
        .env("MNEMO_AGENT", "not valid!")
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("MNEMO_AGENT isn't"));
    let id2 = String::from_utf8_lossy(&o.stdout).trim().to_string();
    let m2: Value = serde_json::from_str(&e.ok(&main, &["get", &id2, "--format", "json"])).unwrap();
    assert!(m2["metadata"].get("_mnemo.author").is_none(), "{m2}");

    // `update <ID>` with no path, setting the visibility only.
    let out = e.ok(&e.p("main/sub"), &["update", &id, "--visibility", "public"]);
    assert!(out.contains("updated"), "{out}");
    let m: Value = serde_json::from_str(&e.ok(&main, &["get", &id, "--format", "json"])).unwrap();
    assert_eq!(m["metadata"]["_mnemo.visibility"], "public");
    assert_eq!(
        m["metadata"]["_mnemo.author"], "worker-a",
        "an edit keeps the author"
    );
    assert!(e
        .fails(&main, &["update", "not-an-id"])
        .contains("`update` takes `[PATH] <ID>`"));

    // `essence resolve <ID>` and `thread start <NEW>` with no path.
    let added = e.ok(
        &main,
        &[
            "essence",
            "add",
            "--project",
            "--section",
            "decisions",
            "--text",
            "ship it",
        ],
    );
    let item = added.split('\t').next().unwrap().trim().to_string();
    assert!(e
        .ok(&main, &["essence", "resolve", &item])
        .contains("resolved"));
    let err = e.fails(&main, &["thread", "start", "brain.mnemo", "--from", "t1"]);
    assert!(err.contains("looks like a brain's path"), "{err}");
    let o = e.run(&main, &["thread", "start", "t2", "--from", "t1"]);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        !err.contains("required") && !err.contains("takes `[PATH]"),
        "parsed as a thread id: {err}"
    );

    // Clearing it sends writes to `default` again.
    assert!(e
        .ok(&main, &["default-agent", "--clear"])
        .contains("updated"));
    assert!(e.ok(&main, &["default-agent"]).contains("(none"));
}

/// 13a step 2 (v0.17): `post`, `inbox` and `ack`, as an agent given by
/// `--as`/`--role` or by `MNEMO_AGENT`/`MNEMO_ROLE`; listing marks seen;
/// `ack <ID>` takes no path; `inbox --prune`.
#[test]
fn post_inbox_and_ack_as_an_agent() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let out = e.ok(
        &main,
        &[
            "post",
            "worker-a",
            "step 2 dispatched",
            "--body",
            "messages",
            "--as",
            "planner-a",
        ],
    );
    let (id, hop) = out.trim().split_once('\t').unwrap();
    assert_eq!(hop, "hop 0");
    e.ok(
        &main,
        &["post", "role:worker", "for any worker", "--as", "planner-a"],
    );
    // `--as` and `--role`, then the same from the environment.
    let t = e.ok(
        &e.p("main/sub"),
        &["inbox", "--as", "worker-a", "--role", "worker"],
    );
    assert!(
        t.contains("planner-a -> worker-a")
            && t.contains("step 2 dispatched")
            && t.contains("    messages"),
        "{t}"
    );
    assert!(t.contains("for any worker"), "{t}");
    let o = e
        .cmd(&main, &["inbox", "--format", "json"])
        .env("MNEMO_AGENT", "worker-a")
        .output()
        .unwrap();
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(
        v.as_array().unwrap().len(),
        1,
        "no role: only what's to worker-a: {v}"
    );
    assert_eq!(v[0]["delivery"], "seen", "listing marked it seen: {v}");
    // Ack with no path, as worker-a.
    assert_eq!(
        e.ok(&main, &["ack", id, "--note", "on it", "--as", "worker-a"])
            .trim(),
        format!("{id}\tacked")
    );
    assert_eq!(e.ok(&main, &["ack", id]).trim(), format!("{id}\tunchanged"));
    let t = e.ok(&main, &["inbox", "--as", "worker-a"]);
    assert!(t.contains("no messages"), "{t}");
    assert!(e
        .ok(&main, &["inbox", "--as", "worker-a", "--all"])
        .contains("[acked]"));
    // Prune what's acked (none older than 30 days yet).
    assert_eq!(
        e.ok(&main, &["inbox", "--prune"]).trim(),
        "pruned 0 acked messages"
    );
    // Bad input.
    assert!(e
        .fails(&main, &["post", "worker a", "s"])
        .contains("isn't an address"));
    assert!(e
        .fails(&main, &["inbox", "--as", "bad name"])
        .contains("--as"));
    // Anonymous: only `all`, with a note.
    let o = e.run(&main, &["inbox"]);
    assert!(String::from_utf8_lossy(&o.stderr).contains("only messages to `all`"));
}

/// 13a step 3 (v0.17): `mnemo claim take|renew|release|check|who`, as an
/// agent given by `--as` (after the op too) or `MNEMO_AGENT`; held and
/// lost exit 1.
#[test]
fn claim_take_renew_release_check_and_who() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let out = e.ok(
        &main,
        &[
            "claim",
            "take",
            "PLAN.md tail",
            "--ttl",
            "10m",
            "--as",
            "worker-a",
            "--role",
            "worker",
        ],
    );
    let parts: Vec<&str> = out.trim().split('\t').collect();
    assert_eq!(parts[0], "claimed", "{out}");
    let token = parts[1].to_string();
    assert_eq!(token.len(), 32);
    // Another agent: held, exit 1.
    let err = e.fails(
        &main,
        &["claim", "--as", "worker-b", "take", "PLAN.md tail"],
    );
    assert!(err.contains("is held by worker-a until"), "{err}");
    // From the environment.
    let o = e
        .cmd(&main, &["claim", "check", "PLAN.md tail", &token])
        .env("MNEMO_AGENT", "worker-a")
        .output()
        .unwrap();
    assert!(o.status.success() && String::from_utf8_lossy(&o.stdout).starts_with("held until"));
    assert!(e
        .ok(
            &main,
            &["claim", "renew", "PLAN.md tail", &token, "--ttl", "90s"]
        )
        .starts_with("held until"));
    assert!(e
        .fails(&main, &["claim", "check", "PLAN.md tail", "0123"])
        .contains("lost"));
    let v: Value =
        serde_json::from_str(&e.ok(&main, &["claim", "who", "--format", "json"])).unwrap();
    let a = v
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["agent"] == "worker-a")
        .unwrap()
        .clone();
    assert_eq!(a["alive"], "unknown", "the CLI holds no presence lock: {v}");
    assert_eq!(a["role"], "worker");
    assert_eq!(a["tool"], "cli");
    assert_eq!(a["claims"], serde_json::json!(["PLAN.md tail"]));
    assert_eq!(
        e.ok(&main, &["claim", "release", "PLAN.md tail", &token])
            .trim(),
        "released"
    );
    assert!(e
        .fails(&main, &["claim", "release", "PLAN.md tail", &token])
        .contains("lost"));
    // Bad input, and no agent.
    assert!(e
        .fails(
            &main,
            &["claim", "take", "t", "--ttl", "5s", "--as", "worker-a"]
        )
        .contains("ttl"));
    assert!(e
        .fails(
            &main,
            &["claim", "take", "t", "--ttl", "soon", "--as", "worker-a"]
        )
        .contains("--ttl"));
    let o = e.run(&main, &["claim", "take", "t"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("needs an agent"));
}

/// 13a step 4 (v0.17): `mnemo task add|next|update|list|get` with `--as`,
/// `--role` and `--for`; nothing to take and lost exit 1; `export --git`
/// keeps tasks in slug order and leaves out messages, claims and presence.
#[test]
fn task_add_next_update_list_get_and_the_git_export() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    assert_eq!(
        e.ok(
            &main,
            &[
                "task",
                "add",
                "Step 4: tasks",
                "--for",
                "worker",
                "--as",
                "planner-a"
            ]
        )
        .trim(),
        "step-4-tasks\tadded\tqueued"
    );
    assert_eq!(
        e.ok(
            &main,
            &[
                "task",
                "add",
                "Alpha",
                "--for",
                "reviewer",
                "--slug",
                "a-review",
                "--as",
                "planner-a"
            ]
        )
        .trim(),
        "a-review\tadded\tqueued"
    );
    assert!(e
        .fails(&main, &["task", "add", "x", "--after", "nope"])
        .contains("no task"));
    let out = e.ok(
        &main,
        &["task", "next", "--as", "worker-a", "--role", "worker"],
    );
    let parts: Vec<&str> = out.trim().split('\t').collect();
    assert_eq!(
        (parts[0], parts[3]),
        ("step-4-tasks", "Step 4: tasks"),
        "{out}"
    );
    let token = parts[1].to_string();
    assert!(e
        .fails(
            &main,
            &["task", "next", "--as", "worker-b", "--role", "worker"]
        )
        .contains("nothing to take"));
    assert!(e
        .fails(
            &main,
            &[
                "task",
                "update",
                "step-4-tasks",
                "--status",
                "ready",
                "--as",
                "worker-b"
            ]
        )
        .contains("lost"));
    assert_eq!(
        e.ok(
            &main,
            &[
                "task",
                "update",
                "step-4-tasks",
                "--status",
                "ready",
                "--note",
                "PR 108",
                "--token",
                &token,
                "--as",
                "worker-a"
            ]
        )
        .trim(),
        "step-4-tasks\tready"
    );
    let t = e.ok(&main, &["task", "list"]);
    assert!(
        t.contains("step-4-tasks  [ready, worker-a]  Step 4: tasks  (for worker)"),
        "{t}"
    );
    let g = e.ok(&main, &["task", "get", "step-4-tasks"]);
    assert!(g.contains("active -> ready by worker-a: PR 108"), "{g}");
    let v: Value = serde_json::from_str(&e.ok(
        &main,
        &["task", "list", "--for", "reviewer", "--format", "json"],
    ))
    .unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    // The git export: tasks by slug; no message, claim or presence.
    e.ok(
        &main,
        &["post", "worker-a", "a message", "--as", "planner-a"],
    );
    e.ok(&main, &["claim", "take", "a topic", "--as", "worker-a"]);
    let x = e.ok(&main, &["export", "--git", "--format", "jsonl"]);
    let kinds: Vec<String> = x
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["metadata"]["_mnemo.kind"].as_str().map(str::to_string))
        .collect();
    assert_eq!(kinds, ["task", "task"], "{x}");
    let slugs: Vec<String> = x
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter_map(|v| v["metadata"]["_mnemo.slug"].as_str().map(str::to_string))
        .collect();
    assert_eq!(slugs, ["a-review", "step-4-tasks"]);
    assert!(!x.contains("_mnemo.delivery"), "{x}");
    // The plain export keeps the message, still without claims or presence.
    let x = e.ok(&main, &["export", "--format", "jsonl"]);
    assert!(x.contains("\"message\"") && !x.contains("\"claim\"") && !x.contains("\"presence\""));
}

/// 13a step 7 (v0.17): `--visibility` on recall, `about` and `export`
/// (`all` with a stderr line; never under `--git`); `about`'s header
/// counts kinds apart and marks a stale entry; `mnemo stale [--mine]`.
#[test]
fn visibility_freshness_and_the_about_counts_on_the_cli() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let embed = format!("{} \"{}\"", python(), e.p("embed.py").display());
    let remember = |text: &str, extra: &[&str]| -> String {
        let mut a = vec![
            "remember",
            "--text",
            text,
            "--embed-cmd",
            &embed,
            "--embed-name",
            "test-4",
        ];
        a.extend(extra);
        e.ok(&main, &a).trim().to_string()
    };
    remember("staging seven is the deploy target", &[]);
    remember("staging seven is secret", &["--visibility", "private"]);
    remember(
        "an onboarding note to review",
        &[
            "--metadata",
            r#"{"area": "onboarding"}"#,
            "--review-by",
            "2020-01-02",
        ],
    );
    // Recall: private only when asked.
    let r = e.ok(
        &main,
        &["recall", "--keywords", "staging seven", "--format", "json"],
    );
    assert!(
        r.contains("deploy target") && !r.contains("is secret"),
        "{r}"
    );
    let r = e.ok(
        &main,
        &[
            "recall",
            "--keywords",
            "staging seven",
            "--visibility",
            "all",
            "--format",
            "json",
        ],
    );
    assert!(r.contains("is secret"), "{r}");
    assert!(e
        .fails(
            &main,
            &["recall", "--keywords", "x", "--visibility", "hidden"]
        )
        .contains("visibility level"));
    // Export: left out with a line; included with `all` and a line; never
    // with --git.
    let o = e.run(&main, &["export", "--format", "jsonl"]);
    assert!(!String::from_utf8_lossy(&o.stdout).contains("is secret"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("1 private record was left out"));
    let o = e.run(
        &main,
        &["export", "--format", "jsonl", "--visibility", "all"],
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("is secret"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("includes 1 private record"));
    let o = e.run(
        &main,
        &[
            "export",
            "--git",
            "--format",
            "jsonl",
            "--visibility",
            "all",
        ],
    );
    assert!(!String::from_utf8_lossy(&o.stdout).contains("is secret"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--git` never writes them"));
    // About: the counts, and the stale mark.
    e.ok(&main, &["post", "worker-a", "hello", "--as", "planner-a"]);
    let a = e.ok(&main, &["about"]);
    assert!(
        a.contains("facts, 0 turns, 0 essence, ") && a.contains(" coordination"),
        "{a}"
    );
    assert!(a.contains("[stale since 2020-01-02]"), "{a}");
    let v: Value = serde_json::from_str(&e.ok(&main, &["about", "--format", "json"])).unwrap();
    assert!(
        v["stats"]["counts"]["coordination"].as_u64().unwrap() >= 1,
        "{v}"
    );
    // mnemo stale, and --mine.
    let s = e.ok(&main, &["stale"]);
    assert!(
        s.contains("fact  stale since 2020-01-02") && s.contains("an onboarding note to review"),
        "{s}"
    );
    assert!(e
        .ok(&main, &["stale", "--mine", "--as", "worker-a"])
        .contains("nothing stale"));
    assert!(e.fails(&main, &["stale", "--mine"]).contains("--as"));
}

/// RB-012.1 and .2 on the CLI: an update prints the claim it holds; a
/// `done` slug's re-add and a stranger's are refused with their messages.
#[test]
fn rb012_task_update_prints_its_claim_and_re_adds_are_refused() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    e.ok(
        &main,
        &[
            "task",
            "add",
            "Bump the version",
            "--for",
            "worker",
            "--as",
            "planner-a",
        ],
    );
    let out = e.ok(
        &main,
        &["task", "next", "--as", "worker-a", "--role", "worker"],
    );
    let token = out.split('\t').nth(1).unwrap().to_string();
    let up = e.ok(
        &main,
        &[
            "task",
            "update",
            "bump-the-version",
            "--note",
            "going",
            "--token",
            &token,
            "--as",
            "worker-a",
        ],
    );
    let f: Vec<&str> = up.trim().split('\t').collect();
    assert_eq!(
        (f[0], f[1], f[2]),
        ("bump-the-version", "active", token.as_str()),
        "{up}"
    );
    assert!(f[3].ends_with('Z'), "{up}");
    // A stranger's re-add.
    let err = e.fails(
        &main,
        &[
            "task",
            "add",
            "Bump the version",
            "--pre-approved",
            "--as",
            "worker-a",
        ],
    );
    assert!(
        err.contains("that slug is planner-a's task; it wasn't changed"),
        "{err}"
    );
    // Done, then re-added.
    e.ok(
        &main,
        &[
            "task",
            "update",
            "bump-the-version",
            "--status",
            "ready",
            "--as",
            "worker-a",
        ],
    );
    e.ok(
        &main,
        &[
            "task",
            "update",
            "bump-the-version",
            "--status",
            "done",
            "--as",
            "planner-a",
        ],
    );
    let err = e.fails(
        &main,
        &[
            "task",
            "add",
            "Bump the version",
            "--body",
            "again",
            "--as",
            "planner-a",
        ],
    );
    assert!(
        err.contains("bump-the-version is done; give a new slug (bump-the-version-2?)"),
        "{err}"
    );
}

/// RB-013.4: `remember --agent` is checked like every agent id, and an
/// invalid `MNEMO_AGENT` is an error for `post`, `task add` and the claim
/// writes (reads go on, with a note).
#[test]
fn rb013_agent_ids_are_checked_for_writes() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    for bad in ["two words", "a/b", &"x".repeat(65)] {
        let err = e.fails(&main, &["remember", "--text", "x", "--agent", bad]);
        assert!(
            err.contains("--agent") && err.contains("1 to 64"),
            "{bad:?}: {err}"
        );
    }
    let run = |args: &[&str]| {
        e.cmd(&main, args)
            .env("MNEMO_AGENT", "two words")
            .output()
            .unwrap()
    };
    for args in [
        &["post", "planner-a", "hello"][..],
        &["task", "add", "A task"][..],
        &["claim", "take", "a topic"][..],
    ] {
        let o = run(args);
        assert!(!o.status.success(), "{args:?} went on");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("MNEMO_AGENT isn't"),
            "{args:?}"
        );
    }
    // `--as` overrides the bad variable; a read goes on with a note.
    let o = e
        .cmd(&main, &["post", "planner-a", "hello", "--as", "worker-a"])
        .env("MNEMO_AGENT", "two words")
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let o = run(&["inbox"]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("it's ignored"));
}

/// PRESENCE-AT-START (v0.17.2): a named shared server writes its presence
/// record at start, with the lock flag, so `who` shows the session alive
/// before any tool call.
#[test]
fn a_named_shared_server_is_present_from_its_start() {
    use std::io::{BufRead, BufReader};
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let mut server = e
        .cmd(&main, &["serve", "--mcp", "--shared"])
        .env("MNEMO_AGENT", "worker-a")
        .env("MNEMO_ROLE", "worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut err = BufReader::new(server.stderr.take().unwrap());
    let mut said = String::new();
    loop {
        let mut line = String::new();
        if err.read_line(&mut line).unwrap() == 0 {
            panic!("the server stopped: {said}");
        }
        said.push_str(&line);
        if line.starts_with("mnemo mcp: ready") {
            break;
        }
    }
    assert!(
        said.contains("mnemo mcp: worker-a is present (tool mcp)"),
        "{said}"
    );
    // No tool call yet: `who` already shows it alive, through `mcp`.
    let v: Value =
        serde_json::from_str(&e.ok(&main, &["claim", "who", "--format", "json"])).unwrap();
    let a = v
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["agent"] == "worker-a")
        .unwrap_or_else(|| panic!("{v}"))
        .clone();
    assert_eq!(a["alive"], "yes", "{v}");
    assert_eq!(a["tool"], "mcp");
    assert_eq!(a["role"], "worker");
    drop(server.stdin.take());
    assert!(server.wait().unwrap().success());
    // WHO-SHAPE (v0.17.2): once it's gone, "no" (the record says it held
    // the lock, and nobody does).
    let v: Value =
        serde_json::from_str(&e.ok(&main, &["claim", "who", "--format", "json"])).unwrap();
    let a = v
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["agent"] == "worker-a")
        .unwrap()
        .clone();
    assert_eq!(a["alive"], "no", "{v}");
}

/// v0.17.2: `cache` takes no path in a git repository, like `get`.
#[test]
fn cache_takes_no_path_in_a_git_repository() {
    let e = Env::new();
    e.init();
    let deep = e.p("main/sub/deep");
    e.ok(
        &deep,
        &[
            "cache",
            "put",
            "--ns",
            "llm",
            "--key",
            "q1",
            "--value",
            "an answer",
        ],
    );
    assert_eq!(
        e.ok(&deep, &["cache", "get", "--ns", "llm", "--key", "q1"])
            .trim_end(),
        "an answer"
    );
    let stats: Value = serde_json::from_str(&e.ok(&e.p("main"), &["cache", "stats"])).unwrap();
    assert_eq!(stats["entries"], 1, "{stats}");
    e.ok(&deep, &["cache", "delete", "--ns", "llm", "--key", "q1"]);
    let o = e.run(&deep, &["cache", "get", "--ns", "llm", "--key", "q1"]);
    assert!(!o.status.success(), "a miss after the delete");
    e.ok(&deep, &["cache", "purge", "--expired"]);
}

/// v0.18.0: `mnemo finding` round trips through the CLI: add, triage,
/// verify, list (each format and filter) and get; a taken id and a refused
/// transition exit 1.
#[test]
fn findings_round_trip_through_the_cli() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    let ok_as = |who: &str, args: &[&str]| -> String {
        let mut all = args.to_vec();
        all.extend(["--as", who]);
        e.ok(&main, &all)
    };
    assert_eq!(
        ok_as(
            "reviewer-b",
            &[
                "finding",
                "add",
                "RB-014.1",
                "--category",
                "CORRECTNESS",
                "--priority",
                "HIGH",
                "--title",
                "The release race",
                "--body",
                "Why it matters.",
                "--ref",
                "a.rs:1",
                "--ref",
                "b.rs:2",
            ]
        )
        .trim(),
        "RB-014.1\topen"
    );
    ok_as(
        "reviewer-b",
        &[
            "finding",
            "add",
            "RB-014.2",
            "--category",
            "PERF",
            "--priority",
            "LOW",
            "--title",
            "Slow",
        ],
    );
    let again = [
        "finding",
        "add",
        "RB-014.1",
        "--category",
        "PERF",
        "--priority",
        "LOW",
        "--title",
        "x",
    ];
    // RB-014: a finding needs an agent.
    let err = e.fails(&main, &again);
    assert!(err.contains("needs its author"), "{err}");
    let err = e.fails(&main, &[&again[..], &["--as", "reviewer-b"]].concat());
    assert!(err.contains("RB-014.1 is taken"), "{err}");
    let t = e.ok(&main, &["finding", "list"]);
    assert!(
        t.contains("RB-014.1  [open]  CORRECTNESS HIGH  The release race")
            && t.find("RB-014.1") < t.find("RB-014.2"),
        "{t}"
    );
    assert_eq!(
        ok_as(
            "planner-a",
            &[
                "finding",
                "triage",
                "RB-014.1",
                "--status",
                "accepted",
                "--text",
                "In v0.18.0"
            ]
        )
        .trim(),
        "RB-014.1\taccepted"
    );
    let err = e.fails(&main, &["finding", "verify", "RB-014.2", "--ref", "abc"]);
    assert!(err.contains("can't go from open to verified"), "{err}");
    ok_as(
        "reviewer-b",
        &["finding", "verify", "RB-014.1", "--ref", "abc1234"],
    );
    let v: Value =
        serde_json::from_str(&e.ok(&main, &["finding", "get", "RB-014.1", "--format", "json"]))
            .unwrap();
    assert_eq!(v["status"], "verified");
    assert_eq!(v["author"], "reviewer-b");
    // RB-014: the record id, as `get <id>` takes it.
    let rid = v["record_id"].as_str().unwrap();
    assert_eq!(rid.len(), 26, "{v}");
    assert!(e.ok(&main, &["get", rid]).contains("The release race"));
    assert_eq!(v["triaged_by"], "planner-a");
    assert_eq!(v["verified_ref"], "abc1234");
    assert_eq!(v["refs"], serde_json::json!(["a.rs:1", "b.rs:2"]));
    assert_eq!(v["history"].as_array().unwrap().len(), 2);
    let get = e.ok(&main, &["finding", "get", "RB-014.1"]);
    assert!(
        get.contains("open -> accepted by planner-a: In v0.18.0"),
        "{get}"
    );
    let lines = e.ok(
        &main,
        &["finding", "list", "--status", "open", "--format", "jsonl"],
    );
    let ids: Vec<String> = lines
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(ids, ["RB-014.2"]);
    let all: Value = serde_json::from_str(&e.ok(
        &main,
        &[
            "finding",
            "list",
            "--batch",
            "RB-014",
            "--category",
            "PERF",
            "--format",
            "json",
        ],
    ))
    .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 1);
    assert!(e
        .fails(&main, &["finding", "get", "RB-1.1"])
        .contains("no finding"));
}

/// v0.18.1: `task add --ref` (repeatable); `task get` shows the refs, and
/// its JSON and `list`'s carry them.
#[test]
fn task_refs_through_the_cli() {
    let e = Env::new();
    e.init();
    let main = e.p("main");
    assert_eq!(
        e.ok(
            &main,
            &[
                "task",
                "add",
                "Fix RB-014.1",
                "--slug",
                "fix-1",
                "--ref",
                "RB-014.1",
                "--ref",
                "src/a.rs:1",
                "--as",
                "planner-a",
            ],
        )
        .trim(),
        "fix-1\tadded\tqueued"
    );
    let g = e.ok(&main, &["task", "get", "fix-1"]);
    assert!(g.contains("refs: RB-014.1, src/a.rs:1"), "{g}");
    let v: Value =
        serde_json::from_str(&e.ok(&main, &["task", "get", "fix-1", "--format", "json"])).unwrap();
    assert_eq!(v["refs"], serde_json::json!(["RB-014.1", "src/a.rs:1"]));
    let l: Value =
        serde_json::from_str(&e.ok(&main, &["task", "list", "--format", "json"])).unwrap();
    assert_eq!(l[0]["refs"][0], "RB-014.1");
    let err = e.fails(
        &main,
        &["task", "add", "Bad", "--ref", "", "--as", "planner-a"],
    );
    assert!(err.contains("refs"), "{err}");
}
