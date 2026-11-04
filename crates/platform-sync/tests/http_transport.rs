//! The HTTP transport against a stand-in for the workspace API on a
//! loopback port: paths, the device's bearer token, and how problem
//! answers map to sync errors.

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use platform_objects::composite::{CompositeOperation, CompositeRequest};
use platform_sync::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

struct Token;

#[async_trait]
impl AccessToken for Token {
    async fn access_token(&self) -> Result<String, SyncError> {
        Ok("device-token".into())
    }
}

#[derive(Default)]
struct Seen {
    calls: Mutex<Vec<String>>,
}

type Shared = Arc<Seen>;

fn authorised(headers: &HeaderMap) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        == Some("Bearer device-token")
}

fn problem(status: StatusCode, code: &str) -> Response {
    (
        status,
        Json(json!({"status": status.as_u16(), "code": code})),
    )
        .into_response()
}

fn record(id: &str) -> Value {
    json!({
        "id": id, "object": "custom:deal", "name": "Deal",
        "owner": null, "created_at": "2026-11-01T10:00:00Z", "created_by": null,
        "updated_at": "2026-11-01T10:00:00Z", "updated_by": null,
        "touched_at": "2026-11-01T10:00:00Z", "version": 2, "archived_at": null,
        "is_deleted": false, "deleted_at": null, "custom:stage": "won", "related": {}
    })
}

async fn objects(State(seen): State<Shared>, headers: HeaderMap) -> Response {
    seen.calls.lock().expect("calls").push("objects".into());
    if !authorised(&headers) {
        return problem(StatusCode::UNAUTHORIZED, "auth.unauthenticated");
    }
    Json(json!({"data": [{"api_name": "custom:deal", "access": {"read": true}}], "can_manage": false}))
        .into_response()
}

async fn rules(Path((_, object)): Path<(Uuid, String)>) -> Response {
    assert_eq!(object, "custom:deal");
    problem(StatusCode::FORBIDDEN, "auth.permission_denied")
}

async fn one_record(
    Path((_, object, id)): Path<(Uuid, String, String)>,
    Query(query): Query<BTreeMap<String, String>>,
) -> Response {
    assert_eq!(object, "custom:deal");
    assert_eq!(
        query.get("include_deleted").map(String::as_str),
        Some("true")
    );
    if id == "gone" {
        return problem(StatusCode::NOT_FOUND, "resource.unavailable");
    }
    Json(record(&id)).into_response()
}

async fn changes(Query(query): Query<BTreeMap<String, String>>) -> Response {
    match query.get("after").map(String::as_str) {
        Some("1") => problem(StatusCode::GONE, "changes.position_expired"),
        Some("2") => problem(StatusCode::UNAUTHORIZED, "auth.device_wipe_required"),
        _ => Json(json!({
            "data": [{"position": 7, "object": "custom:deal", "record": "a0B1", "kind": "updated",
                      "fields": ["custom:stage"], "changed_at": "2026-11-01T10:00:00Z",
                      "changed_by": null, "correlation_id": null}],
            "next_after": 7, "head": 9, "metadata_version": 4
        }))
        .into_response(),
    }
}

async fn stream(Query(query): Query<BTreeMap<String, String>>) -> Response {
    use axum::response::sse::{Event, Sse};
    if query.get("after").map(String::as_str) == Some("1") {
        return problem(StatusCode::GONE, "changes.position_expired");
    }
    assert_eq!(query.get("after").map(String::as_str), Some("6"));
    let events = vec![
        Event::default().comment("keep-alive"),
        Event::default()
            .event("metadata")
            .data(json!({"metadata_version": 4}).to_string()),
        Event::default().event("change").id("7").data(
            json!({"position": 7, "object": "custom:deal", "record": "a0B1", "kind": "updated",
                   "fields": [], "changed_at": "2026-11-01T10:00:00Z", "changed_by": null,
                   "correlation_id": null})
            .to_string(),
        ),
        Event::default()
            .event("expired")
            .data(json!({"code": "changes.position_expired"}).to_string()),
    ];
    Sse::new(futures_util::stream::iter(
        events.into_iter().map(Ok::<_, std::convert::Infallible>),
    ))
    .into_response()
}

async fn composite(Json(request): Json<Value>) -> Response {
    assert_eq!(request["all_or_none"], json!(false));
    assert_eq!(request["operations"][0]["method"], json!("update"));
    Json(json!({"results": [
        {"reference": "r0", "status": 200, "body": record("a0B1")},
        {"reference": "r1", "status": 409, "body": {"code": "records.version_conflict"}}
    ]}))
    .into_response()
}

async fn revocations(Json(request): Json<Value>) -> Response {
    assert_eq!(
        request,
        json!({"records": {"custom:deal": ["a0B1", "a0B2"]}})
    );
    Json(json!({"revoked": {"custom:deal": ["a0B2"]}})).into_response()
}

async fn transport() -> (HttpTransport, Shared) {
    let seen = Shared::default();
    let base = "/api/v1/workspaces/:workspace";
    let app = Router::new()
        .route(&format!("{base}/objects"), get(objects))
        .route(
            &format!("{base}/objects/:object/validation-rules"),
            get(rules),
        )
        .route(
            &format!("{base}/objects/:object/records/:id"),
            get(one_record),
        )
        .route(&format!("{base}/objects/changes"), get(changes))
        .route(&format!("{base}/objects/changes/stream"), get(stream))
        .route(&format!("{base}/composite"), post(composite))
        .route(&format!("{base}/sync/revocations"), post(revocations))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    let transport = HttpTransport::new(
        format!("http://{address}/"),
        Uuid::now_v7(),
        Arc::new(Token),
    )
    .expect("transport");
    (transport, seen)
}

#[tokio::test]
async fn the_transport_speaks_the_workspace_api() {
    let (transport, seen) = transport().await;

    let objects = transport.objects().await.expect("objects");
    assert_eq!(objects[0]["api_name"], json!("custom:deal"));
    assert_eq!(seen.calls.lock().expect("calls").len(), 1);

    // Rules the member may not see leave the replica's as they are.
    assert_eq!(
        transport
            .validation_rules("custom:deal")
            .await
            .expect("rules"),
        None
    );

    let found = transport
        .record("custom:deal", "a0B1")
        .await
        .expect("record")
        .expect("visible");
    assert_eq!(found["custom:stage"], json!("won"));
    assert_eq!(
        transport.record("custom:deal", "gone").await.expect("gone"),
        None
    );

    let page = transport.changes(None, 200).await.expect("page");
    assert_eq!(page.entries[0].record, "a0B1");
    assert_eq!(
        (page.next_after, page.head, page.metadata_version),
        (7, 9, 4)
    );
    assert!(matches!(
        transport.changes(Some(1), 200).await,
        Err(SyncError::PositionExpired)
    ));
    assert!(matches!(
        transport.changes(Some(2), 200).await,
        Err(SyncError::WipeRequired)
    ));

    let results = transport
        .composite(&CompositeRequest {
            all_or_none: false,
            operations: vec![CompositeOperation::Update {
                reference: "r0".into(),
                object: "custom:deal".into(),
                id: "a0B1".into(),
                expected_version: 1,
                values: serde_json::Map::new(),
            }],
        })
        .await
        .expect("composite");
    assert_eq!(results[0].status, 200);
    assert_eq!(results[1].body["code"], json!("records.version_conflict"));

    let revoked = transport
        .revocations(&BTreeMap::from([(
            "custom:deal".to_owned(),
            vec!["a0B1".to_owned(), "a0B2".to_owned()],
        )]))
        .await
        .expect("revocations");
    assert_eq!(revoked.get("custom:deal"), Some(&vec!["a0B2".to_owned()]));
}

#[tokio::test]
async fn the_transport_reads_the_change_stream() {
    use futures_util::StreamExt;
    let (transport, _) = transport().await;
    let events: Vec<StreamEvent> = transport
        .stream(Some(6))
        .await
        .expect("stream")
        .map(|event| event.expect("event"))
        .collect()
        .await;
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[0], StreamEvent::Metadata(4));
    assert!(matches!(
        &events[1],
        StreamEvent::Change(ChangeEntry { position: 7, record, .. }) if record == "a0B1"
    ));
    assert_eq!(events[2], StreamEvent::Expired);
    assert!(matches!(
        transport.stream(Some(1)).await,
        Err(SyncError::PositionExpired)
    ));
}

struct WrongToken;

#[async_trait]
impl AccessToken for WrongToken {
    async fn access_token(&self) -> Result<String, SyncError> {
        Ok("stale".into())
    }
}

#[tokio::test]
async fn a_refused_token_asks_the_app_to_refresh() {
    let (transport, _) = transport().await;
    // The same stand-in, with a token it does not accept.
    let stale = HttpTransport::new(transport.base(), Uuid::now_v7(), Arc::new(WrongToken))
        .expect("transport");
    assert!(matches!(
        stale.objects().await,
        Err(SyncError::Unauthorized)
    ));
}
