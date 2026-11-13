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
| CLI | Nothing to turn on: every command waits up to `--wait` (default 10 s) for the lock, reads alongside other readers and writes with writer preference. A session hook should pass `--wait 2s`, so a busy file delays a session's start by at most about two seconds | v0.14 |
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
12:01:05 UTC (pid 4242, mnemo)"). Writers have preference: busy readers
can't keep a writer out.

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
  shared mode (`Unsupported`). The released binaries and wheels are built
  with stable.
- **Settings that act on the whole file** (snapshot retention, cache
  budgets, the cache flush policy, turn compression) keep their defaults on
  a shared handle; setting one raises `Unsupported`. Per-handle settings
  (the page cache's size, the vector cache budget, the embedder) work and
  are kept across reloads.

## Errors

| Rust (`MnemoError`) | Python | Meaning |
|---|---|---|
| `Locked { holder }` | `LockedError` | the call waited its deadline; `holder` names a long operation when one left a note |
| `Cancelled` | `CancelledError` | cancelled while waiting; nothing changed |
| `Unsupported` | `UnsupportedError` | shared mode isn't available here (a network or synced folder, an old Windows build), or a file-wide setting on a shared handle |
| `MaybeStored` | `MaybeStoredError` | a write failed after its commit point (or after an earlier commit in the same window): it may be in the file; check before retrying |
| `ReplacedDuringWrite` | `ReplacedDuringWriteError` | something (an older compactor) renamed another file over the path during the write; the write isn't in the file now at the path |
| `NeedsWriteOpen` | `NeedsWriteOpenError` | a read had to recover the file (a pending WAL, a torn header) and this process can't write it |
