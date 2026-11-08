//! The Android Keystore for the app shell's secrets (ADR 0024, "Encryption
//! at rest"; ADR 0029).
//!
//! The Kotlin half (`android/`) keeps one AES-256-GCM key in the Android
//! Keystore, where the key material never leaves the platform's keystore,
//! and stores each secret encrypted under it in the app's private
//! preferences, bound to its entry name. Elsewhere the plugin registers
//! nothing, and `Store` does not exist.

use tauri::plugin::{Builder, TauriPlugin};
use tauri::Runtime;

/// The plugin's name; Tauri derives the `links` key from it.
pub const NAME: &str = "device-keystore";

/// The store's error: the platform's message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreError(pub String);

impl std::fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

#[cfg(target_os = "android")]
mod android {
    use super::StoreError;
    use serde::{Deserialize, Serialize};
    use tauri::Runtime;

    #[derive(Serialize)]
    struct Entry<'a> {
        name: &'a str,
    }

    #[derive(Serialize)]
    struct Write<'a> {
        name: &'a str,
        value: &'a str,
    }

    #[derive(Deserialize)]
    struct Read {
        #[serde(default)]
        value: Option<String>,
    }

    #[derive(Deserialize)]
    struct Done {}

    /// The Keystore-backed store. Calls block until the platform answers, so
    /// callers run them off the async runtime.
    pub struct Store<R: Runtime>(pub(crate) tauri::plugin::PluginHandle<R>);

    impl<R: Runtime> std::fmt::Debug for Store<R> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("Store")
        }
    }

    impl<R: Runtime> Clone for Store<R> {
        fn clone(&self) -> Self {
            Self(self.0.clone())
        }
    }

    impl<R: Runtime> Store<R> {
        /// The secret stored under `name`, if any.
        pub fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
            self.0
                .run_mobile_plugin::<Read>("get", Entry { name })
                .map(|read| read.value)
                .map_err(|error| StoreError(error.to_string()))
        }

        /// Stores `value` under `name`, replacing any earlier value.
        pub fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
            self.0
                .run_mobile_plugin::<Done>("set", Write { name, value })
                .map(|_| ())
                .map_err(|error| StoreError(error.to_string()))
        }

        /// Removes `name`; removing a missing entry succeeds.
        pub fn delete(&self, name: &str) -> Result<(), StoreError> {
            self.0
                .run_mobile_plugin::<Done>("delete", Entry { name })
                .map(|_| ())
                .map_err(|error| StoreError(error.to_string()))
        }
    }
}

#[cfg(target_os = "android")]
pub use android::Store;

/// The plugin. On Android it registers the Kotlin half and manages a
/// `Store`; elsewhere it does nothing.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    Builder::new(NAME)
        .setup(|_app, _api| {
            #[cfg(target_os = "android")]
            {
                use tauri::Manager;
                let handle = _api.register_android_plugin("device.keystore", "KeystorePlugin")?;
                _app.manage(android::Store(handle));
            }
            Ok(())
        })
        .build()
}
