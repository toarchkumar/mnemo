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
use crate::store::{Mnemo, RecallRequest};

/// MCP protocol version we advertise on `initialize`.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Environment variable that supplies the database passphrase.
const PASSPHRASE_ENV: &str = "MNEMO_PASSPHRASE";

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
        Self { code, message: message.into(), data: None }
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
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    serve_with_streams(path, stdin.lock(), stdout.lock())
}

/// Run the MCP loop against explicit reader/writer streams. Extracted so
/// integration tests can pipe scripted input without touching the real
/// stdin/stdout.
pub fn serve_with_streams<R: Read, W: Write>(
    path: &Path,
    reader: R,
    mut writer: W,
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

        let line = serde_json::to_string(&resp)
            .map_err(|e| MnemoError::Serialize(e.to_string()))?;
        writeln!(writer, "{line}").map_err(MnemoError::Io)?;
        writer.flush().map_err(MnemoError::Io)?;
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
                "description": "Read-only briefing for this .mnemo file: manifest scaffold plus any memories tagged as onboarding. Run this first to learn how the file expects to be used.",
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }
            },
            {
                "name": "remember",
                "description": "Insert or overwrite a memory. Requires a caller-supplied embedding `vector` (this server is embedder-agnostic; text-only remember is deferred to Phase 3 of the level-up plan). Persists to disk before returning.",
                "inputSchema": {
                    "type": "object",
                    "required": ["content", "memory_type", "vector"],
                    "properties": {
                        "content": { "type": "string" },
                        "memory_type": { "type": "string", "enum": ["episodic", "semantic", "procedural", "working"] },
                        "vector": { "type": "array", "items": { "type": "number" } },
                        "importance": { "type": "number", "minimum": 0, "maximum": 1 },
                        "agent_id": { "type": "string" },
                        "session_id": { "type": "string" },
                        "ttl_secs": { "type": "integer" },
                        "shared": { "type": "boolean" },
                        "metadata": { "type": "object" }
                    },
                    "additionalProperties": false
                }
            },
            {
                "name": "recall",
                "description": "Vector recall: score every live memory (or the ANN-narrowed candidate set) against a caller-supplied query vector and return the top-k. Does not track access on this handle.",
                "inputSchema": {
                    "type": "object",
                    "required": ["query"],
                    "properties": {
                        "query": { "type": "array", "items": { "type": "number" } },
                        "top_k": { "type": "integer", "default": 10 },
                        "types": { "type": "array", "items": { "type": "string" } },
                        "agent": { "type": "string" },
                        "metric": { "type": "string", "enum": ["cosine", "l2", "dot"], "default": "cosine" }
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
        "recall" => tool_recall(db, args)?,
        "forget" => tool_forget(db, args)?,
        "list" => tool_list(db, args)?,
        "snapshot_list" => tool_snapshot_list(db)?,
        "stats" => tool_stats(db)?,
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
fn mnemo_err(e: MnemoError) -> RpcError {
    RpcError::new(INTERNAL_ERROR, e.to_string())
}

fn tool_about(db: &mut Mnemo) -> std::result::Result<String, RpcError> {
    let mems = db.about().map_err(mnemo_err)?;
    let arr: Vec<Value> = mems.iter().map(memory_to_json).collect();
    Ok(serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".into()))
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
    let vector = args
        .get("vector")
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "remember: 'vector' must be a JSON array"))?
        .iter()
        .map(|v| v.as_f64().map(|f| f as f32))
        .collect::<Option<Vec<f32>>>()
        .ok_or_else(|| {
            RpcError::new(INVALID_PARAMS, "remember: 'vector' must be an array of numbers")
        })?;

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

    let id = db.remember(mem).map_err(mnemo_err)?;
    db.flush().map_err(mnemo_err)?;
    Ok(json!({ "id": id.to_string() }).to_string())
}

fn tool_recall(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let query = args
        .get("query")
        .and_then(|v| v.as_array())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "recall: 'query' must be a JSON array"))?
        .iter()
        .map(|v| v.as_f64().map(|f| f as f32))
        .collect::<Option<Vec<f32>>>()
        .ok_or_else(|| {
            RpcError::new(INVALID_PARAMS, "recall: 'query' must be an array of numbers")
        })?;
    let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
    let metric = match args.get("metric").and_then(|v| v.as_str()).unwrap_or("cosine") {
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
    // Deliberately track_access(false) so the MCP server never mutates
    // catalog state on a read-only-shaped tool call. Flushing after
    // every mutating tool call keeps the file durable.
    let mut req = RecallRequest::new(query)
        .top_k(top_k)
        .metric(metric)
        .track_access(false);
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

    let hits = db.recall(&req).map_err(mnemo_err)?;
    let out: Vec<Value> = hits
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
    Ok(serde_json::to_string_pretty(&out).unwrap_or_else(|_| "[]".into()))
}

fn tool_forget(db: &mut Mnemo, args: &Value) -> std::result::Result<String, RpcError> {
    let id_str = args
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| RpcError::new(INVALID_PARAMS, "forget: missing 'id'"))?;
    let ulid = crate::Ulid::from_string(id_str).map_err(|_| {
        RpcError::new(INVALID_PARAMS, format!("forget: invalid ULID '{id_str}'"))
    })?;
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
    let (cache_pages, cache_capacity) = db.cache_stats();
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
