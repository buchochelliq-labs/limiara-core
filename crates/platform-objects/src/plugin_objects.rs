//! Engine objects a plugin declares (ADR 0033 §2).
//!
//! A plugin's manifest may declare objects and fields in its namespace
//! (`[[objects]]`, see `plugin_contract::objects`). When a workspace enables
//! the plugin, the kernel installs them here, as a plugin installer caller
//! ([`ObjectCaller::plugin_installer`]): through the same validation,
//! evidence and metadata versioning as any administrator's change, with
//! `origin = plugin` and the plugin as `owner_plugin`.
//!
//! - **Install and upgrade** ([`ObjectService::install_plugin_objects`]) is
//!   idempotent: it adds objects and fields the manifest declares and the
//!   workspace lacks, and brings labels, options and flags of existing ones
//!   in line with the manifest. It never deletes a field the manifest no
//!   longer declares, and it refuses changes the engine never allows, such
//!   as a new field type. Everything happens in one transaction, so a
//!   refused upgrade leaves the workspace's definitions as they were.
//! - **Suspend and resume** ([`ObjectService::suspend_plugin_objects`],
//!   [`ObjectService::resume_plugin_objects`]) take the plugin's objects out
//!   of service while it is not enabled, and bring them back unchanged. A
//!   suspended object is not active, so every read treats it as missing.
//!
//! - **Related actions** a plugin object declares are kept with it on
//!   install and offered to the objects its related-to fields allow when it
//!   resumes (`crate::related_actions`).
//!
//! New objects are created suspended: the kernel resumes them once the
//! plugin is enabled, so a failure between installing and enabling never
//! shows objects of a plugin that is not enabled.

use crate::events::Change;
use crate::service::{MutationContext, ObjectService, OBJECTS_PER_WORKSPACE_MAX};
use crate::values::{self, LABEL_MAX};
use crate::{
    CreateField, DefinitionStatus, DeleteBehaviour, FieldDefinition, FieldType, ObjectDefinition,
    ObjectError, ObjectTx, Origin, PicklistOption, SharingModel, UpdateField, ValidationIssue,
};
use chrono::Utc;
use plugin_contract::{Manifest, ObjectDecl, ObjectFieldDecl};
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

/// The manifest's declarations do not pass validation.
pub const PLUGIN_OBJECTS_INVALID: &str = "plugin.objects_invalid";
/// Another object already uses a declared key prefix in the workspace.
pub const PLUGIN_KEY_PREFIX_TAKEN: &str = "plugin.objects_key_prefix_taken";
/// A declared object's key prefix differs from the one it was installed
/// with; public ids carry it, so it never changes.
pub const PLUGIN_KEY_PREFIX_CHANGED: &str = "plugin.objects_key_prefix_changed";
/// A declared API name belongs to a definition the plugin does not own, or
/// to one that was deleted.
pub const PLUGIN_NAME_TAKEN: &str = "plugin.objects_name_taken";
/// A declared field's type differs from the installed field's.
pub const PLUGIN_FIELD_TYPE_CHANGED: &str = "plugin.objects_field_type_changed";
/// A declared field changes what the engine fixes once a field exists:
/// uniqueness, decimal places, a lookup's target or delete behaviour.
pub const PLUGIN_FIELD_INCOMPATIBLE: &str = "plugin.objects_field_incompatible";

/// What an installation changed, by API name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PluginObjectsInstalled {
    pub created_objects: Vec<String>,
    pub updated_objects: Vec<String>,
    /// Fields as `object/field`.
    pub created_fields: Vec<String>,
    pub updated_fields: Vec<String>,
}

impl PluginObjectsInstalled {
    pub fn is_empty(&self) -> bool {
        self.created_objects.is_empty()
            && self.updated_objects.is_empty()
            && self.created_fields.is_empty()
            && self.updated_fields.is_empty()
    }
}

impl ObjectService {
    fn require_installer(&self, plugin_id: &str) -> Result<(), ObjectError> {
        if self.caller().installer() == Some(plugin_id) {
            Ok(())
        } else {
            Err(ObjectError::PermissionDenied)
        }
    }

    /// Creates or migrates the objects `manifest` declares in the caller's
    /// workspace. Only a plugin installer for `manifest.id` may. Objects it
    /// creates start suspended; [`Self::resume_plugin_objects`] brings them
    /// into service once the plugin is enabled.
    pub async fn install_plugin_objects(
        &self,
        manifest: &Manifest,
        context: MutationContext,
    ) -> Result<PluginObjectsInstalled, ObjectError> {
        self.require_installer(&manifest.id)?;
        let plugin_id = manifest.id.as_str();
        plugin_contract::objects::validate_objects(
            plugin_id,
            &manifest.objects,
            manifest.dependencies.keys().map(String::as_str),
        )
        .map_err(|issue| {
            ObjectError::invalid(
                match issue.field {
                    Some(field) => format!("{}/{}", issue.object, field),
                    None => issue.object,
                },
                PLUGIN_OBJECTS_INVALID,
                issue.reason,
            )
        })?;
        let mut installed = PluginObjectsInstalled::default();
        if manifest.objects.is_empty() {
            return Ok(installed);
        }
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        // Objects first, so fields can point at any of them.
        let mut prefixes = tx.key_prefixes().await?;
        for declared in &manifest.objects {
            self.put_plugin_object(
                tx.as_mut(),
                plugin_id,
                declared,
                &mut prefixes,
                &mut installed,
                context,
            )
            .await?;
        }
        for declared in &manifest.objects {
            let api_name = format!("{plugin_id}:{}", declared.name);
            for field in &declared.fields {
                // Reloaded per field, so positions and limits count the
                // fields added just before.
                let object = tx
                    .object_by_api_name(&api_name)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                self.put_plugin_field(
                    tx.as_mut(),
                    plugin_id,
                    &object,
                    field,
                    &mut installed,
                    context,
                )
                .await?;
            }
            // Related actions name the fields just installed. They are
            // offered once the objects are in service, when resuming.
            let object = tx
                .object_by_api_name(&api_name)
                .await?
                .ok_or(ObjectError::Invariant)?;
            self.put_plugin_related_actions(tx.as_mut(), plugin_id, &object, declared)
                .await?;
        }
        tx.commit().await?;
        Ok(installed)
    }

    async fn put_plugin_object(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        declared: &ObjectDecl,
        prefixes: &mut Vec<String>,
        installed: &mut PluginObjectsInstalled,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let api_name = format!("{plugin_id}:{}", declared.name);
        let mut issues = Vec::new();
        let mut checked = |field: &str, value: &str| {
            values::label(field, value, LABEL_MAX)
                .map_err(|issue| issues.push(issue))
                .unwrap_or_default()
        };
        let label = checked("label", &declared.label);
        let plural_label = checked("plural_label", &declared.plural_label);
        let name_label = checked(
            "name_label",
            declared.name_label.as_deref().unwrap_or("Name"),
        );
        let description = values::description(declared.description.as_deref())
            .map_err(|issue| issues.push(issue))
            .ok()
            .flatten();
        if !issues.is_empty() {
            return Err(ObjectError::Validation(
                issues
                    .into_iter()
                    .map(|issue| {
                        ValidationIssue::new(
                            format!("{api_name}/{}", issue.field),
                            issue.code,
                            issue.detail,
                        )
                    })
                    .collect(),
            ));
        }
        let track_history = declared.track_history.unwrap_or(false);
        let searchable = declared.searchable.unwrap_or(true);
        let in_navigation = declared.in_navigation.unwrap_or(true);
        let now = Utc::now();
        match tx.object_by_api_name(&api_name).await? {
            None => {
                if prefixes.contains(&declared.key_prefix) {
                    return Err(ObjectError::Conflict(PLUGIN_KEY_PREFIX_TAKEN));
                }
                if prefixes.len() >= OBJECTS_PER_WORKSPACE_MAX {
                    return Err(ObjectError::Conflict("objects.limit_reached"));
                }
                let object = ObjectDefinition {
                    id: Uuid::now_v7(),
                    api_name: api_name.clone(),
                    label,
                    plural_label,
                    description,
                    origin: Origin::Plugin,
                    owner_plugin: Some(plugin_id.to_owned()),
                    key_prefix: declared.key_prefix.clone(),
                    name_label,
                    // The workspace owns sharing once the object exists.
                    sharing_model: declared
                        .sharing_model
                        .and_then(|model| SharingModel::parse(model.as_str()))
                        .unwrap_or_default(),
                    grant_hierarchy: true,
                    track_history,
                    searchable,
                    in_navigation,
                    status: DefinitionStatus::Active,
                    // Out of service until the plugin is enabled.
                    suspended: true,
                    version: 1,
                    created_at: now,
                    created_by: Some(self.caller().user_id()),
                    updated_at: now,
                    updated_by: Some(self.caller().user_id()),
                    fields: Vec::new(),
                };
                tx.insert_object(&object).await?;
                prefixes.push(object.key_prefix.clone());
                self.record_change(tx, context, Change::ObjectCreated(&object))
                    .await?;
                installed.created_objects.push(api_name);
            }
            Some(existing) => {
                if existing.status != DefinitionStatus::Active
                    || existing.origin != Origin::Plugin
                    || existing.owner_plugin.as_deref() != Some(plugin_id)
                {
                    return Err(ObjectError::Conflict(PLUGIN_NAME_TAKEN));
                }
                if existing.key_prefix != declared.key_prefix {
                    return Err(ObjectError::Conflict(PLUGIN_KEY_PREFIX_CHANGED));
                }
                let mut object = existing.clone();
                object.label = label;
                object.plural_label = plural_label;
                object.description = description;
                object.name_label = name_label;
                object.track_history = track_history;
                object.searchable = searchable;
                object.in_navigation = in_navigation;
                if object == existing {
                    return Ok(());
                }
                object.version += 1;
                object.updated_at = now;
                object.updated_by = Some(self.caller().user_id());
                if !tx.update_object(&object, existing.version).await? {
                    return Err(ObjectError::VersionConflict);
                }
                self.record_change(tx, context, Change::ObjectUpdated(&object))
                    .await?;
                installed.updated_objects.push(api_name);
            }
        }
        Ok(())
    }

    async fn put_plugin_field(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        object: &ObjectDefinition,
        declared: &ObjectFieldDecl,
        installed: &mut PluginObjectsInstalled,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let api_name = format!("{plugin_id}:{}", declared.name);
        let qualify = |error: ObjectError| match error {
            ObjectError::Validation(issues) => ObjectError::Validation(
                issues
                    .into_iter()
                    .map(|issue| {
                        ValidationIssue::new(
                            format!("{}/{api_name}/{}", object.api_name, issue.field),
                            issue.code,
                            issue.detail,
                        )
                    })
                    .collect(),
            ),
            other => other,
        };
        let input = create_input(declared)?;
        let Some(existing) = object.field(&api_name).cloned() else {
            Box::pin(self.insert_field_in(
                tx,
                object,
                Some(api_name.clone()),
                Origin::Plugin,
                input,
                Vec::new(),
                context,
            ))
            .await
            .map_err(qualify)?;
            installed
                .created_fields
                .push(format!("{}/{api_name}", object.api_name));
            return Ok(());
        };
        if existing.origin != Origin::Plugin {
            return Err(ObjectError::Conflict(PLUGIN_NAME_TAKEN));
        }
        if existing.field_type != input.field_type {
            return Err(ObjectError::Conflict(PLUGIN_FIELD_TYPE_CHANGED));
        }
        let Some(update) = field_update(&existing, &input)? else {
            return Ok(());
        };
        Box::pin(self.change_field_in(tx, object, existing, update, context))
            .await
            .map_err(qualify)?;
        installed
            .updated_fields
            .push(format!("{}/{api_name}", object.api_name));
        Ok(())
    }

    /// Takes the plugin's objects out of service in the caller's workspace,
    /// keeping their definitions, records and permissions. Only a plugin
    /// installer for `plugin_id` may. Returns the objects it suspended.
    pub async fn suspend_plugin_objects(
        &self,
        plugin_id: &str,
        context: MutationContext,
    ) -> Result<Vec<String>, ObjectError> {
        self.set_plugin_objects_suspended(plugin_id, true, context)
            .await
    }

    /// Brings the plugin's objects back into service, as they were. Only a
    /// plugin installer for `plugin_id` may. Returns the objects it resumed.
    pub async fn resume_plugin_objects(
        &self,
        plugin_id: &str,
        context: MutationContext,
    ) -> Result<Vec<String>, ObjectError> {
        self.set_plugin_objects_suspended(plugin_id, false, context)
            .await
    }

    async fn set_plugin_objects_suspended(
        &self,
        plugin_id: &str,
        suspended: bool,
        context: MutationContext,
    ) -> Result<Vec<String>, ObjectError> {
        self.require_installer(plugin_id)?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let mut changed = Vec::new();
        for existing in tx.objects_owned_by(plugin_id).await? {
            if existing.suspended == suspended {
                continue;
            }
            let mut object = existing.clone();
            object.suspended = suspended;
            object.version += 1;
            object.updated_at = Utc::now();
            object.updated_by = Some(self.caller().user_id());
            if !tx.update_object(&object, existing.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            self.record_change(
                tx.as_mut(),
                context,
                Change::ObjectSuspended(&object, suspended),
            )
            .await?;
            changed.push(object.api_name);
        }
        if !suspended {
            // Objects that entered a related-to allowlist while the plugin
            // was not enabled, or that a new declaration names, are offered
            // its related actions now (ADR 0033 §3).
            let owned = tx.objects_owned_by(plugin_id).await?;
            for owned in &owned {
                let object = tx
                    .object_by_id(owned.id)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                self.offer_related_actions(tx.as_mut(), &object, None, context)
                    .await?;
            }
            // Other plugins' related actions that could not reach these
            // objects while they were out of service reach them now.
            let owned_ids: BTreeSet<Uuid> = owned.iter().map(|object| object.id).collect();
            let mut declaring = BTreeSet::new();
            for owned in &owned {
                for field in tx.fields_referencing_object(owned.id).await? {
                    if field.field_type == FieldType::RelatedTo
                        && !owned_ids.contains(&field.object_id)
                    {
                        declaring.insert((field.object_id, field.id));
                    }
                }
            }
            for (object_id, field_id) in declaring {
                if let Some(object) = tx.object_by_id(object_id).await? {
                    self.offer_related_actions(tx.as_mut(), &object, Some(field_id), context)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(changed)
    }
}

/// The engine input a declared field stands for.
fn create_input(declared: &ObjectFieldDecl) -> Result<CreateField, ObjectError> {
    let field_type =
        FieldType::parse(declared.field_type.as_str()).ok_or(ObjectError::Invariant)?;
    Ok(CreateField {
        name: declared.name.clone(),
        label: declared.label.clone(),
        field_type,
        description: declared.description.clone(),
        required: declared.required,
        unique: declared.unique,
        indexed: declared.indexed,
        searchable: declared.searchable,
        track_history: declared.track_history,
        max_length: declared.max_length,
        scale: declared.scale,
        options: declared
            .options
            .iter()
            .map(|option| PicklistOption {
                value: option.value.trim().to_owned(),
                label: option.label.trim().to_owned(),
                active: true,
                default: option.default,
            })
            .collect(),
        default_value: None,
        reference_object: declared.reference_object.clone(),
        reference_objects: (field_type == FieldType::RelatedTo)
            .then(|| declared.reference_objects.clone()),
        delete_behaviour: declared
            .delete_behaviour
            .and_then(|behaviour| DeleteBehaviour::parse(behaviour.as_str())),
        lookup_filter: None,
        formula: None,
        formula_type: None,
        rollup: None,
    })
}

/// What an upgrade changes on an installed field, or `None` when it is
/// already as declared. Refuses what the engine fixes once a field exists.
fn field_update(
    existing: &FieldDefinition,
    declared: &CreateField,
) -> Result<Option<UpdateField>, ObjectError> {
    let shape = values::field_shape_for(declared, true).map_err(ObjectError::Validation)?;
    let reference_changed = declared.reference_object.as_deref()
        != existing.reference_object.as_deref()
        && existing.field_type == FieldType::Lookup;
    if shape.unique != existing.unique
        || shape.scale != existing.scale
        || shape.delete_behaviour != existing.delete_behaviour
        || reference_changed
    {
        return Err(ObjectError::Conflict(PLUGIN_FIELD_INCOMPATIBLE));
    }
    let mut update = UpdateField {
        expected_version: existing.version,
        label: None,
        description: None,
        required: None,
        indexed: None,
        searchable: None,
        track_history: None,
        max_length: None,
        options: None,
        default_value: None,
        position: None,
        formula: None,
        lookup_filter: None,
        reference_objects: None,
    };
    let mut changed = false;
    let label = declared.label.trim();
    if label != existing.label {
        update.label = Some(label.to_owned());
        changed = true;
    }
    let description = declared
        .description
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if description != existing.description.as_deref() {
        update.description = Some(description.map(str::to_owned));
        changed = true;
    }
    if declared.required != existing.required {
        update.required = Some(declared.required);
        changed = true;
    }
    if (declared.indexed || shape.unique) != existing.indexed {
        update.indexed = Some(declared.indexed);
        changed = true;
    }
    let searchable = declared.searchable && declared.field_type.is_textual();
    if searchable != existing.searchable {
        update.searchable = Some(declared.searchable);
        changed = true;
    }
    if declared.track_history != existing.track_history {
        update.track_history = Some(declared.track_history);
        changed = true;
    }
    if shape.max_length != existing.max_length {
        update.max_length = shape.max_length;
        changed = true;
    }
    // Options the manifest no longer declares stay, inactive, so records
    // keep their values (ADR 0033 §2).
    if matches!(
        existing.field_type,
        FieldType::Picklist | FieldType::MultiPicklist
    ) {
        let mut options = shape.options.clone();
        for kept in &existing.options {
            if !options
                .iter()
                .any(|option| option.value.eq_ignore_ascii_case(&kept.value))
            {
                options.push(PicklistOption {
                    active: false,
                    default: false,
                    ..kept.clone()
                });
            }
        }
        if options != existing.options {
            update.options = Some(options);
            changed = true;
        }
    }
    // A related-to field's allowlist is the workspace's once the field
    // exists: the manifest's list is only the starting value, and upgrades
    // never change it (ADR 0033 §2).
    Ok(changed.then_some(update))
}
