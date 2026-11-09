import Foundation
import Mnemo

/// The Benchmark screen's engine: the `device_budget` cells at 1K and 10K
/// (design note Q7), on synthetic vectors and raw-key files, through the same
/// Swift API an app uses. The phases and JSON keys follow
/// `mnemo/examples/device_budget.rs`; the differences are listed under
/// `corpus` in the output.
final class DeviceBudget: @unchecked Sendable {
    static let sizes = [1_000, 10_000]
    static let openCycles = 20
    static let warmQueries = 1_000
    static let singleWrites = 20
    static let batches = 5
    static let batchSize = 100

    private let root: URL
    private let report: @Sendable (String) -> Void
    private let queries: [[Float]]

    /// - Parameter report: called with a line of progress (any thread).
    init(report: @escaping @Sendable (String) -> Void) throws {
        root = try FileManager.default
            .url(for: .cachesDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
            .appendingPathComponent("device_budget", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        self.report = report
        var q = ClusteredVectors(clusters: 100, seed: 0xDE71_CE00)
        queries = q.take(Self.warmQueries)
    }

    /// Runs every phase at every size and returns the JSON document.
    func run() async throws -> [String: Any] {
        let started = DispatchTime.now()
        var results: [String: Any] = [:]
        for n in Self.sizes {
            report("n = \(n): building the corpus (cached after the first run)")
            let build = try phaseBuild(n)
            let key = try corpusKey(n)
            report("n = \(n): file")
            let file = try phaseFile(n, key: key)
            report("n = \(n): one handle, \(Self.warmQueries) warm recalls")
            let handle = try phaseHandle(n, key: key)
            report("n = \(n): \(Self.openCycles) fresh handles")
            let cycles = try phaseCycles(n, key: key)
            report("n = \(n): raw-key opens")
            let keyopen = try phaseKeyOpen(n, key: key)
            report("n = \(n): durable writes")
            let writes = try phaseWrites(n, key: key)
            report("n = \(n): background close and reopen")
            let background = try await phaseBackground(n, key: key)
            results[String(n)] = [
                "build": build, "file": file, "handle": handle, "cycles": cycles,
                "writes": writes, "layers": [String: Any](), "keyopen": keyopen,
                "background": background,
            ]
        }
        let device = await DeviceInfo.snapshot()
        return [
            "schema": "device_budget_mobile/1",
            "timestamp": ISO8601DateFormatter().string(from: Date()),
            "device": device,
            "build": DeviceInfo.build,
            "corpus": [
                "dimensions": Corpus.dimensions,
                "text_bytes": Corpus.textBytes,
                "generator": "device_budget.rs text_for + corpus.rs gen_clustered_vectors (seeds 0xBEEFC0DE, 0xDE71CE00, 0x57170001)",
                "key": "raw 32-byte key (createWithKey / openWithKey): no KDF on open",
                "index": "none: the mobile API builds no IVF+PQ index, so recall reads the vector column",
                "not_measured": "layers, open_secure_kdf_*, *_fsyncs (not in the mobile API)",
            ],
            "results": results,
            "elapsed_secs": millis(since: started) / 1_000,
        ]
    }

    // MARK: Phases

    private func corpusURL(_ n: Int) -> URL { root.appendingPathComponent("corpus_\(n)_384d_300b_v9.mnemo") }
    private func keyURL(_ n: Int) -> URL { root.appendingPathComponent("corpus_\(n).key") }

    /// The corpus key. Synthetic bench data only, so its raw key sits in a
    /// file beside it, which a real app must never do (use `MnemoStore`).
    private func corpusKey(_ n: Int) throws -> Data { try Data(contentsOf: keyURL(n)) }

    private static let config = MnemoConfig(dimensions: UInt32(Corpus.dimensions), formatVersion: 0,
                                            vectorCacheBudgetBytes: 16 << 20, cachePages: 256)
    private static let recallOptions = RecallOptions(topK: 10, trackAccess: false)

    private func phaseBuild(_ n: Int) throws -> [String: Any] {
        let fm = FileManager.default
        if fm.fileExists(atPath: corpusURL(n).path), fm.fileExists(atPath: keyURL(n).path) {
            return ["built": false]
        }
        try? fm.removeItem(at: corpusURL(n))
        let start = DispatchTime.now()
        let key = generateKey()
        let db = try MnemoDb.createWithKey(path: corpusURL(n).path, key: key, config: Self.config)
        var vectors = ClusteredVectors(clusters: Corpus.clusters(for: n), seed: 0xBEEF_C0DE)
        let options = RememberOptions(memoryType: .semantic)
        for i in 0..<n {
            try db.remember(content: Corpus.text(i), vector: vectors.next(), options: options)
            if i % 1_000 == 999 { report("n = \(n): \(i + 1) memories") }
        }
        try db.flush()
        try db.close()
        try key.write(to: keyURL(n), options: .atomic)
        return ["built": true, "build_secs": millis(since: start) / 1_000]
    }

    private func phaseFile(_ n: Int, key: Data) throws -> [String: Any] {
        let db = try MnemoDb.openReadOnlyWithKey(path: corpusURL(n).path, key: key)
        let stats = try db.stats()
        try db.close()
        let copy = try scratchCopy(n, "compact")
        defer { try? FileManager.default.removeItem(at: copy) }
        let compacted = try compactFileWithKey(path: copy.path, key: key)
        return [
            "memories": stats.memories,
            "format_version": stats.formatVersion,
            "has_index": stats.hasIndex,
            "file_bytes": stats.fileBytes,
            "bytes_per_memory": Double(stats.fileBytes) / Double(n),
            "compacted_file_bytes": compacted.after,
            "compacted_bytes_per_memory": Double(compacted.after) / Double(n),
        ]
    }

    private func phaseHandle(_ n: Int, key: Data) throws -> [String: Any] {
        let before = DeviceInfo.footprintKB()
        var start = DispatchTime.now()
        let db = try MnemoDb.openWithKey(path: corpusURL(n).path, key: key)
        let openMs = millis(since: start)
        let afterOpen = DeviceInfo.footprintKB()
        start = DispatchTime.now()
        _ = try db.recall(vector: queries[0], options: Self.recallOptions)
        let firstMs = millis(since: start)
        let afterFirst = DeviceInfo.footprintKB()
        var warm: [Double] = []
        warm.reserveCapacity(queries.count)
        for q in queries {
            let t = DispatchTime.now()
            _ = try db.recall(vector: q, options: Self.recallOptions)
            warm.append(millis(since: t))
        }
        let afterWarm = DeviceInfo.footprintKB()
        try db.close()
        let afterDrop = DeviceInfo.footprintKB()
        let (med, p95) = medianP95(warm)
        return [
            "open_ms": openMs,
            "first_recall_ms": firstMs,
            "warm_recall_median_ms": med,
            "warm_recall_p95_ms": p95,
            "rss_kb_before_open": before,
            "rss_kb_open": afterOpen,
            "rss_kb_after_first_recall": afterFirst,
            "rss_kb_after_warm": afterWarm,
            "rss_kb_after_drop": afterDrop,
            "handle_rss_kb": afterWarm > before ? afterWarm - before : 0,
            "rss_metric": "task_vm_info.phys_footprint",
        ]
    }

    private func phaseCycles(_ n: Int, key: Data) throws -> [String: Any] {
        var opens: [Double] = [], firsts: [Double] = []
        for i in 0..<Self.openCycles {
            var t = DispatchTime.now()
            let db = try MnemoDb.openWithKey(path: corpusURL(n).path, key: key)
            opens.append(millis(since: t))
            t = DispatchTime.now()
            _ = try db.recall(vector: queries[i % queries.count], options: Self.recallOptions)
            firsts.append(millis(since: t))
            try db.close()
        }
        let (om, op) = medianP95(opens), (fm, fp) = medianP95(firsts)
        return ["cycles": Self.openCycles, "open_median_ms": om, "open_p95_ms": op,
                "first_recall_median_ms": fm, "first_recall_p95_ms": fp]
    }

    private func phaseKeyOpen(_ n: Int, key: Data) throws -> [String: Any] {
        let before = DeviceInfo.footprintKB()
        var opens: [Double] = []
        var afterFirstOpen: UInt64 = 0
        for i in 0..<Self.openCycles {
            let t = DispatchTime.now()
            let db = try MnemoDb.openWithKey(path: corpusURL(n).path, key: key)
            opens.append(millis(since: t))
            if i == 0 { afterFirstOpen = DeviceInfo.footprintKB() }
            try db.close()
        }
        let (med, p95) = medianP95(opens)
        return ["open_with_key_median_ms": med, "open_with_key_p95_ms": p95,
                "rss_kb_before_open": before, "rss_kb_after_key_open": afterFirstOpen]
    }

    private func phaseWrites(_ n: Int, key: Data) throws -> [String: Any] {
        let copy = try scratchCopy(n, "writes")
        defer { try? FileManager.default.removeItem(at: copy) }
        let bytesBefore = fileSize(copy)
        let db = try MnemoDb.openWithKey(path: copy.path, key: key)
        var vectors = ClusteredVectors(clusters: 16, seed: 0x5717_0001)
        let options = RememberOptions(memoryType: .semantic)
        var single: [Double] = []
        for i in 0..<Self.singleWrites {
            let v = vectors.next()
            let t = DispatchTime.now()
            try db.remember(content: Corpus.text(n + i), vector: v, options: options)
            try db.flush()
            single.append(millis(since: t))
        }
        let bytesAfterSingle = fileSize(copy)
        var batched: [Double] = []
        for b in 0..<Self.batches {
            let vs = vectors.take(Self.batchSize)
            let t = DispatchTime.now()
            for j in 0..<Self.batchSize {
                try db.remember(content: Corpus.text(n + Self.singleWrites + b * Self.batchSize + j),
                                vector: vs[j], options: options)
            }
            try db.flush()
            batched.append(millis(since: t))
        }
        try db.close()
        let bytesAfterBatches = fileSize(copy)
        let (sm, sp) = medianP95(single), (bm, bp) = medianP95(batched)
        return [
            "single_write_median_ms": sm,
            "single_write_p95_ms": sp,
            "batch_size": Self.batchSize,
            "batch_write_median_ms": bm,
            "batch_write_p95_ms": bp,
            "batch_write_per_memory_ms": bm / Double(Self.batchSize),
            "file_growth_per_single_write_bytes": growth(bytesBefore, bytesAfterSingle) / UInt64(Self.singleWrites),
            "file_growth_per_batch_bytes": growth(bytesAfterSingle, bytesAfterBatches) / UInt64(Self.batches),
        ]
    }

    /// The Q1 transition, through `MnemoStore`: one pending write, then the
    /// close a background transition does (flush, close, release the lock),
    /// then the next call, which reopens the file.
    private func phaseBackground(_ n: Int, key: Data) async throws -> [String: Any] {
        let dir = root.appendingPathComponent("background", isDirectory: true)
        try? FileManager.default.removeItem(at: dir)
        try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        try FileManager.default.copyItem(at: corpusURL(n), to: dir.appendingPathComponent("bg.mnemo"))
        let keys = BenchKeyStore()
        try keys.storeKey(key, id: "bench", policy: .deviceOnly)
        let store = try MnemoStore(
            configuration: MnemoStoreConfiguration(name: "bg", dimensions: UInt32(Corpus.dimensions), directory: dir),
            keyStore: keys)
        _ = try await store.count() // first open: finds the key, stamps its id
        var vectors = ClusteredVectors(clusters: 16, seed: 0xB6_0001)
        var closes: [Double] = [], reopens: [Double] = []
        for i in 0..<Self.openCycles {
            try await store.remember(Corpus.text(n + i), vector: vectors.next())
            var t = DispatchTime.now()
            try await store.close()
            closes.append(millis(since: t))
            t = DispatchTime.now()
            _ = try await store.count()
            reopens.append(millis(since: t))
        }
        try await store.close()
        let (cm, cp) = medianP95(closes), (rm, rp) = medianP95(reopens)
        return ["cycles": Self.openCycles, "close_median_ms": cm, "close_p95_ms": cp,
                "reopen_median_ms": rm, "reopen_p95_ms": rp,
                "note": "close = flush one pending write + close; reopen = the next call (raw-key open)"]
    }

    // MARK: Helpers

    private func scratchCopy(_ n: Int, _ tag: String) throws -> URL {
        let dst = root.appendingPathComponent("scratch_\(n)_\(tag).mnemo")
        try? FileManager.default.removeItem(at: dst)
        try FileManager.default.copyItem(at: corpusURL(n), to: dst)
        return dst
    }

    /// `after - before`, or 0 when the file shrank.
    private func growth(_ before: UInt64, _ after: UInt64) -> UInt64 { after > before ? after - before : 0 }

    private func fileSize(_ url: URL) -> UInt64 {
        ((try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? NSNumber)?.uint64Value ?? 0
    }
}

/// Keys in memory, for the bench's `MnemoStore` (the corpus key is already
/// known, so there's nothing to keep in the Keychain).
final class BenchKeyStore: MnemoKeyStore, @unchecked Sendable {
    private let lock = NSLock()
    private var keys: [String: Data] = [:]

    func storeKey(_ key: Data, id: String, policy: KeyPolicy) throws {
        lock.lock(); defer { lock.unlock() }
        keys[id] = key
    }

    func key(id: String) throws -> Data {
        lock.lock(); defer { lock.unlock() }
        guard let key = keys[id] else { throw MnemoStoreError.keyMissing(keyID: id, path: nil) }
        return key
    }

    func keyIDs() throws -> [String] {
        lock.lock(); defer { lock.unlock() }
        return keys.keys.sorted()
    }

    func deleteKey(id: String) throws {
        lock.lock(); defer { lock.unlock() }
        keys[id] = nil
    }
}
