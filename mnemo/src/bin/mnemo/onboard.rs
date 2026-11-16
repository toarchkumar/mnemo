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
    /// `None`: keep what an earlier run chose.
    pub wake: Option<Wake>,
}

/// The MCP server's name in every client.
const NAME: &str = "mnemo";
/// The plugin and its marketplace (`.claude-plugin/marketplace.json`).
const PLUGIN: &str = "mnemo@mnemo-engine";
const MARKETPLACE: &str = "toarchkumar/mnemo";
/// The hook timeouts written into the configs, in seconds. 13a adds the
/// per-turn hook (`user-prompt`), and `stop` when `--wake stop` asks for it.
const TIMEOUTS: [(&str, &str, u64); 5] = [
    ("session-start", "SessionStart", 15),
    ("pre-compact", "PreCompact", 30),
    ("session-end", "SessionEnd", 30),
    ("user-prompt", "UserPromptSubmit", 10),
    ("stop", "Stop", 10),
];
/// Cursor's names for the same events (its per-turn hook is
/// `postToolUse`, which the hook gates to once a minute).
const CURSOR_EVENTS: [(&str, &str); 5] = [
    ("session-start", "sessionStart"),
    ("pre-compact", "preCompact"),
    ("session-end", "sessionEnd"),
    ("user-prompt", "postToolUse"),
    ("stop", "stop"),
];

/// 13a: `onboard --wake`: whether the Stop continuation is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Wake {
    /// Write the Stop hook: an agent finishing a turn is kept going for a
    /// pre-approved task or a message to it by id.
    Stop,
    /// Don't (remove it if an earlier run wrote it).
    Off,
}

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
    wake: Option<Wake>,
}

/// One thing the plan does (or found already right).
enum Item {
    /// Create the brain with a generated key.
    CreateBrain,
    /// The brain is there, and its key opens it: where the key came from,
    /// in a few words (never a key, never a path but a file's name).
    BrainOk(String),
    /// Install the built-in model (only a build with `embed-candle` has one).
    #[cfg_attr(not(feature = "embed-candle"), allow(dead_code))]
    InstallModel(String),
    /// A note in the plan (skipped, left alone, an error): one to look at.
    Note(String),
    /// Something already right: `line` for an agent, `what` (a short noun
    /// phrase) for a person at a terminal.
    Ok { line: String, what: String },
    /// Write (or create) a file. `tty` is what a person at a terminal sees.
    Write {
        path: PathBuf,
        old: Option<String>,
        new: String,
        shown: String,
        tty: Vec<(Mark, String)>,
    },
    /// Delete a file `onboard` wrote.
    Remove { path: PathBuf },
    /// Run the `claude` CLI in the project's root. `tty`: what a person at a
    /// terminal sees (none for the first half of a replacement).
    Claude {
        args: Vec<String>,
        what: String,
        tty: Option<(Mark, String)>,
    },
}

/// ONBOARD-UX: how a plan line is marked at a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mark {
    Add,
    Change,
    Same,
    Remove,
    Warn,
}

impl Mark {
    fn sign(self) -> char {
        match self {
            Mark::Add => '+',
            Mark::Change => '~',
            Mark::Same => '=',
            Mark::Remove => '-',
            Mark::Warn => '!',
        }
    }
    /// The ANSI colour of its sign.
    fn colour(self) -> &'static str {
        match self {
            Mark::Add => "32",
            Mark::Change => "33",
            Mark::Same => "2",
            Mark::Remove => "31",
            Mark::Warn => "35",
        }
    }
    fn legend(self) -> &'static str {
        match self {
            Mark::Add => "add",
            Mark::Change => "change",
            Mark::Same => "already right",
            Mark::Remove => "remove",
            Mark::Warn => "needs attention",
        }
    }
}

/// ONBOARD-UX: the plan's groups at a terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    Brain,
    Model,
    ClaudeCode,
    Cursor,
    Git,
    Other,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Brain => "Brain",
            Section::Model => "Model",
            Section::ClaudeCode => "Claude Code",
            Section::Cursor => "Cursor",
            Section::Git => "Git",
            Section::Other => "Clients",
        }
    }
}

impl Item {
    fn is_change(&self) -> bool {
        match self {
            Item::Write { old, new, .. } => old.as_deref() != Some(new.as_str()),
            Item::CreateBrain
            | Item::InstallModel(_)
            | Item::Remove { .. }
            | Item::Claude { .. } => true,
            Item::BrainOk(_) | Item::Note(_) | Item::Ok { .. } => false,
        }
    }
}

/// `p` from the project's root, with `/` on every OS.
fn rel(ctx: &Ctx, p: &Path) -> String {
    match p.strip_prefix(&ctx.root) {
        Ok(r) => r
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => p.display().to_string(),
    }
}

/// The program word of a hook command, always quoted (RB-011.3): a hook's
/// command is run by a shell, and a path can hold anything. On Windows it's
/// written with `/`, which Git Bash, `cmd` and PowerShell all run, and no
/// Windows path holds a `"`. Elsewhere `\`, `"`, `$` and `` ` `` are escaped,
/// as POSIX `sh` reads them inside double quotes.
fn quote(s: &str) -> String {
    if cfg!(windows) {
        format!("\"{}\"", s.replace('\\', "/"))
    } else {
        let mut q = String::from("\"");
        for c in s.chars() {
            if matches!(c, '\\' | '"' | '$' | '`') {
                q.push('\\');
            }
            q.push(c);
        }
        q.push('"');
        q
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

/// The first word of a command line, unquoted: a double-quoted word (with
/// `sh`'s backslash escapes inside it) or one up to the first space.
fn first_word(cmd: &str) -> (std::borrow::Cow<'_, str>, &str) {
    let cmd = cmd.trim_start();
    if let Some(rest) = cmd.strip_prefix('"') {
        let mut word = String::new();
        let mut chars = rest.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return (word.into(), &rest[i + 1..]),
                '\\' => match chars.clone().next() {
                    Some((_, n)) if matches!(n, '\\' | '"' | '$' | '`') => {
                        word.push(n);
                        chars.next();
                    }
                    _ => word.push(c),
                },
                c => word.push(c),
            }
        }
        (word.into(), "")
    } else {
        match cmd.find(' ') {
            Some(i) => (cmd[..i].into(), &cmd[i..]),
            None => (cmd.into(), ""),
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
    is_mnemo(&program) && rest.trim_start().starts_with("hook ")
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

/// Whether an MCP entry is the one `onboard` would write, member for member
/// (RB-011.2): an entry of ours with anything more in it, such as an `env`
/// block, isn't "already right" and is replaced. An empty `env` or
/// `headers` counts as none, since a client may add one.
fn same_server(have: &Value, want: &Value) -> bool {
    let norm = |v: &Value| -> Value {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.retain(|k, x| {
                !((k == "env" || k == "headers") && x.as_object().is_some_and(|m| m.is_empty()))
            });
        }
        v
    };
    norm(have) == norm(want)
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
                vec![(Mark::Remove, format!("the MCP server, in {label}"))],
            )),
            Err(e) => items.push(Item::Note(format!("{label}: {e}"))),
        }
        return;
    }
    if existing.as_ref().is_some_and(|e| same_server(e, entry)) {
        return items.push(Item::Ok {
            line: format!("{label}: the MCP server is already right"),
            what: format!("the MCP server ({label})"),
        });
    }
    match mi::merged(&text, NAME, entry) {
        Ok((new, previous)) => {
            let shown = mi::shown_change(&text, NAME, &previous, entry);
            let mark = if existing.is_some() {
                Mark::Change
            } else {
                Mark::Add
            };
            items.push(Item::Write {
                path: path.to_path_buf(),
                old,
                new,
                shown,
                tty: vec![(mark, format!("the MCP server, in {label}"))],
            });
        }
        Err(e) => items.push(Item::Note(format!("{label}: {e}"))),
    }
}

/// On undo, a file left as an empty object is removed (onboard made it).
fn remove_or_write(
    path: &Path,
    old: Option<String>,
    new: String,
    shown: String,
    tty: Vec<(Mark, String)>,
) -> Item {
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
            tty,
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
    // The Stop continuation: as `--wake` says, else as an earlier run left
    // it (our `hook stop` already in the file).
    let has_stop = hooks.as_ref().is_some_and(|h| {
        let mut found = false;
        let mut walk = vec![h];
        while let Some(v) = walk.pop() {
            match v {
                Value::Object(o) => {
                    if let Some(c) = o.get("command").and_then(Value::as_str) {
                        found |=
                            is_our_hook(c) && first_word(c).1.trim_start().starts_with("hook stop");
                    }
                    walk.extend(o.values());
                }
                Value::Array(a) => walk.extend(a.iter()),
                _ => {}
            }
        }
        found
    });
    let stop = match ctx.wake {
        Some(Wake::Stop) => true,
        Some(Wake::Off) => false,
        None => has_stop,
    };
    let ours: Vec<(String, String, u64)> = if undo {
        Vec::new()
    } else {
        TIMEOUTS
            .iter()
            .filter(|(event, _, _)| *event != "stop" || stop)
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
        items.push(remove_or_write(
            path,
            old,
            new,
            "- mnemo's hooks\n".into(),
            vec![(Mark::Remove, format!("mnemo's hooks, in {label}"))],
        ));
        return;
    }
    if old.as_deref() == Some(new.as_str()) {
        return items.push(Item::Ok {
            line: format!("{label}: the hooks are already right"),
            what: hooks_count(ours.len()),
        });
    }
    let shown = shown_hooks(&hooks, &ours);
    let tty = hook_changes(&hooks, &ours, &label);
    items.push(Item::Write {
        path: path.to_path_buf(),
        old,
        new,
        shown,
        tty,
    });
}

fn hooks_count(n: usize) -> String {
    format!("{n} hook{}", if n == 1 { "" } else { "s" })
}

/// Our handlers in a `hooks` object, in either client's shape: (event,
/// command, timeout).
fn our_hooks(hooks: &Option<Value>) -> Vec<(String, String, Option<u64>)> {
    let mut out = Vec::new();
    let Some(Value::Object(o)) = hooks else {
        return out;
    };
    for (event, arr) in o {
        for g in arr.as_array().into_iter().flatten() {
            let handlers: Vec<&Value> = match g.get("hooks").and_then(Value::as_array) {
                Some(hs) => hs.iter().collect(),
                None => vec![g],
            };
            for h in handlers {
                if let Some(c) = h.get("command").and_then(Value::as_str) {
                    if is_our_hook(c) {
                        let t = h.get("timeout").and_then(Value::as_u64);
                        out.push((event.clone(), c.to_string(), t));
                    }
                }
            }
        }
    }
    out
}

/// ONBOARD-UX: each of our hooks marked added, changed, already right or
/// removed, with the already-right ones counted in one line.
fn hook_changes(
    old_hooks: &Option<Value>,
    ours: &[(String, String, u64)],
    label: &str,
) -> Vec<(Mark, String)> {
    let had = our_hooks(old_hooks);
    let (mut added, mut changed) = (Vec::new(), Vec::new());
    let mut same = 0;
    for (event, cmd, timeout) in ours {
        let before: Vec<_> = had.iter().filter(|(e, _, _)| e == event).collect();
        if before
            .iter()
            .any(|(_, c, t)| c == cmd && *t == Some(*timeout))
        {
            same += 1;
        } else if before.is_empty() {
            added.push(event.clone());
        } else {
            changed.push(event.clone());
        }
    }
    let mut gone: Vec<String> = had
        .iter()
        .filter(|(e, _, _)| !ours.iter().any(|(o, _, _)| o == e))
        .map(|(e, _, _)| e.clone())
        .collect();
    gone.sort_unstable();
    gone.dedup();
    // One line per mark: "the A, B and C hooks, in <file>".
    let mut out = Vec::new();
    for (mark, events) in [
        (Mark::Add, added),
        (Mark::Change, changed),
        (Mark::Remove, gone),
    ] {
        if !events.is_empty() {
            let noun = if events.len() == 1 { "hook" } else { "hooks" };
            out.push((
                mark,
                format!("the {} {noun}, in {label}", and_list(&events)),
            ));
        }
    }
    if out.is_empty() {
        // Only the rest of the file changed (Cursor's `version`, say).
        out.push((Mark::Change, label.to_string()));
    }
    if same > 0 {
        out.push((Mark::Same, hooks_count(same)));
    }
    out
}

/// Lines added and removed between `old` and `new` (a longest common
/// subsequence of lines; the files are a few hundred lines at most).
fn line_changes(old: &str, new: &str) -> (usize, usize) {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut row = vec![0usize; b.len() + 1];
    for x in &a {
        let mut diag = 0;
        for (j, y) in b.iter().enumerate() {
            let up = row[j + 1];
            row[j + 1] = if x == y { diag + 1 } else { up.max(row[j]) };
            diag = up;
        }
    }
    let common = row[b.len()];
    (b.len() - common, a.len() - common)
}

/// A generated file (the skill, the Cursor rule).
fn plan_generated(
    ctx: &Ctx,
    path: &Path,
    content: String,
    what: &str,
    undo: bool,
    items: &mut Vec<Item>,
) {
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
        (Some(t), true, false) if *t == content => items.push(Item::Ok {
            line: format!("{label}: already right"),
            what: what.to_string(),
        }),
        (_, _, false) => {
            let lines = content.lines().count();
            let tty = match &old {
                None => (Mark::Add, format!("{what}, in {label}")),
                Some(o) => {
                    let (added, removed) = line_changes(o, &content);
                    (
                        Mark::Change,
                        format!("{what}, in {label} (+{added} -{removed} lines)"),
                    )
                }
            };
            items.push(Item::Write {
                path: path.to_path_buf(),
                old: old.clone(),
                new: content,
                shown: format!("+ the guide's card ({lines} lines)\n"),
                tty: vec![tty],
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
                tty: Some((Mark::Remove, "the MCP server (local scope)".into())),
            });
        }
        return;
    }
    if existing.as_ref().is_some_and(|e| same_server(e, &entry)) {
        return items.push(Item::Ok {
            line: "Claude Code (local scope): the MCP server is already right".into(),
            what: "the MCP server (local scope)".into(),
        });
    }
    if existing.is_some() {
        items.push(Item::Claude {
            args: remove,
            what: "replace the MCP server (local scope): remove the old one".into(),
            tty: None,
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
        tty: Some(if existing.is_some() {
            (Mark::Change, "the MCP server (local scope)".into())
        } else {
            (Mark::Add, "the MCP server (local scope)".into())
        }),
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
        let tty = if undo {
            (Mark::Remove, "the mnemo plugin".to_string())
        } else {
            (
                Mark::Add,
                "the mnemo plugin (its MCP server, hooks and skill)".to_string(),
            )
        };
        return items.push(Item::Claude {
            args,
            what: what.into(),
            tty: Some(tty),
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
        "the skill",
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
        "the rule",
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
        let mut merged_into = Vec::new();
        for it in items.iter() {
            let (p, created) = match it {
                Item::Write { path, old, .. } => (path, old.is_none()),
                _ => continue,
            };
            if !p.starts_with(&ctx.root) || p.starts_with(&ctx.common) {
                continue;
            }
            if project::tracked(&ctx.root, p) {
                continue;
            }
            // Only the files onboard creates (RB-011.5 item 4): a file of
            // the user's it merges into stays visible to git, with a line.
            let Some(l) = project::exclude_line(&ctx.root, p) else {
                continue;
            };
            if created {
                lines.push(l);
            } else if !text.lines().any(|have| have == l) {
                merged_into.push(rel(ctx, p));
            }
        }
        for f in merged_into {
            items.push(Item::Note(format!(
                "{f} now holds a path on this machine: don't commit it"
            )));
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
    let label = rel(ctx, &path);
    let tty = if undo {
        (Mark::Remove, format!("mnemo onboard's block in {label}"))
    } else {
        (
            Mark::Add,
            format!(
                "{} file{} left out of git, in {label}",
                lines.len(),
                if lines.len() == 1 { "" } else { "s" }
            ),
        )
    };
    items.push(Item::Write {
        path,
        old,
        new,
        shown,
        tty: vec![tty],
    });
}

// --- The brain and the model ------------------------------------------------------

/// Whether the brain's key opens it here: where the key came from, in a
/// few words, or the message to stop with.
fn check_brain(brain: &Path) -> Result<String, String> {
    use mnemo_engine::keys::{self, Resolution, Secret};
    let passphrase =
        keys::file_info(brain).is_ok_and(|i| matches!(i.kind, keys::SecretKind::Passphrase));
    let unlock = if passphrase {
        // A passphrase brain has no recovery code (RB-011.5 item 8).
        format!(
            "this brain is opened with a passphrase, and none is available here: set \
             MNEMO_PASSPHRASE for this command, or move the brain to a key with `mnemo rekey \
             --new-key` (brain: {})",
            brain.display()
        )
    } else {
        format!(
            "this brain's key isn't on this machine: run `mnemo unlock` with its recovery code \
             (brain: {})",
            brain.display()
        )
    };
    let store = keys::default_store();
    let (secret, source) =
        match keys::resolve(brain, store.as_deref(), keys::SERVER_COMMAND_TIMEOUT) {
            Ok(Resolution::Found(r)) => (r.secret, r.source),
            _ => return Err(unlock),
        };
    let from = key_source(&source);
    let mut opts = mnemo_engine::SharedOptions::default();
    opts.deadline = std::time::Duration::from_secs(10);
    let r = match &secret {
        Secret::Passphrase(p) => mnemo_engine::Mnemo::open_read_only_waiting(brain, p, opts),
        Secret::Key(k) => mnemo_engine::Mnemo::open_read_only_waiting_with_key(brain, k, opts),
    };
    r.map(|_| from).map_err(|e| match e {
        mnemo_engine::MnemoError::Locked { .. } => format!("the brain is busy: {e}"),
        _ => unlock,
    })
}

/// Where a brain's secret came from, in a few words: never the secret, and
/// of a key file only its name.
fn key_source(source: &mnemo_engine::keys::Source) -> String {
    use mnemo_engine::keys::Source;
    match source {
        Source::Store(_) => "key found in your keychain".into(),
        Source::EnvKey => "key from MNEMO_KEY".into(),
        Source::KeyFile(p) => match p.file_name() {
            Some(n) => format!("key from the file {}", n.to_string_lossy()),
            None => "key from MNEMO_KEY_FILE".into(),
        },
        Source::KeyCommand => "key from MNEMO_KEY_COMMAND".into(),
        Source::EnvPassphrase => "passphrase from MNEMO_PASSPHRASE".into(),
    }
}

/// Whether Claude Code has recorded this folder as trusted: `~/.claude.json`'s
/// `projects[<root>].hasTrustDialogAccepted`, read and nothing else from the
/// file. `None` when the file or the field is missing or unreadable.
fn claude_trusted(ctx: &Ctx) -> Option<bool> {
    let text = std::fs::read_to_string(ctx.home.join(".claude.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    v.get("projects")?
        .get(ctx.root.to_string_lossy().as_ref())?
        .get("hasTrustDialogAccepted")?
        .as_bool()
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
            items.push(Item::Ok {
                line: "model: installed".into(),
                what: "the built-in model is installed".into(),
            });
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
fn create_brain(brain: &Path, shown: &str, human: &mut Vec<String>) -> Result<(), String> {
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
        shown,
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
        .map(project::plain)
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
        wake: o.wake,
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
            Item::BrainOk(_) => println!("- brain: {} (its key opens it)", rel(ctx, &ctx.brain)),
            Item::InstallModel(size) => println!("- model: install the built-in model ({size})"),
            Item::Note(n) => println!("- {n}"),
            Item::Ok { line, .. } => println!("- {line}"),
            Item::Write {
                path,
                old,
                new,
                shown,
                ..
            } => {
                if old.as_deref() == Some(new.as_str()) {
                    continue;
                }
                let verb = if old.is_some() { "update" } else { "create" };
                match through_link(path) {
                    Some(t) => println!(
                        "- {verb} {} (through its link, in {})",
                        rel(ctx, path),
                        t.display()
                    ),
                    None => println!("- {verb} {}", rel(ctx, path)),
                }
                for l in shown.lines() {
                    println!("    {l}");
                }
            }
            Item::Remove { path } => println!("- remove {}", rel(ctx, path)),
            Item::Claude { args, what, .. } => {
                // The entry carries no secret; its env (none) is masked anyway.
                println!("- Claude Code: {what}: claude {}", shell_words(args));
            }
        }
    }
}

// --- At a terminal (ONBOARD-UX) ------------------------------------------------------

/// Whether a person reads the output: stdout is a terminal. (A dev-hooks
/// build takes `MNEMO_DEV_ONBOARD_TTY` for tests.) Without one, the output
/// is v0.16.1's, for an agent: the plan, the guide, the human steps last.
fn at_terminal() -> bool {
    #[cfg(feature = "dev-hooks")]
    if std::env::var_os("MNEMO_DEV_ONBOARD_TTY").is_some() {
        return true;
    }
    use std::io::IsTerminal;
    std::io::stdout().is_terminal()
}

/// Colour at a terminal, unless `NO_COLOR` is set (and not empty) or the
/// terminal is `dumb`.
fn use_colour() -> bool {
    std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty())
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
}

/// How a person's view is drawn.
struct Pen {
    colour: bool,
}

impl Pen {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.colour {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn heading(&self, t: &str) -> String {
        self.paint("1", t)
    }
    fn mark(&self, m: Mark) -> String {
        self.paint(m.colour(), &m.sign().to_string())
    }
    /// `text` after `lead`, wrapped at 80 columns with a hanging indent.
    /// A command in backticks is never split, so it copies whole.
    fn line(&self, lead: &str, lead_width: usize, text: &str) -> String {
        let room = 80usize.saturating_sub(lead_width).max(30);
        let mut words: Vec<String> = Vec::new();
        let mut open = false;
        for w in text.split_whitespace() {
            match words.last_mut() {
                Some(last) if open => {
                    last.push(' ');
                    last.push_str(w);
                }
                _ => words.push(w.to_string()),
            }
            if w.matches('`').count() % 2 == 1 {
                open = !open;
            }
        }
        let mut out = String::new();
        let mut cur = String::new();
        for w in &words {
            if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > room {
                out.push_str(&cur);
                out.push('\n');
                out.push_str(&" ".repeat(lead_width));
                cur.clear();
            }
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(w);
        }
        out.push_str(&cur);
        format!("{lead}{out}")
    }
    fn marked(&self, m: Mark, text: &str) -> String {
        self.line(&format!("  {} ", self.mark(m)), 4, text)
    }
}

/// `p` with the home directory as `~`.
fn tilde(ctx: &Ctx, p: &Path) -> String {
    match p.strip_prefix(&ctx.home) {
        Ok(r) => format!("~/{}", r.display()),
        Err(_) => p.display().to_string(),
    }
}

/// What a person sees for one item.
fn tty_lines(ctx: &Ctx, it: &Item) -> Vec<(Mark, String)> {
    match it {
        Item::CreateBrain => vec![(
            Mark::Add,
            format!(
                "{}, with a generated key kept in the keychain",
                rel(ctx, &ctx.brain)
            ),
        )],
        Item::BrainOk(from) => vec![(Mark::Same, format!("{} ({from})", rel(ctx, &ctx.brain)))],
        Item::InstallModel(size) => {
            vec![(Mark::Add, format!("the built-in model ({size} download)"))]
        }
        // The Model heading already says it.
        Item::Note(n) => vec![(
            Mark::Warn,
            n.strip_prefix("model: ").unwrap_or(n).to_string(),
        )],
        Item::Ok { what, .. } => vec![(Mark::Same, what.clone())],
        Item::Write { old, new, tty, .. } => {
            if old.as_deref() == Some(new.as_str()) {
                Vec::new()
            } else {
                tty.clone()
            }
        }
        Item::Remove { path } => vec![(Mark::Remove, rel(ctx, path))],
        Item::Claude { tty, .. } => tty.iter().cloned().collect(),
    }
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The plan, grouped, for a person at a terminal.
fn show_tty(ctx: &Ctx, items: &[Item], sections: &[(Section, usize)], undo: bool, pen: &Pen) {
    println!(
        "{} {} ({} scope)",
        pen.heading(if undo {
            "mnemo onboard --undo:"
        } else {
            "mnemo onboard:"
        }),
        tilde(ctx, &ctx.root),
        ctx.scope.word()
    );
    let mut used = Vec::new();
    for (i, (section, start)) in sections.iter().enumerate() {
        let end = sections.get(i + 1).map_or(items.len(), |(_, e)| *e);
        let lines: Vec<(Mark, String)> = items[*start..end]
            .iter()
            .flat_map(|it| tty_lines(ctx, it))
            .collect();
        if lines.is_empty() {
            continue;
        }
        println!();
        println!("{}", pen.heading(section.title()));
        let mut same = Vec::new();
        for (m, t) in &lines {
            if !used.contains(m) {
                used.push(*m);
            }
            match (m, section) {
                // A section of one thing says it plainly.
                (Mark::Same, Section::Brain | Section::Model) => {
                    println!("{}", pen.marked(*m, t))
                }
                (Mark::Same, _) => same.push(t.clone()),
                _ => println!("{}", pen.marked(*m, t)),
            }
        }
        if !same.is_empty() {
            println!(
                "{}",
                pen.marked(Mark::Same, &format!("already right: {}", and_list(&same)))
            );
        }
    }
    let order = [
        Mark::Add,
        Mark::Change,
        Mark::Remove,
        Mark::Same,
        Mark::Warn,
    ];
    let legend: Vec<String> = order
        .iter()
        .filter(|m| used.contains(m))
        .map(|m| format!("{} {}", pen.mark(*m), m.legend()))
        .collect();
    if !legend.is_empty() {
        println!();
        println!("  {}", legend.join("   "));
    }
    println!();
}

/// After the writes, for a person at a terminal: what was done, the guide's
/// pointer, then the next steps, the human steps last.
fn summary_tty(
    ctx: &Ctx,
    o: &Opts,
    done: &[Done],
    human: &[String],
    restart: &[&str],
    clients: &[Client],
    pen: &Pen,
) {
    let tty: Vec<&(Mark, String)> = done.iter().flat_map(|d| d.tty.iter()).collect();
    if !tty.is_empty() {
        println!("{}", pen.heading("Done"));
        for (m, t) in tty {
            println!("{}", pen.marked(*m, t));
        }
    }
    println!();
    println!("The brain: {}", rel(ctx, &ctx.brain));
    if o.undo {
        println!();
        println!("{}", pen.heading("Kept"));
        println!(
            "{}",
            pen.line(
                "  ",
                2,
                "The brain, its key and the model: onboard --undo never removes them."
            )
        );
        println!(
            "{}",
            pen.line(
                "  ",
                2,
                &format!(
                    "To delete the brain (its memories go with it): remove {}",
                    rel(ctx, &ctx.brain)
                )
            )
        );
        #[cfg(feature = "embed-candle")]
        if let Ok(d) = mnemo_engine::builtin::cache_dir() {
            println!(
                "{}",
                pen.line(
                    "  ",
                    2,
                    &format!("To delete the model: remove {}", tilde(ctx, &d))
                )
            );
        }
        if let Some(line) = keychain_delete_line(&ctx.brain) {
            // A command on a line of its own, never wrapped.
            println!("  To delete this brain's keychain entry:");
            println!("    {line}");
        }
        return;
    }
    println!("The guide for agents: mnemo onboard --guide");
    let mut steps: Vec<String> = Vec::new();
    if !restart.is_empty() {
        steps.push(format!(
            "Restart {} (or start a new session) so {} the changes.",
            and_list(&restart.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
            if restart.len() == 1 {
                "it loads"
            } else {
                "they load"
            }
        ));
    }
    steps.extend(person_steps(o, human).iter().map(|s| sentence(s)));
    steps.extend(trust_step(ctx, clients));
    if steps.is_empty() {
        return;
    }
    println!();
    println!("{}", pen.heading("Next"));
    for (i, s) in steps.iter().enumerate() {
        let lead = format!("  {}. ", i + 1);
        let w = lead.len();
        println!("{}", pen.line(&lead, w, s));
    }
}

/// A step as a sentence: a capital first, a full stop last.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    let mut out: String = match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => return String::new(),
    };
    if !out.ends_with('.') {
        out.push('.');
    }
    out
}

/// The file a symbolic link at `path` leads to (RB-011.5 item 3), or `None`
/// when it isn't a link (or leads nowhere, and is then replaced).
fn through_link(path: &Path) -> Option<PathBuf> {
    std::fs::symlink_metadata(path)
        .ok()
        .filter(|m| m.file_type().is_symlink())?;
    std::fs::canonicalize(path).ok().map(project::plain)
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

/// One thing done: the line an agent reads, and what a person sees.
struct Done {
    plain: String,
    tty: Vec<(Mark, String)>,
}

impl Done {
    fn new(plain: String, tty: Vec<(Mark, String)>) -> Done {
        Done { plain, tty }
    }
}

/// Do the plan. Returns what was done; the human steps go in `human`.
fn apply(ctx: &Ctx, items: &[Item], human: &mut Vec<String>) -> Result<Vec<Done>, String> {
    let mut done = Vec::new();
    for it in items {
        // What a person sees: the plan's lines, less what was already right.
        let shown: Vec<(Mark, String)> = tty_lines(ctx, it)
            .into_iter()
            .filter(|(m, _)| *m != Mark::Same && *m != Mark::Warn)
            .collect();
        match it {
            Item::CreateBrain => {
                // A person sees the path from the repository (ONBOARD-UX).
                let shown = if at_terminal() {
                    rel(ctx, &ctx.brain)
                } else {
                    ctx.brain.display().to_string()
                };
                create_brain(&ctx.brain, &shown, human)?;
                done.push(Done::new(
                    format!("created: {}", ctx.brain.display()),
                    vec![(Mark::Add, rel(ctx, &ctx.brain))],
                ));
            }
            Item::InstallModel(_) => install_model(human, &mut done),
            Item::Write { path, old, new, .. } => {
                if old.as_deref() == Some(new.as_str()) {
                    continue;
                }
                // Through a symbolic link to its file, so the link stays.
                let target = through_link(path).unwrap_or_else(|| path.clone());
                mi::write_atomically(&target, new)?;
                let mark = if old.is_some() {
                    Mark::Change
                } else {
                    Mark::Add
                };
                done.push(Done::new(
                    format!("wrote: {}", target.display()),
                    vec![(mark, rel(ctx, path))],
                ));
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
                done.push(Done::new(format!("removed: {}", path.display()), shown));
            }
            Item::Claude { args, what, .. } => {
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
                    done.push(Done::new(format!("Claude Code: {what}"), shown));
                }
            }
            Item::BrainOk(_) | Item::Note(_) | Item::Ok { .. } => {}
        }
    }
    Ok(done)
}

fn install_model(human: &mut Vec<String>, done: &mut Vec<Done>) {
    #[cfg(feature = "embed-candle")]
    {
        match mnemo_engine::builtin::install(&mut |m| eprintln!("{m}")) {
            Ok(_) => done.push(Done::new(
                "model: installed".into(),
                vec![(Mark::Add, "the built-in model".into())],
            )),
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
    if !o.undo {
        project::refuse_link(&ctx.brain)?;
    }
    let exists = ctx.brain.is_file();
    // Where each group starts, for a person's view.
    let mut sections = vec![(Section::Brain, 0)];
    if !o.undo {
        if exists {
            let from = check_brain(&ctx.brain)?;
            items.push(Item::BrainOk(from));
        } else {
            items.push(Item::CreateBrain);
        }
    }
    sections.push((Section::Model, items.len()));
    plan_model(o.no_model, o.undo, &mut items);
    if clients.is_empty() {
        sections.push((Section::Other, items.len()));
        items.push(Item::Note(
            "no client found (Claude Code or Cursor): nothing to wire. Pass --client \
             claude-code,cursor to wire one anyway"
                .into(),
        ));
    }
    for c in &clients {
        match c {
            Client::ClaudeCode => {
                sections.push((Section::ClaudeCode, items.len()));
                plan_claude(&ctx, o.undo, &mut items)
            }
            Client::Cursor => {
                sections.push((Section::Cursor, items.len()));
                plan_cursor(&ctx, o.undo, &mut items)
            }
        }
    }
    sections.push((Section::Git, items.len()));
    plan_exclude(&ctx, o.undo, &mut items);
    let person = at_terminal();
    let pen = Pen {
        colour: person && use_colour(),
    };
    if person {
        show_tty(&ctx, &items, &sections, o.undo, &pen);
    } else {
        show(&ctx, &items, o.undo);
    }
    let changes = items.iter().filter(|i| i.is_change()).count();
    if o.print {
        return Ok(());
    }
    // The clients whose files change: a person is asked to restart them.
    let restart: Vec<&str> = sections
        .iter()
        .enumerate()
        .filter(|(i, (sec, start))| {
            let end = sections.get(i + 1).map_or(items.len(), |(_, e)| *e);
            matches!(sec, Section::ClaudeCode | Section::Cursor)
                && items[*start..end].iter().any(Item::is_change)
        })
        .map(|(_, (sec, _))| sec.title())
        .collect();
    let mut human = Vec::new();
    let done = if changes == 0 {
        println!(
            "{}",
            match (o.undo, person) {
                (true, false) => "nothing to remove",
                (false, false) => "already set up",
                (true, true) => "Nothing to remove.",
                (false, true) => "Nothing to change; everything is already set up.",
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
    if person {
        // ONBOARD-UX: a person gets the guide's pointer, not the guide.
        let restart: &[&str] = if done.is_empty() { &[] } else { &restart };
        summary_tty(&ctx, &o, &done, &human, restart, &clients, &pen);
        return Ok(());
    }
    // The guide first, then the summary, so the output ends with the human
    // steps: an agent that reads only the tail still sees them (v0.16.1).
    if !o.undo {
        print!("{}", mnemo_engine::guide::markdown());
    }
    let done: Vec<String> = done.into_iter().map(|d| d.plain).collect();
    let steps = summary(&ctx, &o, &done, &human);
    if steps > 0 && !interactive() {
        println!(
            "mnemo: the human steps above are for the person you work for: pass them on word for \
             word"
        );
    }
    Ok(())
}

/// The summary, with the human steps last. Returns how many there were.
fn summary(ctx: &Ctx, o: &Opts, done: &[String], human: &[String]) -> usize {
    println!();
    for d in done {
        println!("{d}");
    }
    println!("brain: {}", ctx.brain.display());
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
    }
    let steps = human_steps(o, human);
    if let Some(line) = keychain_delete_line(&ctx.brain) {
        println!("to delete this brain's keychain entry: {line}");
    }
    for s in &steps {
        println!("human step: {s}");
    }
    steps.len()
}

/// The steps only the person can do, the trust prompt last (an agent's
/// view; a person's view words the trust step by what's known).
fn human_steps(o: &Opts, human: &[String]) -> Vec<String> {
    let mut steps = person_steps(o, human);
    if !o.undo {
        steps.push(
            "Claude Code and Cursor run hooks only in a trusted workspace: accept the trust \
             prompt when you open this project"
                .into(),
        );
    }
    steps
}

/// The person's steps but the trust prompt.
fn person_steps(o: &Opts, human: &[String]) -> Vec<String> {
    let mut steps: Vec<String> = human.to_vec();
    if !o.undo {
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
    }
    steps
}

/// ONBOARD-UX: the trust step for the clients wired here, only where it may
/// be needed. Claude Code's half goes when its record says the folder is
/// trusted; Cursor keeps no such record we know of.
fn trust_step(ctx: &Ctx, clients: &[Client]) -> Option<String> {
    let mut who = Vec::new();
    if clients.contains(&Client::ClaudeCode) && claude_trusted(ctx) != Some(true) {
        who.push("Claude Code");
    }
    if clients.contains(&Client::Cursor) {
        who.push("Cursor");
    }
    match who.as_slice() {
        [] => None,
        [one] => Some(format!(
            "If {one} asks you to trust this folder, accept: it runs the hooks only in a \
             trusted folder."
        )),
        _ => Some(
            "If Claude Code or Cursor asks you to trust this folder, accept: they run the hooks \
             only in a trusted folder."
                .into(),
        ),
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
