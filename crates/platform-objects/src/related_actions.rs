//! Related actions (ADR 0033 §3): create quick actions a plugin object
//! offers to every object entering one of its related-to fields'
//! allowlists.
//!
//! A plugin declares them in its manifest (`[[objects.related_actions]]`,
//! see `plugin_contract::objects`). Installing the plugin keeps each
//! declaration in the workspace as a [`RelatedAction`], next to the objects
//! it was already offered to. Whenever an object enters the field's
//! allowlist (the starting list once the plugin's objects come into
//! service, an administrator's change, or a new declaration in an upgrade),
//! the engine adds an ordinary create quick action (ADR 0032) to that
//! object, with the field set to the current record:
//!
//! - **Only on entry.** An object is offered a declaration once while it
//!   stays in the allowlist. An administrator may edit or delete the action
//!   like any other; it is not recreated until the object leaves the
//!   allowlist and enters it again. Leaving it deletes the create actions
//!   that depend on the field, as for any allowlist change (ADR 0032 §4).
//! - **Never in the way.** The action is added only if the object has no
//!   action of that name and fewer than 10 actions, and only if it passes
//!   the same checks as an administrator's. Otherwise it is skipped, the
//!   skip is audited as `objects.quick_action.skipped` with the reason, and
//!   the change that caused it goes ahead. A skipped object counts as
//!   offered.
//! - **It grants nothing.** Running the action is the normal create API,
//!   with the member's own permissions (ADR 0032 §3).

use crate::events::Change;
use crate::quick_actions::{check_action, QUICK_ACTIONS_PER_OBJECT_MAX};
use crate::service::{MutationContext, ObjectService};
use crate::{
    FieldDefinition, FieldType, ObjectDefinition, ObjectError, ObjectTx, QuickAction,
    QuickActionKind, RelatedAction,
};
use chrono::Utc;
use platform_audit::AuditEvent;
use plugin_contract::ObjectDecl;
use serde_json::json;
use uuid::Uuid;

/// The audit action of a related action the engine did not add.
pub const RELATED_ACTION_SKIPPED: &str = "objects.quick_action.skipped";

impl ObjectService {
    /// Brings the related actions `object` keeps in line with its
    /// declaration, in the installation's transaction: new declarations are
    /// added, changed ones keep the objects they were offered to, and
    /// dropped ones go. Quick actions already created stay: they belong to
    /// the workspace.
    pub(crate) async fn put_plugin_related_actions(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        object: &ObjectDefinition,
        declared: &ObjectDecl,
    ) -> Result<(), ObjectError> {
        let mut kept = tx.related_actions(object.id).await?;
        let now = Utc::now();
        let mut wanted = Vec::new();
        for declaration in &declared.related_actions {
            let field = object
                .field(&format!("{plugin_id}:{}", declaration.field))
                .filter(|field| field.field_type == FieldType::RelatedTo)
                .ok_or(ObjectError::Invariant)?;
            let existing = kept
                .iter()
                .position(|action| action.field_id == field.id && action.name == declaration.name);
            let mut action = match existing {
                Some(index) => kept.swap_remove(index),
                None => RelatedAction {
                    id: Uuid::now_v7(),
                    object_id: object.id,
                    field_id: field.id,
                    owner_plugin: plugin_id.to_owned(),
                    name: declaration.name.clone(),
                    label: String::new(),
                    fields: Vec::new(),
                    presets: Default::default(),
                    offered_to: Vec::new(),
                    created_at: now,
                    updated_at: now,
                },
            };
            let before = action.clone();
            action.label = declaration.label.trim().to_owned();
            action.fields = declaration.fields.clone();
            action.presets = declaration.presets.clone();
            if existing.is_none() || action != before {
                action.updated_at = now;
                wanted.push(action);
            }
        }
        for dropped in kept {
            tx.delete_related_action(dropped.id).await?;
        }
        for action in wanted {
            tx.put_related_action(&action).await?;
        }
        Ok(())
    }

    /// Offers `object`'s related actions to the objects that entered their
    /// fields' allowlists since they were last offered, and forgets the
    /// objects that left, in the caller's transaction. `only` limits this to
    /// one field. Nothing happens while `object` is out of service.
    pub(crate) async fn offer_related_actions(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        only: Option<Uuid>,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        if !object.is_active() {
            return Ok(());
        }
        for mut action in tx.related_actions(object.id).await? {
            if only.is_some_and(|field_id| field_id != action.field_id) {
                continue;
            }
            let Some(field) = object
                .fields
                .iter()
                .find(|field| field.id == action.field_id && field.is_active())
                .cloned()
            else {
                continue;
            };
            let before = action.offered_to.clone();
            action
                .offered_to
                .retain(|id| field.reference_object_ids.contains(id));
            for host_id in &field.reference_object_ids {
                if action.offered_to.contains(host_id) {
                    continue;
                }
                if self
                    .add_related_action(tx, object, &field, &action, *host_id, context)
                    .await?
                {
                    action.offered_to.push(*host_id);
                }
            }
            if action.offered_to != before {
                action.updated_at = Utc::now();
                tx.put_related_action(&action).await?;
            }
        }
        Ok(())
    }

    /// Adds `action` to the object `host_id` as a create quick action, or
    /// audits why not. Returns whether the object counts as offered: false
    /// only while it is out of service, so it is offered once it is back.
    async fn add_related_action(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        field: &FieldDefinition,
        action: &RelatedAction,
        host_id: Uuid,
        context: MutationContext,
    ) -> Result<bool, ObjectError> {
        let Some(host) = tx
            .object_by_id(host_id)
            .await?
            .filter(ObjectDefinition::is_active)
        else {
            return Ok(false);
        };
        let existing = tx.quick_actions(host.id).await?;
        let reason = if existing.iter().any(|known| known.name == action.name) {
            Some("quick_actions.duplicate_name")
        } else if existing.len() >= QUICK_ACTIONS_PER_OBJECT_MAX {
            Some("quick_actions.limit_reached")
        } else {
            None
        };
        if let Some(reason) = reason {
            self.skip_related_action(tx, object, field, action, &host, reason, context)
                .await?;
            return Ok(true);
        }
        let now = Utc::now();
        let actor = Some(self.caller().user_id());
        let mut quick = QuickAction {
            id: Uuid::now_v7(),
            object_id: host.id,
            name: action.name.clone(),
            label: action.label.clone(),
            kind: QuickActionKind::Create,
            position: i32::try_from(existing.len()).map_err(|_| ObjectError::Invariant)?,
            target_object_id: Some(object.id),
            target_object: Some(object.api_name.clone()),
            target_field: Some(field.api_name.clone()),
            fields: action.fields.clone(),
            presets: action.presets.clone(),
            version: 1,
            created_at: now,
            created_by: actor,
            updated_at: now,
            updated_by: actor,
        };
        let issues = check_action(&mut quick, &host, Some(object));
        if let Some(first) = issues.first() {
            self.skip_related_action(tx, object, field, action, &host, first.code, context)
                .await?;
            return Ok(true);
        }
        tx.insert_quick_action(&quick).await?;
        self.record_change(tx, context, Change::QuickActionCreated(&host, &quick))
            .await?;
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    async fn skip_related_action(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        field: &FieldDefinition,
        action: &RelatedAction,
        host: &ObjectDefinition,
        reason: &'static str,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        tracing::warn!(
            plugin_id = %action.owner_plugin,
            object = %host.api_name,
            action = %action.name,
            reason,
            "related action skipped"
        );
        let evidence = self.evidence(context);
        let audit = AuditEvent::with_context(
            Some(evidence.tenant_id),
            Some(evidence.actor_id),
            evidence.correlation_id,
            evidence.causation_id,
            RELATED_ACTION_SKIPPED,
            format!("quick_action:{}/{}", host.api_name, action.name),
            json!({
                "object": host.api_name,
                "action": action.name,
                "target_object": object.api_name,
                "target_field": field.api_name,
                "plugin": action.owner_plugin,
                "reason": reason,
            }),
        );
        tx.append_audit(&audit).await
    }
}
