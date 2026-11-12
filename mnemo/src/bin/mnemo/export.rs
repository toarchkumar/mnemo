//! `mnemo export`: the readable plaintext export (PLAN.md 12.5).
//!
//! Two formats, Markdown for people and JSON Lines for tools, over the live
//! memories of a file opened read-only. Vectors are always left out: they
//! aren't readable, and the encrypted file keeps them.
//!
//! `--git` makes the output diff-clean, for keeping an export under version
//! control beside the project:
//! - records are ordered by key, then id (keyed memories first), never by
//!   access time;
//! - thread turns (the raw archive), the access stats (`accessed_at`,
//!   `access_count`) and the summary counts are left out, so a plain read
//!   of the file never changes the export;
//! - the result cache (and so the embedder-identity record) is never part
//!   of an export.
//!
//! The same unchanged file exports to the same bytes, and one new memory
//! adds only its own lines. A new version of a keyed fact also changes the
//! line of the version it supersedes (its `_mnemo.superseded_by` and
//! `_mnemo.invalid_at`).

use std::io::Write;

use mnemo_engine::{
    Memory, MemoryType, Scope, MNEMO_META_KEY, MNEMO_META_KIND, MNEMO_META_KIND_TURN,
};
use serde_json::{Map, Number, Value};

/// The output format.
#[derive(Copy, Clone, Debug, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "lowercase")]
pub enum ExportFormat {
    /// One section per memory, for people.
    Markdown,
    /// One JSON object per memory per line, for tools.
    Jsonl,
}

/// What to export, and how.
pub struct ExportOptions {
    pub format: ExportFormat,
    pub git: bool,
    pub types: Option<Vec<MemoryType>>,
    pub agent: Option<String>,
}

/// What the file says about itself, for the Markdown header.
pub struct FileFacts {
    pub format_version: u16,
    pub dimensions: usize,
}

fn key_of(m: &Memory) -> Option<&str> {
    m.metadata.get(MNEMO_META_KEY).and_then(Value::as_str)
}

fn is_turn(m: &Memory) -> bool {
    m.metadata.get(MNEMO_META_KIND).and_then(Value::as_str) == Some(MNEMO_META_KIND_TURN)
}

/// Apply the filters and the order. Without `--git` the order is by id
/// (ULIDs sort by creation time); with it, keyed memories come first in key
/// order, then the rest, each by id.
pub fn select(mut all: Vec<Memory>, o: &ExportOptions) -> Vec<Memory> {
    if let Some(ts) = &o.types {
        all.retain(|m| ts.contains(&m.memory_type));
    }
    if let Some(a) = &o.agent {
        all.retain(|m| &m.agent_id == a);
    }
    if o.git {
        all.retain(|m| !is_turn(m));
        all.sort_by(|a, b| {
            let ka = key_of(a);
            let kb = key_of(b);
            (ka.is_none(), ka, a.id).cmp(&(kb.is_none(), kb, b.id))
        });
    } else {
        all.sort_by_key(|m| m.id);
    }
    all
}

/// The output, as one string.
pub fn render(memories: &[Memory], o: &ExportOptions, facts: &FileFacts) -> String {
    match o.format {
        ExportFormat::Jsonl => {
            let mut out = String::new();
            for m in memories {
                let line = serde_json::to_string(&Value::Object(record(m, o.git)))
                    .unwrap_or_else(|_| "{}".into());
                out.push_str(&line);
                out.push('\n');
            }
            out
        }
        ExportFormat::Markdown => markdown(memories, o, facts),
    }
}

/// Write `body` to `path` whole: into a new file beside it, readable by
/// its owner only on Unix (it's the plaintext of an encrypted file), then
/// renamed over `path`. So the result is owner-only even when `path`
/// already existed with a wider mode, and a crash leaves either the old
/// file or the new one, never a partial export under the final name.
pub fn write_private(path: &str, body: &str) -> std::result::Result<(), String> {
    let target = std::path::Path::new(path);
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let name = target
        .file_name()
        .ok_or_else(|| format!("{path}: not a file name"))?
        .to_string_lossy()
        .into_owned();
    let mut last_err = None;
    for attempt in 0..16u32 {
        let tmp = dir.join(format!(".{name}.{}-{attempt}.tmp", std::process::id()));
        let mut opts = std::fs::OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = match opts.open(&tmp) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                last_err = Some(e);
                continue;
            }
            Err(e) => return Err(format!("writing beside {path}: {e}")),
        };
        let written = f
            .write_all(body.as_bytes())
            .and_then(|_| f.sync_all())
            .and_then(|_| {
                drop(f);
                std::fs::rename(&tmp, target)
            });
        return written.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("writing {path}: {e}")
        });
    }
    Err(format!(
        "writing beside {path}: {}",
        last_err.map(|e| e.to_string()).unwrap_or_default()
    ))
}

fn scope_str(s: Scope) -> &'static str {
    match s {
        Scope::Private => "private",
        Scope::Shared => "shared",
    }
}

/// `f32` as the JSON number it reads as (`0.7`, not `0.699999988079071`).
fn short_f32(x: f32) -> Value {
    format!("{x}")
        .parse::<f64>()
        .ok()
        .and_then(Number::from_f64)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

/// One memory as a JSON object (keys sort, since `serde_json`'s map is
/// ordered).
fn record(m: &Memory, git: bool) -> Map<String, Value> {
    let mut r = Map::new();
    r.insert("id".into(), Value::String(m.id.to_string()));
    r.insert("type".into(), Value::String(m.memory_type.as_str().into()));
    r.insert("agent".into(), Value::String(m.agent_id.clone()));
    if let Some(k) = key_of(m) {
        r.insert("key".into(), Value::String(k.into()));
    }
    if let Some(s) = &m.session_id {
        r.insert("session".into(), Value::String(s.clone()));
    }
    r.insert("scope".into(), Value::String(scope_str(m.scope).into()));
    r.insert("created_at".into(), Value::from(m.created_at));
    r.insert("importance".into(), short_f32(m.importance));
    if let Some(t) = m.ttl_secs {
        r.insert("ttl_secs".into(), Value::from(t));
    }
    if !git {
        r.insert("accessed_at".into(), Value::from(m.accessed_at));
        r.insert("access_count".into(), Value::from(m.access_count));
    }
    if !m.metadata.is_empty() {
        r.insert("metadata".into(), Value::Object(m.metadata.clone()));
    }
    r.insert("content".into(), Value::String(m.content.clone()));
    r
}

fn markdown(memories: &[Memory], o: &ExportOptions, facts: &FileFacts) -> String {
    let mut out = String::new();
    out.push_str("# mnemo export\n\n");
    out.push_str(&format!(
        "Plaintext export of a `.mnemo` file (format v{}, {} dimensions). \
         Keep it as safe as the file's passphrase.\n\n",
        facts.format_version, facts.dimensions
    ));
    if o.git {
        out.push_str(
            "Written with `--git`: ordered by key, then id; vectors, thread turns \
             and access stats are left out.\n",
        );
    } else {
        out.push_str(&format!(
            "{} {}. Vectors are left out.\n",
            memories.len(),
            if memories.len() == 1 {
                "memory"
            } else {
                "memories"
            }
        ));
    }
    for m in memories {
        out.push('\n');
        out.push_str(&format!("## {}\n\n", m.id));
        out.push_str(&format!("- type: {}\n", m.memory_type.as_str()));
        out.push_str(&format!("- agent: {}\n", code(&m.agent_id)));
        if let Some(k) = key_of(m) {
            out.push_str(&format!("- key: {}\n", code(k)));
        }
        if let Some(s) = &m.session_id {
            out.push_str(&format!("- session: {}\n", code(s)));
        }
        out.push_str(&format!("- scope: {}\n", scope_str(m.scope)));
        out.push_str(&format!("- created: {}\n", rfc3339(m.created_at)));
        out.push_str(&format!("- importance: {}\n", m.importance));
        if let Some(t) = m.ttl_secs {
            out.push_str(&format!("- ttl: {t} s\n"));
        }
        if !o.git {
            out.push_str(&format!(
                "- accessed: {} ({} {})\n",
                rfc3339(m.accessed_at),
                m.access_count,
                if m.access_count == 1 { "time" } else { "times" }
            ));
        }
        if !m.metadata.is_empty() {
            let meta = serde_json::to_string(&Value::Object(m.metadata.clone()))
                .unwrap_or_else(|_| "{}".into());
            out.push_str(&format!("- metadata: {}\n", code(&meta)));
        }
        out.push('\n');
        let fence = "`".repeat(longest_run(&m.content, '`').max(2) + 1);
        out.push_str(&format!("{fence}text\n{}\n{fence}\n", m.content));
    }
    out
}

fn longest_run(s: &str, c: char) -> usize {
    let (mut best, mut cur) = (0, 0);
    for ch in s.chars() {
        if ch == c {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

/// `s` as Markdown inline code, whatever backticks or line breaks it holds.
fn code(s: &str) -> String {
    let s = s.replace('\r', "\\r").replace('\n', "\\n");
    let ticks = "`".repeat(longest_run(&s, '`') + 1);
    if s.starts_with('`') || s.ends_with('`') {
        format!("{ticks} {s} {ticks}")
    } else {
        format!("{ticks}{s}{ticks}")
    }
}

/// Unix seconds as an RFC 3339 UTC time (`2026-11-11T09:30:00Z`).
pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_matches_known_times() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339(1_794_389_400), "2026-11-11T09:30:00Z");
        assert_eq!(rfc3339(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn code_survives_backticks_and_newlines() {
        assert_eq!(code("plain"), "`plain`");
        assert_eq!(code("a`b"), "``a`b``");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(code("two\nlines"), "`two\\nlines`");
    }

    #[test]
    fn importance_reads_as_written() {
        assert_eq!(short_f32(0.7).to_string(), "0.7");
        assert_eq!(short_f32(1.0).to_string(), "1.0");
    }
}
