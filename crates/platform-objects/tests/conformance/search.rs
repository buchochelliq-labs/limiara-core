//! Global search on every backend (ADR 0020): terms from names and
//! searchable fields, prefix matching, ranking, grouping, sharing, the
//! recycle bin and reindexing when search settings change.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn request(q: &str) -> SearchRequest {
    SearchRequest {
        q: q.into(),
        ..SearchRequest::default()
    }
}

/// `object: name, name` per group, in result order.
async fn found(service: &ObjectService, q: &str) -> Vec<String> {
    service
        .search(request(q))
        .await
        .unwrap_or_else(|error| panic!("{q}: {error:?}"))
        .into_iter()
        .map(|group| {
            format!(
                "{}: {}",
                group.object,
                group
                    .records
                    .iter()
                    .map(|record| record.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect()
}

async fn code(service: &ObjectService, request: SearchRequest) -> &'static str {
    match service.search(request).await {
        Err(ObjectError::Validation(issues)) => issues[0].code,
        other => panic!("expected a validation error, got {other:?}"),
    }
}

async fn create(service: &ObjectService, object: &str, value: Value) -> Record {
    service
        .create_record(object, &body(value), context())
        .await
        .expect("record")
}

fn change<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("change")
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let sales = Uuid::now_v7();
    let (alice, carol) = (Uuid::now_v7(), Uuid::now_v7());

    let mut company = object("company", "Company");
    company.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(company, context())
        .await
        .expect("company");
    for (name, kind, searchable) in [
        ("email", FieldType::Email, true),
        ("phone", FieldType::Phone, true),
        ("notes", FieldType::Text, false),
    ] {
        let mut definition = field(name, kind);
        definition.searchable = searchable;
        admin
            .create_field("custom:company", definition, context())
            .await
            .expect("company field");
    }
    admin
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal");
    let mut memo = object("memo", "Memo");
    memo.searchable = Some(false);
    admin.create_object(memo, context()).await.expect("memo");
    for api_name in ["custom:company", "custom:deal", "custom:memo"] {
        admin
            .set_object_permission(
                api_name,
                sales,
                AccessFlags {
                    create: true,
                    delete: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("sales access");
    }
    let alice_s = service(&store, caller(tenant, alice, &[], &[sales]));
    let carol_s = service(&store, caller(tenant, carol, &[], &[sales]));
    let outsider = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));

    let acme = create(
        &alice_s,
        "custom:company",
        json!({"name": "Acme Industries", "custom:email": "Sales@Acme.example",
               "custom:phone": "+44 7946 0958", "custom:notes": "hidden words"}),
    )
    .await;
    create(
        &alice_s,
        "custom:company",
        json!({"name": "Globex", "custom:email": "acme-watch@globex.example"}),
    )
    .await;
    create(&carol_s, "custom:company", json!({"name": "Acme Secret"})).await;
    create(&alice_s, "custom:deal", json!({"name": "Acme renewal"})).await;
    create(&alice_s, "custom:memo", json!({"name": "Acme memo"})).await;

    // Sharing: Carol's company is hidden from Alice. Names outrank fields,
    // and groups follow their best result.
    assert_eq!(
        found(&alice_s, "acme").await,
        [
            "custom:company: Acme Industries, Globex",
            "custom:deal: Acme renewal"
        ]
    );
    assert_eq!(
        found(&admin, "acme").await,
        [
            "custom:company: Acme Industries, Acme Secret, Globex",
            "custom:deal: Acme renewal"
        ]
    );
    assert!(
        found(&outsider, "acme").await.is_empty(),
        "no object access, no results"
    );

    // Every term matches as a prefix; fields and phone digits are found.
    for (q, expected) in [
        ("acm ind", vec!["custom:company: Acme Industries"]),
        ("INDUSTRIES acme", vec!["custom:company: Acme Industries"]),
        ("sales@acme", vec!["custom:company: Acme Industries"]),
        ("447946", vec!["custom:company: Acme Industries"]),
        ("acme zzz", vec![]),
        ("hidden", vec![]),
    ] {
        assert_eq!(found(&alice_s, q).await, expected, "{q}");
    }
    let groups = alice_s.search(request("industries")).await.expect("fields");
    assert_eq!(
        Value::Object(groups[0].records[0].fields.clone()),
        json!({"custom:email": "Sales@Acme.example", "custom:phone": "+44 7946 0958"}),
        "results show searchable fields only"
    );
    assert_eq!(groups[0].records[0].public_id, acme.public_id);
    assert_eq!(groups[0].label, "Company");

    // Search settings: the object flag applies at once, a field flag
    // reindexes the object's records.
    admin
        .update_object(
            "custom:memo",
            change(json!({"expected_version": 1, "searchable": true})),
            context(),
        )
        .await
        .expect("memo searchable");
    assert_eq!(found(&alice_s, "memo").await, ["custom:memo: Acme memo"]);
    admin
        .update_field(
            "custom:company",
            "custom:notes",
            change(json!({"expected_version": 1, "searchable": true})),
            context(),
        )
        .await
        .expect("notes searchable");
    assert_eq!(
        found(&alice_s, "hidden").await,
        ["custom:company: Acme Industries"]
    );
    admin
        .delete_field("custom:company", "custom:notes", 2, context())
        .await
        .expect("delete notes");
    assert!(
        found(&alice_s, "hidden").await.is_empty(),
        "a deleted field stops matching"
    );

    // Edits reindex; the recycle bin hides, undelete restores.
    let acme = alice_s
        .update_record(
            "custom:company",
            &acme.public_id,
            acme.version,
            &body(json!({"name": "Acme Holdings"})),
            context(),
        )
        .await
        .expect("rename");
    assert!(found(&alice_s, "industries").await.is_empty());
    assert_eq!(
        found(&alice_s, "holdings").await,
        ["custom:company: Acme Holdings"]
    );
    let deleted = alice_s
        .delete_record("custom:company", &acme.public_id, acme.version, context())
        .await
        .expect("delete");
    assert!(found(&alice_s, "holdings").await.is_empty());
    alice_s
        .undelete_record(
            "custom:company",
            &deleted.public_id,
            deleted.version,
            context(),
        )
        .await
        .expect("undelete");
    assert_eq!(
        found(&alice_s, "holdings").await,
        ["custom:company: Acme Holdings"]
    );

    // Limits per object, one object for a results page.
    for index in 1..=3 {
        create(
            &alice_s,
            "custom:deal",
            json!({"name": format!("Zeta {index}")}),
        )
        .await;
    }
    let limited = alice_s
        .search(SearchRequest {
            q: "zeta".into(),
            limit: Some(2),
            ..SearchRequest::default()
        })
        .await
        .expect("limited");
    assert_eq!(limited[0].records.len(), 2);
    assert!(limited[0].has_more);
    let page = alice_s
        .search(SearchRequest {
            q: "acme".into(),
            object: Some("custom:deal".into()),
            limit: Some(200),
        })
        .await
        .expect("one object");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].object, "custom:deal");
    assert!(!page[0].has_more);

    assert_eq!(code(&alice_s, request("a")).await, "search.too_short");
    assert_eq!(
        code(
            &outsider,
            SearchRequest {
                q: "acme".into(),
                object: Some("custom:company".into()),
                limit: None,
            }
        )
        .await,
        "search.object_unknown"
    );
    assert_eq!(
        code(
            &alice_s,
            SearchRequest {
                q: "acme".into(),
                limit: Some(51),
                ..SearchRequest::default()
            }
        )
        .await,
        "search.limit_invalid"
    );
}
