package io.github.toarchkumar.mnemo

import android.content.Context
import java.io.File
import java.util.UUID
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * An encrypted MNemo database for an app: suspend functions on
 * [Dispatchers.IO], a key from the Android Keystore, and a flush whenever
 * the app goes to the background.
 *
 * - **Opens lazily**, on the first call. A new file gets a fresh random key,
 *   stored in the key storage under a new id, which is stamped on the file.
 * - **Calls run on the dispatcher**, never on the caller's thread, one at a
 *   time. Each is a blocking call into the engine, so keep batches
 *   reasonable.
 * - **Flushes on `ON_STOP`.** When the app's last activity stops
 *   (`ProcessLifecycleOwner`), the store flushes, so every write so far is
 *   durable. The file stays open: Android has no equivalent of iOS's
 *   lock-on-suspend kill.
 * - **Reopens transparently.** After [close], or if the engine reports the
 *   handle closed, the next call reopens the file. No call fails with
 *   [MnemoException.Closed].
 * - **Files** live in `noBackupFilesDir/mnemo/<name>.mnemo` by default,
 *   device-only like their keys.
 *
 * Make one store per file and keep it for the app's lifetime.
 */
class MnemoStore private constructor(
    /** The configuration the store was made with. */
    val configuration: MnemoStoreConfiguration,
    /** The database file. */
    val file: File,
    private val keys: MnemoKeyStorage,
    @Volatile private var embedder: MnemoEmbedder?,
    private val dispatcher: CoroutineDispatcher,
    observeProcess: Boolean,
) {
    /**
     * A store for an Android app: the file in `noBackupFilesDir/mnemo/`
     * (unless the configuration names a directory), keys in [MnemoKeystore],
     * and a flush on every `ProcessLifecycleOwner` `ON_STOP`.
     *
     * @param embedder turns text into vectors for [rememberText],
     *   [recallText], [update] and threads.
     */
    constructor(
        context: Context,
        configuration: MnemoStoreConfiguration,
        keys: MnemoKeyStorage = MnemoKeystore(context),
        embedder: MnemoEmbedder? = null,
        dispatcher: CoroutineDispatcher = Dispatchers.IO,
    ) : this(configuration, configuration.file(context.applicationContext), keys, embedder, dispatcher, true)

    /**
     * A store without Android: the configuration must name a directory,
     * and nothing flushes on its own (call [flush] or [close]). For JVM
     * tests and tools.
     */
    constructor(
        configuration: MnemoStoreConfiguration,
        keys: MnemoKeyStorage,
        embedder: MnemoEmbedder? = null,
        dispatcher: CoroutineDispatcher = Dispatchers.IO,
    ) : this(configuration, configuration.file(null), keys, embedder, dispatcher, false)

    private val mutex = Mutex()
    private val scope = CoroutineScope(SupervisorJob() + dispatcher)

    @Volatile private var db: MnemoDb? = null
    @Volatile private var keyId: String? = null

    /** How many `ON_STOP` flushes have run on an open file (tests). */
    @Volatile internal var backgroundFlushes = 0
        private set

    /** The last error an `ON_STOP` flush hit, if any. */
    @Volatile var lastBackgroundError: Throwable? = null
        private set

    init {
        if (observeProcess) ProcessStopObserver.attach(this)
    }

    // --- writes ---------------------------------------------------------

    /** Stores a memory with a caller-supplied vector. */
    suspend fun remember(content: String, vector: List<Float>, options: RememberOptions = RememberOptions()): WriteResult =
        withDatabase { it.remember(content, vector, options) }

    /** Stores a memory, embedding [content] with the store's embedder. */
    suspend fun rememberText(content: String, options: RememberOptions = RememberOptions()): WriteResult =
        withDatabase { it.rememberText(content, options) }

    /**
     * Changes fields of a memory. A content change is re-embedded when the
     * patch has no vector and an embedder is set.
     */
    suspend fun update(id: String, patch: MemoryPatch): WriteOutcome =
        withDatabase { it.update(id, patch) }

    /** Deletes a memory. */
    suspend fun delete(id: String) = withDatabase { it.delete(id) }

    // --- reads ----------------------------------------------------------

    /** One memory by id. */
    suspend fun get(id: String): MemoryRecord = withDatabase { it.get(id) }

    /** Every version of a keyed memory, newest first. */
    suspend fun history(key: String): List<MemoryRecord> = withDatabase { it.history(key) }

    /**
     * Recall by vector. An empty vector with [RecallOptions.keywords] set is
     * a keyword-only recall.
     */
    suspend fun recall(vector: List<Float>, options: RecallOptions = RecallOptions()): RecallPage =
        withDatabase { it.recall(vector, options) }

    /** Recall by text, embedded with the store's embedder. */
    suspend fun recallText(query: String, options: RecallOptions = RecallOptions()): RecallPage =
        withDatabase { it.recallText(query, options) }

    /** The file's self-description (manifest and orientation memories). */
    suspend fun about(): List<MemoryRecord> = withDatabase { it.about() }

    /** The essence of the project or of one thread, within [maxBytes]. */
    suspend fun essence(scope: EssenceScope = EssenceScope.Project, maxBytes: ULong = 4096uL): String =
        withDatabase { it.essence(scope, maxBytes) }

    /** File statistics. */
    suspend fun stats(): DbStats = withDatabase { it.stats() }

    /** The number of live memories. */
    suspend fun count(): ULong = withDatabase { it.len() }

    // --- essence writes -------------------------------------------------

    /**
     * Adds an item to an essence [section] (at most 280 bytes, status `open`)
     * and returns its id. With an embedder set, the text is embedded first.
     */
    suspend fun essenceAdd(
        section: EssenceSection,
        text: String,
        scope: EssenceScope = EssenceScope.Project,
        refs: List<String>? = null,
    ): String = withDatabase { it.essenceAdd(scope, section, text, refs) }

    /** Appends a timeline entry (at most 600 bytes; append-only) and returns its id. */
    suspend fun essenceAppendTimeline(
        text: String,
        scope: EssenceScope = EssenceScope.Project,
        refs: List<String>? = null,
    ): String = withDatabase { it.essenceAppendTimeline(scope, text, refs) }

    /** Replaces an item's text and refs. Timeline entries can't be edited. */
    suspend fun essenceUpdate(id: String, text: String, refs: List<String>? = null): WriteOutcome =
        withDatabase { it.essenceUpdate(id, text, refs) }

    /** Marks an item resolved; it stays in the history. */
    suspend fun essenceResolve(id: String): WriteOutcome = withDatabase { it.essenceResolve(id) }

    /** Removes an item from the essence (a soft delete). */
    suspend fun essenceRemove(id: String) = withDatabase { it.essenceRemove(id) }

    // --- index ----------------------------------------------------------

    /**
     * Builds the IVF+PQ index. On a v9 file below 20,000 facts it builds
     * nothing and reports `partitions == 0u`: an exact scan is faster there.
     */
    suspend fun buildIndex(): IndexInfo = withDatabase { it.buildIndex() }

    /** Whether the file has an IVF+PQ index. */
    suspend fun hasIndex(): Boolean = withDatabase { it.hasIndex() }

    // --- threads --------------------------------------------------------

    /** Appends a thread's turns; turns already stored are skipped. Needs an embedder. */
    suspend fun ingestThread(threadId: String, turns: List<ThreadTurn>): IngestReport =
        withDatabase { it.ingestThread(threadId, turns) }

    /** A thread's turns, oldest first. */
    suspend fun threadExport(threadId: String): List<ExportedTurn> =
        withDatabase { it.threadExport(threadId) }

    /** Deletes a thread's turns; returns how many records went. */
    suspend fun threadForget(threadId: String): ULong = withDatabase { it.threadForget(threadId) }

    // --- result cache ---------------------------------------------------

    /** Stores a value in the result cache under an exact key. */
    suspend fun cachePut(namespace: String, key: String, value: ByteArray, ttlSecs: ULong? = null) =
        withDatabase { it.cachePut(namespace, key, value, ttlSecs) }

    /** A cached value, or `null`. */
    suspend fun cacheGet(namespace: String, key: String): ByteArray? =
        withDatabase { it.cacheGet(namespace, key) }

    // --- lifecycle ------------------------------------------------------

    /**
     * Sets (or replaces) the embedder. Its identity must match the one the
     * file was written with, if any.
     */
    suspend fun setEmbedder(embedder: MnemoEmbedder) = locked {
        db?.setEmbedder(embedder)
        this.embedder = embedder
    }

    /** Makes every write so far durable. A no-op while the file is closed. */
    suspend fun flush() = locked { db?.flush() ?: Unit }

    /** Flushes and closes the file, releasing its lock. The next call reopens it. */
    suspend fun close() = locked { closeLocked() }

    /** Rewrites the file compactly, closing it first. The next call reopens it. */
    suspend fun compact(): CompactReport = locked {
        closeLocked()
        if (!file.exists()) throw MnemoException.NotFound(file.path)
        val (id, key) = resolveKey()
        try {
            val report = compactFileWithKey(file.path, key)
            KeyId.write(id, file)
            report
        } finally {
            key.zeroize()
        }
    }

    // --- moving devices -------------------------------------------------

    /**
     * Writes a copy that opens with [passphrase] alone, for moving the
     * database to another device or app. [destination] must not exist.
     * Takes a few seconds: the passphrase goes through the KDF.
     */
    suspend fun exportWithPassphrase(destination: File, passphrase: String) =
        withDatabase { it.exportWithPassphrase(destination.path, passphrase) }

    companion object {
        /**
         * Imports a passphrase export as a new Android store: the copy is
         * re-protected by a fresh key in [keys]. The store's file must not
         * exist yet; [source] is unchanged.
         */
        suspend fun importWithPassphrase(
            context: Context,
            source: File,
            passphrase: String,
            configuration: MnemoStoreConfiguration,
            keys: MnemoKeyStorage = MnemoKeystore(context),
            embedder: MnemoEmbedder? = null,
            dispatcher: CoroutineDispatcher = Dispatchers.IO,
        ): MnemoStore = MnemoStore(context, configuration, keys, embedder, dispatcher)
            .also { it.importFrom(source, passphrase) }

        /** [importWithPassphrase] without Android (see the plain constructor). */
        suspend fun importWithPassphrase(
            source: File,
            passphrase: String,
            configuration: MnemoStoreConfiguration,
            keys: MnemoKeyStorage,
            embedder: MnemoEmbedder? = null,
            dispatcher: CoroutineDispatcher = Dispatchers.IO,
        ): MnemoStore = MnemoStore(configuration, keys, embedder, dispatcher)
            .also { it.importFrom(source, passphrase) }
    }

    // --- internals ------------------------------------------------------

    /** Whether the file is open (tests). */
    internal val isOpen: Boolean get() = db != null

    /** `ON_STOP`: flush in the background, keep the handle open. */
    internal fun onProcessStop() {
        scope.launch { flushForBackground() }
    }

    /** The `ON_STOP` flush. Errors are kept in [lastBackgroundError]. */
    internal suspend fun flushForBackground() = locked {
        val open = db ?: return@locked
        try {
            open.flush()
        } catch (e: Exception) {
            lastBackgroundError = e
        }
        backgroundFlushes += 1
    }

    /** Closes the engine handle behind the store's back (tests of the reopen path). */
    internal suspend fun closeHandleUnderneath() = locked { db?.closeFile() ?: Unit }

    private suspend fun importFrom(source: File, passphrase: String) = locked {
        ensureDirectory(file.parentFile)
        val key = io.github.toarchkumar.mnemo.importWithPassphrase(source.path, passphrase, file.path)
        val id = UUID.randomUUID().toString()
        try {
            keys.storeKey(id, key)
            KeyId.write(id, file)
            keyId = id
        } catch (e: Exception) {
            runCatching { keys.deleteKey(id) }
            file.delete()
            throw e
        } finally {
            key.zeroize()
        }
    }

    /** Runs [body] on the dispatcher, holding the store's lock. */
    private suspend fun <T> locked(body: () -> T): T =
        withContext(dispatcher) { mutex.withLock { body() } }

    /**
     * Runs [body] on the open handle, opening it first if needed. Nothing
     * closes the handle while [body] runs; if the engine reports `Closed`
     * anyway, the store reopens once and retries (`Closed` is reported
     * before any work is done, so the retry is safe).
     */
    private suspend fun <T> withDatabase(body: (MnemoDb) -> T): T = locked {
        try {
            body(handle())
        } catch (e: MnemoException.Closed) {
            dropHandle()
            body(handle())
        }
    }

    private fun closeLocked() {
        val open = db ?: return
        db = null
        try {
            open.flush()
            open.closeFile()
        } finally {
            open.close() // frees the Kotlin handle
        }
    }

    private fun dropHandle() {
        val open = db ?: return
        db = null
        runCatching { open.closeFile() }
        open.close()
    }

    private fun handle(): MnemoDb {
        db?.let { return it }
        val opened = open()
        try {
            embedder?.let { opened.setEmbedder(it) }
        } catch (e: Exception) {
            runCatching { opened.closeFile() }
            opened.close()
            throw e
        }
        db = opened
        return opened
    }

    private fun open(): MnemoDb {
        ensureDirectory(file.parentFile)
        val (id, opened) = if (file.exists()) openExisting() else create()
        try {
            if (KeyId.read(file) != id) KeyId.write(id, file)
            if (configuration.vectorCacheBudgetBytes != MnemoStoreConfiguration.DEFAULT_VECTOR_CACHE_BUDGET) {
                opened.setVectorCacheBudget(configuration.vectorCacheBudgetBytes)
            }
            if (configuration.cachePages != MnemoStoreConfiguration.DEFAULT_CACHE_PAGES) {
                opened.setCacheCapacity(configuration.cachePages)
            }
        } catch (e: Exception) {
            runCatching { opened.closeFile() }
            opened.close()
            throw e
        }
        keyId = id
        return opened
    }

    private fun create(): Pair<String, MnemoDb> {
        val id = UUID.randomUUID().toString()
        val key = generateKey()
        try {
            keys.storeKey(id, key)
            return try {
                id to MnemoDb.createWithKey(file.path, key, configuration.engineConfig)
            } catch (e: Exception) {
                runCatching { keys.deleteKey(id) }
                throw e
            }
        } finally {
            key.zeroize()
        }
    }

    private fun openExisting(): Pair<String, MnemoDb> {
        val (id, key) = resolveKey()
        try {
            return id to MnemoDb.openWithKey(file.path, key)
        } finally {
            key.zeroize()
        }
    }

    /**
     * The key id and key for the existing file: the stamped id, or else the
     * first stored key that opens the file (which is then stamped).
     */
    private fun resolveKey(): Pair<String, ByteArray> {
        val stamped = KeyId.read(file) ?: keyId
        if (stamped != null) {
            return try {
                stamped to keys.key(stamped)
            } catch (e: MnemoStoreException.KeyMissing) {
                throw MnemoStoreException.KeyMissing(stamped, file.path, e.cause)
            }
        }
        for (id in keys.keyIds()) {
            val key = try {
                keys.key(id)
            } catch (e: MnemoStoreException.KeyMissing) {
                continue
            }
            try {
                // Opening with a raw key costs no KDF, so a wrong key is cheap.
                val probe = MnemoDb.openReadOnlyWithKey(file.path, key)
                try {
                    probe.closeFile()
                } finally {
                    probe.close()
                }
            } catch (e: MnemoException.WrongPassphrase) {
                key.zeroize()
                continue
            } catch (e: Exception) {
                key.zeroize()
                throw e
            }
            KeyId.write(id, file)
            return id to key
        }
        throw MnemoStoreException.KeyMissing(null, file.path)
    }

    private fun ensureDirectory(dir: File?) {
        if (dir == null || dir.isDirectory) return
        if (!dir.mkdirs() && !dir.isDirectory) throw MnemoStoreException.File("can't create $dir")
    }
}
