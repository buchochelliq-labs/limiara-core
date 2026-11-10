//! Quick actions on a synced device (ADR 0032 §5, ADR 0024 §3), on SQLite,
//! which devices use. The replica takes an object's actions with its other
//! definitions, keeps only those it can run offline, and follows the
//! server's changes and deletions; changing an action moves the server's
//! metadata version so devices know to take them. Here the server is
//! another workspace in the same store.

use super::{body, caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const PLUGIN: &str = "platform.activities";
const TASK: &str = "platform.activities:task";
const RELATED_TO: &str = "platform.activities:related_to";

const MEMBER_ACCESS: AccessFlags = AccessFlags {
    read: true,
    create: true,
    edit: true,
    delete: true,
    view_all: false,
    modify_all: false,
};

fn action(value: Value) -> CreateQuickAction {
    serde_json::from_value(value).expect("action")
}

/// Every object the server describes to a member who cannot read
/// `custom:vault`, with its rules and quick actions as the API lists them.
async fn described(server: &ObjectService) -> Vec<RemoteObject> {
    let mut objects = Vec::new();
    for (listed, _) in server.list_objects().await.expect("objects") {
        if listed.api_name == "custom:vault" {
            continue;
        }
        // A suspended plugin object is not described to anyone.
        let Ok((object, _)) = server.describe(&listed.api_name).await else {
            continue;
        };
        let rules = server
            .validation_rules(&object.api_name)
            .await
            .expect("rules");
        let actions = server
            .quick_actions(&object.api_name)
            .await
            .expect("quick actions");
        objects.push(
            RemoteObject::from_definition(&object, MEMBER_ACCESS, Some(&rules))
                .with_quick_actions(&actions),
        );
    }
    objects
}

async fn metadata_version(server: &ObjectService) -> i64 {
    server
        .list_changes(None, Some(1), None)
        .await
        .expect("changes")
        .metadata_version
}

async fn names(device: &ObjectService, object: &str) -> Vec<String> {
    device
        .quick_actions(object)
        .await
        .expect("device actions")
        .into_iter()
        .map(|action| action.name)
        .collect()
}

async fn server_action(server: &ObjectService, object: &str, name: &str) -> QuickAction {
    server
        .quick_actions(object)
        .await
        .expect("server actions")
        .into_iter()
        .find(|action| action.name == name)
        .expect("server action")
}

pub async fn run(store: Arc<dyn ObjectStore>) {
    let tenant = Uuid::now_v7();
    let member = Uuid::now_v7();
    let server = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, PLUGIN),
    );
    let device = service(
        &store,
        caller(Uuid::now_v7(), member, &[], &[Uuid::now_v7()]),
    )
    .with_replica()
    .with_client_ids();

    // Accounts and contacts the member reads; a vault they cannot.
    for (name, label) in [
        ("account", "Account"),
        ("contact", "Contact"),
        ("vault", "Vault"),
    ] {
        server
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    let mut region = field("region", FieldType::Picklist);
    region.options = vec![option("emea", false), option("apac", false)];
    for definition in [field("tier", FieldType::Text), region] {
        server
            .create_field("custom:account", definition, context())
            .await
            .expect("account field");
    }
    let lookup = |name: &str, target: &str| {
        let mut definition = field(name, FieldType::Lookup);
        definition.reference_object = Some(target.into());
        definition
    };
    for definition in [
        lookup("employer", "custom:account"),
        lookup("vault_link", "custom:vault"),
        field("notes", FieldType::Text),
    ] {
        server
            .create_field("custom:contact", definition, context())
            .await
            .expect("contact field");
    }
    server
        .create_field(
            "custom:vault",
            lookup("account", "custom:account"),
            context(),
        )
        .await
        .expect("vault field");

    // Creating an action moves the metadata version, as a rule does.
    let before = metadata_version(&server).await;
    for (object, definition) in [
        (
            "custom:account",
            json!({"name": "close", "label": "Close", "kind": "update",
                   "fields": ["custom:tier"], "presets": {"custom:region": "emea"}}),
        ),
        (
            "custom:account",
            json!({"name": "add_contact", "label": "Add contact", "kind": "create",
                   "target_object": "custom:contact", "target_field": "custom:employer",
                   "fields": ["name"], "presets": {"custom:notes": "met at the fair"}}),
        ),
        // Its child object is one the member cannot read.
        (
            "custom:account",
            json!({"name": "add_vault", "label": "Add vault", "kind": "create",
                   "target_object": "custom:vault", "target_field": "custom:account",
                   "fields": ["name"]}),
        ),
        // Its field is a lookup to that object, which the replica leaves out.
        (
            "custom:contact",
            json!({"name": "lock", "label": "Lock", "kind": "update",
                   "fields": ["custom:vault_link"]}),
        ),
    ] {
        server
            .create_quick_action(object, action(definition), context())
            .await
            .expect("quick action");
    }
    let created = metadata_version(&server).await;
    assert_eq!(
        created,
        before + 4,
        "each action moves the metadata version"
    );

    // --- Actions arrive; those the device cannot run are deferred -----------
    let applied = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("definitions");
    assert_eq!(
        applied.quick_actions,
        vec!["custom:account.add_contact", "custom:account.close"]
    );
    assert_eq!(
        applied.deferred_quick_actions,
        vec!["custom:account.add_vault", "custom:contact.lock"]
    );
    assert_eq!(
        names(&device, "custom:account").await,
        ["close", "add_contact"]
    );
    assert!(names(&device, "custom:contact").await.is_empty());
    let local = device
        .quick_actions("custom:account")
        .await
        .expect("device actions");
    let add_contact = local
        .iter()
        .find(|action| action.name == "add_contact")
        .expect("add contact");
    assert_eq!(add_contact.kind, QuickActionKind::Create);
    assert_eq!(add_contact.target_object.as_deref(), Some("custom:contact"));
    assert_eq!(add_contact.target_field.as_deref(), Some("custom:employer"));
    assert_eq!(
        add_contact.presets.get("custom:notes"),
        Some(&json!("met at the fair"))
    );
    assert_eq!(add_contact.version, 1);

    // --- Taking the same definitions again changes nothing ------------------
    let again = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("again");
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(again.deferred_quick_actions, applied.deferred_quick_actions);

    // --- Running the create action offline is an ordinary local create -----
    let acme = device
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("offline account");
    let mut values = body(json!(add_contact.presets));
    values.insert("name".into(), json!("Ada"));
    values.insert(
        add_contact.target_field.clone().expect("reference"),
        json!(acme.public_id),
    );
    let ada = device
        .create_record("custom:contact", &values, context())
        .await
        .expect("the action's record, made offline");
    assert_eq!(
        ada.values.get("custom:notes"),
        Some(&json!("met at the fair"))
    );
    assert_eq!(
        ada.values.get("custom:employer"),
        Some(&json!(acme.public_id))
    );
    let queued = device.push_queue().await.expect("queue");
    assert!(
        queued.operations.len() >= 2,
        "the action's record waits to be pushed: {queued:?}"
    );

    // --- An update and a deletion follow ------------------------------------
    let close = server_action(&server, "custom:account", "close").await;
    server
        .update_quick_action(
            "custom:account",
            "close",
            serde_json::from_value(json!({
                "expected_version": close.version,
                "label": "Close won",
                "presets": {"custom:region": "apac"},
            }))
            .expect("change"),
            context(),
        )
        .await
        .expect("update action");
    let add = server_action(&server, "custom:account", "add_contact").await;
    server
        .delete_quick_action("custom:account", "add_contact", add.version, context())
        .await
        .expect("delete action");
    assert_eq!(
        metadata_version(&server).await,
        created + 2,
        "changing and deleting an action move the metadata version"
    );
    let applied = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("changed definitions");
    assert_eq!(
        applied.quick_actions,
        vec!["custom:account.add_contact", "custom:account.close"]
    );
    let local = device
        .quick_actions("custom:account")
        .await
        .expect("device actions");
    assert_eq!(local.len(), 1);
    assert_eq!(local[0].label, "Close won");
    assert_eq!(local[0].version, 2);
    assert_eq!(local[0].presets.get("custom:region"), Some(&json!("apac")));

    // --- A related action arrives as an ordinary create action -------------
    installer
        .install_plugin_objects(&super::related_actions::activities(), context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    let (task, _) = server.describe(TASK).await.expect("task");
    let related_to = task.field(RELATED_TO).expect("related to").version;
    server
        .update_field(
            TASK,
            RELATED_TO,
            serde_json::from_value(json!({
                "expected_version": related_to,
                "reference_objects": ["custom:account"],
            }))
            .expect("allowlist"),
            context(),
        )
        .await
        .expect("accounts may have tasks");
    let with_tasks = described(&server).await;
    let applied = device
        .apply_remote_definitions(&with_tasks)
        .await
        .expect("plugin definitions");
    assert_eq!(applied.quick_actions, vec!["custom:account.log_a_call"]);
    assert_eq!(
        names(&device, "custom:account").await,
        ["close", "log_a_call"]
    );

    // --- A suspended plugin object leaves no usable action -----------------
    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    let applied = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("suspended definitions");
    assert!(applied.objects.contains(&TASK.to_owned()), "{applied:?}");
    assert_eq!(applied.quick_actions, vec!["custom:account.log_a_call"]);
    assert_eq!(names(&device, "custom:account").await, ["close"]);
    // Even described with its action, as a server that still listed it
    // would, the action stays with the server while its object is gone.
    let mut stale = with_tasks.clone();
    stale.retain(|object| object.owner_plugin.as_deref() != Some(PLUGIN));
    let applied = device
        .apply_remote_definitions(&stale)
        .await
        .expect("stale definitions");
    assert!(applied.is_empty(), "{applied:?}");
    assert_eq!(
        applied.deferred_quick_actions,
        vec![
            "custom:account.add_vault",
            "custom:account.log_a_call",
            "custom:contact.lock"
        ]
    );
    assert_eq!(names(&device, "custom:account").await, ["close"]);
    // Resuming the plugin brings it back.
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume again");
    let applied = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("resumed definitions");
    assert_eq!(applied.quick_actions, vec!["custom:account.log_a_call"]);
    assert_eq!(
        names(&device, "custom:account").await,
        ["close", "log_a_call"]
    );

    // --- An object the server stops describing takes its actions along ----
    let mut without_accounts = described(&server).await;
    without_accounts.retain(|object| object.api_name != "custom:account");
    let applied = device
        .apply_remote_definitions(&without_accounts)
        .await
        .expect("account hidden");
    assert!(applied
        .quick_actions
        .contains(&"custom:account.close".to_owned()));
    let applied = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("account back");
    assert!(applied
        .quick_actions
        .contains(&"custom:account.close".to_owned()));
    assert_eq!(
        names(&device, "custom:account").await,
        ["close", "log_a_call"]
    );

    // --- Without the server's list, the replica's actions stay -------------
    let mut unlisted = described(&server).await;
    for object in &mut unlisted {
        object.quick_actions = None;
    }
    assert!(device
        .apply_remote_definitions(&unlisted)
        .await
        .expect("unlisted")
        .is_empty());
    assert_eq!(
        names(&device, "custom:account").await,
        ["close", "log_a_call"]
    );
}
