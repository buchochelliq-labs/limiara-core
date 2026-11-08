import ObjectiveC
import Tauri
import UIKit
import WebKit
import os.log

class FinishArgs: Decodable {
    let id: UInt64
    let result: String
}

private let log = OSLog(subsystem: "device.push", category: "push")

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
    private var wakes: [UInt64: (UIBackgroundFetchResult) -> Void] = [:]
    private var nextWake: UInt64 = 1

    @objc public override func load(webview: WKWebView) {
        PushPlugin.shared = self
        DispatchQueue.main.async {
            if let delegate = UIApplication.shared.delegate {
                PushPlugin.install(on: type(of: delegate))
            }
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    /// Answers with the next event, now or when one arrives.
    @objc public func next(_ invoke: Invoke) {
        lock.lock()
        if events.isEmpty {
            waiting?.reject("another reader took over")
            waiting = invoke
            lock.unlock()
        } else {
            let event = events.removeFirst()
            lock.unlock()
            invoke.resolve(event)
        }
    }

    /// Completes a wake for iOS.
    @objc public func finish(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(FinishArgs.self)
        lock.lock()
        let completion = wakes.removeValue(forKey: args.id)
        lock.unlock()
        let result: UIBackgroundFetchResult
        switch args.result {
        case "new_data": result = .newData
        case "no_data": result = .noData
        default: result = .failed
        }
        os_log("wake finished: %{public}@", log: log, type: .default, args.result)
        DispatchQueue.main.async { completion?(result) }
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
        os_log("registered for remote notifications", log: log, type: .default)
        emit(["kind": "token", "token": hex])
    }

    func failed(_ error: Error) {
        os_log(
            "remote notifications unavailable: %{public}@", log: log, type: .default,
            error.localizedDescription)
        emit(["kind": "unavailable", "message": error.localizedDescription])
    }

    func woke(_ completion: @escaping (UIBackgroundFetchResult) -> Void) {
        lock.lock()
        let id = nextWake
        nextWake += 1
        wakes[id] = completion
        lock.unlock()
        os_log("woken by a push", log: log, type: .default)
        emit(["kind": "wake", "id": id])
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
