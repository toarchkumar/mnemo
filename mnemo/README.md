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

---
# Memory Nemo (MNemo)

**An encrypted, single-file, portable agent-memory engine, written in Rust.**

A whole memory store — vectors, content, structured metadata, and the
multi-signal recall machinery an agent needs — lives in **one file** you can
copy, back up, or hand to another process. The file is encrypted at rest, so
the memory of an agent is as portable and as private as a SQLite database.

This repository implements a real, compiling, tested **core**. See
[Scope](#scope-what-is-and-isnt-here) for what is deliberately left as roadmap.

> The product is **Memory Nemo (MNemo)**. The crate, binary, Python package,
> and `.mnemo` file extension all use the lowercase identifier `mnemo`.

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

The file is a sequence of fixed **8 KiB pages**:

- **Page 0** — the header (plaintext): magic, version, KDF parameters, salt,
  the wrapped DEK, pointers to the current catalog and ANN index runs, the
  location of the WAL region, and a CRC-32 over the header itself.
- **Pages 1..W** — the write-ahead log region (see below).
- **Pages W+** — encrypted runs. Each record (a memory, MessagePack-encoded),
  each catalog snapshot, the optional ANN index, and the snapshot manifest
  occupy runs of consecutive pages. The page allocator is append-only.

Every page is sealed with a unique nonce derived from `page_number` and a
monotonic `write_counter`, so a nonce is never reused under the DEK.

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

What this buys over a plain copy-on-write header swap: a single-fsync commit
and an explicit, replayable transaction boundary.

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
History reaches back to the last `compact_file`, which reclaims space by
rewriting the file and so collapses the manifest to a single snapshot. That
compaction boundary is the one limit: there is no external log archiving, so
you cannot restore to an instant older than the last compaction.

### Page cache

Decrypted page payloads are held in a **bounded LRU cache** so repeated reads
skip a decrypt. Eviction targets the least-recently-used *clean* page; a dirty
page — the only copy of an un-flushed write — is never evicted, so the cap
bounds retained clean pages without ever risking data. The default cap is
8192 pages (~64 MiB); `Mnemo::set_cache_capacity` tunes it, and
`Mnemo::cache_stats` reports occupancy. The LRU order is an intrusive linked
list over a slab of indices, so it stays within the crate's
`#![forbid(unsafe_code)]`.

### Recall

`recall` scores each candidate with

```
score = α·similarity + β·recency + γ·importance + δ·ln(1 + access_count)
```

where `recency` decays exponentially (default 7-day half-life). It also
filters by memory type and by agent scope (an agent sees its own memories
plus any marked `Shared`), and skips TTL-expired entries. Weights and the
similarity metric (cosine / dot / L2) are per-request.

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

`Mnemo::search` always stays exact — it is the brute-force ground truth.

## Quick start (library)

```rust
use mnemo::{Mnemo, MnemoConfig, Memory, MemoryType, RecallRequest};

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
| `get`     | Fetch one memory by its ULID                         |
| `search`  | Exact nearest-neighbour search (similarity only)     |
| `recall`  | Multi-signal ranked retrieval (sim + recency + …)    |
| `verify`  | Decrypt and re-validate every live record            |
| `rekey`   | Re-encrypt the data key under a new passphrase       |
| `compact` | Rebuild the file, dropping tombstones and expired    |
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
# then by importance descending, ending with a quick-start footer.
```

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

The passphrase comes from `--passphrase` or the `MNEMO_PASSPHRASE`
environment variable. **Passing a secret as a command-line argument is
insecure** — it lands in shell history and process listings. Prefer the
environment variable, and for real applications prefer the library API.

```sh
export MNEMO_PASSPHRASE=hunter2
cargo run --bin mnemo -- init agent.mnemo --dimensions 768
cargo run --bin mnemo -- demo            # try it without any setup
```

## Build and test

```sh
cargo build --release
cargo test            # 31 integration + 9 CLI smoke + 2 doctests + unit tests
cargo run --example quickstart
```

Minimum supported Rust version: **1.75**. All dependency versions are pinned
exactly for reproducibility.

## Performance

A running log of measured performance, so future versions can be compared
against earlier baselines. Each entry records the corpus, the build, and the
numbers — append a new entry rather than overwriting one when a change is
expected to move them.

### Sizing tips

Two knobs cover most of the small-file size questions; the third is a
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

**2. Pick dimensions wisely.** Vector storage is `dimensions × 4` bytes
per memory and dominates the data line for everything but tiny corpora.
Cut dimensions and you cut that line proportionally — a 1024-dim model
costs 4 KiB/memory; a 256-dim model costs 1 KiB/memory.

**3. Use a Matryoshka (MRL) embedder, then truncate.** Matryoshka
Representation Learning trains a model so the most important information
is front-loaded into the early dimensions. With an MRL-trained embedder
(OpenAI `text-embedding-3-*`, Nomic `nomic-embed-text-v1.5`, Snowflake
`arctic-embed-l-v2.0`, and several others) you can store the first
`k` dims of a `d`-dim vector and lose only 1–2% recall — a 4× storage
win before any quantization. Concretely: a 1024-dim MRL model truncated
to 256 dims fits in 1 KiB/memory instead of 4 KiB. Slice the vector on
the client before passing it to `remember`; mnemo just sees a 256-dim
vector and uses it as the database's native dimensionality.

These three combine: a 256-dim MRL model in a fresh-default file gives
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
