use crate::{problem, AppState};
use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Redirect, Response},
    Extension, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use contacts_plugin::{
    ORGANISATIONS_READ_PERMISSION, ORGANISATIONS_WRITE_PERMISSION, PEOPLE_READ_PERMISSION,
    PEOPLE_WRITE_PERMISSION, SETTINGS_MANAGE_PERMISSION,
};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata},
    reqwest, AccessTokenHash, AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl,
    Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope,
    TokenResponse,
};
use platform_db::PluginDesiredState;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tower_http::request_id::RequestId;
use ui_metadata_delivery::UiCaller;
use uuid::Uuid;

const LOGIN_TTL_SECONDS: i64 = 600;

#[derive(Debug, Clone)]
pub(crate) struct SessionPrincipal {
    pub user: platform_db::AuthUser,
    pub session_id: Uuid,
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspacePrincipal {
    pub session: SessionPrincipal,
    pub workspace: platform_db::WorkspaceAccess,
    pub activation: crate::plugins::PluginActivationProjection,
    /// The device whose access token authenticated the request (ADR 0029).
    pub device: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

pub(crate) async fn login(State(state): State<AppState>) -> Response {
    start_login(&state, None).await
}

/// Sends the browser to the identity provider with a fresh state, nonce and
/// PKCE verifier. A device app's sign-in (ADR 0029) carries its own
/// redirect URI and PKCE challenge, kept with the transaction for the
/// callback.
pub(crate) async fn start_login(
    state: &AppState,
    device: Option<platform_db::DeviceLogin>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let settings = &state.settings.platform;
    let http_client = match oidc_http_client() {
        Ok(client) => client,
        Err(()) => return unavailable(),
    };
    let issuer = match IssuerUrl::new(settings.oidc.issuer_url.clone()) {
        Ok(issuer) => issuer,
        Err(_) => return unavailable(),
    };
    let metadata = match CoreProviderMetadata::discover_async(issuer, &http_client).await {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(error_type = %std::any::type_name_of_val(&error), "OIDC discovery unavailable");
            return unavailable();
        }
    };
    let client_secret = settings.oidc.client_secret.clone().map(ClientSecret::new);
    let redirect_url = match RedirectUrl::new(settings.oidc.redirect_url.clone()) {
        Ok(url) => url,
        Err(_) => return unavailable(),
    };
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(settings.oidc.client_id.clone()),
        client_secret,
    )
    .set_redirect_uri(redirect_url);
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let (authorization_url, state_token, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(challenge)
        .url();
    if let Err(error) = platform_db::store_oidc_login(
        pool,
        &secret_hash(state_token.secret()),
        nonce.secret(),
        verifier.secret(),
        Utc::now() + Duration::seconds(LOGIN_TTL_SECONDS),
        device.as_ref(),
    )
    .await
    {
        tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to persist OIDC transaction");
        return unavailable();
    }
    Redirect::temporary(authorization_url.as_str()).into_response()
}

pub(crate) async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    if query.error.is_some() {
        return oidc_rejected();
    }
    let (Some(code), Some(raw_state)) = (query.code, query.state) else {
        return oidc_rejected();
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let transaction = match platform_db::consume_oidc_login(pool, &secret_hash(&raw_state)).await {
        Ok(Some(transaction)) => transaction,
        Ok(None) => return oidc_rejected(),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to consume OIDC transaction");
            return unavailable();
        }
    };
    let http_client = match oidc_http_client() {
        Ok(client) => client,
        Err(()) => return unavailable(),
    };
    let settings = &state.settings.platform;
    let issuer = match IssuerUrl::new(settings.oidc.issuer_url.clone()) {
        Ok(issuer) => issuer,
        Err(_) => return unavailable(),
    };
    let metadata = match CoreProviderMetadata::discover_async(issuer, &http_client).await {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(error_type = %std::any::type_name_of_val(&error), "OIDC discovery unavailable");
            return unavailable();
        }
    };
    let redirect_url = match RedirectUrl::new(settings.oidc.redirect_url.clone()) {
        Ok(url) => url,
        Err(_) => return unavailable(),
    };
    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(settings.oidc.client_id.clone()),
        settings.oidc.client_secret.clone().map(ClientSecret::new),
    )
    .set_redirect_uri(redirect_url);
    let exchange = match client.exchange_code(AuthorizationCode::new(code)) {
        Ok(exchange) => exchange,
        Err(_) => return oidc_rejected(),
    };
    let token_response = match exchange
        .set_pkce_verifier(PkceCodeVerifier::new(transaction.pkce_verifier))
        .request_async(&http_client)
        .await
    {
        Ok(tokens) => tokens,
        Err(error) => {
            tracing::warn!(error_type = %std::any::type_name_of_val(&error), "OIDC token exchange rejected");
            return oidc_rejected();
        }
    };
    let Some(id_token) = token_response.id_token() else {
        return oidc_rejected();
    };
    let verifier = client.id_token_verifier();
    let nonce = Nonce::new(transaction.nonce);
    let claims = match id_token.claims(&verifier, &nonce) {
        Ok(claims) => claims,
        Err(error) => {
            tracing::warn!(error_type = %std::any::type_name_of_val(&error), "OIDC ID token rejected");
            return oidc_rejected();
        }
    };
    if let Some(expected_hash) = claims.access_token_hash() {
        let actual_hash = AccessTokenHash::from_token(
            token_response.access_token(),
            match id_token.signing_alg() {
                Ok(algorithm) => algorithm,
                Err(_) => return oidc_rejected(),
            },
            match id_token.signing_key(&verifier) {
                Ok(key) => key,
                Err(_) => return oidc_rejected(),
            },
        );
        if actual_hash.as_ref().ok() != Some(expected_hash) {
            return oidc_rejected();
        }
    }
    let email = claims.email().map(|email| email.as_str());
    let display_name = email.unwrap_or_else(|| claims.subject().as_str());
    let user = match platform_db::find_or_create_external_user(
        pool,
        claims.issuer().as_str(),
        claims.subject().as_str(),
        display_name,
        email,
    )
    .await
    {
        Ok(user) => user,
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to link external identity");
            return unavailable();
        }
    };
    if let Some(device) = transaction.device {
        return crate::devices::return_to_app(&state, user.id, &device).await;
    }
    let raw_session = new_session_token();
    let expires_at = Utc::now() + Duration::seconds(settings.sessions.ttl_seconds as i64);
    if let Err(error) =
        platform_db::create_session(pool, &secret_hash(&raw_session), user.id, expires_at).await
    {
        tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to create session");
        return unavailable();
    }
    let mut response = Redirect::to(&settings.oidc.post_logout_redirect_url).into_response();
    if let Ok(value) = HeaderValue::from_str(&session_cookie(&state, &raw_session)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

pub(crate) async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let (Some(pool), Some(raw)) = (state.pool.as_ref(), session_token(&state, &headers)) {
        if let Err(error) = platform_db::revoke_session(pool, &secret_hash(raw)).await {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to revoke session");
            return unavailable();
        }
    }
    let mut response =
        Redirect::to(&state.settings.platform.oidc.post_logout_redirect_url).into_response();
    if let Ok(value) = HeaderValue::from_str(&expired_cookie(&state)) {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response
}

pub(crate) async fn session_boundary(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    let principal = match resolve_principal(&state, &headers).await {
        Ok(Some(principal)) => principal,
        Ok(None) => return unauthenticated(),
        Err(SessionStoreUnavailable) => return unavailable(),
    };
    request.extensions_mut().insert(principal);
    next.run(request).await
}

pub(crate) async fn workspace_boundary(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut request: Request,
    next: Next,
) -> Response {
    // An API client's bearer token replaces the session cookie entirely, so
    // a request is never authenticated by both (ADR 0023 §3).
    // An Authorization header that is not a usable bearer token is
    // refused rather than ignored, so it never falls back to the cookie.
    if headers.contains_key(header::AUTHORIZATION) && bearer_token(&headers).is_none() {
        return unauthenticated();
    }
    let (session, workspace, device) = if let Some(token) = bearer_token(&headers) {
        let Some(workspace_id) = workspace_id_from_path(request.uri().path()) else {
            return concealed_workspace();
        };
        let engine_route = client_route(request.uri().path());
        let own_device_route = device_route(request.uri().path());
        let directory = directory_route(request.method(), request.uri().path());
        if !engine_route && !own_device_route && !directory {
            return client_not_allowed();
        }
        let Some(pool) = state.pool.as_ref() else {
            return unavailable();
        };
        let token_hash = secret_hash(token);
        match platform_db::resolve_api_token_access(pool, &token_hash, workspace_id).await {
            Ok(Some((workspace, user_id, client))) => {
                if !engine_route {
                    return client_not_allowed();
                }
                (
                    SessionPrincipal {
                        user: platform_db::AuthUser {
                            id: user_id,
                            display_name: String::new(),
                            email: None,
                        },
                        // The client stands in for the session it replaces.
                        session_id: client,
                    },
                    workspace,
                    None,
                )
            }
            Ok(None) => {
                match platform_db::resolve_device_token_access(pool, &token_hash, workspace_id)
                    .await
                {
                    Ok(Some(found)) if found.wipe => return crate::devices::wipe_required(),
                    Ok(Some(found)) => (
                        SessionPrincipal {
                            user: platform_db::AuthUser {
                                id: found.user_id,
                                display_name: String::new(),
                                email: None,
                            },
                            // The device stands in for the session it replaces.
                            session_id: found.device,
                        },
                        found.access,
                        Some(found.device),
                    ),
                    Ok(None) => return unauthenticated(),
                    Err(error) => {
                        tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to resolve device token");
                        return unavailable();
                    }
                }
            }
            Err(error) => {
                tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to resolve API client token");
                return unavailable();
            }
        }
    } else {
        let session = match resolve_principal(&state, &headers).await {
            Ok(Some(session)) => session,
            Ok(None) => return unauthenticated(),
            Err(SessionStoreUnavailable) => return unavailable(),
        };
        let Some(workspace_id) = workspace_id_from_path(request.uri().path()) else {
            return concealed_workspace();
        };
        let Some(pool) = state.pool.as_ref() else {
            return unavailable();
        };
        let workspace = match platform_db::resolve_workspace_access(
            pool,
            session.session_id,
            session.user.id,
            workspace_id,
        )
        .await
        {
            Ok(Some(access)) => access,
            Ok(None) => return concealed_workspace(),
            Err(error) => {
                tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to resolve workspace principal");
                return unavailable();
            }
        };
        (session, workspace, None)
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let activation = match crate::plugins::activation_projection(&state, &workspace).await {
        Ok(projection) => projection,
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to resolve workspace plugin activation");
            return crate::plugins::activation_unavailable();
        }
    };
    if required_plugin(request.uri().path())
        .is_some_and(|plugin_id| !activation.is_active(plugin_id))
    {
        return crate::plugins::entrypoint_unavailable();
    }
    if required_ui_target(request.uri().path()).is_some_and(|target| match target {
        UiTarget::Page(page_id) => !activation.owns_page(page_id),
        UiTarget::Resource(resource_id) => !activation.owns_resource(resource_id),
    }) {
        return crate::ui_delivery::page_unavailable_response();
    }
    if let Some(required) = required_permission(request.method().as_str(), request.uri().path()) {
        if !workspace.permissions.contains(required) {
            if let Some((plugin_id, desired)) =
                lifecycle_target(request.method().as_str(), request.uri().path())
            {
                let correlation_id = request
                    .extensions()
                    .get::<RequestId>()
                    .map(crate::correlation_id);
                if let Err(error) = platform_db::record_plugin_transition_rejected(
                    pool,
                    workspace.tenant_id,
                    session.user.id,
                    plugin_id,
                    desired,
                    "auth.permission_denied",
                    correlation_id,
                )
                .await
                {
                    tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to audit a rejected plugin transition");
                    return unavailable();
                }
            }
            return permission_denied();
        }
    }
    let ui_caller = UiCaller {
        user_id: session.user.id.to_string(),
        workspace_id: workspace.tenant_id.to_string(),
        active_membership: true,
        membership_version: 1,
        roles: workspace.roles.clone(),
        permissions: workspace.permissions.clone(),
    };
    request.extensions_mut().insert(ui_caller);
    request
        .extensions_mut()
        .insert(platform_objects_http::Caller {
            tenant_id: workspace.tenant_id,
            user_id: session.user.id,
            permissions: workspace.permissions.clone(),
        });
    request.extensions_mut().insert(WorkspacePrincipal {
        session,
        workspace,
        activation,
        device,
    });
    next.run(request).await
}

pub(crate) async fn me(
    Extension(principal): Extension<SessionPrincipal>,
) -> Json<serde_json::Value> {
    Json(json!({
        "user": {
            "id": principal.user.id,
            "display_name": principal.user.display_name,
            "email": principal.user.email,
        },
        "session_id": principal.session_id,
    }))
}

pub(crate) async fn workspaces(
    State(state): State<AppState>,
    Extension(principal): Extension<SessionPrincipal>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::list_workspace_access(pool, principal.session_id, principal.user.id).await {
        Ok(workspaces) => Json(json!({
            "data": workspaces.into_iter().map(|workspace| json!({
                "id": workspace.tenant_id,
                "name": workspace.display_name,
                "roles": workspace.roles,
            })).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to list workspaces");
            unavailable()
        }
    }
}

/// The session store could not be read. Callers answer 503 rather than 401:
/// a database outage must not look like an expired session to the browser.
struct SessionStoreUnavailable;

async fn resolve_principal(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<SessionPrincipal>, SessionStoreUnavailable> {
    let Some(raw) = session_token(state, headers) else {
        return Ok(None);
    };
    let Some(pool) = state.pool.as_ref() else {
        return Err(SessionStoreUnavailable);
    };
    let now = Utc::now();
    let settings = &state.settings.platform.sessions;
    let renewal_boundary = now + Duration::seconds(settings.renewal_window_seconds as i64);
    let renewed_expiry = now + Duration::seconds(settings.ttl_seconds as i64);
    match platform_db::resolve_session(pool, &secret_hash(raw), renewal_boundary, renewed_expiry)
        .await
    {
        Ok(Some(session)) => Ok(Some(SessionPrincipal {
            user: session.user,
            session_id: session.id,
        })),
        Ok(None) => Ok(None),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to resolve session");
            Err(SessionStoreUnavailable)
        }
    }
}

fn oidc_http_client() -> Result<reqwest::Client, ()> {
    reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ())
}

fn new_session_token() -> String {
    URL_SAFE_NO_PAD.encode(rand::random::<[u8; 32]>())
}

fn secret_hash(secret: &str) -> String {
    format!("{:x}", Sha256::digest(secret.as_bytes()))
}

/// The token of an `Authorization: Bearer` header.
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|token| !token.is_empty())
}

/// Workspace routes API clients may call: the object engine's records,
/// metadata, queries, search, composite requests, bulk jobs and device
/// sync. Everything else, including managing API clients and members,
/// needs a person.
const CLIENT_ROUTE_PREFIXES: &[&str] = &["objects", "query", "search", "composite", "bulk", "sync"];

fn client_route(path: &str) -> bool {
    workspace_relative_path(path).is_some_and(|relative| {
        CLIENT_ROUTE_PREFIXES.iter().any(|prefix| {
            relative == *prefix
                || relative
                    .strip_prefix(*prefix)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        })
    })
}

/// The member directory a synced device keeps for offline use (ADR 0024,
/// "Offline directory"): reading the workspace's member and role names,
/// which every member may do in the browser. Only a device's token reaches
/// it; an API client's is refused with the other non-engine routes.
fn directory_route(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::GET
        && workspace_relative_path(path)
            .is_some_and(|relative| relative == "users" || relative == "roles")
}

/// Workspace routes about the calling device itself, which only a device's
/// own access token reaches (ADR 0029).
fn device_route(path: &str) -> bool {
    workspace_relative_path(path).is_some_and(|relative| {
        relative == "devices/current"
            || relative
                .strip_prefix("devices/current")
                .is_some_and(|suffix| suffix.starts_with('/'))
    })
}

fn client_not_allowed() -> Response {
    problem(
        StatusCode::FORBIDDEN,
        "auth.client_route_denied",
        "API clients may call only the object engine routes",
    )
    .into_response()
}

pub(crate) fn new_secret() -> String {
    new_session_token()
}

pub(crate) fn hash_secret(secret: &str) -> String {
    secret_hash(secret)
}

fn session_token<'a>(state: &AppState, headers: &'a HeaderMap) -> Option<&'a str> {
    let cookie_name = &state.settings.platform.sessions.cookie_name;
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|cookie| cookie.trim().split_once('='))
        .find_map(|(name, value)| (name == cookie_name).then_some(value))
        .filter(|value| !value.is_empty())
}

fn session_cookie(state: &AppState, token: &str) -> String {
    let settings = &state.settings.platform.sessions;
    let secure = if settings.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{}={}; Path=/; HttpOnly; SameSite={}; Max-Age={}{}",
        settings.cookie_name, token, settings.same_site, settings.ttl_seconds, secure
    )
}

fn expired_cookie(state: &AppState) -> String {
    let settings = &state.settings.platform.sessions;
    let secure = if settings.cookie_secure {
        "; Secure"
    } else {
        ""
    };
    format!(
        "{}=; Path=/; HttpOnly; SameSite={}; Max-Age=0{}",
        settings.cookie_name, settings.same_site, secure
    )
}

fn workspace_id_from_path(path: &str) -> Option<Uuid> {
    let mut segments = path.trim_matches('/').split('/');
    while let Some(segment) = segments.next() {
        if segment == "workspaces" {
            return segments
                .next()
                .and_then(|value| Uuid::parse_str(value).ok());
        }
    }
    None
}

fn required_permission(method: &str, path: &str) -> Option<&'static str> {
    if path.contains("/customisation") {
        return Some(ui_metadata::publication::MANAGE_PERMISSION);
    }
    if path.ends_with("/extensions/layout") {
        return Some(ui_metadata::publication::MANAGE_PERMISSION);
    }
    if path.ends_with("/extension-plan") || path.ends_with("/extension-export") {
        return Some("platform.plugins.manage");
    }
    // Extension values carry their own write permissions, checked by the
    // kernel extension service; the host route needs only record visibility.
    if path.ends_with("/extensions") && method == "PATCH" {
        if path.contains("/contacts/people/") {
            return Some(PEOPLE_READ_PERMISSION);
        }
        if path.contains("/contacts/organisations/") {
            return Some(ORGANISATIONS_READ_PERMISSION);
        }
    }
    if path.contains("/plugins/") && method != "GET" {
        return Some("platform.plugins.manage");
    }
    if path.contains("/contacts/custom-fields") {
        return Some(SETTINGS_MANAGE_PERMISSION);
    }
    if path.contains("/contacts/people") {
        return Some(if matches!(method, "GET" | "HEAD") {
            PEOPLE_READ_PERMISSION
        } else {
            PEOPLE_WRITE_PERMISSION
        });
    }
    if path.contains("/contacts/organisations") {
        return Some(if matches!(method, "GET" | "HEAD") {
            ORGANISATIONS_READ_PERMISSION
        } else {
            ORGANISATIONS_WRITE_PERMISSION
        });
    }
    if path.contains("/odata/") {
        return Some(PEOPLE_READ_PERMISSION);
    }
    if path.contains("/api-clients") {
        return Some(crate::api_clients::API_CLIENTS_MANAGE_PERMISSION);
    }
    None
}

struct PluginRouteOwner {
    plugin_id: &'static str,
    workspace_relative_prefixes: &'static [&'static str],
}

// Route ownership is platform routing data, not lifecycle policy. The
// activation service remains domain-neutral and this table grows only when a
// plugin-owned API surface is mounted by the host.
const PLUGIN_ROUTE_OWNERS: &[PluginRouteOwner] = &[PluginRouteOwner {
    plugin_id: "platform.contacts",
    workspace_relative_prefixes: &["contacts", "odata"],
}];

fn required_plugin(path: &str) -> Option<&'static str> {
    let relative = workspace_relative_path(path)?;
    PLUGIN_ROUTE_OWNERS.iter().find_map(|owner| {
        owner
            .workspace_relative_prefixes
            .iter()
            .any(|prefix| {
                relative == *prefix
                    || relative
                        .strip_prefix(*prefix)
                        .is_some_and(|suffix| suffix.starts_with('/'))
            })
            .then_some(owner.plugin_id)
    })
}

/// A plugin lifecycle request: `POST .../plugins/{plugin_id}/enable|disable|remove`.
fn lifecycle_target<'a>(method: &str, path: &'a str) -> Option<(&'a str, PluginDesiredState)> {
    if method != "POST" {
        return None;
    }
    let relative = workspace_relative_path(path)?;
    let mut segments = relative.split('/');
    let (Some("plugins"), Some(plugin_id), Some(action), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    let desired = match action {
        "enable" => PluginDesiredState::Enabled,
        "disable" => PluginDesiredState::Disabled,
        "remove" => PluginDesiredState::Removed,
        _ => return None,
    };
    Some((plugin_id, desired))
}

fn workspace_relative_path(path: &str) -> Option<&str> {
    let (_, after_marker) = path.split_once("/workspaces/")?;
    let (_, relative) = after_marker.split_once('/')?;
    Some(relative)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiTarget<'a> {
    Page(&'a str),
    Resource(&'a str),
}

fn required_ui_target(path: &str) -> Option<UiTarget<'_>> {
    let relative = workspace_relative_path(path)?;
    if let Some(path) = relative.strip_prefix("ui/pages/") {
        let id = path.split('/').next().unwrap_or_default();
        return (!id.is_empty()).then_some(UiTarget::Page(id));
    }
    if let Some(path) = relative.strip_prefix("ui/resources/") {
        let id = path.split('/').next().unwrap_or_default();
        return (!id.is_empty()).then_some(UiTarget::Resource(id));
    }
    None
}

fn unauthenticated() -> Response {
    problem(
        StatusCode::UNAUTHORIZED,
        "auth.unauthenticated",
        "authentication is required",
    )
    .into_response()
}

fn concealed_workspace() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "workspace.not_found",
        "workspace not found",
    )
    .into_response()
}

fn permission_denied() -> Response {
    problem(
        StatusCode::FORBIDDEN,
        "auth.permission_denied",
        "permission denied",
    )
    .into_response()
}

fn oidc_rejected() -> Response {
    problem(
        StatusCode::UNAUTHORIZED,
        "auth.oidc_rejected",
        "identity provider response was rejected",
    )
    .into_response()
}

fn unavailable() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "service temporarily unavailable",
    )
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn lifecycle_target_matches_only_plugin_transitions() {
        let base = "/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/plugins";
        assert_eq!(
            lifecycle_target("POST", &format!("{base}/platform.contacts/enable")),
            Some(("platform.contacts", PluginDesiredState::Enabled))
        );
        assert_eq!(
            lifecycle_target("POST", &format!("{base}/platform.contacts/disable")),
            Some(("platform.contacts", PluginDesiredState::Disabled))
        );
        for (method, path) in [
            ("GET", format!("{base}/platform.contacts/enable")),
            ("POST", base.to_owned()),
            ("POST", format!("{base}/platform.contacts")),
            ("POST", format!("{base}/platform.contacts/upgrade")),
            ("POST", format!("{base}/platform.contacts/enable/extra")),
            (
                "POST",
                "/api/v1/workspaces/x/contacts/people/enable".to_owned(),
            ),
        ] {
            assert_eq!(lifecycle_target(method, &path), None, "{method} {path}");
        }
    }

    #[test]
    fn permission_mapping_is_route_boundary_specific() {
        let contacts_cases = [
            (
                "GET",
                "/api/v1/workspaces/x/contacts/people",
                PEOPLE_READ_PERMISSION,
            ),
            (
                "POST",
                "/api/v1/workspaces/x/contacts/people",
                PEOPLE_WRITE_PERMISSION,
            ),
            (
                "GET",
                "/api/v1/workspaces/x/contacts/people/per1",
                PEOPLE_READ_PERMISSION,
            ),
            (
                "HEAD",
                "/api/v1/workspaces/x/contacts/people/per1",
                PEOPLE_READ_PERMISSION,
            ),
            (
                "PUT",
                "/api/v1/workspaces/x/contacts/people/per1",
                PEOPLE_WRITE_PERMISSION,
            ),
            (
                "DELETE",
                "/api/v1/workspaces/x/contacts/people/per1",
                PEOPLE_WRITE_PERMISSION,
            ),
            (
                "GET",
                "/api/v1/workspaces/x/contacts/organisations",
                ORGANISATIONS_READ_PERMISSION,
            ),
            (
                "POST",
                "/api/v1/workspaces/x/contacts/organisations",
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                "GET",
                "/api/v1/workspaces/x/contacts/organisations/org1",
                ORGANISATIONS_READ_PERMISSION,
            ),
            (
                "HEAD",
                "/api/v1/workspaces/x/contacts/organisations/org1",
                ORGANISATIONS_READ_PERMISSION,
            ),
            (
                "PUT",
                "/api/v1/workspaces/x/contacts/organisations/org1",
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                "DELETE",
                "/api/v1/workspaces/x/contacts/organisations/org1",
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                "POST",
                "/api/v1/workspaces/x/contacts/custom-fields",
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                "GET",
                "/api/v1/workspaces/x/contacts/custom-fields/person",
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                "GET",
                "/api/v1/workspaces/x/contacts/custom-fields/definitions/1",
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                "PUT",
                "/api/v1/workspaces/x/contacts/custom-fields/definitions/1",
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                "POST",
                "/api/v1/workspaces/x/contacts/custom-fields/definitions/1/disable",
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                "POST",
                "/api/v1/workspaces/x/contacts/custom-fields/definitions/1/archive",
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                "GET",
                "/api/v1/workspaces/x/odata/contacts/people",
                PEOPLE_READ_PERMISSION,
            ),
        ];

        for (method, path, permission) in contacts_cases {
            assert_eq!(required_permission(method, path), Some(permission));
        }
        assert_eq!(
            required_permission("POST", "/api/v1/workspaces/x/plugins/a/enable"),
            Some("platform.plugins.manage")
        );
        assert_eq!(
            required_permission("GET", "/api/v1/workspaces/x/plugins/a/extension-plan"),
            Some("platform.plugins.manage")
        );
        assert_eq!(
            required_permission("GET", "/api/v1/workspaces/x/plugins/a/extension-export"),
            Some("platform.plugins.manage")
        );
        assert_eq!(
            required_permission("POST", "/api/v1/workspaces/x/plugins/a/remove"),
            Some("platform.plugins.manage")
        );
        assert_eq!(
            lifecycle_target("POST", "/api/v1/workspaces/x/plugins/a.b/remove"),
            Some(("a.b", PluginDesiredState::Removed))
        );
        assert_eq!(
            required_permission(
                "PATCH",
                "/api/v1/workspaces/x/contacts/people/per_1/extensions"
            ),
            Some(PEOPLE_READ_PERMISSION)
        );
        assert_eq!(
            required_permission(
                "PATCH",
                "/api/v1/workspaces/x/contacts/organisations/org_1/extensions"
            ),
            Some(ORGANISATIONS_READ_PERMISSION)
        );
        assert_eq!(
            required_permission("GET", "/api/v1/workspaces/x/extensions/fields"),
            None
        );
        // Any member may open a colleague's user record (ADR 0014); the
        // route is kernel-owned and needs no plugin.
        assert_eq!(
            required_permission("GET", "/api/v1/workspaces/x/users/u"),
            None
        );
        assert_eq!(required_plugin("/api/v1/workspaces/x/users/u"), None);
        for method in ["GET", "PUT"] {
            assert_eq!(
                required_permission(method, "/api/v1/workspaces/x/extensions/layout"),
                Some(ui_metadata::publication::MANAGE_PERMISSION)
            );
        }
    }

    #[test]
    fn only_plugin_owned_routes_require_an_active_plugin() {
        assert_eq!(
            required_plugin("/api/v1/workspaces/x/contacts/people"),
            Some("platform.contacts")
        );
        assert_eq!(
            required_plugin("/api/v1/workspaces/x/odata/$metadata"),
            Some("platform.contacts")
        );
        assert_eq!(
            required_plugin("/api/v1/workspaces/x/odata/contacts/people"),
            Some("platform.contacts")
        );
        assert_eq!(
            required_plugin("/api/v1/workspaces/x/plugins/platform.contacts/enable"),
            None
        );
        assert_eq!(
            required_plugin("/api/v1/workspaces/x/ui/contributions"),
            None
        );
    }

    #[test]
    fn runtime_ui_targets_are_identified_before_handler_extraction() {
        assert_eq!(
            required_ui_target("/api/v1/workspaces/x/ui/pages/platform.contacts.people-directory"),
            Some(UiTarget::Page("platform.contacts.people-directory"))
        );
        assert_eq!(
            required_ui_target("/api/v1/workspaces/x/ui/resources/platform.contacts.person"),
            Some(UiTarget::Resource("platform.contacts.person"))
        );
        assert_eq!(
            required_ui_target(
                "/api/v1/workspaces/x/ui/pages/platform.contacts.people-directory/customisation"
            ),
            Some(UiTarget::Page("platform.contacts.people-directory"))
        );
        assert_eq!(
            required_ui_target(
                "/api/v1/workspaces/x/ui/pages/platform.contacts.people-directory/customisation/preview"
            ),
            Some(UiTarget::Page("platform.contacts.people-directory"))
        );
        assert_eq!(
            required_ui_target(
                "/api/v1/workspaces/x/ui/pages/platform.contacts.people-directory/customisation/compare"
            ),
            Some(UiTarget::Page("platform.contacts.people-directory"))
        );
    }

    #[test]
    fn workspace_parser_rejects_non_uuid_and_does_not_echo_path() {
        assert!(workspace_id_from_path("/api/v1/workspaces/secret/plugins").is_none());
        assert_eq!(
            workspace_id_from_path(
                "/api/v1/workspaces/018f0000-0000-7000-8000-000000000001/plugins"
            ),
            Some(Uuid::parse_str("018f0000-0000-7000-8000-000000000001").expect("fixture UUID"))
        );
    }

    #[test]
    fn api_clients_reach_only_object_engine_routes() {
        let workspace = "/api/v1/workspaces/018f0000-0000-7000-8000-000000000001";
        for allowed in [
            "objects",
            "objects/custom:deal/records",
            "query",
            "search",
            "composite",
            "bulk/018f0000-0000-7000-8000-000000000002/results",
            "sync/revocations",
        ] {
            assert!(client_route(&format!("{workspace}/{allowed}")), "{allowed}");
        }
        for denied in [
            "api-clients",
            "users/x/manager",
            "plugins/p/enable",
            "ui/contributions",
            "contacts/people",
            "objectsx",
            "bulkhead",
            "syncs",
        ] {
            assert!(!client_route(&format!("{workspace}/{denied}")), "{denied}");
        }
    }

    #[test]
    fn only_reading_members_and_roles_is_a_directory_route() {
        let workspace = "/api/v1/workspaces/018f0000-0000-7000-8000-000000000001";
        for allowed in ["users", "roles"] {
            assert!(directory_route(
                &axum::http::Method::GET,
                &format!("{workspace}/{allowed}")
            ));
            assert!(!directory_route(
                &axum::http::Method::POST,
                &format!("{workspace}/{allowed}")
            ));
        }
        for denied in [
            "users/018f0000-0000-7000-8000-000000000002",
            "users/018f0000-0000-7000-8000-000000000002/manager",
            "usersx",
            "api-clients",
            "devices",
        ] {
            assert!(
                !directory_route(&axum::http::Method::GET, &format!("{workspace}/{denied}")),
                "{denied}"
            );
        }
    }

    #[test]
    fn only_the_current_device_routes_are_device_routes() {
        let workspace = "/api/v1/workspaces/018f0000-0000-7000-8000-000000000001";
        for allowed in ["devices/current", "devices/current/push"] {
            assert!(device_route(&format!("{workspace}/{allowed}")), "{allowed}");
            assert!(
                !client_route(&format!("{workspace}/{allowed}")),
                "{allowed}"
            );
        }
        for denied in [
            "devices",
            "devices/018f0000-0000-7000-8000-000000000002/revoke",
            "devices/currently",
            "objects",
        ] {
            assert!(!device_route(&format!("{workspace}/{denied}")), "{denied}");
        }
    }

    #[test]
    fn bearer_tokens_need_the_bearer_scheme_and_a_value() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(bearer_token(&headers), Some("abc"));
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("bearer  abc "),
        );
        assert_eq!(bearer_token(&headers), Some("abc"));
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert_eq!(bearer_token(&headers), None);
        headers.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic abc"));
        assert_eq!(bearer_token(&headers), None);
    }

    #[test]
    fn session_tokens_are_high_entropy_and_only_hashes_are_stable() {
        let first = new_session_token();
        let second = new_session_token();
        assert_ne!(first, second);
        assert!(first.len() >= 43);
        assert_eq!(secret_hash(&first).len(), 64);
    }

    #[test]
    fn session_cookie_is_http_only_and_environment_secure() {
        let mut state = AppState::default();
        Arc::make_mut(&mut state.settings)
            .platform
            .sessions
            .cookie_secure = true;
        let cookie = session_cookie(&state, "opaque-token");
        assert!(cookie.starts_with("platform_session=opaque-token;"));
        assert!(cookie.contains("; HttpOnly;"));
        assert!(cookie.contains("; SameSite=Lax;"));
        assert!(cookie.ends_with("; Secure"));
        assert!(!cookie.contains("postgres://"));
    }
}
