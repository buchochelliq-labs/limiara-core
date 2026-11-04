use crate::model::{ContactCustomFieldDefinition, ContactCustomFieldTarget, Organisation, Person};
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

pub const CONTACTS_PLUGIN_ID: &str = "platform.contacts";
pub const CONTACTS_EVENT_SCHEMA_VERSION: u16 = 1;

pub const PERSON_CREATED_V1: &str = "platform.contacts.person-created.v1";
pub const PERSON_UPDATED_V1: &str = "platform.contacts.person-updated.v1";
pub const PERSON_ARCHIVED_V1: &str = "platform.contacts.person-archived.v1";
pub const ORGANISATION_CREATED_V1: &str = "platform.contacts.organisation-created.v1";
pub const ORGANISATION_UPDATED_V1: &str = "platform.contacts.organisation-updated.v1";
pub const ORGANISATION_ARCHIVED_V1: &str = "platform.contacts.organisation-archived.v1";
pub const PERSON_DELETED_V1: &str = "platform.contacts.person-deleted.v1";
pub const PERSON_UNDELETED_V1: &str = "platform.contacts.person-undeleted.v1";
pub const ORGANISATION_DELETED_V1: &str = "platform.contacts.organisation-deleted.v1";
pub const ORGANISATION_UNDELETED_V1: &str = "platform.contacts.organisation-undeleted.v1";
pub const CUSTOM_FIELD_DEFINITION_CREATED_V1: &str =
    "platform.contacts.custom-field-definition-created.v1";
pub const CUSTOM_FIELD_DEFINITION_UPDATED_V1: &str =
    "platform.contacts.custom-field-definition-updated.v1";
pub const CUSTOM_FIELD_DEFINITION_DISABLED_V1: &str =
    "platform.contacts.custom-field-definition-disabled.v1";
pub const CUSTOM_FIELD_DEFINITION_ARCHIVED_V1: &str =
    "platform.contacts.custom-field-definition-archived.v1";

pub const CONTACTS_EVENT_TYPES: [&str; 14] = [
    PERSON_CREATED_V1,
    PERSON_UPDATED_V1,
    PERSON_ARCHIVED_V1,
    PERSON_DELETED_V1,
    PERSON_UNDELETED_V1,
    ORGANISATION_CREATED_V1,
    ORGANISATION_UPDATED_V1,
    ORGANISATION_ARCHIVED_V1,
    ORGANISATION_DELETED_V1,
    ORGANISATION_UNDELETED_V1,
    CUSTOM_FIELD_DEFINITION_CREATED_V1,
    CUSTOM_FIELD_DEFINITION_UPDATED_V1,
    CUSTOM_FIELD_DEFINITION_DISABLED_V1,
    CUSTOM_FIELD_DEFINITION_ARCHIVED_V1,
];

/// Public payload shared by every v1 person lifecycle event.
///
/// Personal attributes deliberately remain outside the integration event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersonEventPayload {
    pub entity_id: String,
    pub version: i64,
}

/// Public payload shared by every v1 organisation lifecycle event.
///
/// Organisation names and contact details deliberately remain outside the
/// integration event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrganisationEventPayload {
    pub entity_id: String,
    pub version: i64,
}

/// Public payload shared by every v1 custom-field definition lifecycle event.
///
/// This identifies the changed schema element without exposing its API name,
/// label, options, validation rules, classification, purpose, or retention
/// policy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CustomFieldDefinitionEventPayload {
    pub entity_id: Uuid,
    pub applies_to: ContactCustomFieldTarget,
    pub version: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvidenceContext {
    pub(crate) actor_id: Uuid,
    pub(crate) correlation_id: Uuid,
    pub(crate) causation_id: Option<Uuid>,
}

impl EvidenceContext {
    pub(crate) const fn new(
        actor_id: Uuid,
        correlation_id: Uuid,
        causation_id: Option<Uuid>,
    ) -> Self {
        Self {
            actor_id,
            correlation_id,
            causation_id,
        }
    }
}

pub(crate) enum ContactsMutation<'a> {
    PersonCreated(&'a Person),
    PersonUpdated(&'a Person),
    PersonArchived(&'a Person),
    PersonDeleted(&'a Person),
    PersonUndeleted(&'a Person),
    OrganisationCreated(&'a Organisation),
    OrganisationUpdated(&'a Organisation),
    OrganisationArchived(&'a Organisation),
    OrganisationDeleted(&'a Organisation),
    OrganisationUndeleted(&'a Organisation),
    CustomFieldDefinitionCreated(&'a ContactCustomFieldDefinition),
    CustomFieldDefinitionUpdated(&'a ContactCustomFieldDefinition),
    CustomFieldDefinitionDisabled(&'a ContactCustomFieldDefinition),
    CustomFieldDefinitionArchived(&'a ContactCustomFieldDefinition),
}

pub(crate) struct MutationEvidence {
    pub(crate) audit: AuditEvent,
    pub(crate) event: EventEnvelope,
}

impl ContactsMutation<'_> {
    pub(crate) fn records(
        &self,
        context: EvidenceContext,
    ) -> Result<MutationEvidence, serde_json::Error> {
        let tenant_id = self.tenant_id();
        let event = EventEnvelope::with_context(
            self.event_type(),
            CONTACTS_PLUGIN_ID,
            tenant_id,
            Some(context.actor_id),
            context.correlation_id,
            context.causation_id,
            self.payload()?,
        );
        let (action, target, metadata) = self.audit_descriptor(event.id)?;
        let mut audit = AuditEvent::with_context(
            Some(tenant_id),
            Some(context.actor_id),
            context.correlation_id,
            context.causation_id,
            action,
            target,
            metadata,
        );
        audit.occurred_at = event.occurred_at;

        Ok(MutationEvidence { audit, event })
    }

    fn event_type(&self) -> &'static str {
        match self {
            Self::PersonCreated(_) => PERSON_CREATED_V1,
            Self::PersonUpdated(_) => PERSON_UPDATED_V1,
            Self::PersonArchived(_) => PERSON_ARCHIVED_V1,
            Self::PersonDeleted(_) => PERSON_DELETED_V1,
            Self::PersonUndeleted(_) => PERSON_UNDELETED_V1,
            Self::OrganisationCreated(_) => ORGANISATION_CREATED_V1,
            Self::OrganisationUpdated(_) => ORGANISATION_UPDATED_V1,
            Self::OrganisationArchived(_) => ORGANISATION_ARCHIVED_V1,
            Self::OrganisationDeleted(_) => ORGANISATION_DELETED_V1,
            Self::OrganisationUndeleted(_) => ORGANISATION_UNDELETED_V1,
            Self::CustomFieldDefinitionCreated(_) => CUSTOM_FIELD_DEFINITION_CREATED_V1,
            Self::CustomFieldDefinitionUpdated(_) => CUSTOM_FIELD_DEFINITION_UPDATED_V1,
            Self::CustomFieldDefinitionDisabled(_) => CUSTOM_FIELD_DEFINITION_DISABLED_V1,
            Self::CustomFieldDefinitionArchived(_) => CUSTOM_FIELD_DEFINITION_ARCHIVED_V1,
        }
    }

    fn tenant_id(&self) -> Uuid {
        match self {
            Self::PersonCreated(person)
            | Self::PersonUpdated(person)
            | Self::PersonArchived(person)
            | Self::PersonDeleted(person)
            | Self::PersonUndeleted(person) => person.tenant_id,
            Self::OrganisationCreated(organisation)
            | Self::OrganisationUpdated(organisation)
            | Self::OrganisationArchived(organisation)
            | Self::OrganisationDeleted(organisation)
            | Self::OrganisationUndeleted(organisation) => organisation.tenant_id,
            Self::CustomFieldDefinitionCreated(definition)
            | Self::CustomFieldDefinitionUpdated(definition)
            | Self::CustomFieldDefinitionDisabled(definition)
            | Self::CustomFieldDefinitionArchived(definition) => definition.tenant_id,
        }
    }

    fn payload(&self) -> Result<Value, serde_json::Error> {
        match self {
            Self::PersonCreated(person)
            | Self::PersonUpdated(person)
            | Self::PersonArchived(person)
            | Self::PersonDeleted(person)
            | Self::PersonUndeleted(person) => serde_json::to_value(PersonEventPayload {
                entity_id: person.public_id.clone(),
                version: person.version,
            }),
            Self::OrganisationCreated(organisation)
            | Self::OrganisationUpdated(organisation)
            | Self::OrganisationArchived(organisation)
            | Self::OrganisationDeleted(organisation)
            | Self::OrganisationUndeleted(organisation) => {
                serde_json::to_value(OrganisationEventPayload {
                    entity_id: organisation.public_id.clone(),
                    version: organisation.version,
                })
            }
            Self::CustomFieldDefinitionCreated(definition)
            | Self::CustomFieldDefinitionUpdated(definition)
            | Self::CustomFieldDefinitionDisabled(definition)
            | Self::CustomFieldDefinitionArchived(definition) => {
                serde_json::to_value(CustomFieldDefinitionEventPayload {
                    entity_id: definition.id,
                    applies_to: definition.applies_to,
                    version: definition.version,
                })
            }
        }
    }

    fn audit_descriptor(
        &self,
        event_id: Uuid,
    ) -> Result<(&'static str, String, Value), serde_json::Error> {
        match self {
            Self::PersonCreated(person) => {
                person_audit("contacts.person.created", person, event_id)
            }
            Self::PersonUpdated(person) => {
                person_audit("contacts.person.updated", person, event_id)
            }
            Self::PersonArchived(person) => {
                person_audit("contacts.person.archived", person, event_id)
            }
            Self::PersonDeleted(person) => {
                person_audit("contacts.person.deleted", person, event_id)
            }
            Self::PersonUndeleted(person) => {
                person_audit("contacts.person.undeleted", person, event_id)
            }
            Self::OrganisationCreated(organisation) => {
                organisation_audit("contacts.organisation.created", organisation, event_id)
            }
            Self::OrganisationUpdated(organisation) => {
                organisation_audit("contacts.organisation.updated", organisation, event_id)
            }
            Self::OrganisationArchived(organisation) => {
                organisation_audit("contacts.organisation.archived", organisation, event_id)
            }
            Self::OrganisationDeleted(organisation) => {
                organisation_audit("contacts.organisation.deleted", organisation, event_id)
            }
            Self::OrganisationUndeleted(organisation) => {
                organisation_audit("contacts.organisation.undeleted", organisation, event_id)
            }
            Self::CustomFieldDefinitionCreated(definition) => definition_audit(
                "contacts.custom-field-definition.created",
                definition,
                event_id,
            ),
            Self::CustomFieldDefinitionUpdated(definition) => definition_audit(
                "contacts.custom-field-definition.updated",
                definition,
                event_id,
            ),
            Self::CustomFieldDefinitionDisabled(definition) => definition_audit(
                "contacts.custom-field-definition.disabled",
                definition,
                event_id,
            ),
            Self::CustomFieldDefinitionArchived(definition) => definition_audit(
                "contacts.custom-field-definition.archived",
                definition,
                event_id,
            ),
        }
    }
}

#[derive(Serialize)]
struct PersonAuditMetadata<'a> {
    event_id: Uuid,
    entity_id: &'a str,
    version: i64,
}

fn person_audit(
    action: &'static str,
    person: &Person,
    event_id: Uuid,
) -> Result<(&'static str, String, Value), serde_json::Error> {
    Ok((
        action,
        format!("person:{}", person.public_id),
        serde_json::to_value(PersonAuditMetadata {
            event_id,
            entity_id: &person.public_id,
            version: person.version,
        })?,
    ))
}

#[derive(Serialize)]
struct OrganisationAuditMetadata<'a> {
    event_id: Uuid,
    entity_id: &'a str,
    version: i64,
}

fn organisation_audit(
    action: &'static str,
    organisation: &Organisation,
    event_id: Uuid,
) -> Result<(&'static str, String, Value), serde_json::Error> {
    Ok((
        action,
        format!("organisation:{}", organisation.public_id),
        serde_json::to_value(OrganisationAuditMetadata {
            event_id,
            entity_id: &organisation.public_id,
            version: organisation.version,
        })?,
    ))
}

#[derive(Serialize)]
struct CustomFieldDefinitionAuditMetadata {
    event_id: Uuid,
    entity_id: Uuid,
    applies_to: ContactCustomFieldTarget,
    version: i64,
}

fn definition_audit(
    action: &'static str,
    definition: &ContactCustomFieldDefinition,
    event_id: Uuid,
) -> Result<(&'static str, String, Value), serde_json::Error> {
    Ok((
        action,
        format!("custom-field-definition:{}", definition.id),
        serde_json::to_value(CustomFieldDefinitionAuditMetadata {
            event_id,
            entity_id: definition.id,
            applies_to: definition.applies_to,
            version: definition.version,
        })?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ContactCustomFieldType, DataClassification};
    use chrono::{TimeZone, Utc};
    use plugin_contract::Manifest;
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet};

    const TENANT_ID: Uuid = Uuid::from_u128(1);
    const ACTOR_ID: Uuid = Uuid::from_u128(2);
    const CORRELATION_ID: Uuid = Uuid::from_u128(3);
    const CAUSATION_ID: Uuid = Uuid::from_u128(4);
    const DEFINITION_ID: Uuid = Uuid::from_u128(5);

    struct ExpectedContract {
        event_type: &'static str,
        audit_action: &'static str,
        audit_target: &'static str,
        payload: Value,
        metadata_without_event_id: Value,
    }

    #[test]
    fn every_mutation_has_an_exact_v1_payload_and_audit_mapping() {
        let person = person();
        let organisation = organisation();
        let definition = definition();
        let expected = [
            ExpectedContract {
                event_type: PERSON_CREATED_V1,
                audit_action: "contacts.person.created",
                audit_target: "person:per_test",
                payload: json!({"entity_id": "per_test", "version": 7}),
                metadata_without_event_id: json!({"entity_id": "per_test", "version": 7}),
            },
            ExpectedContract {
                event_type: PERSON_UPDATED_V1,
                audit_action: "contacts.person.updated",
                audit_target: "person:per_test",
                payload: json!({"entity_id": "per_test", "version": 7}),
                metadata_without_event_id: json!({"entity_id": "per_test", "version": 7}),
            },
            ExpectedContract {
                event_type: PERSON_ARCHIVED_V1,
                audit_action: "contacts.person.archived",
                audit_target: "person:per_test",
                payload: json!({"entity_id": "per_test", "version": 7}),
                metadata_without_event_id: json!({"entity_id": "per_test", "version": 7}),
            },
            ExpectedContract {
                event_type: PERSON_DELETED_V1,
                audit_action: "contacts.person.deleted",
                audit_target: "person:per_test",
                payload: json!({"entity_id": "per_test", "version": 7}),
                metadata_without_event_id: json!({"entity_id": "per_test", "version": 7}),
            },
            ExpectedContract {
                event_type: PERSON_UNDELETED_V1,
                audit_action: "contacts.person.undeleted",
                audit_target: "person:per_test",
                payload: json!({"entity_id": "per_test", "version": 7}),
                metadata_without_event_id: json!({"entity_id": "per_test", "version": 7}),
            },
            ExpectedContract {
                event_type: ORGANISATION_CREATED_V1,
                audit_action: "contacts.organisation.created",
                audit_target: "organisation:org_test",
                payload: json!({"entity_id": "org_test", "version": 11}),
                metadata_without_event_id: json!({"entity_id": "org_test", "version": 11}),
            },
            ExpectedContract {
                event_type: ORGANISATION_UPDATED_V1,
                audit_action: "contacts.organisation.updated",
                audit_target: "organisation:org_test",
                payload: json!({"entity_id": "org_test", "version": 11}),
                metadata_without_event_id: json!({"entity_id": "org_test", "version": 11}),
            },
            ExpectedContract {
                event_type: ORGANISATION_ARCHIVED_V1,
                audit_action: "contacts.organisation.archived",
                audit_target: "organisation:org_test",
                payload: json!({"entity_id": "org_test", "version": 11}),
                metadata_without_event_id: json!({"entity_id": "org_test", "version": 11}),
            },
            ExpectedContract {
                event_type: ORGANISATION_DELETED_V1,
                audit_action: "contacts.organisation.deleted",
                audit_target: "organisation:org_test",
                payload: json!({"entity_id": "org_test", "version": 11}),
                metadata_without_event_id: json!({"entity_id": "org_test", "version": 11}),
            },
            ExpectedContract {
                event_type: ORGANISATION_UNDELETED_V1,
                audit_action: "contacts.organisation.undeleted",
                audit_target: "organisation:org_test",
                payload: json!({"entity_id": "org_test", "version": 11}),
                metadata_without_event_id: json!({"entity_id": "org_test", "version": 11}),
            },
            ExpectedContract {
                event_type: CUSTOM_FIELD_DEFINITION_CREATED_V1,
                audit_action: "contacts.custom-field-definition.created",
                audit_target: "custom-field-definition:00000000-0000-0000-0000-000000000005",
                payload: definition_payload(),
                metadata_without_event_id: definition_payload(),
            },
            ExpectedContract {
                event_type: CUSTOM_FIELD_DEFINITION_UPDATED_V1,
                audit_action: "contacts.custom-field-definition.updated",
                audit_target: "custom-field-definition:00000000-0000-0000-0000-000000000005",
                payload: definition_payload(),
                metadata_without_event_id: definition_payload(),
            },
            ExpectedContract {
                event_type: CUSTOM_FIELD_DEFINITION_DISABLED_V1,
                audit_action: "contacts.custom-field-definition.disabled",
                audit_target: "custom-field-definition:00000000-0000-0000-0000-000000000005",
                payload: definition_payload(),
                metadata_without_event_id: definition_payload(),
            },
            ExpectedContract {
                event_type: CUSTOM_FIELD_DEFINITION_ARCHIVED_V1,
                audit_action: "contacts.custom-field-definition.archived",
                audit_target: "custom-field-definition:00000000-0000-0000-0000-000000000005",
                payload: definition_payload(),
                metadata_without_event_id: definition_payload(),
            },
        ];

        for (mutation, expected) in mutations(&person, &organisation, &definition)
            .into_iter()
            .zip(expected)
        {
            let evidence = mutation.records(context()).expect("serializable evidence");
            assert_eq!(evidence.event.event_type, expected.event_type);
            assert_eq!(evidence.event.schema_version, CONTACTS_EVENT_SCHEMA_VERSION);
            assert_eq!(evidence.event.source_plugin, CONTACTS_PLUGIN_ID);
            assert_eq!(evidence.event.tenant_id, TENANT_ID);
            assert_eq!(evidence.event.actor_id, Some(ACTOR_ID));
            assert_eq!(evidence.event.correlation_id, CORRELATION_ID);
            assert_eq!(evidence.event.causation_id, Some(CAUSATION_ID));
            assert_eq!(evidence.event.payload, expected.payload);

            assert_eq!(evidence.audit.tenant_id, Some(TENANT_ID));
            assert_eq!(evidence.audit.actor_id, Some(ACTOR_ID));
            assert_eq!(evidence.audit.correlation_id, Some(CORRELATION_ID));
            assert_eq!(evidence.audit.causation_id, Some(CAUSATION_ID));
            assert_eq!(evidence.audit.action, expected.audit_action);
            assert_eq!(evidence.audit.target, expected.audit_target);
            assert_eq!(evidence.audit.occurred_at, evidence.event.occurred_at);

            let mut metadata = evidence.audit.metadata;
            let recorded_event_id = metadata
                .as_object_mut()
                .expect("audit metadata is an object")
                .remove("event_id")
                .expect("audit metadata links to the event");
            assert_eq!(recorded_event_id, json!(evidence.event.id));
            assert_eq!(metadata, expected.metadata_without_event_id);
        }
    }

    #[test]
    fn public_payload_and_audit_metadata_are_redacted_for_every_mutation() {
        let person = person();
        let organisation = organisation();
        let definition = definition();

        for mutation in mutations(&person, &organisation, &definition) {
            let evidence = mutation.records(context()).expect("serializable evidence");
            assert_no_private_keys(&evidence.event.payload);
            assert_no_private_keys(&evidence.audit.metadata);

            let encoded = serde_json::to_string(&json!({
                "event": &evidence.event,
                "audit": &evidence.audit,
            }))
            .expect("serializable evidence");
            for marker in [
                "SENSITIVE_PERSON_NAME",
                "person@example.invalid",
                "+440000000001",
                "SENSITIVE_TAG",
                "SENSITIVE_CUSTOM_VALUE",
                "SENSITIVE_ORGANISATION_NAME",
                "organisation@example.invalid",
                "+440000000002",
                "SENSITIVE_LABEL",
                "SENSITIVE_DESCRIPTION",
                "SENSITIVE_OPTION",
                "SENSITIVE_VALIDATION_RULE",
                "SENSITIVE_PURPOSE",
                "customer_segment",
            ] {
                assert!(!encoded.contains(marker), "evidence leaked marker {marker}");
            }
        }
    }

    #[test]
    fn manifest_declarations_and_capabilities_exactly_match_the_catalogue() {
        let manifest = Manifest::parse_toml(include_str!("../../plugin.toml"))
            .expect("Contacts manifest must remain valid");
        let expected = CONTACTS_EVENT_TYPES.into_iter().collect::<BTreeSet<_>>();
        let declared = manifest
            .events
            .publish
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<BTreeSet<_>>();
        let capabilities = manifest
            .capabilities
            .event_publish
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();

        assert_eq!(declared, expected);
        assert_eq!(capabilities, expected);
        assert_eq!(manifest.events.publish.len(), CONTACTS_EVENT_TYPES.len());
        assert_eq!(
            manifest.capabilities.event_publish.len(),
            CONTACTS_EVENT_TYPES.len()
        );
    }

    fn context() -> EvidenceContext {
        EvidenceContext::new(ACTOR_ID, CORRELATION_ID, Some(CAUSATION_ID))
    }

    fn mutations<'a>(
        person: &'a Person,
        organisation: &'a Organisation,
        definition: &'a ContactCustomFieldDefinition,
    ) -> [ContactsMutation<'a>; 14] {
        [
            ContactsMutation::PersonCreated(person),
            ContactsMutation::PersonUpdated(person),
            ContactsMutation::PersonArchived(person),
            ContactsMutation::PersonDeleted(person),
            ContactsMutation::PersonUndeleted(person),
            ContactsMutation::OrganisationCreated(organisation),
            ContactsMutation::OrganisationUpdated(organisation),
            ContactsMutation::OrganisationArchived(organisation),
            ContactsMutation::OrganisationDeleted(organisation),
            ContactsMutation::OrganisationUndeleted(organisation),
            ContactsMutation::CustomFieldDefinitionCreated(definition),
            ContactsMutation::CustomFieldDefinitionUpdated(definition),
            ContactsMutation::CustomFieldDefinitionDisabled(definition),
            ContactsMutation::CustomFieldDefinitionArchived(definition),
        ]
    }

    fn person() -> Person {
        Person {
            id: Uuid::from_u128(10),
            public_id: "per_test".into(),
            tenant_id: TENANT_ID,
            full_name: "SENSITIVE_PERSON_NAME".into(),
            email: Some("person@example.invalid".into()),
            telephone: Some("+440000000001".into()),
            organisation_id: Some(Uuid::from_u128(11)),
            organisation_public_id: Some("org_test".into()),
            tags: vec!["SENSITIVE_TAG".into()],
            custom_fields: BTreeMap::from([(
                "private_note".into(),
                json!("SENSITIVE_CUSTOM_VALUE"),
            )]),
            extensions: BTreeMap::from([(
                "example.loyalty".into(),
                BTreeMap::from([("tier".into(), json!("SENSITIVE_EXTENSION_VALUE"))]),
            )]),
            version: 7,
            archived_at: None,
            created_at: timestamp(),
            updated_at: timestamp(),
            created_by: Some(Uuid::from_u128(12)),
            updated_by: Some(Uuid::from_u128(12)),
            touched_at: timestamp(),
            deleted_at: None,
        }
    }

    fn organisation() -> Organisation {
        Organisation {
            id: Uuid::from_u128(11),
            public_id: "org_test".into(),
            tenant_id: TENANT_ID,
            name: "SENSITIVE_ORGANISATION_NAME".into(),
            email: Some("organisation@example.invalid".into()),
            telephone: Some("+440000000002".into()),
            custom_fields: BTreeMap::from([(
                "private_note".into(),
                json!("SENSITIVE_CUSTOM_VALUE"),
            )]),
            extensions: BTreeMap::from([(
                "example.loyalty".into(),
                BTreeMap::from([("tier".into(), json!("SENSITIVE_EXTENSION_VALUE"))]),
            )]),
            version: 11,
            archived_at: None,
            created_at: timestamp(),
            updated_at: timestamp(),
            created_by: Some(Uuid::from_u128(12)),
            updated_by: Some(Uuid::from_u128(12)),
            touched_at: timestamp(),
            deleted_at: None,
        }
    }

    fn definition() -> ContactCustomFieldDefinition {
        ContactCustomFieldDefinition {
            id: DEFINITION_ID,
            tenant_id: TENANT_ID,
            applies_to: ContactCustomFieldTarget::Person,
            api_name: "customer_segment".into(),
            label: "SENSITIVE_LABEL".into(),
            description: Some("SENSITIVE_DESCRIPTION".into()),
            field_type: ContactCustomFieldType::Select,
            required: true,
            unique_per_tenant: false,
            indexed: true,
            options: vec!["SENSITIVE_OPTION".into()],
            validation_schema: json!({"rule": "SENSITIVE_VALIDATION_RULE"}),
            data_classification: DataClassification::Sensitive,
            purpose: Some("SENSITIVE_PURPOSE".into()),
            retention_days: Some(30),
            encrypted: true,
            version: 13,
            disabled_at: None,
            archived_at: None,
            created_at: timestamp(),
            updated_at: timestamp(),
        }
    }

    fn definition_payload() -> Value {
        json!({
            "entity_id": DEFINITION_ID,
            "applies_to": "person",
            "version": 13,
        })
    }

    fn timestamp() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0)
            .single()
            .expect("valid timestamp")
    }

    fn assert_no_private_keys(value: &Value) {
        match value {
            Value::Object(object) => {
                for (key, nested) in object {
                    assert!(
                        ![
                            "full_name",
                            "name",
                            "email",
                            "telephone",
                            "tags",
                            "custom_fields",
                            "label",
                            "api_name",
                            "description",
                            "field_type",
                            "required",
                            "unique_per_tenant",
                            "indexed",
                            "options",
                            "validation_schema",
                            "data_classification",
                            "purpose",
                            "retention_days",
                            "encrypted",
                        ]
                        .contains(&key.as_str()),
                        "private key {key} appeared in evidence"
                    );
                    assert_no_private_keys(nested);
                }
            }
            Value::Array(values) => values.iter().for_each(assert_no_private_keys),
            _ => {}
        }
    }
}
