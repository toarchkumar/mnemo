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
pip install mnemo-engine          # Python (import mnemo)
cargo add mnemo-engine            # Rust library (use mnemo::...)
cargo install mnemo-engine        # the `mnemo` CLI and MCP server
```

The package is `mnemo-engine` everywhere. `mnemo-db` on PyPI and `mnemo` on
crates.io are unrelated projects.

Prebuilt CLI binaries for Linux, macOS and Windows are on the
[Releases](https://github.com/toarchkumar/mnemo/releases) page. The PyPI wheels
and release binaries for Apple Silicon and ARM Linux use the CPU's hardware AES.
If you build from crates.io on ARM yourself, see
[enabling hardware AES](mnemo/README.md).

## Quick start

Python, with any embedding model:

```python
import mnemo
from sentence_transformers import SentenceTransformer

MODEL = "sentence-transformers/all-MiniLM-L6-v2"
model = SentenceTransformer(MODEL)
embed = lambda texts: model.encode(texts, normalize_embeddings=True).tolist()

with mnemo.open("agent.mnemo", "a long passphrase", dimensions=384) as db:
    db.set_embedder(embed, name=MODEL, dimensions=384)
    db.remember_text("the user prefers dark mode", "semantic", importance=0.8)
    for hit in db.recall_text("which theme does the user like?", top_k=5):
        print(round(hit["score"], 3), hit["content"])
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

Measured on v0.5.1 (Apple M1 Pro, release build, 384-dimension vectors):

| Operation | Time |
|---|---|
| Exact-key cache hit | ~1 µs |
| Similar-prompt cache hit (500 entries) | ~0.4 ms |
| Open an encrypted file (Argon2id key derivation, on purpose) | ~15 ms |
| Store one memory durably | ~22 ms (~0.28 ms each when 100 are committed together) |
| Exact search over every memory, 10K / 100K | ~105 ms / ~1.05 s |

Full results: [`test/scripts/perf_v0.5.0.json`](test/scripts/perf_v0.5.0.json).
Indexed-search speeds are not listed yet; see the limitations below.

## Known limitations (being fixed)

- **Default index accuracy is low.** With an IVF+PQ index built, the default
  settings return only about 40% of the true top-10 matches. Until the fix
  lands, use exact search for small and medium files (don't build an index, or
  call `drop_index()`). From Rust or the CLI you can raise `n_probe` and
  `n_rerank` instead (`n_probe=32, n_rerank=256` reaches about 83%).
- **Access tracking can skew rankings.** By default, every recall counts its
  results as "used", which boosts them in later recalls, so frequently
  returned memories can crowd out more relevant ones. Pass
  `track_access=False` (Python), `track_access(false)` (Rust) or `--no-track`
  (CLI) to avoid it. A scoring fix is planned for v0.6.0.
- **Exact names and numbers.** Meaning-based search is weak on exact
  identifiers such as version numbers, PR numbers or function names. Keyword
  search combined with vector search is on the roadmap.

See the [changelog](CHANGELOG.md) for release history.

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
