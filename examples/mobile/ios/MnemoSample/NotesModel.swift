import Foundation
import Mnemo

/// The notes demo: notes, a keyed fact with its history, recall with and
/// without keywords, and a thread.
@MainActor
final class NotesModel: ObservableObject {
    static let threadID = "demo-thread"

    @Published private(set) var embedderLabel = ""
    @Published private(set) var count: UInt64 = 0
    @Published private(set) var recent: [String] = []
    @Published private(set) var hits: [RecallHit] = []
    @Published private(set) var history: [MemoryRecord] = []
    @Published private(set) var turns: [ExportedTurn] = []
    @Published private(set) var essence = ""
    @Published var error: String?

    let store: MnemoStore?

    init() {
        let embedder: any MnemoEmbedder
        let name: String
        if let sentence = SentenceEmbedder() {
            embedder = sentence
            name = "notes"
            embedderLabel = "Apple NLEmbedding, sentence, \(sentence.dimensions) dims"
        } else {
            let demo = DemoWordEmbedder()
            embedder = demo
            // A different file: a file keeps the embedder it was written with.
            name = "notes-demo-embedder"
            embedderLabel = "DEMO embedder (hashed words): NLEmbedding isn't available here"
        }
        do {
            let dims = UInt32(embedder.identity().dimensions)
            store = try MnemoStore(configuration: MnemoStoreConfiguration(name: name, dimensions: dims),
                                   embedder: embedder)
        } catch {
            store = nil
            self.error = "\(error)"
        }
    }

    func refresh() async {
        await run {
            self.count = try await $0.count()
            self.turns = try await $0.threadExport(Self.threadID)
            self.essence = try await $0.essence(.thread(threadId: Self.threadID), maxBytes: 2_048)
        }
    }

    func addNote(_ text: String) async {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        await run {
            try await $0.rememberText(text, options: RememberOptions(memoryType: .episodic))
            self.recent.insert(text, at: 0)
            self.count = try await $0.count()
        }
    }

    /// A keyed write: a new value supersedes the old one, which stays in
    /// the history; writing the same value again is a no-op.
    func setFact(key: String, value: String) async {
        let key = key.trimmingCharacters(in: .whitespaces), value = value.trimmingCharacters(in: .whitespaces)
        guard !key.isEmpty, !value.isEmpty else { return }
        await run {
            try await $0.rememberText("\(key): \(value)",
                                      options: RememberOptions(memoryType: .semantic, key: "fact:\(key)"))
            self.history = try await $0.history(key: "fact:\(key)")
            self.count = try await $0.count()
        }
    }

    func loadHistory(key: String) async {
        let key = key.trimmingCharacters(in: .whitespaces)
        guard !key.isEmpty else { return }
        await run { self.history = try await $0.history(key: "fact:\(key)") }
    }

    /// Recall by meaning, or by meaning plus the query's exact words.
    func recall(_ query: String, keywords: Bool) async {
        let query = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !query.isEmpty else { return }
        await run {
            let options = RecallOptions(topK: 5, keywords: keywords ? query : nil, trackAccess: false)
            self.hits = try await $0.recallText(query, options: options).hits
        }
    }

    func addTurn(role: String, _ text: String) async {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        await run {
            try await $0.ingestThread(Self.threadID,
                                      turns: [ThreadTurn(turnId: UUID().uuidString, role: role, content: text)])
            self.turns = try await $0.threadExport(Self.threadID)
            self.essence = try await $0.essence(.thread(threadId: Self.threadID), maxBytes: 2_048)
            self.count = try await $0.count()
        }
    }

    /// An essence item derived from the thread: a decision (an editable
    /// item) or a timeline milestone (append-only).
    func addEssence(_ text: String, timeline: Bool) async {
        let text = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        let scope = EssenceScope.thread(threadId: Self.threadID)
        await run {
            if timeline {
                try await $0.essenceAppendTimeline(text, scope: scope)
            } else {
                try await $0.essenceAdd(.decisions, text, scope: scope)
            }
            self.essence = try await $0.essence(scope, maxBytes: 2_048)
        }
    }

    private func run(_ body: (MnemoStore) async throws -> Void) async {
        guard let store else { return }
        do {
            try await body(store)
            error = nil
        } catch {
            self.error = "\(error)"
        }
    }
}
