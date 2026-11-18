//! Retention of browser draft keys (ADR 0036 §4). A key unused for longer
//! than the longest allowed draft expiry can open no draft that is still
//! kept, so it is deleted. The worker reaches the keys only through the
//! bounded `platform_purge_draft_keys` routine, which returns counts, never
//! key material.

use sqlx::PgPool;
use std::time::Duration;

/// Keys deleted per routine call; a pass repeats until a call deletes fewer.
const PURGE_BATCH: i32 = 1000;

/// Runs one retention pass and returns how many keys it deleted.
pub async fn purge_unused_draft_keys(worker_pool: &PgPool) -> Result<i64, platform_db::DbError> {
    let mut total = 0;
    loop {
        let deleted = platform_db::purge_draft_keys(
            worker_pool,
            platform_db::DRAFT_KEY_RETENTION_DAYS,
            PURGE_BATCH,
        )
        .await?;
        total += deleted;
        if deleted < i64::from(PURGE_BATCH) {
            return Ok(total);
        }
    }
}

/// Runs retention passes for the life of the process.
pub async fn run(worker_pool: PgPool, interval: Duration) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        match purge_unused_draft_keys(&worker_pool).await {
            Ok(0) => {}
            Ok(deleted) => tracing::info!(deleted, "draft key retention pass"),
            Err(error) => tracing::warn!(error = %error, "draft key retention pass failed"),
        }
    }
}
