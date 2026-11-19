# PERF-2: a faster first search after opening

Status: **built in step 2** (task `v0190-2`, 2026-11-17), as
`decision.perf-2-design` scoped it. See section 7, "As built", for what
step 2 changed against the plan below. The spec is the brain's
`spec.perf-2`; step 1 was task `v0190-1`.

**The constraint:** no file-format bump. The v9 ANN index run already starts
with a layout byte (`V9_INDEX_LAYOUT = 1`, `store/mod.rs`). A reader that
doesn't know a layout loads "no index" and recalls exactly; it never
errors (`load_index`, `store/load.rs`). So a new index layout can ship
inside format v9.

## 1. The baseline (v0.18.1)

**Where it was measured:**
- Apple M1 Pro (8 cores, 16 GB), release build.
- `--cfg aes_armv8 --cfg polyval_armv8` (from `.cargo/config.toml`; no
  `RUSTFLAGS` in the environment), the same hardware AES the released
  binaries use.
- `release-0.19.0` at `72ae920`, whose engine is the `v0.18.1` tag's:
  `git diff v0.18.1 HEAD -- src Cargo.toml Cargo.lock` is empty.
- **Load:** the load average was 3.2 to 6.4 during the runs (logged per
  run), with `fileproviderd` holding one core. The two `device_budget`
  runs show the noise.

**The commands:**
- `cargo run --release --features benches --example device_budget --
  --sizes 1000,10000,100000`, twice: 384-dim vectors, about 300 B of
  text, the fast KDF, an IVF+PQ index at 100K;
- `--example ranking_v2 --perf-json <scratch copy>`;
- `--example ann_defaults_v0_6_0` with `MNEMO_DEV_ANN_MIN_FACTS=1000`.
  Without the override it panics at its 10K tier, which has no index on
  v9 by design (`V9_ANN_MIN_FACTS = 20_000`). Its 10K row is therefore a
  forced index; the 100K row is the shipped default.

| What (ms unless marked) | 1K | 10K | 100K |
|---|---|---|---|
| **First recall**, fresh handle, median of 20 (run 1 / run 2) | 0.92 / 0.92 | 10.38 / 9.16 | 118.29 / 116.45 |
| First recall, p95 (run 2) | 0.93 | 10.21 | 122.18 |
| First recall after `stats()` has loaded the index (run 1 / run 2) | 0.33 / 0.33 | 9.14 / 8.85 | 6.31 / 6.37 |
| **Warm recall**, median of 1,000 (run 1 / run 2) | 0.22 / 0.22 | 2.25 / 2.28 | 2.40 / 2.41 |
| Open, median of 20 (run 1 / run 2) | 0.54 / 0.52 | 3.79 / 3.37 | 30.47 / 30.16 |
| One handle's RSS after 1,000 warm recalls, MB (run 1 / run 2) | 2.9 / 2.9 | 17.5 / 16.2 | 112.5 / 112.5 |
| Durable single write, median (run 1 / run 2) | 14.99 / 14.01 | 12.79 / 15.17 | 14.63 / 15.53 |
| Batch of 100, per memory (run 1 / run 2) | 0.256 / 0.280 | 0.198 / 0.271 | 0.376 / 0.388 |
| fsyncs: single write / batch of 100 (run 2's JSON) | 3 / 4 | 3 / 3 | 3 / 4 |
| First `stats()` on a fresh handle, median (run 1) | 1.03 | 12.14 | 208.30 |

**Quality:**
- `ranking_v2`: hit@1 **0.9231**, MRR **0.9487** (the shipped formula),
  and the phase-6 gate 0.5667 / 0.6779, against floors of 0.5467 /
  0.6665.
- ANN recall@10 at 100K with the defaults (`fresh_install`: 317
  partitions, `n_probe` 159, `n_rerank` 500): **0.978**, 3.78 ms per
  query.

**Against the spec's targets:**
- First recall at 100K is 116 to 118 ms (target ≤ 15).
- First recall at 10K is 9.2 to 10.4 ms (target ≤ 5).
- Warm recall at 10K is 2.25 to 2.28 ms (target ≤ 2).
- Everything else above is a "must not regress" floor for step 2.

## 2. Where first recall goes

**At 100K the index load is almost all of it.** `stats()` runs
`ensure_ann()`: it streams and decodes the whole index run, then rebuilds
the `id → partition` map and reconciles the index with the catalog. After
that, the first recall takes 6.3 ms; on a cold handle it takes 116 to 118
ms. So about 110 ms is loading the index (and whatever `stats()`'s own
`memories()` pass warms), and about 6 ms is the query.
- The index at 100K is about 17 MB (the `load_index` comment): 317
  centroids × 384 floats (0.5 MB), a PQ codebook of 96 subspaces × 256
  codes × 4 floats (0.4 MB), and 100K postings of a 16-byte id plus 96
  code bytes.
- A query reads half the partitions (`n_probe` 159 of 317) at the
  defaults, so a partial load saves about half the posting bytes, not
  most of them. **The bigger saving is not building the 100K-entry `id →
  partition` map,** and decoding postings straight into their arrays
  instead of through `rmp_serde`'s per-element path. Step 2 must measure
  which part dominates before the layout is final (see 6).

**At 10K, with no index, it's the vector column.** Loading the index
first changes nothing there (9.14 against 10.38 ms, 8.85 against 9.16):
- The exact scan needs every vector.
- The column is filled lazily, a page at a time (`fill_column_page`), but
  a first scan touches every page: 1,000 vector pages at 10K. At 384
  dimensions an entry is 772 bytes (`veccol::entry_len`) and a page holds
  10 (`veccol::per_page`).
- Each page is a `seek`, a `read_exact` and an AES-GCM decrypt
  (`Pager::read_page_uncached`).
- v0.11's profile put the fill at 66% of first recall and the decrypt at
  48%.

## 3. The design

### 3a. Index layout 2: partition-major, loaded by partition

**Layout 2 is a new base index run, behind the existing layout byte:**
- **The head** (one msgpack value): `dims`, `n_partitions`, `n_probe`,
  `n_rerank`, the centroids, the PQ codebook, and a **directory**:
  - for each partition, its byte offset and length in the run, and its
    entry count;
  - the run's total length, so the reader knows where the postings start.
- **The postings,** partition after partition. Each partition's ids are a
  flat little-endian `u128` array, then its codes are a flat `u8` array
  (`count × m`). Raw arrays, not msgpack, so they load with one copy.

**Reading it:**
- **On first ANN use** (`ensure_ann`), the head is read and decoded: about
  1 MB at 100K, against 17 MB today.
- **Each query** probes its `n_probe` partitions. Any that aren't loaded
  are read then: the pages covering `[offset, offset + length)`, in one
  batched read (3c), decrypted and copied into place.
- A partition is loaded at most once per handle.
- The `id → partition` map isn't built at load. It's built only when a
  write path needs it (below).

**The delta chain and reconcile, with partitions not yet loaded:**
- `IndexOp::Add(id, partition, code)` names its partition. If that
  partition is loaded, it's applied; if not, it's queued on the
  partition and applied when the partition loads.
- `IndexOp::Remove(id)` doesn't name one. It goes into a handle-wide
  removed set, which each partition filters out as it loads (and which
  is applied directly to the partitions already loaded).
- `reconcile_ann_with_catalog` (records newer than the run, by
  generation) assigns each by its nearest centroid, as today, and so
  becomes an `Add` on that partition.

**Writing:**
- Every new base is written in layout 2. A base is written by
  `build_index`, `rebuild_index`, compaction, or a base rewrite when the
  delta chain gets long.
- Anything that needs the whole index first loads every remaining
  partition, then runs today's code unchanged: the `id → partition` map,
  a new base, `drop_index` freeing runs, `stats().index`'s counts. Writes
  that only append a delta (`Add`, `Remove`) don't need a full load.
- Layout 1 is still read, decoded whole as today, and rewritten as layout
  2 the next time a base is written.

**What an older reader does with layout 2:**
- **v0.18.1 and older v9 builds** see an unknown layout byte and load no
  index.
  - Recall falls back to the exact fp16 scan: right results, slower at
    100K. Step 2 measures by how much.
  - The delta runs are tracked and freed with the base (FMT-9f,
    `index_chain_runs`), as with any unknown base.
  - An older **writer** records no index deltas. The records it writes
    have newer generations, so the next v0.19 open re-adds them in
    reconcile.
  - An older build's `build_index` or `rebuild_index` writes layout 1,
    which v0.19 still reads.
- **v8 read-only files** keep their v8 index (no layout byte), loaded
  whole, as today.
- No error in any of these cases.

### 3b. The vector column at 10K: batched reads, parallel decrypt

The exact scan's first recall is the column fill: every vector page,
read and decrypted.
- **One read per run of adjacent pages** (3c), instead of a `seek` and a
  `read` per page.
- **Decrypt on several cores** when there are enough pages: scoped
  threads (`std::thread::scope`), each decrypting a slice of the page list
  into its own part of the column.
  - Only above a page-count threshold that step 2 measures, and with at
    most `available_parallelism()` threads.
  - No new dependency, and no `unsafe` (the crate forbids it).
- The column at 10K is 7.72 MB of entries (10K × 772 bytes). The decrypt
  rate per core on this machine isn't measured yet; step 2 measures it
  and the parallel fill before claiming the 5 ms target.

### 3c. One read per run of pages

`Pager` gets a `read_pages(start, count, gen)`:
- one positioned read (`FileExt::read_at` on Unix, `seek_read` on
  Windows) into a buffer of `count` pages;
- then each page is authenticated and decrypted as now.

Users: `RunReader` (catalog, index head and partitions, manifest) and the
column fill. A short read, or a page that fails authentication, fails the
same way a single page does today.

### 3d. Warm recall at 10K

`dot_f16` already runs eight lanes, with an exact, branch-free fp16 widen
(`veccol.rs`), and the warm scan is 2.25 ms against a 2 ms target. Two
levers, chosen by measurement in step 2:
- **Sixteen lanes in two accumulators,** to hide the add latency.
- **Widening a whole entry at once:** 384 values into a stack `[f32; 384]`,
  then an `f32` dot product. That keeps the widen out of the inner loop's
  dependency chain.

A parallel warm scan (3b's threads) is a third option at 10K. It's only
worth it if the first two fall short: warm recall is called often, and
starting threads has a cost step 2 would have to measure against the 2 ms.

### 3e. Shared mode and the lock

- **A plain handle** holds its file lock for its whole life: exclusive
  for a writer, shared for a read-only open (`LockedFile`, `store/open.rs`).
  No other process can commit, and so free and reuse a page of the loaded
  index run. Lazy partition reads are always reading the run the head came
  from.
- **A shared handle** (12.4) checks its stamp at the start of each call,
  under the lock.
  - If another process committed, the handle reloads and drops everything
    it decoded, the loaded partitions included.
  - If not, the run is the same, and a partition read in this call is
    valid.
  - So lazy loads happen only inside a call that has passed the stamp
    check, which is the only place a shared handle reads the file at all.
- **A safeguard:** every page is authenticated with its generation. A
  partition page read from the wrong run fails as `PageAuthFailed`, never
  as wrong data. Step 2 adds a test: another process commits, then this
  handle's next recall reloads, and the partitions it read before are
  gone.
- **This is where PERF-2 pays most in shared mode.** Every reload after
  another process's commit pays first recall again. The 12.4 note measured
  112.5 ms at 100K for that reload's first recall.

### 3f. The 4th fsync on some batch commits

Measured above: a batch of 100 makes 4 fsyncs at 1K and 100K, and 3 at
10K. Single writes make 3.

The usual three:
1. `Pager::flush` syncs the data pages;
2. `wal::commit` syncs the commit frame, the durability point;
3. `checkpoint` syncs the home pages.

**The fourth is FMT-9c's direct writes.** A control run longer than
`V9_WAL_RUN_MAX_PAGES` is sealed straight to its home pages in step 3,
after `Pager::flush` has already synced, so `flush` syncs again before
the commit (`direct_writes`, `store/flush.rs`).

**The proposal:**
- Defer `Pager::flush`'s sync until after step 3's direct writes, so one
  fsync covers both the data pages and the direct runs.
- Batch commits that write a big run then go from 4 to 3 fsyncs;
  everything else stays at 3.

**Why it's safe:**
- Both sets of pages are fresh and referenced by nothing until the WAL
  commit, and both only need to be durable before it. One barrier before
  `wal::commit` gives that.
- The old reason for an early sync, the leased `write_counter`, went with
  SEC-1, whose per-session random nonce prefix means an orphan page can't
  collide (the DUR-1 note in `prepare_for_flush`).
- **What it must not do:** fold this barrier into the commit's own fsync.
  Within one fsync the disk may persist the commit frame before the pages
  it points to.
- The crash tests (`failpoints`) must cover a crash between the merged
  sync and the commit, and one inside the merged sync.

This changes a write path, so it goes in only with those tests, and the
Planner may prefer to leave it for its own step.

## 4. What step 2 must reach and keep

| Number | v0.18.1 (above) | Step 2 target |
|---|---|---|
| First recall, 100K, fresh handle | 116 to 118 ms | ≤ 15 ms |
| First recall, 10K (exact) | 9.2 to 10.4 ms | ≤ 5 ms |
| Warm recall, 10K | 2.25 to 2.28 ms | ≤ 2 ms |
| Warm recall, 100K | 2.40 to 2.41 ms | no regression |
| Open, 100K | 30.2 to 30.5 ms | no regression |
| RSS of a handle, 100K | 112.5 MB | no regression (layout 2 should lower it) |
| Durable single write | 12.8 to 15.5 ms | no regression |
| ranking_v2 | 0.9231 / 0.9487 | unchanged |
| recall@10, 100K, defaults | 0.978 | unchanged (the same index, laid out differently) |
| Exact results below 20K | recall 1.0 | unchanged |
| v0.18.1 opening a layout-2 file | (n/a) | no error, exact recall; its first-recall time recorded |

The numbers go into `test/scripts/perf_v0.19.0.json`, and the README and
CHANGELOG quote only those.

## 5. Step 2's tests

- **An old-layout index is still read:** a layout-1 run (written through
  `index_layout_override`, the dev hook) gives the same recall results as
  layout 2.
- **A new-layout file opened by v0.18.1 falls back without error:** a
  compat test beside `v0_17_compat.rs`, with the published 0.18.1 binary
  in CI. It recalls, gets the exact-scan results, and writes a record,
  and v0.19 then sees that record.
- **Lazy partitions and the delta chain:**
  - adds and removes on partitions not yet loaded, then queries that load
    them;
  - reconcile of records newer than the run;
  - a full load forced by `rebuild_index` in the middle of all this;
  - the results equal a fully loaded index's.
- **Shared mode:** the reload test in 3e.
- **`read_pages`:** a page that fails authentication inside a batch; a
  short read at the end of the file.
- **If 3f goes in:** failpoint crashes around the merged sync, and a
  proptest round of batch commits with big runs.

## 6. Open questions for the Planner

1. **The order of the levers.** Measure the 100K load's split first:
   decode, the `id → partition` map, reconcile, posting bytes. If the map
   and the decode dominate, layout 2's head-only load may be enough at the
   defaults (half the partitions probed). Lazy partitions then add a
   further cut that's worth its complexity only if the split shows the
   posting bytes are a real share.
2. **3f (the fsync):** in v0.19.0 with its crash tests, or its own step?
3. **3d's parallel warm scan:** in scope only if the two single-thread
   levers miss 2 ms?

## 7. As built (step 2)

Numbers are from `test/scripts/perf_v0.19.0.json`: the same machine and
commands as section 1, with the load average logged per run (5 to 15,
from macOS background work).

**The 100K load split, before and after** (medians of fresh handles,
`examples/perf2_split.rs` through the dev hook `__index_load_split`):

| Stage, ms | v0.18.1, layout 1 | v0.19.0, layout 2 |
|---|---|---|
| Read and decrypt the run | 15.69 | 4.22 |
| Decode | 70.76 | 0.70 |
| `id -> partition` map | 1.08 (in the load) | 1.02 (only when a write needs it) |
| Delta chain | 0.00 | 0.00 |
| Reconcile | 3.32 | 3.02 |
| The first recall that follows | 30.22 | 4.19 |
| Plain first recall, for comparison | 114.98 | 11.54 |
| Run size | 17,298,907 bytes | 12,082,189 bytes |

The decode was the largest share, as section 2 guessed.

**What was built:**
- **Stage A only.** Layout 2 is loaded whole: the head, then the postings
  partition by partition, straight into their arrays. With it, first
  recall at 100K is 11.5 ms, under the 15 ms target. So stage B (lazy
  partitions) wasn't built, as the decision said, and neither was 3e's
  shared-mode test, which was only for stage B.
- **3c:** `Pager::read_pages_uncached` (one positioned read per run of
  adjacent pages, decrypted in place, on scoped threads at 64 pages or
  more) serves `RunReader`, 256 pages at a time. `RunReader` reads the
  decrypted buffer as a plain slice. Serving it through the batch's
  per-page lookup slowed the catalog decode, and with it opening a 100K
  file, by about 10 ms (36.18 against 26.98 ms), so that was changed
  before the final runs.
- **The 64-page threshold** for going parallel was chosen, not measured.
  The single-thread numbers above are the floor it protects; tuning it is
  open.
- **3b, the column fill,** is `Pager::for_each_page`. Threads are started
  once per fill, and each reads and decrypts its share of the pages 15 at
  a time (120 KiB buffers), handing them over a bounded channel. The first
  try read 256-page batches. It was fast, but the allocator kept the 2 MiB
  buffers, which added 2.4 to 3.7 MB to a 10K handle's memory. Smaller
  batches fixed the memory but started threads per batch, and first recall
  at 10K went to 7.3 ms. Streaming fixed both: 4.2 ms and 17.2 MB.
- **3d:** sixteen lanes in `dot_f16` measured no change in warm recall at
  10K (2.35 against 2.25 to 2.28 ms), so it was reverted, and the second
  lever wasn't tried. Warm recall at 10K is 2.3 ms, above the 2 ms target,
  and the time isn't in the dot product. A profile of the warm path is
  the next step if the target stays.
- **3f** is deferred (the decision), not built. (Built in v0.20.0, task
  v0200-1: see the CHANGELOG.)

**Against section 4's targets:**
- First recall at 100K: 11.50 / 11.50 ms ✓. Single-thread: 17.66 ms.
- First recall at 10K: 4.22 / 4.20 ms ✓. Single-thread: 8.19 ms.
- Warm recall at 10K: 2.35 / 2.31 ms ✗, reported as it stands.
- Everything on the must-not-regress list holds:
  - warm recall at 100K: 2.39 / 2.38 ms;
  - opening 100K: 25.78 / 25.70 ms;
  - a 100K handle's RSS: 111.6 / 110.9 MB;
  - a durable single write: 12.0 to 16.8 ms;
  - ranking_v2: 0.9231 / 0.9487;
  - recall@10 at 100K: 0.978;
  - v0.18.1 on a layout-2 file: no error, exact recall
    (`tests/v0_18_compat.rs`).
- A 1K handle's RSS is 3.7 MB, against 2.9 MB in v0.18.1.

