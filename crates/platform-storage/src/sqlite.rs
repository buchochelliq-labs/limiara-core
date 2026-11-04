//! SQLite storage: one database file per workspace (ADR 0013).
//!
//! SQLite has no row-level security, so tenant isolation is physical: a
//! [`SqliteTenantDatabases`] opens only `<directory>/<tenant id>.sqlite` for a
//! workspace, and every tenant table still carries `tenant_id` and every
//! query still filters on it. Each file holds the kernel's tenant tables
//! (audit and outbox) and the tables of the native plugins linked into the
//! process, under each plugin's prefix. Write transactions start with
//! `BEGIN IMMEDIATE` so concurrent writers queue instead of failing on a
//! lock upgrade.

use crate::keys::DatabaseKeys;
use chrono::{DateTime, SecondsFormat, Utc};
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{ConnectOptions, Sqlite, Transaction};
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use uuid::Uuid;

/// One schema change applied to every workspace file. Statements are
/// separated by semicolons, which must not appear inside literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TenantMigration {
    /// `platform.core` or the plugin id that owns the change.
    pub owner: &'static str,
    /// Ordered identifier, unique per owner.
    pub id: &'static str,
    pub sql: &'static str,
}

/// Kernel tables every workspace file carries. Audit and outbox rows are
/// written in the same transaction as the domain change they describe.
pub const KERNEL_TENANT_MIGRATIONS: &[TenantMigration] = &[TenantMigration {
    owner: "platform.core",
    id: "0001_tenant_evidence",
    sql: r#"
CREATE TABLE audit_log (
  id TEXT NOT NULL,
  tenant_id TEXT NOT NULL,
  actor_id TEXT,
  correlation_id TEXT,
  causation_id TEXT,
  action TEXT NOT NULL,
  target TEXT NOT NULL,
  metadata TEXT NOT NULL CHECK (json_valid(metadata)),
  occurred_at TEXT NOT NULL,
  PRIMARY KEY (tenant_id, id)
);
CREATE INDEX audit_log_tenant_time_idx ON audit_log (tenant_id, occurred_at);
CREATE TABLE outbox (
  id TEXT NOT NULL,
  tenant_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  envelope TEXT NOT NULL CHECK (json_valid(envelope)),
  created_at TEXT NOT NULL,
  published_at TEXT,
  PRIMARY KEY (tenant_id, id)
);
CREATE INDEX outbox_unpublished_idx ON outbox (published_at, created_at, id);
"#,
}];

const LEDGER: &str = r#"
CREATE TABLE IF NOT EXISTS migration_ledger (
  owner TEXT NOT NULL,
  id TEXT NOT NULL,
  checksum TEXT NOT NULL,
  applied_at TEXT NOT NULL,
  PRIMARY KEY (owner, id)
)"#;

#[derive(Debug, thiserror::Error)]
pub enum SqliteStoreError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error("migration {owner}/{id} changed after it was applied")]
    MigrationDrift { owner: String, id: String },
    /// The workspace's key could not be had, or does not open its file.
    #[error("the workspace file cannot be opened with its key")]
    Key,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Opens and migrates one database file per workspace, on first use.
pub struct SqliteTenantDatabases {
    directory: PathBuf,
    migrations: Vec<TenantMigration>,
    pools: Mutex<HashMap<Uuid, SqlitePool>>,
    keys: Option<Arc<dyn DatabaseKeys>>,
}

impl SqliteTenantDatabases {
    /// `migrations` are the plugin migrations to apply after the kernel's,
    /// in order.
    pub fn new(directory: impl Into<PathBuf>, migrations: &[TenantMigration]) -> Self {
        let mut all = KERNEL_TENANT_MIGRATIONS.to_vec();
        all.extend_from_slice(migrations);
        Self {
            directory: directory.into(),
            migrations: all,
            pools: Mutex::new(HashMap::new()),
            keys: None,
        }
    }

    /// Encrypts every workspace file with SQLCipher, keyed by `keys`
    /// (ADR 0024, "Encryption at rest"). A file opened without its key, or
    /// with another, fails with [`SqliteStoreError::Key`].
    #[cfg(feature = "encryption")]
    pub fn with_keys(mut self, keys: Arc<dyn DatabaseKeys>) -> Self {
        self.keys = Some(keys);
        self
    }

    /// Deletes a workspace's file, with its write-ahead log, and destroys
    /// its key: what a device does when it signs out or is told to wipe.
    pub async fn wipe(&self, tenant_id: Uuid) -> Result<(), SqliteStoreError> {
        // Held until the key is gone, so no request reopens the file with
        // the old key while the wipe runs.
        let mut pools = self.pools.lock().await;
        if let Some(pool) = pools.remove(&tenant_id) {
            pool.close().await;
        }
        let path = self.path_for(tenant_id);
        for suffix in ["", "-wal", "-shm"] {
            let mut file = path.clone().into_os_string();
            file.push(suffix);
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        if let Some(keys) = &self.keys {
            keys.destroy(tenant_id)
                .await
                .map_err(|_| SqliteStoreError::Key)?;
        }
        drop(pools);
        Ok(())
    }

    /// The path of a workspace's file. Built from the parsed tenant id only,
    /// so no caller-supplied text reaches the file system.
    pub fn path_for(&self, tenant_id: Uuid) -> PathBuf {
        self.directory
            .join(format!("{}.sqlite", tenant_id.as_hyphenated()))
    }

    /// The migrated pool for one workspace.
    pub async fn pool(&self, tenant_id: Uuid) -> Result<SqlitePool, SqliteStoreError> {
        let mut pools = self.pools.lock().await;
        if let Some(pool) = pools.get(&tenant_id) {
            return Ok(pool.clone());
        }
        std::fs::create_dir_all(&self.directory).map_err(sqlx::Error::Io)?;
        let mut options = SqliteConnectOptions::from_str("sqlite://")?
            .filename(self.path_for(tenant_id))
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(5));
        if let Some(keys) = &self.keys {
            let key = keys
                .key(tenant_id)
                .await
                .map_err(|_| SqliteStoreError::Key)?;
            // sqlx sends `key` before every other pragma on each connection,
            // as SQLCipher requires. It runs pragmas through its executor,
            // whose statement and slow-statement logs carry the SQL text, so
            // both are off: the key must never reach a log.
            options = options
                .pragma("key", key.sqlcipher_literal())
                .disable_statement_logging();
        }
        let pool = match SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await
        {
            Ok(pool) => pool,
            // A wrong key fails the first pragma that reads the file.
            Err(_) if self.keys.is_some() => return Err(SqliteStoreError::Key),
            Err(error) => return Err(error.into()),
        };
        if self.keys.is_some() {
            if let Err(error) = sqlx::query("SELECT count(*) FROM sqlite_master")
                .execute(&pool)
                .await
            {
                pool.close().await;
                return match error {
                    sqlx::Error::Database(_) => Err(SqliteStoreError::Key),
                    other => Err(other.into()),
                };
            }
        }
        self.migrate(&pool).await?;
        pools.insert(tenant_id, pool.clone());
        Ok(pool)
    }

    /// A write transaction on a workspace's file.
    pub async fn begin(
        &self,
        tenant_id: Uuid,
    ) -> Result<Transaction<'static, Sqlite>, SqliteStoreError> {
        Ok(self
            .pool(tenant_id)
            .await?
            .begin_with("BEGIN IMMEDIATE")
            .await?)
    }

    async fn migrate(&self, pool: &SqlitePool) -> Result<(), SqliteStoreError> {
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(LEDGER).execute(&mut *tx).await?;
        for migration in &self.migrations {
            let checksum = format!("{:x}", Sha256::digest(migration.sql.as_bytes()));
            let recorded: Option<String> = sqlx::query_scalar(
                "SELECT checksum FROM migration_ledger WHERE owner = ?1 AND id = ?2",
            )
            .bind(migration.owner)
            .bind(migration.id)
            .fetch_optional(&mut *tx)
            .await?;
            match recorded {
                Some(recorded) if recorded == checksum => continue,
                Some(_) => {
                    return Err(SqliteStoreError::MigrationDrift {
                        owner: migration.owner.into(),
                        id: migration.id.into(),
                    })
                }
                None => {}
            }
            // Statements run one at a time; migrations keep semicolons out
            // of literals so splitting on them is exact.
            for statement in migration
                .sql
                .split(';')
                .map(str::trim)
                .filter(|statement| !statement.is_empty())
            {
                sqlx::query(statement).execute(&mut *tx).await?;
            }
            sqlx::query(
                "INSERT INTO migration_ledger (owner, id, checksum, applied_at) VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(migration.owner)
            .bind(migration.id)
            .bind(checksum)
            .bind(timestamp(Utc::now()))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Timestamps are stored as fixed-width UTC text so text order is time order.
pub fn timestamp(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

pub fn parse_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

/// Appends audit evidence in a caller-owned workspace transaction.
pub async fn append_audit(
    tx: &mut Transaction<'_, Sqlite>,
    event: &AuditEvent,
) -> Result<(), sqlx::Error> {
    let tenant_id = event
        .tenant_id
        .ok_or_else(|| sqlx::Error::Protocol("workspace audit evidence needs a tenant".into()))?;
    sqlx::query(
        r#"
        INSERT INTO audit_log (
          id, tenant_id, actor_id, correlation_id, causation_id,
          action, target, metadata, occurred_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        "#,
    )
    .bind(event.id.to_string())
    .bind(tenant_id.to_string())
    .bind(event.actor_id.map(|id| id.to_string()))
    .bind(event.correlation_id.map(|id| id.to_string()))
    .bind(event.causation_id.map(|id| id.to_string()))
    .bind(&event.action)
    .bind(&event.target)
    .bind(event.metadata.to_string())
    .bind(timestamp(event.occurred_at))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Appends an outbox event in a caller-owned workspace transaction.
pub async fn append_outbox(
    tx: &mut Transaction<'_, Sqlite>,
    event: &EventEnvelope,
) -> Result<(), sqlx::Error> {
    event
        .publication_bytes()
        .map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    let envelope =
        serde_json::to_string(event).map_err(|error| sqlx::Error::Encode(Box::new(error)))?;
    sqlx::query(
        "INSERT INTO outbox (id, tenant_id, event_type, envelope, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(event.id.to_string())
    .bind(event.tenant_id.to_string())
    .bind(&event.event_type)
    .bind(envelope)
    .bind(timestamp(event.occurred_at))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDGETS: TenantMigration = TenantMigration {
        owner: "example.widgets",
        id: "0001_widgets",
        sql: "CREATE TABLE p_widgets (tenant_id TEXT NOT NULL, id TEXT NOT NULL, PRIMARY KEY (tenant_id, id));",
    };

    #[tokio::test]
    async fn each_workspace_gets_its_own_migrated_file() {
        let directory = tempfile::tempdir().expect("temp dir");
        let databases = SqliteTenantDatabases::new(directory.path(), &[WIDGETS]);
        let first = Uuid::now_v7();
        let second = Uuid::now_v7();
        let mut tx = databases.begin(first).await.expect("first");
        sqlx::query("INSERT INTO p_widgets (tenant_id, id) VALUES (?1, 'w1')")
            .bind(first.to_string())
            .execute(&mut *tx)
            .await
            .expect("insert");
        tx.commit().await.expect("commit");
        assert!(databases.path_for(first).exists());

        let pool = databases.pool(second).await.expect("second");
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM p_widgets")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(rows, 0, "another workspace's file holds none of its rows");
        let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM migration_ledger")
            .fetch_one(&pool)
            .await
            .expect("ledger");
        assert_eq!(ledger, 2);
    }

    #[tokio::test]
    async fn changed_migrations_are_refused() {
        let directory = tempfile::tempdir().expect("temp dir");
        let tenant = Uuid::now_v7();
        SqliteTenantDatabases::new(directory.path(), &[WIDGETS])
            .pool(tenant)
            .await
            .expect("first run");
        let changed = TenantMigration {
            sql: "CREATE TABLE p_widgets (id TEXT);",
            ..WIDGETS
        };
        let error = SqliteTenantDatabases::new(directory.path(), &[changed])
            .pool(tenant)
            .await
            .expect_err("drift");
        assert!(matches!(error, SqliteStoreError::MigrationDrift { .. }));
    }

    #[tokio::test]
    async fn evidence_is_written_in_the_callers_transaction() {
        let directory = tempfile::tempdir().expect("temp dir");
        let databases = SqliteTenantDatabases::new(directory.path(), &[]);
        let tenant = Uuid::now_v7();
        let mut tx = databases.begin(tenant).await.expect("begin");
        append_audit(
            &mut tx,
            &AuditEvent::new(
                Some(tenant),
                None,
                "test.done",
                "thing:1",
                serde_json::json!({}),
            ),
        )
        .await
        .expect("audit");
        tx.rollback().await.expect("rollback");
        let pool = databases.pool(tenant).await.expect("pool");
        let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log")
            .fetch_one(&pool)
            .await
            .expect("count");
        assert_eq!(rows, 0, "rolled back with the transaction");
    }

    #[test]
    fn timestamps_sort_as_text() {
        let earlier = Utc::now();
        let later = earlier + chrono::Duration::milliseconds(5);
        assert!(timestamp(earlier) < timestamp(later));
        assert_eq!(
            parse_timestamp(&timestamp(earlier)).map(timestamp),
            Some(timestamp(earlier))
        );
    }
}
