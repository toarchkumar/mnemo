import Foundation
import NaturalLanguage
import Testing
import Mnemo
@testable import MnemoAppleEmbedder

/// SAMPLE-1 part 1: Apple's embedders. They need the system's model assets,
/// so every test that needs a model is skipped, not failed, when the
/// machine running it doesn't have that model.
enum Assets {
    static let sentence = NLEmbedding.sentenceEmbedding(for: .english) != nil

    /// The contextual model exists on this system (its assets may not).
    static var contextualModel: Bool {
        if #available(iOS 17.0, macOS 14.0, *) { return AppleContextualEmbedder() != nil }
        return false
    }

    /// The contextual model can embed now: its assets are on the device.
    static var contextualInstalled: Bool {
        if #available(iOS 17.0, macOS 14.0, *) { return AppleContextualEmbedder()?.hasAvailableAssets ?? false }
        return false
    }
}

func cosine(_ a: [Float], _ b: [Float]) -> Float {
    zip(a, b).reduce(0) { $0 + $1.0 * $1.1 }
}

func norm(_ v: [Float]) -> Float {
    v.reduce(0) { $0 + $1 * $1 }.squareRoot()
}

let similar = ["The cat is sleeping on the sofa.", "A kitten naps on the couch."]
let unrelated = "The quarterly tax return is due on the fifteenth."

@Suite struct AppleSentenceEmbedderTests {
    @Test(.enabled(if: Assets.sentence, "no English sentence embedding on this system"))
    func identityNamesTheLanguageAndRevision() throws {
        let e = try #require(AppleSentenceEmbedder())
        let id = e.identity()
        #expect(id.name == "apple-nl-sentence-en-r\(e.revision)")
        #expect(id.dimensions == UInt32(e.dimensions))
        #expect(id.normalize)
        #expect(e.dimensions > 0)
    }

    @Test(.enabled(if: Assets.sentence, "no English sentence embedding on this system"))
    func vectorsAreUnitLengthAndSimilarTextsAreCloser() throws {
        let e = try #require(AppleSentenceEmbedder())
        let v = try e.embed(texts: similar + [unrelated])
        #expect(v.count == 3)
        for x in v {
            #expect(x.count == e.dimensions)
            #expect(abs(norm(x) - 1) < 1e-3)
        }
        #expect(cosine(v[0], v[1]) > cosine(v[0], v[2]))
    }

    @Test(.enabled(if: Assets.sentence, "no English sentence embedding on this system"))
    func aStoreRecallsByMeaning() async throws {
        let e = try #require(AppleSentenceEmbedder())
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "sentence", dimensions: UInt32(e.dimensions),
                                                   directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: e)
        try await store.rememberText("The user's dog is called Biscuit.")
        try await store.rememberText(unrelated)
        let top = try await store.recallText("What's the name of my pet?", options: RecallOptions(topK: 1))
        #expect(top.hits.first?.memory.content == "The user's dog is called Biscuit.")
        try await store.close()
    }

    /// The revision is in the identity, so a store written with one revision
    /// refuses the embedder of another.
    @Test(.enabled(if: Assets.sentence, "no English sentence embedding on this system"))
    func aStoreRefusesAnotherRevision() async throws {
        let e = try #require(AppleSentenceEmbedder())
        let config = MnemoStoreConfiguration(name: "revision", dimensions: UInt32(e.dimensions),
                                             directory: try scratchDirectory())
        let keys = MemoryKeyStore()
        let store = try MnemoStore(configuration: config, keyStore: keys, embedder: e)
        try await store.rememberText("written with this revision")
        try await store.close()

        // The store opens the file lazily, so the check runs on the first call.
        let other = RenamedEmbedder(e, name: "apple-nl-sentence-en-r\(e.revision + 1)")
        let wrong = try MnemoStore(configuration: config, keyStore: keys, embedder: other)
        await #expect(throws: MnemoError.self) { try await wrong.count() }
        try? await wrong.close()

        let right = try MnemoStore(configuration: config, keyStore: keys, embedder: e)
        #expect(try await right.count() >= 1)
        try await right.close()
    }

    @Test func aZeroVectorBecomesANonZeroUnitVector() {
        #expect(unitVector([0, 0, 0]) == [1, 0, 0])
        #expect(unitVector([3, 4]) == [0.6, 0.8])
    }
}

@Suite struct AppleContextualEmbedderTests {
    @Test(.enabled(if: Assets.contextualModel, "no contextual model on this system (needs iOS 17 or macOS 14)"))
    func identityNamesTheLanguageAndRevision() throws {
        guard #available(iOS 17.0, macOS 14.0, *) else { return }
        let e = try #require(AppleContextualEmbedder())
        let id = e.identity()
        #expect(id.name == "apple-nl-contextual-en-r\(e.revision)")
        #expect(id.dimensions == UInt32(e.dimensions))
        #expect(id.normalize)
        #expect(e.maximumSequenceLength > 0)
        #expect(!e.modelIdentifier.isEmpty)
    }

    @Test(.enabled(if: Assets.contextualInstalled, "the contextual model's assets aren't on this machine"))
    func meanPooledVectorsAreUnitLengthAndSimilarTextsAreCloser() throws {
        guard #available(iOS 17.0, macOS 14.0, *) else { return }
        let e = try #require(AppleContextualEmbedder())
        let v = try e.embed(texts: similar + [unrelated, ""])
        #expect(v.count == 4)
        for x in v {
            #expect(x.count == e.dimensions)
            #expect(abs(norm(x) - 1) < 1e-3)
        }
        #expect(cosine(v[0], v[1]) > cosine(v[0], v[2]))
    }

    @Test(.enabled(if: Assets.contextualInstalled, "the contextual model's assets aren't on this machine"))
    func aStoreRecallsByMeaning() async throws {
        guard #available(iOS 17.0, macOS 14.0, *) else { return }
        let e = try #require(AppleContextualEmbedder())
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "contextual", dimensions: UInt32(e.dimensions),
                                                   directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: e)
        try await store.rememberText("The user's dog is called Biscuit.")
        try await store.rememberText(unrelated)
        let top = try await store.recallText("What's my dog's name?", options: RecallOptions(topK: 1))
        #expect(top.hits.first?.memory.content == "The user's dog is called Biscuit.")
        try await store.close()
    }

    /// Without its assets the model can't load: an embed throws, so no write
    /// happens with a missing vector.
    @Test(.enabled(if: Assets.contextualModel && !Assets.contextualInstalled,
                   "the contextual model's assets are installed here (or there's no model)"))
    func withoutAssetsAnEmbedThrows() throws {
        guard #available(iOS 17.0, macOS 14.0, *) else { return }
        let e = try #require(AppleContextualEmbedder())
        #expect(throws: (any Error).self) { try e.embed(texts: ["hello"]) }
        #expect(throws: AppleEmbedderError.self) { try e.load() }
    }
}

@Suite struct AppleEmbeddersTests {
    @Test(.enabled(if: Assets.sentence || Assets.contextualInstalled, "no Apple embedding on this system"))
    func bestAvailablePicksTheContextualModelOnlyWhenItsAssetsAreThere() throws {
        let best = try #require(AppleEmbedders.bestAvailable())
        let name = best.identity().name
        if Assets.contextualInstalled {
            #expect(name.hasPrefix("apple-nl-contextual-en-r"))
        } else {
            #expect(name.hasPrefix("apple-nl-sentence-en-r"))
        }
    }
}

/// Another identity over the same vectors, to stand in for another model
/// revision.
final class RenamedEmbedder: MnemoEmbedder, @unchecked Sendable {
    private let inner: any MnemoEmbedder
    private let name: String
    init(_ inner: any MnemoEmbedder, name: String) { self.inner = inner; self.name = name }
    func identity() -> EmbedderIdentity {
        var id = inner.identity()
        id.name = name
        return id
    }
    func embed(texts: [String]) throws -> [[Float]] { try inner.embed(texts: texts) }
}

/// Keys in memory: the macOS test runner has no Keychain entitlement.
final class MemoryKeyStore: MnemoKeyStore, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [String: Data] = [:]
    func storeKey(_ key: Data, id: String, policy: KeyPolicy) throws {
        lock.lock(); defer { lock.unlock() }; keys[id] = key
    }
    func key(id: String) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        guard let k = keys[id] else { throw MnemoStoreError.keyMissing(keyID: id, path: nil) }
        return k
    }
    func keyIDs() throws -> [String] { lock.lock(); defer { lock.unlock() }; return keys.keys.sorted() }
    func deleteKey(id: String) throws { lock.lock(); defer { lock.unlock() }; keys[id] = nil }
}

func scratchDirectory() throws -> URL {
    let dir = FileManager.default.temporaryDirectory
        .appendingPathComponent("mnemo-apple-embedder-tests")
        .appendingPathComponent(UUID().uuidString)
    try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir
}
