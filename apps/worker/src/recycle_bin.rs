//! Purges engine records that have been in the recycle bin for 15 days
//! (ADR 0035 G2).
//!
//! Discovery is the only cross-tenant step and uses the bounded
//! `platform_due_recycle_bin_purges` routine. Each workspace is then purged
//! through the object engine as the platform itself, in its own bounded
//! transactions under its row-level security, with a batch audit entry and
//! one event per purged record. A pass that stops at its time budget, or a
//! workspace whose purge fails, is carried on by the next pass.

use platform_objects::store::PostgresObjectStore;
use platform_objects::{purge_recycle_bin, ObjectStore, RECYCLE_BIN_RETENTION};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};

/// Workspaces examined per pass.
pub const PURGE_DISCOVERY: i32 = 50;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RecycleBinPass {
    /// Workspaces whose due records were all purged.
    pub workspaces: usize,
    /// Records purged across them.
    pub records: u64,
    /// Workspaces that stopped at the time budget; the next pass goes on.
    pub unfinished: usize,
    /// Workspaces whose purge failed; retried on the next pass.
    pub errors: usize,
}

/// Runs one purge pass at `now`.
pub async fn purge_due_recycle_bins(
    worker_pool: &PgPool,
    tenant_pool: &PgPool,
    now: chrono::DateTime<chrono::Utc>,
    budget: Duration,
) -> Result<RecycleBinPass, sqlx::Error> {
    let due = platform_db::due_recycle_bin_purges(
        worker_pool,
        now - RECYCLE_BIN_RETENTION,
        PURGE_DISCOVERY,
    )
    .await?;
    let store: Arc<dyn ObjectStore> = Arc::new(PostgresObjectStore::new(tenant_pool.clone()));
    let mut pass = RecycleBinPass::default();
    for tenant_id in due {
        match purge_recycle_bin(store.clone(), tenant_id, now, budget).await {
            Ok(outcome) => {
                pass.records += outcome.purged;
                if outcome.complete {
                    pass.workspaces += 1;
                } else {
                    pass.unfinished += 1;
                }
            }
            Err(error) => {
                tracing::warn!(%tenant_id, error = %error, "recycle bin purge failed; retrying later");
                pass.errors += 1;
            }
        }
    }
    Ok(pass)
}

/// Runs purge passes for the life of the process: again straight away
/// while a workspace has more due, otherwise every `interval`.
pub async fn run(worker_pool: PgPool, tenant_pool: PgPool, interval: Duration) {
    loop {
        let busy = match purge_due_recycle_bins(
            &worker_pool,
            &tenant_pool,
            chrono::Utc::now(),
            platform_objects::recycle_bin::PURGE_BUDGET,
        )
        .await
        {
            Ok(pass) => {
                if pass.records > 0 || pass.errors > 0 {
                    tracing::info!(
                        workspaces = pass.workspaces,
                        records = pass.records,
                        unfinished = pass.unfinished,
                        errors = pass.errors,
                        "recycle bin purge pass"
                    );
                }
                pass.unfinished > 0
            }
            Err(error) => {
                tracing::warn!(error = %error, "recycle bin purge discovery failed");
                false
            }
        };
        if !busy {
            tokio::time::sleep(interval).await;
        }
    }
}
