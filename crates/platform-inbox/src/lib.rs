#![forbid(unsafe_code)]

use async_trait::async_trait;
use platform_events::{EventEnvelope, EventSubjectError, MAX_EVENT_ENVELOPE_BYTES};
use sqlx::{PgPool, Postgres, Transaction};
use std::fmt;
use thiserror::Error;

pub const MAX_CONSUMER_ID_BYTES: usize = 100;
const MAX_CONTROL_CODE_BYTES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ConsumerId(String);

impl ConsumerId {
    pub fn parse(value: &str) -> Result<Self, InvalidConsumerId> {
        valid_consumer_id(value)
            .then(|| Self(value.to_owned()))
            .ok_or(InvalidConsumerId)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConsumerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("consumer identity is invalid")]
pub struct InvalidConsumerId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrokerDisposition {
    Acknowledge,
    Retry,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedSchemaDisposition {
    Retry,
    Terminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsumptionStatus {
    Applied,
    Duplicate,
    /// The owning plugin is not active for the event's tenant. The delivery
    /// is acknowledged without effects and without an inbox marker.
    SkippedInactive,
}

impl ConsumptionStatus {
    pub const fn disposition(self) -> BrokerDisposition {
        BrokerDisposition::Acknowledge
    }

    pub const fn control(self) -> &'static str {
        match self {
            Self::Applied => "consumer.applied",
            Self::Duplicate => "consumer.duplicate",
            Self::SkippedInactive => "consumer.plugin_inactive",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerFailureKind {
    Retryable,
    Poison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandlerFailure {
    kind: HandlerFailureKind,
    control: &'static str,
}

impl HandlerFailure {
    pub fn retryable(control: &'static str) -> Result<Self, InvalidControlCode> {
        Self::new(HandlerFailureKind::Retryable, control)
    }

    pub fn poison(control: &'static str) -> Result<Self, InvalidControlCode> {
        Self::new(HandlerFailureKind::Poison, control)
    }

    fn new(kind: HandlerFailureKind, control: &'static str) -> Result<Self, InvalidControlCode> {
        if !valid_control_code(control) {
            return Err(InvalidControlCode);
        }
        Ok(Self { kind, control })
    }

    pub const fn kind(self) -> HandlerFailureKind {
        self.kind
    }

    pub const fn control(self) -> &'static str {
        self.control
    }
}

impl fmt::Display for HandlerFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "consumer handler failed ({})", self.control)
    }
}

impl std::error::Error for HandlerFailure {}

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
#[error("consumer control code is invalid")]
pub struct InvalidControlCode;

#[derive(Debug, Error)]
pub enum ConsumptionError {
    #[error("event envelope exceeds the consumer size limit")]
    OversizedEnvelope,
    #[error("event envelope is not valid JSON")]
    InvalidEnvelope(#[source] serde_json::Error),
    #[error("event envelope uses an unsupported schema version")]
    UnsupportedSchemaVersion,
    #[error("event envelope has an unsafe subject contract")]
    UnsafeSubject,
    #[error("broker subject does not match the event envelope")]
    SubjectMismatch,
    #[error("consumer handler rejected the event ({})", .0.control())]
    Handler(HandlerFailure),
    #[error("consumer gate deferred the event")]
    Deferred,
    #[error("consumer identity is outside its plugin execution context")]
    PluginContext,
    #[error("inbox database operation failed")]
    Database(#[source] sqlx::Error),
}

impl ConsumptionError {
    pub const fn disposition(
        &self,
        unsupported_schema: UnsupportedSchemaDisposition,
    ) -> BrokerDisposition {
        match self {
            // A context mismatch is a deployment error, never a fact about the
            // event, so the delivery stays retryable rather than being dropped.
            Self::Database(_) | Self::Deferred | Self::PluginContext => BrokerDisposition::Retry,
            Self::UnsupportedSchemaVersion => match unsupported_schema {
                UnsupportedSchemaDisposition::Retry => BrokerDisposition::Retry,
                UnsupportedSchemaDisposition::Terminate => BrokerDisposition::Terminate,
            },
            Self::Handler(failure) => match failure.kind() {
                HandlerFailureKind::Retryable => BrokerDisposition::Retry,
                HandlerFailureKind::Poison => BrokerDisposition::Terminate,
            },
            Self::OversizedEnvelope
            | Self::InvalidEnvelope(_)
            | Self::UnsafeSubject
            | Self::SubjectMismatch => BrokerDisposition::Terminate,
        }
    }

    pub const fn control(&self) -> &'static str {
        match self {
            Self::OversizedEnvelope => "consumer.oversized_envelope",
            Self::InvalidEnvelope(_) => "consumer.invalid_envelope",
            Self::UnsupportedSchemaVersion => "consumer.unsupported_schema",
            Self::UnsafeSubject => "consumer.unsafe_subject",
            Self::SubjectMismatch => "consumer.subject_mismatch",
            Self::Handler(failure) => failure.control(),
            Self::Database(_) => "consumer.database",
            Self::Deferred => "consumer.deferred",
            Self::PluginContext => "consumer.plugin_context",
        }
    }
}

/// A gate's decision for one delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Apply the event.
    Permit,
    /// The consumer is not active for this tenant: acknowledge without
    /// effects or an inbox marker.
    Skip,
    /// This process cannot decide or serve the event now (for example it
    /// links a different plugin version): leave it for redelivery.
    Defer,
}

/// Decide inside the tenant transaction whether a consumer may apply an event.
///
/// Implementations should lock the authority they read (for example the
/// tenant plugin installation) so it cannot change before commit. An error
/// fails closed and leaves the delivery retryable.
#[async_trait]
pub trait ConsumptionGate: Send + Sync {
    async fn decide(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<GateDecision, sqlx::Error>;
}

#[async_trait]
pub trait EventHandler: Send + Sync {
    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<(), HandlerFailure>;
}

/// Validate a raw broker delivery before it can select a tenant transaction.
///
/// The canonical subject embeds the tenant and event type. Recomputing it from
/// the bounded envelope prevents a caller-controlled subject/envelope mismatch
/// from selecting another tenant's transaction context.
pub fn validate_delivery(subject: &str, payload: &[u8]) -> Result<EventEnvelope, ConsumptionError> {
    if payload.len() > MAX_EVENT_ENVELOPE_BYTES {
        return Err(ConsumptionError::OversizedEnvelope);
    }
    let event: EventEnvelope =
        serde_json::from_slice(payload).map_err(ConsumptionError::InvalidEnvelope)?;
    let canonical_subject = event.jetstream_subject().map_err(|error| match error {
        EventSubjectError::UnsupportedSchemaVersion => ConsumptionError::UnsupportedSchemaVersion,
        EventSubjectError::UnsafeEventType => ConsumptionError::UnsafeSubject,
    })?;
    if canonical_subject != subject {
        return Err(ConsumptionError::SubjectMismatch);
    }
    Ok(event)
}

/// Apply one event through a tenant-scoped inbox transaction.
///
/// The marker is inserted before the handler and committed with its side
/// effects. A duplicate committed marker skips the handler. Any handler or
/// commit failure leaves the broker delivery retryable and never acknowledges
/// partially committed work.
pub async fn consume(
    pool: &PgPool,
    plugin_id: &str,
    consumer_id: &ConsumerId,
    subject: &str,
    payload: &[u8],
    handler: &dyn EventHandler,
) -> Result<ConsumptionStatus, ConsumptionError> {
    consume_inner(
        pool,
        plugin_id,
        consumer_id,
        subject,
        payload,
        None,
        handler,
    )
    .await
}

/// Apply one event only if `gate` permits it in the same tenant transaction.
///
/// The gate runs before the inbox marker, so a refused delivery leaves no
/// marker and no side effects: the event can still be applied if it is
/// redelivered after the gate opens again.
pub async fn consume_gated(
    pool: &PgPool,
    plugin_id: &str,
    consumer_id: &ConsumerId,
    subject: &str,
    payload: &[u8],
    gate: &dyn ConsumptionGate,
    handler: &dyn EventHandler,
) -> Result<ConsumptionStatus, ConsumptionError> {
    consume_inner(
        pool,
        plugin_id,
        consumer_id,
        subject,
        payload,
        Some(gate),
        handler,
    )
    .await
}

/// Whether a consumer identity belongs to the plugin whose context runs it.
pub fn consumer_in_plugin(plugin_id: &str, consumer_id: &ConsumerId) -> bool {
    platform_db::valid_plugin_id(plugin_id)
        && consumer_id
            .as_str()
            .strip_prefix(plugin_id)
            .is_some_and(|suffix| suffix.len() > 1 && suffix.starts_with('.'))
}

async fn consume_inner(
    pool: &PgPool,
    plugin_id: &str,
    consumer_id: &ConsumerId,
    subject: &str,
    payload: &[u8],
    gate: Option<&dyn ConsumptionGate>,
    handler: &dyn EventHandler,
) -> Result<ConsumptionStatus, ConsumptionError> {
    let event = validate_delivery(subject, payload)?;
    if !consumer_in_plugin(plugin_id, consumer_id) {
        return Err(ConsumptionError::PluginContext);
    }
    let mut transaction = platform_db::begin_plugin_tx(pool, event.tenant_id, plugin_id)
        .await
        .map_err(|platform_db::DbError::Sqlx(error)| ConsumptionError::Database(error))?;
    if let Some(gate) = gate {
        let decision = gate
            .decide(&mut transaction, &event)
            .await
            .map_err(ConsumptionError::Database)?;
        if decision != GateDecision::Permit {
            transaction
                .rollback()
                .await
                .map_err(ConsumptionError::Database)?;
            return match decision {
                GateDecision::Skip => Ok(ConsumptionStatus::SkippedInactive),
                _ => Err(ConsumptionError::Deferred),
            };
        }
    }
    let inserted = sqlx::query(
        r#"
        INSERT INTO inbox (tenant_id, consumer_id, event_id)
        VALUES ($1, $2, $3)
        ON CONFLICT (tenant_id, consumer_id, event_id) DO NOTHING
        "#,
    )
    .bind(event.tenant_id)
    .bind(consumer_id.as_str())
    .bind(event.id)
    .execute(&mut *transaction)
    .await
    .map_err(ConsumptionError::Database)?;

    if inserted.rows_affected() == 0 {
        transaction
            .commit()
            .await
            .map_err(ConsumptionError::Database)?;
        return Ok(ConsumptionStatus::Duplicate);
    }

    if let Err(failure) = handler.handle(&mut transaction, &event).await {
        transaction
            .rollback()
            .await
            .map_err(ConsumptionError::Database)?;
        return Err(ConsumptionError::Handler(failure));
    }

    transaction
        .commit()
        .await
        .map_err(ConsumptionError::Database)?;
    Ok(ConsumptionStatus::Applied)
}

fn valid_consumer_id(value: &str) -> bool {
    (1..=MAX_CONSUMER_ID_BYTES).contains(&value.len())
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .as_bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
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
    use uuid::Uuid;

    fn event() -> EventEnvelope {
        EventEnvelope::with_context(
            "platform.contacts.person-created.v1",
            "platform.contacts",
            Uuid::from_u128(1),
            None,
            Uuid::from_u128(2),
            None,
            json!({"entity_id": "per_example", "version": 1}),
        )
    }

    #[test]
    fn delivery_validation_binds_subject_to_the_canonical_tenant_envelope() {
        let event = event();
        let payload = serde_json::to_vec(&event).unwrap();
        let subject = event.jetstream_subject().unwrap();
        assert_eq!(validate_delivery(&subject, &payload).unwrap(), event);

        let error = validate_delivery(
            "platform.events.v1.tenant.00000000-0000-0000-0000-000000000999.platform.contacts.person-created.v1",
            &payload,
        )
        .unwrap_err();
        assert_eq!(error.control(), "consumer.subject_mismatch");
        assert_eq!(
            error.disposition(UnsupportedSchemaDisposition::Terminate),
            BrokerDisposition::Terminate
        );
    }

    #[test]
    fn malformed_and_oversized_deliveries_are_poison_while_future_schema_retries() {
        let invalid =
            validate_delivery("platform.events.v1.tenant.invalid", b"not-json").unwrap_err();
        assert_eq!(invalid.control(), "consumer.invalid_envelope");
        assert_eq!(
            invalid.disposition(UnsupportedSchemaDisposition::Terminate),
            BrokerDisposition::Terminate
        );

        let oversized = validate_delivery(
            "platform.events.v1.tenant.invalid",
            &vec![b'x'; MAX_EVENT_ENVELOPE_BYTES + 1],
        )
        .unwrap_err();
        assert_eq!(oversized.control(), "consumer.oversized_envelope");

        let mut future = event();
        future.schema_version = 2;
        let error = validate_delivery(
            "platform.events.v2.tenant.invalid",
            &serde_json::to_vec(&future).unwrap(),
        )
        .unwrap_err();
        assert_eq!(error.control(), "consumer.unsupported_schema");
        assert_eq!(
            error.disposition(UnsupportedSchemaDisposition::Retry),
            BrokerDisposition::Retry
        );
        assert_eq!(
            error.disposition(UnsupportedSchemaDisposition::Terminate),
            BrokerDisposition::Terminate
        );
    }

    #[test]
    fn consumer_and_handler_controls_are_bounded_safe_machine_codes() {
        for invalid in [
            "",
            "Uppercase",
            "leading space",
            "trailing-",
            "wildcard.*",
            &"x".repeat(MAX_CONSUMER_ID_BYTES + 1),
        ] {
            assert_eq!(ConsumerId::parse(invalid), Err(InvalidConsumerId));
        }
        let consumer_id = ConsumerId::parse("platform.contacts.timeline-v1").unwrap();
        assert_eq!(consumer_id.as_str(), "platform.contacts.timeline-v1");

        assert!(HandlerFailure::retryable("contacts.transient").is_ok());
        assert!(HandlerFailure::poison("contacts.invalid_event").is_ok());
        assert!(HandlerFailure::retryable("Contacts secret failure").is_err());
    }

    #[test]
    fn consumers_must_live_in_their_plugin_namespace() {
        let consumer = ConsumerId::parse("platform.contacts.timeline-v1").unwrap();
        assert!(consumer_in_plugin("platform.contacts", &consumer));
        assert!(!consumer_in_plugin("platform.contact", &consumer));
        assert!(!consumer_in_plugin("platform.other", &consumer));
        assert!(!consumer_in_plugin("platform", &consumer));
        assert_eq!(
            ConsumptionError::PluginContext.disposition(UnsupportedSchemaDisposition::Terminate),
            BrokerDisposition::Retry
        );
    }

    #[test]
    fn outcomes_define_ack_retry_and_terminate_without_claiming_exactly_once() {
        assert_eq!(
            ConsumptionStatus::Applied.disposition(),
            BrokerDisposition::Acknowledge
        );
        assert_eq!(
            ConsumptionStatus::Duplicate.disposition(),
            BrokerDisposition::Acknowledge
        );
        assert_eq!(
            ConsumptionStatus::SkippedInactive.disposition(),
            BrokerDisposition::Acknowledge
        );
        assert_eq!(
            ConsumptionStatus::SkippedInactive.control(),
            "consumer.plugin_inactive"
        );
        let retry =
            ConsumptionError::Handler(HandlerFailure::retryable("contacts.transient").unwrap());
        assert_eq!(
            retry.disposition(UnsupportedSchemaDisposition::Terminate),
            BrokerDisposition::Retry
        );
        let poison =
            ConsumptionError::Handler(HandlerFailure::poison("contacts.invalid_event").unwrap());
        assert_eq!(
            poison.disposition(UnsupportedSchemaDisposition::Retry),
            BrokerDisposition::Terminate
        );
    }
}
