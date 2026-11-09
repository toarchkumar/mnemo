package io.github.toarchkumar.mnemo

import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The JVM host tests (every PR), the counterpart of the Swift package's
 * StoreTests: the store over the real engine, with keys in memory.
 */
class StoreTests {
    private fun store(name: String, dir: File = scratchDirectory(), keys: MnemoKeyStorage = InMemoryKeyStorage(),
                      dimensions: UInt = 4u, embedder: MnemoEmbedder? = null) =
        MnemoStore(MnemoStoreConfiguration(name, dimensions, dir), keys, embedder)

    @Test fun roundTripAcrossStores(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val config = MnemoStoreConfiguration("notes", 4u, scratchDirectory())
        val store = MnemoStore(config, keys)
        assertFalse(store.isOpen)

        val first = store.remember("alpha", listOf(1f, 0f, 0f, 0f))
        store.remember("beta", listOf(0f, 1f, 0f, 0f))
        assertEquals(WriteOutcome.ADDED, first.outcome)
        assertEquals("alpha", store.get(first.id).content)
        val page = store.recall(listOf(0.9f, 0.1f, 0f, 0f), RecallOptions(topK = 1u))
        assertEquals(listOf("alpha"), page.hits.map { it.memory.content })
        store.close()

        // One key, stamped on the file; a second store on the file opens it.
        val ids = keys.keyIds()
        assertEquals(1, ids.size)
        assertEquals(ids[0], KeyId.read(store.file))
        val again = MnemoStore(config, keys)
        assertEquals(2uL, again.count())
        assertEquals(4u, again.stats().dimensions)
        again.close()
    }

    @Test fun keywordOnlyRecallNeedsNoEmbedder(): Unit = runBlocking {
        val store = store("kw")
        store.remember("the giraffe eats acacia leaves", vec(0))
        store.remember("penguins live in the south", vec(1))
        store.remember("a note about tax forms", vec(2))
        val page = store.recall(emptyList(), RecallOptions(topK = 2u, keywords = "giraffe acacia"))
        assertEquals("the giraffe eats acacia leaves", page.hits.first().memory.content)
        store.close()
    }

    @Test fun kotlinEmbedderAndItsErrors(): Unit = runBlocking {
        val embedder = WordEmbedder()
        val store = store("emb", dimensions = 32u, embedder = embedder)
        store.rememberText("the user prefers dark roast coffee")
        store.rememberText("the meeting moved to thursday afternoon")
        val hit = store.recallText("what coffee does the user like", RecallOptions(topK = 1u))
        assertEquals("the user prefers dark roast coffee", hit.hits.first().memory.content)
        assertTrue(embedder.callCount >= 3)

        // An embedder that throws: Embedder, and nothing is written.
        store.setEmbedder(FailingEmbedder())
        val failed = expectThrows<MnemoException.Embedder> { store.rememberText("never stored") }
        assertEquals("model not loaded", failed.reason)
        store.setEmbedder(CrashingEmbedder())
        val crashed = expectThrows<MnemoException.Embedder> { store.rememberText("never stored either") }
        assertTrue(crashed.reason.isNotEmpty())
        assertEquals(2uL, store.count())

        // A width that doesn't match the file is refused.
        expectThrows<MnemoException> { store.setEmbedder(WordEmbedder(8u)) }
        store.close()
    }

    @Test fun keyedWritesKeepHistory(): Unit = runBlocking {
        val store = store("keyed")
        val first = store.remember("tea", vec(0), RememberOptions(key = "drink"))
        val same = store.remember("tea", vec(0), RememberOptions(key = "drink"))
        val second = store.remember("coffee", vec(1), RememberOptions(key = "drink"))
        assertEquals(WriteOutcome.ADDED, first.outcome)
        assertEquals(WriteOutcome.UNCHANGED, same.outcome)
        assertEquals(WriteOutcome.SUPERSEDED, second.outcome)
        assertEquals(listOf("coffee", "tea"), store.history("drink").map { it.content }) // newest first
        store.close()
    }

    @Test fun threadsIngestExportForget(): Unit = runBlocking {
        val store = store("threads", dimensions = 32u, embedder = WordEmbedder())
        val turns = listOf(
            ThreadTurn("t1", "user", "plan the trip to Lisbon", 1_700_000_000),
            ThreadTurn("t2", "assistant", "flights first, then a hotel", 1_700_000_060),
        )
        val report = store.ingestThread("trip", turns)
        assertTrue(report.addedRecords >= 2uL)
        val again = store.ingestThread("trip", turns)
        assertEquals(0uL, again.addedRecords)
        assertEquals(2uL, again.skippedExistingTurns)
        val exported = store.threadExport("trip")
        assertEquals(listOf("t1", "t2"), exported.map { it.turnId })
        assertEquals("plan the trip to Lisbon", exported.first().content)
        assertTrue(store.threadForget("trip") >= 2uL)
        assertTrue(store.threadExport("trip").isEmpty())
        store.close()
    }

    @Test fun passphraseExportThenImport(): Unit = runBlocking {
        val dir = scratchDirectory()
        val keys = InMemoryKeyStorage()
        val store = store("source", dir, keys)
        store.remember("moves with the user", vec(0))
        store.remember("so does this", vec(1))

        // The export opens with the passphrase, and not with the device key.
        val export = File(dir, "transfer.mnemo")
        store.exportWithPassphrase(export, "correct horse battery staple")
        MnemoDb.openReadOnly(export.path, "correct horse battery staple").use {
            assertEquals(2uL, it.len())
            it.closeFile()
        }
        val sourceKey = keys.key(checkNotNull(KeyId.read(store.file)))
        expectThrows<MnemoException> { MnemoDb.openReadOnlyWithKey(export.path, sourceKey).close() }

        // The import gets a new key under a new id, stamped on the new file.
        val otherKeys = InMemoryKeyStorage()
        val imported = MnemoStore.importWithPassphrase(
            export, "correct horse battery staple",
            MnemoStoreConfiguration("imported", 4u, dir), otherKeys,
        )
        val newId = checkNotNull(KeyId.read(imported.file))
        assertEquals(listOf(newId), otherKeys.keyIds())
        assertEquals(2uL, imported.count())
        expectThrows<MnemoException> { MnemoDb.openReadOnly(imported.file.path, "correct horse battery staple").close() }

        // The destination must not exist.
        expectThrows<MnemoException> {
            MnemoStore.importWithPassphrase(export, "correct horse battery staple",
                MnemoStoreConfiguration("imported", 4u, dir), InMemoryKeyStorage())
        }
        imported.close()
        store.close()
    }

    @Test fun aLostKeyIdIsRecoveredAndAMissingKeyIsClear(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val dir = scratchDirectory()
        // Two other stores' keys sit in the same key storage.
        for (name in listOf("other-a", "other-b")) {
            val other = store(name, dir, keys)
            other.remember("x", vec(0))
            other.close()
        }
        val config = MnemoStoreConfiguration("stamp", 4u, dir)
        val store = MnemoStore(config, keys)
        store.remember("kept", vec(0))
        store.close()
        val id = checkNotNull(KeyId.read(store.file))

        // The attribute is gone (a copy that dropped it): the right key is
        // found by trying each, and the id is stamped again.
        KeyId.remove(store.file)
        assertNull(KeyId.read(store.file))
        val reopened = MnemoStore(config, keys)
        assertEquals(1uL, reopened.count())
        assertEquals(id, KeyId.read(store.file))
        reopened.close()

        // The key itself is gone: a clear KeyMissing naming the id and file.
        keys.deleteKey(id)
        val orphan = MnemoStore(config, keys)
        val stamped = expectThrows<MnemoStoreException.KeyMissing> { orphan.count() }
        assertEquals(id, stamped.keyId)
        assertEquals(store.file.path, stamped.path)
        KeyId.remove(store.file)
        val unstamped = expectThrows<MnemoStoreException.KeyMissing> { MnemoStore(config, keys).count() }
        assertNull(unstamped.keyId)
        assertEquals(store.file.path, unstamped.path)
    }

    @Test fun onStopFlushKeepsTheHandleOpen(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val store = store("stop", keys = keys)
        store.remember("pending one", vec(0))
        store.remember("pending two", vec(1))

        // What ProcessLifecycleOwner's ON_STOP runs: a flush, the file still open.
        store.onProcessStop()
        assertTrue(eventually { store.backgroundFlushes == 1 })
        assertTrue(store.isOpen)
        assertNull(store.lastBackgroundError)
        assertCopyHolds(store.file, keys, 2uL)

        store.remember("after", vec(2))
        assertEquals(3uL, store.count())
        store.close()
        assertUnlockedAndDurable(store.file, keys, 3uL)
    }

    @Test fun aClosedStoreAndAClosedHandleBothReopen(): Unit = runBlocking {
        val keys = InMemoryKeyStorage()
        val store = store("reopen", keys = keys)
        store.remember("one", vec(0))

        // close() releases the lock; the next call reopens.
        store.close()
        assertFalse(store.isOpen)
        assertUnlockedAndDurable(store.file, keys, 1uL)
        assertEquals(1uL, store.count())
        assertTrue(store.isOpen)

        // The engine handle closed underneath: the call reopens and retries.
        store.closeHandleUnderneath()
        store.remember("two", vec(1))
        assertEquals(2uL, store.count())
        store.close()
    }

    @Test fun callsRacingClosesNeverSeeClosed(): Unit = runBlocking {
        val store = store("race")
        coroutineScope {
            for (i in 0 until 60) {
                launch(Dispatchers.Default) { store.remember("note $i", vec(i)) }
                if (i % 6 == 0) {
                    launch(Dispatchers.Default) { store.close() }
                    launch(Dispatchers.Default) { store.flushForBackground() }
                }
            }
        }
        assertEquals(60uL, store.count())
        store.close()
    }

    @Test fun compactKeepsTheKeyAndTheData(): Unit = runBlocking {
        val store = store("compact")
        for (i in 0 until 20) store.remember("note $i", vec(i))
        val id = KeyId.read(store.file)
        assertNotNull(id)
        store.compact()
        assertEquals(id, KeyId.read(store.file))
        assertEquals(20uL, store.count())
        store.close()
    }

    @Test fun resultCacheRoundTrip(): Unit = runBlocking {
        val store = store("cache")
        store.cachePut("llm", "prompt-1", byteArrayOf(1, 2, 3))
        assertTrue(byteArrayOf(1, 2, 3).contentEquals(store.cacheGet("llm", "prompt-1")))
        assertNull(store.cacheGet("llm", "prompt-2"))
        store.close()
    }

    @Test fun invalidNamesAreRefused() {
        for (name in listOf("", "../escape", ".hidden", "a/b")) {
            assertThrowsStoreFile { MnemoStore(MnemoStoreConfiguration(name, 4u, scratchDirectory()), InMemoryKeyStorage()) }
        }
        // No directory and no Context.
        assertThrowsStoreFile { MnemoStore(MnemoStoreConfiguration("x", 4u), InMemoryKeyStorage()) }
    }

    private fun assertThrowsStoreFile(block: () -> Unit) {
        org.junit.Assert.assertThrows(MnemoStoreException.File::class.java) { block() }
    }
}
