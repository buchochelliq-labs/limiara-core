//! Devices (ADR 0029, ADR 0024 §4): apps that sign a member in through the
//! system browser and hold a replica of one workspace. Codes and tokens are
//! random and stored only as SHA-256 hashes.

use crate::{begin_tenant_tx, DbError, WorkspaceAccess};
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use std::collections::BTreeSet;
use thiserror::Error;
use uuid::Uuid;

/// What a device app asked for when it started a sign-in: where to return
/// and its own PKCE challenge, which its verifier must match later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceLogin {
    pub redirect_uri: String,
    pub code_challenge: String,
    /// Returned to the app unchanged, so it can match the response to the
    /// sign-in it started.
    pub state: String,
}

/// Active devices per member and workspace.
pub const DEVICES_PER_MEMBER_MAX: i64 = 10;

/// A device as its member and administrators see it. Push tokens are never
/// shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: Uuid,
    pub user_id: Uuid,
    pub name: String,
    pub platform: String,
    pub push_provider: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub revoked_by: Option<Uuid>,
    pub revoked_reason: Option<String>,
}

const DEVICE_COLUMNS: &str = "id, user_id, name, platform, push_provider, created_at, \
     last_seen_at, revoked_at, revoked_by, revoked_reason";

type DeviceRow = (
    Uuid,
    Uuid,
    String,
    String,
    Option<String>,
    DateTime<Utc>,
    DateTime<Utc>,
    Option<DateTime<Utc>>,
    Option<Uuid>,
    Option<String>,
);

fn device(row: DeviceRow) -> Device {
    let (
        id,
        user_id,
        name,
        platform,
        push_provider,
        created_at,
        last_seen_at,
        revoked_at,
        revoked_by,
        revoked_reason,
    ) = row;
    Device {
        id,
        user_id,
        name,
        platform,
        push_provider,
        created_at,
        last_seen_at,
        revoked_at,
        revoked_by,
        revoked_reason,
    }
}

/// Stores a one-time code for a signed-in person, for the device app whose
/// PKCE challenge it carries.
pub async fn create_device_authorization(
    pool: &PgPool,
    code_hash: &str,
    user_id: Uuid,
    login: &DeviceLogin,
    expires_at: DateTime<Utc>,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM device_authorizations WHERE expires_at <= now()")
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "INSERT INTO device_authorizations (code_hash, user_id, code_challenge, redirect_uri, expires_at) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(code_hash)
    .bind(user_id)
    .bind(&login.code_challenge)
    .bind(&login.redirect_uri)
    .bind(expires_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum DeviceError {
    /// The code is unknown, expired or already used, or the verifier or
    /// redirect URI does not match the sign-in it came from.
    #[error("invalid grant")]
    InvalidGrant,
    /// The person is not a member of the workspace.
    #[error("not a workspace member")]
    NotMember,
    /// The member already has [`DEVICES_PER_MEMBER_MAX`] active devices.
    #[error("too many devices")]
    LimitReached,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<DbError> for DeviceError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::Sqlx(error) => Self::Sqlx(error),
        }
    }
}

/// What a device app sends to register itself with its first tokens.
#[derive(Debug, Clone)]
pub struct NewDevice<'a> {
    pub code_hash: &'a str,
    /// The S256 challenge of the verifier the app presented.
    pub code_challenge: &'a str,
    pub redirect_uri: &'a str,
    pub name: &'a str,
    pub platform: &'a str,
    pub access_hash: &'a str,
    pub refresh_hash: &'a str,
    pub access_expires_at: DateTime<Utc>,
    pub refresh_expires_at: DateTime<Utc>,
    pub correlation_id: Option<Uuid>,
}

/// Redeems a one-time code: registers a device for its person in one
/// workspace and stores its first access and refresh tokens, and audits
/// it. The code is used up whether or not the rest matches.
pub async fn register_device(
    pool: &PgPool,
    tenant_id: Uuid,
    request: &NewDevice<'_>,
) -> Result<Device, DeviceError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let code: Option<(Uuid, String, String)> = sqlx::query_as(
        "DELETE FROM device_authorizations WHERE code_hash = $1 AND expires_at > now() \
         RETURNING user_id, code_challenge, redirect_uri",
    )
    .bind(request.code_hash)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((user_id, challenge, redirect_uri)) = code else {
        tx.commit().await?;
        return Err(DeviceError::InvalidGrant);
    };
    if challenge != request.code_challenge || redirect_uri != request.redirect_uri {
        tx.commit().await?;
        return Err(DeviceError::InvalidGrant);
    }
    let member: Option<Uuid> =
        sqlx::query_scalar("SELECT user_id FROM memberships WHERE tenant_id = $1 AND user_id = $2")
            .bind(tenant_id)
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await?;
    if member.is_none() {
        tx.commit().await?;
        return Err(DeviceError::NotMember);
    }
    // One member's sign-ins are counted one at a time.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("devices:{tenant_id}:{user_id}"))
        .execute(&mut *tx)
        .await?;
    let active: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM devices WHERE tenant_id = $1 AND user_id = $2 AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if active >= DEVICES_PER_MEMBER_MAX {
        tx.commit().await?;
        return Err(DeviceError::LimitReached);
    }
    let row: DeviceRow = sqlx::query_as(&format!(
        "INSERT INTO devices (tenant_id, id, user_id, name, platform, created_at, last_seen_at) \
         VALUES ($1, $2, $3, $4, $5, now(), now()) RETURNING {DEVICE_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(request.name)
    .bind(request.platform)
    .fetch_one(&mut *tx)
    .await?;
    let registered = device(row);
    sqlx::query(
        "INSERT INTO device_tokens (tenant_id, token_hash, device, kind, created_at, expires_at) \
         VALUES ($1, $2, $3, 'access', now(), $4), ($1, $5, $3, 'refresh', now(), $6)",
    )
    .bind(tenant_id)
    .bind(request.access_hash)
    .bind(registered.id)
    .bind(request.access_expires_at)
    .bind(request.refresh_hash)
    .bind(request.refresh_expires_at)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata) \
         VALUES ($1, $2, $3, $4, 'devices.device.registered', $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(user_id)
    .bind(request.correlation_id)
    .bind(format!("device:{}", registered.id))
    .bind(serde_json::json!({"name": registered.name, "platform": registered.platform}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(registered)
}

/// What exchanging a refresh token did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
    /// New tokens are stored for the device.
    Issued { tenant_id: Uuid, device: Uuid },
    /// The device must delete its replica: it was revoked, its member left,
    /// or the refresh token was presented again, which revokes it.
    Wipe { tenant_id: Uuid, device: Uuid },
}

/// New tokens for the hashes given, from a refresh token, through the
/// bounded `platform_refresh_device_token` routine. `None` for unknown or
/// expired refresh tokens.
#[allow(clippy::too_many_arguments)]
pub async fn refresh_device_token(
    pool: &PgPool,
    refresh_hash: &str,
    access_hash: &str,
    new_refresh_hash: &str,
    access_expires_at: DateTime<Utc>,
    refresh_expires_at: DateTime<Utc>,
    correlation_id: Option<Uuid>,
) -> Result<Option<RefreshOutcome>, DbError> {
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT tenant_id, device, outcome FROM platform_refresh_device_token($1, $2, $3, $4, $5, $6)",
    )
    .bind(refresh_hash)
    .bind(access_hash)
    .bind(new_refresh_hash)
    .bind(access_expires_at)
    .bind(refresh_expires_at)
    .bind(correlation_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(tenant_id, device, outcome)| {
        if outcome == "issued" {
            RefreshOutcome::Issued { tenant_id, device }
        } else {
            RefreshOutcome::Wipe { tenant_id, device }
        }
    }))
}

/// Who an access token's device acts for in one workspace.
#[derive(Debug, Clone)]
pub struct DeviceTokenAccess {
    pub access: WorkspaceAccess,
    pub user_id: Uuid,
    pub device: Uuid,
    /// The device was revoked or its member left: it must delete its
    /// replica, and the token authorises nothing.
    pub wipe: bool,
}

type DeviceAccessRow = (
    Uuid,
    String,
    Uuid,
    Uuid,
    bool,
    Option<String>,
    Option<String>,
);

/// The device an unexpired access token belongs to in one workspace, with
/// its member's access: `None` for unknown or expired tokens and other
/// workspaces.
pub async fn resolve_device_token_access(
    pool: &PgPool,
    token_hash: &str,
    tenant_id: Uuid,
) -> Result<Option<DeviceTokenAccess>, DbError> {
    let rows: Vec<DeviceAccessRow> = sqlx::query_as(
        "SELECT tenant_id, display_name, user_id, device, wipe, role_name, permission_id \
         FROM platform_device_token_access($1, $2)",
    )
    .bind(token_hash)
    .bind(tenant_id)
    .fetch_all(pool)
    .await?;
    let mut found: Option<DeviceTokenAccess> = None;
    for (tenant_id, display_name, user_id, device, wipe, role, permission) in rows {
        let entry = found.get_or_insert_with(|| DeviceTokenAccess {
            access: WorkspaceAccess {
                tenant_id,
                display_name,
                roles: BTreeSet::new(),
                permissions: BTreeSet::new(),
            },
            user_id,
            device,
            wipe,
        });
        if let Some(role) = role {
            entry.access.roles.insert(role);
        }
        if let Some(permission) = permission {
            entry.access.permissions.insert(permission);
        }
    }
    Ok(found)
}

/// A workspace's devices, newest first: one member's, or everyone's.
pub async fn devices(
    pool: &PgPool,
    tenant_id: Uuid,
    user_id: Option<Uuid>,
) -> Result<Vec<Device>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let rows: Vec<DeviceRow> = sqlx::query_as(&format!(
        "SELECT {DEVICE_COLUMNS} FROM devices WHERE tenant_id = $1 AND ($2::uuid IS NULL OR user_id = $2) \
         ORDER BY created_at DESC, id DESC"
    ))
    .bind(tenant_id)
    .bind(user_id)
    .fetch_all(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rows.into_iter().map(device).collect())
}

/// Why a device stops working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeReason {
    /// A member or an administrator revoked it.
    Revoked,
    /// The device signed itself out.
    SignedOut,
}

impl RevokeReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::Revoked => "revoked",
            Self::SignedOut => "signed_out",
        }
    }
}

/// Revokes a device, limited to `owner`'s devices when given, and audits
/// it. Its access and refresh tokens then authorise nothing, and
/// presenting either tells the device to wipe its replica. Revoking a
/// revoked device changes nothing.
/// `None` when there is no such device.
pub async fn revoke_device(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    owner: Option<Uuid>,
    actor_id: Uuid,
    reason: RevokeReason,
    correlation_id: Option<Uuid>,
) -> Result<Option<Device>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let row: Option<DeviceRow> = sqlx::query_as(&format!(
        "UPDATE devices SET revoked_at = now(), revoked_by = $4, revoked_reason = $5, \
         push_provider = NULL, push_token = NULL \
         WHERE tenant_id = $1 AND id = $2 AND ($3::uuid IS NULL OR user_id = $3) AND revoked_at IS NULL \
         RETURNING {DEVICE_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(owner)
    .bind(actor_id)
    .bind(reason.as_str())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(row) = row else {
        // Already revoked, or no such device: nothing changes and nothing
        // is audited, so a retried request leaves no duplicate evidence.
        let existing: Option<DeviceRow> = sqlx::query_as(&format!(
            "SELECT {DEVICE_COLUMNS} FROM devices WHERE tenant_id = $1 AND id = $2 \
             AND ($3::uuid IS NULL OR user_id = $3)"
        ))
        .bind(tenant_id)
        .bind(id)
        .bind(owner)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        return Ok(existing.map(device));
    };
    // Tokens stay until they expire: they authorise nothing now, and
    // presenting either kind tells the device to wipe its replica.
    sqlx::query(
        "INSERT INTO audit_log (id, tenant_id, actor_id, correlation_id, action, target, metadata) \
         VALUES ($1, $2, $3, $4, 'devices.device.revoked', $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant_id)
    .bind(actor_id)
    .bind(correlation_id)
    .bind(format!("device:{id}"))
    .bind(serde_json::json!({"reason": reason.as_str()}))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(device(row)))
}

/// Sets or clears where a device's push notifications go. `None` when the
/// device is unknown or revoked. The device counts as told about the
/// workspace as it is now, since an app registers its token after syncing.
pub async fn set_device_push(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    push: Option<(&str, &str)>,
) -> Result<Option<Device>, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let row: Option<DeviceRow> = sqlx::query_as(&format!(
        "UPDATE devices SET push_provider = $3, push_token = $4, last_seen_at = now(), \
         notified_position = coalesce((SELECT last_position FROM change_counters WHERE tenant_id = $1), 0), \
         notified_metadata_version = coalesce((SELECT metadata_version FROM change_counters WHERE tenant_id = $1), 0) \
         WHERE tenant_id = $1 AND id = $2 AND revoked_at IS NULL RETURNING {DEVICE_COLUMNS}"
    ))
    .bind(tenant_id)
    .bind(id)
    .bind(push.map(|(provider, _)| provider))
    .bind(push.map(|(_, token)| token))
    .fetch_optional(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(row.map(device))
}

/// A device the dispatcher claimed to tell that its workspace changed
/// (ADR 0030 §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueNotification {
    pub tenant_id: Uuid,
    pub device: Uuid,
    pub push_provider: String,
    pub push_token: String,
    /// What to record once the provider accepts the notification.
    pub change_position: i64,
    pub metadata_version: i64,
}

/// Claims devices to notify across workspaces through the bounded
/// `platform_claim_device_notifications` routine, the only cross-tenant
/// step. A claimed device is not claimed again for `quiet_seconds`.
pub async fn claim_device_notifications(
    worker_pool: &PgPool,
    limit: i32,
    quiet_seconds: i32,
) -> Result<Vec<DueNotification>, sqlx::Error> {
    let rows: Vec<(Uuid, Uuid, String, String, i64, i64)> = sqlx::query_as(
        "SELECT tenant_id, device, push_provider, push_token, change_position, metadata_version \
         FROM platform_claim_device_notifications($1, $2)",
    )
    .bind(limit)
    .bind(quiet_seconds)
    .fetch_all(worker_pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(tenant_id, device, push_provider, push_token, change_position, metadata_version)| {
                DueNotification {
                    tenant_id,
                    device,
                    push_provider,
                    push_token,
                    change_position,
                    metadata_version,
                }
            },
        )
        .collect())
}

/// Records that a device was told about its workspace up to
/// `change_position` and `metadata_version`. Never moves backwards, so a
/// late record cannot undo a newer one.
pub async fn record_device_notification(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    change_position: i64,
    metadata_version: i64,
) -> Result<(), DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    sqlx::query(
        "UPDATE devices SET notified_position = greatest(notified_position, $3), \
         notified_metadata_version = greatest(notified_metadata_version, $4) \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(tenant_id)
    .bind(id)
    .bind(change_position)
    .bind(metadata_version)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Forgets a push token the provider no longer knows. Only that token is
/// cleared: one the app registered since stays. Returns whether it was
/// still registered.
pub async fn forget_device_push_token(
    pool: &PgPool,
    tenant_id: Uuid,
    id: Uuid,
    push_token: &str,
) -> Result<bool, DbError> {
    let mut tx = begin_tenant_tx(pool, tenant_id).await?;
    let forgotten = sqlx::query(
        "UPDATE devices SET push_provider = NULL, push_token = NULL \
         WHERE tenant_id = $1 AND id = $2 AND push_token = $3",
    )
    .bind(tenant_id)
    .bind(id)
    .bind(push_token)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(forgotten > 0)
}
