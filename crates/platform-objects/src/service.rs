//! The object engine service: authorisation, validation and evidence for
//! every metadata and record change, independent of the storage backend.

use crate::automation::Run;
use crate::events::{Change, EvidenceContext};
use crate::naming::{custom_api_name, next_workspace_prefix};
use crate::sharing::{self, GranteeKind, RecordRights, RecordShare, ShareAccess};
use crate::values::{
    self, apply_values, filter_value, record_index, FIELDS_PER_OBJECT_MAX, LABEL_MAX,
    MASTERS_PER_OBJECT_MAX, ROLLUPS_PER_OBJECT_MAX,
};
use crate::SaveEvent;
use crate::{
    AccessFlags, ChangePage, CreateField, CreateObject, DefinitionStatus, FieldDefinition,
    FieldType, HistoryEntry, HistoryKind, HistoryPage, ObjectAccess, ObjectCaller,
    ObjectDefinition, ObjectError, ObjectStore, ObjectTx, Origin, Record, RecordChange,
    RecordCursor, RecordFilter, RecordIndex, RecordPage, RecordQuery, RecordSort, RecordStatus,
    RelatedTarget, RolePermission, SortDirection, UpdateField, UpdateObject, ValidationIssue,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use uuid::Uuid;

/// Upper bound on objects per workspace.
pub const OBJECTS_PER_WORKSPACE_MAX: usize = 2000;
/// A workspace administrator tried to change or delete a definition a
/// plugin owns: only the plugin's installation may (ADR 0033 §2).
pub const PLUGIN_OWNED: &str = "objects.plugin_owned";
pub const LIST_LIMIT_DEFAULT: u16 = 50;
pub const LIST_LIMIT_MAX: u16 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MutationContext {
    pub correlation_id: Uuid,
    pub causation_id: Option<Uuid>,
    /// The automation making the change, when one is (ADR 0022 §5).
    pub automation_id: Option<Uuid>,
    /// Where the change came from (#233), as the entry point verified it;
    /// `None` when it did not say. Provenance only: it never changes what
    /// the caller may do.
    pub source: Option<crate::MutationSource>,
}

impl MutationContext {
    pub fn new(correlation_id: Uuid) -> Self {
        Self {
            correlation_id,
            causation_id: None,
            automation_id: None,
            source: None,
        }
    }

    /// The same context, recording `source` as where the change came from.
    pub fn with_source(self, source: crate::MutationSource) -> Self {
        Self {
            source: Some(source),
            ..self
        }
    }
}

/// How long the change log keeps changes (ADR 0018); Salesforce keeps
/// change events for three days too.
pub const CHANGE_RETENTION: chrono::TimeDelta = chrono::TimeDelta::hours(72);
/// The log is expired every this many appends.
const CHANGE_EXPIRY_EVERY: i64 = 256;

/// A list request as callers express it, before field names are resolved.
#[derive(Debug, Clone, Default)]
pub struct ListRecords {
    pub status: RecordStatus,
    pub sort: RecordSort,
    pub direction: SortDirection,
    pub limit: Option<u16>,
    pub cursor: Option<String>,
    /// Equality filters by field API name.
    pub filters: Vec<(String, String)>,
    /// Only records this user owns.
    pub owner: Option<Uuid>,
    pub search: Option<String>,
    /// Only records touched strictly after this moment, for sync (#287).
    pub touched_after: Option<chrono::DateTime<Utc>>,
}

/// Opaque position in a record's history: the last entry's time and id.
#[derive(Serialize, Deserialize)]
struct HistoryCursor {
    t: chrono::DateTime<Utc>,
    i: Uuid,
}

#[derive(Serialize, Deserialize)]
struct CursorToken {
    s: String,
    d: String,
    v: String,
    i: Uuid,
}

fn sort_code(sort: RecordSort, direction: SortDirection) -> (&'static str, &'static str) {
    (
        match sort {
            RecordSort::Name => "name",
            RecordSort::CreatedAt => "created_at",
            RecordSort::UpdatedAt => "updated_at",
            RecordSort::TouchedAt => "touched_at",
        },
        match direction {
            SortDirection::Asc => "asc",
            SortDirection::Desc => "desc",
        },
    )
}

fn invalid_cursor() -> ObjectError {
    ObjectError::invalid("cursor", "records.cursor_invalid", "start the list again")
}

fn time_key(value: chrono::DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Micros, true)
}

/// The value a record sorts by, as the cursor stores it.
pub fn sort_value(record: &Record, sort: RecordSort) -> String {
    match sort {
        RecordSort::Name => values::name_key(&record.name),
        RecordSort::CreatedAt => time_key(record.created_at),
        RecordSort::UpdatedAt => time_key(record.updated_at),
        RecordSort::TouchedAt => time_key(record.touched_at),
    }
}

#[derive(Clone)]
pub struct ObjectService {
    store: Arc<dyn ObjectStore>,
    caller: ObjectCaller,
    replica: bool,
    client_ids: bool,
    trusted_clock: Option<Arc<dyn crate::TrustedClock>>,
}

impl ObjectService {
    pub fn new(store: Arc<dyn ObjectStore>, caller: ObjectCaller) -> Self {
        Self {
            store,
            caller,
            replica: false,
            client_ids: false,
            trusted_clock: None,
        }
    }

    /// Measures the offline lease by the device's monotonic clock as well
    /// as the wall clock (ADR 0024, "Trusted elapsed time").
    pub fn with_trusted_clock(mut self, clock: Arc<dyn crate::TrustedClock>) -> Self {
        self.trusted_clock = Some(clock);
        self
    }

    pub(crate) fn trusted_now(&self) -> Option<crate::TrustedInstant> {
        self.trusted_clock.as_ref().and_then(|clock| clock.now())
    }

    /// The engine of a synced device's replica (ADR 0024 §3), which may
    /// store records as the server sends them (`apply_remote`). Never set on
    /// the server.
    pub fn with_replica(mut self) -> Self {
        self.replica = true;
        self
    }

    pub(crate) fn is_replica(&self) -> bool {
        self.replica
    }

    /// New records get ids from the device range instead of the
    /// workspace's sequence (ADR 0031), as a synced replica's engine does,
    /// so the server can keep them when the device pushes.
    pub fn with_client_ids(mut self) -> Self {
        self.client_ids = true;
        self
    }

    pub fn caller(&self) -> &ObjectCaller {
        &self.caller
    }

    pub(crate) async fn begin(&self) -> Result<Box<dyn ObjectTx>, ObjectError> {
        if self.replica {
            self.note_clock().await?;
        }
        self.store.begin(self.caller.tenant_id()).await
    }

    /// A replica writes down the latest device time it has seen, in a
    /// transaction of its own so it is kept even when the work that follows
    /// is refused, which stops a clock turned back from reopening a lease
    /// that has run out (ADR 0024 §4).
    async fn note_clock(&self) -> Result<(), ObjectError> {
        let now = Utc::now();
        let mut tx = self.store.begin(self.caller.tenant_id()).await?;
        let mut lease = tx.replica_lease().await?;
        if crate::replica::Lease::should_note(&lease, now) {
            lease.clock_seen_at = Some(lease.clock_seen_at.map_or(now, |seen| seen.max(now)));
            tx.set_replica_lease(lease).await?;
        }
        tx.commit().await
    }

    /// What the caller may do with each object. In a replica whose offline
    /// lease has run out, the member may do nothing until the device reaches
    /// the server again (ADR 0024 §4); syncing does not go through here, so
    /// a sync can still unlock it.
    pub(crate) async fn access(&self, tx: &mut dyn ObjectTx) -> Result<ObjectAccess, ObjectError> {
        if self.is_replica()
            && crate::replica::Lease::at(
                &tx.replica_lease().await?,
                Utc::now(),
                self.trusted_now().as_ref(),
            )
            .locked
        {
            return Err(ObjectError::Conflict("sync.lease_expired"));
        }
        let grants = tx.grants(self.caller.role_ids()).await?;
        let masters = tx.master_links().await?;
        Ok(ObjectAccess::resolve(&self.caller, grants).with_masters(masters))
    }

    pub(crate) fn evidence(&self, context: MutationContext) -> EvidenceContext {
        EvidenceContext {
            tenant_id: self.caller.tenant_id(),
            actor_id: self.caller.user_id(),
            correlation_id: context.correlation_id,
            causation_id: context.causation_id,
            automation_id: context.automation_id,
            source: self.source(context),
        }
    }

    /// Where a change came from (#233): what the entry point said, or on a
    /// device's replica, where every change is the device's own, `device`.
    pub(crate) fn source(&self, context: MutationContext) -> Option<crate::MutationSource> {
        context
            .source
            .or_else(|| self.replica.then(|| crate::MutationSource::device(None)))
    }

    pub(crate) async fn record_change(
        &self,
        tx: &mut dyn ObjectTx,
        context: MutationContext,
        change: Change<'_>,
    ) -> Result<(), ObjectError> {
        let (mut audit, mut event) = change.evidence(self.evidence(context));
        // A plugin's installation names the plugin in its evidence, next to
        // the member who changed the plugin's state (ADR 0033 §2).
        if let Some(plugin) = self.caller.installer() {
            for value in [&mut audit.metadata, &mut event.payload] {
                if let Some(map) = value.as_object_mut() {
                    map.insert("plugin".into(), Value::String(plugin.to_owned()));
                }
            }
        }
        tx.append_evidence(&audit, &event).await?;
        if change.is_metadata() {
            tx.bump_metadata_version().await?;
        }
        if let Some((object, record, kind, fields)) = change.record_change() {
            let position = tx
                .append_change(&RecordChange {
                    position: 0,
                    record_id: record.id,
                    object_id: object.id,
                    public_id: record.public_id.clone(),
                    kind,
                    fields,
                    changed_at: record.touched_at,
                    changed_by: Some(self.caller.user_id()),
                    correlation_id: Some(context.correlation_id),
                })
                .await?;
            // Expire the log as it grows rather than on a schedule.
            if position % CHANGE_EXPIRY_EVERY == 0 {
                tx.expire_changes(Utc::now() - CHANGE_RETENTION).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn active_object(
        &self,
        tx: &mut dyn ObjectTx,
        api_name: &str,
    ) -> Result<ObjectDefinition, ObjectError> {
        tx.object_by_api_name(api_name)
            .await?
            .filter(ObjectDefinition::is_active)
            .ok_or(ObjectError::NotFound)
    }

    // --- Metadata -------------------------------------------------------

    /// Objects the caller can read records of, or every object for
    /// administrators who manage objects.
    pub async fn list_objects(&self) -> Result<Vec<(ObjectDefinition, AccessFlags)>, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let objects = tx.list_objects().await?;
        tx.commit().await?;
        Ok(objects
            .into_iter()
            .map(|object| {
                let flags = access.for_object(object.id);
                (object, flags)
            })
            .filter(|(_, flags)| flags.read || self.caller.can_manage())
            .collect())
    }

    /// One object with its fields. Unknown and unreadable objects are
    /// indistinguishable.
    pub async fn describe(
        &self,
        api_name: &str,
    ) -> Result<(ObjectDefinition, AccessFlags), ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        tx.commit().await?;
        let flags = access.for_object(object.id);
        if !flags.read && !self.caller.can_manage() {
            return Err(ObjectError::NotFound);
        }
        Ok((object, flags))
    }

    pub async fn create_object(
        &self,
        input: CreateObject,
        context: MutationContext,
    ) -> Result<ObjectDefinition, ObjectError> {
        self.caller.require_manage()?;
        let mut issues = Vec::new();
        let api_name = custom_api_name("name", &input.name).map_err(|error| match error {
            ObjectError::Validation(found) => {
                issues.extend(found);
                ObjectError::Invariant
            }
            other => other,
        });
        let label =
            values::label("label", &input.label, LABEL_MAX).map_err(|issue| issues.push(issue));
        let plural_label = values::label("plural_label", &input.plural_label, LABEL_MAX)
            .map_err(|issue| issues.push(issue));
        let name_label = values::label(
            "name_label",
            input.name_label.as_deref().unwrap_or("Name"),
            LABEL_MAX,
        )
        .map_err(|issue| issues.push(issue));
        let description =
            values::description(input.description.as_deref()).map_err(|issue| issues.push(issue));
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        let (Ok(api_name), Ok(label), Ok(plural_label), Ok(name_label), Ok(description)) =
            (api_name, label, plural_label, name_label, description)
        else {
            return Err(ObjectError::Invariant);
        };

        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let prefixes = tx.key_prefixes().await?;
        if prefixes.len() >= OBJECTS_PER_WORKSPACE_MAX {
            return Err(ObjectError::Conflict("objects.limit_reached"));
        }
        let key_prefix = next_workspace_prefix(prefixes.iter().map(String::as_str))
            .ok_or(ObjectError::Conflict("objects.limit_reached"))?;
        let now = Utc::now();
        let object = ObjectDefinition {
            id: Uuid::now_v7(),
            api_name,
            label,
            plural_label,
            description,
            origin: Origin::Workspace,
            owner_plugin: None,
            key_prefix,
            name_label,
            sharing_model: input.sharing_model.unwrap_or_default(),
            grant_hierarchy: input.grant_hierarchy.unwrap_or(true),
            track_history: input.track_history.unwrap_or(false),
            searchable: input.searchable.unwrap_or(true),
            in_navigation: input.in_navigation.unwrap_or(true),
            status: DefinitionStatus::Active,
            suspended: false,
            version: 1,
            created_at: now,
            created_by: Some(self.caller.user_id()),
            updated_at: now,
            updated_by: Some(self.caller.user_id()),
            fields: Vec::new(),
        };
        tx.insert_object(&object).await?;
        self.record_change(tx.as_mut(), context, Change::ObjectCreated(&object))
            .await?;
        tx.commit().await?;
        Ok(object)
    }

    pub async fn update_object(
        &self,
        api_name: &str,
        input: UpdateObject,
        context: MutationContext,
    ) -> Result<ObjectDefinition, ObjectError> {
        self.caller.require_manage()?;
        let mut tx = self.begin().await?;
        let mut object = self.active_object(tx.as_mut(), api_name).await?;
        // A plugin object's labels and flags are the plugin's; only its
        // sharing settings are the workspace's to choose (ADR 0033 §2).
        if object.is_plugin_owned()
            && (input.label.is_some()
                || input.plural_label.is_some()
                || input.description.is_some()
                || input.name_label.is_some()
                || input.track_history.is_some()
                || input.searchable.is_some()
                || input.in_navigation.is_some())
        {
            return Err(ObjectError::Conflict(PLUGIN_OWNED));
        }
        let mut issues = Vec::new();
        if let Some(label) = input.label.as_deref() {
            match values::label("label", label, LABEL_MAX) {
                Ok(label) => object.label = label,
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(label) = input.plural_label.as_deref() {
            match values::label("plural_label", label, LABEL_MAX) {
                Ok(label) => object.plural_label = label,
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(label) = input.name_label.as_deref() {
            match values::label("name_label", label, LABEL_MAX) {
                Ok(label) => object.name_label = label,
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(description) = input.description {
            match values::description(description.as_deref()) {
                Ok(description) => object.description = description,
                Err(issue) => issues.push(issue),
            }
        }
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        if let Some(model) = input.sharing_model {
            object.sharing_model = model;
        }
        if let Some(grant) = input.grant_hierarchy {
            object.grant_hierarchy = grant;
        }
        if let Some(track) = input.track_history {
            object.track_history = track;
        }
        if let Some(searchable) = input.searchable {
            object.searchable = searchable;
        }
        if let Some(navigation) = input.in_navigation {
            object.in_navigation = navigation;
        }
        self.write_object(tx.as_mut(), &mut object, input.expected_version)
            .await?;
        self.record_change(tx.as_mut(), context, Change::ObjectUpdated(&object))
            .await?;
        tx.commit().await?;
        Ok(object)
    }

    async fn write_object(
        &self,
        tx: &mut dyn ObjectTx,
        object: &mut ObjectDefinition,
        expected_version: i64,
    ) -> Result<(), ObjectError> {
        if object.version != expected_version {
            return Err(ObjectError::VersionConflict);
        }
        object.version += 1;
        object.updated_at = Utc::now();
        object.updated_by = Some(self.caller.user_id());
        if tx.update_object(object, expected_version).await? {
            Ok(())
        } else {
            Err(ObjectError::VersionConflict)
        }
    }

    /// Deletes an object definition. Its records leave every list and read;
    /// lookups from other objects must be removed first. Related-to fields
    /// of other objects stop allowing it and lose their values that point
    /// to its records.
    pub async fn delete_object(
        &self,
        api_name: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<ObjectDefinition, ObjectError> {
        self.caller.require_manage()?;
        let mut tx = self.begin().await?;
        // Serialised with metadata that could start to depend on it.
        tx.lock_metadata().await?;
        let mut object = self.active_object(tx.as_mut(), api_name).await?;
        if object.is_plugin_owned() {
            return Err(ObjectError::Conflict(PLUGIN_OWNED));
        }
        let referencing = tx.fields_referencing_object(object.id).await?;
        // A lookup from another object keeps it, and so does a related-to
        // field that allows nothing else; other related-to fields let go
        // of it below (ADR 0033 §1).
        if referencing.iter().any(|field| {
            field.object_id != object.id
                && (field.field_type.is_reference() || field.reference_object_ids == [object.id])
        }) {
            return Err(ObjectError::Conflict("objects.object_referenced"));
        }
        // A master's roll-up summary of its records keeps it (ADR 0026).
        if rollup_uses(tx.as_mut(), &object, None).await? {
            return Err(ObjectError::Conflict("objects.object_in_use"));
        }
        // What belongs to the object goes with it; anything else that needs
        // it, such as an automation creating its records, keeps it.
        let mut before = crate::rules::compiling_dependents(tx.as_mut()).await?;
        before.retain(|_, owner| *owner != object.id);
        object.status = DefinitionStatus::Deleted;
        self.write_object(tx.as_mut(), &mut object, expected_version)
            .await?;
        if !still_working(&before, tx.as_mut()).await? {
            return Err(ObjectError::Conflict("objects.object_in_use"));
        }
        self.record_change(tx.as_mut(), context, Change::ObjectDeleted(&object))
            .await?;
        // Related-to fields let go of it: those that allow it, and those
        // still holding values from before it left their allowlist.
        let mut releasing: Vec<FieldDefinition> = referencing
            .into_iter()
            .filter(|field| field.field_type == FieldType::RelatedTo)
            .collect();
        for field in tx.related_to_fields_holding(object.id).await? {
            if !releasing.iter().any(|known| known.id == field.id) {
                releasing.push(field);
            }
        }
        for field in releasing
            .iter()
            .filter(|field| field.object_id != object.id)
        {
            self.release_allowlist(tx.as_mut(), field, &object, context)
                .await?;
        }
        // Its quick actions, and the create actions making its records, go
        // with it (ADR 0032 §4).
        self.quick_actions_after_object_deleted(tx.as_mut(), &object, context)
            .await?;
        tx.commit().await?;
        Ok(object)
    }

    pub async fn create_field(
        &self,
        api_name: &str,
        input: CreateField,
        context: MutationContext,
    ) -> Result<FieldDefinition, ObjectError> {
        self.caller.require_manage()?;
        let mut issues = Vec::new();
        let field_api = match custom_api_name("name", &input.name) {
            Ok(name) => Some(name),
            Err(ObjectError::Validation(found)) => {
                issues.extend(found);
                None
            }
            Err(other) => return Err(other),
        };
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        // Boxed: the field checks hold a large future.
        let field = Box::pin(self.insert_field_in(
            tx.as_mut(),
            &object,
            field_api,
            Origin::Workspace,
            input,
            issues,
            context,
        ))
        .await?;
        tx.commit().await?;
        Ok(field)
    }

    /// Validates and adds a field to `object` in the caller's transaction,
    /// with its evidence. `issues` holds what the caller already found wrong
    /// with the name; `field_api` is `None` when the name was refused.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_field_in(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        field_api: Option<String>,
        origin: Origin,
        input: CreateField,
        mut issues: Vec<ValidationIssue>,
        context: MutationContext,
    ) -> Result<FieldDefinition, ObjectError> {
        let label = values::label("label", &input.label, LABEL_MAX)
            .map_err(|issue| issues.push(issue))
            .ok();
        let description = values::description(input.description.as_deref())
            .map_err(|issue| issues.push(issue))
            .ok()
            .flatten();
        let shape = values::field_shape_for(&input, origin == Origin::Plugin)
            .map_err(|found| issues.extend(found))
            .ok();
        let lookup_filter =
            crate::related::lookup_filter(input.field_type, input.lookup_filter.as_deref())
                .map_err(|issue| issues.push(issue))
                .ok()
                .flatten();

        if object.fields.len() >= FIELDS_PER_OBJECT_MAX {
            return Err(ObjectError::Conflict("objects.field_limit_reached"));
        }
        let reference = match input.reference_object.as_deref() {
            Some(target) => match tx.object_by_api_name(target).await? {
                Some(target) if usable_target(&target, self.caller.installer()) => Some(target),
                _ => {
                    issues.push(ValidationIssue::new(
                        "reference_object",
                        "objects.reference_object_unknown",
                        "choose an existing object",
                    ));
                    None
                }
            },
            None => None,
        };
        let allowed = match &shape {
            Some(shape) => {
                match allowed_objects(tx, &shape.reference_objects, self.caller.installer()).await?
                {
                    Ok(allowed) => allowed,
                    Err(issue) => {
                        issues.push(issue);
                        Vec::new()
                    }
                }
            }
            None => Vec::new(),
        };
        let (Some(field_api), Some(label), Some(shape)) = (field_api, label, shape) else {
            return Err(ObjectError::Validation(issues));
        };
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        if input.field_type == FieldType::MasterDetail {
            check_master_detail(tx, object, reference.as_ref()).await?;
        }
        if let (Some(filter), Some(target)) = (&lookup_filter, &reference) {
            self.check_lookup_filter(tx, target, filter).await?;
        }
        let rollup = match (input.field_type, input.rollup.as_ref()) {
            (FieldType::Rollup, Some(rollup)) => {
                // Holds back new masters until the field commits, so each
                // one is either backfilled here or starts with the field.
                tx.lock_object(object.id, true).await?;
                Some(resolve_rollup(tx, object, rollup).await?)
            }
            _ => None,
        };
        let now = Utc::now();
        let mut field = FieldDefinition {
            id: Uuid::now_v7(),
            object_id: object.id,
            api_name: field_api,
            label,
            description,
            field_type: input.field_type,
            origin,
            // A detail always has a master.
            required: input.required || input.field_type == FieldType::MasterDetail,
            unique: shape.unique,
            indexed: input.indexed || shape.unique,
            searchable: input.searchable && input.field_type.is_textual(),
            track_history: input.track_history,
            max_length: shape.max_length,
            scale: rollup.as_ref().map_or(shape.scale, |found| found.scale),
            options: shape.options,
            default_value: None,
            reference_object_id: reference.as_ref().map(|target| target.id),
            reference_object: reference.map(|target| target.api_name),
            reference_object_ids: allowed.iter().map(|target| target.id).collect(),
            reference_objects: allowed.into_iter().map(|target| target.api_name).collect(),
            delete_behaviour: shape.delete_behaviour,
            lookup_filter,
            formula: shape.formula,
            formula_type: rollup
                .as_ref()
                .map_or(shape.formula_type, |found| Some(found.result_type)),
            rollup: rollup.as_ref().map(|found| found.rollup.clone()),
            position: object
                .fields
                .iter()
                .map(|field| field.position)
                .max()
                .map_or(0, |max| max + 1),
            status: DefinitionStatus::Active,
            version: 1,
            created_at: now,
            created_by: Some(self.caller.user_id()),
            updated_at: now,
            updated_by: Some(self.caller.user_id()),
        };
        field.default_value = default_value(&field, input.default_value.as_ref())?;
        if let (Some(formula), Some(formula_type)) = (&field.formula, field.formula_type) {
            crate::formula_fields::check_formula(tx, object, formula, formula_type, None).await?;
        }
        tx.insert_field(&field).await?;
        if field.field_type == FieldType::Formula {
            // Existing records get the new field's stored value (ADR 0021 §6).
            let updated = tx
                .object_by_id(object.id)
                .await?
                .ok_or(ObjectError::Invariant)?;
            reindex(tx, &updated).await?;
        }
        if let Some(found) = rollup {
            if let Some(mut summarised) = found.index {
                // Roll-ups read the summarised field's index rows.
                let version = summarised.version;
                summarised.indexed = true;
                summarised.version += 1;
                summarised.updated_at = now;
                summarised.updated_by = Some(self.caller.user_id());
                if !tx.update_field(&summarised, version).await? {
                    return Err(ObjectError::VersionConflict);
                }
                let detail = tx
                    .object_by_id(summarised.object_id)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                reindex(tx, &detail).await?;
            }
            let master = tx
                .object_by_id(object.id)
                .await?
                .ok_or(ObjectError::Invariant)?;
            let created = master
                .field(&field.api_name)
                .ok_or(ObjectError::Invariant)?;
            // Existing masters get their value now.
            for id in tx.record_ids(master.id).await? {
                self.recompute_rollups(tx, &master, id, &[created], context)
                    .await?;
            }
        }
        self.record_change(tx, context, Change::FieldCreated(object, &field))
            .await?;
        Ok(field)
    }

    pub async fn update_field(
        &self,
        api_name: &str,
        field_api_name: &str,
        input: UpdateField,
        context: MutationContext,
    ) -> Result<FieldDefinition, ObjectError> {
        self.caller.require_manage()?;
        let mut tx = self.begin().await?;
        // Serialised with field deletion when the formula changes (ADR 0021).
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let field = object
            .field(field_api_name)
            .cloned()
            .ok_or(ObjectError::NotFound)?;
        // A plugin's own fields change only with the plugin, except the
        // allowlist of a related-to field, whose targets are the
        // workspace's to choose (ADR 0033 §2).
        if field.origin == Origin::Plugin && !only_allowlist(&field, &input) {
            return Err(ObjectError::Conflict(PLUGIN_OWNED));
        }
        // Boxed: the field checks hold a large future.
        let field =
            Box::pin(self.change_field_in(tx.as_mut(), &object, field, input, context)).await?;
        tx.commit().await?;
        Ok(field)
    }

    /// Validates and applies a change to one of `object`'s fields in the
    /// caller's transaction, with its evidence.
    pub(crate) async fn change_field_in(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        mut field: FieldDefinition,
        input: UpdateField,
        context: MutationContext,
    ) -> Result<FieldDefinition, ObjectError> {
        if field.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        let reindex_before = (field.is_indexed(), field.searchable, field.options.clone());
        let allowlist_before = field.reference_object_ids.clone();
        let mut issues = Vec::new();
        if let Some(label) = input.label.as_deref() {
            match values::label("label", label, LABEL_MAX) {
                Ok(label) => field.label = label,
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(description) = input.description {
            match values::description(description.as_deref()) {
                Ok(description) => field.description = description,
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(required) = input.required {
            if !required && field.field_type == FieldType::MasterDetail {
                issues.push(ValidationIssue::new(
                    "required",
                    "objects.master_detail_settings_invalid",
                    "a master-detail field is always required",
                ));
            }
            field.required = required;
        }
        if let Some(filter) = input.lookup_filter {
            match crate::related::lookup_filter(field.field_type, filter.as_deref()) {
                Ok(Some(filter)) => {
                    let target = tx
                        .object_by_id(field.reference_object_id.ok_or(ObjectError::Invariant)?)
                        .await?
                        .ok_or(ObjectError::Invariant)?;
                    match self.check_lookup_filter(tx, &target, &filter).await {
                        Ok(()) => field.lookup_filter = Some(filter),
                        Err(ObjectError::Validation(found)) => issues.extend(found),
                        Err(other) => return Err(other),
                    }
                }
                Ok(None) => field.lookup_filter = None,
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(names) = input.reference_objects.as_deref() {
            match values::reference_objects(
                field.field_type,
                Some(names),
                field.origin == Origin::Plugin,
            ) {
                Ok(names) => match allowed_objects(tx, &names, self.caller.installer()).await? {
                    Ok(allowed) => {
                        field.reference_object_ids =
                            allowed.iter().map(|target| target.id).collect();
                        field.reference_objects =
                            allowed.into_iter().map(|target| target.api_name).collect();
                    }
                    Err(issue) => issues.push(issue),
                },
                Err(issue) => issues.push(issue),
            }
        }
        if let Some(indexed) = input.indexed {
            // Roll-up summaries read the fields they summarise from the
            // index (ADR 0026).
            if !indexed && rollup_uses(tx, object, Some(field.id)).await? {
                issues.push(ValidationIssue::new(
                    "indexed",
                    "objects.rollup_needs_index",
                    "a roll-up summary summarises this field, so it stays indexed",
                ));
            }
            field.indexed = indexed || field.unique;
        }
        if let Some(searchable) = input.searchable {
            field.searchable = searchable && field.field_type.is_textual();
        }
        if let Some(track) = input.track_history {
            field.track_history = track;
        }
        if let Some(position) = input.position {
            field.position = position.clamp(0, 10_000);
        }
        if let Some(max_length) = input.max_length {
            let limit = match field.field_type {
                FieldType::Text => values::TEXT_LIMIT,
                FieldType::LongText => values::LONG_TEXT_LIMIT,
                _ => {
                    issues.push(ValidationIssue::new(
                        "max_length",
                        "objects.max_length_not_allowed",
                        "only text fields have a maximum length",
                    ));
                    0
                }
            };
            if limit > 0 && !(1..=limit).contains(&max_length) {
                issues.push(ValidationIssue::new(
                    "max_length",
                    "objects.max_length_invalid",
                    format!("use 1 to {limit}"),
                ));
            } else if limit > 0 {
                field.max_length = Some(max_length);
            }
        }
        if let Some(options) = input.options.as_deref() {
            match values::picklist_options(field.field_type, options) {
                Ok(options) => field.options = options,
                Err(issue) => issues.push(issue),
            }
        }
        if field.field_type == FieldType::Formula
            && (field.required
                || field.indexed
                || field.searchable
                || field.track_history
                || input.default_value.is_some())
        {
            issues.push(ValidationIssue::new(
                "field_type",
                "objects.formula_settings_invalid",
                "formula fields cannot be required, indexed, searchable, tracked or defaulted",
            ));
        }
        if field.field_type == FieldType::Rollup
            && (field.required || field.track_history || input.default_value.is_some())
        {
            issues.push(ValidationIssue::new(
                "field_type",
                "objects.rollup_settings_invalid",
                "roll-up summary fields cannot be required, tracked or defaulted",
            ));
        }
        // Formulas that use this one must keep working (ADR 0021).
        let dependents_before = if input.formula.is_some() {
            let mut before = crate::rules::compiling_dependents(tx).await?;
            before.remove(&field.id);
            Some(before)
        } else {
            None
        };
        match (input.formula, field.field_type) {
            (Some(formula), FieldType::Formula) => {
                let formula = formula.trim().to_owned();
                let formula_type = field.formula_type.ok_or(ObjectError::Invariant)?;
                crate::formula_fields::check_formula(
                    tx,
                    object,
                    &formula,
                    formula_type,
                    Some(field.id),
                )
                .await?;
                field.formula = Some(formula);
            }
            (Some(_), _) => issues.push(ValidationIssue::new(
                "formula",
                "objects.formula_not_allowed",
                "only formula fields have a formula",
            )),
            (None, _) => {}
        }
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        if let Some(default) = input.default_value {
            field.default_value = default_value(&field, default.as_ref())?;
        }
        field.version += 1;
        field.updated_at = Utc::now();
        field.updated_by = Some(self.caller.user_id());
        if !tx.update_field(&field, input.expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        let formula_changed = dependents_before.is_some();
        if let Some(before) = dependents_before {
            if !still_working(&before, tx).await? {
                return Err(ObjectError::Conflict("objects.field_in_use"));
            }
        }
        if formula_changed
            || reindex_before != (field.is_indexed(), field.searchable, field.options.clone())
        {
            let updated = tx
                .object_by_id(object.id)
                .await?
                .ok_or(ObjectError::Invariant)?;
            reindex(tx, &updated).await?;
        }
        self.record_change(tx, context, Change::FieldUpdated(object, &field))
            .await?;
        if allowlist_before != field.reference_object_ids {
            // Create actions of an object taken out can no longer fill the
            // field (ADR 0032 §4).
            self.quick_actions_after_allowlist_changed(tx, object, &field, context)
                .await?;
            // Objects that entered it are offered the related actions the
            // plugin declares on the field (ADR 0033 §3).
            if field.origin == Origin::Plugin {
                let updated = tx
                    .object_by_id(object.id)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                self.offer_related_actions(tx, &updated, Some(field.id), context)
                    .await?;
            }
        }
        Ok(field)
    }

    pub async fn delete_field(
        &self,
        api_name: &str,
        field_api_name: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<FieldDefinition, ObjectError> {
        self.caller.require_manage()?;
        let mut tx = self.begin().await?;
        // Serialised with metadata changes, so no rule or formula field can
        // start depending on a field while it is being deleted (ADR 0021).
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let mut field = object
            .field(field_api_name)
            .cloned()
            .ok_or(ObjectError::NotFound)?;
        if field.origin == Origin::Plugin {
            return Err(ObjectError::Conflict(PLUGIN_OWNED));
        }
        if field.version != expected_version {
            return Err(ObjectError::VersionConflict);
        }
        // A roll-up summary that reads the field keeps it (ADR 0026).
        if rollup_uses(tx.as_mut(), &object, Some(field.id)).await? {
            return Err(ObjectError::Conflict("objects.field_in_use"));
        }
        let mut rules_before = crate::rules::compiling_dependents(tx.as_mut()).await?;
        rules_before.remove(&field.id);
        field.status = DefinitionStatus::Deleted;
        field.version += 1;
        field.updated_at = Utc::now();
        field.updated_by = Some(self.caller.user_id());
        if !tx.update_field(&field, expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        // A rule, formula field or automation that needs the field keeps it
        // (ADR 0021, ADR 0022).
        if !still_working(&rules_before, tx.as_mut()).await? {
            return Err(ObjectError::Conflict("objects.field_in_use"));
        }
        tx.drop_field_index(field.id).await?;
        if field.searchable {
            // Its values must stop finding records (ADR 0020).
            let remaining = self.active_object(tx.as_mut(), api_name).await?;
            reindex(tx.as_mut(), &remaining).await?;
        }
        self.record_change(tx.as_mut(), context, Change::FieldDeleted(&object, &field))
            .await?;
        // Quick actions stop using the field; those that depend on it go
        // (ADR 0032 §4).
        self.quick_actions_after_field_deleted(tx.as_mut(), &object, &field, context)
            .await?;
        tx.commit().await?;
        Ok(field)
    }

    pub async fn object_permissions(
        &self,
        api_name: &str,
    ) -> Result<Vec<RolePermission>, ObjectError> {
        self.caller.require_manage()?;
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let permissions = tx.object_permissions(object.id).await?;
        tx.commit().await?;
        Ok(permissions)
    }

    /// Replaces one role's object permissions. The kernel verifies the role
    /// belongs to the workspace before calling.
    pub async fn set_object_permission(
        &self,
        api_name: &str,
        role_id: Uuid,
        access: AccessFlags,
        context: MutationContext,
    ) -> Result<RolePermission, ObjectError> {
        self.caller.require_manage()?;
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let permission = RolePermission {
            role_id,
            access: access.normalised(),
        };
        tx.set_object_permission(object.id, &permission, self.caller.user_id())
            .await?;
        self.record_change(
            tx.as_mut(),
            context,
            Change::PermissionsChanged(&object, role_id),
        )
        .await?;
        tx.commit().await?;
        Ok(permission)
    }

    // --- Records ----------------------------------------------------------

    /// The record's values restricted to the object's active fields.
    pub(crate) fn project(object: &ObjectDefinition, mut record: Record) -> Record {
        record.values.retain(|key, _| object.field(key).is_some());
        record
    }

    /// The visibility predicate for lists of `object`, or `None` when every
    /// record the caller's object access allows is visible. A detail
    /// object's records follow their primary master (ADR 0025).
    pub(crate) async fn visibility_of(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
    ) -> Result<Option<sharing::Visibility>, ObjectError> {
        let flags = access.for_object(object.id);
        let Some(field) = object.primary_master() else {
            return Ok(sharing::visibility(&self.caller, flags, object));
        };
        let master = tx
            .object_by_id(field.reference_object_id.ok_or(ObjectError::Invariant)?)
            .await?
            .ok_or(ObjectError::Invariant)?;
        Ok(sharing::detail_visibility(
            &self.caller,
            flags,
            field.id,
            access.for_object(master.id),
            &master,
        ))
    }

    /// What the caller may do with one record (ADR 0016); a detail record
    /// follows its primary master (ADR 0025).
    async fn rights_on(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
        record: &Record,
    ) -> Result<RecordRights, ObjectError> {
        let Some(field) = object.primary_master() else {
            return self.own_rights(tx, access, object, record).await;
        };
        let flags = access.for_object(object.id);
        if !flags.read {
            return Ok(RecordRights::default());
        }
        let master = match record.values.get(&field.api_name).and_then(Value::as_str) {
            Some(public_id) => tx.record_by_public_id(public_id).await?,
            None => None,
        };
        let master_rights = match master.filter(|master| master.deleted_at.is_none()) {
            Some(master) => match tx.object_by_id(master.object_id).await? {
                Some(master_object) => self.own_rights(tx, access, &master_object, &master).await?,
                None => RecordRights::default(),
            },
            None => RecordRights::default(),
        };
        Ok(sharing::detail_rights(flags, master_rights))
    }

    /// What the caller may do with one record from its owner, the object's
    /// default and manual shares (ADR 0016).
    async fn own_rights(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
        record: &Record,
    ) -> Result<RecordRights, ObjectError> {
        let flags = access.for_object(object.id);
        if !flags.read {
            return Ok(RecordRights::default());
        }
        let share = if flags.modify_all || sharing::owns(&self.caller, object, record.owner_id) {
            None
        } else {
            tx.share_access(record.id, self.caller.user_id(), self.caller.role_ids())
                .await?
        };
        Ok(sharing::rights(
            &self.caller,
            flags,
            object,
            record.owner_id,
            share,
        ))
    }

    /// Whether the caller can see a record of any object, for lookups and
    /// their names. `objects` caches definitions within one request.
    pub(crate) async fn can_see(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        objects: &mut BTreeMap<Uuid, Option<ObjectDefinition>>,
        record: &Record,
    ) -> Result<bool, ObjectError> {
        if record.deleted_at.is_some() || !access.for_object(record.object_id).read {
            return Ok(false);
        }
        let cached = match objects.get(&record.object_id) {
            Some(cached) => cached.clone(),
            None => {
                let loaded = tx
                    .object_by_id(record.object_id)
                    .await?
                    .filter(ObjectDefinition::is_active);
                objects.insert(record.object_id, loaded.clone());
                loaded
            }
        };
        let Some(object) = cached else {
            return Ok(false);
        };
        Ok(self.rights_on(tx, access, &object, record).await?.read)
    }

    /// Resolves the record's lookups to record ids. A lookup the member set
    /// must point to a record they can see; one set by an automation
    /// (`trusted`), or any lookup when an automation writes (`access` is
    /// `None`), only to an existing record.
    async fn resolve_references(
        &self,
        tx: &mut dyn ObjectTx,
        access: Option<&ObjectAccess>,
        object: &ObjectDefinition,
        values: &BTreeMap<String, Value>,
        previous: &BTreeMap<String, Value>,
        trusted: &BTreeSet<String>,
    ) -> Result<BTreeMap<Uuid, Uuid>, ObjectError> {
        let mut resolved = BTreeMap::new();
        let mut issues = Vec::new();
        let mut objects = BTreeMap::new();
        for field in object
            .fields
            .iter()
            .filter(|field| field.is_active() && field.field_type.holds_reference())
        {
            let master = field.field_type == FieldType::MasterDetail;
            let before = previous.get(&field.api_name).and_then(Value::as_str);
            let Some(public_id) = values.get(&field.api_name).and_then(Value::as_str) else {
                continue;
            };
            // A detail stays with the master it was created under.
            if master && before.is_some_and(|before| before != public_id) {
                issues.push(ValidationIssue::new(
                    field.api_name.clone(),
                    "records.master_locked",
                    "a detail cannot move to another master",
                ));
                continue;
            }
            let mut target = tx.record_by_public_id(public_id).await?;
            // A new detail holds its master until it commits, so a
            // concurrent delete either sees it and cascades or is seen
            // here as deleted (ADR 0025).
            // Exclusive, because saving a detail also updates its master's
            // roll-up summaries (ADR 0026); masters are locked before their
            // details, as a cascade from a master does.
            if let (true, Some(found)) = (master, &target) {
                let id = found.id;
                tx.lock_record(id, true).await?;
                target = tx.record_by_id(id).await?;
            }
            // A lookup the change leaves alone is kept as it is: an editor
            // who cannot read its target may still change other fields.
            if before == Some(public_id) {
                if let Some(target) = target {
                    resolved.insert(field.id, target.id);
                }
                continue;
            }
            // A target the caller cannot see is reported exactly like a
            // missing one.
            let related_to = field.field_type == FieldType::RelatedTo;
            let visible = match (&target, access) {
                (Some(target), _)
                    if !related_to && Some(target.object_id) != field.reference_object_id =>
                {
                    false
                }
                (Some(target), Some(access)) if !trusted.contains(&field.api_name) => {
                    self.can_see(tx, access, &mut objects, target).await?
                }
                // Automations skip member visibility, but never point at a
                // record of a suspended plugin object (ADR 0033 §2).
                (Some(target), _) => {
                    target.deleted_at.is_none()
                        && tx
                            .object_by_id(target.object_id)
                            .await?
                            .is_some_and(|object| object.is_active())
                }
                (None, _) => false,
            };
            // New values meet the field's lookup filter (ADR 0028). Like
            // visibility, the filter binds members, not automations.
            let filtered = match (&target, access, &field.lookup_filter) {
                (Some(target), Some(access), Some(filter))
                    if visible && !trusted.contains(&field.api_name) =>
                {
                    self.meets_lookup_filter(tx, access, field, filter, &target.public_id)
                        .await?
                }
                _ => true,
            };
            // Adding a detail is editing its master (ADR 0025).
            let editable = match (&target, access) {
                (Some(target), Some(access))
                    if visible && master && !trusted.contains(&field.api_name) =>
                {
                    match tx.object_by_id(target.object_id).await? {
                        Some(target_object) => {
                            self.own_rights(tx, access, &target_object, target)
                                .await?
                                .edit
                        }
                        None => false,
                    }
                }
                _ => true,
            };
            // A related-to value must point to an object its allowlist
            // names now; one taken out keeps the values it had (ADR 0033).
            // Only a target the writer sees is told apart from a missing one.
            let allowed = match &target {
                Some(target) if related_to => {
                    field.reference_object_ids.contains(&target.object_id)
                }
                _ => true,
            };
            match target {
                Some(_) if visible && !allowed => issues.push(ValidationIssue::new(
                    field.api_name.clone(),
                    "records.reference_object_not_allowed",
                    "choose a record of one of the objects this field allows",
                )),
                Some(_) if visible && !filtered => issues.push(ValidationIssue::new(
                    field.api_name.clone(),
                    "records.lookup_filter_mismatch",
                    "choose a record that meets this field's filter",
                )),
                Some(_) if visible && !editable => issues.push(ValidationIssue::new(
                    field.api_name.clone(),
                    "records.master_read_only",
                    "you can add details only to a record you can edit",
                )),
                Some(target) if visible => {
                    resolved.insert(field.id, target.id);
                }
                _ => issues.push(ValidationIssue::new(
                    field.api_name.clone(),
                    "records.reference_not_found",
                    "choose an existing record",
                )),
            }
        }
        if issues.is_empty() {
            Ok(resolved)
        } else {
            Err(ObjectError::Validation(issues))
        }
    }

    pub async fn create_record(
        &self,
        api_name: &str,
        body: &Map<String, Value>,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        let mut run = Run::default();
        let result = self
            .create_record_in(api_name, body, context, &mut run)
            .await;
        self.keep_failed_runs(run, result).await
    }

    async fn create_record_in(
        &self,
        api_name: &str,
        body: &Map<String, Value>,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let (object, record) = self
            .create_record_tx(tx.as_mut(), &access, api_name, body, context, run)
            .await?;
        tx.commit().await?;
        self.with_formula(&object, record).await
    }

    /// The create record step inside a caller's transaction, for
    /// requests that make several changes at once.
    pub(crate) async fn create_record_tx(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        api_name: &str,
        body: &Map<String, Value>,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<(ObjectDefinition, Record), ObjectError> {
        let object = self.active_object(tx, api_name).await?;
        let flags = access.for_object(object.id);
        if !flags.read {
            return Err(ObjectError::NotFound);
        }
        if !flags.create {
            return Err(ObjectError::PermissionDenied);
        }
        let Some(id) = body.get("id") else {
            let record = self
                .insert_new(tx, Some(access), &object, body, context, run, None)
                .await?;
            return Ok((object, record));
        };
        // A device assigned the id (ADR 0031).
        let id = id
            .as_str()
            .and_then(|id| platform_id::PublicId::parse(id).ok())
            .filter(|id| id.prefix() == object.key_prefix && id.is_client_assigned())
            .ok_or_else(|| {
                ObjectError::invalid(
                    "id",
                    "records.id_invalid",
                    "an assigned id must be in the device range for this object",
                )
            })?;
        // A create and its retry may overlap: the second waits here until the
        // first commits, then finds its record.
        tx.lock_object(object.id, true).await?;
        if let Some(existing) = tx.record_by_public_id(id.as_str()).await? {
            // A device that retries a create it already pushed gets the
            // record back; anyone else's record keeps the id.
            if existing.object_id == object.id && existing.created_by == Some(self.caller.user_id())
            {
                let rights = self.rights_on(tx, access, &object, &existing).await?;
                if rights.read {
                    return Ok((object, existing));
                }
            }
            return Err(ObjectError::Conflict("records.id_taken"));
        }
        let mut values = body.clone();
        values.remove("id");
        let record = self
            .insert_new(
                tx,
                Some(access),
                &object,
                &values,
                context,
                run,
                Some(id.as_str().to_owned()),
            )
            .await?;
        Ok((object, record))
    }

    /// Writes a new record with everything a save does: before-save
    /// automations, lookups, validation rules, indexes, evidence, history
    /// and after-save automations (ADR 0022). `access` is the member's, or
    /// `None` when an automation writes in system context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn insert_new(
        &self,
        tx: &mut dyn ObjectTx,
        access: Option<&ObjectAccess>,
        object: &ObjectDefinition,
        body: &Map<String, Value>,
        context: MutationContext,
        run: &mut Run,
        public_id: Option<String>,
    ) -> Result<Record, ObjectError> {
        // A master-detail field committed since `object` was read would
        // leave this record without a master (ADR 0025), and a roll-up
        // summary without its starting value (ADR 0026).
        tx.lock_object(object.id, false).await?;
        let guarded = object
            .fields
            .iter()
            .filter(|field| {
                field.is_active()
                    && matches!(
                        field.field_type,
                        FieldType::MasterDetail | FieldType::Rollup
                    )
            })
            .count();
        if tx.guarded_field_count(object.id).await? != guarded {
            return Err(ObjectError::Conflict("records.definition_changed"));
        }
        let change =
            apply_values(object, &BTreeMap::new(), body, true).map_err(ObjectError::Validation)?;
        let public_id = match public_id {
            Some(public_id) => public_id,
            None if self.client_ids => {
                // The random tail of a version 7 UUID.
                let random = Uuid::now_v7().as_u128() as u64;
                platform_id::PublicId::new_client(&object.key_prefix, random)
                    .map_err(|_| ObjectError::Invariant)?
                    .as_str()
                    .to_owned()
            }
            None => {
                let sequence = tx.next_record_sequence(object.id).await?;
                platform_id::PublicId::new(&object.key_prefix, sequence)
                    .map_err(|_| ObjectError::Invariant)?
                    .as_str()
                    .to_owned()
            }
        };
        let now = Utc::now();
        let actor = Some(self.caller.user_id());
        let mut record = Record {
            id: Uuid::now_v7(),
            object_id: object.id,
            public_id,
            name: change.name.ok_or(ObjectError::Invariant)?,
            owner_id: actor,
            record_type_id: None,
            values: change.values,
            version: 1,
            created_at: now,
            created_by: actor,
            updated_at: now,
            updated_by: actor,
            touched_at: now,
            archived_at: None,
            deleted_at: None,
        };
        // A new master has no details yet: its counts and sums are zero.
        for field in rollup_fields(object) {
            if let Some(value) = rollup_json(field, None) {
                record.values.insert(field.api_name.clone(), value);
            }
        }
        let mut changed = change.changed;
        let trusted = self
            .before_save(tx, object, None, &mut record, &mut changed, context, run)
            .await?;
        let references = self
            .resolve_references(
                tx,
                access,
                object,
                &record.values,
                &BTreeMap::new(),
                &trusted,
            )
            .await?;
        self.enforce_rules(tx, object, None, &record).await?;
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
        changed.retain(|field| field != "name");
        changed.sort();
        changed.dedup();
        changed.insert(0, "name".into());
        self.record_change(
            tx,
            context,
            Change::RecordCreated(object, &record, &changed),
        )
        .await?;
        self.track(tx, object, None, &record, HistoryKind::Created, context)
            .await?;
        self.refresh_rollups(tx, object, &record, None, context)
            .await?;
        self.after_save(tx, object, SaveEvent::Create, None, &record, context, run)
            .await?;
        Ok(record)
    }

    /// Names of the records the given records' lookups point to, keyed by the
    /// target's public id, so callers can show a name instead of an id. A
    /// target the caller cannot read, or one in the recycle bin, is left
    /// out, exactly as if it were missing.
    pub async fn related_names(
        &self,
        api_name: &str,
        records: &[Record],
    ) -> Result<BTreeMap<String, String>, ObjectError> {
        Ok(self
            .related_targets(api_name, records)
            .await?
            .into_iter()
            .map(|(id, target)| (id, target.name))
            .collect())
    }

    /// The records the given records' lookup, master-detail and related-to
    /// fields point to, keyed by the target's public id: each one's name and
    /// object, so callers can show and link to it. A target the caller
    /// cannot read, or one in the recycle bin, is left out, exactly as if it
    /// were missing.
    pub async fn related_targets(
        &self,
        api_name: &str,
        records: &[Record],
    ) -> Result<BTreeMap<String, RelatedTarget>, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let lookups: Vec<&FieldDefinition> = object
            .fields
            .iter()
            .filter(|field| field.is_active() && field.field_type.holds_reference())
            .collect();
        let mut names = BTreeMap::new();
        let mut objects = BTreeMap::new();
        for record in records {
            for field in &lookups {
                let Some(public_id) = record.values.get(&field.api_name).and_then(Value::as_str)
                else {
                    continue;
                };
                if names.contains_key(public_id) {
                    continue;
                }
                let Some(target) = tx.record_by_public_id(public_id).await? else {
                    continue;
                };
                if !self
                    .can_see(tx.as_mut(), &access, &mut objects, &target)
                    .await?
                {
                    continue;
                }
                // `can_see` cached the target's active object.
                let Some(Some(target_object)) = objects.get(&target.object_id) else {
                    continue;
                };
                names.insert(
                    public_id.to_owned(),
                    RelatedTarget {
                        name: target.name,
                        object: target_object.api_name.clone(),
                    },
                );
            }
        }
        tx.commit().await?;
        Ok(names)
    }

    pub async fn get_record(
        &self,
        api_name: &str,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Record, ObjectError> {
        Ok(self
            .get_record_with_rights(api_name, public_id, include_deleted)
            .await?
            .0)
    }

    /// A record and what the caller may do with it. A record the caller
    /// cannot see is missing.
    pub async fn get_record_with_rights(
        &self,
        api_name: &str,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<(Record, RecordRights), ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if !access.for_object(object.id).read {
            return Err(ObjectError::NotFound);
        }
        let record = tx
            .record(object.id, public_id)
            .await?
            .filter(|record| include_deleted || record.deleted_at.is_none())
            .ok_or(ObjectError::NotFound)?;
        let rights = self
            .rights_on(tx.as_mut(), &access, &object, &record)
            .await?;
        if !rights.read {
            return Err(ObjectError::NotFound);
        }
        tx.commit().await?;
        Ok((
            self.with_formula(&object, Self::project(&object, record))
                .await?,
            rights,
        ))
    }

    /// Applies a partial change: `name` and field API names, `null` clears.
    pub async fn update_record(
        &self,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        body: &Map<String, Value>,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        let mut run = Run::default();
        let result = self
            .update_record_in(
                api_name,
                public_id,
                expected_version,
                body,
                context,
                &mut run,
            )
            .await;
        self.keep_failed_runs(run, result).await
    }

    async fn update_record_in(
        &self,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        body: &Map<String, Value>,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let (object, record) = self
            .update_record_tx(
                tx.as_mut(),
                &access,
                api_name,
                public_id,
                expected_version,
                body,
                context,
                run,
            )
            .await?;
        tx.commit().await?;
        self.with_formula(&object, Self::project(&object, record))
            .await
    }

    /// The update record step inside a caller's transaction, for
    /// requests that make several changes at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_record_tx(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        body: &Map<String, Value>,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<(ObjectDefinition, Record), ObjectError> {
        let object = self.active_object(tx, api_name).await?;
        let flags = access.for_object(object.id);
        if !flags.read {
            return Err(ObjectError::NotFound);
        }
        if !flags.edit {
            return Err(ObjectError::PermissionDenied);
        }
        let current = tx
            .record(object.id, public_id)
            .await?
            .filter(|record| record.deleted_at.is_none())
            .ok_or(ObjectError::NotFound)?;
        let rights = self.rights_on(tx, access, &object, &current).await?;
        if !rights.read {
            return Err(ObjectError::NotFound);
        }
        if !rights.edit {
            return Err(ObjectError::PermissionDenied);
        }
        if current.version != expected_version {
            return Err(ObjectError::VersionConflict);
        }
        let record = self
            .update_existing(tx, Some(access), &object, current, body, context, run)
            .await?;
        Ok((object, record))
    }

    /// Writes a change to a stored record with everything a save does, like
    /// [`Self::insert_new`]. An automation's change that changes nothing is
    /// not written, which also ends automations that update each other.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn update_existing(
        &self,
        tx: &mut dyn ObjectTx,
        access: Option<&ObjectAccess>,
        object: &ObjectDefinition,
        current: Record,
        body: &Map<String, Value>,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let change =
            apply_values(object, &current.values, body, false).map_err(ObjectError::Validation)?;
        let mut changed = change.changed;
        let mut record = current.clone();
        if let Some(name) = change.name {
            if name != record.name {
                changed.push("name".into());
            }
            record.name = name;
        }
        record.values = change.values;
        if access.is_none() && changed.is_empty() {
            return Ok(current);
        }
        self.stamp(&mut record);
        let trusted = self
            .before_save(
                tx,
                object,
                Some(&current),
                &mut record,
                &mut changed,
                context,
                run,
            )
            .await?;
        let references = self
            .resolve_references(
                tx,
                access,
                object,
                &record.values,
                &current.values,
                &trusted,
            )
            .await?;
        self.enforce_rules(tx, object, Some(&current), &record)
            .await?;
        if !tx.update_record(&record, current.version).await? {
            return Err(ObjectError::VersionConflict);
        }
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
        let named = changed.iter().any(|field| field == "name");
        changed.retain(|field| field != "name");
        changed.sort();
        changed.dedup();
        if named {
            changed.insert(0, "name".into());
        }
        self.record_change(
            tx,
            context,
            Change::RecordUpdated(object, &record, &changed),
        )
        .await?;
        self.track(
            tx,
            object,
            Some(&current),
            &record,
            HistoryKind::Changed,
            context,
        )
        .await?;
        self.refresh_rollups(tx, object, &record, None, context)
            .await?;
        self.after_save(
            tx,
            object,
            SaveEvent::Update,
            Some(&current),
            &record,
            context,
            run,
        )
        .await?;
        Ok(record)
    }

    /// Deleting and undeleting need ownership, the hierarchy above the
    /// owner, or modify all; an invisible record is missing.
    async fn require_delete(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
        record: &Record,
    ) -> Result<(), ObjectError> {
        let rights = self.rights_on(tx, access, object, record).await?;
        if !rights.read {
            Err(ObjectError::NotFound)
        } else if !rights.delete {
            Err(ObjectError::PermissionDenied)
        } else {
            Ok(())
        }
    }

    /// Writes the history a change leaves (ADR 0017). Nothing is kept unless
    /// the object tracks history; then the lifecycle, name and owner are
    /// kept, and the values of fields that track history.
    /// In a replica, remembers the server's value of each field the member
    /// changes for the first time since the replica took the server's copy
    /// (ADR 0024 §3), so a later conflict can merge three ways. Records made
    /// on the device have no server copy and need none.
    async fn remember_bases(
        &self,
        tx: &mut dyn ObjectTx,
        before: &Record,
        after: &Record,
    ) -> Result<(), ObjectError> {
        if !self.is_replica() || tx.remote_version(before.id).await?.is_none() {
            return Ok(());
        }
        for (field, value) in crate::replica::changed_fields(before, after) {
            tx.remember_base(before.id, &field, &value).await?;
        }
        Ok(())
    }

    async fn track(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        before: Option<&Record>,
        after: &Record,
        kind: HistoryKind,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        if let Some(before) = before {
            self.remember_bases(tx, before, after).await?;
        }
        if !object.track_history {
            return Ok(());
        }
        let entry = |field: Option<String>,
                     old_value: Option<Value>,
                     new_value: Option<Value>,
                     kind| HistoryEntry {
            id: Uuid::now_v7(),
            record_id: after.id,
            object_id: object.id,
            kind,
            field,
            old_value,
            new_value,
            changed_at: after.updated_at,
            changed_by: Some(self.caller.user_id()),
            correlation_id: Some(context.correlation_id),
            source: self.source(context),
        };
        let mut entries = Vec::new();
        match (kind, before) {
            (HistoryKind::Changed, Some(before)) => {
                if before.name != after.name {
                    entries.push(entry(
                        Some("name".into()),
                        Some(Value::String(before.name.clone())),
                        Some(Value::String(after.name.clone())),
                        HistoryKind::Changed,
                    ));
                }
                if before.owner_id != after.owner_id {
                    let id =
                        |owner: Option<Uuid>| owner.map(|owner| Value::String(owner.to_string()));
                    entries.push(entry(
                        Some("owner".into()),
                        id(before.owner_id),
                        id(after.owner_id),
                        HistoryKind::Changed,
                    ));
                }
                for field in object
                    .fields
                    .iter()
                    .filter(|field| field.is_active() && field.track_history)
                {
                    let old_value = before.values.get(&field.api_name);
                    let new_value = after.values.get(&field.api_name);
                    if old_value != new_value {
                        entries.push(entry(
                            Some(field.api_name.clone()),
                            old_value.cloned(),
                            new_value.cloned(),
                            HistoryKind::Changed,
                        ));
                    }
                }
            }
            (HistoryKind::Changed, None) => return Err(ObjectError::Invariant),
            (kind, _) => entries.push(entry(None, None, None, kind)),
        }
        tx.append_history(&entries).await
    }

    /// A member's edit: the next version and every stamp (ADR 0014).
    fn stamp(&self, record: &mut Record) {
        let now = Utc::now();
        record.version += 1;
        record.updated_at = now;
        record.updated_by = Some(self.caller.user_id());
        record.touched_at = now;
    }

    /// Moves a record to the recycle bin. Lookups that restrict deletion
    /// block it; lookups that clear lose their value on the referencing
    /// records, each as a member's edit with its own event.
    pub async fn delete_record(
        &self,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        let mut run = Run::default();
        let result = self
            .delete_record_in(api_name, public_id, expected_version, context, &mut run)
            .await;
        self.keep_failed_runs(run, result).await
    }

    async fn delete_record_in(
        &self,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let (object, record) = self
            .delete_record_tx(
                tx.as_mut(),
                &access,
                api_name,
                public_id,
                expected_version,
                context,
                run,
            )
            .await?;
        tx.commit().await?;
        self.with_formula(&object, Self::project(&object, record))
            .await
    }

    /// The delete record step inside a caller's transaction, for
    /// requests that make several changes at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn delete_record_tx(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<(ObjectDefinition, Record), ObjectError> {
        let object = self.active_object(tx, api_name).await?;
        let flags = access.for_object(object.id);
        if !flags.read {
            return Err(ObjectError::NotFound);
        }
        if !flags.delete {
            return Err(ObjectError::PermissionDenied);
        }
        let mut record = tx
            .record(object.id, public_id)
            .await?
            .filter(|record| record.deleted_at.is_none())
            .ok_or(ObjectError::NotFound)?;
        self.require_delete(tx, access, &object, &record).await?;
        if record.version != expected_version {
            return Err(ObjectError::VersionConflict);
        }
        // Masters first, then the record, as every save of a detail does.
        lock_masters(tx, &object, &record).await?;
        // Holds back new details until this delete commits (ADR 0025).
        tx.lock_record(record.id, true).await?;
        let details = self.release_references(tx, &record, context).await?;
        for (detail_object, detail) in details {
            self.trash_detail(tx, &detail_object, detail, record.id, context)
                .await?;
        }
        let before_delete = record.clone();
        self.stamp(&mut record);
        record.deleted_at = Some(record.updated_at);
        if !tx.update_record(&record, expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        // A record in the recycle bin releases its unique values.
        let references = current_references(tx, &object, &record).await?;
        tx.replace_index(
            record.id,
            &stored_index(
                &object,
                &record,
                &references,
                &crate::formula_fields::stored_formulas(&object),
            ),
        )
        .await?;
        self.record_change(tx, context, Change::RecordDeleted(&object, &record))
            .await?;
        self.track(tx, &object, None, &record, HistoryKind::Deleted, context)
            .await?;
        self.refresh_rollups(tx, &object, &record, None, context)
            .await?;
        self.after_save(
            tx,
            &object,
            SaveEvent::Delete,
            Some(&before_delete),
            &record,
            context,
            run,
        )
        .await?;
        Ok((object, record))
    }

    /// Recomputes the roll-up summaries of the masters `record` points to
    /// through master-detail fields, except `skip` (ADR 0026). Each master
    /// is locked and read again first, so concurrent details all count.
    async fn refresh_rollups(
        &self,
        tx: &mut dyn ObjectTx,
        detail: &ObjectDefinition,
        record: &Record,
        skip: Option<Uuid>,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        for relationship in detail.masters() {
            let Some(master_object) = (match relationship.reference_object_id {
                Some(id) => tx.object_by_id(id).await?,
                None => None,
            }) else {
                continue;
            };
            let fields: Vec<&FieldDefinition> = rollup_fields(&master_object)
                .into_iter()
                .filter(|field| {
                    field
                        .rollup
                        .as_ref()
                        .is_some_and(|rollup| rollup.relationship_id == relationship.id)
                })
                .collect();
            if fields.is_empty() {
                continue;
            }
            let Some(public_id) = record
                .values
                .get(&relationship.api_name)
                .and_then(Value::as_str)
            else {
                continue;
            };
            let Some(master) = tx.record_by_public_id(public_id).await? else {
                continue;
            };
            if Some(master.id) == skip {
                continue;
            }
            self.recompute_rollups(tx, &master_object, master.id, &fields, context)
                .await?;
        }
        Ok(())
    }

    /// Writes the current values of `fields`, roll-up summaries of
    /// `object`, on one of its records: a system update of the master with
    /// the next version, its own change event and fresh index rows. A
    /// record whose values are already current, or in the recycle bin, is
    /// left alone.
    async fn recompute_rollups(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        record_id: Uuid,
        fields: &[&FieldDefinition],
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        tx.lock_record(record_id, true).await?;
        let Some(current) = tx.record_by_id(record_id).await? else {
            return Ok(());
        };
        if current.deleted_at.is_some() {
            return Ok(());
        }
        let mut record = current.clone();
        let mut changed = Vec::new();
        for field in fields {
            let Some(rollup) = &field.rollup else {
                continue;
            };
            let time = matches!(field.value_type(), FieldType::Date | FieldType::DateTime);
            let value = tx
                .rollup_value(&crate::RollupQuery {
                    relationship_id: rollup.relationship_id,
                    master_id: record.id,
                    function: rollup.function,
                    field: rollup.field_id.map(|id| (id, time)),
                })
                .await?;
            let value = rollup_json(field, value);
            if record.values.get(&field.api_name) != value.as_ref() {
                match value {
                    Some(value) => record.values.insert(field.api_name.clone(), value),
                    None => record.values.remove(&field.api_name),
                };
                changed.push(field.api_name.clone());
            }
        }
        if changed.is_empty() {
            return Ok(());
        }
        self.stamp(&mut record);
        if !tx.update_record(&record, current.version).await? {
            return Err(ObjectError::VersionConflict);
        }
        let references = current_references(tx, object, &record).await?;
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
        changed.sort();
        self.record_change(
            tx,
            context,
            Change::RecordUpdated(object, &record, &changed),
        )
        .await
    }

    /// Lets go of the records that point to `record` before it goes to the
    /// recycle bin: lookups that restrict deletion block it, lookups that
    /// clear lose their value, each as a member's edit with its own event.
    /// Returns the details whose master `record` is, to delete with it
    /// (ADR 0025).
    async fn release_references(
        &self,
        tx: &mut dyn ObjectTx,
        record: &Record,
        context: MutationContext,
    ) -> Result<Vec<(ObjectDefinition, Record)>, ObjectError> {
        let referencing = tx.referencing(record.id).await?;
        let mut owners: BTreeMap<Uuid, ObjectDefinition> = BTreeMap::new();
        let mut clears = Vec::new();
        let mut details = Vec::new();
        for (field_id, child_id) in referencing {
            let Some(child) = tx.record_by_id(child_id).await? else {
                continue;
            };
            if !owners.contains_key(&child.object_id) {
                if let Some(owner) = tx.object_by_id(child.object_id).await? {
                    owners.insert(owner.id, owner);
                }
            }
            let Some(owner) = owners.get(&child.object_id) else {
                continue;
            };
            let Some(field) = owner.fields.iter().find(|field| field.id == field_id) else {
                continue;
            };
            if field.field_type == FieldType::MasterDetail {
                details.push(child);
                continue;
            }
            if field.delete_behaviour == Some(crate::DeleteBehaviour::Restrict) {
                return Err(ObjectError::Conflict("records.delete_restricted"));
            }
            clears.push((child, field.api_name.clone()));
        }
        for (child, field_api) in clears {
            let owner = owners.get(&child.object_id).ok_or(ObjectError::Invariant)?;
            self.clear_reference(tx, owner, child, field_api, context)
                .await?;
        }
        let mut found = Vec::new();
        for detail in details {
            // A junction record reached through both masters is deleted once.
            if found
                .iter()
                .any(|(_, seen): &(ObjectDefinition, Record)| seen.id == detail.id)
            {
                continue;
            }
            let owner = owners
                .get(&detail.object_id)
                .ok_or(ObjectError::Invariant)?
                .clone();
            found.push((owner, detail));
        }
        Ok(found)
    }

    /// Clears one reference field of `child` whose target is going, as a
    /// member's edit with its own event and history.
    async fn clear_reference(
        &self,
        tx: &mut dyn ObjectTx,
        owner: &ObjectDefinition,
        mut child: Record,
        field_api: String,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let child_before = child.clone();
        let child_version = child.version;
        child.values.remove(&field_api);
        self.stamp(&mut child);
        if !tx.update_record(&child, child_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        let child_references = current_references(tx, owner, &child).await?;
        tx.replace_index(
            child.id,
            &stored_index(
                owner,
                &child,
                &child_references,
                &crate::formula_fields::stored_formulas(owner),
            ),
        )
        .await?;
        let changed = [field_api];
        self.record_change(tx, context, Change::RecordUpdated(owner, &child, &changed))
            .await?;
        self.track(
            tx,
            owner,
            Some(&child_before),
            &child,
            HistoryKind::Changed,
            context,
        )
        .await
    }

    /// Takes a deleted object out of a related-to field's allowlist, and
    /// clears the field's values that point to the object's records, as
    /// deleting each target would (ADR 0033 §1).
    async fn release_allowlist(
        &self,
        tx: &mut dyn ObjectTx,
        field: &FieldDefinition,
        deleted: &ObjectDefinition,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut owner = tx
            .object_by_id(field.object_id)
            .await?
            .ok_or(ObjectError::Invariant)?;
        if field.reference_object_ids.contains(&deleted.id) {
            self.drop_from_allowlist(tx, &owner, field, deleted, context)
                .await?;
            owner = tx
                .object_by_id(owner.id)
                .await?
                .ok_or(ObjectError::Invariant)?;
        }
        for id in tx.referencing_object_records(field.id, deleted.id).await? {
            let Some(child) = tx.record_by_id(id).await? else {
                continue;
            };
            self.clear_reference(tx, &owner, child, field.api_name.clone(), context)
                .await?;
        }
        Ok(())
    }

    /// Takes a deleted object out of a related-to field's allowlist, as a
    /// field update with its own event.
    async fn drop_from_allowlist(
        &self,
        tx: &mut dyn ObjectTx,
        owner: &ObjectDefinition,
        field: &FieldDefinition,
        deleted: &ObjectDefinition,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut changed = field.clone();
        let expected_version = changed.version;
        let kept: Vec<(Uuid, String)> = field
            .reference_object_ids
            .iter()
            .copied()
            .zip(field.reference_objects.iter().cloned())
            .filter(|(id, _)| *id != deleted.id)
            .collect();
        changed.reference_object_ids = kept.iter().map(|(id, _)| *id).collect();
        changed.reference_objects = kept.into_iter().map(|(_, name)| name).collect();
        changed.version += 1;
        changed.updated_at = Utc::now();
        changed.updated_by = Some(self.caller.user_id());
        if !tx.update_field(&changed, expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(tx, context, Change::FieldUpdated(owner, &changed))
            .await?;
        Ok(())
    }

    /// Takes a detail to the recycle bin with its master `root`: no
    /// automations run, as for any cascade, but the detail leaves its own
    /// event and history, and restoring `root` brings it back.
    async fn trash_detail(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        record: Record,
        root: Uuid,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        // Masters are never details, so a detail has no details of its own.
        if !self
            .release_references(tx, &record, context)
            .await?
            .is_empty()
        {
            return Err(ObjectError::Invariant);
        }
        // Releasing may have cleared one of the detail's own lookups.
        let mut record = tx
            .record_by_id(record.id)
            .await?
            .ok_or(ObjectError::Invariant)?;
        let version = record.version;
        self.stamp(&mut record);
        record.deleted_at = Some(record.updated_at);
        if !tx.update_record(&record, version).await? {
            return Err(ObjectError::VersionConflict);
        }
        tx.set_deleted_with(record.id, Some(root)).await?;
        let references = current_references(tx, object, &record).await?;
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
        self.record_change(tx, context, Change::RecordDeleted(object, &record))
            .await?;
        self.track(tx, object, None, &record, HistoryKind::Deleted, context)
            .await?;
        // A junction record's other master loses it; `root` is going too.
        self.refresh_rollups(tx, object, &record, Some(root), context)
            .await
    }

    /// Restores a record from the recycle bin. Undeleting needs the same
    /// permission as deleting, as in Salesforce.
    pub async fn undelete_record(
        &self,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        let mut run = Run::default();
        let result = self
            .undelete_record_in(api_name, public_id, expected_version, context, &mut run)
            .await;
        self.keep_failed_runs(run, result).await
    }

    async fn undelete_record_in(
        &self,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let (object, record) = self
            .undelete_record_tx(
                tx.as_mut(),
                &access,
                api_name,
                public_id,
                expected_version,
                context,
                run,
            )
            .await?;
        tx.commit().await?;
        self.with_formula(&object, Self::project(&object, record))
            .await
    }

    /// The undelete record step inside a caller's transaction, for
    /// requests that make several changes at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn undelete_record_tx(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        api_name: &str,
        public_id: &str,
        expected_version: i64,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<(ObjectDefinition, Record), ObjectError> {
        let object = self.active_object(tx, api_name).await?;
        let flags = access.for_object(object.id);
        if !flags.read {
            return Err(ObjectError::NotFound);
        }
        if !flags.delete {
            return Err(ObjectError::PermissionDenied);
        }
        let mut record = tx
            .record(object.id, public_id)
            .await?
            .filter(|record| record.deleted_at.is_some())
            .ok_or(ObjectError::NotFound)?;
        self.require_delete(tx, access, &object, &record).await?;
        if record.version != expected_version {
            return Err(ObjectError::VersionConflict);
        }
        lock_masters(tx, &object, &record).await?;
        if !masters_live(tx, &object, &record).await? {
            return Err(ObjectError::Conflict("records.master_deleted"));
        }
        tx.set_deleted_with(record.id, None).await?;
        let before_restore = record.clone();
        self.stamp(&mut record);
        record.deleted_at = None;
        if !tx.update_record(&record, expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        // Reclaiming unique values fails with `records.duplicate_value` when
        // another record took one while this one was in the bin.
        let references = current_references(tx, &object, &record).await?;
        tx.replace_index(
            record.id,
            &stored_index(
                &object,
                &record,
                &references,
                &crate::formula_fields::stored_formulas(&object),
            ),
        )
        .await?;
        self.record_change(tx, context, Change::RecordUndeleted(&object, &record))
            .await?;
        self.track(tx, &object, None, &record, HistoryKind::Undeleted, context)
            .await?;
        self.restore_details(tx, record.id, context).await?;
        self.refresh_rollups(tx, &object, &record, None, context)
            .await?;
        // Its own roll-ups count what came back with it.
        let own: Vec<&FieldDefinition> = rollup_fields(&object);
        if !own.is_empty() {
            self.recompute_rollups(tx, &object, record.id, &own, context)
                .await?;
            record = tx
                .record_by_id(record.id)
                .await?
                .ok_or(ObjectError::Invariant)?;
        }
        self.after_save(
            tx,
            &object,
            SaveEvent::Undelete,
            Some(&before_restore),
            &record,
            context,
            run,
        )
        .await?;
        Ok((object, record))
    }

    /// Brings back the details deleted with `root` (ADR 0025). A junction
    /// record whose other master is still in the recycle bin stays there,
    /// and comes back when that master does.
    async fn restore_details(
        &self,
        tx: &mut dyn ObjectTx,
        root: Uuid,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut objects: BTreeMap<Uuid, ObjectDefinition> = BTreeMap::new();
        for id in tx.deleted_with(root).await? {
            let Some(mut record) = tx.record_by_id(id).await? else {
                continue;
            };
            if !objects.contains_key(&record.object_id) {
                let object = tx
                    .object_by_id(record.object_id)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                objects.insert(object.id, object);
            }
            let object = objects
                .get(&record.object_id)
                .ok_or(ObjectError::Invariant)?;
            if !object.is_active() {
                continue;
            }
            // A junction record waits for its other master instead.
            if let Some(waiting_on) = deleted_master(tx, object, &record).await? {
                tx.set_deleted_with(record.id, Some(waiting_on)).await?;
                continue;
            }
            if !masters_live(tx, object, &record).await? {
                continue;
            }
            let version = record.version;
            self.stamp(&mut record);
            record.deleted_at = None;
            if !tx.update_record(&record, version).await? {
                return Err(ObjectError::VersionConflict);
            }
            tx.set_deleted_with(record.id, None).await?;
            let references = current_references(tx, object, &record).await?;
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
            self.record_change(tx, context, Change::RecordUndeleted(object, &record))
                .await?;
            self.track(tx, object, None, &record, HistoryKind::Undeleted, context)
                .await?;
            // A junction record's other master counts it again; `root`
            // counts it once it is back itself.
            let object = object.clone();
            self.refresh_rollups(tx, &object, &record, Some(root), context)
                .await?;
        }
        Ok(())
    }

    /// The record's manual shares and what the caller may do with it.
    /// Seeing the shares needs the right to share (ADR 0016).
    pub async fn record_sharing(
        &self,
        api_name: &str,
        public_id: &str,
    ) -> Result<(Record, RecordRights, Vec<RecordShare>), ObjectError> {
        let mut tx = self.begin().await?;
        let (object, record, rights) = self
            .visible_record(tx.as_mut(), api_name, public_id)
            .await?;
        if !rights.share {
            return Err(ObjectError::PermissionDenied);
        }
        let shares = tx.record_shares(record.id).await?;
        tx.commit().await?;
        Ok((
            self.with_formula(&object, Self::project(&object, record))
                .await?,
            rights,
            shares,
        ))
    }

    /// Grants `access` on one record to a member or a role, or changes an
    /// existing share. The kernel checks that the grantee exists in the
    /// workspace before calling.
    pub async fn share_record(
        &self,
        api_name: &str,
        public_id: &str,
        grantee_kind: GranteeKind,
        grantee_id: Uuid,
        access: ShareAccess,
        context: MutationContext,
    ) -> Result<RecordShare, ObjectError> {
        let mut tx = self.begin().await?;
        let (object, record, rights) = self
            .visible_record(tx.as_mut(), api_name, public_id)
            .await?;
        if !rights.share {
            return Err(ObjectError::PermissionDenied);
        }
        if grantee_kind == GranteeKind::User && record.owner_id == Some(grantee_id) {
            return Err(ObjectError::invalid(
                "grantee_id",
                "records.share_owner",
                "the owner already has full access",
            ));
        }
        let share = RecordShare {
            grantee_kind,
            grantee_id,
            access,
            created_at: Utc::now(),
            created_by: Some(self.caller.user_id()),
        };
        tx.put_share(record.id, &share).await?;
        self.record_change(
            tx.as_mut(),
            context,
            Change::SharingChanged(&object, &record, grantee_kind, grantee_id, Some(access)),
        )
        .await?;
        tx.commit().await?;
        Ok(share)
    }

    /// Revokes one manual share.
    pub async fn unshare_record(
        &self,
        api_name: &str,
        public_id: &str,
        grantee_kind: GranteeKind,
        grantee_id: Uuid,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut tx = self.begin().await?;
        let (object, record, rights) = self
            .visible_record(tx.as_mut(), api_name, public_id)
            .await?;
        if !rights.share {
            return Err(ObjectError::PermissionDenied);
        }
        if !tx.delete_share(record.id, grantee_kind, grantee_id).await? {
            return Err(ObjectError::NotFound);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::SharingChanged(&object, &record, grantee_kind, grantee_id, None),
        )
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Transfers a record to another member. Manual shares go with the
    /// previous owner, as in Salesforce. The kernel checks that the new
    /// owner is a member of the workspace before calling.
    pub async fn change_owner(
        &self,
        api_name: &str,
        public_id: &str,
        owner_id: Uuid,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<Record, ObjectError> {
        let mut run = Run::default();
        let result = self
            .change_owner_in(
                api_name,
                public_id,
                owner_id,
                expected_version,
                context,
                &mut run,
            )
            .await;
        self.keep_failed_runs(run, result).await
    }

    async fn change_owner_in(
        &self,
        api_name: &str,
        public_id: &str,
        owner_id: Uuid,
        expected_version: i64,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Record, ObjectError> {
        let mut tx = self.begin().await?;
        let (object, record) = self
            .change_owner_tx(
                tx.as_mut(),
                api_name,
                public_id,
                owner_id,
                expected_version,
                context,
                run,
            )
            .await?;
        tx.commit().await?;
        self.with_formula(&object, Self::project(&object, record))
            .await
    }

    /// The owner change step inside a caller's transaction, for requests
    /// that make several changes at once.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn change_owner_tx(
        &self,
        tx: &mut dyn ObjectTx,
        api_name: &str,
        public_id: &str,
        owner_id: Uuid,
        expected_version: i64,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<(ObjectDefinition, Record), ObjectError> {
        let (object, mut record, rights) = self.visible_record(tx, api_name, public_id).await?;
        if !rights.transfer {
            return Err(ObjectError::PermissionDenied);
        }
        if record.version != expected_version {
            return Err(ObjectError::VersionConflict);
        }
        let previous = record.owner_id;
        if previous == Some(owner_id) {
            return Ok((object, record));
        }
        let before = record.clone();
        record.owner_id = Some(owner_id);
        self.stamp(&mut record);
        // An owner change is an update for automations (ADR 0022 §5).
        let mut changed = Vec::new();
        let trusted = self
            .before_save(
                tx,
                &object,
                Some(&before),
                &mut record,
                &mut changed,
                context,
                run,
            )
            .await?;
        self.enforce_rules(tx, &object, Some(&before), &record)
            .await?;
        if !tx.update_record(&record, expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        if !changed.is_empty() {
            let references = self
                .resolve_references(tx, None, &object, &record.values, &before.values, &trusted)
                .await?;
            tx.replace_index(
                record.id,
                &stored_index(
                    &object,
                    &record,
                    &references,
                    &crate::formula_fields::stored_formulas(&object),
                ),
            )
            .await?;
        }
        let removed = tx.delete_shares(record.id).await?;
        self.record_change(
            tx,
            context,
            Change::OwnerChanged(&object, &record, previous, removed),
        )
        .await?;
        if !changed.is_empty() {
            changed.sort();
            changed.dedup();
            self.record_change(
                tx,
                context,
                Change::RecordUpdated(&object, &record, &changed),
            )
            .await?;
        }
        self.track(
            tx,
            &object,
            Some(&before),
            &record,
            HistoryKind::Changed,
            context,
        )
        .await?;
        self.after_save(
            tx,
            &object,
            SaveEvent::Update,
            Some(&before),
            &record,
            context,
            run,
        )
        .await?;
        Ok((object, record))
    }

    /// A record's history, newest first (ADR 0017), with the names of
    /// lookup targets in it that the caller can see. A record the caller
    /// cannot see has no visible history; entries for retired fields are
    /// left out. Records in the recycle bin keep their history.
    pub async fn record_history(
        &self,
        api_name: &str,
        public_id: &str,
        cursor: Option<&str>,
        limit: Option<u16>,
    ) -> Result<(HistoryPage, BTreeMap<String, String>), ObjectError> {
        let limit = limit.unwrap_or(LIST_LIMIT_DEFAULT);
        if !(1..=LIST_LIMIT_MAX).contains(&limit) {
            return Err(ObjectError::invalid(
                "limit",
                "records.limit_invalid",
                "ask for between 1 and 200 entries",
            ));
        }
        let before = match cursor {
            None => None,
            Some(token) => {
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| invalid_cursor())?;
                let token: HistoryCursor =
                    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
                Some((token.t, token.i))
            }
        };
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if !access.for_object(object.id).read {
            return Err(ObjectError::NotFound);
        }
        let record = tx
            .record(object.id, public_id)
            .await?
            .ok_or(ObjectError::NotFound)?;
        if !self
            .rights_on(tx.as_mut(), &access, &object, &record)
            .await?
            .read
        {
            return Err(ObjectError::NotFound);
        }
        let mut entries = tx.record_history(record.id, before, limit).await?;
        let next_cursor = if entries.len() > usize::from(limit) {
            entries.truncate(usize::from(limit));
            entries.last().map(|last| {
                let token = HistoryCursor {
                    t: last.changed_at,
                    i: last.id,
                };
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&token).unwrap_or_default())
            })
        } else {
            None
        };
        entries.retain(|entry| match entry.field.as_deref() {
            None | Some("name") | Some("owner") => true,
            Some(field) => object.field(field).is_some(),
        });
        let mut names = BTreeMap::new();
        let mut objects = BTreeMap::new();
        for entry in &entries {
            let Some(field) = entry.field.as_deref().and_then(|field| object.field(field)) else {
                continue;
            };
            if !field.field_type.holds_reference() {
                continue;
            }
            for id in [&entry.old_value, &entry.new_value]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if names.contains_key(id) {
                    continue;
                }
                if let Some(target) = tx.record_by_public_id(id).await? {
                    if self
                        .can_see(tx.as_mut(), &access, &mut objects, &target)
                        .await?
                    {
                        names.insert(id.to_owned(), target.name);
                    }
                }
            }
        }
        tx.commit().await?;
        Ok((
            HistoryPage {
                entries,
                next_cursor,
            },
            names,
        ))
    }

    /// The workspace's change log after `after` (ADR 0018), oldest first,
    /// with only the changes to records the caller can see now, and
    /// optionally only one object's. Without `after` it starts at the oldest
    /// change still kept; a position that has expired is
    /// `Conflict("changes.position_expired")`, and the client must
    /// resynchronise with `touched_after` lists.
    pub async fn list_changes(
        &self,
        after: Option<i64>,
        limit: Option<u16>,
        object: Option<&str>,
    ) -> Result<ChangePage, ObjectError> {
        let limit = limit.unwrap_or(LIST_LIMIT_DEFAULT);
        if !(1..=LIST_LIMIT_MAX).contains(&limit) {
            return Err(ObjectError::invalid(
                "limit",
                "records.limit_invalid",
                "ask for between 1 and 200 changes",
            ));
        }
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let only = match object {
            Some(api_name) => {
                let object = self.active_object(tx.as_mut(), api_name).await?;
                if !access.for_object(object.id).read {
                    return Err(ObjectError::NotFound);
                }
                Some(object.id)
            }
            None => None,
        };
        let (head, expired_through) = tx.change_bounds().await?;
        let after = after.unwrap_or(expired_through);
        if after < 0 || after > head {
            return Err(ObjectError::invalid(
                "after",
                "changes.position_invalid",
                "resume from a position the log has issued",
            ));
        }
        if after < expired_through {
            return Err(ObjectError::Conflict("changes.position_expired"));
        }
        let scanned = tx.changes(after, limit).await?;
        let next_after = scanned.last().map_or(after, |change| change.position);
        let mut objects: BTreeMap<Uuid, Option<ObjectDefinition>> = BTreeMap::new();
        let mut changes = Vec::new();
        for change in scanned {
            if only.is_some_and(|only| only != change.object_id) {
                continue;
            }
            let Some(record) = tx.record_by_id(change.record_id).await? else {
                continue;
            };
            let cached = match objects.get(&change.object_id) {
                Some(cached) => cached.clone(),
                None => {
                    let loaded = tx
                        .object_by_id(change.object_id)
                        .await?
                        .filter(ObjectDefinition::is_active);
                    objects.insert(change.object_id, loaded.clone());
                    loaded
                }
            };
            let Some(definition) = cached else {
                continue;
            };
            if self
                .rights_on(tx.as_mut(), &access, &definition, &record)
                .await?
                .read
            {
                changes.push((definition.api_name.clone(), change));
            }
        }
        // Read after the changes, so it is never older than the definitions
        // any change on this page was made under.
        let metadata_version = tx.metadata_version().await?;
        tx.commit().await?;
        Ok(ChangePage {
            changes,
            next_after,
            head,
            metadata_version,
        })
    }

    /// An active record the caller can see, with their rights on it.
    async fn visible_record(
        &self,
        tx: &mut dyn ObjectTx,
        api_name: &str,
        public_id: &str,
    ) -> Result<(ObjectDefinition, Record, RecordRights), ObjectError> {
        let access = self.access(tx).await?;
        let object = self.active_object(tx, api_name).await?;
        if !access.for_object(object.id).read {
            return Err(ObjectError::NotFound);
        }
        let record = tx
            .record(object.id, public_id)
            .await?
            .filter(|record| record.deleted_at.is_none())
            .ok_or(ObjectError::NotFound)?;
        let rights = self.rights_on(tx, &access, &object, &record).await?;
        if !rights.read {
            return Err(ObjectError::NotFound);
        }
        Ok((object, record, rights))
    }

    pub async fn list_records(
        &self,
        api_name: &str,
        request: ListRecords,
    ) -> Result<RecordPage, ObjectError> {
        let limit = request.limit.unwrap_or(LIST_LIMIT_DEFAULT);
        if !(1..=LIST_LIMIT_MAX).contains(&limit) {
            return Err(ObjectError::invalid(
                "limit",
                "records.limit_invalid",
                format!("use 1 to {LIST_LIMIT_MAX}"),
            ));
        }
        let (sort, direction) = sort_code(request.sort, request.direction);
        let after = match request.cursor.as_deref() {
            None | Some("") => None,
            Some(token) => {
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| invalid_cursor())?;
                let token: CursorToken =
                    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
                if token.s != sort || token.d != direction {
                    return Err(invalid_cursor());
                }
                Some(RecordCursor {
                    sort_value: token.v,
                    id: token.i,
                })
            }
        };
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let flags = access.for_object(object.id);
        if !flags.read {
            return Err(ObjectError::NotFound);
        }
        let mut issues = Vec::new();
        let mut filters = Vec::new();
        for (key, raw) in &request.filters {
            match object.field(key) {
                Some(field) if field.is_indexed() => match filter_value(field, raw) {
                    Ok(value) => filters.push(RecordFilter {
                        field_id: field.id,
                        value,
                    }),
                    Err(issue) => issues.push(issue),
                },
                Some(_) => issues.push(ValidationIssue::new(
                    key.clone(),
                    "records.filter_not_indexed",
                    "this field is not indexed, so it cannot be filtered",
                )),
                None => issues.push(ValidationIssue::new(
                    key.clone(),
                    "records.field_unknown",
                    "this object has no such field",
                )),
            }
        }
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        let search = request
            .search
            .as_deref()
            .map(values::name_key)
            .filter(|term| !term.is_empty());
        if search
            .as_ref()
            .is_some_and(|term| term.chars().count() > 100)
        {
            return Err(ObjectError::invalid(
                "search",
                "records.search_too_long",
                "search with at most 100 characters",
            ));
        }
        let query = RecordQuery {
            status: request.status,
            sort: request.sort,
            direction: request.direction,
            limit,
            after,
            filters,
            owner: request.owner,
            name_contains: search,
            visibility: self.visibility_of(tx.as_mut(), &access, &object).await?,
            touched_after: request.touched_after,
        };
        let mut records = tx.list_records(object.id, &query).await?;
        tx.commit().await?;
        let next_cursor = if records.len() > usize::from(limit) {
            records.truncate(usize::from(limit));
            records.last().map(|last| {
                let token = CursorToken {
                    s: sort.into(),
                    d: direction.into(),
                    v: sort_value(last, request.sort),
                    i: last.id,
                };
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&token).unwrap_or_default())
            })
        } else {
            None
        };
        let records = records
            .into_iter()
            .map(|record| Self::project(&object, record))
            .collect();
        Ok(RecordPage {
            records: self.with_formulas(&object, records).await?,
            next_cursor,
        })
    }
}

/// The active roll-up summary fields of an object.
fn rollup_fields(object: &ObjectDefinition) -> Vec<&FieldDefinition> {
    object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.field_type == FieldType::Rollup)
        .collect()
}

/// A roll-up's value as the master stores it: a count or sum is zero
/// without details, a minimum or maximum is empty.
fn rollup_json(field: &FieldDefinition, value: Option<crate::IndexValue>) -> Option<Value> {
    let function = field.rollup.as_ref()?.function;
    match (value, field.value_type()) {
        (Some(crate::IndexValue::Number(number)), _) => {
            let factor = 10f64.powi(field.scale.unwrap_or(0));
            let rounded = (number * factor).round() / factor;
            // Whole results are stored as integers, as entered values are.
            if rounded.fract() == 0.0 && rounded.abs() < 9.0e15 {
                Some(Value::from(rounded as i64))
            } else {
                serde_json::Number::from_f64(rounded).map(Value::Number)
            }
        }
        (Some(crate::IndexValue::Time(time)), FieldType::Date) => {
            Some(Value::String(time.format("%Y-%m-%d").to_string()))
        }
        (Some(crate::IndexValue::Time(time)), _) => Some(Value::String(
            time.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
        )),
        (Some(crate::IndexValue::Text(_)), _) => None,
        (None, _)
            if matches!(
                function,
                crate::RollupFunction::Sum | crate::RollupFunction::Count
            ) =>
        {
            Some(Value::from(0))
        }
        (None, _) => None,
    }
}

/// Locks the masters of a detail record, before the record itself: every
/// path that writes a detail takes its master first (ADR 0026).
async fn lock_masters(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    record: &Record,
) -> Result<(), ObjectError> {
    for field in object.masters() {
        let Some(public_id) = record.values.get(&field.api_name).and_then(Value::as_str) else {
            continue;
        };
        if let Some(master) = tx.record_by_public_id(public_id).await? {
            tx.lock_record(master.id, true).await?;
        }
    }
    Ok(())
}

/// A resolved roll-up summary for a new field.
struct ResolvedRollup {
    rollup: crate::Rollup,
    result_type: FieldType,
    scale: Option<i32>,
    /// The summarised field, when it has to be indexed first.
    index: Option<FieldDefinition>,
}

/// Checks a new roll-up summary on `master` against the detail object it
/// summarises (ADR 0026).
async fn resolve_rollup(
    tx: &mut dyn ObjectTx,
    master: &ObjectDefinition,
    input: &crate::CreateRollup,
) -> Result<ResolvedRollup, ObjectError> {
    use crate::RollupFunction;
    if rollup_fields(master).len() >= ROLLUPS_PER_OBJECT_MAX {
        return Err(ObjectError::Conflict("objects.rollup_limit_reached"));
    }
    let detail = tx
        .object_by_api_name(&input.object)
        .await?
        .filter(ObjectDefinition::is_active)
        .ok_or_else(|| {
            ObjectError::invalid(
                "rollup.object",
                "objects.rollup_object_unknown",
                "choose an existing object",
            )
        })?;
    let relationship = detail
        .field(&input.relationship)
        .filter(|field| {
            field.field_type == FieldType::MasterDetail
                && field.reference_object_id == Some(master.id)
        })
        .ok_or_else(|| {
            ObjectError::invalid(
                "rollup.relationship",
                "objects.rollup_relationship_invalid",
                format!(
                    "choose a master-detail field of {} that points to this object",
                    detail.api_name
                ),
            )
        })?;
    let summarised = match (input.function, input.field.as_deref()) {
        (RollupFunction::Count, None) => None,
        (RollupFunction::Count, Some(_)) => {
            return Err(ObjectError::invalid(
                "rollup.field",
                "objects.rollup_field_not_allowed",
                "a count needs no field",
            ))
        }
        (_, None) => {
            return Err(ObjectError::invalid(
                "rollup.field",
                "objects.rollup_field_required",
                "choose the field to summarise",
            ))
        }
        (function, Some(api_name)) => {
            let field = detail.field(api_name).cloned().ok_or_else(|| {
                ObjectError::invalid(
                    "rollup.field",
                    "objects.rollup_field_invalid",
                    format!("{} has no field {api_name}", detail.api_name),
                )
            })?;
            let allowed = field.field_type.is_numeric()
                || (function != RollupFunction::Sum
                    && matches!(field.field_type, FieldType::Date | FieldType::DateTime));
            if !allowed {
                return Err(ObjectError::invalid(
                    "rollup.field",
                    "objects.rollup_field_invalid",
                    "sum a number, currency or percent field; take the minimum or maximum of one of those or of a date",
                ));
            }
            Some(field)
        }
    };
    let (result_type, scale) = match &summarised {
        None => (FieldType::Number, Some(0)),
        Some(field) => (field.field_type, field.scale),
    };
    Ok(ResolvedRollup {
        rollup: crate::Rollup {
            function: input.function,
            relationship_id: relationship.id,
            field_id: summarised.as_ref().map(|field| field.id),
            object: detail.api_name.clone(),
            relationship: relationship.api_name.clone(),
            field: summarised.as_ref().map(|field| field.api_name.clone()),
        },
        result_type,
        scale,
        index: summarised.filter(|field| !field.is_indexed()),
    })
}

/// Whether a roll-up summary on a master of `object` uses its master-detail
/// field `relationship` (or any of them), or summarises `field`.
async fn rollup_uses(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    field: Option<Uuid>,
) -> Result<bool, ObjectError> {
    for relationship in object.masters() {
        let Some(master) = (match relationship.reference_object_id {
            Some(id) => tx.object_by_id(id).await?,
            None => None,
        }) else {
            continue;
        };
        let used = rollup_fields(&master).into_iter().any(|rollup| {
            rollup.rollup.as_ref().is_some_and(|rollup| {
                rollup.relationship_id == relationship.id
                    && match field {
                        None => true,
                        Some(field) => field == relationship.id || rollup.field_id == Some(field),
                    }
            })
        });
        if used {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The first master of a detail record that is in the recycle bin.
async fn deleted_master(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    record: &Record,
) -> Result<Option<Uuid>, ObjectError> {
    for field in object.masters() {
        let Some(public_id) = record.values.get(&field.api_name).and_then(Value::as_str) else {
            continue;
        };
        if let Some(master) = tx.record_by_public_id(public_id).await? {
            if master.deleted_at.is_some() {
                return Ok(Some(master.id));
            }
        }
    }
    Ok(None)
}

/// Whether every master of a detail record is outside the recycle bin, so
/// the detail may be restored (ADR 0025). Records of other objects pass.
async fn masters_live(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    record: &Record,
) -> Result<bool, ObjectError> {
    for field in object.masters() {
        let Some(public_id) = record.values.get(&field.api_name).and_then(Value::as_str) else {
            return Ok(false);
        };
        match tx.record_by_public_id(public_id).await? {
            Some(master) if master.deleted_at.is_none() => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}

/// Whether `input` changes nothing but a related-to field's allowlist: the
/// one property of a plugin's field administrators may change.
fn only_allowlist(field: &FieldDefinition, input: &UpdateField) -> bool {
    let UpdateField {
        expected_version: _,
        label,
        description,
        required,
        indexed,
        searchable,
        track_history,
        max_length,
        options,
        default_value,
        position,
        formula,
        lookup_filter,
        reference_objects,
    } = input;
    field.field_type == FieldType::RelatedTo
        && reference_objects.is_some()
        && label.is_none()
        && description.is_none()
        && required.is_none()
        && indexed.is_none()
        && searchable.is_none()
        && track_history.is_none()
        && max_length.is_none()
        && options.is_none()
        && default_value.is_none()
        && position.is_none()
        && formula.is_none()
        && lookup_filter.is_none()
}

/// The active objects a related-to field's allowlist names, in its order,
/// or the issue naming the first that is unknown or deleted.
async fn allowed_objects(
    tx: &mut dyn ObjectTx,
    names: &[String],
    installer: Option<&str>,
) -> Result<Result<Vec<ObjectDefinition>, ValidationIssue>, ObjectError> {
    let mut allowed = Vec::with_capacity(names.len());
    for name in names {
        match tx.object_by_api_name(name).await? {
            Some(target) if usable_target(&target, installer) => allowed.push(target),
            _ => {
                return Ok(Err(ValidationIssue::new(
                    "reference_objects",
                    "objects.reference_object_unknown",
                    format!("{name} is not an existing object"),
                )))
            }
        }
    }
    Ok(Ok(allowed))
}

/// Whether a field may point to `target`: an object in service, or, while
/// a plugin installs its objects, one of that plugin's own objects that is
/// still suspended until the installation finishes (ADR 0033 §2).
pub(crate) fn usable_target(target: &ObjectDefinition, installer: Option<&str>) -> bool {
    target.is_active()
        || (target.status == DefinitionStatus::Active
            && target.suspended
            && installer.is_some()
            && target.owner_plugin.as_deref() == installer)
}

/// Whether `object` may gain a master-detail field to `master` (ADR 0025):
/// one level only, at most two masters, and no records yet, since every
/// existing record would need a master.
async fn check_master_detail(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    master: Option<&ObjectDefinition>,
) -> Result<(), ObjectError> {
    let Some(master) = master else {
        return Err(ObjectError::Invariant);
    };
    if master.id == object.id {
        return Err(ObjectError::invalid(
            "reference_object",
            "objects.master_detail_invalid",
            "an object cannot be its own master",
        ));
    }
    if master.primary_master().is_some()
        || tx
            .fields_referencing_object(object.id)
            .await?
            .iter()
            .any(|field| field.field_type == FieldType::MasterDetail)
    {
        return Err(ObjectError::invalid(
            "reference_object",
            "objects.master_detail_invalid",
            "a master cannot itself be a detail; use a lookup",
        ));
    }
    // Waits for record inserts in flight, and holds new ones back until
    // this field commits; they then see it and need a master.
    tx.lock_object(object.id, true).await?;
    if object.masters().len() >= MASTERS_PER_OBJECT_MAX {
        return Err(ObjectError::Conflict("objects.master_detail_limit_reached"));
    }
    if tx.has_records(object.id).await? {
        return Err(ObjectError::Conflict("objects.master_detail_has_records"));
    }
    Ok(())
}

fn default_value(
    field: &FieldDefinition,
    value: Option<&Value>,
) -> Result<Option<Value>, ObjectError> {
    let Some(value) = value else {
        return Ok(None);
    };
    if field.field_type.holds_reference() {
        return Err(ObjectError::invalid(
            "default_value",
            "objects.default_not_allowed",
            "lookup and related-to fields have no default value",
        ));
    }
    values::normalise(field, value).map_err(|issue| {
        ObjectError::Validation(vec![ValidationIssue::new(
            "default_value",
            issue.code,
            issue.detail,
        )])
    })
}

/// The lookup targets a record's stored values resolve to now.
async fn current_references(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    record: &Record,
) -> Result<BTreeMap<Uuid, Uuid>, ObjectError> {
    let mut resolved = BTreeMap::new();
    for field in object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.field_type.holds_reference())
    {
        if let Some(public_id) = record.values.get(&field.api_name).and_then(Value::as_str) {
            if let Some(target) = tx.record_by_public_id(public_id).await? {
                resolved.insert(field.id, target.id);
            }
        }
    }
    Ok(resolved)
}

/// Recomputes every record's pivot rows after a field's indexing changed.
/// Whether everything in `before` still compiles after a metadata change.
async fn still_working(
    before: &BTreeMap<Uuid, Uuid>,
    tx: &mut dyn ObjectTx,
) -> Result<bool, ObjectError> {
    let after = crate::rules::compiling_dependents(tx).await?;
    Ok(before.keys().all(|id| after.contains_key(id)))
}

/// Rebuilds every record's index rows, stored formula values included, and
/// marks the object as having them (ADR 0021 §6).
pub(crate) async fn reindex(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
) -> Result<(), ObjectError> {
    let formulas = crate::formula_fields::stored_formulas(object);
    for id in tx.record_ids(object.id).await? {
        let Some(record) = tx.record_by_id(id).await? else {
            continue;
        };
        let references = current_references(tx, object, &record).await?;
        tx.replace_index(
            record.id,
            &stored_index(object, &record, &references, &formulas),
        )
        .await?;
    }
    tx.mark_formulas_indexed(object.id).await
}

/// The pivot rows a record keeps: everything for a live record, including
/// stored formula values (ADR 0021 §6), and no unique values while it is in
/// the recycle bin, as in Salesforce.
pub(crate) fn stored_index(
    object: &ObjectDefinition,
    record: &Record,
    references: &BTreeMap<Uuid, Uuid>,
    formulas: &[crate::formula_fields::StoredFormula],
) -> RecordIndex {
    let mut index = record_index(object, &record.name, &record.values, references);
    crate::formula_fields::index_formulas(formulas, record, &mut index);
    if record.deleted_at.is_some() {
        index.uniques.clear();
    }
    index
}
