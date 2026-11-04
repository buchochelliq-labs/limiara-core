//! A synced device's offline lease (ADR 0024 §4), on SQLite, which devices
//! use: past the limit the server set, the replica refuses the member until
//! the device reaches the server again, while syncing goes on so the next
//! contact can unlock it.

use super::push::remote;
use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use chrono::{Duration, Utc};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn expired<T: std::fmt::Debug>(result: Result<T, ObjectError>) -> bool {
    matches!(result, Err(ObjectError::Conflict("sync.lease_expired")))
}

pub async fn run(store: Arc<dyn ObjectStore>) {
    let member = Uuid::now_v7();
    let server = service(
        &store,
        caller(Uuid::now_v7(), member, &ADMIN_PERMISSIONS, &[]),
    );
    let device = service(
        &store,
        caller(Uuid::now_v7(), member, &ADMIN_PERMISSIONS, &[]),
    )
    .with_replica()
    .with_client_ids();
    for engine in [&server, &device] {
        engine
            .create_object(object("account", "Account"), context())
            .await
            .expect("account");
        engine
            .create_field("custom:account", field("tier", FieldType::Text), context())
            .await
            .expect("tier");
    }
    let acme = server
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("acme");
    device
        .apply_remote("custom:account", &remote(&acme), None)
        .await
        .expect("pulled");

    // Only a replica's engine holds a lease.
    assert!(matches!(
        server.set_offline_limit(60).await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        server.record_contact().await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        server.lease().await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        device.set_offline_limit(0).await,
        Err(ObjectError::Validation(_))
    ));

    // Without a limit (a standalone workspace) or before the first contact,
    // nothing locks.
    let lease = device.lease().await.expect("lease");
    assert_eq!(lease.offline_limit_seconds, None);
    assert!(!lease.locked && lease.expires_at.is_none());
    let lease = device.set_offline_limit(1).await.expect("limit");
    assert_eq!(lease.offline_limit_seconds, Some(1));
    assert!(!lease.locked, "no contact yet: {lease:?}");
    device
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("readable before the first contact");

    // After contact, the member reads and writes until the limit passes.
    let contact = device.record_contact().await.expect("contact");
    assert!(!contact.locked);
    assert!(contact.expires_at > contact.last_contact_at);
    device
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("readable within the lease");
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    assert!(device.lease().await.expect("lease").locked);
    assert!(expired(
        device
            .get_record("custom:account", &acme.public_id, false)
            .await
    ));
    assert!(expired(
        device
            .list_records("custom:account", Default::default())
            .await
    ));
    assert!(expired(
        device
            .create_record(
                "custom:account",
                &body(json!({"name": "Offline"})),
                context()
            )
            .await
    ));
    assert!(expired(device.list_objects().await));
    assert!(expired(
        device
            .query(QueryRequest {
                q: "SELECT name FROM custom:account".into(),
                cursor: None,
                page_size: None,
                include_deleted: false,
            })
            .await
    ));

    // Syncing goes on while locked, so the next contact can unlock it.
    device
        .push_queue()
        .await
        .expect("the queue is read while locked");
    let changed = server
        .update_record(
            "custom:account",
            &acme.public_id,
            acme.version,
            &body(json!({"custom:tier": "gold"})),
            context(),
        )
        .await
        .expect("server edit");
    device
        .apply_remote("custom:account", &remote(&changed), None)
        .await
        .expect("a pull is stored while locked");
    device
        .record_pull(1, 1)
        .await
        .expect("positions while locked");
    let unlocked = device.record_contact().await.expect("contact");
    assert!(!unlocked.locked);
    let local = device
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("readable again");
    assert_eq!(local.values.get("custom:tier"), Some(&json!("gold")));

    // A clock set before the last contact locks too, so turning the clock
    // back cannot hold the lease open.
    device.set_offline_limit(3_600).await.expect("limit");
    let ahead = device
        .record_contact_at(Utc::now() + Duration::hours(1))
        .await
        .expect("contact in the future");
    assert!(ahead.locked, "{ahead:?}");
    assert!(expired(device.list_objects().await));
    device.record_contact().await.expect("contact");
    assert!(device.list_objects().await.is_ok());

    // With the device's monotonic clock, a wall clock turned back while the
    // app was closed cannot stretch the lease (ADR 0024, "Trusted elapsed
    // time"). The contact's reading is stored, so a later process compares
    // against it.
    let clock = Arc::new(FakeClock::default());
    clock.set("boot-1", 1_000);
    let tenant = device.caller().tenant_id();
    let with_clock = || {
        service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]))
            .with_replica()
            .with_client_ids()
            .with_trusted_clock(clock.clone())
    };
    with_clock().record_contact().await.expect("contact");
    clock.set("boot-1", 1_000 + 7_200);
    let reopened = with_clock();
    let lease = reopened.lease().await.expect("lease");
    assert!(lease.locked, "{lease:?}");
    assert!(expired(reopened.list_objects().await));
    // A restart makes the readings incomparable: the wall clock decides,
    // and its two hours have not passed.
    clock.set("boot-2", 30);
    assert!(with_clock().list_objects().await.is_ok());
    // Within the same boot again, the next contact renews the reading.
    clock.set("boot-2", 9_000);
    with_clock().record_contact().await.expect("contact");
    clock.set("boot-2", 9_600);
    assert!(!with_clock().lease().await.expect("lease").locked);
}

/// A monotonic clock the test moves by hand.
#[derive(Default)]
struct FakeClock(std::sync::Mutex<Option<TrustedInstant>>);

impl FakeClock {
    fn set(&self, boot: &str, seconds: u64) {
        *self.0.lock().expect("clock") = Some(TrustedInstant {
            boot: boot.into(),
            since_boot: std::time::Duration::from_secs(seconds),
        });
    }
}

impl TrustedClock for FakeClock {
    fn now(&self) -> Option<TrustedInstant> {
        self.0.lock().expect("clock").clone()
    }
}
