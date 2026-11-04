//! Workspace files encrypted with SQLCipher (ADR 0024, "Encryption at
//! rest"). Run with `--features encryption`.
#![cfg(feature = "encryption")]

use async_trait::async_trait;
use platform_storage::keys::{DatabaseKey, DatabaseKeys, KeyError, MemoryKeys};
use platform_storage::sqlite::{SqliteStoreError, SqliteTenantDatabases, TenantMigration};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Row};
use std::sync::Arc;
use uuid::Uuid;

const NOTES: &[TenantMigration] = &[TenantMigration {
    owner: "test.notes",
    id: "0001_notes",
    sql: "CREATE TABLE notes (tenant_id TEXT NOT NULL, body TEXT NOT NULL)",
}];

const SECRET: &str = "the vault code is 4711";

async fn write_note(databases: &SqliteTenantDatabases, tenant: Uuid) {
    let mut tx = databases.begin(tenant).await.expect("transaction");
    sqlx::query("INSERT INTO notes (tenant_id, body) VALUES (?1, ?2)")
        .bind(tenant.to_string())
        .bind(SECRET)
        .execute(&mut *tx)
        .await
        .expect("insert");
    tx.commit().await.expect("commit");
}

async fn read_note(databases: &SqliteTenantDatabases, tenant: Uuid) -> String {
    let pool = databases.pool(tenant).await.expect("pool");
    sqlx::query("SELECT body FROM notes")
        .fetch_one(&pool)
        .await
        .expect("note")
        .get(0)
}

/// Every byte of the workspace's file and its write-ahead log.
fn raw_bytes(databases: &SqliteTenantDatabases, tenant: Uuid) -> Vec<u8> {
    let path = databases.path_for(tenant);
    let mut bytes = std::fs::read(&path).expect("database file");
    let mut wal = path.into_os_string();
    wal.push("-wal");
    if let Ok(log) = std::fs::read(&wal) {
        bytes.extend(log);
    }
    bytes
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[tokio::test]
async fn a_workspace_file_is_unreadable_without_its_key() {
    let directory = tempfile::tempdir().expect("dir");
    let keys = Arc::new(MemoryKeys::new());
    let tenant = Uuid::now_v7();
    let databases = SqliteTenantDatabases::new(directory.path(), NOTES).with_keys(keys.clone());
    write_note(&databases, tenant).await;
    assert_eq!(read_note(&databases, tenant).await, SECRET);

    // Neither the data nor SQLite's header is in the file or its log.
    let bytes = raw_bytes(&databases, tenant);
    assert!(!contains(&bytes, SECRET.as_bytes()), "plaintext on disk");
    assert!(!contains(&bytes, b"SQLite format 3"), "plain SQLite header");

    // The same key opens it again, as after a restart.
    let reopened = SqliteTenantDatabases::new(directory.path(), NOTES).with_keys(keys.clone());
    assert_eq!(read_note(&reopened, tenant).await, SECRET);

    // Another key, or none, does not.
    let wrong = Arc::new(MemoryKeys::new());
    wrong.set(tenant, DatabaseKey::generate()).await;
    let with_wrong_key = SqliteTenantDatabases::new(directory.path(), NOTES).with_keys(wrong);
    assert!(matches!(
        with_wrong_key.pool(tenant).await,
        Err(SqliteStoreError::Key)
    ));
    let plain = SqliteConnectOptions::new()
        .filename(databases.path_for(tenant))
        .connect()
        .await;
    let unreadable = match plain {
        Ok(mut connection) => sqlx::query("SELECT count(*) FROM sqlite_master")
            .fetch_one(&mut connection)
            .await
            .is_err(),
        Err(_) => true,
    };
    assert!(unreadable, "a plain connection read the encrypted file");
}

#[tokio::test]
async fn a_wipe_deletes_the_files_and_destroys_the_key() {
    let directory = tempfile::tempdir().expect("dir");
    let keys = Arc::new(MemoryKeys::new());
    let tenant = Uuid::now_v7();
    let databases = SqliteTenantDatabases::new(directory.path(), NOTES).with_keys(keys.clone());
    write_note(&databases, tenant).await;
    assert!(keys.holds(tenant).await);

    databases.wipe(tenant).await.expect("wipe");
    assert!(!databases.path_for(tenant).exists());
    let mut wal = databases.path_for(tenant).into_os_string();
    wal.push("-wal");
    assert!(!std::path::Path::new(&wal).exists());
    assert!(!keys.holds(tenant).await);

    // The workspace starts empty, with a new key.
    let pool = databases.pool(tenant).await.expect("fresh pool");
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0);
    assert!(keys.holds(tenant).await);
}

/// A keystore whose `destroy` takes a while, as a platform keystore may.
struct SlowKeys(MemoryKeys);

#[async_trait]
impl DatabaseKeys for SlowKeys {
    async fn key(&self, tenant_id: Uuid) -> Result<DatabaseKey, KeyError> {
        self.0.key(tenant_id).await
    }

    async fn destroy(&self, tenant_id: Uuid) -> Result<(), KeyError> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        self.0.destroy(tenant_id).await
    }
}

#[tokio::test]
async fn a_workspace_opened_during_a_wipe_waits_for_it() {
    let directory = tempfile::tempdir().expect("dir");
    let keys = Arc::new(SlowKeys(MemoryKeys::new()));
    let tenant = Uuid::now_v7();
    let databases =
        Arc::new(SqliteTenantDatabases::new(directory.path(), NOTES).with_keys(keys.clone()));
    write_note(&databases, tenant).await;

    let wiping = {
        let databases = databases.clone();
        tokio::spawn(async move { databases.wipe(tenant).await })
    };
    // Mid-wipe: the files are gone and the keystore is destroying the key.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let pool = databases.pool(tenant).await.expect("pool after the wipe");
    wiping.await.expect("join").expect("wipe");

    // The pool opened after the wipe, under a new key that the keystore
    // still holds, so what is written now stays readable.
    assert!(
        keys.0.holds(tenant).await,
        "the open file's key was destroyed"
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM notes")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(count, 0, "the old workspace survived the wipe");
}
