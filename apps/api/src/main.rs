#![forbid(unsafe_code)]
use axum::{
    extract::{DefaultBodyLimit, Path},
    http::{header, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{get, patch, post},
    Extension, Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, RequestId, SetRequestIdLayer},
    trace::TraceLayer,
};
use uuid::Uuid;
mod api_clients;
mod auth;
mod contacts;
mod devices;
mod extensions;
mod external_hosts;
mod health;
mod objects;
mod plugins;
mod ui_delivery;
mod ui_publications;
mod ui_snapshots;
mod users;

#[derive(Clone)]
struct AppState {
    settings: Arc<platform_config::Settings>,
    pool: Option<sqlx::PgPool>,
    ui_delivery: Arc<ui_metadata_delivery::UiDeliveryService>,
    /// Primes the workspace snapshot `ui_delivery` reads; `None` when the
    /// delivery service has a fixed source (tests).
    ui_snapshots: Option<Arc<ui_snapshots::UiSnapshotProvider>>,
    ui_publications: Arc<dyn ui_publications::PublicationStore>,
    external_hosts: Arc<dyn external_hosts::ExternalHostService>,
    external_fingerprint_key: Option<Arc<str>>,
}

#[cfg(test)]
impl Default for AppState {
    fn default() -> Self {
        Self {
            settings: Arc::new(
                platform_config::Settings::load_from_paths(
                    &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                        .join("../../config/default.yml"),
                    None,
                )
                .expect("test configuration must load"),
            ),
            pool: None,
            ui_delivery: Arc::new(ui_delivery::unavailable_service()),
            ui_snapshots: None,
            ui_publications: Arc::new(ui_publications::UnavailablePublicationStore),
            external_hosts: external_hosts::DomainExternalHostService::unavailable(),
            external_fingerprint_key: None,
        }
    }
}
#[derive(Serialize)]
struct Problem {
    #[serde(rename = "type")]
    kind: String,
    title: String,
    status: u16,
    code: String,
    detail: String,
}
pub(crate) fn problem(status: StatusCode, code: &str, detail: &str) -> impl IntoResponse {
    (
        status,
        Json(Problem {
            kind: "about:blank".into(),
            title: status.canonical_reason().unwrap_or("error").into(),
            status: status.as_u16(),
            code: code.into(),
            detail: detail.into(),
        }),
    )
}
/// The caller's request id as an audit correlation id. The request-id layers
/// guarantee a single UUID header; a fresh id is the fallback for direct use.
pub(crate) fn correlation_id(request_id: &RequestId) -> Uuid {
    request_id
        .header_value()
        .to_str()
        .ok()
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::now_v7)
}
/// Record hosts linked into this API that can export the values other
/// plugins stored with their records (ADR 0012).
pub(crate) fn linked_extension_hosts(
    pool: &sqlx::PgPool,
) -> Vec<std::sync::Arc<dyn plugin_runtime_native::NativeExtensionHost>> {
    vec![std::sync::Arc::new(
        contacts_plugin::extension_host::ContactsExtensionHost::new(pool.clone()),
    )]
}

/// Native plugins whose UI metadata this host links (compiled in, verified
/// against each plugin's manifest at startup).
fn linked_ui_plugins(
) -> Result<Vec<Arc<dyn plugin_runtime_native::NativeUiPlugin>>, plugin_contract::ManifestError> {
    Ok(vec![
        Arc::new(contacts_plugin::ui::ContactsUiPlugin::new()?),
    ])
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    platform_observability::init("api");
    let settings = Arc::new(platform_config::Settings::load()?);
    // Refuse storage selections the platform cannot make safe, before any
    // store is opened (ADR 0013).
    let storage = platform_storage::selection_from(&settings.platform);
    if let Err(error) = storage.validate() {
        tracing::error!(code = error.code(), %error, "storage selection refused");
        anyhow::bail!("storage selection refused: {error}");
    }
    let pool = platform_db::connect_lazy(
        &settings.platform.database.url,
        settings.platform.database.max_connections,
        Duration::from_millis(settings.platform.database.acquire_timeout_ms),
    )?;
    plugins::spawn_initialiser(
        pool.clone(),
        settings.platform.plugins.clone(),
        storage.primary,
    );
    let ui_snapshots = Arc::new(ui_snapshots::UiSnapshotProvider::from_plugins(
        &linked_ui_plugins()?,
    )?);
    let state = AppState {
        settings: settings.clone(),
        pool: Some(pool.clone()),
        ui_delivery: Arc::new(ui_metadata_delivery::UiDeliveryService::new(
            ui_snapshots.clone(),
            Arc::new(ui_metadata_delivery::NoopDeliveryObserver),
        )),
        ui_snapshots: Some(ui_snapshots),
        ui_publications: Arc::new(ui_publications::PostgresPublicationStore::new(pool.clone())),
        external_hosts: external_hosts::DomainExternalHostService::postgres(pool.clone()),
        external_fingerprint_key: settings
            .platform
            .external_hosts
            .fingerprint_key
            .clone()
            .map(Arc::from),
    };
    let session_routes = Router::new()
        .route("/api/v1/me", get(auth::me))
        .route("/api/v1/workspaces", get(auth::workspaces))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::session_boundary,
        ));
    let workspace_routes = Router::new()
        .route("/api/v1/workspaces/:workspace_id/ui/contributions", get(ui))
        .route(
            "/api/v1/workspaces/:workspace_id/ui/pages/:page_id",
            get(ui_delivery::page),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/ui/resources/:resource_id",
            get(ui_delivery::resource),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/ui/pages/:page_id/customisation",
            get(ui_publications::get).post(ui_publications::mutate),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/ui/pages/:page_id/customisation/preview",
            get(ui_publications::preview),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/ui/pages/:page_id/customisation/compare",
            get(ui_publications::compare),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/plugins",
            get(plugins::list),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/plugins/:plugin_id/extension-plan",
            get(extensions::plan),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/plugins/:plugin_id/extension-export",
            get(extensions::export),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/plugins/:plugin_id/remove",
            post(plugins::remove),
        )
        // Metadata-driven objects (ADR 0015).
        // Validation rules (ADR 0021).
        // Saved reports (ADR 0027).
        // Composite requests (ADR 0023).
        // Device sync (ADR 0024 §4).
        // Devices (ADR 0029). `current` is the calling device's own token.
        .route(
            "/api/v1/workspaces/:workspace_id/devices",
            get(devices::list),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/devices/current",
            get(devices::current_device).delete(devices::sign_out),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/devices/current/push",
            axum::routing::put(devices::set_push).delete(devices::clear_push),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/devices/:device/revoke",
            post(devices::revoke),
        )
        // API clients (ADR 0023 §3).
        .route(
            "/api/v1/workspaces/:workspace_id/api-clients",
            get(api_clients::list).post(api_clients::create),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/api-clients/:client/revoke",
            post(api_clients::revoke),
        )
        // Bulk jobs (ADR 0023 §2): up to 5,000 rows in one request.
        // Automations (ADR 0022).
        // Global search (ADR 0020).
        // The record query language (ADR 0019).
        // Change data capture (ADR 0018). Static segments win over
        // `:object`, and API names always contain a colon.
        // Related lists and lookup candidates (ADR 0028).
        // Field history tracking (ADR 0017).
        // Record ownership and sharing (ADR 0016).
        .route(
            "/api/v1/workspaces/:workspace_id/users",
            get(users::list_users),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/roles",
            get(users::list_roles),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/users/:user_id/manager",
            axum::routing::put(users::set_manager),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/users/:user_id",
            get(users::get_user),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/extensions/fields",
            get(extensions::fields),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/extensions/layout",
            get(extensions::layout).put(extensions::save_layout),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/plugins/:plugin_id/enable",
            post(plugins::enable),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/plugins/:plugin_id/disable",
            post(plugins::disable),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/people",
            get(contacts::list_people).post(contacts::create_person),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/people/:public_id",
            get(contacts::get_person)
                .put(contacts::update_person)
                .delete(contacts::delete_person),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/people/:public_id/archive",
            post(contacts::archive_person),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/people/:public_id/undelete",
            post(contacts::undelete_person),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/people/:public_id/extensions",
            patch(contacts::update_person_extensions),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/people/:public_id/activity",
            get(contacts::person_activity),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/organisations",
            get(contacts::list_organisations).post(contacts::create_organisation),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/organisations/:public_id",
            get(contacts::get_organisation)
                .put(contacts::update_organisation)
                .delete(contacts::delete_organisation),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/organisations/:public_id/archive",
            post(contacts::archive_organisation),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/organisations/:public_id/undelete",
            post(contacts::undelete_organisation),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/organisations/:public_id/extensions",
            patch(contacts::update_organisation_extensions),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/organisations/:public_id/activity",
            get(contacts::organisation_activity),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/custom-fields",
            post(contacts::create_definition),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/custom-fields/:target",
            get(contacts::list_definitions),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/custom-fields/definitions/:id",
            get(contacts::get_definition).put(contacts::update_definition),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/custom-fields/definitions/:id/disable",
            post(contacts::disable_definition),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/contacts/custom-fields/definitions/:id/archive",
            post(contacts::archive_definition),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/odata/$metadata",
            get(odata_metadata),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/odata/contacts/people",
            get(contacts::odata_people),
        )
        .merge(platform_objects_http::routes::<AppState>())
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::workspace_boundary,
        ));
    let app = Router::new()
        .route("/health/live", get(health::live))
        .route("/health/ready", get(health::ready))
        .route("/auth/oidc/login", get(auth::login))
        .route("/auth/oidc/callback", get(auth::callback))
        .route("/auth/logout", post(auth::logout))
        // OAuth 2.0 client credentials for API clients (ADR 0023 §3).
        .route("/oauth/token", post(api_clients::token))
        // Device sign-in through the system browser (ADR 0029).
        .route("/auth/device/login", get(devices::login))
        .route(
            "/api/v1/external/forms/:slug",
            get(external_hosts::public_form),
        )
        .route(
            "/api/v1/external/forms/:slug/submissions",
            post(external_hosts::submit)
                .layer(DefaultBodyLimit::max(1_048_576))
                .layer(middleware::from_fn_with_state(
                    state.clone(),
                    external_hosts::fingerprint,
                ))
                .layer(middleware::from_fn(external_hosts::secure_headers)),
        )
        .route(
            "/api/v1/external/portals/:slug",
            get(external_hosts::portal).layer(middleware::from_fn(
                external_hosts::portal_principal_boundary,
            )),
        )
        .merge(session_routes)
        .merge(workspace_routes)
        .with_state(state)
        .layer(PropagateRequestIdLayer::x_request_id())
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(normalize_request_id));
    let addr: SocketAddr = settings.platform.services.api_bind.parse()?;
    tracing::info!(%addr,"api listening");
    axum::serve(
        tokio::net::TcpListener::bind(addr).await?,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
async fn normalize_request_id(
    mut request: axum::extract::Request,
    next: middleware::Next,
) -> axum::response::Response {
    let request_ids = request.headers().get_all("x-request-id");
    let mut request_ids = request_ids.iter();
    let valid_single_request_id = request_ids
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .is_some()
        && request_ids.next().is_none();

    if request.headers().contains_key("x-request-id") && !valid_single_request_id {
        request.headers_mut().remove("x-request-id");
    }

    next.run(request).await
}
async fn ui(
    axum::extract::State(state): axum::extract::State<AppState>,
    Path(workspace_id): Path<Uuid>,
    Extension(principal): Extension<auth::WorkspacePrincipal>,
    Extension(caller): Extension<platform_objects_http::Caller>,
) -> impl IntoResponse {
    debug_assert_eq!(principal.workspace.tenant_id, workspace_id);
    let mut navigation: Vec<Value> = principal
        .activation
        .navigation()
        .into_iter()
        .filter_map(|item| serde_json::to_value(item).ok())
        .collect();
    let (objects, mut object_permissions) =
        platform_objects_http::ui_contributions(&state, &caller).await;
    if caller
        .permissions
        .contains(users::MEMBERS_MANAGE_PERMISSION)
    {
        object_permissions.push(users::MEMBERS_MANAGE_PERMISSION.to_owned());
    }
    // Object metadata changes navigation without changing plugin
    // activation, so it is part of the revision the shell revalidates.
    let revision = if objects.is_empty() && object_permissions.is_empty() {
        principal.activation.revision().to_owned()
    } else {
        format!(
            "{}.{:016x}",
            principal.activation.revision(),
            stable_hash(&json!([objects, object_permissions]).to_string())
        )
    };
    navigation.extend(objects);
    let mut permissions = principal.activation.permissions();
    permissions.extend(object_permissions);
    permissions.sort();
    permissions.dedup();
    (
        [(header::CACHE_CONTROL, "private, no-store")],
        Json(json!({
            "navigation": navigation,
            "permissions": permissions,
            "revision": revision,
        })),
    )
}

/// FNV-1a: identical on every API replica, unlike the standard hasher.
fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}
async fn odata_metadata(
    Path(workspace_id): Path<Uuid>,
    Extension(principal): Extension<auth::WorkspacePrincipal>,
) -> axum::response::Response {
    if !principal.activation.is_active("platform.contacts") {
        return plugins::entrypoint_unavailable();
    }
    let metadata = format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<edmx:Edmx Version="4.01" xmlns:edmx="http://docs.oasis-open.org/odata/ns/edmx">
  <edmx:DataServices>
    <Schema Namespace="Platform.Contacts" xmlns="http://docs.oasis-open.org/odata/ns/edm">
      <EntityType Name="Person">
        <Key><PropertyRef Name="id" /></Key>
        <Property Name="id" Type="Edm.String" Nullable="false" />
        <Property Name="workspace_id" Type="Edm.String" Nullable="false" />
        <Property Name="full_name" Type="Edm.String" Nullable="false" />
        <Property Name="email" Type="Edm.String" />
        <Property Name="version" Type="Edm.Int64" Nullable="false" />
      </EntityType>
      <EntityContainer Name="Workspace{}">
        <EntitySet Name="People" EntityType="Platform.Contacts.Person" />
      </EntityContainer>
    </Schema>
  </edmx:DataServices>
</edmx:Edmx>"#,
        workspace_id
    );
    ([("content-type", "application/xml")], metadata).into_response()
}

#[cfg(test)]
mod request_id_tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
        Extension,
    };
    use tower::ServiceExt;
    use tower_http::request_id::RequestId;

    async fn echo_request_id(Extension(request_id): Extension<RequestId>) -> String {
        request_id
            .header_value()
            .to_str()
            .expect("request IDs generated or accepted by the API are ASCII")
            .to_owned()
    }

    fn app() -> Router {
        Router::new()
            .route("/", get(echo_request_id))
            .layer(PropagateRequestIdLayer::x_request_id())
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .layer(middleware::from_fn(normalize_request_id))
    }

    async fn effective_request_id(request_id: &str) -> (String, String) {
        let response = app()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("x-request-id", request_id)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let response_id = response
            .headers()
            .get("x-request-id")
            .expect("the effective request ID is propagated")
            .to_str()
            .unwrap()
            .to_owned();
        let body = to_bytes(response.into_body(), 128).await.unwrap();
        let extracted_id = String::from_utf8(body.to_vec()).unwrap();
        (response_id, extracted_id)
    }

    #[tokio::test]
    async fn valid_request_id_is_preserved_end_to_end() {
        let request_id = Uuid::now_v7().to_string();
        let (response_id, extracted_id) = effective_request_id(&request_id).await;

        assert_eq!(response_id, request_id);
        assert_eq!(extracted_id, request_id);
    }

    #[tokio::test]
    async fn malformed_request_id_is_replaced_consistently() {
        let (response_id, extracted_id) = effective_request_id("not-a-uuid").await;

        assert_eq!(response_id, extracted_id);
        assert!(Uuid::parse_str(&response_id).is_ok());
    }
}
