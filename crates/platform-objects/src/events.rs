//! Versioned events and audit evidence for engine changes (ADR 0015
//! section 5). Payloads name what changed, never the values, so events can
//! be routed without carrying personal data.

use crate::{FieldDefinition, ObjectDefinition, Record, ENGINE_CONTEXT};
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use serde_json::{json, Value};
use uuid::Uuid;

pub const OBJECT_CREATED_V1: &str = "platform.objects.object-created.v1";
pub const OBJECT_UPDATED_V1: &str = "platform.objects.object-updated.v1";
pub const OBJECT_DELETED_V1: &str = "platform.objects.object-deleted.v1";
pub const FIELD_CREATED_V1: &str = "platform.objects.field-created.v1";
pub const FIELD_UPDATED_V1: &str = "platform.objects.field-updated.v1";
pub const FIELD_DELETED_V1: &str = "platform.objects.field-deleted.v1";
pub const OBJECT_PERMISSIONS_CHANGED_V1: &str = "platform.objects.object-permissions-changed.v1";
pub const RECORD_CREATED_V1: &str = "platform.objects.record-created.v1";
pub const RECORD_UPDATED_V1: &str = "platform.objects.record-updated.v1";
pub const RECORD_DELETED_V1: &str = "platform.objects.record-deleted.v1";
pub const RECORD_UNDELETED_V1: &str = "platform.objects.record-undeleted.v1";
pub const RECORD_OWNER_CHANGED_V1: &str = "platform.objects.record-owner-changed.v1";
pub const RECORD_SHARING_CHANGED_V1: &str = "platform.objects.record-sharing-changed.v1";
pub const VALIDATION_RULE_CREATED_V1: &str = "platform.objects.validation-rule-created.v1";
pub const VALIDATION_RULE_UPDATED_V1: &str = "platform.objects.validation-rule-updated.v1";
pub const VALIDATION_RULE_DELETED_V1: &str = "platform.objects.validation-rule-deleted.v1";
pub const AUTOMATION_CREATED_V1: &str = "platform.objects.automation-created.v1";
pub const AUTOMATION_UPDATED_V1: &str = "platform.objects.automation-updated.v1";
pub const AUTOMATION_DELETED_V1: &str = "platform.objects.automation-deleted.v1";
pub const AUTOMATION_JOB_RETRIED_V1: &str = "platform.objects.automation-job-retried.v1";
pub const AUTOMATION_JOB_CANCELLED_V1: &str = "platform.objects.automation-job-cancelled.v1";
/// A workspace-defined platform event an automation published (ADR 0022
/// §7). Unlike the engine's other events it carries the field values the
/// automation computed, by the administrator's design.
pub const WORKSPACE_EVENT_V1: &str = "platform.objects.workspace-event.v1";

/// Bulk jobs (ADR 0023 §2): submitted, finished and aborted. Each row's
/// change has its own record event, caused by the job.
pub const BULK_JOB_SUBMITTED_V1: &str = "platform.objects.bulk-job-submitted.v1";
pub const BULK_JOB_COMPLETED_V1: &str = "platform.objects.bulk-job-completed.v1";
pub const BULK_JOB_ABORTED_V1: &str = "platform.objects.bulk-job-aborted.v1";

/// Saved reports (ADR 0027): definitions only, never their rows.
pub const REPORT_CREATED_V1: &str = "platform.objects.report-created.v1";
pub const REPORT_UPDATED_V1: &str = "platform.objects.report-updated.v1";
pub const REPORT_DELETED_V1: &str = "platform.objects.report-deleted.v1";

/// Quick actions (ADR 0032): definitions only, never preset values.
pub const QUICK_ACTION_CREATED_V1: &str = "platform.objects.quick-action-created.v1";
pub const QUICK_ACTION_UPDATED_V1: &str = "platform.objects.quick-action-updated.v1";
pub const QUICK_ACTION_DELETED_V1: &str = "platform.objects.quick-action-deleted.v1";

pub const ENGINE_EVENT_TYPES: [&str; 31] = [
    OBJECT_CREATED_V1,
    OBJECT_UPDATED_V1,
    OBJECT_DELETED_V1,
    FIELD_CREATED_V1,
    FIELD_UPDATED_V1,
    FIELD_DELETED_V1,
    OBJECT_PERMISSIONS_CHANGED_V1,
    RECORD_CREATED_V1,
    RECORD_UPDATED_V1,
    RECORD_DELETED_V1,
    RECORD_UNDELETED_V1,
    RECORD_OWNER_CHANGED_V1,
    RECORD_SHARING_CHANGED_V1,
    VALIDATION_RULE_CREATED_V1,
    VALIDATION_RULE_UPDATED_V1,
    VALIDATION_RULE_DELETED_V1,
    AUTOMATION_CREATED_V1,
    AUTOMATION_UPDATED_V1,
    AUTOMATION_DELETED_V1,
    AUTOMATION_JOB_RETRIED_V1,
    AUTOMATION_JOB_CANCELLED_V1,
    WORKSPACE_EVENT_V1,
    BULK_JOB_SUBMITTED_V1,
    BULK_JOB_COMPLETED_V1,
    BULK_JOB_ABORTED_V1,
    REPORT_CREATED_V1,
    REPORT_UPDATED_V1,
    REPORT_DELETED_V1,
    QUICK_ACTION_CREATED_V1,
    QUICK_ACTION_UPDATED_V1,
    QUICK_ACTION_DELETED_V1,
];

/// Who and why, for every piece of evidence in one request.
#[derive(Debug, Clone, Copy)]
pub struct EvidenceContext {
    pub tenant_id: Uuid,
    pub actor_id: Uuid,
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    /// The automation that made the change; named in the event and audit.
    pub automation_id: Option<Uuid>,
    /// Where the change came from (#233). The audit entry keeps it whole;
    /// the event names only its kind, never the detail.
    pub source: Option<crate::MutationSource>,
}

pub enum Change<'a> {
    ObjectCreated(&'a ObjectDefinition),
    ObjectUpdated(&'a ObjectDefinition),
    ObjectDeleted(&'a ObjectDefinition),
    /// A plugin object went out of service with its plugin (`true`) or
    /// came back when the plugin was enabled again (ADR 0033 §2). Its
    /// event is an `object-updated` one; the audit action says which.
    ObjectSuspended(&'a ObjectDefinition, bool),
    FieldCreated(&'a ObjectDefinition, &'a FieldDefinition),
    FieldUpdated(&'a ObjectDefinition, &'a FieldDefinition),
    FieldDeleted(&'a ObjectDefinition, &'a FieldDefinition),
    PermissionsChanged(&'a ObjectDefinition, Uuid),
    RecordCreated(&'a ObjectDefinition, &'a Record, &'a [String]),
    RecordUpdated(&'a ObjectDefinition, &'a Record, &'a [String]),
    RecordDeleted(&'a ObjectDefinition, &'a Record),
    RecordUndeleted(&'a ObjectDefinition, &'a Record),
    /// The new owner is in the record; the previous owner and how many
    /// manual shares the transfer removed are evidence too (ADR 0016).
    OwnerChanged(&'a ObjectDefinition, &'a Record, Option<Uuid>, u64),
    /// A manual share was granted, changed (`Some`) or revoked (`None`).
    SharingChanged(
        &'a ObjectDefinition,
        &'a Record,
        crate::GranteeKind,
        Uuid,
        Option<crate::ShareAccess>,
    ),
    RuleCreated(&'a ObjectDefinition, &'a crate::ValidationRule),
    RuleUpdated(&'a ObjectDefinition, &'a crate::ValidationRule),
    RuleDeleted(&'a ObjectDefinition, &'a crate::ValidationRule),
    AutomationCreated(&'a ObjectDefinition, &'a crate::Automation),
    AutomationUpdated(&'a ObjectDefinition, &'a crate::Automation),
    AutomationDeleted(&'a ObjectDefinition, &'a crate::Automation),
    /// An administrator retried a failed job (ADR 0022 §6).
    AutomationJobRetried(
        &'a ObjectDefinition,
        &'a crate::Automation,
        &'a crate::AutomationJob,
    ),
    /// An administrator cancelled a pending job.
    AutomationJobCancelled(
        &'a ObjectDefinition,
        &'a crate::Automation,
        &'a crate::AutomationJob,
    ),
    /// A member submitted a bulk job (ADR 0023 §2).
    BulkJobSubmitted(&'a crate::BulkJob),
    /// The worker processed a bulk job's last row.
    BulkJobCompleted(&'a crate::BulkJob),
    /// Its submitter or an administrator aborted a bulk job.
    BulkJobAborted(&'a crate::BulkJob),
    ReportCreated(&'a crate::Report),
    ReportUpdated(&'a crate::Report),
    ReportDeleted(&'a crate::Report),
    QuickActionCreated(&'a ObjectDefinition, &'a crate::QuickAction),
    QuickActionUpdated(&'a ObjectDefinition, &'a crate::QuickAction),
    QuickActionDeleted(&'a ObjectDefinition, &'a crate::QuickAction),
}

impl Change<'_> {
    fn parts(&self) -> (&'static str, &'static str, String, Value) {
        match self {
            Self::ObjectCreated(object) => (
                OBJECT_CREATED_V1,
                "objects.object.created",
                object_target(object),
                object_payload(object),
            ),
            Self::ObjectUpdated(object) => (
                OBJECT_UPDATED_V1,
                "objects.object.updated",
                object_target(object),
                object_payload(object),
            ),
            Self::ObjectDeleted(object) => (
                OBJECT_DELETED_V1,
                "objects.object.deleted",
                object_target(object),
                object_payload(object),
            ),
            Self::ObjectSuspended(object, suspended) => (
                OBJECT_UPDATED_V1,
                if *suspended {
                    "objects.object.suspended"
                } else {
                    "objects.object.resumed"
                },
                object_target(object),
                json!({
                    "object": object.api_name,
                    "version": object.version,
                    "suspended": suspended,
                }),
            ),
            Self::FieldCreated(object, field) => (
                FIELD_CREATED_V1,
                "objects.field.created",
                field_target(object, field),
                field_payload(object, field),
            ),
            Self::FieldUpdated(object, field) => (
                FIELD_UPDATED_V1,
                "objects.field.updated",
                field_target(object, field),
                field_payload(object, field),
            ),
            Self::FieldDeleted(object, field) => (
                FIELD_DELETED_V1,
                "objects.field.deleted",
                field_target(object, field),
                field_payload(object, field),
            ),
            Self::PermissionsChanged(object, role) => (
                OBJECT_PERMISSIONS_CHANGED_V1,
                "objects.permissions.changed",
                object_target(object),
                json!({"object": object.api_name, "role_id": role}),
            ),
            Self::RecordCreated(object, record, fields) => (
                RECORD_CREATED_V1,
                "records.record.created",
                record_target(record),
                record_payload(object, record, fields),
            ),
            Self::RecordUpdated(object, record, fields) => (
                RECORD_UPDATED_V1,
                "records.record.updated",
                record_target(record),
                record_payload(object, record, fields),
            ),
            Self::RecordDeleted(object, record) => (
                RECORD_DELETED_V1,
                "records.record.deleted",
                record_target(record),
                record_payload(object, record, &[]),
            ),
            Self::RecordUndeleted(object, record) => (
                RECORD_UNDELETED_V1,
                "records.record.undeleted",
                record_target(record),
                record_payload(object, record, &[]),
            ),
            Self::OwnerChanged(object, record, previous, removed_shares) => {
                let mut payload = record_payload(object, record, &["owner".to_owned()]);
                if let Some(map) = payload.as_object_mut() {
                    map.insert("owner_id".into(), json!(record.owner_id));
                    map.insert("previous_owner_id".into(), json!(previous));
                    map.insert("removed_shares".into(), json!(removed_shares));
                }
                (
                    RECORD_OWNER_CHANGED_V1,
                    "records.record.owner_changed",
                    record_target(record),
                    payload,
                )
            }
            Self::RuleCreated(object, rule) => (
                VALIDATION_RULE_CREATED_V1,
                "objects.validation_rule.created",
                rule_target(object, rule),
                rule_payload(object, rule),
            ),
            Self::RuleUpdated(object, rule) => (
                VALIDATION_RULE_UPDATED_V1,
                "objects.validation_rule.updated",
                rule_target(object, rule),
                rule_payload(object, rule),
            ),
            Self::RuleDeleted(object, rule) => (
                VALIDATION_RULE_DELETED_V1,
                "objects.validation_rule.deleted",
                rule_target(object, rule),
                rule_payload(object, rule),
            ),
            Self::AutomationCreated(object, automation) => (
                AUTOMATION_CREATED_V1,
                "objects.automation.created",
                automation_target(object, automation),
                automation_payload(object, automation),
            ),
            Self::AutomationUpdated(object, automation) => (
                AUTOMATION_UPDATED_V1,
                "objects.automation.updated",
                automation_target(object, automation),
                automation_payload(object, automation),
            ),
            Self::AutomationDeleted(object, automation) => (
                AUTOMATION_DELETED_V1,
                "objects.automation.deleted",
                automation_target(object, automation),
                automation_payload(object, automation),
            ),
            Self::AutomationJobRetried(object, automation, job) => (
                AUTOMATION_JOB_RETRIED_V1,
                "objects.automation_job.retried",
                automation_target(object, automation),
                job_payload(object, automation, job),
            ),
            Self::AutomationJobCancelled(object, automation, job) => (
                AUTOMATION_JOB_CANCELLED_V1,
                "objects.automation_job.cancelled",
                automation_target(object, automation),
                job_payload(object, automation, job),
            ),
            Self::BulkJobSubmitted(job) => (
                BULK_JOB_SUBMITTED_V1,
                "objects.bulk_job.submitted",
                bulk_target(job),
                bulk_payload(job),
            ),
            Self::BulkJobCompleted(job) => (
                BULK_JOB_COMPLETED_V1,
                "objects.bulk_job.completed",
                bulk_target(job),
                bulk_payload(job),
            ),
            Self::BulkJobAborted(job) => (
                BULK_JOB_ABORTED_V1,
                "objects.bulk_job.aborted",
                bulk_target(job),
                bulk_payload(job),
            ),
            Self::ReportCreated(report) => (
                REPORT_CREATED_V1,
                "reports.report.created",
                report_target(report),
                report_payload(report),
            ),
            Self::ReportUpdated(report) => (
                REPORT_UPDATED_V1,
                "reports.report.updated",
                report_target(report),
                report_payload(report),
            ),
            Self::ReportDeleted(report) => (
                REPORT_DELETED_V1,
                "reports.report.deleted",
                report_target(report),
                report_payload(report),
            ),
            Self::QuickActionCreated(object, action) => (
                QUICK_ACTION_CREATED_V1,
                "objects.quick_action.created",
                quick_action_target(object, action),
                quick_action_payload(object, action),
            ),
            Self::QuickActionUpdated(object, action) => (
                QUICK_ACTION_UPDATED_V1,
                "objects.quick_action.updated",
                quick_action_target(object, action),
                quick_action_payload(object, action),
            ),
            Self::QuickActionDeleted(object, action) => (
                QUICK_ACTION_DELETED_V1,
                "objects.quick_action.deleted",
                quick_action_target(object, action),
                quick_action_payload(object, action),
            ),
            Self::SharingChanged(object, record, kind, grantee, access) => (
                RECORD_SHARING_CHANGED_V1,
                if access.is_some() {
                    "records.record.shared"
                } else {
                    "records.record.unshared"
                },
                record_target(record),
                json!({
                    "object": object.api_name,
                    "record": record.public_id,
                    "grantee_kind": kind.as_str(),
                    "grantee_id": grantee,
                    "access": access.map(crate::ShareAccess::as_str),
                }),
            ),
        }
    }

    /// Whether this change alters definitions a synced device validates and
    /// reads with (ADR 0024 §3): objects, fields, object permissions,
    /// validation rules and automations. It moves the workspace metadata
    /// version.
    pub fn is_metadata(&self) -> bool {
        matches!(
            self,
            Self::ObjectCreated(..)
                | Self::ObjectUpdated(..)
                | Self::ObjectDeleted(..)
                | Self::ObjectSuspended(..)
                | Self::FieldCreated(..)
                | Self::FieldUpdated(..)
                | Self::FieldDeleted(..)
                | Self::PermissionsChanged(..)
                | Self::RuleCreated(..)
                | Self::RuleUpdated(..)
                | Self::RuleDeleted(..)
                | Self::AutomationCreated(..)
                | Self::AutomationUpdated(..)
                | Self::AutomationDeleted(..)
        )
    }

    /// The change-log entry this change leaves (ADR 0018): record data
    /// changes only, with the changed field names. Sharing changes alter
    /// visibility, not data, and leave none.
    pub fn record_change(
        &self,
    ) -> Option<(&ObjectDefinition, &Record, crate::ChangeKind, Vec<String>)> {
        match self {
            Self::RecordCreated(object, record, fields) => {
                Some((object, record, crate::ChangeKind::Created, fields.to_vec()))
            }
            Self::RecordUpdated(object, record, fields) => {
                Some((object, record, crate::ChangeKind::Updated, fields.to_vec()))
            }
            Self::RecordDeleted(object, record) => {
                Some((object, record, crate::ChangeKind::Deleted, Vec::new()))
            }
            Self::RecordUndeleted(object, record) => {
                Some((object, record, crate::ChangeKind::Undeleted, Vec::new()))
            }
            Self::OwnerChanged(object, record, _, _) => Some((
                object,
                record,
                crate::ChangeKind::Updated,
                vec!["owner".to_owned()],
            )),
            _ => None,
        }
    }

    /// The audit entry and outbox event for this change, sharing one event id.
    pub fn evidence(&self, context: EvidenceContext) -> (AuditEvent, EventEnvelope) {
        let (event_type, action, target, mut payload) = self.parts();
        if let (Some(automation_id), Some(map)) = (context.automation_id, payload.as_object_mut()) {
            map.insert("automation_id".into(), json!(automation_id));
        }
        insert_source_kind(&mut payload, context.source);
        let event = EventEnvelope::with_context(
            event_type,
            ENGINE_CONTEXT,
            context.tenant_id,
            Some(context.actor_id),
            context.correlation_id,
            context.causation_id,
            payload.clone(),
        );
        let mut metadata = payload;
        if let Some(object) = metadata.as_object_mut() {
            object.insert("event_id".into(), json!(event.id));
        }
        insert_source(&mut metadata, context.source);
        let mut audit = AuditEvent::with_context(
            Some(context.tenant_id),
            Some(context.actor_id),
            context.correlation_id,
            context.causation_id,
            action,
            target,
            metadata,
        );
        audit.occurred_at = event.occurred_at;
        (audit, event)
    }
}

/// Names the source's kind in an event (#233). Its detail, a device or
/// client id or an automation name, stays in the audit entry, so events
/// carry nothing that points at a person's device.
fn insert_source_kind(payload: &mut Value, source: Option<crate::MutationSource>) {
    if let (Some(source), Some(map)) = (source, payload.as_object_mut()) {
        map.insert("source".into(), json!(source.kind));
    }
}

/// Puts the whole source in audit metadata, as `{"kind", "detail"}`.
fn insert_source(metadata: &mut Value, source: Option<crate::MutationSource>) {
    if let (Some(source), Some(map)) = (source, metadata.as_object_mut()) {
        map.insert("source".into(), json!(source));
    }
}

fn object_target(object: &ObjectDefinition) -> String {
    format!("object:{}", object.api_name)
}

fn field_target(object: &ObjectDefinition, field: &FieldDefinition) -> String {
    format!("field:{}/{}", object.api_name, field.api_name)
}

fn rule_target(object: &ObjectDefinition, rule: &crate::ValidationRule) -> String {
    format!("validation_rule:{}/{}", object.api_name, rule.name)
}

/// Names the rule and whether it is active; conditions and messages are
/// administrator text and stay out of events.
fn rule_payload(object: &ObjectDefinition, rule: &crate::ValidationRule) -> Value {
    json!({
        "object": object.api_name,
        "rule": rule.name,
        "active": rule.active,
        "version": rule.version,
    })
}

fn quick_action_target(object: &ObjectDefinition, action: &crate::QuickAction) -> String {
    format!("quick_action:{}/{}", object.api_name, action.name)
}

/// Names the action, its kind and target, the fields its form asks for and
/// the fields it presets; preset values and the label stay out of events.
fn quick_action_payload(object: &ObjectDefinition, action: &crate::QuickAction) -> Value {
    json!({
        "object": object.api_name,
        "action": action.name,
        "kind": action.kind.as_str(),
        "target_object": action.target_object,
        "target_field": action.target_field,
        "fields": action.fields,
        "preset_fields": action.presets.keys().collect::<Vec<_>>(),
        "version": action.version,
    })
}

fn automation_target(object: &ObjectDefinition, automation: &crate::Automation) -> String {
    format!("automation:{}/{}", object.api_name, automation.name)
}

/// Names the automation, when it runs and whether it is active; formulas
/// are administrator text and stay out of events.
fn automation_payload(object: &ObjectDefinition, automation: &crate::Automation) -> Value {
    json!({
        "object": object.api_name,
        "automation": automation.name,
        "trigger": automation.trigger.as_str(),
        "timing": automation.timing.as_str(),
        "active": automation.active,
        "version": automation.version,
    })
}

fn job_payload(
    object: &ObjectDefinition,
    automation: &crate::Automation,
    job: &crate::AutomationJob,
) -> Value {
    json!({
        "object": object.api_name,
        "automation": automation.name,
        "job_id": job.id,
        "record": job.record,
        "status": job.status.as_str(),
    })
}

fn bulk_target(job: &crate::BulkJob) -> String {
    format!("bulk_job:{}", job.id)
}

/// Names the job and its counts; row values stay out of events.
fn bulk_payload(job: &crate::BulkJob) -> Value {
    json!({
        "object": job.object,
        "job_id": job.id,
        "operation": job.operation.as_str(),
        "status": job.status.as_str(),
        "total": job.total,
        "succeeded": job.succeeded,
        "failed": job.failed,
    })
}

fn report_target(report: &crate::Report) -> String {
    format!("report:{}", report.id)
}

/// Names the report and how it is shown; its query text stays out, as it
/// can name values to filter on.
fn report_payload(report: &crate::Report) -> Value {
    json!({
        "report_id": report.id,
        "chart": report.chart.as_str(),
        "visibility": report.visibility.as_str(),
        "version": report.version,
    })
}

fn record_target(record: &Record) -> String {
    format!("record:{}", record.public_id)
}

fn object_payload(object: &ObjectDefinition) -> Value {
    json!({"object": object.api_name, "version": object.version})
}

fn field_payload(object: &ObjectDefinition, field: &FieldDefinition) -> Value {
    json!({"object": object.api_name, "field": field.api_name, "version": field.version})
}

fn record_payload(object: &ObjectDefinition, record: &Record, fields: &[String]) -> Value {
    json!({
        "object": object.api_name,
        "entity_id": record.public_id,
        "version": record.version,
        "changed_fields": fields,
    })
}

/// The audit entry and outbox event for a workspace-defined platform event
/// (ADR 0022 §7). The event carries `fields`; the audit entry names the
/// event without its values.
pub fn workspace_event(
    context: EvidenceContext,
    object: &ObjectDefinition,
    automation: &crate::Automation,
    name: &str,
    record: &Record,
    fields: serde_json::Map<String, Value>,
) -> (AuditEvent, EventEnvelope) {
    let mut payload = json!({
        "event": name,
        "object": object.api_name,
        "record": record.public_id,
        "automation": automation.name,
        "automation_id": automation.id,
        "fields": fields,
    });
    // Like every engine event, it names where the change came from by
    // kind only; the detail stays in the audit entry (#233).
    insert_source_kind(&mut payload, context.source);
    let event = EventEnvelope::with_context(
        WORKSPACE_EVENT_V1,
        ENGINE_CONTEXT,
        context.tenant_id,
        Some(context.actor_id),
        context.correlation_id,
        context.causation_id,
        payload,
    );
    let mut metadata = json!({
        "event": name,
        "object": object.api_name,
        "record": record.public_id,
        "automation": automation.name,
        "event_id": event.id,
    });
    insert_source(&mut metadata, context.source);
    let mut audit = AuditEvent::with_context(
        Some(context.tenant_id),
        Some(context.actor_id),
        context.correlation_id,
        context.causation_id,
        "objects.workspace_event.published",
        automation_target(object, automation),
        metadata,
    );
    audit.occurred_at = event.occurred_at;
    (audit, event)
}
