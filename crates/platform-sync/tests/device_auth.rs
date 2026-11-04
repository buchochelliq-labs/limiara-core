//! The app's side of ADR 0029 against a stand-in token endpoint: codes are
//! redeemed with the verifier, refresh tokens rotate exactly once however
//! many callers want a token, and a refused refresh says whether to wipe.

use async_trait::async_trait;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use platform_sync::{
    AccessToken, AuthError, DeviceSession, Registration, SignIn, SyncError, TokenClient, TokenSink,
    Tokens,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use url::Url;
use uuid::Uuid;

const WORKSPACE: Uuid = Uuid::from_u128(1);
const DEVICE: Uuid = Uuid::from_u128(2);

#[derive(Default)]
struct Server {
    /// The challenge the code was issued for.
    challenge: Mutex<Option<String>>,
    /// The refresh token that is valid now, and every one spent.
    refresh: Mutex<(String, BTreeSet<String>)>,
    refreshes: Mutex<usize>,
    /// Refresh tokens the server answers with `"wipe": true`.
    revoked: Mutex<bool>,
}

fn tokens(n: usize) -> Value {
    json!({
        "access_token": format!("access-{n}"),
        "token_type": "Bearer",
        "expires_in": 3600,
        "refresh_token": format!("refresh-{n}"),
        "workspace_id": WORKSPACE,
        "device_id": DEVICE,
        "offline_limit_seconds": 1_209_600,
    })
}

fn refused(wipe: bool) -> (axum::http::StatusCode, Json<Value>) {
    let mut body = json!({"error": "invalid_grant", "error_description": "no"});
    if wipe {
        body["wipe"] = Value::Bool(true);
    }
    (axum::http::StatusCode::BAD_REQUEST, Json(body))
}

async fn token(
    State(server): State<Arc<Server>>,
    Form(form): Form<BTreeMap<String, String>>,
) -> (axum::http::StatusCode, Json<Value>) {
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let verifier = form.get("code_verifier").cloned().unwrap_or_default();
            let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
            let expected = server.challenge.lock().expect("lock").take();
            if form.get("code").map(String::as_str) != Some("one-time")
                || expected.as_deref() != Some(challenge.as_str())
                || form.get("workspace_id") != Some(&WORKSPACE.to_string())
                || form.get("platform").map(String::as_str) != Some("desktop")
            {
                return refused(false);
            }
            *server.refresh.lock().expect("lock") = ("refresh-0".into(), BTreeSet::new());
            (axum::http::StatusCode::OK, Json(tokens(0)))
        }
        Some("refresh_token") => {
            if *server.revoked.lock().expect("lock") {
                return refused(true);
            }
            let presented = form.get("refresh_token").cloned().unwrap_or_default();
            let mut refresh = server.refresh.lock().expect("lock");
            if refresh.1.contains(&presented) {
                // A spent token presented again revokes the device.
                *server.revoked.lock().expect("lock") = true;
                return refused(true);
            }
            if presented != refresh.0 {
                return refused(false);
            }
            let mut count = server.refreshes.lock().expect("lock");
            *count += 1;
            refresh.1.insert(presented);
            refresh.0 = format!("refresh-{}", *count);
            (axum::http::StatusCode::OK, Json(tokens(*count)))
        }
        _ => refused(false),
    }
}

async fn current() -> Json<Value> {
    Json(json!({
        "device": {"id": DEVICE, "name": "Laptop", "current": true},
        "member": {
            "id": Uuid::from_u128(3),
            "display_name": "Ada",
            "email": "ada@example.com",
            "permissions": ["objects.manage"],
            "roles": ["Engineer"],
            "role_ids": [Uuid::from_u128(4)],
        },
        "workspace": {"id": WORKSPACE, "name": "Engines"},
    }))
}

async fn serve(server: Arc<Server>) -> String {
    let app = Router::new()
        .route("/oauth/token", post(token))
        .route(
            "/api/v1/workspaces/:workspace/devices/current",
            get(current),
        )
        .with_state(server);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    format!("http://{address}")
}

#[derive(Default)]
struct Kept(Mutex<Vec<Tokens>>);

#[async_trait]
impl TokenSink for Kept {
    async fn rotated(&self, tokens: &Tokens) -> Result<(), AuthError> {
        self.0.lock().expect("lock").push(tokens.clone());
        Ok(())
    }
}

fn registration() -> Registration {
    Registration {
        workspace: WORKSPACE,
        device_name: "Ada's laptop".into(),
        platform: "desktop".into(),
    }
}

async fn signed_in(server: &Arc<Server>, base: &str) -> (TokenClient, Tokens) {
    let sign_in = SignIn::start(base, "http://127.0.0.1:47185/callback").expect("sign-in");
    let browser = Url::parse(&sign_in.browser_url()).expect("browser url");
    let challenge = browser
        .query_pairs()
        .find(|(key, _)| key == "code_challenge")
        .map(|(_, value)| value.into_owned());
    *server.challenge.lock().expect("lock") = challenge;
    let state = browser
        .query_pairs()
        .find(|(key, _)| key == "state")
        .map(|(_, value)| value.into_owned())
        .expect("state");
    let redirect = Url::parse(&format!(
        "http://127.0.0.1:47185/callback?code=one-time&state={state}"
    ))
    .expect("redirect");
    let code = sign_in.code(&redirect).expect("code");
    let client = TokenClient::new(base).expect("client");
    let tokens = client
        .redeem(&sign_in, &code, &registration())
        .await
        .expect("redeem");
    (client, tokens)
}

#[tokio::test]
async fn a_code_is_redeemed_with_the_verifier_and_the_profile_follows() {
    let server = Arc::new(Server::default());
    let base = serve(server.clone()).await;
    let (client, tokens) = signed_in(&server, &base).await;
    assert_eq!(tokens.workspace_id, WORKSPACE);
    assert_eq!(tokens.offline_limit_seconds, 1_209_600);
    assert!(!format!("{tokens:?}").contains("refresh-0"));

    let profile = client
        .profile(&tokens.access_token, WORKSPACE)
        .await
        .expect("profile");
    assert_eq!(profile.device_id, DEVICE);
    assert_eq!(profile.member.display_name, "Ada");
    assert_eq!(profile.member.role_ids, vec![Uuid::from_u128(4)]);
    assert_eq!(profile.workspace.name, "Engines");

    // Another app's verifier gets nothing for the code.
    let other = SignIn::start(&base, "http://127.0.0.1:47185/callback").expect("sign-in");
    *server.challenge.lock().expect("lock") = Some("not-this-one".into());
    assert!(matches!(
        client.redeem(&other, "one-time", &registration()).await,
        Err(AuthError::InvalidGrant(_))
    ));
}

#[tokio::test]
async fn concurrent_callers_rotate_the_refresh_token_once() {
    let server = Arc::new(Server::default());
    let base = serve(server.clone()).await;
    let (client, tokens) = signed_in(&server, &base).await;
    let kept = Arc::new(Kept::default());
    // A resumed session has no access token yet: every caller needs one.
    let session = Arc::new(DeviceSession::resume(
        client,
        WORKSPACE,
        tokens.refresh_token.clone(),
        kept.clone(),
    ));
    let calls = (0..8).map(|_| {
        let session = session.clone();
        tokio::spawn(async move { session.access_token().await })
    });
    for call in calls {
        assert_eq!(call.await.expect("join").expect("token"), "access-1");
    }
    assert_eq!(*server.refreshes.lock().expect("lock"), 1);
    assert!(
        !*server.revoked.lock().expect("lock"),
        "a spent token was sent again"
    );
    let stored = kept.0.lock().expect("lock").clone();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].refresh_token, "refresh-1");

    // A forced refresh presents the new token, not the spent one.
    assert_eq!(session.refresh().await.expect("refresh"), "access-2");
    assert!(!*server.revoked.lock().expect("lock"));
}

#[tokio::test]
async fn a_revoked_device_is_told_to_wipe() {
    let server = Arc::new(Server::default());
    let base = serve(server.clone()).await;
    let (client, tokens) = signed_in(&server, &base).await;
    *server.revoked.lock().expect("lock") = true;
    let session = DeviceSession::resume(
        client.clone(),
        WORKSPACE,
        tokens.refresh_token.clone(),
        Arc::new(Kept::default()),
    );
    assert!(matches!(
        session.access().await,
        Err(AuthError::WipeRequired)
    ));
    assert!(matches!(
        session.access_token().await,
        Err(SyncError::WipeRequired)
    ));

    // An unknown refresh token is a sign-in again, not a wipe.
    *server.revoked.lock().expect("lock") = false;
    assert!(matches!(
        client.refresh("unknown").await,
        Err(AuthError::InvalidGrant(_))
    ));
}
