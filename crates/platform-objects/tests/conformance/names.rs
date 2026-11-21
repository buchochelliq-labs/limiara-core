//! Record names composed from fields on every backend (ADR 0035 G17). An
//! object may name two or three of its own text fields and a separator; the
//! engine then writes every record's name itself, refuses writes to the name,
//! renames existing records when the declaration changes, and keeps the
//! fields from being deleted while they make the name. Plugins may declare it.

use super::{body, caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use plugin_contract::Manifest;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const PERSON: &str = "custom:person";

fn change(version: i64, name_from: Value) -> UpdateObject {
    serde_json::from_value(json!({"expected_version": version, "name_from": name_from}))
        .expect("object change")
}

fn names(first: &str, second: &str, separator: &str) -> Value {
    json!({"fields": [first, second], "separator": separator})
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let member = Uuid::now_v7();
    let admin = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let mut person = object("person", "Person");
    person.track_history = Some(true);
    admin
        .create_object(person, context())
        .await
        .expect("object");
    for (name, kind) in [
        ("first", FieldType::Text),
        ("last", FieldType::Text),
        ("age", FieldType::Number),
    ] {
        admin
            .create_field(PERSON, field(name, kind), context())
            .await
            .expect("field");
    }
    // Before a declaration, members name records themselves.
    let ada = admin
        .create_record(
            PERSON,
            &body(json!({"name": "Ada L", "custom:first": "Ada", "custom:last": "Lovelace"})),
            context(),
        )
        .await
        .expect("a named record");
    let nameless = admin
        .create_record(PERSON, &body(json!({"name": "Someone"})), context())
        .await
        .expect("a record without name fields");

    // --- The declaration names two or three of the object's text fields -----
    let version = |admin: &ObjectService| {
        let admin = admin.clone();
        async move { admin.describe(PERSON).await.expect("describe").0.version }
    };
    for refused in [
        json!({"fields": ["custom:first"]}),
        json!({"fields": ["custom:first", "custom:age"]}),
        json!({"fields": ["custom:first", "custom:nothing"]}),
        json!({"fields": ["custom:first", "custom:first"]}),
        json!({"fields": ["custom:first", "custom:last"], "separator": "------"}),
    ] {
        let error = admin
            .update_object(
                PERSON,
                change(version(&admin).await, refused.clone()),
                context(),
            )
            .await
            .expect_err("an invalid declaration");
        assert_eq!(codes(&error), ["objects.name_from_invalid"], "{refused}");
    }

    // --- Declaring it renames existing records ------------------------------
    let declared = admin
        .update_object(
            PERSON,
            change(
                version(&admin).await,
                names("custom:first", "custom:last", " "),
            ),
            context(),
        )
        .await
        .expect("compose names");
    assert_eq!(
        declared
            .name_from
            .as_ref()
            .map(|name_from| name_from.fields.len()),
        Some(2)
    );
    let renamed = admin
        .get_record(PERSON, &ada.public_id, false)
        .await
        .expect("ada");
    assert_eq!(renamed.name, "Ada Lovelace");
    assert_eq!(
        renamed.version,
        ada.version + 1,
        "a rename is a new version"
    );
    let history = admin
        .record_history(PERSON, &ada.public_id, None, None)
        .await
        .expect("history")
        .0
        .entries;
    assert!(
        history
            .iter()
            .any(|entry| entry.field.as_deref() == Some("name")
                && serde_json::to_value(entry.source).expect("source")["kind"] == json!("system")),
        "the rename is in history as a system change: {history:?}"
    );
    assert_eq!(
        admin
            .get_record(PERSON, &nameless.public_id, false)
            .await
            .expect("nameless")
            .name,
        "Someone",
        "a record with empty name fields keeps its name"
    );

    // --- The engine writes the name; members write its fields --------------
    let refused = admin
        .create_record(
            PERSON,
            &body(json!({"name": "Grace", "custom:first": "Grace"})),
            context(),
        )
        .await
        .expect_err("the name is composed");
    assert_eq!(codes(&refused), ["objects.name_composed"]);
    let refused = admin
        .create_record(PERSON, &body(json!({"custom:age": 36})), context())
        .await
        .expect_err("nothing to make the name from");
    assert_eq!(codes(&refused), ["records.name_required"]);
    let grace = admin
        .create_record(
            PERSON,
            &body(json!({"custom:first": "  Grace ", "custom:last": "Hopper"})),
            context(),
        )
        .await
        .expect("a composed name");
    assert_eq!(grace.name, "Grace Hopper");
    let only_last = admin
        .create_record(PERSON, &body(json!({"custom:last": "Turing"})), context())
        .await
        .expect("one part is enough");
    assert_eq!(only_last.name, "Turing");
    let married = admin
        .update_record(
            PERSON,
            &grace.public_id,
            grace.version,
            &body(json!({"custom:last": "Murray Hopper"})),
            context(),
        )
        .await
        .expect("a name field changes");
    assert_eq!(married.name, "Grace Murray Hopper");
    let aged = admin
        .update_record(
            PERSON,
            &married.public_id,
            married.version,
            &body(json!({"custom:age": 85})),
            context(),
        )
        .await
        .expect("another field changes");
    assert_eq!(aged.name, "Grace Murray Hopper");
    let refused = admin
        .update_record(
            PERSON,
            &aged.public_id,
            aged.version,
            &body(json!({"custom:first": null, "custom:last": null})),
            context(),
        )
        .await
        .expect_err("a name never becomes empty");
    assert_eq!(codes(&refused), ["records.name_required"]);
    // Search finds the composed name.
    let found = admin
        .query(QueryRequest {
            q: "SELECT name FROM custom:person WHERE name = 'Grace Murray Hopper'".into(),
            cursor: None,
            page_size: None,
            include_deleted: false,
            include_archived: false,
        })
        .await
        .expect("query");
    assert_eq!(found.rows.len(), 1);

    // --- Its fields stay while they make the name ---------------------------
    let first = admin.describe(PERSON).await.expect("describe").0;
    let first_version = first.field("custom:first").expect("first").version;
    assert!(matches!(
        admin
            .delete_field(PERSON, "custom:first", first_version, context())
            .await,
        Err(ObjectError::Conflict("objects.field_in_use"))
    ));

    // --- A new separator renames; clearing it hands the name back ----------
    admin
        .update_object(
            PERSON,
            change(
                version(&admin).await,
                names("custom:last", "custom:first", ", "),
            ),
            context(),
        )
        .await
        .expect("last, first");
    assert_eq!(
        admin
            .get_record(PERSON, &ada.public_id, false)
            .await
            .expect("ada")
            .name,
        "Lovelace, Ada"
    );
    admin
        .update_object(
            PERSON,
            change(version(&admin).await, Value::Null),
            context(),
        )
        .await
        .expect("members name records again");
    let kept = admin
        .get_record(PERSON, &ada.public_id, false)
        .await
        .expect("ada");
    assert_eq!(kept.name, "Lovelace, Ada", "names stay as they were");
    let renamed = admin
        .update_record(
            PERSON,
            &kept.public_id,
            kept.version,
            &body(json!({"name": "Countess of Lovelace"})),
            context(),
        )
        .await
        .expect("a member renames");
    assert_eq!(renamed.name, "Countess of Lovelace");

    // --- Plugins declare it, and an upgrade renames ------------------------
    let plugin = "example.people";
    let mut manifest =
        Manifest::parse_toml(include_str!("../../../../plugins/sales/plugin.toml")).expect("base");
    manifest.id = plugin.into();
    manifest.reports = Vec::new();
    manifest.objects = serde_json::from_value(json!([{
        "name": "contact", "label": "Contact", "plural_label": "Contacts", "key_prefix": "ctc",
        "name_from": {"fields": ["first_name", "last_name"]},
        "fields": [
            {"name": "first_name", "label": "First name", "field_type": "text"},
            {"name": "last_name", "label": "Last name", "field_type": "text"}
        ]
    }]))
    .expect("objects");
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, plugin),
    );
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(plugin, context())
        .await
        .expect("resume");
    let contact = admin
        .create_record(
            "example.people:contact",
            &body(
                json!({"example.people:first_name": "Alan", "example.people:last_name": "Turing"}),
            ),
            context(),
        )
        .await
        .expect("a contact");
    assert_eq!(contact.name, "Alan Turing");
    // Administrators cannot change a plugin's declaration.
    let owned = admin
        .describe("example.people:contact")
        .await
        .expect("describe")
        .0;
    assert!(matches!(
        admin
            .update_object(
                "example.people:contact",
                change(owned.version, Value::Null),
                context()
            )
            .await,
        Err(ObjectError::Conflict(_))
    ));
    manifest.objects[0]
        .name_from
        .as_mut()
        .expect("declared")
        .separator = " · ".into();
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("upgrade");
    assert_eq!(
        admin
            .get_record("example.people:contact", &contact.public_id, false)
            .await
            .expect("contact")
            .name,
        "Alan · Turing"
    );
}
