//! Phase 12.7a transcript parsers.
//!
//! Turns a file on disk (or an in-memory reader) into a `Vec<ThreadTurn>`
//! the [`crate::Mnemo::ingest_thread`] path expects. Two formats land in
//! v0.9.0; the general shape is one JSON object per line.
//!
//! * [`TranscriptFormat::JsonlGeneric`] — the documented shape anyone
//!   can produce:
//!   ```json
//!   {"turn_id": "...", "role": "user", "content": "...", "timestamp": 1234567890}
//!   ```
//!   `timestamp` is optional (unix seconds). Unknown keys are ignored.
//!
//! * [`TranscriptFormat::ClaudeCodeJsonl`] — Claude Code writes its
//!   session transcripts at `~/.claude/projects/<project>/<session>.jsonl`
//!   with one JSON object per line. Each line is tagged with a `type`
//!   field (`"message"`, `"system"`, `"tool_use"`, …) and carries a
//!   `uuid` the parser uses as `turn_id`. Only `type == "user"` or
//!   `type == "assistant"` lines yield turns; every other line type is
//!   skipped so new line types don't break the parser.

use std::io::{BufRead, BufReader, Read};

use serde_json::Value;

use crate::error::{MnemoError, Result};
use crate::memory::ThreadTurn;

/// The supported transcript shapes for [`read_transcript`] /
/// `mnemo thread ingest --format …`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptFormat {
    /// `{"turn_id": …, "role": …, "content": …, "timestamp"?: …}` per line.
    JsonlGeneric,
    /// Claude Code's `~/.claude/projects/<p>/<s>.jsonl` shape; see the
    /// module docs.
    ClaudeCodeJsonl,
}

impl TranscriptFormat {
    /// Parse a `--format` string. Case-insensitive; accepts `jsonl` and
    /// `claude-code-jsonl` (or `claude_code_jsonl` for CLI convenience).
    pub fn parse(s: &str) -> Option<TranscriptFormat> {
        match s.to_ascii_lowercase().as_str() {
            "jsonl" => Some(TranscriptFormat::JsonlGeneric),
            "claude-code-jsonl" | "claude_code_jsonl" => Some(TranscriptFormat::ClaudeCodeJsonl),
            _ => None,
        }
    }
}

/// Read a transcript from any `Read` in the given format and return the
/// turns in file order. Non-matching / unrecognized line types are
/// skipped silently (Claude Code adds new `type` values over time).
pub fn read_transcript<R: Read>(reader: R, format: TranscriptFormat) -> Result<Vec<ThreadTurn>> {
    let mut out = Vec::new();
    for (lineno, line) in BufReader::new(reader).lines().enumerate() {
        let line = line.map_err(MnemoError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let v: Value = serde_json::from_str(trimmed).map_err(|e| {
            MnemoError::Invalid(format!("transcript line {}: invalid JSON: {e}", lineno + 1))
        })?;
        let turn = match format {
            TranscriptFormat::JsonlGeneric => parse_generic_line(&v)?,
            TranscriptFormat::ClaudeCodeJsonl => parse_claude_code_line(&v)?,
        };
        if let Some(t) = turn {
            out.push(t);
        }
    }
    Ok(out)
}

fn parse_generic_line(v: &Value) -> Result<Option<ThreadTurn>> {
    let obj = v.as_object().ok_or_else(|| {
        MnemoError::Invalid("transcript: generic jsonl line must be a JSON object".into())
    })?;
    let turn_id = obj
        .get("turn_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            MnemoError::Invalid("transcript: generic jsonl line missing 'turn_id' (string)".into())
        })?
        .to_string();
    let role = obj
        .get("role")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            MnemoError::Invalid("transcript: generic jsonl line missing 'role' (string)".into())
        })?
        .to_string();
    let content = obj
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            MnemoError::Invalid("transcript: generic jsonl line missing 'content' (string)".into())
        })?
        .to_string();
    let timestamp = obj.get("timestamp").and_then(|v| v.as_i64());
    Ok(Some(ThreadTurn {
        turn_id,
        role,
        content,
        timestamp,
    }))
}

/// Parse one Claude Code jsonl line. Returns `None` for any line that
/// isn't a user/assistant message (system init records, tool_use
/// scaffolding, etc.) so new line types added upstream don't break
/// ingest.
///
/// Expected shape (trimmed to the fields we read):
/// ```json
/// {"type": "user", "uuid": "…", "message": {"role": "user", "content": "…"}}
/// {"type": "assistant", "uuid": "…", "message": {"role": "assistant", "content": [{"type":"text","text":"…"}]}}
/// ```
///
/// `message.content` can be either a plain string (older captures) or a
/// list of content blocks each with `{"type": "text", "text": "…"}`.
/// Non-text blocks (tool_use, tool_result, image) are flattened into a
/// readable text summary so the archive stays browsable without being
/// lossless about the original block structure.
fn parse_claude_code_line(v: &Value) -> Result<Option<ThreadTurn>> {
    let obj = match v.as_object() {
        Some(o) => o,
        None => return Ok(None),
    };
    let type_tag = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if type_tag != "user" && type_tag != "assistant" {
        // Skip system init, summary, and other non-message types.
        return Ok(None);
    }
    let turn_id = obj
        .get("uuid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            MnemoError::Invalid("transcript: claude-code-jsonl message missing 'uuid'".into())
        })?;
    let message = obj
        .get("message")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            MnemoError::Invalid(
                "transcript: claude-code-jsonl message missing 'message' object".into(),
            )
        })?;
    let role = message
        .get("role")
        .and_then(|v| v.as_str())
        .unwrap_or(type_tag)
        .to_string();
    let content = match message.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => flatten_blocks(blocks),
        _ => String::new(),
    };
    // Timestamps in Claude Code jsonl live under `message.timestamp`
    // (ISO 8601) or the line's top-level `timestamp`. Convert ISO 8601
    // to unix seconds with a minimal parser; on any failure, let the
    // ingest path pick `now()`.
    let timestamp = obj
        .get("timestamp")
        .and_then(|v| v.as_str())
        .and_then(parse_iso8601_secs)
        .or_else(|| {
            message
                .get("timestamp")
                .and_then(|v| v.as_str())
                .and_then(parse_iso8601_secs)
        });
    Ok(Some(ThreadTurn {
        turn_id,
        role,
        content,
        timestamp,
    }))
}

/// Flatten a Claude Code `message.content` array of blocks into a
/// single string. Text blocks come through verbatim; other block types
/// are rendered as short readable summaries so the archive is
/// grep-able (`[tool_use name=Bash]`, `[tool_result ok]`, `[image]`).
fn flatten_blocks(blocks: &[Value]) -> String {
    let mut out = String::new();
    for b in blocks {
        let Some(obj) = b.as_object() else {
            continue;
        };
        let t = obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match t {
            "text" => {
                if let Some(s) = obj.get("text").and_then(|v| v.as_str()) {
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(s);
                }
            }
            "tool_use" => {
                let name = obj.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[tool_use name={name}]"));
            }
            "tool_result" => {
                let is_error = obj
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let status = if is_error { "error" } else { "ok" };
                let inner = obj.get("content").and_then(|c| match c {
                    Value::String(s) => Some(s.clone()),
                    Value::Array(nested) => Some(flatten_blocks(nested)),
                    _ => None,
                });
                if !out.is_empty() {
                    out.push('\n');
                }
                match inner {
                    Some(body) => out.push_str(&format!("[tool_result {status}] {body}")),
                    None => out.push_str(&format!("[tool_result {status}]")),
                }
            }
            "image" => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str("[image]");
            }
            other => {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[{other}]"));
            }
        }
    }
    out
}

/// Minimal ISO 8601 parser — enough for Claude Code's
/// `"2026-11-07T01:23:45.678Z"` shape. Returns unix seconds when the
/// string parses cleanly; `None` otherwise (the caller falls back to
/// `now()`, which keeps ordering correct within a single ingest).
fn parse_iso8601_secs(s: &str) -> Option<i64> {
    // Accept "YYYY-MM-DDTHH:MM:SS[.fraction][Z|±HH:MM]".
    let bytes = s.as_bytes();
    if bytes.len() < 19 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let year: i64 = s.get(0..4)?.parse().ok()?;
    let month: u32 = s.get(5..7)?.parse().ok()?;
    let day: u32 = s.get(8..10)?.parse().ok()?;
    let hour: u32 = s.get(11..13)?.parse().ok()?;
    let minute: u32 = s.get(14..16)?.parse().ok()?;
    let second: u32 = s.get(17..19)?.parse().ok()?;
    Some(
        days_from_civil(year, month, day) * 86_400
            + hour as i64 * 3600
            + minute as i64 * 60
            + second as i64,
    )
}

/// Howard Hinnant's `days_from_civil` — days since 1970-01-01 (Unix
/// epoch) for a civil date. Used by the ISO 8601 parser above; keeps
/// us out of any chrono / time dependency for v0.9.0.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) as i64 + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonl_generic_round_trip() {
        let src = r#"
{"turn_id":"a","role":"user","content":"hi","timestamp":1700000000}
{"turn_id":"b","role":"assistant","content":"hello"}
"#;
        let turns = read_transcript(src.as_bytes(), TranscriptFormat::JsonlGeneric).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_id, "a");
        assert_eq!(turns[0].role, "user");
        assert_eq!(turns[0].timestamp, Some(1_700_000_000));
        assert_eq!(turns[1].timestamp, None);
    }

    #[test]
    fn claude_code_jsonl_skips_non_message_types() {
        let src = r#"{"type":"system","uuid":"sys-1","content":"init"}
{"type":"user","uuid":"u-1","message":{"role":"user","content":"hi"}}
{"type":"assistant","uuid":"a-1","message":{"role":"assistant","content":[{"type":"text","text":"hey"}]}}
"#;
        let turns = read_transcript(src.as_bytes(), TranscriptFormat::ClaudeCodeJsonl).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].turn_id, "u-1");
        assert_eq!(turns[1].content, "hey");
    }

    #[test]
    fn claude_code_flattens_tool_use_and_tool_result() {
        let src = r#"{"type":"assistant","uuid":"a","message":{"role":"assistant","content":[{"type":"text","text":"let me run:"},{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}
{"type":"user","uuid":"u","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t","content":"ok\n","is_error":false}]}}
"#;
        let turns = read_transcript(src.as_bytes(), TranscriptFormat::ClaudeCodeJsonl).unwrap();
        assert_eq!(turns.len(), 2);
        assert!(turns[0].content.contains("let me run:"));
        assert!(turns[0].content.contains("[tool_use name=Bash]"));
        assert!(turns[1].content.contains("[tool_result ok]"));
    }
}
