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
        }
    }
}

extension Data {
    /// Overwrites the bytes with zeros (best effort: copies Swift already
    /// made aren't reachable).
    mutating func zeroize() {
        resetBytes(in: startIndex..<endIndex)
    }
}
