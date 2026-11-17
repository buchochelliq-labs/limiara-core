package device.routeprobe

import android.app.Activity
import android.util.Log
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class RouteArgs {
    var pattern: String? = null
    var objectName: String? = null
    var recordId: String? = null
}

/**
 * A test plugin for the current-route context (ADR 0034 §9): it receives
 * each route the web shell reports, as a route pattern's name and ids, and
 * records it in a trail in the app's cache directory, which the CI check
 * reads with `run-as`. It reads nothing itself and holds no business logic;
 * an on-screen awareness plugin would hand the same ids to the agent's view
 * of the current screen.
 */
@TauriPlugin
class RouteProbePlugin(private val activity: Activity) : Plugin(activity) {
    private val trail = File(activity.cacheDir, "device-route-probe.log")

    init {
        say("listening for routes")
    }

    @Command
    fun route(invoke: Invoke) {
        val args = invoke.parseArgs(RouteArgs::class.java)
        say("route: ${args.pattern ?: "none"} ${args.objectName ?: "-"} ${args.recordId ?: "-"}")
        invoke.resolve(JSObject())
    }

    /** Where the plugin keeps its trail. */
    @Command
    fun directory(invoke: Invoke) {
        invoke.resolve(JSObject().put("path", activity.cacheDir.absolutePath))
    }

    private fun say(message: String) {
        Log.i("device.route-probe", message)
        try {
            trail.appendText("$message\n")
        } catch (error: Exception) {
            Log.w("device.route-probe", "the trail could not be written: ${error.message}")
        }
    }
}
