import Foundation
import Testing
@testable import Mnemo

/// The flush policy and sessions (MOBILE-2b). "Committed" means on disk: a
/// byte copy of the file, opened on its own, has it. That's what survives a
/// kill. The kill itself is `scripts/kill-test.sh`, on a simulator.
@Suite struct FlushPolicyTests {
    private func store(_ name: String, _ policy: FlushPolicy, keys: MemoryKeyStore = MemoryKeyStore()) throws -> MnemoStore {
        try MnemoStore(
            configuration: MnemoStoreConfiguration(name: name, dimensions: 4, directory: try scratchDirectory(),
                                                   flushPolicy: policy),
            keyStore: keys)
    }

    @Test func theDefaultIsCoalesced() {
        #expect(MnemoStoreConfiguration(name: "x", dimensions: 4).flushPolicy == .coalesced(maxDelay: 1, maxWrites: 64))
        #expect(FlushPolicy.default == .coalesced())
    }

    @Test func coalescedCommitsOnceWithinTheWindow() async throws {
        let keys = MemoryKeyStore()
        let s = try store("window", .coalesced(maxDelay: 0.3, maxWrites: 64), keys: keys)
        for i in 0..<3 { try await s.remember("note \(i)", vector: vec(i)) }
        #expect(await s.commits == 0)
        #expect(try committedCount(s.fileURL, keys: keys) == 0)
        #expect(try await eventually(5) { await s.commits == 1 })
        #expect(try committedCount(s.fileURL, keys: keys) == 3)
        try await Task.sleep(nanoseconds: 600_000_000)
        #expect(await s.commits == 1, "no second commit without new writes")
        try await s.close()
    }

    @Test func aBurstOfMaxWritesIsOneCommit() async throws {
        let keys = MemoryKeyStore()
        let s = try store("burst", .default, keys: keys)
        for i in 0..<64 { try await s.remember("note \(i)", vector: vec(i)) }
        #expect(await s.commits == 1, "the 64th write commits")
        try await Task.sleep(nanoseconds: 1_300_000_000)
        #expect(await s.commits == 1, "and the timer it cancelled doesn't fire")
        #expect(try committedCount(s.fileURL, keys: keys) == 64)
        try await s.close()
    }

    @Test func aBurstOf100WritesIsOneCommit() async throws {
        let keys = MemoryKeyStore()
        let s = try store("burst100", .coalesced(maxDelay: 0.3, maxWrites: 1000), keys: keys)
        for i in 0..<100 { try await s.remember("note \(i)", vector: vec(i)) }
        #expect(await s.commits == 0)
        #expect(try await eventually(5) { await s.commits == 1 })
        try await Task.sleep(nanoseconds: 600_000_000)
        #expect(await s.commits == 1)
        #expect(try committedCount(s.fileURL, keys: keys) == 100)
        try await s.close()
    }

    @Test func everyWriteCommitsEachTime() async throws {
        let keys = MemoryKeyStore()
        let s = try store("every", .everyWrite, keys: keys)
        for i in 0..<10 {
            try await s.remember("note \(i)", vector: vec(i))
            #expect(await s.commits == i + 1)
            #expect(try committedCount(s.fileURL, keys: keys) == UInt64(i + 1))
        }
        try await s.close()
    }

    @Test func manualCommitsOnlyWhenAsked() async throws {
        let keys = MemoryKeyStore()
        let s = try store("manual", .manual, keys: keys)
        for i in 0..<5 { try await s.remember("note \(i)", vector: vec(i)) }
        try await Task.sleep(nanoseconds: 1_300_000_000)
        #expect(await s.commits == 0)
        #expect(try committedCount(s.fileURL, keys: keys) == 0)
        try await s.flush()
        #expect(await s.commits == 1)
        #expect(try committedCount(s.fileURL, keys: keys) == 5)
        try await s.close()
    }

    @Test func aTimerOnAClosedStoreDoesNotReopenIt() async throws {
        let keys = MemoryKeyStore()
        let s = try store("closed", .coalesced(maxDelay: 0.3, maxWrites: 64), keys: keys)
        try await s.remember("one", vector: vec(0))
        try await s.close()
        #expect(await s.commits == 1)
        try await Task.sleep(nanoseconds: 700_000_000)
        #expect(await s.isOpen == false)
        #expect(await s.commits == 1)
        try assertUnlockedAndDurable(s.fileURL, keys: keys, count: 1)
    }

    @Test func aBackgroundCloseCommitsAndCancelsThePendingCommit() async throws {
        let keys = MemoryKeyStore()
        let s = try store("bgcancel", .coalesced(maxDelay: 0.3, maxWrites: 64), keys: keys)
        try await s.remember("one", vector: vec(0))
        await s.closeForBackground()
        #expect(await s.commits == 1)
        try await Task.sleep(nanoseconds: 700_000_000)
        #expect(await s.isOpen == false, "the timer doesn't reopen the file")
        #expect(await s.commits == 1)
        try assertUnlockedAndDurable(s.fileURL, keys: keys, count: 1)
    }

    @Test func aSessionIsOneCommit() async throws {
        let keys = MemoryKeyStore()
        let s = try store("session", .default, keys: keys)
        let kept = try await s.withSession { session -> MnemoSession in
            for i in 0..<100 { try session.remember("note \(i)", vector: vec(i)) }
            #expect(try session.count() == 100)
            return session
        }
        #expect(await s.commits == 1)
        #expect(try committedCount(s.fileURL, keys: keys) == 100)
        #expect(throws: MnemoError.Closed) { try kept.count() }
        try await Task.sleep(nanoseconds: 1_300_000_000)
        #expect(await s.commits == 1)
        try await s.close()
    }

    @Test func aSessionThatThrowsStillCommitsItsWrites() async throws {
        struct Stop: Error {}
        let keys = MemoryKeyStore()
        let s = try store("sessionthrow", .default, keys: keys)
        await #expect(throws: Stop.self) {
            try await s.withSession { session in
                try session.remember("kept", vector: vec(0))
                throw Stop()
            }
        }
        #expect(await s.commits == 1)
        #expect(try committedCount(s.fileURL, keys: keys) == 1)
        await #expect(throws: MnemoError.self) {
            try await s.withSession { session in
                try session.update(id: "01J00000000000000000000000", patch: MemoryPatch(content: "no vector"))
            }
        }
        try await s.close()
    }

    @Test func aManualSessionDoesNotCommit() async throws {
        let keys = MemoryKeyStore()
        let s = try store("sessionmanual", .manual, keys: keys)
        try await s.withSession { session in
            for i in 0..<5 { try session.remember("note \(i)", vector: vec(i)) }
        }
        #expect(await s.commits == 0)
        try await s.close()
        #expect(try committedCount(s.fileURL, keys: keys) == 5)
    }
}

/// The memories a byte copy of `file` holds right now: what a kill would
/// leave. The copy opens on its own, so the store keeps its lock.
func committedCount(_ file: URL, keys: any MnemoKeyStore) throws -> UInt64 {
    let copy = file.deletingLastPathComponent()
        .appendingPathComponent("copy-\(UUID().uuidString).mnemo")
    try FileManager.default.copyItem(at: file, to: copy)
    defer { try? FileManager.default.removeItem(at: copy) }
    var key = try keys.key(id: try #require(KeyID.read(file)))
    defer { key.zeroize() }
    let db = try MnemoDb.openReadOnlyWithKey(path: copy.path, key: key)
    defer { try? db.close() }
    return try db.len()
}
