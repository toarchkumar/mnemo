import SwiftUI

/// The MNemo sample: a notes demo, the device Benchmark, and the background
/// test that checks the app isn't killed for holding a file lock while
/// suspended (`0xdead10cc`). See examples/mobile/README.md.
@main
struct MnemoSampleApp: App {
    var body: some Scene {
        WindowGroup {
            TabView {
                NotesView()
                    .tabItem { Label("Notes", systemImage: "note.text") }
                BenchmarkView()
                    .tabItem { Label("Benchmark", systemImage: "speedometer") }
                BackgroundTestView()
                    .tabItem { Label("Background", systemImage: "moon.zzz") }
            }
        }
    }
}
