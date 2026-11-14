# Shared mode (12.4)

Several processes, sessions and tools can use one brain at once in
**shared mode**. A shared handle takes the file's lock for each call and
holds nothing between calls: a read runs under the shared lock, a write
under the exclusive lock, and every write commits before it returns. Before
each call the handle catches up with other processes' commits. A plain
handle (the default) keeps the exclusive lock for its whole life, as
before.

The design, with the protocol, the crash table and the threat model, is
`docs/design/12.4-shared-access.md`.

## Before you turn it on: update every `mnemo` on the machine

An older build doesn't know about shared mode. It takes the lock the old
way, and an older `mnemo compact` releases the lock before it renames the
compacted file over the brain. A shared handle reports a write lost that way
(`ReplacedDuringWrite`) rather than acknowledging it, but the older build
can still lose its own work. **Update every engine that opens the brain**
before you use shared mode on it:
- every `mnemo` CLI;
- the `mnemo-engine` wheel in every virtualenv;
- any app built on an older binding.

## Turning it on

| Surface | How | Since |
|---|---|---|
| Rust | `Mnemo::open_shared(path, passphrase, SharedOptions::default())` (or `open_shared_with_key`) returns a `SharedMnemo`; call `read(\|db\| ...)` and `write(\|db\| ...)` | v0.14 |
| Python | `mnemo_engine.open(path, passphrase, shared=True, wait=10.0, allow_network_fs=False)` (or `open_with_key(...)`); the same methods as a plain handle, each one read or one write | v0.14 |
| CLI | Nothing to turn on: every command waits up to `--wait` (default 10 s) for the lock, reads alongside other readers, and readers come first ("The wait"). A hook that only reads should pass `--wait 2s`, so a busy file delays a session's start by at most about two seconds; a hook that saves keeps the default, because a busy file can hold a write longer than that ("How busy a file can get") | v0.14 |
| MCP server | `mnemo serve --mcp --shared <file>` holds no lock between tool calls; each call is one read or one write window, waiting up to `--wait`. `--allow-network-fs` with it. Off by default in v0.14: without `--shared` the server holds the file for its whole life | v0.14 |
| `inspect` | Its read-only open waits up to `--wait`. A pending WAL or a torn header it can't recover (it never writes) is explained: any writer's next call recovers it | v0.14 |
| Swift, Kotlin | not in v0.14 | later |

Shared mode opens an existing v9 file only: it never creates one. Create the
brain with a plain open first.

**One engine call per window.** Each surface call is one read or one write.
In Rust, keep each `write` closure to one engine call where you can. A
closure that commits part of its work itself (`flush`, `commit_deltas`,
`restore_to`) and then fails returns `MaybeStored`: part of the window is
in the file.

**The wait.** One call waits at most its deadline for the lock (`SharedOptions::deadline`,
default 10 s; Python's `wait`, in seconds) and then fails with `Locked`.
When a long operation holds the file, the error names it ("compact since
12:01:05 UTC (pid 4242, mnemo)"). **Readers come first:** reads are what an
agent waits on, so a read that finds a writer committing waits only for
that commit, and the next writer queues behind it. A waiting writer still
keeps new readers from cutting in, but readers already waiting go first. So
readers looping with no pause slow a writer down. **Since v0.15 a writer
that has waited 500 ms gets the next turn** (LOCK-2, the writer safety
valve): new readers hold back until it's in, so busy readers can't lock out
a save. That holds when every process using the file shared runs v0.15 or
later; upgrade the tools that read it too. "How busy a file can get" has
the numbers.

**Cancelling.** A `CancelToken` (`SharedOptions::cancel`, Python's
`cancel=`) cancels a call that is *waiting* for the lock, from another
thread; the call returns `Cancelled` and changes nothing. A call that holds
the lock runs to its end. A cancelled token stays cancelled: give the handle
a new one with `set_cancel`.

**Notices.** What a handle warns about once: a sidecar directory it can't
use, a synced folder, an allowed network filesystem. Rust: `notices()`.
Python: `notices()`, and each one raised as a `UserWarning` at open.
`serve --mcp --shared`: stderr, once at start.

## The long plain operations wait too

`compact_file` (and so `mnemo compact` and `mnemo rekey --rotate`) and every
CLI command take the lock through the same layer: they wait (10 s, or the
CLI's `--wait`) with writer preference, and a writer leaves a `lock-info`
note while it holds the file, so a shared caller that times out learns what
holds it. `Mnemo::open_exclusive` does the same for your own long
operations, and `Mnemo::open_read_only_waiting` for reads;
`compact_file_waiting` takes the options. A plain `Mnemo::open` still tries
the lock once and fails at once with `Locked`, and so does a `serve --mcp`
started without `--shared`.

These plain callers use the sidecar directory only if a shared handle has
already made it, so a brain nobody opens in shared mode gains no sibling.
Without it a call still waits with the deadline and the backoff, but there's
no writer preference to queue on (no shared caller exists) and no note.

A compaction also removes the temp files that compactions killed part-way
left beside the brain: `<brain>.compact-<pid>-…​.tmp` files whose process no
longer runs and that this user owns, on Unix only.

## How busy a file can get

Measured in v0.14.0's bench on an M1 Pro at 10K memories, with every
process looping as fast as it can (`shared_v0_14_0` in
`test/scripts/perf_v0.5.0.json`):
- **A call costs about 0.05 ms** when nobody else wrote since this handle's
  last call. After another process's commit, the next call reloads: about
  5 ms at 10K, 34 ms at 100K.
- **A shared write** takes about 24 ms (5 fsyncs), against about 15 ms for a
  plain write and flush. Several writers together make 15 to 20 commits a
  second per file.
- **Reads wait under 1 ms** at the 99th percentile in every mix measured,
  against one to eight writers.
- **A writer against readers looping back to back** (since v0.15, LOCK-2):
  a writer that has waited 500 ms gets the next turn, so its 99th
  percentile is about 570 ms against 7, 8 or 16 readers on cheap reads,
  with no `Locked` (v0.14: 2.9 s against seven readers, and 6 of 7 writes
  locked out against sixteen). Measured in v0.15.0's bench
  (`shared_v0_15_0`).
- **Eight writers at once:** the slowest 1 % of writes wait under 1 s. With
  eight writers and two readers all looping, the readers pay for it: a read
  can wait up to about a second (0.6 to 0.9 s at the 99th percentile;
  v0.14: under 1 ms), because a reader holds back for waiting writers for up
  to 500 ms itself before it queues.
  With many writers saving back to back on a slow machine, a read can wait
  up to about 2 s.
- **Mixing versions:** a v0.14 reader doesn't know the valve, so against
  v0.14 readers a write is only as bounded as in v0.14. Upgrade every tool
  that uses the file shared.

Agents write a few times a minute and read on demand, so these are limits,
not the usual load.

**A compaction holds the file for its whole length.** It took about 0.2 s
at 10K and about 175 s at 100K on the same machine, and every call that
waits longer than its deadline meanwhile fails with `Locked` (naming the
compaction). Compact, or rekey with `--rotate`, when no agent is working.

## What shared mode costs and assumes

- **The handle keeps its passphrase** (or raw key) in memory for its whole
  life, zeroized when it closes. Every compaction gives the file a new data
  key, and without the secret the handle couldn't follow one. A plain handle
  drops the secret once the file is open. `SharedMnemo::rekey` updates the
  kept secret.
- **A sidecar directory** next to the brain, `<brain>.mnemo-shared/`, holds
  the writer-preference lock and the long-operation note. The `.gitignore`
  pattern `*.mnemo-shared/` matches it. It's created 0700 and checked before
  every use; if it can't be used, each call takes the main lock alone.
  Compaction and the other plain operations use it only if a shared handle
  has made it.
- **Network and synced folders are refused**, because the lock may not reach
  every process that opens the file: network filesystems, and on macOS
  iCloud Drive and File Provider folders. `allow_network_fs` overrides it.
  On Linux the check is an allow-list (ext2/3/4, xfs, btrfs, tmpfs, f2fs,
  overlayfs, zfs); **overlayfs needs Linux 4.19 or later**. On Windows UNC
  paths are refused, but **a drive letter mapped to a network share may not
  be detected**.
- **Hard links aren't covered.** Two paths through a symlink find the same
  brain and the same sidecar. Two hard links to one file look like two
  brains to the sidecar, so writer preference doesn't span them (the file's
  own lock still does).
- **Windows builds need Rust 1.85 or later.** An older compiler's
  `std::fs::rename` can't replace an open file, so those builds refuse
  shared mode (`Unsupported`). Since v0.15 the engine needs Rust 1.92
  anyway, so no supported build does; the released binaries and wheels are
  built with stable.
- **Settings that act on the whole file** (snapshot retention, cache
  budgets, the cache flush policy, turn compression) keep their defaults on
  a shared handle; setting one raises `Unsupported`. Per-handle settings
  (the page cache's size, the vector cache budget, the embedder) work and
  are kept across reloads. `cache_stats` counts hits and misses since the
  handle's last reload: another process's commit makes the handle rebuild,
  and the counters start again.

## Errors

| Rust (`MnemoError`) | Python | Meaning |
|---|---|---|
| `Locked { holder }` | `LockedError` | the call waited its deadline; `holder` names a long operation when one left a note |
| `Cancelled` | `CancelledError` | cancelled while waiting; nothing changed |
| `Unsupported` | `UnsupportedError` | shared mode isn't available here (a network or synced folder, an old Windows build), or a file-wide setting on a shared handle |
| `MaybeStored` | `MaybeStoredError` | a write failed after its commit point (or after an earlier commit in the same window): it may be in the file; check before retrying |
| `ReplacedDuringWrite` | `ReplacedDuringWriteError` | something (an older compactor) renamed another file over the path during the write; the write isn't in the file now at the path |
| `Replaced { shared: true }` | `ReplacedError` (`shared` is `True`) | something (an older compactor) renamed another file over the path just before the write's commit; nothing was written, so calling again is safe, and the next call follows the new file |
| `Replaced { missing: true }` | `ReplacedError` | the brain was moved or removed: the handle's path no longer names it, and nothing was written. Move it back, or open it where it is now |
| `NeedsWriteOpen` | `NeedsWriteOpenError` | a read had to recover the file (a pending WAL, a torn header) and this process can't write it |
