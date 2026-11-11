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
    /// Where the key lives, and whether the file goes into backups. Fixed
    /// when the store is created: opening an existing store with another
    /// policy is refused (``MnemoStoreError/keyPolicyMismatch(stored:configured:)``).
    public var keyPolicy: KeyPolicy
    /// `0` creates the current format (v9); `8` creates a v8 file for readers
    /// on v0.10 or earlier. Used only when the file is created.
    @available(*, deprecated, message: "No mobile reader needs a v8 file. v0.13 makes v8 files read-only and removes this setting; leave it at 0 (v9).")
    public var formatVersion: UInt16 {
        get { formatVersionValue }
        set { formatVersionValue = newValue }
    }
    var formatVersionValue: UInt16
    /// The in-memory vector column budget (default 16 MiB).
    public var vectorCacheBudgetBytes: UInt64
    /// The page cache, in 8 KB pages (default 256 = 2 MiB).
    public var cachePages: UInt32
    /// When writes are committed to disk on their own (default
    /// ``FlushPolicy/coalesced(maxDelay:maxWrites:)``: within about a second).
    public var flushPolicy: FlushPolicy

    public init(
        name: String,
        dimensions: UInt32,
        directory: URL? = nil,
        keyPolicy: KeyPolicy = .deviceOnly,
        vectorCacheBudgetBytes: UInt64 = 16 << 20,
        cachePages: UInt32 = 256,
        flushPolicy: FlushPolicy = .default
    ) {
        self.name = name
        self.dimensions = dimensions
        self.directory = directory
        self.keyPolicy = keyPolicy
        self.formatVersionValue = 0
        self.vectorCacheBudgetBytes = vectorCacheBudgetBytes
        self.cachePages = cachePages
        self.flushPolicy = flushPolicy
    }

    /// The same, choosing the format of a new file.
    @available(*, deprecated, message: "No mobile reader needs a v8 file. v0.13 makes v8 files read-only and removes formatVersion; use the initializer without it.")
    public init(
        name: String,
        dimensions: UInt32,
        directory: URL? = nil,
        keyPolicy: KeyPolicy = .deviceOnly,
        formatVersion: UInt16,
        vectorCacheBudgetBytes: UInt64 = 16 << 20,
        cachePages: UInt32 = 256,
        flushPolicy: FlushPolicy = .default
    ) {
        self.init(name: name, dimensions: dimensions, directory: directory, keyPolicy: keyPolicy,
                  vectorCacheBudgetBytes: vectorCacheBudgetBytes, cachePages: cachePages, flushPolicy: flushPolicy)
        self.formatVersionValue = formatVersion
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
        MnemoConfig(dimensions: dimensions, formatVersion: formatVersionValue,
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
/// - **Durability:** a write is in memory until the store commits it. Under
///   the default ``FlushPolicy/coalesced(maxDelay:maxWrites:)`` that's within
///   about a second (or after 64 writes); ``FlushPolicy/everyWrite`` commits
///   before each write returns. The background close above always commits.
///   A kill (a crash, the system ending the app, a force quit) loses only
///   writes that weren't committed yet. ``withSession(_:)`` batches many
///   calls into one commit.
/// - Calls run on the actor, never on the main thread, and are serialized.
///   Each is a blocking call into the engine, so keep batches reasonable.
/// - The embedder runs before a call reaches the actor, off the main thread:
///   other calls to the store go ahead while it runs, a background close
///   isn't held up by it, and it may call back into the same store.
/// - The file lives in `Application Support/mnemo/` by default, with the
///   protection class `completeUntilFirstUserAuthentication`, and under
///   ``KeyPolicy/deviceOnly`` it's excluded from backups.
public actor MnemoStore {
    /// The configuration the store was made with.
    public nonisolated let configuration: MnemoStoreConfiguration
    /// The database file.
    public nonisolated let fileURL: URL

    private let keyStore: any MnemoKeyStore
    /// Read outside the actor: the text calls embed before they reach it.
    private nonisolated let embedderBox: EmbedderBox
    private var db: MnemoDb?
    private var keyID: String?
    private nonisolated let lifecycle = LifecycleObserver()
    private nonisolated let activityReason: String

    /// How many times a background transition has closed the file (tests).
    private(set) var backgroundCloses = 0
    /// How many times a call made in the background has closed the file
    /// after itself (tests).
    private(set) var backgroundCallCloses = 0
    /// How many times the store committed (flushed an open file): its
    /// policy, ``flush()``, ``close()`` and the background closes (tests).
    private(set) var commits = 0
    /// Writes made since the last commit, under the coalesced policy.
    private var pendingWrites = 0
    /// The coalesced policy's timer, armed by the first write after a commit.
    private var commitTimer: Task<Void, Never>?
    private var timerGeneration = 0
    /// Set by ``setMaxSnapshots(_:)``; applied on every open.
    private var maxSnapshots: UInt32?
    /// The last error a flush or close the store ran on its own hit (a
    /// background close, or a coalesced commit), if any.
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
        self.embedderBox = EmbedderBox(embedder)
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
        try await withDatabase(write: true) { try $0.remember(content: content, vector: vector, options: options) }
    }

    /// Stores a memory, embedding `content` with the store's embedder first.
    @discardableResult
    public nonisolated func rememberText(_ content: String,
                                         options: RememberOptions = RememberOptions()) async throws -> WriteResult {
        let vector = try embed(content)
        return try await remember(content, vector: vector, options: options)
    }

    /// Changes fields of a memory. A content change is re-embedded first
    /// when the patch has no vector and an embedder is set.
    @discardableResult
    public nonisolated func update(id: String, patch: MemoryPatch) async throws -> WriteOutcome {
        var patch = patch
        if let content = patch.content, patch.vector == nil, embedderBox.value != nil {
            patch.vector = try embed(content)
        }
        return try await updateOnHandle(id: id, patch: patch)
    }

    private func updateOnHandle(id: String, patch: MemoryPatch) async throws -> WriteOutcome {
        try await withDatabase(write: true) { try $0.update(id: id, patch: patch) }
    }

    /// Deletes a memory.
    public func delete(id: String) async throws {
        try await withDatabase(write: true) { try $0.delete(id: id) }
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

    /// Recall by text, embedded with the store's embedder first.
    public nonisolated func recallText(_ query: String, options: RecallOptions = RecallOptions()) async throws -> RecallPage {
        let vector = try embed(query)
        return try await recall(vector: vector, options: options)
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
    public nonisolated func essenceAdd(_ section: EssenceSection, _ text: String, scope: EssenceScope = .project,
                                       refs: [String]? = nil) async throws -> String {
        let vector = try embedIfSet(text)
        return try await essenceAddOnHandle(section, text, scope: scope, refs: refs, vector: vector)
    }

    private func essenceAddOnHandle(_ section: EssenceSection, _ text: String, scope: EssenceScope,
                                    refs: [String]?, vector: [Float]?) async throws -> String {
        try await withDatabase(write: true) {
            try $0.essenceAddWithVector(scope: scope, section: section, text: text, refs: refs, vector: vector)
        }
    }

    /// Appends a timeline entry (at most 600 bytes; append-only) and returns
    /// its id. Embeds like ``essenceAdd(_:_:scope:refs:)``.
    @discardableResult
    public nonisolated func essenceAppendTimeline(_ text: String, scope: EssenceScope = .project,
                                                  refs: [String]? = nil) async throws -> String {
        let vector = try embedIfSet(text)
        return try await essenceAppendOnHandle(text, scope: scope, refs: refs, vector: vector)
    }

    private func essenceAppendOnHandle(_ text: String, scope: EssenceScope, refs: [String]?,
                                       vector: [Float]?) async throws -> String {
        try await withDatabase(write: true) {
            try $0.essenceAppendTimelineWithVector(scope: scope, text: text, refs: refs, vector: vector)
        }
    }

    /// Replaces an item's text and refs. Timeline entries can't be edited.
    @discardableResult
    public func essenceUpdate(id: String, text: String, refs: [String]? = nil) async throws -> WriteOutcome {
        try await withDatabase(write: true) { try $0.essenceUpdate(id: id, text: text, refs: refs) }
    }

    /// Marks an item resolved; it stays in the history.
    @discardableResult
    public func essenceResolve(id: String) async throws -> WriteOutcome {
        try await withDatabase(write: true) { try $0.essenceResolve(id: id) }
    }

    /// Removes an item from the essence (a soft delete).
    public func essenceRemove(id: String) async throws {
        try await withDatabase(write: true) { try $0.essenceRemove(id: id) }
    }

    // MARK: Index

    /// Builds the IVF+PQ index. On a v9 file below 20,000 facts it builds
    /// nothing and reports `partitions == 0`: an exact scan is faster there.
    @discardableResult
    public func buildIndex() async throws -> IndexInfo {
        try await withDatabase(write: true) { try $0.buildIndex() }
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
    /// embedder. The store asks the file which texts are new, embeds them,
    /// then applies them, and plans again if the thread changed meanwhile.
    @discardableResult
    public nonisolated func ingestThread(_ threadID: String, turns: [ThreadTurn]) async throws -> IngestReport {
        for _ in 0..<3 {
            let texts = try await ingestPlan(threadID, turns: turns)
            let vectors = texts.isEmpty ? [] : try embed(texts)
            if let report = try await ingestApply(threadID, turns: turns, texts: texts, vectors: vectors) {
                return report
            }
        }
        throw MnemoError.Invalid(message: "the thread kept changing during ingest; try again")
    }

    private func ingestPlan(_ threadID: String, turns: [ThreadTurn]) async throws -> [String] {
        try await withDatabase { try $0.ingestThreadPlan(threadId: threadID, turns: turns) }
    }

    private func ingestApply(_ threadID: String, turns: [ThreadTurn], texts: [String],
                             vectors: [[Float]]) async throws -> IngestReport? {
        try await withDatabase(write: true) {
            try $0.ingestThreadWithVectors(threadId: threadID, turns: turns, texts: texts, vectors: vectors)
        }
    }

    /// A thread's turns, oldest first.
    public func threadExport(_ threadID: String) async throws -> [ExportedTurn] {
        try await withDatabase { try $0.threadExport(threadId: threadID) }
    }

    /// Deletes a thread's turns; returns how many records went.
    @discardableResult
    public func threadForget(_ threadID: String) async throws -> UInt64 {
        try await withDatabase(write: true) { try $0.threadForget(threadId: threadID) }
    }

    // MARK: Result cache

    /// Stores a value in the result cache under an exact key.
    public func cachePut(namespace: String, key: String, value: Data, ttlSecs: UInt64? = nil) async throws {
        try await withDatabase(write: true) { try $0.cachePut(namespace: namespace, key: key, value: value, ttlSecs: ttlSecs) }
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
        embedderBox.value = embedder
    }

    /// Makes every write so far durable. A no-op while the file is closed.
    public func flush() throws {
        try commit()
    }

    /// Flushes and closes the file, releasing its lock. The next call reopens
    /// it.
    ///
    /// If the final flush fails (a full disk, say), the file stays open with
    /// every unflushed write and the error is thrown: free space and call
    /// ``flush()`` or `close()` again (v0.12.2).
    public func close() throws {
        guard let db else { return }
        cancelPendingCommit()
        // The binding's close flushes first and keeps its handle on failure.
        try db.close()
        self.db = nil
        commits += 1
    }

    /// How many past snapshots (restore points) the file keeps, replacing the
    /// default (the newest 16 plus any from the last 24 hours). Applies now
    /// and every time the store reopens the file (v0.12.2).
    public func setMaxSnapshots(_ count: UInt32) async throws {
        maxSnapshots = count
        if let db { try db.setMaxSnapshots(count: count) }
    }

    /// Commits and closes the file when the store goes away, so a new store
    /// on the same file sees every write and finds it unlocked (RB-002.4 T2).
    deinit {
        commitTimer?.cancel()
        if let db { try? db.close() }
    }

    /// Runs `body` on one open handle with no other call in between, then
    /// commits once (unless the policy is ``FlushPolicy/manual``). Use it for
    /// a batch of writes: 100 writes in a session are one commit. A session
    /// doesn't embed; compute vectors first. If `body` throws, the writes it
    /// made stay, and are committed the same way.
    public func withSession<T: Sendable>(_ body: @Sendable (MnemoSession) throws -> T) async throws -> T {
        try await withDatabase(write: false) { db in
            let session = MnemoSession(db: db)
            defer { session.end() }
            do {
                let value = try body(session)
                try finishSession(writes: session.writes)
                return value
            } catch {
                try? finishSession(writes: session.writes)
                throw error
            }
        }
    }

    /// Rewrites the file compactly, closing it first. The next call reopens it.
    @discardableResult
    public func compact() async throws -> CompactReport {
        let activity = lifecycle.isForeground ? nil : try await LifecycleObserver.beginActivity(reason: activityReason)
        defer { activity?.end() }
        try close()
        guard FileManager.default.fileExists(atPath: fileURL.path) else {
            throw MnemoError.NotFound(message: fileURL.path)
        }
        let (id, key) = try resolveKey()
        let report = try compactFileWithKey(path: fileURL.path, key: key.data)
        try KeyID.write(id, to: fileURL)
        try FilePolicy.apply(to: fileURL, policy: configuration.keyPolicy)
        return report
    }

    // MARK: Moving devices

    /// Writes a copy that opens with `passphrase` alone, for moving the
    /// database to another device or app: a compacted copy under a fresh
    /// data key, sharing no key material with this file. `destination` must
    /// not exist. Takes a few seconds: the passphrase goes through the KDF.
    public func exportWithPassphrase(to destination: URL, passphrase: String) async throws {
        try await withDatabase { try $0.exportWithPassphrase(dest: destination.path, passphrase: passphrase) }
    }

    /// Imports a passphrase export as a new store: a compacted copy under a
    /// fresh data key, protected by a new key stored in `keyStore` under the
    /// configuration's policy. The transfer passphrase doesn't open it. The
    /// store's file must not exist yet; the source is unchanged.
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

    // MARK: Embedding

    /// One text's vector, embedded here, outside the actor.
    private nonisolated func embed(_ text: String) throws -> [Float] {
        try embed([text])[0]
    }

    private nonisolated func embed(_ texts: [String]) throws -> [[Float]] {
        guard let embedder = embedderBox.value else {
            throw MnemoError.Invalid(message: "no embedder set; call set_embedder first")
        }
        return try embedTexts(embedder: embedder, texts: texts, dimensions: configuration.dimensions)
    }

    private nonisolated func embedIfSet(_ text: String) throws -> [Float]? {
        embedderBox.value == nil ? nil : try embed(text)
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
        cancelPendingCommit()
        do {
            try db.flush()
            commits += 1
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
    ///
    /// A `write` applies the flush policy afterwards, in the foreground (the
    /// background path commits as it closes).
    private func withDatabase<T>(write: Bool = false, _ body: (MnemoDb) throws -> T) async throws -> T {
        if lifecycle.isForeground {
            let value = try onHandle(body)
            if write { try noteWrite() }
            return value
        }
        // No background time left: fail before the file is opened, so the
        // app is never suspended holding its lock (RB-001.20). A long call
        // made of several engine steps (an ingest's plan and apply) begins an
        // activity per step, so it stops at the first step with no time.
        let activity = try await LifecycleObserver.beginActivity(reason: activityReason)
        // Deferred blocks run last-first: close, then end the activity.
        defer { activity.end() }
        defer { closeAfterBackgroundCall() }
        return try onHandle(body)
    }

    // MARK: Flush policy

    /// After a write made in the foreground: commit now, start the coalesced
    /// timer, or nothing, by policy. A failed commit throws to the writer.
    private func noteWrite() throws {
        switch configuration.flushPolicy {
        case .manual:
            return
        case .everyWrite:
            try commit()
        case let .coalesced(maxDelay, maxWrites):
            pendingWrites += 1
            if pendingWrites >= max(1, maxWrites) {
                try commit()
            } else if commitTimer == nil {
                armCommitTimer(after: maxDelay)
            }
        }
    }

    /// The end of a session: one commit, unless the policy is manual or the
    /// file was closed meanwhile (a background call closes, and commits).
    private func finishSession(writes: Int) throws {
        guard writes > 0, configuration.flushPolicy != .manual, db != nil else { return }
        try commit()
    }

    /// Flushes the open file and clears the coalesced state. A no-op while
    /// the file is closed.
    private func commit() throws {
        cancelPendingCommit()
        guard let db else { return }
        try db.flush()
        commits += 1
    }

    private func cancelPendingCommit() {
        pendingWrites = 0
        commitTimer?.cancel()
        commitTimer = nil
        timerGeneration &+= 1
    }

    private func armCommitTimer(after seconds: TimeInterval) {
        timerGeneration &+= 1
        let generation = timerGeneration
        let nanos = UInt64(max(0, seconds) * 1_000_000_000)
        commitTimer = Task { [weak self] in
            guard (try? await Task.sleep(nanoseconds: nanos)) != nil else { return }
            await self?.coalescedCommit(generation: generation)
        }
    }

    /// The coalesced timer fired. It commits an open file only: a store that
    /// closed meanwhile (it committed then) isn't reopened.
    private func coalescedCommit(generation: Int) {
        guard generation == timerGeneration else { return }
        commitTimer = nil
        guard db != nil, pendingWrites > 0 else {
            pendingWrites = 0
            return
        }
        do {
            try commit()
        } catch {
            lastBackgroundError = error
        }
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
        cancelPendingCommit()
        do {
            try db.flush()
            commits += 1
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
            if let embedder = embedderBox.value { try opened.setEmbedder(embedder: embedder) }
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
            if let maxSnapshots {
                try opened.setMaxSnapshots(count: maxSnapshots)
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
        // RB-001.28 part 1: the policy is the key's, fixed at creation. A
        // different configured policy would flip the backup flag but not the
        // Keychain item, so a restore brings the file without its key; refuse
        // it, before anything is changed (v0.12.2).
        if let stored = try keyStore.policy(id: id), stored != configuration.keyPolicy {
            throw MnemoStoreError.keyPolicyMismatch(stored: stored, configured: configuration.keyPolicy)
        }
        return (id, try MnemoDb.openWithKey(path: fileURL.path, key: key.data))
    }

    /// The key id and key for the existing file: the stamped id, or else the
    /// first stored key that opens the file (which is then stamped). The key
    /// is copied out of whatever the key store returned into bytes the store
    /// owns, which are zeroed when the caller lets go of them.
    private func resolveKey() throws -> (String, KeyBytes) {
        if let id = KeyID.read(fileURL) ?? keyID {
            do {
                return (id, KeyBytes(copying: try keyStore.key(id: id)))
            } catch MnemoStoreError.keyMissing {
                throw MnemoStoreError.keyMissing(keyID: id, path: fileURL.path)
            }
        }
        for id in try keyStore.keyIDs() {
            let key: KeyBytes
            do {
                key = KeyBytes(copying: try keyStore.key(id: id))
            } catch MnemoStoreError.keyMissing {
                continue
            }
            // From the header alone (no KDF for a raw key, no catalog load),
            // and it answers even when the file has a WAL to recover, where a
            // read-only open refuses (v0.12.2, RB-001.23). Only `false` means
            // "not this key"; any error is the file's, not the key's.
            guard try keyOpens(path: fileURL.path, key: key.data) else {
                continue
            }
            try? KeyID.write(id, to: fileURL)
            return (id, key)
        }
        throw MnemoStoreError.keyMissing(keyID: nil, path: fileURL.path)
    }
}

/// Key bytes the store owns: a private copy (never shared with the key
/// store's own `Data`, which may be an immutable Keychain buffer), zeroed in
/// place when the last reference goes. Zeroing `Data` you don't own uniquely
/// wipes a copy instead (`Data` copies on its first mutation), which is why
/// the store copies first. Best effort: the key store's buffer and the
/// binding's FFI copy aren't reachable from here.
final class KeyBytes {
    private(set) var data: Data

    init(copying source: Data) {
        var owned = Data(count: source.count)
        owned.withUnsafeMutableBytes { dst in
            _ = source.copyBytes(to: dst)
        }
        data = owned
    }

    /// Zeroes the bytes in place (the store's copy is the only reference).
    func wipe() {
        data.zeroize()
    }

    deinit {
        wipe()
    }
}

/// The store's embedder, readable from outside the actor.
final class EmbedderBox: @unchecked Sendable {
    private let lock = NSLock()
    private var embedder: (any MnemoEmbedder)?

    init(_ embedder: (any MnemoEmbedder)?) { self.embedder = embedder }

    var value: (any MnemoEmbedder)? {
        get { lock.lock(); defer { lock.unlock() }; return embedder }
        set { lock.lock(); embedder = newValue; lock.unlock() }
    }
}
