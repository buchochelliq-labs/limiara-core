//! Runs bulk jobs (ADR 0023 §2).
//!
//! Discovery is the only cross-tenant step and uses the bounded
//! `platform_due_bulk_jobs` routine. Each job is then claimed in its tenant
//! and a batch of its rows is applied through the object engine as the
//! member who submitted it, with their access as the kernel resolves it at
//! that moment, so removed members and revoked permissions take effect
//! between batches. A job a worker stops holding is claimed again once its
//! lock lapses.

use platform_objects::bulk::{claim_bulk_job, process_bulk_job, BulkOutcome};
use platform_objects::store::PostgresObjectStore;
use platform_objects::{ObjectCaller, ObjectStore};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

/// Jobs discovered per pass.
pub const BULK_DISCOVERY: i32 = 20;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BulkPass {
    /// Processed a batch and have rows left.
    pub continuing: usize,
    pub completed: usize,
    pub aborted: usize,
    /// Due when discovered but claimed by another worker first.
    pub skipped: usize,
    /// Could not be run; claimed again once their lock lapses.
    pub errors: usize,
}

/// The submitter as a caller with their current access, or `None` when
/// they are no longer a member.
async fn member(
    tenant_pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<ObjectCaller>, platform_db::DbError> {
    let Some((permissions, roles)) =
        platform_db::member_access(tenant_pool, tenant_id, user_id).await?
    else {
        return Ok(None);
    };
    let reports = platform_db::reporting_reports(tenant_pool, tenant_id, user_id).await?;
    Ok(Some(
        ObjectCaller::new(tenant_id, user_id, permissions, roles).with_reports(reports),
    ))
}

/// Runs one batch of each bulk job that has work now.
pub async fn run_due_bulk_jobs(
    worker_pool: &PgPool,
    tenant_pool: &PgPool,
) -> Result<BulkPass, sqlx::Error> {
    let due = platform_db::due_bulk_jobs(worker_pool, BULK_DISCOVERY).await?;
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(tenant_pool.clone()));
    let mut pass = BulkPass::default();
    for (tenant_id, job_id) in due {
        let job = match claim_bulk_job(&store, tenant_id, job_id, chrono::Utc::now()).await {
            Ok(Some(job)) => job,
            Ok(None) => {
                pass.skipped += 1;
                continue;
            }
            Err(error) => {
                tracing::warn!(%tenant_id, %job_id, error = %error, "bulk job could not be claimed");
                pass.errors += 1;
                continue;
            }
        };
        let caller = match member(tenant_pool, tenant_id, job.actor_id).await {
            Ok(caller) => caller,
            Err(error) => {
                tracing::warn!(%tenant_id, %job_id, error = %error, "bulk job member could not be resolved");
                pass.errors += 1;
                continue;
            }
        };
        match process_bulk_job(store.clone(), tenant_id, job, caller).await {
            Ok(BulkOutcome::Continuing) => pass.continuing += 1,
            Ok(BulkOutcome::Completed) => pass.completed += 1,
            Ok(BulkOutcome::Aborted) => pass.aborted += 1,
            Ok(BulkOutcome::NotClaimed) => pass.skipped += 1,
            Err(error) => {
                tracing::warn!(%tenant_id, %job_id, error = %error, "bulk job batch could not run");
                pass.errors += 1;
            }
        }
    }
    Ok(pass)
}

/// Runs bulk passes for the life of the process: straight away while there
/// is work, otherwise every `interval`.
pub async fn run(worker_pool: PgPool, tenant_pool: PgPool, interval: Duration) {
    loop {
        let busy = match run_due_bulk_jobs(&worker_pool, &tenant_pool).await {
            Ok(pass) => {
                let ran = pass.continuing + pass.completed + pass.aborted;
                if ran > 0 || pass.errors > 0 {
                    tracing::info!(
                        continuing = pass.continuing,
                        completed = pass.completed,
                        aborted = pass.aborted,
                        errors = pass.errors,
                        "bulk job pass"
                    );
                }
                ran > 0
            }
            Err(error) => {
                tracing::warn!(error = %error, "bulk job discovery failed");
                false
            }
        };
        if !busy {
            tokio::time::sleep(interval).await;
        }
    }
}
