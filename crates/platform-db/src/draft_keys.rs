//! Browser draft keys (ADR 0036 §4): one random 256-bit key per user and
//! browser, from which the browser derives a per-workspace AES-256-GCM key
//! for the record drafts it keeps in IndexedDB. The table is user-scoped,
//! like sessions; it holds no record data. Keys are returned only to a
//! browser session of their own user and are never logged.

use crate::DbError;
use sqlx::PgPool;
use uuid::Uuid;

/// Browser keys one user may hold; creating one more deletes the least
/// recently used.
pub const DRAFT_KEYS_PER_USER_MAX: i64 = 20;

/// Keys unused for longer than the longest allowed draft expiry are
/// deleted by the worker: no draft they could open is still kept.
pub const DRAFT_KEY_RETENTION_DAYS: i32 = 90;

/// The length of a draft key in bytes.
pub const DRAFT_KEY_BYTES: usize = 32;

/// Returns this user's key for this browser, creating it from `fresh` on
/// first use, and marks it used. Creating a key beyond
/// [`DRAFT_KEYS_PER_USER_MAX`] deletes the user's least recently used keys,
/// never the one returned. Concurrent requests for one user are serialised
/// on the user's row so the cap holds.
pub async fn draft_key(
    pool: &PgPool,
    user_id: Uuid,
    browser_id: Uuid,
    fresh: &[u8; DRAFT_KEY_BYTES],
) -> Result<Option<Vec<u8>>, DbError> {
    let mut tx = pool.begin().await?;
    let user: Option<Uuid> = sqlx::query_scalar("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    if user.is_none() {
        tx.rollback().await?;
        return Ok(None);
    }
    let key: Vec<u8> = sqlx::query_scalar(
        "INSERT INTO draft_keys (user_id, browser_id, key) VALUES ($1, $2, $3) \
         ON CONFLICT (user_id, browser_id) DO UPDATE SET last_used_at = now() \
         RETURNING key",
    )
    .bind(user_id)
    .bind(browser_id)
    .bind(fresh.as_slice())
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM draft_keys WHERE user_id = $1 AND browser_id NOT IN ( \
           SELECT browser_id FROM draft_keys WHERE user_id = $1 \
           ORDER BY (browser_id = $2) DESC, last_used_at DESC, created_at DESC \
           LIMIT $3)",
    )
    .bind(user_id)
    .bind(browser_id)
    .bind(DRAFT_KEYS_PER_USER_MAX)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(key))
}

/// Revokes the session with this token hash and, in the same transaction,
/// deletes its user's key for `browser`, so the drafts left in that browser
/// can no longer be decrypted (ADR 0036 §11). A session already revoked or
/// expired still identifies its user, so signing out again deletes the key.
pub async fn revoke_session_and_draft_key(
    pool: &PgPool,
    token_hash: &str,
    browser: Option<Uuid>,
) -> Result<(), DbError> {
    let mut tx = pool.begin().await?;
    let user: Option<Uuid> = sqlx::query_scalar(
        "UPDATE sessions SET revoked_at = COALESCE(revoked_at, now()) WHERE token_hash = $1 \
         RETURNING user_id",
    )
    .bind(token_hash)
    .fetch_optional(&mut *tx)
    .await?;
    if let (Some(user), Some(browser)) = (user, browser) {
        sqlx::query("DELETE FROM draft_keys WHERE user_id = $1 AND browser_id = $2")
            .bind(user)
            .bind(browser)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Deletes every draft key of a user, for when the user is deactivated.
/// Returns how many were deleted.
pub async fn delete_user_draft_keys(pool: &PgPool, user_id: Uuid) -> Result<u64, DbError> {
    Ok(sqlx::query("DELETE FROM draft_keys WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?
        .rows_affected())
}

/// How many draft keys a user holds.
pub async fn count_user_draft_keys(pool: &PgPool, user_id: Uuid) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM draft_keys WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(pool)
            .await?,
    )
}

/// Deletes up to `limit` keys unused for longer than `unused_days`, through
/// the worker's bounded routine. Returns how many were deleted.
pub async fn purge_draft_keys(
    worker_pool: &PgPool,
    unused_days: i32,
    limit: i32,
) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT platform_purge_draft_keys($1, $2)")
            .bind(unused_days)
            .bind(limit)
            .fetch_one(worker_pool)
            .await?,
    )
}
