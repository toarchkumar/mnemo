# Memory Nemo (MNemo)

[![CI](https://github.com/toarchkumar/mnemo/actions/workflows/ci.yml/badge.svg)](https://github.com/toarchkumar/mnemo/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/mnemo-engine)](https://pypi.org/project/mnemo-engine/)
[![crates.io](https://img.shields.io/crates/v/mnemo-engine)](https://crates.io/crates/mnemo-engine)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

**Encrypted memory and cache for AI agents, in one file. Written in Rust.**

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

Measured on an Apple M1 Pro (release build, 384-dimension vectors):

| Operation | Time |
|---|---|
| Exact-key cache hit | ~1 µs |
| Similar-prompt cache hit (500 entries) | ~0.4 ms |
| Open a file: passphrase key derivation (Argon2id, slow on purpose) | ~15 ms |
| Open a file: loading its catalog and index, 10K / 100K memories | ~33 ms / ~230 ms more |
| Store one memory durably | ~19 ms (~0.25 ms each when 100 are committed together) |
| Indexed search, 10K / 100K / 1M memories | ~1.4 ms / ~9.4 ms / ~134 ms |
| Exact search over every memory, 10K / 100K | ~105 ms / ~1.05 s |

**Ranking quality (v0.7.0).** On a small age-varied test set (13
questions), the right memory comes first for 92% of them, up from 77% in
v0.6 and 62% when ranking on similarity alone. The ranking stays the same
however often the memories are recalled.

Indexed search finds 96–98% of the true top-10 matches at the default
settings (v0.6.0: 0.964 at 10K, 0.982 at 100K, 0.970 at 1M). Storing got about
14% faster in v0.6.1, after one disk sync per commit was removed. Full results:
[`test/scripts/perf_v0.5.0.json`](test/scripts/perf_v0.5.0.json).

Opening a large file is dominated by loading its catalog and index, not by
the key derivation. Long-running agents and the MCP server pay it once;
faster opening of large files is planned.

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
