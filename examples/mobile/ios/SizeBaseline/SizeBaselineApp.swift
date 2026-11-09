import SwiftUI

/// SizeProbe without MNemo: the baseline for the size report.
@main
struct SizeBaselineApp: App {
    @State private var status = "MNemo size probe"

    var body: some Scene {
        WindowGroup {
            Text(status).task {
                status = "0 hit"
            }
        }
    }
}
