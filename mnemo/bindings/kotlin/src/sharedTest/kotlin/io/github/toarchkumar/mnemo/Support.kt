package io.github.toarchkumar.mnemo

import java.io.File
import java.nio.file.Files
import java.security.SecureRandom
import java.util.concurrent.atomic.AtomicInteger
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows

/** Keys in memory, so the JVM tests need no Keystore. */
class InMemoryKeyStorage : MnemoKeyStorage {
    private val keys = LinkedHashMap<String, ByteArray>()

    @Synchronized override fun storeKey(id: String, key: ByteArray) {
        keys[id] = key.copyOf()
    }

    @Synchronized override fun key(id: String): ByteArray =
        keys[id]?.copyOf() ?: throw MnemoStoreException.KeyMissing(id, null)

    @Synchronized override fun keyIds(): List<String> = keys.keys.sorted()

    @Synchronized override fun deleteKey(id: String) {
        keys.remove(id)?.fill(0)
    }
}

/** AES-GCM in software, standing in for the Android Keystore. */
internal class SoftwareKeyWrapper : KeyWrapper {
    private val key = SecretKeySpec(ByteArray(32).also { SecureRandom().nextBytes(it) }, "AES")

    override fun wrap(plain: ByteArray, aad: ByteArray): Pair<ByteArray, ByteArray> {
        val iv = ByteArray(12).also { SecureRandom().nextBytes(it) }
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key, GCMParameterSpec(128, iv))
        cipher.updateAAD(aad)
        return iv to cipher.doFinal(plain)
    }

    override fun unwrap(iv: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv))
        cipher.updateAAD(aad)
        return cipher.doFinal(sealed)
    }
}

/** A deterministic bag-of-words embedder: texts that share words get similar vectors. */
class WordEmbedder(private val dimensions: UInt = 32u) : MnemoEmbedder {
    private val calls = AtomicInteger()
    val callCount: Int get() = calls.get()

    override fun identity() = EmbedderIdentity(name = "test-words", dimensions = dimensions, normalize = true)

    override fun embed(texts: List<String>): List<List<Float>> {
        calls.incrementAndGet()
        return texts.map(::vector)
    }

    private fun vector(text: String): List<Float> {
        val v = FloatArray(dimensions.toInt())
        for (word in text.lowercase().split(Regex("[^\\p{L}\\p{N}]+")).filter { it.isNotEmpty() }) {
            var h = 2_166_136_261u
            for (b in word.toByteArray(Charsets.UTF_8)) h = (h xor b.toUByte().toUInt()) * 16_777_619u
            v[(h % dimensions).toInt()] += 1f
        }
        if (v.all { it == 0f }) v[0] = 1f
        val norm = kotlin.math.sqrt(v.sumOf { (it * it).toDouble() }).toFloat()
        return v.map { it / norm }
    }
}

/** Throws the binding's own error type. */
class FailingEmbedder : MnemoEmbedder {
    override fun identity() = EmbedderIdentity(name = "test-words", dimensions = 32u, normalize = true)
    override fun embed(texts: List<String>): List<List<Float>> = throw EmbedderException.Failed("model not loaded")
}

/** Throws an arbitrary Kotlin exception. */
class CrashingEmbedder : MnemoEmbedder {
    override fun identity() = EmbedderIdentity(name = "test-words", dimensions = 32u, normalize = true)
    override fun embed(texts: List<String>): List<List<Float>> = throw IllegalStateException("boom")
}

/** A fresh, empty directory for one test. */
fun scratchDirectory(): File = Files.createTempDirectory("mnemo-kotlin-tests").toFile()

/** A non-zero 4-wide vector. */
fun vec(i: Int): List<Float> = listOf(i + 1f, (i % 3).toFloat(), 0.5f, 1f)

/** Waits until [condition] holds, or gives up after [millis]. */
suspend fun eventually(millis: Long = 10_000, condition: suspend () -> Boolean): Boolean {
    val deadline = System.currentTimeMillis() + millis
    while (System.currentTimeMillis() < deadline) {
        if (condition()) return true
        delay(20)
    }
    return condition()
}

/** Asserts that [block] throws a [T], and returns it. */
inline fun <reified T : Throwable> expectThrows(noinline block: suspend () -> Unit): T =
    assertThrows(T::class.java) { runBlocking { block() } }

/**
 * A copy of [file], opened with its key, holds [count] memories: everything
 * written before the last flush reached the disk, even with the original
 * still open (and locked) by its store.
 */
fun assertCopyHolds(file: File, keys: MnemoKeyStorage, count: ULong) {
    val copy = File(file.parentFile, "copy-${System.nanoTime()}.mnemo")
    file.copyTo(copy)
    try {
        assertOpensWith(copy, keys.key(checkNotNull(KeyId.read(file)) { "no key id on $file" }), count)
    } finally {
        copy.delete()
    }
}

/** [file]'s lock is free (a second writer opens it), and it holds [count] memories. */
fun assertUnlockedAndDurable(file: File, keys: MnemoKeyStorage, count: ULong) {
    assertOpensWith(file, keys.key(checkNotNull(KeyId.read(file)) { "no key id on $file" }), count)
}

private fun assertOpensWith(file: File, key: ByteArray, count: ULong) {
    try {
        val other = MnemoDb.openWithKey(file.path, key)
        try {
            assertEquals(count, other.len())
            other.closeFile()
        } finally {
            other.close()
        }
    } finally {
        key.fill(0)
    }
}
