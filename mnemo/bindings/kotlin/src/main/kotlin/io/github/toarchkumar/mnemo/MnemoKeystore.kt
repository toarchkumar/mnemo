package io.github.toarchkumar.mnemo

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.security.keystore.KeyProperties
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileNotFoundException
import java.io.FileOutputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.security.KeyStore
import java.util.UUID
import javax.crypto.AEADBadTagException
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
 * - **Errors.** [MnemoStoreException.KeyMissing] means the key is gone: its
 *   file is missing or damaged, the wrapping key is missing or permanently
 *   invalidated, or the file doesn't authenticate. Every other failure (the
 *   Keystore service busy or restarting, a binder error, an I/O error) is
 *   [MnemoStoreException.Keystore], which is retryable and never a reason to
 *   delete anything.
 * - **Key files are durable:** each is synced to disk before it's renamed
 *   into place, and the directory is synced after.
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
        this(
            defaultDirectory(context),
            AndroidKeystoreWrapper(alias, AndroidKeystoreKeys, File(defaultDirectory(context), LOCK_FILE)),
        )

    override fun storeKey(id: String, key: ByteArray) {
        require(key.size == KEY_BYTES) { "a MNemo key is $KEY_BYTES bytes" }
        val (iv, sealed) = wrapper.wrap(key, aad(id))
        val bytes = byteArrayOf(*MAGIC, iv.size.toByte()) + iv + sealed
        val target = fileFor(id)
        ensureDirectory()
        val tmp = File(directory, "${target.name}.${UUID.randomUUID()}.tmp")
        try {
            // Synced before the rename, so a power loss right after a create
            // or an import never leaves the database without its key.
            FileOutputStream(tmp).use { out ->
                out.write(bytes)
                out.fd.sync()
            }
            if (!tmp.renameTo(target)) throw IOException("rename to $target failed")
        } catch (e: IOException) {
            tmp.delete()
            throw MnemoStoreException.File("can't write the key file $target", e)
        }
        syncDirectory(directory)
    }

    override fun key(id: String): ByteArray {
        val file = fileFor(id)
        // Decided by existence, not by the exception: the JVM reports a
        // permission error or a directory as FileNotFoundException too.
        if (!file.exists()) {
            throw MnemoStoreException.KeyMissing(id, null, FileNotFoundException(file.path))
        }
        val bytes = try {
            file.readBytes()
        } catch (e: IOException) {
            // The file is there but couldn't be read now: retryable.
            throw MnemoStoreException.Keystore("read the key file", e)
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
            if (meansKeyMissing(e)) throw MnemoStoreException.KeyMissing(id, null, e)
            // Anything else may pass (the Keystore busy or restarting): the
            // key may well still be there.
            throw MnemoStoreException.Keystore("unwrap", e)
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

    /**
     * The failures that mean the key is gone for good: the file doesn't
     * authenticate (tampered, or wrapped by a key that's been replaced), or
     * the wrapping key is missing or permanently invalidated.
     */
    private fun meansKeyMissing(e: Exception): Boolean =
        e is AEADBadTagException || e is WrappingKeyMissing || isKeyPermanentlyInvalidated(e)

    companion object {
        /** The default `AndroidKeyStore` alias of the wrapping key. */
        const val DEFAULT_ALIAS = "io.github.toarchkumar.mnemo.wrap"

        /** `noBackupFilesDir/mnemo/keys/`. */
        @JvmStatic
        fun defaultDirectory(context: Context): File =
            File(MnemoStoreConfiguration.defaultDirectory(context), "keys")

        internal const val KEY_BYTES = 32
        internal const val LOCK_FILE = ".wrapping-key.lock"
        private const val SUFFIX = ".key"
        private val MAGIC = byteArrayOf('M'.code.toByte(), 'N'.code.toByte(), 'K'.code.toByte(), 1)
        private val ID = Regex("[A-Za-z0-9-]{1,64}")
    }
}

/** The wrapping key's alias isn't in the Keystore: the key is gone, not busy. */
internal class WrappingKeyMissing(alias: String) : Exception("no wrapping key \"$alias\" in AndroidKeyStore")

/** `KeyPermanentlyInvalidatedException`, by class, so JVM tests needn't load it. */
private fun isKeyPermanentlyInvalidated(e: Exception): Boolean = try {
    e is KeyPermanentlyInvalidatedException
} catch (e: LinkageError) {
    false
}

/**
 * Syncs a directory so a rename in it is durable. Best effort: Android allows
 * it through `Os`, and where it doesn't (a JVM test, an old file system) the
 * file itself is already synced.
 */
internal fun syncDirectory(dir: File) {
    runCatching {
        val fd = Os.open(dir.path, OsConstants.O_RDONLY, 0)
        try {
            Os.fsync(fd)
        } finally {
            Os.close(fd)
        }
    }
}

/** Encrypts and decrypts key material (the Keystore on Android; software in tests). */
internal interface KeyWrapper {
    /** Returns the IV and the ciphertext with its tag. */
    fun wrap(plain: ByteArray, aad: ByteArray): Pair<ByteArray, ByteArray>

    fun unwrap(iv: ByteArray, sealed: ByteArray, aad: ByteArray): ByteArray
}

/** Where the wrapping key lives: `AndroidKeyStore` on a device, a fake in JVM tests. */
internal interface WrappingKeys {
    /** The key under [alias], or `null` if the entry can't be read as a key. */
    fun getKey(alias: String): SecretKey?

    fun containsAlias(alias: String): Boolean

    /** Generates a new key under [alias]. Replaces any key there. */
    fun generate(alias: String): SecretKey
}

/** The real `AndroidKeyStore`. */
internal object AndroidKeystoreKeys : WrappingKeys {
    private const val PROVIDER = "AndroidKeyStore"

    private fun store() = KeyStore.getInstance(PROVIDER).apply { load(null) }

    override fun getKey(alias: String): SecretKey? = store().getKey(alias, null) as? SecretKey

    override fun containsAlias(alias: String): Boolean = store().containsAlias(alias)

    override fun generate(alias: String): SecretKey =
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
}

/**
 * AES-256-GCM with a non-exportable key in `AndroidKeyStore`.
 *
 * The wrapping key is created only when its alias doesn't exist. If the
 * alias exists but the key can't be read, that's a retryable
 * [MnemoStoreException.Keystore], never a new key: generating over a live
 * alias would orphan every key wrapped by the old one. Creation holds a file
 * lock in the keys directory, so two processes of one app can't both create.
 */
internal class AndroidKeystoreWrapper(
    private val alias: String,
    private val keys: WrappingKeys,
    private val lockFile: File,
) : KeyWrapper {
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
        val key = wrappingKey(create = false) ?: throw WrappingKeyMissing(alias)
        return Cipher.getInstance(TRANSFORMATION).run {
            init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(TAG_BITS, iv))
            updateAAD(aad)
            doFinal(sealed)
        }
    }

    /**
     * The wrapping key. `null` only when its alias doesn't exist and
     * [create] is false. An alias that exists but gives no key is the
     * retryable [MnemoStoreException.Keystore], whatever [create] says.
     */
    private fun wrappingKey(create: Boolean): SecretKey? = synchronized(LOCK) {
        existingKey()?.let { return it }
        if (!create) return null
        // Another process may be creating it: take the file lock, then look
        // again before generating.
        lockFile.parentFile?.mkdirs()
        RandomAccessFile(lockFile, "rw").use { raf ->
            raf.channel.lock().use {
                existingKey()?.let { return it }
                try {
                    keys.generate(alias)
                } catch (e: Exception) {
                    throw MnemoStoreException.Keystore("generate the wrapping key", e)
                }
            }
        }
    }

    /** The key, or `null` when the alias doesn't exist; throws if it exists unreadable. */
    private fun existingKey(): SecretKey? {
        val key = try {
            keys.getKey(alias)
        } catch (e: Exception) {
            throw MnemoStoreException.Keystore("load the wrapping key", e)
        }
        if (key != null) return key
        val present = try {
            keys.containsAlias(alias)
        } catch (e: Exception) {
            throw MnemoStoreException.Keystore("look up the wrapping key", e)
        }
        if (present) {
            throw MnemoStoreException.Keystore(
                "load the wrapping key",
                IllegalStateException("the alias \"$alias\" exists but returned no key; not replacing it"),
            )
        }
        return null
    }

    private companion object {
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TAG_BITS = 128
        val LOCK = Any()
    }
}
