#![forbid(unsafe_code)]
//! The local command bridge (ADR 0024 §5): the workspace API's routes,
//! problem codes and JSON shapes, served in process over the engine on a
//! device. The app shell sends it the same requests it sends the server;
//! the native shell hands each one to [`Bridge::handle`] (from a custom URI
//! scheme, with no port and no network) and returns the response.
//!
//! The object routes are the server's own (`platform-objects-http`). The
//! bridge adds the three routes the shell needs before it reaches them, over
//! the device's own state: the signed-in member (`/api/v1/me`), the synced
//! workspace (`/api/v1/workspaces`) and its navigation
//! (`/api/v1/workspaces/{id}/ui/contributions`). Its `users` and `roles`
//! routes, and the names in record responses, come from the offline
//! directory each sync takes (ADR 0024, "Offline directory").
//!
//! Every request runs as the member the device signed in as; requests for
//! any other workspace answer `404 workspace.not_found`, as the server does
//! for a workspace the caller is not a member of. The change stream is the
//! server's alone: the device follows it with the sync client, so the bridge
//! answers `404 resource.unavailable` there.
//!
//! Only the writes the sync loop takes to the server run on the device:
//! record changes. Definitions, validation rules, quick actions,
//! automations, object permissions, sharing, saved reports and bulk jobs
//! are made on the server, and the bridge answers `403 sync.server_only`
//! for them. Running a quick action is a record change (ADR 0032 §3), and
//! the replica lists the actions it took with its definitions.

pub mod app;

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use platform_objects::{MutationSource, ObjectService, ReplicaDirectory};
use platform_objects_http::{name_users, referenced_users, Caller, ObjectsHost};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use uuid::Uuid;

pub use platform_storage::keys::{DatabaseKey, DatabaseKeys, KeyError, MemoryKeys};

/// The device's workspace files, encrypted with SQLCipher under keys from
/// the platform keystore (ADR 0024, "Encryption at rest"). The app opens the
/// replica's store over them, and calls
/// [`SqliteTenantDatabases::wipe`](platform_storage::sqlite::SqliteTenantDatabases::wipe)
/// when it signs out or the server answers `auth.device_wipe_required`,
/// which deletes the files and destroys the key.
pub fn device_databases(
    directory: impl Into<std::path::PathBuf>,
    keys: Arc<dyn DatabaseKeys>,
) -> Arc<platform_storage::sqlite::SqliteTenantDatabases> {
    Arc::new(
        platform_storage::sqlite::SqliteTenantDatabases::new(
            directory,
            platform_objects::store::sqlite::SQLITE_MIGRATIONS,
        )
        .with_keys(keys),
    )
}

/// The largest request or response body the bridge carries: the server's
/// bulk upload limit.
pub const BODY_LIMIT: usize = 16 * 1_048_576;

/// The member the device signed in as, from the token response and the
/// server's `/me` (ADR 0029).
#[derive(Debug, Clone)]
pub struct Member {
    pub display_name: String,
    pub email: Option<String>,
    /// The member's system permissions in the workspace, as the engine's
    /// caller holds them.
    pub permissions: BTreeSet<String>,
    /// The member's role names, as the server lists them.
    pub roles: BTreeSet<String>,
}

/// The workspace the device syncs.
#[derive(Debug, Clone)]
pub struct Workspace {
    pub name: String,
}

#[derive(Clone)]
struct Inner {
    engine: ObjectService,
    caller: Caller,
    member: Member,
    workspace: Workspace,
}

/// An [`ObjectsHost`] over the device's replica: every request gets the
/// engine the app built for the signed-in member.
#[derive(Clone)]
pub struct DeviceHost(Arc<Inner>);

impl DeviceHost {
    /// `engine` is the device's replica engine, built for the signed-in
    /// member with their permissions and roles (`with_replica`,
    /// `with_client_ids`); the member's permissions are the ones it holds.
    pub fn new(engine: ObjectService, member: Member, workspace: Workspace) -> Self {
        let caller = Caller {
            tenant_id: engine.caller().tenant_id(),
            user_id: engine.caller().user_id(),
            permissions: member.permissions.clone(),
            // Every change made through the bridge is the device's own
            // (#233); `with_device` names it once the device is signed in.
            source: MutationSource::device(None),
        };
        Self(Arc::new(Inner {
            engine,
            caller,
            member,
            workspace,
        }))
    }

    /// Names the signed-in device, as the server's token response did, as
    /// the source of the changes made through the bridge.
    pub fn with_device(mut self, device: Uuid) -> Self {
        Arc::make_mut(&mut self.0).caller.source = MutationSource::device(Some(device));
        self
    }

    /// The caller every request runs as.
    pub fn caller(&self) -> &Caller {
        &self.0.caller
    }

    fn me(&self) -> BTreeMap<Uuid, String> {
        BTreeMap::from([(self.0.caller.user_id, self.0.member.display_name.clone())])
    }

    /// The members and roles the last sync took (ADR 0024, "Offline
    /// directory"), with the signed-in member always present.
    async fn directory(&self) -> Result<ReplicaDirectory, Response> {
        // A locked replica shows the member nothing, its directory included.
        match self.0.engine.lease().await {
            Ok(lease) if lease.locked => {
                return Err(problem(
                    StatusCode::CONFLICT,
                    "sync.lease_expired",
                    "the device must reach the server before this workspace opens again",
                ))
            }
            Ok(_) => {}
            Err(_) => return Err(offline()),
        }
        let mut directory = self.0.engine.directory().await.map_err(|_| offline())?;
        directory
            .members
            .entry(self.0.caller.user_id)
            .or_insert_with(|| self.0.member.display_name.clone());
        Ok(directory)
    }
}

#[async_trait]
impl ObjectsHost for DeviceHost {
    async fn engine(&self, caller: &Caller) -> Result<ObjectService, Response> {
        if caller.tenant_id != self.0.caller.tenant_id || caller.user_id != self.0.caller.user_id {
            return Err(workspace_not_found());
        }
        Ok(self.0.engine.clone())
    }

    /// Names from the offline directory; without one, the member's own.
    async fn name_users(&self, _tenant_id: Uuid, value: &mut Value) {
        let names = match self.directory().await {
            Ok(directory) => directory.members,
            Err(_) => self.me(),
        };
        if referenced_users(value)
            .iter()
            .any(|user| names.contains_key(user))
        {
            name_users(value, &names);
        }
    }

    async fn workspace_roles(&self, _tenant_id: Uuid) -> Result<BTreeMap<Uuid, String>, Response> {
        Ok(self.directory().await?.roles)
    }

    async fn member_names(&self, _tenant_id: Uuid, users: &[Uuid]) -> BTreeMap<Uuid, String> {
        let mut names = match self.directory().await {
            Ok(directory) => directory.members,
            Err(_) => self.me(),
        };
        names.retain(|user, _| users.contains(user));
        names
    }

    /// Whether the last sync listed them; someone the server has since
    /// added shows up after the next.
    async fn is_member(&self, _tenant_id: Uuid, user_id: Uuid) -> Result<bool, Response> {
        Ok(self.directory().await?.members.contains_key(&user_id))
    }
}

fn problem(status: StatusCode, code: &str, detail: &str) -> Response {
    (
        status,
        Json(json!({
            "type": "about:blank",
            "title": status.canonical_reason().unwrap_or("error"),
            "status": status.as_u16(),
            "code": code,
            "detail": detail,
        })),
    )
        .into_response()
}

fn workspace_not_found() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "workspace.not_found",
        "workspace not found",
    )
}

/// The replica could not be read, answered as the server does when it is
/// unavailable, so the shell shows the same state.
fn offline() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "the service is temporarily unavailable",
    )
}

/// The workspace id in a `/api/v1/workspaces/{id}/...` path.
fn path_workspace(path: &str) -> Option<&str> {
    path.strip_prefix("/api/v1/workspaces/")?.split('/').next()
}

/// Whether a request is a read, or a write the sync loop takes to the
/// server: record changes, which the push queue sends as composite
/// operations (create, update, delete, undelete, owner change). Every other
/// write (definitions, validation rules, automations, object permissions,
/// sharing, saved reports, bulk jobs) is the server's to make: on the
/// device it would succeed locally, never reach the server, and be
/// overwritten by the next definitions refresh.
fn reaches_the_server(method: &Method, path: &str) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return true;
    }
    let Some(rest) = path
        .strip_prefix("/api/v1/workspaces/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(_, rest)| rest)
    else {
        return false;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    match (method, segments.as_slice()) {
        // Record operations, run here and pushed by the sync loop.
        (&Method::POST, ["composite"])
        | (&Method::POST, ["objects", _, "records"])
        | (&Method::PATCH | &Method::DELETE, ["objects", _, "records", _])
        | (&Method::POST, ["objects", _, "records", _, "undelete" | "owner"]) => true,
        // A read with a body: which held records the member may no longer see.
        (&Method::POST, ["sync", "revocations"]) => true,
        _ => false,
    }
}

fn server_only() -> Response {
    problem(
        StatusCode::FORBIDDEN,
        "sync.server_only",
        "this change is made on the server; it is not available on a synced device",
    )
}

/// The engine's permissions for changes only the server makes; the device
/// leaves them out of its contributions so the shell hides those actions.
const SERVER_ONLY_PERMISSIONS: [&str; 2] = [
    platform_objects::OBJECTS_MANAGE_PERMISSION,
    platform_objects::REPORTS_MANAGE_PERMISSION,
];

/// The bridge's counterpart of the server's workspace boundary: requests
/// for the synced workspace run as the signed-in member; others are
/// concealed.
async fn boundary(State(host): State<DeviceHost>, mut request: Request, next: Next) -> Response {
    let path = request.uri().path();
    if path.ends_with("/objects/changes/stream") {
        return problem(
            StatusCode::NOT_FOUND,
            "resource.unavailable",
            "the requested resource is unavailable",
        );
    }
    if let Some(workspace) = path_workspace(path) {
        if Uuid::parse_str(workspace).ok() != Some(host.caller().tenant_id) {
            return workspace_not_found();
        }
    }
    if !reaches_the_server(request.method(), path) {
        return server_only();
    }
    request.extensions_mut().insert(host.caller().clone());
    next.run(request).await
}

async fn me(State(host): State<DeviceHost>) -> Json<Value> {
    Json(json!({
        "user": {
            "id": host.0.caller.user_id,
            "display_name": host.0.member.display_name,
            "email": host.0.member.email,
        },
        // The device has no browser session; its access token is its own.
        "session_id": Value::Null,
    }))
}

/// `GET .../users` from the offline directory, as the server lists them for
/// owner and share pickers. Reporting lines and member management are the
/// server's.
async fn users(State(host): State<DeviceHost>) -> Response {
    match host.directory().await {
        Ok(directory) => Json(json!({
            "data": directory
                .members
                .iter()
                .map(|(id, name)| json!({"id": id, "name": name, "manager_id": Value::Null}))
                .collect::<Vec<_>>(),
            "can_manage": false,
        }))
        .into_response(),
        Err(response) => response,
    }
}

/// `GET .../roles` from the offline directory.
async fn roles(State(host): State<DeviceHost>) -> Response {
    match host.directory().await {
        Ok(directory) => Json(json!({
            "data": directory
                .roles
                .iter()
                .map(|(id, name)| json!({"id": id, "name": name}))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(response) => response,
    }
}

async fn workspaces(State(host): State<DeviceHost>) -> Json<Value> {
    Json(json!({
        "data": [{
            "id": host.0.caller.tenant_id,
            "name": host.0.workspace.name,
            "roles": host.0.member.roles,
        }]
    }))
}

/// Navigation and permissions from the engine, as the server's
/// contribution endpoint gives them; plugin pages are server-delivered and
/// absent offline.
async fn ui(State(host): State<DeviceHost>) -> Json<Value> {
    let (navigation, mut permissions) =
        platform_objects_http::ui_contributions(&host, host.caller()).await;
    permissions.retain(|permission| !SERVER_ONLY_PERMISSIONS.contains(&permission.as_str()));
    permissions.sort();
    permissions.dedup();
    let revision = format!(
        "device.{:016x}",
        stable_hash(&json!([navigation, permissions]).to_string())
    );
    Json(json!({
        "navigation": navigation,
        "permissions": permissions,
        "revision": revision,
    }))
}

/// FNV-1a, as the server's revision uses.
fn stable_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The bridge: the routes over a [`DeviceHost`], answered in process.
#[derive(Clone)]
pub struct Bridge {
    router: Router,
}

impl Bridge {
    pub fn new(host: DeviceHost) -> Self {
        let router = Router::new()
            .route("/api/v1/me", get(me))
            .route("/api/v1/workspaces", get(workspaces))
            .route("/api/v1/workspaces/:workspace_id/ui/contributions", get(ui))
            .route("/api/v1/workspaces/:workspace_id/users", get(users))
            .route("/api/v1/workspaces/:workspace_id/roles", get(roles))
            .merge(platform_objects_http::routes::<DeviceHost>())
            .layer(middleware::from_fn_with_state(host.clone(), boundary))
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .with_state(host);
        Self { router }
    }

    /// Answers one request as the server would.
    pub async fn handle(&self, request: http::Request<Vec<u8>>) -> http::Response<Vec<u8>> {
        let request = request.map(Body::from);
        let response = match self.router.clone().oneshot(request).await {
            Ok(response) => response,
            Err(never) => match never {},
        };
        let (parts, body) = response.into_parts();
        match axum::body::to_bytes(body, BODY_LIMIT).await {
            Ok(bytes) => http::Response::from_parts(parts, bytes.to_vec()),
            Err(_) => {
                let (parts, body) = offline().into_parts();
                let bytes = axum::body::to_bytes(body, BODY_LIMIT)
                    .await
                    .map(|bytes| bytes.to_vec())
                    .unwrap_or_default();
                http::Response::from_parts(parts, bytes)
            }
        }
    }
}
