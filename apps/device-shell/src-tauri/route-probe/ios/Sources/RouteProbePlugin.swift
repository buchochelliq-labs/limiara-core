import Tauri
import UIKit

class RouteArgs: Decodable {
    let pattern: String?
    let objectName: String?
    let recordId: String?
}

class NoteArgs: Decodable {
    let message: String
}

/// The plugin's trail, in the app's temporary directory, which the CI check
/// reads from the simulator's container.
private let trail = FileManager.default.temporaryDirectory
    .appendingPathComponent("device-route-probe.log")

/// Routes and the shell's notes arrive on different threads; one queue
/// writes the trail, so a line never lands on another's offset.
private let trailQueue = DispatchQueue(label: "device.route-probe.trail")

/// Writes each step to the system log, with a prefix to search for, and to
/// the trail, which only this plugin writes.
private func say(_ message: String) {
    NSLog("[device.route-probe] %@", message)
    let line = Data("\(message)\n".utf8)
    trailQueue.sync {
        if let handle = try? FileHandle(forWritingTo: trail) {
            handle.seekToEndOfFile()
            handle.write(line)
            handle.closeFile()
        } else {
            try? line.write(to: trail)
        }
    }
}

/// A test plugin for the current-route context (ADR 0034 §9): it receives
/// each route the web shell reports, as a route pattern's name and ids, and
/// records it. It reads nothing itself and holds no business logic; an
/// on-screen awareness plugin would hand the same ids to `NSUserActivity`.
class RouteProbePlugin: Plugin {
    override init() {
        super.init()
        say("listening for routes")
    }

    /// Takes one route from the shell.
    @objc public func route(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(RouteArgs.self)
        say("route: \(args.pattern ?? "none") \(args.objectName ?? "-") \(args.recordId ?? "-")")
        invoke.resolve()
    }

    /// Adds one of the shell's lines to the trail, such as the outcome of a
    /// simulated route.
    @objc public func note(_ invoke: Invoke) throws {
        let args = try invoke.parseArgs(NoteArgs.self)
        say(args.message)
        invoke.resolve()
    }

    /// Where the plugin keeps its trail, so the shell's debug check can ask
    /// for a simulated route there.
    @objc public func directory(_ invoke: Invoke) {
        invoke.resolve(["path": FileManager.default.temporaryDirectory.path])
    }
}

@_cdecl("init_plugin_device_route_probe")
func initPlugin() -> Plugin {
    return RouteProbePlugin()
}
