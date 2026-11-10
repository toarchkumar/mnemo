package io.github.toarchkumar.mnemo

import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The embedder runs outside the store's lock (MOBILE-2c), the counterpart
 * of the Swift EmbedderLockTests.
 */
class EmbedderLockTests {
    @Test fun anEmbedderThatCallsBackIntoTheStoreCompletes(): Unit = runBlocking {
        val embedder = CallbackEmbedder()
        val store = MnemoStore(MnemoStoreConfiguration("callback", 32u, scratchDirectory()), InMemoryKeyStorage(), embedder)
        embedder.store = store
        store.remember("already here", WordVectors.of("already here"))

        store.rememberText("a note whose embedder asks the store first")
        val hits = store.recallText("note", RecallOptions(topK = 1u))
        store.essenceAdd(EssenceSection.DECISIONS, "an essence item embedded with a callback")
        store.ingestThread("t", listOf(ThreadTurn("1", "user", "hello", null)))
        assertEquals(0, embedder.deadlocks.get())
        assertTrue(embedder.answers.size >= 4)
        assertTrue(embedder.answers.all { it >= 1uL })
        assertEquals(1, hits.hits.size)
        store.close()
    }

    @Test fun aSlowEmbedderDoesNotBlockAnUnrelatedGet(): Unit = runBlocking {
        val slow = SlowEmbedder(1_500)
        val store = MnemoStore(MnemoStoreConfiguration("slow", 32u, scratchDirectory()), InMemoryKeyStorage(), slow)
        val existing = store.remember("here", WordVectors.of("here"))
        val writer = async(Dispatchers.IO) { store.rememberText("slow to embed") }
        assertTrue(eventually(5_000) { slow.started.get() })
        val start = System.nanoTime()
        assertEquals("here", store.get(existing.id).content)
        val waitedMs = (System.nanoTime() - start) / 1_000_000
        assertTrue("get waited $waitedMs ms for the embedder", waitedMs < 500)
        writer.await()
        assertEquals(2uL, store.count())
        store.close()
    }

    @Test fun embedderErrorsAreUnchangedAndWriteNothing(): Unit = runBlocking {
        val store = MnemoStore(MnemoStoreConfiguration("emberr", 32u, scratchDirectory()), InMemoryKeyStorage())
        val none = expectThrows<MnemoException.Invalid> { store.rememberText("x") }
        assertEquals("no embedder set; call set_embedder first", none.reason)
        store.setEmbedder(WrongWidthEmbedder())
        expectThrows<MnemoException> { store.rememberText("too narrow") }
        assertEquals(0uL, store.count())
        store.close()
    }
}

/** A word-hash vector, as [WordEmbedder] makes, for writes with a vector. */
object WordVectors {
    private val words = WordEmbedder()
    fun of(text: String): List<Float> = words.embed(listOf(text))[0]
}

/**
 * Asks the store for its count before every embed, the way an app's
 * embedder might read a cache or a setting from the store. It waits up to
 * 5 s for the answer and counts a timeout as a deadlock.
 */
class CallbackEmbedder : MnemoEmbedder {
    @Volatile var store: MnemoStore? = null
    val answers = CopyOnWriteArrayList<ULong>()
    val deadlocks = AtomicInteger()
    private val words = WordEmbedder()

    override fun identity() = words.identity()

    override fun embed(texts: List<String>): List<List<Float>> {
        val target = store ?: return words.embed(texts)
        val count = try {
            runBlocking { withTimeout(5_000) { target.count() } }
        } catch (e: kotlinx.coroutines.TimeoutCancellationException) {
            deadlocks.incrementAndGet()
            throw EmbedderException.Failed("deadlock: the store didn't answer within 5 s")
        }
        answers.add(count)
        return words.embed(texts)
    }
}

/** Sleeps before embedding. */
class SlowEmbedder(private val millis: Long) : MnemoEmbedder {
    val started = AtomicBoolean(false)
    private val words = WordEmbedder()

    override fun identity() = words.identity()

    override fun embed(texts: List<String>): List<List<Float>> {
        started.set(true)
        Thread.sleep(millis)
        return words.embed(texts)
    }
}

/** Claims the file's width but returns narrower vectors. */
class WrongWidthEmbedder : MnemoEmbedder {
    override fun identity() = EmbedderIdentity(name = "test-words", dimensions = 32u, normalize = true)
    override fun embed(texts: List<String>): List<List<Float>> = texts.map { listOf(1f, 0f, 0f) }
}
