//! Push registration and background wakes (ADR 0030 §5).
//!
//! On iOS the Swift half adds the APNs callbacks to the app's delegate,
//! registers for remote notifications, and turns each event into one the
//! app reads with [`Push::next`]:
//! - [`Event::Token`]: the APNs device token, for `register_push`;
//! - [`Event::Wake`]: a content-free push woke the app; the app syncs and
//!   answers with [`Push::finish`], which completes the push for iOS;
//! - [`Event::Unavailable`]: iOS refused registration, as it does for a
//!   build without the `aps-environment` entitlement.
//!
//! Elsewhere the plugin registers nothing, and `Push` does not exist.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// The plugin's name; Tauri derives the `links` key from it.
pub const NAME: &str = "device-push";

/// The platform's message when the plugin could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushError(pub String);

impl std::fmt::Display for PushError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for PushError {}

/// What the platform reported.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// The device token, hex encoded.
    Token { token: String },
    /// A push woke the app; answer with [`Push::finish`] and this id.
    /// `received_at` is when iOS delivered it, in seconds since the Unix
    /// epoch: the time iOS allows runs from then.
    Wake { id: u64, received_at: f64 },
    /// Registration failed; the app syncs only when opened.
    Unavailable { message: String },
}

/// How a wake ended, as iOS's background fetch result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Finished {
    /// The sync brought something in.
    NewData,
    /// Nothing to do: no workspace, or it was wiped.
    NoData,
    /// The sync did not finish in time or failed.
    Failed,
}

#[cfg(target_os = "ios")]
mod ios {
    use super::{Event, Finished, PushError};
    use serde::{Deserialize, Serialize};
    use tauri::Runtime;

    #[derive(Serialize)]
    struct Finish {
        id: u64,
        result: Finished,
    }

    #[derive(Deserialize)]
    struct Done {}

    /// The plugin's handle. Calls block until the platform answers, so the
    /// app calls them from a thread of its own.
    pub struct Push<R: Runtime>(pub(crate) tauri::plugin::PluginHandle<R>);

    impl<R: Runtime> std::fmt::Debug for Push<R> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Push")
        }
    }

    impl<R: Runtime> Clone for Push<R> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<R: Runtime> Push<R> {
        /// Waits for the platform's next event.
        pub fn next(&self) -> Result<Event, PushError> {
            self.0
                .run_mobile_plugin::<Event>("next", ())
                .map_err(|error| PushError(error.to_string()))
        }

        /// Completes the wake `id` for iOS with `result`.
        pub fn finish(&self, id: u64, result: Finished) -> Result<(), PushError> {
            self.0
                .run_mobile_plugin::<Done>("finish", Finish { id, result })
                .map(|_| ())
                .map_err(|error| PushError(error.to_string()))
        }
    }
}

#[cfg(target_os = "ios")]
pub use ios::Push;

/// The Swift entry point the iOS half exports. Tauri's binding declares it
/// through FFI, the crate's only `unsafe` (ADR 0030 §5).
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod ios_binding {
    tauri::ios_plugin_binding!(init_plugin_device_push);

    /// The generated binding is private to this module; Tauri calls it.
    pub(super) fn entry_point() -> unsafe fn() -> *const std::ffi::c_void {
        init_plugin_device_push
    }
}

/// The plugin. On iOS it registers the Swift half and manages a `Push`;
/// elsewhere it does nothing.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new(NAME)
        .setup(|_app, _api| {
            #[cfg(target_os = "ios")]
            {
                use tauri::Manager;
                let handle = _api.register_ios_plugin(ios_binding::entry_point())?;
                _app.manage(ios::Push(handle));
            }
            Ok(())
        })
        .build()
}
