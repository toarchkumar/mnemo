# MNemo for Android

An encrypted, single-file agent memory for Android apps, built on the
`mnemo-uniffi` binding (MOBILE-1). This directory is the development
package. Releases are on Maven Central as
**`io.github.toarchkumar:mnemo-android`**.

- **Android API 24 (7.0) or later**, on `arm64-v8a` and `x86_64` (the
  emulator). The native library is linked for 16 KB pages.
- **Kotlin, with coroutines.** One package, `io.github.toarchkumar.mnemo`.
  It holds `MnemoStore`, `MnemoKeystore`, and the generated types
  (`MemoryRecord`, `RecallOptions`, `MnemoException`, ...).
- **One native dependency:** JNA 5.19.1 (`net.java.dev.jna:jna:5.19.1@aar`),
  pinned exactly. See [NOTICE](NOTICE).

## Install

From Maven Central (`mavenCentral()` in your repositories):

```kotlin
dependencies {
    implementation("io.github.toarchkumar:mnemo-android:0.14.0")
}
```

Then keep the app to the ABIs MNemo ships (see [ABIs](#abis)):

```kotlin
android {
    defaultConfig {
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
}
```

## Building

You need Rust with the targets `aarch64-linux-android` and
`x86_64-linux-android`, plus
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk) 4.1.2, an NDK r28 or
later, and JDK 17. The committed Gradle wrapper (`./gradlew`) fetches
Gradle 9.6.1 and checks its SHA-256.

```bash
cd mnemo/bindings/kotlin
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
scripts/build-native.sh          # generated/: bindings, jniLibs, host library
./gradlew testDebugUnitTest         # JVM host tests, against the host library
./gradlew assembleRelease           # build/outputs/aar/mnemo-android-release.aar
./gradlew publishToMavenLocal       # ~/.m2, unsigned unless a signing key is set
./gradlew connectedDebugAndroidTest # instrumented tests on a device or emulator
```

- **Fewer ABIs.** `MNEMO_ABIS=x86_64 scripts/build-native.sh` builds only
  the emulator's ABI. `MNEMO_ABIS=""` skips Android entirely, which is
  enough for the JVM tests.
- **Rebuilding.** Run the script again after any change to `mnemo-uniffi`.
  Everything under `generated/` is a build output and isn't committed.
- **The Gradle files.** `build.gradle.kts` pins every version: AGP 9.5.1,
  Kotlin 2.4.21, compileSdk 37, coroutines 1.11.0, lifecycle 2.11.0.

## Usage

```kotlin
import io.github.toarchkumar.mnemo.*

val store = MnemoStore(
    context,
    MnemoStoreConfiguration(name = "assistant", dimensions = 384u),
    embedder = MyEmbedder(),            // optional; see below
)

store.rememberText(
    "The user prefers dark roast coffee",
    RememberOptions(memoryType = MemoryType.SEMANTIC, key = "coffee"),
)
val page = store.recallText("what coffee does the user like?", RecallOptions(topK = 5u, maxBytes = 2048uL))
for (hit in page.hits) println("${hit.score} ${hit.memory.content}")

// Without an embedder: bring your own vectors, or recall by keywords alone.
store.remember("Flight UA 931 on 12 March", myVector)
val byWord = store.recall(emptyList(), RecallOptions(keywords = "UA 931"))
```

- **Threads:** `ingestThread(threadId, turns)` appends turns (re-ingesting
  a turn is a no-op). `threadExport(threadId)` returns them,
  `threadForget(threadId)` deletes them, and
  `essence(EssenceScope.Thread(threadId))` summarizes one within a byte
  budget.
- **The essence:** what an agent derives from a thread, as small items.
  - `essenceAdd(EssenceSection.DECISIONS, "…", scope)` adds an item (280
    bytes at most) and returns its id.
  - `essenceUpdate(id, text)`, `essenceResolve(id)` and `essenceRemove(id)`
    change one.
  - `essenceAppendTimeline("…", scope)` adds a milestone (600 bytes,
    append-only).
  - `essence(scope, maxBytes)` renders them.
- **The index:** `buildIndex()` builds the IVF+PQ index, and `hasIndex()`
  says whether there is one. On v9 files below 20,000 facts it builds
  nothing (`partitions == 0u`), because an exact scan is faster there.
- **Keyed facts:** `RememberOptions(key = ...)` writes a new version and
  supersedes the old one. `history(key)` lists the versions, newest first.
- **The result cache:** `cachePut` and `cacheGet`, by exact key.
- **Access counts:** `RecallOptions.trackAccess` is off by default here
  (v0.12.2). With it on, every recall marks its hits as read, which dirties
  the file, so a recall made in the background ends in a full commit, and
  ranking doesn't need it. Pass `RecallOptions(topK = 5u, trackAccess = true)` on the calls whose
  access counts the app reads.
- **Batches:** `withSession { session -> ... }` runs many calls on one
  open handle and commits once at the end (see Durability).
- **Also:** `get`, `update`, `delete`, `about`, `stats`, `count`, `flush`,
  `close` and `compact`.

### Memory and recall speed

Recall scans an fp16 copy of the vectors (`dims × 2 + 4` bytes a fact).
It stays in memory while the file's **live facts** fit the configuration's
`vectorCacheBudgetBytes` (16 MiB by default). Thread turns, essence items and
superseded versions don't count. Past the budget, recall reads the vectors
from the file instead, which is slower, and slower still as the file grows.

| Dimensions | Bytes per fact | Facts that fit 16 MiB | Budget to stay resident at 50,000 facts |
|---|---|---|---|
| 384 | 772 | 21,732 | 38.6 MB |
| 512 | 1,028 | 16,320 | 51.4 MB |
| 768 | 1,540 | 10,894 | 77.0 MB |

`store.stats()` says which path recall takes now, in `recallMode`:
`RecallMode.EXACT_RESIDENT` (in memory, the fast path),
`RecallMode.EXACT_PAGED` (from the file) or `RecallMode.INDEXED`, and how many bytes the column needs to stay in memory, in
`vectorColumnBytes` (`facts × (2 × dimensions + 4)`).

**At the cliff, raise the budget first.** Setting `vectorCacheBudgetBytes` to at least
`vectorColumnBytes` costs that much memory and gives the fast path back at
once (at 22,000 facts × 384, warm recall went from 23.6 ms paged to 4.9 ms
with the budget raised). Build an index (`buildIndex()`) only when the
column no longer fits the memory the app can spare, past 20,000 facts, and
only in a maintenance window: the build holds the store and takes long (49.8
s at 22,000 facts on an M1 Pro; 19 s at 10,000 and 179 s at 100,000 in the
perf file's `build_secs`), and a raised `cachePages` doesn't help a large
file.

### History

Every commit leaves a restorable snapshot, but only some are kept. A
new (v9) file keeps the newest 16 plus any from the last 24 hours, and
never lets them pin more than a quarter of the file. Under the default
flush policy a busy app commits about once a second, so the history
reaches back about a day, not further. Under churn the pin cap can leave
far fewer, so to show how far back a restore reaches, read `stats()`:
`snapshotsRetained`, `oldestRestorableAt` (unix seconds) and
`snapshotPinnedBytes`.
`compact()` drops all of them.
`setMaxSnapshots(_:)` (Kotlin: `setMaxSnapshots(count)`) keeps a different
number instead, as a hard cap; the store applies it every time it opens the
file.

### Durability

A write is in memory until the store commits it to disk. A crash, a kill
(`am force-stop`, the low-memory killer, a swipe from Recents) or a power
loss keeps every committed write and loses the rest. When the store
commits is the configuration's `flushPolicy`:

| Policy | A write is durable | Use it for |
|---|---|---|
| `FlushPolicy.Coalesced(maxDelayMillis = 1000, maxWrites = 64)` (the default) | in the foreground, at most about 1 second after it returns, or at once when it's the 64th write waiting; while the app is stopped, before it returns | most apps: one commit per burst of writes |
| `FlushPolicy.EveryWrite` | before it returns | data the app can't lose, at the cost of a commit (a page write and a sync) per write |
| `FlushPolicy.Manual` | only on the events below (the behaviour before v0.12.1) | benchmarks, and apps that call `flush()` themselves |

"Stopped" is `ProcessLifecycleOwner` below `STARTED`: the app is in the
background, or the process started without an activity (a service, a
broadcast, WorkManager). A write made then commits before it returns,
because the process can be killed at any moment. Whatever the policy, the
store also commits on every `ON_STOP`, and on `flush()` and `close()`.

`withSession` batches many writes into one commit:

```kotlin
store.withSession { session ->
    for ((text, vector) in notes) session.remember(text, vector)
}
```

The calls run on one open handle with nothing else in between, and the
store commits once at the end (not under `Manual`). A session doesn't
embed, so compute the vectors first. If the block throws, the writes it
made stay and are committed the same way.

The names follow the Swift package where Kotlin allows. Two differ in the
generated layer:

- **`MnemoDb.closeFile()`** is the engine's close, because `close()` is
  `AutoCloseable`'s (it frees the Kotlin handle).
- **Error fields called `message`** are `reason` in Kotlin, as in
  `MnemoException.Embedder.reason`, because `Throwable.message` already uses
  the name.

### The embedder

Implement `MnemoEmbedder` (a TFLite or ONNX model, or a server):

```kotlin
class MyEmbedder : MnemoEmbedder {
    override fun identity() = EmbedderIdentity(name = "minilm-l6-v2", dimensions = 384u, normalize = true)
    override fun embed(texts: List<String>): List<List<Float>> {
        // one vector per text, each `dimensions` wide
    }
}
```

- `embed` runs on the store's dispatcher before the call takes the store's
  lock, never on the main thread. The store isn't held while it runs, so
  other calls go ahead, and `embed` may call back into the same store.
  (Before v0.12.1 it ran under the store's lock, so a slow model held every
  other call.)
- If it throws anything, the call fails with `MnemoException.Embedder`,
  and nothing is written. Throw `EmbedderException.Failed(reason)` to
  choose the message.
- The identity is recorded in the file, the first time an embedder is
  attached, and checked every time after that (v0.13, EMBED-ID-1). An
  embedder with another name, width or `normalize` flag is refused with
  `MnemoException.EmbedderMismatch(stored, given)`, so vectors from two models never
  mix. A different model, or a new revision of the same one, therefore
  needs a new store: give it its own `name`, and add the memories again
  from your app's own data. The store opens its file lazily, so the error
  comes from the first call after the open, not from the initializer.

## Threads: keep the blocking API off the main thread

- **`MnemoStore` is safe to call from the main thread.** Every method is a
  `suspend` function that switches to `Dispatchers.IO` (or the dispatcher
  you pass), and calls run one at a time.
- **`MnemoDb`, the generated class underneath, blocks.** Every call does
  disk I/O, and opening or exporting with a passphrase runs a KDF that
  takes seconds. If you use `MnemoDb` directly, never call it on the main
  thread. StrictMode flags it, and a long call is an ANR.
- **Large calls hold the store.** `ingestThread` over a long thread,
  `compact`, and the passphrase export are all long, and other calls to
  the same store wait for them.

## The process lifecycle

- **Flushing.** When the app's last activity stops, `ProcessLifecycleOwner`
  sends `ON_STOP`, about 700 ms later, and the store flushes. Every write
  made so far is then durable, even if the process is killed in the
  background. From then until the next `ON_START`, each write under the
  default policy commits before it returns (see Durability).
- **The file stays open.** Android doesn't kill a cached process for
  holding a file lock, unlike iOS, so the next call needs no reopen.
- **Errors.** A failed background flush, or a failed coalesced commit, is
  kept in `lastBackgroundError`. The data written before the last
  successful commit is safe.
- **Reopening.** After `close()`, or if the engine reports its handle
  closed, the next call reopens the file transparently. No call fails with
  `MnemoException.Closed`.

Make one `MnemoStore` per file and keep it for the app's lifetime, for
example in your `Application` or a DI singleton. A second store on the same
file in the same process fails with `MnemoException.Locked`, and so does
one in another process.

## Files, keys and backups

- **Location.** Files live in `noBackupFilesDir/mnemo/` as `<name>.mnemo`
  (`MnemoStoreConfiguration.defaultDirectory(context)`), unless the
  configuration names another directory.
- **One key per file.** Each new file gets a fresh random 256-bit key.
- **Wrapping.** `MnemoKeystore` encrypts that key with one AES-256-GCM key
  in the Android Keystore. The Keystore key is generated on first use,
  can't be exported, and needs no user authentication, so stores work in
  the background and survive lock-screen changes. The encrypted key goes in
  `noBackupFilesDir/mnemo/keys/<id>.key`, bound to its id. The plain key
  never touches disk.
- **The key id.** Each key has a random id, stamped on its file as the
  extended attribute `user.io.github.toarchkumar.mnemo.key-id` (the same
  name as on iOS). The id can't go inside the file, because the file is
  encrypted with the key it names.
  - When the stamp is missing, the store tries each stored key, and the
    first one that opens the file is stamped again. A stamp goes missing
    on API 24 and 25 (no xattr calls), on a file system without user
    attributes, or on a copy that dropped it. A wrong key costs one cheap
    raw-key open.
- **No `androidx.security:security-crypto`.** It's deprecated, and the
  Keystore calls above are the part of it that's needed.

**Backups: device-only.** Android has no option to sync these keys, unlike
iOS's `.iCloudSynced`. Keystore keys never leave the device, and
`noBackupFilesDir` is excluded from Auto Backup and from device-to-device
transfer. So neither the file nor its key reaches a new phone, and the app
starts empty there unless it imports an export.

**Recovering from `MnemoStoreException.KeyMissing`.** The file exists, but
its key is gone. This happens in four ways:

- the key file was deleted;
- the file was copied from another device;
- the key file is damaged, or doesn't authenticate;
- the Keystore key that wraps it is missing or permanently invalidated,
  which is rare.

The data can't be read without the key. The app can import a passphrase
export (below), or delete the file and start over.

**`MnemoStoreException.Keystore` is different: never delete anything for
it.** It means the Keystore, or the key file behind it, failed for a reason
that can pass: the Keystore service busy or restarting, a binder error, an
I/O error, or a wrapping-key entry that exists but couldn't be read. The key
is probably still there. Retry later (on the next launch, say), and treat
only `KeyMissing` as a lost key. The store never replaces a wrapping key
whose alias exists, since that would orphan every key it wraps.

### Moving to another device: passphrase export and import

```kotlin
// On the old device: a copy that opens with the passphrase alone.
store.exportWithPassphrase(exportFile, passphrase)

// On the new device: a fresh copy under a new key in this device's Keystore.
val store = MnemoStore.importWithPassphrase(
    context, exportFile, passphrase,
    MnemoStoreConfiguration(name = "assistant", dimensions = 384u),
)
```

- Both calls take a few seconds, because the passphrase goes through the
  KDF.
- The destination must not exist yet, and the source is never modified.
- Both write a compacted copy (live data, no history) under a **fresh data
  key**. The export shares no key material with the device's file, and the
  import is re-protected under a new data key: the transfer passphrase never
  opens the imported file, even with a leftover copy of the export.
- Rotation can't erase old ciphertext the storage hardware still holds: flash keeps rewritten blocks until it reuses them, and that's out of the engine's hands.
- Have the user choose the passphrase. Never store it next to the export,
  and delete the export once it's imported.

## ABIs

The AAR carries `libmnemo_uniffi.so` for `arm64-v8a` and `x86_64` only,
which covers current phones and the emulator. JNA's AAR adds its dispatch
library for five more ABIs (`armeabi`, `armeabi-v7a`, `mips`, `mips64`,
`x86`). On those, an app would install and then fail to load MNemo. So
filter the app to the two ABIs, as the sample does:

```kotlin
android {
    defaultConfig {
        ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
    }
}
```

## Minification (R8)

The AAR ships consumer rules (`consumer-rules.pro`) that keep JNA and the
generated bindings. JNA binds native functions and structure fields by
name. Nothing else is needed.

## Errors

- **`MnemoException`** comes from the engine: `Locked`, `WrongPassphrase`,
  `HeaderTampered`, `UnsupportedVersion`, `NotFound`, `ReadOnly`,
  `DimensionMismatch`, `Invalid`, `Io`, `Corrupt`, `Closed` and `Embedder`,
  since v0.12.2 `NotAMnemoFile`, `NeedsWriteOpen`, `Kdf` and `Crypto`, and
  since v0.13 `EmbedderMismatch` and `FormatTooOld`.
  `MnemoStore` never surfaces `Closed`: it reopens the file and retries once,
  and a handle a panic poisoned is treated the same way.
- **`MnemoStoreException`** comes from this layer: `KeyMissing(keyId,
  path)` (the key is gone), `Keystore(operation)` (retryable; see above)
  and `File`.

**What to do about an error, in three classes** (`e.isRetryable()` tells the first
apart for `MnemoException`):

| Class | Errors | What to do |
|---|---|---|
| Retry later | `Locked`, `Io`, `Kdf` (the KDF couldn't get its memory), `Closed`; `MnemoStoreException.Keystore` | Try again later, on the next launch, say. The data is fine. |
| Fix the call | `Invalid`, `DimensionMismatch`, `EmbedderMismatch`, `FormatTooOld` (a v8 file: convert it with `mnemo compact` on a Mac), `ReadOnly`, `NeedsWriteOpen`, `NotFound`, `WrongPassphrase`, `Embedder` | Change what's passed (or open read-write once, for `NeedsWriteOpen`). |
| The data is unreadable | `Corrupt` (integrity failures only), `HeaderTampered`, `Crypto`, `NotAMnemoFile`, `UnsupportedVersion`; `MnemoStoreException.KeyMissing` | Restore from a backup or an export. Only here is deleting the file ever the answer. |

## Tests

- **JVM host tests** (`src/test`, on every PR in CI's `kotlin` job): the
  store over the real engine, with keys in memory, and the key files with
  a software AES key in place of the Keystore. They need the host library
  that `scripts/build-native.sh` puts in `generated/host/`.
- **Instrumented tests** (`src/androidTest`): the Android Keystore, the
  `ON_STOP` flush and the flush policy through the real process lifecycle,
  the default directory, and loading the native libraries on the device's
  page size. The `android-emulator` workflow runs them nightly and on
  release tags, on API 35 `x86_64` images with 4 KB and 16 KB pages, then
  the sample's kill test: it writes, is force-stopped with no flush, and a
  relaunch must find every write older than the coalescing window.
- **In your app's own JVM tests**, the plain JNA jar has to be on the test
  classpath (`testImplementation("net.java.dev.jna:jna:5.19.1")`), along
  with a host build of `libmnemo_uniffi` on `jna.library.path`.
