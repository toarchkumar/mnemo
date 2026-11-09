import Foundation
#if canImport(Darwin)
import Darwin
#endif

/// The key id stamped on a database file, as an extended attribute.
///
/// The id can't live inside the file (in the manifest, say), because the
/// file is encrypted with the very key the id names. An extended attribute
/// stays with the file through renames and moves on the same volume, and
/// through `FileManager` copies. When it's lost anyway (a copy that drops
/// attributes, a compaction that replaces the file), ``MnemoStore`` tries
/// each stored key in turn and stamps the one that opens the file.
enum KeyID {
    static let attribute = "io.github.toarchkumar.mnemo.key-id"

    static func read(_ url: URL) -> String? {
        url.withUnsafeFileSystemRepresentation { path -> String? in
            guard let path else { return nil }
            let size = getxattr(path, attribute, nil, 0, 0, 0)
            guard size > 0 else { return nil }
            var bytes = [UInt8](repeating: 0, count: size)
            guard getxattr(path, attribute, &bytes, size, 0, 0) == size else { return nil }
            return String(decoding: bytes, as: UTF8.self)
        }
    }

    static func write(_ id: String, to url: URL) throws {
        let bytes = Array(id.utf8)
        let rc = url.withUnsafeFileSystemRepresentation { path -> Int32 in
            guard let path else { return -1 }
            return setxattr(path, attribute, bytes, bytes.count, 0, 0)
        }
        guard rc == 0 else {
            throw MnemoStoreError.file(message: "can't stamp the key id on \(url.path): \(String(cString: strerror(errno)))")
        }
    }

    static func remove(from url: URL) {
        _ = url.withUnsafeFileSystemRepresentation { path in
            path.map { removexattr($0, attribute, 0) }
        }
    }
}

/// File protection and the backup flag.
enum FilePolicy {
    #if os(iOS) || os(tvOS) || os(watchOS) || os(visionOS)
    static let protection = FileProtectionType.completeUntilFirstUserAuthentication
    #endif

    /// Creates the directory if needed, with the protection class new files
    /// in it inherit.
    static func prepareDirectory(_ dir: URL) throws {
        let fm = FileManager.default
        #if os(iOS) || os(tvOS) || os(watchOS) || os(visionOS)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true,
                               attributes: [.protectionKey: protection])
        try fm.setAttributes([.protectionKey: protection], ofItemAtPath: dir.path)
        #else
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        #endif
    }

    /// Sets the protection class and the backup flag on the database file.
    /// Applied on every open, since a compaction replaces the file.
    static func apply(to file: URL, policy: KeyPolicy) throws {
        #if os(iOS) || os(tvOS) || os(watchOS) || os(visionOS)
        try FileManager.default.setAttributes([.protectionKey: protection], ofItemAtPath: file.path)
        #endif
        var values = URLResourceValues()
        values.isExcludedFromBackup = policy.excludesFileFromBackup
        var url = file
        try url.setResourceValues(values)
    }
}
