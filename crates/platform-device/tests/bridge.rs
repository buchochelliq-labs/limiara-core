//! The local command bridge end to end (ADR 0024 §5). The server is the
//! object routes the workspace API serves (`platform-objects-http`), over a
//! SQLite workspace and behind a stand-in for its authentication, on a local
//! port; the device syncs from it with the real HTTP transport, and the shell's
//! requests go through `Bridge::handle` with the same paths and bodies as
//! `apps/web/src/objects/api.ts`.

use async_trait::async_trait;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use platform_device::{Bridge, DeviceHost, Member, Workspace};
use platform_objects::store::sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::*;
use platform_objects_http::{Caller, ObjectsHost};
use platform_storage::sqlite::SqliteTenantDatabases;
use platform_sync::{AccessToken, HttpTransport, SyncClient, SyncError};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use uuid::Uuid;

const ACCOUNT: &str = "custom:account";

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

/// The server: every member's engine over one SQLite workspace.
#[derive(Clone)]
struct Server {
    store: Arc<dyn ObjectStore>,
    roles: Arc<BTreeMap<Uuid, Vec<Uuid>>>,
}

#[async_trait]
impl ObjectsHost for Server {
    async fn engine(&self, caller: &Caller) -> Result<ObjectService, Response> {
        let roles = self.roles.get(&caller.user_id).cloned().unwrap_or_default();
        Ok(ObjectService::new(
            self.store.clone(),
            ObjectCaller::new(
                caller.tenant_id,
                caller.user_id,
                caller.permissions.clone(),
                roles,
            ),
        ))
    }

    async fn name_users(&self, _tenant_id: Uuid, _value: &mut Value) {}

    async fn workspace_roles(&self, _tenant_id: Uuid) -> Result<BTreeMap<Uuid, String>, Response> {
        Ok(BTreeMap::new())
    }

    async fn member_names(&self, _tenant_id: Uuid, _users: &[Uuid]) -> BTreeMap<Uuid, String> {
        BTreeMap::new()
    }

    async fn is_member(&self, _tenant_id: Uuid, _user_id: Uuid) -> Result<bool, Response> {
        Ok(true)
    }
}

/// The access token is the member's id; the workspace is the path's.
async fn authenticate(mut request: Request, next: Next) -> Response {
    let user = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|value| Uuid::parse_str(value).ok());
    let tenant = request
        .uri()
        .path()
        .strip_prefix("/api/v1/workspaces/")
        .and_then(|rest| rest.split('/').next())
        .and_then(|value| Uuid::parse_str(value).ok());
    let (Some(user_id), Some(tenant_id)) = (user, tenant) else {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    };
    request.extensions_mut().insert(Caller {
        tenant_id,
        user_id,
        permissions: BTreeSet::new(),
    });
    next.run(request).await
}

struct Token(Uuid);

#[async_trait]
impl AccessToken for Token {
    async fn access_token(&self) -> Result<String, SyncError> {
        Ok(self.0.to_string())
    }
}

struct World {
    _directories: (tempfile::TempDir, tempfile::TempDir),
    workspace: Uuid,
    member_id: Uuid,
    colleague: Uuid,
    role: Uuid,
    admin: ObjectService,
    client: SyncClient<HttpTransport>,
    bridge: Bridge,
    device_files: Arc<SqliteTenantDatabases>,
}

impl World {
    async fn new() -> Self {
        let server_directory = tempfile::tempdir().expect("server dir");
        let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(Arc::new(
            SqliteTenantDatabases::new(server_directory.path(), SQLITE_MIGRATIONS),
        )));
        let workspace = Uuid::now_v7();
        let admin = ObjectService::new(
            store.clone(),
            ObjectCaller::new(
                workspace,
                Uuid::now_v7(),
                [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                Vec::new(),
            ),
        );
        let role = Uuid::now_v7();
        admin
            .create_object(
                CreateObject {
                    name: "account".into(),
                    label: "Account".into(),
                    plural_label: "Accounts".into(),
                    description: None,
                    name_label: None,
                    sharing_model: Some(SharingModel::Private),
                    grant_hierarchy: None,
                    track_history: None,
                    searchable: None,
                    in_navigation: Some(true),
                },
                context(),
            )
            .await
            .expect("object");
        admin
            .create_field(
                ACCOUNT,
                CreateField {
                    name: "notes".into(),
                    label: "Notes".into(),
                    field_type: FieldType::Text,
                    description: None,
                    required: false,
                    unique: false,
                    indexed: false,
                    searchable: false,
                    track_history: false,
                    max_length: None,
                    scale: None,
                    options: Vec::new(),
                    default_value: None,
                    reference_object: None,
                    delete_behaviour: None,
                    formula: None,
                    formula_type: None,
                    rollup: None,
                    lookup_filter: None,
                },
                context(),
            )
            .await
            .expect("field");
        admin
            .set_object_permission(
                ACCOUNT,
                role,
                AccessFlags {
                    read: true,
                    create: true,
                    edit: true,
                    delete: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("grant");
        let member_id = Uuid::now_v7();
        // The member owns one account on the server.
        ObjectService::new(
            store.clone(),
            ObjectCaller::new(workspace, member_id, BTreeSet::new(), vec![role]),
        )
        .create_record(
            ACCOUNT,
            json!({"name": "Acme"}).as_object().expect("values"),
            context(),
        )
        .await
        .expect("record");

        let server = Server {
            store,
            roles: Arc::new(BTreeMap::from([(member_id, vec![role])])),
        };
        // The member directory, as the API's `GET /users` and `/roles` list it.
        let colleague = Uuid::now_v7();
        let users = json!({"data": [
            {"id": member_id, "name": "Ada Lovelace", "manager_id": null},
            {"id": colleague, "name": "Grace Hopper", "manager_id": null},
        ], "can_manage": false});
        let roles = json!({"data": [{"id": role, "name": "Sales"}]});
        let app = platform_objects_http::routes::<Server>()
            .route(
                "/api/v1/workspaces/:workspace_id/users",
                axum::routing::get(move || async move { axum::Json(users) }),
            )
            .route(
                "/api/v1/workspaces/:workspace_id/roles",
                axum::routing::get(move || async move { axum::Json(roles) }),
            )
            .layer(middleware::from_fn(authenticate))
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .with_state(server);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });

        // The device: its own SQLite directory, whose workspace is the
        // server's.
        let device_directory = tempfile::tempdir().expect("device dir");
        // The replica is encrypted, as on a phone.
        let device_files = platform_device::device_databases(
            device_directory.path(),
            Arc::new(platform_device::MemoryKeys::new()),
        );
        let device_store: Arc<dyn ObjectStore> =
            Arc::new(SqliteObjectStore::new(device_files.clone()));
        let engine = ObjectService::new(
            device_store,
            ObjectCaller::new(workspace, member_id, BTreeSet::new(), vec![role]),
        )
        .with_replica()
        .with_client_ids();
        let transport = HttpTransport::new(
            format!("http://{address}"),
            workspace,
            Arc::new(Token(member_id)),
        )
        .expect("transport");
        let client = SyncClient::new(engine.clone(), transport);
        client
            .engine()
            .set_offline_limit(3_600)
            .await
            .expect("offline limit");
        client.sync_once(context()).await.expect("first sync");
        let bridge = Bridge::new(DeviceHost::new(
            engine,
            Member {
                display_name: "Ada Lovelace".into(),
                email: Some("ada@example.org".into()),
                // An administrator on the server; on the device only the
                // record changes that sync are open to them.
                permissions: [OBJECTS_MANAGE_PERMISSION, REPORTS_MANAGE_PERMISSION]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
                roles: BTreeSet::from(["Sales".to_owned()]),
            },
            Workspace {
                name: "Analytical Engines".into(),
            },
        ));
        Self {
            _directories: (server_directory, device_directory),
            workspace,
            member_id,
            colleague,
            role,
            admin,
            client,
            bridge,
            device_files,
        }
    }

    fn path(&self, rest: &str) -> String {
        format!("/api/v1/workspaces/{}{rest}", self.workspace)
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = http::Request::builder().method(method).uri(path);
        let bytes = match body {
            Some(body) => {
                request = request.header("content-type", "application/json");
                serde_json::to_vec(&body).expect("body")
            }
            None => Vec::new(),
        };
        let response = self
            .bridge
            .handle(request.body(bytes).expect("request"))
            .await;
        let status = response.status().as_u16();
        let value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).expect("JSON response")
        };
        (status, value)
    }
}

#[tokio::test]
async fn the_shell_reaches_the_replica_through_the_bridge() {
    let world = World::new().await;

    // The shell's first requests: who, which workspace, what navigation.
    let (status, me) = world.call("GET", "/api/v1/me", None).await;
    assert_eq!(status, 200, "{me}");
    assert_eq!(me["user"]["id"], json!(world.member_id));
    assert_eq!(me["user"]["display_name"], "Ada Lovelace");
    let (status, workspaces) = world.call("GET", "/api/v1/workspaces", None).await;
    assert_eq!(status, 200, "{workspaces}");
    assert_eq!(workspaces["data"][0]["id"], json!(world.workspace));
    assert_eq!(workspaces["data"][0]["name"], "Analytical Engines");
    let (status, ui) = world
        .call("GET", &world.path("/ui/contributions"), None)
        .await;
    assert_eq!(status, 200, "{ui}");
    let navigation: Vec<&str> = ui["navigation"]
        .as_array()
        .expect("navigation")
        .iter()
        .filter_map(|item| item["id"].as_str())
        .collect();
    assert!(navigation.contains(&"objects.custom:account"), "{ui}");
    assert!(ui["revision"].as_str().is_some(), "{ui}");
    assert_eq!(
        ui["permissions"],
        json!([]),
        "server-only actions hidden: {ui}"
    );

    // Definitions came down with the first sync.
    let (status, objects) = world.call("GET", &world.path("/objects"), None).await;
    assert_eq!(status, 200, "{objects}");
    assert!(objects["data"]
        .as_array()
        .expect("objects")
        .iter()
        .any(|object| object["api_name"] == ACCOUNT));
    let (status, account) = world
        .call("GET", &world.path("/objects/custom:account"), None)
        .await;
    assert_eq!(status, 200, "{account}");
    assert!(account["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .any(|field| field["api_name"] == "custom:notes"));

    // Records: the synced one, then one made on the device.
    let (status, page) = world
        .call("GET", &world.path("/objects/custom:account/records"), None)
        .await;
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["data"][0]["name"], "Acme");
    assert_eq!(page["data"][0]["owner"]["name"], "Ada Lovelace");

    // The offline directory the first sync took: members for the owner and
    // share pickers, roles for object permissions.
    let (status, users) = world.call("GET", &world.path("/users"), None).await;
    assert_eq!(status, 200, "{users}");
    let names: BTreeMap<String, String> = users["data"]
        .as_array()
        .expect("members")
        .iter()
        .map(|user| {
            (
                user["id"].as_str().unwrap_or_default().to_owned(),
                user["name"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        names[&world.colleague.to_string()],
        "Grace Hopper",
        "{users}"
    );
    assert_eq!(names[&world.member_id.to_string()], "Ada Lovelace");
    let (status, roles) = world.call("GET", &world.path("/roles"), None).await;
    assert_eq!(status, 200, "{roles}");
    assert_eq!(roles["data"], json!([{"id": world.role, "name": "Sales"}]));
    let (status, created) = world
        .call(
            "POST",
            &world.path("/objects/custom:account/records"),
            Some(json!({"name": "Globex"})),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    let record = world.path(&format!("/objects/custom:account/records/{id}"));
    let version = created["version"].as_i64().expect("version");
    let (status, updated) = world
        .call(
            "PATCH",
            &record,
            Some(json!({"expected_version": version, "custom:notes": "met at the fair"})),
        )
        .await;
    assert_eq!(status, 200, "{updated}");
    assert_eq!(updated["custom:notes"], "met at the fair");
    let (status, conflict) = world
        .call(
            "PATCH",
            &record,
            Some(json!({"expected_version": version, "custom:notes": "stale"})),
        )
        .await;
    assert_eq!(status, 409, "{conflict}");
    assert_eq!(conflict["code"], "records.version_conflict");
    let (status, queried) = world
        .call(
            "GET",
            &world.path(
                "/query?q=SELECT%20name%20FROM%20custom%3Aaccount%20WHERE%20name%20%3D%20%27Globex%27",
            ),
            None,
        )
        .await;
    assert_eq!(status, 200, "{queried}");
    assert_eq!(
        queried["data"].as_array().map(Vec::len),
        Some(1),
        "{queried}"
    );

    // Changes only the server makes are refused, not stranded on the
    // device.
    for (method, path, body) in [
        (
            "POST",
            "/objects",
            json!({"name": "lead", "label": "Lead", "plural_label": "Leads"}),
        ),
        (
            "POST",
            "/objects/custom:account/fields",
            json!({"name": "tier", "label": "Tier", "field_type": "text"}),
        ),
        (
            "POST",
            "/bulk",
            json!({"object": ACCOUNT, "operation": "insert"}),
        ),
        (
            "PUT",
            &format!(
                "/objects/custom:account/records/{id}/sharing/user/{}",
                Uuid::now_v7()
            ),
            json!({"access": "read"}),
        ),
    ] {
        let (status, refused) = world.call(method, &world.path(path), Some(body)).await;
        assert_eq!(status, 403, "{method} {path}: {refused}");
        assert_eq!(refused["code"], "sync.server_only", "{method} {path}");
    }
    let (status, objects) = world.call("GET", &world.path("/objects"), None).await;
    assert_eq!(
        objects["data"].as_array().map(Vec::len),
        Some(1),
        "{status} {objects}"
    );

    // Another workspace is concealed, as on the server; the change stream
    // is the server's.
    let (status, other) = world
        .call(
            "GET",
            &format!("/api/v1/workspaces/{}/objects", Uuid::now_v7()),
            None,
        )
        .await;
    assert_eq!(status, 404, "{other}");
    assert_eq!(other["code"], "workspace.not_found");
    let (status, stream) = world
        .call("GET", &world.path("/objects/changes/stream"), None)
        .await;
    assert_eq!(status, 404, "{stream}");

    // Nothing the member wrote is readable in the replica's files.
    let path = world.device_files.path_for(world.workspace);
    let mut on_disk = std::fs::read(&path).expect("replica file");
    let mut wal = path.into_os_string();
    wal.push("-wal");
    on_disk.extend(std::fs::read(&wal).unwrap_or_default());
    assert!(
        !on_disk
            .windows(b"met at the fair".len())
            .any(|window| window == b"met at the fair"),
        "plaintext in the replica"
    );

    // The edit made through the bridge reaches the server on the next sync.
    let report = world.client.sync_once(context()).await.expect("sync");
    assert!(
        report.pushed.iter().all(|pushed| pushed.failed.is_empty()),
        "{report:?}"
    );
    let on_server = world
        .admin
        .list_records(ACCOUNT, Default::default())
        .await
        .expect("server records")
        .records;
    let globex = on_server
        .iter()
        .find(|record| record.name == "Globex")
        .expect("pushed");
    assert_eq!(
        globex.values.get("custom:notes"),
        Some(&json!("met at the fair"))
    );
}

#[tokio::test]
async fn an_expired_lease_answers_as_the_server_would() {
    let world = World::new().await;
    world
        .client
        .engine()
        .record_contact_at(chrono::Utc::now() - chrono::Duration::hours(2))
        .await
        .expect("contact two hours ago");
    let (status, locked) = world
        .call("GET", &world.path("/objects/custom:account/records"), None)
        .await;
    assert_eq!(status, 409, "{locked}");
    assert_eq!(locked["code"], "sync.lease_expired");
    let (status, hidden) = world.call("GET", &world.path("/users"), None).await;
    assert_eq!(status, 409, "{hidden}");
    assert_eq!(hidden["code"], "sync.lease_expired");
    // The next sync that reaches the server unlocks it.
    world.client.sync_once(context()).await.expect("sync");
    let (status, page) = world
        .call("GET", &world.path("/objects/custom:account/records"), None)
        .await;
    assert_eq!(status, 200, "{page}");
}
