#if os(iOS)
import Foundation
import Security
import Testing
import UIKit
@testable import Mnemo

/// Runs under `xcodebuild test` on an iOS simulator: the real lifecycle
/// notifications, the Keychain, and file protection.
@Suite(.serialized) struct IOSTests {
    @Test func backgroundTransitionClosesAndTheNextCallReopens() async throws {
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "bg", dimensions: 4, directory: try scratchDirectory()),
            keyStore: keys)
        for i in 0..<5 { try await store.remember("pending \(i)", vector: vec(i)) }
        #expect(await store.isOpen)

        // Writes are pending (no flush). The app goes to the background.
        await MainActor.run {
            NotificationCenter.default.post(name: UIApplication.didEnterBackgroundNotification, object: nil)
        }
        #expect(try await eventually { await store.backgroundCloses == 1 })
        #expect(await store.isOpen == false)
        #expect(await store.lastBackgroundError == nil)
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 5)

        // Back in the foreground: the next call reopens transparently and
        // the file stays open.
        await post(UIApplication.willEnterForegroundNotification)
        #expect(try await store.count() == 5)
        #expect(await store.isOpen)

        // willTerminate does the same.
        try await store.remember("one more", vector: vec(9))
        await MainActor.run {
            NotificationCenter.default.post(name: UIApplication.willTerminateNotification, object: nil)
        }
        #expect(try await eventually { await store.backgroundCloses == 2 })
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 6)
    }

    @Test func aBackgroundTransitionCommitsAndCancelsThePendingCoalescedCommit() async throws {
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "bgpolicy", dimensions: 4, directory: try scratchDirectory(),
                                                   flushPolicy: .coalesced(maxDelay: 0.5, maxWrites: 64)),
            keyStore: keys)
        await post(UIApplication.didBecomeActiveNotification)
        for i in 0..<3 { try await store.remember("pending \(i)", vector: vec(i)) }
        #expect(await store.commits == 0)

        await post(UIApplication.didEnterBackgroundNotification)
        #expect(try await eventually { await store.backgroundCloses == 1 })
        #expect(await store.commits == 1)
        try await Task.sleep(nanoseconds: 900_000_000)
        #expect(await store.isOpen == false, "the timer doesn't reopen the file")
        #expect(await store.commits == 1)
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 3)

        // In the background each call commits as it closes; no timer.
        try await store.remember("in the background", vector: vec(5))
        #expect(await store.commits == 2)
        #expect(await store.isOpen == false)
        await post(UIApplication.willEnterForegroundNotification)
        #expect(try await store.count() == 4)
        try await store.close()
    }

    @Test func aBackgroundCloseIsNotDelayedByAnEmbedInFlight() async throws {
        let keys = MemoryKeyStore()
        let slow = SlowEmbedder(seconds: 2)
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "bgembed", dimensions: 32, directory: try scratchDirectory()),
            keyStore: keys, embedder: slow)
        await post(UIApplication.didBecomeActiveNotification)
        try await store.remember("open the file", vector: WordEmbedder().vector(for: "open the file"))
        let closesBefore = await store.backgroundCloses
        let writer = Task { try await store.rememberText("embedding while the app leaves") }
        #expect(try await eventually(5) { slow.started })

        let start = Date()
        await post(UIApplication.didEnterBackgroundNotification)
        #expect(try await eventually(5) { await store.backgroundCloses == closesBefore + 1 })
        let waited = Date().timeIntervalSince(start)
        #expect(waited < 1, "the background close waited \(waited) s for the embedder")
        #expect(await store.isOpen == false)

        // The write lands afterwards, as a call made in the background.
        _ = try await writer.value
        #expect(await store.isOpen == false)
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 2)
        await post(UIApplication.willEnterForegroundNotification)
        try await store.close()
    }

    @Test func aCallInTheBackgroundReopensCompletesAndLeavesTheFileClosed() async throws {
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "bgcall", dimensions: 4, directory: try scratchDirectory()),
            keyStore: keys)
        try await store.remember("before", vector: vec(0))
        await post(UIApplication.didEnterBackgroundNotification)
        #expect(try await eventually { await store.backgroundCloses == 1 })
        #expect(store.appIsActive == false)

        // Work the app does in the background: it reopens, completes, and
        // the store closes the file again before the call returns.
        try await store.remember("in the background", vector: vec(1))
        #expect(await store.isOpen == false)
        #expect(await store.backgroundCallCloses == 1)
        #expect(await store.lastBackgroundError == nil)
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 2)

        // Reads too.
        #expect(try await store.count() == 2)
        #expect(await store.isOpen == false)
        #expect(await store.backgroundCallCloses == 2)
    }

    @Test func aBurstOfBackgroundCallsEndsClosed() async throws {
        let keys = MemoryKeyStore()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "burst", dimensions: 4, directory: try scratchDirectory()),
            keyStore: keys)
        try await store.remember("warm", vector: vec(0))
        await post(UIApplication.didEnterBackgroundNotification)
        try await withThrowingTaskGroup(of: Void.self) { group in
            for i in 1...40 {
                group.addTask { try await store.remember("burst \(i)", vector: vec(i)) }
            }
            try await group.waitForAll()
        }
        #expect(await store.isOpen == false)
        #expect(await store.backgroundCallCloses >= 1)
        #expect(await store.lastBackgroundError == nil)
        try assertUnlockedAndDurable(store.fileURL, keys: keys, count: 41)
    }

    @Test func afterWillEnterForegroundTheHandleStaysOpen() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "fg", dimensions: 4, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore())
        try await store.remember("one", vector: vec(0))
        await post(UIApplication.didEnterBackgroundNotification)
        #expect(try await eventually { await store.backgroundCloses == 1 })
        await post(UIApplication.willEnterForegroundNotification)
        #expect(store.appIsActive)

        try await store.remember("two", vector: vec(1))
        #expect(await store.isOpen)
        #expect(try await store.count() == 2)
        #expect(await store.isOpen)
        #expect(await store.backgroundCallCloses == 0)

        // didBecomeActive alone (no willEnterForeground) also counts.
        await post(UIApplication.didEnterBackgroundNotification)
        #expect(try await eventually { await store.backgroundCloses == 2 })
        await post(UIApplication.didBecomeActiveNotification)
        try await store.remember("three", vector: vec(2))
        #expect(await store.isOpen)
        try await store.close()
    }

    @Test func callsRacingTheNotificationCompleteOrReopen() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "race", dimensions: 4, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore())
        try await store.remember("warm", vector: vec(0))
        try await withThrowingTaskGroup(of: Void.self) { group in
            for i in 1...80 {
                group.addTask { try await store.remember("note \(i)", vector: vec(i)) }
                if i % 10 == 0 {
                    group.addTask {
                        await MainActor.run {
                            NotificationCenter.default.post(name: UIApplication.didEnterBackgroundNotification,
                                                            object: nil)
                        }
                    }
                }
            }
            try await group.waitForAll()
        }
        #expect(try await store.count() == 81)
        // Once the app is in the background, a call closes the file after
        // itself, so a notification's close often finds it closed already.
        // Either way the file ends closed, with no error.
        #expect(try await eventually {
            await store.backgroundCloses + store.backgroundCallCloses >= 1
        })
        #expect(await store.isOpen == false)
        #expect(await store.lastBackgroundError == nil)
    }

    @Test(arguments: [KeyPolicy.deviceOnly, .iCloudSynced])
    func keychainStoresUnderThePolicy(_ policy: KeyPolicy) throws {
        let keychain = MnemoKeychain(service: "io.github.toarchkumar.mnemo.tests.\(UUID().uuidString)")
        let id = UUID().uuidString
        var key = generateKey()
        defer { key.zeroize() }
        try keychain.storeKey(key, id: id, policy: policy)
        #expect(try keychain.key(id: id) == key)
        #expect(try keychain.keyIDs() == [id])

        let attributes = try #require(try keychain.attributes(id: id))
        let synced = (attributes[kSecAttrSynchronizable as String] as? NSNumber)?.boolValue ?? false
        #expect(synced == (policy == .iCloudSynced))
        let accessible = attributes[kSecAttrAccessible as String] as? String
        let expected = policy == .deviceOnly
            ? kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
            : kSecAttrAccessibleAfterFirstUnlock as String
        #expect(accessible == expected)

        // Storing again replaces; deleting leaves a clear KeyMissing.
        try keychain.storeKey(key, id: id, policy: policy)
        #expect(try keychain.keyIDs() == [id])
        try keychain.deleteKey(id: id)
        #expect(throws: MnemoStoreError.keyMissing(keyID: id, path: nil)) { try keychain.key(id: id) }
        try keychain.deleteKey(id: id)
    }

    @Test(arguments: [KeyPolicy.deviceOnly, .iCloudSynced])
    func storeWithTheKeychainEndToEnd(_ policy: KeyPolicy) async throws {
        let keychain = MnemoKeychain(service: "io.github.toarchkumar.mnemo.tests.\(UUID().uuidString)")
        let config = MnemoStoreConfiguration(name: "kc", dimensions: 4, directory: try scratchDirectory(),
                                             keyPolicy: policy)
        let store = try MnemoStore(configuration: config, keyStore: keychain)
        try await store.remember("kept in the keychain", vector: vec(0))
        try await store.close()

        let id = try #require(KeyID.read(store.fileURL))
        #expect(try keychain.keyIDs() == [id])
        let reopened = try MnemoStore(configuration: config, keyStore: keychain)
        #expect(try await reopened.count() == 1)

        let values = try store.fileURL.resourceValues(forKeys: [.isExcludedFromBackupKey])
        #expect(values.isExcludedFromBackup == (policy == .deviceOnly))
        let attributes = try FileManager.default.attributesOfItem(atPath: store.fileURL.path)
        let protection = attributes[.protectionKey] as? FileProtectionType
        #if targetEnvironment(simulator)
        // The simulator has no Data Protection (files live on the Mac's file
        // system) and reports no class; a reported class must still match.
        #expect(protection == nil || protection == .completeUntilFirstUserAuthentication)
        #else
        #expect(protection == .completeUntilFirstUserAuthentication)
        #endif
        try await reopened.close()
        try keychain.deleteKey(id: id)
    }

    @Test func defaultDirectoryIsApplicationSupport() throws {
        let dir = try MnemoStoreConfiguration.defaultDirectory()
        #expect(dir.lastPathComponent == "mnemo")
        #expect(dir.deletingLastPathComponent().lastPathComponent == "Application Support")
    }
}
/// Posts a lifecycle notification on the main thread, as UIKit does.
private func post(_ name: Notification.Name) async {
    await MainActor.run { NotificationCenter.default.post(name: name, object: nil) }
}
#endif
