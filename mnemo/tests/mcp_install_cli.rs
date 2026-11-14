//! 12.3 step 3: `mnemo mcp install`. Every test runs the CLI with a
//! temporary `HOME`, `APPDATA` and `PATH` (holding at most a fake `claude`),
//! so nothing here reads or writes this machine's real client configs or
//! runs the real `claude` CLI.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mnemo_engine::{KdfParams, Mnemo, MnemoConfig};
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
];

struct Env {
    dir: TempDir,
}

impl Env {
    fn new() -> Env {
        let dir = tempfile::tempdir().unwrap();
        for d in ["home", "appdata", "bin", "project/.git", "store"] {
            std::fs::create_dir_all(dir.path().join(d)).unwrap();
        }
        Env { dir }
    }
    fn p(&self, rel: &str) -> PathBuf {
        self.dir.path().join(rel)
    }
    /// A passphrase brain at `rel`.
    fn brain(&self, rel: &str) -> PathBuf {
        let p = self.p(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let cfg = MnemoConfig {
            dimensions: 2,
            kdf: KdfParams::fast(),
            ..Default::default()
        };
        Mnemo::create(&p, "pw", cfg).unwrap().close().unwrap();
        p
    }
    /// A fake `claude` on the test's PATH that records its arguments.
    #[cfg(unix)]
    fn fake_claude(&self) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let script = self.p("bin/claude");
        let log = self.p("claude.log");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" > '{}'\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        log
    }
    fn run(&self, args: &[&str]) -> (bool, String, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
        c.arg("mcp").arg("install").args(args);
        for v in CLEARED {
            c.env_remove(v);
        }
        c.env("HOME", self.p("home"))
            .env("USERPROFILE", self.p("home"))
            .env("APPDATA", self.p("appdata"))
            .env("PATH", self.p("bin"))
            .env("MNEMO_DEV_NO_KEYCHAIN", "1")
            .current_dir(self.p("project"))
            .stdin(Stdio::null());
        let o = c.output().unwrap();
        (
            o.status.success(),
            String::from_utf8_lossy(&o.stdout).into_owned(),
            String::from_utf8_lossy(&o.stderr).into_owned(),
        )
    }
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

/// The three rules, on an entry in a project-scoped file.
fn project_rules(e: &Value) {
    let text = e.to_string();
    assert_eq!(e["command"], "mnemo", "{text}");
    let args: Vec<&str> = e["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    assert_eq!(&args[..3], &["serve", "--mcp", "--shared"], "{text}");
    assert!(e.get("env").is_none(), "no secret: {text}");
    for a in &args {
        assert!(!Path::new(a).is_absolute(), "an absolute path: {text}");
        assert!(!a.contains("embed-cmd"), "{text}");
    }
}

#[test]
fn cursor_project_scope_writes_a_relative_entry() {
    let e = Env::new();
    let b = e.brain("project/brains/b.mnemo");
    let (ok, out, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(ok, "{err}");
    assert!(out.contains("project scope"), "{out}");
    let cfg = read_json(&e.p("project/.cursor/mcp.json"));
    let entry = &cfg["mcpServers"]["mnemo"];
    project_rules(entry);
    assert_eq!(entry["args"][3], "${workspaceFolder}/brains/b.mnemo");
    assert_eq!(entry["type"], "stdio");
    assert!(
        err.contains("MNEMO_PASSPHRASE"),
        "the passphrase brain's note: {err}"
    );
}

#[test]
fn cursor_user_scope_writes_absolute_paths_in_home() {
    let e = Env::new();
    let b = e.brain("elsewhere/b.mnemo");
    let (ok, _, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(ok, "{err}");
    let cfg = read_json(&e.p("home/.cursor/mcp.json"));
    let entry = &cfg["mcpServers"]["mnemo"];
    assert!(Path::new(entry["command"].as_str().unwrap()).is_absolute());
    let arg = entry["args"][3].as_str().unwrap();
    assert!(Path::new(arg).is_absolute(), "{arg}");
    assert_eq!(entry["args"][2], "--shared");
    assert!(entry.get("env").is_none());
}

#[test]
fn claude_code_without_claude_writes_mcp_json_with_the_project_variable() {
    let e = Env::new();
    let b = e.brain("project/b.mnemo");
    let (ok, _, err) = e.run(&["--client", "claude-code", "--yes", s(&b)]);
    assert!(ok, "{err}");
    let cfg = read_json(&e.p("project/.mcp.json"));
    let entry = &cfg["mcpServers"]["mnemo"];
    project_rules(entry);
    assert_eq!(entry["args"][3], "${CLAUDE_PROJECT_DIR:-.}/b.mnemo");
    // Local and user scopes need the `claude` CLI.
    let (ok, _, err) = e.run(&[
        "--client",
        "claude-code",
        "--scope",
        "local",
        "--yes",
        s(&b),
    ]);
    assert!(!ok && err.contains("`claude` isn't on the PATH"), "{err}");
}

#[cfg(unix)]
#[test]
fn claude_code_with_claude_runs_add_json() {
    let e = Env::new();
    let log = e.fake_claude();
    let b = e.brain("project/b.mnemo");
    let (ok, out, err) = e.run(&[
        "--client",
        "claude-code",
        "--scope",
        "local",
        "--name",
        "brain",
        "--yes",
        s(&b),
    ]);
    assert!(ok, "{err}");
    assert!(out.contains("claude mcp add-json brain"), "{out}");
    let lines: Vec<String> = std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(&lines[1..4], &["mcp", "add-json", "brain"]);
    let entry: Value = serde_json::from_str(&lines[4]).unwrap();
    assert_eq!(entry["args"][2], "--shared");
    assert!(entry.get("env").is_none());
    assert_eq!(&lines[5..7], &["--scope", "local"]);
    // It ran in the project root, and wrote no file itself.
    assert!(lines[0].ends_with("project"), "{}", lines[0]);
    assert!(!e.p("project/.mcp.json").exists());
}

#[test]
fn claude_desktop_has_only_the_user_scope() {
    let e = Env::new();
    let b = e.brain("project/b.mnemo");
    // Inside a git work tree the default would be project, which Claude
    // Desktop doesn't have: it uses user.
    let (ok, out, err) = e.run(&["--client", "claude-desktop", "--yes", s(&b)]);
    assert!(ok, "{err}");
    assert!(out.contains("user scope"), "{out}");
    let path = if cfg!(target_os = "macos") {
        e.p("home/Library/Application Support/Claude/claude_desktop_config.json")
    } else if cfg!(windows) {
        e.p("appdata/Claude/claude_desktop_config.json")
    } else {
        e.p("home/.config/Claude/claude_desktop_config.json")
    };
    let entry = read_json(&path)["mcpServers"]["mnemo"].clone();
    assert!(entry.get("type").is_none());
    assert!(Path::new(entry["args"][3].as_str().unwrap()).is_absolute());
    let (ok, _, err) = e.run(&["--client", "claude-desktop", "--scope", "project", s(&b)]);
    assert!(!ok && err.contains("has no project scope"), "{err}");
    let (ok, _, err) = e.run(&["--client", "cursor", "--scope", "local", s(&b)]);
    assert!(!ok && err.contains("has no local scope"), "{err}");
}

#[test]
fn a_merge_keeps_other_servers_and_a_parse_error_writes_nothing() {
    let e = Env::new();
    let b = e.brain("project/b.mnemo");
    let cfg = e.p("project/.cursor/mcp.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    let other = r#"{"command": "other-server",   "args": ["--x"]}"#;
    std::fs::write(
        &cfg,
        format!("{{\n  \"note\": 1,\n  \"mcpServers\": {{\n    \"other\": {other}\n  }}\n}}\n"),
    )
    .unwrap();
    let (ok, _, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(ok, "{err}");
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(
        text.contains(other),
        "the other server's text is kept: {text}"
    );
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["note"], 1);
    assert_eq!(v["mcpServers"]["mnemo"]["args"][2], "--shared");

    std::fs::write(&cfg, "{ not json").unwrap();
    let (ok, _, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(!ok && err.contains("can't parse it"), "{err}");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "{ not json");
}

#[test]
fn print_writes_nothing_and_env_values_are_masked() {
    let e = Env::new();
    let b = e.brain("project/b.mnemo");
    let cfg = e.p("project/.cursor/mcp.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    let old = r#"{"mcpServers": {"mnemo": {"command": "mnemo", "args": ["serve"], "env": {"MNEMO_PASSPHRASE": "hunter2", "OTHER": "s3cret"}}}}"#;
    std::fs::write(&cfg, old).unwrap();
    let (ok, out, err) = e.run(&["--client", "cursor", "--print", s(&b)]);
    assert!(ok, "{err}");
    assert!(out.contains("--shared"), "{out}");
    assert!(out.contains("MNEMO_PASSPHRASE"), "{out}");
    assert!(!out.contains("hunter2") && !out.contains("s3cret"), "{out}");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), old, "--print wrote");

    // --yes never moves (or drops) a MNEMO_PASSPHRASE.
    let (ok, _, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(!ok && err.contains("--yes never moves it"), "{err}");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), old);
}

#[test]
fn an_entry_without_a_secret_is_replaced_and_no_terminal_means_no_write() {
    let e = Env::new();
    let b = e.brain("project/b.mnemo");
    let cfg = e.p("project/.cursor/mcp.json");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(&cfg, r#"{"mcpServers": {"mnemo": {"command": "old"}}}"#).unwrap();
    // Not at a terminal, without --yes: nothing written.
    let (ok, _, err) = e.run(&["--client", "cursor", s(&b)]);
    assert!(!ok && err.contains("not at a terminal"), "{err}");
    assert!(std::fs::read_to_string(&cfg).unwrap().contains("\"old\""));
    let (ok, _, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(ok, "{err}");
    project_rules(&read_json(&cfg)["mcpServers"]["mnemo"]);
}

#[test]
fn a_project_scope_refuses_a_brain_outside_the_project() {
    let e = Env::new();
    let b = e.brain("elsewhere/b.mnemo");
    let (ok, _, err) = e.run(&["--client", "cursor", "--scope", "project", "--yes", s(&b)]);
    assert!(!ok && err.contains("outside the project"), "{err}");
    assert!(!e.p("project/.cursor/mcp.json").exists());
}

#[test]
fn a_key_brain_not_in_the_keychain_says_so() {
    let e = Env::new();
    let b = e.p("project/k.mnemo");
    let key = Mnemo::generate_key();
    let cfg = MnemoConfig {
        dimensions: 2,
        ..Default::default()
    };
    Mnemo::create_with_key(&b, &key, cfg)
        .unwrap()
        .close()
        .unwrap();
    let (ok, _, err) = e.run(&["--client", "cursor", "--yes", s(&b)]);
    assert!(ok, "{err}");
    assert!(err.contains("mnemo unlock"), "{err}");
}

/// Start `serve --mcp <arg>` in `cwd` with `project` as `CLAUDE_PROJECT_DIR`
/// (when given), call `list`, and return the response and stderr.
fn serve_list(e: &Env, arg: &str, cwd: &Path, project: Option<&Path>) -> (Value, String) {
    use std::io::{BufRead, BufReader, Write};
    let mut c = Command::new(env!("CARGO_BIN_EXE_mnemo"));
    c.args(["serve", "--mcp", arg]);
    for v in CLEARED {
        c.env_remove(v);
    }
    c.env_remove("CLAUDE_PROJECT_DIR");
    if let Some(p) = project {
        c.env("CLAUDE_PROJECT_DIR", p);
    }
    c.env("HOME", e.p("home"))
        .env("MNEMO_PASSPHRASE", "pw")
        .env("MNEMO_DEV_NO_KEYCHAIN", "1")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut call = |line: &str| -> Value {
        if writeln!(stdin, "{line}")
            .and_then(|_| stdin.flush())
            .is_err()
        {
            return Value::Null;
        }
        match stdout.next() {
            Some(Ok(l)) => serde_json::from_str(&l).unwrap(),
            _ => Value::Null,
        }
    };
    call(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{}}}"#,
    );
    let r = call(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list","arguments":{}}}"#,
    );
    drop(stdin);
    let _ = child.wait();
    let mut err = String::new();
    std::io::Read::read_to_string(&mut child.stderr.take().unwrap(), &mut err).unwrap();
    (r, err)
}

/// What `mcp install` writes for Claude Code reaches the server as
/// `./<path>` (Claude Code doesn't expand `${CLAUDE_PROJECT_DIR}` itself);
/// the server resolves it against `CLAUDE_PROJECT_DIR`, from any working
/// directory.
#[test]
fn serve_resolves_a_relative_path_against_claude_project_dir() {
    let e = Env::new();
    e.brain("project/b.mnemo");
    let sub = e.p("project/deep/sub");
    std::fs::create_dir_all(&sub).unwrap();
    let project = std::fs::canonicalize(e.p("project")).unwrap();
    let (r, err) = serve_list(&e, "./b.mnemo", &sub, Some(&project));
    assert!(r["result"].is_object(), "{r} {err}");
    assert!(err.contains("relative to CLAUDE_PROJECT_DIR"), "{err}");
    assert!(!sub.join("b.mnemo").exists());
}

/// Without `CLAUDE_PROJECT_DIR`, the working directory, as before.
#[test]
fn serve_without_claude_project_dir_uses_the_working_directory() {
    let e = Env::new();
    e.brain("project/b.mnemo");
    let (r, err) = serve_list(&e, "./b.mnemo", &e.p("project"), None);
    assert!(r["result"].is_object(), "{r} {err}");
    assert!(!err.contains("CLAUDE_PROJECT_DIR"), "{err}");
}

/// A missing brain is an error, and the server never makes one: a wrong
/// directory can't produce a new, empty brain.
#[test]
fn serve_on_a_missing_brain_fails_and_creates_nothing() {
    let e = Env::new();
    let sub = e.p("project/sub");
    std::fs::create_dir_all(&sub).unwrap();
    let (r, err) = serve_list(&e, "./b.mnemo", &sub, None);
    assert!(r.is_null() || r["result"].is_null(), "{r}");
    assert!(err.contains("never creates one"), "{err}");
    assert!(!sub.join("b.mnemo").exists());
    let project = std::fs::canonicalize(e.p("project")).unwrap();
    let (_, err) = serve_list(&e, "./b.mnemo", &sub, Some(&project));
    assert!(err.contains("never creates one"), "{err}");
    assert!(!e.p("project/b.mnemo").exists());
}
