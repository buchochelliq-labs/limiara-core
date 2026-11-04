//! The sync loop against a server engine in the same process (ADR 0024
//! §3–4). `LocalTransport` answers as the workspace API does, with the
//! same JSON shapes (`crates/platform-objects-http/src/handlers.rs`), for the member as the
//! server sees them: sharing, object permissions and validation all apply
//! on the server's side. Both engines are SQLite workspaces in one store.

use async_trait::async_trait;
use platform_objects::composite::{CompositeOutcome, CompositeRequest};
use platform_objects::service::ListRecords;
use platform_objects::store::sqlite::{SqliteObjectStore, SQLITE_MIGRATIONS};
use platform_objects::*;
use platform_storage::sqlite::SqliteTenantDatabases;
use platform_sync::*;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use uuid::Uuid;

fn context() -> MutationContext {
    MutationContext::new(Uuid::now_v7())
}

fn body(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object body")
}

fn caller(tenant: Uuid, user: Uuid, permissions: &[&str], roles: &[Uuid]) -> ObjectCaller {
    ObjectCaller::new(
        tenant,
        user,
        permissions
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        roles.to_vec(),
    )
}

fn user_reference(user: Option<Uuid>) -> Value {
    user.map_or(Value::Null, |id| json!({"id": id, "name": null}))
}

/// `record_json` with the caller's rights, as `GET .../records/{id}` sends.
fn record_json(object: &str, record: &Record) -> Value {
    let mut value = json!({
        "id": record.public_id,
        "object": object,
        "name": record.name,
        "owner": user_reference(record.owner_id),
        "created_at": record.created_at,
        "created_by": user_reference(record.created_by),
        "updated_at": record.updated_at,
        "updated_by": user_reference(record.updated_by),
        "touched_at": record.touched_at,
        "version": record.version,
        "archived_at": record.archived_at,
        "is_deleted": record.deleted_at.is_some(),
        "deleted_at": record.deleted_at,
    });
    let map = value.as_object_mut().expect("record");
    for (key, field_value) in &record.values {
        map.insert(key.clone(), field_value.clone());
    }
    map.insert("related".into(), json!({}));
    map.insert("access".into(), json!({"read": true}));
    value
}

fn field_json(field: &FieldDefinition) -> Value {
    json!({
        "api_name": field.api_name,
        "label": field.label,
        "description": field.description,
        "field_type": field.field_type,
        "origin": field.origin,
        "required": field.required,
        "unique": field.unique,
        "indexed": field.is_indexed(),
        "searchable": field.searchable,
        "track_history": field.track_history,
        "max_length": field.max_length,
        "scale": field.scale,
        "options": field.options,
        "default_value": field.default_value,
        "reference_object": field.reference_object,
        "delete_behaviour": field.delete_behaviour,
        "lookup_filter": field.lookup_filter,
        "formula": field.formula,
        "formula_type": field.formula_type,
        "rollup": field.rollup,
        "position": field.position,
        "version": field.version,
    })
}

fn object_json(object: &ObjectDefinition, access: AccessFlags, with_fields: bool) -> Value {
    let mut value = json!({
        "api_name": object.api_name,
        "label": object.label,
        "plural_label": object.plural_label,
        "description": object.description,
        "origin": object.origin,
        "owner_plugin": object.owner_plugin,
        "key_prefix": object.key_prefix,
        "name_label": object.name_label,
        "sharing_model": object.sharing_model,
        "grant_hierarchy": object.grant_hierarchy,
        "track_history": object.track_history,
        "searchable": object.searchable,
        "in_navigation": object.in_navigation,
        "version": object.version,
        "created_at": object.created_at,
        "updated_at": object.updated_at,
        "access": access,
    });
    if with_fields {
        value.as_object_mut().expect("object").insert(
            "fields".into(),
            Value::Array(object.fields.iter().map(field_json).collect()),
        );
    }
    value
}

fn problem(error: &ObjectError) -> (u16, Value) {
    let (status, code) = match error {
        ObjectError::NotFound => (404, "resource.unavailable"),
        ObjectError::PermissionDenied => (403, "auth.permission_denied"),
        ObjectError::Validation(_) => (400, "validation.failed"),
        ObjectError::VersionConflict => (409, "records.version_conflict"),
        ObjectError::Conflict(code) => (409, *code),
        _ => (500, "platform.unexpected"),
    };
    (status, json!({"status": status, "code": code}))
}

/// The workspace API for one member, in process.
struct LocalTransport {
    server: ObjectService,
    /// The next resumed change page answers `410 changes.position_expired`.
    expire: AtomicBool,
    /// Every call answers `401 auth.device_wipe_required`.
    wipe: AtomicBool,
    /// After the next composite request, the administrator adds a field
    /// and the member fills it in on this record, while the sync runs.
    define_during_push: std::sync::Mutex<Option<(ObjectService, String)>>,
    /// How long a stream stays open before the server ends it.
    stream_lifetime: std::sync::Mutex<std::time::Duration>,
    /// Streams opened so far.
    streams: std::sync::atomic::AtomicUsize,
    /// Change pages never answer, like a server that stopped responding.
    hang: AtomicBool,
    /// Every call fails, like a device without a connection.
    down: AtomicBool,
    /// The workspace's members and roles, as `GET /users` and `/roles` list them.
    directory: std::sync::Mutex<ReplicaDirectory>,
}

impl LocalTransport {
    fn check(&self) -> Result<(), SyncError> {
        if self.down.load(Ordering::Relaxed) {
            return Err(SyncError::Transport("no connection".into()));
        }
        if self.wipe.load(Ordering::Relaxed) {
            Err(SyncError::WipeRequired)
        } else {
            Ok(())
        }
    }
}

fn server_error(error: ObjectError) -> SyncError {
    let (status, body) = problem(&error);
    SyncError::Server {
        status,
        code: body["code"].as_str().unwrap_or_default().to_owned(),
    }
}

#[async_trait]
impl SyncTransport for LocalTransport {
    async fn objects(&self) -> Result<Vec<Value>, SyncError> {
        self.check()?;
        let objects = self.server.list_objects().await.map_err(server_error)?;
        Ok(objects
            .iter()
            .filter(|(_, access)| access.read)
            .map(|(object, access)| object_json(object, *access, false))
            .collect())
    }

    async fn describe(&self, api_name: &str) -> Result<Option<Value>, SyncError> {
        self.check()?;
        match self.server.describe(api_name).await {
            Ok((object, access)) => Ok(Some(object_json(&object, access, true))),
            Err(ObjectError::NotFound) => Ok(None),
            Err(error) => Err(server_error(error)),
        }
    }

    async fn validation_rules(&self, api_name: &str) -> Result<Option<Vec<Value>>, SyncError> {
        self.check()?;
        match self.server.validation_rules(api_name).await {
            Ok(rules) => Ok(Some(
                rules
                    .iter()
                    .map(|rule| {
                        json!({
                            "name": rule.name,
                            "description": rule.description,
                            "condition": rule.condition,
                            "message": rule.message,
                            "error_field": rule.error_field,
                            "active": rule.active,
                            "version": rule.version,
                        })
                    })
                    .collect(),
            )),
            Err(ObjectError::PermissionDenied | ObjectError::NotFound) => Ok(None),
            Err(error) => Err(server_error(error)),
        }
    }

    async fn composite(
        &self,
        request: &CompositeRequest,
    ) -> Result<Vec<CompositeResult>, SyncError> {
        self.check()?;
        // The request goes over the wire as JSON.
        let request: CompositeRequest =
            serde_json::from_value(serde_json::to_value(request).expect("encode")).expect("decode");
        let objects: BTreeMap<String, String> = request
            .operations
            .iter()
            .map(|operation| {
                (
                    operation.reference().to_owned(),
                    operation.object().to_owned(),
                )
            })
            .collect();
        let outcomes = self
            .server
            .composite(&request, context())
            .await
            .map_err(server_error)?;
        let during = self.define_during_push.lock().expect("hook").take();
        if let Some((admin, id)) = during {
            admin
                .create_field(ACCOUNT, field("late", FieldType::Text), context())
                .await
                .expect("late field");
            let current = self
                .server
                .get_record(ACCOUNT, &id, false)
                .await
                .expect("record");
            self.server
                .update_record(
                    ACCOUNT,
                    &id,
                    current.version,
                    &body(json!({"custom:late": "arrived"})),
                    context(),
                )
                .await
                .expect("late value");
        }
        Ok(outcomes
            .into_iter()
            .map(|(reference, outcome)| {
                let (status, body) = match outcome {
                    CompositeOutcome::Done { record, created } => (
                        if created { 201 } else { 200 },
                        record_json(&objects[&reference], &record),
                    ),
                    CompositeOutcome::Failed(error) => problem(&error),
                    CompositeOutcome::RolledBack => (409, json!({"code": "composite.rolled_back"})),
                    CompositeOutcome::NotRun => (424, json!({"code": "composite.not_run"})),
                };
                CompositeResult {
                    reference,
                    status,
                    body,
                }
            })
            .collect())
    }

    async fn record(&self, api_name: &str, public_id: &str) -> Result<Option<Value>, SyncError> {
        self.check()?;
        match self.server.get_record(api_name, public_id, true).await {
            Ok(record) => Ok(Some(record_json(api_name, &record))),
            Err(ObjectError::NotFound | ObjectError::PermissionDenied) => Ok(None),
            Err(error) => Err(server_error(error)),
        }
    }

    async fn changes(&self, after: Option<i64>, limit: u16) -> Result<ChangesPage, SyncError> {
        self.check()?;
        if self.hang.load(Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
        if after.is_some() && self.expire.swap(false, Ordering::Relaxed) {
            return Err(SyncError::PositionExpired);
        }
        let page = self
            .server
            .list_changes(after, Some(limit), None)
            .await
            .map_err(server_error)?;
        Ok(ChangesPage {
            entries: page
                .changes
                .iter()
                .map(|(object, change)| ChangeEntry {
                    position: change.position,
                    object: object.clone(),
                    record: change.public_id.clone(),
                    kind: change.kind.as_str().to_owned(),
                })
                .collect(),
            next_after: page.next_after,
            head: page.head,
            metadata_version: page.metadata_version,
        })
    }

    /// The server's stream, as `stream_changes` produces it: a `metadata`
    /// event whenever the version moves, then each visible change, polling
    /// until the stream's lifetime ends.
    async fn stream(&self, after: Option<i64>) -> Result<EventStream, SyncError> {
        use futures_util::StreamExt;
        self.check()?;
        if after.is_some() && self.expire.swap(false, Ordering::Relaxed) {
            return Err(SyncError::PositionExpired);
        }
        self.server
            .list_changes(after, Some(1), None)
            .await
            .map_err(server_error)?;
        self.streams.fetch_add(1, Ordering::Relaxed);
        let lifetime = *self.stream_lifetime.lock().expect("lifetime");
        // Like the server, a task of its own produces the events, so a
        // client that stops reading for a while holds no server transaction.
        let (events, received) = tokio::sync::mpsc::channel(64);
        let server = self.server.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + lifetime;
            let mut after = after;
            let mut metadata = None;
            while tokio::time::Instant::now() < deadline {
                let Ok(page) = server.list_changes(after, Some(200), None).await else {
                    return;
                };
                let mut batch = Vec::new();
                if metadata != Some(page.metadata_version) {
                    metadata = Some(page.metadata_version);
                    batch.push(StreamEvent::Metadata(page.metadata_version));
                }
                for (object, change) in &page.changes {
                    batch.push(StreamEvent::Change(ChangeEntry {
                        position: change.position,
                        object: object.clone(),
                        record: change.public_id.clone(),
                        kind: change.kind.as_str().to_owned(),
                    }));
                }
                let moved = Some(page.next_after) != after;
                after = Some(page.next_after);
                for event in batch {
                    if events.send(Ok(event)).await.is_err() {
                        return;
                    }
                }
                if !moved {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
            }
        });
        Ok(
            futures_util::stream::unfold(received, |mut received| async move {
                received.recv().await.map(|event| (event, received))
            })
            .boxed(),
        )
    }

    async fn records_page(
        &self,
        api_name: &str,
        cursor: Option<&str>,
    ) -> Result<RecordsPage, SyncError> {
        self.check()?;
        let page = self
            .server
            .list_records(
                api_name,
                ListRecords {
                    status: RecordStatus::Any,
                    sort: RecordSort::TouchedAt,
                    direction: SortDirection::Asc,
                    limit: Some(2),
                    cursor: cursor.map(str::to_owned),
                    ..ListRecords::default()
                },
            )
            .await
            .map_err(server_error)?;
        Ok(RecordsPage {
            records: page
                .records
                .iter()
                .map(|record| record_json(api_name, record))
                .collect(),
            next_cursor: page.next_cursor,
        })
    }

    async fn directory(&self) -> Result<ReplicaDirectory, SyncError> {
        self.check()?;
        Ok(self.directory.lock().expect("directory").clone())
    }

    async fn revocations(
        &self,
        records: &BTreeMap<String, Vec<String>>,
    ) -> Result<BTreeMap<String, Vec<String>>, SyncError> {
        self.check()?;
        assert!(records.values().map(Vec::len).sum::<usize>() <= 1_000);
        self.server.revocations(records).await.map_err(server_error)
    }
}

const ACCOUNT: &str = "custom:account";
const CONTACT: &str = "custom:contact";

struct World {
    _directory: tempfile::TempDir,
    store: Arc<dyn ObjectStore>,
    tenant: Uuid,
    admin: ObjectService,
    member_id: Uuid,
    role: Uuid,
    /// The member as the server sees them.
    member: ObjectService,
    client: SyncClient<LocalTransport>,
}

impl World {
    async fn new() -> Self {
        Self::build(|client| client).await
    }

    async fn build(
        configure: impl FnOnce(SyncClient<LocalTransport>) -> SyncClient<LocalTransport>,
    ) -> Self {
        let directory = tempfile::tempdir().expect("temp dir");
        let databases = Arc::new(SqliteTenantDatabases::new(
            directory.path(),
            SQLITE_MIGRATIONS,
        ));
        let store: Arc<dyn ObjectStore> = Arc::new(SqliteObjectStore::new(databases));
        let tenant = Uuid::now_v7();
        let admin = ObjectService::new(
            store.clone(),
            caller(
                tenant,
                Uuid::now_v7(),
                &[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION],
                &[],
            ),
        );
        let role = Uuid::now_v7();
        for (name, label) in [("account", "Account"), ("contact", "Contact")] {
            admin
                .create_object(
                    CreateObject {
                        name: name.into(),
                        label: label.into(),
                        plural_label: format!("{label}s"),
                        description: None,
                        name_label: None,
                        sharing_model: Some(SharingModel::Private),
                        grant_hierarchy: None,
                        track_history: None,
                        searchable: None,
                        in_navigation: None,
                    },
                    context(),
                )
                .await
                .expect("object");
            admin
                .set_object_permission(
                    &format!("custom:{name}"),
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
        let mut tier = field("tier", FieldType::Text);
        tier.indexed = true;
        admin
            .create_field(ACCOUNT, tier, context())
            .await
            .expect("tier");
        admin
            .create_field(ACCOUNT, field("notes", FieldType::Text), context())
            .await
            .expect("notes");
        let mut employer = field("employer", FieldType::Lookup);
        employer.reference_object = Some(ACCOUNT.into());
        admin
            .create_field(CONTACT, employer, context())
            .await
            .expect("employer");

        let member_id = Uuid::now_v7();
        let member = ObjectService::new(store.clone(), caller(tenant, member_id, &[], &[role]));
        let device = ObjectService::new(
            store.clone(),
            caller(Uuid::now_v7(), member_id, &[], &[Uuid::now_v7()]),
        )
        .with_replica()
        .with_client_ids();
        let client = configure(SyncClient::new(
            device,
            LocalTransport {
                server: ObjectService::new(store.clone(), caller(tenant, member_id, &[], &[role])),
                expire: AtomicBool::new(false),
                wipe: AtomicBool::new(false),
                define_during_push: std::sync::Mutex::new(None),
                stream_lifetime: std::sync::Mutex::new(std::time::Duration::from_secs(30)),
                streams: std::sync::atomic::AtomicUsize::new(0),
                hang: AtomicBool::new(false),
                down: AtomicBool::new(false),
                directory: std::sync::Mutex::new(ReplicaDirectory {
                    members: BTreeMap::from([(member_id, "Ada Lovelace".to_owned())]),
                    roles: BTreeMap::from([(role, "Sales".to_owned())]),
                }),
            },
        ));
        Self {
            _directory: directory,
            store,
            tenant,
            admin,
            member_id,
            role,
            member,
            client,
        }
    }

    fn device(&self) -> &ObjectService {
        self.client.engine()
    }

    async fn sync(&self) -> SyncReport {
        self.client.sync_once(context()).await.expect("sync")
    }

    async fn on_server(&self, object: &str, id: &str) -> Record {
        self.admin
            .get_record(object, id, true)
            .await
            .expect("server record")
    }

    async fn on_device(&self, object: &str, id: &str) -> Option<Record> {
        match self.device().get_record(object, id, true).await {
            Ok(record) => Some(record),
            Err(ObjectError::NotFound) => None,
            Err(error) => panic!("device read: {error:?}"),
        }
    }

    async fn edit(&self, engine: &ObjectService, object: &str, id: &str, values: Value) -> Record {
        let current = engine.get_record(object, id, true).await.expect("record");
        engine
            .update_record(object, id, current.version, &body(values), context())
            .await
            .expect("edit")
    }

    async fn create(&self, engine: &ObjectService, object: &str, values: Value) -> Record {
        engine
            .create_record(object, &body(values), context())
            .await
            .expect("create")
    }
}

fn field(name: &str, kind: FieldType) -> CreateField {
    CreateField {
        name: name.into(),
        label: name.into(),
        field_type: kind,
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
    }
}

fn tier(record: &Record) -> Option<&str> {
    record.values.get("custom:tier").and_then(Value::as_str)
}

#[tokio::test]
async fn the_first_sync_takes_definitions_then_every_visible_record() {
    let world = World::new().await;
    let mut mine = Vec::new();
    for name in ["One", "Two", "Three"] {
        mine.push(
            world
                .create(
                    &world.member,
                    ACCOUNT,
                    json!({"name": name, "custom:tier": "gold"}),
                )
                .await,
        );
    }
    let hidden = world
        .create(&world.admin, ACCOUNT, json!({"name": "Hidden"}))
        .await;

    let report = world.sync().await;
    let definitions = report
        .definitions
        .clone()
        .expect("definitions on the first sync");
    assert_eq!(definitions.objects, vec![ACCOUNT, CONTACT]);
    assert!(report.resynced);
    assert_eq!(report.pulled, 3, "{report:?}");
    for record in &mine {
        let local = world
            .on_device(ACCOUNT, &record.public_id)
            .await
            .expect("held");
        assert_eq!(local.version, record.version);
        assert_eq!(tier(&local), Some("gold"));
    }
    assert!(world.on_device(ACCOUNT, &hidden.public_id).await.is_none());
    let positions = world.device().sync_positions().await.expect("positions");
    let head = world
        .member
        .list_changes(None, Some(1), None)
        .await
        .expect("head")
        .head;
    assert_eq!(positions.pulled, Some(head));

    // Nothing moved: the next sync stores nothing and takes no definitions.
    let again = world.sync().await;
    assert!(again.definitions.is_none() && !again.resynced);
    assert_eq!(again.pulled, 0);
    assert!(again.pushed.is_empty());
}

#[tokio::test]
async fn offline_records_push_with_their_ids_and_server_edits_pull() {
    let world = World::new().await;
    world.sync().await;

    let acme = world
        .create(
            world.device(),
            ACCOUNT,
            json!({"name": "Acme", "custom:tier": "gold"}),
        )
        .await;
    let ada = world
        .create(
            world.device(),
            CONTACT,
            json!({"name": "Ada", "custom:employer": acme.public_id}),
        )
        .await;
    let report = world.sync().await;
    assert!(
        report.pushed.iter().all(|pushed| pushed.failed.is_empty()),
        "{report:?}"
    );
    assert!(world.device().push_queue().await.expect("queue").is_empty());
    let server_ada = world.on_server(CONTACT, &ada.public_id).await;
    assert_eq!(
        server_ada.values.get("custom:employer"),
        Some(&json!(acme.public_id))
    );
    assert_eq!(
        world
            .device()
            .remote_version(CONTACT, &ada.public_id)
            .await
            .expect("version"),
        Some(server_ada.version)
    );

    // The member edits on the web; the device pulls it.
    world
        .edit(
            &world.member,
            ACCOUNT,
            &acme.public_id,
            json!({"custom:tier": "platinum"}),
        )
        .await;
    let report = world.sync().await;
    assert_eq!(report.pulled, 1, "{report:?}");
    let local = world
        .on_device(ACCOUNT, &acme.public_id)
        .await
        .expect("held");
    assert_eq!(tier(&local), Some("platinum"));
    assert!(world.device().push_queue().await.expect("queue").is_empty());
}

#[tokio::test]
async fn conflicts_merge_on_their_own_or_wait_for_the_member() {
    let world = World::new().await;
    let acme = world
        .create(
            &world.member,
            ACCOUNT,
            json!({"name": "Acme", "custom:tier": "gold"}),
        )
        .await;
    world.sync().await;

    // Different fields: the push meets a version conflict, merges and goes
    // again in the same sync.
    world
        .edit(
            world.device(),
            ACCOUNT,
            &acme.public_id,
            json!({"custom:tier": "silver"}),
        )
        .await;
    world
        .edit(
            &world.member,
            ACCOUNT,
            &acme.public_id,
            json!({"custom:notes": "call back"}),
        )
        .await;
    let report = world.sync().await;
    assert_eq!(
        report.merged,
        vec![(ACCOUNT.to_owned(), acme.public_id.clone())]
    );
    assert!(report.conflicts.is_empty());
    let server = world.on_server(ACCOUNT, &acme.public_id).await;
    assert_eq!(tier(&server), Some("silver"));
    assert_eq!(server.values.get("custom:notes"), Some(&json!("call back")));
    assert!(world.device().push_queue().await.expect("queue").is_empty());

    // The same field: the member chooses.
    world
        .edit(
            world.device(),
            ACCOUNT,
            &acme.public_id,
            json!({"custom:tier": "bronze"}),
        )
        .await;
    world
        .edit(
            &world.member,
            ACCOUNT,
            &acme.public_id,
            json!({"custom:tier": "diamond"}),
        )
        .await;
    let report = world.sync().await;
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    let pending = &report.conflicts[0];
    assert_eq!(pending.public_id, acme.public_id);
    assert_eq!(pending.merge.conflicts[0].field, "custom:tier");
    assert_eq!(pending.merge.conflicts[0].local, json!("bronze"));
    assert_eq!(pending.merge.conflicts[0].server, json!("diamond"));
    // Still waiting: the next sync neither loops nor loses the edit.
    let report = world.sync().await;
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(
        tier(
            &world
                .on_device(ACCOUNT, &acme.public_id)
                .await
                .expect("held")
        ),
        Some("bronze")
    );

    world
        .client
        .settle(
            ACCOUNT,
            &acme.public_id,
            &Choices {
                fields: BTreeMap::from([("custom:tier".to_owned(), Side::Local)]),
                keep_deletion: None,
            },
            context(),
        )
        .await
        .expect("settle");
    let report = world.sync().await;
    assert!(report.conflicts.is_empty(), "{report:?}");
    assert_eq!(
        tier(&world.on_server(ACCOUNT, &acme.public_id).await),
        Some("bronze")
    );
}

#[tokio::test]
async fn new_definitions_arrive_before_the_records_that_use_them() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    world.sync().await;

    world
        .admin
        .create_field(ACCOUNT, field("region", FieldType::Text), context())
        .await
        .expect("region");
    world
        .edit(
            &world.member,
            ACCOUNT,
            &acme.public_id,
            json!({"custom:region": "north"}),
        )
        .await;
    let report = world.sync().await;
    let definitions = report.definitions.clone().expect("definitions moved");
    assert_eq!(definitions.fields, vec!["custom:account.custom:region"]);
    let local = world
        .on_device(ACCOUNT, &acme.public_id)
        .await
        .expect("held");
    assert_eq!(local.values.get("custom:region"), Some(&json!("north")));
    let positions = world.device().sync_positions().await.expect("positions");
    let page = world
        .member
        .list_changes(None, Some(1), None)
        .await
        .expect("page");
    assert_eq!(positions.metadata_version, Some(page.metadata_version));
}

#[tokio::test]
async fn definitions_that_move_during_a_sync_are_taken_before_the_pull() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    let other = world
        .create(&world.member, ACCOUNT, json!({"name": "Other"}))
        .await;
    world.sync().await;
    world
        .edit(
            world.device(),
            ACCOUNT,
            &other.public_id,
            json!({"custom:tier": "gold"}),
        )
        .await;
    *world
        .client
        .transport()
        .define_during_push
        .lock()
        .expect("hook") = Some((
        ObjectService::new(world.store.clone(), world.admin.caller().clone()),
        acme.public_id.clone(),
    ));
    let report = world.sync().await;
    assert_eq!(
        report.definitions.expect("taken mid-sync").fields,
        vec!["custom:account.custom:late"]
    );
    let local = world
        .on_device(ACCOUNT, &acme.public_id)
        .await
        .expect("held");
    assert_eq!(local.values.get("custom:late"), Some(&json!("arrived")));
}

#[tokio::test]
async fn an_expired_position_takes_new_definitions_before_reading_again() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    let other = world
        .create(&world.member, ACCOUNT, json!({"name": "Other"}))
        .await;
    world.sync().await;
    world
        .edit(
            world.device(),
            ACCOUNT,
            &other.public_id,
            json!({"custom:tier": "gold"}),
        )
        .await;
    // Definitions move after the sync's first look, then the pull finds its
    // position expired.
    *world
        .client
        .transport()
        .define_during_push
        .lock()
        .expect("hook") = Some((
        ObjectService::new(world.store.clone(), world.admin.caller().clone()),
        acme.public_id.clone(),
    ));
    world
        .client
        .transport()
        .expire
        .store(true, Ordering::Relaxed);
    let report = world.sync().await;
    assert!(report.resynced, "{report:?}");
    let local = world
        .on_device(ACCOUNT, &acme.public_id)
        .await
        .expect("held");
    assert_eq!(local.values.get("custom:late"), Some(&json!("arrived")));
}

#[tokio::test]
async fn objects_the_member_may_read_later_bring_their_existing_records() {
    let world = World::new().await;
    world
        .admin
        .create_object(
            CreateObject {
                name: "region".into(),
                label: "Region".into(),
                plural_label: "Regions".into(),
                description: None,
                name_label: None,
                sharing_model: Some(SharingModel::ReadWrite),
                grant_hierarchy: None,
                track_history: None,
                searchable: None,
                in_navigation: None,
            },
            context(),
        )
        .await
        .expect("region");
    let north = world
        .create(&world.admin, "custom:region", json!({"name": "North"}))
        .await;
    let south = world
        .create(&world.admin, "custom:region", json!({"name": "South"}))
        .await;
    world.sync().await;
    assert!(world
        .on_device("custom:region", &north.public_id)
        .await
        .is_none());

    // A grant moves the definitions but leaves nothing in the change log.
    world
        .admin
        .set_object_permission(
            "custom:region",
            world.role,
            AccessFlags {
                read: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("grant");
    let report = world.sync().await;
    assert!(report.definitions.is_some(), "{report:?}");
    for record in [&north, &south] {
        assert!(
            world
                .on_device("custom:region", &record.public_id)
                .await
                .is_some(),
            "{report:?}"
        );
    }
}

#[tokio::test]
async fn records_the_member_loses_are_forgotten_after_a_reconnect() {
    let world = World::new().await;
    let kept = world
        .create(&world.member, ACCOUNT, json!({"name": "Kept"}))
        .await;
    let lost = world
        .create(&world.member, ACCOUNT, json!({"name": "Lost"}))
        .await;
    world.sync().await;
    assert!(world.on_device(ACCOUNT, &lost.public_id).await.is_some());

    // Ownership moves away; under a private sharing model the member can no
    // longer read it, and the change log stops naming it.
    let admin_id = world.admin.caller().user_id();
    world
        .admin
        .change_owner(ACCOUNT, &lost.public_id, admin_id, lost.version, context())
        .await
        .expect("transfer");
    let report = world.sync().await;
    assert_eq!(
        report.revoked, 0,
        "the revocation set waits for a reconnect"
    );
    world.client.reconnected();
    let report = world.sync().await;
    assert_eq!(report.revoked, 1, "{report:?}");
    assert!(world.on_device(ACCOUNT, &lost.public_id).await.is_none());
    assert!(world.on_device(ACCOUNT, &kept.public_id).await.is_some());
}

#[tokio::test]
async fn an_expired_position_reads_every_record_again() {
    let world = World::new().await;
    world.sync().await;
    let late = world
        .create(
            &world.member,
            ACCOUNT,
            json!({"name": "Late", "custom:tier": "gold"}),
        )
        .await;
    world
        .client
        .transport()
        .expire
        .store(true, Ordering::Relaxed);
    let report = world.sync().await;
    assert!(report.resynced);
    assert!(world.on_device(ACCOUNT, &late.public_id).await.is_some());
}

#[tokio::test]
async fn a_change_the_server_refuses_waits_for_the_member() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    world.sync().await;
    // The rule arrives after the edit was made offline, so the device could
    // not check it and the push is refused.
    world
        .admin
        .create_validation_rule(
            ACCOUNT,
            CreateValidationRule {
                name: "tier_known".into(),
                description: None,
                condition: "custom:tier = \"unknown\"".into(),
                message: "Choose a tier".into(),
                error_field: None,
                active: Some(true),
            },
            context(),
        )
        .await
        .expect("rule");
    world
        .edit(
            world.device(),
            ACCOUNT,
            &acme.public_id,
            json!({"custom:tier": "unknown"}),
        )
        .await;
    let report = world.sync().await;
    assert_eq!(report.pushed.len(), 1, "one request, no loop: {report:?}");
    assert_eq!(
        report.pushed[0].failed,
        vec![(
            ACCOUNT.to_owned(),
            acme.public_id.clone(),
            "validation.failed".to_owned()
        )]
    );
    // It stays queued for the member to change.
    assert_eq!(
        world
            .device()
            .push_queue()
            .await
            .expect("queue")
            .operations
            .len(),
        1
    );
    let report = world.sync().await;
    assert_eq!(report.pushed.len(), 1);
}

#[tokio::test]
async fn a_wipe_answer_stops_the_sync() {
    let world = World::new().await;
    world.sync().await;
    world.client.transport().wipe.store(true, Ordering::Relaxed);
    assert!(matches!(
        world.client.sync_once(context()).await,
        Err(SyncError::WipeRequired)
    ));
    // Unused fields of the world are part of its setup.
    let _ = (&world.store, world.tenant, world.member_id);
}

/// Waits up to five seconds for `check` to hold, giving the foreground loop
/// time to run.
async fn eventually<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// Runs the foreground loop alongside `drive`, stopping it once `drive`
/// returns, and gives back everything the loop observed.
async fn following<F: std::future::Future<Output = ()>>(
    world: &World,
    drive: F,
) -> Vec<SyncReport> {
    let stop = tokio::sync::Notify::new();
    let mut seen = Vec::new();
    let (result, ()) = tokio::join!(
        world
            .client
            .follow(context(), stop.notified(), |report| seen
                .push(report.clone())),
        async {
            drive.await;
            stop.notify_one();
        }
    );
    result.expect("follow");
    seen
}

#[tokio::test]
async fn the_foreground_loop_stores_server_changes_as_they_happen() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    let seen = following(&world, async {
        eventually("the stream opens", || async {
            world.client.transport().streams.load(Ordering::Relaxed) == 1
        })
        .await;
        assert!(world.on_device(ACCOUNT, &acme.public_id).await.is_some());
        world
            .edit(
                &world.member,
                ACCOUNT,
                &acme.public_id,
                json!({"custom:tier": "gold"}),
            )
            .await;
        let late = world
            .create(&world.member, ACCOUNT, json!({"name": "Late"}))
            .await;
        eventually("the edit streams in", || async {
            world
                .on_device(ACCOUNT, &acme.public_id)
                .await
                .is_some_and(|record| tier(&record) == Some("gold"))
        })
        .await;
        eventually("the new record streams in", || async {
            world.on_device(ACCOUNT, &late.public_id).await.is_some()
        })
        .await;
    })
    .await;
    assert!(seen[0].resynced, "a sync comes first: {:?}", seen[0]);
    assert!(seen.iter().skip(1).any(|report| report.pulled == 1));
    // The stored position follows the stream.
    let head = world
        .member
        .list_changes(None, Some(1), None)
        .await
        .expect("head")
        .head;
    assert_eq!(
        world
            .device()
            .sync_positions()
            .await
            .expect("positions")
            .pulled,
        Some(head)
    );
}

#[tokio::test]
async fn local_changes_push_after_a_short_pause() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    world.sync().await;
    let seen = following(&world, async {
        eventually("the stream opens", || async {
            world.client.transport().streams.load(Ordering::Relaxed) == 1
        })
        .await;
        // A burst of edits goes as one request.
        for value in ["silver", "gold", "platinum"] {
            world
                .edit(
                    world.device(),
                    ACCOUNT,
                    &acme.public_id,
                    json!({"custom:tier": value}),
                )
                .await;
            world.client.local_change();
        }
        eventually("the burst reaches the server", || async {
            tier(&world.on_server(ACCOUNT, &acme.public_id).await) == Some("platinum")
        })
        .await;
    })
    .await;
    let requests: usize = seen.iter().map(|report| report.pushed.len()).sum();
    assert_eq!(requests, 1, "{seen:?}");
    assert!(world.device().push_queue().await.expect("queue").is_empty());
}

#[tokio::test]
async fn definitions_announced_on_the_stream_arrive_before_their_records() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    world.sync().await;
    following(&world, async {
        // Let the stream open before definitions move.
        eventually("the stream opens", || async {
            world.client.transport().streams.load(Ordering::Relaxed) == 1
        })
        .await;
        world
            .admin
            .create_field(ACCOUNT, field("region", FieldType::Text), context())
            .await
            .expect("region");
        world
            .edit(
                &world.member,
                ACCOUNT,
                &acme.public_id,
                json!({"custom:region": "north"}),
            )
            .await;
        eventually("the new field's value is stored", || async {
            world
                .on_device(ACCOUNT, &acme.public_id)
                .await
                .is_some_and(|record| record.values.get("custom:region") == Some(&json!("north")))
        })
        .await;
    })
    .await;
    assert_eq!(world.client.transport().streams.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn a_stream_the_server_ends_resumes_after_a_sync_that_checks_access() {
    let world = World::new().await;
    let lost = world
        .create(&world.member, ACCOUNT, json!({"name": "Lost"}))
        .await;
    world.sync().await;
    *world
        .client
        .transport()
        .stream_lifetime
        .lock()
        .expect("lifetime") = std::time::Duration::from_millis(200);
    let admin_id = world.admin.caller().user_id();
    let seen = following(&world, async {
        world
            .admin
            .change_owner(ACCOUNT, &lost.public_id, admin_id, lost.version, context())
            .await
            .expect("transfer");
        eventually("the reconnect forgets the record", || async {
            world.on_device(ACCOUNT, &lost.public_id).await.is_none()
        })
        .await;
    })
    .await;
    assert!(world.client.transport().streams.load(Ordering::Relaxed) >= 2);
    assert!(seen.iter().any(|report| report.revoked == 1), "{seen:?}");
}

#[tokio::test]
async fn an_expired_stream_position_reads_every_record_again() {
    let world = World::new().await;
    world.sync().await;
    let late = world
        .create(&world.member, ACCOUNT, json!({"name": "Late"}))
        .await;
    world
        .client
        .transport()
        .expire
        .store(true, Ordering::Relaxed);
    let seen = following(&world, async {
        eventually("the record arrives", || async {
            world.on_device(ACCOUNT, &late.public_id).await.is_some()
        })
        .await;
    })
    .await;
    assert!(seen.iter().any(|report| report.resynced), "{seen:?}");
}

#[tokio::test]
async fn a_background_wake_syncs_within_its_budget() {
    let world = World::new().await;
    world.sync().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    assert!(matches!(
        world
            .client
            .wake(context(), std::time::Duration::ZERO)
            .await,
        Err(SyncError::OutOfTime)
    ));
    let report = world
        .client
        .wake(context(), std::time::Duration::from_secs(10))
        .await
        .expect("wake");
    assert_eq!(report.pulled, 1);
    assert!(world.on_device(ACCOUNT, &acme.public_id).await.is_some());
}

#[tokio::test]
async fn a_background_sync_the_app_drops_leaves_no_budget_behind() {
    let world = World::new().await;
    world.sync().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    // The server stops answering; the app gives up on the wake before its
    // own budget runs out, dropping it mid-request.
    world.client.transport().hang.store(true, Ordering::Relaxed);
    let dropped = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        world
            .client
            .wake(context(), std::time::Duration::from_millis(200)),
    )
    .await;
    assert!(dropped.is_err(), "the wake was still waiting");
    world
        .client
        .transport()
        .hang
        .store(false, Ordering::Relaxed);
    // Past the dropped wake's deadline, an ordinary sync has no budget.
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let report = world.sync().await;
    assert_eq!(report.pulled, 1);
    assert!(world.on_device(ACCOUNT, &acme.public_id).await.is_some());
}

#[tokio::test]
async fn the_server_rules_reach_the_device_and_apply_offline() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    world.sync().await;
    world
        .admin
        .create_validation_rule(
            ACCOUNT,
            CreateValidationRule {
                name: "tier_known".into(),
                description: None,
                condition: "custom:tier = \"unknown\"".into(),
                message: "Choose a tier".into(),
                error_field: Some("custom:tier".into()),
                active: Some(true),
            },
            context(),
        )
        .await
        .expect("rule");
    let report = world.sync().await;
    assert_eq!(
        report.definitions.expect("definitions moved").rules,
        vec!["custom:account.tier_known"]
    );
    // The member's offline edit is refused on the device, before any push.
    let current = world
        .on_device(ACCOUNT, &acme.public_id)
        .await
        .expect("held");
    let refused = world
        .device()
        .update_record(
            ACCOUNT,
            &acme.public_id,
            current.version,
            &body(json!({"custom:tier": "unknown"})),
            context(),
        )
        .await
        .expect_err("the rule applies offline");
    let ObjectError::Validation(issues) = refused else {
        panic!("expected the rule's issue: {refused:?}");
    };
    assert_eq!(issues[0].code, "records.validation_rule");
    assert_eq!(issues[0].detail, "Choose a tier");
    assert!(world.device().push_queue().await.expect("queue").is_empty());
}

#[tokio::test]
async fn a_device_offline_past_its_limit_locks_until_it_reaches_the_server() {
    let world = World::new().await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    world
        .device()
        .set_offline_limit(3_600)
        .await
        .expect("limit");
    world.sync().await;
    let lease = world.device().lease().await.expect("lease");
    assert!(
        lease.last_contact_at.is_some() && !lease.locked,
        "{lease:?}"
    );

    // Offline past the limit: the member can no longer read the replica.
    // The last contact is set two hours back rather than waited out, so the
    // syncs below have the whole hour to finish however busy the machine.
    world.client.transport().down.store(true, Ordering::Relaxed);
    world
        .device()
        .record_contact_at(chrono::Utc::now() - chrono::Duration::hours(2))
        .await
        .expect("contact two hours ago");
    assert!(matches!(
        world
            .device()
            .get_record(ACCOUNT, &acme.public_id, false)
            .await,
        Err(ObjectError::Conflict("sync.lease_expired"))
    ));
    assert!(matches!(
        world.client.sync_once(context()).await,
        Err(SyncError::Transport(_))
    ));
    assert!(world.device().lease().await.expect("lease").locked);

    // The next sync that reaches the server unlocks it, even in the
    // background.
    world
        .client
        .transport()
        .down
        .store(false, Ordering::Relaxed);
    world
        .client
        .wake(context(), std::time::Duration::from_secs(10))
        .await
        .expect("wake");
    assert!(!world.device().lease().await.expect("lease").locked);
    assert!(world.on_device(ACCOUNT, &acme.public_id).await.is_some());
}

#[tokio::test]
async fn a_connected_device_renews_its_lease_while_the_stream_stays_open() {
    let world =
        World::build(|client| client.with_renew_every(std::time::Duration::from_millis(200))).await;
    let acme = world
        .create(&world.member, ACCOUNT, json!({"name": "Acme"}))
        .await;
    // A one-second limit, shorter than the stream stays open: renewing every
    // 200 ms keeps the replica readable throughout.
    world.device().set_offline_limit(1).await.expect("limit");
    following(&world, async {
        eventually("the first sync", || async {
            world.on_device(ACCOUNT, &acme.public_id).await.is_some()
        })
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(1_600)).await;
        assert!(
            world.on_device(ACCOUNT, &acme.public_id).await.is_some(),
            "the lease was renewed while connected"
        );
    })
    .await;
    assert!(!world.device().lease().await.expect("lease").locked);
}

#[tokio::test]
async fn every_sync_takes_the_member_directory() {
    let world = World::new().await;
    let report = world.sync().await;
    assert_eq!(report.directory, Some((1, 1)), "{report:?}");
    let directory = world.device().directory().await.expect("directory");
    assert_eq!(directory.members[&world.member_id], "Ada Lovelace");
    assert_eq!(directory.roles[&world.role], "Sales");

    // Members and roles change without the metadata version moving; the
    // next sync replaces the directory whole.
    let colleague = Uuid::now_v7();
    {
        let transport = world.client.transport();
        let mut held = transport.directory.lock().expect("directory");
        held.members.insert(colleague, "Charles Babbage".to_owned());
        held.roles.insert(world.role, "Field sales".to_owned());
        held.members.remove(&world.member_id);
    }
    let again = world.sync().await;
    assert!(again.definitions.is_none(), "{again:?}");
    let directory = world.device().directory().await.expect("directory");
    assert_eq!(
        directory.members,
        BTreeMap::from([(colleague, "Charles Babbage".to_owned())])
    );
    assert_eq!(directory.roles[&world.role], "Field sales");

    // Only a replica keeps a directory.
    assert!(matches!(
        world.member.directory().await,
        Err(ObjectError::PermissionDenied)
    ));
}
