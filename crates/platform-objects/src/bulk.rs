//! Bulk jobs (ADR 0023 §2): up to [`BULK_ROWS_MAX`] rows inserted, updated
//! or deleted in the background. Each row is a change of its own, with the
//! submitting member's access and the same validation, automations and
//! evidence as the single-record routes; its result is written in the same
//! transaction as its change.

use crate::automation::Run;
use crate::events::Change;
use crate::service::{MutationContext, ObjectService};
use crate::{
    BulkJob, BulkOperation, BulkRow, BulkRowStatus, BulkStatus, ObjectCaller, ObjectError,
    ObjectStore,
};
use chrono::{DateTime, Utc};
use serde_json::{json, Map, Value};
use std::sync::Arc;
use uuid::Uuid;

/// Rows per bulk job.
pub const BULK_ROWS_MAX: usize = 5000;
/// Rows the worker processes per claim, so other jobs get a turn.
pub const BULK_BATCH: u32 = 200;
/// How long a claim lasts; a worker that stops loses it.
const BULK_LOCK: chrono::TimeDelta = chrono::TimeDelta::minutes(10);
/// Problems reported for a rejected submission.
const ISSUES_MAX: usize = 20;

fn invalid(field: impl Into<String>, code: &'static str, detail: impl Into<String>) -> ObjectError {
    ObjectError::invalid(field, code, detail)
}

/// A row as submitted: the values for an insert, or the record `id`, its
/// optional `expected_version` and, for an update, its `values`.
struct RowInput<'a> {
    id: Option<&'a str>,
    expected_version: Option<i64>,
    values: Option<&'a Map<String, Value>>,
}

fn row_input(operation: BulkOperation, row: &Value) -> Result<RowInput<'_>, &'static str> {
    let Some(row) = row.as_object() else {
        return Err("send each row as an object");
    };
    if operation == BulkOperation::Insert {
        return Ok(RowInput {
            id: None,
            expected_version: None,
            values: Some(row),
        });
    }
    let allowed: &[&str] = if operation == BulkOperation::Update {
        &["id", "expected_version", "values"]
    } else {
        &["id", "expected_version"]
    };
    if row.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(if operation == BulkOperation::Update {
            "send id, values and optionally expected_version"
        } else {
            "send id and optionally expected_version"
        });
    }
    let Some(id) = row.get("id").and_then(Value::as_str) else {
        return Err("send the record's id");
    };
    let expected_version = match row.get("expected_version") {
        None | Some(Value::Null) => None,
        Some(version) => match version.as_i64() {
            Some(version) => Some(version),
            None => return Err("send expected_version as a whole number"),
        },
    };
    let values = match (operation, row.get("values")) {
        (BulkOperation::Update, Some(Value::Object(values))) => Some(values),
        (BulkOperation::Update, _) => return Err("send the values to set as an object"),
        _ => None,
    };
    Ok(RowInput {
        id: Some(id),
        expected_version,
        values,
    })
}

/// A row's problem as the single-record route would report it: the stable
/// code and, for validation, each issue.
fn row_errors(error: &ObjectError) -> Value {
    match error {
        ObjectError::NotFound => json!({"code": "resource.unavailable"}),
        ObjectError::PermissionDenied => json!({"code": "auth.permission_denied"}),
        ObjectError::Validation(issues) => {
            json!({"code": "validation.failed", "errors": issues})
        }
        ObjectError::VersionConflict => json!({"code": "records.version_conflict"}),
        ObjectError::Conflict(code) => json!({"code": code}),
        ObjectError::Database(_) | ObjectError::Invariant => {
            json!({"code": "platform.unavailable"})
        }
    }
}

/// What a worker's turn at a job did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BulkOutcome {
    /// Someone else holds it, or it has no work.
    NotClaimed,
    /// It processed a batch and has rows left.
    Continuing,
    Completed,
    /// It was aborted while the worker held it.
    Aborted,
}

impl ObjectService {
    /// Queues a bulk job (ADR 0023 §2). The caller needs the access the
    /// operation needs on the object; each row is checked again, with the
    /// caller's access as it is now, when it runs. A malformed row rejects
    /// the whole job.
    pub async fn submit_bulk_job(
        &self,
        api_name: &str,
        operation: BulkOperation,
        rows: &[Value],
        context: MutationContext,
    ) -> Result<BulkJob, ObjectError> {
        if rows.is_empty() || rows.len() > BULK_ROWS_MAX {
            return Err(invalid(
                "rows",
                "bulk.rows_invalid",
                format!("send 1 to {BULK_ROWS_MAX} rows"),
            ));
        }
        let issues: Vec<_> = rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                row_input(operation, row).err().map(|detail| {
                    crate::ValidationIssue::new(
                        format!("rows[{index}]"),
                        "bulk.row_invalid",
                        detail,
                    )
                })
            })
            .take(ISSUES_MAX)
            .collect();
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let flags = access.for_object(object.id);
        if !flags.read {
            return Err(ObjectError::NotFound);
        }
        let allowed = match operation {
            BulkOperation::Insert => flags.create,
            BulkOperation::Update => flags.edit,
            BulkOperation::Delete => flags.delete,
        };
        if !allowed {
            return Err(ObjectError::PermissionDenied);
        }
        let now = Utc::now();
        let caller = self.caller();
        let job = BulkJob {
            id: Uuid::now_v7(),
            object: object.api_name.clone(),
            operation,
            status: BulkStatus::Queued,
            total: i64::try_from(rows.len()).map_err(|_| ObjectError::Invariant)?,
            succeeded: 0,
            failed: 0,
            actor_id: caller.user_id(),
            correlation_id: context.correlation_id,
            locked_until: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        let rows: Vec<BulkRow> = rows
            .iter()
            .enumerate()
            .map(|(index, input)| {
                Ok(BulkRow {
                    job_id: job.id,
                    index: i32::try_from(index).map_err(|_| ObjectError::Invariant)?,
                    input: input.clone(),
                    status: BulkRowStatus::Pending,
                    record: None,
                    errors: None,
                })
            })
            .collect::<Result<_, ObjectError>>()?;
        tx.insert_bulk_job(&job, &rows).await?;
        self.record_change(tx.as_mut(), context, Change::BulkJobSubmitted(&job))
            .await?;
        tx.commit().await?;
        Ok(job)
    }

    /// A bulk job the caller submitted, or any job for administrators who
    /// manage objects.
    async fn visible_bulk_job(
        &self,
        tx: &mut dyn crate::ObjectTx,
        job_id: Uuid,
    ) -> Result<BulkJob, ObjectError> {
        tx.bulk_job(job_id)
            .await?
            .filter(|job| job.actor_id == self.caller().user_id() || self.caller().can_manage())
            .ok_or(ObjectError::NotFound)
    }

    pub async fn bulk_job(&self, job_id: Uuid) -> Result<BulkJob, ObjectError> {
        let mut tx = self.begin().await?;
        let job = self.visible_bulk_job(tx.as_mut(), job_id).await?;
        tx.commit().await?;
        Ok(job)
    }

    /// A job's rows in order, after row `after`, optionally only those with
    /// `status`.
    pub async fn bulk_job_rows(
        &self,
        job_id: Uuid,
        status: Option<BulkRowStatus>,
        after: Option<i32>,
        limit: u32,
    ) -> Result<Vec<BulkRow>, ObjectError> {
        if !(1..=1000).contains(&limit) {
            return Err(invalid(
                "limit",
                "bulk.limit_invalid",
                "ask for between 1 and 1000 rows",
            ));
        }
        let mut tx = self.begin().await?;
        self.visible_bulk_job(tx.as_mut(), job_id).await?;
        let rows = tx.bulk_rows(job_id, status, after, limit).await?;
        tx.commit().await?;
        Ok(rows)
    }

    /// Stops a queued or running job; rows already processed keep their
    /// changes and the rest stay pending. Anything else is
    /// `409 bulk.job_state`.
    pub async fn abort_bulk_job(
        &self,
        job_id: Uuid,
        context: MutationContext,
    ) -> Result<BulkJob, ObjectError> {
        let mut tx = self.begin().await?;
        let mut job = self.visible_bulk_job(tx.as_mut(), job_id).await?;
        if !matches!(job.status, BulkStatus::Queued | BulkStatus::Running) {
            return Err(ObjectError::Conflict("bulk.job_state"));
        }
        let now = Utc::now();
        job.status = BulkStatus::Aborted;
        job.locked_until = None;
        job.updated_at = now;
        job.completed_at = Some(now);
        // A worker may have completed it, or someone aborted it, since it
        // was read.
        if !tx.update_bulk_job(&job).await? {
            return Err(ObjectError::Conflict("bulk.job_state"));
        }
        let job = tx.bulk_job(job_id).await?.ok_or(ObjectError::Invariant)?;
        self.record_change(tx.as_mut(), context, Change::BulkJobAborted(&job))
            .await?;
        tx.commit().await?;
        Ok(job)
    }

    /// Applies one row in a transaction of its own, writing its result in
    /// the same transaction. A row that fails is rolled back and its
    /// problem written in another. False when the job was aborted.
    async fn apply_bulk_row(&self, job: &BulkJob, row: BulkRow) -> Result<bool, ObjectError> {
        // Each row's change names the job as its source (#233).
        let context = MutationContext {
            correlation_id: job.correlation_id,
            causation_id: Some(job.id),
            automation_id: None,
            source: Some(crate::MutationSource::import(job.id)),
        };
        let mut run = Run::default();
        let mut tx = self.begin().await?;
        if tx
            .bulk_job(job.id)
            .await?
            .is_none_or(|current| current.status == BulkStatus::Aborted)
        {
            return Ok(false);
        }
        let result = self
            .apply_bulk_input(tx.as_mut(), job, &row.input, context, &mut run)
            .await;
        let errors = match result {
            Ok(record) => {
                let finished = BulkRow {
                    status: BulkRowStatus::Succeeded,
                    record: Some(record),
                    errors: None,
                    ..row
                };
                // A worker whose claim lapsed may find the row already
                // done; its change then rolls back, so no row applies twice.
                if tx.finish_bulk_row(&finished).await? {
                    tx.commit().await?;
                }
                return Ok(true);
            }
            Err(error) => {
                drop(tx);
                let errors = row_errors(&error);
                // Automation runs that failed are kept, as for a single
                // change; the row's own problem is reported below.
                let _ = self.keep_failed_runs(run, Err::<(), _>(error)).await;
                errors
            }
        };
        let mut tx = self.begin().await?;
        tx.finish_bulk_row(&BulkRow {
            status: BulkRowStatus::Failed,
            record: None,
            errors: Some(errors),
            ..row
        })
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// The row's change, through the same steps as the single-record
    /// routes. Without `expected_version` the record's current version is
    /// used, as Salesforce bulk updates do.
    async fn apply_bulk_input(
        &self,
        tx: &mut dyn crate::ObjectTx,
        job: &BulkJob,
        input: &Value,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<String, ObjectError> {
        let parsed = row_input(job.operation, input)
            .map_err(|detail| invalid("row", "bulk.row_invalid", detail))?;
        let access = self.access(tx).await?;
        let version = match (parsed.id, parsed.expected_version) {
            (Some(_), Some(version)) => version,
            (Some(id), None) => {
                let object = self.active_object(tx, &job.object).await?;
                tx.record(object.id, id)
                    .await?
                    .filter(|record| record.deleted_at.is_none())
                    .map(|record| record.version)
                    .ok_or(ObjectError::NotFound)?
            }
            (None, _) => 0,
        };
        let empty = Map::new();
        let values = parsed.values.unwrap_or(&empty);
        let (_, record) = match (job.operation, parsed.id) {
            (BulkOperation::Insert, _) => {
                self.create_record_tx(tx, &access, &job.object, values, context, run)
                    .await?
            }
            (BulkOperation::Update, Some(id)) => {
                self.update_record_tx(tx, &access, &job.object, id, version, values, context, run)
                    .await?
            }
            (BulkOperation::Delete, Some(id)) => {
                self.delete_record_tx(tx, &access, &job.object, id, version, context, run)
                    .await?
            }
            (_, None) => return Err(ObjectError::Invariant),
        };
        Ok(record.public_id)
    }
}

/// Claims a bulk job in its workspace for one batch (ADR 0023 §2). `None`
/// when someone else holds it or it has no work.
pub async fn claim_bulk_job(
    store: &Arc<dyn ObjectStore>,
    tenant_id: Uuid,
    job_id: Uuid,
    now: DateTime<Utc>,
) -> Result<Option<BulkJob>, ObjectError> {
    let mut tx = store.begin(tenant_id).await?;
    let claimed = tx.claim_bulk_job(job_id, now, now + BULK_LOCK).await?;
    tx.commit().await?;
    Ok(claimed)
}

/// Runs a batch of a claimed bulk job: up to [`BULK_BATCH`] pending rows,
/// as `member`, the job's submitter with their access as the kernel
/// resolves it now. Then it requeues the job or marks it completed. A
/// submitter who is no longer a member (`None`) aborts the job.
pub async fn process_bulk_job(
    store: Arc<dyn ObjectStore>,
    tenant_id: Uuid,
    job: BulkJob,
    member: Option<ObjectCaller>,
) -> Result<BulkOutcome, ObjectError> {
    let context = MutationContext {
        correlation_id: job.correlation_id,
        causation_id: Some(job.id),
        automation_id: None,
        source: Some(crate::MutationSource::import(job.id)),
    };
    let Some(member) =
        member.filter(|member| member.tenant_id() == tenant_id && member.user_id() == job.actor_id)
    else {
        let service = ObjectService::new(
            store,
            ObjectCaller::new(tenant_id, job.actor_id, Default::default(), Vec::new()),
        );
        let mut tx = service.begin().await?;
        let finished = Utc::now();
        let mut aborted = job.clone();
        aborted.status = BulkStatus::Aborted;
        aborted.locked_until = None;
        aborted.updated_at = finished;
        aborted.completed_at = Some(finished);
        if !tx.update_bulk_job(&aborted).await? {
            // Already aborted by someone else; nothing more to write.
            return Ok(BulkOutcome::Aborted);
        }
        let aborted = tx.bulk_job(job.id).await?.ok_or(ObjectError::Invariant)?;
        service
            .record_change(tx.as_mut(), context, Change::BulkJobAborted(&aborted))
            .await?;
        tx.commit().await?;
        return Ok(BulkOutcome::Aborted);
    };
    let service = ObjectService::new(store, member);
    let mut tx = service.begin().await?;
    let rows = tx
        .bulk_rows(job.id, Some(BulkRowStatus::Pending), None, BULK_BATCH)
        .await?;
    tx.commit().await?;
    for row in rows {
        if !service.apply_bulk_row(&job, row).await? {
            return Ok(BulkOutcome::Aborted);
        }
    }
    let mut tx = service.begin().await?;
    let left = tx
        .bulk_rows(job.id, Some(BulkRowStatus::Pending), None, 1)
        .await?;
    let finished = Utc::now();
    let mut next = job.clone();
    next.locked_until = None;
    next.updated_at = finished;
    let outcome = if left.is_empty() {
        next.status = BulkStatus::Completed;
        next.completed_at = Some(finished);
        BulkOutcome::Completed
    } else {
        next.status = BulkStatus::Queued;
        BulkOutcome::Continuing
    };
    if !tx.update_bulk_job(&next).await? {
        // Aborted since the last row; nothing more to write.
        return Ok(BulkOutcome::Aborted);
    }
    if outcome == BulkOutcome::Completed {
        let done = tx.bulk_job(job.id).await?.ok_or(ObjectError::Invariant)?;
        service
            .record_change(tx.as_mut(), context, Change::BulkJobCompleted(&done))
            .await?;
    }
    tx.commit().await?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::{row_errors, row_input};
    use crate::{BulkOperation, ObjectError};
    use serde_json::json;

    #[test]
    fn rows_have_the_shape_their_operation_needs() {
        assert!(row_input(BulkOperation::Insert, &json!({"name": "A"})).is_ok());
        assert!(row_input(BulkOperation::Insert, &json!(["A"])).is_err());
        let row = json!({"id": "x", "expected_version": 2, "values": {"name": "B"}});
        let update = row_input(BulkOperation::Update, &row).expect("update");
        assert_eq!(update.id, Some("x"));
        assert_eq!(update.expected_version, Some(2));
        assert!(row_input(BulkOperation::Update, &json!({"id": "x"})).is_err());
        assert!(row_input(BulkOperation::Update, &json!({"values": {}})).is_err());
        assert!(row_input(BulkOperation::Delete, &json!({"id": "x"})).is_ok());
        assert!(row_input(BulkOperation::Delete, &json!({"id": "x", "values": {}})).is_err());
        assert!(row_input(
            BulkOperation::Delete,
            &json!({"id": "x", "expected_version": "2"})
        )
        .is_err());
    }

    #[test]
    fn row_errors_use_the_route_codes() {
        assert_eq!(
            row_errors(&ObjectError::VersionConflict),
            json!({"code": "records.version_conflict"})
        );
        assert_eq!(
            row_errors(&ObjectError::Conflict("records.duplicate_value")),
            json!({"code": "records.duplicate_value"})
        );
        assert_eq!(
            row_errors(&ObjectError::invalid(
                "name",
                "records.value_required",
                "Name is required"
            )),
            json!({"code": "validation.failed", "errors": [{"field": "name", "code": "records.value_required", "detail": "Name is required"}]})
        );
    }
}
