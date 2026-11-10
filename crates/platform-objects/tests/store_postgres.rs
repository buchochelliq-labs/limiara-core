//! The object engine conformance suite on PostgreSQL, through the runtime
//! role under forced row-level security. Run with
//! `bash scripts/test-plugin-runtime.sh`.

mod conformance;

use platform_objects::store::PostgresObjectStore;
use platform_objects::{ChangeKind, ObjectStore, RecordChange};
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use uuid::Uuid;

async fn pool(variable: &str) -> sqlx::PgPool {
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&std::env::var(variable).unwrap_or_else(|_| panic!("{variable}")))
        .await
        .expect("connect")
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_formula_field_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Formula field conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::formulas::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_composite_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Composite conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::composite::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_bulk_job_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Bulk job conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::bulk::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_automation_job_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Automation job conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::jobs::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_automation_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Automation conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::automations::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_relationship_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Relationship conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::relationships::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_rollup_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Roll-up conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::rollups::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_serialises_rollup_creation() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Roll-up locks')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::rollups::locks(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_serialises_master_detail_changes() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Master-detail locks')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::relationships::locks(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_validation_rule_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO tenants (id, display_name) VALUES ($1, 'Validation rule conformance')",
    )
    .bind(tenant)
    .execute(&migrator)
    .await
    .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::rules::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_quick_action_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Quick action conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::quick_actions::run(store, tenant).await;

    // Every change left its event; none carries a preset value.
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT event_type, envelope::text FROM outbox \
         WHERE tenant_id = $1 AND event_type LIKE 'platform.objects.quick-action-%'",
    )
    .bind(tenant)
    .fetch_all(&migrator)
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
        "SELECT count(*) FROM audit_log WHERE tenant_id = $1 AND action LIKE 'objects.quick_action.%'",
    )
    .bind(tenant)
    .fetch_one(&migrator)
    .await
    .expect("audit");
    assert_eq!(audited, i64::try_from(rows.len()).expect("count"));
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_search_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Search conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::search::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_query_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Query conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::query::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_object_engine_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let (tenant_a, tenant_b) = (Uuid::now_v7(), Uuid::now_v7());
    for tenant in [tenant_a, tenant_b] {
        sqlx::query(
            "INSERT INTO tenants (id, display_name) VALUES ($1, 'Object engine conformance')",
        )
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    }
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app.clone()));
    let report = conformance::run(store, tenant_a, tenant_b).await;

    for table in ["audit_log", "outbox"] {
        let count: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM {table} WHERE tenant_id = $1"
        ))
        .bind(report.tenant_a)
        .fetch_one(&migrator)
        .await
        .expect(table);
        assert_eq!(
            count, report.mutations_in_a,
            "{table} holds one row per mutation"
        );
    }

    // Row-level security: without a workspace context the runtime role sees
    // no engine rows at all, and inside one it sees only that workspace's.
    for table in [
        "object_definitions",
        "field_definitions",
        "records",
        "record_index",
        "record_shares",
        "reporting_lines",
        "record_history",
        "record_changes",
        "change_counters",
        "record_search_terms",
        "validation_rules",
        "quick_actions",
        "field_reference_objects",
        "related_actions",
        "timeline_sources",
        "automations",
        "automation_runs",
        "automation_jobs",
        "bulk_jobs",
        "bulk_job_rows",
        "formula_index_marks",
    ] {
        let visible: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
            .fetch_one(&app)
            .await
            .expect(table);
        assert_eq!(visible, 0, "{table} without app.tenant_id");
    }
    let mut tx = platform_db::begin_tenant_tx(&app, report.tenant_b)
        .await
        .expect("tenant b");
    let foreign: i64 = sqlx::query_scalar("SELECT count(*) FROM records WHERE tenant_id = $1")
        .bind(report.tenant_a)
        .fetch_one(&mut *tx)
        .await
        .expect("foreign");
    assert_eq!(foreign, 0, "workspace B cannot read workspace A's records");
    let write = sqlx::query(
        "INSERT INTO object_definitions (tenant_id, id, api_name, label, plural_label, origin, key_prefix) \
         VALUES ($1, $2, 'custom:smuggled', 'S', 'S', 'workspace', '9zz')",
    )
    .bind(report.tenant_a)
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await;
    assert!(write.is_err(), "workspace B cannot write into workspace A");
    let mut tx = platform_db::begin_tenant_tx(&app, report.tenant_b)
        .await
        .expect("tenant b");
    let share = sqlx::query(
        "INSERT INTO record_shares (tenant_id, record_id, grantee_kind, grantee_id, access, created_at) \
         VALUES ($1, $2, 'user', $3, 'edit', now())",
    )
    .bind(report.tenant_a)
    .bind(Uuid::now_v7())
    .bind(Uuid::now_v7())
    .execute(&mut *tx)
    .await;
    assert!(
        share.is_err(),
        "workspace B cannot share workspace A's records"
    );
    drop(tx);
    let mut tx = platform_db::begin_tenant_tx(&app, report.tenant_a)
        .await
        .expect("tenant a");
    let kept: i64 = sqlx::query_scalar("SELECT count(*) FROM record_history")
        .fetch_one(&mut *tx)
        .await
        .expect("history");
    assert!(kept > 0, "tracked changes left history");
    let rewrite = sqlx::query("UPDATE record_history SET new_value = '\"forged\"'")
        .execute(&mut *tx)
        .await;
    assert!(
        rewrite.is_err(),
        "history is append-only for the application"
    );

    drop(tx);

    // Change log positions follow commit order: a second writer waits for
    // the first to commit before it takes the next position (ADR 0018).
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app.clone()));
    let change = RecordChange {
        position: 0,
        record_id: Uuid::now_v7(),
        object_id: Uuid::now_v7(),
        public_id: "000ordering".into(),
        kind: ChangeKind::Updated,
        fields: vec!["name".into()],
        changed_at: chrono::Utc::now(),
        changed_by: None,
        correlation_id: None,
    };
    let mut first = store.begin(report.tenant_a).await.expect("first writer");
    let first_position = first.append_change(&change).await.expect("first append");
    let second = {
        let store = store.clone();
        let change = change.clone();
        let tenant = report.tenant_a;
        tokio::spawn(async move {
            let mut second = store.begin(tenant).await.expect("second writer");
            let position = second.append_change(&change).await.expect("second append");
            second.commit().await.expect("second commit");
            position
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !second.is_finished(),
        "the second writer waits for the first to commit"
    );
    first.commit().await.expect("first commit");
    let second_position = second.await.expect("second writer task");
    assert_eq!(second_position, first_position + 1, "no gaps, commit order");

    // Fresh changes cannot be removed or rewritten by the application.
    let mut tx = platform_db::begin_tenant_tx(&app, report.tenant_a)
        .await
        .expect("tenant a");
    let removed = sqlx::query("DELETE FROM record_changes")
        .execute(&mut *tx)
        .await
        .expect("delete is allowed only for expired rows")
        .rows_affected();
    assert_eq!(removed, 0, "changes inside the retention window stay");
    let rewrite = sqlx::query("UPDATE record_changes SET kind = 'deleted'")
        .execute(&mut *tx)
        .await;
    assert!(rewrite.is_err(), "the change log is append-only");
}

/// Formula fields created before their values were stored have no index
/// rows; the first query that needs them builds them (ADR 0021 §6).
#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn queries_backfill_stored_formula_values_once() {
    use platform_objects::*;
    use serde_json::json;
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Formula backfill')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    let admin = ObjectService::new(
        store,
        ObjectCaller::new(
            tenant,
            Uuid::now_v7(),
            [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            Vec::new(),
        ),
    );
    let context = || MutationContext::new(Uuid::now_v7());
    let object: CreateObject =
        serde_json::from_value(json!({"name": "item", "label": "Item", "plural_label": "Items"}))
            .expect("object");
    admin.create_object(object, context()).await.expect("item");
    for definition in [
        json!({"name": "amount", "label": "Amount", "field_type": "number", "scale": 0}),
        json!({"name": "double", "label": "Double", "field_type": "formula",
               "formula": "custom:amount * 2", "formula_type": "number"}),
    ] {
        let field: CreateField = serde_json::from_value(definition).expect("field");
        admin
            .create_field("custom:item", field, context())
            .await
            .expect("field");
    }
    let body = json!({"name": "Ten", "custom:amount": 5});
    admin
        .create_record("custom:item", body.as_object().expect("body"), context())
        .await
        .expect("record");
    // As an upgraded workspace looks: no stored formula values, no mark.
    let double = admin
        .describe("custom:item")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:double")
        .expect("double");
    sqlx::query("DELETE FROM record_index WHERE tenant_id = $1 AND field_id = $2")
        .bind(tenant)
        .bind(double.id)
        .execute(&migrator)
        .await
        .expect("drop stored values");
    sqlx::query("DELETE FROM formula_index_marks WHERE tenant_id = $1")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("drop marks");

    let found = admin
        .query(QueryRequest {
            q: "SELECT name FROM custom:item WHERE custom:double = 10".into(),
            ..QueryRequest::default()
        })
        .await
        .expect("query");
    assert_eq!(found.rows.len(), 1, "the first query backfills the values");
    let marked: i64 =
        sqlx::query_scalar("SELECT count(*) FROM formula_index_marks WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&migrator)
            .await
            .expect("marks");
    assert_eq!(marked, 1, "and marks the object so it happens once");
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_report_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Report conformance')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::reports::run(store, tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_keeps_the_report_limit() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Report limit')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::reports::limit(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_related_list_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Related lists')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::related::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_related_to_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    let other = Uuid::now_v7();
    for (id, name) in [(tenant, "Related to"), (other, "Related to, other")] {
        sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, $2)")
            .bind(id)
            .bind(name)
            .execute(&migrator)
            .await
            .expect("tenant");
    }
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app.clone()));
    let report = conformance::related_to::run(store, tenant).await;

    // The events of records holding related-to values name what changed,
    // never the values (ADR 0015 §5).
    let envelopes: Vec<String> =
        sqlx::query_scalar("SELECT envelope::text FROM outbox WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_all(&migrator)
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

    // The allowlist table is tenant-scoped under forced row-level security.
    let forced: (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class \
         WHERE relname = 'field_reference_objects'",
    )
    .fetch_one(&migrator)
    .await
    .expect("rls flags");
    assert_eq!(forced, (true, true));
    let stored: i64 =
        sqlx::query_scalar("SELECT count(*) FROM field_reference_objects WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&migrator)
            .await
            .expect("rows");
    assert!(stored > 0);
    let unscoped: i64 = sqlx::query_scalar("SELECT count(*) FROM field_reference_objects")
        .fetch_one(&app)
        .await
        .expect("unscoped");
    assert_eq!(unscoped, 0, "no rows without app.tenant_id");
    let (field_id, object_id): (Uuid, Uuid) = sqlx::query_as(
        // The field's own object, which its allowlist does not name, so only
        // row-level security can refuse the row.
        "SELECT id, object_id FROM field_definitions \
         WHERE tenant_id = $1 AND field_type = 'related_to' LIMIT 1",
    )
    .bind(tenant)
    .fetch_one(&migrator)
    .await
    .expect("a row");
    let mut tx = platform_db::begin_tenant_tx(&app, other)
        .await
        .expect("other tenant");
    let foreign: i64 = sqlx::query_scalar("SELECT count(*) FROM field_reference_objects")
        .fetch_one(&mut *tx)
        .await
        .expect("foreign");
    assert_eq!(foreign, 0, "another workspace sees none");
    let write = sqlx::query(
        "INSERT INTO field_reference_objects (tenant_id, field_id, object_id, position) \
         VALUES ($1, $2, $3, 9)",
    )
    .bind(tenant)
    .bind(field_id)
    .bind(object_id)
    .execute(&mut *tx)
    .await;
    assert!(write.is_err(), "another workspace cannot write");
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_sync_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Device sync')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::sync::run(store, tenant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_client_id_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Device ids')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app));
    conformance::client_ids::run(store, tenant).await;
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_plugin_object_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    let other = Uuid::now_v7();
    for (id, name) in [(tenant, "Plugin objects"), (other, "Plugin objects, other")] {
        sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, $2)")
            .bind(id)
            .bind(name)
            .execute(&migrator)
            .await
            .expect("tenant");
    }
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app.clone()));
    let report = conformance::plugin_objects::run(store.clone(), tenant).await;

    // The installation's evidence names the plugin that made each change,
    // next to the member who changed the plugin's state (ADR 0033 §2).
    let by_plugin: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE tenant_id = $1 AND action = 'objects.object.created' \
         AND metadata->>'plugin' = 'example.tracker' AND actor_id IS NOT NULL",
    )
    .bind(tenant)
    .fetch_one(&migrator)
    .await
    .expect("audit");
    assert_eq!(by_plugin, 2);
    let events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox WHERE tenant_id = $1 \
         AND envelope->'payload'->>'plugin' = 'example.tracker'",
    )
    .bind(tenant)
    .fetch_one(&migrator)
    .await
    .expect("events");
    assert!(events >= 2, "events name the plugin too");
    for id in &report.record_ids {
        let kept: i64 =
            sqlx::query_scalar("SELECT count(*) FROM records WHERE tenant_id = $1 AND id = $2")
                .bind(tenant)
                .bind(id)
                .fetch_one(&migrator)
                .await
                .expect("record");
        assert_eq!(kept, 1, "removal keeps records for export");
    }

    // Only plugin objects can be suspended, and a plugin object names its
    // owner in its own namespace.
    let workspace_suspended = sqlx::query(
        "UPDATE object_definitions SET suspended = true \
         WHERE tenant_id = $1 AND origin = 'workspace'",
    )
    .bind(tenant)
    .execute(&migrator)
    .await;
    assert!(workspace_suspended.is_err());
    let foreign_owner = sqlx::query(
        "UPDATE object_definitions SET owner_plugin = 'example.other' \
         WHERE tenant_id = $1 AND origin = 'plugin'",
    )
    .bind(tenant)
    .execute(&migrator)
    .await;
    assert!(foreign_owner.is_err());

    // Another workspace sees none of the plugin's objects.
    let mut tx = platform_db::begin_tenant_tx(&app, other)
        .await
        .expect("other tenant");
    let foreign: i64 =
        sqlx::query_scalar("SELECT count(*) FROM object_definitions WHERE origin = 'plugin'")
            .fetch_one(&mut *tx)
            .await
            .expect("foreign");
    assert_eq!(foreign, 0);
    tx.commit().await.expect("commit");
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_related_action_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Related actions')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app.clone()));
    let report = conformance::related_actions::run(store, tenant).await;

    // Each skipped related action is audited once, with its reason, and
    // publishes no event: nothing changed.
    let skipped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_log WHERE tenant_id = $1 \
         AND action = 'objects.quick_action.skipped' \
         AND metadata->>'plugin' = 'platform.activities' AND metadata ? 'reason'",
    )
    .bind(tenant)
    .fetch_one(&migrator)
    .await
    .expect("audit");
    assert_eq!(skipped, report.skipped);
    let declared: i64 =
        sqlx::query_scalar("SELECT count(*) FROM related_actions WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&migrator)
            .await
            .expect("related actions");
    assert_eq!(declared, 2, "log_a_call and the upgrade's log_an_email");
    // The runtime role sees none without the workspace's context.
    let unscoped: i64 = sqlx::query_scalar("SELECT count(*) FROM related_actions")
        .fetch_one(&app)
        .await
        .expect("unscoped");
    assert_eq!(unscoped, 0);
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_timeline_conformance_suite() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Timeline')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(app.clone()));
    conformance::timeline::run(store, tenant).await;

    // The upgrade dropped Event's declaration; Task's is kept.
    let kept: i64 =
        sqlx::query_scalar("SELECT count(*) FROM timeline_sources WHERE tenant_id = $1")
            .bind(tenant)
            .fetch_one(&migrator)
            .await
            .expect("timeline sources");
    assert_eq!(kept, 1);
    // The runtime role sees none without the workspace's context.
    let unscoped: i64 = sqlx::query_scalar("SELECT count(*) FROM timeline_sources")
        .fetch_one(&app)
        .await
        .expect("unscoped");
    assert_eq!(unscoped, 0);
}
