//! The port a record host implements so the kernel can export and purge the
//! values other plugins stored with its records (ADR 0012, #277).
//!
//! Hosts own their storage. The kernel never reads or writes a host's tables;
//! it asks the host, through this port, to act on one tenant and one
//! extending plugin's namespace, inside the host's own execution context.

use async_trait::async_trait;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;
use uuid::Uuid;

/// One record's values for one extending plugin, keyed by field key.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExportedExtensionValues {
    pub resource: String,
    pub record_id: String,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Error)]
pub enum ExtensionHostError {
    #[error("namespace is not an extending plugin id")]
    InvalidNamespace,
    #[error("extension host storage is unavailable")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),
}

#[async_trait]
pub trait NativeExtensionHost: Send + Sync {
    /// The plugin id of the host, matching extension declarations' `host`.
    fn host_plugin_id(&self) -> &str;

    /// Every record in the tenant holding values from `namespace`, up to
    /// `limit` records; the flag reports whether more exist.
    async fn export_extension_namespace(
        &self,
        tenant_id: Uuid,
        namespace: &str,
        limit: usize,
    ) -> Result<(Vec<ExportedExtensionValues>, bool), ExtensionHostError>;

    /// Removes every value `namespace` stored in the tenant and returns how
    /// many records changed. Must be idempotent.
    async fn purge_extension_namespace(
        &self,
        tenant_id: Uuid,
        namespace: &str,
    ) -> Result<u64, ExtensionHostError>;
}

/// Whether a namespace is a plausible extending plugin id other than the
/// host's own. Hosts call this before touching storage.
pub fn valid_extension_namespace(host_plugin_id: &str, namespace: &str) -> bool {
    namespace != host_plugin_id
        && namespace.len() <= 100
        && namespace.split('.').count() >= 2
        && namespace.split('.').all(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::valid_extension_namespace;

    #[test]
    fn namespaces_are_other_plugins_ids() {
        assert!(valid_extension_namespace(
            "platform.contacts",
            "example.loyalty"
        ));
        assert!(!valid_extension_namespace(
            "platform.contacts",
            "platform.contacts"
        ));
        for invalid in [
            "",
            "loyalty",
            "example..loyalty",
            "Example.loyalty",
            "example.loyalty'",
        ] {
            assert!(
                !valid_extension_namespace("platform.contacts", invalid),
                "{invalid}"
            );
        }
    }
}
