package io.github.toarchkumar.mnemo.check

import io.github.toarchkumar.mnemo.EssenceScope
import io.github.toarchkumar.mnemo.EssenceSection
import io.github.toarchkumar.mnemo.MnemoKeyStorage
import io.github.toarchkumar.mnemo.MnemoStore
import io.github.toarchkumar.mnemo.MnemoStoreConfiguration
import io.github.toarchkumar.mnemo.MnemoStoreException
import io.github.toarchkumar.mnemo.RecallOptions
import java.nio.file.Files
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** Keys in memory: the JVM has no Android Keystore. */
private class MemoryKeys : MnemoKeyStorage {
    private val keys = HashMap<String, ByteArray>()
    @Synchronized override fun storeKey(id: String, key: ByteArray) { keys[id] = key.copyOf() }
    @Synchronized override fun key(id: String): ByteArray =
        keys[id]?.copyOf() ?: throw MnemoStoreException.KeyMissing(id, null)
    @Synchronized override fun keyIds(): List<String> = keys.keys.sorted()
    @Synchronized override fun deleteKey(id: String) { keys.remove(id) }
}

/** One round trip through the published artifact: write, recall, essence, reopen. */
class ConsumerTest {
    @Test fun roundTripThroughThePublishedArtifact(): Unit = runBlocking {
        val dir = Files.createTempDirectory("mnemo-release-check").toFile()
        val keys = MemoryKeys()
        val config = MnemoStoreConfiguration("consumer", 4u, dir)
        val store = MnemoStore(config, keys)
        store.remember("the user prefers green tea", listOf(1f, 0f, 0f, 0f))
        store.remember("flight UA 931 on 12 March", listOf(0f, 1f, 0f, 0f))
        store.essenceAdd(EssenceSection.DECISIONS, "keep the sample small")
        val top = store.recall(listOf(0.9f, 0.1f, 0f, 0f), RecallOptions(topK = 1u)).hits.first().memory.content
        store.close()

        val again = MnemoStore(config, keys)
        assertTrue("count", again.count() >= 2uL) // the essence item is a record too
        assertEquals("the user prefers green tea", top)
        assertTrue(again.essence(EssenceScope.Project).contains("keep the sample small"))
        again.close()
    }
}
