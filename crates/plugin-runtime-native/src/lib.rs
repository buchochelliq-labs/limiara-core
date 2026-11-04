#![forbid(unsafe_code)]
use async_trait::async_trait;
use platform_inbox::{ConsumerId, EventHandler};
use platform_jobs::ScheduledJob;
use plugin_contract::Manifest;
use std::{collections::BTreeSet, sync::Arc};
use thiserror::Error;

mod extension_host;
mod ui;
pub use extension_host::{
    valid_extension_namespace, ExportedExtensionValues, ExtensionHostError, NativeExtensionHost,
};
pub use ui::{
    verify_ui_plugin, NativeUiDocument, NativeUiDocuments, NativeUiPlugin, UiPackageError,
};

#[async_trait]
pub trait NativePlugin: Send + Sync {
    fn manifest(&self) -> &Manifest;
    async fn health(&self) -> bool;
}

/// A linked event consumer implementation for a manifest-declared consumer.
#[derive(Clone)]
pub struct NativeConsumer {
    pub id: ConsumerId,
    pub handler: Arc<dyn EventHandler>,
}

/// A linked job implementation for a manifest-declared job.
#[derive(Clone)]
pub struct NativeJob {
    pub id: String,
    pub job: Arc<dyn ScheduledJob>,
}

/// Background contributions a trusted native plugin links into the worker.
///
/// The manifest remains the source of truth: every implementation must match
/// a declaration, and the worker reads event filters and intervals from the
/// manifest rather than from code.
pub trait NativeWorkerPlugin: Send + Sync {
    fn manifest(&self) -> &Manifest;
    fn consumers(&self) -> Vec<NativeConsumer>;
    fn jobs(&self) -> Vec<NativeJob>;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ContributionError {
    #[error("plugin {plugin_id} does not declare worker contribution {id}")]
    Undeclared { plugin_id: String, id: String },
    #[error("plugin {plugin_id} links worker contribution {id} more than once")]
    Duplicate { plugin_id: String, id: String },
    #[error("plugin {plugin_id} links no implementation for declared contribution {id}")]
    Unimplemented { plugin_id: String, id: String },
}

/// Verify that a plugin's linked implementations exactly match its manifest
/// declarations, so undeclared background work cannot be smuggled in by code.
pub fn verify_worker_plugin(plugin: &dyn NativeWorkerPlugin) -> Result<(), ContributionError> {
    let manifest = plugin.manifest();
    let declared_consumers = manifest
        .worker
        .consumers
        .iter()
        .map(|consumer| consumer.id.as_str())
        .collect::<BTreeSet<_>>();
    let declared_jobs = manifest
        .worker
        .jobs
        .iter()
        .map(|job| job.id.as_str())
        .collect::<BTreeSet<_>>();
    let consumers = plugin.consumers();
    let jobs = plugin.jobs();
    let linked = consumers
        .iter()
        .map(|consumer| (consumer.id.as_str(), &declared_consumers))
        .chain(jobs.iter().map(|job| (job.id.as_str(), &declared_jobs)));
    let mut seen = BTreeSet::new();
    for (id, declared) in linked {
        if !declared.contains(id) {
            return Err(ContributionError::Undeclared {
                plugin_id: manifest.id.clone(),
                id: id.to_owned(),
            });
        }
        if !seen.insert(id.to_owned()) {
            return Err(ContributionError::Duplicate {
                plugin_id: manifest.id.clone(),
                id: id.to_owned(),
            });
        }
    }
    if let Some(id) = declared_consumers
        .iter()
        .chain(declared_jobs.iter())
        .find(|id| !seen.contains(**id))
    {
        return Err(ContributionError::Unimplemented {
            plugin_id: manifest.id.clone(),
            id: (*id).to_owned(),
        });
    }
    Ok(())
}
