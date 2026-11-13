# Memory Nemo (MNemo)

[![CI](https://github.com/toarchkumar/mnemo/actions/workflows/ci.yml/badge.svg)](https://github.com/toarchkumar/mnemo/actions/workflows/ci.yml)
[![PyPI](https://img.shields.io/pypi/v/mnemo-engine)](https://pypi.org/project/mnemo-engine/)
[![crates.io](https://img.shields.io/crates/v/mnemo-engine)](https://crates.io/crates/mnemo-engine)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)

**Encrypted memory and cache for on-device AI agents, in one file, from desktop apps to phones. Written in Rust.**

MNemo gives an agent durable memory and a cache for expensive calls, inside a
single encrypted `.mnemo` file. There's no server or vector database to run:
copy the file and the memory comes with it.

- **Memory that ranks like an agent thinks.** Four memory types (episodic,
  semantic, procedural, working), recalled on similarity, recency, importance
  and use, with per-agent scoping and expiry.
- **Text in, text out.** Hand MNemo plain text; it calls your embedding model
  and checks it's the model the file was built with.
- **A result cache** for LLM responses, tool outputs and API calls: exact keys
  or near-identical prompts, with namespaces, expiry and size limits.
- **An MCP server built in**, so Claude Desktop, Cursor or any MCP client can
  use the file as tools.
- **Encrypted and crash-safe.** Every page is sealed with AES-256-GCM; commits
  go through a write-ahead log; recent commits are kept as restorable
  snapshots; OS file locks allow one writer and many readers.

Site: **https://toarchkumar.github.io/mnemo/**

## Install

```sh
pip install mnemo-engine          # Python (import mnemo_engine)
cargo add mnemo-engine            # Rust library (use mnemo_engine::...)
cargo install mnemo-engine        # the `mnemo` CLI and MCP server
```

iOS and Android (from v0.12.0; current 0.13.0):

```swift
// Swift Package Manager (Xcode: File > Add Package Dependencies)
.package(url: "https://github.com/toarchkumar/mnemo-swift", from: "0.13.0")
```

```kotlin
// Android, from Maven Central (arm64-v8a and x86_64)
implementation("io.github.toarchkumar:mnemo-android:0.13.0")
```

The package is `mnemo-engine` everywhere, and from v0.7.0 the Python module and
Rust library are `mnemo_engine` too. `mnemo-db` on PyPI and `mnemo` on
crates.io are unrelated projects.

Prebuilt CLI binaries for Linux, macOS and Windows are on the
[Releases](https://github.com/toarchkumar/mnemo/releases) page. The PyPI wheels
and release binaries for Apple Silicon and ARM Linux use the CPU's hardware AES.
If you build from crates.io on ARM yourself, see
[enabling hardware AES](mnemo/README.md).

## Quick start

Python, with any embedding model:

```python
import mnemo_engine
from sentence_transformers import SentenceTransformer

MODEL = "sentence-transformers/all-MiniLM-L6-v2"
model = SentenceTransformer(MODEL)
embed = lambda texts: model.encode(texts, normalize_embeddings=True).tolist()

with mnemo_engine.open("agent.mnemo", "a long passphrase", dimensions=384) as db:
    db.set_embedder(embed, name=MODEL, dimensions=384)
    db.remember_text("the user prefers dark mode", "semantic", importance=0.8)
    for hit in db.recall_text("which theme does the user like?", top_k=5):
        print(round(hit["score"], 3), hit["content"])

    # A fact that changes: the key replaces the old version, and history is kept
    db.remember_text("the current release is v0.11.0", "semantic", key="current-release")
```

CLI, with any program that turns text into vectors (a reference script is in
[`mnemo/examples/embedder_server.py`](mnemo/examples/embedder_server.py)):

```sh
export MNEMO_PASSPHRASE='a long passphrase'
mnemo init agent.mnemo --dimensions 384
mnemo remember agent.mnemo --text "the user prefers dark mode" \
  --embed-cmd "python3 embedder_server.py" --embed-name sentence-transformers/all-MiniLM-L6-v2
mnemo recall agent.mnemo --text "which theme does the user like?" \
  --embed-cmd "python3 embedder_server.py" --embed-name sentence-transformers/all-MiniLM-L6-v2
```

MCP (for example, Claude Desktop's `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "mnemo": {
      "command": "mnemo",
      "args": [
        "serve", "--mcp", "/absolute/path/to/agent.mnemo",
        "--embed-cmd", "python3 /absolute/path/to/embedder_server.py",
        "--embed-name", "sentence-transformers/all-MiniLM-L6-v2"
      ],
      "env": { "MNEMO_PASSPHRASE": "your-passphrase-here" }
    }
  }
}
```

The full Rust API, every CLI command, the cache, the file format, durability
and the security model are documented in [`mnemo/README.md`](mnemo/README.md).
The Python API is in [`mnemo-python/README.md`](mnemo-python/README.md).

## What's new in v0.13.2: a failed passphrase change changes nothing

- **A failed `rekey` leaves the old passphrase or key in force.** Before,
  a passphrase change that failed partway (on a full disk, say) could
  still switch the file to the new secret at the next save. Now an error
  means the old secret still opens the file (the engine README covers the
  one rare exception). Once the change has
  taken effect, a problem tidying up afterwards is reported as a warning,
  not as a failure.
- **`mnemo inspect`'s printed link opens one tab.** The page trades it for
  a private session on first load, so a copy of the link in your browser's
  history opens nothing. A refresh keeps working; another tab needs a new
  link. After the inspector stops, the page says so and how to restart it.
- **Clearer advice for damaged old files:** a v8 file with a damaged
  header now points straight at `mnemo compact`, which repairs and
  upgrades it.
- The iOS and Android packages are unchanged and stay at 0.13.0.

## What's new in v0.13.1: safer opens, raw-key files from the CLI

- **A wrong passphrase never changes the file.** Opening a file that has
  an unfinished save now checks the passphrase or key, and the format,
  before completing that save, so a typo or a refused open of an old file
  leaves it byte-for-byte as it was.
- **Raw-key files work from the CLI.** `info`, `about`, `list`, `verify`,
  `recall`, `export` and `compact` take `--key-file`, as `inspect` does,
  and Python gains `compact_file_with_key`.
- **Python errors have types.** Every engine error is a
  `mnemo_engine.MnemoError` (still a `RuntimeError`), with subclasses such
  as `WrongPassphraseError`, `FormatTooOldError` and
  `EmbedderMismatchError`, so you can catch the one you mean.
- **`mnemo inspect` is lighter and tidier:** it holds much less in memory
  on large files, stops after 30 minutes without use, and `--open` no
  longer passes the link to the browser on its command line.
- The iOS and Android packages are unchanged and stay at 0.13.0.

## What's new in v0.13.0: see what your agents remember

- **`mnemo inspect`: a read-only view of a memory file in your browser.**
  Browse and search memories, follow the history of a keyed fact, read a
  thread's essence and turns, and check restore points and stats. It runs
  only on your own machine, behind a private link that opens one browser
  tab (from v0.13.2) and works only while the inspector runs. It never
  changes the file or blocks an agent that's writing to it.
  ```sh
  mnemo inspect agent.mnemo                  # prints a link; open it in your browser
  mnemo inspect app.mnemo --key-file app.key # a raw-key file (what create_with_key writes)
  ```
  Copy the printed link into your browser, or add `--open` to launch it
  (on Linux with a snap or flatpak browser, copy the link instead).
  The inspector stops on its own after 30 minutes without use
  (`--idle-timeout`).
- **`mnemo export`: a readable copy of a file,** as Markdown or JSON Lines.
  `--git` writes a stable version for a repository: the same file always
  gives the same output, and a new memory changes only its own lines (a new
  version of a keyed fact also changes the line of the version it
  replaces). The output is plaintext, so treat it like the memories
  themselves.
- **Old v8 files are now read-only.** Run `mnemo compact <file>` once to
  upgrade a file to the current format; that also repairs files from
  early versions that compaction used to refuse. For a raw-key file, add
  `--key-file <key>` (from v0.13.1).
- **Every file records which embedding model made its vectors,** and
  refuses a different one, so a model change can't quietly mix
  incompatible vectors.
- **`stats` shows the restore points that are actually kept,** not just
  the policy.
- **iOS:** an optional `MnemoAppleEmbedder` library uses Apple's built-in
  models, so an app can store and search text without bringing its own.

**Upgrading from 0.12.3:** v8 files open read-only until you compact them;
`mnemo compact --format v8` and the mobile `formatVersion` setting are
gone; and the error types gain cases (`FormatTooOld`, `EmbedderMismatch`),
so an exhaustive match needs them. See the [changelog](CHANGELOG.md).

## What's new in v0.12.3: two fixes

- **Swift: a slow embedding model no longer holds up your app.** The store
  runs embeds one at a time on its own queue instead of on Swift's shared
  task threads, so a slow on-device model can't stall the app's other
  tasks. A cancelled text call now stops at once and writes nothing.
- **A closed file is unlocked at once.** On Linux and macOS, an app that
  started another program (an agent running a tool, say) at the moment it
  closed a store could find the file still locked when it reopened it.
  The engine now releases the lock itself.

## What's new in v0.12.2: clearer errors, safer keys

A second hardening release for the phone packages, with a few engine
safety checks. The file format is unchanged.

- **Android never mistakes a passing Keystore error for a lost key.** A
  busy or restarting Keystore now gives a retryable error instead of
  "key missing", and the wrapping key is never regenerated over one that
  exists. Never delete a store because of a Keystore error.
- **Errors say what to do.** The phone packages report distinct errors for
  a memory or crypto failure, a file that isn't a MNemo file, and a file
  that needs a write open, each marked retryable or not. "Corrupt" now
  means only that the file failed its integrity checks.
- **A failed close keeps your writes.** If the last save fails, for
  example because the disk is full, the store keeps its handle so you can
  free space and save again. In Rust, `try_close` returns the handle with
  the error.
- **Reading no longer writes on phones.** Recall doesn't record access
  counts there unless you turn `trackAccess` on, so a search in the iOS
  background no longer ends in a save. `stats()` is cheap on a fresh
  store, and it says how much memory search needs to stay fast.
- **Engine bookkeeping can't be forged.** Metadata keys starting with
  `_mnemo.` are refused on every write, because the engine uses them to
  record versions, threads and keys.
- **What a passphrase change gives you.** A plain `rekey` changes the
  secret that opens the file from now on; anyone who had the file and the
  old secret can still read it and later writes. Use
  `mnemo rekey --rotate` to revoke. Rotation can't erase old ciphertext
  that flash storage still holds in blocks it hasn't reused.

Upgrading from 0.12.1: an exhaustive `switch` on the mobile error type
needs the new cases, and the store configurations' `formatVersion` is
deprecated (v8 files become read-only in v0.13). See the
[changelog](CHANGELOG.md).

## What's new in v0.12.1: safer phone stores

A hardening release for the iOS and Android packages, from an independent
review.

- **Writes are saved on their own.** The phone stores now commit within
  about a second (or every 64 writes) by default, so a crash or a killed
  app no longer loses everything since the app last went to the
  background. `withSession { }` batches a burst into one commit. To keep
  the old behaviour, choose the `.manual` flush policy.
- **Your embedding model runs outside the store's lock,** so a slow model
  no longer stalls other calls or the iOS background close.
- **Search stays fast as threads grow.** The vector memory budget now
  counts only the memories that search scans, not archived conversation
  turns, and `stats()` reports which search mode is in use.
- **Passphrase changes, exports and imports are hardened.** Exports and
  imports now get their own fresh data key. A plain `rekey` changes the
  secret that opens the file from now on, but someone who had a copy and
  the old secret could still read it. To revoke the old secret, use
  `mnemo rekey --rotate`, which also re-encrypts the file under a new key.
  If you think a passphrase leaked, rotate.

## What's new in v0.12.0: iOS and Android

The same engine, now as native packages for phones and tablets: a Swift
package for iOS 15 and later, and an Android library for API 24 and later.

- **Built for how phones behave.** When an iOS app goes to the background,
  the store saves and closes its file, and reopens it on the next call. So
  iOS never kills the app for holding a file lock, even with the file in a
  shared App Group. On Android the store saves when the app is stopped.
- **Keys in the platform's safe.** A random key in the iOS Keychain or the
  Android Keystore, kept on that device by default, with an opt-in for
  iCloud Keychain sync on iOS. Files are excluded from backups unless their
  key can follow them, and `exportWithPassphrase` moves a memory to a new
  device.
- **Your own embedding model.** Pass the phone's embedding API or a
  bundled model to the store; it's called without holding the file lock.
- **Small.** About 2.6 MB added to an iOS app, and 2.7 MB of native code
  on 64-bit Arm Android (3.1 MB on x86_64), aligned for Android's 16 KB
  pages.

```swift
import Mnemo

let store = try MnemoStore(
    configuration: MnemoStoreConfiguration(name: "assistant", dimensions: 384),
    embedder: MyEmbedder())
try await store.rememberText("The user prefers dark roast coffee",
                             options: RememberOptions(memoryType: .semantic, key: "coffee"))
let page = try await store.recallText("what coffee does the user like?",
                                      options: RecallOptions(topK: 5))
```

The Kotlin store is the same shape. Full guides:
[Swift](mnemo/bindings/swift/README.md) and
[Android](mnemo/bindings/kotlin/README.md). On Android, set
`abiFilters` to `arm64-v8a` and `x86_64` (the Android guide shows how),
because MNemo ships only those two.

**Measured on an iPhone** (iPhone18,3, iOS 27, 10K memories): 1.3 KB per
memory on disk, 7.3 MB of memory for an open file, 2.1 ms to open, 1.7 ms
per search, 12 ms to store a memory durably. The first search after
opening takes about 7 ms while the vectors load.

## Upgrading to v0.12.0

- **Rust: `close()` now takes the handle** (`close(self)`), and the file
  and its lock are released before it returns. Code that used the handle
  after `close()` no longer compiles. If you keep a `Mnemo` in a struct,
  hold an `Option<Mnemo>` and call `.take()`. Python already worked this
  way.
- **No file format change.** v8 files still open and write in v0.12.
  **v0.13 makes v8 files read-only,** so run `mnemo compact` to upgrade
  them to v9 (see "Upgrading to v0.11.0").

## What's new in v0.11.0: files about 6× smaller

A new file format, v9. At 100K memories, compared with v0.10.0:

- **1.5 KB per memory on disk instead of 8.7 KB.** Memories share pages,
  vectors are stored at half precision, and a save writes only what
  changed. Old snapshots are kept for a window, and their space is then
  reused. Point-in-time restore reaches back only to the retained
  snapshots: on v9, the newest 16 plus any from the last 24 hours, never
  pinning more than a quarter of the file (so under heavy churn, fewer);
  on v8, the newest 256. Call `set_max_snapshots` to keep a fixed number
  instead; `compact` drops the history.
- **Faster saves and searches.** Storing one memory durably takes 16 ms
  instead of 28 ms, and a warm search takes 2.5 ms instead of 8.8 ms.
- **Keyword search for exact names.** Pass versions, PR numbers or
  function names as `keywords`, and they're matched exactly alongside the
  meaning-based search. Don't paste the whole question there.
- **Exact search on smaller files.** Below 20K memories there's no index:
  every search checks every memory, so it always finds the true best
  matches.
- **Old conversation turns are compressed** once they're a week old, so
  files with long threads end up about a third smaller.

```python
db.recall_text("what changed in that release?", keywords="v0.10.0")
```

```sh
mnemo recall agent.mnemo --keywords "v0.10.0"   # keyword-only; add --text to combine
```

**The trade-offs.** At 100K memories, an open file uses about 112 MB of
memory instead of 34 MB, because vectors stay in memory for fast search
(lower the limit with `set_vector_cache_budget`). Opening takes 30 ms
instead of 9 ms, and the first search 112 ms instead of 99 ms. The library
is about 1.2 MB larger (2.6 MB for the Python package, 3.6 MB for the
CLI), about 0.5 MB of it for compression.

## Upgrading to v0.11.0

- **Existing files keep working as they are** (format v8). To get the
  smaller format, run `mnemo compact agent.mnemo`, or `compact_file` in
  Python or Rust.
- **Upgrade every tool that reads a file before you compact it.** v0.10
  and earlier can't open v9 files. They report `unsupported .mnemo format
  version 9 (this build supports v1)`, which means you need v0.11 or
  later. (`mnemo compact --format v8`, which kept a file readable by
  older versions, was removed in v0.13.)
- **Vectors come back at half precision** from `get` and `recall` (about
  0.1% error). In Rust, create the file with
  `MnemoConfig::store_float_vectors` to keep full precision.
- **Rust 1.75 to 1.83:** if your project depends on `mnemo-engine`, run
  `cargo update -p jobserver --precise 0.1.34` once. Cargo before 1.84
  doesn't choose dependency versions by Rust version.

## What's new in v0.10.0: smaller and faster

The first release of the on-device work, with no file format change. At
100K memories, compared with v0.9.1:

- **Opening takes 9 ms instead of 384 ms.** It no longer re-reads the crash
  log, and the search index loads on first use.
- **Saving one memory takes 28 ms instead of 449 ms,** and grows the file
  by 3.4 MB instead of 21 MB. The search index is saved every 64 commits
  and on close, not on every save.
- **An open file uses 34–54 MB of memory instead of 115–158 MB.** Warm
  searches take 8.8 ms instead of 23–27 ms, with the same accuracy.
- **Keys for keychains.** A file created with a random key skips the
  passphrase step, so it opens without the ~16 ms and ~19 MB that
  passphrase hashing costs:

```python
key = mnemo_engine.generate_key()  # keep it in the OS keychain
db = mnemo_engine.open_with_key("agent.mnemo", key, dimensions=384)
```

The page cache is now 8 MB by default (it was 64 MB). Raise it with
`set_cache_capacity` if your searches rerank many candidates.

## What's new in v0.9.0: threads and essence

Memory is the knowledge an agent looks up. **Essence** is where a piece of
work stands and how it got there, so a new session can carry on where the
last one stopped. v0.9.0 keeps both, in the same encrypted file:

- **A full copy of a thread.** `ingest_thread` (or `mnemo thread ingest
  --transcript … --format claude-code-jsonl`) saves every turn word for
  word. Running it again only adds the new turns. `thread_export` gives the
  thread back, and `thread_forget` removes it. Turns stay out of normal
  recall unless you ask for them.
- **Essence.** Short items in sections (intent, conventions, decisions,
  state, open, next, lessons), plus a timeline of milestones that only
  grows. `essence(thread=…)` renders it within a size budget, ready to load
  at the start of a session. Items are edited one at a time, never
  rewritten as a whole summary, so they don't drift.
- **Carry on in a new thread.** `thread_start("new", "old")` links the
  threads, and the new thread's essence begins with the old one's open
  items, next steps and recent milestones.
- **One call to save a batch.** `commit_deltas` writes facts, essence
  changes and turns together, or nothing at all if any part is invalid.

```python
db.ingest_thread("planner", [{"turn_id": "t1", "role": "user", "content": "..."}])
db.essence_add("decisions", "Chose V2 scoring: best ranking, no drift", thread="planner")
db.essence_append("Shipped v0.9.0", thread="planner")
db.thread_start("planner-2", "planner")
print(db.essence(thread="planner-2"))
```

This uses the embedder you registered with `set_embedder` (see Quick
start). No file format change: v0.9.0 files open in v0.7 and v0.8, but
those versions show turns and essence items as ordinary memories.

## What's new in v0.8.0

- **Facts that change.** `remember(..., key="current-release")` replaces the
  memory holding that key with a new version. The old version leaves recall
  but stays in `history(key)`. Writing the same value again does nothing.
  `update(id, ...)` corrects a memory in place (the id is kept).
- **Filtered recall.** Limit a search to a session or to metadata values:
  `recall_text(..., filter={"session_id": "s1", "metadata": {"area": "billing"}})`,
  with `$in`, `$gte`/`$lte` and `$exists`. The CLI has `--session` and
  `--filter`; MCP takes a `filter` object.
- **Recall within a size budget.** `max_bytes=4096` stops adding results once
  their text would pass the budget, and reports whether anything was cut.
  It's for small context windows.

All three work the same in Rust, Python, the CLI and MCP. Files written by
v0.8.0 still open in v0.7, but v0.7 shows replaced versions as ordinary
memories, so upgrade every tool that reads the file.

## For AI agents

A `.mnemo` file introduces itself. Handed a file and its passphrase, an agent
needs one command to learn which embedding model it expects, its conventions,
and any notes its author left:

```sh
mnemo about path/to/agent.mnemo      # or db.about() in Python
```

`mnemo init` adds a starter manifest, so new files are self-describing from the
start. Working on this repository itself? Start at [AGENTS.md](AGENTS.md).

## Performance

Measured with v0.11.0 (format v9) on an Apple M1 Pro (release build,
384-dimension vectors, about 300 bytes of text per memory; files of 20K
memories or more are indexed):

| | 1K memories | 10K | 100K |
|---|---|---|---|
| Open with a raw key (from a keychain) | 0.4 ms | 2.9 ms | 30 ms |
| Open with a passphrase (Argon2id, slow on purpose) | ~15 ms | ~17 ms | ~44 ms |
| First search after opening (loads the vectors and index) | 0.9 ms | 9 ms | 112 ms |
| Search, warm | 0.2 ms | 2.2 ms | 2.5 ms |
| Store one memory durably | 16 ms | 15 ms | 16 ms |
| … each, when 100 are committed together | 0.23 ms | 0.20 ms | 0.32 ms |
| Memory used by an open file (after 1,000 searches) | 3 MB | 17 MB | 112 MB |
| Disk per memory | 1.4 KB | 1.3 KB | 1.5 KB |

The library adds about 2.6 MB (Python package) or 3.6 MB (CLI), stripped,
on ARM Linux.

**On an iPhone** (v0.12.0, iPhone18,3, iOS 27, the same corpus, raw-key
files):

| | 1K memories | 10K |
|---|---|---|
| Open | 0.3 ms | 2.1 ms |
| First search after opening | 0.7 ms | 7.1 ms |
| Search, warm | 0.2 ms | 1.7 ms |
| Store one memory durably | | 12 ms |
| Memory used by an open file | | 7.3 MB |
| Disk per memory | | 1.3 KB |

Android has only been measured on the emulator so far.
An exact-key cache hit takes ~1 µs, and a similar-prompt hit over 500
entries ~0.4 ms. Full results:
[`test/scripts/perf_v0.5.0.json`](test/scripts/perf_v0.5.0.json).

**Ranking quality (v0.7.0).** On a small age-varied test set (13
questions), the right memory comes first for 92% of them, up from 77% in
v0.6 and 62% when ranking on similarity alone. The ranking stays the same
however often the memories are recalled.

Indexed search finds 97–99% of the true top-10 matches at the default
settings (0.994 at 20K, 0.974 at 50K, 0.978 at 100K). Below 20K memories,
search is exact. Ranking quality is unchanged in v0.11.0.

**Upgrading an index from v0.5.x?** It improves automatically (about 76–83%
of the true top-10), and rebuilding it restores the full accuracy: run
`mnemo index <file>` again, `build_index()` in Python, or `rebuild_index()`
in Rust. Each re-clusters the index with the new defaults.

## Upgrading to v0.7.0

- **New import name.** In Python, `import mnemo` becomes `import mnemo_engine`;
  in Rust, `use mnemo::...` becomes `use mnemo_engine::...`. The package
  name, the `mnemo` CLI, the `.mnemo` file format and existing files are
  unchanged.
- **New ranking.** Recency now counts from when a memory was stored, not
  when it was last recalled. The bonuses for recency, importance and use
  are scaled by similarity, so an unrelated memory can't win on being new
  or popular. Scores are on a new scale: if you compare `score` to a fixed
  threshold, check it again.

## Known limitations (being fixed)

- **The first search after opening is slower.** It loads the vectors,
  and the index on large files: about 112 ms at 100K memories, about 9 ms
  at 10K on a Mac, and about 7 ms at 10K on an iPhone. Searches after that
  take 2–3 ms. Loading only what a search needs is planned.
- **Saving threads isn't automatic yet.** You (or your agent) call
  `thread ingest` and the essence tools. A Claude Code plugin with hooks
  that save before compaction and load at session start is on the roadmap,
  after the current size and speed work. Transcripts can contain anything that was pasted into
  them; they're encrypted like everything else, and `thread forget`
  removes them.
- **Version history has no limit yet.** Every keyed replacement keeps the
  old version, so a key that changes very often grows the file. A
  per-key retention setting is planned.
- **v8 files are read-only from v0.13** and don't get the v0.11
  improvements (smaller files, keyword search, compression) until you run
  `mnemo compact`. See "Upgrading to v0.11.0".
- **The mobile packages are new.** The Swift and Android APIs cover the
  common calls (memories, recall, threads, essence, the exact-key cache),
  not the semantic cache, snapshots or rekey yet. Android has been tested
  on emulators, not yet measured on a phone.

See the [changelog](CHANGELOG.md) for release history.

## Security advisory (fixed in v0.5.2)

Versions before 0.5.2 could reuse AES-GCM nonces if a `.mnemo` file was
copied and both copies were then written to: for example, two machines
syncing the same file, a sync service's "conflicted copy", or restoring an
older backup and carrying on with it. Someone holding both copies could
recover parts of the content or forge pages. From v0.5.2, every writing
session draws a fresh random nonce prefix, so this can't happen.

If you may have written to two copies of the same file with an earlier
version, upgrade, then run `mnemo compact` (or `compact_file` in Rust or
Python) on the copy you keep. Compaction rewrites the file under a fresh
data key. Starting in v0.6.0 compaction also preserves the result cache
(FIX-1); on v0.5.x the result cache was emptied by compaction and had to
be recomputed on next use. Details are in the [changelog](CHANGELOG.md).

## Repository layout

```
├── mnemo/              # Rust engine, CLI and MCP server, benches, tests
├── mnemo-python/       # Python package (PyO3 + maturin), published as mnemo-engine
├── test/               # dogfooding sandbox: this project keeps its own memory in a .mnemo file
├── index.html          # the project site
├── AGENTS.md           # orientation for AI coding agents working on this repo
└── CHANGELOG.md
```

To build from source: `cd mnemo && cargo test` for the engine, or
`cd mnemo-python && pip install maturin && maturin develop --release` for the
Python package.

## License

Apache-2.0. See [LICENSE](LICENSE).
