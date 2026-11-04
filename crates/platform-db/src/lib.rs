#![forbid(unsafe_code)]
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, Transaction};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use thiserror::Error;
use ui_metadata::publication::TenantPagePublications;
use ui_metadata::{
    external_host::CompiledExternalPage, external_submission::PublicSubmissionPolicy,
};
use uuid::Uuid;

mod devices;
pub use devices::*;

/// Latest core migration required before runtime services can report ready.
pub const CORE_MIGRATION_SENTINEL: &str = "0034_device_notifications.sql";
#[derive(Debug, Error)]
pub enum DbError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: Uuid,
    pub user: AuthUser,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct OidcLoginTransaction {
    pub nonce: String,
    pub pkce_verifier: String,
    /// Set when a device app started the sign-in (ADR 0029).
    pub device: Option<DeviceLogin>,
}

/// A workspace member's user record (ADR 0014): what `created_by` and
/// `updated_by` link to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMember {
    pub id: Uuid,
    pub display_name: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct WorkspaceAccess {
    pub tenant_id: Uuid,
    pub display_name: String,
    pub roles: BTreeSet<String>,
    pub permissions: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub struct PluginCandidateRecord {
    pub plugin_id: String,
    pub version: String,
    pub manifest: serde_json::Value,
    pub manifest_path: String,
    pub artifact_path: Option<String>,
    pub declared_digest: Option<String>,
    pub verified_digest: Option<String>,
    pub lifecycle_state: String,
    pub failure_control: Option<String>,
    pub failure_detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TenantPluginSummary {
    pub id: String,
    pub name: String,
    pub version: String,
    pub state: String,
    pub verified_digest: String,
}

/// The persisted inputs needed to decide whether a tenant may use a plugin.
///
/// This deliberately keeps installation state separate from registry state: an
/// `enabled` installation is not active unless its exact installed version is
/// still a verified, health-checked registry candidate.
#[derive(Debug, Clone)]
pub struct TenantPluginActivationRecord {
    pub plugin_id: String,
    pub installation_state: Option<String>,
    pub active_version: Option<String>,
    pub manifest: Option<serde_json::Value>,
    pub lifecycle_state: Option<String>,
    pub verified_digest: Option<String>,
    pub revision: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginDesiredState {
    Enabled,
    Disabled,
    /// Inactive like `Disabled`, and starts the extension retention window
    /// after which the worker purges the plugin's extension values (#277).
    Removed,
}

impl PluginDesiredState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Removed => "removed",
        }
    }
}

/// Days a removed plugin's extension values are kept before they are purged.
pub const EXTENSION_RETENTION_DAYS: i32 = 30;

/// A removed installation whose extension values are due to be purged.
#[derive(Debug, Clone)]
pub struct DueExtensionPurge {
    pub tenant_id: Uuid,
    pub plugin_id: String,
    pub manifest: serde_json::Value,
}

#[derive(Debug, Error)]
pub enum PluginLifecycleError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("plugin not found")]
    NotFound,
    #[error("plugin has no verified usable candidate")]
    NoUsableCandidate,
    #[error("plugin must be disabled before it is removed")]
    RemovalRequiresDisabled,
    #[error("plugin transition is already in progress")]
    TransitionInProgress,
    #[error("plugin recovery returned an invalid row count")]
    InvalidRecoveryCount,
}

type SessionRow = (Uuid, Uuid, String, Option<String>, DateTime<Utc>);

#[derive(Debug, Error)]
pub enum UiMetadataStateError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("metadata publication state changed; expected revision {expected}, actual {actual:?}")]
    StaleRevision { expected: u64, actual: Option<u64> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExternalPublicationRecord {
    pub tenant_id: Uuid,
    pub portal_audience: Option<String>,
    pub compiled: CompiledExternalPage,
    pub generation: u64,
    pub enabled_plugins: BTreeSet<String>,
    pub revoked: bool,
    pub submission_policy: Option<PublicSubmissionPolicy>,
}

#[derive(Debug, Error)]
pub enum ExternalPublicationStateError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error("external publication generation is invalid")]
    InvalidGeneration,
    #[error("external publication host is invalid")]
    InvalidHost,
}
pub async fn connect(database_url: &str) -> Result<PgPool, DbError> {
    Ok(PgPool::connect(database_url).await?)
}

pub fn connect_lazy(
    database_url: &str,
    max_connections: u32,
    acquire_timeout: std::time::Duration,
) -> Result<PgPool, DbError> {
    Ok(PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(acquire_timeout)
        .connect_lazy(database_url)?)
}

pub async fn store_oidc_login(
    pool: &PgPool,
    state_hash: &str,
    nonce: &str,
    pkce_verifier: &str,
    expires_at: DateTime<Utc>,
    device: Option<&DeviceLogin>,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM oidc_login_transactions WHERE expires_at <= now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO oidc_login_transactions \
         (state_hash, nonce, pkce_verifier, expires_at, device_redirect_uri, device_code_challenge, device_state) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(state_hash)
    .bind(nonce)
    .bind(pkce_verifier)
    .bind(expires_at)
    .bind(device.map(|device| device.redirect_uri.as_str()))
    .bind(device.map(|device| device.code_challenge.as_str()))
    .bind(device.map(|device| device.state.as_str()))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

type OidcLoginRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
);

pub async fn consume_oidc_login(
    pool: &PgPool,
    state_hash: &str,
) -> Result<Option<OidcLoginTransaction>, DbError> {
    let row: Option<OidcLoginRow> = sqlx::query_as(
        "DELETE FROM oidc_login_transactions WHERE state_hash = $1 AND expires_at > now() \
         RETURNING nonce, pkce_verifier, device_redirect_uri, device_code_challenge, device_state",
    )
    .bind(state_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(nonce, pkce_verifier, redirect_uri, code_challenge, state)| OidcLoginTransaction {
            nonce,
            pkce_verifier,
            device: match (redirect_uri, code_challenge, state) {
                (Some(redirect_uri), Some(code_challenge), Some(state)) => Some(DeviceLogin {
                    redirect_uri,
                    code_challenge,
                    state,
                }),
                _ => None,
            },
        },
    ))
}

pub async fn find_or_create_external_user(
    pool: &PgPool,
    issuer: &str,
    subject: &str,
    display_name: &str,
    email: Option<&str>,
) -> Result<AuthUser, DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        // PostgreSQL text cannot hold NUL, so the issuer's length keeps the
        // key unambiguous instead of a separator.
        .bind(format!("{}:{issuer}{subject}", issuer.len()))
        .execute(&mut *tx)
        .await?;
    let existing: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT u.id, u.display_name, u.email FROM external_identities e JOIN users u ON u.id = e.user_id WHERE e.issuer = $1 AND e.subject = $2",
    )
    .bind(issuer)
    .bind(subject)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some((id, display_name, email)) = existing {
        tx.commit().await?;
        return Ok(AuthUser {
            id,
            display_name,
            email,
        });
    }
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO users (id, display_name, email) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(display_name)
        .bind(email)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO external_identities (issuer, subject, user_id) VALUES ($1, $2, $3)")
        .bind(issuer)
        .bind(subject)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(AuthUser {
        id,
        display_name: display_name.to_owned(),
        email: email.map(str::to_owned),
    })
}

pub async fn create_session(
    pool: &PgPool,
    token_hash: &str,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
) -> Result<SessionRecord, DbError> {
    let id = Uuid::now_v7();
    let (display_name, email): (String, Option<String>) = sqlx::query_as(
        "WITH inserted AS (INSERT INTO sessions (id, token_hash, user_id, expires_at) VALUES ($1, $2, $3, $4) RETURNING user_id) SELECT u.display_name, u.email FROM inserted JOIN users u ON u.id = inserted.user_id",
    )
    .bind(id)
    .bind(token_hash)
    .bind(user_id)
    .bind(expires_at)
    .fetch_one(pool)
    .await?;
    Ok(SessionRecord {
        id,
        user: AuthUser {
            id: user_id,
            display_name,
            email,
        },
        expires_at,
    })
}

pub async fn resolve_session(
    pool: &PgPool,
    token_hash: &str,
    renew_if_before: DateTime<Utc>,
    renewed_expiry: DateTime<Utc>,
) -> Result<Option<SessionRecord>, DbError> {
    let mut tx = pool.begin().await?;
    let row: Option<SessionRow> = sqlx::query_as(
        "SELECT s.id, u.id, u.display_name, u.email, s.expires_at FROM sessions s JOIN users u ON u.id = s.user_id WHERE s.token_hash = $1 AND s.revoked_at IS NULL AND s.expires_at > now() FOR UPDATE OF s",
    )
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((id, user_id, display_name, email, expires_at)) = row else {
        tx.rollback().await?;
        return Ok(None);
    };
    let expires_at = if expires_at <= renew_if_before {
        sqlx::query("UPDATE sessions SET expires_at = $1, last_seen_at = now() WHERE id = $2")
            .bind(renewed_expiry)
            .bind(id)
            .execute(&mut *tx)
            .await?;
        renewed_expiry
    } else {
        sqlx::query("UPDATE sessions SET last_seen_at = now() WHERE id = $1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        expires_at
    };
    tx.commit().await?;
    Ok(Some(SessionRecord {
        id,
        user: AuthUser {
            id: user_id,
            display_name,
            email,
        },
        expires_at,
    }))
}

pub async fn revoke_session(pool: &PgPool, token_hash: &str) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE sessions SET revoked_at = COALESCE(revoked_at, now()) WHERE token_hash = $1",
    )
    .bind(token_hash)
    .execute(pool)
    .await?;
    Ok(())
}

/// List the workspaces available to a live session's user.
///
/// Membership and role tables force tenant RLS, so this pre-tenant-context
/// lookup goes through a bounded routine that answers only for the user of an
/// unrevoked, unexpired session.
pub async fn list_workspace_access(
    pool: &PgPool,
    session_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<WorkspaceAccess>, DbError> {
    session_workspace_access(pool, session_id, user_id, None).await
}

/// Resolve one workspace for a live session's user, or `None` when the user is
/// not a member or the session is no longer live.
pub async fn resolve_workspace_access(
    pool: &PgPool,
    session_id: Uuid,
    user_id: Uuid,
    tenant_id: Uuid,
) -> Result<Option<WorkspaceAccess>, DbError> {
    Ok(
        session_workspace_access(pool, session_id, user_id, Some(tenant_id))
            .await?
            .into_iter()
            .find(|access| access.tenant_id == tenant_id),
    )
}

async fn session_workspace_access(
    pool: &PgPool,
    session_id: Uuid,
    user_id: Uuid,
    tenant_id: Option<Uuid>,
) -> Result<Vec<WorkspaceAccess>, DbError> {
    let rows: Vec<(Uuid, String, Option<String>, Option<String>)> = sqlx::query_as(
        r#"
        SELECT tenant_id, display_name, role_name, permission_id
        FROM platform_session_workspace_access($1, $2, $3)
        "#,
    )
    .bind(session_id)
    .bind(user_id)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    let mut by_tenant = std::collections::BTreeMap::<Uuid, WorkspaceAccess>::new();
    for (tenant_id, display_name, role, permission) in rows {
        let access = by_tenant
            .entry(tenant_id)
            .or_insert_with(|| WorkspaceAccess {
                tenant_id,
                display_name,
                roles: BTreeSet::new(),
                permissions: BTreeSet::new(),
            });
        if let Some(role) = role {
            access.roles.insert(role);
        }
        if let Some(permission) = permission {
            access.permissions.insert(permission);
        }
    }
    Ok(by_tenant.into_values().collect())
}

pub async fn register_plugin_candidate(
    pool: &PgPool,
    candidate: &PluginCandidateRecord,
) -> Result<(), PluginLifecycleError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("plugin:{}", candidate.plugin_id))
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        r#"
        INSERT INTO plugin_catalogue (plugin_id, manifest, lifecycle_state)
        VALUES ($1, $2, $3)
        ON CONFLICT (plugin_id) DO NOTHING
        "#,
    )
    .bind(&candidate.plugin_id)
    .bind(&candidate.manifest)
    .bind(&candidate.lifecycle_state)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO plugin_versions (
          plugin_id, version, manifest, manifest_path, artifact_path,
          declared_digest, verified_digest, lifecycle_state, failure_control,
          failure_detail, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, now())
        ON CONFLICT (plugin_id, version) DO UPDATE SET
          manifest = EXCLUDED.manifest,
          manifest_path = EXCLUDED.manifest_path,
          artifact_path = EXCLUDED.artifact_path,
          declared_digest = EXCLUDED.declared_digest,
          verified_digest = EXCLUDED.verified_digest,
          lifecycle_state = EXCLUDED.lifecycle_state,
          failure_control = EXCLUDED.failure_control,
          failure_detail = EXCLUDED.failure_detail,
          updated_at = now()
        "#,
    )
    .bind(&candidate.plugin_id)
    .bind(&candidate.version)
    .bind(&candidate.manifest)
    .bind(&candidate.manifest_path)
    .bind(&candidate.artifact_path)
    .bind(&candidate.declared_digest)
    .bind(&candidate.verified_digest)
    .bind(&candidate.lifecycle_state)
    .bind(&candidate.failure_control)
    .bind(&candidate.failure_detail)
    .execute(&mut *tx)
    .await?;

    let accepted = candidate.lifecycle_state == "health_checked"
        && candidate.verified_digest.is_some()
        && candidate.failure_control.is_none();
    if accepted {
        sqlx::query(
            r#"
            UPDATE plugin_catalogue
            SET manifest = $2,
                lifecycle_state = 'health_checked',
                latest_usable_version = $3,
                updated_at = now()
            WHERE plugin_id = $1
            "#,
        )
        .bind(&candidate.plugin_id)
        .bind(&candidate.manifest)
        .bind(&candidate.version)
        .execute(&mut *tx)
        .await?;
    }
    append_plugin_audit(
        &mut tx,
        None,
        None,
        None,
        if accepted {
            "plugin.candidate.accepted"
        } else {
            "plugin.candidate.rejected"
        },
        &candidate.plugin_id,
        serde_json::json!({
            "version": candidate.version,
            "control": candidate.failure_control,
            "verified_digest": candidate.verified_digest,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn record_plugin_discovery_failure(
    pool: &PgPool,
    plugin_id: Option<&str>,
    version: Option<&str>,
    manifest_path: &str,
    control: &str,
) -> Result<(), PluginLifecycleError> {
    let mut tx = pool.begin().await?;
    if let (Some(plugin_id), Some(version)) = (plugin_id, version) {
        sqlx::query(
            r#"
            INSERT INTO plugin_catalogue (plugin_id, manifest, lifecycle_state)
            VALUES ($1, '{}'::jsonb, 'failed')
            ON CONFLICT (plugin_id) DO NOTHING
            "#,
        )
        .bind(plugin_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO plugin_versions (
              plugin_id, version, manifest, manifest_path, lifecycle_state,
              failure_control, failure_detail
            ) VALUES ($1, $2, '{}'::jsonb, $3, 'failed', $4, 'candidate rejected by policy')
            ON CONFLICT (plugin_id, version) DO UPDATE SET
              manifest_path = EXCLUDED.manifest_path,
              lifecycle_state = 'failed',
              failure_control = EXCLUDED.failure_control,
              failure_detail = EXCLUDED.failure_detail,
              updated_at = now()
            WHERE plugin_versions.lifecycle_state <> 'health_checked'
            "#,
        )
        .bind(plugin_id)
        .bind(version)
        .bind(manifest_path)
        .bind(control)
        .execute(&mut *tx)
        .await?;
    }
    append_plugin_audit(
        &mut tx,
        None,
        None,
        None,
        "plugin.candidate.rejected",
        plugin_id.unwrap_or("unknown"),
        serde_json::json!({"version": version, "control": control}),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn recover_plugin_transitions(pool: &PgPool) -> Result<u64, PluginLifecycleError> {
    let recovered: i64 = sqlx::query_scalar("SELECT platform_recover_plugin_transitions()")
        .fetch_one(pool)
        .await?;
    recovered
        .try_into()
        .map_err(|_| PluginLifecycleError::InvalidRecoveryCount)
}

/// The manifest of a plugin's latest usable registry version, whether or not
/// the workspace has installed it. Used to describe what enabling the plugin
/// would change before an administrator enables it.
pub async fn latest_usable_plugin_manifest(
    pool: &PgPool,
    tenant_id: Uuid,
    plugin_id: &str,
) -> Result<Option<serde_json::Value>, PluginLifecycleError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let manifest: Option<serde_json::Value> = sqlx::query_scalar(
        r#"
        SELECT v.manifest
        FROM plugin_catalogue c
        JOIN plugin_versions v
          ON v.plugin_id = c.plugin_id AND v.version = c.latest_usable_version
        WHERE c.plugin_id = $1
        "#,
    )
    .bind(plugin_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(manifest)
}

pub async fn list_tenant_plugins(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<TenantPluginSummary>, PluginLifecycleError> {
    type Row = (String, String, String, String, String);
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let rows: Vec<Row> = sqlx::query_as(
        r#"
        SELECT c.plugin_id,
               COALESCE(v.manifest->>'name', c.plugin_id),
               c.latest_usable_version,
               COALESCE(i.state, 'disabled'),
               v.verified_digest
        FROM plugin_catalogue c
        JOIN plugin_versions v
          ON v.plugin_id = c.plugin_id AND v.version = c.latest_usable_version
        LEFT JOIN tenant_plugin_installations i
          ON i.tenant_id = $1 AND i.plugin_id = c.plugin_id
        WHERE c.latest_usable_version IS NOT NULL
        ORDER BY c.plugin_id
        "#,
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, version, state, verified_digest)| TenantPluginSummary {
                id,
                name,
                version,
                state,
                verified_digest,
            },
        )
        .collect())
}

const TENANT_PLUGIN_ACTIVATION_PROJECTION_SQL: &str = r#"
    SELECT c.plugin_id,
           i.state,
           i.active_version,
           v.manifest,
           v.lifecycle_state,
           v.verified_digest,
           GREATEST(
             c.updated_at,
             COALESCE(i.updated_at, c.updated_at),
             COALESCE(v.updated_at, c.updated_at)
           ) AS revision
    FROM plugin_catalogue c
    LEFT JOIN tenant_plugin_installations i
      ON i.tenant_id = $1 AND i.plugin_id = c.plugin_id
    LEFT JOIN plugin_versions v
      ON v.plugin_id = i.plugin_id AND v.version = i.active_version
    ORDER BY c.plugin_id
"#;

/// Load all durable registry and installation inputs for a workspace in one
/// snapshot. Callers must still parse the validated manifest and apply caller
/// permissions after filtering inactive records.
pub async fn list_tenant_plugin_activation_records(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<TenantPluginActivationRecord>, PluginLifecycleError> {
    type Row = (
        String,
        Option<String>,
        Option<String>,
        Option<serde_json::Value>,
        Option<String>,
        Option<String>,
        DateTime<Utc>,
    );
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|error| match error {
            DbError::Sqlx(error) => PluginLifecycleError::Database(error),
        })?;
    let rows: Vec<Row> = sqlx::query_as(TENANT_PLUGIN_ACTIVATION_PROJECTION_SQL)
        .bind(tenant_id)
        .fetch_all(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                plugin_id,
                installation_state,
                active_version,
                manifest,
                lifecycle_state,
                verified_digest,
                revision,
            )| TenantPluginActivationRecord {
                plugin_id,
                installation_state,
                active_version,
                manifest,
                lifecycle_state,
                verified_digest,
                revision,
            },
        )
        .collect())
}

/// Read one tenant installation inside an existing tenant transaction and hold
/// a share lock on it until that transaction ends.
///
/// Background plugin work calls this before its side effects. The share lock
/// conflicts with `transition_tenant_plugin`, so a disable waits for in-flight
/// work to commit or roll back (drain), and work that starts afterwards sees
/// the committed disabled state. A missing installation returns `None`.
pub async fn lock_tenant_plugin_activation(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Uuid,
    plugin_id: &str,
) -> Result<Option<TenantPluginActivationRecord>, sqlx::Error> {
    let installation: Option<(String, Option<String>, DateTime<Utc>)> = sqlx::query_as(
        r#"
        SELECT state, active_version, updated_at
        FROM tenant_plugin_installations
        WHERE tenant_id = $1 AND plugin_id = $2
        FOR SHARE
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((installation_state, active_version, installation_revision)) = installation else {
        return Ok(None);
    };
    // Lock the installed registry version too, so revoking or quarantining it
    // also waits for in-flight work instead of racing it.
    let version: Option<(serde_json::Value, String, Option<String>, DateTime<Utc>)> =
        match active_version.as_deref() {
            Some(active_version) => {
                sqlx::query_as(
                    r#"
                    SELECT manifest, lifecycle_state, verified_digest, updated_at
                    FROM plugin_versions
                    WHERE plugin_id = $1 AND version = $2
                    FOR SHARE
                    "#,
                )
                .bind(plugin_id)
                .bind(active_version)
                .fetch_optional(&mut **tx)
                .await?
            }
            None => None,
        };
    let (manifest, lifecycle_state, verified_digest, version_revision) = match version {
        Some((manifest, lifecycle_state, verified_digest, revision)) => (
            Some(manifest),
            Some(lifecycle_state),
            verified_digest,
            Some(revision),
        ),
        None => (None, None, None, None),
    };
    Ok(Some(TenantPluginActivationRecord {
        plugin_id: plugin_id.to_owned(),
        installation_state: Some(installation_state),
        active_version,
        manifest,
        lifecycle_state,
        verified_digest,
        revision: version_revision.map_or(installation_revision, |revision| {
            revision.max(installation_revision)
        }),
    }))
}

pub async fn tenant_plugin_enabled(
    pool: &PgPool,
    tenant_id: Uuid,
    plugin_id: &str,
) -> Result<bool, PluginLifecycleError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let enabled = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM tenant_plugin_installations
          WHERE tenant_id = $1 AND plugin_id = $2 AND state = 'enabled'
        )
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(enabled)
}

/// Transition one tenant's plugin installation. Every attempt, outcome, and
/// refusal is audited in the same transaction, carrying `correlation_id` (the
/// caller's request id) so lifecycle evidence can be joined to its request.
pub async fn transition_tenant_plugin(
    pool: &PgPool,
    tenant_id: Uuid,
    actor_id: Uuid,
    plugin_id: &str,
    desired: PluginDesiredState,
    correlation_id: Option<Uuid>,
) -> Result<TenantPluginSummary, PluginLifecycleError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("plugin-installation:{tenant_id}:{plugin_id}"))
        .execute(&mut *tx)
        .await?;
    append_plugin_audit(
        &mut tx,
        Some(tenant_id),
        Some(actor_id),
        correlation_id,
        "plugin.transition.attempted",
        plugin_id,
        serde_json::json!({"desired_state": desired.as_str()}),
    )
    .await?;

    let candidate: Option<(String, String, String)> = sqlx::query_as(
        r#"
        SELECT c.latest_usable_version,
               COALESCE(v.manifest->>'name', c.plugin_id),
               v.verified_digest
        FROM plugin_catalogue c
        JOIN plugin_versions v
          ON v.plugin_id = c.plugin_id AND v.version = c.latest_usable_version
        WHERE c.plugin_id = $1 AND v.lifecycle_state = 'health_checked'
        "#,
    )
    .bind(plugin_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((version, name, verified_digest)) = candidate else {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM plugin_catalogue WHERE plugin_id = $1)",
        )
        .bind(plugin_id)
        .fetch_one(&mut *tx)
        .await?;
        append_plugin_audit(
            &mut tx,
            Some(tenant_id),
            Some(actor_id),
            correlation_id,
            "plugin.transition.failed",
            plugin_id,
            serde_json::json!({
                "desired_state": desired.as_str(),
                "control": if exists { "plugin.no_usable_candidate" } else { "plugin.not_found" },
            }),
        )
        .await?;
        tx.commit().await?;
        return Err(if exists {
            PluginLifecycleError::NoUsableCandidate
        } else {
            PluginLifecycleError::NotFound
        });
    };

    let current: Option<String> = sqlx::query_scalar(
        "SELECT state FROM tenant_plugin_installations WHERE tenant_id = $1 AND plugin_id = $2 FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .fetch_optional(&mut *tx)
    .await?;
    if current
        .as_deref()
        .is_some_and(|state| matches!(state, "enabling" | "disabling"))
    {
        append_plugin_audit(
            &mut tx,
            Some(tenant_id),
            Some(actor_id),
            correlation_id,
            "plugin.transition.failed",
            plugin_id,
            serde_json::json!({
                "desired_state": desired.as_str(),
                "control": "plugin.transition_in_progress",
            }),
        )
        .await?;
        tx.commit().await?;
        return Err(PluginLifecycleError::TransitionInProgress);
    }
    if desired == PluginDesiredState::Removed
        && !matches!(current.as_deref(), Some("disabled" | "removed"))
    {
        append_plugin_audit(
            &mut tx,
            Some(tenant_id),
            Some(actor_id),
            correlation_id,
            "plugin.transition.failed",
            plugin_id,
            serde_json::json!({
                "desired_state": desired.as_str(),
                "control": "plugin.remove_requires_disabled",
            }),
        )
        .await?;
        tx.commit().await?;
        return Err(PluginLifecycleError::RemovalRequiresDisabled);
    }
    let idempotent = current.as_deref() == Some(desired.as_str());
    sqlx::query(
        r#"
        INSERT INTO tenant_plugin_installations (
          tenant_id, plugin_id, state, active_version, enabled_at, disabled_at, updated_at
        ) VALUES (
          $1, $2, $3, $4,
          CASE WHEN $3 = 'enabled' THEN now() ELSE NULL END,
          CASE WHEN $3 = 'disabled' THEN now() ELSE NULL END,
          now()
        )
        ON CONFLICT (tenant_id, plugin_id) DO UPDATE SET
          state = EXCLUDED.state,
          active_version = CASE
            WHEN EXCLUDED.state = 'enabled' THEN EXCLUDED.active_version
            ELSE tenant_plugin_installations.active_version
          END,
          enabled_at = CASE
            WHEN EXCLUDED.state = 'enabled' THEN COALESCE(tenant_plugin_installations.enabled_at, now())
            ELSE tenant_plugin_installations.enabled_at
          END,
          disabled_at = CASE
            WHEN EXCLUDED.state = 'disabled' THEN now()
            WHEN EXCLUDED.state = 'removed' THEN tenant_plugin_installations.disabled_at
            ELSE NULL
          END,
          -- Removal starts the retention window once; enabling or disabling
          -- again cancels it and keeps the stored values.
          removed_at = CASE
            WHEN EXCLUDED.state = 'removed' THEN COALESCE(tenant_plugin_installations.removed_at, now())
            ELSE NULL
          END,
          extensions_purged_at = CASE
            WHEN EXCLUDED.state = 'removed' THEN tenant_plugin_installations.extensions_purged_at
            ELSE NULL
          END,
          candidate_version = NULL,
          previous_state = NULL,
          transition_id = NULL,
          transition_started_at = NULL,
          last_error_control = NULL,
          updated_at = now()
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .bind(desired.as_str())
    .bind(&version)
    .execute(&mut *tx)
    .await?;
    append_plugin_audit(
        &mut tx,
        Some(tenant_id),
        Some(actor_id),
        correlation_id,
        "plugin.transition.succeeded",
        plugin_id,
        serde_json::json!({
            "state": desired.as_str(),
            "version": version,
            "verified_digest": verified_digest,
            "idempotent": idempotent,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(TenantPluginSummary {
        id: plugin_id.to_owned(),
        name,
        version,
        state: desired.as_str().into(),
        verified_digest,
    })
}

/// Removed installations, across tenants, whose retention window has passed
/// and whose extension values are not yet purged. Uses the worker's bounded
/// discovery routine; purging happens per tenant.
pub async fn due_extension_purges(
    worker_pool: &PgPool,
    retention_days: i32,
    limit: i32,
) -> Result<Vec<DueExtensionPurge>, sqlx::Error> {
    let rows: Vec<(Uuid, String, serde_json::Value)> = sqlx::query_as(
        "SELECT tenant_id, plugin_id, manifest FROM platform_due_extension_purges($1, $2)",
    )
    .bind(retention_days)
    .bind(limit)
    .fetch_all(worker_pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(tenant_id, plugin_id, manifest)| DueExtensionPurge {
            tenant_id,
            plugin_id,
            manifest,
        })
        .collect())
}

/// Due automation jobs across tenants (ADR 0022 §6), oldest first, from the
/// worker's bounded discovery routine. Each job then runs in its tenant's
/// own transaction.
pub async fn due_automation_jobs(
    worker_pool: &PgPool,
    limit: i32,
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as("SELECT tenant_id, id FROM platform_due_automation_jobs($1)")
        .bind(limit)
        .fetch_all(worker_pool)
        .await
}

/// Bulk jobs with work to do, across workspaces (ADR 0023 §2). Only the
/// bounded `platform_due_bulk_jobs` routine crosses tenants.
pub async fn due_bulk_jobs(
    worker_pool: &PgPool,
    limit: i32,
) -> Result<Vec<(Uuid, Uuid)>, sqlx::Error> {
    sqlx::query_as("SELECT tenant_id, id FROM platform_due_bulk_jobs($1)")
        .bind(limit)
        .fetch_all(worker_pool)
        .await
}

/// A member's current system permissions and roles in a workspace, read
/// under its row-level security, or `None` when they are not a member.
/// Background work done for a member, such as a bulk job, uses it so that
/// removed members and revoked permissions take effect between batches.
pub async fn member_access(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<(BTreeSet<String>, Vec<Uuid>)>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let member: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM memberships WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if member.is_none() {
        tx.commit().await?;
        return Ok(None);
    }
    let roles: Vec<Uuid> = sqlx::query_scalar(
        "SELECT role_id FROM user_roles WHERE tenant_id = $1 AND user_id = $2 ORDER BY role_id",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    let permissions: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT rp.permission_id FROM user_roles ur \
         JOIN role_permissions rp ON rp.tenant_id = ur.tenant_id AND rp.role_id = ur.role_id \
         WHERE ur.tenant_id = $1 AND ur.user_id = $2",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some((permissions.into_iter().collect(), roles)))
}

/// Records that every host purged a removed plugin's extension values, with
/// per-host counts, in the tenant's own transaction. Returns false when the
/// installation is no longer removed or was already purged, for example
/// because an administrator reinstalled it meanwhile.
pub async fn mark_extensions_purged(
    pool: &PgPool,
    tenant_id: Uuid,
    plugin_id: &str,
    purged: &BTreeMap<String, u64>,
) -> Result<bool, sqlx::Error> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let updated = sqlx::query(
        r#"
        UPDATE tenant_plugin_installations
        SET extensions_purged_at = now(), updated_at = now()
        WHERE tenant_id = $1 AND plugin_id = $2
          AND state = 'removed' AND extensions_purged_at IS NULL
        "#,
    )
    .bind(tenant_id)
    .bind(plugin_id)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    if updated == 1 {
        append_plugin_audit(
            &mut tx,
            Some(tenant_id),
            None,
            None,
            "plugin.extensions.purged",
            plugin_id,
            serde_json::json!({"records_by_host": purged}),
        )
        .await?;
    }
    tx.commit().await?;
    Ok(updated == 1)
}

/// Audits an administrator's export of a plugin's extension values. Only the
/// plugin identifier and the record count are recorded, never the values.
pub async fn record_extension_export(
    pool: &PgPool,
    tenant_id: Uuid,
    actor_id: Uuid,
    plugin_id: &str,
    correlation_id: Option<Uuid>,
    records: usize,
) -> Result<(), sqlx::Error> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    append_plugin_audit(
        &mut tx,
        Some(tenant_id),
        Some(actor_id),
        correlation_id,
        "plugin.extensions.exported",
        plugin_id,
        serde_json::json!({"records": records}),
    )
    .await?;
    tx.commit().await
}

/// A workspace's stored arrangement of extension fields for one resource.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredFieldLayout {
    pub layout: serde_json::Value,
    pub revision: i64,
}

#[derive(Debug, Error)]
pub enum FieldLayoutError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("the layout changed since it was read")]
    RevisionConflict,
}

/// Reads the stored field layout for a resource, if an administrator saved one.
pub async fn get_field_layout(
    pool: &PgPool,
    tenant_id: Uuid,
    resource: &str,
) -> Result<Option<StoredFieldLayout>, sqlx::Error> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let row: Option<(serde_json::Value, i64)> = sqlx::query_as(
        "SELECT layout, revision FROM resource_field_layouts WHERE tenant_id = $1 AND resource = $2",
    )
    .bind(tenant_id)
    .bind(resource)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(|(layout, revision)| StoredFieldLayout { layout, revision }))
}

/// Saves a kernel-validated layout when `expected_revision` matches the
/// stored one (0 when none is stored) and audits the change in the same
/// transaction. Returns the new revision.
pub async fn save_field_layout(
    pool: &PgPool,
    tenant_id: Uuid,
    actor_id: Uuid,
    resource: &str,
    layout: &serde_json::Value,
    expected_revision: i64,
    correlation_id: Option<Uuid>,
) -> Result<i64, FieldLayoutError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let current: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM resource_field_layouts WHERE tenant_id = $1 AND resource = $2 FOR UPDATE",
    )
    .bind(tenant_id)
    .bind(resource)
    .fetch_optional(&mut *tx)
    .await?;
    if current.unwrap_or(0) != expected_revision {
        return Err(FieldLayoutError::RevisionConflict);
    }
    let revision = expected_revision + 1;
    sqlx::query(
        r#"
        INSERT INTO resource_field_layouts (tenant_id, resource, layout, revision, updated_by, updated_at)
        VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (tenant_id, resource) DO UPDATE SET
          layout = EXCLUDED.layout,
          revision = EXCLUDED.revision,
          updated_by = EXCLUDED.updated_by,
          updated_at = now()
        "#,
    )
    .bind(tenant_id)
    .bind(resource)
    .bind(layout)
    .bind(revision)
    .bind(actor_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata)
        VALUES ($1, $2, $3, $4, 'extension.layout.saved', $5, $6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(actor_id)
    .bind(correlation_id)
    .bind(format!("resource:{resource}"))
    .bind(serde_json::json!({"revision": revision}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(revision)
}

/// Audit a lifecycle request the host refused before reaching the lifecycle
/// service, such as a caller without the plugin management permission. Only
/// the plugin identifier and stable control codes are recorded.
pub async fn record_plugin_transition_rejected(
    pool: &PgPool,
    tenant_id: Uuid,
    actor_id: Uuid,
    plugin_id: &str,
    desired: PluginDesiredState,
    control: &str,
    correlation_id: Option<Uuid>,
) -> Result<(), PluginLifecycleError> {
    if !valid_plugin_id(plugin_id) {
        return Ok(());
    }
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    append_plugin_audit(
        &mut tx,
        Some(tenant_id),
        Some(actor_id),
        correlation_id,
        "plugin.transition.rejected",
        plugin_id,
        serde_json::json!({
            "desired_state": desired.as_str(),
            "control": control,
        }),
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn append_plugin_audit(
    tx: &mut Transaction<'_, Postgres>,
    tenant_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    correlation_id: Option<Uuid>,
    action: &str,
    plugin_id: &str,
    metadata: serde_json::Value,
) -> Result<(), sqlx::Error> {
    let id = Uuid::now_v7();
    let target = format!("plugin:{plugin_id}");
    if tenant_id.is_none() {
        sqlx::query("SELECT platform_append_global_plugin_audit($1, $2, $3, $4)")
            .bind(id)
            .bind(action)
            .bind(target)
            .bind(metadata)
            .execute(&mut **tx)
            .await?;
        return Ok(());
    }
    sqlx::query(
        r#"
        INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(tenant_id)
    .bind(actor_id)
    .bind(correlation_id)
    .bind(action)
    .bind(target)
    .bind(metadata)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub async fn begin_tenant_tx(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Transaction<'static, Postgres>, DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
        .bind(tenant_id.to_string())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

/// Display names for the users among `user_ids` who are members of the
/// workspace, for the standard `created_by` and `updated_by` record fields
/// (ADR 0014). Users who are not members are left out, so a record never
/// discloses a name from outside its workspace; membership is read under the
/// workspace's row-level security.
pub async fn member_display_names(
    pool: &PgPool,
    tenant_id: Uuid,
    user_ids: &[Uuid],
) -> Result<BTreeMap<Uuid, String>, DbError> {
    if user_ids.is_empty() {
        return Ok(BTreeMap::new());
    }
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT u.id, u.display_name FROM memberships m JOIN users u ON u.id = m.user_id \
         WHERE m.tenant_id = $1 AND m.user_id = ANY($2)",
    )
    .bind(tenant_id)
    .bind(user_ids)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().collect())
}

/// The roles a member holds in a workspace, read under the workspace's
/// row-level security. The object engine resolves object permissions from
/// them (ADR 0015).
pub async fn user_role_ids(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let roles = sqlx::query_scalar(
        "SELECT role_id FROM user_roles WHERE tenant_id = $1 AND user_id = $2 ORDER BY role_id",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(roles)
}

/// The deepest reporting line followed when resolving someone's reports
/// (ADR 0016); deeper lines are cut off rather than walked forever.
pub const REPORTING_DEPTH_LIMIT: i32 = 64;

/// Everyone who reports to `user_id`, directly or through others, in the
/// workspace. The engine lets a manager see and edit what these members own
/// on objects that grant access using hierarchies (ADR 0016).
pub async fn reporting_reports(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Vec<Uuid>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let reports = sqlx::query_scalar(
        "WITH RECURSIVE below(user_id, depth, path) AS ( \
           SELECT l.user_id, 1, ARRAY[$2::uuid, l.user_id] FROM reporting_lines l \
           WHERE l.tenant_id = $1 AND l.manager_user_id = $2 \
           UNION ALL \
           SELECT l.user_id, b.depth + 1, b.path || l.user_id FROM reporting_lines l \
           JOIN below b ON l.manager_user_id = b.user_id \
           WHERE l.tenant_id = $1 AND b.depth < $3 AND NOT l.user_id = ANY(b.path) \
         ) SELECT DISTINCT user_id FROM below ORDER BY user_id",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(REPORTING_DEPTH_LIMIT)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(reports)
}

/// Who `user_id` reports to, if anyone.
pub async fn reporting_manager(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<Uuid>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let manager = sqlx::query_scalar(
        "SELECT manager_user_id FROM reporting_lines WHERE tenant_id = $1 AND user_id = $2",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(manager)
}

#[derive(Debug, thiserror::Error)]
pub enum ReportingLineError {
    /// The member or the manager is not a member of the workspace.
    #[error("not a workspace member")]
    NotMember,
    /// The manager already reports to the member, directly or indirectly.
    #[error("reporting line would form a cycle")]
    Cycle,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Sets or clears who `user_id` reports to, refusing cycles, and audits the
/// change in the same transaction.
pub async fn set_reporting_line(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    manager_user_id: Option<Uuid>,
    actor_id: Uuid,
    correlation_id: Option<Uuid>,
) -> Result<(), ReportingLineError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    // One writer at a time per workspace, so two concurrent changes cannot
    // together form a cycle that neither would alone.
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('reporting_lines:' || $1::text, 0))",
    )
    .bind(tenant_id)
    .execute(&mut *tx)
    .await?;
    let members: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM memberships WHERE tenant_id = $1 AND user_id = ANY($2)",
    )
    .bind(tenant_id)
    .bind(
        [Some(user_id), manager_user_id]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>(),
    )
    .fetch_one(&mut *tx)
    .await?;
    let expected = if manager_user_id.is_some_and(|manager| manager != user_id) {
        2
    } else {
        1
    };
    if members != expected {
        return Err(ReportingLineError::NotMember);
    }
    if let Some(manager) = manager_user_id {
        if manager == user_id {
            return Err(ReportingLineError::Cycle);
        }
        // Walk up from the proposed manager; meeting the member is a cycle.
        let cycle: bool = sqlx::query_scalar(
            "WITH RECURSIVE above(user_id, depth) AS ( \
               SELECT manager_user_id, 1 FROM reporting_lines WHERE tenant_id = $1 AND user_id = $2 \
               UNION ALL \
               SELECT l.manager_user_id, a.depth + 1 FROM reporting_lines l \
               JOIN above a ON l.user_id = a.user_id \
               WHERE l.tenant_id = $1 AND a.depth < $4 \
             ) SELECT EXISTS (SELECT 1 FROM above WHERE user_id = $3)",
        )
        .bind(tenant_id)
        .bind(manager)
        .bind(user_id)
        .bind(REPORTING_DEPTH_LIMIT)
        .fetch_one(&mut *tx)
        .await?;
        if cycle {
            return Err(ReportingLineError::Cycle);
        }
        sqlx::query(
            "INSERT INTO reporting_lines (tenant_id, user_id, manager_user_id, updated_at, updated_by) \
             VALUES ($1, $2, $3, now(), $4) \
             ON CONFLICT (tenant_id, user_id) DO UPDATE SET \
               manager_user_id = EXCLUDED.manager_user_id, updated_at = now(), updated_by = EXCLUDED.updated_by",
        )
        .bind(tenant_id)
        .bind(user_id)
        .bind(manager)
        .bind(actor_id)
        .execute(&mut *tx)
        .await?;
    } else {
        sqlx::query("DELETE FROM reporting_lines WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query(
        r#"
        INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata)
        VALUES ($1, $2, $3, $4, 'members.reporting_line.changed', $5, $6)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(actor_id)
    .bind(correlation_id)
    .bind(format!("user:{user_id}"))
    .bind(serde_json::json!({"manager_user_id": manager_user_id}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// An API client (ADR 0023 §3) as administrators see it. Its secret is
/// shown once, when it is created, and stored only as a hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiClient {
    pub id: Uuid,
    pub name: String,
    pub client_id: String,
    /// The member the client acts as.
    pub run_as: Uuid,
    pub created_by: Uuid,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_used_at: Option<chrono::DateTime<chrono::Utc>>,
    pub revoked_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Active API clients per workspace.
pub const API_CLIENTS_MAX: i64 = 20;

#[derive(Debug, Error)]
pub enum ApiClientError {
    /// The member to act as is not a member of the workspace.
    #[error("not a workspace member")]
    NotMember,
    /// The workspace already has [`API_CLIENTS_MAX`] active clients.
    #[error("too many API clients")]
    LimitReached,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

const API_CLIENT_COLUMNS: &str =
    "id, name, client_id, run_as, created_by, created_at, last_used_at, revoked_at";

type ApiClientRow = (
    Uuid,
    String,
    String,
    Uuid,
    Uuid,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<chrono::DateTime<chrono::Utc>>,
);

fn api_client(row: ApiClientRow) -> ApiClient {
    let (id, name, client_id, run_as, created_by, created_at, last_used_at, revoked_at) = row;
    ApiClient {
        id,
        name,
        client_id,
        run_as,
        created_by,
        created_at,
        last_used_at,
        revoked_at,
    }
}

/// Stores a new API client acting as `run_as`, with its secret's hash, and
/// audits it in the same transaction.
#[allow(clippy::too_many_arguments)]
pub async fn create_api_client(
    pool: &PgPool,
    tenant_id: Uuid,
    name: &str,
    client_id: &str,
    secret_hash: &str,
    run_as: Uuid,
    actor_id: Uuid,
    correlation_id: Option<Uuid>,
) -> Result<ApiClient, ApiClientError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('api_clients:' || $1::text, 0))")
        .bind(tenant_id)
        .execute(&mut *tx)
        .await?;
    let member: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM memberships WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant_id)
            .bind(run_as)
            .fetch_optional(&mut *tx)
            .await?;
    if member.is_none() {
        return Err(ApiClientError::NotMember);
    }
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM api_clients WHERE tenant_id = $1 AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .fetch_one(&mut *tx)
    .await?;
    if active >= API_CLIENTS_MAX {
        return Err(ApiClientError::LimitReached);
    }
    let row: ApiClientRow = sqlx::query_as(&format!(
        "INSERT INTO api_clients (tenant_id, id, name, client_id, secret_hash, run_as, created_by, created_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now()) RETURNING {API_CLIENT_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(name)
    .bind(client_id)
    .bind(secret_hash)
    .bind(run_as)
    .bind(actor_id)
    .fetch_one(&mut *tx)
    .await?;
    let client = api_client(row);
    sqlx::query(
        "INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata) \
         VALUES ($1, $2, $3, $4, 'api_clients.client.created', $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(actor_id)
    .bind(correlation_id)
    .bind(format!("api_client:{}", client.id))
    .bind(serde_json::json!({"name": client.name, "run_as": run_as}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(client)
}

/// A workspace's API clients, newest first.
pub async fn api_clients(pool: &PgPool, tenant_id: Uuid) -> Result<Vec<ApiClient>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let rows: Vec<ApiClientRow> = sqlx::query_as(&format!(
        "SELECT {API_CLIENT_COLUMNS} FROM api_clients WHERE tenant_id = $1 ORDER BY created_at DESC, id DESC"
    ))
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(api_client).collect())
}

/// Revokes an API client and every token it holds, and audits it. Revoking
/// a revoked client changes nothing. `None` when there is no such client.
pub async fn revoke_api_client(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    actor_id: Uuid,
    correlation_id: Option<Uuid>,
) -> Result<Option<ApiClient>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let row: Option<ApiClientRow> = sqlx::query_as(&format!(
        "UPDATE api_clients SET revoked_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND revoked_at IS NULL RETURNING {API_CLIENT_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        // Already revoked, or no such client: nothing changes and nothing
        // is audited, so a retried request leaves no duplicate evidence.
        let existing: Option<ApiClientRow> = sqlx::query_as(&format!(
            "SELECT {API_CLIENT_COLUMNS} FROM api_clients WHERE tenant_id = $1 AND id = $2"
        ))
        .bind(tenant_id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(existing.map(api_client));
    };
    let deleted = sqlx::query("DELETE FROM api_client_tokens WHERE tenant_id = $1 AND client = $2")
        .bind(tenant_id)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata) \
         VALUES ($1, $2, $3, $4, 'api_clients.client.revoked', $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(actor_id)
    .bind(correlation_id)
    .bind(format!("api_client:{id}"))
    .bind(serde_json::json!({"tokens_revoked": deleted.rows_affected()}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(api_client(row)))
}

/// The workspace, member and client an issued API token is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedApiToken {
    pub tenant_id: Uuid,
    pub client: Uuid,
    pub run_as: Uuid,
}

/// Exchanges a client's id and secret hash for a token stored as
/// `token_hash`, through the bounded `platform_issue_api_token` routine.
/// `None` for unknown or revoked clients, wrong secrets, and clients whose
/// member left the workspace.
pub async fn issue_api_token(
    pool: &PgPool,
    client_id: &str,
    secret_hash: &str,
    token_hash: &str,
    expires_at: chrono::DateTime<chrono::Utc>,
    correlation_id: Option<Uuid>,
) -> Result<Option<IssuedApiToken>, DbError> {
    let row: Option<(Uuid, Uuid, Uuid)> = sqlx::query_as(
        "SELECT tenant_id, client, run_as FROM platform_issue_api_token($1, $2, $3, $4, $5)",
    )
    .bind(client_id)
    .bind(secret_hash)
    .bind(token_hash)
    .bind(expires_at)
    .bind(correlation_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(tenant_id, client, run_as)| IssuedApiToken {
        tenant_id,
        client,
        run_as,
    }))
}

type TokenAccessRow = (Uuid, String, Uuid, Uuid, Option<String>, Option<String>);

/// The member an API token acts as in one workspace, with their access,
/// and the client: `None` for unknown, expired or revoked tokens, other
/// workspaces, and members who left.
pub async fn resolve_api_token_access(
    pool: &PgPool,
    token_hash: &str,
    tenant_id: Uuid,
) -> Result<Option<(WorkspaceAccess, Uuid, Uuid)>, DbError> {
    let rows: Vec<TokenAccessRow> = sqlx::query_as(
        "SELECT tenant_id, display_name, user_id, client, role_name, permission_id \
         FROM platform_api_token_access($1, $2)",
    )
    .bind(token_hash)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    let mut found: Option<(WorkspaceAccess, Uuid, Uuid)> = None;
    for (tenant_id, display_name, user_id, client, role, permission) in rows {
        let (access, _, _) = found.get_or_insert_with(|| {
            (
                WorkspaceAccess {
                    tenant_id,
                    display_name,
                    roles: BTreeSet::new(),
                    permissions: BTreeSet::new(),
                },
                user_id,
                client,
            )
        });
        if let Some(role) = role {
            access.roles.insert(role);
        }
        if let Some(permission) = permission {
            access.permissions.insert(permission);
        }
    }
    Ok(found)
}

/// A workspace member as the member directory lists them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberSummary {
    pub id: Uuid,
    pub display_name: String,
    pub manager_user_id: Option<Uuid>,
}

/// Every member of a workspace with who they report to, by name.
pub async fn workspace_members(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<MemberSummary>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let rows: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT u.id, u.display_name, l.manager_user_id \
         FROM memberships m JOIN users u ON u.id = m.user_id \
         LEFT JOIN reporting_lines l ON l.tenant_id = m.tenant_id AND l.user_id = m.user_id \
         WHERE m.tenant_id = $1 ORDER BY u.display_name, u.id",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows
        .into_iter()
        .map(|(id, display_name, manager_user_id)| MemberSummary {
            id,
            display_name,
            manager_user_id,
        })
        .collect())
}

/// Every role defined in a workspace, by id.
pub async fn workspace_roles(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<BTreeMap<Uuid, String>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let rows: Vec<(Uuid, String)> =
        sqlx::query_as("SELECT id, name FROM roles WHERE tenant_id = $1 ORDER BY name, id")
            .bind(tenant_id)
            .fetch_all(&mut *tx)
            .await?;
    tx.commit().await?;
    Ok(rows.into_iter().collect())
}

/// One member's user record, or `None` when the user is not a member of the
/// workspace, so a lookup never reveals users outside it. Membership and
/// roles are read under the workspace's row-level security.
pub async fn workspace_member(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
) -> Result<Option<WorkspaceMember>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let member: Option<(Uuid, String, Option<String>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT u.id, u.display_name, u.email, u.created_at \
             FROM memberships m JOIN users u ON u.id = m.user_id \
             WHERE m.tenant_id = $1 AND m.user_id = $2",
        )
        .bind(tenant_id)
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some((id, display_name, email, created_at)) = member else {
        tx.commit().await?;
        return Ok(None);
    };
    let roles: Vec<String> = sqlx::query_scalar(
        "SELECT r.name FROM user_roles ur \
         JOIN roles r ON r.tenant_id = ur.tenant_id AND r.id = ur.role_id \
         WHERE ur.tenant_id = $1 AND ur.user_id = $2 ORDER BY r.name",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(WorkspaceMember {
        id,
        display_name,
        email,
        roles,
        created_at,
    }))
}

/// Begin a tenant transaction that runs inside one plugin's execution context.
///
/// `app.plugin_id` is set transaction-locally next to `app.tenant_id`.
/// Restrictive RLS policies then limit plugin-owned tables, job schedules, and
/// inbox/outbox writes to that plugin (see migration
/// `0012_plugin_execution_context.sql`). An invalid plugin identifier is
/// rejected before any statement runs.
pub async fn begin_plugin_tx(
    pool: &PgPool,
    tenant_id: Uuid,
    plugin_id: &str,
) -> Result<Transaction<'static, Postgres>, DbError> {
    if !valid_plugin_id(plugin_id) {
        return Err(DbError::Sqlx(sqlx::Error::Protocol(
            "invalid plugin execution context".into(),
        )));
    }
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    sqlx::query("SELECT set_config('app.plugin_id', $1, true)")
        .bind(plugin_id)
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}

/// A plugin identifier as accepted by manifests: dotted lowercase tokens.
pub fn valid_plugin_id(value: &str) -> bool {
    (3..=100).contains(&value.len())
        && value.split('.').count() >= 2
        && value.split('.').all(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

/// Resolve one external capability by its SHA-256 digest. The database function
/// intentionally has no enumeration or prefix-search equivalent.
pub async fn load_external_publication(
    pool: &PgPool,
    slug_digest: &str,
) -> Result<Option<ExternalPublicationRecord>, ExternalPublicationStateError> {
    type Row = (
        Uuid,
        Option<String>,
        serde_json::Value,
        i64,
        Vec<String>,
        bool,
        Option<serde_json::Value>,
    );
    let row: Option<Row> = sqlx::query_as(
        "SELECT tenant_id, portal_audience, compiled, generation, enabled_plugins, revoked, submission_policy FROM lookup_ui_external_publication($1)",
    )
    .bind(slug_digest)
    .fetch_optional(pool)
    .await?;
    row.map(
        |(tenant_id, portal_audience, compiled, generation, enabled_plugins, revoked, policy)| {
            Ok(ExternalPublicationRecord {
                tenant_id,
                portal_audience,
                compiled: serde_json::from_value(compiled)?,
                generation: generation
                    .try_into()
                    .map_err(|_| ExternalPublicationStateError::InvalidGeneration)?,
                enabled_plugins: enabled_plugins.into_iter().collect(),
                revoked,
                submission_policy: policy.map(serde_json::from_value).transpose()?,
            })
        },
    )
    .transpose()
}

/// Upsert an external publication through the tenant-scoped management path.
pub async fn save_external_publication(
    pool: &PgPool,
    slug_digest: &str,
    record: &ExternalPublicationRecord,
) -> Result<(), ExternalPublicationStateError> {
    let mut tx = begin_tenant_tx(pool, record.tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let host_value = serde_json::to_value(&record.compiled.host)?;
    let host = match host_value.as_str() {
        Some("public-form") => "public-form",
        Some("portal") => "portal",
        _ => return Err(ExternalPublicationStateError::InvalidHost),
    };
    sqlx::query(
        r#"
        INSERT INTO ui_external_publications (
            slug_digest, tenant_id, host, generation, portal_audience, compiled,
            submission_policy, enabled_plugins, revoked, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now())
        ON CONFLICT (slug_digest) DO UPDATE SET
            tenant_id = EXCLUDED.tenant_id,
            host = EXCLUDED.host,
            generation = EXCLUDED.generation,
            portal_audience = EXCLUDED.portal_audience,
            compiled = EXCLUDED.compiled,
            submission_policy = EXCLUDED.submission_policy,
            enabled_plugins = EXCLUDED.enabled_plugins,
            revoked = EXCLUDED.revoked,
            updated_at = now()
        "#,
    )
    .bind(slug_digest)
    .bind(record.tenant_id)
    .bind(host)
    .bind(
        i64::try_from(record.generation)
            .map_err(|_| ExternalPublicationStateError::InvalidGeneration)?,
    )
    .bind(&record.portal_audience)
    .bind(serde_json::to_value(&record.compiled)?)
    .bind(
        record
            .submission_policy
            .as_ref()
            .map(serde_json::to_value)
            .transpose()?,
    )
    .bind(record.enabled_plugins.iter().cloned().collect::<Vec<_>>())
    .bind(record.revoked)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

pub async fn load_ui_metadata_page_state(
    pool: &PgPool,
    tenant_id: Uuid,
    page_id: &str,
) -> Result<Option<TenantPagePublications>, UiMetadataStateError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let value: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT state FROM ui_metadata_page_state WHERE tenant_id = $1 AND page_id = $2",
    )
    .bind(tenant_id)
    .bind(page_id)
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    value
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}

/// Every page publication aggregate of one workspace, for building its UI
/// delivery snapshot (published patches and publication generations).
pub async fn list_ui_metadata_page_states(
    pool: &PgPool,
    tenant_id: Uuid,
) -> Result<Vec<TenantPagePublications>, UiMetadataStateError> {
    let mut tx = begin_tenant_tx(pool, tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    let values: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT state FROM ui_metadata_page_state WHERE tenant_id = $1 ORDER BY page_id",
    )
    .bind(tenant_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    values
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<_, _>>()
        .map_err(Into::into)
}

/// Persist a page aggregate with database-level optimistic concurrency and
/// atomically append its redacted audit records.
pub async fn save_ui_metadata_page_state(
    pool: &PgPool,
    state: &TenantPagePublications,
    expected_revision: u64,
) -> Result<(), UiMetadataStateError> {
    let mut tx = begin_tenant_tx(pool, state.tenant_id)
        .await
        .map_err(|DbError::Sqlx(error)| error)?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("{}:{}", state.tenant_id, state.page_id))
        .execute(&mut *tx)
        .await?;
    let actual: Option<i64> = sqlx::query_scalar(
        "SELECT revision FROM ui_metadata_page_state WHERE tenant_id = $1 AND page_id = $2",
    )
    .bind(state.tenant_id)
    .bind(&state.page_id)
    .fetch_optional(&mut *tx)
    .await?;
    let actual_revision = actual.and_then(|value| value.try_into().ok());
    if actual_revision.unwrap_or(0) != expected_revision
        || (actual_revision.is_none() && expected_revision != 0)
    {
        tx.rollback().await?;
        return Err(UiMetadataStateError::StaleRevision {
            expected: expected_revision,
            actual: actual_revision,
        });
    }
    let result = sqlx::query(
        r#"
        INSERT INTO ui_metadata_page_state (
            tenant_id, page_id, revision, publication_generation, state, updated_at
        ) VALUES ($1, $2, $3, $4, $5, now())
        ON CONFLICT (tenant_id, page_id) DO UPDATE SET
            revision = EXCLUDED.revision,
            publication_generation = EXCLUDED.publication_generation,
            state = EXCLUDED.state,
            updated_at = now()
        "#,
    )
    .bind(state.tenant_id)
    .bind(&state.page_id)
    .bind(state.revision as i64)
    .bind(state.publication_generation as i64)
    .bind(serde_json::to_value(state)?)
    .execute(&mut *tx)
    .await?;
    debug_assert_eq!(result.rows_affected(), 1);
    for event in &state.audit {
        sqlx::query(
            r#"
            INSERT INTO audit_log (id, tenant_id, actor_id, action, target, metadata, occurred_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7)
            ON CONFLICT (tenant_id, id) DO NOTHING
            "#,
        )
        .bind(event.id)
        .bind(state.tenant_id)
        .bind(event.actor_id)
        .bind(format!("ui.metadata.{}", event.action.as_str()))
        .bind(format!("ui-page:{}", state.page_id))
        .bind(serde_json::json!({
            "revision": event.revision,
            "draft_id": event.draft_id,
            "publication_version": event.publication_version,
        }))
        .bind(event.occurred_at)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    const MIGRATION: &str = include_str!("../migrations/0002_ui_metadata_publications.sql");
    const EXTERNAL_MIGRATION: &str =
        include_str!("../migrations/0003_external_ui_publications.sql");
    const AUTH_MIGRATION: &str = include_str!("../migrations/0004_auth_sessions_rbac.sql");
    const PLUGIN_MIGRATION: &str = include_str!("../migrations/0005_plugin_registry_lifecycle.sql");
    const AUDIT_CONTEXT_MIGRATION: &str =
        include_str!("../migrations/0006_audit_event_context.sql");
    const OUTBOX_PUBLISHER_MIGRATION: &str =
        include_str!("../migrations/0007_outbox_publisher.sql");
    const INBOX_CONSUMPTION_MIGRATION: &str =
        include_str!("../migrations/0008_inbox_consumption.sql");
    const TENANT_RLS_ASSURANCE_MIGRATION: &str =
        include_str!("../migrations/0009_tenant_rls_assurance.sql");
    const PLUGIN_WORKER_CONTRIBUTIONS_MIGRATION: &str =
        include_str!("../migrations/0010_plugin_worker_contributions.sql");
    const RBAC_TENANT_RLS_MIGRATION: &str = include_str!("../migrations/0011_rbac_tenant_rls.sql");
    const PLUGIN_EXECUTION_CONTEXT_MIGRATION: &str =
        include_str!("../migrations/0012_plugin_execution_context.sql");

    #[test]
    fn publication_state_schema_forces_tenant_isolation_and_tracks_generation() {
        assert!(MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(MIGRATION.contains("current_setting('app.tenant_id'"));
        assert!(MIGRATION.contains("publication_generation"));
        assert!(MIGRATION.contains("PRIMARY KEY (tenant_id, page_id)"));
    }

    #[test]
    fn external_publications_are_digest_only_non_enumerating_and_tenant_managed() {
        assert!(EXTERNAL_MIGRATION.contains("slug_digest text PRIMARY KEY"));
        assert!(!EXTERNAL_MIGRATION.contains("raw_slug"));
        assert!(EXTERNAL_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(EXTERNAL_MIGRATION.contains("SECURITY DEFINER"));
        assert!(EXTERNAL_MIGRATION.contains("SET search_path = pg_catalog, public"));
        assert!(EXTERNAL_MIGRATION.contains("REVOKE ALL ON FUNCTION"));
        assert!(!EXTERNAL_MIGRATION.contains("LIKE p_slug_digest"));
    }

    #[test]
    fn auth_schema_stores_only_session_hashes_and_one_time_oidc_state() {
        assert!(AUTH_MIGRATION.contains("token_hash text"));
        assert!(AUTH_MIGRATION.contains("UNIQUE INDEX IF NOT EXISTS sessions_token_hash_idx"));
        assert!(AUTH_MIGRATION.contains("state_hash text PRIMARY KEY"));
        assert!(AUTH_MIGRATION.contains("pkce_verifier text NOT NULL"));
        assert!(!AUTH_MIGRATION.contains("access_token"));
        assert!(!AUTH_MIGRATION.contains("refresh_token"));
    }

    #[test]
    fn plugin_schema_persists_verified_candidates_and_interrupted_transitions() {
        assert!(PLUGIN_MIGRATION.contains("CREATE TABLE plugin_versions"));
        assert!(PLUGIN_MIGRATION.contains("verified_digest text CHECK"));
        assert!(PLUGIN_MIGRATION.contains("latest_usable_version"));
        assert!(PLUGIN_MIGRATION.contains("transition_id uuid"));
        assert!(PLUGIN_MIGRATION.contains("WHERE state IN ('enabling', 'disabling')"));
    }

    #[test]
    fn activation_projection_joins_the_exact_installed_registry_version() {
        assert!(super::TENANT_PLUGIN_ACTIVATION_PROJECTION_SQL
            .contains("v.plugin_id = i.plugin_id AND v.version = i.active_version"));
        assert!(super::TENANT_PLUGIN_ACTIVATION_PROJECTION_SQL.contains("i.tenant_id = $1"));
        assert!(super::TENANT_PLUGIN_ACTIVATION_PROJECTION_SQL.contains("v.lifecycle_state"));
        assert!(super::TENANT_PLUGIN_ACTIVATION_PROJECTION_SQL.contains("v.verified_digest"));
    }

    #[test]
    fn audit_schema_tracks_event_correlation_without_requiring_legacy_producers() {
        assert!(AUDIT_CONTEXT_MIGRATION.contains("ADD COLUMN correlation_id uuid"));
        assert!(AUDIT_CONTEXT_MIGRATION.contains("ADD COLUMN causation_id uuid"));
        assert!(AUDIT_CONTEXT_MIGRATION.contains("WHERE correlation_id IS NOT NULL"));
        assert!(!AUDIT_CONTEXT_MIGRATION.contains("correlation_id uuid NOT NULL"));
        assert!(!AUDIT_CONTEXT_MIGRATION.contains("causation_id uuid NOT NULL"));
    }

    #[test]
    fn outbox_schema_supports_expiring_claims_and_observable_retries() {
        assert!(OUTBOX_PUBLISHER_MIGRATION.contains("attempt_count integer NOT NULL DEFAULT 0"));
        assert!(OUTBOX_PUBLISHER_MIGRATION.contains("failure_count integer NOT NULL DEFAULT 0"));
        assert!(OUTBOX_PUBLISHER_MIGRATION
            .contains("next_attempt_at timestamptz NOT NULL DEFAULT now()"));
        assert!(OUTBOX_PUBLISHER_MIGRATION
            .contains("(claim_token IS NULL) = (claim_expires_at IS NULL)"));
        assert!(OUTBOX_PUBLISHER_MIGRATION.contains("published_at IS NULL OR claim_token IS NULL"));
        assert!(OUTBOX_PUBLISHER_MIGRATION
            .contains("NOT (published_at IS NOT NULL AND quarantined_at IS NOT NULL)"));
        assert!(
            OUTBOX_PUBLISHER_MIGRATION.contains("quarantined_at IS NULL OR claim_token IS NULL")
        );
        assert!(OUTBOX_PUBLISHER_MIGRATION.contains("ON outbox (next_attempt_at, created_at, id)"));
        assert!(OUTBOX_PUBLISHER_MIGRATION.contains("INCLUDE (claim_expires_at)"));
        assert!(OUTBOX_PUBLISHER_MIGRATION
            .contains("WHERE published_at IS NULL AND quarantined_at IS NULL"));
        assert!(OUTBOX_PUBLISHER_MIGRATION.contains("CREATE INDEX outbox_quarantined_idx"));
    }

    #[test]
    fn inbox_schema_is_tenant_isolated_bounded_and_append_only() {
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("FOREIGN KEY (tenant_id)"));
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("ON DELETE RESTRICT"));
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("octet_length(consumer_id) BETWEEN 1 AND 100"));
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("ENABLE ROW LEVEL SECURITY"));
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("FORCE ROW LEVEL SECURITY"));
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("current_setting('app.tenant_id'"));
        assert!(INBOX_CONSUMPTION_MIGRATION.contains("REVOKE UPDATE, DELETE ON inbox"));
    }

    #[test]
    fn remaining_tenant_tables_are_forced_through_bounded_runtime_roles() {
        assert!(super::CORE_MIGRATION_SENTINEL >= "0009_tenant_rls_assurance.sql");
        for table in ["tenant_plugin_installations", "audit_log", "outbox"] {
            assert!(TENANT_RLS_ASSURANCE_MIGRATION
                .contains(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY")));
        }
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("TO platform_app"));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("current_setting('app.tenant_id'"));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("TO platform_worker"));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains(
            "GRANT EXECUTE ON FUNCTION platform_recover_plugin_transitions() TO platform_worker"
        ));
        assert!(!TENANT_RLS_ASSURANCE_MIGRATION.contains(
            "GRANT EXECUTE ON FUNCTION platform_recover_plugin_transitions() TO platform_app"
        ));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("SECURITY DEFINER"));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("SET search_path = pg_catalog, public"));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("FROM PUBLIC"));
        assert!(
            TENANT_RLS_ASSURANCE_MIGRATION.contains("UNIQUE NULLS NOT DISTINCT (tenant_id, id)")
        );
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("ADD PRIMARY KEY (tenant_id, id)"));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains(
            "REVOKE EXECUTE ON ALL FUNCTIONS IN SCHEMA %I FROM PUBLIC, platform_app, platform_worker"
        ));
        assert!(TENANT_RLS_ASSURANCE_MIGRATION.contains("WHERE tenant_id = p_tenant_id"));
    }

    #[test]
    fn plugin_job_schedules_are_tenant_isolated_and_lease_settled() {
        assert!(super::CORE_MIGRATION_SENTINEL >= "0010_plugin_worker_contributions.sql");
        let migration = PLUGIN_WORKER_CONTRIBUTIONS_MIGRATION;
        assert!(migration.contains("ALTER TABLE plugin_job_schedules FORCE ROW LEVEL SECURITY"));
        assert!(migration.contains("REFERENCES tenants(id) ON DELETE RESTRICT"));
        assert!(migration.contains("REVOKE ALL ON plugin_job_schedules FROM platform_app"));
        let column_grant = migration
            .split_once("GRANT SELECT (")
            .and_then(|(_, rest)| rest.split_once(") ON plugin_job_schedules TO platform_app"))
            .map(|(columns, _)| columns)
            .expect("column-level SELECT grant");
        assert!(column_grant.contains("checkpoint"));
        assert!(!column_grant.contains("claim_token"));
        assert!(migration.contains("schedule.claim_expires_at > clock_timestamp()"));
        assert!(migration.contains("pg_advisory_xact_lock"));
        // Claims only select enabled, health-checked installations.
        assert!(migration.contains("installation.state = 'enabled'"));
        assert!(migration.contains("version.lifecycle_state = 'health_checked'"));
        // Completion takes the tenant only from the transaction-local context
        // and requires a live lease.
        assert!(migration.contains("nullif(current_setting('app.tenant_id', true), '')::uuid"));
        assert!(!migration.contains("DELETE FROM"));
        assert!(migration.contains(
            "GRANT EXECUTE ON FUNCTION platform_complete_plugin_job(text, text, uuid, jsonb) TO platform_app"
        ));
        assert!(!migration.contains(
            "platform_claim_plugin_jobs(text, text, bigint, uuid, bigint) TO platform_app"
        ));
        assert_eq!(migration.matches("SECURITY DEFINER").count(), 5);
        assert_eq!(
            migration
                .matches("SET search_path = pg_catalog, public")
                .count(),
            5
        );
    }

    #[test]
    fn rbac_tables_force_rls_and_are_read_only_to_the_request_role() {
        assert!(super::CORE_MIGRATION_SENTINEL >= "0011_rbac_tenant_rls.sql");
        let migration = RBAC_TENANT_RLS_MIGRATION;
        for table in [
            "memberships",
            "roles",
            "role_permissions",
            "user_roles",
            "idempotency_keys",
            "entitlement_snapshots",
            "deployment_placements",
        ] {
            assert!(migration.contains(&format!("ALTER TABLE {table} FORCE ROW LEVEL SECURITY")));
            assert!(migration.contains(&format!(
                "CREATE POLICY {table}_tenant_isolation ON {table} TO platform_app"
            )));
        }
        assert!(migration.contains(
            "REVOKE INSERT, UPDATE, DELETE, TRUNCATE ON memberships, roles, role_permissions, user_roles FROM platform_app"
        ));
        // The bounded lookup only answers for a live session's own user.
        assert!(migration.contains("s.revoked_at IS NULL"));
        assert!(migration.contains("s.expires_at > now()"));
        assert!(migration.contains("s.user_id = p_user_id"));
        assert!(migration.contains("SECURITY DEFINER"));
        assert!(migration.contains("SET search_path = pg_catalog, public"));
    }

    #[test]
    fn plugin_execution_context_restricts_plugin_state_and_ledger_writes() {
        assert!(super::CORE_MIGRATION_SENTINEL >= "0012_plugin_execution_context.sql");
        let migration = PLUGIN_EXECUTION_CONTEXT_MIGRATION;
        assert_eq!(migration.matches("AS RESTRICTIVE").count(), 3);
        assert!(migration.contains("current_setting('app.plugin_id', true)"));
        assert!(migration.contains("envelope->>'source_plugin'"));
        assert!(migration.contains("p_plugin_id IS DISTINCT FROM context_plugin"));
        assert!(migration.contains("SET search_path = pg_catalog, public"));
    }

    #[test]
    fn plugin_ids_are_validated_before_setting_context() {
        for valid in ["platform.contacts", "acme.field-service", "a1.b2"] {
            assert!(super::valid_plugin_id(valid), "{valid}");
        }
        for invalid in [
            "",
            "contacts",
            "platform.",
            ".contacts",
            "Platform.contacts",
            "platform.*",
            "a.b c",
        ] {
            assert!(!super::valid_plugin_id(invalid), "{invalid}");
        }
    }
}
