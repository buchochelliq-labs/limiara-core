#![forbid(unsafe_code)]
//! Moves a workspace's people and organisations from the Contacts store to
//! the object engine (ADR 0035 §2).
//!
//! It reads through the Contacts store port, in the Contacts execution
//! context, and writes through the engine as the kernel's migrator (G4), so
//! both sides keep their tenancy, row-level security and evidence. It lives
//! beside Contacts because it knows Contacts' records; the engine knows
//! nothing of them.
//!
//! This part covers the dry run and the first copy of records with their
//! standard fields, tags, organisation lookups and activity history.
//! Workspaces with custom fields or other plugins' values on their records
//! are reported as not ready until those are mapped (slice 3b); catching up
//! with later changes, verification, rollback and cutover follow (slice 3c).

use chrono::{DateTime, SecondsFormat, Utc};
use contacts_plugin::store::{Actor, ContactsStore};
use contacts_plugin::{
    ActivityEntry, ContactCustomFieldTarget, ContactsError, EngineMigration, EngineMigrationPhase,
    ExportedActivity, Organisation, Person,
};
use platform_objects::import::{ImportedHistory, ImportedRecord, IMPORT_BATCH_MAX};
use platform_objects::{
    HistoryKind, MutationContext, ObjectCaller, ObjectError, ObjectService, ObjectStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use uuid::Uuid;

/// The engine objects Contacts declares (ADR 0035 §1).
pub const PERSON: &str = "platform.contacts:person";
pub const ORGANISATION: &str = "platform.contacts:organisation";

/// Records read and written at a time: the engine's import batch.
const BATCH: u32 = IMPORT_BATCH_MAX as u32;

/// A dry run found something that must be fixed before the copy.
pub const BLOCKED_REFUSED: &str = "contacts.migration_values_refused";
/// The workspace has custom field definitions or values; they are mapped
/// in a later part of the migration.
pub const BLOCKED_CUSTOM_FIELDS: &str = "contacts.migration_custom_fields_pending";
/// Records carry other plugins' values; those are mapped later too.
pub const BLOCKED_EXTENSION_VALUES: &str = "contacts.migration_extension_values_pending";
/// The engine objects are not installed: Contacts is not on a version that
/// declares them.
pub const BLOCKED_OBJECTS_MISSING: &str = "contacts.migration_objects_missing";

#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(transparent)]
    Contacts(#[from] ContactsError),
    #[error(transparent)]
    Engine(#[from] ObjectError),
    /// The dry run blocks the start; its report says why.
    #[error("the migration cannot start")]
    Blocked(Box<DryRun>),
    /// The copy already ran, or the workspace is past it.
    #[error("the workspace's migration is {}", .0.as_str())]
    Phase(EngineMigrationPhase),
    /// The state row holds a checkpoint this tool did not write.
    #[error("the migration state is unreadable")]
    State,
}

/// Splits a Contacts `full_name` into first and last name by ADR 0035 §1:
/// white space trimmed and collapsed; one word is the last name; otherwise
/// the last word is the last name and the words before it the first.
pub fn split_name(full_name: &str) -> (Option<String>, String) {
    let words: Vec<&str> = full_name.split_whitespace().collect();
    match words.split_last() {
        None => (None, String::new()),
        Some((last, [])) => (None, (*last).to_owned()),
        Some((last, first)) => (Some(first.join(" ")), (*last).to_owned()),
    }
}

/// What a dry run found for one object.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ObjectReport {
    pub active: u64,
    pub archived: u64,
    /// In the recycle bin; they move with their deletion time.
    pub deleted: u64,
    /// Records whose creator is no longer a member, or is unknown: they
    /// move without an owner.
    pub without_owner: u64,
    /// Values the engine would refuse.
    pub refused: Vec<Refusal>,
}

/// One value the engine would refuse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Refusal {
    pub public_id: String,
    /// The engine field, or empty for the record as a whole.
    pub field: String,
    pub code: String,
    pub detail: String,
}

/// Something that stops the copy from starting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Blocker {
    pub code: &'static str,
    pub detail: String,
}

/// A dry run's report (ADR 0035 §2, step 1). It writes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DryRun {
    pub organisations: ObjectReport,
    pub people: ObjectReport,
    /// People whose name has three or more words: the split may be wrong
    /// ("Ludwig van Beethoven"), so administrators may correct the parts.
    /// They do not block.
    pub long_names: Vec<String>,
    pub blockers: Vec<Blocker>,
}

impl DryRun {
    pub fn can_start(&self) -> bool {
        self.blockers.is_empty()
    }
}

/// What a copy wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CopyReport {
    pub organisations: u64,
    pub people: u64,
    /// Records an interrupted earlier run had already written.
    pub already_there: u64,
    pub state: Checksums,
}

/// A count and an order-independent digest of each object's records, so a
/// restarted copy can carry them on and verification can compare both
/// sides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checksums {
    pub organisations: Aggregate,
    pub people: Aggregate,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Aggregate {
    pub count: u64,
    /// The wrapping sum of each record checksum's first 16 bytes, in hex.
    pub digest: String,
}

impl Aggregate {
    fn add(&mut self, checksum: &str) -> Result<(), MigrationError> {
        let current = if self.digest.is_empty() {
            0
        } else {
            u128::from_str_radix(&self.digest, 16).map_err(|_| MigrationError::State)?
        };
        let part = u128::from_str_radix(&checksum[..32], 16).map_err(|_| MigrationError::State)?;
        self.digest = format!("{:032x}", current.wrapping_add(part));
        self.count += 1;
        Ok(())
    }
}

/// Where a copy stands, kept in the state row between batches.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Checkpoint {
    organisations_after: Option<Uuid>,
    people_after: Option<Uuid>,
    organisations_done: bool,
    copied_organisations: u64,
    copied_people: u64,
    already_there: u64,
    checksums: Checksums,
}

fn time(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// A record's checksum over its canonical form (ADR 0035 §2, step 5): ids,
/// every field value, owner, version, stamps and states, with times to the
/// microsecond as both stores keep them. History is left out; it is
/// checked by count.
pub fn checksum(record: &ImportedRecord) -> String {
    let canonical = json!([
        record.id,
        record.public_id,
        Value::Object(record.values.clone()),
        record.owner_id,
        record.version,
        time(record.created_at),
        record.created_by,
        time(record.updated_at),
        record.updated_by,
        record.archived_at.map(time),
        record.deleted_at.map(time),
    ]);
    format!("{:x}", Sha256::digest(canonical.to_string().as_bytes()))
}

fn history(activity: &[&ActivityEntry]) -> Vec<ImportedHistory> {
    activity
        .iter()
        .map(|entry| ImportedHistory {
            kind: match entry.action.as_str() {
                "created" => HistoryKind::Created,
                "archived" => HistoryKind::Archived,
                // An update the timeline kept without its values:
                // "details not recorded".
                _ => HistoryKind::Changed,
            },
            field: None,
            old_value: None,
            new_value: None,
            changed_at: entry.occurred_at,
            changed_by: entry.actor_id,
        })
        .collect()
}

fn insert(values: &mut Map<String, Value>, field: &str, value: Option<&str>) {
    if let Some(value) = value {
        values.insert(format!("platform.contacts:{field}"), json!(value));
    }
}

/// An organisation as the engine imports it.
pub fn organisation_record(
    organisation: &Organisation,
    activity: &[&ActivityEntry],
    members: &BTreeSet<Uuid>,
) -> ImportedRecord {
    let mut values = Map::new();
    values.insert("name".into(), json!(organisation.name));
    insert(&mut values, "email", organisation.email.as_deref());
    insert(&mut values, "telephone", organisation.telephone.as_deref());
    ImportedRecord {
        id: organisation.id,
        public_id: organisation.public_id.clone(),
        values,
        owner_id: organisation.created_by.filter(|id| members.contains(id)),
        version: organisation.version,
        created_at: organisation.created_at,
        created_by: organisation.created_by,
        updated_at: organisation.updated_at,
        updated_by: organisation.updated_by,
        archived_at: organisation.archived_at,
        deleted_at: organisation.deleted_at,
        history: history(activity),
    }
}

/// A person as the engine imports it: the name split into its parts, which
/// the engine composes back into the record name (G17).
pub fn person_record(
    person: &Person,
    activity: &[&ActivityEntry],
    members: &BTreeSet<Uuid>,
) -> ImportedRecord {
    let (first, last) = split_name(&person.full_name);
    let mut values = Map::new();
    insert(&mut values, "first_name", first.as_deref());
    insert(&mut values, "last_name", Some(&last));
    insert(&mut values, "email", person.email.as_deref());
    insert(&mut values, "telephone", person.telephone.as_deref());
    insert(
        &mut values,
        "organisation",
        person.organisation_public_id.as_deref(),
    );
    if !person.tags.is_empty() {
        values.insert("platform.contacts:tags".into(), json!(person.tags));
    }
    ImportedRecord {
        id: person.id,
        public_id: person.public_id.clone(),
        values,
        owner_id: person.created_by.filter(|id| members.contains(id)),
        version: person.version,
        created_at: person.created_at,
        created_by: person.created_by,
        updated_at: person.updated_at,
        updated_by: person.updated_by,
        archived_at: person.archived_at,
        deleted_at: person.deleted_at,
        history: history(activity),
    }
}

fn by_record(activity: &[ExportedActivity]) -> BTreeMap<&str, Vec<&ActivityEntry>> {
    let mut grouped: BTreeMap<&str, Vec<&ActivityEntry>> = BTreeMap::new();
    for row in activity {
        grouped
            .entry(row.entity_public_id.as_str())
            .or_default()
            .push(&row.entry);
    }
    grouped
}

fn count(report: &mut ObjectReport, record: &ImportedRecord) {
    if record.deleted_at.is_some() {
        report.deleted += 1;
    } else if record.archived_at.is_some() {
        report.archived += 1;
    } else {
        report.active += 1;
    }
    if record.owner_id.is_none() {
        report.without_owner += 1;
    }
}

/// One page of records, read in one Contacts transaction.
struct Page {
    records: Vec<ImportedRecord>,
    last: Option<Uuid>,
    /// Whether any record carries custom field or other plugins' values.
    custom_values: bool,
    extension_values: bool,
    long_names: Vec<String>,
    /// For people: the organisations they point at, internal and public id.
    references: BTreeMap<Uuid, String>,
}

/// One workspace's migration.
pub struct Migration {
    contacts: Arc<dyn ContactsStore>,
    engine: ObjectService,
    tenant: Uuid,
    members: BTreeSet<Uuid>,
}

impl Migration {
    /// `members` are the workspace's current members: a record's creator
    /// becomes its owner only while still one (ADR 0035 §2). `actor` is the
    /// administrator running it, named in the engine's evidence.
    pub fn new(
        contacts: Arc<dyn ContactsStore>,
        objects: Arc<dyn ObjectStore>,
        tenant: Uuid,
        actor: Uuid,
        members: BTreeSet<Uuid>,
    ) -> Self {
        Self {
            contacts,
            engine: ObjectService::new(objects, ObjectCaller::migrator(tenant, actor)),
            tenant,
            members,
        }
    }

    async fn organisations_page(&self, after: Option<Uuid>) -> Result<Page, MigrationError> {
        let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
        let rows = tx.export_organisations(self.tenant, after, BATCH).await?;
        let ids: Vec<String> = rows.iter().map(|row| row.public_id.clone()).collect();
        let activity = tx
            .export_activity(self.tenant, "organisation", &ids)
            .await?;
        tx.commit().await?;
        let activity = by_record(&activity);
        Ok(Page {
            last: rows.last().map(|row| row.id),
            custom_values: rows.iter().any(|row| !row.custom_fields.is_empty()),
            extension_values: rows.iter().any(|row| !row.extensions.is_empty()),
            long_names: Vec::new(),
            references: BTreeMap::new(),
            records: rows
                .iter()
                .map(|row| {
                    let entries = activity.get(row.public_id.as_str());
                    organisation_record(row, entries.map_or(&[][..], Vec::as_slice), &self.members)
                })
                .collect(),
        })
    }

    async fn people_page(&self, after: Option<Uuid>) -> Result<Page, MigrationError> {
        let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
        let rows = tx.export_people(self.tenant, after, BATCH).await?;
        let ids: Vec<String> = rows.iter().map(|row| row.public_id.clone()).collect();
        let activity = tx.export_activity(self.tenant, "person", &ids).await?;
        tx.commit().await?;
        let activity = by_record(&activity);
        Ok(Page {
            last: rows.last().map(|row| row.id),
            custom_values: rows.iter().any(|row| !row.custom_fields.is_empty()),
            extension_values: rows.iter().any(|row| !row.extensions.is_empty()),
            long_names: rows
                .iter()
                .filter(|row| row.full_name.split_whitespace().count() >= 3)
                .map(|row| row.public_id.clone())
                .collect(),
            references: rows
                .iter()
                .filter_map(|row| row.organisation_id.zip(row.organisation_public_id.clone()))
                .collect(),
            records: rows
                .iter()
                .map(|row| {
                    let entries = activity.get(row.public_id.as_str());
                    person_record(row, entries.map_or(&[][..], Vec::as_slice), &self.members)
                })
                .collect(),
        })
    }

    /// These organisations, in any state, as the engine imports them.
    async fn organisations_by_id(
        &self,
        public_ids: &[&String],
    ) -> Result<Vec<ImportedRecord>, MigrationError> {
        let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
        let mut rows = Vec::with_capacity(public_ids.len());
        for public_id in public_ids {
            rows.push(tx.get_organisation(self.tenant, public_id, true).await?);
        }
        let ids: Vec<String> = rows.iter().map(|row| row.public_id.clone()).collect();
        let activity = tx
            .export_activity(self.tenant, "organisation", &ids)
            .await?;
        tx.commit().await?;
        let activity = by_record(&activity);
        Ok(rows
            .iter()
            .map(|row| {
                let entries = activity.get(row.public_id.as_str());
                organisation_record(row, entries.map_or(&[][..], Vec::as_slice), &self.members)
            })
            .collect())
    }

    async fn page(&self, object: &str, after: Option<Uuid>) -> Result<Page, MigrationError> {
        if object == ORGANISATION {
            self.organisations_page(after).await
        } else {
            self.people_page(after).await
        }
    }

    /// Reads everything and writes nothing (ADR 0035 §2, step 1): records
    /// by state, those that would move without an owner, every value the
    /// engine would refuse, long names, and what blocks the start.
    pub async fn dry_run(&self) -> Result<DryRun, MigrationError> {
        let mut report = DryRun::default();
        let mut custom_values = false;
        let mut extension_values = false;
        {
            let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
            for target in [
                ContactCustomFieldTarget::Organisation,
                ContactCustomFieldTarget::Person,
            ] {
                if !tx.definitions(self.tenant, target).await?.is_empty() {
                    custom_values = true;
                }
            }
            tx.commit().await?;
        }
        // Every page is read before the check starts, so no Contacts
        // transaction opens while the check holds the engine's: on SQLite
        // both may share a workspace's file.
        let mut pages = Vec::new();
        for object in [ORGANISATION, PERSON] {
            let mut after = None;
            loop {
                let page = self.page(object, after).await?;
                if page.records.is_empty() {
                    break;
                }
                after = page.last;
                pages.push((object, page));
            }
        }
        // Organisations go in first, so people's lookups find them. Both
        // objects must be installed, whether or not there is anything to
        // move.
        let mut check = self.engine.check_import().await?;
        for object in [ORGANISATION, PERSON] {
            match check.check(object, Vec::new()).await {
                Ok(_) => {}
                Err(ObjectError::NotFound) => {
                    report.blockers.push(Blocker {
                        code: BLOCKED_OBJECTS_MISSING,
                        detail: format!("the engine object {object} is not installed"),
                    });
                    return Ok(report);
                }
                Err(other) => return Err(other.into()),
            }
        }
        for (object, page) in pages {
            {
                custom_values |= page.custom_values;
                extension_values |= page.extension_values;
                report.long_names.extend(page.long_names);
                let target = if object == ORGANISATION {
                    &mut report.organisations
                } else {
                    &mut report.people
                };
                for record in &page.records {
                    count(target, record);
                }
                let refused = match check.check(object, page.records).await {
                    Ok(refused) => refused,
                    Err(ObjectError::NotFound) => {
                        report.blockers.push(Blocker {
                            code: BLOCKED_OBJECTS_MISSING,
                            detail: format!("the engine object {object} is not installed"),
                        });
                        return Ok(report);
                    }
                    Err(other) => return Err(other.into()),
                };
                for refusal in refused {
                    for issue in refusal.issues {
                        // `records/{position}/{field}` names the field.
                        let field = issue.field.splitn(3, '/').nth(2).unwrap_or("").to_owned();
                        target.refused.push(Refusal {
                            public_id: refusal.public_id.clone(),
                            field,
                            code: issue.code.to_owned(),
                            detail: issue.detail,
                        });
                    }
                }
            }
        }
        drop(check);
        if !report.organisations.refused.is_empty() || !report.people.refused.is_empty() {
            report.blockers.push(Blocker {
                code: BLOCKED_REFUSED,
                detail: "fix the listed values through the Contacts API, then run again".into(),
            });
        }
        if custom_values {
            report.blockers.push(Blocker {
                code: BLOCKED_CUSTOM_FIELDS,
                detail: "custom fields are not moved to the engine yet".into(),
            });
        }
        if extension_values {
            report.blockers.push(Blocker {
                code: BLOCKED_EXTENSION_VALUES,
                detail: "values other plugins keep on records are not moved to the engine yet"
                    .into(),
            });
        }
        Ok(report)
    }

    /// The first copy (ADR 0035 §2, step 2): organisations, then people,
    /// in id order and batches, each batch one engine import, with a
    /// checkpoint saved in the state row after each. It starts only when a
    /// dry run finds nothing blocking, and resumes after an interruption:
    /// records an interrupted batch already wrote are left as they are.
    pub async fn copy(&self) -> Result<CopyReport, MigrationError> {
        let correlation = MutationContext::new(Uuid::now_v7());
        let (mut state, mut checkpoint) = self.start().await?;
        let counters = {
            let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
            let organisations = tx.public_id_counter(self.tenant, "organisation").await?;
            let people = tx.public_id_counter(self.tenant, "person").await?;
            tx.commit().await?;
            [organisations, people].map(|counter| u64::try_from(counter).unwrap_or(0))
        };
        for (object, counter) in [(ORGANISATION, counters[0]), (PERSON, counters[1])] {
            if object == ORGANISATION && checkpoint.organisations_done {
                continue;
            }
            let mut wrote_any = false;
            loop {
                let after = if object == ORGANISATION {
                    checkpoint.organisations_after
                } else {
                    checkpoint.people_after
                };
                let page = self.page(object, after).await?;
                if page.records.is_empty() {
                    break;
                }
                let ids: Vec<Uuid> = page.records.iter().map(|record| record.id).collect();
                let there = self.engine.imported_ids(&ids).await?;
                let aggregate = if object == ORGANISATION {
                    &mut checkpoint.checksums.organisations
                } else {
                    &mut checkpoint.checksums.people
                };
                for record in &page.records {
                    aggregate.add(&checksum(record))?;
                }
                let fresh: Vec<ImportedRecord> = page
                    .records
                    .into_iter()
                    .filter(|record| !there.contains(&record.id))
                    .collect();
                checkpoint.already_there += there.len() as u64;
                if object == PERSON {
                    // Contacts stays writable during the copy: a person may
                    // point at an organisation created after the
                    // organisations' pass.
                    let referenced: Vec<Uuid> = page.references.keys().copied().collect();
                    let present = self.engine.imported_ids(&referenced).await?;
                    let missing: Vec<&String> = page
                        .references
                        .iter()
                        .filter(|(id, _)| !present.contains(id))
                        .map(|(_, public_id)| public_id)
                        .collect();
                    if !missing.is_empty() {
                        let records = self.organisations_by_id(&missing).await?;
                        for record in &records {
                            checkpoint.checksums.organisations.add(&checksum(record))?;
                        }
                        checkpoint.copied_organisations += records.len() as u64;
                        self.engine
                            .import_records(ORGANISATION, records, counters[0], correlation)
                            .await?;
                    }
                }
                let written = fresh.len() as u64;
                if !fresh.is_empty() {
                    self.engine
                        .import_records(object, fresh, counter, correlation)
                        .await?;
                    wrote_any = true;
                }
                if object == ORGANISATION {
                    checkpoint.organisations_after = page.last;
                    checkpoint.copied_organisations += written;
                } else {
                    checkpoint.people_after = page.last;
                    checkpoint.copied_people += written;
                }
                state = self
                    .save(&state, EngineMigrationPhase::Copying, &checkpoint)
                    .await?;
            }
            if !wrote_any {
                // Raise the object's numbering past ids the source issued,
                // even for records since purged.
                self.engine
                    .import_records(object, Vec::new(), counter, correlation)
                    .await?;
            }
            if object == ORGANISATION {
                checkpoint.organisations_done = true;
                state = self
                    .save(&state, EngineMigrationPhase::Copying, &checkpoint)
                    .await?;
            }
        }
        self.save(&state, EngineMigrationPhase::Copied, &checkpoint)
            .await?;
        Ok(CopyReport {
            organisations: checkpoint.copied_organisations,
            people: checkpoint.copied_people,
            already_there: checkpoint.already_there,
            state: checkpoint.checksums,
        })
    }

    /// The state to copy under: a fresh start after a dry run, or the
    /// checkpoint of an interrupted copy.
    async fn start(&self) -> Result<(EngineMigration, Checkpoint), MigrationError> {
        let current = {
            let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
            let current = tx.engine_migration(self.tenant).await?;
            tx.commit().await?;
            current
        };
        match current {
            Some(state) if state.phase == EngineMigrationPhase::Copying => {
                let checkpoint = serde_json::from_value(state.checkpoint.clone())
                    .map_err(|_| MigrationError::State)?;
                Ok((state, checkpoint))
            }
            Some(state) if state.phase != EngineMigrationPhase::Legacy => {
                Err(MigrationError::Phase(state.phase))
            }
            current => {
                let report = self.dry_run().await?;
                if !report.can_start() {
                    return Err(MigrationError::Blocked(Box::new(report)));
                }
                let checkpoint = Checkpoint::default();
                let fresh = EngineMigration {
                    phase: EngineMigrationPhase::Copying,
                    checkpoint: serde_json::to_value(&checkpoint)
                        .map_err(|_| MigrationError::State)?,
                    checksums: json!({}),
                    cutover_at: None,
                    rollback_deadline: None,
                    version: 0,
                    updated_at: Utc::now(),
                };
                let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
                let saved = tx
                    .save_engine_migration(
                        self.tenant,
                        &fresh,
                        current.as_ref().map(|state| state.version),
                    )
                    .await?;
                tx.commit().await?;
                Ok((saved, checkpoint))
            }
        }
    }

    async fn save(
        &self,
        state: &EngineMigration,
        phase: EngineMigrationPhase,
        checkpoint: &Checkpoint,
    ) -> Result<EngineMigration, MigrationError> {
        let next = EngineMigration {
            phase,
            checkpoint: serde_json::to_value(checkpoint).map_err(|_| MigrationError::State)?,
            checksums: serde_json::to_value(&checkpoint.checksums)
                .map_err(|_| MigrationError::State)?,
            ..state.clone()
        };
        let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
        let saved = tx
            .save_engine_migration(self.tenant, &next, Some(state.version))
            .await?;
        tx.commit().await?;
        Ok(saved)
    }

    /// The workspace's migration state.
    pub async fn state(&self) -> Result<Option<EngineMigration>, MigrationError> {
        let mut tx = self.contacts.begin(self.tenant, Actor::System).await?;
        let state = tx.engine_migration(self.tenant).await?;
        tx.commit().await?;
        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_split_by_the_adr_rule() {
        assert_eq!(split_name("Ada"), (None, "Ada".to_owned()));
        assert_eq!(
            split_name("  Ada   Lovelace "),
            (Some("Ada".to_owned()), "Lovelace".to_owned())
        );
        assert_eq!(
            split_name("Ludwig van Beethoven"),
            (Some("Ludwig van".to_owned()), "Beethoven".to_owned())
        );
        assert_eq!(split_name("   "), (None, String::new()));
    }

    #[test]
    fn activity_becomes_backdated_history() {
        let at = |minute: u32| {
            DateTime::parse_from_rfc3339(&format!("2026-01-01T00:{minute:02}:00Z"))
                .expect("time")
                .with_timezone(&Utc)
        };
        let actor = Uuid::now_v7();
        let entries: Vec<ActivityEntry> = [("created", 1), ("updated", 2), ("archived", 3)]
            .into_iter()
            .map(|(action, minute)| ActivityEntry {
                event_id: Uuid::now_v7(),
                action: action.into(),
                entity_version: 1,
                actor_id: Some(actor),
                occurred_at: at(minute),
            })
            .collect();
        let mapped = history(&entries.iter().collect::<Vec<_>>());
        assert_eq!(
            mapped.iter().map(|entry| entry.kind).collect::<Vec<_>>(),
            vec![
                HistoryKind::Created,
                HistoryKind::Changed,
                HistoryKind::Archived
            ]
        );
        assert!(mapped
            .iter()
            .all(|entry| entry.field.is_none() && entry.changed_by == Some(actor)));
        assert_eq!(mapped[1].changed_at, at(2), "kept at its own time");
    }

    #[test]
    fn the_aggregate_does_not_depend_on_order() {
        let a = format!("{:064x}", 7u8);
        let b = format!("{:0>64}", "ff".repeat(16));
        let mut one = Aggregate::default();
        let mut two = Aggregate::default();
        one.add(&a).expect("a");
        one.add(&b).expect("b");
        two.add(&b).expect("b");
        two.add(&a).expect("a");
        assert_eq!(one, two);
        assert_eq!(one.count, 2);
    }
}
