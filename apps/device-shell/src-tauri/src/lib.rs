//! The Limiara app shell for desktop and mobile (ADR 0024 §5).
//!
//! The shell's webview loads the same React shell as the browser, from the
//! `limiara://` scheme. One handler answers that scheme:
//!
//! - `/api/...` goes to the local command bridge (`platform-device`), which
//!   answers with the server's own routes over the replica on the device;
//! - everything else is the embedded web build, with `index.html` for the
//!   shell's client-side routes.
//!
//! Because both come from one origin, the web shell's relative `/api/v1`
//! requests reach the bridge unchanged, and nothing else can: there is no
//! port, no network listener and no third-party script in the origin.

use http::header::{CACHE_CONTROL, CONTENT_TYPE};
use http::{HeaderValue, Method, Request, Response, StatusCode};
use platform_device::Bridge;
use std::collections::HashMap;
use std::sync::Arc;

pub mod app;
pub mod fixture;

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
    bridge: Bridge,
    assets: Arc<dyn Assets>,
}

impl Shell {
    pub fn new(bridge: Bridge, assets: Arc<dyn Assets>) -> Self {
        Self { bridge, assets }
    }

    /// Answers one request from the webview.
    pub async fn respond(&self, request: Request<Vec<u8>>) -> Response<Vec<u8>> {
        let path = request.uri().path().to_owned();
        if path == "/api" || path.starts_with("/api/") {
            let mut response = self.bridge.handle(request).await;
            response
                .headers_mut()
                .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return secure(response);
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
