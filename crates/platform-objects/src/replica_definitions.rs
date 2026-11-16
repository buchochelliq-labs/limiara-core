//! A synced device's definitions (ADR 0024 §3). When the server's metadata
//! version moves, the device takes the objects, fields, validation rules
//! and quick actions (ADR 0032 §5) the server describes, as the server has
//! them, before it applies record changes made under them. The server's API names and key prefixes are
//! kept, so records and device-assigned ids (ADR 0031) mean the same on
//! both sides; the replica keeps its own internal ids.

use crate::service::{reindex, ObjectService};
use crate::{
    AccessFlags, DefinitionStatus, DeleteBehaviour, FieldDefinition, FieldType, ObjectDefinition,
    ObjectError, ObjectTx, Origin, PicklistOption, QuickAction, QuickActionKind, RolePermission,
    Rollup, RollupFunction, SharingModel, ValidationRule,
};
use chrono::Utc;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// An object as the server describes it to the member, with the member's
/// effective access to it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteObject {
    pub api_name: String,
    pub label: String,
    pub plural_label: String,
    pub description: Option<String>,
    pub origin: Origin,
    pub owner_plugin: Option<String>,
    pub key_prefix: String,
    pub name_label: String,
    pub sharing_model: SharingModel,
    pub grant_hierarchy: bool,
    pub track_history: bool,
    pub searchable: bool,
    pub in_navigation: bool,
    pub version: i64,
    pub access: AccessFlags,
    /// Active fields in display order.
    pub fields: Vec<RemoteField>,
    /// The object's validation rules; `None` when the server does not show
    /// them to the member, which leaves the replica's rules as they are.
    pub rules: Option<Vec<RemoteRule>>,
    /// The object's quick actions (ADR 0032 §5); `None` when the server
    /// does not show them to the member, which leaves the replica's actions
    /// as they are.
    pub quick_actions: Option<Vec<RemoteQuickAction>>,
}

impl RemoteObject {
    /// The description a server's engine gives of one of its objects, as
    /// its API renders it.
    pub fn from_definition(
        object: &ObjectDefinition,
        access: AccessFlags,
        rules: Option<&[ValidationRule]>,
    ) -> Self {
        Self {
            api_name: object.api_name.clone(),
            label: object.label.clone(),
            plural_label: object.plural_label.clone(),
            description: object.description.clone(),
            origin: object.origin,
            owner_plugin: object.owner_plugin.clone(),
            key_prefix: object.key_prefix.clone(),
            name_label: object.name_label.clone(),
            sharing_model: object.sharing_model,
            grant_hierarchy: object.grant_hierarchy,
            track_history: object.track_history,
            searchable: object.searchable,
            in_navigation: object.in_navigation,
            version: object.version,
            access,
            fields: object
                .fields
                .iter()
                .filter(|field| field.is_active())
                .map(|field| RemoteField {
                    api_name: field.api_name.clone(),
                    label: field.label.clone(),
                    description: field.description.clone(),
                    field_type: field.field_type,
                    origin: field.origin,
                    required: field.required,
                    unique: field.unique,
                    indexed: field.is_indexed(),
                    searchable: field.searchable,
                    track_history: field.track_history,
                    max_length: field.max_length,
                    scale: field.scale,
                    options: field.options.clone(),
                    default_value: field.default_value.clone(),
                    reference_object: field.reference_object.clone(),
                    reference_objects: field.reference_objects.clone(),
                    delete_behaviour: field.delete_behaviour,
                    lookup_filter: field.lookup_filter.clone(),
                    formula: field.formula.clone(),
                    formula_type: field.formula_type,
                    rollup: field.rollup.as_ref().map(|rollup| RemoteRollup {
                        function: rollup.function,
                        object: rollup.object.clone(),
                        relationship: rollup.relationship.clone(),
                        field: rollup.field.clone(),
                    }),
                    position: field.position,
                    version: field.version,
                })
                .collect(),
            rules: rules.map(|rules| {
                rules
                    .iter()
                    .map(|rule| RemoteRule {
                        name: rule.name.clone(),
                        description: rule.description.clone(),
                        condition: rule.condition.clone(),
                        message: rule.message.clone(),
                        error_field: rule.error_field.clone(),
                        active: rule.active,
                        version: rule.version,
                    })
                    .collect()
            }),
            quick_actions: None,
        }
    }

    /// The same description with the object's quick actions, as the
    /// server's `GET /objects/{object}/quick-actions` lists them.
    pub fn with_quick_actions(mut self, actions: &[QuickAction]) -> Self {
        self.quick_actions = Some(
            actions
                .iter()
                .map(|action| RemoteQuickAction {
                    name: action.name.clone(),
                    label: action.label.clone(),
                    kind: action.kind,
                    position: action.position,
                    target_object: action.target_object.clone(),
                    target_field: action.target_field.clone(),
                    fields: action.fields.clone(),
                    presets: action.presets.clone(),
                    version: action.version,
                })
                .collect(),
        );
        self
    }
}

/// A roll-up summary by API names, as the server describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRollup {
    pub function: RollupFunction,
    pub object: String,
    pub relationship: String,
    pub field: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteField {
    pub api_name: String,
    pub label: String,
    pub description: Option<String>,
    pub field_type: FieldType,
    pub origin: Origin,
    pub required: bool,
    pub unique: bool,
    pub indexed: bool,
    pub searchable: bool,
    pub track_history: bool,
    pub max_length: Option<i32>,
    pub scale: Option<i32>,
    pub options: Vec<PicklistOption>,
    pub default_value: Option<Value>,
    pub reference_object: Option<String>,
    /// A related-to field's allowlist, by API name.
    pub reference_objects: Vec<String>,
    pub delete_behaviour: Option<DeleteBehaviour>,
    pub lookup_filter: Option<String>,
    pub formula: Option<String>,
    pub formula_type: Option<FieldType>,
    pub rollup: Option<RemoteRollup>,
    pub position: i32,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RemoteRule {
    pub name: String,
    pub description: Option<String>,
    pub condition: String,
    pub message: String,
    pub error_field: Option<String>,
    pub active: bool,
    pub version: i64,
}

/// A quick action by API names, as the server lists it (ADR 0032).
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteQuickAction {
    pub name: String,
    pub label: String,
    pub kind: QuickActionKind,
    pub position: i32,
    /// The child object a create action makes records of.
    pub target_object: Option<String>,
    /// The field on the child object set to the current record.
    pub target_field: Option<String>,
    pub fields: Vec<String>,
    pub presets: BTreeMap<String, Value>,
    pub version: i64,
}

/// What taking the server's definitions changed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DefinitionsApplied {
    /// Objects added, changed or retired, by API name.
    pub objects: Vec<String>,
    /// Fields added, changed or retired, as `object.field`.
    pub fields: Vec<String>,
    /// Validation rules added, changed or removed, as `object.rule`.
    pub rules: Vec<String>,
    /// Validation rules the replica cannot check, as `object.rule`: their
    /// condition names a field the replica leaves out, such as a lookup to
    /// an object the member cannot read. The server checks them on push.
    pub deferred_rules: Vec<String>,
    /// Quick actions added, changed or removed, as `object.action`.
    pub quick_actions: Vec<String>,
    /// Quick actions the replica cannot offer, as `object.action`: a field
    /// they ask for or preset, or for a create action its object or
    /// reference field, is not on the replica, such as a lookup to an object
    /// the member cannot read or an object suspended with its plugin. They
    /// stay with the server, and come back when what they need does.
    pub deferred_quick_actions: Vec<String>,
}

impl DefinitionsApplied {
    /// Whether anything changed; rules and quick actions deferred to the
    /// server change nothing on the replica.
    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
            && self.fields.is_empty()
            && self.rules.is_empty()
            && self.quick_actions.is_empty()
    }
}

impl ObjectService {
    /// Makes the replica's definitions the server's: `objects` is every
    /// object the server describes to the member. Objects and fields the
    /// server no longer describes are retired, as deleting them would, and
    /// rules and quick actions it no longer has are removed, with the quick
    /// actions of retired objects. Only a replica's engine may, and
    /// it needs no `objects.manage`: the server already decided. Nothing is
    /// logged and the metadata version does not move; the device stores the
    /// server's version with `record_pull` once its records follow.
    ///
    /// The member's access to each object is granted to every role of the
    /// replica's caller. Every record a replica holds was sent because the
    /// member may read it, so read access covers all of them; what the
    /// member may change is decided again by the server on push.
    pub async fn apply_remote_definitions(
        &self,
        objects: &[RemoteObject],
    ) -> Result<DefinitionsApplied, ObjectError> {
        if !self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        let mut applied = DefinitionsApplied::default();
        let actor = self.caller().user_id();
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;

        // Objects first, so fields can point at any of them.
        let mut ids: BTreeMap<String, Uuid> = BTreeMap::new();
        for remote in objects {
            let id = put_object(tx.as_mut(), remote, actor, &mut applied).await?;
            ids.insert(remote.api_name.clone(), id);
        }
        let described: BTreeSet<&str> = objects.iter().map(|o| o.api_name.as_str()).collect();
        let mut undescribed = Vec::new();
        for local in tx.list_objects().await? {
            if !described.contains(local.api_name.as_str()) {
                undescribed.push(local.clone());
            }
            if local.is_active() && !described.contains(local.api_name.as_str()) {
                let mut retired = local.clone();
                retired.status = DefinitionStatus::Deleted;
                retired.updated_at = Utc::now();
                if !tx.update_object(&retired, local.version).await? {
                    return Err(ObjectError::VersionConflict);
                }
                applied.objects.push(local.api_name);
            }
        }

        // Fields that name no other field first, then roll-ups, which name
        // the detail object's master-detail field and summarised field.
        let mut changed_objects = BTreeSet::new();
        for rollups in [false, true] {
            for remote in objects {
                let object_id = ids
                    .get(&remote.api_name)
                    .copied()
                    .ok_or(ObjectError::Invariant)?;
                for field in remote
                    .fields
                    .iter()
                    .filter(|field| field.rollup.is_some() == rollups)
                {
                    if put_field(tx.as_mut(), object_id, field, &ids, actor).await? {
                        changed_objects.insert(object_id);
                        applied
                            .fields
                            .push(format!("{}.{}", remote.api_name, field.api_name));
                    }
                }
            }
        }
        for remote in objects {
            let object_id = ids
                .get(&remote.api_name)
                .copied()
                .ok_or(ObjectError::Invariant)?;
            let local = tx
                .object_by_id(object_id)
                .await?
                .ok_or(ObjectError::Invariant)?;
            let described: BTreeSet<&str> =
                remote.fields.iter().map(|f| f.api_name.as_str()).collect();
            for field in &local.fields {
                if described.contains(field.api_name.as_str()) {
                    continue;
                }
                let mut retired = field.clone();
                retired.status = DefinitionStatus::Deleted;
                retired.version += 1;
                retired.updated_at = Utc::now();
                retired.updated_by = Some(actor);
                if !tx.update_field(&retired, field.version).await? {
                    return Err(ObjectError::VersionConflict);
                }
                tx.drop_field_index(field.id).await?;
                changed_objects.insert(object_id);
                applied
                    .fields
                    .push(format!("{}.{}", remote.api_name, field.api_name));
            }
            if let Some(rules) = &remote.rules {
                put_rules(tx.as_mut(), &local, rules, actor, &mut applied).await?;
            }
            for role_id in self.caller().role_ids() {
                tx.set_object_permission(
                    object_id,
                    &RolePermission {
                        role_id: *role_id,
                        access: replica_access(remote.access),
                    },
                    actor,
                )
                .await?;
            }
        }

        // Quick actions last: a create action names another object's fields,
        // which are now as the server describes them. An object no longer
        // described keeps no actions; nothing on the device could run them.
        for remote in objects {
            let Some(actions) = &remote.quick_actions else {
                continue;
            };
            let object_id = ids
                .get(&remote.api_name)
                .copied()
                .ok_or(ObjectError::Invariant)?;
            let local = tx
                .object_by_id(object_id)
                .await?
                .ok_or(ObjectError::Invariant)?;
            put_quick_actions(tx.as_mut(), &local, actions, actor, &mut applied).await?;
        }
        for local in &undescribed {
            put_quick_actions(tx.as_mut(), local, &[], actor, &mut applied).await?;
        }

        // Index rows, search terms and stored formula values follow the
        // fields as they now are.
        for object_id in changed_objects {
            if let Some(object) = tx.object_by_id(object_id).await? {
                reindex(tx.as_mut(), &object).await?;
            }
        }
        tx.commit().await?;
        Ok(applied)
    }
}

/// The member reads every record the replica holds; the rest is as the
/// server grants it.
fn replica_access(access: AccessFlags) -> AccessFlags {
    AccessFlags {
        view_all: access.view_all || access.read,
        ..access
    }
    .normalised()
}

async fn put_object(
    tx: &mut dyn ObjectTx,
    remote: &RemoteObject,
    actor: Uuid,
    applied: &mut DefinitionsApplied,
) -> Result<Uuid, ObjectError> {
    let now = Utc::now();
    match tx.object_by_api_name(&remote.api_name).await? {
        None => {
            let object = ObjectDefinition {
                id: Uuid::now_v7(),
                api_name: remote.api_name.clone(),
                label: remote.label.clone(),
                plural_label: remote.plural_label.clone(),
                description: remote.description.clone(),
                origin: remote.origin,
                owner_plugin: remote.owner_plugin.clone(),
                key_prefix: remote.key_prefix.clone(),
                name_label: remote.name_label.clone(),
                sharing_model: remote.sharing_model,
                grant_hierarchy: remote.grant_hierarchy,
                track_history: remote.track_history,
                searchable: remote.searchable,
                in_navigation: remote.in_navigation,
                status: DefinitionStatus::Active,
                suspended: false,
                version: remote.version.max(1),
                created_at: now,
                created_by: Some(actor),
                updated_at: now,
                updated_by: Some(actor),
                fields: Vec::new(),
            };
            tx.insert_object(&object).await?;
            applied.objects.push(remote.api_name.clone());
            Ok(object.id)
        }
        Some(local) => {
            if local.key_prefix != remote.key_prefix {
                // The replica's records would carry ids the server does not
                // recognise; the device starts its replica again.
                return Err(ObjectError::Conflict("sync.definitions_diverged"));
            }
            let mut object = local.clone();
            object.label = remote.label.clone();
            object.plural_label = remote.plural_label.clone();
            object.description = remote.description.clone();
            object.name_label = remote.name_label.clone();
            object.sharing_model = remote.sharing_model;
            object.grant_hierarchy = remote.grant_hierarchy;
            object.track_history = remote.track_history;
            object.searchable = remote.searchable;
            object.in_navigation = remote.in_navigation;
            object.status = DefinitionStatus::Active;
            object.version = remote.version.max(1);
            if object == local {
                return Ok(local.id);
            }
            object.updated_at = now;
            object.updated_by = Some(actor);
            if !tx.update_object(&object, local.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            applied.objects.push(remote.api_name.clone());
            Ok(local.id)
        }
    }
}

/// Adds or changes one field; whether anything changed.
async fn put_field(
    tx: &mut dyn ObjectTx,
    object_id: Uuid,
    remote: &RemoteField,
    ids: &BTreeMap<String, Uuid>,
    actor: Uuid,
) -> Result<bool, ObjectError> {
    let object = tx
        .object_by_id(object_id)
        .await?
        .ok_or(ObjectError::Invariant)?;
    // A lookup to an object the member cannot read, or a roll-up of one,
    // has nothing on the device to point at: the replica leaves the field
    // out, as it does any field it does not define.
    let reference_object_id = match remote.reference_object.as_deref() {
        Some(target) => match ids.get(target) {
            Some(id) => Some(*id),
            None => return Ok(false),
        },
        None => None,
    };
    // A related-to field allows, on the device, the objects of its
    // allowlist the replica holds; the server decides any other target on
    // push (ADR 0033 §1). A new one that allows none of them is left out
    // like a lookup to an object the member cannot read. One already here
    // keeps an empty allowlist instead of retiring, so nothing new can be
    // chosen locally and a later allowlist or access change brings its
    // targets back.
    let allowed: Vec<(Uuid, String)> = remote
        .reference_objects
        .iter()
        .filter_map(|name| ids.get(name).map(|id| (*id, name.clone())))
        .collect();
    if remote.field_type == FieldType::RelatedTo
        && allowed.is_empty()
        && object.field(&remote.api_name).is_none()
    {
        return Ok(false);
    }
    let reference_object_ids: Vec<Uuid> = allowed.iter().map(|(id, _)| *id).collect();
    let reference_objects: Vec<String> = allowed.into_iter().map(|(_, name)| name).collect();
    let rollup = match &remote.rollup {
        Some(rollup) => match resolve_rollup(tx, rollup, ids).await? {
            Some(rollup) => Some(rollup),
            None => return Ok(false),
        },
        None => None,
    };
    let now = Utc::now();
    match object.field(&remote.api_name) {
        None => {
            let field = FieldDefinition {
                id: Uuid::now_v7(),
                object_id,
                api_name: remote.api_name.clone(),
                label: remote.label.clone(),
                description: remote.description.clone(),
                field_type: remote.field_type,
                origin: remote.origin,
                required: remote.required,
                unique: remote.unique,
                indexed: remote.indexed,
                searchable: remote.searchable,
                track_history: remote.track_history,
                max_length: remote.max_length,
                scale: remote.scale,
                options: remote.options.clone(),
                default_value: remote.default_value.clone(),
                reference_object_id,
                reference_object: remote.reference_object.clone(),
                reference_object_ids,
                reference_objects,
                delete_behaviour: remote.delete_behaviour,
                lookup_filter: remote.lookup_filter.clone(),
                formula: remote.formula.clone(),
                formula_type: remote.formula_type,
                rollup,
                position: remote.position,
                status: DefinitionStatus::Active,
                version: remote.version.max(1),
                created_at: now,
                created_by: Some(actor),
                updated_at: now,
                updated_by: Some(actor),
            };
            tx.insert_field(&field).await?;
            Ok(true)
        }
        Some(local) => {
            if local.field_type != remote.field_type {
                // A field's type never changes on the server (ADR 0015).
                return Err(ObjectError::Conflict("sync.definitions_diverged"));
            }
            let mut field = local.clone();
            field.label = remote.label.clone();
            field.description = remote.description.clone();
            field.required = remote.required;
            field.indexed = remote.indexed;
            field.searchable = remote.searchable;
            field.track_history = remote.track_history;
            field.max_length = remote.max_length;
            field.options = remote.options.clone();
            field.default_value = remote.default_value.clone();
            field.lookup_filter = remote.lookup_filter.clone();
            field.reference_object_ids = reference_object_ids;
            field.reference_objects = reference_objects;
            field.formula = remote.formula.clone();
            field.position = remote.position;
            field.version = remote.version.max(1);
            if field == *local {
                return Ok(false);
            }
            field.updated_at = now;
            field.updated_by = Some(actor);
            if !tx.update_field(&field, local.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            Ok(true)
        }
    }
}

async fn resolve_rollup(
    tx: &mut dyn ObjectTx,
    rollup: &RemoteRollup,
    ids: &BTreeMap<String, Uuid>,
) -> Result<Option<Rollup>, ObjectError> {
    let Some(detail_id) = ids.get(&rollup.object) else {
        return Ok(None);
    };
    let detail = tx
        .object_by_id(*detail_id)
        .await?
        .ok_or(ObjectError::Invariant)?;
    let Some(relationship) = detail.field(&rollup.relationship) else {
        return Ok(None);
    };
    let field_id = match rollup.field.as_deref() {
        Some(name) => match detail.field(name) {
            Some(field) => Some(field.id),
            None => return Ok(None),
        },
        None => None,
    };
    Ok(Some(Rollup {
        function: rollup.function,
        relationship_id: relationship.id,
        field_id,
        object: rollup.object.clone(),
        relationship: rollup.relationship.clone(),
        field: rollup.field.clone(),
    }))
}

async fn put_rules(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    remote: &[RemoteRule],
    actor: Uuid,
    applied: &mut DefinitionsApplied,
) -> Result<(), ObjectError> {
    let now = Utc::now();
    let local: BTreeMap<String, ValidationRule> = tx
        .validation_rules(object.id)
        .await?
        .into_iter()
        .map(|rule| (rule.name.clone(), rule))
        .collect();
    // Rules are checked against the object as the replica now defines it.
    // One that names a field the replica leaves out could not be checked
    // offline and would refuse every save, so it stays with the server.
    let current = tx
        .object_by_id(object.id)
        .await?
        .ok_or(ObjectError::Invariant)?;
    let reachable = crate::rules::formula_objects(tx, &current).await?;
    let mut kept = Vec::new();
    for rule in remote {
        if crate::rules::compiles(&rule.condition, &current, &reachable) {
            kept.push(rule);
        } else {
            applied
                .deferred_rules
                .push(format!("{}.{}", object.api_name, rule.name));
        }
    }
    for rule in &kept {
        match local.get(&rule.name) {
            None => {
                tx.insert_validation_rule(&ValidationRule {
                    id: Uuid::now_v7(),
                    object_id: object.id,
                    name: rule.name.clone(),
                    description: rule.description.clone(),
                    condition: rule.condition.clone(),
                    message: rule.message.clone(),
                    error_field: rule.error_field.clone(),
                    active: rule.active,
                    version: rule.version.max(1),
                    created_at: now,
                    created_by: Some(actor),
                    updated_at: now,
                    updated_by: Some(actor),
                })
                .await?;
            }
            Some(existing) => {
                let mut changed = existing.clone();
                changed.description = rule.description.clone();
                changed.condition = rule.condition.clone();
                changed.message = rule.message.clone();
                changed.error_field = rule.error_field.clone();
                changed.active = rule.active;
                changed.version = rule.version.max(1);
                if changed == *existing {
                    continue;
                }
                changed.updated_at = now;
                changed.updated_by = Some(actor);
                if !tx
                    .update_validation_rule(&changed, existing.version)
                    .await?
                {
                    return Err(ObjectError::VersionConflict);
                }
            }
        }
        applied
            .rules
            .push(format!("{}.{}", object.api_name, rule.name));
    }
    let described: BTreeSet<&str> = kept.iter().map(|rule| rule.name.as_str()).collect();
    for (name, rule) in &local {
        if !described.contains(name.as_str()) {
            if !tx.delete_validation_rule(rule.id, rule.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            applied.rules.push(format!("{}.{name}", object.api_name));
        }
    }
    Ok(())
}

/// Makes an object's quick actions on the replica the server's `remote`
/// ones that the replica can offer. An action is kept only when it would
/// pass the server's own check (`check_action`) against the object as the
/// replica defines it and, for a create action, against its child object
/// in service on the replica: its fields, presets and reference field are
/// all there. Any other is deferred: offering it would refuse every save,
/// and for a suspended plugin object would make records the server no
/// longer accepts. Running a kept action is an ordinary local create or
/// update, which the server checks again on push (ADR 0024).
async fn put_quick_actions(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    remote: &[RemoteQuickAction],
    actor: Uuid,
    applied: &mut DefinitionsApplied,
) -> Result<(), ObjectError> {
    let now = Utc::now();
    let local: BTreeMap<String, QuickAction> = tx
        .quick_actions(object.id)
        .await?
        .into_iter()
        .map(|action| (action.name.clone(), action))
        .collect();
    let mut kept: BTreeMap<String, QuickAction> = BTreeMap::new();
    for action in remote {
        let target = match action.target_object.as_deref() {
            Some(name) => match tx
                .object_by_api_name(name)
                .await?
                .filter(ObjectDefinition::is_active)
            {
                Some(target) => Some(target),
                None => {
                    applied
                        .deferred_quick_actions
                        .push(format!("{}.{}", object.api_name, action.name));
                    continue;
                }
            },
            None => None,
        };
        let existing = local.get(&action.name);
        let mut candidate = QuickAction {
            id: existing.map_or_else(Uuid::now_v7, |existing| existing.id),
            object_id: object.id,
            name: action.name.clone(),
            label: action.label.clone(),
            kind: action.kind,
            position: action.position,
            target_object_id: target.as_ref().map(|target| target.id),
            target_object: target.as_ref().map(|target| target.api_name.clone()),
            target_field: action.target_field.clone(),
            fields: action.fields.clone(),
            presets: action.presets.clone(),
            version: action.version.max(1),
            created_at: existing.map_or(now, |existing| existing.created_at),
            created_by: existing.map_or(Some(actor), |existing| existing.created_by),
            updated_at: now,
            updated_by: Some(actor),
        };
        let usable = (candidate.kind == QuickActionKind::Create) == target.is_some()
            && crate::quick_actions::check_action(&mut candidate, object, target.as_ref())
                .is_empty();
        if usable {
            kept.insert(candidate.name.clone(), candidate);
        } else {
            applied
                .deferred_quick_actions
                .push(format!("{}.{}", object.api_name, action.name));
        }
    }
    // Removals first, so a name the server reused for a different kind or
    // child object is free again.
    for (name, existing) in &local {
        let replaced = kept
            .get(name)
            .is_some_and(|action| !same_shape(existing, action));
        if kept.contains_key(name) && !replaced {
            continue;
        }
        if !tx
            .delete_quick_action(existing.id, existing.version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        if !replaced {
            applied
                .quick_actions
                .push(format!("{}.{name}", object.api_name));
        }
    }
    for (name, action) in &kept {
        match local.get(name) {
            Some(existing) if same_shape(existing, action) => {
                let unchanged = existing.label == action.label
                    && existing.position == action.position
                    && existing.fields == action.fields
                    && existing.presets == action.presets
                    && existing.version == action.version;
                if unchanged {
                    continue;
                }
                if !tx.update_quick_action(action, existing.version).await? {
                    return Err(ObjectError::VersionConflict);
                }
            }
            Some(_) => {
                let mut fresh = action.clone();
                fresh.id = Uuid::now_v7();
                fresh.created_at = now;
                fresh.created_by = Some(actor);
                tx.insert_quick_action(&fresh).await?;
            }
            None => tx.insert_quick_action(action).await?,
        }
        applied
            .quick_actions
            .push(format!("{}.{name}", object.api_name));
    }
    Ok(())
}

/// Whether two actions of one name are the same action: what the server
/// never changes after creating one (its kind, child object and reference
/// field) is the same.
fn same_shape(local: &QuickAction, remote: &QuickAction) -> bool {
    local.kind == remote.kind
        && local.target_object_id == remote.target_object_id
        && local.target_field == remote.target_field
}
