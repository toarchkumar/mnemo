package io.github.toarchkumar.mnemo

import java.io.File
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * MnemoKeystore's key files on the JVM, with a software AES-GCM key in place
 * of the Android Keystore (the instrumented tests cover the real one).
 */
class KeystoreFileTests {
    private fun keystore(dir: File = scratchDirectory()) = MnemoKeystore(File(dir, "keys"), SoftwareKeyWrapper())

    @Test fun keysRoundTripAndAreNeverWrittenInTheClear() {
        val keystore = keystore()
        val key = generateKey()
        keystore.storeKey("a1", key)
        assertArrayEquals(key, keystore.key("a1"))
        assertEquals(listOf("a1"), keystore.keyIds())

        val onDisk = keystore.fileFor("a1").readBytes()
        assertFalse(onDisk.toList().windowed(key.size).any { it == key.toList() })

        // Replacing, then deleting; deleting again is fine.
        val other = generateKey()
        keystore.storeKey("a1", other)
        assertArrayEquals(other, keystore.key("a1"))
        keystore.deleteKey("a1")
        assertEquals(emptyList<String>(), keystore.keyIds())
        keystore.deleteKey("a1")
        val missing = assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("a1") }
        assertEquals("a1", missing.keyId)
    }

    @Test fun aTamperedOrSwappedKeyFileIsKeyMissing() {
        val keystore = keystore()
        keystore.storeKey("first", generateKey())
        keystore.storeKey("second", generateKey())

        // A file renamed to another id fails: the id is authenticated.
        keystore.fileFor("first").copyTo(keystore.fileFor("second"), overwrite = true)
        assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("second") }

        // A flipped bit fails.
        val file = keystore.fileFor("first")
        val bytes = file.readBytes()
        bytes[bytes.size - 1] = (bytes[bytes.size - 1].toInt() xor 1).toByte()
        file.writeBytes(bytes)
        val tampered = assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("first") }
        assertTrue(tampered.cause != null)

        // So does a truncated one, and one that isn't a key file.
        file.writeBytes(bytes.copyOf(6))
        assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("first") }
        file.writeText("not a key")
        assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("first") }
    }

    @Test fun idsCantEscapeTheDirectory() {
        val keystore = keystore()
        for (id in listOf("../x", "a/b", "", ".hidden")) {
            assertThrows(IllegalArgumentException::class.java) { keystore.storeKey(id, generateKey()) }
        }
        assertThrows(IllegalArgumentException::class.java) { keystore.storeKey("short", ByteArray(16)) }
    }

    @Test fun aStoreOnTheKeystoreEndToEnd(): Unit = runBlocking {
        val dir = scratchDirectory()
        val keystore = keystore(dir)
        val config = MnemoStoreConfiguration("wrapped", 4u, dir)
        val store = MnemoStore(config, keystore)
        store.remember("kept under a wrapped key", vec(0))
        store.close()

        val id = checkNotNull(KeyId.read(store.file))
        assertEquals(listOf(id), keystore.keyIds())
        assertTrue(keystore.fileFor(id).isFile)
        assertEquals(1uL, MnemoStore(config, keystore).count())
    }
}
