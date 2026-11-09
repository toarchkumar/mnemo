import Foundation
import Mnemo
import NaturalLanguage

/// Apple's built-in sentence embedding (`NLEmbedding`, NaturalLanguage), so
/// the sample ships no model. 512 dimensions for English.
final class SentenceEmbedder: MnemoEmbedder, @unchecked Sendable {
    private let embedding: NLEmbedding
    private let language: NLLanguage
    // NLEmbedding isn't documented as thread-safe; the engine may call from
    // any thread.
    private let lock = NSLock()

    init?(language: NLLanguage = .english) {
        guard let embedding = NLEmbedding.sentenceEmbedding(for: language) else { return nil }
        self.embedding = embedding
        self.language = language
    }

    var dimensions: Int { embedding.dimension }

    func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "apple-nl-sentence-\(language.rawValue)-r\(embedding.revision)",
                         dimensions: UInt32(embedding.dimension), normalize: true)
    }

    func embed(texts: [String]) throws -> [[Float]] {
        lock.lock()
        defer { lock.unlock() }
        return try texts.map { text in
            guard let v = embedding.vector(for: text) else {
                throw EmbedderError.Failed(message: "NLEmbedding has no vector for this text")
            }
            return normalized(v.map { Float($0) })
        }
    }
}

/// A demo embedder for when `NLEmbedding` isn't available: hashed words, so
/// texts that share words come out similar. It has no idea what words mean;
/// it's labelled as a demo in the UI.
final class DemoWordEmbedder: MnemoEmbedder, @unchecked Sendable {
    let dimensions = 64

    func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "demo-word-hash", dimensions: UInt32(dimensions), normalize: true)
    }

    func embed(texts: [String]) throws -> [[Float]] {
        texts.map { text in
            var v = [Float](repeating: 0, count: dimensions)
            for word in text.lowercased().split(whereSeparator: { !$0.isLetter && !$0.isNumber }) {
                var h: UInt32 = 2_166_136_261
                for b in word.utf8 { h = (h ^ UInt32(b)) &* 16_777_619 }
                v[Int(h % UInt32(dimensions))] += 1
            }
            return normalized(v)
        }
    }
}

/// A unit vector (a non-zero one for empty input: the engine refuses
/// all-zero vectors).
func normalized(_ v: [Float]) -> [Float] {
    let norm = v.reduce(0) { $0 + $1 * $1 }.squareRoot()
    guard norm > 0 else {
        var unit = [Float](repeating: 0, count: v.count)
        if !unit.isEmpty { unit[0] = 1 }
        return unit
    }
    return v.map { $0 / norm }
}
