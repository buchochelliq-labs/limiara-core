//! Real-PostgreSQL acceptance for the recycle-bin purge (ADR 0035 G2): the
//! worker finds due workspaces through its bounded discovery routine only,
//! and purges each under forced RLS as the platform, leaving the other
//! workspaces alone. Run with `bash scripts/test-plugin-runtime.sh`.

use platform_objects::store::PostgresObjectStore;
use platform_objects::*;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;
use worker::recycle_bin::purge_due_recycle_bins;

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is set"));
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect {variable}: {error}"))
}

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

/// A workspace with one deleted note.
async fn workspace(app: &PgPool, migrator: &PgPool, name: &str) -> (Uuid, Record) {
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, $2)")
        .bind(tenant)
        .bind(name)
        .execute(migrator)
        .await
        .expect("tenant");
    let admin = ObjectService::new(
        Arc::new(PostgresObjectStore::new(app.clone())),
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
    admin
        .create_object(
            CreateObject {
                name: "note".into(),
                label: "Note".into(),
                plural_label: "Notes".into(),
                description: None,
                name_label: None,
                sharing_model: None,
                grant_hierarchy: None,
                track_history: Some(true),
                searchable: None,
                in_navigation: None,
            },
            context(),
        )
        .await
        .expect("object");
    let note = admin
        .create_record(
            "custom:note",
            json!({"name": "Old"}).as_object().expect("body"),
            context(),
        )
        .await
        .expect("record");
    let note = admin
        .delete_record("custom:note", &note.public_id, note.version, context())
        .await
        .expect("delete");
    (tenant, note)
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn the_worker_purges_due_recycle_bins_in_every_workspace() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let (first, first_note) = workspace(&app, &migrator, "Purge one").await;
    let (second, second_note) = workspace(&app, &migrator, "Purge two").await;

    // The worker role reads no engine table itself.
    let direct = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM records")
        .fetch_one(&worker)
        .await;
    assert!(direct.is_err(), "the worker role has no table access");

    // Not yet due: nothing is found or purged.
    let pass = purge_due_recycle_bins(&worker, &app, chrono::Utc::now(), Duration::from_secs(30))
        .await
        .expect("pass");
    let present = |tenant: Uuid, id: Uuid| {
        let migrator = migrator.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM records WHERE tenant_id = $1 AND id = $2",
            )
            .bind(tenant)
            .bind(id)
            .fetch_one(&migrator)
            .await
            .expect("count")
        }
    };
    assert_eq!(present(first, first_note.id).await, 1, "{pass:?}");

    // Sixteen days on, both workspaces are found and purged; a second pass
    // finds nothing more in them.
    let later = chrono::Utc::now() + chrono::TimeDelta::days(16);
    let mut purged = 0;
    for _ in 0..20 {
        let pass = purge_due_recycle_bins(&worker, &app, later, Duration::from_secs(30))
            .await
            .expect("pass");
        assert_eq!(pass.errors, 0, "{pass:?}");
        purged += pass.records;
        if present(first, first_note.id).await == 0 && present(second, second_note.id).await == 0 {
            break;
        }
    }
    assert!(purged >= 2);
    assert_eq!(present(first, first_note.id).await, 0);
    assert_eq!(present(second, second_note.id).await, 0);
    let due = platform_db::due_recycle_bin_purges(&worker, later - RECYCLE_BIN_RETENTION, 500)
        .await
        .expect("discovery");
    assert!(!due.contains(&first) && !due.contains(&second));
    for tenant in [first, second] {
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_log WHERE tenant_id = $1 \
             AND action = 'records.recycle_bin.purged' AND actor_id IS NULL",
        )
        .bind(tenant)
        .fetch_one(&migrator)
        .await
        .expect("audit");
        assert_eq!(audited, 1);
    }
}
