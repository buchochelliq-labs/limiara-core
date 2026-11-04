use crate::{problem, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Deserialize;
use std::collections::BTreeMap;
use ui_metadata_delivery::{
    CallerSafePageResponse, CallerSafeResourceResponse, Delivery, DeliveryStatus, SnapshotError,
    UiCaller, UiDeliveryError, UiFailureCategory, UiPageRequest, UiResourceRequest,
};

#[cfg(test)]
pub(crate) use unavailable::unavailable_service;

/// A source that is never ready, for tests that do not exercise delivery.
#[cfg(test)]
mod unavailable {
    use std::sync::Arc;
    use ui_metadata_delivery::{
        NoopDeliveryObserver, SnapshotError, UiDeliveryService, UiSnapshotSource,
        UiWorkspaceSnapshot,
    };

    struct UnavailableSnapshotSource;

    impl UiSnapshotSource for UnavailableSnapshotSource {
        fn snapshot(&self, _workspace_id: &str) -> Result<UiWorkspaceSnapshot, SnapshotError> {
            Err(SnapshotError::TemporarilyUnavailable)
        }

        fn is_ready(&self) -> bool {
            false
        }
    }

    pub(crate) fn unavailable_service() -> UiDeliveryService {
        UiDeliveryService::new(
            Arc::new(UnavailableSnapshotSource),
            Arc::new(NoopDeliveryObserver),
        )
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct PageQuery {
    #[serde(default = "default_locale")]
    locale: String,
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default = "default_form_factor")]
    form_factor: String,
    audience: Option<String>,
    resource_subtype: Option<String>,
    #[serde(default)]
    record: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResourceQuery {
    #[serde(default = "default_locale")]
    locale: String,
    #[serde(default = "default_host")]
    host: String,
}

fn default_locale() -> String {
    "en-GB".into()
}

fn default_host() -> String {
    "internal-web".into()
}

fn default_mode() -> String {
    "view".into()
}

fn default_form_factor() -> String {
    "wide".into()
}

pub(crate) async fn page(
    State(state): State<AppState>,
    principal: Option<Extension<crate::auth::WorkspacePrincipal>>,
    caller: Option<Extension<UiCaller>>,
    Path((workspace_id, page_id)): Path<(String, String)>,
    Query(query): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(Extension(principal)) = principal else {
        return page_unavailable_response();
    };
    let Some(Extension(caller)) = caller else {
        return page_unavailable_response();
    };
    if !principal.activation.owns_page(&page_id) {
        return page_unavailable_response();
    }
    let Some(host) = parse_host(&query.host) else {
        return invalid_request_response();
    };
    let request = UiPageRequest {
        workspace_id,
        page_id,
        locale: query.locale,
        host,
        mode: query.mode,
        form_factor: query.form_factor,
        audience: query.audience,
        resource_subtype: query.resource_subtype,
        record_context: query.record,
        if_none_match: header_string(&headers, header::IF_NONE_MATCH),
    };
    if let Err(response) = prime_snapshot(&state, &principal).await {
        return response;
    }
    match state.ui_delivery.deliver_page(&caller, &request) {
        Ok(delivery) => delivery_response(delivery),
        Err(error) => error_response(error),
    }
}

pub(crate) async fn resource(
    State(state): State<AppState>,
    principal: Option<Extension<crate::auth::WorkspacePrincipal>>,
    caller: Option<Extension<UiCaller>>,
    Path((workspace_id, resource_id)): Path<(String, String)>,
    Query(query): Query<ResourceQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(Extension(principal)) = principal else {
        return page_unavailable_response();
    };
    let Some(Extension(caller)) = caller else {
        return page_unavailable_response();
    };
    if !principal.activation.owns_resource(&resource_id) {
        return page_unavailable_response();
    }
    let Some(host) = parse_host(&query.host) else {
        return invalid_request_response();
    };
    let request = UiResourceRequest {
        workspace_id,
        resource_id,
        host,
        locale: query.locale,
        if_none_match: header_string(&headers, header::IF_NONE_MATCH),
    };
    if let Err(response) = prime_snapshot(&state, &principal).await {
        return response;
    }
    match state.ui_delivery.deliver_resource(&caller, &request) {
        Ok(delivery) => delivery_response(delivery),
        Err(error) => error_response(error),
    }
}

/// Build the workspace snapshot delivery is about to read. Without a
/// provider (a fixed test source) there is nothing to prime.
pub(crate) async fn prime_snapshot(
    state: &AppState,
    principal: &crate::auth::WorkspacePrincipal,
) -> Result<(), Response> {
    let Some(provider) = state.ui_snapshots.as_ref() else {
        return Ok(());
    };
    let Some(pool) = state.pool.as_ref() else {
        return Err(snapshot_error_response(
            SnapshotError::TemporarilyUnavailable,
        ));
    };
    provider
        .prime(pool, principal.workspace.tenant_id, &principal.activation)
        .await
        .map_err(snapshot_error_response)
}

fn snapshot_error_response(error: SnapshotError) -> Response {
    let category = match error {
        SnapshotError::Hidden => UiFailureCategory::PageUnavailable,
        SnapshotError::Incompatible => UiFailureCategory::Incompatible,
        SnapshotError::Conflict => UiFailureCategory::Conflict,
        SnapshotError::TemporarilyUnavailable => UiFailureCategory::TemporarilyUnavailable,
    };
    error_response(UiDeliveryError { category })
}

fn delivery_response<T: serde::Serialize>(delivery: Delivery<T>) -> Response {
    let mut response = match (delivery.status, delivery.body) {
        (DeliveryStatus::NotModified, _) => StatusCode::NOT_MODIFIED.into_response(),
        (DeliveryStatus::Fresh, Some(body)) => (StatusCode::OK, Json(body)).into_response(),
        (DeliveryStatus::Fresh, None) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if let Ok(value) = HeaderValue::from_str(&delivery.etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static(delivery.cache_control),
    );
    response
}

fn error_response(error: UiDeliveryError) -> Response {
    let status = match error.category {
        UiFailureCategory::PageUnavailable => StatusCode::NOT_FOUND,
        UiFailureCategory::Incompatible | UiFailureCategory::Conflict => StatusCode::CONFLICT,
        UiFailureCategory::TemporarilyUnavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    problem(status, error.code(), error.public_detail()).into_response()
}

pub(crate) fn page_unavailable_response() -> Response {
    let error = UiDeliveryError {
        category: UiFailureCategory::PageUnavailable,
    };
    error_response(error)
}

fn invalid_request_response() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "ui.invalid_request",
        "The UI request is invalid.",
    )
    .into_response()
}

fn parse_host(host: &str) -> Option<plugin_contract::ui::HostProfile> {
    match host {
        "internal-web" => Some(plugin_contract::ui::HostProfile::InternalWeb),
        "desktop" => Some(plugin_contract::ui::HostProfile::Desktop),
        "portal" => Some(plugin_contract::ui::HostProfile::Portal),
        "public-form" => Some(plugin_contract::ui::HostProfile::PublicForm),
        _ => None,
    }
}

fn header_string(headers: &HeaderMap, name: header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

// Keep response DTOs referenced here so API compile checks fail if the delivery
// contract stops being serializable or is accidentally removed.
const _: fn(CallerSafePageResponse, CallerSafeResourceResponse) = |_, _| {};

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::collections::BTreeSet;

    fn state() -> AppState {
        AppState::default()
    }

    fn query(host: &str) -> PageQuery {
        PageQuery {
            locale: "en-GB".into(),
            host: host.into(),
            mode: "view".into(),
            form_factor: "wide".into(),
            audience: None,
            resource_subtype: None,
            record: BTreeMap::new(),
        }
    }

    fn caller() -> UiCaller {
        UiCaller {
            user_id: "user-a".into(),
            workspace_id: "workspace-a".into(),
            active_membership: true,
            membership_version: 1,
            roles: BTreeSet::new(),
            permissions: BTreeSet::new(),
        }
    }

    fn principal(enabled: bool) -> crate::auth::WorkspacePrincipal {
        let user_id =
            uuid::Uuid::parse_str("018f0000-0000-7000-8000-000000000103").expect("fixture UUID");
        crate::auth::WorkspacePrincipal {
            session: crate::auth::SessionPrincipal {
                user: platform_db::AuthUser {
                    id: user_id,
                    display_name: "Test user".into(),
                    email: None,
                },
                session_id: uuid::Uuid::parse_str("018f0000-0000-7000-8000-000000000303")
                    .expect("fixture UUID"),
            },
            workspace: platform_db::WorkspaceAccess {
                tenant_id: uuid::Uuid::parse_str("018f0000-0000-7000-8000-000000000001")
                    .expect("fixture UUID"),
                display_name: "Test workspace".into(),
                roles: BTreeSet::new(),
                permissions: BTreeSet::from(["contacts.people.read".into()]),
            },
            activation: crate::plugins::contacts_test_projection(
                enabled,
                &["contacts.people.read"],
            ),
            device: None,
        }
    }

    #[tokio::test]
    async fn endpoint_fails_closed_without_membership_context() {
        let response = page(
            State(state()),
            None,
            None,
            Path(("workspace-secret".into(), "plugin.secret.page".into())),
            Query(query("internal-web")),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(text.contains("ui.page_unavailable"));
        assert!(!text.contains("workspace-secret"));
        assert!(!text.contains("plugin.secret"));
    }

    #[tokio::test]
    async fn endpoint_reports_provider_unavailability_without_demo_fallbacks() {
        let response = page(
            State(state()),
            Some(Extension(principal(true))),
            Some(Extension(caller())),
            Path((
                "workspace-a".into(),
                "platform.contacts.people-directory".into(),
            )),
            Query(query("internal-web")),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn disabled_page_and_resource_targets_are_non_enumerating() {
        let page_response = page(
            State(state()),
            Some(Extension(principal(false))),
            Some(Extension(caller())),
            Path((
                "workspace-a".into(),
                "platform.contacts.people-directory".into(),
            )),
            Query(query("internal-web")),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(page_response.status(), StatusCode::NOT_FOUND);
        let page_body = to_bytes(page_response.into_body(), 16 * 1024)
            .await
            .expect("bounded body");
        let page_text = String::from_utf8(page_body.to_vec()).expect("problem JSON is UTF-8");
        assert!(page_text.contains("ui.page_unavailable"));
        assert!(!page_text.contains("contacts"));

        let resource_response = resource(
            State(state()),
            Some(Extension(principal(false))),
            Some(Extension(caller())),
            Path(("workspace-a".into(), "platform.contacts.person".into())),
            Query(ResourceQuery {
                locale: "en-GB".into(),
                host: "internal-web".into(),
            }),
            HeaderMap::new(),
        )
        .await;
        assert_eq!(resource_response.status(), StatusCode::NOT_FOUND);
        let resource_body = to_bytes(resource_response.into_body(), 16 * 1024)
            .await
            .expect("bounded body");
        let resource_text =
            String::from_utf8(resource_body.to_vec()).expect("problem JSON is UTF-8");
        assert!(resource_text.contains("ui.page_unavailable"));
        assert!(!resource_text.contains("contacts"));
    }

    #[test]
    fn response_adapter_applies_private_cache_and_exact_etag_headers() {
        let fresh = delivery_response(Delivery {
            status: DeliveryStatus::Fresh,
            body: Some(serde_json::json!({"page":"safe"})),
            etag: "\"ui-safe\"".into(),
            cache_control: "private, max-age=0, must-revalidate",
            cache_hit: false,
        });
        assert_eq!(fresh.status(), StatusCode::OK);
        assert_eq!(fresh.headers()[header::ETAG], "\"ui-safe\"");
        assert_eq!(
            fresh.headers()[header::CACHE_CONTROL],
            "private, max-age=0, must-revalidate"
        );

        let not_modified = delivery_response(Delivery::<serde_json::Value> {
            status: DeliveryStatus::NotModified,
            body: None,
            etag: "\"ui-safe\"".into(),
            cache_control: "private, max-age=0, must-revalidate",
            cache_hit: true,
        });
        assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    }
}
