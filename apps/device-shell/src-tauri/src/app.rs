//! The app itself: the window over the `limiara://` scheme, shared by the
//! desktop binary and the mobile entry point (Tauri builds the iOS and
//! Android apps from this library).

use crate::{fixture, start_url, Asset, Assets, Shell, SCHEME};
use std::sync::Arc;
use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

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

/// Builds and runs the app: the desktop binary's `main`, and the entry point
/// the iOS and Android projects call.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let shell: Arc<std::sync::OnceLock<Shell>> = Arc::new(std::sync::OnceLock::new());
    let handler_shell = shell.clone();
    let result = tauri::Builder::default()
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
            let bridge = tauri::async_runtime::block_on(fixture::open(&directory))
                .map_err(|error| format!("the demonstration workspace did not open: {error}"))?;
            let assets: Arc<dyn Assets> = Arc::new(Embedded(app.handle().clone()));
            let _ = shell.set(Shell::new(bridge, assets));
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
