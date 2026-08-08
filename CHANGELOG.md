# Changelog

All notable changes to this project are recorded here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0, the minor component carries the breaking-change signal.

## [Unreleased]

### Fixed

- **Windows lock-conflict detection** — `open_with_lock` previously
  only mapped Unix `WouldBlock` errors to `MnemoError::Locked`.
  Windows `LockFileEx` returns `ERROR_LOCK_VIOLATION` (raw OS code 33)
  or `ERROR_SHARING_VIOLATION` (32), both surfaced by Rust as
  `ErrorKind::Uncategorized`. All Phase 1.1 lock-collision tests were
  panicking on Windows CI with `Io(...)` instead of `Locked(...)` as
  a result. Added a platform-conditional `is_lock_conflict` helper
  that recognizes both.
- **Windows raw-file-I/O in tests** — 7 tamper/WAL tests read
  (`std::fs::read` or the `read_bytes` helper) or wrote raw bytes
  while the Mnemo handle was still alive. On Unix `flock(2)` is
  advisory so plain reads are unaffected; on Windows `LockFileEx`
  is *mandatory* — any other open of a locked byte range fails with
  `ERROR_LOCK_VIOLATION`. Added `drop(db);` right after `close()`
  in `file_is_encrypted_at_rest`, `page_swap_attack_is_detected_by_aad`,
  `wal_crash_recovery_replays_committed_txn`, `wal_heals_torn_header`,
  `wal_discards_uncommitted_garbage`,
  `wal_region_grows_for_large_catalog`, and inside the closure of
  `fresh_file_uses_small_wal_by_default`. Removed the now-unreachable
  `drop(db)` before subsequent reopens in the same tests.
- **Python test `test_persistence_and_reopen` / `test_index_and_snapshots`
  / `test_wrong_passphrase`** — added `del db` after `close()` before
  reopens. Python's `close()` currently flushes but does not release
  the underlying Rust handle (and thus the OS lock); the Python
  binding needs a follow-up refactor to `Option<Core>` so `close()`
  drops the inner and users don't need the `del` incantation. Tracked
  as a follow-up.

### Added

- **OS file locking (Phase 1.1).** `Mnemo::create` and `Mnemo::open` now
  acquire an exclusive advisory lock on the target `.mnemo` file for the
  lifetime of the returned handle. A second concurrent open (from any
  process) returns the new `MnemoError::Locked { path }` instead of
  silently interleaving WAL frames and corrupting the file. Uses `fs4`
  (`flock` on Unix, `LockFileEx` on Windows); MSRV 1.75 preserved.
- **Read-only opens.** New `Mnemo::open_read_only(path, passphrase)`
  takes a shared OS lock. Multiple read-only handles coexist; a
  read-only handle does NOT coexist with a writer holding the exclusive
  lock (advisory locks via `flock`/`LockFileEx` refuse a shared-lock
  request while an exclusive lock is held, on both Unix and Windows).
  All mutating methods return the new `MnemoError::ReadOnly`; `recall`
  with default `track_access(true)` counts as a mutation and is refused
  (call with `track_access(false)` for read-only recall).
- **Read-only opens fail loud on pending on-disk work.** A read-only
  handle cannot replay a committed-but-uncheckpointed WAL transaction
  nor perform a format-version migration — both would require write
  access. Either case returns the new `MnemoError::NeedsWriteOpen`
  with a `reason`, telling the caller to open read-write once first.
  See the README's Durability section.
- **CLI `--no-track` on `recall`.** Opens the file via
  `Mnemo::open_read_only` with `RecallRequest::track_access(false)`, so
  the command can run alongside an agent process holding the exclusive
  write lock.
- **CLI read-only commands** (`info`, `about`, `list`, `get`, `search`,
  `verify`, `snapshots`, `recall --no-track`) now open via the shared
  lock and coexist with a writer.
- **Python `mnemo.open(..., read_only=True)`** mirrors
  `Mnemo::open_read_only`. Combining `read_only=True` with a
  non-existent path raises `ValueError` (read-only cannot create).

### Changed

- `Mnemo::close()` on a read-only handle is a no-op success (no dirty
  state to persist) rather than returning `ReadOnly` from the inner
  `flush()` call.

### Added (Phase 10.2: semantic cache)

- **`Mnemo::cache_put_semantic(ns, key, vector, value, opts)`** — same
  entry model as `cache_put` plus a required embedding vector and a
  required `model` string in [`SemanticCachePutOpts`]. Vector
  dimensionality is validated against the database's configured
  `dimensions` (same signal as `remember`). Vector is stored on the
  directory entry so lookup can score without decrypting record
  bodies.
- **`Mnemo::cache_get_semantic(ns, query_vector, threshold, model)
  -> Option<(CachedValue, similarity)>`** — top-1 cosine scan over
  live vectored entries in the namespace whose `model` matches the
  query; hits iff `sim >= threshold`. Exact-key entries (those
  without a vector) are transparently skipped. Different-model
  entries are invisible to the query, by design — a hit from a
  different embedder's cache is a bug, not a win.
- **`DEFAULT_SEMANTIC_THRESHOLD = 0.97`** — conservative default,
  model-dependent. Override per call.
- **No format bump.** `CacheEntry` and `CacheDirectoryEntry` gain
  `vector: Option<Vec<f32>>` and `model: Option<String>` fields with
  `#[serde(default)]`. Existing v8 cache entries (created between
  PR 3 and PR 4) decode unchanged as `vector = None, model = None`
  — the exact-key state. The extension rides in-place under v8.
- **Five new integration tests** covering hit above threshold, miss
  below, model-mismatch miss, dimension-mismatch rejection, and
  side-by-side coexistence of exact-key + semantic entries in one
  namespace (backwards-compat regression).
- **`memory::cosine`** promoted to `pub(crate)` so
  `cache_get_semantic` reuses the same scoring routine as `recall`
  — one canonical implementation, one behavior.

Deferred to PR 5 (Phase 10.4): MCP cache tools including
`cache_get_semantic`, Python `db.cache_get_semantic` mirror, and the
`@db.cached(embed=...)` decorator recipe.

### Added (Phase 10.1 + 10.3: exact-key result cache, format v8)

- **On-disk format bump v7 → v8.** Header gains three u64 fields
  (`cache_start`, `cache_pages`, `cache_len`) at bytes 270–293, after
  the v7 seal tag. The v7 header seal AAD is extended to cover them
  for v8+ files, so tampering with the cache pointer trips the seal
  just like every other mutable header field. Migration from v7 is
  trivial: pre-v8 bytes at those offsets are zero, which is the legal
  "empty cache directory" state; no data touched, no page rewrites.
  Follows the AGENTS.md format-version policy end to end.
- **`Mnemo::cache_put` / `cache_get` / `cache_delete` / `cache_purge` /
  `cache_stats`.** SHA-256-hashed keys, per-namespace budgets (default
  10,000 entries / 64 MiB), TTL support, catalog-only access-stat
  updates on hit (v5 recall-trick applied to the cache). `cache_get`
  on a read-only handle succeeds silently without bumping stats.
- **`CacheFlushPolicy::Strict` (default) or `Batched { max_dirty,
  max_age }`.** Batched mode auto-flushes when either threshold
  trips; the flush is a full WAL-committed transaction, so unflushed
  cache entries on a crash are misses, never corruption. **Memory
  writes are never batched** — only cache mutations get the relaxed
  lane. Set via `Mnemo::set_cache_flush_policy`.
- **`Mnemo::set_cache_budget(namespace, CacheBudget)`** for per-namespace
  eviction caps.
- **CLI `mnemo cache <get|put|delete|stats|purge> <file> ...`** — five
  subcommands under a nested-subcommand shape. `mnemo info` gains a
  cache summary line.
- **Nine integration tests** covering put/get roundtrip (json/text/
  bytes), TTL expiry, LRU budget eviction, stats counters,
  persistence across reopen, batched-policy auto-flush + crash
  behavior, read-only handle refusing mutations, and the v7→v8
  migration outcome (empty cache directory).

### Changed

- **`Mnemo::cache_stats` renamed to `page_cache_stats`.** Same rename
  in the Python binding. The `cache_stats` name now belongs to the
  result cache (Phase 10.1) — it takes an optional namespace and
  returns a `CacheStats` struct. Callers on the page-cache pair
  should update to `page_cache_stats` (breaking; documented here).
- **`IndexInfo` derives `Serialize`/`Deserialize`.** Was landed as
  part of PR 2's MCP `stats` tool; noting here for completeness.

### Added (Phase 4: MCP server)

- **`mnemo serve --mcp <file.mnemo>`** — Model Context Protocol server
  over stdio, so any MCP-compatible agent can drive a `.mnemo` file
  as a tool. Tools: `about`, `remember`, `recall`, `forget`, `list`,
  `snapshot_list`, `stats`. Every mutation flushes before returning.
  Passphrase via `MNEMO_PASSPHRASE` only (no CLI-flag fallback).
- **Hand-rolled** rather than depending on `rmcp`. The official SDK
  declares `edition = "2024"` on its workspace, requiring Rust 1.85+;
  our MSRV is 1.75 and locked (`rmp` 0.8.14 and `base64ct` 1.6.0
  transitively pin us). Hand-rolling stdio JSON-RPC framing is ~250
  LoC and adds zero deps beyond `serde_json` (already present). Lives
  in `mnemo/src/mcp.rs`; wired into the CLI via the new `Serve`
  subcommand.
- **README's "Serve as an MCP server" block** near the top with a
  ready-to-paste Claude Desktop `mcpServers` config snippet.
- **Smoke tests** in `cli_smoke.rs` that spawn `mnemo serve --mcp`
  as a subprocess and drive `initialize` / `tools/list` /
  `tools/call` over piped stdin/stdout.
- **`remember` and `recall` are embedder-agnostic** — the caller
  supplies vectors on both. Text-only variants that call an embedder
  land with Phase 3.

## [0.3.2] — 2026-06-22

Distribution rename: `mnemo-db` → `mnemo-engine` on both PyPI and
crates.io. No code or on-disk format changes; v0.3.1 files open
unchanged.

### Why the rename

When v0.3.1 attempted its first automated PyPI publish, the upload was
rejected with HTTP 403: `mnemo-db` on PyPI is a different project
([sattyamjjain/mnemo](https://github.com/sattyamjjain/mnemo), an
MCP-native agent-memory layer built on DuckDB + USearch HNSW +
Tantivy). Independent project, same naming logic — they reached for
`-db` for the same reason we did (the bare `mnemo` is held by an
unrelated 2020-era notebook helper) and they shipped first.

We picked `mnemo-engine` over close alternatives (`mnemodb`,
`mnemo-store`) because (a) the README's own tagline already calls
MNemo an "agent-memory engine"; (b) it reads as a structurally
different category from `mnemo-db`, reducing search-confusion with
the other project; (c) the name is free on every registry we ship to
(crates.io, PyPI).

### Changed

- **`mnemo/Cargo.toml`** — package `name` is now `mnemo-engine`;
  `documentation` URL is `https://docs.rs/mnemo-engine`. Library and
  binary names stay `mnemo`, so `use mnemo::...` and the `mnemo` CLI
  are unaffected.
- **`mnemo-python/pyproject.toml`** — PyPI distribution name is
  `mnemo-engine`. Import name stays `mnemo`. Users move from
  `pip install mnemo-db` (which never resolved for this project) to
  `pip install mnemo-engine`.
- **`mnemo-python/Cargo.toml`** and **`mnemo/bindings/node/Cargo.toml`**
  — `mnemo_core` / `mnemo` path-deps now reference
  `package = "mnemo-engine"` to match the core crate's new name.
- **`AGENTS.md`**, **`mnemo-python/README.md`**, **`index.html`**,
  **`test/scripts/seed.json`** — install commands and the
  distribution-name explanation updated to `mnemo-engine`.

### Notes for downstream users

- `cargo add mnemo-db` and `pip install mnemo-db` will no longer
  resolve to this project. Switch to `cargo add mnemo-engine` /
  `pip install mnemo-engine`.
- The failed v0.3.1 git tag and GitHub Release are retained as a
  historical record of the collision; no PyPI/crates.io artifact
  was ever published under v0.3.1.

## [0.3.1] — 2026-06-15

First *attempted* automated publish to PyPI. The `publish-pypi` job
in `release.yml` was reached for the first time after the maintainer
added `ENABLE_PYPI_PUBLISH=true` and `PYPI_API_TOKEN` to the GitHub
repo. The upload failed with HTTP 403: the `mnemo-db` name on PyPI is
owned by a different project (see the v0.3.2 entry above for the full
rationale). No artifact was published. The git tag and GitHub Release
remain as a historical record. The rename happens in v0.3.2.

The originally-intended changelog text was: "After this lands,
`pip install mnemo-db` resolves to a published wheel on PyPI for the
first time. The CLI binaries continue to ship as GitHub Release
attachments." — that did not happen; the CLI binaries did attach to
the GitHub Release, but no wheel reached PyPI.

## [0.3.0] — 2026-06-15

This release is the consolidation of the Phase 3 (CLI / UX) and Phase 4
(release engineering) work that accumulated against `main` after v0.2.0.
No on-disk format change — v0.2.x files open transparently and continue
to use v7.

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

[Unreleased]: https://github.com/toarchkumar/mnemo/compare/v0.3.1...HEAD
[0.3.1]: https://github.com/toarchkumar/mnemo/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/toarchkumar/mnemo/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/toarchkumar/mnemo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/toarchkumar/mnemo/releases/tag/v0.1.0
