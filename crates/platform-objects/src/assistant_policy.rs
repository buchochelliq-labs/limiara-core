//! The assistant policy (ADR 0034 §4, #234): what a device's assistant
//! bridge may reach in a workspace.
//!
//! It is object metadata like validation rules and quick actions: holders
//! of `objects.manage` change it, each change is versioned, audited and
//! published as `platform.objects.assistant-policy-updated.v1` in its
//! transaction, and it moves the workspace metadata version so it reaches
//! device replicas with the object definitions. Everything is off until an
//! administrator turns it on, and it only narrows: it is applied after the
//! member's permissions and sharing, removes, and grants nothing.
//!
//! - **Workspace switch:** assistant access on or off.
//! - **Per object:** exposed (find and read its records as the member),
//!   writable (create and update them; requires exposed), indexable (put
//!   them into the OS index; requires exposed) with fields whose values
//!   never enter the index, and the quick actions offered to assistants.
//! - **Per quick action:** offered only while its object is exposed and the
//!   object it writes is writable.
//!
//! A plugin-declared object follows the workspace's policy like its sharing
//! model: a plugin's installation never sets it, so no plugin can ship it
//! turned on.
//!
//! The server re-checks every pushed write a device marks as an
//! assistant's (§6) against its own current policy here, and refuses one
//! the policy does not allow with `assistant.not_allowed`.

use crate::events::Change;
use crate::service::{MutationContext, ObjectService};
use crate::{
    Assistant, ObjectAccess, ObjectDefinition, ObjectError, ObjectTx, QuickAction, QuickActionKind,
    ValidationIssue,
};
use chrono::{DateTime, SubsecRound, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// The stable code of a pushed assistant write the server's policy does
/// not allow (ADR 0034 §4).
pub const ASSISTANT_NOT_ALLOWED: &str = "assistant.not_allowed";
/// The stable code of an assistant marker sent by anything but a synced
/// device's push to the server (ADR 0034 §6).
pub const ASSISTANT_MARKER_NOT_ACCEPTED: &str = "assistant.marker_not_accepted";
/// Quick actions one object may offer: every action it can have.
pub const OFFERED_ACTIONS_MAX: usize = crate::quick_actions::QUICK_ACTIONS_PER_OBJECT_MAX;
/// The record name, which the index excludes like a field.
const NAME: &str = "name";

/// The workspace switch. A workspace that never set it has it off at
/// version 0.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AssistantSettings {
    pub enabled: bool,
    pub version: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<Uuid>,
}

/// One object's policy as a store keeps it, by the ids of its fields and
/// quick actions, so a field or action made again under a deleted one's
/// name is not excluded or offered by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredAssistantPolicy {
    pub object_id: Uuid,
    pub exposed: bool,
    pub writable: bool,
    pub indexable: bool,
    /// Whether the record name is excluded from the index.
    pub exclude_name: bool,
    pub excluded_field_ids: Vec<Uuid>,
    pub offered_action_ids: Vec<Uuid>,
    pub version: i64,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

impl StoredAssistantPolicy {
    /// Everything off, as an object without a stored policy has it.
    pub fn off(object_id: Uuid) -> Self {
        Self {
            object_id,
            exposed: false,
            writable: false,
            indexable: false,
            exclude_name: false,
            excluded_field_ids: Vec::new(),
            offered_action_ids: Vec::new(),
            version: 0,
            updated_at: DateTime::<Utc>::UNIX_EPOCH,
            updated_by: None,
        }
    }
}

/// One object's policy by API names, as callers see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssistantPolicy {
    pub object: String,
    pub exposed: bool,
    pub writable: bool,
    pub indexable: bool,
    /// Fields whose values never enter the OS index; `name` is the record
    /// name.
    pub excluded_fields: Vec<String>,
    /// Quick actions offered to assistants, by API name.
    pub offered_actions: Vec<String>,
    /// 0 while the object has never had a policy.
    pub version: i64,
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Option<Uuid>,
}

/// The workspace switch with the policy of every object the caller can
/// read (every object, for administrators).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkspaceAssistantPolicy {
    #[serde(flatten)]
    pub settings: AssistantSettings,
    pub objects: Vec<AssistantPolicy>,
}

/// A change to the workspace switch.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAssistantSettings {
    /// The switch's version as read; 0 before it was ever set.
    pub expected_version: i64,
    pub enabled: bool,
}

/// A partial change to one object's policy.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAssistantPolicy {
    /// The policy's version as read; 0 before it was ever set.
    pub expected_version: i64,
    #[serde(default)]
    pub exposed: Option<bool>,
    #[serde(default)]
    pub writable: Option<bool>,
    #[serde(default)]
    pub indexable: Option<bool>,
    #[serde(default)]
    pub excluded_fields: Option<Vec<String>>,
    #[serde(default)]
    pub offered_actions: Option<Vec<String>>,
}

/// What a pushed operation says about itself when a device's assistant
/// bridge made it (ADR 0034 §6, ADR 0017 "Change source" rule 3 as
/// amended). It is a narrowing claim: the server checks the write against
/// its own policy as well as everything it checks anyway, and records the
/// source as the assistant's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantMarker {
    pub assistant: Assistant,
    /// The quick action the assistant ran, when it ran one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quick_action: Option<MarkedQuickAction>,
}

/// A quick action by the API names of its object and of the action.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MarkedQuickAction {
    pub object: String,
    pub name: String,
}

/// Which write a marked operation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssistantWrite {
    Create,
    Update,
}

fn issue(
    field: impl Into<String>,
    code: &'static str,
    detail: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue::new(field, code, detail)
}

/// Now, to the microsecond both stores keep, so what a change returns is
/// what a later read gives.
fn now() -> DateTime<Utc> {
    Utc::now().trunc_subsecs(6)
}

fn not_allowed() -> ObjectError {
    ObjectError::Conflict(ASSISTANT_NOT_ALLOWED)
}

/// A policy by API names, from what is stored: ids that no longer resolve
/// to an active field or an existing action are left out.
pub(crate) fn resolve(
    object: &ObjectDefinition,
    actions: &[QuickAction],
    stored: &StoredAssistantPolicy,
) -> AssistantPolicy {
    let mut excluded_fields = Vec::new();
    if stored.exclude_name {
        excluded_fields.push(NAME.to_owned());
    }
    for id in &stored.excluded_field_ids {
        if let Some(field) = object
            .fields
            .iter()
            .find(|field| field.id == *id && field.is_active())
        {
            excluded_fields.push(field.api_name.clone());
        }
    }
    let offered_actions = stored
        .offered_action_ids
        .iter()
        .filter_map(|id| actions.iter().find(|action| action.id == *id))
        .map(|action| action.name.clone())
        .collect();
    AssistantPolicy {
        object: object.api_name.clone(),
        exposed: stored.exposed,
        writable: stored.writable,
        indexable: stored.indexable,
        excluded_fields,
        offered_actions,
        version: stored.version,
        updated_at: (stored.version > 0).then_some(stored.updated_at),
        updated_by: stored.updated_by,
    }
}

/// A store's policies by object id.
pub(crate) async fn stored_policies(
    tx: &mut dyn ObjectTx,
) -> Result<BTreeMap<Uuid, StoredAssistantPolicy>, ObjectError> {
    Ok(tx
        .assistant_policies()
        .await?
        .into_iter()
        .map(|policy| (policy.object_id, policy))
        .collect())
}

pub(crate) async fn stored_policy(
    tx: &mut dyn ObjectTx,
    object_id: Uuid,
) -> Result<StoredAssistantPolicy, ObjectError> {
    Ok(stored_policies(tx)
        .await?
        .remove(&object_id)
        .unwrap_or_else(|| StoredAssistantPolicy::off(object_id)))
}

impl ObjectService {
    /// The workspace switch and the policy of every object the caller can
    /// read; administrators who manage objects see every object's. Any
    /// member may read it, as a device needs it to decide what its
    /// assistant bridge offers.
    pub async fn assistant_policy(&self) -> Result<WorkspaceAssistantPolicy, ObjectError> {
        let manage = self.caller().can_manage();
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let settings = tx.assistant_settings().await?.unwrap_or_default();
        let mut stored = stored_policies(tx.as_mut()).await?;
        let mut objects = Vec::new();
        for listed in tx.list_objects().await? {
            if !listed.is_active() || !(manage || access.for_object(listed.id).read) {
                continue;
            }
            let Some(object) = tx.object_by_id(listed.id).await? else {
                continue;
            };
            let actions = tx.quick_actions(object.id).await?;
            let policy = stored
                .remove(&object.id)
                .unwrap_or_else(|| StoredAssistantPolicy::off(object.id));
            let object = self
                .policy_object(tx.as_mut(), &access, manage, object)
                .await?;
            objects.push(resolve(&object, &actions, &policy));
        }
        tx.commit().await?;
        Ok(WorkspaceAssistantPolicy { settings, objects })
    }

    /// One object's policy. To a caller who cannot read the object it is
    /// unknown, as the object is.
    pub async fn object_assistant_policy(
        &self,
        api_name: &str,
    ) -> Result<AssistantPolicy, ObjectError> {
        let manage = self.caller().can_manage();
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if !manage && !access.for_object(object.id).read {
            return Err(ObjectError::NotFound);
        }
        let actions = tx.quick_actions(object.id).await?;
        let stored = stored_policy(tx.as_mut(), object.id).await?;
        let object = self
            .policy_object(tx.as_mut(), &access, manage, object)
            .await?;
        tx.commit().await?;
        Ok(resolve(&object, &actions, &stored))
    }

    /// The object a policy is resolved against for the caller: to a member
    /// who does not manage objects, without the fields they cannot read
    /// (G5 of ADR 0035), so a policy never names a field the member's
    /// description of the object leaves out. Administrators see every
    /// field, as `describe` shows them.
    async fn policy_object(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        manage: bool,
        object: ObjectDefinition,
    ) -> Result<ObjectDefinition, ObjectError> {
        if manage {
            return Ok(object);
        }
        let hidden = self.hidden_fields(tx, access, &object).await?;
        Ok(crate::field_security::visible(&object, &hidden))
    }

    /// Turns assistant access on or off for the workspace.
    pub async fn update_assistant_settings(
        &self,
        input: UpdateAssistantSettings,
        context: MutationContext,
    ) -> Result<AssistantSettings, ObjectError> {
        self.require_policy_editor()?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let current = tx.assistant_settings().await?.unwrap_or_default();
        if current.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        let settings = AssistantSettings {
            enabled: input.enabled,
            version: current.version + 1,
            updated_at: Some(now()),
            updated_by: Some(self.caller().user_id()),
        };
        if !tx
            .put_assistant_settings(&settings, input.expected_version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::AssistantSettingsUpdated(&settings),
        )
        .await?;
        tx.commit().await?;
        Ok(settings)
    }

    /// Changes one object's policy. Writable, indexable and offered actions
    /// each require exposed, and an action is offered only when the object
    /// it writes is writable. Turning writable off withdraws, in the same
    /// transaction, the create actions of other objects that make its
    /// records.
    pub async fn update_object_assistant_policy(
        &self,
        api_name: &str,
        input: UpdateAssistantPolicy,
        context: MutationContext,
    ) -> Result<AssistantPolicy, ObjectError> {
        self.require_policy_editor()?;
        let mut tx = self.begin().await?;
        // Serialised with quick action and field changes, which this checks
        // against, and with other policy changes.
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let mut policies = stored_policies(tx.as_mut()).await?;
        let current = policies
            .get(&object.id)
            .cloned()
            .unwrap_or_else(|| StoredAssistantPolicy::off(object.id));
        if current.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        let actions = tx.quick_actions(object.id).await?;
        let mut next = current.clone();
        if let Some(exposed) = input.exposed {
            next.exposed = exposed;
        }
        if let Some(writable) = input.writable {
            next.writable = writable;
        }
        if let Some(indexable) = input.indexable {
            next.indexable = indexable;
        }
        let mut issues = Vec::new();
        for (on, address) in [(next.writable, "writable"), (next.indexable, "indexable")] {
            if on && !next.exposed {
                issues.push(issue(
                    address,
                    "assistant_policy.requires_exposed",
                    format!("expose {} to assistants first", object.label),
                ));
            }
        }
        if let Some(fields) = &input.excluded_fields {
            next.exclude_name = false;
            next.excluded_field_ids.clear();
            let mut seen = BTreeSet::new();
            for api_name in fields {
                let address = format!("excluded_fields.{api_name}");
                if !seen.insert(api_name.as_str()) {
                    issues.push(issue(
                        address,
                        "assistant_policy.field_duplicate",
                        "choose each field once",
                    ));
                } else if api_name == NAME {
                    next.exclude_name = true;
                } else if let Some(field) = object.field(api_name) {
                    next.excluded_field_ids.push(field.id);
                } else {
                    issues.push(issue(
                        address,
                        "assistant_policy.field_unknown",
                        format!("{} has no field {api_name}", object.api_name),
                    ));
                }
            }
        } else {
            // Keep only what still resolves.
            next.excluded_field_ids
                .retain(|id| object.fields.iter().any(|field| field.id == *id));
        }
        let offered: Vec<String> = match &input.offered_actions {
            Some(names) => names.clone(),
            None => resolve(&object, &actions, &current).offered_actions,
        };
        if offered.len() > OFFERED_ACTIONS_MAX {
            issues.push(issue(
                "offered_actions",
                "assistant_policy.actions_invalid",
                format!("offer at most {OFFERED_ACTIONS_MAX} quick actions"),
            ));
        }
        next.offered_action_ids.clear();
        let mut seen = BTreeSet::new();
        for name in &offered {
            let address = format!("offered_actions.{name}");
            if !seen.insert(name.as_str()) {
                issues.push(issue(
                    address,
                    "assistant_policy.action_duplicate",
                    "choose each quick action once",
                ));
                continue;
            }
            let Some(action) = actions.iter().find(|action| action.name == *name) else {
                issues.push(issue(
                    address,
                    "assistant_policy.action_unknown",
                    format!("{} has no quick action {name}", object.api_name),
                ));
                continue;
            };
            if !next.exposed {
                issues.push(issue(
                    address,
                    "assistant_policy.requires_exposed",
                    format!("expose {} to assistants first", object.label),
                ));
                continue;
            }
            let writes = match (action.kind, action.target_object_id) {
                (QuickActionKind::Update, _) => Some((object.id, object.label.clone())),
                (QuickActionKind::Create, Some(target)) => tx
                    .object_by_id(target)
                    .await?
                    .filter(ObjectDefinition::is_active)
                    .map(|target| (target.id, target.label)),
                (QuickActionKind::Create, None) => None,
            };
            let writable = match &writes {
                Some((id, _)) if *id == object.id => next.writable,
                Some((id, _)) => policies.get(id).is_some_and(|policy| policy.writable),
                None => false,
            };
            if !writable {
                let label = writes.map_or_else(|| "its object".to_owned(), |(_, label)| label);
                issues.push(issue(
                    address,
                    "assistant_policy.action_not_writable",
                    format!("let assistants write {label} first"),
                ));
                continue;
            }
            next.offered_action_ids.push(action.id);
        }
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        next.version = current.version + 1;
        next.updated_at = now();
        next.updated_by = Some(self.caller().user_id());
        if !tx
            .put_assistant_policy(&next, input.expected_version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        let resolved = resolve(&object, &actions, &next);
        self.record_change(
            tx.as_mut(),
            context,
            Change::AssistantPolicyUpdated(&object, &resolved),
        )
        .await?;
        policies.insert(object.id, next.clone());
        if current.writable && !next.writable {
            self.withdraw_actions_creating(tx.as_mut(), &object, &mut policies, context)
                .await?;
        }
        tx.commit().await?;
        Ok(resolved)
    }

    /// Takes the create actions of other objects that make records of
    /// `object` out of their objects' offered actions, each change with its
    /// evidence: `object` is no longer writable.
    async fn withdraw_actions_creating(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        policies: &mut BTreeMap<Uuid, StoredAssistantPolicy>,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let mut withdrawn: BTreeMap<Uuid, BTreeSet<Uuid>> = BTreeMap::new();
        for action in tx.quick_actions_targeting(object.id).await? {
            if action.kind == QuickActionKind::Create && action.object_id != object.id {
                withdrawn
                    .entry(action.object_id)
                    .or_default()
                    .insert(action.id);
            }
        }
        for (owner_id, ids) in withdrawn {
            let Some(mut policy) = policies.get(&owner_id).cloned() else {
                continue;
            };
            let before = policy.offered_action_ids.len();
            policy.offered_action_ids.retain(|id| !ids.contains(id));
            if policy.offered_action_ids.len() == before {
                continue;
            }
            let Some(owner) = tx.object_by_id(owner_id).await? else {
                continue;
            };
            let expected = policy.version;
            policy.version += 1;
            policy.updated_at = now();
            policy.updated_by = Some(self.caller().user_id());
            if !tx.put_assistant_policy(&policy, expected).await? {
                return Err(ObjectError::VersionConflict);
            }
            let actions = tx.quick_actions(owner.id).await?;
            let resolved = resolve(&owner, &actions, &policy);
            self.record_change(
                tx,
                context,
                Change::AssistantPolicyUpdated(&owner, &resolved),
            )
            .await?;
            policies.insert(owner_id, policy);
        }
        Ok(())
    }

    /// Only an administrator changes the policy, never a plugin's
    /// installation (ADR 0034 §4: plugin objects follow the workspace), and
    /// never a replica's engine, which takes the server's.
    fn require_policy_editor(&self) -> Result<(), ObjectError> {
        self.caller().require_manage()?;
        if self.caller().installer().is_some() || self.is_replica() {
            return Err(ObjectError::PermissionDenied);
        }
        Ok(())
    }

    /// The context of an operation marked as an assistant's: its source is
    /// the assistant, and the verified device that pushed it is kept for
    /// the audit entry. Only the server accepts a marker, and only from a
    /// request a synced device's token authenticated.
    pub(crate) fn assistant_context(
        &self,
        context: MutationContext,
        marker: Option<&AssistantMarker>,
    ) -> Result<MutationContext, ObjectError> {
        let Some(marker) = marker else {
            return Ok(context);
        };
        let device = context
            .source
            .filter(|source| source.kind == crate::SourceKind::Device)
            .and_then(|source| source.detail);
        match device {
            Some(device) if !self.is_replica() => Ok(MutationContext {
                source: Some(crate::MutationSource::assistant(marker.assistant)),
                assistant_device: Some(device),
                ..context
            }),
            _ => Err(ObjectError::invalid(
                "assistant",
                ASSISTANT_MARKER_NOT_ACCEPTED,
                "only a synced device's push may say a change came from an assistant",
            )),
        }
    }

    /// Whether the workspace's current policy lets an assistant make this
    /// write to `object` (ADR 0034 §4 and §6): assistant access is on, the
    /// object is exposed and writable, and a named quick action exists, is
    /// offered on an exposed object and writes this object in this way. It
    /// runs after the engine's own checks, in the write's transaction, so
    /// it only ever refuses what they would allow.
    pub(crate) async fn check_assistant_write(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        write: AssistantWrite,
        marker: &AssistantMarker,
    ) -> Result<(), ObjectError> {
        let enabled = tx
            .assistant_settings()
            .await?
            .is_some_and(|settings| settings.enabled);
        if !enabled {
            return Err(not_allowed());
        }
        let policies = stored_policies(tx).await?;
        let writable = policies
            .get(&object.id)
            .is_some_and(|policy| policy.exposed && policy.writable);
        if !writable {
            return Err(not_allowed());
        }
        let Some(marked) = &marker.quick_action else {
            return Ok(());
        };
        let Some(owner) = tx
            .object_by_api_name(&marked.object)
            .await?
            .filter(ObjectDefinition::is_active)
        else {
            return Err(not_allowed());
        };
        let Some(action) = tx
            .quick_actions(owner.id)
            .await?
            .into_iter()
            .find(|action| action.name == marked.name)
        else {
            return Err(not_allowed());
        };
        let offered = policies
            .get(&owner.id)
            .is_some_and(|policy| policy.exposed && policy.offered_action_ids.contains(&action.id));
        let writes_this = match action.kind {
            QuickActionKind::Update => write == AssistantWrite::Update && owner.id == object.id,
            QuickActionKind::Create => {
                write == AssistantWrite::Create && action.target_object_id == Some(object.id)
            }
        };
        if offered && writes_this {
            Ok(())
        } else {
            Err(not_allowed())
        }
    }
}
