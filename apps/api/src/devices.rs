//! Devices (ADR 0029, ADR 0024 §4): phone and desktop apps that sign a
//! member in through the system browser and hold a replica of one
//! workspace.
//!
//! - `GET /auth/device/login` starts an OIDC sign-in for an app. The
//!   server runs the authorization code flow with PKCE against the
//!   identity provider as it does for the web, then returns a one-time code
//!   to the app's registered redirect URI (RFC 8252).
//! - `POST /oauth/token` redeems the code with the app's own PKCE verifier
//!   (`authorization_code`) and rotates tokens (`refresh_token`).
//! - Members list and revoke their devices; `devices.manage` reaches
//!   everyone's. A revoked device, or one whose member left, is told to
//!   wipe its replica the next time it calls.
//!
//! Codes and tokens are random and stored only as hashes. There are no
//! passwords.

use crate::{auth::WorkspacePrincipal, problem, AppState};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Extension, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{Duration, Utc};
use openidconnect::url::Url;
use serde::Deserialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tower_http::request_id::RequestId;
use uuid::Uuid;

/// See and revoke every member's devices.
pub(crate) const DEVICES_MANAGE_PERMISSION: &str = "devices.manage";
/// How long a one-time code waits for the app to redeem it.
const CODE_TTL_SECONDS: i64 = 120;
/// How long an access token lasts.
const ACCESS_TTL_SECONDS: i64 = 3600;
/// How long a refresh token lasts unused. Each use replaces it.
const REFRESH_TTL_SECONDS: i64 = 30 * 24 * 3600;
const NAME_MAX: usize = 80;
const STATE_MAX: usize = 512;
const PUSH_TOKEN_MAX: usize = 4096;
const PLATFORMS: [&str; 3] = ["ios", "android", "desktop"];
const PUSH_PROVIDERS: [&str; 2] = ["apns", "fcm"];

fn unavailable() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "the service is temporarily unavailable",
    )
    .into_response()
}

fn invalid(code: &str, detail: &str) -> Response {
    problem(StatusCode::BAD_REQUEST, code, detail).into_response()
}

fn not_found() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "resource.unavailable",
        "the requested resource is unavailable",
    )
    .into_response()
}

/// The answer to a revoked device, or one whose member left: delete this
/// workspace's replica. Unlike a plain `401`, signing in again does not
/// keep the data (ADR 0024 §4).
pub(crate) fn wipe_required() -> Response {
    problem(
        StatusCode::UNAUTHORIZED,
        "auth.device_wipe_required",
        "this device no longer has access to the workspace; delete its data",
    )
    .into_response()
}

/// The S256 PKCE challenge of a verifier (RFC 7636 §4.2).
fn challenge_of(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// A PKCE verifier: 43 to 128 unreserved characters (RFC 7636 §4.1).
fn valid_verifier(verifier: &str) -> bool {
    (43..=128).contains(&verifier.len())
        && verifier
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
}

/// An S256 challenge: 43 base64url characters.
fn valid_challenge(challenge: &str) -> bool {
    challenge.len() == 43
        && challenge
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn device_json(device: &platform_db::Device, current: Option<Uuid>) -> Value {
    json!({
        "id": device.id,
        "name": device.name,
        "platform": device.platform,
        "member": crate::users::user_reference(Some(device.user_id)),
        "push": device.push_provider,
        "created_at": device.created_at,
        "last_seen_at": device.last_seen_at,
        "revoked_at": device.revoked_at,
        "revoked_by": crate::users::user_reference(device.revoked_by),
        "revoked_reason": device.revoked_reason,
        "current": current == Some(device.id),
    })
}

// --- Sign-in -----------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct LoginQuery {
    redirect_uri: Option<String>,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    state: Option<String>,
}

/// Starts a device app's sign-in in the system browser. Problems are shown
/// in the browser rather than sent to an unregistered redirect URI.
pub(crate) async fn login(
    State(state): State<AppState>,
    Query(query): Query<LoginQuery>,
) -> Response {
    let devices = &state.settings.platform.devices;
    let Some(redirect_uri) = query
        .redirect_uri
        .filter(|uri| devices.redirect_uris.iter().any(|allowed| allowed == uri))
    else {
        return invalid(
            "auth.device_redirect_invalid",
            "the redirect URI is not registered for device sign-in",
        );
    };
    let Some(code_challenge) = query.code_challenge.filter(|value| valid_challenge(value)) else {
        return invalid(
            "auth.device_challenge_invalid",
            "send an S256 PKCE code challenge",
        );
    };
    if query.code_challenge_method.as_deref() != Some("S256") {
        return invalid(
            "auth.device_challenge_invalid",
            "send an S256 PKCE code challenge",
        );
    }
    let Some(app_state) = query
        .state
        .filter(|value| !value.is_empty() && value.len() <= STATE_MAX)
    else {
        return invalid(
            "auth.device_state_invalid",
            "send a state of 1 to 512 characters",
        );
    };
    crate::auth::start_login(
        &state,
        Some(platform_db::DeviceLogin {
            redirect_uri,
            code_challenge,
            state: app_state,
        }),
    )
    .await
}

/// Finishes a device app's sign-in after the identity provider's callback:
/// stores a one-time code for the person and returns it to the app.
pub(crate) async fn return_to_app(
    state: &AppState,
    user_id: Uuid,
    login: &platform_db::DeviceLogin,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let Ok(mut target) = Url::parse(&login.redirect_uri) else {
        return unavailable();
    };
    let code = crate::auth::new_secret();
    if let Err(error) = platform_db::create_device_authorization(
        pool,
        &crate::auth::hash_secret(&code),
        user_id,
        login,
        Utc::now() + Duration::seconds(CODE_TTL_SECONDS),
    )
    .await
    {
        tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to store a device authorization");
        return unavailable();
    }
    target
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", &login.state);
    let mut response = Redirect::to(target.as_str()).into_response();
    no_store(response.headers_mut());
    response
}

// --- Token grants (RFC 6749 §4.1.3, §6) ------------------------------------------

fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
}

/// An OAuth error response. `wipe` tells the device to delete its replica.
fn grant_error(error: &str, description: &str, wipe: bool) -> Response {
    let mut body = json!({"error": error, "error_description": description});
    if wipe {
        body["wipe"] = Value::Bool(true);
    }
    let mut response = (StatusCode::BAD_REQUEST, Json(body)).into_response();
    no_store(response.headers_mut());
    response
}

fn tokens_json(
    state: &AppState,
    access: &str,
    refresh: &str,
    tenant_id: Uuid,
    device: Uuid,
) -> Response {
    let mut response = Json(json!({
        "access_token": access,
        "token_type": "Bearer",
        "expires_in": ACCESS_TTL_SECONDS,
        "refresh_token": refresh,
        "workspace_id": tenant_id,
        "device_id": device,
        "offline_limit_seconds": state.settings.platform.devices.offline_limit_seconds,
    }))
    .into_response();
    no_store(response.headers_mut());
    response
}

/// The device fields of a token request.
pub(crate) struct CodeGrant<'a> {
    pub code: Option<&'a str>,
    pub code_verifier: Option<&'a str>,
    pub redirect_uri: Option<&'a str>,
    pub workspace_id: Option<&'a str>,
    pub device_name: Option<&'a str>,
    pub platform: Option<&'a str>,
}

/// Redeems a one-time code: registers the device in the workspace and
/// returns its first tokens.
pub(crate) async fn authorization_code(
    state: &AppState,
    request_id: &RequestId,
    grant: CodeGrant<'_>,
) -> Response {
    let (Some(code), Some(verifier), Some(redirect_uri), Some(workspace_id)) = (
        grant.code,
        grant.code_verifier,
        grant.redirect_uri,
        grant.workspace_id,
    ) else {
        return grant_error(
            "invalid_request",
            "send code, code_verifier, redirect_uri and workspace_id",
            false,
        );
    };
    let Ok(tenant_id) = Uuid::parse_str(workspace_id) else {
        return grant_error("invalid_request", "workspace_id is not a workspace", false);
    };
    if !valid_verifier(verifier) {
        return grant_error("invalid_grant", "the code verifier is malformed", false);
    }
    let name = grant.device_name.unwrap_or_default().trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return grant_error(
            "invalid_request",
            "name the device in 1 to 80 characters",
            false,
        );
    }
    let Some(platform) = grant
        .platform
        .filter(|platform| PLATFORMS.contains(platform))
    else {
        return grant_error(
            "invalid_request",
            "platform is one of ios, android and desktop",
            false,
        );
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let access = crate::auth::new_secret();
    let refresh = crate::auth::new_secret();
    let now = Utc::now();
    let challenge = challenge_of(verifier);
    match platform_db::register_device(
        pool,
        tenant_id,
        &platform_db::NewDevice {
            code_hash: &crate::auth::hash_secret(code),
            code_challenge: &challenge,
            redirect_uri,
            name,
            platform,
            access_hash: &crate::auth::hash_secret(&access),
            refresh_hash: &crate::auth::hash_secret(&refresh),
            access_expires_at: now + Duration::seconds(ACCESS_TTL_SECONDS),
            refresh_expires_at: now + Duration::seconds(REFRESH_TTL_SECONDS),
            correlation_id: Some(crate::correlation_id(request_id)),
        },
    )
    .await
    {
        Ok(device) => tokens_json(state, &access, &refresh, tenant_id, device.id),
        Err(platform_db::DeviceError::InvalidGrant) => grant_error(
            "invalid_grant",
            "the code is unknown, used, expired, or not for this verifier",
            false,
        ),
        // The same answer as a workspace that does not exist.
        Err(platform_db::DeviceError::NotMember) => grant_error(
            "invalid_grant",
            "the workspace is unavailable to this person",
            false,
        ),
        Err(platform_db::DeviceError::LimitReached) => grant_error(
            "invalid_grant",
            "revoke a device before signing in on another",
            false,
        ),
        Err(platform_db::DeviceError::Sqlx(error)) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to register a device");
            unavailable()
        }
    }
}

/// Replaces a refresh token with a new access and refresh token.
pub(crate) async fn refresh_token(
    state: &AppState,
    request_id: &RequestId,
    refresh: Option<&str>,
) -> Response {
    let Some(presented) = refresh.filter(|token| !token.is_empty()) else {
        return grant_error("invalid_request", "send refresh_token", false);
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let access = crate::auth::new_secret();
    let next = crate::auth::new_secret();
    let now = Utc::now();
    match platform_db::refresh_device_token(
        pool,
        &crate::auth::hash_secret(presented),
        &crate::auth::hash_secret(&access),
        &crate::auth::hash_secret(&next),
        now + Duration::seconds(ACCESS_TTL_SECONDS),
        now + Duration::seconds(REFRESH_TTL_SECONDS),
        Some(crate::correlation_id(request_id)),
    )
    .await
    {
        Ok(Some(platform_db::RefreshOutcome::Issued { tenant_id, device })) => {
            tokens_json(state, &access, &next, tenant_id, device)
        }
        Ok(Some(platform_db::RefreshOutcome::Wipe { .. })) => grant_error(
            "invalid_grant",
            "this device no longer has access to the workspace; delete its data",
            true,
        ),
        Ok(None) => grant_error(
            "invalid_grant",
            "the refresh token is unknown or expired",
            false,
        ),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to refresh a device token");
            unavailable()
        }
    }
}

// --- Devices -----------------------------------------------------------------

fn manages_devices(principal: &WorkspacePrincipal) -> bool {
    principal
        .workspace
        .permissions
        .contains(DEVICES_MANAGE_PERMISSION)
}

/// The caller's devices, or everyone's with `devices.manage`, newest first.
pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let owner = (!manages_devices(&principal)).then_some(principal.session.user.id);
    match platform_db::devices(pool, principal.workspace.tenant_id, owner).await {
        Ok(devices) => {
            let mut value = json!({
                "data": devices
                    .iter()
                    .map(|device| device_json(device, principal.device))
                    .collect::<Vec<_>>()
            });
            crate::users::name_record_users(&state, principal.workspace.tenant_id, &mut value)
                .await;
            Json(value).into_response()
        }
        Err(_) => unavailable(),
    }
}

async fn revoke_as(
    state: &AppState,
    principal: &WorkspacePrincipal,
    request_id: &RequestId,
    id: Uuid,
    owner: Option<Uuid>,
    reason: platform_db::RevokeReason,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::revoke_device(
        pool,
        principal.workspace.tenant_id,
        id,
        owner,
        principal.session.user.id,
        reason,
        Some(crate::correlation_id(request_id)),
    )
    .await
    {
        Ok(Some(device)) => {
            let mut value = device_json(&device, principal.device);
            crate::users::name_record_users(state, principal.workspace.tenant_id, &mut value).await;
            Json(value).into_response()
        }
        Ok(None) => not_found(),
        Err(_) => unavailable(),
    }
}

/// Revokes one of the caller's devices, or anyone's with `devices.manage`.
/// The device is told to wipe its replica on its next call.
pub(crate) async fn revoke(
    State(state): State<AppState>,
    Path((_workspace, id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let owner = (!manages_devices(&principal)).then_some(principal.session.user.id);
    revoke_as(
        &state,
        &principal,
        &request_id,
        id,
        owner,
        platform_db::RevokeReason::Revoked,
    )
    .await
}

/// The calling device's id: only its own access token reaches these routes.
fn current(principal: &WorkspacePrincipal) -> Option<Uuid> {
    principal.device
}

/// The calling device, its member and the workspace: what the app needs to
/// build its replica's engine, since `/me` and `/workspaces` belong to
/// browser sessions. Roles come as ids, for the engine's object
/// permissions, and as names, for the shell.
pub(crate) async fn current_device(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(device) = current(&principal) else {
        return not_found();
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let tenant_id = principal.workspace.tenant_id;
    let user = &principal.session.user;
    let Ok(devices) = platform_db::devices(pool, tenant_id, Some(user.id)).await else {
        return unavailable();
    };
    let Ok(role_ids) = platform_db::user_role_ids(pool, tenant_id, user.id).await else {
        return unavailable();
    };
    // A device token's principal carries no name; the membership has it.
    let Ok(Some(member)) = platform_db::workspace_member(pool, tenant_id, user.id).await else {
        return unavailable();
    };
    let Some(row) = devices.iter().find(|row| row.id == device) else {
        return not_found();
    };
    let mut value = json!({
        "device": device_json(row, Some(device)),
        "member": {
            "id": user.id,
            "display_name": member.display_name,
            "email": member.email,
            "permissions": principal.workspace.permissions,
            "roles": principal.workspace.roles,
            "role_ids": role_ids,
        },
        "workspace": {
            "id": tenant_id,
            "name": principal.workspace.display_name,
        },
    });
    crate::users::name_record_users(&state, tenant_id, &mut value).await;
    Json(value).into_response()
}

/// Signs the calling device out of the workspace.
pub(crate) async fn sign_out(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(device) = current(&principal) else {
        return not_found();
    };
    revoke_as(
        &state,
        &principal,
        &request_id,
        device,
        Some(principal.session.user.id),
        platform_db::RevokeReason::SignedOut,
    )
    .await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PushRequest {
    provider: String,
    token: String,
}

/// Sets where the calling device's content-free push notifications go.
pub(crate) async fn set_push(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
    body: Result<Json<PushRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(device) = current(&principal) else {
        return not_found();
    };
    let Ok(Json(request)) = body else {
        return invalid("validation.failed", "the request body is invalid");
    };
    if !PUSH_PROVIDERS.contains(&request.provider.as_str()) {
        return invalid("devices.push_provider_invalid", "provider is apns or fcm");
    }
    if request.token.is_empty() || request.token.len() > PUSH_TOKEN_MAX {
        return invalid(
            "devices.push_token_invalid",
            "send a push token of 1 to 4096 characters",
        );
    }
    update_push(
        &state,
        &principal,
        device,
        Some((&request.provider, &request.token)),
    )
    .await
}

/// Stops push notifications to the calling device.
pub(crate) async fn clear_push(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(device) = current(&principal) else {
        return not_found();
    };
    update_push(&state, &principal, device, None).await
}

async fn update_push(
    state: &AppState,
    principal: &WorkspacePrincipal,
    device: Uuid,
    push: Option<(&str, &str)>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::set_device_push(pool, principal.workspace.tenant_id, device, push).await {
        Ok(Some(device)) => {
            let mut value = device_json(&device, principal.device);
            crate::users::name_record_users(state, principal.workspace.tenant_id, &mut value).await;
            Json(value).into_response()
        }
        Ok(None) => not_found(),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::{challenge_of, valid_challenge, valid_verifier};

    #[test]
    fn pkce_challenges_follow_rfc_7636() {
        // The example from RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert!(valid_verifier(verifier));
        let challenge = challenge_of(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert!(valid_challenge(&challenge));
        assert!(!valid_verifier("too-short"));
        assert!(!valid_verifier(&"a".repeat(129)));
        assert!(!valid_verifier(&format!("{}!", "a".repeat(43))));
        assert!(!valid_challenge(
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-c="
        ));
    }
}
