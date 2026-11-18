//! The assistant bridge (ADR 0034): one typed, versioned interface
//! (`assistant.v1`, [`contract`]) that native assistant plugins call, built
//! from the same [`DeviceHost`] as the local command bridge.
//!
//! - **Only through a capability.** A native plugin reaches it only with
//!   the `device.assistant` local capability, through the shell's grant
//!   ([`crate::capabilities::PluginGrant::assistant`]). No route of the
//!   command bridge and no Tauri command serves it, so the web view cannot
//!   reach it, and no script in the web origin can make a change that
//!   records the `assistant` source.
//! - **Through the command bridge, as the member.** Every operation becomes
//!   requests to [`Bridge::handle`], as the signed-in member, so every check
//!   the engine makes for the web shell applies: permissions, sharing,
//!   field-level security, validation rules, lookup filters, uniqueness,
//!   before-save automations, history and audit. The bridge adds checks and
//!   never skips one.
//! - **The policy only narrows** (§4). Before every operation the bridge
//!   reads the assistant policy in the replica's definitions: the workspace
//!   switch, and per object exposed, writable and the offered quick
//!   actions. It is applied after the member's own access.
//! - **Two-step writes** (§5). `create`, `update` and `run_action` run the
//!   engine's checks as a dry run and answer with a [`Confirmation`] and a
//!   single-use token ([`confirm`]); only `commit` with that token writes,
//!   and nothing writes without one.
//! - **Provenance** (§6). Writes record `MutationSource::assistant` with
//!   the assistant's name, and are marked for the device's own push, so
//!   the server re-checks them against its current policy.
//! - **Unavailable states and errors** (§8, §10): stable `assistant.*`
//!   codes with fixed sentences ([`errors`]).
//! - **The index stream** (§7, [`index`]): the replica change feed turned
//!   into what an OS index may hold, already filtered by the policy and the
//!   member's access.
//!
//! The member's own per-device setting ([`DeviceAssistantSettings`]) can
//! turn assistant access off on the device, and keeps OS indexing off
//! until the member turns it on.

pub mod confirm;
pub mod contract;
mod display;
pub mod errors;
pub mod index;
mod model;

use crate::feed::ReplicaFeed;
use crate::route::RouteContext;
use crate::Bridge;
use chrono::Utc;
use confirm::{AssistantClock, Confirmations, Prepared, PreparedWrite, TokenRefused};
pub use contract::*;
use errors::{from_problem, AssistantCode, AssistantError, FieldProblem};
use model::{ActionMeta, FieldMeta, ObjectMeta, PolicyMeta, Reader};
use platform_objects::{Assistant, AssistantMarker, FieldType, MarkedQuickAction, QuickActionKind};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::{Arc, RwLock};
use tokio::sync::watch;
use uuid::Uuid;

/// The member's own choice on one device (ADR 0034 §4 and §7).
///
/// - `assistant_access` turns every assistant operation off on this device
///   (`assistant.disabled`). It is on by default, so assistants work once
///   the workspace's administrator allows them; this default is a decision
///   recorded for the owner to confirm (ADR 0034, open question 2).
/// - `indexing` lets the device put records into the OS's index. It is off
///   until the member turns it on, as §7 requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeviceAssistantSettings {
    pub assistant_access: bool,
    pub indexing: bool,
}

impl Default for DeviceAssistantSettings {
    fn default() -> Self {
        Self {
            assistant_access: true,
            indexing: false,
        }
    }
}

/// Asks the sync loop to push, after a write.
pub type PushNudge = Arc<dyn Fn() + Send + Sync>;

/// Whether the bridge can answer from a replica.
#[derive(Clone)]
enum Availability {
    SignedOut,
    /// Sign-out or a wipe is deleting the replica.
    Closing,
    Open {
        bridge: Bridge,
        /// Asks the sync loop to push, after a write.
        pushed: Option<PushNudge>,
    },
}

struct Shared {
    state: RwLock<Availability>,
    /// Held for reading by every operation; sign-out and a wipe take it
    /// for writing before the replica's files go, so no operation sees a
    /// half-deleted replica.
    gate: tokio::sync::RwLock<()>,
    settings: watch::Sender<DeviceAssistantSettings>,
    confirmations: Confirmations,
    /// The assistant the shell's platform has: Siri on iOS, Gemini on
    /// Android, none on desktop.
    platform: Option<Assistant>,
    feed: ReplicaFeed,
    routes: RouteContext,
}

/// The shell's side of the assistant bridge: the controller opens and
/// closes it with the workspace and keeps the member's device setting.
/// Native plugins never get this; they get an [`AssistantBridge`] through
/// their grant.
#[derive(Clone)]
pub struct AssistantService(Arc<Shared>);

impl std::fmt::Debug for AssistantService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssistantService")
            .field("platform", &self.0.platform)
            .finish_non_exhaustive()
    }
}

impl AssistantService {
    pub(crate) fn new(
        feed: ReplicaFeed,
        routes: RouteContext,
        platform: Option<Assistant>,
        clock: Arc<dyn AssistantClock>,
    ) -> Self {
        Self(Arc::new(Shared {
            state: RwLock::new(Availability::SignedOut),
            gate: tokio::sync::RwLock::new(()),
            settings: watch::Sender::new(DeviceAssistantSettings::default()),
            confirmations: Confirmations::new(clock),
            platform,
            feed,
            routes,
        }))
    }

    /// The assistant this platform has, if any.
    pub fn platform(&self) -> Option<Assistant> {
        self.0.platform
    }

    fn set(&self, availability: Availability) {
        if let Ok(mut state) = self.0.state.write() {
            *state = availability;
        }
    }

    /// Answers from the workspace `bridge` serves, as its member. `pushed`
    /// asks the sync loop to push after a write.
    pub fn open(&self, bridge: &Bridge, pushed: Option<PushNudge>) {
        self.0.confirmations.clear();
        self.set(Availability::Open {
            bridge: bridge.clone(),
            pushed,
        });
    }

    /// Sign-out or a wipe started: every call answers `assistant.signed_out`
    /// from now on, and prepared changes are forgotten.
    pub fn closing(&self) {
        self.set(Availability::Closing);
        self.0.confirmations.clear();
    }

    /// Waits for the operations that started before [`Self::closing`].
    /// The controller calls it before the replica's files are deleted.
    pub async fn drained(&self) {
        drop(self.0.gate.write().await);
    }

    /// No workspace is open.
    pub fn signed_out(&self) {
        self.set(Availability::SignedOut);
        self.0.confirmations.clear();
    }

    pub fn device_settings(&self) -> DeviceAssistantSettings {
        *self.0.settings.borrow()
    }

    /// The member's choice on this device. Turning indexing off makes the
    /// index stream remove the workspace's entries; turning it on rebuilds.
    pub fn set_device_settings(&self, settings: DeviceAssistantSettings) {
        self.0.settings.send_if_modified(|current| {
            let changed = *current != settings;
            *current = settings;
            changed
        });
    }

    /// The workspace open now, if any.
    pub fn workspace(&self) -> Option<Uuid> {
        match self.0.state.read().ok()?.clone() {
            Availability::Open { bridge, .. } => Some(bridge.host().caller().tenant_id),
            _ => None,
        }
    }

    /// The handle a plugin holding `device.assistant` gets.
    pub(crate) fn bridge_for(&self, plugin: &str) -> AssistantBridge {
        AssistantBridge {
            service: self.clone(),
            plugin: plugin.to_owned(),
        }
    }

    fn open_bridge(&self) -> Option<(Bridge, Option<PushNudge>)> {
        match self.0.state.read().ok()?.clone() {
            Availability::Open { bridge, pushed } => Some((bridge, pushed)),
            _ => None,
        }
    }
}

/// A native assistant plugin's handle on the bridge. Only the shell's
/// grant of `device.assistant` hands one out.
#[derive(Clone)]
pub struct AssistantBridge {
    service: AssistantService,
    plugin: String,
}

impl std::fmt::Debug for AssistantBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AssistantBridge")
            .field("plugin", &self.plugin)
            .finish_non_exhaustive()
    }
}

impl AssistantBridge {
    /// The plugin the handle was granted to.
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// Answers one request.
    pub async fn call(&self, request: AssistantRequest) -> AssistantAnswer {
        match self.run(request).await {
            Ok(result) => AssistantAnswer::ok(result),
            Err(error) => AssistantAnswer::error(error),
        }
    }

    /// [`Self::call`] over JSON, for the Swift and Kotlin halves. A request
    /// that is not `assistant.v1` JSON answers `assistant.invalid`.
    pub async fn call_json(&self, request: &[u8]) -> Vec<u8> {
        let answer = match serde_json::from_slice::<AssistantRequest>(request) {
            Ok(request) => self.call(request).await,
            Err(_) => AssistantAnswer::error(AssistantError::new(AssistantCode::Invalid)),
        };
        serde_json::to_vec(&answer).unwrap_or_default()
    }

    /// The index stream (§7) for this plugin.
    pub fn index_stream(&self) -> index::IndexStream {
        index::IndexStream::new(self.service.clone())
    }

    /// The launch-time sweep (§7): of the workspaces the OS index holds
    /// entries for, those the shell does not hold, whose entries the
    /// plugin deletes, with every entry past its expiry.
    pub fn sweep(&self, indexed: &[Uuid]) -> index::IndexSweep {
        index::sweep(&self.service, indexed)
    }

    async fn run(&self, request: AssistantRequest) -> Result<AssistantResult, AssistantError> {
        let shared = &self.service.0;
        if request.contract != CONTRACT {
            return Err(AssistantError::new(AssistantCode::Invalid));
        }
        // Only the assistant of the platform the shell built the plugin for.
        if shared.platform != Some(request.assistant) {
            return Err(AssistantError::new(AssistantCode::Invalid));
        }
        if !request.device_unlocked {
            return Err(AssistantError::new(AssistantCode::DeviceLocked));
        }
        let _gate = shared.gate.read().await;
        let Some((bridge, pushed)) = self.service.open_bridge() else {
            return Err(AssistantError::new(AssistantCode::SignedOut));
        };
        if !self.service.device_settings().assistant_access {
            return Err(AssistantError::new(AssistantCode::Disabled));
        }
        let workspace = bridge.host().caller().tenant_id;
        let reader = Reader {
            bridge: &bridge,
            workspace,
        };
        let policy = reader.policy().await?;
        if !policy.enabled {
            return Err(AssistantError::new(AssistantCode::Disabled));
        }
        let session = Session {
            service: &self.service,
            bridge: &bridge,
            reader,
            workspace,
            member: bridge.host().caller().user_id,
            assistant: request.assistant,
            policy,
            pushed,
        };
        session.dispatch(request.operation).await
    }
}

/// One call, against the replica open when it started.
struct Session<'a> {
    service: &'a AssistantService,
    bridge: &'a Bridge,
    reader: Reader<'a>,
    workspace: Uuid,
    member: Uuid,
    assistant: Assistant,
    policy: PolicyMeta,
    pushed: Option<PushNudge>,
}

fn failed() -> AssistantError {
    AssistantError::new(AssistantCode::Failed)
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// The name field, as a field.
fn name_field(object: &ObjectMeta) -> FieldMeta {
    FieldMeta {
        api_name: "name".into(),
        label: object.name_label.clone(),
        field_type: FieldType::Text,
        required: true,
        indexed: true,
        read: true,
        edit: true,
        options: Vec::new(),
        reference_object: None,
        scale: None,
        formula_type: None,
    }
}

/// Escapes text for a query-language string literal (ADR 0019 §1); with
/// `like`, also its wildcards. Control characters other than newline and
/// tab are refused.
fn quoted(value: &str, like: bool) -> Option<String> {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for character in value.chars() {
        match character {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '%' if like => out.push_str("\\%"),
            '_' if like => out.push_str("\\_"),
            other if other.is_control() => return None,
            other => out.push(other),
        }
    }
    out.push('\'');
    Some(out)
}

impl Session<'_> {
    async fn dispatch(&self, operation: Operation) -> Result<AssistantResult, AssistantError> {
        match operation {
            Operation::Describe => self.describe().await.map(AssistantResult::Description),
            Operation::Search { text, object } => self
                .search(&text, object.as_deref())
                .await
                .map(AssistantResult::SearchResults),
            Operation::Get { object, record } => self
                .get(&object, &record)
                .await
                .map(AssistantResult::Record),
            Operation::List {
                object,
                conditions,
                sort,
                cursor,
            } => self
                .list(&object, &conditions, sort.as_ref(), cursor.as_deref())
                .await
                .map(AssistantResult::Records),
            Operation::Timeline {
                object,
                record,
                cursor,
            } => self
                .timeline(&object, &record, cursor.as_deref())
                .await
                .map(AssistantResult::Timeline),
            Operation::Actions { object, record } => self
                .actions(&object, record.as_deref())
                .await
                .map(AssistantResult::Actions),
            Operation::RunAction {
                object,
                action,
                record,
                values,
            } => self
                .prepare_action(&object, &action, &record, values)
                .await
                .map(AssistantResult::Confirmation),
            Operation::Create { object, values } => self
                .prepare_create(&object, values)
                .await
                .map(AssistantResult::Confirmation),
            Operation::Update {
                object,
                record,
                values,
            } => self
                .prepare_update(&object, &record, values)
                .await
                .map(AssistantResult::Confirmation),
            Operation::Commit { token } => {
                self.commit(&token).await.map(AssistantResult::Committed)
            }
            Operation::OnScreen => self.on_screen().await.map(AssistantResult::OnScreen),
        }
    }

    async fn metadata_version(&self) -> Result<i64, AssistantError> {
        self.bridge
            .host()
            .engine()
            .sync_positions()
            .await
            .map(|positions| positions.metadata_version.unwrap_or(0))
            .map_err(|_| AssistantError::new(AssistantCode::Unavailable))
    }

    /// An object the policy exposes and the member can read, with the
    /// fields they can read.
    async fn exposed(&self, object: &str) -> Result<ObjectMeta, AssistantError> {
        let meta = self.reader.object(object).await?;
        match meta {
            Some(meta) if self.policy.exposed(&meta.api_name) => Ok(meta),
            Some(meta) => Err(AssistantError::about(
                AssistantCode::ObjectNotExposed,
                &meta.plural_label,
            )),
            None => Err(AssistantError::new(AssistantCode::ObjectNotExposed)),
        }
    }

    /// The quick actions offered on `object`: the member can complete
    /// them, the policy offers them, and the object each writes is
    /// writable.
    async fn offered(&self, object: &str) -> Result<Vec<ActionMeta>, AssistantError> {
        Ok(self
            .reader
            .actions(object)
            .await?
            .into_iter()
            .filter(|action| {
                self.policy.offers(object, &action.name)
                    && action
                        .writes(object)
                        .is_some_and(|target| self.policy.writable(target))
            })
            .collect())
    }

    fn described_action(object: &str, action: &ActionMeta) -> DescribedAction {
        DescribedAction {
            name: action.name.clone(),
            label: action.label.clone(),
            kind: action.kind,
            object: object.to_owned(),
            target_object: action.writes(object).unwrap_or(object).to_owned(),
            fields: action.fields.clone(),
        }
    }

    /// Whether the object's records have a timeline the member reads: the
    /// timeline counts the sources that can point at the object's records,
    /// which depends on the object and the member, not on the record, so
    /// one record of it answers for all.
    async fn has_timeline(&self, object: &str) -> bool {
        let query = model::query_string(&[("limit", "1")]);
        let Ok(page) = self
            .reader
            .get(&format!(
                "/objects/{}/records?{query}",
                model::segment(object)
            ))
            .await
        else {
            return false;
        };
        let Some(id) = page["data"][0]["id"].as_str() else {
            return false;
        };
        self.reader
            .get(&format!(
                "/objects/{}/records/{}/timeline",
                model::segment(object),
                model::segment(id)
            ))
            .await
            .ok()
            .and_then(|timeline| timeline["sources"].as_u64())
            .is_some_and(|sources| sources > 0)
    }

    async fn describe(&self) -> Result<Description, AssistantError> {
        let metadata_version = self.metadata_version().await?;
        let mut objects = Vec::new();
        for summary in self.reader.objects().await? {
            if !self.policy.exposed(&summary.api_name) {
                continue;
            }
            let Some(meta) = self.reader.object(&summary.api_name).await? else {
                continue;
            };
            let actions = self
                .offered(&meta.api_name)
                .await?
                .iter()
                .map(|action| Self::described_action(&meta.api_name, action))
                .collect();
            let timeline = self.has_timeline(&meta.api_name).await;
            objects.push(DescribedObject {
                writable: self.policy.writable(&meta.api_name),
                fields: meta
                    .fields
                    .iter()
                    .map(|field| DescribedField {
                        api_name: field.api_name.clone(),
                        label: field.label.clone(),
                        field_type: field.field_type,
                        required: field.required,
                        read_only: field.computed() || !field.edit,
                        options: field
                            .options
                            .iter()
                            .map(|option| DescribedOption {
                                value: option.value.clone(),
                                label: option.label.clone(),
                            })
                            .collect(),
                        reference_object: field.reference_object.clone(),
                    })
                    .collect(),
                api_name: meta.api_name,
                label: meta.label,
                plural_label: meta.plural_label,
                name_label: meta.name_label,
                can_create: meta.can_create,
                can_edit: meta.can_edit,
                timeline,
                actions,
                metadata_version,
            });
        }
        Ok(Description {
            metadata_version,
            objects,
        })
    }

    async fn search(
        &self,
        text: &str,
        object: Option<&str>,
    ) -> Result<SearchResults, AssistantError> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > VALUE_TEXT_MAX {
            return Err(AssistantError::field(
                "text",
                "Search",
                "Say what to look for.".into(),
            ));
        }
        if let Some(object) = object {
            self.exposed(object).await?;
        }
        let limit = SEARCH_MAX.to_string();
        let mut pairs = vec![("q", text), ("limit", limit.as_str())];
        if let Some(object) = object {
            pairs.push(("object", object));
        }
        let body = self
            .reader
            .get(&format!("/search?{}", model::query_string(&pairs)))
            .await
            .map_err(|problem| from_problem(&problem, &|_| "Search".to_owned()))?;
        let mut results = Vec::new();
        for group in body["data"].as_array().into_iter().flatten() {
            let group_object = text_of(group, "object");
            if !self.policy.exposed(&group_object) {
                continue;
            }
            for record in group["records"].as_array().into_iter().flatten() {
                results.push(RecordRef {
                    object: group_object.clone(),
                    object_label: text_of(group, "label"),
                    id: text_of(record, "id"),
                    name: text_of(record, "name"),
                });
            }
        }
        results.truncate(SEARCH_MAX);
        Ok(SearchResults { results })
    }

    fn values_of(meta: &ObjectMeta, record: &Map<String, Value>) -> Vec<FieldValue> {
        let related = record
            .get("related")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        meta.fields
            .iter()
            .map(|field| {
                let value = record.get(&field.api_name).cloned().unwrap_or(Value::Null);
                FieldValue {
                    field: field.api_name.clone(),
                    label: field.label.clone(),
                    field_type: field.field_type,
                    display: display::display(field, &value, &related),
                    value,
                }
            })
            .collect()
    }

    async fn get(&self, object: &str, record: &str) -> Result<RecordView, AssistantError> {
        let meta = self.exposed(object).await?;
        let found = self
            .reader
            .record(&meta.api_name, record)
            .await?
            .ok_or_else(|| AssistantError::new(AssistantCode::NotFound))?;
        Ok(RecordView {
            object: meta.api_name.clone(),
            object_label: meta.label.clone(),
            id: found
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(record)
                .to_owned(),
            name: found
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            version: found.get("version").and_then(Value::as_i64).unwrap_or(0),
            fields: Self::values_of(&meta, &found),
            metadata_version: self.metadata_version().await?,
        })
    }

    /// One condition as query-language text, over a field the member reads.
    fn compile(meta: &ObjectMeta, condition: &Condition) -> Result<String, AssistantError> {
        let name = name_field(meta);
        let field = match condition.field() {
            "name" => &name,
            other => meta.field(other).ok_or_else(|| {
                AssistantError::field(other, other, "That field isn't available.".into())
            })?,
        };
        let refuse = || {
            AssistantError::field(
                &field.api_name,
                &field.label,
                format!("Assistants can't filter on {} that way.", field.label),
            )
        };
        let literal = |value: &Value, like: bool| -> Result<String, AssistantError> {
            literal(field, value, like).ok_or_else(refuse)
        };
        let path = &field.api_name;
        let textual = matches!(
            field.field_type,
            FieldType::Text
                | FieldType::LongText
                | FieldType::Email
                | FieldType::Phone
                | FieldType::Url
        );
        let ordered = matches!(
            field.field_type,
            FieldType::Number
                | FieldType::Currency
                | FieldType::Percent
                | FieldType::Date
                | FieldType::DateTime
        );
        if field.computed() || field.field_type == FieldType::RelatedTo {
            return Err(refuse());
        }
        // Lookups are answered through their references; other fields only
        // through an index (ADR 0019 §2).
        let referenced = matches!(
            field.field_type,
            FieldType::Lookup | FieldType::MasterDetail
        );
        if !field.indexed && !referenced {
            return Err(AssistantError::field(
                &field.api_name,
                &field.label,
                format!("Assistants can't filter on {}.", field.label),
            ));
        }
        let multi = field.field_type == FieldType::MultiPicklist;
        Ok(match condition {
            Condition::Equals { value, .. } if multi => {
                format!("{path} INCLUDES ({})", literal(value, false)?)
            }
            Condition::NotEquals { value, .. } if multi => {
                format!("{path} EXCLUDES ({})", literal(value, false)?)
            }
            Condition::Equals { value, .. } => format!("{path} = {}", literal(value, false)?),
            Condition::NotEquals { value, .. } => {
                format!("{path} != {}", literal(value, false)?)
            }
            Condition::Contains { value, .. } if textual => {
                let inner = quoted(value, true).ok_or_else(refuse)?;
                let inner = &inner[1..inner.len() - 1];
                if value.chars().count() > VALUE_TEXT_MAX || value.is_empty() {
                    return Err(refuse());
                }
                format!("{path} LIKE '%{inner}%'")
            }
            Condition::Before { value, .. } if ordered => {
                format!("{path} < {}", literal(value, false)?)
            }
            Condition::After { value, .. } if ordered => {
                format!("{path} > {}", literal(value, false)?)
            }
            Condition::Between { from, to, .. } if ordered => format!(
                "({path} >= {} AND {path} <= {})",
                literal(from, false)?,
                literal(to, false)?
            ),
            Condition::IsEmpty { .. } => format!("{path} = null"),
            _ => return Err(refuse()),
        })
    }

    async fn list(
        &self,
        object: &str,
        conditions: &[Condition],
        sort: Option<&Sort>,
        cursor: Option<&str>,
    ) -> Result<RecordPage, AssistantError> {
        let meta = self.exposed(object).await?;
        if conditions.len() > CONDITIONS_MAX {
            return Err(AssistantError {
                message: format!("Use at most {CONDITIONS_MAX} conditions."),
                ..AssistantError::new(AssistantCode::Invalid)
            });
        }
        let compiled = conditions
            .iter()
            .map(|condition| Self::compile(&meta, condition))
            .collect::<Result<Vec<_>, _>>()?;
        let order = match sort {
            None => "name".to_owned(),
            Some(sort) => {
                let field = match sort.field.as_str() {
                    "name" => "name",
                    other => match meta.field(other) {
                        Some(field)
                            if field.indexed
                                && !field.computed()
                                && !matches!(
                                    field.field_type,
                                    FieldType::LongText
                                        | FieldType::MultiPicklist
                                        | FieldType::Lookup
                                        | FieldType::MasterDetail
                                        | FieldType::RelatedTo
                                ) =>
                        {
                            field.api_name.as_str()
                        }
                        _ => {
                            return Err(AssistantError::field(
                                other,
                                &meta.label_of(other),
                                "Assistants can't sort on that.".into(),
                            ))
                        }
                    },
                };
                format!("{field} {}", if sort.descending { "DESC" } else { "ASC" })
            }
        };
        let mut query = format!("SELECT id, name FROM {}", meta.api_name);
        if !compiled.is_empty() {
            query.push_str(" WHERE ");
            query.push_str(&compiled.join(" AND "));
        }
        query.push_str(" ORDER BY ");
        query.push_str(&order);
        let page_size = LIST_PAGE_MAX.to_string();
        let mut pairs = vec![("q", query.as_str()), ("page_size", page_size.as_str())];
        if let Some(cursor) = cursor {
            pairs.push(("cursor", cursor));
        }
        let body = self
            .reader
            .get(&format!("/query?{}", model::query_string(&pairs)))
            .await
            .map_err(|problem| from_problem(&problem, &|field| meta.label_of(field)))?;
        Ok(RecordPage {
            object: meta.api_name.clone(),
            records: body["data"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|row| RecordRef {
                    object: meta.api_name.clone(),
                    object_label: meta.label.clone(),
                    id: text_of(row, "id"),
                    name: text_of(row, "name"),
                })
                .collect(),
            next_cursor: body["next_cursor"].as_str().map(str::to_owned),
        })
    }

    async fn timeline(
        &self,
        object: &str,
        record: &str,
        cursor: Option<&str>,
    ) -> Result<TimelineView, AssistantError> {
        let meta = self.exposed(object).await?;
        let query = cursor
            .map(|cursor| format!("?{}", model::query_string(&[("cursor", cursor)])))
            .unwrap_or_default();
        let body = match self
            .reader
            .get(&format!(
                "/objects/{}/records/{}/timeline{query}",
                model::segment(&meta.api_name),
                model::segment(record)
            ))
            .await
        {
            Ok(body) => body,
            Err(problem) if problem.status == 404 => {
                return Err(AssistantError::new(AssistantCode::NotFound))
            }
            Err(problem) => return Err(from_problem(&problem, &|field| field.to_owned())),
        };
        // Entries of objects the policy does not expose are left out.
        let entries = |key: &str| -> Vec<TimelineEntry> {
            body[key]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|item| self.policy.exposed(&text_of(item, "object")))
                .map(|item| TimelineEntry {
                    object: text_of(item, "object"),
                    object_label: text_of(item, "object_label"),
                    id: text_of(item, "id"),
                    name: text_of(item, "name"),
                    date: item["date"].as_str().map(str::to_owned),
                    open: item["open"].as_bool().unwrap_or(false),
                })
                .collect()
        };
        Ok(TimelineView {
            open: entries("open"),
            past: entries("past"),
            next_cursor: body["next_cursor"].as_str().map(str::to_owned),
        })
    }

    async fn actions(
        &self,
        object: &str,
        record: Option<&str>,
    ) -> Result<ActionList, AssistantError> {
        let meta = self.exposed(object).await?;
        if let Some(record) = record {
            if self.reader.record(&meta.api_name, record).await?.is_none() {
                return Err(AssistantError::new(AssistantCode::NotFound));
            }
        }
        Ok(ActionList {
            actions: self
                .offered(&meta.api_name)
                .await?
                .iter()
                .map(|action| Self::described_action(&meta.api_name, action))
                .collect(),
        })
    }

    async fn on_screen(&self) -> Result<OnScreen, AssistantError> {
        let nothing = OnScreen {
            workspace: self.workspace,
            object: None,
            record: None,
        };
        let Some(route) = self.service.0.routes.current() else {
            return Ok(nothing);
        };
        let Some(object) = route.object.filter(|_| route.workspace == self.workspace) else {
            return Ok(nothing);
        };
        // Passed on only for exposed objects the member can read, as ids.
        match self.exposed(&object).await {
            Ok(_) => Ok(OnScreen {
                workspace: self.workspace,
                object: Some(object),
                record: route.record_id,
            }),
            Err(error) if error.code == AssistantCode::ObjectNotExposed => Ok(nothing),
            Err(error) => Err(error),
        }
    }

    // --- Writes --------------------------------------------------------

    /// Checks that `values` names only fields the member may set on
    /// `meta`, the name included.
    fn check_values(
        meta: &ObjectMeta,
        values: &Map<String, Value>,
        allowed: Option<&[String]>,
    ) -> Result<(), AssistantError> {
        let mut problems = Vec::new();
        for key in values.keys() {
            let listed = allowed.is_none_or(|allowed| allowed.iter().any(|name| name == key));
            let message = if key == "name" {
                (!listed).then(|| format!("{} can't be set here.", meta.name_label))
            } else {
                match meta.field(key) {
                    None => Some("That field isn't available.".to_owned()),
                    Some(field) if field.computed() || !field.edit => {
                        Some(format!("{} can't be changed.", field.label))
                    }
                    Some(field) if !listed => Some(format!("{} can't be set here.", field.label)),
                    Some(_) => None,
                }
            };
            if let Some(message) = message {
                problems.push(FieldProblem {
                    field: key.clone(),
                    label: meta.label_of(key),
                    message,
                });
            }
        }
        if problems.is_empty() {
            Ok(())
        } else {
            Err(AssistantError::invalid(problems))
        }
    }

    /// The bridge the assistant writes through: its source and its marker.
    fn writer(&self, quick_action: Option<MarkedQuickAction>) -> Bridge {
        Bridge::new(self.bridge.host().for_assistant(AssistantMarker {
            assistant: self.assistant,
            quick_action,
        }))
    }

    /// Runs the write as a dry run (§5): an all-or-none composite request
    /// whose second operation always fails, so the engine runs every check
    /// of the save path (permissions, sharing, field-level security,
    /// required fields, validation rules, lookup filters, uniqueness,
    /// before-save automations) and then rolls everything back.
    async fn dry_run(
        &self,
        meta: &ObjectMeta,
        record: Option<(&str, i64)>,
        values: &Map<String, Value>,
    ) -> Result<(), AssistantError> {
        let operation = match record {
            None => json!({"method": "create", "reference": "change",
                           "object": meta.api_name, "values": values}),
            Some((id, version)) => json!({"method": "update", "reference": "change",
                "object": meta.api_name, "id": id, "expected_version": version,
                "values": values}),
        };
        let body = json!({"all_or_none": true, "operations": [
            operation,
            {"method": "update", "reference": "dry_run", "object": meta.api_name,
             "id": "@{dry_run_end.id}", "expected_version": 1, "values": {}},
        ]});
        let writer = self.writer(None);
        let reader = Reader {
            bridge: &writer,
            workspace: self.workspace,
        };
        let label = |field: &str| meta.label_of(field);
        let answer = reader
            .call("POST", "/composite", Some(&body))
            .await
            .map_err(|problem| from_problem(&problem, &label))?;
        let first = &answer["results"][0];
        let status = first["status"].as_u64().unwrap_or(0);
        let code = text_of(&first["body"], "code");
        if status == 409 && code == "composite.rolled_back" {
            return Ok(());
        }
        let problem = model::problem_of(
            u16::try_from(status).unwrap_or(500),
            &serde_json::to_vec(&first["body"]).unwrap_or_default(),
        );
        Err(from_problem(&problem, &label))
    }

    /// Each value as the member will confirm it.
    async fn changes(
        &self,
        meta: &ObjectMeta,
        values: &Map<String, Value>,
    ) -> Result<Vec<FieldValue>, AssistantError> {
        let name = name_field(meta);
        let mut changes = Vec::new();
        for (key, value) in values {
            let field = match key.as_str() {
                "name" => &name,
                other => meta.field(other).ok_or_else(failed)?,
            };
            let mut related = Map::new();
            if let (Some(id), Some(target)) = (value.as_str(), field.reference_object.as_deref()) {
                if let Some(found) = self.reader.record(target, id).await? {
                    related.insert(
                        id.to_owned(),
                        found.get("name").cloned().unwrap_or(Value::Null),
                    );
                }
            }
            changes.push(FieldValue {
                field: key.clone(),
                label: field.label.clone(),
                field_type: field.field_type,
                display: display::display(field, value, &related),
                value: value.clone(),
            });
        }
        Ok(changes)
    }

    fn not_writable(&self, meta: &ObjectMeta) -> AssistantError {
        AssistantError::about(AssistantCode::NotWritable, &meta.plural_label)
    }

    #[allow(clippy::too_many_arguments)]
    async fn confirmation(
        &self,
        write: PreparedWrite,
        meta: &ObjectMeta,
        record: Option<(String, i64)>,
        record_name: Option<String>,
        action_label: Option<String>,
        values: Map<String, Value>,
        metadata_version: i64,
    ) -> Result<Confirmation, AssistantError> {
        let changes = self.changes(meta, &values).await?;
        let kind = match write {
            PreparedWrite::Create => WriteKind::Create,
            PreparedWrite::Update => WriteKind::Update,
            PreparedWrite::RunAction { .. } => WriteKind::RunAction,
        };
        let token = self
            .service
            .0
            .confirmations
            .issue(Prepared {
                workspace: self.workspace,
                member: self.member,
                assistant: self.assistant,
                write,
                object: meta.api_name.clone(),
                record: record.as_ref().map(|(id, _)| id.clone()),
                record_version: record.as_ref().map(|(_, version)| *version),
                metadata_version,
                values,
            })
            .ok_or_else(failed)?;
        Ok(Confirmation {
            token,
            expires_at: Utc::now()
                + chrono::Duration::from_std(confirm::CONFIRMATION_TTL)
                    .unwrap_or_else(|_| chrono::Duration::minutes(2)),
            write: kind,
            object: meta.api_name.clone(),
            object_label: meta.label.clone(),
            record_name,
            action_label,
            changes,
        })
    }

    async fn prepare_create(
        &self,
        object: &str,
        values: Map<String, Value>,
    ) -> Result<Confirmation, AssistantError> {
        let meta = self.exposed(object).await?;
        if !self.policy.writable(&meta.api_name) {
            return Err(self.not_writable(&meta));
        }
        if !meta.can_create {
            return Err(AssistantError::new(AssistantCode::NotPermitted));
        }
        if values.is_empty() {
            return Err(AssistantError::new(AssistantCode::Invalid));
        }
        Self::check_values(&meta, &values, None)?;
        let metadata_version = self.metadata_version().await?;
        self.dry_run(&meta, None, &values).await?;
        self.confirmation(
            PreparedWrite::Create,
            &meta,
            None,
            None,
            None,
            values,
            metadata_version,
        )
        .await
    }

    async fn prepare_update(
        &self,
        object: &str,
        record: &str,
        values: Map<String, Value>,
    ) -> Result<Confirmation, AssistantError> {
        let meta = self.exposed(object).await?;
        let found = self
            .reader
            .record(&meta.api_name, record)
            .await?
            .ok_or_else(|| AssistantError::new(AssistantCode::NotFound))?;
        if !self.policy.writable(&meta.api_name) {
            return Err(self.not_writable(&meta));
        }
        Self::check_values(&meta, &values, None)?;
        // Only what would change is confirmed, and only that is sent.
        let values: Map<String, Value> = values
            .into_iter()
            .filter(|(key, value)| found.get(key).unwrap_or(&Value::Null) != value)
            .collect();
        if values.is_empty() {
            return Err(AssistantError {
                message: "Nothing would change.".into(),
                ..AssistantError::new(AssistantCode::Invalid)
            });
        }
        let id = text_of(&Value::Object(found.clone()), "id");
        let version = found
            .get("version")
            .and_then(Value::as_i64)
            .ok_or_else(failed)?;
        let metadata_version = self.metadata_version().await?;
        self.dry_run(&meta, Some((&id, version)), &values).await?;
        self.confirmation(
            PreparedWrite::Update,
            &meta,
            Some((id, version)),
            Some(text_of(&Value::Object(found), "name")),
            None,
            values,
            metadata_version,
        )
        .await
    }

    async fn prepare_action(
        &self,
        object: &str,
        name: &str,
        record: &str,
        values: Map<String, Value>,
    ) -> Result<Confirmation, AssistantError> {
        let meta = self.exposed(object).await?;
        let action = self
            .offered(&meta.api_name)
            .await?
            .into_iter()
            .find(|action| action.name == name);
        let Some(action) = action else {
            // Whether it exists, is not offered or writes an object that
            // is not writable, the answer is the same.
            let exists = self
                .reader
                .actions(&meta.api_name)
                .await?
                .into_iter()
                .find(|action| action.name == name);
            return Err(match exists {
                Some(action)
                    if self.policy.offers(&meta.api_name, name)
                        && !action
                            .writes(&meta.api_name)
                            .is_some_and(|target| self.policy.writable(target)) =>
                {
                    AssistantError::new(AssistantCode::NotWritable)
                }
                _ => AssistantError::new(AssistantCode::ActionNotOffered),
            });
        };
        let found = self
            .reader
            .record(&meta.api_name, record)
            .await?
            .ok_or_else(|| AssistantError::new(AssistantCode::NotFound))?;
        let id = text_of(&Value::Object(found.clone()), "id");
        let version = found
            .get("version")
            .and_then(Value::as_i64)
            .ok_or_else(failed)?;
        let target = match action.kind {
            QuickActionKind::Update => meta.clone(),
            QuickActionKind::Create => {
                let target = action.target_object.as_deref().ok_or_else(failed)?;
                self.reader
                    .object(target)
                    .await?
                    .ok_or_else(|| AssistantError::new(AssistantCode::NotPermitted))?
            }
        };
        Self::check_values(&target, &values, Some(&action.fields))?;
        let mut values = values;
        for (key, value) in &action.presets {
            values.insert(key.clone(), value.clone());
        }
        if let (QuickActionKind::Create, Some(field)) = (action.kind, &action.target_field) {
            values.insert(field.clone(), Value::String(id.clone()));
        }
        let metadata_version = self.metadata_version().await?;
        let on_record = match action.kind {
            QuickActionKind::Update => Some((id.as_str(), version)),
            QuickActionKind::Create => None,
        };
        self.dry_run(&target, on_record, &values).await?;
        self.confirmation(
            PreparedWrite::RunAction {
                action_object: meta.api_name.clone(),
                action: action.name.clone(),
                creates: action.kind == QuickActionKind::Create,
            },
            &target,
            Some((id, version)),
            Some(text_of(&Value::Object(found), "name")),
            Some(action.label.clone()),
            values,
            metadata_version,
        )
        .await
    }

    async fn commit(&self, token: &str) -> Result<Committed, AssistantError> {
        let prepared =
            self.service
                .0
                .confirmations
                .take(token)
                .map_err(|refused| match refused {
                    TokenRefused::Invalid | TokenRefused::Expired => {
                        AssistantError::new(AssistantCode::ConfirmationInvalid)
                    }
                })?;
        if prepared.workspace != self.workspace
            || prepared.member != self.member
            || prepared.assistant != self.assistant
        {
            return Err(AssistantError::new(AssistantCode::ConfirmationInvalid));
        }
        let stale = || AssistantError::new(AssistantCode::ConfirmationStale);
        if self.metadata_version().await? != prepared.metadata_version {
            return Err(stale());
        }
        // The record the change was prepared against, unchanged.
        let (action_object, quick_action) = match &prepared.write {
            PreparedWrite::RunAction {
                action_object,
                action,
                ..
            } => (
                action_object.clone(),
                Some(MarkedQuickAction {
                    object: action_object.clone(),
                    name: action.clone(),
                }),
            ),
            _ => (prepared.object.clone(), None),
        };
        if let (Some(record), Some(version)) = (&prepared.record, prepared.record_version) {
            let found = self.reader.record(&action_object, record).await?;
            if found.and_then(|found| found.get("version").and_then(Value::as_i64)) != Some(version)
            {
                return Err(stale());
            }
        }
        // The policy in force now, which a moved metadata version would
        // already have caught; checked again so a write never outruns it.
        let meta = self
            .reader
            .object(&prepared.object)
            .await?
            .ok_or_else(stale)?;
        if !self.policy.writable(&prepared.object) {
            return Err(self.not_writable(&meta));
        }
        if let Some(action) = &quick_action {
            if !self.policy.offers(&action.object, &action.name) {
                return Err(AssistantError::new(AssistantCode::ActionNotOffered));
            }
        }
        let creates = match &prepared.write {
            PreparedWrite::Create => true,
            PreparedWrite::Update => false,
            PreparedWrite::RunAction { creates, .. } => *creates,
        };
        let writer = self.writer(quick_action);
        let reader = Reader {
            bridge: &writer,
            workspace: self.workspace,
        };
        let label = |field: &str| meta.label_of(field);
        let written = if creates {
            reader
                .call(
                    "POST",
                    &format!("/objects/{}/records", model::segment(&prepared.object)),
                    Some(&Value::Object(prepared.values.clone())),
                )
                .await
        } else {
            let record = prepared.record.as_deref().ok_or_else(failed)?;
            let mut body = prepared.values.clone();
            body.insert(
                "expected_version".into(),
                json!(prepared.record_version.ok_or_else(failed)?),
            );
            reader
                .call(
                    "PATCH",
                    &format!(
                        "/objects/{}/records/{}",
                        model::segment(&prepared.object),
                        model::segment(record)
                    ),
                    Some(&Value::Object(body)),
                )
                .await
        }
        .map_err(|problem| from_problem(&problem, &label))?;
        if let Some(pushed) = &self.pushed {
            pushed();
        }
        Ok(Committed {
            object: prepared.object.clone(),
            id: text_of(&written, "id"),
            name: text_of(&written, "name"),
            version: written["version"].as_i64().unwrap_or(0),
        })
    }
}

fn text_of(value: &Value, key: &str) -> String {
    text(value, key)
}

/// A typed value as a query-language literal for `field`, or `None` when
/// it does not suit the field.
fn literal(field: &FieldMeta, value: &Value, like: bool) -> Option<String> {
    match field.field_type {
        FieldType::Text
        | FieldType::LongText
        | FieldType::Email
        | FieldType::Phone
        | FieldType::Url
        | FieldType::Picklist
        | FieldType::MultiPicklist => {
            let text = value.as_str()?;
            (text.chars().count() <= VALUE_TEXT_MAX)
                .then(|| quoted(text, like))
                .flatten()
        }
        FieldType::Lookup | FieldType::MasterDetail => {
            let id = value.as_str()?;
            let plain = !id.is_empty()
                && id.len() <= 64
                && id
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '-');
            plain.then(|| format!("'{id}'"))
        }
        FieldType::Number | FieldType::Currency | FieldType::Percent => {
            let number = value.as_f64()?;
            number.is_finite().then(|| match value {
                Value::Number(number) => number.to_string(),
                _ => String::new(),
            })
        }
        FieldType::Boolean => value.as_bool().map(|flag| flag.to_string()),
        FieldType::Date => {
            let text = value.as_str()?;
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .map(|date| date.format("%Y-%m-%d").to_string())
        }
        FieldType::DateTime => {
            let text = value.as_str()?;
            if let Ok(date) = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d") {
                return Some(date.format("%Y-%m-%d").to_string());
            }
            chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| {
                time.with_timezone(&Utc)
                    .format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string()
            })
        }
        FieldType::Formula | FieldType::Rollup | FieldType::RelatedTo => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_escaped_and_typed() {
        assert_eq!(
            quoted("O'Brien \\ 100%", false).as_deref(),
            Some("'O\\'Brien \\\\ 100%'")
        );
        assert_eq!(quoted("100%_", true).as_deref(), Some("'100\\%\\_'"));
        assert_eq!(quoted("bell\u{7}", false), None);
        let mut field = name_field(&ObjectMeta {
            api_name: "custom:account".into(),
            label: "Account".into(),
            plural_label: "Accounts".into(),
            name_label: "Name".into(),
            can_create: true,
            can_edit: true,
            fields: Vec::new(),
        });
        assert_eq!(
            literal(&field, &json!("x' OR 1=1"), false).as_deref(),
            Some("'x\\' OR 1=1'")
        );
        assert_eq!(literal(&field, &json!(3), false), None);
        field.field_type = FieldType::Number;
        assert_eq!(literal(&field, &json!(3.5), false).as_deref(), Some("3.5"));
        assert_eq!(literal(&field, &json!("3"), false), None);
        field.field_type = FieldType::Date;
        assert_eq!(
            literal(&field, &json!("2026-11-18"), false).as_deref(),
            Some("2026-11-18")
        );
        assert_eq!(literal(&field, &json!("TODAY"), false), None);
        field.field_type = FieldType::Lookup;
        assert_eq!(literal(&field, &json!("a01-1 OR"), false), None);
    }

    #[test]
    fn device_settings_default_to_access_on_and_indexing_off() {
        let settings = DeviceAssistantSettings::default();
        assert!(settings.assistant_access);
        assert!(!settings.indexing);
        let stored: DeviceAssistantSettings = serde_json::from_value(json!({})).expect("settings");
        assert_eq!(stored, settings);
    }
}
