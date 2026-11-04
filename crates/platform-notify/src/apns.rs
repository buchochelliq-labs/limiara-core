//! Apple Push Notification service (APNs) over its HTTP/2 API.
//!
//! Each notification is a background push (`content-available`, priority
//! 5) that wakes the app to sync. The provider token is an ES256 JWT signed
//! with the deployment's `.p8` key and reused for 50 minutes, inside Apple's
//! 20 to 60 minute window.

use crate::{http_client, jwt, unix_now, PushError, PushNotifier, PushProvider};
use async_trait::async_trait;
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::DecodePrivateKey;
use serde_json::json;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

pub const PRODUCTION_URL: &str = "https://api.push.apple.com";
pub const SANDBOX_URL: &str = "https://api.sandbox.push.apple.com";
const TOKEN_REUSE: Duration = Duration::from_secs(50 * 60);
/// How long Apple keeps trying an undelivered notification.
const EXPIRATION_SECONDS: u64 = 3600;

/// The deployment's APNs credentials.
#[derive(Clone)]
pub struct ApnsConfig {
    /// The `.p8` signing key, PKCS#8 PEM.
    pub key_pem: String,
    pub key_id: String,
    pub team_id: String,
    /// The app's bundle id.
    pub topic: String,
    /// [`PRODUCTION_URL`] or [`SANDBOX_URL`].
    pub base_url: String,
}

pub struct ApnsNotifier {
    config: ApnsConfig,
    key: SigningKey,
    client: reqwest::Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl ApnsNotifier {
    pub fn new(config: ApnsConfig) -> Result<Self, PushError> {
        let key = SigningKey::from_pkcs8_pem(&config.key_pem)
            .map_err(|_| PushError::Rejected("the APNs key is not a PKCS#8 P-256 key".into()))?;
        Ok(Self {
            config,
            key,
            client: http_client()?,
            token: Mutex::new(None),
        })
    }

    /// A provider token signed at `issued_at`.
    fn sign(&self, issued_at: u64) -> String {
        let input = jwt::signing_input(
            &json!({"alg": "ES256", "kid": self.config.key_id}),
            &json!({"iss": self.config.team_id, "iat": issued_at}),
        );
        let signature: Signature = self.key.sign(input.as_bytes());
        jwt::compact(&input, &signature.to_bytes())
    }

    async fn provider_token(&self, renew: bool) -> String {
        let mut cached = self.token.lock().await;
        match cached.as_ref() {
            Some((token, issued)) if !renew && issued.elapsed() < TOKEN_REUSE => token.clone(),
            _ => {
                let token = self.sign(unix_now());
                *cached = Some((token.clone(), Instant::now()));
                token
            }
        }
    }

    async fn send(&self, token: &str, workspace_id: Uuid, renew: bool) -> Result<(), Attempt> {
        let provider_token = self.provider_token(renew).await;
        let url = format!(
            "{}/3/device/{}",
            self.config.base_url.trim_end_matches('/'),
            token
        );
        let response = self
            .client
            .post(url)
            .bearer_auth(provider_token)
            .header("apns-push-type", "background")
            .header("apns-priority", "5")
            .header("apns-topic", &self.config.topic)
            .header(
                "apns-expiration",
                (unix_now() + EXPIRATION_SECONDS).to_string(),
            )
            // A newer notification for the workspace replaces an undelivered one.
            .header("apns-collapse-id", workspace_id.to_string())
            .json(&payload(workspace_id))
            .send()
            .await
            .map_err(|_| Attempt::Failed(PushError::Unavailable))?;
        let status = response.status();
        if status.is_success() {
            return Ok(());
        }
        let reason = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| {
                body.get("reason")
                    .and_then(|reason| reason.as_str())
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        Err(classify(status.as_u16(), &reason))
    }
}

/// The notification body: wake up, and which workspace. Nothing else.
pub(crate) fn payload(workspace_id: Uuid) -> serde_json::Value {
    json!({"aps": {"content-available": 1}, "workspace": workspace_id})
}

enum Attempt {
    /// The provider token expired or was refused: sign a new one and retry
    /// once.
    StaleToken,
    Failed(PushError),
}

/// Apple's documented status codes and reasons.
fn classify(status: u16, reason: &str) -> Attempt {
    match (status, reason) {
        (410, _) | (400, "BadDeviceToken" | "DeviceTokenNotForTopic") => {
            Attempt::Failed(PushError::TokenInvalid)
        }
        (403, "ExpiredProviderToken" | "InvalidProviderToken") => Attempt::StaleToken,
        (429, _) | (500..=599, _) => Attempt::Failed(PushError::Unavailable),
        (_, reason) => Attempt::Failed(PushError::Rejected(format!("APNs {status} {reason}"))),
    }
}

#[async_trait]
impl PushNotifier for ApnsNotifier {
    fn provider(&self) -> PushProvider {
        PushProvider::Apns
    }

    async fn workspace_changed(&self, token: &str, workspace_id: Uuid) -> Result<(), PushError> {
        if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(PushError::TokenInvalid);
        }
        match self.send(token, workspace_id, false).await {
            Ok(()) => Ok(()),
            Err(Attempt::Failed(error)) => Err(error),
            Err(Attempt::StaleToken) => match self.send(token, workspace_id, true).await {
                Ok(()) => Ok(()),
                Err(Attempt::Failed(error)) => Err(error),
                Err(Attempt::StaleToken) => Err(PushError::Rejected(
                    "APNs refused a freshly signed provider token".into(),
                )),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::Path, http::HeaderMap, routing::post, Json, Router};
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use p256::ecdsa::{signature::Verifier, VerifyingKey};
    use p256::pkcs8::{EncodePrivateKey, LineEnding};
    use std::sync::Arc;

    fn key_pem() -> (String, VerifyingKey) {
        let key = SigningKey::from_slice(&[7u8; 32]).expect("key");
        let pem = key.to_pkcs8_pem(LineEnding::LF).expect("pem").to_string();
        (pem, *key.verifying_key())
    }

    #[test]
    fn provider_tokens_are_es256_jwts_apple_can_verify() {
        let (pem, verifying) = key_pem();
        let notifier = ApnsNotifier::new(ApnsConfig {
            key_pem: pem,
            key_id: "KEY123".into(),
            team_id: "TEAM456".into(),
            topic: "com.example.app".into(),
            base_url: SANDBOX_URL.into(),
        })
        .expect("notifier");
        let token = notifier.sign(1_700_000_000);
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        let header: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).expect("header"))
                .expect("json");
        assert_eq!(header, json!({"alg": "ES256", "kid": "KEY123"}));
        let claims: serde_json::Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).expect("claims"))
                .expect("json");
        assert_eq!(claims, json!({"iss": "TEAM456", "iat": 1_700_000_000}));
        let signature = Signature::from_slice(&URL_SAFE_NO_PAD.decode(parts[2]).expect("sig"))
            .expect("raw r||s signature");
        verifying
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("valid signature");
        assert!(ApnsNotifier::new(ApnsConfig {
            key_pem: "not a key".into(),
            key_id: String::new(),
            team_id: String::new(),
            topic: String::new(),
            base_url: String::new(),
        })
        .is_err());
    }

    #[test]
    fn payloads_carry_only_the_workspace() {
        let workspace = Uuid::now_v7();
        assert_eq!(
            payload(workspace),
            json!({"aps": {"content-available": 1}, "workspace": workspace.to_string()})
        );
    }

    #[derive(Default)]
    struct Seen {
        requests: std::sync::Mutex<Vec<(String, HeaderMap, serde_json::Value)>>,
    }

    /// A stand-in for Apple's API: refuses the first provider token as
    /// expired, forgets one device and accepts the rest.
    async fn serve(seen: Arc<Seen>) -> String {
        let handler = move |Path(token): Path<String>,
                            headers: HeaderMap,
                            Json(body): Json<serde_json::Value>| {
            let seen = seen.clone();
            async move {
                let first = seen.requests.lock().expect("lock").is_empty();
                seen.requests
                    .lock()
                    .expect("lock")
                    .push((token.clone(), headers, body));
                if first {
                    (
                        axum::http::StatusCode::FORBIDDEN,
                        Json(json!({"reason": "ExpiredProviderToken"})),
                    )
                } else if token == "dead" {
                    (
                        axum::http::StatusCode::GONE,
                        Json(json!({"reason": "Unregistered"})),
                    )
                } else if token == "beef" {
                    (
                        axum::http::StatusCode::SERVICE_UNAVAILABLE,
                        Json(json!({"reason": "ServiceUnavailable"})),
                    )
                } else {
                    (axum::http::StatusCode::OK, Json(json!({})))
                }
            }
        };
        let app = Router::new().route("/3/device/:token", post(handler));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        format!("http://{address}")
    }

    #[tokio::test]
    async fn notifications_renew_a_refused_token_and_classify_failures() {
        let seen = Arc::new(Seen::default());
        let base_url = serve(seen.clone()).await;
        let (pem, _) = key_pem();
        let notifier = ApnsNotifier::new(ApnsConfig {
            key_pem: pem,
            key_id: "KEY123".into(),
            team_id: "TEAM456".into(),
            topic: "com.example.app".into(),
            base_url,
        })
        .expect("notifier");
        let workspace = Uuid::now_v7();
        notifier
            .workspace_changed("abcd", workspace)
            .await
            .expect("delivered after renewing the provider token");
        {
            let requests = seen.requests.lock().expect("lock");
            assert_eq!(requests.len(), 2);
            let (token, headers, body) = &requests[1];
            assert_eq!(token, "abcd");
            assert_eq!(headers["apns-push-type"], "background");
            assert_eq!(headers["apns-priority"], "5");
            assert_eq!(headers["apns-topic"], "com.example.app");
            assert_eq!(headers["apns-collapse-id"], workspace.to_string().as_str());
            assert!(headers["authorization"]
                .to_str()
                .expect("header")
                .starts_with("Bearer "));
            assert_eq!(body, &payload(workspace));
        }
        assert_eq!(
            notifier.workspace_changed("dead", workspace).await,
            Err(PushError::TokenInvalid)
        );
        assert_eq!(
            notifier.workspace_changed("beef", workspace).await,
            Err(PushError::Unavailable)
        );
        assert_eq!(
            notifier.workspace_changed("not/hex", workspace).await,
            Err(PushError::TokenInvalid)
        );
    }
}
