//! The app itself: the window over the `limiara://` scheme, shared by the
//! desktop binary and the mobile entry point (Tauri builds the iOS and
//! Android apps from this library).

use crate::{start_url, Asset, Assets, Shell, SCHEME};
use platform_device::app::{AppSettings, Browser, Controller, Redirects, Services};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

/// The redirect URI desktop builds register in
/// `platform.devices.redirect_uris` (ADR 0029).
#[cfg(desktop)]
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
#[cfg(desktop)]
struct SystemBrowser;

#[cfg(desktop)]
impl Browser for SystemBrowser {
    fn open(&self, url: &str) -> Result<(), String> {
        open::that_detached(url).map_err(|error| error.to_string())
    }
}

/// This build's redirect URI and the ports that open the browser and
/// receive the redirect.
type SignInPorts = (String, Arc<dyn Browser>, Arc<dyn Redirects>);

/// Desktop: the default browser and a loopback redirect (RFC 8252 §7.3).
#[cfg(desktop)]
fn sign_in_ports<R: Runtime>(_app: &tauri::App<R>) -> Result<SignInPorts, String> {
    let redirect_uri =
        std::env::var("LIMIARA_REDIRECT_URI").unwrap_or_else(|_| DESKTOP_REDIRECT_URI.to_owned());
    let redirects = platform_device::app::LoopbackRedirects::new(&redirect_uri)
        .map_err(|error| format!("the redirect URI is invalid: {error}"))?;
    Ok((redirect_uri, Arc::new(SystemBrowser), Arc::new(redirects)))
}

/// Phones: `ASWebAuthenticationSession` or a Custom Tab, redirecting to the
/// app's private-use scheme, its application id (RFC 8252 §7.1).
#[cfg(mobile)]
fn sign_in_ports(app: &tauri::App<tauri::Wry>) -> Result<SignInPorts, String> {
    let browser = app
        .try_state::<device_browser::Browser<tauri::Wry>>()
        .ok_or("the sign-in browser plugin did not start")?
        .inner()
        .clone();
    let sign_in = Arc::new(crate::phone::BrowserSignIn::new(
        app.config().identifier.clone(),
        Arc::new(move |url: &str, scheme: &str| {
            browser
                .authenticate(url, scheme)
                .map_err(|error| error.to_string())
        }),
    ));
    let redirect_uri = sign_in.redirect_uri();
    let opener: Arc<dyn Browser> = sign_in.clone();
    let receiver: Arc<dyn Redirects> = sign_in;
    Ok((redirect_uri, opener, receiver))
}

/// Hands the platform's push events to the controller (ADR 0030 §5): the
/// APNs token goes to the server, and each content-free push runs one
/// budgeted sync, whose outcome completes the push for iOS.
#[cfg(target_os = "ios")]
fn follow_push(app: &tauri::App<tauri::Wry>, controller: Arc<Controller>) {
    use device_push::Event;
    use platform_device::app::PushProvider;
    let Some(push) = app
        .try_state::<device_push::Push<tauri::Wry>>()
        .map(|push| push.inner().clone())
    else {
        return;
    };
    let follow = move || loop {
        match push.next() {
            Ok(Event::Token { token }) => {
                // Sent again whenever a workspace opens, so a failure here
                // costs nothing.
                let _ = tauri::async_runtime::block_on(
                    controller.register_push(PushProvider::Apns, &token),
                );
            }
            Ok(Event::Wake { id, received_at }) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|since| since.as_secs_f64())
                    .unwrap_or(received_at);
                let result = match crate::phone::wake_budget(received_at, now) {
                    Some(budget) => crate::phone::finished(&tauri::async_runtime::block_on(
                        controller.wake(budget),
                    )),
                    // Its time ran out while an earlier wake synced.
                    None => device_push::Finished::Failed,
                };
                let _ = push.finish(id, result);
            }
            // Without the push entitlement, the app syncs when it is opened.
            Ok(Event::Unavailable { .. }) => {}
            Err(_) => return,
        }
    };
    let _ = std::thread::Builder::new()
        .name("push".into())
        .spawn(follow);
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
        .plugin(device_browser::init())
        .plugin(device_push::init())
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
            let (redirect_uri, browser, redirects) = sign_in_ports(app)?;
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
                    browser,
                    redirects,
                },
            );
            // Reopens a signed-in workspace before any network.
            tauri::async_runtime::block_on(controller.resume());
            #[cfg(target_os = "ios")]
            follow_push(app, controller.clone());
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
