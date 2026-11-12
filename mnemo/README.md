This directory is the **Memory Nemo** Rust crate: encrypted single-file storage, the `mnemo` CLI, examples, and tests.

The repository overview and links to the landing page and Python bindings are in the [root README](../README.md).

## Quick start

```sh
cargo run --example quickstart
cargo run --bin mnemo -- demo
```

## For AI agents

The CLI is designed so an agent can become productive against a `.mnemo`
file in two commands, with no external documentation:

```sh
# Handed an existing file? The file introduces itself.
mnemo about path/to/agent.mnemo

# Starting fresh? `init` auto-inserts a scaffold manifest so the new file
# is self-describing from creation. Replace the scaffold once you know your
# embedder, agent_id convention, and project metadata.
mnemo init my-agent.mnemo --dimensions 768
mnemo about my-agent.mnemo
```

Working on this codebase (not just using mnemo)? Start at
[../AGENTS.md](../AGENTS.md) — repo layout, build/test commands,
conventions, and the dogfood workflow.

### Serve as an MCP server

`mnemo` can expose a `.mnemo` file as a Model Context Protocol server over
stdio, so any MCP-compatible agent (Claude Desktop, Cursor, custom MCP
clients) can drive it as a tool:

```sh
mnemo serve --mcp path/to/agent.mnemo
```

The passphrase is read from `MNEMO_PASSPHRASE` (never a CLI flag — that
would leak into shell history and process listings). Tools exposed:
`about`, `remember`, `recall`, `forget`, `list`, `snapshot_list`, `stats`,
`cache_put`, `cache_get`, `cache_stats`, `cache_put_semantic`,
`cache_get_semantic`. Every mutation flushes before returning.

#### Text-in via `--embed-cmd` (Phase 3)

`remember`, `recall`, `cache_put_semantic`, and `cache_get_semantic`
accept **either** a caller-supplied `vector`/`query` **or** a `text`
string that the server embeds for them. Enable the text path by passing
an embedder command at serve start:

```sh
MNEMO_PASSPHRASE=... mnemo serve --mcp agent.mnemo \
  --embed-cmd "python3 mnemo/examples/embedder_server.py" \
  --embed-name "sentence-transformers/all-MiniLM-L6-v2"
```

The embedder is a **persistent child process** spawned once at serve
start. Protocol: it reads one JSON-encoded string per line from stdin
and writes one JSON array of floats per line to stdout, flushed after
every reply. See `mnemo/examples/embedder_server.py` for a 10-line
reference implementation (sentence-transformers + MiniLM, 384-dim).

> **Stdout is for replies only.** Any line the embedder prints to
> stdout outside a reply — startup banners, model-load progress,
> warnings, "model loaded" messages — is a protocol violation and
> would shift every subsequent reply by one line. EmbedCmd detects
> it, kills the child, and surfaces a clear error ("stdout is
> reserved for replies only; log to stderr instead"); the next call
> respawns the child. **Log to stderr.** Python: `print("...",
> file=sys.stderr, flush=True)`, or configure `logging` with
> `stream=sys.stderr`. Stderr is inherited to the parent and is
> safe for anything you want to see while the server runs.

Per-request timeout defaults to 60 seconds; override with
`--embed-timeout <secs>` on `serve`, `remember`, and `recall`. Set
it longer than your slowest cold-start reply (sentence-transformers'
first `encode()` on CPU can be 3–5s). On timeout — or any other
protocol error (malformed JSON, wrong dimensionality, zero vector,
unexpected EOF) — the child is killed and the next call respawns a
fresh child with the same argv/env/timeout. The guarantee is
asymmetric and deliberate: **an error is allowed; a mismatched
vector never is.**

The `--embed-name` string is the embedder's identity, with the width and
the `normalize` flag. Every file records the identity of the first embedder
attached to it, with or without a manifest (EMBED-ID-1). Starting a server
with a different `--embed-name` on an existing file fails loudly with
`MnemoError::EmbedderMismatch`, because mixing embedders in one file
silently corrupts recall quality. A new model needs a new file.

- The record is a reserved, encrypted entry, not a memory: `len()`,
  `stats()`, recall, `about()` and `get()` never see it, and the cache API
  refuses its `_mnemo.*` namespace. Compaction, `export_rekeyed` and the
  mobile passphrase import carry it.
- A read-only handle checks the identity and never writes one.
- A file with a manifest also keeps it at the manifest's `metadata.embedder
  = { name, dimensions, normalize }`, as before. If the manifest and the
  record disagree, `set_embedder` says so and names both.
- A file from before v0.13 records the first embedder attached after the
  upgrade. The check protects from then on; it can't tell which model made
  the older vectors.

Claude Desktop config snippet (`~/Library/Application Support/Claude/claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "mnemo": {
      "command": "mnemo",
      "args": [
        "serve", "--mcp", "/absolute/path/to/agent.mnemo",
        "--embed-cmd", "python3 /absolute/path/to/embedder_server.py",
        "--embed-name", "sentence-transformers/all-MiniLM-L6-v2"
      ],
      "env": { "MNEMO_PASSPHRASE": "your-passphrase-here" }
    }
  }
}
```

Omit `--embed-cmd`/`--embed-name` to keep the pre-Phase-3
embedder-agnostic behavior (`vector`/`query` only).

#### CLI text path

The same `--embed-cmd` + `--embed-name` pair works on the one-shot
CLI commands:

```sh
mnemo remember agent.mnemo --text "the user prefers tea" \
  --type semantic --importance 0.8 --agent assistant \
  --embed-cmd "python3 embedder_server.py" \
  --embed-name "sentence-transformers/all-MiniLM-L6-v2"

mnemo recall agent.mnemo --text "what drink?" --top-k 5 \
  --embed-cmd "python3 embedder_server.py" \
  --embed-name "sentence-transformers/all-MiniLM-L6-v2" \
  --no-track
```

`--text` is mutually exclusive with `--query`/`--query-file`.

The server holds an exclusive write lock on the file for its lifetime
(see [Concurrency](#concurrency--read-only-opens)), so two MCP servers
on the same file will conflict — one will fail to start with
`Locked`. That's intentional.

### Result caching

The same `.mnemo` file is also a durable exact-key cache (Phase 10.1). One
file, two live surfaces: agent memories and result cache, sharing the same
encrypted-page storage and the same single-fsync WAL commit. Useful for
memoizing LLM tool-call outputs, prompt→completion pairs, expensive HTTP
responses — anything reconstructible on cache miss:

```rust
use mnemo_engine::{Mnemo, CachePutOpts};

let mut db = Mnemo::open("agent.mnemo", "correct horse battery")?;

// Insert (key is SHA-256-hashed by the engine — pass the raw input).
db.cache_put(
    "llm",
    "prompt-42",
    br#"{"model":"gpt-4","cost":0.03}"#,
    CachePutOpts { content_type: "json".into(), ttl_secs: Some(3600), cost_hint_ms: Some(250) },
)?;
db.flush()?;

// Lookup — hit updates access stats catalog-only (no full record rewrite).
if let Some(hit) = db.cache_get("llm", "prompt-42")? {
    println!("cached: {} bytes, {} accesses", hit.value.len(), hit.access_count);
}
```

Namespaces (`"llm"`, `"http"`, `"tool"`, `...`) isolate independent caches
inside one file. Per-namespace [`CacheBudget`] caps entry count and byte
total; overflow evicts LRU. TTL is per-entry.

**Flush policy** (`Mnemo::set_cache_flush_policy`) is per-handle:

- `Strict` (default) — every `cache_put` dirties the cache directory;
  next `flush()` persists it.
- `Batched { max_dirty, max_age }` — cache mutations accumulate; when
  either threshold trips, the engine internally calls `flush()`. **Memory
  writes stay strict** — only cache mutations get the relaxed lane.

Losing unflushed batched entries on a crash is a cache *miss*, never
corruption — commits still ride the same single-fsync WAL transaction.

CLI:

```sh
mnemo cache put   agent.mnemo --ns llm  --key prompt-42 --value '{"...":"..."}' --content-type json --ttl 3600
mnemo cache get   agent.mnemo --ns llm  --key prompt-42
mnemo cache stats agent.mnemo --ns llm         # or omit --ns for the whole cache
mnemo cache purge agent.mnemo --ns llm --expired
mnemo cache delete agent.mnemo --ns llm --key prompt-42
```

**MCP:** `mnemo serve --mcp` exposes `cache_put`, `cache_get`,
`cache_stats`, `cache_put_semantic`, and `cache_get_semantic` as
tools (Phase 10.4) — any MCP-compatible agent can drive the cache
alongside the memory surface. **Python:** the `Mnemo` class carries
matching `cache_*` methods; the ~90-line `@db.cached(...)` decorator
recipe (with OpenAI, Anthropic, and semantic-mode examples) lives
in `mnemo-python/README.md` — copy it into your project.

#### Semantic cache (Phase 10.2)

For LLM workloads where prompts vary slightly but map to the same
completion, the exact-key hash-of-string layer isn't enough — "please
summarize this doc" and "summarize the following doc please" would
miss. Semantic caching bridges the gap by keying on an embedding
vector and returning the closest hit above a similarity threshold:

```rust
use mnemo_engine::{Mnemo, SemanticCachePutOpts, DEFAULT_SEMANTIC_THRESHOLD};

let mut db = Mnemo::open("agent.mnemo", "pw")?;

// Store: bring your own embedding vector + the model that produced it.
db.cache_put_semantic(
    "llm",
    "prompt-abc",
    embed("please summarize this doc"),          // Vec<f32>, caller-supplied
    b"...LLM response bytes...",
    SemanticCachePutOpts::new("bge-large-en-v1.5"),
)?;
db.flush()?;

// Look up: top-1 cosine over the namespace's vectored entries whose
// model matches. Returns Some((value, similarity)) iff sim >= τ.
if let Some((hit, sim)) = db.cache_get_semantic(
    "llm",
    &embed("summarize the following doc please"),
    DEFAULT_SEMANTIC_THRESHOLD,                  // 0.97
    "bge-large-en-v1.5",
)? {
    println!("cache hit at similarity {sim:.3}: {} bytes", hit.value.len());
}
```

Key contracts:

- **Exact-key and semantic entries coexist** in the same namespace.
  `cache_get` (exact-key) and `cache_get_semantic` (vector) each
  ignore the other kind, so you can freely mix.
- **Model is matched exactly.** A hit under `"text-embedding-3-small"`
  is invisible to a query under `"bge-large-en-v1.5"`. Different
  embedders produce vectors in incompatible spaces — a cross-model
  hit would be a semantics bug.
- **Vector dimensionality is enforced** against the database's
  configured `dimensions` (same as `remember`) — a mismatch returns
  `MnemoError::DimensionMismatch`.
- **`DEFAULT_SEMANTIC_THRESHOLD = 0.97`** is a conservative default.
  Model-dependent — with a lossier embedder consider raising it;
  with dense models on well-separated tasks you can drop it.

Text-in/text-out (`cache_get_semantic_text(ns, "prompt", &embedder, τ)`)
lands with Phase 3's embedder integration. Until then callers own the
embedding step, same as `remember` / `recall`.

---
# Memory Nemo (MNemo)

**An encrypted, single-file, portable agent-memory engine, written in Rust.**

A whole memory store — vectors, content, structured metadata, and the
multi-signal recall machinery an agent needs — lives in **one file** you can
copy, back up, or hand to another process. The file is encrypted at rest, so
the memory of an agent is as portable and as private as a SQLite database.

This repository implements a real, compiling, tested **core**. See
[Scope](#scope-what-is-and-isnt-here) for what is deliberately left as roadmap.

> The product is **Memory Nemo (MNemo)**. The crate and Python package are
> distributed as `mnemo-engine` (both `mnemo` and `mnemo-db` were already
> taken by unrelated projects — see the "Breaking: import name" section in
> CHANGELOG.md for the full rationale). **Starting in v0.7.0, the Rust
> library name and Python import name are `mnemo_engine`:**
> `use mnemo_engine::...` and `import mnemo_engine` (or `import
> mnemo_engine as mnemo` if you want call sites unchanged). Rust code
> that wants to keep writing `use mnemo::...` can rename the dependency
> locally via `mnemo = { package = "mnemo-engine", version = "0.7" }` —
> that stays supported indefinitely because it's a Cargo-side alias.
> Python has no `import mnemo` shim: the old top-level `mnemo` module
> would still clash with `mnemo-db` and PyPI `mnemo`, which is exactly
> what the rename exists to end. The CLI command (`mnemo`) and the
> `.mnemo` file extension are unchanged.

---

## Why

Agent frameworks usually bolt memory onto an external vector service: a
network hop, a second thing to deploy, a second place secrets can leak.
MNemo takes the SQLite position instead — memory is a *file*:

- **Single file.** No server, no daemon, no schema migration dance.
- **Encrypted at rest.** Passphrase in, ciphertext on disk.
- **Portable.** `cp agent.mnemo backup.mnemo` is a complete, consistent backup.
- **Agent-native.** First-class notions of memory *type*, *importance*,
  *recency*, *access frequency*, TTL, and per-agent scoping — recall ranks on
  all of them, not similarity alone.

## Architecture

```
passphrase ──Argon2id──▶ KEK ──AES-256-GCM──▶ wraps DEK (random 256-bit)
                                                   │
                                                   ▼
                              every 8 KiB page ──AES-256-GCM──▶ ciphertext
```

Two-tier key hierarchy: a passphrase derives a key-encryption key (KEK) via
Argon2id; the KEK wraps a random data-encryption key (DEK); the DEK encrypts
every data page with AES-256-GCM. Changing the passphrase (`rekey`) only
re-wraps the DEK — the bulk pages are never rewritten.

That makes `rekey` cheap, and it means **a plain `rekey` changes the
secret that opens this file from now on; anyone who had the file and the
old secret can still read it and later writes.** The DEK is the same, so an
earlier copy of the file, or of just its header, still unlocks it with the
old passphrase. **Use `--rotate` to revoke** (after a suspected leak, say):
`mnemo rekey --rotate`, or `rekey` followed by `Mnemo::compact_file`,
rewrites the file under a fresh DEK. `Mnemo::export_rekeyed` writes a copy
under a fresh DEK and a new secret, sharing no key material with the
source. Rotation can't erase old ciphertext the storage hardware still holds: flash keeps rewritten blocks until it reuses them, and that's out of the engine's hands.

**A `rekey` that fails leaves the old secret in force** (REKEY-1, v0.13.2),
for the handle and for the file, including after later writes and a
close. It takes effect at one point, the write of the new page 0; a
failure before that puts the old header back. The one exception is a
failure to write the new page 0 followed by a failure to put the old one
back; the error then says the file may open with either secret. Once the
new page 0 is on disk, scrubbing the old key wrap from the file is
hygiene: if part of it fails, `rekey` still succeeds, and
`Mnemo::rekey_cleanup_error` (a warning in the CLI, a `RuntimeWarning` in
Python) says what was left. `--rotate` covers it.

The file is a sequence of fixed **8 KiB pages**:

- **Page 0** — the header (plaintext): magic, version, KDF parameters, salt,
  the wrapped DEK, pointers to the current catalog and ANN index runs, the
  location of the WAL region, and a CRC-32 over the header itself.
- **Pages 1..W** — the write-ahead log region (see below).
- **Pages W+** — encrypted runs. Each record (a memory, MessagePack-encoded),
  each catalog snapshot, the optional ANN index, and the snapshot manifest
  occupy runs of consecutive pages. The page allocator is append-only.

Every page is sealed with a unique nonce derived from `page_number` and a
monotonic `write_counter`, so a nonce is never reused under the DEK. From
**format v6** onwards, each page's home `page_no` is also bound into the
GCM authentication tag as Associated Authenticated Data (AAD) — an attacker
who swaps two valid encrypted pages between slots makes them un-decryptable
at the new addresses (the auth check fails with `PageAuthFailed`). The v5→v6
migration re-encrypts every live data page in place under the new AAD.

**Format v7** adds an AES-GCM seal at the tail of the header page that
authenticates every mutable header field (write counter, next page,
catalog/index/manifest pointers, version) under the DEK. The CRC at
byte 238 stays as a pre-passphrase torn-write check; the seal is the
keyed integrity layer that catches an attacker rewriting, say,
`catalog_start` to point at an older catalog run. Open after a tampered
flush errors with `HeaderTampered` instead of silently loading the
stale state. The seal does not prevent rollback to a previous *valid*
sealed state (replay of an old header byte-block); detecting that
would need monotonic counters tracked outside the file.

### Durability — write-ahead log

`flush()` is one **atomic transaction**, committed through a write-ahead log:

1. Record (vector) data pages are written copy-on-write to fresh pages and
   fsynced. Being unreferenced until the catalog below commits, they are
   crash-safe by construction.
2. The transaction's control plane — the new catalog, the ANN index, and the
   new header — is written into the WAL region as a run of
   `(txn_id, page_no, page_image)` frames followed by a checksummed `COMMIT`
   frame.
3. **One fsync of the WAL is the commit point.** Before it the transaction
   does not exist; after it the transaction is durable even though no home
   page has changed.
4. A *checkpoint* copies each frame to its home page and rewrites page 0.

A crash *before* the commit fsync leaves the previous state untouched. A crash
*after* it is repaired on open: [`recover`](src/wal.rs) replays the committed
transaction; a torn, never-committed tail is discarded. Because page 0 carries
a CRC, even a torn header write is detected and healed from the WAL. The WAL
region grows automatically when a transaction's control plane outgrows it.
(`tests/integration.rs` exercises crash recovery, the torn-header heal, a
discarded uncommitted log, and WAL growth.)

**Torn-header recovery (DUR-1).** Every post-`create` page-0 change —
WAL relocation when the control plane outgrows the current region, and
`rekey` when the passphrase moves — now goes through the WAL first:
commit a one-frame `(0, new_header)` transaction into the new region,
checkpoint page 0, then invalidate the predecessor region's first
frame header. Combined with a streaming file-wide scan on open, this
means a torn page-0 write at *any* WAL location is recoverable: open
walks every 8 KiB-aligned `MWAL` candidate in the file, picks the
highest-`txn_id` committed header frame whose `wal_start` + `wal_seq`
agree with the frame's region, whose DEK unwraps under the
passphrase, and whose AES-GCM seal validates, then replays it. Only
the initial `Mnemo::create` writes page 0 unlogged. **A header that
fails its seal is reported as `HeaderTampered` and never auto-healed**
— the entire 294-byte header lies inside one 512-byte sector that
storage writes atomically, so a surviving CRC with a failing seal
cannot be a torn write, and silently loading a stale authenticated
header from the WAL would erase the tamper-evidence the seal exists
to give.

What this buys over a plain copy-on-write header swap: a single-fsync commit
and an explicit, replayable transaction boundary.

### Concurrency & read-only opens

Every read-write open (`Mnemo::create`, `Mnemo::open`) takes an **exclusive
OS advisory lock** on the file for the lifetime of the handle (`flock(2)` on
Unix, `LockFileEx` on Windows). A second concurrent open — from any
process — returns `MnemoError::Locked` instead of silently interleaving WAL
frames and corrupting the file. The lock releases when the handle drops.

`Mnemo::open_read_only(path, passphrase)` opens the file with a **shared**
lock. Multiple read-only handles coexist — this is what the shared-lock
tier buys you. A read-only handle does **not** coexist with a writer
holding the exclusive lock: `flock(2)` on Unix and `LockFileEx` on Windows
both refuse a shared-lock request while an exclusive lock is held. So
"long-lived reader alongside a live writer" needs the writer to release
first (drop the write handle, then reopen read-only). Every mutating
method on a read-only handle returns `MnemoError::ReadOnly`; `recall`
with the default `track_access(true)` counts as a mutation (it bumps
catalog access stats) and requires `track_access(false)` on a read-only
handle. The matching CLI knob is `mnemo recall --no-track`.

Two conditions block a read-only open and return `MnemoError::NeedsWriteOpen`:
a committed-but-uncheckpointed WAL transaction waiting to be replayed on
open, and an on-disk format version older than the current build. Both cases
require write access to fix. The remedy is the same either way: open
read-write once (which recovers / migrates in place), close cleanly, and
reopen read-only. We chose this fail-loud policy over an in-memory WAL
overlay so read-only reads can never return stale pre-recovery state.

### Snapshots & point-in-time recovery

Because record, catalog, and index pages are only ever *appended*, every past
flush's pages are still on disk. MNemo exploits this: each `flush` appends a
small entry to a **snapshot manifest** recording where that transaction's
catalog and index runs live. The manifest turns the append-only file into a
navigable history.

- `snapshots()` lists every committed transaction — its id, timestamp, and
  memory count.
- `restore_to(txn_id)` reinstates the database exactly as that transaction
  left it; `restore_to_time(unix_secs)` picks the latest snapshot at or before
  an instant.

A restore is itself an ordinary committed transaction (and a new snapshot), so
it is crash-safe *and* reversible — having rewound, you can roll forward again.

**How far back you can restore is capped.** Only retained snapshots can be
restored, and the cap differs by format:

| Format | Retained by default | Override |
|---|---|---|
| v9 (new files) | the newest **16** snapshots, plus any from the **last 24 hours**; retained snapshots never pin more than a quarter of the file (past that, the oldest go first, whatever their age) | `max_snapshots` / `set_max_snapshots` changes the count |
| v8 (read-only since v0.13) | the newest **256** snapshots | — |

So on a v9 file that flushes often (a phone app under the default
coalesced flush policy commits about once a second while it's writing),
"restore to an hour ago" works only within the last 24 hours, and "restore
to last week" doesn't work unless you raise the count. `restore_to` a
snapshot that's gone returns `MnemoError::NotFound`. `compact_file`
collapses the manifest to a single snapshot, and there's no external log
archiving, so nothing older than the last compaction is ever restorable.
`snapshots()` lists exactly what's restorable now, and since v0.13
`stats()` and `stats_summary()` sum it up: `snapshots_retained`,
`oldest_restorable_at` (the furthest back a restore reaches) and
`snapshot_pinned_bytes` (what the retained snapshots keep from reuse; `0`
on v8). Under churn the pin cap can leave only a few snapshots, seconds
apart, so show these rather than the policy. `mnemo info` prints them.

### Format versions: v9, and v8 read-only

New files are **v9**, the only format this build writes. Since v0.13 (the v8
sunset):

- **A v8 file opens read-only.** `open_read_only` (and every read command:
  `info`, `about`, `get`, `list`, `search`, `recall`, `verify`, `history`,
  `thread export`, `essence`) works as before.
- **A read-write open of it fails** with `MnemoError::FormatTooOld { version:
  8 }`, whose message says to run `mnemo compact`.
- **`mnemo compact` (`Mnemo::compact_file`) converts it to v9** and keeps
  every live memory, the result cache, the threads and the essence. It
  collapses the snapshot history to one snapshot, as any compaction does.
- **A v4–v7 file** (from v0.6 and earlier) no longer opens at all, read-only
  included: `compact` converts it the same way.
- **No new v8 file can be made.** `format_version: 8` is refused, and
  `compact --format v8` is gone.

The mobile packages never open a v8 file read-write either. Convert an old
file on a Mac with `mnemo compact` before copying it to a phone.

### The vector column (v9)

On a v9 file, recall scans an fp16 copy of the vectors (`dims × 2 + 4`
bytes a fact). It stays in memory while the file's **live facts** fit
`set_vector_cache_budget` (96 MiB by default, 16 MiB in the mobile
packages). Thread turns, essence items, tombstones and superseded
versions don't count. Past the budget, recall reads the vectors through
the page cache instead: at 10,000 facts × 384 dimensions, warm recall goes
from about 2 ms to about 11 ms (M1 Pro). `stats().recall_mode` says which
path recall takes now: `ExactResident`, `ExactPaged` (over the budget, or
a v8 file) or `Indexed` (an IVF+PQ index, which v9 builds from 20,000
facts).

### Page cache

Decrypted page payloads are held in a **bounded LRU cache** so repeated reads
skip a decrypt. Eviction targets the least-recently-used *clean* page; a dirty
page — the only copy of an un-flushed write — is never evicted, so the cap
bounds retained clean pages without ever risking data. The default cap is
1024 pages (~8 MiB; 8192 pages before v0.10.0); `Mnemo::set_cache_capacity` tunes it, and
`Mnemo::cache_stats` reports occupancy. The LRU order is an intrusive linked
list over a slab of indices, so it stays within the crate's
`#![forbid(unsafe_code)]`.

### Recall

`recall` scores each candidate with the v0.7.0 scoring-v2 formula:

```
score = similarity
      + max(similarity, 0)
        · ( β · recency
          + γ · (importance − 0.5)
          + δ · tanh(access_count / 5) )

recency = exp( -ln(2) · age_secs / half_life_secs )
age_secs = now - memory.created_at      # not accessed_at
```

Four deliberate shape changes from the pre-v0.7.0 additive formula
(`α·sim + β·recency + γ·importance + δ·ln(1 + uses)`):

- **Similarity-gated bonuses.** Recency / importance / usage are
  multiplied by `max(similarity, 0)`, so a stale-but-off-topic memory
  can't ride recency past a semantically-on-point one.
- **Importance centered at 0.5.** A default-importance memory
  contributes zero importance bonus; importance only pushes a score
  up or down from that reference point.
- **Saturating usage.** `tanh(access_count / 5)` caps the feedback
  bonus so a heavily-returned memory stops accumulating weight past
  ~5 uses.
- **Recency from `created_at`.** A `recall()` no longer bumps its own
  results' recency on later queries.

`recency` still decays exponentially (default 7-day half-life).
`recall` also filters by memory type and by agent scope (an agent
sees its own memories plus any marked `Shared`), and skips
TTL-expired entries. Weights and the similarity metric (cosine /
dot / L2) are per-request. The raw `RecallResult.score` sits on
a different scale than pre-v0.7.0 — callers comparing raw scores
to a fixed threshold should re-pick the threshold.

### Sessions

A `Session` wraps the database for the span of one conversation. `db.session(agent)`
opens it with a fresh session id; `add_turn` records each conversation turn as
a `Working` memory tagged with that session and agent; `recall` retrieves
context scoped to the agent. Closing the session **consolidates** its turns —
`close()` promotes them from working memory to durable `Episodic` memory ("what
happened"), while `discard()` throws them away. The session borrows the
database mutably for its lifetime, so the single-writer rule is enforced by the
compiler. See `examples/session.rs`.

### Approximate index (IVF + PQ)

Exact scan is `O(n)` — fine for thousands of memories. Past that, build an
**IVF + PQ** index and `recall` becomes sub-linear:

- **IVF** (inverted file): k-means groups vectors into ≈`√n` partitions; a
  query only scans the `n_probe` partitions nearest its centroid.
- **PQ** (product quantization): each vector is split into `m` subspaces,
  each quantized to one of ≤256 learned codewords — a vector becomes `m`
  bytes. Candidate distances come from precomputed per-subspace lookup
  tables, so the scan never touches a full float vector.
- **Rerank**: the closest `n_rerank` candidates are loaded at full precision
  and ranked exactly with the caller's metric.

```rust
db.build_index()?;                 // or build_index_with(IndexConfig { .. })
db.flush()?;                       // the index is persisted in the file

// recall now runs IVF → PQ → rerank automatically; tune per query:
let req = RecallRequest::new(query).top_k(10).n_probe(16).n_rerank(128);
```

`n_probe` and `n_rerank` are the accuracy/speed dials — higher is more
accurate, lower is faster. The index is stored *inside* the encrypted file
(its own page run), maintained incrementally on insert, and rebuilt fresh by
`compact`. `build_index` re-clusters; `drop_index` reverts to exact scans.
Internally the index ranks by squared-L2; the exact rerank uses the
requested metric, so for cosine queries a generous `n_rerank` is advisable.

**Default auto-formulas (PR ANN-1 + ANN-1.1, pending v0.6.0).**
`IndexConfig::default()` now picks `pq_subspaces = dims/4` at build time
(was `dims/8`), plus query-time formulas:

- `n_probe = max(ceil(partitions / 2), min(partitions, 100))` (was fixed `8`)
  — full probe up to ~10K-vector files (≤100 partitions), half probe above
- `n_rerank = (n_vectors / 200).clamp(100, 1000)` (ANN-1 shipped a fixed
  500; ANN-1.1 made it size-aware — see
  `index::effective_auto_n_rerank`). Hits 100 at 10K, 500 at 100K, caps
  at 1000.

Measured on the Phase 6 synthetic corpus: 10K hits recall@10 ≈ 0.98 at
~1.4 ms/query; 100K hits ~0.98 at ~10.6 ms/query. The old pre-ANN-1
`n_probe=8, n_rerank=64` defaults capped 100K recall@10 at ~0.40.

**Upgrade path for existing files.** The `n_probe` / `n_rerank` fix
takes effect at query time (ANN-1.1), so a v0.5.x-built index gets the
new defaults immediately on first open under v0.6.0+ — no rebuild
needed. The `pq_subspaces` fix is build-time (baked into the PQ
codebook), so you'll stay on the old 48-subspace codebook until you run
`mnemo index --rebuild` (CLI) or `db.rebuild_index()` (Rust / Python).
With `pq_subspaces=48` + new query-time formulas, 100K tops out at
~0.92; rebuilding to 96 subspaces lifts it to ~0.98. See
`test/scripts/perf_v0.5.0.json` keys `ann_recall_grid_v2` (grid) and
`ann_defaults_v0_6_0` (measurements at the shipped defaults).

Any explicit `n_probe` / `n_rerank` passed per call still overrides
everything above.

`Mnemo::search` always stays exact — it is the brute-force ground truth.

## Security model

MNemo's threat model is **data at rest plus tamper-evidence**. The file on
disk is opaque without the passphrase, and an attacker who modifies bytes
in the file is detected — not allowed to silently substitute or roll back
to a previous valid state.

**What's protected:**

- **Confidentiality of page contents.** Every 8 KiB page (records,
  catalog, ANN index, snapshot manifest, WAL frames) is encrypted under
  the random 256-bit DEK using AES-256-GCM. The header's KDF parameters
  and salt are plaintext — they're the bootstrap; everything past the
  passphrase derivation is ciphertext.
- **Page placement integrity (v6+).** Each page's home `page_no` is bound
  into the GCM authentication tag as AAD. An attacker who transplants a
  valid encrypted page to a different slot makes it un-decryptable at the
  new address — open errors with `MnemoError::PageAuthFailed`.
- **Header field integrity (v7+).** A small AES-GCM seal at the tail of
  the header page authenticates every mutable header field
  (`write_counter`, `next_page`, catalog/index/manifest pointers, version)
  under the DEK. Rewriting any of them invalidates the GCM tag and open
  errors with `MnemoError::HeaderTampered` — not a silent stale-data load.
- **Passphrase secrecy.** Wrong passphrase fails authenticated decryption
  on the wrapped DEK and is reported as `MnemoError::WrongPassphrase` —
  indistinguishable from a tampered wrapped-DEK blob. No oracle.
- **Crash-safety nonce uniqueness.** `flush` leases write-counter and
  page slots in the header *before* any data page hits the disk, so a
  crash mid-flush can't lead a subsequent flush to reuse a `(page_no,
  write_counter)` nonce on different plaintext under the same DEK.

**What's not protected:**

- **Availability.** An attacker with write access can always destroy data
  — truncate the file, zero pages, delete it outright. Tamper-evidence
  means modifications are *detected*, not prevented.
- **Rollback to a previously valid sealed state.** The v7 seal catches a
  point-in-time tamper, but it can't tell that the entire header block
  has been replaced with an older valid snapshot of itself (a "replay"
  attack). Detecting that needs monotonic counters tracked outside the
  file — out of scope.
- **`agent_id` scoping.** `recall(agent_id=...)` is *cooperative*
  filtering, not isolation. Anyone holding the passphrase can read every
  agent's memories; the scoping helps an honest agent stay in its lane,
  not protect one agent's data from another.
- **Side channels.** Encryption is not constant-time at the engine level;
  page-cache behavior, file-size growth, and access patterns can leak
  metadata. Mnemo trusts the OS and the AES-GCM implementation for
  primitive-level side-channel hardness (AES-NI on x86, equivalent on
  ARM).
- **In-memory secrecy.** Decrypted page payloads and the DEK live in
  process memory while the database is open. Use [`Mnemo::close`] (or
  drop the handle) promptly when the database isn't needed; do not
  swap-out without `mlock` if the host's swap policy is a concern.
- **External attackers with the passphrase.** Mnemo authenticates page
  content under the DEK, not the user — anyone with the passphrase IS the
  user as far as the format is concerned.

The v0.2.0 format (v7) closes every "silent stale-data load" path
flagged in the security review; rollback-replay and availability are
acknowledged limitations.

**v0.5.2 (SEC-1): AES-GCM nonces are now per-session random.** Up to
v0.5.1, each page's encryption nonce was `(page_no, write_counter)` —
unique within a single file, but **shared across copies of a file**
under the same DEK (the data key never changes after `create`). If you
had two copies of the same `.mnemo` file and both received writes
(two laptops syncing through Dropbox and the file "conflict-copies"
diverge; a restored older backup resuming alongside the current file;
anything that leads to `cp agent.mnemo backup.mnemo` + writes to both),
the two copies' writers would produce pages with the same encryption
nonce under the same key but different plaintexts — breaking AES-GCM
confidentiality and tamper-evidence. v0.5.2 moves to a random 64-bit
per-session prefix + 32-bit per-session counter (NIST SP 800-38D
§8.2.1), drawn fresh on every writable open. Cross-copy collisions
now require two sessions to independently draw the same 64-bit value
— astronomically unlikely. **If you ran a pre-v0.5.2 build on diverged
copies, rotate your DEK by running `compact_file`** — it rewrites the
file through a fresh `random_dek`, so the prior nonce reuse under the
old key stops mattering for the rewritten pages. No format change;
existing files open on v0.5.2 unchanged.

## Quick start (library)

```rust
use mnemo_engine::{Mnemo, MnemoConfig, Memory, MemoryType, RecallRequest};

fn main() -> mnemo::Result<()> {
    let cfg = MnemoConfig { dimensions: 3, ..Default::default() };
    let mut db = Mnemo::create("agent.mnemo", "correct horse battery", cfg)?;

    db.remember(
        Memory::new("the user prefers dark mode", MemoryType::Semantic, vec![0.1, 0.2, 0.9])
            .with_agent("assistant-1")
            .with_importance(0.8),
    )?;
    db.flush()?; // durable

    for hit in db.recall(&RecallRequest::new(vec![0.1, 0.2, 0.9]).top_k(5))? {
        println!("{:.3}  {}", hit.score, hit.memory.content);
    }
    Ok(())
}
```

Run the bundled example: `cargo run --example quickstart`.

## Command-line tool

```
cargo run --bin mnemo -- <command>
```

| Command   | Purpose                                              |
|-----------|------------------------------------------------------|
| `init`    | Create a new encrypted database (auto-adds scaffold manifest) |
| `info`    | Print statistics (including ANN index shape)         |
| `about`   | Self-describing briefing — print onboarding memories |
| `import`  | Bulk-load memories from a JSON Lines file            |
| `index`   | Build, rebuild, or drop the IVF+PQ index             |
| `list`    | Browse live memories (table / json / jsonl, filters) |
| `export`  | Readable plaintext export (Markdown or JSON Lines, filters); `--git` for diff-clean output |
| `inspect` | Browse the file in a local, read-only web view (memories, keyed history, threads and essence, snapshots, about) |
| `get`     | Fetch one memory by its ULID                         |
| `search`  | Exact nearest-neighbour search (similarity only)     |
| `recall`  | Multi-signal ranked retrieval (sim + recency + …)    |
| `verify`  | Decrypt and re-validate every live record            |
| `rekey`   | Re-encrypt the data key under a new passphrase; `--rotate` also rewrites the file under a fresh data key (revokes the old passphrase) |
| `compact` | Rebuild the file, dropping tombstones and expired; writes v9, which converts a v8 (or older) file. Since v0.13 that's the only way to make a v8 file writable again |
| `snapshots` | List the restorable snapshots (one per flush)      |
| `restore` | Roll the database back to a past snapshot            |
| `demo`    | Self-contained end-to-end demonstration              |

### Self-describing databases

A `.mnemo` file should be able to introduce itself. The single-file philosophy
is that everything an agent needs to use this database lives in the file —
not in a sibling README, not in environment configuration, not in tribal
knowledge. The `about` command surfaces that introduction:

```sh
cargo run --bin mnemo -- about agent.mnemo
# Prints a one-line stats header, then every memory tagged
# metadata.area = "onboarding" sorted with the canonical manifest first,
# then by importance descending, then the project essence (if any),
# ending with a quick-start footer.
```

Since v0.9.1 the briefing also carries the file's **project essence**: the
render of its project-scope essence items (conventions, lasting decisions,
open items), at the default 6 KB budget, after the manifest and onboarding
memories. `Mnemo::about_essence()` returns it (`None` when there is none),
`mnemo about --format json` adds it as `"essence"`, and the MCP `about` tool
appends it as a final `{"kind": "essence", "render": …}` element.

The convention is two metadata keys on ordinary memories — no new schema, no
extra file:

- `metadata.area = "onboarding"` marks a memory as part of the orientation
  briefing. Returned by `Mnemo::about()` (Rust) and `db.about()` (Python).
- `metadata.topic = "manifest"` marks the *one* canonical "I am this file"
  entry. Hoisted to the top of `about` regardless of importance, and the
  only entry returned by `mnemo about --manifest-only`.

The manifest is the headline orientation point — every database should have one.
`mnemo init` auto-inserts a **scaffold manifest** (a placeholder with
`metadata.scaffold = true`) so a new file is self-describing from the moment
it's created; `mnemo about` tags it as `(scaffold — please replace)` until you
overwrite it. Pass `--no-manifest` to `mnemo init` for an entirely empty file.

Replace the scaffold with one that records your project's actual values.
Recommended fields inside `metadata` on the manifest itself:

- `embedder.name`, `embedder.dimensions`, `embedder.normalize` — which
  embedding model produces vectors compatible with this file. A receiving
  agent uses this to pick the right embedder or fail loudly when it can't.
- `agent_id_default` — the agent id convention used when writing here.
- `project.name`, `project.repo` — what project this database serves.
- `conventions.*` — any project-specific metadata schemas (e.g. perf entries
  always carry `version`, `metric`, `value`, `units`, `build`, `corpus`).

`seed.json` in `test/scripts/` shows a working example. With a manifest in
place, an agent receiving a `.mnemo` file plus its passphrase needs no
external docs: `mnemo about <file>` (or `db.about()`) tells them what the
file is and how to use it.

### Exploring a database

The exploration commands compose into a quick read-only workflow:

```sh
# orient — what is this file? (manifest + onboarding briefing)
cargo run --bin mnemo -- about agent.mnemo

# overview — size, agents, index shape, snapshot count
cargo run --bin mnemo -- info agent.mnemo

# browse — table by default; --format json|jsonl for pipelines
cargo run --bin mnemo -- list agent.mnemo --type semantic --limit 20

# fetch one — copy a ULID from `list`
cargo run --bin mnemo -- get agent.mnemo 01HXYZ... --verbose

# rank — multi-signal recall (uses the ANN index if built)
cargo run --bin mnemo -- recall agent.mnemo --query 0.1,0.2,0.3 --top-k 5
```

`list` decrypts every live record (O(n)); it is meant for human-scale
exploration, not for serving requests. `recall` updates each returned
memory's `accessed_at` and `access_count` — but as of **v5** these live
on the catalog entry, not in the record body, so the update touches
**one catalog page per flush**, not the full vector of every result.
Pre-v5, a top-K recall rewrote K full records (vector + content) at
the next flush; v5 makes recall effectively a write-once-on-catalog
operation. Set `RecallRequest::track_access(false)` for a fully
read-only recall — useful for batch scoring, dry-runs, or tooling that
shouldn't perturb the database.

`recall` needs a vector in the database's dimensionality; for dogfooded
runs over real embeddings, pull a query vector from your embedding model
(or from `project-memory.jsonl` in the `test/` sandbox).

The passphrase is resolved in this priority order:

1. **`--passphrase`** flag if given. The CLI prints a warning to stderr
   because the value lands in shell history and process listings.
2. **`MNEMO_PASSPHRASE`** environment variable if set. Recommended for
   scripts and CI.
3. **Interactive TTY prompt** without echo. The fallback when neither
   the flag nor the env var is set. `init` and `rekey` prompt twice and
   verify a match so a typo doesn't lock the file. `rekey`'s new
   passphrase follows the same chain via `--new-passphrase`,
   `MNEMO_NEW_PASSPHRASE`, or a double-prompt.

**Raw-key files** (what `Mnemo::create_with_key` writes, and what the
phone libraries keep) open with `--key-file <path>` instead, on `info`,
`about`, `list`, `verify`, `recall`, `export`, `compact` and `inspect`
(since v0.13.1). The file holds the 32-byte key, raw or as 64 hex digits;
it can't be combined with `--passphrase`. `mnemo compact --key-file` is
how a raw-key v8 file becomes v9.

For real applications, prefer the library API and supply the passphrase
in-process.

```sh
export MNEMO_PASSPHRASE=hunter2
cargo run --bin mnemo -- init agent.mnemo --dimensions 768
cargo run --bin mnemo -- demo            # try it without any setup
```

### Readable export

`mnemo export` writes a file's live memories as plaintext: Markdown (a
section per memory, content in a fenced block) or JSON Lines (one object
per memory). It opens the file read-only, so the file never changes.

```sh
# to stdout; a warning on stderr says the output isn't encrypted
cargo run --bin mnemo -- export agent.mnemo --type semantic,procedural --agent ops

# diff-clean, for committing beside the project
cargo run --bin mnemo -- export agent.mnemo --git -o memory.md
```

- Vectors are always left out; superseded keyed versions are included.
- It writes a file only with `-o`, and replaces it whole: a new
  owner-only file (on Unix) renamed over the old one, never over the
  `.mnemo` file itself.
- **`--git`** orders records by key, then id (keyed memories first), and
  leaves out thread turns, access stats and counts. The same unchanged file
  exports to the same bytes, a recall doesn't change the output, and one new
  memory adds only its own lines (a new version of a keyed fact also
  changes the line of the version it supersedes).
- The result cache (and so the embedder-identity record) is never exported.
- **A committed export is world-readable elsewhere.** Git stores no file
  mode beyond the executable bit, so another checkout of a committed
  export is `0644` whatever `-o` wrote. Commit one only where everyone who
  can read the repository may read the memories.

### The inspector

`mnemo inspect` serves a read-only web view of a file on your machine:

```sh
cargo run --bin mnemo -- inspect agent.mnemo   # prints a link that works once: open it in your browser
cargo run --bin mnemo -- inspect app.mnemo --key-file app.key   # a raw-key file (32 bytes or 64 hex digits)
cargo run --bin mnemo -- inspect agent.mnemo --open   # opens the browser for you
```

The passphrase comes from the prompt or `MNEMO_PASSPHRASE`. Copy the
printed URL into your browser, or pass `--open`, which hands the browser a
private local page (owner-only, deleted once it has loaded) that forwards
to the URL. On Linux, a browser installed as a snap or a flatpak (Ubuntu's
default Firefox is a snap) has its own private `/tmp` and can't read that
page, so copy the printed link instead.

- **Views:** memories (filter by type, agent, keyed or not, and thread;
  keyword search with BM25, no embedder needed), keyed history (each
  key's versions, oldest first, with when each stopped being current),
  threads (the essence render, and the turn archive on demand), snapshots
  (the restore points `stats` reports) and about (format, recall mode,
  embedder identity, sizes).
- **It never holds the file.** It reads it once, read-only, and closes it,
  so an agent writing to the file is never blocked. **Reload** in the page
  reads it again (one passphrase derivation). If a writer has the file
  open at that moment, it says so and keeps the last read.
- **It stays on your machine.** It binds `127.0.0.1` only (a random free
  port unless `--port`). The printed link carries a random one-time code,
  which the page exchanges for a random session token on its first load
  and keeps for that tab only. **The link works once** (since v0.13.2): the
  code is refused after that, so the copy the browser's history records
  (or `--open`'s forwarding page) opens nothing. A refresh keeps working;
  another tab needs a new link (run `mnemo inspect` again). Every data
  request must carry the token and a `Host` of the loopback address and
  port, which stops other sites' pages (DNS rebinding included). Only `GET`
  is served, with no CORS, and no endpoint writes the file. **What it
  writes to disk:** without `--open`, nothing. With `--open`, one small
  page that forwards the browser to the link, in a new folder under the
  system temp directory (on macOS and Linux, a folder only you can open,
  holding a page only you can read). It's removed (with
  its folder) as soon as the page has exchanged the link's code, or after
  60 seconds if it never does. If the inspector is stopped before that, the
  page can be left behind, holding a code that no longer works. The token
  stops working when the inspector stops.
- **It stops when it's left alone.** After `--idle-timeout` without a
  request from the page (30 minutes by default; `90s`, `1h`, or `0` for
  never) it wipes the passphrase or key from memory and exits. It holds
  the secret only so that Reload works.
- It works on v8 files too, and on passphrase exports from the phone
  libraries.

`mnemo_engine::KeywordIndex` is the in-memory keyword index it searches
with: the tokenizer and BM25 scoring of v9's keyword recall, over texts
you already hold.

## Build and test

```sh
cargo build --release
cargo test            # 66 integration + 26 CLI smoke + proptest state machine + unit tests
cargo run --example quickstart
```

Minimum supported Rust version: **1.75**. All dependency versions are pinned
exactly for reproducibility.

## Performance

A running log of measured performance, so future versions can be compared
against earlier baselines. Each entry records the corpus, the build, and the
numbers — append a new entry rather than overwriting one when a change is
expected to move them.

### ARM users: enable hardware AES

> This affects everyone running mnemo on Apple Silicon, AWS Graviton,
> or any other aarch64 CPU, if you build mnemo from crates.io.

The `aes` and `polyval` crates (used by `aes-gcm`, which encrypts every
page in a `.mnemo` file) only emit ARMv8 hardware AES + PMULL instructions
when built with two cfg flags. Without them, every aarch64 build runs a
constant-time software fallback at roughly **5× the cost per page**
decrypted or encrypted, dominating the exact-scan recall path and the
per-page WAL/flush overhead on an M1.

If you installed from crates.io (`cargo install mnemo-engine`), set
RUSTFLAGS before installing:

```sh
RUSTFLAGS="--cfg aes_armv8 --cfg polyval_armv8" cargo install mnemo-engine
```

Or add to your own `~/.cargo/config.toml` so every build picks it up:

```toml
[target.'cfg(target_arch = "aarch64")']
rustflags = ["--cfg", "aes_armv8", "--cfg", "polyval_armv8"]
```

To verify which backend your build picked up, run `mnemo info` (CLI) or
`mnemo.build_info()` (Python) — the output names each backend explicitly:

```
aes:         armv8 hardware
polyval:     armv8 PMULL hardware
```

If you see `armv8 software (constant-time fallback)` instead, your
RUSTFLAGS didn't reach the build.

x86_64 users don't need to do anything — the same crates runtime-detect
AES-NI + CLMUL. The flags above are a no-op on x86_64.

Mnemo's own PyPI wheels (`pip install mnemo-engine`) and GitHub Release
binaries carry the hardware path baked in — this section only matters if
you build from source.


### Sizing tips

Three knobs cover most of the small-file size questions; the fourth is a
modelling choice that compounds with the others.

**1. Right-size the WAL reservation.** The default initial WAL is 8 pages
(64 KiB), down from 64 (512 KiB) in v0.1.0 — the v0.1.0 reservation
dominated small-file size (~62% of a 31-memory dogfood file). The WAL
auto-grows, so the default is safe even for large catalogs; raise it only
if you know each transaction routinely commits a large catalog or ANN
index and you want to skip the first grow event:

```rust
let cfg = MnemoConfig {
    dimensions: 768,
    wal_pages_initial: 64,     // 512 KiB up front
    ..Default::default()
};
```

**2. Cap the snapshot manifest.** Every `flush` appends one snapshot
entry; without a cap, long-running services accumulate them forever —
manifest serialize-cost and the manifest run on disk grow O(total
flushes). The default cap on v8 is 256 retained snapshots (the most
recent N are kept; older ones are pruned at flush time, with their data
pages left on disk until the next `compact_file`). On v9 it's 16, plus
any from the last 24 hours (see "Snapshots & point-in-time recovery"),
and pruned snapshots' pages are reused. Override per-database:

```rust
let cfg = MnemoConfig {
    dimensions: 768,
    max_snapshots: 1024,   // keep more history
    ..Default::default()
};
// or 0 to disable the cap entirely (pre-v0.3 behavior):
let cfg = MnemoConfig { max_snapshots: 0, ..Default::default() };
```

`Mnemo::set_max_snapshots(usize)` overrides on an already-open handle.
`restore_to` a pruned `txn_id` returns `MnemoError::NotFound`.

**3. Pick dimensions wisely.** Vector storage is `dimensions × 4` bytes
per memory and dominates the data line for everything but tiny corpora.
Cut dimensions and you cut that line proportionally — a 1024-dim model
costs 4 KiB/memory; a 256-dim model costs 1 KiB/memory.

**4. Use a Matryoshka (MRL) embedder, then truncate.** Matryoshka
Representation Learning trains a model so the most important information
is front-loaded into the early dimensions. With an MRL-trained embedder
(OpenAI `text-embedding-3-*`, Nomic `nomic-embed-text-v1.5`, Snowflake
`arctic-embed-l-v2.0`, and several others) you can store the first
`k` dims of a `d`-dim vector and lose only 1–2% recall — a 4× storage
win before any quantization. Concretely: a 1024-dim MRL model truncated
to 256 dims fits in 1 KiB/memory instead of 4 KiB. Slice the vector on
the client before passing it to `remember`; mnemo just sees a 256-dim
vector and uses it as the database's native dimensionality.

These knobs combine: a 256-dim MRL model in a fresh-default file gives
roughly **8× less on-disk footprint at small N** versus a 1024-dim model
in a v0.1.0 default file, and the gap widens as the corpus grows. Future
versions may add page-level compression (Zstd) and binary quantization
(32× on vectors); when they land they will compose on top of these
choices, not replace them.

### Baseline — v0.1.0, May 2026 (dogfood DB)

**Corpus.** `test/project.mnemo`, 31 live memories, 384-dim MiniLM
embeddings, encrypted, no ANN index, 1.41 MB on disk (844 KB at 28
memories — the file grew under repeated bootstraps via append-only history).

**Builds.** CLI numbers are from the **debug** `cargo run --bin mnemo`;
Python numbers are from the **release** `maturin develop` wheel. Argon2id
parameters are `KdfParams::secure()` (`m_cost=19456`, `t_cost=2`). Latencies
are medians over a handful of runs, not rigorous percentiles.

**File-size breakdown** at 844 KB / 28 memories:

| Region                          | Pages | Size   | Share |
|---------------------------------|-------|--------|-------|
| Header (plaintext, KDF + salt)  | 1     | 8 KB   | ~1%   |
| WAL region (reserved)           | 64    | 512 KB | ~62%  |
| Data, catalog, manifest, history| 38    | 311 KB | ~37%  |

Per-page crypto overhead is 28 B (12-byte nonce + 16-byte GCM tag) ≈ 0.3% —
negligible. The dominant costs at small N are the 512 KiB WAL reservation,
8 KiB page rounding, and the append-only snapshot history left behind by
repeated flushes. Logical payload (UTF-8 text + f32 vectors) is ~50 KB,
≈ 5.9% of the file; effective on-disk cost is ~45 KB per memory at N=31.

**Operation latency** (debug CLI unless noted):

| Operation                           | Median  | Notes                              |
|-------------------------------------|---------|------------------------------------|
| `info`                              | ~330 ms | KDF + read header/catalog          |
| `list` (31 memories)                | ~330 ms | KDF + decrypt/decode all records   |
| `get` (one memory)                  | ~290 ms | KDF + decrypt one record           |
| `verify`                            | ~330 ms | KDF + decrypt/validate all         |
| Python `open` + `recall` top-10     | ~35 ms  | release wheel, same crypto         |
| Python `remember` + `flush`         | ~31 ms  | includes MiniLM embed + WAL fsync  |

**Verdict at this scale.** Argon2id dominates every CLI invocation
(~250–300 ms of the ~330 ms). Per-record decrypt is cheap; reopening the
file is not. Long-lived processes (an agent server, the Python binding held
across calls) avoid the KDF tax and land in the tens-of-milliseconds range.
For a 31-memory dogfood file, on-disk overhead is ~17× the raw payload —
this is the format's worst case (fixed WAL reservation + page rounding +
append history dominate everything else). The README design targets
(<5 ms recall at 100 K, <15 ms at 1 M, <4 KB/memory) are aspirational
goals at scale, not yet measured here.

> **Historical note (updated 2026-10).** The CLI timings in this
> baseline were taken on a *debug* `cargo run --bin mnemo` build. The
> v0.5.x Phase 6 harness re-measures the same operations with release
> builds and reports them in the Performance history row + the
> `test/scripts/perf_v0.5.0.json` artifact — on a release build with
> production KDF the open latency settles at ~15 ms, not ~330 ms.
> Keeping this row for history, not as a current baseline.

**Knobs that should help small files.** Running `compact` collapses
snapshot history back to a single page run; the default 64-page WAL
reservation is fixed today and would need a config change to shrink. ANN
index builds are not worth it below ~thousands of memories.

### Performance history

Append a row whenever a release changes how the format behaves. Earlier
rows stay as-is so improvements (or regressions) are visible.

| Version | Date     | Corpus     | File size | CLI `info` | Python recall | Notes                  |
|---------|----------|------------|-----------|------------|---------------|------------------------|
| v0.1.0  | 2026-05  | 31 mems, 384-dim, no index | 1.41 MB | ~330 ms | ~35 ms | Initial dogfood baseline |
| v0.5.0  | 2026-10  | 10K mems, 384-dim, IVF+PQ  | 82.9 MB | — (open 14.9 ms secure KDF) | 374 µs (IVF+PQ†) / 623 ms (exact‡) | Phase 6: criterion benches + ranking harness + ANN recall@10 grid + file-size breakdown. IVF+PQ ≈ 1,660× faster than exact at 10K; 10,400× at 100K. Default `IndexConfig` recovers only 40% of exact top-10 — **index-quality defect, fixed in ANN-1 below** (originally mis-filed as a scoring issue). Full numbers in `test/scripts/perf_v0.5.0.json`. |
| v0.5.1  | 2026-10  | same corpus, hardware AES on ARM | — | — | 105 ms (exact 10K) / 1.05 s (exact 100K); IVF+PQ unchanged | PR 11.0: enable `--cfg aes_armv8 --cfg polyval_armv8` so aarch64 builds use hardware AES + PMULL instead of the constant-time software fallback. **6.0×** faster exact scan at 10K, **6.1×** at 100K; 1.3× on cache_put; IVF+PQ and Argon2id-bound open unchanged. See `test/scripts/perf_v0.5.0.json`'s `speed_hw_aes` block for the full set. No API or format change. If you build from crates.io see the "ARM users" section above. |

> † IVF+PQ speed at the old defaults; recall@10 was ~0.40 at those
> defaults, not 1.0. PR ANN-1 raises the default to ≥0.95 recall@10 at
> ~10 ms/query at 100K; see the `ann_recall_grid_v2` block in
> `perf_v0.5.0.json`.
> ‡ Both v0.5.0 timings were measured on software AES (the fix landed
> in v0.5.1). On hardware AES, exact scan at 10K drops to 105 ms;
> IVF+PQ was never AES-bound so it's unchanged.

## Scope: what is and isn't here

This crate is a faithful build of the MNemo plan, built to actually compile,
run, and pass tests. The storage engine, encryption, agent-memory model,
multi-signal recall, the IVF+PQ approximate index (Phase 2), the write-ahead
log (Phase 3), snapshot-based point-in-time recovery, the bounded LRU page
cache, and the `Session` lifecycle wrapper (Phase 5) are all built and tested.
Search and recall scale from exact brute force to sub-linear retrieval; `flush`
is a single-fsync atomic transaction repaired by WAL replay on a crash; every
transaction is a restorable snapshot back to the last compaction; and a
conversation runs through a `Session` that consolidates its turns into episodic
memory. See [Durability](#durability--write-ahead-log),
[Snapshots](#snapshots--point-in-time-recovery), [Sessions](#sessions), and
[Approximate index](#approximate-index-ivf--pq).

One item is deliberately left as a roadmap item and is **not** in this build:

- **TypeScript bindings.** Phase 6 of the plan also calls for a Node/WASM
  package via napi-rs; only the Python bindings are built so far. They live in
  the sibling `mnemo-python/` crate — a PyO3 wrapper exposing `mnemo.open(...)`
  and a `Mnemo` class (`pip install maturin && maturin build`). The Rust core
  a TypeScript binding would wrap is complete.

## License

Apache-2.0.
