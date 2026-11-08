package device.browser

import android.app.Activity
import android.content.Intent
import android.os.Bundle

/**
 * Opened by the browser's redirect to the app's scheme. It hands the
 * redirect to the waiting sign-in, then brings the app back over the
 * browser tab. A redirect no sign-in is waiting for is dropped.
 */
class RedirectActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        intent?.data?.let { Pending.deliver(it) }
        packageManager.getLaunchIntentForPackage(packageName)?.let { launch ->
            launch.addFlags(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP)
            startActivity(launch)
        }
        finish()
    }
}
