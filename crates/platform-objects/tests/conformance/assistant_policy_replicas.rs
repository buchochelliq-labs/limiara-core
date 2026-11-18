//! The assistant policy on a synced device (ADR 0034 §4, ADR 0024 §3), on
//! SQLite, which devices use. The replica takes the workspace switch and
//! each object's policy with its other definitions, by its own fields and
//! quick actions, and a policy the server does not send reads as off. Here
//! the server is another workspace in the same store.

use super::{caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const MEMBER_ACCESS: AccessFlags = AccessFlags {
    read: true,
    create: true,
    edit: true,
    delete: true,
    view_all: false,
    modify_all: false,
};

/// Every object with its quick actions and assistant policy, and the
/// switch, as the server's API gives them.
async fn described(server: &ObjectService) -> (Vec<RemoteObject>, RemoteAssistantSettings) {
    let policy = server.assistant_policy().await.expect("policy");
    let mut objects = Vec::new();
    for (listed, _) in server.list_objects().await.expect("objects") {
        let (object, _) = server.describe(&listed.api_name).await.expect("describe");
        let actions = server
            .quick_actions(&object.api_name)
            .await
            .expect("quick actions");
        let mut remote = RemoteObject::from_definition(&object, MEMBER_ACCESS, Some(&[]))
            .with_quick_actions(&actions);
        if let Some(found) = policy
            .objects
            .iter()
            .find(|found| found.object == object.api_name)
        {
            remote = remote.with_assistant_policy(found);
        }
        objects.push(remote);
    }
    (
        objects,
        RemoteAssistantSettings {
            enabled: policy.settings.enabled,
            version: policy.settings.version,
        },
    )
}

fn change(value: Value) -> UpdateAssistantPolicy {
    serde_json::from_value(value).expect("policy change")
}

pub async fn run(store: Arc<dyn ObjectStore>) {
    let tenant = Uuid::now_v7();
    let member = Uuid::now_v7();
    let server = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let device = service(
        &store,
        caller(Uuid::now_v7(), member, &[], &[Uuid::now_v7()]),
    )
    .with_replica()
    .with_client_ids();

    server
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    for definition in [
        field("tier", FieldType::Text),
        field("secret", FieldType::Text),
    ] {
        server
            .create_field("custom:account", definition, context())
            .await
            .expect("field");
    }
    server
        .create_quick_action(
            "custom:account",
            serde_json::from_value(json!({
                "name": "promote", "label": "Promote", "kind": "update", "fields": ["custom:tier"]
            }))
            .expect("action"),
            context(),
        )
        .await
        .expect("action");

    // Nothing set: the device takes everything off.
    let (objects, settings) = described(&server).await;
    device
        .apply_remote_definitions_with(&objects, Some(settings))
        .await
        .expect("first definitions");
    let taken = device.assistant_policy().await.expect("device policy");
    assert!(!taken.settings.enabled);
    assert!(taken.objects.iter().all(|policy| !policy.exposed));

    // Turned on: the switch and the policy reach the device.
    let before = server
        .list_changes(None, Some(1), None)
        .await
        .expect("changes")
        .metadata_version;
    server
        .update_assistant_settings(
            UpdateAssistantSettings {
                expected_version: 0,
                enabled: true,
            },
            context(),
        )
        .await
        .expect("switch");
    server
        .update_object_assistant_policy(
            "custom:account",
            change(json!({
                "expected_version": 0, "exposed": true, "writable": true, "indexable": true,
                "excluded_fields": ["name", "custom:secret"], "offered_actions": ["promote"]
            })),
            context(),
        )
        .await
        .expect("policy");
    let after = server
        .list_changes(None, Some(1), None)
        .await
        .expect("changes")
        .metadata_version;
    assert_eq!(after, before + 2, "each change moves the metadata version");
    let (objects, settings) = described(&server).await;
    let applied = device
        .apply_remote_definitions_with(&objects, Some(settings))
        .await
        .expect("definitions");
    assert_eq!(
        applied.assistant_policy,
        vec!["workspace", "custom:account"]
    );
    let taken = device.assistant_policy().await.expect("device policy");
    assert!(taken.settings.enabled);
    let account = device
        .object_assistant_policy("custom:account")
        .await
        .expect("account");
    assert!(account.exposed && account.writable && account.indexable);
    assert_eq!(account.excluded_fields, vec!["name", "custom:secret"]);
    assert_eq!(account.offered_actions, vec!["promote"]);
    assert_eq!(account.version, 1);

    // Taking them again changes nothing.
    let applied = device
        .apply_remote_definitions_with(&objects, Some(settings))
        .await
        .expect("again");
    assert!(applied.assistant_policy.is_empty());

    // An action or field the replica does not hold is left out: it only
    // narrows.
    let mut narrowed = objects.clone();
    if let Some(policy) = narrowed[0].assistant.as_mut() {
        policy.offered_actions.push("unknown".into());
        policy.excluded_fields.push("custom:unknown".into());
        policy.version += 1;
    }
    device
        .apply_remote_definitions_with(&narrowed, Some(settings))
        .await
        .expect("unknown names");
    let account = device
        .object_assistant_policy("custom:account")
        .await
        .expect("account");
    assert_eq!(account.offered_actions, vec!["promote"]);
    assert_eq!(account.excluded_fields, vec!["name", "custom:secret"]);

    // A policy the server does not send reads as off.
    let mut missing = objects.clone();
    missing[0].assistant = None;
    device
        .apply_remote_definitions_with(&missing, Some(RemoteAssistantSettings::default()))
        .await
        .expect("no policy");
    let taken = device.assistant_policy().await.expect("device policy");
    assert!(!taken.settings.enabled);
    let account = device
        .object_assistant_policy("custom:account")
        .await
        .expect("account");
    assert!(!account.exposed && !account.writable && !account.indexable);
    assert!(account.offered_actions.is_empty());
}
