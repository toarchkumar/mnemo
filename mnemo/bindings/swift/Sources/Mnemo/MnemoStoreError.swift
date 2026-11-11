import Foundation
import Security

/// Errors from the Swift layer. Errors from the engine arrive as
/// ``MnemoError``.
public enum MnemoStoreError: Error, Equatable, Sendable {
    /// The key that opens a file isn't in the key store.
    ///
    /// `keyID` is the id stamped on the file, or `nil` when the file carries
    /// no id and none of the stored keys opens it. `path` is the file, or
    /// `nil` when a key store reports the miss on its own.
    ///
    /// Recovery: import a passphrase export
    /// (``MnemoStore/importWithPassphrase(from:passphrase:configuration:keyStore:embedder:)``)
    /// or delete the file and start over. A key under ``KeyPolicy/deviceOnly``
    /// never leaves the device, so a file restored onto another device can't
    /// be opened without such an export.
    case keyMissing(keyID: String?, path: String?)
    /// A Keychain call failed with this `OSStatus`.
    case keychain(status: Int32, operation: String)
    /// A file or directory couldn't be prepared, or the configuration names
    /// an invalid file.
    case file(message: String)
    /// The app is in the background and the system gave no more background
    /// time, so the call didn't open the file (an app suspended holding it
    /// would be killed). Retryable: call again when the app is active, or
    /// in the next background task (v0.12.2).
    case noBackgroundTime
    /// The store's file was created with a key under `stored`, and the
    /// configuration asks for `configured`. The policy is fixed when the
    /// store is created; changing it would put the file in backups without
    /// a synced key, or the reverse. Nothing was changed (v0.12.2).
    case keyPolicyMismatch(stored: KeyPolicy, configured: KeyPolicy)

    /// Whether the same call may succeed later with nothing changed: the
    /// Keychain and background-time errors.
    public var isRetryable: Bool {
        switch self {
        case .keychain, .noBackgroundTime: return true
        case .keyMissing, .file, .keyPolicyMismatch: return false
        }
    }
}

extension MnemoStoreError: LocalizedError {
    public var errorDescription: String? {
        switch self {
        case let .keyMissing(keyID, path):
            let id = keyID.map { "key \($0)" } ?? "no stored key"
            return "MNemo: \(id) for \(path ?? "the file") is missing from the key store"
        case let .keychain(status, operation):
            let text = SecCopyErrorMessageString(status, nil) as String? ?? "OSStatus \(status)"
            return "MNemo: Keychain \(operation) failed: \(text)"
        case let .file(message):
            return "MNemo: \(message)"
        case .noBackgroundTime:
            return "MNemo: no background time left; the file wasn't opened (retry when the app is active)"
        case let .keyPolicyMismatch(stored, configured):
            return "MNemo: the store's key was created with \(stored), but the configuration asks for \(configured); the policy is fixed when a store is created"
        }
    }
}

extension Data {
    /// Overwrites the bytes with zeros. It wipes the storage this value
    /// owns: when the storage is shared with another `Data` (or bridged from
    /// an immutable `CFData`), the first mutation copies it and the wipe
    /// lands on the copy, so call it only on a value nothing else holds (see
    /// `KeyBytes`). Best effort: copies Swift already made aren't reachable.
    mutating func zeroize() {
        resetBytes(in: startIndex..<endIndex)
    }
}
