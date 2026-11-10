package io.github.toarchkumar.mnemo.sample

import android.content.Context
import io.github.toarchkumar.mnemo.MnemoConfig
import io.github.toarchkumar.mnemo.MnemoDb
import io.github.toarchkumar.mnemo.MnemoKeyStorage
import io.github.toarchkumar.mnemo.MnemoStore
import io.github.toarchkumar.mnemo.MnemoStoreConfiguration
import io.github.toarchkumar.mnemo.MnemoStoreException
import io.github.toarchkumar.mnemo.MemoryType
import io.github.toarchkumar.mnemo.RecallOptions
import io.github.toarchkumar.mnemo.RememberOptions
import io.github.toarchkumar.mnemo.compactFileWithKey
import io.github.toarchkumar.mnemo.generateKey
import java.io.File
import java.time.Instant
import org.json.JSONObject

/**
 * The Benchmark screen's engine: the `device_budget` cells at 1K and 10K
 * (design note Q7), on synthetic vectors and raw-key files, through the same
 * Kotlin API an app uses. Phases and JSON keys follow
 * `mnemo/examples/device_budget.rs`; the differences are listed under
 * `corpus` in the output. Call [run] off the main thread.
 */
class DeviceBudget(private val context: Context, private val report: (String) -> Unit) {
    private val root = File(context.cacheDir, "device_budget").apply { mkdirs() }
    private val queries = ClusteredVectors(100, 0xDE71_CE00uL).take(WARM_QUERIES)

    suspend fun run(): JSONObject {
        val started = System.nanoTime()
        val results = JSONObject()
        for (n in SIZES) {
            report("n = $n: building the corpus (cached after the first run)")
            val build = phaseBuild(n)
            val key = keyFile(n).readBytes()
            try {
                report("n = $n: file")
                val file = phaseFile(n, key)
                report("n = $n: one handle, $WARM_QUERIES warm recalls")
                val handle = phaseHandle(n, key)
                report("n = $n: $OPEN_CYCLES fresh handles")
                val cycles = phaseCycles(n, key)
                report("n = $n: raw-key opens")
                val keyopen = phaseKeyOpen(n, key)
                report("n = $n: durable writes")
                val writes = phaseWrites(n, key)
                report("n = $n: background close and reopen")
                val background = phaseBackground(n, key)
                results.put(
                    n.toString(),
                    JSONObject()
                        .put("build", build).put("file", file).put("handle", handle).put("cycles", cycles)
                        .put("writes", writes).put("layers", JSONObject()).put("keyopen", keyopen)
                        .put("background", background),
                )
            } finally {
                key.fill(0)
            }
        }
        return JSONObject()
            .put("schema", "device_budget_mobile/1")
            .put("timestamp", Instant.now().toString())
            .put("device", DeviceInfo.snapshot(context))
            .put("build", DeviceInfo.build())
            .put(
                "corpus",
                JSONObject()
                    .put("dimensions", Corpus.DIMENSIONS)
                    .put("text_bytes", Corpus.TEXT_BYTES)
                    .put("generator", "device_budget.rs text_for + corpus.rs gen_clustered_vectors (seeds 0xBEEFC0DE, 0xDE71CE00, 0x57170001)")
                    .put("key", "raw 32-byte key (createWithKey / openWithKey): no KDF on open")
                    .put("index", "exact scan: build_index runs, but 1K and 10K are below V9_ANN_MIN_FACTS (20,000), where a v9 file builds no IVF+PQ index (as on desktop)")
                    .put("not_measured", "layers, open_secure_kdf_*, *_fsyncs (not in the mobile API), indexed recall (25K+)"),
            )
            .put("results", results)
            .put("elapsed_secs", millisSince(started) / 1_000)
    }

    // --- phases ----------------------------------------------------------

    private fun corpus(n: Int) = File(root, "corpus_${n}_384d_300b_v9.mnemo")

    /** Synthetic bench data only: its raw key sits beside it, which a real app must never do. */
    private fun keyFile(n: Int) = File(root, "corpus_$n.key")

    private fun phaseBuild(n: Int): JSONObject {
        if (corpus(n).exists() && keyFile(n).exists()) return JSONObject().put("built", false)
        corpus(n).delete()
        val started = System.nanoTime()
        val key = generateKey()
        var partitions = 0L
        try {
            withDb(MnemoDb.createWithKey(corpus(n).path, key, CONFIG)) { db ->
                val vectors = ClusteredVectors(Corpus.clusters(n), 0xBEEF_C0DEuL)
                for (i in 0 until n) {
                    db.remember(Corpus.text(i), vectors.next(), SEMANTIC)
                    if (i % 1_000 == 999) report("n = $n: ${i + 1} memories")
                }
                db.flush()
                // As device_budget.rs does. Below V9_ANN_MIN_FACTS (20,000)
                // a v9 file builds nothing, so recall is an exact scan.
                partitions = db.buildIndex().partitions.toLong()
                db.flush()
            }
            keyFile(n).writeBytes(key)
        } finally {
            key.fill(0)
        }
        return JSONObject().put("built", true).put("build_secs", millisSince(started) / 1_000)
            .put("index_partitions", partitions)
    }

    private fun phaseFile(n: Int, key: ByteArray): JSONObject {
        val stats = withDb(MnemoDb.openReadOnlyWithKey(corpus(n).path, key)) { it.stats() }
        val copy = scratchCopy(n, "compact")
        try {
            // CompactReport counts memories (before/after), not bytes:
            // measure the compacted copy itself.
            val compacted = compactFileWithKey(copy.path, key)
            val compactedBytes = copy.length()
            return JSONObject()
                .put("memories", stats.memories.toLong())
                .put("format_version", stats.formatVersion.toInt())
                .put("has_index", stats.hasIndex)
                .put("file_bytes", stats.fileBytes.toLong())
                .put("bytes_per_memory", stats.fileBytes.toDouble() / n)
                .put("compacted_file_bytes", compactedBytes)
                .put("compacted_bytes_per_memory", compactedBytes.toDouble() / n)
                .put("compacted_memories", compacted.after.toLong())
        } finally {
            copy.delete()
        }
    }

    private class HandleRun(
        val firstMs: Double,
        val warm: List<Double>,
        val rssFirst: Long,
        val rssWarm: Long,
        val pssWarm: Long,
    )

    private fun phaseHandle(n: Int, key: ByteArray): JSONObject {
        val pssBefore = DeviceInfo.pssKb()
        val rssBefore = DeviceInfo.rssKb()
        val opened = System.nanoTime()
        val db = MnemoDb.openWithKey(corpus(n).path, key)
        val openMs = millisSince(opened)
        val rssOpen = DeviceInfo.rssKb()
        val run = withDb(db) {
            val t = System.nanoTime()
            it.recall(queries[0], RECALL)
            val firstMs = millisSince(t)
            val rssFirst = DeviceInfo.rssKb()
            val warm = queries.map { q ->
                val w = System.nanoTime()
                it.recall(q, RECALL)
                millisSince(w)
            }
            HandleRun(firstMs, warm, rssFirst, DeviceInfo.rssKb(), DeviceInfo.pssKb())
        }
        val (med, p95) = medianP95(run.warm)
        return JSONObject()
            .put("open_ms", openMs)
            .put("first_recall_ms", run.firstMs)
            .put("warm_recall_median_ms", med)
            .put("warm_recall_p95_ms", p95)
            .put("rss_kb_before_open", rssBefore)
            .put("rss_kb_open", rssOpen)
            .put("rss_kb_after_first_recall", run.rssFirst)
            .put("rss_kb_after_warm", run.rssWarm)
            .put("rss_kb_after_drop", DeviceInfo.rssKb())
            .put("handle_rss_kb", maxOf(0L, run.rssWarm - rssBefore))
            .put("pss_kb_before_open", pssBefore)
            .put("pss_kb_after_warm", run.pssWarm)
            .put("handle_pss_kb", maxOf(0L, run.pssWarm - pssBefore))
            .put("rss_metric", "VmRSS (/proc/self/status); PSS from Debug.MemoryInfo")
    }

    private fun phaseCycles(n: Int, key: ByteArray): JSONObject {
        val opens = ArrayList<Double>()
        val firsts = ArrayList<Double>()
        repeat(OPEN_CYCLES) { i ->
            var t = System.nanoTime()
            val db = MnemoDb.openWithKey(corpus(n).path, key)
            opens.add(millisSince(t))
            withDb(db) {
                t = System.nanoTime()
                it.recall(queries[i % queries.size], RECALL)
                firsts.add(millisSince(t))
            }
        }
        val (om, op) = medianP95(opens)
        val (fm, fp) = medianP95(firsts)
        return JSONObject().put("cycles", OPEN_CYCLES)
            .put("open_median_ms", om).put("open_p95_ms", op)
            .put("first_recall_median_ms", fm).put("first_recall_p95_ms", fp)
    }

    private fun phaseKeyOpen(n: Int, key: ByteArray): JSONObject {
        val before = DeviceInfo.rssKb()
        var afterFirst = 0L
        val opens = ArrayList<Double>()
        repeat(OPEN_CYCLES) { i ->
            val t = System.nanoTime()
            val db = MnemoDb.openWithKey(corpus(n).path, key)
            opens.add(millisSince(t))
            if (i == 0) afterFirst = DeviceInfo.rssKb()
            withDb(db) {}
        }
        val (med, p95) = medianP95(opens)
        return JSONObject().put("open_with_key_median_ms", med).put("open_with_key_p95_ms", p95)
            .put("rss_kb_before_open", before).put("rss_kb_after_key_open", afterFirst)
    }

    private fun phaseWrites(n: Int, key: ByteArray): JSONObject {
        val copy = scratchCopy(n, "writes")
        try {
            val bytesBefore = copy.length()
            val single = ArrayList<Double>()
            val batched = ArrayList<Double>()
            var bytesAfterSingle = 0L
            withDb(MnemoDb.openWithKey(copy.path, key)) { db ->
                val vectors = ClusteredVectors(16, 0x5717_0001uL)
                for (i in 0 until SINGLE_WRITES) {
                    val v = vectors.next()
                    val t = System.nanoTime()
                    db.remember(Corpus.text(n + i), v, SEMANTIC)
                    db.flush()
                    single.add(millisSince(t))
                }
                bytesAfterSingle = copy.length()
                for (b in 0 until BATCHES) {
                    val vs = vectors.take(BATCH_SIZE)
                    val t = System.nanoTime()
                    for (j in 0 until BATCH_SIZE) {
                        db.remember(Corpus.text(n + SINGLE_WRITES + b * BATCH_SIZE + j), vs[j], SEMANTIC)
                    }
                    db.flush()
                    batched.add(millisSince(t))
                }
            }
            val bytesAfterBatches = copy.length()
            val (sm, sp) = medianP95(single)
            val (bm, bp) = medianP95(batched)
            return JSONObject()
                .put("single_write_median_ms", sm)
                .put("single_write_p95_ms", sp)
                .put("batch_size", BATCH_SIZE)
                .put("batch_write_median_ms", bm)
                .put("batch_write_p95_ms", bp)
                .put("batch_write_per_memory_ms", bm / BATCH_SIZE)
                .put("file_growth_per_single_write_bytes", maxOf(0L, bytesAfterSingle - bytesBefore) / SINGLE_WRITES)
                .put("file_growth_per_batch_bytes", maxOf(0L, bytesAfterBatches - bytesAfterSingle) / BATCHES)
        } finally {
            copy.delete()
        }
    }

    /**
     * The background transition, through `MnemoStore`: one pending write,
     * then a close (flush, close, release the lock), then the next call,
     * which reopens. On Android the store flushes on `ON_STOP` and keeps the
     * file open, so the close here is what `close()` costs, the reopen what a
     * process restart's first call costs.
     */
    private suspend fun phaseBackground(n: Int, key: ByteArray): JSONObject {
        val dir = File(root, "background").apply { deleteRecursively(); mkdirs() }
        try {
            corpus(n).copyTo(File(dir, "bg.mnemo"))
            val keys = BenchKeyStorage().apply { storeKey("bench", key) }
            val store = MnemoStore(MnemoStoreConfiguration("bg", Corpus.DIMENSIONS.toUInt(), directory = dir), keys)
            store.count() // first open: finds the key, stamps its id
            val vectors = ClusteredVectors(16, 0xB6_0001uL)
            val closes = ArrayList<Double>()
            val reopens = ArrayList<Double>()
            repeat(OPEN_CYCLES) { i ->
                store.remember(Corpus.text(n + i), vectors.next())
                var t = System.nanoTime()
                store.close()
                closes.add(millisSince(t))
                t = System.nanoTime()
                store.count()
                reopens.add(millisSince(t))
            }
            store.close()
            val (cm, cp) = medianP95(closes)
            val (rm, rp) = medianP95(reopens)
            return JSONObject().put("cycles", OPEN_CYCLES)
                .put("close_median_ms", cm).put("close_p95_ms", cp)
                .put("reopen_median_ms", rm).put("reopen_p95_ms", rp)
                .put("note", "close = flush one pending write + close; reopen = the next call (raw-key open)")
        } finally {
            dir.deleteRecursively()
        }
    }

    // --- helpers ---------------------------------------------------------

    private fun scratchCopy(n: Int, tag: String): File =
        File(root, "scratch_${n}_$tag.mnemo").also { corpus(n).copyTo(it, overwrite = true) }

    companion object {
        val SIZES = listOf(1_000, 10_000)
        const val OPEN_CYCLES = 20
        const val WARM_QUERIES = 1_000
        const val SINGLE_WRITES = 20
        const val BATCHES = 5
        const val BATCH_SIZE = 100

        private val CONFIG = MnemoConfig(dimensions = Corpus.DIMENSIONS.toUInt())
        private val RECALL = RecallOptions(topK = 10u, trackAccess = false)
        private val SEMANTIC = RememberOptions(memoryType = MemoryType.SEMANTIC)
    }
}

/** Closes the engine and frees the handle after [body]. */
inline fun <T> withDb(db: MnemoDb, body: (MnemoDb) -> T): T {
    try {
        val result = body(db)
        db.closeFile()
        return result
    } finally {
        db.close()
    }
}

/** Keys in memory, for the bench's `MnemoStore` (the corpus key is already known). */
class BenchKeyStorage : MnemoKeyStorage {
    private val keys = HashMap<String, ByteArray>()

    @Synchronized override fun storeKey(id: String, key: ByteArray) {
        keys[id] = key.copyOf()
    }

    @Synchronized override fun key(id: String): ByteArray =
        keys[id]?.copyOf() ?: throw MnemoStoreException.KeyMissing(id, null)

    @Synchronized override fun keyIds(): List<String> = keys.keys.sorted()

    @Synchronized override fun deleteKey(id: String) {
        keys.remove(id)?.fill(0)
    }
}
