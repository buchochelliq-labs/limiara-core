//! A synced device's push queue (ADR 0024 §3). The device's change log
//! holds only the member's own edits, since records the server sends leave
//! no entry, so the records it names that moved past the server version
//! they last took are what the device sends, as composite requests.

use crate::composite::{CompositeOperation, CompositeRequest, COMPOSITE_MAX};
use crate::replica::{Applied, RemoteRecord};
use crate::service::{MutationContext, ObjectService};
use crate::{
    FieldType, ObjectDefinition, ObjectError, ObjectTx, Record, RecordChange, RemoteVersion,
    SyncPositions,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Change-log entries one call reads at most, so a long queue of records
/// that already went is passed over in steps.
const SCAN_MAX: usize = 1_000;
const PAGE: u16 = 200;
/// Stands for every field when a record's changes run past what one call
/// reads, so the fields named later are not missed. An update then sends
/// every field, which is safe because it is checked against the server
/// version the replica took.
const EVERY_FIELD: &str = "*";

/// The next composite request a device sends, and what it needs to take
/// the answers.
#[derive(Debug, Clone, PartialEq)]
pub struct PushBatch {
    pub operations: Vec<PushOperation>,
}

impl PushBatch {
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// The request to send to `POST /composite`. Operations run apart, so
    /// one that fails leaves the others in place.
    pub fn request(&self) -> CompositeRequest {
        CompositeRequest {
            all_or_none: false,
            operations: self
                .operations
                .iter()
                .map(|operation| operation.operation.clone())
                .collect(),
        }
    }
}

/// One record's change as the device pushes it.
#[derive(Debug, Clone, PartialEq)]
pub struct PushOperation {
    pub operation: CompositeOperation,
    pub public_id: String,
    /// The local version pushed, so an edit made while the request was out
    /// is kept when its answer arrives.
    pub local_version: i64,
    /// Values held back because their lookup targets reach the server only
    /// later. They are edited back in after the answer and push next time.
    pub held_back: Map<String, Value>,
    /// An owner change made with other changes, which a composite request
    /// carries as its own operation: it is made again after the answer and
    /// pushes next time.
    pub held_owner: Option<Uuid>,
}

/// The server's answer to one operation, by its reference.
#[derive(Debug, Clone, PartialEq)]
pub enum PushAnswer {
    /// The record as the server now has it.
    Done(RemoteRecord),
    /// The operation failed, with the error's `code`. The record keeps its
    /// edits and goes again with the next push unless the member changes
    /// or discards it.
    Failed { status: u16, code: String },
}

/// What taking a batch's answers did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PushReport {
    /// Records the server's answer replaced.
    pub stored: Vec<String>,
    /// Records the member edited again while the request was out; their
    /// new edits go next time.
    pub edited_since: Vec<String>,
    /// Operations that failed, by object API name and public ID, with the
    /// error code. `sync.held_back_failed` means the record went but a
    /// held-back lookup could not be set again on the device.
    pub failed: Vec<(String, String, String)>,
    /// Operations the server did not answer; they go again next time.
    pub unanswered: Vec<String>,
}

/// What a replica record still has to send.
enum Pending {
    Create(Map<String, Value>),
    Update(i64, Map<String, Value>),
    Delete(i64),
    Undelete(i64),
    /// Only the owner changed.
    Owner(i64, Uuid),
}

impl ObjectService {
    /// The next records to push, up to [`COMPOSITE_MAX`], oldest change
    /// first. Only a replica's engine may (`with_replica`).
    ///
    /// A record created on the device pushes as a create with its id
    /// (ADR 0031); one the server has pushes against the version the
    /// replica last took, with the fields the member changed. A lookup to a
    /// record that reaches the server only later in the queue is held back
    /// and pushes in the next batch, so records made offline that point at
    /// each other all arrive.
    pub async fn push_queue(&self) -> Result<PushBatch, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let mut positions = tx.sync_positions().await?;
        positions.pushed = sent_through(tx.as_mut(), positions.pushed).await?;
        tx.set_sync_positions(positions).await?;
        let candidates = queued(tx.as_mut(), positions.pushed).await?;
        let mut operations = Vec::new();
        // Records this batch creates on the server, before the one at hand.
        let mut arriving = BTreeSet::new();
        for (record_id, fields) in candidates {
            if operations.len() == COMPOSITE_MAX {
                break;
            }
            let Some((object, record, pending)) =
                pending_for(tx.as_mut(), record_id, &fields).await?
            else {
                continue;
            };
            let reference = format!("r{}", operations.len());
            let object_name = object.api_name.clone();
            let id = record.public_id.clone();
            let (operation, held_back) = match pending {
                Pending::Create(values) => {
                    let (mut values, held_back) =
                        hold_back(tx.as_mut(), &object, values, &arriving).await?;
                    values.insert("id".into(), Value::String(id.clone()));
                    arriving.insert(id.clone());
                    (
                        CompositeOperation::Create {
                            reference,
                            object: object_name,
                            values,
                        },
                        held_back,
                    )
                }
                Pending::Update(expected_version, values) => {
                    let (values, held_back) =
                        hold_back(tx.as_mut(), &object, values, &arriving).await?;
                    if values.is_empty() {
                        continue;
                    }
                    (
                        CompositeOperation::Update {
                            reference,
                            object: object_name,
                            id: id.clone(),
                            expected_version,
                            values,
                        },
                        held_back,
                    )
                }
                Pending::Delete(expected_version) => (
                    CompositeOperation::Delete {
                        reference,
                        object: object_name,
                        id: id.clone(),
                        expected_version,
                    },
                    Map::new(),
                ),
                Pending::Undelete(expected_version) => (
                    CompositeOperation::Undelete {
                        reference,
                        object: object_name,
                        id: id.clone(),
                        expected_version,
                    },
                    // The restore goes first; edits made with it push next.
                    record_values(&object, &record, Some(&fields)),
                ),
                Pending::Owner(expected_version, owner_id) => (
                    CompositeOperation::ChangeOwner {
                        reference,
                        object: object_name,
                        id: id.clone(),
                        expected_version,
                        owner_id,
                    },
                    Map::new(),
                ),
            };
            let held_owner = match &operation {
                CompositeOperation::Create { .. }
                | CompositeOperation::Update { .. }
                | CompositeOperation::Undelete { .. }
                    if owner_changed(&fields) =>
                {
                    record.owner_id
                }
                _ => None,
            };
            operations.push(PushOperation {
                operation,
                public_id: id,
                local_version: record.version,
                held_back,
                held_owner,
            });
        }
        tx.commit().await?;
        Ok(PushBatch { operations })
    }

    /// Takes the server's answers to a batch from [`push_queue`]: each
    /// record the server changed is stored as it has it, unless the member
    /// edited it again meanwhile, and held-back values are edited back in.
    /// Records with nothing left to send leave the queue.
    ///
    /// [`push_queue`]: ObjectService::push_queue
    pub async fn take_push_answers(
        &self,
        batch: &PushBatch,
        answers: &BTreeMap<String, PushAnswer>,
        context: MutationContext,
    ) -> Result<PushReport, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut report = PushReport::default();
        for pushed in &batch.operations {
            let object = pushed.operation.object();
            let Some(answer) = answers.get(pushed.operation.reference()) else {
                report.unanswered.push(pushed.public_id.clone());
                continue;
            };
            match answer {
                PushAnswer::Done(remote) => {
                    match self
                        .apply_remote(object, remote, Some(pushed.local_version))
                        .await?
                    {
                        Applied::Conflict(_) => report.edited_since.push(pushed.public_id.clone()),
                        Applied::Inserted(record)
                        | Applied::Updated(record)
                        | Applied::Unchanged(record) => {
                            report.stored.push(pushed.public_id.clone());
                            // The server's answer stays stored either way; a
                            // change that no longer fits is the member's to
                            // make again.
                            if self
                                .redo_held_back(object, &record, pushed, context)
                                .await
                                .is_err()
                            {
                                report.failed.push((
                                    object.to_owned(),
                                    pushed.public_id.clone(),
                                    "sync.held_back_failed".to_owned(),
                                ));
                            }
                        }
                    }
                }
                PushAnswer::Failed { code, .. } => {
                    report
                        .failed
                        .push((object.to_owned(), pushed.public_id.clone(), code.clone()))
                }
            }
        }
        let mut tx = self.begin().await?;
        let mut positions = tx.sync_positions().await?;
        positions.pushed = sent_through(tx.as_mut(), positions.pushed).await?;
        tx.set_sync_positions(positions).await?;
        tx.commit().await?;
        Ok(report)
    }

    /// Makes the changes a pushed operation held back again on the device,
    /// so they push next time.
    async fn redo_held_back(
        &self,
        object: &str,
        record: &Record,
        pushed: &PushOperation,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut version = record.version;
        if !pushed.held_back.is_empty() {
            version = self
                .update_record(
                    object,
                    &record.public_id,
                    version,
                    &pushed.held_back,
                    context,
                )
                .await?
                .version;
        }
        if let Some(owner) = pushed
            .held_owner
            .filter(|owner| record.owner_id != Some(*owner))
        {
            self.change_owner(object, &record.public_id, owner, version, context)
                .await?;
        }
        Ok(())
    }

    /// Where the replica's push and pull stand.
    pub async fn sync_positions(&self) -> Result<SyncPositions, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let positions = tx.sync_positions().await?;
        tx.commit().await?;
        Ok(positions)
    }

    /// Remembers the server change-log position the replica has pulled to
    /// and the metadata version it applied, after the page's records are
    /// stored, so a device that stops resumes from there. A position never
    /// moves back.
    pub async fn record_pull(
        &self,
        position: i64,
        metadata_version: i64,
    ) -> Result<SyncPositions, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        if position < 0 || metadata_version < 0 {
            return Err(ObjectError::invalid(
                "position",
                "sync.position_invalid",
                "positions are zero or more",
            ));
        }
        let mut tx = self.begin().await?;
        let mut positions = tx.sync_positions().await?;
        positions.pulled = Some(positions.pulled.map_or(position, |at| at.max(position)));
        positions.metadata_version = Some(
            positions
                .metadata_version
                .map_or(metadata_version, |at| at.max(metadata_version)),
        );
        tx.set_sync_positions(positions).await?;
        tx.commit().await?;
        Ok(positions)
    }
}

/// The furthest position from `after` up to which no entry's record has
/// anything left to send.
async fn sent_through(tx: &mut dyn ObjectTx, after: i64) -> Result<i64, ObjectError> {
    let (entries, _) = scan(tx, after).await?;
    let unsent = since_taken(tx, &entries).await?;
    let unsent_at: BTreeSet<i64> = unsent.iter().map(|change| change.position).collect();
    let mut fields: BTreeMap<Uuid, BTreeSet<String>> = BTreeMap::new();
    for change in &unsent {
        fields
            .entry(change.record_id)
            .or_default()
            .extend(change.fields.iter().cloned());
    }
    let mut decided: BTreeMap<Uuid, bool> = BTreeMap::new();
    let mut through = after;
    for change in &entries {
        // An entry the record's server copy already covers has nothing to
        // send.
        if !unsent_at.contains(&change.position) {
            through = change.position;
            continue;
        }
        let waiting = match decided.get(&change.record_id) {
            Some(waiting) => *waiting,
            None => {
                let named = fields.get(&change.record_id).cloned().unwrap_or_default();
                let waiting = pending_for(tx, change.record_id, &named).await?.is_some();
                decided.insert(change.record_id, waiting);
                waiting
            }
        };
        if waiting {
            break;
        }
        through = change.position;
    }
    Ok(through)
}

/// Up to [`SCAN_MAX`] change-log entries after `after`, oldest first, and
/// whether they reach the end of the log.
async fn scan(tx: &mut dyn ObjectTx, after: i64) -> Result<(Vec<RecordChange>, bool), ObjectError> {
    let mut entries = Vec::new();
    let mut at = after;
    while entries.len() < SCAN_MAX {
        let page = tx.changes(at, PAGE).await?;
        let full = page.len() == usize::from(PAGE);
        let Some(last) = page.last() else {
            return Ok((entries, true));
        };
        at = last.position;
        entries.extend(page);
        if !full {
            return Ok((entries, true));
        }
    }
    // A record seen here may have more changes after the last one read.
    let ended = tx.changes(at, 1).await?.is_empty();
    Ok((entries, ended))
}

/// The entries a record's server copy does not cover yet: those after the
/// position it took that copy at.
async fn since_taken(
    tx: &mut dyn ObjectTx,
    entries: &[RecordChange],
) -> Result<Vec<RecordChange>, ObjectError> {
    let mut through: BTreeMap<Uuid, i64> = BTreeMap::new();
    let mut unsent = Vec::new();
    for change in entries {
        let taken = match through.get(&change.record_id) {
            Some(taken) => *taken,
            None => {
                let taken = tx
                    .remote_version(change.record_id)
                    .await?
                    .map_or(0, |taken| taken.through);
                through.insert(change.record_id, taken);
                taken
            }
        };
        if change.position > taken {
            unsent.push(change.clone());
        }
    }
    Ok(unsent)
}

/// The records changed after `after`, in order of their first change, with
/// every field name their changes name, or [`EVERY_FIELD`] when the log runs
/// on past what was read.
async fn queued(
    tx: &mut dyn ObjectTx,
    after: i64,
) -> Result<Vec<(Uuid, BTreeSet<String>)>, ObjectError> {
    let mut order: Vec<Uuid> = Vec::new();
    let mut fields: BTreeMap<Uuid, BTreeSet<String>> = BTreeMap::new();
    let (entries, whole) = scan(tx, after).await?;
    for change in since_taken(tx, &entries).await? {
        fields
            .entry(change.record_id)
            .or_insert_with(|| {
                order.push(change.record_id);
                BTreeSet::new()
            })
            .extend(change.fields);
    }
    Ok(order
        .into_iter()
        .map(|id| {
            let mut named = fields.remove(&id).unwrap_or_default();
            if !whole {
                named.insert(EVERY_FIELD.to_owned());
            }
            (id, named)
        })
        .collect())
}

/// What a record still has to send, if anything. `fields` names the
/// fields its queued changes touched.
async fn pending_for(
    tx: &mut dyn ObjectTx,
    record_id: Uuid,
    fields: &BTreeSet<String>,
) -> Result<Option<(ObjectDefinition, Record, Pending)>, ObjectError> {
    let Some(record) = tx.record_by_id(record_id).await? else {
        return Ok(None);
    };
    let Some(object) = tx
        .object_by_id(record.object_id)
        .await?
        .filter(ObjectDefinition::is_active)
    else {
        return Ok(None);
    };
    let deleted = record.deleted_at.is_some();
    let pending = match tx.remote_version(record.id).await? {
        // Made on the device and discarded before it went: nothing to send.
        None if deleted => None,
        None => Some(Pending::Create(record_values(&object, &record, None))),
        Some(RemoteVersion { version, .. }) if version == record.version => None,
        Some(RemoteVersion {
            version,
            deleted: false,
            ..
        }) if deleted => Some(Pending::Delete(version)),
        // Changes to a record in the recycle bin on both sides stay local.
        Some(RemoteVersion { deleted: true, .. }) if deleted => None,
        Some(RemoteVersion {
            version,
            deleted: true,
            ..
        }) => Some(Pending::Undelete(version)),
        Some(RemoteVersion { version, .. }) => {
            let values = record_values(&object, &record, Some(fields));
            match record.owner_id {
                _ if !values.is_empty() => Some(Pending::Update(version, values)),
                Some(owner) if owner_changed(fields) => Some(Pending::Owner(version, owner)),
                _ => None,
            }
        }
    };
    Ok(pending.map(|pending| (object, record, pending)))
}

/// Whether the queued changes may include an owner change.
fn owner_changed(fields: &BTreeSet<String>) -> bool {
    fields.contains("owner") || fields.contains(EVERY_FIELD)
}

/// The values of `fields` (or every field when `None`) a member may write:
/// the name and stored fields, never formula or roll-up values, which the
/// server computes. A cleared field sends `null`.
fn record_values(
    object: &ObjectDefinition,
    record: &Record,
    fields: Option<&BTreeSet<String>>,
) -> Map<String, Value> {
    let wanted = |name: &str| {
        fields.is_none_or(|fields| fields.contains(EVERY_FIELD) || fields.contains(name))
    };
    let mut values = Map::new();
    if wanted("name") {
        values.insert("name".into(), Value::String(record.name.clone()));
    }
    for field in object.fields.iter().filter(|field| {
        field.is_active()
            && !matches!(field.field_type, FieldType::Formula | FieldType::Rollup)
            && wanted(&field.api_name)
    }) {
        match record.values.get(&field.api_name) {
            Some(value) => {
                values.insert(field.api_name.clone(), value.clone());
            }
            // A new record leaves empty fields out; an edit clears them.
            None if fields.is_some() => {
                values.insert(field.api_name.clone(), Value::Null);
            }
            None => {}
        }
    }
    values
}

/// Splits off lookups to records made on the device that are not on the
/// server yet and are not created earlier in this batch.
async fn hold_back(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    mut values: Map<String, Value>,
    arriving: &BTreeSet<String>,
) -> Result<(Map<String, Value>, Map<String, Value>), ObjectError> {
    let mut held_back = Map::new();
    for field in object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.field_type == FieldType::Lookup)
    {
        let Some(target) = values.get(&field.api_name).and_then(Value::as_str) else {
            continue;
        };
        if arriving.contains(target) {
            continue;
        }
        let Some(found) = tx.record_by_public_id(target).await? else {
            continue;
        };
        if found.deleted_at.is_none() && tx.remote_version(found.id).await?.is_none() {
            if let Some(value) = values.remove(&field.api_name) {
                held_back.insert(field.api_name.clone(), value);
            }
        }
    }
    Ok((values, held_back))
}
