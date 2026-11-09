package io.github.toarchkumar.mnemo

import android.os.Build
import android.system.ErrnoException
import android.system.Os
import java.io.File
import java.nio.ByteBuffer
import java.nio.file.Files
import java.nio.file.attribute.UserDefinedFileAttributeView

/**
 * The key id stamped on a database file, as the extended attribute
 * `user.io.github.toarchkumar.mnemo.key-id` (the same name as on iOS, in
 * Linux's `user` namespace).
 *
 * The id can't live inside the file: the file is encrypted with the key it
 * names. The stamp is a fast path, not the source of truth. When it's
 * missing (API 24 and 25 have no xattr calls, a file system without user
 * attributes, or a copy that dropped it), [MnemoStore] tries each stored key
 * and stamps the one that opens the file. So writes here are best effort.
 */
internal object KeyId {
    /** The NIO name; Linux adds the `user.` prefix. */
    private const val NAME = "io.github.toarchkumar.mnemo.key-id"
    const val ATTRIBUTE = "user.$NAME"

    private val onAndroid = System.getProperty("java.vm.name") == "Dalvik"

    /** The stamped id, or `null`. */
    fun read(file: File): String? = try {
        val bytes = if (onAndroid) {
            if (Build.VERSION.SDK_INT < 26) return null
            Os.getxattr(file.path, ATTRIBUTE)
        } else {
            val view = view(file) ?: return null
            if (NAME !in view.list()) return null
            ByteBuffer.allocate(view.size(NAME)).also { view.read(NAME, it) }.array()
        }
        String(bytes, Charsets.UTF_8).takeIf { it.isNotEmpty() }
    } catch (e: ErrnoException) {
        null
    } catch (e: Exception) {
        null
    }

    /** Stamps [id] on [file]; `false` when the file system doesn't keep it. */
    fun write(id: String, file: File): Boolean = try {
        val bytes = id.toByteArray(Charsets.UTF_8)
        if (onAndroid) {
            if (Build.VERSION.SDK_INT < 26) return false
            Os.setxattr(file.path, ATTRIBUTE, bytes, 0)
        } else {
            val view = view(file) ?: return false
            view.write(NAME, ByteBuffer.wrap(bytes))
        }
        true
    } catch (e: ErrnoException) {
        false
    } catch (e: Exception) {
        false
    }

    /** Removes the stamp (tests, and files handed to another key). */
    fun remove(file: File) {
        try {
            if (onAndroid) {
                if (Build.VERSION.SDK_INT >= 26) Os.removexattr(file.path, ATTRIBUTE)
            } else {
                view(file)?.takeIf { NAME in it.list() }?.delete(NAME)
            }
        } catch (e: ErrnoException) {
            // not stamped
        } catch (e: Exception) {
            // not supported
        }
    }

    private fun view(file: File): UserDefinedFileAttributeView? =
        Files.getFileAttributeView(file.toPath(), UserDefinedFileAttributeView::class.java)
}
