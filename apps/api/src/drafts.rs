//! The server's part of record drafts (ADR 0036). Drafts stay in the
//! browser; the server hands each browser session a key to encrypt them
//! with, and tells the shell where drafts are kept and for how long.

use crate::{auth::SessionPrincipal, problem, AppState};
use axum::{
    extract::{Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Extension, Json,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

/// Drafts one member may keep per workspace in one browser (ADR 0036 §7).
pub(crate) const DRAFTS_PER_WORKSPACE_MAX: u32 = 50;
/// The largest draft document, before encryption.
pub(crate) const DRAFT_BYTES_MAX: u32 = 1_048_576;

#[derive(Debug, Deserialize)]
pub(crate) struct DraftKeyQuery {
    browser: Option<String>,
}

/// Refuses bearer-token callers before the session boundary runs: API
/// clients and devices never keep browser drafts, so they never receive a
/// draft key (`403 drafts.browser_only`).
pub(crate) async fn browser_only(headers: HeaderMap, request: Request, next: Next) -> Response {
    if headers.contains_key(header::AUTHORIZATION) {
        return problem(
            StatusCode::FORBIDDEN,
            "drafts.browser_only",
            "draft keys are issued only to browser sessions",
        )
        .into_response();
    }
    next.run(request).await
}

/// `GET /api/v1/me/draft-key?browser={id}`: the signed-in user's random
/// 256-bit key for this browser, created on first use. The response is
/// never cached and the key is never logged.
pub(crate) async fn draft_key(
    State(state): State<AppState>,
    Extension(principal): Extension<SessionPrincipal>,
    Query(query): Query<DraftKeyQuery>,
) -> Response {
    let Some(browser) = query
        .browser
        .as_deref()
        .and_then(|value| Uuid::parse_str(value).ok())
    else {
        return no_store(
            problem(
                StatusCode::BAD_REQUEST,
                "drafts.browser_invalid",
                "name this browser with its draft browser id",
            )
            .into_response(),
        );
    };
    let Some(pool) = state.pool.as_ref() else {
        return no_store(unavailable());
    };
    let fresh: [u8; platform_db::DRAFT_KEY_BYTES] = rand::random();
    match platform_db::draft_key(pool, principal.user.id, browser, &fresh).await {
        Ok(Some(key)) if key.len() == platform_db::DRAFT_KEY_BYTES => no_store(
            Json(json!({
                "browser": browser,
                "key": URL_SAFE_NO_PAD.encode(&key),
            }))
            .into_response(),
        ),
        Ok(Some(_)) => {
            tracing::error!("stored draft key has the wrong length");
            no_store(unavailable())
        }
        Ok(None) => no_store(
            problem(
                StatusCode::UNAUTHORIZED,
                "auth.unauthenticated",
                "authentication is required",
            )
            .into_response(),
        ),
        Err(error) => {
            tracing::error!(error_type = %std::any::type_name_of_val(&error), "failed to issue a draft key");
            no_store(unavailable())
        }
    }
}

fn no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert(header::PRAGMA, HeaderValue::from_static("no-cache"));
    response
}

fn unavailable() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "service temporarily unavailable",
    )
    .into_response()
}

/// The `drafts` block of the server's UI contributions (ADR 0036 §3): web
/// drafts live in the browser, with the workspace's expiry and the limits.
/// Without the expiry setting, the block is left out and the shell keeps no
/// drafts, as with a host that offers none.
pub(crate) fn contribution(expiry_days: Option<i32>) -> Option<Value> {
    expiry_days.map(|expiry_days| {
        json!({
            "store": "browser",
            "expiry_days": expiry_days,
            "max_drafts": DRAFTS_PER_WORKSPACE_MAX,
            "max_bytes": DRAFT_BYTES_MAX,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, routing::get, Router};
    use tower::ServiceExt;

    #[test]
    fn the_contribution_names_the_browser_store_and_the_limits() {
        let block = contribution(Some(7)).expect("a block");
        assert_eq!(block["store"], "browser");
        assert_eq!(block["expiry_days"], 7);
        assert_eq!(block["max_drafts"], 50);
        assert_eq!(block["max_bytes"], 1_048_576);
        assert!(contribution(None).is_none());
    }

    async fn reached() -> &'static str {
        "reached"
    }

    #[tokio::test]
    async fn bearer_callers_are_refused_before_the_session_boundary() {
        let app = Router::new()
            .route("/", get(reached))
            .route_layer(axum::middleware::from_fn(browser_only));
        let refused = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(header::AUTHORIZATION, "Bearer device-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(refused.into_body(), 4096)
            .await
            .expect("body");
        let body: Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(body["code"], "drafts.browser_only");
        let passed = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(passed.status(), StatusCode::OK);
    }

    /// The route against real PostgreSQL: browser sessions only, never
    /// cached, one key per browser, at most 20 per user, and gone after
    /// sign-out, deactivation or 90 days unused. Run with
    /// `bash scripts/test-plugin-runtime.sh`.
    #[tokio::test]
    #[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
    async fn draft_keys_against_postgres() {
        let connect = |variable: &str| {
            let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable}"));
            async move {
                sqlx::postgres::PgPoolOptions::new()
                    .max_connections(2)
                    .connect(&url)
                    .await
                    .expect("connect")
            }
        };
        let app_pool = connect("PLATFORM_TEST_DATABASE_URL").await;
        let migrator = connect("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
        let worker = connect("PLATFORM_TEST_WORKER_DATABASE_URL").await;
        let user = Uuid::now_v7();
        let token = format!("draft-key-{user}");
        sqlx::query("INSERT INTO users (id, display_name) VALUES ($1, 'Drafter')")
            .bind(user)
            .execute(&migrator)
            .await
            .expect("user");
        sqlx::query(
            "INSERT INTO sessions (id, token_hash, user_id, expires_at) \
             VALUES ($1, $2, $3, now() + interval '1 hour')",
        )
        .bind(Uuid::now_v7())
        .bind(crate::auth::hash_secret(&token))
        .bind(user)
        .execute(&migrator)
        .await
        .expect("session");
        let state = AppState {
            pool: Some(app_pool.clone()),
            ..AppState::default()
        };
        let app = crate::draft_key_routes(state.clone())
            .merge(Router::new().route("/auth/logout", axum::routing::post(crate::auth::logout)))
            .with_state(state);
        let cookie = format!("platform_session={token}");
        let call = |uri: String, cookie: Option<String>, bearer: bool, method: &'static str| {
            let app = app.clone();
            async move {
                let mut request = Request::builder().method(method).uri(uri);
                if let Some(cookie) = cookie {
                    request = request.header(header::COOKIE, cookie);
                }
                if bearer {
                    request = request.header(header::AUTHORIZATION, "Bearer api-client");
                }
                let response = app
                    .oneshot(request.body(Body::empty()).expect("request"))
                    .await
                    .expect("response");
                let status = response.status();
                let cache = response
                    .headers()
                    .get(header::CACHE_CONTROL)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                let body = axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .expect("body");
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                (status, cache, body)
            }
        };
        let browser = Uuid::new_v4();
        let uri = |browser: Uuid| format!("/api/v1/me/draft-key?browser={browser}");

        let (status, cache, first) = call(uri(browser), Some(cookie.clone()), false, "GET").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(cache.as_deref(), Some("no-store"));
        let key = URL_SAFE_NO_PAD
            .decode(first["key"].as_str().expect("key"))
            .expect("base64url");
        assert_eq!(key.len(), 32);
        let (_, _, again) = call(uri(browser), Some(cookie.clone()), false, "GET").await;
        assert_eq!(again["key"], first["key"], "one key per browser");

        let (status, _, refused) = call(uri(browser), Some(cookie.clone()), true, "GET").await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(refused["code"], "drafts.browser_only");
        let (status, _, _) = call(uri(browser), None, false, "GET").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, cache, invalid) = call(
            "/api/v1/me/draft-key?browser=not-a-browser".into(),
            Some(cookie.clone()),
            false,
            "GET",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(cache.as_deref(), Some("no-store"));
        assert_eq!(invalid["code"], "drafts.browser_invalid");

        // Twenty more browsers: the first, least recently used, goes.
        sqlx::query(
            "UPDATE draft_keys SET last_used_at = now() - interval '1 day' WHERE user_id = $1",
        )
        .bind(user)
        .execute(&migrator)
        .await
        .expect("age the first key");
        let mut browsers = Vec::new();
        for _ in 0..20 {
            let other = Uuid::new_v4();
            let (status, _, _) = call(uri(other), Some(cookie.clone()), false, "GET").await;
            assert_eq!(status, StatusCode::OK);
            browsers.push(other);
        }
        assert_eq!(
            platform_db::count_user_draft_keys(&app_pool, user)
                .await
                .expect("count"),
            20
        );
        let kept: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM draft_keys WHERE user_id = $1 AND browser_id = $2",
        )
        .bind(user)
        .bind(browser)
        .fetch_one(&migrator)
        .await
        .expect("first key");
        assert_eq!(kept, 0, "the least recently used key was deleted");

        // Unused for longer than 90 days: the worker's routine deletes it.
        let stale = browsers[0];
        sqlx::query(
            "UPDATE draft_keys SET last_used_at = now() - interval '91 days' \
             WHERE user_id = $1 AND browser_id = $2",
        )
        .bind(user)
        .bind(stale)
        .execute(&migrator)
        .await
        .expect("age a key");
        let purged = platform_db::purge_draft_keys(&worker, 90, 1000)
            .await
            .expect("purge");
        assert!(purged >= 1);
        assert_eq!(
            platform_db::count_user_draft_keys(&app_pool, user)
                .await
                .expect("count"),
            19
        );

        // Sign-out deletes this browser's key with the session.
        let signed_out = browsers[1];
        let (status, _, _) = call(
            format!("/auth/logout?browser={signed_out}"),
            Some(cookie.clone()),
            false,
            "POST",
        )
        .await;
        assert!(status.is_redirection(), "{status}");
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM draft_keys WHERE user_id = $1 AND browser_id = $2",
        )
        .bind(user)
        .bind(signed_out)
        .fetch_one(&migrator)
        .await
        .expect("signed-out key");
        assert_eq!(left, 0);
        let (status, _, _) = call(uri(browser), Some(cookie.clone()), false, "GET").await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "the session was revoked");

        // Deactivating the user deletes the rest.
        assert_eq!(
            platform_db::delete_user_draft_keys(&app_pool, user)
                .await
                .expect("delete"),
            18
        );
    }
}
