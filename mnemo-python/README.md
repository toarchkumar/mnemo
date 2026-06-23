# Memory Nemo (MNemo) — Python bindings

Repository overview: [root README](../README.md) ·
landing page: [index.html](../index.html).

Python bindings for **Memory Nemo (MNemo)**, the encrypted single-file
agent-memory engine. The package is a thin [PyO3](https://pyo3.rs) wrapper
over the Rust core in the
sibling `mnemo/` crate — the storage engine, AES-256-GCM encryption, the
write-ahead log, the IVF+PQ index, snapshots, and the agent-memory model all
run as compiled Rust; Python only sees a small, ergonomic surface.

> Distribution name on PyPI: `mnemo-engine` (both `mnemo` and `mnemo-db`
> were already taken by unrelated projects). The import name stays
> `mnemo`, so `pip install mnemo-engine` then `import mnemo` is the
> full setup.

## For AI agents

An agent that's been handed a `.mnemo` file and its passphrase can become
productive in two calls — no external documentation required:

```python
import mnemo, os

db = mnemo.open("agent.mnemo", os.environ["MNEMO_PASSPHRASE"])

# The file introduces itself: returns memories tagged metadata.area="onboarding",
# manifest first. Each entry tells you the embedder, agent_id convention,
# project metadata, and any other context the file's author recorded.
for entry in db.about():
    print(entry["content"])
```

Creating a new database? It's self-describing from creation:

```python
db = mnemo.open("new.mnemo", "passphrase", dimensions=384)
db.insert_default_manifest()    # same scaffold that `mnemo init` adds
db.flush()
```

The scaffold tells the next agent what to do: replace it with one that
records your real embedder and conventions. See the [main README](../mnemo/README.md#self-describing-databases)
for the full pattern.

## Build & install

The bindings build with [maturin](https://www.maturin.rs):

```bash
pip install maturin
cd mnemo-python
maturin build --release          # produces a wheel in target/wheels/
pip install target/wheels/mnemo-*.whl
```

`maturin develop` installs straight into the active virtualenv during
development. The extension is built against the stable ABI (`abi3-py38`), so a
single wheel works on CPython 3.8 and newer.

## Usage

```python
import mnemo

# Open an existing database, or create one (dimensions required to create).
db = mnemo.open("agent.mnemo", "passphrase", dimensions=4)

# Store typed memories. memory_type is one of:
# "episodic", "semantic", "procedural", "working".
db.remember(
    "the user prefers concise answers",
    "procedural",
    [0.1, 0.2, 0.3, 0.4],
    importance=0.8,
    agent_id="assistant",
    metadata={"source": "onboarding"},
)

# Multi-signal recall — similarity blended with recency, importance, frequency.
for hit in db.recall([0.1, 0.2, 0.3, 0.4], top_k=5):
    print(hit["score"], hit["content"])

db.flush()
db.close()
```

`mnemo.open` returns a `Mnemo` object that is also a context manager —
`with mnemo.open(...) as db:` flushes automatically on exit.

### Sessions

A `Session` wraps the database for one conversation: it records each turn as
`working` memory and, when closed, consolidates those turns into durable
`episodic` memory.

```python
db = mnemo.open("agent.mnemo", "passphrase", dimensions=4)

with db.session("assistant") as chat:
    chat.add_turn(mnemo.Turn.user("my flight is Friday", [1.0, 0.0, 0.0, 0.0]))
    chat.add_turn(mnemo.Turn.assistant("noted", [0.9, 0.1, 0.0, 0.0]))
    context = chat.recall([1.0, 0.0, 0.0, 0.0], top_k=5)
    # leaving the block consolidates the turns into episodic memory

# or, explicitly:
chat = db.session("assistant")
chat.add_turn(mnemo.Turn("system", "be concise", [0.0, 0.0, 0.0, 1.0]))
chat.close()      # consolidate working -> episodic
# chat.discard()  # alternative: throw the turns away
```

`mnemo.Turn` has `Turn.user(...)`, `Turn.assistant(...)`, `Turn.system(...)`,
and `Turn(role, content, vector)`. A `Session`'s `recall` is always scoped to
its own agent.

## API

`mnemo.open(path, passphrase, dimensions=None) -> Mnemo`

`Mnemo` methods:

| Method | Purpose |
|---|---|
| `remember(content, memory_type, vector, *, agent_id, importance, session_id, ttl_secs, shared, metadata)` | Store a memory; returns its id |
| `recall(query, top_k=10, memory_types=None, agent_id=None, track_access=True)` | Multi-signal ranked retrieval. `track_access=False` skips access-stat updates (fully read-only recall) |
| `search(query, top_k=10)` | Exact nearest-neighbour search |
| `get(id)` / `delete(id)` | Fetch / soft-delete by id |
| `about()` | Self-describing onboarding briefing — memories tagged `metadata.area="onboarding"`, manifest first |
| `insert_default_manifest()` | Insert the canonical scaffold manifest (same one `mnemo init` adds); returns its id |
| `session(agent_id)` | Begin a conversation `Session` |
| `flush()` / `close()` | Persist pending changes |
| `verify()` | Decrypt and re-validate every record |
| `build_index()` / `drop_index()` / `has_index()` | Approximate index control |
| `snapshots()` / `restore_to(txn_id)` / `restore_to_time(unix_secs)` | Point-in-time recovery |
| `set_cache_capacity(pages)` / `cache_stats()` | Page-cache tuning |
| `set_max_snapshots(max)` | Override the snapshot-manifest retention cap (default 256; `0` disables) |
| `stats()` | Summary statistics |
| `export_encrypted(dest)` | Copy the (already-encrypted) file elsewhere |
| `len(db)` | Live memory count |

`Session` methods: `add_turn(turn)`, `recall(query, top_k=10, memory_types=None)`,
`close()`, `discard()`, `id()`, `agent()`, `turn_ids()`, `turn_count()`; also a
context manager (exiting consolidates).

Memories and results are returned as plain dicts; `metadata` round-trips as a
nested dict.

## License

Apache-2.0.
