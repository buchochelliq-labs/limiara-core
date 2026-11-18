//! The replica change feed and the route context end to end (ADR 0034 §9),
//! over the SQLite replica. The server is the workspace API's object routes
//! over a SQLite workspace, on a local port; the device syncs from it with
//! the real HTTP transport, and the member's writes and route messages go
//! through `Bridge::handle`, as the web shell sends them. Every event kind
//! and every removal case is checked, with what reaches subscribers and in
//! which order.

use async_trait::async_trait;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use platform_device::capabilities::{LocalServices, PluginDeclaration};
use platform_device::feed::{
    ChangeSubscription, FeedTap, ObjectRemovalReason, RebuildReason, RemovalReason, ReplicaChange,
    ReplicaFeed,
};
use platform_device::route::{CurrentRoute, RouteContext, RoutePattern};
use platform_device::{Bridge, DeviceHost, Member, Workspace};
use platform_objects::store::sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::*;
use platform_objects_http::{Caller, ObjectsHost};
use platform_storage::sqlite::SqliteTenantDatabases;
use platform_sync::{AccessToken, HttpTransport, SyncClient, SyncError};
use plugin_contract::Manifest;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use uuid::Uuid;

const ACCOUNT: &str = "custom:account";
const PLUGIN: &str = "example.tracker";
const TICKET: &str = "example.tracker:ticket";

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

fn values(value: Value) -> serde_json::Map<String, Value> {
    match value {
        Value::Object(map) => map,
        _ => serde_json::Map::new(),
    }
}

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
        source: MutationSource::device(None),
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
    store: Arc<dyn ObjectStore>,
    engine: ObjectService,
    client: SyncClient<HttpTransport>,
    bridge: Bridge,
    services: LocalServices,
    tap: FeedTap,
}

impl World {
    async fn new(capacity: usize) -> Self {
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
            .create_field(ACCOUNT, text_field("notes"), context())
            .await
            .expect("field");
        grant(&admin, ACCOUNT, role, true).await;
        let member_id = Uuid::now_v7();
        let server = Server {
            store: store.clone(),
            roles: Arc::new(BTreeMap::from([(member_id, vec![role])])),
        };
        let users = json!({"data": [{"id": member_id, "name": "Ada Lovelace", "manager_id": null}], "can_manage": false});
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

        // The device, as the controller builds it: the engine reports to
        // the feed, and the bridge reports the member's writes and takes
        // route messages.
        let services =
            LocalServices::new(ReplicaFeed::with_capacity(capacity), RouteContext::new());
        let tap = services.feed().open(workspace);
        let device_directory = tempfile::tempdir().expect("device dir");
        let device_files = platform_device::device_databases(
            device_directory.path(),
            Arc::new(platform_device::MemoryKeys::new()),
        );
        let engine = ObjectService::new(
            Arc::new(SqliteObjectStore::new(device_files)),
            ObjectCaller::new(workspace, member_id, BTreeSet::new(), vec![role]),
        )
        .with_replica()
        .with_client_ids()
        .with_replica_observer(Arc::new(tap.clone()));
        tap.start(&engine).await.expect("feed started");
        let transport = HttpTransport::new(
            format!("http://{address}"),
            workspace,
            Arc::new(Token(member_id)),
        )
        .expect("transport");
        let client = SyncClient::new(engine.clone(), transport);
        engine
            .set_offline_limit(3_600)
            .await
            .expect("offline limit");
        client.sync_once(context()).await.expect("first sync");
        let bridge = Bridge::new(
            DeviceHost::new(
                engine.clone(),
                Member {
                    display_name: "Ada Lovelace".into(),
                    email: None,
                    permissions: BTreeSet::new(),
                    roles: BTreeSet::from(["Sales".to_owned()]),
                },
                Workspace {
                    name: "Analytical Engines".into(),
                },
            )
            .with_feed(tap.clone())
            .with_route_context(services.routes().clone()),
        );
        Self {
            _directories: (server_directory, device_directory),
            workspace,
            member_id,
            colleague: Uuid::now_v7(),
            role,
            admin,
            store,
            engine,
            client,
            bridge,
            services,
            tap,
        }
    }

    /// The member's engine on the server.
    fn member_on_server(&self) -> ObjectService {
        ObjectService::new(
            self.store.clone(),
            ObjectCaller::new(
                self.workspace,
                self.member_id,
                BTreeSet::new(),
                vec![self.role],
            ),
        )
    }

    /// A subscription for a plugin that declared the feed, caught up.
    fn subscriber(&self) -> ChangeSubscription {
        let grant = self.services.grant(
            &PluginDeclaration::new("feed-test", &["device.replica_changes"]).expect("declaration"),
        );
        let mut subscription = grant.replica_changes().expect("declared");
        assert_eq!(
            next(&mut subscription),
            Some(ReplicaChange::Rebuild {
                reason: RebuildReason::Subscribed
            })
        );
        assert_eq!(changes(&mut subscription), Vec::new());
        subscription
    }

    async fn sync(&self) {
        self.client.sync_once(context()).await.expect("sync");
    }

    fn path(&self, rest: &str) -> String {
        format!("/api/v1/workspaces/{}{rest}", self.workspace)
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        self.send(method, path, body, "application/json").await
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        content_type: &str,
    ) -> (u16, Value) {
        let mut request = http::Request::builder().method(method).uri(path);
        let bytes = match body {
            Some(body) => {
                request = request.header("content-type", content_type);
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

    /// A record the member owns, made on the server.
    async fn server_record(&self, name: &str) -> Record {
        self.member_on_server()
            .create_record(ACCOUNT, &values(json!({"name": name})), context())
            .await
            .expect("server record")
    }

    async fn on_server(&self, id: &str) -> Record {
        self.admin
            .get_record(ACCOUNT, id, true)
            .await
            .expect("server copy")
    }
}

fn text_field(name: &str) -> CreateField {
    CreateField {
        name: name.into(),
        label: name.into(),
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
        reference_objects: None,
        delete_behaviour: None,
        formula: None,
        formula_type: None,
        rollup: None,
        lookup_filter: None,
        read_permission: None,
        write_permission: None,
    }
}

async fn grant(admin: &ObjectService, object: &str, role: Uuid, read: bool) {
    admin
        .set_object_permission(
            object,
            role,
            AccessFlags {
                read,
                create: read,
                edit: read,
                delete: read,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("grant");
}

fn next(subscription: &mut ChangeSubscription) -> Option<ReplicaChange> {
    subscription.try_next().map(|event| event.change)
}

fn changes(subscription: &mut ChangeSubscription) -> Vec<ReplicaChange> {
    std::iter::from_fn(|| subscription.try_next())
        .map(|event| event.change)
        .collect()
}

fn upserted(object: &str, record: &str) -> ReplicaChange {
    ReplicaChange::Upserted {
        object: object.into(),
        record: record.into(),
    }
}

fn removed(record: &str, reason: RemovalReason) -> ReplicaChange {
    ReplicaChange::Removed {
        object: ACCOUNT.into(),
        record: record.into(),
        reason,
    }
}

fn remote(record: &Record) -> RemoteRecord {
    RemoteRecord {
        public_id: record.public_id.clone(),
        name: record.name.clone(),
        owner_id: record.owner_id,
        values: record
            .values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        version: record.version,
        created_at: record.created_at,
        created_by: record.created_by,
        updated_at: record.updated_at,
        updated_by: record.updated_by,
        touched_at: record.touched_at,
        deleted_at: record.deleted_at,
        archived_at: record.archived_at,
    }
}

#[tokio::test]
async fn pulled_changes_reach_subscribers_by_id() {
    let world = World::new(1024).await;
    let mut feed = world.subscriber();

    // Created on the server.
    let made = world.server_record("Initech").await;
    world.sync().await;
    assert_eq!(changes(&mut feed), vec![upserted(ACCOUNT, &made.public_id)]);

    // Changed on the server.
    let changed = world
        .admin
        .update_record(
            ACCOUNT,
            &made.public_id,
            made.version,
            &values(json!({"custom:notes": "a confidential note"})),
            context(),
        )
        .await
        .expect("update");
    world.sync().await;
    let events: Vec<_> = std::iter::from_fn(|| feed.try_next()).collect();
    assert_eq!(
        events.iter().map(|event| &event.change).collect::<Vec<_>>(),
        vec![&upserted(ACCOUNT, &made.public_id)]
    );
    // Ids only: no name, no value.
    let wire = serde_json::to_string(&events).expect("json");
    assert!(
        !wire.contains("Initech") && !wire.contains("confidential"),
        "{wire}"
    );
    assert!(events
        .iter()
        .all(|event| event.workspace == Some(world.workspace)));

    // Deleted on the server, then undeleted.
    let deleted = world
        .admin
        .delete_record(ACCOUNT, &made.public_id, changed.version, context())
        .await
        .expect("delete");
    world.sync().await;
    assert_eq!(
        changes(&mut feed),
        vec![removed(&made.public_id, RemovalReason::Deleted)]
    );
    world
        .admin
        .undelete_record(ACCOUNT, &made.public_id, deleted.version, context())
        .await
        .expect("undelete");
    world.sync().await;
    assert_eq!(changes(&mut feed), vec![upserted(ACCOUNT, &made.public_id)]);

    // A sync with nothing new says nothing.
    world.sync().await;
    assert_eq!(changes(&mut feed), Vec::new());
}

#[tokio::test]
async fn the_members_writes_reach_subscribers_before_the_answer() {
    let world = World::new(1024).await;
    let mut feed = world.subscriber();

    let (status, created) = world
        .call(
            "POST",
            &world.path("/objects/custom:account/records"),
            Some(json!({"name": "Globex", "custom:notes": "private"})),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let id = created["id"].as_str().expect("id").to_owned();
    assert_eq!(changes(&mut feed), vec![upserted(ACCOUNT, &id)]);

    let record = world.path(&format!("/objects/custom:account/records/{id}"));
    let (status, updated) = world
        .call(
            "PATCH",
            &record,
            Some(json!({"expected_version": created["version"], "custom:notes": "met"})),
        )
        .await;
    assert_eq!(status, 200, "{updated}");
    // A refused write commits nothing and says nothing.
    let (status, _) = world
        .call(
            "PATCH",
            &record,
            Some(json!({"expected_version": created["version"], "custom:notes": "stale"})),
        )
        .await;
    assert_eq!(status, 409);
    assert_eq!(changes(&mut feed), vec![upserted(ACCOUNT, &id)]);

    let version = updated["version"].as_i64().expect("version");
    let (status, deleted) = world
        .call(
            "DELETE",
            &format!("{record}?expected_version={version}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{deleted}");
    assert_eq!(
        changes(&mut feed),
        vec![removed(&id, RemovalReason::Deleted)]
    );
    let version = deleted["version"].as_i64().expect("version");
    let (status, restored) = world
        .call(
            "POST",
            &format!("{record}/undelete?expected_version={version}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{restored}");
    assert_eq!(changes(&mut feed), vec![upserted(ACCOUNT, &id)]);

    // Reads say nothing.
    world
        .call("GET", &world.path("/objects/custom:account/records"), None)
        .await;
    assert_eq!(changes(&mut feed), Vec::new());

    // The push stores the server's answer: the same record again, at
    // least once.
    world.sync().await;
    let pushed = changes(&mut feed);
    assert!(
        pushed
            .iter()
            .all(|change| change == &upserted(ACCOUNT, &id)),
        "{pushed:?}"
    );
}

#[tokio::test]
async fn a_delete_that_wins_a_conflict_removes_the_record() {
    let world = World::new(1024).await;
    let made = world.server_record("Hooli").await;
    world.sync().await;
    let mut feed = world.subscriber();

    // The member edits on the device while the server deletes.
    let local = world
        .engine
        .get_record(ACCOUNT, &made.public_id, false)
        .await
        .expect("local");
    world
        .call(
            "PATCH",
            &world.path(&format!(
                "/objects/custom:account/records/{}",
                made.public_id
            )),
            Some(json!({"expected_version": local.version, "custom:notes": "edited here"})),
        )
        .await;
    assert_eq!(changes(&mut feed), vec![upserted(ACCOUNT, &made.public_id)]);
    world
        .admin
        .delete_record(ACCOUNT, &made.public_id, made.version, context())
        .await
        .expect("delete on the server");
    let server = remote(&world.on_server(&made.public_id).await);
    // The pull keeps the member's copy until they choose: nothing moved.
    world
        .engine
        .apply_remote(ACCOUNT, &server, None)
        .await
        .expect("pulled");
    assert_eq!(changes(&mut feed), Vec::new());
    // The member lets the deletion stand.
    let settled = world
        .engine
        .settle(
            ACCOUNT,
            &server,
            &Choices {
                keep_deletion: Some(true),
                ..Choices::default()
            },
            context(),
        )
        .await
        .expect("settled");
    assert!(settled.deleted_at.is_some());
    assert_eq!(
        changes(&mut feed),
        vec![removed(&made.public_id, RemovalReason::Deleted)]
    );
}

#[tokio::test]
async fn losing_access_on_the_server_removes_records_and_objects() {
    let world = World::new(1024).await;
    let mine = world
        .member_on_server()
        .create_record(ACCOUNT, &values(json!({"name": "Mine"})), context())
        .await
        .expect("mine");
    let shared = world
        .member_on_server()
        .create_record(ACCOUNT, &values(json!({"name": "Handed over"})), context())
        .await
        .expect("handed over");
    world.sync().await;
    let mut feed = world.subscriber();

    // Sharing: the record goes to a colleague, so a private object hides
    // it from the member. The revocation set after a reconnect removes it.
    world
        .admin
        .change_owner(
            ACCOUNT,
            &shared.public_id,
            world.colleague,
            shared.version,
            context(),
        )
        .await
        .expect("owner change");
    world.client.reconnected();
    world.sync().await;
    assert_eq!(
        changes(&mut feed),
        vec![removed(&shared.public_id, RemovalReason::AccessLost)]
    );

    // Permissions: the member's role no longer reads accounts. The object
    // leaves the replica with every record at once, then the definitions
    // move.
    grant(&world.admin, ACCOUNT, world.role, false).await;
    world.sync().await;
    let events = changes(&mut feed);
    assert_eq!(
        events.first(),
        Some(&ReplicaChange::ObjectRemoved {
            object: ACCOUNT.into(),
            reason: ObjectRemovalReason::LeftReplica,
        }),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|change| matches!(change, ReplicaChange::DefinitionsChanged { .. })),
        "{events:?}"
    );
    // Its records may be named again as the revocation set forgets them;
    // never as anything but a removal.
    assert!(
        events.iter().all(|change| match change {
            ReplicaChange::Removed { record, .. } => record == &mine.public_id,
            ReplicaChange::ObjectRemoved { .. } | ReplicaChange::DefinitionsChanged { .. } => true,
            _ => false,
        }),
        "{events:?}"
    );
}

#[tokio::test]
async fn definitions_report_their_metadata_version() {
    let world = World::new(1024).await;
    let mut feed = world.subscriber();
    world
        .admin
        .create_field(ACCOUNT, text_field("region"), context())
        .await
        .expect("field");
    let version = world
        .admin
        .list_changes(None, Some(1), None)
        .await
        .expect("changes")
        .metadata_version;
    world.sync().await;
    assert_eq!(
        changes(&mut feed),
        vec![ReplicaChange::DefinitionsChanged {
            metadata_version: version
        }]
    );
    // The same version again is no change.
    world.sync().await;
    assert_eq!(changes(&mut feed), Vec::new());
}

#[tokio::test]
async fn a_suspended_plugin_takes_its_objects_off_the_replica() {
    let world = World::new(1024).await;
    let manifest = Manifest::parse_toml(include_str!(
        "../../../fixtures/plugins/tracker/plugin.toml"
    ))
    .expect("the tracker fixture parses");
    let installer = ObjectService::new(
        world.store.clone(),
        ObjectCaller::plugin_installer(world.workspace, Uuid::now_v7(), PLUGIN),
    );
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    grant(&world.admin, TICKET, world.role, true).await;
    let ticket = world
        .member_on_server()
        .create_record(
            TICKET,
            &values(json!({"name": "Printer on fire"})),
            context(),
        )
        .await
        .expect("ticket");
    world.sync().await;
    let mut feed = world.subscriber();
    assert!(world.engine.describe(TICKET).await.is_ok());

    // A ticket changed while the plugin is in service.
    world
        .member_on_server()
        .update_record(
            TICKET,
            &ticket.public_id,
            ticket.version,
            &values(json!({"name": "Printer still on fire"})),
            context(),
        )
        .await
        .expect("ticket update");
    world.sync().await;
    assert_eq!(
        changes(&mut feed),
        vec![upserted(TICKET, &ticket.public_id)]
    );

    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    world.sync().await;
    let events = changes(&mut feed);
    assert_eq!(
        events.first(),
        Some(&ReplicaChange::ObjectRemoved {
            object: TICKET.into(),
            reason: ObjectRemovalReason::PluginSuspended,
        }),
        "{events:?}"
    );
    assert!(
        events
            .iter()
            .any(|change| matches!(change, ReplicaChange::DefinitionsChanged { .. })),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|change| matches!(change, ReplicaChange::Upserted { .. })),
        "{events:?}"
    );
    assert!(world.engine.describe(TICKET).await.is_err());
}

#[tokio::test]
async fn the_lease_locks_and_the_next_sync_unlocks() {
    let world = World::new(1024).await;
    let mut feed = world.subscriber();
    // Last contact two hours ago, with a one-hour limit.
    world
        .engine
        .record_contact_at(chrono::Utc::now() - chrono::Duration::hours(2))
        .await
        .expect("contact");
    assert_eq!(changes(&mut feed), vec![ReplicaChange::WorkspaceLocked]);
    // Reading the lease again changes nothing.
    world.tap.check_lease(&world.engine).await.expect("lease");
    assert_eq!(changes(&mut feed), Vec::new());
    // A locked replica refuses the member's route messages too.
    let (status, refused) = world
        .call(
            "POST",
            &world.path("/ui/route"),
            Some(json!({"route": "object", "object": ACCOUNT})),
        )
        .await;
    assert_eq!(status, 409, "{refused}");
    assert_eq!(refused["code"], "sync.lease_expired");

    world.sync().await;
    assert_eq!(changes(&mut feed), vec![ReplicaChange::WorkspaceUnlocked]);
}

#[tokio::test]
async fn a_subscriber_that_falls_behind_rebuilds() {
    let world = World::new(4).await;
    let mut slow = world.subscriber();
    let mut keeping_up = world.subscriber();
    let mut made = Vec::new();
    for index in 0..3 {
        made.push(world.server_record(&format!("Account {index}")).await);
        world.sync().await;
        assert_eq!(
            changes(&mut keeping_up),
            vec![upserted(ACCOUNT, &made[index].public_id)]
        );
    }
    for index in 3..8 {
        made.push(world.server_record(&format!("Account {index}")).await);
    }
    world.sync().await;
    // Eight events through a buffer of four.
    assert_eq!(
        next(&mut slow),
        Some(ReplicaChange::Rebuild {
            reason: RebuildReason::FellBehind
        })
    );
    assert_eq!(next(&mut slow), None);
    // The other subscriber lost its place too, for the five pulled at once.
    assert_eq!(
        next(&mut keeping_up),
        Some(ReplicaChange::Rebuild {
            reason: RebuildReason::FellBehind
        })
    );
    // Both carry on with what follows, in the same order.
    let (status, created) = world
        .call(
            "POST",
            &world.path("/objects/custom:account/records"),
            Some(json!({"name": "After"})),
        )
        .await;
    assert_eq!(status, 201);
    let id = created["id"].as_str().expect("id");
    let a = slow.try_next().expect("slow");
    let b = keeping_up.try_next().expect("keeping up");
    assert_eq!(a, b);
    assert_eq!(a.change, upserted(ACCOUNT, id));
}

#[tokio::test]
async fn the_route_context_takes_only_valid_routes_for_the_replica() {
    let world = World::new(1024).await;
    let grant = world.services.grant(
        &PluginDeclaration::new("route-test", &["device.route_context"]).expect("declaration"),
    );
    let mut routes = grant.route_context().expect("declared");
    assert!(grant.replica_changes().is_err(), "not declared");

    // The host says it takes route messages.
    let (status, ui) = world
        .call("GET", &world.path("/ui/contributions"), None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(ui["route_context"], true, "{ui}");

    let made = world.server_record("Umbrella").await;
    world.sync().await;
    let route = world.path("/ui/route");
    let (status, body) = world
        .call(
            "POST",
            &route,
            Some(json!({"route": "record", "object": ACCOUNT, "record_id": made.public_id})),
        )
        .await;
    assert_eq!(status, 204, "{body}");
    assert_eq!(
        routes.changed().await,
        Ok(Some(CurrentRoute {
            workspace: world.workspace,
            route: RoutePattern::Record,
            object: Some(ACCOUNT.into()),
            record_id: Some(made.public_id.clone()),
        }))
    );
    let (status, _) = world
        .call("POST", &route, Some(json!({"route": "search"})))
        .await;
    assert_eq!(status, 204);
    assert_eq!(
        routes
            .changed()
            .await
            .expect("open")
            .map(|route| route.route),
        Some(RoutePattern::Search)
    );

    // Refused, and the route stays as it was.
    let other_prefix = platform_id::PublicId::new("zzz", 1).expect("id");
    for (body, status, code) in [
        (
            json!({"route": "record", "object": ACCOUNT}),
            400,
            "route.invalid",
        ),
        (
            json!({"route": "search", "q": "secret"}),
            400,
            "route.invalid",
        ),
        (json!({"route": "<script>"}), 400, "route.invalid"),
        (json!(["record"]), 400, "route.invalid"),
        (
            json!({"route": "object", "object": "custom:missing"}),
            404,
            "route.object_not_found",
        ),
        (
            json!({"route": "object", "object": "javascript:alert"}),
            400,
            "route.invalid",
        ),
        (
            json!({"route": "record", "object": ACCOUNT, "record_id": "not an id"}),
            400,
            "route.record_id_invalid",
        ),
        (
            json!({"route": "record", "object": ACCOUNT, "record_id": other_prefix.as_str()}),
            400,
            "route.record_id_invalid",
        ),
    ] {
        let (answer, problem) = world.call("POST", &route, Some(body.clone())).await;
        assert_eq!(answer, status, "{body}: {problem}");
        assert_eq!(problem["code"], code, "{body}: {problem}");
    }
    let (status, problem) = world
        .send("POST", &route, Some(json!({"route": "home"})), "text/plain")
        .await;
    assert_eq!(
        (status, problem["code"].clone()),
        (400, json!("route.invalid"))
    );
    assert_eq!(
        routes.current().map(|route| route.route),
        Some(RoutePattern::Search)
    );

    // The web view can read neither the route back nor the feed.
    let (status, _) = world.call("GET", &route, None).await;
    assert_eq!(status, 405);
    for path in ["/replica/changes", "/feed", "/ui/route/stream"] {
        let (status, _) = world.call("GET", &world.path(path), None).await;
        assert!(status == 404 || status == 405, "{path}: {status}");
    }
    // Another workspace's route is refused at the boundary.
    let (status, problem) = world
        .call(
            "POST",
            &format!("/api/v1/workspaces/{}/ui/route", Uuid::now_v7()),
            Some(json!({"route": "home"})),
        )
        .await;
    assert_eq!(
        (status, problem["code"].clone()),
        (404, json!("workspace.not_found"))
    );
}

#[tokio::test]
async fn a_host_without_a_route_context_takes_no_route_messages() {
    let world = World::new(1024).await;
    let bridge = Bridge::new(DeviceHost::new(
        world.engine.clone(),
        Member {
            display_name: "Ada Lovelace".into(),
            email: None,
            permissions: BTreeSet::new(),
            roles: BTreeSet::new(),
        },
        Workspace {
            name: "Analytical Engines".into(),
        },
    ));
    let ui = bridge
        .handle(
            http::Request::builder()
                .uri(world.path("/ui/contributions"))
                .body(Vec::new())
                .expect("request"),
        )
        .await;
    let ui: Value = serde_json::from_slice(ui.body()).expect("json");
    assert!(ui.get("route_context").is_none(), "{ui}");
    let refused = bridge
        .handle(
            http::Request::builder()
                .method("POST")
                .uri(world.path("/ui/route"))
                .header("content-type", "application/json")
                .body(json!({"route": "home"}).to_string().into_bytes())
                .expect("request"),
        )
        .await;
    assert_eq!(refused.status(), 404);
}
