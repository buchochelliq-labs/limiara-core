#![forbid(unsafe_code)]
//! The kernel's push notification port (ADR 0030, ADR 0024 "Continual
//! sync"). A device app that is suspended learns that its workspace changed
//! through Apple's or Google's push service, then wakes and pulls.
//!
//! The only message is "workspace changed", carrying the workspace's id and
//! nothing else: no record names, values, counts or member names. Apple and
//! Google therefore see which devices use which workspace and when it
//! changes, and no customer data.
//!
//! - [`apns`] sends background notifications through Apple's HTTP/2 API with
//!   an ES256 provider token.
//! - [`fcm`] sends data messages through Firebase Cloud Messaging's HTTP v1
//!   API with a service account's OAuth token.

pub mod apns;
pub mod fcm;
mod jwt;

use async_trait::async_trait;
use thiserror::Error;
use uuid::Uuid;

/// Where a device's notifications go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushProvider {
    Apns,
    Fcm,
}

impl PushProvider {
    /// The provider's name as devices register it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Apns => "apns",
            Self::Fcm => "fcm",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "apns" => Some(Self::Apns),
            "fcm" => Some(Self::Fcm),
            _ => None,
        }
    }
}

/// Why a notification was not delivered.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum PushError {
    /// The provider no longer knows the device token: the app was removed
    /// or the token was replaced. The caller forgets the token.
    #[error("the device token is no longer valid")]
    TokenInvalid,
    /// The provider is unreachable, overloaded or failing; worth trying
    /// again later.
    #[error("the push provider is unavailable")]
    Unavailable,
    /// The provider refused the request, for example because the
    /// deployment's credentials are wrong. Retrying will not help until the
    /// configuration changes.
    #[error("the push provider rejected the request: {0}")]
    Rejected(String),
    /// No credentials are configured for this provider.
    #[error("the push provider is not configured")]
    NotConfigured,
}

/// Sends content-free notifications to one provider.
#[async_trait]
pub trait PushNotifier: Send + Sync {
    fn provider(&self) -> PushProvider;

    /// Tells the device holding `token` that `workspace_id` changed.
    async fn workspace_changed(&self, token: &str, workspace_id: Uuid) -> Result<(), PushError>;
}

/// The notifiers a deployment has configured, by provider.
#[derive(Default, Clone)]
pub struct PushNotifiers {
    apns: Option<std::sync::Arc<dyn PushNotifier>>,
    fcm: Option<std::sync::Arc<dyn PushNotifier>>,
}

impl PushNotifiers {
    pub fn with(mut self, notifier: std::sync::Arc<dyn PushNotifier>) -> Self {
        match notifier.provider() {
            PushProvider::Apns => self.apns = Some(notifier),
            PushProvider::Fcm => self.fcm = Some(notifier),
        }
        self
    }

    pub fn is_empty(&self) -> bool {
        self.apns.is_none() && self.fcm.is_none()
    }

    /// Sends through the provider the device registered with.
    pub async fn workspace_changed(
        &self,
        provider: PushProvider,
        token: &str,
        workspace_id: Uuid,
    ) -> Result<(), PushError> {
        let notifier = match provider {
            PushProvider::Apns => self.apns.as_ref(),
            PushProvider::Fcm => self.fcm.as_ref(),
        };
        match notifier {
            Some(notifier) => notifier.workspace_changed(token, workspace_id).await,
            None => Err(PushError::NotConfigured),
        }
    }
}

/// An HTTP client for the providers: HTTP/2 where the server offers it, a
/// bounded timeout, and no redirects, since neither provider redirects.
fn http_client() -> Result<reqwest::Client, PushError> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| PushError::Rejected(format!("HTTP client: {error}")))
}

/// Seconds since the Unix epoch, for token claims.
fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// The DER bytes of a PEM document's first block.
fn pem_der(pem: &str) -> Option<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let body: String = pem
        .lines()
        .map(str::trim)
        .skip_while(|line| !line.starts_with("-----BEGIN "))
        .skip(1)
        .take_while(|line| !line.starts_with("-----END "))
        .collect();
    STANDARD.decode(body).ok().filter(|der| !der.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Fixed(PushProvider);

    #[async_trait]
    impl PushNotifier for Fixed {
        fn provider(&self) -> PushProvider {
            self.0
        }

        async fn workspace_changed(&self, token: &str, _: Uuid) -> Result<(), PushError> {
            if token == "gone" {
                Err(PushError::TokenInvalid)
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn notifiers_route_by_provider_and_refuse_unconfigured_ones() {
        let notifiers = PushNotifiers::default().with(Arc::new(Fixed(PushProvider::Apns)));
        let workspace = Uuid::now_v7();
        assert_eq!(
            notifiers
                .workspace_changed(PushProvider::Apns, "token", workspace)
                .await,
            Ok(())
        );
        assert_eq!(
            notifiers
                .workspace_changed(PushProvider::Apns, "gone", workspace)
                .await,
            Err(PushError::TokenInvalid)
        );
        assert_eq!(
            notifiers
                .workspace_changed(PushProvider::Fcm, "token", workspace)
                .await,
            Err(PushError::NotConfigured)
        );
        assert!(!notifiers.is_empty());
        assert!(PushNotifiers::default().is_empty());
        assert_eq!(PushProvider::parse("fcm"), Some(PushProvider::Fcm));
        assert_eq!(PushProvider::parse("sms"), None);
    }

    #[test]
    fn pem_blocks_decode_to_der() {
        let pem = "junk\n-----BEGIN PRIVATE KEY-----\nAQID\nBA==\n-----END PRIVATE KEY-----\n";
        assert_eq!(pem_der(pem), Some(vec![1, 2, 3, 4]));
        assert_eq!(pem_der("no pem here"), None);
    }
}
