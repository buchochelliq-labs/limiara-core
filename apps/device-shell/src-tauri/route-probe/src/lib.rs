//! A test plugin for the current-route context (ADR 0034 §9, slice 2): it
//! declares `device.route_context` and nothing else, and receives each route
//! the web shell reports, on every platform, the way an on-screen awareness
//! plugin will.
//!
//! - iOS and Android: the Swift (`ios/`) and Kotlin (`android/`) halves take
//!   each route with [`Probe::deliver`] and write it, as ids, to a trail in
//!   the app's temporary directory ([`Probe::directory`]), which the CI
//!   checks read from the simulator's container and the emulator. The shell
//!   adds its own lines with [`Probe::note`]: the native half is the
//!   trail's only writer, so no line overwrites another.
//! - Desktop: there is no native half; [`Probe`] keeps what it received.
//!
//! The probe holds no business logic and reads nothing itself: it gets the
//! route pattern's name and ids, never an address or record content. The
//! shell registers it in debug builds only.

use serde::{Deserialize, Serialize};
use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// The plugin's name; Tauri derives the `links` key from it.
pub const NAME: &str = "device-route-probe";

/// The local capabilities it declares (ADR 0024 §6).
pub const CAPABILITIES: &[&str] = &["device.route_context"];

/// The platform's message when the native half could not be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeError(pub String);

impl std::fmt::Display for ProbeError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProbeError {}

/// A route as the native half receives it: the pattern's name and ids, or
/// nothing when no workspace route is shown.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub pattern: Option<String>,
    pub object_name: Option<String>,
    pub record_id: Option<String>,
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod mobile {
    use super::{ProbeError, Route};
    use serde::Deserialize;
    use tauri::Runtime;

    #[derive(Deserialize)]
    struct Done {}

    #[derive(Deserialize)]
    struct Directory {
        path: String,
    }

    #[derive(serde::Serialize)]
    struct Note<'a> {
        message: &'a str,
    }

    /// The plugin's handle. Calls block until the platform answers.
    pub struct Probe<R: Runtime>(pub(crate) tauri::plugin::PluginHandle<R>);

    impl<R: Runtime> std::fmt::Debug for Probe<R> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Probe")
        }
    }

    impl<R: Runtime> Clone for Probe<R> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<R: Runtime> Probe<R> {
        /// Hands a route to the native half.
        pub fn deliver(&self, route: &Route) -> Result<(), ProbeError> {
            self.0
                .run_mobile_plugin::<Done>("route", route.clone())
                .map(|_| ())
                .map_err(|error| ProbeError(error.to_string()))
        }

        /// Adds a line of the shell's to the native half's trail, which
        /// writes its lines one at a time.
        pub fn note(&self, message: &str) -> Result<(), ProbeError> {
            self.0
                .run_mobile_plugin::<Done>("note", Note { message })
                .map(|_| ())
                .map_err(|error| ProbeError(error.to_string()))
        }

        /// The native half's temporary directory, where it keeps its trail.
        pub fn directory(&self) -> Result<std::path::PathBuf, ProbeError> {
            self.0
                .run_mobile_plugin::<Directory>("directory", ())
                .map(|directory| std::path::PathBuf::from(directory.path))
                .map_err(|error| ProbeError(error.to_string()))
        }
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
pub use mobile::Probe;

/// The desktop probe: no native half, so it keeps the routes it received.
#[cfg(not(any(target_os = "android", target_os = "ios")))]
#[derive(Debug, Clone, Default)]
pub struct Probe(std::sync::Arc<std::sync::Mutex<Vec<Route>>>);

#[cfg(not(any(target_os = "android", target_os = "ios")))]
impl Probe {
    /// Takes a route, as the native halves do.
    pub fn deliver(&self, route: &Route) -> Result<(), ProbeError> {
        self.0
            .lock()
            .map_err(|_| ProbeError("the probe's lock was poisoned".into()))?
            .push(route.clone());
        Ok(())
    }

    /// The routes received so far, oldest first.
    pub fn received(&self) -> Vec<Route> {
        self.0
            .lock()
            .map(|routes| routes.clone())
            .unwrap_or_default()
    }
}

/// The Swift entry point the iOS half exports. Tauri's binding declares it
/// through FFI, the crate's only `unsafe`, as in `device-push`.
#[cfg(target_os = "ios")]
#[allow(unsafe_code)]
mod ios_binding {
    tauri::ios_plugin_binding!(init_plugin_device_route_probe);

    /// The generated binding is private to this module; Tauri calls it.
    pub(super) fn entry_point() -> unsafe fn() -> *const std::ffi::c_void {
        init_plugin_device_route_probe
    }
}

/// The plugin. It registers the native half on a phone and manages a
/// `Probe` everywhere.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new(NAME)
        .setup(|_app, _api| {
            use tauri::Manager;
            #[cfg(target_os = "android")]
            {
                let handle =
                    _api.register_android_plugin("device.routeprobe", "RouteProbePlugin")?;
                _app.manage(mobile::Probe(handle));
            }
            #[cfg(target_os = "ios")]
            {
                let handle = _api.register_ios_plugin(ios_binding::entry_point())?;
                _app.manage(mobile::Probe(handle));
            }
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            _app.manage(Probe::default());
            Ok(())
        })
        .build()
}

#[cfg(all(test, not(any(target_os = "android", target_os = "ios"))))]
mod tests {
    use super::*;

    #[test]
    fn the_desktop_probe_keeps_what_it_received() {
        let probe = Probe::default();
        let route = Route {
            pattern: Some("record".into()),
            object_name: Some("custom:account".into()),
            record_id: Some("a01000000000001xyz".into()),
        };
        probe.deliver(&route).expect("delivered");
        probe.deliver(&Route::default()).expect("cleared");
        assert_eq!(probe.received(), vec![route, Route::default()]);
    }

    #[test]
    fn routes_reach_the_native_halves_in_camel_case() {
        let route = Route {
            pattern: Some("record".into()),
            object_name: Some("custom:account".into()),
            record_id: Some("a01000000000001xyz".into()),
        };
        assert_eq!(
            serde_json::to_value(&route).expect("json"),
            serde_json::json!({
                "pattern": "record",
                "objectName": "custom:account",
                "recordId": "a01000000000001xyz",
            })
        );
    }

    #[test]
    fn it_declares_only_the_route_context() {
        assert_eq!(CAPABILITIES, &["device.route_context"]);
    }
}
