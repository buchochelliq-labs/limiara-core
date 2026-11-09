//! The SQLite object store: the same tables in each workspace's own file
//! (ADR 0013). The file is the tenant boundary; every row still carries
//! `tenant_id` and every query filters on it.

use super::{ObjectStore, ObjectTx};
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
use platform_events::EventEnvelope;
use platform_storage::sqlite::{
    append_audit, append_outbox, parse_timestamp, timestamp, SqliteStoreError,
    SqliteTenantDatabases, TenantMigration,
};
use serde_json::Value;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// Engine tables in every workspace file.
pub const SQLITE_MIGRATIONS: &[TenantMigration] = &[
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0001_object_engine",
        sql: r#"
CREATE TABLE object_definitions (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  api_name TEXT NOT NULL,
  label TEXT NOT NULL,
  plural_label TEXT NOT NULL,
  description TEXT,
  origin TEXT NOT NULL CHECK (origin IN ('workspace', 'plugin')),
  owner_plugin TEXT,
  key_prefix TEXT NOT NULL,
  name_label TEXT NOT NULL,
  sharing_model TEXT NOT NULL CHECK (sharing_model IN ('private', 'read', 'read_write')),
  searchable INTEGER NOT NULL,
  in_navigation INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active', 'deleted')),
  record_sequence INTEGER NOT NULL DEFAULT 0,
  version INTEGER NOT NULL CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  deleted_at TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, api_name),
  UNIQUE (tenant_id, key_prefix)
);
CREATE TABLE field_definitions (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  api_name TEXT NOT NULL,
  label TEXT NOT NULL,
  description TEXT,
  field_type TEXT NOT NULL,
  origin TEXT NOT NULL,
  required INTEGER NOT NULL,
  unique_value INTEGER NOT NULL,
  indexed INTEGER NOT NULL,
  searchable INTEGER NOT NULL,
  max_length INTEGER,
  scale INTEGER,
  options TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(options)),
  default_value TEXT CHECK (default_value IS NULL OR json_valid(default_value)),
  reference_object_id TEXT,
  delete_behaviour TEXT,
  position INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active', 'deleted')),
  version INTEGER NOT NULL CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  deleted_at TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, object_id, api_name)
);
CREATE INDEX field_definitions_object_idx ON field_definitions (tenant_id, object_id, position);
CREATE TABLE object_permissions (
  tenant_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  role_id TEXT NOT NULL,
  can_read INTEGER NOT NULL,
  can_create INTEGER NOT NULL,
  can_edit INTEGER NOT NULL,
  can_delete INTEGER NOT NULL,
  view_all INTEGER NOT NULL,
  modify_all INTEGER NOT NULL,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  PRIMARY KEY (tenant_id, object_id, role_id)
);
CREATE TABLE records (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  public_id TEXT NOT NULL,
  name TEXT NOT NULL,
  name_key TEXT NOT NULL,
  owner_id TEXT,
  record_type_id TEXT,
  data TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(data) AND json_type(data) = 'object'),
  version INTEGER NOT NULL CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  touched_at TEXT NOT NULL,
  archived_at TEXT,
  deleted_at TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, public_id)
);
CREATE INDEX records_object_name_idx ON records (tenant_id, object_id, name_key, id);
CREATE INDEX records_object_created_idx ON records (tenant_id, object_id, created_at, id);
CREATE INDEX records_object_updated_idx ON records (tenant_id, object_id, updated_at, id);
CREATE INDEX records_object_owner_idx ON records (tenant_id, object_id, owner_id);
CREATE INDEX records_touched_idx ON records (tenant_id, touched_at, id);
CREATE TABLE record_index (
  tenant_id TEXT NOT NULL,
  field_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  text_value TEXT,
  number_value REAL,
  time_value TEXT,
  CHECK ((text_value IS NOT NULL) + (number_value IS NOT NULL) + (time_value IS NOT NULL) = 1)
);
CREATE INDEX record_index_text_idx ON record_index (tenant_id, field_id, text_value, record_id) WHERE text_value IS NOT NULL;
CREATE INDEX record_index_number_idx ON record_index (tenant_id, field_id, number_value, record_id) WHERE number_value IS NOT NULL;
CREATE INDEX record_index_time_idx ON record_index (tenant_id, field_id, time_value, record_id) WHERE time_value IS NOT NULL;
CREATE INDEX record_index_record_idx ON record_index (tenant_id, record_id);
CREATE TABLE record_unique (
  tenant_id TEXT NOT NULL,
  field_id TEXT NOT NULL,
  value_key TEXT NOT NULL,
  record_id TEXT NOT NULL,
  PRIMARY KEY (tenant_id, field_id, value_key)
);
CREATE INDEX record_unique_record_idx ON record_unique (tenant_id, record_id);
CREATE TABLE record_references (
  tenant_id TEXT NOT NULL,
  field_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  target_id TEXT NOT NULL,
  PRIMARY KEY (tenant_id, field_id, record_id)
);
CREATE INDEX record_references_target_idx ON record_references (tenant_id, target_id, field_id, record_id);
"#,
    },
    // Record ownership and sharing (ADR 0016). Reporting lines live in the
    // kernel, which passes each caller's reports to the engine.
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0002_record_sharing",
        sql: r#"
ALTER TABLE object_definitions ADD COLUMN grant_hierarchy INTEGER NOT NULL DEFAULT 1 CHECK (grant_hierarchy IN (0, 1));
CREATE TABLE record_shares (
  tenant_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  grantee_kind TEXT NOT NULL CHECK (grantee_kind IN ('user', 'role')),
  grantee_id TEXT NOT NULL,
  access TEXT NOT NULL CHECK (access IN ('read', 'edit')),
  reason TEXT NOT NULL DEFAULT 'manual' CHECK (reason IN ('manual')),
  created_at TEXT NOT NULL,
  created_by TEXT,
  PRIMARY KEY (tenant_id, record_id, grantee_kind, grantee_id)
);
CREATE INDEX record_shares_grantee_idx ON record_shares (tenant_id, grantee_kind, grantee_id, record_id);
"#,
    },
    // Field history tracking (ADR 0017).
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0003_field_history",
        sql: r#"
ALTER TABLE object_definitions ADD COLUMN track_history INTEGER NOT NULL DEFAULT 0 CHECK (track_history IN (0, 1));
ALTER TABLE field_definitions ADD COLUMN track_history INTEGER NOT NULL DEFAULT 0 CHECK (track_history IN (0, 1));
CREATE TABLE record_history (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('created', 'changed', 'deleted', 'undeleted')),
  field TEXT,
  old_value TEXT CHECK (old_value IS NULL OR json_valid(old_value)),
  new_value TEXT CHECK (new_value IS NULL OR json_valid(new_value)),
  changed_at TEXT NOT NULL,
  changed_by TEXT,
  correlation_id TEXT,
  PRIMARY KEY (tenant_id, id),
  CHECK ((kind = 'changed') = (field IS NOT NULL))
);
CREATE INDEX record_history_record_idx ON record_history (tenant_id, record_id, changed_at DESC, id DESC);
-- The store only ever inserts history. Unlike PostgreSQL there is no
-- separate application role to revoke from: the file belongs to the
-- workspace's process.
"#,
    },
    // Change data capture and incremental sync (ADR 0018). SQLite has one
    // writer at a time, so positions follow commit order by construction.
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0004_change_data_capture",
        sql: r#"
CREATE TABLE change_counters (
  tenant_id TEXT PRIMARY KEY,
  last_position INTEGER NOT NULL DEFAULT 0 CHECK (last_position >= 0),
  expired_through INTEGER NOT NULL DEFAULT 0 CHECK (expired_through >= 0)
);
CREATE TABLE record_changes (
  tenant_id TEXT NOT NULL,
  position INTEGER NOT NULL CHECK (position >= 1),
  record_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  public_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('created', 'updated', 'deleted', 'undeleted')),
  fields TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(fields) AND json_type(fields) = 'array'),
  changed_at TEXT NOT NULL,
  changed_by TEXT,
  correlation_id TEXT,
  PRIMARY KEY (tenant_id, position)
);
CREATE INDEX record_changes_time_idx ON record_changes (tenant_id, changed_at, position);
CREATE INDEX records_object_touched_idx ON records (tenant_id, object_id, touched_at, id);
"#,
    },
    // Global search (ADR 0020): terms compare as bytes, like PostgreSQL's
    // COLLATE "C", so prefix ranges behave the same on both backends.
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0005_record_search",
        sql: r#"
CREATE TABLE record_search_terms (
  tenant_id TEXT NOT NULL,
  term TEXT NOT NULL COLLATE BINARY CHECK (length(term) BETWEEN 1 AND 40),
  record_id TEXT NOT NULL,
  weight INTEGER NOT NULL CHECK (weight BETWEEN 1 AND 3),
  PRIMARY KEY (tenant_id, term, record_id)
);
CREATE INDEX record_search_terms_record_idx ON record_search_terms (tenant_id, record_id);
"#,
    },
    // Validation rules (ADR 0021).
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0006_validation_rules",
        sql: r#"
CREATE TABLE validation_rules (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 63),
  description TEXT CHECK (length(description) <= 1000),
  condition TEXT NOT NULL CHECK (length(condition) BETWEEN 1 AND 3900),
  message TEXT NOT NULL CHECK (length(message) BETWEEN 1 AND 255),
  error_field TEXT,
  active INTEGER NOT NULL DEFAULT 1,
  version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, object_id, name)
);
"#,
    },
    // Formula fields (ADR 0021): computed when read, never stored.
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0007_formula_fields",
        sql: r#"
ALTER TABLE field_definitions ADD COLUMN formula TEXT CHECK (formula IS NULL OR length(formula) BETWEEN 1 AND 3900);
ALTER TABLE field_definitions ADD COLUMN formula_type TEXT;
"#,
    },
    // Record-triggered automations (ADR 0022).
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0008_automations",
        sql: r#"
CREATE TABLE automations (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 63),
  description TEXT CHECK (length(description) <= 1000),
  trigger TEXT NOT NULL CHECK (trigger IN ('create', 'update', 'create_or_update')),
  timing TEXT NOT NULL CHECK (timing IN ('before_save', 'after_save')),
  condition TEXT CHECK (length(condition) BETWEEN 1 AND 3900),
  actions TEXT NOT NULL CHECK (json_valid(actions) AND json_type(actions) = 'array'),
  active INTEGER NOT NULL DEFAULT 1,
  version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, object_id, name)
);
"#,
    },
    // Automation run history and triggers on delete and restore (ADR 0022 §5).
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0009_automation_runs",
        sql: r#"
CREATE TABLE automations_next (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 63),
  description TEXT CHECK (length(description) <= 1000),
  trigger TEXT NOT NULL CHECK (trigger IN ('create', 'update', 'create_or_update', 'delete', 'undelete')),
  timing TEXT NOT NULL CHECK (timing IN ('before_save', 'after_save')),
  condition TEXT CHECK (length(condition) BETWEEN 1 AND 3900),
  actions TEXT NOT NULL CHECK (json_valid(actions) AND json_type(actions) = 'array'),
  active INTEGER NOT NULL DEFAULT 1,
  version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, object_id, name)
);
INSERT INTO automations_next SELECT tenant_id, id, object_id, name, description, trigger, timing,
  condition, actions, active, version, created_at, created_by, updated_at, updated_by FROM automations;
DROP TABLE automations;
ALTER TABLE automations_next RENAME TO automations;
CREATE TABLE automation_runs (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  automation_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  record TEXT NOT NULL CHECK (length(record) BETWEEN 1 AND 40),
  event TEXT NOT NULL CHECK (event IN ('create', 'update', 'delete', 'undelete')),
  outcome TEXT NOT NULL CHECK (outcome IN ('succeeded', 'failed')),
  detail TEXT CHECK (length(detail) <= 2000),
  saves INTEGER NOT NULL DEFAULT 0 CHECK (saves >= 0),
  ran_at TEXT NOT NULL,
  ran_by TEXT,
  correlation_id TEXT,
  PRIMARY KEY (tenant_id, id)
);
CREATE INDEX automation_runs_automation_idx ON automation_runs (tenant_id, automation_id, ran_at, id);
CREATE INDEX automation_runs_ran_at_idx ON automation_runs (tenant_id, ran_at);
"#,
    },
    // Background and scheduled automations (ADR 0022 §6).
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0010_automation_jobs",
        sql: r#"
CREATE TABLE automations_next (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 63),
  description TEXT CHECK (length(description) <= 1000),
  trigger TEXT NOT NULL CHECK (trigger IN ('create', 'update', 'create_or_update', 'delete', 'undelete')),
  timing TEXT NOT NULL CHECK (timing IN ('before_save', 'after_save', 'async', 'scheduled')),
  condition TEXT CHECK (length(condition) BETWEEN 1 AND 3900),
  actions TEXT NOT NULL CHECK (json_valid(actions) AND json_type(actions) = 'array'),
  schedule TEXT CHECK (schedule IS NULL OR (json_valid(schedule) AND json_type(schedule) = 'object')),
  active INTEGER NOT NULL DEFAULT 1,
  version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, object_id, name)
);
INSERT INTO automations_next SELECT tenant_id, id, object_id, name, description, trigger, timing,
  condition, actions, NULL, active, version, created_at, created_by, updated_at, updated_by FROM automations;
DROP TABLE automations;
ALTER TABLE automations_next RENAME TO automations;
CREATE TABLE automation_jobs (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  automation_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  record TEXT NOT NULL CHECK (length(record) BETWEEN 1 AND 40),
  event TEXT NOT NULL CHECK (event IN ('create', 'update', 'delete', 'undelete')),
  scheduled INTEGER NOT NULL,
  due_at TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'running', 'succeeded', 'failed', 'cancelled')),
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  locked_until TEXT,
  last_error TEXT CHECK (length(last_error) <= 2000),
  depth INTEGER NOT NULL DEFAULT 0 CHECK (depth >= 0),
  actor_id TEXT NOT NULL,
  correlation_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id)
);
CREATE UNIQUE INDEX automation_jobs_scheduled_idx ON automation_jobs (tenant_id, automation_id, record_id)
  WHERE scheduled AND status = 'pending';
CREATE INDEX automation_jobs_automation_idx ON automation_jobs (tenant_id, automation_id, created_at);
CREATE INDEX automation_jobs_due_idx ON automation_jobs (tenant_id, due_at) WHERE status IN ('pending', 'running');
"#,
    },
    // Bulk jobs (ADR 0023 §2).
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0011_bulk_jobs",
        sql: r#"
CREATE TABLE bulk_jobs (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object TEXT NOT NULL CHECK (length(object) BETWEEN 1 AND 128),
  operation TEXT NOT NULL CHECK (operation IN ('insert', 'update', 'delete')),
  status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'completed', 'aborted')),
  total INTEGER NOT NULL CHECK (total >= 0),
  succeeded INTEGER NOT NULL DEFAULT 0 CHECK (succeeded >= 0),
  failed INTEGER NOT NULL DEFAULT 0 CHECK (failed >= 0),
  actor_id TEXT NOT NULL,
  correlation_id TEXT NOT NULL,
  locked_until TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  completed_at TEXT,
  PRIMARY KEY (tenant_id, id)
);
CREATE INDEX bulk_jobs_due_idx ON bulk_jobs (tenant_id, updated_at) WHERE status IN ('queued', 'running');
CREATE TABLE bulk_job_rows (
  tenant_id TEXT NOT NULL,
  job_id TEXT NOT NULL,
  row_index INTEGER NOT NULL CHECK (row_index >= 0),
  input TEXT NOT NULL CHECK (json_valid(input) AND json_type(input) = 'object'),
  status TEXT NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
  record TEXT,
  errors TEXT CHECK (errors IS NULL OR json_valid(errors)),
  PRIMARY KEY (tenant_id, job_id, row_index)
);
CREATE INDEX bulk_job_rows_pending_idx ON bulk_job_rows (tenant_id, job_id, row_index) WHERE status = 'pending';
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0012_formula_index_marks",
        sql: r#"
CREATE TABLE formula_index_marks (
  tenant_id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  indexed_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, object_id)
);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0013_master_detail",
        sql: r#"
ALTER TABLE records ADD COLUMN deleted_with TEXT;
CREATE INDEX records_deleted_with_idx ON records (tenant_id, deleted_with) WHERE deleted_with IS NOT NULL;
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0014_rollup_summaries",
        sql: r#"
ALTER TABLE field_definitions ADD COLUMN rollup_function TEXT
  CHECK (rollup_function IS NULL OR rollup_function IN ('count', 'sum', 'min', 'max'));
ALTER TABLE field_definitions ADD COLUMN rollup_relationship_id TEXT;
ALTER TABLE field_definitions ADD COLUMN rollup_field_id TEXT;
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0015_reports",
        sql: r#"
CREATE TABLE reports (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 80),
  description TEXT CHECK (description IS NULL OR length(description) <= 1000),
  query TEXT NOT NULL CHECK (length(query) BETWEEN 1 AND 10000),
  chart TEXT NOT NULL CHECK (chart IN ('table', 'bar', 'line', 'pie', 'metric')),
  label_column TEXT,
  value_column TEXT,
  visibility TEXT NOT NULL CHECK (visibility IN ('private', 'workspace')),
  owner_id TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  PRIMARY KEY (tenant_id, id)
);
CREATE INDEX reports_visibility_idx ON reports (tenant_id, visibility, owner_id);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0016_lookup_filters",
        sql: r#"
ALTER TABLE field_definitions ADD COLUMN lookup_filter TEXT
  CHECK (lookup_filter IS NULL OR length(lookup_filter) BETWEEN 1 AND 1000);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0017_metadata_version",
        sql: r#"
ALTER TABLE change_counters ADD COLUMN metadata_version INTEGER NOT NULL DEFAULT 0
  CHECK (metadata_version >= 0);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0018_replica",
        sql: r#"
-- A synced device's replica (ADR 0024 §3): the server version each record
-- last took, so a local edit shows as the record's version moving past it,
-- and lookups waiting for a target the replica does not hold yet.
CREATE TABLE replica_versions (
  tenant_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  version INTEGER NOT NULL CHECK (version >= 1),
  PRIMARY KEY (tenant_id, record_id)
);
CREATE TABLE replica_pending_references (
  tenant_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  target_public_id TEXT NOT NULL,
  PRIMARY KEY (tenant_id, record_id, target_public_id)
);
CREATE INDEX replica_pending_references_target_idx
  ON replica_pending_references (tenant_id, target_public_id);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0019_replica_sync",
        sql: r#"
-- Whether the server had a replica record in its recycle bin, so a restore
-- on the device pushes as one, and where the replica's push and pull stand.
ALTER TABLE replica_versions ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0
  CHECK (deleted IN (0, 1));
CREATE TABLE replica_sync (
  tenant_id TEXT NOT NULL PRIMARY KEY,
  pushed_position INTEGER NOT NULL CHECK (pushed_position >= 0),
  pulled_position INTEGER CHECK (pulled_position >= 0),
  metadata_version INTEGER CHECK (metadata_version >= 0)
);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0020_replica_through",
        sql: r#"
-- The device's change-log position each record took the server's copy at:
-- the member's changes after it are what the record still has to send.
ALTER TABLE replica_versions ADD COLUMN through_position INTEGER NOT NULL DEFAULT 0
  CHECK (through_position >= 0);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0021_replica_bases",
        sql: r#"
-- The value each field had in the server's copy before the member first
-- changed it, for a three-way merge when the server changed the record too.
-- Only changed fields of records with unpushed edits are kept.
CREATE TABLE replica_bases (
  tenant_id TEXT NOT NULL,
  record_id TEXT NOT NULL,
  field TEXT NOT NULL,
  value TEXT NOT NULL,
  PRIMARY KEY (tenant_id, record_id, field)
);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0022_replica_lease",
        sql: r#"
-- The replica's offline lease (ADR 0024 §4): the limit the server set and
-- when the device last reached the server. Past the limit, the replica
-- refuses the member until the next contact.
ALTER TABLE replica_sync ADD COLUMN offline_limit_seconds INTEGER
  CHECK (offline_limit_seconds IS NULL OR offline_limit_seconds > 0);
ALTER TABLE replica_sync ADD COLUMN last_contact_at TEXT;
-- The latest device time seen, so turning the clock back cannot reopen a
-- lease that has run out.
ALTER TABLE replica_sync ADD COLUMN clock_seen_at TEXT;
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0023_replica_directory",
        sql: r#"
-- The workspace's members and roles as the replica last took them (ADR
-- 0024, "Offline directory"): names only, so records can name people and
-- roles offline. Replaced whole at each sync.
CREATE TABLE replica_members (
  tenant_id TEXT NOT NULL,
  user_id TEXT NOT NULL,
  name TEXT NOT NULL,
  PRIMARY KEY (tenant_id, user_id)
);
CREATE TABLE replica_roles (
  tenant_id TEXT NOT NULL,
  role_id TEXT NOT NULL,
  name TEXT NOT NULL,
  PRIMARY KEY (tenant_id, role_id)
);
"#,
    },
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0024_replica_trusted_contact",
        sql: r#"
-- The device's monotonic clock at the last contact (ADR 0024, "Trusted
-- elapsed time"): the boot it belongs to and the milliseconds since it.
ALTER TABLE replica_sync ADD COLUMN contact_boot TEXT;
ALTER TABLE replica_sync ADD COLUMN contact_since_boot_ms INTEGER
  CHECK (contact_since_boot_ms IS NULL OR contact_since_boot_ms >= 0);
"#,
    },
    // Quick actions (ADR 0032): per-object shortcuts to the record forms.
    TenantMigration {
        owner: ENGINE_CONTEXT,
        id: "0025_quick_actions",
        sql: r#"
CREATE TABLE quick_actions (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  object_id TEXT NOT NULL,
  name TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 63),
  label TEXT NOT NULL CHECK (length(label) BETWEEN 1 AND 80),
  kind TEXT NOT NULL CHECK (kind IN ('update', 'create')),
  position INTEGER NOT NULL CHECK (position >= 0),
  target_object_id TEXT,
  target_field TEXT,
  fields TEXT NOT NULL CHECK (json_valid(fields) AND json_type(fields) = 'array'
    AND json_array_length(fields) BETWEEN 1 AND 20),
  presets TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(presets) AND json_type(presets) = 'object'),
  version INTEGER NOT NULL DEFAULT 1 CHECK (version >= 1),
  created_at TEXT NOT NULL,
  created_by TEXT,
  updated_at TEXT NOT NULL,
  updated_by TEXT,
  CHECK ((kind = 'create') = (target_object_id IS NOT NULL AND target_field IS NOT NULL)),
  CHECK ((target_object_id IS NULL) = (target_field IS NULL)),
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, object_id, name)
);
CREATE INDEX quick_actions_target_idx ON quick_actions (tenant_id, target_object_id)
  WHERE target_object_id IS NOT NULL;
"#,
    },
];

#[derive(Clone)]
pub struct SqliteObjectStore {
    databases: Arc<SqliteTenantDatabases>,
}

impl SqliteObjectStore {
    /// `databases` must have been created with [`SQLITE_MIGRATIONS`].
    pub fn new(databases: Arc<SqliteTenantDatabases>) -> Self {
        Self { databases }
    }
}

fn storage(error: SqliteStoreError) -> ObjectError {
    match error {
        SqliteStoreError::Database(error) => ObjectError::Database(error),
        SqliteStoreError::MigrationDrift { .. } => ObjectError::Invariant,
        // The device keystore is locked or holds another key: the replica
        // cannot be opened until it unlocks (ADR 0024, "Encryption at rest").
        SqliteStoreError::Key => ObjectError::Conflict("storage.key_unavailable"),
        SqliteStoreError::Io(error) => ObjectError::Database(sqlx::Error::Io(error)),
    }
}

#[async_trait]
impl ObjectStore for SqliteObjectStore {
    async fn begin(&self, tenant_id: Uuid) -> Result<Box<dyn ObjectTx>, ObjectError> {
        let tx = self.databases.begin(tenant_id).await.map_err(storage)?;
        Ok(Box::new(SqliteObjectTx {
            tx,
            tenant: tenant_id.to_string(),
            tenant_id,
        }))
    }
}

struct SqliteObjectTx {
    tx: Transaction<'static, Sqlite>,
    tenant: String,
    tenant_id: Uuid,
}

impl SqliteObjectTx {
    /// Runs a rendered query. UUIDs and times are stored as text.
    async fn run_query(&mut self, rendered: &Rendered) -> Result<Vec<SqliteRow>, ObjectError> {
        let mut query = sqlx::query(&rendered.sql);
        for param in &rendered.params {
            query = match param {
                Param::Text(text) => query.bind(text.clone()),
                Param::Number(number) => query.bind(*number),
                Param::Int(number) => query.bind(*number),
                Param::Time(time) => query.bind(timestamp(*time)),
                Param::Uuid(id) => query.bind(id.to_string()),
            };
        }
        Ok(query.fetch_all(&mut *self.tx).await?)
    }
}

const OBJECT_COLUMNS: &str =
    "id, api_name, label, plural_label, description, origin, owner_plugin, \
     key_prefix, name_label, sharing_model, searchable, in_navigation, status, version, \
     created_at, created_by, updated_at, updated_by, grant_hierarchy, track_history";
const FIELD_COLUMNS: &str = "f.id, f.object_id, f.api_name, f.label, f.description, f.field_type, \
     f.origin, f.required, f.unique_value, f.indexed, f.searchable, f.max_length, f.scale, \
     f.options, f.default_value, f.reference_object_id, r.api_name AS reference_api, \
     f.delete_behaviour, f.position, f.status, f.version, f.created_at, f.created_by, \
     f.updated_at, f.updated_by, f.track_history, f.formula, f.formula_type, f.lookup_filter, \
     f.rollup_function, f.rollup_relationship_id, f.rollup_field_id, \
     ro.api_name AS rollup_object, rr.api_name AS rollup_relationship, rf.api_name AS rollup_field";
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

fn uuid(row: &SqliteRow, column: &str) -> Result<Uuid, ObjectError> {
    Uuid::parse_str(&row.try_get::<String, _>(column)?).map_err(|_| ObjectError::Invariant)
}

fn optional_uuid(row: &SqliteRow, column: &str) -> Result<Option<Uuid>, ObjectError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|value| Uuid::parse_str(&value).map_err(|_| ObjectError::Invariant))
        .transpose()
}

fn time(row: &SqliteRow, column: &str) -> Result<DateTime<Utc>, ObjectError> {
    parse(parse_timestamp(&row.try_get::<String, _>(column)?))
}

fn optional_time(row: &SqliteRow, column: &str) -> Result<Option<DateTime<Utc>>, ObjectError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|value| parse(parse_timestamp(&value)))
        .transpose()
}

fn flag(row: &SqliteRow, column: &str) -> Result<bool, ObjectError> {
    Ok(row.try_get::<i64, _>(column)? != 0)
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, ObjectError> {
    serde_json::to_string(value).map_err(|_| ObjectError::Invariant)
}

fn text(id: Option<Uuid>) -> Option<String> {
    id.map(|id| id.to_string())
}

fn conflict_on_unique(error: sqlx::Error, code: &'static str) -> ObjectError {
    if ObjectError::is_unique_violation(&error) {
        ObjectError::Conflict(code)
    } else {
        ObjectError::Database(error)
    }
}

fn object_from_row(row: &SqliteRow) -> Result<ObjectDefinition, ObjectError> {
    Ok(ObjectDefinition {
        id: uuid(row, "id")?,
        api_name: row.try_get("api_name")?,
        label: row.try_get("label")?,
        plural_label: row.try_get("plural_label")?,
        description: row.try_get("description")?,
        origin: parse(Origin::parse(&row.try_get::<String, _>("origin")?))?,
        owner_plugin: row.try_get("owner_plugin")?,
        key_prefix: row.try_get("key_prefix")?,
        name_label: row.try_get("name_label")?,
        sharing_model: parse(SharingModel::parse(
            &row.try_get::<String, _>("sharing_model")?,
        ))?,
        grant_hierarchy: flag(row, "grant_hierarchy")?,
        track_history: flag(row, "track_history")?,
        searchable: flag(row, "searchable")?,
        in_navigation: flag(row, "in_navigation")?,
        status: parse(DefinitionStatus::parse(
            &row.try_get::<String, _>("status")?,
        ))?,
        version: row.try_get("version")?,
        created_at: time(row, "created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time(row, "updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
        fields: Vec::new(),
    })
}

const REPORT_COLUMNS: &str = "id, name, description, query, chart, label_column, value_column, \
     visibility, owner_id, version, created_at, created_by, updated_at, updated_by";

fn report_from_row(row: &SqliteRow) -> Result<crate::Report, ObjectError> {
    Ok(crate::Report {
        id: uuid(row, "id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        query: row.try_get("query")?,
        chart: parse(crate::ReportChart::parse(
            &row.try_get::<String, _>("chart")?,
        ))?,
        label_column: row.try_get("label_column")?,
        value_column: row.try_get("value_column")?,
        visibility: parse(crate::ReportVisibility::parse(
            &row.try_get::<String, _>("visibility")?,
        ))?,
        owner_id: uuid(row, "owner_id")?,
        version: row.try_get("version")?,
        created_at: time(row, "created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time(row, "updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
    })
}

fn field_from_row(row: &SqliteRow) -> Result<FieldDefinition, ObjectError> {
    let options: Vec<PicklistOption> = serde_json::from_str(&row.try_get::<String, _>("options")?)
        .map_err(|_| ObjectError::Invariant)?;
    let default_value = row
        .try_get::<Option<String>, _>("default_value")?
        .map(|value| serde_json::from_str::<Value>(&value).map_err(|_| ObjectError::Invariant))
        .transpose()?;
    let delete_behaviour: Option<String> = row.try_get("delete_behaviour")?;
    Ok(FieldDefinition {
        id: uuid(row, "id")?,
        object_id: uuid(row, "object_id")?,
        api_name: row.try_get("api_name")?,
        label: row.try_get("label")?,
        description: row.try_get("description")?,
        field_type: parse(FieldType::parse(&row.try_get::<String, _>("field_type")?))?,
        origin: parse(Origin::parse(&row.try_get::<String, _>("origin")?))?,
        required: flag(row, "required")?,
        unique: flag(row, "unique_value")?,
        indexed: flag(row, "indexed")?,
        searchable: flag(row, "searchable")?,
        track_history: flag(row, "track_history")?,
        max_length: row.try_get("max_length")?,
        scale: row.try_get("scale")?,
        options,
        default_value,
        reference_object_id: optional_uuid(row, "reference_object_id")?,
        reference_object: row.try_get("reference_api")?,
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
                relationship_id: parse(optional_uuid(row, "rollup_relationship_id")?)?,
                field_id: optional_uuid(row, "rollup_field_id")?,
                object: parse(row.try_get("rollup_object")?)?,
                relationship: parse(row.try_get("rollup_relationship")?)?,
                field: row.try_get("rollup_field")?,
            }),
            None => None,
        },
        position: row.try_get("position")?,
        status: parse(DefinitionStatus::parse(
            &row.try_get::<String, _>("status")?,
        ))?,
        version: row.try_get("version")?,
        created_at: time(row, "created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time(row, "updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
    })
}

const RULE_COLUMNS: &str = "id, object_id, name, description, condition, message, error_field, \
     active, version, created_at, created_by, updated_at, updated_by";

const AUTOMATION_COLUMNS: &str = "id, object_id, name, description, trigger, timing, condition, \
     actions, schedule, active, version, created_at, created_by, updated_at, updated_by";

fn automation_from_row(row: &SqliteRow) -> Result<crate::Automation, ObjectError> {
    let uuid = |column: &str| -> Result<Uuid, ObjectError> {
        Uuid::parse_str(&row.try_get::<String, _>(column)?).map_err(|_| ObjectError::Invariant)
    };
    let time = |column: &str| -> Result<DateTime<Utc>, ObjectError> {
        parse(parse_timestamp(&row.try_get::<String, _>(column)?))
    };
    let actions: String = row.try_get("actions")?;
    Ok(crate::Automation {
        id: uuid("id")?,
        object_id: uuid("object_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        trigger: crate::AutomationTrigger::parse(&row.try_get::<String, _>("trigger")?)
            .ok_or(ObjectError::Invariant)?,
        timing: crate::AutomationTiming::parse(&row.try_get::<String, _>("timing")?)
            .ok_or(ObjectError::Invariant)?,
        condition: row.try_get("condition")?,
        actions: serde_json::from_str(&actions).map_err(|_| ObjectError::Invariant)?,
        schedule: row
            .try_get::<Option<String>, _>("schedule")?
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|_| ObjectError::Invariant)?,
        active: row.try_get("active")?,
        version: row.try_get("version")?,
        created_at: time("created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time("updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
    })
}

const RUN_COLUMNS: &str = "id, automation_id, object_id, record, event, outcome, detail, saves, \
     ran_at, ran_by, correlation_id";

fn run_from_row(row: &SqliteRow) -> Result<crate::AutomationRun, ObjectError> {
    let uuid = |column: &str| -> Result<Uuid, ObjectError> {
        Uuid::parse_str(&row.try_get::<String, _>(column)?).map_err(|_| ObjectError::Invariant)
    };
    Ok(crate::AutomationRun {
        id: uuid("id")?,
        automation_id: uuid("automation_id")?,
        object_id: uuid("object_id")?,
        record: row.try_get("record")?,
        event: crate::SaveEvent::parse(&row.try_get::<String, _>("event")?)
            .ok_or(ObjectError::Invariant)?,
        outcome: crate::RunOutcome::parse(&row.try_get::<String, _>("outcome")?)
            .ok_or(ObjectError::Invariant)?,
        detail: row.try_get("detail")?,
        saves: row.try_get("saves")?,
        ran_at: parse(parse_timestamp(&row.try_get::<String, _>("ran_at")?))?,
        ran_by: optional_uuid(row, "ran_by")?,
        correlation_id: optional_uuid(row, "correlation_id")?,
    })
}

fn schedule_text(automation: &crate::Automation) -> Result<Option<String>, ObjectError> {
    automation
        .schedule
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| ObjectError::Invariant)
}

const JOB_COLUMNS: &str = "id, automation_id, object_id, record_id, record, event, scheduled, due_at, \
     status, attempts, locked_until, last_error, depth, actor_id, correlation_id, created_at, updated_at";

fn job_from_row(row: &SqliteRow) -> Result<crate::AutomationJob, ObjectError> {
    let uuid = |column: &str| -> Result<Uuid, ObjectError> {
        Uuid::parse_str(&row.try_get::<String, _>(column)?).map_err(|_| ObjectError::Invariant)
    };
    let time = |column: &str| -> Result<DateTime<Utc>, ObjectError> {
        parse(parse_timestamp(&row.try_get::<String, _>(column)?))
    };
    Ok(crate::AutomationJob {
        id: uuid("id")?,
        automation_id: uuid("automation_id")?,
        object_id: uuid("object_id")?,
        record_id: uuid("record_id")?,
        record: row.try_get("record")?,
        event: crate::SaveEvent::parse(&row.try_get::<String, _>("event")?)
            .ok_or(ObjectError::Invariant)?,
        scheduled: row.try_get("scheduled")?,
        due_at: time("due_at")?,
        status: crate::JobStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or(ObjectError::Invariant)?,
        attempts: row.try_get("attempts")?,
        locked_until: row
            .try_get::<Option<String>, _>("locked_until")?
            .map(|text| parse(parse_timestamp(&text)))
            .transpose()?,
        last_error: row.try_get("last_error")?,
        depth: row.try_get("depth")?,
        actor_id: uuid("actor_id")?,
        correlation_id: uuid("correlation_id")?,
        created_at: time("created_at")?,
        updated_at: time("updated_at")?,
    })
}

const BULK_COLUMNS: &str = "id, object, operation, status, total, succeeded, failed, actor_id, \
     correlation_id, locked_until, created_at, updated_at, completed_at";

fn bulk_from_row(row: &SqliteRow) -> Result<crate::BulkJob, ObjectError> {
    let uuid = |column: &str| -> Result<Uuid, ObjectError> {
        Uuid::parse_str(&row.try_get::<String, _>(column)?).map_err(|_| ObjectError::Invariant)
    };
    let time = |column: &str| -> Result<Option<DateTime<Utc>>, ObjectError> {
        row.try_get::<Option<String>, _>(column)?
            .map(|text| parse(parse_timestamp(&text)))
            .transpose()
    };
    Ok(crate::BulkJob {
        id: uuid("id")?,
        object: row.try_get("object")?,
        operation: crate::BulkOperation::parse(&row.try_get::<String, _>("operation")?)
            .ok_or(ObjectError::Invariant)?,
        status: crate::BulkStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or(ObjectError::Invariant)?,
        total: row.try_get("total")?,
        succeeded: row.try_get("succeeded")?,
        failed: row.try_get("failed")?,
        actor_id: uuid("actor_id")?,
        correlation_id: uuid("correlation_id")?,
        locked_until: time("locked_until")?,
        created_at: time("created_at")?.ok_or(ObjectError::Invariant)?,
        updated_at: time("updated_at")?.ok_or(ObjectError::Invariant)?,
        completed_at: time("completed_at")?,
    })
}

fn bulk_row_from_row(row: &SqliteRow) -> Result<crate::BulkRow, ObjectError> {
    let json = |text: Option<String>| -> Result<Option<Value>, ObjectError> {
        text.map(|text| serde_json::from_str(&text).map_err(|_| ObjectError::Invariant))
            .transpose()
    };
    Ok(crate::BulkRow {
        job_id: Uuid::parse_str(&row.try_get::<String, _>("job_id")?)
            .map_err(|_| ObjectError::Invariant)?,
        index: row.try_get("row_index")?,
        input: json(Some(row.try_get("input")?))?.ok_or(ObjectError::Invariant)?,
        status: crate::BulkRowStatus::parse(&row.try_get::<String, _>("status")?)
            .ok_or(ObjectError::Invariant)?,
        record: row.try_get("record")?,
        errors: json(row.try_get("errors")?)?,
    })
}

fn actions_text(automation: &crate::Automation) -> Result<String, ObjectError> {
    serde_json::to_string(&automation.actions).map_err(|_| ObjectError::Invariant)
}

const QUICK_ACTION_COLUMNS: &str = "q.id, q.object_id, q.name, q.label, q.kind, q.position, \
     q.target_object_id, t.api_name AS target_object, q.target_field, q.fields, q.presets, \
     q.version, q.created_at, q.created_by, q.updated_at, q.updated_by";
const QUICK_ACTION_FROM: &str = "FROM quick_actions q LEFT JOIN object_definitions t \
     ON t.tenant_id = q.tenant_id AND t.id = q.target_object_id";

fn quick_action_from_row(row: &SqliteRow) -> Result<crate::QuickAction, ObjectError> {
    let fields: String = row.try_get("fields")?;
    let presets: String = row.try_get("presets")?;
    Ok(crate::QuickAction {
        id: uuid(row, "id")?,
        object_id: uuid(row, "object_id")?,
        name: row.try_get("name")?,
        label: row.try_get("label")?,
        kind: crate::QuickActionKind::parse(&row.try_get::<String, _>("kind")?)
            .ok_or(ObjectError::Invariant)?,
        position: row.try_get("position")?,
        target_object_id: optional_uuid(row, "target_object_id")?,
        target_object: row.try_get("target_object")?,
        target_field: row.try_get("target_field")?,
        fields: serde_json::from_str(&fields).map_err(|_| ObjectError::Invariant)?,
        presets: serde_json::from_str(&presets).map_err(|_| ObjectError::Invariant)?,
        version: row.try_get("version")?,
        created_at: time(row, "created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time(row, "updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
    })
}

fn rule_from_row(row: &SqliteRow) -> Result<ValidationRule, ObjectError> {
    let uuid = |column: &str| -> Result<Uuid, ObjectError> {
        Uuid::parse_str(&row.try_get::<String, _>(column)?).map_err(|_| ObjectError::Invariant)
    };
    let time = |column: &str| -> Result<DateTime<Utc>, ObjectError> {
        parse(parse_timestamp(&row.try_get::<String, _>(column)?))
    };
    Ok(ValidationRule {
        id: uuid("id")?,
        object_id: uuid("object_id")?,
        name: row.try_get("name")?,
        description: row.try_get("description")?,
        condition: row.try_get("condition")?,
        message: row.try_get("message")?,
        error_field: row.try_get("error_field")?,
        active: row.try_get("active")?,
        version: row.try_get("version")?,
        created_at: time("created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time("updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
    })
}

fn record_from_row(row: &SqliteRow) -> Result<Record, ObjectError> {
    let values: BTreeMap<String, Value> = serde_json::from_str(&row.try_get::<String, _>("data")?)
        .map_err(|_| ObjectError::Invariant)?;
    Ok(Record {
        id: uuid(row, "id")?,
        object_id: uuid(row, "object_id")?,
        public_id: row.try_get("public_id")?,
        name: row.try_get("name")?,
        owner_id: optional_uuid(row, "owner_id")?,
        record_type_id: optional_uuid(row, "record_type_id")?,
        values,
        version: row.try_get("version")?,
        created_at: time(row, "created_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_at: time(row, "updated_at")?,
        updated_by: optional_uuid(row, "updated_by")?,
        touched_at: time(row, "touched_at")?,
        archived_at: optional_time(row, "archived_at")?,
        deleted_at: optional_time(row, "deleted_at")?,
    })
}

impl SqliteObjectTx {
    async fn fields_of(&mut self, object_id: Uuid) -> Result<Vec<FieldDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {FIELD_COLUMNS} {FIELD_FROM} \
             WHERE f.tenant_id = ?1 AND f.object_id = ?2 AND f.status = 'active' \
             ORDER BY f.position, f.created_at, f.id"
        ))
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(field_from_row).collect()
    }

    async fn object_where(
        &mut self,
        column: &str,
        value: String,
    ) -> Result<Option<ObjectDefinition>, ObjectError> {
        let row = sqlx::query(&format!(
            "SELECT {OBJECT_COLUMNS} FROM object_definitions WHERE tenant_id = ?1 AND {column} = ?2"
        ))
        .bind(&self.tenant)
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
        binds: &[String],
    ) -> Result<Option<Record>, ObjectError> {
        let sql = format!(
            "SELECT {RECORD_COLUMNS} FROM records r WHERE r.tenant_id = ?1 AND {condition}"
        );
        let mut query = sqlx::query(&sql).bind(&self.tenant);
        for value in binds {
            query = query.bind(value);
        }
        let row = query.fetch_optional(&mut *self.tx).await?;
        row.as_ref().map(record_from_row).transpose()
    }
}

impl SqliteObjectTx {
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
                 correlation_id, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, \
                 ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
            )
            .bind(&self.tenant)
            .bind(job.id.to_string())
            .bind(job.automation_id.to_string())
            .bind(job.object_id.to_string())
            .bind(job.record_id.to_string())
            .bind(&job.record)
            .bind(job.event.as_str())
            .bind(job.scheduled)
            .bind(timestamp(job.due_at))
            .bind(job.status.as_str())
            .bind(job.attempts)
            .bind(job.locked_until.map(timestamp))
            .bind(&job.last_error)
            .bind(job.depth)
            .bind(job.actor_id.to_string())
            .bind(job.correlation_id.to_string())
            .bind(timestamp(job.created_at))
            .bind(timestamp(job.updated_at))
            .execute(&mut *self.tx)
            .await?
        };
        Ok(result.rows_affected() == 1)
    }

    async fn update_job_from(
        &mut self,
        job: &crate::AutomationJob,
        from: crate::JobStatus,
    ) -> Result<sqlx::sqlite::SqliteQueryResult, ObjectError> {
        Ok(sqlx::query(
            "UPDATE automation_jobs SET due_at = ?1, status = ?2, attempts = ?3, locked_until = ?4, \
             last_error = ?5, updated_at = ?6 WHERE tenant_id = ?7 AND id = ?8 AND status = ?9 \
             AND (status <> 'running' OR attempts = ?10)",
        )
        .bind(timestamp(job.due_at))
        .bind(job.status.as_str())
        .bind(job.attempts)
        .bind(job.locked_until.map(timestamp))
        .bind(&job.last_error)
        .bind(timestamp(job.updated_at))
        .bind(&self.tenant)
        .bind(job.id.to_string())
        .bind(from.as_str())
        .bind(job.attempts)
        .execute(&mut *self.tx)
        .await?)
    }
}

#[async_trait]
impl ObjectTx for SqliteObjectTx {
    async fn lock_metadata(&mut self) -> Result<(), ObjectError> {
        // Workspace transactions begin IMMEDIATE, so they already hold the
        // file's write lock.
        Ok(())
    }

    async fn list_objects(&mut self) -> Result<Vec<ObjectDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {OBJECT_COLUMNS} FROM object_definitions \
             WHERE tenant_id = ?1 AND status = 'active' ORDER BY label, api_name"
        ))
        .bind(&self.tenant)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(object_from_row).collect()
    }

    async fn object_by_api_name(
        &mut self,
        api_name: &str,
    ) -> Result<Option<ObjectDefinition>, ObjectError> {
        self.object_where("api_name", api_name.to_owned()).await
    }

    async fn object_by_id(&mut self, id: Uuid) -> Result<Option<ObjectDefinition>, ObjectError> {
        self.object_where("id", id.to_string()).await
    }

    async fn key_prefixes(&mut self) -> Result<Vec<String>, ObjectError> {
        Ok(
            sqlx::query_scalar("SELECT key_prefix FROM object_definitions WHERE tenant_id = ?1")
                .bind(&self.tenant)
                .fetch_all(&mut *self.tx)
                .await?,
        )
    }

    async fn fields_referencing_object(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<FieldDefinition>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {FIELD_COLUMNS} {FIELD_FROM} \
             WHERE f.tenant_id = ?1 AND f.reference_object_id = ?2 AND f.status = 'active'"
        ))
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(field_from_row).collect()
    }

    async fn insert_object(&mut self, object: &ObjectDefinition) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO object_definitions (tenant_id, id, api_name, label, plural_label, description, \
               origin, owner_plugin, key_prefix, name_label, sharing_model, searchable, in_navigation, \
               status, version, created_at, created_by, updated_at, updated_by, grant_hierarchy, track_history) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)",
        )
        .bind(&self.tenant)
        .bind(object.id.to_string())
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
        .bind(timestamp(object.created_at))
        .bind(text(object.created_by))
        .bind(timestamp(object.updated_at))
        .bind(text(object.updated_by))
        .bind(object.grant_hierarchy)
        .bind(object.track_history)
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
            "UPDATE object_definitions SET label = ?4, plural_label = ?5, description = ?6, \
               name_label = ?7, sharing_model = ?8, searchable = ?9, in_navigation = ?10, \
               status = ?11, version = ?12, updated_at = ?13, updated_by = ?14, \
               grant_hierarchy = ?15, track_history = ?16, \
               deleted_at = CASE WHEN ?11 = 'deleted' THEN coalesce(deleted_at, ?13) END \
             WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
        )
        .bind(&self.tenant)
        .bind(object.id.to_string())
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
        .bind(timestamp(object.updated_at))
        .bind(text(object.updated_by))
        .bind(object.grant_hierarchy)
        .bind(object.track_history)
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, \
               ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32)",
        )
        .bind(&self.tenant)
        .bind(field.id.to_string())
        .bind(field.object_id.to_string())
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
        .bind(encode(&field.options)?)
        .bind(field.default_value.as_ref().map(encode).transpose()?)
        .bind(text(field.reference_object_id))
        .bind(field.delete_behaviour.map(DeleteBehaviour::as_str))
        .bind(field.position)
        .bind(field.status.as_str())
        .bind(field.version)
        .bind(timestamp(field.created_at))
        .bind(text(field.created_by))
        .bind(timestamp(field.updated_at))
        .bind(text(field.updated_by))
        .bind(field.track_history)
        .bind(&field.formula)
        .bind(field.formula_type.map(FieldType::as_str))
        .bind(field.rollup.as_ref().map(|rollup| rollup.function.as_str()))
        .bind(text(field.rollup.as_ref().map(|rollup| rollup.relationship_id)))
        .bind(text(field.rollup.as_ref().and_then(|rollup| rollup.field_id)))
        .bind(&field.lookup_filter)
        .execute(&mut *self.tx)
        .await
        .map_err(|error| conflict_on_unique(error, "objects.duplicate_api_name"))?;
        Ok(())
    }

    async fn update_field(
        &mut self,
        field: &FieldDefinition,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let updated = sqlx::query(
            "UPDATE field_definitions SET label = ?4, description = ?5, required = ?6, indexed = ?7, \
               searchable = ?8, max_length = ?9, options = ?10, default_value = ?11, position = ?12, \
               status = ?13, version = ?14, updated_at = ?15, updated_by = ?16, \
               track_history = ?17, formula = ?18, lookup_filter = ?19, \
               deleted_at = CASE WHEN ?13 = 'deleted' THEN coalesce(deleted_at, ?15) END \
             WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
        )
        .bind(&self.tenant)
        .bind(field.id.to_string())
        .bind(expected_version)
        .bind(&field.label)
        .bind(&field.description)
        .bind(field.required)
        .bind(field.indexed)
        .bind(field.searchable)
        .bind(field.max_length)
        .bind(encode(&field.options)?)
        .bind(field.default_value.as_ref().map(encode).transpose()?)
        .bind(field.position)
        .bind(field.status.as_str())
        .bind(field.version)
        .bind(timestamp(field.updated_at))
        .bind(text(field.updated_by))
        .bind(field.track_history)
        .bind(&field.formula)
        .bind(&field.lookup_filter)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        Ok(updated == 1)
    }

    async fn drop_field_index(&mut self, field_id: Uuid) -> Result<(), ObjectError> {
        for table in ["record_index", "record_unique", "record_references"] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE tenant_id = ?1 AND field_id = ?2"
            ))
            .bind(&self.tenant)
            .bind(field_id.to_string())
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
        let mut sql: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT object_id, max(can_read) AS can_read, max(can_create) AS can_create, \
               max(can_edit) AS can_edit, max(can_delete) AS can_delete, \
               max(view_all) AS view_all, max(modify_all) AS modify_all \
             FROM object_permissions WHERE tenant_id = ",
        );
        sql.push_bind(self.tenant.clone()).push(" AND role_id IN (");
        let mut separated = sql.separated(", ");
        for role in role_ids {
            separated.push_bind(role.to_string());
        }
        sql.push(") GROUP BY object_id");
        let rows = sql.build().fetch_all(&mut *self.tx).await?;
        rows.iter()
            .map(|row| {
                Ok((
                    uuid(row, "object_id")?,
                    AccessFlags {
                        read: flag(row, "can_read")?,
                        create: flag(row, "can_create")?,
                        edit: flag(row, "can_edit")?,
                        delete: flag(row, "can_delete")?,
                        view_all: flag(row, "view_all")?,
                        modify_all: flag(row, "modify_all")?,
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
             FROM object_permissions WHERE tenant_id = ?1 AND object_id = ?2 ORDER BY role_id",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(RolePermission {
                    role_id: uuid(row, "role_id")?,
                    access: AccessFlags {
                        read: flag(row, "can_read")?,
                        create: flag(row, "can_create")?,
                        edit: flag(row, "can_edit")?,
                        delete: flag(row, "can_delete")?,
                        view_all: flag(row, "view_all")?,
                        modify_all: flag(row, "modify_all")?,
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
                "DELETE FROM object_permissions WHERE tenant_id = ?1 AND object_id = ?2 AND role_id = ?3",
            )
            .bind(&self.tenant)
            .bind(object_id.to_string())
            .bind(permission.role_id.to_string())
            .execute(&mut *self.tx)
            .await?;
            return Ok(());
        }
        sqlx::query(
            "INSERT INTO object_permissions (tenant_id, object_id, role_id, can_read, can_create, \
               can_edit, can_delete, view_all, modify_all, updated_at, updated_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11) \
             ON CONFLICT (tenant_id, object_id, role_id) DO UPDATE SET \
               can_read = excluded.can_read, can_create = excluded.can_create, \
               can_edit = excluded.can_edit, can_delete = excluded.can_delete, \
               view_all = excluded.view_all, modify_all = excluded.modify_all, \
               updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .bind(permission.role_id.to_string())
        .bind(access.read)
        .bind(access.create)
        .bind(access.edit)
        .bind(access.delete)
        .bind(access.view_all)
        .bind(access.modify_all)
        .bind(timestamp(Utc::now()))
        .bind(actor.to_string())
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn next_record_sequence(&mut self, object_id: Uuid) -> Result<u64, ObjectError> {
        let sequence: i64 = sqlx::query_scalar(
            "UPDATE object_definitions SET record_sequence = record_sequence + 1 \
             WHERE tenant_id = ?1 AND id = ?2 RETURNING record_sequence",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_one(&mut *self.tx)
        .await?;
        u64::try_from(sequence).map_err(|_| ObjectError::Invariant)
    }

    async fn insert_record(&mut self, record: &Record) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO records (tenant_id, id, object_id, public_id, name, name_key, owner_id, \
               record_type_id, data, version, created_at, created_by, updated_at, updated_by, \
               touched_at, archived_at, deleted_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
        )
        .bind(&self.tenant)
        .bind(record.id.to_string())
        .bind(record.object_id.to_string())
        .bind(&record.public_id)
        .bind(&record.name)
        .bind(name_key(&record.name))
        .bind(text(record.owner_id))
        .bind(text(record.record_type_id))
        .bind(encode(&record.values)?)
        .bind(record.version)
        .bind(timestamp(record.created_at))
        .bind(text(record.created_by))
        .bind(timestamp(record.updated_at))
        .bind(text(record.updated_by))
        .bind(timestamp(record.touched_at))
        .bind(record.archived_at.map(timestamp))
        .bind(record.deleted_at.map(timestamp))
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
            "UPDATE records SET name = ?4, name_key = ?5, owner_id = ?6, record_type_id = ?7, \
               data = ?8, version = ?9, updated_at = ?10, updated_by = ?11, touched_at = ?12, \
               archived_at = ?13, deleted_at = ?14 \
             WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
        )
        .bind(&self.tenant)
        .bind(record.id.to_string())
        .bind(expected_version)
        .bind(&record.name)
        .bind(name_key(&record.name))
        .bind(text(record.owner_id))
        .bind(text(record.record_type_id))
        .bind(encode(&record.values)?)
        .bind(record.version)
        .bind(timestamp(record.updated_at))
        .bind(text(record.updated_by))
        .bind(timestamp(record.touched_at))
        .bind(record.archived_at.map(timestamp))
        .bind(record.deleted_at.map(timestamp))
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
            "r.object_id = ?2 AND r.public_id = ?3",
            &[object_id.to_string(), public_id.to_owned()],
        )
        .await
    }

    async fn record_by_public_id(
        &mut self,
        public_id: &str,
    ) -> Result<Option<Record>, ObjectError> {
        self.record_where("r.public_id = ?2", &[public_id.to_owned()])
            .await
    }

    async fn record_by_id(&mut self, id: Uuid) -> Result<Option<Record>, ObjectError> {
        self.record_where("r.id = ?2", &[id.to_string()]).await
    }

    async fn replace_index(
        &mut self,
        record_id: Uuid,
        index: &RecordIndex,
    ) -> Result<(), ObjectError> {
        let record = record_id.to_string();
        for table in [
            "record_index",
            "record_unique",
            "record_references",
            "record_search_terms",
        ] {
            sqlx::query(&format!(
                "DELETE FROM {table} WHERE tenant_id = ?1 AND record_id = ?2"
            ))
            .bind(&self.tenant)
            .bind(&record)
            .execute(&mut *self.tx)
            .await?;
        }
        for chunk in index.search.chunks(200) {
            let mut insert: QueryBuilder<Sqlite> = QueryBuilder::new(
                "INSERT INTO record_search_terms (tenant_id, term, record_id, weight) ",
            );
            insert.push_values(chunk, |mut row, term| {
                row.push_bind(self.tenant.clone())
                    .push_bind(term.term.clone())
                    .push_bind(record.clone())
                    .push_bind(term.weight);
            });
            insert.build().execute(&mut *self.tx).await?;
        }
        for entry in &index.entries {
            let (text_value, number, time_value) = match &entry.value {
                IndexValue::Text(value) => (Some(value.clone()), None, None),
                IndexValue::Number(value) => (None, Some(*value), None),
                IndexValue::Time(value) => (None, None, Some(timestamp(*value))),
            };
            sqlx::query(
                "INSERT INTO record_index (tenant_id, field_id, record_id, text_value, number_value, time_value) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&self.tenant)
            .bind(entry.field_id.to_string())
            .bind(&record)
            .bind(text_value)
            .bind(number)
            .bind(time_value)
            .execute(&mut *self.tx)
            .await?;
        }
        for unique in &index.uniques {
            let inserted = sqlx::query(
                "INSERT INTO record_unique (tenant_id, field_id, value_key, record_id) \
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
            )
            .bind(&self.tenant)
            .bind(unique.field_id.to_string())
            .bind(&unique.value_key)
            .bind(&record)
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
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(&self.tenant)
            .bind(reference.field_id.to_string())
            .bind(&record)
            .bind(reference.target_id.to_string())
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn referencing(&mut self, target_id: Uuid) -> Result<Vec<(Uuid, Uuid)>, ObjectError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT rr.field_id, rr.record_id FROM record_references rr \
             JOIN records r ON r.tenant_id = rr.tenant_id AND r.id = rr.record_id \
             WHERE rr.tenant_id = ?1 AND rr.target_id = ?2 AND r.deleted_at IS NULL \
             ORDER BY rr.record_id",
        )
        .bind(&self.tenant)
        .bind(target_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.into_iter()
            .map(|(field, record)| {
                Ok((
                    Uuid::parse_str(&field).map_err(|_| ObjectError::Invariant)?,
                    Uuid::parse_str(&record).map_err(|_| ObjectError::Invariant)?,
                ))
            })
            .collect()
    }

    async fn list_records(
        &mut self,
        object_id: Uuid,
        query: &RecordQuery,
    ) -> Result<Vec<Record>, ObjectError> {
        let mut sql: QueryBuilder<Sqlite> = QueryBuilder::new(format!(
            "SELECT {RECORD_COLUMNS} FROM records r WHERE r.tenant_id = "
        ));
        sql.push_bind(self.tenant.clone())
            .push(" AND r.object_id = ")
            .push_bind(object_id.to_string());
        sql.push(match query.status {
            RecordStatus::Active => " AND r.deleted_at IS NULL",
            RecordStatus::Deleted => " AND r.deleted_at IS NOT NULL",
            RecordStatus::Any => "",
        });
        if let Some(owner) = query.owner {
            sql.push(" AND r.owner_id = ").push_bind(owner.to_string());
        }
        if let Some(after) = query.touched_after {
            sql.push(" AND r.touched_at > ").push_bind(timestamp(after));
        }
        if let Some(visibility) = &query.visibility {
            // A detail is visible when its master is (ADR 0025).
            let target = if let Some(field_id) = visibility.master {
                sql.push(
                    " AND EXISTS (SELECT 1 FROM record_references mr JOIN records m \
                     ON m.tenant_id = mr.tenant_id AND m.id = mr.target_id \
                     WHERE mr.tenant_id = r.tenant_id AND mr.record_id = r.id AND mr.field_id = ",
                )
                .push_bind(field_id.to_string());
                "m"
            } else {
                "r"
            };
            sql.push(format!(" AND ({target}.owner_id IN ("));
            for (position, owner) in visibility.owners.iter().enumerate() {
                if position > 0 {
                    sql.push(", ");
                }
                sql.push_bind(owner.to_string());
            }
            sql.push(format!(
                ") OR EXISTS (SELECT 1 FROM record_shares s WHERE s.tenant_id = {target}.tenant_id \
                 AND s.record_id = {target}.id AND ((s.grantee_kind = 'user' AND s.grantee_id = "
            ))
            .push_bind(visibility.user_id.to_string())
            .push(")");
            if !visibility.role_ids.is_empty() {
                sql.push(" OR (s.grantee_kind = 'role' AND s.grantee_id IN (");
                for (position, role) in visibility.role_ids.iter().enumerate() {
                    if position > 0 {
                        sql.push(", ");
                    }
                    sql.push_bind(role.to_string());
                }
                sql.push("))");
            }
            sql.push(")))");
            if visibility.master.is_some() {
                sql.push(")");
            }
        }
        if let Some(term) = &query.name_contains {
            sql.push(" AND instr(r.name_key, ")
                .push_bind(term.clone())
                .push(") > 0");
        }
        for filter in &query.filters {
            sql.push(
                " AND EXISTS (SELECT 1 FROM record_index i WHERE i.tenant_id = r.tenant_id \
                 AND i.record_id = r.id AND i.field_id = ",
            )
            .push_bind(filter.field_id.to_string());
            match &filter.value {
                IndexValue::Text(value) => {
                    sql.push(" AND i.text_value = ").push_bind(value.clone())
                }
                IndexValue::Number(value) => sql.push(" AND i.number_value = ").push_bind(*value),
                IndexValue::Time(value) => sql
                    .push(" AND i.time_value = ")
                    .push_bind(timestamp(*value)),
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
            if query.sort != RecordSort::Name && parse_timestamp(&cursor.sort_value).is_none() {
                return Err(ObjectError::invalid(
                    "cursor",
                    "records.cursor_invalid",
                    "start the list again",
                ));
            }
            sql.push(format!(" AND ({column}, r.id) {comparison} ("))
                .push_bind(cursor.sort_value.clone())
                .push(", ")
                .push_bind(cursor.id.to_string())
                .push(")");
        }
        sql.push(format!(
            " ORDER BY {column} {direction}, r.id {direction} LIMIT "
        ))
        .push_bind(i64::from(query.limit) + 1);
        let rows = sql.build().fetch_all(&mut *self.tx).await?;
        rows.iter().map(record_from_row).collect()
    }

    async fn record_shares(&mut self, record_id: Uuid) -> Result<Vec<RecordShare>, ObjectError> {
        let rows = sqlx::query(
            "SELECT grantee_kind, grantee_id, access, created_at, created_by FROM record_shares \
             WHERE tenant_id = ?1 AND record_id = ?2 ORDER BY grantee_kind, created_at, grantee_id",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(RecordShare {
                    grantee_kind: parse(GranteeKind::parse(
                        &row.try_get::<String, _>("grantee_kind")?,
                    ))?,
                    grantee_id: uuid(row, "grantee_id")?,
                    access: parse(ShareAccess::parse(&row.try_get::<String, _>("access")?))?,
                    created_at: time(row, "created_at")?,
                    created_by: optional_uuid(row, "created_by")?,
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
        let mut sql: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT access FROM record_shares WHERE tenant_id = ");
        sql.push_bind(self.tenant.clone())
            .push(" AND record_id = ")
            .push_bind(record_id.to_string())
            .push(" AND ((grantee_kind = 'user' AND grantee_id = ")
            .push_bind(user_id.to_string())
            .push(")");
        if !role_ids.is_empty() {
            sql.push(" OR (grantee_kind = 'role' AND grantee_id IN (");
            for (position, role) in role_ids.iter().enumerate() {
                if position > 0 {
                    sql.push(", ");
                }
                sql.push_bind(role.to_string());
            }
            sql.push("))");
        }
        sql.push(")");
        let access: Vec<String> = sql.build_query_scalar().fetch_all(&mut *self.tx).await?;
        Ok(access
            .iter()
            .filter_map(|value| ShareAccess::parse(value))
            .max())
    }

    async fn put_share(&mut self, record_id: Uuid, share: &RecordShare) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO record_shares (tenant_id, record_id, grantee_kind, grantee_id, access, created_at, created_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT (tenant_id, record_id, grantee_kind, grantee_id) DO UPDATE SET access = excluded.access",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .bind(share.grantee_kind.as_str())
        .bind(share.grantee_id.to_string())
        .bind(share.access.as_str())
        .bind(timestamp(share.created_at))
        .bind(text(share.created_by))
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
            "DELETE FROM record_shares WHERE tenant_id = ?1 AND record_id = ?2 \
             AND grantee_kind = ?3 AND grantee_id = ?4",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .bind(grantee_kind.as_str())
        .bind(grantee_id.to_string())
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        Ok(deleted == 1)
    }

    async fn delete_shares(&mut self, record_id: Uuid) -> Result<u64, ObjectError> {
        Ok(
            sqlx::query("DELETE FROM record_shares WHERE tenant_id = ?1 AND record_id = ?2")
                .bind(&self.tenant)
                .bind(record_id.to_string())
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
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )
            .bind(&self.tenant)
            .bind(entry.id.to_string())
            .bind(entry.record_id.to_string())
            .bind(entry.object_id.to_string())
            .bind(entry.kind.as_str())
            .bind(&entry.field)
            .bind(entry.old_value.as_ref().map(encode).transpose()?)
            .bind(entry.new_value.as_ref().map(encode).transpose()?)
            .bind(timestamp(entry.changed_at))
            .bind(text(entry.changed_by))
            .bind(text(entry.correlation_id))
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
        let mut sql: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT id, record_id, object_id, kind, field, old_value, new_value, changed_at, \
             changed_by, correlation_id FROM record_history WHERE tenant_id = ",
        );
        sql.push_bind(self.tenant.clone())
            .push(" AND record_id = ")
            .push_bind(record_id.to_string());
        if let Some((at, id)) = before {
            sql.push(" AND (changed_at < ")
                .push_bind(timestamp(at))
                .push(" OR (changed_at = ")
                .push_bind(timestamp(at))
                .push(" AND id < ")
                .push_bind(id.to_string())
                .push("))");
        }
        sql.push(" ORDER BY changed_at DESC, id DESC LIMIT ")
            .push_bind(i64::from(limit) + 1);
        let rows = sql.build().fetch_all(&mut *self.tx).await?;
        let json = |row: &SqliteRow, column: &str| -> Result<Option<Value>, ObjectError> {
            row.try_get::<Option<String>, _>(column)?
                .map(|text| serde_json::from_str(&text).map_err(|_| ObjectError::Invariant))
                .transpose()
        };
        rows.iter()
            .map(|row| {
                Ok(HistoryEntry {
                    id: uuid(row, "id")?,
                    record_id: uuid(row, "record_id")?,
                    object_id: uuid(row, "object_id")?,
                    kind: parse(HistoryKind::parse(&row.try_get::<String, _>("kind")?))?,
                    field: row.try_get("field")?,
                    old_value: json(row, "old_value")?,
                    new_value: json(row, "new_value")?,
                    changed_at: time(row, "changed_at")?,
                    changed_by: optional_uuid(row, "changed_by")?,
                    correlation_id: optional_uuid(row, "correlation_id")?,
                })
            })
            .collect()
    }

    async fn append_change(&mut self, change: &RecordChange) -> Result<i64, ObjectError> {
        let position: i64 = sqlx::query_scalar(
            "INSERT INTO change_counters (tenant_id, last_position) VALUES (?1, 1) \
             ON CONFLICT (tenant_id) DO UPDATE SET last_position = last_position + 1 \
             RETURNING last_position",
        )
        .bind(&self.tenant)
        .fetch_one(&mut *self.tx)
        .await?;
        sqlx::query(
            "INSERT INTO record_changes (tenant_id, position, record_id, object_id, public_id, kind, \
               fields, changed_at, changed_by, correlation_id) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        )
        .bind(&self.tenant)
        .bind(position)
        .bind(change.record_id.to_string())
        .bind(change.object_id.to_string())
        .bind(&change.public_id)
        .bind(change.kind.as_str())
        .bind(encode(&change.fields)?)
        .bind(timestamp(change.changed_at))
        .bind(text(change.changed_by))
        .bind(text(change.correlation_id))
        .execute(&mut *self.tx)
        .await?;
        Ok(position)
    }

    async fn changes(&mut self, after: i64, limit: u16) -> Result<Vec<RecordChange>, ObjectError> {
        let rows = sqlx::query(
            "SELECT position, record_id, object_id, public_id, kind, fields, changed_at, changed_by, \
               correlation_id FROM record_changes WHERE tenant_id = ?1 AND position > ?2 \
             ORDER BY position LIMIT ?3",
        )
        .bind(&self.tenant)
        .bind(after)
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(RecordChange {
                    position: row.try_get("position")?,
                    record_id: uuid(row, "record_id")?,
                    object_id: uuid(row, "object_id")?,
                    public_id: row.try_get("public_id")?,
                    kind: parse(ChangeKind::parse(&row.try_get::<String, _>("kind")?))?,
                    fields: serde_json::from_str(&row.try_get::<String, _>("fields")?)
                        .map_err(|_| ObjectError::Invariant)?,
                    changed_at: time(row, "changed_at")?,
                    changed_by: optional_uuid(row, "changed_by")?,
                    correlation_id: optional_uuid(row, "correlation_id")?,
                })
            })
            .collect()
    }

    async fn change_bounds(&mut self) -> Result<(i64, i64), ObjectError> {
        let bounds: Option<(i64, i64)> = sqlx::query_as(
            "SELECT last_position, expired_through FROM change_counters WHERE tenant_id = ?1",
        )
        .bind(&self.tenant)
        .fetch_optional(&mut *self.tx)
        .await?;
        Ok(bounds.unwrap_or((0, 0)))
    }

    async fn bump_metadata_version(&mut self) -> Result<i64, ObjectError> {
        Ok(sqlx::query_scalar(
            "INSERT INTO change_counters (tenant_id, metadata_version) VALUES (?1, 1) \
             ON CONFLICT (tenant_id) DO UPDATE \
             SET metadata_version = change_counters.metadata_version + 1 \
             RETURNING metadata_version",
        )
        .bind(&self.tenant)
        .fetch_one(&mut *self.tx)
        .await?)
    }

    async fn metadata_version(&mut self) -> Result<i64, ObjectError> {
        let version: Option<i64> =
            sqlx::query_scalar("SELECT metadata_version FROM change_counters WHERE tenant_id = ?1")
                .bind(&self.tenant)
                .fetch_optional(&mut *self.tx)
                .await?;
        Ok(version.unwrap_or(0))
    }

    async fn expire_changes(&mut self, before: DateTime<Utc>) -> Result<u64, ObjectError> {
        let through: Option<i64> = sqlx::query_scalar(
            "SELECT max(position) FROM record_changes WHERE tenant_id = ?1 AND changed_at < ?2",
        )
        .bind(&self.tenant)
        .bind(timestamp(before))
        .fetch_one(&mut *self.tx)
        .await?;
        let Some(through) = through else {
            return Ok(0);
        };
        let removed =
            sqlx::query("DELETE FROM record_changes WHERE tenant_id = ?1 AND position <= ?2")
                .bind(&self.tenant)
                .bind(through)
                .execute(&mut *self.tx)
                .await?
                .rows_affected();
        sqlx::query(
            "UPDATE change_counters SET expired_through = max(expired_through, ?2) WHERE tenant_id = ?1",
        )
        .bind(&self.tenant)
        .bind(through)
        .execute(&mut *self.tx)
        .await?;
        Ok(removed)
    }

    async fn query_records(&mut self, plan: &RecordPlan) -> Result<Vec<Record>, ObjectError> {
        let rendered = sql::records(Dialect::Sqlite, self.tenant_id, plan);
        let rows = self.run_query(&rendered).await?;
        rows.iter().map(record_from_row).collect()
    }

    async fn query_aggregate(
        &mut self,
        plan: &AggregatePlan,
    ) -> Result<Vec<Vec<Cell>>, ObjectError> {
        let rendered = sql::aggregate(Dialect::Sqlite, self.tenant_id, plan);
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
                            // SQLite may return a whole REAL sum as an integer.
                            CellKind::Number => match row.try_get::<Option<f64>, _>(index) {
                                Ok(value) => value.map_or(Cell::Null, Cell::Number),
                                Err(_) => row
                                    .try_get::<Option<i64>, _>(index)?
                                    .map_or(Cell::Null, |value| Cell::Number(value as f64)),
                            },
                            CellKind::Int => row
                                .try_get::<Option<i64>, _>(index)?
                                .map_or(Cell::Null, Cell::Int),
                            CellKind::Time => match row.try_get::<Option<String>, _>(index)? {
                                None => Cell::Null,
                                Some(text) => Cell::Time(parse(parse_timestamp(&text))?),
                            },
                            CellKind::Uuid => match row.try_get::<Option<String>, _>(index)? {
                                None => Cell::Null,
                                Some(text) => Cell::Uuid(
                                    Uuid::parse_str(&text).map_err(|_| ObjectError::Invariant)?,
                                ),
                            },
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
        let rendered = sql::search(Dialect::Sqlite, self.tenant_id, plan);
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
            "SELECT {RULE_COLUMNS} FROM validation_rules WHERE tenant_id = ?1 AND object_id = ?2 ORDER BY name"
        ))
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(rule_from_row).collect()
    }

    async fn all_validation_rules(&mut self) -> Result<Vec<ValidationRule>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {RULE_COLUMNS} FROM validation_rules WHERE tenant_id = ?1 ORDER BY object_id, name"
        ))
        .bind(&self.tenant)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(rule_from_row).collect()
    }

    async fn insert_validation_rule(&mut self, rule: &ValidationRule) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO validation_rules (tenant_id, id, object_id, name, description, condition, message, \
             error_field, active, version, created_at, created_by, updated_at, updated_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )
        .bind(&self.tenant)
        .bind(rule.id.to_string())
        .bind(rule.object_id.to_string())
        .bind(&rule.name)
        .bind(&rule.description)
        .bind(&rule.condition)
        .bind(&rule.message)
        .bind(&rule.error_field)
        .bind(rule.active)
        .bind(rule.version)
        .bind(timestamp(rule.created_at))
        .bind(rule.created_by.map(|id| id.to_string()))
        .bind(timestamp(rule.updated_at))
        .bind(rule.updated_by.map(|id| id.to_string()))
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
            "UPDATE validation_rules SET description = ?1, condition = ?2, message = ?3, error_field = ?4, \
             active = ?5, version = ?6, updated_at = ?7, updated_by = ?8 \
             WHERE tenant_id = ?9 AND id = ?10 AND version = ?11",
        )
        .bind(&rule.description)
        .bind(&rule.condition)
        .bind(&rule.message)
        .bind(&rule.error_field)
        .bind(rule.active)
        .bind(rule.version)
        .bind(timestamp(rule.updated_at))
        .bind(rule.updated_by.map(|id| id.to_string()))
        .bind(&self.tenant)
        .bind(rule.id.to_string())
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
            "DELETE FROM validation_rules WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
        )
        .bind(&self.tenant)
        .bind(rule_id.to_string())
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
             WHERE q.tenant_id = ?1 AND q.object_id = ?2 ORDER BY q.position, q.name"
        ))
        .bind(&self.tenant)
        .bind(object_id.to_string())
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
             WHERE q.tenant_id = ?1 AND q.target_object_id = ?2 \
             ORDER BY q.object_id, q.position, q.name"
        ))
        .bind(&self.tenant)
        .bind(target_object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(quick_action_from_row).collect()
    }

    async fn insert_quick_action(
        &mut self,
        action: &crate::QuickAction,
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO quick_actions (tenant_id, id, object_id, name, label, kind, position, \
             target_object_id, target_field, fields, presets, version, created_at, created_by, \
             updated_at, updated_by) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )
        .bind(&self.tenant)
        .bind(action.id.to_string())
        .bind(action.object_id.to_string())
        .bind(&action.name)
        .bind(&action.label)
        .bind(action.kind.as_str())
        .bind(action.position)
        .bind(text(action.target_object_id))
        .bind(&action.target_field)
        .bind(encode(&action.fields)?)
        .bind(encode(&action.presets)?)
        .bind(action.version)
        .bind(timestamp(action.created_at))
        .bind(text(action.created_by))
        .bind(timestamp(action.updated_at))
        .bind(text(action.updated_by))
        .execute(&mut *self.tx)
        .await
        .map_err(|error| conflict_on_unique(error, "quick_actions.duplicate_name"))?;
        Ok(())
    }

    async fn update_quick_action(
        &mut self,
        action: &crate::QuickAction,
        expected_version: i64,
    ) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE quick_actions SET label = ?1, position = ?2, fields = ?3, presets = ?4, \
             version = ?5, updated_at = ?6, updated_by = ?7 \
             WHERE tenant_id = ?8 AND id = ?9 AND version = ?10",
        )
        .bind(&action.label)
        .bind(action.position)
        .bind(encode(&action.fields)?)
        .bind(encode(&action.presets)?)
        .bind(action.version)
        .bind(timestamp(action.updated_at))
        .bind(text(action.updated_by))
        .bind(&self.tenant)
        .bind(action.id.to_string())
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
            "DELETE FROM quick_actions WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
        )
        .bind(&self.tenant)
        .bind(action_id.to_string())
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn automations(
        &mut self,
        object_id: Uuid,
    ) -> Result<Vec<crate::Automation>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations WHERE tenant_id = ?1 AND object_id = ?2 ORDER BY name"
        ))
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(automation_from_row).collect()
    }

    async fn all_automations(&mut self) -> Result<Vec<crate::Automation>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {AUTOMATION_COLUMNS} FROM automations WHERE tenant_id = ?1 ORDER BY object_id, name"
        ))
        .bind(&self.tenant)
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
        )
        .bind(&self.tenant)
        .bind(automation.id.to_string())
        .bind(automation.object_id.to_string())
        .bind(&automation.name)
        .bind(&automation.description)
        .bind(automation.trigger.as_str())
        .bind(automation.timing.as_str())
        .bind(&automation.condition)
        .bind(actions_text(automation)?)
        .bind(schedule_text(automation)?)
        .bind(automation.active)
        .bind(automation.version)
        .bind(timestamp(automation.created_at))
        .bind(automation.created_by.map(|id| id.to_string()))
        .bind(timestamp(automation.updated_at))
        .bind(automation.updated_by.map(|id| id.to_string()))
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
            "UPDATE automations SET description = ?1, trigger = ?2, condition = ?3, actions = ?4, \
             schedule = ?5, active = ?6, version = ?7, updated_at = ?8, updated_by = ?9 \
             WHERE tenant_id = ?10 AND id = ?11 AND version = ?12",
        )
        .bind(&automation.description)
        .bind(automation.trigger.as_str())
        .bind(&automation.condition)
        .bind(actions_text(automation)?)
        .bind(schedule_text(automation)?)
        .bind(automation.active)
        .bind(automation.version)
        .bind(timestamp(automation.updated_at))
        .bind(automation.updated_by.map(|id| id.to_string()))
        .bind(&self.tenant)
        .bind(automation.id.to_string())
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
            "DELETE FROM automations WHERE tenant_id = ?1 AND id = ?2 AND version = ?3",
        )
        .bind(&self.tenant)
        .bind(automation_id.to_string())
        .bind(expected_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn enqueue_job(&mut self, job: &crate::AutomationJob) -> Result<(), ObjectError> {
        if job.scheduled {
            let moved = sqlx::query(
                "UPDATE automation_jobs SET due_at = ?1, event = ?2, actor_id = ?3, correlation_id = ?4, \
                 depth = ?5, updated_at = ?6 WHERE tenant_id = ?7 AND automation_id = ?8 AND record_id = ?9 \
                 AND scheduled AND status = 'pending'",
            )
            .bind(timestamp(job.due_at))
            .bind(job.event.as_str())
            .bind(job.actor_id.to_string())
            .bind(job.correlation_id.to_string())
            .bind(job.depth)
            .bind(timestamp(job.updated_at))
            .bind(&self.tenant)
            .bind(job.automation_id.to_string())
            .bind(job.record_id.to_string())
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
            "UPDATE automation_jobs SET status = 'cancelled', updated_at = ?1 WHERE tenant_id = ?2 \
             AND automation_id = ?3 AND record_id = ?4 AND scheduled AND status = 'pending'",
        )
        .bind(timestamp(at))
        .bind(&self.tenant)
        .bind(automation_id.to_string())
        .bind(record_id.to_string())
        .execute(&mut *self.tx)
        .await?;
        Ok(result.rows_affected())
    }

    async fn job(&mut self, job_id: Uuid) -> Result<Option<crate::AutomationJob>, ObjectError> {
        let row = sqlx::query(&format!(
            "SELECT {JOB_COLUMNS} FROM automation_jobs WHERE tenant_id = ?1 AND id = ?2"
        ))
        .bind(&self.tenant)
        .bind(job_id.to_string())
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
            "UPDATE automation_jobs SET status = 'running', attempts = attempts + 1, locked_until = ?1, \
             updated_at = ?2 WHERE tenant_id = ?3 AND id = ?4 AND ((status = 'pending' AND due_at <= ?2) \
             OR (status = 'running' AND locked_until < ?2)) RETURNING {JOB_COLUMNS}"
        ))
        .bind(timestamp(locked_until))
        .bind(timestamp(now))
        .bind(&self.tenant)
        .bind(job_id.to_string())
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
            "SELECT {JOB_COLUMNS} FROM automation_jobs WHERE tenant_id = ?1 AND automation_id = ?2 \
             ORDER BY created_at DESC, id DESC LIMIT ?3"
        ))
        .bind(&self.tenant)
        .bind(automation_id.to_string())
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(job_from_row).collect()
    }

    async fn due_jobs(&mut self, now: DateTime<Utc>, limit: u32) -> Result<Vec<Uuid>, ObjectError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM automation_jobs WHERE tenant_id = ?1 AND ((status = 'pending' AND due_at <= ?2) \
             OR (status = 'running' AND locked_until < ?2)) ORDER BY due_at, id LIMIT ?3",
        )
        .bind(&self.tenant)
        .bind(timestamp(now))
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        ids.iter()
            .map(|id| Uuid::parse_str(id).map_err(|_| ObjectError::Invariant))
            .collect()
    }

    async fn insert_bulk_job(
        &mut self,
        job: &crate::BulkJob,
        rows: &[crate::BulkRow],
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO bulk_jobs (tenant_id, id, object, operation, status, total, succeeded, failed, \
             actor_id, correlation_id, locked_until, created_at, updated_at, completed_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        )
        .bind(&self.tenant)
        .bind(job.id.to_string())
        .bind(&job.object)
        .bind(job.operation.as_str())
        .bind(job.status.as_str())
        .bind(job.total)
        .bind(job.succeeded)
        .bind(job.failed)
        .bind(job.actor_id.to_string())
        .bind(job.correlation_id.to_string())
        .bind(job.locked_until.map(timestamp))
        .bind(timestamp(job.created_at))
        .bind(timestamp(job.updated_at))
        .bind(job.completed_at.map(timestamp))
        .execute(&mut *self.tx)
        .await?;
        for row in rows {
            sqlx::query(
                "INSERT INTO bulk_job_rows (tenant_id, job_id, row_index, input, status, record, errors) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(&self.tenant)
            .bind(row.job_id.to_string())
            .bind(row.index)
            .bind(row.input.to_string())
            .bind(row.status.as_str())
            .bind(&row.record)
            .bind(row.errors.as_ref().map(Value::to_string))
            .execute(&mut *self.tx)
            .await?;
        }
        Ok(())
    }

    async fn bulk_job(&mut self, job_id: Uuid) -> Result<Option<crate::BulkJob>, ObjectError> {
        let row = sqlx::query(&format!(
            "SELECT {BULK_COLUMNS} FROM bulk_jobs WHERE tenant_id = ?1 AND id = ?2"
        ))
        .bind(&self.tenant)
        .bind(job_id.to_string())
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
            "UPDATE bulk_jobs SET status = 'running', locked_until = ?1, updated_at = ?2 \
             WHERE tenant_id = ?3 AND id = ?4 AND (status = 'queued' OR (status = 'running' AND locked_until < ?2)) \
             RETURNING {BULK_COLUMNS}"
        ))
        .bind(timestamp(locked_until))
        .bind(timestamp(now))
        .bind(&self.tenant)
        .bind(job_id.to_string())
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(bulk_from_row).transpose()
    }

    async fn update_bulk_job(&mut self, job: &crate::BulkJob) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE bulk_jobs SET status = ?1, locked_until = ?2, updated_at = ?3, completed_at = ?4, \
             succeeded = (SELECT count(*) FROM bulk_job_rows r WHERE r.tenant_id = bulk_jobs.tenant_id \
             AND r.job_id = bulk_jobs.id AND r.status = 'succeeded'), \
             failed = (SELECT count(*) FROM bulk_job_rows r WHERE r.tenant_id = bulk_jobs.tenant_id \
             AND r.job_id = bulk_jobs.id AND r.status = 'failed') \
             WHERE tenant_id = ?5 AND id = ?6 AND status IN ('queued', 'running') \
             AND (?1 = 'aborted' OR status = 'running')",
        )
        .bind(job.status.as_str())
        .bind(job.locked_until.map(timestamp))
        .bind(timestamp(job.updated_at))
        .bind(job.completed_at.map(timestamp))
        .bind(&self.tenant)
        .bind(job.id.to_string())
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
             WHERE tenant_id = ?1 AND job_id = ?2 AND (?3 IS NULL OR status = ?3) \
             AND row_index > ?4 ORDER BY row_index LIMIT ?5",
        )
        .bind(&self.tenant)
        .bind(job_id.to_string())
        .bind(status.map(crate::BulkRowStatus::as_str))
        .bind(after.unwrap_or(-1))
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(bulk_row_from_row).collect()
    }

    async fn finish_bulk_row(&mut self, row: &crate::BulkRow) -> Result<bool, ObjectError> {
        let result = sqlx::query(
            "UPDATE bulk_job_rows SET status = ?1, record = ?2, errors = ?3 \
             WHERE tenant_id = ?4 AND job_id = ?5 AND row_index = ?6 AND status = 'pending'",
        )
        .bind(row.status.as_str())
        .bind(&row.record)
        .bind(row.errors.as_ref().map(Value::to_string))
        .bind(&self.tenant)
        .bind(row.job_id.to_string())
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
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM bulk_jobs WHERE tenant_id = ?1 AND (status = 'queued' \
             OR (status = 'running' AND locked_until < ?2)) ORDER BY updated_at, id LIMIT ?3",
        )
        .bind(&self.tenant)
        .bind(timestamp(now))
        .bind(i64::from(limit))
        .fetch_all(&mut *self.tx)
        .await?;
        ids.iter()
            .map(|id| Uuid::parse_str(id).map_err(|_| ObjectError::Invariant))
            .collect()
    }

    async fn insert_automation_runs(
        &mut self,
        runs: &[crate::AutomationRun],
    ) -> Result<(), ObjectError> {
        for run in runs {
            sqlx::query(
                "INSERT INTO automation_runs (tenant_id, id, automation_id, object_id, record, event, \
                 outcome, detail, saves, ran_at, ran_by, correlation_id) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )
            .bind(&self.tenant)
            .bind(run.id.to_string())
            .bind(run.automation_id.to_string())
            .bind(run.object_id.to_string())
            .bind(&run.record)
            .bind(run.event.as_str())
            .bind(run.outcome.as_str())
            .bind(&run.detail)
            .bind(run.saves)
            .bind(timestamp(run.ran_at))
            .bind(run.ran_by.map(|id| id.to_string()))
            .bind(run.correlation_id.map(|id| id.to_string()))
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
                    "SELECT {RUN_COLUMNS} FROM automation_runs WHERE tenant_id = ?1 AND automation_id = ?2 \
                     AND (ran_at < ?3 OR (ran_at = ?3 AND id < ?4)) ORDER BY ran_at DESC, id DESC LIMIT ?5"
                ))
                .bind(&self.tenant)
                .bind(automation_id.to_string())
                .bind(timestamp(ran_at))
                .bind(id.to_string())
                .bind(i64::from(limit))
                .fetch_all(&mut *self.tx)
                .await?
            }
            None => {
                sqlx::query(&format!(
                    "SELECT {RUN_COLUMNS} FROM automation_runs WHERE tenant_id = ?1 AND automation_id = ?2 \
                     ORDER BY ran_at DESC, id DESC LIMIT ?3"
                ))
                .bind(&self.tenant)
                .bind(automation_id.to_string())
                .bind(i64::from(limit))
                .fetch_all(&mut *self.tx)
                .await?
            }
        };
        rows.iter().map(run_from_row).collect()
    }

    async fn expire_automation_runs(&mut self, before: DateTime<Utc>) -> Result<u64, ObjectError> {
        let result =
            sqlx::query("DELETE FROM automation_runs WHERE tenant_id = ?1 AND ran_at < ?2")
                .bind(&self.tenant)
                .bind(timestamp(before))
                .execute(&mut *self.tx)
                .await?;
        Ok(result.rows_affected())
    }

    async fn formulas_indexed(&mut self, object_id: Uuid) -> Result<bool, ObjectError> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM formula_index_marks WHERE tenant_id = ?1 AND object_id = ?2)",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_one(&mut *self.tx)
        .await?)
    }

    async fn mark_formulas_indexed(&mut self, object_id: Uuid) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO formula_index_marks (tenant_id, object_id, indexed_at) VALUES (?1, ?2, ?3) \
             ON CONFLICT (tenant_id, object_id) DO UPDATE SET indexed_at = excluded.indexed_at",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .bind(timestamp(chrono::Utc::now()))
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn record_ids(&mut self, object_id: Uuid) -> Result<Vec<Uuid>, ObjectError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM records WHERE tenant_id = ?1 AND object_id = ?2 ORDER BY id",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        ids.iter()
            .map(|id| Uuid::parse_str(id).map_err(|_| ObjectError::Invariant))
            .collect()
    }

    async fn lock_object(&mut self, _object_id: Uuid, _exclusive: bool) -> Result<(), ObjectError> {
        // Workspace transactions begin IMMEDIATE and hold the file's write
        // lock, so they are already serialised.
        Ok(())
    }

    async fn lock_record(&mut self, _record_id: Uuid, _exclusive: bool) -> Result<(), ObjectError> {
        // Serialised by the IMMEDIATE transaction, as above.
        Ok(())
    }

    async fn guarded_field_count(&mut self, object_id: Uuid) -> Result<usize, ObjectError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM field_definitions WHERE tenant_id = ?1 AND object_id = ?2 \
             AND field_type IN ('master_detail', 'rollup') AND status = 'active'",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
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
        let filter = "rr.tenant_id = ?1 AND rr.field_id = ?2 AND rr.target_id = ?3 \
             AND d.deleted_at IS NULL";
        let (function, field) = match (query.function, query.field) {
            (RollupFunction::Count, _) => {
                let count: i64 =
                    sqlx::query_scalar(&format!("SELECT count(*) {details} WHERE {filter}"))
                        .bind(&self.tenant)
                        .bind(query.relationship_id.to_string())
                        .bind(query.master_id.to_string())
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
             ON i.tenant_id = rr.tenant_id AND i.record_id = rr.record_id AND i.field_id = ?4 \
             WHERE {filter}"
        );
        if time {
            // Times are stored in one fixed-width format, so they order as text.
            let value: Option<String> = sqlx::query_scalar(&sql)
                .bind(&self.tenant)
                .bind(query.relationship_id.to_string())
                .bind(query.master_id.to_string())
                .bind(field_id.to_string())
                .fetch_one(&mut *self.tx)
                .await?;
            return value
                .map(|value| parse(parse_timestamp(&value)).map(IndexValue::Time))
                .transpose();
        }
        let value: Option<f64> = sqlx::query_scalar(&sql)
            .bind(&self.tenant)
            .bind(query.relationship_id.to_string())
            .bind(query.master_id.to_string())
            .bind(field_id.to_string())
            .fetch_one(&mut *self.tx)
            .await?;
        Ok(value.map(IndexValue::Number))
    }

    async fn reports(&mut self) -> Result<Vec<crate::Report>, ObjectError> {
        let rows = sqlx::query(&format!(
            "SELECT {REPORT_COLUMNS} FROM reports WHERE tenant_id = ?1 ORDER BY id"
        ))
        .bind(&self.tenant)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(report_from_row).collect()
    }

    async fn report(&mut self, id: Uuid) -> Result<Option<crate::Report>, ObjectError> {
        sqlx::query(&format!(
            "SELECT {REPORT_COLUMNS} FROM reports WHERE tenant_id = ?1 AND id = ?2"
        ))
        .bind(&self.tenant)
        .bind(id.to_string())
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
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        )
        .bind(&self.tenant)
        .bind(report.id.to_string())
        .bind(&report.name)
        .bind(&report.description)
        .bind(&report.query)
        .bind(report.chart.as_str())
        .bind(&report.label_column)
        .bind(&report.value_column)
        .bind(report.visibility.as_str())
        .bind(report.owner_id.to_string())
        .bind(report.version)
        .bind(timestamp(report.created_at))
        .bind(text(report.created_by))
        .bind(timestamp(report.updated_at))
        .bind(text(report.updated_by))
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
            "UPDATE reports SET name = ?3, description = ?4, query = ?5, chart = ?6, \
               label_column = ?7, value_column = ?8, visibility = ?9, version = ?10, \
               updated_at = ?11, updated_by = ?12 \
             WHERE tenant_id = ?1 AND id = ?2 AND version = ?13",
        )
        .bind(&self.tenant)
        .bind(report.id.to_string())
        .bind(&report.name)
        .bind(&report.description)
        .bind(&report.query)
        .bind(report.chart.as_str())
        .bind(&report.label_column)
        .bind(&report.value_column)
        .bind(report.visibility.as_str())
        .bind(report.version)
        .bind(timestamp(report.updated_at))
        .bind(text(report.updated_by))
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
            sqlx::query("DELETE FROM reports WHERE tenant_id = ?1 AND id = ?2 AND version = ?3")
                .bind(&self.tenant)
                .bind(id.to_string())
                .bind(expected_version)
                .execute(&mut *self.tx)
                .await?
                .rows_affected();
        Ok(deleted == 1)
    }

    async fn has_records(&mut self, object_id: Uuid) -> Result<bool, ObjectError> {
        let found: i64 = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM records WHERE tenant_id = ?1 AND object_id = ?2)",
        )
        .bind(&self.tenant)
        .bind(object_id.to_string())
        .fetch_one(&mut *self.tx)
        .await?;
        Ok(found != 0)
    }

    async fn master_links(&mut self) -> Result<Vec<(Uuid, Uuid)>, ObjectError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT object_id, reference_object_id FROM field_definitions \
             WHERE tenant_id = ?1 AND field_type = 'master_detail' AND status = 'active' \
             ORDER BY object_id, id",
        )
        .bind(&self.tenant)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.into_iter()
            .map(|(detail, master)| {
                Ok((
                    Uuid::parse_str(&detail).map_err(|_| ObjectError::Invariant)?,
                    Uuid::parse_str(&master).map_err(|_| ObjectError::Invariant)?,
                ))
            })
            .collect()
    }

    async fn set_deleted_with(
        &mut self,
        record_id: Uuid,
        root: Option<Uuid>,
    ) -> Result<(), ObjectError> {
        sqlx::query("UPDATE records SET deleted_with = ?3 WHERE tenant_id = ?1 AND id = ?2")
            .bind(&self.tenant)
            .bind(record_id.to_string())
            .bind(root.map(|root| root.to_string()))
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    async fn deleted_with(&mut self, root: Uuid) -> Result<Vec<Uuid>, ObjectError> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM records WHERE tenant_id = ?1 AND deleted_with = ?2 \
             AND deleted_at IS NOT NULL ORDER BY id",
        )
        .bind(&self.tenant)
        .bind(root.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        ids.iter()
            .map(|id| Uuid::parse_str(id).map_err(|_| ObjectError::Invariant))
            .collect()
    }

    async fn append_evidence(
        &mut self,
        audit: &AuditEvent,
        event: &EventEnvelope,
    ) -> Result<(), ObjectError> {
        append_audit(&mut self.tx, audit).await?;
        append_outbox(&mut self.tx, event).await?;
        Ok(())
    }

    async fn remote_version(
        &mut self,
        record_id: Uuid,
    ) -> Result<Option<RemoteVersion>, ObjectError> {
        let row: Option<(i64, bool, i64)> = sqlx::query_as(
            "SELECT version, deleted, through_position FROM replica_versions \
             WHERE tenant_id = ?1 AND record_id = ?2",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .fetch_optional(&mut *self.tx)
        .await?;
        Ok(row.map(|(version, deleted, through)| RemoteVersion {
            version,
            deleted,
            through,
        }))
    }

    async fn set_remote_version(
        &mut self,
        record_id: Uuid,
        remote: RemoteVersion,
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO replica_versions (tenant_id, record_id, version, deleted, through_position) \
             VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT (tenant_id, record_id) \
             DO UPDATE SET version = excluded.version, deleted = excluded.deleted, \
             through_position = excluded.through_position",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .bind(remote.version)
        .bind(remote.deleted)
        .bind(remote.through)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn remember_base(
        &mut self,
        record_id: Uuid,
        field: &str,
        value: &Value,
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO replica_bases (tenant_id, record_id, field, value) VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT DO NOTHING",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .bind(field)
        .bind(value.to_string())
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn replica_bases(
        &mut self,
        record_id: Uuid,
    ) -> Result<BTreeMap<String, Value>, ObjectError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT field, value FROM replica_bases WHERE tenant_id = ?1 AND record_id = ?2",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.into_iter()
            .map(|(field, value)| {
                serde_json::from_str(&value)
                    .map(|value| (field, value))
                    .map_err(|_| ObjectError::Invariant)
            })
            .collect()
    }

    async fn clear_replica_bases(&mut self, record_id: Uuid) -> Result<(), ObjectError> {
        sqlx::query("DELETE FROM replica_bases WHERE tenant_id = ?1 AND record_id = ?2")
            .bind(&self.tenant)
            .bind(record_id.to_string())
            .execute(&mut *self.tx)
            .await?;
        Ok(())
    }

    async fn sync_positions(&mut self) -> Result<SyncPositions, ObjectError> {
        let row: Option<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT pushed_position, pulled_position, metadata_version FROM replica_sync \
             WHERE tenant_id = ?1",
        )
        .bind(&self.tenant)
        .fetch_optional(&mut *self.tx)
        .await?;
        Ok(row.map_or_else(
            SyncPositions::default,
            |(pushed, pulled, metadata_version)| SyncPositions {
                pushed,
                pulled,
                metadata_version,
            },
        ))
    }

    async fn set_sync_positions(&mut self, positions: SyncPositions) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO replica_sync (tenant_id, pushed_position, pulled_position, metadata_version) \
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT (tenant_id) DO UPDATE SET \
             pushed_position = excluded.pushed_position, \
             pulled_position = excluded.pulled_position, \
             metadata_version = excluded.metadata_version",
        )
        .bind(&self.tenant)
        .bind(positions.pushed)
        .bind(positions.pulled)
        .bind(positions.metadata_version)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn replica_lease(&mut self) -> Result<ReplicaLease, ObjectError> {
        type Row = (
            Option<i64>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<i64>,
        );
        let row: Option<Row> = sqlx::query_as(
            "SELECT offline_limit_seconds, last_contact_at, clock_seen_at, contact_boot, \
               contact_since_boot_ms FROM replica_sync WHERE tenant_id = ?1",
        )
        .bind(&self.tenant)
        .fetch_optional(&mut *self.tx)
        .await?;
        let Some((offline_limit_seconds, last_contact_at, clock_seen_at, boot, since_boot)) = row
        else {
            return Ok(ReplicaLease::default());
        };
        let contact_trusted = match (boot, since_boot) {
            (Some(boot), Some(millis)) => Some(crate::TrustedInstant {
                boot,
                since_boot: std::time::Duration::from_millis(
                    u64::try_from(millis).map_err(|_| ObjectError::Invariant)?,
                ),
            }),
            _ => None,
        };
        let time = |value: Option<String>| {
            value
                .map(|value| parse(parse_timestamp(&value)))
                .transpose()
        };
        Ok(ReplicaLease {
            offline_limit_seconds,
            last_contact_at: time(last_contact_at)?,
            clock_seen_at: time(clock_seen_at)?,
            contact_trusted,
        })
    }

    async fn set_replica_lease(&mut self, lease: ReplicaLease) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO replica_sync (tenant_id, pushed_position, offline_limit_seconds, \
               last_contact_at, clock_seen_at, contact_boot, contact_since_boot_ms) \
             VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6) ON CONFLICT (tenant_id) DO UPDATE SET \
             offline_limit_seconds = excluded.offline_limit_seconds, \
             last_contact_at = excluded.last_contact_at, \
             clock_seen_at = excluded.clock_seen_at, \
             contact_boot = excluded.contact_boot, \
             contact_since_boot_ms = excluded.contact_since_boot_ms",
        )
        .bind(&self.tenant)
        .bind(lease.offline_limit_seconds)
        .bind(lease.last_contact_at.map(timestamp))
        .bind(lease.clock_seen_at.map(timestamp))
        .bind(
            lease
                .contact_trusted
                .as_ref()
                .map(|trusted| trusted.boot.clone()),
        )
        .bind(
            lease
                .contact_trusted
                .as_ref()
                .map(|trusted| i64::try_from(trusted.since_boot.as_millis()).unwrap_or(i64::MAX)),
        )
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn replica_directory(&mut self) -> Result<ReplicaDirectory, ObjectError> {
        let mut directory = ReplicaDirectory::default();
        let members: Vec<(String, String)> =
            sqlx::query_as("SELECT user_id, name FROM replica_members WHERE tenant_id = ?1")
                .bind(&self.tenant)
                .fetch_all(&mut *self.tx)
                .await?;
        for (id, name) in members {
            directory
                .members
                .insert(parse(Uuid::parse_str(&id).ok())?, name);
        }
        let roles: Vec<(String, String)> =
            sqlx::query_as("SELECT role_id, name FROM replica_roles WHERE tenant_id = ?1")
                .bind(&self.tenant)
                .fetch_all(&mut *self.tx)
                .await?;
        for (id, name) in roles {
            directory
                .roles
                .insert(parse(Uuid::parse_str(&id).ok())?, name);
        }
        Ok(directory)
    }

    async fn replace_replica_directory(
        &mut self,
        directory: &ReplicaDirectory,
    ) -> Result<(), ObjectError> {
        for table in ["replica_members", "replica_roles"] {
            sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = ?1"))
                .bind(&self.tenant)
                .execute(&mut *self.tx)
                .await?;
        }
        for (id, name) in &directory.members {
            sqlx::query(
                "INSERT INTO replica_members (tenant_id, user_id, name) VALUES (?1, ?2, ?3)",
            )
            .bind(&self.tenant)
            .bind(id.to_string())
            .bind(name)
            .execute(&mut *self.tx)
            .await?;
        }
        for (id, name) in &directory.roles {
            sqlx::query("INSERT INTO replica_roles (tenant_id, role_id, name) VALUES (?1, ?2, ?3)")
                .bind(&self.tenant)
                .bind(id.to_string())
                .bind(name)
                .execute(&mut *self.tx)
                .await?;
        }
        Ok(())
    }

    async fn replica_manifest(&mut self) -> Result<Vec<(String, String)>, ObjectError> {
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT o.api_name, r.public_id FROM records r \
             JOIN replica_versions v ON v.tenant_id = r.tenant_id AND v.record_id = r.id \
             JOIN object_definitions o ON o.tenant_id = r.tenant_id AND o.id = r.object_id \
             WHERE r.tenant_id = ?1 AND r.deleted_at IS NULL \
             ORDER BY r.created_at, r.id",
        )
        .bind(&self.tenant)
        .fetch_all(&mut *self.tx)
        .await?;
        Ok(rows)
    }

    async fn forget_record(&mut self, record_id: Uuid) -> Result<(), ObjectError> {
        let id = record_id.to_string();
        for statement in [
            "DELETE FROM record_index WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM record_unique WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM record_references WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM record_search_terms WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM record_shares WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM record_history WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM replica_versions WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM replica_pending_references WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM replica_bases WHERE tenant_id = ?1 AND record_id = ?2",
            "DELETE FROM records WHERE tenant_id = ?1 AND id = ?2",
        ] {
            sqlx::query(statement)
                .bind(&self.tenant)
                .bind(&id)
                .execute(&mut *self.tx)
                .await?;
        }
        Ok(())
    }

    async fn add_pending_reference(
        &mut self,
        record_id: Uuid,
        target_public_id: &str,
    ) -> Result<(), ObjectError> {
        sqlx::query(
            "INSERT INTO replica_pending_references (tenant_id, record_id, target_public_id) \
             VALUES (?1, ?2, ?3) ON CONFLICT DO NOTHING",
        )
        .bind(&self.tenant)
        .bind(record_id.to_string())
        .bind(target_public_id)
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn take_pending_references(
        &mut self,
        target_public_id: &str,
    ) -> Result<Vec<Uuid>, ObjectError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "DELETE FROM replica_pending_references WHERE tenant_id = ?1 AND target_public_id = ?2 \
             RETURNING record_id",
        )
        .bind(&self.tenant)
        .bind(target_public_id)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|id| Uuid::parse_str(id).map_err(|_| ObjectError::Invariant))
            .collect()
    }

    async fn commit(self: Box<Self>) -> Result<(), ObjectError> {
        self.tx.commit().await?;
        Ok(())
    }
}
