package device.browser

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.Intent
import android.net.Uri
import androidx.appcompat.app.AppCompatActivity
import androidx.browser.customtabs.CustomTabsIntent
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

@InvokeArg
class AuthenticateArgs {
    lateinit var url: String
    lateinit var callbackScheme: String
}

/**
 * Sign-in in a Custom Tab (RFC 8252): the browser keeps the member's
 * existing sign-in at the identity provider, and the app never sees their
 * credentials. [RedirectActivity] receives the redirect to the app's scheme.
 */
@TauriPlugin
class BrowserPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun authenticate(invoke: Invoke) {
        val args = invoke.parseArgs(AuthenticateArgs::class.java)
        val uri = Uri.parse(args.url)
        if (uri.scheme != "https" && uri.scheme != "http") {
            invoke.reject("the sign-in address is not a web address")
            return
        }
        synchronized(Pending) {
            Pending.invoke?.reject("another sign-in started")
            Pending.invoke = invoke
            Pending.scheme = args.callbackScheme
            Pending.left = false
        }
        try {
            CustomTabsIntent.Builder().build().launchUrl(activity, uri)
        } catch (error: ActivityNotFoundException) {
            try {
                activity.startActivity(Intent(Intent.ACTION_VIEW, uri))
            } catch (error: ActivityNotFoundException) {
                Pending.fail("no browser is installed")
            }
        }
    }

    override fun onPause(activity: AppCompatActivity) {
        synchronized(Pending) {
            if (Pending.invoke != null) Pending.left = true
        }
    }

    /** Back in the app with no redirect: the member closed the browser. */
    override fun onResume(activity: AppCompatActivity) {
        synchronized(Pending) {
            if (Pending.left) Pending.fail("the sign-in was cancelled")
        }
    }
}

/** The one sign-in waiting for its redirect. */
internal object Pending {
    var invoke: Invoke? = null
    var scheme: String? = null
    var left = false

    @Synchronized
    fun deliver(uri: Uri): Boolean {
        val waiting = invoke ?: return false
        if (uri.scheme != scheme) return false
        invoke = null
        left = false
        waiting.resolve(JSObject().put("url", uri.toString()))
        return true
    }

    @Synchronized
    fun fail(message: String) {
        val waiting = invoke ?: return
        invoke = null
        left = false
        waiting.reject(message)
    }
}
