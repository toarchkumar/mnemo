package io.github.toarchkumar.mnemo

/**
 * Where [MnemoStore] keeps the 32-byte keys of its files, by key id.
 *
 * [MnemoKeystore] is the Android implementation. Tests can supply their own,
 * for example one that keeps keys in memory.
 *
 * Every method may block (disk, Keystore IPC). [MnemoStore] calls them on its
 * dispatcher, never on the main thread. Returned arrays belong to the caller,
 * who zeroes them after use.
 */
interface MnemoKeyStorage {
    /** Stores [key] under [id], replacing any key already there. */
    fun storeKey(id: String, key: ByteArray)

    /**
     * The key stored under [id].
     *
     * @throws MnemoStoreException.KeyMissing when there's none, or it can't
     *   be read.
     */
    fun key(id: String): ByteArray

    /** Every stored key id. */
    fun keyIds(): List<String>

    /** Deletes the key under [id]; deleting a missing key is fine. */
    fun deleteKey(id: String)
}
