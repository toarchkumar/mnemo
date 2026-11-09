package io.github.toarchkumar.mnemo.sample

import android.content.Context
import android.util.Log
import io.github.toarchkumar.mnemo.EssenceScope
import io.github.toarchkumar.mnemo.EssenceSection
import io.github.toarchkumar.mnemo.MnemoStore
import io.github.toarchkumar.mnemo.MnemoStoreConfiguration
import io.github.toarchkumar.mnemo.RecallOptions
import io.github.toarchkumar.mnemo.RememberOptions
import io.github.toarchkumar.mnemo.ThreadTurn
import java.io.File

/**
 * Exercises every path R8 could break: the native library through JNA, the
 * callback interface (the embedder), records in both directions, errors,
 * the Keystore and a reopen. The android-emulator workflow launches the
 * minified release APK with `--ez selftest true` and reads the result from
 * logcat (tag `MnemoSelfTest`).
 */
object SelfTest {
    const val TAG = "MnemoSelfTest"

    suspend fun run(context: Context): String {
        val dir = File(context.cacheDir, "selftest").apply { deleteRecursively(); mkdirs() }
        return try {
            val config = MnemoStoreConfiguration(name = "selftest", dimensions = 64u, directory = dir)
            var store = MnemoStore(context, config, embedder = DemoEmbedder())
            store.rememberText("the user prefers green tea", RememberOptions(key = "drink"))
            store.rememberText("the user prefers coffee now", RememberOptions(key = "drink"))
            store.ingestThread("t", listOf(ThreadTurn(turnId = "1", role = "user", content = "hello there")))
            val scope = EssenceScope.Thread("t")
            store.essenceAdd(EssenceSection.DECISIONS, "say hello first", scope)
            store.essenceAppendTimeline("greeted", scope)
            check(store.essence(scope).contains("say hello first")) { "essence" }
            check(store.buildIndex().vectors >= 1uL) { "index" }
            val hits = store.recallText("tea", RecallOptions(topK = 3u, keywords = "tea")).hits
            check(hits.isNotEmpty()) { "no recall hits" }
            check(store.history("drink").size == 2) { "history" }
            val failed = runCatching { store.get("not-a-ulid") }.exceptionOrNull()
            check(failed != null) { "an invalid id didn't throw" }
            store.close()
            store = MnemoStore(context, config, embedder = DemoEmbedder())
            val count = store.count()
            check(store.threadExport("t").size == 1) { "thread" }
            store.close()
            "MNEMO_SELFTEST ok count=$count hits=${hits.size} error=${failed!!::class.java.simpleName}"
        } catch (e: Throwable) {
            "MNEMO_SELFTEST failed: $e"
        } finally {
            dir.deleteRecursively()
        }.also { Log.i(TAG, it) }
    }
}
