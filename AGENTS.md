# AGENTS.md

Orientation for any AI coding agent (Claude, Cursor, Copilot, Aider, etc.)
working on the Memory Nemo (MNemo) source code. **For agents using a
`.mnemo` file**, run `mnemo about <file>` instead — every `.mnemo` file
introduces itself.

## Project at a glance

MNemo is an encrypted, single-file, portable agent-memory engine written in
Rust. One `.mnemo` file holds vectors, content, structured metadata, the
write-ahead log, the IVF+PQ index, and snapshot history. Distributed as
`mnemo-engine` on PyPI and crates.io (both `mnemo` and `mnemo-db` were
already taken by unrelated projects). The library identifier stays
`mnemo` everywhere — `use mnemo::...` and `import mnemo` both work
after install.

## Repository layout

```
mnemo/              Rust core: library + CLI + integration tests
  src/              format / crypto / pager / wal / store / index / memory / session / cache
  src/bin/mnemo.rs  CLI binary (`mnemo` command after install)
  tests/            integration + cli_smoke
  examples/         quickstart, session
mnemo-python/       PyO3 bindings published as `mnemo-engine` on PyPI
test/               Dogfooding sandbox (encrypted project.mnemo + helpers)
  scripts/          project_memory.py — recall, remember, bootstrap, perf
  scripts/seed.json Canonical onboarding seed (manifest + 16 orientation memories)
index.html          Landing page
MNEMO_BUILD_PLAN.md Original 7-phase spec
```

## Build & test

Rust core (most common):

```bash
cd mnemo
cargo build --release
cargo test            # integration tests + CLI smoke + doctests + unit tests
cargo run --example quickstart
cargo run --bin mnemo -- demo
```

Python bindings (only when changing the PyO3 layer):

```bash
cd mnemo-python
pip install maturin
maturin develop --release    # installs into the active venv
python test_mnemo.py
```

Minimum supported Rust version: **1.75**. All dependency versions are pinned
exactly (`=x.y.z`) for reproducibility.

## Conventions

- `#![forbid(unsafe_code)]` on the core crate — never reach for unsafe.
- Public API gets doc comments (`#![warn(missing_docs)]`).
- Errors flow through `MnemoError` (`thiserror`) — return `crate::Result<T>`.
- No emojis in code or commits.
- New CLI commands follow the pattern in `mnemo/src/bin/mnemo.rs`: a
  `Subcommand` variant + a match arm in `run()` + an entry in the README's
  CLI table.
- New library methods go on `Mnemo` in `mnemo/src/store.rs`; mirror in the
  Python binding (`mnemo-python/src/lib.rs`) if user-facing.

## Dogfood the engine on yourself

This repo keeps an encrypted project memory at `test/project.mnemo`. Before
substantive work, orient yourself from the database itself:

```bash
# Engine-level: what is this file?
mnemo about test/project.mnemo

# Project-specific: pull context for the current task
python test/scripts/project_memory.py recall "<short description of your task>"
```

After learning something durable, persist it for the next session:

```bash
python test/scripts/project_memory.py remember "<one concise fact or decision>" \
  --type semantic|episodic|procedural --importance 0.0-1.0
```

One fact per `remember` call. Use `episodic` for dated events/decisions,
`procedural` for how-to steps, `semantic` for stable facts.

For performance baselines: `python test/scripts/project_memory.py perf <file.json>`
ingests a structured measurement set as episodic memories — see
`test/scripts/perf_v0.1.0.json` for the schema.

**Never commit:** `test/.env`, `test/project.mnemo`, or any other `.mnemo`
file. The `.gitignore` already excludes them.

## Common task locations

| Task | Where |
|---|---|
| Add a CLI command | `mnemo/src/bin/mnemo.rs` (Subcommand + match arm + README table) |
| Add a library method | `mnemo/src/store.rs` impl Mnemo (+ re-export in `lib.rs` if needed) |
| Add a Python method | `mnemo-python/src/lib.rs` (impl Mnemo block) + `mnemo-python/README.md` API table |
| Add an integration test | `mnemo/tests/integration.rs` (uses `KdfParams::fast()` for speed) |
| Add a CLI smoke test | `mnemo/tests/cli_smoke.rs` (uses `CARGO_BIN_EXE_mnemo` + tempfile) |
| Touch the on-disk format | `mnemo/src/format.rs` (bump VERSION constant + handle the migration) |
| Change the manifest scaffold | `mnemo/src/memory.rs` (`Memory::scaffold_manifest`) |

## CI

A GitHub Actions workflow at `.github/workflows/ci.yml` runs on every
push to `main` and every PR. Two jobs:

- `rust (ubuntu-latest | macos-latest | windows-latest)` — `cargo test`
  + `cargo clippy --all-targets -- -D warnings` against `mnemo/` on all
  three platforms in parallel.
- `python bindings (linux)` — `maturin build --release` + `pip install`
  the wheel + `python test_mnemo.py` against `mnemo-python/`.

`cargo fmt --check` is deliberately not yet wired (no codebase-wide
format pass has happened); if you do that pass, add the check.

## Format-version policy

The on-disk format is `mnemo/src/format.rs::VERSION` (currently 7).
`MIGRATABLE_FROM` is the lowest version this build can auto-upgrade on
open — files older than that are rejected with
`MnemoError::UnsupportedVersion`. Files in `[MIGRATABLE_FROM, VERSION)`
are migrated in place on first open and rewritten under `VERSION` on
the next flush.

Rules for any change that touches on-disk shape (catalog encoding, page
crypto, header layout):

1. **Bump `VERSION`** and document the change in the doc-comment history
   on `MIGRATABLE_FROM` (one bullet per bump, newest first).
2. **Add the migration path in `Mnemo::open`** so existing files upgrade
   transparently. Cascade — a v4 file may need to walk through v5, v6,
   ... to current.
3. **Preserve live data.** Past snapshots can be dropped (the migration
   policy does this uniformly; PITR into the pre-migration past is
   sacrificed for migration simplicity), but every live memory must
   survive the upgrade.
4. **Add a regression test** that exercises either the new format or the
   migration boundary.
5. **Update CHANGELOG.md** under `[Unreleased]` and note that the
   on-disk format moved.

If the change is breaking enough that auto-migration isn't viable, fail
fast with a clear error and document the manual upgrade path (e.g.
`compact_file` from the previous build, then open with the new one).

## Pull request expectations

- Run `cargo test` before pushing. The sandbox most agents work in cannot
  always run cargo; if you couldn't, say so in the PR description and CI
  will catch breakage at push time.
- Run `cargo clippy --all-targets -- -D warnings` — CI gates on it.
- Update the relevant README when adding a public API or CLI command.
- If a change affects the on-disk format or any agent-facing convention,
  add a memory to `test/scripts/seed.json` explaining the new behaviour
  so future agents pick it up via recall.
- Format-breaking changes must follow the "Format-version policy" above.

## Where the philosophy lives

The single-file invariant ("the file IS the agent's brain") is the design
center. New features should preserve it — anything that needs a sibling
file or external configuration is suspect. The recent self-describing
manifest pattern (`mnemo about`, `Memory::scaffold_manifest`) is the model
to follow: put metadata that agents need *inside* the `.mnemo` file, not
beside it.
