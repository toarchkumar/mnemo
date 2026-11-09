import Mnemo
import SwiftUI

/// The iOS release check (design note Q1): a store in the App Group
/// container, a write loop, then the app in the background for more than
/// 30 seconds. iOS kills an app that's suspended holding a lock on a file in
/// a shared container, with the exception code `0xdead10cc`; `MnemoStore`
/// closes its file when the app enters the background so that never happens.
/// The README says how to read the device logs afterwards.
@MainActor
final class BackgroundTest: ObservableObject {
    static let dimensions: UInt32 = 16

    @Published private(set) var running = false
    /// The second mode: the loop keeps writing after the app leaves the
    /// foreground, so each write is a background call that reopens the
    /// file and closes it again (inside an expiring activity).
    @Published var keepWritingInBackground = false
    @Published private(set) var writes = 0
    @Published private(set) var stored: UInt64 = 0
    @Published private(set) var location = ""
    @Published private(set) var fileProtection = "(no file yet)"
    @Published private(set) var directoryProtection = "(no directory yet)"
    @Published private(set) var events: [String] = []
    @Published private(set) var error: String?

    private var store: MnemoStore?
    private var loop: Task<Void, Never>?
    private var backgroundAt: Date?
    private let clock = DateFormatter()

    init() {
        clock.dateFormat = "HH:mm:ss"
        do {
            let (directory, label) = Self.directory()
            location = label
            store = try MnemoStore(configuration: MnemoStoreConfiguration(
                name: "background-test", dimensions: Self.dimensions, directory: directory))
        } catch {
            self.error = "\(error)"
        }
    }

    /// `group.<bundle id>`, the group the entitlements file names.
    static var appGroup: String { "group.\(Bundle.main.bundleIdentifier ?? "")" }

    private static func directory() -> (URL?, String) {
        if let url = try? MnemoStoreConfiguration.appGroupDirectory(appGroup) {
            return (url, "App Group container (\(appGroup))")
        }
        return (nil, "Application Support: no App Group container (unsigned build, or the App Group isn't enabled for this team). The test still runs but is weaker; see the README.")
    }

    func start() {
        guard let store, loop == nil else { return }
        running = true
        log("write loop started")
        loop = Task {
            var i = 0
            while !Task.isCancelled {
                do {
                    let v = (0..<Int(Self.dimensions)).map { Float(($0 * 7 + i) % 13) + 1 }
                    try await store.remember("background test write \(i) at \(Date())", vector: v)
                    i += 1
                    writes += 1
                    if i % 20 == 0 { try await store.flush() }
                } catch {
                    self.error = "\(error)"
                }
                try? await Task.sleep(nanoseconds: 100_000_000)
            }
        }
        Task { await readProtection() }
    }

    func stop() {
        loop?.cancel()
        loop = nil
        running = false
        log("write loop stopped")
    }

    /// The scene phase drives the loop, as a real app's work would: it stops
    /// before the app leaves the foreground (`inactive` comes before
    /// `background`), so no call reopens the file during the background
    /// transition, and it resumes when the app is active again.
    func phaseChanged(_ phase: ScenePhase) {
        switch phase {
        case .active:
            if let at = backgroundAt {
                log(String(format: "back in the foreground after %.0f s", Date().timeIntervalSince(at)))
                backgroundAt = nil
                Task { await verify() }
            }
        case .inactive:
            if loop != nil, !keepWritingInBackground {
                loop?.cancel()
                loop = nil
                log("inactive: write loop paused")
            }
        case .background:
            backgroundAt = Date()
            log(keepWritingInBackground
                ? "background: still writing; each write reopens and closes the file"
                : "background: MnemoStore flushes and closes the file")
        @unknown default:
            break
        }
        if phase == .active, running, loop == nil {
            running = false
            start()
        }
    }

    /// After the return: every write is stored, and no background close
    /// failed.
    func verify() async {
        guard let store else { return }
        do {
            stored = try await store.count()
                if let bgError = await store.lastBackgroundError {
                log("background close error: \(bgError)")
            }
            log("stored \(stored), wrote \(writes)\(stored >= UInt64(writes) ? "" : ": WRITES MISSING")")
        } catch {
            self.error = "\(error)"
        }
        await readProtection()
    }

    /// The protection class iOS reports for the file and its directory. On a
    /// device it's `completeUntilFirstUserAuthentication`; the simulator has
    /// no Data Protection and reports none.
    func readProtection() async {
        guard let store else { return }
        _ = try? await store.count()
        let fm = FileManager.default
        func protection(_ path: String) -> String {
            guard let attrs = try? fm.attributesOfItem(atPath: path) else { return "(unreadable)" }
            return (attrs[.protectionKey] as? FileProtectionType)?.rawValue ?? "none reported"
        }
        fileProtection = protection(store.fileURL.path)
        directoryProtection = protection(store.fileURL.deletingLastPathComponent().path)
    }

    private func log(_ message: String) {
        events.insert("\(clock.string(from: Date()))  \(message)", at: 0)
        if events.count > 50 { events.removeLast() }
    }
}

struct BackgroundTestView: View {
    @StateObject private var test = BackgroundTest()
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        NavigationStack {
            List {
                Section {
                    Text(test.location).font(.footnote)
                    LabeledContent("Writes", value: "\(test.writes)")
                    LabeledContent("Stored (after return)", value: "\(test.stored)")
                    if let error = test.error { Text(error).font(.footnote).foregroundStyle(.red) }
                    Toggle("Keep writing in the background", isOn: $test.keepWritingInBackground)
                        .disabled(test.running)
                    Button(test.running ? "Stop the write loop" : "Start the write loop") {
                        test.running ? test.stop() : test.start()
                    }
                }
                Section("File protection") {
                    LabeledContent("File", value: test.fileProtection)
                    LabeledContent("Directory", value: test.directoryProtection)
                    Button("Read again") { Task { await test.readProtection() } }
                }
                Section("Steps") {
                    Text("1. Choose the mode, then start the write loop.\n2. Go to the Home Screen and lock the phone.\n3. Wait at least 30 seconds (a minute is better).\n4. Come back: the app should still be running (no relaunch), and the log shows \"stored N, wrote N\".\n5. Check the device logs for 0xdead10cc (see the README).\n6. Run it again in the other mode.")
                        .font(.footnote)
                }
                Section("Log") {
                    ForEach(test.events, id: \.self) { Text($0).font(.caption.monospaced()) }
                }
            }
            .navigationTitle("Background test")
        }
        .onChange(of: scenePhase) { test.phaseChanged($0) }
    }
}
