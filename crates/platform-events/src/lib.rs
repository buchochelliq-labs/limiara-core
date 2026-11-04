#![forbid(unsafe_code)]
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::Uuid;

/// JetStream subject filter for the first public platform event contract.
pub const JETSTREAM_V1_SUBJECTS: &str = "platform.events.v1.tenant.*.>";
pub const MAX_EVENT_ENVELOPE_BYTES: usize = 256 * 1024;
const MAX_EVENT_TYPE_BYTES: usize = 200;
const MAX_SOURCE_PLUGIN_BYTES: usize = 100;

#[derive(Debug, Clone, Copy, Error, PartialEq, Eq)]
pub enum EventSubjectError {
    #[error("event schema version is not supported by the v1 subject contract")]
    UnsupportedSchemaVersion,
    #[error("event type cannot be represented as a safe JetStream subject")]
    UnsafeEventType,
}

#[derive(Debug, Error)]
pub enum EventContractError {
    #[error(transparent)]
    Subject(#[from] EventSubjectError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("event envelope exceeds the publication size limit")]
    OversizedEnvelope,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventEnvelope {
    pub id: Uuid,
    pub event_type: String,
    pub schema_version: u16,
    pub source_plugin: String,
    pub tenant_id: Uuid,
    pub actor_id: Option<Uuid>,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
    pub payload: Value,
}
impl EventEnvelope {
    pub fn new(
        event_type: impl Into<String>,
        source_plugin: impl Into<String>,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        payload: Value,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            event_type: event_type.into(),
            schema_version: 1,
            source_plugin: source_plugin.into(),
            tenant_id,
            actor_id,
            correlation_id: Uuid::now_v7(),
            causation_id: None,
            occurred_at: Utc::now(),
            payload,
        }
    }

    pub fn with_context(
        event_type: impl Into<String>,
        source_plugin: impl Into<String>,
        tenant_id: Uuid,
        actor_id: Option<Uuid>,
        correlation_id: Uuid,
        causation_id: Option<Uuid>,
        payload: Value,
    ) -> Self {
        Self {
            id: Uuid::now_v7(),
            event_type: event_type.into(),
            schema_version: 1,
            source_plugin: source_plugin.into(),
            tenant_id,
            actor_id,
            correlation_id,
            causation_id,
            occurred_at: Utc::now(),
            payload,
        }
    }

    /// Build the stable tenant-scoped JetStream subject for this envelope.
    ///
    /// Event types are repository-owned dotted identifiers. Restricting every
    /// token prevents a malformed stored row from introducing NATS wildcards
    /// or publishing outside the platform event namespace.
    pub fn jetstream_subject(&self) -> Result<String, EventSubjectError> {
        if self.schema_version != 1 {
            return Err(EventSubjectError::UnsupportedSchemaVersion);
        }
        if self.event_type.len() > MAX_EVENT_TYPE_BYTES
            || self.source_plugin.len() > MAX_SOURCE_PLUGIN_BYTES
            || !safe_event_type(&self.event_type)
            || !safe_event_type(&self.source_plugin)
            || self
                .event_type
                .strip_prefix(&self.source_plugin)
                .and_then(|suffix| suffix.strip_prefix('.'))
                .is_none_or(str::is_empty)
        {
            return Err(EventSubjectError::UnsafeEventType);
        }
        Ok(format!(
            "platform.events.v1.tenant.{}.{}",
            self.tenant_id, self.event_type
        ))
    }

    /// Validate and serialise the canonical broker contract.
    pub fn publication_bytes(&self) -> Result<Vec<u8>, EventContractError> {
        self.jetstream_subject()?;
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_EVENT_ENVELOPE_BYTES {
            return Err(EventContractError::OversizedEnvelope);
        }
        Ok(bytes)
    }
}

fn safe_event_type(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|token| {
            !token.is_empty()
                && token.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || byte == b'-'
                        || byte == b'_'
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_context_is_preserved() {
        let tenant_id = Uuid::from_u128(1);
        let actor_id = Uuid::from_u128(2);
        let correlation_id = Uuid::from_u128(3);
        let causation_id = Uuid::from_u128(4);

        let event = EventEnvelope::with_context(
            "example.changed.v1",
            "example",
            tenant_id,
            Some(actor_id),
            correlation_id,
            Some(causation_id),
            serde_json::json!({"version": 7}),
        );

        assert_eq!(event.schema_version, 1);
        assert_eq!(event.tenant_id, tenant_id);
        assert_eq!(event.actor_id, Some(actor_id));
        assert_eq!(event.correlation_id, correlation_id);
        assert_eq!(event.causation_id, Some(causation_id));
    }

    #[test]
    fn subject_is_versioned_tenant_scoped_and_preserves_the_event_type() {
        let event = EventEnvelope::with_context(
            "platform.contacts.person-created.v1",
            "platform.contacts",
            Uuid::from_u128(1),
            None,
            Uuid::from_u128(2),
            None,
            serde_json::json!({"entity_id": "per_example", "version": 1}),
        );

        assert_eq!(
            event.jetstream_subject().expect("safe subject"),
            "platform.events.v1.tenant.00000000-0000-0000-0000-000000000001.platform.contacts.person-created.v1"
        );
    }

    #[test]
    fn subject_rejects_wildcards_empty_tokens_and_unknown_schema_versions() {
        let mut event = EventEnvelope::new(
            "platform.contacts.person-created.v1",
            "platform.contacts",
            Uuid::from_u128(1),
            None,
            serde_json::json!({}),
        );
        for invalid in [
            "platform.contacts.*",
            "platform..contacts",
            "Platform.contacts",
        ] {
            event.event_type = invalid.into();
            assert_eq!(
                event.jetstream_subject(),
                Err(EventSubjectError::UnsafeEventType)
            );
        }
        event.event_type = "platform.contacts.person-created.v1".into();
        event.schema_version = 2;
        assert_eq!(
            event.jetstream_subject(),
            Err(EventSubjectError::UnsupportedSchemaVersion)
        );

        event.schema_version = 1;
        event.source_plugin = "platform.other".into();
        assert_eq!(
            event.jetstream_subject(),
            Err(EventSubjectError::UnsafeEventType)
        );
    }

    #[test]
    fn publication_contract_rejects_oversized_payloads() {
        let event = EventEnvelope::new(
            "example.changed.v1",
            "example",
            Uuid::from_u128(1),
            None,
            serde_json::json!({"blob": "x".repeat(MAX_EVENT_ENVELOPE_BYTES)}),
        );
        assert!(matches!(
            event.publication_bytes(),
            Err(EventContractError::OversizedEnvelope)
        ));
    }
}
