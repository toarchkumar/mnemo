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
   - **Ignores the content of this file** during PR execution — `PLAN.md` and `AGENTS.md` remain the sole authoritative execution instructions for the Worker, ensuring untriaged review notes never derail an in-flight PR. As sole committer, the Worker commits `PLAN.md` and `reviewer_notes.md` by explicit path in a separate `chore(plan): sync coordination files` commit with each release (before tagging) or when the user asks for a backup.

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
| `R-001.4` | 2026-10-31 | `CORRECTNESS` | `MEDIUM` | CLI `mnemo recall` (default tracking mode) takes exclusive lock but drops without flushing stats or manifest | `VERIFIED` | Shipped in `v0.7.0` (`mnemo.rs:1785-1887`; minor exclusive-lock fallback msg edge case in `R-004.4`) |
| `R-001.5` | 2026-10-31 | `CORRECTNESS` | `MEDIUM` | Windows mandatory-lock failure in Python `export_encrypted` & `rekey(fast=False)` clobbering custom KDF params | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `93e2620`, `v0.6.0`) |
| `R-001.6` | 2026-10-31 | `CORRECTNESS` | `MEDIUM` | `EmbedCmd::embed` omits zero-vector protocol check on `recall` and `cache_get_semantic` | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `v0.6.0`) |
| `R-001.7` | 2026-10-31 | `PERF` | `HIGH` | `SEC-1` made pre-flush page-0 nonce-lease `sync_all` obsolete: cut `flush()` from 4 fsyncs to 2 (~35–50% faster writes) | `ACCEPTED` | Lease-write removal `VERIFIED` in PR DUR-1 (`4f1edc1`, `v0.6.0`); fsync merge -> Phase 11.1 / `PERF-1` |
| `R-001.8` | 2026-10-31 | `PERF` | `HIGH` | `Mnemo::stats()` is `O(N)` full-DB decrypt; cache/access flushes evict 256-slot PITR snapshots; `O(M log M)` sort in ANN search | `VERIFIED` | Part 3 in ANN-1.1 (`190a407`); Parts 1-2 in PR FIX-1 (`8e09e60`, `v0.6.0`) |
| `R-001.9` | 2026-10-31 | `PERF` | `MEDIUM` | ANN 1M-tier `n_probe = 50%` scaling ceiling & Residual IVF-PQ (`IVF-RPQ`) opportunity in format v9 | `DEFERRED` | Start of `FMT-9` (`v0.11.0`) / Phase 2 + 11.2 evaluation |
| `R-001.10` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | Positional MessagePack (`rmp_serde::to_vec`) forward-compatibility hazard on `format::VERSION = 8` | `VERIFIED` | Standing rule in PLAN.md + `V052IvfPqIndex` field-count guard in ANN-1.1 |
| `R-001.11` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | `PR PY-1` (`py.allow_threads` + `Option<Core>`): avoid `PyFnEmbedder` re-entrant borrow panic & cover `Session` | `VERIFIED` | Shipped in PR PY-1 (`abfaff3`, `v0.6.0`) |
| `R-001.12` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | `Recall scoring v2`: similarity-gated boosts, `created_at` vs `accessed_at` recency split, and `ranking_harness.py` timestamp variance | `VERIFIED` | Shipped in `v0.7.0` (`memory.rs:1128-1182`, `store.rs:4043-4052`) |
| `R-001.13` | 2026-10-31 | `PLAN-REFINEMENT` | `MEDIUM` | `Phase 12.4` Shared Access (`--shared`): `try_lock` contention & `compact_file` `rename` hazards on Windows/Unix | `ACCEPTED` | Phase 12.4 design-note requirements |
| `R-001.14` | 2026-10-31 | `PRODUCT` | `HIGH` | Pre-top-k structured metadata & `session_id` predicate filtering in `RecallRequest`, CLI, Python, and MCP | `VERIFIED` | Shipped in Phase 12.6 (`v0.8.0`, `store.rs:4019-4023, 4142-4174`) |
| `R-001.15` | 2026-10-31 | `PRODUCT` | `MEDIUM` | Token-budgeted context packing & Maximal Marginal Relevance (MMR) diversity in `recall` | `DEFERRED` | `max_bytes` budget `VERIFIED` in Phase 12.8 (`v0.8.0`); MMR diversity -> Phase 7.5 (harness-gated) |
| `R-001.16` | 2026-10-31 | `PRODUCT` | `MEDIUM` | Cognitive consolidation on `Session::close` & transactional brain diffing (`mnemo diff <txn_a> [txn_b]`) | `ACCEPTED` | diff -> Phase 12.5; consolidation/session -> Phase 12.1 & 12.7 (`commit_deltas`) |
| `R-001.17` | 2026-10-31 | `PRODUCT` | `IDEA` | Portable encrypted Knowledge Packs: read-only federated mounts (`attach_pack`) & scoped encrypted fork/merge | `DEFERRED` | After Phase 12.4 + v9 (`merge` picked up in Phase 13.7) |
| `R-002.1` | 2026-10-31 | `PLAN-REFINEMENT` | `HIGH` | `PR DUR-1` recovery design: 3 crash/truncation windows (relocation zero-before-write gap, 64-page scan cap, torn bytes outside `0..238` CRC) | `VERIFIED` | Shipped in PR DUR-1 (`4f1edc1`, `6bb5a84`, `22b4c1e`, `v0.6.0`) |
| `R-002.2` | 2026-10-31 | `PLAN-REFINEMENT` | `MEDIUM` | `PR FIX-1` details: sync directory `accessed_at`/`access_count` on cache compaction (+ read-only `cache_get` bug) & guard `export_encrypted` self-truncation | `VERIFIED` | Shipped in PR FIX-1 (`8e09e60`, `93e2620`, `v0.6.0`) |
| `R-003.1` | 2026-11-06 | `PRODUCT` | `HIGH` | Semantic Extraction Compaction (`Session::compact` / `commit_deltas`): discrete atomic deltas over rolling summaries | `VERIFIED` | Shipped in Phase 12.1 (`v0.8.0`) + Phase 12.7 (`v0.9.0` / `v0.9.1`, `89f1224`) |
| `R-003.2` | 2026-11-06 | `PLAN-REFINEMENT` | `HIGH` | Codify Semantic Extraction / SLM / DSPy GEPA philosophy in `AGENTS.md` & `PLAN.md` (narrow Phase 7.3 graph, prioritize token-budgeted recall) | `VERIFIED` | Shipped in `AGENTS.md`, `PLAN.md` standing rules, and Phase 12.8 (`v0.8.0`) |
| `R-004.1` | 2026-11-07 | `CORRECTNESS` | `HIGH` | `PERF-1` Step 2 (Deferred ANN index persistence): stale-index hazards on `update()`, `delete()`, `restore_to()`, `mcp` shutdown, and `open_read_only()` | `VERIFIED` | Shipped in `PERF-1` (`d143585`, `v0.10.0`) & v9 gen rule in `FMT-9b.1` (`4ac0e54`, `store.rs:1791, 6131`, `mcp.rs:253`) |
| `R-004.2` | 2026-11-07 | `PERF` | `HIGH` | `Phase 12.7` + `about` `O(N)` full-DB decrypt scans & ANN shortlist pollution from archived turns/essence/thread-links | `VERIFIED` | Shipped in `PERF-1` Step 2b (`d143585`, `v0.10.0`, `store.rs:3524, 6037`) + v9 inline catalog row tags/hashes in `FMT-9b.1` (`7fe63ea`, `4ac0e54`) |
| `R-004.3` | 2026-11-07 | `PLAN-REFINEMENT` | `HIGH` | `FMT-9` (`v0.11.0`) design hazards: free-list CoW/snapshot-overwrite crash window & Page-0 single-sector atomicity broken by `13.2` key slots | `VERIFIED` | Shipped in `FMT-9a` (`8066f50`: quarantined `freelist.rs`, ≤512 B Page 0 in `format.rs:127`, CoW AEAD `keyslot.rs`) |
| `R-004.4` | 2026-11-07 | `CORRECTNESS` | `LOW` | CLI `mnemo recall` lock fallback (`mnemo.rs:1787-1793`) prints misleading `"running recall read-only"` before failing when an exclusive writer holds the lock | `VERIFIED` | Shipped in `PERF-1` (`f8ecfe0`, `v0.10.0`, `mnemo.rs:1787-1798`) |
| `R-004.5` | 2026-11-07 | `PRODUCT` | `MEDIUM` | Roadmap vs Repo alignment: `README.md` `v0.10.0` drift, pulling `12.3` raw machine-key open forward before `MOBILE-1` (Argon2id 19 MiB / latency), `12.4` iOS constraint, and `14.1` compression padding | `ACCEPTED` | Parts (1)–(2) `VERIFIED` in `v0.10.0` (`01db7e7`, `a44ee64` `open_with_key`); Parts (3)–(4) folded into `12.4`, `FMT-9e`, `14.1` specs |
| `R-005.1` | 2026-11-08 | `CORRECTNESS` | `MEDIUM` | CLI `mnemo demo` fails at runtime with `MnemoError::Locked` (`mnemo.rs:2617-2618`) due to shadowed handle drop order after `db.close()` | `VERIFIED` | Shipped in `b08469b` (`v0.11.0`, `cli_smoke.rs:2161`) and consuming `Mnemo::close(self)` in `REFAC-1` (`5254395`, `v0.12.0`, `store/flush.rs:852`) |
| `R-005.2` | 2026-11-08 | `DX/DOCS` | `LOW` | Stale `mnemo/bindings/node` v0.4 skeleton (`Cargo.toml` pre-`NAME-1` dep) & post-`FMT-9` modularization of `mnemo/src/store.rs` (`8,104` lines) | `VERIFIED` | Shipped in `b08469b` (`mnemo/bindings/node/README.md` status note) and `REFAC-1` (`b0a0018`..`49feaea`, `v0.12.0`, 16 `mnemo/src/store/` submodules) |
| `RB-001.1` | 2026-11-09 | `PERF` | `HIGH` | Recall slows about 9× once the vector column passes its budget; under the mobile defaults that's about 21.7K rows at 384 dims (turns and tombstones count) | `ACCEPTED` | MOBILE-2 (v0.12.1): rec. 1, 3 and 5; rec. 2 → PERF-2; rec. 4 DEFERRED |
| `RB-001.2` | 2026-11-09 | `PERF` | `MEDIUM` | Coalesced flush policy and a background batch scope in the Swift and Kotlin stores (battery, flash writes, write latency) | `VERIFIED` | MOBILE-2 (v0.12.1), folded into RB-001.18; verified in RB-003 at `afd65b3` |
| `RB-001.3` | 2026-11-09 | `PERF` | `MEDIUM` | PERF-2 refinement: a lazy fill can't help the exact scan's first recall (the whole phone range); the levers are `pread` runs, decrypt in place and parallel page decrypt | `ACCEPTED` | PERF-2 spec |
| `RB-001.4` | 2026-11-09 | `PERF` | `MEDIUM` | Apple platforms: three `F_FULLFSYNC`s per commit; use `F_BARRIERFSYNC` where only ordering is needed | `ACCEPTED` | PERF-2 (with crash tests) |
| `RB-001.5` | 2026-11-09 | `PERF` | `MEDIUM` | `track_access` defaults to on in the mobile binding, so every recall dirties the catalog and every iOS background recall ends in a full commit | `VERIFIED` | MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-001.6` | 2026-11-09 | `PERF` | `LOW` | Library size and build graph: `clap` and `rpassword` are unconditional engine dependencies; per-package `opt-level` trial for cold crates | `REJECTED` | Size half refuted by RB-002; no time gain |
| `RB-001.7` | 2026-11-09 | `DX/DOCS` | `MEDIUM` | CI time: no `concurrency` cancel and no path filters, so every docs push runs all 18 jobs (three on macOS) | `VERIFIED` | MOBILE-2e (v0.12.2) (CI chore); verified in RB-003 at `afd65b3` |
| `RB-001.8` | 2026-11-09 | `DX/DOCS` | `MEDIUM` | `file_growth_per_single_write_bytes` (57,344) measures the first 20 commits only; steady state is about 2 KB per write | `ACCEPTED` | PERF-2 (bench methodology) |
| `RB-001.9` | 2026-11-09 | `PERF` | `IDEA` | One maintenance call for charging-and-idle windows (`BGProcessingTask`, `WorkManager`): compaction, cold-turn compression, index build | `DEFERRED` | PERF-2 spec, with automatic indexing |
| `RB-001.10` | 2026-11-09 | `PRODUCT` | `MEDIUM` | Adapters for the phones' own agent frameworks (Apple Foundation Models tools, Android on-device function calling): the mobile counterpart of the MCP server | `DEFERRED` | after INSP-1; a candidate for MOBILE-3 (the user picks) |
| `RB-001.11` | 2026-11-09 | `PRODUCT` | `IDEA` | Ready-made embedders as optional package targets (the mobile half of 12.3) | `ACCEPTED` | SAMPLE-1 (iOS target); Android recipe DEFERRED |
| `RB-001.12` | 2026-11-09 | `PRODUCT` | `IDEA` | React Native, Flutter and Kotlin Multiplatform packages from the same `mnemo-uniffi` crate | `DEFERRED` | on demand |
| `RB-001.13` | 2026-11-09 | `PRODUCT` | `IDEA` | Key policies for privacy-first apps: "only while unlocked" and biometric-gated stores | `DEFERRED` | MOBILE-3 or Phase 15 |
| `RB-001.14` | 2026-11-09 | `PRODUCT` | `IDEA` | The demo: export a phone's memory with a passphrase and open it in the inspector on a Mac; make it an INSP-1 acceptance test | `ACCEPTED` | INSP-1 acceptance test + SAMPLE-1 Export button, after SEC-2 |
| `RB-001.15` | 2026-11-09 | `PRODUCT` | `IDEA` | A community device table: one measured phone today; let sample-app users contribute `device_runs/*.json` | `ACCEPTED` | SAMPLE-1 |
| `RB-001.16` | 2026-11-09 | `PRODUCT` | `IDEA` | A mobile adoption kit: hosted API docs (DocC, Dokka), package-index listings and a two-screen quickstart | `ACCEPTED` | DOCS-1, after INSP-1 |
| `RB-001.17` | 2026-11-09 | `CORRECTNESS` | `HIGH` | `release.yml`: the mobile publish jobs still run when an engine build job fails (a skipped publish isn't a `failure`), so Maven Central and `mnemo-swift` can ship without crates.io or PyPI | `VERIFIED` | MOBILE-2 step 1 (v0.12.1), before any further tag; verified in RB-003 at `afd65b3` |
| `RB-001.18` | 2026-11-09 | `CORRECTNESS` | `HIGH` | The mobile stores make writes durable only at background, close or an explicit `flush()`; on Android, writes made in the background are never flushed | `VERIFIED` | MOBILE-2 (v0.12.1), with RB-001.2; verified in RB-003 at `afd65b3` |
| `RB-001.19` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | The store's lock is held while the app's embedder runs: calling the store from `embed` deadlocks, and a slow embedder blocks every call and the iOS background close | `VERIFIED` | MOBILE-2 (v0.12.1); verified in RB-003 at `afd65b3` |
| `RB-001.20` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | iOS background close: long calls can't be interrupted, a call proceeds on an already-expired activity, and app extensions never close the file | `ACCEPTED` | MOBILE-2e (v0.12.2); device check in the next device run |
| `RB-001.21` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | `MnemoKeystore` (Android): transient Keystore errors are reported as `KeyMissing`, the wrapping key can be regenerated over a live alias, and key files aren't fsynced | `VERIFIED` | MOBILE-2e (v0.12.2) (priority); verified in RB-003 at `afd65b3` |
| `RB-001.22` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | REFAC-1 `close(self)`: a failed final flush destroys the handle, so the buffered writes can't be retried (disk full on a phone) | `VERIFIED` | MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-001.23` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | FFI error mapping: `Kdf`, `Crypto` and `Serialize` become `Corrupt`; `NeedsWriteOpen` becomes `Invalid` and breaks the try-each-key probe; no retryable class | `VERIFIED` | MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-001.24` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | Caller-supplied `_mnemo.*` metadata is honoured as engine state (verified: such memories vanish from recall); reject it at every public write surface before 13.4 and 13.5 | `VERIFIED` | MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-001.25` | 2026-11-09 | `DX/DOCS` | `MEDIUM` | FMT-9 retention: the READMEs promise "the newest 16, plus the last 24 hours"; the pin cap left 6 restore points in a churn test | `ACCEPTED` | Docs in MOBILE-2; `stats` fields in INSP-1; the minimum-count option DEFERRED |
| `RB-001.26` | 2026-11-09 | `PLAN-REFINEMENT` | `MEDIUM` | FMT-9: no fuzz target covers the v9 page, column, delta, free-list, BM25 or key-slot decoders, and the decompression bound is 64 MiB for 7.9 KB records; needed before 14.1 | `ACCEPTED` | The bound in MOBILE-2e (v0.12.2); the fuzz targets before 14.1 |
| `RB-001.27` | 2026-11-09 | `PLAN-REFINEMENT` | `LOW` | The new Swift and Kotlin configurations expose `formatVersion: 8` one release before v8 becomes read-only | `VERIFIED` | MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-001.28` | 2026-11-09 | `CORRECTNESS` | `LOW` | Swift: changing `keyPolicy` on an existing store half-applies; the key probe does a full read-only open per stored key | `VERIFIED` | MOBILE-2e (v0.12.2) (part 1); part 2 REJECTED; verified in RB-003 at `afd65b3` |
| `RB-001.29` | 2026-11-09 | `DX/DOCS` | `LOW` | Release builds carry 18 `__*` test hooks (three mutate the file) and two `MNEMO_DEV_*` environment switches | `ACCEPTED` | HOOKS-1, with v0.13 |
| `RB-001.30` | 2026-11-09 | `SECURITY` | `MEDIUM` | Held in the gitignored `SECURITY.local.md` until the fix ships | `VERIFIED` | SEC-2 (v0.12.1); details in SECURITY.local.md; verified in RB-003 at `afd65b3` |
| `RB-001.31` | 2026-11-09 | `SECURITY` | `MEDIUM` | Held in the gitignored `SECURITY.local.md` until the fix ships | `VERIFIED` | SEC-2 (v0.12.1); details in SECURITY.local.md; verified in RB-003 at `afd65b3` |
| `RB-001.32` | 2026-11-09 | `SECURITY` | `LOW` | Held in the gitignored `SECURITY.local.md` until the fix ships | `ACCEPTED` | SEC-2 (v0.12.1); details in SECURITY.local.md |
| `RB-002.1` | 2026-11-09 | `CORRECTNESS` | `MEDIUM` | `release.yml`: the post-publish "Resolve the published package" step can't pass (it names a scheme Xcode doesn't generate), the dry run never runs it, and no release job has a timeout; the v0.12.0 run was still `in_progress` 45 minutes after everything had published | `VERIFIED` | Fixed in MOBILE-2a (`6bc592c`, `86e6836`, v0.12.1); verified on tag run 42919069975 and dry run 42901448023 (`6e32b68`) |
| `RB-002.2` | 2026-11-09 | `SECURITY` | `MEDIUM` | Held in the gitignored `SECURITY.local.md` until the fix ships (refines RB-001.30's fix and its acceptance test) | `VERIFIED` | SEC-2b, MOBILE-2e (v0.12.2); details in SECURITY.local.md; verified in RB-003 at `afd65b3` |
| `RB-002.3` | 2026-11-09 | `PERF` | `MEDIUM` | The first `stats()` on a handle decrypts every record (12 ms at 10K, 26 ms at 20K) for an agent list the mobile binding throws away; fix it before MOBILE-2 puts the recall mode in `stats()` | `VERIFIED` | MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-002.4` | 2026-11-09 | `PLAN-REFINEMENT` | `MEDIUM` | MOBILE-2 and the RB-001.22–.24 fixes: ten in-process regression tests that need no device or emulator, with today's result and the result to expect after each fix | `VERIFIED` | The test plan for MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `RB-002.5` | 2026-11-09 | `PERF` | `LOW` | What an app should do at the recall cliff: raising the budget by 1 MB restores 4.9 ms at 22K rows; `buildIndex` also works but took 50 s on an M1 Pro; the MOBILE-2 table should say which first | `VERIFIED` | Docs in MOBILE-2e (v0.12.2); verified in RB-003 at `afd65b3` |
| `R-006.1` | 2026-11-10 | `PRODUCT` | `HIGH` | Post-`INSP-1` sequencing: pull the Local Multi-Agent Harness stack (`12.4 + 12.3 + 12.2 + 13a`) ahead of `PERF-2`, splitting Phase 13 into Local Coordination (`13a`) vs Multi-Party Crypto (`13b`) | `ACCEPTED` | Order changed (harness stack ahead of PERF-2; 13a/13b); 12.4 design note first |
| `R-006.2` | 2026-11-10 | `PRODUCT` | `HIGH` | Personal-harness edge over Git (`CLAUDE.md`/`PLAN.md`/`Beads`): `git worktree` resolution, zero-embedder BM25 mode, `mnemo harness init` (Claude Code + Cursor), deterministic `--git` projection, and `--personal` brain mount | `ACCEPTED` | Folded into 12.2–12.5 |
| `R-006.3` | 2026-11-10 | `PLAN-REFINEMENT` | `HIGH` | Double-Run Dogfood Cutover (`HARNESS-DOGFOOD-1`): 2-PR dual-run orchestrating `Planner`/`Worker`/`Reviewer` through `test/project.mnemo` alongside `PLAN.md` to prove zero drift before retiring manual `WORKLOG` relay | `ACCEPTED` | HARNESS-DOGFOOD-1, with the parity checklist; private brain kept separate |
| `RB-003.1` | 2026-11-12 | `SECURITY` | `MEDIUM` | `mnemo inspect --open` passed the token on a command line, and the server kept the secret and plaintext with no idle stop (fixed in v0.13.1 as SEC-3; the write-up is now in PLAN.md's WORKLOG) | `VERIFIED` | Fixed in v0.13.1 (SEC-3); REVIEWER-B marks it VERIFIED at its next pass; verified in RB-005 (v0.13.1) |
| `RB-003.2` | 2026-11-12 | `SECURITY` | `LOW` | `mnemo export -o` wrote the plaintext into an existing file without tightening its permissions (fixed in v0.13.0, `6767b7a`; the write-up is now in PLAN.md's WORKLOG) | `VERIFIED` | Fixed in v0.13.0 (`6767b7a`); verified in RB-004 at `d350d53` |
| `RB-003.3` | 2026-11-12 | `CORRECTNESS` | `LOW` | A v8 file that crashed mid-commit gets two errors that point at each other (`compact` does fix it); a refused read-write open, even with a wrong passphrase, still replays the WAL into the file | `VERIFIED` | Rec. 1 before the tag; rec. 2 (key before replay; refuse old formats before any write) in v0.13.1; verified in RB-005 (v0.13.1) |
| `RB-003.4` | 2026-11-12 | `DX/DOCS` | `LOW` | The published crate doubles to 1.2 MB because the test fixtures ship in it, and the tests it ships don't compile from it; decide before the tag, since a published version can't change | `VERIFIED` | Option 1 before the tag: exclude tests, benches and the fixture generators from the crate; fixtures README; verified in RB-004 (the published 0.13.0 crate is 349,137 bytes) |
| `RB-003.5` | 2026-11-12 | `PLAN-REFINEMENT` | `MEDIUM` | The engine half of `release.yml` never runs before a tag, so each change to it (this time nine bumped actions) is first tried on the real one; and a failed wheel leg still lets crates.io publish | `VERIFIED` | Before v0.13.1 or the next release.yml bump: dry-run the engine builds; publishes wait for every build. For this tag the user holds approval until every build is green; verified in RB-005 (v0.13.1) |
| `RB-003.6` | 2026-11-12 | `DX/DOCS` | `LOW` | `--key-file` exists only on `inspect`: raw-key files can't be exported or converted from the CLI, and `FormatTooOld`'s advice doesn't work for a raw-key v8 file | `VERIFIED` | v0.13.1: --key-file on export and compact; FormatTooOld names compact_file_with_key; verified in RB-005 (v0.13.1) |
| `RB-003.7` | 2026-11-12 | `PERF` | `LOW` | The inspector clones every matching memory per request before paging and keeps the file in memory several times over (by reading; not measured) | `VERIFIED` | v0.13.1 fixes; a PERF-2 cell for load time and peak memory at 10K and 100K; verified in RB-005 (v0.13.1), the fix only |
| `RB-003.8` | 2026-11-12 | `DX/DOCS` | `LOW` | Small things: the CHANGELOG date, the "only its own lines" claim for keyed writes, Python's single `RuntimeError`, two edges of the mobile-off note | `ACCEPTED` | 1 and 2 before the tag; 3 docs before the tag, subclasses in v0.13.1; 4 with REL-2 |
| `RB-004.1` | 2026-11-12 | `SECURITY` | `LOW` | Held in the gitignored `SECURITY.local.md` until the fix ships (`mnemo inspect`, SEC-3's documentation) | `ACCEPTED` | Before the tag: the wording in three places. Details in SECURITY.local.md |
| `RB-004.2` | 2026-11-12 | `CORRECTNESS` | `MEDIUM` | A rekey that returns an error can still change the secret: the handle keeps the new key wrap, and the next flush commits it (older than v0.13.1; the in-place `rekey`, not the phone stores) | `ACCEPTED` | v0.13.2, first item (REKEY-1); the documentation sentence before the tag |
| `RB-004.3` | 2026-11-12 | `CORRECTNESS` | `LOW` | REL-2 runs the mobile-off note at the same moment as the asset attach, and both rewrite the Release body: the note can be lost with every job green | `VERIFIED` | Before the tag: `attach` in the note job's `needs`; verified in RB-005 (v0.13.1) |
| `RB-004.4` | 2026-11-12 | `PLAN-REFINEMENT` | `LOW` | The one release approval doesn't show the version or the mobile mode, and the mode is a repository variable flipped by hand around each tag | `ACCEPTED` | Rec. 1 and 3 before the tag; rec. 2 in v0.13.2 |
| `RB-004.5` | 2026-11-12 | `DX/DOCS` | `LOW` | Small things: a comment in `open.rs`, the torn-header advice for v8 files, `--idle-timeout` overflow, the page's text after an idle stop, `--open` with sandboxed Linux browsers, the root README, the T9a flake | `ACCEPTED` | 1, 3 and 7 (FLAKE-3) before the tag; 6 is the Planner's; 2, 4 and 5 in v0.13.2 |
| `RB-005.1` | 2026-11-12 | `SECURITY` | `LOW` | Held in the gitignored `SECURITY.local.md` until the fix ships (`release.yml`, the mode lines in `version check`) | `ACCEPTED` | Before the tag, with RB-005.4 items 1 to 3. Details in SECURITY.local.md |
| `RB-005.2` | 2026-11-12 | `CORRECTNESS` | `LOW` | When a failed rekey also fails to zero its new WAL region, the error doesn't say so, and a later damaged header lets the new secret recover the file (two I/O failures in a row, then a damaged header) | `ACCEPTED` | Next engine release, as REKEY-2, with RB-005.4 item 5 |
| `RB-005.3` | 2026-11-12 | `CORRECTNESS` | `LOW` | Python: where warnings are errors, a rekey that succeeded raises, against the docstring's "if it raises, the old secret is still in force" (by reading) | `ACCEPTED` | Before the tag: recommendation 1 and the docstring |
| `RB-005.4` | 2026-11-12 | `DX/DOCS` | `LOW` | Small things: the off switch's spelling, a dry run whose `mobile` input differs from the file, "no re-run can change it", `mnemo rekey`'s failure message, what `rekey_cut.rs` can't reach, a moved doc comment, a skipped job's raw name, "always" in the root README | `ACCEPTED` | 1 to 4 and 6 before the tag; 5 with REKEY-2; 7 won't fix; 8 done in the README handoff |
| `RB-006.1` | 2026-11-13 | `CORRECTNESS` | `HIGH` | 12.4 design note: it covers a process that dies, not a commit that fails in a process that lives on. Today such a handle is ahead of the file with an unchanged stamp, an error can still mean "committed", and step 6 doesn't reload after a replay | `ACCEPTED` | Before step 2: all five recommendations go into the note |
| `RB-006.2` | 2026-11-13 | `CORRECTNESS` | `MEDIUM` | 12.4 design note: what tells a handle the file changed. Every compaction makes a new data key, the note doesn't say what a failed seal means, and the stamp can repeat across files at one path | `ACCEPTED` | Before step 2: the seal is the stamp; the third outcome; open without create |
| `RB-006.3` | 2026-11-13 | `CORRECTNESS` | `MEDIUM` | 12.4 design note, mixed versions: an older `mnemo compact` can still lose a shared writer's commit, and today's lifetime lock is what prevents it (not reproduced with the identity check in place; silent when it happens) | `ACCEPTED` | Rec. 1 and 2 before step 2; rec. 3 decided as an upgrade note, revisited before step 8 |
| `RB-006.4` | 2026-11-13 | `PERF` | `MEDIUM` | 12.4 design note: writer preference works for one writer; the backoff costs more than the note's numbers say, and several writers have no order among themselves (a simulation of the protocol, not the engine) | `ACCEPTED` | Rec. 1 and 3 before step 2; 2, 4 and 5 in step 5's bench |
| `RB-006.5` | 2026-11-13 | `CORRECTNESS` | `MEDIUM` | 12.4 design note: presence by lock probe is wrong in several ordinary cases, and "a dead holder's claim lapses at once" turns each into a claim taken from a working agent; a holder never learns it lost a claim | `ACCEPTED` | Step 7, except rec. 5 (resolve the path) in step 2 |
| `RB-006.6` | 2026-11-13 | `CORRECTNESS` | `MEDIUM` | 12.4 design note, mobile: the presence lock is held between calls, which is what iOS kills for, and a wait for the lock inside a background activity can't be stopped (by reading) | `ACCEPTED` | Rec. 2 (a cancellable wait) before step 2; the rest with mobile's opt-in |
| `RB-006.7` | 2026-11-13 | `CORRECTNESS` | `MEDIUM` | 12.4 design note: what a shared handle keeps or does that the note doesn't cover (`cache_get` writes, per-handle snapshot retention, the embedding, read-then-write, statistics at close, the watcher's descriptor and cost) | `ACCEPTED` | Items 1 and 2 before step 2; the rest at their steps |
| `RB-006.8` | 2026-11-13 | `CORRECTNESS` | `LOW` | 12.4 design note, incremental catch-up: a cached page is never checked again, so the note's safety argument for stale pages doesn't hold (by reading; the first version is safe) | `ACCEPTED` | Rec. 1 now (fix the sentence); the rest in step 6 |
| `RB-006.9` | 2026-11-13 | `DX/DOCS` | `LOW` | 12.4 design note, smaller points: `lock-info` written before the lock, the Linux filesystem list, a directory the caller can't write, read-only callers and a pending WAL, the per-call cost, Windows, the "no descriptor" sentence | `ACCEPTED` | Items 1 and 3 in step 2; the rest as edits to the note |
| `RB-006.10` | 2026-11-13 | `SECURITY` | `LOW` | Held in the gitignored `SECURITY.local.md` until the fix ships (shipped code; the lines of `maintenance.rs` and `load.rs` that v0.14.0 step 1 changes) | `ACCEPTED` | In step 1's PR. Details in SECURITY.local.md |
| `RB-006.11` | 2026-11-13 | `SECURITY` | `MEDIUM` | Held in the gitignored `SECURITY.local.md` (the 12.4 design of the lock layer; no code exists) | `ACCEPTED` | (a) and (b) before step 2; (c) now in the note, the choice at 12.3. Details in SECURITY.local.md |
| `RB-006.12` | 2026-11-13 | `SECURITY` | `MEDIUM` | Held in the gitignored `SECURITY.local.md` (the 12.4 design of the daemon; no code exists) | `ACCEPTED` | Before step 9 (the daemon). Details in SECURITY.local.md |

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
- **Status:** `VERIFIED (v0.7.0 — mnemo.rs:1885-1887; see R-004.4 for minor exclusive-lock fallback msg edge case)`
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
- **Status:** `VERIFIED (v0.7.0 — memory.rs:1128-1182, store.rs:4043-4052)`
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
- **Status:** `VERIFIED (Phase 12.6, v0.8.0 — store.rs:4019-4023, 4142-4174)`
- **Context / Evidence:** `mnemo/src/memory.rs:342-406` (`RecallRequest`), `mnemo/src/store.rs:1464-1565`, `mnemo/src/mcp.rs:640-750`.
- **Finding:** `Memory` encourages rich structured `metadata: Map<String, Value>` (`area`, `topic`, `repo`, `ticket`, `role`, `source`) plus `session_id`, and `Phase 12.3` plans `delete_where(predicate)` for bulk deletion — yet **`RecallRequest` has no metadata or `session_id` filter at all** (only `memory_types` and `agent_id`). Callers today must over-fetch `top_k` and post-filter in application code, which fails catastrophically whenever one prolific project/session dominates the top `n_rerank` / `top_k` slots and starves the target metadata slice.
- **Recommendation:** Share a unified, lightweight `Filter` / metadata-predicate type across **both `RecallRequest` and Phase 12.3's `delete_where` / `export`**:
  - Support `session_id`, exact key-value match (`meta_eq("area", "security")`), set membership (`$in`), numeric/string range, and key existence.
  - Apply the predicate **before** `top_k` truncation (and in ANN mode, ensure filtered queries either expand candidate probing or push filter awareness into candidate selection so selective filters don't return fewer than `top_k` existing matches).
  - Expose as `--where key=value` on CLI `recall`/`list`, `metadata_filter={...}` in Python `recall`/`recall_text`, and `filter: {...}` on MCP `recall`/`list_memories`. Preserves the single-file invariant with zero format break.
- **Planner Triage:** `ACCEPTED` (2026-11-01) → NEW Phase 12.6 (after 12.1): a shared `Filter` (session_id, metadata eq/$in/range/exists) applied before top-k in recall/list/export/delete_where, ANN mode widening candidates until top_k matches; CLI `--where`, Python `metadata_filter=`/`session_id=`, MCP `filter`. No format change. Verified `RecallRequest` has only type/agent filters today.

#### R-001.15 — [PRODUCT] [MEDIUM] Token-budgeted context packing & Maximal Marginal Relevance (MMR) diversity in `recall`
- **Status:** `DEFERRED (max_bytes budget VERIFIED in Phase 12.8 v0.8.0 @ store.rs:4090, 4207-4240; MMR diversity -> Phase 7.5)`
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

### Batch `REV-003` — 2026-11-06 (`v0.6.0` Verification Sweep + Semantic Extraction Compaction & SLM / DSPy GEPA Architectural Alignment)

**`v0.6.0` / `v0.6.1` Verification Summary (`HEAD` `2dc69db`):**
- Marked `R-001.1`, `R-001.2`, `R-001.5`, `R-001.6`, `R-001.8`, `R-001.11`, `R-002.1`, and `R-002.2` **`VERIFIED`** in `v0.6.0` (`0e51973`).
- Confirmed `Recall scoring v2` (`R-001.12`) landed in `3eb687c` (with pre-tag fixes queued by `PLANNER-A` in `PLAN.md:5736-5757`).

---

#### R-003.1 — [PRODUCT] [HIGH] Semantic Extraction Compaction (`Session::compact` / `commit_deltas`): discrete atomic deltas over rolling summaries
- **Status:** `VERIFIED (Phase 12.1 v0.8.0 + Phase 12.7 v0.9.0/v0.9.1 @ 89f1224)`
- **Context / Evidence:** User architectural context & product evaluation (2026-11-06); `mnemo/src/session.rs:142-197` (`Session::{add_turn, close, discard}`), `mnemo/src/mcp.rs:28-35`, `PLAN.md:394` (`Phase 7.4`), `PLAN.md:783-849` (`Phase 12.1` & `Phase 12.2`).
- **Finding:** Evaluating conversation compaction and the concept of "Essence" against MNemo's core **Semantic Extraction Philosophy** exposes a critical distinction between **what to avoid (Rolling Narrative Summaries)** and **what MNemo actually needs (Atomic Delta Extraction)**:
  1. **Why Rolling Narrative Summaries ("Text-based Compaction") are an anti-pattern for MNemo:**
     - Storing a paragraph-style `Essence { summary: String }` or injecting rolling conversation summaries into `mnemo about` causes **Summary Drift** (smoothing away exact identifiers, file paths, ULIDs, and hard constraints over multi-session horizons) and **bloats the active context window** (increasing prefill latency for local SLMs and DSPy GEPA loops).
  2. **Why `Session::close()` (`session.rs:165-182`) today is also broken:**
     - `Session::close()` currently promotes every raw `Working` turn 1:1 into `Episodic` memory. A 50-turn dialogue injects 50 noisy, conversational chunks into the vector store, diluting retrieval precision and wasting prefill tokens.
  3. **What "Essence" and Conversation Compaction mean under Semantic Extraction:**
     - The "essence" of a conversation is **not** a narrative summary — it is the **extracted delta of discrete, atomic, identifier-preserving facts** (`Semantic` facts/constraints, `Procedural` rules, `Episodic` state changes) distilled from the dialogue before the raw turns are discarded.
- **Recommendation (Revised 2026-11-06 to align strictly with Semantic Extraction & Small-Context SLMs):**
  1. **Reject narrative summary objects; build Batched Atomic Delta Compaction (`Mnemo::commit_deltas` / `Session::compact` / MCP `compact_session`):**
     - Input: `deltas: Vec<FactDelta>` where each `FactDelta` is a discrete, isolated statement (`content`, `memory_type: Semantic | Procedural | Episodic`, `key: Option<String>`, `importance: f32`, `metadata`), plus `turn_policy: TurnCompactionPolicy` (`Drop` [default], `ExpireAfter(ttl_secs)`, or `KeepAsEpisodic`).
     - **Single-batch embedding + single-WAL-commit execution:**
       - In text mode (`commit_deltas_text` / Python `db.commit_deltas` / MCP `compact_session`), embed all `K` delta strings in **one batched `Embedder::embed(&texts)` call** (critical for local ONNX / SLM latency in DSPy GEPA loops).
       - Upsert each discrete fact into the append-only WAL catalog: when `key` is supplied (`Phase 12.1`), deterministically supersede the prior constraint holding that `key` (`_mnemo.supersedes = <old_ulid>`) so state transitions resolve conflicts without a mutable graph.
       - Atomically delete (or TTL-expire) the session's raw `Working` turns in the **same `flush()` transaction**.
  2. **If `essence` is exposed as a read surface (`db.essence()` / `mnemo essence` / MCP `mnemo://essence`):**
     - Define it strictly as a **token-budgeted Pinboard of Active Keyed Constraints** (e.g., live keyed `Semantic`/`Procedural` facts above an importance floor, capped to a strict token/byte budget such as `<= 256 tokens`), **never** an unindexed rolling prose summary.
  3. **Roadmap placement (`Phase 12` / `v0.8.0`):**
     - Fold `commit_deltas` / `Session::compact` into **Phase 12** right alongside `12.1` (keyed upserts) and `12.2` (`PreCompact` / `SessionEnd` hooks), pulling `Phase 7.4` forward and replacing `7.4`'s generic summarization wording with discrete delta extraction.
- **Planner Triage:** `NEEDS-USER` (PLANNER-B, 2026-11-06). Agree on the diagnosis: rewriting a prose summary from scratch drifts (ACE, arXiv 2510.04618, calls it "context collapse" and "brevity bias"), and `Session::close` promoting turns 1:1 is noise. **Accepted now, no user call needed:** (a) `commit_deltas` as the one-call batch write (one batched `embed`, keyed upserts, one `flush`) → PLAN.md Phase 12.7. It must validate and embed every item before staging any of them, because `remember` stages immediately and there's no rollback (store.rs `put`), so a mid-batch error would otherwise leak a partial batch into the next flush. (b) Keyed supersession appends a new record and marks the old one `_mnemo.superseded_by` / `_mnemo.invalid_at` (Graphiti-style invalidation, not erasure) → a 12.1 amendment. This also gives the new fact a fresh `created_at`, which scoring v2's recency term needs. (c) Every read surface gets a byte budget. **Changed:** the `turn_policy` default is `ExpireAfter(7d)`, not `Drop`. Extraction is lossy, and Factory's compression evaluation found artifact and identifier tracking the weakest area for every method, so keep the raw turns as a short recovery window. `turn_policy` applies only to SDK `Session` users anyway; MCP/Claude Code clients never write their turns into mnemo. **The fork for the user:** the Reviewer's pinboard only *filters* existing keyed facts. My recommendation ("Option C") is that Essence is a separate kind of item: **task state, not world facts**. Items are sectioned (`goal`, `state`, `decision`, `open`, `next`, `constraint`), have a lifecycle (open → resolved), are edited by engine-applied ops (add / update / resolve / remove; never an LLM full rewrite, so they can't drift by construction), are loaded by scope rather than by query, and are rendered under a byte budget. This matches Letta's always-in-context core memory blocks, Factory's sectioned state, ACE's itemized deltas, and Claude Code's own compaction sections. Write timing: the agent maintains it **incrementally** through MCP tools while it works. Compaction-time extraction can't be the main path, because Claude Code's `PreCompact` is a shell hook that can't drive the main model; `SessionStart` (`source=compact`) re-injects the render instead. Full spec draft: PLAN.md Phase 12.7.
  - **Resolution (PLANNER-A covering lane B, 2026-11-06): `ACCEPTED`, modified by user decision.** The user defined essence as the *continuous flow* of a long working thread, preserved and backed up so a new thread continues with the same experience, and distinct from memory (retrieved key knowledge). Neither the pinboard nor Option C covered *how we got here*. Adopted design (PLAN.md 12.7, v0.9.0), keeping the Reviewer's no-prose-blob rule:
    - (a) a **lossless, idempotent turn archive** per thread (raw history is the backup; Factory found identifier and artifact tracking weakest under any compression);
    - (b) **essence** = an append-only timeline (milestones are never edited, so the narrative can't drift; ACE) plus Option C's delta-edited sections, now including `conventions` and `lessons`, with project scope and thread continuation (`--from`);
    - (c) `commit_deltas` as triaged, with `turn_policy` defaulting to `Archive`;
    - (d) automatic capture through Claude Code's PreCompact/SessionEnd hooks, using `transcript_path`, in 12.2, after 12.4.

    One MCP `essence` tool, not four. The write-path pieces ship earlier: v0.8.0 = 12.1 + 12.6 + 12.8 (user decision: small releases).

#### R-003.2 — [PLAN-REFINEMENT] [HIGH] Codify Semantic Extraction / SLM / DSPy GEPA philosophy in `AGENTS.md` & `PLAN.md` (narrow Phase 7.3 graph, prioritize token-budgeted recall)
- **Status:** `VERIFIED (AGENTS.md + PLAN.md standing rules + Phase 12.8 v0.8.0)`
- **Context / Evidence:** User architectural context (2026-11-06); `AGENTS.md:287-294` (`## Where the philosophy lives`), `PLAN.md:361-397` (`Phase 7`), `PLAN.md:5577-5598` (`BP.1` exact-identifier cosine miss in `scoring v2`).
- **Finding:** The maintainer's **Architectural Context** establishes three load-bearing operating assumptions that should govern `AGENTS.md` and `PLAN.md` phase specs:
  1. **Semantic Extraction over Rolling Summaries** (discrete atomic deltas, zero summary drift).
  2. **Append-Only Atomic Log over Rigid Context Graphs** (state changes handled by appending new keyed constraints in the WAL, resolving conflicts deterministically without brittle relational schemas).
  3. **Aggressively Small Context Windows & Exact Identifier Durability** (built for local SLMs and DSPy GEPA loops that swap discrete constraints in/out in milliseconds to minimize prefill compute, while preserving exact IDs, paths, and rules).
- **Recommendation:**
  1. **Update `AGENTS.md` (`## Where the philosophy lives`) and `PLAN.md`:** Document these three operating assumptions alongside the single-file invariant so every Planner, Worker, and Reviewer designs against them.
  2. **De-scope / narrow `Phase 7.3` (Memory links graph) & `Phase 7.2`'s graph-degree dependency:** `Phase 7.3` (`relations: Vec<(Ulid, RelationKind)>` on `Memory` + live in-degree traversal) edges toward the *Pure Persistent Context Graph* anti-pattern and violates `R-001.10` if appended to `Memory` under v8. Instead, rely on `Phase 12.1`'s deterministic **keyed supersession (`metadata.key` + `_mnemo.supersedes`)** in the append-only WAL; if lightweight provenance is needed, `_mnemo.supersedes` (already planned in `12.1`) suffices without a general relational graph.
  3. **Elevate Token-Budgeted Recall (`R-001.15`: `max_bytes` / `token_budget` + MMR diversity) and `Phase 2` (BM25 + Vector Hybrid):**
     - Local SLMs and GEPA loops with tiny context windows need `RecallRequest` to enforce a hard byte/token ceiling and MMR diversity so prefill never bloats with redundant hits.
     - `Phase 2`'s BM25 lexical leg directly enforces Assumption 3 (*exact identifiers preserved and retrieved*), fixing the exact `"what did BP.1 ship?"` cosine miss proven in the `v0.7.0` harness (`PLAN.md:5577-5598`).
- **Planner Triage:** `ACCEPTED` with modifications (PLANNER-B, 2026-11-06). (1) The three principles go into PLAN.md "Standing technical rules", and into AGENTS.md "Where the philosophy lives" through a Planner HANDOFF. They're reworded as engineering rules ("write state as discrete items through deltas; never rewrite wholesale", "supersede, don't erase", "every read surface has a byte budget; exact identifiers must survive"). DSPy GEPA and local SLMs are named as a motivating workload, not a principle: GEPA is a prompt optimizer, and its relevance is a persona choice. Caveat for "millisecond swap-in": each open pays the Argon2id KDF, so only long-lived handles (MCP, SDK, 12.4's in-memory key) get millisecond reads; one-shot CLI calls don't. (2) Phase 7.3's general relation graph is **DEFERRED**, not dropped: it needs a persisted-field change (R-001.10, so v9 at the earliest) and has no evidence behind it yet. Provenance uses `_mnemo.supersedes` / `_mnemo.superseded_by` / `_mnemo.derived_from` metadata keys. Phase 7.2's connectivity guard becomes "never decay live keyed, pinned or Essence-referenced items". (3) R-001.15 is promoted from Phase 7.5 to **Phase 12.8**: a `max_bytes` budget on `RecallRequest` (the engine has no tokenizer, so bytes; document roughly 4 bytes per token), plus MMR only if the ranking harness shows it doesn't cost hit@1. Essence rendering reuses the same budget code. Phase 2's BM25 leg stays the identifier fix, ordered as before (v9).


---

### Batch `REV-004` — 2026-11-07 (`v0.7.0`–`v0.9.1` Verification + `PLAN.md` Product Direction vs Live Repo Audit @ `7fad106` + In-Flight `PERF-1`)

**`v0.7.0`–`v0.9.1` Verification & Working-Tree Check (`HEAD` `7fad106`):**
- Marked `R-001.4` (`v0.7.0`), `R-001.12` (`v0.7.0`), `R-001.14` (`Phase 12.6`, `v0.8.0`), `R-003.1` (`Phase 12.1` `v0.8.0` + `Phase 12.7` `v0.9.0`/`v0.9.1`), and `R-003.2` (`AGENTS.md` + `PLAN.md` + `Phase 12.8` `v0.8.0` `max_bytes`) **`VERIFIED`** in shipped code.
- Checked the Worker's uncommitted `PERF-1` Step 1 diff in `mnemo/src/wal.rs` (`slice-by-8` CRC-32 + 29-byte frame-0 `txn_id <= wal_seq` short-circuit in `wal::recover`): bit-identical to IEEE 802.3 CRC-32, eliminates the `wal_pages * 8 KiB` (`26 MB` at 100K) heap allocation and full-region scan on both `Mnemo::open` and `Mnemo::open_read_only` (`store.rs:1515`), and leaves `DUR-1`'s streaming `wal::scan_committed_txn` untouched.
- **Strategic assessment of the new `PLAN.md` direction vs the live repo:**
  - Positioning MNemo around **on-device agents/apps (desktop → phone/tablet → edge)** + **shared encrypted team brains (`Phase 13`) + zero-knowledge record-changeset sync (`Phase 14`)** ("the encrypted brain your agents share and carry", vs server-keyed "Supabase for Agents") plays directly to MNemo's unique Rust + single-file + AEAD strengths.
  - Running `P-0` (`7fad106`) *before* building `12.4`, `13`, `14`, or `MOBILE-1` was the single highest-leverage move on the board: it proved that while MNemo's **semantic API surface** (`v0.7`–`v0.9.1`) is already close to the product vision, its **v8 physical storage engine** (`8.7 KB/memory`, `449 ms` + `+20.8 MB` file growth per single write at 100K, `158 MB` open RSS, and `O(N)` full-DB decrypts in `12.7`) cannot ship to mobile or multi-agent shared files until `PERF-1` (`v0.10.0`) and `FMT-9` (`v0.11.0`) land.

---

#### R-004.1 — [CORRECTNESS] [HIGH] `PERF-1` Step 2 (Deferred ANN index persistence): stale-index hazards on `update()`, `delete()`, `restore_to()`, `mcp` shutdown, and `open_read_only()`
- **Status:** `VERIFIED` (`d143585` / `v0.10.0`, extended for v9 in `4ac0e54`)
- **Context / Evidence:** `PLAN.md:8655-8660` (`PERF-1` Step 2 spec), `mnemo/src/store.rs:1409-1452, 1474-1565` (`open` / `open_read_only`), `store.rs:1685-1718` (`put`), `store.rs:1908-1911` (`delete`), `store.rs:2153-2234` (`update`), `store.rs:4471-4485, 4540-4550` (`flush` + `Snapshot`), `mnemo/src/mcp.rs:251`.
- **Finding:** `PERF-1` Step 2 proposes skipping ANN index persistence on `flush()` (persisting only on `close()` or every `N = 64` commits / `M = 1,000` new memories) and catching up on `open` by assigning *"any catalog entries whose id is missing from the index (`pending`)"*. Tracing `store.rs` exposes four correctness/latency traps if catch-up only checks `id` absence on `Mnemo::open`:
  1. **In-place `update()` / `essence_update()` keeps the same `Ulid` with a new vector (`store.rs:2226-2229`):** `update()` rewrites a record under its existing `id: u128` at a fresh `start_page` with a changed `vector`. In a stale on-disk `IvfPqIndex`, `ann.assignment` *already contains* `id` — mapped to its **old IVF centroid and old PQ code**. A reopen pass that only checks `!ann.contains(id)` silently skips every updated record, leaving it permanently mis-indexed in the wrong cluster until a full `rebuild_index()`.
  2. **Tombstoned IDs (`delete`, `thread_forget`, `essence_remove`):** `delete()` (`store.rs:1908-1910`) removes `id` from `self.ann` in RAM, but if the index isn't persisted before exit/crash, the stale on-disk `ann` retains dead IDs in its posting lists — consuming part of `ann.query`'s fixed `n_rerank` shortlist budget before `score_once` filters `!e.deleted` at `store.rs:3976`.
  3. **`open_read_only()` (`store.rs:1474`) & `restore_to()` (`store.rs:4540-4550`):** `Snapshot` entries pushed during the 63 skipped commits pair a newer `(catalog_start, catalog_pages)` with an older `(index_start, index_pages)`. Both `open_read_only()` and `restore_to()` load that `(catalog, stale_ann)` pair and will serve wrong/missing ANN results unless they run the exact same in-memory reconciliation as `Mnemo::open`.
  4. **MCP server shutdown (`mcp.rs:251`) calls `db.flush()`, not `db.close()`:** When `mnemo serve --mcp` exits cleanly on stdin EOF, `mcp.rs:251` calls `db.flush()`. If only `close()` forces a dirty deferred index to disk, every clean MCP shutdown leaves up to 63 commits / 999 records unpersisted in `ann`, forcing every subsequent `open_read_only` (`mnemo recall --no-track`, `mnemo about`) to re-decrypt those delta pages on every open.
- **Recommendation:**
  1. **Zero-format-change reconciliation rule under v8's monotonic allocator:** Extract a single `reconcile_ann_with_catalog(&mut pager, &catalog, &mut ann, header.index_start)` helper and call it in **`Mnemo::open`, `Mnemo::open_read_only`, and `Mnemo::restore_to`**:
     - First, remove any `id` present in `ann` whose catalog entry is missing or `deleted`.
     - Second, under format v8's strictly monotonic `header.next_page`, when `flush()` seals `idx_bytes` at `header.index_start` (`store.rs:4474`), every record already reflected in that persisted index has `entry.start_page < header.index_start`, whereas **every record inserted OR `update()`d in later commits has `entry.start_page > header.index_start`**. Re-adding every live (`!deleted`) catalog entry where `!ann.contains(entry.id) || entry.start_page > header.index_start` catches 100% of both new inserts and in-place vector updates in `O(catalog.len())` integer checks with zero v8 format change. (Note: `FMT-9`'s free-page list will break monotonic `start_page`, so `FMT-9` will replace this with incremental index deltas / `modified_txn`.)
  2. Switch `mnemo/src/mcp.rs:251` from `db.flush()` to `db.close()` so clean MCP server shutdown persists any dirty ANN index once at session end, while one-shot CLI writes (`mnemo remember` / `update`) keep calling `db.flush()` for fast `<20 ms` commits.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-07). Verified: `put()` adds to `self.ann` for every record, `update` rewrites under the same id at a new page, and `close()` exists (store.rs ~L4750) but MCP exit calls `flush()`. Your monotonic `start_page > header.index_start` rule is correct for v8, because record pages are allocated before the flush that writes the index run, so records in that flush are in the persisted index. Folded into PERF-1 step 2: a single `reconcile_ann_with_catalog` helper called from `open`, `open_read_only` and `restore_to`, which prunes dead, deleted or non-catalog ids and re-adds `!ann.contains(id) || start_page > index_start`. MCP stdin-EOF persists the index (`close()` or `flush_index()`). Tests: `update` → crash before index persist → reopen → recall finds the new vector's neighbours; `delete` → reopen → no dead id in the shortlist; `restore_to` an older snapshot with a stale index; a read-only open of a file with pending memories. FMT-9 replaces the rule with incremental index deltas.

#### R-004.2 — [PERF] [HIGH] `Phase 12.7` + `about` `O(N)` full-DB decrypt scans & ANN shortlist pollution from archived turns/essence/thread-links
- **Status:** `VERIFIED` (`d143585` / `v0.10.0` + `FMT-9b.1` `7fe63ea`, `4ac0e54`)
- **Context / Evidence:** `mnemo/src/store.rs:1715-1718` (`put` -> `ann.add`), `store.rs:2561, 2570-2613, 2622-2641` (`apply_ingest`, `collect_turn_ids_in_thread`, `max_seq_in_thread`, `thread_export`), `store.rs:2949-3000` (`essence`), `store.rs:3841-3898` (`about`, `about_essence`), `store.rs:3972, 4001, 4033-4041` (`score_once`).
- **Finding:** Two `Phase 12.7` interactions with `store.rs` severely impact the new On-Device latency targets as a `.mnemo` file accumulates real conversation archives:
  1. **Repeated `O(N_total)` full-database AES-GCM decrypts on routine session-start & ingest calls:**
     - `ingest_thread` / `commit_deltas` calls `collect_turn_ids_in_thread` (`store.rs:2574-2589`, decrypting *every* live record in `self.catalog`, contrary to the doc comment on L2569) and immediately calls `max_seq_in_thread` (`store.rs:2596-2612`, decrypting *every* live record in `self.catalog` **a second time**).
     - Session-start `about` + `about_essence` in CLI/MCP calls `about()` (`store.rs:3846`, 1st full-DB decrypt), then `about_essence()` (`store.rs:3876-3892`, up to a 2nd full-DB decrypt when no open project essence exists), and when one *does* exist, calls `self.essence(EssenceScope::Project, ...)` (`store.rs:3896` -> `store.rs:2967-2968`, another full-DB decrypt!). Unlike `agent_set` (`R-001.8`) and `key_index`/`superseded` (`store.rs:705-706`), none of these are cached on the handle — so even a warm MCP/mobile handle pays 2–3 full-file decrypts (~2–3 s at 100K on M1 Pro; multi-second stalls on mobile) per session start or turn ingest.
  2. **Archived turns, essence items, and sentinel thread-links pollute `self.ann` and starve default `recall` (`store.rs:1715-1718, 4033-4041`):**
     - `apply_ingest` (`store.rs:2561`), `essence_add`, and `thread_start` (`_mnemo.kind = "thread_link"`, carrying an all-equal sentinel vector) all write via `self.put(m)`, which unconditionally inserts every record into `self.ann` (`store.rs:1716`).
     - Yet default `recall` (`score_once`, `store.rs:4033-4041`) excludes `turn`, `essence`, and `thread_link` **after** `ann.query` has already truncated to `n_rerank` and **after** `self.read_memory(e)` has decrypted each candidate (`store.rs:4001`)! Because real agent usage produces 10–50x more raw turns than distilled facts (e.g. 50K archived turns vs 2K facts), turns dominate `ann.query`'s `n_rerank` shortlist, starve `scored.len() < req.top_k`, trigger `recall_budgeted`'s 2x widen + **fallback to an `O(N)` full-file exact scan (`store.rs:4169-4171`)**, and degrade IVF/PQ centroid quality.
- **Recommendation:**
  1. **Immediate (`PERF-1` / `v0.10.0`, no format bump):**
     - Skip inserting `MNEMO_META_KIND_THREAD_LINK` (sentinel vector) and `MNEMO_META_KIND_ESSENCE` (read by scope, never by similarity — `store.rs:4027`) into `self.ann` in `put()` / `build_index()`. (If `_mnemo.kind = "turn"` must remain searchable when `req.filter.mentions_mnemo_meta()`, keep turns out of the primary fact `ann` or filter `!turn_ids.contains(&id)` inside `ann.query` when `!req.filter.mentions_mnemo_meta()` so turns never consume the fact `n_rerank` budget!)
     - Merge `collect_turn_ids_in_thread` + `max_seq_in_thread` into one pass, and add lazy handle-scoped index sets (`essence_ids: HashSet<u128>`, `onboarding_ids: HashSet<u128>`, `turn_ids: HashSet<u128>`, maintained in `put`/`delete`) populated in the same single scan as `ensure_key_caches()` so warm handles render `about` / `about_essence` / `essence` in `< 1 ms` (`O(essence_items)`).
  2. **Structural (`FMT-9` / `v0.11.0`):** Inline a 1-byte `kind` tag (`Fact | Turn | Essence | ThreadLink | Onboarding`) + `superseded` bit + optional `thread_hash` / `key` directly into v9's catalog row so even cold handles never decrypt non-matching records on `about()`, `essence()`, `ingest_thread()`, or `recall()`.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-07). Verified: `put()` adds every kind to the ANN, and `collect_turn_ids_in_thread` and `max_seq_in_thread` each read every live record. **PERF-1 new step 2b:**
  - `essence` and `thread_link` records are never added to the ANN.
  - Turns go into no main ANN either. Kind-filtered recall over turns (`--thread X` or `kind: turn`) scans that thread's cached turn-id list exactly, and a cross-thread turn search is documented as an exact scan.
  - One lazy handle scan (folded into `ensure_key_caches`) builds `key_index`, `superseded`, `essence_ids`, `onboarding_ids` and `thread → {turn_ids, max_seq}`, maintained incrementally in `put`/`delete`.
  - Ingest does one pass. `build_index`/`rebuild_index` exclude those kinds, and the first lazy scan prunes them from an existing persisted index.
  - **Gates at 100K:** warm `about` + `about_essence` + `essence` ≤ 1 ms, and a warm single-turn ingest does no full scan.

  The **cold**-handle cost stays one scan until **FMT-9** adds a kind tag, a superseded bit and thread/key hashes to the v9 catalog row (accepted into FMT-9).

#### R-004.3 — [PLAN-REFINEMENT] [HIGH] `FMT-9` (`v0.11.0`) design hazards: free-list CoW/snapshot-overwrite crash window & Page-0 single-sector atomicity broken by `13.2` key slots
- **Status:** `VERIFIED` (`FMT-9a` `8066f50`)
- **Context / Evidence:** `PLAN.md:1314-1325` (`Phase 13.2` key slots in v9), `PLAN.md:1569-1589` (`FMT-9` spec), `mnemo/src/format.rs:70-116, 268-287` (Page-0 CRC + seal layout), `mnemo/src/store.rs:1488-1500, 4456-4555` (`flush` CoW order + `Snapshot` retention + `DUR-1` seal policy).
- **Finding:** Before the `FMT-9` design note is written, two low-level storage/crypto invariants in the current engine directly collide with `FMT-9`'s bullet list if not designed together:
  1. **Free-page list (`PLAN.md:1579-1583`) vs `pager.flush()`-before-`wal::commit` & retained PITR snapshots:**
     - In `Mnemo::flush()` (`store.rs:4461-4498`), dirty data pages are written to their home page numbers and fsynced **before** `wal::commit` logs the transaction. In v8 this is crash-safe *only* because `header.next_page` is append-only (no committed state or snapshot references `>= next_page`).
     - Under `FMT-9`'s free-page list, if transaction `T` frees page `P` (via `delete`, `update`, packed-page compaction, or catalog delta merge) and re-allocates `P` for new data written during `T` itself (before `T` commits), a crash before `wal::commit` leaves the pre-`T` committed header pointing at `P` — whose contents were just overwritten with `T`'s new ciphertext/nonce (`CorruptPage` data loss!). Likewise, reusing `P` while any retained `Snapshot` in `self.manifest` (`max_snapshots = 256`) still references `P` silently corrupts `restore_to()`.
  2. **32 key slots in Page 0 (`PLAN.md:1316, 1574`: `~3.2 KB`) breaks `DUR-1`'s single-sector atomicity invariant (`R-002.1` item 3):**
     - Today's v8 header occupies bytes `0..294` (`format.rs:107-113`), fitting inside a single 512-byte atomic sector. `DUR-1` (`R-002.1` triage amendment, 2026-11-01) explicitly relied on this single-sector fit to declare `CRC-valid + seal-invalid` impossible on power loss and therefore treat *every* `validate_seal` failure as `HeaderTampered` (never WAL-healed; `store.rs:1488-1490`).
     - Placing 32 wrapped-DEK slots (`~100 B × 32 ≈ 3.2 KB`) into Page 0 spans **7–8 physical 512-byte sectors**. A power cut mid-Page-0 checkpoint can write sector 0 (valid `0..238` CRC!) while leaving sectors 1..7 torn — causing either KEK/DEK unwrap failure on a torn slot or `HeaderTampered` with zero WAL recovery!
- **Recommendation:** Bake both invariants into the `FMT-9` design note upfront:
  1. **Epoch/txn-quarantined free list:** Track freed pages as `(freed_at_txn, page_no, page_count)`. A freed range may be allocated in transaction `T` **only if** `freed_at_txn < last_committed_txn` (never freed in `T` itself) **and** `freed_at_txn` is older than the earliest retained `Snapshot` (or snapshot retention explicitly pins/copies pages).
  2. **Ping-pong double headers (Pages 0 & 1) or externalized key-slot page:** Either keep Page 0 `<= 512` bytes (covering *all* pre-seal bytes in CRC-32) and store the `13.2` key-slot directory in a CoW encrypted/authenticated page run (`key_slots_start`), **or** adopt **ping-pong headers on Pages 0 and 1** (each carrying full-header CRC-32 + AEAD seal + monotonic `wal_seq`, alternating `wal_seq & 1`). Ping-pong Page 0/1 also eliminates `DUR-1`'s `O(file_size)` `MWAL` scan on torn-header open!
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-07). Both become **required sections of the FMT-9 design note**.
  - (a) A free-page list keyed by `(freed_at_txn, page_no, page_count)`. A page is reusable only once its freeing txn has committed **and** it's older than the earliest retained snapshot (snapshot retention pins pages). Include a crash test: free and reuse in the same txn is impossible.
  - (b) **Preferred:** keep Page 0 ≤ 512 bytes, with the whole pre-seal region under the CRC, and put the 13.2 key slots in a CoW authenticated page run (`key_slots_start`). Ping-pong headers on Pages 0/1 must be **evaluated** as the alternative, since they could also remove DUR-1's file-wide torn-header scan. The design note picks one, with crash analysis.

#### R-004.4 — [CORRECTNESS] [LOW] CLI `mnemo recall` lock fallback (`mnemo.rs:1787-1793`) prints misleading `"running recall read-only"` before failing when an exclusive writer holds the lock
- **Status:** `VERIFIED` (`f8ecfe0`, `v0.10.0`)
- **Context / Evidence:** `mnemo/src/bin/mnemo.rs:1785-1795`, `mnemo/src/store.rs:749-760`, `mnemo/tests/cli_smoke.rs:1511-1565`.
- **Finding:** When default `mnemo recall` calls `Mnemo::open(path, pp)` and hits `Err(MnemoError::Locked { .. })`, `mnemo.rs:1788-1792` unconditionally prints `"mnemo: another writer holds the lock on {path}; running recall read-only with --no-track"` to `stderr` *before* calling `Mnemo::open_read_only(path, pp)`.
  - When a **reader** holds a shared lock (`LOCK_SH`, the case tested in `cli_smoke.rs:1529`), `open_read_only` succeeds, so `"another writer holds the lock"` is inaccurate (it was a reader).
  - When an **actual writer** (e.g. `mnemo serve --mcp`) holds an exclusive lock (`LOCK_EX`), `open_read_only`'s `try_lock_shared` (`store.rs:752`) *also* fails immediately with `MnemoError::Locked` — so the CLI promises `"running recall read-only with --no-track"` on stderr and crashes on the very next line with `Locked`.
- **Recommendation:** In `mnemo/src/bin/mnemo.rs:1787-1793`, attempt `Mnemo::open_read_only(path, pp)` *first* inside the `Err(Locked)` branch: if it returns `Ok(db)`, print `"mnemo: shared lock held on {path}; running recall read-only with --no-track"` and proceed; if it also returns `Err(Locked)`, return that `Locked` error directly without printing the false fallback message.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-07). Verified at bin/mnemo.rs ~L1787–1792. Fix: in the `Locked` branch, try `open_read_only` first. On success, print "file is busy; recall ran read-only without tracking". On `Locked`, return the error with no fallback message. Extend cli_smoke with an exclusive-holder case. Ships in PERF-1.

#### R-004.5 — [PRODUCT] [MEDIUM] Roadmap vs Repo alignment: `README.md` `v0.10.0` drift, pulling `12.3` raw machine-key open forward before `MOBILE-1` (Argon2id 19 MiB / latency), `12.4` iOS constraint, and `14.1` compression padding
- **Status:** `ACCEPTED` (Parts 1–2 `VERIFIED` in `v0.10.0` `01db7e7`, `a44ee64`; Parts 3–4 in `12.4` / `FMT-9e` / `14.1` specs)
- **Context / Evidence:** `README.md:176-180, 215-220`, `mnemo/src/crypto.rs:57-63` (`KdfParams::secure` = `19_456 KiB`), `PLAN.md:72-80, 1076-1096, 1185-1201, 1385-1415, 1550-1604`.
- **Finding:** Comparing `PLAN.md`'s new On-Device + Team Brain + Sync product direction against the live repo and release queue surfaces four alignment gaps:
  1. **Stale `v0.10.0` promise in `README.md:216-218`:** `README.md` currently states *"A Claude Code plugin with hooks that save before compaction and load at session start comes with shared access in v0.10.0."* With the On-Device track override (`v0.10.0 = PERF-1`, `v0.11.0 = FMT-9`, `v0.12.0 = MOBILE-1`), shared access + the Claude Code plugin moved to `v0.13.0+`. Also `README.md:178` cites `~19 ms` durable single write without noting corpus size (`449 ms` at 100K until `PERF-1`/`FMT-9` land), and neither `README.md` nor `index.html` yet mentions the on-device / multi-device encrypted brain direction.
  2. **Sequencing inversion between `MOBILE-1` (`v0.12.0`) and `12.3` generated 256-bit key (`PLAN.md:1554, 1592-1603`):** `MOBILE-1` targets **`open <= 10 ms` and `RSS <= 50 MB` on iPhone/Android**, integrating with iOS Keychain / Android Keystore — yet `Phase 12.3` (which introduces the random 256-bit non-passphrase key so machine/keychain opens skip Argon2id!) is scheduled *after* `MOBILE-1` and `12.4`. Today `Mnemo::open` *always* runs `KdfParams::secure()` (`m_cost = 19,456 KiB ≈ 19 MiB`, `t_cost = 2`; `crypto.rs:57-62`), which eats **38% of the entire `50 MB` mobile RSS budget** (risking iOS App Extension / Widget Jetsam kills) and takes `~15 ms` on M1 Pro (`> 15–30 ms` on mobile CPUs, blowing the `<= 10 ms` open gate before reading page 1!).
  3. **`Phase 12.4` (per-op locking vs daemon) has platform-split winners:** iOS/Android app sandboxes prohibit spawning background daemon processes (`fork`/`exec`), whereas desktop multi-agent setups (`Claude Code` + `Cursor` + CLI) strongly benefit from a warm daemon that holds the unlocked key, catalog/index, and embedder in RAM.
  4. **`FMT-9` (`zstd`) + `Phase 14.1` (zero-knowledge record-changeset sync) compression side-channel:** `14.1` notes the sync relay *"learns only sizes and timing"*, while `13.5` mixes `public | team | private` records in one shared brain. Compressing unpadded per-record sync payloads leaks exact compressed byte lengths, and sharing a compression frame/dictionary across untrusted `public` inputs and `private` secrets creates a CRIME/BREACH-style length oracle.
- **Recommendation:**
  1. Hand off a small `README.md` edit removing the hardcoded `"in v0.10.0"` promise on L218 (say `"on the roadmap"` / after the on-device storage pass) and scoping L178's write latency note.
  2. **Pull `Mnemo::open_with_key` / raw 256-bit key-slot unwrap (`0 KiB` KDF RAM, `< 0.2 ms` HKDF/AES-KW unwrap) forward into `FMT-9` (`v0.11.0`, or `PERF-1`) BEFORE `MOBILE-1` (`v0.12.0`)** so iOS Keychain, Android Keystore, and desktop Keychain handles hit `< 10 ms` open and `< 50 MB` RSS out of the gate (leaving CLI keychain UX / base32 recovery-code commands for `12.3` if desired).
  3. Frame `12.4` not as "locking OR daemon", but as **bounded-backoff per-op locking as the universal portable base** (used on mobile / iOS App Groups / serverless) + **optional desktop local daemon** when multiple CLI/MCP processes share a file.
  4. In `FMT-9` + `14.1`, pad encrypted network sync envelopes to power-of-two/bucketed byte sizes and never compress records of different `13.5` visibility scopes (`public` vs `private`) inside the same `zstd` frame unless padded to fixed 8 KiB pages.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-07).
  - (1) **Fixed now:** the README's "in v0.10.0" promise is gone, and the write-latency row is scoped by file size. The site's "Next" column now shows the on-device work. Both are handed off. The full on-device positioning change comes with the v0.10.0 docs.
  - (2) **`open_with_key` moves into PERF-1 (step 7), no format change:** `Mnemo::create_with_key` / `open_with_key(&[u8; 32])` record minimum Argon2id parameters in the header, because a random 256-bit key needs no stretching, so there's about 0 KDF RAM and sub-ms derivation. Also in Python. Keychain and recovery-code UX stay in 12.3 and MOBILE-1.
  - (3) **12.4 brief reframed:** bounded-backoff per-op locking as the portable base (iOS/Android have no daemon), plus an optional desktop daemon. That matches the Worker's parked draft.
  - (4) **FMT-9 + 14.1:** page compression only within a single 13.5 visibility scope, and sync envelopes padded to bucketed sizes. Both are added to the specs.

---

### Batch `REV-005` — 2026-11-08 (Full Rust Codebase Substance & Robustness Audit @ `7103b96` / `FMT-9b.1`)

**`REV-004` Verification & Audit Summary (`HEAD` `7103b96`):**
- Verified **`R-004.1`**, **`R-004.2`**, **`R-004.3`**, and **`R-004.4`** in shipped code (`v0.10.0` `d143585`, `FMT-9a` `8066f50`, `FMT-9b.1` `7fe63ea` / `4ac0e54`) and Parts (1)–(2) of **`R-004.5`** (`01db7e7`, `a44ee64`). Notably, `written_after_index` (`store.rs:1791-1797`) cleanly avoids re-decrypting pre-index turns/essence/links while catching 100% of post-index inserts and updates across both v8 (`start_page > index_start`) and v9 (`gen == 0 || gen > index_gen`), and `veccol.rs` ships an exhaustively verified (`0..=65535`) dependency-free IEEE-754 fp16 implementation.
- **Full-repo Rust depth audit:** `mnemo/src/` contains `14,262` pure executable SLOC (`20,516` lines with docs/blanks) + `1,800` executable SLOC in `mnemo-python/src/lib.rs` + `8,287` executable test SLOC across `262` Rust `#[test]`s (`mnemo/tests/` + unit tests), `5` `cargo-fuzz` targets, and `7` `criterion` benches — with `#![forbid(unsafe_code)]`, `0` `todo!`/`unimplemented!`, `0` `.unwrap()`s in `store.rs`, and zero external DB/ANN wrapper dependencies. Executing pre-built release binaries uncovered one live CLI regression (`R-005.1`) and two minor structural hygiene items (`R-005.2`).

---

#### R-005.1 — [CORRECTNESS] [MEDIUM] CLI `mnemo demo` fails at runtime with `MnemoError::Locked` (`mnemo.rs:2617-2618`) due to shadowed handle drop order after `db.close()`
- **Status:** `VERIFIED` (`b08469b` / `v0.11.0`, plus consuming `Mnemo::close(self)` in `REFAC-1` `5254395` / `v0.12.0`)
- **Context / Evidence:** `mnemo/src/bin/mnemo.rs:2617-2618`, `mnemo/src/store.rs:6806-6817`, `AGENTS.md:55` (`cargo run --bin mnemo -- demo`), `mnemo/tests/cli_smoke.rs` (38 tests, none invoking `demo`). Reproduced live with `./mnemo/target/release/mnemo demo --path /tmp/mnemo-demo-audit.mnemo` at `HEAD` `7103b96` (`exit 1`: `error: file already locked by another handle`).
- **Finding:** `demo()` (`mnemo/src/bin/mnemo.rs:2617-2618`) executes:
  ```rust
  db.close()?;
  let db = Mnemo::open(path, pp)?;
  ```
  `Mnemo::close(&mut self)` (`store.rs:6809`) takes `&mut self` — it forces ANN index persistence and calls `self.flush()`, while `self.pager.file` remains open and holds the exclusive `fs4` OS advisory lock until `db` is dropped. In Rust, `let db = Mnemo::open(path, pp)?;` evaluates the right-hand side `Mnemo::open` *before* dropping the shadowed `db` binding, so `try_lock_exclusive` collides with the process's own still-open `File` handle and aborts the demo on its final step. Because `mnemo demo` is advertised in `AGENTS.md:55` and CLI help as the one-command first-run tour, a new user running `mnemo demo` hits an immediate failure.
- **Recommendation:**
  1. Insert `drop(db);` immediately after `db.close()?;` in `mnemo/src/bin/mnemo.rs:2617` (before `let db = Mnemo::open(path, pp)?;`), and add a `cli_smoke_demo` test in `mnemo/tests/cli_smoke.rs`.
  2. Add a one-sentence caution to `Mnemo::close(&mut self)`'s rustdoc (`store.rs:6806-6808`) warning callers to `drop(db)` before reopening the same path in the same scope (`let db = Mnemo::open(...)` evaluates before dropping a shadowed `db`). (Keep `&mut self` on `close()` since `mcp.rs:253` and `mnemo-python`'s `self.inner.take()` rely on it.)
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-08). Verified at bin/mnemo.rs ~L2617: `db.close()?; let db = Mnemo::open(path, pp)?;`, and `close(&mut self)` only flushes; the doc says the file is released on drop. Not a new regression: `git log -S` shows the code is from the initial commit, so it has been broken since file locking arrived (v0.4.0). **Fix:** `drop(db)` before reopening, a `close()` rustdoc caution, and a cli_smoke `demo` test. It lands as **its own small commit at the next FMT-9 sub-PR boundary** (it doesn't touch `store.rs` logic, so no conflict with FMT-9) and ships in v0.11.0. A v0.10.1 hotfix is only worth it if the user asks: `demo` isn't in the README quick start. **Follow-up (REFAC-1, after FMT-9):** evaluate whether `close()` should release the file and lock itself (a closed-state handle where later calls return a clear error). A method named `close` that keeps the lock is a footgun for every Rust caller, not just `demo`.

#### R-005.2 — [DX/DOCS] [LOW] Stale `mnemo/bindings/node` v0.4 skeleton (`Cargo.toml` pre-`NAME-1` dep) & post-`FMT-9` modularization of `mnemo/src/store.rs` (`8,104` lines)
- **Status:** `VERIFIED` (`b08469b` / `v0.11.0` + `REFAC-1` `b0a0018`..`49feaea` / `v0.12.0`)
- **Context / Evidence:** `mnemo/bindings/node/Cargo.toml:14` (`mnemo = { path = "../.." }`), `mnemo/bindings/node/src/lib.rs:1-245`, `mnemo/src/store.rs` (`8,104` total lines / `5,632` SLOC).
- **Finding:** Two repository-hygiene observations surfaced during the full Rust audit:
  1. `mnemo/bindings/node/` (`excluded` from the workspace in `mnemo/Cargo.toml:8`) is an unmaintained early `napi-rs` skeleton (~v0.4 vector-only API) whose `Cargo.toml` still declares `mnemo = { path = "../.." }` without `package = "mnemo-engine"` — after `PR NAME-1` (`v0.7.0`), `cargo check` inside `mnemo/bindings/node` will fail Cargo dependency resolution, and anyone inspecting the repository tree may mistake the 245-line file for current Node.js parity.
  2. `mnemo/src/store.rs` has grown to `8,104` lines (`5,632` pure SLOC — ~40% of the entire library crate in a single file), concentrating lifecycle/migrations, CRUD, recall scoring, result-cache methods, threads/ingest, essence, and v8/v9 dual paths.
- **Recommendation:**
  1. When convenient (or in Phase 5), either update `mnemo/bindings/node/Cargo.toml` (`mnemo = { package = "mnemo-engine", path = "../.." }`) + add a top-of-file/README status note marking `bindings/node` as an experimental Phase 5 preview, or remove/park it until Phase 5 builds the real `napi-rs` package.
  2. **After `FMT-9f` completes** (do *not* split while the Worker is actively modifying `store.rs` for `FMT-9`), consider splitting `store.rs` into focused private submodules under `mnemo/src/store/` (`open.rs`, `recall.rs`, `threads.rs`, `essence.rs`, `cache_api.rs`, `compact.rs`) implementing blocks on `Mnemo` with zero public API change.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-08). (1) **Part of the finding is stale:** `mnemo/bindings/node/Cargo.toml` already declares `mnemo_engine = { package = "mnemo-engine", path = "../.." }` (updated in NAME-1, v0.7.0), and `src/lib.rs` uses `mnemo_engine::`. The *API* is still the v0.4 vector-only skeleton, so add a short status note (a README or top-of-file comment: "experimental preview, not published; the real package comes in Phase 5"). (2) **Accepted as REFAC-1, after FMT-9f and before MOBILE-1:** split `store.rs` (8,113 lines) into private submodules under `mnemo/src/store/` (open/lifecycle, recall, threads, essence, cache API, compact/migration) with zero public API change. Fold in the `close()` evaluation from R-005.1. Never split while FMT-9 is modifying `store.rs`.

---

### Batch `RB-001` — 2026-11-09 (REVIEWER-B: whole-project assessment, optimization and product items, then the first review of MOBILE-1a–1d.2, FMT-9, REFAC-1 and the CI and release workflows)

**What was read.** Code at `46e9da7` (`release: v0.12.0`, branch `release-0.12.0`). `release.yml` was last changed in `8a9af2e`. By the time this batch was written the tree was at `f79715e`: three more commits, docs and coordination files only (`git diff 46e9da7..f79715e -- .github mnemo` is empty). **`v0.12.0` was tagged at `f79715e` and pushed while this batch was being written** (2026-11-09 19:13 PST), so every item below describes the code that shipped in v0.12.0, and fixes land in a later release.

**How claims were checked.**
- **Code:** file and line references are to `46e9da7`.
- **Measured data:** `test/scripts/perf_v0.5.0.json` (`device_budget_v0_11_0`, `device_budget_mobile`, `first_recall_10k_profile`) and `test/scripts/device_runs/iphone18_3.json`.
- **RB scratch runs:** small Python scripts outside the repo, against the published `mnemo-engine==0.11.0` aarch64 Linux wheel (hardware AES), on raw-key v9 files at 384 dims. The engine paths they exercise are the same code at `46e9da7`, moved by REFAC-1. They're labelled "RB scratch run" below. Numbers from them are for ratios, not absolute phone timings.
- **Not run:** `cargo test` (no Rust toolchain in this sandbox), the Swift and Kotlin test suites, anything on a device.
- **Read-only:** nothing but this file and the gitignored `SECURITY.local.md` was written. `test/project.mnemo` was not opened.

**Existing items checked first.** Nothing below re-raises a REJECTED item. Overlaps with accepted work are cited in place: R-001.7 (fsync count), R-001.13 (12.4 shared access), R-005.1 (`close()`), PERF-2, SAMPLE-1, 12.3, 12.5, 13.2, 14.1, 14.2/INSP-1 and Phase 15.

#### Project assessment (plain English)

**What MNemo is.** One encrypted file that is an agent's memory: facts that can be superseded by key, whole conversation threads, a running "essence" of where the work stands, and a cache for expensive calls. It ranks recall on similarity, recency, importance and use. It's reached from Rust, Python, a CLI, an MCP server and, as of this release, Swift and Kotlin packages.

**How well the code delivers on it.**
- **Architecture: strong.** The layers are clean: format, crypto, pager and WAL at the bottom; the store, now 15 modules; thin, curated bindings on top. The single-file rule holds everywhere, even on phones, where the key id is an extended attribute on the file rather than a sidecar. Format v9 is a real storage-engine design: copy-on-write pages, a quarantined free list, delta chains for the catalog and index, and page encryption bound to a generation.
- **Code health: good.** About 25,000 lines of Rust in the engine and binding, `forbid(unsafe_code)`, exactly pinned dependencies, `cargo deny`, clippy with warnings as errors, and comments that explain why. The weak spots are leftovers of speed: 18 hidden test hooks and two environment switches compiled into release builds (RB-001.29), and v8 and v9 code paths living side by side.
- **Tests and CI: deep for the engine, thin at the new edges.** The engine has crash-injection property tests, five fuzz targets, a three-OS matrix, two MSRV jobs, size gates, a simulator and an emulator job, and a release dry run. The gaps: the fuzzers predate v9 (RB-001.26); the mobile store layers' failure paths (disk full, Keystore errors, an expired background activity) have no tests; and exactly one phone has been measured.
- **Docs: unusually honest.** Trade-offs, known limitations and measured numbers are stated plainly. A few statements are stronger than the code: snapshot retention (RB-001.25), "never with the database locked" (RB-001.19), and the extension paragraph of the Swift README (RB-001.20).

**How well it fits the current direction.**
- **On-device, size and speed first: the numbers are real.** On an iPhone, a 10K-memory file is 1.33 KB per memory on disk and 7.3 MB in memory, opens in 2.1 ms and answers a warm recall in 1.7 ms. Four of five phone targets pass, and the library adds about 2.6 MB.
- **The gap is behaviour, not speed.** The Swift and Kotlin stores are thin wrappers over a contract shaped for desktop tools: the caller flushes, one process owns the file, the vector budget assumes a small file, and the embedder blocks. Phone apps will meet those four things first (RB-001.1, .18, .19, .20).
- **"An engine for now" fits,** and the inspector next is the right call: it's also the demo (RB-001.14).
- **The order INSP-1, then PERF-2, then Phases 12–15 is reasonable, with one caveat.** Two mobile hardening items shouldn't wait behind all of it: the durability policy (RB-001.18 with .2) and the recall cliff (RB-001.1). Both are small next to a phase.

**The three biggest strengths.**
1. **Storage rigor well beyond the project's age:** a WAL with crash-injection tests, copy-on-write with a quarantined free list, authenticated pages, and a fast, public response to its one security bug (SEC-1).
2. **A measure-first habit:** every performance claim points at a cell in `perf_v0.5.0.json`, and designs are chosen by gates (int8 vectors were dropped at −0.0165 recall; fp16 passed at −0.0005).
3. **A coherent product shape:** one portable encrypted file with agent semantics (keyed supersession, threads, essence, byte budgets) that behaves the same on six surfaces and ships through one release pipeline.

**The three biggest risks.**
1. **Mobile behaviour at the edges is unproven.** One flagship iPhone, no Android hardware, no low-end device. In the store layers, writes aren't durable until the app leaves the foreground, the embedder runs under the store lock, and recall falls off a cliff past the vector budget.
2. **The release and key-handling surface grew faster than its hardening.** Four registries hang off one workflow with a gating hole (RB-001.17), and three security items are in `SECURITY.local.md`.
3. **A long roadmap on a narrow base.** One maintainer, one serialized Worker queue, an 880 KB `PLAN.md`, and Phases 12 to 15 still ahead, while the adoption surface for mobile developers (docs, embedders, framework adapters) is thin. The risk isn't quality; it's that the engine gets far ahead of anyone using it.

---

**Part 1a — Optimization areas**

#### RB-001.1 — [PERF] [HIGH] Recall slows about 9× once the vector column passes its budget; under the mobile defaults that's about 21.7K rows at 384 dims (turns and tombstones count)
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `mnemo/src/store/records.rs:1091-1099` (`ensure_column`): the column is resident only if `self.catalog.len() * entry_len(dims)` fits `vector_cache_budget`. That's every catalog row: thread turns, essence items and tombstones too.
  - `records.rs:795-812`: a resident column that grows past the budget is dropped.
  - `mnemo/src/store/recall.rs:423-445`: a row that isn't resident goes through `column_entry` (`records.rs:1065-1085`), which calls `Pager::read_page` (`mnemo/src/pager.rs:324-345`). That returns an owned copy of the whole 8 KB page for each row and pushes vector pages through the page cache.
  - `mnemo/bindings/uniffi/src/types.rs:43-48`: the mobile defaults are a 16 MiB budget and a 256-page cache.
  - Nothing builds an index on its own: `build_index` runs only when the app calls it (`mnemo/src/store/ann.rs:15-56`).
  - **RB scratch run** (10K × 384, warm, median of 59 recalls): resident **2.56 ms**; budget lowered to 4 MiB with the 256-page cache **22.19 ms** (8.7×); the same with a 1,024-page cache 4.49 ms.
- **Finding:**
  - An entry is `dims × 2 + 4` bytes (`mnemo/src/veccol.rs:28-30`), so 16 MiB holds 21,732 rows at 384 dims, 16,320 at 512 (the width SAMPLE-1 plans for `NLContextualEmbedding`) and 10,894 at 768.
  - Past that, every recall decrypts the whole column again. A chat app that archives threads gets there on turns alone, even with a few thousand facts, because turns count toward the budget although recall never scores them.
  - The on-device track's phone range is "typically 1K–50K memories" (`PLAN.md`, On-device track), so the cliff is inside it. No error or stat tells the app it happened.
- **Recommendation:**
  1. Size the resident column by the rows recall actually scans (facts), not by `catalog.len()`.
  2. Make the paged path cheap: when the column isn't resident, scan a vector page at a time with one decrypt and no page-cache insert (`fill_column_page` already decodes that way), instead of one `read_page` copy per row. Estimated cost of an over-budget recall at 10K: about 7 ms instead of 22 (one pass of decrypt plus the scan).
  3. Tell the app: `stats()` reports the recall mode (`exact_resident`, `exact_paged`, `indexed`), and the Swift and Kotlin READMEs get a rows-by-dimension table.
  4. Consider building the index automatically when a v9 file crosses `V9_ANN_MIN_FACTS`, or at least when the column stops fitting. RB-001.9 is the place to do it off the hot path.
  5. Add a device-budget cell for "over budget" so the regression can't come back unseen.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-09). Verified: `ensure_column` sizes the resident column by `self.catalog.len()` (`records.rs:1096-1098`), so turns, essence items and tombstones count against the budget. **In MOBILE-2:** (1) size it by the rows recall scans (live facts); (3) `stats()` reports the recall mode (`exact_resident`, `exact_paged`, `indexed`), and the Swift and Kotlin READMEs get a rows-by-dimension table for the default budget; (5) a `device_budget` over-budget cell. **(2) the cheap paged path goes to PERF-2.** (4) automatic index building is DEFERRED to PERF-2's spec, together with RB-001.9. RB-002's on-Mac measurement replaces the wheel-based 8.7× figure.

#### RB-001.2 — [PERF] [MEDIUM] Coalesced flush policy and a background batch scope in the Swift and Kotlin stores (battery, flash writes, write latency)
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - `test/scripts/device_runs/iphone18_3.json` (10K): a single durable write is 11.99 ms; a batch of 100 is 17.36 ms (0.17 ms per memory). `device_budget_v0_11_0`: 3 fsyncs per commit.
  - The same file: background close 12.80 ms, the reopen after it 10.18 ms, first recall on a fresh handle 7.08 ms.
  - `mnemo/bindings/swift/Sources/Mnemo/MnemoStore.swift:401-408, 425-439`: each call made while the app isn't active reopens the file, then flushes and closes it.
  - RB-001.18 is the correctness side of this (when writes become durable). This item is the mechanism.
- **Finding:**
  - An app that wants each memory durable must call `flush()` after each write: 12 ms and three full flushes to storage per memory, 70 times the per-memory cost of a batch. An app that doesn't call it has no durability until the app goes to the background.
  - On iOS, N calls from a background task cost N reopens and N closing commits: about 23 ms of overhead per call at 10K from the close and reopen numbers above, plus a cold first recall (7 ms) when the call is a recall. Most of it is AES and fsync.
- **Recommendation:**
  1. A `flushPolicy` on both stores: `.coalesced(maxDelay: about 1 s, maxWrites: 64)` as the default, `.everyWrite`, and `.manual` (today's behaviour). The store arms one timer on the first dirty write and commits once. A burst of 100 writes becomes one commit (about 17 ms, 3 fsyncs) instead of 100 (about 1.2 s, 300 fsyncs), and the durability window is a second instead of "until background".
  2. A batch scope: `store.withSession { db in … }` holds one expiring activity (iOS), runs many calls on one open handle, and commits once at the end. The Swift README already tells apps to "batch background work where you can", but there's no API for it.
  3. Report the number of commits per foreground minute in the sample's Benchmark, so the battery effect is visible.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09), folded into RB-001.18's flush policy. The batch scope (`withSession` on iOS, `withSession` or `batch {}` on Kotlin) is in MOBILE-2: one open handle and one expiring activity, committing once. The commits-per-minute line in the Benchmark is welcome, not a gate.

#### RB-001.3 — [PERF] [MEDIUM] PERF-2 refinement: a lazy fill can't help the exact scan's first recall (the whole phone range); the levers are `pread` runs, decrypt in place and parallel page decrypt
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `PLAN.md` WORKLOG 2026-11-08, the v0.11.0 READY (the 10K first-recall profile, also `first_recall_10k_profile` in the perf file): of 9.05 ms, the vector-column fill is 65.6%: AES-GCM decrypt 48.4% (4.4 ms), a separate `lseek` and `read` per page 15.5% (1.4 ms), the fp16 copy 4.1%, allocation 2.0%. The scan itself is 22.5%.
  - The same entry concludes "a lazy or partial fill is the lever", and PERF-2's scope says "lazy vector-column fill" for the no-index path.
  - `mnemo/src/store/recall.rs:423-431`: the fill is already lazy, a page at a time as rows are visited. `mnemo/src/pager.rs:348-361`: each page is a seek, a read, and a freshly allocated plaintext `Vec`.
  - Device: 7.08 ms on the iPhone against a target of 5 (`device_budget_mobile`).
- **Finding:** Below `V9_ANN_MIN_FACTS` there's no index, so the first recall visits every fact and must decrypt every vector page. Nothing can be skipped, so a lazier fill buys nothing there. Coalescing the reads recovers at most the 15.5%: about 9.05 → 7.7 ms on the M1 Pro, and by the same share about 7.1 → 6.0 ms on the iPhone. That's still over 5 ms. The target needs the decrypt itself to get cheaper.
- **Recommendation (for the PERF-2 spec, in this order, each measured on the existing profile):**
  1. Read each contiguous run of vector pages with one positioned read (`FileExt::read_at` on Unix, `seek_read` on Windows): about −1.2 ms.
  2. Decrypt into a reused buffer and decode straight into `col.data`, with no per-page `Vec` and no zeroed pre-allocation of the whole column: about −0.4 ms.
  3. Decrypt pages on two or three threads with `std::thread::scope` (safe Rust; pages are independent AES-GCM messages): 4.4 ms of decrypt becomes about 1.6–2.2 ms. Gate it on file size so small files stay single-threaded.
  4. Together that's an estimated 4.5–5 ms on the M1 Pro and about 4 ms on the iPhone. Only if that misses, consider a smaller first-pass column (PLAN.md already names int8 as "a possible future shortlist accelerator"), which is a format question.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09). The argument is right: below `V9_ANN_MIN_FACTS` the first recall must decrypt every vector page, so lazier filling can't help. The PERF-2 spec will take recommendations 1–3 in your order (positioned reads per contiguous run, decrypt into a reused buffer straight into `col.data`, scoped-thread parallel decrypt gated on size), each measured against `first_recall_10k_profile`, with RB-001.1's cheap paged path. int8 stays out unless all three miss.

#### RB-001.4 — [PERF] [MEDIUM] Apple platforms: three `F_FULLFSYNC`s per commit; use `F_BARRIERFSYNC` where only ordering is needed
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `PLAN.md` WORKLOG 2026-11-08 (the v0.11.0 READY): "54% was the three F_FULLFSYNCs", and 74% after the BM25 fix.
  - `iphone18_3.json`: 11.99 ms per single write.
  - `File::sync_all` call sites: `mnemo/src/pager.rs:421` (data pages), `mnemo/src/wal.rs:207` (the commit), `mnemo/src/store/flush.rs:129` (the checkpoint). On macOS and iOS, Rust's `sync_all` and `sync_data` are both `fcntl(F_FULLFSYNC)`.
  - Related and already accepted: R-001.7 (merge the data and WAL fsyncs, lazy checkpoint → Phase 11.1) and PERF-2's "4th fsync on some batch commits". This item is a different lever: the *kind* of sync, not the count.
- **Finding:** Only the WAL commit needs "on stable storage now". The data-page sync before it needs ordering (the pages must not land after the commit record), and the checkpoint sync only bounds recovery work. Apple provides `F_BARRIERFSYNC` for exactly that and documents it as the cheaper call. How much cheaper on current iPhones is unmeasured.
- **Recommendation:**
  1. On Apple targets, use a barrier for the data-page and checkpoint syncs and keep `F_FULLFSYNC` at the commit.
  2. It needs one `fcntl` that std doesn't expose. Keep `forbid(unsafe_code)` on the core by going through `rustix` or a ten-line platform shim crate, with `cargo deny` and MSRV checked.
  3. Measure it in the iOS sample's Benchmark before deciding. If a barrier costs a quarter of a full flush, a single write goes from about 12 ms to about 6–7 ms. That's an estimate, not a measurement.
  4. Run the crash-injection suite against the new ordering, and consider exposing it as a durability level, as SQLite does with `fullfsync`.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) into **PERF-2**. RB-002 measured `F_BARRIERFSYNC` as 16× cheaper on the Mac. Only orderings that don't need durability may use it, so PERF-2's design states which syncs need durability and which only ordering, and the crash-injection suite covers it. The iPhone is measured in the next device run.

#### RB-001.5 — [PERF] [MEDIUM] `track_access` defaults to on in the mobile binding, so every recall dirties the catalog and every iOS background recall ends in a full commit
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - `mnemo/bindings/uniffi/src/types.rs:211-214`: `track_access` defaults to `true`.
  - `mnemo/src/store/recall.rs:614-626`: tracked hits update the catalog rows.
  - `MnemoStore.swift:425-439`: a background call flushes and closes after itself.
  - **RB scratch run** (2K × 384, 30 cycles of open, one recall, flush, close): with `track_access=false` the flush did 0 fsyncs and the file didn't grow; with `true` it did 4 fsyncs and the file grew 24,576 bytes per cycle; flush plus close took 9.7 ms against 3.2 ms.
- **Finding:** On a phone, a read turns into a write. In the foreground the access stats ride along with the next real commit, which is fine. In the iOS background path each recall pays a durable commit (12.8 ms measured on the iPhone for the close) just to record that a memory was read. Scoring v2 already made ranking stable however often memories are recalled (root `README.md`, "Ranking quality"), so the stats are advisory.
- **Recommendation:** Keep tracking, but never commit for access stats alone: mark them as soft dirt that's written with the next real commit and dropped on close if nothing else changed. Failing that, default `track_access` to `false` in the mobile binding and say why.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). RB-002 confirmed 4 fsyncs and +24 KB per open-recall-close cycle with `track_access` on. The mobile binding's `RecallOptions.track_access` defaults to **false**, and the store docs say how to turn it on. Desktop defaults are unchanged.

#### RB-001.6 — [PERF] [LOW] Library size and build graph: `clap` and `rpassword` are unconditional engine dependencies; per-package `opt-level` trial for cold crates
- **Status:** `REJECTED`
- **Context / Evidence:**
  - `mnemo/Cargo.toml:85, 91`: `clap` (with `derive`) and `rpassword` are plain dependencies; the comment there says Cargo has no binary-only dependencies.
  - `mnemo/bindings/uniffi/Cargo.toml:36`: the binding depends on the engine with default features, so each of the six mobile targets (four Apple, two Android) compiles the CLI's dependency tree, in CI and in the release.
  - `mnemo/Cargo.toml:194-196`: the release profile is `lto = true`, `codegen-units = 1`, default `opt-level = 3`.
  - Sizes: `libmnemo_uniffi.so` 2,564,456 bytes on arm64 (the 1e dry run), against a 5 MB gate; zstd is about 0.46 MB of it (`library_size_bytes`).
- **Finding:** Size is already inside its gate, so this is low priority. Two cheap things are still on the table: build time for every binding target, and a few hundred KB.
- **Recommendation:**
  1. A default-on `cli` feature on `mnemo-engine` that owns `clap`, `rpassword` and the `mnemo` binary (`required-features`); the UniFFI, Python and Node crates depend with `default-features = false`. `cargo install mnemo-engine` is unchanged.
  2. Try `[profile.release.package."*"] opt-level = "s"` with `opt-level = 3` kept for `mnemo-engine`, `aes`, `aes-gcm`, `polyval` and `ghash`, and let `size.yml` and the recall bench decide. Expect 0.2–0.4 MB (an estimate).
  3. Don't use `panic = "abort"`: the binding's poisoned-lock handling relies on unwinding.
- **Planner Triage:** `REJECTED` (PLANNER-A, 2026-11-10, after RB-002). RB-002 measured no size gain from either change, and the build-graph half has no time benefit. Revisit only if the CLI split comes up for its own reasons.

#### RB-001.7 — [DX/DOCS] [MEDIUM] CI time: no `concurrency` cancel and no path filters, so every docs push runs all 18 jobs (three on macOS)
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** `.github/workflows/ci.yml:3-18`: the only ignored paths are `PLAN.md` and `reviewer_notes.md`. No workflow has a `concurrency:` block. One run is 18 jobs: rust ×3, deny, msrv, python, compat, mobile ×3, msrv-uniffi, swift, kotlin, fuzz-smoke ×5. The rust macOS leg, the iOS build and the swift job run on macOS runners.
- **Finding:** The Planner's README and `index.html` handoffs, and CHANGELOG-only commits such as `8112fc9` and `15e1001`, each run the full matrix. A second push to a branch doesn't cancel the first run. With one serialized Worker queue, CI wall time is the queue's clock.
- **Recommendation:**
  1. `concurrency: { group: ci-${{ github.ref }}, cancel-in-progress: true }` for branches and PRs (not for `main`).
  2. Skip docs-only changes: add `README.md`, `index.html`, `CHANGELOG.md`, `RELEASING.md` and `**/README.md` to `paths-ignore`, or use a small "changes" job when required checks are configured.
  3. Run `swift`, `kotlin` and `mobile` only when `mnemo/**` or `examples/mobile/**` change, and `fuzz-smoke` only when `mnemo/src/**` or `mnemo/fuzz/**` change.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2), as a CI-only commit: `concurrency:` with cancel-in-progress per ref, and `paths-ignore` for docs-only pushes (README, `index.html`, `*.md` outside the code folders). Release and tag workflows are exempt.

#### RB-001.8 — [DX/DOCS] [MEDIUM] `file_growth_per_single_write_bytes` (57,344) measures the first 20 commits only; steady state is about 2 KB per write
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `mnemo/examples/device_budget.rs:46` (`SINGLE_WRITES = 20`) and `:278-281`; the sample apps' Benchmarks port it.
  - `mnemo/src/store/mod.rs:146-153` and `mnemo/src/store/snapshots.rs:135-161`: retained snapshots may pin a quarter of the live pages or 128 pages (1 MiB), whichever is larger.
  - The value 57,344 appears in `device_budget_v0_11_0` (1K) and in `iphone18_3.json` (1K and 10K).
  - **RB scratch run** (1K base, then 400 single durable writes): writes 1–16 grew the file 56.0 KB each, 17–32 32.0 KB, and 33–400 between 1.6 and 2.8 KB each.
- **Finding:** Each commit writes about 7 new pages, and they stay pinned until the 1 MiB floor is reached, about 18 commits in. A 20-write sample therefore reports the warm-up, not the steady state, and overstates long-run growth about 25 times. The code comment at `device_budget.rs:278-279` ("the old ones stay until compact") describes v8.
- **Recommendation:** Report both: `first_commits` (today's number, which is the real cost of a fresh file's first megabyte) and `steady_state` after at least 128 commits. Fix the comment. Mark the existing cells as warm-up values in the perf file.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) into PERF-2's bench work. `device_budget` samples file growth after the retained-snapshot count stops rising (RB-002's addition), and reports steady-state bytes per write alongside the first 20.

#### RB-001.9 — [PERF] [IDEA] One maintenance call for charging-and-idle windows (`BGProcessingTask`, `WorkManager`): compaction, cold-turn compression, index build
- **Status:** `DEFERRED`
- **Context / Evidence:** Commits do maintenance in bounded slices so they stay fast: `COLD_TURNS_PER_COMMIT = 256` (`mnemo/src/store/records.rs:39`), `VACUUM_PAGES_PER_COMMIT = 4` (`mnemo/src/store/mod.rs:163`), the index persisted every 64 commits (`mod.rs:108`). `compact` and `buildIndex` are explicit calls that hold the store (both READMEs, "Large calls hold the store"). Nothing in `PLAN.md` schedules maintenance on phones.
- **Finding:** Phones have a natural place for heavy work: plugged in, idle, with a longer background allowance. Today an app developer has to know which engine calls to make there, and in which order.
- **Recommendation:** `store.performMaintenance(deadline:)`, a single resumable call that does the outstanding work in priority order (build or refresh the index, compress cold turns, vacuum, compact if the file is mostly free) and stops at the deadline or on RB-001.20's interrupt. Ship a ten-line registration snippet for `BGProcessingTask` and `WorkManager` (charging and idle constraints) in the two READMEs.
- **Planner Triage:** `DEFERRED` (PLANNER-A, 2026-11-10, after RB-002) to the PERF-2 spec, together with RB-001.1's automatic index build: one maintenance call for charging-and-idle windows.

---

**Part 1b — Product ideas** (category `PRODUCT`; priority `IDEA` unless argued otherwise)

#### RB-001.10 — [PRODUCT] [MEDIUM] Adapters for the phones' own agent frameworks (Apple Foundation Models tools, Android on-device function calling): the mobile counterpart of the MCP server
- **Status:** `DEFERRED`
- **Context / Evidence:** On desktop, an agent reaches a `.mnemo` file through the MCP server (`mnemo/src/mcp.rs`). On phones there's no MCP host; `MnemoStore` is an API for app code, not for a model. `PLAN.md` has no entry for on-device model frameworks (checked for "Foundation Models", "Gemini Nano", "AICore").
- **Who it's for:** iOS and Android developers building an assistant on the platform's on-device model, which is the "on-device agents and apps" audience named in the 2026-11-07 target-market decision.
- **Why it fits:** It's an engine feature, not an app. A small optional target (`MnemoTools`) exposes `remember`, `recall` and `essence` as tool definitions the platform's model can call, with the byte budgets from 12.8 mapped to the small context windows these models have. It reuses the MCP tool descriptions, so the wording agents see is the same on every surface.
- **Why MEDIUM, not IDEA:** Without it, each adopter writes the same glue, and the "agent memory" story on phones is only "a database with vectors". It's also the fastest path to a convincing demo (RB-001.14).
- **Recommendation:** Spec it as a follow-up to MOBILE-1 once INSP-1 is out: one Swift target and one Kotlin artifact, each under 300 lines, with the tool schema generated from one source shared with `mcp.rs`.
- **Planner Triage:** `DEFERRED` (PLANNER-A, 2026-11-10, after RB-002). A strong idea (the mobile counterpart of the MCP server). It's a candidate for MOBILE-3 after INSP-1, and the user decides its priority against Phase 12.

#### RB-001.11 — [PRODUCT] [IDEA] Ready-made embedders as optional package targets (the mobile half of 12.3)
- **Status:** `ACCEPTED`
- **Context / Evidence:** 12.3 gives desktop a built-in embedder (`embed-fastembed`). On mobile, SAMPLE-1 puts the `NLEmbedding` → `NLContextualEmbedding` chain in the *sample app* (`PLAN.md` WORKLOG 2026-11-09, the 1d device results and the VERIFIED that follows). The iPhone run showed why it matters: the Notes tab fell back to the demo embedder because the sentence asset wasn't on the phone. MediaPipe was measured and ruled out for the Android sample (a 16.6 MB library, `PLAN.md` WORKLOG 2026-11-09, MOBILE-1d DONE).
- **Who it's for:** App developers who don't want to learn about embedding models, asset downloads and pooling to store a note.
- **Why it fits:** "Text in, text out" is the first promise in the README, and on mobile it currently needs the app to bring a model. An optional `MnemoAppleEmbedder` target (asset check and download, mean pooling, a versioned identity such as `apple-nl-contextual-en-r1`) costs no binary size for apps that don't link it.
- **Recommendation:** Promote SAMPLE-1's chain from sample code to an optional library target. For Android, document one supported recipe (a small ONNX or LiteRT sentence model the app bundles) rather than shipping a model.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-09). SAMPLE-1 builds the `NLEmbedding` → `NLContextualEmbedding` chain as an optional **library target** of the Swift package (`MnemoAppleEmbedder`: asset check and download, mean pooling, versioned identities), not sample-only code. The sample then uses it. The Android "one supported recipe" doc is DEFERRED until after INSP-1.

#### RB-001.12 — [PRODUCT] [IDEA] React Native, Flutter and Kotlin Multiplatform packages from the same `mnemo-uniffi` crate
- **Status:** `DEFERRED`
- **Context / Evidence:** Phase 5 covers Node (napi) and Python polish. `PLAN.md` has no entry for cross-platform mobile frameworks. The curated UniFFI API (`mnemo/bindings/uniffi/src/db.rs`) already takes metadata as JSON strings and keys as bytes, which is what bridge generators need.
- **Who it's for:** Small teams and solo developers shipping AI apps on both platforms from one codebase, a large share of the on-device app market.
- **Why it fits:** No engine work. The cost is packaging and the lifecycle layer (the background close on iOS, the flush on Android) re-exposed through the bridge.
- **Recommendation:** After v0.13, try one (React Native first, by audience size) as a spike with the existing sample's Notes screen, and measure the bridge overhead per recall before committing to support it.
- **Planner Triage:** `DEFERRED` (PLANNER-A, 2026-11-10, after RB-002) until there's demand for React Native, Flutter or KMP. The `mnemo-uniffi` crate keeps that path open.

#### RB-001.13 — [PRODUCT] [IDEA] Key policies for privacy-first apps: "only while unlocked" and biometric-gated stores
- **Status:** `DEFERRED`
- **Context / Evidence:** `mnemo/bindings/swift/Sources/Mnemo/MnemoKeychain.swift:5-29`: two policies, both "after first unlock". `FilePolicy.swift:49`: the file class is fixed at `completeUntilFirstUserAuthentication`. `MnemoKeystore.kt:189-198`: the wrapping key needs no user authentication. Those are the right defaults for background work.
- **Who it's for:** Journaling, health and companion apps, where "what the assistant remembers about me" is the most sensitive data in the app and background access isn't needed.
- **Why it fits:** It's the positioning line from Phase 14 ("the encrypted brain your agents … carry") made visible to the end user. It's a few attributes, not a design change: `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` with file class `complete`, and an access-control option for Face ID or Touch ID; on Android `setUnlockedDeviceRequired` and an authenticated key with a validity window.
- **Recommendation:** Add `KeyPolicy.whenUnlocked` and `.biometric` (iOS) and the Android equivalents, each documented with what stops working in the background. Phase 15's screens can then show a lock state.
- **Planner Triage:** `DEFERRED` (PLANNER-A, 2026-11-10, after RB-002) to MOBILE-3 or Phase 15: "only while unlocked" and biometric-gated key policies.

#### RB-001.14 — [PRODUCT] [IDEA] The demo: export a phone's memory with a passphrase and open it in the inspector on a Mac; make it an INSP-1 acceptance test
- **Status:** `ACCEPTED`
- **Context / Evidence:** `export_with_passphrase` ships in v0.12 (`mnemo/bindings/uniffi/src/db.rs:711-738`). INSP-1 (`PLAN.md`, execution order 4a) "unlocks with a passphrase prompt or a raw-key file". The 14.2 spec lists the inspector as the candidate "when the user wants a visible demo".
- **Who it's for:** Everyone deciding whether to adopt: it shows the single-file promise in thirty seconds, across two devices and two languages, with no server.
- **Why it fits:** Both halves are already scheduled. Only the join is missing: the sample app needs an "Export" button with a share sheet, and INSP-1 needs to open that file by drag and drop.
- **Recommendation:** Add to the INSP-1 spec: "opens a passphrase export made by the iOS sample, shows its memories, threads and essence". Add the Export button to SAMPLE-1. Record it as the site's hero clip. Read `SECURITY.local.md` RB-001.30 before promoting the export widely.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09). INSP-1's acceptance includes "opens a passphrase export made by the iOS sample and shows its memories, threads and essence", and SAMPLE-1 adds the Export button with a share sheet. **Ordering:** both wait for SEC-2 in v0.12.1 (details in `SECURITY.local.md`), as you advised. The hero clip is the user's call when it exists.

#### RB-001.15 — [PRODUCT] [IDEA] A community device table: one measured phone today; let sample-app users contribute `device_runs/*.json`
- **Status:** `ACCEPTED`
- **Context / Evidence:** `test/scripts/device_runs/` holds one file, `iphone18_3.json` (an 8 GB flagship). `device_budget_mobile.android` says "not run on a device this time (emulator only)". The Benchmark already writes a versioned schema (`device_budget_mobile/1`) with the device, OS, thermal state and battery.
- **Who it's for:** Adopters who need to know how it runs on *their* floor device (a four-year-old mid-range Android), and the project, which needs that data to set honest targets.
- **Why it fits:** "Size and speed first" is the direction, and the phone targets currently rest on the fastest phone on sale. Nothing has been measured on older Android storage, and the durable-write cell (12 ms here) is the one most likely to differ there.
- **Recommendation:** A `device_runs/README.md` with the schema and a "share results" button in both samples; a script that validates a submitted JSON and renders a table for the site. Before that, close the gap the project can close itself: one mid-range and one low-end Android run before the Android numbers are quoted anywhere.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → SAMPLE-1: a short contributing note and a JSON schema check for `test/scripts/device_runs/*.json`, so sample-app users can add phones.

#### RB-001.16 — [PRODUCT] [IDEA] A mobile adoption kit: hosted API docs (DocC, Dokka), package-index listings and a two-screen quickstart
- **Status:** `ACCEPTED`
- **Context / Evidence:** The Swift and Kotlin READMEs are thorough (about 270 lines each) but are the only mobile documentation. The Maven publication ships an empty javadoc jar (`PLAN.md` WORKLOG 2026-11-09, the MOBILE-1c VERIFIED: "Sonatype accepts a placeholder; Dokka can …"). `toarchkumar/mnemo-swift` is a CI-written mirror that developers will land on first.
- **Who it's for:** A mobile developer evaluating the package in ten minutes, who never opens this repo.
- **Why it fits:** v0.12 makes the packages installable; nothing yet makes them discoverable or browsable.
- **Recommendation:** Generate DocC and Dokka in the release workflow and publish them next to the site; a real javadoc jar; list the Swift package on the Swift Package Index; a README for the mirror that opens with a five-line example and links to the guide; a "your first memory" quickstart of two screens per platform.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) as **DOCS-1** after INSP-1: hosted DocC and Dokka API docs, package-index listings, and a two-screen quickstart.

---

**Part 2 — The detailed review** (MOBILE-1a–1d.2, then FMT-9, REFAC-1, and the CI and release workflows)

**What held up.** These were checked and no item is raised:
- **The binding's embedder rule:** no code path in `mnemo/bindings/uniffi/src/db.rs` calls the app's embedder with the handle lock held (`embed_one` at `:148-152`; `ingest_thread`'s plan, embed, re-plan under the lock at `:612-636`).
- **Poisoned locks:** mapped to `Corrupt`, never a panic (`db.rs:105-114`).
- **Key bytes at the FFI:** copied into a `Zeroizing` buffer and the incoming `Vec` wiped (`db.rs:33-47`).
- **The iOS foreground and background flag:** set synchronously in the notification handlers, so the race between a call and `didEnterBackground` resolves to a close after the call (`Lifecycle.swift:81-112`, `MnemoStore.swift:401-408`).
- **The Keychain item's attributes:** they match the signed-off policy (`MnemoKeychain.swift:21-28, 127-136`).
- **The Android wrapping:** the key id is bound in as associated data (`MnemoKeystore.kt:48, 120`).
- **The free list's eligibility rule:** it matches its crash argument and has its own property test (`mnemo/src/freelist.rs:1-17, 156-181`).
- **Turn compression:** it sits behind page authentication, with a per-record frame and a size check before allocation (`mnemo/src/store/records.rs:94-123`).
- **REFAC-1:** the split left the public API alone (REVIEWER-A's R-005.2 VERIFIED covers the moves; RB-001.22 and .29 are what's left).
- **The release workflow:** a manual run can't publish (`release.yml:351-354`), the `mnemo-swift` push refuses an existing tag (`:470-472`), and SwiftPM's checksum pins the zip.

#### RB-001.17 — [CORRECTNESS] [HIGH] `release.yml`: the mobile publish jobs still run when an engine build job fails (a skipped publish isn't a `failure`), so Maven Central and `mnemo-swift` can ship without crates.io or PyPI
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** `.github/workflows/release.yml` at `8a9af2e`:
  - `:297-301`: `publish-crates` needs `build-cli` and has a plain `if:` (the repo variable), so when a `build-cli` leg fails the job is **skipped**.
  - `:259-261`: `publish-pypi` is skipped the same way when a wheel or the sdist fails.
  - `:322-327`: `mobile-prepare` needs only those two jobs and runs on `!cancelled() && !contains(needs.*.result, 'failure')`. A skipped job's result is `skipped`, not `failure`.
  - The dry run proves the semantics: on `workflow_dispatch` every engine job is skipped and the mobile jobs run (run 42743609144, `PLAN.md` WORKLOG 2026-11-09, the MOBILE-1e READY).
  - `RELEASING.md:14-15` says "The mobile jobs run after the engine jobs succeed."
- **Finding:** On a real tag, if any CLI target, any wheel, the sdist or `create-release` fails, the engine publishes are skipped and the mobile jobs go ahead: `publishAndReleaseToMavenCentral` and the `mnemo-swift` tag, neither of which can be taken back. The result would be `mnemo-android 0.12.0` and Swift `0.12.0` in public with no matching crate or wheel. The comment at `:324-326` ("never after one failed") shows the intent; the condition doesn't implement it for upstream failures.
- **Timing:** v0.12.0 was tagged and pushed while this was being written, so that run was exposed. If every engine job in it succeeded, the hole didn't open. If one failed, check Maven Central and `mnemo-swift` before re-running anything.
- **Recommendation (three lines, before the next tag):**
  1. `mobile-prepare.needs: [create-release, build-cli, build-wheels, build-sdist, publish-crates, publish-pypi]`, keeping the same `if:`. An upstream failure is then visible as `failure` in `needs.*.result`.
  2. Or, on `push` events, require `needs.publish-crates.result == 'success' && needs.publish-pypi.result == 'success'` unless the matching `ENABLE_*` variable is off.
  3. Re-run the manual dry run afterwards (skipped engine jobs must still let it through), and correct the `RELEASING.md` sentence if option 2's exception is used.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09). Verified at `release.yml:322-327`: `mobile-prepare` needs only `publish-crates`/`publish-pypi`, and a build failure upstream makes them `skipped`, not `failure`. **The v0.12.0 run was not affected:** crates.io has 0.12.0 (03:18 UTC), PyPI has all five wheels plus the sdist, so every `build-cli`, `build-wheels` and `build-sdist` leg and `create-release` succeeded (both publish jobs depend on them), and `mnemo-swift` has `main` plus the tag `0.12.0`. Fix in MOBILE-2: `mobile-prepare` needs every engine job, and on `push` it requires each publish job to be `success` unless its `ENABLE_*` variable is off. The dry run must still pass, and `RELEASING.md` must match.

#### RB-001.18 — [CORRECTNESS] [HIGH] The mobile stores make writes durable only at background, close or an explicit `flush()`; on Android, writes made in the background are never flushed
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - Engine: `Mnemo::remember` only stages the write (`mnemo/src/store/crud.rs:252-257`); dirty pages stay pinned in memory until a flush (`mnemo/src/pager.rs:8`); a dropped handle never flushes (`mnemo/src/store/flush.rs:827-828`).
  - Swift: the only flushes are `flush()`, `close()`, the background close and the close after a background call (`MnemoStore.swift:301-312, 379-393, 425-439`).
  - Kotlin: the only automatic flush is `ON_STOP` (`MnemoStore.kt:285-298`; `ProcessStopObserver.kt:18-25`). A call made after the process has stopped (a `WorkManager` job, a service, a push handler) writes into a handle that's flushed at the *next* `ON_STOP`, which needs the app to come to the foreground and leave again.
  - Samples: the Notes screens never call `flush()` (only the Benchmarks and the background test do).
  - Docs: both READMEs say what happens at background ("every write made so far is durable"); neither says a foreground write isn't durable when the call returns.
- **Finding:**
  - **Data loss.** A crash, a debugger stop, or a kill while the app is in the foreground loses every memory written since the app last left the foreground: possibly a whole session. On Android, a cached process killed after background work loses that work silently. Android kills cached processes routinely.
  - **Memory.** Unflushed pages are pinned, so a long foreground session of writes grows the heap without bound.
  - `await store.remember(…)` returning successfully reads as "saved" to an app developer. On desktop the same contract is visible because the caller owns the handle and the flush; the stores hide the handle, so they should own the flush too.
- **Recommendation:**
  1. Make durability a store setting with a safe default: the coalesced policy in RB-001.2 (commit within about a second or 64 writes), so no call needs to remember to flush and no session can be lost.
  2. On Android, track the process state as the Swift layer does and flush after each call made while stopped (or use the same coalescing timer; it fires regardless of lifecycle).
  3. Until then, say it in both READMEs and the method docs, and have the samples' Notes screens call `flush()` after a save.
  4. Tests: write, kill the process without a lifecycle event, reopen, and count (the emulator workflow can do this with `am force-stop`).
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09). Verified: the Swift store flushes only in `flush()`, `close()`, the background close and the close after a background call; the Kotlin store only on `ON_STOP` and explicit calls (`MnemoStore.kt:221, 284-298`). In MOBILE-2: a store-level flush policy with **`.coalesced` as the default** (commit within about 1 s or 64 writes, one timer), plus `.everyWrite` and `.manual`. Android tracks the stopped state and commits after calls made while stopped. The READMEs and method docs state the durability window. Test: write, force-stop with no lifecycle event (`am force-stop` on the emulator, a kill on the simulator), reopen, count.

#### RB-001.19 — [CORRECTNESS] [MEDIUM] The store's lock is held while the app's embedder runs: calling the store from `embed` deadlocks, and a slow embedder blocks every call and the iOS background close
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - The binding's contract: `embed` "is called … never while the handle's lock is held, so it may call back into the same database" (`mnemo/bindings/uniffi/src/embedder.rs:9-12`), tested at the `MnemoDb` level.
  - Kotlin: `withDatabase` runs the whole engine call, embedder included, inside a non-reentrant coroutine `Mutex` (`MnemoStore.kt:321-337`).
  - Swift: the call runs synchronously on the actor (`MnemoStore.swift:401-421`).
  - Both READMEs repeat "never with the database locked" (`swift/README.md:114-115`, `kotlin/README.md:129-130`) and suggest the embedder may be "a server".
- **Finding:**
  1. **Re-entrancy.** An embedder that uses the store, for example caching embeddings with `cacheGet` and `cachePut`, the natural use of the built-in result cache, has to block on a `suspend` or `async` call into a store that's waiting for it. Kotlin deadlocks on the `Mutex`; Swift deadlocks on the actor. The promise holds for `MnemoDb` but not for the type the READMEs tell apps to use.
  2. **Latency.** While the model runs, every other call on the store waits, recalls included. With a network embedder that's the length of a request.
  3. **iOS background.** `closeForBackground` queues behind the in-flight call. An embedder on a slow network can outlast the background allowance with the file lock held, which is the `0xdead10cc` case the design exists to prevent.
- **Recommendation:**
  1. Embed in the store layer, outside the lock: `rememberText` awaits the embedding first, then calls `remember(content, vector)` under the lock. `recallText`, `update`, the essence writes and `ingestThread` follow the same shape (the binding's `ingest_thread_texts` and `ingest_thread_with_vectors` split already exists for the last).
  2. Offer an async embedder protocol at the store level (`func embed(_:) async throws`, a `suspend` function in Kotlin), so Core ML and network embedders don't block a thread.
  3. Fix the README sentence either way, and add a store-level re-entrancy test next to the binding's.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09). Verified: the binding embeds outside its own mutex, but the store layers wrap the whole call: Kotlin `withDatabase` runs under `mutex.withLock` (`MnemoStore.kt:100-101, 321-322`), and Swift runs it inside the actor's `withDatabase` (`MnemoStore.swift:149-152`). So a slow embedder blocks every store call, including the iOS background close, and a call back into the store from `embed` deadlocks. In MOBILE-2, both stores call the embedder before taking their lock and pass the vector to the binding's vector API, with a re-entrancy test in each suite. Correct the READMEs' "never with the database locked" line.

#### RB-001.20 — [CORRECTNESS] [MEDIUM] iOS background close: long calls can't be interrupted, a call proceeds on an already-expired activity, and app extensions never close the file
- **Status:** `ACCEPTED`
- **Context / Evidence:** `mnemo/bindings/swift/Sources/Mnemo/Lifecycle.swift` and `MnemoStore.swift`; `swift/README.md:136-164, 221-235`. The device gate (two lock-screen runs of 1–2 minutes, 288 writes, no `0xdead10cc`) covered short writes from the main app.
- **Finding:**
  1. **Long calls.** `runExpiring` (`Lifecycle.swift:129-144`) ignores the `expired` callback, and nothing can stop an engine call in flight. `buildIndex`, `compact`, the passphrase export (a KDF of seconds), a long `ingestThread`, or RB-001.19's slow embedder can all outlast the allowance. The data is safe (it's a crash to the WAL), but the user sees the app killed, and in an App Group container it's the lock-held kill.
  2. **Expired activities.** In `beginActivity` (`Lifecycle.swift:151-168`), when the system calls the block with `expired == true` the first time (no background time left), the code resumes anyway and holds nothing (`:160-163`). The call then opens the file and takes the lock with no assertion at all.
  3. **Extensions.** `applicationState()` returns `nil` outside an app (`Lifecycle.swift:124-127`), so in an extension the store believes it's in the foreground forever and keeps the file open and locked. The README's lifecycle section says so ("close it yourself", `:157-160`), but its App Groups section says the background close "is what keeps a suspended app or extension from holding the lock" (`:229-230`). An extension suspended with the lock blocks the main app with `Locked`, and can be killed for it.
- **Recommendation:**
  1. An engine-level interrupt: an `AtomicBool` checked in the long loops (k-means and assignment in `build_index`, the copy loops in compaction and export, between chunks in ingest), exposed as `MnemoDb.interrupt()`, like `sqlite3_interrupt`. `runExpiring` calls it when the block is re-entered with `expired`. The interrupted call returns a typed error and leaves the file as it was.
  2. In `beginActivity`, an expired first callback throws a typed "no background time" error instead of proceeding.
  3. In an extension, default to the background behaviour (open, work, flush and close per call), and observe `NSExtensionHostDidEnterBackground`. Make the two README sections agree.
  4. Add each case to the run sheet (`examples/mobile/README.md`): background during `buildIndex` on a 100K file, and a share extension writing while the app is suspended.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). A call must not proceed on an already-expired activity: it closes and fails with a retryable error. Long calls check for expiry between engine steps where they can. The README's extension paragraph says what's true. The device checks from RB-002's run sheet go into the next device run.

#### RB-001.21 — [CORRECTNESS] [MEDIUM] `MnemoKeystore` (Android): transient Keystore errors are reported as `KeyMissing`, the wrapping key can be regenerated over a live alias, and key files aren't fsynced
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** `mnemo/bindings/kotlin/src/main/kotlin/io/github/toarchkumar/mnemo/MnemoKeystore.kt`.
- **Finding:**
  1. **Transient errors look permanent** (`:62-92`). Any exception from the unwrap, and any `IOException` reading the key file, becomes `KeyMissing`. The README's recovery advice for `KeyMissing` includes "delete the file and start over". Keystore calls can fail for reasons that pass (the keystore service busy or restarting, a binder error), and the code can't tell those from a lost key. An app that follows the documentation destroys a readable database on a glitch. The Swift side gets this right: `errSecItemNotFound` is `keyMissing`, everything else is `.keychain(status:)` (`MnemoKeychain.swift:86-91`).
  2. **Regeneration over a live alias** (`:182-205`). `wrappingKey(create = true)` generates a new key whenever `getKey` returns `null`. If the alias exists but the entry comes back `null`, the new key replaces the old one and every stored key is orphaned for good. The code's own comment names the danger. The process-wide lock (`:211`) doesn't cover a second process of the same app.
  3. **No fsync** (`:52-55`). The key file is written and renamed without syncing the file or its directory. The database it protects is fsynced. After a power loss just after a first create or an import, the database can survive while its key file is empty or missing.
- **Recommendation:**
  1. Map only a missing file, a missing alias, `AEADBadTagException` and `KeyPermanentlyInvalidatedException` to `KeyMissing`. Everything else is `MnemoStoreException.Keystore("unwrap", e)`, documented as retryable.
  2. Before generating, check `store.containsAlias(alias)`: if it's there and `getKey` gave nothing, throw the retryable error. For apps with several processes, guard creation with a file lock in the keys directory.
  3. Write the key file through a `FileOutputStream`, `fd.sync()` before the rename, and sync the directory where the platform allows it.
  4. Tests with a fake `KeyWrapper` that throws once and then succeeds, and one that returns no key for an existing alias.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2), **first in the batch**. Reporting transient Keystore errors as `KeyMissing` is dangerous, because the documented `KeyMissing` recovery includes deleting the file. Regenerating the wrapping key over a live alias would orphan every key. **Fix:** a distinct retryable `Keystore` error; never generate when the alias exists; fsync and rename for key files; JVM tests per RB-002.4.

#### RB-001.22 — [CORRECTNESS] [MEDIUM] REFAC-1 `close(self)`: a failed final flush destroys the handle, so the buffered writes can't be retried (disk full on a phone)
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - `mnemo/src/store/flush.rs:852-862`: `close(mut self)` runs `self.flush()?`. On an error the handle has already been moved in, so it's dropped, with everything unflushed. The rustdoc states this (`:825-828`).
  - Before `5254395`, `close(&mut self)` left the handle with the caller, who could free space and try again.
  - The binding takes the handle out of its `Option` first (`mnemo/bindings/uniffi/src/db.rs:337-345`). The Swift background close records the error and moves on (`MnemoStore.swift:379-393`). The Kotlin `closeLocked` frees the handle in `finally` (`MnemoStore.kt:339-348`).
  - Related: R-005.1 (VERIFIED) is why `close` consumes the handle; that fix is right. This is the failure path it left.
- **Finding:** "No space left on device" is an ordinary condition on phones. With RB-001.18's behaviour, the first flush of a session may be the close itself, so a full disk at that moment loses the session even if space returns a minute later. The caller gets an error but no way to keep the data.
- **Recommendation:**
  1. Return the handle with the error, as `BufWriter::into_inner` does: `close(self) -> Result<(), CloseError>` where `CloseError` carries the `Mnemo` and the cause. Or add `try_close(&mut self) -> Result<()>`, which releases the file only after a successful flush.
  2. In the binding, put the handle back when the flush fails, so `flush()` can be retried. On the iOS background path the lock must still be released, so there the store drops the handle but keeps `lastBackgroundError` and counts the lost writes for the app.
  3. A failpoint test: the flush fails once inside `close`, then succeeds on retry, and every write is present.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). Confirmed by RB-002 with a failpoint. `close` flushes **first**, and on failure returns the error **with the handle kept** so the caller can retry. In Rust that means `close(self) -> Result<(), (Mnemo, Error)>`, or a `try_close` beside it (your call, without breaking 0.12 callers if you can). The Swift and Kotlin `close()` keep the handle on failure.

#### RB-001.23 — [CORRECTNESS] [MEDIUM] FFI error mapping: `Kdf`, `Crypto` and `Serialize` become `Corrupt`; `NeedsWriteOpen` becomes `Invalid` and breaks the try-each-key probe; no retryable class
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** `mnemo/bindings/uniffi/src/error.rs:49-77`; `MnemoStore.swift:503-530`; `MnemoStore.kt:420-454`.
- **Finding:**
  1. **`Corrupt` is a catch-all** (`error.rs:72-74`). `Kdf` errors land there, and Argon2's 19 MiB allocation can fail on a low-memory phone during `open`, the export or the import. So do `Crypto` and `Serialize`, which can come from the write side. An app reads `Corrupt` as "the file is damaged" and may reset it.
  2. **`NeedsWriteOpen` maps to `Invalid`** (`error.rs:69-71`). The stores' fallback for a file that has lost its key-id stamp probes each stored key with a *read-only* open and skips only `WrongPassphrase`. If that file also has a WAL to recover (the app was killed between the commit and the checkpoint), the read-only open refuses with `NeedsWriteOpen` before it even checks the key (`mnemo/src/store/open.rs:719-771`), the probe throws `Invalid` on the first candidate, and the store can't open the file on this launch or any later one. Both conditions follow a kill at a bad moment (Swift's `compact()` replaces the file and re-stamps it afterwards, `MnemoStore.swift:316-330`), so they can coincide.
  3. **A poisoned handle isn't recovered by the stores.** `onHandle` and `withDatabase` reopen only on `Closed`. After a panic every call returns `Corrupt` until the app calls `close()`, although the message says "reopen the file" and the stores promise transparent reopening.
  4. `UnsupportedVersion`'s message drops the engine's hint about which versions this build opens.
- **Recommendation:**
  1. Add `MnemoError.NeedsWriteOpen` (or a `key_opens(path, key) -> Bool` binding call that only unwraps the header, which also makes the probe cost microseconds instead of a catalog load; see RB-001.28). Probe with it.
  2. Split the catch-all: `Kdf` gets its own variant (or `Io` with an out-of-memory message), write-side `Serialize` is `Invalid`, and `Corrupt` is kept for integrity failures only.
  3. Document three classes in both READMEs: retry later (`Locked`, `Io`, Keychain and Keystore errors, the KDF memory error), fix the call (`Invalid`, `DimensionMismatch`, `ReadOnly`), and the data is unreadable (`Corrupt`, `HeaderTampered`, `KeyMissing`).
  4. Treat the poisoned-handle error like `Closed` in the stores: drop the handle, reopen once, retry.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). The FFI error enum gains distinct `Kdf`, `Crypto`, `NotAMnemoFile` (bad magic) and `NeedsWriteOpen` cases, and a `retryable` flag. `UnsupportedVersion` carries the engine's message. The try-each-key probe treats only key and decrypt failures as "not this key".

#### RB-001.24 — [CORRECTNESS] [MEDIUM] Caller-supplied `_mnemo.*` metadata is honoured as engine state (verified: such memories vanish from recall); reject it at every public write surface before 13.4 and 13.5
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - `mnemo/bindings/uniffi/src/db.rs:174-178` and `types.rs:383-393`: metadata JSON is inserted key by key with no check.
  - The only filters on the reserved namespace are in the keyed-write comparison and in `update`'s preservation (`mnemo/src/store/keyed.rs:29, 216`). Plain `remember` has none, on any surface.
  - **RB scratch run:** four memories stored with plain `remember`: one normal, one with `{"_mnemo.superseded_by": …, "_mnemo.invalid_at": 1}`, one with `{"_mnemo.kind": "turn", "_mnemo.thread": "t1"}`, one with `{"_mnemo.key": "current-release"}`. Recall returned two of the four. `history("current-release")` listed the fourth, and the next keyed write superseded it.
- **Finding:** Reserved keys are how the engine records supersession, kinds, threads and keys (the standing rule from R-001.10). Any caller can set them directly, and agents often pass model-written metadata straight through (the MCP `remember` tool takes a metadata object). A memory can be stored already hidden, filed into a thread it isn't part of, or placed in another key's history. It isn't a privilege boundary today, since every key holder may write anything. It becomes one in Phase 13, where `_mnemo.author`, `_mnemo.sig` and `_mnemo.visibility` are planned as metadata (13.4, 13.5), and in Phase 15, where end users see and delete what's stored.
- **Recommendation:** Reject `_mnemo.*` keys in caller metadata with `Invalid` at every public write surface (Rust `remember` and `update`, Python, the CLI, MCP, the binding). The engine sets them through internal constructors only. `export` and `get` keep returning them. Add this as a requirement in the 13.4 and 13.5 specs and in 14.1's change-set validation, with a dual-format test.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). Confirmed on v8, v9 and through the binding. Every public write surface (Rust `remember`/`update`, Python, the CLI, MCP, the binding) rejects caller-supplied `_mnemo.*` metadata keys with `Invalid`. The engine's own writes go through an internal path. Existing files are unaffected.

#### RB-001.25 — [DX/DOCS] [MEDIUM] FMT-9 retention: the READMEs promise "the newest 16, plus the last 24 hours"; the pin cap left 6 restore points in a churn test
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - Root `README.md:175` and `CHANGELOG.md:298-299` describe the window. Only the CHANGELOG adds "never pinning more than a quarter of the file (or 1 MiB)".
  - `mnemo/src/store/snapshots.rs:135-161` (`apply_retention`): after the count and age rules, snapshots are dropped oldest-first while they pin more than the cap, down to one.
  - **RB scratch run** (384 dims, all commits within a minute): 40 commits of 100 new memories kept 15–21 snapshots; then 20 commits of 100 deletes and 100 adds each kept **6–7** (7.6 MB file, 4,000 live).
- **Finding:** The cap is a sound engineering choice, and it's why steady-state growth is small (RB-001.8). But the documented window isn't a floor. Under churn, point-in-time restore reaches back a handful of commits, which can be seconds. That matters for anything that presents snapshots as "undo": INSP-1's snapshot view, and Phase 15's delete and forget screens.
- **Recommendation:**
  1. State the cap in the root and engine READMEs: "up to 16 and 24 hours, limited to a quarter of the file".
  2. Expose what's actually restorable: `stats()` gains the retained count, the oldest restorable time and the pinned bytes. INSP-1 shows those, not the policy.
  3. Let an app choose: a minimum count that's kept whatever it pins (it pays in file size), next to `set_max_snapshots`.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-09). Verified: `apply_retention` drops oldest-first past the pin cap (`snapshots.rs:135-161`), so the documented window isn't a floor. **MOBILE-2's docs commit** states the cap in the root and engine READMEs. **INSP-1:** `stats()` gains the retained count, the oldest restorable time and the pinned bytes, and the inspector shows those, not the policy. The minimum-count option is DEFERRED to Phase 15, where undo-style screens need it.

#### RB-001.26 — [PLAN-REFINEMENT] [MEDIUM] FMT-9: no fuzz target covers the v9 page, column, delta, free-list, BM25 or key-slot decoders, and the decompression bound is 64 MiB for 7.9 KB records; needed before 14.1
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `mnemo/fuzz/fuzz_targets/` has five targets: header, WAL replay, record decode, cache directory, MCP frame. `mnemo/src/lib.rs:308-362` (`__fuzz`) gained one v9 entry, `decode_v9_memory`.
  - New in v9 with no fuzz entry: the packed record page and its slot directory (`record_slot`), vector pages (`mnemo/src/veccol.rs:212-262`), the v9 catalog base and delta runs and the index delta runs (`mnemo/src/store/load.rs`), `FreeList::from_runs` (`mnemo/src/freelist.rs:45-52`), the BM25 manifest, dictionaries and postings (`mnemo/src/store/records.rs`), and the key-slot run (`mnemo/src/keyslot.rs`).
  - `records.rs:31-35`: `RECORD_PLAIN_MAX = 64 << 20`, while a compressed record is "a chunk of at most `V9_TURN_CHUNK_BYTES` (7,900) plus its metadata".
- **Finding:** Today all of these sit behind page authentication, so only a key holder or a bug can feed them bad bytes; that's why this isn't a security item. Phase 13 (several writers, roles that "stop mistakes") and 14.1 (change sets from other replicas) change who produces those bytes. A decoder that panics or over-allocates on a peer's record becomes a way for one device to break another. A 64 MiB allocation per record is also four times the whole mobile vector budget.
- **Recommendation:**
  1. Add fuzz targets for each v9 decoder listed above, with the same matrix rows as the existing five, before the 14.1 design note is signed off.
  2. Lower `RECORD_PLAIN_MAX` to what a record can be (1 MiB is still generous).
  3. In the 14.1 note, require that change-set records pass the same decoders under fuzzing as local ones.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002). **The decompression bound in MOBILE-2e (v0.12.2):** a v9 turn record decompresses to at most its chunk size, so the bound drops from 64 MiB to 1 MiB, with a test using RB-002's 2,067-byte frame. **The v9 decoder fuzz targets** (page, column, delta, free list, BM25, key slot) become **FUZZ-2**, required before Phase 14.1.

#### RB-001.27 — [PLAN-REFINEMENT] [LOW] The new Swift and Kotlin configurations expose `formatVersion: 8` one release before v8 becomes read-only
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** `MnemoStore.swift:15-17, 28` and `MnemoStoreConfiguration.kt:14, 25` ("`8` creates a v8 file for readers on v0.10 or earlier"); `mnemo/bindings/uniffi/src/types.rs:55-58`. The `[0.12.0]` CHANGELOG says "v0.13 makes them read-only" (`8112fc9`).
- **Finding:** No mobile reader older than v0.12 exists, so nothing on a phone can need a v8 file. The option is new public API that the next release has to deprecate, and a file created with it misses every v9 gain the mobile numbers are built on (1.3 KB per memory, keyword recall, compression).
- **Recommendation:** Remove `formatVersion` from the two store configurations before the packages have users (it can stay in the low-level `MnemoConfig` for tests), or mark it deprecated. v0.12.0 is tagged, so do it in v0.13 together with the v8 sunset.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). Remove `formatVersion: 8` from the Swift and Kotlin configurations (v8 becomes read-only in v0.13).

#### RB-001.28 — [CORRECTNESS] [LOW] Swift: changing `keyPolicy` on an existing store half-applies; the key probe does a full read-only open per stored key
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** `MnemoStore.swift:454-479` (`open`), `:503-530` (`resolveKey`); `FilePolicy.swift:62-71`; `MnemoKeychain.swift:66-77`.
- **Finding:**
  1. **Policy changes.** On every open, `FilePolicy.apply` sets the backup flag from the *configured* policy, but the Keychain item keeps the attributes it was created with. Switching a store from `.deviceOnly` to `.iCloudSynced` puts the file into backups while the key stays device-only and unsynced. A restore then brings the file without its key (`keyMissing`), which is the outcome the policy table says `.iCloudSynced` avoids. The reverse switch leaves a synced key behind for a file that's now excluded.
  2. **Probe cost.** The fallback opens the file read-only once per stored key (`:520`), and each open loads the catalog: about 2 ms at 10K and 30 ms at 100K per candidate, from the open numbers in the perf file.
- **Recommendation:** On open, compare the stored item's attributes with the policy (the internal `attributes(id:)` already reads them) and either migrate the item with `SecItemUpdate` or throw a clear error. Document that the policy is fixed at creation unless migrated. Use the header-only `key_opens` check from RB-001.23 for the probe.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002), part 1 → MOBILE-2e (v0.12.2). A `keyPolicy` change on an existing store is either refused or applied completely (moving the key and flipping the backup flag together). Part 2 was refuted by RB-002 (a wrong-key probe fails in 0.05 ms).

#### RB-001.29 — [DX/DOCS] [LOW] Release builds carry 18 `__*` test hooks (three mutate the file) and two `MNEMO_DEV_*` environment switches
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `mnemo/src/store/hooks.rs` (17 `pub fn __…`, all `#[doc(hidden)]`, none behind a feature) and `mnemo/src/store/flush.rs:887-888` (`__crash_partial_flush_for_testing`, whose doc says it "strands data pages; never expose it from a binding").
  - `__set_index_layout_for_testing` (`hooks.rs:97`) and `__add_key_slot_for_testing` (`hooks.rs:117`) also change the file.
  - `MNEMO_DEV_ANN_MIN_FACTS` (`mnemo/src/store/ann.rs:30-36`) and `MNEMO_DEV_TURN_ZSTD` (`mnemo/src/store/records.rs:44-47`) are read in every build.
  - The crate already has the right pattern: the failpoints are compiled only under `cfg(any(test, feature = "failpoints"))` (`mnemo/src/pager.rs:17, 70, 76`).
- **Finding:** Hidden isn't private. Any Rust dependent can call a hook that leaves a file half-written, and an environment variable set by a parent process changes when an index exists and whether turns are compressed. The risk is low, but it's the kind of surface that's cheap to close now and awkward once Phase 13 adds roles.
- **Recommendation:** Put the hooks and the two environment switches behind a `dev-hooks` feature (or reuse `failpoints`), enabled by the test, bench and example targets that need them. Check that the release artifacts are built without it.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) as **HOOKS-1** in v0.13: the `__*` hooks and the `MNEMO_DEV_*` switches are compiled only behind a non-default feature used by the test suites. The three hooks that mutate the file go first.

#### RB-001.30, RB-001.31, RB-001.32 — [SECURITY] Held in `SECURITY.local.md`
- **Status:** `ACCEPTED`
- **Context / Evidence:** Three unfixed security findings from this pass (two `MEDIUM`, one `LOW`) are written up in the gitignored `SECURITY.local.md` at the repository root, with evidence, a reproduction for the first, and recommendations. Per AGENTS.md they stay out of this file until the fixes ship. Areas: key handling in the engine and binding, the release workflow's use of secrets, and secret hygiene in memory.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-09), all three, as SEC-2 in v0.12.1. The full triage is in `SECURITY.local.md`; the details stay out of this file until the fix ships.

---

### Batch `RB-002` — 2026-11-09 (REVIEWER-B on the Mac: RB-001 re-checked with real tests and measurements, the v0.12.0 release run, and five new items)

**What was read and run.** Working tree `f79715e` (= tag `v0.12.0`). `git log f79715e..HEAD` is empty, and the only uncommitted changes are the two coordination files, so **Part 3 has nothing to review**: no code has landed since the tag.

**How.**
- **Machine:** Apple M1 Pro (8 cores, 16 GB), macOS 26.7.1, internal APFS SSD, Rust 1.95.0, Swift 6.4.2 (Xcode 27.2).
- **Outside the repo only.** A scratch crate that depends on `mnemo-engine` (with `failpoints`) and `mnemo-uniffi` by path, with a copy of `mnemo/Cargo.lock` and of `.cargo/config.toml` (hardware AES); and scratch copies of the repo made with `git archive HEAD` for anything that needs crate internals, a changed manifest, or `swift test`. `CARGO_TARGET_DIR` pointed at the scratch directory throughout.
- **Profiles.** Behaviour tests: debug. Every timing: release with `lto = true`, `codegen-units = 1`, as the repo builds it. Run counts are given with each number.
- **Baselines, all green at `f79715e`:**
  - `cargo test --locked --features failpoints` (scratch copy): **453 passed, 0 failed**.
  - `cargo test --locked -p mnemo-uniffi`: **11 passed**.
  - `swift test` on macOS, with the XCFramework built from `f79715e` by the package's own script (`MNEMO_SLICES=macos`): **14 passed** (`StoreTests`; the iOS suite needs a simulator host app and wasn't run).
  - **Kotlin: not run here.** This Mac has no JDK and no Gradle (`java -version`: "Unable to locate a Java Runtime"). The CI job `kotlin (linux, Android AAR)` is green at `f79715e` (run 42747998700).
- **Read-only:** nothing was written but this file and the gitignored `SECURITY.local.md`. `test/project.mnemo` wasn't opened. No git state was changed.
- **Triage respected.** Re-read right before writing: PLANNER-A's partial triage of RB-001 (13 of 32) is unchanged since this pass began. No RB-001 item is marked `VERIFIED`, because no fix has shipped. No triage line or ledger status was edited.

**Items the Planner held for RB-002 were tested first:** RB-001.4, .5, .6, .7, .8, .20–.24 and .26–.29. The idea items among the held ones (.9, .10, .12, .13, .15, .16) have nothing to test; they stand as written.

**Addendum, 2026-11-10 05:15 UTC — which commit each finding was tested at, and how this pass measures against three working rules.**

The tree has moved since the batch was written. Two commits follow the tag, both workflow-only:
- `ce88b06` removes `android-publish.yml`;
- `6bc592c` (MOBILE-2a, branch `mobile-2a-release-gates`) changes `release.yml`, adds `scripts/check-published-package.sh`, and updates `RELEASING.md` and the CHANGELOG.

`git diff f79715e 6bc592c` touches no engine, binding, Swift, Kotlin or Python source and neither `Cargo.toml` nor `Cargo.lock`. No finding below has met its fix yet, apart from the two workflow items. At 05:13 UTC, when the re-runs were made, the working tree had no uncommitted code change (`git status` on `mnemo/` and `.github/`, checked just before). By 05:17 an untracked `mnemo/bindings/swift/Sources/Mnemo/FlushPolicy.swift` had appeared: MOBILE-2b is under way in the working tree. From here on, tests pin a commit with `git archive <sha>` into a scratch copy and never build the live tree by path, so an uncommitted Worker edit can't leak into a result.

| Finding | Tested at | Notes |
|---|---|---|
| Part 1: the release run and the four registries | tag `f79715e`, run 42748000883; workflow text as of `8a9af2e` | The run ended `cancelled` at 05:02:46 UTC (the Worker's BLOCKED entry in `PLAN.md`). An earlier version of this batch said "6 CLI legs"; it's 5, corrected above. |
| RB-001.17 (the semantics) | `8a9af2e` (dry run 42743609144) and `f79715e` (the tag run) | The fix is in `6bc592c`; see the update under `RB-002.1`. |
| RB-001.18, .22, .23 (items 1, 2, 4), .24 and .30 at engine and binding level | `f79715e`, and **re-run at `6bc592c`** (05:13 UTC) | 11 scratch tests; the same results at both. |
| RB-001.30 and `RB-002.2`: the in-crate header graft | scratch copy of `f79715e` | Re-run at 05:14 UTC; the same results. |
| Swift store tests: RB-001.18, .19, .23 item 2, .28 part 1, .32 item 4, and the key-store test beside .21 | package built from a scratch copy of `f79715e` | Not re-run; `mnemo/bindings/swift/Sources` and `mnemo/bindings/uniffi` are identical at `6bc592c`. |
| Every timing and size: RB-001.1, .2, .3, .4, .5, .6, .8, .25, .26 (the allocation), .28 part 2, `RB-002.3`, `RB-002.5` | `f79715e` only | **Provisional**: see rule 2. |
| RB-001.7 | `f79715e` (CI run 42747998700) | |
| RB-001.20, .21, .26 (the target list), .27, .29 (the hook count), .31 | read at `f79715e` | Those files are unchanged at `6bc592c`, except `release.yml` for .31 (see `SECURITY.local.md`). |
| `RB-002.1` | workflow at `f79715e`; the fix read at `6bc592c` | Update under the item. |
| The baselines (453, 11 and 14 tests) | `f79715e` | |

**Three rules for this role, received after the batch was written, and where the pass above departed from them.**
1. **`CARGO_TARGET_DIR=/tmp/rb-target` and `--locked` on every build and test.**
   - The pass used a target directory inside its own scratch folder, never the Worker's `mnemo/target/`. `mnemo/Cargo.lock` is unmodified (`git status`).
   - Two things ran without `--locked`, both against scratch-only lock files: five late builds of the scratch crate, after `zstd` was added to *its* manifest (the allocation test for RB-001.26, the single-write, `stats()` and no-op-flush timings, and the rebuilt recall benchmark); and the Swift package's `build-xcframework.sh` inside a scratch copy, which calls cargo itself.
   - The two re-runs in the table used `/tmp/rb-target` and `--locked`. Everything from here on does.
2. **No benchmarks while the Worker is mid-PR; check the end of `PLAN.md` first.**
   - The timings were taken between about 03:45 and 04:10 UTC without that check. `PLAN.md`'s last Worker entry then was the MOBILE-1e READY, and its BLOCKED entry shows the Worker watching the hung release run through that window, but that's reconstruction, not a check made at the time. The spreads were tight (for example p10 2.17 ms and p90 2.29 ms on the 10K recall), which argues against heavy contention, but **treat every number in the "timing and size" row as provisional**.
   - The verdicts that rest on them are ratios of 4× and more, or exact byte counts, so I don't expect them to change. The absolute milliseconds might.
   - **Pending re-measurement:** the Worker is mid-PR now (MOBILE-2a, with dry run 42760637871 in progress at 05:14 UTC), so nothing was re-timed. To redo when `PLAN.md` shows the Worker idle: the RB-001.1 table, the RB-001.3 profile, the single-write and `track_access` timings, and `RB-002.3`'s `stats()` cell. The fsync-kind measurement (RB-001.4) and the file-size counts (RB-001.6, .8, .25) don't depend on CPU load in the same way but were taken in the same window.
3. **Label each finding with the commit it was tested at.** The table above. From now on each finding carries its commit where it's stated.

---

**Part 1 — the v0.12.0 release (run 42748000883, `release.yml` on the tag)**

- **All four registries have 0.12.0,** checked directly:
  - **crates.io:** `mnemo-engine 0.12.0`, created 03:18:30 UTC, not yanked.
  - **PyPI:** five wheels and the sdist, uploaded 03:19:01–03:19:09 UTC.
  - **Maven Central:** `io.github.toarchkumar:mnemo-android:0.12.0` serves the `.aar` (2,692,667 bytes), `.pom`, `.module` and `.aar.asc`; `maven-metadata.xml` lists it as `release`.
  - **`toarchkumar/mnemo-swift`:** public, `main` plus tag `0.12.0` (`a23f218`). A scratch consumer package that depends on it with `exact: "0.12.0"` **resolved from GitHub and built with `swift build` in 9 s**, so the URL and the checksum in the published `Package.swift` are right.
  - The GitHub Release has 13 assets and isn't a draft.
- **RB-001.17: the v0.12.0 run was not affected. CONFIRMED against the job results, and the finding itself is CONFIRMED.**
  - In the tag run every engine job succeeded: Create GitHub Release, 5 CLI legs, 5 wheel legs, the sdist, publish to crates.io, publish to PyPI. The mobile jobs then ran in the intended order.
  - The semantics the finding rests on are visible in the dry run (42743609144): every engine job is `skipped`, and `mobile (version check)`, the Swift job and the Android job all ran to `success`. A skipped upstream job doesn't stop them.
  - **Against PLANNER-A's WORKLOG entry** ("RB-001 triaged in part", 2026-11-09), which inferred the same from the registries: it's right. Two differences:
    1. it couldn't reach Maven Central; the artifact is there;
    2. **the run had not finished.** At 04:13 UTC it was still `in_progress`: the Swift job's last step, "Resolve the published package", had been running since 03:28:23 (45 minutes). That's `RB-002.1` below. Nothing is unpublished or half-published because of it.

---

**Part 2 — RB-001, item by item**

| Item | Verdict | Evidence from this pass |
|---|---|---|
| RB-001.1 | **CONFIRMED, magnitude corrected** | The cliff is at the predicted row count and turns count toward it. The slowdown is **4.8–5.1×** here, not 9×. Table below. |
| RB-001.2 | **CONFIRMED** (the ratio) | One durable write at 10K × 384: 14.98 ms median (n=100), 3 fsyncs. A flush of 100 writes: 15.54 ms (n=10). |
| RB-001.3 | **CONFIRMED; one estimate was high** | The profile reproduces (52% decrypt, 16% reads). Parallel decrypt delivers what was estimated. Coalesced reads are worth about 0.3 ms, not 1.2. Details below. |
| RB-001.4 | **CONFIRMED on a Mac; the iPhone is NOT TESTABLE HERE** | `F_BARRIERFSYNC` is 16× cheaper than `F_FULLFSYNC` on this SSD, and the proposed commit shape halves the sync cost. Details below. |
| RB-001.5 | **CONFIRMED** | 30 cycles of open, one recall, flush, close at 2K and at 10K × 384. `track_access` off: 0 fsyncs, file unchanged. On: **4 fsyncs and +24,576 bytes every cycle**, and 16.6 ms more per cycle at both sizes. |
| RB-001.6 | **REFUTED for size; the build-graph half holds, with no time gain** | Neither suggested change makes the library smaller. The earlier estimate (0.2–0.4 MB) was wrong. Details below. |
| RB-001.7 | **CONFIRMED** | Run 42747998700: a push of three docs-only commits (`8112fc9`, `15e1001`, `f79715e`) ran all 18 jobs: 3,299 job-seconds (55 job-minutes), 12 minutes wall, 1,115 s of it on macOS runners. The same SHA had run the full matrix 13 minutes earlier as a pull request (42746527773). `paths-ignore` is still only the two coordination files (`ci.yml:15, 18`); no workflow has `concurrency:`. |
| RB-001.8 | **CONFIRMED, with one addition** | 400 single-write commits. 1K base: the first 20 commits grow the file **57,344 bytes each, exactly** (the published figure); commits 129–400 average **1,777 bytes**. **New:** at a 10K base the warm-up lasts 64 commits, not 20, because the pin cap is a quarter of the live pages; after it the mean is 4,668 bytes with swings of ±700 KB. So "sample after N commits" should be "sample after the retained-snapshot count stops rising". |
| RB-001.9, .10, .12, .13, .15, .16 | **NOT TESTABLE** (ideas) | Nothing to run. Unchanged. |
| RB-001.11, .14 | **NOT TESTABLE** (accepted ideas) | Nothing to run. |
| RB-001.17 | **CONFIRMED** | Part 1. |
| RB-001.18 | **CONFIRMED at all three levels; the Android half is NOT TESTABLE HERE** | Engine, v8 and v9: 10 flushed, 50 more written, handle dropped, reopened: **10**. Binding: every `remember` returned Ok; after the object was freed and the file reopened: **10 of 60**. Swift `MnemoStore`: 5 flushed, 20 more, store released with no `close()`: **5 of 25**. A file created and never flushed reopens with 0 memories. Kotlin: by reading only (`MnemoStore.kt:283-298, 321-330`: the only automatic flush is `ON_STOP`). |
| RB-001.19 | **CONFIRMED in Swift with tests; Kotlin by reading** | At the `MnemoDb` level an embedder that calls `db.len()` from `embed` works. At the `MnemoStore` level an embedder that calls `store.cacheGet` from `embed` **never gets an answer** (it gave up after its own 3 s timeout). A `count()` issued 0.2 s into a 1.5 s embed waited **1.30 s**; `closeForBackground()` in the same position waited **1.33 s**. |
| RB-001.20 | **NOT TESTABLE HERE** (needs a device) | The three code paths read as described at `f79715e` (`Lifecycle.swift:124-127, 129-144, 151-168`). Exact device checks are in the run-sheet block below. |
| RB-001.21 | **NOT TESTABLE HERE** (no JDK; the Keystore needs a device) | Reads as described (`MnemoKeystore.kt:62-92` maps every unwrap failure and every `IOException` to `KeyMissing`; `:46-58` writes and renames with no sync; `:182-205` generates when `getKey` is null). The Swift counterpart **was** tested and behaves correctly: a key store that fails once with `OSStatus -25308` surfaces `.keychain(status: -25308, operation: "read")`, not `keyMissing`, and the next call opens the file. A JVM test recipe is in `RB-002.4`. |
| RB-001.22 | **CONFIRMED** | Failpoint on the first write of the final flush. Engine, v8 and v9: `close()` returns the I/O error, the handle is gone, and the reopened file has **10 of 40** memories. Binding: `close()` errors, the retry `flush()` returns `Closed`, reopen has 10 of 40. Contrast: the same failure inside `flush()` keeps the handle, and a retry saves all 40. The Swift `close()` has the same shape (`MnemoStore.swift:307-312` clears `db` before flushing). |
| RB-001.23 | **Items 1, 2 and 4 CONFIRMED; item 3 NOT TESTED** | Item 2 is worse than written: see below. Item 1: `Kdf`, `Crypto` and `Serialize` each map to `Corrupt`, and so does a file that isn't a `.mnemo` file ("bad magic"). Item 4: the engine says "this build opens v4 to v9; a newer file needs a newer mnemo-engine"; the binding says only "unsupported .mnemo format version 42". Item 3 (a poisoned handle) needs a panic inside the engine and wasn't provoked. |
| RB-001.24 | **CONFIRMED on the current tree, v8 and v9, and through the binding** | Four plain `remember` calls. Recall returns **2 of 4** ("A normal" and "D forged-key"). `history("current-release")` returns the forged D; a real keyed write then reports `Superseded` and stamps `_mnemo.superseded_by` on D. `thread_export("t1")` returns the forged turn C. `len()` still counts all of them. Through `metadata_json` in the binding: `Added`, and the memory never appears in recall. |
| RB-001.25 | **CONFIRMED, with one addition** | 40 commits of 100 adds keep up to 24 snapshots. Then 20 commits of 100 deletes and 100 adds: 24, 18, 14, 11, 12, 9, **7, 8, 7, 7, 6, 7, …, 6**; the oldest restore point is 5–7 commits back. **New:** the engine already has the "minimum count" that was deferred to Phase 15. An explicit `set_max_snapshots(16)` replaces the policy (`snapshots.rs:135-142`): the same churn kept exactly 16 throughout, at 9.3 MB against 7.7 MB. It just isn't exposed in the mobile binding. |
| RB-001.26 | **CONFIRMED** | `mnemo/fuzz/fuzz_targets/` still holds the same five targets. The bound: a **2,067-byte** zstd frame that declares 64 MiB is accepted by `decode_v9_memory`, and the process's resident size rose by 66 MB; 65 MiB is refused. |
| RB-001.27 | **CONFIRMED by reading** | `MnemoStore.swift:17, 28` and `MnemoStoreConfiguration.kt:14, 25`. Nothing to measure. |
| RB-001.28 | **Part 1 CONFIRMED; part 2 REFUTED** | Part 1, with the test key store: a store created `.deviceOnly` and reopened as `.iCloudSynced` has its file's backup exclusion flipped to `false` while the stored key's policy is still `.deviceOnly`. Part 2 was wrong: a read-only open with a **wrong** key fails at the unwrap, before the catalog loads. Measured: 0.054 ms at 10K and 0.109 ms at 50K, against 3.15 ms and 15.56 ms for the right key (n=50). The probe costs one extra full open for the matching key, not one per stored key. |
| RB-001.29 | **CONFIRMED** | A release-profile binary in another crate calls `__vector_column` and `__catalog_shape` with no feature enabled. With `MNEMO_DEV_ANN_MIN_FACTS=100` in the environment, the same release binary builds a 45-partition index at 2,000 facts; without it, none. |
| RB-001.30, .32 | **CONFIRMED** | Results are in `SECURITY.local.md`. |
| RB-001.31 | **NOT TESTABLE HERE** | Nothing to run; `release.yml` is unchanged since `8a9af2e`, so the reading stands. Note in `SECURITY.local.md`. |

**RB-001.1 in numbers.** v9, 384 dims, raw key, 300-byte contents, no index, `top_k` 10, `track_access` off; median of 200 warm recalls on one handle.

| Rows | Budget / page cache | Column | Warm recall |
|---|---|---|---|
| 10,000 facts | 16 MiB / 256 (mobile defaults) | resident | **2.19 ms** |
| 10,000 facts | 4 MiB / 256 | paged | **10.55 ms** (4.8×) |
| 10,000 facts | 4 MiB / 1,024 | paged | 4.49 ms |
| 21,500 facts | 16 MiB / 256 | resident | **4.80 ms** |
| 22,000 facts | 16 MiB / 256 | paged | **23.59 ms** (4.9× for 500 more rows) |
| 22,000 facts | 16 MiB / 1,024 | paged | 23.94 ms (2,200 vector pages don't fit 1,024) |
| 22,000 facts | 96 MiB / 256 | resident | 4.92 ms |
| 5,000 facts + 17,000 turn rows | 16 MiB / 256 | **paged** | **5.43 ms**, against 1.06 ms for the same 5,000 facts alone (5.1×) |

- The earlier 8.7× (22.19 ms against 2.56 ms) came from a Linux wheel. Its 1,024-page figure, 4.49 ms, is the same here.
- A paged recall costs about what a first recall costs (24.9 ms first, 23.6 ms warm at 22K): every recall decrypts the whole column again.

**RB-001.3 in numbers.** 10K × 384, mobile tuning, median of 40 fresh opens, with timers added to `read_page_uncached` in the scratch copy.
- **First recall 9.32 ms** = file reads 1.47 ms (16%) + decrypt 4.82 ms (52%) + everything else 3.03 ms (a warm recall is 2.32 ms of that). 1,000 vector pages. That matches `first_recall_10k_profile`.
- **Lever 1 (one read per run):** 1,000 pages by seek-and-read took 0.99 ms in isolation, one 8 MB read 0.70 ms. The saving is 0.3 ms alone, and at most 0.8 ms if all of the in-engine read time above that floor is recovered. RB-001.3 said 1.2 ms; that was too high.
- **Lever 3 (parallel decrypt of 1,000 pages, scoped threads, spawn included):** 4.84 ms on 1 thread, **2.67 on 2, 1.75 on 3**, 1.37 on 4 (median of 30). The estimate (1.6–2.2 ms) was right.
- **Together:** about 5.4–5.9 ms on this Mac, not the 4.5–5 ms estimated. Scaled by the perf file's iPhone-to-M1 ratio (7.08 / 9.05) that's about 4.2–4.6 ms on the iPhone: under the 5 ms target, with little room. Lever 2 (no per-page `Vec`) wasn't measured.

**RB-001.4 in numbers.** A Python loop on the internal SSD: write seven 8 KB pages, then sync; n=200, three rounds.
- `F_FULLFSYNC` **3.9–4.4 ms**; `F_BARRIERFSYNC` **0.24–0.26 ms**; plain `fsync(2)` 0.04–0.05 ms.
- A commit-shaped sequence of three syncs: full, full, full **12.0–12.7 ms**; barrier, full, barrier **5.9–6.2 ms** (n=150, three rounds).
- The engine's own single durable write here is 15.0 ms with 3 fsyncs, so about 80% of it is the three full flushes. The estimate "12 ms becomes 6–7 ms" holds on this Mac. An iPhone's flash controller may behave differently; that needs the sample app's Benchmark.

**RB-001.6 in numbers.** Three scratch copies; `cargo rustc -p mnemo-uniffi --lib --release --crate-type cdylib`, cold builds, `strip -x`.

| Variant | macOS arm64, stripped | iOS arm64, stripped | Crates compiled (macOS / iOS) | Cold build |
|---|---|---|---|---|
| A: `f79715e` as is | 2,389,376 | 2,370,648 | 106 / 78 | 50 s / 47 s |
| B: `opt-level = "s"` for every dependency except `aes`, `aes-gcm`, `polyval`, `ghash`, `zstd-sys` | 2,389,440 (**+64**) | 2,370,720 (**+72**) | 106 / 78 | 63 s / 46 s |
| C: `clap` and `rpassword` behind a default-on `cli` feature; the binding with `default-features = false` | 2,389,376 (**identical**) | 2,370,648 (**identical**) | 91 / 65 | 49 s / 46 s |

- LTO already drops the CLI's dependencies from the library, and the code that dominates it is compiled in the engine crate (generics are instantiated there) or is zstd's C.
- What's left of the item: 13–15 fewer crates per binding target, worth about a second here. It's tidy, not a saving. The `panic = "abort"` warning stands.

**Run-sheet checks for what couldn't be tested here** (for `examples/mobile/README.md`; each gives today's expected result).
1. **RB-001.20, long call (iPhone, sample app, 100K Benchmark file):** start `buildIndex`, press Home after 2 s, leave it for 60 s, then check the device log. Today: the process is killed when the allowance ends; with the file in an App Group container the termination reason is `0xdead10cc`. After a fix: the call returns a typed "interrupted" error and the file reopens with no index.
2. **RB-001.20, expired activity (iPhone):** from a `BGAppRefreshTask` expiration handler (or 31 s after entering the background), call `store.remember`. Log `expired` inside `beginActivity`. Today: the call proceeds with `expired == true` and no assertion held. After: a typed "no background time" error.
3. **RB-001.20, extension (iPhone, App Group store):** a share extension writes one memory and is dismissed without `close()`; then open the app and write. Today: `Locked` in the app until the extension process dies. After: the extension closes the file after each call.
4. **RB-001.18, Android foreground kill (emulator):** in the sample's Notes tab save 5 notes, then `adb shell am force-stop <package>` with the app still in the foreground; relaunch. Today: 0 of the 5 (unless an earlier `ON_STOP` happened). After: all 5.
5. **RB-001.18, Android background work (emulator):** enqueue a `WorkManager` job that calls `store.remember` three times, press Home first so the process is stopped, let the job run, then `adb shell am kill <package>`; relaunch. Today: the three are missing. After: present.
6. **RB-001.19 and .21 on Kotlin:** the two JVM tests in `RB-002.4` (re-entrant embedder; a `KeyWrapper` that throws once), run with `gradle testDebugUnitTest`.

---

**New items**

#### RB-002.1 — [CORRECTNESS] [MEDIUM] `release.yml`: the post-publish "Resolve the published package" step can't pass, the dry run never runs it, and no release job has a timeout
- **Status:** `VERIFIED` (`v0.12.1`, tag run 42919069975)
- **Context / Evidence:**
  - `.github/workflows/release.yml:481-501` at `f79715e` (unchanged since `8a9af2e`): after the push to `mnemo-swift`, the step writes a package named `Check` with one target and no product, then runs `xcodebuild build -scheme Check -destination 'generic/platform=iOS Simulator' -quiet`.
  - `:482`: `if: env.DRY_RUN == 'false'`, so the dry run (42743609144) skipped it. Its first execution ever was the real v0.12.0 release.
  - **Run 42748000883:** every step of the Swift job up to and including "Push to mnemo-swift" succeeded by 03:28:23 UTC. "Resolve the published package" started then and was still `in_progress` at 04:13 UTC, 45 minutes later.
  - **Reproduced here** (Xcode 27.2, the same `Package.swift` and command, in a scratch directory): the package graph resolves in about 13 s, then `xcodebuild: error: The workspace named "Check" does not contain a scheme named "Check"` (exit 65). `xcodebuild -list` shows the only scheme is **`Check-Package`**. I can't see the runner's log while the job runs, so I can't say why it hangs there instead of failing; either way the step can't succeed as written.
  - The sibling script already handles this: `mnemo/bindings/swift/scripts/check-release-package.sh:126-131` asks `xcodebuild -list -json` for the scheme.
  - `grep timeout-minutes .github/workflows/`: only `android-emulator.yml:25` has one. The release jobs run to GitHub's 6-hour default.
- **Finding:**
  - The published package is fine (Part 1: it resolves and builds). The check of it is what's broken, so the release run can't end green, and a hung macOS job holds a runner for up to six hours.
  - PLANNER-A's HANDOFF calls this step "the end-to-end check" for the `mnemo-swift` deviation in the dry run. It gave no signal.
  - AGENTS.md has the Worker stop on a red gate. A release whose last job is red or hung, with all four registries already published, is exactly the state where nobody should improvise.
- **Recommendation:**
  1. Replace the `xcodebuild` line with `swift build` in the consumer directory (9 s here; it resolves the URL, verifies the checksum and compiles `import Mnemo`), or discover the scheme as the sibling script does.
  2. Make the dry run exercise the step: resolve the previous published tag when `DRY_RUN` is true. A step that only ever runs after an irreversible publish will keep failing first in production.
  3. Move it to its own job (`verify-published`) after both mobile publishes, so a failed check is reported as a failed check and doesn't make the publish job look failed.
  4. Add `timeout-minutes` to every job in `release.yml` (30 for the two mobile jobs is three times their measured 7–10 minutes).
  5. For this run: nothing needs re-publishing. Cancelling it is the maintainer's call; the WORKLOG's v0.12.0 DONE should record the run as "published, post-publish check broken", not as green.
  6. This belongs with RB-001.17 in MOBILE-2 step 1, before any further tag.
- **Update, 2026-11-10 05:15 UTC (the fix read at `6bc592c`; not run by me):**
  - **Already queued and now committed.** The Worker reached the same diagnosis on its own (`PLAN.md`, "WORKER — BLOCKED — v0.12.0 …": the run was cancelled at 05:02:46 after the step had hung for 94 minutes), and PLANNER-A dispatched the fix as MOBILE-2a step 2. Commit `6bc592c` implements recommendations 1, 2 and 4: `scripts/check-published-package.sh` takes the scheme from `xcodebuild -list -json`, resolves and builds as two steps without `-quiet`, and sets `GIT_TERMINAL_PROMPT=0`; the dry run checks the newest existing `mnemo-swift` tag (`release.yml:570-594`); every job has `timeout-minutes: 45` and every step 10 to 20.
  - **RB-001.17's gate, by reading:** `mobile-prepare` now needs all six engine jobs, runs on `!cancelled()`, and its first step fails unless each result is `success` on a tag (a publish job may be `skipped` only when its `ENABLE_*` variable isn't `true`), or `skipped`/`success` on a manual run (`release.yml:352-405`). I walked four cases through it: a failed CLI leg, a failed publish with its variable on, a timed-out job, and the dry run. Each ends where it should; a failed gate leaves both mobile jobs `skipped`.
  - **Neither item is marked `VERIFIED`.** The commit is on a PR branch, the dry run that exercises it (42760637871) was still in progress at 05:14 UTC, and nothing has shipped.
  - **Still open from this item:** recommendation 3 only. The check still runs inside the Swift publish job, after the push. `RELEASING.md:91-93` now says what that means (a failure after the push can't be re-run, because the push step refuses an existing tag; fix forward in the next version). With tight step limits, a slow runner is now a more likely cause of that state than before. A separate `verify-published` job would be re-runnable by itself. Low priority.
- **Verification, 2026-11-11 00:30 UTC — `VERIFIED` at tag `v0.12.1` (`6e32b68`); the fix is `6bc592c` and `86e6836`.** Checked against the tag run, the dry run before it, the workflow as tagged, and the published packages.
  - **The tag run, 42919069975** (`push`, attempt 1, at `6e32b68`): `success`, all 20 jobs `success`, nothing re-run. In `mobile (Swift publish)`:
    - "Push to mnemo-swift": success in 12 s ("pushed main and tag 0.12.1");
    - **"Resolve the published package": success in 35 s.** The log shows the zip fetched and checked first ("10551138 bytes, checksum matches"), then `Mnemo: https://github.com/toarchkumar/mnemo-swift @ 0.12.1` and "resolved; scheme Check-Package";
    - **"Build against the published package": success in 46 s:** `xcodebuild build -scheme Check-Package …`, `** BUILD SUCCEEDED **`, "resolved and built mnemo-swift 0.12.1 from GitHub". Runner image `macos-26-arm64`, Xcode 26.6.
  - **The dry run, 42901448023** (`workflow_dispatch`, `release-0.12.1` at `6e32b68`): `success`. The same two steps ran, against the newest existing version: "dry run: checking the newest published version, 0.12.0", scheme `Check-Package`, `** BUILD SUCCEEDED **` (34 s and 31 s). So the steps were exercised before the tag, which is what failed to happen for v0.12.0.
  - **The workflow as tagged** (`release.yml` at `6e32b68`): 12 jobs, each with `timeout-minutes: 45`, and all 60 steps with a limit of their own. The two check steps are at `:682-705` and call `scripts/check-published-package.sh`, which takes the scheme from `xcodebuild -list -json` and, since `86e6836`, reads package credentials from netrc, downloads and checks the zip with `curl` before `xcodebuild` touches it, and runs each `xcodebuild` under a watchdog (300 s to resolve, 600 s to build). The longest job in the tag run took 349 s.
  - **The published packages, checked directly:**
    - **crates.io:** `mnemo-engine 0.12.1`, created 23:38:37 UTC, not yanked;
    - **PyPI:** 0.12.1 with 6 files (5 wheels and the sdist);
    - **Maven Central:** `mnemo-android:0.12.1` serves the `.aar` (2,728,983 bytes), `.pom`, `.module`, `.aar.asc` and the sources jar; `maven-metadata.xml` has `release` 0.12.1;
    - **`mnemo-swift`:** tag `0.12.1` is an annotated tag (`89254f95`) on commit `aef603cb05eb`; its `Package.swift` points at `releases/download/v0.12.1/MnemoFFI-0.12.1.xcframework.zip` with checksum `f2b108f0…1994`;
    - **the zip on the GitHub Release** (13 assets, not a draft) is 10,551,138 bytes, and both `shasum -a 256` and `swift package compute-checksum` of my own download give that checksum.
  - **An independent consumer on this Mac** (Swift 6.4.2, scratch folder): a package depending on the published sources at tag `0.12.1` (`aef603cb05eb`) with the published binary target. SwiftPM downloaded the zip and verified its checksum, `swift build` completed, and a small program made a file with `MnemoDb.createWithKey`, stored a memory, closed, reopened and printed "reopened with 1 memory; format v9".
  - **By recommendation:** 1, 2 and 4 are done and seen working. 5 and 6 are done (the Worker's BLOCKED and PLANNER-A's VERIFIED record v0.12.0 as published with a broken check and a cancelled run; MOBILE-2a landed before this tag). **3 was not adopted:** the check still runs inside `publish-swift`, after the push (`release.yml:614-705`), and `RELEASING.md` documents the consequence. That doesn't hold up `VERIFIED`: the defect raised here is fixed.
  - **Two things this does not establish.**
    1. **Why v0.12.0 hung on the runner was never confirmed.** `86e6836` guards against the likeliest cause (a Keychain credential prompt) and bounds any hang, and the wrong scheme is gone. One green tag run and one green dry run are the evidence so far.
    2. **RB-001.17's gate was only seen passing** ("The engine jobs succeeded", success). A green run can't show it refusing. RB-001.17 isn't marked here.
  - **An observation, not a new item.** On this Mac, SwiftPM's own download of the 10.5 MB zip took **218 s** in the attempt that finished, and two earlier attempts hadn't finished after 150 s and 338 s when I stopped them; `curl` fetched the same URL in 1 s each time. A control against `0.12.0`, which resolved and built in 9 s here about 21 hours earlier, stalled the same way, so it's this machine at that moment (load average about 29: the Worker's test suite was running, and the repo's iCloud file provider was busy), not the 0.12.1 package. I didn't investigate the cause. On the runner the resolve step took 35 s and 34 s. It's the case recommendation 3 is for: if a runner ever has a slow minute like that, the 300 s watchdog fails the check after the push, and that job can't be re-run.
  - **How:** read-only. No cargo build and no benchmark ran (this check compiles no Rust); the Swift builds were in my scratch folder. The Worker had a PR open on `release-0.12.2` throughout.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002). Already fixed in MOBILE-2a, in v0.12.1: the scheme comes from `-list -json`, packages are resolved as their own step, there are time limits, and the dry run exercises the check. REVIEWER-B marks it `VERIFIED` after the v0.12.1 tag run.

#### RB-002.2 — [SECURITY] [MEDIUM] Held in `SECURITY.local.md`
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** One finding from the RB-001.30 reproduction, written up in the gitignored `SECURITY.local.md` with the tests. It refines an accepted fix and its acceptance test; it doesn't contradict the triage. Per AGENTS.md it stays out of this file until the fix ships.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10), as SEC-2b in v0.12.2. The full triage is in `SECURITY.local.md`.

#### RB-002.3 — [PERF] [MEDIUM] The first `stats()` on a handle decrypts every record for an agent list the mobile binding throws away; fix it before MOBILE-2 puts the recall mode in `stats()`
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - `mnemo/src/store/maintenance.rs:290-302`: on the first call per handle `stats()` runs `self.memories()?` and collects each record's `agent_id`. Later calls reuse the set. `mnemo/src/store/crud.rs:318`: a `delete` clears it, so the next `stats()` pays again.
  - `maintenance.rs:319-322`: `stats()` also calls `ensure_ann()`, which loads a persisted index.
  - `mnemo/bindings/uniffi/src/db.rs:579-589`: the binding's `DbStats` has `memories`, `deleted`, `dimensions`, `file_bytes`, `format_version`, `has_index`, `wal_pages`. No agents.
  - **Measured** (v9, 384 dims, release, median of 20 fresh handles after one recall): first `stats()` **2.29 ms at 2K, 12.32 ms at 10K, 25.67 ms at 20K**. Second call: 0.00–0.02 ms. For scale, at 10K an open is 3.5 ms and a first recall 9.3 ms.
  - Related: R-001.8 part 2 (REVIEWER-A, VERIFIED in FIX-1) is the change that made this lazy and cached. That fix is real; this item is about the first call, which the phone lifecycle makes frequent.
  - Queued work that depends on it: MOBILE-2 has `stats()` report the recall mode (RB-001.1's triage), and INSP-1 adds the retained-snapshot fields (RB-001.25's triage).
- **Finding:**
  - On iOS every call made in the background runs on a fresh handle (`MnemoStore.swift:401-408`), and so does the first call after every return to the foreground. An app that checks the recall mode, as the MOBILE-2 docs will tell it to, pays a full pass of record decrypts each time: more than the open and the first recall together.
  - The pass also pushes every record page through the 256-page cache.
  - The binding does all of that work and then discards the result.
- **Recommendation:**
  1. Take the agent list out of the default path: keep `Stats.agents` for Rust and Python callers, but fill it only when asked (`stats()` cheap, `agents()` or `stats_full()` separate), or derive the distinct set from the v9 catalog rows' `agent_hash` and decrypt one record per distinct hash.
  2. In the binding, call the cheap path. Don't load the index to answer `has_index`; the header already says whether one is persisted.
  3. Put the recall mode and the retention fields on the cheap path, and add a `device_budget` cell "first stats() on a fresh handle" so it can't regress.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002) → MOBILE-2e (v0.12.2). `stats()` takes the cheap path: no record decrypts and no index load (the header says whether one is persisted). Recall mode and retention fields go there, and the agent set is filled only by an explicit `agents()` (or `stats_full()`) for Rust and Python. Plus a `device_budget` cell for the first `stats()` on a fresh handle.

#### RB-002.4 — [PLAN-REFINEMENT] [MEDIUM] Ten in-process regression tests for MOBILE-2 and the RB-001.22–.24 fixes, none needing a device or an emulator
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:** MOBILE-2's accepted tests for RB-001.18 are `am force-stop` on the emulator and a kill on the simulator; for .19, "a re-entrancy test in each suite". Every reproduction in this batch ran in-process: under `cargo test` or under `swift test` on macOS, in seconds. RB-001's own scripts were lost with its sandbox, so the recipes are written down here.
- **Finding:** The emulator and simulator jobs are the slowest and least-run legs of CI (the swift job is 730 s). The behaviours MOBILE-2 changes can be pinned by ordinary unit tests that run on every PR, with the device checks kept as confirmation.
- **Recommendation:** add these with their fixes. "Today" is the result at `f79715e`.

  | # | Where | Test | Today | After the fix |
  |---|---|---|---|---|
  | 1 | `mnemo-uniffi` tests | Create with a key, flush 10, `remember` 50 more, drop the `Arc<MnemoDb>` without `close`, reopen, `len()` | 10 | unchanged: this is the engine contract the stores must cover |
  | 2 | Swift `StoreTests` | 5 writes, `flush()`, 20 more, set the store to `nil`, new store on the same file, `count()` | 5 | 25 under `.coalesced` once the delay has passed, and under `.everyWrite` at once |
  | 3 | Kotlin `StoreTests` | The same with `MnemoStore` and a cancelled scope | not run | as 2 |
  | 4 | engine `tests/` (failpoints) | 10 flushed, 30 more, `set_writes_until_fail(0)`, `close()`, reopen | `Err`, 10 of 40, no handle left | the handle (or a `try_close`) survives; a retry gives 40 |
  | 5 | `mnemo-uniffi` tests | The same through `MnemoDb.close()`, then `flush()` | `close` errors, `flush` is `Closed`, 10 of 40 | `flush` retries and saves 40 |
  | 6 | engine `tests/` (failpoints) | Fixture: for `n` in `0..`, create, flush 10, write 5, `set_writes_until_fail(n)`, `flush()`, drop; stop at the first `n` where `open_read_only_with_key` returns `NeedsWriteOpen` (`n` = 3 on v9, 6 on v8 for this shape). Then `open_read_only_with_key` with a **wrong** key | `NeedsWriteOpen` (binding: `Invalid`) | `WrongPassphrase`, or a header-only `key_opens` that answers `false` |
  | 7 | Swift `StoreTests` | Copy fixture 6 to the store's path as plain bytes (no key-id attribute), key store holding the right key (and, in a second case, a wrong key first), `count()` three times | `MnemoError.Invalid` three times, in both cases; once the id is stamped by hand, 15 | opens, 15, and stamps the id |
  | 8 | engine `tests/`, v8 and v9 | Plain `remember` of four memories: normal; `_mnemo.superseded_by` + `_mnemo.invalid_at`; `_mnemo.kind = "turn"` + `_mnemo.thread`; `_mnemo.key`. Then recall, `history`, `thread_export`. Also `metadata_json` through the binding | 2 of 4 recalled; the forged key is in `history`; the forged turn is in `thread_export` | each forged write is `Invalid`; nothing stored |
  | 9 | Swift `StoreTests` (and Kotlin) | (a) An embedder whose `embed` starts a detached task that awaits `store.cacheGet` and waits for it with a 3 s semaphore timeout. (b) An embedder that sleeps 1.5 s; issue `count()` and `closeForBackground()` 0.2 s in and time them | (a) times out; (b) both wait about 1.3 s | (a) returns at once; (b) both return in milliseconds |
  | 10 | Swift `StoreTests` | Create `.deviceOnly`, close, reopen `.iCloudSynced`; read the file's `isExcludedFromBackup` and the key store's recorded policy | file `false`, key still `.deviceOnly` | both migrated, or a clear error |

  - **Kotlin recipes, not run here:**
    - RB-001.19: an embedder whose `embed` calls `runBlocking { withTimeout(3_000) { store.cacheGet("emb", texts[0]) } }`. Today it should throw `TimeoutCancellationException`, because `withDatabase` holds the store's `Mutex` (`MnemoStore.kt:321-330`).
    - RB-001.21: `MnemoKeystore(dir, wrapper)` with a `KeyWrapper` whose `unwrap` throws `java.security.ProviderException("keystore busy")` once and then delegates to `SoftwareKeyWrapper`. Today the first `key(id)` throws `MnemoStoreException.KeyMissing`. After the fix it throws the retryable `Keystore("unwrap", …)` and the second call returns the key.
  - The security counterpart (a header graft) is in `SECURITY.local.md`.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002). The ten in-process tests are MOBILE-2e's acceptance tests, each with its before and after result.

#### RB-002.5 — [PERF] [LOW] What an app should do at the recall cliff: raise the budget first, build the index second; the MOBILE-2 table should say so
- **Status:** `VERIFIED` (RB-003, `afd65b3`)
- **Context / Evidence:**
  - RB-001.1's triage puts three things in MOBILE-2: the budget sized by live facts, `stats()` reporting `exact_resident`, `exact_paged` or `indexed`, and a rows-by-dimension table for the default budget. Automatic index building is deferred to PERF-2.
  - **Measured at 22,000 facts × 384** (the first size over the 16 MiB default, which needs 17.0 MB):
    - defaults, no index: **23.59 ms** warm;
    - budget raised to fit (96 MiB set; 17 MB used): **4.92 ms**;
    - defaults plus `build_index()`: **4.08 ms** warm, but the build took **49.8 s** on the M1 Pro and the first recall on a fresh handle was 30.2 ms;
    - page cache raised to 1,024: 23.94 ms, no help, because 2,200 vector pages don't fit. (At 10K, where 1,000 pages do fit, it brings 10.55 ms down to 4.49 ms.)
  - `build_index` is one of the calls RB-001.20 says can't be interrupted, and `V9_ANN_MIN_FACTS` (20,000) means it does nothing below that.
- **Finding:** Once `stats()` tells an app it is in `exact_paged`, the app has two remedies with very different costs: about 1 MB more memory, or a 50-second (on a Mac) uninterruptible call that holds the store. The docs MOBILE-2 plans describe the limit but not the remedy, and the obvious-looking one (`cachePages`) only works on small files.
- **Recommendation:**
  1. In the rows-by-dimension table, add a column "budget to stay resident" (`rows × (2 × dims + 4)` bytes) and say plainly: raise `vectorCacheBudgetBytes` first; build the index only when the column no longer fits the memory the app can spare, and only in a maintenance window (RB-001.9).
  2. Have the recall-mode report carry the number of bytes the column needs, so an app can act without the table.
  3. Record the index build time per size in the READMEs' "Large calls hold the store" section (the perf file already has `build_secs`).
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-10, after RB-002). The MOBILE-2d rows-by-dimension table gains "what to do at the cliff": raise `vectorCacheBudget` first (cheap), build an index only for large files, and expect it to take time. Also expose `setMaxSnapshots` in both stores (RB-002's addition to RB-001.25).

---

### Batch `REV-006` — 2026-11-10 (REVIEWER-A: 3-Tier Market Research, Device-Level Multi-Agent Harness Strategy & Double-Run Dogfood Cutover @ `3f9ac4d` / `v0.12.2`–`v0.12.3`)

**Market & Positioning Summary (Hyperscale vs SaaS vs Device / Personal Harness):**
- **Tier 1 — Hyperscale Cloud Vector/DB Infra (`Pinecone`, `Turbopuffer`, `Qdrant Cloud`, `Milvus/Zilliz`, `Supabase pgvector`, `AWS Bedrock Memory`):** Optimize for `10M–10B` multi-tenant cloud vectors and managed SLAs, with server-held keys, `20–150 ms` network RTT, and zero offline/local-sandbox capability. Not MNemo's battleground.
- **Tier 2 — Agent-Memory SaaS & Frameworks (`Mem0` / `OpenMemory`, `Zep` / `Graphiti`, `Letta` / `MemGPT`, `Supermemory`, `Cognee`, `LangMem`):** Python/TS middleware layers over external server DBs (`Qdrant`, `Neo4j`/`FalkorDB`, `Postgres`) that invoke an LLM on every write (`500–3,000 ms`, recurring API cost, sending private code/conversations to external APIs). None ship as a single portable zero-knowledge encrypted file or a `2.6 MB` native mobile/CLI engine.
- **Tier 3 — Device, Embedded & Local-First Harnesses (`sqlite-vec` + SQLCipher, `LanceDB`, `Turso/libSQL`, `Pieces OS`, `Basic Memory`, `Beads (bd)`, and Git + Markdown `CLAUDE.md` / `AGENTS.md` / `.cursor/rules` / `PLAN.md`):** This is MNemo's primary battleground. Raw embedded stores (`sqlite-vec`, `LanceDB`) have zero agent semantics (no essence, threads, keyed supersession, byte budgets, or zero-config encryption). Conversely, Git + Markdown harnesses (`CLAUDE.md`, `PLAN.md`, `Beads` JSONL) have zero semantic/hybrid search, zero encryption for private transcripts or embargoed notes (`SECURITY.local.md`), and zero atomic multi-agent coordination — causing context collapse (`PLAN.md` is now `15,966` lines / `880 KB`) and forcing the human user to act as the copy-paste message bus.
- **Core strategic finding:** With `PERF-1` (`v0.10.0`), `FMT-9` (`v0.11.0`), and `MOBILE-1`/`2` (`v0.12.0`–`v0.12.3`), MNemo's on-device engine physics (`1.33 KB/mem`, `2.1 ms` open, `< 2 ms` warm recall) and mobile packages are already in place. The highest-leverage gap now is **device-level multi-agent orchestration** (`12.4 + 12.3 + 12.2 + 13a`), which should be pulled ahead of `PERF-2` and validated via a **2-PR Double-Run Dogfood Cutover (`HARNESS-DOGFOOD-1`)** on this repository itself.

---

#### R-006.1 — [PRODUCT] [HIGH] Post-`INSP-1` sequencing: pull the Local Multi-Agent Harness stack (`12.4 + 12.3 + 12.2 + 13a`) ahead of `PERF-2`, splitting Phase 13 into Local Coordination (`13a`) vs Multi-Party Crypto (`13b`)
- **Status:** `ACCEPTED`
- **Context / Evidence:** `PLAN.md:78-80, 1243-1386, 1546-1592` (`Authoritative execution order`: `v0.12.3 → REL-1 → SAMPLE-1 → INSP-1 (v0.13) → PERF-2 → 12.4 + 12.2 → 12.3 → 12.5 → Phase 13`), `mnemo/src/store/open.rs` (`try_lock_exclusive` single-writer lock).
- **Finding:** Two sequencing bottlenecks currently delay MNemo's strongest desktop/device adoption wedge (developers orchestrating multiple local agents across Claude Code, Cursor, CLI, and worktrees):
  1. **`PERF-2` is slotted ahead of `Phase 12.4` / `12.2` / `12.3`:** `FMT-9` + `MOBILE-2` already achieved `1.7–2.5 ms` warm recall and `2.1 ms` open. Shaving cold 10K exact-scan first-recall from `~7 ms` to `≤ 5 ms` (`PERF-2`) is an incremental polish, whereas `Phase 12.4` (concurrent multi-process access) fixes a hard blocker: today, if Claude Code holds `project.mnemo` via `mnemo serve --mcp`, Cursor, a second terminal agent, a hook, or `mnemo inspect` cannot open the file for writing (`MnemoError::Locked`).
  2. **Phase 13 bundles local multi-agent coordination with multi-human cryptography:** In `PLAN.md:1279-1384`, `Phase 13` groups **local coordination primitives** (`13.1` messages + task queue, `13.3` atomic claims + presence, `13.5` `public | team | private` visibility, `13.6` freshness) together with **multi-party cryptographic primitives** (`13.2` LUKS/X25519 per-agent key slots, `13.4` Ed25519 record signatures, `13.7` diverged-file merge). When a developer orchestrates 3–5 local agents (`Planner`, `Worker`, `Reviewer`) on one machine, all local sessions share the user's OS Keychain key (`12.3`) — they need `13.1 + 13.3 + 13.5 + 13.6` (**`Phase 13a`: Local Multi-Agent Orchestration**) immediately, and do not need to wait for X25519/Ed25519 (`Phase 13b`) before dogfooding the unified shared brain.
- **Recommendation:**
  1. Keep the near-term queue through **`v0.12.3 → REL-1 → SAMPLE-1 → INSP-1 (v0.13.0, with 12.5 export + v8 sunset)`**.
  2. Immediately after `INSP-1` (`v0.13.0`), pull the **Local Harness & Orchestration track (`12.4 → 12.3 → 12.2 → Phase 13a [13.1 + 13.3 + 13.5 + 13.6]`) ahead of `PERF-2`**, so MNemo can dogfood and ship multi-agent harness orchestration in `v0.14.0`, followed by `HARNESS-DOGFOOD-1` (`R-006.3`), `PERF-2`, and `Phase 13b` (`13.2 + 13.4 + 13.7`) + `Phase 14`.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-11). The order after v0.12.3: REL-1 → SAMPLE-1 → INSP-1 (v0.13) → **the 12.4 design note first**, then 12.4 → 12.3 → 12.2 → Phase 13a (13.1, 13.3, 13.5, 13.6) → HARNESS-DOGFOOD-1 → PERF-2 → Phase 13b (13.2, 13.4, 13.7) → 14 → 15. In 13a, visibility is a filter, not a security boundary, until 13b. FLAKE-1 (v0.12.3) feeds the 12.4 note: agent apps spawn children constantly.

#### R-006.2 — [PRODUCT] [HIGH] Personal-harness edge over Git (`CLAUDE.md`/`PLAN.md`/`Beads`): `git worktree` resolution, zero-embedder BM25 mode, `mnemo harness init` (Claude Code + Cursor), deterministic `--git` projection, and `--personal` brain mount
- **Status:** `ACCEPTED`
- **Context / Evidence:** `PLAN.md:1025-1055` (`12.2` Claude Code plugin, `12.3` zero-prompt onboarding, `12.4` shared access, `12.5` readable export), `mnemo/src/store/crud.rs` (`remember_text` requires a configured embedder).
- **Finding:** Comparing MNemo against how developers actually run multi-agent personal harnesses in 2025–2026 (Claude Code + Cursor + CLI agents + `git worktree` checkouts + Git-tracked `CLAUDE.md`/`AGENTS.md`/`Beads`) surfaces five concrete feature refinements needed across `12.2`–`12.5` so MNemo beats both raw Git files and `sqlite-vec`/`LanceDB`:
  1. **`git worktree` path resolution (`12.4` / `12.2`):** Multi-agent workflows routinely run parallel workers in separate `git worktree` directories (like our own `release-0.12.3` / `v0123-2-flake1` branches or subagent worktrees). A relative path like `test/project.mnemo` or `.mnemo/brain.mnemo` resolves to a separate (or missing) file per worktree. Resolving the default repo brain path relative to `git rev-parse --git-common-dir` (or a configured shared path) lets every worktree share one live `.mnemo` brain automatically.
  2. **Zero-embedder BM25 + Essence out-of-the-box (`12.3`):** Today, calling `remember --text` or MCP `remember` without an embedder returns an error, forcing users to configure `--embed-cmd` before storing their first fact. Since format v9 (`v0.11.0`) has a native BM25 lexical index and structured `Essence` / keyed records, allowing a v9 database created with `dimensions = 0` (or lazy/unconfigured embedder) to store text memories and serve BM25 keyword + `Essence` + keyed recall immediately — and seamlessly backfilling vectors when `12.3`'s local embedder is attached — removes the #1 onboarding cliff vs plain Markdown/`Beads`.
  3. **Multi-tool `mnemo harness init` (`12.2` widened beyond Claude Code):** Instead of a Claude-Code-only plugin, `mnemo harness init` should detect and wire **both** Claude Code (`.claude/` hooks + MCP) and Cursor (`.cursor/mcp.json` + `.cursor/rules/mnemo.mdc`) plus the shared `AGENTS.md` pointer in one command.
  4. **Deterministic Git Projection (`12.5` + `13.5`):** Developers keep `PLAN.md` and `AGENTS.md` in Git because they want human-readable `git diff`s in PRs. `12.5`'s `mnemo export` should include a `--git` mode that writes a deterministic, diff-clean Markdown/JSONL summary of **only `public` and `team` Essence, active Tasks, and Decision Ledger rows** (excluding high-churn lock claims, raw turn archives, vectors, and `private` security notes like `SECURITY.local.md`).
  5. **Dual-scope Personal + Project Brain (`12.2` / `12.4`):** A developer's personal harness has two scopes: user-wide conventions/lessons (`~/.mnemo/personal.mnemo`) and repo-specific state (`project.mnemo`). Allowing MCP / CLI `about` and `recall` to optionally attach a secondary read-only/read-write `--personal <path>` brain gives developers cross-repo memory continuity that repo-bound `CLAUDE.md` files cannot match.
- **Recommendation:** Fold (1) `git-common-dir` worktree resolution into `12.4`/`12.2`, (2) zero-embedder v9 BM25 operation into `12.3`, (3) Cursor + Claude Code `mnemo harness init` into `12.2`, (4) visibility-filtered deterministic `--git` projection into `12.5` (starting in `INSP-1`), and (5) optional `--personal <path>` mounting into `12.2`/`12.4`.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-11), all five, folded into 12.2–12.5. (b) is a format question: today `create` refuses `dimensions == 0` (`store/open.rs:197`) and `remember_text` needs an embedder (`store/crud.rs:231`), so 12.3 decides how text is stored without a vector and how vectors are filled in later. (c) Cursor: `.cursor/mcp.json` (`mcpServers`) and `.cursor/rules/*.mdc`, re-checked when 12.2 is specified. **Renamed (user, 2026-11-11):** `mnemo harness init` becomes `mnemo onboard`, which also prints the GUIDE-1 instructions (`--guide` for the guide alone). (d) first version in INSP-1, the visibility filter with 13.5.

#### R-006.3 — [PLAN-REFINEMENT] [HIGH] Double-Run Dogfood Cutover (`HARNESS-DOGFOOD-1`): 2-PR dual-run orchestrating `Planner`/`Worker`/`Reviewer` through `test/project.mnemo` alongside `PLAN.md` to prove zero drift before retiring manual `WORKLOG` relay
- **Status:** `ACCEPTED`
- **Context / Evidence:** `PLAN.md:1207-1212` (`12.7` dogfood criterion), `PLAN.md:1271-1274` (`Phase 13` success criterion: *"this project's Planner, Worker and Reviewer loop runs through `test/project.mnemo`"*), user request (2026-11-10: *"when KEY work is done, we need to attempt to truly dogfood and move to mnemo harness to try to orchestrate (perhaps a double run to see if a UNIFIED shared brain being orchestrated works)"*).
- **Finding:** Cutting over cold-turkey from `PLAN.md` (`15,966` lines) + `reviewer_notes.md` (`1,237` lines) + `SECURITY.local.md` to `test/project.mnemo` risks losing visibility if a hook, claim TTL, or MCP tool prompt has a subtle gap. Conversely, today's `python test/scripts/project_memory.py remember` dogfooding only writes isolated post-hoc facts after a PR is already done — it does **not** test multi-agent orchestration.
- **Recommendation:** Schedule an explicit **2-PR Double-Run Dogfood gate (`HARNESS-DOGFOOD-1`)** immediately after the Key Harness Work (`INSP-1` + `12.4` + `12.3` + `12.2` + `13a`) lands:
  1. **Run 1 (Dual-Write / Blind-Read Verification PR):**
     - Seed active `Status`, `NEXT DISPATCH`, role rules, and open `R-`/`RB-` ledger items into `test/project.mnemo` (unlocked via OS Keychain, shared across all sessions via `12.4`, visible live in `mnemo inspect`).
     - `PLANNER-A` dispatches the PR by writing the spec/task/message into `test/project.mnemo` (`essence`, `task`, `post`) **and** logging in `PLAN.md`.
     - **Blind-read test:** The `Worker` (and `REVIEWER-A` / `REVIEWER-B`) starts the turn by reading **only `test/project.mnemo`** (`mnemo essence`, `mnemo task next --role worker`, `mnemo inbox`, `mnemo recall`) *before* opening `PLAN.md` or `reviewer_notes.md`, records any discrepancy between the `.mnemo` brain and the Markdown files as a dogfood bug, and dual-writes its `READY`/`DONE` or review items to both.
  2. **Run 2 (Unified Brain Primary + Auto-Projected Git Markdown PR):**
     - Once Run 1 passes with zero context loss or lock contention, `test/project.mnemo` becomes the **sole primary write target** for `Planner`, `Worker`, and `Reviewers` (including `visibility = private` findings replacing `SECURITY.local.md`).
     - Hand-editing `WORKLOG` and `Decision Ledger` tables stops; instead, the `Worker`'s backup step runs `mnemo export test/project.mnemo --git` (`12.5`) to deterministically generate the Git-tracked Markdown summary for `git diff` and GitHub backup.
- **Planner Triage:** `ACCEPTED`, modified (PLANNER-A, 2026-11-11). **Success criterion (user):** everything the project does today through git Markdown runs entirely in MNemo; see the parity checklist in PLAN.md's 2026-11-11 entry. Met when Run 2 takes one full release, dispatch to tag, with no hand-edited PLAN.md or reviewer_notes.md. **Changed:** private findings never go into the shared project brain (every agent holds its key, anyone with the key reads everything until 13b, and Run 2's `--git` export is committed; corrected 2026-11-11: `test/project.mnemo` itself is gitignored); a separate, never-committed private brain replaces `SECURITY.local.md`.

---

### Batch `RB-003` — 2026-11-12 (REVIEWER-B: the v0.13.0 READY review, before the tag, with PLANNER-A's three checks, a verification sweep of shipped RB items, and eight new items)

**What was reviewed.** `release-0.13.0` at `afd65b3` (`release: v0.13.0`), which is what the READY and the dry run 43138424847 cover. The branch head is `1afdfd7`, which adds only `.gitignore` and `AGENTS.md`. Nothing is tagged. The change since `v0.12.3` is 29 commits, 109 files, +7,996 / −914 lines.

**How.**
- **Pinned.** Every build and test ran on a scratch copy made with `git archive afd65b3`, never on the live tree, with `CARGO_TARGET_DIR=/tmp/rb-target` and `--locked`. Scratch crates depend on that copy by path; their own lock files were generated once from the repo's pins (`cargo update --workspace --offline`), then used with `--locked`.
- **No benchmarks.** One figure below is a count of bytes; nothing was timed.
- **Machine:** Apple M1 Pro, macOS 26.7.1, Rust 1.95.0.
- **Baselines at `afd65b3`, both green:** `cargo test --locked --features failpoints`: **366 passed, 0 failed** (the Worker's count). `cargo test --locked -p mnemo-uniffi`: **23 passed**. The Swift and Kotlin suites were not run here (the Swift build script calls cargo without `--locked`; this Mac has no JDK); Mobile CI is green on `afd65b3`.
- **Read-only.** I wrote this file and the gitignored `SECURITY.local.md`, nothing else. One exception to record: the repo moved to `mnemo.nosync` mid-session and my shell's working directory still resets to the old path, where an empty `~/Documents/GitHub/mnemo` folder exists. I created nothing in it.
- **Unfixed security findings** from this pass (`RB-003.1`, `RB-003.2`) are only in `SECURITY.local.md`, with the inspector's probe results.

---

**The verdict first: I would not hold the v0.13.0 tag for any item below.**

Four things are worth doing before it anyway, because they're small and the tag makes two of them permanent:
1. **`RB-003.4`: decide what ships in the crate.** Once `mnemo-engine 0.13.0` is on crates.io it can't be changed, and as staged it carries 745 KB of test fixtures and doubles in size.
2. **`RB-003.3`: one error string.** This is the release that makes v8 read-only, and a v8 file that crashed mid-commit currently gets two messages that point at each other.
3. **`RB-003.1` (security, in `SECURITY.local.md`): two sentences of help and README text.** The rest of that item can wait for v0.13.1.
4. **The CHANGELOG heading** says `[0.13.0] — 2026-11-11`; the tag will land on a later day (the Worker's READY already notes this).

The three checks PLANNER-A asked for all pass; details next.

---

**PLANNER-A's three checks**

**1. The release artifacts are built without `dev-hooks` (HOOKS-1): yes.**
- **How the feature reaches the tests:** the crate dev-depends on itself with `features = ["dev-hooks"]` (`mnemo/Cargo.toml:141-145`). A dev-dependency's features apply only to test, bench and example builds.
- **Feature resolution, checked for each release build shape** (`cargo tree --locked -e features --edges normal,build -i mnemo-engine | grep 'feature "dev-hooks"'`): the workspace root, `-p mnemo-uniffi`, `-p mnemo-node`, and `mnemo-python`: **0 matches each**. With dev edges included, as a control: 1.
- **The release workflow's build commands** carry no feature flag: `cargo build --release --target …` (`release.yml:138`), maturin `args: --release --out dist` (`:210`) and `--out dist` (`:253`), `cargo publish --no-verify` (`:394`), `cargo rustc -p mnemo-uniffi --lib --release` in both mobile scripts. Only CI's test wheel passes `--features dev-hooks` (`ci.yml:248`).
- **The artifacts themselves.** I downloaded what dry run 43138424847 built at `afd65b3` and searched each library for the two switch names (`MNEMO_DEV_`), with a string that must be there as a sanity check:

  | Artifact | `MNEMO_DEV_` | Sanity string |
  |---|---|---|
  | `MnemoFFI-0.13.0.xcframework`: `ios-arm64`, `macos-arm64`, simulator `arm64`, simulator `x86_64` | 0, 0, 0, 0 | present in all |
  | `mnemo-android-0.13.0.aar`: `arm64-v8a`, `x86_64` | 0, 0 | present in both |
  | Control: the published `mnemo-android-0.12.3.aar`, `arm64-v8a` | **2** (`MNEMO_DEV_ANN_MIN_FACTS`, `MNEMO_DEV_TURN_ZSTD`) | present |
  | The CLI, built here as the workflow builds it (`cargo build --release --locked`) | 0 | present |

- **Not covered by any artifact:** the wheels and the CLI binaries the tag will build, because a dry run skips the engine jobs. For those the evidence is the feature resolution and the build commands above.
- **`cargo package` works with the self dev-dependency:** it's stripped from the published manifest, so `cargo publish` won't trip on it. Two side effects are in `RB-003.4`.
- **`RB-001.29` is fixed at `afd65b3`.** I'll mark it `VERIFIED` once v0.13.0 is published.

**2. `mnemo export --git` never leaks vectors, turns, the cache or the embedder-identity record: confirmed, with one thing to know.**
- **Test** (scratch crate, release CLI, both formats). A file with an embedder named by a marker string, vectors with marker values and `store_float_vectors` on, three thread turns with marker text (one longer than a chunk), essence items, an exact-key cache entry and a semantic cache entry with marker values, key and model.

  | In the `--git` output | Markdown | JSON Lines |
  |---|---|---|
  | Thread turn text, turn records | absent | absent |
  | Cache value, cache key, semantic cache value, cache model | absent | absent |
  | A `vector` field, or any marker vector value | absent | absent |
  | The embedder's name (the identity record) | absent | absent |
  | `accessed_at`, `access_count` | absent | absent |
  | Essence items and timeline entries; superseded versions of a keyed fact; the thread id on essence items | present (by design) | present (by design) |

- **The thing to know:** a file that has a manifest memory (one made by `mnemo init`) names its embedder in that memory's metadata, and the manifest is a memory, so it's exported. The v8 fixture's `--git` export contains `"embedder":{"dimensions":8,"name":"fixture-embedder-r1","normalize":true}`. The identity *record* (the reserved cache entry) is never exported, which is what the documentation claims.
- **Determinism:** two exports in a row are byte-identical. They stay identical after a tracked recall, a cache write and a commit, and **across a compaction from v8 to v9**. One new unkeyed memory adds exactly one JSON line and changes none.
- **One wording nit** (in `RB-003.8`): "one new memory adds only its own lines" isn't true of a keyed write. A new version added 2 lines and changed 1, because the superseded version gains `_mnemo.superseded_by` and `_mnemo.invalid_at`. That's correct behaviour, just not what the sentence says.

**3. `action-gh-release` v3 and REL-1's mobile-off note: both right by reading. Neither can be proved before a tag.**
- **The pin is what it says.** `8a0cd651e5b2…` is the commit that the annotated tag `v3.0.4` points to ("release 3.0.4").
- **What changed from what v0.12.3 ran.** v0.12.3 used `3bb12739…` = **2.6.2**. Between it and 3.0.4 there are 61 commits: the move of the action's runtime from Node 20 to Node 24 (3.0.0), dependency updates, and four reliability fixes in 3.0.2 (small-asset uploads, draft prereleases, Gitea, clearer 404s). No input was removed or renamed.
- **The five uses** (`release.yml:68, 283, 550, 668, 967`) pass only `generate_release_notes`, `draft`, `prerelease`, `files`, `body` and `append_body`, all defined in 3.0.4's `action.yml`.
- **The attach steps keep the release body.** I read the update path in the action's source at the pinned commit (`src/github.ts:684-716`): with no `body` input it re-sends the existing body, and it preserves `draft` and `prerelease` from the existing release.
- **Node 24 actions need runner 2.327.1 or newer.** Every job runs on GitHub-hosted runners (the v0.12.1 run used 2.338.2).
- **Immutable releases are off** for the repo (`gh api …/immutable-releases`: `enabled: false`), so attaching assets after the release is published still works.
- **The mobile-off note** (`release.yml:516-553`): on a tag with `ENABLE_MOBILE_PUBLISH` not `true`, `mobile-prepare` still checks the versions, the build, publish and verify jobs are skipped by their `if:` conditions, and `mobile-skipped-note` appends one line to the release body. With `append_body: true` and a non-empty existing body, the action writes `existing + "\n" + note`. The note's two versions come from `mnemo-swift`'s newest tag and Maven's `<release>`. That's right.
  - Two small edges, in `RB-003.8`: re-running that job appends the line a second time, and a failed lookup of the Swift tags fails the job, which turns a fully published engine release red.
- **For this tag the note won't run:** `ENABLE_MOBILE_PUBLISH` is `true` (set 2026-11-11), with `ENABLE_CRATES_PUBLISH` and `ENABLE_PYPI_PUBLISH`. All four publish environments have a required reviewer, so the run will pause four times for approval.
- **What a real tag will run for the first time** (the same Dependabot PR bumped these, and a dry run skips the engine jobs):
  - `action-gh-release` 3.0.4: all five uses;
  - `download-artifact` 8.0.2 with `pattern` and `merge-multiple` (`release.yml:274-279, 321-329`). The dry run exercised 8.0.2 by `name` only. Its release notes change two things, neither of which touches this use: files are unzipped by content type, and a digest mismatch is now an error;
  - `setup-python` 7.1.0 and `upload-artifact` 7.0.2 on the macOS and Windows wheel and CLI legs (CI and the dry run used them on Linux and macOS).
  - By the release notes none of these changes behaviour here, so I don't hold the tag for it. The standing gap is `RB-003.5`.

---

**Blind spots: what else I looked at**

**Held up, no item raised.**
- **The inspector's request handling** (token, `Host`, method, paths, bind address, headers) and its page: 13 live probes and a read of both files. Results are in `SECURITY.local.md`.
- **EMBED-ID-1's identity record survives the cache's own maintenance.** It's stored as a result-cache entry in a reserved namespace, so I tried to lose it: `cache_put`, `cache_delete` and `cache_get` on the reserved namespace are refused; 50 puts into a 3-entry namespace don't evict it (budgets are per namespace); `cache_purge(None, false)` leaves it; it survives `compact_file` and is in an `export_rekeyed` copy; `cache_stats` doesn't count it. A second embedder is refused with `EmbedderMismatch`.
- **A new revision of an Apple model strands a store's text path.** The Swift docs say so plainly (`AppleEmbedders.swift:7-20`), and the engine-side re-embed is already queued (12.3). Not re-raised.
- **The committed fixtures are synthetic.** I exported all of them: grocery notes, an offsite plan, a repaint plan. No names, paths or device identifiers. `v8_key.key` is a throwaway raw key for one fixture.
- **The v8 sunset's ordinary paths.** On the v8 fixture, `about`, `list`, `verify`, `snapshots`, `cache stats` and `thread export` work and leave the file byte-identical; `index` is refused with a clear `FormatTooOld`; `compact` converts it (17 → 17 memories) and the `--git` export is unchanged by the conversion.
- **Mobile artifacts match the READY:** the Swift tree from the dry run has the `MnemoAppleEmbedder` product and no `formatVersion` on the store configuration.

**Raised:** `RB-003.3` to `RB-003.8` below.

---

**Verification of shipped RB items** (the scheduled pass; each was confirmed at `afd65b3`, by test where one is named, otherwise by reading)

| Item | Result | Evidence |
|---|---|---|
| RB-001.2, RB-001.18 | **`VERIFIED`** (v0.12.1) | Both stores default to `.coalesced()` (`FlushPolicy.swift:26`, Kotlin `FlushPolicy.Coalesced`), with `withSession` in both and a commit before a write returns while the Android process is stopped. Tests exist in both suites (`theDefaultIsCoalesced`, `coalescedCommitsOnceWithinTheWindow`, `whileStoppedACoalescedWriteCommitsBeforeItReturns`). Binding test T1 is in. Read, not run by me. |
| RB-001.5 | **`VERIFIED`** (v0.12.2) | `RecallOptions.track_access` defaults to `false` in the binding (`types.rs:212-218`). |
| RB-001.7 | **`VERIFIED`** (v0.12.2) | `ci.yml:45-47` cancels superseded runs; `:18-27` ignores docs-only changes; mobile jobs moved to `mobile-ci.yml` with path filters. |
| RB-001.17 | **`VERIFIED`** (v0.12.1) | The gate now needs seven engine jobs including `attach` and checks each result (`release.yml:412-463`). Tag runs 42919069975 (v0.12.1) and 42979921391 (v0.12.3) are green. The refusing path has still only been read, never seen. |
| RB-001.19 | **`VERIFIED`** (v0.12.1) | Swift's text methods are `nonisolated` and embed before entering the actor (`MnemoStore.swift:183-195, 228`); Kotlin embeds before taking the lock. `aSlowEmbedderDoesNotBlockAnUnrelatedGet` is in both suites. Read, not run by me. |
| RB-001.21 | **`VERIFIED`** (v0.12.2) | `MnemoKeystore.kt`: only a bad tag, a missing wrapping key or a permanently invalidated key is `KeyMissing` (`:160`); anything else is `Keystore("unwrap", …)` (`:117`); the key file is synced (`:76`); generation checks `containsAlias` (`:326`). `aTransientUnwrapFailureIsRetryableNotKeyMissing` is in the JVM suite. Read only. |
| RB-001.22 | **`VERIFIED`** (v0.12.2) | **Test:** with the flush failing once, `try_close()` returns the handle with the error, a retry succeeds, and the file has 40 of 40. Through the binding: `close()` errors, `flush()` then succeeds, 40 of 40. |
| RB-001.23 | **`VERIFIED`** (v0.12.2) | **Test:** on a file with a WAL to recover, `key_opens` answers `true` for the right key and `false` for a wrong one, in the engine and the binding, and leaves the file untouched. `Kdf`, `Crypto`, `NotAMnemoFile`, `NeedsWriteOpen` are distinct cases and `UnsupportedVersion` carries the engine's message. Item 3 (a poisoned handle) wasn't tested. |
| RB-001.24 | **`VERIFIED`** (v0.12.2) | **Test:** each of four forged `_mnemo.*` writes is `Invalid`, including an unknown `_mnemo.author`, and through `metadata_json` in the binding; nothing is stored. An ordinary update of a keyed memory still works. |
| RB-001.27 | **`VERIFIED`** (v0.12.2, removed in v0.13) | Neither store configuration has `formatVersion`. |
| RB-001.28 (part 1) | **`VERIFIED`** (v0.12.2) | A changed `keyPolicy` is refused with `keyPolicyMismatch` (`MnemoStore.swift:741`); test T10 is in. Read only. |
| RB-001.30, RB-002.2 | **`VERIFIED`** (v0.12.1, v0.12.2) | **Test, in a second scratch copy with crate internals:** after a plain rekey the old wrapped key is gone from the file and the old data key still validates the header (documented: plain rekey doesn't revoke); an `export_rekeyed` copy has a different data key and none of the live file's wrap; after rekey plus compact (the CLI's `--rotate`) the old data key no longer validates the header. The graft is now the repo's own acceptance test (`src/store/header_graft_tests.rs`). |
| RB-001.31 | **`VERIFIED`** (v0.12.1) | Top-level `contents: read` (`release.yml:50-51`); write only on the five jobs that touch the release; `persist-credentials: false` on every checkout; every action pinned to a commit; all four secrets families step-scoped; crates.io by Trusted Publishing; four environments with a required reviewer; the derived credential masked (`central-dry-run.sh:19`). |
| RB-002.3 | **`VERIFIED`** (v0.12.2) | The binding's `stats()` calls `stats_summary(&self)` (`db.rs:627-631`), which can't decrypt records. |
| RB-002.4 | **`VERIFIED`** (v0.12.2) | Tests T1, T4, T6, T8 are in `tests/engine_safety.rs` and the binding's tests; T10 and the embedder and flush-policy tests are in the Swift and Kotlin suites. |
| RB-002.5 | **`VERIFIED`** (v0.12.2) | Both mobile READMEs have the table with "Budget to stay resident" (`swift/README.md:119-121`). |
| RB-001.1 | stays `ACCEPTED` | The MOBILE-2 parts hold. **Test:** with 100 facts and 2,000 turn rows, a budget with room for 150 rows gives `ExactResident` and one with room for 50 gives `ExactPaged`, so turns no longer count. The over-budget cell is in `device_budget.rs`. The cheap paged path is still PERF-2's. |
| RB-001.20 | stays `ACCEPTED` | An expired first callback now throws `noBackgroundTime` (`Lifecycle.swift:152-171`). The device checks are still owed. |
| RB-001.25 | stays `ACCEPTED` | The docs state the cap (`README.md:246`, `mnemo/README.md:460`). The `stats` fields are in v0.13, not yet published. |
| RB-001.26 | stays `ACCEPTED` | The bound is 1 MiB. **Test:** a frame declaring 1 MiB + 1 byte, or 64 MiB, is refused as "too large". The fuzz targets are still owed before 14.1. |
| RB-001.29 | stays `ACCEPTED` until the tag | Fixed at `afd65b3`; see check 1. |
| RB-001.32 | stays `ACCEPTED` | Items 1, 2 and 4 are done (`Zeroizing` before the first fallible call; the binding keeps no secret; Swift zeroes a buffer it owns). **Item 3 is unchanged:** `generate_key` still returns `key.to_vec()` (`db.rs:858-861`), and that `Vec` goes to the FFI unwiped. It may not be fixable on the Rust side; if so, say that in the triage and close it. |

---

**New items**

#### RB-003.1 — [SECURITY] [MEDIUM] Held in `SECURITY.local.md`
- **Status:** `VERIFIED` (RB-005, v0.13.1)
- **Context / Evidence:** One finding about `mnemo inspect`, new in v0.13 and not yet released, written up in the gitignored `SECURITY.local.md` with what was tested and what held up. Not a hold for the tag; one part of its recommendation is two sentences of documentation worth taking before it.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). Before the tag: the documentation part and one small hardening; the rest in v0.13.1. Full triage in `SECURITY.local.md`.

#### RB-003.2 — [SECURITY] [LOW] Held in `SECURITY.local.md`
- **Status:** `VERIFIED` (RB-004, `d350d53`)
- **Context / Evidence:** One finding about `mnemo export`, new in v0.13 and not yet released, in `SECURITY.local.md` with a test. Not a hold for the tag.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), fixed before the tag. Full triage in `SECURITY.local.md`.

#### RB-003.3 — [CORRECTNESS] [LOW] A v8 file that crashed mid-commit gets two errors that point at each other; and a refused read-write open still writes to the file
- **Status:** `VERIFIED` (RB-005, v0.13.1)
- **Context / Evidence:** tested at `afd65b3` with the release CLI.
  - **The fixture:** a v8 passphrase file written by the published `mnemo-engine 0.12.3` (a scratch crate on crates.io's 0.12.3 with `failpoints`), 10 memories checkpointed and 5 committed in the WAL but not checkpointed. That's what a kill between the commit and the checkpoint leaves under v0.12.
  - **Every read command** (`about`, `list`, `export --git`, `verify`) fails with: "read-only open blocked: pending WAL recovery — open read-write once to fix".
  - **Every read-write command** (`index` here) fails with: "this file is format v8, which this build opens read-only; run `mnemo compact <file>` to convert it to v9".
  - **`mnemo compact` works:** "compacted … (v9): 15 -> 15 live memories", and all 15 are there, including the 5 from the WAL.
  - **The refused `index` changed the file** (212,992 → 229,376 bytes): it replayed the WAL into the v8 file and then refused. After it, `about` works and shows 15 memories.
  - **So did a read-write open with the wrong passphrase.** It returns `WrongPassphrase` and has already replayed the WAL. The order in `open_secret_with` is recovery (`mnemo/src/store/open.rs:416-419`), then the key (`:445-446`), then the format check. The same holds for v9 (test in this batch: "open rw(WRONG) → WrongPassphrase; file changed by it: true"). That part is older than v0.13.
- **Finding:**
  - Nothing is lost and the fix is one command, but the user is sent to "open read-write", which v0.13 no longer allows for this file, before the second message tells them about `compact`. v0.13 is the release where people meet v8 files for the last time.
  - "v8 opens read-only" isn't strictly what happens: a refused read-write open writes recovered pages into a v8 file. The write is the right data (a committed transaction), so this is harmless, but it's accidental.
  - Replaying before the key is checked gives no one a capability they lack (whoever can make the app open a file could also write to it, and pages are authenticated when read). It does mean a mistyped passphrase modifies the file.
- **Recommendation:**
  1. In the read-only open, when the file is older than v9, make the pending-WAL error say "run `mnemo compact <file>`" (it recovers and converts). One string and one test on a fixture like the one above; the generator for it is ten lines on top of `tests/fixtures/v8/generator`.
  2. Decide the order on purpose: check the key before replaying when the header is intact (the torn-header path already validates the key first, `open.rs:101-150`), and either refuse old formats before any write or document that a refused open may complete a committed transaction.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). **Recommendation 1 before the tag:** on a file older than v9, the read-only open's pending-WAL error says to run `mnemo compact <file>`, with the fixture test. **Recommendation 2 in v0.13.1:** check the key before replaying when the header is intact, and refuse old formats before any write; a test that a wrong passphrase and a refused v8 read-write open leave the file byte-identical, on v8 and v9.

#### RB-003.4 — [DX/DOCS] [LOW] The published crate doubles in size because the new test fixtures ship in it, and the tests it ships no longer compile from it
- **Status:** `VERIFIED` (RB-004, `d350d53`)
- **Context / Evidence:** `cargo package --locked --no-verify` on the pinned copy.
  - **Size:** `mnemo-engine-0.13.0.crate` is **1,205,799 bytes**; 0.12.3 on crates.io is 609,341. The difference is four `.mnemo` fixtures (745,472 bytes, encrypted, so they don't compress) plus `tests/fixtures/v8/v8_key.key`. The PyPI sdist packages the same directory.
  - **The packaged tests:** the self dev-dependency that turns `dev-hooks` on is stripped from the published `Cargo.toml` (correctly, or publishing would fail). `tests/fmt9_skeleton.rs`, `fmt9b_column.rs`, `fmt9c_commit.rs`, `fmt9d_bm25.rs` and `examples/gen_v8_zero_vector_fixture.rs` call the hooks, so `cargo test` from the published crate fails to compile unless `--features dev-hooks` is passed.
  - `mnemo/Cargo.toml` has no `include` or `exclude`.
- **Finding:** Neither matters to someone who depends on the crate. It matters to anyone who builds or tests from the registry tarball (distribution packagers, registry-wide test runs), and a raw key file in a published crate is the kind of thing secret scanners flag, harmless as this one is. A published version can't be amended.
- **Recommendation (pick one before the tag):**
  1. `exclude = ["tests/", "benches/", "examples/gen_*"]` in `mnemo/Cargo.toml`: the crate goes back to about 0.5 MB and ships no tests that can't build. Check that `cargo publish --dry-run` still passes and that the sdist follows.
  2. Or keep the tests and make them honest: `required-features = ["dev-hooks"]` on the four test targets and the example, and a line in the crate README saying how to run them.
  3. Either way, a short `tests/fixtures/README.md` saying the files are synthetic and the key and passphrases are throwaway.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), **option 1, before the tag:** `exclude` the test, bench and fixture-generator files from `mnemo-engine`'s package, plus a `tests/fixtures/README.md` (synthetic data, throwaway secrets). Check `cargo package --list` and `cargo publish --dry-run --locked`, and that the PyPI sdist no longer carries the fixtures either. Report the new `.crate` size.

#### RB-003.5 — [PLAN-REFINEMENT] [MEDIUM] The engine half of the release workflow never runs before a tag, so every change to it is first tried on the real one
- **Status:** `VERIFIED` (RB-005, v0.13.1)
- **Context / Evidence:**
  - `release.yml` at `afd65b3`: on `workflow_dispatch`, `create-release`, `build-cli` (5 legs), `build-wheels` (5 legs), `build-sdist`, `attach`, `publish-pypi` and `publish-crates` are skipped (`:60` and the jobs that need it). The dry runs I've read show those as `skipped` (42901448023 for v0.12.1: 7 jobs skipped, 5 run; the READY says the same of 43138424847).
  - This release is the first tag since `eb6c8e0` bumped nine actions by a major version or more. Check 3 lists what will run for the first time on the v0.13.0 tag.
  - The order of the engine jobs makes a failure there a split release: `publish-crates` needs only `build-cli` (`:356-358`), and `publish-pypi` needs the wheels and the sdist (`:308-310`). If a wheel leg fails, crates.io gets 0.13.0 and PyPI doesn't, and the mobile gate (rightly) stops the rest. A failure caused by the workflow file itself can't be fixed by re-running, because a re-run uses the file at the tag.
  - Related, and fixed: RB-002.1 was this same pattern in the mobile half (a step that first ran on the v0.12.0 tag and hung). REL-1 fixed it there by making the dry run exercise the step.
- **Finding:** The mobile half is now rehearsed end to end before every tag. The engine half, which publishes to the two registries with the most users, is rehearsed never. By the release notes this tag's action bumps are safe; the next change to those jobs gets the same first night.
- **Recommendation:**
  1. In the dry run, run the engine **build** jobs too, with their publish steps replaced: build the CLI and the wheels and the sdist, upload them as artifacts, download them in `attach` with the real `pattern`, and list `dist/` instead of calling `action-gh-release`. That covers every bumped action except the release action itself, in about five minutes.
  2. Make both publish jobs wait for all the builds (`needs: [build-cli, build-wheels, build-sdist, attach]`), so a build failure anywhere publishes nothing anywhere.
  3. Rehearse `action-gh-release` once on a throwaway prerelease tag in a fork, or accept it as read (check 3).
  4. This is a release-workflow change, so it wants its own dry run; I wouldn't squeeze it in before v0.13.0. Do it before v0.13.1 or the next Dependabot bump of that workflow, whichever comes first.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), as **REL-2**, before v0.13.1 or the next Dependabot bump of `release.yml`, whichever is first: recommendations 1 and 2 (the dry run builds the CLI, wheels and sdist and rehearses `attach`; both publish jobs need every build). 3 is accepted as read. **For the v0.13.0 tag itself:** the user approves `crates-io` and `pypi` only once every CLI, wheel and sdist job is green, which gives recommendation 2's protection by hand.

#### RB-003.6 — [DX/DOCS] [LOW] `--key-file` exists only on `inspect`, so a raw-key file can't be exported or converted from the CLI, and `FormatTooOld`'s advice doesn't work for one
- **Status:** `VERIFIED` (RB-005, v0.13.1)
- **Context / Evidence:**
  - `mnemo/src/bin/mnemo.rs:333-348`: `inspect` takes `--key-file`. `export`, `compact`, `list`, `about` and `recall` don't (checked from each command's `--help`).
  - `mnemo/src/error.rs` (`FormatTooOld`): "run `mnemo compact <file>` to convert it to v9". For `tests/fixtures/v8/v8_key.mnemo`, a raw-key v8 file, that command asks for a passphrase. The library call exists (`Mnemo::compact_file_with_key`), and the fixture tests use it.
- **Finding:** Raw-key files are what phones write and what `create_with_key` writes on the desktop. v0.13 adds two commands meant for exactly those files' owners (look at a phone's memory; export it readably). One of the two can open them. And the one file the v8 sunset's message can't help is a raw-key v8 file, which only the library can convert.
- **Recommendation:** give `export` and `compact` the same `--key-file` (the parsing is already in `inspect::read_key_file`), and have `FormatTooOld` name `compact_file_with_key` for raw-key files. A few lines each; v0.13.1 is fine.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12) → v0.13.1: `--key-file` on `export` and `compact` (and the other read commands where it's cheap), and `FormatTooOld`'s message names the raw-key route. The v0.13.0 README points raw-key users at `compact_file_with_key` until then.

#### RB-003.7 — [PERF] [LOW] The inspector clones every matching memory on each request before it pages, and holds the whole file in memory two or three times
- **Status:** `VERIFIED` (RB-005, v0.13.1, the fix only)
- **Context / Evidence:** by reading at `afd65b3`; not measured (no benchmarks this pass).
  - `mnemo/src/bin/mnemo/inspect.rs:625-639`: `/api/memories` builds `hits` by cloning each matching memory's JSON value, then applies `skip(offset).take(limit)`. The page asks for 200.
  - `:232-346`: one load keeps `db.memories()` (with vectors) until it has built a JSON value per memory, a `KeywordIndex` over every content, and each thread's turns as JSON.
  - `:570-584`: the state lock is held while a response is written to the client.
- **Finding:** On the files the inspector is for today (a phone's export, a project's brain: thousands of memories) this is invisible. On a 100K-memory file each page view copies 100K values to show 200, and the load needs a multiple of the plaintext in RAM. Nothing bounds it.
- **Recommendation:** count first and clone only the page; drop the `Vec<Memory>` as soon as the JSON is built; release the lock before writing the response. Add a cell to the PERF-2 measurements: load time and peak memory of `mnemo inspect` at 10K and 100K.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12) → v0.13.1: count, then clone only the page; drop the `Vec<Memory>` once the JSON is built; release the lock before writing the response. PERF-2 gains a cell: `mnemo inspect` load time and peak memory at 10K and 100K.

#### RB-003.8 — [DX/DOCS] [LOW] Small things in the v0.13 text and edges
- **Status:** `ACCEPTED`
- **Context / Evidence and recommendation, one line each:**
  1. **The CHANGELOG heading** is `## [0.13.0] — 2026-11-11` (`CHANGELOG.md:11`). Set it to the tag's date.
  2. **"One new memory adds only its own lines"** (`CHANGELOG.md:93`, `mnemo/README.md:894`, `export.rs:17-18`). Measured: an unkeyed memory adds 1 line and changes 0; a new version of a keyed fact adds 2 and changes 1. Say "a new version of a keyed fact also changes the line of the version it supersedes".
  3. **Python can't tell the new errors apart.** Every engine error becomes `RuntimeError` (`mnemo-python/src/lib.rs:43`), including `FormatTooOld` (which wants `compact`) and `EmbedderMismatch` (which wants the original embedder). At least document the message prefixes to match on; exception subclasses are the real fix.
  4. **The mobile-off note** (`release.yml:516-553`): re-running the job appends the line again, and a failed tag lookup fails the job after the engine has fully published. Make the lookup's failure print "latest: unknown", and skip the append when the body already has the line.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). **Before the tag:** 1 (the CHANGELOG date, set on the tag day) and 2 (the keyed-write wording; the root README already uses it); 3's documentation (the Python README lists the message prefixes to match on). **v0.13.1:** 3's exception subclasses. **With REL-2:** 4, the mobile-off note's two edges.

### Batch `RB-004` — 2026-11-12 (REVIEWER-B: the scoped v0.13.1 review before the tag: the open order, SEC-3 and REL-2's job graph, with five new items)

**What was reviewed.** `release-0.13.1` at `d350d53` (`release: v0.13.1`) against `v0.13.0` (`933177e`): 10 commits, 28 files, +1,363 / −254 lines. The scope is the three areas the WORKER READY names: step 1 (`mnemo/src/store/open.rs`), step 5 (SEC-3 in `inspect.rs` and `inspect.html`) and step 6 (REL-2's `needs` graph in `release.yml`). Step 4 (the inspector's paging) I read as part of the same file. Steps 2 and 3 (`--key-file`, the Python exception types) were not reviewed beyond the baseline run. Nothing is tagged.

**How.**
- **Pinned.** Every build and test ran on a scratch copy made with `git archive d350d53`, with `CARGO_TARGET_DIR=/tmp/rb-target` and `--locked`. Scratch crates depend on that copy by path.
- **No benchmarks, nothing timed.** The seconds quoted below are test timeouts, not measurements of the engine.
- **Machine:** Apple M1 Pro, macOS 26.7.1, Rust 1.95.0.
- **Baseline at `d350d53`, green:** `cargo test --locked --features failpoints --no-fail-fast`: **375 passed, 0 failed**. The `mnemo-uniffi`, Swift and Kotlin suites were not run here: none of their sources changed, only the version numbers.
- **GitHub, read-only:** the five environments, the variables, the rulesets and the three release dry runs were read through the API.
- **One browser check** used Chrome 157 headless with a new profile in my scratch folder, never the user's own.
- **Read-only.** I wrote this file and the gitignored `SECURITY.local.md`, nothing else.
- **The unfixed security finding** from this pass (`RB-004.1`) is only in `SECURITY.local.md`, with the SEC-3 probe results.

---

**The verdict first.**

- **I'd hold the tag for one thing, and it's wording: `RB-004.1`** (in `SECURITY.local.md`). One sentence in the crate README and the CHANGELOG states something my test contradicts, and a published README can't be corrected for that version. No code changes. If re-cutting the release commit isn't worth it, shipping as it is harms no one's data.
- **If that commit is made, take `RB-004.3`'s one line with it** (a `needs` entry in `release.yml`). On its own I wouldn't hold for it.
- **Nothing else is a hold.** `RB-004.2` is the most serious item here, but it's older than this release and v0.13.1 doesn't change it.
- **Two things for the person who pushes the tag and approves it:**
  1. `ENABLE_MOBILE_PUBLISH` is `true` as I write (read from the API). The READY says to set it to `false` before the tag. The approval prompt doesn't show which mode the run is in (`RB-004.4`), so check the run's summary for "Mobile packages skipped" before approving.
  2. This tag is the first real run of `approve the release`, of the `release` environment, of every job after it in the new order, and of the mobile-off note's append. The dry runs can't reach those. If the approval doesn't appear, nothing publishes, which is the safe failure.

---

**Area 1: the key and the format are checked before the WAL is replayed (`open.rs`). It's right.**

- **The order, as read** (`mnemo/src/store/open.rs:437-482`): read the pending frames; take the header the replay would leave (the WAL's page-0 frame, else page 0); derive the key and unwrap against it; refuse a format older than v9; only then write the frames. The torn-header path refuses an old format before healing (`:410-418`); its scan had already checked the key.
- **Nothing else opens for writing.** `open_with_lock(.., write=true, ..)` has two callers, `create_secret` and `open_secret_with` (`:223`, `:389`). Compaction goes through the second (`maintenance.rs:619`). `key_opens` and the read-only open never write.
- **Cost:** the same single key derivation as before, moved earlier (by reading; not timed).
- **The three tests, checked by putting `v0.13.0`'s `open.rs` back** into a second scratch copy:
  - `a_wrong_key_leaves_a_pending_wal_untouched`: **fails** there ("a wrong key replayed the WAL"), passes at `d350d53`;
  - `a_refused_open_of_a_v8_pending_wal_leaves_it_byte_identical`: **fails** there ("a wrong passphrase wrote"), passes at `d350d53`;
  - `a_cut_off_rekey_opens_with_one_passphrase_and_a_wrong_one_writes_nothing`: **passes on both.** It guards a property the old order already had (see `RB-004.5` item 1).
- **Cases the three tests don't cover, which I ran** (scratch crate `rb4`, all on copies):

| Case | Result at `d350d53` |
|---|---|
| v8 file, sector 0 zeroed, read-write, wrong passphrase | `BadMagic`, byte-identical |
| v8 file, sector 0 zeroed, read-write, right passphrase | `FormatTooOld { version: 8 }`, byte-identical |
| The same two on the v8 pending-WAL fixture | the same two results |
| `compact_file`, wrong passphrase, on a torn v8 header and on the v8 pending-WAL fixture | `BadMagic` and `WrongPassphrase`, byte-identical, no temp file left |
| `compact_file`, right passphrase, on the two torn v8 files | heals and converts: 17 → 17 and 15 → 15, then opens read-write |
| v9 pending WAL with sector 0 zeroed, wrong key | `BadMagic`, byte-identical; `key_opens` agrees |
| The same, right key | opens with all 15 |
| A raw-key rekey cut off after each of 60 write counts, then a wrong key | `WrongPassphrase`, byte-identical, at all 60 |
| The same 60, `key_opens` against the open | they agree for both keys at every point |

- **Two findings came out of these runs.** Neither is about the new order: `RB-004.2` (a rekey that returns an error) and `RB-004.5` items 1 and 2.

**Area 2: SEC-3 (`inspect.rs`, `inspect.html`). Three of its four parts hold as described; the fourth does less than the docs say.**

- The forwarding page for `--open`, the idle timeout, the duplicate-`Host` refusal and the token check on every API route all passed live probes. The probe table is in `SECURITY.local.md`.
- One documented claim is contradicted by a test in a real browser: `RB-004.1`.
- Step 4's changes are as the DONE says, by reading: the matches are counted, only the page is cloned, the records are dropped before the turns are read, and the lock is released before the response is written. One probe: `limit=999999` returns the fixture's 7 memories without error (the cap is 1,000).

**Area 3: REL-2's `needs` graph (`release.yml`). Nothing can publish without `approve-release`, as long as the workflow file is the one at `d350d53`.**

- **By reading, job by job.** Seven jobs have a step that changes something outside the run: `create-release`, `attach`, `publish-pypi`, `publish-crates`, `mobile-skipped-note`, `publish-swift` and `publish-android`. On a tag each one's `if` requires `approve-release` to have succeeded, directly or through `create-release`. In a manual run each real step is behind `dry_run == 'false'` or `github.event_name == 'push'`, and `mobile-prepare` fails a manual run that isn't a dry run (`:555-557`).
- **By simulation.** I translated every `needs` and `if` by hand into a script and ran all 65,536 combinations of event, inputs, variables and job outcomes (no cancellations). None broke these: a real publish implies a tag, an approved `approve-release` and every build green (mobile builds too when mobile is on); a mobile publish implies the engine gate passed; a manual run never publishes and never reaches the approval; a failed build on a tag publishes nothing. The script is a translation, so it checks my reading, not GitHub's evaluator.
- **The settings match RELEASING.md** (read through the API):
  - `release`: required reviewer `toarchkumar`, self-review allowed, deployments from tags `v*` only, no secrets.
  - `crates-io`, `pypi`, `maven-central`, `mnemo-swift`: no reviewer; tags `v*` and branches `release-*`. `maven-central` holds four secrets and `mnemo-swift` one. `crates-io` and `pypi` hold none (Trusted Publishing; `CRATES_TRUSTED_PUBLISHING` is `true`).
  - No repository secrets, no rulesets, no branch protection, one collaborator.
- **The dry runs' graphs are what the file says** (43275719483 mobile on at `a5c9f72`; 43279923650 mobile off at `d350d53`): `approve the release`, `Create GitHub Release` and the engine gate skipped; attach, crates.io and PyPI ran their rehearsal steps; no run waited. 43279923650 created deployments on `crates-io` and `pypi` from `release-0.13.1` with no review, as designed.
- **What the approval now rests on.** Before REL-2, GitHub itself stopped any run from using a registry environment until a person clicked. Now GitHub enforces that only for `approve-release`, a job with no secrets. That the publishes wait for it is written in the workflow file of whatever ref is run. RELEASING.md states this trade-off, and with one collaborator and no rules it's a guard against accidents, which is its purpose. I'm recording it so that a later change (a second maintainer, a bot with push access) reopens the question.
- **Two findings:** `RB-004.3` (a race REL-2 introduced) and `RB-004.4` (the approval doesn't show what is being approved).

---

**RB-003 items after this pass**

| Item | What I checked | Result |
|---|---|---|
| RB-003.2 | `mnemo export -o` onto an existing 0644 file, with the CLI built at `d350d53` (`export.rs` is unchanged since `v0.13.0`) | The result is 0600 and a new inode: an old hard link still holds the old text. No temp file left. **`VERIFIED`** |
| RB-003.4 | The published `mnemo-engine 0.13.0` | 349,137 bytes (0.12.3: 609,341; as staged at `afd65b3`: 1,205,799). `cargo package --list` at `d350d53`: 57 files, none under `tests/`. **`VERIFIED`** |
| RB-003.3 | Recommendation 2, area 1 above | Fix confirmed at `d350d53`. Mark it when v0.13.1 is published |
| RB-003.5 | The dry run builds the engine artifacts and rehearses both publishes; `publish-crates` now waits for every build | Fix confirmed at `d350d53` and in runs 43275719483 and 43279923650. Mark it at the tag |
| RB-003.7 | Step 4, by reading and one probe | Fix confirmed at `d350d53`; not measured. Mark it at the tag |
| RB-003.8 item 4 | The note's "unknown" fallback and its already-present check (`release.yml:603-638`) | Right by reading. The append has still never run on a tag, and REL-2 changed when it runs: `RB-004.3` |
| RB-003.1 | SEC-3 | Three parts confirmed, one overstated in the docs: `RB-004.1`. Its details stay in `SECURITY.local.md` |
| RB-003.6 | Not in this pass's scope | Its tests (`key_file_cli`) pass in the baseline. Not reviewed |

---

**New items**

#### RB-004.1 — [SECURITY] [LOW] Held in `SECURITY.local.md`
- **Status:** `ACCEPTED`
- **Context / Evidence:** One finding about SEC-3's part of `mnemo inspect`, not yet released, written up in the gitignored `SECURITY.local.md` with the test and the probe table. One sentence in the crate README and the CHANGELOG says more than the change does. The fix before the tag is wording only.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), before the tag: the wording only. Full triage in `SECURITY.local.md`.

#### RB-004.2 — [CORRECTNESS] [MEDIUM] A rekey that returns an error can still change the secret: the handle keeps the new key wrap, and the next flush commits it
- **Status:** `ACCEPTED`
- **Context / Evidence:** tested at `d350d53` (scratch test `rb4/tests/rekey_err.rs`, `failpoints`). **Not new in v0.13.1:** `rekey_secret` was last changed in `280a54b` (SEC-2, v0.12.1). I didn't test older releases.
  - **The code** (`mnemo/src/store/maintenance.rs:137-198`): the new salt, wrap, KDF costs and WAL position are written into `self.header` and `self.kdf` (`:161-180`) before `relocate_header_through_wal` commits them (`:181`). Nothing restores them when that call fails. And once the new page 0 is on disk, three more writes can still fail the call: the old region's invalidation inside `relocate_header_through_wal` (`flush.rs:116-118`) and the two scrub calls (`:188-189`).
  - **The test.** Create a file with key A, flush, make the n-th write fail, call `rekey_with_key(B)`, then carry on with the same handle:

| The failing write | `rekey` returns | Then drop, or close with nothing new | Then one `remember` and a flush or close |
|---|---|---|---|
| 1st or 2nd (the WAL commit, or page 0 itself) | `Err` | A opens, B doesn't | **B opens, A doesn't** |
| 3rd or 4th (clearing the old WAL region, after page 0 is written) | `Err` | **B opens, A doesn't** | B opens, A doesn't |
| none | `Ok` | B opens | B opens |

  - **Who calls it.** The CLI's `mnemo rekey` and `rekey --rotate`; Python's `rekey` and `rekey_with_key` (`mnemo-python/src/lib.rs:1633-1661`), where the handle stays usable after the exception; any Rust caller. The Swift and Kotlin stores don't: they use `export_rekeyed`, which writes a copy.
- **Finding:**
  - A caller can't tell from the error which secret the file is under, and in the first row the answer changes later, at the next flush.
  - The realistic trigger is a full disk: a rekey appends a new WAL region to the file, so its first write is the one that fails. A long-running Python agent that catches the exception, logs "rekey failed, keeping the old passphrase" and keeps writing will find at its next start that the old passphrase is refused.
  - The data isn't destroyed: the new secret opens the file, and whoever called `rekey` supplied it. It's lost only if the caller generated a random key and threw it away on the error.
- **Recommendation:**
  1. In `rekey_secret`, keep a copy of the header fields and `self.kdf` it's about to change, and put them back when the failure comes before the new page 0 is written. An `Err` then means "still the old secret", including after later writes.
  2. From that write on, the rekey has taken effect. Don't return a plain error for clearing the old region: finish what can be finished and report it another way (it's hygiene, as the doc comment says), or return a distinct error that says the new secret is in force.
  3. A failpoint test over every cut point: after `Err`, the old secret opens the file even after another write and a close; after `Ok`, the new one does.
  4. Until then, one sentence in `rekey`'s documentation (Rust and Python): after an error, close the handle without writing, and try both secrets.
- **Would I hold the tag for it?** No. It's in every release since at least v0.12.1, it needs an I/O failure at one moment, and v0.13.1 neither adds nor changes it. A good first item for v0.13.2.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). **Before the tag:** recommendation 4, one sentence in `rekey`'s Rust and Python docs (after an error, close the handle without writing, and try both secrets). **v0.13.2, first item, as REKEY-1:** recommendations 1 to 3 (restore the header fields on a failure before page 0 is written; after it, report the old-region cleanup without a plain error; a failpoint test over every cut point).

#### RB-004.3 — [CORRECTNESS] [LOW] REL-2 made the mobile-off note run at the same moment as the asset attach, and both rewrite the Release's body: the note can be lost with every job green
- **Status:** `VERIFIED` (RB-005, v0.13.1)
- **Context / Evidence:** by reading at `d350d53`, with the dry run's step times; not reproduced (it needs a real Release).
  - **The graph changed.** At `v0.13.0`, `mobile-skipped-note` needed `mobile-prepare`, which needed `attach`, so the note ran after the assets were attached. At `d350d53` it needs `[mobile-prepare, create-release]` (`release.yml:585`) and `attach` needs `create-release` too (`:304`), so both start when the Release is created.
  - **Both write the body.** On an existing release, `softprops/action-gh-release` v3.0.4 reads the release and then updates it with a `body`: the text it read, or that text plus the note when `append_body` is set (`src/github.ts:664-716`, the existing-release branch). `attach` passes only `files`, so it writes back the body it read.
  - **They'd have called the action in the same second.** In dry run 43279923650 the note job's lookup step and `attach`'s listing step both ran at 18:37:48, and on a tag the action is each job's next step.
- **Finding:** if `attach` reads the body, the note job appends its line, and `attach` then writes what it read, the line is gone. No job fails, and the note's own "already there?" check only helps on a re-run. The note is REL-1's whole deliverable for a release like this one, and v0.13.1 is its first real run. The consequence is a missing sentence on the Release page, nothing more.
- **Recommendation:** add `attach` to the note job's `needs` (`needs: [mobile-prepare, create-release, attach]`); its `if` already runs with `!cancelled()`. One line. If it isn't taken before the tag, look at the Release page afterwards and paste the line if it's missing.
- **Would I hold the tag for it?** No. Take it if a commit is being made anyway.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), before the tag: `mobile-skipped-note` needs `attach` too. Covered by the release-commit dry run, and checked on the Release page after the tag.

#### RB-004.4 — [PLAN-REFINEMENT] [LOW] The one approval doesn't show what is being approved, and the mobile mode is a repository variable flipped by hand around each tag
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - `release.yml:64-87`: the job is named `approve the release`. The version and the mode are printed by its only step, which runs after the approval.
  - `:566-570`: on a tag the mode is `vars.ENABLE_MOBILE_PUBLISH`, read when `version check` starts. It's `true` now, and the READY's remaining steps are "set it to `false` right before the tag, and back to `true` once the Release is out".
  - `version check`'s outputs are kept by "Re-run failed jobs" and recomputed by "Re-run all jobs".
- **Finding:**
  - Forgetting the first flip builds and, after one click, publishes `mnemo-swift` and `mnemo-android` 0.13.1, which the CHANGELOG and both install lines say don't exist. Maven Central doesn't take a version back. The approver's only cue is whether the graph shows the Swift and Android build jobs.
  - After the flip back, "Re-run all jobs" on the same tag run would turn mobile on for it.
  - The mode of a release is a fact about that release. It's in the release commit already, in words (the CHANGELOG summary, the install lines), but the workflow reads it from a setting that isn't versioned or reviewed.
- **Recommendation:**
  1. Put the facts in the job's name, which GitHub shows on the pending approval: `name: approve the release (v${{ needs.mobile-prepare.outputs.version }}, mobile ${{ needs.mobile-prepare.outputs.mobile }})`. A job name can read the `needs` context.
  2. Read the mode from the repository at the tagged commit (one line in a small file the release commit sets, checked by `version check`), and keep the variable only as an off switch. The mode is then in the PR, and a re-run can't change it.
  3. RELEASING.md: say to flip the variable back only once the tag run has finished, and not to use "Re-run all jobs" on a tag run after that.
- **Would I hold the tag for it?** No. For this tag, the check is one look at the run summary before approving.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). **Before the tag:** recommendation 1 (the job name shows the version and the mobile mode) and 3 (RELEASING.md: flip the variable back only once the tag run has finished, and never use "Re-run all jobs" on a tag run after that). **v0.13.2:** recommendation 2 (the mode is read from the tagged commit, and the variable is only an off switch).

#### RB-004.5 — [DX/DOCS] [LOW] Small things in the v0.13.1 changes
- **Status:** `ACCEPTED`
- **Context / Evidence and recommendation, one item each:**
  1. **A comment in `open.rs` describes a state a rekey can't leave** (`mnemo/src/store/open.rs:444-450`: "after a crash mid-rekey, page 0 still wraps the data key under the old secret and the WAL under the new one"). At none of 60 cut points did a read-only open report a pending WAL, and the rekey test's own comment says why. So the branch that takes the key from the WAL's page-0 frame is never reached with a wrap that differs from page 0's, by any test or by any write the engine makes. Keep the code (it's the right defence); correct the comment, and say in the rekey test that it also passes on the old order.
  2. **A v8 file with a torn header still gets the two-step advice** that RB-003.3 removed for the pending-WAL case. Read-only (so `about`, `list`, `export`): "torn header page needs recovery from WAL" (`open.rs:830`), which sends the user to a read-write open, which answers `FormatTooOld`. `mnemo compact` does fix it (tested: 17 → 17 and 15 → 15). The scan already has the header frame, so the read-only error can name `compact` for a pre-v9 file. One string and one test.
  3. **`--idle-timeout` multiplies without a check** (`inspect.rs:453-454`). `1152921504606846976h` is accepted and the server starts (tested). By arithmetic the product wraps to 0 in a release build, which means never. Use `checked_mul` and refuse.
  4. **After an idle stop the page says "Failed to fetch"** (the browser's text, through `fail()` at `inspect.html:137`). Say that the inspector has stopped and how to start it again.
  5. **`--open` on Linux with a sandboxed browser: not tested, worth one check.** The forwarding page is under `std::env::temp_dir()`, which is `/tmp` there. A browser installed as a snap or a flatpak (Ubuntu's default Firefox is a snap) gets its own private `/tmp`, so it may show "file not found" where v0.13.0's `xdg-open <url>` worked. This is from how those sandboxes work, not from a run. If it's so, say it in the `--open` help text; the printed URL still works.
  6. **The root README** (Planner-owned; the Worker flagged the first two): `--open` "only on a machine where you're the only user" (`README.md:126-127`) and `--key-file` "planned for v0.13.1" (`:138`) are out of date, and "behind a one-time link" (`:120`) was never accurate: the link works until the inspector stops.
  7. **The red Mobile CI run on `d350d53`** (43279893187, attempt 1) is `t9aAnEmbedderThatCallsCacheGetReturnsAtOnce`, at `StoreHardeningTests.swift:38`: `#expect(Date().timeIntervalSince(start) < 2)` took 2.514 s. The line after it, `embedder.timedOut == false`, passed, so the behaviour held and only the wall-clock bound was missed. No Swift source changed in v0.13.1. The re-run (attempt 2) passed. For FLAKE-3: `timedOut` is the real signal (the embedder gives up at 3 s), so drop the 2 s bound or raise it to that.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). **Before the tag:** item 1 (the `open.rs` comment, plus the rekey test's comment), item 3 (`checked_mul`, refusing an overflow), item 7 (FLAKE-3, as the Worker proposed). **Item 6:** the root README handoff fixes it. **v0.13.2:** item 2 (the torn-header read-only message names `compact` on a pre-v9 file), item 4 (the page's text after an idle stop), and item 5 (check `--open` with a snap or flatpak browser on Linux, and document it if it fails).

### Batch `RB-005` — 2026-11-12 (REVIEWER-B: the scoped v0.13.2 review before the tag: REKEY-1, SEC-4 and the mobile mode from the tagged commit, a verification sweep of what v0.13.1 shipped, and four new items)

**What was reviewed.** `release-0.13.2` at `e7fbb59` (`release: v0.13.2`) against `v0.13.1` (`15bc00f`): 6 commits, 26 files, +721 / −117 lines. The scope is the three areas the WORKER READY names: step 1 (REKEY-1 in `mnemo/src/store/maintenance.rs`, with `tests/rekey_cut.rs`), step 2 (SEC-4 in `inspect.rs` and `inspect.html`) and step 3 (the mobile mode in `release.yml`, `.github/release-mobile` and RELEASING.md). Step 4 I tested in passing (the torn-header advice) or read (the stopped page, the `--open` note). Nothing is tagged.

**How.**
- **Pinned.** Every build and test ran on a scratch copy made with `git archive e7fbb59`, with `CARGO_TARGET_DIR=/tmp/rb-target` and `--locked`. Scratch crates depend on that copy by path.
- **A second scratch copy has two added test hooks** (41 lines in `pager.rs` and `lib.rs`, nothing else): a scripted set of failing writes, and a header write that reaches the file and then reports failure. The repo's failpoint fires once, so it can't make two writes in a row fail. The code under review is unchanged in that copy.
- **No benchmarks, nothing timed.**
- **Machine:** Apple M1 Pro, macOS 26.7.1, Rust 1.95.0.
- **Baseline at `e7fbb59`, green:** `cargo test --locked --features failpoints --no-fail-fast`: **380 passed, 0 failed** (375 at `d350d53`, plus the five new tests). The `mnemo-uniffi`, Swift, Kotlin and Python suites were not run here.
- **GitHub, read-only:** the variables, the five environments, the v0.13.1 tag run, the dry run and the push runs on `e7fbb59` were read through the API. The published `mnemo-engine 0.13.1` crate was fetched into an empty scratch folder and compared with the tag; nothing in it was run.
- **One browser check** used Chrome 157 headless with new profiles in my scratch folder, never the user's own.
- **Read-only.** I wrote this file and the gitignored `SECURITY.local.md`, nothing else.
- **The unfixed security finding** from this pass (`RB-005.1`) is only in `SECURITY.local.md`, with the SEC-4 probe table.

---

**The verdict first.**

- **I wouldn't hold the tag for anything here.** The three areas do what the READY says, and each of the four new items is LOW.
- **REKEY-1 holds.** In every case I could construct, a plain `Err` left the old secret opening the file, straight away and after a later write and a close. `Ok` always left the new one. Exactly one secret opened the file in all of them.
- **SEC-4 holds.** The code worked once in every probe, including 200 connections racing for it, and in Chrome.
- **The mobile mode can't be turned on by anything but the tagged commit.** The variable can only turn it off.
- **For the person who pushes the tag and approves it:**
  1. `ENABLE_MOBILE_PUBLISH` is `true` (read from the API). From this release that means "follow the commit", and the commit says `false`, so there's nothing to flip.
  2. The run pauses once. The pending approval should read **"approve the release (v0.13.2, mobile false)"**. If it says anything else, don't approve.
  3. This tag is the first real run of the new mode lines on a tag. The dry run exercised the file check, not the tag branch. If they fail, `version check` fails and nothing builds or publishes.

---

**Area 1: REKEY-1 (`rekey_secret`, `undo_rekey`, `rekey_cleanup_error`). It's right, with one gap that needs two failures and then a damaged header (`RB-005.2`).**

- **As read** (`mnemo/src/store/maintenance.rs:149-296`):
  - The header and the KDF are copied (`:176-177`) before anything changes. The three steps that can fail before the new secret is in force (the seal, the WAL commit at `:209`, the page-0 write and its sync at `:213-216`) each go through `undo_rekey`, which puts both back first (`:261-262`).
  - **Where the new secret takes effect:** when page 0's write and its sync have both succeeded (`:213-220`). A write that lands with a failed sync is undone by writing the old page 0 back (`:266-275`).
  - **After that point nothing returns `Err`.** The three cleanup steps each run whatever the one before did (`:230-239`), and what failed is kept for `rekey_cleanup_error` (`:240`, `:292`).
  - **The double failure:** if the write-back fails too, the error says the file may open with either secret (`:278-281`).
- **`tests/rekey_cut.rs` does what its header says**, on a clean handle, with the one-shot failpoint. Its limits are in `RB-005.4` item 5. I did not repeat the Worker's check with the restore removed; my RB-004 test showed the old behaviour at `d350d53`.
- **Scripted failures** (scratch crate `rb5s`, a 3-memory passphrase file; each row run three ways: drop, close, or one more write and a close). "Old" and "new" say which secret then opens the file; the other one got `WrongPassphrase` every time.

| What fails | `rekey` returns | After a drop or a close | After one more write and a close | A copy of the file with page 0 damaged |
|---|---|---|---|---|
| the WAL commit | the I/O error | old | old | old recovers it, new doesn't |
| the WAL commit, then zeroing the new region | the I/O error | old | old | the same |
| page 0 (not written; or written, sync failed) | the I/O error | old | old | the same |
| page 0, then zeroing the new region | the I/O error, nothing more | old | old | **both recover it**, until the next write (`RB-005.2`) |
| page 0 not written, then the write-back | "may open with either secret" | old | old | old only |
| page 0 written, then the write-back | "may open with either secret" | **new** | old | old only |
| page 0 written, zeroing and the write-back | "may open with either secret" | **new** | old | both, until the next write |
| after page 0: the old region's first frame, its zeroing, or both | `Ok`, with a cleanup report naming each | new | new | new only |

  - The two "new" cells are the documented exception, and the error text matches what the file does.
  - In the last row the old secret recovers nothing even when both region steps fail, because the third step (overwriting other copies of the old wrap, which has no failpoint) had already broken the old frame.
- **A handle with unflushed writes** (scratch crate `rb5`, the repo's own failpoint, 40 cut points, three ways each). The Worker's test rekeys a clean handle, so the flush inside the rekey writes nothing there. With one unflushed memory: writes 0 to 10 (the flush's own, then the commit and page 0) give `Err`, 11 and 12 give `Ok` with cleanup left, the rest `Ok`. In all 120 runs the right secret opened the file with the right count, the other was refused, and a damaged-header copy didn't recover with the other.
- **A real I/O error, no failpoint.** `RLIMIT_FSIZE` makes the write of the new WAL region fail with `EFBIG` after `k` bytes of it:

| `k` (bytes of the new region that reach the file) | `rekey` | Old secret | After raising the limit, one write and a close |
|---|---|---|---|
| 0, 1, 28, 29, 4096, 8191, 8192, 8249 | `Err`: "File too large (os error 27)" | opens, 3 memories | old opens with 4; new refused; a damaged-header copy doesn't recover with the new |
| 8250 (the whole commit) and 20000 | `Ok` | refused | new opens with 4 |

- **The CLI under the same limit** (release build of `e7fbb59`): `mnemo rekey` and `rekey --rotate` exit 1 with "error: io error: File too large (os error 27)". The old passphrase still opens the file and the new one doesn't. (`RB-005.4` item 4: the message doesn't say which passphrase is in force.)
- **Who reports the cleanup.** The CLI prints a warning (not with `--rotate`, which rewrites the file anyway). Python emits a `RuntimeWarning`: `RB-005.3`. The Swift and Kotlin stores don't call the in-place rekey.

**Area 2: SEC-4 (`/api/session`, `session()`). The code can't be used twice.**

- **As read** (`inspect.rs:692-714`): the code sits in a mutex. One request takes the lock, compares in equal time, and clears the slot before the lock is released, so two requests can't both pass. The `Host` and method checks come first. Every response carries `Cache-Control: no-store`, so a browser can't replay the exchange from its cache.
- **Live probes** (release CLI of `e7fbb59`, a copy of the iOS fixture): wrong codes, another `Host`, `POST` and `HEAD` are refused without using the code up; the first right request gets the token; the next 101 get 403; with 64 and then 200 connections released together, exactly one got a token each time. The table is in `SECURITY.local.md`, with RB-004.1's details.
- **In Chrome 157** (headless, new profiles): the first load shows the fixture's 7 memories; the same link again, in that profile and in another, shows "This link has already been used"; the profile's history holds the link with its spent code, and no token.
- **By reading only:** the prerender wait (`inspect.html:109-111`). Firefox and Safari weren't tested, and `--open` ran through a stand-in launcher, not a real browser (the Worker checked it in the browser pane).
- **The docs match:** the engine README and the CHANGELOG say the link works once, that a refresh keeps working and that another tab needs a new link. All three are what I saw.

**Area 3: the mobile mode from the tagged commit (`release.yml:563-596`, `.github/release-mobile`, RELEASING.md). Nothing but the commit can turn mobile on.**

- **The job graph is the one I simulated in RB-004, plus that batch's two fixes.** Parsed with a YAML reader and compared job by job: from `d350d53` to `e7fbb59` the differences are `approve-release`'s name, `attach` in `mobile-skipped-note`'s `needs`, the `mobile` input's description, and the script of `version check`. No `needs`, `if`, `environment` or `permissions` changed in v0.13.2.
- **The mode lines, run in real bash** as GitHub runs them (`bash -eo pipefail`), with the `${{ }}` expressions substituted as text:

| On a tag | Result |
|---|---|
| file `true`, `false` (with or without a newline, spaces or CRLF) | on, off |
| file `TRUE`, `True`, `yes`, `1`, `true` then `false`, `false # off`, a BOM, empty, missing | `version check` fails; nothing builds |
| file `false`, any value of the variable | off |
| file `true`, variable `true` or unset | on |
| file `true`, variable `false` | off, and the log says the switch did it |
| file `true`, variable `False`, `FALSE`, ` false`, `0`, `off`, `no` | **on** (`RB-005.4` item 1) |
| shell text in the file | not run (the value is quoted); `version check` fails |
| shell text in the variable | `RB-005.1` |

  - A dry run follows its `mobile` input and still refuses a bad file. It doesn't say when the input and the file differ (`RB-005.4` item 2).
- **`version check` checks out the tagged commit** (the default ref, the whole tree) before it reads the file, has `contents: read` and no environment or secrets.
- **Settings, read through the API:** unchanged since RB-004. `release` has the one reviewer and takes `v*` tags only; the four registry environments have no reviewer. `ENABLE_MOBILE_PUBLISH` is `true`.
- **Runs on `e7fbb59`:** CI 43321915091 and Mobile CI 43321915393 are green on attempt 1 (60 check runs: 52 passed, 8 skipped). The dry run 43321935230 is green with no pause: `version check` printed the commit's mode, the mobile jobs were skipped, attach and both publishes ran their rehearsal steps, and the note started after attach finished.
- **RELEASING.md** matches the workflow, with two sentences that say slightly more than it does (`RB-005.4` items 2 and 3).

---

**Earlier items after this pass**

Shipped in v0.13.1, so marked now. The published `mnemo-engine 0.13.1` crate (353,564 bytes; its SHA-256 matches crates.io's) records commit `15bc00f`, the tag's, and each of its 59 files is byte-identical to the tag's copy apart from the two Cargo rewrites (`Cargo.toml`, `Cargo.lock`). PyPI has 0.13.1 with 6 files.

| Item | What I checked | Result |
|---|---|---|
| RB-003.1 | SEC-3 as shipped: the code I probed at `d350d53`, plus the step 8 changes (read). Re-probed on the `e7fbb59` build: the idle stop ignores requests without a secret, two `Host` headers get 400, `--open` passes only a 0600 page in a 0700 folder | **`VERIFIED`** (v0.13.1) |
| RB-003.3 | Rec. 1 shipped in v0.13.0, rec. 2 in v0.13.1; `open.rs` in the published crate is the tag's; its three tests pass at `e7fbb59` | **`VERIFIED`** (v0.13.1) |
| RB-003.5 | The v0.13.1 tag run 43305366410: the last of the 11 builds finished at 21:02:37, the one approval was at 21:06:36, and all three publishes started after it. The dry run on `e7fbb59` built and rehearsed the engine half | **`VERIFIED`** (v0.13.1) |
| RB-003.6 | CLI built at `e7fbb59` (these commands are unchanged since v0.13.1): `export --key-file` and `compact --key-file` work on the v8 raw-key fixture; `FormatTooOld` names `mnemo compact` with `--key-file` in the published crate | **`VERIFIED`** (v0.13.1) |
| RB-003.7 | The paging fix as read at `d350d53`, shipped unchanged. Not measured; the PERF-2 cell in the triage is still to come | **`VERIFIED`** (v0.13.1), the fix only |
| RB-004.3 | The tag run: `attach` finished at 21:07:07 and the note job started at 21:07:10. The Release has 11 assets and the "Mobile packages not published" line | **`VERIFIED`** (v0.13.1) |
| RB-004.1 | Rec. 1 (the wording) is in the published crate README. Rec. 2 is SEC-4: confirmed at `e7fbb59`, area 2 | Stays `ACCEPTED` until v0.13.2 ships |
| RB-004.2 | Rec. 4 (the doc sentence) shipped in v0.13.1. Recs. 1 to 3 are REKEY-1: confirmed at `e7fbb59`, area 1 | Stays `ACCEPTED` until v0.13.2 ships |
| RB-004.4 | Recs. 1 and 3 shipped: the tag run's approval was named "approve the release (v0.13.1, mobile false)". Rec. 2: confirmed at `e7fbb59`, area 3 | Stays `ACCEPTED` until v0.13.2 ships |
| RB-004.5 | Items 1 and 3 shipped (read; the overflowing `--idle-timeout` is now refused). Item 7 shipped (FLAKE-3; Mobile CI is green on `e7fbb59`, not otherwise re-checked). At `e7fbb59`: item 2 tested (a damaged v8 header names `compact`, writes nothing, and `compact --key-file` heals it to v9 with all 3 memories); item 4 read; item 5 is documented, and I didn't repeat the Worker's Linux probe | Stays `ACCEPTED` until v0.13.2 ships |
| RB-003.8 | Not in this pass | Unchanged |

---

**New items**

#### RB-005.1 — [SECURITY] [LOW] Held in `SECURITY.local.md`
- **Status:** `ACCEPTED`
- **Context / Evidence:** One finding about the new mode lines in `release.yml`'s `version check`, not yet released, written up in the gitignored `SECURITY.local.md` with the test. It needs rights that only the repository's owner has today. The fix is three lines.
- **Would I hold the tag for it?** No.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), **before the tag:** the variable goes through `env:`, together with RB-005.4 items 1 to 3, then a dry run. The tag runs the tagged commit's workflow, so fixing it now means v0.13.2 never ships the pattern. Full triage in `SECURITY.local.md`.

#### RB-005.2 — [CORRECTNESS] [LOW] When a failed rekey also fails to zero its new WAL region, the error doesn't say so, and a later damaged header lets the new secret recover the file
- **Status:** `ACCEPTED`
- **Context / Evidence:** tested at `e7fbb59` with the scripted hook (scratch test `rb5s/tests/scripted.rs`).
  - **The code.** `undo_rekey` zeroes the new region with `let _ = wal::scrub_region(..)` (`mnemo/src/store/maintenance.rs:263-265`) and returns the original error. By then that region can hold a committed copy of page 0 with the data key wrapped under the new secret: always when the failure was at page 0, and possibly when the commit's own write or sync failed partway.
  - **The test.** Page 0's write fails, then the zeroing fails, then the write-back (or nothing, if page 0 wasn't touched) succeeds. `rekey` returns the page-0 error and nothing else. The old secret opens the file and the new one gets `WrongPassphrase`, as documented. But a copy with a damaged page 0 opens with **either** secret: the torn-header scan finds the leftover commit and accepts it for the new one. That lasted through a drop and a close. After one more write and a close it was gone, because the write reused that part of the file. I don't know that every later write does.
- **Finding:**
  - The claim "an error means the old secret is still in force, for this handle and for the file" has a second exception besides the documented one, and this one is silent. The error text gives the caller no reason to think the new secret is anywhere in the file.
  - It takes two I/O failures in a row and then a damaged header, and the secret that gains anything is the caller's own new one. So it's as rare as the documented double failure, and it matters for the same reason: a caller told "the old secret is in force" may discard the new one, or may have reasons not to want it to work.
  - If the damaged-header recovery does pick the leftover commit, it restores the header from the time of the failed rekey, not the latest one. I didn't test what that does to writes made since.
- **Recommendation:**
  1. Don't discard the zeroing's result. If it fails, return the same kind of message as the double failure: the old secret is in force, a copy of the new key wrap may be left in the file, and a later write or `compact` clears it.
  2. Or remember the region in the handle and zero it at the next flush, and say so in the error.
  3. A test for it needs a failpoint that can fail two writes in a row (see `RB-005.4` item 5).
- **Would I hold the tag for it?** No.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12) → **REKEY-2**, in the next engine release: recommendation 1 (a failed zeroing gets the "a copy of the new key wrap may be left" message), plus the multi-write failpoint from RB-005.4 item 5, so the repo keeps tests for the write-back failure, the "either secret" message and this case.

#### RB-005.3 — [CORRECTNESS] [LOW] Python: where warnings are errors, a rekey that succeeded raises
- **Status:** `ACCEPTED`
- **Context / Evidence:** by reading at `e7fbb59`; not run (no Python build here).
  - `mnemo-python/src/lib.rs:133-147`: `warn_rekey_cleanup` returns the result of `PyErr::warn_bound(..)?`. Python turns a warning into an exception when the filter says so (`-W error`, `PYTHONWARNINGS=error`, pytest's `filterwarnings = error`), and `warn_bound` then returns that exception.
  - `:1674-1676` and `:1692-1694`: `rekey` and `rekey_with_key` return it, after the engine's rekey has already succeeded.
  - The docstrings (`:1651-1655`, `:1684-1688`) and the Python README row say: "If it raises, the old secret is still in force".
  - The Worker's check ("a clean rekey raises no warning under `-W error::RuntimeWarning`") covers the clean case only.
- **Finding:** in such a process, a rekey whose cleanup was cut short raises `RuntimeWarning` with the new secret in force. A caller that follows the docstring keeps the old secret, and at its next start the file doesn't open. It needs an I/O failure in the cleanup and warnings set to errors, so it's rare. The message does say "rekey succeeded", for whoever reads it.
- **Recommendation:** one of:
  1. don't let the warning's failure become the call's: if `warn_bound` returns an error, drop it (or pass it to `write_unraisable`) and return normally;
  2. expose `rekey_cleanup_error()` on the Python handle, as Rust has it, and keep the warning as a convenience;
  3. at least add the case to the docstrings.
- **Would I hold the tag for it?** No.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12), **before the tag:** recommendation 1 (a failure to issue the warning never becomes the call's failure: pass it to `write_unraisable`, and return normally), with a test under `-W error`, and the docstring says what a `RuntimeWarning` means. Recommendation 2 (`rekey_cleanup_error()` on the Python handle) with REKEY-2.

#### RB-005.4 — [DX/DOCS] [LOW] Small things in the v0.13.2 changes
- **Status:** `ACCEPTED`
- **Context / Evidence and recommendation, one item each:**
  1. **The off switch only works spelled exactly `false`** (`release.yml:580`, tested in bash). With the commit saying `true`, a variable set to `False`, `FALSE`, `0`, `off` or `false` with a space leaves mobile on, and nothing in the log says the variable was ignored. Until v0.13.1 it failed the other way: anything but `true` meant off. Compare in lower case, and fail `version check` on any value other than `true`, `false` or empty. Fix it with `RB-005.1`, in the same lines.
  2. **A dry run doesn't notice that its `mobile` input differs from the file**, and RELEASING.md's step 5 says the dry run's summary prints what the commit says (`RELEASING.md:111`). It's printed in `version check`'s log (`release.yml:576`), not in the summary. A dry run with the default input (`true`) against a commit that says `false` rehearses a release the tag won't make. Print a `::warning::` when they differ, and write the commit's mode to the summary.
  3. **"No re-run can change it"** (`RELEASING.md:52`, the CHANGELOG, the comment at `release.yml:566-567`) is true for turning mobile on. "Re-run all jobs" does read the off switch again, so with a commit that says `true`, setting the variable to `false` between attempts turns mobile off mid-release. Say "no re-run can turn it on".
  4. **`mnemo rekey`'s failure message doesn't say which passphrase is in force.** Under a real write error it prints only "error: io error: File too large (os error 27)". REKEY-1 makes the answer certain, so say it: "the passphrase was not changed".
  5. **What `tests/rekey_cut.rs` can't reach.** The failpoint fires once, so no test runs the write-back's failure, the "either secret" message, or a failed zeroing in `undo_rekey` (`RB-005.2`); the third cleanup step has no failpoint; and the handle is always clean when `rekey` is called. My runs of those cases are in area 1. A failpoint that takes a set of write numbers would let the repo keep them.
  6. **A doc comment moved onto the wrong function** (`mnemo-python/src/lib.rs:129-133`): "Map a core error onto its Python exception type" now heads `warn_rekey_cleanup`; it belongs to `to_py` at `:149`.
  7. **In a dry run, the skipped approval job shows its raw name** ("approve the release (v${{ needs.mobile-prepare.outputs.version }}, …)" in run 43321935230). GitHub doesn't evaluate a skipped job's name. Cosmetic.
  8. **The root README handoff** (Planner-owned, staged in the working tree): "Now an error **always** means the old secret still opens the file." The engine README states the one exception. Drop "always", or leave it as a headline.
- **Would I hold the tag for it?** No. Items 1 to 3 belong with the next change to `release.yml`, which needs its own dry run.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-12). **Before the tag:** items 1 to 3 (with RB-005.1), item 4 (`mnemo rekey` says "the passphrase was not changed" on an error) and item 6 (the doc comment). **With REKEY-2:** item 5. **Item 7:** won't fix, since GitHub doesn't evaluate a skipped job's name; a line in RELEASING.md is enough. **Item 8:** done; the root README drops "always" and points at the exception.

---

### Batch `RB-006` — 2026-11-13 (REVIEWER-B: an off-cycle review of the 12.4 shared-access design note, before any locking code: the lock protocol, staleness, crash cases, the identity check, presence and claims, the daemon and mobile, with twelve new items)

**What was reviewed.** `docs/design/12.4-shared-access.md` as it stands at `9fc260b` (the note with PLANNER-A's four sign-off changes folded in; 628 lines, SHA-256 `791c622c7ecc806b…`), with PLANNER-A's 2026-11-13 sign-off entry in `PLAN.md`. The user asked for this review; it isn't a scheduled pass. The note describes code that doesn't exist yet, so most findings are about what the note says or leaves out, checked against the engine as it is on `main` at `233eba4` (v0.13.2).

**How.**
- **By reading,** the note against `mnemo/src/store/{load,open,flush,records,maintenance,snapshots,cache_api}.rs`, `pager.rs`, `wal.rs`, `format.rs`, `mcp.rs` and the Swift `FilePolicy.swift`. Line numbers are at `233eba4`.
- **Four experiments,** all in my scratch folder, on a `git archive 233eba4` copy, with `CARGO_TARGET_DIR=/tmp/rb-target`, `--locked` and `--offline`:
  1. `rb6/tests/failed_commit.rs`: the repo's one-shot failpoint, failing each write of a commit in turn, in a handle that then stays open.
  2. `rb6/tests/old_compactor.rs`: v0.13.2's compactor in a loop against a writer that opens the file for every write, as a shared handle will.
  3. `rb6/locksim.py`: the note's lock protocol (try-lock, jittered sleeps of 1 ms doubling to 50 ms, the intent lock) with real `flock` between processes. **It simulates the protocol, not the engine:** a read and a commit are sleeps. Read its ratios, not its absolute numbers.
  4. `rb6/presence.py`: the note's presence probe and identity check with real `flock` and real files on APFS.
- **Not done:** nothing ran on Linux, Windows, iOS or Android (this Mac has no Docker). Where an item depends on one of them it says "not tested". Nothing in the engine was timed. I read v0.14.0 step 1's commit (`35ab403`) only where an item touches it; that commit is not reviewed here.
- **Machine:** Apple M1 Pro, macOS 26.7.1, Rust 1.95.0, Python 3.11.2.
- **Read-only.** I wrote this file and the gitignored `SECURITY.local.md`, nothing else.
- **Three security findings** (`RB-006.10` to `.12`) are only in `SECURITY.local.md`. One of them is about shipped code and bears on step 1.

---

**The verdict first.**

- **The shape is right.** (A) as the one protocol, with the daemon as an optional (A) client, survives this review. I found nothing that argues for (B) as the base, and nothing that has to have a format change.
- **The note isn't yet enough to write step 2 from.** It covers a process that dies. It doesn't cover a commit that fails in a process that lives on, and the engine today leaves such a handle ahead of the file with an unchanged stamp (`RB-006.1`). It also leaves out what a caller does when the header's seal doesn't verify, which is what every compaction causes (`RB-006.2`).
- **What I'd want settled before step 2's code:**
  1. `RB-006.1`: the rule for a failed commit, and a reload after a WAL replay.
  2. `RB-006.2`: what tells a handle the file changed (the stamp, a seal that fails, the identity check).
  3. `RB-006.11`, parts (a) and (b) (in `SECURITY.local.md`): two things the lock layer's code has to do from its first version.
  4. From `RB-006.3`: check the file's identity again after the commit, and decide whether older binaries need a gate.
  5. From `RB-006.4`: a reader shouldn't hold the intent lock while it sleeps, and the backoff's shape. Both are a few lines in step 2 and awkward to change later.
  6. From `RB-006.6`: on iOS, no lock is held between calls (presence included), and a wait can be cancelled.
  7. From `RB-006.7`: items 1 and 2 (the second read that writes, and per-handle settings).
- **What can wait for its own step:** `RB-006.5` (presence and claims, step 7; only its sidecar-path rule lands in step 2), `RB-006.8` (incremental catch-up, step 6), `RB-006.9`, and `RB-006.12` (the daemon, step 9).
- **For step 1, now:** `RB-006.10` (in `SECURITY.local.md`) is about today's compaction and export. Step 1's commit already changes the lines involved, so it's cheapest to finish there.
- **Two of the Planner's changes tested as intended.** Without the intent lock a writer starves (8 of 40 commits got the lock within 3 s against eight readers, and 0 of 20 against sixteen). With it, one writer never waited more than 188 ms. And without step 1's fix, v0.13.2's compactor lost 207 of 278 acknowledged writes in my run, which matches the Worker's 149 of 150.

---

**Area by area: what holds and what doesn't.**

| Area | Holds | Doesn't, or isn't said |
|---|---|---|
| The lock protocol (steps 1 to 8) | The order is right: slow work first, lock, identity, stamp, WAL, work, commit, unlock, close. `flock` per open file description means two handles in one process exclude each other, as they must | A failed commit (`.1`). A reload after replay (`.1`). The seal failing (`.2`). One deadline across the intent and main locks, and across step 4's retries (`.4`, `.2`) |
| Writer preference | One writer against 8, 16 and 32 readers: 100 of 100 commits, the longest wait 188 ms | The numbers in the note (`.4`): about 20 commits a second with readers present, not 60; readers wait 50 to 600 ms, not "milliseconds"; several writers have no order among themselves |
| Staleness and catch-up | Every header write in the engine raises the transaction id (`flush.rs:553`, `flush.rs:75`, `records.rs:1309`, `maintenance.rs:198`), so the stamp never misses a commit to the same file. A restore is a new transaction (`snapshots.rs:131-136`) | The stamp can repeat across different files at one path (`.2`). The page cache's GCM claim (`.8`). What else a handle holds (`.7`) |
| The crash table | Every row is right for a process that dies | It has no row for a process that gets an error and lives (`.1`), and none for the handle's first write (`.11`) |
| The identity check | Catches a rename over the path, tested: 337 of 337 against the old compactor | Not a copy over the file, tested (`.2`). Inode numbers can be reused on Linux, not tested (`.2`). An older compactor after the check (`.3`) |
| Presence and claims | A killed holder is seen as gone at once, tested | Five ordinary cases where the probe is wrong, and no way for a holder to learn it lost a claim (`.5`) |
| The daemon's trust boundary | Opt-in, never auto-started, and the note says plainly that same-user processes get access | `RB-006.12`, in `SECURITY.local.md` |
| Mobile | (A) alone is right for phones, and the file's protection class (`completeUntilFirstUserAuthentication`, `FilePolicy.swift:49`) lets a per-call open work on a locked device | The presence lock contradicts "no lock between calls" (`.6`) |
| Mixed versions | An older long-lived handle only makes shared callers wait | "No version can corrupt another's file" has one exception (`.3`) |

---

**New items**

#### RB-006.1 — [CORRECTNESS] [HIGH] The note covers a process that dies, not a commit that fails in a process that lives on: today such a handle is ahead of the file with an unchanged stamp, and an error can still mean "committed"
- **Status:** `ACCEPTED`
- **Context / Evidence:** tested at `233eba4` (`rb6/tests/failed_commit.rs`): a 3-memory file, then one `remember` and `flush` with the failpoint failing one write. Each row is one or more failing writes. "Page 0" is the note's stamp (the write counter and the transaction id) and the header's seal.

| Row | The handle | Failing write | The call | The handle then holds | Page 0 | Another process, read-only open | Another process, read-write open |
|---|---|---|---|---|---|---|---|
| 1 | has written before | 0 to 2 (up to the WAL commit) | `Err` | 4 memories | unchanged, same seal | 3 | 3 |
| 2 | has written before | 3 to 8 (after the WAL commit) | `Err` | 4 | unchanged, same seal | `NeedsWriteOpen` | **4** (it replays the WAL) |
| 3 | new | 0 | `Err` | 3 | unchanged | 3 | 3 |
| 4 | new | 1 | `Err` | 3 | unchanged | `NeedsWriteOpen` | 3 |
| 5 | new | 2 to 4 | `Err` | 4 | transaction id 2 → 3, new seal | 3 | 3 |
| 6 | new | 5 to 8 | `Err` | 4 | the same | `NeedsWriteOpen` | **4** |

  - In every row the same handle's next `flush` succeeded. That is today's design: one owner, which keeps its unflushed writes and retries (`flush.rs:484-491`, `try_close`).
  - The crash table and step 5 assume the opposite: the process is gone, or its memory matches the header.
- **Finding:**
  1. **A handle can be ahead of the file while the stamp says "equal".** After row 1 the handle holds a memory the file doesn't, and page 0 hasn't moved (row 5 is the same, apart from the new handle's first header-only commit). Under the note's step 5 its next call, a read included, serves that memory to its caller. Its next successful commit stores a write whose caller was told it failed. If another process commits first, the handle either drops the write on reload or, with any incremental path, builds on a base that no longer exists.
  2. **An error can mean "committed".** In rows 2 and 6 the caller got `Err`, and the next process to open the file read-write got the memory. In shared mode that next process is usually another agent, milliseconds later. A caller that retries its `remember` stores it twice.
  3. **Step 6 doesn't reload after a replay.** It says the caller "re-checks from step 4, replays the WAL as `open` does today, and goes on". The replay rewrites page 0 with a later transaction, and the caller's memory is from before it. By reading: a caller that goes on from there commits a transaction id the file has already used, on top of a catalog that lacks the replayed rows. `open` doesn't have this problem because it loads after it replays (`open.rs:472-486`).
  4. **A panic inside the window** unlocks through `LockedFile::drop` and leaves the handle half-changed. The same rule has to cover it.
  5. **A new handle's first write can leave a pending WAL with nothing changed** (row 4). Read-only callers then get `NeedsWriteOpen` until a writer's call.
- **Recommendation:**
  1. Add the rule to the protocol: a handle clears its stamp before a mutation touches its memory, and sets it again only from the header a successful commit wrote. A cleared stamp means "reload from the file" at the next call, whatever page 0 says. An error or a panic inside the window leaves it cleared.
  2. Say what the caller gets when the failure comes after the WAL's commit point. Either finish the checkpoint from the WAL before returning (it's the same replay), or return an error that says the write may have been stored. Never a plain "failed".
  3. Step 6: after a replay, reload unconditionally.
  4. Add the rows to the crash table: "a commit fails before the commit point, the process lives", "after it", and "a replay by a handle that was current".
  5. Step 3's two-process test injects failures in a child that dies. Add the same set in a process that lives and keeps calling, and assert that no call ever returns a memory that a fresh open of the file doesn't have.
- **Would I want it settled before step 2's code?** Yes. It's the core of what step 2 writes.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13), **before step 2's code**, all five: the stamp is cleared before a mutation and set only from a successful commit; a failure after the commit point finishes the checkpoint from the WAL before returning, or, if it can't, returns a distinct "may have been stored" error, never a plain failure; step 6 always reloads after a replay; the crash table gains the three rows; the test set runs in a process that lives on.

#### RB-006.2 — [CORRECTNESS] [MEDIUM] What tells a handle the file changed: every compaction makes a new data key, the note doesn't say what a failed seal means, and the stamp can repeat
- **Status:** `ACCEPTED`
- **Context / Evidence:**
  - **Every compaction rotates the data key and restarts the counters.** `compact_file_secret` builds the new file with `create_secret` (`maintenance.rs:726`), which draws a fresh salt and a fresh data key (`open.rs:225-226`) and starts the write counter and the transaction id at 0 (`open.rs:242`, `:263`). The note's gap 2 treats a new data key as what `rekey --rotate` does, and a plain compaction as something a handle might survive without its secret.
  - **A seal that fails is tampering today.** `open` never heals it (`open.rs:488-502`, the DUR-1 decision). The note's step 5 says "verify its seal under the data key" and then gives two outcomes, "equal" and "different". It doesn't give the third.
  - **`cp` over the file keeps the inode** (tested, `rb6/presence.py`): the identity check can't see it. A rename changes it.
  - **APFS didn't reuse an inode number** in 2,000 create-and-delete rounds (tested). ext4 and tmpfs do reuse them soon after a file is freed. That's from general knowledge of those filesystems, not tested here.
  - **The header's seal has a fresh random 12-byte nonce on every write** (`format.rs:97`), stored at bytes 242 to 270 of page 0.
- **Finding:**
  1. After any `mnemo compact`, every running handle holds the wrong data key. On macOS the identity check sees the new inode first. On a filesystem that reuses inode numbers, two compactions in a row can leave the path with the inode the handle remembers. A freshly written small file ends at a low transaction id (2 for my 3-memory test file, with a write counter of 6), and the write counter depends only on how many pages were sealed, so the stamp can match too. Then the only thing that tells the handle is the seal failing under its old key. If step 2's code treats that as `HeaderTampered`, every session dies with a tampering error after an ordinary compaction on Linux.
  2. A copy written over the file in place (a restored backup, `cp`, a sync tool's in-place mode) keeps the inode and may keep the data key. The stamp catches it only if the counters differ. Two copies that diverged from one ancestor by the same number of commits and pages have equal stamps and different contents.
  3. Step 2 says "open the path". It must not create it. A handle whose file was deleted (`git clean -fdx` on an ignored brain) should get an error, never a new empty brain.
  4. Step 4's "go back to step 2" has no bound of its own. Step 1's code uses 8 attempts (`load.rs`, `35ab403`); the note should say the deadline covers it.
  5. A torn page 0 isn't in step 5 either. The scan that heals it reads the whole file and runs the KDF once per candidate (`open.rs:46-161`), which shouldn't happen under a shared lock.
- **Recommendation:**
  1. **Make the stamp the header's seal** (its 12-byte nonce and 16-byte tag, 28 bytes compared), with the file identity as a quick first test. It changes on every header write, and it can't repeat across compactions, copies or restores. It needs no format change, and it's cheaper than parsing two counters.
  2. **Write the third outcome into step 5:** the seal doesn't verify under the held key → treat the file as replaced, reopen with the kept secret (one KDF run), and only if that fails return the error (`WrongPassphrase`, or `HeaderTampered` when the secret opens the key but the seal still fails).
  3. Correct gap 2: every compaction needs the kept secret, not only a rotation. That also settles that a shared handle must keep its secret (see `RB-006.11` part (c)).
  4. Step 2 opens without creating. A missing file is an error.
  5. A torn page 0: a read-write caller takes the exclusive lock and runs `open`'s recovery; a read-only caller gets `NeedsWriteOpen` at once, without the scan.
  6. Tests: a handle that lives through two compactions; a copy written over the file in place; a deleted file. On Linux CI, the inode-reuse case if it can be forced.
- **Would I want it settled before step 2's code?** Yes.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13), **before step 2**, all six: the stamp is the header seal (nonce and tag) with the identity as the quick test; step 5 gains the "seal doesn't verify" outcome (treat as replaced, reopen with the kept secret, then the specific error); gap 2 says every compaction needs the secret; step 2 never creates the file; the torn-page-0 rule; the tests.

#### RB-006.3 — [CORRECTNESS] [MEDIUM] Mixed versions: an older `mnemo compact` can still lose a shared writer's commit, and today's lifetime lock is what prevents it
- **Status:** `ACCEPTED`
- **Context / Evidence:** tested at `233eba4` (`rb6/tests/old_compactor.rs`): v0.13.2's `compact_file` in a loop, against a writer that opens, writes one memory and closes, 20 seconds a run.

| The writer | Compactions | Writes acknowledged | Missing at the end | Opens the identity test refused |
|---|---|---|---|---|
| retries at once, no identity test | 209 | 278 | **207** | none |
| retries at once, with the note's step-4 test (as near as a caller can get: the path names the same inode before and after the open) | 337 | 19 | 0 | 336 |
| 1 to 50 ms backoff, with the test | 144 | 228 | 0 | 1 |

  - The first row is the note's gap 1, and step 1 fixes it for binaries that have the fix.
  - **Today a running MCP server's lifetime lock makes `mnemo compact` fail with `Locked`.** In shared mode nothing holds the lock between calls, so an older CLI, an older wheel in some virtualenv, or an app built against an older binding compacts whenever it likes. v0.13.2's compactor still drops its lock before it renames (`maintenance.rs:744-746`).
  - The identity test caught every case I could produce. What it can't catch is a rename that lands after the test passes, during the commit. I didn't hit that in 481 compactions. By reading, nothing prevents it, and when it happens the write is lost with `Ok` returned.
  - Old binaries ignore the header's `flags` field (`format.rs:447` reads it; nothing checks it), so a new flag wouldn't stop them. Only a format version would.
- **Finding:** the note says "No version can corrupt another's file, because both use the same OS lock." That's true of corruption. It isn't true of lost writes: an older compactor plus a shared writer is the one pair where the lock doesn't serialize the two. It's rare with the identity test in place, and silent.
- **Recommendation:**
  1. **Check the identity again after the commit, before the call returns `Ok`.** If the path no longer names the file just written, return an error that says the file was replaced during the write and the write isn't in it. Step 1's `ensure_not_replaced` runs before each commit; this is the same call after it. An old compactor renames within microseconds of dropping its lock and a commit takes milliseconds, so this turns nearly every silent loss into a reported one.
  2. Change the note's sentence to name the exception.
  3. Decide whether pre-0.14 binaries should be kept off a file that shared handles use. The only gate they obey is the format version, which the format-version policy covers. The cheaper answer is a line in the upgrade notes: update every `mnemo` on the machine before turning `--shared` on, and before step 8 makes it the default.
  4. Step 3's "Windows on CI" should include an old-compactor case if the 0.13.2 binary can be fetched there.
- **Would I want it settled before step 2's code?** Recommendation 1, yes: it's a line in step 2. Recommendation 3 is a decision to make before step 8.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). **Before step 2:** recommendation 1 (the identity check again after the commit, with a distinct "replaced during the write" error) and 2 (the note names the exception). **Recommendation 3:** for v0.14 the upgrade notes say to update every `mnemo` on the machine before using `--shared`; whether to gate older binaries by format version is decided before step 8. **Recommendation 4:** if the 0.13.2 binary can be fetched on Windows CI.

#### RB-006.4 — [PERF] [MEDIUM] Writer preference does its job for one writer; the backoff costs more than the note's numbers say, and several writers have no order among themselves
- **Status:** `ACCEPTED`
- **Context / Evidence:** `rb6/locksim.py`, a simulation of the note's protocol with real `flock` between processes. A commit is a 16 ms sleep under the exclusive lock. Readers loop with no pause. "Plain" is the note before the sign-off change; its writer gave up after 3 s.

| Readers | A read holds the lock for | Mode | Commits that got the lock | Wait for the lock: median, max | Commits a second | Reads a second | A reader's longest wait |
|---|---|---|---|---|---|---|---|
| 8 | 0.2 ms | plain | 8 of 40 | 1,431 ms, 2,018 ms | 0.1 | 23,556 | 52 ms |
| 8 | 0.2 ms | intent | 100 of 100 | 14 ms, 37 ms | 25.0 | 7,184 | 55 ms |
| 8 | 2.5 ms | plain | 7 of 40 | 968 ms, 2,315 ms | 0.1 | 2,168 | 78 ms |
| 8 | 2.5 ms | intent | 100 of 100 | 20 ms, 44 ms | 20.7 | 808 | 54 ms |
| 8 | 12 ms | intent | 100 of 100 | 34 ms, 99 ms | 15.5 | 190 | 178 ms |
| 8 | 112 ms | intent | 100 of 100 | 153 ms, 188 ms | 5.6 | 43 | 617 ms |
| 16 | 2.5 ms | plain | 0 of 20 | none | 0 | 4,257 | 10 ms |
| 16 | 2.5 ms | intent | 100 of 100 | 22 ms, 76 ms | 19.0 | 1,571 | 64 ms |
| 32 | 2.5 ms | intent | 100 of 100 | 25 ms, 63 ms | 17.9 | 3,353 | 73 ms |

  Writers only (the intent protocol, no pause between commits; 16 ms back to back would be 62 a second):

| Writers | Commits a second, all together | Wait for the lock: median, 95th percentile, max | Waits over 1 s |
|---|---|---|---|
| 1 | 42.9 | 0.2 ms, 0.6 ms, 0.8 ms | 0 of 60 |
| 2 | 33.3 | 38 ms, 52 ms, 54 ms | 0 of 120 |
| 4 | 33.5 | 41 ms, 400 ms, 1,045 ms | 1 of 240 |
| 8 | 32.3 | 50 ms, 874 ms, 2,612 ms | 19 of 480 |
| 4, with 8 readers | 20.0 | 43 ms, 667 ms, 1,039 ms | 1 of 160 |

- **Finding:**
  1. **The change was needed and it works.** Without the intent lock a writer mostly never gets in. With it, one writer always did, well inside the sign-off test's 1 s.
  2. **The note's rate is high by a factor of two to three.** It says "roughly 60 durable commits a second" across all processes. With two or more writers the simulation gives about 33, and about 20 once readers are present. The cause is the backoff: a waiter that sleeps up to 50 ms overshoots a 16 ms hold, and the lock sits free until it wakes.
  3. **Readers pay more than "milliseconds at most"** (the note's comparison and readers tables). Each commit costs every waiting reader up to one 50 ms sleep, and reads fall to about a third while one writer is busy.
  4. **Writers have no order.** Each polls on its own, so with 8 writers 4% of commits waited over a second and one waited 2.6 s. The sign-off test has one writer, so its "every write under 1 s" would pass while this case fails.
  5. **A reader holds the intent lock while it sleeps.** The note says a reader "never holds it during the read", which is true, but it does hold it for as long as it waits for the main lock. While a long operation holds the main lock, every waiting reader holds the intent lock shared, and a second writer can't take it to queue.
  6. A writer waits for the intent lock "with the same backoff and deadline", then for the main lock. Read literally that's two deadlines, 20 s.
- **Recommendation:**
  1. A reader takes the intent lock, tries the main lock once without waiting, and releases the intent lock whether or not it got it. If it didn't, it sleeps and starts again from the intent lock.
  2. Keep the sleeps short while the wait is young (a cap of a few milliseconds for the first few hundred), and grow them only after. A commit holds the lock for 16 ms, so a 50 ms sleep can't be right for the common case. Measure it in step 5's bench.
  3. One deadline per call, covering both locks and step 4's retries.
  4. Add to the starvation test: four and eight writers at once, with the assertion on the 99th percentile; and readers' waits.
  5. Correct the note's numbers once step 5 has measured them, and until then say "tens of commits a second".
- **Would I want it settled before step 2's code?** Recommendations 1 and 3, yes: they're part of the lock loop. The rest is measurement.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). **Before step 2:** recommendations 1 (a reader holds the intent lock only for one try) and 3 (one deadline per call, across both locks and the identity retries). **Step 5:** recommendation 2 (short sleeps first), 4 (multi-writer tests asserting on the 99th percentile, plus readers' waits) and 5 (the note says "tens of commits a second" until measured). If several writers' 99th percentile is poor, step 5 adds an order among writers.

#### RB-006.5 — [CORRECTNESS] [MEDIUM] Presence by lock probe is wrong in several ordinary cases, and "a dead holder's claim lapses at once" turns each one into a claim taken from an agent that is still working
- **Status:** `ACCEPTED`
- **Context / Evidence:** `rb6/presence.py`, the note's probe with real `flock` on APFS. "Alive" is what the probe returned.

| Case | The probe says | Right? |
|---|---|---|
| nobody holds the file; a holder runs; the holder is killed with `kill -9` | not alive; alive; not alive | yes |
| the holder is stopped (`SIGSTOP`) | alive | yes, and `expires_at` covers it |
| the holder forked a worker that never `exec`s (Python's `multiprocessing` fork start), then was killed | **alive** | no |
| the sidecar directory is deleted while the holder runs, and the probe recreates the file | **not alive** | no |
| two probes at the same moment, nobody running | the second says **alive** | no |
| a holder tries to register while a probe has the file | **refused** | it must retry, or it's never seen |

  By reading, not tested:
  - **The sidecar is found by path, the lock by inode.** Two paths to one brain (a symlinked directory, a hard link) agree on the main lock, as the note says, and get two sidecar directories. Agents on one path are "not alive" to agents on the other.
  - **An agent with no long-lived process is never alive.** 13.1b's hook-and-poll fallback and the CLI are processes that exit. Their claims are free the moment they return.
  - **A restart is a gap.** A client that reconnects its MCP server, a crash and restart, or an upgrade leaves the presence file unlocked for a moment.
- **Finding:**
  1. The note's rule is "a crashed holder's claim lapses at once (the lock probe says it isn't alive)". In four of the cases above the holder is working and the probe says it isn't. `git clean -fdx` in a repository whose brain's sidecar is ignored (the `.gitignore` pattern makes it so) releases every claim of every running session.
  2. **A holder never learns it lost a claim.** `claim` returns `Claimed` once. If the claim lapses during a stall (stopped, out of usage, a sleeping laptop past `expires_at`) or is taken through one of the cases above, the first holder resumes and carries on, and two agents work the same topic. That's the thing claims exist to prevent.
  3. The note says "Deleting the directory while nothing runs loses nothing". While something runs, it loses presence, silently.
- **Recommendation:**
  1. **Give a claim a token** (the transaction id that wrote it). `renew`, `release` and a new `check(topic, token)` return `Lost` when the live claim's token differs. Say in 13.3's docs that an agent checks before an irreversible step.
  2. **Make the early lapse a hint.** Lapse early only when the holder registered a presence lock when it claimed (record that in the claim) and has been not-alive for a short grace period (a few seconds, seen twice). Otherwise wait for `expires_at`.
  3. Presence files get the main file's identity check on every call: if the path no longer names the locked file, lock the new one.
  4. The probe retries once after a short sleep before it says "alive", and a holder registers with the bounded backoff.
  5. Resolve the brain's path (symlinks included) before deriving the sidecar's path, and say that hard links aren't covered.
  6. The clock: say what a step backwards or forwards does to `expires_at`. Leave claims and presence records out of 13.7's merge, as `export --git` leaves them out.
- **Would I want it settled before step 2's code?** No, it's step 7. Only recommendation 5 lands in step 2, with the sidecar directory.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13) → step 7 (13.3's engine half): claim tokens with `Lost`, the early lapse as a hint with a grace period, presence files re-checked by identity, the probe's retry, and the clock. **Recommendation 5** (resolve symlinks before deriving the sidecar path; hard links not covered) lands in step 2 with the sidecar directory.

#### RB-006.6 — [CORRECTNESS] [MEDIUM] Mobile: the presence lock is held between calls, which is what iOS kills for, and a wait for the lock inside a background activity has no way to stop
- **Status:** `ACCEPTED`
- **Context / Evidence:** by reading; nothing here ran on a device or a simulator.
  - The mobile section rests on "A shared-mode store holds no lock between calls, so suspension between calls is always safe." The protocol says the same: "no file descriptor and no lock between calls".
  - The presence section says each shared handle that declares an agent id "holds a shared lock on its own sidecar file for its lifetime". The sidecar directory is next to the brain, so for an App Group file it's in the shared container.
  - MOBILE-2's reason for its background rules is that iOS kills a suspended app that holds a file lock in a shared container (`0xdead10cc`).
  - The engine creates the sidecar files. The Swift package's `FilePolicy` sets the protection class and the backup flag on the store's own directory (`FilePolicy.swift:46-58`), not on files the engine adds later.
- **Finding:**
  1. A phone app that uses presence holds a lock in the shared container while suspended. That's the case MOBILE-2 exists to avoid.
  2. A caller waiting for the lock holds the intent lock (a writer always; a reader too, see `RB-006.4`) for up to the 10 s deadline. A background activity can expire sooner than that, and an extension's whole budget is short (a notification service extension gets about 30 s). If the app is suspended mid-wait, it's suspended holding a lock.
  3. In an app whose default protection is `complete`, sidecar files created by the engine can't be opened while the device is locked, so a notification extension's calls fail there even though the brain itself opens.
  4. A widget or an extension that opens read-only gets `NeedsWriteOpen` after a writer died mid-commit, and stays that way until the app itself runs.
  5. Android: "the magic numbers on Linux" has to accept f2fs, which most phones use for app data, and refuse the FUSE-backed shared storage. The note doesn't say whether the list allows or denies.
- **Recommendation:**
  1. On iOS, no presence lock. Report `last_active` only, or hold the presence lock for the length of each call.
  2. A wait can be cancelled from another thread (the activity's expiry handler), and the cancel releases every lock the call holds. The mobile stores pass a deadline no longer than the time the activity has left, and a default much shorter than 10 s.
  3. The Swift and Kotlin stores create the sidecar directory themselves, through the same policy as the brain's directory (protection class, excluded from backup).
  4. Say what a read-only extension does with `NeedsWriteOpen`.
  5. Say that the Linux filesystem list is an allow-list, and name f2fs and ext4 for Android.
- **Would I want it settled before step 2's code?** Recommendation 2's cancel, yes: it's in the shape of the lock loop's API. The rest goes with mobile's opt-in.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). **Before step 2:** recommendation 2 (a wait can be cancelled from another thread and releases every lock; the mobile stores pass a deadline no longer than the activity's remaining time). **With mobile's opt-in:** recommendations 1 (no presence lock on iOS), 3, 4 and 5 (the Linux list is an allow-list naming ext4 and f2fs).

#### RB-006.7 — [CORRECTNESS] [MEDIUM] What a shared handle keeps or does that the note doesn't cover: a second read that writes, per-handle settings that change the file, and the watcher's descriptor
- **Status:** `ACCEPTED`
- **Context / Evidence and recommendation, one item each:**
  1. **`cache_get` writes too.** On a read-write handle a cache hit updates the directory's `accessed_at` and `access_count` and sets `dirty_cache` (`cache_api.rs:169-171`; `cache_get_semantic` at `:563-565`). The note defers recall's statistics and nothing else. Treat cache hits the same way, or a cache read needs the exclusive lock and a commit.
  2. **Per-handle settings decide what a commit deletes.** Snapshot retention runs at every commit with the committing handle's `max_snapshots` (`snapshots.rs:152-178`). An explicit cap in one process ("keep the newest 10") removes restore points that every other process would have kept, at its next commit. The cache budgets and `turn_compress_after` are per handle in the same way. Today that needs two programs opening the file in turn; in shared mode it's continuous. Either store the retention policy in the file, or say that shared handles use the default policy and refuse an explicit one. Separately, a reload "exactly as `open` does" resets these settings and the embedder (`open.rs:691-736`); the reload must carry them over.
  3. **The embedding from step 1 can be stale.** It's computed before the lock. After the catch-up, check that the file's dimensions (and model, where recorded) still match, and recompute if not.
  4. **Read, then write, across two calls.** An agent recalls a memory and then updates or forgets it by id. Another process may have changed, superseded or deleted it in between. Say what `update` and `forget` return for a superseded or missing id, and consider an expected-version argument later.
  5. **Statistics applied "at close".** `close` then needs the exclusive lock and a commit, can wait for the deadline, and can fail. An MCP server's `close` runs only when stdin closes (`mcp.rs:253`); clients often just kill it. And each such commit moves the stamp, so every other process reloads for a change it doesn't care about. Apply them only with a commit the handle makes anyway, and drop them at `close`.
  6. **The 13.1b watcher needs a descriptor.** `EVFILT_VNODE` watches an open file, so a session's server holds a descriptor between calls after all (no lock; say so, since the protocol says "no file descriptor"). After a compaction it's watching the replaced file, so it must re-arm on rename and delete, or watch the directory. If it watches the directory, writes to `lock-info` wake it.
  7. **The watcher's cost is a reload, not a header check.** The note says "one header check per foreign commit per session". A moved stamp means the session must catch up before it can query its inbox: 3.2 ms at 10K and about 30 ms at 100K per session per commit by the note's own table, and marking an item `delivered` is another commit that wakes everyone again.
- **Would I want it settled before step 2's code?** Items 1 and 2, yes. The rest belong to their steps.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). **Before step 2:** item 1 (cache-hit statistics deferred like recall's) and item 2 (in v0.14, shared handles use the file's default retention, cache budgets and `turn_compress_after`, and refuse explicit settings with a clear error; storing the policy in the file comes later; a reload carries the handle's settings and embedder over). **At their steps:** items 3 to 7, with item 5 as recommended (apply statistics only with a commit the handle makes anyway, and drop them at close).

#### RB-006.8 — [CORRECTNESS] [LOW] Incremental catch-up (step 6): a cached page is never checked again, so the note's safety argument for stale pages doesn't hold
- **Status:** `ACCEPTED`
- **Context / Evidence:** by reading.
  - The note: "v9 binds each page's generation into its AAD, so a stale cached page that was freed and reused fails its GCM check rather than returning wrong data."
  - The page cache holds decrypted pages keyed by page number alone, and a hit returns before any decryption (`pager.rs:365-368`). The generation is checked only when a page is read from disk.
  - The note's later rule is to keep the cache "except for pages the free list shows as reused". A page that was freed and then reused since the handle's transaction is no longer on the free list at all.
  - The catalog's delta chain is dropped and rebuilt at a base rewrite (`flush.rs:269-277`), and that rewrite removes tombstoned rows, which moves row positions (`flush.rs:687-689`); deltas address rows by position (`load.rs:292-295`).
- **Finding:** the first version (clear the cache, reload everything) is safe. The incremental version as described would serve an old page's contents for a reused page number, with no error, and can't find the reused pages the way it says.
- **Recommendation:**
  1. Fix the sentence now, so nobody builds on it.
  2. For step 6: key the cache by page number and generation, or drop every cached page named by a changed row (old and new location).
  3. Fall back to the full reload when the chain doesn't reach back to the handle's transaction (a base rewrite happened).
  4. The "both give the same state" test should include a base rewrite, a compaction of tombstones, and a page reused between the two points.
- **Would I want it settled before step 2's code?** No. Recommendation 1 is a one-line edit to the note.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). Recommendation 1 now, in the note's revision; 2 to 4 in step 6.

#### RB-006.9 — [DX/DOCS] [LOW] Smaller points in the note
- **Status:** `ACCEPTED`
- **Context / Evidence and recommendation, one item each:**
  1. **`lock-info` is written before the lock is taken**, so a second long operation that is still waiting replaces the line of the one that holds the lock (tested with two writers, `rb6/presence.py` step 7: the waiter read the wrong name). Write it after the lock is held, and remove it before unlocking.
  2. **Which filesystems are refused on Linux.** The note says "the magic numbers", without saying allow or deny. A brain shared between a host and a container or WSL is a likely setup for coding agents, and those mounts (virtiofs, 9p, FUSE) are local-looking filesystems where a lock may not reach the other side. Not tested here. Make it an allow-list, and add a line to the docs.
  3. **A directory the caller can't write.** A reader of another user's file, or of a file in a read-only directory, can't create the sidecar. Say that it then takes the main lock alone.
  4. **Read-only callers and a pending WAL.** With hooks reading `--no-track` and servers idle, nobody heals the file until a write-capable call happens. Let the watcher or `serve --mcp --shared` heal it when it sees `NeedsWriteOpen`.
  5. **The per-call cost** in the comparison table leaves out the WAL check (one more 29-byte read, `wal.rs:358-369`). It's small; list it so step 5's bench measures the real sequence.
  6. **Windows.** The note plans to test the rename over an open file on CI. `std::fs::rename` has changed its Windows behaviour between toolchains, and the MSRV (1.75) and CI's stable differ. I'm saying this from memory and didn't check it here. Step 1's commit message names one toolchain for its probe; run it on the MSRV too before relying on it.
  7. **"It holds no file descriptor and no lock between calls"** needs its two exceptions written next to it: the presence lock (`RB-006.5`, `.6`) and the watcher's descriptor (`RB-006.7` item 6).
- **Would I want it settled before step 2's code?** Item 1 and item 3, since step 2 creates the sidecar directory. The rest are edits to the note.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). Items 1 (write `lock-info` after the lock) and 3 (no sidecar when the directory isn't writable) in step 2; items 2, 4, 5, 6 and 7 as edits to the note (item 6: run step 1's Windows probe on the MSRV too).

#### RB-006.10 — [SECURITY] [LOW] Held in `SECURITY.local.md`
- **Status:** `ACCEPTED`
- **Context / Evidence:** one finding about shipped code (v0.13.2 and earlier), in the lines of `maintenance.rs` and `load.rs` that step 1's commit `35ab403` is changing. Tested at `233eba4`. Step 1's change already makes it impractical; one more line closes it.
- **Would I want it settled before step 2's code?** It belongs in step 1's PR.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13), in step 1's PR. Full triage in `SECURITY.local.md`.

#### RB-006.11 — [SECURITY] [MEDIUM] Held in `SECURITY.local.md`
- **Status:** `ACCEPTED`
- **Context / Evidence:** three findings about the design of the lock layer (the protocol and the sidecar directory). No code exists, so nothing shipped is affected. By reading, with line references.
- **Would I want it settled before step 2's code?** Yes, parts (a) and (b). Part (a) also changes the note's cost for a write.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13). Full triage in `SECURITY.local.md`.

#### RB-006.12 — [SECURITY] [MEDIUM] Held in `SECURITY.local.md`
- **Status:** `ACCEPTED`
- **Context / Evidence:** findings about the design of the daemon's discovery and trust boundary. No code exists. By reading.
- **Would I want it settled before step 2's code?** No. Before step 9.
- **Planner Triage:** `ACCEPTED` (PLANNER-A, 2026-11-13), before step 9. Full triage in `SECURITY.local.md`.
