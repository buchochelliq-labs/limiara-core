//! The index stream (ADR 0034 §7): what a native plugin may put into the
//! OS's semantic or search index, as a stream of updates it applies and
//! nothing else. Native plugins never decide what to index.
//!
//! The bridge turns the replica change feed into these updates, already
//! filtered:
//!
//! - **three opt-ins**: the workspace switch, the object's `indexable`
//!   setting, and the member's `indexing` setting on this device, which is
//!   off until they turn it on. Turning any of them off removes the entries
//!   it covered;
//! - **only what is needed**: the record's id, its object's label, its name
//!   (unless the policy excludes it) and the fields the object's policy
//!   does not exclude, and nothing the member cannot read, since every
//!   record is read back through the engine as the member;
//! - **grouped by workspace**, so one update removes a workspace's entries;
//! - **expiring on their own**: each entry expires after the offline lease
//!   length from when it was written, and each sync that reaches the server
//!   moves the expiry of the rest (`refresh`). A lease that locks does not
//!   clear the index.
//!
//! Sign-out and a wipe remove the workspace's entries before its replica
//! goes (`workspace_cleared` is the feed's last event for it). At launch,
//! [`sweep`] tells the plugin which workspaces' entries to delete.

use super::model::{FieldMeta, PolicyMeta, Reader};
use super::{AssistantService, DeviceAssistantSettings};
use crate::feed::{ChangeSubscription, ReplicaChange};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

/// How often the stream looks for a sync that moved the lease, when the
/// feed is quiet.
const REFRESH_EVERY: Duration = Duration::from_secs(30);
/// Records read per page when the index is rebuilt.
const REBUILD_PAGE: &str = "100";

/// One field an entry carries, as it reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexedField {
    pub field: String,
    pub label: String,
    pub display: String,
}

/// One record as the OS index may hold it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexEntry {
    /// The entry's group: a domain identifier on iOS, a namespace on
    /// Android.
    pub workspace: Uuid,
    pub object: String,
    pub object_label: String,
    pub record: String,
    /// `None` when the policy excludes the name: the entry carries only the
    /// object's label.
    pub name: Option<String>,
    pub fields: Vec<IndexedField>,
    /// When the OS drops the entry unless a later update moves it.
    pub expires_at: DateTime<Utc>,
}

/// What the plugin does to the OS index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IndexUpdate {
    /// Adds the entry, or replaces the one for the same record.
    Upsert { entry: IndexEntry },
    Remove {
        workspace: Uuid,
        object: String,
        record: String,
    },
    /// Removes every entry of one object.
    RemoveObject { workspace: Uuid, object: String },
    /// Removes every entry of the workspace.
    RemoveWorkspace { workspace: Uuid },
    /// Removes every entry of the workspace; the upserts that follow are
    /// all it should hold.
    Rebuild { workspace: Uuid },
    /// A sync reached the server: every entry of the workspace now expires
    /// at `expires_at`.
    Refresh {
        workspace: Uuid,
        expires_at: DateTime<Utc>,
    },
}

/// What the launch-time sweep removes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IndexSweep {
    /// Workspaces whose entries go: every one the shell does not hold, and
    /// the one it holds when the member's device setting keeps indexing
    /// off.
    pub remove_workspaces: Vec<Uuid>,
    /// Entries that expired before this time go too.
    pub remove_expired_before: DateTime<Utc>,
}

/// At launch, after the shell reopened what it holds: of the workspaces
/// the OS index has entries for, which to clear.
pub(crate) fn sweep(service: &AssistantService, indexed: &[Uuid]) -> IndexSweep {
    let held = service
        .workspace()
        .filter(|_| service.device_settings().indexing);
    let mut remove_workspaces: Vec<Uuid> = indexed
        .iter()
        .copied()
        .filter(|workspace| Some(*workspace) != held)
        .collect();
    remove_workspaces.sort();
    remove_workspaces.dedup();
    IndexSweep {
        remove_workspaces,
        remove_expired_before: Utc::now(),
    }
}

/// What the stream indexes of one object.
#[derive(Debug, Clone, PartialEq)]
struct ObjectSnapshot {
    label: String,
    name_excluded: bool,
    fields: Vec<FieldMeta>,
}

/// The index stream. Each plugin holding `device.assistant` gets its own.
pub struct IndexStream {
    service: AssistantService,
    feed: ChangeSubscription,
    settings: watch::Receiver<DeviceAssistantSettings>,
    queue: VecDeque<IndexUpdate>,
    /// The workspace the index holds entries for, with what it indexes;
    /// `None` while it holds none.
    indexed: Option<(Uuid, BTreeMap<String, ObjectSnapshot>)>,
    /// A rebuild that could not read the replica yet, as while it is
    /// locked; it runs when the replica opens again.
    rebuild_waiting: bool,
    last_contact: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for IndexStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("IndexStream")
            .field("queued", &self.queue.len())
            .finish_non_exhaustive()
    }
}

/// Why a read of the replica stopped.
enum Unread {
    /// Nothing may be indexed: remove what is.
    Off,
    /// The replica cannot answer now (locked, unavailable): keep what is.
    Later,
}

impl IndexStream {
    pub(crate) fn new(service: AssistantService) -> Self {
        let feed = service.0.feed.subscribe();
        let mut settings = service.0.settings.subscribe();
        settings.mark_unchanged();
        Self {
            service,
            feed,
            settings,
            queue: VecDeque::new(),
            indexed: None,
            rebuild_waiting: false,
            last_contact: None,
        }
    }

    /// The next update, waiting for one.
    pub async fn next(&mut self) -> IndexUpdate {
        loop {
            if let Some(update) = self.queue.pop_front() {
                return update;
            }
            tokio::select! {
                event = self.feed.next() => self.on_change(event.workspace, event.change).await,
                changed = self.settings.changed() => {
                    if changed.is_ok() {
                        self.on_settings().await;
                    } else {
                        // The service is gone with the app.
                        std::future::pending::<()>().await;
                    }
                }
                () = tokio::time::sleep(REFRESH_EVERY) => {}
            }
            self.check_refresh().await;
        }
    }

    /// The next update, waiting at most `timeout`.
    pub async fn next_within(&mut self, timeout: Duration) -> Option<IndexUpdate> {
        tokio::time::timeout(timeout, self.next()).await.ok()
    }

    /// Every update the changes already published call for, without
    /// waiting: the feed's waiting events and the member's setting are
    /// taken in turn until neither has more. A plugin that applies the
    /// index in batches, and the reference harness (ADR 0034 slice 4),
    /// which must not depend on timing, call it after each step.
    pub async fn ready(&mut self) -> Vec<IndexUpdate> {
        loop {
            let mut progressed = false;
            while let Some(event) = self.feed.try_next() {
                self.on_change(event.workspace, event.change).await;
                self.check_refresh().await;
                progressed = true;
            }
            if self.settings.has_changed().unwrap_or(false) {
                self.on_settings().await;
                self.check_refresh().await;
                progressed = true;
            }
            if !progressed {
                break;
            }
        }
        self.check_refresh().await;
        self.queue.drain(..).collect()
    }

    fn remove_workspace(&mut self) {
        if let Some((workspace, _)) = self.indexed.take() {
            self.queue
                .push_back(IndexUpdate::RemoveWorkspace { workspace });
        }
    }

    async fn on_settings(&mut self) {
        let indexing = self.settings.borrow_and_update().indexing;
        if indexing {
            self.rebuild().await;
        } else {
            self.rebuild_waiting = false;
            self.remove_workspace();
        }
    }

    async fn on_change(&mut self, workspace: Option<Uuid>, change: ReplicaChange) {
        // Entries of a workspace other than the open one go.
        if let (Some((held, _)), Some(workspace)) = (&self.indexed, workspace) {
            if *held != workspace {
                self.remove_workspace();
            }
        }
        match change {
            ReplicaChange::Rebuild { .. } => self.rebuild().await,
            ReplicaChange::WorkspaceCleared { .. } => {
                self.rebuild_waiting = false;
                self.last_contact = None;
                match (self.indexed.take(), workspace) {
                    (Some((held, _)), _) => self
                        .queue
                        .push_back(IndexUpdate::RemoveWorkspace { workspace: held }),
                    // Nothing indexed here may still have entries from an
                    // earlier launch; they go too.
                    (None, Some(workspace)) => self
                        .queue
                        .push_back(IndexUpdate::RemoveWorkspace { workspace }),
                    (None, None) => {}
                }
            }
            // A lease that locks keeps the index; entries expire on their own.
            ReplicaChange::WorkspaceLocked => {}
            ReplicaChange::WorkspaceUnlocked => {
                if self.rebuild_waiting {
                    self.rebuild().await;
                }
            }
            ReplicaChange::DefinitionsChanged { .. } => self.definitions_changed().await,
            ReplicaChange::ObjectRemoved { object, .. } => {
                if let Some((held, objects)) = &mut self.indexed {
                    if objects.remove(&object).is_some() {
                        self.queue.push_back(IndexUpdate::RemoveObject {
                            workspace: *held,
                            object,
                        });
                    }
                }
            }
            ReplicaChange::Removed { object, record, .. } => {
                if let Some((held, objects)) = &self.indexed {
                    if objects.contains_key(&object) {
                        self.queue.push_back(IndexUpdate::Remove {
                            workspace: *held,
                            object,
                            record,
                        });
                    }
                }
            }
            ReplicaChange::Upserted { object, record } => self.upserted(object, record).await,
        }
    }

    /// The policy, the snapshot of what to index and the lease length,
    /// read through the bridge as the member.
    async fn read_snapshot(
        &self,
    ) -> Result<(Uuid, BTreeMap<String, ObjectSnapshot>, Option<i64>), Unread> {
        if !self.service.device_settings().indexing {
            return Err(Unread::Off);
        }
        let _gate = self.service.0.gate.read().await;
        let Some((bridge, _)) = self.service.open_bridge() else {
            return Err(Unread::Off);
        };
        let workspace = bridge.host().caller().tenant_id;
        let reader = Reader {
            bridge: &bridge,
            workspace,
        };
        let policy = reader.policy().await.map_err(|_| Unread::Later)?;
        if !policy.enabled {
            return Err(Unread::Off);
        }
        let objects = snapshot(&reader, &policy)
            .await
            .map_err(|_| Unread::Later)?;
        let limit = bridge
            .host()
            .engine()
            .lease()
            .await
            .map_err(|_| Unread::Later)?
            .offline_limit_seconds;
        Ok((workspace, objects, limit))
    }

    /// Entries for every record of `objects`.
    async fn entries(
        &self,
        workspace: Uuid,
        objects: &BTreeMap<String, ObjectSnapshot>,
        limit: Option<i64>,
    ) -> Result<Vec<IndexEntry>, Unread> {
        let _gate = self.service.0.gate.read().await;
        let Some((bridge, _)) = self.service.open_bridge() else {
            return Err(Unread::Off);
        };
        if bridge.host().caller().tenant_id != workspace {
            return Err(Unread::Later);
        }
        let reader = Reader {
            bridge: &bridge,
            workspace,
        };
        let expires_at = expiry(limit);
        let mut entries = Vec::new();
        for (object, snapshot) in objects {
            let mut cursor: Option<String> = None;
            loop {
                let mut pairs = vec![("limit", REBUILD_PAGE)];
                if let Some(cursor) = &cursor {
                    pairs.push(("cursor", cursor.as_str()));
                }
                let page = reader
                    .get(&format!(
                        "/objects/{}/records?{}",
                        super::model::segment(object),
                        super::model::query_string(&pairs)
                    ))
                    .await
                    .map_err(|_| Unread::Later)?;
                for record in page["data"].as_array().into_iter().flatten() {
                    if let Some(record) = record.as_object() {
                        entries.push(entry(workspace, object, snapshot, record, expires_at));
                    }
                }
                cursor = page["next_cursor"].as_str().map(str::to_owned);
                if cursor.is_none() {
                    break;
                }
            }
        }
        Ok(entries)
    }

    async fn rebuild(&mut self) {
        let read = match self.read_snapshot().await {
            Ok((workspace, objects, limit)) => self
                .entries(workspace, &objects, limit)
                .await
                .map(|entries| (workspace, objects, entries)),
            Err(unread) => Err(unread),
        };
        match read {
            Ok((workspace, objects, entries)) => {
                self.rebuild_waiting = false;
                self.queue.push_back(IndexUpdate::Rebuild { workspace });
                self.queue.extend(
                    entries
                        .into_iter()
                        .map(|entry| IndexUpdate::Upsert { entry }),
                );
                self.indexed = Some((workspace, objects));
            }
            Err(Unread::Off) => {
                self.rebuild_waiting = false;
                self.remove_workspace();
            }
            Err(Unread::Later) => self.rebuild_waiting = true,
        }
    }

    /// The policy, labels or fields moved: objects no longer indexable lose
    /// their entries, and objects whose indexed shape changed are written
    /// again.
    async fn definitions_changed(&mut self) {
        let Some((held, before)) = self.indexed.clone() else {
            // Nothing indexed: a policy that now allows it starts the index.
            self.rebuild().await;
            return;
        };
        let (workspace, after, limit) = match self.read_snapshot().await {
            Ok(read) => read,
            Err(Unread::Off) => {
                self.remove_workspace();
                return;
            }
            Err(Unread::Later) => {
                self.rebuild_waiting = true;
                return;
            }
        };
        if workspace != held {
            self.rebuild().await;
            return;
        }
        let changed: BTreeMap<String, ObjectSnapshot> = after
            .iter()
            .filter(|(object, snapshot)| before.get(*object) != Some(*snapshot))
            .map(|(object, snapshot)| (object.clone(), snapshot.clone()))
            .collect();
        let entries = match self.entries(workspace, &changed, limit).await {
            Ok(entries) => entries,
            Err(_) => {
                self.rebuild_waiting = true;
                return;
            }
        };
        for object in before.keys() {
            if !after.contains_key(object) || changed.contains_key(object) {
                self.queue.push_back(IndexUpdate::RemoveObject {
                    workspace,
                    object: object.clone(),
                });
            }
        }
        self.queue.extend(
            entries
                .into_iter()
                .map(|entry| IndexUpdate::Upsert { entry }),
        );
        self.indexed = Some((workspace, after));
    }

    async fn upserted(&mut self, object: String, record: String) {
        let Some((workspace, objects)) = &self.indexed else {
            return;
        };
        let workspace = *workspace;
        let Some(snapshot) = objects.get(&object).cloned() else {
            return;
        };
        let read = {
            let _gate = self.service.0.gate.read().await;
            let Some((bridge, _)) = self.service.open_bridge() else {
                return;
            };
            if bridge.host().caller().tenant_id != workspace {
                return;
            }
            let reader = Reader {
                bridge: &bridge,
                workspace,
            };
            let limit = bridge
                .host()
                .engine()
                .lease()
                .await
                .ok()
                .and_then(|lease| lease.offline_limit_seconds);
            reader
                .record(&object, &record)
                .await
                .map(|found| (found, limit))
        };
        match read {
            // Read back as the member: a record they can no longer read
            // leaves the index.
            Ok((Some(found), limit)) => self.queue.push_back(IndexUpdate::Upsert {
                entry: entry(workspace, &object, &snapshot, &found, expiry(limit)),
            }),
            Ok((None, _)) => self.queue.push_back(IndexUpdate::Remove {
                workspace,
                object,
                record,
            }),
            // Locked or unavailable: the entry stays as it was.
            Err(_) => {}
        }
    }

    /// A sync that reached the server moves every entry's expiry.
    async fn check_refresh(&mut self) {
        let Some((workspace, _)) = &self.indexed else {
            return;
        };
        let workspace = *workspace;
        let lease = {
            let _gate = self.service.0.gate.read().await;
            let Some((bridge, _)) = self.service.open_bridge() else {
                return;
            };
            if bridge.host().caller().tenant_id != workspace {
                return;
            }
            bridge.host().engine().lease().await
        };
        let Ok(lease) = lease else {
            return;
        };
        let (Some(contact), Some(limit)) = (lease.last_contact_at, lease.offline_limit_seconds)
        else {
            return;
        };
        match self.last_contact {
            Some(seen) if seen >= contact => {}
            Some(_) => {
                self.last_contact = Some(contact);
                self.queue.push_back(IndexUpdate::Refresh {
                    workspace,
                    expires_at: contact + chrono::Duration::seconds(limit),
                });
            }
            None => self.last_contact = Some(contact),
        }
    }
}

fn expiry(limit: Option<i64>) -> DateTime<Utc> {
    // Without a lease length (a workspace that never syncs) an entry
    // expires at once.
    Utc::now() + chrono::Duration::seconds(limit.unwrap_or(0).max(0))
}

/// What to index of each indexable object the member reads.
async fn snapshot(
    reader: &Reader<'_>,
    policy: &PolicyMeta,
) -> Result<BTreeMap<String, ObjectSnapshot>, super::errors::AssistantError> {
    let mut objects = BTreeMap::new();
    for summary in reader.objects().await? {
        if !policy.indexable(&summary.api_name) {
            continue;
        }
        let Some(meta) = reader.object(&summary.api_name).await? else {
            continue;
        };
        let excluded = policy
            .objects
            .get(&meta.api_name)
            .map(|policy| policy.excluded_fields.clone())
            .unwrap_or_default();
        objects.insert(
            meta.api_name.clone(),
            ObjectSnapshot {
                label: meta.label.clone(),
                name_excluded: excluded.iter().any(|field| field == "name"),
                fields: meta
                    .fields
                    .into_iter()
                    .filter(|field| !excluded.contains(&field.api_name))
                    .collect(),
            },
        );
    }
    Ok(objects)
}

fn entry(
    workspace: Uuid,
    object: &str,
    snapshot: &ObjectSnapshot,
    record: &Map<String, Value>,
    expires_at: DateTime<Utc>,
) -> IndexEntry {
    let related = record
        .get("related")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    IndexEntry {
        workspace,
        object: object.to_owned(),
        object_label: snapshot.label.clone(),
        record: record
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: (!snapshot.name_excluded).then(|| {
            record
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        }),
        fields: snapshot
            .fields
            .iter()
            .filter_map(|field| {
                let value = record.get(&field.api_name)?;
                let display = super::display::display(field, value, &related);
                (!display.is_empty()).then(|| IndexedField {
                    field: field.api_name.clone(),
                    label: field.label.clone(),
                    display,
                })
            })
            .collect(),
        expires_at,
    }
}
