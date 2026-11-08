//! Phone sign-in (ADR 0029 §7, "Phone sign-in"): the system browser of
//! the `device-browser` plugin behind the controller's [`Browser`] and
//! [`Redirects`] ports.
//!
//! The controller prepares the redirect before it opens the browser. Here
//! [`Redirects::prepare`] parks the waiting side, and [`Browser::open`] runs
//! the platform's sign-in, which blocks until the browser redirects to the
//! app's scheme, then hands the redirect over.

use async_trait::async_trait;
use platform_device::app::{Browser, PendingRedirect, Redirects};
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use url::Url;

type Waiting = oneshot::Sender<Result<Url, String>>;

/// The platform's sign-in: opens a URL and returns the redirect it ends on.
pub type Authenticate = dyn Fn(&str, &str) -> Result<String, String> + Send + Sync;

/// The browser and redirect ports over one platform sign-in.
pub struct BrowserSignIn {
    scheme: String,
    authenticate: Arc<Authenticate>,
    waiting: Mutex<Option<Waiting>>,
}

impl BrowserSignIn {
    /// `scheme` is the app's private-use scheme, its application id.
    pub fn new(scheme: impl Into<String>, authenticate: Arc<Authenticate>) -> Self {
        Self {
            scheme: scheme.into(),
            authenticate,
            waiting: Mutex::new(None),
        }
    }

    /// The redirect URI this build registers in
    /// `platform.devices.redirect_uris`.
    pub fn redirect_uri(&self) -> String {
        format!("{}:/callback", self.scheme)
    }
}

#[async_trait]
impl Redirects for BrowserSignIn {
    async fn prepare(&self) -> Result<PendingRedirect, String> {
        let (sender, receiver) = oneshot::channel();
        let mut waiting = self
            .waiting
            .lock()
            .map_err(|_| "the sign-in's lock was poisoned".to_owned())?;
        // A new sign-in replaces one that never opened its browser.
        *waiting = Some(sender);
        Ok(Box::pin(async move {
            receiver
                .await
                .map_err(|_| "the sign-in was abandoned".to_owned())?
        }))
    }
}

impl Browser for BrowserSignIn {
    fn open(&self, url: &str) -> Result<(), String> {
        let sender = self
            .waiting
            .lock()
            .map_err(|_| "the sign-in's lock was poisoned".to_owned())?
            .take()
            .ok_or_else(|| "no sign-in is waiting for the browser".to_owned())?;
        let url = url.to_owned();
        let scheme = self.scheme.clone();
        let authenticate = self.authenticate.clone();
        // The platform call blocks until the member finishes in the browser.
        std::thread::Builder::new()
            .name("sign-in".into())
            .spawn(move || {
                let redirect = authenticate(&url, &scheme).and_then(|redirect| {
                    let redirect = Url::parse(&redirect)
                        .map_err(|_| "the browser returned an unreadable address".to_owned())?;
                    if redirect.scheme() == scheme {
                        Ok(redirect)
                    } else {
                        Err("the browser returned to another app's address".to_owned())
                    }
                });
                // The receiver is gone if the sign-in was cancelled meanwhile.
                let _ = sender.send(redirect);
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign_in(
        authenticate: impl Fn(&str, &str) -> Result<String, String> + Send + Sync + 'static,
    ) -> BrowserSignIn {
        BrowserSignIn::new("app.example.shell", Arc::new(authenticate))
    }

    #[tokio::test]
    async fn the_redirect_reaches_the_prepared_sign_in() {
        let ports = sign_in(|url, scheme| {
            assert_eq!(url, "https://server.example/devices/authorize?state=s");
            Ok(format!("{scheme}:/callback?code=c&state=s"))
        });
        assert_eq!(ports.redirect_uri(), "app.example.shell:/callback");
        let pending = ports.prepare().await.expect("prepared");
        ports
            .open("https://server.example/devices/authorize?state=s")
            .expect("opened");
        let redirect = pending.await.expect("redirected");
        assert_eq!(
            redirect.as_str(),
            "app.example.shell:/callback?code=c&state=s"
        );
    }

    #[tokio::test]
    async fn a_cancelled_or_foreign_redirect_fails_the_sign_in() {
        let cancelled = sign_in(|_, _| Err("the sign-in was cancelled".into()));
        let pending = cancelled.prepare().await.expect("prepared");
        cancelled.open("https://server.example/").expect("opened");
        assert_eq!(pending.await.unwrap_err(), "the sign-in was cancelled");

        let foreign = sign_in(|_, _| Ok("other.app:/callback?code=c".into()));
        let pending = foreign.prepare().await.expect("prepared");
        foreign.open("https://server.example/").expect("opened");
        assert_eq!(
            pending.await.unwrap_err(),
            "the browser returned to another app's address"
        );
    }

    #[tokio::test]
    async fn the_browser_opens_only_for_a_prepared_sign_in() {
        let ports = sign_in(|_, scheme| Ok(format!("{scheme}:/callback")));
        assert!(ports.open("https://server.example/").is_err());
        let pending = ports.prepare().await.expect("prepared");
        ports.open("https://server.example/").expect("opened");
        assert!(pending.await.is_ok());
        assert!(ports.open("https://server.example/").is_err());
    }
}
