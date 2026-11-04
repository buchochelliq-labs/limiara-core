//! Tells devices that their workspace changed (ADR 0030 §3, ADR 0024
//! "Continual sync").
//!
//! Discovery is the only cross-tenant step and uses the bounded
//! `platform_claim_device_notifications` routine, which also stamps each
//! claimed device so no device is told twice within the quiet window. Each
//! notification carries the workspace's id and nothing else. What the
//! provider accepted is recorded, and tokens it no longer knows are
//! forgotten, in the device's own workspace under forced RLS.

use futures_util::{stream, StreamExt};
use platform_notify::{
    apns::{ApnsConfig, ApnsNotifier, PRODUCTION_URL, SANDBOX_URL},
    fcm::{FcmConfig, FcmNotifier, ServiceAccount, API_URL},
    PushError, PushNotifiers, PushProvider,
};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

/// Devices claimed per pass.
pub const NOTIFY_BATCH: i32 = 200;
/// Notifications in flight at once within a pass.
const CONCURRENCY: usize = 16;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct NotifyPass {
    pub delivered: usize,
    /// Tokens the provider no longer knows, now forgotten.
    pub forgotten: usize,
    /// The provider was unavailable; tried again after the quiet window.
    pub unavailable: usize,
    /// The provider refused the deployment's request, or no credentials
    /// are configured for the device's provider.
    pub refused: usize,
    /// Recording the outcome failed; the device is claimed again after the
    /// quiet window.
    pub errors: usize,
}

impl NotifyPass {
    pub fn claimed(&self) -> usize {
        self.delivered + self.forgotten + self.unavailable + self.refused + self.errors
    }
}

enum Outcome {
    Delivered,
    Forgotten,
    Unavailable,
    Refused,
    Error,
}

/// The notifiers configured for this deployment. Credential files are read
/// once, at startup; a key that cannot be read or parsed stops the worker,
/// since it is a deployment error that retrying cannot fix.
pub fn push_notifiers(settings: &platform_config::PushSettings) -> anyhow::Result<PushNotifiers> {
    let mut notifiers = PushNotifiers::default();
    if let Some(apns) = &settings.apns {
        let key_pem = std::fs::read_to_string(&apns.key_file).map_err(|error| {
            anyhow::anyhow!("platform.devices.push.apns.key_file could not be read: {error}")
        })?;
        let notifier = ApnsNotifier::new(ApnsConfig {
            key_pem,
            key_id: apns.key_id.clone(),
            team_id: apns.team_id.clone(),
            topic: apns.topic.clone(),
            base_url: if apns.sandbox {
                SANDBOX_URL
            } else {
                PRODUCTION_URL
            }
            .to_owned(),
        })
        .map_err(|error| anyhow::anyhow!("platform.devices.push.apns: {error}"))?;
        notifiers = notifiers.with(Arc::new(notifier));
    }
    if let Some(fcm) = &settings.fcm {
        let file = std::fs::read_to_string(&fcm.service_account_file).map_err(|error| {
            anyhow::anyhow!(
                "platform.devices.push.fcm.service_account_file could not be read: {error}"
            )
        })?;
        let account: ServiceAccount = serde_json::from_str(&file).map_err(|_| {
            anyhow::anyhow!(
                "platform.devices.push.fcm.service_account_file is not a service account key"
            )
        })?;
        let notifier = FcmNotifier::new(FcmConfig {
            project_id: fcm.project_id.clone(),
            account,
            api_url: API_URL.to_owned(),
        })
        .map_err(|error| anyhow::anyhow!("platform.devices.push.fcm: {error}"))?;
        notifiers = notifiers.with(Arc::new(notifier));
    }
    Ok(notifiers)
}

/// Claims the devices that are due and notifies each one.
pub async fn run_pass(
    worker_pool: &PgPool,
    tenant_pool: &PgPool,
    notifiers: &PushNotifiers,
    quiet: Duration,
) -> Result<NotifyPass, sqlx::Error> {
    let quiet_seconds = i32::try_from(quiet.as_secs().max(1)).unwrap_or(i32::MAX);
    let due =
        platform_db::claim_device_notifications(worker_pool, NOTIFY_BATCH, quiet_seconds).await?;
    let outcomes: Vec<Outcome> = stream::iter(due)
        .map(|due| async move {
            let Some(provider) = PushProvider::parse(&due.push_provider) else {
                return Outcome::Refused;
            };
            match notifiers
                .workspace_changed(provider, &due.push_token, due.tenant_id)
                .await
            {
                Ok(()) => match platform_db::record_device_notification(
                    tenant_pool,
                    due.tenant_id,
                    due.device,
                    due.change_position,
                    due.metadata_version,
                )
                .await
                {
                    Ok(()) => Outcome::Delivered,
                    Err(error) => {
                        tracing::warn!(tenant_id = %due.tenant_id, device = %due.device, error = %error, "device notification could not be recorded");
                        Outcome::Error
                    }
                },
                Err(PushError::TokenInvalid) => match platform_db::forget_device_push_token(
                    tenant_pool,
                    due.tenant_id,
                    due.device,
                    &due.push_token,
                )
                .await
                {
                    Ok(_) => Outcome::Forgotten,
                    Err(error) => {
                        tracing::warn!(tenant_id = %due.tenant_id, device = %due.device, error = %error, "invalid push token could not be forgotten");
                        Outcome::Error
                    }
                },
                Err(PushError::Unavailable) => Outcome::Unavailable,
                Err(error @ (PushError::Rejected(_) | PushError::NotConfigured)) => {
                    tracing::warn!(provider = provider.as_str(), error = %error, "push notification refused");
                    Outcome::Refused
                }
            }
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut pass = NotifyPass::default();
    for outcome in outcomes {
        match outcome {
            Outcome::Delivered => pass.delivered += 1,
            Outcome::Forgotten => pass.forgotten += 1,
            Outcome::Unavailable => pass.unavailable += 1,
            Outcome::Refused => pass.refused += 1,
            Outcome::Error => pass.errors += 1,
        }
    }
    Ok(pass)
}

/// Runs notification passes for the life of the process: straight away
/// while a full batch was claimed, otherwise every `interval`.
pub async fn run(
    worker_pool: PgPool,
    tenant_pool: PgPool,
    notifiers: PushNotifiers,
    quiet: Duration,
    interval: Duration,
) {
    loop {
        let full = match run_pass(&worker_pool, &tenant_pool, &notifiers, quiet).await {
            Ok(pass) => {
                if pass.claimed() > 0 {
                    tracing::info!(
                        delivered = pass.delivered,
                        forgotten = pass.forgotten,
                        unavailable = pass.unavailable,
                        refused = pass.refused,
                        errors = pass.errors,
                        "device notification pass"
                    );
                }
                pass.claimed() >= NOTIFY_BATCH as usize
            }
            Err(error) => {
                tracing::warn!(error = %error, "device notification discovery failed");
                false
            }
        };
        if !full {
            tokio::time::sleep(interval).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_config::{ApnsSettings, FcmSettings, PushSettings};

    #[test]
    fn no_credentials_means_no_notifiers() {
        assert!(push_notifiers(&PushSettings::default())
            .expect("notifiers")
            .is_empty());
    }

    #[test]
    fn unreadable_or_malformed_keys_stop_startup() {
        let missing = PushSettings {
            apns: Some(ApnsSettings {
                key_file: "/nonexistent/AuthKey.p8".into(),
                key_id: "KEY".into(),
                team_id: "TEAM".into(),
                topic: "com.example.app".into(),
                sandbox: true,
            }),
            ..PushSettings::default()
        };
        let error = push_notifiers(&missing).err().expect("missing key file");
        assert!(error.to_string().contains("apns.key_file"), "{error}");

        let directory = std::env::temp_dir().join(format!("push-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&directory).expect("directory");
        let account = directory.join("account.json");
        std::fs::write(&account, "{\"not\": \"an account\"}").expect("account");
        let malformed = PushSettings {
            fcm: Some(FcmSettings {
                project_id: "project".into(),
                service_account_file: account.display().to_string(),
            }),
            ..PushSettings::default()
        };
        let error = push_notifiers(&malformed).err().expect("malformed account");
        assert!(
            error.to_string().contains("not a service account key"),
            "{error}"
        );
        std::fs::remove_dir_all(&directory).expect("cleanup");
    }
}
