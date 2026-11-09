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

        // Back in the foreground: the next call reopens transparently.
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
        #expect(try await eventually { await store.backgroundCloses >= 1 })
        try await store.close()
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
        #expect(attributes[.protectionKey] as? FileProtectionType == .completeUntilFirstUserAuthentication)
        try await reopened.close()
        try keychain.deleteKey(id: id)
    }

    @Test func defaultDirectoryIsApplicationSupport() throws {
        let dir = try MnemoStoreConfiguration.defaultDirectory()
        #expect(dir.lastPathComponent == "mnemo")
        #expect(dir.deletingLastPathComponent().lastPathComponent == "Application Support")
    }
}
#endif
