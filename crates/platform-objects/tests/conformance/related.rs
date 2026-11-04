//! Related lists and lookup filters on every backend (ADR 0028): related
//! records and lookup candidates are read as the caller, and a lookup's new
//! values must meet its filter.

use super::{body, caller, codes, context, field, object, service};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn names(result: &QueryResult) -> Vec<String> {
    result
        .rows
        .iter()
        .filter_map(|row| row.get("name").and_then(|name| name.as_str()))
        .map(str::to_owned)
        .collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(
            tenant,
            Uuid::now_v7(),
            &[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION],
            &[],
        ),
    );
    let role = Uuid::now_v7();
    let ana = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));
    let ben = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));

    admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut active = field("active", FieldType::Boolean);
    active.indexed = true;
    admin
        .create_field("custom:account", active, context())
        .await
        .expect("active");
    let mut contact = object("contact", "Contact");
    contact.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(contact, context())
        .await
        .expect("contact");
    for api in ["custom:account", "custom:contact"] {
        admin
            .set_object_permission(
                api,
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
    }

    // A filter is one condition on the target that runs for whoever sets it.
    let lookup = |filter: &str| {
        let mut account = field("account", FieldType::Lookup);
        account.reference_object = Some("custom:account".into());
        account.lookup_filter = Some(filter.into());
        account
    };
    let broken = admin
        .create_field("custom:contact", lookup("custom:nope = true"), context())
        .await
        .expect_err("unknown field");
    match &broken {
        ObjectError::Validation(issues) => assert_eq!(issues[0].field, "lookup_filter"),
        other => panic!("{other:?}"),
    }
    let ordered = admin
        .create_field(
            "custom:contact",
            lookup("custom:active = true ORDER BY name"),
            context(),
        )
        .await
        .expect_err("not a condition");
    assert_eq!(codes(&ordered), ["objects.lookup_filter_invalid"]);
    // The engine adds the record and the picker's search to a filter, so
    // a filter must leave room for them under the query's condition limit.
    let crowded = vec!["name = 'x'"; 49].join(" OR ");
    let full = admin
        .create_field("custom:contact", lookup(&crowded), context())
        .await
        .expect_err("no room for the record");
    assert_eq!(codes(&full), ["query.too_complex"]);
    let mut text = field("nickname", FieldType::Text);
    text.lookup_filter = Some("name = 'x'".into());
    assert_eq!(
        codes(
            &admin
                .create_field("custom:contact", text, context())
                .await
                .expect_err("text")
        ),
        ["objects.lookup_filter_invalid"]
    );
    let created = admin
        .create_field(
            "custom:contact",
            lookup("  custom:active = true  "),
            context(),
        )
        .await
        .expect("filtered lookup");
    assert_eq!(
        created.lookup_filter.as_deref(),
        Some("custom:active = true")
    );

    let open = admin
        .create_record(
            "custom:account",
            &body(json!({"name": "Open Co", "custom:active": true})),
            context(),
        )
        .await
        .expect("open");
    let closed = admin
        .create_record(
            "custom:account",
            &body(json!({"name": "Closed Co", "custom:active": false})),
            context(),
        )
        .await
        .expect("closed");

    // New values must meet the filter.
    let refused = ana
        .create_record(
            "custom:contact",
            &body(json!({"name": "Ana's contact", "custom:account": closed.public_id})),
            context(),
        )
        .await
        .expect_err("closed account");
    assert_eq!(codes(&refused), ["records.lookup_filter_mismatch"]);
    let mine = ana
        .create_record(
            "custom:contact",
            &body(json!({"name": "Ana's contact", "custom:account": open.public_id})),
            context(),
        )
        .await
        .expect("open account");
    ben.create_record(
        "custom:contact",
        &body(json!({"name": "Ben's contact", "custom:account": open.public_id})),
        context(),
    )
    .await
    .expect("ben's contact");

    // A value set before the target stopped matching is kept on edits.
    let closing = admin
        .update_record(
            "custom:account",
            &open.public_id,
            open.version,
            &body(json!({"custom:active": false})),
            context(),
        )
        .await
        .expect("close");
    let renamed = ana
        .update_record(
            "custom:contact",
            &mine.public_id,
            mine.version,
            &body(json!({"name": "Renamed", "custom:account": open.public_id})),
            context(),
        )
        .await
        .expect("unchanged lookup is kept");
    admin
        .update_record(
            "custom:account",
            &open.public_id,
            closing.version,
            &body(json!({"custom:active": true})),
            context(),
        )
        .await
        .expect("reopen");

    // The picker offers the records within the filter, by name.
    let offered = ana
        .lookup_candidates("custom:contact", "custom:account", None, None)
        .await
        .expect("candidates");
    assert_eq!(names(&offered), ["Open Co"]);
    let searched = ana
        .lookup_candidates("custom:contact", "custom:account", Some("zzz"), None)
        .await
        .expect("search");
    assert!(searched.rows.is_empty());
    let unfiltered = admin
        .update_field(
            "custom:contact",
            "custom:account",
            serde_json::from_value(json!({
                "expected_version": created.version,
                "lookup_filter": null,
            }))
            .expect("update"),
            context(),
        )
        .await
        .expect("remove the filter");
    assert_eq!(unfiltered.lookup_filter, None);
    let all = ana
        .lookup_candidates("custom:contact", "custom:account", Some("co"), None)
        .await
        .expect("all");
    assert_eq!(names(&all), ["Closed Co", "Open Co"]);
    assert!(matches!(
        ana.lookup_candidates("custom:contact", "name", None, None)
            .await,
        Err(ObjectError::NotFound)
    ));

    // Related lists show each viewer the records they may see.
    let related = |who: &ObjectService| {
        let who = who.clone();
        let id = open.public_id.clone();
        async move { who.related_lists("custom:account", &id).await }
    };
    let for_admin = related(&admin).await.expect("admin's lists");
    assert_eq!(for_admin.len(), 1);
    assert_eq!(
        (
            for_admin[0].relationship.object.as_str(),
            for_admin[0].relationship.field.as_str(),
            for_admin[0].relationship.master_detail
        ),
        ("custom:contact", "custom:account", false)
    );
    assert_eq!(names(&for_admin[0].records), ["Ben's contact", "Renamed"]);
    let for_ana = related(&ana).await.expect("ana's lists");
    assert_eq!(names(&for_ana[0].records), ["Renamed"]);
    let page = ben
        .related_records(
            "custom:account",
            &open.public_id,
            "custom:contact",
            "custom:account",
            None,
        )
        .await
        .expect("ben's page");
    assert_eq!(names(&page.records), ["Ben's contact"]);
    assert!(matches!(
        ben.related_records(
            "custom:account",
            &open.public_id,
            "custom:contact",
            "name",
            None
        )
        .await,
        Err(ObjectError::NotFound)
    ));
    let _ = renamed;
}
