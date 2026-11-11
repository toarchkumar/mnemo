import Foundation
import Testing
@testable import Mnemo

/// The embedder runs outside the store (MOBILE-2c): before a call reaches
/// the actor, so the store keeps serving other calls while it runs.
@Suite struct EmbedderLockTests {
    @Test func anEmbedderThatCallsBackIntoTheStoreCompletes() async throws {
        let embedder = CallbackEmbedder()
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "callback", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: embedder)
        embedder.store = store
        try await store.remember("already here", vector: WordEmbedder().vector(for: "already here"))

        try await store.rememberText("a note whose embedder asks the store first")
        let hits = try await store.recallText("note", options: RecallOptions(topK: 1))
        try await store.essenceAdd(.decisions, "an essence item embedded with a callback")
        try await store.ingestThread("t", turns: [ThreadTurn(turnId: "1", role: "user", content: "hello", timestamp: nil)])
        #expect(embedder.deadlocks == 0)
        #expect(embedder.answers.allSatisfy { $0 >= 1 })
        #expect(embedder.answers.count >= 4)
        #expect(hits.hits.count == 1)
        try await store.close()
    }

    @Test func aSlowEmbedderDoesNotBlockAnUnrelatedGet() async throws {
        let slow = SlowEmbedder(seconds: 1.5)
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "slow", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore(), embedder: slow)
        let existing = try await store.remember("here", vector: WordEmbedder().vector(for: "here"))
        let writer = Task { try await store.rememberText("slow to embed") }
        #expect(try await eventually(5) { slow.started })
        let start = Date()
        #expect(try await store.get(id: existing.id).content == "here")
        let waited = Date().timeIntervalSince(start)
        #expect(waited < 0.5, "get waited \(waited) s for the embedder")
        _ = try await writer.value
        #expect(try await store.count() == 2)
        try await store.close()
    }

    @Test func embedderErrorsAreUnchangedAndWriteNothing() async throws {
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "emberr", dimensions: 32, directory: try scratchDirectory()),
            keyStore: MemoryKeyStore())
        // No embedder: the text calls say so.
        await #expect(throws: MnemoError.Invalid(message: "no embedder set; call set_embedder first")) {
            try await store.rememberText("x")
        }
        try await store.setEmbedder(WrongWidthEmbedder())
        await #expect(throws: MnemoError.self) { try await store.rememberText("too narrow") }
        #expect(try await store.count() == 0)
        try await store.close()
    }
}

/// An embedder that asks the store for its count before every embed, the
/// way an app's embedder might read a cache or a setting from the store.
/// It waits up to 5 s for the answer and counts a timeout as a deadlock.
final class CallbackEmbedder: MnemoEmbedder, @unchecked Sendable {
    weak var store: MnemoStore?
    private let lock = NSLock()
    private var _answers: [UInt64] = []
    private var _deadlocks = 0
    private let words = WordEmbedder()

    var answers: [UInt64] { lock.lock(); defer { lock.unlock() }; return _answers }
    var deadlocks: Int { lock.lock(); defer { lock.unlock() }; return _deadlocks }

    func identity() -> EmbedderIdentity { words.identity() }

    func embed(texts: [String]) throws -> [[Float]] {
        let done = DispatchSemaphore(value: 0)
        let answer = Box<UInt64?>(nil)
        let store = self.store
        Task.detached {
            answer.value = try? await store?.count()
            done.signal()
        }
        if done.wait(timeout: .now() + 5) == .timedOut {
            lock.lock(); _deadlocks += 1; lock.unlock()
            throw EmbedderError.Failed(message: "deadlock: the store didn't answer within 5 s")
        }
        lock.lock(); _answers.append(answer.value ?? 0); lock.unlock()
        return try words.embed(texts: texts)
    }
}

/// Holds each embed until the test calls `open()`, or for at most `seconds`.
/// A test that measures while an embed is in flight opens the gate after
/// measuring, so the embed can't land first however late the test's task
/// resumes on a busy runner.
final class SlowEmbedder: MnemoEmbedder, @unchecked Sendable {
    let seconds: Double
    private let lock = NSLock()
    private var _started = false
    private let gate = DispatchSemaphore(value: 0)
    private let words = WordEmbedder()

    init(seconds: Double) { self.seconds = seconds }

    var started: Bool { lock.lock(); defer { lock.unlock() }; return _started }

    /// Lets the embed in flight (and any later one) finish now.
    func open() {
        // Enough signals for every embed a test makes.
        for _ in 0..<16 { gate.signal() }
    }

    func identity() -> EmbedderIdentity { words.identity() }

    func embed(texts: [String]) throws -> [[Float]] {
        lock.lock(); _started = true; lock.unlock()
        _ = gate.wait(timeout: .now() + seconds)
        return try words.embed(texts: texts)
    }
}

/// Claims the file's width but returns narrower vectors.
final class WrongWidthEmbedder: MnemoEmbedder, @unchecked Sendable {
    func identity() -> EmbedderIdentity { EmbedderIdentity(name: "test-words", dimensions: 32, normalize: true) }
    func embed(texts: [String]) throws -> [[Float]] { texts.map { _ in [1, 0, 0] } }
}

final class Box<T>: @unchecked Sendable {
    var value: T
    init(_ value: T) { self.value = value }
}
