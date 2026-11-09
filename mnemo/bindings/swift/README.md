# MNemo for Swift

An encrypted, single-file agent memory for iOS and macOS apps, built on the
`mnemo-uniffi` binding (MOBILE-1). This is the development package. Releases
are published to `toarchkumar/mnemo-swift` by CI (MOBILE-1e), and nobody
edits that repo by hand.

- **iOS 15 / macOS 12 or later**, Swift tools 6.0 (Xcode 16 or later).
- One product, `Mnemo`. It holds `MnemoStore`, `MnemoKeychain`, and the
  generated types (`MemoryRecord`, `RecallOptions`, `MnemoError`, ...).

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
- **Keyed facts:** `RememberOptions(key:)` writes a new version and
  supersedes the old one. `history(key:)` lists the versions, newest first.
- **The result cache:** `cachePut` and `cacheGet`, by exact key.
- **Also:** `get`, `update`, `delete`, `about`, `stats`, `count`, `flush`,
  `close` and `compact`.

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

- `embed` is called synchronously on the store's thread, never on the main
  thread, and never with the database locked.
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
- **Reopening.** The next call reopens the file transparently. A call that
  races the transition either finishes before the close or reopens after
  it. It never fails with `Closed`.
- **Calls while backgrounded.** A call made while the app is in the
  background reopens the file. Make it inside your own background task,
  and the next transition closes it again.
- **Errors.** A failed background flush is kept in
  `lastBackgroundError`. The data written before the last successful
  flush is safe.

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
