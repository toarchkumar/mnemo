# Incremental catch-up: the design note

Status: **approved for the build** (task `v0200-3`) by the Planner's
`decision.catch-up-design`, 2026-11-18, with its additions (a) to (d)
folded in below. Drafted by the Worker in task `v0200-2`. The spec is the brain's
`spec.v0.20.0`, part (B): step 6 of `12.4-shared-access.md`'s
implementation order (incremental catch-up) and step 8's open question
(whether to gate older binaries). Section 9 is that answer. **Built** in
task `v0200-3`: section 8a says what was built, and gives the measurements
and which targets were met or missed.

**The constraint:** no format change. v9 already writes the catalog and
the ANN index as delta chains, and every run pointer carries the
generation of the commit that wrote it (FMT-9c). That is all this needs.

## 1. What a shared handle pays today

A shared handle (`SharedMnemo`, `store/shared.rs`) checks page 0's seal
at the start of every call. When another process has committed, the seal
differs and the handle rebuilds itself with `Mnemo::from_locked`, which is
`open` minus the KDF:
- it reads the catalog base and replays the whole delta chain;
- it rebuilds the id index, the packed-record map and the vector column's
  page map;
- it reads the snapshot manifest, the cache directory, the free list, the
  key slots and the BM25 manifest;
- it builds a new `Pager`, so the page cache starts empty;
- it drops everything built lazily: the resident vector column, the ANN
  index, the key and supersession indexes, the layers, the BM25
  dictionaries.

So the next recall pays for a fresh handle's first recall as well.

### 1a. The baseline (v0.19.1)

**Where it was measured:**
- Apple M1 Pro (8 cores, 16 GB), release build, with the repository's
  `.cargo/config.toml` (hardware AES).
- The engine at the `v0.19.1` tag (`git archive v0.19.1`), corpora built
  fresh by it: `benches/corpus.rs`, 384 dimensions, the fast KDF, an
  IVF+PQ index at 100K.
- A scratch example, `catchup_bench` (section 8 says where it goes). It
  needs the `failpoints` feature for `SharedMnemo::__reloads`, which
  checks that every call after a commit really reloaded.
- **Load:** 2.8 to 3.6 during the two runs below (`uptime` before and
  after each), with other sessions working on the machine.

**The loop**, in one process: two shared handles `a` and `b` on one copy.
`a` is warmed with 20 recalls (top 10, no access tracking). Then, N
times: `b` commits one `remember`; `a` runs an empty read window (the
catch-up alone); `a` recalls; `a` recalls again. A second loop measures a
recall as the first call after `b`'s commit, which is what an MCP recall
pays.

Two runs, each size in its own process: run A with N = 200 at 10K and
100 at 100K, run B with N = 100 at both (which also logged the physical
footprint). p50 / p99 in ms; `__reloads` counted one reload per
commit in every run.

| v0.19.1, shared handle | 10K, run A | 10K, run B | 100K, run A | 100K, run B |
|---|---|---|---|---|
| **The catch-up** (an empty read window after the commit) | 3.78 / 4.39 | 3.56 / 4.23 | 30.74 / 32.32 | 30.80 / 33.67 |
| The first recall after it | 4.92 / 5.88 | 4.82 / 5.36 | 24.05 / 32.59 | 24.67 / 35.67 |
| The two together | 8.68 / 9.85 | 8.42 / 9.20 | 54.93 / 63.78 | 55.18 / 67.98 |
| **A recall as the first call after the commit** | 11.25 / 14.58 | 11.67 / 16.00 | 64.55 / 73.90 | 64.61 / 73.73 |
| The second recall after it | 2.82 / 3.35 | 2.71 / 3.06 | 3.31 / 6.19 | 3.39 / 5.83 |
| A warm recall, no commit in between | 2.43 / 2.62 | 2.41 / 2.49 | 2.69 / 4.14 | 2.75 / 4.04 |

Step 5 measured the catch-up at 5.2 ms (10K) and 34.1 ms (100K) on
v0.14.0, with its own corpus; the empty-window rows here are the same
measurement on v0.19.1.

**Memory.** Every reload builds a new state and frees the old one, and
on macOS the freed memory stays in the process. In run B the physical
footprint (`vmmap --summary`) went from 18.6 MB to 72.4 MB over 100
reloads at 10K, and from 127.2 MB to 599.6 MB at 100K, about 4.7 MB per
reload, rising in a straight line with no plateau. It stayed at that
level after both handles were dropped. `leaks` found no leaked bytes, and
`vmmap` showed 617 MB of it as `MALLOC_LARGE (empty)` in 24 regions, in a
separate 60-reload run at 100K: freed large blocks the allocator keeps
rather than live data. Whether it ever stops growing wasn't measured past
100 reloads, and Linux wasn't measured. The `mnemo serve --mcp --shared`
processes on this machine (v0.18.1, a brain of about 11K records) had an
RSS of 3 to 90 MB after a day.

**So:** a shared handle at 100K pays about 31 ms to catch up after every
commit by another process, then about 25 ms more on its first recall,
because the reload threw away the vector column and the index. An MCP
recall that arrives after another session's commit takes about 65 ms
instead of about 3. At 10K the same is about 11 ms instead of about 2.4.
And on macOS each reload leaves its old state's large blocks resident.

## 2. What changes between two commits

A commit by another process changes, in the header:
- `wal_seq` (the transaction id) and the seal, always;
- the catalog's delta head, or its base (section 6);
- the index's delta head, or its base;
- the snapshot manifest, rewritten at every commit that records a
  snapshot;
- the free list, whenever the commit freed or reused pages;
- sometimes the cache directory, the key slots and the BM25 manifest.

A shared write window also takes a fresh generation lease first, which is
a header-only transaction (`ensure_gen_lease`). Readers never see that
state on its own, because the writer holds the exclusive lock through
both, but it means `wal_seq` can move by more than one per window.

## 3. Finding the deltas newer than the handle's transaction

The handle holds the header of the state it built, `H_t`, where `t` is
its `wal_seq`. It reads the new header `H_n` (it already does, to compare
seals) and checks, in order:

1. **The same file.** The stamp's identity (device and inode on Unix) is
   unchanged, and `H_n`'s seal verifies under the data key the handle
   holds. Otherwise: a compaction, a rekey with a new data key, or a copy
   over the file. Full reload.
2. **The same catalog base.** `H_n`'s base pointer (`catalog_start`,
   `catalog_pages`, `catalog_len`, `catalog_gen`) equals `H_t`'s.
   Otherwise the base was rewritten (section 6). Full reload.
3. **The chain reaches back to `t`.** Walk `H_n.catalog_delta` back
   through each delta's `prev`. Every run pointer carries the generation
   its writer sealed it under (`write_gen`). A writer takes its lease
   (a header-only transaction) only after it has caught up, so every run
   committed after `t` has a generation above `t`, and the walk stops at
   the first pointer whose generation is at most `t`. That delta must be
   exactly `H_t.catalog_delta` (all four fields), or the walk must end at
   an empty pointer when `H_t`'s head is empty. Otherwise: full reload.
   The deltas the walk read, oldest first, are the new ones.

The index chain is checked the same way, with one difference: the index is
derived data and loads lazily. When its base changed, or its chain doesn't
reach back to the handle's head, the handle drops the index it holds
(back to "not loaded") and the next recall loads it. That never forces a
full reload.

**Why the generation is enough.** Two different committed runs can't share
a start page and a generation: every write window takes a generation no
other writer has used (`ensure_gen_lease`, RB-006.11, RB-007.6), and a page
is reused only after the commit that freed it. So pointer equality means
the same bytes, and the walk never mistakes a reused page for the run the
handle knew. The handle never has to trust the free list to say which
pages were reused, which it can't.

The walk reads only the new deltas. The pointer it stops at is compared,
never read.

## 4. Applying them

The checklist is what `apply_snapshot` (`store/snapshots.rs`) already
resets when a catalog is replaced, done row by row instead of wholesale.

**The catalog.** Apply each new delta's rows as `apply_delta_chain` does
(a position below the length replaces a row, one at the length appends,
anything past it is an error and a full reload). Collect the set `C` of
positions that changed, with each row's old entry (if any) and new one.
Append the new deltas to `delta_chain` and add their rows to `delta_rows`,
so the handle's own next commit makes the same delta-or-base choice a
fresh open would.

**Built from the catalog, per row in `C`:**
- the id index (`IdIndex`): drop the old id, insert the new one;
- the packed-record map (`RecPages`): move the row from its old page to
  its new one;
- the vector column (below).

**Built lazily, dropped when `C` isn't empty:** the key index, the
supersession index, the layers and the agent set (`key_index`,
`superseded`, `layers`, `agent_set`), as `apply_snapshot` does. Each is
rebuilt on its next use. The build measures the first-recall cost of
each one dropped, at 100K, and reports it before choosing which to make
incremental (addition (d)).

**The small runs:** the snapshot manifest, the free list, the key slots,
the cache directory (and its `DirectoryIndex`) and the BM25 manifest are
each reread when, and only when, their pointer in `H_n` differs from
`H_t`'s. When the BM25 pointer changed, the BM25 dictionaries and
statistics are dropped too (they load lazily).

**The ANN index**, when it's loaded and its chain reaches back: apply the
new deltas' ops (`apply_ops`), oldest first, and extend `index_chain` and
`index_chain_ops`. Otherwise it goes back to "not loaded" (section 3).

**The header:** `H_n` replaces `H_t`, and the stamp is taken from it, as
`install` does today. Per-process settings (the page cache's capacity,
the vector cache budget, the embedder, the author, presence) stay where
they are, since the handle isn't replaced.

### 4a. The vector column

The resident column holds decoded fp16 vectors by catalog row, filled one
vector page at a time on first use, with `have` marking the filled rows
and `page_rows` mapping each vector page to its live rows. Each row names
its vector page, slot and the page's generation (`RowV9`: `vec_page`,
`vec_slot`, `vec_gen`).

A vector page is never rewritten in place under the same generation, so a
row whose catalog entry didn't change still names the same vector bytes.
So only rows in `C` can be stale. For each one:
- clear `have[i]`, remove `i` from its old page's `page_rows` list, and
  add it to its new page's;
- a deleted or superseded row stays unfilled (recall skips it already);
- an appended row grows `data`, `norms` and `have` (zeroed, unfilled), as
  `ensure_column` sizes them;
- the next recall fills it from its page.

When the column would now be over its budget (`scored_rows` against
`vector_cache_budget`), drop it, as a fresh open would decline to build
it.

### 4b. The page cache

The cache holds decrypted pages keyed by page number only, and a hit
returns before the generation is looked at (`Pager::read_page`,
`read_page_uncached` and the batch reads all `peek` by page number). That
is why today's reload has to start a new cache: a page another process
freed and reused would come back with its old content.

**The rule: key the cache by page and generation.** Each cached page keeps
the generation it was read or written under (the pager already tracks it
for dirty pages, in `dirty_gens`). A lookup passes the generation its
pointer expects, which every caller already has, and a different
generation is a miss that drops the stale entry. Then the cache stays
valid across a catch-up with no bookkeeping, a reused page can never be
served stale, and an exclusive handle behaves as before (it never sees a
generation change under it).

The alternative the 12.4 note allows, dropping every page a changed row
names (old and new), also has to cover the control-plane runs and pages
reused through the free list. Every row would have to name every page it
touches, and nothing would check that it did. The key is one `u64` per
cached page and one compare per hit, so the build takes the key.

## 5. The fallback

The incremental path never fails a call. Anything it can't prove goes to
today's full reload, which stays as it is:
- a different file identity, or a seal that doesn't verify under the held
  key;
- a pending WAL or a torn page 0 (already reloads after the replay or the
  heal);
- a changed catalog base, or a chain that doesn't reach back to `t`;
- a delta that doesn't read, decode or apply;
- **no trusted state:** after a failed or refused write the handle's stamp
  is cleared (protocol step 7), because its memory may hold the closure's
  half-done changes. Then there's no `H_t` to diff against. That stays a
  full reload, as in step 5 ("the next read after a refused write").

## 6. How often the fallback runs

A commit writes a new catalog base instead of a delta
(`catalog_as_delta`, `store/records.rs`) when:
- the chain has 64 runs;
- the chain plus this commit's rows would be more than a quarter of the
  catalog;
- a restore (`apply_snapshot`) happened.

When it writes a new base and deleted rows are more than a quarter of the
catalog, it also drops them first (`compact_tombstones`), so rows move.
The reader falls back for a new base either way. The index follows the
same rules for its own chain (`plan_index_v9`), plus a build, a rebuild or
a drop, and a new index base only unloads the reader's index.

So with one-memory commits, a reader falls back about once per 64 commits,
and a batch of more than a quarter of the catalog falls back every time.

A middle path is possible and left out of the first build: on a base
change without moved rows, read the new base and diff it row by row
against the held catalog, keeping the column and the cache. It saves the
column refill, not the catalog read. Build it only if the measured
fallback rate makes it worth it.

## 7. The tests

**Equivalence.** After any catch-up, the handle's state equals a fresh
`from_locked` of the same file: the catalog, the id index, the
packed-record map, the header, the manifest, the free list, the key
slots, the cache directory, the BM25 manifest, the delta chain and its
row count, the vector column's filled rows (the same values as the fresh
handle's after it fills them), the ANN index when loaded (same results on
a fixed query set), and recall results. A dev-hooks counter
(`SharedMnemo::__catch_ups() -> (incremental, full)`) lets each case
assert which path ran. The cases:
1. one `remember` by another handle; a batch; an `update` (a supersede);
   a `forget` (a tombstone); a cache put; a key-slot change (`rekey` with
   the same data key);
2. **a base rewrite**: 64 one-row commits, then one more (full reload,
   asserted by the counter); and a batch past a quarter of the catalog;
3. **a tombstone compaction**: enough deletes that the next base drops
   rows and rows move (full reload); and `compact_file` (a new file, so
   the identity check falls back);
4. **a page reused between the two points**: the reader caches a record
   page and fills a vector page; the writer forgets those rows, commits
   until the freed pages are eligible, and writes new rows that take
   them; the reader catches up and must read the new content (this is
   the case the page-cache key exists for);
5. the index: a delta applied to a loaded index; a rebuild (the index
   drops to "not loaded"); an index chain past 64 runs;
6. a refused write, then another handle's commit (full reload: no trusted
   state);
7. **several commits behind** (addition (a)): the reader misses commits
   from two different writers, with a lease-only transaction between
   them, so the walk covers more than one delta; and a reader behind
   across a base rewrite followed by more deltas (full reload);
8. **a retried write on the writer's own handle** (addition (b)): a write
   fails before its commit point, and the retry rewrites the same page
   under the same generation (v0200-1r's restore path); the page cache
   must hold the retry's bytes.

**The proptest model** (`tests/proptest_store.rs`) gets a second shared
handle: random interleavings of the two handles' writes and reads, and
after each read window the reader's state equals a fresh open.

**Crash injection** (the existing `__failpoints` hooks): a writer that dies
at each write of its commit, then a reader's catch-up, which must reach the
replayed state (it takes the full reload through the replay) and never
an earlier or half state.

**The page-cache key alone**, on an exclusive handle: the existing suite
passes unchanged, and a unit test in `cache.rs` checks that a different
generation is a miss.

## 8. The targets

Measured by `shared_bench` cell 4 (the call after another process's
commit) and a new cell 10, the `catchup_bench` loop in section 1a, which
the build adds to `shared_bench`. Same machine, the same corpora, load
logged.

Against section 1a's baseline (p50, after a one-memory commit by
another handle, at 10K and 100K):

| | Baseline, 10K / 100K | Target |
|---|---|---|
| The catch-up (an empty read window) | 3.6 / 30.8 ms | **at most 1 ms** at both |
| The first recall after it | 4.8 / 24.7 ms | **at most 1.5 times a warm recall** (2.4 / 2.7 ms today) |
| A recall as the first call after the commit | 11.7 / 64.6 ms | **at most 5 ms** at both |
| Physical footprint after 100 catch-ups | +54 / +472 MB | **under +10 MB** at 100K |
| Physical footprint over 500 one-memory commits (about 8 fallbacks) | not measured | **under +20 MB** at 100K (addition (c)) |
| A fallback (a base rewrite) | the catch-up row | no slower than today's full reload (within the noise of two runs) |

If dropping the lazy indexes (section 4) keeps the first recall over its
target, the build makes the costly ones incremental and says which.

**The footprint (addition (c)).** `fact.shared-reload-footprint` is
folded into this build. The 500-commit run includes the fallbacks, so it
shows whether the full reload alone still grows about 4.7 MB per reload
with no plateau. If it does, the report says so, and the Planner decides
whether that gets its own task.

**And kept:** a commit's fsyncs (3, `device_budget`); the per-call check
on an unchanged file (0.048 ms, cell 1); the write window (cell 2).

## 8a. As built (v0200-3)

**What the build does.** `SharedMnemo::catch_up` (`store/shared.rs`)
tries `Mnemo::catch_up_to` (`store/catchup.rs`) whenever the file is the
one the handle was built from and the new seal verifies under its key.
That runs as sections 3 and 4 say, with these choices:
- **It applies only a catalog delta chain on the same base**, with a
  handle that has nothing waiting to commit (`clean_for_catch_up`).
  Anything else returns `Ok(false)`, and an error after it started
  changing the handle clears the stamp, so the reload that follows keeps
  nothing of it.
- **The lazy parts it drops** when rows changed are the key index, the
  superseded set, the layers and the agent set. Addition (d) below showed
  them cheap. It keeps and updates the BM25 statistics and dictionaries,
  the vector column (moved rows lose their filled entry) and a loaded
  ANN index.
- **The ANN index**: the new index deltas, then each changed row as
  `reconcile_ann_with_catalog` would treat it on a fresh open. A
  repacked row whose vector is unchanged is skipped only when a fresh
  open would skip it too: the row was written after the index under both
  the old and the new index generation, and its fp16 entry is bit for bit
  the same. An entry from the index build isn't equal to `add` of the
  same vector (k-means assignment against the nearest centroid), so a
  row the build placed is always re-added. Without the skip, a catch-up
  at 100K cost 4 to 9 ms, re-adding the 25 to 65 rows each commit repacks.
- **The fallback keeps the column.** On a full reload of the same file
  from a trusted state, the new state takes the old one's vector column
  (`adopt_column`), keeping each row whose vector entry is where it was.
  This is part of section 6's middle path. The rest (diffing the new base
  against the held catalog) isn't built.
- **The vector column leaves room for an eighth more rows** (mapped
  lazily, `ensure_column`), so the rows that later commits add grow it in
  place.

**The tests.** `tests/catch_up.rs` has 9 cases (13 with v0200-3r's four
below), covering section 7's 1 to 8 and the crash injection: a writer
that dies at each of its 11 writes,
then a catch-up that must reach a state equal to a fresh open. Each case
checks equivalence part by part and which path ran. The proptest's second
shared handle (`prop_two_shared_handles`) checks the reader against a
fresh open after each read window. Mutations that each made a test fail:
- dropping the "re-added under both generations" rule, or treating fp16
  entries as always equal (`the_index_follows`);
- `adopt_column` keeping every row (`tombstones_and_compaction_fall_back`);
- the cache ignoring the generation, skipping the `have` clearing, and
  skipping the BM25 statistics.

**A copy restored in place (v0200-3r).** An older copy of the file
written over it in place (`cp` keeps the inode) has the file's identity
and its key, so it passes the identity and seal checks. Its next commits
reuse page numbers and generations: the free list hands out the same pages
again, and generations follow `wal_seq`. So a pointer the reader holds can
come back exactly, with other bytes behind it. The walk back then stops at
what looks like the reader's head, and the page cache and `adopt_column`
could keep bytes from the timeline that was overwritten. The tests show
all three: a catch-up that applied only the last delta, a column row kept
from the old timeline, and a result-cache value served from the page
cache.

The guard needs no format change. When a shared handle takes a state
(a load, a catch-up, its own commit), it reads the on-disk nonce of the
first page of every run its header points at (`Anchors` in
`store/shared.rs`). Before a catch-up, or a reload that keeps the column,
it checks two things:
- **The transaction didn't go back:** the new `wal_seq` is at least the
  held one.
- **Every held run the new state keeps is still on disk, read past the
  cache.** These are the catalog base and head, a loaded index's base and
  head when the new header keeps its base, and every other run whose
  pointer the new header still has.

Every run is written whole in one commit, and SEC-1 gives each session
its own random nonce prefix. So a run written again, in any timeline, has
another nonce. A run the new header no longer points at isn't checked:
nothing of it is kept, and on the reader's own timeline its pages may be
in use again. That's why the reused-page case (section 7, case 4) still
catches up incrementally.

When the check passes, every run it checked predates any copy that might
have been written over the file. So the handle also drops cached pages
newer than the newest of those runs, and BM25 dictionaries keyed by such a
segment. On its own timeline those are the few pages of commits since its
last catalog change. When the check fails, the handle does the full
reload, with a fresh page cache and no `adopt_column`.

**The guard's limit (RB-016.3).** It assumes the copy replaced the whole
file. `cp`, `std::fs::write` and a snapshot rollback do: each cuts the file
to the copy's length, so a held run past the copy's end reads as nothing or
zeros and fails the check. A copy written over the head of the file without
cutting it (`dd conv=notrunc`, a block-level restore) leaves the overwritten
timeline's tail on disk. A held run that lies in that tail keeps its nonce,
so the check passes, and a reader can keep column rows and cached pages that
a fresh open doesn't have (REVIEWER-B's test showed it). There's no guard for
it: one would need something per commit that can't repeat, which is a
format change. The rule instead, in `docs/shared-mode.md`: restore a copy by
replacing the file, not by writing it over the file in place.

The cost is two short reads per checked run (at most nine runs), one when
the state is taken and one at the check. Over two alternating runs of
cell 10 (load 2.9 to 5.0, p50), the catch-up took 0.450 and 0.403 ms at
10K with the check, against 0.455 and 0.413 without it, and 0.808 and
0.824 ms at 100K, against 0.802 and 0.846. That's within the noise.

`tests/catch_up.rs` has four cases. Each first asserts that the pointers
really coincide (the held head is in the restored chain, the vector
entries or the cache value page are at the same page and generation),
then requires the full path, or the incremental path with the right
bytes, and a state equal to a fresh open:
- the catalog: the walk back;
- a new base: `adopt_column`;
- a result-cache put only, where the catalog head alone wouldn't tell;
- a run that moved: the page-cache drop.

Mutations, each failing the cases it should:
- no on-disk check (all four);
- checking the catalog's runs only (the cache-put case);
- no page-cache drop (the moved-run case).

**Where it was measured.** The same M1 Pro, release builds, `shared_bench`
cells 10 to 12 and 1 to 3 (`--features failpoints` for the path counter).
The corpora were built fresh by the build under test (`CARGO_TARGET_DIR`
pointing at an empty directory; an older cached corpus has a layout-1
index, which loads in about 87 ms at 100K against about 9.5 ms). Each size
ran in its own process, two runs (A, B). Load: 3.0 to 4.3 for cells 10
to 12 and 1 to 3; 2.7 to 7.4 for the two long runs; 5.0 to 11.9 for the
v0.19.1 long run. Cell 10 makes 200 one-memory commits by handle `b`,
with 100 in each loop; the long run adds 500 more. p50 / p99 in ms.

| After a one-memory commit by another handle | 10K, A / B | 100K, A / B | v0.19.1, 10K / 100K | Target |
|---|---|---|---|---|
| The catch-up (an empty read window) | 0.39 / 0.44 | 0.83 / 0.82 | 3.6 / 30.8 | at most 1 ms: **met** |
| The first recall after it | 5.80 / 6.74 | 5.80 / 5.93 | 4.8 / 24.7 | at most 1.5 times warm: **missed** |
| The second recall after it | 5.72 / 5.87 | 5.19 / 5.23 | 2.7 / 3.4 | |
| A recall as the first call after the commit | 10.6 / 11.0 | 8.16 / 8.19 (p99 66 / 69) | 11.7 / 64.6 | at most 5 ms: **missed** |
| A warm recall, no commit | 2.43 / 2.41 | 2.82 / 2.77 | 2.4 / 2.7 | |
| The control: a warm recall after a 20 ms sleep, no commit | 11.8 / 16.0 | 17.5 / 18.3 | not measured | |
| Warm recalls after all the catch-ups, no commit | 2.41 / 2.45 | 2.21 / 2.22 | | |
| Catch-ups, incremental / full | 394 / 7 | 197 / 4 | all full | |

**The recalls after a commit miss their targets.** The cause isn't
proven, but the control row suggests the machine more than the engine.
On this machine, a recall after any pause of about a commit's length is
slower, with no commit and no catch-up at all, and spinning instead of
sleeping didn't help. The caught-up state itself recalls as fast as the
warm one (the "after all the catch-ups" row). What isn't explained is
the second recall: v0.19.1's ran at 2.7 ms at 10K, against 5.7 to 5.9 ms
here. At 100K, the p99 of a direct recall is the fallbacks.

**The fallback.** Over the long run at 100K (two runs, 7 fallbacks
each), against v0.19.1 on the same shape (200 commits, then 500, every
call a reload):

| 100K | This build, run 1 / run 2 | v0.19.1 (n = 500) |
|---|---|---|
| The full catch-up (the reload) | 37.3 / 38.3 (max 43 / 48) | 27.6 (p99 32.8) |
| The recall after it | 73.6 / 73.0 | 73.6 (p99 103.9) |
| An incremental catch-up, after cell 10's 200 commits | 1.18 / 1.22 (p99 3.2 / 3.4) | |
| The recall after an incremental one | 5.6 / 6.8 | |

**The fallback misses "no slower than today" by about 10 ms.** The time
is in the reload itself (`from_locked`, 28 to 38 ms); `adopt_column` and
the swap take about 1 ms each. Probes of process CPU time (`ps`, 10 ms
resolution) showed about the same user time as v0.19.1 (about 30 against
28.9 ms) and more system time (about 10 against 4.8 ms per reload).
That fits fresh page faults: v0.19.1 reuses the freed blocks its earlier
reloads left resident, and this build doesn't keep them. The recall
after a fallback costs what v0.19.1's does. Most of that is the persisted
index's re-adds: the bench's writer never loads the index, so every
memory written since the corpus was built is re-added on each load.

The incremental catch-up also rises over the long run, to about 1.2 ms
(p50) after 200 commits, against 0.8 ms in cell 10. Why wasn't measured.

**The footprint** (`vmmap --summary`, physical footprint, MB; both
handles live in one process, as in section 1a). Run A's start includes
building the corpus in the same process, so only run B is shown for
cell 10:

| | 10K | 100K | Target |
|---|---|---|---|
| Cell 10, 200 commits (201 catch-ups) | 18.6 → 36.9 | 131.3 → 240.9 | under +10 MB after 100: **missed** |
| What the handle holds (the budget report), same span | 9.6 → 11.0 | 107.2 → 129.8 | |
| Cell 12, recalls alone, no commit | 15.3 → 16.1 | 111.8 → 111.8 | |
| The long run, 500 more commits (11 fallbacks), three runs | | 167.0 → 262.4, 168.2 → 244.8, 165.1 → 225.5 | under +20 MB: **missed** |
| v0.19.1, the same shape | | 1,433.6 after 300 commits, 1,126.4 after 700 | |
| v0.19.1, 100 reloads (section 1a) | +54 | +472 | |

**Addition (c): does the fallback alone still grow about 4.7 MB per
reload with no plateau?** No. In the long runs, the footprint steps up
at fallbacks by about 2 to 34 MB (largest at the first; the series is
every 25 commits), stays flat between them, and falls back twice: 272.0 to 244.8 and 250.5 to 224.8.
Net, that's about 5 to 9 MB per fallback over 500 commits. Whether it levels
off past 500 commits wasn't measured. What remains, from the budget report and
`vmmap`, is capacity the allocator keeps. The first catch-ups after a
load double the capacity of the catalog, the ANN lists and the id map
(live data rose 22.6 MB over cell 10), and each fallback builds a new
state beside the old one. Carrying more of the state across a reload
(section 6's middle path) is the next step if the Planner wants the
target. It isn't built.

**Addition (d): the first recall after each lazy part is dropped**
(cell 11, p50 ms, 20 recalls each; "none" ran first and includes the
warm-up):

| Dropped | 10K, A / B | 100K, A / B |
|---|---|---|
| none | 2.45 / 2.46 | 3.45 / 3.51 |
| key_index | 2.33 / 2.45 | 3.35 / 3.31 |
| superseded | 2.41 / 2.35 | 2.54 / 2.64 |
| layers | 2.32 / 2.45 | 2.56 / 2.47 |
| agent_set | 2.40 / 2.34 | 2.39 / 2.28 |
| bm25_stats | 2.34 / 2.45 | 2.27 / 2.35 |
| bm25_dicts | 2.41 / 2.45 | 2.18 / 2.13 |
| ann | 2.33 / 2.35 | 9.54 / 9.41 |
| column | 4.20 / 4.20 | 4.51 / 4.47 |

The four parts the catch-up drops cost at most about 0.6 ms over a warm
recall at 100K, so none was made incremental. The two that cost more (the
index, the column) are kept.

**Kept.** A commit's fsyncs: 3 (`device_budget`, single and batch;
cell 2's plain write averages 3.01 / 3.02). The per-call check on an
unchanged file: 0.039 ms at 10K and 0.037 at 100K (cell 1; 0.048
before). The shared write window: 24.9 ms at 10K and 32.0 at 100K
(cell 2, 5 fsyncs; "about 24 ms" at 10K in `docs/shared-mode.md`).

## 9. Step 8: gating older binaries

**The question.** Step 8 makes `mnemo serve --mcp` shared by default.
`mnemo onboard` and `mnemo mcp install` already write `--shared`, so the
change reaches only configurations written by hand. 12.4's "Things (A)
refuses" lists the one unsafe pairing with older builds: **a pre-0.14
`mnemo compact` releases its lock before it renames**, so a shared writer
can commit into the old file in that gap and the write can be lost. Every
other older build only holds the lock longer, which makes shared callers
wait and fail with `Locked`, loudly and safely.

**Who could do that.** v9 has been the default since v0.11.0, so the
builds that open a v9 file read-write and have the gap are v0.11.0 to
v0.13.2. v0.10 and older can't open v9 at all.

**The only gate they obey is the format version.** A v0.11 to v0.13 build
refuses a file whose version is above 9. But so does every build from
v0.14 to v0.19, which are safe, including the mobile packages in shipped
apps (0.19.0). A gate would mean a v10 that changes nothing but the
number, an upgrade step for every file, and every older tool and app
locked out, to close a gap that needs an old compaction to run during a
shared write.

**Recommendation: don't gate; no format change.** Instead:
- the step 8 release notes repeat v0.14's: update every `mnemo` on the
  machine (CLIs, wheels in every virtualenv, apps built against older
  bindings) before relying on shared mode;
- a write lost to that gap is never acknowledged: the shared writer's
  check after each commit (protocol step 8) returns
  `ReplacedDuringWrite`, and a rename before the commit is refused. CI's
  `old compactor (v0.13.2, linux)` job runs the released v0.13.2
  `mnemo compact` against three shared writers on every PR and asserts
  exactly that (`the_0_13_2_compactor_never_loses_an_acknowledged_write`,
  `tests/cross_process.rs`);
- `--exclusive` keeps today's lifetime lock for anyone who wants it.

If the Planner or the user wants a gate anyway, it's a format change, and
it goes to the user first (`spec.v0.20.0`).
