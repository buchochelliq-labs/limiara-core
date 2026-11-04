//! Purges extension values of plugins removed longer than the retention
//! window (ADR 0012, #277).
//!
//! Discovery is the only cross-tenant step and uses the bounded
//! `platform_due_extension_purges` routine. Each purge then runs per tenant
//! through the host's own port, and the evidence is recorded in the tenant's
//! transaction. A purge whose host is not linked in this worker is retried on
//! a later pass rather than being marked done.

use plugin_contract::Manifest;
use plugin_runtime_native::NativeExtensionHost;
use sqlx::PgPool;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::Duration,
};

/// Removed installations examined per pass.
pub const PURGE_BATCH: i32 = 50;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PurgePass {
    pub purged: usize,
    pub deferred: usize,
}

/// Runs one purge pass and reports how many installations were purged and
/// how many must wait for a linked host or a healthy store.
pub async fn purge_due_extensions(
    worker_pool: &PgPool,
    tenant_pool: &PgPool,
    hosts: &[Arc<dyn NativeExtensionHost>],
    retention_days: i32,
) -> Result<PurgePass, sqlx::Error> {
    let due = platform_db::due_extension_purges(worker_pool, retention_days, PURGE_BATCH).await?;
    let hosts = hosts
        .iter()
        .map(|host| (host.host_plugin_id().to_owned(), host.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut pass = PurgePass::default();
    for item in due {
        let Ok(manifest) = serde_json::from_value::<Manifest>(item.manifest) else {
            tracing::warn!(plugin_id = %item.plugin_id, "removed plugin manifest is unreadable; purge deferred");
            pass.deferred += 1;
            continue;
        };
        let host_ids = manifest
            .extensions
            .fields
            .iter()
            .map(|field| field.host.clone())
            .collect::<BTreeSet<_>>();
        let mut counts = BTreeMap::new();
        let mut complete = true;
        for host_id in host_ids {
            let Some(host) = hosts.get(&host_id) else {
                tracing::warn!(plugin_id = %item.plugin_id, host = %host_id, "extension host is not linked; purge deferred");
                complete = false;
                continue;
            };
            match host
                .purge_extension_namespace(item.tenant_id, &item.plugin_id)
                .await
            {
                Ok(records) => {
                    counts.insert(host_id, records);
                }
                Err(error) => {
                    tracing::warn!(plugin_id = %item.plugin_id, host = %host_id, error = %error, "extension purge failed; retrying later");
                    complete = false;
                }
            }
        }
        if !complete {
            pass.deferred += 1;
            continue;
        }
        if platform_db::mark_extensions_purged(
            tenant_pool,
            item.tenant_id,
            &item.plugin_id,
            &counts,
        )
        .await?
        {
            pass.purged += 1;
        }
    }
    Ok(pass)
}

/// Runs purge passes for the life of the process.
pub async fn run(
    worker_pool: PgPool,
    tenant_pool: PgPool,
    hosts: Vec<Arc<dyn NativeExtensionHost>>,
    interval: Duration,
) {
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        match purge_due_extensions(
            &worker_pool,
            &tenant_pool,
            &hosts,
            platform_db::EXTENSION_RETENTION_DAYS,
        )
        .await
        {
            Ok(pass) if pass.purged > 0 || pass.deferred > 0 => {
                tracing::info!(
                    purged = pass.purged,
                    deferred = pass.deferred,
                    "extension retention pass"
                );
            }
            Ok(_) => {}
            Err(error) => tracing::warn!(error = %error, "extension retention pass failed"),
        }
    }
}
