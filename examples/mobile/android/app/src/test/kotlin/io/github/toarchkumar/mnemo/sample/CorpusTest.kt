package io.github.toarchkumar.mnemo.sample

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * The port matches `device_budget.rs` and `corpus.rs`: the expected values
 * are what the Rust generator prints (and the Swift port, identically).
 */
class CorpusTest {
    @Test
    fun textMatchesTheRustGenerator() {
        assertEquals(
            "memory 0: prefers thread device prefers essence release essence prefers thread essence budget decided " +
                "device agent essence device essence because the user budget flush recall decided flush agent the " +
                "agent memory agent because decided device user recall essence essence essence device flush because th",
            Corpus.text(0),
        )
        assertEquals(300, Corpus.text(9_999).length)
    }

    @Test
    fun vectorsMatchTheRustGenerator() {
        val v = ClusteredVectors(16, 0xBEEF_C0DEuL).next()
        assertEquals(0.008277863f, v[0], 1e-6f)
        assertEquals(-0.04379678f, v[1], 1e-6f)
        assertEquals(0.01814693f, v[383], 1e-6f)
        val q = ClusteredVectors(100, 0xDE71_CE00uL).take(3)
        assertEquals(-0.014266164f, q[2][0], 1e-6f)
    }

    @Test
    fun percentilesPickLikeTheBench() {
        assertEquals(3.0 to 5.0, medianP95(listOf(5.0, 1.0, 3.0, 2.0, 4.0)))
    }
}
