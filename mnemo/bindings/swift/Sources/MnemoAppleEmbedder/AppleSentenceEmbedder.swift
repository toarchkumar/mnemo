import Foundation
import Mnemo
import NaturalLanguage

/// Apple's built-in sentence embedding (`NLEmbedding.sentenceEmbedding`),
/// so an app ships no model of its own. English is 512 dimensions.
///
/// Available on every system the package supports (iOS 15, macOS 12). Its
/// identity names the language and the model revision, for example
/// `apple-nl-sentence-en-r1`, so a store written with one revision refuses
/// an embedder of another (see ``AppleEmbedders`` for moving between
/// models).
public final class AppleSentenceEmbedder: MnemoEmbedder, @unchecked Sendable {
    private let embedding: NLEmbedding
    /// The language the model embeds.
    public let language: NLLanguage
    // NLEmbedding isn't documented as thread-safe, and the engine may call
    // from any thread.
    private let lock = NSLock()

    /// The sentence embedding for `language`, at the system's current
    /// revision, or `nil` when the system has none for that language.
    public convenience init?(language: NLLanguage = .english) {
        guard let embedding = NLEmbedding.sentenceEmbedding(for: language) else { return nil }
        self.init(embedding: embedding, language: language)
    }

    /// The sentence embedding for `language` at a given model revision, or
    /// `nil` when the system doesn't have that revision. Use it to keep
    /// opening a store written with an older revision.
    public convenience init?(language: NLLanguage, revision: Int) {
        guard let embedding = NLEmbedding.sentenceEmbedding(for: language, revision: revision) else {
            return nil
        }
        self.init(embedding: embedding, language: language)
    }

    private init(embedding: NLEmbedding, language: NLLanguage) {
        self.embedding = embedding
        self.language = language
    }

    /// The model's revision, part of ``identity()``.
    public var revision: Int { embedding.revision }

    /// The vector width: pass it as the store's `dimensions`.
    public var dimensions: Int { embedding.dimension }

    public func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "apple-nl-sentence-\(language.rawValue)-r\(embedding.revision)",
                         dimensions: UInt32(embedding.dimension), normalize: true)
    }

    public func embed(texts: [String]) throws -> [[Float]] {
        lock.lock()
        defer { lock.unlock() }
        return try texts.map { text in
            guard let v = embedding.vector(for: text) else {
                throw EmbedderError.Failed(message: "NLEmbedding has no vector for this text")
            }
            return unitVector(v.map { Float($0) })
        }
    }
}

/// A unit vector, and a non-zero one for an all-zero input (the engine
/// refuses all-zero vectors).
func unitVector(_ v: [Float]) -> [Float] {
    let norm = v.reduce(0) { $0 + $1 * $1 }.squareRoot()
    guard norm > 0, norm.isFinite else {
        var unit = [Float](repeating: 0, count: v.count)
        if !unit.isEmpty { unit[0] = 1 }
        return unit
    }
    return v.map { $0 / norm }
}
