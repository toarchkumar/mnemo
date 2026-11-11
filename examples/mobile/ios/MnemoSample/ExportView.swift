import SwiftUI

/// The Export button's sheet (RB-001.14): a passphrase, asked twice, then a
/// passphrase copy of the notes in the share sheet. The copy opens with the
/// `mnemo` CLI on a Mac; the passphrase is the only secret it needs.
struct ExportView: View {
    @ObservedObject var model: NotesModel
    @Environment(\.dismiss) private var dismiss
    @State private var passphrase = ""
    @State private var again = ""
    @State private var working = false
    @State private var exported: URL?

    /// At least 8 characters, typed the same twice.
    private var problem: String? {
        if passphrase.count < 8 { return "Use at least 8 characters." }
        if passphrase != again { return "The two passphrases don't match." }
        return nil
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    SecureField("Passphrase", text: $passphrase)
                    SecureField("The same passphrase again", text: $again)
                } footer: {
                    Text(problem ?? "The export opens with this passphrase alone, on any device. Anyone with the file and the passphrase can read the notes.")
                }
                Section {
                    Button(working ? "Exporting..." : "Export") {
                        working = true
                        Task {
                            exported = await model.export(passphrase: passphrase)
                            working = false
                        }
                    }
                    .disabled(problem != nil || working)
                    if let exported {
                        ShareLink("Share \(exported.lastPathComponent)", item: exported)
                    }
                    if let error = model.error {
                        Text(error).font(.footnote).foregroundStyle(.red)
                    }
                } footer: {
                    Text("On a Mac: MNEMO_PASSPHRASE='...' mnemo info mnemo-notes-export.mnemo, or mnemo recall --text 'oat milk' for a keyword search.")
                }
            }
            .navigationTitle("Export the notes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
            }
        }
    }
}
