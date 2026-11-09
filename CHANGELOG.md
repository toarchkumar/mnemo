# Changelog

All notable changes to this project are recorded here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Pre-1.0, the minor component carries the breaking-change signal.

## [Unreleased]

### Changed

- **`Mnemo::close` takes `self`** (it took `&mut self`). It flushes as
  before (nothing on a read-only handle), then drops the handle, so the
  file and its OS lock are released when it returns and the same file can
  be reopened at once. Using a handle after `close` is now a compile error
  rather than a runtime `Locked` on the next open. Code that keeps a
  handle in a struct and closes it from `&mut self` stores an
  `Option<Mnemo>` and calls `self.db.take()` (see the `close` docs). If
  the flush fails, the handle is still dropped. `Drop` alone still
  doesn't flush. Python's `close()` already released the lock and is
  unchanged. Node's `close()` now releases it too, and the Node methods
  throw "database is closed" afterwards (`len()` and `snapshots()` now
  return errors instead of values).
- Internal: `store.rs` is split into private submodules under
  `mnemo/src/store/` (REFAC-1). No public API change.

### Added

- **`Mnemo::ingest_thread_texts` and `Mnemo::ingest_thread_with_vectors`**
  split `ingest_thread` in two, so a caller can embed between the calls
  without holding the handle (the UniFFI binding does). The first returns
  the texts to embed; the second applies the caller's vectors, planning
  again and refusing a count that no longer matches.
- **`mnemo-uniffi`** (MOBILE-1a, `mnemo/bindings/uniffi/`, not published
  yet): the UniFFI binding that the Swift and Kotlin packages are built
  from. It has a curated API, a foreign-callback embedder that's never
  called with the handle locked, mobile defaults (a 16 MiB vector budget
  and a 2 MiB page cache), and passphrase export and import for moving a
  key-bound file between devices. It needs Rust 1.82 or later; the engine
  stays at 1.75.
- CI: `cargo deny --workspace` now checks every workspace member, which
  adds per-crate license exceptions (MPL-2.0 for UniFFI; ISC for
  `libloading`, via the experimental Node binding). New jobs: an Android
  `x86_64` build, a release build of the binding on each mobile target
  with a 5 MB stripped-size gate, and `msrv-uniffi`.

## [0.11.0] — 2026-11-08

FMT-9: the on-disk format is v9. New files are v9; v8 files keep opening
and writing as v8 until you run `mnemo compact`. v0.10 and earlier can't
open v9 files, so upgrade every tool that reads a file before compacting
it. Measured with `examples/device_budget.rs` on an Apple M1 Pro (384-dim
vectors, ~300 bytes of text); full numbers in
`test/scripts/perf_v0.5.0.json` (`device_budget_v0_11_0`, marked
`remeasured`).

### Changed — the on-disk format is v9 (FMT-9)

**New files are v9.** The format details are under the 9a–9f items below;
in short, a 100K-memory file is about 1.5 KB per memory instead of about
8.7 KB, durable writes and warm recall are faster, and recall can match
exact identifiers.

**Upgrading (9f):**
- **v8 files keep working.** They open and write as v8, so v0.10 and
  earlier can still read them. `mnemo info` and `mnemo about` print a
  hint to upgrade.
- **`mnemo compact <file>` upgrades a file to v9** (`Mnemo::compact_file`
  in Rust, `compact_file` in Python). **Upgrade every tool that reads the
  file before you compact it:** v0.10 and earlier can't open v9, and
  report `unsupported .mnemo format version 9`. Compaction keeps every
  live memory and the result cache, drops past snapshots (as before), and
  stores vectors as fp16 (see 9b.1).
- `mnemo compact --format v8` (Rust `Mnemo::compact_file_to(path,
  passphrase, 8)`) compacts and keeps v8, for one more release. v0.12
  makes v8 read-only.
- A file newer than v9 now reports `unsupported .mnemo format version N:
  this build opens v4 to v9; a newer file needs a newer mnemo-engine`
  (the old message said "supports v1").
- `Stats::format_version` (Python `stats()["format_version"]`) reports a
  file's format.
- The `MNEMO_DEV_FORMAT` development switch is gone.
  `MnemoConfig::format_version` (hidden) stays: `0` is v9, and `8` makes
  a v8 file.

- **9a skeleton.**
  - A v9 header that fits in 512 bytes. The seal now covers every byte of
    it, including the dimensions, flags, KDF parameters and salt.
  - A quarantined free list, so v9 files reuse the pages of deleted and
    overwritten records and of superseded catalog, index, manifest,
    cache-directory and WAL runs. A page is reused only once the
    transaction that freed it is durable and no retained snapshot is older
    than it.
  - The writing transaction's generation is now bound into every v9 page's
    AAD, so putting back a reused page's old ciphertext fails
    authentication.
  - A key-slot run for Phase 13.2.
  - Each writable session takes one small header-only commit before its
    first v9 write.
- **9b.1: the v9 catalog row and the fp16 vector column.**
  - v9 records no longer carry their vector. Vectors live in a column of
    fp16 entries (10 per page at 384 dims), which stays resident while it
    fits `Mnemo::set_vector_cache_budget` (default 96 MiB) and is read
    through the page cache above that.
  - Catalog rows carry the kind, flags, type, thread, key and agent hashes,
    `created_at` and `importance`. Recall scores candidates from the rows
    and the column, and decrypts only the hits it returns.
  - `get()` and `recall()` return fp16-rounded vectors (about 1e-3
    relative error) unless the file is created with
    `MnemoConfig::store_float_vectors`.
  - v9 files under `V9_ANN_MIN_FACTS` (20K) facts use an exact scan and
    build no ANN index.
- **9b.2: packed record pages.**
  - v9 records share pages: a slot directory on each 8 KB page. Records
    over a page own whole pages, as before.
  - Each commit folds pages under 3/4 full into its new pages.
  - Thread turns are chunked at 7,900 bytes on v9, so a chunk fits one
    page.
  - At 100K memories a v9 file takes 1.8 KB per memory (v8: 8.7 KB). Warm
    recall is 2.5 ms (v8: 8.6 ms), and an open handle's RSS is about 112 MB
    with the vector column resident.
- **9c.1: commits.**
  - On v9, page runs longer than two pages (a large catalog or the index)
    are written in place and fsynced before the commit instead of going
    through the WAL, so the WAL stays at 64 KiB.
  - Free runs at the end of the file are truncated once reusable.
  - Snapshot retention defaults to the newest 16 plus any from the last 24
    hours, never pinning more than a quarter of the file (or 1 MiB). An
    explicit `set_max_snapshots(n)` keeps exactly `n`.
  - Restoring past the window names it.
- **9c.2 and 9c.3: deltas.**
  - On v9 a commit writes only the catalog rows it changed, and only the
    ANN index changes it made, as small delta runs. Each chain is folded
    into a new base at 64 runs or a quarter of the base.
  - Deleted rows are dropped from the catalog when a new base is written.
  - The index is never stale after a crash: the every-64-commits rule
    applies to v8 only.
  - At 100K, a durable write takes 15 ms (v8: 27 ms) and the file 1.45 KB
    per memory.
  - Under the default retention, a very small file keeps about its last
    20 commits of history, since the 1 MiB pin floor fills that fast. Set
    `set_max_snapshots(n)` to keep exactly `n`.
- **9d: hybrid recall (Phase 2), v9 files.**
  - `RecallRequest::keywords(..)` adds identifiers or exact terms to match
    (a version, a PR number, a function name). Their BM25 scores, divided
    by the query's best, are added to the similarity with weight
    `keyword_weight` (`DEFAULT_KEYWORD_WEIGHT` 0.5).
  - `RecallRequest::keywords_only(..)` recalls by keyword with no vector
    or embedder.
  - Pass identifiers, not the question: the vector covers its meaning.
  - On ranking_v2's Phase 6 identifier queries: pr_exact 0.2 → 1.0,
    release_exact 0.8 → 1.0, hit@1 0.567 → 0.767. Recall without keywords
    is unchanged.
  - **Surfaces:** CLI `recall --keywords "<terms>"` (`--text` without an
    embedder is keyword-only on v9; with one it stays vector-only), Python
    `recall(..., keywords=, keyword_weight=)`, Node `keywords`, and MCP
    `recall`'s `keywords`.
  - The v9 index run carries a layout byte; a layout this build doesn't
    know is treated as no index.
  - First recall at 100K is about 120 ms, up from about 95 ms in v0.10.
    Partition-major index loading (PERF-2) is planned.
- **9e: compressed thread turns, v9 files.**
  - v9 thread-turn records can be stored zstd-compressed (level 3, before
    encryption) when that saves at least an eighth. Each record is its
    own frame, with no dictionary and nothing shared with any other
    record. Facts, essence items and cache entries aren't compressed, and
    v8 files never are.
  - **Compress when cold, by default.** A turn is written plain and
    compressed once it's older than 7 days (by its `created_at`): each
    commit compresses up to 256 such turns. `compact_file` compresses
    every turn, and so does a vacuum that moves one. A turn already past
    the cutoff when it's ingested (an imported archive) is compressed at
    write. `Mnemo::set_turn_compress_after(secs)` (Python
    `set_turn_compress_after`) changes the cutoff for one handle; `0`
    compresses every turn at write.
  - **zstd is now a required dependency** (`zstd` 0.13.4, the engine's
    first C dependency, through `zstd-sys`; about 0.46 MB of stripped
    library size). CI cross-builds the library for `aarch64-apple-ios`
    and `aarch64-linux-android`. The MSRV stays 1.75: `Cargo.lock` pins
    `jobserver` 0.1.34, since 0.1.35 needs Rust 1.85. A project that
    depends on `mnemo-engine` and builds with Rust 1.75–1.83 should run
    `cargo update -p jobserver --precise 0.1.34` once. Cargo before 1.84
    doesn't pick dependency versions by `rust-version`.
  - **Faster first thread read.** A fresh handle no longer decodes every
    turn in the file to build its caches. Turns are decoded per thread
    the first time that thread is used, and `about()` and essence never
    decode turns. On 10K facts plus 50K turns, the first thread read on a
    fresh handle drops from about 390 ms to about 16 ms.
  - **On that thread-heavy file** (`examples/thread_profile.rs`):
    compacted, it's 33% smaller (280 → 188 MB, 5.6 → 3.8 KB per turn).
    Recent turns read and ingest at the uncompressed speed. Reading back
    a compressed 500-turn thread (about 1.7 MB of text) takes about 6.8 ms
    instead of about 4.4 ms. Fact-only files don't change.
  - A compressed record must state its decompressed size, at most
    64 MiB; anything else is refused as malformed.
  - A file that isn't written to again keeps its aged turns plain until
    its next write or compaction (only writing commits do this work, so
    idle flushes stay no-ops).
- **9f: v9 becomes the default** (see "Upgrading" above).
  - **v9 index defaults:** an unpinned recall on v9 reranks at least
    `V9_RERANK_PER_HIT` (50) × `top_k` candidates (500 for `top_k` 10),
    or the index's own auto depth where that's deeper (above 100K). PQ
    subspaces stay at `dims / 4` (96 at 384 dims), the default since
    v0.6. v8 is unchanged. The v9 index is trained on fp16 vectors; at
    the shipped default cells, recall@10 against v8 is 0.994 vs 0.932
    at 20K, 0.974 vs 0.966 at 50K and 0.978 vs 0.982 at 100K, at 1.0 /
    1.9 / 3.5 ms per query (v8: 1.6 / 3.7 / 7.4 ms).
  - **Fixed: v9 commits no longer rescan the file for keyword search.**
    Each write used to rebuild the BM25 document stats from every row and
    reload every segment's dictionary at the next commit, which cost about
    9 ms per commit at 100K memories. Both are now kept current
    incrementally. A durable single write at 100K is about 16.5 ms
    (it was about 23 ms during FMT-9 development; v0.10 on v8: about
    28 ms). A batch of 100 is about 31 ms.
  - The first recall on a fresh handle at 10K memories (no ANN index on
    v9 below 20K) is about 9 ms on the M1 Pro, down from about 11 ms in
    v0.10. Most of it is filling the in-memory vector column the first
    time (decrypting its pages); making that lazier is planned (PERF-2).
  - Fixed: after a v9 index base in a layout this build doesn't know
    loaded as "no index", `build_index` and `drop_index` now also free
    its delta runs (they were left unreferenced until compaction).
  - Python: `recall_text(..., keywords=, keyword_weight=)` (hybrid recall
    from text, as in `recall`) and `set_vector_cache_budget(bytes)`.
  - The DUR-1, SEC-1, FIX-1 and integration suites run on both formats,
    and CI opens, writes and compacts a file built by the published
    v0.10.0 CLI.

### Fixed

- `mnemo demo` failed with `Locked` on its reopen step (since v0.4.0): it
  now drops the handle before reopening. `close()`'s docs note that the
  file and its lock are held until the handle is dropped (R-005.1).

## [0.10.0] — 2026-11-07

PERF-1, the first release of the on-device track: size and speed, with no
file format change. v0.10.0 files open in v0.9.x and earlier v8 readers.
Measured with `examples/device_budget.rs` on an Apple M1 Pro (384-dim
vectors, ~300 bytes of text, IVF+PQ index); full numbers in
`test/scripts/perf_v0.5.0.json` (`device_budget_v0_10_0` against the P-0
`device_budget` baseline, marked `remeasured`).

| at 100K memories | v0.9.1 | v0.10.0 |
|---|---|---|
| Open (fast KDF) | 384 ms | **9 ms** |
| Open with a raw key | — | **9 ms** (vs 23–25 ms with the secure passphrase KDF) |
| Single durable write | 449 ms | **26–28 ms** |
| File growth per single write | 20.8 MB | **3.4 MB** |
| Warm recall (median) | 23–27 ms | **8.4–8.8 ms** |
| RSS of one handle after 1,000 recalls | 115–158 MB | **34–54 MB** |

Recall@10 (`ann_recall_grid_v2`, every cell) and ranking_v2 hit@1 are
unchanged.

### Changed

- **Opening no longer reads the WAL region.** Recovery checks the first
  frame header and stops when its transaction is already checkpointed;
  before, every open read, checksummed and copied the whole region (26 MB
  at 100K) to discard it. The CRC-32 is now table-driven (slice-by-8),
  bit-identical to the old one, which also speeds every commit.
- **The ANN index is persisted every 64 commits or 1,000 changed vectors,**
  and on `close()`, the new `Mnemo::flush_index()`, a build/rebuild/drop,
  and snapshot restore, instead of on every flush. A plain flush rewrites
  only the catalog. Records written since the persisted index (found by
  page order, so updates count too) are re-assigned when it's next loaded,
  and deleted ids are pruned. `INDEX_PERSIST_EVERY_COMMITS` and
  `INDEX_PERSIST_EVERY_CHANGES` are exported. The MCP server closes the
  file on stdin EOF, so the index is persisted then.
- **The ANN index loads on first use** (the first recall, `stats()`, or a
  flush once 1,000 records are newer than it), not at open. `has_index()`
  is true for a persisted, unloaded index.
- **Turns, essence items and thread links stay out of the ANN index**
  (R-004.2). `build_index` indexes facts only, and an index persisted by an
  earlier version is pruned on load. A recall whose filter pins
  `_mnemo.thread` scans that thread's records exactly; one restricted to
  those kinds skips the index.
- **One lazy handle scan** builds the key-slot, superseded, essence,
  onboarding and thread caches, which `put` and `delete` then keep current.
  Warm `about()` + `about_essence()` + `essence()` take 0.2 ms at 100K, and
  ingesting one turn takes 0.008 ms with no full scan (it was two full
  scans per call). The first such call on a fresh handle still decrypts
  every record once (≈ 1 s at 100K); `set_embedder` triggers it, because
  it reads the manifest through `about()`.
- **Smaller in-memory maps:** the id → catalog-row map is a 4-byte-per-slot
  table and the ANN assignment a compact `u128 → u32` map (8.6 → 3.1 MB at
  100K).
- **The page cache default is 1024 pages (8 MiB), down from 8192 (64
  MiB).** At 10K and 100K memories the old cache was most of a handle's RSS.
  Workloads that rerank many candidates over a file that fits in memory can
  raise it with `set_cache_capacity` (at 10K, the 500-rerank grid cells
  take about 4.8 ms instead of 2.7 ms at the new default).
- **The PQ shortlist scan** sums a flat distance table with four unrolled
  accumulators (the default 100K grid cell: 10.65 → 6.97 ms).
- **`delete` and `update` keep the 12.1 key caches** instead of dropping
  them.

### Added

- **Raw-key databases (R-004.5):** `Mnemo::create_with_key`,
  `open_with_key`, `open_read_only_with_key`, `rekey_with_key`,
  `compact_file_with_key` and `generate_key`, for 32-byte keys held by an
  OS keychain. The header records the Argon2id floor (`KdfParams::raw_key`,
  8 KiB, one pass), so opening skips the KDF cost. Python:
  `mnemo.open_with_key(path, key, dimensions=None, read_only=False)`,
  `mnemo.generate_key()` and `db.rekey_with_key(key)`. `KdfParams` derives
  `PartialEq` and `Eq`.
- **`examples/device_budget.rs`** (P-0): disk per memory by part, RSS of one
  handle, open, first and warm recall, durable writes and their file
  growth, the `about`/essence and ingest caches, and raw-key open, at
  1K/10K/100K. Each phase runs in its own process; `--only` and `--sizes`
  pick a subset.
- **`.github/workflows/size.yml`:** a manual job reporting the stripped
  Python extension and CLI sizes on arm64 Linux and macOS.
- `ann_recall_grid_v2` honours `ANN_GRID_OUT=path` to write its block to a
  scratch file.

### Fixed

- **CLI `recall` lock fallback (R-004.4):** the "running read-only" notice
  printed even when the read-only open then failed. It's printed only after
  that open succeeds.

## [0.9.1] — 2026-11-07

PR 12.7d: the gaps left in Phase 12.7, found in the v0.9.0 catch-up and
verification. No file format change.

### Changed

- **`commit_deltas` embeds.** Facts may now omit `vector`: every fact
  `content`, every essence `Add` / `AppendTimeline` text and every
  archive turn in a batch is embedded with the registered embedder in
  **one** `Embedder::embed` call, before the vectors are checked. This
  makes the MCP `commit_deltas` tool usable from a model, which can't
  produce vectors. In Rust, a fact with an empty `vector` means "embed
  `content`". With no embedder, a text-only fact (or a turn to archive)
  is still an error.
- **Stricter `commit_deltas` gate.** Before anything is written, the
  batch now also checks that every essence `Update` / `Resolve` /
  `Remove` targets a live essence record it can apply to (not a missing
  id, a plain memory, a timeline entry for `Update`, or an id removed
  earlier in the same batch), and that every supplied or embedded vector
  is finite and non-zero. Previously these failed half-way through the
  writes, leaving part of the batch staged.
- **`thread_export` returns the ingest shape.** Python `db.thread_export`
  and the new MCP `thread_export {thread}` tool return
  `[{turn_id, role, content, timestamp}]`, one entry per turn in order
  (a long turn's chunks joined), plus `sha256` and `original_length` on
  a truncated tool result. `db.ingest_thread(t, db.thread_export(t))`
  adds nothing. Rust gains `Mnemo::thread_export_turns` and
  `ExportedTurn`; `Mnemo::thread_export` still returns raw records.
  Python callers of v0.9.0's raw-dict `thread_export` need updating.
- **`about` carries the project essence.** `Mnemo::about_essence()` (and
  Python `db.about_essence()`) returns the project essence render at the
  default budget, or `None`. `mnemo about` prints it after the manifest
  and onboarding memories (`"essence"` in `--format json`), and the MCP
  `about` tool appends it as a final `{"kind": "essence", "render"}`
  element. `about()` itself still returns records.

### Fixed

- **`mnemo thread export` kept the wrong order.** It grouped turns by
  `turn_id` in a sorted map, so the JSONL came out in `turn_id` order
  rather than conversation order. It now uses the shared seq-ordered
  export.
- **`TurnPolicy::ExpireAfter` stamped old turns.** It set the TTL on every
  record whose `turn_id` was in the batch, including turns that were
  already archived and skipped. The TTL is now set only on the records
  the batch writes.

### Tests and benches

- An atomicity property test for `commit_deltas` (random valid and
  invalid batches, plus write failpoints: a rejected batch stages
  nothing, a torn flush lands on the pre or post state), an end-to-end
  MCP test (essence ops, a text-only `commit_deltas` through an
  `--embed-cmd` stub, `thread_ingest`, `thread_start`, the new thread's
  render, `thread_export`, `about`), a one-embed-call-per-batch check,
  and export round-trip and `about_essence` tests in Rust and Python.
- New `benches/thread.rs`, recorded as `remeasured` in
  `test/scripts/perf_v0.5.0.json` (`speed_v0_9_1`), M1 Pro, canned
  in-process embedder: ingest **≈31,000 turns/s** (1,000 turns in 32 ms),
  essence render over 1,000 items **6.3 ms**, `about()` +
  `about_essence()` with 1,000 project items **6.1 ms**.

### Not published

- **`mnemo-node` stays unpublished** (0.9.0 and 0.9.1). The Node binding
  waits for Phase 5 (npm).

## [0.9.0] — 2026-11-07

Phase 12.7 ships: thread archive (12.7a), essence + status-record core
(12.7b), continuation + atomic batch write (12.7c). No on-disk format
bump — v0.9.0 files still open under v0.7.x and v0.8.x readers; new
metadata lives entirely in the reserved `_mnemo.*` namespace.

### Fixed — default recall excludes every thread layer

Default recall now skips `_mnemo.kind = "essence"` and
`"thread_link"` records as well as `"turn"`. Essence items are read by
scope through the render, and link markers carry a sentinel vector
that could surface as junk hits. Opt in as before with any `_mnemo.*`
filter term, for example `_mnemo.kind` equality or `$in`.

### Added — PR 12.7c: continuation + commit_deltas

**New thread, same experience.** Phase 12.7c makes two things routine
for long-running agents: starting a successor thread that still
remembers where the prior one left off, and shipping a session-end
burst of updates in one atomic call.

**Continuation (`thread_start --from`).** `Mnemo::thread_start(new,
from)` writes a `_mnemo.kind = "thread_link"` marker record carrying
`_mnemo.continues = <from>`. A subsequent `essence(scope =
Thread(new))` discovers the link, pulls the prior thread's open
items, its `next` section and the last ten timeline entries, and
renders them ahead of the new thread's own essence under a
`## continued from \`<from>\`` header. No format bump: the marker
lives in the reserved `_mnemo.*` metadata namespace, so v0.9.0 files
open unchanged under v0.7.x and v0.8.x readers.

**`commit_deltas` (atomic batch write).** `Mnemo::commit_deltas(batch)`
takes a `CommitBatch { facts, essence_ops, turns, thread_id,
turn_policy }` and applies all three in one call with a strict
validate-everything-before-staging gate: a single dim-mismatched
vector, oversized essence item, or turns-without-thread-id fails
the whole batch with no record written. On success the apply order
is facts (12.1 keyed or plain) → essence ops (12.7b) → turns (12.7a
archive), followed by a single `flush()`. `TurnPolicy ∈ {Archive,
ExpireAfter(ttl_secs), Drop}` decides what happens to the batch's
turns.

**Surfaces:**
- **Rust:** `CommitBatch`, `CommitReport`, `EssenceOp`,
  `EssenceOpKind`, `KeyedMemory`, `TurnPolicy`,
  `CONTINUATION_TIMELINE_ENTRIES`, `MNEMO_META_CONTINUES`,
  `MNEMO_META_KIND_THREAD_LINK` all exported from the crate root.
- **CLI:** `mnemo thread start <file> --new <id> --from <id>` writes
  the link record; `mnemo essence <file> --thread <id>` then pulls
  the continuation prelude automatically.
- **MCP:** `thread_start {new, from}` → `{id}` and
  `commit_deltas {facts?, essence_ops?, turns?, thread_id?,
  turn_policy?, turn_policy_ttl?}` → counts summary. Facts must
  carry `vector` fields (there is no server-side embedder for
  batch writes).
- **Python:** `db.thread_start(new, from_)` returns the link-record
  ULID; `db.commit_deltas(batch_dict)` returns a counts dict
  mirroring `CommitReport`.

**Tests.** New integration tests cover the link-record marker shape,
the continuation render (previous `open`, `next`, last-N timeline
appearing alongside the new thread's items), the validation gate
(a dim-mismatched fact fails the whole batch atomically), the
happy-path end-to-end (facts + essence ops + turns in one call,
with the essence item rendered and the turn archived), the `Drop`
policy discarding turns, and the thread-id-required guard.

### Added — PR 12.7b: status-record core + essence (timeline + sections + render)

**Essence is the continuous flow of a working thread** — intent,
conventions, decisions, open problems, next step, lessons learned
— plus an append-only milestone timeline. Lives in the same file as
memories and thread-archive turns, under the `_mnemo.kind =
"essence"` reserved metadata kind. The render reuses the Phase 12.8
budget code so a thread's essence loads into a fresh session's
context deterministically and within a byte cap.

**New Rust API:**
- `EssenceSection::{Intent, Conventions, Decisions, State, Open, Next,
  Lessons, Timeline}` + `EssenceScope::{Thread(id), Project}` +
  `essence_status::{OPEN, RESOLVED}`.
- `Mnemo::essence_add(scope, section, text, refs)` — adds a section
  item at status `open`. ≤ 280 bytes; rejected (not truncated) over.
- `Mnemo::essence_append_timeline(scope, text, refs)` — append-only
  milestone entry. ≤ 600 bytes; rejected over.
- `Mnemo::essence_update(id, text, refs)` — in-place text edit for a
  section item; timeline entries reject this with "append-only".
- `Mnemo::essence_resolve(id)` — `open → resolved` transition. Writes
  a new `_mnemo.status_history` entry. Idempotent.
- `Mnemo::essence_remove(id)` — soft-delete (compact reclaims space).
- `Mnemo::essence(scope, max_bytes)` — deterministic markdown render.
  Fixed section order (intent → conventions → decisions → state →
  open → next → lessons → timeline). Each section gets a budget
  proportional to its position; timeline folds its tail into
  `+N earlier milestones` when the budget runs out. Ends with an
  archive pointer (`mnemo thread export …` / `recall --thread …`).

**Status-record core (reused by Phase 13 for tasks + findings).** The
new `_mnemo.status` + `_mnemo.status_history` keys ride with any
record kind that has a lifecycle. v0.9.0 ships the essence
transition table (`open → resolved`) and declares the task
(`queued → active → ready → done | blocked`) and finding
(`open → {accepted | deferred | rejected | needs-user} → verified`)
tables as `TASK_TRANSITIONS` / `FINDING_TRANSITIONS` constants —
Phase 13 uses them unchanged.

**Surfaces:**
- **CLI:** `mnemo essence <file>` with subcommands
  `render [--thread X | --project] [--budget N]`,
  `add --section S --text T [--thread X | --project] [--refs r1,r2]`,
  `append --text T [--thread X | --project]`,
  `update ID --text T`, `resolve ID`, `remove ID`.
- **MCP:** ONE `essence {op, thread?, project?, section?, id?, text?,
  refs?, budget?}` tool, with `op ∈ {add, append, update, resolve,
  remove, render}`. Tool description tells the model when to call it
  ("when the goal, a decision, an open problem, the next step or the
  user's way of working changes, and append a timeline entry at each
  milestone"). Single tool keeps client tool lists small.
- **Python:** `db.essence_add(section, text, …)`,
  `db.essence_append(text, …)`, `db.essence_update(id, text, …)`,
  `db.essence_resolve(id)`, `db.essence_remove(id)`,
  `db.essence(thread=..., project=..., budget=6144)`.

**Tests.** 7 new integration tests cover the add + render round-trip,
timeline-cap rejection at 601 bytes, section-cap rejection at 281
bytes, timeline update is an error, `open → resolved` transitions
status + appends status_history, render budget enforces fold-tail
behavior, and project vs thread scope isolation.

### Added — PR 12.7a: thread archive (ingest / export / forget)

**The backup layer for long working threads.** Every turn of a
transcript lands as an `Episodic` record with `_mnemo.kind = "turn"`
+ `_mnemo.thread = <id>` + `_mnemo.seq` + `_mnemo.role` +
`_mnemo.turn_id`. The archive is encrypted at rest like any other
record, excluded from default `recall`, and queryable through the
12.6 filter when an agent wants drill-down ("what did we say about
the V2 decision?") or through `recall --thread <id>`.

**New Rust API:**
- `Mnemo::ingest_thread(thread_id, turns)` — batched embed, idempotent
  per `turn_id`. Chunking at 8 KB (`THREAD_TURN_CHUNK_BYTES`) for
  non-tool turns; truncation at 16 KB (`THREAD_TOOL_RESULT_TRUNCATE_BYTES`)
  for `role == "tool"` with `_mnemo.sha256` + `_mnemo.length` capturing
  the original. Returns an `IngestReport { added_records,
  skipped_existing_turns, chunked_turns, truncated_turns }`.
- `Mnemo::thread_export(thread_id) -> Result<Vec<Memory>>` — records
  in `_mnemo.seq` then `_mnemo.part` order.
- `Mnemo::thread_forget(thread_id) -> Result<usize>` — soft-delete
  every `_mnemo.kind = "turn"` record in the thread; compact later
  to reclaim space.
- `TranscriptFormat::{JsonlGeneric, ClaudeCodeJsonl}` + `read_transcript`:
  parse a generic one-object-per-line JSONL shape or Claude Code's
  `~/.claude/projects/<p>/<s>.jsonl` (walks `type: "user"` / `"assistant"`
  messages, flattens `content` blocks — text verbatim, `tool_use` /
  `tool_result` / `image` rendered as `[tool_use name=Bash]` /
  `[tool_result ok] …` / `[image]`; skips unknown `type` lines so new
  upstream types don't break ingest).

**Default recall now excludes turn records.** A caller opts in to
seeing them by adding any `_mnemo.*` term to the 12.6 filter —
equality, `$in`, `$exists`, or a range on `_mnemo.thread` /
`_mnemo.kind` / `_mnemo.seq`. The hot path checks `Filter::
mentions_mnemo_meta()` once per request.

**Surfaces:**
- **CLI:** `mnemo thread ingest <file> --thread <id> --transcript <path>
  [--format jsonl|claude-code-jsonl] --embed-cmd … --embed-name …`,
  `mnemo thread export <file> --thread <id> [-o out.jsonl]`,
  `mnemo thread forget <file> --thread <id>`.
- **MCP:** `thread_ingest {thread, transcript_path, format}` tool,
  returning `{added_records, skipped_existing_turns, chunked_turns,
  truncated_turns}`.
- **Python:** `db.ingest_thread(thread_id, turns)` (turns is
  `list[dict]` with `turn_id` / `role` / `content` / optional
  `timestamp`), `db.thread_export(thread_id) -> list[dict]`,
  `db.thread_forget(thread_id) -> int`.

**Privacy.** Transcripts can carry pasted secrets. Everything is
encrypted at rest; `thread forget <id>` plus `compact_file` is the
documented permanent-removal path. Only synthetic fixtures live in
the test tree — the Claude Code parser is validated against
hand-crafted JSONL that mirrors a real `.jsonl` without carrying
any real transcript content.

**Tests.** Integration suite grows by six: round-trip
(`thread_ingest_then_export_round_trip_for_short_turns`), idempotence
(`thread_ingest_is_idempotent_per_turn_id`), chunking at 20 KB /
reassembly (`thread_ingest_chunks_long_non_tool_turns`), truncation
with sha256 + length (`thread_ingest_truncates_oversized_tool_results`),
default-exclusion + filter opt-in (`default_recall_excludes_turn_records`),
and scope-correct forget (`thread_forget_removes_only_archive_records_for_thread`).
Transcript parser unit tests cover the generic shape, Claude Code
skip-unknown behavior, and tool-use / tool-result flattening.

## [0.8.1] — 2026-11-06

**Patch release.** Fixes one usability bug landed in v0.8.0 and adds
two remeasured bench cells. No format change, no API shape change
except the additive `KeyScope` enum and the new `history_in` /
`shared` arguments.

### Fixed — PR 12.1.1: `history(key)` with default arguments now walks the matching slot

Pre-v0.8.1, `remember(key=K)` with no explicit agent wrote a *private*
record under agent `"default"`, but `history(K)` with no arguments
looked up the *shared* slot and returned an empty chain. The obvious
call silently found nothing. v0.8.1 makes `history`'s default slot
match the keyed `remember`'s default slot — the private slot of the
default agent — so `remember(key=K)` + `history(K)` round-trips
without any extra kwargs. The old shared-slot behavior is still
reachable, now explicitly:

* **Rust:** new `KeyScope::{Private(String), Shared}` + new
  `Mnemo::history_in(key, scope)` for the explicit form.
  `Mnemo::history(key, agent_id)` is preserved as the convenience
  shim and maps `agent_id = None` to `KeyScope::private_default()`
  (= `Private("default")`).
* **Python:** `db.history(key, agent_id=None, shared=False)`. Pass
  `shared=True` for the shared slot. `agent_id=...` together with
  `shared=True` is rejected with `ValueError`.
* **CLI:** `mnemo history <file> --key K [--agent A] [--shared]`.
  `--agent` + `--shared` conflict at parse time. No flags → default
  agent's private slot.
* **MCP:** `history` tool gains a `shared: bool` parameter;
  `shared` + `agent` together returns an `INVALID_PARAMS` error.

New regression tests across every surface:

* Rust: `history_default_slot_matches_remember_default_slot`,
  `history_in_shared_and_private_are_separate_chains` (in
  `mnemo/tests/integration.rs`).
* CLI: `cli_history_defaults_to_private_slot_after_keyed_remember`,
  `cli_history_rejects_agent_with_shared` (in
  `mnemo/tests/cli_smoke.rs`).
* Python: `test_history_default_slot_matches_remember_default_slot`
  (in `mnemo-python/test_mnemo.py`).

### Added — remeasured bench cells (lane A DISPATCH)

- `bench_open_latency_indexed_10k` and
  `bench_open_latency_indexed_100k` in `mnemo/benches/open.rs`:
  open latency on an indexed file at the 10K / 100K tiers. The
  numbers feed the lane-A README edit (Planner-A's handoff
  replaces the "open ~15 ms" line, which was KDF-only, with the
  full open-with-catalog-and-index number).
- The `bench_first_recall_fresh_handle_100k` cell added in v0.8.0
  was removed in v0.8.1. Running it alongside the 1M recall setup
  pushed the bench binary into swap on development machines, so the
  measurement is deferred to a dedicated investigation (PR MEM-1).
  The post-v0.8.0 fix is still covered by
  `fresh_handle_recall_hides_superseded_without_building_key_caches`
  in the integration suite — only the remeasured bench number is
  deferred.

## [0.8.0] — 2026-11-06

**Additive release.** Three PRs ship together, plus two lane-A
recall regressions fixed in the release gate (same commit as the
version bump):

1. **Fresh-handle recall no longer decrypts the whole file.** Pre-fix,
   `recall_budgeted` called `ensure_key_caches()` on every invocation,
   so the first recall on a fresh handle triggered a lazy scan that
   decrypted every live record (~1 s on a 100K-memory file). The lazy
   `superseded` set is now consulted only when a prior keyed op on
   this handle has already built it; the correctness case is covered
   by a slow-path `_mnemo.superseded_by` check on each decrypted
   record. Added `bench_first_recall_fresh_handle_100k` to the recall
   benches to track the post-fix number.
2. **Selective-filter widening uses `effective_defaults()`, not the
   caller's `None`.** Pre-fix, the widen step was gated on
   `req.n_probe.map(|n| n * 2)`, which stayed `None` for the default
   recall path and skipped straight to exact scan on moderately
   selective filters. Recall now widens from
   `IvfPqIndex::effective_defaults()` (the ANN-1.1 auto formulas),
   and falls through to exact scan only if the widened pass still
   can't satisfy `top_k`. New `RecallResponse.used_exact_scan: bool`
   makes the path observable on every call so callers can monitor
   ANN degradation / excess supersession.

The three feature PRs: — all three add *new*
query-time knobs without changing the on-disk format or any
pre-12.1 Rust / Python / CLI / MCP signature. Files written by
v0.8.0 open under v0.7.x; all v0.7.x callers compile and run
unchanged against v0.8.0. Upgrade-path summary:

* **12.1 Edit in place + keyed supersession:** new `update(id,
  patch)`, new `remember(..., key=K)` keyed-upsert path (NOOP on
  identical re-writes), new `history(key, agent)`. Superseded
  records drop out of default `recall` / `about` automatically.
  All new per-memory data lives under the reserved `_mnemo.*`
  metadata namespace, no format bump.
* **12.6 Filtered recall:** new `RecallRequest::filter(Filter)` with
  session + per-metadata-key conditions (`$eq` / `$in` / `$exists`
  / `$gte`+`$lte`). Reuses 12.1's widen-then-exact-scan logic, so
  a selective filter still returns `top_k` correct hits on an
  indexed file.
* **12.8 Budgeted recall:** new `RecallRequest::max_bytes(n)` + new
  `Mnemo::recall_budgeted(req) -> RecallResponse { hits, truncated }`.
  The pre-12.8 `Mnemo::recall(req) -> Vec<RecallResult>` is now a
  thin shim over `recall_budgeted` that discards the flag. Python
  and MCP wrap their responses as `{hits, truncated}` when
  `max_bytes` is set, and keep the bare list / array shape when
  it isn't.

Known limitations tracked for follow-up:

* `history_keep` (per-key retention cap) is named in the 12.1 spec
  and in `MemoryPatch` docs but **not implemented** — supersession
  chains are unbounded in v0.8.0. Scheduled for Phase 12.5 / Phase
  7.
* MMR diversity (`mmr_lambda`) **not shipped** — the ranking_v2
  harness doesn't have an MMR variant plumbed in, so the planner's
  "hit@1 drops by no more than 0.02" gate wasn't exercised. The
  budget loop in `recall_budgeted` is structured so a future PR can
  add MMR re-ranking before truncation.

### Added — PR 12.8: budgeted recall (`max_bytes`)

`RecallRequest::max_bytes(n)` caps the total UTF-8 bytes of the
`content` fields returned by one recall. Hits are dropped in rank
order once the running sum would exceed `n`. The top-1 hit is
**always** returned, even if it alone exceeds the budget — callers
that need a strict cap should also pre-size `top_k`. The budget
counts only `content` bytes so the number is identical across the
Rust / Python / CLI / MCP surfaces (no JSON or MessagePack envelope,
no metadata). English text runs roughly 4 bytes per BPE token.

**New Rust API:** `Mnemo::recall_budgeted(req) -> Result<RecallResponse>`.
`RecallResponse { hits: Vec<RecallResult>, truncated: bool }` — the
`truncated` flag is `true` iff the budget dropped at least one
hit. The pre-12.8 `Mnemo::recall(req) -> Result<Vec<RecallResult>>`
is now a thin wrapper that discards the flag, so every pre-12.8
caller stays source-compatible.

**Surfaces:**
* **Python:** `db.recall(..., max_bytes=N)` and
  `db.recall_text(..., max_bytes=N)`. Without `max_bytes` the return
  value is still a `list[dict]` (unchanged); with `max_bytes` it
  wraps as `{"hits": [...], "truncated": bool}`.
* **CLI:** `mnemo recall ... --max-bytes N`. The hit stdout format
  is unchanged; a trailing `"(truncated: hits dropped by
  --max-bytes; rerun with a larger budget or smaller --top-k to see
  more)"` line goes to stderr whenever the budget cut anything.
* **MCP:** `recall` tool gains `max_bytes` (positive integer). When
  set, the response is `{hits, truncated}` instead of the bare JSON
  array of hits, mirroring the Python shape.

**MMR (`mmr_lambda`) — deferred.** The DISPATCH gated the optional
MMR diversity knob on a ranking_v2 harness check ("hit@1 drops by no
more than 0.02"). The harness isn't wired for an MMR variant yet,
so I'm leaving MMR out of v0.8.0 per the planner's instruction
("otherwise leave it out and say so"). The budget code is already
structured so a future `recall_budgeted` can re-rank hits with an
MMR pass before truncating — the API knob can land in a later PR
after the harness run.

The essence render (12.7) is specified to reuse this same budget
code — it will call `recall_budgeted` directly or re-use the
`max_bytes` plumbing. No cross-PR coupling in v0.8.0.

**Tests.** 86 → 91 integration tests pass (5 new):
`budgeted_recall_without_max_bytes_matches_plain_recall`,
`budgeted_recall_drops_hits_past_budget`,
`budgeted_recall_always_returns_top_hit_even_over_budget`,
`budgeted_recall_truncated_false_when_everything_fits`,
`recall_wrapper_still_returns_hits_vec` (checks the shim keeps
pre-12.8 callers source-compatible).

### Added — PR 12.6: filtered recall

`RecallRequest::filter(Filter)` restricts candidates before scoring.
A `Filter` combines an optional `session_id` with a conjunction of
per-metadata-key terms:

* `Exists(key)` — `metadata[key]` must be present
* `Eq(key, value)` — strict JSON equality (numbers compare as f64 so
  `5` and `5.0` match)
* `In(key, [v1, v2, …])` — `metadata[key]` must be one of the listed
  values
* `Range { key, min, max }` — `metadata[key]` must be a number in
  `[min, max]` (either bound optional)

Supersession + 12.6 compose cleanly: a superseded record never survives
to the filter stage, and a filter-starved ANN shortlist reuses Phase
12.1's widen-then-exact-scan path so a selective filter still returns
`top_k` correct hits on an indexed file.

Surfaces across every binding:

* **Rust:** `Filter` / `FilterTerm` with a builder (`.metadata_eq`,
  `.metadata_in`, `.metadata_exists`, `.metadata_range`,
  `.with_session`).
* **Python:** `db.recall(..., filter={...})` and
  `db.recall_text(..., filter={...})` accept a MongoDB-shaped dict:
  ```python
  filter={
      "session_id": "sess-1",
      "metadata": {
          "area": "onboarding",
          "tag":  {"$in": ["a", "b"]},
          "notes": {"$exists": True},
          "score": {"$gte": 5, "$lte": 10},
      },
  }
  ```
* **CLI:** a repeatable `--filter` flag on `mnemo recall` with four
  shorthand forms (`key=value`, `key:exists`, `key:in=a,b,c`,
  `key:gte=NUM`, `key:lte=NUM`); plain values are parsed as JSON when
  they look like JSON, otherwise treated as strings. `--session` is a
  convenience for the `session_id` filter.
* **MCP:** a new `filter` object on the `recall` tool, same shape as
  the Python kwarg.

12.7 relies on this to select `_mnemo.kind` / `_mnemo.thread` on the
thread archive; nothing in v0.8.0 depends on it yet.

**Tests.** 80 → 86 integration tests pass (6 new:
`filter_metadata_equality_restricts_candidates`,
`filter_in_matches_any_listed_value`,
`filter_range_matches_inclusive_bounds`,
`filter_exists_requires_key_presence`,
`filter_session_id_only_returns_matching_memories`,
`filter_selective_still_returns_top_k_on_indexed_file` — the last one
runs a 4%-selective filter against a 1 000-memory indexed corpus and
asserts the result still carries `top_k=10` correct hits via the
widen-then-exact-scan fix).

### Added — PR 12.1: edit in place + keyed supersession

**Two new write verbs** on top of `remember`:

* **`Mnemo::update(id, patch)`** — edit a memory in place, preserving its
  ULID, `created_at` and access stats. A patch whose fields all equal the
  live record is a no-op; otherwise fields change and a `_mnemo.updated_at`
  stamp is added. A `content` change requires a matching `vector` (or use
  `update_text`, which re-embeds through the registered embedder) so text
  and embedding can't drift silently apart. `MemoryPatch` has optional
  `content`, `vector`, `importance`, `memory_type`, `metadata` (full
  replacement of user-visible metadata; reserved `_mnemo.*` keys are
  preserved), and `ttl_secs: Option<Option<i64>>` (outer `None` =
  unchanged, `Some(Some(s))` = set, `Some(None)` = clear).
* **`Mnemo::remember_keyed(memory, key)`** — upsert by `(scope_owner, key)`
  slot. `Private`-scope memories share a slot per `(agent_id, key)`;
  `Shared` memories share one slot per `key` across the file. The first
  keyed write to a slot is `Added`, a later write with different user
  content is `Superseded` (new record inserted, old record marked with
  `_mnemo.superseded_by` / `_mnemo.invalid_at`, both survive through
  `Mnemo::history(key, agent)`), and an identical re-write is `Unchanged`
  — returns the existing live ULID without writing. Retries, idempotent
  extractors, and session-end summaries that re-emit the same current
  fact no longer grow the supersession chain.

**Recall and about drop superseded records.** Before the first decrypt,
every recall checks a lazy in-memory `superseded` set (built once per
handle on first keyed op) and skips those IDs. `about()` filters the
same way. Superseded records remain in `memories()`, in snapshot history,
and in `history(key, agent)`.

**Short results on the indexed path, fixed.** Pre-12.1, filters (type,
agent, TTL, and now supersession) applied *after* the ANN shortlist
could return fewer than `top_k` hits on an indexed file. Recall now
widens `n_probe` / `n_rerank` 2× when that happens, and if the widened
shortlist would already cover half or more of the live non-superseded
records, it skips the ANN pass for an exact scan. This fix is
independent of supersession and also affects pre-existing type / agent
filtering.

**Surfaces** across Rust, Python, the CLI, and MCP:
* **Rust:** `Mnemo::update`, `Mnemo::update_text`, `Mnemo::remember_keyed`,
  `Mnemo::remember_keyed_text`, `Mnemo::history`. New public types
  `MemoryPatch`, `WriteOutcome`, and the reserved-key constants
  (`MNEMO_META_KEY`, `MNEMO_META_SUPERSEDES`, `MNEMO_META_SUPERSEDED_BY`,
  `MNEMO_META_INVALID_AT`, `MNEMO_META_UPDATED_AT`, `MNEMO_META_NS`).
* **Python:** `db.update(id, ...)`, `db.update_text(id, new_text, ...)`,
  `db.history(key, agent_id=None)`, plus a `key=` kwarg on
  `remember(...)` and `remember_text(...)`. A keyed write returns a
  `{"id": str, "outcome": "added"|"superseded"|"unchanged"}` dict;
  without `key`, `remember` still returns a bare ULID string.
* **CLI:** `mnemo update <file> <id> [--text ... --embed-cmd ... --embed-name ...] [--importance F] [--metadata '{...}'] [--ttl-secs N | --clear-ttl]`,
  `mnemo history <file> --key K [--agent A] [--format table|json|jsonl]`,
  and a new `--key K` on `mnemo remember`. Keyed `mnemo remember` prints
  `<ulid>\t<outcome>`.
* **MCP:** new `update` and `history` tools, a new optional `key`
  parameter on the `remember` tool. Tool descriptions steer callers
  toward `key` for facts that replace earlier ones. The `remember`
  response is now `{id, outcome}` with `outcome` =
  `inserted` (un-keyed) or `added`/`superseded`/`unchanged` (keyed).

**No on-disk format change.** All new per-memory data lives in
`Memory.metadata` under the reserved `_mnemo.*` namespace (R-001.10),
so a file written by v0.8.0 still opens under v0.7.x. The v9 format
bump (post-12.1) will promote these into typed catalog columns so the
lazy key-index + superseded-set scan goes away.

**`history_keep`** (retention per key) is named in the spec but
**deferred** to Phase 12.5 / Phase 7 — the supersession chain is
unbounded in v0.8.0.

**Dogfood.** `test/scripts/project_memory.py` gains `remember --key K`:
pass the same key on repeat runs to supersede the prior record, and an
identical re-emission is now a no-op. Scripts that key their writes get
`id\toutcome` on stdout instead of a bare ULID.

**Tests.** 73 → 80 integration tests (7 new: `update_in_place_preserves_id_and_created_at`,
`update_with_identical_patch_is_noop`, `update_content_without_vector_is_rejected`,
`remember_keyed_added_then_superseded_then_noop`,
`recall_drops_superseded_records`, `history_walks_the_supersession_chain_newest_first`,
`private_and_shared_keys_are_independent_slots`,
`file_written_with_12_1_opens_under_v0_7_layout`). No pre-12.1 test
changed behavior.

## [0.7.0] — 2026-11-06

**Breaking release.** Two import-surface changes ship together: the
Rust/Python library name became `mnemo_engine` (NAME-1), and the recall
scoring formula moved to scoring v2 (new scale for raw `score` values).
See each section below for migration notes. The CLI command (`mnemo`),
the `.mnemo` file extension, the on-disk format, and every Rust /
Python API signature are unchanged.

### Breaking — PR NAME-1: import name becomes `mnemo_engine` (no `mnemo` alias)

**The one-line Python migration:**

```python
import mnemo_engine as mnemo   # keeps every call site unchanged
# ...or, as the recommended end state:
import mnemo_engine
```

**Why.** The distribution name on PyPI and crates.io has always
been `mnemo-engine` — the plain `mnemo` name was already taken by
unrelated projects when we first published. The pre-v0.7.0
*library/import* name, though, was `mnemo` in both languages
(`use mnemo::...`, `import mnemo`). On Python that silently
collided with:

- PyPI `mnemo-db` 0.5.30 ("MCP-native memory database for AI
  agents"), an active project targeting the same audience, which
  installs a top-level `mnemo/` module. Installing `mnemo-engine`
  alongside wrote the same `site-packages/mnemo/` — and **pip
  doesn't warn**, so one silently clobbered the other depending on
  install order.
- PyPI `mnemo` 0.0.2, which also installs a `mnemo/` module.

On Rust, crates.io `mnemo` 1.0.0 (a terminal knowledge vault) also
holds the lib name `mnemo`; Cargo reports that clash loudly, but
it's still avoidable.

**v0.7.0 ends the collision by renaming the import name to
`mnemo_engine` and shipping no `mnemo` alias.** We considered a
pure-Python deprecation shim at `python/mnemo/__init__.py` that
would re-export `mnemo_engine` for the v0.7.x series. Dropped: as
long as our wheel wrote a top-level `mnemo/`, it kept clashing with
`mnemo-db` and `mnemo` 0.0.2 — exactly the collision the rename
exists to end. One short edit at every call site (`import
mnemo_engine as mnemo`) is cheaper than one more release of the
silent-clobber surface.

Unchanged on purpose: the CLI command (`mnemo`), the `.mnemo` file
extension, the product name MNemo, the MCP server key in client
configs, manifest metadata, and the on-disk format.

#### Python migration

Pick the form that fits your codebase:

```python
# Drop-in: keep every call site unchanged.
import mnemo_engine as mnemo
db = mnemo.open("agent.mnemo", pp, dimensions=4)

# Direct: the recommended end state.
import mnemo_engine
db = mnemo_engine.open("agent.mnemo", pp, dimensions=4)
```

`import mnemo` raises `ModuleNotFoundError` at v0.7.0 — there is
no shim.

#### Rust migration

```toml
# Direct new name — the recommended end state.
[dependencies]
mnemo-engine = "0.7"
```

```rust
use mnemo_engine::{Mnemo, Memory, MemoryType, RecallRequest};
```

Code that wants to keep `use mnemo::...` unchanged can rename the
dependency locally; this stays supported indefinitely because it's
a Cargo-side local alias, not a global namespace grab:

```toml
[dependencies]
mnemo = { package = "mnemo-engine", version = "0.7" }
```

#### What actually changed on disk

- `mnemo/Cargo.toml`: dropped the `[lib] name = "mnemo"` override.
  The lib now gets Cargo's default from the package name
  (`mnemo-engine` → `mnemo_engine`). `[[bin]] name = "mnemo"` is
  unchanged — the executable users type at the shell is still
  `mnemo`.
- `mnemo-python/Cargo.toml`: `[lib] name = "mnemo"` → `"mnemo_engine"`.
- `mnemo-python/src/lib.rs`: `#[pymodule] fn mnemo` → `fn mnemo_engine`.
- `mnemo-python/pyproject.toml`: switched to maturin's mixed
  Python/Rust layout (`python-source = "python"`, `module-name =
  "mnemo_engine"`). `python/mnemo_engine/__init__.py` re-exports
  the compiled extension; no `python/mnemo/` package (Option A).
- `mnemo/fuzz/Cargo.toml`: dep rename `mnemo` → `mnemo_engine`
  (local alias, not a crates.io change). Fuzz targets now write
  `mnemo_engine::__fuzz::...`.
- `mnemo/bindings/node/Cargo.toml`: same dep rename.
- Every `use mnemo::` in `mnemo/src/bin`, `mnemo/examples`,
  `mnemo/tests`, `mnemo/benches` → `use mnemo_engine::`.
- `test/scripts/project_memory.py` and
  `test/scripts/ranking_harness.py` moved to
  `import mnemo_engine as mnemo` so their call sites stay the
  same and no reference to the old top-level `mnemo` package
  remains.
- `AGENTS.md` + both `README.md` code blocks updated.

### Breaking — PR scoring v2: recall scoring formula + MCP tracking default

**TL;DR.** `db.recall(...)` and `db.recall_text(...)` return the same
set of memories, but each result's raw `score` now sits on a different
scale — anything comparing raw scores to a fixed threshold needs to
re-pick the threshold. Ranking quality on the bake-off corpus is higher
(MRR 0.9487 vs 0.8654 under tracking-off; drift 0 vs 5), the
no-op-but-measurable feedback-loop drift that the prior formula had is
gone, and MCP recall now feeds its own usage back into later queries
(flushed through the next mutating tool call, not per-recall).

#### What the scorer does now

```text
score = similarity
      + max(similarity, 0)
        · ( β · recency
          + γ · (importance − 0.5)
          + δ · tanh(access_count / 5) )

recency = exp( -ln(2) · age_secs / half_life_secs )
age_secs = now - memory.created_at
```

Four deliberate shape changes from pre-v0.7.0 (see the
`ranking_v2` block of `test/scripts/perf_v0.5.0.json` for the full
measurement; the harness is `mnemo/examples/ranking_v2.rs`):

1. **Similarity-gated bonuses.** Recency / importance / usage are
   multiplied by `max(similarity, 0)`, so a stale-but-off-topic memory
   can't ride recency past a semantically-on-point one. Pre-v0.7.0
   scoring was purely additive.
2. **Importance centered at 0.5.** A default-importance memory
   contributes zero importance bonus; importance only pushes a score
   up or down from the reference point (pre-v0.7.0: strictly additive,
   so default-importance memories always got `γ · 0.5` for free).
3. **Saturating usage.** `tanh(access_count / 5)` caps the feedback
   bonus — a heavily-returned memory stops accumulating weight past a
   plateau (~76% of asymptote at 5 uses, ~96% at 10). Pre-v0.7.0:
   `δ · ln(access_count + 1)`, unbounded.
4. **Recency reads `created_at`, not `accessed_at`.** A `recall` no
   longer bumps the recency score of its own results on later queries.
   Pre-v0.7.0 drift (top-1 flips between pass 1 and pass 5 under
   tracking): 5/13 queries on the bake-off corpus. Post-v0.7.0: 0.

#### New score scale (important for threshold-based callers)

The raw `RecallResult.score` sits on the cosine-similarity scale with a
small gated bonus added — for a normalized-embedding recall, typical
top-1 scores land in roughly `[0.5, 1.3]`, where pre-v0.7.0 they
clustered around `[1.0, 1.4]` because every memory got the additive
`β · recency + γ · importance + δ · ln(uses+1)` bonus added on top of
`α · sim` regardless of similarity. **Any caller comparing
`RecallResult.score` to a fixed cutoff must re-pick the cutoff.**
The *ranking* (ordering of results) is still the single thing recall
guarantees; `score` is advisory.

Known failure mode in the shipped scorer that none of the bake-off
candidates fix: on the `ranking_v2` fixture, the query
`"what did BP.1 ship?"` goes to `sup:lib_name:current` (the
"library renamed to `mnemo_engine`" memory) instead of the labeled
`recent:bp1` memory, across V0, V1, V2, V3, V4, V5, and
similarity-only. The rename-current memory has both high similarity
to "ship / release" phrasing and high importance (0.9), and the
BP.1 memory's embedding is slightly further from "what did BP.1
ship?" than the rename memory's. This is a retrieval/embedding
failure, not a scoring failure — no weighted combination of
(similarity, recency, importance, usage) over the shortlist that
cosine returns can fix it. Report only; the sitewide fix is a
better embedder or a hybrid-retrieval shortlist, which is out of
scope for scoring v2.

#### MCP tracking default

Recalls issued through the MCP server (`mnemo serve --mcp`) now
**track access** by default: a recall increments its hits'
`access_count` and updates their `accessed_at`, in memory. Pre-v0.7.0
MCP forced `track_access=false` on every recall. Why the flip: the
feedback-loop drift that made tracking risky before is gone (recency
no longer reads `accessed_at`), and the saturating-tanh usage term
caps how much a heavily-returned memory can accumulate.

Mechanics: the bump happens in-memory only (`accessed_at` /
`access_count` + `dirty_catalog = true`) — **recall itself does not
fsync**. Access stats reach disk on either (a) the next mutating MCP
tool call's exit path (`remember`, `forget`, `cache_put_semantic`,
`cache_get_semantic`-on-miss, etc. each end in `db.flush()`), or (b)
the MCP server's own shutdown flush when stdin closes — so a
read-only session that issued only recalls still persists its stats
on clean shutdown.

#### CLI tracking recall flush (R-001.4)

The default `mnemo recall` (without `--no-track`) always opened the
database read-write to bump access stats, but **never flushed before
exit**, so the mutation lived only in the subprocess's RAM and was
discarded at exit. Under v0.7.0's saturating `tanh(uses/5)` usage
term, that bug would have hidden every CLI recall from the scoring
formula. Both the `--query` / `--query-file` and `--text` branches
now `db.flush()` after a tracking recall; the fallback / `--no-track`
paths skip the flush (read-only handle).

The `--text` branch also gained the same `MnemoError::Locked`
read-only fallback that the vector branches got: when the exclusive
lock isn't available, it opens read-only + sets `track_access=false`
and prints the same one-line stderr note. Test:
`cli_smoke.rs::recall_text_falls_back_to_read_only_when_write_lock_held`.
Persistence regression test:
`cli_smoke.rs::recall_tracking_flushes_access_count_to_disk`.

#### CLI `recall` lock fallback

`mnemo recall <file>` defaults to opening read-write so it can bump
access stats. When that fails because another holder has taken a
shared or exclusive lock (e.g. a second concurrent `mnemo recall`, or
a long-running writer), it now falls back automatically to
`--no-track` read-only mode, prints one line to stderr —

```
mnemo: another writer holds the lock on <path>; running recall read-only with --no-track
```

— and exits successfully with the recall output on stdout.
Pre-v0.7.0: `MnemoError::Locked` and exit non-zero. See the
`cli_smoke.rs::recall_falls_back_to_read_only_when_write_lock_held`
regression test.

#### Python migration

The `recall` / `recall_text` signatures and `track_access` kwarg
default (`True`) are unchanged; the kwarg was already there.

The repo's dogfood runbook at `test/scripts/project_memory.py` was
updated to pass `track_access=False` on its own `cmd_recall` path:
the CLI operator re-queries the same planner/worker coordination
memories many times a day, and the saturation term is informative
only for real agent writes, not for how often the human greps the
memory. Projects using the Python binding should make the same
call about their own `recall` sites: default `True` matches an
agent actually learning from what it retrieves; override to `False`
for developer / tooling reads.

### Fixed — `perf_v0.5.0.json` `speed_v0_6_0` provenance note

Added a `remeasured` field + `remeasured_note` to the
`speed_v0_6_0` block in `test/scripts/perf_v0.5.0.json` listing the
two write benches that were actually re-run for v0.6.1 (the other
cells were copied verbatim from the v0.5.1 `speed_hw_aes` block
because no code touched them between v0.5.1 and v0.6.1, but the
copy was silent — easy to misread as fresh measurement).

## [0.6.1] — 2026-11-05

**Patch release.** Additive API + doc corrections; no behavior change
in any code path that already existed at v0.6.0.

### Added — PR BP.1 (`mnemo index --rebuild` + Python `rebuild_index()`)

The v0.6.0 upgrade note told users to run `mnemo index --rebuild` /
`db.rebuild_index()` so an index built under the pre-ANN-1
`pq_subspaces = dims/8` defaults would pick up the ANN-1.1 shipped
numbers (`pq_subspaces = dims/4` + size-aware `n_probe` / `n_rerank`).
Neither command actually existed:

- **CLI:** `mnemo index` only had `--drop`; `mnemo index <file>`
  without a flag already re-clustered under the current defaults,
  but the docs promised a dedicated `--rebuild` flag.
- **Python:** `db.build_index()` → `build_index_with(IndexConfig::
  default())` worked, but a direct `db.rebuild_index()` call raised
  `AttributeError`.

BP.1 adds the two user-visible commands. Both call the existing core
`Mnemo::rebuild_index` (added in ANN-1.1): re-clusters centroids +
retrains the PQ codebook under the current `IndexConfig::default()`
for the file's live vector count. Shipped behavior:

- **`mnemo index <file> --rebuild`** (CLI) — mutually exclusive with
  `--drop`; rejects a mix of `--rebuild` with explicit tuning knobs
  (`--partitions` / `--subspaces` / `--n-probe` / `--n-rerank`) with
  a clear error that points at the right command.
- **`db.rebuild_index()`** (Python) — releases the GIL during the
  k-means + PQ training (same pattern as `build_index`, same PY-1
  rules). Does not flush; call `db.flush()` to persist.

Three new tests in `mnemo/tests/cli_smoke.rs`
(`cli_index_rebuild_runs_rebuild_index`,
`cli_index_rebuild_rejects_tuning_overrides`,
`cli_index_drop_and_rebuild_are_mutually_exclusive`) plus
`test_rebuild_index_reclusters_under_current_defaults` in
`mnemo-python/test_mnemo.py`.

### Fixed — process house-keeping

- `mnemo/bindings/node/Cargo.toml` was still pinned at `0.5.2`
  (previous releases kept it in step). Bumped to `0.6.1` with the
  rest of the manifests.

### Measured — DUR-1 step 5 write benches

Re-ran `cargo bench --features benches --bench write` on an Apple
M1 Pro release build with hardware AES to close out DUR-1 step 5
(not done at v0.6.0 time). Numbers landed under the new top-level
`speed_v0_6_0` key in `test/scripts/perf_v0.5.0.json`, next to the
pre-DUR-1 `speed_hw_aes` block from v0.5.1 for an apples-to-apples
before/after. The DUR-1 CHANGELOG entry already explained the
change (steady-state flush drops from 4 fsyncs to 3 — the leased-
header fsync is gone; relocating flushes add one fsync for the
new `invalidate_region` call but gain crash-safety). The
`fsyncs_last_flush` field on `Stats` reports the per-flush count
at runtime.

**v0.6.0 ships five PRs in one minor bump** — the first is a user-
visible behavior change (ANN defaults), which is why it's a minor
and not a patch. Nothing else changes the API surface. Everything
below under `[0.6.0]` was previously under `[Unreleased]` and is
unchanged in content.

### Changed — PR ANN-1.1 (approximate index: query-time auto defaults, size-aware n_rerank)

Follow-up to PR ANN-1 (same `[Unreleased]` block). Two gaps the ANN-1
VERIFIED entry flagged, now closed:

**1. The ANN-1 fix now reaches v0.5.x-built indexes without a rebuild.**
`IvfPqIndex::query` previously defaulted to the `n_probe` / `n_rerank`
values stamped into the index at build time. Every v0.5.x build stamped
in the then-hardcoded `(8, 64)`, so after upgrading to v0.6.0 those
indexes kept running at ~0.40 recall@10 until a manual rebuild. ANN-1.1
adds a "legacy sentinel" rule inside `IvfPqIndex::effective_defaults`:
a stored `(n_probe, n_rerank)` of exactly `(8, 64)` is treated as "auto"
and both dials are recomputed from the live partition count + vector
count on every query. Any explicit `.n_probe()` / `.n_rerank()` passed on
the `RecallRequest` still wins. (Users who built a v0.6.0 index with an
explicit `IndexConfig { n_probe: 8, n_rerank: 64, .. }` get the sentinel
treatment too — acceptable because at the one regime where 8/64 is a
sensible pin, the auto formulas pick nearly-identical values.)

**`IvfPqIndex` keeps its exact v0.5.2 serialized shape** — no appended
fields, no format bump. The planner's R-001.10 standing rule
(2026-11-01) made this a hard requirement: rmp-serde 1.3.0's
positional decoder rejects extra array elements with
`Error::LengthMismatch`, so an appended `#[serde(default)]` field would
make v0.5.x readers fail to open any file produced by v0.6.0 — breaking
the "portable file across versions" guarantee. The sentinel approach
reaches the same outcome without touching the schema. Guarded by a new
test (`current_build_serializes_with_v0_5_2_field_count`) that catches
any future append at CI time.

**2. `n_rerank` scales with the live vector count.** Previously fixed
at 500 after ANN-1, which on 10K files cost ~5 ms of record-decode
time per query for no recall gain (rerank cost ≈ 10 µs per candidate
on hardware AES; a 10K index with ~98% recall needs only ~100
candidates). The new formula is `(n_vectors / 200).clamp(100, 1000)`:

| Corpus | Old n_rerank (ANN-1) | New n_rerank (ANN-1.1) | Rationale |
|--------|----------------------|------------------------|-----------|
| 10K    | 500                  | **100**                | Grid knee at 10K was n_rerank=100 at 0.964 recall |
| 100K   | 500                  | **500**                | Grid knee at 100K was n_rerank=500 at 0.982 recall |
| 1M     | 500                  | **1000** (cap)         | Linear extrapolation + clamp; measured in `ann_defaults_v0_6_0` where feasible |

`n_probe` auto formula was tightened from ANN-1's `ceil(partitions / 2)`
to `max(ceil(partitions / 2), min(partitions, 100))` after the ANN-1.1
measurements showed the half-probe formula undershoot at 10K (0.914
recall@10 vs the 0.95 target; the ANN-1 grid knee was at full probe).
Full probe for files up to ~10K vectors (100 partitions), half probe
above:
- 10K  ≈ 100 partitions  → 100 (full)
- 100K ≈ 317 partitions  → 159 (half)
- 1M   ≈ 1000 partitions → 500 (half)
Also recomputed per query so growth takes effect automatically.

### Added

- `IvfPqIndex::effective_defaults()` returns the live `(n_probe, n_rerank)`
  pair that `query()` uses when the caller passes `None` — applies the
  legacy sentinel rule above.
- `index::effective_auto_n_probe(partitions)` and
  `effective_auto_n_rerank(n_vectors)` as `pub(crate)` helpers so the
  build + query paths share one source of truth.
- Five tests in `mnemo/src/index.rs::ann_1_1_tests` using a frozen
  `V052IvfPqIndex` shadow struct:
  (1) a v0.5.2-shaped blob decodes under the current schema;
  (2) **the current build's serialized index decodes under the v0.5.2
      shape** — forward-compat guard against any accidental field
      append (R-001.10);
  (3) stored `(8, 64)` triggers auto formulas at query time;
  (4) new builds with `IndexConfig::default()` store **resolved**
      concrete values (never 0 sentinels);
  (5) explicit non-sentinel values persist verbatim.

### Changed — API cleanup

- **CLI `mnemo index` `--n-probe` / `--n-rerank` default to `0` (auto),
  matching `--partitions` / `--subspaces`.** Previously defaulted to
  `8` / `64` — so `mnemo index` built an index baked to the small
  hardcoded dials even on huge files. Users who want to pin can still
  pass explicit values.
- **`Mnemo::rebuild_index`** now passes `IndexConfig::default()` instead
  of resolving the current index's `(n_probe, n_rerank)` and feeding
  them back as explicit values. Rebuild means "re-tune for the current
  corpus size" — freezing values from the pre-rebuild state defeated
  the point. Users who want to pin can call
  `build_index_with(IndexConfig { … })` directly.
- **`Mnemo::compact_file`** likewise re-derives all parameters by
  passing `IndexConfig::default()` to the rebuild step on the compacted
  file.
- **`IvfPqIndex::info` / `Stats::index` / `mnemo info` / Python
  `db.stats()['index']`** report the **effective** `n_probe` / `n_rerank`
  queries actually use (after the legacy sentinel is applied), not the
  raw stored values. So an upgraded v0.5.x file shows the larger auto
  values it searches with — not the 8/64 baked into the file by the
  old build.

### Performance

- **IVF+PQ rerank:** `query()` now partition-selects to top-`n_rerank`
  with `select_nth_unstable_by` **before** sorting, then sorts only
  the kept slice (R-001.8 part 3). Full sort of all N candidates was
  O(N log N); the new path is O(N) for the select plus O(k log k) for
  the sort of k = n_rerank. At the ANN-1.1 defaults on a 1M file the
  per-query candidate pool is ~500K; the old pipeline sorted every one.

### Numbers at the shipped defaults (M1 Pro, release, hardware AES)

Measured in `test/scripts/perf_v0.5.0.json["ann_defaults_v0_6_0"]`.
Two scenarios per tier: **fresh_install** = `IndexConfig::default()`
build (pq_subspaces=96, auto n_probe + n_rerank); **upgrade_path** =
v0.5.x-built index (pq_subspaces=48, stored 8/64 → ANN-1.1 sentinel
substitutes auto formulas at query time; **no rebuild needed**).

| Tier | Scenario       | n_probe | n_rerank | recall@10 | latency   |
|------|----------------|---------|----------|-----------|-----------|
| 10K  | fresh_install  | 100     | 100      | **0.964** | 1.4 ms    |
| 10K  | upgrade_path   | 100     | 100      | 0.764     | 0.9 ms    |
| 100K | fresh_install  | 159     | 500      | **0.982** | 9.4 ms    |
| 100K | upgrade_path   | 159     | 500      | 0.834     | 7.1 ms    |
| 1M   | fresh_install  | 500     | 1000     | **0.970** | 134 ms    |
| 1M   | upgrade_path   | 500     | 1000     | 0.802     | 97 ms     |

**Fresh install clears the 0.95 target at every tier.** Upgrade path
clears 0.75-0.85 depending on tier — a 2x+ improvement over the
pre-ANN-1 ~0.40 baseline without touching the file. Users who want
the fresh-install recall on an existing file run
`mnemo index --rebuild` / `db.rebuild_index()` to get the ANN-1
pq_subspaces=96 build-time fix as well.

### Upgrade note (for the v0.6.0 release notes)

After upgrading to v0.6.0, your existing `.mnemo` files will:

- Use the new query-time `n_probe` / `n_rerank` auto formulas
  immediately — no rebuild needed. The on-disk index already stores
  everything the formulas need (partition count, live vector count).
- Keep the pre-ANN-1 `pq_subspaces` setting (whatever was baked in at
  build time — usually `dims/8` on v0.5.x files). This is a build-time
  parameter, not query-time, so the ANN-1 pq-subspaces fix (`dims/4`,
  which lifts the 100K ceiling past 0.92) only takes effect for new
  indexes OR after `mnemo index --rebuild` (CLI) /
  `db.rebuild_index()` (Rust / Python). The upgrade-note numbers
  under `ann_defaults_v0_6_0` in `perf_v0.5.0.json` show what an old
  48-subspace index gets with the new query-time defaults vs. a fresh
  96-subspace rebuild.

### Changed — PR ANN-1 (approximate index defaults, from 2026-10-31)

The `IndexConfig::default()` auto-formulas now scale with the index and
query shape instead of fixed small numbers. Measured on the Phase 6
synthetic corpus (M1 Pro, release build, hardware AES):

| Corpus | Old defaults (v0.5.x) | New defaults (ANN-1) | Latency |
|--------|-----------------------|---------------------|---------|
| 10K    | recall@10 ≈ 0.40 (n_probe=8, n_rerank=64)  | **recall@10 ≈ 0.98** (n_probe=50, n_rerank=500) | ~2-3 ms/query |
| 100K   | recall@10 ≈ 0.40 (same)                     | **recall@10 ≈ 0.98** (n_probe=159, n_rerank=500) | ~10 ms/query |

Previously `n_probe=8, n_rerank=64` were fixed numbers tuned for a toy
corpus; at 100K that scanned 2.5% of partitions and reranked 64 of
~100,000 candidates. The old defaults' 0.40 recall was mis-filed in
the v0.5.0 CHANGELOG as a "scoring v2 candidate"; the PR ANN-1
diagnose harness (step 1 of the planner's spec) ruled out any engine
bug — full-scan + rerank-1000 hits 0.98 at 10K and 0.92 at 100K, so
the ceiling is tunable, not broken.

Formulas:
- `pq_subspaces` auto: `dims / 4` (was `dims / 8`). On 384-dim data,
  96 subspaces instead of 48. Costs 2× PQ codes per vector (48 → 96
  bytes, negligible against the 1.5 KB raw vector) and ~15 % more
  build time. Required to lift the 100K ceiling past 0.92.
- `n_probe` auto: `ceil(partitions / 2)` (was fixed `8`).
- `n_rerank` auto: fixed `500` (was fixed `64`). Decoupled from `top_k`
  because the index is built before any query knows `top_k`; 500 is
  50× the default `top_k=10` — generous headroom.

Explicit non-zero values on `IndexConfig` still override, so callers
who want the old tiny defaults can set them explicitly (useful for
very small corpora where sqrt(n) partitions is overkill).

**No format change.** The on-disk `IvfPqIndex` schema is unchanged;
the different `pq_subspaces` just serializes differently. Existing
files keep their old defaults until the user calls `rebuild_index` or
`compact_file`. Any explicit `n_probe` / `n_rerank` passed per call
still overrides, as before.

### Added — Python parity

- **`Mnemo.recall(..., n_probe=None, n_rerank=None)`** and
  **`Mnemo.recall_text(..., n_probe=None, n_rerank=None)`** — per-query
  ANN tuning dials now reach the Python surface. `None` means "use the
  index's build-time defaults" (the ANN-1 auto-formulas above). Rust
  and the CLI already had this via `RecallRequest::n_probe/n_rerank`;
  Python users had no handle on it at all. One Python-side test
  (`test_recall_accepts_n_probe_n_rerank_overrides`).

### Docs — PR ANN-1 cleanup

- `mnemo/README.md`:
  - "Approximate index (IVF + PQ)" gains a "Default auto-formulas"
    paragraph pointing at `ann_recall_grid_v2` in `perf_v0.5.0.json`.
  - v0.1.0 Performance-baseline block gets a note that its CLI timings
    were **debug builds**; v0.5.x re-measured on release.
  - v0.5.0 Performance-history row gets two footnotes: (†) the IVF+PQ
    speed in that row was at the old defaults → recall@10 ≈ 0.40, not
    1.0, and the ANN-1 defaults raise it to 0.98; (‡) exact-scan
    timings were on software AES, fixed in v0.5.1.
  - Stale test counts at build-and-test section updated (34→66
    integration, 12→26 CLI smoke).
- `mnemo-python/README.md`:
  - "Install" now leads with `pip install mnemo-engine` (prebuilt
    wheels for every common platform); the maturin steps move under
    a "From source (contributors)" subsection.
  - `recall()` / `recall_text()` kwargs updated for `n_probe` /
    `n_rerank`.
  - `stats()` row notes `fsyncs_last_flush` (shipped unmentioned in
    v0.5.1).
  - New module-level `mnemo.build_info()` row (shipped unmentioned in
    v0.5.1).
- Root `README.md` and `index.html` stay planner-owned — not touched.

### Methodology

Full ANN-1 numbers (diagnose cells + 2×5×5 tuning grid at 10K and
100K) are under new top-level keys `ann_diagnose` and
`ann_recall_grid_v2` in `test/scripts/perf_v0.5.0.json`, next to the
earlier Phase 6 `speed`, `ranking`, `ann_recall_grid`,
`file_size_breakdown`, and `speed_hw_aes` blocks.

### Fixed — PR PY-1 (Python binding: `close()` releases the file lock; GIL released on heavy calls; closed-aware access; re-entrant embedders)

Scoped by REV-001 R-001.11 + the 2026-11-01 SEC-1 reproduction note
("`close()` doesn't release the OS file lock"). Four independent
improvements to the Python binding, no behavior change in the Rust
core beyond the new `Mnemo::embedder_arc()` accessor.

**1. `db.close()` releases the OS file lock synchronously.** Pre-PY-1
`Mnemo.close()` only called `Core::close` (which flushes but holds
the file handle); the exclusive advisory lock sat until Python's GC
collected the object, so `db.close(); reopen(path)` in the same
process raised `Locked`. Only `del db` released the lock. PY-1
wraps the Rust core in `Option<Core>`; `close()` and `__exit__`
`take()` it and drop — the `File` destructor runs synchronously,
which releases the `flock` / `LockFileEx` in the same call. Every
method now routes through a `core()` / `core_mut()` accessor that
raises `RuntimeError: database is closed` after close, so a
post-close call surfaces a clear error instead of a panic or a
stale read. `close()` is idempotent. The matching pattern applies
to `Session` methods, which borrow `Py<Mnemo>` mutably: they now
inherit the closed-handle error automatically.

**2. Heavy calls release the GIL.** `flush`, `recall`, `recall_text`,
`search`, `build_index`, `verify`, `rekey`, `cache_get_semantic`,
`cache_get_semantic_text`, the module-level `open` (Argon2id KDF)
and `compact_file` are now wrapped in `py.allow_threads`. Another
Python thread can tick during a long recall or a Argon2id-paced
reopen; pre-PY-1 any of these pinned the GIL for the whole call.
`compact_file` now also surfaces `cache_before` / `cache_after` in
its returned dict (matching FIX-1 R-001.1's `CompactReport`).

**3. Embedder callbacks are safe to re-enter the database.** R-001.11:
pre-PY-1 the Python binding borrowed `&Core` across `embedder.embed()`,
so a Python embedder that touched `db` from inside the callback
(e.g. `db.dimensions()` for shape probing, or
`db.cache_get(embed_cache_key)`) hit PyO3's "Already borrowed" at
the next method dispatch. PY-1 adds a new
`Mnemo::embedder_arc(&self) -> Option<Arc<dyn Embedder>>` accessor
on the core; `remember_text`, `recall_text`, and
`cache_get_semantic_text` clone the Arc under a short `&Core`
borrow, drop the borrow, then call `embed()` with no `self` borrow
held — so the callback owns the floor.

**4. `rekey(fast=False)` wire-up unchanged** (FIX-1 already fixed the
params inheritance): now runs under `allow_threads` too, so a
production-grade rekey (~50–100 ms Argon2id) doesn't stall the GIL.

**Tests.** `mnemo-python/test_mnemo.py` gains five new cases:
`test_close_releases_lock_and_blocks_further_use` (close then
reopen same path in-process; method on closed handle raises
`RuntimeError: database is closed`; `close()` is idempotent),
`test_with_block_releases_lock_on_exit` (`__exit__` releases the
lock without `del`), `test_session_methods_fail_cleanly_after_db_close`
(session `add_turn` / `recall` / `close` after `db.close()` →
clean error), `test_embedder_callback_can_reenter_db_without_already_borrowed`
(re-entrant `db.dimensions()` from inside the embedder succeeds),
and `test_background_thread_makes_progress_during_long_flush` (a
Python thread ticks during a `verify()` call — the GIL was
genuinely released). The pre-PY-1 `test_persistence_and_reopen`
test loses its `del db` workaround — proof the lock releases on
`close()` alone.

### Fixed — PR FIX-1 (correctness sweep per REV-001 + REV-002)

Reviewer-triaged under REV-001 (R-001.1, R-001.5, R-001.6, R-001.8
parts 1–4) and REV-002 (R-002.2 parts 1–3). Six independent
correctness gaps closed in one PR:

**1. Compaction preserves the result cache (R-001.1 + R-002.2
part 2).** Pre-FIX-1 `Mnemo::compact_file` copied only live memories
and silently dropped the entire result cache — the Planner's interim
`c417d2e` README caveat called that out. Now every live, non-expired
directory entry is copied verbatim into the new file via a new
`copy_cache_entry_from` helper. `accessed_at` and `access_count`
come from the **directory** (the record body only holds the insert-
time snapshot); `created_at`, `ttl_secs`, `cost_hint_ms`,
`key_preview`, `size_bytes`, `content_type`, `value`, `vector` and
`model` come from the body. Tombstoned and TTL-expired entries drop.
`CompactReport` grows two fields (`cache_before`, `cache_after`).
`Mnemo::verify()` now walks the cache too — pre-FIX-1 it checked only
the memory catalog, so a corrupted cache record slipped by.

**2. Read-only `cache_get` / `cache_get_semantic` return the
directory's `accessed_at` (R-002.2 part 1).** Hits by write handles
update only the directory (v5 "no full record rewrite on a hit"
trick); the record body keeps its insert-time snapshot forever. A
read-only handle used to surface that stale body snapshot, so a
read-only reader saw "last accessed = when it was inserted" even
after many hits by concurrent writers. The new code reads from
`self.cache_dir[pos].accessed_at` in the read-only branch instead.

**3. Safe `Mnemo::export_encrypted(dest)` (R-001.5 + R-002.2
part 3).** Added a core method the Python binding now delegates to.
Refuses when `dest` resolves to the open database (Unix: `(dev, ino)`
via `MetadataExt`, catching symlinks and hard links; Windows:
canonicalized paths) — `File::create` would otherwise `O_TRUNC` the
live file to 0 bytes. Streams from the already-open file handle (so
a Windows `LockFileEx` mandatory lock can't block the copy) into a
sibling temp file, `sync_all`s it, renames onto `dest`, fsyncs the
parent directory on Unix. Pre-FIX-1 the Python path was
`close() + fs::copy()` — which left the handle unusable and happily
truncated the database when `dest == path`.

**4. `Mnemo::kdf_params()` + Python `rekey(fast=False)` preserves
the file's KDF (R-001.5).** `kdf_params()` is a new O(1) accessor
returning the file's current Argon2id params. The Python binding's
`rekey(fast=False)` used to pass `KdfParams::default()` — so a file
created with weaker or stronger parameters silently reset on every
rekey, even though the docstring promised inheritance. Now it passes
`self.inner.kdf_params()`.

**5. `EmbedCmd` rejects all-zero and non-finite replies (R-001.6).**
The module doc already promised "any protocol error (… zero vector …)
kills the child" but the check was missing. A buggy embedder
returning `[0, 0, 0, …]` (common on `sentence-transformers` cold
starts) or `[NaN, …]` would silently poison recall:
`Mnemo::remember` refuses all-zero itself, but `cache_get_semantic`
and `recall --text` consumed the result. Both checks now route
through `fail_and_restart` on the same path as a wrong-width reply.
Two new `tests/cli_smoke.rs` cases drive them.

**6. `stats()` / lazy agent set, dimensions over stats, no restore-
point churn on cache flushes, Batched `max_age` on reads (R-001.8).**
- `stats()` used to call `memories()?.into_iter().map(|m| m.agent_id).collect()`
  — O(N) AES-GCM decrypts just to list a usually-tiny set of distinct
  agent IDs. Now kept in a per-handle lazy `agent_set: Option<
  BTreeSet<String>>` built once on first `stats()` call, maintained
  in O(1) on `put`, invalidated on `delete`. (The format doesn't
  carry `agent_id` on the catalog entry in v8; v9 will, and this
  shim goes away.)
- Python `insert_default_manifest` now uses `self.inner.dimensions()`
  — a thin O(1) accessor — instead of `self.inner.stats().dimensions`
  (which triggered the agent-set walk above).
- `Mnemo::flush` no longer pushes a restore-point snapshot on a
  cache-only flush. Snapshots don't capture cache state, and every
  push counts against `max_snapshots` (default 256) + rewrites the
  manifest run — so a cache-heavy workload previously evicted every
  real memory restore point within ~256 cache writes.
- `cache_get` / `cache_get_semantic` on read-write handles now
  evaluate `BatchState::should_auto_flush` once at entry too, so
  the `Batched` policy's `max_age` fires when a workload switches
  from writes to pure reads (pre-FIX-1 it only fired on
  `cache_put`).

**Tests.** `mnemo/tests/fix1_correctness_sweep.rs` carries 10 new
cases spanning all six items; `mnemo/tests/cli_smoke.rs` gains
`embed_cmd_rejects_all_zero_vector_and_respawns` and
`embed_cmd_rejects_non_finite_vector_and_respawns`.

### Fixed — PR DUR-1 (torn-header recovery after WAL relocation; obsolete pre-flush lease write dropped)

Reviewer-triaged under REV-001 (R-001.2 "wal relocation leaves a
torn-header window the recovery scan cannot heal", R-001.7 "the
pre-flush leased-header write is now obsolete under SEC-1") and
further sharpened by REV-002 (R-002.1): the Phase 1.3 recovery heals
*only* on `HeaderChecksum`, and `Header::from_page` returns
`BadMagic` / `UnsupportedVersion` *before* the CRC gate, so the
realistic failure mode (the 294-byte header sits inside the first
512-byte sector; storage either writes that sector atomically or
leaves it zeroed / garbled) skips healing entirely. Alongside this,
`ensure_wal_capacity` and `rekey` wrote page 0 directly — a torn
write there was unrecoverable because the WAL region was either empty
(relocation path) or stale (rekey path).

**1. Every post-`create` page-0 write now goes through the WAL
first.** Both `ensure_wal_capacity` (relocation) and `rekey` build the
new header, WAL-commit a single-frame transaction `(0, new_header)`
with `txn_id = wal_seq + 1` *into the new region*, checkpoint page 0,
then zero the first frame header of the predecessor region. A crash
anywhere in that sequence leaves either the old state intact (if
page 0 wasn't overwritten) or the new state recoverable from the new
region (if it was). The initial `create` is the only unlogged page-0
write that remains.

**2. The torn-header scan now walks the whole file.** `Mnemo::open`
reads page 0, and on *any* `Header::from_page` error (`BadMagic`,
`UnsupportedVersion`, `HeaderChecksum`), scans every 8 KiB-aligned
page ≥ 1 for `MWAL` magic and tries each as a candidate WAL-region
start. A candidate is accepted only if (a) its chained-CRC commit
verifies, (b) it carries a `(0, header_page)` frame, (c) that
header's `wal_start` equals the candidate page and `wal_seq` equals
the transaction's id, (d) the DEK unwraps under the passphrase, and
(e) the AES-GCM seal validates. The highest-`txn_id` winner replays
to page 0; if nothing is valid the *original* error is returned, so
non-mnemo files still report `BadMagic` cleanly. The pre-DUR-1 fixed
`TORN_HEADER_SCAN_PAGES = 64` cap is gone.

**3. Seal failure is always `HeaderTampered`, never auto-healed**
(Planner decision 2026-11-01, Option B; supersedes the R-002.1 part 3
"seal-mismatch WAL fallback" draft). The entire 294-byte header lies
inside one 512-byte sector that storage writes atomically — a surviving
0..238 CRC with a failing 242..294 AES-GCM tag cannot be a torn write;
only post-write tampering produces that shape. Silently loading an
authenticated committed header frame from the WAL in that case would
erase the tamper-evidence the seal exists to give. The scan in point 2
is therefore gated on *parse* failures only; `header_tamper_is_
detected_by_v7_seal` (the pre-DUR-1 security regression test) stays
unchanged.

**4. `prepare_for_flush`'s leased header write is removed.** It
existed to close the Phase 1.1 nonce-reuse window (reopen must see a
`write_counter` past any nonce this flush could produce). SEC-1
superseded that: nonces now carry a per-session random prefix, and
`write_counter` is no longer a nonce input, so the leased write
bought nothing but an extra `write + fsync` on every flush.

**5. `open_read_only` recognizes torn-header parse failures.** It used
to map only `HeaderChecksum` to `NeedsWriteOpen`; a zeroed or garbled
sector 0 came back as `BadMagic` even when the file-wide scan would
have found a recoverable committed header frame. Now any
parse-level failure is routed to `NeedsWriteOpen` when the scan turns
up a valid candidate, otherwise the original parse error is returned
unchanged. A `validate_seal` failure stays `HeaderTampered` (same
Option B rule).

**Tests.** `mnemo/tests/dur1_torn_header.rs` covers the sector-level
shapes the spec enumerates: sector 0 zeroed, sector 0 random garbage,
bytes 0..6 corrupted, bytes 242..294 stale with CRC intact →
`HeaderTampered`, a sealed field altered with the CRC recomputed →
`HeaderTampered`, a WAL-relocated transaction larger than 64 pages
with page 0 zeroed, and failpoint sweeps across relocation and rekey
(both the just-flushed and never-flushed variants).
`prop_failpoint_relocation_or_rekey_yields_pre_or_post` in
`proptest_store.rs` extends property 4 to the two new WAL-logged
paths — the live set after a torn relocation or rekey must equal the
pre- or post-op snapshot, never a third state, and at least one of
the two passphrases must open the file after a torn rekey.

**Durability math.** `Stats::fsyncs_last_flush` drops from 4 → 3 on a
steady-state flush (data pages, WAL commit, checkpoint; the leased-
header fsync is gone). A relocating flush adds one fsync
(invalidate-old-region) relative to the pre-DUR-1 count but gains
crash-safety. The crypto module's stale "nonces come from
`(page_number, write_counter)`" doc paragraph is replaced with the
SEC-1 description.

### Not released yet

Holding on `main` to ship together with **Recall scoring v2** as
**v0.6.0** per the planner's step-7 call (both PRs change which
results come back; one minor-version bump covers both). No early
v0.5.2 patch — the ANN-1 diagnose did not surface a user-visible bug
(the pipeline is correct; the old defaults were just too small for
realistic corpora).

## [0.5.2] — 2026-10-31

### Security — PR SEC-1 (AES-GCM nonce reuse across diverged copies)

**Advisory.** Before v0.5.2, page nonces were constructed deterministically
from `(page_no, write_counter)` under a per-file DEK that never changes
after `create`. If you had two copies of the same `.mnemo` file and
*both* copies received writes — scenarios include two laptops syncing
through Dropbox (where "conflicted copies" diverge independently), a
restored older backup resuming operation alongside the current file,
or any workflow where `cp agent.mnemo backup.mnemo` is followed by
writes to both — the two copies' writers would produce pages carrying
**the same encryption nonce under the same key with different
plaintexts**. That breaks AES-GCM's confidentiality (the XOR of the two
plaintexts is recoverable from the two ciphertexts) and its
tamper-evidence (the GHASH authentication key is recoverable, allowing
page forgery with a valid tag).

**What to do.**

1. Upgrade to v0.5.2 (or any later release that includes SEC-1).
2. If you've ever written to two copies of the same file, or resumed
   operation after restoring an older backup alongside an active file,
   you should **rotate your data-encryption-key**. The simplest path is
   `mnemo.compact_file(path, passphrase)` (Python) or
   `Mnemo::compact_file(path, passphrase)` (Rust) — compaction rewrites
   the file through a fresh `random_dek`, so any prior nonce reuse under
   the old key becomes irrelevant for the rewritten pages.
3. Future writes from v0.5.2 don't repeat nonces across copies even
   when `cp` + diverge happens again, because every writable open draws
   a fresh 64-bit random prefix.

**What SEC-1 does not fix.** Full rollback detection — reverting a
`.mnemo` to a byte-identical older state and continuing to use it — is
still out of scope. The only cure for rollback is a monotonic counter
kept **outside** the file; a counter inside the file can itself be
rolled back. See the Security model section of `mnemo/README.md`.

### Fixed — SEC-1 implementation

- Page nonces now follow NIST SP 800-38D §8.2.1 (fixed field +
  invocation field): a **random 64-bit per-session prefix** (drawn
  fresh on every writable open and regenerated if the counter would
  wrap) followed by a **32-bit per-session counter**. The `page_no`
  component of the pre-SEC-1 nonce is dropped — page position is
  already bound through AAD since v6 (`page_no.to_le_bytes()`), so a
  page swapped to a different slot still fails authentication.
  Cross-copy nonce collisions now require two independent sessions
  to draw the same 64-bit random value (birthday bound ~2^32 sessions
  — astronomically unlikely).
- **No format change.** Each page already stores its own 12-byte
  nonce in bytes `[0..12]` of the on-disk image; decrypt reads the
  stored nonce rather than recomputing one. A v0.5.1 file opens
  cleanly on v0.5.2 and vice versa.
- Audited every `aead_encrypt` call site: data-page writes (`pager::
  seal_page` + `pager::flush`) switched to the new per-session scheme;
  WAL frames reuse the same page images and inherit the fix;
  `format::apply_seal` was already on `random_nonce()`; `crypto::
  wrap_dek` was already on `random_nonce()` (via `store`).

### Added — tests

- `mnemo/tests/sec1_nonce_uniqueness.rs` — three targeted invariants:
  (1) no nonce appears in both copies after a fork-and-write
  sequence; (2) nonces drawn in one session don't share the 8-byte
  prefix with nonces drawn in a later reopen of the same file;
  (3) decrypt round-trip still works (format schema unchanged).
- `mnemo/tests/proptest_store.rs::prop_no_page_nonce_repeats_within_file`
  — random `remember` / `flush` / `reopen` sequences on one file;
  every encrypted page's 12-byte nonce must be distinct.

### Release mechanics

Shipped from a `release/0.5.x` hotfix branch cut from the `v0.5.1`
tag, because `main` carries unreleased PR ANN-1 behavior changes
being held for `v0.6.0`. `ci.yml` push + pull-request triggers
extended to cover `release/**` so the hotfix branch runs the full
matrix + fuzz-smoke before tagging. Merged back to `main` after
release.

## [0.5.1] — 2026-10-29

ARM hardware-AES fix. Pure build-config change — no API, no format,
same CHANGELOG `[0.5.0]` behavior otherwise.

### Fixed (PR 11.0 — hardware AES on ARM)

- **`aes` 0.8.4 and `polyval` 0.6.2 run their ARMv8 hardware paths
  only when built with `--cfg aes_armv8 --cfg polyval_armv8`.** The
  v0.5.0 release (and every earlier aarch64 build) didn't set them,
  so the `aarch64-apple-darwin` CLI binary and the `aarch64-apple-
  darwin` + `aarch64-unknown-linux-gnu` wheels ran the constant-time
  software fallback for every page encryption — ~130 MB/s per the
  Phase 6 bench, roughly 5× slower than the hardware path on an M1.
  x86_64 was unaffected (both crates auto-detect AES-NI and CLMUL at
  runtime).
- **Fix:** `.cargo/config.toml` at the repo root sets the cfgs under
  `[target.'cfg(target_arch = "aarch64")']`; `RUSTFLAGS` in both
  `ci.yml` and `release.yml` carries the same flags (env `RUSTFLAGS`
  fully overrides config `rustflags`, so the workflows have to repeat
  them). `release.yml` also passes `rustflags:` into the
  `PyO3/maturin-action` input so the manylinux + zigbuild aarch64
  containers see them. Declared under `[lints.rust] unexpected_cfgs`
  with explicit `check-cfg` in all three Cargo.tomls so `-D warnings`
  doesn't trip on them.
- **Verification surface:** new `mnemo::build_info() -> BuildInfo`
  reports the AES + PMULL backend. `mnemo info` prints the result at
  the top; Python `mnemo.build_info()` returns the same dict. On an
  M1 ARMv8 build with the cfgs active, the output reads:
  ```
  aes:         armv8 hardware
  polyval:     armv8 PMULL hardware
  ```
- **Users installing from crates.io on ARM:** your builds *don't* see
  this repo's `.cargo/config.toml`. Set the cfgs yourself before
  `cargo install mnemo-engine`:
  ```sh
  RUSTFLAGS="--cfg aes_armv8 --cfg polyval_armv8" cargo install mnemo-engine
  ```
  Or add a matching `[target.'cfg(target_arch = "aarch64")']` block to
  your own `~/.cargo/config.toml`. The `mnemo-engine` crate on PyPI
  (installed via `pip install mnemo-engine`) carries the hardware
  path baked in — the fix lives in the wheel's compile step on the
  release-workflow side.

### Added (Phase 6 instrumentation, Phase 11.1 input)

- **`Stats::fsyncs_last_flush: u32`** reports how many `sync_all`
  calls the most recent successful `flush()` on this handle issued.
  Thread-local counter is incremented at every `sync_all` call site
  (`pager.rs` × 2, `wal.rs` × 1, `store.rs` × 2 — the WAL recovery
  path on open); `flush()` snapshots it before and after, exposes
  the delta. The planner wants this before scheduling the write-
  latency work in Phase 11.1 — cutting fsync count preserves
  durability; batching memory writes would not. Surfaced in
  `mnemo info` as `fsyncs/flush: N`.

### Numbers (M1 Pro, release build, 384-dim clustered corpus)

Re-run of the Phase 6 speed benches with hardware AES active, side-by-
side with the v0.5.0 software-AES baseline. Full set under
`test/scripts/perf_v0.5.0.json` — the v0.5.0 numbers stay under `speed`,
the v0.5.1 rerun lives under `speed_hw_aes`.

| Bench | SW AES (v0.5.0) | HW AES (v0.5.1) | Speedup |
|-------|-----------------|-----------------|---------|
| `recall/exact_scan_10k` | 634.6 ms | **105.2 ms** | **6.0×** |
| `recall/exact_scan_100k` | 6.41 s | **1.05 s** | **6.1×** |
| `cache/cache_put_exact_strict` | 611 µs | 471 µs | 1.30× |
| `cache/cache_put_exact_batched` | 397 µs | 295 µs | 1.35× |
| `cache/cache_put_semantic_strict` | 652 µs | 489 µs | 1.33× |
| `open/open_latency_fast_kdf` | 744 µs | 629 µs | 1.18× |

The planner's PR 11.0 C1 call was exactly right: v0.5.0's 623 ms
exact-scan cost at 10K was software AES, not record decryption as such.
Unaffected (as expected):

- `recall/ivfpq_recall_*` — IVF+PQ decrypts only a few partitions per
  query, so AES throughput wasn't a bottleneck there.
- `open/open_latency_secure_kdf` — Argon2id-bound, doesn't use AES.
- `write/remember_*` — fsync-bound.

The exact-vs-IVF+PQ gap narrows from 1,660× to 282× at 10K and from
10,400× to 1,740× at 100K. IVF+PQ remains the right default; the exact
fallback is now usable at smaller corpora. The 1M tier wasn't re-run
(stale 64 s number in the `speed` block is still software AES; expect
roughly 10-11 s on hardware AES, extrapolating from the 6× ratio).

## [0.5.0] — 2026-10-24

Phase 3 release — **bring-your-own-embedder**. The engine now takes
raw text in addition to raw vectors on `remember`, `recall`, and the
semantic cache tools; an `Embedder` trait wires any model you can
invoke from a child process. The persistent-child `EmbedCmd` adapter
spawns your embedder once per handle and talks a one-JSON-line-per-
request protocol over stdio. CLI gets `remember --text` and
`recall --text`; MCP tools pick up a `text` field; `serve --mcp`
takes `--embed-cmd` + `--embed-name` + `--embed-timeout`. All-zero
vectors are now refused at `remember` time (silent-noise fix; the
breaking signal that carries the minor bump). See the entries
below for the five PRs that got us here (10a, 10a.1, 10a.2, 10b +
10b.1, 10b.2).

### Breaking (behavior)

- **All-zero vectors are now refused by `Mnemo::remember`.** They score
  exactly `0.0` on every cosine query — they can't contribute signal,
  but recency/importance can still pull them into top-k as noise. The
  check lives in `store::put` and surfaces as `MnemoError::Invalid`
  with the message `"all-zero vector refused: …"`. Mirrored to the
  Python binding (same `RuntimeError` text). The internal
  `Memory::scaffold_manifest` helper is updated to emit a single-axis
  unit spike (`[1, 0, 0, …]`) so `mnemo init` and
  `Mnemo.insert_default_manifest` still work. **Pre-1.0 convention:
  the minor bump carries the breaking signal, so this ships as v0.5.0,
  not v0.4.2.**

### Phase 3 — bring-your-own-embedder (part 2: PR 10b)

- **`EmbedCmd`** adapter in `mnemo/src/embed_cmd.rs`. Implements the
  `Embedder` trait by spawning a user-supplied command as a
  **persistent child process** (one child per `Mnemo` handle, reused
  for every subsequent embed call). Protocol: one JSON-encoded string
  per line in, one JSON array of floats per line out, flush after
  each reply. Timeout per request (default 60s via
  `DEFAULT_EMBED_TIMEOUT`). Child is reaped on handle drop. Errors
  surface as `MnemoError::Invalid` with the command and position in
  the batch — never a hang. No shell is invoked: the command string
  is shell-split in Rust (quotes honored, no `~` or `$VAR` expansion).
- **CLI `mnemo remember <file> --text "..." --embed-cmd ...
  --embed-name ..."`** — new subcommand that fills the only path to
  write a single memory from the CLI that previously existed (bulk
  `mnemo import` with precomputed vectors was the only option).
  Mirrors Python `remember_text`'s kwargs (`--type`, `--importance`,
  `--agent`, `--metadata` JSON).
- **CLI `mnemo recall <file> --text "..." --embed-cmd ...
  --embed-name ...`** — `--text` is mutually exclusive with
  `--query` / `--query-file`. Works alongside `--no-track` on a
  read-only handle for concurrent-reader scenarios.
- **MCP `mnemo serve --mcp <file> --embed-cmd ... --embed-name ...`**
  — the server spawns the embedder once at startup. `remember`,
  `recall`, `cache_put_semantic`, and `cache_get_semantic` tools now
  accept a `text` param as an alternative to the explicit vector.
  For the semantic cache tools, `model` defaults to the embedder's
  identity name when `text` is used. Tool descriptions in `tools/list`
  updated to document the new input shape. Passing `text` without
  having started the server with `--embed-cmd` returns a JSON-RPC
  error that spells out the fix.
- Reference embedder in `mnemo/examples/embedder_server.py` —
  `sentence-transformers/all-MiniLM-L6-v2`, 384-dim normalized, 15
  lines excluding docstring.
- Closes the Phase 4 "text path waits on Phase 3" note.

### Fixed (PR 10b.2 — EmbedCmd correctness)

- **Timeouts actually fire.** PR 10b's `read_line_with_timeout` was a
  blocking read with a wall-clock check that only ran *after* the read
  returned — a child that stayed silent (hang, deadlock, dead CPU,
  model-load wedge) would block the parent forever despite
  `--embed-timeout`. **Fix:** a background reader thread owns
  `BufReader<ChildStdout>` and forwards each line over an `mpsc`
  channel; `EmbedCmd::embed` reads via `recv_timeout`, so a silent-
  but-alive child trips the deadline deterministically. Side effect:
  stdout is drained in parallel with stdin writes, so a 2,000-text
  batch no longer deadlocks against the child's own stdout buffer.
- **Protocol errors can't produce mismatched vectors anymore.** PR 10b
  returned an error on malformed JSON / wrong width / zero vector but
  left stale replies in the pipe, so the *next* `embed()` call could
  read a leftover line as its first reply and return the previous
  text's vector for a new input. Silent corruption of recall quality.
  **Fix:** any protocol error (write failure, timeout, EOF, malformed
  JSON, wrong width, zero vector, or any line printed to stdout
  *before* a request) now kills the child, drops the reader thread,
  and marks the adapter for restart. The next `embed()` call spawns
  a fresh child + reader with the same argv/env/timeout before
  writing anything, and logs one stderr line saying it restarted and
  why. Rule, enforced by test: *an error is allowed; a mismatched
  vector never is.*
- **Startup-banner footgun has a clear error.** A Python embedder
  that prints `"[loading model...]"` to stdout before entering its
  read loop used to shift every subsequent reply by one line.
  EmbedCmd now detects any line waiting in the pipe before a request
  and errors with `"stdout is reserved for replies only; log to
  stderr instead"`. Documented in the `EmbedCmd` module docs, the
  `mnemo serve --mcp` section of `mnemo/README.md`, and the
  reference embedder script.
- **CLI `--embed-timeout <secs>` on `remember`, `recall`, and `serve`.**
  Default 60s (unchanged). Mirrors the Rust `EmbedCmd::with_timeout`
  setter — set it longer than your slowest observed cold-start reply
  (sentence-transformers' first `encode()` can be 3–5s on CPU).
- **Windows: moved the CLI dispatch onto a worker thread with 8 MiB
  of stack.** Windows' default main-thread stack is 1 MiB, and the
  CLI's `run()` is a single ~600-line match whose debug-mode frame
  reserves space for every variant's locals at once. The extra
  `embed_timeout` field on three subcommands pushed us over the
  edge on Rust 1.95 / `windows-latest` (stack overflow in every CLI
  test, exit code `0xC00000FD`). Spawning `run` on a named worker
  thread sidesteps the whole class — portable, no linker flags,
  no cfg guards. Overhead is ~1 ms, invisible against the file I/O
  that follows.
- Covered by six new tests in `mnemo/tests/cli_smoke.rs`: hang bounded
  by `--embed-timeout`, MCP hang-and-respawn (server survives, next
  call returns the right vector), junk-line realignment, double-line
  realignment, stdout-banner surfaces as a clear error, and a direct
  2,000-text batch call completing without deadlock. The test fake
  embedder returns text-dependent vectors (`v[axis] = len(s) + 1.0`,
  `axis = sum(bytes(s)) % dims`) so a misalignment is detectable at
  the vector level, not just via `is_ok()`.

### Fixed (PR 10a.2 — Python binding only)

- **`Mnemo.close()` and `Mnemo.__exit__` on read-only handles no
  longer raise `ReadOnly`.** Both now delegate to the Rust core's
  `Mnemo::close()`, which short-circuits on read-only (nothing to
  flush) instead of calling `flush()` directly. A latent bug since
  v0.4.0 that only became obvious once PR 10a promoted read-only
  text recall into a routine use case: `with mnemo.open(..., read_only=True)`
  blocks raised on scope exit, and `db.close()` raised on readers.
- **`Mnemo.export_encrypted(dest)` on a read-only handle no longer
  raises.** Same root cause — switched from `flush()` to the
  mode-aware `close()`. A reader can now take a byte-for-byte
  backup of the file without needing to reopen read-write first.
- Covers both with a new `test_read_only_with_block_and_close_are_noops`
  test (with-block exit, explicit `close()`, `export_encrypted`).

### Phase 3 fixups (PR 10a.1)

- **Manifest identity check now reads the canonical nested form.**
  PR 10a wrote and read three flat keys (`embedder_name`,
  `embedder_dimensions`, `embedder_normalize`) on the scaffold
  manifest. The documented convention — `mnemo/README.md`,
  `test/scripts/seed.json`, every file bootstrapped from it — is a
  nested object: `metadata.embedder = { name, dimensions, normalize }`.
  10a's check looked for the flat keys, never found them, treated
  the file as having no recorded identity, and silently accepted
  whatever embedder was passed. **Fix:** canonical read + write is the
  nested object; flat keys are still accepted on read as a legacy alias
  (unreleased dev builds only). Writes always canonicalize to nested
  and remove any legacy flat keys found alongside.
- **`set_embedder` now works on read-only handles.** PR 10a's early
  `if self.read_only { return Err(ReadOnly) }` meant readers (CLI
  `--read-only` commands, any `Mnemo::open_read_only` caller)
  couldn't register an embedder at all — the whole text path was
  gated behind write access. **Fix:** run the dimension + manifest
  identity checks regardless of mode; skip only the manifest upsert
  on read-only (never write from a reader).
- **`test/scripts/project_memory.py` EMBED_MODEL** = `"sentence-
  transformers/all-MiniLM-L6-v2"` (was `"all-MiniLM-L6-v2"`), so
  the identity recorded on the dogfood manifest and the identity
  the script passes to `set_embedder` match exactly under the now-
  canonical nested read path. Same model, same weights, same
  vectors — only the name string changes.
- **AGENTS.md** picks up the "first vector component must be
  non-zero" note on the integration-test row — the Phase 3 zero-
  vector guard trips `for i in 0..N` loops that stuff `i` into the
  first component; prefer `(i as f32) + 1.0`.

### Phase 3 — bring-your-own-embedder (part 1: PR 10a)

- **`Embedder` trait + `EmbedderIdentity`** in `mnemo/src/embed.rs`
  (`pub use mnemo::{Embedder, EmbedderIdentity}`). Minimal surface:
  `identity() -> EmbedderIdentity` + `embed(&[&str]) -> Result<Vec<Vec<f32>>>`.
  Thread-safe (`Send + Sync`) so a handle can share one `Arc<dyn
  Embedder>` across threads.
- **`Mnemo::set_embedder(embedder)`** registers a text embedder on an
  open handle. Dimension-checks against `Mnemo::dimensions()` and
  **identity-checks against the scaffold manifest** — if the manifest
  already records an embedder (`embedder_name` / `embedder_dimensions`
  / `embedder_normalize` metadata keys), the new one must match
  exactly or the call fails with `MnemoError::Invalid` carrying
  `EmbedderIdentity::MISMATCH_MSG`. On first registration into a
  manifest that has no embedder metadata yet, the three keys are
  upserted so a subsequent reopen catches an embedder swap.
- **`Mnemo::remember_text(content, memory_type)`** and
  **`RecallRequest::from_text(&embedder, query)`** convenience paths.
  Both validate the embedder's returned-vector width and bubble up
  embedder failures as `MnemoError::Invalid`.
- **`Mnemo::embedder() -> Option<&dyn Embedder>`** getter so bindings
  can drive their own text-side wrappers without duplicating the
  identity check.
- **Python binding (`mnemo-python`):**
  - `db.set_embedder(fn, name=..., dimensions=..., normalize=True)` —
    accepts any Python callable `embed(texts: list[str]) ->
    list[list[float]]`. Validated once, held on the handle.
  - `db.remember_text(content, memory_type, *, agent_id=None,
    importance=None, session_id=None, ttl_secs=None, shared=False,
    metadata=None)` — mirrors `remember`'s full kwarg surface so
    migration is a one-word swap.
  - `db.recall_text(query, top_k=10, memory_types=None, agent_id=None,
    track_access=True)` — same shape as `recall` but takes a string.
  - `db.cache_get_semantic_text(namespace, text, threshold=0.97)` —
    text-side semantic cache lookup, uses the embedder's `name` as
    the semantic cache's `model` component so a different embedder's
    entries don't match.
- **`test/scripts/project_memory.py` moved to the text path** —
  `set_embedder(embed_texts, name=EMBED_MODEL, dimensions=DIMENSIONS,
  normalize=True)` at open time, then `remember_text` / `recall_text`
  everywhere. Fixes the zero-vector dogfood footgun that produced
  ULID `01M3ST1KRS5G7P07691SB5TNYF` on v0.4.1.
- **Not in this PR, intentionally:** the heavy `embed-fastembed`
  feature (PR 10c), and the `mnemo remember/recall --embed-cmd ...`
  CLI/MCP text paths (PR 10b). This PR keeps the dep tree unchanged
  so `cargo deny check` stays silent.

## [0.4.1] — 2026-09-25

Patch release. **No format change** — 0.4.1 opens any 0.4.x file, and
files written by 0.4.1 open on 0.4.0. This is the first release
published through PyPI Trusted Publishing (OIDC); no repo-side
`PYPI_API_TOKEN` was used.

Cumulative deltas since v0.4.0, grouped by shipping PR:

### Security / hardening

- **PR 7 — Phase 1.2 fuzz harness.** New `mnemo/fuzz/` standalone
  `cargo-fuzz` crate with five libFuzzer targets covering every
  untrusted-input parser: `fuzz_header` (page-0 header, pre-passphrase
  attack surface, v8 layout), `fuzz_wal_replay` (WAL scan on open),
  `fuzz_record_decode` (MessagePack `Memory` + `CacheEntry` decode),
  `fuzz_cache_directory` (`Vec<CacheDirectoryEntry>` decode incl. the
  `#[serde(default)]`-tolerant PR 4 fields), `fuzz_mcp_frame` (MCP
  stdio JSON-RPC request-line parse). Targets reach into the crate
  via `mnemo::__fuzz::*` (`#[doc(hidden)]` — not part of the stable
  API). CI runs each target for 60s on every PR (`fuzz-smoke` job,
  nightly) and 10 min/target weekly (`fuzz-weekly.yml`, Sundays
  05:00 UTC, uploads crash artifacts on failure). `wal::recover`
  was refactored to delegate to a byte-slice variant
  `recover_bytes(&[u8], u64)` so the WAL scanner fuzzes without
  needing a `File`; behavior is unchanged for production callers.
- **PR 8 — Phase 1.3 proptest state machine + crash injection.**
  New `mnemo/tests/proptest_store.rs` with four properties over the
  operation alphabet (`remember` / `delete` / `flush` / `reopen` /
  `restore_to` / `compact_file` / `cache_put` / `cache_get` /
  `cache_purge`) under both `CacheFlushPolicy::Strict` and
  `Batched { max_dirty, max_age }`: (a) flush + reopen preserves
  the observable memory set; (b) batched-policy `cache_get` never
  surfaces a stale/wrong value across the crash boundary; (c) a
  partial-flush crash (WAL commit not reached) reverts the reopened
  database to the last committed state; (d) arming a fail-after-N-
  writes hook during `flush()` yields either the pre-flush or the
  post-flush state on reopen, never a third state (the atomicity
  invariant). Adds `proptest = "=1.5.0"` as a dev-dep (MSRV 1.75
  safe), a new optional `failpoints` feature exposing
  `mnemo::__failpoints::set_writes_until_fail` (`#[doc(hidden)]`,
  not stable API), and a `pub(crate)` `__failpoint_check()` in
  `pager.rs` that is a zero-cost stub outside `cfg(test)` and the
  `failpoints` feature. Hooks land in `pager::write_raw`,
  `pager::write_sealed`, `pager::flush` (per-page), and
  `wal::commit` — every file-writing site in the durability path.
- **PR 9 — Phase 1.4 supply-chain + fmt-check CI.** Adds
  `mnemo/deny.toml` with three checks wired into a new `deny`
  CI job (prebuilt cargo-deny via `taiki-e/install-action`):
  advisories cross-referenced against the RustSec DB, an
  SPDX license allowlist (Apache-2.0-compatible: Apache-2.0,
  MIT, BSD-2/3, ISC, Zlib, 0BSD, Unicode-DFS-2016, Unicode-3.0,
  CC0-1.0, MPL-2.0, and Apache-2.0 WITH LLVM-exception), and
  a duplicate-major-version check (`multiple-versions = "warn"`
  as a first-landing safety net; follow-up PR flips to `deny`
  once CI has enumerated the necessary skip set). Wires
  `cargo fmt --check` into the rust matrix job so rustfmt
  drift is caught at PR time (previously only clippy gated).
  Switches the release workflow's PyPI publish from the
  deprecated `maturin publish/upload` path (PyO3/maturin#2334)
  to PyPI Trusted Publishing via `pypa/gh-action-pypi-publish`
  — no long-lived `PYPI_API_TOKEN`; each release mints a
  short-lived OIDC token from GitHub and trades it with PyPI.
  Requires a one-time PyPI-side trusted-publisher registration
  (owner: `toarchkumar`, repo: `mnemo`, workflow: `release.yml`,
  environment: `pypi`).
- **PR BP — Python binding parity.** Additive; no format change, no
  breaking API on the existing surface. Fills the gaps identified
  by the 2026-09-20 audit so every user-facing method on
  `Mnemo` in Rust is reachable from Python:
  - `Mnemo.set_cache_flush_policy(policy, max_dirty=None, max_age_secs=None)`
    — switches the cache-flush lane. Under `"batched"` the Python
    binding stops calling `flush()` after every cache mutation and
    lets the engine's auto-flush thresholds fire; matches the
    Phase 10.3 Rust semantics. Default remains `"strict"` so
    existing callers see unchanged behavior.
  - `Mnemo.set_cache_budget(namespace, max_entries, max_bytes)` —
    per-namespace LRU cap; previously Python was stuck with the
    10 000 / 64 MiB defaults.
  - `Mnemo.memories()` — enumerate every live memory as a list of
    dicts (mirror of the Rust `memories()`).
  - `Mnemo.dimensions()` — plain getter for the vector width.
  - `Mnemo.rekey(new_passphrase, fast=False)` — change the
    passphrase; O(1) in db size (DEK is re-wrapped, pages untouched).
    `fast=True` selects the cheap-Argon2 test params.
  - Module-level `mnemo.compact_file(path, passphrase)` — the
    static rewrite that drops tombstones/expired entries. Returns
    `{"before": int, "after": int}`.
  - `test_mnemo.py` extends with six new checks: `memories() +
    dimensions()`, batched auto-flush surviving reopen, batched
    unflushed-put appearing as a miss (the Phase 1.3 invariant
    surfaced through Python), per-namespace LRU eviction under
    `set_cache_budget`, rekey → reopen with the new passphrase
    (old rejected), and `compact_file` round-trip.
  - **Deferred (logged for a future BP.1):** `build_index_with` /
    `rebuild_index` are not exposed yet — they require thin
    wrappers around `IndexConfig` (n_probe, n_rerank, plus the
    subspace/partition params) that no current Python caller has
    asked for. `build_index()` still works as-is.

## [0.4.0] — 2026-08-28

**On-disk format v7 → v8.** Files written by v0.3.x on v4/v5/v6/v7
migrate transparently on first open under v0.4.0 (the existing
cascade migration extends by one step). **Files written by v0.4.0
do NOT open on v0.3.x** — the header seal's AAD now covers the
new cache-directory pointer, so a 0.3.x build reads the file as
tampered rather than trying to interpret unknown bytes. Pre-1.0,
the minor-version bump carries the breaking-change signal, so
this is 0.4.0 rather than 0.3.3.

Cumulative feature deltas since v0.3.2, grouped by shipping PR:

- **PR 1 — Phase 1.1 file locking + read-only opens** (`fs4`,
  `MnemoError::Locked`/`ReadOnly`/`NeedsWriteOpen`,
  `Mnemo::open_read_only`, CLI `recall --no-track`, Python
  `mnemo.open(..., read_only=True)`).
- **PR 2 — Phase 4 MCP server** (`mnemo serve --mcp`, hand-rolled
  JSON-RPC 2.0, 7 initial tools, `MNEMO_PASSPHRASE`-only auth,
  ready-to-paste Claude Desktop config in the README).
- **PR 3 — Phase 10.1 + 10.3 exact-key result cache, format v8**
  (`result_cache.rs`, `Mnemo::cache_put`/`get`/`delete`/`purge`/
  `stats`, `CacheFlushPolicy::Strict`/`Batched`, CLI
  `mnemo cache <op>`, `page_cache_stats` rename).
- **PR 4 — Phase 10.2 semantic cache** (`cache_put_semantic` /
  `cache_get_semantic`, `SemanticCachePutOpts` with mandatory
  `model`, `DEFAULT_SEMANTIC_THRESHOLD = 0.97`, no format bump —
  optional fields ride under `#[serde(default)]`).
- **PR 5 — Phase 10.4 live surfaces** (five new MCP cache tools,
  seven new Python `cache_*` methods, ~90-line `@db.cached(...)`
  decorator recipe with OpenAI/Anthropic/semantic-mode examples
  in `mnemo-python/README.md`).

Two hotfixes on top of PR 1 kept CI green after Windows exposed
that `LockFileEx` returns `ERROR_LOCK_VIOLATION` (raw OS code 33)
as `ErrorKind::Uncategorized`, and that it is mandatory (unlike
Unix `flock`) — so several tamper/WAL tests needed `drop(db)`
before raw file reads.

Sections below capture the full change list unchanged from the
`[Unreleased]` sequence that these PRs appended.

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

### Added (Phase 10.4: live surfaces for the result cache)

- **MCP tools** — `mnemo serve --mcp` gains five cache tools:
  `cache_put`, `cache_get`, `cache_stats`, `cache_put_semantic`,
  `cache_get_semantic`. All mutating variants flush before returning.
  Semantic variants take vectors as JSON arrays (BYO-vector, same
  pattern as `remember` / `recall`).
- **Python bindings** — `Mnemo` gets `cache_put` / `cache_get` /
  `cache_delete` / `cache_purge` / `cache_stats` /
  `cache_put_semantic` / `cache_get_semantic` mirrors. `cache_put` and
  `cache_put_semantic` accept `str` or `bytes` for the value; get
  variants return `bytes` in the `value` field of the response dict.
- **`@db.cached(...)` decorator recipe** — ~90-line pure-Python
  helper documented in `mnemo-python/README.md`, with three copy-paste
  recipes: OpenAI GPT-4o call wrapped end-to-end, Anthropic Claude
  swap-in, and semantic-mode variant using OpenAI embeddings. Not
  shipped as an installable module — the plan called for a helper
  users copy into their project, so the README carries it verbatim.
- **Python API table updated** in `mnemo-python/README.md` with the
  new methods and the `cache_stats` → `page_cache_stats` rename.

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

[Unreleased]: https://github.com/toarchkumar/mnemo/compare/v0.10.0...HEAD
[0.10.0]: https://github.com/toarchkumar/mnemo/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/toarchkumar/mnemo/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/toarchkumar/mnemo/compare/v0.8.1...v0.9.0
[0.5.2]: https://github.com/toarchkumar/mnemo/compare/v0.5.1...v0.5.2
[0.5.1]: https://github.com/toarchkumar/mnemo/compare/v0.5.0...v0.5.1
[0.5.0]: https://github.com/toarchkumar/mnemo/compare/v0.4.1...v0.5.0
[0.4.1]: https://github.com/toarchkumar/mnemo/compare/v0.4.0...v0.4.1
[0.4.0]: https://github.com/toarchkumar/mnemo/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/toarchkumar/mnemo/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/toarchkumar/mnemo/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/toarchkumar/mnemo/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/toarchkumar/mnemo/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/toarchkumar/mnemo/releases/tag/v0.1.0
