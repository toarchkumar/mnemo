import Mnemo
import SwiftUI

struct NotesView: View {
    @StateObject private var model = NotesModel()
    @State private var note = ""
    @State private var factKey = "drink"
    @State private var factValue = ""
    @State private var query = ""
    @State private var useKeywords = false
    @State private var turn = ""
    @State private var role = "user"

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    LabeledContent("Memories", value: "\(model.count)")
                    Text(model.embedderLabel).font(.footnote).foregroundStyle(.secondary)
                    if let error = model.error {
                        Text(error).font(.footnote).foregroundStyle(.red)
                    }
                }

                Section("Add a note") {
                    TextField("Bought oat milk for the office", text: $note, axis: .vertical)
                    Button("Remember") {
                        Task { await model.addNote(note); note = "" }
                    }
                    ForEach(model.recent.prefix(5), id: \.self) { Text($0).font(.footnote) }
                }

                Section {
                    TextField("Key", text: $factKey)
                        .textInputAutocapitalization(.never)
                    TextField("Value (tea, then coffee...)", text: $factValue)
                    HStack {
                        Button("Set") { Task { await model.setFact(key: factKey, value: factValue) } }
                        Spacer()
                        Button("Show history") { Task { await model.loadHistory(key: factKey) } }
                    }
                    .buttonStyle(.borderless)
                    ForEach(Array(model.history.enumerated()), id: \.element.id) { index, record in
                        HStack {
                            Text(record.content)
                            Spacer()
                            Text(index == 0 ? "current" : "superseded")
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                } header: {
                    Text("A keyed fact")
                } footer: {
                    Text("A new value supersedes the old one, which stays in the history. The same value again is a no-op.")
                }

                Section {
                    TextField("What did I buy?", text: $query)
                    Toggle("Also match the exact words", isOn: $useKeywords)
                    Button("Recall") { Task { await model.recall(query, keywords: useKeywords) } }
                    ForEach(model.hits, id: \.memory.id) { hit in
                        VStack(alignment: .leading) {
                            Text(hit.memory.content)
                            Text(String(format: "score %.3f, similarity %.3f", hit.score, hit.similarity))
                                .font(.caption).foregroundStyle(.secondary)
                        }
                    }
                } header: {
                    Text("Recall")
                } footer: {
                    Text("Without keywords, recall ranks by meaning. With them, it also weighs the query's exact words (names, ids, numbers).")
                }

                Section {
                    Picker("Role", selection: $role) {
                        Text("user").tag("user")
                        Text("assistant").tag("assistant")
                    }
                    .pickerStyle(.segmented)
                    TextField("A turn of the conversation", text: $turn, axis: .vertical)
                    Button("Add turn") {
                        Task { await model.addTurn(role: role, turn); turn = "" }
                    }
                    ForEach(model.turns, id: \.turnId) { t in
                        Text("\(t.role): \(t.content)").font(.footnote)
                    }
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Essence").font(.caption).foregroundStyle(.secondary)
                        Text(model.essence.isEmpty ? "(empty)" : model.essence)
                            .font(.footnote.monospaced())
                    }
                } header: {
                    Text("A thread")
                } footer: {
                    Text("Turns are kept losslessly. The essence holds the items an agent derives from them (decisions, open questions, a timeline); the mobile API reads it but doesn't write essence items yet.")
                }
            }
            .navigationTitle("MNemo notes")
            .task { await model.refresh() }
        }
    }
}
