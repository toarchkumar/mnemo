//! 12.2 step 3: the MCP server's resources and prompts
//! (`docs/design/12.2-onboard.md`, section 7).
//!
//! - **Resources** (read-only, `text/markdown`): `mnemo://about`,
//!   `mnemo://recent`, `mnemo://guide`, and the template
//!   `mnemo://key/{key}`. Each has a byte budget. Reads don't count as
//!   accesses, and a shared server reads under the shared lock.
//! - **Prompts:** `session-start` (an optional `task`) and `session-end`
//!   (an optional `thread`), 6,000 bytes each at most.
//!
//! Everything shown from the brain sits under [`guide::FRAMING`]: memories
//! are written by agents, often from untrusted input, so they're data.

use super::*;

use crate::guide;
use crate::memory::{EssenceScope, KeyScope, MNEMO_META_KEY};
use crate::store::format_unix_date;

/// JSON-RPC's code for a resource that isn't there (MCP's "resource not
/// found").
const RESOURCE_NOT_FOUND: i32 = -32002;

/// `mnemo://about`'s budget.
pub(super) const ABOUT_BYTES: usize = 8_000;
/// `mnemo://recent`'s budget, and how many memories and bytes of each.
pub(super) const RECENT_BYTES: usize = 6_000;
const RECENT_COUNT: usize = 20;
const RECENT_ITEM_BYTES: usize = 300;
/// `mnemo://key/{key}`'s budget.
pub(super) const KEY_BYTES: usize = 4_000;
/// Each prompt's budget.
pub(super) const PROMPT_BYTES: usize = 6_000;

const MD: &str = "text/markdown";

/// `text` cut at a character boundary to at most `max` bytes, with a note
/// when it was cut.
pub(super) fn fit(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let note = format!("\n\n[cut at {max} bytes]");
    let mut end = max.saturating_sub(note.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{note}", text[..end].trim_end())
}

/// One line for a memory: its id, type, key and date, then its content on
/// one line, cut at `max` bytes.
fn memory_line(m: &Memory, max: usize) -> String {
    let mut head = format!("- `{}` {}", m.id, m.memory_type.as_str());
    if let Some(k) = m.metadata.get(MNEMO_META_KEY).and_then(Value::as_str) {
        head.push_str(&format!(", key `{k}`"));
    }
    head.push_str(&format!(", {}", format_unix_date(m.created_at)));
    let one_line: String = m.content.split_whitespace().collect::<Vec<_>>().join(" ");
    format!(
        "{head}: {}",
        guide::escape_brain(&fit_inline(&one_line, max))
    )
}

/// `text` cut to `max` bytes at a character boundary, with `…`.
fn fit_inline(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max.saturating_sub(3);
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Run `f` on the server's handle: directly, or in a shared read window.
fn read_with<T>(
    db: &mut ServerDb,
    f: impl FnOnce(&mut Mnemo) -> std::result::Result<T, RpcError>,
) -> std::result::Result<T, RpcError> {
    match db {
        ServerDb::Plain(m) => f(m),
        ServerDb::Shared(s) => {
            let mut err: Option<RpcError> = None;
            let r = s.read(|m| {
                f(m).map_err(|e| {
                    let msg = e.message.clone();
                    err = Some(e);
                    MnemoError::Invalid(msg)
                })
            });
            r.map_err(|e| err.unwrap_or_else(|| mnemo_err(e)))
        }
    }
}

// --- Resources ---------------------------------------------------------------

pub(super) fn resources_list() -> Value {
    json!({ "resources": [
        {
            "uri": "mnemo://about",
            "name": "about",
            "title": "About this brain",
            "description": "The guide's card, then this brain's briefing (the manifest and onboarding memories) and the project essence.",
            "mimeType": MD,
        },
        {
            "uri": "mnemo://recent",
            "name": "recent",
            "title": "Recent memories",
            "description": "The 20 most recently written memories, newest first.",
            "mimeType": MD,
        },
        {
            "uri": "mnemo://guide",
            "name": "guide",
            "title": "The MNemo guide",
            "description": "How to set up a project and how to organize its memory. Built into this server.",
            "mimeType": MD,
        },
    ]})
}

pub(super) fn resource_templates_list() -> Value {
    json!({ "resourceTemplates": [
        {
            "uriTemplate": "mnemo://key/{key}",
            "name": "key",
            "title": "A key's value and history",
            "description": "The live value of a key, then its earlier versions.",
            "mimeType": MD,
        },
    ]})
}

/// Undo `%XX` escapes in a URI part; anything malformed is left as it is.
/// Works on bytes, so a `%` before a character wider than a byte is just
/// left (RB-011.1).
fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let (Some(h), Some(l)) = (
                b.get(i + 1).copied().and_then(hex),
                b.get(i + 2).copied().and_then(hex),
            ) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

pub(super) fn resources_read(
    db: &mut ServerDb,
    params: &Value,
) -> std::result::Result<Value, RpcError> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "resources/read: missing 'uri'"))?
        .to_string();
    let not_found = || RpcError::new(RESOURCE_NOT_FOUND, format!("resource not found: {uri}"));
    let text = match uri.as_str() {
        "mnemo://guide" => guide::markdown(),
        "mnemo://about" => read_with(db, about_text)?,
        "mnemo://recent" => read_with(db, recent_text)?,
        other => match other.strip_prefix("mnemo://key/") {
            Some(k) if !k.is_empty() => {
                let key = percent_decode(k);
                read_with(db, |m| key_text(m, &key))?.ok_or_else(not_found)?
            }
            _ => return Err(not_found()),
        },
    };
    Ok(json!({ "contents": [ { "uri": uri, "mimeType": MD, "text": text } ] }))
}

/// `mnemo://about`: the card, the framing line, the counts (as `mnemo
/// about`'s header, v0.17.2), the onboarding memories (the manifest first)
/// and the project essence.
pub(super) fn about_text(m: &mut Mnemo) -> std::result::Result<String, RpcError> {
    let c = m.kind_counts();
    // v0.21.0: tombstones aren't the brain's content.
    let memories = m
        .stats()
        .map_err(mnemo_err)?
        .memories
        .saturating_sub(c.tombstones);
    let mut out = format!(
        "{}\n\n{}\n\n## About this brain\n\n{memories} memories: {} facts, {} turns, {} essence, \
         {} coordination\n",
        guide::card(),
        guide::FRAMING,
        c.facts,
        c.turns,
        c.essence,
        c.coordination
    );
    let mems = m.about().map_err(mnemo_err)?;
    if mems.is_empty() {
        out.push_str("\n(no briefing yet)\n");
    }
    for mem in &mems {
        out.push_str(&format!("\n{}\n", guide::escape_brain(mem.content.trim())));
    }
    out.push_str("\n## Project essence\n\n");
    match m.about_essence().map_err(mnemo_err)? {
        Some(r) => out.push_str(&guide::escape_brain(r.trim())),
        None => out.push_str("(none yet)"),
    }
    out.push('\n');
    Ok(fit(&out, ABOUT_BYTES))
}

/// `mnemo://recent`: the framing line, then the most recent memories.
pub(super) fn recent_text(m: &mut Mnemo) -> std::result::Result<String, RpcError> {
    let mut out = format!("{}\n\n## Recent memories, newest first\n\n", guide::FRAMING);
    let mems = m.recent(RECENT_COUNT).map_err(mnemo_err)?;
    if mems.is_empty() {
        out.push_str("(none yet)\n");
    }
    for mem in &mems {
        let line = memory_line(mem, RECENT_ITEM_BYTES);
        if out.len() + line.len() + 1 > RECENT_BYTES {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(fit(&out, RECENT_BYTES))
}

/// `mnemo://key/{key}`: the framing line, the live value, then the earlier
/// versions. The default agent's slot, else the shared one; `None` when the
/// key has neither.
pub(super) fn key_text(m: &mut Mnemo, key: &str) -> std::result::Result<Option<String>, RpcError> {
    let mut chain = m.history(key, None).map_err(mnemo_err)?;
    if chain.is_empty() {
        chain = m.history_in(key, KeyScope::Shared).map_err(mnemo_err)?;
    }
    // RB-013.2: a resource can't ask for private records. A private live
    // version reads as no such key; private earlier versions are left out.
    let shown = crate::VisibilitySet::DEFAULT;
    if chain.first().is_some_and(|m| !shown.shows(m)) {
        return Ok(None);
    }
    chain.retain(|m| shown.shows(m));
    let Some((live, earlier)) = chain.split_first() else {
        return Ok(None);
    };
    let mut out = format!(
        "{}\n\n## `{key}`\n\n{}\n\nWritten {}, id `{}`.\n",
        guide::FRAMING,
        guide::escape_brain(live.content.trim()),
        format_unix_date(live.created_at),
        live.id
    );
    if !earlier.is_empty() {
        out.push_str("\n### Earlier versions, newest first\n\n");
        for e in earlier {
            let line = memory_line(e, 200);
            if out.len() + line.len() + 1 > KEY_BYTES {
                break;
            }
            out.push_str(&line);
            out.push('\n');
        }
    }
    Ok(Some(fit(&out, KEY_BYTES)))
}

// --- Prompts -----------------------------------------------------------------

pub(super) fn prompts_list() -> Value {
    json!({ "prompts": [
        {
            "name": "session-start",
            "title": "Start a session with this project's memory",
            "description": "The guide's card, the project essence, and the memories relevant to the task (or the most recent ones).",
            "arguments": [
                { "name": "task", "description": "What this session is about, in a sentence.", "required": false },
            ],
        },
        {
            "name": "session-end",
            "title": "Save what lasts before ending",
            "description": "A reminder to save durable facts and decisions with a key, and the thread's open items.",
            "arguments": [
                { "name": "thread", "description": "The thread whose open items to show.", "required": false },
            ],
        },
    ]})
}

const START_END: &str =
    "Start on the task. Save durable facts with `remember` and a `key` as you go.";
const END_REMINDER: &str = "Before ending, save what lasts: each decision or fact that \
replaces an earlier one with `remember` and a `key`; open items and the next step with \
`essence`.";

fn prompt_arg<'a>(params: &'a Value, name: &str) -> Option<&'a str> {
    params
        .get("arguments")
        .and_then(|a| a.get(name))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub(super) fn prompts_get(
    db: &mut ServerDb,
    params: &Value,
) -> std::result::Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "prompts/get: missing 'name'"))?;
    let (description, text) = match name {
        "session-start" => {
            let task = prompt_arg(params, "task").map(str::to_string);
            // A shared server embeds the task before it takes the lock.
            if let (Some(t), ServerDb::Shared(s)) = (&task, &*db) {
                pre_embed(s, &json!({ "name": "recall", "arguments": { "text": t } }))?;
            }
            let r = read_with(db, |m| session_start_text(m, task.as_deref()));
            PRE_EMBEDDED.with(|p| p.borrow_mut().take());
            ("Start a session with this project's memory", r?)
        }
        "session-end" => {
            let thread = prompt_arg(params, "thread").map(str::to_string);
            let text = match thread {
                None => END_REMINDER.to_string(),
                Some(t) => read_with(db, |m| session_end_text(m, &t))?,
            };
            ("Save what lasts before ending", text)
        }
        other => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("unknown prompt '{other}' (there are session-start and session-end)"),
            ))
        }
    };
    Ok(json!({
        "description": description,
        "messages": [ { "role": "user", "content": { "type": "text", "text": text } } ],
    }))
}

/// `session-start`: the card, the framing line, the project essence, then a
/// budgeted recall for the task (or, with no task, the most recent
/// memories), within [`PROMPT_BYTES`]. The recall doesn't count as an
/// access.
pub(super) fn session_start_text(
    m: &mut Mnemo,
    task: Option<&str>,
) -> std::result::Result<String, RpcError> {
    let mut head = format!("{}\n\n", guide::card());
    if let Some(t) = task {
        head.push_str(&format!("The task: {}\n\n", fit_inline(t, 500)));
    }
    head.push_str(guide::FRAMING);
    head.push_str("\n\n## Project essence\n\n");
    let tail = format!("\n\n{START_END}");
    let room = PROMPT_BYTES.saturating_sub(head.len() + tail.len() + 64);
    let essence_room = room * 2 / 5;
    let essence = match m.about_essence().map_err(mnemo_err)? {
        Some(_) => m
            .essence(EssenceScope::Project, essence_room.max(256))
            .map_err(mnemo_err)?,
        None => "(none yet)".to_string(),
    };
    let mut out = head;
    out.push_str(&guide::escape_brain(&fit(
        essence.trim(),
        essence_room.max(64),
    )));
    let memory_room = PROMPT_BYTES.saturating_sub(out.len() + tail.len() + 64);
    let (title, mems) = match task {
        Some(t) => (
            "Memories relevant to the task",
            task_recall(m, t, memory_room)?,
        ),
        None => ("Recent memories", m.recent(5).map_err(mnemo_err)?),
    };
    out.push_str(&format!("\n\n## {title}\n\n"));
    if mems.is_empty() {
        out.push_str("(none yet)");
    }
    for mem in &mems {
        let line = memory_line(mem, RECENT_ITEM_BYTES * 2);
        if out.len() + line.len() + 1 + tail.len() > PROMPT_BYTES {
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out.push_str(&tail);
    Ok(fit(&out, PROMPT_BYTES))
}

/// The five memories most relevant to `task`, within `max` bytes: hybrid
/// with the server's embedder, by keyword without one (v9).
fn task_recall(
    m: &mut Mnemo,
    task: &str,
    max: usize,
) -> std::result::Result<Vec<Memory>, RpcError> {
    let has_embedder = m.embedder().is_some();
    let req = if has_embedder {
        let v = embed_text_via_server(m, "recall", task)?;
        RecallRequest::new(v).text(task)
    } else if m.stats_summary().format_version >= 9 {
        RecallRequest::new(Vec::new()).text(task).keywords(task)
    } else {
        return Ok(Vec::new());
    };
    let req = req.top_k(5).track_access(false).max_bytes(max.max(256));
    let resp = m.recall_budgeted(&req).map_err(mnemo_err)?;
    Ok(resp.hits.into_iter().map(|h| h.memory).collect())
}

/// `session-end` with a thread: the reminder, then the framing line and
/// the thread's essence.
pub(super) fn session_end_text(
    m: &mut Mnemo,
    thread: &str,
) -> std::result::Result<String, RpcError> {
    let head = format!(
        "{END_REMINDER}\n\n{}\n\n## Thread `{thread}`\n\n",
        guide::FRAMING
    );
    let room = PROMPT_BYTES.saturating_sub(head.len() + 64);
    let render = m
        .essence(EssenceScope::Thread(thread.to_string()), room.max(256))
        .map_err(mnemo_err)?;
    Ok(fit(
        &format!("{head}{}", guide::escape_brain(render.trim())),
        PROMPT_BYTES,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_cuts_at_a_character_boundary_and_says_so() {
        assert_eq!(fit("short", 100), "short");
        let long = "é".repeat(100);
        let cut = fit(&long, 60);
        assert!(cut.len() <= 60, "{}", cut.len());
        assert!(cut.ends_with("[cut at 60 bytes]"), "{cut}");
        assert!(fit_inline(&long, 21).len() <= 21);
        assert!(fit_inline(&long, 21).ends_with('…'));
    }

    #[test]
    fn percent_decode_undoes_escapes_and_keeps_the_rest() {
        assert_eq!(percent_decode("process%2Erelease"), "process.release");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("plain.key"), "plain.key");
        assert_eq!(percent_decode("bad%zz%"), "bad%zz%");
        assert_eq!(percent_decode("end%2"), "end%2");
        // A `%` before a character wider than a byte (RB-011.1: these
        // panicked), and escapes that make one.
        assert_eq!(percent_decode("50%→60%"), "50%→60%");
        assert_eq!(percent_decode("a%é"), "a%é");
        assert_eq!(percent_decode("%😀%"), "%😀%");
        assert_eq!(percent_decode("caf%C3%A9"), "café");
        assert_eq!(percent_decode("bad%FF"), "bad%FF");
    }
}
