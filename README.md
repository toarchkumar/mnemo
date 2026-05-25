# Memory Nemo (MNemo)

**An encrypted, single-file, portable agent-memory engine, written in Rust.**

Give your agent a brain it can carry: one `.mnemo` file, encrypted at rest, with
multi-signal recall, sessions, snapshots, and an optional IVF+PQ index — no
vector server required.

| | |
|---|---|
| **Site** | [index.html](index.html) — open locally, or enable **GitHub Pages** (Settings → Pages → branch `main`, folder `/ (root)`) for `https://toarchkumar.github.io/mnemo/` |
| **GitHub** | https://github.com/toarchkumar/mnemo |
| **Rust core** | [`mnemo/`](mnemo/) — library, CLI, examples, integration tests |
| **Python** | [`mnemo-python/`](mnemo-python/) — PyO3 bindings ([maturin](https://www.maturin.rs)) |

## Quick start

```sh
cd mnemo
cargo run --example quickstart
cargo run --bin mnemo -- demo
```

```sh
cd mnemo-python
pip install maturin && maturin develop
```

Full architecture, API, CLI, durability, and scope notes:
[`mnemo/README.md`](mnemo/README.md).

## Repository layout

```
├── index.html       # landing page (GitHub Pages)
├── mnemo/           # Rust crate + CLI
└── mnemo-python/    # Python package
```

## License

Apache-2.0 — see [LICENSE](LICENSE).
