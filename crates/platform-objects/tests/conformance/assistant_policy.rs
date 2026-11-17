//! The assistant policy on every backend (ADR 0034 §4 and §6): everything
//! off until an administrator turns it on, readable by members, changed only
//! by administrators with its rules checked, versioned and published, moving
//! the metadata version; plugin objects follow the workspace; and pushed
//! writes a device marks as an assistant's are checked against the
//! server's current policy and recorded as the assistant's.

use super::{caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::composite::{CompositeOutcome, CompositeRequest};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// What the store's own test checks in its tables.
pub struct AssistantReport {
    /// The device that pushed the assistant's writes, which their audit
    /// entries name beside the source.
    pub device: Uuid,
    /// The record an assistant created through a push.
    pub assistant_record: String,
}

fn change(value: Value) -> UpdateAssistantPolicy {
    serde_json::from_value(value).expect("policy change")
}

fn settings(expected_version: i64, enabled: bool) -> UpdateAssistantSettings {
    UpdateAssistantSettings {
        expected_version,
        enabled,
    }
}

async fn metadata_version(service: &ObjectService) -> i64 {
    service
        .list_changes(None, Some(1), None)
        .await
        .expect("changes")
        .metadata_version
}

fn composite(operations: Value) -> CompositeRequest {
    serde_json::from_value(json!({"operations": operations})).expect("composite")
}

/// The single outcome's error code, or `ok`.
fn outcome(results: &[(String, CompositeOutcome)]) -> &'static str {
    match &results[0].1 {
        CompositeOutcome::Done { .. } => "ok",
        CompositeOutcome::Failed(ObjectError::Conflict(code)) => code,
        CompositeOutcome::Failed(ObjectError::Validation(issues)) => issues[0].code,
        CompositeOutcome::Failed(ObjectError::PermissionDenied) => "permission_denied",
        CompositeOutcome::Failed(ObjectError::NotFound) => "not_found",
        other => panic!("unexpected outcome {other:?}"),
    }
}

fn created(results: &[(String, CompositeOutcome)]) -> Record {
    match &results[0].1 {
        CompositeOutcome::Done { record, .. } => (**record).clone(),
        other => panic!("expected a record, got {other:?}"),
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> AssistantReport {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let member_role = Uuid::now_v7();
    let member_id = Uuid::now_v7();
    let member = service(&store, caller(tenant, member_id, &[], &[member_role]));
    let stranger = service(
        &store,
        caller(tenant, Uuid::now_v7(), &[], &[Uuid::now_v7()]),
    );

    for (name, label) in [("account", "Account"), ("contact", "Contact")] {
        let mut definition = object(name, label);
        definition.track_history = Some(true);
        admin
            .create_object(definition, context())
            .await
            .expect("object");
    }
    let mut notes = field("notes", FieldType::Text);
    notes.track_history = true;
    for definition in [notes, field("amount", FieldType::Number)] {
        admin
            .create_field("custom:account", definition, context())
            .await
            .expect("account field");
    }
    let mut lookup = field("account", FieldType::Lookup);
    lookup.reference_object = Some("custom:account".into());
    admin
        .create_field("custom:contact", lookup, context())
        .await
        .expect("contact lookup");
    for object in ["custom:account", "custom:contact"] {
        admin
            .set_object_permission(
                object,
                member_role,
                AccessFlags {
                    read: true,
                    create: true,
                    edit: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("member access");
    }
    let quick =
        |value: Value| -> CreateQuickAction { serde_json::from_value(value).expect("action") };
    admin
        .create_quick_action(
            "custom:account",
            quick(json!({"name": "close", "label": "Close", "kind": "update", "fields": ["custom:notes"]})),
            context(),
        )
        .await
        .expect("update action");
    admin
        .create_quick_action(
            "custom:account",
            quick(json!({
                "name": "add_contact", "label": "Add contact", "kind": "create",
                "target_object": "custom:contact", "target_field": "custom:account",
                "fields": ["name"]
            })),
            context(),
        )
        .await
        .expect("create action");

    // --- Everything is off until turned on ------------------------------
    let workspace = member.assistant_policy().await.expect("member reads");
    assert_eq!(workspace.settings, AssistantSettings::default());
    assert_eq!(workspace.objects.len(), 2);
    for policy in &workspace.objects {
        assert!(!policy.exposed && !policy.writable && !policy.indexable);
        assert!(policy.excluded_fields.is_empty() && policy.offered_actions.is_empty());
        assert_eq!((policy.version, policy.updated_at), (0, None));
    }
    assert!(
        stranger
            .assistant_policy()
            .await
            .expect("stranger")
            .objects
            .is_empty(),
        "objects a member cannot read are not listed"
    );
    assert!(matches!(
        stranger.object_assistant_policy("custom:account").await,
        Err(ObjectError::NotFound)
    ));
    assert!(matches!(
        admin.object_assistant_policy("custom:missing").await,
        Err(ObjectError::NotFound)
    ));

    // --- Only administrators change it ------------------------------------
    assert!(matches!(
        member
            .update_assistant_settings(settings(0, true), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        member
            .update_object_assistant_policy(
                "custom:account",
                change(json!({"expected_version": 0, "exposed": true})),
                context()
            )
            .await,
        Err(ObjectError::PermissionDenied)
    ));

    // --- The rules -----------------------------------------------------------
    let refused = admin
        .update_object_assistant_policy(
            "custom:account",
            change(json!({"expected_version": 0, "writable": true, "indexable": true})),
            context(),
        )
        .await
        .expect_err("writable and indexable require exposed");
    assert_eq!(
        codes(&refused),
        vec![
            "assistant_policy.requires_exposed",
            "assistant_policy.requires_exposed"
        ]
    );
    let refused = admin
        .update_object_assistant_policy(
            "custom:account",
            change(json!({
                "expected_version": 0, "exposed": true,
                "excluded_fields": ["custom:notes", "custom:notes", "custom:nope"],
                "offered_actions": ["close", "add_contact", "nope"]
            })),
            context(),
        )
        .await
        .expect_err("fields and actions are checked");
    assert_eq!(
        codes(&refused),
        vec![
            "assistant_policy.field_duplicate",
            "assistant_policy.field_unknown",
            "assistant_policy.action_not_writable",
            "assistant_policy.action_not_writable",
            "assistant_policy.action_unknown",
        ]
    );
    let refused = admin
        .update_object_assistant_policy(
            "custom:account",
            change(json!({"expected_version": 0, "offered_actions": ["close"]})),
            context(),
        )
        .await
        .expect_err("offering requires exposed");
    assert_eq!(codes(&refused), vec!["assistant_policy.requires_exposed"]);
    assert!(matches!(
        admin
            .update_object_assistant_policy(
                "custom:account",
                change(json!({"expected_version": 3, "exposed": true})),
                context()
            )
            .await,
        Err(ObjectError::VersionConflict)
    ));

    // --- Changes are versioned and move the metadata version -------------
    let before = metadata_version(&admin).await;
    let switch = admin
        .update_assistant_settings(settings(0, true), context())
        .await
        .expect("switch on");
    assert!(switch.enabled);
    assert_eq!(switch.version, 1);
    assert!(metadata_version(&admin).await > before);
    assert!(matches!(
        admin
            .update_assistant_settings(settings(0, false), context())
            .await,
        Err(ObjectError::VersionConflict)
    ));
    let account = admin
        .update_object_assistant_policy(
            "custom:account",
            change(json!({
                "expected_version": 0, "exposed": true, "writable": true, "indexable": true,
                "excluded_fields": ["name", "custom:amount"],
                "offered_actions": ["close"]
            })),
            context(),
        )
        .await
        .expect("account policy");
    assert_eq!(account.version, 1);
    assert!(account.exposed && account.writable && account.indexable);
    assert_eq!(account.excluded_fields, vec!["name", "custom:amount"]);
    assert_eq!(account.offered_actions, vec!["close"]);
    assert_eq!(
        member
            .object_assistant_policy("custom:account")
            .await
            .expect("member reads the object's"),
        account
    );
    // A create action needs the object it writes to be writable.
    let refused = admin
        .update_object_assistant_policy(
            "custom:account",
            change(json!({"expected_version": 1, "offered_actions": ["close", "add_contact"]})),
            context(),
        )
        .await
        .expect_err("contacts are not writable");
    assert_eq!(
        codes(&refused),
        vec!["assistant_policy.action_not_writable"]
    );
    let contact = admin
        .update_object_assistant_policy(
            "custom:contact",
            change(json!({"expected_version": 0, "exposed": true, "writable": true})),
            context(),
        )
        .await
        .expect("contact policy");
    let account = admin
        .update_object_assistant_policy(
            "custom:account",
            change(json!({"expected_version": 1, "offered_actions": ["close", "add_contact"]})),
            context(),
        )
        .await
        .expect("both offered");
    assert_eq!(account.offered_actions, vec!["close", "add_contact"]);
    assert_eq!(
        account.excluded_fields,
        vec!["name", "custom:amount"],
        "a partial change keeps what it does not name"
    );

    // --- Pushed assistant writes are re-checked by the server -------------
    let device = Uuid::now_v7();
    let pushed = || context().with_source(MutationSource::device(Some(device)));
    let member_pushed = service(&store, caller(tenant, member_id, &[], &[member_role]));
    let results = member_pushed
        .composite(
            &composite(json!([{
                "method": "create", "reference": "r0", "object": "custom:account",
                "values": {"name": "Spoken account"},
                "assistant": {"assistant": "siri"}
            }])),
            pushed(),
        )
        .await
        .expect("push");
    assert_eq!(outcome(&results), "ok");
    let spoken = created(&results);
    let history = member
        .record_history("custom:account", &spoken.public_id, None, None)
        .await
        .expect("history")
        .0
        .entries;
    assert!(!history.is_empty());
    assert!(history
        .iter()
        .all(|entry| entry.source == Some(MutationSource::assistant(Assistant::Siri))));

    // An offered quick action, named by its object and API name.
    let results = member_pushed
        .composite(
            &composite(json!([{
                "method": "update", "reference": "r0", "object": "custom:account",
                "id": spoken.public_id, "expected_version": spoken.version,
                "values": {"custom:notes": "closed by voice"},
                "assistant": {"assistant": "gemini",
                              "quick_action": {"object": "custom:account", "name": "close"}}
            }])),
            pushed(),
        )
        .await
        .expect("push");
    assert_eq!(outcome(&results), "ok");
    let spoken = created(&results);
    let results = member_pushed
        .composite(
            &composite(json!([{
                "method": "create", "reference": "r0", "object": "custom:contact",
                "values": {"name": "Ada", "custom:account": spoken.public_id},
                "assistant": {"assistant": "siri",
                              "quick_action": {"object": "custom:account", "name": "add_contact"}}
            }])),
            pushed(),
        )
        .await
        .expect("push");
    assert_eq!(outcome(&results), "ok", "a create action makes its child");

    // A quick action that does not write this way, or is unknown, is refused.
    for marker in [
        json!({"assistant": "siri", "quick_action": {"object": "custom:account", "name": "close"}}),
        json!({"assistant": "siri", "quick_action": {"object": "custom:account", "name": "nope"}}),
        json!({"assistant": "siri", "quick_action": {"object": "custom:nope", "name": "close"}}),
    ] {
        let results = member_pushed
            .composite(
                &composite(json!([{
                    "method": "create", "reference": "r0", "object": "custom:account",
                    "values": {"name": "Wrong action"}, "assistant": marker
                }])),
                pushed(),
            )
            .await
            .expect("push");
        assert_eq!(outcome(&results), "assistant.not_allowed");
    }

    // The marker is accepted only from a device's push to the server.
    for source in [MutationSource::web(), MutationSource::device(None)] {
        let results = member
            .composite(
                &composite(json!([{
                    "method": "create", "reference": "r0", "object": "custom:account",
                    "values": {"name": "Claimed"}, "assistant": {"assistant": "siri"}
                }])),
                context().with_source(source),
            )
            .await
            .expect("composite");
        assert_eq!(outcome(&results), "assistant.marker_not_accepted");
    }
    assert!(
        serde_json::from_value::<CompositeRequest>(json!({"operations": [{
            "method": "create", "reference": "r0", "object": "custom:account",
            "values": {"name": "x"}, "assistant": {"assistant": "robot"}
        }]}))
        .is_err(),
        "only the closed list of assistants"
    );

    // The marker never widens: the engine's own checks still apply first.
    let results = stranger
        .composite(
            &composite(json!([{
                "method": "create", "reference": "r0", "object": "custom:account",
                "values": {"name": "Hidden"}, "assistant": {"assistant": "siri"}
            }])),
            pushed(),
        )
        .await
        .expect("push");
    assert_eq!(outcome(&results), "not_found");

    // Turning writable off withdraws the create actions that make its
    // records, and refuses the writes with the server's current policy.
    let contact = admin
        .update_object_assistant_policy(
            "custom:contact",
            change(json!({"expected_version": contact.version, "writable": false})),
            context(),
        )
        .await
        .expect("contacts read-only");
    assert!(contact.exposed && !contact.writable);
    let account = admin
        .object_assistant_policy("custom:account")
        .await
        .expect("account");
    assert_eq!(account.offered_actions, vec!["close"]);
    assert_eq!(account.version, 3, "the withdrawal is its own change");
    for request in [
        json!([{"method": "create", "reference": "r0", "object": "custom:contact",
                "values": {"name": "Grace"}, "assistant": {"assistant": "siri"}}]),
        json!([{"method": "create", "reference": "r0", "object": "custom:contact",
                "values": {"name": "Grace", "custom:account": spoken.public_id},
                "assistant": {"assistant": "siri",
                              "quick_action": {"object": "custom:account", "name": "add_contact"}}}]),
    ] {
        let results = member_pushed
            .composite(&composite(request), pushed())
            .await
            .expect("push");
        assert_eq!(outcome(&results), "assistant.not_allowed");
    }
    // In one transaction too: the refusal rolls the request back.
    let mut together = composite(json!([
        {"method": "create", "reference": "a", "object": "custom:account", "values": {"name": "Kept?"}},
        {"method": "create", "reference": "b", "object": "custom:contact", "values": {"name": "No"},
         "assistant": {"assistant": "gemini"}}
    ]));
    together.all_or_none = true;
    let results = member_pushed
        .composite(&together, pushed())
        .await
        .expect("push");
    assert!(matches!(results[0].1, CompositeOutcome::RolledBack));
    assert!(matches!(
        results[1].1,
        CompositeOutcome::Failed(ObjectError::Conflict("assistant.not_allowed"))
    ));

    // Unmarked writes are untouched by the policy.
    let results = member_pushed
        .composite(
            &composite(json!([{"method": "create", "reference": "r0",
                "object": "custom:contact", "values": {"name": "Typed"}}])),
            pushed(),
        )
        .await
        .expect("push");
    assert_eq!(outcome(&results), "ok");

    // The workspace switch off refuses every assistant write.
    let switch = admin
        .update_assistant_settings(settings(switch.version, false), context())
        .await
        .expect("switch off");
    assert_eq!(switch.version, 2);
    let results = member_pushed
        .composite(
            &composite(json!([{"method": "create", "reference": "r0",
                "object": "custom:account", "values": {"name": "Off"},
                "assistant": {"assistant": "siri"}}])),
            pushed(),
        )
        .await
        .expect("push");
    assert_eq!(outcome(&results), "assistant.not_allowed");

    // --- Deleted fields and actions drop out of the policy ---------------
    let (definition, _) = admin.describe("custom:account").await.expect("account");
    let amount = definition.field("custom:amount").expect("amount").version;
    admin
        .delete_field("custom:account", "custom:amount", amount, context())
        .await
        .expect("delete amount");
    let close = admin
        .quick_actions("custom:account")
        .await
        .expect("actions")
        .into_iter()
        .find(|action| action.name == "close")
        .expect("close");
    admin
        .delete_quick_action("custom:account", "close", close.version, context())
        .await
        .expect("delete close");
    admin
        .create_quick_action(
            "custom:account",
            quick(json!({"name": "close", "label": "Close again", "kind": "update", "fields": ["name"]})),
            context(),
        )
        .await
        .expect("same name again");
    let account = admin
        .object_assistant_policy("custom:account")
        .await
        .expect("account");
    assert_eq!(account.excluded_fields, vec!["name"]);
    assert!(
        account.offered_actions.is_empty(),
        "an action made again under an offered one's name is not offered"
    );

    // --- A replica's engine never edits the policy -------------------------
    let replica = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    )
    .with_replica();
    assert!(matches!(
        replica
            .update_assistant_settings(settings(2, true), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));

    AssistantReport {
        device,
        assistant_record: spoken.public_id,
    }
}

/// Plugin-declared objects follow the workspace: installing one turns
/// nothing on, its installation cannot change the policy, and an
/// administrator can.
pub async fn run_plugin_objects(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let manifest = super::plugin_objects::fixture();
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, Uuid::now_v7(), manifest.id.as_str()),
    );
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(manifest.id.as_str(), context())
        .await
        .expect("resume");
    let workspace = admin.assistant_policy().await.expect("policy");
    assert!(!workspace.objects.is_empty());
    for policy in &workspace.objects {
        assert!(
            !policy.exposed && !policy.writable && !policy.indexable && policy.version == 0,
            "{} ships off",
            policy.object
        );
    }
    let object = workspace.objects[0].object.clone();
    assert!(matches!(
        installer
            .update_object_assistant_policy(
                &object,
                change(json!({"expected_version": 0, "exposed": true})),
                context()
            )
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    let changed = admin
        .update_object_assistant_policy(
            &object,
            change(json!({"expected_version": 0, "exposed": true})),
            context(),
        )
        .await
        .expect("an administrator decides");
    assert!(changed.exposed);
    // Suspended with its plugin, it is missing here too.
    installer
        .suspend_plugin_objects(manifest.id.as_str(), context())
        .await
        .expect("suspend");
    assert!(matches!(
        admin.object_assistant_policy(&object).await,
        Err(ObjectError::NotFound)
    ));
    assert!(admin
        .assistant_policy()
        .await
        .expect("policy")
        .objects
        .iter()
        .all(|policy| policy.object != object));
}
