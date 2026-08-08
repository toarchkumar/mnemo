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
| `set_cache_capacity(pages)` / `page_cache_stats()` | Page-cache tuning (renamed from `cache_stats` in v0.4.0 — that name now belongs to the result cache below) |
| `cache_put(namespace, key, value, content_type="text", ttl_secs=None)` | Exact-key result cache put (Phase 10.1). `value` accepts `str` or `bytes` |
| `cache_get(namespace, key)` | Exact-key cache get — returns `dict` on hit (`value` is `bytes`) or `None` |
| `cache_delete(namespace, key)` / `cache_purge(namespace=None, expired_only=False)` | Tombstone by key or in bulk |
| `cache_stats(namespace=None)` | Result-cache stats: `{entries, bytes, hits, misses, hit_rate, evictions}` |
| `cache_put_semantic(namespace, key, vector, value, model, content_type="text", ttl_secs=None)` | Semantic cache put (Phase 10.2) — vector must match db dimensions |
| `cache_get_semantic(namespace, query, model, threshold=0.97)` | Top-1 cosine over the namespace's vectored entries whose `model` matches |
| `set_max_snapshots(max)` | Override the snapshot-manifest retention cap (default 256; `0` disables) |
| `stats()` | Summary statistics |
| `export_encrypted(dest)` | Copy the (already-encrypted) file elsewhere |
| `len(db)` | Live memory count |

`Session` methods: `add_turn(turn)`, `recall(query, top_k=10, memory_types=None)`,
`close()`, `discard()`, `id()`, `agent()`, `turn_ids()`, `turn_count()`; also a
context manager (exiting consolidates).

Memories and results are returned as plain dicts; `metadata` round-trips as a
nested dict.

## Result caching

MNemo doubles as a durable result cache — memoize LLM tool-call outputs,
prompt→completion pairs, HTTP responses, anything reconstructible on miss —
in the same encrypted single file that holds your agent memory. See the
[core README's Result caching section](../mnemo/README.md) for the
concepts (namespaces, budgets, TTL, Strict vs Batched flush policy).

### Recipe: `@db.cached(...)` decorator (~90 lines, pure Python)

Copy this into your project as `mnemo_cached.py` or paste inline. It
wraps any function whose arguments serialize to JSON, keying the cache
on `f"{namespace}:{fn_name}:{json_args}"`. When `embed` is supplied it
switches to semantic mode; otherwise it's an exact-key cache.

```python
"""@db.cached — pure-Python helper on top of mnemo.Mnemo's cache API.

Usage:
    import mnemo, json, os
    from mnemo_cached import cached

    db = mnemo.open("agent.mnemo", os.environ["MNEMO_PASSPHRASE"])

    @cached(db, namespace="llm", ttl=3600)
    def call_llm(prompt: str) -> str:
        return openai.chat.completions.create(...).choices[0].message.content

    # Second call with the same prompt is a cache hit.
    answer = call_llm("summarize this doc")
"""
from __future__ import annotations

import functools
import hashlib
import json
from typing import Any, Callable, Optional


def cached(
    db,
    *,
    namespace: str,
    ttl: Optional[int] = None,
    embed: Optional[Callable[[str], list[float]]] = None,
    model: Optional[str] = None,
    threshold: float = 0.97,
    key_fn: Optional[Callable[..., str]] = None,
):
    """Memoize a function's results into a mnemo.Mnemo cache.

    - Exact-key mode (default): key is JSON(args, kwargs) plus fn name.
    - Semantic mode: pass `embed=my_embedder` and `model="..."`; the key
      is embedded and semantic recall is tried before falling back.
    - Custom `key_fn(*args, **kwargs) -> str` overrides the default key.
    """
    if embed is not None and model is None:
        raise ValueError("semantic mode requires `model=`")

    def decorator(fn: Callable[..., Any]) -> Callable[..., Any]:
        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            key = key_fn(*args, **kwargs) if key_fn else _default_key(fn, args, kwargs)

            # Try semantic hit first (if configured); otherwise exact-key.
            if embed is not None:
                vec = embed(key)
                hit = db.cache_get_semantic(namespace, vec, model, threshold)
                if hit is not None:
                    return _decode(hit["value"])
                # Miss — compute the real result.
                result = fn(*args, **kwargs)
                encoded = _encode(result)
                db.cache_put_semantic(namespace, key, vec, encoded, model, ttl_secs=ttl)
                return result

            hit = db.cache_get(namespace, key)
            if hit is not None:
                return _decode(hit["value"])
            result = fn(*args, **kwargs)
            encoded = _encode(result)
            db.cache_put(namespace, key, encoded, ttl_secs=ttl)
            return result

        return wrapper

    return decorator


def _default_key(fn, args, kwargs) -> str:
    payload = json.dumps(
        {"fn": fn.__qualname__, "args": args, "kwargs": kwargs},
        default=str, sort_keys=True,
    )
    return hashlib.sha256(payload.encode()).hexdigest()


def _encode(v: Any) -> bytes:
    if isinstance(v, (bytes, bytearray)):
        return bytes(v)
    if isinstance(v, str):
        return v.encode()
    return json.dumps(v, default=str).encode()


def _decode(b: bytes) -> Any:
    # Best-effort round-trip: try JSON first, fall back to str, then bytes.
    try:
        return json.loads(b)
    except (json.JSONDecodeError, UnicodeDecodeError):
        try:
            return b.decode()
        except UnicodeDecodeError:
            return b
```

### Recipe: OpenAI/Anthropic call wrapped end-to-end

```python
import os, mnemo
from openai import OpenAI
from mnemo_cached import cached

db = mnemo.open("agent.mnemo", os.environ["MNEMO_PASSPHRASE"])
client = OpenAI()

@cached(db, namespace="openai-gpt-4o-mini", ttl=86_400)
def chat(prompt: str) -> str:
    r = client.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": prompt}],
    )
    return r.choices[0].message.content

# First call → hits the model, caches the response.
# Second identical call → hits the cache, no OpenAI request.
print(chat("Give me a haiku about SQLite."))
print(chat("Give me a haiku about SQLite."))

print("cache stats:", db.cache_stats("openai-gpt-4o-mini"))
```

Anthropic swap-in — same shape, different client:

```python
import anthropic
from mnemo_cached import cached

client = anthropic.Anthropic()

@cached(db, namespace="claude-3-5-sonnet", ttl=86_400)
def chat(prompt: str) -> str:
    r = client.messages.create(
        model="claude-3-5-sonnet-latest",
        max_tokens=1024,
        messages=[{"role": "user", "content": prompt}],
    )
    return r.content[0].text
```

### Recipe: semantic cache with an embedder

```python
from openai import OpenAI
from mnemo_cached import cached

client = OpenAI()
def embed(text: str) -> list[float]:
    return client.embeddings.create(
        model="text-embedding-3-small", input=text,
    ).data[0].embedding

# Semantic mode: prompts that differ in wording but mean the same
# thing hit the same cache entry.
@cached(db, namespace="llm-semantic", ttl=86_400,
        embed=embed, model="text-embedding-3-small", threshold=0.97)
def chat(prompt: str) -> str:
    return client.chat.completions.create(
        model="gpt-4o-mini",
        messages=[{"role": "user", "content": prompt}],
    ).choices[0].message.content
```

**Note:** the database must have been created with `dimensions` matching
your embedder's output — `text-embedding-3-small` is 1536-dim,
`bge-large-en-v1.5` is 1024-dim, etc. Set at
`mnemo.open(path, pw, dimensions=1536)` on first creation.

**Don't use this for:** multi-node fleets where the cache must be
consistent across hosts (use Redis / DynamoDB DAX). MNemo's cache is
optimized for a single-host agent that owns its cache file.

## License

Apache-2.0.
