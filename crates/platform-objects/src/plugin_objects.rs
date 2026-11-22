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
//! - **Starting automations** a plugin object declares are added as
//!   workspace automations when the object is created, and left to the
//!   workspace's administrators afterwards (`crate::automation`).
//! - **Timeline sources** a plugin object declares are kept with it on
//!   install, replaced on upgrade and dropped with the declaration
//!   (`crate::timeline`).
//!
//! New objects are created suspended: the kernel resumes them once the
//! plugin is enabled, so a failure between installing and enabling never
//! shows objects of a plugin that is not enabled.
//!
//! An object the manifest marks `dormant` (ADR 0035 G11) is created
//! suspended and stays suspended when the plugin is enabled, until the
//! plugin wakes it ([`ObjectService::wake_plugin_objects`]); from then on it
//! follows the plugin like any other object. An upgrade never changes
//! whether an existing object is dormant.

use crate::events::Change;
use crate::service::{MutationContext, ObjectService, OBJECTS_PER_WORKSPACE_MAX};
use crate::values::{self, LABEL_MAX};
use crate::{
    CreateField, DefinitionStatus, DeleteBehaviour, FieldDefinition, FieldType, NameFrom,
    ObjectDefinition, ObjectError, ObjectTx, OptionOutcome, Origin, PicklistOption, SharingModel,
    UpdateField, ValidationIssue,
};
use crate::{DataClassification, FieldGovernance};
use chrono::Utc;
use plugin_contract::objects::{ObjectDataClassification, ObjectOptionOutcome};
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
/// An upgrade moves an object to another resource (ADR 0035 G6), which
/// would strand the fields other plugins added to it.
pub const PLUGIN_RESOURCE_CHANGED: &str = "plugin.objects_resource_changed";
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

/// What one installation has done so far, across its objects.
struct InstallProgress {
    /// Key prefixes the workspace's objects use, the new ones included.
    prefixes: Vec<String>,
    /// Existing objects whose composed name changed (ADR 0035 G17), whose
    /// records are renamed once every field is installed.
    renamed: Vec<String>,
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
        plugin_contract::objects::validate_objects_with(
            plugin_id,
            &manifest.objects,
            manifest.dependencies.keys().map(String::as_str),
            manifest.optional_dependencies.keys().map(String::as_str),
            manifest
                .permissions
                .iter()
                .map(|permission| permission.id.as_str()),
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
        if manifest.objects.is_empty() && manifest.extensions.fields.is_empty() {
            // A plugin that once extended others keeps its fields recorded.
            let mut tx = self.begin().await?;
            let known = tx
                .plugin_extensions()
                .await?
                .iter()
                .any(|(plugin, _, _)| plugin == plugin_id);
            if !known {
                tx.commit().await?;
                return Ok(installed);
            }
            tx.lock_metadata().await?;
            tx.put_plugin_extensions(plugin_id, &serde_json::json!([]))
                .await?;
            tx.commit().await?;
            return Ok(installed);
        }
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        // Objects first, so fields can point at any of them.
        let mut progress = InstallProgress {
            prefixes: tx.key_prefixes().await?,
            renamed: Vec::new(),
        };
        for declared in &manifest.objects {
            self.put_plugin_object(
                tx.as_mut(),
                plugin_id,
                declared,
                &mut progress,
                &mut installed,
                context,
            )
            .await?;
        }
        let renamed = progress.renamed;
        for declared in &manifest.objects {
            let api_name = format!("{plugin_id}:{}", declared.name);
            for field in &declared.fields {
                // Reloaded per field, so positions and limits count the
                // fields added just before.
                let object = tx
                    .object_by_api_name(&api_name)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                // An optional dependency's objects wait to join the
                // allowlist until they are in service (ADR 0035 G18).
                let (waiting, field) = split_optional(manifest, field);
                self.put_plugin_field(
                    tx.as_mut(),
                    plugin_id,
                    &object,
                    &field,
                    &mut installed,
                    context,
                )
                .await?;
                if field.field_type == plugin_contract::ObjectFieldType::RelatedTo {
                    let installed_field = tx
                        .object_by_api_name(&api_name)
                        .await?
                        .and_then(|object| {
                            object
                                .field(&format!("{plugin_id}:{}", field.name))
                                .map(|field| field.id)
                        })
                        .ok_or(ObjectError::Invariant)?;
                    tx.put_allowlist_offers(installed_field, &waiting).await?;
                }
            }
            // Related actions name the fields just installed. They are
            // offered once the objects are in service, when resuming.
            let object = tx
                .object_by_api_name(&api_name)
                .await?
                .ok_or(ObjectError::Invariant)?;
            self.put_plugin_related_actions(tx.as_mut(), plugin_id, &object, declared)
                .await?;
            self.put_plugin_timeline(tx.as_mut(), plugin_id, &object, declared)
                .await?;
        }
        // Composed names (ADR 0035 G17) are checked once every field exists;
        // an upgrade that changes one renames the object's records.
        for declared in &manifest.objects {
            if declared.name_from.is_none() {
                continue;
            }
            let api_name = format!("{plugin_id}:{}", declared.name);
            let object = tx
                .object_by_api_name(&api_name)
                .await?
                .ok_or(ObjectError::Invariant)?;
            if let Some(name_from) = &object.name_from {
                values::check_name_from(&object, name_from).map_err(|issue| {
                    ObjectError::invalid(
                        format!("{}/name_from", declared.name),
                        PLUGIN_OBJECTS_INVALID,
                        issue.detail,
                    )
                })?;
            }
            if renamed.contains(&api_name) {
                self.rename_records(tx.as_mut(), &object, context).await?;
            }
        }
        // Starting automations come with a new object only: afterwards they
        // belong to the workspace's administrators. They are checked once
        // every object's fields are installed, so a formula may follow a
        // lookup to an object declared later in the manifest.
        for declared in &manifest.objects {
            let api_name = format!("{plugin_id}:{}", declared.name);
            if !installed.created_objects.contains(&api_name) {
                continue;
            }
            let object = tx
                .object_by_api_name(&api_name)
                .await?
                .ok_or(ObjectError::Invariant)?;
            self.put_plugin_automations(tx.as_mut(), plugin_id, &object, declared, context)
                .await?;
        }
        // Waiting entries whose objects are already in service join now.
        self.offer_waiting_allowlists(tx.as_mut(), context).await?;
        // The plugin's fields on other plugins' objects (ADR 0035 G6), and
        // other plugins' fields on the objects it serves.
        if !manifest.extensions.fields.is_empty() {
            let declarations = serde_json::to_value(&manifest.extensions.fields)
                .map_err(|_| ObjectError::Invariant)?;
            tx.put_plugin_extensions(plugin_id, &declarations).await?;
        }
        self.apply_extensions(tx.as_mut(), &mut installed, context)
            .await?;
        tx.commit().await?;
        Ok(installed)
    }

    /// Brings every plugin's recorded extension declarations (ADR 0035 G6)
    /// onto the objects serving their target resources in this workspace,
    /// as fields in the extending plugin's namespace that it owns. A target
    /// no object serves yet waits; it is applied when its object is
    /// installed. Fields of a plugin that is not in service are added out
    /// of service, and changed without coming back into service.
    pub(crate) async fn apply_extensions(
        &self,
        tx: &mut dyn ObjectTx,
        installed: &mut PluginObjectsInstalled,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        for (plugin_id, declarations, suspended) in tx.plugin_extensions().await? {
            let declarations: Vec<plugin_contract::ExtensionFieldDecl> =
                serde_json::from_value(declarations).map_err(|_| ObjectError::Invariant)?;
            let mut added = false;
            for declared in &declarations {
                let Some(object) = tx.object_by_resource(&declared.target).await? else {
                    continue;
                };
                // Only the host may serve its resources.
                if object.owner_plugin.as_deref() != Some(declared.host.as_str())
                    || object.owner_plugin.as_deref() == Some(plugin_id.as_str())
                {
                    continue;
                }
                let field = extension_field(declared);
                let api_name = format!("{plugin_id}:{}", field.name);
                match tx.suspended_field(object.id, &api_name).await? {
                    Some(existing) => {
                        let input = create_input(&field)?;
                        if existing.field_type != input.field_type {
                            return Err(ObjectError::Conflict(PLUGIN_FIELD_TYPE_CHANGED));
                        }
                        if let Some(update) = field_update(&existing, &input)? {
                            Box::pin(self.change_field_in(tx, &object, existing, update, context))
                                .await?;
                            installed
                                .updated_fields
                                .push(format!("{}/{api_name}", object.api_name));
                        }
                    }
                    None => {
                        let new = object.field(&api_name).is_none();
                        self.put_plugin_field(tx, &plugin_id, &object, &field, installed, context)
                            .await?;
                        added |= new;
                    }
                }
            }
            if suspended && added {
                self.suspend_extension_fields(tx, &plugin_id, true, context)
                    .await?;
            }
        }
        Ok(())
    }

    /// Takes a plugin's fields on other plugins' objects (ADR 0035 G6) out
    /// of service or back into it, recording a change for each object.
    pub(crate) async fn suspend_extension_fields(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        suspended: bool,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        tx.set_plugin_extensions_suspended(plugin_id, suspended)
            .await?;
        for object_id in tx
            .set_extension_fields_suspended(plugin_id, suspended)
            .await?
        {
            if let Some(object) = tx.object_by_id(object_id).await? {
                // Edits made while the fields were out of service rebuilt
                // their records' index without them: rebuild it with them.
                if !suspended {
                    crate::service::reindex(tx, &object).await?;
                }
                self.record_change(tx, context, Change::ObjectUpdated(&object))
                    .await?;
            }
        }
        Ok(())
    }

    /// Adds each waiting allowlist entry (ADR 0035 G18) whose object is now
    /// in service to its related-to field, once, through the same change as
    /// an administrator's allowlist edit, which offers the field's related
    /// actions to it. An entry that cannot be added yet, because the field
    /// is gone or its allowlist is full, keeps waiting.
    pub(crate) async fn offer_waiting_allowlists(
        &self,
        tx: &mut dyn ObjectTx,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let waiting = tx.pending_allowlist_offers().await?;
        if waiting.is_empty() {
            return Ok(());
        }
        for (object_id, field_id, name) in waiting {
            let active = tx
                .object_by_api_name(&name)
                .await?
                .is_some_and(|target| target.is_active());
            if !active {
                continue;
            }
            // Loaded per entry, so each change sees the one before.
            let Some(object) = tx.object_by_id(object_id).await? else {
                continue;
            };
            let Some(field) = object
                .fields
                .iter()
                .find(|field| field.id == field_id && field.is_active())
                .cloned()
            else {
                continue;
            };
            if !field.reference_objects.contains(&name) {
                let mut names = field.reference_objects.clone();
                names.push(name.clone());
                let change: UpdateField = serde_json::from_value(serde_json::json!({
                    "expected_version": field.version,
                    "reference_objects": names,
                }))
                .map_err(|_| ObjectError::Invariant)?;
                match Box::pin(self.change_field_in(tx, &object, field, change, context)).await {
                    Ok(_) => {}
                    // A full allowlist: it waits for room.
                    Err(ObjectError::Validation(_)) => continue,
                    Err(other) => return Err(other),
                }
            }
            tx.mark_allowlist_offered(field_id, &name, Utc::now())
                .await?;
        }
        Ok(())
    }

    async fn put_plugin_object(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        declared: &ObjectDecl,
        progress: &mut InstallProgress,
        installed: &mut PluginObjectsInstalled,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let prefixes = &mut progress.prefixes;
        let renamed = &mut progress.renamed;
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
        // The manifest checked that it names one of the object's picklists,
        // which the installer puts in the same transaction.
        let path_field = declared
            .path_field
            .as_ref()
            .map(|field| format!("{plugin_id}:{field}"));
        // Checked against the installed fields once they all exist.
        let name_from = declared.name_from.as_ref().map(|decl| NameFrom {
            fields: decl
                .fields
                .iter()
                .map(|field| format!("{plugin_id}:{field}"))
                .collect(),
            separator: decl.separator.clone(),
        });
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
                    path_field,
                    name_from,
                    status: DefinitionStatus::Active,
                    // Out of service until the plugin is enabled.
                    suspended: true,
                    dormant: declared.dormant,
                    resource: declared.resource.clone(),
                    suspended_fields: Default::default(),
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
                // Other plugins' fields follow the resource an object
                // serves (ADR 0035 G6), so it may be declared, not moved.
                if existing.resource.is_some() && existing.resource != declared.resource {
                    return Err(ObjectError::Conflict(PLUGIN_RESOURCE_CHANGED));
                }
                let mut object = existing.clone();
                object.label = label;
                object.plural_label = plural_label;
                object.description = description;
                object.name_label = name_label;
                object.track_history = track_history;
                object.searchable = searchable;
                object.in_navigation = in_navigation;
                object.path_field = path_field;
                object.resource = declared.resource.clone();
                if object.name_from != name_from {
                    object.name_from = name_from;
                    renamed.push(api_name.clone());
                }
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

    /// Wakes dormant objects of the caller's plugin (ADR 0035 G11): each
    /// named object stops being dormant, for good, and comes into service
    /// with the plugin's other objects. Only a plugin installer for
    /// `plugin_id` may, and the kernel does so only while the plugin is
    /// enabled. Objects already awake are left alone; an object the plugin
    /// does not own is refused. Returns the objects woken.
    pub async fn wake_plugin_objects(
        &self,
        plugin_id: &str,
        objects: &[&str],
        context: MutationContext,
    ) -> Result<Vec<String>, ObjectError> {
        self.require_installer(plugin_id)?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let owned = tx.objects_owned_by(plugin_id).await?;
        let mut woken = Vec::new();
        for api_name in objects {
            let existing = owned
                .iter()
                .find(|object| object.api_name == *api_name)
                .ok_or(ObjectError::NotFound)?;
            if !existing.dormant {
                continue;
            }
            let mut object = existing.clone();
            object.dormant = false;
            object.version += 1;
            object.updated_at = Utc::now();
            object.updated_by = Some(self.caller().user_id());
            if !tx.update_object(&object, existing.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            self.record_change(tx.as_mut(), context, Change::ObjectWoken(&object))
                .await?;
            woken.push(object.api_name);
        }
        // Woken objects come into service in the same transaction, so a
        // failure leaves them dormant and waking can be repeated.
        if !woken.is_empty() {
            self.set_suspended_in(tx.as_mut(), plugin_id, false, context)
                .await?;
        }
        tx.commit().await?;
        Ok(woken)
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
        let changed = self
            .set_suspended_in(tx.as_mut(), plugin_id, suspended, context)
            .await?;
        tx.commit().await?;
        Ok(changed)
    }

    /// [`Self::set_plugin_objects_suspended`] inside a transaction already
    /// open, with the metadata lock held.
    async fn set_suspended_in(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        suspended: bool,
        context: MutationContext,
    ) -> Result<Vec<String>, ObjectError> {
        let mut changed = Vec::new();
        // The plugin's reports (ADR 0037 §5) follow its objects.
        tx.set_plugin_reports_suspended(plugin_id, suspended)
            .await?;
        // So do the fields it added to other plugins' objects (ADR 0035 G6).
        self.suspend_extension_fields(tx, plugin_id, suspended, context)
            .await?;
        for existing in tx.objects_owned_by(plugin_id).await? {
            // A dormant object stays out of service until its plugin wakes
            // it (ADR 0035 G11).
            if existing.suspended == suspended || (!suspended && existing.dormant) {
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
            self.record_change(tx, context, Change::ObjectSuspended(&object, suspended))
                .await?;
            changed.push(object.api_name);
        }
        if !suspended {
            // Optional allowlist entries naming objects that just came into
            // service join their fields (ADR 0035 G18).
            self.offer_waiting_allowlists(tx, context).await?;
            // Objects that entered a related-to allowlist while the plugin
            // was not enabled, or that a new declaration names, are offered
            // its related actions now (ADR 0033 §3).
            let owned = tx.objects_owned_by(plugin_id).await?;
            for owned in &owned {
                let object = tx
                    .object_by_id(owned.id)
                    .await?
                    .ok_or(ObjectError::Invariant)?;
                self.offer_related_actions(tx, &object, None, context)
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
                    self.offer_related_actions(tx, &object, Some(field_id), context)
                        .await?;
                }
            }
        }
        Ok(changed)
    }
}

/// Splits a related-to field's starting allowlist into the entries that
/// name an optional dependency's objects, which wait until those are in
/// service (ADR 0035 G18), and the field as installed now, without them.
fn split_optional(manifest: &Manifest, field: &ObjectFieldDecl) -> (Vec<String>, ObjectFieldDecl) {
    let mut field = field.clone();
    let (waiting, now): (Vec<String>, Vec<String>) =
        field.reference_objects.drain(..).partition(|target| {
            target.split_once(':').is_some_and(|(namespace, _)| {
                manifest.optional_dependencies.contains_key(namespace)
            })
        });
    field.reference_objects = now;
    (waiting, field)
}

/// The engine field an ADR 0012 extension declaration stands for (ADR 0035
/// G6): text up to 255 characters stays text and longer becomes long text,
/// numbers, booleans and dates keep their type, and a choice is a picklist.
/// The read and write permissions are the extending plugin's.
fn extension_field(declared: &plugin_contract::ExtensionFieldDecl) -> ObjectFieldDecl {
    use plugin_contract::ExtensionFieldType as Kind;
    use plugin_contract::ObjectFieldType as Type;
    let length = i32::from(
        declared
            .max_length
            .unwrap_or(plugin_contract::DEFAULT_EXTENSION_TEXT_LENGTH),
    );
    let field_type = match declared.field_type {
        Kind::Text if length <= values::TEXT_LIMIT => Type::Text,
        Kind::Text => Type::LongText,
        Kind::Number => Type::Number,
        Kind::Boolean => Type::Boolean,
        Kind::Date => Type::Date,
        Kind::Choice => Type::Picklist,
    };
    ObjectFieldDecl {
        name: declared.key.clone(),
        label: declared.label.clone(),
        field_type,
        description: declared.description.clone(),
        required: false,
        unique: false,
        indexed: false,
        searchable: false,
        track_history: false,
        max_length: matches!(field_type, Type::Text | Type::LongText).then_some(length),
        // ADR 0012 numbers take fractions: the engine's widest scale.
        scale: (field_type == Type::Number).then_some(values::SCALE_LIMIT),
        options: declared
            .options
            .iter()
            .map(|option| plugin_contract::objects::ObjectFieldOption {
                value: option.value.clone(),
                label: option.label.clone(),
                default: false,
                outcome: ObjectOptionOutcome::Open,
                weight: None,
            })
            .collect(),
        reference_object: None,
        reference_objects: Vec::new(),
        delete_behaviour: None,
        read_permission: Some(declared.read_permission.clone()),
        write_permission: Some(declared.write_permission.clone()),
        data_classification: None,
        purpose: None,
        retention_days: None,
        encrypted: false,
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
                outcome: match option.outcome {
                    ObjectOptionOutcome::Open => OptionOutcome::Open,
                    ObjectOptionOutcome::Positive => OptionOutcome::Positive,
                    ObjectOptionOutcome::Negative => OptionOutcome::Negative,
                },
                weight: option.weight,
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
        read_permission: declared.read_permission.clone(),
        write_permission: declared.write_permission.clone(),
        governance: FieldGovernance {
            data_classification: declared.data_classification.map(|class| match class {
                ObjectDataClassification::Public => DataClassification::Public,
                ObjectDataClassification::Internal => DataClassification::Internal,
                ObjectDataClassification::Personal => DataClassification::Personal,
                ObjectDataClassification::Sensitive => DataClassification::Sensitive,
            }),
            purpose: declared
                .purpose
                .as_deref()
                .map(|purpose| purpose.trim().to_owned()),
            retention_days: declared.retention_days,
            encrypted: declared.encrypted,
        },
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
        read_permission: None,
        write_permission: None,
        reference_objects: None,
        governance: None,
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
    // An email field without a limit allows the default (ADR 0035 G8), so a
    // manifest that drops its limit brings the field back to it, and an
    // omitted limit matches an installed default.
    let effective = |length: Option<i32>| match existing.field_type {
        FieldType::Email => length.or(Some(values::TEXT_DEFAULT_MAX)),
        _ => length,
    };
    if effective(shape.max_length) != effective(existing.max_length) {
        update.max_length = effective(shape.max_length);
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
    // The field's permissions are the plugin's, like the rest of it (G5).
    if declared.read_permission != existing.read_permission {
        update.read_permission = Some(declared.read_permission.clone());
        changed = true;
    }
    if declared.write_permission != existing.write_permission {
        update.write_permission = Some(declared.write_permission.clone());
        changed = true;
    }
    // So are its governance declarations (G9).
    if declared.governance != existing.governance {
        update.governance = Some(declared.governance.clone());
        changed = true;
    }
    // A related-to field's allowlist is the workspace's once the field
    // exists: the manifest's list is only the starting value, and upgrades
    // never change it (ADR 0033 §2).
    Ok(changed.then_some(update))
}
