//! Kernel extension field service endpoints (ADR 0012).
//!
//! These routes are domain-neutral: they describe and plan extension fields
//! for any host resource. Hosts mount their own value-writing routes and call
//! [`catalog`] and [`CallerAccess`] so every write and read goes through the
//! kernel extension service.

use crate::{auth::WorkspacePrincipal, plugins::PluginActivationProjection, problem, AppState};
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use platform_extensions::{
    zones_for, ExtensionAccess, ExtensionCatalog, ExtensionError, ExtensionPlan, FieldLayout,
};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

/// The extension catalog of the caller's workspace: fields from active
/// plugins whose hosts are active and compatible.
pub(crate) fn catalog(activation: &PluginActivationProjection) -> ExtensionCatalog {
    ExtensionCatalog::from_manifests(activation.active_manifests())
}

/// Answers extension permission checks from the caller's workspace
/// permissions, filtered to what each plugin declares.
pub(crate) struct CallerAccess<'a>(pub(crate) &'a PluginActivationProjection);

impl ExtensionAccess for CallerAccess<'_> {
    fn allows(&self, plugin_id: &str, permission: &str) -> bool {
        self.0
            .permissions_for(plugin_id)
            .is_some_and(|permissions| permissions.contains(permission))
    }
}

#[derive(Deserialize)]
pub(crate) struct FieldsQuery {
    resource: String,
}

/// The stored layout for a resource, or `None` when none was saved or it no
/// longer parses (defaults then apply).
async fn stored_layout(
    pool: &sqlx::PgPool,
    tenant_id: Uuid,
    resource: &str,
) -> Result<(Option<FieldLayout>, i64), sqlx::Error> {
    Ok(
        match platform_db::get_field_layout(pool, tenant_id, resource).await? {
            Some(stored) => (
                serde_json::from_value::<FieldLayout>(stored.layout).ok(),
                stored.revision,
            ),
            None => (None, 0),
        },
    )
}

/// `GET .../extensions/fields?resource=…`: caller-safe descriptors of the
/// extension fields on one resource that the caller may read, with the host
/// zones and where each field is placed in this workspace.
pub(crate) async fn fields(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    query: Result<Query<FieldsQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return problem(
            StatusCode::BAD_REQUEST,
            "extension.resource_required",
            "a resource query parameter is required",
        )
        .into_response();
    };
    if !principal.activation.owns_resource(&query.resource) {
        return crate::plugins::entrypoint_unavailable();
    }
    let descriptors = catalog(&principal.activation)
        .descriptors(&query.resource, &CallerAccess(&principal.activation));
    let catalog = catalog(&principal.activation);
    let zones = zones_for(principal.activation.active_manifests(), &query.resource);
    let layout = match state.pool.as_ref() {
        Some(pool) => {
            match stored_layout(pool, principal.workspace.tenant_id, &query.resource).await {
                Ok((layout, _)) => layout,
                Err(error) => {
                    tracing::error!(error_type = %std::any::type_name_of_val(&error), "field layout lookup failed");
                    return crate::plugins::activation_unavailable();
                }
            }
        }
        None => None,
    };
    let readable = descriptors
        .iter()
        .map(|descriptor| descriptor.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let placements = catalog
        .effective_placements(&query.resource, &zones, layout.as_ref())
        .into_iter()
        .filter(|placement| readable.contains(placement.field_id.as_str()))
        .collect::<Vec<_>>();
    (
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "resource": query.resource,
            "revision": principal.activation.revision(),
            "data": descriptors,
            "zones": zones,
            "placements": placements,
        })),
    )
        .into_response()
}

#[derive(Deserialize)]
pub(crate) struct LayoutInput {
    expected_revision: i64,
    #[serde(default)]
    arranged: Vec<String>,
    #[serde(default)]
    placements: Vec<platform_extensions::FieldPlacement>,
}

fn resource_required() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "extension.resource_required",
        "a resource query parameter is required",
    )
    .into_response()
}

/// `GET .../extensions/layout?resource=…`: everything an administrator needs
/// to arrange a resource's extension fields: every active field, the host's
/// zones, the stored layout and its revision, and the effective placements.
pub(crate) async fn layout(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    query: Result<Query<FieldsQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return resource_required();
    };
    if !principal.activation.owns_resource(&query.resource) {
        return crate::plugins::entrypoint_unavailable();
    }
    let Some(pool) = state.pool.as_ref() else {
        return crate::plugins::activation_unavailable();
    };
    let (stored, revision) = match stored_layout(
        pool,
        principal.workspace.tenant_id,
        &query.resource,
    )
    .await
    {
        Ok(found) => found,
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "field layout lookup failed");
            return crate::plugins::activation_unavailable();
        }
    };
    let catalog = catalog(&principal.activation);
    let zones = zones_for(principal.activation.active_manifests(), &query.resource);
    // Administrators arrange every active field, including ones their own
    // role cannot read; labels and types are metadata, never record values.
    let all = |_: &str, _: &str| true;
    let fields = catalog.descriptors(&query.resource, &all);
    let placements = catalog.effective_placements(&query.resource, &zones, stored.as_ref());
    (
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "resource": query.resource,
            "revision": revision,
            "fields": fields,
            "zones": zones,
            "layout": stored.unwrap_or_default(),
            "placements": placements,
        })),
    )
        .into_response()
}

/// `PUT .../extensions/layout?resource=…`: saves an administrator's
/// arrangement after the kernel validates it against active fields and zones.
pub(crate) async fn save_layout(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<tower_http::request_id::RequestId>,
    query: Result<Query<FieldsQuery>, QueryRejection>,
    Json(input): Json<LayoutInput>,
) -> Response {
    let Ok(Query(query)) = query else {
        return resource_required();
    };
    if !principal.activation.owns_resource(&query.resource) {
        return crate::plugins::entrypoint_unavailable();
    }
    let Some(pool) = state.pool.as_ref() else {
        return crate::plugins::activation_unavailable();
    };
    let catalog = catalog(&principal.activation);
    let zones = zones_for(principal.activation.active_manifests(), &query.resource);
    let layout = match catalog.validate_layout(
        &query.resource,
        &zones,
        &FieldLayout {
            arranged: input.arranged,
            placements: input.placements,
        },
    ) {
        Ok(layout) => layout,
        Err(ExtensionError::Invalid(issues)) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "type": "about:blank",
                    "title": "Bad Request",
                    "status": 400,
                    "code": "validation.failed",
                    "detail": "one or more values are invalid",
                    "errors": issues,
                })),
            )
                .into_response()
        }
        Err(_) => return crate::plugins::activation_unavailable(),
    };
    let Ok(document) = serde_json::to_value(&layout) else {
        return crate::plugins::activation_unavailable();
    };
    match platform_db::save_field_layout(
        pool,
        principal.workspace.tenant_id,
        principal.session.user.id,
        &query.resource,
        &document,
        input.expected_revision,
        Some(crate::correlation_id(&request_id)),
    )
    .await
    {
        Ok(revision) => (
            StatusCode::OK,
            Json(json!({
                "resource": query.resource,
                "revision": revision,
                "layout": layout,
                "placements": catalog.effective_placements(&query.resource, &zones, Some(&layout)),
            })),
        )
            .into_response(),
        Err(platform_db::FieldLayoutError::RevisionConflict) => problem(
            StatusCode::CONFLICT,
            "extension.layout_conflict",
            "the layout changed since it was read",
        )
        .into_response(),
        Err(platform_db::FieldLayoutError::Database(error)) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "field layout save failed");
            crate::plugins::activation_unavailable()
        }
    }
}

/// `GET .../plugins/{plugin}/extension-plan`: what enabling a plugin would
/// add to other plugins, where values are stored, the permissions it
/// introduces, and which fields would pause, for review before enabling.
pub(crate) async fn plan(
    State(state): State<AppState>,
    Path((workspace_id, plugin_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return crate::plugins::activation_unavailable();
    };
    let manifest = match platform_db::latest_usable_plugin_manifest(pool, workspace_id, &plugin_id)
        .await
    {
        Ok(Some(manifest)) => manifest,
        Ok(None) => return plugin_not_found(),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "extension plan lookup failed");
            return crate::plugins::activation_unavailable();
        }
    };
    let Ok(candidate) = serde_json::from_value::<plugin_contract::Manifest>(manifest) else {
        tracing::error!("stored plugin manifest could not be parsed for an extension plan");
        return crate::plugins::activation_unavailable();
    };
    if candidate.id != plugin_id {
        return plugin_not_found();
    }
    let plan = ExtensionPlan::for_plugin(&candidate, principal.activation.active_manifests());
    (
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(json!({ "data": plan })),
    )
        .into_response()
}

/// Most records one export returns per call.
const EXPORT_LIMIT: usize = 10_000;

/// `GET .../plugins/{plugin}/extension-export`: every value a plugin stored
/// with other plugins' records in this workspace, so an administrator can
/// keep a copy before removal purges them. The caller must manage plugins
/// and hold the read permission of every field the plugin declares.
pub(crate) async fn export(
    State(state): State<AppState>,
    Path((workspace_id, plugin_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<tower_http::request_id::RequestId>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return crate::plugins::activation_unavailable();
    };
    let manifest = match platform_db::latest_usable_plugin_manifest(pool, workspace_id, &plugin_id)
        .await
    {
        Ok(Some(manifest)) => manifest,
        Ok(None) => return plugin_not_found(),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "extension export lookup failed");
            return crate::plugins::activation_unavailable();
        }
    };
    let Ok(candidate) = serde_json::from_value::<plugin_contract::Manifest>(manifest) else {
        return crate::plugins::activation_unavailable();
    };
    if candidate.id != plugin_id {
        return plugin_not_found();
    }
    let may_read_all = candidate.extensions.fields.iter().all(|field| {
        principal
            .workspace
            .permissions
            .contains(&field.read_permission)
    });
    if !may_read_all {
        return problem(
            StatusCode::FORBIDDEN,
            "auth.permission_denied",
            "permission denied",
        )
        .into_response();
    }
    let hosts = candidate
        .extensions
        .fields
        .iter()
        .map(|field| field.host.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let linked = crate::linked_extension_hosts(pool);
    let mut records = Vec::new();
    let mut truncated = false;
    let mut unavailable_hosts = Vec::new();
    for host_id in hosts {
        let Some(host) = linked.iter().find(|host| host.host_plugin_id() == host_id) else {
            unavailable_hosts.push(host_id.to_owned());
            continue;
        };
        let remaining = EXPORT_LIMIT.saturating_sub(records.len());
        match host
            .export_extension_namespace(workspace_id, &plugin_id, remaining)
            .await
        {
            Ok((exported, more)) => {
                truncated |= more;
                records.extend(exported.into_iter().map(|record| {
                    json!({
                        "host": host_id,
                        "resource": record.resource,
                        "record_id": record.record_id,
                        "values": record.values,
                    })
                }));
            }
            Err(error) => {
                tracing::error!(error = %error, host = host_id, "extension export failed");
                return crate::plugins::activation_unavailable();
            }
        }
    }
    if let Err(error) = platform_db::record_extension_export(
        pool,
        workspace_id,
        principal.session.user.id,
        &plugin_id,
        Some(crate::correlation_id(&request_id)),
        records.len(),
    )
    .await
    {
        tracing::error!(error_type = %std::any::type_name_of_val(&error), "extension export could not be audited");
        return crate::plugins::activation_unavailable();
    }
    (
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "plugin_id": plugin_id,
            "data": records,
            "truncated": truncated,
            "unavailable_hosts": unavailable_hosts,
        })),
    )
        .into_response()
}

fn plugin_not_found() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "plugin.not_found",
        "plugin not found",
    )
    .into_response()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::plugins::build_activation_projection;
    use plugin_contract::Manifest;
    use std::collections::BTreeSet;

    const DIGEST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

    pub(crate) const LOYALTY: &str = r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty tiers"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[supply_chain]
artifact_digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111"

[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.read"
description = "Read loyalty fields"
[[permissions]]
id = "loyalty.write"
description = "Change loyalty fields"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Loyalty tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }]
read_permission = "loyalty.read"
write_permission = "loyalty.write"
"#;

    fn record(
        manifest: &Manifest,
        digest: &str,
        enabled: bool,
    ) -> platform_db::TenantPluginActivationRecord {
        platform_db::TenantPluginActivationRecord {
            plugin_id: manifest.id.clone(),
            installation_state: Some(if enabled { "enabled" } else { "disabled" }.into()),
            active_version: Some(manifest.version.to_string()),
            manifest: Some(serde_json::to_value(manifest).expect("manifest json")),
            lifecycle_state: Some("health_checked".into()),
            verified_digest: Some(digest.into()),
            revision: chrono::Utc::now(),
        }
    }

    /// Contacts plus the Loyalty extension, with the given caller permissions.
    pub(crate) fn projection(
        loyalty_enabled: bool,
        permissions: &[&str],
    ) -> PluginActivationProjection {
        let contacts = Manifest::parse_toml(include_str!("../../../plugins/contacts/plugin.toml"))
            .expect("Contacts manifest");
        let contacts_digest = contacts
            .supply_chain
            .artifact_digest
            .clone()
            .expect("Contacts digest");
        let loyalty = Manifest::parse_toml(LOYALTY).expect("Loyalty manifest");
        build_activation_projection(
            vec![
                record(&contacts, &contacts_digest, true),
                record(&loyalty, DIGEST, loyalty_enabled),
            ],
            &permissions
                .iter()
                .map(|permission| (*permission).to_owned())
                .collect::<BTreeSet<_>>(),
        )
    }

    #[test]
    fn descriptors_follow_activation_and_caller_permissions() {
        let reader = projection(true, &["contacts.people.read", "loyalty.read"]);
        let descriptors =
            catalog(&reader).descriptors("platform.contacts.person", &CallerAccess(&reader));
        assert_eq!(descriptors.len(), 1);
        assert_eq!(descriptors[0].id, "example.loyalty.tier");
        assert!(!descriptors[0].writable);

        let writer = projection(true, &["loyalty.read", "loyalty.write"]);
        assert!(
            catalog(&writer).descriptors("platform.contacts.person", &CallerAccess(&writer))[0]
                .writable
        );

        let without = projection(true, &["contacts.people.read"]);
        assert!(catalog(&without)
            .descriptors("platform.contacts.person", &CallerAccess(&without))
            .is_empty());

        let disabled = projection(false, &["loyalty.read", "loyalty.write"]);
        assert!(catalog(&disabled)
            .descriptors("platform.contacts.person", &CallerAccess(&disabled))
            .is_empty());
    }

    #[test]
    fn caller_access_never_lends_one_plugins_permissions_to_another() {
        let projection = projection(true, &["loyalty.read", "contacts.people.read"]);
        let access = CallerAccess(&projection);
        assert!(access.allows("example.loyalty", "loyalty.read"));
        assert!(!access.allows("platform.contacts", "loyalty.read"));
        assert!(!access.allows("example.loyalty", "contacts.people.read"));
        assert!(!access.allows("example.loyalty", "loyalty.write"));
    }

    #[test]
    fn plan_describes_a_disabled_plugin_against_the_active_hosts() {
        let projection = projection(false, &[]);
        let loyalty = Manifest::parse_toml(LOYALTY).expect("Loyalty manifest");
        let plan = ExtensionPlan::for_plugin(&loyalty, projection.active_manifests());
        assert_eq!(plan.hosts, ["platform.contacts"]);
        assert_eq!(plan.fields.len(), 1);
        assert_eq!(plan.fields[0].paused, None);
        assert_eq!(plan.permissions, ["loyalty.read", "loyalty.write"]);
    }
}
