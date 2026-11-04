//! The sync loop against the real workspace API, with a device's own access
//! token (ADR 0024, ADR 0029). Run by `scripts/test-objects-acceptance.sh`,
//! which sets:
//!
//! - `PLATFORM_SYNC_API`: the API origin;
//! - `PLATFORM_SYNC_WORKSPACE`: the workspace id;
//! - `PLATFORM_SYNC_TOKEN`: the device's access token;
//! - `PLATFORM_SYNC_OBJECT`: an object the member may read and edit.

use async_trait::async_trait;
use futures_util::StreamExt;
use platform_objects::store::sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::*;
use platform_storage::sqlite::SqliteTenantDatabases;
use platform_sync::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

struct Fixed(String);

#[async_trait]
impl AccessToken for Fixed {
    async fn access_token(&self) -> Result<String, SyncError> {
        Ok(self.0.clone())
    }
}

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is set"))
}

#[tokio::test]
#[ignore = "run by scripts/test-objects-acceptance.sh against the real API"]
async fn a_device_syncs_with_the_real_api() {
    let object = env("PLATFORM_SYNC_OBJECT");
    let workspace = Uuid::parse_str(&env("PLATFORM_SYNC_WORKSPACE")).expect("workspace id");
    let transport = HttpTransport::new(
        env("PLATFORM_SYNC_API"),
        workspace,
        Arc::new(Fixed(env("PLATFORM_SYNC_TOKEN"))),
    )
    .expect("transport");

    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    let engine = ObjectService::new(
        store,
        ObjectCaller::new(
            workspace,
            Uuid::now_v7(),
            Default::default(),
            vec![Uuid::now_v7()],
        ),
    )
    .with_replica()
    .with_client_ids();
    let client = SyncClient::new(engine, transport);

    // The app passes the offline limit from the token response (14 days by
    // default); the first sync that reaches the server starts the lease.
    client
        .engine()
        .set_offline_limit(1_209_600)
        .await
        .expect("offline limit");
    let report = client
        .sync_once(MutationContext::new(Uuid::now_v7()))
        .await
        .expect("first sync");
    let lease = client.engine().lease().await.expect("lease");
    assert!(
        lease.last_contact_at.is_some() && !lease.locked,
        "{lease:?}"
    );
    let definitions = report.definitions.clone().expect("definitions");
    assert!(definitions.objects.contains(&object), "{report:?}");
    assert!(report.resynced && report.pulled > 0, "{report:?}");

    // The member renames a record offline; the next sync pushes it.
    let held = client
        .engine()
        .list_records(&object, Default::default())
        .await
        .expect("held records")
        .records;
    let record = held.first().expect("a record of the object");
    let renamed = format!("{} (from a phone)", record.name);
    client
        .engine()
        .update_record(
            &object,
            &record.public_id,
            record.version,
            &json!({"name": renamed})
                .as_object()
                .cloned()
                .expect("values"),
            MutationContext::new(Uuid::now_v7()),
        )
        .await
        .expect("offline edit");
    let report = client
        .sync_once(MutationContext::new(Uuid::now_v7()))
        .await
        .expect("second sync");
    assert!(
        report.pushed.iter().all(|pushed| pushed.failed.is_empty()) && !report.pushed.is_empty(),
        "{report:?}"
    );
    let server = client
        .transport()
        .record(&object, &record.public_id)
        .await
        .expect("server copy")
        .expect("visible");
    assert_eq!(server["name"], json!(renamed));
    assert!(client
        .engine()
        .push_queue()
        .await
        .expect("queue")
        .is_empty());
    // The change stream opens from the stored position and announces the
    // workspace's metadata version first.
    let pulled = client
        .engine()
        .sync_positions()
        .await
        .expect("positions")
        .pulled;
    let mut stream = client.transport().stream(pulled).await.expect("stream");
    let first = tokio::time::timeout(std::time::Duration::from_secs(10), stream.next())
        .await
        .expect("an event within ten seconds")
        .expect("an event")
        .expect("a valid event");
    assert!(matches!(first, StreamEvent::Metadata(_)), "{first:?}");
    drop(stream);
    let woken = client
        .wake(
            MutationContext::new(Uuid::now_v7()),
            std::time::Duration::from_secs(20),
        )
        .await
        .expect("a background sync");
    assert!(woken.pushed.is_empty(), "{woken:?}");
    println!("device sync against the API: {} records held", held.len());
}
