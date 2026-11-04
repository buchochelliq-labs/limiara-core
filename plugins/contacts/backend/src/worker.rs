//! Contacts background contributions, linked into the worker.
//!
//! Both contributions are declared in `plugins/contacts/plugin.toml`; the
//! worker refuses to start if these implementations and the declarations
//! diverge, and it runs them only for tenants whose Contacts installation is
//! active.

use crate::events::{
    OrganisationEventPayload, PersonEventPayload, CONTACTS_PLUGIN_ID, ORGANISATION_ARCHIVED_V1,
    ORGANISATION_CREATED_V1, ORGANISATION_DELETED_V1, ORGANISATION_UNDELETED_V1,
    ORGANISATION_UPDATED_V1, PERSON_ARCHIVED_V1, PERSON_CREATED_V1, PERSON_DELETED_V1,
    PERSON_UNDELETED_V1, PERSON_UPDATED_V1,
};
use async_trait::async_trait;
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use platform_id::PublicId;
use platform_inbox::{ConsumerId, EventHandler, HandlerFailure};
use platform_jobs::{ClaimedJob, JobFailure, ScheduledJob};
use plugin_contract::{Manifest, ManifestError};
use plugin_runtime_native::{NativeConsumer, NativeJob, NativeWorkerPlugin};
use serde_json::{json, Value};
use sqlx::{Postgres, Transaction};
use std::sync::Arc;
use uuid::Uuid;

pub const ACTIVITY_TIMELINE_CONSUMER_ID: &str = "platform.contacts.activity-timeline-v1";
pub const LIST_CURSOR_CLEANUP_JOB_ID: &str = "platform.contacts.list-cursor-cleanup-v1";

/// Upper bound on cursors removed per tenant run, so one run's transaction
/// stays short; a backlog drains over successive runs.
pub const LIST_CURSOR_CLEANUP_BATCH: i64 = 5_000;

pub const RECYCLE_BIN_PURGE_JOB_ID: &str = "platform.contacts.recycle-bin-purge-v1";

/// How long a deleted record stays in the recycle bin before the worker
/// removes it for good, as in Salesforce (ADR 0014).
pub const RECYCLE_BIN_RETENTION_DAYS: i32 = 15;

/// Upper bound on records purged per kind per tenant run.
pub const RECYCLE_BIN_PURGE_BATCH: i64 = 1_000;

const MANIFEST: &str = include_str!("../../plugin.toml");

/// The Contacts worker plugin: its manifest plus linked implementations.
pub struct ContactsWorkerPlugin {
    manifest: Manifest,
}

impl ContactsWorkerPlugin {
    pub fn new() -> Result<Self, ManifestError> {
        Ok(Self {
            manifest: Manifest::parse_toml(MANIFEST)?,
        })
    }
}

impl NativeWorkerPlugin for ContactsWorkerPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn consumers(&self) -> Vec<NativeConsumer> {
        ConsumerId::parse(ACTIVITY_TIMELINE_CONSUMER_ID)
            .map(|id| NativeConsumer {
                id,
                handler: Arc::new(ActivityTimelineConsumer),
            })
            .into_iter()
            .collect()
    }

    fn jobs(&self) -> Vec<NativeJob> {
        vec![
            NativeJob {
                id: LIST_CURSOR_CLEANUP_JOB_ID.into(),
                job: Arc::new(ListCursorCleanupJob),
            },
            NativeJob {
                id: RECYCLE_BIN_PURGE_JOB_ID.into(),
                job: Arc::new(RecycleBinPurgeJob),
            },
        ]
    }
}

/// Projects person and organisation lifecycle events into
/// `p_contacts.activity`.
pub struct ActivityTimelineConsumer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityType {
    Person,
    Organisation,
}

impl EntityType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Organisation => "organisation",
        }
    }

    const fn prefix(self) -> &'static str {
        match self {
            Self::Person => "per",
            Self::Organisation => "org",
        }
    }
}

fn classify(event_type: &str) -> Option<(EntityType, &'static str)> {
    Some(match event_type {
        PERSON_CREATED_V1 => (EntityType::Person, "created"),
        PERSON_UPDATED_V1 => (EntityType::Person, "updated"),
        PERSON_ARCHIVED_V1 => (EntityType::Person, "archived"),
        PERSON_DELETED_V1 => (EntityType::Person, "deleted"),
        PERSON_UNDELETED_V1 => (EntityType::Person, "undeleted"),
        ORGANISATION_CREATED_V1 => (EntityType::Organisation, "created"),
        ORGANISATION_UPDATED_V1 => (EntityType::Organisation, "updated"),
        ORGANISATION_ARCHIVED_V1 => (EntityType::Organisation, "archived"),
        ORGANISATION_DELETED_V1 => (EntityType::Organisation, "deleted"),
        ORGANISATION_UNDELETED_V1 => (EntityType::Organisation, "undeleted"),
        _ => return None,
    })
}

fn failure(result: Result<HandlerFailure, platform_inbox::InvalidControlCode>) -> HandlerFailure {
    // Static, pattern-valid control codes; covered by unit tests.
    result.expect("static control code is valid")
}

fn poison(control: &'static str) -> HandlerFailure {
    failure(HandlerFailure::poison(control))
}

/// Parse the public entity reference from a v1 lifecycle payload.
fn entity_reference(entity: EntityType, payload: &Value) -> Result<(String, i64), HandlerFailure> {
    let (entity_id, version) = match entity {
        EntityType::Person => serde_json::from_value::<PersonEventPayload>(payload.clone())
            .map(|payload| (payload.entity_id, payload.version)),
        EntityType::Organisation => {
            serde_json::from_value::<OrganisationEventPayload>(payload.clone())
                .map(|payload| (payload.entity_id, payload.version))
        }
    }
    .map_err(|_| poison("contacts.activity_invalid_payload"))?;
    let valid_id =
        PublicId::parse(&entity_id).is_ok_and(|parsed| parsed.prefix() == entity.prefix());
    if !valid_id || version < 1 {
        return Err(poison("contacts.activity_invalid_payload"));
    }
    Ok((entity_id, version))
}

#[async_trait]
impl EventHandler for ActivityTimelineConsumer {
    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<(), HandlerFailure> {
        if event.source_plugin != CONTACTS_PLUGIN_ID {
            return Err(poison("contacts.activity_foreign_source"));
        }
        let (entity, action) =
            classify(&event.event_type).ok_or_else(|| poison("contacts.activity_unknown_event"))?;
        let (entity_public_id, version) = entity_reference(entity, &event.payload)?;
        // The inbox already deduplicates per consumer; the conflict clause
        // keeps the projection idempotent if it is ever rebuilt.
        sqlx::query(
            r#"
            INSERT INTO p_contacts.activity (
              tenant_id, event_id, entity_type, entity_public_id, action,
              entity_version, actor_id, correlation_id, occurred_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
            ON CONFLICT (tenant_id, event_id) DO NOTHING
            "#,
        )
        .bind(event.tenant_id)
        .bind(event.id)
        .bind(entity.as_str())
        .bind(&entity_public_id)
        .bind(action)
        .bind(version)
        .bind(event.actor_id)
        .bind(event.correlation_id)
        .bind(event.occurred_at)
        .execute(&mut **transaction)
        .await
        .map_err(|_| failure(HandlerFailure::retryable("contacts.activity_database")))?;
        Ok(())
    }
}

/// Removes expired keyset-pagination cursors for one tenant per run.
pub struct ListCursorCleanupJob;

#[async_trait]
impl ScheduledJob for ListCursorCleanupJob {
    async fn run(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        claim: &ClaimedJob,
    ) -> Result<Value, JobFailure> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM p_contacts.list_cursors
            WHERE tenant_id = $1
              AND token_hash IN (
                SELECT token_hash
                FROM p_contacts.list_cursors
                WHERE tenant_id = $1 AND expires_at < now()
                ORDER BY expires_at
                LIMIT $2
              )
            "#,
        )
        .bind(claim.tenant_id)
        .bind(LIST_CURSOR_CLEANUP_BATCH)
        .execute(&mut **transaction)
        .await
        .map_err(|_| JobFailure::new("contacts.cursor_cleanup_database").expect("valid control"))?
        .rows_affected();
        let total = claim
            .checkpoint
            .get("deleted_total")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(deleted);
        Ok(json!({ "deleted_total": total, "last_deleted": deleted }))
    }
}

/// Permanently removes people and organisations deleted more than
/// [`RECYCLE_BIN_RETENTION_DAYS`] ago, for one tenant per run (ADR 0014).
/// People linked to a purged organisation lose the link, which is a system
/// change: only their `touched_at` moves. Each run that removes anything
/// leaves one audit entry with the counts.
pub struct RecycleBinPurgeJob;

#[async_trait]
impl ScheduledJob for RecycleBinPurgeJob {
    async fn run(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        claim: &ClaimedJob,
    ) -> Result<Value, JobFailure> {
        let database = || JobFailure::new("contacts.recycle_bin_database").expect("valid control");
        let organisations: Vec<Uuid> = sqlx::query_scalar(
            r#"
            SELECT id FROM p_contacts.organisations
            WHERE tenant_id = $1 AND deleted_at < now() - make_interval(days => $2)
            ORDER BY deleted_at
            LIMIT $3
            "#,
        )
        .bind(claim.tenant_id)
        .bind(RECYCLE_BIN_RETENTION_DAYS)
        .bind(RECYCLE_BIN_PURGE_BATCH)
        .fetch_all(&mut **transaction)
        .await
        .map_err(|_| database())?;
        let unlinked = sqlx::query(
            "UPDATE p_contacts.people SET organisation_id = NULL, touched_at = now() \
             WHERE tenant_id = $1 AND organisation_id = ANY($2)",
        )
        .bind(claim.tenant_id)
        .bind(&organisations)
        .execute(&mut **transaction)
        .await
        .map_err(|_| database())?
        .rows_affected();
        let purged_organisations = sqlx::query(
            "DELETE FROM p_contacts.organisations WHERE tenant_id = $1 AND id = ANY($2)",
        )
        .bind(claim.tenant_id)
        .bind(&organisations)
        .execute(&mut **transaction)
        .await
        .map_err(|_| database())?
        .rows_affected();
        let purged_people = sqlx::query(
            r#"
            DELETE FROM p_contacts.people
            WHERE tenant_id = $1
              AND id IN (
                SELECT id FROM p_contacts.people
                WHERE tenant_id = $1 AND deleted_at < now() - make_interval(days => $2)
                ORDER BY deleted_at
                LIMIT $3
              )
            "#,
        )
        .bind(claim.tenant_id)
        .bind(RECYCLE_BIN_RETENTION_DAYS)
        .bind(RECYCLE_BIN_PURGE_BATCH)
        .execute(&mut **transaction)
        .await
        .map_err(|_| database())?
        .rows_affected();
        if purged_people + purged_organisations > 0 {
            let audit = AuditEvent::with_context(
                Some(claim.tenant_id),
                None,
                Uuid::now_v7(),
                None,
                "contacts.recycle-bin.purged",
                "plugin:platform.contacts",
                json!({
                    "people": purged_people,
                    "organisations": purged_organisations,
                    "people_unlinked": unlinked,
                    "retention_days": RECYCLE_BIN_RETENTION_DAYS,
                }),
            );
            platform_audit::append_in_transaction(transaction, &audit)
                .await
                .map_err(|_| database())?;
        }
        let total = claim
            .checkpoint
            .get("purged_total")
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(purged_people + purged_organisations);
        Ok(json!({
            "purged_total": total,
            "last_people": purged_people,
            "last_organisations": purged_organisations,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_inbox::HandlerFailureKind;
    use plugin_runtime_native::verify_worker_plugin;

    #[test]
    fn linked_contributions_match_the_contacts_manifest() {
        let plugin = ContactsWorkerPlugin::new().expect("Contacts manifest parses");
        verify_worker_plugin(&plugin).expect("implementations match declarations");
        let declared = &plugin.manifest().worker.consumers[0];
        for event in &declared.events {
            assert!(classify(event).is_some(), "{event} has a timeline mapping");
        }
        assert_eq!(plugin.manifest().worker.jobs[0].interval_seconds, 3600);
    }

    #[test]
    fn payloads_must_reference_a_valid_public_id_of_the_right_kind() {
        let person = PublicId::new("per", 7).unwrap();
        let organisation = PublicId::new("org", 7).unwrap();
        assert_eq!(
            entity_reference(
                EntityType::Person,
                &json!({"entity_id": person.as_str(), "version": 3})
            )
            .unwrap(),
            (person.as_str().to_owned(), 3)
        );
        for (entity, payload) in [
            (
                EntityType::Person,
                json!({"entity_id": organisation.as_str(), "version": 1}),
            ),
            (
                EntityType::Person,
                json!({"entity_id": "per_invalid", "version": 1}),
            ),
            (
                EntityType::Organisation,
                json!({"entity_id": organisation.as_str(), "version": 0}),
            ),
            (
                EntityType::Organisation,
                json!({"entity_id": organisation.as_str()}),
            ),
        ] {
            let error = entity_reference(entity, &payload).unwrap_err();
            assert_eq!(error.kind(), HandlerFailureKind::Poison);
            assert_eq!(error.control(), "contacts.activity_invalid_payload");
        }
    }

    #[test]
    fn controls_are_valid_static_codes() {
        for control in [
            "contacts.activity_invalid_payload",
            "contacts.activity_foreign_source",
            "contacts.activity_unknown_event",
        ] {
            assert_eq!(poison(control).control(), control);
        }
        assert!(HandlerFailure::retryable("contacts.activity_database").is_ok());
        assert!(JobFailure::new("contacts.cursor_cleanup_database").is_ok());
    }
}
