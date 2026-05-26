# Memory Nemo (MNemo)

**An encrypted, single-file, portable agent-memory engine, written in Rust.**

Give your agent a brain it can carry: one `.mnemo` file, encrypted at rest, with
multi-signal recall, sessions, snapshots, and an optional IVF+PQ index — no
vector server required.

| | |
|---|---|
| **Site** | https://toarchkumar.github.io/mnemo/ |
| **Rust core** | [`mnemo/`](mnemo/) — library, CLI, examples, integration tests |
| **Python** | [`mnemo-python/`](mnemo-python/) — PyO3 bindings ([maturin](https://www.maturin.rs)) |

## Quick start

```sh
cd mnemo
cargo run --example quickstart
cargo run --bin mnemo -- demo
```

CLI exploration (`info`, `list`, `get`, `recall`): see [`mnemo/README.md`](mnemo/README.md#exploring-a-database).

```sh
cd mnemo-python
pip install maturin && maturin develop
```

Full architecture, API, CLI, durability, and scope notes:
[`mnemo/README.md`](mnemo/README.md).

## Repository layout

```
├── AGENTS.md        # orientation for AI coding agents working on this repo
├── index.html       # landing page
├── mnemo/           # Rust crate + CLI
└── mnemo-python/    # Python package
```

## For AI agents

MNemo is built so an AI agent can pick up a `.mnemo` file and use it
without reading any external documentation — the file introduces itself.
Two commands cover the common cases:

```sh
# You've been handed an existing .mnemo file (plus its passphrase).
# This prints the orientation manifest: embedder, agent_id convention,
# project notes — everything the file's author chose to record.
mnemo about path/to/agent.mnemo

# You're starting fresh. `init` auto-inserts a scaffold manifest so
# the new file is self-describing from the moment it's created.
mnemo init my-agent.mnemo --dimensions 768
mnemo about my-agent.mnemo    # see what the scaffold says; replace it.
```

In Python: `db.about()` returns the same orientation memories;
`db.insert_default_manifest()` mirrors `mnemo init`'s scaffold.

**Working on this repo**, not just using mnemo? Start at
[AGENTS.md](AGENTS.md) — codebase layout, build commands, conventions,
and the dogfood workflow.

## License

Apache-2.0 — see [LICENSE](LICENSE).
