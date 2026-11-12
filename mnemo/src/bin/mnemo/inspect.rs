//! `mnemo inspect`: the read-only inspector (PLAN.md 14.2, the read-only
//! half).
//!
//! **How it reads.** It opens the file read-only once, reads everything the
//! views show into memory, and closes the handle, so it holds no lock
//! between reads and never blocks a writer. "Reload" repeats that (one KDF
//! per reload). Keyword search runs over what was read
//! ([`mnemo_engine::KeywordIndex`], the same tokenizer and BM25 as v9's
//! keyword recall), so it needs no embedder and works on v8 files too.
//!
//! **How it serves.** A small HTTP/1.1 server on `std::net::TcpListener`,
//! bound to `127.0.0.1` only. Every request must carry the random token
//! from the printed URL and a `Host` header naming the loopback address and
//! this port (against DNS rebinding and other sites' pages). Only `GET` is
//! served, with no CORS headers, and no endpoint changes the file. The page
//! is one static HTML file compiled in (`inspect.html`).

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use mnemo_engine::{
    EssenceScope, KeywordIndex, Memory, Mnemo, MnemoError, Scope, MNEMO_META_INVALID_AT,
    MNEMO_META_KEY, MNEMO_META_KIND, MNEMO_META_KIND_TURN, MNEMO_META_SUPERSEDED_BY,
    MNEMO_META_THREAD,
};
use serde_json::{json, Map, Value};

/// The page, compiled in: no build step and no files beside the binary.
const PAGE: &str = include_str!("inspect.html");

/// The byte budget for each essence render (the views show it whole).
const ESSENCE_BUDGET: usize = 64 * 1024;

/// The most memories one `/api/memories` page returns.
const MAX_PAGE: usize = 1000;

/// The longest request head read, in bytes.
const MAX_HEAD: usize = 16 * 1024;

/// What unlocks the file.
/// Both are wiped from memory when dropped.
pub enum Secret {
    /// A passphrase (the prompt, `MNEMO_PASSPHRASE` or `--passphrase`).
    Passphrase(Zeroizing<String>),
    /// A raw 32-byte key (`--key-file`).
    Key(Zeroizing<[u8; 32]>),
}

/// A raw key from `--key-file`: 32 bytes as they are, or 64 hex digits.
pub fn read_key_file(path: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    let bytes = Zeroizing::new(std::fs::read(path).map_err(|e| format!("reading {path}: {e}"))?);
    if bytes.len() == 32 {
        let mut k = Zeroizing::new([0u8; 32]);
        k.copy_from_slice(&bytes);
        return Ok(k);
    }
    let hex = String::from_utf8_lossy(&bytes);
    let hex = hex.trim();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!(
            "{path}: a key file holds 32 raw bytes or 64 hex digits"
        ));
    }
    let mut k = Zeroizing::new([0u8; 32]);
    for (i, byte) in k.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("checked hex");
    }
    Ok(k)
}

/// One version of a keyed fact, oldest first in its chain.
struct Version {
    id: String,
    content: String,
    created_at: i64,
    /// When it stopped being current (`None` for the current one).
    invalid_at: Option<i64>,
    /// The next version (`_mnemo.superseded_by`).
    superseded_by: Option<String>,
}

/// Put a key's versions in supersession order, oldest first, by following
/// the `superseded_by` links (ids alone can't: two ULIDs from the same
/// millisecond sort randomly). A version nothing points at starts a chain;
/// chains start in creation order, and anything left over goes last.
fn chain_order(versions: Vec<Version>) -> Vec<Version> {
    let targets: BTreeSet<String> = versions
        .iter()
        .filter_map(|v| v.superseded_by.clone())
        .collect();
    let mut by_id: BTreeMap<String, Version> =
        versions.into_iter().map(|v| (v.id.clone(), v)).collect();
    let mut starts: Vec<(i64, String)> = by_id
        .values()
        .filter(|v| !targets.contains(&v.id))
        .map(|v| (v.created_at, v.id.clone()))
        .collect();
    starts.sort();
    let mut out = Vec::new();
    for (_, id) in starts {
        let mut next = Some(id);
        while let Some(v) = next.and_then(|id| by_id.remove(&id)) {
            next = v.superseded_by.clone();
            out.push(v);
        }
    }
    let mut rest: Vec<Version> = by_id.into_values().collect();
    rest.sort_by(|a, b| (a.created_at, &a.id).cmp(&(b.created_at, &b.id)));
    out.extend(rest);
    out
}

/// One thread: its essence render and its turn archive.
struct Thread {
    id: String,
    essence: String,
    essence_items: usize,
    turns: Vec<Value>,
}

/// Everything the views show, read in one pass.
pub struct Loaded {
    about: Value,
    /// Every live record except thread turns, by id.
    memories: Vec<Value>,
    contents: KeywordIndex,
    /// The agents of `memories`, sorted.
    agents: Vec<String>,
    /// `(key, owner)` → versions, oldest first. The owner is the agent for
    /// a private key and empty for a shared one.
    keys: BTreeMap<(String, String), Vec<Version>>,
    threads: Vec<Thread>,
    project_essence: Option<String>,
    snapshots: Value,
    loaded_at: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The open error a person can act on.
fn open_error(e: MnemoError) -> String {
    match e {
        MnemoError::Locked { .. } => {
            "a writer has this file open; close it or try again".to_string()
        }
        other => other.to_string(),
    }
}

fn meta_str<'a>(m: &'a Memory, k: &str) -> Option<&'a str> {
    m.metadata.get(k).and_then(Value::as_str)
}

fn memory_json(m: &Memory) -> Value {
    let mut r = Map::new();
    r.insert("id".into(), json!(m.id.to_string()));
    r.insert("type".into(), json!(m.memory_type.as_str()));
    r.insert("agent".into(), json!(m.agent_id));
    r.insert(
        "scope".into(),
        json!(match m.scope {
            Scope::Private => "private",
            Scope::Shared => "shared",
        }),
    );
    for (field, k) in [
        ("key", MNEMO_META_KEY),
        ("thread", MNEMO_META_THREAD),
        ("kind", MNEMO_META_KIND),
        ("superseded_by", MNEMO_META_SUPERSEDED_BY),
    ] {
        if let Some(v) = meta_str(m, k) {
            r.insert(field.into(), json!(v));
        }
    }
    if let Some(t) = m.metadata.get(MNEMO_META_INVALID_AT) {
        r.insert("invalid_at".into(), t.clone());
    }
    if let Some(s) = &m.session_id {
        r.insert("session".into(), json!(s));
    }
    r.insert("created_at".into(), json!(m.created_at));
    r.insert("accessed_at".into(), json!(m.accessed_at));
    r.insert("access_count".into(), json!(m.access_count));
    r.insert(
        "importance".into(),
        format!("{}", m.importance)
            .parse::<f64>()
            .map(|x| json!(x))
            .unwrap_or(Value::Null),
    );
    if let Some(t) = m.ttl_secs {
        r.insert("ttl_secs".into(), json!(t));
    }
    r.insert("metadata".into(), Value::Object(m.metadata.clone()));
    r.insert("content".into(), json!(m.content));
    Value::Object(r)
}

/// Open `path` read-only, read everything, and close it.
pub fn load(path: &str, secret: &Secret) -> Result<Loaded, String> {
    let mut db = match secret {
        Secret::Passphrase(p) => Mnemo::open_read_only(path, p),
        Secret::Key(k) => Mnemo::open_read_only_with_key(path, k),
    }
    .map_err(open_error)?;
    let e = |e: MnemoError| e.to_string();

    let stats = db.stats().map_err(e)?;
    let summary = db.stats_summary();
    let identity = db.embedder_identity().map_err(e)?;
    let briefing = db.about().map_err(e)?;
    // A file from before EMBED-ID-1 records no identity; its manifest may
    // still name the embedder.
    let manifest_embedder = briefing
        .iter()
        .find(|m| meta_str(m, "topic") == Some("manifest"))
        .and_then(|m| m.metadata.get("embedder").cloned());
    let onboarding: Vec<Value> = briefing
        .iter()
        .map(|m| {
            json!({
                "id": m.id.to_string(),
                "topic": meta_str(m, "topic"),
                "content": m.content,
            })
        })
        .collect();
    let cache = db.cache_stats(None);
    let all = db.memories().map_err(e)?;

    let mut thread_ids: BTreeSet<String> = BTreeSet::new();
    let mut essence_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut project_items = 0usize;
    let mut keys: BTreeMap<(String, String), Vec<Version>> = BTreeMap::new();
    let mut shown: Vec<&Memory> = Vec::new();
    for m in &all {
        let kind = meta_str(m, MNEMO_META_KIND);
        if let Some(t) = meta_str(m, MNEMO_META_THREAD) {
            thread_ids.insert(t.to_string());
            if kind == Some("essence") {
                *essence_counts.entry(t.to_string()).or_default() += 1;
            }
        }
        if kind == Some("essence") && meta_str(m, MNEMO_META_THREAD).is_none() {
            project_items += 1;
        }
        if kind == Some(MNEMO_META_KIND_TURN) {
            continue;
        }
        if let Some(k) = meta_str(m, MNEMO_META_KEY) {
            let owner = match m.scope {
                Scope::Private => m.agent_id.clone(),
                Scope::Shared => String::new(),
            };
            keys.entry((k.to_string(), owner))
                .or_default()
                .push(Version {
                    id: m.id.to_string(),
                    content: m.content.clone(),
                    created_at: m.created_at,
                    invalid_at: m
                        .metadata
                        .get(MNEMO_META_INVALID_AT)
                        .and_then(Value::as_i64),
                    superseded_by: meta_str(m, MNEMO_META_SUPERSEDED_BY).map(str::to_string),
                });
        }
        shown.push(m);
    }
    shown.sort_by_key(|m| m.id);
    let keys: BTreeMap<(String, String), Vec<Version>> = keys
        .into_iter()
        .map(|(k, vs)| (k, chain_order(vs)))
        .collect();
    // RB-003.7: build what the views keep, then let the records (vectors
    // and all) go before the thread turns are read, so the peak doesn't
    // hold both.
    let contents = KeywordIndex::new(shown.iter().map(|m| m.content.as_str()));
    let memories: Vec<Value> = shown.iter().map(|m| memory_json(m)).collect();
    let agents: Vec<String> = shown
        .iter()
        .map(|m| m.agent_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    drop(shown);
    drop(all);

    let mut threads = Vec::new();
    for id in &thread_ids {
        let essence = db
            .essence(EssenceScope::Thread(id.clone()), ESSENCE_BUDGET)
            .map_err(e)?;
        let turns = db
            .thread_export_turns(id)
            .map_err(e)?
            .iter()
            .map(|t| t.to_json())
            .collect();
        threads.push(Thread {
            id: id.clone(),
            essence,
            essence_items: essence_counts.get(id).copied().unwrap_or(0),
            turns,
        });
    }
    let project = db
        .essence(EssenceScope::Project, ESSENCE_BUDGET)
        .map_err(e)?;
    let snaps: Vec<Value> = db
        .snapshots()
        .iter()
        .map(|s| {
            json!({
                "txn_id": s.txn_id,
                "created_at": s.created_at,
                "memory_count": s.memory_count,
            })
        })
        .collect();
    drop(db);

    let about = json!({
        "path": path,
        "format_version": summary.format_version,
        "recall_mode": summary.recall_mode.as_str(),
        "dimensions": summary.dimensions,
        "memories": summary.memories,
        "tombstoned": summary.deleted,
        "file_bytes": summary.file_bytes,
        "wal_pages": summary.wal_pages,
        "has_index": summary.has_index,
        "vector_column_bytes": summary.vector_column_bytes,
        "created_at": stats.created_at,
        "agents": stats.agents,
        "embedder": identity.map(|i| json!({
            "name": i.name,
            "dimensions": i.dimensions,
            "normalize": i.normalize,
        })),
        "manifest_embedder": manifest_embedder,
        "cache": { "entries": cache.entries, "bytes": cache.bytes },
        "onboarding": onboarding,
        "threads": thread_ids.len(),
        "keys": keys.len(),
    });
    let snapshots = json!({
        "retained": summary.snapshots_retained,
        "oldest_restorable_at": summary.oldest_restorable_at,
        "pinned_bytes": summary.snapshot_pinned_bytes,
        "list": snaps,
    });
    Ok(Loaded {
        about,
        memories,
        contents,
        agents,
        keys,
        threads,
        // The render has a header even with no items.
        project_essence: (project_items > 0).then_some(project),
        snapshots,
        loaded_at: now(),
    })
}

/// The server's state: the last load, and what it needs to reload.
pub struct State {
    path: String,
    secret: Secret,
    loaded: Loaded,
}

impl State {
    /// Wrap a first load.
    pub fn new(path: String, secret: Secret, loaded: Loaded) -> Self {
        Self {
            path,
            secret,
            loaded,
        }
    }
}

/// Bind `127.0.0.1` (a random free port without `port`). Never another
/// interface.
pub fn bind(port: Option<u16>) -> std::io::Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port.unwrap_or(0)))
}

/// A random token for the URL: 32 bytes from the engine's key generator.
pub fn new_token() -> String {
    Mnemo::generate_key()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// What the server and its helper threads share: when the last request
/// with the token came in, and whether the page has been served.
pub struct Watch {
    started: Instant,
    last_secs: AtomicU64,
    page_served: AtomicBool,
}

impl Watch {
    /// A watch that starts now.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            started: Instant::now(),
            last_secs: AtomicU64::new(0),
            page_served: AtomicBool::new(false),
        })
    }

    fn touch(&self) {
        self.last_secs
            .store(self.started.elapsed().as_secs(), Ordering::Relaxed);
    }

    fn idle_for(&self) -> Duration {
        let last = Duration::from_secs(self.last_secs.load(Ordering::Relaxed));
        self.started.elapsed().saturating_sub(last)
    }
}

/// `--idle-timeout`: `30m`, `90s`, `1h`, or a bare number of minutes.
/// `0` means never.
pub fn parse_idle(s: &str) -> Result<Option<Duration>, String> {
    let s = s.trim();
    let (num, unit) = match s.char_indices().find(|(_, c)| !c.is_ascii_digit()) {
        Some((i, _)) => s.split_at(i),
        None => (s, "m"),
    };
    let n: u64 = num
        .parse()
        .map_err(|_| format!("--idle-timeout {s:?}: use a number with s, m or h (30m)"))?;
    let secs = match unit {
        "s" => n,
        "m" => n * 60,
        "h" => n * 3600,
        _ => return Err(format!("--idle-timeout {s:?}: use s, m or h (30m)")),
    };
    Ok((secs > 0).then(|| Duration::from_secs(secs)))
}

/// Accept connections until the process stops, one thread each. With
/// `idle`, the server stops once no request carrying the token has come in
/// for that long: it wipes the secret, drops what it read, and exits.
pub fn serve(
    listener: TcpListener,
    token: String,
    state: Arc<Mutex<State>>,
    watch: Arc<Watch>,
    idle: Option<Duration>,
) -> std::io::Result<()> {
    let port = listener.local_addr()?.port();
    let token = Arc::new(Zeroizing::new(token));
    if let Some(idle) = idle {
        let (state, watch) = (state.clone(), watch.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(250));
            if watch.idle_for() >= idle {
                let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
                // Dropping wipes it (Zeroizing).
                st.secret = Secret::Key(Zeroizing::new([0u8; 32]));
                println!(
                    "stopping: no request for {}s (--idle-timeout)",
                    idle.as_secs()
                );
                let _ = std::io::Write::flush(&mut std::io::stdout());
                std::process::exit(0);
            }
        });
    }
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let (token, state, watch) = (token.clone(), state.clone(), watch.clone());
        std::thread::spawn(move || {
            let _ = handle(stream, &token, port, &state, &watch);
        });
    }
    Ok(())
}

struct Request {
    method: String,
    path: String,
    query: BTreeMap<String, String>,
    host: Option<String>,
    /// How many `Host` headers the request carried (only one is served).
    hosts: usize,
}

fn percent_decode(s: &str) -> String {
    fn hex(c: u8) -> Option<u8> {
        (c as char).to_digit(16).map(|d| d as u8)
    }
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse(head: &str) -> Option<Request> {
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    first.next()?.strip_prefix("HTTP/1.")?;
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));
    let mut query = BTreeMap::new();
    for pair in qs.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        query.insert(percent_decode(k), percent_decode(v));
    }
    let mut host = None;
    let mut hosts = 0;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("host") {
                hosts += 1;
                host = Some(v.trim().to_ascii_lowercase());
            }
        }
    }
    Some(Request {
        method,
        path: path.to_string(),
        query,
        host,
        hosts,
    })
}

/// Equal-time comparison, so the token can't be guessed byte by byte.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn respond(
    s: &mut TcpStream,
    status: &str,
    ctype: &str,
    body: &[u8],
    extra: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Referrer-Policy: no-referrer\r\nX-Frame-Options: DENY\r\n{extra}Connection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes())?;
    s.write_all(body)?;
    s.flush()
}

fn respond_json(s: &mut TcpStream, status: &str, v: &Value) -> std::io::Result<()> {
    respond(
        s,
        status,
        "application/json; charset=utf-8",
        v.to_string().as_bytes(),
        "",
    )
}

fn handle(
    mut s: TcpStream,
    token: &str,
    port: u16,
    state: &Mutex<State>,
    watch: &Watch,
) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    s.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > MAX_HEAD {
            return respond(
                &mut s,
                "431 Request Header Fields Too Large",
                "text/plain",
                b"too large",
                "",
            );
        }
    }
    let end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(buf.len());
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let Some(req) = parse(&head) else {
        return respond(&mut s, "400 Bad Request", "text/plain", b"bad request", "");
    };
    // DNS rebinding: a page on another site that resolves its name to
    // 127.0.0.1 still sends its own name as the Host.
    // Exactly one Host: with several, which one counts would be up to
    // whoever reads them.
    if req.hosts != 1 {
        return respond(
            &mut s,
            "400 Bad Request",
            "text/plain",
            b"exactly one Host header",
            "",
        );
    }
    let ok_host = req
        .host
        .as_deref()
        .is_some_and(|h| h == format!("127.0.0.1:{port}") || h == format!("localhost:{port}"));
    if !ok_host {
        return respond(&mut s, "403 Forbidden", "text/plain", b"wrong Host", "");
    }
    if req.method != "GET" {
        return respond(
            &mut s,
            "405 Method Not Allowed",
            "text/plain",
            b"only GET is served",
            "Allow: GET\r\n",
        );
    }
    // The page holds no data, so it's served without the token: after it
    // moves the token out of the address bar, a refresh still loads. Every
    // API route below needs the token.
    if req.path == "/" {
        // Only the browser that has the token counts as having loaded it.
        if req.query.get("t").is_some_and(|t| same(t, token)) {
            watch.page_served.store(true, Ordering::Relaxed);
            watch.touch();
        }
        return respond(
            &mut s,
            "200 OK",
            "text/html; charset=utf-8",
            PAGE.as_bytes(),
            "Content-Security-Policy: default-src 'none'; script-src 'unsafe-inline'; \
             style-src 'unsafe-inline'; connect-src 'self'; base-uri 'none'; \
             form-action 'none'; frame-ancestors 'none'\r\n",
        );
    }
    if !req.query.get("t").is_some_and(|t| same(t, token)) {
        return respond(
            &mut s,
            "403 Forbidden",
            "text/plain",
            b"missing or wrong token",
            "",
        );
    }
    watch.touch();
    // RB-003.7: build the response under the lock, then release it before
    // writing to the client, so a slow reader doesn't hold up other
    // requests.
    let (status, body) = {
        let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
        if req.path == "/api/reload" {
            match load(&st.path, &st.secret) {
                Ok(l) => {
                    st.loaded = l;
                    ("200 OK", about(&st.loaded))
                }
                // The previous load stays, so the views keep working.
                Err(e) => ("409 Conflict", json!({ "error": e })),
            }
        } else {
            match route(&st.loaded, &req) {
                Some(v) => ("200 OK", v),
                None => ("404 Not Found", json!({ "error": "not found" })),
            }
        }
    };
    respond_json(&mut s, status, &body)
}

fn about(l: &Loaded) -> Value {
    let mut a = l.about.clone();
    a["loaded_at"] = json!(l.loaded_at);
    a
}

fn route(l: &Loaded, req: &Request) -> Option<Value> {
    let q = |k: &str| {
        req.query
            .get(k)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    };
    match req.path.as_str() {
        "/api/about" => Some(about(l)),
        "/api/memories" => {
            let order: Vec<(usize, Option<f32>)> = match q("q") {
                Some(text) => l
                    .contents
                    .search(text, l.memories.len())
                    .into_iter()
                    .map(|(i, s)| (i, Some(s)))
                    .collect(),
                None => (0..l.memories.len()).map(|i| (i, None)).collect(),
            };
            let types: Option<Vec<&str>> = q("type").map(|t| t.split(',').collect());
            let keep = |m: &Value| {
                types
                    .as_ref()
                    .map_or(true, |ts| ts.contains(&m["type"].as_str().unwrap_or("")))
                    && q("agent").map_or(true, |a| m["agent"] == a)
                    && q("thread").map_or(true, |t| m["thread"] == t)
                    && match q("keyed") {
                        Some("yes") => m.get("key").is_some(),
                        Some("no") => m.get("key").is_none(),
                        _ => true,
                    }
            };
            // RB-003.7: count the matches, then clone only the page.
            let matches: Vec<(usize, Option<f32>)> = order
                .into_iter()
                .filter(|(i, _)| keep(&l.memories[*i]))
                .collect();
            let limit: usize = q("limit")
                .and_then(|v| v.parse().ok())
                .unwrap_or(200)
                .min(MAX_PAGE);
            let offset: usize = q("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
            let total = matches.len();
            let page: Vec<Value> = matches
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|(i, score)| {
                    let mut m = l.memories[i].clone();
                    if let Some(s) = score {
                        m["score"] = json!(s);
                    }
                    m
                })
                .collect();
            let agents = &l.agents;
            Some(json!({ "total": total, "offset": offset, "memories": page, "agents": agents }))
        }
        "/api/keys" => Some(Value::Array(
            l.keys
                .iter()
                .map(|((key, owner), vs)| {
                    json!({
                        "key": key,
                        "owner": owner,
                        "versions": vs.len(),
                        "current": vs.last().map(|v| v.content.as_str()),
                        "updated_at": vs.last().map(|v| v.created_at),
                    })
                })
                .collect(),
        )),
        "/api/history" => {
            let key = q("key")?;
            let owner = req.query.get("owner").cloned().unwrap_or_default();
            let vs = l.keys.get(&(key.to_string(), owner.clone()))?;
            Some(json!({
                "key": key,
                "owner": owner,
                "versions": vs.iter().map(|v| json!({
                    "id": v.id,
                    "content": v.content,
                    "created_at": v.created_at,
                    "invalid_at": v.invalid_at,
                    "current": v.invalid_at.is_none(),
                })).collect::<Vec<_>>(),
            }))
        }
        "/api/threads" => Some(json!({
            "project_essence": l.project_essence,
            "threads": l.threads.iter().map(|t| json!({
                "id": t.id,
                "turns": t.turns.len(),
                "essence_items": t.essence_items,
            })).collect::<Vec<_>>(),
        })),
        "/api/thread" => {
            let id = q("id")?;
            let t = l.threads.iter().find(|t| t.id == id)?;
            let mut v = json!({
                "id": t.id,
                "essence": t.essence,
                "essence_items": t.essence_items,
                "turn_count": t.turns.len(),
            });
            // The archive only on demand: it can be long.
            if q("turns") == Some("1") {
                v["turns"] = Value::Array(t.turns.clone());
            }
            Some(v)
        }
        "/api/snapshots" => Some(l.snapshots.clone()),
        _ => None,
    }
}

/// Write a page that redirects to `url` into a new private directory
/// (0700, the file 0600, on Unix), so the URL never goes on a command line.
/// Returns the directory and the page's path.
pub fn write_redirect(url: &str) -> std::io::Result<(std::path::PathBuf, std::path::PathBuf)> {
    let tag: String = Mnemo::generate_key()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let dir = std::env::temp_dir().join(format!("mnemo-inspect-{}-{tag}", std::process::id()));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    let page = dir.join("open.html");
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(&page)?;
    // The URL is ours: digits, hex and fixed characters, nothing to escape.
    write!(
        f,
        "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
         <meta http-equiv=\"refresh\" content=\"0;url={url}\"><title>mnemo inspect</title>\
         <a href=\"{url}\">Open the inspector</a>"
    )?;
    f.sync_all()?;
    Ok((dir, page))
}

/// Open the inspector in the default browser. The launcher gets the path of
/// a private redirect page, not the URL: a launcher's arguments are visible
/// to other users of the machine. The page is deleted once the browser has
/// loaded the inspector, or after a minute.
pub fn open_browser(url: &str, watch: Arc<Watch>) -> std::io::Result<()> {
    let (dir, page) = write_redirect(url)?;
    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");
    let launched = cmd
        .arg(&page)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ());
    std::thread::spawn(move || {
        let start = Instant::now();
        while !watch.page_served.load(Ordering::Relaxed)
            && start.elapsed() < Duration::from_secs(60)
        {
            std::thread::sleep(Duration::from_millis(200));
        }
        let _ = std::fs::remove_dir_all(&dir);
    });
    launched
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_parse_and_decode() {
        let r =
            parse("GET /api/memories?q=oat+milk&key=a%3Ab&t=x HTTP/1.1\r\nHost: 127.0.0.1:9\r\n")
                .unwrap();
        assert_eq!(
            (r.method.as_str(), r.path.as_str()),
            ("GET", "/api/memories")
        );
        assert_eq!(r.query["q"], "oat milk");
        assert_eq!(r.query["key"], "a:b");
        assert_eq!(r.host.as_deref(), Some("127.0.0.1:9"));
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert!(parse("garbage").is_none());
    }

    #[test]
    fn tokens_compare_in_full() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd") && !same("abc", "ab"));
        assert_eq!(new_token().len(), 64);
        assert_ne!(new_token(), new_token());
    }

    #[test]
    fn chains_follow_the_links_not_the_ids() {
        let v = |id: &str, next: Option<&str>, at: i64| Version {
            id: id.into(),
            content: id.into(),
            created_at: at,
            invalid_at: next.map(|_| at),
            superseded_by: next.map(str::to_string),
        };
        // Same second, ids in the wrong order: z -> a -> m.
        let order: Vec<String> = chain_order(vec![
            v("m", None, 5),
            v("a", Some("m"), 5),
            v("z", Some("a"), 5),
        ])
        .into_iter()
        .map(|x| x.id)
        .collect();
        assert_eq!(order, ["z", "a", "m"]);
    }

    #[test]
    fn the_redirect_page_is_private_and_points_at_the_url() {
        let url = "http://127.0.0.1:9/?t=abc123";
        let (dir, page) = write_redirect(url).unwrap();
        let body = std::fs::read_to_string(&page).unwrap();
        assert!(
            body.contains("content=\"0;url=http://127.0.0.1:9/?t=abc123\""),
            "{body}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode =
                |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&dir), 0o700);
            assert_eq!(mode(&page), 0o600);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn idle_timeouts_parse() {
        assert_eq!(parse_idle("30m").unwrap(), Some(Duration::from_secs(1800)));
        assert_eq!(parse_idle("90s").unwrap(), Some(Duration::from_secs(90)));
        assert_eq!(parse_idle("1h").unwrap(), Some(Duration::from_secs(3600)));
        assert_eq!(parse_idle("45").unwrap(), Some(Duration::from_secs(2700)));
        assert_eq!(parse_idle("0").unwrap(), None);
        assert!(parse_idle("soon").is_err() && parse_idle("5d").is_err());
    }

    #[test]
    fn it_binds_loopback_only() {
        let l = bind(None).unwrap();
        assert!(l.local_addr().unwrap().ip().is_loopback());
    }
}
