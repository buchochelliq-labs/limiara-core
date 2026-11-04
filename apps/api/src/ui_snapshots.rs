//! Durable workspace snapshots for permission-filtered UI metadata delivery.
//!
//! The delivery service resolves pages synchronously from a
//! [`UiSnapshotSource`]. The inputs live in the plugin registry and in
//! PostgreSQL, so each request first [primes](UiSnapshotProvider::prime) the
//! workspace's snapshot asynchronously and then delivers from it:
//!
//! - the catalogue holds the linked native UI packages of plugins active in
//!   the workspace whose version and declared documents match the active,
//!   digest-verified manifest; anything else is left out and fails closed;
//! - the tenant patches are the workspace's published page customisations.

use crate::plugins::PluginActivationProjection;
use plugin_contract::{ui::ResourceDescriptor, Manifest};
use plugin_runtime_native::{verify_ui_plugin, NativeUiPlugin, UiPackageError};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex, PoisonError};
use ui_metadata::{CatalogueError, MetadataCatalogue, PluginUiPackage, PluginUiState};
use ui_metadata_delivery::{SnapshotError, UiSnapshotSource, UiWorkspaceSnapshot};
use uuid::Uuid;

/// Components the internal web host renders. This mirrors the default host
/// registry in `apps/web/src/plugin-ui/renderer/hostComponents.tsx`; a unit
/// test fails when the two drift.
pub(crate) const HOST_COMPONENTS: &[&str] = &[
    "platform.ui.accordion",
    "platform.ui.action",
    "platform.ui.action-bar",
    "platform.ui.alert",
    "platform.ui.card",
    "platform.ui.collection",
    "platform.ui.collection-controls",
    "platform.ui.cursor",
    "platform.ui.denied",
    "platform.ui.dialog",
    "platform.ui.empty",
    "platform.ui.error",
    "platform.ui.field.select",
    "platform.ui.field.text",
    "platform.ui.filter-chips",
    "platform.ui.form",
    "platform.ui.fragment",
    "platform.ui.grid",
    "platform.ui.highlights",
    "platform.ui.loading",
    "platform.ui.page",
    "platform.ui.path",
    "platform.ui.query-input",
    "platform.ui.query-select",
    "platform.ui.record-preview",
    "platform.ui.record-summary",
    "platform.ui.relationship-list",
    "platform.ui.section",
    "platform.ui.slot",
    "platform.ui.split",
    "platform.ui.stack",
    "platform.ui.state",
    "platform.ui.table",
    "platform.ui.tabs",
    "platform.ui.text",
    "platform.ui.timeline",
    "platform.ui.wizard",
];

/// Upper bound on cached catalogues (one per distinct set of active UI
/// packages); the cache is cleared rather than grown past it.
const CATALOGUE_CACHE_LIMIT: usize = 64;

/// The UI documents a manifest declares, by kind, for exact matching.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeclaredUi([BTreeSet<String>; 5]);

impl DeclaredUi {
    fn of(manifest: &Manifest) -> Self {
        let ids = |references: &[plugin_contract::UiFileReference]| {
            references
                .iter()
                .map(|reference| reference.id.clone())
                .collect()
        };
        let ui = &manifest.ui;
        Self([
            ids(&ui.resources),
            ids(&ui.pages),
            ids(&ui.fragments),
            ids(&ui.extensions),
            ids(&ui.components),
        ])
    }
}

#[derive(Debug, Clone)]
struct LinkedUiPackage {
    version: String,
    declared: DeclaredUi,
    package: PluginUiPackage,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum LinkError {
    #[error(transparent)]
    Package(#[from] UiPackageError),
    #[error("plugin {0} links its UI more than once")]
    Duplicate(String),
    #[error("linked UI of plugin {plugin_id} does not fit the host catalogue: {source}")]
    Catalogue {
        plugin_id: String,
        source: CatalogueError,
    },
}

pub(crate) struct UiSnapshotProvider {
    linked: BTreeMap<String, LinkedUiPackage>,
    catalogues: Mutex<BTreeMap<String, Arc<MetadataCatalogue>>>,
    primed: Mutex<BTreeMap<String, UiWorkspaceSnapshot>>,
}

impl UiSnapshotProvider {
    /// Verify each linked plugin's UI against its own manifest and the host
    /// component registry. A failure here is a build defect, so the API
    /// refuses to start rather than serve a partial catalogue.
    pub(crate) fn from_plugins(plugins: &[Arc<dyn NativeUiPlugin>]) -> Result<Self, LinkError> {
        let mut linked = BTreeMap::new();
        for plugin in plugins {
            let manifest = plugin.manifest();
            let package = PluginUiPackage {
                plugin_id: manifest.id.clone(),
                plugin_version: manifest.version.to_string(),
                dependencies: BTreeSet::new(),
                state: PluginUiState::Enabled,
                bundle: verify_ui_plugin(plugin.as_ref())?,
            };
            host_catalogue()
                .install(package.clone())
                .map_err(|source| LinkError::Catalogue {
                    plugin_id: manifest.id.clone(),
                    source,
                })?;
            let entry = LinkedUiPackage {
                version: manifest.version.to_string(),
                declared: DeclaredUi::of(manifest),
                package,
            };
            if linked.insert(manifest.id.clone(), entry).is_some() {
                return Err(LinkError::Duplicate(manifest.id.clone()));
            }
        }
        Ok(Self {
            linked,
            catalogues: Mutex::new(BTreeMap::new()),
            primed: Mutex::new(BTreeMap::new()),
        })
    }

    /// Build the workspace snapshot from its activation and published
    /// customisations, and make it the one delivery reads next.
    pub(crate) async fn prime(
        &self,
        pool: &sqlx::PgPool,
        tenant_id: Uuid,
        activation: &PluginActivationProjection,
    ) -> Result<(), SnapshotError> {
        let pages = platform_db::list_ui_metadata_page_states(pool, tenant_id)
            .await
            .map_err(|error| {
                tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to load published UI customisations");
                SnapshotError::TemporarilyUnavailable
            })?;
        let mut publication = Sha256::new();
        let mut tenant_patches = Vec::new();
        for page in &pages {
            publication.update(page.page_id.as_bytes());
            publication.update([0]);
            publication.update(page.publication_generation.to_le_bytes());
            tenant_patches.extend_from_slice(page.published_patches());
        }
        let (catalogue, catalogue_generation) = self.catalogue_for(activation)?;
        let resources = resources_of(&catalogue_packages(&self.linked, activation));
        let snapshot = UiWorkspaceSnapshot {
            catalogue,
            resources,
            tenant_patches,
            catalogue_generation,
            publication_generation: generation(publication),
        };
        self.primed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(tenant_id.to_string(), snapshot);
        Ok(())
    }

    fn catalogue_for(
        &self,
        activation: &PluginActivationProjection,
    ) -> Result<(Arc<MetadataCatalogue>, u64), SnapshotError> {
        let packages = catalogue_packages(&self.linked, activation);
        let mut key = Sha256::new();
        for package in &packages {
            key.update(package.plugin_id.as_bytes());
            key.update([0]);
            key.update(package.plugin_version.as_bytes());
            key.update([0]);
        }
        let key = format!("{:x}", key.finalize());
        let catalogue_generation = u64::from_str_radix(&key[..16], 16).unwrap_or_default();
        let mut cache = self
            .catalogues
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(catalogue) = cache.get(&key) {
            return Ok((catalogue.clone(), catalogue_generation));
        }
        let mut catalogue = host_catalogue();
        for package in packages {
            // Linked packages were verified to install cleanly on their own;
            // a combination that conflicts is withheld rather than partial.
            if let Err(error) = catalogue.install(package.clone()) {
                tracing::error!(plugin_id = %package.plugin_id, error = %error, "active UI packages conflict");
                return Err(SnapshotError::Conflict);
            }
        }
        let catalogue = Arc::new(catalogue);
        if cache.len() >= CATALOGUE_CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key, catalogue.clone());
        Ok((catalogue, catalogue_generation))
    }
}

impl UiSnapshotSource for UiSnapshotProvider {
    fn snapshot(&self, workspace_id: &str) -> Result<UiWorkspaceSnapshot, SnapshotError> {
        self.primed
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(workspace_id)
            .cloned()
            .ok_or(SnapshotError::TemporarilyUnavailable)
    }

    fn is_ready(&self) -> bool {
        true
    }
}

fn host_catalogue() -> MetadataCatalogue {
    let mut catalogue = MetadataCatalogue::default();
    for component in HOST_COMPONENTS {
        catalogue.register_host_component(*component);
    }
    catalogue
}

/// Linked packages that may serve this workspace: the plugin is active and its
/// linked version and declared documents match the active manifest exactly.
fn catalogue_packages<'a>(
    linked: &'a BTreeMap<String, LinkedUiPackage>,
    activation: &PluginActivationProjection,
) -> Vec<&'a PluginUiPackage> {
    activation
        .active_manifests()
        .filter_map(|manifest| {
            let entry = linked.get(&manifest.id)?;
            if entry.version != manifest.version.to_string()
                || entry.declared != DeclaredUi::of(manifest)
            {
                tracing::warn!(plugin_id = %manifest.id, "active plugin version has no matching linked UI; its pages stay unavailable");
                return None;
            }
            Some(&entry.package)
        })
        .collect()
}

fn resources_of(packages: &[&PluginUiPackage]) -> Vec<ResourceDescriptor> {
    packages
        .iter()
        .flat_map(|package| package.bundle.resources.iter().cloned())
        .collect()
}

fn generation(hasher: Sha256) -> u64 {
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    u64::from_le_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contacts() -> Arc<dyn NativeUiPlugin> {
        Arc::new(contacts_plugin::ui::ContactsUiPlugin::new().expect("Contacts manifest parses"))
    }

    #[test]
    fn host_components_mirror_the_web_registry() {
        let registry = include_str!("../../web/src/plugin-ui/renderer/hostComponents.tsx");
        let web = registry
            .split("define({id: '")
            .skip(1)
            .filter_map(|rest| rest.split('\'').next())
            .collect::<BTreeSet<_>>();
        let host = HOST_COMPONENTS.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(
            host, web,
            "update HOST_COMPONENTS to match hostComponents.tsx"
        );
    }

    #[test]
    fn linked_contacts_ui_installs_on_the_host_catalogue() {
        let provider = UiSnapshotProvider::from_plugins(&[contacts()]).expect("Contacts UI links");
        assert!(provider.linked.contains_key("platform.contacts"));
    }

    #[test]
    fn a_plugin_linked_twice_is_rejected() {
        assert!(matches!(
            UiSnapshotProvider::from_plugins(&[contacts(), contacts()]),
            Err(LinkError::Duplicate(_))
        ));
    }

    #[test]
    fn only_active_matching_versions_enter_the_catalogue() {
        let provider = UiSnapshotProvider::from_plugins(&[contacts()]).expect("Contacts UI links");
        let enabled = crate::plugins::contacts_test_projection(true, &[]);
        let disabled = crate::plugins::contacts_test_projection(false, &[]);
        assert_eq!(catalogue_packages(&provider.linked, &enabled).len(), 1);
        assert!(catalogue_packages(&provider.linked, &disabled).is_empty());

        let mut skewed = provider.linked.clone();
        if let Some(entry) = skewed.get_mut("platform.contacts") {
            entry.version = "9.9.9".into();
        }
        assert!(catalogue_packages(&skewed, &enabled).is_empty());

        let mut undeclared = provider.linked.clone();
        if let Some(entry) = undeclared.get_mut("platform.contacts") {
            entry.declared.0[1].insert("platform.contacts.extra-page".into());
        }
        assert!(catalogue_packages(&undeclared, &enabled).is_empty());
    }

    #[test]
    fn enabled_and_disabled_catalogues_differ_and_are_cached() {
        let provider = UiSnapshotProvider::from_plugins(&[contacts()]).expect("Contacts UI links");
        let enabled = crate::plugins::contacts_test_projection(true, &[]);
        let disabled = crate::plugins::contacts_test_projection(false, &[]);
        let (first, enabled_generation) = provider.catalogue_for(&enabled).expect("catalogue");
        let (again, _) = provider.catalogue_for(&enabled).expect("catalogue");
        assert!(Arc::ptr_eq(&first, &again));
        let (_, disabled_generation) = provider.catalogue_for(&disabled).expect("catalogue");
        assert_ne!(enabled_generation, disabled_generation);
    }

    #[test]
    fn an_unprimed_workspace_fails_closed() {
        let provider = UiSnapshotProvider::from_plugins(&[contacts()]).expect("Contacts UI links");
        assert_eq!(
            provider
                .snapshot("018f0000-0000-7000-8000-000000000001")
                .err(),
            Some(SnapshotError::TemporarilyUnavailable)
        );
    }
}
