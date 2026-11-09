import Foundation

/// Where a ``MnemoStore`` keeps its file, and how it creates it.
public struct MnemoStoreConfiguration: Sendable, Equatable {
    /// The file name without the `.mnemo` extension. Letters, digits, `-`,
    /// `_` and `.`; no path separators.
    public var name: String
    /// Embedding width. Every stored vector must match it.
    public var dimensions: UInt32
    /// The directory, or `nil` for `Application Support/mnemo/` (see
    /// ``defaultDirectory()`` and ``appGroupDirectory(_:)``).
    public var directory: URL?
    /// Where the key lives, and whether the file goes into backups.
    public var keyPolicy: KeyPolicy
    /// `0` creates the current format (v9); `8` creates a v8 file for readers
    /// on v0.10 or earlier. Used only when the file is created.
    public var formatVersion: UInt16
    /// The in-memory vector column budget (default 16 MiB).
    public var vectorCacheBudgetBytes: UInt64
    /// The page cache, in 8 KB pages (default 256 = 2 MiB).
    public var cachePages: UInt32

    public init(
        name: String,
        dimensions: UInt32,
        directory: URL? = nil,
        keyPolicy: KeyPolicy = .deviceOnly,
        formatVersion: UInt16 = 0,
        vectorCacheBudgetBytes: UInt64 = 16 << 20,
        cachePages: UInt32 = 256
    ) {
        self.name = name
        self.dimensions = dimensions
        self.directory = directory
        self.keyPolicy = keyPolicy
        self.formatVersion = formatVersion
        self.vectorCacheBudgetBytes = vectorCacheBudgetBytes
        self.cachePages = cachePages
    }

    /// `Application Support/mnemo/` in the app's container.
    public static func defaultDirectory() throws -> URL {
        try FileManager.default
            .url(for: .applicationSupportDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("mnemo", isDirectory: true)
    }

    /// `Library/Application Support/mnemo/` in an App Group container, so an
    /// app and its extensions can use the same file (one process at a time;
    /// see the README).
    public static func appGroupDirectory(_ identifier: String) throws -> URL {
        guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: identifier) else {
            throw MnemoStoreError.file(message: "no App Group container for \(identifier)")
        }
        return container.appendingPathComponent("Library/Application Support/mnemo", isDirectory: true)
    }

    /// The database file this configuration names.
    public func fileURL() throws -> URL {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_."))
        guard !name.isEmpty, !name.hasPrefix("."), name.unicodeScalars.allSatisfy(allowed.contains) else {
            throw MnemoStoreError.file(message: "invalid store name \"\(name)\"")
        }
        let dir = try directory ?? Self.defaultDirectory()
        return dir.appendingPathComponent(name, isDirectory: false).appendingPathExtension("mnemo")
    }

    var engineConfig: MnemoConfig {
        MnemoConfig(dimensions: dimensions, formatVersion: formatVersion,
                    vectorCacheBudgetBytes: vectorCacheBudgetBytes, cachePages: cachePages)
    }
}

/// An encrypted MNemo database for an app: async, keyed by the Keychain, and
/// safe across the app's background transitions.
///
/// - Opens lazily, on the first call. A new file gets a fresh random key,
///   stored in the key store under a new id, which is stamped on the file.
/// - On iOS, the store never lets the app be suspended holding the file
///   lock (`0xdead10cc`):
///   - when the app enters the background or is about to terminate, it
///     flushes and closes its file inside an expiring activity;
///   - while the app isn't active, every call runs inside its own expiring
///     activity: it reopens the file, does its work, then flushes and
///     closes it again before the activity ends. That covers work the app
///     does in the background (a background fetch, a `Task` still running)
///     and an app launched straight into the background;
///   - once the app is active again, the file stays open between calls.
///
///   A call never sees ``MnemoError/Closed`` because of a transition.
/// - Calls run on the actor, never on the main thread, and are serialized.
///   Each is a blocking call into the engine, so keep batches reasonable.
/// - The file lives in `Application Support/mnemo/` by default, with the
///   protection class `completeUntilFirstUserAuthentication`, and under
///   ``KeyPolicy/deviceOnly`` it's excluded from backups.
public actor MnemoStore {
    /// The configuration the store was made with.
    public nonisolated let configuration: MnemoStoreConfiguration
    /// The database file.
    public nonisolated let fileURL: URL

    private let keyStore: any MnemoKeyStore
    private var embedder: (any MnemoEmbedder)?
    private var db: MnemoDb?
    private var keyID: String?
    private nonisolated let lifecycle = LifecycleObserver()
    private nonisolated let activityReason: String

    /// How many times a background transition has closed the file (tests).
    private(set) var backgroundCloses = 0
    /// How many times a call made in the background has closed the file
    /// after itself (tests).
    private(set) var backgroundCallCloses = 0
    /// The last error a background flush or close hit, if any.
    public private(set) var lastBackgroundError: (any Error)?

    /// Makes a store. Nothing is opened until the first call.
    ///
    /// - Parameters:
    ///   - keyStore: where keys live; ``MnemoKeychain`` by default.
    ///   - embedder: turns text into vectors for ``rememberText(_:options:)``,
    ///     ``recallText(_:options:)``, ``update(id:patch:)`` and threads.
    public init(
        configuration: MnemoStoreConfiguration,
        keyStore: any MnemoKeyStore = MnemoKeychain(),
        embedder: (any MnemoEmbedder)? = nil
    ) throws {
        self.configuration = configuration
        self.fileURL = try configuration.fileURL()
        self.keyStore = keyStore
        self.embedder = embedder
        activityReason = "MNemo: \(configuration.name) in the background"
        lifecycle.start(reason: "MNemo: flush and close \(configuration.name)") { [weak self] in
            await self?.closeForBackground()
        }
    }

    // MARK: Writes

    /// Stores a memory with a caller-supplied vector.
    @discardableResult
    public func remember(_ content: String, vector: [Float],
                         options: RememberOptions = RememberOptions()) async throws -> WriteResult {
        try await withDatabase { try $0.remember(content: content, vector: vector, options: options) }
    }

    /// Stores a memory, embedding `content` with the store's embedder.
    @discardableResult
    public func rememberText(_ content: String,
                             options: RememberOptions = RememberOptions()) async throws -> WriteResult {
        try await withDatabase { try $0.rememberText(content: content, options: options) }
    }

    /// Changes fields of a memory. A content change is re-embedded when the
    /// patch has no vector and an embedder is set.
    @discardableResult
    public func update(id: String, patch: MemoryPatch) async throws -> WriteOutcome {
        try await withDatabase { try $0.update(id: id, patch: patch) }
    }

    /// Deletes a memory.
    public func delete(id: String) async throws {
        try await withDatabase { try $0.delete(id: id) }
    }

    // MARK: Reads

    /// One memory by id.
    public func get(id: String) async throws -> MemoryRecord {
        try await withDatabase { try $0.get(id: id) }
    }

    /// Every version of a keyed memory, newest first.
    public func history(key: String) async throws -> [MemoryRecord] {
        try await withDatabase { try $0.history(key: key) }
    }

    /// Recall by vector. An empty vector with `options.keywords` set is a
    /// keyword-only recall.
    public func recall(vector: [Float], options: RecallOptions = RecallOptions()) async throws -> RecallPage {
        try await withDatabase { try $0.recall(vector: vector, options: options) }
    }

    /// Recall by text, embedded with the store's embedder.
    public func recallText(_ query: String, options: RecallOptions = RecallOptions()) async throws -> RecallPage {
        try await withDatabase { try $0.recallText(query: query, options: options) }
    }

    /// The file's self-description (manifest and orientation memories).
    public func about() async throws -> [MemoryRecord] {
        try await withDatabase { try $0.about() }
    }

    /// The essence of the project or of one thread, within `maxBytes`.
    public func essence(_ scope: EssenceScope = .project, maxBytes: UInt64 = 4096) async throws -> String {
        try await withDatabase { try $0.essence(scope: scope, maxBytes: maxBytes) }
    }

    // MARK: Essence writes

    /// Adds an item to an essence section (at most 280 bytes, status
    /// `open`) and returns its id. With an embedder set, the text is
    /// embedded first.
    @discardableResult
    public func essenceAdd(_ section: EssenceSection, _ text: String, scope: EssenceScope = .project,
                           refs: [String]? = nil) async throws -> String {
        try await withDatabase { try $0.essenceAdd(scope: scope, section: section, text: text, refs: refs) }
    }

    /// Appends a timeline entry (at most 600 bytes; append-only) and returns
    /// its id.
    @discardableResult
    public func essenceAppendTimeline(_ text: String, scope: EssenceScope = .project,
                                      refs: [String]? = nil) async throws -> String {
        try await withDatabase { try $0.essenceAppendTimeline(scope: scope, text: text, refs: refs) }
    }

    /// Replaces an item's text and refs. Timeline entries can't be edited.
    @discardableResult
    public func essenceUpdate(id: String, text: String, refs: [String]? = nil) async throws -> WriteOutcome {
        try await withDatabase { try $0.essenceUpdate(id: id, text: text, refs: refs) }
    }

    /// Marks an item resolved; it stays in the history.
    @discardableResult
    public func essenceResolve(id: String) async throws -> WriteOutcome {
        try await withDatabase { try $0.essenceResolve(id: id) }
    }

    /// Removes an item from the essence (a soft delete).
    public func essenceRemove(id: String) async throws {
        try await withDatabase { try $0.essenceRemove(id: id) }
    }

    // MARK: Index

    /// Builds the IVF+PQ index. On a v9 file below 20,000 facts it builds
    /// nothing and reports `partitions == 0`: an exact scan is faster there.
    @discardableResult
    public func buildIndex() async throws -> IndexInfo {
        try await withDatabase { try $0.buildIndex() }
    }

    /// Whether the file has an IVF+PQ index.
    public func hasIndex() async throws -> Bool {
        try await withDatabase { try $0.hasIndex() }
    }

    /// File statistics.
    public func stats() async throws -> DbStats {
        try await withDatabase { try $0.stats() }
    }

    /// The number of live memories.
    public func count() async throws -> UInt64 {
        try await withDatabase { try $0.len() }
    }

    // MARK: Threads

    /// Appends a thread's turns; turns already stored are skipped. Needs an
    /// embedder.
    @discardableResult
    public func ingestThread(_ threadID: String, turns: [ThreadTurn]) async throws -> IngestReport {
        try await withDatabase { try $0.ingestThread(threadId: threadID, turns: turns) }
    }

    /// A thread's turns, oldest first.
    public func threadExport(_ threadID: String) async throws -> [ExportedTurn] {
        try await withDatabase { try $0.threadExport(threadId: threadID) }
    }

    /// Deletes a thread's turns; returns how many records went.
    @discardableResult
    public func threadForget(_ threadID: String) async throws -> UInt64 {
        try await withDatabase { try $0.threadForget(threadId: threadID) }
    }

    // MARK: Result cache

    /// Stores a value in the result cache under an exact key.
    public func cachePut(namespace: String, key: String, value: Data, ttlSecs: UInt64? = nil) async throws {
        try await withDatabase { try $0.cachePut(namespace: namespace, key: key, value: value, ttlSecs: ttlSecs) }
    }

    /// A cached value, or `nil`.
    public func cacheGet(namespace: String, key: String) async throws -> Data? {
        try await withDatabase { try $0.cacheGet(namespace: namespace, key: key) }
    }

    // MARK: Lifecycle

    /// Sets (or replaces) the embedder. The embedder's identity must match
    /// the one the file was written with, if any.
    public func setEmbedder(_ embedder: any MnemoEmbedder) throws {
        try db?.setEmbedder(embedder: embedder)
        self.embedder = embedder
    }

    /// Makes every write so far durable. A no-op while the file is closed.
    public func flush() throws {
        try db?.flush()
    }

    /// Flushes and closes the file, releasing its lock. The next call reopens
    /// it.
    public func close() throws {
        guard let db else { return }
        self.db = nil
        try db.flush()
        try db.close()
    }

    /// Rewrites the file compactly, closing it first. The next call reopens it.
    @discardableResult
    public func compact() async throws -> CompactReport {
        let activity = lifecycle.isForeground ? nil : await LifecycleObserver.beginActivity(reason: activityReason)
        defer { activity?.end() }
        try close()
        guard FileManager.default.fileExists(atPath: fileURL.path) else {
            throw MnemoError.NotFound(message: fileURL.path)
        }
        let (id, key) = try resolveKey()
        var bytes = key
        defer { bytes.zeroize() }
        let report = try compactFileWithKey(path: fileURL.path, key: bytes)
        try KeyID.write(id, to: fileURL)
        try FilePolicy.apply(to: fileURL, policy: configuration.keyPolicy)
        return report
    }

    // MARK: Moving devices

    /// Writes a copy that opens with `passphrase` alone, for moving the
    /// database to another device or app. `destination` must not exist.
    /// Takes a few seconds: the passphrase goes through the KDF.
    public func exportWithPassphrase(to destination: URL, passphrase: String) async throws {
        try await withDatabase { try $0.exportWithPassphrase(dest: destination.path, passphrase: passphrase) }
    }

    /// Imports a passphrase export as a new store: the copy is re-protected
    /// by a fresh key, stored in `keyStore` under the configuration's
    /// policy. The store's file must not exist yet; the source is unchanged.
    public static func importWithPassphrase(
        from source: URL,
        passphrase: String,
        configuration: MnemoStoreConfiguration,
        keyStore: any MnemoKeyStore = MnemoKeychain(),
        embedder: (any MnemoEmbedder)? = nil
    ) async throws -> MnemoStore {
        let store = try MnemoStore(configuration: configuration, keyStore: keyStore, embedder: embedder)
        let dest = store.fileURL
        try FilePolicy.prepareDirectory(dest.deletingLastPathComponent())
        var key = try Mnemo.importWithPassphrase(src: source.path, passphrase: passphrase, dest: dest.path)
        defer { key.zeroize() }
        let id = UUID().uuidString
        do {
            try keyStore.storeKey(key, id: id, policy: configuration.keyPolicy)
            try KeyID.write(id, to: dest)
            try FilePolicy.apply(to: dest, policy: configuration.keyPolicy)
        } catch {
            try? keyStore.deleteKey(id: id)
            try? FileManager.default.removeItem(at: dest)
            throw error
        }
        return store
    }

    // MARK: Internals

    /// Whether the file is open (tests).
    var isOpen: Bool { db != nil }

    /// Whether the store treats the app as active (tests).
    nonisolated var appIsActive: Bool { lifecycle.isForeground }

    /// The background transition: flush, close, release the lock. Errors are
    /// kept in ``lastBackgroundError``; the next call reopens either way.
    func closeForBackground() {
        guard let db else { return }
        self.db = nil
        do {
            try db.flush()
        } catch {
            lastBackgroundError = error
        }
        do {
            try db.close()
        } catch {
            lastBackgroundError = error
        }
        backgroundCloses += 1
    }

    /// Runs `body` on the open handle, opening it first if needed.
    ///
    /// While the app isn't active, the call holds an expiring activity, and
    /// the file is flushed and closed again before the activity ends, so the
    /// app is never suspended holding the lock. Calls while it's active
    /// leave the file open.
    private func withDatabase<T>(_ body: (MnemoDb) throws -> T) async throws -> T {
        if lifecycle.isForeground { return try onHandle(body) }
        let activity = await LifecycleObserver.beginActivity(reason: activityReason)
        // Deferred blocks run last-first: close, then end the activity.
        defer { activity.end() }
        defer { closeAfterBackgroundCall() }
        return try onHandle(body)
    }

    /// Actor isolation means nothing closes the handle while `body` runs; if
    /// the engine reports `Closed` anyway, the store reopens once and
    /// retries (`Closed` is reported before any work is done, so the retry
    /// is safe).
    private func onHandle<T>(_ body: (MnemoDb) throws -> T) throws -> T {
        do {
            return try body(try handle())
        } catch MnemoError.Closed {
            db = nil
            return try body(try handle())
        }
    }

    /// After a call made in the background: flush and close, unless the app
    /// became active meanwhile. Errors go to ``lastBackgroundError``.
    private func closeAfterBackgroundCall() {
        guard !lifecycle.isForeground, let db else { return }
        self.db = nil
        do {
            try db.flush()
        } catch {
            lastBackgroundError = error
        }
        do {
            try db.close()
        } catch {
            lastBackgroundError = error
        }
        backgroundCallCloses += 1
    }

    private func handle() throws -> MnemoDb {
        if let db { return db }
        let opened = try open()
        do {
            if let embedder { try opened.setEmbedder(embedder: embedder) }
        } catch {
            try? opened.close()
            throw error
        }
        db = opened
        return opened
    }

    private func open() throws -> MnemoDb {
        try FilePolicy.prepareDirectory(fileURL.deletingLastPathComponent())
        let path = fileURL.path
        let opened: MnemoDb
        let id: String
        if FileManager.default.fileExists(atPath: path) {
            (id, opened) = try openExisting()
        } else {
            (id, opened) = try create()
        }
        do {
            if KeyID.read(fileURL) != id { try KeyID.write(id, to: fileURL) }
            try FilePolicy.apply(to: fileURL, policy: configuration.keyPolicy)
            if configuration.vectorCacheBudgetBytes != 16 << 20 {
                try opened.setVectorCacheBudget(bytes: configuration.vectorCacheBudgetBytes)
            }
            if configuration.cachePages != 256 {
                try opened.setCacheCapacity(pages: configuration.cachePages)
            }
        } catch {
            try? opened.close()
            throw error
        }
        keyID = id
        return opened
    }

    private func create() throws -> (String, MnemoDb) {
        let id = UUID().uuidString
        var key = generateKey()
        defer { key.zeroize() }
        try keyStore.storeKey(key, id: id, policy: configuration.keyPolicy)
        do {
            return (id, try MnemoDb.createWithKey(path: fileURL.path, key: key, config: configuration.engineConfig))
        } catch {
            try? keyStore.deleteKey(id: id)
            throw error
        }
    }

    private func openExisting() throws -> (String, MnemoDb) {
        let (id, key) = try resolveKey()
        var bytes = key
        defer { bytes.zeroize() }
        return (id, try MnemoDb.openWithKey(path: fileURL.path, key: bytes))
    }

    /// The key id and key for the existing file: the stamped id, or else the
    /// first stored key that opens the file (which is then stamped).
    private func resolveKey() throws -> (String, Data) {
        if let id = KeyID.read(fileURL) ?? keyID {
            do {
                return (id, try keyStore.key(id: id))
            } catch MnemoStoreError.keyMissing {
                throw MnemoStoreError.keyMissing(keyID: id, path: fileURL.path)
            }
        }
        for id in try keyStore.keyIDs() {
            var key: Data
            do {
                key = try keyStore.key(id: id)
            } catch MnemoStoreError.keyMissing {
                continue
            }
            do {
                // Opening with a raw key costs no KDF, so a wrong key is cheap.
                let probe = try MnemoDb.openReadOnlyWithKey(path: fileURL.path, key: key)
                try probe.close()
            } catch MnemoError.WrongPassphrase {
                key.zeroize()
                continue
            }
            try? KeyID.write(id, to: fileURL)
            return (id, key)
        }
        throw MnemoStoreError.keyMissing(keyID: nil, path: fileURL.path)
    }
}
