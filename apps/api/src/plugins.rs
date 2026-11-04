use crate::{problem, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use platform_db::{PluginCandidateRecord, PluginDesiredState, PluginLifecycleError};
use plugin_registry::{activation::ActivationInputs, DiscoveryFailure, RegistryPolicy};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;
use tower_http::request_id::RequestId;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct ActivePlugin {
    manifest: plugin_contract::Manifest,
    effective_permissions: BTreeSet<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct PluginActivationProjection {
    revision: String,
    active: BTreeMap<String, ActivePlugin>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct NavigationContribution {
    id: String,
    label: String,
    route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    icon: Option<String>,
    order: i32,
}

impl PluginActivationProjection {
    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    /// Manifests of the plugins active in this workspace, by plugin id.
    pub(crate) fn active_manifests(&self) -> impl Iterator<Item = &plugin_contract::Manifest> {
        self.active.values().map(|plugin| &plugin.manifest)
    }

    pub(crate) fn is_active(&self, plugin_id: &str) -> bool {
        self.active.contains_key(plugin_id)
    }

    pub(crate) fn permissions_for(&self, plugin_id: &str) -> Option<&BTreeSet<String>> {
        self.active
            .get(plugin_id)
            .map(|plugin| &plugin.effective_permissions)
    }

    pub(crate) fn owns_page(&self, page_id: &str) -> bool {
        self.active.values().any(|plugin| {
            plugin
                .manifest
                .ui
                .pages
                .iter()
                .any(|page| page.id == page_id)
        })
    }

    pub(crate) fn owns_resource(&self, resource_id: &str) -> bool {
        self.active.values().any(|plugin| {
            plugin
                .manifest
                .ui
                .resources
                .iter()
                .any(|resource| resource.id == resource_id)
        })
    }

    pub(crate) fn navigation(&self) -> Vec<NavigationContribution> {
        let mut navigation = self
            .active
            .values()
            .flat_map(|plugin| {
                plugin
                    .manifest
                    .ui
                    .navigation
                    .iter()
                    .filter(|item| {
                        item.permission.as_ref().is_none_or(|permission| {
                            plugin.effective_permissions.contains(permission)
                        }) && safe_navigation_route(&item.route)
                    })
                    .map(|item| NavigationContribution {
                        id: item.id.clone(),
                        label: item.label.clone(),
                        route: item.route.clone(),
                        icon: item.icon.clone(),
                        order: item.order,
                    })
            })
            .collect::<Vec<_>>();
        navigation.sort_by(|left, right| {
            left.order
                .cmp(&right.order)
                .then_with(|| left.id.cmp(&right.id))
        });
        navigation
    }

    pub(crate) fn permissions(&self) -> Vec<String> {
        self.active
            .values()
            .flat_map(|plugin| plugin.effective_permissions.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }
}

pub(crate) async fn activation_projection(
    state: &AppState,
    workspace: &platform_db::WorkspaceAccess,
) -> Result<PluginActivationProjection, PluginLifecycleError> {
    let Some(pool) = state.pool.as_ref() else {
        return Err(PluginLifecycleError::Database(sqlx::Error::PoolClosed));
    };
    let records =
        platform_db::list_tenant_plugin_activation_records(pool, workspace.tenant_id).await?;
    Ok(build_activation_projection(records, &workspace.permissions))
}

pub(crate) fn build_activation_projection(
    mut records: Vec<platform_db::TenantPluginActivationRecord>,
    caller_permissions: &BTreeSet<String>,
) -> PluginActivationProjection {
    records.sort_by(|left, right| left.plugin_id.cmp(&right.plugin_id));
    let mut revision_hasher = Sha256::new();
    for record in &records {
        hash_revision_part(&mut revision_hasher, Some(&record.plugin_id));
        hash_revision_part(&mut revision_hasher, record.installation_state.as_deref());
        hash_revision_part(&mut revision_hasher, record.active_version.as_deref());
        hash_revision_part(&mut revision_hasher, record.lifecycle_state.as_deref());
        hash_revision_part(&mut revision_hasher, record.verified_digest.as_deref());
        revision_hasher.update(record.revision.timestamp_micros().to_le_bytes());
    }
    for permission in caller_permissions {
        hash_revision_part(&mut revision_hasher, Some(permission));
    }
    let revision = format!("{:x}", revision_hasher.finalize());
    let mut active = BTreeMap::new();
    for record in records {
        let Some(manifest) =
            plugin_registry::activation::resolve_active_manifest(ActivationInputs {
                plugin_id: &record.plugin_id,
                installation_state: record.installation_state.as_deref(),
                active_version: record.active_version.as_deref(),
                manifest: record.manifest.as_ref(),
                lifecycle_state: record.lifecycle_state.as_deref(),
                verified_digest: record.verified_digest.as_deref(),
            })
        else {
            continue;
        };
        let declared_permissions = manifest
            .permissions
            .iter()
            .map(|permission| permission.id.as_str())
            .collect::<BTreeSet<_>>();
        let effective_permissions = caller_permissions
            .iter()
            .filter(|permission| declared_permissions.contains(permission.as_str()))
            .cloned()
            .collect();
        active.insert(
            record.plugin_id,
            ActivePlugin {
                manifest,
                effective_permissions,
            },
        );
    }
    PluginActivationProjection { revision, active }
}

fn hash_revision_part(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update(value.len().to_le_bytes());
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0]),
    }
}

fn safe_navigation_route(route: &str) -> bool {
    route.starts_with('/')
        && !route.starts_with("//")
        && !route.contains("..")
        && !route.contains('\\')
        && !route.chars().any(char::is_control)
}

pub(crate) fn spawn_initialiser(
    pool: sqlx::PgPool,
    settings: platform_config::PluginSettings,
    storage_backend: plugin_contract::StorageBackend,
) {
    tokio::spawn(async move {
        loop {
            match initialise(&pool, &settings, storage_backend).await {
                Ok(()) => {
                    tracing::info!("plugin registry initialised");
                    return;
                }
                Err(error) => {
                    tracing::warn!(
                        error_type = %std::any::type_name_of_val(&error),
                        "plugin registry unavailable; retrying initialisation"
                    );
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        }
    });
}

pub(crate) async fn initialise(
    pool: &sqlx::PgPool,
    settings: &platform_config::PluginSettings,
    storage_backend: plugin_contract::StorageBackend,
) -> anyhow::Result<()> {
    let policy = RegistryPolicy {
        reserved_namespaces: settings.reserved_namespaces.clone(),
        trusted_core_repositories: settings.trusted_core_repositories.clone(),
        trusted_provenance_hosts: settings.trusted_provenance_hosts.clone(),
        storage_backend,
    };
    let paths = settings
        .manifest_paths
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let report = plugin_registry::discover(&paths, &policy);
    for failure in &report.failures {
        record_failure(pool, failure).await?;
        tracing::warn!(
            plugin_id = failure.plugin_id.as_deref().unwrap_or("unknown"),
            control = failure.control,
            detail = failure.detail,
            "plugin candidate rejected"
        );
    }
    for plugin in report.accepted {
        platform_db::register_plugin_candidate(
            pool,
            &PluginCandidateRecord {
                plugin_id: plugin.manifest.id.clone(),
                version: plugin.manifest.version.to_string(),
                manifest: serde_json::to_value(&plugin.manifest)?,
                manifest_path: plugin.manifest_path.to_string_lossy().into_owned(),
                artifact_path: Some(plugin.artifact_path.to_string_lossy().into_owned()),
                declared_digest: plugin.manifest.supply_chain.artifact_digest.clone(),
                verified_digest: Some(plugin.verified_digest),
                lifecycle_state: "health_checked".into(),
                failure_control: None,
                failure_detail: None,
            },
        )
        .await?;
    }
    Ok(())
}

async fn record_failure(
    pool: &sqlx::PgPool,
    failure: &DiscoveryFailure,
) -> Result<(), PluginLifecycleError> {
    platform_db::record_plugin_discovery_failure(
        pool,
        failure.plugin_id.as_deref(),
        failure.version.as_deref(),
        &failure.manifest_path.to_string_lossy(),
        failure.control,
    )
    .await
}

pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<crate::auth::WorkspacePrincipal>,
    Path(workspace_id): Path<Uuid>,
) -> impl IntoResponse {
    debug_assert_eq!(principal.workspace.tenant_id, workspace_id);
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::list_tenant_plugins(pool, workspace_id).await {
        Ok(data) => (StatusCode::OK, Json(serde_json::json!({"data": data}))).into_response(),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "plugin catalogue query failed");
            unavailable()
        }
    }
}

pub(crate) async fn enable(
    State(state): State<AppState>,
    Extension(principal): Extension<crate::auth::WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Path((workspace_id, plugin_id)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    transition(
        &state,
        &principal,
        workspace_id,
        &plugin_id,
        PluginDesiredState::Enabled,
        crate::correlation_id(&request_id),
    )
    .await
}

pub(crate) async fn disable(
    State(state): State<AppState>,
    Extension(principal): Extension<crate::auth::WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Path((workspace_id, plugin_id)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    transition(
        &state,
        &principal,
        workspace_id,
        &plugin_id,
        PluginDesiredState::Disabled,
        crate::correlation_id(&request_id),
    )
    .await
}

/// `POST .../plugins/{plugin}/remove`: makes a disabled plugin inactive and
/// starts the extension retention window. Reinstalling within the window
/// keeps the values; afterwards the worker purges them.
pub(crate) async fn remove(
    State(state): State<AppState>,
    Extension(principal): Extension<crate::auth::WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Path((workspace_id, plugin_id)): Path<(Uuid, String)>,
) -> impl IntoResponse {
    transition(
        &state,
        &principal,
        workspace_id,
        &plugin_id,
        PluginDesiredState::Removed,
        crate::correlation_id(&request_id),
    )
    .await
}

async fn transition(
    state: &AppState,
    principal: &crate::auth::WorkspacePrincipal,
    workspace_id: Uuid,
    plugin_id: &str,
    desired: PluginDesiredState,
    correlation_id: Uuid,
) -> axum::response::Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::transition_tenant_plugin(
        pool,
        workspace_id,
        principal.session.user.id,
        plugin_id,
        desired,
        Some(correlation_id),
    )
    .await
    {
        Ok(plugin) => {
            state
                .ui_delivery
                .invalidate_workspace(&workspace_id.to_string());
            let activation_revision = match activation_projection(state, &principal.workspace).await
            {
                Ok(projection) => Some(projection.revision().to_owned()),
                Err(error) => {
                    tracing::error!(error_type = %std::any::type_name_of_val(&error), "plugin transitioned but activation revision could not be reloaded");
                    None
                }
            };
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "id": plugin.id,
                    "name": plugin.name,
                    "version": plugin.version,
                    "state": plugin.state,
                    "verified_digest": plugin.verified_digest,
                    "activation_revision": activation_revision,
                })),
            )
                .into_response()
        }
        Err(PluginLifecycleError::NotFound) => problem(
            StatusCode::NOT_FOUND,
            "plugin.not_found",
            "plugin not found",
        )
        .into_response(),
        Err(PluginLifecycleError::NoUsableCandidate) => problem(
            StatusCode::CONFLICT,
            "plugin.no_usable_candidate",
            "plugin has no verified usable candidate",
        )
        .into_response(),
        Err(PluginLifecycleError::RemovalRequiresDisabled) => problem(
            StatusCode::CONFLICT,
            "plugin.remove_requires_disabled",
            "disable the plugin before removing it",
        )
        .into_response(),
        Err(PluginLifecycleError::TransitionInProgress) => problem(
            StatusCode::CONFLICT,
            "plugin.transition_in_progress",
            "plugin lifecycle transition is already in progress",
        )
        .into_response(),
        Err(PluginLifecycleError::Database(error)) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "plugin transition failed");
            unavailable()
        }
        Err(PluginLifecycleError::InvalidRecoveryCount) => {
            tracing::error!("plugin transition returned an invalid recovery result");
            unavailable()
        }
    }
}

fn unavailable() -> axum::response::Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "plugin.registry_unavailable",
        "plugin registry is unavailable",
    )
    .into_response()
}

pub(crate) fn entrypoint_unavailable() -> axum::response::Response {
    problem(
        StatusCode::NOT_FOUND,
        "resource.unavailable",
        "the requested resource is unavailable",
    )
    .into_response()
}

pub(crate) fn activation_unavailable() -> axum::response::Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "service temporarily unavailable",
    )
    .into_response()
}

#[cfg(test)]
pub(crate) fn contacts_test_projection(
    enabled: bool,
    permissions: &[&str],
) -> PluginActivationProjection {
    let manifest = plugin_contract::Manifest::parse_toml(include_str!(
        "../../../plugins/contacts/plugin.toml"
    ))
    .expect("Contacts fixture manifest is valid");
    let digest = manifest
        .supply_chain
        .artifact_digest
        .clone()
        .expect("Contacts fixture declares its digest");
    build_activation_projection(
        vec![platform_db::TenantPluginActivationRecord {
            plugin_id: manifest.id.clone(),
            installation_state: Some(if enabled { "enabled" } else { "disabled" }.into()),
            active_version: Some(manifest.version.to_string()),
            manifest: Some(serde_json::to_value(manifest).expect("fixture serializes")),
            lifecycle_state: Some("health_checked".into()),
            verified_digest: Some(digest),
            revision: chrono::Utc::now(),
        }],
        &permissions
            .iter()
            .map(|permission| (*permission).to_owned())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    const PEOPLE_READ: &str = "contacts.people.read";
    const PEOPLE_WRITE: &str = "contacts.people.write";

    fn fixture_record() -> platform_db::TenantPluginActivationRecord {
        let manifest = plugin_contract::Manifest::parse_toml(include_str!(
            "../../../plugins/contacts/plugin.toml"
        ))
        .expect("Contacts fixture manifest is valid");
        let digest = manifest
            .supply_chain
            .artifact_digest
            .clone()
            .expect("Contacts fixture declares its digest");
        platform_db::TenantPluginActivationRecord {
            plugin_id: manifest.id.clone(),
            installation_state: Some("enabled".into()),
            active_version: Some(manifest.version.to_string()),
            manifest: Some(serde_json::to_value(manifest).expect("fixture serializes")),
            lifecycle_state: Some("health_checked".into()),
            verified_digest: Some(digest),
            revision: chrono::Utc::now(),
        }
    }

    fn permissions(values: &[&str]) -> BTreeSet<String> {
        values.iter().map(|value| (*value).into()).collect()
    }

    #[test]
    fn enabled_verified_plugin_projects_manifest_navigation_and_targets() {
        let projection =
            build_activation_projection(vec![fixture_record()], &permissions(&[PEOPLE_READ]));
        assert!(projection.is_active("platform.contacts"));
        assert!(projection.owns_page("platform.contacts.people-directory"));
        assert!(projection.owns_resource("platform.contacts.person"));
        assert_eq!(
            projection.navigation(),
            vec![NavigationContribution {
                id: "contacts".into(),
                label: "People".into(),
                route: "/contacts".into(),
                icon: Some("people".into()),
                order: 100,
            }]
        );
        assert_eq!(projection.permissions(), vec![PEOPLE_READ.to_owned()]);
    }

    #[test]
    fn lifecycle_filtering_precedes_caller_permission_filtering() {
        let reduced =
            build_activation_projection(vec![fixture_record()], &permissions(&[PEOPLE_WRITE]));
        assert!(reduced.is_active("platform.contacts"));
        assert!(reduced.navigation().is_empty());
        assert_eq!(reduced.permissions(), vec![PEOPLE_WRITE.to_owned()]);
        assert_eq!(
            reduced.permissions_for("platform.contacts"),
            Some(&permissions(&[PEOPLE_WRITE]))
        );

        let mut disabled_record = fixture_record();
        disabled_record.installation_state = Some("disabled".into());
        let disabled = build_activation_projection(
            vec![disabled_record.clone()],
            &permissions(&[PEOPLE_READ, PEOPLE_WRITE]),
        );
        assert!(!disabled.is_active("platform.contacts"));
        assert!(disabled.navigation().is_empty());

        disabled_record.installation_state = Some("enabled".into());
        let re_enabled = build_activation_projection(
            vec![disabled_record],
            &permissions(&[PEOPLE_READ, PEOPLE_WRITE]),
        );
        assert!(re_enabled.is_active("platform.contacts"));
        assert_eq!(re_enabled.navigation().len(), 1);
    }

    #[test]
    fn unresolved_registry_inputs_fail_closed() {
        let mut unhealthy = fixture_record();
        unhealthy.lifecycle_state = Some("staged".into());
        assert!(
            !build_activation_projection(vec![unhealthy], &permissions(&[PEOPLE_READ]))
                .is_active("platform.contacts")
        );

        let mut malformed = fixture_record();
        malformed.manifest = Some(serde_json::json!({"id": "platform.contacts"}));
        assert!(
            !build_activation_projection(vec![malformed], &permissions(&[PEOPLE_READ]))
                .is_active("platform.contacts")
        );

        let mut mismatched_digest = fixture_record();
        mismatched_digest.verified_digest = Some(format!("sha256:{}", "0".repeat(64)));
        assert!(!build_activation_projection(
            vec![mismatched_digest],
            &permissions(&[PEOPLE_READ])
        )
        .is_active("platform.contacts"));
    }

    #[test]
    fn unsafe_navigation_from_a_registry_record_is_not_projected() {
        let mut record = fixture_record();
        let manifest = record.manifest.as_mut().expect("fixture manifest");
        manifest["ui"]["navigation"][0]["route"] = serde_json::json!("//attacker.invalid");
        let projection = build_activation_projection(vec![record], &permissions(&[PEOPLE_READ]));
        assert!(projection.is_active("platform.contacts"));
        assert!(projection.navigation().is_empty());
    }

    #[tokio::test]
    async fn unavailable_entrypoints_and_storage_failures_have_stable_safe_responses() {
        let hidden = entrypoint_unavailable();
        assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
        let hidden_body = to_bytes(hidden.into_body(), 16 * 1024)
            .await
            .expect("bounded problem body");
        let hidden_text =
            String::from_utf8(hidden_body.to_vec()).expect("problem response is UTF-8");
        assert!(hidden_text.contains("resource.unavailable"));
        assert!(!hidden_text.contains("contacts"));
        assert!(!hidden_text.contains("disabled"));

        let unavailable = activation_unavailable();
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        let unavailable_body = to_bytes(unavailable.into_body(), 16 * 1024)
            .await
            .expect("bounded problem body");
        let unavailable_text =
            String::from_utf8(unavailable_body.to_vec()).expect("problem response is UTF-8");
        assert!(unavailable_text.contains("platform.unavailable"));
        assert!(!unavailable_text.contains("plugin"));
    }
}
