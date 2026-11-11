import Foundation
import Security

/// Where a database key may live, and whether the file goes into backups.
public enum KeyPolicy: Sendable, Equatable {
    /// The default. The key stays on this device
    /// (`kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly`, never synced), and
    /// the file is excluded from iCloud and computer backups, since a
    /// restored copy couldn't be opened anyway.
    case deviceOnly
    /// The key syncs through iCloud Keychain (`kSecAttrSynchronizable`,
    /// `kSecAttrAccessibleAfterFirstUnlock`), and the file stays in backups,
    /// so a restore onto a new device can open it. If the user has iCloud
    /// Keychain turned off, the key doesn't sync and the restored file reports
    /// ``MnemoStoreError/keyMissing(keyID:path:)``.
    case iCloudSynced

    /// Whether the database file is excluded from backups.
    public var excludesFileFromBackup: Bool { self == .deviceOnly }

    var accessible: CFString {
        switch self {
        case .deviceOnly: return kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        case .iCloudSynced: return kSecAttrAccessibleAfterFirstUnlock
        }
    }

    var synchronizable: Bool { self == .iCloudSynced }
}

/// Stores database keys by id. ``MnemoKeychain`` is the implementation apps
/// use; tests and custom setups can supply their own.
public protocol MnemoKeyStore: Sendable {
    /// Stores `key` under `id`, replacing any key already stored there.
    func storeKey(_ key: Data, id: String, policy: KeyPolicy) throws
    /// The key stored under `id`. Throws
    /// ``MnemoStoreError/keyMissing(keyID:path:)`` when there's none.
    func key(id: String) throws -> Data
    /// The ids of every key in this store.
    func keyIDs() throws -> [String]
    /// Deletes the key under `id`. Deleting a missing key isn't an error.
    func deleteKey(id: String) throws
    /// The policy the key under `id` was stored with, or `nil` when the key
    /// store can't tell (v0.12.2). A store whose key has a different policy
    /// than its configuration refuses to open. The default returns `nil`.
    func policy(id: String) throws -> KeyPolicy?
}

extension MnemoKeyStore {
    public func policy(id: String) throws -> KeyPolicy? { nil }
}

/// Database keys as Keychain generic-password items.
///
/// Each key is one item: `kSecAttrService` is ``service``, and
/// `kSecAttrAccount` is the key id, a random UUID stamped on the database
/// file, so renaming or moving the file doesn't orphan its key. Items use the
/// data-protection keychain on macOS as well.
public struct MnemoKeychain: MnemoKeyStore {
    /// The default `kSecAttrService`.
    public static let defaultService = "io.github.toarchkumar.mnemo"

    /// The `kSecAttrService` of every item this keychain reads and writes.
    public let service: String
    /// An optional `kSecAttrAccessGroup`, to share keys with an app
    /// extension or another app from the same team.
    public let accessGroup: String?

    public init(service: String = MnemoKeychain.defaultService, accessGroup: String? = nil) {
        self.service = service
        self.accessGroup = accessGroup
    }

    public func storeKey(_ key: Data, id: String, policy: KeyPolicy) throws {
        try deleteKey(id: id)
        var item = query(id: id)
        item[kSecAttrSynchronizable] = policy.synchronizable
        item[kSecAttrAccessible] = policy.accessible
        item[kSecAttrLabel] = "MNemo database key"
        item[kSecValueData] = key
        let status = SecItemAdd(item as CFDictionary, nil)
        guard status == errSecSuccess else {
            throw MnemoStoreError.keychain(status: status, operation: "add")
        }
    }

    public func key(id: String) throws -> Data {
        var search = query(id: id)
        search[kSecAttrSynchronizable] = kSecAttrSynchronizableAny
        search[kSecReturnData] = true
        search[kSecMatchLimit] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(search as CFDictionary, &result)
        if status == errSecItemNotFound {
            throw MnemoStoreError.keyMissing(keyID: id, path: nil)
        }
        guard status == errSecSuccess, let data = result as? Data else {
            throw MnemoStoreError.keychain(status: status, operation: "read")
        }
        return data
    }

    public func keyIDs() throws -> [String] {
        try items().compactMap { $0[kSecAttrAccount as String] as? String }
    }

    public func deleteKey(id: String) throws {
        var search = query(id: id)
        search[kSecAttrSynchronizable] = kSecAttrSynchronizableAny
        let status = SecItemDelete(search as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw MnemoStoreError.keychain(status: status, operation: "delete")
        }
    }

    /// The policy the item was stored with: synced means ``KeyPolicy/iCloudSynced``.
    public func policy(id: String) throws -> KeyPolicy? {
        guard let item = try attributes(id: id) else { return nil }
        let synced = (item[kSecAttrSynchronizable as String] as? Bool)
            ?? ((item[kSecAttrSynchronizable as String] as? NSNumber)?.boolValue ?? false)
        return synced ? .iCloudSynced : .deviceOnly
    }

    /// The stored item's attributes (no key bytes), for tests and diagnostics.
    func attributes(id: String) throws -> [String: Any]? {
        try items(id: id).first
    }

    private func items(id: String? = nil) throws -> [[String: Any]] {
        var search = query(id: id)
        search[kSecAttrSynchronizable] = kSecAttrSynchronizableAny
        search[kSecReturnAttributes] = true
        search[kSecMatchLimit] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(search as CFDictionary, &result)
        if status == errSecItemNotFound { return [] }
        guard status == errSecSuccess else {
            throw MnemoStoreError.keychain(status: status, operation: "list")
        }
        return result as? [[String: Any]] ?? []
    }

    private func query(id: String?) -> [CFString: Any] {
        var q: [CFString: Any] = [
            kSecClass: kSecClassGenericPassword,
            kSecAttrService: service,
            kSecUseDataProtectionKeychain: true,
        ]
        if let id { q[kSecAttrAccount] = id }
        if let accessGroup { q[kSecAttrAccessGroup] = accessGroup }
        return q
    }
}
