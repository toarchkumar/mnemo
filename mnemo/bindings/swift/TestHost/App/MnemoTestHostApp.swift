import Foundation
import Mnemo
import SwiftUI

/// Hosts the package's tests so they get an app's Keychain entitlements.
///
/// Launched with `-mnemoDurability write` or `-mnemoDurability count`, it
/// runs a step of the kill test instead (``DurabilityTest``, driven by
/// `scripts/kill-test.sh`).
@main
struct MnemoTestHostApp: App {
    init() {
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: "-mnemoDurability"), i + 1 < args.count {
            let mode = args[i + 1]
            Task.detached { await DurabilityTest.run(mode) }
        }
    }

    var body: some Scene {
        WindowGroup {
            Text("MNemo test host")
        }
    }
}

/// The kill test for the default flush policy (MOBILE-2b):
///
/// 1. `write`: a fresh store with the default policy (coalesced, about 1 s)
///    gets ``settled`` writes, waits past the window and prints
///    `MNEMO_DURABILITY settled`, then makes ``late`` more writes and prints
///    `MNEMO_DURABILITY late`, with nothing flushed by hand.
/// 2. The script kills the app (`simctl terminate`): no lifecycle event, no
///    flush.
/// 3. `count`: reopens the file and prints `MNEMO_DURABILITY count=N`. The
///    settled writes, older than the window, must be there.
enum DurabilityTest {
    static let settled = 20
    static let late = 5

    static func say(_ line: String) {
        print(line)
        fflush(stdout)
    }

    static func run(_ mode: String) async {
        do {
            let dir = try MnemoStoreConfiguration.defaultDirectory().appendingPathComponent("durability")
            let config = MnemoStoreConfiguration(name: "durability", dimensions: 4, directory: dir)
            func vector(_ i: Int) -> [Float] { [Float(i) + 1, Float(i % 3), 0.5, 1] }
            switch mode {
            case "write":
                if let old = try? MnemoStore(configuration: config) {
                    try? FileManager.default.removeItem(at: old.fileURL)
                }
                let store = try MnemoStore(configuration: config)
                for i in 0..<settled { try await store.remember("settled \(i)", vector: vector(i)) }
                try await Task.sleep(nanoseconds: 2_500_000_000)
                say("MNEMO_DURABILITY settled \(settled)")
                for i in 0..<late { try await store.remember("late \(i)", vector: vector(settled + i)) }
                say("MNEMO_DURABILITY late \(late)")
            case "count":
                let store = try MnemoStore(configuration: config)
                let n = try await store.count()
                try await store.close()
                say("MNEMO_DURABILITY count=\(n)")
            default:
                say("MNEMO_DURABILITY failed: unknown mode \(mode)")
            }
        } catch {
            say("MNEMO_DURABILITY failed: \(error)")
        }
    }
}
