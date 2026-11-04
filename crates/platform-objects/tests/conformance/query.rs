//! The query language on every backend (ADR 0019): typed filters, ranges,
//! relationship hops, sharing at every hop, paging, aggregates and the
//! recycle bin.

use super::{body, caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use chrono::{Duration, Utc};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn request(q: &str) -> QueryRequest {
    QueryRequest {
        q: q.into(),
        ..QueryRequest::default()
    }
}

async fn names(service: &ObjectService, q: &str) -> Vec<String> {
    let result = service
        .query(request(q))
        .await
        .unwrap_or_else(|error| panic!("{q}: {error:?}"));
    result
        .rows
        .iter()
        .map(|row| row["name"].as_str().unwrap_or_default().to_owned())
        .collect()
}

async fn code(service: &ObjectService, q: &str) -> &'static str {
    match service.query(request(q)).await {
        Err(ObjectError::Validation(issues)) => issues[0].code,
        other => panic!("{q}: expected a validation error, got {other:?}"),
    }
}

async fn create(service: &ObjectService, object: &str, value: Value) -> Record {
    service
        .create_record(object, &body(value), context())
        .await
        .expect("record")
}

const BASE: &str = "SELECT name FROM custom:opportunity";

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let sales = Uuid::now_v7();
    let (alice, bob, carol) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());

    let mut company = object("company", "Company");
    company.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(company, context())
        .await
        .expect("company");
    let mut tier = field("tier", FieldType::Picklist);
    tier.options = vec![option("Gold", false), option("Silver", false)];
    admin
        .create_field("custom:company", tier, context())
        .await
        .expect("tier");

    let mut opportunity = object("opportunity", "Opportunity");
    opportunity.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(opportunity, context())
        .await
        .expect("opportunity");
    let mut stage = field("stage", FieldType::Picklist);
    stage.options = vec![
        option("open", false),
        option("won", false),
        option("lost", false),
    ];
    let mut tags = field("tags", FieldType::MultiPicklist);
    tags.options = vec![option("a", false), option("b", false), option("c", false)];
    let indexed = |name: &str, kind: FieldType| {
        let mut definition = field(name, kind);
        definition.indexed = true;
        definition
    };
    let mut company_link = field("company", FieldType::Lookup);
    company_link.reference_object = Some("custom:company".into());
    for definition in [
        stage,
        tags,
        indexed("amount", FieldType::Currency),
        indexed("close", FieldType::Date),
        indexed("email", FieldType::Email),
        indexed("hot", FieldType::Boolean),
        field("notes", FieldType::LongText),
        company_link,
    ] {
        admin
            .create_field("custom:opportunity", definition, context())
            .await
            .expect("opportunity field");
    }
    for api_name in ["custom:company", "custom:opportunity"] {
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
    let member = |user: Uuid, reports: Vec<Uuid>| {
        service(
            &store,
            caller(tenant, user, &[], &[sales]).with_reports(reports),
        )
    };
    let alice_s = member(alice, vec![bob]);
    let bob_s = member(bob, Vec::new());
    let carol_s = member(carol, Vec::new());
    let outsider = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));

    let acme = create(
        &alice_s,
        "custom:company",
        json!({"name": "Acme", "custom:tier": "Gold"}),
    )
    .await;
    let globex = create(
        &alice_s,
        "custom:company",
        json!({"name": "Globex", "custom:tier": "Silver"}),
    )
    .await;
    let initech = create(
        &carol_s,
        "custom:company",
        json!({"name": "Initech", "custom:tier": "Gold"}),
    )
    .await;

    // Bob may point his deal at Acme.
    alice_s
        .share_record(
            "custom:company",
            &acme.public_id,
            GranteeKind::User,
            bob,
            ShareAccess::Read,
            context(),
        )
        .await
        .expect("share acme");

    let today = Utc::now().date_naive();
    let day = |offset: i64| {
        (today + Duration::days(offset))
            .format("%Y-%m-%d")
            .to_string()
    };
    create(
        &alice_s,
        "custom:opportunity",
        json!({"name": "Alpha", "custom:stage": "open", "custom:amount": 100, "custom:close": day(-40),
               "custom:hot": true, "custom:tags": ["a", "b"], "custom:company": acme.public_id,
               "custom:email": "Sales@Acme.example", "custom:notes": "long"}),
    )
    .await;
    create(
        &alice_s,
        "custom:opportunity",
        json!({"name": "beta", "custom:stage": "won", "custom:amount": 250.5, "custom:close": day(0),
               "custom:hot": false, "custom:tags": ["b"], "custom:company": globex.public_id}),
    )
    .await;
    let gamma = create(
        &alice_s,
        "custom:opportunity",
        json!({"name": "Gamma", "custom:stage": "lost"}),
    )
    .await;
    create(
        &bob_s,
        "custom:opportunity",
        json!({"name": "Delta", "custom:stage": "won", "custom:amount": 1000, "custom:company": acme.public_id}),
    )
    .await;
    create(
        &carol_s,
        "custom:opportunity",
        json!({"name": "Epsilon", "custom:stage": "won", "custom:amount": 5000, "custom:company": initech.public_id}),
    )
    .await;
    let eta = create(
        &carol_s,
        "custom:opportunity",
        json!({"name": "Eta", "custom:stage": "open", "custom:amount": 10, "custom:company": initech.public_id}),
    )
    .await;
    carol_s
        .share_record(
            "custom:opportunity",
            &eta.public_id,
            GranteeKind::User,
            alice,
            ShareAccess::Read,
            context(),
        )
        .await
        .expect("share eta");
    create(
        &alice_s,
        "custom:opportunity",
        json!({"name": "Promo 50% off"}),
    )
    .await;

    // Sharing applies to every query: Epsilon is Carol's and unshared.
    assert_eq!(
        names(&alice_s, &format!("{BASE} ORDER BY name")).await,
        ["Alpha", "beta", "Delta", "Eta", "Gamma", "Promo 50% off"]
    );
    assert_eq!(
        names(&admin, &format!("{BASE} WHERE custom:amount >= 5000")).await,
        ["Epsilon"]
    );

    // Typed comparisons, negation that includes records without a value.
    for (filter, expected) in [
        ("custom:stage = 'won'", vec!["beta", "Delta"]),
        (
            "custom:stage IN ('won', 'lost')",
            vec!["beta", "Delta", "Gamma"],
        ),
        (
            "custom:stage NOT IN ('won', 'lost')",
            vec!["Alpha", "Eta", "Promo 50% off"],
        ),
        (
            "custom:amount >= 250.5 AND custom:amount < 1000",
            vec!["beta"],
        ),
        (
            "custom:amount != 100",
            vec!["beta", "Delta", "Eta", "Gamma", "Promo 50% off"],
        ),
        ("custom:amount = null", vec!["Gamma", "Promo 50% off"]),
        (
            "NOT (custom:amount > 50 OR custom:stage = 'lost')",
            vec!["Eta", "Promo 50% off"],
        ),
        ("custom:close = TODAY", vec!["beta"]),
        ("custom:close < TODAY", vec!["Alpha"]),
        ("custom:close = LAST_N_DAYS:7", vec!["beta"]),
        ("custom:close = LAST_N_DAYS:60", vec!["Alpha", "beta"]),
        ("custom:close > TODAY", vec![]),
        ("name LIKE 'b%'", vec!["beta"]),
        ("name LIKE '_eta'", vec!["beta"]),
        ("name LIKE '%50\\% off'", vec!["Promo 50% off"]),
        ("name LIKE '%50\\%'", vec![]),
        ("name = 'GAMMA'", vec!["Gamma"]),
        ("custom:email = 'sales@acme.EXAMPLE'", vec!["Alpha"]),
        ("custom:hot = true", vec!["Alpha"]),
        ("custom:hot = false", vec!["beta"]),
        ("custom:tags INCLUDES ('a')", vec!["Alpha"]),
        ("custom:tags INCLUDES ('a', 'b')", vec!["Alpha", "beta"]),
        (
            "custom:tags EXCLUDES ('a')",
            vec!["beta", "Delta", "Eta", "Gamma", "Promo 50% off"],
        ),
        (&format!("owner = '{bob}'"), vec!["Delta"]),
        (
            &format!("custom:company = '{}'", acme.public_id),
            vec!["Alpha", "Delta"],
        ),
        ("custom:company = null", vec!["Gamma", "Promo 50% off"]),
    ] {
        assert_eq!(
            names(&alice_s, &format!("{BASE} WHERE {filter} ORDER BY name")).await,
            expected,
            "{filter}"
        );
    }

    // Every hop applies the target's sharing: Initech is Carol's, so Eta's
    // company is hidden from Alice in conditions and in selected values.
    for (filter, expected) in [
        ("custom:company.name = 'Acme'", vec!["Alpha", "Delta"]),
        (
            "custom:company.custom:tier = 'Gold'",
            vec!["Alpha", "Delta"],
        ),
        ("custom:company.name = 'Initech'", vec![]),
        (
            "custom:company.name = null",
            vec!["Eta", "Gamma", "Promo 50% off"],
        ),
        (
            "custom:company.name LIKE 'g%' OR custom:stage = 'lost'",
            vec!["beta", "Gamma"],
        ),
    ] {
        assert_eq!(
            names(&alice_s, &format!("{BASE} WHERE {filter} ORDER BY name")).await,
            expected,
            "{filter}"
        );
    }
    assert_eq!(
        names(
            &carol_s,
            &format!("{BASE} WHERE custom:company.name = 'Initech' ORDER BY name")
        )
        .await,
        ["Epsilon", "Eta"]
    );
    let related = alice_s
        .query(request(
            "SELECT name, custom:company.name company, custom:company.custom:tier, custom:notes \
             FROM custom:opportunity WHERE name IN ('alpha', 'eta') ORDER BY name",
        ))
        .await
        .expect("related");
    assert_eq!(
        related
            .columns
            .iter()
            .map(|column| (column.key.as_str(), column.column_type))
            .collect::<Vec<_>>(),
        [
            ("name", "text"),
            ("company", "text"),
            ("custom:company.custom:tier", "picklist"),
            ("custom:notes", "long_text"),
        ]
    );
    assert_eq!(related.rows[0]["company"], json!("Acme"));
    assert_eq!(related.rows[0]["custom:company.custom:tier"], json!("Gold"));
    assert_eq!(related.rows[0]["custom:notes"], json!("long"));
    assert!(related.rows[0]["id"].is_string());
    assert_eq!(related.rows[1]["name"], json!("Eta"));
    assert_eq!(related.rows[1]["company"], Value::Null);

    // Keyset paging in the query's order, missing values last, with LIMIT.
    let mut seen = Vec::new();
    let mut cursor = None;
    let q = "SELECT name, custom:amount FROM custom:opportunity ORDER BY custom:amount DESC";
    for _ in 0..10 {
        let page = alice_s
            .query(QueryRequest {
                q: q.into(),
                cursor: cursor.clone(),
                page_size: Some(2),
                include_deleted: false,
            })
            .await
            .expect("page");
        assert!(page.rows.len() <= 2);
        seen.extend(
            page.rows
                .iter()
                .map(|row| row["name"].as_str().unwrap_or_default().to_owned()),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        seen,
        ["Delta", "beta", "Alpha", "Eta", "Gamma", "Promo 50% off"],
        "missing amounts sort last, then by id"
    );
    let first = alice_s
        .query(QueryRequest {
            q: format!("{q} LIMIT 3"),
            page_size: Some(2),
            ..QueryRequest::default()
        })
        .await
        .expect("limited");
    assert_eq!(first.rows.len(), 2);
    let second = alice_s
        .query(QueryRequest {
            q: format!("{q} LIMIT 3"),
            cursor: first.next_cursor.clone(),
            page_size: Some(2),
            include_deleted: false,
        })
        .await
        .expect("limited second page");
    assert_eq!(second.rows.len(), 1);
    assert!(second.next_cursor.is_none(), "LIMIT ends the paging");
    match alice_s
        .query(QueryRequest {
            q: format!("{BASE} ORDER BY custom:amount DESC"),
            cursor: first.next_cursor,
            ..QueryRequest::default()
        })
        .await
    {
        Err(ObjectError::Validation(issues)) => assert_eq!(issues[0].code, "query.cursor_invalid"),
        other => panic!("a cursor is bound to its query: {other:?}"),
    }
    let mut by_name = Vec::new();
    let mut cursor = None;
    loop {
        let page = alice_s
            .query(QueryRequest {
                q: format!("{BASE} ORDER BY custom:stage, name DESC"),
                cursor: cursor.clone(),
                page_size: Some(1),
                include_deleted: false,
            })
            .await
            .expect("page");
        by_name.extend(
            page.rows
                .iter()
                .map(|row| row["name"].as_str().unwrap_or_default().to_owned()),
        );
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        by_name,
        ["Gamma", "Eta", "Alpha", "Delta", "beta", "Promo 50% off"],
        "two keys in mixed directions, one row per page"
    );

    // Aggregates see only visible rows.
    let grouped = alice_s
        .query(request(
            "SELECT custom:stage, COUNT() n, SUM(custom:amount) total FROM custom:opportunity \
             GROUP BY custom:stage ORDER BY custom:stage",
        ))
        .await
        .expect("grouped");
    let rows: Vec<Value> = grouped.rows.into_iter().map(Value::Object).collect();
    assert_eq!(
        rows,
        [
            json!({"custom:stage": "lost", "n": 1, "total": null}),
            json!({"custom:stage": "open", "n": 2, "total": 110.0}),
            json!({"custom:stage": "won", "n": 2, "total": 1250.5}),
            json!({"custom:stage": null, "n": 1, "total": null}),
        ]
    );
    let totals = alice_s
        .query(request(
            "SELECT COUNT(), COUNT(custom:amount), MIN(custom:close), MAX(custom:amount), \
             AVG(custom:amount) FROM custom:opportunity",
        ))
        .await
        .expect("totals");
    assert_eq!(
        Value::Object(totals.rows[0].clone()),
        json!({
            "count()": 6,
            "count(custom:amount)": 4,
            "min(custom:close)": day(-40),
            "max(custom:amount)": 1000.0,
            "avg(custom:amount)": 340.125,
        })
    );
    let admin_count = admin
        .query(request("SELECT COUNT() n FROM custom:opportunity"))
        .await
        .expect("admin count");
    assert_eq!(admin_count.rows[0]["n"], json!(7));
    let by_flag = alice_s
        .query(request(
            "SELECT custom:hot, COUNT() n FROM custom:opportunity GROUP BY custom:hot ORDER BY n DESC",
        ))
        .await
        .expect("by flag");
    assert_eq!(
        by_flag
            .rows
            .into_iter()
            .map(Value::Object)
            .collect::<Vec<_>>(),
        [
            json!({"custom:hot": null, "n": 4}),
            json!({"custom:hot": false, "n": 1}),
            json!({"custom:hot": true, "n": 1}),
        ]
    );
    let by_company = alice_s
        .query(request(
            "SELECT custom:company, COUNT() n FROM custom:opportunity \
             WHERE custom:company != null GROUP BY custom:company ORDER BY n DESC, custom:company",
        ))
        .await
        .expect("by company");
    assert_eq!(by_company.rows[0]["custom:company"], json!(acme.public_id));
    assert_eq!(by_company.rows[0]["n"], json!(2));
    assert_eq!(by_company.rows.len(), 3, "Eta still names its company's ID");

    // The recycle bin, only on request.
    alice_s
        .delete_record(
            "custom:opportunity",
            &gamma.public_id,
            gamma.version,
            context(),
        )
        .await
        .expect("delete gamma");
    assert!(names(&alice_s, &format!("{BASE} WHERE name = 'gamma'"))
        .await
        .is_empty());
    let deleted = alice_s
        .query(QueryRequest {
            q: "SELECT name, is_deleted FROM custom:opportunity WHERE is_deleted = true".into(),
            include_deleted: true,
            ..QueryRequest::default()
        })
        .await
        .expect("recycle bin");
    assert_eq!(deleted.rows.len(), 1);
    assert_eq!(deleted.rows[0]["name"], json!("Gamma"));
    assert_eq!(deleted.rows[0]["is_deleted"], json!(true));

    // Stable error codes.
    assert_eq!(code(&outsider, BASE).await, "query.object_unknown");
    assert_eq!(
        code(&alice_s, "SELECT name FROM custom:nothing").await,
        "query.object_unknown"
    );
    assert_eq!(code(&alice_s, "SELECT name FROM").await, "query.syntax");
    assert_eq!(
        code(&alice_s, &format!("{BASE} WHERE custom:notes = 'x'")).await,
        "query.field_not_indexed"
    );
    assert_eq!(
        code(&alice_s, &format!("{BASE} WHERE custom:amount = 'lots'")).await,
        "query.value_invalid"
    );
}
