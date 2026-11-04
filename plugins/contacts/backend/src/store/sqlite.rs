//! The SQLite Contacts store (ADR 0013).
//!
//! Each workspace has its own database file, opened only through
//! [`SqliteTenantDatabases`], and every table still carries `tenant_id` with
//! every statement filtering on it. Contacts tables use the `p_contacts_`
//! prefix. Values that PostgreSQL keeps as `jsonb`, `uuid`, `text[]` and
//! `timestamptz` are stored as JSON text, hyphenated UUID text, a JSON array
//! and fixed-width UTC text respectively. Name and email search keys are
//! computed in Rust so search and keyset cursors behave as on PostgreSQL.

use super::{Actor, ContactsStore, ContactsTx};
use crate::model::*;
use crate::ContactsError;
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Duration, Utc};
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use platform_extensions::ExtensionValues;
use platform_id::PublicId;
use platform_storage::sqlite::{
    append_audit, append_outbox, parse_timestamp, timestamp, SqliteStoreError,
    SqliteTenantDatabases, TenantMigration,
};
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, Sqlite, Transaction};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// Contacts tables in every workspace file.
pub const SQLITE_MIGRATIONS: &[TenantMigration] = &[
    TenantMigration {
        owner: "platform.contacts",
        id: "0001_contacts",
        sql: r#"
CREATE TABLE p_contacts_organisations (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  public_id TEXT NOT NULL,
  name TEXT NOT NULL,
  name_key TEXT NOT NULL,
  email TEXT,
  email_key TEXT,
  telephone TEXT,
  custom_fields TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(custom_fields) AND json_type(custom_fields) = 'object'),
  extensions TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(extensions) AND json_type(extensions) = 'object' AND length(extensions) <= 65536),
  version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
  archived_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, public_id)
);
CREATE INDEX p_contacts_organisations_name_idx ON p_contacts_organisations (tenant_id, name_key, id);
CREATE INDEX p_contacts_organisations_email_idx ON p_contacts_organisations (tenant_id, email_key);

CREATE TABLE p_contacts_people (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  public_id TEXT NOT NULL,
  full_name TEXT NOT NULL,
  name_key TEXT NOT NULL,
  email TEXT,
  email_key TEXT,
  telephone TEXT,
  organisation_id TEXT,
  tags TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(tags) AND json_type(tags) = 'array'),
  custom_fields TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(custom_fields) AND json_type(custom_fields) = 'object'),
  extensions TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(extensions) AND json_type(extensions) = 'object' AND length(extensions) <= 65536),
  version INTEGER NOT NULL DEFAULT 1 CHECK (version > 0),
  archived_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, public_id),
  FOREIGN KEY (tenant_id, organisation_id) REFERENCES p_contacts_organisations (tenant_id, id)
);
CREATE INDEX p_contacts_people_name_idx ON p_contacts_people (tenant_id, name_key, id);
CREATE INDEX p_contacts_people_email_idx ON p_contacts_people (tenant_id, email_key);
CREATE INDEX p_contacts_people_org_idx ON p_contacts_people (tenant_id, organisation_id, id);

CREATE TABLE p_contacts_custom_field_definitions (
  tenant_id TEXT NOT NULL,
  id TEXT NOT NULL,
  applies_to TEXT NOT NULL CHECK (applies_to IN ('person', 'organisation')),
  api_name TEXT NOT NULL,
  label TEXT NOT NULL,
  description TEXT,
  field_type TEXT NOT NULL,
  required INTEGER NOT NULL,
  unique_per_tenant INTEGER NOT NULL,
  indexed INTEGER NOT NULL,
  options TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(options)),
  validation_schema TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(validation_schema)),
  data_classification TEXT NOT NULL,
  purpose TEXT,
  retention_days INTEGER,
  encrypted INTEGER NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  disabled_at TEXT,
  archived_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id),
  UNIQUE (tenant_id, applies_to, api_name)
);

CREATE TABLE p_contacts_public_id_counters (
  tenant_id TEXT NOT NULL,
  construct TEXT NOT NULL,
  next_value INTEGER NOT NULL CHECK (next_value > 0),
  PRIMARY KEY (tenant_id, construct)
);

CREATE TABLE p_contacts_list_cursors (
  tenant_id TEXT NOT NULL,
  token_hash TEXT NOT NULL,
  resource TEXT NOT NULL,
  query_hash TEXT NOT NULL,
  sort_value TEXT NOT NULL,
  record_id TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, token_hash)
);

CREATE TABLE p_contacts_activity (
  tenant_id TEXT NOT NULL,
  event_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_public_id TEXT NOT NULL,
  action TEXT NOT NULL,
  entity_version INTEGER NOT NULL,
  actor_id TEXT,
  occurred_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, event_id)
);
CREATE INDEX p_contacts_activity_entity_idx ON p_contacts_activity (tenant_id, entity_type, entity_public_id, occurred_at);
"#,
    },
    // Standard record fields (ADR 0014). Existing rows start with
    // touched_at at their last update and no creator.
    TenantMigration {
        owner: "platform.contacts",
        id: "0002_record_stamps",
        sql: r#"
ALTER TABLE p_contacts_people ADD COLUMN created_by TEXT;
ALTER TABLE p_contacts_people ADD COLUMN updated_by TEXT;
ALTER TABLE p_contacts_people ADD COLUMN touched_at TEXT;
UPDATE p_contacts_people SET touched_at = updated_at;
ALTER TABLE p_contacts_organisations ADD COLUMN created_by TEXT;
ALTER TABLE p_contacts_organisations ADD COLUMN updated_by TEXT;
ALTER TABLE p_contacts_organisations ADD COLUMN touched_at TEXT;
UPDATE p_contacts_organisations SET touched_at = updated_at;
CREATE INDEX p_contacts_people_touched_idx ON p_contacts_people (tenant_id, touched_at, id);
CREATE INDEX p_contacts_organisations_touched_idx ON p_contacts_organisations (tenant_id, touched_at, id);
"#,
    },
    // Recycle bin (ADR 0014, #290).
    TenantMigration {
        owner: "platform.contacts",
        id: "0003_recycle_bin",
        sql: r#"
ALTER TABLE p_contacts_people ADD COLUMN deleted_at TEXT;
ALTER TABLE p_contacts_organisations ADD COLUMN deleted_at TEXT;
CREATE INDEX p_contacts_people_deleted_idx ON p_contacts_people (tenant_id, deleted_at, id) WHERE deleted_at IS NOT NULL;
CREATE INDEX p_contacts_organisations_deleted_idx ON p_contacts_organisations (tenant_id, deleted_at, id) WHERE deleted_at IS NOT NULL;
"#,
    },
];

#[derive(Clone)]
pub struct SqliteContactsStore {
    databases: Arc<SqliteTenantDatabases>,
}

impl SqliteContactsStore {
    /// `databases` must have been created with [`SQLITE_MIGRATIONS`].
    pub fn new(databases: Arc<SqliteTenantDatabases>) -> Self {
        Self { databases }
    }
}

fn storage(error: SqliteStoreError) -> ContactsError {
    match error {
        SqliteStoreError::Database(error) => ContactsError::Database(error),
        SqliteStoreError::MigrationDrift { .. } => ContactsError::Invariant,
        // Contacts does not run on an encrypted device file yet.
        SqliteStoreError::Key => ContactsError::Invariant,
        SqliteStoreError::Io(error) => ContactsError::Database(sqlx::Error::Io(error)),
    }
}

#[async_trait]
impl ContactsStore for SqliteContactsStore {
    async fn begin(
        &self,
        tenant_id: Uuid,
        actor: Actor,
    ) -> Result<Box<dyn ContactsTx>, ContactsError> {
        let tx = self.databases.begin(tenant_id).await.map_err(storage)?;
        Ok(Box::new(SqliteContactsTx { tx, actor }))
    }
}

struct SqliteContactsTx {
    tx: Transaction<'static, Sqlite>,
    actor: Actor,
}

const PERSON_COLUMNS: &str = "p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone, \
     p.organisation_id, o.public_id AS organisation_public_id, p.tags, p.custom_fields, \
     p.extensions, p.version, p.archived_at, p.created_at, p.updated_at, \
     p.created_by, p.updated_by, p.touched_at, p.deleted_at";
const PERSON_FROM: &str = "FROM p_contacts_people p LEFT JOIN p_contacts_organisations o \
     ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id";
const ORGANISATION_COLUMNS: &str = "o.id, o.public_id, o.tenant_id, o.name, o.email, \
     o.telephone, o.custom_fields, o.extensions, o.version, o.archived_at, o.created_at, o.updated_at, \
     o.created_by, o.updated_by, o.touched_at, o.deleted_at";
const DEFINITION_COLUMNS: &str = "id, tenant_id, applies_to, api_name, label, description, \
     field_type, required, unique_per_tenant, indexed, options, validation_schema, \
     data_classification, purpose, retention_days, encrypted, version, disabled_at, archived_at, \
     created_at, updated_at";

fn invariant<T, E>(result: Result<T, E>) -> Result<T, ContactsError> {
    result.map_err(|_| ContactsError::Invariant)
}

fn uuid(row: &SqliteRow, column: &str) -> Result<Uuid, ContactsError> {
    invariant(Uuid::parse_str(&row.try_get::<String, _>(column)?))
}

fn optional_uuid(row: &SqliteRow, column: &str) -> Result<Option<Uuid>, ContactsError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|value| invariant(Uuid::parse_str(&value)))
        .transpose()
}

fn time(row: &SqliteRow, column: &str) -> Result<DateTime<Utc>, ContactsError> {
    parse_timestamp(&row.try_get::<String, _>(column)?).ok_or(ContactsError::Invariant)
}

fn optional_time(row: &SqliteRow, column: &str) -> Result<Option<DateTime<Utc>>, ContactsError> {
    row.try_get::<Option<String>, _>(column)?
        .map(|value| parse_timestamp(&value).ok_or(ContactsError::Invariant))
        .transpose()
}

fn json<T: serde::de::DeserializeOwned>(row: &SqliteRow, column: &str) -> Result<T, ContactsError> {
    invariant(serde_json::from_str(&row.try_get::<String, _>(column)?))
}

fn encode<T: serde::Serialize>(value: &T) -> Result<String, ContactsError> {
    invariant(serde_json::to_string(value))
}

fn person_from_row(row: &SqliteRow) -> Result<Person, ContactsError> {
    Ok(Person {
        id: uuid(row, "id")?,
        public_id: row.try_get("public_id")?,
        tenant_id: uuid(row, "tenant_id")?,
        full_name: row.try_get("full_name")?,
        email: row.try_get("email")?,
        telephone: row.try_get("telephone")?,
        organisation_id: optional_uuid(row, "organisation_id")?,
        organisation_public_id: row.try_get("organisation_public_id")?,
        tags: json(row, "tags")?,
        custom_fields: json(row, "custom_fields")?,
        extensions: json(row, "extensions")?,
        version: row.try_get("version")?,
        archived_at: optional_time(row, "archived_at")?,
        created_at: time(row, "created_at")?,
        updated_at: time(row, "updated_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_by: optional_uuid(row, "updated_by")?,
        touched_at: time(row, "touched_at")?,
        deleted_at: optional_time(row, "deleted_at")?,
    })
}

fn organisation_from_row(row: &SqliteRow) -> Result<Organisation, ContactsError> {
    Ok(Organisation {
        id: uuid(row, "id")?,
        public_id: row.try_get("public_id")?,
        tenant_id: uuid(row, "tenant_id")?,
        name: row.try_get("name")?,
        email: row.try_get("email")?,
        telephone: row.try_get("telephone")?,
        custom_fields: json(row, "custom_fields")?,
        extensions: json(row, "extensions")?,
        version: row.try_get("version")?,
        archived_at: optional_time(row, "archived_at")?,
        created_at: time(row, "created_at")?,
        updated_at: time(row, "updated_at")?,
        created_by: optional_uuid(row, "created_by")?,
        updated_by: optional_uuid(row, "updated_by")?,
        touched_at: time(row, "touched_at")?,
        deleted_at: optional_time(row, "deleted_at")?,
    })
}

fn definition_from_row(row: &SqliteRow) -> Result<ContactCustomFieldDefinition, ContactsError> {
    let applies_to: String = row.try_get("applies_to")?;
    Ok(ContactCustomFieldDefinition {
        id: uuid(row, "id")?,
        tenant_id: uuid(row, "tenant_id")?,
        applies_to: match applies_to.as_str() {
            "person" => ContactCustomFieldTarget::Person,
            "organisation" => ContactCustomFieldTarget::Organisation,
            _ => return Err(ContactsError::Invariant),
        },
        api_name: row.try_get("api_name")?,
        label: row.try_get("label")?,
        description: row.try_get("description")?,
        field_type: ContactCustomFieldType::parse(&row.try_get::<String, _>("field_type")?)
            .ok_or(ContactsError::Invariant)?,
        required: row.try_get("required")?,
        unique_per_tenant: row.try_get("unique_per_tenant")?,
        indexed: row.try_get("indexed")?,
        options: json(row, "options")?,
        validation_schema: json(row, "validation_schema")?,
        data_classification: DataClassification::parse(
            &row.try_get::<String, _>("data_classification")?,
        )
        .ok_or(ContactsError::Invariant)?,
        purpose: row.try_get("purpose")?,
        retention_days: row
            .try_get::<Option<i64>, _>("retention_days")?
            .map(|days| invariant(u32::try_from(days)))
            .transpose()?,
        encrypted: row.try_get("encrypted")?,
        version: row.try_get("version")?,
        disabled_at: optional_time(row, "disabled_at")?,
        archived_at: optional_time(row, "archived_at")?,
        created_at: time(row, "created_at")?,
        updated_at: time(row, "updated_at")?,
    })
}

fn key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn optional_key(value: Option<&str>) -> Option<String> {
    value.map(key).filter(|value| !value.is_empty())
}

fn trimmed(value: Option<&str>) -> Option<String> {
    value.map(str::trim).map(str::to_owned)
}

/// A JSON path to one key, quoted so dotted plugin ids stay one segment.
fn json_path(segments: &[&str]) -> String {
    segments.iter().fold(String::from("$"), |path, segment| {
        format!("{path}.\"{}\"", segment.replace('"', ""))
    })
}

fn status_filter(alias: &str, status: RecordStatus) -> String {
    match status {
        RecordStatus::Active => {
            format!("{alias}.archived_at IS NULL AND {alias}.deleted_at IS NULL")
        }
        RecordStatus::Archived => {
            format!("{alias}.archived_at IS NOT NULL AND {alias}.deleted_at IS NULL")
        }
        RecordStatus::All => format!("{alias}.deleted_at IS NULL"),
        RecordStatus::Deleted => format!("{alias}.deleted_at IS NOT NULL"),
    }
}

fn query_hash(resource: &str, request: &ListRequest) -> Result<String, ContactsError> {
    let mut request = request.clone();
    request.cursor = None;
    let extension_filter = request
        .extension_filter
        .as_ref()
        .map(platform_extensions::ValidatedExtensionFilter::identity);
    let document = encode(&(resource, request, extension_filter))?;
    Ok(format!("{:x}", Sha256::digest(document.as_bytes())))
}

impl SqliteContactsTx {
    /// Stamps the standard record fields after a write in this transaction
    /// (ADR 0014): the acting member as modifier (and creator for a new
    /// record), and `touched_at` at the write's `updated_at`.
    async fn stamp(
        &mut self,
        table: &str,
        tenant_id: Uuid,
        public_id: &str,
        created: bool,
    ) -> Result<(), ContactsError> {
        let creator = if created { ", created_by = ?3" } else { "" };
        sqlx::query(&format!(
            "UPDATE {table} SET updated_by = ?3, touched_at = updated_at{creator} \
             WHERE tenant_id = ?1 AND public_id = ?2"
        ))
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(self.actor.member_id().map(|id| id.to_string()))
        .execute(&mut *self.tx)
        .await?;
        Ok(())
    }

    async fn person_by_public_id(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
    ) -> Result<Option<Person>, ContactsError> {
        let row = sqlx::query(&format!(
            "SELECT {PERSON_COLUMNS} {PERSON_FROM} WHERE p.tenant_id = ?1 AND p.public_id = ?2"
        ))
        .bind(tenant_id.to_string())
        .bind(public_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(person_from_row).transpose()
    }

    async fn organisation_by_public_id(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
    ) -> Result<Option<Organisation>, ContactsError> {
        let row = sqlx::query(&format!(
            "SELECT {ORGANISATION_COLUMNS} FROM p_contacts_organisations o \
             WHERE o.tenant_id = ?1 AND o.public_id = ?2"
        ))
        .bind(tenant_id.to_string())
        .bind(public_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        row.as_ref().map(organisation_from_row).transpose()
    }

    /// A guarded record write that matched no row: the record exists in the
    /// expected state (`in_bin` for undelete) at another version, or it does
    /// not exist for this write at all.
    async fn classify_record(
        &mut self,
        table: &str,
        tenant_id: Uuid,
        public_id: &str,
        in_bin: bool,
    ) -> Result<ContactsError, ContactsError> {
        let exists: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM {table} \
             WHERE tenant_id = ?1 AND public_id = ?2 AND (deleted_at IS NOT NULL) = ?3)"
        ))
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(in_bin)
        .fetch_one(&mut *self.tx)
        .await?;
        Ok(if exists {
            ContactsError::VersionConflict
        } else {
            ContactsError::NotFound
        })
    }

    /// A write that matched no row: the record is missing or its version moved.
    async fn classify(
        &mut self,
        table: &str,
        column: &str,
        tenant_id: Uuid,
        id: &str,
    ) -> Result<ContactsError, ContactsError> {
        let exists: bool = sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM {table} WHERE tenant_id = ?1 AND {column} = ?2)"
        ))
        .bind(tenant_id.to_string())
        .bind(id)
        .fetch_one(&mut *self.tx)
        .await?;
        Ok(if exists {
            ContactsError::VersionConflict
        } else {
            ContactsError::NotFound
        })
    }

    async fn resolve_cursor(
        &mut self,
        tenant_id: Uuid,
        resource: &str,
        query_hash: &str,
        token: Option<&str>,
    ) -> Result<Option<(String, String)>, ContactsError> {
        let Some(token) = token else { return Ok(None) };
        let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT sort_value, record_id FROM p_contacts_list_cursors \
             WHERE tenant_id = ?1 AND token_hash = ?2 AND resource = ?3 AND query_hash = ?4 \
               AND expires_at > ?5",
        )
        .bind(tenant_id.to_string())
        .bind(token_hash)
        .bind(resource)
        .bind(query_hash)
        .bind(timestamp(Utc::now()))
        .fetch_optional(&mut *self.tx)
        .await?;
        row.ok_or(ContactsError::InvalidCursor).map(Some)
    }

    async fn store_cursor(
        &mut self,
        tenant_id: Uuid,
        resource: &str,
        query_hash: &str,
        sort_value: String,
        record_id: Uuid,
    ) -> Result<String, ContactsError> {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));
        sqlx::query(
            "INSERT INTO p_contacts_list_cursors \
             (tenant_id, token_hash, resource, query_hash, sort_value, record_id, expires_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(tenant_id.to_string())
        .bind(token_hash)
        .bind(resource)
        .bind(query_hash)
        .bind(sort_value)
        .bind(record_id.to_string())
        .bind(timestamp(Utc::now() + Duration::hours(1)))
        .execute(&mut *self.tx)
        .await?;
        Ok(token)
    }

    /// Runs a keyset-paginated list. `table` is `p` for people, `o` for
    /// organisations; the caller provides the select and source clauses.
    #[allow(clippy::too_many_arguments)]
    async fn list<T>(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
        resource: &str,
        alias: &str,
        select: &str,
        extra_filters: &str,
        decode: fn(&SqliteRow) -> Result<T, ContactsError>,
        sort_value: fn(&T, ContactSort) -> String,
        record_id: fn(&T) -> Uuid,
    ) -> Result<Page<T>, ContactsError> {
        let query_hash = query_hash(resource, request)?;
        let cursor = self
            .resolve_cursor(tenant_id, resource, &query_hash, request.cursor.as_deref())
            .await?;
        let sort_expression = match request.sort {
            ContactSort::Name => format!("{alias}.name_key"),
            ContactSort::CreatedAt => format!("{alias}.created_at"),
            ContactSort::UpdatedAt => format!("{alias}.updated_at"),
        };
        let (operator, direction) = if request.direction == SortDirection::Asc {
            (">", "ASC")
        } else {
            ("<", "DESC")
        };
        let (filter_path, filter_value) = match &request.extension_filter {
            Some(filter) => (
                Some(json_path(&[filter.namespace(), filter.key()])),
                Some(filter.value().to_string()),
            ),
            None => (None, None),
        };
        let sql = format!(
            "{select} WHERE {alias}.tenant_id = ?1 \
               AND (?2 IS NULL OR instr({alias}.name_key, ?2) > 0) \
               AND (?3 IS NULL OR {alias}.email_key = ?3) \
               {extra_filters} \
               AND (?6 IS NULL OR json_extract({alias}.extensions, ?6) = json_extract(?7, '$')) \
               AND ({status}) \
               AND (?4 IS NULL OR ({sort_expression}, {alias}.id) {operator} (?4, ?5)) \
             ORDER BY {sort_expression} {direction}, {alias}.id {direction} \
             LIMIT ?8",
            status = status_filter(alias, request.status),
        );
        let rows = sqlx::query(&sql)
            .bind(tenant_id.to_string())
            .bind(optional_key(request.search.as_deref()))
            .bind(optional_key(request.email.as_deref()))
            .bind(cursor.as_ref().map(|(sort, _)| sort.clone()))
            .bind(cursor.as_ref().map(|(_, id)| id.clone()))
            .bind(filter_path)
            .bind(filter_value)
            .bind(i64::from(request.limit) + 1)
            .bind(request.organisation_id.as_deref())
            .fetch_all(&mut *self.tx)
            .await?;
        let has_more = rows.len() > usize::from(request.limit);
        let mut records = rows.iter().map(decode).collect::<Result<Vec<_>, _>>()?;
        records.truncate(usize::from(request.limit));
        let next_cursor = if has_more {
            let last = records.last().ok_or(ContactsError::Invariant)?;
            Some(
                self.store_cursor(
                    tenant_id,
                    resource,
                    &query_hash,
                    sort_value(last, request.sort),
                    record_id(last),
                )
                .await?,
            )
        } else {
            None
        };
        Ok(Page {
            data: records,
            next_cursor,
        })
    }
}

fn person_sort_value(person: &Person, sort: ContactSort) -> String {
    match sort {
        ContactSort::Name => key(&person.full_name),
        ContactSort::CreatedAt => timestamp(person.created_at),
        ContactSort::UpdatedAt => timestamp(person.updated_at),
    }
}

fn organisation_sort_value(organisation: &Organisation, sort: ContactSort) -> String {
    match sort {
        ContactSort::Name => key(&organisation.name),
        ContactSort::CreatedAt => timestamp(organisation.created_at),
        ContactSort::UpdatedAt => timestamp(organisation.updated_at),
    }
}

#[async_trait]
impl ContactsTx for SqliteContactsTx {
    async fn next_public_id(
        &mut self,
        tenant_id: Uuid,
        construct: &str,
        prefix: &str,
    ) -> Result<String, ContactsError> {
        let sequence: i64 = sqlx::query_scalar(
            "INSERT INTO p_contacts_public_id_counters (tenant_id, construct, next_value) \
             VALUES (?1, ?2, 1) \
             ON CONFLICT (tenant_id, construct) DO UPDATE SET next_value = next_value + 1 \
             RETURNING next_value",
        )
        .bind(tenant_id.to_string())
        .bind(construct)
        .fetch_one(&mut *self.tx)
        .await?;
        let sequence = invariant(u64::try_from(sequence))?;
        invariant(PublicId::new(prefix, sequence)).map(|id| id.as_str().to_owned())
    }

    async fn resolve_organisation_id(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
    ) -> Result<Uuid, ContactsError> {
        let id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM p_contacts_organisations \
             WHERE tenant_id = ?1 AND public_id = ?2 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .fetch_optional(&mut *self.tx)
        .await?;
        invariant(Uuid::parse_str(&id.ok_or(ContactsError::NotFound)?))
    }

    async fn insert_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &CreatePerson,
        organisation_id: Option<Uuid>,
        custom_fields: &BTreeMap<String, Value>,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError> {
        let now = timestamp(Utc::now());
        sqlx::query(
            "INSERT INTO p_contacts_people (tenant_id, id, public_id, full_name, name_key, email, \
               email_key, telephone, organisation_id, tags, custom_fields, extensions, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        )
        .bind(tenant_id.to_string())
        .bind(Uuid::now_v7().to_string())
        .bind(public_id)
        .bind(input.full_name.trim())
        .bind(key(&input.full_name))
        .bind(trimmed(input.email.as_deref()))
        .bind(optional_key(input.email.as_deref()))
        .bind(trimmed(input.telephone.as_deref()))
        .bind(organisation_id.map(|id| id.to_string()))
        .bind(encode(&input.tags)?)
        .bind(encode(custom_fields)?)
        .bind(encode(extensions)?)
        .bind(now)
        .execute(&mut *self.tx)
        .await?;
        self.stamp("p_contacts_people", tenant_id, public_id, true)
            .await?;
        self.person_by_public_id(tenant_id, public_id)
            .await?
            .ok_or(ContactsError::Invariant)
    }

    async fn get_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Person, ContactsError> {
        self.person_by_public_id(tenant_id, public_id)
            .await?
            .filter(|record| include_deleted || record.deleted_at.is_none())
            .ok_or(ContactsError::NotFound)
    }

    async fn set_person_deleted(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
    ) -> Result<Person, ContactsError> {
        let now = timestamp(Utc::now());
        let updated = sqlx::query(
            "UPDATE p_contacts_people SET deleted_at = CASE WHEN ?5 THEN ?4 ELSE NULL END, \
               version = version + 1, updated_at = ?4 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 \
               AND (deleted_at IS NULL) = ?5",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(expected_version)
        .bind(now)
        .bind(deleted)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_people", tenant_id, public_id, !deleted)
                .await?);
        }
        self.stamp("p_contacts_people", tenant_id, public_id, false)
            .await?;
        self.get_person(tenant_id, public_id, true).await
    }

    async fn update_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &UpdatePerson,
        organisation_id: Option<Uuid>,
        custom_fields: &BTreeMap<String, Value>,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError> {
        let updated = sqlx::query(
            "UPDATE p_contacts_people SET full_name = ?4, name_key = ?5, email = ?6, email_key = ?7, \
               telephone = ?8, organisation_id = ?9, tags = ?10, custom_fields = ?11, extensions = ?12, \
               version = version + 1, updated_at = ?13 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(input.expected_version)
        .bind(input.full_name.trim())
        .bind(key(&input.full_name))
        .bind(trimmed(input.email.as_deref()))
        .bind(optional_key(input.email.as_deref()))
        .bind(trimmed(input.telephone.as_deref()))
        .bind(organisation_id.map(|id| id.to_string()))
        .bind(encode(&input.tags)?)
        .bind(encode(custom_fields)?)
        .bind(encode(extensions)?)
        .bind(timestamp(Utc::now()))
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_people", tenant_id, public_id, false)
                .await?);
        }
        self.stamp("p_contacts_people", tenant_id, public_id, false)
            .await?;
        self.get_person(tenant_id, public_id, false).await
    }

    async fn archive_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
    ) -> Result<Person, ContactsError> {
        let now = timestamp(Utc::now());
        let updated = sqlx::query(
            "UPDATE p_contacts_people SET archived_at = ?4, version = version + 1, updated_at = ?4 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(expected_version)
        .bind(now)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_people", tenant_id, public_id, false)
                .await?);
        }
        self.stamp("p_contacts_people", tenant_id, public_id, false)
            .await?;
        self.get_person(tenant_id, public_id, false).await
    }

    async fn update_person_extensions(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError> {
        let updated = sqlx::query(
            "UPDATE p_contacts_people SET extensions = ?4, version = version + 1, updated_at = ?5 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(expected_version)
        .bind(encode(extensions)?)
        .bind(timestamp(Utc::now()))
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_people", tenant_id, public_id, false)
                .await?);
        }
        self.stamp("p_contacts_people", tenant_id, public_id, false)
            .await?;
        self.get_person(tenant_id, public_id, false).await
    }

    async fn list_people(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
    ) -> Result<Page<Person>, ContactsError> {
        self.list(
            tenant_id,
            request,
            "people",
            "p",
            &format!("SELECT {PERSON_COLUMNS} {PERSON_FROM}"),
            "AND (?9 IS NULL OR o.public_id = ?9)",
            person_from_row,
            person_sort_value,
            |person| person.id,
        )
        .await
    }

    async fn insert_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &CreateOrganisation,
        custom_fields: &BTreeMap<String, Value>,
    ) -> Result<Organisation, ContactsError> {
        let now = timestamp(Utc::now());
        sqlx::query(
            "INSERT INTO p_contacts_organisations (tenant_id, id, public_id, name, name_key, email, \
               email_key, telephone, custom_fields, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)",
        )
        .bind(tenant_id.to_string())
        .bind(Uuid::now_v7().to_string())
        .bind(public_id)
        .bind(input.name.trim())
        .bind(key(&input.name))
        .bind(trimmed(input.email.as_deref()))
        .bind(optional_key(input.email.as_deref()))
        .bind(trimmed(input.telephone.as_deref()))
        .bind(encode(custom_fields)?)
        .bind(now)
        .execute(&mut *self.tx)
        .await?;
        self.stamp("p_contacts_organisations", tenant_id, public_id, true)
            .await?;
        self.organisation_by_public_id(tenant_id, public_id)
            .await?
            .ok_or(ContactsError::Invariant)
    }

    async fn get_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Organisation, ContactsError> {
        self.organisation_by_public_id(tenant_id, public_id)
            .await?
            .filter(|record| include_deleted || record.deleted_at.is_none())
            .ok_or(ContactsError::NotFound)
    }

    async fn set_organisation_deleted(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
    ) -> Result<Organisation, ContactsError> {
        let now = timestamp(Utc::now());
        let updated = sqlx::query(
            "UPDATE p_contacts_organisations SET deleted_at = CASE WHEN ?5 THEN ?4 ELSE NULL END, \
               version = version + 1, updated_at = ?4 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 \
               AND (deleted_at IS NULL) = ?5",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(expected_version)
        .bind(now)
        .bind(deleted)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_organisations", tenant_id, public_id, !deleted)
                .await?);
        }
        self.stamp("p_contacts_organisations", tenant_id, public_id, false)
            .await?;
        self.get_organisation(tenant_id, public_id, true).await
    }

    async fn update_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &UpdateOrganisation,
        custom_fields: &BTreeMap<String, Value>,
    ) -> Result<Organisation, ContactsError> {
        let updated = sqlx::query(
            "UPDATE p_contacts_organisations SET name = ?4, name_key = ?5, email = ?6, email_key = ?7, \
               telephone = ?8, custom_fields = ?9, version = version + 1, updated_at = ?10 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(input.expected_version)
        .bind(input.name.trim())
        .bind(key(&input.name))
        .bind(trimmed(input.email.as_deref()))
        .bind(optional_key(input.email.as_deref()))
        .bind(trimmed(input.telephone.as_deref()))
        .bind(encode(custom_fields)?)
        .bind(timestamp(Utc::now()))
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_organisations", tenant_id, public_id, false)
                .await?);
        }
        self.stamp("p_contacts_organisations", tenant_id, public_id, false)
            .await?;
        self.get_organisation(tenant_id, public_id, false).await
    }

    async fn archive_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
    ) -> Result<Organisation, ContactsError> {
        let now = timestamp(Utc::now());
        let updated = sqlx::query(
            "UPDATE p_contacts_organisations SET archived_at = ?4, version = version + 1, updated_at = ?4 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(expected_version)
        .bind(now)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_organisations", tenant_id, public_id, false)
                .await?);
        }
        self.stamp("p_contacts_organisations", tenant_id, public_id, false)
            .await?;
        self.get_organisation(tenant_id, public_id, false).await
    }

    async fn update_organisation_extensions(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        extensions: &ExtensionValues,
    ) -> Result<Organisation, ContactsError> {
        let updated = sqlx::query(
            "UPDATE p_contacts_organisations SET extensions = ?4, version = version + 1, updated_at = ?5 \
             WHERE tenant_id = ?1 AND public_id = ?2 AND version = ?3 AND archived_at IS NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(public_id)
        .bind(expected_version)
        .bind(encode(extensions)?)
        .bind(timestamp(Utc::now()))
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify_record("p_contacts_organisations", tenant_id, public_id, false)
                .await?);
        }
        self.stamp("p_contacts_organisations", tenant_id, public_id, false)
            .await?;
        self.get_organisation(tenant_id, public_id, false).await
    }

    async fn list_organisations(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
    ) -> Result<Page<Organisation>, ContactsError> {
        if request.organisation_id.is_some() {
            return Err(ContactsError::validation(
                "organisation_id",
                "query.filter_unsupported",
                "organisation_id is supported only when listing people",
            ));
        }
        self.list(
            tenant_id,
            request,
            "organisations",
            "o",
            &format!("SELECT {ORGANISATION_COLUMNS} FROM p_contacts_organisations o"),
            // Organisations have no organisation filter; the parameter is unset.
            "AND ?9 IS NULL",
            organisation_from_row,
            organisation_sort_value,
            |organisation| organisation.id,
        )
        .await
    }

    async fn activity(
        &mut self,
        tenant_id: Uuid,
        entity_type: &str,
        public_id: &str,
    ) -> Result<Vec<ActivityEntry>, ContactsError> {
        let rows = sqlx::query(
            "SELECT event_id, action, entity_version, actor_id, occurred_at FROM p_contacts_activity \
             WHERE tenant_id = ?1 AND entity_type = ?2 AND entity_public_id = ?3 \
             ORDER BY occurred_at DESC, event_id DESC LIMIT ?4",
        )
        .bind(tenant_id.to_string())
        .bind(entity_type)
        .bind(public_id)
        .bind(ACTIVITY_PAGE_LIMIT)
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(ActivityEntry {
                    event_id: uuid(row, "event_id")?,
                    action: row.try_get("action")?,
                    entity_version: row.try_get("entity_version")?,
                    actor_id: optional_uuid(row, "actor_id")?,
                    occurred_at: time(row, "occurred_at")?,
                })
            })
            .collect()
    }

    async fn definitions(
        &mut self,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
    ) -> Result<Vec<ContactCustomFieldDefinition>, ContactsError> {
        let rows = sqlx::query(&format!(
            "SELECT {DEFINITION_COLUMNS} FROM p_contacts_custom_field_definitions \
             WHERE tenant_id = ?1 AND applies_to = ?2 ORDER BY api_name"
        ))
        .bind(tenant_id.to_string())
        .bind(target.as_str())
        .fetch_all(&mut *self.tx)
        .await?;
        rows.iter().map(definition_from_row).collect()
    }

    async fn get_definition(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let row = sqlx::query(&format!(
            "SELECT {DEFINITION_COLUMNS} FROM p_contacts_custom_field_definitions \
             WHERE tenant_id = ?1 AND id = ?2"
        ))
        .bind(tenant_id.to_string())
        .bind(id.to_string())
        .fetch_optional(&mut *self.tx)
        .await?
        .ok_or(ContactsError::NotFound)?;
        definition_from_row(&row)
    }

    async fn insert_definition(
        &mut self,
        tenant_id: Uuid,
        input: &CreateCustomFieldDefinition,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let id = Uuid::now_v7();
        let now = timestamp(Utc::now());
        sqlx::query(
            "INSERT INTO p_contacts_custom_field_definitions (tenant_id, id, applies_to, api_name, \
               label, description, field_type, required, unique_per_tenant, indexed, options, \
               validation_schema, data_classification, purpose, retention_days, encrypted, \
               created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17)",
        )
        .bind(tenant_id.to_string())
        .bind(id.to_string())
        .bind(input.applies_to.as_str())
        .bind(&input.api_name)
        .bind(input.label.trim())
        .bind(trimmed(input.description.as_deref()))
        .bind(input.field_type.as_str())
        .bind(input.required)
        .bind(input.unique_per_tenant)
        .bind(input.indexed)
        .bind(encode(&input.options)?)
        .bind(encode(&input.validation_schema)?)
        .bind(input.data_classification.as_str())
        .bind(trimmed(input.purpose.as_deref()))
        .bind(input.retention_days.map(i64::from))
        .bind(input.encrypted)
        .bind(now)
        .execute(&mut *self.tx)
        .await?;
        self.get_definition(tenant_id, id).await
    }

    async fn update_definition(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
        input: &UpdateCustomFieldDefinition,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let updated = sqlx::query(
            "UPDATE p_contacts_custom_field_definitions SET label = ?4, description = ?5, \
               required = ?6, unique_per_tenant = ?7, indexed = ?8, options = ?9, \
               validation_schema = ?10, data_classification = ?11, purpose = ?12, \
               retention_days = ?13, encrypted = ?14, version = version + 1, updated_at = ?15 \
             WHERE tenant_id = ?1 AND id = ?2 AND version = ?3 AND archived_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(id.to_string())
        .bind(input.expected_version)
        .bind(input.label.trim())
        .bind(trimmed(input.description.as_deref()))
        .bind(input.required)
        .bind(input.unique_per_tenant)
        .bind(input.indexed)
        .bind(encode(&input.options)?)
        .bind(encode(&input.validation_schema)?)
        .bind(input.data_classification.as_str())
        .bind(trimmed(input.purpose.as_deref()))
        .bind(input.retention_days.map(i64::from))
        .bind(input.encrypted)
        .bind(timestamp(Utc::now()))
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify(
                    "p_contacts_custom_field_definitions",
                    "id",
                    tenant_id,
                    &id.to_string(),
                )
                .await?);
        }
        self.get_definition(tenant_id, id).await
    }

    async fn set_definition_status(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
        expected_version: i64,
        archive: bool,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let now = timestamp(Utc::now());
        let updated = sqlx::query(
            "UPDATE p_contacts_custom_field_definitions \
             SET disabled_at = COALESCE(disabled_at, ?4), \
                 archived_at = CASE WHEN ?5 THEN COALESCE(archived_at, ?4) ELSE archived_at END, \
                 version = version + 1, updated_at = ?4 \
             WHERE tenant_id = ?1 AND id = ?2 AND version = ?3 AND archived_at IS NULL",
        )
        .bind(tenant_id.to_string())
        .bind(id.to_string())
        .bind(expected_version)
        .bind(now)
        .bind(archive)
        .execute(&mut *self.tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(self
                .classify(
                    "p_contacts_custom_field_definitions",
                    "id",
                    tenant_id,
                    &id.to_string(),
                )
                .await?);
        }
        self.get_definition(tenant_id, id).await
    }

    async fn custom_value_exists(
        &mut self,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
        api_name: &str,
        value: &Value,
        exclude_id: Option<Uuid>,
    ) -> Result<bool, ContactsError> {
        // The transaction began with BEGIN IMMEDIATE, so it already holds the
        // workspace file's write lock: the check and the write that follows
        // cannot interleave with another writer.
        let table = match target {
            ContactCustomFieldTarget::Person => "p_contacts_people",
            ContactCustomFieldTarget::Organisation => "p_contacts_organisations",
        };
        Ok(sqlx::query_scalar(&format!(
            "SELECT EXISTS (SELECT 1 FROM {table} WHERE tenant_id = ?1 \
               AND json_extract(custom_fields, ?2) = json_extract(?3, '$') \
               AND (?4 IS NULL OR id <> ?4))"
        ))
        .bind(tenant_id.to_string())
        .bind(json_path(&[api_name]))
        .bind(value.to_string())
        .bind(exclude_id.map(|id| id.to_string()))
        .fetch_one(&mut *self.tx)
        .await?)
    }

    async fn append_evidence(
        &mut self,
        audit: &AuditEvent,
        event: &EventEnvelope,
    ) -> Result<(), ContactsError> {
        append_audit(&mut self.tx, audit).await?;
        append_outbox(&mut self.tx, event).await?;
        Ok(())
    }

    async fn commit(self: Box<Self>) -> Result<(), ContactsError> {
        self.tx.commit().await?;
        Ok(())
    }
}
