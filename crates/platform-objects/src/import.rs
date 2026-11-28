//! Importing records with the identity they had elsewhere (ADR 0035 G4).
//!
//! The kernel moves a plugin's records into the engine without renumbering
//! or restamping them: each keeps its internal and public id, its stamps,
//! version, owner and archive and recycle-bin state, and its history. Only
//! the kernel's migrator caller ([`ObjectCaller::migrator`]) may import,
//! one workspace at a time, and only into objects that exist there,
//! suspended or not, since a plugin's objects stay out of service until
//! its workspace cuts over.
//!
//! An import checks each value as a member's write would, but runs no
//! validation rules and no automations, which judge new edits rather than
//! records that already exist. Each record goes into the change log, so
//! replicas and change-feed clients pick it up, but publishes no event of
//! its own: the batch leaves one audit entry naming every record. The
//! object's record sequence is raised past every imported id, so the next
//! record the engine numbers cannot take one.

use crate::service::{stored_index, MutationContext, ObjectService};
use crate::values::apply_values;
use crate::{
    ChangeKind, FieldType, HistoryEntry, HistoryKind, ObjectDefinition, ObjectError, ObjectTx,
    Record, RecordChange, ValidationIssue,
};
use chrono::{DateTime, Utc};
use platform_audit::AuditEvent;
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// The most records one import call writes.
pub const IMPORT_BATCH_MAX: usize = 500;
/// The audit action each import batch leaves.
pub const RECORDS_IMPORTED_ACTION: &str = "objects.records.imported";
/// A record with this internal or public id already exists.
pub const IMPORT_EXISTS: &str = "records.import_exists";
/// The object is a detail or summarises details: importing into it would
/// leave master roll-ups unmaintained, so it is not supported yet.
pub const IMPORT_UNSUPPORTED: &str = "records.import_unsupported";

/// One record as it was elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedRecord {
    pub id: Uuid,
    /// A server-range public id with the object's key prefix.
    pub public_id: String,
    /// The record's values, keyed by field API name, with the standard
    /// `name` unless the object composes it (ADR 0035 G17).
    pub values: Map<String, Value>,
    pub owner_id: Option<Uuid>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
    pub archived_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    /// Entries for the record's history, at the times they happened.
    pub history: Vec<ImportedHistory>,
}

/// One history entry as it was elsewhere; the engine marks it `system`.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedHistory {
    pub kind: HistoryKind,
    pub field: Option<String>,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
    pub changed_at: DateTime<Utc>,
    pub changed_by: Option<Uuid>,
}

/// What one import call wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: usize,
    /// The object's record sequence afterwards.
    pub record_sequence: u64,
}

impl ObjectService {
    /// Writes `records` into the object with their identity (ADR 0035 G4),
    /// all or none. `source_sequence` is the source's own counter for the
    /// object: the record sequence is raised to at least it and past every
    /// imported public id. Only the kernel's migrator may call it.
    pub async fn import_records(
        &self,
        api_name: &str,
        records: Vec<ImportedRecord>,
        source_sequence: u64,
        context: MutationContext,
    ) -> Result<ImportReport, ObjectError> {
        if !self.caller().is_migrator() {
            return Err(ObjectError::PermissionDenied);
        }
        if records.len() > IMPORT_BATCH_MAX {
            return Err(ObjectError::invalid(
                "records",
                "records.import_batch_too_large",
                format!("import at most {IMPORT_BATCH_MAX} records at a time"),
            ));
        }
        let mut tx = self.begin().await?;
        let object = import_target(tx.as_mut(), api_name).await?;
        let mut highest = source_sequence;
        let mut ids = Vec::with_capacity(records.len());
        for (position, imported) in records.into_iter().enumerate() {
            let sequence = self
                .import_one(tx.as_mut(), &object, position, imported, context, &mut ids)
                .await?;
            highest = highest.max(sequence);
        }
        let highest = tx.raise_record_sequence(object.id, highest).await?;
        let evidence = self.evidence(context);
        let mut audit = AuditEvent::with_context(
            Some(evidence.tenant_id),
            evidence.actor_id,
            evidence.correlation_id,
            evidence.causation_id,
            RECORDS_IMPORTED_ACTION,
            format!("object:{}", object.api_name),
            json!({
                "object": object.api_name,
                "imported": ids.len(),
                "records": ids,
                "record_sequence": highest,
                "source": crate::MutationSource::system(),
            }),
        );
        audit.occurred_at = Utc::now();
        tx.append_audit(&audit).await?;
        tx.commit().await?;
        Ok(ImportReport {
            imported: ids.len(),
            record_sequence: highest,
        })
    }

    /// The ids among `ids` that are already records, in any state, so a
    /// restarted import knows what an interrupted batch wrote. Only the
    /// kernel's migrator may ask.
    pub async fn imported_ids(&self, ids: &[Uuid]) -> Result<BTreeSet<Uuid>, ObjectError> {
        if !self.caller().is_migrator() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        let mut found = BTreeSet::new();
        for id in ids {
            if tx.record_by_id(*id).await?.is_some() {
                found.insert(*id);
            }
        }
        tx.commit().await?;
        Ok(found)
    }

    /// Starts checking an import without writing it (a migration's dry
    /// run): batches given to [`ImportCheck::check`] are imported in one
    /// transaction, so later batches may point at earlier ones, and that
    /// transaction is rolled back when the check is dropped. Only the
    /// kernel's migrator may check.
    pub async fn check_import(&self) -> Result<ImportCheck<'_>, ObjectError> {
        if !self.caller().is_migrator() {
            return Err(ObjectError::PermissionDenied);
        }
        Ok(ImportCheck {
            service: self,
            tx: self.begin().await?,
        })
    }

    /// Writes one record and returns its public id's sequence.
    async fn import_one(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        position: usize,
        imported: ImportedRecord,
        context: MutationContext,
        ids: &mut Vec<String>,
    ) -> Result<u64, ObjectError> {
        let at = |field: &str| format!("records/{position}/{field}");
        let imported_at = Utc::now();
        let invalid = |field: &str, code: &'static str, detail: &str| {
            ObjectError::invalid(at(field), code, detail.to_owned())
        };
        // The id is the object's, from the server's range: a device range
        // id would collide with ids devices assign (ADR 0031).
        let public = platform_id::PublicId::parse(&imported.public_id)
            .ok()
            .filter(|id| id.prefix() == object.key_prefix)
            .ok_or_else(|| {
                invalid(
                    "public_id",
                    "records.import_id_invalid",
                    "a public id of this object's key prefix",
                )
            })?;
        let sequence = public.sequence().ok_or_else(|| {
            invalid(
                "public_id",
                "records.import_id_invalid",
                "a server-assigned public id, not one a device assigned",
            )
        })?;
        if imported.id.is_nil() {
            return Err(invalid("id", "records.import_id_invalid", "a record id"));
        }
        if imported.version < 1 {
            return Err(invalid(
                "version",
                "records.import_version_invalid",
                "a version of 1 or more",
            ));
        }
        if imported.updated_at < imported.created_at {
            return Err(invalid(
                "updated_at",
                "records.import_stamps_invalid",
                "updated no earlier than created",
            ));
        }
        if tx.record_by_id(imported.id).await?.is_some()
            || tx.record_by_public_id(&imported.public_id).await?.is_some()
        {
            return Err(ObjectError::Conflict(IMPORT_EXISTS));
        }
        // Values are checked as a write checks them; rules and automations,
        // which judge new edits, do not run.
        let change =
            apply_values(object, &BTreeMap::new(), &imported.values, true).map_err(|issues| {
                ObjectError::Validation(
                    issues
                        .into_iter()
                        .map(|issue| {
                            ValidationIssue::new(at(&issue.field), issue.code, issue.detail)
                        })
                        .collect(),
                )
            })?;
        let record = Record {
            id: imported.id,
            object_id: object.id,
            public_id: imported.public_id,
            name: change.name.ok_or(ObjectError::Invariant)?,
            owner_id: imported.owner_id,
            record_type_id: None,
            values: change.values,
            version: imported.version,
            created_at: imported.created_at,
            created_by: imported.created_by,
            updated_at: imported.updated_at,
            updated_by: imported.updated_by,
            // The stamps are the source's; the touch is the import's, so
            // the change log and `touched_after` see the record as new
            // (ADR 0024 §3), and it expires no earlier changes with it.
            touched_at: imported_at,
            archived_at: imported.archived_at,
            deleted_at: imported.deleted_at,
        };
        let references = imported_references(tx, object, &record, position).await?;
        tx.insert_record(&record).await?;
        tx.replace_index(
            record.id,
            &stored_index(
                object,
                &record,
                &references,
                &crate::formula_fields::stored_formulas(object),
            ),
        )
        .await?;
        // The change log carries it to replicas and change feeds; a record
        // in the recycle bin arrives as deleted, so they leave it out.
        let mut fields: Vec<String> = record.values.keys().cloned().collect();
        fields.insert(0, "name".into());
        tx.append_change(&RecordChange {
            position: 0,
            record_id: record.id,
            object_id: object.id,
            public_id: record.public_id.clone(),
            kind: if record.deleted_at.is_some() {
                ChangeKind::Deleted
            } else {
                ChangeKind::Created
            },
            fields,
            changed_at: record.touched_at,
            changed_by: self.caller().actor(),
            correlation_id: Some(context.correlation_id),
        })
        .await?;
        let history: Vec<HistoryEntry> = imported
            .history
            .into_iter()
            .map(|entry| HistoryEntry {
                id: Uuid::now_v7(),
                record_id: record.id,
                object_id: object.id,
                kind: entry.kind,
                field: entry.field,
                old_value: entry.old_value,
                new_value: entry.new_value,
                changed_at: entry.changed_at,
                changed_by: entry.changed_by,
                correlation_id: Some(context.correlation_id),
                source: Some(crate::MutationSource::system()),
            })
            .collect();
        if !history.is_empty() {
            tx.append_history(&history).await?;
        }
        ids.push(record.public_id);
        Ok(sequence)
    }
}

/// The records an imported record's lookups and related-to fields point
/// at: an existing record of an object the field allows, whatever its
/// state, as the source had it. Targets are imported first.
/// The object an import writes to, locked for it: active, which takes in
/// suspended and dormant objects (that is when a plugin's records move in),
/// and without master-detail or roll-up fields.
async fn import_target(
    tx: &mut dyn ObjectTx,
    api_name: &str,
) -> Result<ObjectDefinition, ObjectError> {
    tx.lock_metadata().await?;
    let object = tx
        .object_by_api_name(api_name)
        .await?
        .filter(|object| object.status == crate::DefinitionStatus::Active)
        .ok_or(ObjectError::NotFound)?;
    let guarded = object.fields.iter().any(|field| {
        matches!(
            field.field_type,
            FieldType::MasterDetail | FieldType::Rollup
        )
    });
    if guarded {
        return Err(ObjectError::Conflict(IMPORT_UNSUPPORTED));
    }
    tx.lock_object(object.id, true).await?;
    Ok(object)
}

/// A record an import check refused, with why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRefusal {
    pub public_id: String,
    /// Each problem, with fields named `records/{position}/{field}` as an
    /// import names them, or one entry carrying the conflict's code.
    pub issues: Vec<ValidationIssue>,
}

/// An import checked without being written (see
/// [`ObjectService::check_import`]); dropping it rolls everything back.
pub struct ImportCheck<'a> {
    service: &'a ObjectService,
    tx: Box<dyn ObjectTx>,
}

impl ImportCheck<'_> {
    /// Imports `records` into the check's transaction as
    /// [`ObjectService::import_records`] would, and answers the records it
    /// would refuse. Unlike an import, a refused record does not stop the
    /// batch: the others are still checked and kept for later batches.
    /// A store failure ends the check.
    pub async fn check(
        &mut self,
        api_name: &str,
        records: Vec<ImportedRecord>,
    ) -> Result<Vec<ImportRefusal>, ObjectError> {
        let object = import_target(self.tx.as_mut(), api_name).await?;
        let mut refused = Vec::new();
        let mut ids = Vec::new();
        let context = MutationContext::new(Uuid::now_v7());
        for (position, imported) in records.into_iter().enumerate() {
            let public_id = imported.public_id.clone();
            let outcome = self
                .service
                .import_one(
                    self.tx.as_mut(),
                    &object,
                    position,
                    imported,
                    context,
                    &mut ids,
                )
                .await;
            let issues = match outcome {
                Ok(_) => continue,
                Err(ObjectError::Validation(issues)) | Err(ObjectError::Refused(_, issues)) => {
                    issues
                }
                Err(ObjectError::Conflict(code)) => vec![ValidationIssue::new(
                    format!("records/{position}"),
                    code,
                    "the record conflicts with one already there",
                )],
                Err(other) => return Err(other),
            };
            refused.push(ImportRefusal { public_id, issues });
        }
        Ok(refused)
    }
}

async fn imported_references(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    record: &Record,
    position: usize,
) -> Result<BTreeMap<Uuid, Uuid>, ObjectError> {
    let mut resolved = BTreeMap::new();
    let mut issues = Vec::new();
    for field in object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.field_type.holds_reference())
    {
        let Some(public_id) = record.values.get(&field.api_name).and_then(Value::as_str) else {
            continue;
        };
        let allowed: BTreeSet<Uuid> = if field.field_type == FieldType::RelatedTo {
            field.reference_object_ids.iter().copied().collect()
        } else {
            field.reference_object_id.into_iter().collect()
        };
        match tx.record_by_public_id(public_id).await? {
            Some(target) if allowed.contains(&target.object_id) => {
                resolved.insert(field.id, target.id);
            }
            _ => issues.push(ValidationIssue::new(
                format!("records/{position}/{}", field.api_name),
                "records.import_reference_missing",
                "import the record this points to first",
            )),
        }
    }
    if issues.is_empty() {
        Ok(resolved)
    } else {
        Err(ObjectError::Validation(issues))
    }
}
