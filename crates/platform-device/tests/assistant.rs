//! The assistant bridge's conformance suite (ADR 0034 slice 3), over the
//! SQLite replica. The server is the object routes the workspace API
//! serves, over a SQLite workspace and behind a stand-in for its
//! authentication that treats every token as a synced device's; the device
//! syncs from it over real HTTP, and a native plugin's calls go through the
//! `device.assistant` grant.

use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use platform_device::assistant::confirm::AssistantClock;
use platform_device::assistant::errors::{AssistantCode, AssistantError};
use platform_device::assistant::index::{IndexStream, IndexUpdate};
use platform_device::assistant::*;
use platform_device::capabilities::{LocalServices, PluginDeclaration};
use platform_device::feed::{ClearReason, FeedTap, ReplicaFeed};
use platform_device::route::RouteContext;
use platform_device::{Bridge, DeviceHost, Member, Workspace};
use platform_objects::store::sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::*;
use platform_objects_http::{Caller, ObjectsHost};
use platform_storage::sqlite::SqliteTenantDatabases;
use platform_sync::{AccessToken, HttpTransport, SyncClient, SyncError};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use uuid::Uuid;

const ACCOUNT: &str = "custom:account";
const SECRET: &str = "custom:secret";
const READ: &str = "hr.salary.read";
const WRITE: &str = "hr.rating.write";
const WAIT: Duration = Duration::from_millis(300);

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

fn values(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("values")
}

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
            (WRITE.to_owned(), "Change ratings".to_owned()),
            (
                OBJECTS_MANAGE_PERMISSION.to_owned(),
                "Manage objects".to_owned(),
            ),
        ])))
    }
}

/// The member's permissions, and the device every token belongs to.
type Authority = Arc<(BTreeMap<Uuid, BTreeSet<String>>, Uuid)>;

/// The access token is the member's id; every request is a synced
/// device's, as the server verifies its token.
async fn authenticate(
    State(authority): State<Authority>,
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
        permissions: authority.0.get(&user_id).cloned().unwrap_or_default(),
        source: MutationSource::device(Some(authority.1)),
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

/// A clock the tests move.
struct TestClock(Instant, AtomicU64);

impl AssistantClock for TestClock {
    fn now(&self) -> Instant {
        self.0 + Duration::from_secs(self.1.load(Ordering::SeqCst))
    }
}

struct World {
    _directories: (tempfile::TempDir, tempfile::TempDir),
    workspace: Uuid,
    device: Uuid,
    admin: ObjectService,
    client: SyncClient<HttpTransport>,
    engine: ObjectService,
    /// The web shell's bridge.
    bridge: Bridge,
    services: LocalServices,
    assistant: AssistantBridge,
    tap: FeedTap,
    clock: Arc<TestClock>,
    acme: String,
    /// A record of a colleague's the member cannot see.
    hidden: String,
}

fn field(value: Value) -> CreateField {
    serde_json::from_value(value).expect("field")
}

impl World {
    async fn new() -> Self {
        let server_directory = tempfile::tempdir().expect("server dir");
        let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(Arc::new(
            SqliteTenantDatabases::new(server_directory.path(), SQLITE_MIGRATIONS),
        )));
        let workspace = Uuid::now_v7();
        let role = Uuid::now_v7();
        let set = |values: &[&str]| -> BTreeSet<String> {
            values.iter().map(|value| (*value).to_owned()).collect()
        };
        let admin = ObjectService::new(
            store.clone(),
            ObjectCaller::new(
                workspace,
                Uuid::now_v7(),
                set(&[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION]),
                Vec::new(),
            ),
        );
        for (name, label, plural) in [
            ("account", "Account", "Accounts"),
            ("secret", "Secret", "Secrets"),
        ] {
            admin
                .create_object(
                    serde_json::from_value(json!({"name": name, "label": label,
                        "plural_label": plural, "sharing_model": "private",
                        "track_history": true, "searchable": true, "in_navigation": true}))
                    .expect("object"),
                    context(),
                )
                .await
                .expect("object");
        }
        for definition in [
            json!({"name": "notes", "label": "Notes", "field_type": "text",
                   "indexed": true, "searchable": true, "track_history": true}),
            json!({"name": "amount", "label": "Amount", "field_type": "currency",
                   "indexed": true, "scale": 2, "track_history": true}),
            json!({"name": "stage", "label": "Stage", "field_type": "picklist",
                   "options": [{"value": "open", "label": "Open"},
                               {"value": "won", "label": "Closed won"}]}),
            json!({"name": "parent", "label": "Parent account", "field_type": "lookup",
                   "reference_object": ACCOUNT}),
            json!({"name": "salary", "label": "Salary", "field_type": "text",
                   "read_permission": READ}),
            json!({"name": "rating", "label": "Rating", "field_type": "number",
                   "write_permission": WRITE}),
            json!({"name": "summary", "label": "Summary", "field_type": "long_text"}),
        ] {
            admin
                .create_field(ACCOUNT, field(definition), context())
                .await
                .expect("field");
        }
        admin
            .create_validation_rule(
                ACCOUNT,
                CreateValidationRule {
                    name: "amount_positive".into(),
                    description: None,
                    condition: "custom:amount < 0".into(),
                    message: "Amount can't be negative.".into(),
                    error_field: Some("custom:amount".into()),
                    active: None,
                },
                context(),
            )
            .await
            .expect("rule");
        for object in [ACCOUNT, SECRET] {
            admin
                .set_object_permission(
                    object,
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
        }
        for action in [
            json!({"name": "note", "label": "Add a note", "kind": "update",
                   "fields": ["custom:notes"], "presets": {"custom:stage": "won"}}),
            json!({"name": "add_branch", "label": "Add branch", "kind": "create",
                   "target_object": ACCOUNT, "target_field": "custom:parent",
                   "fields": ["name"], "presets": {"custom:notes": "branch office"}}),
            json!({"name": "quiet", "label": "Quiet", "kind": "update",
                   "fields": ["custom:notes"]}),
        ] {
            admin
                .create_quick_action(
                    ACCOUNT,
                    serde_json::from_value(action).expect("action"),
                    context(),
                )
                .await
                .expect("quick action");
        }
        admin
            .update_assistant_settings(
                UpdateAssistantSettings {
                    expected_version: 0,
                    enabled: true,
                },
                context(),
            )
            .await
            .expect("switch");
        admin
            .update_object_assistant_policy(
                ACCOUNT,
                serde_json::from_value(json!({"expected_version": 0, "exposed": true,
                    "writable": true, "indexable": true,
                    "excluded_fields": ["custom:notes"],
                    "offered_actions": ["note", "add_branch"]}))
                .expect("policy"),
                context(),
            )
            .await
            .expect("policy");

        let member = Uuid::now_v7();
        let colleague = Uuid::now_v7();
        // On the server the member's own account has a salary they cannot
        // read on the device.
        let acme = ObjectService::new(
            store.clone(),
            ObjectCaller::new(workspace, member, set(&[READ, WRITE]), vec![role]),
        )
        .create_record(
            ACCOUNT,
            &values(json!({"name": "Acme", "custom:notes": "Key account",
                "custom:amount": 1200, "custom:stage": "open",
                "custom:salary": "90000", "custom:rating": 4})),
            context(),
        )
        .await
        .expect("acme")
        .public_id;
        let hidden = ObjectService::new(
            store.clone(),
            ObjectCaller::new(workspace, colleague, BTreeSet::new(), vec![role]),
        )
        .create_record(ACCOUNT, &values(json!({"name": "Hidden Co"})), context())
        .await
        .expect("hidden")
        .public_id;
        ObjectService::new(
            store.clone(),
            ObjectCaller::new(workspace, member, BTreeSet::new(), vec![role]),
        )
        .create_record(SECRET, &values(json!({"name": "Vault"})), context())
        .await
        .expect("secret");

        let device = Uuid::now_v7();
        let authority: Authority = Arc::new((BTreeMap::new(), device));
        let users = json!({"data": [{"id": member, "name": "Ada Lovelace", "manager_id": null}],
                           "can_manage": false});
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
            .layer(middleware::from_fn_with_state(authority, authenticate))
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .with_state(Server { store, role });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("port");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await });

        // The device, with the shell's local services on a phone with Siri.
        let device_directory = tempfile::tempdir().expect("device dir");
        let files = platform_device::device_databases(
            device_directory.path(),
            Arc::new(platform_device::MemoryKeys::new()),
        );
        let clock = Arc::new(TestClock(Instant::now(), AtomicU64::new(0)));
        let services = LocalServices::with_assistant(
            ReplicaFeed::new(),
            RouteContext::new(),
            Some(Assistant::Siri),
            clock.clone(),
        );
        let tap = services.feed().open(workspace);
        let engine = ObjectService::new(
            Arc::new(SqliteObjectStore::new(files)),
            ObjectCaller::new(workspace, member, BTreeSet::new(), vec![role]),
        )
        .with_replica()
        .with_client_ids()
        .with_replica_observer(Arc::new(tap.clone()));
        let transport = HttpTransport::new(
            format!("http://{address}"),
            workspace,
            Arc::new(Token(member)),
        )
        .expect("transport");
        let client = SyncClient::new(engine.clone(), transport);
        engine.set_offline_limit(3_600).await.expect("limit");
        client.sync_once(context()).await.expect("first sync");
        tap.start(&engine).await.expect("feed");
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
            .with_device(device)
            .with_feed(tap.clone())
            .with_route_context(services.routes().clone()),
        );
        services.assistant().open(&bridge, None);
        let assistant = services
            .grant(&PluginDeclaration::new("siri-assistant", &["device.assistant"]).expect("decl"))
            .assistant()
            .expect("granted");
        Self {
            _directories: (server_directory, device_directory),
            workspace,
            device,
            admin,
            client,
            engine,
            bridge,
            services,
            assistant,
            tap,
            clock,
            acme,
            hidden,
        }
    }

    async fn ask(&self, operation: Operation) -> Result<AssistantResult, AssistantError> {
        self.assistant
            .call(AssistantRequest::new(Assistant::Siri, true, operation))
            .await
            .into_result()
    }

    async fn code(&self, operation: Operation) -> AssistantCode {
        match self.ask(operation).await {
            Ok(result) => panic!("expected a refusal, got {result:?}"),
            Err(error) => {
                // Every refusal speaks its code's fixed sentence or one
                // filled with labels only.
                assert!(!error.message.contains("assistant."), "{error:?}");
                error.code
            }
        }
    }

    async fn prepare(&self, operation: Operation) -> Confirmation {
        match self.ask(operation).await {
            Ok(AssistantResult::Confirmation(confirmation)) => confirmation,
            other => panic!("expected a confirmation, got {other:?}"),
        }
    }

    async fn commit(&self, token: &str) -> Result<Committed, AssistantError> {
        match self
            .ask(Operation::Commit {
                token: token.to_owned(),
            })
            .await
        {
            Ok(AssistantResult::Committed(committed)) => Ok(committed),
            Ok(other) => panic!("expected a commit, got {other:?}"),
            Err(error) => Err(error),
        }
    }

    async fn web(&self, method: &str, rest: &str, body: Option<Value>) -> (u16, Value) {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("/api/v1/workspaces/{}{rest}", self.workspace));
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
            serde_json::from_slice(response.body()).expect("JSON")
        };
        (status, value)
    }

    async fn device_record(&self, id: &str) -> Value {
        self.web("GET", &format!("/objects/{ACCOUNT}/records/{id}"), None)
            .await
            .1
    }

    async fn set_policy(&self, change: Value) {
        let current = self
            .admin
            .object_assistant_policy(ACCOUNT)
            .await
            .expect("policy");
        let mut body = change;
        body["expected_version"] = json!(current.version);
        self.admin
            .update_object_assistant_policy(
                ACCOUNT,
                serde_json::from_value(body).expect("policy"),
                context(),
            )
            .await
            .expect("policy");
    }

    async fn set_switch(&self, enabled: bool) {
        let current = self.admin.assistant_policy().await.expect("policy");
        self.admin
            .update_assistant_settings(
                UpdateAssistantSettings {
                    expected_version: current.settings.version,
                    enabled,
                },
                context(),
            )
            .await
            .expect("switch");
    }

    async fn sync(&self) -> platform_sync::SyncReport {
        self.client.sync_once(context()).await.expect("sync")
    }
}

fn get(object: &str, record: &str) -> Operation {
    Operation::Get {
        object: object.into(),
        record: record.into(),
    }
}

#[tokio::test]
async fn the_capability_is_required_and_the_web_view_reaches_none_of_it() {
    let world = World::new().await;
    for declared in [
        &[][..],
        &["device.replica_changes", "device.route_context"][..],
    ] {
        let grant = world
            .services
            .grant(&PluginDeclaration::new("other", declared).expect("decl"));
        assert_eq!(
            grant.assistant().map(|_| ()).map_err(|error| error.code()),
            Err("device.capability_not_granted")
        );
    }
    // The command bridge serves no assistant route, for any method.
    for (method, rest) in [
        ("GET", "/assistant"),
        ("POST", "/assistant"),
        ("POST", "/assistant/commit"),
        ("GET", "/assistant/describe"),
        ("POST", "/ui/assistant"),
    ] {
        let (status, body) = world.web(method, rest, Some(json!({}))).await;
        assert!(
            status == 404 || status == 403 || status == 405,
            "{method} {rest}: {status} {body}"
        );
    }
    let (status, body) = world.web("GET", "/assistant", None).await;
    assert_eq!(status, 404, "{body}");
    // Nothing the web view sends can claim an assistant's write.
    let (status, answer) = world
        .web(
            "POST",
            "/composite",
            Some(
                json!({"operations": [{"method": "create", "reference": "r0",
                "object": ACCOUNT, "values": {"name": "Claimed"},
                "assistant": {"assistant": "siri"}}]}),
            ),
        )
        .await;
    assert_eq!(status, 200, "{answer}");
    assert_eq!(
        answer["results"][0]["body"]["errors"][0]["code"],
        "assistant.marker_not_accepted"
    );
    // A web-view write pushes unmarked, as the device's.
    let (status, created) = world
        .web(
            "POST",
            &format!("/objects/{ACCOUNT}/records"),
            Some(json!({"name": "Typed"})),
        )
        .await;
    assert_eq!(status, 201, "{created}");
    let batch = world.engine.push_queue().await.expect("queue");
    assert!(batch
        .request()
        .operations
        .iter()
        .all(|operation| operation.assistant().is_none()));
    // A plugin for another platform's assistant, or another contract, is
    // refused.
    let answer = world
        .assistant
        .call(AssistantRequest::new(
            Assistant::Gemini,
            true,
            Operation::Describe,
        ))
        .await;
    assert_eq!(
        answer.into_result().map(|_| ()).map_err(|error| error.code),
        Err(AssistantCode::Invalid)
    );
    let mut request = AssistantRequest::new(Assistant::Siri, true, Operation::Describe);
    request.contract = "assistant.v0".into();
    assert_eq!(
        world
            .assistant
            .call(request)
            .await
            .into_result()
            .map(|_| ())
            .map_err(|error| error.code),
        Err(AssistantCode::Invalid)
    );
    let answer: Value = serde_json::from_slice(
        &world
            .assistant
            .call_json(br#"{"contract":"assistant.v1","assistant":"siri","device_unlocked":true,"operation":{"name":"delete","object":"custom:account","record":"x"}}"#)
            .await,
    )
    .expect("json");
    assert_eq!(answer["error"]["code"], "assistant.invalid", "{answer}");
    assert_eq!(answer["contract"], "assistant.v1");
}

#[tokio::test]
async fn every_operation_runs_as_the_member_over_the_replica() {
    let world = World::new().await;

    // describe: only exposed objects, only readable fields, offered actions.
    let Ok(AssistantResult::Description(description)) = world.ask(Operation::Describe).await else {
        panic!("describe")
    };
    assert!(description.metadata_version > 0);
    let names: Vec<&str> = description
        .objects
        .iter()
        .map(|object| object.api_name.as_str())
        .collect();
    assert_eq!(names, [ACCOUNT], "the secret object is not exposed");
    let account = &description.objects[0];
    assert_eq!(account.label, "Account");
    assert!(account.writable && account.can_create && account.can_edit);
    assert!(!account.timeline, "replicas carry no timeline sources yet");
    assert_eq!(account.metadata_version, description.metadata_version);
    let fields: Vec<&str> = account
        .fields
        .iter()
        .map(|field| field.api_name.as_str())
        .collect();
    assert!(fields.contains(&"custom:notes") && fields.contains(&"custom:stage"));
    assert!(!fields.contains(&"custom:salary"), "field-level security");
    let rating = account
        .fields
        .iter()
        .find(|field| field.api_name == "custom:rating")
        .expect("rating");
    assert!(rating.read_only, "readable, not writable by the member");
    let stage = account
        .fields
        .iter()
        .find(|field| field.api_name == "custom:stage")
        .expect("stage");
    assert_eq!(stage.options[1].label, "Closed won");
    let actions: Vec<&str> = account
        .actions
        .iter()
        .map(|action| action.name.as_str())
        .collect();
    assert_eq!(actions, ["note", "add_branch"], "quiet is not offered");
    assert_eq!(account.actions[1].target_object, ACCOUNT);

    // search
    let Ok(AssistantResult::SearchResults(found)) = world
        .ask(Operation::Search {
            text: "acme".into(),
            object: None,
        })
        .await
    else {
        panic!("search")
    };
    assert_eq!(found.results.len(), 1, "{found:?}");
    assert_eq!(found.results[0].id, world.acme);
    assert_eq!(found.results[0].object_label, "Account");
    assert_eq!(
        world
            .code(Operation::Search {
                text: "vault".into(),
                object: Some(SECRET.into()),
            })
            .await,
        AssistantCode::ObjectNotExposed
    );
    let Ok(AssistantResult::SearchResults(found)) = world
        .ask(Operation::Search {
            text: "vault".into(),
            object: None,
        })
        .await
    else {
        panic!("search")
    };
    assert!(found.results.is_empty(), "unexposed objects stay out");

    // get, with display values formatted by the bridge
    let Ok(AssistantResult::Record(acme)) = world.ask(get(ACCOUNT, &world.acme)).await else {
        panic!("get")
    };
    assert_eq!(acme.name, "Acme");
    let value = |name: &str| {
        acme.fields
            .iter()
            .find(|field| field.field == name)
            .unwrap_or_else(|| panic!("{name}"))
            .clone()
    };
    assert_eq!(value("custom:amount").display, "1200.00");
    assert_eq!(value("custom:stage").display, "Open");
    assert_eq!(value("custom:stage").value, json!("open"));
    assert!(acme
        .fields
        .iter()
        .all(|field| field.field != "custom:salary"));
    assert!(
        !serde_json::to_value(&acme)
            .expect("json")
            .to_string()
            .contains("90000"),
        "a hidden value never reaches the assistant"
    );

    // list: typed conditions compiled into the query language
    let list = |conditions: Value, sort: Option<Sort>| Operation::List {
        object: ACCOUNT.into(),
        conditions: serde_json::from_value(conditions).expect("conditions"),
        sort,
        cursor: None,
    };
    for conditions in [
        json!([{"test": "equals", "field": "custom:stage", "value": "open"}]),
        json!([{"test": "after", "field": "custom:amount", "value": 1000}]),
        json!([{"test": "between", "field": "custom:amount", "from": 1000, "to": 1200}]),
        json!([{"test": "contains", "field": "name", "value": "cm"}]),
        json!([{"test": "equals", "field": "name", "value": "acme' OR name != '"}]),
        json!([{"test": "is_empty", "field": "custom:parent"}]),
    ] {
        let Ok(AssistantResult::Records(page)) = world.ask(list(conditions.clone(), None)).await
        else {
            panic!("list {conditions}")
        };
        let expected: &[&str] = if conditions.to_string().contains("OR") {
            &[]
        } else {
            &["Acme"]
        };
        assert_eq!(
            page.records
                .iter()
                .map(|record| record.name.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{conditions}"
        );
    }
    let Ok(AssistantResult::Records(page)) = world
        .ask(list(
            json!([]),
            Some(Sort {
                field: "custom:amount".into(),
                descending: true,
            }),
        ))
        .await
    else {
        panic!("sorted")
    };
    assert_eq!(page.records.len(), 1);
    let too_many = json!([
        {"test": "is_empty", "field": "name"}, {"test": "is_empty", "field": "name"},
        {"test": "is_empty", "field": "name"}, {"test": "is_empty", "field": "name"},
        {"test": "is_empty", "field": "name"}, {"test": "is_empty", "field": "name"}
    ]);
    assert_eq!(
        world.code(list(too_many, None)).await,
        AssistantCode::Invalid
    );
    // A field the engine cannot filter on is named by its label.
    let Err(error) = world
        .ask(list(
            json!([{"test": "equals", "field": "custom:summary", "value": "x"}]),
            None,
        ))
        .await
    else {
        panic!("unindexed")
    };
    assert_eq!(error.code, AssistantCode::Invalid);
    assert_eq!(error.fields[0].label, "Summary", "{error:?}");
    assert_eq!(
        world
            .code(list(
                json!([{"test": "before", "field": "custom:stage", "value": "open"}]),
                None,
            ))
            .await,
        AssistantCode::Invalid
    );

    // timeline: the replica has none yet, and an unknown record is not found
    let Ok(AssistantResult::Timeline(timeline)) = world
        .ask(Operation::Timeline {
            object: ACCOUNT.into(),
            record: world.acme.clone(),
            cursor: None,
        })
        .await
    else {
        panic!("timeline")
    };
    assert!(timeline.open.is_empty() && timeline.past.is_empty());

    // actions
    let Ok(AssistantResult::Actions(listed)) = world
        .ask(Operation::Actions {
            object: ACCOUNT.into(),
            record: Some(world.acme.clone()),
        })
        .await
    else {
        panic!("actions")
    };
    assert_eq!(listed.actions.len(), 2);

    // create, in two steps
    let confirmation = world
        .prepare(Operation::Create {
            object: ACCOUNT.into(),
            values: values(json!({"name": "Globex", "custom:amount": 50,
                                  "custom:stage": "won", "custom:parent": world.acme})),
        })
        .await;
    assert_eq!(confirmation.write, WriteKind::Create);
    assert_eq!(confirmation.object_label, "Account");
    let change = |name: &str| {
        confirmation
            .changes
            .iter()
            .find(|change| change.field == name)
            .unwrap_or_else(|| panic!("{name}"))
            .display
            .clone()
    };
    assert_eq!(change("name"), "Globex");
    assert_eq!(change("custom:amount"), "50.00");
    assert_eq!(change("custom:stage"), "Closed won");
    assert_eq!(change("custom:parent"), "Acme");
    let created = world.commit(&confirmation.token).await.expect("created");
    assert_eq!(created.name, "Globex");
    // History records the assistant as the source.
    let (history, _) = world
        .engine
        .record_history(ACCOUNT, &created.id, None, None)
        .await
        .expect("history");
    assert!(
        history
            .entries
            .iter()
            .all(|entry| entry.source == Some(MutationSource::assistant(Assistant::Siri))),
        "{history:?}"
    );

    // update, in two steps: only what changes is confirmed
    let confirmation = world
        .prepare(Operation::Update {
            object: ACCOUNT.into(),
            record: world.acme.clone(),
            values: values(json!({"custom:notes": "Renewal due", "custom:stage": "open"})),
        })
        .await;
    assert_eq!(confirmation.record_name.as_deref(), Some("Acme"));
    assert_eq!(confirmation.changes.len(), 1, "{confirmation:?}");
    assert_eq!(confirmation.changes[0].label, "Notes");
    world.commit(&confirmation.token).await.expect("updated");
    assert_eq!(
        world.device_record(&world.acme).await["custom:notes"],
        "Renewal due"
    );

    // run_action: an update action applies its presets
    let confirmation = world
        .prepare(Operation::RunAction {
            object: ACCOUNT.into(),
            action: "note".into(),
            record: world.acme.clone(),
            values: values(json!({"custom:notes": "Called"})),
        })
        .await;
    assert_eq!(confirmation.action_label.as_deref(), Some("Add a note"));
    world.commit(&confirmation.token).await.expect("noted");
    let acme = world.device_record(&world.acme).await;
    assert_eq!(acme["custom:notes"], "Called");
    assert_eq!(acme["custom:stage"], "won");
    // and a create action makes the child, pointing at the record
    let confirmation = world
        .prepare(Operation::RunAction {
            object: ACCOUNT.into(),
            action: "add_branch".into(),
            record: world.acme.clone(),
            values: values(json!({"name": "Acme Paris"})),
        })
        .await;
    let branch = world.commit(&confirmation.token).await.expect("branch");
    let branch = world.device_record(&branch.id).await;
    assert_eq!(branch["custom:parent"], json!(world.acme));
    assert_eq!(branch["custom:notes"], "branch office");
    // A form field the action does not ask for is refused.
    assert_eq!(
        world
            .code(Operation::RunAction {
                object: ACCOUNT.into(),
                action: "note".into(),
                record: world.acme.clone(),
                values: values(json!({"custom:amount": 1})),
            })
            .await,
        AssistantCode::Invalid
    );

    // on_screen passes on only an exposed object, as ids
    let (status, _) = world
        .web(
            "POST",
            "/ui/route",
            Some(json!({"route": "record", "object": ACCOUNT, "record_id": world.acme})),
        )
        .await;
    assert_eq!(status, 204);
    let Ok(AssistantResult::OnScreen(screen)) = world.ask(Operation::OnScreen).await else {
        panic!("on screen")
    };
    assert_eq!(screen.record.as_deref(), Some(world.acme.as_str()));
    let (_, secrets) = world
        .web("GET", &format!("/objects/{SECRET}/records"), None)
        .await;
    let vault = secrets["data"][0]["id"].as_str().expect("vault").to_owned();
    world
        .web(
            "POST",
            "/ui/route",
            Some(json!({"route": "record", "object": SECRET, "record_id": vault})),
        )
        .await;
    let Ok(AssistantResult::OnScreen(screen)) = world.ask(Operation::OnScreen).await else {
        panic!("on screen")
    };
    assert_eq!((screen.object, screen.record), (None, None));
}

#[tokio::test]
async fn the_policy_and_the_members_access_only_narrow() {
    let world = World::new().await;
    // Not exposed: every operation on it is refused the same way.
    for operation in [
        get(SECRET, "x"),
        Operation::List {
            object: SECRET.into(),
            conditions: Vec::new(),
            sort: None,
            cursor: None,
        },
        Operation::Actions {
            object: SECRET.into(),
            record: None,
        },
        Operation::Create {
            object: SECRET.into(),
            values: values(json!({"name": "New"})),
        },
        get("custom:missing", "x"),
    ] {
        assert_eq!(world.code(operation).await, AssistantCode::ObjectNotExposed);
    }
    let Err(error) = world.ask(get(SECRET, "x")).await else {
        panic!("refused")
    };
    assert_eq!(error.message, "Assistants can't reach Secrets in Limiara.");
    // An action the policy does not offer.
    assert_eq!(
        world
            .code(Operation::RunAction {
                object: ACCOUNT.into(),
                action: "quiet".into(),
                record: world.acme.clone(),
                values: Map::new(),
            })
            .await,
        AssistantCode::ActionNotOffered
    );
    assert_eq!(
        world
            .code(Operation::RunAction {
                object: ACCOUNT.into(),
                action: "missing".into(),
                record: world.acme.clone(),
                values: Map::new(),
            })
            .await,
        AssistantCode::ActionNotOffered
    );
    // Not writable: reads still work, writes are refused.
    world
        .set_policy(json!({"writable": false, "offered_actions": []}))
        .await;
    world.sync().await;
    assert!(world.ask(get(ACCOUNT, &world.acme)).await.is_ok());
    assert_eq!(
        world
            .code(Operation::Create {
                object: ACCOUNT.into(),
                values: values(json!({"name": "New"})),
            })
            .await,
        AssistantCode::NotWritable
    );
    assert_eq!(
        world
            .code(Operation::Update {
                object: ACCOUNT.into(),
                record: world.acme.clone(),
                values: values(json!({"custom:notes": "x"})),
            })
            .await,
        AssistantCode::NotWritable
    );
    // The workspace switch off: everything is disabled.
    world.set_switch(false).await;
    world.sync().await;
    assert_eq!(
        world.code(Operation::Describe).await,
        AssistantCode::Disabled
    );
    assert_eq!(
        world.code(get(ACCOUNT, &world.acme)).await,
        AssistantCode::Disabled
    );
    world.set_switch(true).await;
    world.sync().await;
    assert!(world.ask(Operation::Describe).await.is_ok());
    // The member's own setting on this device turns it off there.
    world
        .services
        .assistant()
        .set_device_settings(DeviceAssistantSettings {
            assistant_access: false,
            indexing: false,
        });
    assert_eq!(
        world.code(Operation::Describe).await,
        AssistantCode::Disabled
    );
}

#[tokio::test]
async fn field_level_security_applies_to_reads_and_writes() {
    let world = World::new().await;
    // A field the member cannot read is unknown to the assistant.
    let Err(error) = world
        .ask(Operation::Update {
            object: ACCOUNT.into(),
            record: world.acme.clone(),
            values: values(json!({"custom:salary": "1"})),
        })
        .await
    else {
        panic!("salary")
    };
    assert_eq!(error.code, AssistantCode::Invalid);
    assert_eq!(error.fields[0].message, "That field isn't available.");
    assert_eq!(
        world
            .code(Operation::List {
                object: ACCOUNT.into(),
                conditions: vec![Condition::IsEmpty {
                    field: "custom:salary".into()
                }],
                sort: None,
                cursor: None,
            })
            .await,
        AssistantCode::Invalid
    );
    // A field the member reads but may not change is refused by label.
    let Err(error) = world
        .ask(Operation::Update {
            object: ACCOUNT.into(),
            record: world.acme.clone(),
            values: values(json!({"custom:rating": 5})),
        })
        .await
    else {
        panic!("rating")
    };
    assert_eq!(error.code, AssistantCode::Invalid);
    assert_eq!(error.fields[0].message, "Rating can't be changed.");
    // Validation rules run in the dry run, with the rule's own message.
    let Err(error) = world
        .ask(Operation::Update {
            object: ACCOUNT.into(),
            record: world.acme.clone(),
            values: values(json!({"custom:amount": -5})),
        })
        .await
    else {
        panic!("rule")
    };
    assert_eq!(error.code, AssistantCode::Invalid);
    assert_eq!(error.fields[0].label, "Amount");
    assert_eq!(error.fields[0].message, "Amount can't be negative.");
    // A required value is asked for by label.
    let Err(error) = world
        .ask(Operation::Create {
            object: ACCOUNT.into(),
            values: values(json!({"custom:notes": "no name"})),
        })
        .await
    else {
        panic!("name")
    };
    assert_eq!(error.code, AssistantCode::Invalid, "{error:?}");
    assert!(
        error.fields.iter().any(|field| field.field == "name"),
        "{error:?}"
    );
}

#[tokio::test]
async fn a_confirmation_commits_exactly_once_and_only_unchanged() {
    let world = World::new().await;
    let prepare_update = |notes: &str| Operation::Update {
        object: ACCOUNT.into(),
        record: world.acme.clone(),
        values: values(json!({"custom:notes": notes})),
    };
    // A prepare writes nothing.
    let before = world.device_record(&world.acme).await;
    let confirmation = world.prepare(prepare_update("Prepared")).await;
    let create = world
        .prepare(Operation::Create {
            object: ACCOUNT.into(),
            values: values(json!({"name": "Dry"})),
        })
        .await;
    assert_eq!(world.device_record(&world.acme).await, before);
    assert!(world.engine.push_queue().await.expect("queue").is_empty());
    let (_, listed) = world
        .web("GET", &format!("/objects/{ACCOUNT}/records"), None)
        .await;
    assert_eq!(listed["data"].as_array().map(Vec::len), Some(1));

    // Used.
    world.commit(&confirmation.token).await.expect("first");
    assert_eq!(
        world.commit(&confirmation.token).await.map_err(|e| e.code),
        Err(AssistantCode::ConfirmationInvalid)
    );
    // No commit without a prepare: there is no write operation but commit,
    // and only a token the bridge issued commits.
    for token in [
        "",
        "garbage",
        &create.token[..20],
        &create.token.replace('A', "B"),
    ] {
        if token == create.token {
            continue;
        }
        assert_eq!(
            world.commit(token).await.map_err(|e| e.code),
            Err(AssistantCode::ConfirmationInvalid),
            "{token}"
        );
    }
    // Expired after two minutes.
    world.clock.1.store(121, Ordering::SeqCst);
    assert_eq!(
        world.commit(&create.token).await.map_err(|e| e.code),
        Err(AssistantCode::ConfirmationInvalid)
    );
    world.clock.1.store(0, Ordering::SeqCst);

    // Stale record: it changed after the member confirmed.
    let confirmation = world.prepare(prepare_update("Mine")).await;
    let version = world.device_record(&world.acme).await["version"].clone();
    let (status, _) = world
        .web(
            "PATCH",
            &format!("/objects/{ACCOUNT}/records/{}", world.acme),
            Some(json!({"expected_version": version, "custom:notes": "Theirs"})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(
        world.commit(&confirmation.token).await.map_err(|e| e.code),
        Err(AssistantCode::ConfirmationStale)
    );
    assert_eq!(
        world.device_record(&world.acme).await["custom:notes"],
        "Theirs"
    );

    // Stale metadata: the definitions moved.
    let confirmation = world.prepare(prepare_update("Later")).await;
    let account = world.admin.describe(ACCOUNT).await.expect("account").0;
    world
        .admin
        .update_object(
            ACCOUNT,
            serde_json::from_value(json!({"expected_version": account.version,
                "label": "Customer"}))
            .expect("update"),
            context(),
        )
        .await
        .expect("relabel");
    world.sync().await;
    let Err(error) = world.commit(&confirmation.token).await else {
        panic!("stale")
    };
    assert_eq!(error.code, AssistantCode::ConfirmationStale);
    assert_eq!(error.message, AssistantCode::ConfirmationStale.message());
    // A stale token is spent too.
    assert_eq!(
        world.commit(&confirmation.token).await.map_err(|e| e.code),
        Err(AssistantCode::ConfirmationInvalid)
    );
    // Sign-out forgets every prepared change.
    let confirmation = world.prepare(prepare_update("Gone")).await;
    world.services.assistant().closing();
    world.services.assistant().open(&world.bridge, None);
    assert_eq!(
        world.commit(&confirmation.token).await.map_err(|e| e.code),
        Err(AssistantCode::ConfirmationInvalid)
    );
}

#[tokio::test]
async fn the_push_marker_reaches_the_server_and_its_policy_rechecks_it() {
    let world = World::new().await;
    // An assistant's create pushes marked; the server records it as Siri's.
    let confirmation = world
        .prepare(Operation::Create {
            object: ACCOUNT.into(),
            values: values(json!({"name": "Initech", "custom:notes": "via Siri"})),
        })
        .await;
    let created = world.commit(&confirmation.token).await.expect("created");
    let batch = world.engine.push_queue().await.expect("queue");
    assert_eq!(
        batch.request().operations[0].assistant(),
        Some(&AssistantMarker {
            assistant: Assistant::Siri,
            quick_action: None
        })
    );
    let report = world.sync().await;
    assert!(
        report.pushed.iter().all(|pushed| pushed.failed.is_empty()),
        "{report:?}"
    );
    let (history, _) = world
        .admin
        .record_history(ACCOUNT, &created.id, None, None)
        .await
        .expect("history");
    assert!(!history.entries.is_empty());
    assert!(history
        .entries
        .iter()
        .all(|entry| entry.source == Some(MutationSource::assistant(Assistant::Siri))));
    // Once pushed, the mark is gone.
    assert!(world.engine.push_queue().await.expect("queue").is_empty());

    // A quick action carries its name.
    let confirmation = world
        .prepare(Operation::RunAction {
            object: ACCOUNT.into(),
            action: "note".into(),
            record: world.acme.clone(),
            values: values(json!({"custom:notes": "Called"})),
        })
        .await;
    world.commit(&confirmation.token).await.expect("noted");
    let batch = world.engine.push_queue().await.expect("queue");
    assert_eq!(
        batch.request().operations[0].assistant(),
        Some(&AssistantMarker {
            assistant: Assistant::Siri,
            quick_action: Some(MarkedQuickAction {
                object: ACCOUNT.into(),
                name: "note".into()
            })
        })
    );
    world.sync().await;

    // The device wrote while the policy allowed it; before the push, the
    // server's policy changed. The server's re-check refuses it.
    let confirmation = world
        .prepare(Operation::Update {
            object: ACCOUNT.into(),
            record: world.acme.clone(),
            values: values(json!({"custom:notes": "Offline change"})),
        })
        .await;
    world.commit(&confirmation.token).await.expect("updated");
    world
        .set_policy(json!({"writable": false, "offered_actions": []}))
        .await;
    let report = world.sync().await;
    let failed: Vec<&str> = report
        .pushed
        .iter()
        .flat_map(|pushed| pushed.failed.iter().map(|(_, _, code)| code.as_str()))
        .collect();
    assert_eq!(failed, ["assistant.not_allowed"], "{report:?}");
    let on_server = world
        .admin
        .get_record(ACCOUNT, &world.acme, false)
        .await
        .expect("acme");
    assert_ne!(
        on_server.values.get("custom:notes"),
        Some(&json!("Offline change"))
    );
    // The member's own web-view write is not the policy's business.
    let created = world
        .web(
            "POST",
            &format!("/objects/{ACCOUNT}/records"),
            Some(json!({"name": "By hand"})),
        )
        .await;
    assert_eq!(created.0, 201);
    let report = world.sync().await;
    assert!(report
        .pushed
        .iter()
        .flat_map(|pushed| pushed.failed.iter())
        .all(|(_, id, _)| id != created.1["id"].as_str().unwrap_or_default()));
    let typed = created.1["id"].as_str().expect("id");
    assert!(world.admin.get_record(ACCOUNT, typed, false).await.is_ok());
    let _ = world.device;
}

#[tokio::test]
async fn every_unavailable_state_answers_its_code_and_sentence() {
    let world = World::new().await;
    let sentence = |code: AssistantCode| code.message();
    // The device is locked: refused before anything is read.
    let answer = world
        .assistant
        .call(AssistantRequest::new(
            Assistant::Siri,
            false,
            Operation::Describe,
        ))
        .await
        .into_result();
    let Err(error) = answer else { panic!("locked") };
    assert_eq!(error.code, AssistantCode::DeviceLocked);
    assert_eq!(error.message, "Unlock your device first.");
    // Disabled on the device.
    world
        .services
        .assistant()
        .set_device_settings(DeviceAssistantSettings {
            assistant_access: false,
            indexing: false,
        });
    let Err(error) = world.ask(Operation::Describe).await else {
        panic!("disabled")
    };
    assert_eq!(
        (error.code, error.message.as_str()),
        (
            AssistantCode::Disabled,
            "Assistant access is off for this workspace."
        )
    );
    world
        .services
        .assistant()
        .set_device_settings(DeviceAssistantSettings::default());
    // Past the offline lease.
    world.engine.set_offline_limit(1).await.expect("limit");
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    let Err(error) = world.ask(get(ACCOUNT, &world.acme)).await else {
        panic!("lease")
    };
    assert_eq!(error.code, AssistantCode::Locked);
    assert_eq!(
        error.message,
        "Open Limiara while online so it can check your access."
    );
    world.engine.set_offline_limit(3_600).await.expect("limit");
    world.sync().await;
    assert!(world.ask(get(ACCOUNT, &world.acme)).await.is_ok());
    // Sign-out in progress, then signed out.
    world.services.assistant().closing();
    assert_eq!(
        world.code(Operation::Describe).await,
        AssistantCode::SignedOut
    );
    world.services.assistant().drained().await;
    world.services.assistant().signed_out();
    let Err(error) = world.ask(Operation::Describe).await else {
        panic!("signed out")
    };
    assert_eq!(error.message, sentence(AssistantCode::SignedOut));
    assert_eq!(error.message, "Sign in to Limiara on this device first.");

    // Storage that cannot be opened.
    struct NoKeys;
    #[async_trait]
    impl platform_device::DatabaseKeys for NoKeys {
        async fn key(
            &self,
            _: Uuid,
        ) -> Result<platform_device::DatabaseKey, platform_device::KeyError> {
            Err(platform_device::KeyError::Unavailable)
        }
        async fn destroy(&self, _: Uuid) -> Result<(), platform_device::KeyError> {
            Ok(())
        }
    }
    let directory = tempfile::tempdir().expect("dir");
    let locked = ObjectService::new(
        Arc::new(SqliteObjectStore::new(platform_device::device_databases(
            directory.path(),
            Arc::new(NoKeys),
        ))),
        ObjectCaller::new(world.workspace, Uuid::now_v7(), BTreeSet::new(), Vec::new()),
    )
    .with_replica();
    let bridge = Bridge::new(DeviceHost::new(
        locked,
        Member {
            display_name: "Ada".into(),
            email: None,
            permissions: BTreeSet::new(),
            roles: BTreeSet::new(),
        },
        Workspace { name: "W".into() },
    ));
    world.services.assistant().open(&bridge, None);
    let Err(error) = world.ask(Operation::Describe).await else {
        panic!("unavailable")
    };
    assert_eq!(error.code, AssistantCode::Unavailable);
    assert_eq!(
        error.message,
        "Limiara can't open this workspace right now."
    );

    // Every code has its sentence, and none carries a code or an id.
    for code in AssistantCode::ALL {
        let message = code.message();
        assert!(!message.contains("assistant.") && !message.contains("custom:"));
    }
}

#[tokio::test]
async fn not_found_and_no_access_answer_the_same() {
    let world = World::new().await;
    let answers = |record: String| {
        let world = &world;
        async move {
            let mut out = Vec::new();
            for operation in [
                get(ACCOUNT, &record),
                Operation::Timeline {
                    object: ACCOUNT.into(),
                    record: record.clone(),
                    cursor: None,
                },
                Operation::Actions {
                    object: ACCOUNT.into(),
                    record: Some(record.clone()),
                },
                Operation::Update {
                    object: ACCOUNT.into(),
                    record: record.clone(),
                    values: values(json!({"custom:notes": "x"})),
                },
                Operation::RunAction {
                    object: ACCOUNT.into(),
                    action: "note".into(),
                    record: record.clone(),
                    values: Map::new(),
                },
            ] {
                let answer = world
                    .assistant
                    .call(AssistantRequest::new(Assistant::Siri, true, operation))
                    .await;
                out.push(serde_json::to_value(&answer).expect("json"));
            }
            out
        }
    };
    let hidden = answers(world.hidden.clone()).await;
    let prefix = world.acme.split('-').next().unwrap_or_default().to_owned();
    let missing = answers(format!("{prefix}-zzzzzzzzzz")).await;
    let malformed = answers("not-an-id".into()).await;
    assert_eq!(hidden, missing);
    assert_eq!(hidden, malformed);
    for answer in &hidden {
        assert_eq!(answer["error"]["code"], "assistant.not_found", "{answer}");
        assert_eq!(
            answer["error"]["message"],
            "I couldn't find that record in Limiara."
        );
    }
}

/// The updates that change what the index holds: without the expiry
/// refreshes each sync adds.
async fn changes(stream: &mut IndexStream) -> Vec<IndexUpdate> {
    drain(stream)
        .await
        .into_iter()
        .filter(|update| !matches!(update, IndexUpdate::Refresh { .. }))
        .collect()
}

/// Every update the stream has ready, waiting briefly for each.
async fn drain(stream: &mut IndexStream) -> Vec<IndexUpdate> {
    let mut updates = Vec::new();
    while let Some(update) = stream.next_within(WAIT).await {
        updates.push(update);
    }
    updates
}

fn upserts(updates: &[IndexUpdate]) -> Vec<&platform_device::assistant::index::IndexEntry> {
    updates
        .iter()
        .filter_map(|update| match update {
            IndexUpdate::Upsert { entry } => Some(entry),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn the_index_stream_follows_policy_access_and_the_device() {
    let world = World::new().await;
    let mut stream = world.assistant.index_stream();
    // Indexing is off on the device until the member turns it on.
    assert!(upserts(&drain(&mut stream).await).is_empty());
    let sweep = world.assistant.sweep(&[world.workspace, Uuid::nil()]);
    assert_eq!(sweep.remove_workspaces.len(), 2, "nothing may stay indexed");
    world
        .services
        .assistant()
        .set_device_settings(DeviceAssistantSettings {
            assistant_access: true,
            indexing: true,
        });
    let updates = drain(&mut stream).await;
    assert_eq!(
        updates.first(),
        Some(&IndexUpdate::Rebuild {
            workspace: world.workspace
        })
    );
    let entries = upserts(&updates);
    assert_eq!(entries.len(), 1, "{updates:?}");
    let acme = entries[0];
    assert_eq!(acme.record, world.acme);
    assert_eq!(acme.name.as_deref(), Some("Acme"));
    assert_eq!(acme.object_label, "Account");
    let indexed: Vec<&str> = acme
        .fields
        .iter()
        .map(|field| field.field.as_str())
        .collect();
    assert!(indexed.contains(&"custom:amount"), "{indexed:?}");
    assert!(!indexed.contains(&"custom:notes"), "excluded by the policy");
    assert!(
        !indexed.contains(&"custom:salary"),
        "unreadable by the member"
    );
    let lease = chrono::Duration::seconds(3_600);
    assert!(acme.expires_at > chrono::Utc::now() + lease - chrono::Duration::minutes(1));
    assert!(acme.expires_at <= chrono::Utc::now() + lease);
    // The launch-time sweep keeps only the workspace the shell holds.
    let sweep = world.assistant.sweep(&[world.workspace, Uuid::nil()]);
    assert_eq!(sweep.remove_workspaces, vec![Uuid::nil()]);

    // A write adds an entry; a delete removes it.
    let confirmation = world
        .prepare(Operation::Create {
            object: ACCOUNT.into(),
            values: values(json!({"name": "Umbrella"})),
        })
        .await;
    let created = world.commit(&confirmation.token).await.expect("created");
    let updates = drain(&mut stream).await;
    assert_eq!(
        upserts(&updates)
            .iter()
            .map(|entry| entry.record.as_str())
            .collect::<Vec<_>>(),
        [created.id.as_str()],
        "{updates:?}"
    );
    let record = world.device_record(&created.id).await;
    let (status, _) = world
        .web(
            "DELETE",
            &format!(
                "/objects/{ACCOUNT}/records/{}?expected_version={}",
                created.id, record["version"]
            ),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert!(drain(&mut stream).await.contains(&IndexUpdate::Remove {
        workspace: world.workspace,
        object: ACCOUNT.into(),
        record: created.id.clone(),
    }));

    // A sync that reaches the server moves every entry's expiry; the next
    // change carries it.
    world.sync().await;
    drain(&mut stream).await;
    tokio::time::sleep(Duration::from_millis(20)).await;
    world.sync().await;
    let version = world.device_record(&world.acme).await["version"].clone();
    world
        .web(
            "PATCH",
            &format!("/objects/{ACCOUNT}/records/{}", world.acme),
            Some(json!({"expected_version": version, "custom:amount": 1300})),
        )
        .await;
    let updates = drain(&mut stream).await;
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, IndexUpdate::Refresh { workspace, .. } if *workspace == world.workspace)),
        "{updates:?}"
    );
    let entry = upserts(&updates)[0].clone();
    assert!(entry
        .fields
        .iter()
        .any(|field| field.field == "custom:amount" && field.display == "1300.00"));

    // The edit pushes, and its answer is stored again.
    world.sync().await;
    drain(&mut stream).await;

    // Excluding the name rewrites the object's entries without it.
    world
        .set_policy(json!({"excluded_fields": ["custom:notes", "name"]}))
        .await;
    world.sync().await;
    let updates = drain(&mut stream).await;
    assert!(updates.contains(&IndexUpdate::RemoveObject {
        workspace: world.workspace,
        object: ACCOUNT.into()
    }));
    assert!(
        upserts(&updates).iter().all(|entry| entry.name.is_none()),
        "{updates:?}"
    );
    assert!(!upserts(&updates).is_empty());

    // Indexable off removes the object's entries.
    world.set_policy(json!({"indexable": false})).await;
    world.sync().await;
    assert_eq!(
        changes(&mut stream).await,
        [IndexUpdate::RemoveObject {
            workspace: world.workspace,
            object: ACCOUNT.into()
        }]
    );
    world
        .set_policy(json!({"indexable": true, "excluded_fields": ["custom:notes"]}))
        .await;
    world.sync().await;
    assert!(!upserts(&drain(&mut stream).await).is_empty());

    // A lease that locks keeps the index.
    world.engine.set_offline_limit(1).await.expect("limit");
    tokio::time::sleep(Duration::from_millis(1_200)).await;
    world.tap.check_lease(&world.engine).await.expect("lease");
    let updates = drain(&mut stream).await;
    assert!(
        updates.iter().all(|update| !matches!(
            update,
            IndexUpdate::RemoveWorkspace { .. }
                | IndexUpdate::RemoveObject { .. }
                | IndexUpdate::Rebuild { .. }
        )),
        "{updates:?}"
    );
    world.engine.set_offline_limit(3_600).await.expect("limit");
    world.sync().await;
    drain(&mut stream).await;

    // The workspace switch off removes the workspace's entries.
    world.set_switch(false).await;
    world.sync().await;
    assert_eq!(
        changes(&mut stream).await,
        [IndexUpdate::RemoveWorkspace {
            workspace: world.workspace
        }]
    );
    world.set_switch(true).await;
    world.sync().await;
    assert!(!upserts(&drain(&mut stream).await).is_empty());

    // The member turning indexing off removes them too.
    world
        .services
        .assistant()
        .set_device_settings(DeviceAssistantSettings::default());
    assert_eq!(
        changes(&mut stream).await,
        [IndexUpdate::RemoveWorkspace {
            workspace: world.workspace
        }]
    );
    world
        .services
        .assistant()
        .set_device_settings(DeviceAssistantSettings {
            assistant_access: true,
            indexing: true,
        });
    assert!(!upserts(&drain(&mut stream).await).is_empty());

    // Sign-out clears everything, before the replica goes.
    world
        .services
        .feed()
        .clear(&world.tap, ClearReason::SignedOut);
    world.services.assistant().closing();
    assert_eq!(
        changes(&mut stream).await,
        [IndexUpdate::RemoveWorkspace {
            workspace: world.workspace
        }]
    );
    world.services.assistant().signed_out();
    let sweep = world.assistant.sweep(&[world.workspace]);
    assert_eq!(sweep.remove_workspaces, vec![world.workspace]);

    // A wipe does the same for a workspace opened again.
    let tap = world.services.feed().open(world.workspace);
    world.services.assistant().open(&world.bridge, None);
    assert!(!upserts(&drain(&mut stream).await).is_empty());
    world.services.feed().clear(&tap, ClearReason::Wiped);
    world.services.assistant().closing();
    assert_eq!(
        changes(&mut stream).await,
        [IndexUpdate::RemoveWorkspace {
            workspace: world.workspace
        }]
    );
}
