package io.github.toarchkumar.mnemo

/**
 * Errors from the app-facing layer ([MnemoStore], [MnemoKeystore]). Errors
 * from the engine itself are [MnemoException].
 */
sealed class MnemoStoreException(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /**
     * The file exists, but its key is gone: deleted, never on this device (a
     * file copied from elsewhere), damaged, or wrapped by a Keystore key
     * that's missing or permanently invalidated. The data can't be read
     * without it; see the README for recovery. A failure that may pass is
     * [Keystore] instead.
     *
     * @property keyId the id the file is stamped with, or `null` when it
     *   has no stamp and no stored key opens it.
     * @property path the database file, when known.
     */
    class KeyMissing(val keyId: String?, val path: String?, cause: Throwable? = null) :
        MnemoStoreException(
            buildString {
                append("the key for ")
                append(path ?: "this store")
                append(" is missing")
                if (keyId != null) append(" (key id $keyId)")
            },
            cause,
        )

    /**
     * The Android Keystore, or the key file behind it, failed an operation
     * (generate, wrap, unwrap, a read) for a reason that can pass: the
     * Keystore service busy or restarting, a binder error, an I/O error.
     * **Retryable**: try again later. The key is probably still there, so this
     * is never a reason to delete the database or its key.
     */
    class Keystore(val operation: String, cause: Throwable) :
        MnemoStoreException("Android Keystore: $operation failed: ${cause.message}", cause)

    /** A file or name problem: an invalid store name, an unwritable directory. */
    class File(message: String, cause: Throwable? = null) : MnemoStoreException(message, cause)
}

/** Overwrites the bytes with zeros. */
internal fun ByteArray.zeroize() = fill(0)
