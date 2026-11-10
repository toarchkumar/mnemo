# MNemo for Swift

An encrypted, single-file agent memory for iOS and macOS apps, built on the
`mnemo-uniffi` binding (MOBILE-1). This directory is the development
package. Releases go to
[`toarchkumar/mnemo-swift`](https://github.com/toarchkumar/mnemo-swift):
on each tag, CI publishes there a `Package.swift` that points at the
XCFramework zip on this repo's GitHub Release, plus these sources. Nobody
edits that repo by hand.

- **iOS 15 / macOS 12 or later**, Swift tools 6.0 (Xcode 16 or later).
- One product, `Mnemo`. It holds `MnemoStore`, `MnemoKeychain`, and the
  generated types (`MemoryRecord`, `RecallOptions`, `MnemoError`, ...).

## Install

In Xcode, choose **File > Add Package Dependencies...**, enter
`https://github.com/toarchkumar/mnemo-swift`, and add the `Mnemo`
product. Or, in `Package.swift`:

```swift
dependencies: [
    .package(url: "https://github.com/toarchkumar/mnemo-swift", from: "0.12.0"),
],
targets: [
    .target(name: "MyApp", dependencies: [.product(name: "Mnemo", package: "mnemo-swift")]),
]
```

## Building

```bash
cd mnemo/bindings/swift
scripts/build-xcframework.sh   # build/MnemoFFI.xcframework + Sources/Mnemo/Generated/
swift test                     # macOS

# iOS simulator: the tests run inside a small host app (TestHost/), because
# the Keychain needs an app's entitlements. The project comes from XcodeGen.
cd TestHost && xcodegen generate
xcodebuild test -project MnemoTestHost.xcodeproj -scheme MnemoHostTests \
  -destination 'platform=iOS Simulator,name=iPhone 16'
```

Running the package's own scheme on a simulator (`xcodebuild test -scheme
Mnemo`) works too, except for the Keychain tests, which fail with
`errSecMissingEntitlement` (-34018) outside an app.

`scripts/kill-test.sh` is the kill test for the default flush policy. It
launches the host app on a simulator to write, kills it with `simctl
terminate` (no lifecycle event, no flush), relaunches it, and checks that
every write older than the coalescing window survived.

The script needs the Rust targets `aarch64-apple-ios`,
`aarch64-apple-ios-sim`, `x86_64-apple-ios` and `aarch64-apple-darwin`. Set
`MNEMO_SLICES=macos` to build only the macOS slice. Run it again after any
change to `mnemo-uniffi`. The XCFramework and the generated Swift are build
outputs and aren't committed.

To use the package from a local app during development, add it as a local
package (File > Add Package Dependencies > Add Local...).

## Usage

```swift
import Mnemo

let store = try MnemoStore(
    configuration: MnemoStoreConfiguration(name: "assistant", dimensions: 384),
    embedder: MyEmbedder()           // optional; see below
)

try await store.rememberText("The user prefers dark roast coffee",
                             options: RememberOptions(memoryType: .semantic, key: "coffee"))
let page = try await store.recallText("what coffee does the user like?",
                                      options: RecallOptions(topK: 5, maxBytes: 2048))
for hit in page.hits { print(hit.score, hit.memory.content) }

// Without an embedder: bring your own vectors, or recall by keywords alone.
try await store.remember("Flight UA 931 on 12 March", vector: myVector)
let byWord = try await store.recall(vector: [], options: RecallOptions(keywords: "UA 931"))
```

- **Threads:** `ingestThread(_:turns:)` appends turns (re-ingesting a turn
  is a no-op), `threadExport(_:)` returns them, `threadForget(_:)` deletes
  them, and `essence(.thread(threadId:))` summarizes one within a byte
  budget.
- **The essence:** what an agent derives from a thread, as small items.
  - `essenceAdd(.decisions, "…", scope:)` adds an item (280 bytes at
    most) and returns its id.
  - `essenceUpdate(id:text:)`, `essenceResolve(id:)` and
    `essenceRemove(id:)` change one.
  - `essenceAppendTimeline("…", scope:)` adds a milestone (600 bytes,
    append-only).
  - `essence(_:maxBytes:)` renders them.
- **The index:** `buildIndex()` builds the IVF+PQ index, and `hasIndex()`
  says whether there is one. On v9 files below 20,000 facts it builds
  nothing (`partitions == 0`), because an exact scan is faster there.
- **Keyed facts:** `RememberOptions(key:)` writes a new version and
  supersedes the old one. `history(key:)` lists the versions, newest first.
- **The result cache:** `cachePut` and `cacheGet`, by exact key.
- **Batches:** `withSession { session in ... }` runs many calls on one
  open handle and commits once at the end (see Durability).
- **Also:** `get`, `update`, `delete`, `about`, `stats`, `count`, `flush`,
  `close` and `compact`.

### Durability

A write is in memory until the store commits it to disk. A crash, a kill
(the system ending a suspended app, a force quit) or a power loss keeps
every committed write and loses the rest. When the store commits is the
configuration's `flushPolicy`:

| Policy | A write is durable | Use it for |
|---|---|---|
| `.coalesced(maxDelay: 1, maxWrites: 64)` (the default) | at most about 1 second after it returns, or at once when it's the 64th write waiting | most apps: one commit per burst of writes |
| `.everyWrite` | before it returns | data the app can't lose, at the cost of a commit (a page write and a sync) per write |
| `.manual` | only on the events below (the behaviour before v0.12.1) | benchmarks, and apps that call `flush()` themselves |

Whatever the policy, the store also commits when the app goes to the
background or is about to terminate, after every call made while the app
isn't active, and on `flush()` and `close()`.

`withSession` batches many writes into one commit:

```swift
try await store.withSession { session in
    for (text, vector) in notes { try session.remember(text, vector: vector) }
}
```

The calls run on one open handle with nothing else in between, and the
store commits once at the end (not under `.manual`). A session doesn't
embed, so compute the vectors first. If the closure throws, the writes it
made stay and are committed the same way.

### The embedder

Implement `MnemoEmbedder` (Core ML, an on-device model, or a server):

```swift
final class MyEmbedder: MnemoEmbedder, @unchecked Sendable {
    func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "minilm-l6-v2", dimensions: 384, normalize: true)
    }
    func embed(texts: [String]) throws -> [[Float]] {
        // one vector per text, each `dimensions` wide
    }
}
```

- `embed` runs before a call reaches the store's actor, synchronously and
  off the main thread. The store isn't held while it runs, so other calls
  go ahead, a background close isn't held up, and `embed` may call back
  into the same store. (Before v0.12.1 it ran on the actor, so a slow
  model held every other call.)
- If it throws (any error), the call fails with `MnemoError.Embedder`, and
  nothing is written.
- The identity is recorded in the file. Opening the file with a different
  model name or width is refused, so vectors from two models never mix.

## Threads: keep the blocking API off the main thread

- **`MnemoStore` is an actor.** Every call is `async`, runs off the main
  thread, and is serialized.
- **`MnemoDb`, the generated class underneath, blocks.** Every call does
  disk I/O, and opening or exporting with a passphrase runs a KDF that
  takes seconds. If you use `MnemoDb` directly, never call it from the
  main thread or the main actor. Wrap it in your own actor or queue.
- **Large calls hold the store.** `ingestThread` over a long thread,
  `compact`, and the passphrase export are all long. They run on the store
  and hold it, so other calls to the same store wait for them.

## Background, termination and the file lock

iOS kills a suspended app that holds a file lock in a shared container
(`0xdead10cc`). `MnemoStore` avoids that:

- **Closing.** On `didEnterBackgroundNotification` and
  `willTerminateNotification`, it flushes and closes the file inside an
  expiring background activity. That releases the lock before suspension,
  and every write made so far is durable.
- **Calls while the app isn't active close after themselves.** The store
  tracks the app's state: `didEnterBackground` marks it inactive, and
  `willEnterForeground` or `didBecomeActive` marks it active again.
  - While it's inactive, every call runs inside its own expiring
    activity. It reopens the file, does its work, then flushes and closes
    the file again before the activity ends. So work the app does in the
    background (a background fetch, a push, a `Task` still running) never
    leaves the lock held into suspension.
  - A call that races the transition either finishes before the close or
    runs as a background call. It never fails with `Closed`.
  - Each such call pays an open (a few milliseconds with a raw key), so
    batch background work where you can.
- **Launched into the background.** An app launched straight into the
  background (a background fetch, a push) gets no `didEnterBackground`.
  So the store reads `UIApplication`'s state once, on the main thread, and
  starts out inactive when the app is in the background. In an app
  extension, which gets no `UIApplication` notifications, the store treats
  the process as active, so close it yourself when the extension's work
  ends.
- **Back in the foreground,** the file stays open between calls again.
- **Errors.** A failed background flush, or a failed coalesced commit, is
  kept in `lastBackgroundError`. The data written before the last
  successful commit is safe.
- **The coalescing timer** never reopens the file: a background close
  commits and cancels it.

Open one `MnemoStore` per file and keep it for the app's lifetime. Every
open costs a file open and a key lookup (no KDF: device keys are raw keys).

## Files, keys and backups

- **Location and protection.** Files live in `Application Support/mnemo/`
  as `<name>.mnemo`. The directory and the file get
  `FileProtectionType.completeUntilFirstUserAuthentication`, so a store
  works in the background after the first unlock since boot.
- **One key per file.** Each new file gets a fresh random 256-bit key,
  stored by `MnemoKeychain` as a generic-password item (`kSecAttrService`
  `io.github.toarchkumar.mnemo`). The key never touches disk outside the
  Keychain.
- **The key id.** Each key is stored under a random id, and the id is
  stamped on the file as an extended attribute
  (`io.github.toarchkumar.mnemo.key-id`). Renaming or moving the file
  doesn't orphan its key. The id can't go inside the file, because the
  file is encrypted with the key it names. If the attribute is lost (a
  copy that drops it), the store tries each stored key, and the first one
  that opens the file is stamped again.

| `KeyPolicy` | Keychain item | File in backups | After a restore to a new device |
|---|---|---|---|
| `.deviceOnly` (default) | `kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, not synced | excluded | nothing to restore; the app starts empty, or imports an export |
| `.iCloudSynced` | `kSecAttrAccessibleAfterFirstUnlock`, synced through iCloud Keychain | included | opens, once iCloud Keychain has synced the key |

- **Recovering from `MnemoStoreError.keyMissing`.** The file exists, but
  its key isn't in the Keychain. That happens with a `.deviceOnly` file
  copied to another device, an `.iCloudSynced` file restored before the
  key synced or with iCloud Keychain off, or a deleted Keychain item. The
  data can't be read without the key. The app can wait and retry (iCloud
  sync), import a passphrase export (below), or delete the file and start
  over.
- **Sharing keys.** `MnemoKeychain(accessGroup:)` shares keys with app
  extensions or other apps from the same team. Give every process the same
  group and service.

### Moving to another device: passphrase export and import

```swift
// On the old device: a copy that opens with the passphrase alone.
try await store.exportWithPassphrase(to: exportURL, passphrase: passphrase)

// On the new device: re-protected by a fresh key in this device's Keychain.
let store = try await MnemoStore.importWithPassphrase(
    from: exportURL, passphrase: passphrase,
    configuration: MnemoStoreConfiguration(name: "assistant", dimensions: 384))
```

- Both calls take a few seconds, because the passphrase goes through the
  KDF.
- The destination must not exist yet, and the source is never modified.
- Have the user choose the passphrase. Never store it next to the export.

## App Groups and extensions

```swift
let dir = try MnemoStoreConfiguration.appGroupDirectory("group.com.example.app")
let store = try MnemoStore(
    configuration: MnemoStoreConfiguration(name: "assistant", dimensions: 384, directory: dir),
    keyStore: MnemoKeychain(accessGroup: "TEAMID.com.example.shared"))
```

- **Use App Group containers only through `MnemoStore`.** Its background
  close is what keeps a suspended app or extension from holding the lock.
  The close relies on `ProcessInfo.performExpiringActivity`, which works
  in extensions as well, where `UIApplication.shared` isn't available.
- **One process at a time.** A store already open in another process fails
  with `MnemoError.Locked`. Concurrent access from an app and its
  extensions waits for engine phase 12.4.

## Errors

- `MnemoError` comes from the engine: `Locked`, `WrongPassphrase`,
  `HeaderTampered`, `UnsupportedVersion`, `NotFound`, `ReadOnly`,
  `DimensionMismatch`, `Invalid`, `Io`, `Corrupt`, `Closed`, `Embedder`.
  `MnemoStore` never surfaces `Closed`.
- `MnemoStoreError` comes from this layer: `keyMissing(keyID:path:)`,
  `keychain(status:operation:)` and `file(message:)`.
