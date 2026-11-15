//! 12.2 step 5: `mnemo onboard`, one command that sets up a project's memory
//! for Claude Code and Cursor, safe to run again
//! (`docs/design/12.2-onboard.md`, sections 2 and 4).
//!
//! It finds or creates the project brain (`<git-common-dir>/mnemo/
//! project.mnemo`) with a generated key in the keychain, installs the
//! built-in model, and wires each client: the MCP server, the hooks, and a
//! skill or rule file with the guide's card. It shows the whole plan first
//! (only its own entries, masked, as RB-010.2 asks) and asks once; `--yes`
//! writes without asking, `--print` only shows, and with no terminal and no
//! `--yes` it shows the plan, writes nothing, and exits 3. A re-run writes
//! only what's missing or out of date. `--undo` removes what it wrote,
//! never the brain, its key or the model.
//!
//! The scopes:
//! - **`local`** (the default): Claude Code's local scope through `claude
//!   mcp add-json`, hooks in `.claude/settings.local.json`, and Cursor's
//!   project files, all listed in `.git/info/exclude`. Commands are the
//!   running binary's absolute path, since a client started from the GUI
//!   may not have `~/.local/bin` on its PATH.
//! - **`project`**: `.mcp.json`, `.claude/settings.json` and Cursor's files,
//!   meant to be committed, with bare `mnemo` commands.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::hook::Client;
use super::mcp_install as mi;
use super::project;

/// Where `onboard` writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Scope {
    /// For this user only (Claude Code's local scope, files left out of git).
    Local,
    /// Committed with the project, for a team where everyone runs onboard.
    Project,
}

impl Scope {
    fn word(self) -> &'static str {
        match self {
            Scope::Local => "local",
            Scope::Project => "project",
        }
    }
}

/// `onboard`'s options.
pub struct Opts {
    pub clients: Vec<Client>,
    pub scope: Scope,
    pub hook_budget: Option<usize>,
    pub yes: bool,
    pub print: bool,
    pub no_model: bool,
    pub undo: bool,
    pub plugin: bool,
    pub moved: bool,
}

/// The MCP server's name in every client.
const NAME: &str = "mnemo";
/// The plugin and its marketplace (`.claude-plugin/marketplace.json`).
const PLUGIN: &str = "mnemo@mnemo-engine";
const MARKETPLACE: &str = "toarchkumar/mnemo";
/// The hook timeouts written into the configs, in seconds.
const TIMEOUTS: [(&str, &str, u64); 3] = [
    ("session-start", "SessionStart", 15),
    ("pre-compact", "PreCompact", 30),
    ("session-end", "SessionEnd", 30),
];
/// Cursor's names for the same events.
const CURSOR_EVENTS: [(&str, &str); 3] = [
    ("session-start", "sessionStart"),
    ("pre-compact", "preCompact"),
    ("session-end", "sessionEnd"),
];

/// What a run knows about the project.
struct Ctx {
    root: PathBuf,
    common: PathBuf,
    brain: PathBuf,
    exe: String,
    home: PathBuf,
    scope: Scope,
    budget: Option<usize>,
    plugin: bool,
}

/// One thing the plan does (or found already right).
enum Item {
    /// Create the brain with a generated key.
    CreateBrain,
    /// The brain is there, and its key opens it.
    BrainOk,
    /// Install the built-in model (only a build with `embed-candle` has one).
    #[cfg_attr(not(feature = "embed-candle"), allow(dead_code))]
    InstallModel(String),
    /// A note in the plan (already right, skipped, left alone).
    Note(String),
    /// Write (or create) a file.
    Write {
        path: PathBuf,
        old: Option<String>,
        new: String,
        shown: String,
    },
    /// Delete a file `onboard` wrote.
    Remove { path: PathBuf },
    /// Run the `claude` CLI in the project's root.
    Claude { args: Vec<String>, what: String },
}

impl Item {
    fn is_change(&self) -> bool {
        match self {
            Item::Write { old, new, .. } => old.as_deref() != Some(new.as_str()),
            Item::CreateBrain
            | Item::InstallModel(_)
            | Item::Remove { .. }
            | Item::Claude { .. } => true,
            Item::BrainOk | Item::Note(_) => false,
        }
    }
}

fn rel(ctx: &Ctx, p: &Path) -> String {
    p.strip_prefix(&ctx.root)
        .map(|r| r.display().to_string())
        .unwrap_or_else(|_| p.display().to_string())
}

/// A command word, quoted when it has a space.
fn quote(s: &str) -> String {
    if s.contains(' ') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// The command a config runs: the binary's absolute path (local), or
/// `mnemo` from the PATH (project).
fn command(ctx: &Ctx) -> String {
    match ctx.scope {
        Scope::Local => ctx.exe.clone(),
        Scope::Project => "mnemo".into(),
    }
}

/// The hook command for `event`.
fn hook_command(ctx: &Ctx, client: Client, event: &str) -> String {
    let mut c = format!(
        "{} hook {event} --client {}",
        quote(&command(ctx)),
        match client {
            Client::ClaudeCode => "claude-code",
            Client::Cursor => "cursor",
        }
    );
    if event == "session-start" {
        if let Some(b) = ctx.budget {
            c.push_str(&format!(" --budget {b}"));
        }
    }
    c
}

/// The first word of a command line, unquoted.
fn first_word(cmd: &str) -> (&str, &str) {
    let cmd = cmd.trim_start();
    if let Some(rest) = cmd.strip_prefix('"') {
        match rest.find('"') {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (rest, ""),
        }
    } else {
        match cmd.find(' ') {
            Some(i) => (&cmd[..i], &cmd[i..]),
            None => (cmd, ""),
        }
    }
}

/// Whether a path's file name is `mnemo` (or `mnemo.exe`).
fn is_mnemo(program: &str) -> bool {
    // Both separators: a config written on Windows is read anywhere.
    let name = program.rsplit(['/', '\\']).next().unwrap_or("");
    name == "mnemo" || name.eq_ignore_ascii_case("mnemo.exe")
}

/// Whether a hook command is one `onboard` writes: `mnemo` (any directory)
/// then `hook`.
pub fn is_our_hook(cmd: &str) -> bool {
    let (program, rest) = first_word(cmd);
    is_mnemo(program) && rest.trim_start().starts_with("hook ")
}

/// Whether an MCP entry is one `onboard` (or `mcp install`) writes: the
/// command is `mnemo` (any directory) and `serve` is in the args.
pub fn is_our_server(entry: &Value) -> bool {
    let program = entry.get("command").and_then(Value::as_str).unwrap_or("");
    let serve = entry
        .get("args")
        .and_then(Value::as_array)
        .is_some_and(|a| a.iter().any(|x| x.as_str() == Some("serve")));
    is_mnemo(program) && serve
}

// --- JSON edits ----------------------------------------------------------------

/// The top-level members of a JSON object's text (an empty text is `{}`).
fn members(old: &str) -> Result<Vec<(String, String)>, String> {
    if old.trim().is_empty() {
        return Ok(Vec::new());
    }
    let top: mi::Ordered =
        serde_json::from_str(old).map_err(|e| format!("can't parse it ({e}); left alone"))?;
    Ok(top
        .0
        .into_iter()
        .map(|(k, v)| (k, v.get().to_string()))
        .collect())
}

/// The text with `key`'s member set to `value` (added last if new), or
/// removed with `None`; every other member's text as it was.
fn set_member(old: &str, key: &str, value: Option<&Value>) -> Result<String, String> {
    let mut m = members(old)?;
    match (m.iter().position(|(k, _)| k == key), value) {
        (Some(i), Some(v)) => m[i].1 = mi::pretty_at(v, 1),
        (Some(i), None) => {
            m.remove(i);
        }
        (None, Some(v)) => m.push((key.to_string(), mi::pretty_at(v, 1))),
        (None, None) => {}
    }
    Ok(format!("{}\n", mi::write_object(&m, 0)))
}

/// A member's value.
fn member(old: &str, key: &str) -> Result<Option<Value>, String> {
    Ok(members(old)?
        .into_iter()
        .find(|(k, _)| k == key)
        .and_then(|(_, v)| serde_json::from_str(&v).ok()))
}

/// Claude Code's `hooks` object with our handlers removed, then (when
/// `add`) ours appended, one group per event.
fn claude_hooks(hooks: Option<Value>, add: &[(String, String, u64)]) -> Value {
    let mut obj = match hooks {
        Some(Value::Object(o)) => o,
        _ => serde_json::Map::new(),
    };
    for (_, arr) in obj.iter_mut() {
        if let Some(groups) = arr.as_array_mut() {
            for g in groups.iter_mut() {
                if let Some(hs) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                    hs.retain(|h| {
                        !h.get("command")
                            .and_then(Value::as_str)
                            .is_some_and(is_our_hook)
                    });
                }
            }
            groups.retain(|g| {
                g.get("hooks")
                    .and_then(Value::as_array)
                    .is_none_or(|h| !h.is_empty())
            });
        }
    }
    obj.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    for (event, cmd, timeout) in add {
        let group = json!({"hooks": [{"type": "command", "command": cmd, "timeout": timeout}]});
        if let Some(a) = obj
            .entry(event.clone())
            .or_insert_with(|| json!([]))
            .as_array_mut()
        {
            a.push(group);
        }
    }
    Value::Object(obj)
}

/// Cursor's `hooks` object, the same way (its entries sit in the event
/// arrays directly).
fn cursor_hooks(hooks: Option<Value>, add: &[(String, String, u64)]) -> Value {
    let mut obj = match hooks {
        Some(Value::Object(o)) => o,
        _ => serde_json::Map::new(),
    };
    for (_, arr) in obj.iter_mut() {
        if let Some(a) = arr.as_array_mut() {
            a.retain(|h| {
                !h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_our_hook)
            });
        }
    }
    obj.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    for (event, cmd, timeout) in add {
        if let Some(a) = obj
            .entry(event.clone())
            .or_insert_with(|| json!([]))
            .as_array_mut()
        {
            a.push(json!({"command": cmd, "timeout": timeout}));
        }
    }
    Value::Object(obj)
}

/// What a plan shows for a hooks file: only our handlers, and how many
/// others it keeps.
fn shown_hooks(old_hooks: &Option<Value>, ours: &[(String, String, u64)]) -> String {
    let mut out = String::new();
    for (event, cmd, timeout) in ours {
        out.push_str(&format!("+ {event}: {cmd} (timeout {timeout} s)\n"));
    }
    let count = |v: &Value| -> usize {
        v.as_object()
            .map(|o| {
                o.values()
                    .filter_map(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .map(|g| match g.get("hooks").and_then(Value::as_array) {
                                Some(hs) => hs
                                    .iter()
                                    .filter(|h| {
                                        !h.get("command")
                                            .and_then(Value::as_str)
                                            .is_some_and(is_our_hook)
                                    })
                                    .count(),
                                None => usize::from(
                                    !g.get("command")
                                        .and_then(Value::as_str)
                                        .is_some_and(is_our_hook),
                                ),
                            })
                            .sum::<usize>()
                    })
                    .sum()
            })
            .unwrap_or(0)
    };
    let others = old_hooks.as_ref().map_or(0, count);
    out.push_str(&format!(
        "({others} other hook{} in the file left unchanged)\n",
        if others == 1 { "" } else { "s" }
    ));
    out
}

/// The text with server `name` removed from `mcpServers`.
fn without_server(old: &str, name: &str) -> Result<String, String> {
    let Some(Value::Object(mut servers)) = member(old, "mcpServers")? else {
        return Ok(old.to_string());
    };
    servers.remove(name);
    // The last server gone: the member goes too, so a file onboard made
    // is left empty, and removed.
    let v = Value::Object(servers);
    let empty = v.as_object().is_some_and(|o| o.is_empty());
    set_member(old, "mcpServers", (!empty).then_some(&v))
}

fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

// --- The plan --------------------------------------------------------------------

/// The MCP entry for a client.
fn server_entry(ctx: &Ctx, client: Client) -> Value {
    let mut args: Vec<String> = Vec::new();
    if client == Client::Cursor {
        args.extend(["-C".to_string(), "${workspaceFolder}".to_string()]);
    }
    args.extend(["serve", "--mcp", "--shared"].map(String::from));
    json!({"type": "stdio", "command": command(ctx), "args": args})
}

/// `~/.claude.json`'s local-scope `mnemo` entry for this project.
fn claude_local_entry(ctx: &Ctx) -> Option<Value> {
    let text = std::fs::read_to_string(ctx.home.join(".claude.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let key = ctx.root.to_string_lossy();
    v.get("projects")?
        .get(key.as_ref())?
        .get("mcpServers")?
        .get(NAME)
        .cloned()
}

/// The parts of an MCP entry that matter for "already right".
fn same_server(a: &Value, b: &Value) -> bool {
    a.get("command") == b.get("command") && a.get("args") == b.get("args")
}

/// An MCP server merged into (or, on undo, out of) a JSON config file.
fn plan_server_file(ctx: &Ctx, path: &Path, entry: &Value, undo: bool, items: &mut Vec<Item>) {
    let label = rel(ctx, path);
    let old = match read(path) {
        Ok(o) => o,
        Err(e) => return items.push(Item::Note(format!("{label}: {e}"))),
    };
    let text = old.clone().unwrap_or_default();
    let existing = match member(&text, "mcpServers") {
        Ok(v) => v.and_then(|s| s.get(NAME).cloned()),
        Err(e) => return items.push(Item::Note(format!("{label}: {e}"))),
    };
    if let Some(e) = &existing {
        if !is_our_server(e) {
            return items.push(Item::Note(format!(
                "{label}: a server named {NAME} that isn't mnemo's is there; left alone"
            )));
        }
    }
    if undo {
        if existing.is_none() {
            return;
        }
        match without_server(&text, NAME) {
            Ok(new) => items.push(remove_or_write(
                path,
                old,
                new,
                format!("- the {NAME} server\n"),
            )),
            Err(e) => items.push(Item::Note(format!("{label}: {e}"))),
        }
        return;
    }
    if existing.as_ref().is_some_and(|e| same_server(e, entry)) {
        return items.push(Item::Note(format!(
            "{label}: the MCP server is already right"
        )));
    }
    match mi::merged(&text, NAME, entry) {
        Ok((new, previous)) => {
            let shown = mi::shown_change(&text, NAME, &previous, entry);
            items.push(Item::Write {
                path: path.to_path_buf(),
                old,
                new,
                shown,
            });
        }
        Err(e) => items.push(Item::Note(format!("{label}: {e}"))),
    }
}

/// On undo, a file left as an empty object is removed (onboard made it).
fn remove_or_write(path: &Path, old: Option<String>, new: String, shown: String) -> Item {
    let empty = serde_json::from_str::<Value>(&new)
        .ok()
        .and_then(|v| v.as_object().map(|o| o.is_empty()))
        .unwrap_or(false);
    if empty {
        Item::Remove {
            path: path.to_path_buf(),
        }
    } else {
        Item::Write {
            path: path.to_path_buf(),
            old,
            new,
            shown,
        }
    }
}

/// A hooks file (Claude Code's settings, or Cursor's hooks.json).
fn plan_hooks_file(ctx: &Ctx, client: Client, path: &Path, undo: bool, items: &mut Vec<Item>) {
    let label = rel(ctx, path);
    let old = match read(path) {
        Ok(o) => o,
        Err(e) => return items.push(Item::Note(format!("{label}: {e}"))),
    };
    let text = old.clone().unwrap_or_default();
    let hooks = match member(&text, "hooks") {
        Ok(h) => h,
        Err(e) => return items.push(Item::Note(format!("{label}: {e}"))),
    };
    let ours: Vec<(String, String, u64)> = if undo {
        Vec::new()
    } else {
        TIMEOUTS
            .iter()
            .map(|(event, claude_event, timeout)| {
                let name = match client {
                    Client::ClaudeCode => claude_event.to_string(),
                    Client::Cursor => CURSOR_EVENTS
                        .iter()
                        .find(|(e, _)| e == event)
                        .map(|(_, c)| c.to_string())
                        .unwrap(),
                };
                (name, hook_command(ctx, client, event), *timeout)
            })
            .collect()
    };
    let new_hooks = match client {
        Client::ClaudeCode => claude_hooks(hooks.clone(), &ours),
        Client::Cursor => cursor_hooks(hooks.clone(), &ours),
    };
    let empty = new_hooks.as_object().is_some_and(|o| o.is_empty());
    let mut new = match set_member(&text, "hooks", (!empty).then_some(&new_hooks)) {
        Ok(t) => t,
        Err(e) => return items.push(Item::Note(format!("{label}: {e}"))),
    };
    if client == Client::Cursor && !undo && member(&new, "version").ok().flatten().is_none() {
        // Cursor's hooks.json starts with its version.
        let mut m = members(&new).unwrap_or_default();
        m.insert(0, ("version".into(), "1".into()));
        new = format!("{}\n", mi::write_object(&m, 0));
    }
    if undo {
        if hooks == member(&new, "hooks").ok().flatten() {
            return;
        }
        // Cursor's version alone left: the file was ours.
        let only_version = members(&new)
            .map(|m| m.iter().all(|(k, _)| k == "version"))
            .unwrap_or(false);
        let new = if client == Client::Cursor && only_version {
            "{}\n".to_string()
        } else {
            new
        };
        items.push(remove_or_write(path, old, new, "- mnemo's hooks\n".into()));
        return;
    }
    if old.as_deref() == Some(new.as_str()) {
        return items.push(Item::Note(format!("{label}: the hooks are already right")));
    }
    let shown = shown_hooks(&hooks, &ours);
    items.push(Item::Write {
        path: path.to_path_buf(),
        old,
        new,
        shown,
    });
}

/// A generated file (the skill, the Cursor rule).
fn plan_generated(ctx: &Ctx, path: &Path, content: String, undo: bool, items: &mut Vec<Item>) {
    let label = rel(ctx, path);
    let old = match read(path) {
        Ok(o) => o,
        Err(e) => return items.push(Item::Note(format!("{label}: {e}"))),
    };
    let ours = old
        .as_deref()
        .is_some_and(|t| t.contains(mnemo_engine::guide::ONBOARD_HEADER));
    match (&old, ours, undo) {
        (None, _, true) => {}
        (Some(_), false, _) => items.push(Item::Note(format!(
            "{label}: a file mnemo onboard didn't write is there; left alone"
        ))),
        (Some(_), true, true) => items.push(Item::Remove {
            path: path.to_path_buf(),
        }),
        (Some(t), true, false) if *t == content => {
            items.push(Item::Note(format!("{label}: already right")))
        }
        (_, _, false) => {
            let lines = content.lines().count();
            items.push(Item::Write {
                path: path.to_path_buf(),
                old: old.clone(),
                new: content,
                shown: format!("+ the guide's card ({lines} lines)\n"),
            });
        }
    }
}

/// Claude Code's MCP entry in its local scope, through the `claude` CLI.
fn plan_claude_local(ctx: &Ctx, claude: &Path, undo: bool, items: &mut Vec<Item>) {
    let _ = claude;
    let entry = server_entry(ctx, Client::ClaudeCode);
    let existing = claude_local_entry(ctx);
    if let Some(e) = &existing {
        if !is_our_server(e) {
            return items.push(Item::Note(format!(
                "Claude Code (local scope): a server named {NAME} that isn't mnemo's is there; \
                 left alone"
            )));
        }
    }
    let remove = vec![
        "mcp".to_string(),
        "remove".into(),
        NAME.into(),
        "--scope".into(),
        "local".into(),
    ];
    if undo {
        if existing.is_some() {
            items.push(Item::Claude {
                args: remove,
                what: "remove the MCP server (local scope)".into(),
            });
        }
        return;
    }
    if existing.as_ref().is_some_and(|e| same_server(e, &entry)) {
        return items.push(Item::Note(
            "Claude Code (local scope): the MCP server is already right".into(),
        ));
    }
    if existing.is_some() {
        items.push(Item::Claude {
            args: remove,
            what: "replace the MCP server (local scope): remove the old one".into(),
        });
    }
    let json = serde_json::to_string(&entry).unwrap();
    items.push(Item::Claude {
        args: vec![
            "mcp".into(),
            "add-json".into(),
            NAME.into(),
            json,
            "--scope".into(),
            "local".into(),
        ],
        what: "add the MCP server (local scope)".into(),
    });
}

fn plan_claude(ctx: &Ctx, undo: bool, items: &mut Vec<Item>) {
    let claude = mi::on_path("claude");
    if ctx.plugin {
        let Some(_) = claude else {
            return items.push(Item::Note(
                "Claude Code: --plugin needs the `claude` CLI on the PATH; nothing was done for \
                 Claude Code"
                    .into(),
            ));
        };
        let args = if undo {
            vec![
                "plugin".into(),
                "uninstall".into(),
                PLUGIN.into(),
                "--scope".into(),
                ctx.scope.word().into(),
            ]
        } else {
            vec![
                "plugin".into(),
                "install".into(),
                "mnemo".into(),
                "--marketplace".into(),
                MARKETPLACE.into(),
                "--scope".into(),
                ctx.scope.word().into(),
                "-y".into(),
            ]
        };
        let what = if undo {
            "uninstall the mnemo plugin"
        } else {
            "install the mnemo plugin (its MCP server, hooks and skill)"
        };
        return items.push(Item::Claude {
            args,
            what: what.into(),
        });
    }
    // The MCP server.
    match (ctx.scope, &claude) {
        (Scope::Local, Some(c)) => plan_claude_local(ctx, c, undo, items),
        // Without `claude`, the local scope falls back to `.mcp.json`, left
        // out of git.
        _ => plan_server_file(
            ctx,
            &ctx.root.join(".mcp.json"),
            &server_entry(ctx, Client::ClaudeCode),
            undo,
            items,
        ),
    }
    let settings = match ctx.scope {
        Scope::Local => ".claude/settings.local.json",
        Scope::Project => ".claude/settings.json",
    };
    plan_hooks_file(
        ctx,
        Client::ClaudeCode,
        &ctx.root.join(settings),
        undo,
        items,
    );
    plan_generated(
        ctx,
        &ctx.root.join(".claude/skills/mnemo/SKILL.md"),
        mnemo_engine::guide::skill_md(),
        undo,
        items,
    );
}

fn plan_cursor(ctx: &Ctx, undo: bool, items: &mut Vec<Item>) {
    plan_server_file(
        ctx,
        &ctx.root.join(".cursor/mcp.json"),
        &server_entry(ctx, Client::Cursor),
        undo,
        items,
    );
    plan_hooks_file(
        ctx,
        Client::Cursor,
        &ctx.root.join(".cursor/hooks.json"),
        undo,
        items,
    );
    plan_generated(
        ctx,
        &ctx.root.join(".cursor/rules/mnemo.mdc"),
        mnemo_engine::guide::cursor_rule(),
        undo,
        items,
    );
}

/// `.git/info/exclude`'s block: the work-tree files written for this user.
fn plan_exclude(ctx: &Ctx, undo: bool, items: &mut Vec<Item>) {
    let path = project::exclude_path(&ctx.common);
    let old = read(&path).ok().flatten();
    let text = old.clone().unwrap_or_default();
    let mut lines = Vec::new();
    if !undo && ctx.scope == Scope::Local {
        for it in items.iter() {
            let p = match it {
                Item::Write { path, .. } => path,
                Item::Note(_) | Item::Remove { .. } | Item::Claude { .. } => continue,
                _ => continue,
            };
            if !p.starts_with(&ctx.root) || p.starts_with(&ctx.common) {
                continue;
            }
            if project::tracked(&ctx.root, p) {
                continue;
            }
            if let Some(l) = project::exclude_line(&ctx.root, p) {
                lines.push(l);
            }
        }
        // Already listed, from an earlier run.
        if lines.is_empty() {
            return;
        }
    }
    let new = project::with_excluded(&text, &lines);
    if new == text {
        return;
    }
    let shown = if undo {
        "- mnemo onboard's block\n".to_string()
    } else {
        lines.iter().map(|l| format!("+ {l}\n")).collect()
    };
    items.push(Item::Write {
        path,
        old,
        new,
        shown,
    });
}

// --- The brain and the model ------------------------------------------------------

/// Whether the brain's key opens it here: `Ok(())`, or the message to stop
/// with.
fn check_brain(brain: &Path) -> Result<(), String> {
    use mnemo_engine::keys::{self, Resolution, Secret};
    let unlock = format!(
        "this brain's key isn't on this machine: run `mnemo unlock` with its recovery code \
         (brain: {})",
        brain.display()
    );
    let store = keys::default_store();
    let secret = match keys::resolve(brain, store.as_deref(), keys::SERVER_COMMAND_TIMEOUT) {
        Ok(Resolution::Found(r)) => r.secret,
        _ => return Err(unlock),
    };
    let mut opts = mnemo_engine::SharedOptions::default();
    opts.deadline = std::time::Duration::from_secs(10);
    let r = match &secret {
        Secret::Passphrase(p) => mnemo_engine::Mnemo::open_read_only_waiting(brain, p, opts),
        Secret::Key(k) => mnemo_engine::Mnemo::open_read_only_waiting_with_key(brain, k, opts),
    };
    r.map(|_| ()).map_err(|e| match e {
        mnemo_engine::MnemoError::Locked { .. } => format!("the brain is busy: {e}"),
        _ => unlock,
    })
}

/// The model's plan item.
fn plan_model(no_model: bool, undo: bool, items: &mut Vec<Item>) {
    if undo {
        return;
    }
    #[cfg(feature = "embed-candle")]
    {
        use mnemo_engine::builtin;
        if no_model {
            items.push(Item::Note(
                "model: skipped (--no-model); recall is keyword-only and the hooks save nothing \
                 until `mnemo embedder install`"
                    .into(),
            ));
        } else if builtin::is_installed() {
            items.push(Item::Note("model: installed".into()));
        } else {
            items.push(Item::InstallModel(builtin::human_size(
                builtin::download_size(),
            )));
        }
    }
    #[cfg(not(feature = "embed-candle"))]
    {
        let _ = no_model;
        items.push(Item::Note(
            "model: this build of mnemo has no built-in embedder; recall is keyword-only and the \
             hooks save nothing"
                .into(),
        ));
    }
}

/// Create the brain with a generated key in the keychain, and show its
/// recovery code under the display rule. Returns whether the code was
/// shown and confirmed.
fn create_brain(brain: &Path, human: &mut Vec<String>) -> Result<(), String> {
    use mnemo_engine::keys;
    use mnemo_engine::{Memory, MnemoConfig};
    let store = keys::default_store().ok_or_else(no_keychain)?;
    store
        .check_available()
        .map_err(|e| format!("{} ({e})", no_keychain()))?;
    project::create_dir(brain)?;
    let cfg = MnemoConfig {
        dimensions: 384,
        ..Default::default()
    };
    let mut manifest = |db: &mut mnemo_engine::Mnemo| -> mnemo_engine::Result<()> {
        db.remember(Memory::scaffold_manifest(384))?;
        db.flush()
    };
    let label = brain.display().to_string();
    let mut persist = |salt: &[u8; 16], key: &[u8; 32]| {
        let entry = keys::StoredSecret::Key(zeroize::Zeroizing::new(*key));
        keys::store_and_check(&*store, salt, &entry, &label)
    };
    let key = keys::create_generated(brain, cfg, &mut manifest, &mut persist)
        .map_err(|e| e.to_string())?;
    println!(
        "created {} with a generated key, kept in {}",
        label,
        store.describe()
    );
    let code = keys::recovery::encode(&key);
    match keys::display::refusal_reason() {
        None => match show_and_confirm(&code) {
            Ok(true) => {}
            Ok(false) => human.push(
                "the recovery code wasn't confirmed: run `mnemo recovery-code` in a terminal of \
                 your own and write it down"
                    .into(),
            ),
            Err(e) => human.push(format!(
                "the recovery code couldn't be shown ({e}): run `mnemo recovery-code` in a \
                 terminal of your own and write it down"
            )),
        },
        Some(_) => human.push(
            "back up the recovery code: run `mnemo recovery-code` in a terminal of your own (not \
             through an agent's shell) and write it down"
                .into(),
        ),
    }
    Ok(())
}

fn no_keychain() -> String {
    "no keychain is available here, so onboard can't keep the brain's key; nothing was created. \
     Make the brain yourself with `mnemo init --ask-passphrase` or `mnemo init --key-file <PATH>`, \
     then run `mnemo onboard` again"
        .into()
}

/// Show the code, then ask for its last group back, up to three times.
fn show_and_confirm(code: &str) -> std::io::Result<bool> {
    use mnemo_engine::keys::display;
    let last = code.rsplit('-').next().unwrap_or("").to_ascii_uppercase();
    for _ in 0..3 {
        display::show(code)?;
        let answer = display::ask_hidden(
            "To check you wrote it down, type the code's last group (five characters): ",
        )?;
        let answer: String = answer
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_uppercase();
        if answer == last {
            return Ok(true);
        }
        eprintln!("that isn't the last group; here's the code again");
    }
    Ok(false)
}

/// The line that deletes this brain's keychain entry, for this OS.
fn keychain_delete_line(brain: &Path) -> Option<String> {
    use mnemo_engine::keys;
    let info = keys::file_info(brain).ok()?;
    let account = keys::hex(&info.salt);
    let service = keys::KEYCHAIN_SERVICE;
    Some(if cfg!(target_os = "macos") {
        format!("security delete-generic-password -s {service} -a {account}")
    } else if cfg!(windows) {
        format!("cmdkey /delete:{account}.{service}")
    } else {
        format!("secret-tool clear service {service} username {account}")
    })
}

// --- Running it ------------------------------------------------------------------

fn context(o: &Opts) -> Result<Ctx, String> {
    let (start, _) = project::start_dir(o.moved)?;
    let root = project::toplevel(&start).map_err(|_| {
        format!(
            "no project here: {} isn't in a git repository. Run `mnemo onboard` in your \
             project's repository",
            start.display()
        )
    })?;
    let common = project::common_dir(&root)?;
    let brain = common.join(project::DIR).join(project::FILE);
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|e| format!("can't find this mnemo's path: {e}"))?
        .to_string_lossy()
        .into_owned();
    Ok(Ctx {
        root,
        common,
        brain,
        exe,
        home: mi::home()?,
        scope: o.scope,
        budget: o.hook_budget.map(|b| b.min(super::hook::BUDGET_MAX)),
        plugin: o.plugin,
    })
}

/// The clients to wire: those given, else those found.
fn clients(o: &Opts, ctx: &Ctx) -> Vec<Client> {
    if !o.clients.is_empty() {
        return o.clients.clone();
    }
    let mut out = Vec::new();
    if mi::on_path("claude").is_some() || ctx.home.join(".claude").is_dir() {
        out.push(Client::ClaudeCode);
    }
    if mi::on_path("cursor").is_some() || ctx.home.join(".cursor").is_dir() {
        out.push(Client::Cursor);
    }
    out
}

fn interactive() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Print the plan.
fn show(ctx: &Ctx, items: &[Item], undo: bool) {
    println!(
        "mnemo onboard{}: {} ({} scope)",
        if undo { " --undo" } else { "" },
        ctx.root.display(),
        ctx.scope.word()
    );
    for it in items {
        match it {
            Item::CreateBrain => println!(
                "- brain: create {} with a generated key, kept in the keychain",
                rel(ctx, &ctx.brain)
            ),
            Item::BrainOk => println!("- brain: {} (its key opens it)", rel(ctx, &ctx.brain)),
            Item::InstallModel(size) => println!("- model: install the built-in model ({size})"),
            Item::Note(n) => println!("- {n}"),
            Item::Write {
                path,
                old,
                new,
                shown,
            } => {
                if old.as_deref() == Some(new.as_str()) {
                    continue;
                }
                let verb = if old.is_some() { "update" } else { "create" };
                println!("- {verb} {}", rel(ctx, path));
                for l in shown.lines() {
                    println!("    {l}");
                }
            }
            Item::Remove { path } => println!("- remove {}", rel(ctx, path)),
            Item::Claude { args, what } => {
                // The entry carries no secret; its env (none) is masked anyway.
                println!("- Claude Code: {what}: claude {}", shell_words(args));
            }
        }
    }
}

fn shell_words(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.contains(' ') || a.contains('{') || a.contains('"') {
                format!("'{a}'")
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Do the plan. Returns the human steps.
fn apply(ctx: &Ctx, items: &[Item], human: &mut Vec<String>) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    for it in items {
        match it {
            Item::CreateBrain => {
                create_brain(&ctx.brain, human)?;
                done.push(format!("created: {}", ctx.brain.display()));
            }
            Item::InstallModel(_) => install_model(human, &mut done),
            Item::Write { path, old, new, .. } => {
                if old.as_deref() == Some(new.as_str()) {
                    continue;
                }
                mi::write_atomically(path, new)?;
                done.push(format!("wrote: {}", path.display()));
            }
            Item::Remove { path } => {
                std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
                // The directories it was in, up to the root, when they're
                // left empty (`remove_dir` refuses a directory that isn't).
                let mut dir = path.parent();
                while let Some(d) = dir {
                    if d == ctx.root || !d.starts_with(&ctx.root) || std::fs::remove_dir(d).is_err()
                    {
                        break;
                    }
                    dir = d.parent();
                }
                done.push(format!("removed: {}", path.display()));
            }
            Item::Claude { args, what } => {
                let claude = mi::on_path("claude").ok_or("`claude` isn't on the PATH")?;
                let out = std::process::Command::new(claude)
                    .args(args)
                    .current_dir(&ctx.root)
                    .stdin(std::process::Stdio::null())
                    .output()
                    .map_err(|e| format!("running claude: {e}"))?;
                if !out.status.success() {
                    let why = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    human.push(format!(
                        "Claude Code: `claude {}` failed ({why}); run it in your own terminal",
                        shell_words(args)
                    ));
                } else {
                    done.push(format!("Claude Code: {what}"));
                }
            }
            Item::BrainOk | Item::Note(_) => {}
        }
    }
    Ok(done)
}

fn install_model(human: &mut Vec<String>, done: &mut Vec<String>) {
    #[cfg(feature = "embed-candle")]
    {
        match mnemo_engine::builtin::install(&mut |m| eprintln!("{m}")) {
            Ok(_) => done.push("model: installed".into()),
            Err(e) => human.push(format!(
                "the model couldn't be downloaded ({e}): recall is keyword-only and the hooks \
                 save nothing until `mnemo embedder install` works"
            )),
        }
    }
    #[cfg(not(feature = "embed-candle"))]
    {
        let _ = (human, done);
    }
}

/// Run `mnemo onboard` (without `--guide`).
pub fn run(o: Opts) -> Result<(), String> {
    let ctx = context(&o)?;
    let clients = clients(&o, &ctx);
    let mut items = Vec::new();
    let exists = ctx.brain.is_file();
    if !o.undo {
        if exists {
            check_brain(&ctx.brain)?;
            items.push(Item::BrainOk);
        } else {
            items.push(Item::CreateBrain);
        }
    }
    plan_model(o.no_model, o.undo, &mut items);
    if clients.is_empty() {
        items.push(Item::Note(
            "no client found (Claude Code or Cursor): nothing to wire. Pass --client \
             claude-code,cursor to wire one anyway"
                .into(),
        ));
    }
    for c in &clients {
        match c {
            Client::ClaudeCode => plan_claude(&ctx, o.undo, &mut items),
            Client::Cursor => plan_cursor(&ctx, o.undo, &mut items),
        }
    }
    plan_exclude(&ctx, o.undo, &mut items);
    show(&ctx, &items, o.undo);
    let changes = items.iter().filter(|i| i.is_change()).count();
    if o.print {
        return Ok(());
    }
    let mut human = Vec::new();
    let done = if changes == 0 {
        println!(
            "{}",
            if o.undo {
                "nothing to remove"
            } else {
                "already set up"
            }
        );
        Vec::new()
    } else {
        if !o.yes {
            if !interactive() {
                println!(
                    "nothing was written; run `mnemo onboard{} --yes` to write it",
                    if o.undo { " --undo" } else { "" }
                );
                std::process::exit(3);
            }
            let q = format!(
                "{} these {changes} change{}?",
                if o.undo { "Make" } else { "Write" },
                if changes == 1 { "" } else { "s" }
            );
            if !mi::confirm(&q)? {
                return Err("nothing was written".into());
            }
        }
        apply(&ctx, &items, &mut human)?
    };
    summary(&ctx, &o, &done, &human);
    if !o.undo {
        println!();
        print!("{}", mnemo_engine::guide::markdown());
    }
    Ok(())
}

fn summary(ctx: &Ctx, o: &Opts, done: &[String], human: &[String]) {
    println!();
    for d in done {
        println!("{d}");
    }
    println!("brain: {}", ctx.brain.display());
    let mut steps: Vec<String> = human.to_vec();
    if o.undo {
        println!("kept: the brain, its key and the model; onboard --undo never removes them");
        println!(
            "to delete the brain (its memories go with it): remove {}",
            ctx.brain.display()
        );
        #[cfg(feature = "embed-candle")]
        if let Ok(d) = mnemo_engine::builtin::cache_dir() {
            println!("to delete the model: remove {}", d.display());
        }
    } else {
        if o.no_model {
            steps.push(
                "the hooks save transcripts only once the model is installed: run `mnemo \
                 embedder install`"
                    .into(),
            );
        }
        if o.plugin {
            steps.push(
                "if the plugin install was refused (Claude Code ignores -y when an agent runs it), \
                 run `claude plugin install mnemo --marketplace toarchkumar/mnemo` in your own \
                 terminal"
                    .into(),
            );
        }
        steps.push(
            "Claude Code and Cursor run hooks only in a trusted workspace: accept the trust \
             prompt when you open this project"
                .into(),
        );
    }
    if let Some(line) = keychain_delete_line(&ctx.brain) {
        println!("to delete this brain's keychain entry: {line}");
    }
    for s in &steps {
        println!("human step: {s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn our_hooks_and_servers_are_recognised_in_both_forms() {
        for c in [
            "mnemo hook session-start --client claude-code",
            "/usr/local/bin/mnemo hook session-end --client cursor",
            "\"/Users/me/My Apps/mnemo\" hook pre-compact --client claude-code",
            "C:\\tools\\mnemo.exe hook session-start --client claude-code",
        ] {
            assert!(is_our_hook(c), "{c}");
        }
        for c in [
            "mnemo serve --mcp",
            "./my-script.sh hook",
            "notmnemo hook x",
            "echo mnemo hook",
            "mnemo-other hook session-start",
        ] {
            assert!(!is_our_hook(c), "{c}");
        }
        assert!(is_our_server(
            &json!({"command": "/x/mnemo", "args": ["serve", "--mcp"]})
        ));
        assert!(is_our_server(
            &json!({"command": "mnemo", "args": ["-C", "${workspaceFolder}", "serve"]})
        ));
        assert!(!is_our_server(
            &json!({"command": "npx", "args": ["mnemo", "serve"]})
        ));
        assert!(!is_our_server(
            &json!({"command": "mnemo", "args": ["about"]})
        ));
    }

    #[test]
    fn hooks_merge_beside_a_users_own_and_undo_leaves_them() {
        let user =
            json!({"SessionStart": [{"hooks": [{"type": "command", "command": "./mine.sh"}]}]});
        let ours = vec![(
            "SessionStart".to_string(),
            "mnemo hook session-start --client claude-code".to_string(),
            15,
        )];
        let added = claude_hooks(Some(user.clone()), &ours);
        assert_eq!(added["SessionStart"].as_array().unwrap().len(), 2);
        // Adding again replaces ours, never doubles it.
        let again = claude_hooks(Some(added.clone()), &ours);
        assert_eq!(again, added);
        // Undo leaves the user's.
        assert_eq!(claude_hooks(Some(added), &[]), user);
        // Cursor's flat arrays.
        let cu = json!({"sessionStart": [{"command": "./mine.sh"}]});
        let ours = vec![(
            "sessionStart".to_string(),
            "mnemo hook session-start --client cursor".to_string(),
            15,
        )];
        let added = cursor_hooks(Some(cu.clone()), &ours);
        assert_eq!(added["sessionStart"].as_array().unwrap().len(), 2);
        assert_eq!(cursor_hooks(Some(added), &[]), cu);
    }

    #[test]
    fn set_member_keeps_the_rest_of_the_text() {
        let old = "{\n  \"theme\": \"dark\",\n  \"x\": {\"a\":   1}\n}\n";
        let new = set_member(old, "hooks", Some(&json!({"A": []}))).unwrap();
        assert!(new.contains("{\"a\":   1}"), "{new}");
        assert!(new.find("theme").unwrap() < new.find("hooks").unwrap());
        let back = set_member(&new, "hooks", None).unwrap();
        assert!(
            !back.contains("hooks") && back.contains("{\"a\":   1}"),
            "{back}"
        );
        assert!(set_member("{ not json", "hooks", None).is_err());
    }
}
