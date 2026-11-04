//! The key never reaches a log, even with every sqlx log enabled. Its own
//! test binary, because it installs the process's global subscriber.
#![cfg(feature = "encryption")]

use platform_storage::keys::{DatabaseKey, MemoryKeys};
use platform_storage::sqlite::SqliteTenantDatabases;
use std::io::Write;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().expect("log buffer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn opening_an_encrypted_workspace_logs_no_key() {
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .expect("subscriber");

    let directory = tempfile::tempdir().expect("dir");
    let tenant = Uuid::now_v7();
    let key = DatabaseKey::from_bytes([0x5a; 32]);
    let keys = Arc::new(MemoryKeys::new());
    keys.set(tenant, key.clone()).await;
    let databases =
        SqliteTenantDatabases::new(directory.path().join("encrypted"), &[]).with_keys(keys);
    let pool = databases.pool(tenant).await.expect("pool");
    sqlx::query("SELECT count(*) FROM audit_log")
        .execute(&pool)
        .await
        .expect("query");

    // The logger is live: a plain workspace's statements are logged.
    let plain = SqliteTenantDatabases::new(directory.path().join("plain"), &[]);
    let plain_pool = plain.pool(Uuid::now_v7()).await.expect("plain pool");
    sqlx::query("SELECT count(*) FROM outbox")
        .execute(&plain_pool)
        .await
        .expect("plain query");

    let logs = String::from_utf8_lossy(&captured.0.lock().expect("logs")).into_owned();
    assert!(
        logs.contains("outbox"),
        "no sqlx statement logs captured: {logs}"
    );
    let hex: String = key
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert!(!logs.contains(&hex), "the key was logged");
    assert!(
        !logs.to_lowercase().contains("pragma key"),
        "the key pragma was logged"
    );
}
