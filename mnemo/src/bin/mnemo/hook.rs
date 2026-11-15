//! 12.2 step 4: `mnemo hook <event> --client <client>`, the hooks `onboard`
//! installs for Claude Code and Cursor (`docs/design/12.2-onboard.md`,
//! sections 4 and 5).
//!
//! - **`session-start`** adds the guide's card, then (after the framing
//!   line) the project's notes, its essence, the session thread's essence
//!   on `resume` and `compact`, and a short recall, to the agent's context:
//!   8,000 bytes at most by default (`--budget`, at most 9,000).
//! - **`pre-compact`** saves the transcript into the session's thread, and
//!   for Claude Code prints compaction instructions (600 bytes at most).
//! - **`session-end`** saves the transcript.
//!
//! The rules: it always exits 0 (a memory tool must never block a session
//! or a compaction), never prompts, never downloads, and never contends
//! with the server: `session-start` reads with a 2 s wait, the saves write
//! in shared mode with the usual wait. Transcripts are saved without tool
//! output (`[tool_result ok|error, N bytes]`) and with known secret shapes
//! redacted.
//!
//! What it takes from its input (stdin, the client's hook JSON): the
//! session id, which must be 1 to 128 characters of `[A-Za-z0-9._-]`
//! because it becomes part of a thread id; the transcript's path, which
//! must be absolute and a regular file; the source (`resume`, `compact`);
//! and the working directory (`cwd`, or Cursor's `workspace_roots[0]`),
//! absolute, as a place to look for the brain. Nothing else from the input
//! reaches a key, a path or a command.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mnemo_engine::keys::Secret;
use mnemo_engine::{guide, redact, EssenceScope, Mnemo, MnemoError, RecallRequest, ThreadTurn};
use serde_json::{json, Value};

use super::project;

/// The hook's events.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Event {
    /// A session starts (or resumes, or was cleared or compacted).
    SessionStart,
    /// The context is about to be compacted.
    PreCompact,
    /// The session ends.
    SessionEnd,
}

/// The client that runs the hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Client {
    /// Claude Code.
    ClaudeCode,
    /// Cursor.
    Cursor,
}

impl Client {
    fn word(self) -> &'static str {
        match self {
            Client::ClaudeCode => "claude-code",
            Client::Cursor => "cursor",
        }
    }
}

/// `session-start`'s default and largest injection.
pub const BUDGET_DEFAULT: usize = 8_000;
pub const BUDGET_MAX: usize = 9_000;
/// `pre-compact`'s compaction instructions, at most.
pub const COMPACT_MAX: usize = 600;
/// How long `session-start` waits for the brain's lock.
const READ_WAIT: Duration = Duration::from_secs(2);
/// `session-start`'s time budget for its own work, before the recall is
/// dropped.
const TIME_BUDGET: Duration = Duration::from_secs(3);
/// The most of a transcript read: its last 64 MiB.
const TRANSCRIPT_MAX: u64 = 64 << 20;
/// The most of the hook's input read.
const INPUT_MAX: u64 = 1 << 20;

/// What a hook run needs from the command line.
pub struct Opts {
    pub event: Event,
    pub client: Client,
    pub budget: Option<usize>,
    /// `-C` moved the process: the brain is found from here.
    pub moved: bool,
}

/// Run the hook. It never fails: every problem is a one-line note, and a
/// panic is caught and noted, so the client always sees exit 0.
pub fn run(o: Opts) {
    let input = read_input();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| event(&o, &input)));
    if r.is_err() {
        eprintln!("mnemo: the hook stopped on an internal error; the session goes on");
    }
}

/// The hook's input, or `{}` when there's none, it's too long, or it isn't
/// JSON. A terminal on stdin (someone typing the command) gives `{}` too.
fn read_input() -> Value {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        return json!({});
    }
    let mut buf = Vec::new();
    let _ = std::io::stdin().take(INPUT_MAX).read_to_end(&mut buf);
    serde_json::from_slice(&buf).unwrap_or_else(|_| json!({}))
}

/// One event, start to end.
pub fn event(o: &Opts, input: &Value) {
    // Cursor runs Claude Code's hooks too ("all matching hooks from every
    // source run"): the Claude Code one stands aside when Cursor runs it.
    if o.client == Client::ClaudeCode && input.get("cursor_version").is_some() {
        return;
    }
    match o.event {
        Event::SessionStart => {
            let text = session_start(o, input);
            print_context(o.client, &text);
        }
        Event::PreCompact | Event::SessionEnd => {
            let id = session_id(o.client, input);
            save_transcript(o, input, id.as_deref());
            if o.event == Event::PreCompact && o.client == Client::ClaudeCode {
                if let Some(id) = id {
                    print!("{}", compact_instructions(o.client, &id));
                }
            }
        }
    }
}

// --- The input -----------------------------------------------------------------

/// The session id, if it's one the hook accepts.
pub fn session_id(client: Client, input: &Value) -> Option<String> {
    let field = match client {
        Client::ClaudeCode => "session_id",
        Client::Cursor => "conversation_id",
    };
    let id = input
        .get(field)
        .or_else(|| input.get("session_id"))
        .and_then(Value::as_str)?;
    valid_id(id).then(|| id.to_string())
}

/// 1 to 128 characters of `[A-Za-z0-9._-]`.
pub fn valid_id(id: &str) -> bool {
    (1..=128).contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The session's thread: `claude-code:<id>` or `cursor:<id>`.
pub fn thread_id(client: Client, id: &str) -> String {
    format!("{}:{id}", client.word())
}

/// An absolute directory the client named in its input.
fn input_dir(client: Client, input: &Value) -> Option<PathBuf> {
    let v = match client {
        Client::ClaudeCode => input.get("cwd").and_then(Value::as_str),
        Client::Cursor => input
            .get("workspace_roots")
            .and_then(|r| r.get(0))
            .and_then(Value::as_str)
            .or_else(|| input.get("cwd").and_then(Value::as_str)),
    }?;
    let p = PathBuf::from(v);
    (p.is_absolute() && p.is_dir()).then_some(p)
}

/// Where the brain is looked for: `-C`, then `CLAUDE_PROJECT_DIR`, then the
/// input's directory, then the working directory.
fn start_dir(o: &Opts, input: &Value) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    if o.moved {
        return cwd;
    }
    if let Some(d) = std::env::var_os("CLAUDE_PROJECT_DIR").map(PathBuf::from) {
        if d.is_absolute() {
            return Some(d);
        }
    }
    input_dir(o.client, input).or(cwd)
}

/// The project brain, or the note that says why there's none.
fn brain(o: &Opts, input: &Value) -> Result<PathBuf, String> {
    let start = start_dir(o, input).ok_or_else(|| NO_BRAIN.to_string())?;
    let common = project::common_dir(&start).map_err(|_| NO_BRAIN.to_string())?;
    let b = common.join(project::DIR).join(project::FILE);
    if b.is_file() {
        Ok(b)
    } else {
        Err(NO_BRAIN.into())
    }
}

const NO_BRAIN: &str = "mnemo: no brain for this project; run `mnemo onboard`";
const NO_KEY: &str = "mnemo: this brain's key isn't available here (`mnemo unlock`)";
const BUSY: &str = "mnemo: the brain was busy; call the `about` tool";
const BAD_ID: &str = "mnemo: the session id isn't one this hook accepts";

/// The brain's secret from the key sources, never a prompt.
fn secret(path: &Path) -> Result<Secret, String> {
    use mnemo_engine::keys::{self, Resolution};
    let store = keys::default_store();
    match keys::resolve(path, store.as_deref(), keys::SERVER_COMMAND_TIMEOUT) {
        Ok(Resolution::Found(r)) => Ok(r.secret),
        _ => Err(NO_KEY.into()),
    }
}

/// A note for an open that failed.
fn open_note(e: &MnemoError) -> String {
    match e {
        MnemoError::Locked { .. } => BUSY.into(),
        MnemoError::FormatTooOld { .. } => {
            "mnemo: this brain is an old format and opens read-only; run `mnemo compact`".into()
        }
        e if e.to_string().contains("passphrase") || e.to_string().contains("key") => NO_KEY.into(),
        e => format!(
            "mnemo: the brain couldn't be read: {}",
            one_line(&e.to_string())
        ),
    }
}

fn one_line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A read-only open with the 2 s wait.
fn open_read(path: &Path, s: &Secret) -> Result<Mnemo, String> {
    let mut opts = mnemo_engine::SharedOptions::default();
    opts.deadline = READ_WAIT;
    match s {
        Secret::Passphrase(p) => Mnemo::open_read_only_waiting(path, p, opts),
        Secret::Key(k) => Mnemo::open_read_only_waiting_with_key(path, k, opts),
    }
    .map_err(|e| open_note(&e))
}

// --- The embedder ----------------------------------------------------------------

/// The embedder the hook may use for a brain of width `dims` that records
/// `identity`: the built-in when its model is already installed and the
/// brain is (or will be) the built-in's; never a download, never a command
/// from a file. `None` otherwise: recall is then keyword-only, and nothing
/// is saved. (A dev-hooks build takes a test command from
/// `MNEMO_DEV_HOOK_EMBED_CMD`.)
fn embedder(
    dims: usize,
    identity: Option<&mnemo_engine::EmbedderIdentity>,
) -> Option<Box<dyn mnemo_engine::Embedder>> {
    #[cfg(feature = "dev-hooks")]
    if let Some(cmd) = std::env::var_os("MNEMO_DEV_HOOK_EMBED_CMD") {
        let name = std::env::var("MNEMO_DEV_HOOK_EMBED_NAME").unwrap_or_else(|_| "dev-hook".into());
        return super::new_embedder(
            Some(&cmd.to_string_lossy()),
            Some(&name),
            true,
            60,
            "",
            Some(dims),
        )
        .ok();
    }
    #[cfg(feature = "embed-candle")]
    {
        let builtin = mnemo_engine::builtin::identity();
        let fits = dims == builtin.dimensions && identity.is_none_or(|i| i.name == builtin.name);
        if fits && super::builtin_ready() {
            if let Some(dir) = super::MODEL_DIR.get().and_then(Option::as_deref) {
                return mnemo_engine::BuiltinEmbedder::from_dir(dir)
                    .ok()
                    .map(|e| Box::new(e) as Box<dyn mnemo_engine::Embedder>);
            }
            return mnemo_engine::BuiltinEmbedder::load()
                .ok()
                .map(|e| Box::new(e) as Box<dyn mnemo_engine::Embedder>);
        }
    }
    let _ = (dims, identity);
    None
}

// --- session-start -----------------------------------------------------------------

/// The parts' shares of what's left after the card and the framing line:
/// the project notes, the project essence, the thread essence, the recall.
const SHARES: [usize; 4] = [800, 2_400, 1_600, 2_000];

/// What `session-start` adds to the context: the card and the framing line,
/// then up to four parts, each first given its share of what's left
/// ([`SHARES`]), then what the others didn't use, in order.
pub fn session_start(o: &Opts, input: &Value) -> String {
    let started = Instant::now();
    let budget = o.budget.unwrap_or(BUDGET_DEFAULT).min(BUDGET_MAX);
    let path = match brain(o, input) {
        Ok(p) => p,
        Err(note) => return note,
    };
    let s = match secret(&path) {
        Ok(s) => s,
        Err(note) => return note,
    };
    let card = guide::card();
    let head = format!("{card}\n\n{}\n", guide::FRAMING);
    let room = budget.saturating_sub(head.len() + 200);
    let resume = matches!(
        input.get("source").and_then(Value::as_str),
        Some("resume") | Some("compact")
    );
    let thread = session_id(o.client, input).map(|id| thread_id(o.client, &id));

    // The parts' full texts, under one short read.
    let mut parts: Vec<(&str, String)> = Vec::new();
    let (essence, width, identity) = {
        let mut db = match open_read(&path, &s) {
            Ok(db) => db,
            Err(note) => return note,
        };
        // 2. Project notes: the briefing (the manifest first).
        let notes = db
            .about()
            .map(|ms| {
                ms.iter()
                    .map(|m| m.content.trim().to_string())
                    .collect::<Vec<_>>()
                    .join("\n\n")
            })
            .unwrap_or_default();
        parts.push(("Project notes", notes));
        // 3. The project essence.
        let mut essence = String::new();
        if matches!(db.about_essence(), Ok(Some(_))) {
            essence = db
                .essence(EssenceScope::Project, room.max(256))
                .unwrap_or_default();
        }
        parts.push(("Project essence", essence.trim().to_string()));
        // 4. The session thread's essence, on resume and compact.
        let mut t4 = String::new();
        if let (true, Some(t)) = (resume, &thread) {
            if let Ok(r) = db.essence(EssenceScope::Thread(t.clone()), room.max(256)) {
                if r.lines().any(|l| l.starts_with("## ")) {
                    t4 = r.trim().to_string();
                }
            }
        }
        parts.push(("This session's thread", t4));
        let width = db.dimensions();
        let identity = db.embedder_identity().ok().flatten();
        (essence, width, identity)
    };

    // 5. A recall, on its own thread, dropped if it's late.
    let mut late = false;
    let query = recall_query(start_dir(o, input).as_deref(), &essence);
    let mut lines: Vec<String> = Vec::new();
    if !query.is_empty() {
        let left = TIME_BUDGET.saturating_sub(started.elapsed());
        let (tx, rx) = std::sync::mpsc::channel();
        let path2 = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(recall(&path2, &s, &query, width, identity.as_ref(), room));
        });
        match rx.recv_timeout(left) {
            Ok(Some(l)) => lines = l,
            Ok(None) => {}
            Err(_) => late = true,
        }
    }
    parts.push(("Relevant memories", lines.join("\n")));

    // Each part's room: its share, then what the others left, in order.
    let section = |title: &str| format!("\n## {title}\n\n").len() + 1;
    let need: Vec<usize> = parts
        .iter()
        .map(|(t, b)| {
            if b.is_empty() {
                0
            } else {
                b.len() + section(t)
            }
        })
        .collect();
    let total: usize = SHARES.iter().sum();
    let mut alloc: Vec<usize> = need
        .iter()
        .zip(SHARES)
        .map(|(n, sh)| (*n).min(room * sh / total))
        .collect();
    let mut left = room.saturating_sub(alloc.iter().sum());
    for (a, n) in alloc.iter_mut().zip(&need) {
        let more = n.saturating_sub(*a).min(left);
        *a += more;
        left -= more;
    }
    let mut out = head;
    for (((title, body), a), last) in parts.iter().zip(&alloc).zip([false, false, false, true]) {
        if body.is_empty() || *a <= section(title) + 16 {
            continue;
        }
        let space = a - section(title);
        let text = if last {
            // Whole lines only.
            let mut kept = String::new();
            for l in body.lines() {
                if kept.len() + l.len() + 1 > space {
                    break;
                }
                kept.push_str(l);
                kept.push('\n');
            }
            kept.trim_end().to_string()
        } else {
            fit(body, space)
        };
        if !text.is_empty() {
            out.push_str(&format!("\n## {title}\n\n{text}\n"));
        }
    }
    if late {
        out.push_str(
            "\n(mnemo: the recall was dropped: it took longer than the hook's 3 s; \
             call `recall` with your task)\n",
        );
    }
    fit(&out, budget.max(card.len() + guide::FRAMING.len() + 2))
}

/// The recall's query: the branch, the last three commit subjects, and the
/// project essence's `next` items.
fn recall_query(dir: Option<&Path>, essence: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(d) = dir {
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(d)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default()
        };
        let branch = git(&["rev-parse", "--abbrev-ref", "HEAD"]);
        if !branch.is_empty() && branch != "HEAD" {
            parts.push(branch.replace(['/', '-', '_'], " "));
        }
        let log = git(&["log", "-3", "--format=%s"]);
        parts.extend(log.lines().map(str::to_string));
    }
    let mut in_next = false;
    for l in essence.lines() {
        if let Some(h) = l.strip_prefix("## ") {
            in_next = h.trim() == "next";
            continue;
        }
        if in_next {
            if let Some(item) = l.strip_prefix("- ") {
                let item = item.split_once("] ").map_or(item, |(_, t)| t);
                parts.push(item.to_string());
            }
        }
    }
    let q = parts.join(". ");
    fit_plain(&q, 1_000)
}

/// The recall's lines, within `max` bytes, not counted as accesses.
fn recall(
    path: &Path,
    s: &Secret,
    query: &str,
    width: usize,
    identity: Option<&mnemo_engine::EmbedderIdentity>,
    max: usize,
) -> Option<Vec<String>> {
    // Embed with no lock held.
    let vector = match embedder(width, identity) {
        Some(e) => mnemo_engine::Embedder::embed_query(e.as_ref(), &[query])
            .ok()
            .and_then(|mut v| v.pop()),
        None => None,
    };
    let mut db = open_read(path, s).ok()?;
    let req = match vector {
        Some(v) => RecallRequest::new(v).text(query),
        None if db.stats_summary().format_version >= 9 => {
            RecallRequest::new(Vec::new()).text(query).keywords(query)
        }
        None => return None,
    };
    let req = req.top_k(5).track_access(false).max_bytes(max.max(256));
    let resp = db.recall_budgeted(&req).ok()?;
    Some(
        resp.hits
            .iter()
            .map(|h| {
                let m = &h.memory;
                let key = m
                    .metadata
                    .get(mnemo_engine::MNEMO_META_KEY)
                    .and_then(Value::as_str)
                    .map(|k| format!(", key `{k}`"))
                    .unwrap_or_default();
                format!(
                    "- `{}` {}{key}: {}",
                    m.id,
                    m.memory_type.as_str(),
                    fit_plain(&one_line(&m.content), 600)
                )
            })
            .collect(),
    )
}

/// `text` within `max` bytes, cut at a character boundary with a note.
fn fit(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let note = "\n[cut to fit]";
    let mut end = max.saturating_sub(note.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{note}", text[..end].trim_end())
}

/// `text` within `max` bytes, cut at a character boundary with `…`.
fn fit_plain(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Print `session-start`'s context in the client's shape.
fn print_context(client: Client, text: &str) {
    let v = match client {
        Client::ClaudeCode => json!({
            "hookSpecificOutput": {
                "hookEventName": "SessionStart",
                "additionalContext": text,
            }
        }),
        Client::Cursor => json!({ "additional_context": text }),
    };
    println!("{v}");
}

// --- pre-compact and session-end -------------------------------------------------

/// Claude Code's compaction instructions (its PreCompact stdout).
pub fn compact_instructions(client: Client, id: &str) -> String {
    let t = format!(
        "Keep in the summary: this session's earlier turns are saved in mnemo as thread `{}`; \
         the `essence` and `recall` tools bring them back. Before going on, save any durable \
         fact or decision from this part of the session with `remember` and a `key`.\n",
        thread_id(client, id)
    );
    fit_plain(&t, COMPACT_MAX)
}

/// Save the session's transcript into its thread. Notes go to stderr.
fn save_transcript(o: &Opts, input: &Value, id: Option<&str>) {
    let Some(id) = id else {
        eprintln!("{BAD_ID}");
        return;
    };
    let path = match brain(o, input) {
        Ok(p) => p,
        Err(note) => return eprintln!("{note}"),
    };
    let transcript = input
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CURSOR_TRANSCRIPT_PATH").map(PathBuf::from));
    let Some(transcript) = transcript else {
        return eprintln!("mnemo: no transcript path in the hook's input");
    };
    let mut turns = match read_transcript(&transcript) {
        Ok(t) => t,
        Err(note) => return eprintln!("{note}"),
    };
    if turns.is_empty() {
        if o.client == Client::Cursor {
            eprintln!("mnemo: the transcript had no turns this hook reads");
        }
        return;
    }
    let counts = redact_turns(&mut turns);
    if let Some(line) = redaction_note(&counts) {
        eprintln!("{line}");
    }
    let s = match secret(&path) {
        Ok(s) => s,
        Err(note) => return eprintln!("{note}"),
    };
    let thread = thread_id(o.client, id);
    let p = path.to_string_lossy().into_owned();
    // The brain's identity, to pick the embedder (a short read).
    let identity = match open_read(&path, &s) {
        Ok(mut db) => db.embedder_identity().ok().flatten(),
        Err(note) => return eprintln!("{note}"),
    };
    let r = super::ingest_turns(
        &p,
        &s,
        &thread,
        &turns,
        "the hook's transcript save",
        |dims| {
            embedder(dims, identity.as_ref()).ok_or_else(|| {
                "mnemo: the transcript wasn't saved: the built-in model isn't installed \
             (`mnemo embedder install`)"
                    .to_string()
            })
        },
    );
    if let Err(e) = r {
        let e = if e.starts_with("mnemo:") {
            e
        } else if e.contains("ocked") || e.contains("busy") {
            format!(
                "mnemo: the brain was busy; the next save catches up ({})",
                one_line(&e)
            )
        } else {
            format!("mnemo: the transcript wasn't saved: {}", one_line(&e))
        };
        eprintln!("{e}");
    }
}

/// Read a transcript: an absolute path to a regular file (a FIFO or a
/// device would block the hook), through the handle it was checked on. Over
/// 64 MiB, its last 64 MiB, from the first whole line.
fn read_transcript(path: &Path) -> Result<Vec<ThreadTurn>, String> {
    if !path.is_absolute() {
        return Err("mnemo: the transcript path isn't absolute".into());
    }
    let not_file = || {
        format!(
            "mnemo: the transcript isn't a regular file: {}",
            path.display()
        )
    };
    // Before the open: a FIFO's open would wait for a writer.
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() => {}
        Ok(_) => return Err(not_file()),
        Err(e) => return Err(format!("mnemo: the transcript can't be read: {e}")),
    }
    let mut f =
        open_nonblocking(path).map_err(|e| format!("mnemo: the transcript can't be read: {e}"))?;
    // After the open, on the handle: the file the path named then.
    let len = match f.metadata() {
        Ok(m) if m.is_file() => m.len(),
        _ => return Err(not_file()),
    };
    let mut buf = Vec::new();
    if len > TRANSCRIPT_MAX {
        eprintln!("mnemo: the transcript is over 64 MiB; its last 64 MiB were read");
        f.seek(SeekFrom::Start(len - TRANSCRIPT_MAX))
            .map_err(|e| format!("mnemo: the transcript can't be read: {e}"))?;
        f.take(TRANSCRIPT_MAX)
            .read_to_end(&mut buf)
            .map_err(|e| format!("mnemo: the transcript can't be read: {e}"))?;
        // The first line is cut: start after it.
        let first = buf
            .iter()
            .position(|&b| b == b'\n')
            .map_or(buf.len(), |i| i + 1);
        buf.drain(..first);
    } else {
        f.read_to_end(&mut buf)
            .map_err(|e| format!("mnemo: the transcript can't be read: {e}"))?;
    }
    let opts = mnemo_engine::TranscriptOptions {
        tool_results: mnemo_engine::ToolResults::Omit,
        lenient: true,
    };
    mnemo_engine::read_transcript_with(
        &buf[..],
        mnemo_engine::TranscriptFormat::ClaudeCodeJsonl,
        opts,
    )
    .map_err(|e| {
        format!(
            "mnemo: the transcript can't be read: {}",
            one_line(&e.to_string())
        )
    })
}

/// Open for reading without waiting on a FIFO (Unix).
#[cfg(unix)]
fn open_nonblocking(path: &Path) -> std::io::Result<std::fs::File> {
    use rustix::fs::{Mode, OFlags};
    let fd = rustix::fs::open(
        path,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    Ok(std::fs::File::from(fd))
}

#[cfg(not(unix))]
fn open_nonblocking(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::open(path)
}

/// Redact every turn's content alone; the counts by kind.
pub fn redact_turns(turns: &mut [ThreadTurn]) -> Vec<(&'static str, usize)> {
    let mut all = Vec::new();
    for t in turns.iter_mut() {
        let r = redact::secrets(&t.content);
        if r.total() > 0 {
            redact::add_counts(&mut all, &r.counts);
            t.content = r.text.into_owned();
        }
    }
    all
}

/// "mnemo: 2 secrets redacted (github-token, aws-key-id)", or `None`.
pub fn redaction_note(counts: &[(&'static str, usize)]) -> Option<String> {
    let n: usize = counts.iter().map(|(_, n)| n).sum();
    if n == 0 {
        return None;
    }
    let kinds: Vec<&str> = counts.iter().map(|(k, _)| *k).collect();
    Some(format!(
        "mnemo: {n} secret{} redacted ({})",
        if n == 1 { "" } else { "s" },
        kinds.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_ids() {
        for ok in ["abc", "0f8e-11.x_Y", &"a".repeat(128)] {
            assert!(valid_id(ok), "{ok}");
        }
        for bad in ["", "a/b", "a b", "../x", "a:b", &"a".repeat(129), "é"] {
            assert!(!valid_id(bad), "{bad}");
        }
        let input = json!({"session_id": "../../etc"});
        assert_eq!(session_id(Client::ClaudeCode, &input), None);
        let input = json!({"conversation_id": "c-1", "cursor_version": "1.0"});
        assert_eq!(session_id(Client::Cursor, &input).as_deref(), Some("c-1"));
    }

    #[test]
    fn the_compaction_instructions_fit() {
        let t = compact_instructions(Client::ClaudeCode, &"x".repeat(128));
        assert!(t.len() <= COMPACT_MAX, "{}", t.len());
        assert!(t.contains("claude-code:"));
    }

    #[test]
    fn the_redaction_note_names_kinds_not_values() {
        assert_eq!(redaction_note(&[]), None);
        assert_eq!(
            redaction_note(&[("aws-key-id", 1), ("github-token", 1)]).unwrap(),
            "mnemo: 2 secrets redacted (aws-key-id, github-token)"
        );
        assert_eq!(
            redaction_note(&[("jwt", 1)]).unwrap(),
            "mnemo: 1 secret redacted (jwt)"
        );
    }

    #[test]
    fn the_query_takes_the_essences_next_items() {
        let essence = "# essence — project\n\n## decisions\n- [open] not this\n## next\n- [open] fix the upload test\n- [resolved] ship it\n## open\n- [open] nor this\n";
        let q = recall_query(None, essence);
        assert_eq!(q, "fix the upload test. ship it");
    }
}
