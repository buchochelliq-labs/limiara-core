#![forbid(unsafe_code)]

pub mod automation_jobs;
pub mod bulk_jobs;
pub mod device_notifications;
pub mod extension_retention;
pub mod outbox_publisher;
pub mod plugin_runtime;

use plugin_runtime_native::{NativeExtensionHost, NativeWorkerPlugin};
use std::sync::Arc;

/// Trusted native plugins linked into this worker binary. Each must declare
/// every consumer and job it implements in its manifest; a manifest that does
/// not parse stops startup rather than silently dropping background work.
pub fn linked_worker_plugins(
) -> Result<Vec<Arc<dyn NativeWorkerPlugin>>, plugin_contract::ManifestError> {
    Ok(vec![Arc::new(
        contacts_plugin::worker::ContactsWorkerPlugin::new()?,
    )])
}

/// Record hosts linked into this worker that can purge extension values of
/// removed plugins. Each runs its own statements in its execution context.
pub fn linked_extension_hosts(tenant_pool: &sqlx::PgPool) -> Vec<Arc<dyn NativeExtensionHost>> {
    vec![Arc::new(
        contacts_plugin::extension_host::ContactsExtensionHost::new(tenant_pool.clone()),
    )]
}
