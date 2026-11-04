//! Runs queued background and scheduled automation jobs (ADR 0022 §6).
//!
//! Discovery is the only cross-tenant step and uses the bounded
//! `platform_due_automation_jobs` routine. Each job is then claimed and run
//! in its tenant's own transactions through the object engine, as the member
//! whose change queued it, so tenancy, validation, evidence and history all
//! apply. A job a worker stops holding is claimed again once its lock lapses.

use platform_objects::automation::{run_job, JobOutcome};
use platform_objects::store::PostgresObjectStore;
use platform_objects::ObjectStore;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

/// Jobs discovered per pass.
pub const JOB_BATCH: i32 = 100;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct JobPass {
    pub succeeded: usize,
    pub retrying: usize,
    pub failed: usize,
    pub cancelled: usize,
    /// Due when discovered but claimed by another worker first.
    pub skipped: usize,
    /// Could not be run at all; claimed again once their lock lapses.
    pub errors: usize,
}

/// Runs one pass over the jobs that are due now.
pub async fn run_due_jobs(
    worker_pool: &PgPool,
    tenant_pool: &PgPool,
) -> Result<JobPass, sqlx::Error> {
    let due = platform_db::due_automation_jobs(worker_pool, JOB_BATCH).await?;
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(tenant_pool.clone()));
    let mut pass = JobPass::default();
    for (tenant_id, job_id) in due {
        match run_job(store.clone(), tenant_id, job_id, chrono::Utc::now()).await {
            Ok(JobOutcome::Succeeded) => pass.succeeded += 1,
            Ok(JobOutcome::Retrying) => pass.retrying += 1,
            Ok(JobOutcome::Failed) => {
                tracing::warn!(%tenant_id, %job_id, "automation job failed its last attempt");
                pass.failed += 1;
            }
            Ok(JobOutcome::Cancelled) => pass.cancelled += 1,
            Ok(JobOutcome::NotClaimed) => pass.skipped += 1,
            Err(error) => {
                tracing::warn!(%tenant_id, %job_id, error = %error, "automation job could not run");
                pass.errors += 1;
            }
        }
    }
    Ok(pass)
}

/// Runs job passes for the life of the process: straight away while there
/// is work, otherwise every `interval`.
pub async fn run(worker_pool: PgPool, tenant_pool: PgPool, interval: Duration) {
    loop {
        let busy = match run_due_jobs(&worker_pool, &tenant_pool).await {
            Ok(pass) => {
                let ran = pass.succeeded + pass.retrying + pass.failed + pass.cancelled;
                if ran > 0 || pass.errors > 0 {
                    tracing::info!(
                        succeeded = pass.succeeded,
                        retrying = pass.retrying,
                        failed = pass.failed,
                        cancelled = pass.cancelled,
                        errors = pass.errors,
                        "automation job pass"
                    );
                }
                ran > 0
            }
            Err(error) => {
                tracing::warn!(error = %error, "automation job discovery failed");
                false
            }
        };
        if !busy {
            tokio::time::sleep(interval).await;
        }
    }
}
