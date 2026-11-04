//! API clients (ADR 0023 §3): OAuth 2.0 client-credential clients that let
//! an integration call the object engine as one workspace member without a
//! person signing in. Administrators create and revoke them; the token
//! endpoint exchanges a client's id and secret for a short-lived bearer
//! token. Secrets and tokens are random and stored only as hashes, and
//! nothing here is a password for people.

use crate::{auth::WorkspacePrincipal, problem, AppState};
use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Form, Json,
};
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::request_id::RequestId;
use uuid::Uuid;

/// Create and revoke API clients.
pub(crate) const API_CLIENTS_MANAGE_PERMISSION: &str = "api_clients.manage";
/// How long an access token lasts.
const TOKEN_TTL_SECONDS: i64 = 3600;
const NAME_MAX: usize = 80;

fn client_json(client: &platform_db::ApiClient) -> Value {
    json!({
        "id": client.id,
        "name": client.name,
        "client_id": client.client_id,
        "run_as": crate::users::user_reference(Some(client.run_as)),
        "created_by": crate::users::user_reference(Some(client.created_by)),
        "created_at": client.created_at,
        "last_used_at": client.last_used_at,
        "revoked_at": client.revoked_at,
    })
}

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

/// A workspace's API clients, newest first, without secrets.
pub(crate) async fn list(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::api_clients(pool, principal.workspace.tenant_id).await {
        Ok(clients) => {
            let mut value = json!({"data": clients.iter().map(client_json).collect::<Vec<_>>()});
            crate::users::name_record_users(&state, principal.workspace.tenant_id, &mut value)
                .await;
            Json(value).into_response()
        }
        Err(_) => unavailable(),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NewClient {
    name: String,
    /// The member the client acts as; the caller when omitted.
    run_as: Option<Uuid>,
}

/// Creates a client and returns its secret, the only time it is shown.
/// Acting as another member also needs `members.manage`.
pub(crate) async fn create(
    State(state): State<AppState>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<NewClient>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return invalid("validation.failed", "the request body is invalid");
    };
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return invalid(
            "api_clients.name_invalid",
            "name the client in 1 to 80 characters",
        );
    }
    let caller = principal.session.user.id;
    let run_as = request.run_as.unwrap_or(caller);
    if run_as != caller
        && !principal
            .workspace
            .permissions
            .contains(crate::users::MEMBERS_MANAGE_PERMISSION)
    {
        return problem(
            StatusCode::FORBIDDEN,
            "auth.permission_denied",
            "acting as another member needs members.manage",
        )
        .into_response();
    }
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let client_id = URL_SAFE_NO_PAD.encode(rand::random::<[u8; 16]>());
    let secret = crate::auth::new_secret();
    match platform_db::create_api_client(
        pool,
        principal.workspace.tenant_id,
        name,
        &client_id,
        &crate::auth::hash_secret(&secret),
        run_as,
        caller,
        Some(crate::correlation_id(&request_id)),
    )
    .await
    {
        Ok(client) => {
            let mut value = client_json(&client);
            value["client_secret"] = Value::String(secret);
            crate::users::name_record_users(&state, principal.workspace.tenant_id, &mut value)
                .await;
            let mut response = (StatusCode::CREATED, Json(value)).into_response();
            no_store(response.headers_mut());
            response
        }
        Err(platform_db::ApiClientError::NotMember) => invalid(
            "api_clients.run_as_invalid",
            "the client must act as a workspace member",
        ),
        Err(platform_db::ApiClientError::LimitReached) => problem(
            StatusCode::CONFLICT,
            "api_clients.limit_reached",
            "revoke a client before creating another",
        )
        .into_response(),
        Err(platform_db::ApiClientError::Sqlx(_)) => unavailable(),
    }
}

/// Revokes a client and every token it holds.
pub(crate) async fn revoke(
    State(state): State<AppState>,
    Path((_workspace, id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::revoke_api_client(
        pool,
        principal.workspace.tenant_id,
        id,
        principal.session.user.id,
        Some(crate::correlation_id(&request_id)),
    )
    .await
    {
        Ok(Some(client)) => {
            let mut value = client_json(&client);
            crate::users::name_record_users(&state, principal.workspace.tenant_id, &mut value)
                .await;
            Json(value).into_response()
        }
        Ok(None) => problem(
            StatusCode::NOT_FOUND,
            "resource.unavailable",
            "the requested resource is unavailable",
        )
        .into_response(),
        Err(_) => unavailable(),
    }
}

// --- Token endpoint (RFC 6749 §4.4) ------------------------------------------

#[derive(Debug, Deserialize)]
pub(crate) struct TokenRequest {
    grant_type: Option<String>,
    client_id: Option<String>,
    client_secret: Option<String>,
    // Device grants (ADR 0029).
    code: Option<String>,
    code_verifier: Option<String>,
    redirect_uri: Option<String>,
    workspace_id: Option<String>,
    device_name: Option<String>,
    platform: Option<String>,
    refresh_token: Option<String>,
}

fn no_store(headers: &mut HeaderMap) {
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
}

/// An OAuth error response: `400` for a malformed request, `401` with a
/// challenge for client authentication failures.
fn oauth_error(status: StatusCode, error: &str) -> Response {
    let mut response = (status, Json(json!({"error": error}))).into_response();
    no_store(response.headers_mut());
    if status == StatusCode::UNAUTHORIZED {
        response.headers_mut().insert(
            header::WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"api\""),
        );
    }
    response
}

/// Client credentials from HTTP Basic authentication.
fn basic_credentials(headers: &HeaderMap) -> Option<(String, String)> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, encoded) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = String::from_utf8(STANDARD.decode(encoded.trim()).ok()?).ok()?;
    let (id, secret) = decoded.split_once(':')?;
    Some((id.to_owned(), secret.to_owned()))
}

/// The token endpoint: an API client's credentials, sent with HTTP Basic
/// or in the form, for a bearer token for the object engine routes of its
/// workspace; or a device app's one-time code or refresh token (ADR 0029).
pub(crate) async fn token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(request_id): Extension<RequestId>,
    form: Result<Form<TokenRequest>, axum::extract::rejection::FormRejection>,
) -> Response {
    let Ok(Form(request)) = form else {
        return oauth_error(StatusCode::BAD_REQUEST, "invalid_request");
    };
    match request.grant_type.as_deref() {
        Some("client_credentials") => {}
        Some("authorization_code") => {
            return crate::devices::authorization_code(
                &state,
                &request_id,
                crate::devices::CodeGrant {
                    code: request.code.as_deref(),
                    code_verifier: request.code_verifier.as_deref(),
                    redirect_uri: request.redirect_uri.as_deref(),
                    workspace_id: request.workspace_id.as_deref(),
                    device_name: request.device_name.as_deref(),
                    platform: request.platform.as_deref(),
                },
            )
            .await;
        }
        Some("refresh_token") => {
            return crate::devices::refresh_token(
                &state,
                &request_id,
                request.refresh_token.as_deref(),
            )
            .await;
        }
        _ => return oauth_error(StatusCode::BAD_REQUEST, "unsupported_grant_type"),
    }
    let credentials = match (
        basic_credentials(&headers),
        request.client_id,
        request.client_secret,
    ) {
        (Some(basic), None, None) => basic,
        (None, Some(id), Some(secret)) => (id, secret),
        // Both, or neither, or half of the form pair.
        _ => return oauth_error(StatusCode::UNAUTHORIZED, "invalid_client"),
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let token = crate::auth::new_secret();
    let expires_at = Utc::now() + Duration::seconds(TOKEN_TTL_SECONDS);
    match platform_db::issue_api_token(
        pool,
        &credentials.0,
        &crate::auth::hash_secret(&credentials.1),
        &crate::auth::hash_secret(&token),
        expires_at,
        Some(crate::correlation_id(&request_id)),
    )
    .await
    {
        Ok(Some(issued)) => {
            let mut response = Json(json!({
                "access_token": token,
                "token_type": "Bearer",
                "expires_in": TOKEN_TTL_SECONDS,
                "workspace_id": issued.tenant_id,
            }))
            .into_response();
            no_store(response.headers_mut());
            response
        }
        Ok(None) => oauth_error(StatusCode::UNAUTHORIZED, "invalid_client"),
        Err(_) => unavailable(),
    }
}

#[cfg(test)]
mod tests {
    use super::basic_credentials;
    use axum::http::{header, HeaderMap, HeaderValue};
    use base64::{engine::general_purpose::STANDARD, Engine};

    #[test]
    fn basic_credentials_split_on_the_first_colon() {
        let mut headers = HeaderMap::new();
        let encoded = STANDARD.encode("client:sec:ret");
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {encoded}")).expect("header"),
        );
        assert_eq!(
            basic_credentials(&headers),
            Some(("client".to_owned(), "sec:ret".to_owned()))
        );
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer abc"),
        );
        assert_eq!(basic_credentials(&headers), None);
    }
}
