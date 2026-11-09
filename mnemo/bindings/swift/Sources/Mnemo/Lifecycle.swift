import Foundation
#if canImport(UIKit) && !os(watchOS)
import UIKit
#endif

/// Observes the app moving to the background or terminating, and calls the
/// handler inside an expiring activity, so the process keeps running until
/// the handler returns.
///
/// `ProcessInfo.performExpiringActivity` is used rather than
/// `UIApplication.beginBackgroundTask` because it also works in app
/// extensions (App Group containers) and in test processes, where
/// `UIApplication.shared` isn't available. Both ask the system for the same
/// extra background time.
final class LifecycleObserver: @unchecked Sendable {
    /// The notifications that close the store.
    static var notificationNames: [Notification.Name] {
        #if canImport(UIKit) && !os(watchOS)
        return [UIApplication.didEnterBackgroundNotification, UIApplication.willTerminateNotification]
        #else
        return []
        #endif
    }

    private let lock = NSLock()
    private var tokens: [NSObjectProtocol] = []

    /// Starts observing. `work` runs on a background thread and must finish
    /// before it returns. A no-op where there are no lifecycle notifications
    /// (macOS).
    func start(reason: String, work: @escaping @Sendable () async -> Void) {
        #if canImport(UIKit) && !os(watchOS)
        let center = NotificationCenter.default
        let new = Self.notificationNames.map { name in
            center.addObserver(forName: name, object: nil, queue: nil) { _ in
                LifecycleObserver.runExpiring(reason: reason, work: work)
            }
        }
        lock.lock()
        tokens = new
        lock.unlock()
        #endif
    }

    deinit {
        for token in tokens {
            NotificationCenter.default.removeObserver(token)
        }
    }

    #if canImport(UIKit) && !os(watchOS)
    static func runExpiring(reason: String, work: @escaping @Sendable () async -> Void) {
        let started = Once()
        ProcessInfo.processInfo.performExpiringActivity(withReason: reason) { _ in
            // The block is called a second time, with `expired` set, when time
            // runs out while the first call is still working; that call has
            // nothing left to do. A first call that is already expired still
            // tries: a flush is short.
            guard started.claim() else { return }
            let done = DispatchSemaphore(value: 0)
            Task.detached {
                await work()
                done.signal()
            }
            done.wait()
        }
    }
    #endif
}

/// A flag that can be claimed once, from any thread.
final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if claimed { return false }
        claimed = true
        return true
    }
}
