//! The Activities plugin's objects and related actions on every backend
//! (ADR 0033 §2 and §3). Administrators choose a plugin related-to field's
//! targets, and only those; the allowlist may be empty, which refuses every
//! value; upgrades keep the workspace's allowlist. An object entering the
//! allowlist gains the plugin's "Log a call" quick action once: deleting it
//! sticks until the object leaves and enters again, and an action that
//! cannot be added is skipped and audited without failing the change.

use super::{body, caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::service::PLUGIN_OWNED;
use platform_objects::*;
use plugin_contract::{Manifest, RelatedActionDecl};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const PLUGIN: &str = "platform.activities";
const TASK: &str = "platform.activities:task";
const EVENT: &str = "platform.activities:event";
const RELATED_TO: &str = "platform.activities:related_to";
const WHO: &str = "platform.activities:who";

/// What the store test checks against its own tables.
pub struct Report {
    /// Related actions the engine skipped, each audited once.
    pub skipped: i64,
}

pub fn activities() -> Manifest {
    Manifest::parse_toml(include_str!("../../../../plugins/activities/plugin.toml"))
        .expect("the Activities manifest parses")
}

fn change(value: Value) -> UpdateField {
    serde_json::from_value(value).expect("field change")
}

fn conflict(result: Result<impl std::fmt::Debug, ObjectError>) -> &'static str {
    match result {
        Err(ObjectError::Conflict(code)) => code,
        other => panic!("expected a conflict, got {other:?}"),
    }
}

async fn field_version(admin: &ObjectService, object: &str, field: &str) -> i64 {
    admin
        .describe(object)
        .await
        .expect("describe")
        .0
        .field(field)
        .expect("field")
        .version
}

async fn allow(admin: &ObjectService, field: &str, objects: &[&str]) -> FieldDefinition {
    let version = field_version(admin, TASK, field).await;
    admin
        .update_field(
            TASK,
            field,
            change(json!({"expected_version": version, "reference_objects": objects})),
            context(),
        )
        .await
        .expect("administrators choose a plugin related-to field's targets")
}

async fn action_names(admin: &ObjectService, object: &str) -> Vec<String> {
    admin
        .quick_actions(object)
        .await
        .expect("quick actions")
        .into_iter()
        .map(|action| action.name)
        .collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> Report {
    let member = Uuid::now_v7();
    let admin = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, PLUGIN),
    );
    let manifest = activities();
    let mut skipped = 0;

    // --- Install: Task and Event, with empty allowlists ----------------------
    let installed = installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    assert_eq!(installed.created_objects, vec![TASK, EVENT]);
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    let (task, _) = admin.describe(TASK).await.expect("task");
    assert_eq!(task.name_label, "Subject");
    assert_eq!(task.sharing_model, SharingModel::Private);
    for name in [RELATED_TO, WHO] {
        let related = task.field(name).expect("related-to field");
        assert_eq!(related.field_type, FieldType::RelatedTo);
        assert!(related.reference_objects.is_empty());
    }
    let (event, _) = admin.describe(EVENT).await.expect("event");
    assert!(
        event
            .field("platform.activities:starts_at")
            .expect("start")
            .required
    );

    // --- An empty allowlist refuses every value -----------------------------
    admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let acme = admin
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("an account");
    let refused = admin
        .create_record(
            TASK,
            &body(json!({"name": "Call Acme", RELATED_TO: acme.public_id})),
            context(),
        )
        .await
        .expect_err("nothing is allowed yet");
    assert_eq!(codes(&refused), ["records.reference_object_not_allowed"]);
    let refused = admin
        .reference_candidates(TASK, RELATED_TO, Some("custom:account"), None, None)
        .await
        .expect_err("no candidates while nothing is allowed");
    assert_eq!(codes(&refused), ["records.reference_object_not_allowed"]);
    let plain = admin
        .create_record(TASK, &body(json!({"name": "Plan the week"})), context())
        .await
        .expect("a task about nothing");
    assert_eq!(
        plain.values.get("platform.activities:status"),
        Some(&json!("open"))
    );
    assert_eq!(
        plain.values.get("platform.activities:type"),
        Some(&json!("to_do"))
    );

    // --- Only the allowlist is the workspace's to change --------------------
    let version = field_version(&admin, TASK, RELATED_TO).await;
    for refused in [
        json!({"expected_version": version, "label": "About"}),
        json!({"expected_version": version, "label": "About", "reference_objects": ["custom:account"]}),
        json!({"expected_version": version, "required": true}),
    ] {
        assert_eq!(
            conflict(
                admin
                    .update_field(TASK, RELATED_TO, change(refused), context())
                    .await
            ),
            PLUGIN_OWNED
        );
    }
    let status_version = field_version(&admin, TASK, "platform.activities:status").await;
    assert_eq!(
        conflict(
            admin
                .update_field(
                    TASK,
                    "platform.activities:status",
                    change(json!({"expected_version": status_version, "reference_objects": []})),
                    context(),
                )
                .await
        ),
        PLUGIN_OWNED,
        "only a related-to field has an allowlist to change"
    );
    let version = field_version(&admin, TASK, RELATED_TO).await;
    let refused = admin
        .update_field(
            TASK,
            RELATED_TO,
            change(json!({"expected_version": version, "reference_objects": ["custom:nope"]})),
            context(),
        )
        .await
        .expect_err("an unknown object");
    assert_eq!(codes(&refused), ["objects.reference_object_unknown"]);

    // --- Entering the allowlist adds Log a call ------------------------------
    let related = allow(&admin, RELATED_TO, &["custom:account"]).await;
    assert_eq!(related.reference_objects, ["custom:account"]);
    let actions = admin
        .quick_actions("custom:account")
        .await
        .expect("account actions");
    assert_eq!(actions.len(), 1);
    let log_a_call = &actions[0];
    assert_eq!(log_a_call.name, "log_a_call");
    assert_eq!(log_a_call.label, "Log a call");
    assert_eq!(log_a_call.kind, QuickActionKind::Create);
    assert_eq!(log_a_call.target_object.as_deref(), Some(TASK));
    assert_eq!(log_a_call.target_field.as_deref(), Some(RELATED_TO));
    assert_eq!(
        log_a_call.fields,
        ["name", "platform.activities:description"]
    );
    assert_eq!(
        log_a_call.presets.get("platform.activities:type"),
        Some(&json!("call"))
    );
    // Running it is the normal create API with the presets and the
    // related-to field set to the current record.
    let mut values = body(json!({
        "name": "Called about the renewal",
        "platform.activities:description": "They will sign next week",
        RELATED_TO: acme.public_id,
    }));
    for (name, value) in &log_a_call.presets {
        values.insert(name.clone(), value.clone());
    }
    let call = admin
        .create_record(TASK, &values, context())
        .await
        .expect("a logged call");
    assert_eq!(
        call.values.get("platform.activities:status"),
        Some(&json!("completed"))
    );
    assert_eq!(
        call.values.get("platform.activities:type"),
        Some(&json!("call"))
    );
    assert_eq!(
        call.values.get(RELATED_TO),
        Some(&json!(acme.public_id.clone()))
    );
    // Who has no related action.
    admin
        .create_object(object("contact", "Contact"), context())
        .await
        .expect("contact");
    allow(&admin, WHO, &["custom:contact"]).await;
    assert!(action_names(&admin, "custom:contact").await.is_empty());

    // --- Disabling hides the action, resuming brings the same one back -----
    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    assert!(
        action_names(&admin, "custom:account").await.is_empty(),
        "an action making a disabled plugin's records is hidden"
    );
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    let again = admin
        .quick_actions("custom:account")
        .await
        .expect("account actions");
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].id, log_a_call.id, "not created twice");

    // --- Administrators may delete it, and it stays deleted -----------------
    admin
        .delete_quick_action("custom:account", "log_a_call", again[0].version, context())
        .await
        .expect("administrators remove it");
    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    assert!(action_names(&admin, "custom:account").await.is_empty());
    admin
        .create_object(object("case", "Case"), context())
        .await
        .expect("case");
    allow(&admin, RELATED_TO, &["custom:account", "custom:case"]).await;
    assert!(
        action_names(&admin, "custom:account").await.is_empty(),
        "an object still in the allowlist is not offered it again"
    );
    assert_eq!(action_names(&admin, "custom:case").await, ["log_a_call"]);

    // --- Leaving the allowlist deletes it; entering again adds it ----------
    allow(&admin, RELATED_TO, &["custom:case"]).await;
    let kept = admin
        .get_record(TASK, &call.public_id, false)
        .await
        .expect("the logged call");
    assert_eq!(
        kept.values.get(RELATED_TO),
        Some(&json!(acme.public_id.clone())),
        "values to an object taken out stay"
    );
    allow(&admin, RELATED_TO, &["custom:account", "custom:case"]).await;
    assert_eq!(action_names(&admin, "custom:account").await, ["log_a_call"]);
    allow(&admin, RELATED_TO, &["custom:account"]).await;
    assert!(
        action_names(&admin, "custom:case").await.is_empty(),
        "the cascade deletes create actions of an object taken out"
    );

    // --- An action that cannot be added is skipped, not an error -----------
    admin
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal");
    admin
        .create_quick_action(
            "custom:deal",
            serde_json::from_value(json!({
                "name": "log_a_call", "label": "Log it", "kind": "update", "fields": ["name"],
            }))
            .expect("action"),
            context(),
        )
        .await
        .expect("the deal's own log_a_call");
    admin
        .create_object(object("lead", "Lead"), context())
        .await
        .expect("lead");
    for index in 0..10 {
        admin
            .create_quick_action(
                "custom:lead",
                serde_json::from_value(json!({
                    "name": format!("rename_{index}"), "label": "Rename", "kind": "update",
                    "fields": ["name"],
                }))
                .expect("action"),
                context(),
            )
            .await
            .expect("lead actions");
    }
    allow(
        &admin,
        RELATED_TO,
        &["custom:account", "custom:deal", "custom:lead"],
    )
    .await;
    skipped += 2;
    let deal_actions = admin.quick_actions("custom:deal").await.expect("deal");
    assert_eq!(deal_actions.len(), 1);
    assert_eq!(
        deal_actions[0].kind,
        QuickActionKind::Update,
        "the administrator's action of that name stays"
    );
    assert_eq!(action_names(&admin, "custom:lead").await.len(), 10);

    // --- Upgrades keep the allowlist and offer new declarations ------------
    let mut upgrade = activities();
    upgrade.version = "0.2.0".parse().expect("version");
    let task_decl = &mut upgrade.objects[0];
    task_decl
        .fields
        .iter_mut()
        .find(|field| field.name == "related_to")
        .expect("related_to")
        .reference_objects = vec![EVENT.into()];
    task_decl.related_actions.push(RelatedActionDecl {
        field: "related_to".into(),
        name: "log_an_email".into(),
        label: "Log an email".into(),
        fields: vec!["name".into()],
        presets: [
            ("platform.activities:type".to_owned(), json!("email")),
            ("platform.activities:status".to_owned(), json!("completed")),
        ]
        .into_iter()
        .collect(),
    });
    installer
        .install_plugin_objects(&upgrade, context())
        .await
        .expect("upgrade");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume after the upgrade");
    let (task, _) = admin.describe(TASK).await.expect("task");
    assert_eq!(
        task.field(RELATED_TO)
            .expect("related_to")
            .reference_objects,
        ["custom:account", "custom:deal", "custom:lead"],
        "an upgrade keeps the administrators' allowlist"
    );
    assert_eq!(
        action_names(&admin, "custom:account").await,
        ["log_a_call", "log_an_email"]
    );
    assert_eq!(action_names(&admin, "custom:deal").await.len(), 2);
    skipped += 1; // the lead is full

    // --- The allowlist may be emptied again; workspace fields need one -----
    allow(&admin, RELATED_TO, &[]).await;
    assert!(action_names(&admin, "custom:account").await.is_empty());
    assert_eq!(
        action_names(&admin, "custom:deal").await,
        ["log_a_call"],
        "only create actions that depend on the field go"
    );
    let mut regarding = field("regarding", FieldType::RelatedTo);
    regarding.reference_objects = Some(Vec::new());
    let refused = admin
        .create_field("custom:case", regarding, context())
        .await
        .expect_err("a workspace related-to field allows 1 to 10 objects");
    assert_eq!(codes(&refused), ["objects.reference_objects_invalid"]);

    // --- Another plugin's object out of service is offered on its return ---
    let tracker = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, "example.tracker"),
    );
    tracker
        .install_plugin_objects(&super::plugin_objects::fixture(), context())
        .await
        .expect("install the tracker");
    tracker
        .resume_plugin_objects("example.tracker", context())
        .await
        .expect("resume the tracker");
    const MILESTONE: &str = "example.tracker:milestone";
    allow(&admin, RELATED_TO, &[MILESTONE]).await;
    assert_eq!(
        action_names(&admin, MILESTONE).await,
        ["log_a_call", "log_an_email"]
    );
    tracker
        .suspend_plugin_objects("example.tracker", context())
        .await
        .expect("disable the tracker");
    let mut later = upgrade.clone();
    later.version = "0.3.0".parse().expect("version");
    later.objects[0].related_actions.push(RelatedActionDecl {
        field: "related_to".into(),
        name: "log_a_meeting".into(),
        label: "Log a meeting".into(),
        fields: vec!["name".into()],
        presets: [
            ("platform.activities:type".to_owned(), json!("meeting")),
            ("platform.activities:status".to_owned(), json!("completed")),
        ]
        .into_iter()
        .collect(),
    });
    installer
        .install_plugin_objects(&later, context())
        .await
        .expect("upgrade while the tracker is disabled");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume after the upgrade");
    tracker
        .resume_plugin_objects("example.tracker", context())
        .await
        .expect("enable the tracker again");
    assert_eq!(
        action_names(&admin, MILESTONE).await,
        ["log_a_call", "log_an_email", "log_a_meeting"],
        "a declaration that could not reach a suspended object reaches it on its return"
    );

    Report { skipped }
}
