//! Firebase Cloud Messaging (FCM) over its HTTP v1 API.
//!
//! Each notification is a high-priority data message with only the
//! workspace's id, which wakes the app to sync. The access token comes from
//! the deployment's service account: an RS256 assertion exchanged at the
//! account's token endpoint (RFC 7523) and reused until shortly before it
//! expires.

use crate::{http_client, jwt, pem_der, unix_now, PushError, PushNotifier, PushProvider};
use async_trait::async_trait;
use ring::{rand::SystemRandom, signature::RsaKeyPair};
use serde::Deserialize;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

pub const API_URL: &str = "https://fcm.googleapis.com";
const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
const ASSERTION_LIFETIME: u64 = 3600;
/// Renew an access token this long before it expires.
const RENEW_MARGIN: Duration = Duration::from_secs(120);

/// The fields of a Google service account key file the adapter uses.
#[derive(Clone, Deserialize)]
pub struct ServiceAccount {
    pub client_email: String,
    /// PKCS#8 PEM.
    pub private_key: String,
    pub token_uri: String,
}

/// The deployment's FCM credentials.
#[derive(Clone)]
pub struct FcmConfig {
    pub project_id: String,
    pub account: ServiceAccount,
    /// [`API_URL`].
    pub api_url: String,
}

pub struct FcmNotifier {
    config: FcmConfig,
    key: RsaKeyPair,
    random: SystemRandom,
    client: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

impl FcmNotifier {
    pub fn new(config: FcmConfig) -> Result<Self, PushError> {
        let refused =
            || PushError::Rejected("the FCM service account key is not a PKCS#8 RSA key".into());
        let der = pem_der(&config.account.private_key).ok_or_else(refused)?;
        let key = RsaKeyPair::from_pkcs8(&der).map_err(|_| refused())?;
        Ok(Self {
            config,
            key,
            random: SystemRandom::new(),
            client: http_client()?,
            token: Mutex::new(None),
        })
    }

    /// A signed assertion for the token endpoint, issued at `issued_at`.
    fn assertion(&self, issued_at: u64) -> Result<String, PushError> {
        let account = &self.config.account;
        let input = jwt::signing_input(
            &json!({"alg": "RS256", "typ": "JWT"}),
            &json!({
                "iss": account.client_email,
                "scope": SCOPE,
                "aud": account.token_uri,
                "iat": issued_at,
                "exp": issued_at + ASSERTION_LIFETIME,
            }),
        );
        let mut signature = vec![0; self.key.public().modulus_len()];
        self.key
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &self.random,
                input.as_bytes(),
                &mut signature,
            )
            .map_err(|_| PushError::Rejected("could not sign the FCM assertion".into()))?;
        Ok(jwt::compact(&input, &signature))
    }

    /// The cached access token, or a new one. `rejected` is a token FCM
    /// refused: it is replaced, but a different token another send already
    /// renewed is reused, so a burst of refusals costs one exchange.
    async fn access_token(&self, rejected: Option<&str>) -> Result<String, PushError> {
        let mut cached = self.token.lock().await;
        if let Some((token, expires)) = cached.as_ref() {
            let fresh = Instant::now() + RENEW_MARGIN < *expires;
            if fresh && rejected != Some(token.as_str()) {
                return Ok(token.clone());
            }
        }
        let assertion = self.assertion(unix_now())?;
        let response = self
            .client
            .post(&self.config.account.token_uri)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", assertion.as_str()),
            ])
            .send()
            .await
            .map_err(|_| PushError::Unavailable)?;
        let status = response.status();
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(PushError::Unavailable);
        }
        if !status.is_success() {
            return Err(PushError::Rejected(format!(
                "the FCM token endpoint refused the service account ({status})"
            )));
        }
        let issued: TokenResponse = response
            .json()
            .await
            .map_err(|_| PushError::Rejected("the FCM token response is malformed".into()))?;
        let expires = Instant::now() + Duration::from_secs(issued.expires_in);
        *cached = Some((issued.access_token.clone(), expires));
        Ok(issued.access_token)
    }

    async fn send(
        &self,
        token: &str,
        workspace_id: Uuid,
        rejected: Option<&str>,
    ) -> Result<(), Attempt> {
        let access = self.access_token(rejected).await.map_err(Attempt::Failed)?;
        let url = format!(
            "{}/v1/projects/{}/messages:send",
            self.config.api_url.trim_end_matches('/'),
            self.config.project_id
        );
        let response = self
            .client
            .post(url)
            .bearer_auth(&access)
            .json(&message(token, workspace_id))
            .send()
            .await
            .map_err(|_| Attempt::Failed(PushError::Unavailable))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let body = response
            .json::<serde_json::Value>()
            .await
            .unwrap_or_default();
        Err(match classify(status.as_u16(), &body) {
            Attempt::StaleToken(_) => Attempt::StaleToken(access),
            other => other,
        })
    }
}

/// The message: the device's token and which workspace. Nothing else.
pub(crate) fn message(token: &str, workspace_id: Uuid) -> serde_json::Value {
    let workspace = workspace_id.to_string();
    json!({"message": {
        "token": token,
        "data": {"workspace": workspace},
        "android": {"priority": "high", "collapse_key": workspace, "ttl": "3600s"},
    }})
}

enum Attempt {
    /// This access token was refused: get a new one and retry once.
    StaleToken(String),
    Failed(PushError),
}

/// FCM's documented errors. Only `UNREGISTERED` names the device token
/// itself, so only it forgets the token: a generic `400` or `404` can come
/// from a wrong project or a malformed request, and must not erase every
/// device's registration.
fn classify(status: u16, body: &serde_json::Value) -> Attempt {
    let error = &body["error"];
    let code = error["details"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|detail| detail["errorCode"].as_str())
        .or_else(|| error["status"].as_str())
        .unwrap_or_default();
    match (status, code) {
        (_, "UNREGISTERED") => Attempt::Failed(PushError::TokenInvalid),
        (401, _) => Attempt::StaleToken(String::new()),
        (429, _) | (500..=599, _) => Attempt::Failed(PushError::Unavailable),
        (_, code) => Attempt::Failed(PushError::Rejected(format!("FCM {status} {code}"))),
    }
}

#[async_trait]
impl PushNotifier for FcmNotifier {
    fn provider(&self) -> PushProvider {
        PushProvider::Fcm
    }

    async fn workspace_changed(&self, token: &str, workspace_id: Uuid) -> Result<(), PushError> {
        if token.is_empty() || token.len() > 4096 {
            return Err(PushError::TokenInvalid);
        }
        match self.send(token, workspace_id, None).await {
            Ok(()) => Ok(()),
            Err(Attempt::Failed(error)) => Err(error),
            Err(Attempt::StaleToken(rejected)) => {
                match self.send(token, workspace_id, Some(&rejected)).await {
                    Ok(()) => Ok(()),
                    Err(Attempt::Failed(error)) => Err(error),
                    Err(Attempt::StaleToken(_)) => Err(PushError::Rejected(
                        "FCM refused a fresh access token".into(),
                    )),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::State, http::HeaderMap, routing::post, Form, Json, Router};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex as StdMutex};

    /// A 2048-bit RSA key, PKCS#8 PEM, generated once per test run so no
    /// private key is kept in the repository.
    fn test_key() -> &'static str {
        static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
        KEY.get_or_init(|| {
            use rsa::pkcs8::{EncodePrivateKey, LineEnding};
            let mut random = rsa::rand_core::OsRng;
            rsa::RsaPrivateKey::new(&mut random, 2048)
                .expect("test key")
                .to_pkcs8_pem(LineEnding::LF)
                .expect("pem")
                .to_string()
        })
    }

    #[derive(Default)]
    struct Seen {
        assertions: StdMutex<Vec<String>>,
        messages: StdMutex<Vec<(String, serde_json::Value)>>,
    }

    async fn token(
        State(seen): State<Arc<Seen>>,
        Form(form): Form<HashMap<String, String>>,
    ) -> Json<serde_json::Value> {
        assert_eq!(
            form.get("grant_type").map(String::as_str),
            Some("urn:ietf:params:oauth:grant-type:jwt-bearer")
        );
        let mut assertions = seen.assertions.lock().expect("lock");
        assertions.push(form.get("assertion").cloned().unwrap_or_default());
        Json(json!({"access_token": format!("access-{}", assertions.len()), "expires_in": 3600}))
    }

    async fn send(
        State(seen): State<Arc<Seen>>,
        headers: HeaderMap,
        Json(body): Json<serde_json::Value>,
    ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
        let authorization = headers["authorization"]
            .to_str()
            .expect("header")
            .to_owned();
        let device = body["message"]["token"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        seen.messages
            .lock()
            .expect("lock")
            .push((authorization.clone(), body));
        if authorization == "Bearer access-1" {
            return (
                axum::http::StatusCode::UNAUTHORIZED,
                Json(json!({"error": {"status": "UNAUTHENTICATED"}})),
            );
        }
        match device.as_str() {
            "gone" => (
                axum::http::StatusCode::NOT_FOUND,
                Json(
                    json!({"error": {"status": "NOT_FOUND", "details": [{"errorCode": "UNREGISTERED"}]}}),
                ),
            ),
            "busy" => (axum::http::StatusCode::SERVICE_UNAVAILABLE, Json(json!({}))),
            // A route-level 404, such as a wrong project: no token detail.
            "misrouted" => (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error": {"status": "NOT_FOUND"}})),
            ),
            "malformed" => (
                axum::http::StatusCode::BAD_REQUEST,
                Json(json!({"error": {"status": "INVALID_ARGUMENT"}})),
            ),
            _ => (
                axum::http::StatusCode::OK,
                Json(json!({"name": "projects/p/messages/1"})),
            ),
        }
    }

    async fn serve(seen: Arc<Seen>) -> String {
        let app = Router::new()
            .route("/token", post(token))
            .route("/v1/projects/demo/messages:send", post(send))
            .with_state(seen);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        format!("http://{address}")
    }

    fn notifier(base: &str) -> FcmNotifier {
        FcmNotifier::new(FcmConfig {
            project_id: "demo".into(),
            account: ServiceAccount {
                client_email: "push@demo.iam.gserviceaccount.com".into(),
                private_key: test_key().into(),
                token_uri: format!("{base}/token"),
            },
            api_url: base.into(),
        })
        .expect("notifier")
    }

    #[test]
    fn assertions_are_rs256_jwts_for_the_messaging_scope() {
        let notifier = notifier("https://oauth2.example");
        let assertion = notifier.assertion(1_700_000_000).expect("assertion");
        let parts: Vec<&str> = assertion.split('.').collect();
        assert_eq!(parts.len(), 3);
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).expect("claims"))
                .expect("json");
        assert_eq!(
            claims,
            json!({
                "iss": "push@demo.iam.gserviceaccount.com",
                "scope": SCOPE,
                "aud": "https://oauth2.example/token",
                "iat": 1_700_000_000,
                "exp": 1_700_003_600,
            })
        );
        let public = ring::signature::UnparsedPublicKey::new(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            notifier.key.public().as_ref().to_vec(),
        );
        public
            .verify(
                format!("{}.{}", parts[0], parts[1]).as_bytes(),
                &URL_SAFE_NO_PAD.decode(parts[2]).expect("signature"),
            )
            .expect("valid signature");
    }

    #[test]
    fn messages_carry_only_the_device_token_and_workspace() {
        let workspace = Uuid::now_v7();
        let id = workspace.to_string();
        assert_eq!(
            message("device", workspace),
            json!({"message": {
                "token": "device",
                "data": {"workspace": id},
                "android": {"priority": "high", "collapse_key": id, "ttl": "3600s"},
            }})
        );
    }

    #[tokio::test]
    async fn notifications_renew_a_refused_token_and_classify_failures() {
        let seen = Arc::new(Seen::default());
        let base = serve(seen.clone()).await;
        let notifier = notifier(&base);
        let workspace = Uuid::now_v7();
        notifier
            .workspace_changed("device", workspace)
            .await
            .expect("delivered after renewing the access token");
        assert_eq!(seen.assertions.lock().expect("lock").len(), 2);
        // The cached token serves the next message without a new assertion.
        notifier
            .workspace_changed("device", workspace)
            .await
            .expect("delivered");
        assert_eq!(seen.assertions.lock().expect("lock").len(), 2);
        assert_eq!(
            notifier.workspace_changed("gone", workspace).await,
            Err(PushError::TokenInvalid)
        );
        assert_eq!(
            notifier.workspace_changed("busy", workspace).await,
            Err(PushError::Unavailable)
        );
        let messages = seen.messages.lock().expect("lock");
        assert_eq!(messages[1].0, "Bearer access-2");
        assert_eq!(messages[1].1, message("device", workspace));
    }

    #[tokio::test]
    async fn generic_client_errors_keep_the_device_token() {
        let seen = Arc::new(Seen::default());
        let base = serve(seen.clone()).await;
        let notifier = notifier(&base);
        let workspace = Uuid::now_v7();
        for device in ["misrouted", "malformed"] {
            assert!(matches!(
                notifier.workspace_changed(device, workspace).await,
                Err(PushError::Rejected(_))
            ));
        }
    }

    #[tokio::test]
    async fn a_burst_of_refusals_renews_the_access_token_once() {
        let seen = Arc::new(Seen::default());
        let base = serve(seen.clone()).await;
        let notifier = Arc::new(notifier(&base));
        let workspace = Uuid::now_v7();
        let sends: Vec<_> = (0..5)
            .map(|_| {
                let notifier = notifier.clone();
                tokio::spawn(async move { notifier.workspace_changed("device", workspace).await })
            })
            .collect();
        for send in sends {
            send.await.expect("join").expect("delivered");
        }
        // One assertion for the first token, one for its single renewal.
        assert_eq!(seen.assertions.lock().expect("lock").len(), 2);
    }
}
