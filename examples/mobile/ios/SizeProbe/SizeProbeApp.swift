import Mnemo
import SwiftUI

/// One screen that opens a store, writes and recalls, so the linker keeps
/// what an app using MNemo keeps. Compared with SizeBaseline for the size
/// report; it's built, never run.
@main
struct SizeProbeApp: App {
    @State private var status = "MNemo size probe"

    var body: some Scene {
        WindowGroup {
            Text(status).task {
                do {
                    let store = try MnemoStore(configuration: MnemoStoreConfiguration(name: "probe", dimensions: 4))
                    try await store.remember("hello", vector: [1, 0, 0, 0])
                    let hits = try await store.recall(vector: [1, 0, 0, 0],
                                                      options: RecallOptions(topK: 1, keywords: "hello"))
                    _ = try await store.history(key: "k")
                    _ = try await store.essence()
                    try await store.close()
                    status = "\(hits.hits.count) hit"
                } catch {
                    status = "\(error)"
                }
            }
        }
    }
}
