package io.github.toarchkumar.mnemo

import java.util.concurrent.atomic.AtomicReference
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** v0.12.2 store hardening (MOBILE-2e step 5): RB-002.4 T3 and T9. */
class StoreHardeningTests {
    private fun store(name: String, policy: FlushPolicy, keys: MnemoKeyStorage, embedder: MnemoEmbedder? = null, dims: UInt = 4u) =
        MnemoStore(MnemoStoreConfiguration(name, dims, scratchDirectory(), flushPolicy = policy), keys, embedder)

    /**
     * T3: 5 writes, `flush()`, 20 more from a scope that's then cancelled,
     * and the app moves on without closing. Not run before v0.12.2 (the
     * stores had no flush policy, so the 20 were memory-only until ON_STOP).
     * After: all 25 are on disk once the window passes, under the coalesced
     * default, and at once under EveryWrite. (A JVM store has no destructor
     * and keeps its file lock, so the check reads a byte copy of the file,
     * the state a kill would leave, instead of opening a second store.)
     */
    @Test fun t3AStoreLeftBehindByACancelledScopeStillCommits(): Unit = runBlocking {
        for (policy in listOf(FlushPolicy.Coalesced(300, 64), FlushPolicy.EveryWrite)) {
            val keys = InMemoryKeyStorage()
            val s = store("t3", policy, keys)
            repeat(5) { s.remember("flushed $it", vec(it)) }
            s.flush()
            val scope = CoroutineScope(Dispatchers.IO)
            scope.launch { repeat(20) { s.remember("pending $it", vec(5 + it)) } }.join()
            scope.cancel()
            if (policy == FlushPolicy.EveryWrite) {
                assertEquals(25uL, committedCount(s.file, keys))
            } else {
                assertTrue("$policy: committed within the window", eventually(5_000) { committedCountOrNull(s.file, keys) == 25uL })
            }
            s.close()
        }
    }

    /**
     * T9 (a): an embedder that reads the store (`cacheGet`) before
     * embedding, waiting up to 3 s. Today (before 2c) it timed out, since the
     * call held the store's lock while embedding. After: it returns at once.
     */
    @Test fun t9aAnEmbedderThatCallsCacheGetReturnsAtOnce(): Unit = runBlocking {
        val result = AtomicReference<String>()
        val holder = AtomicReference<MnemoStore>()
        val words = WordEmbedder()
        val embedder = object : MnemoEmbedder {
            override fun identity() = words.identity()
            override fun embed(texts: List<String>): List<List<Float>> {
                val got = runBlocking { withTimeout(3_000) { holder.get().cacheGet("emb", texts[0]) } }
                result.set(got?.decodeToString() ?: "miss")
                return words.embed(texts)
            }
        }
        val s = store("t9a", FlushPolicy.DEFAULT, InMemoryKeyStorage(), embedder, 32u)
        holder.set(s)
        val start = System.nanoTime()
        s.rememberText("hello")
        assertTrue("took ${(System.nanoTime() - start) / 1_000_000} ms", (System.nanoTime() - start) < 2_000_000_000L)
        assertEquals("miss", result.get())
        s.close()
    }

    /**
     * T9 (b): a 1.5 s embed in flight. Today `count()` and the ON_STOP flush
     * both waited about 1.3 s. After: both return in milliseconds.
     */
    @Test fun t9bCountAndTheStopFlushDontWaitForAnEmbed(): Unit = runBlocking {
        val slow = SlowEmbedder(1_500)
        val s = store("t9b", FlushPolicy.DEFAULT, InMemoryKeyStorage(), slow, 32u)
        s.remember("there", WordVectors.of("there"))
        val writer = async(Dispatchers.IO) { s.rememberText("slow to embed") }
        assertTrue(eventually(5_000) { slow.started.get() })
        delay(200)
        var t = System.nanoTime()
        assertEquals(1uL, s.count())
        val countMs = (System.nanoTime() - t) / 1_000_000
        t = System.nanoTime()
        s.flushForBackground()
        val flushMs = (System.nanoTime() - t) / 1_000_000
        assertTrue("count took $countMs ms", countMs < 300)
        assertTrue("the ON_STOP flush took $flushMs ms", flushMs < 300)
        writer.await()
        s.close()
    }

    /** RB-002.5: setMaxSnapshots applies now and after a reopen. */
    @Test fun setMaxSnapshotsAppliesNowAndAfterAReopen(): Unit = runBlocking {
        val s = store("snaps", FlushPolicy.EveryWrite, InMemoryKeyStorage())
        s.setMaxSnapshots(3u)
        repeat(10) { s.remember("n $it", vec(it)) }
        s.close()
        repeat(5) { s.remember("after $it", vec(20 + it)) }
        assertEquals(15uL, s.count())
        s.close()
    }

}
