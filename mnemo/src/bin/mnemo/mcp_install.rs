//! 12.3 step 3: `mnemo mcp install --client <name> <file>` (R-007.3).
//!
//! Adds this brain's MCP server to a client's config: always
//! `serve --mcp --shared`, never a secret, and in a project scope (a file
//! that's committed) no absolute path, `--embed-cmd` or key command.
//!
//! The clients, each checked against its vendor's documentation (fetched
//! 2026-11-14):
//! - **`claude-code`** (<https://code.claude.com/docs/en/mcp>):
//!   `claude mcp add-json <name> '<json>' --scope local|project|user`;
//!   project scope is `.mcp.json` at the project root (`mcpServers`), local
//!   and user are in `~/.claude.json`. `.mcp.json` expands `${VAR:-default}`
//!   in `args`, and Claude Code sets `CLAUDE_PROJECT_DIR` to the project root
//!   in the server's environment, so a project-relative path is
//!   `${CLAUDE_PROJECT_DIR:-.}/<path>`. Without `claude` on the PATH, only
//!   the project scope can be written (the file itself).
//! - **`cursor`** (<https://cursor.com/docs/context/mcp>): `.cursor/mcp.json`
//!   in the project, `~/.cursor/mcp.json` for the user (`mcpServers`, with
//!   `type`, `command`, `args`, `env`); `${workspaceFolder}` is the project
//!   root.
//! - **`claude-desktop`**
//!   (<https://modelcontextprotocol.io/docs/develop/connect-local-servers>):
//!   one per-user file, `~/Library/Application Support/Claude/` on macOS,
//!   `%APPDATA%\Claude\` on Windows, `~/.config/Claude/` on Linux,
//!   `claude_desktop_config.json` (`mcpServers`); paths must be absolute.
//!
//! Not in the table: VS Code (two formats, and its portable project file is
//! the same `.mcp.json` Claude Code reads, which expands a different
//! variable) and Codex (TOML, which would need a new dependency to edit).

use std::path::{Path, PathBuf};

use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};
use serde_json::value::RawValue;
use serde_json::{json, Value};

/// A client `mcp install` knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Client {
    /// Claude Code.
    ClaudeCode,
    /// Cursor.
    Cursor,
    /// Claude Desktop.
    ClaudeDesktop,
}

/// Where the entry goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Scope {
    /// Committed with the project: no secret, no absolute path.
    Project,
    /// This project, for this user only (Claude Code).
    Local,
    /// Every project, for this user.
    User,
}

impl Scope {
    fn word(self) -> &'static str {
        match self {
            Scope::Project => "project",
            Scope::Local => "local",
            Scope::User => "user",
        }
    }
}

/// `mcp install`'s options.
pub struct Install {
    pub client: Client,
    pub file: String,
    pub scope: Option<Scope>,
    pub name: String,
    pub print: bool,
    pub yes: bool,
}

fn client_name(c: Client) -> &'static str {
    match c {
        Client::ClaudeCode => "claude-code",
        Client::Cursor => "cursor",
        Client::ClaudeDesktop => "claude-desktop",
    }
}

fn scopes(c: Client) -> &'static [Scope] {
    match c {
        Client::ClaudeCode => &[Scope::Project, Scope::Local, Scope::User],
        Client::Cursor => &[Scope::Project, Scope::User],
        Client::ClaudeDesktop => &[Scope::User],
    }
}

/// The git work tree `start` is in: the nearest directory up from it with a
/// `.git` (a directory, or a worktree's file).
fn git_root(start: &Path) -> Option<PathBuf> {
    let mut d = Some(start);
    while let Some(dir) = d {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        d = dir.parent();
    }
    None
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| "can't find the home directory (HOME isn't set)".to_string())
}

/// Claude Desktop's config file on this OS.
fn desktop_config() -> Result<PathBuf, String> {
    if cfg!(target_os = "macos") {
        Ok(home()?.join("Library/Application Support/Claude/claude_desktop_config.json"))
    } else if cfg!(windows) {
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| "APPDATA isn't set".to_string())?;
        Ok(appdata.join("Claude").join("claude_desktop_config.json"))
    } else {
        Ok(home()?.join(".config/Claude/claude_desktop_config.json"))
    }
}

/// An executable named `name` on the PATH.
fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|d| {
        let p = d.join(name);
        let ok = std::fs::metadata(&p).is_ok_and(|m| m.is_file());
        #[cfg(windows)]
        let ok = ok || std::fs::metadata(p.with_extension("exe")).is_ok_and(|m| m.is_file());
        ok.then_some(p)
    })
}

/// What `run` decided: the entry, and where it goes.
struct Plan {
    scope: Scope,
    entry: Value,
    /// The config file to edit, or `None` when `claude mcp add-json` does it.
    file: Option<PathBuf>,
    /// The directory `claude` runs in (the project root), for its scopes.
    claude: Option<(PathBuf, PathBuf)>,
}

fn plan(o: &Install) -> Result<Plan, String> {
    let brain = std::fs::canonicalize(&o.file)
        .map_err(|e| format!("{}: {e} (mcp install needs an existing brain)", o.file))?;
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
    let root = git_root(&cwd);
    let allowed = scopes(o.client);
    let scope = match o.scope {
        Some(s) => s,
        None => match &root {
            Some(r) if brain.starts_with(r) && allowed.contains(&Scope::Project) => Scope::Project,
            _ => Scope::User,
        },
    };
    if !allowed.contains(&scope) {
        let names: Vec<&str> = allowed.iter().map(|s| s.word()).collect();
        return Err(format!(
            "{} has no {} scope (it has: {}); nothing was written",
            client_name(o.client),
            scope.word(),
            names.join(", ")
        ));
    }
    // The command and the path. In a project scope (committed), `mnemo`
    // from the PATH and a path relative to the project root; otherwise
    // absolute, since a GUI client doesn't get the shell's PATH.
    let (command, brain_arg) = if scope == Scope::Project {
        let Some(root) = &root else {
            return Err(format!(
                "the project scope needs a git work tree, and {} isn't in one; use --scope user",
                cwd.display()
            ));
        };
        let rel = brain.strip_prefix(root).map_err(|_| {
            format!(
                "{} is outside the project ({}), so a project-scoped config can't name it; \
                 use --scope user",
                brain.display(),
                root.display()
            )
        })?;
        let rel = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        let arg = match o.client {
            Client::ClaudeCode => format!("${{CLAUDE_PROJECT_DIR:-.}}/{rel}"),
            Client::Cursor => format!("${{workspaceFolder}}/{rel}"),
            Client::ClaudeDesktop => unreachable!("no project scope"),
        };
        ("mnemo".to_string(), arg)
    } else {
        let exe = std::env::current_exe()
            .and_then(std::fs::canonicalize)
            .map_err(|e| format!("can't find this mnemo's path: {e}"))?;
        (
            exe.to_string_lossy().into_owned(),
            brain.to_string_lossy().into_owned(),
        )
    };
    let mut entry = json!({
        "command": command,
        "args": ["serve", "--mcp", "--shared", brain_arg],
    });
    if o.client != Client::ClaudeDesktop {
        entry["type"] = json!("stdio");
    }
    check_rules(&entry, scope)?;
    let (file, claude) = match (o.client, scope) {
        (Client::ClaudeCode, s) => match on_path("claude") {
            Some(c) => (None, Some((c, root.clone().unwrap_or(cwd.clone())))),
            None if s == Scope::Project => (Some(root.clone().unwrap().join(".mcp.json")), None),
            None => {
                return Err(format!(
                    "Claude Code's {} scope lives in ~/.claude.json, which only the `claude` CLI \
                     writes, and `claude` isn't on the PATH; use --scope project, or install \
                     Claude Code",
                    s.word()
                ))
            }
        },
        (Client::Cursor, Scope::Project) => (
            Some(root.clone().unwrap().join(".cursor").join("mcp.json")),
            None,
        ),
        (Client::Cursor, _) => (Some(home()?.join(".cursor").join("mcp.json")), None),
        (Client::ClaudeDesktop, _) => (Some(desktop_config()?), None),
    };
    Ok(Plan {
        scope,
        entry,
        file,
        claude,
    })
}

/// The rules for everything `mcp install` writes (12.3 step 3): always
/// `--shared`, no secret, and in a project scope no absolute path,
/// `--embed-cmd` or key command.
fn check_rules(entry: &Value, scope: Scope) -> Result<(), String> {
    let args: Vec<&str> = entry["args"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if !args.contains(&"--shared") {
        return Err("internal: an entry without --shared".into());
    }
    if entry.get("env").is_some() {
        return Err("internal: an entry with an environment".into());
    }
    if scope == Scope::Project {
        let cmd = entry["command"].as_str().unwrap_or("");
        let absolute = |s: &str| Path::new(s).is_absolute() || s.starts_with('~');
        if absolute(cmd) || args.iter().any(|a| absolute(a)) {
            return Err("internal: an absolute path in a project scope".into());
        }
        if args
            .iter()
            .any(|a| a.contains("embed-cmd") || a.contains("KEY_COMMAND"))
        {
            return Err("internal: an embedder or key command in a project scope".into());
        }
    }
    Ok(())
}

// --- An order-keeping JSON object --------------------------------------------

/// A JSON object's members in their order, each value as its original text.
struct Ordered(Vec<(String, Box<RawValue>)>);

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Ordered;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a JSON object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Ordered, A::Error> {
                let mut v = Vec::new();
                while let Some((k, val)) = m.next_entry::<String, Box<RawValue>>()? {
                    v.push((k, val));
                }
                Ok(Ordered(v))
            }
        }
        d.deserialize_map(V)
    }
}

/// Write `members` as an object at `depth` (two spaces a level); each value's
/// own text is kept as it was.
fn write_object(members: &[(String, String)], depth: usize) -> String {
    if members.is_empty() {
        return "{}".into();
    }
    let pad = "  ".repeat(depth + 1);
    let body: Vec<String> = members
        .iter()
        .map(|(k, v)| format!("{pad}{}: {v}", Value::String(k.clone())))
        .collect();
    format!("{{\n{}\n{}}}", body.join(",\n"), "  ".repeat(depth))
}

/// `entry` pretty-printed for `depth`.
fn pretty_at(entry: &Value, depth: usize) -> String {
    let text = serde_json::to_string_pretty(entry).unwrap();
    let pad = "  ".repeat(depth);
    text.lines()
        .enumerate()
        .map(|(i, l)| {
            if i == 0 {
                l.to_string()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The config's new text: `entry` under `mcpServers.<name>`, everything else
/// as it was. Returns it with the old entry, if there was one.
fn merged(old: &str, name: &str, entry: &Value) -> Result<(String, Option<Value>), String> {
    let top: Ordered = if old.trim().is_empty() {
        Ordered(Vec::new())
    } else {
        serde_json::from_str(old)
            .map_err(|e| format!("can't parse it ({e}); nothing was written"))?
    };
    let mut members: Vec<(String, String)> = Vec::new();
    let mut found_servers = false;
    let mut previous = None;
    for (k, v) in top.0 {
        if k == "mcpServers" {
            found_servers = true;
            let servers: Ordered = serde_json::from_str(v.get()).map_err(|e| {
                format!("its mcpServers isn't an object ({e}); nothing was written")
            })?;
            let mut inner: Vec<(String, String)> = Vec::new();
            let mut replaced = false;
            for (sk, sv) in servers.0 {
                if sk == name {
                    previous = serde_json::from_str(sv.get()).ok();
                    inner.push((sk, pretty_at(entry, 2)));
                    replaced = true;
                } else {
                    inner.push((sk, sv.get().to_string()));
                }
            }
            if !replaced {
                inner.push((name.to_string(), pretty_at(entry, 2)));
            }
            members.push((k, write_object(&inner, 1)));
        } else {
            members.push((k, v.get().to_string()));
        }
    }
    if !found_servers {
        let inner = vec![(name.to_string(), pretty_at(entry, 2))];
        members.push(("mcpServers".into(), write_object(&inner, 1)));
    }
    let text = format!("{}\n", write_object(&members, 0));
    serde_json::from_str::<Value>(&text)
        .map_err(|e| format!("internal: the merge isn't JSON ({e})"))?;
    Ok((text, previous))
}

// --- Showing it ---------------------------------------------------------------

/// `v` with every value under an `env` or `headers` key masked, for display.
fn masked(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter()
                .map(|(k, val)| {
                    let val = if (k == "env" || k == "headers") && val.is_object() {
                        Value::Object(
                            val.as_object()
                                .unwrap()
                                .keys()
                                .map(|kk| (kk.clone(), json!("••••••")))
                                .collect(),
                        )
                    } else {
                        masked(val)
                    };
                    (k.clone(), val)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(masked).collect()),
        other => other.clone(),
    }
}

/// A line diff of `a` against `b` (`-` removed, `+` added, two spaces kept).
fn diff(a: &str, b: &str) -> String {
    let (a, b): (Vec<&str>, Vec<&str>) = (a.lines().collect(), b.lines().collect());
    let (n, m) = (a.len(), b.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut out) = (0, 0, String::new());
    while i < n || j < m {
        if i < n && j < m && a[i] == b[j] {
            out.push_str(&format!("  {}\n", a[i]));
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push_str(&format!("+ {}\n", b[j]));
            j += 1;
        } else {
            out.push_str(&format!("- {}\n", a[i]));
            i += 1;
        }
    }
    out
}

/// The masked, pretty form of a config's text, for the diff.
fn shown(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    serde_json::from_str::<Value>(text)
        .map(|v| serde_json::to_string_pretty(&masked(&v)).unwrap())
        .unwrap_or_default()
}

/// Ask `question` at the terminal; `true` for yes.
fn confirm(question: &str) -> Result<bool, String> {
    use std::io::{BufRead, IsTerminal, Write};
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return Err(
            "not at a terminal, so nothing was written; pass --yes to write without \
                    asking, or --print to see it"
                .into(),
        );
    }
    print!("{question} [y/N] ");
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    Ok(matches!(line.trim(), "y" | "Y" | "yes"))
}

/// Write `text` to `path` atomically: a new temp file beside it, synced,
/// given the old file's mode, then renamed over it.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let dir = path.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.mnemo-{}.tmp",
        path.file_name().unwrap().to_string_lossy(),
        std::process::id()
    ));
    let result = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        if let Ok(old) = std::fs::metadata(path) {
            std::fs::set_permissions(&tmp, old.permissions())?;
        }
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("writing {}: {e}", path.display())
    })
}

/// What the server will need for this brain's secret, said once.
fn secret_note(brain: &str) -> Option<String> {
    use mnemo_engine::keys::{self, SecretKind};
    let info = keys::file_info(brain).ok()?;
    match info.kind {
        SecretKind::Passphrase => Some(
            "this brain uses a passphrase: the server will need MNEMO_PASSPHRASE in its \
             environment, or the passphrase in the keychain (nothing secret is written to the \
             config)"
                .into(),
        ),
        SecretKind::Key => {
            let stored = keys::default_store()
                .and_then(|s| s.get(&info.salt).ok().flatten())
                .is_some();
            (!stored).then(|| {
                "this brain's key isn't in this machine's keychain: run `mnemo unlock` first, or \
                 give the server MNEMO_KEY or MNEMO_KEY_FILE"
                    .into()
            })
        }
    }
}

/// Offer to move a `MNEMO_PASSPHRASE` the old entry carried into the
/// keychain, at a terminal. `--yes` never moves one. Returns whether the
/// entry can now be written without it.
fn move_passphrase(previous: &Option<Value>, brain: &str, yes: bool) -> Result<(), String> {
    use mnemo_engine::keys::{self, StoredSecret};
    let Some(pp) = previous
        .as_ref()
        .and_then(|p| p.pointer("/env/MNEMO_PASSPHRASE"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    let pp = zeroize::Zeroizing::new(pp.to_string());
    if yes {
        return Err(
            "the entry being replaced holds MNEMO_PASSPHRASE, and --yes never moves it: \
                    run this at a terminal to move it into the keychain, or remove it from the \
                    config first; nothing was written"
                .into(),
        );
    }
    if !confirm("The entry being replaced holds MNEMO_PASSPHRASE. Move it into the keychain?")? {
        return Err("left as it is; nothing was written (the new entry carries no secret)".into());
    }
    let store =
        keys::default_store().ok_or("no keychain is available here; nothing was written")?;
    store.check_available().map_err(|e| e.to_string())?;
    // It must open the brain before it's kept.
    mnemo_engine::Mnemo::open_read_only(brain, &pp)
        .map_err(|e| format!("that passphrase doesn't open {brain} ({e}); nothing was written"))?;
    let info = keys::file_info(brain).map_err(|e| e.to_string())?;
    keys::store_and_check(&*store, &info.salt, &StoredSecret::Passphrase(pp), brain)
        .map_err(|e| e.to_string())?;
    println!("moved MNEMO_PASSPHRASE into {}", store.describe());
    Ok(())
}

/// Run `mcp install`.
pub fn run(o: Install) -> Result<(), String> {
    let p = plan(&o)?;
    let client = client_name(o.client);
    if let Some((claude, dir)) = &p.claude {
        let json = serde_json::to_string(&p.entry).unwrap();
        println!(
            "{client} ({} scope): claude mcp add-json {} '{json}' --scope {}",
            p.scope.word(),
            o.name,
            p.scope.word()
        );
        if o.print {
            return Ok(());
        }
        if !o.yes && !confirm("Run it?")? {
            return Err("nothing was written".into());
        }
        let out = std::process::Command::new(claude)
            .args(["mcp", "add-json", &o.name, &json, "--scope", p.scope.word()])
            .current_dir(dir)
            .output()
            .map_err(|e| format!("running claude: {e}"))?;
        if !out.status.success() {
            return Err(format!(
                "claude mcp add-json failed: {} (if a server named {} is already there, remove \
                 it with `claude mcp remove {} --scope {}`, then run this again)",
                String::from_utf8_lossy(&out.stderr).trim(),
                o.name,
                o.name,
                p.scope.word()
            ));
        }
    } else {
        let path = p.file.clone().unwrap();
        let old = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        let (new, previous) =
            merged(&old, &o.name, &p.entry).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("{client} ({} scope): {}", p.scope.word(), path.display());
        print!("{}", diff(&shown(&old), &shown(&new)));
        if o.print {
            return Ok(());
        }
        move_passphrase(&previous, &o.file, o.yes)?;
        if previous.is_some()
            && !o.yes
            && !confirm(&format!("Replace the server named {}?", o.name))?
        {
            return Err("nothing was written".into());
        }
        if previous.is_none() && !o.yes && !confirm("Write it?")? {
            return Err("nothing was written".into());
        }
        write_atomically(&path, &new)?;
        println!("wrote {}", path.display());
    }
    if let Some(note) = secret_note(&o.file) {
        eprintln!("note: {note}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_merge_keeps_other_servers_and_their_text() {
        let old = "{\n  \"theme\": \"dark\",\n  \"mcpServers\": {\n    \"other\": {\"command\": \"x\",   \"args\": []}\n  }\n}\n";
        let entry = json!({"command": "mnemo", "args": ["serve", "--mcp", "--shared", "b"]});
        let (new, prev) = merged(old, "mnemo", &entry).unwrap();
        assert!(prev.is_none());
        assert!(
            new.contains("{\"command\": \"x\",   \"args\": []}"),
            "{new}"
        );
        assert!(new.find("theme").unwrap() < new.find("mcpServers").unwrap());
        let v: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(v["mcpServers"]["mnemo"]["args"][2], "--shared");
    }

    #[test]
    fn env_and_headers_are_masked_for_display() {
        let v = json!({"mcpServers": {"a": {"env": {"MNEMO_PASSPHRASE": "hunter2"}, "headers": {"Authorization": "Bearer t"}}}});
        let s = serde_json::to_string(&masked(&v)).unwrap();
        assert!(!s.contains("hunter2") && !s.contains("Bearer"), "{s}");
        assert!(s.contains("MNEMO_PASSPHRASE"));
    }
}
