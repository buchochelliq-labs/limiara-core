//! The Contacts store conformance suite on SQLite (ADR 0013). Runs in
//! ordinary `cargo test`: each workspace gets its own temporary file.

mod conformance;

use contacts_plugin::store::{sqlite::SQLITE_MIGRATIONS, ContactsStore, SqliteContactsStore};
use platform_storage::sqlite::SqliteTenantDatabases;
use std::sync::Arc;

#[tokio::test]
async fn sqlite_store_passes_the_contacts_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ContactsStore> = Arc::new(SqliteContactsStore::new(databases.clone()));
    let report = conformance::run(store.clone()).await;
    conformance::standard_fields(store.clone()).await;
    conformance::recycle_bin(store).await;

    // Each workspace lives in its own file.
    assert!(databases.path_for(report.tenant_a).exists());
    assert!(databases.path_for(report.tenant_b).exists());
    assert_ne!(
        databases.path_for(report.tenant_a),
        databases.path_for(report.tenant_b)
    );

    // Every successful mutation left one audit row and one outbox event in
    // the same file, written in the same transaction.
    let pool = databases.pool(report.tenant_a).await.expect("pool");
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
        .fetch_one(&pool)
        .await
        .expect("audit");
    let outbox: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox")
        .fetch_one(&pool)
        .await
        .expect("outbox");
    assert_eq!(audit, report.mutations_in_a);
    assert_eq!(outbox, report.mutations_in_a);
    let foreign: i64 =
        sqlx::query_scalar("SELECT count(*) FROM p_contacts_people WHERE tenant_id <> ?1")
            .bind(report.tenant_a.to_string())
            .fetch_one(&pool)
            .await
            .expect("foreign rows");
    assert_eq!(foreign, 0, "a workspace file holds only its own rows");
}

#[tokio::test]
async fn record_stamps_upgrade_existing_workspace_files() {
    let directory = tempfile::tempdir().expect("temp dir");
    let tenant = uuid::Uuid::now_v7();
    let before = SqliteTenantDatabases::new(directory.path(), &SQLITE_MIGRATIONS[..1]);
    let pool = before.pool(tenant).await.expect("pool");
    sqlx::query(
        "INSERT INTO p_contacts_people (tenant_id, id, public_id, full_name, name_key, tags, \
           created_at, updated_at) \
         VALUES (?1, ?2, 'per_old', 'Old Person', 'old person', '[]', ?3, ?3)",
    )
    .bind(tenant.to_string())
    .bind(uuid::Uuid::now_v7().to_string())
    .bind("2026-01-02T03:04:05.000000Z")
    .execute(&pool)
    .await
    .expect("row before stamps");
    pool.close().await;
    drop(before);

    let after = SqliteTenantDatabases::new(directory.path(), SQLITE_MIGRATIONS);
    let pool = after.pool(tenant).await.expect("upgraded pool");
    let (touched, creator): (String, Option<String>) = sqlx::query_as(
        "SELECT touched_at, created_by FROM p_contacts_people WHERE public_id = 'per_old'",
    )
    .fetch_one(&pool)
    .await
    .expect("upgraded row");
    assert_eq!(touched, "2026-01-02T03:04:05.000000Z");
    assert_eq!(
        creator, None,
        "rows written before the stamps have no creator"
    );
}
