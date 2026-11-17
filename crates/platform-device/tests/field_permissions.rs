//! Field-level permissions over real HTTP (G5 of ADR 0035): the object
//! routes the workspace API serves, over a SQLite workspace on a local port,
//! answer each member with only the fields they may read and refuse changes
//! to fields they may not write; a device syncing as a member never
//! receives a hidden value, and its replica hides the field locally too.

use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use platform_objects::store::sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::*;
use platform_objects_http::{Caller, ObjectsHost};
use platform_storage::sqlite::SqliteTenantDatabases;
use platform_sync::{AccessToken, HttpTransport, SyncClient, SyncError};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower::ServiceExt;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use uuid::Uuid;

const ACCOUNT: &str = "custom:account";
const READ: &str = "hr.salary.read";
const WRITE: &str = "hr.salary.write";

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

type Permissions = Arc<BTreeMap<Uuid, BTreeSet<String>>>;

#[derive(Clone)]
struct Server {
    store: Arc<dyn ObjectStore>,
    role: Uuid,
}

#[async_trait]
impl ObjectsHost for Server {
    async fn engine(&self, caller: &Caller) -> Result<ObjectService, Response> {
        Ok(ObjectService::new(
            self.store.clone(),
            ObjectCaller::new(
                caller.tenant_id,
                caller.user_id,
                caller.permissions.clone(),
                vec![self.role],
            ),
        ))
    }

    async fn name_users(&self, _tenant_id: Uuid, _value: &mut Value) {}

    async fn workspace_roles(&self, _tenant_id: Uuid) -> Result<BTreeMap<Uuid, String>, Response> {
        Ok(BTreeMap::from([(self.role, "Sales".to_owned())]))
    }

    async fn member_names(&self, _tenant_id: Uuid, _users: &[Uuid]) -> BTreeMap<Uuid, String> {
        BTreeMap::new()
    }

    async fn is_member(&self, _tenant_id: Uuid, _user_id: Uuid) -> Result<bool, Response> {
        Ok(true)
    }

    async fn permission_catalog(
        &self,
        _tenant_id: Uuid,
    ) -> Result<Option<BTreeMap<String, String>>, Response> {
        Ok(Some(BTreeMap::from([
            (READ.to_owned(), "Read salaries".to_owned()),
            (WRITE.to_owned(), "Change salaries".to_owned()),
            (
                OBJECTS_MANAGE_PERMISSION.to_owned(),
                "Manage objects".to_owned(),
            ),
        ])))
    }
}

/// The access token is the member's id, and their permissions are the
/// test's; the workspace is the path's.
async fn authenticate(
    State(permissions): State<Permissions>,
    mut request: Request,
    next: Next,
) -> Response {
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
        permissions: permissions.get(&user_id).cloned().unwrap_or_default(),
        source: platform_objects::MutationSource::device(None),
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
    _directory: tempfile::TempDir,
    workspace: Uuid,
    role: Uuid,
    admin: Uuid,
    member: Uuid,
    writer: Uuid,
    app: axum::Router,
    address: std::net::SocketAddr,
}

impl World {
    async fn new() -> Self {
        let directory = tempfile::tempdir().expect("server dir");
        let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(Arc::new(
            SqliteTenantDatabases::new(directory.path(), SQLITE_MIGRATIONS),
        )));
        let workspace = Uuid::now_v7();
        let role = Uuid::now_v7();
        let (admin, member, writer) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let set = |values: &[&str]| -> BTreeSet<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        };
        // The member manages objects, so definitions reach their device with
        // every field, but holds neither salary permission.
        let permissions: Permissions = Arc::new(BTreeMap::from([
            (
                admin,
                set(&[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]),
            ),
            (member, set(&[OBJECTS_MANAGE_PERMISSION])),
            (writer, set(&[READ, WRITE])),
        ]));
        let users = json!({"data": [
            {"id": member, "name": "Ada Lovelace", "manager_id": null},
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
            .layer(middleware::from_fn_with_state(permissions, authenticate))
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .with_state(Server { store, role });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port");
        let address = listener.local_addr().expect("address");
        let served = app.clone();
        tokio::spawn(async move { axum::serve(listener, served).await });
        Self {
            _directory: directory,
            workspace,
            role,
            admin,
            member,
            writer,
            app,
            address,
        }
    }

    fn path(&self, rest: &str) -> String {
        format!("/api/v1/workspaces/{}{rest}", self.workspace)
    }

    async fn call(
        &self,
        user: Uuid,
        method: &str,
        rest: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = http::Request::builder()
            .method(method)
            .uri(self.path(rest))
            .header("authorization", format!("Bearer {user}"));
        let bytes = match body {
            Some(body) => {
                request = request.header("content-type", "application/json");
                serde_json::to_vec(&body).expect("body")
            }
            None => Vec::new(),
        };
        let response = self
            .app
            .clone()
            .oneshot(
                request
                    .body(axum::body::Body::from(bytes))
                    .expect("request"),
            )
            .await
            .expect("response");
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .expect("body");
        let value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).expect("JSON response")
        };
        (status, value)
    }

    /// The account object with notes and a salary only the writer reads
    /// and writes, and one account whose salary the writer set.
    async fn accounts(&self) -> String {
        let (status, _) = self
            .call(
                self.admin,
                "POST",
                "/objects",
                Some(json!({"name": "account", "label": "Account",
                            "plural_label": "Accounts"})),
            )
            .await;
        assert_eq!(status, 201);
        let (status, created) = self
            .call(
                self.admin,
                "POST",
                "/objects/custom:account/fields",
                Some(
                    json!({"name": "salary", "label": "Salary", "field_type": "number",
                            "read_permission": READ, "write_permission": WRITE}),
                ),
            )
            .await;
        assert_eq!(status, 201, "{created}");
        assert_eq!(created["read_permission"], json!(READ));
        assert_eq!(created["write_permission"], json!(WRITE));
        let (status, _) = self
            .call(
                self.admin,
                "POST",
                "/objects/custom:account/fields",
                Some(json!({"name": "notes", "label": "Notes", "field_type": "text"})),
            )
            .await;
        assert_eq!(status, 201);
        let (status, _) = self
            .call(
                self.admin,
                "PUT",
                &format!("/objects/custom:account/permissions/{}", self.role),
                Some(
                    json!({"read": true, "create": true, "edit": true, "delete": true,
                            "view_all": true, "modify_all": true}),
                ),
            )
            .await;
        assert_eq!(status, 200);
        let (status, record) = self
            .call(
                self.writer,
                "POST",
                "/objects/custom:account/records",
                Some(json!({"name": "Acme", "custom:salary": 100, "custom:notes": "hello"})),
            )
            .await;
        assert_eq!(status, 201, "{record}");
        record["id"].as_str().expect("id").to_owned()
    }
}

#[tokio::test]
async fn the_routes_hide_and_refuse_fields() {
    let world = World::new().await;

    // The pickers' catalog is for administrators.
    let (status, catalog) = world
        .call(world.admin, "GET", "/field-permissions", None)
        .await;
    assert_eq!(status, 200);
    assert!(catalog["data"]
        .as_array()
        .expect("data")
        .iter()
        .any(|entry| entry["id"] == json!(READ)));
    let (status, _) = world
        .call(world.writer, "GET", "/field-permissions", None)
        .await;
    assert_eq!(status, 403);

    let id = world.accounts().await;

    // A permission outside the catalog, or malformed, is refused.
    let (status, problem) = world
        .call(
            world.admin,
            "POST",
            "/objects/custom:account/fields",
            Some(
                json!({"name": "bonus", "label": "Bonus", "field_type": "number",
                        "read_permission": "hr.bonus.read"}),
            ),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(
        problem["errors"][0]["code"],
        json!("objects.field_permission_unknown")
    );
    let (status, problem) = world
        .call(
            world.admin,
            "POST",
            "/objects/custom:account/fields",
            Some(
                json!({"name": "bonus", "label": "Bonus", "field_type": "number",
                        "read_permission": "Bonus!"}),
            ),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(
        problem["errors"][0]["code"],
        json!("objects.field_permission_invalid")
    );

    // Describe: every field for administrators, with what they may do.
    let (_, described) = world
        .call(world.member, "GET", "/objects/custom:account", None)
        .await;
    let salary = described["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["api_name"] == json!("custom:salary"))
        .expect("salary listed for a manager");
    assert_eq!(salary["access"], json!({"read": false, "edit": false}));
    assert_eq!(salary["read_permission"], json!(READ));
    let (_, described) = world
        .call(world.writer, "GET", "/objects/custom:account", None)
        .await;
    let salary = described["fields"]
        .as_array()
        .expect("fields")
        .iter()
        .find(|field| field["api_name"] == json!("custom:salary"))
        .expect("salary");
    assert_eq!(salary["access"], json!({"read": true, "edit": true}));

    // Records, lists and queries.
    let (status, record) = world
        .call(
            world.member,
            "GET",
            &format!("/objects/custom:account/records/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(record.get("custom:salary").is_none(), "{record}");
    assert_eq!(record["custom:notes"], json!("hello"));
    let (_, page) = world
        .call(world.member, "GET", "/objects/custom:account/records", None)
        .await;
    assert!(page["data"][0].get("custom:salary").is_none(), "{page}");
    let (status, problem) = world
        .call(
            world.member,
            "GET",
            "/query?q=SELECT%20custom%3Asalary%20FROM%20custom%3Aaccount",
            None,
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(problem["errors"][0]["code"], json!("query.field_unknown"));
    let (_, page) = world
        .call(
            world.writer,
            "GET",
            "/query?q=SELECT%20custom%3Asalary%20FROM%20custom%3Aaccount",
            None,
        )
        .await;
    assert_eq!(page["data"][0]["custom:salary"].as_f64(), Some(100.0));

    // Writes: hidden is unknown, read-only is refused with its own code.
    let (status, problem) = world
        .call(
            world.member,
            "PATCH",
            &format!("/objects/custom:account/records/{id}"),
            Some(json!({"expected_version": 1, "custom:salary": 1})),
        )
        .await;
    assert_eq!(status, 400);
    assert_eq!(problem["errors"][0]["code"], json!("records.field_unknown"));
    let (status, problem) = world
        .call(
            world.admin,
            "PATCH",
            "/objects/custom:account/fields/custom:notes",
            Some(json!({"expected_version": 1, "write_permission": WRITE})),
        )
        .await;
    assert_eq!(status, 200, "{problem}");
    let (status, problem) = world
        .call(
            world.member,
            "PATCH",
            &format!("/objects/custom:account/records/{id}"),
            Some(json!({"expected_version": 1, "custom:notes": "changed"})),
        )
        .await;
    assert_eq!(status, 403, "{problem}");
    assert_eq!(problem["code"], json!(FIELD_NOT_WRITABLE));
    assert_eq!(problem["errors"][0]["field"], json!("custom:notes"));
    let (status, _) = world
        .call(
            world.writer,
            "PATCH",
            &format!("/objects/custom:account/records/{id}"),
            Some(json!({"expected_version": 1, "custom:notes": "changed"})),
        )
        .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn hidden_values_never_reach_the_device() {
    let world = World::new().await;
    let id = world.accounts().await;

    let device_directory = tempfile::tempdir().expect("device dir");
    let device_files = platform_device::device_databases(
        device_directory.path(),
        Arc::new(platform_device::MemoryKeys::new()),
    );
    let device_store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(device_files.clone()));
    let member_permissions: BTreeSet<String> = [OBJECTS_MANAGE_PERMISSION.to_owned()].into();
    let engine = ObjectService::new(
        device_store.clone(),
        ObjectCaller::new(
            world.workspace,
            world.member,
            member_permissions,
            vec![world.role],
        ),
    )
    .with_replica()
    .with_client_ids();
    let transport = HttpTransport::new(
        format!("http://{}", world.address),
        world.workspace,
        Arc::new(Token(world.member)),
    )
    .expect("transport");
    let client = SyncClient::new(engine.clone(), transport);
    client
        .engine()
        .set_offline_limit(3_600)
        .await
        .expect("offline limit");
    let report = client.sync_once(context()).await.expect("first sync");
    assert!(report.pulled >= 1);

    // The definition arrived with its permissions; the value did not. An
    // engine over the same replica holding the permission would show a
    // stored value, so none was stored.
    let (definition, _, access) = engine
        .describe_with_field_access(ACCOUNT)
        .await
        .expect("local definition");
    let salary = definition.field("custom:salary").expect("salary defined");
    assert_eq!(salary.read_permission.as_deref(), Some(READ));
    assert!(!access["custom:salary"].read);
    let local = engine.get_record(ACCOUNT, &id, false).await.expect("local");
    assert_eq!(local.values.get("custom:notes"), Some(&json!("hello")));
    assert!(!local.values.contains_key("custom:salary"));
    let privileged = ObjectService::new(
        device_store.clone(),
        ObjectCaller::new(
            world.workspace,
            world.member,
            [READ.to_owned(), OBJECTS_MANAGE_PERMISSION.to_owned()].into(),
            vec![world.role],
        ),
    )
    .with_replica();
    let stored = privileged
        .get_record(ACCOUNT, &id, false)
        .await
        .expect("stored copy");
    assert!(
        !stored.values.contains_key("custom:salary"),
        "the device never received the value: {:?}",
        stored.values
    );

    // A change on the server to the hidden field reaches the device as a
    // change, without its value.
    let (status, record) = world
        .call(
            world.writer,
            "PATCH",
            &format!("/objects/custom:account/records/{id}"),
            Some(json!({"expected_version": 1, "custom:salary": 250})),
        )
        .await;
    assert_eq!(status, 200, "{record}");
    let (_, changes) = world
        .call(world.member, "GET", "/objects/changes", None)
        .await;
    let last = changes["data"]
        .as_array()
        .and_then(|entries| entries.last())
        .cloned()
        .expect("a change");
    assert_eq!(last["fields"], json!([]), "{changes}");
    client.sync_once(context()).await.expect("second sync");
    let stored = privileged
        .get_record(ACCOUNT, &id, false)
        .await
        .expect("stored copy");
    assert_eq!(stored.version, 2, "the device took the new version");
    assert!(!stored.values.contains_key("custom:salary"));

    // The member's offline edit pushes without touching the hidden value.
    engine
        .update_record(
            ACCOUNT,
            &id,
            2,
            json!({"custom:notes": "from the device"})
                .as_object()
                .expect("values"),
            context(),
        )
        .await
        .expect("local edit");
    client.sync_once(context()).await.expect("push");
    let (_, server) = world
        .call(
            world.writer,
            "GET",
            &format!("/objects/custom:account/records/{id}"),
            None,
        )
        .await;
    assert_eq!(server["custom:notes"], json!("from the device"));
    assert_eq!(server["custom:salary"].as_f64(), Some(250.0));
}
