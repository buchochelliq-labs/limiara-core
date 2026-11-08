//! Phone sign-in in the system browser (RFC 8252; ADR 0029 §7, "Phone
//! sign-in").
//!
//! - iOS: `ASWebAuthenticationSession`, which shares the system browser's
//!   sign-in and hands back the redirect to the app's private-use scheme.
//! - Android: a Custom Tab, or the default browser where no browser offers
//!   Custom Tabs. The redirect to `<application id>:/…` opens the plugin's
//!   redirect activity, which hands it back and returns to the app. Coming
//!   back to the app without a redirect cancels the sign-in.
//!
//! Elsewhere the plugin registers nothing, and `Browser` does not exist.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// The plugin's name; Tauri derives the `links` key from it.
pub const NAME: &str = "device-browser";

/// The platform's message when a sign-in did not come back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserError(pub String);

impl std::fmt::Display for BrowserError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for BrowserError {}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile {
    use super::BrowserError;
    use serde::{Deserialize, Serialize};
    use tauri::Runtime;

    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Authenticate<'a> {
        url: &'a str,
        callback_scheme: &'a str,
    }

    #[derive(Deserialize)]
    struct Redirect {
        url: String,
    }

    /// The system browser for sign-in.
    pub struct Browser<R: Runtime>(pub(crate) tauri::plugin::PluginHandle<R>);

    impl<R: Runtime> std::fmt::Debug for Browser<R> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Browser")
        }
    }

    impl<R: Runtime> Clone for Browser<R> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<R: Runtime> Browser<R> {
        /// Opens `url` and blocks until the browser redirects to
        /// `callback_scheme` (the URL it redirected to) or the member
        /// abandons the sign-in.
        pub fn authenticate(
            &self,
            url: &str,
            callback_scheme: &str,
        ) -> Result<String, BrowserError> {
            self.0
                .run_mobile_plugin::<Redirect>(
                    "authenticate",
                    Authenticate {
                        url,
                        callback_scheme,
                    },
                )
                .map(|redirect| redirect.url)
                .map_err(|error| BrowserError(error.to_string()))
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::Browser;

/// The Swift entry point the iOS half exports. Tauri's binding declares it
/// through FFI, the crate's only `unsafe` (ADR 0029 §7).
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod ios_binding {
    tauri::ios_plugin_binding!(init_plugin_device_browser);

    /// The generated binding is private to this module; Tauri calls it.
    pub(super) fn entry_point() -> unsafe fn() -> *const std::ffi::c_void {
        init_plugin_device_browser
    }
}

/// The plugin. On a phone it registers the native half and manages a
/// `Browser`; elsewhere it does nothing.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new(NAME)
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin("device.browser", "BrowserPlugin")?;
                _app.manage(mobile::Browser(handle));
            }
            #[cfg(target_os = "ios")]
            {
                use tauri::Manager;
                let handle = _api.register_ios_plugin(ios_binding::entry_point())?;
                _app.manage(mobile::Browser(handle));
            }
            Ok(())
        })
        .build()
}
