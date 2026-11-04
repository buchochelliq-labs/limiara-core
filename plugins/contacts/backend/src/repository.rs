use crate::model::*;
use crate::ContactsError;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use platform_id::PublicId;
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::{FromRow, Postgres, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, FromRow)]
struct PersonRow {
    id: Uuid,
    public_id: String,
    tenant_id: Uuid,
    full_name: String,
    email: Option<String>,
    telephone: Option<String>,
    organisation_id: Option<Uuid>,
    organisation_public_id: Option<String>,
    tags: Vec<String>,
    custom_fields: Value,
    extensions: Value,
    version: i64,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
    touched_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct OrganisationRow {
    id: Uuid,
    public_id: String,
    tenant_id: Uuid,
    name: String,
    email: Option<String>,
    telephone: Option<String>,
    custom_fields: Value,
    extensions: Value,
    version: i64,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
    touched_at: DateTime<Utc>,
    deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, FromRow)]
struct DefinitionRow {
    id: Uuid,
    tenant_id: Uuid,
    applies_to: String,
    api_name: String,
    label: String,
    description: Option<String>,
    field_type: String,
    required: bool,
    unique_per_tenant: bool,
    indexed: bool,
    options: Value,
    validation_schema: Value,
    data_classification: String,
    purpose: Option<String>,
    retention_days: Option<i32>,
    encrypted: bool,
    version: i64,
    disabled_at: Option<DateTime<Utc>>,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Debug)]
struct CursorState {
    sort_value: String,
    record_id: Uuid,
}

pub(crate) async fn next_public_id(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    construct: &str,
    prefix: &str,
) -> Result<String, ContactsError> {
    let sequence: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO p_contacts.public_id_counters (tenant_id, construct, next_value)
        VALUES ($1, $2, 1)
        ON CONFLICT (tenant_id, construct) DO UPDATE
          SET next_value = p_contacts.public_id_counters.next_value + 1
        RETURNING next_value
        "#,
    )
    .bind(tenant_id)
    .bind(construct)
    .fetch_one(&mut **tx)
    .await?;
    let sequence = u64::try_from(sequence).map_err(|_| ContactsError::Invariant)?;
    PublicId::new(prefix, sequence)
        .map(|id| id.as_str().to_owned())
        .map_err(|_| ContactsError::Invariant)
}

pub(crate) async fn resolve_organisation_id(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    public_id: &str,
) -> Result<Uuid, ContactsError> {
    sqlx::query_scalar(
        r#"
        SELECT id FROM p_contacts.organisations
        WHERE tenant_id = $1 AND public_id = $2 AND archived_at IS NULL AND deleted_at IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ContactsError::NotFound)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn insert_person(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    input: &CreatePerson,
    organisation_id: Option<Uuid>,
    custom_fields: &BTreeMap<String, Value>,
    extensions: &platform_extensions::ExtensionValues,
) -> Result<Person, ContactsError> {
    let row = sqlx::query_as::<_, PersonRow>(
        r#"
        WITH inserted AS (
          INSERT INTO p_contacts.people (
            tenant_id, id, public_id, full_name, email, telephone,
            organisation_id, tags, custom_fields, extensions, name_key, email_key,
            created_by, updated_by
          ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $13)
          RETURNING *
        )
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM inserted p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        "#,
    )
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(public_id)
    .bind(input.full_name.trim())
    .bind(input.email.as_deref().map(str::trim))
    .bind(input.telephone.as_deref().map(str::trim))
    .bind(organisation_id)
    .bind(&input.tags)
    .bind(serde_json::to_value(custom_fields).map_err(|_| ContactsError::Invariant)?)
    .bind(serde_json::to_value(extensions).map_err(|_| ContactsError::Invariant)?)
    .bind(search_key(&input.full_name))
    .bind(normalize_optional(input.email.as_deref()))
    .bind(actor)
    .fetch_one(&mut **tx)
    .await?;
    person_from_row(row)
}

type ActivityRow = (Uuid, String, i64, Option<Uuid>, DateTime<Utc>);

pub(crate) async fn activity(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    entity_type: &str,
    public_id: &str,
) -> Result<Vec<ActivityEntry>, ContactsError> {
    let rows: Vec<ActivityRow> = sqlx::query_as(
        r#"
        SELECT event_id, action, entity_version, actor_id, occurred_at
        FROM p_contacts.activity
        WHERE tenant_id = $1 AND entity_type = $2 AND entity_public_id = $3
        ORDER BY occurred_at DESC, event_id DESC
        LIMIT $4
        "#,
    )
    .bind(tenant_id)
    .bind(entity_type)
    .bind(public_id)
    .bind(ACTIVITY_PAGE_LIMIT)
    .fetch_all(&mut **tx)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(event_id, action, entity_version, actor_id, occurred_at)| ActivityEntry {
                event_id,
                action,
                entity_version,
                actor_id,
                occurred_at,
            },
        )
        .collect())
}

pub(crate) async fn get_person(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    public_id: &str,
    include_deleted: bool,
) -> Result<Person, ContactsError> {
    let row = sqlx::query_as::<_, PersonRow>(
        r#"
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM p_contacts.people p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        WHERE p.tenant_id = $1 AND p.public_id = $2 AND ($3 OR p.deleted_at IS NULL)
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(include_deleted)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ContactsError::NotFound)?;
    person_from_row(row)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn update_person(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    input: &UpdatePerson,
    organisation_id: Option<Uuid>,
    custom_fields: &BTreeMap<String, Value>,
    extensions: &platform_extensions::ExtensionValues,
) -> Result<Person, ContactsError> {
    let row = sqlx::query_as::<_, PersonRow>(
        r#"
        WITH updated AS (
          UPDATE p_contacts.people
          SET full_name = $4, email = $5, telephone = $6, organisation_id = $7,
              tags = $8, custom_fields = $9, extensions = $10,
              name_key = $11, email_key = $12,
              version = version + 1, updated_at = now(),
              updated_by = $13, touched_at = now()
          WHERE tenant_id = $1 AND public_id = $2 AND version = $3 AND archived_at IS NULL AND deleted_at IS NULL
          RETURNING *
        )
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM updated p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(input.expected_version)
    .bind(input.full_name.trim())
    .bind(input.email.as_deref().map(str::trim))
    .bind(input.telephone.as_deref().map(str::trim))
    .bind(organisation_id)
    .bind(&input.tags)
    .bind(serde_json::to_value(custom_fields).map_err(|_| ContactsError::Invariant)?)
    .bind(serde_json::to_value(extensions).map_err(|_| ContactsError::Invariant)?)
    .bind(search_key(&input.full_name))
    .bind(normalize_optional(input.email.as_deref()))
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => person_from_row(row),
        None => Err(classify_person_write_failure(tx, tenant_id, public_id, false).await?),
    }
}

pub(crate) async fn archive_person(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    expected_version: i64,
) -> Result<Person, ContactsError> {
    let row = sqlx::query_as::<_, PersonRow>(
        r#"
        WITH archived AS (
          UPDATE p_contacts.people
          SET archived_at = now(), version = version + 1, updated_at = now(),
              updated_by = $4, touched_at = now()
          WHERE tenant_id = $1 AND public_id = $2 AND version = $3 AND archived_at IS NULL AND deleted_at IS NULL
          RETURNING *
        )
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM archived p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(expected_version)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => person_from_row(row),
        None => Err(classify_person_write_failure(tx, tenant_id, public_id, false).await?),
    }
}

/// Why a guarded write matched no row: the record exists in the expected
/// state (`in_bin` for undelete) but at another version, or it does not.
async fn classify_person_write_failure(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    public_id: &str,
    in_bin: bool,
) -> Result<ContactsError, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM p_contacts.people \
         WHERE tenant_id = $1 AND public_id = $2 AND (deleted_at IS NOT NULL) = $3)",
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(in_bin)
    .fetch_one(&mut **tx)
    .await?;
    Ok(if exists {
        ContactsError::VersionConflict
    } else {
        ContactsError::NotFound
    })
}

pub(crate) async fn insert_organisation(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    input: &CreateOrganisation,
    custom_fields: &BTreeMap<String, Value>,
) -> Result<Organisation, ContactsError> {
    let row = sqlx::query_as::<_, OrganisationRow>(
        r#"
        INSERT INTO p_contacts.organisations (
          tenant_id, id, public_id, name, email, telephone, custom_fields, name_key, email_key,
          created_by, updated_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $10)
        RETURNING id, public_id, tenant_id, name, email, telephone, custom_fields,
                  extensions, version, archived_at, created_at, updated_at,
                  created_by, updated_by, touched_at, deleted_at
        "#,
    )
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(public_id)
    .bind(input.name.trim())
    .bind(input.email.as_deref().map(str::trim))
    .bind(input.telephone.as_deref().map(str::trim))
    .bind(serde_json::to_value(custom_fields).map_err(|_| ContactsError::Invariant)?)
    .bind(search_key(&input.name))
    .bind(normalize_optional(input.email.as_deref()))
    .bind(actor)
    .fetch_one(&mut **tx)
    .await?;
    organisation_from_row(row)
}

pub(crate) async fn get_organisation(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    public_id: &str,
    include_deleted: bool,
) -> Result<Organisation, ContactsError> {
    let row = sqlx::query_as::<_, OrganisationRow>(
        r#"
        SELECT id, public_id, tenant_id, name, email, telephone, custom_fields,
               extensions, version, archived_at, created_at, updated_at,
                  created_by, updated_by, touched_at, deleted_at
        FROM p_contacts.organisations
        WHERE tenant_id = $1 AND public_id = $2 AND ($3 OR deleted_at IS NULL)
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(include_deleted)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ContactsError::NotFound)?;
    organisation_from_row(row)
}

pub(crate) async fn update_organisation(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    input: &UpdateOrganisation,
    custom_fields: &BTreeMap<String, Value>,
) -> Result<Organisation, ContactsError> {
    let row = sqlx::query_as::<_, OrganisationRow>(
        r#"
        UPDATE p_contacts.organisations
        SET name = $4, email = $5, telephone = $6, custom_fields = $7,
            name_key = $8, email_key = $9,
            version = version + 1, updated_at = now(),
              updated_by = $10, touched_at = now()
        WHERE tenant_id = $1 AND public_id = $2 AND version = $3 AND archived_at IS NULL AND deleted_at IS NULL
        RETURNING id, public_id, tenant_id, name, email, telephone, custom_fields,
                  extensions, version, archived_at, created_at, updated_at,
                  created_by, updated_by, touched_at, deleted_at
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(input.expected_version)
    .bind(input.name.trim())
    .bind(input.email.as_deref().map(str::trim))
    .bind(input.telephone.as_deref().map(str::trim))
    .bind(serde_json::to_value(custom_fields).map_err(|_| ContactsError::Invariant)?)
    .bind(search_key(&input.name))
    .bind(normalize_optional(input.email.as_deref()))
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => organisation_from_row(row),
        None => Err(classify_organisation_write_failure(tx, tenant_id, public_id, false).await?),
    }
}

pub(crate) async fn archive_organisation(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    expected_version: i64,
) -> Result<Organisation, ContactsError> {
    let row = sqlx::query_as::<_, OrganisationRow>(
        r#"
        UPDATE p_contacts.organisations
        SET archived_at = now(), version = version + 1, updated_at = now(),
              updated_by = $4, touched_at = now()
        WHERE tenant_id = $1 AND public_id = $2 AND version = $3 AND archived_at IS NULL AND deleted_at IS NULL
        RETURNING id, public_id, tenant_id, name, email, telephone, custom_fields,
                  extensions, version, archived_at, created_at, updated_at,
                  created_by, updated_by, touched_at, deleted_at
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(expected_version)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => organisation_from_row(row),
        None => Err(classify_organisation_write_failure(tx, tenant_id, public_id, false).await?),
    }
}

/// Moves a person to the recycle bin (`deleted`) or restores one from it.
/// Either way it is a member's edit (ADR 0014).
pub(crate) async fn set_person_deleted(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    expected_version: i64,
    deleted: bool,
) -> Result<Person, ContactsError> {
    let row = sqlx::query_as::<_, PersonRow>(
        r#"
        WITH changed AS (
          UPDATE p_contacts.people
          SET deleted_at = CASE WHEN $5 THEN now() ELSE NULL END,
              version = version + 1, updated_at = now(),
              updated_by = $4, touched_at = now()
          WHERE tenant_id = $1 AND public_id = $2 AND version = $3
            AND (deleted_at IS NULL) = $5
          RETURNING *
        )
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM changed p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(expected_version)
    .bind(actor)
    .bind(deleted)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => person_from_row(row),
        None => Err(classify_person_write_failure(tx, tenant_id, public_id, !deleted).await?),
    }
}

/// Moves an organisation to the recycle bin or restores one from it.
pub(crate) async fn set_organisation_deleted(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    expected_version: i64,
    deleted: bool,
) -> Result<Organisation, ContactsError> {
    let row = sqlx::query_as::<_, OrganisationRow>(
        r#"
        UPDATE p_contacts.organisations
        SET deleted_at = CASE WHEN $5 THEN now() ELSE NULL END,
            version = version + 1, updated_at = now(),
            updated_by = $4, touched_at = now()
        WHERE tenant_id = $1 AND public_id = $2 AND version = $3
          AND (deleted_at IS NULL) = $5
        RETURNING id, public_id, tenant_id, name, email, telephone, custom_fields,
                  extensions, version, archived_at, created_at, updated_at,
                  created_by, updated_by, touched_at, deleted_at
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(expected_version)
    .bind(actor)
    .bind(deleted)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => organisation_from_row(row),
        None => Err(classify_organisation_write_failure(tx, tenant_id, public_id, !deleted).await?),
    }
}

/// Why a guarded write matched no row: the record exists in the expected
/// state (`in_bin` for undelete) but at another version, or it does not.
async fn classify_organisation_write_failure(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    public_id: &str,
    in_bin: bool,
) -> Result<ContactsError, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM p_contacts.organisations \
         WHERE tenant_id = $1 AND public_id = $2 AND (deleted_at IS NOT NULL) = $3)",
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(in_bin)
    .fetch_one(&mut **tx)
    .await?;
    Ok(if exists {
        ContactsError::VersionConflict
    } else {
        ContactsError::NotFound
    })
}

pub(crate) async fn list_people(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    request: &ListRequest,
) -> Result<Page<Person>, ContactsError> {
    let query_hash = query_hash("people", request)?;
    let cursor = resolve_cursor(
        tx,
        tenant_id,
        "people",
        &query_hash,
        request.cursor.as_deref(),
    )
    .await?;
    let (sort_expression, cursor_cast) = match request.sort {
        ContactSort::Name => ("coalesce(p.name_key, lower(p.full_name))", "$5::text"),
        ContactSort::CreatedAt => ("p.created_at", "$5::timestamptz"),
        ContactSort::UpdatedAt => ("p.updated_at", "$5::timestamptz"),
    };
    let operator = if request.direction == SortDirection::Asc {
        ">"
    } else {
        "<"
    };
    let direction = if request.direction == SortDirection::Asc {
        "ASC"
    } else {
        "DESC"
    };
    let sql = format!(
        r#"
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM p_contacts.people p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        WHERE p.tenant_id = $1
          AND ($2::text IS NULL OR position($2 in coalesce(p.name_key, lower(p.full_name))) > 0)
          AND ($3::text IS NULL OR coalesce(p.email_key, lower(p.email)) = $3)
          AND ($4::text IS NULL OR o.public_id = $4)
          AND ($8::jsonb IS NULL OR p.extensions @> $8)
          AND ({status_filter})
          AND ({cursor_cast} IS NULL OR ({sort_expression}, p.id) {operator} ({cursor_cast}, $6))
        ORDER BY {sort_expression} {direction}, p.id {direction}
        LIMIT $7
        "#,
        status_filter = status_filter("p", request.status),
    );
    let search = normalize_optional(request.search.as_deref());
    let email = normalize_optional(request.email.as_deref());
    let mut rows = sqlx::query_as::<_, PersonRow>(&sql)
        .bind(tenant_id)
        .bind(search)
        .bind(email)
        .bind(request.organisation_id.as_deref())
        .bind(cursor.as_ref().map(|cursor| cursor.sort_value.as_str()))
        .bind(cursor.as_ref().map(|cursor| cursor.record_id))
        .bind(i64::from(request.limit) + 1)
        .bind(extension_containment(request))
        .fetch_all(&mut **tx)
        .await?;
    let has_more = rows.len() > usize::from(request.limit);
    rows.truncate(usize::from(request.limit));
    let people = rows
        .into_iter()
        .map(person_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if has_more {
        let last = people.last().ok_or(ContactsError::Invariant)?;
        Some(
            store_cursor(
                tx,
                tenant_id,
                "people",
                &query_hash,
                person_sort_value(last, request.sort),
                last.id,
            )
            .await?,
        )
    } else {
        None
    };
    Ok(Page {
        data: people,
        next_cursor,
    })
}

pub(crate) async fn list_organisations(
    tx: &mut Transaction<'_, Postgres>,
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
    let query_hash = query_hash("organisations", request)?;
    let cursor = resolve_cursor(
        tx,
        tenant_id,
        "organisations",
        &query_hash,
        request.cursor.as_deref(),
    )
    .await?;
    let (sort_expression, cursor_cast) = match request.sort {
        ContactSort::Name => ("coalesce(o.name_key, lower(o.name))", "$4::text"),
        ContactSort::CreatedAt => ("o.created_at", "$4::timestamptz"),
        ContactSort::UpdatedAt => ("o.updated_at", "$4::timestamptz"),
    };
    let operator = if request.direction == SortDirection::Asc {
        ">"
    } else {
        "<"
    };
    let direction = if request.direction == SortDirection::Asc {
        "ASC"
    } else {
        "DESC"
    };
    let sql = format!(
        r#"
        SELECT o.id, o.public_id, o.tenant_id, o.name, o.email, o.telephone,
               o.custom_fields, o.extensions, o.version, o.archived_at, o.created_at, o.updated_at,
               o.created_by, o.updated_by, o.touched_at, o.deleted_at
        FROM p_contacts.organisations o
        WHERE o.tenant_id = $1
          AND ($2::text IS NULL OR position($2 in coalesce(o.name_key, lower(o.name))) > 0)
          AND ($3::text IS NULL OR coalesce(o.email_key, lower(o.email)) = $3)
          AND ($7::jsonb IS NULL OR o.extensions @> $7)
          AND ({status_filter})
          AND ({cursor_cast} IS NULL OR ({sort_expression}, o.id) {operator} ({cursor_cast}, $5))
        ORDER BY {sort_expression} {direction}, o.id {direction}
        LIMIT $6
        "#,
        status_filter = status_filter("o", request.status),
    );
    let mut rows = sqlx::query_as::<_, OrganisationRow>(&sql)
        .bind(tenant_id)
        .bind(normalize_optional(request.search.as_deref()))
        .bind(normalize_optional(request.email.as_deref()))
        .bind(cursor.as_ref().map(|cursor| cursor.sort_value.as_str()))
        .bind(cursor.as_ref().map(|cursor| cursor.record_id))
        .bind(i64::from(request.limit) + 1)
        .bind(extension_containment(request))
        .fetch_all(&mut **tx)
        .await?;
    let has_more = rows.len() > usize::from(request.limit);
    rows.truncate(usize::from(request.limit));
    let organisations = rows
        .into_iter()
        .map(organisation_from_row)
        .collect::<Result<Vec<_>, _>>()?;
    let next_cursor = if has_more {
        let last = organisations.last().ok_or(ContactsError::Invariant)?;
        Some(
            store_cursor(
                tx,
                tenant_id,
                "organisations",
                &query_hash,
                organisation_sort_value(last, request.sort),
                last.id,
            )
            .await?,
        )
    } else {
        None
    };
    Ok(Page {
        data: organisations,
        next_cursor,
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
        // The recycle bin (ADR 0014): only deleted records.
        RecordStatus::Deleted => format!("{alias}.deleted_at IS NOT NULL"),
    }
}

/// The Unicode-lowercased key names are searched and sorted by, computed here
/// so it does not depend on the database locale.
fn search_key(value: &str) -> String {
    value.trim().to_lowercase()
}

fn normalize_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
}

fn person_sort_value(person: &Person, sort: ContactSort) -> String {
    match sort {
        ContactSort::Name => search_key(&person.full_name),
        ContactSort::CreatedAt => person.created_at.to_rfc3339(),
        ContactSort::UpdatedAt => person.updated_at.to_rfc3339(),
    }
}

fn organisation_sort_value(organisation: &Organisation, sort: ContactSort) -> String {
    match sort {
        ContactSort::Name => search_key(&organisation.name),
        ContactSort::CreatedAt => organisation.created_at.to_rfc3339(),
        ContactSort::UpdatedAt => organisation.updated_at.to_rfc3339(),
    }
}

fn query_hash(resource: &str, request: &ListRequest) -> Result<String, ContactsError> {
    let mut request = request.clone();
    request.cursor = None;
    // The extension filter is not serialised with the request, so it joins
    // the cursor identity explicitly: a cursor never crosses filters.
    let extension_filter = request
        .extension_filter
        .as_ref()
        .map(platform_extensions::ValidatedExtensionFilter::identity);
    let document = serde_json::to_vec(&(resource, request, extension_filter))
        .map_err(|_| ContactsError::Invariant)?;
    Ok(format!("{:x}", Sha256::digest(document)))
}

fn extension_containment(request: &ListRequest) -> Option<Value> {
    request
        .extension_filter
        .as_ref()
        .map(platform_extensions::ValidatedExtensionFilter::containment)
}

/// Replaces a person's extension values after the kernel validated the patch
/// and the service applied it to the current values.
pub(crate) async fn update_person_extensions(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    expected_version: i64,
    extensions: &platform_extensions::ExtensionValues,
) -> Result<Person, ContactsError> {
    let row = sqlx::query_as::<_, PersonRow>(
        r#"
        WITH updated AS (
          UPDATE p_contacts.people
          SET extensions = $4, version = version + 1, updated_at = now(),
              updated_by = $5, touched_at = now()
          WHERE tenant_id = $1 AND public_id = $2 AND version = $3 AND archived_at IS NULL AND deleted_at IS NULL
          RETURNING *
        )
        SELECT p.id, p.public_id, p.tenant_id, p.full_name, p.email, p.telephone,
               p.organisation_id, o.public_id AS organisation_public_id, p.tags,
               p.custom_fields, p.extensions, p.version, p.archived_at, p.created_at, p.updated_at,
               p.created_by, p.updated_by, p.touched_at, p.deleted_at
        FROM updated p
        LEFT JOIN p_contacts.organisations o
          ON o.tenant_id = p.tenant_id AND o.id = p.organisation_id
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(expected_version)
    .bind(serde_json::to_value(extensions).map_err(|_| ContactsError::Invariant)?)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => person_from_row(row),
        None => Err(classify_person_write_failure(tx, tenant_id, public_id, false).await?),
    }
}

/// Replaces an organisation's extension values; see `update_person_extensions`.
pub(crate) async fn update_organisation_extensions(
    tx: &mut Transaction<'_, Postgres>,
    actor: Option<Uuid>,
    tenant_id: Uuid,
    public_id: &str,
    expected_version: i64,
    extensions: &platform_extensions::ExtensionValues,
) -> Result<Organisation, ContactsError> {
    let row = sqlx::query_as::<_, OrganisationRow>(
        r#"
        UPDATE p_contacts.organisations
        SET extensions = $4, version = version + 1, updated_at = now(),
              updated_by = $5, touched_at = now()
        WHERE tenant_id = $1 AND public_id = $2 AND version = $3 AND archived_at IS NULL AND deleted_at IS NULL
        RETURNING id, public_id, tenant_id, name, email, telephone, custom_fields,
                  extensions, version, archived_at, created_at, updated_at,
                  created_by, updated_by, touched_at, deleted_at
        "#,
    )
    .bind(tenant_id)
    .bind(public_id)
    .bind(expected_version)
    .bind(serde_json::to_value(extensions).map_err(|_| ContactsError::Invariant)?)
    .bind(actor)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => organisation_from_row(row),
        None => Err(classify_organisation_write_failure(tx, tenant_id, public_id, false).await?),
    }
}

async fn resolve_cursor(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    resource: &str,
    query_hash: &str,
    token: Option<&str>,
) -> Result<Option<CursorState>, ContactsError> {
    let Some(token) = token else { return Ok(None) };
    let token_hash = format!("{:x}", Sha256::digest(token.as_bytes()));
    let row: Option<(String, Uuid)> = sqlx::query_as(
        r#"
        SELECT sort_value, record_id FROM p_contacts.list_cursors
        WHERE token_hash = $1 AND tenant_id = $2 AND resource = $3
          AND query_hash = $4 AND expires_at > now()
        "#,
    )
    .bind(token_hash)
    .bind(tenant_id)
    .bind(resource)
    .bind(query_hash)
    .fetch_optional(&mut **tx)
    .await?;
    row.map(|(sort_value, record_id)| CursorState {
        sort_value,
        record_id,
    })
    .ok_or(ContactsError::InvalidCursor)
    .map(Some)
}

async fn store_cursor(
    tx: &mut Transaction<'_, Postgres>,
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
        r#"
        INSERT INTO p_contacts.list_cursors (
          token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at
        ) VALUES ($1, $2, $3, $4, $5, $6, now() + interval '1 hour')
        "#,
    )
    .bind(token_hash)
    .bind(tenant_id)
    .bind(resource)
    .bind(query_hash)
    .bind(sort_value)
    .bind(record_id)
    .execute(&mut **tx)
    .await?;
    Ok(token)
}

pub(crate) async fn definitions(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    target: ContactCustomFieldTarget,
) -> Result<Vec<ContactCustomFieldDefinition>, ContactsError> {
    let rows = sqlx::query_as::<_, DefinitionRow>(
        r#"
        SELECT id, tenant_id, applies_to, api_name, label, description, field_type,
               required, unique_per_tenant, indexed, options, validation_schema,
               data_classification, purpose, retention_days, encrypted, version,
               disabled_at, archived_at, created_at, updated_at
        FROM p_contacts.custom_field_definitions
        WHERE tenant_id = $1 AND applies_to = $2
        ORDER BY api_name
        "#,
    )
    .bind(tenant_id)
    .bind(target.as_str())
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(definition_from_row).collect()
}

pub(crate) async fn get_definition(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<ContactCustomFieldDefinition, ContactsError> {
    let row = sqlx::query_as::<_, DefinitionRow>(
        r#"
        SELECT id, tenant_id, applies_to, api_name, label, description, field_type,
               required, unique_per_tenant, indexed, options, validation_schema,
               data_classification, purpose, retention_days, encrypted, version,
               disabled_at, archived_at, created_at, updated_at
        FROM p_contacts.custom_field_definitions
        WHERE tenant_id = $1 AND id = $2
        "#,
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(ContactsError::NotFound)?;
    definition_from_row(row)
}

pub(crate) async fn insert_definition(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    input: &CreateCustomFieldDefinition,
) -> Result<ContactCustomFieldDefinition, ContactsError> {
    let row = sqlx::query_as::<_, DefinitionRow>(
        r#"
        INSERT INTO p_contacts.custom_field_definitions (
          tenant_id, id, applies_to, api_name, label, description, field_type,
          required, unique_per_tenant, indexed, options, validation_schema,
          data_classification, purpose, retention_days, encrypted
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16)
        RETURNING id, tenant_id, applies_to, api_name, label, description, field_type,
                  required, unique_per_tenant, indexed, options, validation_schema,
                  data_classification, purpose, retention_days, encrypted, version,
                  disabled_at, archived_at, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(input.applies_to.as_str())
    .bind(&input.api_name)
    .bind(input.label.trim())
    .bind(input.description.as_deref().map(str::trim))
    .bind(input.field_type.as_str())
    .bind(input.required)
    .bind(input.unique_per_tenant)
    .bind(input.indexed)
    .bind(serde_json::to_value(&input.options).map_err(|_| ContactsError::Invariant)?)
    .bind(&input.validation_schema)
    .bind(input.data_classification.as_str())
    .bind(input.purpose.as_deref().map(str::trim))
    .bind(input.retention_days.map(|days| days as i32))
    .bind(input.encrypted)
    .fetch_one(&mut **tx)
    .await?;
    definition_from_row(row)
}

pub(crate) async fn update_definition(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    id: Uuid,
    input: &UpdateCustomFieldDefinition,
) -> Result<ContactCustomFieldDefinition, ContactsError> {
    let row = sqlx::query_as::<_, DefinitionRow>(
        r#"
        UPDATE p_contacts.custom_field_definitions
        SET label = $4, description = $5, required = $6, unique_per_tenant = $7,
            indexed = $8, options = $9, validation_schema = $10,
            data_classification = $11, purpose = $12, retention_days = $13,
            encrypted = $14, version = version + 1, updated_at = now()
        WHERE tenant_id = $1 AND id = $2 AND version = $3 AND archived_at IS NULL
        RETURNING id, tenant_id, applies_to, api_name, label, description, field_type,
                  required, unique_per_tenant, indexed, options, validation_schema,
                  data_classification, purpose, retention_days, encrypted, version,
                  disabled_at, archived_at, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(id)
    .bind(input.expected_version)
    .bind(input.label.trim())
    .bind(input.description.as_deref().map(str::trim))
    .bind(input.required)
    .bind(input.unique_per_tenant)
    .bind(input.indexed)
    .bind(serde_json::to_value(&input.options).map_err(|_| ContactsError::Invariant)?)
    .bind(&input.validation_schema)
    .bind(input.data_classification.as_str())
    .bind(input.purpose.as_deref().map(str::trim))
    .bind(input.retention_days.map(|days| days as i32))
    .bind(input.encrypted)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => definition_from_row(row),
        None => Err(classify_definition_write_failure(tx, tenant_id, id).await?),
    }
}

pub(crate) async fn set_definition_status(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    id: Uuid,
    expected_version: i64,
    archive: bool,
) -> Result<ContactCustomFieldDefinition, ContactsError> {
    let row = sqlx::query_as::<_, DefinitionRow>(
        r#"
        UPDATE p_contacts.custom_field_definitions
        SET disabled_at = COALESCE(disabled_at, now()),
            archived_at = CASE WHEN $4 THEN COALESCE(archived_at, now()) ELSE archived_at END,
            version = version + 1, updated_at = now()
        WHERE tenant_id = $1 AND id = $2 AND version = $3 AND archived_at IS NULL
        RETURNING id, tenant_id, applies_to, api_name, label, description, field_type,
                  required, unique_per_tenant, indexed, options, validation_schema,
                  data_classification, purpose, retention_days, encrypted, version,
                  disabled_at, archived_at, created_at, updated_at
        "#,
    )
    .bind(tenant_id)
    .bind(id)
    .bind(expected_version)
    .bind(archive)
    .fetch_optional(&mut **tx)
    .await?;
    match row {
        Some(row) => definition_from_row(row),
        None => Err(classify_definition_write_failure(tx, tenant_id, id).await?),
    }
}

async fn classify_definition_write_failure(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    id: Uuid,
) -> Result<ContactsError, sqlx::Error> {
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM p_contacts.custom_field_definitions WHERE tenant_id = $1 AND id = $2)",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_one(&mut **tx)
    .await?;
    Ok(if exists {
        ContactsError::VersionConflict
    } else {
        ContactsError::NotFound
    })
}

pub(crate) async fn custom_value_exists(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    target: ContactCustomFieldTarget,
    api_name: &str,
    value: &Value,
    exclude_id: Option<Uuid>,
) -> Result<bool, ContactsError> {
    let table = match target {
        ContactCustomFieldTarget::Person => "p_contacts.people",
        ContactCustomFieldTarget::Organisation => "p_contacts.organisations",
    };
    // Dynamic JSON fields cannot have one static unique index per definition.
    // Serialize writes for the tenant/target/field tuple so the following
    // existence check and mutation are race-safe within the transaction.
    let lock_key = format!("{tenant_id}:{}:{api_name}", target.as_str());
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(lock_key)
        .execute(&mut **tx)
        .await?;
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM {table} WHERE tenant_id = $1 AND custom_fields -> $2 = $3::jsonb AND ($4::uuid IS NULL OR id <> $4))"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(tenant_id)
        .bind(api_name)
        .bind(value)
        .bind(exclude_id)
        .fetch_one(&mut **tx)
        .await?)
}

fn person_from_row(row: PersonRow) -> Result<Person, ContactsError> {
    Ok(Person {
        id: row.id,
        public_id: row.public_id,
        tenant_id: row.tenant_id,
        full_name: row.full_name,
        email: row.email,
        telephone: row.telephone,
        organisation_id: row.organisation_id,
        organisation_public_id: row.organisation_public_id,
        tags: row.tags,
        custom_fields: serde_json::from_value(row.custom_fields)
            .map_err(|_| ContactsError::Invariant)?,
        extensions: serde_json::from_value(row.extensions).map_err(|_| ContactsError::Invariant)?,
        version: row.version,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        created_by: row.created_by,
        updated_by: row.updated_by,
        touched_at: row.touched_at,
        deleted_at: row.deleted_at,
    })
}

fn organisation_from_row(row: OrganisationRow) -> Result<Organisation, ContactsError> {
    Ok(Organisation {
        id: row.id,
        public_id: row.public_id,
        tenant_id: row.tenant_id,
        name: row.name,
        email: row.email,
        telephone: row.telephone,
        custom_fields: serde_json::from_value(row.custom_fields)
            .map_err(|_| ContactsError::Invariant)?,
        extensions: serde_json::from_value(row.extensions).map_err(|_| ContactsError::Invariant)?,
        version: row.version,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        created_by: row.created_by,
        updated_by: row.updated_by,
        touched_at: row.touched_at,
        deleted_at: row.deleted_at,
    })
}

fn definition_from_row(row: DefinitionRow) -> Result<ContactCustomFieldDefinition, ContactsError> {
    Ok(ContactCustomFieldDefinition {
        id: row.id,
        tenant_id: row.tenant_id,
        applies_to: match row.applies_to.as_str() {
            "person" => ContactCustomFieldTarget::Person,
            "organisation" => ContactCustomFieldTarget::Organisation,
            _ => return Err(ContactsError::Invariant),
        },
        api_name: row.api_name,
        label: row.label,
        description: row.description,
        field_type: ContactCustomFieldType::parse(&row.field_type)
            .ok_or(ContactsError::Invariant)?,
        required: row.required,
        unique_per_tenant: row.unique_per_tenant,
        indexed: row.indexed,
        options: serde_json::from_value(row.options).map_err(|_| ContactsError::Invariant)?,
        validation_schema: row.validation_schema,
        data_classification: DataClassification::parse(&row.data_classification)
            .ok_or(ContactsError::Invariant)?,
        purpose: row.purpose,
        retention_days: row
            .retention_days
            .map(u32::try_from)
            .transpose()
            .map_err(|_| ContactsError::Invariant)?,
        encrypted: row.encrypted,
        version: row.version,
        disabled_at: row.disabled_at,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIGRATION: &str = include_str!("../../migrations/0002_contacts_persistence.sql");
    const TENANT_SCOPED_CURSOR_MIGRATION: &str =
        include_str!("../../migrations/0003_tenant_scoped_cursor_identity.sql");

    #[test]
    fn persistence_schema_forces_tenant_cursor_and_identifier_isolation() {
        assert!(MIGRATION.contains("public_id_counters"));
        assert!(MIGRATION.contains("list_cursors"));
        assert!(MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(MIGRATION.contains("current_setting('app.tenant_id'"));
        assert!(MIGRATION.contains("version bigint NOT NULL DEFAULT 1"));
        assert!(MIGRATION.contains("disabled_at timestamptz"));
        assert!(MIGRATION.contains("archived_at timestamptz"));
    }

    #[test]
    fn cursor_tokens_are_opaque_random_values() {
        let mut bytes = [0_u8; 32];
        rand::rng().fill_bytes(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        assert_eq!(token.len(), 43);
        assert!(!token.contains("person"));
    }

    #[test]
    fn cursor_token_identity_is_unique_only_within_a_tenant() {
        assert!(TENANT_SCOPED_CURSOR_MIGRATION.contains("DROP CONSTRAINT list_cursors_pkey"));
        assert!(TENANT_SCOPED_CURSOR_MIGRATION.contains("ADD PRIMARY KEY (tenant_id, token_hash)"));
    }
}
