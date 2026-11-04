//! The object engine conformance suite on SQLite (ADR 0013, ADR 0015). Runs
//! in ordinary `cargo test`: each workspace gets its own temporary file.

mod conformance;

use platform_objects::store::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::ObjectStore;
use platform_storage::sqlite::SqliteTenantDatabases;
use std::sync::Arc;

#[tokio::test]
async fn sqlite_store_passes_the_object_engine_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let report = conformance::run(store, uuid::Uuid::now_v7(), uuid::Uuid::now_v7()).await;

    assert_ne!(
        databases.path_for(report.tenant_a),
        databases.path_for(report.tenant_b),
        "each workspace lives in its own file"
    );
    let pool = databases.pool(report.tenant_a).await.expect("pool");
    for table in ["audit_log", "outbox"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .expect(table);
        assert_eq!(
            count, report.mutations_in_a,
            "{table} holds one row per mutation"
        );
    }
    let foreign: i64 = sqlx::query_scalar("SELECT count(*) FROM records WHERE tenant_id <> ?1")
        .bind(report.tenant_a.to_string())
        .fetch_one(&pool)
        .await
        .expect("foreign rows");
    assert_eq!(foreign, 0);
}

#[tokio::test]
async fn sqlite_store_passes_the_query_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::query::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_search_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::search::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_validation_rule_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::rules::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_formula_field_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::formulas::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_automation_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::automations::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_automation_job_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::jobs::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_composite_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::composite::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_bulk_job_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::bulk::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_relationship_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::relationships::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_rollup_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::rollups::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_report_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::reports::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sqlite_store_keeps_the_report_limit() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::reports::limit(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_related_list_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::related::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_sync_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::sync::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_replica_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::replica::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_client_id_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::client_ids::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_push_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::push::run(store).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_conflict_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::conflicts::run(store).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_replica_definitions_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::definitions::run(store).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_replica_lease_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::lease::run(store).await;
}
