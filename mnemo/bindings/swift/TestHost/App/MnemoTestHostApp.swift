import SwiftUI

/// Hosts the package's tests so they get an app's Keychain entitlements.
@main
struct MnemoTestHostApp: App {
    var body: some Scene {
        WindowGroup {
            Text("MNemo test host")
        }
    }
}
