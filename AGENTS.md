# AGENTS.md

Orientation for any AI coding agent (Claude, Cursor, Copilot, Aider, etc.)
working on the Memory Nemo (MNemo) source code. **For agents using a
`.mnemo` file**, run `mnemo about <file>` instead — every `.mnemo` file
introduces itself. **If assigned a multi-agent role (`Planner`, `Worker`,
or `Reviewer`)**, jump to [Multi-agent roles & coordination](#multi-agent-roles--coordination)
before modifying any file or running git commands.

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
| Add an integration test | `mnemo/tests/integration.rs` (uses `KdfParams::fast()` for speed). **Vector tip:** `Mnemo::remember` refuses all-zero vectors since PR 10a (Phase 3) — in a loop like `for i in 0..N { … vec!(i as f32, 0., 0., 0.) }`, the first iteration ships `[0, 0, 0, 0]` and panics. Prefer `(i as f32) + 1.0` on the first component, or any other scheme that keeps the vector non-zero on every iteration. |
| Add a CLI smoke test | `mnemo/tests/cli_smoke.rs` (uses `CARGO_BIN_EXE_mnemo` + tempfile) |
| Add a fuzz target | `mnemo/fuzz/fuzz_targets/<name>.rs` + `[[bin]]` in `mnemo/fuzz/Cargo.toml` + expose entry via `mnemo::__fuzz` in `mnemo/src/lib.rs` + matrix rows in `.github/workflows/ci.yml` (`fuzz-smoke`) and `.github/workflows/fuzz-weekly.yml` |
| Add a proptest scenario | `mnemo/tests/proptest_store.rs` (add a new `proptest! { #[test] fn prop_<name>(...) { ... } }` block; use `Mnemo::create` + `KdfParams::fast()` + `tempfile` so cases stay ~ms). Run tests with `cargo test --features failpoints` so `mnemo::__failpoints` is visible across the integration-test crate boundary. For crash-injection variants, arm the pager hook via `mnemo::__failpoints::set_writes_until_fail(n)` then disarm with `-1` before reopen. |
| Add a licence / advisory exception | `mnemo/deny.toml` — `[licenses].exceptions` for a per-crate SPDX override, `[advisories].ignore` (with reason + expiration) for a time-boxed CVE grant, or `[bans].skip` for a duplicate-version pair we can't collapse. Run `cargo deny check` (from `mnemo/`) locally to verify before pushing. |
| Touch the on-disk format | `mnemo/src/format.rs` (bump VERSION constant + handle the migration) |
| Change the manifest scaffold | `mnemo/src/memory.rs` (`Memory::scaffold_manifest`) |
| Add a mutating method | must guard on `self.read_only` → `MnemoError::ReadOnly` (see `remember`, `flush`, `rekey` for the pattern) and be routed via `Mnemo::open` (not `open_read_only`) in the CLI |
| Add an MCP tool | `mnemo/src/mcp.rs` — extend the JSON schema in `tools_list()` and add a `tool_*` handler dispatched from `tools_call()`. Mutating tools must `db.flush()` before returning. |
| Touch the result cache | `mnemo/src/result_cache.rs` (types + on-disk shapes) and `mnemo/src/store.rs`'s cache API block. **Not `cache.rs`** — that's the page cache. Cache-mutating methods must gate on `self.read_only`, dirty `self.dirty_cache`, and call `self.cache_batch.record_dirty()` so the Batched flush policy fires correctly. |

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

The on-disk format is `mnemo/src/format.rs::VERSION` (currently 8).
`MIGRATABLE_FROM` is the lowest version this build can auto-upgrade on
open — files older than that are rejected with
`MnemoError::UnsupportedVersion`. Files in `[MIGRATABLE_FROM, VERSION)`
are migrated in place on first open and rewritten under `VERSION` on
the next flush.

**Standing MessagePack rule:** `rmp_serde::to_vec` encodes Rust structs as
positional MessagePack arrays (not named maps), and `rmp-serde` (1.3) rejects
an array with more elements than the struct expects (`LengthMismatch`).
`#[serde(default)]` on an appended field only lets *newer* builds read *older*
data; older builds still reject records written by the newer build. So never
add, remove or reorder fields of a persisted struct (`Memory`, `CatalogEntry`,
`IvfPqIndex`, `CacheEntry`, `CacheDirectoryEntry`, `Snapshot`, or any type
nested inside them) under the same `format::VERSION`. Either bump `VERSION`
with a migration, or store optional per-memory data in `Memory.metadata`
under reserved `_mnemo.*` keys, which older builds carry through untouched.

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

## Multi-agent roles & coordination

Solo contributors and unassigned agents follow the general codebase rules
above. When multiple LLMs collaborate in a shared working tree, the user
assigns each session one of three roles — **`Planner`**, **`Worker`**, or
**`Reviewer`** — and information flows through two local, gitignored
coordination files at the repository root:

- **`PLAN.md` (`Planner` <-> `Worker`):** Single source of truth for
  roadmap sequencing, architecture decisions, `Status`, `NEXT DISPATCH`,
  per-PR specs, and the append-only `WORKLOG` at the bottom.
- **`reviewer_notes.md` (`Reviewer` <-> `Planner`):** Independent audit and
  product advisory log containing a `Decision Ledger` table and append-only
  `Review Log` batches (`REV-001`, `REV-002`, ...) with stable item IDs
  (`R-001.1`, `R-001.2`, ...).

**Never `git add` or commit `PLAN.md` or `reviewer_notes.md`** (both are
gitignored). If either file is absent in a fresh workspace, only its primary
author (`Planner` for `PLAN.md`, `Reviewer` for `reviewer_notes.md`) creates
it; other roles stop and ask the user first.

| Role | Startup reading list | Allowed file writes | Git commits |
|---|---|---|---|
| `Planner` | `AGENTS.md`, `PLAN.md`, `reviewer_notes.md`, read-only `test/project.mnemo` | `PLAN.md` (all sections except Worker `WORKLOG` entries), triage fields in `reviewer_notes.md`, root `README.md`, `index.html`, `.gitignore`, and the protocol text in `AGENTS.md` (this section + the standing MessagePack rule) | **Never commits** — hands off tracked edits via `PLANNER — HANDOFF` in `PLAN.md` |
| `Worker` | `AGENTS.md`, `PLAN.md` (`Status`, `NEXT DISPATCH`, active spec, recent `WORKLOG`), `mnemo/README.md`, `CHANGELOG.md`, `test/project.mnemo` | Active PR code/tests/crate docs/manifests/`CHANGELOG.md`/`seed.json`, technical sections of `AGENTS.md` (as part of a PR), bottom `WORKLOG` of `PLAN.md` (append-only), `test/project.mnemo` | **Sole committer** — stages explicit paths only; commits `PLANNER — HANDOFF` files separately |
| `Reviewer` | `AGENTS.md`, live code & `git diff`, `test/scripts/perf_*.json`, `PLAN.md`, `reviewer_notes.md`, read-only `test/project.mnemo` | `reviewer_notes.md` **only** (`Decision Ledger` + `Review Log`) | **Never commits** (read-only across tracked files, `PLAN.md`, and `test/project.mnemo`) |

### `Planner` rules

1. **Own sequencing, specs, and repo-level docs:** Maintain `PLAN.md`
   (`Status`, `NEXT DISPATCH`, phase/PR specs) and log every dispatch,
   decision, review triage, handoff, or verification as a
   `PLANNER — PLAN | DECISION | REVIEW | HANDOFF | VERIFIED` entry in
   `PLAN.md`'s `WORKLOG`. Inspect `test/project.mnemo` read-only
   (`--no-track` / `read_only=True`) so you never contend for the write lock.
2. **Hand off tracked-file edits (never commit directly):** You own the
   content of root `README.md`, `index.html`, `.gitignore`, and the
   protocol text in `AGENTS.md` (this "Multi-agent roles & coordination"
   section and the standing MessagePack rule), plus any explicitly
   user-requested repo doc. Edit them in place, never run `git add` or
   `git commit`, and signal completion by appending a `PLANNER — HANDOFF`
   entry in `PLAN.md` listing the exact file path(s) and a suggested commit
   message for the `Worker`. Only edit `AGENTS.md` when it has no
   uncommitted Worker changes, and hand it off promptly, so the two sets of
   edits never share one uncommitted file.
3. **Triage `reviewer_notes.md` when asked:** Verify each `OPEN` item in
   the `Decision Ledger` against live code and `PLAN.md`, update its status
   in **both** the `Decision Ledger` row and the item's `**Planner Triage:**`
   line to `ACCEPTED` (citing target PR/Phase), `DEFERRED` (citing
   trigger/phase), `REJECTED` (with a 1-2 sentence technical/product
   rationale), or `NEEDS-USER`, and log a `PLANNER — REVIEW` entry in
   `PLAN.md` citing the triaged IDs whenever `PLAN.md` changes.

### `Worker` rules

1. **Execute one dispatched PR at a time:** Read `Status`, `NEXT DISPATCH`,
   and recent `WORKLOG` entries in `PLAN.md`. **Ignore `reviewer_notes.md`**
   — `AGENTS.md` (conventions) and `PLAN.md` (scope and sequencing) are your
   sole authority so untriaged review notes never derail an in-flight PR.
2. **Append-only `WORKLOG` & fail-stop discipline:** Never edit `PLAN.md`
   above `WORKLOG`. If a spec conflicts with live code or a gate fails
   (CI red, MSRV build failure, publish rejection), **STOP**, append a
   `WORKER — BLOCKED` entry with exact evidence, and end the session — do
   not improvise around the plan. After completing a PR, append a
   `WORKER — DONE` entry (commit hashes, test counts, deviations) and persist
   durable findings to `test/project.mnemo` if `test/.env` is present (note
   in `DONE` if skipped).
3. **Sole committer in the shared working tree:**
   - **Stage by explicit file path only** (`git add <path>...`). Never run
     `git add -A`, `git add .`, or `git commit -a`.
   - Never edit the content of Planner-owned files (root `README.md`,
     `index.html`, `.gitignore`, and the protocol text in `AGENTS.md`);
     request needed edits in your `DONE` or `BLOCKED` entry. You may still
     update `AGENTS.md`'s technical sections (build/test, task table,
     conventions) as part of a PR, **except** while `AGENTS.md` has changes
     you didn't make and no `PLANNER — HANDOFF` lists it. In that case leave
     the file alone until it's handed off and committed.
   - Leave modified Planner-owned files unstaged **unless** a
     `PLANNER — HANDOFF` entry lists them as ready. When handed off, commit
     the exact listed paths as **their own separate commit** (never mixed
     into a PR's code commit) and cite the hash in your next `DONE` entry.
   - During release PRs, bump versions in Cargo/Python manifests, lockfiles,
     and `CHANGELOG.md` (the `Planner` updates version text in root
     `README.md` and `index.html` via handoff).
   - Never `git reset`, `git rebase`, `git commit --amend`, or
     `git push --force` over commits you did not make. If `.git/index.lock`
     exists, wait and retry; check `git log origin/main..HEAD` before pushing.

### `Reviewer` rules

1. **Strictly read-only audit boundary:** Never edit tracked files,
   `PLAN.md`, or `test/project.mnemo` (always open `test/project.mnemo`
   with `read_only=True` or `--no-track`), and never run `git add` or
   `git commit`. Write exclusively to `reviewer_notes.md` unless the user
   explicitly asks for a doc edit and routes commit authority through a
   `Planner`/`Worker` handoff.
2. **Evidence-grounded findings:** Trace exact call paths and line numbers
   in live source code, uncommitted `git diff`s, `test/scripts/perf_*.json`,
   and `PLAN.md` before making quantitative or behavioral claims. Do not
   recommend already-shipped or already-queued work unless refining or
   challenging an active spec.
3. **Structured ledger & lifecycle (`OPEN` -> `ACCEPTED | DEFERRED | REJECTED | NEEDS-USER` -> `VERIFIED`):**
   Append batches (`REV-001`, `REV-002`, ...) with stable item IDs
   (`R-001.1`, ...), category (`SECURITY | CORRECTNESS | PERF | PLAN-REFINEMENT | PRODUCT | DX/DOCS`),
   and priority (`HIGH | MEDIUM | LOW | IDEA`), adding each item as `OPEN`
   to the `Decision Ledger`. Before every pass, read prior `Planner Triage`
   dispositions: never re-raise `REJECTED` items without new code or
   benchmark evidence, and mark shipped `ACCEPTED` items
   `VERIFIED (<commit/tag>)` once confirmed in code.

## Where the philosophy lives

The single-file invariant ("the file IS the agent's brain") is the design
center. New features should preserve it — anything that needs a sibling
file or external configuration is suspect. The recent self-describing
manifest pattern (`mnemo about`, `Memory::scaffold_manifest`) is the model
to follow: put metadata that agents need *inside* the `.mnemo` file, not
beside it.
