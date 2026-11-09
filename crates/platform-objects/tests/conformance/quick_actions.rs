//! Quick actions on every backend (ADR 0032): checked when saved, read by
//! anyone who can read the object, changed only by administrators, and
//! updated or deleted with the fields and objects they depend on.

use super::{caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// A preset value that must never reach an event or audit record.
pub const SECRET_PRESET: &str = "preset-secret-value";

fn action(value: Value) -> CreateQuickAction {
    serde_json::from_value(value).expect("action")
}

fn change(value: Value) -> UpdateQuickAction {
    serde_json::from_value(value).expect("change")
}

/// `(field, code)` of a refused change.
fn issues(error: ObjectError) -> Vec<(String, &'static str)> {
    match error {
        ObjectError::Validation(issues) => issues
            .into_iter()
            .map(|issue| (issue.field, issue.code))
            .collect(),
        other => panic!("expected validation issues, got {other:?}"),
    }
}

fn names(actions: &[QuickAction]) -> Vec<&str> {
    actions.iter().map(|action| action.name.as_str()).collect()
}

async fn field_version(service: &ObjectService, object: &str, field: &str) -> i64 {
    let (definition, _) = service.describe(object).await.expect("describe");
    definition.field(field).expect("field").version
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let member_role = Uuid::now_v7();
    let member = service(&store, caller(tenant, Uuid::now_v7(), &[], &[member_role]));
    let stranger = service(
        &store,
        caller(tenant, Uuid::now_v7(), &[], &[Uuid::now_v7()]),
    );

    for (name, label) in [
        ("account", "Account"),
        ("contact", "Contact"),
        ("task", "Task"),
        ("other", "Other"),
        ("scratch", "Scratch"),
    ] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    let mut region = field("region", FieldType::Picklist);
    region.options = vec![option("emea", false), option("closed", false)];
    let mut score = field("score", FieldType::Formula);
    score.formula = Some("1".into());
    score.formula_type = Some(FieldType::Number);
    for definition in [
        region,
        field("rating", FieldType::Number),
        field("notes", FieldType::Text),
        score,
    ] {
        admin
            .create_field("custom:account", definition, context())
            .await
            .expect("account field");
    }
    let mut master = field("account", FieldType::MasterDetail);
    master.reference_object = Some("custom:account".into());
    for definition in [master, field("subject", FieldType::Text)] {
        admin
            .create_field("custom:task", definition, context())
            .await
            .expect("task field");
    }
    let mut tasks = field("tasks", FieldType::Rollup);
    tasks.rollup = Some(CreateRollup {
        function: RollupFunction::Count,
        object: "custom:task".into(),
        relationship: "custom:account".into(),
        field: None,
    });
    admin
        .create_field("custom:account", tasks, context())
        .await
        .expect("roll-up");
    let lookup = |name: &str, target: &str| {
        let mut definition = field(name, FieldType::Lookup);
        definition.reference_object = Some(target.into());
        definition
    };
    let mut role = field("role", FieldType::Picklist);
    role.options = vec![option("buyer", false), option("user", false)];
    for definition in [
        lookup("account", "custom:account"),
        lookup("account2", "custom:account"),
        lookup("other", "custom:other"),
        field("email", FieldType::Email),
        role,
        field("notes", FieldType::Text),
    ] {
        admin
            .create_field("custom:contact", definition, context())
            .await
            .expect("contact field");
    }
    admin
        .create_field(
            "custom:scratch",
            lookup("parent", "custom:scratch"),
            context(),
        )
        .await
        .expect("self lookup");
    admin
        .set_object_permission(
            "custom:account",
            member_role,
            AccessFlags {
                read: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("member reads accounts");

    // Both kinds are created; presets are normalised as record values.
    let close = admin
        .create_quick_action(
            "custom:account",
            action(json!({
                "name": "close", "label": " Close ", "kind": "update",
                "fields": ["custom:rating", "name"],
                "presets": {"custom:region": "closed", "custom:notes": SECRET_PRESET}
            })),
            context(),
        )
        .await
        .expect("update action");
    assert_eq!(
        (close.label.as_str(), close.position, close.version),
        ("Close", 0, 1)
    );
    assert_eq!(close.kind, QuickActionKind::Update);
    assert_eq!(close.presets["custom:region"], json!("closed"));
    assert_eq!(
        (close.target_object.clone(), close.target_field.clone()),
        (None, None)
    );
    let (contact, _) = admin.describe("custom:contact").await.expect("contact");
    let add_contact = admin
        .create_quick_action(
            "custom:account",
            action(json!({
                "name": "add_contact", "label": "Add contact", "kind": "create",
                "target_object": "custom:contact", "target_field": "custom:account",
                "fields": ["name", "custom:email", "custom:role"],
                "presets": {"custom:role": "buyer", "custom:notes": "from account"}
            })),
            context(),
        )
        .await
        .expect("create action");
    assert_eq!(add_contact.position, 1);
    assert_eq!(add_contact.target_object_id, Some(contact.id));
    assert_eq!(add_contact.target_object.as_deref(), Some("custom:contact"));
    assert_eq!(add_contact.target_field.as_deref(), Some("custom:account"));
    admin
        .create_quick_action(
            "custom:account",
            action(json!({
                "name": "add_task", "label": "Add task", "kind": "create", "position": 5,
                "target_object": "custom:task", "target_field": "custom:account",
                "fields": ["name", "custom:subject"]
            })),
            context(),
        )
        .await
        .expect("create action through master-detail");

    // Every check, on create.
    let long_label = "x".repeat(81);
    let many_fields: Vec<String> = (0..21).map(|index| format!("custom:f{index}")).collect();
    let many_presets: serde_json::Map<String, Value> = (0..21)
        .map(|index| (format!("custom:p{index}"), json!(1)))
        .collect();
    let update = |extra: Value| {
        let mut body = json!({"name": "bad", "label": "Bad", "kind": "update", "fields": ["name"]});
        for (key, value) in extra.as_object().expect("extra") {
            body[key] = value.clone();
        }
        body
    };
    let create = |extra: Value| {
        let mut body = json!({
            "name": "bad", "label": "Bad", "kind": "create",
            "target_object": "custom:contact", "target_field": "custom:account",
            "fields": ["name"]
        });
        for (key, value) in extra.as_object().expect("extra") {
            if value.is_null() {
                body.as_object_mut().expect("body").remove(key);
            } else {
                body[key] = value.clone();
            }
        }
        body
    };
    for (body, field, code) in [
        (
            update(json!({"name": "Bad Name"})),
            "name",
            "quick_actions.name_invalid",
        ),
        (
            update(json!({"label": "  "})),
            "label",
            "quick_actions.label_invalid",
        ),
        (
            update(json!({"label": long_label})),
            "label",
            "quick_actions.label_invalid",
        ),
        (
            update(json!({"position": -1})),
            "position",
            "quick_actions.position_invalid",
        ),
        (
            update(json!({"fields": []})),
            "fields",
            "quick_actions.fields_invalid",
        ),
        (
            update(json!({"fields": many_fields})),
            "fields",
            "quick_actions.fields_invalid",
        ),
        (
            update(json!({"fields": ["custom:rating", "custom:rating"]})),
            "fields.custom:rating",
            "quick_actions.field_duplicate",
        ),
        (
            update(json!({"fields": ["custom:nope"]})),
            "fields.custom:nope",
            "quick_actions.field_unknown",
        ),
        (
            update(json!({"fields": ["custom:email"]})),
            "fields.custom:email",
            "quick_actions.field_unknown",
        ),
        (
            update(json!({"fields": ["custom:score"]})),
            "fields.custom:score",
            "quick_actions.field_not_allowed",
        ),
        (
            update(json!({"fields": ["custom:tasks"]})),
            "fields.custom:tasks",
            "quick_actions.field_not_allowed",
        ),
        (
            update(json!({"presets": {"custom:score": 1}})),
            "presets.custom:score",
            "quick_actions.field_not_allowed",
        ),
        (
            update(json!({"presets": {"custom:tasks": 1}})),
            "presets.custom:tasks",
            "quick_actions.field_not_allowed",
        ),
        (
            update(json!({"presets": {"custom:nope": 1}})),
            "presets.custom:nope",
            "quick_actions.field_unknown",
        ),
        (
            update(json!({"presets": {"custom:rating": "many"}})),
            "presets.custom:rating",
            "records.value_type",
        ),
        (
            update(json!({"presets": {"custom:region": "mars"}})),
            "presets.custom:region",
            "records.picklist_value",
        ),
        (
            update(json!({"presets": {"name": ""}})),
            "presets.name",
            "records.name_invalid",
        ),
        (
            update(json!({"presets": many_presets})),
            "presets",
            "quick_actions.presets_invalid",
        ),
        (
            update(json!({"target_object": "custom:contact"})),
            "target_object",
            "quick_actions.target_not_allowed",
        ),
        (
            update(json!({"target_field": "custom:account"})),
            "target_field",
            "quick_actions.target_not_allowed",
        ),
        (
            create(json!({"target_object": null})),
            "target_object",
            "quick_actions.target_required",
        ),
        (
            create(json!({"target_field": null})),
            "target_field",
            "quick_actions.target_required",
        ),
        (
            create(json!({"target_object": "custom:nope"})),
            "target_object",
            "quick_actions.target_object_unknown",
        ),
        (
            create(json!({"target_field": "custom:other"})),
            "target_field",
            "quick_actions.target_field_invalid",
        ),
        (
            create(json!({"target_field": "custom:email"})),
            "target_field",
            "quick_actions.target_field_invalid",
        ),
        (
            create(json!({"target_field": "custom:nope"})),
            "target_field",
            "quick_actions.target_field_invalid",
        ),
        (
            create(json!({"fields": ["custom:account"]})),
            "fields.custom:account",
            "quick_actions.field_not_allowed",
        ),
        (
            create(json!({"presets": {"custom:account": null}})),
            "presets.custom:account",
            "quick_actions.field_not_allowed",
        ),
        (
            create(json!({"fields": ["custom:rating"]})),
            "fields.custom:rating",
            "quick_actions.field_unknown",
        ),
        (
            create(json!({"presets": {"custom:email": "not an address"}})),
            "presets.custom:email",
            "records.email_invalid",
        ),
    ] {
        let found = issues(
            admin
                .create_quick_action("custom:account", action(body.clone()), context())
                .await
                .expect_err("invalid action"),
        );
        assert!(
            found
                .iter()
                .any(|(name, found_code)| name == field && *found_code == code),
            "{field} {code} in {found:?} for {body}"
        );
    }
    assert!(matches!(
        admin
            .create_quick_action(
                "custom:account",
                action(update(json!({"name": "close"}))),
                context()
            )
            .await,
        Err(ObjectError::Conflict("quick_actions.duplicate_name"))
    ));
    assert!(matches!(
        member
            .create_quick_action(
                "custom:account",
                action(update(json!({"name": "mine"}))),
                context()
            )
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        admin
            .create_quick_action("custom:nope", action(update(json!({}))), context())
            .await,
        Err(ObjectError::NotFound)
    ));

    // Anyone who reads the object reads its actions, in position order; to
    // anyone else the object is unknown.
    let listed = member
        .quick_actions("custom:account")
        .await
        .expect("member reads actions");
    assert_eq!(names(&listed), ["close", "add_contact", "add_task"]);
    assert_eq!(listed[1].target_object.as_deref(), Some("custom:contact"));
    assert_eq!(
        admin.quick_actions("custom:account").await.expect("admin"),
        listed
    );
    for (service, api_name) in [
        (&stranger, "custom:account"),
        (&member, "custom:contact"),
        (&admin, "custom:nope"),
    ] {
        assert!(matches!(
            service.quick_actions(api_name).await,
            Err(ObjectError::NotFound)
        ));
    }
    assert!(admin
        .quick_actions("custom:contact")
        .await
        .expect("none")
        .is_empty());

    // Changes are versioned and checked like creates.
    assert!(matches!(
        admin
            .update_quick_action(
                "custom:account",
                "close",
                change(json!({"expected_version": 2, "label": "Late"})),
                context(),
            )
            .await,
        Err(ObjectError::VersionConflict)
    ));
    assert!(matches!(
        member
            .update_quick_action(
                "custom:account",
                "close",
                change(json!({"expected_version": 1, "label": "Mine"})),
                context(),
            )
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        admin
            .update_quick_action(
                "custom:account",
                "nope",
                change(json!({"expected_version": 1})),
                context(),
            )
            .await,
        Err(ObjectError::NotFound)
    ));
    let close = admin
        .update_quick_action(
            "custom:account",
            "close",
            change(json!({
                "expected_version": 1, "label": "Close deal", "position": 9,
                "fields": ["custom:rating", "custom:notes"],
                "presets": {"custom:region": "closed"}
            })),
            context(),
        )
        .await
        .expect("change");
    assert_eq!(
        (close.label.as_str(), close.position, close.version),
        ("Close deal", 9, 2)
    );
    assert_eq!(close.kind, QuickActionKind::Update);
    for (body, field, code) in [
        (
            json!({"fields": []}),
            "fields",
            "quick_actions.fields_invalid",
        ),
        (json!({"label": ""}), "label", "quick_actions.label_invalid"),
        (
            json!({"position": -3}),
            "position",
            "quick_actions.position_invalid",
        ),
        (
            json!({"presets": {"custom:rating": "x"}}),
            "presets.custom:rating",
            "records.value_type",
        ),
        (
            json!({"fields": ["custom:score"]}),
            "fields.custom:score",
            "quick_actions.field_not_allowed",
        ),
    ] {
        let mut body = body;
        body["expected_version"] = json!(2);
        let found = issues(
            admin
                .update_quick_action("custom:account", "close", change(body), context())
                .await
                .expect_err("invalid change"),
        );
        assert!(
            found
                .iter()
                .any(|(name, found_code)| name == field && *found_code == code),
            "{field} {code} in {found:?}"
        );
    }
    let invalid_target_change = issues(
        admin
            .update_quick_action(
                "custom:account",
                "add_contact",
                change(json!({"expected_version": 1, "fields": ["custom:account"]})),
                context(),
            )
            .await
            .expect_err("reference field in a create form"),
    );
    assert_eq!(
        invalid_target_change,
        [(
            "fields.custom:account".to_owned(),
            "quick_actions.field_not_allowed"
        )]
    );
    assert_eq!(
        names(
            &admin
                .quick_actions("custom:account")
                .await
                .expect("ordered")
        ),
        ["add_contact", "add_task", "close"]
    );

    // Deletion is versioned too.
    for (service, name, version) in [(&admin, "close", 1), (&member, "close", 2)] {
        let result = service
            .delete_quick_action("custom:account", name, version, context())
            .await;
        assert!(
            matches!(
                result,
                Err(ObjectError::VersionConflict | ObjectError::PermissionDenied)
            ),
            "{result:?}"
        );
    }
    assert!(matches!(
        admin
            .delete_quick_action("custom:account", "nope", 1, context())
            .await,
        Err(ObjectError::NotFound)
    ));

    // At most ten per object.
    for index in 0..7 {
        admin
            .create_quick_action(
                "custom:account",
                action(update(json!({"name": format!("extra_{index}")}))),
                context(),
            )
            .await
            .expect("within the limit");
    }
    let full = issues(
        admin
            .create_quick_action(
                "custom:account",
                action(update(json!({"name": "eleventh"}))),
                context(),
            )
            .await
            .expect_err("limit"),
    );
    assert_eq!(full, [("name".to_owned(), "quick_actions.limit_reached")]);
    for index in 0..7 {
        let deleted = admin
            .delete_quick_action("custom:account", &format!("extra_{index}"), 1, context())
            .await
            .expect("delete");
        assert_eq!(deleted.name, format!("extra_{index}"));
    }
    admin
        .create_quick_action(
            "custom:account",
            action(json!({
                "name": "rate", "label": "Rate", "kind": "update",
                "fields": ["custom:rating"]
            })),
            context(),
        )
        .await
        .expect("rate");

    // Deleting a field takes it out of the update actions on its object; an
    // action left with no fields goes.
    let version = field_version(&admin, "custom:account", "custom:rating").await;
    admin
        .delete_field("custom:account", "custom:rating", version, context())
        .await
        .expect("delete rating");
    let account_actions = admin.quick_actions("custom:account").await.expect("after");
    assert_eq!(
        names(&account_actions),
        ["add_contact", "add_task", "close"]
    );
    let close = &account_actions[2];
    assert_eq!(close.fields, ["custom:notes"]);
    assert_eq!(close.version, 3);
    let version = field_version(&admin, "custom:account", "custom:region").await;
    admin
        .delete_field("custom:account", "custom:region", version, context())
        .await
        .expect("delete region");
    let close = admin
        .quick_actions("custom:account")
        .await
        .expect("after")
        .into_iter()
        .find(|action| action.name == "close")
        .expect("close");
    assert!(close.presets.is_empty());
    assert_eq!(
        (close.fields.clone(), close.version),
        (vec!["custom:notes".to_owned()], 4)
    );

    // And out of the create actions whose child object it belongs to.
    admin
        .create_quick_action(
            "custom:account",
            action(json!({
                "name": "add_contact_two", "label": "Add contact (second)", "kind": "create",
                "target_object": "custom:contact", "target_field": "custom:account2",
                "fields": ["name"]
            })),
            context(),
        )
        .await
        .expect("second create action");
    admin
        .create_quick_action(
            "custom:contact",
            action(json!({
                "name": "edit", "label": "Edit", "kind": "update", "fields": ["custom:notes"]
            })),
            context(),
        )
        .await
        .expect("contact's own action");
    let version = field_version(&admin, "custom:contact", "custom:notes").await;
    admin
        .delete_field("custom:contact", "custom:notes", version, context())
        .await
        .expect("delete contact notes");
    assert!(
        admin
            .quick_actions("custom:contact")
            .await
            .expect("contact actions")
            .is_empty(),
        "the contact's action lost its only field"
    );
    let version = field_version(&admin, "custom:contact", "custom:email").await;
    admin
        .delete_field("custom:contact", "custom:email", version, context())
        .await
        .expect("delete contact email");
    let add_contact = admin
        .quick_actions("custom:account")
        .await
        .expect("after")
        .into_iter()
        .find(|action| action.name == "add_contact")
        .expect("add_contact");
    assert_eq!(add_contact.fields, ["name", "custom:role"]);
    assert_eq!(
        add_contact.presets,
        [("custom:role".to_owned(), json!("buyer"))]
            .into_iter()
            .collect()
    );
    assert_eq!(add_contact.version, 3);
    let version = field_version(&admin, "custom:task", "custom:subject").await;
    admin
        .delete_field("custom:task", "custom:subject", version, context())
        .await
        .expect("delete task subject");

    // Deleting a create action's reference field deletes the action.
    let version = field_version(&admin, "custom:contact", "custom:account").await;
    admin
        .delete_field("custom:contact", "custom:account", version, context())
        .await
        .expect("delete reference");
    let account_actions = admin.quick_actions("custom:account").await.expect("after");
    assert_eq!(
        names(&account_actions),
        ["add_contact_two", "add_task", "close"]
    );
    assert_eq!(account_actions[0].version, 1, "untouched");
    assert_eq!(account_actions[1].fields, ["name"]);
    assert_eq!(account_actions[1].version, 2);

    // Deleting the child object deletes the create actions that make its
    // records.
    admin
        .create_quick_action(
            "custom:contact",
            action(
                json!({"name": "rename", "label": "Rename", "kind": "update", "fields": ["name"]}),
            ),
            context(),
        )
        .await
        .expect("contact action");
    let contact_version = admin
        .describe("custom:contact")
        .await
        .expect("contact")
        .0
        .version;
    admin
        .delete_object("custom:contact", contact_version, context())
        .await
        .expect("delete contact");
    assert_eq!(
        names(&admin.quick_actions("custom:account").await.expect("after")),
        ["add_task", "close"]
    );

    // Deleting an object deletes its own actions, including create actions
    // through a lookup to itself.
    for body in [
        json!({"name": "edit", "label": "Edit", "kind": "update", "fields": ["name", "custom:parent"]}),
        json!({
            "name": "child", "label": "Add child", "kind": "create",
            "target_object": "custom:scratch", "target_field": "custom:parent", "fields": ["name"]
        }),
    ] {
        admin
            .create_quick_action("custom:scratch", action(body), context())
            .await
            .expect("scratch action");
    }
    let (scratch, _) = admin.describe("custom:scratch").await.expect("scratch");
    admin
        .delete_object("custom:scratch", scratch.version, context())
        .await
        .expect("delete scratch");
    let mut tx = store.begin(tenant).await.expect("tx");
    assert!(tx.quick_actions(scratch.id).await.expect("own").is_empty());
    assert!(tx
        .quick_actions_targeting(scratch.id)
        .await
        .expect("targeting")
        .is_empty());
    assert!(tx
        .quick_actions_targeting(contact.id)
        .await
        .expect("targeting contact")
        .is_empty());
    assert!(tx
        .quick_actions(contact.id)
        .await
        .expect("contact")
        .is_empty());
    tx.commit().await.expect("commit");
}
