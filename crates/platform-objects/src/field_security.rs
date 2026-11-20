//! Field-level permissions (G5 of ADR 0035, #316).
//!
//! A field may name a `read_permission` and a `write_permission`, both
//! permission ids from the kernel RBAC catalog. A member without the read
//! permission never sees the field: not in records, queries, search,
//! history, the change feed, related lists, timelines or replicas, and a
//! query or write that names it is answered exactly as if the field did not
//! exist (ADR 0019 §3). A member who reads a field but lacks its write
//! permission has their changes to it refused with
//! [`FIELD_NOT_WRITABLE`]. Writing a field always needs reading it.
//!
//! Formula fields and roll-up summaries are evaluated with full access and
//! hidden whenever they depend on a field the member cannot read, so a
//! derived value never leaks a hidden one.
//!
//! [`FieldSecurity::field`] is the one place a field's access is decided.
//! Role-based field-level security (#316) extends it with per-role grants
//! without changing any caller.
//!
//! System context bypasses the check, as it bypasses sharing (ADR 0016,
//! ADR 0022 §3): the kernel, a plugin's installation, the migrator (G4,
//! once it lands, through the same system paths), automations, validation
//! rules, lookup filters and roll-up summaries kept current read and write
//! every field. Member permissions never bypass it: `records.view_all` and
//! `records.modify_all` widen record access, not field access.

use crate::formula::{FieldRef, Leaf, Node};
use crate::service::ObjectService;
use crate::{
    FieldDefinition, FieldType, ObjectAccess, ObjectCaller, ObjectDefinition, ObjectError,
    ObjectTx, Record, ValidationIssue,
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// A write changes a field the member may read but not change.
pub const FIELD_NOT_WRITABLE: &str = "records.field_not_writable";
/// A field permission is not a permission id, or not one the field takes.
pub const FIELD_PERMISSION_INVALID: &str = "objects.field_permission_invalid";
/// A field permission names no permission in the workspace's catalog.
pub const FIELD_PERMISSION_UNKNOWN: &str = "objects.field_permission_unknown";
/// Longest permission id a field may name.
pub const PERMISSION_ID_MAX: usize = 120;

/// What a caller may do with one field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FieldAccess {
    pub read: bool,
    pub edit: bool,
}

impl FieldAccess {
    pub const FULL: Self = Self {
        read: true,
        edit: true,
    };
}

/// Decides what a caller may do with each field. It is resolved with the
/// caller's object access ([`ObjectAccess`]), so every engine path that
/// reads or writes as the caller applies it.
#[derive(Debug, Clone)]
pub struct FieldSecurity {
    /// The caller's system permissions; `None` in system context, where
    /// every field is readable and writable.
    permissions: Option<BTreeSet<String>>,
}

impl Default for FieldSecurity {
    /// Holds no permissions: a field that names one is hidden.
    fn default() -> Self {
        Self {
            permissions: Some(BTreeSet::new()),
        }
    }
}

impl FieldSecurity {
    /// System context: every field.
    pub fn system() -> Self {
        Self { permissions: None }
    }

    /// The member's field access. A plugin installer works on definitions
    /// only and is system context.
    pub fn for_caller(caller: &ObjectCaller) -> Self {
        if caller.installer().is_some() {
            return Self::system();
        }
        Self {
            permissions: Some(caller.permissions().clone()),
        }
    }

    pub fn is_system(&self) -> bool {
        self.permissions.is_none()
    }

    /// The one check: whether the caller may read and change `field`.
    /// Role-based field-level security (#316) adds its grants here.
    pub fn field(&self, field: &FieldDefinition) -> FieldAccess {
        let Some(held) = &self.permissions else {
            return FieldAccess::FULL;
        };
        let holds = |permission: &Option<String>| {
            permission
                .as_deref()
                .is_none_or(|permission| held.contains(permission))
        };
        let read = holds(&field.read_permission);
        FieldAccess {
            read,
            edit: read && holds(&field.write_permission),
        }
    }
}

/// Checks a permission id a field names: lowercase dotted segments such as
/// `contacts.people.custom-fields.read`. Blank clears it.
pub fn permission_id(
    key: &'static str,
    value: Option<&str>,
) -> Result<Option<String>, ValidationIssue> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let segment = |segment: &str| {
        !segment.is_empty()
            && segment.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
    };
    let mut segments = value.split('.');
    let starts_with_letter = value.as_bytes()[0].is_ascii_lowercase();
    let valid = value.len() <= PERMISSION_ID_MAX
        && starts_with_letter
        && value.contains('.')
        && segments.all(segment);
    if valid {
        Ok(Some(value.to_owned()))
    } else {
        Err(ValidationIssue::new(
            key,
            FIELD_PERMISSION_INVALID,
            format!(
                "use a permission id of up to {PERMISSION_ID_MAX} characters, such as objects.manage"
            ),
        ))
    }
}

/// Checks the permissions a field is given against its type: formula and
/// roll-up summary values are computed, never written, so they take a read
/// permission only.
pub(crate) fn check_permissions(field: &FieldDefinition) -> Result<(), ValidationIssue> {
    if field.write_permission.is_some()
        && matches!(field.field_type, FieldType::Formula | FieldType::Rollup)
    {
        return Err(ValidationIssue::new(
            "write_permission",
            FIELD_PERMISSION_INVALID,
            "formula and roll-up summary fields are never written; give them a read permission only",
        ));
    }
    Ok(())
}

/// Whether a read of `object` may need fields hidden: some field names a
/// read permission, or a formula or roll-up summary may depend on one
/// elsewhere.
pub(crate) fn may_hide(object: &ObjectDefinition) -> bool {
    object.fields.iter().any(|field| {
        field.is_active()
            && (field.read_permission.is_some()
                || matches!(field.field_type, FieldType::Formula | FieldType::Rollup))
    })
}

/// `object` without the `hidden` fields, so that everything resolved against
/// it treats them as missing.
pub(crate) fn visible(object: &ObjectDefinition, hidden: &BTreeSet<String>) -> ObjectDefinition {
    if hidden.is_empty() {
        return object.clone();
    }
    let mut visible = object.clone();
    visible
        .fields
        .retain(|field| !hidden.contains(&field.api_name));
    // A member who cannot read the path field sees no path or board by it.
    if visible
        .path_field
        .as_ref()
        .is_some_and(|field| hidden.contains(field))
    {
        visible.path_field = None;
    }
    visible
}

/// Removes the values of hidden fields from a record.
pub(crate) fn strip(record: &mut Record, hidden: &BTreeSet<String>) {
    if !hidden.is_empty() {
        record.values.retain(|key, _| !hidden.contains(key));
    }
}

/// Every field reference in a compiled formula.
fn references<'a>(node: &'a Node, out: &mut Vec<&'a FieldRef>) {
    match node {
        Node::Const(_) => {}
        Node::Field(field) | Node::Prior(field) | Node::Changed(field) => out.push(field),
        Node::Unary(_, operand) => references(operand, out),
        Node::Binary(_, left, right) => {
            references(left, out);
            references(right, out);
        }
        Node::Call(_, arguments) => arguments
            .iter()
            .for_each(|argument| references(argument, out)),
        Node::Matches(subject, _) => references(subject, out),
    }
}

/// Whether a roll-up summary reads a field the caller cannot: the details'
/// relationship field or the field it summarises.
async fn rollup_hidden(
    tx: &mut dyn ObjectTx,
    access: &ObjectAccess,
    field: &FieldDefinition,
    details: &mut BTreeMap<String, Option<ObjectDefinition>>,
) -> Result<bool, ObjectError> {
    let Some(rollup) = &field.rollup else {
        return Ok(false);
    };
    if !details.contains_key(&rollup.object) {
        let loaded = tx.object_by_api_name(&rollup.object).await?;
        details.insert(rollup.object.clone(), loaded);
    }
    let Some(Some(detail)) = details.get(&rollup.object) else {
        return Ok(false);
    };
    Ok(detail
        .fields
        .iter()
        .filter(|candidate| {
            candidate.id == rollup.relationship_id || Some(candidate.id) == rollup.field_id
        })
        .any(|candidate| !access.field(candidate).read))
}

impl ObjectService {
    /// The API names of `object`'s fields the caller may not read: those
    /// whose read permission they lack, formula fields that read such a
    /// field (on the record, through a lookup, or through another formula
    /// field), and roll-up summaries of one. Empty in system context.
    pub(crate) async fn hidden_fields(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
    ) -> Result<BTreeSet<String>, ObjectError> {
        if access.fields().is_system() || !may_hide(object) {
            return Ok(BTreeSet::new());
        }
        let mut hidden: BTreeSet<String> = object
            .fields
            .iter()
            .filter(|field| field.is_active() && !access.field(field).read)
            .map(|field| field.api_name.clone())
            .collect();
        let mut details = BTreeMap::new();
        let rollups: Vec<&FieldDefinition> = object
            .fields
            .iter()
            .filter(|field| {
                field.is_active()
                    && field.field_type == FieldType::Rollup
                    && !hidden.contains(&field.api_name)
            })
            .collect();
        for field in rollups {
            if rollup_hidden(tx, access, field, &mut details).await? {
                hidden.insert(field.api_name.clone());
            }
        }
        let formulas: Vec<&FieldDefinition> = object
            .fields
            .iter()
            .filter(|field| {
                field.is_active()
                    && field.field_type == FieldType::Formula
                    && !hidden.contains(&field.api_name)
            })
            .collect();
        if formulas.is_empty() {
            return Ok(hidden);
        }
        let objects = crate::rules::formula_objects(tx, object).await?;
        let by_id: BTreeMap<Uuid, &FieldDefinition> = objects
            .values()
            .flat_map(|candidate| candidate.fields.iter())
            .map(|field| (field.id, field))
            .collect();
        let mut found = Vec::new();
        for field in formulas {
            let (Some(text), Some(kind)) = (&field.formula, field.formula_type) else {
                continue;
            };
            // A formula that no longer compiles has no value to leak.
            let Ok(formula) =
                crate::formula_fields::compile_field(text, kind, object, &objects, Some(field.id))
            else {
                continue;
            };
            let mut reads_hidden = formula
                .uses
                .iter()
                .filter_map(|id| by_id.get(id))
                .any(|used| !access.field(used).read);
            let mut refs = Vec::new();
            references(&formula.node, &mut refs);
            'refs: for reference in refs {
                if reads_hidden {
                    break;
                }
                let mut current = object;
                for hop in &reference.hops {
                    let Some(lookup) = current.field(hop) else {
                        continue 'refs;
                    };
                    if !access.field(lookup).read
                        || (current.id == object.id && hidden.contains(hop))
                    {
                        reads_hidden = true;
                        continue 'refs;
                    }
                    let Some(next) = lookup
                        .reference_object_id
                        .and_then(|target| objects.get(&target))
                    else {
                        continue 'refs;
                    };
                    current = next;
                }
                let Leaf::Field { api_name, .. } = &reference.leaf else {
                    continue;
                };
                let Some(leaf) = current.field(api_name) else {
                    continue;
                };
                reads_hidden = if current.id == object.id {
                    hidden.contains(api_name)
                } else {
                    !access.field(leaf).read
                        || rollup_hidden(tx, access, leaf, &mut details).await?
                };
            }
            if reads_hidden {
                found.push(field.api_name.clone());
            }
        }
        hidden.extend(found);
        Ok(hidden)
    }

    /// Refuses a write naming a field the caller cannot read exactly as one
    /// naming a field the object does not have, before any value is looked
    /// at, so a refusal never tells a hidden value apart.
    pub(crate) fn check_readable_input(
        access: &ObjectAccess,
        object: &ObjectDefinition,
        body: &Map<String, Value>,
    ) -> Result<(), ObjectError> {
        let issues: Vec<ValidationIssue> = body
            .keys()
            .filter(|key| {
                object
                    .field(key)
                    .is_some_and(|field| !access.field(field).read)
            })
            .map(|key| {
                ValidationIssue::new(
                    key.clone(),
                    "records.field_unknown",
                    "this object has no such field",
                )
            })
            .collect();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ObjectError::Validation(issues))
        }
    }

    /// Refuses a write that sets a field the caller reads but may not
    /// change: on a create, any value given for one (its default still
    /// applies), on an update, a value that differs from the stored one.
    pub(crate) fn check_writable_input(
        access: &ObjectAccess,
        object: &ObjectDefinition,
        body: &Map<String, Value>,
        changed: &[String],
        creating: bool,
    ) -> Result<(), ObjectError> {
        let issues: Vec<ValidationIssue> = body
            .keys()
            .filter(|key| creating || changed.contains(key))
            .filter(|key| {
                object
                    .field(key)
                    .is_some_and(|field| !access.field(field).edit)
            })
            .map(|key| {
                ValidationIssue::new(
                    key.clone(),
                    FIELD_NOT_WRITABLE,
                    "you may read this field but not change it",
                )
            })
            .collect();
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ObjectError::Refused(FIELD_NOT_WRITABLE, issues))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_ids_are_dotted_lowercase_names() {
        assert_eq!(
            permission_id(
                "read_permission",
                Some(" contacts.people.custom-fields.read ")
            ),
            Ok(Some("contacts.people.custom-fields.read".into()))
        );
        assert_eq!(permission_id("read_permission", Some("  ")), Ok(None));
        assert_eq!(permission_id("read_permission", None), Ok(None));
        for invalid in [
            "objects",
            "Objects.manage",
            "1objects.manage",
            "objects..manage",
            "objects.manage.",
            "objects manage.x",
            &format!("a.{}", "b".repeat(PERMISSION_ID_MAX)),
        ] {
            assert!(
                permission_id("read_permission", Some(invalid)).is_err(),
                "{invalid}"
            );
        }
    }

    #[test]
    fn writing_needs_reading_and_system_context_reads_everything() {
        let now = chrono::Utc::now();
        let mut field = FieldDefinition {
            id: Uuid::nil(),
            object_id: Uuid::nil(),
            api_name: "custom:salary".into(),
            label: "Salary".into(),
            description: None,
            field_type: FieldType::Number,
            origin: crate::Origin::Workspace,
            required: false,
            unique: false,
            indexed: false,
            searchable: false,
            track_history: false,
            max_length: None,
            scale: None,
            options: Vec::new(),
            default_value: None,
            reference_object_id: None,
            reference_object: None,
            reference_object_ids: Vec::new(),
            reference_objects: Vec::new(),
            delete_behaviour: None,
            lookup_filter: None,
            formula: None,
            formula_type: None,
            rollup: None,
            read_permission: None,
            write_permission: None,
            position: 0,
            status: crate::DefinitionStatus::Active,
            version: 1,
            created_at: now,
            created_by: None,
            updated_at: now,
            updated_by: None,
        };
        field.read_permission = Some("hr.salary.read".into());
        field.write_permission = Some("hr.salary.write".into());
        let with = |permissions: &[&str]| FieldSecurity {
            permissions: Some(permissions.iter().map(|p| (*p).to_owned()).collect()),
        };
        assert_eq!(
            with(&[]).field(&field),
            FieldAccess {
                read: false,
                edit: false
            }
        );
        assert_eq!(
            with(&["hr.salary.write"]).field(&field),
            FieldAccess {
                read: false,
                edit: false
            },
            "a write permission alone gives nothing"
        );
        assert_eq!(
            with(&["hr.salary.read"]).field(&field),
            FieldAccess {
                read: true,
                edit: false
            }
        );
        assert_eq!(
            with(&["hr.salary.read", "hr.salary.write"]).field(&field),
            FieldAccess::FULL
        );
        assert_eq!(FieldSecurity::system().field(&field), FieldAccess::FULL);
        assert!(
            !FieldSecurity::default().field(&field).read,
            "the default holds no permissions"
        );
        field.read_permission = None;
        field.write_permission = None;
        assert_eq!(FieldSecurity::default().field(&field), FieldAccess::FULL);
    }
}
