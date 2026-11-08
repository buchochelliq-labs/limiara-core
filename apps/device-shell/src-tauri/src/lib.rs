//! The Limiara app shell for desktop and mobile (ADR 0024 §5).
//!
//! The shell's webview loads the same React shell as the browser, from the
//! `limiara://` scheme. One handler answers that scheme:
//!
//! - `/api/device/session` is the app's sign-in (ADR 0029): its state, and
//!   signing in, out and into the demonstration workspace, run by
//!   `platform_device::app::Controller`;
//! - the rest of `/api/...` goes to the local command bridge
//!   (`platform-device`), which answers with the server's own routes over
//!   the replica on the device, or `401 auth.signed_out` while no workspace
//!   is open;
//! - everything else is the embedded web build, with `index.html` for the
//!   shell's client-side routes.
//!
//! Because both come from one origin, the web shell's relative `/api/v1`
//! requests reach the bridge unchanged, and nothing else can: there is no
//! port, no network listener and no third-party script in the origin.

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, Method, Request, Response, StatusCode};
use platform_device::app::{Controller, SignInError};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

pub mod app;
pub mod fixture;
pub mod keychain;
#[cfg(any(mobile, test))]
pub mod phone;

/// The app's own sign-in route, answered by the shell rather than the
/// bridge, so it works while no workspace is open.
pub const SESSION_PATH: &str = "/api/device/session";

/// The scheme the webview loads the shell from.
pub const SCHEME: &str = "limiara";

/// The shell's content security policy. Scripts, styles, images and
/// requests come only from the shell's own origin; nothing frames it and
/// nothing is embedded. Inline styles stay allowed because the React shell
/// sets `style` attributes.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; \
object-src 'none'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

/// One embedded file of the web build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub bytes: Vec<u8>,
    pub mime_type: String,
}

/// Where the shell finds the web build: Tauri's embedded assets in the app,
/// a map in tests.
pub trait Assets: Send + Sync {
    fn get(&self, path: &str) -> Option<Asset>;
}

impl Assets for HashMap<String, Asset> {
    fn get(&self, path: &str) -> Option<Asset> {
        HashMap::get(self, path).cloned()
    }
}

/// Answers the `limiara://` scheme.
#[derive(Clone)]
pub struct Shell {
    controller: Arc<Controller>,
    assets: Arc<dyn Assets>,
    /// Where the demonstration workspace's file goes.
    directory: PathBuf,
}

impl Shell {
    pub fn new(
        controller: Arc<Controller>,
        assets: Arc<dyn Assets>,
        directory: impl Into<PathBuf>,
    ) -> Self {
        Self {
            controller,
            assets,
            directory: directory.into(),
        }
    }

    pub fn controller(&self) -> &Arc<Controller> {
        &self.controller
    }

    /// Answers one request from the webview.
    pub async fn respond(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let path = request.uri().path().to_owned();
        if path == SESSION_PATH {
            return secure(no_store(self.session(request).await));
        }
        if path == "/api" || path.starts_with("/api/") {
            let writes = !matches!(*request.method(), Method::GET | Method::HEAD);
            let response = match self.controller.bridge() {
                Some(bridge) => bridge.handle(request).await,
                None => problem(
                    StatusCode::UNAUTHORIZED,
                    "auth.signed_out",
                    "sign in to a workspace on this device",
                ),
            };
            if writes && response.status().is_success() {
                // The foreground sync pushes the change after a pause.
                self.controller.local_change();
            }
            return secure(no_store(response));
        }
        if !matches!(*request.method(), Method::GET | Method::HEAD) {
            return secure(plain(StatusCode::METHOD_NOT_ALLOWED));
        }
        match self.asset(&path) {
            Some(asset) => {
                let mut response = Response::new(if request.method() == Method::HEAD {
                    Vec::new()
                } else {
                    asset.bytes
                });
                if let Ok(value) = HeaderValue::from_str(&asset.mime_type) {
                    response.headers_mut().insert(CONTENT_TYPE, value);
                }
                secure(response)
            }
            None => secure(plain(StatusCode::NOT_FOUND)),
        }
    }

    /// The file for a path; a path without a file extension is one of the
    /// shell's own routes, answered by `index.html`.
    fn asset(&self, path: &str) -> Option<Asset> {
        if path.split('/').any(|segment| segment == "..") {
            return None;
        }
        let file = if path == "/" { "/index.html" } else { path };
        if let Some(asset) = self.assets.get(file) {
            return Some(asset);
        }
        let last = path.rsplit('/').next().unwrap_or_default();
        if last.contains('.') {
            return None;
        }
        self.assets.get("/index.html")
    }

    /// `GET` the sign-in state; `POST {"address": …}` to sign in through the
    /// system browser, or `{"demo": true}` for the demonstration workspace;
    /// `DELETE` to cancel a sign-in or sign out, which deletes the device's
    /// copy of the workspace.
    async fn session(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let controller = &self.controller;
        match *request.method() {
            Method::GET => {}
            Method::POST => {
                let body: Value = serde_json::from_slice(request.body()).unwrap_or(Value::Null);
                let started = if body["demo"] == Value::Bool(true) {
                    match fixture::open(&self.directory).await {
                        Ok(bridge) => controller.show_demo(bridge),
                        Err(_) => {
                            return problem(
                                StatusCode::SERVICE_UNAVAILABLE,
                                "device.storage_unavailable",
                                "the demonstration workspace could not be opened",
                            )
                        }
                    }
                } else {
                    controller.start_sign_in(body["address"].as_str().unwrap_or_default())
                };
                match started {
                    Ok(()) => {}
                    Err(error @ SignInError::InvalidAddress) => {
                        return problem(
                            StatusCode::BAD_REQUEST,
                            "device.address_invalid",
                            &error.to_string(),
                        )
                    }
                    Err(error @ SignInError::Busy) => {
                        return problem(StatusCode::CONFLICT, "device.busy", &error.to_string())
                    }
                }
            }
            Method::DELETE => controller.sign_out().await,
            _ => return plain(StatusCode::METHOD_NOT_ALLOWED),
        }
        json_response(StatusCode::OK, &json!(controller.status().await))
    }
}

fn json_response(status: StatusCode, body: &Value) -> Response<Vec<u8>> {
    let mut response = Response::new(serde_json::to_vec(body).unwrap_or_default());
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

/// A problem answer in the server's shape.
fn problem(status: StatusCode, code: &str, detail: &str) -> Response<Vec<u8>> {
    let mut response = json_response(
        status,
        &json!({"type": "about:blank", "status": status.as_u16(), "code": code, "detail": detail}),
    );
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/problem+json"),
    );
    response
}

fn no_store(mut response: Response<Vec<u8>>) -> Response<Vec<u8>> {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn plain(status: StatusCode) -> Response<Vec<u8>> {
    let mut response = Response::new(Vec::new());
    *response.status_mut() = status;
    response
}

/// The headers every response carries.
fn secure(mut response: Response<Vec<u8>>) -> Response<Vec<u8>> {
    let headers = response.headers_mut();
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

/// The URL the main window opens. Windows and Android webviews reach custom
/// schemes as `http://<scheme>.localhost`.
pub fn start_url() -> &'static str {
    if cfg!(any(windows, target_os = "android")) {
        "http://limiara.localhost/"
    } else {
        "limiara://localhost/"
    }
}
