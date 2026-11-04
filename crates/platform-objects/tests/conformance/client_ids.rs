//! Record ids assigned by devices on every backend (ADR 0031): a device
//! creates records offline with ids from the device range, and the server
//! keeps them when it pushes, so references between offline records hold.

use super::{body, caller, codes, context, object, service};
use platform_objects::composite::{CompositeOutcome, CompositeRequest};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &[OBJECTS_MANAGE_PERMISSION], &[]),
    );
    let role = Uuid::now_v7();
    let ana = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));
    let ben = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));
    let account = admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut lookup = super::field("parent", FieldType::Lookup);
    lookup.reference_object = Some("custom:account".into());
    admin
        .create_field("custom:account", lookup, context())
        .await
        .expect("parent");
    admin
        .set_object_permission(
            "custom:account",
            role,
            AccessFlags {
                read: true,
                create: true,
                edit: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("access");

    // The device's engine assigns ids from the device range.
    let device = ana.clone().with_client_ids();
    let offline = device
        .create_record(
            "custom:account",
            &body(json!({"name": "Made offline"})),
            context(),
        )
        .await
        .expect("offline create");
    let id = platform_id::PublicId::parse(&offline.public_id).expect("valid id");
    assert!(id.is_client_assigned());
    assert_eq!(id.prefix(), account.key_prefix);
    let online = ana
        .create_record(
            "custom:account",
            &body(json!({"name": "Made online"})),
            context(),
        )
        .await
        .expect("server create");
    assert!(!platform_id::PublicId::parse(&online.public_id)
        .expect("valid id")
        .is_client_assigned());

    // The server keeps an assigned id, and a retried create returns the
    // same record rather than a second one.
    let pushed_id = platform_id::PublicId::new_client(&account.key_prefix, 42)
        .expect("id")
        .as_str()
        .to_owned();
    let pushed = ana
        .create_record(
            "custom:account",
            &body(json!({"id": pushed_id, "name": "Pushed"})),
            context(),
        )
        .await
        .expect("create with an assigned id");
    assert_eq!(pushed.public_id, pushed_id);
    let retried = ana
        .create_record(
            "custom:account",
            &body(json!({"id": pushed_id, "name": "Pushed again"})),
            context(),
        )
        .await
        .expect("a retried create");
    assert_eq!(retried.id, pushed.id);
    assert_eq!(retried.name, "Pushed");
    // A create and its retry that overlap still make one record.
    for round in 0..20u64 {
        let raced_id = platform_id::PublicId::new_client(&account.key_prefix, 1_000 + round)
            .expect("id")
            .as_str()
            .to_owned();
        let raced = body(json!({"id": raced_id, "name": "Raced"}));
        let sends: Vec<_> = (0..2)
            .map(|_| {
                let ana = ana.clone();
                let raced = raced.clone();
                tokio::spawn(
                    async move { ana.create_record("custom:account", &raced, context()).await },
                )
            })
            .collect();
        let mut ids = Vec::new();
        for send in sends {
            ids.push(send.await.expect("join").expect("an overlapping create").id);
        }
        assert_eq!(ids[0], ids[1]);
    }
    let error = ben
        .create_record(
            "custom:account",
            &body(json!({"id": pushed_id, "name": "Someone else"})),
            context(),
        )
        .await
        .expect_err("another member's id");
    assert!(matches!(error, ObjectError::Conflict("records.id_taken")));

    // Only ids from the device range, for this object, are accepted.
    let sequence_id = platform_id::PublicId::new(&account.key_prefix, 9_999)
        .expect("id")
        .as_str()
        .to_owned();
    let other_prefix = platform_id::PublicId::new_client("zzz", 1)
        .expect("id")
        .as_str()
        .to_owned();
    for bad in [
        json!(sequence_id),
        json!(other_prefix),
        json!("not-an-id"),
        json!(7),
    ] {
        let error = ana
            .create_record(
                "custom:account",
                &body(json!({"id": bad, "name": "Bad"})),
                context(),
            )
            .await
            .expect_err("bad id");
        assert_eq!(codes(&error), vec!["records.id_invalid"], "{bad}");
    }

    // Offline records that refer to each other push in one composite
    // request with their own ids, so no reference needs rewriting.
    let parent_id = platform_id::PublicId::new_client(&account.key_prefix, 100)
        .expect("id")
        .as_str()
        .to_owned();
    let child_id = platform_id::PublicId::new_client(&account.key_prefix, 101)
        .expect("id")
        .as_str()
        .to_owned();
    let request: CompositeRequest = serde_json::from_value(json!({
        "all_or_none": true,
        "operations": [
            {"method": "create", "reference": "parent", "object": "custom:account",
             "values": {"id": parent_id, "name": "Parent"}},
            {"method": "create", "reference": "child", "object": "custom:account",
             "values": {"id": child_id, "name": "Child", "custom:parent": parent_id}},
        ],
    }))
    .expect("request");
    let outcomes = ana.composite(&request, context()).await.expect("composite");
    let records: Vec<Record> = outcomes
        .into_iter()
        .map(|(_, outcome)| match outcome {
            CompositeOutcome::Done { record, .. } => *record,
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(records[0].public_id, parent_id);
    assert_eq!(records[1].public_id, child_id);
    assert_eq!(records[1].values["custom:parent"], json!(parent_id));
}
