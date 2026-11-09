import ObjectiveC
import Tauri
import UIKit
import WebKit

class FinishArgs: Decodable {
    let id: UInt64
    let result: String
}

/// The plugin's own trail, in the app's temporary directory, which the CI
/// check reads from the simulator's container.
private let trail = FileManager.default.temporaryDirectory
    .appendingPathComponent("device-push.log")

/// Writes each step to the system log, with a prefix to search for, and
/// to the trail.
private func say(_ message: String) {
    NSLog("[device.push] %@", message)
    let line = Data("\(message)\n".utf8)
    if let handle = try? FileHandle(forWritingTo: trail) {
        handle.seekToEndOfFile()
        handle.write(line)
        handle.closeFile()
    } else {
        try? line.write(to: trail)
    }
}

/// APNs registration and content-free background wakes (ADR 0030 §5).
///
/// iOS reports both through the app delegate, which the app shell does not
/// own, so the plugin adds the three callbacks to the delegate's class when
/// it loads, keeping any the delegate already has. Each callback becomes an
/// event the app reads with `next`; a wake stays open until the app calls
/// `finish`, which completes it for iOS.
class PushPlugin: Plugin {
    static weak var shared: PushPlugin?

    private let lock = NSLock()
    private var events: [JsonObject] = []
    private var waiting: Invoke?
    /// The completion handlers each wake answers; pushes that arrive while
    /// a wake is still queued join it.
    private var wakes: [UInt64: [(UIBackgroundFetchResult) -> Void]] = [:]
    /// The wake queued and not yet read, if any.
    private var queuedWake: UInt64?
    private var nextWake: UInt64 = 1

    /// Installs the delegate's callbacks and registers as soon as the
    /// plugin exists, without waiting for the webview: a push can wake the
    /// app before any page loads.
    override init() {
        super.init()
        PushPlugin.shared = self
        DispatchQueue.main.async {
            guard let delegate = UIApplication.shared.delegate else {
                say("no app delegate to receive pushes")
                return
            }
            PushPlugin.install(on: type(of: delegate))
            say("listening for pushes")
            UIApplication.shared.registerForRemoteNotifications()
            #if DEBUG && targetEnvironment(simulator)
                PushPlugin.watchForSimulatedPushes(delegate)
            #endif
        }
    }

    #if DEBUG && targetEnvironment(simulator)
        /// The simulator hands pushes to an app only once APNs has given it
        /// a token, which a CI runner never gets. In debug simulator builds
        /// the CI check asks for a push by creating `simulate-push` in the
        /// app's temporary directory, and the plugin delivers it through
        /// the delegate's callback, as iOS would.
        private static func watchForSimulatedPushes(_ delegate: UIApplicationDelegate) {
            let request = FileManager.default.temporaryDirectory
                .appendingPathComponent("simulate-push")
            Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in
                guard (try? FileManager.default.removeItem(at: request)) != nil else { return }
                say("delivering a simulated push")
                delegate.application?(
                    UIApplication.shared,
                    didReceiveRemoteNotification: ["aps": ["content-available": 1]],
                    fetchCompletionHandler: { result in
                        say("iOS told: \(result.rawValue)")
                    })
            }
        }
    #endif

    /// Answers with the next event, now or when one arrives.
    @objc public func next(_ invoke: Invoke) {
        lock.lock()
        if events.isEmpty {
            waiting?.reject("another reader took over")
            waiting = invoke
            lock.unlock()
        } else {
            let event = events.removeFirst()
            if let value = event["id"], let id = value as? UInt64, id == queuedWake {
                queuedWake = nil
            }
            lock.unlock()
            invoke.resolve(event)
        }
    }

    /// Completes a wake for iOS.
    @objc public func finish(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(FinishArgs.self)
        lock.lock()
        let completions = wakes.removeValue(forKey: args.id) ?? []
        lock.unlock()
        let result: UIBackgroundFetchResult
        switch args.result {
        case "new_data": result = .newData
        case "no_data": result = .noData
        default: result = .failed
        }
        say("wake finished: \(args.result)")
        DispatchQueue.main.async { completions.forEach { $0(result) } }
        invoke.resolve()
    }

    private func emit(_ event: JsonObject) {
        lock.lock()
        if let invoke = waiting {
            waiting = nil
            lock.unlock()
            invoke.resolve(event)
        } else {
            events.append(event)
            lock.unlock()
        }
    }

    func registered(_ token: Data) {
        let hex = token.map { String(format: "%02x", $0) }.joined()
        say("registered for remote notifications")
        emit(["kind": "token", "token": hex])
    }

    func failed(_ error: Error) {
        say("remote notifications unavailable: \(error.localizedDescription)")
        emit(["kind": "unavailable", "message": error.localizedDescription])
    }

    func woke(_ completion: @escaping (UIBackgroundFetchResult) -> Void) {
        lock.lock()
        if let id = queuedWake {
            // One sync answers both pushes, within the first one's window.
            wakes[id, default: []].append(completion)
            lock.unlock()
            say("woken by a push, joining the queued wake")
            return
        }
        let id = nextWake
        nextWake += 1
        wakes[id] = [completion]
        // When iOS delivered it: the app's time to answer runs from here,
        // however long the event waits to be read.
        let event: JsonObject = [
            "kind": "wake", "id": id, "received_at": Date().timeIntervalSince1970,
        ]
        say("woken by a push")
        if let invoke = waiting {
            waiting = nil
            lock.unlock()
            invoke.resolve(event)
        } else {
            // Only a wake still waiting to be read can take later pushes.
            events.append(event)
            queuedWake = id
            lock.unlock()
        }
    }

    // MARK: - The delegate's callbacks

    private typealias Registered = @convention(c) (AnyObject, Selector, UIApplication, Data) -> Void
    private typealias Failed = @convention(c) (AnyObject, Selector, UIApplication, Error) -> Void
    private typealias Received = @convention(c) (
        AnyObject, Selector, UIApplication, NSDictionary,
        @escaping @convention(block) (UIBackgroundFetchResult) -> Void
    ) -> Void

    private static var installed = false

    /// Adds the callbacks to `cls`, calling the delegate's own first where
    /// it has one.
    static func install(on cls: AnyClass) {
        guard !installed else { return }
        installed = true

        let registered = #selector(
            UIApplicationDelegate.application(_:didRegisterForRemoteNotificationsWithDeviceToken:))
        let previousRegistered = original(cls, registered, as: Registered.self)
        let registeredBlock: @convention(block) (AnyObject, UIApplication, Data) -> Void = {
            object, application, token in
            previousRegistered?(object, registered, application, token)
            PushPlugin.shared?.registered(token)
        }
        put(cls, registered, imp_implementationWithBlock(registeredBlock), "v@:@@")

        let failed = #selector(
            UIApplicationDelegate.application(_:didFailToRegisterForRemoteNotificationsWithError:))
        let previousFailed = original(cls, failed, as: Failed.self)
        let failedBlock: @convention(block) (AnyObject, UIApplication, Error) -> Void = {
            object, application, error in
            previousFailed?(object, failed, application, error)
            PushPlugin.shared?.failed(error)
        }
        put(cls, failed, imp_implementationWithBlock(failedBlock), "v@:@@")

        let received = #selector(
            UIApplicationDelegate.application(_:didReceiveRemoteNotification:fetchCompletionHandler:))
        let previousReceived = original(cls, received, as: Received.self)
        let receivedBlock:
            @convention(block) (
                AnyObject, UIApplication, NSDictionary,
                @escaping @convention(block) (UIBackgroundFetchResult) -> Void
            ) -> Void = { object, application, payload, completion in
                guard let plugin = PushPlugin.shared else {
                    if let previous = previousReceived {
                        previous(object, received, application, payload, completion)
                    } else {
                        completion(.noData)
                    }
                    return
                }
                // The notification carries no data the app reads; it only
                // says the workspace changed (ADR 0030 §1).
                plugin.woke(completion)
            }
        put(cls, received, imp_implementationWithBlock(receivedBlock), "v@:@@@?")
    }

    private static func original<T>(_ cls: AnyClass, _ selector: Selector, as _: T.Type) -> T? {
        guard let method = class_getInstanceMethod(cls, selector) else { return nil }
        return unsafeBitCast(method_getImplementation(method), to: T.self)
    }

    private static func put(_ cls: AnyClass, _ selector: Selector, _ imp: IMP, _ types: String) {
        if !class_addMethod(cls, selector, imp, types),
            let method = class_getInstanceMethod(cls, selector)
        {
            method_setImplementation(method, imp)
        }
    }
}

@_cdecl("init_plugin_device_push")
func initPlugin() -> Plugin {
    return PushPlugin()
}
