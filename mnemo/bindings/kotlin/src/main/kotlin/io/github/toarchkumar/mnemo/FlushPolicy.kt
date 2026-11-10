package io.github.toarchkumar.mnemo

/**
 * When a [MnemoStore] commits its writes to disk on its own.
 *
 * A write is durable once it's committed: a crash, a kill (`am force-stop`,
 * the low-memory killer) or a power loss after that keeps it. Before that,
 * it's in memory only. Whatever the policy, the store also commits on every
 * `ON_STOP`, and on [MnemoStore.flush] and [MnemoStore.close].
 */
sealed class FlushPolicy {
    /**
     * The default. The first write after a commit starts a timer, and the
     * store commits once when it fires ([maxDelayMillis] later) or as soon as
     * [maxWrites] writes are waiting, whichever comes first. A write made
     * while the app is in the foreground is durable at most about
     * [maxDelayMillis] after it returns. While the app is stopped (in the
     * background, or started without an activity), each write commits
     * before it returns.
     */
    data class Coalesced(val maxDelayMillis: Long = 1_000, val maxWrites: Int = 64) : FlushPolicy()

    /**
     * Every write commits before it returns. The safest, and the slowest for
     * bursts: each commit writes and syncs pages.
     */
    data object EveryWrite : FlushPolicy()

    /**
     * The store commits only on the events above (the behaviour before
     * v0.12.1). For benchmarks, and for apps that call [MnemoStore.flush]
     * themselves.
     */
    data object Manual : FlushPolicy()

    companion object {
        /** `Coalesced(maxDelayMillis = 1000, maxWrites = 64)`. */
        @JvmField
        val DEFAULT: FlushPolicy = Coalesced()
    }
}

/**
 * A batch of calls on one open handle, from [MnemoStore.withSession].
 *
 * The calls run in order on the store's handle, with no other call in
 * between, and the store commits once at the end. A session doesn't embed:
 * pass vectors (embed first, then open the session). It's valid only inside
 * the block it was passed to; afterwards every call throws
 * [MnemoException.Closed].
 */
class MnemoSession internal constructor(private val db: MnemoDb) {
    @Volatile private var ended = false

    /** The writes made in this session. */
    @Volatile internal var writes = 0
        private set

    internal fun end() {
        ended = true
    }

    private fun handle(write: Boolean): MnemoDb {
        if (ended) throw MnemoException.Closed()
        if (write) writes += 1
        return db
    }

    /** Stores a memory with its vector. */
    fun remember(content: String, vector: List<Float>, options: RememberOptions = RememberOptions()): WriteResult =
        handle(true).remember(content, vector, options)

    /** Changes fields of a memory. A content change needs its vector in the patch: a session doesn't embed. */
    fun update(id: String, patch: MemoryPatch): WriteOutcome {
        if (patch.content != null && patch.vector == null) {
            throw MnemoException.Invalid("a session doesn't embed: a content change needs its vector in the patch")
        }
        return handle(true).update(id, patch)
    }

    /** Deletes a memory. */
    fun delete(id: String) = handle(true).delete(id)

    /** One memory by id. */
    fun get(id: String): MemoryRecord = handle(false).get(id)

    /** Recall by vector. */
    fun recall(vector: List<Float>, options: RecallOptions = RecallOptions()): RecallPage =
        handle(false).recall(vector, options)

    /** The number of live memories. */
    fun count(): ULong = handle(false).len()

    /** Stores a value in the result cache. */
    fun cachePut(namespace: String, key: String, value: ByteArray, ttlSecs: ULong? = null) =
        handle(true).cachePut(namespace, key, value, ttlSecs)

    /** A cached value, or `null`. */
    fun cacheGet(namespace: String, key: String): ByteArray? = handle(false).cacheGet(namespace, key)
}
