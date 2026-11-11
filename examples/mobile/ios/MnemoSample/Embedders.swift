import Foundation
import Mnemo

/// Apple's embedders come from the package's `MnemoAppleEmbedder` product
/// (SAMPLE-1). This is the last resort, for a system with no Apple model:
/// hashed words, so texts that share words come out similar. It has no
/// idea what words mean; it's labelled as a demo in the UI.
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
