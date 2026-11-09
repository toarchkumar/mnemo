import Foundation
import Testing
@testable import Mnemo

/// Runs on every platform (`swift test` on macOS, and the iOS simulator).
@Suite struct StoreTests {
    @Test func roundTripAcrossStores() async throws {
        let keys = MemoryKeyStore()
        let config = MnemoStoreConfiguration(name: "notes", dimensions: 4, directory: try scratchDirectory())
        let store = try MnemoStore(configuration: config, keyStore: keys)
        #expect(await store.isOpen == false)

        let first = try await store.remember("alpha", vector: [1, 0, 0, 0])
        try await store.remember("beta", vector: [0, 1, 0, 0])
        #expect(first.outcome == .added)
        #expect(try await store.get(id: first.id).content == "alpha")
        let page = try await store.recall(vector: [0.9, 0.1, 0, 0], options: RecallOptions(topK: 1))
        #expect(page.hits.map(\.memory.content) == ["alpha"])
        try await store.close()

        // One key, stamped on the file; a second store on the file opens it.
        let ids = try keys.keyIDs()
        #expect(ids.count == 1)
        #expect(KeyID.read(store.fileURL) == ids.first)
        #expect(keys.policy(id: ids[0]) == .deviceOnly)
        let again = try MnemoStore(configuration: config, keyStore: keys)
        #expect(try await again.count() == 2)
        #expect(try await again.stats().dimensions == 4)
        try await again.close()
    }

    @Test func keywordOnlyRecallNeedsNoEmbedder() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "kw", dimensions: 4, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore())
        try await store.remember("the giraffe eats acacia leaves", vector: vec(0))
        try await store.remember("penguins live in the south", vector: vec(1))
        try await store.remember("a note about tax forms", vector: vec(2))
        let page = try await store.recall(vector: [], options: RecallOptions(topK: 2, keywords: "giraffe acacia"))
        #expect(page.hits.first?.memory.content == "the giraffe eats acacia leaves")
        try await store.close()
    }

    @Test func swiftEmbedderAndItsErrors() async throws {
        let embedder = WordEmbedder()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "emb", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: embedder)
        try await store.rememberText("the user prefers dark roast coffee")
        try await store.rememberText("the meeting moved to thursday afternoon")
        let hit = try await store.recallText("what coffee does the user like", options: RecallOptions(topK: 1))
        #expect(hit.hits.first?.memory.content == "the user prefers dark roast coffee")
        #expect(embedder.callCount >= 3)

        // An embedder that throws: Embedder, and nothing is written.
        try await store.setEmbedder(FailingEmbedder())
        await #expect(throws: MnemoError.Embedder(message: "model not loaded")) {
            try await store.rememberText("never stored")
        }
        try await store.setEmbedder(CrashingEmbedder())
        do {
            try await store.rememberText("never stored either")
            Issue.record("expected an Embedder error")
        } catch let MnemoError.Embedder(message) {
            #expect(!message.isEmpty)
        }
        #expect(try await store.count() == 2)

        // A width that doesn't match the file is refused.
        await #expect(throws: MnemoError.self) {
            try await store.setEmbedder(WordEmbedder(dimensions: 8))
        }
        try await store.close()
    }

    @Test func keyedWritesKeepHistory() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "keyed", dimensions: 4, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore())
        let first = try await store.remember("tea", vector: vec(0), options: RememberOptions(key: "drink"))
        let same = try await store.remember("tea", vector: vec(0), options: RememberOptions(key: "drink"))
        let second = try await store.remember("coffee", vector: vec(1), options: RememberOptions(key: "drink"))
        #expect(first.outcome == .added)
        #expect(same.outcome == .unchanged)
        #expect(second.outcome == .superseded)
        let history = try await store.history(key: "drink")
        #expect(history.map(\.content) == ["coffee", "tea"])  // newest first
        try await store.close()
    }

    @Test func threadsIngestExportForget() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "threads", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: WordEmbedder())
        let turns = [
            ThreadTurn(turnId: "t1", role: "user", content: "plan the trip to Lisbon", timestamp: 1_700_000_000),
            ThreadTurn(turnId: "t2", role: "assistant", content: "flights first, then a hotel", timestamp: 1_700_000_060),
        ]
        let report = try await store.ingestThread("trip", turns: turns)
        #expect(report.addedRecords >= 2)
        let again = try await store.ingestThread("trip", turns: turns)
        #expect(again.addedRecords == 0)
        #expect(again.skippedExistingTurns == 2)
        let exported = try await store.threadExport("trip")
        #expect(exported.map(\.turnId) == ["t1", "t2"])
        #expect(exported.first?.content == "plan the trip to Lisbon")
        #expect(try await store.threadForget("trip") >= 2)
        #expect(try await store.threadExport("trip").isEmpty)
        try await store.close()
    }

    @Test func passphraseExportThenImport() async throws {
        let dir = try scratchDirectory()
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "source", dimensions: 4, directory: dir),
            keyStore: keys)
        try await store.remember("moves with the user", vector: vec(0))
        try await store.remember("so does this", vector: vec(1))

        // The export opens with the passphrase, and not with the device key.
        let export = dir.appendingPathComponent("transfer.mnemo")
        try await store.exportWithPassphrase(to: export, passphrase: "correct horse battery staple")
        let viaPassphrase = try MnemoDb.openReadOnly(path: export.path, passphrase: "correct horse battery staple")
        #expect(try viaPassphrase.len() == 2)
        try viaPassphrase.close()
        let sourceKey = try keys.key(id: try #require(KeyID.read(store.fileURL)))
        #expect(throws: MnemoError.self) { try MnemoDb.openReadOnlyWithKey(path: export.path, key: sourceKey) }

        // The import gets a new key under a new id, stamped on the new file.
        let otherKeys = MemoryKeyStore()
        let imported = try await MnemoStore.importWithPassphrase(
            from: export, passphrase: "correct horse battery staple",
            configuration: MnemoStoreConfiguration(name: "imported", dimensions: 4, directory: dir,
                                                   keyPolicy: .iCloudSynced),
            keyStore: otherKeys)
        let newID = try #require(KeyID.read(imported.fileURL))
        #expect(try otherKeys.keyIDs() == [newID])
        #expect(otherKeys.policy(id: newID) == .iCloudSynced)
        #expect(try await imported.count() == 2)
        #expect(throws: MnemoError.self) {
            try MnemoDb.openReadOnly(path: imported.fileURL.path, passphrase: "correct horse battery staple")
        }
        let values = try imported.fileURL.resourceValues(forKeys: [.isExcludedFromBackupKey])
        #expect(values.isExcludedFromBackup == false)
        try await imported.close()
        try await store.close()
    }

    @Test func aLostKeyIDIsRecoveredAndAMissingKeyIsClear() async throws {
        let keys = MemoryKeyStore()
        let dir = try scratchDirectory()
        let config = MnemoStoreConfiguration(name: "stamp", dimensions: 4, directory: dir)
        // Two other stores' keys sit in the same key store.
        for name in ["other-a", "other-b"] {
            let other = try MnemoStore(configuration: MnemoStoreConfiguration(name: name, dimensions: 4, directory: dir),
                                       keyStore: keys)
            try await other.remember("x", vector: vec(0))
            try await other.close()
        }
        let store = try MnemoStore(configuration: config, keyStore: keys)
        try await store.remember("kept", vector: vec(0))
        try await store.close()
        let id = try #require(KeyID.read(store.fileURL))

        // The attribute is gone (a copy that dropped it): the right key is
        // found by trying each, and the id is stamped again.
        KeyID.remove(from: store.fileURL)
        #expect(KeyID.read(store.fileURL) == nil)
        let reopened = try MnemoStore(configuration: config, keyStore: keys)
        #expect(try await reopened.count() == 1)
        #expect(KeyID.read(store.fileURL) == id)
        try await reopened.close()

        // The key itself is gone: a clear KeyMissing naming the id and file.
        try keys.deleteKey(id: id)
        let orphan = try MnemoStore(configuration: config, keyStore: keys)
        await #expect(throws: MnemoStoreError.keyMissing(keyID: id, path: store.fileURL.path)) {
            try await orphan.count()
        }
        KeyID.remove(from: store.fileURL)
        await #expect(throws: MnemoStoreError.keyMissing(keyID: nil, path: store.fileURL.path)) {
            try await orphan.count()
        }
    }

    @Test func backgroundCloseReleasesTheLockAndTheNextCallReopens() async throws {
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "bg", dimensions: 4, directory: try scratchDirectory()),
            keyStore: keys)
        try await store.remember("pending one", vector: vec(0))
        try await store.remember("pending two", vector: vec(1))

        await store.closeForBackground()
        #expect(await store.isOpen == false)
        #expect(await store.backgroundCloses == 1)
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 2)

        #expect(try await store.count() == 2)
        #expect(await store.isOpen)
        try await store.close()
    }

    @Test func callsRacingBackgroundClosesNeverSeeClosed() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "race", dimensions: 4, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore())
        try await withThrowingTaskGroup(of: Void.self) { group in
            for i in 0..<60 {
                group.addTask { try await store.remember("note \(i)", vector: vec(i)) }
                if i % 6 == 0 {
                    group.addTask { await store.closeForBackground() }
                }
            }
            try await group.waitForAll()
        }
        #expect(try await store.count() == 60)
        #expect(await store.backgroundCloses >= 1)
        try await store.close()
    }

    @Test func backupFlagFollowsThePolicy() async throws {
        let dir = try scratchDirectory()
        for (name, policy, excluded) in [("dev", KeyPolicy.deviceOnly, true), ("cloud", .iCloudSynced, false)] {
            let store = try MnemoStore(
                configuration: MnemoStoreConfiguration(name: name, dimensions: 4, directory: dir, keyPolicy: policy),
                keyStore: MemoryKeyStore())
            try await store.remember("x", vector: vec(0))
            let values = try store.fileURL.resourceValues(forKeys: [.isExcludedFromBackupKey])
            #expect(values.isExcludedFromBackup == excluded, "\(name)")
            try await store.close()
        }
    }

    @Test func compactKeepsTheKeyAndTheData() async throws {
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "compact", dimensions: 4, directory: try scratchDirectory()),
            keyStore: keys)
        for i in 0..<20 { try await store.remember("note \(i)", vector: vec(i)) }
        let id = KeyID.read(store.fileURL)
        try await store.compact()
        #expect(KeyID.read(store.fileURL) == id)
        #expect(try await store.count() == 20)
        try await store.close()
    }

    @Test func invalidNamesAreRefused() throws {
        for name in ["", "../escape", ".hidden", "a/b"] {
            #expect(throws: MnemoStoreError.self) {
                try MnemoStore(configuration: MnemoStoreConfiguration(name: name, dimensions: 4,
                                                                      directory: try scratchDirectory()),
                               keyStore: MemoryKeyStore())
            }
        }
    }
}

/// The file's lock is free (a second writer opens it) and it holds `count`
/// memories.
func assertUnlockedAndDurable(_ file: URL, keys: any MnemoKeyStore, count: UInt64) throws {
    var key = try keys.key(id: try #require(KeyID.read(file)))
    defer { key.zeroize() }
    let other = try MnemoDb.openWithKey(path: file.path, key: key)
    #expect(try other.len() == count)
    try other.close()
}
