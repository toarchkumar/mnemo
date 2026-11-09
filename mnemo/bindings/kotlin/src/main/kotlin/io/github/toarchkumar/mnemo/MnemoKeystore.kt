package io.github.toarchkumar.mnemo

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.io.File
import java.io.IOException
import java.security.KeyStore
import java.util.UUID
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Keys for [MnemoStore] on Android, wrapped by the Android Keystore.
 *
 * - **The wrapping key** is one AES-256-GCM key in `AndroidKeyStore` under
 *   [alias]: generated on first use, non-exportable (it never leaves the
 *   Keystore, in hardware where the device has it), and usable without user
 *   authentication, so stores work in the background and after a lock-screen
 *   change.
 * - **Each MNemo key** (32 random bytes per file) is encrypted by it and
 *   written to `noBackupFilesDir/mnemo/keys/<id>.key`, with the key id as
 *   associated data, so a key file can't be passed off under another id.
 * - **Device-only.** Keystore keys never leave the device, and
 *   `noBackupFilesDir` is excluded from backups and device transfers. A file
 *   restored or copied elsewhere fails with [MnemoStoreException.KeyMissing];
 *   move data with [MnemoStore.exportWithPassphrase] instead.
 *
 * No `androidx.security:security-crypto`: it's deprecated, and this is the
 * part of it that's needed.
 */
class MnemoKeystore internal constructor(
    private val directory: File,
    private val wrapper: KeyWrapper,
) : MnemoKeyStorage {
    /**
     * @param alias the `AndroidKeyStore` alias of the wrapping key. Keep the
     *   default unless two independent key sets must not share one.
     */
    @JvmOverloads
    constructor(context: Context, alias: String = DEFAULT_ALIAS) :
        this(defaultDirectory(context), AndroidKeystoreWrapper(alias))

    override fun storeKey(id: String, key: ByteArray) {
        require(key.size == KEY_BYTES) { "a MNemo key is $KEY_BYTES bytes" }
        val (iv, sealed) = wrapper.wrap(key, aad(id))
        val bytes = byteArrayOf(*MAGIC, iv.size.toByte()) + iv + sealed
        val target = fileFor(id)
        ensureDirectory()
        val tmp = File(directory, "${target.name}.${UUID.randomUUID()}.tmp")
        try {
            tmp.writeBytes(bytes)
            if (!tmp.renameTo(target)) throw IOException("rename to $target failed")
        } catch (e: IOException) {
            tmp.delete()
            throw MnemoStoreException.File("can't write the key file $target", e)
        }
    }

    override fun key(id: String): ByteArray {
        val file = fileFor(id)
        val bytes = try {
            file.readBytes()
        } catch (e: IOException) {
            throw MnemoStoreException.KeyMissing(id, null, e)
        }
        val header = MAGIC.size + 1
        if (bytes.size <= header || !bytes.copyOfRange(0, MAGIC.size).contentEquals(MAGIC)) {
            throw MnemoStoreException.KeyMissing(id, null, IOException("$file isn't a MNemo key file"))
        }
        val ivLength = bytes[MAGIC.size].toInt()
        if (ivLength <= 0 || bytes.size <= header + ivLength) {
            throw MnemoStoreException.KeyMissing(id, null, IOException("$file is truncated"))
        }
        val iv = bytes.copyOfRange(header, header + ivLength)
        val sealed = bytes.copyOfRange(header + ivLength, bytes.size)
        val key = try {
            wrapper.unwrap(iv, sealed, aad(id))
        } catch (e: MnemoStoreException) {
            throw e
        } catch (e: Exception) {
            // A tampered file, or a wrapping key that's gone or invalidated.
            throw MnemoStoreException.KeyMissing(id, null, e)
        }
        if (key.size != KEY_BYTES) {
            key.zeroize()
            throw MnemoStoreException.KeyMissing(id, null, IOException("$file holds no 32-byte key"))
        }
        return key
    }

    override fun keyIds(): List<String> =
        directory.listFiles { f -> f.isFile && f.name.endsWith(SUFFIX) }
            ?.map { it.name.removeSuffix(SUFFIX) }
            ?.filter { ID.matches(it) }
            ?.sorted()
            ?: emptyList()

    override fun deleteKey(id: String) {
        val file = fileFor(id)
        if (file.exists() && !file.delete()) {
            throw MnemoStoreException.File("can't delete the key file $file")
        }
    }

    /** The key file for [id] (tests). */
    internal fun fileFor(id: String): File {
        require(ID.matches(id)) { "invalid key id \"$id\"" }
        return File(directory, "$id$SUFFIX")
    }

    private fun ensureDirectory() {
        if (!directory.isDirectory && !directory.mkdirs() && !directory.isDirectory) {
            throw MnemoStoreException.File("can't create $directory")
        }
    }

    private fun aad(id: String) = "mnemo-key:$id".toByteArray(Charsets.UTF_8)

    companion object {
        /** The default `AndroidKeyStore` alias of the wrapping key. */
        const val DEFAULT_ALIAS = "io.github.toarchkumar.mnemo.wrap"

        /** `noBackupFilesDir/mnemo/keys/`. */
        @JvmStatic
        fun defaultDirectory(context: Context): File =
            File(MnemoStoreConfiguration.defaultDirectory(context), "keys")

        internal const val KEY_BYTES = 32
        private const val SUFFIX = ".key"
        private val MAGIC = byteArrayOf('M'.code.toByte(), 'N'.code.toByte(), 'K'.code.toByte(), 1)
        private val ID = Regex("[A-Za-z0-9-]{1,64}")
    }
}

/** Encrypts and decrypts key material (the Keystore on Android; software in tests). */
internal interface KeyWrapper {
    /** Returns the IV and the ciphertext with its tag. */
    fun wrap(plain: ByteArray, aad: ByteArray): Pair<ByteArray, ByteArray>

    fun unwrap(iv: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray
}

/** AES-256-GCM with a non-exportable key in `AndroidKeyStore`. */
internal class AndroidKeystoreWrapper(private val alias: String) : KeyWrapper {
    override fun wrap(plain: ByteArray, aad: ByteArray): Pair<ByteArray, ByteArray> {
        val cipher = try {
            Cipher.getInstance(TRANSFORMATION).apply {
                // The Keystore picks a fresh random IV (randomized encryption).
                init(Cipher.ENCRYPT_MODE, wrappingKey(create = true))
                updateAAD(aad)
            }
        } catch (e: MnemoStoreException) {
            throw e
        } catch (e: Exception) {
            throw MnemoStoreException.Keystore("wrap", e)
        }
        return try {
            cipher.iv to cipher.doFinal(plain)
        } catch (e: Exception) {
            throw MnemoStoreException.Keystore("wrap", e)
        }
    }

    override fun unwrap(iv: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray {
        val key = wrappingKey(create = false)
            ?: throw IllegalStateException("no wrapping key \"$alias\" in AndroidKeyStore")
        return Cipher.getInstance(TRANSFORMATION).run {
            init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, iv))
            updateAAD(aad)
            doFinal(sealed)
        }
    }

    /**
     * The wrapping key; with [create], made on first use. Creation is
     * serialized in the process: generating under an existing alias would
     * replace the key and orphan every key wrapped by the old one.
     */
    private fun wrappingKey(create: Boolean): SecretKey? = synchronized(LOCK) {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        if (!create) return null
        try {
            KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER).run {
                init(
                    KeyGenParameterSpec.Builder(
                        alias,
                        KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                    )
                        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                        .setKeySize(256)
                        .setRandomizedEncryptionRequired(true)
                        .setUserAuthenticationRequired(false)
                        .build(),
                )
                generateKey()
            }
        } catch (e: Exception) {
            throw MnemoStoreException.Keystore("generate the wrapping key", e)
        }
    }

    private companion object {
        const val PROVIDER = "AndroidKeyStore"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
        val LOCK = Any()
    }
}
