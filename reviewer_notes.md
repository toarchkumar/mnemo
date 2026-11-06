# reviewer_notes.md — Independent Review & Product Advisory Log

> **Coordination file (Reviewer ↔ Planner), tracked in git as a backup (since 2026-11-06). Only the `Worker` commits this file (`chore(plan): sync coordination files`).**
> **The repository is public: unfixed security findings go in a gitignored `*.local.md` file (e.g. `SECURITY.local.md`), not here, until the fix ships.**

## Protocol

Three LLM roles collaborate on this repository, with strict file boundaries so active execution is never disrupted:

1. **Reviewer** (independent read-only auditor & product advisor):
   - **Never edits** tracked code, `PLAN.md`, or `test/project.mnemo` (inspects `test/project.mnemo` strictly with `read_only=True` / `--no-track`), and **never runs `git add` or `git commit`**. Writes exclusively to `reviewer_notes.md` (except unfixed security findings, which go in a gitignored `*.local.md` file until the fix ships).
   - Grounds every finding in live source code, uncommitted working-tree diffs, `test/scripts/perf_*.json`, and `PLAN.md` (no stale-doc recommendations for already-shipped or already-queued work unless refining or challenging an active spec).
   - Appends review batches (`REV-001`, `REV-002`, …) with stable item IDs (`R-001.1`, `R-001.2`, …), adds each item to the **Decision Ledger** below with status `OPEN`, and leaves implementation scheduling to the Planner.
   - Before each new review pass, reads prior Planner dispositions: never re-raises `REJECTED` items unless new code/benchmark evidence invalidates the rationale, and marks shipped `ACCEPTED` items `VERIFIED (<commit/tag>)` once confirmed in code.

2. **Planner** (architecture & dispatch owner of `PLAN.md`):
   - Scans the **Decision Ledger** for items marked `OPEN` whenever asked by the user (e.g., *"Triage open items in reviewer_notes.md."*).
   - Updates each `OPEN` item's **Status** in both the **Decision Ledger** table and the item's `**Planner Triage:**` line to one of:
     - `ACCEPTED` — folded into `PLAN.md` (cite target PR / Phase, e.g. `PR ANN-1.1`, `v0.6.0`, `Phase 12.4`).
     - `DEFERRED` — valid, parked for later (cite trigger/condition or future Phase).
     - `REJECTED` — not adopting (give a 1–2 sentence technical/product reason so it stays settled).
     - `NEEDS-USER` — trade-off surfaced to the maintainer for a decision.
   - Logs a brief `REVIEW` entry in `PLAN.md`'s `WORKLOG` referencing the item IDs (e.g. `Triaged REV-001: accepted R-001.1 -> PR ..., rejected R-001.X`) if it changes `PLAN.md`.

3. **Worker** (PR executor & sole committer):
   - **Ignores the content of this file** during PR execution — `PLAN.md` and `AGENTS.md` remain the sole authoritative execution instructions for the Worker, ensuring untriaged review notes never derail an in-flight PR. As sole committer, the Worker commits `PLAN.md` and `reviewer_notes.md` by explicit path in a separate `chore(plan): sync coordination files` commit after each `DONE` entry and before tagging a release.

### Status Lifecycle

```
OPEN ──► ACCEPTED (Planner schedules in PLAN.md) ──► VERIFIED (Reviewer confirms in shipped code)
  ├────► DEFERRED (Planner parks with trigger/phase)
  ├────► NEEDS-USER (Planner flags for maintainer call)
  └────► REJECTED (Planner records rationale; closed)
```

### Item Template

```markdown
#### R-XXX.Y — [<CATEGORY>] [<SEVERITY>] <Concise title>
- **Status:** `OPEN`
- **Context / Evidence:** `<file:line>`, commit `<sha>` / working tree, relation to `PLAN.md` section.
- **Finding:** What is wrong, risky, suboptimal, or missing.
- **Recommendation:** Concrete fix, spec refinement, or product direction.
- **Planner Triage:** _(pending)_
```

Categories: `SECURITY` · `CORRECTNESS` · `PERF` · `PLAN-REFINEMENT` · `PRODUCT` · `DX/DOCS`
Severity / Priority: `HIGH` · `MEDIUM` · `LOW` · `IDEA`

---

## Decision Ledger

| ID | Date | Category | Pri | Title | Status | Disposition / Target (`PLAN.md`) |
|---|---|---|---|---|---|---|
| `R-001.1` | 2026-10-31 | `CORRECTNESS` | `HIGH` | `Mnemo::compact_file` silently destroys the entire Result Cache (`cache_dir` + entries) | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `v0.6.0`) |
| `R-001.2` | 2026-10-31 | `CORRECTNESS` | `HIGH` | Torn-header WAL recovery replays ancient page-1 WAL after relocation + unlogged page-0 writes | `VERIFIED` | Shipped in PR DUR-1 (`4f1edc1`, `v0.6.0`) |
| `R-001.3` | 2026-10-31 | `CORRECTNESS` | `HIGH` | In-flight `PR ANN-1.1` auto-defaults bypassed by CLI `mnemo index`, lost on `rebuild_index`/`compact_file`, stale in `IndexInfo` | `VERIFIED` | Shipped in PR ANN-1.1 (`190a407`, `v0.6.0`) |
| `R-001.4` | 2026-10-31 | `CORRECTNESS` | `MEDIUM` | CLI `mnemo recall` (default tracking mode) takes exclusive lock but drops without flushing stats or manifest | `ACCEPTED` | Recall scoring v2 (CLI follows the tracking decision) |
| `R-001.5` | 2026-10-31 | `CORRECTNESS` | `MEDIUM` | Windows mandatory-lock failure in Python `export_encrypted` & `rekey(fast=False)` clobbering custom KDF params | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `93e2620`, `v0.6.0`) |
| `R-001.6` | 2026-10-31 | `CORRECTNESS` | `MEDIUM` | `EmbedCmd::embed` omits zero-vector protocol check on `recall` and `cache_get_semantic` | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `v0.6.0`) |
| `R-001.7` | 2026-10-31 | `PERF` | `HIGH` | `SEC-1` made pre-flush page-0 nonce-lease `sync_all` obsolete: cut `flush()` from 4 fsyncs to 2 (~35–50% faster writes) | `ACCEPTED` | Lease-write removal `VERIFIED` in PR DUR-1 (`4f1edc1`, `v0.6.0`); fsync merge -> Phase 11.1 |
| `R-001.8` | 2026-10-31 | `PERF` | `HIGH` | `Mnemo::stats()` is `O(N)` full-DB decrypt; cache/access flushes evict 256-slot PITR snapshots; `O(M log M)` sort in ANN search | `VERIFIED` | Part 3 in ANN-1.1 (`190a407`); Parts 1-2 in PR FIX-1 (`8e09e60`, `v0.6.0`) |
| `R-001.9` | 2026-10-31 | `PERF` | `MEDIUM` | ANN 1M-tier `n_probe = 50%` scaling ceiling & Residual IVF-PQ (`IVF-RPQ`) opportunity in format v9 | `DEFERRED` | Start of Phase 2 + 11.2 (v9) evaluation |
| `R-001.10` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | Positional MessagePack (`rmp_serde::to_vec`) forward-compatibility hazard on `format::VERSION = 8` | `VERIFIED` | Standing rule in PLAN.md + `V052IvfPqIndex` field-count guard in ANN-1.1 |
| `R-001.11` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | `PR PY-1` (`py.allow_threads` + `Option<Core>`): avoid `PyFnEmbedder` re-entrant borrow panic & cover `Session` | `VERIFIED` | Shipped in PR PY-1 (`abfaff3`, `v0.6.0`) |
| `R-001.12` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | `Recall scoring v2`: similarity-gated boosts, `created_at` vs `accessed_at` recency split, and `ranking_harness.py` timestamp variance | `ACCEPTED` | Recall scoring v2 (V2 selected 2026-11-06 -> `v0.7.0`) |
| `R-001.13` | 2026-10-31 | `PLAN-REFINEMENT` | `MEDIUM` | `Phase 12.4` Shared Access (`--shared`): `try_lock` contention & `compact_file` `rename` hazards on Windows/Unix | `ACCEPTED` | Phase 12.4 design-note requirements |
| `R-001.14` | 2026-10-31 | `PRODUCT` | `HIGH` | Pre-top-k structured metadata & `session_id` predicate filtering in `RecallRequest`, CLI, Python, and MCP | `ACCEPTED` | NEW Phase 12.6 filtered recall |
| `R-001.15` | 2026-10-31 | `PRODUCT` | `MEDIUM` | Token-budgeted context packing & Maximal Marginal Relevance (MMR) diversity in `recall` | `DEFERRED` | Phase 7.5 (harness-evaluated) |
| `R-001.16` | 2026-10-31 | `PRODUCT` | `MEDIUM` | Cognitive consolidation on `Session::close` & transactional brain diffing (`mnemo diff <txn_a> [txn_b]`) | `ACCEPTED` | diff -> Phase 12.5; consolidation -> Phase 7.4; Session text methods -> Phase 12.1 |
| `R-001.17` | 2026-10-31 | `PRODUCT` | `IDEA` | Portable encrypted Knowledge Packs: read-only federated mounts (`attach_pack`) & scoped encrypted fork/merge | `DEFERRED` | After Phase 12.4 + v9; trigger: real multi-file use |
| `R-002.1` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | `PR DUR-1` recovery design: 3 crash/truncation windows (relocation zero-before-write gap, 64-page scan cap, torn bytes outside `0..238` CRC) | `VERIFIED` | Shipped in PR DUR-1 (`4f1edc1`, `6bb5a84`, `22b4c1e`, `v0.6.0`) |
| `R-002.2` | 2026-10-31 | `PLAN-REFINEMENT` | `MEDIUM` | `PR FIX-1` details: sync directory `accessed_at`/`access_count` on cache compaction (+ read-only `cache_get` bug) & guard `export_encrypted` self-truncation | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `93e2620`, `v0.6.0`) |
| `R-003.1` | 2026-11-06 | `PRODUCT` | `HIGH` | First-class `Essence` object & seamless conversation compaction (`Session::compact` / MCP `compact_conversation` / `mnemo essence`) | `OPEN` | _(pending)_ |

---

## Review Log (append-only batches; newest batch LAST; update `Status` & `Planner Triage` in place)

### Batch `REV-001` — 2026-10-31 (Baseline Codebase Audit @ `69752d0` / `v0.5.2` + In-Flight `PR ANN-1.1` + Full `PLAN.md` Review)

**Baseline Health Summary:**
- **Core strengths:** `#![forbid(unsafe_code)]`, pinned dependencies (`=x.y.z`), `cargo-deny`, 5 `cargo-fuzz` targets, and crash-injected `proptest` state-machine testing (`__failpoints`) put MNemo's storage rigor well ahead of typical v0.5 agent-memory engines. The single-file encrypted architecture, self-describing onboarding manifest (`mnemo about`), embedder-identity guardrail (`EmbedderIdentity`), zero-daemon stdio MCP server, and rapid `SEC-1` (`v0.5.2`) response are all strong.
- **Strategic roadmap alignment:** The Planner's `PLAN.md` sequencing — fixing ANN recall@10 defaults, Python handle/thread safety, and Recall scoring v2 in `v0.6.0`, followed by Phase 12 lifecycle hygiene and combining Phase 2 (BM25 + RRF hybrid) with Phase 11.2 (inline hot catalog fields) into a single format-v9 bump (`v0.7.0`) before TS/WASM bindings and competitor benchmarks — is well-ordered ("correct answers before faster answers before external benchmarks").
- **Focus of `REV-001`:** 6 shipped/in-flight correctness & durability bugs (`R-001.1`–`R-001.6`), 3 high-leverage engine performance improvements (`R-001.7`–`R-001.9`), 4 concrete spec refinements to queued PRs/Phases (`R-001.10`–`R-001.13`), and 4 product design directions aligned with the single-file brain philosophy (`R-001.14`–`R-001.17`).

---

#### R-001.1 — [CORRECTNESS] [HIGH] `Mnemo::compact_file` silently destroys the entire Result Cache (`cache_dir` + entries)
- **Status:** `VERIFIED (PR FIX-1 @ 8e09e60, v0.6.0)`
- **Context / Evidence:** `mnemo/src/store.rs:2699-2742` (`Mnemo::compact_file`), `mnemo/src/store.rs:2018, 2172`, `mnemo-python/src/lib.rs:795`, `mnemo/src/bin/mnemo.rs:431, 1523`, `README.md` (`v0.5.2` security advisory).
- **Finding:** `Mnemo::compact_file` opens `old`, creates `new`, copies live `old.memories()` (`store.rs:2716-2725`), and rebuilds the ANN index (`store.rs:2727-2735`), then atomically `rename`s `tmp` over `path`. It **never copies `old.cache_dir` or any `CacheEntry` (neither `Exact` nor `Semantic`)**. This causes silent 100% cache data loss whenever `compact_file` / `mnemo compact` runs. Worse: (1) `cache_delete` / `cache_purge` docs explicitly instruct users to run `compact_file` / `mnemo compact` to reclaim tombstoned cache pages, and (2) the `v0.5.2` security advisory instructs users to run `mnemo compact` to rotate the DEK on copied files. Additionally, `Mnemo::verify` (`store.rs:2659-2673`) only decrypts/verifies `self.catalog` records and skips `self.cache_dir` records.
- **Recommendation:**
  1. In `Mnemo::compact_file`, iterate live, non-tombstoned, non-expired `old.cache_dir` entries, read/decode each `CacheEntry`, write its encrypted pages into `new`, and append the remapped `CacheDirEntry` (preserving `created_at`, `accessed_at`, `access_count`, `ttl_secs`, `kind`, and `content_type`) with `new.dirty_cache = true` before `new.flush()`. (Consider reporting cache before/after counts on `CompactReport` or keeping `CompactReport` backwards-compatible.)
  2. Extend `Mnemo::verify` to also decrypt and MessagePack-decode every live `cache_dir` entry so corrupt cache pages are caught by `mnemo verify`.
  3. Add an integration test verifying exact + semantic cache entries (including metadata/access counts) survive `compact_file` while tombstoned/expired cache entries are dropped.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → PR FIX-1. Verified: `compact_file` copies only `old.memories()` into the new file and drops every cache entry; `verify()` checks only the memory catalog. FIX-1 copies live exact+semantic cache entries (with their metadata), reports cache counts and extends `verify()`. The planner added an interim caveat to the root README's v0.5.2 advisory, since it tells users to run `mnemo compact`.

#### R-001.2 — [CORRECTNESS] [HIGH] Torn-header WAL recovery replays ancient page-1 WAL after relocation + unlogged page-0 writes
- **Status:** `VERIFIED (PR DUR-1 @ 4f1edc1, v0.6.0)`
- **Context / Evidence:** `mnemo/src/store.rs:73, 729-750, 1650-1670, 1939-1945, 2653`, `mnemo/src/wal.rs:146-210`.
- **Finding:** Two interacting page-0 durability hazards exist:
  1. **Stale WAL replay after WAL relocation:** When `Header::from_page` returns `Err(MnemoError::HeaderChecksum)` (a torn page-0 write), `Mnemo::open` (`store.rs:736`) calls `wal::recover(&mut file, 1, TORN_HEADER_SCAN_PAGES, 0)` — hardcoding `wal_start = 1` and `wal_seq = 0`. Since `DEFAULT_WAL_PAGES` was lowered to `8` (64 KiB), any DB whose control plane (catalog + ANN index + cache dir + manifest) grows past ~56 KiB triggers `ensure_wal_capacity` (`store.rs:1650-1670`), which relocates `wal_start` to `header.next_page` at the file tail **without clearing the old WAL frames at pages `1..9`**. Pages `1..9` still retain a valid `MWAL` frame + `KIND_COMMIT` CRC from early in the file's life (`txn_id >= 1 > 0`). If page 0 is ever torn later, `Mnemo::open` scans page 1, sees a valid committed txn with `id > 0`, and **replays the ancient page-0 header and catalog**, silently rolling back and corrupting the database.
  2. **Unlogged direct overwrites of page 0:** `ensure_wal_capacity` (`store.rs:1667`), `prepare_for_flush` (`store.rs:1944`), and `rekey` (`store.rs:2653`) write directly to page 0 via `write_raw(0, &page)` *without* logging the new page-0 image in the WAL first — creating actual crash windows where page 0 can tear with no matching WAL commit frame anywhere in the file.
- **Recommendation:**
  1. Whenever `ensure_wal_capacity` relocates `wal_start`, explicitly zero/clear the old `wal_start` header page (`wal::clear`) and `sync_all()` so stale `MWAL` magic bytes can never remain at page 1 (or any prior relocated WAL region).
  2. Route `rekey` (and header updates) through a WAL-logged commit before checkpointing page 0, and remove the unlogged `prepare_for_flush` page-0 write (see `R-001.7`).
  3. For format v9 (or even within v8 if torn recovery scans for a relocated `MWAL` magic when page 1 is cleared), consider either a shadow/ping-pong header on pages 0 & 1 (eliminating blind torn-header scanning entirely) or scanning candidate WAL regions for the highest valid `txn_id` whose `MWAL` commit CRC verifies. Add a `--features failpoints` crash test covering torn page 0 *after* WAL relocation.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → NEW PR DUR-1. Verified: the torn-header heal always scans from page 1 with wal_seq=0 (store.rs ~L736); relocation never invalidates the old region (no `wal::clear` exists), so a torn page 0 after relocation replays a stale transaction. DUR-1: recover from the highest valid committed txn across candidate WAL regions, invalidate the old region on relocation, remove the obsolete pre-flush page-0 lease write, add failpoint crash tests. Release timing settled by the user 2026-11-01: ships in v0.6.0 with the other fixes (no v0.5.3 hotfix); scoring v2 moves to its own v0.7.0.

#### R-001.3 — [CORRECTNESS] [HIGH] In-flight `PR ANN-1.1` auto-defaults bypassed by CLI `mnemo index`, lost on `rebuild_index`/`compact_file`, stale in `IndexInfo`
- **Status:** `VERIFIED (amended ANN-1.1 working tree @ c417d2e+)`
- **Context / Evidence:** Working-tree diff in `mnemo/src/index.rs:416-510`, `mnemo/src/bin/mnemo.rs:158-164, 1036-1043`, `mnemo/src/store.rs:1595-1605, 2714, 2727-2735`, `PLAN.md:3630-3654` (`PR ANN-1.1`).
- **Finding:** Auditing the in-progress `PR ANN-1.1` diff against its callers surfaced four bugs that would ship broken behavior if not caught before merge:
  1. **CLI `mnemo index` hardcodes `8` and `64`:** `Command::Index` (`mnemo/src/bin/mnemo.rs:158-164`) still has `#[arg(long, default_value_t = 8)] n_probe: usize` and `#[arg(long, default_value_t = 64)] n_rerank: usize`, passing them straight into `IndexConfig` (`mnemo.rs:1036-1042`). `IvfPqIndex::build` treats any non-zero value as an explicit user override (`defaults_auto = false`), so **100% of default CLI `mnemo index <path>` builds bypass `ANN-1.1` and stay stuck at `8 / 64`**.
  2. **`rebuild_index` and `compact_file` silently flip `defaults_auto` to `false` and erase custom partition/subspace settings:** `IvfPqIndex::build` resolves `0` into concrete non-zero numbers stored in `self.n_probe` / `self.n_rerank`. Then `Mnemo::rebuild_index` (`store.rs:1596-1601`) and `Mnemo::compact_file` (`store.rs:2714, 2729-2733`) pass `n_probe: a.n_probe(), n_rerank: a.n_rerank()` (non-zero!) into `IndexConfig { ..Default::default() }`. Rebuilding or compacting therefore turns `defaults_auto` off (freezing `n_probe`/`n_rerank` as the corpus later grows from 1K → 100K) *and* resets custom `n_partitions` / `pq_subspaces` to `0`.
  3. **`IndexInfo` lies on upgraded v0.5.x files:** `IvfPqIndex::info()` (`index.rs:501-510`) returns raw `self.n_probe` / `self.n_rerank` rather than `self.effective_defaults(10)`, so `mnemo info` and `db.stats()` on an upgraded v0.5.x index still print `defaults: n_probe=8, n_rerank=64` even while `search` runs auto values.
  4. **All-or-nothing `defaults_auto`:** `cfg_had_auto_probe && cfg_had_auto_rerank` disables auto-scaling of `n_rerank` when a user only overrides `n_probe` (or vice versa).
- **Recommendation:**
  1. In `mnemo/src/bin/mnemo.rs:158-164`, change `n_probe` and `n_rerank` `default_value_t` to `0` (documenting `0 = auto`, matching `partitions` and `subspaces`).
  2. Store `0` directly in `IvfPqIndex.{n_probe, n_rerank}` to mean "auto" independently per dial (resolving `0` via `effective_auto_n_probe` / `effective_auto_n_rerank` inside `effective_defaults(top_k)` used by both `search` and `info()`), while keeping the legacy `(8, 64)` shim for v0.5.x files that lacked `defaults_auto`. That makes independent single-dial overrides work *and* lets `rebuild_index` / `compact_file` preserve auto-scaling plus `n_partitions: a.partitions()` and `pq_subspaces: a.subspaces()`. (Also double-check `R-001.10` before adding a 9th positional field to `IvfPqIndex` without a version check.)
- **Planner Triage:** `ACCEPTED` (2026-11-01) → in-flight PR ANN-1.1 (URGENT AMEND entry, 2026-11-01). All four sub-points verified in the working tree. Revised design: no new `IvfPqIndex` field; keep the legacy (8,64) shim; CLI `--n-probe/--n-rerank` default to 0 = auto; rebuild/compact re-derive all params for the current size; `info()` reports effective values.

#### R-001.4 — [CORRECTNESS] [MEDIUM] CLI `mnemo recall` (default tracking mode) takes exclusive lock but drops without flushing stats or manifest
- **Status:** `ACCEPTED`
- **Context / Evidence:** `mnemo/src/bin/mnemo.rs:258-260, 1309-1375`.
- **Finding:** Without `--no-track`, `Command::Recall` opens the database read-write via `Mnemo::open` (`mnemo.rs:1314, 1370`) specifically to update `accessed_at` / `access_count` (and, on first `--text` use, persist `metadata.embedder` onto the scaffold manifest via `db.set_embedder` at `mnemo.rs:1324`). However, after `db.recall(&req)`, `db` is dropped at the end of the `match` arm without calling `db.flush()` or `db.close()`. Because `Mnemo` does not auto-flush on `Drop`, both the access-stat updates and the first-run manifest embedder identity write are silently discarded, despite blocking concurrent readers with an exclusive OS lock.
- **Recommendation:** Call `db.close().map_err(fmt)?;` (or `db.flush()`) before returning in the `!no_track` branches of `Command::Recall`, and add a CLI smoke test verifying `access_count` persists across `mnemo recall` (and remains untouched under `--no-track`).
- **Planner Triage:** `ACCEPTED` (2026-11-01) — (partly correct) -> Recall scoring v2. Verified: the CLI's tracking `recall` opens read-write and drops the handle without flushing, discarding access-stat updates. Incorrect sub-claim: the manifest identity write is NOT lost, because `set_embedder` flushes its own upsert. Scoring v2 decides the tracking default; the CLI then flushes (if tracking) or opens read-only (if not).

#### R-001.5 — [CORRECTNESS] [MEDIUM] Windows mandatory-lock failure in Python `export_encrypted` & `rekey(fast=False)` clobbering custom KDF params
- **Status:** `VERIFIED (PR FIX-1 @ 8e09e60, 93e2620, v0.6.0)`
- **Context / Evidence:** `mnemo-python/src/lib.rs:580-597, 710-740`, `mnemo/src/pager.rs:102-115`.
- **Finding:**
  1. **`export_encrypted` on Windows read-write handles:** `Mnemo.export_encrypted` calls `self.inner.close()` (`lib.rs:731`, which flushes `Core` in place while keeping its `File` handle and exclusive `LockFileEx` lock open) and then calls `std::fs::copy(&self.path, dest)` (`lib.rs:734`). Because Windows `LockFileEx` byte-range locks (`0..u64::MAX`) are **mandatory**, opening `self.path` via a second file descriptor inside `std::fs::copy` while holding an exclusive lock fails with `ERROR_LOCK_VIOLATION` (OS error 33). Existing tests in `mnemo-python/test_mnemo.py` only exercised `export_encrypted` on a `read_only=True` shared lock.
  2. **`Mnemo.rekey(new_passphrase, fast=False)` docstring vs behavior mismatch:** `lib.rs:586-596` documents that `fast=False` *"inherits the same protection level as the file's original creation-time parameters"*, but actually passes `KdfParams::default()`, overwriting whatever `KdfParams` the file was created with.
- **Recommendation:**
  1. Add a core `Mnemo::export_encrypted(&mut self, dest: impl AsRef<Path>) -> Result<()>` (streaming bytes directly from the already-open `&mut self.pager.file` after `flush()`, followed by `dest.sync_all()`) and delegate Python's `export_encrypted` to it — works identically on read-only and read-write handles across Linux, macOS, and Windows without releasing or violating the OS lock. Add a read-write `export_encrypted` test to `test_mnemo.py` so Windows CI exercises it.
  2. Expose `Mnemo::kdf_params(&self) -> KdfParams` on core `Mnemo` and use `self.core()?.kdf_params()` when `fast=False` in Python's `rekey`.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → PR FIX-1: a core `Mnemo::export_encrypted` streaming from the open handle (Python delegates; Rust test on the 3-OS matrix, since Python CI is Linux-only) and a `kdf_params()` accessor so `rekey(fast=False)` keeps the file's current KDF params. Impact of the KDF point is low (`KdfParams::default()` == `secure()` == the creation default), but the docstring is wrong.

#### R-001.6 — [CORRECTNESS] [MEDIUM] `EmbedCmd::embed` omits zero-vector protocol check on `recall` and `cache_get_semantic`
- **Status:** `VERIFIED (PR FIX-1 @ 8e09e60, v0.6.0)`
- **Context / Evidence:** `mnemo/src/embed_cmd.rs:53, 402, 464-495`, `mnemo/src/embed.rs:89-91`.
- **Finding:** Both `embed_cmd.rs` module docs (lines 53, 402) and `CHANGELOG.md` state that an all-zero vector reply from `--embed-cmd` is treated as a protocol violation that kills and respawns the child process. In reality, `EmbedCmd::embed` checks dimension width (`embed_cmd.rs:465`) and `x.is_finite()` (`embed_cmd.rs:474`), but **never checks `crate::embed::is_zero_vector(&vec)`**. While `remember` later rejects zero vectors inside `store::put` (leaving the wedged child alive instead of restarting it), `recall --text` and `cache_get_semantic` do not reject zero vectors at all — silently returning 0-similarity results.
- **Recommendation:** Add an `is_zero_vector(&vec)` check right after the `is_finite()` check in `EmbedCmd::embed` (`embed_cmd.rs:480`), routing through `fail_and_restart` with a clear `"all-zero vector"` protocol error, plus a unit test in `embed_cmd.rs`.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → PR FIX-1. Verified: `embed_cmd.rs` documents zero-vector replies as protocol errors (L53, L402) but has no check, and no finiteness check either. Add both through `fail_and_restart`, with unit tests.

---

#### R-001.7 — [PERF] [HIGH] `SEC-1` made pre-flush page-0 nonce-lease `sync_all` obsolete: cut `flush()` from 4 fsyncs to 2 (~35–50% faster writes)
- **Status:** `ACCEPTED`
- **Context / Evidence:** `mnemo/src/store.rs:1788-1947`, `mnemo/src/pager.rs:144-166`, `test/scripts/perf_v0.5.0.json:4259` (`remember_1_then_flush = 21.9 ms`), `PLAN.md` Phase 11.1 (`fsyncs_last_flush`).
- **Finding:** `Mnemo::flush()` currently executes **4 sequential `sync_all()` (`F_FULLFSYNC` on macOS) calls** (~22 ms for a single memory write):
  1. `prepare_for_flush` (`store.rs:1939-1945`): writes unlogged page 0 + `sync_all()` solely to persist a leased `header.write_counter` ahead of `pager.flush()` so deterministic `(page_id, write_counter)` AES-GCM nonces could not repeat after a crash.
  2. `pager.flush()` (`store.rs:1892`): writes dirty encrypted data pages + `sync_all()`.
  3. `wal::commit` (`store.rs:1921`, `wal.rs:94-127`): writes WAL frames + `sync_all()`, then `KIND_COMMIT` + `sync_all()` (actually **5** `sync_all`s total when `wal::commit`'s 2 internal fsyncs are counted — note `self.fsyncs_last_flush` at `store.rs:1895-1934` undercounts `wal::commit` as `+1` instead of `+2`!).
  4. Checkpoint page 0 + `wal::clear` (`store.rs:1928-1934`): `sync_all()` + another `sync_all()` inside `wal::clear`!
  Counting `wal.rs:112, 126, 139` and `pager.rs:198`, a single `Mnemo::flush()` actually issues **6 `file.sync_all()` calls** (`prepare_for_flush` [1] + `pager.flush` [2] + `wal::commit` data [3] + `wal::commit` commit-record [4] + page-0 checkpoint [5] + `wal::clear` [6])!
  Crucially, **since `v0.5.2` (`PR SEC-1`), page nonces use a random per-session 64-bit prefix (`pager.rs:148-153`) — `header.write_counter` is no longer used in any cryptographic nonce!**
- **Recommendation:** Prioritize this in **Phase 11.1** (high-reward, zero durability trade-off):
  1. Delete `prepare_for_flush`'s unlogged page-0 write + `sync_all()` (fixes a torn-header hazard from `R-001.2` *and* saves 1 `F_FULLFSYNC`).
  2. Write dirty newly-allocated out-of-place data pages *and* WAL data frames first, then issue **1 `sync_all()`** before writing `KIND_COMMIT` + **1 `sync_all()`** to commit the transaction, followed by page-0 checkpoint (and lazy WAL invalidation via monotonic `wal_seq`, removing `wal::clear`'s extra `sync_all()`). That drops physical `sync_all()` calls per flush from **6 → 2–3**, cutting `remember_1_then_flush` latency by ~50% while preserving strict crash atomicity. Also fix `fsyncs_last_flush` to count actual `sync_all()` invocations accurately.
- **Planner Triage:** `ACCEPTED` (2026-11-01) — (partly correct). Confirmed: since SEC-1, `write_counter` feeds no nonce, so `prepare_for_flush`'s page-0 lease write + fsync is obsolete -> removed in PR DUR-1 (4 -> 3 fsyncs). Incorrect sub-claims: `wal::commit` issues ONE `sync_all` and `wal::clear` does not exist, so a flush is 4 fsyncs (the counter is accurate), not 6. The data+WAL fsync merge / lazy checkpoint (~2 fsyncs) -> Phase 11.1 with crash tests.

#### R-001.8 — [PERF] [HIGH] `Mnemo::stats()` is `O(N)` full-DB decrypt; cache/access flushes evict 256-slot PITR snapshots; `O(M log M)` sort in ANN search
- **Status:** `VERIFIED (Part 3 in ANN-1.1 @ 190a407; Parts 1-2 in PR FIX-1 @ 8e09e60, v0.6.0)`
- **Context / Evidence:** `mnemo/src/store.rs:1794-1820, 2146-2150, 2497-2501, 2676-2694`, `mnemo-python/src/lib.rs:620`, `mnemo/src/index.rs:593-595`.
- **Finding:** Three engine bottlenecks that degrade scaling and PITR usability:
  1. **`Mnemo::stats()` decrypts every live record (`O(N)`):** `stats()` (`store.rs:2679`) calls `self.memories()?` solely to compute distinct `agents: Vec<String>`. At 100K–1M memories, `mnemo info`, `mnemo about`, or `db.stats()` takes 1–64 seconds of AES-GCM decryption + MessagePack decoding. Worse, Python's `Mnemo.insert_default_manifest` (`mnemo-python/src/lib.rs:620`) calls `self.inner.stats()` just to read `dimensions` instead of `self.inner.dimensions()`.
  2. **Cache and access-tracking flushes evict real PITR snapshots:** `Mnemo::flush()` unconditionally appends a new `Snapshot` to `self.manifest` and evicts entries past `max_snapshots = 256` (`store.rs:1803-1812`) even when *only* `dirty_cache` was dirty (and `Snapshot` / `apply_snapshot` doesn't even snapshot or restore `cache_dir`!). Under normal agent cache activity (`Strict` policy flushes on every `cache_put`), 256 cache writes completely wipe out all memory restore points. Also, under `CacheFlushPolicy::Batched`, `max_age` is never checked during `cache_get` / `cache_get_semantic`, leaving aged dirty batches unflushed during read-heavy phases.
  3. **`O(M log M)` full candidate sort in `IvfPqIndex::search` (`index.rs:594-595`):** With `ANN-1.1` probing 50% of partitions, `cands` holds ~50,000 items at 100K and ~500,000 items at 1M. `cands.sort_by(...)` sorts the entire vector just to `truncate(rerank)` (500–1,000 items).
- **Recommendation:**
  1. Switch `mnemo-python`'s `insert_default_manifest` to `self.inner.dimensions()` immediately; for `Mnemo::stats()`, inline an interned `agent_id` index (or `agent_hash`) into format v9's catalog entry alongside Phase 11.2 so `stats()` becomes `O(N_catalog)` in-memory without touching a single record page.
  2. Only push a new PITR `Snapshot` onto `self.manifest` when memory/index state actually changed (e.g. skip snapshot push on cache-only flushes, and consider coalescing pure access-stat-bump flushes so `recall` traffic doesn't overwrite `remember`/`delete` restore points). Check `cache_batch.should_flush` opportunistically or document flush triggers on `Batched`.
  3. In `IvfPqIndex::search` (`index.rs:594-595`), if `cands.len() > rerank`, call `cands.select_nth_unstable_by(rerank - 1, |a, b| a.1.total_cmp(&b.1)); cands.truncate(rerank);` before sorting the small `rerank` slice (`O(M + R log R)` instead of `O(M log M)`).
- **Planner Triage:** `ACCEPTED` (2026-11-01) — Verified all three. Parts 1-2 -> PR FIX-1: `insert_default_manifest` uses `dimensions()`; a lazy in-memory agent set makes `stats()` cheap without a format change; no restore-point push on cache-only flushes (snapshots don't capture the cache anyway); Batched `max_age` checked on reads. Part 3 (`select_nth_unstable_by` before the rerank sort) -> folded into in-flight PR ANN-1.1.

#### R-001.9 — [PERF] [MEDIUM] ANN 1M-tier `n_probe = 50%` scaling ceiling & Residual IVF-PQ (`IVF-RPQ`) opportunity in format v9
- **Status:** `DEFERRED`
- **Context / Evidence:** `mnemo/src/index.rs:68-74, 345-410`, `test/scripts/perf_v0.5.0.json:1-70` (`ann_diagnose`), `PLAN.md:3615-3654`.
- **Finding:** `ANN-1.1`'s `n_probe = ceil(partitions / 2)` + `pq_subspaces = dim / 4` is the right pragmatic fix for `v0.6.0` at 10K–100K, but at 1M vectors (`1000` partitions) probing 50% means scanning 500,000 96-byte PQ codes (~48 MB of lookup traffic) on every query. `perf_v0.5.0.json`'s `ann_diagnose` block pinpoints why raw PQ struggles (`full_scan_recall_at_10 = 0.918` at 100K with `dim/8`): `IvfPqIndex::build` quantizes **raw vectors `v`** globally rather than **partition residuals `r = v - coarse_centroid[p]`**. Residual encoding removes inter-partition variance before running 256-centroid subspace k-means, typically lifting recall@10 by +8–15 pp while requiring far fewer subspaces (`dim/8`) and smaller probe fractions (`10–15%` instead of `50%`).
- **Recommendation:** Ship `ANN-1.1` for `v0.6.0` (benchmarking 1M query latency with `select_nth_unstable_by` from `R-001.8` to verify whether a sublinear cap at ≥250K vectors is warranted), and evaluate **Residual IVF-PQ (`IVF-RPQ`)** for the **format v9** bump (`v0.7.0`) when on-disk structures already migrate.
- **Planner Triage:** `DEFERRED` (2026-11-01) → evaluated at the start of Phase 2 + 11.2 (format v9) alongside the flat-int8 baseline, using ANN-1.1's 1M measurements. Verified that PQ trains on raw vectors (no residuals). Valid, but it's an index-format change, so it belongs with the v9 bump.

---

#### R-001.10 — [PLAN-REFINEMENT] [HIGH] Positional MessagePack (`rmp_serde::to_vec`) forward-compatibility hazard on `format::VERSION = 8`
- **Status:** `VERIFIED (standing rule in PLAN.md + V052IvfPqIndex field-count guard in amended ANN-1.1; ongoing audit rule)`
- **Context / Evidence:** `PLAN.md:3640` (`PR ANN-1.1`), `PLAN.md:677-695` (`Phase 12.1`), `PLAN.md:1075-1085` (`Phase 7.3`), `mnemo/src/index.rs:323`, `mnemo/src/store.rs:1857`.
- **Finding:** `PR ANN-1.1` (adding `defaults_auto: bool` to `IvfPqIndex`), `Phase 12.1` (adding `content_hash` / `supersedes`), and `Phase 7.3` rely on `#[serde(default)]` appended fields to avoid bumping `format::VERSION` (`8`). Because `rmp_serde::to_vec` serializes Rust structs as **positional MessagePack arrays (`fixarray(N)`)** (not named maps), `#[serde(default)]` only provides *backward* compatibility (v0.6.0 reading a v0.5.2 file). **Forward compatibility breaks ungracefully:** if a `v0.5.0`–`v0.5.2` binary opens a `VERSION = 8` file written by `v0.6.0`, `Header::from_page` succeeds (`8 == 8`), and deserialization then aborts mid-open or mid-query with `MnemoError::Serialize("invalid length 9, expected struct ... with 8 elements")` instead of `MnemoError::UnsupportedVersion`.
- **Recommendation:**
  1. Note that for `PR ANN-1.1`, if `0` is stored directly in `n_probe` / `n_rerank` to mean "auto" (see `R-001.3`), **no 9th struct field is needed on `IvfPqIndex` at all** (except optionally distinguishing an explicit `n_probe=8, n_rerank=64` override from a legacy v0.5.x build, which rarely matters if `(8, 64)` upgrades to auto until rebuilt with e.g. explicit values, or if `VERSION` bumps cleanly).
  2. For `Phase 12.1` and future struct extensions: either fold `Phase 12.1`'s new persisted fields into the `format::VERSION = 9` bump (header-only cascade migrations in MNemo are trivial and zero-copy for unchanged pages), OR add a `min_reader_version` field in reserved header bytes so older binaries fail cleanly at `Header::from_page` with `UnsupportedVersion`.
- **Planner Triage:** `ACCEPTED` (2026-11-01) — Verified in rmp-serde 1.3.0 source: excess array elements -> `Error::LengthMismatch` (decode.rs ~L571-574). New standing rule: no appended positional fields under the same `format::VERSION`; either bump VERSION with a migration or use reserved `_mnemo.*` keys in `Memory.metadata`. Applied: ANN-1.1 drops its new field; the Phase 12.1 spec now uses metadata keys; v9 adds `min_reader_version`.

#### R-001.11 — [PLAN-REFINEMENT] [HIGH] `PR PY-1` (`py.allow_threads` + `Option<Core>`): avoid `PyFnEmbedder` re-entrant borrow panic & cover `Session`
- **Status:** `VERIFIED (PR PY-1 @ abfaff3, v0.6.0)`
- **Context / Evidence:** `PLAN.md:3258-3288, 3655-3672` (`PR PY-1`), `mnemo-python/src/lib.rs:145-195, 345-450, 980-1033, 1157-1319`.
- **Finding:** Two PyO3 subtleties in `PR PY-1`'s current spec:
  1. **Re-entrant `PyRefMut` borrow panic if `PyFnEmbedder` runs inside `py.allow_threads` on a `&mut self` method:** When a `#[pymethods]` function takes `&mut self`, PyO3 holds the `PyCell`/`PyRefMut` mutable borrow flag for the **entire duration of the Rust method, even while the GIL is released inside `py.allow_threads`**. That protects against concurrent threads mutating `db`, *but* if `py.allow_threads` invokes a Rust method that calls back into `PyFnEmbedder::embed` (re-acquiring the GIL on the *same* thread) and the user's Python embedder callback touches `db` (e.g. `db.dimensions()`, `db.cache_get("embeddings", text)`, or `len(db)`), PyO3 immediately raises `RuntimeError: Already borrowed`!
  2. **`Session` holds `Py<Mnemo>` (`lib.rs:1159`):** `Session.{add_turn, recall, close, discard}` and `Session.__exit__` directly access `db.borrow_mut(py).inner`. Once `Mnemo` wraps `Option<Core>`, all `Session` methods must go through `db.core_mut()?` so closing `db` before `session` raises a clean `"Mnemo handle is closed"` `RuntimeError`.
- **Recommendation:**
  1. Keep Python-side `embedder.embed(...)` **outside** `py.allow_threads` in `remember_text`, `recall_text`, and `cache_get_semantic_text`! Even better: take `slf: &Bound<'_, Self>`, clone the `Arc<dyn Embedder>` inside a brief immutable `slf.borrow()`, drop the borrow while calling `embedder.embed(&[text])` (so re-entrant `db.cache_get(...)` inside a Python embedder works seamlessly!), and *then* `slf.borrow_mut()` + `py.allow_threads(|| core.remember(...))` for the pure-Rust crypto/I/O/scan work. (Note `set_embedder` never invokes `embed()`, so wrapping `core.set_embedder` in `py.allow_threads` is already re-entrancy-free.)
  2. Route all `Session` methods through `core_mut()?` and add a test in `test_mnemo.py` for `db.close()` followed by `session.add_turn` / `session.close()`, plus a test where the Python embedder callback reads `db.dimensions()` / `db.cache_get(...)` re-entrantly.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → PR PY-1 spec: call the Python embedder outside `allow_threads` and outside any held mutable borrow (clone the `Arc<dyn Embedder>` under a short borrow); route every `Session` method (holds `Py<Mnemo>`, borrows mutably) through the closed-aware accessor; tests for a re-entrant embedder callback and close-then-session use.

#### R-001.12 — [PLAN-REFINEMENT] [HIGH] `Recall scoring v2`: similarity-gated boosts, `created_at` vs `accessed_at` recency split, and `ranking_harness.py` timestamp variance
- **Status:** `ACCEPTED`
- **Context / Evidence:** `mnemo/src/store.rs:1522-1531`, `mnemo/src/memory.rs:280-318`, `test/scripts/ranking_harness.py:491-508`, `test/scripts/perf_v0.5.0.json:3031-3197, 3653-3977`, `PLAN.md:458-495, 3673-3680`.
- **Finding:** Auditing `store.rs:1522-1531` alongside `perf_v0.5.0.json`'s miss traces (`similarity_only` `hit@1 = 0.5667, MRR = 0.669` vs `default_weights_track_true` `hit@1 = 0.4000, MRR = 0.525`; `function_semantic` `hit@1` collapsing `0.50 → 0.00`) exposes **two root flaws in the formula** and **one blind spot in `ranking_harness.py`**:
  1. **Un-gated additive bonuses overpower cosine similarity:** With `all-MiniLM-L6-v2`, true semantic matches often score `sim ∈ [0.30, 0.50]` while completely unrelated entries score `sim ∈ [0.08, 0.15]`. Because `0.3*recency + 0.2*importance + 0.1*ln(uses+1)` adds up to `+0.65` unconditionally, `release:v0.1.0` (`sim = 0.0957`, `importance = 0.90`, high `uses`) beat `api:Mnemo::open` on `"how do I open a database?"` and pushed the right answer completely out of top-10 (`perf_v0.5.0.json:3031-3049`).
  2. **Recency decays from `accessed_at` instead of `created_at` (`store.rs:1523`):** `let age = (now - m.accessed_at) as f32;` means a single accidental recall hit on an old memory *both* increments `access_count` (`delta`) *and* resets `recency` to `1.0` (`+0.30` full `beta` jump!), creating a severe rich-get-richer feedback loop.
  3. **`ranking_harness.py` never tests recency variance:** `_ensure_corpus` (`ranking_harness.py:499-507`) inserts all 55 memories within ~50 ms, so in `track_false` every memory has `age ≈ 0` (`recency = 1.0`, identical `+0.30` constant across all 55 rows). Thus the harness currently tests `importance` + `access_count` drift, not temporal recency decay.
- **Recommendation:**
  1. **Split temporal freshness from usage reinforcement:** Compute `recency` from `now - m.created_at` (when the memory was created), leaving `access_count` (and optionally a much gentler decay on `accessed_at`) strictly in the reinforcement term.
  2. **Gate secondary signals by semantic relevance (multiplicative or floor-gated modulation):** Replace pure additive `alpha*sim + bonus` with similarity-modulated scoring, e.g. `let base = sim.max(0.0); score = sim + base * (beta * recency + gamma * (importance - 0.5) + delta * ln_1p_uses_capped)` (or gate bonuses to candidates within a margin of the query's `max_sim`). Under multiplicative modulation, a `sim = 0.09` off-topic entry gets at most `~0.12` total score and can *never* outrank a `sim = 0.38` relevant memory, while two genuinely relevant memories (`sim = 0.44` vs `0.41`) still let recency/importance break the near-tie cleanly. Also consider capping `ln(uses + 1)` (or sub-logarithmic saturation) so heavy usage plateaus instead of growing unboundedly.
  3. **Add staggered timestamps to `ranking_harness.py`:** Assign realistic historical `created_at` timestamps across `v0.1.0`..`v0.5.0` (e.g. via JSONL import or an optional `created_at` parameter) and add 2–3 recency-tiebreaker queries (e.g. superseded config/status facts) so the harness validates both failure modes.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → Recall scoring v2: created_at recency, similarity-gated/multiplicative bonuses, saturating usage term (plus RRF) become harness-evaluated candidates. Staggered `created_at` timestamps and superseded-fact queries are now REQUIRED harness additions before any formula is chosen. Matches the planner's 2026-10-25 FINDING.

#### R-001.13 — [PLAN-REFINEMENT] [MEDIUM] `Phase 12.4` Shared Access (`--shared`): `try_lock` contention & `compact_file` `rename` hazards on Windows/Unix
- **Status:** `ACCEPTED`
- **Context / Evidence:** `PLAN.md:731-755, 3192-3199` (`Phase 12.4`), `mnemo/src/pager.rs:78-115`, `mnemo/src/store.rs:2740`.
- **Finding:** Before locking in the Phase 12.4 design note (`open_shared` / `mnemo serve --mcp --shared`), two OS file-locking and filesystem invariants in the current codebase need explicit treatment:
  1. **`try_lock_*` causes spurious `MnemoError::Locked` under short per-op locks:** Neither Unix `flock` nor Windows `LockFileEx` allows an exclusive writer lock and a shared reader lock simultaneously. Under a lock-per-operation model, if Process A is inside a 15 ms `flush()` (holding exclusive lock) and Process B calls non-blocking `try_lock_shared()`, Process B fails immediately with `MnemoError::Locked`.
  2. **`compact_file`'s `std::fs::rename(&tmp, path)` breaks open shared handles:** On Windows, `rename` over an existing file fails with `AccessDenied` / sharing violation if *any* process holds an open `File` handle to `path` (even with zero active byte-range locks). On Unix, `rename` unlinks the old inode while long-lived `open_shared` handles keep reading and writing the **orphaned old inode**, causing silent split-brain data loss!
- **Recommendation:** Capture both constraints in the Phase 12.4 design note:
  1. Use **blocking lock acquisition with a bounded timeout/backoff** (e.g. `lock_timeout = 2s`, retrying every few ms) for `--shared` per-operation locks rather than instant-fail `try_lock_*`.
  2. For `open_shared`: either open/close the file descriptor per operation (or on Unix check `fstat` `(dev, ino)` against `stat(path)` after acquiring the lock and transparently reopen if `compact_file` replaced the inode, while on Windows `compact_file` acquires an exclusive lock / requires no other open handles or compacts in-place).
- **Planner Triage:** `ACCEPTED` (2026-11-01) → Phase 12.4 design-note requirements: bounded blocking lock acquisition with backoff (no instant-fail `try_lock` in shared mode); `compact_file` rename hazards (Windows refuses while handles are open; Unix orphaned-inode split brain -> check `(dev, ino)` after locking and reopen; Windows compaction needs exclusive access or compacts in place).

---

#### R-001.14 — [PRODUCT] [HIGH] Pre-top-k structured metadata & `session_id` predicate filtering in `RecallRequest`, CLI, Python, and MCP
- **Status:** `ACCEPTED`
- **Context / Evidence:** `mnemo/src/memory.rs:342-406` (`RecallRequest`), `mnemo/src/store.rs:1464-1565`, `mnemo/src/mcp.rs:640-750`.
- **Finding:** `Memory` encourages rich structured `metadata: Map<String, Value>` (`area`, `topic`, `repo`, `ticket`, `role`, `source`) plus `session_id`, and `Phase 12.3` plans `delete_where(predicate)` for bulk deletion — yet **`RecallRequest` has no metadata or `session_id` filter at all** (only `memory_types` and `agent_id`). Callers today must over-fetch `top_k` and post-filter in application code, which fails catastrophically whenever one prolific project/session dominates the top `n_rerank` / `top_k` slots and starves the target metadata slice.
- **Recommendation:** Share a unified, lightweight `Filter` / metadata-predicate type across **both `RecallRequest` and Phase 12.3's `delete_where` / `export`**:
  - Support `session_id`, exact key-value match (`meta_eq("area", "security")`), set membership (`$in`), numeric/string range, and key existence.
  - Apply the predicate **before** `top_k` truncation (and in ANN mode, ensure filtered queries either expand candidate probing or push filter awareness into candidate selection so selective filters don't return fewer than `top_k` existing matches).
  - Expose as `--where key=value` on CLI `recall`/`list`, `metadata_filter={...}` in Python `recall`/`recall_text`, and `filter: {...}` on MCP `recall`/`list_memories`. Preserves the single-file invariant with zero format break.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → NEW Phase 12.6 (after 12.1): a shared `Filter` (session_id, metadata eq/$in/range/exists) applied before top-k in recall/list/export/delete_where, ANN mode widening candidates until top_k matches; CLI `--where`, Python `metadata_filter=`/`session_id=`, MCP `filter`. No format change. Verified `RecallRequest` has only type/agent filters today.

#### R-001.15 — [PRODUCT] [MEDIUM] Token-budgeted context packing & Maximal Marginal Relevance (MMR) diversity in `recall`
- **Status:** `DEFERRED`
- **Context / Evidence:** `mnemo/src/store.rs:1464-1565`, `mnemo/src/mcp.rs:695-745`.
- **Finding:** LLM agents and MCP clients ultimately consume `recall` results inside a bounded prompt window, not a fixed item count. `top_k = 10` suffers from two practical agent UX problems: (1) unpredictable prompt size (10 terse facts = 150 tokens; 10 code/log memories = 15,000 tokens), and (2) **redundancy** — top-k similarity frequently returns 4–6 near-identical episodic memories from the same debugging thread, crowding out complementary procedural rules ranked #11–#15.
- **Recommendation:** Add two optional, composable retrieval controls to `RecallRequest` (and MCP `recall` / Python `recall_text`):
  1. **`max_content_bytes` (or `token_budget` heuristic):** Greedily pack ranked results until the budget is filled rather than cutting strictly at `top_k`.
  2. **`diversity: Option<f32>` (Maximal Marginal Relevance, MMR):** Because `Mnemo::recall` already decrypts the top candidate records (including their `vector: Vec<f32>`) during scoring (`store.rs:1503`), greedy MMR selection over the top `M` (e.g. `3 * top_k`) scored candidates (`λ * score(d) - (1 - λ) * max_{s ∈ S} cosine(d.vec, s.vec)`) requires **zero extra disk I/O** and only a few dozen dot products, dramatically improving context coverage for conversational agents.
- **Planner Triage:** `DEFERRED` (2026-11-01) → Phase 7.5: token-budget packing + MMR diversity, judged by the ranking harness after scoring v2 and Phase 12. Valid and cheap on the exact path, but ranking-quality work should follow the scoring fix so its effect can be measured separately.

#### R-001.16 — [PRODUCT] [MEDIUM] Cognitive consolidation on `Session::close` & transactional brain diffing (`mnemo diff <txn_a> [txn_b]`)
- **Status:** `ACCEPTED`
- **Context / Evidence:** `mnemo/src/session.rs:124-153`, `mnemo-python/src/lib.rs:1075-1319`, `mnemo/src/store.rs:2525-2623`.
- **Finding:**
  1. **Raw working-turn bloat:** `Session::close()` currently promotes *every* `Working` turn 1:1 into permanent `Episodic` memory (`session.rs:138-145`). Over weeks of agent use, raw turn-by-turn transcripts pollute semantic recall with low-signal conversational scaffolding. (Also, Python's `Session` lacks `add_turn_text` / `recall_text` convenience methods even after Phase 3 added `set_embedder`.)
  2. **Invisible learning / auditability:** MNemo's copy-on-write `Snapshot` history is a unique architectural asset over Chroma/LanceDB/SQLite-vec, yet users and orchestrators can only list snapshot counts (`mnemo snapshots`) or blindly roll back (`mnemo restore`) — they cannot inspect *what an agent learned, superseded, or deleted* during a task!
- **Recommendation:**
  1. **Consolidate-on-close (`Session::consolidate` + Python `Session.add_turn_text` / `recall_text`):** Allow closing a session by committing caller/LLM-distilled summary memories (linked via `session_id` or Phase 12.1's `supersedes`) while atomically expiring or discarding the raw `Working` turns in the same `flush()`.
  2. **`mnemo diff <txn_a> [txn_b]` (and `Mnemo::diff_snapshots` + MCP `diff_snapshots`):** Compare two snapshots' catalogs by `(Ulid, start_page, len, deleted)` in `O(N)` catalog entries — decrypting *only* the delta records that were added, modified, or tombstoned between `txn_a` and `txn_b`. Gives developers `"git diff"` observability over an agent's brain after every autonomous run, and pairs naturally with `mnemo restore` when an agent hallucinates bad memories.
- **Planner Triage:** `ACCEPTED` (2026-11-01) — (split): `mnemo diff` / `diff_snapshots` -> Phase 12.5 next to export (catalog compare, decrypt only changed records); consolidate-on-close -> existing Phase 7.4, adding atomic expiry of raw working turns in the same flush; Python `Session.add_turn_text`/`recall_text` -> Phase 12.1.

#### R-001.17 — [PRODUCT] [IDEA] Portable encrypted Knowledge Packs: read-only federated mounts (`attach_pack`) & scoped encrypted fork/merge
- **Status:** `DEFERRED`
- **Context / Evidence:** `AGENTS.md` ("the file IS the agent's brain"), `test/scripts/seed.json`, `PLAN.md` Phase 12.1 (`content_hash`) & Phase 12.3 (`mnemo export`).
- **Finding:** MNemo's single-file portability makes `.mnemo` files ideal distributable artifacts (e.g. a repository's `project.mnemo`, a team's security-runbook `.mnemo`, or a vendor SDK docs `.mnemo`). Today, however, an agent that needs *both* a shared read-only team/repo `.mnemo` *and* its own read-write private `.mnemo` must manage multiple handles manually, verify embedder compatibility itself, and merge rankings in userland — or dump everything into one file, entangling private episodic history with shareable team knowledge.
- **Recommendation:** Explore **Encrypted Knowledge Packs** (post-`v0.7.0` / builds cleanly on Phase 12.1 `content_hash` + Phase 3 `EmbedderIdentity`):
  1. **Read-Only Pack Attachment (`mnemo serve --mcp personal.mnemo --pack repo.mnemo` / `db.attach_pack(...)`):** Mount 1+ read-only `.mnemo` packs onto a primary handle (each can use its own passphrase via env/key); enforce matching `EmbedderIdentity` + `dimensions` at attach time; route all writes/sessions to the primary brain; transparently union + rank `recall` across primary + packs with provenance (`source_pack`). Every file stays a 100% standalone, single-file `.mnemo`.
  2. **Scoped Encrypted Fork & Idempotent Merge (`mnemo fork --where ...` / `mnemo merge`):** Export a filtered slice of a brain (e.g. `scope=shared` or `area=onboarding`) directly into a fresh `.mnemo` encrypted under a *new* DEK and passphrase (safe to share without leaking prior snapshots or DEK), and merge packs idempotently using Phase 12.1's `content_hash`.
- **Planner Triage:** `DEFERRED` (2026-11-01) → after Phase 12.4 (shared access) and the v9 bump; trigger: real multi-file demand. A promising direction for the portable-file story; the design must keep each pack under its own key, and fork must re-encrypt under a fresh DEK without carrying snapshot history.

---

### Batch `REV-002` — 2026-10-31 (Verification of Amended `PR ANN-1.1` + Pre-Implementation Design Audit of `PR DUR-1` & `PR FIX-1` Specs)

**Status Check on `REV-001` & Working Tree (`HEAD` `c417d2e`):**
- Read all `REV-001` Planner triage dispositions (`13 ACCEPTED`, `4 DEFERRED`, `0 REJECTED`) and noted both calibration corrections (`flush()` issues 4 `sync_all`s — `prepare_for_flush`, `pager.flush`, `wal::commit`, `checkpoint` — and `set_embedder` flushes its own manifest upsert).
- Confirmed the new single-committer `PLANNER — HANDOFF` rule in action: Worker committed root `README.md` separately in `c417d2e`.
- **Verified amended `PR ANN-1.1` working-tree diff (`R-001.3`, `R-001.8` part 3, `R-001.10` → `VERIFIED`):**
  1. `IvfPqIndex` (`mnemo/src/index.rs:331-360`) has the exact 7 serialized fields of `v0.5.2`, no `defaults_auto`, guarded mechanically by `current_build_serializes_with_v0_5_2_field_count` (`index.rs:676-696`).
  2. CLI `Command::Index` (`mnemo/src/bin/mnemo.rs:158-166`) defaults both `n_probe` and `n_rerank` to `0` (`auto`).
  3. `Mnemo::rebuild_index` (`mnemo/src/store.rs:1595-1604`) and `Mnemo::compact_file` (`store.rs:2731-2733`) both pass `IndexConfig::default()`.
  4. `IvfPqIndex::info()` (`mnemo/src/index.rs:504-513`) reports `self.effective_defaults()`.
  5. `IvfPqIndex::query` (`mnemo/src/index.rs:603-607`) uses `cands.select_nth_unstable_by(n_rerank - 1, ...)` + `truncate(n_rerank)` before sorting (`n_rerank >= 1` guaranteed at line 568).
- Audited the queued `PR DUR-1` (`PLAN.md:3785-3815`) and `PR FIX-1` (`PLAN.md:3817-3847`) specs line-by-line against `store.rs`, `wal.rs`, `format.rs`, and `result_cache.rs` ahead of Worker execution.

---

#### R-002.1 — [PLAN-REFINEMENT] [HIGH] `PR DUR-1` recovery design: 3 crash/truncation windows (relocation zero-before-write gap, 64-page scan cap, torn bytes outside `0..238` CRC)
- **Status:** `VERIFIED (PR DUR-1 @ 4f1edc1, 6bb5a84, 22b4c1e, v0.6.0)`
- **Context / Evidence:** `PLAN.md:3798-3812` (`PR DUR-1` spec), `mnemo/src/store.rs:73, 727-791, 1650-1670, 2627-2655`, `mnemo/src/wal.rs:156-245`, `mnemo/src/format.rs:70, 89-113, 268-311`.
- **Finding:** Tracing the `PR DUR-1` spec line-by-line against `store.rs`, `wal.rs`, and `format.rs` reveals three concrete crash/recovery windows that should be closed in the `DUR-1` design before coding:
  1. **Zeroing old WAL *before* an unlogged page-0 write leaves ZERO valid WAL during relocation (`PLAN.md:3802-3807`, `store.rs:1650-1670`):** Step 2 specifies *"invalidate the old region (zero its first frame header) and sync before the header points elsewhere"*, while Step 3 allows keeping `ensure_wal_capacity`'s and `rekey`'s unlogged `write_raw(0, &page)`. If `ensure_wal_capacity` zeroes and syncs `old_wal_start` first and then crashes mid-`write_raw(0, &page)` (the exact *"torn page 0 during relocation"* test in Step 4!), `old_wal_start` is already zeroed and `new_wal_start` has no frames yet — **0 valid committed `MWAL` transactions exist anywhere in the file**, making the torn header permanently unrecoverable (`Err(HeaderChecksum)`). Likewise, `rekey` (`store.rs:2627-2655`) calls `self.flush()?` (a no-op when clean at `store.rs:1718`) and does an unlogged `write_raw(0, &page)`; on a 0-flush database (`Mnemo::create` with `--no-manifest`, `wal_seq == 0`) a torn page 0 during `rekey` permanently loses the DEK, and after prior flushes it silently rolls back the passphrase rotation.
  2. **`TORN_HEADER_SCAN_PAGES = 64` (512 KiB) truncates relocated WAL transactions (`store.rs:73, 736`, `wal.rs:165-178, 219`):** A WAL relocates precisely because its control-plane transaction exceeds the current WAL size; at 10K–100K vectors with an ANN index or large catalog, a single WAL transaction spans hundreds to thousands of pages (multiple MiB). If candidate-region scanning reads a fixed `TORN_HEADER_SCAN_PAGES` (64 pages) window into `wal::recover`, `recover_bytes` hits `body + len > filled` (`wal.rs:219`) before `KIND_COMMIT` and returns `Ok(None)`, failing to heal any large relocated transaction.
  3. **Torn bytes `0..6` (`BadMagic` / `UnsupportedVersion`) and torn bytes `242..294` (`HeaderTampered` past `HEADER_CRC_OFF = 238`) bypass WAL heal (`format.rs:70, 268-311`, `store.rs:727-791`):**
     - `Header::from_page` checks `MAGIC` (`b[0..4]`) and `version` (`b[4..6]`) *before* `HEADER_CRC_OFF` (`b[0..238]`), and `Mnemo::open` (`store.rs:729`) only triggers torn-header heal on `Err(MnemoError::HeaderChecksum)`. A tear or block-zeroing affecting bytes `0..6` returns `BadMagic` / `UnsupportedVersion` without consulting the WAL.
     - Bytes `242..294` (`seal_nonce` at `242..254`, `seal_tag` at `254..270`, and `cache_start/pages/len` at `270..294`) live **outside** `HEADER_CRC_OFF` (`0..238`). If page 0 tears after byte 242 (new `0..242` with valid CRC, stale `242..294`), `Header::from_page` returns `Ok(h)` with the new `wal_seq`, normal `wal::recover(..., header.wal_seq)` (`wal.rs:236`: `id > wal_seq`) skips replay because `id == header.wal_seq`, and `header.validate_seal(&dek)` (`store.rs:790`) fails with `Err(MnemoError::HeaderTampered)`.
- **Recommendation:**
  1. **Make 1-frame WAL-logged header commits mandatory (not optional) for both `ensure_wal_capacity` and `rekey`:** Bump `txn_id = self.header.wal_seq + 1`, `wal::commit` a 1-frame `[(0, new_hpage)]` transaction into **`new_wal_start`** (for relocation) or current `wal_start` (for `rekey`) *first*, then checkpoint page 0, and *then* zero `old_wal_start`'s first frame header + sync. Under this order, `new_wal_start` already holds durable committed `txn_id = N + 1 > N` *before* page 0 or `old_wal_start` is touched — zero unlogged page-0 writes remain anywhere after `create`.
  2. **Stream candidate WAL frames to EOF instead of capping at `TORN_HEADER_SCAN_PAGES = 64`:** At any page boundary `p >= 1` starting with `b"MWAL"`, stream 29-byte frame headers + payloads sequentially until `KIND_COMMIT` (or invalid frame / EOF), and require the committed transaction to include a `(0, hpage)` frame where `Header::from_page(&hpage)` succeeds with `h.wal_start == p && h.wal_seq == txn_id`.
  3. **Broaden heal triggers on `open`:** Run torn-header WAL scan on *any* `Header::from_page` error (returning the original error if no valid WAL commit exists, so non-`.mnemo` files still return `BadMagic`); and if `header.validate_seal(&dek)` fails with `HeaderTampered`, check `wal::recover(&mut file, header.wal_start, header.wal_pages, header.wal_seq.saturating_sub(1))` — replaying if the WAL's committed `wal_seq` frame passes `from_page` + DEK unwrap + `validate_seal(&dek)`. Add byte-range partial-page-0 tear tests covering bytes `0..6` and `242..294`.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → PR DUR-1 spec amended before dispatch (PLAN.md 2026-11-01 REV-002 entry). Verified all three windows: heal triggers only on `HeaderChecksum` (magic/version checked first); CRC covers 0..238 while seal/cache pointers sit at 242..294; `rekey` writes page 0 unlogged; the heal reads a fixed 64-page window. Adopted in full: WAL-logged one-frame header commits for relocation and rekey (new region first, then checkpoint, then invalidate old); streaming scan with header-frame validation (consistency + DEK unwrap + seal), no cap; heal on any header error, with a `HeaderTampered` fallback to the `wal_seq` txn. Context: the header lies in sector 0 (atomic writes), so the realistic failure is a whole zeroed/garbled sector (`BadMagic`), which the broadened trigger now covers; sub-sector tears at byte 242 are unlikely but covered at no extra cost. **AMENDED 2026-11-01 (after Worker 2's BLOCKED):** item 3's `HeaderTampered` → WAL-heal fallback is WITHDRAWN. It conflicts with the v7 seal's tamper-evidence contract (`header_tamper_is_detected_by_v7_seal`), the two on-disk states are indistinguishable, a crash can't produce CRC-valid + seal-invalid (the header is in an atomic sector), and the fallback would trust `wal_start`/`wal_seq` from the unauthenticated page. Rule: heal only on header parse failures; a seal failure is always `HeaderTampered`. Items 1, 2 and the rest of 3 (heal on BadMagic/UnsupportedVersion/HeaderChecksum) stand. An explicit `mnemo recover-header` is DEFERRED (trigger: a real sub-sector tear report).

#### R-002.2 — [PLAN-REFINEMENT] [MEDIUM] `PR FIX-1` details: sync directory `accessed_at`/`access_count` on cache compaction (+ read-only `cache_get` bug) & guard `export_encrypted` self-truncation
- **Status:** `VERIFIED (PR FIX-1 @ 8e09e60, 93e2620, v0.6.0)`
- **Context / Evidence:** `PLAN.md:3819-3834` (`PR FIX-1` spec), `mnemo/src/store.rs:2063-2080, 2146-2168, 2497-2518`.
- **Finding:** Two subtle details in `store.rs` affect `PR FIX-1`'s cache compaction and `export_encrypted` implementations:
  1. **Stale `accessed_at` / `access_count` in `CacheEntry` record bodies (`store.rs:2146-2165, 2497-2514`):** Like v5 memories, cache hits only bump `self.cache_dir[pos].accessed_at` and `self.cache_dir[pos].access_count` (`store.rs:2147-2148`); the `CacheEntry` record body on disk retains creation-time `accessed_at` and `access_count = 0`. Two consequences:
     - Bug today: on a `read_only` handle, `cache_get` (`store.rs:2161-2162`) and `cache_get_semantic` (`store.rs:2509-2510`) return `entry.accessed_at` (creation time!) alongside `self.cache_dir[pos].access_count` (post-hit count!), instead of returning `self.cache_dir[pos].accessed_at`.
     - Pitfall for `FIX-1` `compact_file`: if `compact_file` copies `CacheEntry` records or re-inserts via `cache_put`, reading `accessed_at`/`access_count` from `CacheEntry` resets every entry's stats to creation time / `0`.
  2. **`export_encrypted(dest)` self-path truncation hazard on Unix:** Because Unix `flock` is advisory, if a caller passes `dest` pointing to the same file as the open `Mnemo` handle (same path, symlink, or hardlink), `File::create(dest)` opens with `O_TRUNC` and **truncates the live open database to 0 bytes** before streaming from `&mut self.pager.file`.
- **Recommendation:**
  1. Fix read-only `cache_get` and `cache_get_semantic` (`store.rs:2162, 2510`) to return `self.cache_dir[pos].accessed_at` instead of `entry.accessed_at`; and in `compact_file`, source `accessed_at` and `access_count` from `old.cache_dir[pos]` (syncing them into both the rewritten `CacheEntry` and `CacheDirectoryEntry`, while preserving `created_at`, `ttl_secs`, `cost_hint_ms`, `key_preview`, `size_bytes`, `vector`, and `model`).
  2. In `Mnemo::export_encrypted(&mut self, dest)`, write to a sibling temporary file in `dest`'s parent directory, `sync_all()`, and atomically `std::fs::rename` onto `dest` (also rejecting `dest == self_path` if `Mnemo` stores or checks file identity) — preventing both self-truncation on Unix and partial/torn export files on crash.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → PR FIX-1 spec amended before dispatch. Verified the read-only `cache_get`/`cache_get_semantic` bug (returns the record body's creation-time `accessed_at` next to the directory's live `access_count`). Adopted: read-only fix + test; compaction takes `accessed_at`/`access_count` from the old directory entry and preserves the other fields; `export_encrypted` refuses the live file (Unix `(dev, ino)`, Windows canonical path), writes to a sibling temp, syncs, renames, and fsyncs the parent dir; tests for same-path/symlink/hardlink refusal.

---

### Batch `REV-003` — 2026-11-06 (`v0.6.0` Verification Sweep + Product Design: First-Class `Essence` & Seamless Conversation Compaction)

**`v0.6.0` / `v0.6.1` Verification Summary (`HEAD` `e244a39`):**
- Marked `R-001.1`, `R-001.2`, `R-001.5`, `R-001.6`, `R-001.8`, `R-001.11`, `R-002.1`, and `R-002.2` **`VERIFIED`** in `v0.6.0` (`0e51973`):
  - `store.rs:3217-3449`: `compact_file` preserves live exact+semantic cache entries with directory `accessed_at`/`access_count`; `verify()` validates live cache records.
  - `store.rs:2581-2597`: read-only `cache_get` / `cache_get_semantic` return directory `accessed_at`.
  - `store.rs:3101-3215`: `export_encrypted` checks `paths_refer_to_same_file`, writes to a sibling temp, `sync_all`, atomic `rename`, and Unix parent-dir `sync_all`; `rekey` commits a 1-frame WAL header transaction into a fresh WAL region before checkpointing page 0.
  - `store.rs:3260-3293`: `stats()` maintains a lazy in-memory `agent_set` (`O(1)` after first build); cache-only flushes no longer push PITR snapshots.
  - `mnemo-python/src/lib.rs`: `Option<Core>` handle release on `close()`, GIL released via `py.allow_threads`, Python embedder invoked outside mutable borrow, `Session` routed through closed-aware accessor.
- Confirmed `Recall scoring v2` (`R-001.12`) in-flight in the working tree: variant `V2` (`sim + max(sim, 0) * (beta * recency_created + gamma * (importance - 0.5) + delta * tanh(uses / 5.0))`) selected by the Planner on 2026-11-06 with `pass5 MRR = 0.9103`, `drift = 0`, `superseded_fact hit@1 = 1.000`.

---

#### R-003.1 — [PRODUCT] [HIGH] First-class `Essence` object & seamless conversation compaction (`Session::compact` / MCP `compact_conversation` / `mnemo essence`)
- **Status:** `OPEN`
- **Context / Evidence:** User product request (2026-11-06); `mnemo/src/session.rs:142-197` (`Session::{add_turn, close, discard}`), `mnemo/src/store.rs:1675-1697` (`Mnemo::about`), `mnemo/src/mcp.rs:28-35`, `PLAN.md:394` (`Phase 7.4`), `PLAN.md:783-849` (`Phase 12.1` & `Phase 12.2`).
- **Finding:** Today MNemo has a structural gap between **raw conversation turns** and **long-term brain orientation**:
  1. **`Session::close()` (`session.rs:165-182`) promotes every `Working` turn 1:1 into `Episodic` memory** with zero distillation, while `mnemo serve --mcp` (`mcp.rs:28-35`) exposes no session or conversation-compaction primitives at all.
  2. **Atomic `Memory` records vs holistic `Essence`:** Individual `Semantic`/`Episodic`/`Procedural` memories are **query-retrieved fragments** — an agent only sees them if its `recall` query vector matches. When an agent starts a new session or resumes immediately after a context-window `/compact`, it needs the **holistic distilled state ("Essence")** without guessing a search query:
     - **Session Essence (Conversation Handoff):** Distilled goal, current state / where we left off, key decisions made, and open threads / next steps from a compacted conversation.
     - **Brain / Topic Essence:** Living executive synthesis of the project, user, or domain (`"brain"`, `"project"`, `"architecture"`), whereas `Mnemo::about()` (`store.rs:1675-1697`) currently returns only static day-1 `area = "onboarding"` memories.
  3. **Timing mismatch in `PLAN.md`:** `Phase 12.2` (`v0.8.0`) schedules Claude Code `SessionStart`, `PreCompact`, and `SessionEnd` hooks plus MCP resources/prompts, while `Phase 7.4` (`Session::close_with`) is parked much later after Phase 2 and Phase 11 — leaving `Phase 12.2`'s `PreCompact` hook without a first-class compaction target.
- **Recommendation:** Introduce **`Essence`** as a first-class API & MCP concept paired with **atomic conversation compaction**, building directly on `Phase 12.1`'s keyed-upsert + `_mnemo.*` metadata architecture (**no format bump required in v8; `R-001.10`-compliant**):
  1. **First-class `Essence` type (distinct in API, unified in encrypted storage):**
     - Expose `Essence { id: Ulid, key: String, title: String, summary: String, decisions: Vec<String>, open_threads: Vec<String>, source_session_id: Option<String>, source_turn_count: usize, extracted_memory_ids: Vec<Ulid>, agent_id: String, created_at: i64, updated_at: i64 }`.
     - **Why store in `self.catalog` via `_mnemo.kind = "essence"` rather than a third on-disk directory (like `cache_dir`):**
       - Avoids a `format::VERSION` bump (`v8` stays compatible per `R-001.10`), avoids duplicating `compact_file` / `verify` / WAL / ANN machinery, and keeps `Essence` automatically covered by **PITR snapshots** (`snapshots()` / `restore_to()`) and vector search.
       - Standard ` Essence` keys: `"brain"` (living top-level synthesis), `"latest_session"` (rolling handoff slot updated on every conversation compaction), `"session:<id>"` (historical per-session essences), and `"topic:<slug>"`.
       - Separate cleanly at the API surface: `db.put_essence(...)`, `db.essence(key)` (`O(1)` via `Phase 12.1`'s lazy key index), `db.essences()`, and a filter on `RecallRequest` (exclude or include essences; default `recall` can either include or filter via `include_essence`).
       - Upgrade `Mnemo::about()` (and `Phase 12.2`'s `mnemo://about` + `mnemo://essence` MCP resources) to return both the static onboarding manifest *and* the live `"brain"` + `"latest_session"` Essences so any newly connected agent is oriented in one `<1 ms` read.
  2. **Seamless Conversation Compaction (`Session::compact` / `Mnemo::compact_session` / MCP `compact_conversation`):**
     - Because MNemo is an embedded engine without an internal LLM, the **host LLM** (in Claude Code `PreCompact` / `SessionEnd`, Cursor, MCP, or Python/Rust SDK) supplies the distillation in **one atomic call**:
       - `essence`: The distilled `Essence` (`title`, `summary`, `decisions`, `open_threads`).
       - `extracted_memories`: `Vec<Memory>` (or text facts embedded via the registered embedder) for durable `Semantic` facts and `Procedural` rules extracted from the conversation, supporting `Phase 12.1` `key` upserts so superseded facts update in place, each stamped with `_mnemo.from_essence = <essence_ulid>`.
       - `turn_policy: TurnCompactionPolicy`: what to do with the session's raw `Working` turns in the same transaction:
         - `Drop` (default: tombstones raw `Working` turns so transcript chatter never pollutes recall),
         - `ExpireAfter(ttl_secs)` (demotes raw turns to `TTL`-bounded `Episodic` traces that auto-expire after e.g. 7 days), or
         - `KeepAsEpisodic` (legacy `Session::close` behavior).
     - All three steps (write `"session:<id>"` + upsert `"latest_session"` Essence, upsert extracted `Semantic`/`Procedural` memories, apply `turn_policy` to raw `Working` turns) commit in a **single `flush()` WAL transaction**.
     - Also keep `Session::compact_with(|turns: &[Memory]| -> CompactionPlan)` in Rust/Python (`Phase 7.4`) with a deterministic extractive fallback when no LLM callback is provided.
  3. **Roadmap placement (`Phase 12` / `v0.8.0`):**
     - Do not touch `v0.7.0` (`NAME-1` + `Recall scoring v2`).
     - Pull `Phase 7.4` forward into **Phase 12** right between `12.1` (keyed upserts) and `12.2` (`PreCompact` / `SessionStart` hooks + MCP resources), so `Phase 12.2`'s `PreCompact` hook invokes `compact_conversation` to persist the `Essence` + extracted facts, and `SessionStart` injects `mnemo://essence` automatically.
- **Planner Triage:** _(pending)_
