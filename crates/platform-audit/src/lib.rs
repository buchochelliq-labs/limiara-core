#![forbid(unsafe_code)]
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: Uuid,
    pub tenant_id: Option<Uuid>,
    pub actor_id: Option<Uuid>,
    #[serde(default)]
    pub correlation_id: Option<Uuid>,
    #[serde(default)]
    pub causation_id: Option<Uuid>,
    pub action: String,
    pub target: String,
    pub metadata: Value,
    pub occurred_at: DateTime<Utc>,
}
impl AuditEvent {
    pub fn new(
        tenant_id: Option<Uuid>,
        actor_id: Option<Uuid>,
        action: impl Into<String>,
        target: impl Into<String>,
        metadata: Value,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            tenant_id,
            actor_id,
            correlation_id: None,
            causation_id: None,
            action: action.into(),
            target: target.into(),
            metadata,
            occurred_at: Utc::now(),
        }
    }

    pub fn with_context(
        tenant_id: Option<Uuid>,
        actor_id: Option<Uuid>,
        correlation_id: Uuid,
        causation_id: Option<Uuid>,
        action: impl Into<String>,
        target: impl Into<String>,
        metadata: Value,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            tenant_id,
            actor_id,
            correlation_id: Some(correlation_id),
            causation_id,
            action: action.into(),
            target: target.into(),
            metadata,
            occurred_at: Utc::now(),
        }
    }
}

/// Append audit evidence through a caller-owned transaction.
///
/// This function deliberately never commits. The caller must commit the
/// domain mutation, audit row, and outbox row together.
pub async fn append_in_transaction(
    tx: &mut Transaction<'_, Postgres>,
    event: &AuditEvent,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO audit_log (
            id, tenant_id, actor_id, correlation_id, causation_id,
            action, target, metadata, occurred_at
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        "#,
    )
    .bind(event.id)
    .bind(event.tenant_id)
    .bind(event.actor_id)
    .bind(event.correlation_id)
    .bind(event.causation_id)
    .bind(&event.action)
    .bind(&event.target)
    .bind(&event.metadata)
    .bind(event.occurred_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
#[async_trait]
pub trait AuditService: Send + Sync {
    async fn record(&self, event: AuditEvent);
}
#[derive(Default)]
pub struct InMemoryAudit {
    events: std::sync::Mutex<Vec<AuditEvent>>,
}
#[async_trait]
impl AuditService for InMemoryAudit {
    async fn record(&self, event: AuditEvent) {
        self.events.lock().expect("poisoned audit").push(event);
    }
}
impl InMemoryAudit {
    pub fn events(&self) -> Vec<AuditEvent> {
        self.events.lock().expect("poisoned audit").clone()
    }
}
