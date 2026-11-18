//! The reference world: a workspace server and a device that syncs from
//! it over real HTTP into an encrypted SQLite replica, with the shell's
//! local services and a native plugin's `device.assistant` grant.
//!
//! The server is the object routes the workspace API serves, over a SQLite
//! workspace, behind a stand-in for its authentication that treats every
//! token as a synced device's. It listens on an ephemeral loopback port and
//! stops with the world.
//!
//! The reference workspace (its API names are what the scripts use):
//!
//! - `custom:account` ("Account", "Accounts"), private sharing, exposed,
//!   writable and indexable with `custom:notes` excluded from the index.
//!   Fields: `custom:notes` (text, indexed, searchable), `custom:amount`
//!   (currency, 2 places, indexed), `custom:stage` (picklist `open` "Open",
//!   `won` "Closed won"), `custom:parent` (lookup to accounts),
//!   `custom:salary` (text the member cannot read), `custom:rating`
//!   (number the member cannot change), `custom:summary` (long text, not
//!   indexed), `custom:code` (unique text), `custom:seen_at` (a formula,
//!   `NOW()`, computed again at every read). A validation rule refuses a
//!   negative amount with "Amount can't be negative.". Quick actions:
//!   `note` (update, fields notes and stage, preset stage `won`, offered),
//!   `add_branch` (create an account under this one, field name, preset
//!   notes "branch office", offered) and `quiet` (not offered).
//! - `custom:secret` ("Secret", "Secrets"), readable but not exposed.
//! - `custom:region` ("Region", "Regions"), exposed and writable, which the
//!   member may read but not create.
//!
//! Records: `acme` (the member's "Acme", amount 1200, stage open, notes
//! "Key account", code "ACME", a salary of 90000 hidden from the member,
//! rating 4), `hidden` (a colleague's "Hidden Co" the member cannot read)
//! and `vault` (the member's secret "Vault").

use crate::clock::HarnessClock;
use async_trait::async_trait;
use axum::extract::{Request, State};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use platform_device::assistant::AssistantBridge;
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
use std::sync::Arc;
use tower_http::request_id::{MakeRequestUuid, SetRequestIdLayer};
use uuid::Uuid;

pub const ACCOUNT: &str = "custom:account";
pub const SECRET: &str = "custom:secret";
pub const REGION: &str = "custom:region";
const READ: &str = "hr.salary.read";
const WRITE: &str = "hr.rating.write";
/// The offline lease the server gives the device, in seconds.
pub const OFFLINE_LIMIT: u64 = 3_600;
/// The plugin the harness plays.
pub const PLUGIN: &str = "assistant-harness";

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
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

/// The device every token belongs to.
type Authority = Arc<Uuid>;

/// The access token is the member's id; every request is a synced
/// device's, as the server verifies its token.
async fn authenticate(
    State(device): State<Authority>,
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
        permissions: BTreeSet::new(),
        source: MutationSource::device(Some(*device)),
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

/// A key store that can never give the key.
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

/// The device's open replica.
pub struct Replica {
    pub engine: ObjectService,
    pub client: SyncClient<HttpTransport>,
    pub bridge: Bridge,
    pub tap: FeedTap,
    /// The replica's directory, deleted at sign-out and wipe.
    directory: std::path::PathBuf,
}

/// Where the device stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceState {
    Open,
    /// A sign-out started and is not finished.
    Closing,
    SignedOut,
    /// The bridge answers from a workspace whose key is unavailable.
    Unavailable,
}

pub struct World {
    _server_directory: tempfile::TempDir,
    device_directory: tempfile::TempDir,
    server: tokio::task::JoinHandle<()>,
    address: std::net::SocketAddr,
    pub workspace: Uuid,
    pub member: Uuid,
    pub device: Uuid,
    role: Uuid,
    pub admin: ObjectService,
    store: Arc<dyn ObjectStore>,
    pub services: LocalServices,
    pub assistant: AssistantBridge,
    pub platform: Assistant,
    pub clock: Arc<HarnessClock>,
    pub replica: Option<Replica>,
    state: DeviceState,
    sign_ins: u32,
    offline_limit: u64,
    /// The fixture's records, by name.
    pub records: BTreeMap<String, String>,
}

impl Drop for World {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn field(value: Value) -> anyhow::Result<CreateField> {
    Ok(serde_json::from_value(value)?)
}

fn values(value: Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

impl World {
    /// The reference workspace, served and synced to a device whose
    /// platform has `platform` as its assistant.
    pub async fn new(platform: Assistant) -> anyhow::Result<Self> {
        let server_directory = tempfile::tempdir()?;
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
            ("region", "Region", "Regions"),
        ] {
            admin
                .create_object(
                    serde_json::from_value(json!({"name": name, "label": label,
                        "plural_label": plural, "sharing_model": "private",
                        "track_history": true, "searchable": true, "in_navigation": true}))?,
                    context(),
                )
                .await?;
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
            json!({"name": "code", "label": "Code", "field_type": "text", "unique": true}),
            json!({"name": "seen_at", "label": "Seen at", "field_type": "formula",
                   "formula": "NOW()", "formula_type": "date_time"}),
        ] {
            admin
                .create_field(ACCOUNT, field(definition)?, context())
                .await?;
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
            .await?;
        let full = AccessFlags {
            read: true,
            create: true,
            edit: true,
            delete: true,
            ..AccessFlags::default()
        };
        for (object, flags) in [
            (ACCOUNT, full),
            (SECRET, full),
            (
                REGION,
                AccessFlags {
                    read: true,
                    ..AccessFlags::default()
                },
            ),
        ] {
            admin
                .set_object_permission(object, role, flags, context())
                .await?;
        }
        for action in [
            json!({"name": "note", "label": "Add a note", "kind": "update",
                   "fields": ["custom:notes", "custom:stage"],
                   "presets": {"custom:stage": "won"}}),
            json!({"name": "add_branch", "label": "Add branch", "kind": "create",
                   "target_object": ACCOUNT, "target_field": "custom:parent",
                   "fields": ["name"], "presets": {"custom:notes": "branch office"}}),
            json!({"name": "quiet", "label": "Quiet", "kind": "update",
                   "fields": ["custom:notes"]}),
        ] {
            admin
                .create_quick_action(ACCOUNT, serde_json::from_value(action)?, context())
                .await?;
        }
        admin
            .update_assistant_settings(
                UpdateAssistantSettings {
                    expected_version: 0,
                    enabled: true,
                },
                context(),
            )
            .await?;
        admin
            .update_object_assistant_policy(
                ACCOUNT,
                serde_json::from_value(json!({"expected_version": 0, "exposed": true,
                    "writable": true, "indexable": true,
                    "excluded_fields": ["custom:notes"],
                    "offered_actions": ["note", "add_branch"]}))?,
                context(),
            )
            .await?;
        admin
            .update_object_assistant_policy(
                REGION,
                serde_json::from_value(json!({"expected_version": 0, "exposed": true,
                    "writable": true}))?,
                context(),
            )
            .await?;

        let member = Uuid::now_v7();
        let colleague = Uuid::now_v7();
        let as_member = |permissions: BTreeSet<String>| {
            ObjectService::new(
                store.clone(),
                ObjectCaller::new(workspace, member, permissions, vec![role]),
            )
        };
        let acme = as_member(set(&[READ, WRITE]))
            .create_record(
                ACCOUNT,
                &values(json!({"name": "Acme", "custom:notes": "Key account",
                    "custom:amount": 1200, "custom:stage": "open", "custom:code": "ACME",
                    "custom:salary": "90000", "custom:rating": 4})),
                context(),
            )
            .await?
            .public_id;
        let hidden = ObjectService::new(
            store.clone(),
            ObjectCaller::new(workspace, colleague, BTreeSet::new(), vec![role]),
        )
        .create_record(ACCOUNT, &values(json!({"name": "Hidden Co"})), context())
        .await?
        .public_id;
        let vault = as_member(BTreeSet::new())
            .create_record(SECRET, &values(json!({"name": "Vault"})), context())
            .await?
            .public_id;

        let device = Uuid::now_v7();
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
            .layer(middleware::from_fn_with_state(
                Arc::new(device),
                authenticate,
            ))
            .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
            .with_state(Server {
                store: store.clone(),
                role,
            });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let clock = Arc::new(HarnessClock::new());
        let services = LocalServices::with_assistant(
            ReplicaFeed::new(),
            RouteContext::new(),
            Some(platform),
            clock.clone(),
        );
        let assistant = services
            .grant(&PluginDeclaration::new(PLUGIN, &["device.assistant"])?)
            .assistant()?;
        let mut world = Self {
            _server_directory: server_directory,
            device_directory: tempfile::tempdir()?,
            server,
            address,
            workspace,
            member,
            device,
            role,
            admin,
            store,
            services,
            assistant,
            platform,
            clock,
            replica: None,
            state: DeviceState::SignedOut,
            sign_ins: 0,
            offline_limit: OFFLINE_LIMIT,
            records: BTreeMap::from([
                ("acme".to_owned(), acme),
                ("hidden".to_owned(), hidden),
                ("vault".to_owned(), vault),
            ]),
        };
        world.sign_in().await?;
        Ok(world)
    }

    /// Signs in: a fresh replica, synced for the first time, opened to the
    /// bridge. After `storage_unavailable` it reopens the replica it has.
    pub async fn sign_in(&mut self) -> anyhow::Result<()> {
        if let Some(replica) = &self.replica {
            anyhow::ensure!(self.state == DeviceState::Unavailable, "already signed in");
            self.services.assistant().open(&replica.bridge, None);
            self.state = DeviceState::Open;
            return Ok(());
        }
        anyhow::ensure!(self.state == DeviceState::SignedOut, "sign out first");
        self.sign_ins += 1;
        let directory = self
            .device_directory
            .path()
            .join(format!("sign-in-{}", self.sign_ins));
        std::fs::create_dir_all(&directory)?;
        let files = platform_device::device_databases(
            &directory,
            Arc::new(platform_device::MemoryKeys::new()),
        );
        let tap = self.services.feed().open(self.workspace);
        let engine = ObjectService::new(
            Arc::new(SqliteObjectStore::new(files)),
            ObjectCaller::new(
                self.workspace,
                self.member,
                BTreeSet::new(),
                vec![self.role],
            ),
        )
        .with_replica()
        .with_client_ids()
        .with_trusted_clock(self.clock.clone())
        .with_replica_observer(Arc::new(tap.clone()));
        let transport = HttpTransport::new(
            format!("http://{}", self.address),
            self.workspace,
            Arc::new(Token(self.member)),
        )?;
        let client = SyncClient::new(engine.clone(), transport);
        engine.set_offline_limit(self.offline_limit).await?;
        client.sync_once(context()).await?;
        tap.start(&engine).await?;
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
            .with_device(self.device)
            .with_feed(tap.clone())
            .with_route_context(self.services.routes().clone()),
        );
        self.services.assistant().open(&bridge, None);
        self.replica = Some(Replica {
            engine,
            client,
            bridge,
            tap,
            directory,
        });
        self.state = DeviceState::Open;
        Ok(())
    }

    pub fn replica(&self) -> anyhow::Result<&Replica> {
        self.replica
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the device is signed out"))
    }

    /// Sign-out or a wipe starts, as the controller does it: the feed's
    /// last event for the workspace, then the bridge closes.
    pub fn begin_close(&mut self, reason: ClearReason) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(self.state, DeviceState::Open | DeviceState::Unavailable),
            "nothing to sign out of"
        );
        let replica = self.replica()?;
        self.services.feed().clear(&replica.tap, reason);
        self.services.assistant().closing();
        self.state = DeviceState::Closing;
        Ok(())
    }

    /// Sign-out or a wipe finishes: the calls already running end, then
    /// the replica's files go.
    pub async fn close(&mut self, reason: ClearReason) -> anyhow::Result<()> {
        if self.state != DeviceState::Closing {
            self.begin_close(reason)?;
        }
        self.services.assistant().drained().await;
        self.services.assistant().signed_out();
        if let Some(replica) = self.replica.take() {
            drop(replica.bridge);
            drop(replica.client);
            drop(replica.engine);
            let _ = std::fs::remove_dir_all(&replica.directory);
        }
        self.state = DeviceState::SignedOut;
        Ok(())
    }

    /// The bridge answers from a workspace whose key cannot be had.
    pub fn storage_unavailable(&mut self) -> anyhow::Result<()> {
        let directory = self.device_directory.path().join("no-keys");
        std::fs::create_dir_all(&directory)?;
        let locked = ObjectService::new(
            Arc::new(SqliteObjectStore::new(platform_device::device_databases(
                &directory,
                Arc::new(NoKeys),
            ))),
            ObjectCaller::new(self.workspace, self.member, BTreeSet::new(), Vec::new()),
        )
        .with_replica();
        let bridge = Bridge::new(DeviceHost::new(
            locked,
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
        self.services.assistant().open(&bridge, None);
        self.state = DeviceState::Unavailable;
        Ok(())
    }

    pub async fn set_offline_limit(&mut self, seconds: u64) -> anyhow::Result<()> {
        self.offline_limit = seconds;
        self.replica()?.engine.set_offline_limit(seconds).await?;
        Ok(())
    }

    pub async fn check_lease(&self) -> anyhow::Result<Value> {
        let replica = self.replica()?;
        let lease = replica.tap.check_lease(&replica.engine).await?;
        Ok(json!({"locked": lease.locked}))
    }

    pub async fn sync(&self) -> anyhow::Result<Value> {
        let report = self.replica()?.client.sync_once(context()).await?;
        let failed: Vec<Value> = report
            .pushed
            .iter()
            .flat_map(|pushed| pushed.failed.iter())
            .map(|(object, record, code)| json!({"object": object, "record": record, "code": code}))
            .collect();
        Ok(json!({"failed": failed, "pulled": report.pulled}))
    }

    pub async fn push_queue(&self) -> anyhow::Result<Value> {
        let batch = self.replica()?.engine.push_queue().await?;
        let request = serde_json::to_value(batch.request())?;
        Ok(json!({"operations": request.get("operations").cloned().unwrap_or(json!([]))}))
    }

    pub async fn set_switch(&self, enabled: bool) -> anyhow::Result<()> {
        let current = self.admin.assistant_policy().await?;
        self.admin
            .update_assistant_settings(
                UpdateAssistantSettings {
                    expected_version: current.settings.version,
                    enabled,
                },
                context(),
            )
            .await?;
        Ok(())
    }

    pub async fn set_policy(
        &self,
        object: &str,
        change: &Map<String, Value>,
    ) -> anyhow::Result<()> {
        let current = self.admin.object_assistant_policy(object).await?;
        let mut body = change.clone();
        body.insert("expected_version".into(), json!(current.version));
        self.admin
            .update_object_assistant_policy(
                object,
                serde_json::from_value(Value::Object(body))?,
                context(),
            )
            .await?;
        Ok(())
    }

    pub async fn relabel(&self, object: &str, label: &str) -> anyhow::Result<()> {
        let current = self.admin.describe(object).await?.0;
        self.admin
            .update_object(
                object,
                serde_json::from_value(
                    json!({"expected_version": current.version, "label": label}),
                )?,
                context(),
            )
            .await?;
        Ok(())
    }

    pub async fn server_update(
        &self,
        object: &str,
        record: &str,
        values: &Map<String, Value>,
    ) -> anyhow::Result<Value> {
        let current = self.admin.get_record(object, record, false).await?;
        let updated = self
            .admin
            .update_record(object, record, current.version, values, context())
            .await?;
        Ok(json!({"version": updated.version}))
    }

    pub async fn server_record(&self, object: &str, record: &str) -> anyhow::Result<Value> {
        Ok(match self.admin.get_record(object, record, false).await {
            Ok(found) => json!({"found": true, "name": found.name, "values": found.values,
                                "version": found.version}),
            Err(_) => json!({"found": false}),
        })
    }

    pub async fn history(
        &self,
        on_server: bool,
        object: &str,
        record: &str,
    ) -> anyhow::Result<Value> {
        let engine = if on_server {
            &self.admin
        } else {
            &self.replica()?.engine
        };
        let (page, _) = engine.record_history(object, record, None, None).await?;
        let sources: Vec<Value> = page
            .entries
            .iter()
            .map(|entry| serde_json::to_value(entry.source).unwrap_or(Value::Null))
            .collect();
        Ok(json!({"sources": sources}))
    }

    /// A request from the web shell through the local command bridge.
    pub async fn web(
        &self,
        method: &str,
        rest: &str,
        body: Option<&Value>,
    ) -> anyhow::Result<Value> {
        let mut request = http::Request::builder()
            .method(method)
            .uri(format!("/api/v1/workspaces/{}{rest}", self.workspace));
        let bytes = match body {
            Some(body) => {
                request = request.header("content-type", "application/json");
                serde_json::to_vec(body)?
            }
            None => Vec::new(),
        };
        let response = self.replica()?.bridge.handle(request.body(bytes)?).await;
        let status = response.status().as_u16();
        let body = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Ok(json!({"status": status, "body": body}))
    }

    /// The variables every script starts with.
    pub fn variables(&self) -> BTreeMap<String, Value> {
        let other = match self.platform {
            Assistant::Siri => Assistant::Gemini,
            Assistant::Gemini => Assistant::Siri,
        };
        let mut variables: BTreeMap<String, Value> = self
            .records
            .iter()
            .map(|(name, id)| (name.clone(), json!(id)))
            .collect();
        variables.insert("workspace".into(), json!(self.workspace));
        variables.insert("other_workspace".into(), json!(Uuid::nil()));
        variables.insert("member".into(), json!(self.member));
        variables.insert("device".into(), json!(self.device));
        variables.insert("assistant".into(), json!(self.platform.as_str()));
        variables.insert("other_assistant".into(), json!(other.as_str()));
        variables.insert("offline_limit".into(), json!(self.offline_limit));
        variables
    }

    /// The server's object store, for checks beyond the admin's engine.
    pub fn server_store(&self) -> &Arc<dyn ObjectStore> {
        &self.store
    }
}
