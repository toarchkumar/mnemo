import Foundation
import Mnemo
import NaturalLanguage

/// Apple's contextual embedding (`NLContextualEmbedding`, a transformer
/// model the system downloads on demand), mean-pooled over its token
/// vectors into one unit vector per text.
///
/// It needs iOS 17 or macOS 14. Its model assets may not be on the device
/// yet: check ``hasAvailableAssets`` and call ``requestAssets()`` (for
/// example behind a "Download Apple's model" button) before the first
/// embed, which otherwise throws.
///
/// The identity names the language and the model revision, for example
/// `apple-nl-contextual-en-r1`. Only the first ``maximumSequenceLength``
/// tokens of a text count.
@available(iOS 17.0, macOS 14.0, *)
public final class AppleContextualEmbedder: MnemoEmbedder, @unchecked Sendable {
    private let embedding: NLContextualEmbedding
    /// The language the embedder asks the model for.
    public let language: NLLanguage
    // Serialises embeds and the lazy load: NLContextualEmbedding isn't
    // documented as thread-safe.
    private let lock = NSLock()
    private var loaded = false

    /// The contextual embedding for `language`, or `nil` when the system has
    /// no model for it. The model's assets may still need downloading.
    public init?(language: NLLanguage = .english) {
        guard let embedding = NLContextualEmbedding(language: language) else { return nil }
        self.embedding = embedding
        self.language = language
    }

    /// The model's revision, part of ``identity()``.
    public var revision: Int { embedding.revision }

    /// The vector width: pass it as the store's `dimensions`.
    public var dimensions: Int { embedding.dimension }

    /// The most tokens of one text the model reads; the rest is ignored.
    public var maximumSequenceLength: Int { embedding.maximumSequenceLength }

    /// The system's identifier for the model.
    public var modelIdentifier: String { embedding.modelIdentifier }

    /// Whether the model's assets are on the device.
    public var hasAvailableAssets: Bool { embedding.hasAvailableAssets }

    /// Asks the system for the model's assets and downloads them if needed.
    /// Returns at once when they're already there.
    @discardableResult
    public func requestAssets() async throws -> AppleModelAssets {
        switch try await embedding.requestAssets() {
        case .available: return .available
        case .notAvailable: return .notAvailable
        case .error: return .notAvailable
        @unknown default: return .notAvailable
        }
    }

    /// Loads the model now rather than on the first embed. Throws when its
    /// assets aren't on the device.
    public func load() throws {
        lock.lock()
        defer { lock.unlock() }
        try loadLocked()
    }

    /// Frees the loaded model; the next embed loads it again.
    public func unload() {
        lock.lock()
        defer { lock.unlock() }
        if loaded { embedding.unload(); loaded = false }
    }

    public func identity() -> EmbedderIdentity {
        EmbedderIdentity(name: "apple-nl-contextual-\(language.rawValue)-r\(embedding.revision)",
                         dimensions: UInt32(embedding.dimension), normalize: true)
    }

    public func embed(texts: [String]) throws -> [[Float]] {
        lock.lock()
        defer { lock.unlock() }
        do {
            try loadLocked()
        } catch {
            throw EmbedderError.Failed(message: "\(error)")
        }
        return try texts.map { text in
            let result: NLContextualEmbeddingResult
            do {
                result = try embedding.embeddingResult(for: text, language: language)
            } catch {
                throw EmbedderError.Failed(message: "NLContextualEmbedding: \(error.localizedDescription)")
            }
            return meanPooled(result, dimension: embedding.dimension)
        }
    }

    private func loadLocked() throws {
        guard !loaded else { return }
        guard embedding.hasAvailableAssets else {
            throw AppleEmbedderError.assetsNotInstalled(modelIdentifier: embedding.modelIdentifier)
        }
        try embedding.load()
        loaded = true
    }
}

/// The mean of a result's token vectors, as a unit vector.
@available(iOS 17.0, macOS 14.0, *)
private func meanPooled(_ result: NLContextualEmbeddingResult, dimension: Int) -> [Float] {
    var sum = [Double](repeating: 0, count: dimension)
    var tokens = 0
    let text = result.string
    result.enumerateTokenVectors(in: text.startIndex..<text.endIndex) { vector, _ in
        if vector.count == dimension {
            for i in 0..<dimension { sum[i] += vector[i] }
            tokens += 1
        }
        return true
    }
    guard tokens > 0 else { return unitVector([Float](repeating: 0, count: dimension)) }
    return unitVector(sum.map { Float($0 / Double(tokens)) })
}

/// Whether a model's assets are on the device after a request.
public enum AppleModelAssets: Sendable, Equatable {
    /// On the device: the embedder can load.
    case available
    /// Not on the device, and the system can't get them now (no network,
    /// or no model for this language).
    case notAvailable
}

/// Errors of the Apple embedders themselves.
public enum AppleEmbedderError: Error, Equatable, CustomStringConvertible {
    /// The contextual model's assets aren't on the device yet: call
    /// `requestAssets()` first.
    case assetsNotInstalled(modelIdentifier: String)

    public var description: String {
        switch self {
        case .assetsNotInstalled(let id):
            return "the assets of Apple's contextual model \(id) aren't on this device; call requestAssets() first"
        }
    }
}
