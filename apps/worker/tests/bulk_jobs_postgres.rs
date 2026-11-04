//! Real-PostgreSQL acceptance for bulk jobs (ADR 0023 §2): the worker finds
//! jobs across workspaces through its bounded discovery routine only, runs
//! each batch in its workspace under forced RLS as the submitter with their
//! current access, and aborts a job whose submitter left the workspace. Run
//! with `bash scripts/test-plugin-runtime.sh`.

use platform_objects::store::PostgresObjectStore;
use platform_objects::*;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::sync::Arc;
use uuid::Uuid;
use worker::bulk_jobs::run_due_bulk_jobs;

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

/// A workspace with an item object and a member whose role grants
/// `objects.manage` and `records.modify_all`, as the kernel stores them.
async fn workspace(app: &PgPool, migrator: &PgPool, name: &str) -> (Uuid, Uuid, ObjectService) {
    let tenant = Uuid::now_v7();
    let user = Uuid::now_v7();
    let role = Uuid::now_v7();
    for statement in [
        "INSERT INTO tenants (id, display_name) VALUES ($1, $3)",
        "INSERT INTO users (id, display_name) VALUES ($2, $3)",
        "INSERT INTO memberships (tenant_id, user_id) VALUES ($1, $2)",
        "INSERT INTO roles (tenant_id, id, name) VALUES ($1, $4, 'bulk loader')",
        "INSERT INTO role_permissions (tenant_id, role_id, permission_id) \
         VALUES ($1, $4, 'objects.manage'), ($1, $4, 'records.modify_all')",
        "INSERT INTO user_roles (tenant_id, user_id, role_id) VALUES ($1, $2, $4)",
    ] {
        sqlx::query(statement)
            .bind(tenant)
            .bind(user)
            .bind(name)
            .bind(role)
            .execute(migrator)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    let member = ObjectService::new(
        Arc::new(PostgresObjectStore::new(app.clone())),
        ObjectCaller::new(
            tenant,
            user,
            [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            vec![role],
        ),
    );
    let object: CreateObject =
        serde_json::from_value(json!({"name": "item", "label": "Item", "plural_label": "Items"}))
            .expect("object");
    member.create_object(object, context()).await.expect("item");
    (tenant, user, member)
}

async fn submit(member: &ObjectService, name: &str) -> BulkJob {
    member
        .submit_bulk_job(
            "custom:item",
            BulkOperation::Insert,
            &[
                json!({"name": format!("{name} 1")}),
                json!({"name": format!("{name} 2")}),
            ],
            context(),
        )
        .await
        .expect("submit")
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn the_worker_runs_bulk_jobs_as_their_current_members() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let (_, _, north) = workspace(&app, &migrator, "North").await;
    let (south_tenant, south_user, south) = workspace(&app, &migrator, "South").await;
    let north_job = submit(&north, "North").await;
    let south_job = submit(&south, "South").await;

    // The worker role cannot read jobs directly; it only discovers them.
    let direct = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bulk_jobs")
        .fetch_one(&worker)
        .await;
    assert!(
        direct.is_err() || direct.is_ok_and(|count| count == 0),
        "the worker role sees no bulk jobs without a workspace"
    );

    // South's submitter leaves before the worker runs.
    sqlx::query("DELETE FROM user_roles WHERE tenant_id = $1 AND user_id = $2")
        .bind(south_tenant)
        .bind(south_user)
        .execute(&migrator)
        .await
        .expect("roles");
    sqlx::query("DELETE FROM memberships WHERE tenant_id = $1 AND user_id = $2")
        .bind(south_tenant)
        .bind(south_user)
        .execute(&migrator)
        .await
        .expect("membership");

    let mut completed = 0;
    let mut aborted = 0;
    for _ in 0..5 {
        let pass = run_due_bulk_jobs(&worker, &app).await.expect("pass");
        assert_eq!(pass.errors, 0, "{pass:?}");
        completed += pass.completed;
        aborted += pass.aborted;
        if pass.completed + pass.aborted + pass.continuing == 0 {
            break;
        }
    }
    assert!(completed >= 1 && aborted >= 1, "{completed} {aborted}");
    let north_done = north.bulk_job(north_job.id).await.expect("north");
    assert_eq!(
        (north_done.status, north_done.succeeded),
        (BulkStatus::Completed, 2)
    );
    let south_done = south.bulk_job(south_job.id).await.expect("south");
    assert_eq!(
        (south_done.status, south_done.succeeded),
        (BulkStatus::Aborted, 0),
        "a former member's job writes nothing"
    );
    let again = run_due_bulk_jobs(&worker, &app).await.expect("again");
    assert_eq!(
        again,
        Default::default(),
        "finished jobs are not discovered again"
    );
}
