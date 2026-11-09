package io.github.toarchkumar.mnemo

import android.content.Context
import java.io.File

/**
 * Where a [MnemoStore] keeps its file, and how it creates it.
 *
 * @property name the file name without the `.mnemo` extension: letters,
 *   digits, `-`, `_` and `.`, not starting with a dot; no path separators.
 * @property dimensions the embedding width. Every stored vector must match it.
 * @property directory the directory, or `null` for
 *   `noBackupFilesDir/mnemo/` (see [defaultDirectory]).
 * @property formatVersion `0` creates the current format (v9); `8` creates a
 *   v8 file for readers on v0.10 or earlier. Used only when the file is
 *   created.
 * @property vectorCacheBudgetBytes the in-memory vector column budget
 *   (default 16 MiB).
 * @property cachePages the page cache, in 8 KB pages (default 256 = 2 MiB).
 */
data class MnemoStoreConfiguration(
    val name: String,
    val dimensions: UInt,
    val directory: File? = null,
    val formatVersion: UShort = 0u,
    val vectorCacheBudgetBytes: ULong = DEFAULT_VECTOR_CACHE_BUDGET,
    val cachePages: UInt = DEFAULT_CACHE_PAGES,
) {
    /**
     * The database file this configuration names. [context] supplies the
     * default directory when [directory] is `null`.
     */
    fun file(context: Context?): File {
        val valid = name.isNotEmpty() && !name.startsWith(".") && name.all { it.isAsciiLetterOrDigit() || it in "-_." }
        if (!valid) throw MnemoStoreException.File("invalid store name \"$name\"")
        val dir = directory
            ?: context?.let { defaultDirectory(it) }
            ?: throw MnemoStoreException.File("no directory for \"$name\": pass one, or a Context")
        return File(dir, "$name.mnemo")
    }

    internal val engineConfig: MnemoConfig
        get() = MnemoConfig(
            dimensions = dimensions,
            formatVersion = formatVersion,
            vectorCacheBudgetBytes = vectorCacheBudgetBytes,
            cachePages = cachePages,
        )

    companion object {
        /** 16 MiB, the mobile default. */
        const val DEFAULT_VECTOR_CACHE_BUDGET: ULong = 16_777_216uL

        /** 256 pages of 8 KB = 2 MiB, the mobile default. */
        const val DEFAULT_CACHE_PAGES: UInt = 256u

        /**
         * `noBackupFilesDir/mnemo/`: app-private, excluded from Auto Backup
         * and device transfer, like the Keystore key that protects it.
         */
        @JvmStatic
        fun defaultDirectory(context: Context): File = File(context.noBackupFilesDir, "mnemo")
    }
}

private fun Char.isAsciiLetterOrDigit() = this in 'a'..'z' || this in 'A'..'Z' || this in '0'..'9'
