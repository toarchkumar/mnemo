import Foundation
#if canImport(UIKit) && !os(watchOS)
import UIKit
#endif

/// Tracks whether the app is in the foreground, and closes the store when
/// it leaves.
///
/// - `didEnterBackgroundNotification` and `willTerminateNotification` mark
///   the app as not active and call the close handler inside an expiring
///   activity, so the process keeps running until the handler returns.
/// - `willEnterForegroundNotification` and `didBecomeActiveNotification`
///   mark it active again.
/// - The starting state is read from `UIApplication` once, on the main
///   thread: an app launched straight into the background (a background
///   fetch, a push) posts no `didEnterBackground`, so a store made then
///   must still know it isn't active.
///
/// The flag is set synchronously in the notification handlers, so a call
/// made after the notification is posted always sees the new state.
///
/// `ProcessInfo.performExpiringActivity` is used rather than
/// `UIApplication.beginBackgroundTask` because it also works in app
/// extensions (App Group containers) and in test processes, where
/// `UIApplication.shared` isn't available. Both ask the system for the same
/// extra background time.
final class LifecycleObserver: @unchecked Sendable {
    /// The notifications that close the store.
    static var closeNotifications: [Notification.Name] {
        #if canImport(UIKit) && !os(watchOS)
        return [UIApplication.didEnterBackgroundNotification, UIApplication.willTerminateNotification]
        #else
        return []
        #endif
    }

    /// The notifications that mark the app active again.
    static var foregroundNotifications: [Notification.Name] {
        #if canImport(UIKit) && !os(watchOS)
        return [UIApplication.willEnterForegroundNotification, UIApplication.didBecomeActiveNotification]
        #else
        return []
        #endif
    }

    private let lock = NSLock()
    private var tokens: [NSObjectProtocol] = []
    private var foreground = true
    private var notified = false

    /// Whether the app is in the foreground. Always `true` where there are
    /// no lifecycle notifications (macOS), and in an app extension, which
    /// gets no `UIApplication` notifications.
    var isForeground: Bool {
        lock.lock()
        defer { lock.unlock() }
        return foreground
    }

    /// Sets the state from a notification (or a test).
    func setForeground(_ value: Bool) {
        lock.lock()
        foreground = value
        notified = true
        lock.unlock()
    }

    /// Takes the starting state, unless a notification has already set one.
    /// Returns whether it was taken.
    private func adoptStartingState(foreground value: Bool) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard !notified else { return false }
        foreground = value
        return true
    }

    /// Starts observing. `close` runs on a background thread inside an
    /// expiring activity, and must finish before it returns. A no-op where
    /// there are no lifecycle notifications (macOS).
    func start(reason: String, close: @escaping @Sendable () async -> Void) {
        #if canImport(UIKit) && !os(watchOS)
        let center = NotificationCenter.default
        var new = Self.closeNotifications.map { name in
            center.addObserver(forName: name, object: nil, queue: nil) { [weak self] _ in
                self?.setForeground(false)
                LifecycleObserver.runExpiring(reason: reason, work: close)
            }
        }
        new += Self.foregroundNotifications.map { name in
            center.addObserver(forName: name, object: nil, queue: nil) { [weak self] _ in
                self?.setForeground(true)
            }
        }
        lock.lock()
        tokens = new
        lock.unlock()

        let readStartingState: @Sendable () -> Void = { [weak self] in
            guard let self, let state = Self.applicationState() else { return }
            let inBackground = state == .background
            if self.adoptStartingState(foreground: !inBackground), inBackground {
                LifecycleObserver.runExpiring(reason: reason, work: close)
            }
        }
        if Thread.isMainThread {
            readStartingState()
        } else {
            DispatchQueue.main.async(execute: readStartingState)
        }
        #endif
    }

    deinit {
        for token in tokens {
            NotificationCenter.default.removeObserver(token)
        }
    }

    #if canImport(UIKit) && !os(watchOS)
    /// The app's state, or `nil` outside an app (an extension, a tool).
    /// Read through KVC so the package still links into app extensions,
    /// where `UIApplication.shared` is unavailable. Main thread only.
    static func applicationState() -> UIApplication.State? {
        guard Bundle.main.bundleURL.pathExtension == "app" else { return nil }
        return (UIApplication.value(forKey: "sharedApplication") as? UIApplication)?.applicationState
    }

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

    /// Holds an expiring activity open until ``ExpiringActivity/end()``: the
    /// process isn't suspended while a call made in the background runs and
    /// closes the file. Where there are no expiring activities (macOS), it
    /// holds nothing.
    ///
    /// Throws ``MnemoStoreError/noBackgroundTime`` when the system has no
    /// time left (its first callback says `expired`): the caller must not
    /// open the file then (v0.12.2, RB-001.20).
    static func beginActivity(reason: String) async throws -> ExpiringActivity {
        let activity = ExpiringActivity()
        #if canImport(UIKit) && !os(watchOS)
        let granted = await withCheckedContinuation { (started: CheckedContinuation<Bool, Never>) in
            let first = Once()
            ProcessInfo.processInfo.performExpiringActivity(withReason: reason) { expired in
                if first.claim() {
                    started.resume(returning: !expired)
                    if !expired { activity.wait() }
                } else if expired {
                    // Time ran out while the call held the activity: later
                    // steps of a long call check this.
                    activity.markExpired()
                }
            }
        }
        if !granted { throw MnemoStoreError.noBackgroundTime }
        #endif
        return activity
    }
}

/// An expiring activity held open by a waiting thread.
final class ExpiringActivity: @unchecked Sendable {
    private let done = DispatchSemaphore(value: 0)
    private let ended = Once()

    private let expiredFlag = Once()

    fileprivate func wait() {
        done.wait()
    }

    fileprivate func markExpired() {
        _ = expiredFlag.claim()
    }

    /// Whether the system said the time ran out while the activity was held.
    var hasExpired: Bool { !expiredFlag.claimable }

    /// Releases the activity. Safe to call more than once.
    func end() {
        if ended.claim() { done.signal() }
    }
}

/// A flag that can be claimed once, from any thread.
final class Once: @unchecked Sendable {
    private let lock = NSLock()
    private var claimed = false

    /// Whether ``claim()`` would still succeed.
    var claimable: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !claimed
    }

    func claim() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if claimed { return false }
        claimed = true
        return true
    }
}
