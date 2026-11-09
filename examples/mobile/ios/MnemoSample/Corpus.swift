import Foundation

/// The `device_budget` corpus, ported from `mnemo/examples/device_budget.rs`
/// and `mnemo/benches/corpus.rs` so the phone builds the same memories the
/// desktop bench measures: about 300 bytes of text and 384-dim clustered
/// unit vectors per memory.
enum Corpus {
    static let dimensions = 384
    static let textBytes = 300

    private static let words = [
        "agent", "memory", "decided", "release", "the", "user", "prefers", "tea", "index", "thread",
        "essence", "budget", "device", "flush", "recall", "because",
    ]

    /// Deterministic ~300-byte ASCII text for memory `i` (`text_for`).
    static func text(_ i: Int) -> String {
        var s = "memory \(i):"
        var k = UInt64(i) &* 2_654_435_761 &+ 1
        while s.utf8.count < textBytes {
            k = k &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
            s += " " + words[Int((k >> 33) % UInt64(words.count))]
        }
        return String(s.utf8.prefix(textBytes))!
    }

    /// The cluster count the bench uses for `n` memories.
    static func clusters(for n: Int) -> Int {
        max(Int(Double(n).squareRoot().rounded(.up)), 16)
    }
}

/// `gen_clustered_vectors`, as a stream: the centres first, then one vector
/// per call, in the Rust generator's order (so the same seed gives the same
/// vectors, up to libm rounding).
struct ClusteredVectors {
    private var state: UInt64
    private var centres: [[Float]] = []
    private var index = 0
    private let dims: Int
    private static let noiseSigma: Float = 0.1

    init(dims: Int = Corpus.dimensions, clusters: Int, seed: UInt64) {
        self.dims = dims
        state = seed &* 0x9E37_79B9_7F4A_7C15 &+ 1
        centres.reserveCapacity(clusters)
        for _ in 0..<clusters {
            var c = (0..<dims).map { _ in nextNormal() }
            Self.normalize(&c)
            centres.append(c)
        }
    }

    mutating func next() -> [Float] {
        let centre = centres[index % centres.count]
        index += 1
        var v = centre.map { $0 + Self.noiseSigma * nextNormal() }
        Self.normalize(&v)
        return v
    }

    /// `n` vectors at once.
    mutating func take(_ n: Int) -> [[Float]] {
        (0..<n).map { _ in next() }
    }

    private mutating func nextU64() -> UInt64 {
        state = state &+ 0x9E37_79B9_7F4A_7C15
        var z = state
        z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
        z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
        return z ^ (z >> 31)
    }

    private mutating func nextNormal() -> Float {
        let u1 = max(Double(nextU64() >> 11) / Double(UInt64(1) << 53), 1e-12)
        let u2 = Double(nextU64() >> 11) / Double(UInt64(1) << 53)
        let r = (-2.0 * log(u1)).squareRoot()
        return Float(r * cos(2.0 * Double.pi * u2))
    }

    private static func normalize(_ v: inout [Float]) {
        var norm: Float = 0
        for x in v { norm += x * x }
        norm = norm.squareRoot()
        if norm == 0 {
            v[0] = 1
        } else {
            for i in v.indices { v[i] /= norm }
        }
    }
}

/// Median and 95th percentile, picked as `device_budget.rs` picks them.
func medianP95(_ xs: [Double]) -> (Double, Double) {
    let s = xs.sorted()
    guard !s.isEmpty else { return (0, 0) }
    func at(_ p: Double) -> Double { s[min(Int((p * Double(s.count - 1)).rounded()), s.count - 1)] }
    return (at(0.5), at(0.95))
}

/// Milliseconds since `start`.
func millis(since start: DispatchTime) -> Double {
    Double(DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds) / 1_000_000
}
