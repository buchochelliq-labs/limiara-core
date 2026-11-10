//! The PostgreSQL object store: kernel tables under forced row-level
//! security, written in the engine's own execution context.

use super::{ObjectStore, ObjectTx};
use crate::query::limits;
use crate::query::plan::{AggregatePlan, Cell, CellKind, Param, RecordPlan};
use crate::query::sql::{self, Dialect, Rendered};
use crate::values::name_key;
use crate::{
    AccessFlags, ChangeKind, DefinitionStatus, DeleteBehaviour, FieldDefinition, FieldType,
    GranteeKind, HistoryEntry, HistoryKind, IndexValue, ObjectDefinition, ObjectError, Origin,
    PicklistOption, Record, RecordChange, RecordIndex, RecordQuery, RecordShare, RecordSort,
    RecordStatus, RolePermission, Rollup, RollupFunction, RollupQuery, ShareAccess, SharingModel,
    SortDirection, ValidationRule, ENGINE_CONTEXT,
};
use crate::{RemoteVersion, ReplicaDirectory, ReplicaLease, SyncPositions};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use platform_audit::AuditEvent;
use platform_db::{begin_plugin_tx, DbError};
use platform_events::EventEnvelope;
use serde_json::Value;
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Postgres, QueryBuilder, Row, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresObjectStore {
    pool: PgPool,
}

impl PostgresObjectStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ObjectStore for PostgresObjectStore {
    async fn begin(&self, tenant_id: Uuid) -> Result<Box<dyn ObjectTx>, ObjectError> {
        let tx = begin_plugin_tx(&self.pool, tenant_id, ENGINE_CONTEXT)
            .await
            .map_err(|DbError::Sqlx(error)| ObjectError::Database(error))?;
        Ok(Box::new(PostgresObjectTx { tx, tenant_id }))
    }
}

struct PostgresObjectTx {
    tx: Transaction<'static, Postgres>,
    tenant_id: Uuid,
}

impl PostgresObjectTx {
    /// Replaces a related-to field's allowlist rows with its current list;
    /// other fields keep none.
    async fn write_reference_objects(
        &mut self,
        field: &FieldDefinition,
    ) -> Result<(), ObjectError> {
        if field.field_type != FieldType::RelatedTo {
            return Ok(());
        }
        sqlx::query("DELETE FROM field_reference_objects WHERE tenant_id = $1 AND field_id = $2")
            .bind(self.tenant_id)
            .bind(field.id)
            .execute(&mut *self.tx)
            .await?;
        for (position, object_id) in field.reference_object_ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO field_reference_objects (tenant_id, field_id, object_id, position) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(self.tenant_id)
            .bind(field.id)
            .bind(object_id)
            .bind(i32::try_from(position).map_err(|_| ObjectError::Invariant)?)
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    /// Runs a rendered query under the query statement timeout (ADR 0019
    /// §5), or the session's own timeout when that is shorter. It lasts for
    /// the rest of the transaction, which only reads once a query runs.
    async fn run_query(&mut self, rendered: &Rendered) -> Result<Vec<PgRow>, ObjectError> {
        sqlx::query(
            "SELECT set_config('statement_timeout', \
             LEAST(NULLIF(setting::bigint, 0), $1)::text, true) \
             FROM pg_settings WHERE name = 'statement_timeout'",
        )
        .bind(i64::from(limits::STATEMENT_TIMEOUT_MS))
        .execute(&mut *self.tx)
        .await?;
        let mut query = sqlx::query(&rendered.sql);
        for param in &rendered.params {
            query = match param {
                Param::Text(text) => query.bind(text.clone()),
                Param::Number(number) => query.bind(*number),
                Param::Int(number) => query.bind(*number),
                Param::Time(time) => query.bind(*time),
                Param::Uuid(id) => query.bind(*id),
            };
        }
        query.fetch_all(&mut *self.tx).await.map_err(|error| {
            let cancelled = error
                .as_database_error()
                .and_then(|error| error.code())
                .is_some_and(|code| code == "57014");
            if cancelled {
                ObjectError::Conflict("query.timeout")
            } else {
                ObjectError::Database(error)
            }
        })
    }
}

const OBJECT_COLUMNS: &str =
    "id, api_name, label, plural_label, description, origin, owner_plugin, \
     key_prefix, name_label, sharing_model, searchable, in_navigation, status, version, \
     created_at, created_by, updated_at, updated_by, grant_hierarchy, track_history, suspended";
const FIELD_COLUMNS: &str = "f.id, f.object_id, f.api_name, f.label, f.description, f.field_type, \
     f.origin, f.required, f.unique_value, f.indexed, f.searchable, f.max_length, f.scale, \
     f.options, f.default_value, f.reference_object_id, r.api_name AS reference_api, \
     f.delete_behaviour, f.position, f.status, f.version, f.created_at, f.created_by, \
     f.updated_at, f.updated_by, f.track_history, f.formula, f.formula_type, f.lookup_filter, \
     f.rollup_function, f.rollup_relationship_id, f.rollup_field_id, \
     ro.api_name AS rollup_object, rr.api_name AS rollup_relationship, rf.api_name AS rollup_field, \
     (SELECT coalesce(jsonb_agg(jsonb_build_array(fa.object_id, oa.api_name) ORDER BY fa.position), \
        '[]'::jsonb) FROM field_reference_objects fa \
        JOIN object_definitions oa ON oa.tenant_id = fa.tenant_id AND oa.id = fa.object_id \
        WHERE fa.tenant_id = f.tenant_id AND fa.field_id = f.id) AS reference_objects";
const FIELD_FROM: &str = "FROM field_definitions f LEFT JOIN object_definitions r \
     ON r.tenant_id = f.tenant_id AND r.id = f.reference_object_id \
     LEFT JOIN field_definitions rr ON rr.tenant_id = f.tenant_id AND rr.id = f.rollup_relationship_id \
     LEFT JOIN object_definitions ro ON ro.tenant_id = rr.tenant_id AND ro.id = rr.object_id \
     LEFT JOIN field_definitions rf ON rf.tenant_id = f.tenant_id AND rf.id = f.rollup_field_id";
const RECORD_COLUMNS: &str =
    "r.id, r.object_id, r.public_id, r.name, r.owner_id, r.record_type_id, \
     r.data, r.version, r.created_at, r.created_by, r.updated_at, r.updated_by, r.touched_at, \
     r.archived_at, r.deleted_at";

fn parse<T>(value: Option<T>) -> Result<T, ObjectError> {
    value.ok_or(ObjectError::Invariant)
}

fn object_from_row(row: &PgRow) -> Result<ObjectDefinition, ObjectError> {
    Ok(ObjectDefinition {
        id: row.try_get("id")?,
        api_name: row.try_get("api_name")?,
        label: row.try_get("label")?,
        plural_label: row.try_get("plural_label")?,
        description: row.try_get("description")?,
        origin: parse(Origin::parse(row.try_get("origin")?))?,
        owner_plugin: row.try_get("owner_plugin")?,
        key_prefix: row.try_get("key_prefix")?,
        name_label: row.try_get("name_label")?,
        sharing_model: parse(SharingModel::parse(row.try_get("sharing_model")?))?,
        grant_hierarchy: row.try_get("grant_hierarchy")?,
        track_history: row.try_get("track_history")?,
        searchable: row.try_get("searchable")?,
        in_navigation: row.try_get("in_navigation")?,
        status: parse(DefinitionStatus::parse(row.try_get("status")?))?,
        suspended: row.try_get("suspended")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
        fields: Vec::new(),
    })
}

const REPORT_COLUMNS: &str = "id, name, description, query, chart, label_column, value_column, \
     visibility, owner_id, version, created_at, created_by, updated_at, updated_by";

fn report_from_row(row: &PgRow) -> Result<crate::Report, ObjectError> {
    Ok(crate::Report {
        id: row.try_get("id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        query: row.try_get("query")?,
        chart: parse(crate::ReportChart::parse(row.try_get("chart")?))?,
        label_column: row.try_get("label_column")?,
        value_column: row.try_get("value_column")?,
        visibility: parse(crate::ReportVisibility::parse(row.try_get("visibility")?))?,
        owner_id: row.try_get("owner_id")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

fn field_from_row(row: &PgRow) -> Result<FieldDefinition, ObjectError> {
    let options: Value = row.try_get("options")?;
    let options: Vec<PicklistOption> =
        serde_json::from_value(options).map_err(|_| ObjectError::Invariant)?;
    let delete_behaviour: Option<String> = row.try_get("delete_behaviour")?;
    let (reference_object_ids, reference_objects) =
        super::reference_objects(&row.try_get::<Value, _>("reference_objects")?)?;
    Ok(FieldDefinition {
        id: row.try_get("id")?,
        object_id: row.try_get("object_id")?,
        api_name: row.try_get("api_name")?,
        label: row.try_get("label")?,
        description: row.try_get("description")?,
        field_type: parse(FieldType::parse(row.try_get("field_type")?))?,
        origin: parse(Origin::parse(row.try_get("origin")?))?,
        required: row.try_get("required")?,
        unique: row.try_get("unique_value")?,
        indexed: row.try_get("indexed")?,
        searchable: row.try_get("searchable")?,
        track_history: row.try_get("track_history")?,
        max_length: row.try_get("max_length")?,
        scale: row.try_get("scale")?,
        options,
        default_value: row.try_get("default_value")?,
        reference_object_id: row.try_get("reference_object_id")?,
        reference_object: row.try_get("reference_api")?,
        reference_object_ids,
        reference_objects,
        delete_behaviour: delete_behaviour
            .map(|value| parse(DeleteBehaviour::parse(&value)))
            .transpose()?,
        lookup_filter: row.try_get("lookup_filter")?,
        formula: row.try_get("formula")?,
        formula_type: row
            .try_get::<Option<String>, _>("formula_type")?
            .map(|value| parse(FieldType::parse(&value)))
            .transpose()?,
        rollup: match row.try_get::<Option<String>, _>("rollup_function")? {
            Some(function) => Some(Rollup {
                function: parse(RollupFunction::parse(&function))?,
                relationship_id: parse(row.try_get::<Option<Uuid>, _>("rollup_relationship_id")?)?,
                field_id: row.try_get::<Option<Uuid>, _>("rollup_field_id")?,
                object: parse(row.try_get("rollup_object")?)?,
                relationship: parse(row.try_get("rollup_relationship")?)?,
                field: row.try_get("rollup_field")?,
            }),
            None => None,
        },
        position: row.try_get("position")?,
        status: parse(DefinitionStatus::parse(row.try_get("status")?))?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

const RULE_COLUMNS: &str = "id, object_id, name, description, condition, message, error_field, \
     active, version, created_at, created_by, updated_at, updated_by";

fn rule_from_row(row: &PgRow) -> Result<ValidationRule, ObjectError> {
    Ok(ValidationRule {
        id: row.try_get("id")?,
        object_id: row.try_get("object_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        condition: row.try_get("condition")?,
        message: row.try_get("message")?,
        error_field: row.try_get("error_field")?,
        active: row.try_get("active")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

const QUICK_ACTION_COLUMNS: &str = "q.id, q.object_id, q.name, q.label, q.kind, q.position, \
     q.target_object_id, t.api_name AS target_object, q.target_field, q.fields, q.presets, \
     q.version, q.created_at, q.created_by, q.updated_at, q.updated_by";
const QUICK_ACTION_FROM: &str = "FROM quick_actions q LEFT JOIN object_definitions t \
     ON t.tenant_id = q.tenant_id AND t.id = q.target_object_id";

fn quick_action_from_row(row: &PgRow) -> Result<crate::QuickAction, ObjectError> {
    let fields: Value = row.try_get("fields")?;
    let presets: Value = row.try_get("presets")?;
    Ok(crate::QuickAction {
        id: row.try_get("id")?,
        object_id: row.try_get("object_id")?,
        name: row.try_get("name")?,
        label: row.try_get("label")?,
        kind: crate::QuickActionKind::parse(&row.try_get::<String, _>("kind")?)
            .ok_or(ObjectError::Invariant)?,
        position: row.try_get("position")?,
        target_object_id: row.try_get("target_object_id")?,
        target_object: row.try_get("target_object")?,
        target_field: row.try_get("target_field")?,
        fields: serde_json::from_value(fields).map_err(|_| ObjectError::Invariant)?,
        presets: serde_json::from_value(presets).map_err(|_| ObjectError::Invariant)?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

fn quick_action_json(action: &crate::QuickAction) -> Result<(Value, Value), ObjectError> {
    Ok((
        serde_json::to_value(&action.fields).map_err(|_| ObjectError::Invariant)?,
        serde_json::to_value(&action.presets).map_err(|_| ObjectError::Invariant)?,
    ))
}

const RELATED_ACTION_COLUMNS: &str = "id, object_id, field_id, owner_plugin, name, label, \
     fields, presets, offered_to, created_at, updated_at";

fn related_action_from_row(row: &PgRow) -> Result<crate::RelatedAction, ObjectError> {
    let json = |column: &str| -> Result<Value, ObjectError> { Ok(row.try_get(column)?) };
    Ok(crate::RelatedAction {
        id: row.try_get("id")?,
        object_id: row.try_get("object_id")?,
        field_id: row.try_get("field_id")?,
        owner_plugin: row.try_get("owner_plugin")?,
        name: row.try_get("name")?,
        label: row.try_get("label")?,
        fields: serde_json::from_value(json("fields")?).map_err(|_| ObjectError::Invariant)?,
        presets: serde_json::from_value(json("presets")?).map_err(|_| ObjectError::Invariant)?,
        offered_to: serde_json::from_value(json("offered_to")?)
            .map_err(|_| ObjectError::Invariant)?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

const TIMELINE_COLUMNS: &str = "object_id, owner_plugin, through, date_field_id, open_kind, \
     open_field_id, open_value, created_at, updated_at";

fn timeline_from_row(row: &PgRow) -> Result<crate::TimelineSource, ObjectError> {
    let kind: Option<String> = row.try_get("open_kind")?;
    let open = match kind.as_deref() {
        None => None,
        Some("upcoming") => Some(crate::TimelineOpen::Upcoming),
        Some("field_equals") => Some(crate::TimelineOpen::FieldEquals {
            field_id: row
                .try_get::<Option<Uuid>, _>("open_field_id")?
                .ok_or(ObjectError::Invariant)?,
            value: row
                .try_get::<Option<String>, _>("open_value")?
                .ok_or(ObjectError::Invariant)?,
        }),
        Some(_) => return Err(ObjectError::Invariant),
    };
    Ok(crate::TimelineSource {
        object_id: row.try_get("object_id")?,
        owner_plugin: row.try_get("owner_plugin")?,
        through: serde_json::from_value(row.try_get::<Value, _>("through")?)
            .map_err(|_| ObjectError::Invariant)?,
        date_field_id: row.try_get("date_field_id")?,
        open,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// A timeline source's open rule as its three columns.
fn timeline_open_columns(
    open: Option<&crate::TimelineOpen>,
) -> (Option<&'static str>, Option<Uuid>, Option<String>) {
    match open {
        None => (None, None, None),
        Some(crate::TimelineOpen::Upcoming) => (Some("upcoming"), None, None),
        Some(crate::TimelineOpen::FieldEquals { field_id, value }) => {
            (Some("field_equals"), Some(*field_id), Some(value.clone()))
        }
    }
}

const AUTOMATION_COLUMNS: &str = "id, object_id, name, description, trigger, timing, condition, \
     actions, schedule, active, version, created_at, created_by, updated_at, updated_by";

fn automation_from_row(row: &PgRow) -> Result<crate::Automation, ObjectError> {
    let actions: Value = row.try_get("actions")?;
    Ok(crate::Automation {
        id: row.try_get("id")?,
        object_id: row.try_get("object_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        trigger: crate::AutomationTrigger::parse(&row.try_get::<String, _>("trigger")?)
            .ok_or(ObjectError::Invariant)?,
        timing: crate::AutomationTiming::parse(&row.try_get::<String, _>("timing")?)
            .ok_or(ObjectError::Invariant)?,
        condition: row.try_get("condition")?,
        actions: serde_json::from_value(actions).map_err(|_| ObjectError::Invariant)?,
        schedule: row
            .try_get::<Option<Value>, _>("schedule")?
            .map(serde_json::from_value)
            .transpose()
            .map_err(|_| ObjectError::Invariant)?,
        active: row.try_get("active")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
    })
}

const RUN_COLUMNS: &str = "id, automation_id, object_id, record, event, outcome, detail, saves, \
     ran_at, ran_by, correlation_id";

fn run_from_row(row: &PgRow) -> Result<crate::AutomationRun, ObjectError> {
    Ok(crate::AutomationRun {
        id: row.try_get("id")?,
        automation_id: row.try_get("automation_id")?,
        object_id: row.try_get("object_id")?,
        record: row.try_get("record")?,
        event: crate::SaveEvent::parse(&row.try_get::<String, _>("event")?)
            .ok_or(ObjectError::Invariant)?,
        outcome: crate::RunOutcome::parse(&row.try_get::<String, _>("outcome")?)
            .ok_or(ObjectError::Invariant)?,
        detail: row.try_get("detail")?,
        saves: row.try_get("saves")?,
        ran_at: row.try_get("ran_at")?,
        ran_by: row.try_get("ran_by")?,
        correlation_id: row.try_get("correlation_id")?,
    })
}

fn schedule_json(automation: &crate::Automation) -> Result<Option<Value>, ObjectError> {
    automation
        .schedule
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|_| ObjectError::Invariant)
}

const JOB_COLUMNS: &str = "id, automation_id, object_id, record_id, record, event, scheduled, due_at, \
     status, attempts, locked_until, last_error, depth, actor_id, correlation_id, created_at, updated_at";

fn job_from_row(row: &PgRow) -> Result<crate::AutomationJob, ObjectError> {
    Ok(crate::AutomationJob {
        id: row.try_get("id")?,
        automation_id: row.try_get("automation_id")?,
        object_id: row.try_get("object_id")?,
        record_id: row.try_get("record_id")?,
        record: row.try_get("record")?,
        event: crate::SaveEvent::parse(&row.try_get::<String, _>("event")?)
            .ok_or(ObjectError::Invariant)?,
        scheduled: row.try_get("scheduled")?,
        due_at: row.try_get("due_at")?,
        status: crate::JobStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or(ObjectError::Invariant)?,
        attempts: row.try_get("attempts")?,
        locked_until: row.try_get("locked_until")?,
        last_error: row.try_get("last_error")?,
        depth: row.try_get("depth")?,
        actor_id: row.try_get("actor_id")?,
        correlation_id: row.try_get("correlation_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

const BULK_COLUMNS: &str = "id, object, operation, status, total, succeeded, failed, actor_id, \
     correlation_id, locked_until, created_at, updated_at, completed_at";

fn bulk_from_row(row: &PgRow) -> Result<crate::BulkJob, ObjectError> {
    Ok(crate::BulkJob {
        id: row.try_get("id")?,
        object: row.try_get("object")?,
        operation: crate::BulkOperation::parse(&row.try_get::<String, _>("operation")?)
            .ok_or(ObjectError::Invariant)?,
        status: crate::BulkStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or(ObjectError::Invariant)?,
        total: row.try_get("total")?,
        succeeded: row.try_get("succeeded")?,
        failed: row.try_get("failed")?,
        actor_id: row.try_get("actor_id")?,
        correlation_id: row.try_get("correlation_id")?,
        locked_until: row.try_get("locked_until")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        completed_at: row.try_get("completed_at")?,
    })
}

fn bulk_row_from_row(row: &PgRow) -> Result<crate::BulkRow, ObjectError> {
    Ok(crate::BulkRow {
        job_id: row.try_get("job_id")?,
        index: row.try_get("row_index")?,
        input: row.try_get("input")?,
        status: crate::BulkRowStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or(ObjectError::Invariant)?,
        record: row.try_get("record")?,
        errors: row.try_get("errors")?,
    })
}

fn actions_json(automation: &crate::Automation) -> Result<Value, ObjectError> {
    serde_json::to_value(&automation.actions).map_err(|_| ObjectError::Invariant)
}

fn record_from_row(row: &PgRow) -> Result<Record, ObjectError> {
    let data: Value = row.try_get("data")?;
    let values: BTreeMap<String, Value> =
        serde_json::from_value(data).map_err(|_| ObjectError::Invariant)?;
    Ok(Record {
        id: row.try_get("id")?,
        object_id: row.try_get("object_id")?,
        public_id: row.try_get("public_id")?,
        name: row.try_get("name")?,
        owner_id: row.try_get("owner_id")?,
        record_type_id: row.try_get("record_type_id")?,
        values,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        created_by: row.try_get("created_by")?,
        updated_at: row.try_get("updated_at")?,
        updated_by: row.try_get("updated_by")?,
        touched_at: row.try_get("touched_at")?,
        archived_at: row.try_get("archived_at")?,
        deleted_at: row.try_get("deleted_at")?,
    })
}

fn json<T: serde::Serialize>(value: &T) -> Result<Value, ObjectError> {
    serde_json::to_value(value).map_err(|_| ObjectError::Invariant)
}

fn conflict_on_unique(error: sqlx::Error, code: &'static str) -> ObjectError {
    if ObjectError::is_unique_violation(&error) {
        ObjectError::Conflict(code)
    } else {
        ObjectError::Database(error)
    }
}

impl PostgresObjectTx {
    async fn fields_of(&mut self, object_id: Uuid) -> Result<Vec<FieldDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {FIELD_COLUMNS} {FIELD_FROM} \
             WHERE f.tenant_id = $1 AND f.object_id = $2 AND f.status = 'active' \
             ORDER BY f.position, f.created_at, f.id"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(field_from_row).collect()
    }

    async fn object_where(
        &mut self,
        condition: &str,
        value: impl for<'q> sqlx::Encode<'q, Postgres> + sqlx::Type<Postgres> + Send,
    ) -> Result<Option<ObjectDefinition>, ObjectError> {
        let row = sqlx::query(&format!(
            "SELECT {OBJECT_COLUMNS} FROM object_definitions WHERE tenant_id = $1 AND {condition}"
        ))
        .bind(self.tenant_id)
        .bind(value)
        .fetch_optional(&mut *self.tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let mut object = object_from_row(&row)?;
        object.fields = self.fields_of(object.id).await?;
        Ok(Some(object))
    }

    async fn record_where(
        &mut self,
        condition: &str,
        binds: (Option<Uuid>, Option<&str>),
    ) -> Result<Option<Record>, ObjectError> {
        let sql = format!(
            "SELECT {RECORD_COLUMNS} FROM records r WHERE r.tenant_id = $1 AND {condition}"
        );
        let mut query = sqlx::query(&sql).bind(self.tenant_id);
        if let Some(id) = binds.0 {
            query = query.bind(id);
        }
        if let Some(text) = binds.1 {
            query = query.bind(text);
        }
        let row = query.fetch_optional(&mut *self.tx).await?;
        row.as_ref().map(record_from_row).transpose()
    }
}

impl PostgresObjectTx {
    /// Inserts a job, or writes a claimed or finished job's state.
    async fn write_job(
        &mut self,
        job: &crate::AutomationJob,
        from: Option<crate::JobStatus>,
    ) -> Result<bool, ObjectError> {
        let result = if let Some(from) = from {
            self.update_job_from(job, from).await?
        } else {
            sqlx::query(
                "INSERT INTO automation_jobs (tenant_id, id, automation_id, object_id, record_id, record, \
                 event, scheduled, due_at, status, attempts, locked_until, last_error, depth, actor_id, \
                 correlation_id, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, \
                 $11, $12, $13, $14, $15, $16, $17, $18)",
            )
            .bind(self.tenant_id)
            .bind(job.id)
            .bind(job.automation_id)
            .bind(job.object_id)
            .bind(job.record_id)
            .bind(&job.record)
            .bind(job.event.as_str())
            .bind(job.scheduled)
            .bind(job.due_at)
            .bind(job.status.as_str())
            .bind(job.attempts)
            .bind(job.locked_until)
            .bind(&job.last_error)
            .bind(job.depth)
            .bind(job.actor_id)
            .bind(job.correlation_id)
            .bind(job.created_at)
            .bind(job.updated_at)
            .execute(&mut *self.tx)
            .await?
        };
        Ok(result.rows_affected() == 1)
    }

    async fn update_job_from(
        &mut self,
        job: &crate::AutomationJob,
        from: crate::JobStatus,
    ) -> Result<sqlx::postgres::PgQueryResult, ObjectError> {
        Ok(sqlx::query(
            "UPDATE automation_jobs SET due_at = $1, status = $2, attempts = $3, locked_until = $4, \
             last_error = $5, updated_at = $6 WHERE tenant_id = $7 AND id = $8 AND status = $9 \
             AND (status <> 'running' OR attempts = $10)",
        )
        .bind(job.due_at)
        .bind(job.status.as_str())
        .bind(job.attempts)
        .bind(job.locked_until)
        .bind(&job.last_error)
        .bind(job.updated_at)
        .bind(self.tenant_id)
        .bind(job.id)
        .bind(from.as_str())
        .bind(job.attempts)
        .execute(&mut *self.tx)
        .await?)
    }
}

#[async_trait]
impl ObjectTx for PostgresObjectTx {
    async fn lock_metadata(&mut self) -> Result<(), ObjectError> {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("{ENGINE_CONTEXT}:metadata:{}", self.tenant_id))
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    async fn list_objects(&mut self) -> Result<Vec<ObjectDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {OBJECT_COLUMNS} FROM object_definitions \
             WHERE tenant_id = $1 AND status = 'active' AND NOT suspended ORDER BY label, api_name"
        ))
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(object_from_row).collect()
    }

    async fn object_by_api_name(
        &mut self,
        api_name: &str,
    ) -> Result<Option<ObjectDefinition>, ObjectError> {
        self.object_where("api_name = $2", api_name.to_owned())
            .await
    }

    async fn object_by_id(&mut self, id: Uuid) -> Result<Option<ObjectDefinition>, ObjectError> {
        self.object_where("id = $2", id).await
    }

    async fn key_prefixes(&mut self) -> Result<Vec<String>, ObjectError> {
        Ok(
            sqlx::query_scalar("SELECT key_prefix FROM object_definitions WHERE tenant_id = $1")
                .bind(self.tenant_id)
                .fetch_all(&mut *self.tx)
                .await?,
        )
    }

    async fn objects_owned_by(
        &mut self,
        plugin_id: &str,
    ) -> Result<Vec<ObjectDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {OBJECT_COLUMNS} FROM object_definitions \
             WHERE tenant_id = $1 AND owner_plugin = $2 AND status = 'active' ORDER BY api_name"
        ))
        .bind(self.tenant_id)
        .bind(plugin_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(object_from_row).collect()
    }

    async fn fields_referencing_object(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<FieldDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {FIELD_COLUMNS} {FIELD_FROM} \
             WHERE f.tenant_id = $1 AND f.status = 'active' AND (f.reference_object_id = $2 \
               OR EXISTS (SELECT 1 FROM field_reference_objects fo WHERE fo.tenant_id = f.tenant_id \
                 AND fo.field_id = f.id AND fo.object_id = $2))"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(field_from_row).collect()
    }

    async fn related_to_fields_holding(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<FieldDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {FIELD_COLUMNS} {FIELD_FROM} \
             WHERE f.tenant_id = $1 AND f.status = 'active' AND f.field_type = 'related_to' \
               AND EXISTS (SELECT 1 FROM record_references rr JOIN records t \
                 ON t.tenant_id = rr.tenant_id AND t.id = rr.target_id \
                 WHERE rr.tenant_id = f.tenant_id AND rr.field_id = f.id AND t.object_id = $2)"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(field_from_row).collect()
    }

    async fn insert_object(&mut self, object: &ObjectDefinition) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO object_definitions (tenant_id, id, api_name, label, plural_label, description, \
               origin, owner_plugin, key_prefix, name_label, sharing_model, searchable, in_navigation, \
               status, version, created_at, created_by, updated_at, updated_by, grant_hierarchy, track_history, \
               suspended) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21, $22)",
        )
        .bind(self.tenant_id)
        .bind(object.id)
        .bind(&object.api_name)
        .bind(&object.label)
        .bind(&object.plural_label)
        .bind(&object.description)
        .bind(object.origin.as_str())
        .bind(&object.owner_plugin)
        .bind(&object.key_prefix)
        .bind(&object.name_label)
        .bind(object.sharing_model.as_str())
        .bind(object.searchable)
        .bind(object.in_navigation)
        .bind(object.status.as_str())
        .bind(object.version)
        .bind(object.created_at)
        .bind(object.created_by)
        .bind(object.updated_at)
        .bind(object.updated_by)
        .bind(object.grant_hierarchy)
        .bind(object.track_history)
        .bind(object.suspended)
        .execute(&mut *self.tx)
        .await
        .map_err(|error| conflict_on_unique(error, "objects.duplicate_api_name"))?;
        Ok(())
    }

    async fn update_object(
        &mut self,
        object: &ObjectDefinition,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let updated = sqlx::query(
            "UPDATE object_definitions SET label = $4, plural_label = $5, description = $6, \
               name_label = $7, sharing_model = $8, searchable = $9, in_navigation = $10, \
               status = $11, version = $12, updated_at = $13, updated_by = $14, \
               grant_hierarchy = $15, track_history = $16, suspended = $17, \
               deleted_at = CASE WHEN $11 = 'deleted' THEN coalesce(deleted_at, $13) END \
             WHERE tenant_id = $1 AND id = $2 AND version = $3",
        )
        .bind(self.tenant_id)
        .bind(object.id)
        .bind(expected_version)
        .bind(&object.label)
        .bind(&object.plural_label)
        .bind(&object.description)
        .bind(&object.name_label)
        .bind(object.sharing_model.as_str())
        .bind(object.searchable)
        .bind(object.in_navigation)
        .bind(object.status.as_str())
        .bind(object.version)
        .bind(object.updated_at)
        .bind(object.updated_by)
        .bind(object.grant_hierarchy)
        .bind(object.track_history)
        .bind(object.suspended)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    async fn insert_field(&mut self, field: &FieldDefinition) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO field_definitions (tenant_id, id, object_id, api_name, label, description, \
               field_type, origin, required, unique_value, indexed, searchable, max_length, scale, \
               options, default_value, reference_object_id, delete_behaviour, position, status, \
               version, created_at, created_by, updated_at, updated_by, track_history, formula, formula_type, \
               rollup_function, rollup_relationship_id, rollup_field_id, lookup_filter) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, \
               $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32)",
        )
        .bind(self.tenant_id)
        .bind(field.id)
        .bind(field.object_id)
        .bind(&field.api_name)
        .bind(&field.label)
        .bind(&field.description)
        .bind(field.field_type.as_str())
        .bind(field.origin.as_str())
        .bind(field.required)
        .bind(field.unique)
        .bind(field.indexed)
        .bind(field.searchable)
        .bind(field.max_length)
        .bind(field.scale)
        .bind(json(&field.options)?)
        .bind(&field.default_value)
        .bind(field.reference_object_id)
        .bind(field.delete_behaviour.map(DeleteBehaviour::as_str))
        .bind(field.position)
        .bind(field.status.as_str())
        .bind(field.version)
        .bind(field.created_at)
        .bind(field.created_by)
        .bind(field.updated_at)
        .bind(field.updated_by)
        .bind(field.track_history)
        .bind(&field.formula)
        .bind(field.formula_type.map(FieldType::as_str))
        .bind(field.rollup.as_ref().map(|rollup| rollup.function.as_str()))
        .bind(field.rollup.as_ref().map(|rollup| rollup.relationship_id))
        .bind(field.rollup.as_ref().and_then(|rollup| rollup.field_id))
        .bind(&field.lookup_filter)
        .execute(&mut *self.tx)
        .await
        .map_err(|error| conflict_on_unique(error, "objects.duplicate_api_name"))?;
        self.write_reference_objects(field).await
    }

    async fn update_field(
        &mut self,
        field: &FieldDefinition,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let updated = sqlx::query(
            "UPDATE field_definitions SET label = $4, description = $5, required = $6, indexed = $7, \
               searchable = $8, max_length = $9, options = $10, default_value = $11, position = $12, \
               status = $13, version = $14, updated_at = $15, updated_by = $16, \
               track_history = $17, formula = $18, lookup_filter = $19, \
               deleted_at = CASE WHEN $13 = 'deleted' THEN coalesce(deleted_at, $15) END \
             WHERE tenant_id = $1 AND id = $2 AND version = $3",
        )
        .bind(self.tenant_id)
        .bind(field.id)
        .bind(expected_version)
        .bind(&field.label)
        .bind(&field.description)
        .bind(field.required)
        .bind(field.indexed)
        .bind(field.searchable)
        .bind(field.max_length)
        .bind(json(&field.options)?)
        .bind(&field.default_value)
        .bind(field.position)
        .bind(field.status.as_str())
        .bind(field.version)
        .bind(field.updated_at)
        .bind(field.updated_by)
        .bind(field.track_history)
        .bind(&field.formula)
        .bind(&field.lookup_filter)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 1 {
            self.write_reference_objects(field).await?;
        }
        Ok(updated == 1)
    }

    async fn drop_field_index(&mut self, field_id: Uuid) -> Result<(), ObjectError> {
        for table in ["record_index", "record_unique", "record_references"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE tenant_id = $1 AND field_id = $2"
            ))
            .bind(self.tenant_id)
            .bind(field_id)
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn grants(
        &mut self,
        role_ids: &[Uuid],
    ) -> Result<BTreeMap<Uuid, AccessFlags>, ObjectError> {
        if role_ids.is_empty() {
            return Ok(BTreeMap::new());
        }
        let rows = sqlx::query(
            "SELECT object_id, bool_or(can_read) AS can_read, bool_or(can_create) AS can_create, \
               bool_or(can_edit) AS can_edit, bool_or(can_delete) AS can_delete, \
               bool_or(view_all) AS view_all, bool_or(modify_all) AS modify_all \
             FROM object_permissions WHERE tenant_id = $1 AND role_id = ANY($2) GROUP BY object_id",
        )
        .bind(self.tenant_id)
        .bind(role_ids)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok((
                    row.try_get("object_id")?,
                    AccessFlags {
                        read: row.try_get("can_read")?,
                        create: row.try_get("can_create")?,
                        edit: row.try_get("can_edit")?,
                        delete: row.try_get("can_delete")?,
                        view_all: row.try_get("view_all")?,
                        modify_all: row.try_get("modify_all")?,
                    },
                ))
            })
            .collect()
    }

    async fn object_permissions(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<RolePermission>, ObjectError> {
        let rows = sqlx::query(
            "SELECT role_id, can_read, can_create, can_edit, can_delete, view_all, modify_all \
             FROM object_permissions WHERE tenant_id = $1 AND object_id = $2 ORDER BY role_id",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(RolePermission {
                    role_id: row.try_get("role_id")?,
                    access: AccessFlags {
                        read: row.try_get("can_read")?,
                        create: row.try_get("can_create")?,
                        edit: row.try_get("can_edit")?,
                        delete: row.try_get("can_delete")?,
                        view_all: row.try_get("view_all")?,
                        modify_all: row.try_get("modify_all")?,
                    },
                })
            })
            .collect()
    }

    async fn set_object_permission(
        &mut self,
        object_id: Uuid,
        permission: &RolePermission,
        actor: Uuid,
    ) -> Result<(), ObjectError> {
        let access = permission.access;
        if access == AccessFlags::default() {
            sqlx::query(
                "DELETE FROM object_permissions WHERE tenant_id = $1 AND object_id = $2 AND role_id = $3",
            )
            .bind(self.tenant_id)
            .bind(object_id)
            .bind(permission.role_id)
            .execute(&mut *self.tx)
            .await?;
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO object_permissions (tenant_id, object_id, role_id, can_read, can_create, \
               can_edit, can_delete, view_all, modify_all, updated_at, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), $10) \
             ON CONFLICT (tenant_id, object_id, role_id) DO UPDATE SET \
               can_read = EXCLUDED.can_read, can_create = EXCLUDED.can_create, \
               can_edit = EXCLUDED.can_edit, can_delete = EXCLUDED.can_delete, \
               view_all = EXCLUDED.view_all, modify_all = EXCLUDED.modify_all, \
               updated_at = EXCLUDED.updated_at, updated_by = EXCLUDED.updated_by",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .bind(permission.role_id)
        .bind(access.read)
        .bind(access.create)
        .bind(access.edit)
        .bind(access.delete)
        .bind(access.view_all)
        .bind(access.modify_all)
        .bind(actor)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn next_record_sequence(&mut self, object_id: Uuid) -> Result<u64, ObjectError> {
        let sequence: i64 = sqlx::query_scalar(
            "UPDATE object_definitions SET record_sequence = record_sequence + 1 \
             WHERE tenant_id = $1 AND id = $2 RETURNING record_sequence",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_one(&mut *self.tx)
        .await?;
        u64::try_from(sequence).map_err(|_| ObjectError::Invariant)
    }

    async fn insert_record(&mut self, record: &Record) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO records (tenant_id, id, object_id, public_id, name, name_key, owner_id, \
               record_type_id, data, version, created_at, created_by, updated_at, updated_by, \
               touched_at, archived_at, deleted_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
        )
        .bind(self.tenant_id)
        .bind(record.id)
        .bind(record.object_id)
        .bind(&record.public_id)
        .bind(&record.name)
        .bind(name_key(&record.name))
        .bind(record.owner_id)
        .bind(record.record_type_id)
        .bind(json(&record.values)?)
        .bind(record.version)
        .bind(record.created_at)
        .bind(record.created_by)
        .bind(record.updated_at)
        .bind(record.updated_by)
        .bind(record.touched_at)
        .bind(record.archived_at)
        .bind(record.deleted_at)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn update_record(
        &mut self,
        record: &Record,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let updated = sqlx::query(
            "UPDATE records SET name = $4, name_key = $5, owner_id = $6, record_type_id = $7, \
               data = $8, version = $9, updated_at = $10, updated_by = $11, touched_at = $12, \
               archived_at = $13, deleted_at = $14 \
             WHERE tenant_id = $1 AND id = $2 AND version = $3",
        )
        .bind(self.tenant_id)
        .bind(record.id)
        .bind(expected_version)
        .bind(&record.name)
        .bind(name_key(&record.name))
        .bind(record.owner_id)
        .bind(record.record_type_id)
        .bind(json(&record.values)?)
        .bind(record.version)
        .bind(record.updated_at)
        .bind(record.updated_by)
        .bind(record.touched_at)
        .bind(record.archived_at)
        .bind(record.deleted_at)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    async fn record(
        &mut self,
        object_id: Uuid,
        public_id: &str,
    ) -> Result<Option<Record>, ObjectError> {
        self.record_where(
            "r.object_id = $2 AND r.public_id = $3",
            (Some(object_id), Some(public_id)),
        )
        .await
    }

    async fn record_by_public_id(
        &mut self,
        public_id: &str,
    ) -> Result<Option<Record>, ObjectError> {
        self.record_where("r.public_id = $2", (None, Some(public_id)))
            .await
    }

    async fn record_by_id(&mut self, id: Uuid) -> Result<Option<Record>, ObjectError> {
        self.record_where("r.id = $2", (Some(id), None)).await
    }

    async fn replace_index(
        &mut self,
        record_id: Uuid,
        index: &RecordIndex,
    ) -> Result<(), ObjectError> {
        for table in [
            "record_index",
            "record_unique",
            "record_references",
            "record_search_terms",
        ] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE tenant_id = $1 AND record_id = $2"
            ))
            .bind(self.tenant_id)
            .bind(record_id)
            .execute(&mut *self.tx)
            .await?;
        }
        if !index.search.is_empty() {
            sqlx::query(
                "INSERT INTO record_search_terms (tenant_id, term, record_id, weight) \
                 SELECT $1, term, $2, weight FROM UNNEST($3::text[], $4::smallint[]) AS t(term, weight)",
            )
            .bind(self.tenant_id)
            .bind(record_id)
            .bind(index.search.iter().map(|term| term.term.clone()).collect::<Vec<_>>())
            .bind(index.search.iter().map(|term| term.weight).collect::<Vec<_>>())
            .execute(&mut *self.tx)
            .await?;
        }
        for entry in &index.entries {
            let (text, number, time) = match &entry.value {
                IndexValue::Text(text) => (Some(text.as_str()), None, None),
                IndexValue::Number(number) => (None, Some(*number), None),
                IndexValue::Time(time) => (None, None, Some(*time)),
            };
            sqlx::query(
                "INSERT INTO record_index (tenant_id, field_id, record_id, text_value, number_value, time_value) \
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(self.tenant_id)
            .bind(entry.field_id)
            .bind(record_id)
            .bind(text)
            .bind(number)
            .bind(time)
            .execute(&mut *self.tx)
            .await?;
        }
        for unique in &index.uniques {
            // DO NOTHING keeps the transaction usable; no row inserted means
            // another record already holds the value.
            let inserted = sqlx::query(
                "INSERT INTO record_unique (tenant_id, field_id, value_key, record_id) \
                 VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
            )
            .bind(self.tenant_id)
            .bind(unique.field_id)
            .bind(&unique.value_key)
            .bind(record_id)
            .execute(&mut *self.tx)
            .await?
            .rows_affected();
            if inserted == 0 {
                return Err(ObjectError::Conflict("records.duplicate_value"));
            }
        }
        for reference in &index.references {
            sqlx::query(
                "INSERT INTO record_references (tenant_id, field_id, record_id, target_id) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(self.tenant_id)
            .bind(reference.field_id)
            .bind(record_id)
            .bind(reference.target_id)
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn referencing(&mut self, target_id: Uuid) -> Result<Vec<(Uuid, Uuid)>, ObjectError> {
        Ok(sqlx::query_as(
            "SELECT rr.field_id, rr.record_id FROM record_references rr \
             JOIN records r ON r.tenant_id = rr.tenant_id AND r.id = rr.record_id \
             WHERE rr.tenant_id = $1 AND rr.target_id = $2 AND r.deleted_at IS NULL \
             ORDER BY rr.record_id",
        )
        .bind(self.tenant_id)
        .bind(target_id)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn referencing_object_records(
        &mut self,
        field_id: Uuid,
        object_id: Uuid,
    ) -> Result<Vec<Uuid>, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT rr.record_id FROM record_references rr \
             JOIN records r ON r.tenant_id = rr.tenant_id AND r.id = rr.record_id \
             JOIN records t ON t.tenant_id = rr.tenant_id AND t.id = rr.target_id \
             WHERE rr.tenant_id = $1 AND rr.field_id = $2 AND t.object_id = $3 \
               AND r.deleted_at IS NULL \
             ORDER BY rr.record_id",
        )
        .bind(self.tenant_id)
        .bind(field_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn list_records(
        &mut self,
        object_id: Uuid,
        query: &RecordQuery,
    ) -> Result<Vec<Record>, ObjectError> {
        let mut sql: QueryBuilder<Postgres> = QueryBuilder::new(format!(
            "SELECT {RECORD_COLUMNS} FROM records r WHERE r.tenant_id = "
        ));
        sql.push_bind(self.tenant_id)
            .push(" AND r.object_id = ")
            .push_bind(object_id);
        sql.push(match query.status {
            RecordStatus::Active => " AND r.deleted_at IS NULL",
            RecordStatus::Deleted => " AND r.deleted_at IS NOT NULL",
            RecordStatus::Any => "",
        });
        if let Some(owner) = query.owner {
            sql.push(" AND r.owner_id = ").push_bind(owner);
        }
        if let Some(after) = query.touched_after {
            sql.push(" AND r.touched_at > ").push_bind(after);
        }
        if let Some(visibility) = &query.visibility {
            // A detail is visible when its master is (ADR 0025).
            let target = if let Some(field_id) = visibility.master {
                sql.push(
                    " AND EXISTS (SELECT 1 FROM record_references mr JOIN records m \
                     ON m.tenant_id = mr.tenant_id AND m.id = mr.target_id \
                     WHERE mr.tenant_id = r.tenant_id AND mr.record_id = r.id AND mr.field_id = ",
                )
                .push_bind(field_id);
                "m"
            } else {
                "r"
            };
            sql.push(format!(" AND ({target}.owner_id = ANY("))
                .push_bind(visibility.owners.clone())
                .push(format!(
                    ") OR EXISTS (SELECT 1 FROM record_shares s WHERE s.tenant_id = {target}.tenant_id \
                       AND s.record_id = {target}.id AND ((s.grantee_kind = 'user' AND s.grantee_id = "
                ))
                .push_bind(visibility.user_id)
                .push(") OR (s.grantee_kind = 'role' AND s.grantee_id = ANY(")
                .push_bind(visibility.role_ids.clone())
                .push(")))))");
            if visibility.master.is_some() {
                sql.push(")");
            }
        }
        if let Some(term) = &query.name_contains {
            sql.push(" AND strpos(r.name_key, ")
                .push_bind(term.clone())
                .push(") > 0");
        }
        for filter in &query.filters {
            sql.push(
                " AND EXISTS (SELECT 1 FROM record_index i WHERE i.tenant_id = r.tenant_id \
                 AND i.record_id = r.id AND i.field_id = ",
            )
            .push_bind(filter.field_id);
            match &filter.value {
                IndexValue::Text(text) => sql.push(" AND i.text_value = ").push_bind(text.clone()),
                IndexValue::Number(number) => sql.push(" AND i.number_value = ").push_bind(*number),
                IndexValue::Time(time) => sql.push(" AND i.time_value = ").push_bind(*time),
            };
            sql.push(")");
        }
        let column = match query.sort {
            RecordSort::Name => "r.name_key",
            RecordSort::CreatedAt => "r.created_at",
            RecordSort::UpdatedAt => "r.updated_at",
            RecordSort::TouchedAt => "r.touched_at",
        };
        let (comparison, direction) = match query.direction {
            SortDirection::Asc => (">", "ASC"),
            SortDirection::Desc => ("<", "DESC"),
        };
        if let Some(cursor) = &query.after {
            sql.push(format!(" AND ({column}, r.id) {comparison} ("));
            match query.sort {
                RecordSort::Name => {
                    sql.push_bind(cursor.sort_value.clone());
                }
                RecordSort::CreatedAt | RecordSort::UpdatedAt | RecordSort::TouchedAt => {
                    let time = DateTime::parse_from_rfc3339(&cursor.sort_value)
                        .map(|time| time.with_timezone(&Utc))
                        .map_err(|_| {
                            ObjectError::invalid(
                                "cursor",
                                "records.cursor_invalid",
                                "start the list again",
                            )
                        })?;
                    sql.push_bind(time);
                }
            }
            sql.push(", ").push_bind(cursor.id).push(")");
        }
        sql.push(format!(
            " ORDER BY {column} {direction}, r.id {direction} LIMIT "
        ))
        .push_bind(i64::from(query.limit) + 1);
        let rows = sql.build().fetch_all(&mut *self.tx).await?;
        rows.iter().map(record_from_row).collect()
    }

    async fn record_shares(&mut self, record_id: Uuid) -> Result<Vec<RecordShare>, ObjectError> {
        let rows: Vec<(String, Uuid, String, DateTime<Utc>, Option<Uuid>)> = sqlx::query_as(
            "SELECT grantee_kind, grantee_id, access, created_at, created_by FROM record_shares \
             WHERE tenant_id = $1 AND record_id = $2 ORDER BY grantee_kind, created_at, grantee_id",
        )
        .bind(self.tenant_id)
        .bind(record_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.into_iter()
            .map(|(kind, grantee_id, access, created_at, created_by)| {
                Ok(RecordShare {
                    grantee_kind: parse(GranteeKind::parse(&kind))?,
                    grantee_id,
                    access: parse(ShareAccess::parse(&access))?,
                    created_at,
                    created_by,
                })
            })
            .collect()
    }

    async fn share_access(
        &mut self,
        record_id: Uuid,
        user_id: Uuid,
        role_ids: &[Uuid],
    ) -> Result<Option<ShareAccess>, ObjectError> {
        let access: Vec<String> = sqlx::query_scalar(
            "SELECT access FROM record_shares WHERE tenant_id = $1 AND record_id = $2 \
             AND ((grantee_kind = 'user' AND grantee_id = $3) \
                  OR (grantee_kind = 'role' AND grantee_id = ANY($4)))",
        )
        .bind(self.tenant_id)
        .bind(record_id)
        .bind(user_id)
        .bind(role_ids.to_vec())
        .fetch_all(&mut *self.tx)
        .await?;
        Ok(access
            .iter()
            .filter_map(|value| ShareAccess::parse(value))
            .max())
    }

    async fn put_share(&mut self, record_id: Uuid, share: &RecordShare) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO record_shares (tenant_id, record_id, grantee_kind, grantee_id, access, created_at, created_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7) \
             ON CONFLICT (tenant_id, record_id, grantee_kind, grantee_id) DO UPDATE SET access = EXCLUDED.access",
        )
        .bind(self.tenant_id)
        .bind(record_id)
        .bind(share.grantee_kind.as_str())
        .bind(share.grantee_id)
        .bind(share.access.as_str())
        .bind(share.created_at)
        .bind(share.created_by)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn delete_share(
        &mut self,
        record_id: Uuid,
        grantee_kind: GranteeKind,
        grantee_id: Uuid,
    ) -> Result<bool, ObjectError> {
        let deleted = sqlx::query(
            "DELETE FROM record_shares WHERE tenant_id = $1 AND record_id = $2 \
             AND grantee_kind = $3 AND grantee_id = $4",
        )
        .bind(self.tenant_id)
        .bind(record_id)
        .bind(grantee_kind.as_str())
        .bind(grantee_id)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        Ok(deleted == 1)
    }

    async fn delete_shares(&mut self, record_id: Uuid) -> Result<u64, ObjectError> {
        Ok(
            sqlx::query("DELETE FROM record_shares WHERE tenant_id = $1 AND record_id = $2")
                .bind(self.tenant_id)
                .bind(record_id)
                .execute(&mut *self.tx)
                .await?
                .rows_affected(),
        )
    }

    async fn append_history(&mut self, entries: &[HistoryEntry]) -> Result<(), ObjectError> {
        for entry in entries {
            sqlx::query(
                "INSERT INTO record_history (tenant_id, id, record_id, object_id, kind, field, \
                   old_value, new_value, changed_at, changed_by, correlation_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)",
            )
            .bind(self.tenant_id)
            .bind(entry.id)
            .bind(entry.record_id)
            .bind(entry.object_id)
            .bind(entry.kind.as_str())
            .bind(&entry.field)
            .bind(&entry.old_value)
            .bind(&entry.new_value)
            .bind(entry.changed_at)
            .bind(entry.changed_by)
            .bind(entry.correlation_id)
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn record_history(
        &mut self,
        record_id: Uuid,
        before: Option<(DateTime<Utc>, Uuid)>,
        limit: u16,
    ) -> Result<Vec<HistoryEntry>, ObjectError> {
        let mut sql: QueryBuilder<Postgres> = QueryBuilder::new(
            "SELECT id, record_id, object_id, kind, field, old_value, new_value, changed_at, \
             changed_by, correlation_id FROM record_history WHERE tenant_id = ",
        );
        sql.push_bind(self.tenant_id)
            .push(" AND record_id = ")
            .push_bind(record_id);
        if let Some((at, id)) = before {
            sql.push(" AND (changed_at, id) < (")
                .push_bind(at)
                .push(", ")
                .push_bind(id)
                .push(")");
        }
        sql.push(" ORDER BY changed_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let rows = sql.build().fetch_all(&mut *self.tx).await?;
        rows.iter()
            .map(|row| {
                Ok(HistoryEntry {
                    id: row.try_get("id")?,
                    record_id: row.try_get("record_id")?,
                    object_id: row.try_get("object_id")?,
                    kind: parse(HistoryKind::parse(row.try_get("kind")?))?,
                    field: row.try_get("field")?,
                    old_value: row.try_get("old_value")?,
                    new_value: row.try_get("new_value")?,
                    changed_at: row.try_get("changed_at")?,
                    changed_by: row.try_get("changed_by")?,
                    correlation_id: row.try_get("correlation_id")?,
                })
            })
            .collect()
    }

    async fn append_change(&mut self, change: &RecordChange) -> Result<i64, ObjectError> {
        let position: i64 = sqlx::query_scalar(
            "INSERT INTO change_counters (tenant_id, last_position) VALUES ($1, 1) \
             ON CONFLICT (tenant_id) DO UPDATE SET last_position = change_counters.last_position + 1 \
             RETURNING last_position",
        )
        .bind(self.tenant_id)
        .fetch_one(&mut *self.tx)
        .await?;
        sqlx::query(
            "INSERT INTO record_changes (tenant_id, position, record_id, object_id, public_id, kind, \
               fields, changed_at, changed_by, correlation_id) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(self.tenant_id)
        .bind(position)
        .bind(change.record_id)
        .bind(change.object_id)
        .bind(&change.public_id)
        .bind(change.kind.as_str())
        .bind(json(&change.fields)?)
        .bind(change.changed_at)
        .bind(change.changed_by)
        .bind(change.correlation_id)
        .execute(&mut *self.tx)
        .await?;
        Ok(position)
    }

    async fn changes(&mut self, after: i64, limit: u16) -> Result<Vec<RecordChange>, ObjectError> {
        let rows = sqlx::query(
            "SELECT position, record_id, object_id, public_id, kind, fields, changed_at, changed_by, \
               correlation_id FROM record_changes WHERE tenant_id = $1 AND position > $2 \
             ORDER BY position LIMIT $3",
        )
        .bind(self.tenant_id)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(RecordChange {
                    position: row.try_get("position")?,
                    record_id: row.try_get("record_id")?,
                    object_id: row.try_get("object_id")?,
                    public_id: row.try_get("public_id")?,
                    kind: parse(ChangeKind::parse(row.try_get("kind")?))?,
                    fields: serde_json::from_value(row.try_get("fields")?)
                        .map_err(|_| ObjectError::Invariant)?,
                    changed_at: row.try_get("changed_at")?,
                    changed_by: row.try_get("changed_by")?,
                    correlation_id: row.try_get("correlation_id")?,
                })
            })
            .collect()
    }

    async fn change_bounds(&mut self) -> Result<(i64, i64), ObjectError> {
        let bounds: Option<(i64, i64)> = sqlx::query_as(
            "SELECT last_position, expired_through FROM change_counters WHERE tenant_id = $1",
        )
        .bind(self.tenant_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        Ok(bounds.unwrap_or((0, 0)))
    }

    async fn bump_metadata_version(&mut self) -> Result<i64, ObjectError> {
        Ok(sqlx::query_scalar(
            "INSERT INTO change_counters (tenant_id, metadata_version) VALUES ($1, 1) \
             ON CONFLICT (tenant_id) DO UPDATE \
             SET metadata_version = change_counters.metadata_version + 1 \
             RETURNING metadata_version",
        )
        .bind(self.tenant_id)
        .fetch_one(&mut *self.tx)
        .await?)
    }

    async fn metadata_version(&mut self) -> Result<i64, ObjectError> {
        let version: Option<i64> =
            sqlx::query_scalar("SELECT metadata_version FROM change_counters WHERE tenant_id = $1")
                .bind(self.tenant_id)
                .fetch_optional(&mut *self.tx)
                .await?;
        Ok(version.unwrap_or(0))
    }

    async fn expire_changes(&mut self, before: DateTime<Utc>) -> Result<u64, ObjectError> {
        let through: Option<i64> = sqlx::query_scalar(
            "SELECT max(position) FROM record_changes WHERE tenant_id = $1 AND changed_at < $2",
        )
        .bind(self.tenant_id)
        .bind(before)
        .fetch_one(&mut *self.tx)
        .await?;
        let Some(through) = through else {
            return Ok(0);
        };
        let removed =
            sqlx::query("DELETE FROM record_changes WHERE tenant_id = $1 AND position <= $2")
                .bind(self.tenant_id)
                .bind(through)
                .execute(&mut *self.tx)
                .await?
                .rows_affected();
        sqlx::query(
            "UPDATE change_counters SET expired_through = greatest(expired_through, $2) WHERE tenant_id = $1",
        )
        .bind(self.tenant_id)
        .bind(through)
        .execute(&mut *self.tx)
        .await?;
        Ok(removed)
    }

    async fn query_records(&mut self, plan: &RecordPlan) -> Result<Vec<Record>, ObjectError> {
        let rendered = sql::records(Dialect::Postgres, self.tenant_id, plan);
        let rows = self.run_query(&rendered).await?;
        rows.iter().map(record_from_row).collect()
    }

    async fn query_aggregate(
        &mut self,
        plan: &AggregatePlan,
    ) -> Result<Vec<Vec<Cell>>, ObjectError> {
        let rendered = sql::aggregate(Dialect::Postgres, self.tenant_id, plan);
        let kinds = plan.output_kinds();
        let rows = self.run_query(&rendered).await?;
        rows.iter()
            .map(|row| {
                kinds
                    .iter()
                    .enumerate()
                    .map(|(index, kind)| {
                        Ok(match kind {
                            CellKind::Text => row
                                .try_get::<Option<String>, _>(index)?
                                .map_or(Cell::Null, Cell::Text),
                            CellKind::Number => row
                                .try_get::<Option<f64>, _>(index)?
                                .map_or(Cell::Null, Cell::Number),
                            CellKind::Int => row
                                .try_get::<Option<i64>, _>(index)?
                                .map_or(Cell::Null, Cell::Int),
                            CellKind::Time => row
                                .try_get::<Option<DateTime<Utc>>, _>(index)?
                                .map_or(Cell::Null, Cell::Time),
                            CellKind::Uuid => row
                                .try_get::<Option<Uuid>, _>(index)?
                                .map_or(Cell::Null, Cell::Uuid),
                        })
                    })
                    .collect()
            })
            .collect()
    }

    async fn search(
        &mut self,
        plan: &crate::search::SearchPlan,
    ) -> Result<Vec<(Record, i64)>, ObjectError> {
        let rendered = sql::search(Dialect::Postgres, self.tenant_id, plan);
        let rows = self.run_query(&rendered).await?;
        rows.iter()
            .map(|row| Ok((record_from_row(row)?, row.try_get::<i64, _>("score")?)))
            .collect()
    }

    async fn validation_rules(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<ValidationRule>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {RULE_COLUMNS} FROM validation_rules WHERE tenant_id = $1 AND object_id = $2 ORDER BY name"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(rule_from_row).collect()
    }

    async fn all_validation_rules(&mut self) -> Result<Vec<ValidationRule>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {RULE_COLUMNS} FROM validation_rules WHERE tenant_id = $1 ORDER BY object_id, name"
        ))
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(rule_from_row).collect()
    }

    async fn insert_validation_rule(&mut self, rule: &ValidationRule) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO validation_rules (tenant_id, id, object_id, name, description, condition, message, \
             error_field, active, version, created_at, created_by, updated_at, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(self.tenant_id)
        .bind(rule.id)
        .bind(rule.object_id)
        .bind(&rule.name)
        .bind(&rule.description)
        .bind(&rule.condition)
        .bind(&rule.message)
        .bind(&rule.error_field)
        .bind(rule.active)
        .bind(rule.version)
        .bind(rule.created_at)
        .bind(rule.created_by)
        .bind(rule.updated_at)
        .bind(rule.updated_by)
        .execute(&mut *self.tx)
        .await
        .map_err(|error| {
            if ObjectError::is_unique_violation(&error) {
                ObjectError::Conflict("rules.duplicate_name")
            } else {
                ObjectError::Database(error)
            }
        })?;
        Ok(())
    }

    async fn update_validation_rule(
        &mut self,
        rule: &ValidationRule,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE validation_rules SET description = $1, condition = $2, message = $3, error_field = $4, \
             active = $5, version = $6, updated_at = $7, updated_by = $8 \
             WHERE tenant_id = $9 AND id = $10 AND version = $11",
        )
        .bind(&rule.description)
        .bind(&rule.condition)
        .bind(&rule.message)
        .bind(&rule.error_field)
        .bind(rule.active)
        .bind(rule.version)
        .bind(rule.updated_at)
        .bind(rule.updated_by)
        .bind(self.tenant_id)
        .bind(rule.id)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete_validation_rule(
        &mut self,
        rule_id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "DELETE FROM validation_rules WHERE tenant_id = $1 AND id = $2 AND version = $3",
        )
        .bind(self.tenant_id)
        .bind(rule_id)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn quick_actions(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<crate::QuickAction>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {QUICK_ACTION_COLUMNS} {QUICK_ACTION_FROM} \
             WHERE q.tenant_id = $1 AND q.object_id = $2 ORDER BY q.position, q.name"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(quick_action_from_row).collect()
    }

    async fn quick_actions_targeting(
        &mut self,
        target_object_id: Uuid,
    ) -> Result<Vec<crate::QuickAction>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {QUICK_ACTION_COLUMNS} {QUICK_ACTION_FROM} \
             WHERE q.tenant_id = $1 AND q.target_object_id = $2 \
             ORDER BY q.object_id, q.position, q.name"
        ))
        .bind(self.tenant_id)
        .bind(target_object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(quick_action_from_row).collect()
    }

    async fn insert_quick_action(
        &mut self,
        action: &crate::QuickAction,
    ) -> Result<(), ObjectError> {
        let (fields, presets) = quick_action_json(action)?;
        sqlx::query(
            "INSERT INTO quick_actions (tenant_id, id, object_id, name, label, kind, position, \
             target_object_id, target_field, fields, presets, version, created_at, created_by, \
             updated_at, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(self.tenant_id)
        .bind(action.id)
        .bind(action.object_id)
        .bind(&action.name)
        .bind(&action.label)
        .bind(action.kind.as_str())
        .bind(action.position)
        .bind(action.target_object_id)
        .bind(&action.target_field)
        .bind(fields)
        .bind(presets)
        .bind(action.version)
        .bind(action.created_at)
        .bind(action.created_by)
        .bind(action.updated_at)
        .bind(action.updated_by)
        .execute(&mut *self.tx)
        .await
        .map_err(|error| {
            if ObjectError::is_unique_violation(&error) {
                ObjectError::Conflict("quick_actions.duplicate_name")
            } else {
                ObjectError::Database(error)
            }
        })?;
        Ok(())
    }

    async fn update_quick_action(
        &mut self,
        action: &crate::QuickAction,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let (fields, presets) = quick_action_json(action)?;
        let result = sqlx::query(
            "UPDATE quick_actions SET label = $1, position = $2, fields = $3, presets = $4, \
             version = $5, updated_at = $6, updated_by = $7 \
             WHERE tenant_id = $8 AND id = $9 AND version = $10",
        )
        .bind(&action.label)
        .bind(action.position)
        .bind(fields)
        .bind(presets)
        .bind(action.version)
        .bind(action.updated_at)
        .bind(action.updated_by)
        .bind(self.tenant_id)
        .bind(action.id)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete_quick_action(
        &mut self,
        action_id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "DELETE FROM quick_actions WHERE tenant_id = $1 AND id = $2 AND version = $3",
        )
        .bind(self.tenant_id)
        .bind(action_id)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn related_actions(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<crate::RelatedAction>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {RELATED_ACTION_COLUMNS} FROM related_actions \
             WHERE tenant_id = $1 AND object_id = $2 ORDER BY field_id, name"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(related_action_from_row).collect()
    }

    async fn put_related_action(
        &mut self,
        action: &crate::RelatedAction,
    ) -> Result<(), ObjectError> {
        let json =
            |value: Result<Value, serde_json::Error>| value.map_err(|_| ObjectError::Invariant);
        sqlx::query(
            "INSERT INTO related_actions (tenant_id, id, object_id, field_id, owner_plugin, name, \
             label, fields, presets, offered_to, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12) \
             ON CONFLICT (tenant_id, id) DO UPDATE SET label = EXCLUDED.label, \
             fields = EXCLUDED.fields, presets = EXCLUDED.presets, \
             offered_to = EXCLUDED.offered_to, updated_at = EXCLUDED.updated_at",
        )
        .bind(self.tenant_id)
        .bind(action.id)
        .bind(action.object_id)
        .bind(action.field_id)
        .bind(&action.owner_plugin)
        .bind(&action.name)
        .bind(&action.label)
        .bind(json(serde_json::to_value(&action.fields))?)
        .bind(json(serde_json::to_value(&action.presets))?)
        .bind(json(serde_json::to_value(&action.offered_to))?)
        .bind(action.created_at)
        .bind(action.updated_at)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn delete_related_action(&mut self, id: Uuid) -> Result<(), ObjectError> {
        sqlx::query("DELETE FROM related_actions WHERE tenant_id = $1 AND id = $2")
            .bind(self.tenant_id)
            .bind(id)
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    async fn timeline_sources(&mut self) -> Result<Vec<crate::TimelineSource>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {TIMELINE_COLUMNS} FROM timeline_sources WHERE tenant_id = $1 ORDER BY object_id"
        ))
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(timeline_from_row).collect()
    }

    async fn put_timeline_source(
        &mut self,
        source: &crate::TimelineSource,
    ) -> Result<(), ObjectError> {
        let (kind, field, value) = timeline_open_columns(source.open.as_ref());
        sqlx::query(
            "INSERT INTO timeline_sources (tenant_id, object_id, owner_plugin, through, \
             date_field_id, open_kind, open_field_id, open_value, created_at, updated_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) \
             ON CONFLICT (tenant_id, object_id) DO UPDATE SET through = EXCLUDED.through, \
             date_field_id = EXCLUDED.date_field_id, open_kind = EXCLUDED.open_kind, \
             open_field_id = EXCLUDED.open_field_id, open_value = EXCLUDED.open_value, \
             updated_at = EXCLUDED.updated_at",
        )
        .bind(self.tenant_id)
        .bind(source.object_id)
        .bind(&source.owner_plugin)
        .bind(serde_json::to_value(&source.through).map_err(|_| ObjectError::Invariant)?)
        .bind(source.date_field_id)
        .bind(kind)
        .bind(field)
        .bind(value)
        .bind(source.created_at)
        .bind(source.updated_at)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn delete_timeline_source(&mut self, object_id: Uuid) -> Result<(), ObjectError> {
        sqlx::query("DELETE FROM timeline_sources WHERE tenant_id = $1 AND object_id = $2")
            .bind(self.tenant_id)
            .bind(object_id)
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    async fn automations(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<crate::Automation>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations WHERE tenant_id = $1 AND object_id = $2 ORDER BY name"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(automation_from_row).collect()
    }

    async fn all_automations(&mut self) -> Result<Vec<crate::Automation>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations WHERE tenant_id = $1 ORDER BY object_id, name"
        ))
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(automation_from_row).collect()
    }

    async fn insert_automation(
        &mut self,
        automation: &crate::Automation,
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO automations (tenant_id, id, object_id, name, description, trigger, timing, \
             condition, actions, schedule, active, version, created_at, created_by, updated_at, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)",
        )
        .bind(self.tenant_id)
        .bind(automation.id)
        .bind(automation.object_id)
        .bind(&automation.name)
        .bind(&automation.description)
        .bind(automation.trigger.as_str())
        .bind(automation.timing.as_str())
        .bind(&automation.condition)
        .bind(actions_json(automation)?)
        .bind(schedule_json(automation)?)
        .bind(automation.active)
        .bind(automation.version)
        .bind(automation.created_at)
        .bind(automation.created_by)
        .bind(automation.updated_at)
        .bind(automation.updated_by)
        .execute(&mut *self.tx)
        .await
        .map_err(|error| {
            if ObjectError::is_unique_violation(&error) {
                ObjectError::Conflict("automations.duplicate_name")
            } else {
                ObjectError::Database(error)
            }
        })?;
        Ok(())
    }

    async fn update_automation(
        &mut self,
        automation: &crate::Automation,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE automations SET description = $1, trigger = $2, condition = $3, actions = $4, \
             schedule = $5, active = $6, version = $7, updated_at = $8, updated_by = $9 \
             WHERE tenant_id = $10 AND id = $11 AND version = $12",
        )
        .bind(&automation.description)
        .bind(automation.trigger.as_str())
        .bind(&automation.condition)
        .bind(actions_json(automation)?)
        .bind(schedule_json(automation)?)
        .bind(automation.active)
        .bind(automation.version)
        .bind(automation.updated_at)
        .bind(automation.updated_by)
        .bind(self.tenant_id)
        .bind(automation.id)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn delete_automation(
        &mut self,
        automation_id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "DELETE FROM automations WHERE tenant_id = $1 AND id = $2 AND version = $3",
        )
        .bind(self.tenant_id)
        .bind(automation_id)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn enqueue_job(&mut self, job: &crate::AutomationJob) -> Result<(), ObjectError> {
        if job.scheduled {
            let moved = sqlx::query(
                "UPDATE automation_jobs SET due_at = $1, event = $2, actor_id = $3, correlation_id = $4, \
                 depth = $5, updated_at = $6 WHERE tenant_id = $7 AND automation_id = $8 AND record_id = $9 \
                 AND scheduled AND status = 'pending'",
            )
            .bind(job.due_at)
            .bind(job.event.as_str())
            .bind(job.actor_id)
            .bind(job.correlation_id)
            .bind(job.depth)
            .bind(job.updated_at)
            .bind(self.tenant_id)
            .bind(job.automation_id)
            .bind(job.record_id)
            .execute(&mut *self.tx)
            .await?;
            if moved.rows_affected() > 0 {
                return Ok(());
            }
        }
        self.write_job(job, None).await.map(|_| ())
    }

    async fn cancel_scheduled_job(
        &mut self,
        automation_id: Uuid,
        record_id: Uuid,
        at: DateTime<Utc>,
    ) -> Result<u64, ObjectError> {
        let result = sqlx::query(
            "UPDATE automation_jobs SET status = 'cancelled', updated_at = $1 WHERE tenant_id = $2 \
             AND automation_id = $3 AND record_id = $4 AND scheduled AND status = 'pending'",
        )
        .bind(at)
        .bind(self.tenant_id)
        .bind(automation_id)
        .bind(record_id)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected())
    }

    async fn job(&mut self, job_id: Uuid) -> Result<Option<crate::AutomationJob>, ObjectError> {
        let row = sqlx::query(&format!(
            "SELECT {JOB_COLUMNS} FROM automation_jobs WHERE tenant_id = $1 AND id = $2"
        ))
        .bind(self.tenant_id)
        .bind(job_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(job_from_row).transpose()
    }

    async fn claim_job(
        &mut self,
        job_id: Uuid,
        now: DateTime<Utc>,
        locked_until: DateTime<Utc>,
    ) -> Result<Option<crate::AutomationJob>, ObjectError> {
        let row = sqlx::query(&format!(
            "UPDATE automation_jobs SET status = 'running', attempts = attempts + 1, locked_until = $1, \
             updated_at = $2 WHERE tenant_id = $3 AND id = $4 AND ((status = 'pending' AND due_at <= $2) \
             OR (status = 'running' AND locked_until < $2)) RETURNING {JOB_COLUMNS}"
        ))
        .bind(locked_until)
        .bind(now)
        .bind(self.tenant_id)
        .bind(job_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(job_from_row).transpose()
    }

    async fn update_job(
        &mut self,
        job: &crate::AutomationJob,
        from: crate::JobStatus,
    ) -> Result<bool, ObjectError> {
        self.write_job(job, Some(from)).await
    }

    async fn automation_jobs(
        &mut self,
        automation_id: Uuid,
        limit: u32,
    ) -> Result<Vec<crate::AutomationJob>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {JOB_COLUMNS} FROM automation_jobs WHERE tenant_id = $1 AND automation_id = $2 \
             ORDER BY created_at DESC, id DESC LIMIT $3"
        ))
        .bind(self.tenant_id)
        .bind(automation_id)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(job_from_row).collect()
    }

    async fn due_jobs(&mut self, now: DateTime<Utc>, limit: u32) -> Result<Vec<Uuid>, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM automation_jobs WHERE tenant_id = $1 AND ((status = 'pending' AND due_at <= $2) \
             OR (status = 'running' AND locked_until < $2)) ORDER BY due_at, id LIMIT $3",
        )
        .bind(self.tenant_id)
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn insert_bulk_job(
        &mut self,
        job: &crate::BulkJob,
        rows: &[crate::BulkRow],
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO bulk_jobs (tenant_id, id, object, operation, status, total, succeeded, failed, \
             actor_id, correlation_id, locked_until, created_at, updated_at, completed_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)",
        )
        .bind(self.tenant_id)
        .bind(job.id)
        .bind(&job.object)
        .bind(job.operation.as_str())
        .bind(job.status.as_str())
        .bind(job.total)
        .bind(job.succeeded)
        .bind(job.failed)
        .bind(job.actor_id)
        .bind(job.correlation_id)
        .bind(job.locked_until)
        .bind(job.created_at)
        .bind(job.updated_at)
        .bind(job.completed_at)
        .execute(&mut *self.tx)
        .await?;
        for row in rows {
            sqlx::query(
                "INSERT INTO bulk_job_rows (tenant_id, job_id, row_index, input, status, record, errors) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(self.tenant_id)
            .bind(row.job_id)
            .bind(row.index)
            .bind(&row.input)
            .bind(row.status.as_str())
            .bind(&row.record)
            .bind(&row.errors)
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn bulk_job(&mut self, job_id: Uuid) -> Result<Option<crate::BulkJob>, ObjectError> {
        let row = sqlx::query(&format!(
            "SELECT {BULK_COLUMNS} FROM bulk_jobs WHERE tenant_id = $1 AND id = $2"
        ))
        .bind(self.tenant_id)
        .bind(job_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(bulk_from_row).transpose()
    }

    async fn claim_bulk_job(
        &mut self,
        job_id: Uuid,
        now: DateTime<Utc>,
        locked_until: DateTime<Utc>,
    ) -> Result<Option<crate::BulkJob>, ObjectError> {
        let row = sqlx::query(&format!(
            "UPDATE bulk_jobs SET status = 'running', locked_until = $1, updated_at = $2 \
             WHERE tenant_id = $3 AND id = $4 AND (status = 'queued' OR (status = 'running' AND locked_until < $2)) \
             RETURNING {BULK_COLUMNS}"
        ))
        .bind(locked_until)
        .bind(now)
        .bind(self.tenant_id)
        .bind(job_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(bulk_from_row).transpose()
    }

    async fn update_bulk_job(&mut self, job: &crate::BulkJob) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE bulk_jobs j SET status = $1, locked_until = $2, updated_at = $3, completed_at = $4, \
             succeeded = (SELECT count(*) FROM bulk_job_rows r WHERE r.tenant_id = j.tenant_id \
             AND r.job_id = j.id AND r.status = 'succeeded'), \
             failed = (SELECT count(*) FROM bulk_job_rows r WHERE r.tenant_id = j.tenant_id \
             AND r.job_id = j.id AND r.status = 'failed') \
             WHERE j.tenant_id = $5 AND j.id = $6 AND j.status IN ('queued', 'running') \
             AND ($1 = 'aborted' OR j.status = 'running')",
        )
        .bind(job.status.as_str())
        .bind(job.locked_until)
        .bind(job.updated_at)
        .bind(job.completed_at)
        .bind(self.tenant_id)
        .bind(job.id)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn bulk_rows(
        &mut self,
        job_id: Uuid,
        status: Option<crate::BulkRowStatus>,
        after: Option<i32>,
        limit: u32,
    ) -> Result<Vec<crate::BulkRow>, ObjectError> {
        let rows = sqlx::query(
            "SELECT job_id, row_index, input, status, record, errors FROM bulk_job_rows \
             WHERE tenant_id = $1 AND job_id = $2 AND ($3::text IS NULL OR status = $3) \
             AND row_index > $4 ORDER BY row_index LIMIT $5",
        )
        .bind(self.tenant_id)
        .bind(job_id)
        .bind(status.map(crate::BulkRowStatus::as_str))
        .bind(after.unwrap_or(-1))
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(bulk_row_from_row).collect()
    }

    async fn finish_bulk_row(&mut self, row: &crate::BulkRow) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE bulk_job_rows SET status = $1, record = $2, errors = $3 \
             WHERE tenant_id = $4 AND job_id = $5 AND row_index = $6 AND status = 'pending'",
        )
        .bind(row.status.as_str())
        .bind(&row.record)
        .bind(&row.errors)
        .bind(self.tenant_id)
        .bind(row.job_id)
        .bind(row.index)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn due_bulk_jobs(
        &mut self,
        now: DateTime<Utc>,
        limit: u32,
    ) -> Result<Vec<Uuid>, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM bulk_jobs WHERE tenant_id = $1 AND (status = 'queued' \
             OR (status = 'running' AND locked_until < $2)) ORDER BY updated_at, id LIMIT $3",
        )
        .bind(self.tenant_id)
        .bind(now)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn insert_automation_runs(
        &mut self,
        runs: &[crate::AutomationRun],
    ) -> Result<(), ObjectError> {
        for run in runs {
            sqlx::query(
                "INSERT INTO automation_runs (tenant_id, id, automation_id, object_id, record, event, \
                 outcome, detail, saves, ran_at, ran_by, correlation_id) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
            )
            .bind(self.tenant_id)
            .bind(run.id)
            .bind(run.automation_id)
            .bind(run.object_id)
            .bind(&run.record)
            .bind(run.event.as_str())
            .bind(run.outcome.as_str())
            .bind(&run.detail)
            .bind(run.saves)
            .bind(run.ran_at)
            .bind(run.ran_by)
            .bind(run.correlation_id)
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn automation_runs(
        &mut self,
        automation_id: Uuid,
        before: Option<(DateTime<Utc>, Uuid)>,
        limit: u32,
    ) -> Result<Vec<crate::AutomationRun>, ObjectError> {
        let rows = match before {
            Some((ran_at, id)) => {
                sqlx::query(&format!(
                    "SELECT {RUN_COLUMNS} FROM automation_runs WHERE tenant_id = $1 AND automation_id = $2 \
                     AND (ran_at, id) < ($3, $4) ORDER BY ran_at DESC, id DESC LIMIT $5"
                ))
                .bind(self.tenant_id)
                .bind(automation_id)
                .bind(ran_at)
                .bind(id)
                .bind(i64::from(limit))
                .fetch_all(&mut *self.tx)
                .await?
            }
            None => {
                sqlx::query(&format!(
                    "SELECT {RUN_COLUMNS} FROM automation_runs WHERE tenant_id = $1 AND automation_id = $2 \
                     ORDER BY ran_at DESC, id DESC LIMIT $3"
                ))
                .bind(self.tenant_id)
                .bind(automation_id)
                .bind(i64::from(limit))
                .fetch_all(&mut *self.tx)
                .await?
            }
        };
        rows.iter().map(run_from_row).collect()
    }

    async fn expire_automation_runs(&mut self, before: DateTime<Utc>) -> Result<u64, ObjectError> {
        let result =
            sqlx::query("DELETE FROM automation_runs WHERE tenant_id = $1 AND ran_at < $2")
                .bind(self.tenant_id)
                .bind(before)
                .execute(&mut *self.tx)
                .await?;
        Ok(result.rows_affected())
    }

    async fn formulas_indexed(&mut self, object_id: Uuid) -> Result<bool, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM formula_index_marks WHERE tenant_id = $1 AND object_id = $2)",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_one(&mut *self.tx)
        .await?)
    }

    async fn mark_formulas_indexed(&mut self, object_id: Uuid) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO formula_index_marks (tenant_id, object_id, indexed_at) VALUES ($1, $2, now()) \
             ON CONFLICT (tenant_id, object_id) DO UPDATE SET indexed_at = EXCLUDED.indexed_at",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn record_ids(&mut self, object_id: Uuid) -> Result<Vec<Uuid>, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM records WHERE tenant_id = $1 AND object_id = $2 ORDER BY id",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn lock_object(&mut self, object_id: Uuid, exclusive: bool) -> Result<(), ObjectError> {
        let mode = if exclusive { "UPDATE" } else { "SHARE" };
        sqlx::query(&format!(
            "SELECT 1 FROM object_definitions WHERE tenant_id = $1 AND id = $2 FOR {mode}"
        ))
        .bind(self.tenant_id)
        .bind(object_id)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn lock_record(&mut self, record_id: Uuid, exclusive: bool) -> Result<(), ObjectError> {
        let mode = if exclusive { "UPDATE" } else { "SHARE" };
        sqlx::query(&format!(
            "SELECT 1 FROM records WHERE tenant_id = $1 AND id = $2 FOR {mode}"
        ))
        .bind(self.tenant_id)
        .bind(record_id)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn guarded_field_count(&mut self, object_id: Uuid) -> Result<usize, ObjectError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM field_definitions WHERE tenant_id = $1 AND object_id = $2 \
             AND field_type IN ('master_detail', 'rollup') AND status = 'active'",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_one(&mut *self.tx)
        .await?;
        usize::try_from(count).map_err(|_| ObjectError::Invariant)
    }

    async fn rollup_value(
        &mut self,
        query: &RollupQuery,
    ) -> Result<Option<IndexValue>, ObjectError> {
        let details = "FROM record_references rr JOIN records d \
             ON d.tenant_id = rr.tenant_id AND d.id = rr.record_id";
        let filter = "rr.tenant_id = $1 AND rr.field_id = $2 AND rr.target_id = $3 \
             AND d.deleted_at IS NULL";
        let (function, field) = match (query.function, query.field) {
            (RollupFunction::Count, _) => {
                let count: i64 =
                    sqlx::query_scalar(&format!("SELECT count(*) {details} WHERE {filter}"))
                        .bind(self.tenant_id)
                        .bind(query.relationship_id)
                        .bind(query.master_id)
                        .fetch_one(&mut *self.tx)
                        .await?;
                return Ok(Some(IndexValue::Number(count as f64)));
            }
            (_, None) => return Err(ObjectError::Invariant),
            (RollupFunction::Sum, Some(field)) => ("sum", field),
            (RollupFunction::Min, Some(field)) => ("min", field),
            (RollupFunction::Max, Some(field)) => ("max", field),
        };
        let (field_id, time) = field;
        let column = if time { "time_value" } else { "number_value" };
        let sql = format!(
            "SELECT {function}(i.{column}) {details} JOIN record_index i \
             ON i.tenant_id = rr.tenant_id AND i.record_id = rr.record_id AND i.field_id = $4 \
             WHERE {filter}"
        );
        let value = if time {
            sqlx::query_scalar::<_, Option<DateTime<Utc>>>(&sql)
                .bind(self.tenant_id)
                .bind(query.relationship_id)
                .bind(query.master_id)
                .bind(field_id)
                .fetch_one(&mut *self.tx)
                .await?
                .map(IndexValue::Time)
        } else {
            sqlx::query_scalar::<_, Option<f64>>(&sql)
                .bind(self.tenant_id)
                .bind(query.relationship_id)
                .bind(query.master_id)
                .bind(field_id)
                .fetch_one(&mut *self.tx)
                .await?
                .map(IndexValue::Number)
        };
        Ok(value)
    }

    async fn reports(&mut self) -> Result<Vec<crate::Report>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {REPORT_COLUMNS} FROM reports WHERE tenant_id = $1 ORDER BY id"
        ))
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(report_from_row).collect()
    }

    async fn report(&mut self, id: Uuid) -> Result<Option<crate::Report>, ObjectError> {
        sqlx::query(&format!(
            "SELECT {REPORT_COLUMNS} FROM reports WHERE tenant_id = $1 AND id = $2"
        ))
        .bind(self.tenant_id)
        .bind(id)
        .fetch_optional(&mut *self.tx)
        .await?
        .as_ref()
        .map(report_from_row)
        .transpose()
    }

    async fn insert_report(&mut self, report: &crate::Report) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO reports (tenant_id, id, name, description, query, chart, label_column, \
               value_column, visibility, owner_id, version, created_at, created_by, updated_at, updated_by) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)",
        )
        .bind(self.tenant_id)
        .bind(report.id)
        .bind(&report.name)
        .bind(&report.description)
        .bind(&report.query)
        .bind(report.chart.as_str())
        .bind(&report.label_column)
        .bind(&report.value_column)
        .bind(report.visibility.as_str())
        .bind(report.owner_id)
        .bind(report.version)
        .bind(report.created_at)
        .bind(report.created_by)
        .bind(report.updated_at)
        .bind(report.updated_by)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn update_report(
        &mut self,
        report: &crate::Report,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let updated = sqlx::query(
            "UPDATE reports SET name = $3, description = $4, query = $5, chart = $6, \
               label_column = $7, value_column = $8, visibility = $9, version = $10, \
               updated_at = $11, updated_by = $12 \
             WHERE tenant_id = $1 AND id = $2 AND version = $13",
        )
        .bind(self.tenant_id)
        .bind(report.id)
        .bind(&report.name)
        .bind(&report.description)
        .bind(&report.query)
        .bind(report.chart.as_str())
        .bind(&report.label_column)
        .bind(&report.value_column)
        .bind(report.visibility.as_str())
        .bind(report.version)
        .bind(report.updated_at)
        .bind(report.updated_by)
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    async fn delete_report(
        &mut self,
        id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let deleted =
            sqlx::query("DELETE FROM reports WHERE tenant_id = $1 AND id = $2 AND version = $3")
                .bind(self.tenant_id)
                .bind(id)
                .bind(expected_version)
                .execute(&mut *self.tx)
                .await?
                .rows_affected();
        Ok(deleted == 1)
    }

    async fn has_records(&mut self, object_id: Uuid) -> Result<bool, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM records WHERE tenant_id = $1 AND object_id = $2)",
        )
        .bind(self.tenant_id)
        .bind(object_id)
        .fetch_one(&mut *self.tx)
        .await?)
    }

    async fn master_links(&mut self) -> Result<Vec<(Uuid, Uuid)>, ObjectError> {
        Ok(sqlx::query_as(
            "SELECT object_id, reference_object_id FROM field_definitions \
             WHERE tenant_id = $1 AND field_type = 'master_detail' AND status = 'active' \
             ORDER BY object_id, id",
        )
        .bind(self.tenant_id)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn set_deleted_with(
        &mut self,
        record_id: Uuid,
        root: Option<Uuid>,
    ) -> Result<(), ObjectError> {
        sqlx::query("UPDATE records SET deleted_with = $3 WHERE tenant_id = $1 AND id = $2")
            .bind(self.tenant_id)
            .bind(record_id)
            .bind(root)
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    async fn deleted_with(&mut self, root: Uuid) -> Result<Vec<Uuid>, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM records WHERE tenant_id = $1 AND deleted_with = $2 \
             AND deleted_at IS NOT NULL ORDER BY id",
        )
        .bind(self.tenant_id)
        .bind(root)
        .fetch_all(&mut *self.tx)
        .await?)
    }

    async fn append_evidence(
        &mut self,
        audit: &AuditEvent,
        event: &EventEnvelope,
    ) -> Result<(), ObjectError> {
        platform_audit::append_in_transaction(&mut self.tx, audit).await?;
        platform_outbox::append_in_transaction(&mut self.tx, event).await?;
        Ok(())
    }

    async fn append_audit(&mut self, audit: &AuditEvent) -> Result<(), ObjectError> {
        platform_audit::append_in_transaction(&mut self.tx, audit).await?;
        Ok(())
    }

    // Replicas live on devices, which use SQLite (ADR 0024 §1).
    async fn remote_version(&mut self, _: Uuid) -> Result<Option<RemoteVersion>, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn set_remote_version(&mut self, _: Uuid, _: RemoteVersion) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn remember_base(&mut self, _: Uuid, _: &str, _: &Value) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn replica_bases(&mut self, _: Uuid) -> Result<BTreeMap<String, Value>, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn clear_replica_bases(&mut self, _: Uuid) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn sync_positions(&mut self) -> Result<SyncPositions, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn set_sync_positions(&mut self, _: SyncPositions) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn replica_lease(&mut self) -> Result<ReplicaLease, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn set_replica_lease(&mut self, _: ReplicaLease) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn replica_directory(&mut self) -> Result<ReplicaDirectory, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn replace_replica_directory(&mut self, _: &ReplicaDirectory) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn replica_manifest(&mut self) -> Result<Vec<(String, String)>, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn forget_record(&mut self, _: Uuid) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn add_pending_reference(&mut self, _: Uuid, _: &str) -> Result<(), ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn take_pending_references(&mut self, _: &str) -> Result<Vec<Uuid>, ObjectError> {
        Err(ObjectError::Conflict("replica.unsupported"))
    }

    async fn commit(self: Box<Self>) -> Result<(), ObjectError> {
        self.tx.commit().await?;
        Ok(())
    }
}
