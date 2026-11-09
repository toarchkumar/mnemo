import SwiftUI
import UIKit

struct BenchmarkView: View {
    @State private var running = false
    @State private var log: [String] = []
    @State private var resultURL: URL?
    @State private var summary: [String] = []
    @State private var error: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Button(running ? "Running..." : "Run the benchmark") { start() }
                        .disabled(running)
                    if let resultURL {
                        ShareLink(item: resultURL) { Label("Share the JSON", systemImage: "square.and.arrow.up") }
                        Text(resultURL.lastPathComponent).font(.caption).foregroundStyle(.secondary)
                    }
                    if let error { Text(error).foregroundStyle(.red).font(.footnote) }
                } footer: {
                    Text("The device_budget cells at 1K and 10K memories (384-dim synthetic vectors, raw-key files). Use a Release build, keep the app in the foreground and the phone unplugged; it takes a few minutes the first time, while it builds the corpus.")
                }
                if !summary.isEmpty {
                    Section("Results") {
                        ForEach(summary, id: \.self) { Text($0).font(.footnote.monospaced()) }
                    }
                }
                if !log.isEmpty {
                    Section("Progress") {
                        ForEach(Array(log.enumerated()), id: \.offset) { Text($0.element).font(.caption) }
                    }
                }
            }
            .navigationTitle("Benchmark")
        }
    }

    private func start() {
        running = true
        error = nil
        log = []
        summary = []
        UIApplication.shared.isIdleTimerDisabled = true
        Task.detached(priority: .userInitiated) {
            do {
                let bench = try DeviceBudget { line in
                    Task { @MainActor in log.append(line) }
                }
                let doc = try await bench.run()
                let url = try write(doc)
                let lines = summarize(doc)
                await MainActor.run {
                    resultURL = url
                    summary = lines
                }
            } catch {
                await MainActor.run { self.error = "\(error)" }
            }
            await MainActor.run {
                running = false
                UIApplication.shared.isIdleTimerDisabled = false
            }
        }
    }
}

/// Writes the JSON to Documents (visible in the Files app).
private func write(_ doc: [String: Any]) throws -> URL {
    let docs = try FileManager.default.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
    let device = ((doc["device"] as? [String: Any])?["model_identifier"] as? String ?? "device")
        .replacingOccurrences(of: ",", with: "_")
    let stamp = ISO8601DateFormatter().string(from: Date()).replacingOccurrences(of: ":", with: "")
    let url = docs.appendingPathComponent("device_budget_\(device)_\(stamp).json")
    let data = try JSONSerialization.data(withJSONObject: doc, options: [.prettyPrinted, .sortedKeys])
    try data.write(to: url, options: .atomic)
    return url
}

private func summarize(_ doc: [String: Any]) -> [String] {
    guard let results = doc["results"] as? [String: Any] else { return [] }
    func num(_ cell: [String: Any], _ phase: String, _ key: String) -> Double {
        ((cell[phase] as? [String: Any])?[key] as? NSNumber)?.doubleValue ?? .nan
    }
    return results.keys.sorted { Int($0)! < Int($1)! }.compactMap { n in
        guard let cell = results[n] as? [String: Any] else { return nil }
        return String(format: "%@: %.0f B/mem, handle %.1f MB, key open %.1f ms, warm recall %.2f ms (p95 %.2f), write %.1f ms, bg close %.1f / reopen %.1f ms",
                      n as NSString, num(cell, "file", "bytes_per_memory"), num(cell, "handle", "handle_rss_kb") / 1024,
                      num(cell, "keyopen", "open_with_key_median_ms"), num(cell, "handle", "warm_recall_median_ms"),
                      num(cell, "handle", "warm_recall_p95_ms"), num(cell, "writes", "single_write_median_ms"),
                      num(cell, "background", "close_median_ms"), num(cell, "background", "reopen_median_ms"))
    }
}
