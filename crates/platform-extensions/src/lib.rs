#![forbid(unsafe_code)]
//! Kernel extension field service (ADR 0012).
//!
//! Plugins may add simple, typed fields to record resources owned by other
//! plugins. This crate is the only place that decides which extension fields
//! are active in a workspace, validates values written to them, filters values
//! returned to callers, and describes them to user interfaces.
//!
//! The crate is domain-neutral: it reasons about manifests, resource ids,
//! namespaces and permissions, never about what a field means. Hosts store
//! values in their own tables but can only apply a [`ValidatedExtensionPatch`],
//! which nothing outside this crate can construct.

use plugin_contract::{
    ui::DataClassification, DependencyReason, ExtensionFieldDecl, ExtensionFieldOption,
    ExtensionFieldType, Manifest, DEFAULT_EXTENSION_TEXT_LENGTH,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub mod layout;
pub use layout::{zones_for, FieldLayout, FieldPlacement, FieldZone};

/// Stored extension values for one record: plugin id, then field key.
pub type ExtensionValues = BTreeMap<String, BTreeMap<String, Value>>;

/// Largest encoded size of one record's extension values.
pub const MAX_STORED_EXTENSION_BYTES: usize = 32 * 1024;
/// Most fields one write may change.
pub const MAX_PATCH_ENTRIES: usize = 50;
/// Largest absolute number an extension number field stores.
const MAX_NUMBER_MAGNITUDE: f64 = 1e15;

/// Whether the caller may use a permission declared by a plugin.
///
/// Implementations answer from the caller's workspace permissions filtered to
/// what the plugin declares, so one plugin can never grant another plugin's
/// permissions.
pub trait ExtensionAccess {
    fn allows(&self, plugin_id: &str, permission: &str) -> bool;
}

impl<F> ExtensionAccess for F
where
    F: Fn(&str, &str) -> bool,
{
    fn allows(&self, plugin_id: &str, permission: &str) -> bool {
        self(plugin_id, permission)
    }
}

/// An extension field that is usable in this workspace.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveExtensionField {
    pub plugin_id: String,
    pub plugin_name: String,
    pub declaration: ExtensionFieldDecl,
}

impl ActiveExtensionField {
    /// The public field id, `<plugin id>.<key>`. Keys never contain dots, so
    /// the id splits unambiguously at its last dot.
    pub fn id(&self) -> String {
        field_id(&self.plugin_id, &self.declaration.key)
    }
}

/// Why a declared extension field is not active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PauseReason {
    /// The host plugin is not active in this workspace.
    HostInactive,
    /// The active host version does not satisfy the extension's requirement.
    HostIncompatible,
    /// The host no longer declares the target resource.
    TargetMissing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PausedExtensionField {
    pub plugin_id: String,
    pub host: String,
    pub target: String,
    pub key: String,
    pub reason: PauseReason,
}

/// The extension fields of one workspace, derived from its active manifests.
#[derive(Debug, Clone, Default)]
pub struct ExtensionCatalog {
    active: Vec<ActiveExtensionField>,
    paused: Vec<PausedExtensionField>,
}

impl ExtensionCatalog {
    /// Builds the catalog from the manifests active in a workspace.
    ///
    /// A field is active only when its host is also active, the host version
    /// still satisfies the extension's dependency requirement, and the host
    /// still declares the target resource. This is re-evaluated whenever the
    /// active set changes, so a host upgrade pauses incompatible fields
    /// without blocking the host.
    pub fn from_manifests<'a>(manifests: impl IntoIterator<Item = &'a Manifest>) -> Self {
        let manifests = manifests
            .into_iter()
            .map(|manifest| (manifest.id.as_str(), manifest))
            .collect::<BTreeMap<_, _>>();
        let mut catalog = Self::default();
        for manifest in manifests.values() {
            for declaration in &manifest.extensions.fields {
                match resolve(manifest, declaration, &manifests) {
                    Ok(()) => catalog.active.push(ActiveExtensionField {
                        plugin_id: manifest.id.clone(),
                        plugin_name: manifest.name.clone(),
                        declaration: declaration.clone(),
                    }),
                    Err(reason) => catalog.paused.push(PausedExtensionField {
                        plugin_id: manifest.id.clone(),
                        host: declaration.host.clone(),
                        target: declaration.target.clone(),
                        key: declaration.key.clone(),
                        reason,
                    }),
                }
            }
        }
        catalog
    }

    pub fn active(&self) -> &[ActiveExtensionField] {
        &self.active
    }

    pub fn paused(&self) -> &[PausedExtensionField] {
        &self.paused
    }

    /// Active fields added to one host resource.
    pub fn fields_for<'a>(
        &'a self,
        target: &'a str,
    ) -> impl Iterator<Item = &'a ActiveExtensionField> + 'a {
        self.active
            .iter()
            .filter(move |field| field.declaration.target == target)
    }

    fn field(&self, target: &str, id: &str) -> Option<&ActiveExtensionField> {
        let (plugin_id, key) = id.rsplit_once('.')?;
        self.active.iter().find(|field| {
            field.declaration.target == target
                && field.plugin_id == plugin_id
                && field.declaration.key == key
        })
    }

    /// Descriptors for the fields on `target` the caller may read.
    pub fn descriptors(
        &self,
        target: &str,
        access: &impl ExtensionAccess,
    ) -> Vec<ExtensionFieldDescriptor> {
        let mut descriptors = self
            .fields_for(target)
            .filter(|field| can_read(field, access))
            .map(|field| ExtensionFieldDescriptor {
                id: field.id(),
                plugin_id: field.plugin_id.clone(),
                plugin_name: field.plugin_name.clone(),
                target: field.declaration.target.clone(),
                key: field.declaration.key.clone(),
                label: field.declaration.label.trim().to_owned(),
                description: field.declaration.description.clone(),
                field_type: field.declaration.field_type,
                options: field.declaration.options.clone(),
                max_length: (field.declaration.field_type == ExtensionFieldType::Text)
                    .then(|| text_limit(&field.declaration)),
                classification: field.declaration.classification.clone(),
                writable: can_write(field, access),
            })
            .collect::<Vec<_>>();
        descriptors.sort_by(|left, right| {
            left.plugin_name
                .cmp(&right.plugin_name)
                .then_with(|| left.id.cmp(&right.id))
        });
        descriptors
    }

    /// Validates a write to extension fields on one record of `target`.
    ///
    /// `values` maps field ids to new values; `null` or an empty text clears
    /// a value. Fields the caller cannot read are reported as unknown so the
    /// error does not disclose them; readable fields the caller cannot write
    /// deny the whole write.
    pub fn validate_patch(
        &self,
        target: &str,
        values: &BTreeMap<String, Value>,
        access: &impl ExtensionAccess,
    ) -> Result<ValidatedExtensionPatch, ExtensionError> {
        if values.is_empty() {
            return Err(ExtensionError::issue(
                "values",
                "extension.values_empty",
                "at least one extension field value is required",
            ));
        }
        if values.len() > MAX_PATCH_ENTRIES {
            return Err(ExtensionError::issue(
                "values",
                "extension.values_too_many",
                "too many extension fields in one write",
            ));
        }
        let mut issues = Vec::new();
        let mut denied = false;
        let mut changes: ExtensionChanges = BTreeMap::new();
        for (id, value) in values {
            let Some(field) = self
                .field(target, id)
                .filter(|field| can_read(field, access))
            else {
                issues.push(ExtensionIssue::new(
                    id,
                    "extension.field_unknown",
                    "this field is not available on this record",
                ));
                continue;
            };
            if !can_write(field, access) {
                denied = true;
                continue;
            }
            match normalise_value(&field.declaration, value) {
                Ok(normalised) => {
                    changes
                        .entry(field.plugin_id.clone())
                        .or_default()
                        .insert(field.declaration.key.clone(), normalised);
                }
                Err((code, detail)) => issues.push(ExtensionIssue::new(id, code, detail)),
            }
        }
        if !issues.is_empty() {
            return Err(ExtensionError::Invalid(issues));
        }
        if denied {
            return Err(ExtensionError::PermissionDenied);
        }
        Ok(ValidatedExtensionPatch {
            target: target.to_owned(),
            changes,
        })
    }

    /// The values of `stored` the caller may read, keyed by field id.
    ///
    /// Values of inactive plugins, unreadable fields, and values that no
    /// longer match the current declaration are left out but stay stored.
    pub fn project(
        &self,
        target: &str,
        stored: &ExtensionValues,
        access: &impl ExtensionAccess,
    ) -> BTreeMap<String, Value> {
        self.fields_for(target)
            .filter(|field| can_read(field, access))
            .filter_map(|field| {
                let value = stored.get(&field.plugin_id)?.get(&field.declaration.key)?;
                matches!(normalise_value(&field.declaration, value), Ok(Some(_)))
                    .then(|| (field.id(), value.clone()))
            })
            .collect()
    }

    /// Validates an equality filter on one extension field, given the raw
    /// query-string value. Fields the caller cannot read cannot be filtered.
    pub fn validate_filter(
        &self,
        target: &str,
        id: &str,
        raw: &str,
        access: &impl ExtensionAccess,
    ) -> Result<ValidatedExtensionFilter, ExtensionError> {
        let Some(field) = self
            .field(target, id)
            .filter(|field| can_read(field, access))
        else {
            return Err(ExtensionError::issue(
                "extension_field",
                "extension.filter_unknown",
                "this field cannot be used to filter these records",
            ));
        };
        let candidate = match field.declaration.field_type {
            ExtensionFieldType::Number => raw
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            ExtensionFieldType::Boolean => match raw.trim() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            _ => Some(Value::String(raw.to_owned())),
        };
        let value = candidate
            .and_then(|value| normalise_value(&field.declaration, &value).ok().flatten())
            .ok_or_else(|| {
                ExtensionError::issue(
                    "extension_value",
                    "extension.filter_value_invalid",
                    "the filter value does not match the field type",
                )
            })?;
        Ok(ValidatedExtensionFilter {
            target: target.to_owned(),
            plugin_id: field.plugin_id.clone(),
            key: field.declaration.key.clone(),
            value,
        })
    }
}

/// What enabling a plugin would add to other plugins, for administrator
/// review before it is enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionPlan {
    pub plugin_id: String,
    pub hosts: Vec<String>,
    pub fields: Vec<PlannedExtensionField>,
    /// Permissions the plugin introduces for its extension fields; roles must
    /// grant them before users can see or change the fields.
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlannedExtensionField {
    pub id: String,
    pub host: String,
    pub target: String,
    pub label: String,
    pub field_type: ExtensionFieldType,
    pub read_permission: String,
    pub write_permission: String,
    /// Where values live: always with the host record for simple fields.
    pub storage: &'static str,
    /// `None` when the field would be active, otherwise why it would pause.
    pub paused: Option<PauseReason>,
}

impl ExtensionPlan {
    /// Describes `candidate` as if it were enabled next to the `active`
    /// manifests of the workspace.
    pub fn for_plugin<'a>(
        candidate: &Manifest,
        active: impl IntoIterator<Item = &'a Manifest>,
    ) -> Self {
        let manifests = active
            .into_iter()
            .filter(|manifest| manifest.id != candidate.id)
            .map(|manifest| (manifest.id.as_str(), manifest))
            .chain([(candidate.id.as_str(), candidate)])
            .collect::<BTreeMap<_, _>>();
        let mut hosts = BTreeSet::new();
        let mut permissions = BTreeSet::new();
        let fields = candidate
            .extensions
            .fields
            .iter()
            .map(|declaration| {
                hosts.insert(declaration.host.clone());
                permissions.insert(declaration.read_permission.clone());
                permissions.insert(declaration.write_permission.clone());
                PlannedExtensionField {
                    id: field_id(&candidate.id, &declaration.key),
                    host: declaration.host.clone(),
                    target: declaration.target.clone(),
                    label: declaration.label.trim().to_owned(),
                    field_type: declaration.field_type,
                    read_permission: declaration.read_permission.clone(),
                    write_permission: declaration.write_permission.clone(),
                    storage: "host-record",
                    paused: resolve(candidate, declaration, &manifests).err(),
                }
            })
            .collect();
        Self {
            plugin_id: candidate.id.clone(),
            hosts: hosts.into_iter().collect(),
            fields,
            permissions: permissions.into_iter().collect(),
        }
    }
}

/// A caller-safe description of one extension field.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtensionFieldDescriptor {
    pub id: String,
    pub plugin_id: String,
    pub plugin_name: String,
    pub target: String,
    pub key: String,
    pub label: String,
    pub description: Option<String>,
    pub field_type: ExtensionFieldType,
    pub options: Vec<ExtensionFieldOption>,
    pub max_length: Option<u16>,
    pub classification: Option<DataClassification>,
    pub writable: bool,
}

type ExtensionChanges = BTreeMap<String, BTreeMap<String, Option<Value>>>;

/// A kernel-validated change to one record's extension values.
///
/// Only [`ExtensionCatalog::validate_patch`] constructs it, so a host that
/// accepts this type can only store values the kernel has checked against the
/// active declarations and the caller's permissions.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedExtensionPatch {
    target: String,
    changes: ExtensionChanges,
}

impl ValidatedExtensionPatch {
    /// The host resource the patch was validated for.
    pub fn target(&self) -> &str {
        &self.target
    }

    /// Plugin namespaces the patch touches.
    pub fn namespaces(&self) -> impl Iterator<Item = &str> {
        self.changes.keys().map(String::as_str)
    }

    /// Applies the patch to a record's stored values. Other plugins'
    /// namespaces, including those of disabled plugins, are left untouched.
    pub fn apply(&self, stored: &mut ExtensionValues) -> Result<(), ExtensionError> {
        for (plugin_id, fields) in &self.changes {
            let namespace = stored.entry(plugin_id.clone()).or_default();
            for (key, value) in fields {
                match value {
                    Some(value) => {
                        namespace.insert(key.clone(), value.clone());
                    }
                    None => {
                        namespace.remove(key);
                    }
                }
            }
            if namespace.is_empty() {
                stored.remove(plugin_id);
            }
        }
        let size = serde_json::to_vec(stored)
            .map_err(|_| ExtensionError::Invariant)?
            .len();
        if size > MAX_STORED_EXTENSION_BYTES {
            return Err(ExtensionError::issue(
                "values",
                "extension.values_too_large",
                "this record cannot hold more extension data",
            ));
        }
        Ok(())
    }
}

/// A kernel-validated equality filter on one extension field.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidatedExtensionFilter {
    target: String,
    plugin_id: String,
    key: String,
    value: Value,
}

impl ValidatedExtensionFilter {
    pub fn target(&self) -> &str {
        &self.target
    }

    /// The JSON document a stored value must contain to match, for a jsonb
    /// containment (`@>`) predicate.
    pub fn containment(&self) -> Value {
        let mut field = serde_json::Map::new();
        field.insert(self.key.clone(), self.value.clone());
        let mut namespace = serde_json::Map::new();
        namespace.insert(self.plugin_id.clone(), Value::Object(field));
        Value::Object(namespace)
    }

    /// The extending plugin whose namespace holds the value.
    pub fn namespace(&self) -> &str {
        &self.plugin_id
    }

    /// The field key inside the namespace.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The JSON value a stored value must equal.
    pub fn value(&self) -> &Value {
        &self.value
    }

    /// A stable identity for cursor and cache keys.
    pub fn identity(&self) -> String {
        format!(
            "{}.{}={}",
            self.plugin_id,
            self.key,
            serde_json::to_string(&self.value).unwrap_or_default()
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExtensionIssue {
    pub field: String,
    pub code: &'static str,
    pub detail: &'static str,
}

impl ExtensionIssue {
    fn new(field: &str, code: &'static str, detail: &'static str) -> Self {
        Self {
            field: field.to_owned(),
            code,
            detail,
        }
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ExtensionError {
    #[error("extension values are invalid")]
    Invalid(Vec<ExtensionIssue>),
    #[error("permission denied")]
    PermissionDenied,
    #[error("extension values could not be encoded")]
    Invariant,
}

impl ExtensionError {
    fn issue(field: &str, code: &'static str, detail: &'static str) -> Self {
        Self::Invalid(vec![ExtensionIssue::new(field, code, detail)])
    }
}

fn field_id(plugin_id: &str, key: &str) -> String {
    format!("{plugin_id}.{key}")
}

fn resolve(
    manifest: &Manifest,
    declaration: &ExtensionFieldDecl,
    manifests: &BTreeMap<&str, &Manifest>,
) -> Result<(), PauseReason> {
    let host = manifests
        .get(declaration.host.as_str())
        .ok_or(PauseReason::HostInactive)?;
    let compatible = manifest
        .dependencies
        .get(&declaration.host)
        .is_some_and(|dependency| {
            dependency.reasons.contains(&DependencyReason::Extension)
                && dependency.version.matches(&host.version)
        });
    if !compatible {
        return Err(PauseReason::HostIncompatible);
    }
    if !host
        .ui
        .resources
        .iter()
        .any(|resource| resource.id == declaration.target)
    {
        return Err(PauseReason::TargetMissing);
    }
    Ok(())
}

fn can_read(field: &ActiveExtensionField, access: &impl ExtensionAccess) -> bool {
    access.allows(&field.plugin_id, &field.declaration.read_permission)
}

fn can_write(field: &ActiveExtensionField, access: &impl ExtensionAccess) -> bool {
    can_read(field, access) && access.allows(&field.plugin_id, &field.declaration.write_permission)
}

fn text_limit(declaration: &ExtensionFieldDecl) -> u16 {
    declaration
        .max_length
        .unwrap_or(DEFAULT_EXTENSION_TEXT_LENGTH)
}

/// Checks a value against its declaration. `Ok(None)` clears the value.
fn normalise_value(
    declaration: &ExtensionFieldDecl,
    value: &Value,
) -> Result<Option<Value>, (&'static str, &'static str)> {
    if value.is_null() {
        return Ok(None);
    }
    let mismatch = (
        "extension.value_type",
        "the value does not match the field type",
    );
    match declaration.field_type {
        ExtensionFieldType::Text => {
            let text = value.as_str().ok_or(mismatch)?.trim();
            if text.chars().any(char::is_control) {
                return Err((
                    "extension.value_unprintable",
                    "text must not contain control characters",
                ));
            }
            if text.chars().count() > usize::from(text_limit(declaration)) {
                return Err(("extension.value_too_long", "the text is too long"));
            }
            Ok((!text.is_empty()).then(|| Value::String(text.to_owned())))
        }
        ExtensionFieldType::Number => {
            let number = value.as_f64().ok_or(mismatch)?;
            if !number.is_finite() || number.abs() > MAX_NUMBER_MAGNITUDE {
                return Err(("extension.value_out_of_range", "the number is out of range"));
            }
            Ok(Some(value.clone()))
        }
        ExtensionFieldType::Boolean => value.as_bool().map(|_| Some(value.clone())).ok_or(mismatch),
        ExtensionFieldType::Date => {
            let text = value.as_str().ok_or(mismatch)?;
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .ok()
                .filter(|date| date.format("%Y-%m-%d").to_string() == text)
                .map(|_| Some(value.clone()))
                .ok_or((
                    "extension.value_date",
                    "dates must be written as YYYY-MM-DD",
                ))
        }
        ExtensionFieldType::Choice => {
            let choice = value.as_str().ok_or(mismatch)?;
            declaration
                .options
                .iter()
                .any(|option| option.value == choice)
                .then(|| Some(value.clone()))
                .ok_or(("extension.value_option", "choose one of the listed options"))
        }
    }
}

#[cfg(test)]
mod tests;
