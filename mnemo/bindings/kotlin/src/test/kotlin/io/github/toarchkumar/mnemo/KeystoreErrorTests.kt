package io.github.toarchkumar.mnemo

import java.io.File
import java.security.ProviderException
import java.util.concurrent.atomic.AtomicInteger
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * RB-001.21 (v0.12.2): a failure that can pass is the retryable
 * [MnemoStoreException.Keystore], never [MnemoStoreException.KeyMissing],
 * and the wrapping key is never generated over an alias that exists.
 */
class KeystoreErrorTests {
    /** Throws [ProviderException] on the first unwrap, then works. */
    private class FlakyWrapper : KeyWrapper {
        private val inner = SoftwareKeyWrapper()
        val unwraps = AtomicInteger()
        override fun wrap(plain: ByteArray, aad: ByteArray) = inner.wrap(plain, aad)
        override fun unwrap(iv: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray {
            if (unwraps.incrementAndGet() == 1) throw ProviderException("keystore busy")
            return inner.unwrap(iv, sealed, aad)
        }
    }

    /** A fake Keystore for [AndroidKeystoreWrapper]: software AES keys by alias. */
    private class FakeKeys(var aliasExists: Boolean = false, var keyReadable: Boolean = true) : WrappingKeys {
        val generated = AtomicInteger()
        private var key: SecretKey? = null

        override fun getKey(alias: String): SecretKey? = if (aliasExists && keyReadable) key else null
        override fun containsAlias(alias: String) = aliasExists
        override fun generate(alias: String): SecretKey {
            generated.incrementAndGet()
            val k = KeyGenerator.getInstance("AES").apply { init(256) }.generateKey()
            key = k
            aliasExists = true
            keyReadable = true
            return k
        }
    }

    private fun androidKeystore(keys: FakeKeys, dir: File = scratchDirectory()): MnemoKeystore {
        val keysDir = File(dir, "keys")
        return MnemoKeystore(keysDir, AndroidKeystoreWrapper("test.alias", keys, File(keysDir, MnemoKeystore.LOCK_FILE)))
    }

    @Test fun aTransientUnwrapFailureIsRetryableNotKeyMissing() {
        val wrapper = FlakyWrapper()
        val keystore = MnemoKeystore(File(scratchDirectory(), "keys"), wrapper)
        val key = generateKey()
        keystore.storeKey("k1", key)

        // Before v0.12.2 this was KeyMissing, whose documented recovery
        // includes deleting the file.
        val first = assertThrows(MnemoStoreException.Keystore::class.java) { keystore.key("k1") }
        assertEquals("unwrap", first.operation)
        assertTrue(first.cause is ProviderException)
        // The retry works: the key was there all along.
        assertArrayEquals(key, keystore.key("k1"))
        assertEquals(2, wrapper.unwraps.get())
    }

    @Test fun anAliasThatExistsButGivesNoKeyIsRetryableAndNeverRegenerated() {
        val keys = FakeKeys()
        val keystore = androidKeystore(keys)
        val key = generateKey()
        keystore.storeKey("k1", key)
        assertEquals(1, keys.generated.get())

        // The alias is there but its entry can't be read right now.
        keys.keyReadable = false
        val read = assertThrows(MnemoStoreException.Keystore::class.java) { keystore.key("k1") }
        assertTrue(read.cause?.message.orEmpty().contains("exists but returned no key"))
        // Storing a new key must not generate a replacement wrapping key.
        assertThrows(MnemoStoreException.Keystore::class.java) { keystore.storeKey("k2", generateKey()) }
        assertEquals("no key generated over a live alias", 1, keys.generated.get())

        // Once the entry is readable again, the old key still opens.
        keys.keyReadable = true
        assertArrayEquals(key, keystore.key("k1"))
    }

    @Test fun aMissingAliasIsKeyMissingAndCreationHappensOnce() {
        val keys = FakeKeys()
        val dir = scratchDirectory()
        val keystore = androidKeystore(keys, dir)
        keystore.storeKey("k1", generateKey())
        keystore.storeKey("k2", generateKey())
        assertEquals("one wrapping key for every file", 1, keys.generated.get())
        assertTrue(File(dir, "keys/${MnemoKeystore.LOCK_FILE}").exists())

        // The alias itself is gone (the Keystore was wiped): the keys are lost.
        keys.aliasExists = false
        val missing = assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("k1") }
        assertTrue(missing.cause is WrappingKeyMissing)
    }

    @Test fun aMissingFileIsKeyMissingButAnUnreadableOneIsRetryable() {
        val dir = File(scratchDirectory(), "keys")
        val keystore = MnemoKeystore(dir, SoftwareKeyWrapper())
        assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("nothing") }

        // A key "file" that exists but can't be read as a file.
        keystore.storeKey("k1", generateKey())
        val file = keystore.fileFor("k1")
        assertTrue(file.delete())
        assertTrue(file.mkdir())
        assertThrows(MnemoStoreException.Keystore::class.java) { keystore.key("k1") }
    }

    @Test fun keyFilesAreWrittenWholeWithNoTemporariesLeft() {
        val dir = File(scratchDirectory(), "keys")
        val keystore = MnemoKeystore(dir, SoftwareKeyWrapper())
        repeat(5) { keystore.storeKey("k$it", generateKey()) }
        val leftovers = dir.listFiles { f -> f.name.endsWith(".tmp") }.orEmpty()
        assertFalse("no temporary files left", leftovers.isNotEmpty())
        assertEquals(5, keystore.keyIds().size)
    }
}
