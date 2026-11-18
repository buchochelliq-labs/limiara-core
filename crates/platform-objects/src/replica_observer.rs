//! What a replica's engine tells the device about the changes it commits
//! (ADR 0034 §9). A device builds its replica change feed from two sources:
//!
//! - the member's own changes, which every write already records in the
//!   device's change log ([`ObjectService::replica_local_changes`]);
//! - the changes the server makes to the replica, which leave no entry in
//!   that log: records a pull, a push answer or a settled conflict stores,
//!   records forgotten because the member can no longer read them, objects
//!   the definitions retire, the metadata version the replica follows, and
//!   the offline lease. A [`ReplicaObserver`] receives these after each
//!   commit.
//!
//! Both carry ids only, never field values.

use crate::service::ObjectService;
use crate::{ChangeKind, Lease, ObjectError};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

/// One change the server's side of sync committed to a replica, by API
/// name and public id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplicaEvent {
    /// A server copy of a record was stored: by a pull, as the answer to a
    /// push, or as the outcome of a settled conflict. `deleted` says
    /// whether the record is now in the recycle bin.
    RecordStored {
        object: String,
        record: String,
        deleted: bool,
    },
    /// Records the replica removed because the member can no longer read
    /// them on the server (the revocation set, ADR 0024 §4).
    RecordsForgotten {
        object: String,
        records: Vec<String>,
    },
    /// Objects that left the replica because the server no longer
    /// describes them to the member: deleted, no longer readable, or
    /// suspended with their plugin (ADR 0033 §2).
    ObjectsRetired { objects: Vec<RetiredObject> },
    /// The workspace metadata version the replica's records now follow
    /// moved.
    MetadataVersion(i64),
    /// The offline lease as it stands after a change to it.
    Lease(Lease),
}

/// An object the definitions retired.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RetiredObject {
    pub api_name: String,
    /// The plugin that declared it, if any.
    pub owner_plugin: Option<String>,
}

/// Receives a replica's committed changes. It is called after the commit,
/// on the task that made it, and must not block: a device queues the event
/// and returns.
pub trait ReplicaObserver: Send + Sync {
    fn committed(&self, event: ReplicaEvent);
}

/// One entry of the device's change log: a change the member made on the
/// device, by API name and public id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalChange {
    pub position: i64,
    pub object: String,
    pub record: String,
    pub kind: ChangeKind,
}

/// A page of the device's change log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalChanges {
    pub changes: Vec<LocalChange>,
    /// The last position read: the next page starts after it.
    pub through: i64,
    /// The newest position in the log.
    pub head: i64,
    /// The position up to which the log has expired: a reader whose
    /// position is below it missed changes.
    pub expired_through: i64,
}

impl ObjectService {
    /// Tells `observer` about every change the server's side of sync
    /// commits to this replica (ADR 0034 §9). Only a replica's engine calls
    /// it.
    pub fn with_replica_observer(mut self, observer: Arc<dyn ReplicaObserver>) -> Self {
        self.set_observer(observer);
        self
    }

    pub(crate) fn observe(&self, event: ReplicaEvent) {
        if let Some(observer) = self.observer() {
            observer.committed(event);
        }
    }

    /// Up to `limit` entries of the device's change log after `after`,
    /// oldest first, with the log's bounds: the member's own changes, by
    /// API name and public id. It reads ids only, so it needs no access to
    /// the records and works while the lease is locked. Only a replica's
    /// engine may.
    pub async fn replica_local_changes(
        &self,
        after: i64,
        limit: u16,
    ) -> Result<LocalChanges, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let (head, expired_through) = tx.change_bounds().await?;
        let entries = tx.changes(after, limit).await?;
        let mut names: BTreeMap<Uuid, Option<String>> = BTreeMap::new();
        let mut changes = Vec::with_capacity(entries.len());
        let mut through = after;
        for entry in entries {
            through = through.max(entry.position);
            let name = match names.get(&entry.object_id) {
                Some(name) => name.clone(),
                None => {
                    let name = tx
                        .object_by_id(entry.object_id)
                        .await?
                        .map(|object| object.api_name);
                    names.insert(entry.object_id, name.clone());
                    name
                }
            };
            // An object the replica no longer defines has no records to
            // name; its departure is reported with the definitions.
            let Some(object) = name else {
                continue;
            };
            changes.push(LocalChange {
                position: entry.position,
                object,
                record: entry.public_id,
                kind: entry.kind,
            });
        }
        tx.commit().await?;
        Ok(LocalChanges {
            changes,
            through,
            head,
            expired_through,
        })
    }
}
