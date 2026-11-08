//! The app's side of device sign-in (ADR 0029): PKCE, the sign-in URL for
//! the system browser, the one-time code on the redirect, the token grants
//! and the device's own profile. [`DeviceSession`] is the
//! [`AccessToken`] the sync transport calls: it refreshes the access token
//! before it runs out and hands every rotated refresh token to the app's
//! [`TokenSink`], which stores it in the platform keychain and passes the
//! offline limit to the replica.
//!
//! The app talks only to the endpoints ADR 0029 defines; there is no
//! password anywhere.

use crate::http::AccessToken;
use crate::SyncError;
use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::RngCore;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::time::Instant;
use url::Url;
use uuid::Uuid;

/// How long before its expiry an access token is replaced.
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// Why signing in or refreshing failed.
#[derive(Debug, Error)]
pub enum AuthError {
    /// The server address is not an `http` or `https` origin.
    #[error("the server address is not valid")]
    InvalidServer,
    /// The browser came back with an error, such as a cancelled sign-in.
    #[error("the sign-in was not completed: {0}")]
    Refused(String),
    /// The redirect carried another sign-in's state, or no code.
    #[error("the redirect does not belong to this sign-in")]
    StateMismatch,
    /// The server refused the code or the refresh token (`invalid_grant`);
    /// the app signs the member in again.
    #[error("the server refused the grant: {0}")]
    InvalidGrant(String),
    /// The device was revoked or its member left: the app deletes the
    /// workspace's replica (`"wipe": true`).
    #[error("the device must wipe this workspace")]
    WipeRequired,
    /// The server answered with a problem this client does not handle.
    #[error("the server answered {status} {code}")]
    Server { status: u16, code: String },
    #[error("the server could not be reached: {0}")]
    Transport(String),
    #[error("unexpected answer from the server: {0}")]
    Protocol(String),
    /// The app could not store the rotated refresh token.
    #[error("the token could not be stored: {0}")]
    Storage(String),
}

impl From<AuthError> for SyncError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::WipeRequired => SyncError::WipeRequired,
            AuthError::InvalidGrant(_) | AuthError::StateMismatch | AuthError::Refused(_) => {
                SyncError::Unauthorized
            }
            AuthError::Server { status, code } => SyncError::Server { status, code },
            AuthError::Transport(detail) => SyncError::Transport(detail),
            other => SyncError::Protocol(other.to_string()),
        }
    }
}

fn random_token(bytes: usize) -> String {
    let mut buffer = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut buffer);
    URL_SAFE_NO_PAD.encode(buffer)
}

/// A PKCE verifier and its S256 challenge (RFC 7636).
pub struct Pkce {
    verifier: String,
    challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        // 32 random bytes are 43 characters, the shortest verifier allowed.
        let verifier = random_token(32);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }

    pub fn challenge(&self) -> &str {
        &self.challenge
    }

    pub fn verifier(&self) -> &str {
        &self.verifier
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Pkce {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pkce")
            .field("challenge", &self.challenge)
            .finish_non_exhaustive()
    }
}

/// The server's origin, such as `https://limiara.example.com`.
fn origin(server: &str) -> Result<Url, AuthError> {
    let url = Url::parse(server.trim()).map_err(|_| AuthError::InvalidServer)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(AuthError::InvalidServer);
    }
    let mut origin = url;
    origin.set_path("/");
    origin.set_query(None);
    origin.set_fragment(None);
    Ok(origin)
}

/// One sign-in in the system browser: its PKCE verifier and state stay in
/// the app, the browser gets only the challenge.
#[derive(Debug)]
pub struct SignIn {
    server: Url,
    redirect_uri: String,
    state: String,
    pkce: Pkce,
}

impl SignIn {
    /// `redirect_uri` is the app build's registered redirect, such as
    /// `com.example.app:/oauth` or a loopback `http://127.0.0.1:port/…`.
    pub fn start(server: &str, redirect_uri: &str) -> Result<Self, AuthError> {
        Ok(Self {
            server: origin(server)?,
            redirect_uri: redirect_uri.to_owned(),
            state: random_token(16),
            pkce: Pkce::new(),
        })
    }

    /// `GET /auth/device/login`, for the system browser.
    pub fn browser_url(&self) -> String {
        let mut url = self.server.clone();
        url.set_path("/auth/device/login");
        url.query_pairs_mut()
            .append_pair("redirect_uri", &self.redirect_uri)
            .append_pair("code_challenge", self.pkce.challenge())
            .append_pair("code_challenge_method", "S256")
            .append_pair("state", &self.state);
        url.to_string()
    }

    pub fn server(&self) -> &Url {
        &self.server
    }

    pub fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// The one-time code from the redirect the browser delivered, once its
    /// state is this sign-in's.
    pub fn code(&self, redirect: &Url) -> Result<String, AuthError> {
        let mut code = None;
        let mut state = None;
        let mut error = None;
        for (key, value) in redirect.query_pairs() {
            match key.as_ref() {
                "code" => code = Some(value.into_owned()),
                "state" => state = Some(value.into_owned()),
                "error" => error = Some(value.into_owned()),
                _ => {}
            }
        }
        if state.as_deref() != Some(self.state.as_str()) {
            return Err(AuthError::StateMismatch);
        }
        if let Some(error) = error {
            return Err(AuthError::Refused(error));
        }
        code.filter(|code| !code.is_empty())
            .ok_or(AuthError::StateMismatch)
    }
}

/// A token response (`POST /oauth/token`).
#[derive(Clone, Deserialize)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: u64,
    pub workspace_id: Uuid,
    pub device_id: Uuid,
    pub offline_limit_seconds: u64,
}

impl fmt::Debug for Tokens {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Tokens")
            .field("expires_in", &self.expires_in)
            .field("workspace_id", &self.workspace_id)
            .field("device_id", &self.device_id)
            .field("offline_limit_seconds", &self.offline_limit_seconds)
            .finish_non_exhaustive()
    }
}

/// What the app tells the server about itself when it redeems a code.
#[derive(Debug, Clone)]
pub struct Registration {
    pub workspace: Uuid,
    /// Shown in the member's device list, such as "Ada's laptop".
    pub device_name: String,
    /// `ios`, `android` or `desktop`.
    pub platform: String,
}

/// The member a device signed in as (`GET /devices/current`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileMember {
    pub id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub permissions: BTreeSet<String>,
    /// Role names, for the shell.
    pub roles: BTreeSet<String>,
    /// Role ids, for the replica engine's object permissions.
    pub role_ids: Vec<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileWorkspace {
    pub id: Uuid,
    pub name: String,
}

/// The calling device, its member and its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub device_id: Uuid,
    pub member: ProfileMember,
    pub workspace: ProfileWorkspace,
}

/// The token endpoint and the device's own routes on one server.
#[derive(Clone)]
pub struct TokenClient {
    client: reqwest::Client,
    server: Url,
}

impl TokenClient {
    pub fn new(server: &str) -> Result<Self, AuthError> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|error| AuthError::Transport(error.to_string()))?;
        Ok(Self::with_client(client, origin(server)?))
    }

    pub fn with_client(client: reqwest::Client, server: Url) -> Self {
        Self { client, server }
    }

    pub fn server(&self) -> &Url {
        &self.server
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.server.as_str().trim_end_matches('/'), path)
    }

    async fn grant(&self, form: &[(&str, &str)]) -> Result<Tokens, AuthError> {
        let response = self
            .client
            .post(self.url("/oauth/token"))
            .form(form)
            .send()
            .await
            .map_err(|error| AuthError::Transport(error.to_string()))?;
        let status = response.status();
        let body: Value = response
            .json()
            .await
            .map_err(|error| AuthError::Protocol(format!("{status} token body: {error}")))?;
        if status.is_success() {
            return serde_json::from_value(body)
                .map_err(|error| AuthError::Protocol(format!("token response: {error}")));
        }
        let error = body["error"].as_str().unwrap_or_default();
        if error == "invalid_grant" {
            if body["wipe"] == Value::Bool(true) {
                return Err(AuthError::WipeRequired);
            }
            let description = body["error_description"].as_str().unwrap_or_default();
            return Err(AuthError::InvalidGrant(description.to_owned()));
        }
        Err(AuthError::Server {
            status: status.as_u16(),
            code: error.to_owned(),
        })
    }

    /// Redeems the code from [`SignIn::code`]: registers the device in the
    /// workspace and returns its first tokens.
    pub async fn redeem(
        &self,
        sign_in: &SignIn,
        code: &str,
        registration: &Registration,
    ) -> Result<Tokens, AuthError> {
        let workspace = registration.workspace.to_string();
        self.grant(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", sign_in.pkce.verifier()),
            ("redirect_uri", sign_in.redirect_uri()),
            ("workspace_id", &workspace),
            ("device_name", &registration.device_name),
            ("platform", &registration.platform),
        ])
        .await
    }

    /// Replaces a refresh token. The old one must never be sent again: a
    /// reused refresh token revokes the device.
    pub async fn refresh(&self, refresh_token: &str) -> Result<Tokens, AuthError> {
        self.grant(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .await
    }

    async fn device_call(
        &self,
        method: reqwest::Method,
        access_token: &str,
        workspace: Uuid,
    ) -> Result<(StatusCode, Value), AuthError> {
        self.device_request(method, access_token, workspace, "", None)
            .await
    }

    /// A call on `/devices/current{suffix}` with the device's own token.
    async fn device_request(
        &self,
        method: reqwest::Method,
        access_token: &str,
        workspace: Uuid,
        suffix: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value), AuthError> {
        let mut request = self
            .client
            .request(
                method,
                self.url(&format!(
                    "/api/v1/workspaces/{workspace}/devices/current{suffix}"
                )),
            )
            .bearer_auth(access_token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| AuthError::Transport(error.to_string()))?;
        let status = response.status();
        let body = response.json().await.unwrap_or(Value::Null);
        if status == StatusCode::UNAUTHORIZED {
            return Err(if body["code"] == "auth.device_wipe_required" {
                AuthError::WipeRequired
            } else {
                AuthError::InvalidGrant("the access token was refused".into())
            });
        }
        Ok((status, body))
    }

    /// `GET /devices/current`: the member and workspace the replica's
    /// engine is built for.
    pub async fn profile(
        &self,
        access_token: &str,
        workspace: Uuid,
    ) -> Result<DeviceProfile, AuthError> {
        let (status, body) = self
            .device_call(reqwest::Method::GET, access_token, workspace)
            .await?;
        if !status.is_success() {
            return Err(AuthError::Server {
                status: status.as_u16(),
                code: body["code"].as_str().unwrap_or_default().to_owned(),
            });
        }
        let device_id = body["device"]["id"]
            .as_str()
            .and_then(|id| Uuid::parse_str(id).ok())
            .ok_or_else(|| AuthError::Protocol("the profile names no device".into()))?;
        let member = serde_json::from_value(body["member"].clone())
            .map_err(|error| AuthError::Protocol(format!("profile member: {error}")))?;
        let workspace = serde_json::from_value(body["workspace"].clone())
            .map_err(|error| AuthError::Protocol(format!("profile workspace: {error}")))?;
        Ok(DeviceProfile {
            device_id,
            member,
            workspace,
        })
    }

    /// `PUT /devices/current/push`: where the server sends this device's
    /// content-free push notifications (ADR 0030). The app calls it with
    /// each token the platform hands it, since a provider may change it.
    pub async fn set_push(
        &self,
        access_token: &str,
        workspace: Uuid,
        provider: PushProvider,
        token: &str,
    ) -> Result<(), AuthError> {
        let (status, body) = self
            .device_request(
                reqwest::Method::PUT,
                access_token,
                workspace,
                "/push",
                Some(serde_json::json!({"provider": provider, "token": token})),
            )
            .await?;
        server_result(status, &body)
    }

    /// `DELETE /devices/current/push`: the server stops waking this device.
    pub async fn clear_push(&self, access_token: &str, workspace: Uuid) -> Result<(), AuthError> {
        let (status, body) = self
            .device_request(
                reqwest::Method::DELETE,
                access_token,
                workspace,
                "/push",
                None,
            )
            .await?;
        server_result(status, &body)
    }

    /// `DELETE /devices/current`: the device signs itself out. A device
    /// the server already revoked counts as signed out.
    pub async fn sign_out(&self, access_token: &str, workspace: Uuid) -> Result<(), AuthError> {
        match self
            .device_call(reqwest::Method::DELETE, access_token, workspace)
            .await
        {
            Ok((status, _)) if status.is_success() || status == StatusCode::NOT_FOUND => Ok(()),
            Ok((status, body)) => Err(AuthError::Server {
                status: status.as_u16(),
                code: body["code"].as_str().unwrap_or_default().to_owned(),
            }),
            Err(AuthError::WipeRequired) => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// The push service a device's token belongs to (ADR 0030).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PushProvider {
    /// Apple Push Notification service, on iOS.
    Apns,
    /// Firebase Cloud Messaging, on Android.
    Fcm,
}

fn server_result(status: StatusCode, body: &Value) -> Result<(), AuthError> {
    if status.is_success() {
        Ok(())
    } else {
        Err(AuthError::Server {
            status: status.as_u16(),
            code: body["code"].as_str().unwrap_or_default().to_owned(),
        })
    }
}

/// Where a [`DeviceSession`] hands each token response: the app stores the
/// new refresh token in the platform keychain, replacing the old one, and
/// passes `offline_limit_seconds` to the replica's engine. A session whose
/// sink fails keeps its new tokens in memory, so the device stays usable
/// until it restarts.
#[async_trait]
pub trait TokenSink: Send + Sync {
    async fn rotated(&self, tokens: &Tokens) -> Result<(), AuthError>;
}

struct Current {
    access: Option<(String, Instant)>,
    refresh: String,
}

/// The device's tokens for one workspace.
pub struct DeviceSession {
    client: TokenClient,
    workspace: Uuid,
    sink: Arc<dyn TokenSink>,
    /// Held across a refresh, so two callers never send the same refresh
    /// token: the second would revoke the device.
    current: tokio::sync::Mutex<Current>,
}

impl DeviceSession {
    /// After a sign-in: the tokens [`TokenClient::redeem`] returned, which
    /// the app has already handed to its sink.
    pub fn new(client: TokenClient, tokens: &Tokens, sink: Arc<dyn TokenSink>) -> Self {
        Self {
            client,
            workspace: tokens.workspace_id,
            sink,
            current: tokio::sync::Mutex::new(Current {
                access: Some((tokens.access_token.clone(), expiry(tokens))),
                refresh: tokens.refresh_token.clone(),
            }),
        }
    }

    /// At launch, from the stored refresh token: the first call refreshes.
    pub fn resume(
        client: TokenClient,
        workspace: Uuid,
        refresh_token: String,
        sink: Arc<dyn TokenSink>,
    ) -> Self {
        Self {
            client,
            workspace,
            sink,
            current: tokio::sync::Mutex::new(Current {
                access: None,
                refresh: refresh_token,
            }),
        }
    }

    pub fn client(&self) -> &TokenClient {
        &self.client
    }

    pub fn workspace(&self) -> Uuid {
        self.workspace
    }

    /// A current access token, refreshed first when it has run out or is
    /// about to.
    pub async fn access(&self) -> Result<String, AuthError> {
        let mut current = self.current.lock().await;
        if let Some((token, expires_at)) = &current.access {
            if Instant::now() + REFRESH_MARGIN < *expires_at {
                return Ok(token.clone());
            }
        }
        self.refresh_locked(&mut current).await
    }

    /// Refreshes now, as after the server refused the access token.
    pub async fn refresh(&self) -> Result<String, AuthError> {
        let mut current = self.current.lock().await;
        self.refresh_locked(&mut current).await
    }

    async fn refresh_locked(&self, current: &mut Current) -> Result<String, AuthError> {
        let tokens = self.client.refresh(&current.refresh).await?;
        if tokens.workspace_id != self.workspace {
            return Err(AuthError::Protocol(
                "the refreshed token is for another workspace".into(),
            ));
        }
        // The old refresh token is spent whatever the sink does next.
        current.refresh = tokens.refresh_token.clone();
        current.access = Some((tokens.access_token.clone(), expiry(&tokens)));
        if let Err(error) = self.sink.rotated(&tokens).await {
            tracing::warn!(%error, "the rotated refresh token was not stored");
        }
        Ok(tokens.access_token)
    }

    /// The current access token without refreshing, for signing out.
    pub async fn last_access(&self) -> Option<String> {
        self.current
            .lock()
            .await
            .access
            .as_ref()
            .map(|(token, _)| token.clone())
    }
}

fn expiry(tokens: &Tokens) -> Instant {
    Instant::now() + Duration::from_secs(tokens.expires_in)
}

#[async_trait]
impl AccessToken for DeviceSession {
    async fn access_token(&self) -> Result<String, SyncError> {
        self.access().await.map_err(SyncError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_follows_rfc_7636() {
        let pkce = Pkce::new();
        assert_eq!(pkce.verifier().len(), 43);
        assert_eq!(pkce.challenge().len(), 43);
        assert_eq!(
            pkce.challenge(),
            URL_SAFE_NO_PAD.encode(Sha256::digest(pkce.verifier().as_bytes()))
        );
        assert_ne!(Pkce::new().verifier(), pkce.verifier());
        assert!(!format!("{pkce:?}").contains(pkce.verifier()));
    }

    #[test]
    fn the_browser_gets_the_challenge_and_never_the_verifier() {
        let sign_in = SignIn::start(
            "https://limiara.example.com/some/page?x=1",
            "http://127.0.0.1:47185/callback",
        )
        .expect("sign-in");
        let url = Url::parse(&sign_in.browser_url()).expect("url");
        assert_eq!(
            url.origin().ascii_serialization(),
            "https://limiara.example.com"
        );
        assert_eq!(url.path(), "/auth/device/login");
        let query: std::collections::BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["redirect_uri"], "http://127.0.0.1:47185/callback");
        assert_eq!(query["code_challenge"], sign_in.pkce.challenge());
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["state"], sign_in.state);
        assert!(!sign_in.browser_url().contains(sign_in.pkce.verifier()));
    }

    #[test]
    fn only_this_sign_ins_redirect_yields_a_code() {
        let sign_in =
            SignIn::start("http://localhost:8080", "com.example.app:/oauth").expect("sign-in");
        let redirect =
            |query: &str| Url::parse(&format!("com.example.app:/oauth?{query}")).expect("url");
        let state = sign_in.state.clone();
        assert_eq!(
            sign_in
                .code(&redirect(&format!("code=abc&state={state}")))
                .expect("code"),
            "abc"
        );
        assert!(matches!(
            sign_in.code(&redirect("code=abc&state=another")),
            Err(AuthError::StateMismatch)
        ));
        assert!(matches!(
            sign_in.code(&redirect("code=abc")),
            Err(AuthError::StateMismatch)
        ));
        assert!(matches!(
            sign_in.code(&redirect(&format!("error=access_denied&state={state}"))),
            Err(AuthError::Refused(error)) if error == "access_denied"
        ));
    }

    #[test]
    fn servers_are_web_origins() {
        assert!(SignIn::start("limiara.example.com", "x:/y").is_err());
        assert!(SignIn::start("ftp://limiara.example.com", "x:/y").is_err());
        assert!(SignIn::start(" https://limiara.example.com ", "x:/y").is_ok());
    }
}
