//! Bulk jobs on every backend (ADR 0023 §2): submission checks, batches,
//! per-row results, updates and deletes by id, aborting, and submitters
//! who lose their membership.

use super::{caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::bulk::{claim_bulk_job, process_bulk_job, BulkOutcome, BULK_BATCH};
use platform_objects::service::ListRecords;
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

async fn names(admin: &ObjectService) -> Vec<String> {
    let mut names: Vec<String> = admin
        .list_records(
            "custom:item",
            ListRecords {
                limit: Some(200),
                ..ListRecords::default()
            },
        )
        .await
        .expect("list")
        .records
        .into_iter()
        .map(|record| record.name)
        .collect();
    names.sort();
    names
}

/// Claims the job and runs one batch as `member`.
async fn batch(
    store: &Arc<dyn ObjectStore>,
    tenant: Uuid,
    job: Uuid,
    member: Option<ObjectCaller>,
) -> BulkOutcome {
    match claim_bulk_job(store, tenant, job, chrono::Utc::now())
        .await
        .expect("claim")
    {
        Some(claimed) => process_bulk_job(store.clone(), tenant, claimed, member)
            .await
            .expect("batch"),
        None => BulkOutcome::NotClaimed,
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin_caller = caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]);
    let admin = service(&store, admin_caller.clone());
    admin
        .create_object(object("item", "Item"), context())
        .await
        .expect("item");
    let mut quantity = field("quantity", FieldType::Number);
    quantity.scale = Some(0);
    admin
        .create_field("custom:item", quantity, context())
        .await
        .expect("quantity");

    // Submissions are checked whole before anything is queued.
    let codes = |error: ObjectError| match error {
        ObjectError::Validation(issues) => issues
            .into_iter()
            .map(|issue| format!("{}:{}", issue.field, issue.code))
            .collect::<Vec<_>>(),
        other => vec![format!("{other:?}")],
    };
    assert_eq!(
        codes(
            admin
                .submit_bulk_job("custom:item", BulkOperation::Insert, &[], context())
                .await
                .expect_err("empty")
        ),
        ["rows:bulk.rows_invalid"]
    );
    assert_eq!(
        codes(
            admin
                .submit_bulk_job(
                    "custom:item",
                    BulkOperation::Update,
                    &[json!({"id": "x", "values": {}}), json!({"values": {}})],
                    context(),
                )
                .await
                .expect_err("malformed")
        ),
        ["rows[1]:bulk.row_invalid"]
    );
    let outsider = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));
    assert!(matches!(
        outsider
            .submit_bulk_job(
                "custom:item",
                BulkOperation::Insert,
                &[json!({"name": "X"})],
                context()
            )
            .await,
        Err(ObjectError::NotFound)
    ));

    // Inserts: each row is its own change, and a bad row fails alone.
    let rows: Vec<Value> = vec![
        json!({"name": "Anchor", "custom:quantity": 3}),
        json!({"name": "Broken", "custom:quantity": "many"}),
        json!({"name": "Chain", "custom:quantity": 5}),
    ];
    let job = admin
        .submit_bulk_job("custom:item", BulkOperation::Insert, &rows, context())
        .await
        .expect("submit");
    assert_eq!(job.status, BulkStatus::Queued);
    assert_eq!(job.total, 3);
    assert!(matches!(
        outsider.bulk_job(job.id).await,
        Err(ObjectError::NotFound)
    ));
    assert_eq!(
        batch(&store, tenant, job.id, Some(admin_caller.clone())).await,
        BulkOutcome::Completed
    );
    assert_eq!(
        batch(&store, tenant, job.id, Some(admin_caller.clone())).await,
        BulkOutcome::NotClaimed,
        "a completed job has no work"
    );
    let done = admin.bulk_job(job.id).await.expect("job");
    assert_eq!(
        (done.status, done.succeeded, done.failed),
        (BulkStatus::Completed, 2, 1)
    );
    assert!(done.completed_at.is_some());
    assert!(matches!(
        admin.abort_bulk_job(job.id, context()).await,
        Err(ObjectError::Conflict("bulk.job_state"))
    ));
    assert_eq!(names(&admin).await, ["Anchor", "Chain"]);
    let failed = admin
        .bulk_job_rows(job.id, Some(BulkRowStatus::Failed), None, 10)
        .await
        .expect("failed rows");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].index, 1);
    assert_eq!(
        failed[0].errors.as_ref().expect("errors")["code"],
        "validation.failed"
    );
    let succeeded = admin
        .bulk_job_rows(job.id, Some(BulkRowStatus::Succeeded), None, 10)
        .await
        .expect("rows");
    let anchor = succeeded[0].record.clone().expect("record id");
    let chain = succeeded[1].record.clone().expect("record id");
    assert_eq!(
        admin
            .bulk_job_rows(job.id, None, Some(0), 10)
            .await
            .expect("after")
            .iter()
            .map(|row| row.index)
            .collect::<Vec<_>>(),
        [1, 2]
    );

    // Updates and deletes name records by id; without expected_version the
    // current version is used, and a stale one fails that row only.
    let update = admin
        .submit_bulk_job(
            "custom:item",
            BulkOperation::Update,
            &[
                json!({"id": anchor, "values": {"name": "Anchor 2"}}),
                json!({"id": chain, "expected_version": 99, "values": {"name": "Chain 2"}}),
                json!({"id": "0zz000000000000000", "values": {"name": "Ghost"}}),
            ],
            context(),
        )
        .await
        .expect("update job");
    batch(&store, tenant, update.id, Some(admin_caller.clone())).await;
    let rows = admin
        .bulk_job_rows(update.id, None, None, 10)
        .await
        .expect("rows");
    assert_eq!(
        rows.iter()
            .map(|row| (
                row.status,
                row.errors.as_ref().map(|errors| errors["code"].clone())
            ))
            .collect::<Vec<_>>(),
        [
            (BulkRowStatus::Succeeded, None),
            (
                BulkRowStatus::Failed,
                Some(json!("records.version_conflict"))
            ),
            (BulkRowStatus::Failed, Some(json!("resource.unavailable"))),
        ]
    );
    let delete = admin
        .submit_bulk_job(
            "custom:item",
            BulkOperation::Delete,
            &[json!({"id": chain})],
            context(),
        )
        .await
        .expect("delete job");
    batch(&store, tenant, delete.id, Some(admin_caller.clone())).await;
    assert_eq!(names(&admin).await, ["Anchor 2"]);

    // Large jobs run in batches, so other jobs get a turn.
    let many: Vec<Value> = (0..BULK_BATCH + 5)
        .map(|index| json!({"name": format!("Bulk {index:03}")}))
        .collect();
    let large = admin
        .submit_bulk_job("custom:item", BulkOperation::Insert, &many, context())
        .await
        .expect("large");
    let waiting = admin
        .submit_bulk_job(
            "custom:item",
            BulkOperation::Insert,
            &[json!({"name": "Waiting"})],
            context(),
        )
        .await
        .expect("waiting job");
    assert_eq!(
        batch(&store, tenant, large.id, Some(admin_caller.clone())).await,
        BulkOutcome::Continuing
    );
    // The large job goes behind the job that was waiting.
    let mut tx = store.begin(tenant).await.expect("tx");
    let due = tx.due_bulk_jobs(chrono::Utc::now(), 10).await.expect("due");
    tx.commit().await.expect("commit");
    assert_eq!(due, [waiting.id, large.id]);
    assert_eq!(
        batch(&store, tenant, waiting.id, Some(admin_caller.clone())).await,
        BulkOutcome::Completed
    );
    let partway = admin.bulk_job(large.id).await.expect("partway");
    assert_eq!(
        (partway.status, partway.succeeded),
        (BulkStatus::Queued, i64::from(BULK_BATCH))
    );
    assert_eq!(
        batch(&store, tenant, large.id, Some(admin_caller.clone())).await,
        BulkOutcome::Completed
    );
    assert_eq!(
        admin.bulk_job(large.id).await.expect("large").succeeded,
        i64::from(BULK_BATCH + 5)
    );

    // Aborting stops a job; its rows stay pending.
    let stopped = admin
        .submit_bulk_job(
            "custom:item",
            BulkOperation::Insert,
            &[json!({"name": "Never"})],
            context(),
        )
        .await
        .expect("job");
    let aborted = admin
        .abort_bulk_job(stopped.id, context())
        .await
        .expect("abort");
    assert_eq!(aborted.status, BulkStatus::Aborted);
    assert_eq!(
        batch(&store, tenant, stopped.id, Some(admin_caller.clone())).await,
        BulkOutcome::NotClaimed
    );
    assert!(matches!(
        admin.abort_bulk_job(stopped.id, context()).await,
        Err(ObjectError::Conflict("bulk.job_state"))
    ));

    // A submitter who is no longer a member aborts the job at its next
    // batch, and nothing is written.
    let orphaned = admin
        .submit_bulk_job(
            "custom:item",
            BulkOperation::Insert,
            &[json!({"name": "Orphan"})],
            context(),
        )
        .await
        .expect("job");
    assert_eq!(
        batch(&store, tenant, orphaned.id, None).await,
        BulkOutcome::Aborted
    );
    let orphaned = admin.bulk_job(orphaned.id).await.expect("job");
    assert_eq!(
        (orphaned.status, orphaned.succeeded),
        (BulkStatus::Aborted, 0)
    );
    assert!(!names(&admin).await.contains(&"Orphan".to_owned()));
}
