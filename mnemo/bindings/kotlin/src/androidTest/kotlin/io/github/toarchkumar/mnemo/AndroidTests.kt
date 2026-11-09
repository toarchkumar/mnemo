package io.github.toarchkumar.mnemo

import android.content.Context
import android.os.Build
import android.security.keystore.KeyInfo
import android.system.Os
import android.system.OsConstants
import android.util.Log
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import java.security.KeyStore
import java.util.UUID
import javax.crypto.SecretKey
import javax.crypto.SecretKeyFactory
import kotlinx.coroutines.runBlocking
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * On an emulator (nightly and on tags): the Android Keystore, the
 * `ON_STOP` flush through the real process lifecycle, the default
 * directory, and the native library on this device's page size.
 */
@RunWith(AndroidJUnit4::class)
class AndroidTests {
    private val context: Context = ApplicationProvider.getApplicationContext()
    private val aliases = mutableListOf<String>()
    private val cleanup = mutableListOf<File>()

    @After fun tearDown() {
        val keystore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        aliases.forEach { keystore.deleteEntry(it) }
        cleanup.forEach { it.deleteRecursively() }
    }

    private fun testKeystore(): Pair<MnemoKeystore, String> {
        val alias = "mnemo-test-${UUID.randomUUID()}"
        aliases += alias
        val dir = File(context.cacheDir, "keys-${UUID.randomUUID()}").also { cleanup += it }
        return MnemoKeystore(dir, AndroidKeystoreWrapper(alias)) to alias
    }

    @Test fun theKeystoreWrapsKeysWithANonExportableKey() {
        val (keystore, alias) = testKeystore()
        val key = generateKey()
        keystore.storeKey("k1", key)
        assertArrayEquals(key, keystore.key("k1"))
        assertEquals(listOf("k1"), keystore.keyIds())

        val wrapping = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.getKey(alias, null) as SecretKey
        assertNull("the wrapping key never leaves the Keystore", wrapping.encoded)
        val info = SecretKeyFactory.getInstance(wrapping.algorithm, "AndroidKeyStore")
            .getKeySpec(wrapping, KeyInfo::class.java) as KeyInfo
        assertEquals(256, info.keySize)
        assertFalse(info.isUserAuthenticationRequired)
        Log.i(TAG, "wrapping key inside secure hardware: ${info.isInsideSecureHardware}")

        // A flipped bit, and a wrapping key that's gone: both KeyMissing.
        val file = keystore.fileFor("k1")
        val bytes = file.readBytes()
        bytes[bytes.size - 1] = (bytes[bytes.size - 1].toInt() xor 1).toByte()
        file.writeBytes(bytes)
        assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("k1") }
        keystore.storeKey("k2", generateKey())
        KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(alias)
        val gone = assertThrows(MnemoStoreException.KeyMissing::class.java) { keystore.key("k2") }
        assertEquals("k2", gone.keyId)
    }

    @Test fun aStoreLivesInNoBackupFilesDirWithItsKey(): Unit = runBlocking {
        val dir = MnemoStoreConfiguration.defaultDirectory(context)
        assertEquals(File(context.noBackupFilesDir, "mnemo"), dir)
        assertEquals(File(dir, "keys"), MnemoKeystore.defaultDirectory(context))

        val name = "e2e-${UUID.randomUUID()}"
        val config = MnemoStoreConfiguration(name, 4u)
        val store = MnemoStore(context, config)
        cleanup += store.file
        assertEquals(File(dir, "$name.mnemo"), store.file)
        store.remember("kept in noBackupFilesDir", vec(0))
        store.close()

        // The key id: stamped as an xattr where the API has it (26+).
        val keys = MnemoKeystore(context)
        val id = KeyId.read(store.file)
        if (Build.VERSION.SDK_INT >= 26) {
            assertTrue("the file system keeps ${KeyId.ATTRIBUTE}", id != null)
            assertTrue(File(MnemoKeystore.defaultDirectory(context), "$id.key").isFile)
        }
        assertEquals(1uL, MnemoStore(context, config).count())
        if (id != null) {
            cleanup += File(MnemoKeystore.defaultDirectory(context), "$id.key")
        }
        Log.i(TAG, "key id $id stamped as ${KeyId.ATTRIBUTE}; keys: ${keys.keyIds().size}")
    }

    @Test fun onStopFlushesAndKeepsTheFileOpen(): Unit = runBlocking {
        val (keystore, _) = testKeystore()
        val dir = File(context.cacheDir, "stop-${UUID.randomUUID()}").also { cleanup += it }
        val store = MnemoStore(context, MnemoStoreConfiguration("stop", 4u, dir), keystore)
        // The observer is added on the main thread; wait for it.
        InstrumentationRegistry.getInstrumentation().runOnMainSync {}
        for (i in 0 until 3) store.remember("pending $i", vec(i))
        assertEquals(0, store.backgroundFlushes)

        // Foreground, then background: ProcessLifecycleOwner sends ON_STOP
        // about 700 ms after the last activity stops. (Not moveToState(CREATED):
        // ActivityScenario stops an activity by starting one of its own on top,
        // so the app never leaves the foreground. Closing it does.)
        val scenario = ActivityScenario.launch(TestActivity::class.java)
        scenario.moveToState(Lifecycle.State.RESUMED)
        assertTrue("ProcessLifecycleOwner saw the activity start",
            eventually { processState().isAtLeast(Lifecycle.State.STARTED) })
        scenario.close()
        assertTrue("ON_STOP flushed", eventually(10_000) { store.backgroundFlushes >= 1 })
        assertEquals(Lifecycle.State.CREATED, processState())
        assertTrue("the file stays open", store.isOpen)
        assertNull(store.lastBackgroundError)
        assertCopyHolds(store.file, keystore, 3uL)
        store.remember("after", vec(3))
        assertEquals(4uL, store.count())
        store.close()
    }

    @Test fun theNativeLibraryLoadsOnThisPageSize(): Unit = runBlocking {
        val pageSize = Os.sysconf(OsConstants._SC_PAGESIZE)
        Log.i(TAG, "page size $pageSize, API ${Build.VERSION.SDK_INT}, ABIs ${Build.SUPPORTED_ABIS.joinToString()}")
        InstrumentationRegistry.getArguments().getString("expectPageSize")?.let {
            assertEquals("the emulator's page size", it.toLong(), pageSize)
        }
        // Loads libmnemo_uniffi.so and JNA's dispatch library, and uses them.
        assertEquals(32, generateKey().size)
        val dir = File(context.cacheDir, "page-${UUID.randomUUID()}").also { cleanup += it }
        val store = MnemoStore(MnemoStoreConfiguration("page", 4u, dir), InMemoryKeyStorage())
        for (i in 0 until 50) store.remember("note $i", vec(i))
        assertEquals(50uL, store.count())
        store.close()
    }

    /** The process lifecycle's state, read on the main thread. */
    private fun processState(): Lifecycle.State {
        var state = Lifecycle.State.INITIALIZED
        InstrumentationRegistry.getInstrumentation().runOnMainSync {
            state = ProcessLifecycleOwner.get().lifecycle.currentState
        }
        return state
    }

    private companion object {
        const val TAG = "MnemoAndroidTests"
    }
}
