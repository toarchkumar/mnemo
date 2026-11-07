//! Hand-rolled Model Context Protocol (MCP) stdio server for MNemo.
//!
//! Exposes a `.mnemo` file as an MCP server so any MCP-compatible agent
//! (Claude Desktop, Cursor, mcp-server, custom clients) can drive it as a
//! tool. Framed as **JSON-RPC 2.0 with newline-delimited JSON** over
//! stdin/stdout, per the MCP spec. Stderr is used for human-readable
//! diagnostics only — never for protocol traffic.
//!
//! ## Why hand-rolled
//!
//! The official [`rmcp`](https://crates.io/crates/rmcp) SDK declares
//! `edition = "2024"` on its workspace, which requires Rust 1.85+; our
//! MSRV is 1.75 and locked (transitive deps pin us). `rmcp` also brings
//! in tokio + async, a big architectural shift for a sync-only core.
//! Hand-rolling stdio JSON-RPC is ~250 LoC and adds zero dependencies
//! beyond `serde_json` (already a dep). See CHANGELOG's Phase 4 entry
//! for the decision record.
//!
//! ## Protocol version
//!
//! We advertise MCP protocol `2025-06-18` on `initialize`. If a client
//! requests a different version we echo their version back — the standard
//! compatibility dance. The seven tools we expose are stable across
//! recent protocol revisions.
//!
//! ## Tools
//!
//! - `about` — read-only briefing (manifest + onboarding memories)
//! - `remember` — insert a memory (BYO vector; embedder integration deferred to Phase 3)
//! - `recall` — vector recall (BYO query vector)
//! - `forget` — delete by ULID
//! - `list` — enumerate live memories
//! - `snapshot_list` — enumerate PITR snapshots
//! - `stats` — database statistics
//!
//! Every mutation calls `Mnemo::flush()` before returning success — MCP
//! clients treat tool calls as atomic and expect on-disk durability.
//!
//! ## Passphrase
//!
//! Read from the `MNEMO_PASSPHRASE` environment variable only.
//! `serve_stdio` refuses to start without it. There is no CLI-flag
//! fallback here — that would leave the passphrase in shell history
//! and process listings.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::error::{MnemoError, Result};
use crate::memory::{Memory, MemoryType, Metric, Scope};
use crate::result_cache::{CachePutOpts, SemanticCachePutOpts, DEFAULT_SEMANTIC_THRESHOLD};
use crate::store::{Mnemo, RecallRequest};

/// MCP protocol version we advertise on `initialize`.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Environment variable that supplies the database passphrase.
const PASSPHRASE_ENV: &str = "MNEMO_PASSPHRASE";

/// Fuzz-target helper: parse a single JSON-RPC line as a `Request` and
/// discard. Panic-free — malformed input returns `Err(String)`, valid
/// requests return `Ok(())`. Exposed only via `crate::__fuzz`; not part
/// of the stable public API.
#[doc(hidden)]
pub fn __fuzz_parse_request_line(line: &str) -> std::result::Result<(), String> {
    serde_json::from_str::<Request>(line)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

// --- JSON-RPC 2.0 wire types ---------------------------------------------

/// Incoming JSON-RPC 2.0 request or notification (a notification is a
/// request with no `id`).
#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

/// Outgoing JSON-RPC 2.0 response envelope.
#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// JSON-RPC 2.0 error object.
#[derive(Debug, Serialize)]
struct RpcError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

impl RpcError {
    fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
}

// Standard JSON-RPC error codes.
const PARSE_ERROR: i32 = -32700;
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

// --- Entry point ---------------------------------------------------------

/// Run the MCP stdio server over the provided `.mnemo` file.
///
/// Blocks the calling thread — reads JSON-RPC requests line-by-line from
/// stdin, dispatches to the tool implementations, writes responses to
/// stdout, and logs diagnostics to stderr. Returns when stdin closes
/// (typical MCP client shutdown) or on a fatal I/O error.
///
/// The passphrase is read from the `MNEMO_PASSPHRASE` environment
/// variable. If unset, returns [`MnemoError::Invalid`] without opening
/// the file.
pub fn serve_stdio(path: &Path) -> Result<()> {
    serve_stdio_with_embedder(path, None)
}

/// Variant that optionally registers an [`crate::EmbedCmd`] embedder on
/// the handle at server start (Phase 3 PR 10b). The MCP `remember`,
/// `recall`, `cache_put_semantic` and `cache_get_semantic` tools then
/// accept a `text` param as an alternative to the explicit vector —
/// the embedder runs once per request against the persistent child
/// process. Pass `None` for the pre-Phase-3 vector-only behavior.
///
/// `embed_spec` tuple fields: `(command, name, normalize)`. See
/// [`crate::EmbedCmd::new`] for the command format.
pub fn serve_stdio_with_embedder(
    path: &Path,
    embed_spec: Option<(String, String, bool, std::time::Duration)>,
) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_with_streams(path, stdin.lock(), stdout.lock(), embed_spec)
}

/// Run the MCP loop against explicit reader/writer streams. Extracted so
/// integration tests can pipe scripted input without touching the real
/// stdin/stdout. `embed_spec` is as in [`serve_stdio_with_embedder`].
pub fn serve_with_streams<R: Read, W: Write>(
    path: &Path,
    reader: R,
    mut writer: W,
    embed_spec: Option<(String, String, bool, std::time::Duration)>,
) -> Result<()> {
    let passphrase = std::env::var(PASSPHRASE_ENV).map_err(|_| {
        MnemoError::Invalid(format!(
            "{PASSPHRASE_ENV} is required to serve — set it in the shell before starting"
        ))
    })?;

    eprintln!(
        "mnemo mcp: opening {} (protocol {PROTOCOL_VERSION})",
        path.display()
    );
    let mut db = Mnemo::open(path, &passphrase)?;
    if let Some((cmd, name, normalize, timeout)) = embed_spec {
        let identity = crate::EmbedderIdentity {
            name,
            dimensions: db.dimensions(),
            normalize,
        };
        let embedder = crate::EmbedCmd::new(&cmd, identity)?.with_timeout(timeout);
        db.set_embedder(embedder)?;
        eprintln!(
            "mnemo mcp: embedder ready (text params accepted, timeout {}s)",
            timeout.as_secs()
        );
    }
    eprintln!("mnemo mcp: ready ({} live memories)", db.len());

    let reader = BufReader::new(reader);
    for line in reader.lines() {
        let line = line.map_err(MnemoError::Io)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (id, out) = match serde_json::from_str::<Request>(trimmed) {
            Ok(req) => {
                let id = req.id.clone();
                let is_notification = id.is_none();
                let result = handle(&mut db, &req);
                if is_notification {
                    // Per JSON-RPC 2.0: notifications get no response.
                    continue;
                }
                (id.unwrap_or(Value::Null), result)
            }
            Err(e) => (
                Value::Null,
                Err(RpcError::new(PARSE_ERROR, format!("parse error: {e}"))),
            ),
        };

        let resp = match out {
            Ok(value) => Response {
                jsonrpc: "2.0",
                id,
                result: Some(value),
                error: None,
            },
            Err(err) => Response {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(err),
            },
        };

        let line =
            serde_json::to_string(&resp).map_err(|e| MnemoError::Serialize(e.to_string()))?;
        writeln!(writer, "{line}").map_err(MnemoError::Io)?;
        writer.flush().map_err(MnemoError::Io)?;
    }
    // Scoring v2 (v0.7.0) flipped `tool_recall` to `track_access(true)`:
    // every recall bumps `accessed_at` / `access_count` in the catalog
    // in memory (no per-call fsync). The previous `db.flush()` only
    // fired on each mutating tool call's exit path, so a session that
    // issued only `recall` / `about` / `stats` / `list` /
    // `cache_get_semantic`-on-hit calls (read-shaped) would exit with
    // every one of its access-stat bumps still in RAM, discarded on
    // shutdown. Flush at shutdown so a read-only session persists its
    // access stats too — the write is cheap (catalog page only; the
    // record bodies aren't dirty) and guarantees the usage-saturation
    // signal survives across sessions.
    if let Err(e) = db.flush() {
        eprintln!("mnemo mcp: flush on shutdown failed: {e}");
    }
    eprintln!("mnemo mcp: stdin closed, shutting down");
    Ok(())
}

// --- Dispatch ------------------------------------------------------------

/// Route a request to a method handler. Returns either the `result`
/// payload for a JSON-RPC success or an [`RpcError`] for a failure.
fn handle(db: &mut Mnemo, req: &Request) -> std::result::Result<Value, RpcError> {
    if req.jsonrpc != "2.0" {
        return Err(RpcError::new(
            INVALID_REQUEST,
            format!("expected jsonrpc \"2.0\", got {:?}", req.jsonrpc),
        ));
    }
    match req.method.as_str() {
        "initialize" => Ok(initialize_result(&req.params)),
        "notifications/initialized" | "notifications/cancelled" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => tools_call(db, &req.params),
        other => Err(RpcError::new(
            METHOD_NOT_FOUND,
            format!("method '{other}' not implemented"),
        )),
    }
}

/// Echo the client's requested protocol version if present; fall back to
/// ours. Always advertise the same `serverInfo` and `capabilities` block.
fn initialize_result(params: &Value) -> Value {
    let client_version = params
        .get("protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or(PROTOCOL_VERSION)
        .to_string();
    json!({
        "protocolVersion": client_version,
        "capabilities": {
            "tools": { "listChanged": false },
        },
        "serverInfo": {
            "name": "mnemo",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

// --- Tools registry ------------------------------------------------------

/// The seven tools this server exposes. Kept as a hand-authored constant
/// (rather than macro-derived) so the JSON schemas stay auditable —
/// every field a client sees is visible right here.
fn tools_list() -> Value {
    json!({
        "tools": [
            {
                "name": "about",
                "description": "Read-only briefing for this .mnemo file: manifest scaffold plus any memories tagged as onboarding, then the project essence (conventions, decisions, open items) as a final `{kind: \"essence\", render}` element when the file has one. Run this first to learn how the file expects to be used.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "remember",
                "description": "Insert, overwrite, or UPSERT a memory. Pass EITHER `vector` (explicit embedding) OR `text` (embedded by the server if it was started with --embed-cmd / --embed-name). Phase 12.1: when `key` is set, this becomes a keyed upsert — if a live memory already holds this `(agent + key)` slot the prior record is marked `_mnemo.superseded_by` and a new one is inserted, and an identical re-write is a no-op. **PREFER `key` for any fact that replaces an earlier one** (versions, current status, user preferences, plan outcomes). Persists to disk before returning. Returns `{id, outcome}` where `outcome` is `added`, `superseded`, `unchanged`, or (no key) `inserted`.",
                "inputSchema": {
                    "type": "object",
                    "required": ["content", "memory_type"],
                    "properties": {
                        "content": { "type": "string" },
                        "memory_type": { "type": "string", "enum": ["episodic", "semantic", "procedural", "working"] },
                        "vector": { "type": "array", "items": { "type": "number" }, "description": "Explicit embedding; mutually exclusive with `text`." },
                        "text": { "type": "string", "description": "Content to embed via the server's registered embedder; mutually exclusive with `vector`. Requires the server to have been started with --embed-cmd." },
                        "importance": { "type": "number", "minimum": 0, "maximum": 1 },
                        "agent_id": { "type": "string" },
                        "session_id": { "type": "string" },
                        "ttl_secs": { "type": "integer" },
                        "shared": { "type": "boolean" },
                        "metadata": { "type": "object" },
                        "key": { "type": "string", "description": "Phase 12.1 upsert slot name. A later `remember` with the same `key` + agent scope supersedes the prior record; an identical re-write is a no-op. Use this for any fact that replaces an earlier one." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "update",
                "description": "Edit a memory in place (Phase 12.1). Preserves the ULID, `created_at` and access stats. Any of `content` (needs `vector` to avoid embedding drift), `text` (re-embeds via the server), `importance`, `metadata`, `ttl_secs` can change. An identical patch returns `{id, outcome: 'unchanged'}` without writing. Prefer `remember` with `key` for facts that replace earlier ones; use `update` for in-place corrections to one specific record.",
                "inputSchema": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "description": "ULID of the memory to update." },
                        "content": { "type": "string", "description": "New content. Requires a matching `vector` (use `text` to re-embed instead)." },
                        "vector": { "type": "array", "items": { "type": "number" } },
                        "text": { "type": "string", "description": "New content; the server embeds it through its registered embedder. Mutually exclusive with `content` + `vector`." },
                        "importance": { "type": "number", "minimum": 0, "maximum": 1 },
                        "metadata": { "type": "object", "description": "Replace the user-visible metadata. Reserved `_mnemo.*` keys on the record are preserved." },
                        "ttl_secs": { "type": "integer", "description": "Set the TTL to this many seconds. Use `clear_ttl: true` to drop any TTL." },
                        "clear_ttl": { "type": "boolean", "description": "Clear any TTL on the record." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "essence",
                "description": "Phase 12.7b: edit or render the essence layer of a thread (or the project layer). ONE tool for every op — set `op` to `add` / `append` / `update` / `resolve` / `remove` / `render`. **Call it when the goal, a decision, an open problem, the next step or the user's way of working changes, and append a timeline entry at each milestone.** Section items (`intent`, `conventions`, `decisions`, `state`, `open`, `next`, `lessons`) are ≤ 280 bytes and have an `open → resolved` lifecycle; `append` adds an append-only `timeline` entry (≤ 600 bytes). Thread-scoped (`thread`) or project-scoped (`project: true`). `render` returns the deterministic markdown view, budgeted at `budget` bytes (default 6144).",
                "inputSchema": {
                    "type": "object",
                    "required": ["op"],
                    "properties": {
                        "op": { "type": "string", "enum": ["add", "append", "update", "resolve", "remove", "render"] },
                        "thread": { "type": "string", "description": "Thread id. Omit with `project: true` for the project layer." },
                        "project": { "type": "boolean", "default": false, "description": "Project-scoped item (shows in every thread's render). Mutually exclusive with `thread`." },
                        "section": { "type": "string", "enum": ["intent", "conventions", "decisions", "state", "open", "next", "lessons"], "description": "Required for `op: add`. Timeline uses `op: append`, not add." },
                        "id": { "type": "string", "description": "ULID of the section item. Required for update / resolve / remove." },
                        "text": { "type": "string", "description": "Required for add / append / update." },
                        "refs": { "type": "array", "items": { "type": "string" }, "description": "Optional free-form reference strings (turn ids, memory ids, commits, paths)." },
                        "budget": { "type": "integer", "default": 6144, "description": "Byte budget for `op: render`." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "thread_start",
                "description": "Phase 12.7c: start a new thread that continues from a previous one. `essence {op: render, thread: <new>}` then pulls the prior thread's open items, `next` and last-N timeline entries ahead of the new thread's own essence — the 'new thread, same experience' path. Returns `{id}` (the link-record ULID).",
                "inputSchema": {
                    "type": "object",
                    "required": ["new", "from"],
                    "properties": {
                        "new": { "type": "string", "description": "New thread id." },
                        "from": { "type": "string", "description": "Previous thread id to continue from." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "commit_deltas",
                "description": "Atomic batch write: facts, essence ops and thread archive turns in one call. Every item is checked before anything is written, so an invalid item writes nothing; one flush on success. Facts may omit `vector`: the server embeds every fact `content`, essence text and turn in one batched call with its embedder (`--embed-cmd` or builtin). Use at a session end to save a burst of updates in one round trip. `turn_policy` defaults to `archive`; `expire_after` takes a TTL in seconds; `drop` discards turns. Returns a counts summary.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "facts": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["content", "memory_type"],
                                "properties": {
                                    "content": { "type": "string" },
                                    "memory_type": { "type": "string", "enum": ["episodic", "semantic", "procedural", "working"] },
                                    "vector": { "type": "array", "items": { "type": "number" }, "description": "Optional. Omit it to have the server embed `content`." },
                                    "importance": { "type": "number", "minimum": 0, "maximum": 1 },
                                    "agent_id": { "type": "string" },
                                    "session_id": { "type": "string" },
                                    "shared": { "type": "boolean" },
                                    "metadata": { "type": "object" },
                                    "key": { "type": "string" }
                                }
                            }
                        },
                        "essence_ops": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["kind"],
                                "properties": {
                                    "kind": { "type": "string", "enum": ["add", "append_timeline", "update", "resolve", "remove"] },
                                    "thread": { "type": "string" },
                                    "project": { "type": "boolean" },
                                    "section": { "type": "string" },
                                    "id": { "type": "string" },
                                    "text": { "type": "string" },
                                    "refs": { "type": "array", "items": { "type": "string" } }
                                }
                            }
                        },
                        "turns": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["turn_id", "role", "content"],
                                "properties": {
                                    "turn_id": { "type": "string" },
                                    "role": { "type": "string" },
                                    "content": { "type": "string" },
                                    "timestamp": { "type": "integer" }
                                }
                            }
                        },
                        "thread_id": { "type": "string", "description": "Required when `turns` is non-empty." },
                        "turn_policy": { "type": "string", "enum": ["archive", "expire_after", "drop"], "default": "archive" },
                        "turn_policy_ttl": { "type": "integer", "description": "TTL in seconds; required when `turn_policy = 'expire_after'`." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "thread_ingest",
                "description": "Phase 12.7a: ingest a transcript file into a thread's archive layer. Idempotent per `turn_id` — re-running on a longer transcript appends only the new turns. Requires an embedder on the server (`--embed-cmd` at serve start) because every turn is embedded. Returns `{added_records, skipped_existing_turns, chunked_turns, truncated_turns}`.",
                "inputSchema": {
                    "type": "object",
                    "required": ["thread", "transcript_path"],
                    "properties": {
                        "thread": { "type": "string", "description": "Thread id to append under." },
                        "transcript_path": { "type": "string", "description": "Local path to the transcript file." },
                        "format": { "type": "string", "enum": ["jsonl", "claude-code-jsonl"], "default": "jsonl" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "thread_export",
                "description": "Return a thread's archive in the ingest shape: `[{turn_id, role, content, timestamp}]` in order, one entry per turn (long turns' chunks joined), plus `sha256` and `original_length` on a truncated tool result. Feeding it back to ingest for the same thread adds nothing.",
                "inputSchema": {
                    "type": "object",
                    "required": ["thread"],
                    "properties": {
                        "thread": { "type": "string", "description": "Thread id to export." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "history",
                "description": "Walk the full chain for a keyed slot (Phase 12.1), newest first. Entry 0 is the live record (or empty if the slot is empty); subsequent entries are the superseded history. Slot resolution matches keyed `remember`: with no `agent` and `shared=false`, walks the default agent's private slot (v0.8.1 bugfix — pre-v0.8.1 this walked the shared slot by accident and returned empty for the common case). Pass `shared: true` for the shared slot; `agent` + `shared=true` is rejected.",
                "inputSchema": {
                    "type": "object",
                    "required": ["key"],
                    "properties": {
                        "key": { "type": "string" },
                        "agent": { "type": "string", "description": "Query the private slot for this agent. Mutually exclusive with `shared`." },
                        "shared": { "type": "boolean", "default": false, "description": "Query the shared slot (one per file). Mutually exclusive with `agent`." }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "recall",
                "description": "Score every live memory (or the ANN-narrowed candidate set) against a query and return the top-k. Pass EITHER `query` (explicit vector) OR `text` (embedded by the server; requires --embed-cmd at serve start). Phase 12.6: add a `filter` object to restrict candidates before scoring (`session_id` plus per-metadata-key conditions: equality, `$in`, `$exists`, `$gte`/`$lte`). Phase 12.1: superseded records never appear. Phase 12.8: set `max_bytes: N` to cap the total UTF-8 bytes of returned `content` fields; the response is then shaped as `{hits: [...], truncated: bool}` so the caller can tell whether the budget cut hits. Without `max_bytes`, the response is a bare JSON array of hits (unchanged).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "array", "items": { "type": "number" }, "description": "Explicit query vector; mutually exclusive with `text`." },
                        "text": { "type": "string", "description": "Query text; embedded by the server's registered embedder. Mutually exclusive with `query`." },
                        "top_k": { "type": "integer", "default": 10 },
                        "types": { "type": "array", "items": { "type": "string" } },
                        "agent": { "type": "string" },
                        "metric": { "type": "string", "enum": ["cosine", "l2", "dot"], "default": "cosine" },
                        "max_bytes": { "type": "integer", "minimum": 1, "description": "Phase 12.8 budget: stop adding hits once the UTF-8 bytes of their `content` fields would exceed N. Always returns at least the top hit. When set, the response is `{hits, truncated}` instead of a bare array." },
                        "filter": {
                            "type": "object",
                            "description": "Phase 12.6 pre-scoring filter. All conditions AND together. Shape: `{\"session_id\": \"...\", \"metadata\": {key: value | {\"$in\": [...]} | {\"$exists\": true} | {\"$gte\": n, \"$lte\": n}}}`.",
                            "properties": {
                                "session_id": { "type": "string" },
                                "metadata": { "type": "object" }
                            },
                            "additionalProperties": false
                        }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "forget",
                "description": "Soft-delete a memory by ULID. Space is reclaimed on the next `mnemo compact`. Persists to disk before returning.",
                "inputSchema": {
                    "type": "object",
                    "required": ["id"],
                    "properties": {
                        "id": { "type": "string", "description": "ULID of the memory to delete" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "list",
                "description": "Return every live memory as JSON. Sorted by creation time, oldest first. Vectors are omitted by default to keep payloads small.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "include_vectors": { "type": "boolean", "default": false }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "snapshot_list",
                "description": "List every restorable snapshot (one per committed flush). Each entry gives the transaction ID, creation timestamp, and memory count.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "stats",
                "description": "Summary statistics: memory count, deleted count, agents present, snapshot count, page cache occupancy.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "cache_put",
                "description": "Insert or overwrite an exact-key cache entry. Value is a UTF-8 string; use `content_type` to label the payload. Persists to disk before returning. Namespace and key are caller-chosen; the key is SHA-256 hashed by the engine.",
                "inputSchema": {
                    "type": "object",
                    "required": ["namespace", "key", "value"],
                    "properties": {
                        "namespace": { "type": "string" },
                        "key": { "type": "string" },
                        "value": { "type": "string" },
                        "content_type": { "type": "string", "default": "text" },
                        "ttl_secs": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "cache_get",
                "description": "Look up an exact-key cache entry. Returns `{value, content_type, hit}` on a hit, `{hit: false}` on miss (missing or TTL-expired). Bumps access stats catalog-only.",
                "inputSchema": {
                    "type": "object",
                    "required": ["namespace", "key"],
                    "properties": {
                        "namespace": { "type": "string" },
                        "key": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "cache_stats",
                "description": "Cache statistics for a namespace (omit for the whole cache). Returns entries, bytes, hits, misses, hit_rate, evictions.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "namespace": { "type": "string" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "cache_put_semantic",
                "description": "Insert an entry with an embedding vector for semantic recall (Phase 10.2). Pass EITHER `vector` + `model` (explicit) OR `text` (embedded by the server; `model` defaults to the server embedder's identity name). Later cache_get_semantic calls under a different `model` won't see this entry (a different-model hit would be a semantics bug).",
                "inputSchema": {
                    "type": "object",
                    "required": ["namespace", "key", "value"],
                    "properties": {
                        "namespace": { "type": "string" },
                        "key": { "type": "string" },
                        "vector": { "type": "array", "items": { "type": "number" }, "description": "Explicit embedding; mutually exclusive with `text`. Requires `model`." },
                        "text": { "type": "string", "description": "Content to embed via the server's registered embedder; mutually exclusive with `vector`." },
                        "value": { "type": "string" },
                        "model": { "type": "string", "description": "Required with `vector`; optional with `text` (defaults to the embedder's identity name)." },
                        "content_type": { "type": "string", "default": "text" },
                        "ttl_secs": { "type": "integer", "minimum": 0 }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "cache_get_semantic",
                "description": "Semantic recall over the cache: top-1 cosine over live vectored entries in the namespace whose `model` matches. Pass EITHER `query` + `model` OR `text` (which embeds via the server and defaults the model to the embedder's identity). Returns `{hit: true, value, similarity, ...}` iff similarity >= threshold, else `{hit: false}`. Threshold defaults to 0.97.",
                "inputSchema": {
                    "type": "object",
                    "required": ["namespace"],
                    "properties": {
                        "namespace": { "type": "string" },
                        "query": { "type": "array", "items": { "type": "number" }, "description": "Explicit query vector; mutually exclusive with `text`. Requires `model`." },
                        "text": { "type": "string", "description": "Query text; embedded by the server. Mutually exclusive with `query`." },
                        "model": { "type": "string", "description": "Required with `query`; optional with `text`." },
                        "threshold": { "type": "number", "minimum": -1.0, "maximum": 1.0 }
                    },
                    "additionalProperties": false
                }
            }
        ]
    })
}

// --- Tool dispatch -------------------------------------------------------

/// The MCP `tools/call` method: dispatches on the caller-supplied `name`
/// and returns the tool's `content` array wrapped in an MCP result.
fn tools_call(db: &mut Mnemo, params: &Value) -> std::result::Result<Value, RpcError> {
    let name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "missing tool name"))?;
    let empty = json!({});
    let args = params.get("arguments").unwrap_or(&empty);

    let text = match name {
        "about" => tool_about(db)?,
        "remember" => tool_remember(db, args)?,
        "update" => tool_update(db, args)?,
        "history" => tool_history(db, args)?,
        "thread_ingest" => tool_thread_ingest(db, args)?,
        "thread_export" => tool_thread_export(db, args)?,
        "thread_start" => tool_thread_start(db, args)?,
        "commit_deltas" => tool_commit_deltas(db, args)?,
        "essence" => tool_essence(db, args)?,
        "recall" => tool_recall(db, args)?,
        "forget" => tool_forget(db, args)?,
        "list" => tool_list(db, args)?,
        "snapshot_list" => tool_snapshot_list(db)?,
        "stats" => tool_stats(db)?,
        "cache_put" => tool_cache_put(db, args)?,
        "cache_get" => tool_cache_get(db, args)?,
        "cache_stats" => tool_cache_stats(db, args)?,
        "cache_put_semantic" => tool_cache_put_semantic(db, args)?,
        "cache_get_semantic" => tool_cache_get_semantic(db, args)?,
        other => {
            return Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown tool '{other}'"),
            ))
        }
    };

    Ok(json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": false,
    }))
}

/// Convert a [`MnemoError`] to an [`RpcError`]. Every error becomes an
/// `INTERNAL_ERROR` with the human-readable message — MCP clients
/// display `error.message` verbatim to the user.
/// Resolve a text argument into a vector via the server's registered
/// embedder (Phase 3 PR 10b). Called by the four MCP tools that
/// accept `text` as an alternative to the explicit vector: `remember`,
/// `recall`, `cache_put_semantic`, `cache_get_semantic`. If no
/// embedder was configured at server start (`mnemo serve --mcp
/// --embed-cmd ...`) the error message tells the caller how to turn
/// it on.
fn embed_text_via_server(
    db: &mut Mnemo,
    tool: &str,
    text: &str,
) -> std::result::Result<Vec<f32>, RpcError> {
    let embedder = db.embedder().ok_or_else(|| {
        RpcError::new(
            INVALID_PARAMS,
            format!(
                "{tool}: 'text' requires an embedder on the server. \
                 Start with: mnemo serve --mcp <file> --embed-cmd '<cmd>' --embed-name <name>"
            ),
        )
    })?;
    let mut vs = embedder.embed(&[text]).map_err(mnemo_err)?;
    vs.pop().ok_or_else(|| {
        RpcError::new(
            INTERNAL_ERROR,
            format!("{tool}: embedder returned no vector for the input text"),
        )
    })
}

fn mnemo_err(e: MnemoError) -> RpcError {
    RpcError::new(INTERNAL_ERROR, e.to_string())
}

fn tool_about(db: &mut Mnemo) -> std::result::Result<String, RpcError> {
    let mems = db.about().map_err(mnemo_err)?;
    let mut arr: Vec<Value> = mems.iter().map(memory_to_json).collect();
    // Phase 12.7d: the project essence follows the manifest and the
    // onboarding memories, as one tagged element.
    if let Some(render) = db.about_essence().map_err(mnemo_err)? {
        arr.push(json!({ "kind": "essence", "scope": "project", "render": render }));
    }
    Ok(serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".into()))
}

fn tool_thread_export(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let thread = args
        .get("thread")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "thread_export: missing 'thread'"))?;
    let turns = db.thread_export_turns(thread).map_err(mnemo_err)?;
    let arr: Vec<Value> = turns.iter().map(|t| t.to_json()).collect();
    Ok(Value::Array(arr).to_string())
}

fn tool_remember(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let content = args
        .get("content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "remember: missing 'content'"))?;
    let mtype_str = args
        .get("memory_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "remember: missing 'memory_type'"))?;
    let mtype = MemoryType::parse(mtype_str).ok_or_else(|| {
        RpcError::new(
            INVALID_PARAMS,
            format!("remember: unknown memory_type '{mtype_str}'"),
        )
    })?;
    // Accept either `vector` (explicit) or `text` (embedded via the
    // server's registered embedder, Phase 3). Exactly one required.
    let vector = match (args.get("vector"), args.get("text")) {
        (Some(_), Some(_)) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "remember: pass EITHER 'vector' or 'text', not both",
            ));
        }
        (Some(v), None) => v
            .as_array()
            .ok_or_else(|| {
                RpcError::new(INVALID_PARAMS, "remember: 'vector' must be a JSON array")
            })?
            .iter()
            .map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "remember: 'vector' must be an array of numbers",
                )
            })?,
        (None, Some(t)) => {
            let text = t.as_str().ok_or_else(|| {
                RpcError::new(INVALID_PARAMS, "remember: 'text' must be a string")
            })?;
            embed_text_via_server(db, "remember", text)?
        }
        (None, None) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "remember: missing 'vector' or 'text'",
            ))
        }
    };

    let mut mem = Memory::new(content, mtype, vector);
    if let Some(agent) = args.get("agent_id").and_then(|v| v.as_str()) {
        mem = mem.with_agent(agent);
    }
    if let Some(sess) = args.get("session_id").and_then(|v| v.as_str()) {
        mem = mem.with_session(sess);
    }
    if let Some(imp) = args.get("importance").and_then(|v| v.as_f64()) {
        mem = mem.with_importance(imp as f32);
    }
    if let Some(ttl) = args.get("ttl_secs").and_then(|v| v.as_i64()) {
        mem = mem.with_ttl(ttl);
    }
    if let Some(shared) = args.get("shared").and_then(|v| v.as_bool()) {
        if shared {
            mem = mem.with_scope(Scope::Shared);
        }
    }
    if let Some(meta) = args.get("metadata").and_then(|v| v.as_object()) {
        for (k, v) in meta {
            mem.metadata.insert(k.clone(), v.clone());
        }
    }

    // Phase 12.1: when `key` is set, take the keyed-upsert path
    // (supersede / no-op / add); otherwise keep the pre-12.1
    // single-id return shape (now carrying `outcome: "inserted"` so
    // callers can distinguish it from the keyed outcomes).
    match args.get("key").and_then(|v| v.as_str()) {
        None => {
            let id = db.remember(mem).map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            Ok(json!({ "id": id.to_string(), "outcome": "inserted" }).to_string())
        }
        Some(k) => {
            let (id, outcome) = db.remember_keyed(mem, k).map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            let outcome_str = match outcome {
                crate::WriteOutcome::Added => "added",
                crate::WriteOutcome::Superseded => "superseded",
                crate::WriteOutcome::Unchanged => "unchanged",
                crate::WriteOutcome::Updated => "superseded",
            };
            Ok(json!({ "id": id.to_string(), "outcome": outcome_str }).to_string())
        }
    }
}

fn tool_update(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    use crate::Ulid;
    use crate::{MemoryPatch, WriteOutcome};
    let id_str = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "update: missing 'id'"))?;
    let id = Ulid::from_string(id_str)
        .map_err(|_| RpcError::new(INVALID_PARAMS, format!("update: invalid ULID '{id_str}'")))?;
    // Mutually exclusive: `text` (server re-embeds) vs `content`+`vector`
    // (explicit). Either path produces `(new_content, new_vector)` for
    // the engine; both unset means no text/vector change.
    let has_text = args.get("text").is_some();
    let has_content = args.get("content").is_some();
    let has_vector = args.get("vector").is_some();
    if has_text && (has_content || has_vector) {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "update: pass EITHER 'text' or 'content'+'vector', not both",
        ));
    }
    let mut patch = MemoryPatch::default();
    if has_content {
        let c = args
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "update: 'content' must be a string"))?;
        patch.content = Some(c.to_string());
    }
    if has_vector {
        let v = args
            .get("vector")
            .and_then(|v| v.as_array())
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "update: 'vector' must be a JSON array"))?
            .iter()
            .map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "update: 'vector' must be an array of numbers",
                )
            })?;
        patch.vector = Some(v);
    }
    if let Some(imp) = args.get("importance").and_then(|v| v.as_f64()) {
        patch.importance = Some(imp as f32);
    }
    if let Some(meta) = args.get("metadata").and_then(|v| v.as_object()) {
        patch.metadata = Some(meta.clone());
    }
    let clear_ttl = args
        .get("clear_ttl")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if clear_ttl {
        patch.ttl_secs = Some(None);
    } else if let Some(ttl) = args.get("ttl_secs").and_then(|v| v.as_i64()) {
        patch.ttl_secs = Some(Some(ttl));
    }

    let outcome = if has_text {
        let t = args
            .get("text")
            .and_then(|v| v.as_str())
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "update: 'text' must be a string"))?;
        db.update_text(&id, t, patch).map_err(mnemo_err)?
    } else {
        db.update(&id, patch).map_err(mnemo_err)?
    };
    db.flush().map_err(mnemo_err)?;
    let outcome_str = match outcome {
        WriteOutcome::Updated => "updated",
        WriteOutcome::Unchanged => "unchanged",
        WriteOutcome::Added => "updated",
        WriteOutcome::Superseded => "updated",
    };
    Ok(json!({ "id": id.to_string(), "outcome": outcome_str }).to_string())
}

fn tool_thread_start(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let new = args
        .get("new")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "thread_start: missing 'new'"))?;
    let from = args
        .get("from")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "thread_start: missing 'from'"))?;
    let id = db.thread_start(new, from).map_err(mnemo_err)?;
    db.flush().map_err(mnemo_err)?;
    Ok(json!({ "id": id.to_string() }).to_string())
}

fn tool_commit_deltas(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    use crate::{
        CommitBatch, EssenceOp, EssenceOpKind, EssenceScope, EssenceSection, KeyedMemory,
        ThreadTurn, TurnPolicy, Ulid,
    };
    let mut batch = CommitBatch::default();

    // facts
    if let Some(arr) = args.get("facts").and_then(|v| v.as_array()) {
        for (i, f) in arr.iter().enumerate() {
            let obj = f.as_object().ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("commit_deltas: facts[{i}] must be an object"),
                )
            })?;
            let content = obj.get("content").and_then(|v| v.as_str()).ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("commit_deltas: facts[{i}] missing 'content'"),
                )
            })?;
            let mtype_str = obj
                .get("memory_type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("commit_deltas: facts[{i}] missing 'memory_type'"),
                    )
                })?;
            let mtype = MemoryType::parse(mtype_str).ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("commit_deltas: facts[{i}] unknown memory_type '{mtype_str}'"),
                )
            })?;
            // 12.7d: `vector` is optional. Without one the engine embeds
            // `content` with the server's embedder, batched with the rest
            // of the call.
            let vector = match obj.get("vector") {
                None | Some(Value::Null) => Vec::new(),
                Some(v) => v
                    .as_array()
                    .and_then(|a| {
                        a.iter()
                            .map(|x| x.as_f64().map(|f| f as f32))
                            .collect::<Option<Vec<f32>>>()
                    })
                    .ok_or_else(|| {
                        RpcError::new(
                            INVALID_PARAMS,
                            format!("commit_deltas: facts[{i}] vector must be an array of numbers"),
                        )
                    })?,
            };
            let mut mem = Memory::new(content, mtype, vector);
            if let Some(a) = obj.get("agent_id").and_then(|v| v.as_str()) {
                mem = mem.with_agent(a);
            }
            if let Some(s) = obj.get("session_id").and_then(|v| v.as_str()) {
                mem = mem.with_session(s);
            }
            if let Some(imp) = obj.get("importance").and_then(|v| v.as_f64()) {
                mem = mem.with_importance(imp as f32);
            }
            if let Some(shared) = obj.get("shared").and_then(|v| v.as_bool()) {
                if shared {
                    mem = mem.with_scope(Scope::Shared);
                }
            }
            if let Some(meta) = obj.get("metadata").and_then(|v| v.as_object()) {
                for (k, v) in meta {
                    mem.metadata.insert(k.clone(), v.clone());
                }
            }
            let key = obj.get("key").and_then(|v| v.as_str()).map(String::from);
            batch.facts.push(KeyedMemory { memory: mem, key });
        }
    }

    // essence_ops
    if let Some(arr) = args.get("essence_ops").and_then(|v| v.as_array()) {
        for (i, o) in arr.iter().enumerate() {
            let obj = o.as_object().ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("commit_deltas: essence_ops[{i}] must be an object"),
                )
            })?;
            let kind_str = obj.get("kind").and_then(|v| v.as_str()).ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("commit_deltas: essence_ops[{i}] missing 'kind'"),
                )
            })?;
            let kind = match kind_str {
                "add" => EssenceOpKind::Add,
                "append_timeline" => EssenceOpKind::AppendTimeline,
                "update" => EssenceOpKind::Update,
                "resolve" => EssenceOpKind::Resolve,
                "remove" => EssenceOpKind::Remove,
                other => {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        format!("commit_deltas: essence_ops[{i}] unknown kind '{other}'"),
                    ));
                }
            };
            let thread = obj.get("thread").and_then(|v| v.as_str()).map(String::from);
            let project = obj
                .get("project")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let scope = match (thread, project) {
                (Some(t), false) => Some(EssenceScope::Thread(t)),
                (None, true) => Some(EssenceScope::Project),
                (None, false) => None,
                (Some(_), true) => {
                    return Err(RpcError::new(
                        INVALID_PARAMS,
                        format!(
                            "commit_deltas: essence_ops[{i}] can't set both 'thread' and 'project'"
                        ),
                    ));
                }
            };
            let section = obj
                .get("section")
                .and_then(|v| v.as_str())
                .and_then(EssenceSection::parse);
            let id = obj
                .get("id")
                .and_then(|v| v.as_str())
                .map(Ulid::from_string)
                .transpose()
                .map_err(|_| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("commit_deltas: essence_ops[{i}] 'id' is not a valid ULID"),
                    )
                })?;
            let text = obj.get("text").and_then(|v| v.as_str()).map(String::from);
            let refs = obj.get("refs").and_then(|v| v.as_array()).map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            });
            batch.essence_ops.push(EssenceOp {
                kind,
                scope,
                section,
                id,
                text,
                refs,
            });
        }
    }

    // turns
    if let Some(arr) = args.get("turns").and_then(|v| v.as_array()) {
        for (i, t) in arr.iter().enumerate() {
            let obj = t.as_object().ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("commit_deltas: turns[{i}] must be an object"),
                )
            })?;
            let turn_id = obj
                .get("turn_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("commit_deltas: turns[{i}] missing 'turn_id'"),
                    )
                })?
                .to_string();
            let role = obj
                .get("role")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("commit_deltas: turns[{i}] missing 'role'"),
                    )
                })?
                .to_string();
            let content = obj
                .get("content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        format!("commit_deltas: turns[{i}] missing 'content'"),
                    )
                })?
                .to_string();
            let timestamp = obj.get("timestamp").and_then(|v| v.as_i64());
            batch.turns.push(ThreadTurn {
                turn_id,
                role,
                content,
                timestamp,
            });
        }
    }
    batch.thread_id = args
        .get("thread_id")
        .and_then(|v| v.as_str())
        .map(String::from);
    let policy_str = args
        .get("turn_policy")
        .and_then(|v| v.as_str())
        .unwrap_or("archive");
    batch.turn_policy = match policy_str {
        "archive" => TurnPolicy::Archive,
        "drop" => TurnPolicy::Drop,
        "expire_after" => {
            let ttl = args
                .get("turn_policy_ttl")
                .and_then(|v| v.as_i64())
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        "commit_deltas: turn_policy='expire_after' requires 'turn_policy_ttl' (seconds)",
                    )
                })?;
            TurnPolicy::ExpireAfter(ttl)
        }
        other => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("commit_deltas: unknown turn_policy '{other}'"),
            ));
        }
    };

    let report = db.commit_deltas(batch).map_err(mnemo_err)?;
    Ok(json!({
        "facts_added": report.facts_added,
        "facts_superseded": report.facts_superseded,
        "facts_unchanged": report.facts_unchanged,
        "essence_ops_applied": report.essence_ops_applied,
        "essence_ops_unchanged": report.essence_ops_unchanged,
        "turns_added": report.turns_added,
        "turns_skipped": report.turns_skipped,
        "turns_truncated": report.turns_truncated,
        "turns_chunked": report.turns_chunked,
    })
    .to_string())
}

fn tool_essence(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    use crate::{EssenceScope, EssenceSection, Ulid, WriteOutcome};
    let op = args
        .get("op")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence: missing 'op'"))?;
    let thread = args
        .get("thread")
        .and_then(|v| v.as_str())
        .map(String::from);
    let project = args
        .get("project")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let scope_from = |t: Option<String>, p: bool| -> std::result::Result<EssenceScope, RpcError> {
        match (t, p) {
            (Some(s), false) => Ok(EssenceScope::Thread(s)),
            (None, true) => Ok(EssenceScope::Project),
            (None, false) => Err(RpcError::new(
                INVALID_PARAMS,
                "essence: pass either `thread` or `project: true`",
            )),
            (Some(_), true) => Err(RpcError::new(
                INVALID_PARAMS,
                "essence: `thread` and `project` are mutually exclusive",
            )),
        }
    };
    let refs: Option<Vec<String>> = args.get("refs").and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect()
    });
    let text = args.get("text").and_then(|v| v.as_str()).map(String::from);
    let id_str = args.get("id").and_then(|v| v.as_str());
    let outcome_to_str = |o: WriteOutcome| -> &'static str {
        match o {
            WriteOutcome::Updated => "updated",
            WriteOutcome::Unchanged => "unchanged",
            WriteOutcome::Added => "updated",
            WriteOutcome::Superseded => "updated",
        }
    };
    match op {
        "add" => {
            let section_str = args
                .get("section")
                .and_then(|v| v.as_str())
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence add: missing 'section'"))?;
            let section = EssenceSection::parse(section_str).ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("essence add: unknown section '{section_str}'"),
                )
            })?;
            if section == EssenceSection::Timeline {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "essence add: timeline uses op: append, not add",
                ));
            }
            let text =
                text.ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence add: missing 'text'"))?;
            let scope = scope_from(thread, project)?;
            let id = db
                .essence_add(scope, section, &text, refs)
                .map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            Ok(json!({ "id": id.to_string(), "outcome": "added" }).to_string())
        }
        "append" => {
            let text = text
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence append: missing 'text'"))?;
            let scope = scope_from(thread, project)?;
            let id = db
                .essence_append_timeline(scope, &text, refs)
                .map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            Ok(json!({ "id": id.to_string(), "outcome": "appended" }).to_string())
        }
        "update" => {
            let id = id_str
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence update: missing 'id'"))?;
            let ulid = Ulid::from_string(id).map_err(|_| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("essence update: invalid ULID '{id}'"),
                )
            })?;
            let text = text
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence update: missing 'text'"))?;
            let outcome = db.essence_update(&ulid, &text, refs).map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            Ok(json!({ "id": id, "outcome": outcome_to_str(outcome) }).to_string())
        }
        "resolve" => {
            let id = id_str
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence resolve: missing 'id'"))?;
            let ulid = Ulid::from_string(id).map_err(|_| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("essence resolve: invalid ULID '{id}'"),
                )
            })?;
            let outcome = db.essence_resolve(&ulid).map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            let s = match outcome {
                WriteOutcome::Updated => "resolved",
                WriteOutcome::Unchanged => "already-resolved",
                _ => "resolved",
            };
            Ok(json!({ "id": id, "outcome": s }).to_string())
        }
        "remove" => {
            let id = id_str
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "essence remove: missing 'id'"))?;
            let ulid = Ulid::from_string(id).map_err(|_| {
                RpcError::new(
                    INVALID_PARAMS,
                    format!("essence remove: invalid ULID '{id}'"),
                )
            })?;
            db.essence_remove(&ulid).map_err(mnemo_err)?;
            db.flush().map_err(mnemo_err)?;
            Ok(json!({ "id": id, "outcome": "removed" }).to_string())
        }
        "render" => {
            let budget = args.get("budget").and_then(|v| v.as_u64()).unwrap_or(6144) as usize;
            let scope = scope_from(thread, project)?;
            let rendered = db.essence(scope, budget).map_err(mnemo_err)?;
            Ok(rendered)
        }
        other => Err(RpcError::new(
            INVALID_PARAMS,
            format!("essence: unknown op '{other}'"),
        )),
    }
}

fn tool_thread_ingest(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let thread = args
        .get("thread")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "thread_ingest: missing 'thread'"))?;
    let path = args
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "thread_ingest: missing 'transcript_path'"))?;
    let format_str = args
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("jsonl");
    let format = crate::TranscriptFormat::parse(format_str).ok_or_else(|| {
        RpcError::new(
            INVALID_PARAMS,
            format!(
                "thread_ingest: unknown format '{format_str}' (want 'jsonl' or 'claude-code-jsonl')"
            ),
        )
    })?;
    let file = std::fs::File::open(path).map_err(|e| {
        RpcError::new(
            INVALID_PARAMS,
            format!("thread_ingest: cannot open {path}: {e}"),
        )
    })?;
    let turns = crate::read_transcript(file, format).map_err(mnemo_err)?;
    let report = db.ingest_thread(thread, &turns).map_err(mnemo_err)?;
    db.flush().map_err(mnemo_err)?;
    Ok(json!({
        "added_records": report.added_records,
        "skipped_existing_turns": report.skipped_existing_turns,
        "chunked_turns": report.chunked_turns,
        "truncated_turns": report.truncated_turns,
    })
    .to_string())
}

fn tool_history(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    use crate::KeyScope;
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "history: missing 'key'"))?;
    let agent = args.get("agent").and_then(|v| v.as_str());
    let shared = args
        .get("shared")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if shared && agent.is_some() {
        return Err(RpcError::new(
            INVALID_PARAMS,
            "history: 'agent' and 'shared' are mutually exclusive",
        ));
    }
    // v0.8.1 bugfix (lane A): match keyed `remember` slot resolution.
    let scope = if shared {
        KeyScope::Shared
    } else {
        match agent {
            Some(a) => KeyScope::Private(a.to_string()),
            None => KeyScope::private_default(),
        }
    };
    let chain = db.history_in(key, scope).map_err(mnemo_err)?;
    let out: Vec<Value> = chain
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut v = memory_to_json(m);
            if let Some(obj) = v.as_object_mut() {
                obj.insert(
                    "state".into(),
                    Value::String(if i == 0 {
                        "live".into()
                    } else {
                        "superseded".into()
                    }),
                );
            }
            v
        })
        .collect();
    Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into()))
}

fn tool_recall(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    // Accept either `query` (explicit) or `text` (embedded).
    let query = match (args.get("query"), args.get("text")) {
        (Some(_), Some(_)) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "recall: pass EITHER 'query' or 'text', not both",
            ))
        }
        (Some(v), None) => v
            .as_array()
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "recall: 'query' must be a JSON array"))?
            .iter()
            .map(|v| v.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<f32>>>()
            .ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "recall: 'query' must be an array of numbers",
                )
            })?,
        (None, Some(t)) => {
            let text = t
                .as_str()
                .ok_or_else(|| RpcError::new(INVALID_PARAMS, "recall: 'text' must be a string"))?;
            embed_text_via_server(db, "recall", text)?
        }
        (None, None) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "recall: missing 'query' or 'text'",
            ))
        }
    };
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let metric = match args
        .get("metric")
        .and_then(|v| v.as_str())
        .unwrap_or("cosine")
    {
        "cosine" => Metric::Cosine,
        "l2" => Metric::L2,
        "dot" => Metric::Dot,
        other => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                format!("recall: unknown metric '{other}'"),
            ))
        }
    };
    // Scoring v2 (v0.7.0) flipped the default: MCP recalls DO track
    // access, so an agent's own retrievals contribute to the usage
    // saturation term on later queries. The bump happens in-memory
    // (`accessed_at` / `access_count` + `dirty_catalog`), and persists
    // through the next flush — recall itself does not fsync, so there
    // is no per-tool-call write-amplification cost. Access stats reach
    // disk on either (a) the next mutating tool call's exit path (every
    // one of `remember` / `forget` / `cache_put_semantic` /
    // `cache_get_semantic`-on-miss ends in `db.flush()`), or (b) the
    // server's own shutdown flush when stdin closes (see
    // `serve_with_streams`), so a read-only session that issued only
    // recalls still persists its stats.
    let mut req = RecallRequest::new(query)
        .top_k(top_k)
        .metric(metric)
        .track_access(true);
    if let Some(types) = args.get("types").and_then(|v| v.as_array()) {
        let parsed: Vec<MemoryType> = types
            .iter()
            .filter_map(|t| t.as_str().and_then(MemoryType::parse))
            .collect();
        if !parsed.is_empty() {
            req = req.types(parsed);
        }
    }
    if let Some(agent) = args.get("agent").and_then(|v| v.as_str()) {
        req = req.agent(agent.to_string());
    }
    // Phase 12.6: filter object. Shape:
    //   { "session_id": "...",
    //     "metadata": { "k": v,
    //                   "k2": {"$in": [v1, v2]},
    //                   "k3": {"$exists": true},
    //                   "k4": {"$gte": 5, "$lte": 10} } }
    if let Some(filter_obj) = args.get("filter").and_then(|v| v.as_object()) {
        req = req.filter(parse_mcp_filter(filter_obj)?);
    }
    // Phase 12.8: `max_bytes` opts into the budgeted response shape.
    let has_max_bytes = args.get("max_bytes").is_some();
    if let Some(n) = args.get("max_bytes").and_then(|v| v.as_u64()) {
        req = req.max_bytes(n as usize);
    }

    let resp = db.recall_budgeted(&req).map_err(mnemo_err)?;
    let out: Vec<Value> = resp
        .hits
        .into_iter()
        .map(|h| {
            let mut obj = memory_to_json(&h.memory);
            if let Some(map) = obj.as_object_mut() {
                map.insert("score".into(), json!(h.score));
                map.insert("similarity".into(), json!(h.similarity));
            }
            obj
        })
        .collect();
    if has_max_bytes {
        // Phase 12.8 wrapped shape so callers can tell whether the
        // budget cut anything.
        let wrapped = json!({
            "hits": out,
            "truncated": resp.truncated,
        });
        Ok(serde_json::to_string_pretty(&wrapped).unwrap_or_else(|_| "{}".into()))
    } else {
        Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into()))
    }
}

/// Phase 12.6: parse an MCP `filter` object into a [`crate::Filter`].
///
/// Values under `metadata.<key>`:
/// * plain scalar / array / string → [`crate::FilterTerm::Eq`]
/// * `{"$exists": true}` → [`crate::FilterTerm::Exists`]
/// * `{"$in": [...]}` → [`crate::FilterTerm::In`]
/// * `{"$gte": n, "$lte": n}` → [`crate::FilterTerm::Range`] (either bound
///   optional)
/// * `{"$exists": false}` is rejected (use the absence of a term instead)
fn parse_mcp_filter(
    obj: &serde_json::Map<String, Value>,
) -> std::result::Result<crate::Filter, RpcError> {
    let mut f = crate::Filter::default();
    if let Some(sid) = obj.get("session_id").and_then(|v| v.as_str()) {
        f = f.with_session(sid.to_string());
    }
    if let Some(meta) = obj.get("metadata").and_then(|v| v.as_object()) {
        for (k, v) in meta {
            match v {
                // Nested operator object.
                Value::Object(ops) => {
                    for (op_key, op_val) in ops {
                        match op_key.as_str() {
                            "$exists" => match op_val.as_bool() {
                                Some(true) => {
                                    f = f.metadata_exists(k.clone());
                                }
                                Some(false) => {
                                    return Err(RpcError::new(
                                        INVALID_PARAMS,
                                        format!(
                                            "filter.metadata.{k}.$exists=false isn't supported; \
                                             drop the filter term instead"
                                        ),
                                    ));
                                }
                                None => {
                                    return Err(RpcError::new(
                                        INVALID_PARAMS,
                                        format!("filter.metadata.{k}.$exists must be a boolean"),
                                    ));
                                }
                            },
                            "$in" => {
                                let arr = op_val.as_array().ok_or_else(|| {
                                    RpcError::new(
                                        INVALID_PARAMS,
                                        format!("filter.metadata.{k}.$in must be an array"),
                                    )
                                })?;
                                f = f.metadata_in(k.clone(), arr.clone());
                            }
                            "$gte" | "$lte" => {
                                let mut min = None;
                                let mut max = None;
                                for (bound, bound_val) in ops {
                                    let n = bound_val.as_f64().ok_or_else(|| {
                                        RpcError::new(
                                            INVALID_PARAMS,
                                            format!("filter.metadata.{k}.{bound} must be a number"),
                                        )
                                    })?;
                                    match bound.as_str() {
                                        "$gte" => min = Some(n),
                                        "$lte" => max = Some(n),
                                        other => {
                                            return Err(RpcError::new(
                                                INVALID_PARAMS,
                                                format!(
                                                    "filter.metadata.{k}: unsupported range \
                                                     operator '{other}' (want $gte, $lte)"
                                                ),
                                            ));
                                        }
                                    }
                                }
                                f = f.metadata_range(k.clone(), min, max);
                                break; // All operator entries consumed in one pass.
                            }
                            other => {
                                return Err(RpcError::new(
                                    INVALID_PARAMS,
                                    format!(
                                        "filter.metadata.{k}: unknown operator '{other}' \
                                         (want $exists, $in, $gte, $lte)"
                                    ),
                                ));
                            }
                        }
                    }
                }
                // Plain scalar / array / string → equality.
                other => {
                    f = f.metadata_eq(k.clone(), other.clone());
                }
            }
        }
    }
    Ok(f)
}

fn tool_forget(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let id_str = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "forget: missing 'id'"))?;
    let ulid = crate::Ulid::from_string(id_str)
        .map_err(|_| RpcError::new(INVALID_PARAMS, format!("forget: invalid ULID '{id_str}'")))?;
    db.delete(&ulid).map_err(mnemo_err)?;
    db.flush().map_err(mnemo_err)?;
    Ok(json!({ "deleted": id_str }).to_string())
}

fn tool_list(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let include_vectors = args
        .get("include_vectors")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mems = db.memories().map_err(mnemo_err)?;
    let out: Vec<Value> = mems
        .iter()
        .map(|m| {
            let mut v = memory_to_json(m);
            if !include_vectors {
                if let Some(o) = v.as_object_mut() {
                    o.remove("vector");
                }
            }
            v
        })
        .collect();
    Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into()))
}

fn tool_snapshot_list(db: &mut Mnemo) -> std::result::Result<String, RpcError> {
    let snaps = db.snapshots();
    let out: Vec<Value> = snaps
        .into_iter()
        .map(|s| {
            json!({
                "txn_id": s.txn_id,
                "created_at": s.created_at,
                "memory_count": s.memory_count,
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into()))
}

fn tool_stats(db: &mut Mnemo) -> std::result::Result<String, RpcError> {
    let s = db.stats().map_err(mnemo_err)?;
    let (cache_pages, cache_capacity) = db.page_cache_stats();
    let snapshot_count = db.snapshots().len();
    // Field names mirror `Stats` in `store.rs` verbatim so MCP clients
    // that reference the Rust doc for the same DB see the same shape.
    Ok(json!({
        "memories": s.memories,
        "deleted": s.deleted,
        "dimensions": s.dimensions,
        "file_bytes": s.file_bytes,
        "agents": s.agents,
        "encrypted": s.encrypted,
        "created_at": s.created_at,
        "wal_pages": s.wal_pages,
        "index": s.index,
        "snapshot_count": snapshot_count,
        "cache_pages": cache_pages,
        "cache_capacity": cache_capacity,
    })
    .to_string())
}

// --- Result-cache tool handlers (Phase 10.4) -----------------------------

fn tool_cache_put(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let ns = args
        .get("namespace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_put: missing 'namespace'"))?;
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_put: missing 'key'"))?;
    let value = args
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_put: missing 'value'"))?;
    let content_type = args
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text")
        .to_string();
    let ttl_secs = args.get("ttl_secs").and_then(|v| v.as_u64());
    let opts = CachePutOpts {
        content_type,
        ttl_secs,
        cost_hint_ms: None,
    };
    db.cache_put(ns, key, value.as_bytes(), opts)
        .map_err(mnemo_err)?;
    db.flush().map_err(mnemo_err)?;
    Ok(json!({ "cached": true, "namespace": ns, "key": key, "bytes": value.len() }).to_string())
}

fn tool_cache_get(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let ns = args
        .get("namespace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_get: missing 'namespace'"))?;
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_get: missing 'key'"))?;
    match db.cache_get(ns, key).map_err(mnemo_err)? {
        Some(hit) => Ok(json!({
            "hit": true,
            "value": String::from_utf8_lossy(&hit.value),
            "content_type": hit.content_type,
            "created_at": hit.created_at,
            "accessed_at": hit.accessed_at,
            "access_count": hit.access_count,
        })
        .to_string()),
        None => Ok(json!({ "hit": false }).to_string()),
    }
}

fn tool_cache_stats(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let ns = args.get("namespace").and_then(|v| v.as_str());
    let s = db.cache_stats(ns);
    Ok(json!({
        "namespace": ns,
        "entries": s.entries,
        "bytes": s.bytes,
        "hits": s.hits,
        "misses": s.misses,
        "hit_rate": s.hit_rate,
        "evictions": s.evictions,
    })
    .to_string())
}

fn tool_cache_put_semantic(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let ns = args
        .get("namespace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_put_semantic: missing 'namespace'"))?;
    let key = args
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_put_semantic: missing 'key'"))?;
    let value = args
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_put_semantic: missing 'value'"))?;
    // When `text` is used, derive the model name from the server's
    // registered embedder (identity). Explicit `model` wins otherwise.
    let (vector, model) = match (args.get("vector"), args.get("text")) {
        (Some(_), Some(_)) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "cache_put_semantic: pass EITHER 'vector' or 'text', not both",
            ));
        }
        (Some(v), None) => {
            let vec = v
                .as_array()
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        "cache_put_semantic: 'vector' must be a JSON array",
                    )
                })?
                .iter()
                .map(|v| v.as_f64().map(|f| f as f32))
                .collect::<Option<Vec<f32>>>()
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        "cache_put_semantic: 'vector' must be an array of numbers",
                    )
                })?;
            let model = args
                .get("model")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    RpcError::new(INVALID_PARAMS, "cache_put_semantic: missing 'model'")
                })?
                .to_string();
            (vec, model)
        }
        (None, Some(t)) => {
            let text = t.as_str().ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "cache_put_semantic: 'text' must be a string",
                )
            })?;
            let vec = embed_text_via_server(db, "cache_put_semantic", text)?;
            // Allow explicit model override; else derive from embedder.
            let model = match args.get("model").and_then(|v| v.as_str()) {
                Some(m) => m.to_string(),
                None => {
                    db.embedder()
                        .expect("embed_text_via_server would have errored otherwise")
                        .identity()
                        .name
                }
            };
            (vec, model)
        }
        (None, None) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "cache_put_semantic: missing 'vector' or 'text'",
            ))
        }
    };
    let content_type = args
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text")
        .to_string();
    let ttl_secs = args.get("ttl_secs").and_then(|v| v.as_u64());
    let opts = SemanticCachePutOpts {
        content_type,
        ttl_secs,
        cost_hint_ms: None,
        model,
    };
    db.cache_put_semantic(ns, key, vector, value.as_bytes(), opts)
        .map_err(mnemo_err)?;
    db.flush().map_err(mnemo_err)?;
    Ok(json!({ "cached": true, "namespace": ns, "key": key, "bytes": value.len() }).to_string())
}

fn tool_cache_get_semantic(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let ns = args
        .get("namespace")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "cache_get_semantic: missing 'namespace'"))?;
    // When `text` is used, derive the model name from the server's
    // registered embedder (identity). Explicit `model` wins otherwise.
    let (query, model_owned) = match (args.get("query"), args.get("text")) {
        (Some(_), Some(_)) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "cache_get_semantic: pass EITHER 'query' or 'text', not both",
            ));
        }
        (Some(q), None) => {
            let vec = q
                .as_array()
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        "cache_get_semantic: 'query' must be a JSON array",
                    )
                })?
                .iter()
                .map(|v| v.as_f64().map(|f| f as f32))
                .collect::<Option<Vec<f32>>>()
                .ok_or_else(|| {
                    RpcError::new(
                        INVALID_PARAMS,
                        "cache_get_semantic: 'query' must be an array of numbers",
                    )
                })?;
            let model = args
                .get("model")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    RpcError::new(INVALID_PARAMS, "cache_get_semantic: missing 'model'")
                })?
                .to_string();
            (vec, model)
        }
        (None, Some(t)) => {
            let text = t.as_str().ok_or_else(|| {
                RpcError::new(
                    INVALID_PARAMS,
                    "cache_get_semantic: 'text' must be a string",
                )
            })?;
            let vec = embed_text_via_server(db, "cache_get_semantic", text)?;
            let model = match args.get("model").and_then(|v| v.as_str()) {
                Some(m) => m.to_string(),
                None => {
                    db.embedder()
                        .expect("embed_text_via_server would have errored otherwise")
                        .identity()
                        .name
                }
            };
            (vec, model)
        }
        (None, None) => {
            return Err(RpcError::new(
                INVALID_PARAMS,
                "cache_get_semantic: missing 'query' or 'text'",
            ))
        }
    };
    let model = model_owned.as_str();
    let threshold = args
        .get("threshold")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32)
        .unwrap_or(DEFAULT_SEMANTIC_THRESHOLD);
    match db
        .cache_get_semantic(ns, &query, threshold, model)
        .map_err(mnemo_err)?
    {
        Some((hit, similarity)) => Ok(json!({
            "hit": true,
            "similarity": similarity,
            "value": String::from_utf8_lossy(&hit.value),
            "content_type": hit.content_type,
            "created_at": hit.created_at,
            "accessed_at": hit.accessed_at,
            "access_count": hit.access_count,
        })
        .to_string()),
        None => Ok(json!({ "hit": false }).to_string()),
    }
}

/// Serialize a `Memory` into the JSON shape MCP tool responses use.
/// Serde on `Memory` produces the canonical shape; we mirror what the
/// CLI's `memory_to_json` does but without the CLI-only vector-stripping
/// switch (that lives in the individual tools that care).
fn memory_to_json(m: &Memory) -> Value {
    serde_json::to_value(m).unwrap_or_else(|_| {
        let mut map = Map::new();
        map.insert("id".into(), json!(m.id.to_string()));
        Value::Object(map)
    })
}
