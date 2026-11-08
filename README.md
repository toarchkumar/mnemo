# Memory Nemo (MNemo)

[![CI](https://github.com/toarchkumar/mnemo/actions/workflows/ci.yml/badge.svg)](https://github.com/toarchkumar/mnemo/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/mnemo-engine)](https://pypi.org/project/mnemo-engine/)
[![crates.io](https://img.shields.io/crates/v/mnemo-engine)](https://crates.io/crates/mnemo-engine)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

**Encrypted memory and cache for on-device AI agents, in one file, from desktop apps to phones. Written in Rust.**

MNemo gives an agent durable memory and a cache for expensive calls, inside a
single encrypted `.mnemo` file. There's no server or vector database to run:
copy the file and the memory comes with it.

- **Memory that ranks like an agent thinks.** Four memory types (episodic,
  semantic, procedural, working), recalled on similarity, recency, importance
  and use, with per-agent scoping and expiry.
- **Text in, text out.** Hand MNemo plain text; it calls your embedding model
  and checks it's the model the file was built with.
- **A result cache** for LLM responses, tool outputs and API calls: exact keys
  or near-identical prompts, with namespaces, expiry and size limits.
- **An MCP server built in**, so Claude Desktop, Cursor or any MCP client can
  use the file as tools.
- **Encrypted and crash-safe.** Every page is sealed with AES-256-GCM; commits
  go through a write-ahead log; every commit is a restorable snapshot; OS file
  locks allow one writer and many readers.

Site: **https://toarchkumar.github.io/mnemo/**

## Install

```sh
pip install mnemo-engine          # Python (import mnemo_engine)
cargo add mnemo-engine            # Rust library (use mnemo_engine::...)
cargo install mnemo-engine        # the `mnemo` CLI and MCP server
```

The package is `mnemo-engine` everywhere, and from v0.7.0 the Python module and
Rust library are `mnemo_engine` too. `mnemo-db` on PyPI and `mnemo` on
crates.io are unrelated projects.

Prebuilt CLI binaries for Linux, macOS and Windows are on the
[Releases](https://github.com/toarchkumar/mnemo/releases) page. The PyPI wheels
and release binaries for Apple Silicon and ARM Linux use the CPU's hardware AES.
If you build from crates.io on ARM yourself, see
[enabling hardware AES](mnemo/README.md).

## Quick start

Python, with any embedding model:

```python
import mnemo_engine
from sentence_transformers import SentenceTransformer

MODEL = "sentence-transformers/all-MiniLM-L6-v2"
model = SentenceTransformer(MODEL)
embed = lambda texts: model.encode(texts, normalize_embeddings=True).tolist()

with mnemo_engine.open("agent.mnemo", "a long passphrase", dimensions=384) as db:
    db.set_embedder(embed, name=MODEL, dimensions=384)
    db.remember_text("the user prefers dark mode", "semantic", importance=0.8)
    for hit in db.recall_text("which theme does the user like?", top_k=5):
        print(round(hit["score"], 3), hit["content"])

    # A fact that changes: the key replaces the old version, and history is kept
    db.remember_text("the current release is v0.8.0", "semantic", key="current-release")
```

CLI, with any program that turns text into vectors (a reference script is in
[`mnemo/examples/embedder_server.py`](mnemo/examples/embedder_server.py)):

```sh
export MNEMO_PASSPHRASE='a long passphrase'
mnemo init agent.mnemo --dimensions 384
mnemo remember agent.mnemo --text "the user prefers dark mode" \
  --embed-cmd "python3 embedder_server.py" --embed-name sentence-transformers/all-MiniLM-L6-v2
mnemo recall agent.mnemo --text "which theme does the user like?" \
  --embed-cmd "python3 embedder_server.py" --embed-name sentence-transformers/all-MiniLM-L6-v2
```

MCP (for example, Claude Desktop's `claude_desktop_config.json`):

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

The full Rust API, every CLI command, the cache, the file format, durability
and the security model are documented in [`mnemo/README.md`](mnemo/README.md).
The Python API is in [`mnemo-python/README.md`](mnemo-python/README.md).

## What's new in v0.10.0: smaller and faster

The first release of the on-device work, with no file format change. At
100K memories, compared with v0.9.1:

- **Opening takes 9 ms instead of 384 ms.** It no longer re-reads the crash
  log, and the search index loads on first use.
- **Saving one memory takes 28 ms instead of 449 ms,** and grows the file
  by 3.4 MB instead of 21 MB. The search index is saved every 64 commits
  and on close, not on every save.
- **An open file uses 34–54 MB of memory instead of 115–158 MB.** Warm
  searches take 8.8 ms instead of 23–27 ms, with the same accuracy.
- **Keys for keychains.** A file created with a random key skips the
  passphrase step, so it opens without the ~16 ms and ~19 MB that
  passphrase hashing costs:

```python
key = mnemo_engine.generate_key()  # keep it in the OS keychain
db = mnemo_engine.open_with_key("agent.mnemo", key, dimensions=384)
```

The page cache is now 8 MB by default (it was 64 MB). Raise it with
`set_cache_capacity` if your searches rerank many candidates.

## What's new in v0.9.0: threads and essence

Memory is the knowledge an agent looks up. **Essence** is where a piece of
work stands and how it got there, so a new session can carry on where the
last one stopped. v0.9.0 keeps both, in the same encrypted file:

- **A full copy of a thread.** `ingest_thread` (or `mnemo thread ingest
  --transcript … --format claude-code-jsonl`) saves every turn word for
  word. Running it again only adds the new turns. `thread_export` gives the
  thread back, and `thread_forget` removes it. Turns stay out of normal
  recall unless you ask for them.
- **Essence.** Short items in sections (intent, conventions, decisions,
  state, open, next, lessons), plus a timeline of milestones that only
  grows. `essence(thread=…)` renders it within a size budget, ready to load
  at the start of a session. Items are edited one at a time, never
  rewritten as a whole summary, so they don't drift.
- **Carry on in a new thread.** `thread_start("new", "old")` links the
  threads, and the new thread's essence begins with the old one's open
  items, next steps and recent milestones.
- **One call to save a batch.** `commit_deltas` writes facts, essence
  changes and turns together, or nothing at all if any part is invalid.

```python
db.ingest_thread("planner", [{"turn_id": "t1", "role": "user", "content": "..."}])
db.essence_add("decisions", "Chose V2 scoring: best ranking, no drift", thread="planner")
db.essence_append("Shipped v0.9.0", thread="planner")
db.thread_start("planner-2", "planner")
print(db.essence(thread="planner-2"))
```

This uses the embedder you registered with `set_embedder` (see Quick
start). No file format change: v0.9.0 files open in v0.7 and v0.8, but
those versions show turns and essence items as ordinary memories.

## What's new in v0.8.0

- **Facts that change.** `remember(..., key="current-release")` replaces the
  memory holding that key with a new version. The old version leaves recall
  but stays in `history(key)`. Writing the same value again does nothing.
  `update(id, ...)` corrects a memory in place (the id is kept).
- **Filtered recall.** Limit a search to a session or to metadata values:
  `recall_text(..., filter={"session_id": "s1", "metadata": {"area": "billing"}})`,
  with `$in`, `$gte`/`$lte` and `$exists`. The CLI has `--session` and
  `--filter`; MCP takes a `filter` object.
- **Recall within a size budget.** `max_bytes=4096` stops adding results once
  their text would pass the budget, and reports whether anything was cut.
  It's for small context windows.

All three work the same in Rust, Python, the CLI and MCP. Files written by
v0.8.0 still open in v0.7, but v0.7 shows replaced versions as ordinary
memories, so upgrade every tool that reads the file.

## For AI agents

A `.mnemo` file introduces itself. Handed a file and its passphrase, an agent
needs one command to learn which embedding model it expects, its conventions,
and any notes its author left:

```sh
mnemo about path/to/agent.mnemo      # or db.about() in Python
```

`mnemo init` adds a starter manifest, so new files are self-describing from the
start. Working on this repository itself? Start at [AGENTS.md](AGENTS.md).

## Performance

Measured with v0.10.0 on an Apple M1 Pro (release build, 384-dimension
vectors, about 300 bytes of text per memory, indexed):

| | 1K memories | 10K | 100K |
|---|---|---|---|
| Open with a raw key (from a keychain) | 0.2 ms | 0.9 ms | 9 ms |
| Open with a passphrase (Argon2id, slow on purpose) | ~16 ms | ~16 ms | ~23 ms |
| First search after opening (loads the index) | 3 ms | 11 ms | 99 ms |
| Search, warm | 0.4 ms | 1.4 ms | 8.8 ms |
| Store one memory durably | 16 ms | 16 ms | 28 ms |
| … each, when 100 are committed together | 0.26 ms | 0.26 ms | 0.42 ms |
| Memory used by an open file (after 1,000 searches) | 11 MB | 13 MB | 34 MB |
| Disk per memory | 10 KB | 8.9 KB | 8.7 KB |

The library adds about 1.4 MB (Python package) or 2.4 MB (CLI), stripped.
An exact-key cache hit takes ~1 µs, and a similar-prompt hit over 500
entries ~0.4 ms. Full results:
[`test/scripts/perf_v0.5.0.json`](test/scripts/perf_v0.5.0.json).

**Ranking quality (v0.7.0).** On a small age-varied test set (13
questions), the right memory comes first for 92% of them, up from 77% in
v0.6 and 62% when ranking on similarity alone. The ranking stays the same
however often the memories are recalled.

Indexed search finds 96–98% of the true top-10 matches at the default
settings (0.964 at 10K, 0.982 at 100K, 0.970 at 1M); v0.10.0 kept every
measured accuracy cell unchanged.

**Upgrading an index from v0.5.x?** It improves automatically (about 76–83%
of the true top-10), and rebuilding it restores the full accuracy: run
`mnemo index <file>` again, `build_index()` in Python, or `rebuild_index()`
in Rust. Each re-clusters the index with the new defaults.

## Upgrading to v0.7.0

- **New import name.** In Python, `import mnemo` becomes `import mnemo_engine`;
  in Rust, `use mnemo::...` becomes `use mnemo_engine::...`. The package
  name, the `mnemo` CLI, the `.mnemo` file format and existing files are
  unchanged.
- **New ranking.** Recency now counts from when a memory was stored, not
  when it was last recalled. The bonuses for recency, importance and use
  are scaled by similarity, so an unrelated memory can't win on being new
  or popular. Scores are on a new scale: if you compare `score` to a fixed
  threshold, check it again.

## Known limitations (being fixed)

- **Files still grow with each save and use about 8.7 KB per memory.**
  Each save adds a new copy of the memory list (about 3.4 MB at 100K
  memories), and each memory takes a whole 8 KB page. `mnemo compact`
  reclaims the space. The next file format (v0.11) saves only what changed,
  reuses old space and packs memories together.
- **Saving threads isn't automatic yet.** You (or your agent) call
  `thread ingest` and the essence tools. A Claude Code plugin with hooks
  that save before compaction and load at session start is on the roadmap,
  after the current size and speed work. Transcripts can contain anything that was pasted into
  them; they're encrypted like everything else, and `thread forget`
  removes them.
- **Version history has no limit yet.** Every keyed replacement keeps the
  old version, so a key that changes very often grows the file. A
  per-key retention setting is planned.
- **Exact names and numbers.** Meaning-based search is weak on exact
  identifiers such as version numbers, PR numbers or function names (a
  question about "BP.1" can surface the wrong memory). Keyword search
  combined with vector search is on the roadmap.

See the [changelog](CHANGELOG.md) for release history.

## Security advisory (fixed in v0.5.2)

Versions before 0.5.2 could reuse AES-GCM nonces if a `.mnemo` file was
copied and both copies were then written to: for example, two machines
syncing the same file, a sync service's "conflicted copy", or restoring an
older backup and carrying on with it. Someone holding both copies could
recover parts of the content or forge pages. From v0.5.2, every writing
session draws a fresh random nonce prefix, so this can't happen.

If you may have written to two copies of the same file with an earlier
version, upgrade, then run `mnemo compact` (or `compact_file` in Rust or
Python) on the copy you keep. Compaction rewrites the file under a fresh
data key. Starting in v0.6.0 compaction also preserves the result cache
(FIX-1); on v0.5.x the result cache was emptied by compaction and had to
be recomputed on next use. Details are in the [changelog](CHANGELOG.md).

## Repository layout

```
├── mnemo/              # Rust engine, CLI and MCP server, benches, tests
├── mnemo-python/       # Python package (PyO3 + maturin), published as mnemo-engine
├── test/               # dogfooding sandbox: this project keeps its own memory in a .mnemo file
├── index.html          # the project site
├── AGENTS.md           # orientation for AI coding agents working on this repo
└── CHANGELOG.md
```

To build from source: `cd mnemo && cargo test` for the engine, or
`cd mnemo-python && pip install maturin && maturin develop --release` for the
Python package.

## License

Apache-2.0. See [LICENSE](LICENSE).
