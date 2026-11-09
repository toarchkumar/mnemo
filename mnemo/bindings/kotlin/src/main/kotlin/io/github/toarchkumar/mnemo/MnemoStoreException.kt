package io.github.toarchkumar.mnemo

/**
 * Errors from the app-facing layer ([MnemoStore], [MnemoKeystore]). Errors
 * from the engine itself are [MnemoException].
 */
sealed class MnemoStoreException(message: String, cause: Throwable? = null) : Exception(message, cause) {
    /**
     * The file exists, but its key isn't available: deleted, never on this
     * device (a file copied from elsewhere), or no longer readable (the
     * Keystore key that wraps it is gone). The data can't be read without
     * it; see the README for recovery.
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

    /** The Android Keystore refused an operation (generate, wrap, unwrap). */
    class Keystore(val operation: String, cause: Throwable) :
        MnemoStoreException("Android Keystore: $operation failed: ${cause.message}", cause)

    /** A file or name problem: an invalid store name, an unwritable directory. */
    class File(message: String, cause: Throwable? = null) : MnemoStoreException(message, cause)
}

/** Overwrites the bytes with zeros. */
internal fun ByteArray.zeroize() = fill(0)
