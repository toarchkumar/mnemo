# Changelog

All notable changes to this project are recorded here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0, the minor component carries the breaking-change signal.

## [Unreleased]

### Added

- **`release.yml` GitHub Actions workflow** that fires on `v*` tag
  pushes. Builds the `mnemo` CLI binary for x86_64 / aarch64 Linux,
  x86_64 / aarch64 macOS, and x86_64 Windows; builds matching Python
  wheels via `maturin-action`. All artifacts attach to the auto-created
  GitHub Release. PyPI publishing is opt-in via an `ENABLE_PYPI_PUBLISH`
  repo variable + a `PYPI_API_TOKEN` secret, so first releases produce
  just the GitHub-attached assets until the maintainer is ready to
  automate uploads.
- **MSRV check in CI** — `ci.yml` gained a Linux-only Rust 1.75 job that
  builds the crate and runs the integration + CLI smoke tests, catching
  accidental drift past the declared MSRV.
- **`cargo publish` readiness on `mnemo-db`.** `mnemo/Cargo.toml` gained
  `homepage`, `documentation`, `keywords`, and `categories`, completing
  the required metadata for crates.io. `mnemo-python/Cargo.toml` and
  `mnemo/bindings/node/Cargo.toml` are marked `publish = false` —
  they're built by maturin / napi-rs into their own distribution
  artifacts (PyPI wheel, npm package) and aren't meant for crates.io.

### Changed

- CLI exploration (Phase 3.2): `search` and `recall` accept `--query-file
  <path|->` as an alternative to inline `--query`. File contents
  auto-detected as JSON array, comma-separated, or whitespace-separated
  floats. Makes high-dimensional queries usable from the CLI.
- Python binding (Phase 3.4): `db.recall(..., track_access=True)` exposes
  the read-only-recall flag; new `db.set_max_snapshots(max)` mirrors the
  Rust API.
- CLI (Phase 3.1): passphrase resolution adds a TTY prompt fallback
  (`rpassword`). `init` and `rekey` double-prompt and verify a match.
- Storage (Phase 2.3): `MnemoConfig::max_snapshots` (default 256) caps
  the snapshot manifest; pruning happens at flush time.

### Removed

- The in-tree `mnemo/bindings/python` crate. It was a strict subset of
  the published `mnemo-python/` crate (13 methods vs 25+, no unique
  features). The workspace member list drops it; the standalone crate
  is now the single source of truth for the Python bindings.


## [0.2.0] — 2026-06-13

This release tightens the security model, drops file-size and recall-cost
overhead for small and read-heavy workloads, and makes the surface
agent-friendly enough that an AI agent can be productive against a `.mnemo`
file in one command. The on-disk format moves from **v4 to v7** in three
migration steps; every step is automatic on the next `Mnemo::open`. Pre-0.2
files are upgraded in place — no data loss — but snapshots written by older
builds are dropped during migration (point-in-time recovery into the
pre-migration past is sacrificed for migration simplicity; live data is
preserved).

### Security

- **Closed the AES-GCM nonce-reuse window across crashed flushes** (Phase 1.1
  of the improvement plan). `Mnemo::flush` previously fsynced encrypted data
  pages with bumped `write_counter` values *before* committing the WAL,
  leaving a crash window where the on-disk header still recorded the old
  counter. On reopen, the next flush re-used the same `(page_no,
  write_counter)` nonce on different plaintext under the same DEK — a
  keystream-XOR leak and authentication forgery surface. Fixed by leasing
  counter and page slots in a new `prepare_for_flush` prelude that persists a
  clone of the header with bumped values *before* any encrypted page hits the
  disk. One extra header write + fsync per flush; no format change.
- **Bound page numbers as AES-GCM AAD on every page encrypt/decrypt** (v6,
  Phase 1.2A). An attacker with file-write access can no longer transplant a
  valid encrypted page to a different home slot — the GCM tag refuses to
  decrypt at the wrong page_no. The v5→v6 migration re-encrypts every live
  record page in place under the new AAD.
- **AEAD-sealed the mutable header tail under the DEK** (v7, Phase 1.2B).
  Pre-v7 the only integrity check on `catalog_start`, `next_page`,
  `write_counter`, and friends was an unkeyed CRC-32 that an attacker could
  trivially recompute. v7 appends a small AES-GCM seal whose AAD covers
  every mutable field; rewriting any of them invalidates the GCM tag and
  open errors with `MnemoError::HeaderTampered` instead of silently loading
  stale state. The seal does not prevent rollback to a previous *valid*
  sealed state (replaying an old header byte-block); catching that needs
  monotonic counters tracked outside the file.

### Performance

- **`Mnemo::recall` no longer rewrites full records on access-stat updates**
  (Phase 2.1). `accessed_at` and `access_count` moved from the `Memory`
  record body into `CatalogEntry`. Pre-v5, a top-K recall called
  `self.put(m.clone())` per result, rewriting the full record (vector
  included) to fresh pages — a top-10 recall did roughly
  K × ~vector-size of churn per flush. v5 makes recall an in-place catalog
  mutation: one catalog page rewrite per flush regardless of K. The values
  on `Memory` are still populated (from the catalog) for API compatibility.
- **Default initial WAL right-sized from 64 pages (512 KiB) to 8 pages
  (64 KiB).** A freshly-initialised file with scaffold manifest now occupies
  about 96 KiB on disk — down from ~544 KiB on the v0.1.0 default. The WAL
  auto-grows beyond the initial reservation, so this is a hint about
  expected per-transaction size, not a cap. Configurable via the new
  `MnemoConfig::wal_pages_initial`.

### Added

#### Library

- `Mnemo::about()` — returns the database's self-describing onboarding
  memories (those tagged `metadata.area = "onboarding"`), with the
  canonical manifest entry (tagged `metadata.topic = "manifest"`) hoisted
  to the top regardless of importance. Engine-level entry point for any
  agent to learn what a `.mnemo` file is, which embedder it expects, and
  any other conventions the file's author chose to record — all without
  needing external documentation.
- `Memory::scaffold_manifest(dimensions)` — canonical placeholder manifest
  for a fresh database. Inserted automatically by `mnemo init` so every
  new file is self-describing from creation.
- `RecallRequest::track_access(bool)` — opt out of access-stat updates for
  fully read-only recall. Useful for batch scoring, dry-runs, and
  introspection tooling that shouldn't perturb the database.
- `RecallRequest::metric(Metric)` and `RecallRequest::weights(ScoreWeights)`
  builder methods, for symmetry with the existing `.top_k()`/`.types()`/
  `.agent()`/`.n_probe()`/`.n_rerank()` builders.
- `MnemoConfig::wal_pages_initial: u64` — initial WAL region size in 8 KiB
  pages. Defaults to 8; clamps to `MIN_WAL_PAGES` (2).
- `MnemoError::HeaderTampered` variant for v7 header-seal authentication
  failures.

#### CLI

- `mnemo about <path>` — self-describing briefing for a database. Prints a
  stats header, every onboarding memory sorted with the manifest first,
  and a quick-start footer. Supports `--format table|json|jsonl` and
  `--manifest-only`.
- `mnemo list <path>` — browse live memories with `--type`, `--agent`,
  `--limit`, `--offset`, `--sort created|importance|id`, `--vector`, and
  `--format table|json|jsonl`.
- `mnemo get <path> <ulid>` — fetch one memory by ULID. `--verbose`,
  `--vector`, `--format table|json`.
- `mnemo recall <path> --query VEC` — multi-signal ranked retrieval from
  the CLI (was library-only). `--metric`, `--n-probe`, `--n-rerank`,
  type and agent filters.
- `mnemo init` now auto-inserts a scaffold manifest so brand-new databases
  are self-describing from creation. `--no-manifest` opts out for an
  entirely empty file.
- `mnemo about` tags scaffold manifests as `(scaffold — please replace)`
  in table output so an agent immediately knows it's looking at a
  placeholder.

#### Python bindings

- `db.about()` — Python counterpart to `Mnemo::about()`.
- `db.insert_default_manifest()` — Python counterpart to the CLI's scaffold
  manifest insertion.

#### Documentation and conventions

- `AGENTS.md` at the repo root — tool-agnostic orientation for AI coding
  agents working on the codebase (build commands, conventions, codebase
  layout, dogfood workflow). Different audience from `mnemo about <file>`
  which orients agents *using* a `.mnemo` file.
- "For AI agents" sections in the root, `mnemo/`, and `mnemo-python/`
  READMEs covering the two-command quickstart.
- "Agentic-first" section on the landing page (`index.html`) with a sample
  `mnemo about` terminal output.
- "Self-describing databases" section in `mnemo/README.md` documenting the
  `metadata.area = "onboarding"` convention.
- "Sizing tips" section in `mnemo/README.md` covering WAL right-sizing,
  dimensions-as-a-knob, and Matryoshka (MRL) truncation as the three
  composable size levers.
- `test/scripts/project_memory.py perf <file.json>` workflow for ingesting
  structured performance measurements as episodic memories tagged
  `metadata.area = "performance"`. Schema documented in
  `test/scripts/perf_v0.1.0.json`.

#### Tooling

- GitHub Actions CI (`.github/workflows/ci.yml`) — `cargo test` + `cargo
  clippy --all-targets -- -D warnings` on Linux/macOS/Windows for the Rust
  core, plus `maturin build` + Python tests for the bindings on Linux.

### Changed

- **Distribution renamed from `mnemo` to `mnemo-db`** on both PyPI and
  crates.io. The bare `mnemo` name was already taken in both ecosystems
  (PyPI: an unrelated 2020 notebook assistant; crates.io: `aayushadhikari7/
  mnemo`). The *library* and *import* names stay `mnemo` end-to-end:
  `pip install mnemo-db; import mnemo` works, `cargo add mnemo-db; use
  mnemo::...` works, and the CLI binary is still invoked as `mnemo`.
- `pre_v5_snapshot_manifest` is cleared during migration. PITR is preserved
  forward from the post-migration first flush onward.

### Internal

- `Pager` is now format-version aware. `Pager::new` takes a `version`
  argument; `Pager::set_version` enables mid-flight migration switches.
  `page_aad` returns `page_no.to_le_bytes()` for v6+ and empty for v4/v5.
- Two new private structs in `mnemo/src/store.rs`: `FlushPrelude` (carrying
  pre-flush serialized control plane through the lease) and `CatalogEntryV4`
  (frozen v4 catalog shape used only by the migration path).

---

## [0.1.0] — 2026-05-25

Initial public commit. Encrypted single-file storage, AES-256-GCM
page-level encryption with Argon2id key derivation, write-ahead log with
single-fsync commit, snapshot manifest with point-in-time recovery,
bounded LRU page cache, IVF+PQ approximate-nearest-neighbour index, the
four agent memory types (Episodic, Semantic, Procedural, Working),
multi-signal recall, and the `Session` conversation wrapper. Python
bindings via PyO3 (released as `mnemo` on PyPI; renamed in 0.2.0). CLI
binary with `init`, `info`, `import`, `index`, `search`, `verify`,
`rekey`, `compact`, `snapshots`, `restore`, `demo` subcommands.

[Unreleased]: https://github.com/toarchkumar/mnemo/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/toarchkumar/mnemo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/toarchkumar/mnemo/releases/tag/v0.1.0
