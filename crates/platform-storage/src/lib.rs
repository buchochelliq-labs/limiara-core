#![forbid(unsafe_code)]
//! Storage backend selection and capabilities (ADR 0013).
//!
//! Every persistent component talks to a store trait; this crate decides
//! which backend a deployment may use and refuses combinations the platform
//! cannot make safe, before any store is opened.

pub use plugin_contract::StorageBackend;

pub mod keys;
pub mod sqlite;
use serde::Serialize;
use thiserror::Error;

/// Where analytics are kept, separate from the transactional store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AnalyticsBackend {
    None,
    Duckdb,
}

/// How many nodes run the API and worker against the store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentProfile {
    SingleNode,
    MultiNode,
}

/// What a transactional backend guarantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Capabilities {
    /// Tenant rows are filtered by the database itself (row-level security).
    pub row_level_security: bool,
    /// Plugin tables are guarded by database policies, not only code ownership.
    pub plugin_context_isolation: bool,
    /// Tenant data lives in a separate database file or schema per workspace.
    pub database_per_tenant: bool,
    /// Separate processes may write concurrently.
    pub multi_process_writers: bool,
    /// More than one node may share the store.
    pub multi_node: bool,
    /// Queues can lease rows without blocking other claimants.
    pub row_leasing: bool,
}

impl Capabilities {
    pub const fn of(backend: StorageBackend) -> Self {
        match backend {
            StorageBackend::Postgres => Self {
                row_level_security: true,
                plugin_context_isolation: true,
                database_per_tenant: false,
                multi_process_writers: true,
                multi_node: true,
                row_leasing: true,
            },
            StorageBackend::Sqlite => Self {
                row_level_security: false,
                plugin_context_isolation: false,
                database_per_tenant: true,
                multi_process_writers: true,
                multi_node: false,
                row_leasing: false,
            },
        }
    }

    /// Tenant isolation holds either in the database or by physical
    /// separation; a backend with neither is never supported.
    pub const fn isolates_tenants(&self) -> bool {
        self.row_level_security || self.database_per_tenant
    }
}

/// A requested storage configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageSelection {
    pub primary: StorageBackend,
    pub analytics: AnalyticsBackend,
    pub profile: DeploymentProfile,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum StorageSelectionError {
    #[error("storage backend {backend} supports a single node only; set platform.deployment.profile to single-node or use postgres")]
    SingleNodeOnly { backend: &'static str },
    #[error("storage backend {backend} cannot isolate tenants")]
    NoTenantIsolation { backend: &'static str },
    #[error("the {component} store is not available on {backend} yet")]
    StoreUnavailable {
        component: &'static str,
        backend: &'static str,
    },
}

impl StorageSelectionError {
    /// Stable code for logs and readiness responses.
    pub const fn code(&self) -> &'static str {
        match self {
            Self::SingleNodeOnly { .. } => "storage.single_node_only",
            Self::NoTenantIsolation { .. } => "storage.no_tenant_isolation",
            Self::StoreUnavailable { .. } => "storage.store_unavailable",
        }
    }
}

/// Kernel stores and whether each backend has an adapter for them. A
/// service refuses to start until every store it uses is available.
pub const KERNEL_STORES: &[(&str, &[StorageBackend])] = &[
    ("identity and sessions", &[StorageBackend::Postgres]),
    ("workspace access", &[StorageBackend::Postgres]),
    ("plugin registry", &[StorageBackend::Postgres]),
    ("audit and outbox", &[StorageBackend::Postgres]),
    ("inbox", &[StorageBackend::Postgres]),
    ("jobs", &[StorageBackend::Postgres]),
    ("ui publications", &[StorageBackend::Postgres]),
    ("extension layouts", &[StorageBackend::Postgres]),
];

impl StorageSelection {
    /// Checks the selection against backend capabilities, the deployment
    /// profile and the kernel stores implemented for the backend.
    pub fn validate(&self) -> Result<Capabilities, StorageSelectionError> {
        let capabilities = Capabilities::of(self.primary);
        if !capabilities.isolates_tenants() {
            return Err(StorageSelectionError::NoTenantIsolation {
                backend: self.primary.as_str(),
            });
        }
        if self.profile == DeploymentProfile::MultiNode && !capabilities.multi_node {
            return Err(StorageSelectionError::SingleNodeOnly {
                backend: self.primary.as_str(),
            });
        }
        if let Some((component, _)) = KERNEL_STORES
            .iter()
            .find(|(_, backends)| !backends.contains(&self.primary))
        {
            return Err(StorageSelectionError::StoreUnavailable {
                component,
                backend: self.primary.as_str(),
            });
        }
        Ok(capabilities)
    }
}

/// The selection a service's settings ask for.
pub fn selection_from(settings: &platform_config::PlatformSettings) -> StorageSelection {
    use platform_config::{AnalyticsBackendSetting, DatabaseBackend, DeploymentProfileSetting};
    StorageSelection {
        primary: match settings.database.backend {
            DatabaseBackend::Postgres => StorageBackend::Postgres,
            DatabaseBackend::Sqlite => StorageBackend::Sqlite,
        },
        analytics: match settings.analytics.backend {
            AnalyticsBackendSetting::None => AnalyticsBackend::None,
            AnalyticsBackendSetting::Duckdb => AnalyticsBackend::Duckdb,
        },
        profile: match settings.deployment.profile {
            DeploymentProfileSetting::SingleNode => DeploymentProfile::SingleNode,
            DeploymentProfileSetting::MultiNode => DeploymentProfile::MultiNode,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(primary: StorageBackend, profile: DeploymentProfile) -> StorageSelection {
        StorageSelection {
            primary,
            analytics: AnalyticsBackend::None,
            profile,
        }
    }

    #[test]
    fn postgres_is_valid_for_every_profile() {
        for profile in [DeploymentProfile::SingleNode, DeploymentProfile::MultiNode] {
            let capabilities = selection(StorageBackend::Postgres, profile)
                .validate()
                .expect("postgres");
            assert!(capabilities.row_level_security);
        }
    }

    #[test]
    fn sqlite_is_single_node_and_isolates_by_file() {
        let capabilities = Capabilities::of(StorageBackend::Sqlite);
        assert!(capabilities.isolates_tenants());
        assert!(capabilities.database_per_tenant);
        assert!(!capabilities.row_level_security);
        let error = selection(StorageBackend::Sqlite, DeploymentProfile::MultiNode)
            .validate()
            .expect_err("multi-node sqlite");
        assert_eq!(error.code(), "storage.single_node_only");
    }

    #[test]
    fn services_refuse_backends_missing_kernel_stores() {
        let error = selection(StorageBackend::Sqlite, DeploymentProfile::SingleNode)
            .validate()
            .expect_err("kernel stores are postgres-only for now");
        assert_eq!(error.code(), "storage.store_unavailable");
    }
}
