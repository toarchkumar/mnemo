# MNemo for Android

An encrypted, single-file agent memory for Android apps, built on the
`mnemo-uniffi` binding (MOBILE-1). This is the development package. It will
be published to Maven Central as **`io.github.toarchkumar:mnemo-android`**
(MOBILE-1e).

- **Android API 24 (7.0) or later**, on `arm64-v8a` and `x86_64` (the
  emulator). The native library is linked for 16 KB pages.
- **Kotlin, with coroutines.** One package, `io.github.toarchkumar.mnemo`.
  It holds `MnemoStore`, `MnemoKeystore`, and the generated types
  (`MemoryRecord`, `RecallOptions`, `MnemoException`, ...).
- **One native dependency:** JNA 5.19.1 (`net.java.dev.jna:jna:5.19.1@aar`),
  pinned exactly. See [NOTICE](NOTICE).

```kotlin
dependencies {
    implementation("io.github.toarchkumar:mnemo-android:0.11.0")
}
```

## Building

You need Rust with the targets `aarch64-linux-android` and
`x86_64-linux-android`, plus
[cargo-ndk](https://github.com/bbqsrc/cargo-ndk) 4.1.2, an NDK r28 or
later, JDK 17 and Gradle 9.6.1.

```bash
cd mnemo/bindings/kotlin
export ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version>
scripts/build-native.sh          # generated/: bindings, jniLibs, host library
gradle testDebugUnitTest         # JVM host tests, against the host library
gradle assembleRelease           # build/outputs/aar/mnemo-android-release.aar
gradle publishToMavenLocal       # ~/.m2, unsigned unless a signing key is set
gradle connectedDebugAndroidTest # instrumented tests on a device or emulator
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
- **Keyed facts:** `RememberOptions(key = ...)` writes a new version and
  supersedes the old one. `history(key)` lists the versions, newest first.
- **The result cache:** `cachePut` and `cacheGet`, by exact key.
- **Also:** `get`, `update`, `delete`, `about`, `stats`, `count`, `flush`,
  `close` and `compact`.

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

- `embed` is called synchronously on the store's dispatcher thread, never
  on the main thread, and never with the database locked.
- If it throws anything, the call fails with `MnemoException.Embedder`,
  and nothing is written. Throw `EmbedderException.Failed(reason)` to
  choose the message.
- The identity is recorded in the file. Opening the file with a different
  model name or width is refused, so vectors from two models never mix.

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
  background.
- **The file stays open.** Android doesn't kill a cached process for
  holding a file lock, unlike iOS, so the next call needs no reopen.
- **Errors.** A failed background flush is kept in `lastBackgroundError`.
  The data written before the last successful flush is safe.
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
its key isn't available. This happens in three ways:

- the key file was deleted;
- the file was copied from another device;
- the Keystore key that wraps it is gone or invalidated, which is rare.

The data can't be read without the key. The app can import a passphrase
export (below), or delete the file and start over.

### Moving to another device: passphrase export and import

```kotlin
// On the old device: a copy that opens with the passphrase alone.
store.exportWithPassphrase(exportFile, passphrase)

// On the new device: re-protected by a fresh key in this device's Keystore.
val store = MnemoStore.importWithPassphrase(
    context, exportFile, passphrase,
    MnemoStoreConfiguration(name = "assistant", dimensions = 384u),
)
```

- Both calls take a few seconds, because the passphrase goes through the
  KDF.
- The destination must not exist yet, and the source is never modified.
- Have the user choose the passphrase. Never store it next to the export.

## Minification (R8)

The AAR ships consumer rules (`consumer-rules.pro`) that keep JNA and the
generated bindings. JNA binds native functions and structure fields by
name. Nothing else is needed.

## Errors

- **`MnemoException`** comes from the engine: `Locked`, `WrongPassphrase`,
  `HeaderTampered`, `UnsupportedVersion`, `NotFound`, `ReadOnly`,
  `DimensionMismatch`, `Invalid`, `Io`, `Corrupt`, `Closed` and `Embedder`.
  `MnemoStore` never surfaces `Closed`.
- **`MnemoStoreException`** comes from this layer: `KeyMissing(keyId,
  path)`, `Keystore(operation)` and `File`.

## Tests

- **JVM host tests** (`src/test`, on every PR in CI's `kotlin` job): the
  store over the real engine, with keys in memory, and the key files with
  a software AES key in place of the Keystore. They need the host library
  that `scripts/build-native.sh` puts in `generated/host/`.
- **Instrumented tests** (`src/androidTest`): the Android Keystore, the
  `ON_STOP` flush through the real process lifecycle, the default
  directory, and loading the native libraries on the device's page size.
  The `android-emulator` workflow runs them nightly and on release tags,
  on API 35 `x86_64` images with 4 KB and 16 KB pages.
- **In your app's own JVM tests**, the plain JNA jar has to be on the test
  classpath (`testImplementation("net.java.dev.jna:jna:5.19.1")`), along
  with a host build of `libmnemo_uniffi` on `jna.library.path`.
