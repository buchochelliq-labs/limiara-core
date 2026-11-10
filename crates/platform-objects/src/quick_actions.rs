//! Quick actions (ADR 0032): administrator-defined shortcuts on an object's
//! record page that update a few chosen fields or create a related child
//! record, with constant presets.
//!
//! An action is metadata like a validation rule (ADR 0021). It grants
//! nothing: running it goes through the normal create and update APIs with
//! the member's own permissions. Presets are checked against their field's
//! type when the action is saved, with the same value validation as record
//! writes. Deleting a field, a child object's reference field or an object
//! updates or deletes the actions that depend on it in the same transaction.

use crate::events::Change;
use crate::service::{MutationContext, ObjectService};
use crate::values;
use crate::{
    CreateQuickAction, FieldDefinition, FieldType, ObjectDefinition, ObjectError, ObjectTx,
    QuickAction, QuickActionKind, UpdateQuickAction, ValidationIssue,
};
use chrono::Utc;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Quick actions per object.
pub const QUICK_ACTIONS_PER_OBJECT_MAX: usize = 10;
/// Fields one action's form may ask for.
pub const QUICK_ACTION_FIELDS_MAX: usize = 20;
/// Presets one action may apply.
pub const QUICK_ACTION_PRESETS_MAX: usize = 20;
const LABEL_MAX: usize = 80;

fn issue(
    field: impl Into<String>,
    code: &'static str,
    detail: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue::new(field, code, detail)
}

/// Why `api_name` cannot be used in an action's form or presets, if it
/// cannot. The record name is addressed as `name`, as in the record API.
fn field_problem(
    form: &ObjectDefinition,
    target_field: Option<&str>,
    api_name: &str,
) -> Option<(&'static str, String)> {
    if api_name == "name" {
        return None;
    }
    let Some(field) = form.field(api_name) else {
        return Some((
            "quick_actions.field_unknown",
            format!("{} has no field {api_name}", form.api_name),
        ));
    };
    if matches!(field.field_type, FieldType::Formula | FieldType::Rollup) {
        return Some((
            "quick_actions.field_not_allowed",
            "formula and roll-up summary fields are computed and cannot be chosen or preset".into(),
        ));
    }
    if target_field == Some(api_name) {
        return Some((
            "quick_actions.field_not_allowed",
            "the reference field is set to the current record".into(),
        ));
    }
    None
}

/// Whether a create action on `object_id` may set `field` to its current
/// record: a lookup or master-detail field to it, or a related-to field
/// whose allowlist names it (ADR 0033 §1).
fn points_to(field: &FieldDefinition, object_id: Uuid) -> bool {
    match field.field_type {
        FieldType::RelatedTo => field.reference_object_ids.contains(&object_id),
        kind => kind.is_reference() && field.reference_object_id == Some(object_id),
    }
}

/// Checks an action against the object it belongs to and, for a create
/// action, the child object its form writes to. Presets are normalised in
/// place, as record values are.
pub(crate) fn check_action(
    action: &mut QuickAction,
    object: &ObjectDefinition,
    target: Option<&ObjectDefinition>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !crate::rules::valid_name(&action.name) {
        issues.push(issue(
            "name",
            "quick_actions.name_invalid",
            "use a lowercase letter, then lowercase letters, digits or underscores, up to 63",
        ));
    }
    let length = action.label.chars().count();
    if length == 0 || length > LABEL_MAX || action.label.chars().any(char::is_control) {
        issues.push(issue(
            "label",
            "quick_actions.label_invalid",
            format!("enter between 1 and {LABEL_MAX} printable characters"),
        ));
    }
    if action.position < 0 {
        issues.push(issue(
            "position",
            "quick_actions.position_invalid",
            "use a position of 0 or more",
        ));
    }
    let form = match action.kind {
        QuickActionKind::Update => Some(object),
        QuickActionKind::Create => target,
    };
    if let (QuickActionKind::Create, Some(target), Some(target_field)) =
        (action.kind, target, action.target_field.as_deref())
    {
        let points_here = target
            .field(target_field)
            .is_some_and(|field| points_to(field, object.id));
        if !points_here {
            issues.push(issue(
                "target_field",
                "quick_actions.target_field_invalid",
                format!(
                    "use a lookup or master-detail field on {} that points to {}, or a related-to field that allows it",
                    target.api_name, object.api_name
                ),
            ));
        }
    }
    if action.fields.is_empty() || action.fields.len() > QUICK_ACTION_FIELDS_MAX {
        issues.push(issue(
            "fields",
            "quick_actions.fields_invalid",
            format!("choose 1 to {QUICK_ACTION_FIELDS_MAX} fields"),
        ));
    }
    if action.presets.len() > QUICK_ACTION_PRESETS_MAX {
        issues.push(issue(
            "presets",
            "quick_actions.presets_invalid",
            format!("set at most {QUICK_ACTION_PRESETS_MAX} presets"),
        ));
    }
    let Some(form) = form else {
        return issues;
    };
    let target_field = action.target_field.as_deref();
    let mut seen = BTreeSet::new();
    for api_name in &action.fields {
        let address = format!("fields.{api_name}");
        if !seen.insert(api_name.as_str()) {
            issues.push(issue(
                address,
                "quick_actions.field_duplicate",
                "choose each field once",
            ));
        } else if let Some((code, detail)) = field_problem(form, target_field, api_name) {
            issues.push(issue(address, code, detail));
        }
    }
    let mut presets = BTreeMap::new();
    for (api_name, raw) in &action.presets {
        let address = format!("presets.{api_name}");
        if let Some((code, detail)) = field_problem(form, target_field, api_name) {
            issues.push(issue(address, code, detail));
            continue;
        }
        let checked = if api_name == "name" {
            values::record_name(raw).map(Value::String)
        } else {
            match form.field(api_name) {
                Some(field) => {
                    values::normalise(field, raw).map(|value| value.unwrap_or(Value::Null))
                }
                None => continue,
            }
        };
        match checked {
            Ok(value) => {
                presets.insert(api_name.clone(), value);
            }
            Err(problem) => issues.push(issue(address, problem.code, problem.detail)),
        }
    }
    if issues.is_empty() {
        action.presets = presets;
    }
    issues
}

impl ObjectService {
    /// An object's quick actions, by position then name. Anyone who can read
    /// the object sees them (ADR 0032 §3: an action grants nothing); to
    /// anyone else the object is unknown.
    pub async fn quick_actions(&self, api_name: &str) -> Result<Vec<QuickAction>, ObjectError> {
        let manage = self.caller().can_manage();
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if !manage {
            let access = self.access(tx.as_mut()).await?;
            if !access.for_object(object.id).read {
                return Err(ObjectError::NotFound);
            }
        }
        let mut actions = Vec::new();
        for action in tx.quick_actions(object.id).await? {
            // A create action whose object is out of service, such as a
            // disabled plugin's (ADR 0033 §2), is hidden with it.
            if let Some(target) = action.target_object_id {
                let in_service = tx
                    .object_by_id(target)
                    .await?
                    .is_some_and(|target| target.is_active());
                if !in_service {
                    continue;
                }
            }
            actions.push(action);
        }
        tx.commit().await?;
        Ok(actions)
    }

    pub async fn create_quick_action(
        &self,
        api_name: &str,
        input: CreateQuickAction,
        context: MutationContext,
    ) -> Result<QuickAction, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        // Serialised with field and object deletion, which update or delete
        // the actions that use what they delete.
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let existing = tx.quick_actions(object.id).await?;
        if existing.len() >= QUICK_ACTIONS_PER_OBJECT_MAX {
            return Err(ObjectError::invalid(
                "name",
                "quick_actions.limit_reached",
                format!("an object can have at most {QUICK_ACTIONS_PER_OBJECT_MAX} quick actions"),
            ));
        }
        let mut issues = Vec::new();
        let mut target = None;
        match input.kind {
            QuickActionKind::Update => {
                for (present, address) in [
                    (input.target_object.is_some(), "target_object"),
                    (input.target_field.is_some(), "target_field"),
                ] {
                    if present {
                        issues.push(issue(
                            address,
                            "quick_actions.target_not_allowed",
                            "only create actions have a target",
                        ));
                    }
                }
            }
            QuickActionKind::Create => {
                match input.target_object.as_deref() {
                    None => issues.push(issue(
                        "target_object",
                        "quick_actions.target_required",
                        "choose the object a create action makes records of",
                    )),
                    Some(target_api_name) => {
                        match tx
                            .object_by_api_name(target_api_name)
                            .await?
                            .filter(ObjectDefinition::is_active)
                        {
                            Some(found) => target = Some(found),
                            None => issues.push(issue(
                                "target_object",
                                "quick_actions.target_object_unknown",
                                format!("there is no object {target_api_name}"),
                            )),
                        }
                    }
                }
                if input.target_field.is_none() {
                    issues.push(issue(
                        "target_field",
                        "quick_actions.target_required",
                        "choose the field that points to the current record",
                    ));
                }
            }
        }
        let now = Utc::now();
        let actor = Some(self.caller().user_id());
        let position = match input.position {
            Some(position) => position,
            None => i32::try_from(existing.len()).map_err(|_| ObjectError::Invariant)?,
        };
        let create = input.kind == QuickActionKind::Create;
        let mut action = QuickAction {
            id: Uuid::now_v7(),
            object_id: object.id,
            name: input.name.trim().to_owned(),
            label: input.label.trim().to_owned(),
            kind: input.kind,
            position,
            target_object_id: target.as_ref().map(|target| target.id),
            target_object: target.as_ref().map(|target| target.api_name.clone()),
            target_field: input.target_field.filter(|_| create),
            fields: input.fields,
            presets: input.presets,
            version: 1,
            created_at: now,
            created_by: actor,
            updated_at: now,
            updated_by: actor,
        };
        issues.extend(check_action(&mut action, &object, target.as_ref()));
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        tx.insert_quick_action(&action).await?;
        self.record_change(
            tx.as_mut(),
            context,
            Change::QuickActionCreated(&object, &action),
        )
        .await?;
        tx.commit().await?;
        Ok(action)
    }

    pub async fn update_quick_action(
        &self,
        api_name: &str,
        action_name: &str,
        input: UpdateQuickAction,
        context: MutationContext,
    ) -> Result<QuickAction, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let mut action = tx
            .quick_actions(object.id)
            .await?
            .into_iter()
            .find(|action| action.name == action_name)
            .ok_or(ObjectError::NotFound)?;
        if action.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        if let Some(label) = input.label {
            action.label = label.trim().to_owned();
        }
        if let Some(position) = input.position {
            action.position = position;
        }
        if let Some(fields) = input.fields {
            action.fields = fields;
        }
        if let Some(presets) = input.presets {
            action.presets = presets;
        }
        let target = match action.target_object_id {
            Some(id) => tx
                .object_by_id(id)
                .await?
                .filter(ObjectDefinition::is_active),
            None => None,
        };
        let mut issues = Vec::new();
        if action.kind == QuickActionKind::Create && target.is_none() {
            issues.push(issue(
                "target_object",
                "quick_actions.target_object_unknown",
                "the object this action creates records of no longer exists",
            ));
        }
        issues.extend(check_action(&mut action, &object, target.as_ref()));
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        action.version += 1;
        action.updated_at = Utc::now();
        action.updated_by = Some(self.caller().user_id());
        if !tx
            .update_quick_action(&action, input.expected_version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::QuickActionUpdated(&object, &action),
        )
        .await?;
        tx.commit().await?;
        Ok(action)
    }

    pub async fn delete_quick_action(
        &self,
        api_name: &str,
        action_name: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<QuickAction, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let action = tx
            .quick_actions(object.id)
            .await?
            .into_iter()
            .find(|action| action.name == action_name)
            .ok_or(ObjectError::NotFound)?;
        if action.version != expected_version
            || !tx.delete_quick_action(action.id, expected_version).await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::QuickActionDeleted(&object, &action),
        )
        .await?;
        tx.commit().await?;
        Ok(action)
    }

    /// Removes a deleted field from the actions that use it, in the field
    /// deletion's transaction: update actions on its object and create
    /// actions whose child object it belongs to. A create action whose
    /// reference field it is, or an action left with no fields, is deleted.
    pub(crate) async fn quick_actions_after_field_deleted(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        field: &FieldDefinition,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut affected: Vec<QuickAction> = tx
            .quick_actions(object.id)
            .await?
            .into_iter()
            .filter(|action| action.kind == QuickActionKind::Update)
            .collect();
        affected.extend(
            tx.quick_actions_targeting(object.id)
                .await?
                .into_iter()
                .filter(|action| action.kind == QuickActionKind::Create),
        );
        let mut owners = BTreeMap::from([(object.id, object.clone())]);
        for mut action in affected {
            let api_name = field.api_name.as_str();
            let is_target = action.target_field.as_deref() == Some(api_name);
            let in_fields = action.fields.iter().any(|name| name == api_name);
            let in_presets = action.presets.contains_key(api_name);
            if !is_target && !in_fields && !in_presets {
                continue;
            }
            let owner = match owners.get(&action.object_id) {
                Some(owner) => owner.clone(),
                None => {
                    let owner = tx
                        .object_by_id(action.object_id)
                        .await?
                        .ok_or(ObjectError::Invariant)?;
                    owners.insert(owner.id, owner.clone());
                    owner
                }
            };
            action.fields.retain(|name| name != api_name);
            action.presets.remove(api_name);
            if is_target || action.fields.is_empty() {
                if !tx.delete_quick_action(action.id, action.version).await? {
                    return Err(ObjectError::VersionConflict);
                }
                self.record_change(tx, context, Change::QuickActionDeleted(&owner, &action))
                    .await?;
            } else {
                let expected_version = action.version;
                action.version += 1;
                action.updated_at = Utc::now();
                action.updated_by = Some(self.caller().user_id());
                if !tx.update_quick_action(&action, expected_version).await? {
                    return Err(ObjectError::VersionConflict);
                }
                self.record_change(tx, context, Change::QuickActionUpdated(&owner, &action))
                    .await?;
            }
        }
        Ok(())
    }

    /// Deletes, in the transaction that changed it, the create actions
    /// whose reference field is the related-to `field` of `object` and that
    /// belong to an object its allowlist no longer names (ADR 0033 §1).
    pub(crate) async fn quick_actions_after_allowlist_changed(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        field: &FieldDefinition,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let stale: Vec<QuickAction> = tx
            .quick_actions_targeting(object.id)
            .await?
            .into_iter()
            .filter(|action| {
                action.kind == QuickActionKind::Create
                    && action.target_field.as_deref() == Some(field.api_name.as_str())
                    && !points_to(field, action.object_id)
            })
            .collect();
        for action in stale {
            let owner = tx
                .object_by_id(action.object_id)
                .await?
                .ok_or(ObjectError::Invariant)?;
            if !tx.delete_quick_action(action.id, action.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            self.record_change(tx, context, Change::QuickActionDeleted(&owner, &action))
                .await?;
        }
        Ok(())
    }

    /// Deletes, in the object deletion's transaction, the object's own
    /// actions and the create actions of other objects that make its records.
    pub(crate) async fn quick_actions_after_object_deleted(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let own = tx.quick_actions(object.id).await?;
        let targeting: Vec<QuickAction> = tx
            .quick_actions_targeting(object.id)
            .await?
            .into_iter()
            .filter(|action| action.object_id != object.id)
            .collect();
        let mut owners = BTreeMap::from([(object.id, object.clone())]);
        for action in own.into_iter().chain(targeting) {
            let owner = match owners.get(&action.object_id) {
                Some(owner) => owner.clone(),
                None => {
                    let owner = tx
                        .object_by_id(action.object_id)
                        .await?
                        .ok_or(ObjectError::Invariant)?;
                    owners.insert(owner.id, owner.clone());
                    owner
                }
            };
            if !tx.delete_quick_action(action.id, action.version).await? {
                return Err(ObjectError::VersionConflict);
            }
            self.record_change(tx, context, Change::QuickActionDeleted(&owner, &action))
                .await?;
        }
        Ok(())
    }
}
