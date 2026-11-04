#![forbid(unsafe_code)]

//! Tenant-scoped recurring jobs contributed by plugins.
//!
//! Scheduling state lives in `plugin_job_schedules`. The worker's bounded role
//! registers and leases schedules across tenants; every run then executes in
//! an RLS-bound tenant transaction that re-checks activation, applies the
//! job's effects, and settles the lease atomically. A run whose lease expired
//! rolls back, so a reclaimed job never commits its effects twice.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::{fmt, num::NonZeroU32, time::Duration};
use thiserror::Error;
use uuid::Uuid;

pub const MAX_CHECKPOINT_BYTES: usize = 16 * 1024;
const MAX_CONTROL_CODE_BYTES: usize = 64;

const ENSURE_SQL: &str = "SELECT platform_ensure_plugin_job_schedules($1, $2, $3)";
const CLAIM_SQL: &str = r#"
SELECT tenant_id, checkpoint, run_count, attempt_count, claim_expires_at
FROM platform_claim_plugin_jobs($1, $2, $3, $4, $5)
"#;
const COMPLETE_SQL: &str = "SELECT platform_complete_plugin_job($1, $2, $3, $4)";
const FAIL_SQL: &str = "SELECT platform_fail_plugin_job($1, $2, $3, $4, $5, $6)";
const RELEASE_SQL: &str = "SELECT platform_release_plugin_job($1, $2, $3, $4, $5)";

/// One leased run of a job for a tenant.
#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedJob {
    pub tenant_id: Uuid,
    pub checkpoint: Value,
    pub run_count: i64,
    pub attempt_count: i32,
    pub claim_token: Uuid,
    pub claim_expires_at: DateTime<Utc>,
}

/// A bounded, machine-readable job failure. Free-form error text never reaches
/// the schedule table or metrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobFailure {
    control: &'static str,
}

impl JobFailure {
    pub fn new(control: &'static str) -> Result<Self, InvalidControlCode> {
        valid_control_code(control)
            .then_some(Self { control })
            .ok_or(InvalidControlCode)
    }

    pub const fn control(self) -> &'static str {
        self.control
    }
}

impl fmt::Display for JobFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "scheduled job failed ({})", self.control)
    }
}

impl std::error::Error for JobFailure {}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("job control code is invalid")]
pub struct InvalidControlCode;

/// A plugin-contributed recurring job.
///
/// `run` receives the tenant transaction and the last committed checkpoint and
/// returns the next checkpoint, which must be a JSON object. All effects must
/// use the provided transaction so they commit or roll back with the lease.
#[async_trait]
pub trait ScheduledJob: Send + Sync {
    async fn run(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        claim: &ClaimedJob,
    ) -> Result<Value, JobFailure>;
}

/// Decide inside the run's tenant transaction whether the job may execute.
/// An error fails closed.
#[async_trait]
pub trait JobGate: Send + Sync {
    async fn permits(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
    ) -> Result<bool, sqlx::Error>;
}

/// The settled result of one claimed run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// Effects and the next schedule committed together.
    Succeeded,
    /// Activation no longer permits the run; nothing executed and the claim
    /// must be released without consuming the due time.
    Inactive,
    /// The lease expired or was replaced before commit; effects rolled back.
    LeaseLost,
    /// The run rolled back and must be recorded as failed.
    Failed(&'static str),
}

impl RunOutcome {
    pub const fn control(self) -> &'static str {
        match self {
            Self::Succeeded => "job.succeeded",
            Self::Inactive => "job.plugin_inactive",
            Self::LeaseLost => "job.lease_lost",
            Self::Failed(control) => control,
        }
    }
}

#[derive(Debug, Error)]
pub enum JobStoreError {
    #[error("job store database operation failed")]
    Database(#[from] sqlx::Error),
    #[error("job lease duration is outside the supported range")]
    InvalidLease,
    #[error("job retry delay is outside the supported range")]
    InvalidRetryDelay,
    #[error("job interval is outside the supported range")]
    InvalidInterval,
}

/// Register a schedule for every tenant with an active installation of the
/// plugin. Existing schedules keep their checkpoint and due time.
pub async fn ensure_schedules(
    pool: &PgPool,
    plugin_id: &str,
    job_id: &str,
    interval_seconds: u32,
) -> Result<u64, JobStoreError> {
    let interval = i32::try_from(interval_seconds).map_err(|_| JobStoreError::InvalidInterval)?;
    let registered: i64 = sqlx::query_scalar(ENSURE_SQL)
        .bind(plugin_id)
        .bind(job_id)
        .bind(interval)
        .fetch_one(pool)
        .await?;
    Ok(u64::try_from(registered).unwrap_or_default())
}

/// Lease a bounded batch of due runs for enabled tenant installations.
pub async fn claim(
    pool: &PgPool,
    plugin_id: &str,
    job_id: &str,
    batch_size: NonZeroU32,
    lease: Duration,
) -> Result<Vec<ClaimedJob>, JobStoreError> {
    let lease_ms = duration_millis(lease).ok_or(JobStoreError::InvalidLease)?;
    let claim_token = Uuid::now_v7();
    type Row = (Uuid, Value, i64, i32, DateTime<Utc>);
    let rows: Vec<Row> = sqlx::query_as(CLAIM_SQL)
        .bind(plugin_id)
        .bind(job_id)
        .bind(i64::from(batch_size.get()))
        .bind(claim_token)
        .bind(lease_ms)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(tenant_id, checkpoint, run_count, attempt_count, claim_expires_at)| ClaimedJob {
                tenant_id,
                checkpoint,
                run_count,
                attempt_count,
                claim_token,
                claim_expires_at,
            },
        )
        .collect())
}

/// Execute one claimed run in its tenant transaction.
///
/// `tenant_pool` must use the RLS-bound runtime role. The caller settles the
/// non-success outcomes with [`release`] or [`fail`] through the worker role.
pub async fn run_claimed(
    tenant_pool: &PgPool,
    plugin_id: &str,
    job_id: &str,
    claim: &ClaimedJob,
    gate: &dyn JobGate,
    job: &dyn ScheduledJob,
) -> RunOutcome {
    const DATABASE: &str = "job.database";
    const ACTIVATION_UNAVAILABLE: &str = "job.activation_unavailable";
    const INVALID_CHECKPOINT: &str = "job.invalid_checkpoint";

    let Ok(mut transaction) =
        platform_db::begin_plugin_tx(tenant_pool, claim.tenant_id, plugin_id).await
    else {
        return RunOutcome::Failed(DATABASE);
    };
    match gate.permits(&mut transaction, claim.tenant_id).await {
        Ok(true) => {}
        Ok(false) => {
            let _ = transaction.rollback().await;
            return RunOutcome::Inactive;
        }
        Err(_) => {
            let _ = transaction.rollback().await;
            return RunOutcome::Failed(ACTIVATION_UNAVAILABLE);
        }
    }
    let checkpoint = match job.run(&mut transaction, claim).await {
        Ok(checkpoint) => checkpoint,
        Err(failure) => {
            let _ = transaction.rollback().await;
            return RunOutcome::Failed(failure.control());
        }
    };
    if !valid_checkpoint(&checkpoint) {
        let _ = transaction.rollback().await;
        return RunOutcome::Failed(INVALID_CHECKPOINT);
    }
    let completed = sqlx::query_scalar::<_, bool>(COMPLETE_SQL)
        .bind(plugin_id)
        .bind(job_id)
        .bind(claim.claim_token)
        .bind(&checkpoint)
        .fetch_one(&mut *transaction)
        .await;
    match completed {
        Ok(true) => match transaction.commit().await {
            Ok(()) => RunOutcome::Succeeded,
            Err(_) => RunOutcome::Failed(DATABASE),
        },
        Ok(false) => {
            let _ = transaction.rollback().await;
            RunOutcome::LeaseLost
        }
        Err(_) => {
            let _ = transaction.rollback().await;
            RunOutcome::Failed(DATABASE)
        }
    }
}

/// Record a failed run and schedule its retry. `false` means the claim is
/// stale and a newer owner's state was left untouched.
pub async fn fail(
    pool: &PgPool,
    plugin_id: &str,
    job_id: &str,
    claim: &ClaimedJob,
    retry: Duration,
    control: &'static str,
) -> Result<bool, JobStoreError> {
    let retry_ms = duration_millis(retry).ok_or(JobStoreError::InvalidRetryDelay)?;
    Ok(sqlx::query_scalar(FAIL_SQL)
        .bind(claim.tenant_id)
        .bind(plugin_id)
        .bind(job_id)
        .bind(claim.claim_token)
        .bind(retry_ms)
        .bind(control)
        .fetch_one(pool)
        .await?)
}

/// Return a claim without running it, deferring the run by `retry` so a
/// schedule this worker cannot serve does not starve other tenants.
pub async fn release(
    pool: &PgPool,
    plugin_id: &str,
    job_id: &str,
    claim: &ClaimedJob,
    retry: Duration,
) -> Result<bool, JobStoreError> {
    let retry_ms = duration_millis(retry).ok_or(JobStoreError::InvalidRetryDelay)?;
    Ok(sqlx::query_scalar(RELEASE_SQL)
        .bind(claim.tenant_id)
        .bind(plugin_id)
        .bind(job_id)
        .bind(claim.claim_token)
        .bind(retry_ms)
        .fetch_one(pool)
        .await?)
}

/// Exponential retry delay for the `attempt`th consecutive failure.
pub fn retry_delay(base: Duration, max: Duration, attempt: i32) -> Duration {
    let exponent = u32::try_from(attempt.saturating_sub(1).clamp(0, 31)).unwrap_or_default();
    base.saturating_mul(2_u32.saturating_pow(exponent)).min(max)
}

fn valid_checkpoint(checkpoint: &Value) -> bool {
    checkpoint.is_object()
        && serde_json::to_vec(checkpoint).is_ok_and(|bytes| bytes.len() <= MAX_CHECKPOINT_BYTES)
}

fn duration_millis(duration: Duration) -> Option<i64> {
    i64::try_from(duration.as_millis())
        .ok()
        .filter(|millis| *millis > 0)
}

fn valid_control_code(value: &str) -> bool {
    (1..=MAX_CONTROL_CODE_BYTES).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn failures_are_bounded_machine_codes() {
        assert_eq!(
            JobFailure::new("contacts.digest_unavailable")
                .unwrap()
                .control(),
            "contacts.digest_unavailable"
        );
        for invalid in [
            "",
            "Upper",
            "has space",
            "x.toolong-0123456789012345678901234567890123456789012345678901234",
        ] {
            assert_eq!(JobFailure::new(invalid), Err(InvalidControlCode));
        }
    }

    #[test]
    fn checkpoints_are_bounded_json_objects() {
        assert!(valid_checkpoint(&json!({"cursor": "abc"})));
        assert!(!valid_checkpoint(&json!(["not", "an", "object"])));
        assert!(!valid_checkpoint(&json!(null)));
        assert!(!valid_checkpoint(
            &json!({"blob": "x".repeat(MAX_CHECKPOINT_BYTES)})
        ));
    }

    #[test]
    fn retry_delay_grows_exponentially_and_is_capped() {
        let base = Duration::from_secs(5);
        let max = Duration::from_secs(60);
        assert_eq!(retry_delay(base, max, 1), Duration::from_secs(5));
        assert_eq!(retry_delay(base, max, 2), Duration::from_secs(10));
        assert_eq!(retry_delay(base, max, 4), Duration::from_secs(40));
        assert_eq!(retry_delay(base, max, 5), max);
        assert_eq!(retry_delay(base, max, i32::MAX), max);
        assert_eq!(retry_delay(base, max, 0), base);
    }

    #[test]
    fn outcomes_have_stable_controls() {
        assert_eq!(RunOutcome::Inactive.control(), "job.plugin_inactive");
        assert_eq!(RunOutcome::LeaseLost.control(), "job.lease_lost");
        assert_eq!(RunOutcome::Failed("job.database").control(), "job.database");
    }
}
