import Foundation
@testable import Mnemo

/// Keys in memory, so the macOS tests need no Keychain entitlement.
final class MemoryKeyStore: MnemoKeyStore, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [String: (Data, KeyPolicy)] = [:]

    func storeKey(_ key: Data, id: String, policy: KeyPolicy) throws {
        lock.lock(); defer { lock.unlock() }
        keys[id] = (key, policy)
    }

    func key(id: String) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        guard let entry = keys[id] else { throw MnemoStoreError.keyMissing(keyID: id, path: nil) }
        return entry.0
    }

    func keyIDs() throws -> [String] {
        lock.lock(); defer { lock.unlock() }
        return keys.keys.sorted()
    }

    func deleteKey(id: String) throws {
        lock.lock(); defer { lock.unlock() }
        keys[id] = nil
    }

    func policy(id: String) -> KeyPolicy? {
        lock.lock(); defer { lock.unlock() }
        return keys[id]?.1
    }
}

/// A deterministic bag-of-words embedder: texts that share words get similar
/// vectors.
final class WordEmbedder: MnemoEmbedder, @unchecked Sendable {
    let dimensions: UInt32
    private let lock = NSLock()
    private var calls = 0

    /// The identity's name: another name stands in for another model.
    let name: String

    init(dimensions: UInt32 = 32, name: String = "test-words") {
        self.dimensions = dimensions
        self.name = name
    }

    var callCount: Int { lock.lock(); defer { lock.unlock() }; return calls }

    func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: name, dimensions: dimensions, normalize: true)
    }

    func embed(texts: [String]) throws -> [[Float]] {
        lock.lock(); calls += 1; lock.unlock()
        return texts.map(vector(for:))
    }

    func vector(for text: String) -> [Float] {
        var v = [Float](repeating: 0, count: Int(dimensions))
        for word in text.lowercased().split(whereSeparator: { !$0.isLetter && !$0.isNumber }) {
            var h: UInt32 = 2_166_136_261
            for b in word.utf8 { h = (h ^ UInt32(b)) &* 16_777_619 }
            v[Int(h % dimensions)] += 1
        }
        if v.allSatisfy({ $0 == 0 }) { v[0] = 1 }
        let norm = v.reduce(0) { $0 + $1 * $1 }.squareRoot()
        return v.map { $0 / norm }
    }
}

/// Throws the binding's own error type.
final class FailingEmbedder: MnemoEmbedder, @unchecked Sendable {
    func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "test-words", dimensions: 32, normalize: true)
    }

    func embed(texts: [String]) throws -> [[Float]] {
        throw EmbedderError.Failed(message: "model not loaded")
    }
}

/// Throws an arbitrary Swift error.
final class CrashingEmbedder: MnemoEmbedder, @unchecked Sendable {
    struct Boom: Error {}

    func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "test-words", dimensions: 32, normalize: true)
    }

    func embed(texts: [String]) throws -> [[Float]] {
        throw Boom()
    }
}

/// A fresh, empty directory for one test.
func scratchDirectory() throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("mnemo-swift-tests", isDirectory: true)
        .appendingPathComponent(UUID().uuidString, isDirectory: true)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}

/// A non-zero 4-wide vector.
func vec(_ i: Int) -> [Float] {
    [Float(i) + 1, Float(i % 3), 0.5, 1]
}

/// Waits until `condition` holds, or fails after `seconds`.
func eventually(_ seconds: Double = 10, _ condition: () async throws -> Bool) async throws -> Bool {
    let deadline = Date().addingTimeInterval(seconds)
    while Date() < deadline {
        if try await condition() { return true }
        try await Task.sleep(nanoseconds: 20_000_000)
    }
    return try await condition()
}
