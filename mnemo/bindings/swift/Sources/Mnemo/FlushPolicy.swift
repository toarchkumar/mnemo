import Foundation

/// When a ``MnemoStore`` commits its writes to disk on its own.
///
/// A write is durable once it's committed: a crash, a kill or a power loss
/// after that keeps it. Before that, it's in memory only. Whatever the
/// policy, the store also commits when the app goes to the background, after
/// every call made while the app isn't active, and on ``MnemoStore/flush()``
/// and ``MnemoStore/close()``.
public enum FlushPolicy: Sendable, Equatable {
    /// The default. The first write after a commit starts a timer, and the
    /// store commits once when it fires (`maxDelay` seconds later) or as soon
    /// as `maxWrites` writes are waiting, whichever comes first. A write is
    /// durable at most about `maxDelay` seconds after it returns.
    case coalesced(maxDelay: TimeInterval = 1, maxWrites: Int = 64)
    /// Every write commits before it returns. The safest, and the slowest
    /// for bursts: each commit writes and syncs pages.
    case everyWrite
    /// The store commits only on the events above. Writes made in the
    /// foreground stay in memory until then (the behaviour before v0.12.1).
    /// For benchmarks, and for apps that call ``MnemoStore/flush()``
    /// themselves.
    case manual

    /// `.coalesced(maxDelay: 1, maxWrites: 64)`.
    public static let `default`: FlushPolicy = .coalesced()
}

/// A batch of calls on one open handle, from ``MnemoStore/withSession(_:)``.
///
/// The calls run in order on the store's handle, with no other call in
/// between, and the store commits once at the end. A session doesn't embed:
/// pass vectors (embed first, then open the session). It's valid only inside
/// the closure it was passed to; afterwards every call throws
/// ``MnemoError/Closed``.
public final class MnemoSession: @unchecked Sendable {
    private let db: MnemoDb
    private let lock = NSLock()
    private var ended = false
    private(set) var writes = 0

    init(db: MnemoDb) { self.db = db }

    func end() {
        lock.lock(); ended = true; lock.unlock()
    }

    private func handle(write: Bool) throws -> MnemoDb {
        lock.lock()
        defer { lock.unlock() }
        if ended { throw MnemoError.Closed }
        if write { writes += 1 }
        return db
    }

    /// Stores a memory with its vector.
    @discardableResult
    public func remember(_ content: String, vector: [Float],
                         options: RememberOptions = RememberOptions()) throws -> WriteResult {
        try handle(write: true).remember(content: content, vector: vector, options: options)
    }

    /// Changes fields of a memory. A content change needs its vector in the
    /// patch: a session doesn't embed.
    @discardableResult
    public func update(id: String, patch: MemoryPatch) throws -> WriteOutcome {
        if patch.content != nil, patch.vector == nil {
            throw MnemoError.Invalid(message: "a session doesn't embed: a content change needs its vector in the patch")
        }
        return try handle(write: true).update(id: id, patch: patch)
    }

    /// Deletes a memory.
    public func delete(id: String) throws {
        try handle(write: true).delete(id: id)
    }

    /// One memory by id.
    public func get(id: String) throws -> MemoryRecord {
        try handle(write: false).get(id: id)
    }

    /// Recall by vector.
    public func recall(vector: [Float], options: RecallOptions = RecallOptions()) throws -> RecallPage {
        try handle(write: false).recall(vector: vector, options: options)
    }

    /// The number of live memories.
    public func count() throws -> UInt64 {
        try handle(write: false).len()
    }

    /// Stores a value in the result cache.
    public func cachePut(namespace: String, key: String, value: Data, ttlSecs: UInt64? = nil) throws {
        try handle(write: true).cachePut(namespace: namespace, key: key, value: value, ttlSecs: ttlSecs)
    }

    /// A cached value, or `nil`.
    public func cacheGet(namespace: String, key: String) throws -> Data? {
        try handle(write: false).cacheGet(namespace: namespace, key: key)
    }
}
