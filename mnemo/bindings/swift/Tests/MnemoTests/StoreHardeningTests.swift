import Foundation
import Testing
@testable import Mnemo

/// v0.12.2 store hardening (MOBILE-2e step 5): RB-002.4 T2, T9 and T10.
@Suite struct StoreHardeningTests {
    /// T2: 5 writes, `flush()`, 20 more, then the app lets go of the store
    /// without closing it, and a new store opens the same file. Today: 5.
    /// After: 25, under the coalesced default and under `.everyWrite`. The
    /// store commits and closes when it goes away, which also frees the lock.
    @Test(arguments: [FlushPolicy.coalesced(maxDelay: 0.3, maxWrites: 64), .everyWrite])
    func t2AStoreLetGoWithoutCloseKeepsEveryWrite(_ policy: FlushPolicy) async throws {
        let keys = MemoryKeyStore()
        let config = MnemoStoreConfiguration(name: "t2", dimensions: 4, directory: try scratchDirectory(),
                                             flushPolicy: policy)
        var store: MnemoStore? = try MnemoStore(configuration: config, keyStore: keys)
        for i in 0..<5 { try await store!.remember("flushed \(i)", vector: vec(i)) }
        try await store!.flush()
        for i in 5..<25 { try await store!.remember("pending \(i)", vector: vec(i)) }
        store = nil

        let again = try MnemoStore(configuration: config, keyStore: keys)
        #expect(try await eventually(5) { (try? await again.count()) == 25 })
        try await again.close()
    }

    /// T9 (a): an embedder that reads the store (`cacheGet`) from a detached
    /// task and waits up to 3 s for it. Before 2c it timed out (the actor was
    /// busy embedding). After: it returns at once.
    @Test func t9aAnEmbedderThatCallsCacheGetReturnsAtOnce() async throws {
        let embedder = CacheReadingEmbedder()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "t9a", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: embedder)
        embedder.store = store
        let start = Date()
        try await store.rememberText("hello")
        #expect(Date().timeIntervalSince(start) < 2)
        #expect(embedder.timedOut == false)
        try await store.close()
    }

    /// T9 (b): an embed in flight; `count()` and `closeForBackground()`
    /// 0.2 s in. Before 2c both waited for the embed. After: milliseconds.
    /// The embed is held until both are measured (at most 10 s), so it can't
    /// land before `count()` on a slow runner.
    @Test func t9bCountAndTheBackgroundCloseDontWaitForAnEmbed() async throws {
        let slow = SlowEmbedder(seconds: 10)
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "t9b", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: slow)
        try await store.remember("there", vector: WordEmbedder().vector(for: "there"))
        let writer = Task { try await store.rememberText("slow to embed") }
        #expect(try await eventually(5) { slow.started })
        try await Task.sleep(nanoseconds: 200_000_000)
        var t = Date()
        #expect(try await store.count() == 1)
        let countSecs = Date().timeIntervalSince(t)
        t = Date()
        await store.closeForBackground()
        let closeSecs = Date().timeIntervalSince(t)
        slow.open()
        #expect(countSecs < 0.3, "count took \(countSecs) s")
        #expect(closeSecs < 0.3, "the background close took \(closeSecs) s")
        _ = try await writer.value
        #expect(try await store.count() == 2)
        try await store.close()
    }

    /// T10: create under `.deviceOnly`, close, reopen as `.iCloudSynced`.
    /// Today: the file moves into backups while the key stays device-only.
    /// After: a clear error naming both policies, and nothing changed.
    @Test func t10AChangedKeyPolicyIsRefusedAndChangesNothing() async throws {
        let keys = MemoryKeyStore()
        let dir = try scratchDirectory()
        let device = try MnemoStore(configuration: MnemoStoreConfiguration(name: "t10", dimensions: 4, directory: dir),
                                    keyStore: keys)
        try await device.remember("kept", vector: vec(0))
        try await device.close()
        let id = try #require(KeyID.read(device.fileURL))

        let synced = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "t10", dimensions: 4, directory: dir, keyPolicy: .iCloudSynced),
            keyStore: keys)
        await #expect(throws: MnemoStoreError.keyPolicyMismatch(stored: .deviceOnly, configured: .iCloudSynced)) {
            try await synced.count()
        }
        #expect(MnemoStoreError.keyPolicyMismatch(stored: .deviceOnly, configured: .iCloudSynced)
            .errorDescription?.contains("iCloudSynced") == true)
        // Nothing was half-applied: still excluded from backups, key unchanged.
        let excluded = try device.fileURL.resourceValues(forKeys: [.isExcludedFromBackupKey]).isExcludedFromBackup
        #expect(excluded == true)
        #expect(keys.policy(id: id) == .deviceOnly)
        // The right policy still opens it.
        let again = try MnemoStore(configuration: MnemoStoreConfiguration(name: "t10", dimensions: 4, directory: dir),
                                   keyStore: keys)
        #expect(try await again.count() == 1)
        try await again.close()
    }

    @Test func noBackgroundTimeIsRetryableAndTheOthersAreNot() {
        #expect(MnemoStoreError.noBackgroundTime.isRetryable)
        #expect(MnemoStoreError.keychain(status: -25308, operation: "read").isRetryable)
        #expect(!MnemoStoreError.keyMissing(keyID: nil, path: nil).isRetryable)
        #expect(!MnemoError.Corrupt(message: "x").isRetryable())
        #expect(MnemoError.Closed.isRetryable())
    }

    @Test func setMaxSnapshotsAppliesNowAndAfterAReopen() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "snaps", dimensions: 4, directory: try scratchDirectory(),
                                                   flushPolicy: .everyWrite),
            keyStore: MemoryKeyStore())
        try await store.setMaxSnapshots(3)
        for i in 0..<10 { try await store.remember("n \(i)", vector: vec(i)) }
        try await store.close()
        for i in 10..<15 { try await store.remember("n \(i)", vector: vec(i)) }
        #expect(try await store.count() == 15)
        try await store.close()
    }
}

/// Reads the store's cache from a detached task before embedding, waiting
/// up to 3 s, as an app's embedder might check a cache.
final class CacheReadingEmbedder: MnemoEmbedder, @unchecked Sendable {
    weak var store: MnemoStore?
    private let lock = NSLock()
    private var _timedOut = false
    private let words = WordEmbedder()

    var timedOut: Bool { lock.lock(); defer { lock.unlock() }; return _timedOut }

    func identity() -> EmbedderIdentity { words.identity() }

    func embed(texts: [String]) throws -> [[Float]] {
        let done = DispatchSemaphore(value: 0)
        let store = self.store
        let text = texts[0]
        Task.detached {
            _ = try? await store?.cacheGet(namespace: "emb", key: text)
            done.signal()
        }
        if done.wait(timeout: .now() + 3) == .timedOut {
            lock.lock(); _timedOut = true; lock.unlock()
        }
        return try words.embed(texts: texts)
    }
}
