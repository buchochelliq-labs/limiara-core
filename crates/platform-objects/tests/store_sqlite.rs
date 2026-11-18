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
async fn sqlite_store_passes_the_draft_settings_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let changes = conformance::draft_settings::run(store, tenant).await;

    // Each change left one event and one audit entry naming the days.
    let pool = databases.pool(tenant).await.expect("pool");
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT envelope FROM outbox WHERE event_type = 'platform.objects.draft-settings-updated.v1'",
    )
    .fetch_all(&pool)
    .await
    .expect("events");
    assert_eq!(i64::try_from(events.len()).expect("count"), changes);
    assert!(events
        .iter()
        .all(|envelope| envelope.contains("\"expiry_days\"")));
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'objects.draft_settings.updated'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(audited, changes);
}

#[tokio::test]
async fn sqlite_store_passes_the_quick_action_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    conformance::quick_actions::run(store, tenant).await;

    // Every change left its event; none carries a preset value.
    let pool = databases.pool(tenant).await.expect("pool");
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type, envelope FROM outbox WHERE event_type LIKE 'platform.objects.quick-action-%'",
    )
    .fetch_all(&pool)
    .await
    .expect("events");
    for kind in ["created", "updated", "deleted"] {
        let event = format!("platform.objects.quick-action-{kind}.v1");
        assert!(rows.iter().any(|(found, _)| *found == event), "{event}");
    }
    assert!(rows
        .iter()
        .all(|(_, envelope)| !envelope.contains(conformance::quick_actions::SECRET_PRESET)));
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action LIKE 'objects.quick_action.%'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(audited, i64::try_from(rows.len()).expect("count"));
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
async fn sqlite_store_passes_the_related_to_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::related_to::run(store, tenant).await;

    // The events of records holding related-to values name what changed,
    // never the values (ADR 0015 §5).
    let pool = databases.pool(tenant).await.expect("pool");
    let envelopes: Vec<String> = sqlx::query_scalar("SELECT envelope FROM outbox")
        .fetch_all(&pool)
        .await
        .expect("events");
    let about_tasks: Vec<&String> = envelopes
        .iter()
        .filter(|envelope| {
            report
                .task_ids
                .iter()
                .any(|id| envelope.contains(id.as_str()))
        })
        .collect();
    assert!(about_tasks.len() >= report.task_ids.len());
    for envelope in about_tasks {
        for target in &report.target_ids {
            assert!(!envelope.contains(target.as_str()), "{envelope}");
        }
    }
}

#[tokio::test]
async fn sqlite_store_keeps_related_to_fields_in_a_replica() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::related_to::replica(store).await;
}

#[tokio::test]
async fn sqlite_store_keeps_quick_actions_in_a_replica() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::quick_action_replicas::run(store).await;
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

#[tokio::test]
async fn sqlite_store_passes_the_plugin_object_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::plugin_objects::run(store, tenant).await;

    let pool = databases.pool(tenant).await.expect("pool");
    // The installation's evidence names the plugin that made each change.
    let by_plugin: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'objects.object.created' \
         AND json_extract(metadata, '$.plugin') = 'example.tracker'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(by_plugin, 2);
    let suspensions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'objects.object.suspended'",
    )
    .fetch_one(&pool)
    .await
    .expect("suspensions");
    assert_eq!(suspensions, 4, "two objects, suspended twice");
    for id in report.record_ids {
        let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM records WHERE id = ?1")
            .bind(id.to_string())
            .fetch_one(&pool)
            .await
            .expect("record");
        assert_eq!(kept, 1);
    }
}

#[tokio::test]
async fn sqlite_store_passes_the_related_action_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::related_actions::run(store, tenant).await;

    let pool = databases.pool(tenant).await.expect("pool");
    // Each skipped related action is audited once, with its reason.
    let skipped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'objects.quick_action.skipped' \
         AND json_extract(metadata, '$.plugin') = 'platform.activities' \
         AND json_extract(metadata, '$.reason') IS NOT NULL",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(skipped, report.skipped);
    let declared: i64 = sqlx::query_scalar("SELECT count(*) FROM related_actions")
        .fetch_one(&pool)
        .await
        .expect("related actions");
    assert_eq!(
        declared, 3,
        "log_a_call and the upgrades' log_an_email and log_a_meeting"
    );
}

#[tokio::test]
async fn sqlite_store_passes_the_timeline_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    conformance::timeline::run(store, tenant).await;

    let pool = databases.pool(tenant).await.expect("pool");
    // The upgrade dropped Event's declaration; Task's is kept.
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM timeline_sources")
        .fetch_one(&pool)
        .await
        .expect("timeline sources");
    assert_eq!(kept, 1);
}

#[tokio::test]
async fn sqlite_store_passes_the_change_source_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::sources::run(store.clone(), tenant).await;

    // Each creation's audit entry carries the source whole; its event
    // names only the kind.
    let pool = databases.pool(tenant).await.expect("pool");
    for (record, source) in &report.created {
        let metadata: String = sqlx::query_scalar(
            "SELECT metadata FROM audit_log WHERE action = 'records.record.created' AND target = ?1",
        )
        .bind(format!("record:{record}"))
        .fetch_one(&pool)
        .await
        .expect("audit");
        let metadata: serde_json::Value = serde_json::from_str(&metadata).expect("metadata");
        assert_eq!(
            metadata.get("source").unwrap_or(&serde_json::Value::Null),
            source,
            "{record}"
        );
        let envelope: String = sqlx::query_scalar(
            "SELECT envelope FROM outbox WHERE event_type = 'platform.objects.record-created.v1' \
             AND json_extract(envelope, '$.payload.entity_id') = ?1",
        )
        .bind(record)
        .fetch_one(&pool)
        .await
        .expect("event");
        let envelope: serde_json::Value = serde_json::from_str(&envelope).expect("envelope");
        assert_eq!(
            envelope["payload"]
                .get("source")
                .unwrap_or(&serde_json::Value::Null),
            &source["kind"],
            "{record}"
        );
        // A device's or client's id stays out of events. (A bulk job's id
        // is the event's causation already.)
        if ["device", "api_client"].contains(&source["kind"].as_str().unwrap_or_default()) {
            let detail = source["detail"].as_str().expect("detail");
            assert!(
                !envelope.to_string().contains(detail),
                "events carry no detail"
            );
        }
    }

    // A device's replica records its own changes as the device's.
    let replica = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        replica.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::sources::replica(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_assistant_policy_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::assistant_policy::run(store.clone(), tenant).await;
    conformance::assistant_policy::run_plugin_objects(store, uuid::Uuid::now_v7()).await;

    // Every policy change left its event and audit entry, by API names.
    let pool = databases.pool(tenant).await.expect("pool");
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE event_type = 'platform.objects.assistant-policy-updated.v1'",
    )
    .fetch_one(&pool)
    .await
    .expect("events");
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action IN \
         ('objects.assistant_settings.updated', 'objects.assistant_policy.updated')",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert!(events >= 6, "{events}");
    assert_eq!(events, audited);

    // The assistant's write names the assistant as its source, and the
    // device that pushed it in the audit entry only.
    let metadata: String = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE action = 'records.record.created' AND target = ?1",
    )
    .bind(format!("record:{}", report.assistant_record))
    .fetch_one(&pool)
    .await
    .expect("audit");
    let metadata: serde_json::Value = serde_json::from_str(&metadata).expect("metadata");
    assert_eq!(
        metadata["source"],
        serde_json::json!({"kind": "assistant", "detail": "siri"})
    );
    assert_eq!(metadata["device_id"], report.device.to_string());
    let envelope: String = sqlx::query_scalar(
        "SELECT envelope FROM outbox WHERE event_type = 'platform.objects.record-created.v1' \
         AND json_extract(envelope, '$.payload.entity_id') = ?1",
    )
    .bind(&report.assistant_record)
    .fetch_one(&pool)
    .await
    .expect("event");
    assert!(envelope.contains("\"source\":\"assistant\""));
    assert!(!envelope.contains(&report.device.to_string()));
}

#[tokio::test]
async fn sqlite_store_keeps_the_assistant_policy_in_a_replica() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::assistant_policy_replicas::run(store).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_archive_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::archive::run(store.clone(), tenant).await;

    // Each archive and unarchive left its event, with the source's kind and
    // the standard record payload, and its audit entry.
    let pool = databases.pool(tenant).await.expect("pool");
    for (kind, records) in [
        ("archived", &report.archived),
        ("unarchived", &report.unarchived),
    ] {
        let envelopes: Vec<String> =
            sqlx::query_scalar("SELECT envelope FROM outbox WHERE event_type = ?1 ORDER BY rowid")
                .bind(format!("platform.objects.record-{kind}.v1"))
                .fetch_all(&pool)
                .await
                .expect("events");
        assert_eq!(
            envelopes.len(),
            records.len() + usize::from(kind == "archived"),
            "{kind}"
        );
        for record in records {
            let envelope = envelopes
                .iter()
                .map(|text| serde_json::from_str::<serde_json::Value>(text).expect("envelope"))
                .find(|envelope| envelope["payload"]["entity_id"] == *record)
                .expect("event for the record");
            assert_eq!(envelope["payload"]["source"], "web");
            assert_eq!(envelope["payload"]["changed_fields"], serde_json::json!([]));
            assert!(envelope["payload"]["version"].is_i64());
            let audited: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM audit_log WHERE action = ?1 AND target = ?2",
            )
            .bind(format!("records.record.{kind}"))
            .bind(format!("record:{record}"))
            .fetch_one(&pool)
            .await
            .expect("audit");
            assert!(audited >= 1, "{kind} {record}");
        }
    }

    // A device replica takes the server's archive state.
    let server = tempfile::tempdir().expect("temp dir");
    let device = tempfile::tempdir().expect("temp dir");
    let open = |path: &std::path::Path| -> Arc<dyn ObjectStore> {
        Arc::new(SqliteObjectStore::new(Arc::new(
            SqliteTenantDatabases::new(path, SQLITE_MIGRATIONS),
        )))
    };
    conformance::archive::replica(open(server.path()), open(device.path())).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_field_permission_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::field_permissions::run(store, uuid::Uuid::now_v7()).await;
}

#[tokio::test]
async fn sqlite_store_keeps_field_permissions_in_a_replica() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
    conformance::field_permissions::replica(store).await;
}

#[tokio::test]
async fn sqlite_store_passes_the_recycle_bin_conformance_suite() {
    let directory = tempfile::tempdir().expect("temp dir");
    let databases = Arc::new(SqliteTenantDatabases::new(
        directory.path(),
        SQLITE_MIGRATIONS,
    ));
    let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases.clone()));
    let tenant = uuid::Uuid::now_v7();
    let report = conformance::recycle_bin::run(store, tenant).await;

    // Nothing of a purged record is left: no row in any table naming it.
    let pool = databases.pool(tenant).await.expect("pool");
    for id in &report.purged {
        for (table, column) in [
            ("records", "id"),
            ("record_index", "record_id"),
            ("record_unique", "record_id"),
            ("record_references", "record_id"),
            ("record_references", "target_id"),
            ("record_search_terms", "record_id"),
            ("record_shares", "record_id"),
            ("record_history", "record_id"),
            ("record_changes", "record_id"),
            ("record_cleared_lookups", "record_id"),
            ("record_cleared_lookups", "deleted_record_id"),
        ] {
            let left: i64 =
                sqlx::query_scalar(&format!("SELECT count(*) FROM {table} WHERE {column} = ?1"))
                    .bind(id.to_string())
                    .fetch_one(&pool)
                    .await
                    .expect(table);
            assert_eq!(left, 0, "{table}.{column} for {id}");
        }
    }
    let survivor: i64 = sqlx::query_scalar("SELECT count(*) FROM records WHERE id = ?1")
        .bind(report.survivor.to_string())
        .fetch_one(&pool)
        .await
        .expect("survivor");
    assert_eq!(survivor, 1);

    // One event per purged record, and one audit entry per batch; the
    // record's earlier evidence stays.
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT envelope FROM outbox WHERE event_type = 'platform.objects.record-purged.v1'",
    )
    .fetch_all(&pool)
    .await
    .expect("events");
    let purged_events: Vec<serde_json::Value> = events
        .iter()
        .map(|text| serde_json::from_str(text).expect("envelope"))
        .collect();
    for public_id in &report.purged_public {
        let event = purged_events
            .iter()
            .find(|event| event["payload"]["entity_id"] == *public_id.as_str())
            .unwrap_or_else(|| panic!("event for {public_id}"));
        assert_eq!(event["payload"]["source"], "system");
        assert!(event["payload"]["deleted_at"].is_string());
    }
    let batches: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'records.recycle_bin.purged'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert!(batches >= i64::from(report.batches), "{batches}");
    let deleted_evidence: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE action = 'records.record.deleted'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert!(deleted_evidence >= report.purged.len() as i64);
}
