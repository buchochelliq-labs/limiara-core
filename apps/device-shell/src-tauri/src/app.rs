//! The app itself: the window over the `limiara://` scheme, shared by the
//! desktop binary and the mobile entry point (Tauri builds the iOS and
//! Android apps from this library).

use crate::{start_url, Asset, Assets, Shell, SCHEME};
use platform_device::app::{AppSettings, Browser, Controller, LoopbackRedirects, Services};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// The redirect URI desktop builds register in
/// `platform.devices.redirect_uris` (ADR 0029).
const DESKTOP_REDIRECT_URI: &str = "http://127.0.0.1:47185/callback";

/// The web build embedded in the app by `tauri::generate_context!`.
struct Embedded<R: Runtime>(AppHandle<R>);

impl<R: Runtime> Assets for Embedded<R> {
    fn get(&self, path: &str) -> Option<Asset> {
        self.0
            .asset_resolver()
            .get(path.to_owned())
            .map(|asset| Asset {
                bytes: asset.bytes,
                mime_type: asset.mime_type,
            })
    }
}

/// The member's default browser: the system browser RFC 8252 asks for, with
/// their existing sign-in at the identity provider.
struct SystemBrowser;

impl Browser for SystemBrowser {
    #[cfg(desktop)]
    fn open(&self, url: &str) -> Result<(), String> {
        open::that_detached(url).map_err(|error| error.to_string())
    }

    /// Phones sign in through `ASWebAuthenticationSession` and Custom Tabs
    /// with the app's own redirect scheme, which come with the phone
    /// sign-in work; until then the sign-in screen says so.
    #[cfg(mobile)]
    fn open(&self, _url: &str) -> Result<(), String> {
        Err("signing in on a phone is not available yet".into())
    }
}

/// The platform the server records for the device (ADR 0029).
fn platform() -> &'static str {
    if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "desktop"
    }
}

/// How the device shows in the member's device list.
fn device_name() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_default();
    let host = host.trim();
    if !host.is_empty() {
        host.chars().take(80).collect()
    } else if cfg!(mobile) {
        "Limiara phone".into()
    } else {
        "Limiara desktop".into()
    }
}

/// Builds and runs the app: the desktop binary's `main`, and the entry point
/// the iOS and Android projects call.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let shell: Arc<std::sync::OnceLock<Shell>> = Arc::new(std::sync::OnceLock::new());
    let handler_shell = shell.clone();
    let result = tauri::Builder::default()
        .plugin(device_keystore::init())
        .register_asynchronous_uri_scheme_protocol(SCHEME, move |_context, request, responder| {
            let shell = handler_shell.clone();
            tauri::async_runtime::spawn(async move {
                let response = match shell.get() {
                    Some(shell) => shell.respond(request).await,
                    None => {
                        let mut response = http::Response::new(Vec::new());
                        *response.status_mut() = http::StatusCode::SERVICE_UNAVAILABLE;
                        response
                    }
                };
                responder.respond(response);
            });
        })
        .setup(move |app| {
            let directory = app.path().app_data_dir()?;
            let redirect_uri = std::env::var("LIMIARA_REDIRECT_URI")
                .unwrap_or_else(|_| DESKTOP_REDIRECT_URI.to_owned());
            let redirects = LoopbackRedirects::new(&redirect_uri)
                .map_err(|error| format!("the redirect URI is invalid: {error}"))?;
            let keychain = crate::keychain::Keychain::default();
            #[cfg(target_os = "android")]
            let keychain = {
                use tauri::Manager as _;
                let store = app
                    .try_state::<device_keystore::Store<tauri::Wry>>()
                    .ok_or("the Android Keystore plugin did not start")?;
                let keychain = keychain.with_store(store.inner().clone());
                // Debug builds prove the Keystore round trip at launch; CI
                // then checks that only ciphertext reached the app's files.
                if cfg!(debug_assertions) {
                    tauri::async_runtime::block_on(keychain.probe())?;
                }
                keychain
            };
            let keychain = Arc::new(keychain);
            let controller = Controller::new(
                directory.join("workspaces"),
                AppSettings {
                    redirect_uri,
                    device_name: device_name(),
                    platform: platform().into(),
                },
                Services {
                    vault: keychain.clone(),
                    keys: keychain,
                    browser: Arc::new(SystemBrowser),
                    redirects: Arc::new(redirects),
                },
            );
            // Reopens a signed-in workspace before any network.
            tauri::async_runtime::block_on(controller.resume());
            let assets: Arc<dyn Assets> = Arc::new(Embedded(app.handle().clone()));
            let _ = shell.set(Shell::new(
                controller,
                assets,
                directory.join("demonstration"),
            ));
            let url = start_url()
                .parse()
                .map_err(|error| format!("the start URL is invalid: {error}"))?;
            WebviewWindowBuilder::new(app, "main", WebviewUrl::CustomProtocol(url))
                .title("Limiara")
                .inner_size(1200.0, 800.0)
                .build()?;
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(error) = result {
        eprintln!("Limiara could not start: {error}");
        std::process::exit(1);
    }
}
