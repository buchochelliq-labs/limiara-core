#![forbid(unsafe_code)]
use chrono::{DateTime, Utc};
use platform_events::EventEnvelope;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::{fmt, num::NonZeroU32, time::Duration};
use uuid::Uuid;

const CLAIM_BATCH_SQL: &str = r#"
SELECT id,
       tenant_id,
       event_type,
       envelope,
       created_at,
       attempt_count,
       claim_token,
       claim_expires_at
FROM platform_claim_outbox_batch($1, $2, $3)
"#;

const MARK_PUBLISHED_SQL: &str = "SELECT platform_mark_outbox_published($1, $2, $3)";

const MARK_FAILED_SQL: &str = "SELECT platform_mark_outbox_failed($1, $2, $3, $4, $5)";

const MARK_QUARANTINED_SQL: &str = "SELECT platform_mark_outbox_quarantined($1, $2, $3, $4)";

const BACKLOG_SQL: &str =
    "SELECT pending, claimable, claimed, oldest_pending_at, quarantined FROM platform_outbox_backlog()";

#[derive(Debug, Clone, PartialEq)]
pub struct ClaimedOutboxRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event_type: String,
    pub envelope: Value,
    pub created_at: DateTime<Utc>,
    pub attempt_count: i32,
    pub claim_token: Uuid,
    pub claim_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxBacklog {
    pub pending: i64,
    pub claimable: i64,
    pub claimed: i64,
    pub quarantined: i64,
    pub oldest_pending_at: Option<DateTime<Utc>>,
}

#[derive(Debug)]
pub enum OutboxStoreError {
    Database(sqlx::Error),
    InvalidLease,
    InvalidRetryDelay,
    InvalidFailureCode,
}

impl fmt::Display for OutboxStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database(error) => write!(formatter, "outbox database operation failed: {error}"),
            Self::InvalidLease => {
                formatter.write_str("outbox claim lease must be non-zero and fit in milliseconds")
            }
            Self::InvalidRetryDelay => {
                formatter.write_str("outbox retry delay must be non-zero and fit in milliseconds")
            }
            Self::InvalidFailureCode => formatter.write_str("outbox failure code is invalid"),
        }
    }
}

impl std::error::Error for OutboxStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Database(error) => Some(error),
            Self::InvalidLease | Self::InvalidRetryDelay | Self::InvalidFailureCode => None,
        }
    }
}

impl From<sqlx::Error> for OutboxStoreError {
    fn from(error: sqlx::Error) -> Self {
        Self::Database(error)
    }
}

#[derive(Debug, Clone)]
pub struct OutboxRecord {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub event: EventEnvelope,
    pub published_at: Option<DateTime<Utc>>,
}
#[derive(Default)]
pub struct InMemoryOutbox {
    records: std::sync::Mutex<Vec<OutboxRecord>>,
}
impl InMemoryOutbox {
    pub fn push(&self, event: EventEnvelope) {
        let id = event.id;
        self.records
            .lock()
            .expect("poisoned outbox")
            .push(OutboxRecord {
                id,
                tenant_id: event.tenant_id,
                event,
                published_at: None,
            });
    }
    pub fn pending(&self) -> Vec<OutboxRecord> {
        self.records
            .lock()
            .expect("poisoned outbox")
            .iter()
            .filter(|r| r.published_at.is_none())
            .cloned()
            .collect()
    }
}

/// Append an event through a caller-owned transaction.
///
/// The envelope tenant and ID are also the durable outbox key so downstream
/// message identity and consumer deduplication share one stable pair. This
/// function deliberately never commits or suppresses conflicts.
pub async fn append_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    event: &EventEnvelope,
) -> Result<(), sqlx::Error> {
    event
        .publication_bytes()
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    let envelope =
        serde_json::to_value(event).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    sqlx::query(
        r#"
        INSERT INTO outbox (id, tenant_id, event_type, envelope, created_at)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(event.id)
    .bind(event.tenant_id)
    .bind(&event.event_type)
    .bind(envelope)
    .bind(event.occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Claim a bounded batch in one committed statement.
///
/// PostgreSQL row locks only protect selection and assignment. The expiring
/// token protects broker work after this statement commits, while allowing a
/// crashed worker's rows to be reclaimed later.
pub async fn claim_batch(
    pool: &PgPool,
    batch_size: NonZeroU32,
    lease: Duration,
) -> Result<Vec<ClaimedOutboxRecord>, OutboxStoreError> {
    let lease_ms = duration_millis(lease).ok_or(OutboxStoreError::InvalidLease)?;
    let claim_token = Uuid::now_v7();
    type ClaimedRow = (
        Uuid,
        Uuid,
        String,
        Value,
        DateTime<Utc>,
        i32,
        Uuid,
        DateTime<Utc>,
    );
    let rows: Vec<ClaimedRow> = sqlx::query_as(CLAIM_BATCH_SQL)
        .bind(i64::from(batch_size.get()))
        .bind(claim_token)
        .bind(lease_ms)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                tenant_id,
                event_type,
                envelope,
                created_at,
                attempt_count,
                claim_token,
                claim_expires_at,
            )| ClaimedOutboxRecord {
                id,
                tenant_id,
                event_type,
                envelope,
                created_at,
                attempt_count,
                claim_token,
                claim_expires_at,
            },
        )
        .collect())
}

/// Mark a row published after its broker acknowledgement.
///
/// `false` means that this claim is stale or the row was already published;
/// the caller must not overwrite the newer owner's state.
pub async fn mark_published(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    claim_token: Uuid,
) -> Result<bool, OutboxStoreError> {
    let updated = sqlx::query_scalar::<_, bool>(MARK_PUBLISHED_SQL)
        .bind(tenant_id)
        .bind(id)
        .bind(claim_token)
        .fetch_one(pool)
        .await?;
    Ok(updated)
}

/// Release a failed claim for a later retry without storing broker details.
///
/// The error code is deliberately a bounded machine code. Runtime error text,
/// URLs, credentials, and event payloads must remain in neither this table nor
/// operator logs.
pub async fn mark_failed(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    claim_token: Uuid,
    retry_delay: Duration,
    error_code: &'static str,
) -> Result<bool, OutboxStoreError> {
    if !valid_failure_code(error_code) {
        return Err(OutboxStoreError::InvalidFailureCode);
    }
    let retry_ms = duration_millis(retry_delay).ok_or(OutboxStoreError::InvalidRetryDelay)?;
    let updated = sqlx::query_scalar::<_, bool>(MARK_FAILED_SQL)
        .bind(tenant_id)
        .bind(id)
        .bind(claim_token)
        .bind(retry_ms)
        .bind(error_code)
        .fetch_one(pool)
        .await?;
    Ok(updated)
}

/// Preserve a deterministically invalid row for operator inspection while
/// removing it from the retry queue.
pub async fn mark_quarantined(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    claim_token: Uuid,
    error_code: &'static str,
) -> Result<bool, OutboxStoreError> {
    if !valid_failure_code(error_code) {
        return Err(OutboxStoreError::InvalidFailureCode);
    }
    let updated = sqlx::query_scalar::<_, bool>(MARK_QUARANTINED_SQL)
        .bind(tenant_id)
        .bind(id)
        .bind(claim_token)
        .bind(error_code)
        .fetch_one(pool)
        .await?;
    Ok(updated)
}

pub async fn backlog(pool: &PgPool) -> Result<OutboxBacklog, OutboxStoreError> {
    type BacklogRow = (i64, i64, i64, Option<DateTime<Utc>>, i64);
    let (pending, claimable, claimed, oldest_pending_at, quarantined): BacklogRow =
        sqlx::query_as(BACKLOG_SQL).fetch_one(pool).await?;
    Ok(OutboxBacklog {
        pending,
        claimable,
        claimed,
        quarantined,
        oldest_pending_at,
    })
}

fn duration_millis(duration: Duration) -> Option<i64> {
    if duration.is_zero() {
        return None;
    }
    let milliseconds = i64::try_from(duration.as_millis()).ok()?;
    (milliseconds > 0).then_some(milliseconds)
}

fn valid_failure_code(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_memory_record_uses_envelope_identity() {
        let event = EventEnvelope::new(
            "example.changed.v1",
            "example",
            Uuid::from_u128(1),
            None,
            serde_json::json!({}),
        );
        let event_id = event.id;
        let outbox = InMemoryOutbox::default();

        outbox.push(event);

        assert_eq!(outbox.pending()[0].id, event_id);
    }

    #[test]
    fn claim_statement_uses_the_bounded_worker_capability() {
        assert!(CLAIM_BATCH_SQL.contains("FROM platform_claim_outbox_batch($1, $2, $3)"));
        assert!(!CLAIM_BATCH_SQL.contains("FROM outbox"));
    }

    #[test]
    fn finalisation_uses_claim_owned_worker_capabilities() {
        assert!(MARK_PUBLISHED_SQL.contains("platform_mark_outbox_published($1, $2, $3)"));
        assert!(MARK_FAILED_SQL.contains("platform_mark_outbox_failed($1, $2, $3, $4, $5)"));
        assert!(MARK_QUARANTINED_SQL.contains("platform_mark_outbox_quarantined($1, $2, $3, $4)"));
        for statement in [MARK_PUBLISHED_SQL, MARK_FAILED_SQL, MARK_QUARANTINED_SQL] {
            assert!(!statement.contains("UPDATE outbox"));
        }
    }

    #[test]
    fn backlog_uses_the_non_enumerating_worker_metric() {
        assert!(BACKLOG_SQL.contains("FROM platform_outbox_backlog()"));
        assert!(!BACKLOG_SQL.contains("FROM outbox"));
    }

    #[test]
    fn durations_and_failure_codes_are_bounded_before_database_use() {
        assert_eq!(duration_millis(Duration::ZERO), None);
        assert_eq!(duration_millis(Duration::from_nanos(1)), None);
        assert_eq!(duration_millis(Duration::from_millis(25)), Some(25));
        assert!(valid_failure_code("jetstream.ack-timeout"));
        assert!(!valid_failure_code("JetStream failure"));
        assert!(!valid_failure_code(""));
        assert!(!valid_failure_code(&"a".repeat(65)));
    }
}
