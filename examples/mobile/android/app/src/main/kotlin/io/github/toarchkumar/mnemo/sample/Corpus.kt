package io.github.toarchkumar.mnemo.sample

import kotlin.math.cos
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlin.math.sqrt

/**
 * The `device_budget` corpus, ported from `mnemo/examples/device_budget.rs`
 * and `mnemo/benches/corpus.rs`: about 300 bytes of text and 384-dim
 * clustered unit vectors per memory, the same ones the desktop bench builds.
 */
object Corpus {
    const val DIMENSIONS = 384
    const val TEXT_BYTES = 300

    private val words = listOf(
        "agent", "memory", "decided", "release", "the", "user", "prefers", "tea", "index", "thread",
        "essence", "budget", "device", "flush", "recall", "because",
    )

    /** Deterministic ~300-byte ASCII text for memory [i] (`text_for`). */
    fun text(i: Int): String {
        val s = StringBuilder("memory $i:")
        var k = i.toULong() * 2_654_435_761uL + 1uL
        while (s.length < TEXT_BYTES) {
            k = k * 6_364_136_223_846_793_005uL + 1_442_695_040_888_963_407uL
            s.append(' ').append(words[((k shr 33) % words.size.toULong()).toInt()])
        }
        return s.substring(0, TEXT_BYTES)
    }

    /** The cluster count the bench uses for [n] memories. */
    fun clusters(n: Int): Int = max(kotlin.math.ceil(sqrt(n.toDouble())).toInt(), 16)
}

/**
 * `gen_clustered_vectors` as a stream: the centres first, then one vector per
 * call, in the Rust generator's order.
 */
class ClusteredVectors(clusters: Int, seed: ULong, private val dims: Int = Corpus.DIMENSIONS) {
    private var state = seed * 0x9E37_79B9_7F4A_7C15uL + 1uL
    private val centres: List<FloatArray>
    private var index = 0

    init {
        centres = List(clusters) { FloatArray(dims) { nextNormal() }.also(::normalize) }
    }

    fun next(): List<Float> {
        val centre = centres[index % centres.size]
        index++
        val v = FloatArray(dims) { centre[it] + NOISE_SIGMA * nextNormal() }
        normalize(v)
        return v.asList()
    }

    fun take(n: Int): List<List<Float>> = List(n) { next() }

    private fun nextU64(): ULong {
        state += 0x9E37_79B9_7F4A_7C15uL
        var z = state
        z = (z xor (z shr 30)) * 0xBF58_476D_1CE4_E5B9uL
        z = (z xor (z shr 27)) * 0x94D0_49BB_1331_11EBuL
        return z xor (z shr 31)
    }

    private fun nextNormal(): Float {
        val u1 = max((nextU64() shr 11).toDouble() / (1uL shl 53).toDouble(), 1e-12)
        val u2 = (nextU64() shr 11).toDouble() / (1uL shl 53).toDouble()
        val r = sqrt(-2.0 * ln(u1))
        return (r * cos(2.0 * Math.PI * u2)).toFloat()
    }

    private fun normalize(v: FloatArray) {
        var norm = 0f
        for (x in v) norm += x * x
        norm = sqrt(norm)
        if (norm == 0f) v[0] = 1f else for (i in v.indices) v[i] /= norm
    }

    private companion object {
        const val NOISE_SIGMA = 0.1f
    }
}

/** Median and 95th percentile, picked as `device_budget.rs` picks them. */
fun medianP95(xs: List<Double>): Pair<Double, Double> {
    if (xs.isEmpty()) return 0.0 to 0.0
    val s = xs.sorted()
    fun at(p: Double) = s[min((p * (s.size - 1)).roundToInt(), s.size - 1)]
    return at(0.5) to at(0.95)
}

/** Milliseconds since [startNanos] (`System.nanoTime`). */
fun millisSince(startNanos: Long): Double = (System.nanoTime() - startNanos) / 1_000_000.0
