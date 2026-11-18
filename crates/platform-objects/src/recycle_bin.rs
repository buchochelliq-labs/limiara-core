//! The engine's recycle bin (ADR 0035 G2 and G3).
//!
//! **Purge (G2).** A record stays in the recycle bin for
//! [`RECYCLE_BIN_RETENTION`] (15 days, as Contacts and ADR 0014 §5), then a
//! per-workspace job removes it for good: the record with its pivots,
//! search terms, shares, history, change-log rows and kept cleared lookups.
//! The job runs as the platform ([`ObjectCaller::system`]) through the
//! store, so the workspace's row-level security still scopes every
//! statement. It works in bounded batches, each its own transaction, and
//! stops after a time budget; a later pass carries on, and a pass with
//! nothing due changes nothing. A record undeleted while the purge runs is
//! never purged: each record is claimed with a write that checks it is
//! still in the bin, and an undelete that commits first wins.
//!
//! What a purge leaves: audit entries and published events are evidence and
//! stay (migrations and purges never rewrite evidence). Each batch writes
//! one audit entry for the workspace, `records.recycle_bin.purged`, naming
//! the records it removed by object, and one
//! `platform.objects.record-purged.v1` event per record, so consumers keyed
//! on a record (read models, search, integrations) drop it as they would on
//! any other record event. Records elsewhere that still pointed at a purged
//! record lose that value, as a system edit with its own `updated` event,
//! history entry and change-log entry; details of a purged master are
//! purged with it, since they could never be restored.
//!
//! **Restoring cleared lookups (G3).** Deleting a record clears the lookups
//! that point to it, as before; each cleared value is now kept with the
//! referencing record's version. Undeleting the record puts each value back
//! where the referencing record is live and has not written that field
//! since, as a versioned system edit: `system` source in history, a
//! change-log entry and a `record-updated` event, as for any update.
//! Validation rules and automations do not run for it, as they do not for
//! the clear. The rest are skipped, counted and named in the audit entry.
//! What was kept goes once the record is undeleted or purged.

use crate::events::{Change, EvidenceContext, RECORD_PURGED_V1};
use crate::service::{MutationContext, ObjectService};
use crate::{
    FieldType, HistoryKind, ObjectCaller, ObjectDefinition, ObjectError, ObjectStore, ObjectTx,
    Record, ENGINE_CONTEXT,
};
use chrono::{DateTime, TimeDelta, Utc};
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// How long a deleted record stays in the recycle bin before the purge
/// removes it (ADR 0014 §5, ADR 0035 G2). An engine constant: there is no
/// per-workspace setting.
pub const RECYCLE_BIN_RETENTION: TimeDelta = TimeDelta::days(15);
/// The window in days, as the API and pages state it.
pub const RECYCLE_BIN_RETENTION_DAYS: i64 = 15;
/// Records purged per transaction.
pub const PURGE_BATCH: u32 = 100;
/// How long one workspace's purge pass may run before it hands over; the
/// next pass carries on.
pub const PURGE_BUDGET: Duration = Duration::from_secs(30);
/// The audit action of a purge batch.
pub const PURGED_ACTION: &str = "records.recycle_bin.purged";
/// The audit action naming the cleared lookups an undelete did not restore.
pub const LOOKUPS_SKIPPED_ACTION: &str = "records.lookups.restore_skipped";

/// A lookup value that deleting a record cleared on another record (G3).
#[derive(Debug, Clone, PartialEq)]
pub struct ClearedLookup {
    /// The record whose deletion cleared it.
    pub deleted_record_id: Uuid,
    /// The referencing record.
    pub record_id: Uuid,
    pub field_id: Uuid,
    /// The value cleared: the deleted record's public id.
    pub value: Value,
    /// The referencing record's version just after the clear.
    pub cleared_version: i64,
    /// Whether the field was written since.
    pub changed: bool,
    pub cleared_at: DateTime<Utc>,
}

/// Why an undelete did not restore a cleared lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// The referencing record wrote the field after it was cleared.
    Changed,
    /// The referencing record is in the recycle bin.
    Deleted,
    /// The field, or the referencing object, is no longer in service.
    FieldRemoved,
}

impl SkipReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Deleted => "deleted",
            Self::FieldRemoved => "field_removed",
        }
    }
}

/// What undeleting a record did with the lookups its deletion cleared.
/// Counts only: the member may not be able to read the records involved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LookupRestore {
    pub restored: usize,
    /// Skipped links by reason.
    pub skipped: BTreeMap<SkipReason, usize>,
}

impl LookupRestore {
    pub fn skipped_total(&self) -> usize {
        self.skipped.values().sum()
    }

    pub(crate) fn add(&mut self, other: LookupRestore) {
        self.restored += other.restored;
        for (reason, count) in other.skipped {
            *self.skipped.entry(reason).or_default() += count;
        }
    }
}

/// What one workspace's purge pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeOutcome {
    /// Records removed.
    pub purged: u64,
    /// Transactions that removed records.
    pub batches: u32,
    /// Whether the pass reached the end of what was due; false when it
    /// stopped at its time budget.
    pub complete: bool,
}

impl ObjectService {
    /// Keeps a lookup value that deleting `deleted` cleared on `child`,
    /// now at `child_version` (G3). A replica keeps none: the server
    /// restores the values and the replica takes them with the pull.
    pub(crate) async fn keep_cleared_lookup(
        &self,
        tx: &mut dyn ObjectTx,
        deleted: &Record,
        child: &Record,
        field_id: Uuid,
    ) -> Result<(), ObjectError> {
        if self.is_replica() {
            return Ok(());
        }
        tx.put_cleared_lookup(&ClearedLookup {
            deleted_record_id: deleted.id,
            record_id: child.id,
            field_id,
            value: Value::String(deleted.public_id.clone()),
            cleared_version: child.version,
            changed: false,
            cleared_at: child.updated_at,
        })
        .await
    }

    /// Marks kept cleared lookups of `record`'s changed reference fields
    /// as changed since (G3).
    pub(crate) async fn note_reference_writes(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        record: &Record,
        changed: &[String],
    ) -> Result<(), ObjectError> {
        if self.is_replica() {
            return Ok(());
        }
        let fields: Vec<Uuid> = object
            .fields
            .iter()
            .filter(|field| field.field_type.holds_reference() && changed.contains(&field.api_name))
            .map(|field| field.id)
            .collect();
        if fields.is_empty() {
            return Ok(());
        }
        tx.mark_cleared_lookups_changed(record.id, &fields).await
    }

    /// Puts back the lookup values deleting `restored` cleared, where the
    /// referencing record is live and has not written the field since, and
    /// forgets them all (G3). Each restore is a system edit of its own.
    pub(crate) async fn restore_cleared_lookups(
        &self,
        tx: &mut dyn ObjectTx,
        restored: &Record,
        context: MutationContext,
    ) -> Result<LookupRestore, ObjectError> {
        let mut report = LookupRestore::default();
        if self.is_replica() {
            return Ok(report);
        }
        let kept = tx.cleared_lookups(restored.id).await?;
        if kept.is_empty() {
            return Ok(report);
        }
        let system = context.with_source(crate::MutationSource::system());
        let mut objects: BTreeMap<Uuid, Option<ObjectDefinition>> = BTreeMap::new();
        let mut skipped = Vec::new();
        for lookup in kept {
            let Some(child) = tx.record_by_id(lookup.record_id).await? else {
                continue;
            };
            let owner = match objects.get(&child.object_id) {
                Some(owner) => owner.clone(),
                None => {
                    let owner = tx
                        .object_by_id(child.object_id)
                        .await?
                        .filter(ObjectDefinition::is_active);
                    objects.insert(child.object_id, owner.clone());
                    owner
                }
            };
            let field = owner.as_ref().and_then(|owner| {
                owner
                    .fields
                    .iter()
                    .find(|field| field.id == lookup.field_id && field.is_active())
                    .cloned()
            });
            let (Some(owner), Some(field)) = (owner, field) else {
                skipped.push((child.public_id.clone(), None, SkipReason::FieldRemoved));
                continue;
            };
            let reason = if child.deleted_at.is_some() {
                Some(SkipReason::Deleted)
            } else if lookup.changed || child.values.contains_key(&field.api_name) {
                Some(SkipReason::Changed)
            } else {
                None
            };
            if let Some(reason) = reason {
                skipped.push((child.public_id.clone(), Some(field.api_name), reason));
                continue;
            }
            self.write_reference(
                tx,
                &owner,
                child,
                &field.api_name,
                Some(lookup.value.clone()),
                system,
            )
            .await?;
            report.restored += 1;
        }
        tx.forget_cleared_lookups(restored.id).await?;
        for (_, _, reason) in &skipped {
            *report.skipped.entry(*reason).or_default() += 1;
        }
        if !skipped.is_empty() {
            let EvidenceContext {
                tenant_id,
                actor_id,
                correlation_id,
                causation_id,
                ..
            } = self.evidence(system);
            let links: Vec<Value> = skipped
                .iter()
                .map(|(record, field, reason)| {
                    json!({"record": record, "field": field, "reason": reason.as_str()})
                })
                .collect();
            tx.append_audit(&AuditEvent::with_context(
                Some(tenant_id),
                actor_id,
                correlation_id,
                causation_id,
                LOOKUPS_SKIPPED_ACTION,
                format!("record:{}", restored.public_id),
                json!({"restored": report.restored, "skipped": links, "source": crate::MutationSource::system()}),
            ))
            .await?;
        }
        Ok(report)
    }

    /// Writes one reference field of `child` as a system edit: the next
    /// version, fresh pivots, a change-log entry and event, and history.
    /// `None` clears it.
    pub(crate) async fn write_reference(
        &self,
        tx: &mut dyn ObjectTx,
        owner: &ObjectDefinition,
        mut child: Record,
        field_api: &str,
        value: Option<Value>,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        let before = child.clone();
        let version = child.version;
        match value {
            Some(value) => child.values.insert(field_api.to_owned(), value),
            None => child.values.remove(field_api),
        };
        self.stamp(&mut child);
        if !tx.update_record(&child, version).await? {
            return Err(ObjectError::VersionConflict);
        }
        let references = crate::service::current_references(tx, owner, &child).await?;
        tx.replace_index(
            child.id,
            &crate::service::stored_index(
                owner,
                &child,
                &references,
                &crate::formula_fields::stored_formulas(owner),
            ),
        )
        .await?;
        let changed = [field_api.to_owned()];
        self.record_change(tx, context, Change::RecordUpdated(owner, &child, &changed))
            .await?;
        self.track(
            tx,
            owner,
            Some(&before),
            &child,
            HistoryKind::Changed,
            context,
        )
        .await?;
        Ok(child)
    }
}

/// Purges one workspace's records that went to the recycle bin more than
/// [`RECYCLE_BIN_RETENTION`] before `now`, in batches of [`PURGE_BATCH`],
/// until nothing is due or `budget` is spent.
pub async fn purge_recycle_bin(
    store: Arc<dyn ObjectStore>,
    tenant_id: Uuid,
    now: DateTime<Utc>,
    budget: Duration,
) -> Result<PurgeOutcome, ObjectError> {
    let started = std::time::Instant::now();
    let cutoff = now - RECYCLE_BIN_RETENTION;
    let service = ObjectService::new(store, ObjectCaller::system(tenant_id));
    let run = Uuid::now_v7();
    let mut outcome = PurgeOutcome::default();
    loop {
        let (purged, more) = service.purge_batch(cutoff, run, outcome.batches).await?;
        if purged > 0 {
            outcome.purged += purged;
            outcome.batches += 1;
        }
        if !more {
            outcome.complete = true;
            return Ok(outcome);
        }
        if started.elapsed() >= budget {
            return Ok(outcome);
        }
    }
}

impl ObjectService {
    /// One purge transaction: up to [`PURGE_BATCH`] due records, with the
    /// details that go with them. Returns how many records went and
    /// whether more may be due.
    async fn purge_batch(
        &self,
        cutoff: DateTime<Utc>,
        run: Uuid,
        batch: u32,
    ) -> Result<(u64, bool), ObjectError> {
        let mut tx = self.begin().await?;
        let candidates = tx.purge_candidates(cutoff, PURGE_BATCH).await?;
        let more = candidates.len() >= PURGE_BATCH as usize;
        let context = MutationContext {
            causation_id: Some(run),
            ..MutationContext::new(run)
        }
        .with_source(crate::MutationSource::system());
        let mut objects: BTreeMap<Uuid, Option<ObjectDefinition>> = BTreeMap::new();
        let mut going: Vec<Record> = Vec::new();
        let mut claimed: BTreeSet<Uuid> = BTreeSet::new();
        let mut cleared = 0usize;
        let mut pending: Vec<(Uuid, Option<DateTime<Utc>>)> = candidates
            .into_iter()
            .map(|id| (id, Some(cutoff)))
            .collect();
        while let Some((id, before)) = pending.pop() {
            if claimed.contains(&id) || !tx.claim_purge(id, before).await? {
                continue;
            }
            let Some(record) = tx.record_by_id(id).await? else {
                continue;
            };
            claimed.insert(id);
            for (field_id, referencing_id) in tx.referencing_all(id).await? {
                if claimed.contains(&referencing_id) || referencing_id == id {
                    continue;
                }
                let Some(child) = tx.record_by_id(referencing_id).await? else {
                    continue;
                };
                let Some(owner) = object_of(tx.as_mut(), &mut objects, child.object_id).await?
                else {
                    continue;
                };
                let Some(field) = owner.fields.iter().find(|field| field.id == field_id) else {
                    continue;
                };
                if field.field_type == FieldType::MasterDetail {
                    // A detail cannot outlive its master: it goes too,
                    // whenever it was deleted.
                    if child.deleted_at.is_some() {
                        pending.push((child.id, None));
                    }
                    continue;
                }
                let api_name = field.api_name.clone();
                if child.values.contains_key(&api_name) {
                    self.write_reference(tx.as_mut(), &owner, child, &api_name, None, context)
                        .await?;
                    cleared += 1;
                }
            }
            going.push(record);
        }
        if going.is_empty() {
            tx.commit().await?;
            return Ok((0, more));
        }
        let ids: Vec<Uuid> = going.iter().map(|record| record.id).collect();
        let purged = tx.purge_records(&ids).await?;
        if purged != ids.len() as u64 {
            return Err(ObjectError::Invariant);
        }
        let purged_at = Utc::now();
        let mut by_object: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for record in &going {
            let api_name = match object_of(tx.as_mut(), &mut objects, record.object_id).await? {
                Some(object) => object.api_name,
                None => match tx.object_by_id(record.object_id).await? {
                    Some(object) => object.api_name,
                    None => return Err(ObjectError::Invariant),
                },
            };
            let event = EventEnvelope::with_context(
                RECORD_PURGED_V1,
                ENGINE_CONTEXT,
                self.caller().tenant_id(),
                None,
                run,
                Some(run),
                json!({
                    "object": api_name,
                    "entity_id": record.public_id,
                    "version": record.version,
                    "deleted_at": record.deleted_at,
                    "purged_at": purged_at,
                    "source": crate::SourceKind::System,
                }),
            );
            tx.append_event(&event).await?;
            by_object
                .entry(api_name)
                .or_default()
                .push(record.public_id.clone());
        }
        let mut audit = AuditEvent::with_context(
            Some(self.caller().tenant_id()),
            None,
            run,
            Some(run),
            PURGED_ACTION,
            "recycle_bin".to_owned(),
            json!({
                "purged": purged,
                "records": by_object,
                "cleared_references": cleared,
                "deleted_before": cutoff,
                "retention_days": RECYCLE_BIN_RETENTION_DAYS,
                "batch": batch + 1,
                "source": crate::MutationSource::system(),
            }),
        );
        audit.occurred_at = purged_at;
        tx.append_audit(&audit).await?;
        tx.note_purge().await?;
        tx.commit().await?;
        Ok((purged, more))
    }
}

/// An object by id, active or not, cached for the batch.
async fn object_of(
    tx: &mut dyn ObjectTx,
    cache: &mut BTreeMap<Uuid, Option<ObjectDefinition>>,
    id: Uuid,
) -> Result<Option<ObjectDefinition>, ObjectError> {
    if let Some(found) = cache.get(&id) {
        return Ok(found.clone());
    }
    let found = tx.object_by_id(id).await?;
    cache.insert(id, found.clone());
    Ok(found)
}
