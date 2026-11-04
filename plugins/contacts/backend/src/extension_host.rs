//! Contacts as an extension host for kernel retention work (ADR 0012, #277).
//!
//! The kernel calls this port to export or purge one extending plugin's
//! values. Every statement runs in a Contacts plugin transaction for one
//! tenant, so the existing tenant and plugin-context policies apply.

use crate::events::CONTACTS_PLUGIN_ID;
use crate::{ORGANISATION_RESOURCE, PERSON_RESOURCE};
use async_trait::async_trait;
use platform_db::{begin_plugin_tx, DbError};
use plugin_runtime_native::{
    valid_extension_namespace, ExportedExtensionValues, ExtensionHostError, NativeExtensionHost,
};
use serde_json::Value;
use sqlx::PgPool;
use std::collections::BTreeMap;
use uuid::Uuid;

/// The tables holding extension values, with the resource each represents.
const HOSTED: [(&str, &str); 2] = [
    (PERSON_RESOURCE, "p_contacts.people"),
    (ORGANISATION_RESOURCE, "p_contacts.organisations"),
];

pub struct ContactsExtensionHost {
    pool: PgPool,
}

impl ContactsExtensionHost {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn begin(
        &self,
        tenant_id: Uuid,
    ) -> Result<sqlx::Transaction<'_, sqlx::Postgres>, ExtensionHostError> {
        begin_plugin_tx(&self.pool, tenant_id, CONTACTS_PLUGIN_ID)
            .await
            .map_err(|DbError::Sqlx(error)| storage(error))
    }
}

fn storage(error: sqlx::Error) -> ExtensionHostError {
    ExtensionHostError::Storage(Box::new(error))
}

#[async_trait]
impl NativeExtensionHost for ContactsExtensionHost {
    fn host_plugin_id(&self) -> &str {
        CONTACTS_PLUGIN_ID
    }

    async fn export_extension_namespace(
        &self,
        tenant_id: Uuid,
        namespace: &str,
        limit: usize,
    ) -> Result<(Vec<ExportedExtensionValues>, bool), ExtensionHostError> {
        if !valid_extension_namespace(CONTACTS_PLUGIN_ID, namespace) {
            return Err(ExtensionHostError::InvalidNamespace);
        }
        let mut tx = self.begin(tenant_id).await?;
        let mut exported = Vec::new();
        let mut truncated = false;
        for (resource, table) in HOSTED {
            let remaining = limit.saturating_sub(exported.len());
            let fetch = i64::try_from(remaining.saturating_add(1)).unwrap_or(i64::MAX);
            let rows: Vec<(String, Value)> = sqlx::query_as(&format!(
                "SELECT public_id, extensions -> $2 FROM {table} \
                 WHERE tenant_id = $1 AND extensions ? $2 ORDER BY public_id LIMIT $3"
            ))
            .bind(tenant_id)
            .bind(namespace)
            .bind(fetch)
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            if rows.len() > remaining {
                truncated = true;
            }
            for (record_id, values) in rows.into_iter().take(remaining) {
                let values = serde_json::from_value::<BTreeMap<String, Value>>(values)
                    .map_err(|error| ExtensionHostError::Storage(Box::new(error)))?;
                exported.push(ExportedExtensionValues {
                    resource: resource.to_owned(),
                    record_id,
                    values,
                });
            }
            if truncated {
                break;
            }
        }
        tx.commit().await.map_err(storage)?;
        Ok((exported, truncated))
    }

    async fn purge_extension_namespace(
        &self,
        tenant_id: Uuid,
        namespace: &str,
    ) -> Result<u64, ExtensionHostError> {
        if !valid_extension_namespace(CONTACTS_PLUGIN_ID, namespace) {
            return Err(ExtensionHostError::InvalidNamespace);
        }
        let mut tx = self.begin(tenant_id).await?;
        let mut purged = 0;
        for (_, table) in HOSTED {
            // Values were already hidden from every caller; removing them is
            // not a record edit, so versions and record events are unchanged.
            // It is still a change to the stored record: only the standard
            // `touched_at` stamp moves (ADR 0014).
            purged += sqlx::query(&format!(
                "UPDATE {table} SET extensions = extensions - $2, touched_at = now() \
                 WHERE tenant_id = $1 AND extensions ? $2"
            ))
            .bind(tenant_id)
            .bind(namespace)
            .execute(&mut *tx)
            .await
            .map_err(storage)?
            .rows_affected();
        }
        tx.commit().await.map_err(storage)?;
        Ok(purged)
    }
}
