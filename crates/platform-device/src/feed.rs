//! The replica change feed (ADR 0034 §9): what changed in the device's
//! replica, for native plugins that keep something derived from it, such as
//! an OS search index. The web view cannot reach it: no route of the bridge
//! serves it, and a plugin subscribes only through the shell's grant of
//! `device.replica_changes` ([`crate::capabilities`]).
//!
//! Events carry ids only, never field values. A plugin that needs content
//! reads it back through the engine as the member. Events come from the
//! points where the replica changes:
//!
//! - the member's own writes, read from the device's change log after each
//!   write through the bridge and after each sync step
//!   ([`FeedTap::local_writes`]);
//! - what the server's side of sync commits, from the replica engine's
//!   observer ([`FeedTap`] is a [`ReplicaObserver`]): records a pull, a push
//!   answer or a settled conflict stored, records forgotten when the member
//!   lost access, objects the definitions retired, and the metadata version
//!   the replica follows;
//! - the offline lease, when it locks or reopens;
//! - sign-out and wipe ([`ReplicaFeed::clear`]).
//!
//! **Ordering.** Every event gets the next sequence number when it is
//! published, under one lock, so subscribers see the same events in the
//! same order: the order in which their commits were published. Delivery is
//! at least once while the app runs; an event may say again what an earlier
//! one said, and a plugin re-reads rather than counting.
//!
//! **Bounded buffer.** The feed keeps the latest `capacity` events. A
//! subscriber that falls further behind than that loses its place: its next
//! event is `rebuild` (`fell_behind`), and it continues with events
//! published after it. A new subscriber starts with `rebuild`
//! (`subscribed`), and every subscriber gets `rebuild` (`opened`) when a
//! workspace opens. The feed is not persisted: the rebuild at launch covers
//! what happened while the app was not running.
//!
//! **One workspace at a time.** Each event names the workspace it is for.
//! `workspace_cleared` is the last event for a workspace: whatever its
//! engine still reports after sign-out or a wipe is dropped.

use platform_objects::{
    ChangeKind, Lease, ObjectError, ObjectService, ReplicaEvent, ReplicaObserver,
};
use serde::Serialize;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Notify;
use uuid::Uuid;

/// How many events the feed keeps for subscribers that fall behind.
pub const DEFAULT_CAPACITY: usize = 1024;

/// Change-log entries read per page.
const LOCAL_PAGE: u16 = 200;

/// Why a record left what the member has on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalReason {
    /// Deleted, on the device or on the server, including a delete that won
    /// a conflict. It may be in the recycle bin.
    Deleted,
    /// The member can no longer read it: sharing or permissions changed on
    /// the server (the revocation set, ADR 0024 §4).
    AccessLost,
}

/// Why every record of an object left at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectRemovalReason {
    /// The server no longer describes the object to the member: it was
    /// deleted, or the member can no longer read it.
    LeftReplica,
    /// The object belongs to a plugin, and the server no longer describes
    /// it: the plugin was suspended (ADR 0033 §2). The server hides a
    /// suspended plugin's objects from every read, so the device knows a
    /// plugin object left, not which of the two happened; the plugin is the
    /// likelier cause, and the consequence is the same.
    PluginSuspended,
}

/// Why the workspace's copy is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClearReason {
    /// The member signed the device out.
    SignedOut,
    /// The server refused the device, so its copy was wiped (ADR 0029 §4).
    Wiped,
}

/// Why a subscriber should rebuild from the records it can list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RebuildReason {
    /// It has just subscribed.
    Subscribed,
    /// It fell further behind than the feed's buffer holds.
    FellBehind,
    /// A workspace opened: at launch, or after a sign-in.
    Opened,
}

/// One change, by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplicaChange {
    /// Created, changed or undeleted.
    Upserted { object: String, record: String },
    Removed {
        object: String,
        record: String,
        reason: RemovalReason,
    },
    /// Every record of the object at once.
    ObjectRemoved {
        object: String,
        reason: ObjectRemovalReason,
    },
    /// Fields, labels or other definitions moved: a plugin re-reads what it
    /// derives from them.
    DefinitionsChanged { metadata_version: i64 },
    /// The offline lease ran out; the replica refuses the member until the
    /// next sync.
    WorkspaceLocked,
    /// A sync reopened a locked replica.
    WorkspaceUnlocked,
    /// Sign-out or wipe: the last event for the workspace.
    WorkspaceCleared { reason: ClearReason },
    /// Rebuild from the records the member can list.
    Rebuild { reason: RebuildReason },
}

/// An event as a subscriber receives it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FeedEvent {
    /// Its place in the feed. Published events count up from 1 with no
    /// gaps; a `rebuild` a subscriber is given carries the sequence of the
    /// last event published before it, so everything after it is newer.
    pub sequence: u64,
    /// The workspace it is about. For `rebuild`, the workspace open at the
    /// time, if any: a plugin drops what it holds for any other.
    pub workspace: Option<Uuid>,
    pub change: ReplicaChange,
}

struct OpenWorkspace {
    workspace: Uuid,
    epoch: u64,
    locked: bool,
}

struct State {
    /// The sequence the next published event takes.
    next: u64,
    buffer: VecDeque<FeedEvent>,
    capacity: usize,
    open: Option<OpenWorkspace>,
    /// Counts the workspaces opened, so a closed one's tap is ignored.
    epoch: u64,
}

impl State {
    fn push(&mut self, workspace: Option<Uuid>, change: ReplicaChange) {
        let event = FeedEvent {
            sequence: self.next,
            workspace,
            change,
        };
        self.next += 1;
        if self.buffer.len() == self.capacity {
            self.buffer.pop_front();
        }
        self.buffer.push_back(event);
    }

    fn workspace(&self) -> Option<Uuid> {
        self.open.as_ref().map(|open| open.workspace)
    }
}

struct Shared {
    state: Mutex<State>,
    notify: Notify,
}

impl Shared {
    fn with_state<T>(&self, change: impl FnOnce(&mut State) -> T) -> Option<T> {
        // A poisoned lock means a panic while publishing; the feed stops
        // rather than handing out a half-written buffer.
        let result = self.state.lock().ok().map(|mut state| change(&mut state));
        self.notify.notify_waiters();
        result
    }
}

/// The feed. The controller owns it for the app's lifetime; plugins reach
/// it only through [`crate::capabilities::PluginGrant`].
#[derive(Clone)]
pub struct ReplicaFeed(Arc<Shared>);

impl Default for ReplicaFeed {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }
}

impl std::fmt::Debug for ReplicaFeed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ReplicaFeed")
    }
}

impl ReplicaFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// A feed that keeps the latest `capacity` events (at least one).
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self(Arc::new(Shared {
            state: Mutex::new(State {
                next: 1,
                buffer: VecDeque::with_capacity(capacity.min(DEFAULT_CAPACITY)),
                capacity,
                open: None,
                epoch: 0,
            }),
            notify: Notify::new(),
        }))
    }

    /// A subscription that starts with `rebuild`. Only the shell's grant
    /// hands one out.
    pub(crate) fn subscribe(&self) -> ChangeSubscription {
        let cursor = self
            .0
            .state
            .lock()
            .map(|state| state.next)
            .unwrap_or(u64::MAX);
        ChangeSubscription {
            feed: self.0.clone(),
            cursor,
            rebuild: Some(RebuildReason::Subscribed),
        }
    }

    /// A tap for `workspace` that reports nothing until [`Self::activate`]
    /// opens it, so the controller can build the workspace's engine over it
    /// before the workspace is known to open.
    pub fn tap(&self, workspace: Uuid) -> FeedTap {
        let epoch = self
            .0
            .with_state(|state| {
                state.epoch += 1;
                state.epoch
            })
            .unwrap_or(0);
        FeedTap(Arc::new(TapInner {
            feed: self.0.clone(),
            workspace,
            epoch,
            local: tokio::sync::Mutex::new(None),
        }))
    }

    /// Opens the workspace of `tap`, replacing any open one. Subscribers
    /// get `rebuild` (`opened`).
    pub fn activate(&self, tap: &FeedTap) {
        self.0.with_state(|state| {
            state.open = Some(OpenWorkspace {
                workspace: tap.0.workspace,
                epoch: tap.0.epoch,
                locked: false,
            });
            state.push(
                Some(tap.0.workspace),
                ReplicaChange::Rebuild {
                    reason: RebuildReason::Opened,
                },
            );
        });
    }

    /// [`Self::tap`] and [`Self::activate`] in one.
    pub fn open(&self, workspace: Uuid) -> FeedTap {
        let tap = self.tap(workspace);
        self.activate(&tap);
        tap
    }

    /// Publishes `workspace_cleared` for the workspace `tap` opened, as its
    /// last event, if it is still the open one. Later reports through the
    /// tap are dropped.
    pub fn clear(&self, tap: &FeedTap, reason: ClearReason) {
        self.0.with_state(|state| {
            if state
                .open
                .as_ref()
                .is_some_and(|open| open.epoch == tap.0.epoch)
            {
                state.open = None;
                state.push(
                    Some(tap.0.workspace),
                    ReplicaChange::WorkspaceCleared { reason },
                );
            }
        });
    }

    /// The workspace open on the feed.
    pub fn workspace(&self) -> Option<Uuid> {
        self.0.state.lock().ok().and_then(|state| state.workspace())
    }
}

/// Where subscribers read the feed, from where they subscribed.
pub struct ChangeSubscription {
    feed: Arc<Shared>,
    /// The sequence of the next event to hand out.
    cursor: u64,
    /// A `rebuild` to hand out first.
    rebuild: Option<RebuildReason>,
}

impl std::fmt::Debug for ChangeSubscription {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ChangeSubscription")
            .field("cursor", &self.cursor)
            .finish_non_exhaustive()
    }
}

impl ChangeSubscription {
    /// The next event, if one is waiting.
    pub fn try_next(&mut self) -> Option<FeedEvent> {
        let state = self.feed.state.lock().ok()?;
        if let Some(reason) = self.rebuild.take() {
            return Some(rebuild(&state, self.cursor, reason));
        }
        let oldest = state
            .buffer
            .front()
            .map_or(state.next, |event| event.sequence);
        if self.cursor < oldest {
            // Evicted before this subscriber read it: its place is lost.
            self.cursor = state.next;
            return Some(rebuild(&state, self.cursor, RebuildReason::FellBehind));
        }
        let index = usize::try_from(self.cursor - oldest).ok()?;
        let event = state.buffer.get(index)?.clone();
        self.cursor = event.sequence + 1;
        Some(event)
    }

    /// The next event, waiting for one.
    pub async fn next(&mut self) -> FeedEvent {
        let feed = self.feed.clone();
        loop {
            let notified = feed.notify.notified();
            tokio::pin!(notified);
            // Registered before checking, so an event published in between
            // still wakes this subscriber.
            notified.as_mut().enable();
            if let Some(event) = self.try_next() {
                return event;
            }
            notified.await;
        }
    }

    /// The next event, waiting at most `timeout`.
    pub async fn next_within(&mut self, timeout: Duration) -> Option<FeedEvent> {
        tokio::time::timeout(timeout, self.next()).await.ok()
    }
}

fn rebuild(state: &State, cursor: u64, reason: RebuildReason) -> FeedEvent {
    FeedEvent {
        sequence: cursor.saturating_sub(1),
        workspace: state.workspace(),
        change: ReplicaChange::Rebuild { reason },
    }
}

struct TapInner {
    feed: Arc<Shared>,
    workspace: Uuid,
    epoch: u64,
    /// The last change-log position published; `None` until
    /// [`FeedTap::start`].
    local: tokio::sync::Mutex<Option<i64>>,
}

/// How one open workspace reports to the feed: the replica engine's
/// observer, and the reader of its change log. Reports after the workspace
/// closed are dropped.
#[derive(Clone)]
pub struct FeedTap(Arc<TapInner>);

impl std::fmt::Debug for FeedTap {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FeedTap")
            .field("workspace", &self.0.workspace)
            .finish_non_exhaustive()
    }
}

impl FeedTap {
    pub fn workspace(&self) -> Uuid {
        self.0.workspace
    }

    fn publish(&self, changes: impl IntoIterator<Item = ReplicaChange>) {
        let changes: Vec<ReplicaChange> = changes.into_iter().collect();
        if changes.is_empty() {
            return;
        }
        self.0.feed.with_state(|state| {
            if !self.is_open(state) {
                return;
            }
            for change in changes {
                state.push(Some(self.0.workspace), change);
            }
        });
    }

    fn is_open(&self, state: &State) -> bool {
        state
            .open
            .as_ref()
            .is_some_and(|open| open.epoch == self.0.epoch)
    }

    /// Starts reading the change log from where it stands, and takes the
    /// lease's state, once the workspace's engine exists. What came before
    /// is covered by the `rebuild` the workspace opened with.
    pub async fn start(&self, engine: &ObjectService) -> Result<(), ObjectError> {
        let head = engine.replica_local_changes(i64::MAX - 1, 1).await?.head;
        *self.0.local.lock().await = Some(head);
        self.check_lease(engine).await.map(|_| ())
    }

    /// Publishes the member's changes the device's change log gained since
    /// the last call. The bridge calls it after each write, and the
    /// controller after each sync step, whose settled conflicts and
    /// held-back edits are the member's changes too. Calls run one at a
    /// time, so each entry is published once and in log order.
    pub async fn local_writes(&self, engine: &ObjectService) -> Result<(), ObjectError> {
        let mut local = self.0.local.lock().await;
        let Some(mut after) = *local else {
            drop(local);
            return self.start(engine).await;
        };
        loop {
            let page = engine.replica_local_changes(after, LOCAL_PAGE).await?;
            if page.expired_through > after {
                // Entries expired before they were read.
                self.publish([ReplicaChange::Rebuild {
                    reason: RebuildReason::FellBehind,
                }]);
                *local = Some(page.head);
                return Ok(());
            }
            self.publish(page.changes.into_iter().map(|change| match change.kind {
                ChangeKind::Deleted => ReplicaChange::Removed {
                    object: change.object,
                    record: change.record,
                    reason: RemovalReason::Deleted,
                },
                ChangeKind::Created | ChangeKind::Updated | ChangeKind::Undeleted => {
                    ReplicaChange::Upserted {
                        object: change.object,
                        record: change.record,
                    }
                }
            }));
            let progressed = page.through > after;
            after = page.through;
            *local = Some(after);
            if !progressed || after >= page.head {
                return Ok(());
            }
        }
    }

    /// Reads the lease and publishes a change of state. The controller
    /// calls it when the lease may have run out, since time, not a commit,
    /// locks a replica.
    pub async fn check_lease(&self, engine: &ObjectService) -> Result<Lease, ObjectError> {
        let lease = engine.lease().await?;
        self.lease_state(lease.locked);
        Ok(lease)
    }

    fn lease_state(&self, locked: bool) {
        self.0.feed.with_state(|state| {
            let epoch = self.0.epoch;
            let Some(open) = state.open.as_mut().filter(|open| open.epoch == epoch) else {
                return;
            };
            if open.locked == locked {
                return;
            }
            open.locked = locked;
            state.push(
                Some(self.0.workspace),
                if locked {
                    ReplicaChange::WorkspaceLocked
                } else {
                    ReplicaChange::WorkspaceUnlocked
                },
            );
        });
    }
}

impl ReplicaObserver for FeedTap {
    fn committed(&self, event: ReplicaEvent) {
        match event {
            ReplicaEvent::RecordStored {
                object,
                record,
                deleted,
            } => self.publish([if deleted {
                ReplicaChange::Removed {
                    object,
                    record,
                    reason: RemovalReason::Deleted,
                }
            } else {
                ReplicaChange::Upserted { object, record }
            }]),
            ReplicaEvent::RecordsForgotten { object, records } => {
                self.publish(records.into_iter().map(|record| ReplicaChange::Removed {
                    object: object.clone(),
                    record,
                    reason: RemovalReason::AccessLost,
                }))
            }
            ReplicaEvent::ObjectsRetired { objects } => self.publish(objects.into_iter().map(
                |object| ReplicaChange::ObjectRemoved {
                    reason: if object.owner_plugin.is_some() {
                        ObjectRemovalReason::PluginSuspended
                    } else {
                        ObjectRemovalReason::LeftReplica
                    },
                    object: object.api_name,
                },
            )),
            ReplicaEvent::MetadataVersion(metadata_version) => {
                self.publish([ReplicaChange::DefinitionsChanged { metadata_version }])
            }
            ReplicaEvent::Lease(lease) => self.lease_state(lease.locked),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upserted(record: &str) -> ReplicaChange {
        ReplicaChange::Upserted {
            object: "custom:account".into(),
            record: record.into(),
        }
    }

    fn stored(record: &str) -> ReplicaEvent {
        ReplicaEvent::RecordStored {
            object: "custom:account".into(),
            record: record.into(),
            deleted: false,
        }
    }

    fn drain(subscription: &mut ChangeSubscription) -> Vec<ReplicaChange> {
        std::iter::from_fn(|| subscription.try_next())
            .map(|event| event.change)
            .collect()
    }

    #[test]
    fn subscribers_see_every_event_in_publication_order() {
        let feed = ReplicaFeed::new();
        let workspace = Uuid::now_v7();
        let mut early = feed.subscribe();
        let tap = feed.open(workspace);
        for record in ["a", "b", "c"] {
            tap.committed(stored(record));
        }
        let mut late = feed.subscribe();
        let events: Vec<FeedEvent> = std::iter::from_fn(|| early.try_next()).collect();
        assert_eq!(
            events.iter().map(|event| &event.change).collect::<Vec<_>>(),
            vec![
                &ReplicaChange::Rebuild {
                    reason: RebuildReason::Subscribed
                },
                &ReplicaChange::Rebuild {
                    reason: RebuildReason::Opened
                },
                &upserted("a"),
                &upserted("b"),
                &upserted("c"),
            ]
        );
        // Sequences count up with no gaps, and each names its workspace.
        assert_eq!(
            events[1..]
                .iter()
                .map(|event| event.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(events[1..]
            .iter()
            .all(|event| event.workspace == Some(workspace)));
        // A late subscriber rebuilds, then sees only what follows.
        let first = late.try_next().expect("rebuild");
        assert_eq!(
            first.change,
            ReplicaChange::Rebuild {
                reason: RebuildReason::Subscribed
            }
        );
        assert_eq!(first.sequence, 4);
        assert_eq!(first.workspace, Some(workspace));
        assert!(late.try_next().is_none());
        tap.committed(stored("d"));
        assert_eq!(drain(&mut late), vec![upserted("d")]);
        assert_eq!(drain(&mut early), vec![upserted("d")]);
    }

    #[test]
    fn a_subscriber_that_falls_behind_the_buffer_rebuilds() {
        let feed = ReplicaFeed::with_capacity(3);
        let tap = feed.open(Uuid::now_v7());
        let mut slow = feed.subscribe();
        let mut keeping_up = feed.subscribe();
        assert!(matches!(
            slow.try_next().map(|event| event.change),
            Some(ReplicaChange::Rebuild {
                reason: RebuildReason::Subscribed
            })
        ));
        drain(&mut keeping_up);
        for (index, record) in ["a", "b", "c", "d", "e"].into_iter().enumerate() {
            tap.committed(stored(record));
            // Reading as it goes, this subscriber never loses its place.
            assert_eq!(drain(&mut keeping_up), vec![upserted(record)], "{index}");
        }
        // Five events into a buffer of three: "a" and "b" are gone.
        let rebuilt = slow.try_next().expect("rebuild");
        assert_eq!(
            rebuilt.change,
            ReplicaChange::Rebuild {
                reason: RebuildReason::FellBehind
            }
        );
        // It continues with what is published after it.
        assert!(slow.try_next().is_none());
        tap.committed(stored("f"));
        let next = slow.try_next().expect("next");
        assert_eq!(next.change, upserted("f"));
        assert!(next.sequence > rebuilt.sequence);
    }

    #[test]
    fn workspace_cleared_is_the_last_event_for_the_workspace() {
        let feed = ReplicaFeed::new();
        let workspace = Uuid::now_v7();
        let tap = feed.open(workspace);
        let mut subscription = feed.subscribe();
        drain(&mut subscription);
        feed.clear(&tap, ClearReason::Wiped);
        // A sync step still finishing reports through the old tap.
        tap.committed(stored("late"));
        tap.lease_state(true);
        feed.clear(&tap, ClearReason::SignedOut);
        assert_eq!(
            drain(&mut subscription),
            vec![ReplicaChange::WorkspaceCleared {
                reason: ClearReason::Wiped
            }]
        );
        assert_eq!(feed.workspace(), None);
        // Signing in again opens the workspace afresh.
        let again = feed.open(workspace);
        again.committed(stored("new"));
        assert_eq!(
            drain(&mut subscription),
            vec![
                ReplicaChange::Rebuild {
                    reason: RebuildReason::Opened
                },
                upserted("new")
            ]
        );
    }

    #[test]
    fn the_lease_reports_only_changes_of_state() {
        let feed = ReplicaFeed::new();
        let tap = feed.open(Uuid::now_v7());
        let mut subscription = feed.subscribe();
        drain(&mut subscription);
        tap.lease_state(false);
        tap.lease_state(true);
        tap.lease_state(true);
        tap.lease_state(false);
        assert_eq!(
            drain(&mut subscription),
            vec![
                ReplicaChange::WorkspaceLocked,
                ReplicaChange::WorkspaceUnlocked
            ]
        );
    }

    #[tokio::test]
    async fn a_waiting_subscriber_wakes_for_the_next_event() {
        let feed = ReplicaFeed::new();
        let tap = feed.open(Uuid::now_v7());
        let mut subscription = feed.subscribe();
        drain(&mut subscription);
        let waiting = tokio::spawn(async move { subscription.next().await });
        tokio::task::yield_now().await;
        tap.committed(stored("a"));
        let event = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("woken")
            .expect("task");
        assert_eq!(event.change, upserted("a"));
    }

    #[test]
    fn events_serialise_as_ids_only() {
        let event = FeedEvent {
            sequence: 7,
            workspace: None,
            change: ReplicaChange::Removed {
                object: "custom:account".into(),
                record: "a01-0000000001".into(),
                reason: RemovalReason::AccessLost,
            },
        };
        assert_eq!(
            serde_json::to_value(&event).expect("json"),
            serde_json::json!({
                "sequence": 7,
                "workspace": null,
                "change": {
                    "kind": "removed",
                    "object": "custom:account",
                    "record": "a01-0000000001",
                    "reason": "access_lost",
                },
            })
        );
    }
}
