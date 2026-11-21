//! Field governance metadata on every backend (ADR 0035 G9): a field may
//! declare a data classification, a purpose, a retention period and
//! whether it is stored encrypted. The engine checks them with the rules
//! Contacts applies to its custom fields, stores them, returns them when
//! describing the field, replaces them on change, and keeps a plugin's
//! declarations the plugin's. Nothing enforces retention or encryption.

use super::{caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::service::PLUGIN_OWNED;
use platform_objects::*;
use plugin_contract::objects::ObjectDataClassification;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const PATIENT: &str = "custom:patient";
const PLUGIN: &str = "example.tracker";
const TICKET: &str = "example.tracker:ticket";
const SUMMARY: &str = "example.tracker:summary";

fn governance(value: Value) -> FieldGovernance {
    serde_json::from_value(value).expect("governance")
}

fn codes(error: &ObjectError) -> Vec<(String, &'static str)> {
    match error {
        ObjectError::Validation(issues) => issues
            .iter()
            .map(|issue| (issue.field.clone(), issue.code))
            .collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

async fn described(admin: &ObjectService, object: &str, field: &str) -> FieldDefinition {
    admin
        .describe(object)
        .await
        .expect("describe")
        .0
        .field(field)
        .expect("field")
        .clone()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let member = Uuid::now_v7();
    let admin = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    admin
        .create_object(object("patient", "Patient"), context())
        .await
        .expect("object");

    // --- Without declarations a field is unclassified ------------------------
    let plain = admin
        .create_field(PATIENT, field("ward", FieldType::Text), context())
        .await
        .expect("a plain field");
    assert!(plain.governance.is_empty());

    // --- Contacts' rules ------------------------------------------------------
    for (declared, expected) in [
        (
            json!({"data_classification": "personal"}),
            vec![("governance.purpose", "objects.purpose_required")],
        ),
        (
            json!({"data_classification": "sensitive", "purpose": "Care"}),
            vec![("governance.encrypted", "objects.encryption_required")],
        ),
        (
            json!({"data_classification": "sensitive"}),
            vec![
                ("governance.purpose", "objects.purpose_required"),
                ("governance.encrypted", "objects.encryption_required"),
            ],
        ),
        (
            json!({"data_classification": "internal", "purpose": "   "}),
            vec![
                ("governance.purpose", "objects.purpose_invalid"),
                ("governance.purpose", "objects.purpose_required"),
            ],
        ),
        (
            json!({"retention_days": 0}),
            vec![("governance.retention_days", "objects.retention_invalid")],
        ),
        (
            json!({"purpose": "x".repeat(4001)}),
            vec![("governance.purpose", "objects.purpose_invalid")],
        ),
    ] {
        let mut refused = field("diagnosis", FieldType::Text);
        refused.governance = governance(declared.clone());
        let error = admin
            .create_field(PATIENT, refused, context())
            .await
            .expect_err("inconsistent declarations");
        let expected: Vec<_> = expected
            .into_iter()
            .map(|(field, code)| (field.to_owned(), code))
            .collect();
        assert_eq!(codes(&error), expected, "{declared}");
    }
    // Unknown keys and classifications are refused when read.
    assert!(
        serde_json::from_value::<FieldGovernance>(json!({"classification": "public"})).is_err()
    );
    assert!(
        serde_json::from_value::<FieldGovernance>(json!({"data_classification": "secret"}))
            .is_err()
    );

    // --- Declared, stored and described ---------------------------------------
    let mut diagnosis = field("diagnosis", FieldType::Text);
    diagnosis.governance = governance(json!({
        "data_classification": "sensitive",
        "purpose": "  Providing care  ",
        "retention_days": 3650,
        "encrypted": true,
    }));
    let created = admin
        .create_field(PATIENT, diagnosis, context())
        .await
        .expect("consistent declarations");
    let expected = governance(json!({
        "data_classification": "sensitive",
        "purpose": "Providing care",
        "retention_days": 3650,
        "encrypted": true,
    }));
    assert_eq!(created.governance, expected, "the purpose is trimmed");
    assert_eq!(
        described(&admin, PATIENT, "custom:diagnosis")
            .await
            .governance,
        expected
    );
    // A public field needs no purpose, and a declaration alone is enough.
    let mut ward_floor = field("floor", FieldType::Text);
    ward_floor.governance = governance(json!({"data_classification": "public"}));
    admin
        .create_field(PATIENT, ward_floor, context())
        .await
        .expect("a public field");

    // --- A change replaces the declarations, with the same rules -------------
    let version = described(&admin, PATIENT, "custom:diagnosis").await.version;
    let refused = admin
        .update_field(
            PATIENT,
            "custom:diagnosis",
            serde_json::from_value(json!({
                "expected_version": version,
                "governance": {"data_classification": "sensitive", "purpose": "Care"},
            }))
            .expect("change"),
            context(),
        )
        .await
        .expect_err("a sensitive field stays encrypted");
    assert_eq!(
        codes(&refused),
        [(
            "governance.encrypted".to_owned(),
            "objects.encryption_required"
        )]
    );
    let changed = admin
        .update_field(
            PATIENT,
            "custom:diagnosis",
            serde_json::from_value(json!({
                "expected_version": version,
                "governance": {"data_classification": "personal", "purpose": "Care"},
            }))
            .expect("change"),
            context(),
        )
        .await
        .expect("reclassified");
    assert_eq!(
        changed.governance,
        governance(json!({"data_classification": "personal", "purpose": "Care"})),
        "replaced, not merged: retention and encryption are gone"
    );
    // A change that leaves governance out keeps it.
    let relabelled = admin
        .update_field(
            PATIENT,
            "custom:diagnosis",
            serde_json::from_value(json!({
                "expected_version": changed.version,
                "label": "Diagnosis",
            }))
            .expect("change"),
            context(),
        )
        .await
        .expect("relabelled");
    assert_eq!(relabelled.governance, changed.governance);
    // An empty document clears it.
    let cleared = admin
        .update_field(
            PATIENT,
            "custom:diagnosis",
            serde_json::from_value(json!({
                "expected_version": relabelled.version,
                "governance": {},
            }))
            .expect("change"),
            context(),
        )
        .await
        .expect("cleared");
    assert!(cleared.governance.is_empty());
    assert!(described(&admin, PATIENT, "custom:diagnosis")
        .await
        .governance
        .is_empty());

    // --- A plugin's declarations are the plugin's ------------------------------
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, PLUGIN),
    );
    let mut manifest = super::plugin_objects::fixture();
    let declare = |manifest: &mut plugin_contract::Manifest, retention: i32| {
        let summary = manifest
            .objects
            .iter_mut()
            .find(|declared| declared.name == "ticket")
            .and_then(|ticket| {
                ticket
                    .fields
                    .iter_mut()
                    .find(|field| field.name == "summary")
            })
            .expect("summary");
        summary.data_classification = Some(ObjectDataClassification::Internal);
        summary.purpose = Some("Support".into());
        summary.retention_days = Some(retention);
    };
    declare(&mut manifest, 365);
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("install");
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    let summary = described(&admin, TICKET, SUMMARY).await;
    assert_eq!(
        summary.governance,
        governance(
            json!({"data_classification": "internal", "purpose": "Support", "retention_days": 365})
        )
    );
    let refused = admin
        .update_field(
            TICKET,
            SUMMARY,
            serde_json::from_value(json!({
                "expected_version": summary.version,
                "governance": {},
            }))
            .expect("change"),
            context(),
        )
        .await
        .expect_err("administrators cannot change a plugin's declarations");
    assert!(matches!(refused, ObjectError::Conflict(code) if code == PLUGIN_OWNED));
    // An upgrade that changes them changes the field.
    declare(&mut manifest, 30);
    installer
        .install_plugin_objects(&manifest, context())
        .await
        .expect("upgrade");
    assert_eq!(
        described(&admin, TICKET, SUMMARY)
            .await
            .governance
            .retention_days,
        Some(30)
    );
}
