//! A synced device's definitions and revocations (ADR 0024 §3–4), on
//! SQLite, which devices use. The replica takes the objects, fields and
//! validation rules the server describes, with the server's key prefixes,
//! and forgets records the member can no longer read, without logging
//! either. Here the server is another workspace in the same store.

use super::push::{flush, remote};
use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::json;
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

/// Everything the server describes to the member, as its API renders it.
async fn described(server: &ObjectService) -> Vec<RemoteObject> {
    let mut objects = Vec::new();
    for (listed, _) in server.list_objects().await.expect("objects") {
        let (object, _) = server.describe(&listed.api_name).await.expect("describe");
        let rules = server
            .validation_rules(&object.api_name)
            .await
            .expect("rules");
        objects.push(RemoteObject::from_definition(
            &object,
            MEMBER_ACCESS,
            Some(&rules),
        ));
    }
    objects
}

async fn log_head(device: &ObjectService) -> i64 {
    device
        .list_changes(None, Some(200), None)
        .await
        .expect("changes")
        .head
}

fn conflict(error: ObjectError) -> &'static str {
    match error {
        ObjectError::Conflict(code) => code,
        other => panic!("expected a conflict: {other:?}"),
    }
}

pub async fn run(store: Arc<dyn ObjectStore>) {
    let member = Uuid::now_v7();
    let server = service(
        &store,
        caller(Uuid::now_v7(), member, &ADMIN_PERMISSIONS, &[]),
    );
    // The device's member has no system permissions: what they may do
    // comes from the access the server describes.
    let device = service(
        &store,
        caller(Uuid::now_v7(), member, &[], &[Uuid::now_v7()]),
    )
    .with_replica()
    .with_client_ids();

    let account = server
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut tier = field("tier", FieldType::Text);
    tier.indexed = true;
    server
        .create_field("custom:account", tier, context())
        .await
        .expect("tier");
    server
        .create_field("custom:account", field("notes", FieldType::Text), context())
        .await
        .expect("notes");
    let contact = server
        .create_object(object("contact", "Contact"), context())
        .await
        .expect("contact");
    let mut employer = field("employer", FieldType::Lookup);
    employer.reference_object = Some("custom:account".into());
    server
        .create_field("custom:contact", employer, context())
        .await
        .expect("employer");
    let mut shout = field("shout", FieldType::Formula);
    shout.formula = Some("UPPER(name)".into());
    shout.formula_type = Some(FieldType::Text);
    server
        .create_field("custom:contact", shout, context())
        .await
        .expect("formula");
    server
        .create_validation_rule(
            "custom:account",
            CreateValidationRule {
                name: "tier_known".into(),
                description: None,
                condition: "custom:tier = \"unknown\"".into(),
                message: "Choose a tier".into(),
                error_field: Some("custom:tier".into()),
                active: Some(true),
            },
            context(),
        )
        .await
        .expect("rule");

    // Only a replica's engine takes the server's definitions.
    assert!(matches!(
        server.apply_remote_definitions(&[]).await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        server.forget_remote("custom:account", &[]).await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(matches!(
        server.replica_manifest().await,
        Err(ObjectError::PermissionDenied)
    ));

    let head = log_head(&device).await;
    let applied = device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("first definitions");
    assert_eq!(applied.objects, vec!["custom:account", "custom:contact"]);
    assert_eq!(applied.rules, vec!["custom:account.tier_known"]);
    assert!(applied
        .fields
        .contains(&"custom:contact.custom:employer".to_owned()));
    assert_eq!(
        log_head(&device).await,
        head,
        "definitions leave no log entry"
    );

    // The replica keeps the server's API names and key prefixes, and grants
    // the member the access the server described.
    let (local, access) = device.describe("custom:account").await.expect("describe");
    assert_eq!(local.key_prefix, account.key_prefix);
    assert!(access.read && access.create && access.edit && access.view_all);
    assert!(!access.modify_all);
    let (local_contact, _) = device.describe("custom:contact").await.expect("contact");
    assert_eq!(local_contact.key_prefix, contact.key_prefix);
    let employer = local_contact
        .field("custom:employer")
        .expect("lookup field");
    assert_eq!(employer.reference_object.as_deref(), Some("custom:account"));
    assert!(local
        .field("custom:tier")
        .is_some_and(FieldDefinition::is_indexed));

    // Taking the same definitions again changes nothing.
    assert!(device
        .apply_remote_definitions(&described(&server).await)
        .await
        .expect("again")
        .is_empty());

    // A record made on the device carries the server's prefix, so it pushes
    // with its id; the server's validation rule already applies offline.
    let refused = device
        .create_record(
            "custom:account",
            &body(json!({"name": "Bad", "custom:tier": "unknown"})),
            context(),
        )
        .await
        .expect_err("the rule applies on the device");
    assert!(matches!(refused, ObjectError::Validation(_)));
    let acme = device
        .create_record(
            "custom:account",
            &body(json!({"name": "Acme", "custom:tier": "gold"})),
            context(),
        )
        .await
        .expect("offline account");
    assert!(acme.public_id.starts_with(&account.key_prefix));
    let ada = device
        .create_record(
            "custom:contact",
            &body(json!({"name": "Ada", "custom:employer": acme.public_id})),
            context(),
        )
        .await
        .expect("offline contact");
    assert_eq!(
        device
            .get_record("custom:contact", &ada.public_id, false)
            .await
            .expect("formula on read")
            .values
            .get("custom:shout"),
        Some(&json!("ADA"))
    );
    let batch = device.push_queue().await.expect("queue");
    let report = flush(&device, &server, &batch).await;
    assert!(report.failed.is_empty(), "{report:?}");
    let next = device.push_queue().await.expect("next");
    if !next.is_empty() {
        let report = flush(&device, &server, &next).await;
        assert!(report.failed.is_empty(), "{report:?}");
    }
    assert!(device.push_queue().await.expect("drained").is_empty());
    assert_eq!(
        server
            .get_record("custom:contact", &ada.public_id, false)
            .await
            .expect("on the server")
            .values
            .get("custom:employer"),
        Some(&json!(acme.public_id))
    );

    // The server changes a field, retires another, drops its rule and
    // stops describing an object: the replica follows.
    let notes = server
        .describe("custom:account")
        .await
        .expect("account")
        .0
        .field("custom:notes")
        .cloned()
        .expect("notes");
    server
        .delete_field("custom:account", "custom:notes", notes.version, context())
        .await
        .expect("retire notes");
    let tier = server
        .describe("custom:account")
        .await
        .expect("account")
        .0
        .field("custom:tier")
        .cloned()
        .expect("tier");
    server
        .update_field(
            "custom:account",
            "custom:tier",
            UpdateField {
                expected_version: tier.version,
                label: Some("Service tier".into()),
                description: None,
                required: None,
                indexed: None,
                searchable: None,
                track_history: None,
                max_length: None,
                options: None,
                default_value: None,
                position: None,
                formula: None,
                lookup_filter: None,
            },
            context(),
        )
        .await
        .expect("relabel tier");
    let rule = server
        .validation_rules("custom:account")
        .await
        .expect("rules")
        .remove(0);
    server
        .delete_validation_rule("custom:account", "tier_known", rule.version, context())
        .await
        .expect("drop rule");
    let mut objects = described(&server).await;
    objects.retain(|object| object.api_name == "custom:account");
    let applied = device
        .apply_remote_definitions(&objects)
        .await
        .expect("changed definitions");
    assert_eq!(applied.objects, vec!["custom:contact"]);
    assert_eq!(
        applied.fields,
        vec!["custom:account.custom:tier", "custom:account.custom:notes"]
    );
    assert_eq!(applied.rules, vec!["custom:account.tier_known"]);
    let (local, _) = device.describe("custom:account").await.expect("account");
    assert_eq!(
        local.field("custom:tier").map(|field| field.label.as_str()),
        Some("Service tier")
    );
    assert!(local.field("custom:notes").is_none());
    assert!(matches!(
        device.describe("custom:contact").await,
        Err(ObjectError::NotFound)
    ));
    // Its records stay in the revocation manifest until the server's
    // answer erases them.
    let manifest = device.replica_manifest().await.expect("manifest");
    assert_eq!(
        manifest.get("custom:contact"),
        Some(&vec![ada.public_id.clone()]),
        "records of a retired object are still asked about"
    );
    assert_eq!(
        device
            .forget_remote("custom:contact", std::slice::from_ref(&ada.public_id))
            .await
            .expect("forget a retired object's record"),
        1
    );
    assert!(!device
        .replica_manifest()
        .await
        .expect("manifest")
        .contains_key("custom:contact"));
    device
        .update_record(
            "custom:account",
            &acme.public_id,
            device
                .get_record("custom:account", &acme.public_id, false)
                .await
                .expect("acme")
                .version,
            &body(json!({"custom:tier": "unknown"})),
            context(),
        )
        .await
        .expect("the dropped rule no longer applies");

    // A key prefix that differs from the replica's means the replica no
    // longer matches the server.
    let mut diverged = objects.clone();
    diverged[0].key_prefix = "a9Z".into();
    assert_eq!(
        conflict(
            device
                .apply_remote_definitions(&diverged)
                .await
                .expect_err("diverged")
        ),
        "sync.definitions_diverged"
    );

    // A lookup to an object the member cannot read is left out, and so is a
    // rule that reaches through it: the replica could not check it and it
    // would refuse every save, so the server checks it on push. Rules on
    // the object's own fields still apply offline.
    for (name, condition) in [
        ("employer_open", "custom:employer.custom:tier = \"closed\""),
        ("named", "name = \"Nobody\""),
    ] {
        server
            .create_validation_rule(
                "custom:contact",
                CreateValidationRule {
                    name: name.into(),
                    description: None,
                    condition: condition.into(),
                    message: format!("{name} refused"),
                    error_field: None,
                    active: Some(true),
                },
                context(),
            )
            .await
            .expect("contact rule");
    }
    let mut hidden = described(&server).await;
    hidden.retain(|object| object.api_name == "custom:contact");
    hidden.extend(objects.clone());
    hidden.retain(|object| object.api_name != "custom:account");
    let device_two = service(
        &store,
        caller(Uuid::now_v7(), member, &[], &[Uuid::now_v7()]),
    )
    .with_replica();
    let applied = device_two
        .apply_remote_definitions(&hidden)
        .await
        .expect("partial definitions");
    assert_eq!(applied.deferred_rules, vec!["custom:contact.employer_open"]);
    let (partial, _) = device_two
        .describe("custom:contact")
        .await
        .expect("contact");
    assert!(partial.field("custom:employer").is_none());
    assert!(partial.field("custom:shout").is_some());
    let rules: Vec<String> = device_two
        .validation_rules("custom:contact")
        .await
        .expect("replica rules")
        .into_iter()
        .map(|rule| rule.name)
        .collect();
    assert_eq!(rules, vec!["named"]);
    let saved = device_two
        .create_record("custom:contact", &body(json!({"name": "Grace"})), context())
        .await;
    assert!(
        saved.is_ok(),
        "a deferred rule does not block saves: {saved:?}"
    );
    let refused = device_two
        .create_record(
            "custom:contact",
            &body(json!({"name": "Nobody"})),
            context(),
        )
        .await
        .expect_err("the contact's own rule applies offline");
    assert!(matches!(refused, ObjectError::Validation(_)));

    forget(&server, &device).await;
}

/// Revocations: the replica lists only the records the server sent it, and
/// forgets the ones the member can no longer read, unpushed edits and all.
async fn forget(server: &ObjectService, device: &ObjectService) {
    let kept = server
        .create_record("custom:account", &body(json!({"name": "Kept"})), context())
        .await
        .expect("kept");
    let lost = server
        .create_record(
            "custom:account",
            &body(json!({"name": "Lost", "custom:tier": "gold"})),
            context(),
        )
        .await
        .expect("lost");
    for record in [&kept, &lost] {
        device
            .apply_remote("custom:account", &remote(record), None)
            .await
            .expect("pulled");
    }
    let offline = device
        .create_record(
            "custom:account",
            &body(json!({"name": "Offline"})),
            context(),
        )
        .await
        .expect("offline");
    device
        .update_record(
            "custom:account",
            &lost.public_id,
            lost.version,
            &body(json!({"custom:tier": "silver"})),
            context(),
        )
        .await
        .expect("unpushed edit");

    let manifest = device.replica_manifest().await.expect("manifest");
    let accounts = manifest.get("custom:account").expect("accounts");
    assert!(accounts.contains(&kept.public_id) && accounts.contains(&lost.public_id));
    assert!(
        !accounts.contains(&offline.public_id),
        "records the server does not know are not asked about"
    );

    let head = log_head(device).await;
    assert_eq!(
        device
            .forget_remote(
                "custom:account",
                &[lost.public_id.clone(), "a0Bnot-held".into()],
            )
            .await
            .expect("forget"),
        1
    );
    assert_eq!(log_head(device).await, head, "forgetting is not logged");
    assert!(matches!(
        device
            .get_record("custom:account", &lost.public_id, true)
            .await,
        Err(ObjectError::NotFound)
    ));
    assert!(device
        .remote_version("custom:account", &lost.public_id)
        .await
        .expect("version")
        .is_none());
    let batch = device.push_queue().await.expect("queue");
    assert!(
        batch
            .operations
            .iter()
            .all(|pushed| pushed.public_id != lost.public_id),
        "a forgotten record's edits do not push"
    );
    let filtered = device
        .list_records(
            "custom:account",
            platform_objects::service::ListRecords {
                filters: vec![("custom:tier".into(), "silver".into())],
                ..Default::default()
            },
        )
        .await
        .expect("filter");
    assert!(filtered.records.is_empty(), "its index rows are gone");
    assert!(device
        .get_record("custom:account", &kept.public_id, false)
        .await
        .is_ok());
    assert_eq!(
        device
            .forget_remote("custom:unknown", std::slice::from_ref(&kept.public_id))
            .await
            .expect("unknown object"),
        0
    );
}
