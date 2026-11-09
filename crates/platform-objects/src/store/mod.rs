//! The object engine storage port (ADR 0013, ADR 0015).
//!
//! The service owns validation, authorisation and evidence; a store only
//! persists. Each backend implements [`ObjectStore`] and passes the same
//! conformance suite.

use crate::query::plan::{AggregatePlan, Cell, RecordPlan};
use crate::{
    AccessFlags, FieldDefinition, GranteeKind, HistoryEntry, ObjectDefinition, ObjectError, Record,
    RecordChange, RecordIndex, RecordQuery, RecordShare, RolePermission, ShareAccess,
    ValidationRule,
};
use crate::{RemoteVersion, ReplicaDirectory, ReplicaLease, SyncPositions};
use async_trait::async_trait;
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use std::collections::BTreeMap;
use uuid::Uuid;

pub mod postgres;
pub mod sqlite;

pub use postgres::PostgresObjectStore;
pub use sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};

/// Opens workspace units of work.
#[async_trait]
pub trait ObjectStore: Send + Sync {
    /// A write transaction for one workspace. Metadata, records, pivots,
    /// audit evidence and outbox events written through it commit together.
    async fn begin(&self, tenant_id: Uuid) -> Result<Box<dyn ObjectTx>, ObjectError>;
}

/// One roll-up summary for one master record (ADR 0026).
#[derive(Debug, Clone, Copy)]
pub struct RollupQuery {
    pub relationship_id: Uuid,
    pub master_id: Uuid,
    pub function: crate::RollupFunction,
    /// The summarised field, and whether its index holds times.
    pub field: Option<(Uuid, bool)>,
}

/// One workspace transaction. Every method is scoped to the workspace the
/// transaction was opened for.
#[async_trait]
pub trait ObjectTx: Send {
    /// Serialises metadata changes in the workspace for the rest of the
    /// transaction (key prefix allocation, limits).
    async fn lock_metadata(&mut self) -> Result<(), ObjectError>;

    /// Active objects, without fields.
    async fn list_objects(&mut self) -> Result<Vec<ObjectDefinition>, ObjectError>;
    /// An active object with its active fields.
    async fn object_by_api_name(
        &mut self,
        api_name: &str,
    ) -> Result<Option<ObjectDefinition>, ObjectError>;
    /// Any object by id, with its active fields.
    async fn object_by_id(&mut self, id: Uuid) -> Result<Option<ObjectDefinition>, ObjectError>;
    /// Every key prefix used by any object, deleted ones included.
    async fn key_prefixes(&mut self) -> Result<Vec<String>, ObjectError>;
    /// Active lookup fields on any object that point to `object_id`.
    async fn fields_referencing_object(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<FieldDefinition>, ObjectError>;
    /// Inserts a definition; a duplicate API name is
    /// `Conflict("objects.duplicate_api_name")`.
    async fn insert_object(&mut self, object: &ObjectDefinition) -> Result<(), ObjectError>;
    /// Writes a definition if it is still at `expected_version`.
    async fn update_object(
        &mut self,
        object: &ObjectDefinition,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    async fn insert_field(&mut self, field: &FieldDefinition) -> Result<(), ObjectError>;
    async fn update_field(
        &mut self,
        field: &FieldDefinition,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// Removes every pivot row of a field.
    async fn drop_field_index(&mut self, field_id: Uuid) -> Result<(), ObjectError>;

    /// Object grants for these roles, combined per object.
    async fn grants(
        &mut self,
        role_ids: &[Uuid],
    ) -> Result<BTreeMap<Uuid, AccessFlags>, ObjectError>;
    async fn object_permissions(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<RolePermission>, ObjectError>;
    /// Replaces one role's grant on an object; no access removes the row.
    async fn set_object_permission(
        &mut self,
        object_id: Uuid,
        permission: &RolePermission,
        actor: Uuid,
    ) -> Result<(), ObjectError>;

    /// The next public-ID sequence number for an object's records.
    async fn next_record_sequence(&mut self, object_id: Uuid) -> Result<u64, ObjectError>;
    async fn insert_record(&mut self, record: &Record) -> Result<(), ObjectError>;
    /// Writes a record if it is still at `expected_version`.
    async fn update_record(
        &mut self,
        record: &Record,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// A record of one object by public ID, deleted ones included.
    async fn record(
        &mut self,
        object_id: Uuid,
        public_id: &str,
    ) -> Result<Option<Record>, ObjectError>;
    /// A record of any object by public ID, deleted ones included.
    async fn record_by_public_id(&mut self, public_id: &str)
        -> Result<Option<Record>, ObjectError>;
    async fn record_by_id(&mut self, id: Uuid) -> Result<Option<Record>, ObjectError>;
    /// Replaces a record's pivot rows. A unique value another record holds is
    /// `Conflict("records.duplicate_value")`.
    async fn replace_index(
        &mut self,
        record_id: Uuid,
        index: &RecordIndex,
    ) -> Result<(), ObjectError>;
    /// `(field id, record id)` for records outside the recycle bin whose
    /// lookups point to `target_id`.
    async fn referencing(&mut self, target_id: Uuid) -> Result<Vec<(Uuid, Uuid)>, ObjectError>;
    /// One page of an object's records, fetching one more than the limit so
    /// the service knows whether another page follows.
    async fn list_records(
        &mut self,
        object_id: Uuid,
        query: &RecordQuery,
    ) -> Result<Vec<Record>, ObjectError>;
    /// Manual shares of one record (ADR 0016).
    async fn record_shares(&mut self, record_id: Uuid) -> Result<Vec<RecordShare>, ObjectError>;
    /// The strongest share on a record held by `user_id` or one of
    /// `role_ids`.
    async fn share_access(
        &mut self,
        record_id: Uuid,
        user_id: Uuid,
        role_ids: &[Uuid],
    ) -> Result<Option<ShareAccess>, ObjectError>;
    /// Adds a share or changes the access of an existing one.
    async fn put_share(&mut self, record_id: Uuid, share: &RecordShare) -> Result<(), ObjectError>;
    /// Removes one share; `false` when there was none.
    async fn delete_share(
        &mut self,
        record_id: Uuid,
        grantee_kind: GranteeKind,
        grantee_id: Uuid,
    ) -> Result<bool, ObjectError>;
    /// Removes every manual share of a record, returning how many there were.
    async fn delete_shares(&mut self, record_id: Uuid) -> Result<u64, ObjectError>;
    /// Appends history entries (ADR 0017); history is never changed.
    async fn append_history(&mut self, entries: &[HistoryEntry]) -> Result<(), ObjectError>;
    /// A record's history, newest first, strictly before `before`, fetching
    /// one more than `limit` so the service knows whether more follow.
    async fn record_history(
        &mut self,
        record_id: Uuid,
        before: Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
        limit: u16,
    ) -> Result<Vec<HistoryEntry>, ObjectError>;
    /// Appends to the change log at the workspace's next position, holding
    /// the workspace's counter until commit so positions follow commit order
    /// (ADR 0018). `change.position` is ignored; the assigned one is returned.
    async fn append_change(&mut self, change: &RecordChange) -> Result<i64, ObjectError>;
    /// Up to `limit` changes after `after`, oldest first.
    async fn changes(&mut self, after: i64, limit: u16) -> Result<Vec<RecordChange>, ObjectError>;
    /// `(head, expired_through)`: the newest position, and the position up to
    /// which changes have expired.
    async fn change_bounds(&mut self) -> Result<(i64, i64), ObjectError>;
    /// Increments and returns the workspace metadata version (ADR 0024 §3),
    /// holding the workspace's counter until commit like `append_change`.
    async fn bump_metadata_version(&mut self) -> Result<i64, ObjectError>;
    /// The workspace metadata version; 0 before any definition exists.
    async fn metadata_version(&mut self) -> Result<i64, ObjectError>;
    /// Removes changes made before `before` and records how far the log has
    /// expired.
    async fn expire_changes(
        &mut self,
        before: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, ObjectError>;
    /// Records matching a query plan, in its order (ADR 0019).
    async fn query_records(&mut self, plan: &RecordPlan) -> Result<Vec<Record>, ObjectError>;
    /// The rows of an aggregate plan, one cell per output, groups first.
    async fn query_aggregate(
        &mut self,
        plan: &AggregatePlan,
    ) -> Result<Vec<Vec<Cell>>, ObjectError>;
    /// Records matching a global search with their scores, best first
    /// (ADR 0020).
    async fn search(
        &mut self,
        plan: &crate::search::SearchPlan,
    ) -> Result<Vec<(Record, i64)>, ObjectError>;
    /// An object's validation rules, by name (ADR 0021).
    async fn validation_rules(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<ValidationRule>, ObjectError>;
    /// Every validation rule in the workspace, for dependency checks.
    async fn all_validation_rules(&mut self) -> Result<Vec<ValidationRule>, ObjectError>;
    /// Inserts a rule; a duplicate name on the object is
    /// `Conflict("rules.duplicate_name")`.
    async fn insert_validation_rule(&mut self, rule: &ValidationRule) -> Result<(), ObjectError>;
    /// Writes a rule if it is still at `expected_version`.
    async fn update_validation_rule(
        &mut self,
        rule: &ValidationRule,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// Removes a rule if it is still at `expected_version`.
    async fn delete_validation_rule(
        &mut self,
        rule_id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// An object's quick actions, by position then name (ADR 0032).
    async fn quick_actions(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<crate::QuickAction>, ObjectError>;
    /// The create actions of any object whose child object is
    /// `target_object_id`, for cascades.
    async fn quick_actions_targeting(
        &mut self,
        target_object_id: Uuid,
    ) -> Result<Vec<crate::QuickAction>, ObjectError>;
    /// Inserts an action; a duplicate name on the object is
    /// `Conflict("quick_actions.duplicate_name")`.
    async fn insert_quick_action(&mut self, action: &crate::QuickAction)
        -> Result<(), ObjectError>;
    /// Writes an action's label, position, fields and presets if it is
    /// still at `expected_version`.
    async fn update_quick_action(
        &mut self,
        action: &crate::QuickAction,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// Removes an action if it is still at `expected_version`.
    async fn delete_quick_action(
        &mut self,
        action_id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// An object's automations, by name (ADR 0022).
    async fn automations(&mut self, object_id: Uuid)
        -> Result<Vec<crate::Automation>, ObjectError>;
    /// Every automation in the workspace, for dependency checks.
    async fn all_automations(&mut self) -> Result<Vec<crate::Automation>, ObjectError>;
    /// Inserts an automation; a duplicate name on the object is
    /// `Conflict("automations.duplicate_name")`.
    async fn insert_automation(
        &mut self,
        automation: &crate::Automation,
    ) -> Result<(), ObjectError>;
    /// Writes an automation if it is still at `expected_version`.
    async fn update_automation(
        &mut self,
        automation: &crate::Automation,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// Removes an automation if it is still at `expected_version`.
    async fn delete_automation(
        &mut self,
        automation_id: Uuid,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// Queues a background or scheduled job (ADR 0022 §6). A scheduled job
    /// moves the pending job of its automation and record, if there is one.
    async fn enqueue_job(&mut self, job: &crate::AutomationJob) -> Result<(), ObjectError>;
    /// Cancels the pending scheduled job of an automation and record.
    async fn cancel_scheduled_job(
        &mut self,
        automation_id: Uuid,
        record_id: Uuid,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, ObjectError>;
    async fn job(&mut self, job_id: Uuid) -> Result<Option<crate::AutomationJob>, ObjectError>;
    /// Claims a due job, or one whose claim lapsed: it becomes `running`
    /// until `locked_until`, with one more attempt. `None` when it is not
    /// due or someone else holds it.
    async fn claim_job(
        &mut self,
        job_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
        locked_until: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<crate::AutomationJob>, ObjectError>;
    /// Writes a job's state if it is still `from` and, for a running job,
    /// still under the claim that set its `attempts`; false otherwise, so a
    /// stale transition or a lapsed claim never overwrites a newer one.
    async fn update_job(
        &mut self,
        job: &crate::AutomationJob,
        from: crate::JobStatus,
    ) -> Result<bool, ObjectError>;
    /// An automation's jobs, newest first.
    async fn automation_jobs(
        &mut self,
        automation_id: Uuid,
        limit: u32,
    ) -> Result<Vec<crate::AutomationJob>, ObjectError>;
    /// Ids of the workspace's jobs that are due at `now`.
    async fn due_jobs(
        &mut self,
        now: chrono::DateTime<chrono::Utc>,
        limit: u32,
    ) -> Result<Vec<Uuid>, ObjectError>;
    /// Stores a bulk job and its rows (ADR 0023 §2).
    async fn insert_bulk_job(
        &mut self,
        job: &crate::BulkJob,
        rows: &[crate::BulkRow],
    ) -> Result<(), ObjectError>;
    async fn bulk_job(&mut self, job_id: Uuid) -> Result<Option<crate::BulkJob>, ObjectError>;
    /// Claims a queued bulk job, or one whose claim lapsed, until
    /// `locked_until`. `None` when it has no work or someone holds it.
    async fn claim_bulk_job(
        &mut self,
        job_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
        locked_until: chrono::DateTime<chrono::Utc>,
    ) -> Result<Option<crate::BulkJob>, ObjectError>;
    /// Writes a bulk job's status, lock and times, and recounts its rows.
    /// Only a queued or running job can be aborted, and only a running one
    /// can be requeued or completed; false when the job was in any other
    /// state, so a stale transition never overwrites a newer one.
    async fn update_bulk_job(&mut self, job: &crate::BulkJob) -> Result<bool, ObjectError>;
    /// A job's rows in order, after `after` when given, with `status` when
    /// given.
    async fn bulk_rows(
        &mut self,
        job_id: Uuid,
        status: Option<crate::BulkRowStatus>,
        after: Option<i32>,
        limit: u32,
    ) -> Result<Vec<crate::BulkRow>, ObjectError>;
    /// Writes a row's result if it is still pending; false otherwise.
    async fn finish_bulk_row(&mut self, row: &crate::BulkRow) -> Result<bool, ObjectError>;
    /// Ids of the workspace's bulk jobs that have work to do at `now`, least
    /// recently touched first, so a job handed back after a batch waits its turn.
    async fn due_bulk_jobs(
        &mut self,
        now: chrono::DateTime<chrono::Utc>,
        limit: u32,
    ) -> Result<Vec<Uuid>, ObjectError>;
    /// Records automation runs (ADR 0022 §5).
    async fn insert_automation_runs(
        &mut self,
        runs: &[crate::AutomationRun],
    ) -> Result<(), ObjectError>;
    /// An automation's runs, newest first, from before `before` when given.
    async fn automation_runs(
        &mut self,
        automation_id: Uuid,
        before: Option<(chrono::DateTime<chrono::Utc>, Uuid)>,
        limit: u32,
    ) -> Result<Vec<crate::AutomationRun>, ObjectError>;
    /// Removes runs older than `before`.
    async fn expire_automation_runs(
        &mut self,
        before: chrono::DateTime<chrono::Utc>,
    ) -> Result<u64, ObjectError>;
    /// Every record id of an object, for reindexing after a field change.
    async fn record_ids(&mut self, object_id: Uuid) -> Result<Vec<Uuid>, ObjectError>;
    /// Locks an object's definition row until the transaction ends:
    /// shared while inserting its records, exclusive while adding a
    /// master-detail field, so neither misses the other (ADR 0025).
    async fn lock_object(&mut self, object_id: Uuid, exclusive: bool) -> Result<(), ObjectError>;
    /// Locks a record's row until the transaction ends: shared while a new
    /// detail points to it, exclusive while it is deleted (ADR 0025).
    async fn lock_record(&mut self, record_id: Uuid, exclusive: bool) -> Result<(), ObjectError>;
    /// The number of active master-detail and roll-up summary fields on an
    /// object: the fields a record insert must not miss (ADR 0025, 0026).
    async fn guarded_field_count(&mut self, object_id: Uuid) -> Result<usize, ObjectError>;
    /// The value a roll-up summary has for one master (ADR 0026): over the
    /// master's details outside the recycle bin, from the summarised
    /// field's index rows. `None` when a minimum or maximum has no value.
    async fn rollup_value(
        &mut self,
        query: &RollupQuery,
    ) -> Result<Option<crate::IndexValue>, ObjectError>;
    /// Every saved report of the workspace (ADR 0027).
    async fn reports(&mut self) -> Result<Vec<crate::Report>, ObjectError>;
    async fn report(&mut self, id: Uuid) -> Result<Option<crate::Report>, ObjectError>;
    async fn insert_report(&mut self, report: &crate::Report) -> Result<(), ObjectError>;
    /// Writes a report if it is still at `expected_version`.
    async fn update_report(
        &mut self,
        report: &crate::Report,
        expected_version: i64,
    ) -> Result<bool, ObjectError>;
    /// Removes a report if it is still at `expected_version`.
    async fn delete_report(&mut self, id: Uuid, expected_version: i64)
        -> Result<bool, ObjectError>;
    /// Whether the object has any record, the recycle bin included.
    async fn has_records(&mut self, object_id: Uuid) -> Result<bool, ObjectError>;
    /// `(detail object, master object)` for every active master-detail
    /// field (ADR 0025).
    async fn master_links(&mut self) -> Result<Vec<(Uuid, Uuid)>, ObjectError>;
    /// Marks a record as deleted along with `root`, the master whose
    /// deletion took it to the recycle bin, or clears the mark.
    async fn set_deleted_with(
        &mut self,
        record_id: Uuid,
        root: Option<Uuid>,
    ) -> Result<(), ObjectError>;
    /// Records in the recycle bin because `root` was deleted.
    async fn deleted_with(&mut self, root: Uuid) -> Result<Vec<Uuid>, ObjectError>;
    /// Whether the object's stored formula values have been built since
    /// values were first stored (ADR 0021 §6).
    async fn formulas_indexed(&mut self, object_id: Uuid) -> Result<bool, ObjectError>;
    /// Records that the object's index, stored formula values included, was
    /// just rebuilt.
    async fn mark_formulas_indexed(&mut self, object_id: Uuid) -> Result<(), ObjectError>;

    async fn append_evidence(
        &mut self,
        audit: &AuditEvent,
        event: &EventEnvelope,
    ) -> Result<(), ObjectError>;
    /// The server version a replica record last took (ADR 0024 §3), or
    /// `None` when the server has not sent it. Replicas run on SQLite only.
    async fn remote_version(
        &mut self,
        record_id: Uuid,
    ) -> Result<Option<RemoteVersion>, ObjectError>;

    async fn set_remote_version(
        &mut self,
        record_id: Uuid,
        remote: RemoteVersion,
    ) -> Result<(), ObjectError>;

    /// Remembers a replica record field's value from before the member
    /// first changed it since the replica took the server's copy (ADR 0024
    /// §3), for a three-way merge. The first value kept wins.
    async fn remember_base(
        &mut self,
        record_id: Uuid,
        field: &str,
        value: &serde_json::Value,
    ) -> Result<(), ObjectError>;

    /// The remembered values of the fields the member changed, by field.
    async fn replica_bases(
        &mut self,
        record_id: Uuid,
    ) -> Result<BTreeMap<String, serde_json::Value>, ObjectError>;

    /// Forgets them, once the replica takes the server's copy.
    async fn clear_replica_bases(&mut self, record_id: Uuid) -> Result<(), ObjectError>;

    /// Where the replica's sync stands; all zero and `None` before it has
    /// pushed or pulled.
    async fn sync_positions(&mut self) -> Result<SyncPositions, ObjectError>;

    async fn set_sync_positions(&mut self, positions: SyncPositions) -> Result<(), ObjectError>;

    /// The replica's offline lease; empty before a limit or contact is set.
    async fn replica_lease(&mut self) -> Result<ReplicaLease, ObjectError>;

    async fn set_replica_lease(&mut self, lease: ReplicaLease) -> Result<(), ObjectError>;

    /// The members and roles the replica last took; empty before the first.
    async fn replica_directory(&mut self) -> Result<ReplicaDirectory, ObjectError>;

    /// Replaces them with the server's.
    async fn replace_replica_directory(
        &mut self,
        directory: &ReplicaDirectory,
    ) -> Result<(), ObjectError>;

    /// The live records a replica holds as the server sent them, as
    /// `(object API name, public id)`, oldest first: what a device asks the
    /// server about for its revocation set (ADR 0024 §4). Records made on the
    /// device and not pushed yet are left out; records of objects the
    /// replica retired are kept in, so losing access to a whole object
    /// erases its records too.
    async fn replica_manifest(&mut self) -> Result<Vec<(String, String)>, ObjectError>;

    /// Removes a record from a replica with everything kept about it, and
    /// leaves no trace in the change log: the member can no longer read it
    /// on the server.
    async fn forget_record(&mut self, record_id: Uuid) -> Result<(), ObjectError>;

    /// Remembers that a replica record's lookup points at a record the
    /// replica does not hold yet.
    async fn add_pending_reference(
        &mut self,
        record_id: Uuid,
        target_public_id: &str,
    ) -> Result<(), ObjectError>;

    /// The replica records waiting for `target_public_id`, forgotten as they
    /// are returned.
    async fn take_pending_references(
        &mut self,
        target_public_id: &str,
    ) -> Result<Vec<Uuid>, ObjectError>;

    async fn commit(self: Box<Self>) -> Result<(), ObjectError>;
}
