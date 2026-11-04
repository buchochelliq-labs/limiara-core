//! Real-PostgreSQL acceptance for the device notification dispatcher (ADR
//! 0030 §3): the worker claims due devices across workspaces through its
//! bounded routine only, sends each one a content-free notification at most
//! once per quiet window, records what it told the device in the device's
//! workspace, and forgets tokens the provider no longer knows. Revoked
//! devices and devices whose member left are never notified. Run with
//! `bash scripts/test-plugin-runtime.sh`.

use async_trait::async_trait;
use platform_notify::{PushError, PushNotifier, PushNotifiers, PushProvider};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;
use worker::device_notifications::{run_pass, NotifyPass};

const QUIET: Duration = Duration::from_secs(180);

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is set"));
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect {variable}: {error}"))
}

/// Records what it was asked to send. Token "gone" is unknown to the
/// provider and "busy" finds it unavailable.
struct Recorder {
    provider: PushProvider,
    sent: Arc<Mutex<Vec<(String, Uuid)>>>,
}

#[async_trait]
impl PushNotifier for Recorder {
    fn provider(&self) -> PushProvider {
        self.provider
    }

    async fn workspace_changed(&self, token: &str, workspace_id: Uuid) -> Result<(), PushError> {
        self.sent
            .lock()
            .expect("sent")
            .push((token.to_owned(), workspace_id));
        match token {
            "gone" => Err(PushError::TokenInvalid),
            "busy" => Err(PushError::Unavailable),
            _ => Ok(()),
        }
    }
}

async fn workspace(migrator: &PgPool, name: &str, position: i64, metadata: i64) -> (Uuid, Uuid) {
    let tenant = Uuid::now_v7();
    let user = Uuid::now_v7();
    for statement in [
        "INSERT INTO tenants (id, display_name) VALUES ($1, $3)",
        "INSERT INTO users (id, display_name) VALUES ($2, $3)",
        "INSERT INTO memberships (tenant_id, user_id) VALUES ($1, $2)",
    ] {
        sqlx::query(statement)
            .bind(tenant)
            .bind(user)
            .bind(name)
            .execute(migrator)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    set_counters(migrator, tenant, position, metadata).await;
    (tenant, user)
}

async fn set_counters(migrator: &PgPool, tenant: Uuid, position: i64, metadata: i64) {
    sqlx::query(
        "INSERT INTO change_counters (tenant_id, last_position, metadata_version) VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id) DO UPDATE SET last_position = $2, metadata_version = $3",
    )
    .bind(tenant)
    .bind(position)
    .bind(metadata)
    .execute(migrator)
    .await
    .expect("counters");
}

async fn device(
    migrator: &PgPool,
    tenant: Uuid,
    user: Uuid,
    push: Option<(&str, &str)>,
    revoked: bool,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO devices (tenant_id, id, user_id, name, platform, push_provider, push_token, \
         created_at, last_seen_at, revoked_at, revoked_reason) \
         VALUES ($1, $2, $3, 'Phone', 'ios', $4, $5, now(), now(), \
         CASE WHEN $6 THEN now() END, CASE WHEN $6 THEN 'revoked' END)",
    )
    .bind(tenant)
    .bind(id)
    .bind(user)
    .bind(push.map(|(provider, _)| provider))
    .bind(push.map(|(_, token)| token))
    .bind(revoked)
    .execute(migrator)
    .await
    .expect("device");
    id
}

async fn told(migrator: &PgPool, tenant: Uuid, id: Uuid) -> (i64, i64, Option<String>) {
    sqlx::query_as(
        "SELECT notified_position, notified_metadata_version, push_token FROM devices \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .fetch_one(migrator)
    .await
    .expect("device")
}

/// Moves a device's last claim back past the quiet window.
async fn age_claim(migrator: &PgPool, tenant: Uuid, id: Uuid) {
    sqlx::query(
        "UPDATE devices SET notified_at = now() - interval '10 minutes' \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant)
    .bind(id)
    .execute(migrator)
    .await
    .expect("age");
}

fn take(sent: &Arc<Mutex<Vec<(String, Uuid)>>>) -> Vec<(String, Uuid)> {
    let mut sent = std::mem::take(&mut *sent.lock().expect("sent"));
    sent.sort();
    sent
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn the_worker_tells_each_device_once_per_quiet_window() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let sent = Arc::new(Mutex::new(Vec::new()));
    let notifiers = PushNotifiers::default()
        .with(Arc::new(Recorder {
            provider: PushProvider::Apns,
            sent: sent.clone(),
        }))
        .with(Arc::new(Recorder {
            provider: PushProvider::Fcm,
            sent: sent.clone(),
        }));

    let (north, north_member) = workspace(&migrator, "North", 5, 1).await;
    let (south, south_member) = workspace(&migrator, "South", 3, 0).await;
    let (quiet, quiet_member) = workspace(&migrator, "Quiet", 0, 0).await;
    let phone = device(
        &migrator,
        north,
        north_member,
        Some(("apns", "phone")),
        false,
    )
    .await;
    let revoked = device(
        &migrator,
        north,
        north_member,
        Some(("apns", "revoked")),
        true,
    )
    .await;
    let silent = device(&migrator, north, north_member, None, false).await;
    let leaver = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'Leaver')")
        .bind(leaver)
        .execute(&migrator)
        .await
        .expect("leaver");
    let left = device(&migrator, north, leaver, Some(("apns", "left")), false).await;
    let gone = device(&migrator, south, south_member, Some(("fcm", "gone")), false).await;
    let busy = device(&migrator, south, south_member, Some(("fcm", "busy")), false).await;
    let unchanged = device(
        &migrator,
        quiet,
        quiet_member,
        Some(("apns", "unchanged")),
        false,
    )
    .await;

    // The worker role reads no devices directly; it only claims them.
    let direct = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM devices")
        .fetch_one(&worker)
        .await;
    assert!(
        direct.is_err() || direct.is_ok_and(|count| count == 0),
        "the worker role sees no devices without a workspace"
    );

    // Other workspaces' devices may be due too; only these are asserted.
    let mut ours = run_pass(&worker, &app, &notifiers, QUIET)
        .await
        .expect("pass");
    while ours.claimed() == worker::device_notifications::NOTIFY_BATCH as usize {
        ours = run_pass(&worker, &app, &notifiers, QUIET)
            .await
            .expect("pass");
    }
    let mine: Vec<_> = take(&sent)
        .into_iter()
        .filter(|(_, workspace)| [north, south, quiet].contains(workspace))
        .collect();
    assert_eq!(
        mine,
        vec![
            ("busy".to_owned(), south),
            ("gone".to_owned(), south),
            ("phone".to_owned(), north),
        ],
        "only active devices with a token whose workspace moved are told, with its id alone"
    );
    assert_eq!(
        told(&migrator, north, phone).await,
        (5, 1, Some("phone".into()))
    );
    assert_eq!(
        told(&migrator, south, gone).await,
        (0, 0, None),
        "invalid tokens are forgotten"
    );
    assert_eq!(
        told(&migrator, south, busy).await,
        (0, 0, Some("busy".into())),
        "an unavailable provider records nothing"
    );
    for (tenant, id) in [
        (north, revoked),
        (north, silent),
        (north, left),
        (quiet, unchanged),
    ] {
        assert_eq!(told(&migrator, tenant, id).await.0, 0);
    }

    // A burst of changes inside the quiet window sends nothing more, and an
    // unavailable provider is not retried until the window passes.
    set_counters(&migrator, north, 9, 1).await;
    set_counters(&migrator, south, 4, 0).await;
    let pass = run_pass(&worker, &app, &notifiers, QUIET)
        .await
        .expect("pass");
    assert!(
        take(&sent)
            .iter()
            .all(|(_, workspace)| ![north, south].contains(workspace)),
        "{pass:?}"
    );

    // Once the window passes, the device is told once about the latest
    // position, and the busy device is tried again.
    age_claim(&migrator, north, phone).await;
    age_claim(&migrator, south, busy).await;
    run_pass(&worker, &app, &notifiers, QUIET)
        .await
        .expect("pass");
    let mine: Vec<_> = take(&sent)
        .into_iter()
        .filter(|(_, workspace)| [north, south].contains(workspace))
        .collect();
    assert_eq!(
        mine,
        vec![("busy".to_owned(), south), ("phone".to_owned(), north)]
    );
    assert_eq!(told(&migrator, north, phone).await.0, 9);

    // A device registering a token has just synced, so it counts as told
    // about the workspace as it is.
    platform_db::set_device_push(&app, north, silent, Some(("apns", "fresh")))
        .await
        .expect("register")
        .expect("device");
    assert_eq!(
        told(&migrator, north, silent).await,
        (9, 1, Some("fresh".into()))
    );
    age_claim(&migrator, north, phone).await;
    let pass: NotifyPass = run_pass(&worker, &app, &notifiers, QUIET)
        .await
        .expect("pass");
    assert!(
        take(&sent).iter().all(|(_, workspace)| *workspace != north),
        "nothing moved in North: {pass:?}"
    );

    // A metadata change alone also moves the workspace.
    set_counters(&migrator, north, 9, 2).await;
    age_claim(&migrator, north, phone).await;
    run_pass(&worker, &app, &notifiers, QUIET)
        .await
        .expect("pass");
    let mine: Vec<_> = take(&sent)
        .into_iter()
        .filter(|(_, workspace)| *workspace == north)
        .collect();
    assert_eq!(
        mine,
        vec![("fresh".to_owned(), north), ("phone".to_owned(), north)]
    );
    assert_eq!(
        told(&migrator, north, phone).await,
        (9, 2, Some("phone".into()))
    );
}
