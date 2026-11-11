import Foundation
import Testing
@testable import Mnemo

/// MOBILE-2f (v0.12.3): the store runs the app's synchronous embedder on its
/// own serial queue, so a slow embed never blocks Swift's cooperative pool.
@Suite struct EmbedQueueTests {
    /// Twice as many stores as the machine has cores, each with a slow embed
    /// in flight (before v0.12.3 those blocked every cooperative-pool
    /// thread), and more short tasks than cores: each task's 50 ms sleep
    /// ends within about 0.5 s, while the embeds are still running.
    @Test func slowEmbedsDontStarveTheAppsOtherTasks() async throws {
        let cores = ProcessInfo.processInfo.activeProcessorCount
        let embedders = (0..<(2 * cores)).map { _ in SlowEmbedder(seconds: 1.5) }
        var stores: [MnemoStore] = []
        for (i, embedder) in embedders.enumerated() {
            stores.append(try MnemoStore(
                configuration: MnemoStoreConfiguration(name: "starve\(i)", dimensions: 32,
                                                       directory: try scratchDirectory()),
                keyStore: MemoryKeyStore(), embedder: embedder))
        }
        let launched = Date()
        let writers = stores.map { store in Task { try await store.rememberText("slow to embed") } }
        #expect(try await eventually(5) { embedders.contains(where: \.started) })
        try await Task.sleep(nanoseconds: 100_000_000)

        // Timed from before the tasks are scheduled, so a task that waits
        // for a free thread counts that wait.
        let taskCount = cores + 2
        let start = Date()
        let waits = await withTaskGroup(of: Double.self) { group in
            for _ in 0..<taskCount {
                group.addTask {
                    try? await Task.sleep(nanoseconds: 50_000_000)
                    return Date().timeIntervalSince(start)
                }
            }
            var all: [Double] = []
            for await w in group { all.append(w) }
            return all
        }
        let stillEmbedding = embedders.filter { !$0.finished }.count
        // The test's own task resumed while the first embeds were running.
        let sinceLaunch = Date().timeIntervalSince(launched)
        #expect(sinceLaunch < 1.2, "the short tasks ended \(sinceLaunch) s after the embeds began")
        #expect(waits.count == taskCount)
        #expect(waits.allSatisfy { $0 < 0.5 }, "slowest task: \(waits.max() ?? 0) s")
        #expect(stillEmbedding > 0, "the embeds were over before the tasks ran")

        for writer in writers { _ = try await writer.value }
        for store in stores {
            #expect(try await store.count() == 1)
            try await store.close()
        }
    }

    /// A cancelled text call stops waiting at once and writes nothing; the
    /// embed already running finishes on the queue and its result is dropped.
    @Test func aCancelledTextCallStopsWaitingAndWritesNothing() async throws {
        let slow = SlowEmbedder(seconds: 1.5)
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "cancel", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: slow)
        let writer = Task { try await store.rememberText("never stored") }
        #expect(try await eventually(5) { slow.started })
        let start = Date()
        writer.cancel()
        await #expect(throws: CancellationError.self) { try await writer.value }
        #expect(Date().timeIntervalSince(start) < 0.5)

        #expect(try await eventually(5) { slow.finished })
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(try await store.count() == 0)
        // The store still works, and the queue is free for the next embed.
        let fast = Task { try await store.rememberText("stored") }
        _ = try await fast.value
        #expect(try await store.count() == 1)
        try await store.close()
    }

    /// Embeds on one store run one at a time (Core ML models often aren't
    /// safe to call concurrently), whatever the number of callers.
    @Test func embedsOnOneStoreRunOneAtATime() async throws {
        let counting = ConcurrencyCountingEmbedder()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "serial", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: counting)
        try await withThrowingTaskGroup(of: Void.self) { group in
            for i in 0..<6 {
                group.addTask { try await store.rememberText("note \(i)") }
            }
            try await group.waitForAll()
        }
        #expect(counting.maxConcurrent == 1)
        #expect(counting.calls == 6)
        #expect(try await store.count() == 6)
        try await store.close()
    }
}

/// Records how many embeds overlap; each takes 50 ms.
final class ConcurrencyCountingEmbedder: MnemoEmbedder, @unchecked Sendable {
    private let lock = NSLock()
    private var running = 0
    private var _maxConcurrent = 0
    private var _calls = 0
    private let words = WordEmbedder()

    var maxConcurrent: Int { lock.lock(); defer { lock.unlock() }; return _maxConcurrent }
    var calls: Int { lock.lock(); defer { lock.unlock() }; return _calls }

    func identity() -> EmbedderIdentity { words.identity() }

    func embed(texts: [String]) throws -> [[Float]] {
        lock.lock(); running += 1; _calls += 1; _maxConcurrent = max(_maxConcurrent, running); lock.unlock()
        Thread.sleep(forTimeInterval: 0.05)
        lock.lock(); running -= 1; lock.unlock()
        return try words.embed(texts: texts)
    }
}
