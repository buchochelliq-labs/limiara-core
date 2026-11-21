//! Field types and their limits on every backend (ADR 0035 slice 1). An
//! email field may allow addresses of up to 320 characters (G8): declared
//! when the field is created or changed later, and kept by the store. A
//! plugin upgrade that drops an email field's limit brings it back to 255.

use super::{body, caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    admin
        .create_object(object("member", "Member"), context())
        .await
        .expect("object");

    // --- G8: email addresses of up to 320 characters ------------------------
    let label = "b".repeat(60);
    let long = format!(
        "{}@{label}.{label}.{label}.{}.example",
        "a".repeat(64),
        "c".repeat(44)
    );
    assert_eq!(long.chars().count(), 300);
    let mut wide = field("email", FieldType::Email);
    wide.max_length = Some(320);
    let wide = admin
        .create_field("custom:member", wide, context())
        .await
        .expect("an email field of up to 320 characters");
    assert_eq!(wide.max_length, Some(320));
    let refused = {
        let mut too_wide = field("email_too_wide", FieldType::Email);
        too_wide.max_length = Some(321);
        admin
            .create_field("custom:member", too_wide, context())
            .await
            .expect_err("over 320")
    };
    assert_eq!(codes(&refused), ["objects.max_length_invalid"]);
    let narrow = admin
        .create_field(
            "custom:member",
            field("work_email", FieldType::Email),
            context(),
        )
        .await
        .expect("an email field with the default limit");
    assert_eq!(narrow.max_length, None);
    let refused = admin
        .create_record(
            "custom:member",
            &body(json!({"name": "Ada", "custom:work_email": long})),
            context(),
        )
        .await
        .expect_err("255 by default");
    assert_eq!(codes(&refused), ["records.value_too_long"]);
    let member = admin
        .create_record(
            "custom:member",
            &body(json!({"name": "Ada", "custom:email": long})),
            context(),
        )
        .await
        .expect("a 300-character address");
    let read = admin
        .get_record("custom:member", &member.public_id, false)
        .await
        .expect("read back");
    assert_eq!(read.values.get("custom:email"), Some(&json!(long)));
    // The limit can be raised on an existing field.
    let raised = admin
        .update_field(
            "custom:member",
            "custom:work_email",
            serde_json::from_value(json!({"expected_version": narrow.version, "max_length": 320}))
                .expect("change"),
            context(),
        )
        .await
        .expect("raise the limit");
    assert_eq!(raised.max_length, Some(320));

    // A plugin's email field follows its manifest's limit, and dropping the
    // limit brings it back to the default.
    let plugin = "example.mail";
    let mut manifest = plugin_contract::Manifest::parse_toml(include_str!(
        "../../../../plugins/sales/plugin.toml"
    ))
    .expect("base");
    manifest.id = plugin.into();
    let declared = |max_length: Option<i32>| {
        serde_json::from_value(json!([{
            "name": "contact", "label": "Contact", "plural_label": "Contacts", "key_prefix": "mlc",
            "fields": [{"name": "email", "label": "Email", "field_type": "email", "max_length": max_length}]
        }]))
        .expect("objects")
    };
    manifest.objects = declared(Some(100));
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, Uuid::now_v7(), plugin),
    );
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    let email_limit = |admin: ObjectService| async move {
        admin
            .describe("example.mail:contact")
            .await
            .expect("describe")
            .0
            .field("example.mail:email")
            .expect("email")
            .max_length
    };
    installer
        .resume_plugin_objects(plugin, context())
        .await
        .expect("resume");
    assert_eq!(email_limit(admin.clone()).await, Some(100));
    manifest.objects = declared(None);
    let upgraded = installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("upgrade");
    assert_eq!(
        upgraded.updated_fields,
        ["example.mail:contact/example.mail:email"]
    );
    assert_eq!(email_limit(admin.clone()).await, Some(255));
    assert!(
        installer
            .install_plugin_objects(&manifest, context())
            .await
            .expect("reinstall")
            .is_empty(),
        "the default matches an omitted limit"
    );
}
