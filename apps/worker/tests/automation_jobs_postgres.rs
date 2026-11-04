//! Real-PostgreSQL acceptance for background automations (ADR 0022 §6): the
//! worker finds due jobs across workspaces through its bounded discovery
//! routine only, runs each in its own workspace under forced RLS, and runs
//! each job once. Run with `bash scripts/test-plugin-runtime.sh`.

use platform_objects::store::PostgresObjectStore;
use platform_objects::*;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use uuid::Uuid;
use worker::automation_jobs::run_due_jobs;

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is set"));
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect {variable}: {error}"))
}

fn admin(app: &PgPool, tenant: Uuid) -> ObjectService {
    ObjectService::new(
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
    )
}

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

/// A workspace with a ticket object whose new records get a note in the
/// background, and one new ticket.
async fn workspace(app: &PgPool, migrator: &PgPool, name: &str) -> (ObjectService, String) {
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, $2)")
        .bind(tenant)
        .bind(name)
        .execute(migrator)
        .await
        .expect("tenant");
    let admin = admin(app, tenant);
    let object: CreateObject = serde_json::from_value(
        json!({"name": "ticket", "label": "Ticket", "plural_label": "Tickets"}),
    )
    .expect("object");
    admin
        .create_object(object, context())
        .await
        .expect("ticket");
    let field: CreateField =
        serde_json::from_value(json!({"name": "note", "label": "Note", "field_type": "text"}))
            .expect("field");
    admin
        .create_field("custom:ticket", field, context())
        .await
        .expect("note");
    let automation: CreateAutomation = serde_json::from_value(json!({
        "name": "note_later", "trigger": "create", "timing": "async",
        "actions": [{"type": "update_record", "values": [
            {"field": "custom:note", "formula": "'noted in ' & name"}]}]}))
    .expect("automation");
    admin
        .create_automation("custom:ticket", automation, context())
        .await
        .expect("automation");
    let body = json!({"name": name}).as_object().cloned().expect("body");
    let ticket = admin
        .create_record("custom:ticket", &body, context())
        .await
        .expect("ticket");
    assert!(
        !ticket.values.contains_key("custom:note"),
        "nothing ran yet"
    );
    (admin, ticket.public_id)
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn the_worker_runs_due_jobs_in_each_workspace_once() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let (first, first_ticket) = workspace(&app, &migrator, "North").await;
    let (second, second_ticket) = workspace(&app, &migrator, "South").await;

    // The worker role cannot read jobs directly; it only discovers them.
    let direct = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM automation_jobs")
        .fetch_one(&worker)
        .await;
    assert!(
        direct.is_err() || direct.is_ok_and(|count| count == 0),
        "the worker role sees no jobs without a workspace"
    );

    let mut succeeded = 0;
    for _ in 0..5 {
        let pass = run_due_jobs(&worker, &app).await.expect("pass");
        assert_eq!(pass.errors, 0, "{pass:?}");
        succeeded += pass.succeeded;
        if pass.succeeded == 0 {
            break;
        }
    }
    assert!(succeeded >= 2, "both workspaces' jobs ran");
    for (admin, ticket, name) in [
        (&first, &first_ticket, "North"),
        (&second, &second_ticket, "South"),
    ] {
        let record = admin
            .get_record("custom:ticket", ticket, false)
            .await
            .expect("ticket");
        assert_eq!(
            record.values["custom:note"],
            json!(format!("noted in {name}"))
        );
        let jobs = admin
            .automation_jobs("custom:ticket", "note_later", 10)
            .await
            .expect("jobs");
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            (jobs[0].status, jobs[0].attempts),
            (JobStatus::Succeeded, 1)
        );
    }
    let again = run_due_jobs(&worker, &app).await.expect("again");
    assert_eq!(
        (again.succeeded, again.skipped),
        (0, 0),
        "finished jobs are not discovered again"
    );
}
