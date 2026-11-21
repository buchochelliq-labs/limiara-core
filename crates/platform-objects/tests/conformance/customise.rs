//! Customising one object on every backend (ADR 0035 G10): a role granted
//! `customise` on an object lets its members add, change and retire that
//! object's workspace fields, validation rules, quick actions and
//! automations without `objects.manage`. It grants nothing on records or on
//! other objects; field permissions, a plugin field's allowlist, object
//! settings and object permissions stay with administrators.

use super::{caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const PROJECT: &str = "custom:project";
const INVOICE: &str = "custom:invoice";

fn denied<T: std::fmt::Debug>(result: Result<T, ObjectError>) {
    assert!(
        matches!(result, Err(ObjectError::PermissionDenied)),
        "expected a refusal, got {result:?}"
    );
}

fn parse<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> T {
    serde_json::from_value(value).expect("input")
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    for (name, label) in [("project", "Project"), ("invoice", "Invoice")] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    let role = Uuid::now_v7();
    let stored = admin
        .set_object_permission(
            PROJECT,
            role,
            AccessFlags {
                read: true,
                customise: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("grant customise");
    assert!(stored.access.customise && !stored.access.edit);
    let listed = admin
        .object_permissions(PROJECT)
        .await
        .expect("permissions");
    assert!(listed
        .iter()
        .any(|row| row.role_id == role && row.access.customise));

    let customiser = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));
    let stranger = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));

    // --- Fields on the object the role customises ---------------------------
    let created = customiser
        .create_field(PROJECT, field("code", FieldType::Text), context())
        .await
        .expect("a customiser adds a field");
    let changed = customiser
        .update_field(
            PROJECT,
            "custom:code",
            parse(json!({"expected_version": created.version, "label": "Project code"})),
            context(),
        )
        .await
        .expect("and changes it");
    assert_eq!(changed.label, "Project code");
    // Field permissions stay with administrators.
    let mut protected = field("secret", FieldType::Text);
    protected.read_permission = Some("projects.secret.read".into());
    denied(customiser.create_field(PROJECT, protected, context()).await);
    denied(
        customiser
            .update_field(
                PROJECT,
                "custom:code",
                parse(json!({
                    "expected_version": changed.version,
                    "read_permission": "projects.secret.read",
                })),
                context(),
            )
            .await,
    );

    // --- Rules, quick actions and automations --------------------------------
    let rule = customiser
        .create_validation_rule(
            PROJECT,
            parse(json!({"name": "code_set", "condition": "ISBLANK(custom:code)", "message": "Set a code"})),
            context(),
        )
        .await
        .expect("a validation rule");
    customiser
        .delete_validation_rule(PROJECT, &rule.name, rule.version, context())
        .await
        .expect("and its removal");
    customiser
        .create_quick_action(
            PROJECT,
            parse(json!({"name": "recode", "label": "Recode", "kind": "update", "fields": ["custom:code"]})),
            context(),
        )
        .await
        .expect("a quick action");
    customiser
        .create_automation(
            PROJECT,
            parse(json!({
                "name": "tidy", "trigger": "create_or_update", "timing": "before_save",
                "actions": [{"type": "update_record", "values": [
                    {"field": "custom:code", "formula": "TRIM(custom:code)"}]}]})),
            context(),
        )
        .await
        .expect("an automation");
    assert_eq!(
        customiser
            .automations(PROJECT)
            .await
            .expect("automations")
            .len(),
        1
    );
    let kept = customiser
        .delete_field(PROJECT, "custom:code", changed.version, context())
        .await
        .expect_err("a field an automation sets stays");
    assert!(
        matches!(kept, ObjectError::Conflict(_)),
        "refused as in use, not as forbidden: {kept:?}"
    );
    let removable = customiser
        .create_field(PROJECT, field("spare", FieldType::Text), context())
        .await
        .expect("another field");
    customiser
        .delete_field(PROJECT, "custom:spare", removable.version, context())
        .await
        .expect("a customiser retires a field");

    // --- Nothing beyond that object, and nothing on records -----------------
    denied(
        customiser
            .create_field(INVOICE, field("code", FieldType::Text), context())
            .await,
    );
    denied(customiser.automations(INVOICE).await);
    denied(
        customiser
            .create_record(PROJECT, &super::body(json!({"name": "Apollo"})), context())
            .await,
    );
    denied(
        customiser
            .set_object_permission(PROJECT, role, AccessFlags::FULL, context())
            .await,
    );
    let version = admin.describe(PROJECT).await.expect("describe").0.version;
    denied(
        customiser
            .update_object(
                PROJECT,
                parse(json!({"expected_version": version, "label": "Programme"})),
                context(),
            )
            .await,
    );
    denied(customiser.delete_object(PROJECT, version, context()).await);

    // --- Without the grant, nothing; missing objects say nothing ------------
    denied(
        stranger
            .create_field(PROJECT, field("other", FieldType::Text), context())
            .await,
    );
    denied(
        stranger
            .create_field("custom:nothing", field("other", FieldType::Text), context())
            .await,
    );
    assert!(matches!(
        customiser
            .create_field("custom:nothing", field("other", FieldType::Text), context())
            .await,
        Err(ObjectError::NotFound)
    ));

    // --- Taking the grant away takes the right away --------------------------
    admin
        .set_object_permission(
            PROJECT,
            role,
            AccessFlags {
                read: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("revoke");
    denied(
        customiser
            .create_field(PROJECT, field("later", FieldType::Text), context())
            .await,
    );
}
