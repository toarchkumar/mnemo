# Incremental catch-up: the design note

Status: **draft by the Worker, 2026-11-18** (task `v0200-2`), for the
Planner's review before the build. The spec is the brain's
`spec.v0.20.0`, part (B): step 6 of `12.4-shared-access.md`'s
implementation order (incremental catch-up) and step 8's open question
(whether to gate older binaries). Section 9 is that answer.

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
rebuilt on its next use. The build measures what that costs at 100K and
makes the expensive ones incremental only if the numbers say so.

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
   state).

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
| A fallback (a base rewrite) | the catch-up row | no slower than today's full reload (within the noise of two runs) |

If dropping the lazy indexes (section 4) keeps the first recall over its
target, the build makes the costly ones incremental and says which.

**And kept:** a commit's fsyncs (3, `device_budget`); the per-call check
on an unchanged file (0.048 ms, cell 1); the write window (cell 2).

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
