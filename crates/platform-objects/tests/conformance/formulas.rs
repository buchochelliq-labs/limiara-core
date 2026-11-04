//! Formula fields on every backend (ADR 0021): checked when defined,
//! computed whenever a record is read, read-only, with the reader's
//! sharing applied to lookups.

use super::{body, caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn formula(name: &str, text: &str, kind: FieldType) -> CreateField {
    let mut definition = field(name, FieldType::Formula);
    definition.formula = Some(text.into());
    definition.formula_type = Some(kind);
    definition
}

fn first_code(error: ObjectError) -> &'static str {
    match error {
        ObjectError::Validation(issues) => issues[0].code,
        other => panic!("expected validation issues, got {other:?}"),
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let role = Uuid::now_v7();
    let member = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));

    let mut account = object("account", "Account");
    account.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(account, context())
        .await
        .expect("account");
    let mut region = field("region", FieldType::Picklist);
    region.options = vec![option("emea", false), option("apac", false)];
    admin
        .create_field("custom:account", region, context())
        .await
        .expect("region");
    admin
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal");
    let mut lookup = field("account", FieldType::Lookup);
    lookup.reference_object = Some("custom:account".into());
    for definition in [
        field("amount", FieldType::Currency),
        field("quantity", FieldType::Number),
        lookup,
    ] {
        admin
            .create_field("custom:deal", definition, context())
            .await
            .expect("deal field");
    }
    for api_name in ["custom:account", "custom:deal"] {
        admin
            .set_object_permission(
                api_name,
                role,
                AccessFlags {
                    create: true,
                    edit: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("access");
    }

    // Formulas are checked when the field is defined.
    for (input, code) in [
        (
            formula("bad", "custom:amount *", FieldType::Number),
            "objects.formula_invalid",
        ),
        (
            formula("bad", "custom:amount * 2", FieldType::Text),
            "objects.formula_invalid",
        ),
        (
            formula("bad", "ISCHANGED(custom:amount)", FieldType::Boolean),
            "objects.formula_invalid",
        ),
        (
            formula("bad", "1", FieldType::Picklist),
            "objects.formula_type_invalid",
        ),
        (
            {
                let mut missing = field("bad", FieldType::Formula);
                missing.formula_type = Some(FieldType::Number);
                missing
            },
            "objects.formula_required",
        ),
        (
            {
                let mut stray = field("bad", FieldType::Text);
                stray.formula = Some("1".into());
                stray
            },
            "objects.formula_not_allowed",
        ),
        (
            {
                let mut indexed = formula("bad", "1", FieldType::Number);
                indexed.indexed = true;
                indexed
            },
            "objects.formula_settings_invalid",
        ),
    ] {
        assert_eq!(
            first_code(
                admin
                    .create_field("custom:deal", input, context())
                    .await
                    .expect_err("invalid formula field")
            ),
            code
        );
    }
    let mut total = formula("total", "custom:amount * 1.2", FieldType::Currency);
    total.scale = Some(2);
    for definition in [
        total,
        formula(
            "label",
            "name & ' x' & TEXT(custom:quantity)",
            FieldType::Text,
        ),
        formula("big", "custom:amount > 100", FieldType::Boolean),
        formula(
            "region",
            "TEXT(custom:account.custom:region)",
            FieldType::Text,
        ),
    ] {
        admin
            .create_field("custom:deal", definition, context())
            .await
            .expect("formula field");
    }
    // Formulas can use other formula fields, which are inlined.
    admin
        .create_field(
            "custom:deal",
            formula("twice", "custom:total * 2", FieldType::Number),
            context(),
        )
        .await
        .expect("formula on formula");

    // Values are computed on every read and cannot be written.
    let emea = admin
        .create_record(
            "custom:account",
            &body(json!({"name": "Emea Co", "custom:region": "emea"})),
            context(),
        )
        .await
        .expect("account");
    let created = admin
        .create_record(
            "custom:deal",
            &body(
                json!({"name": "Deal", "custom:amount": 99.99, "custom:quantity": 3,
                         "custom:account": emea.public_id}),
            ),
            context(),
        )
        .await
        .expect("deal");
    assert_eq!(created.values["custom:total"], json!(119.99));
    // The inlined total is not rounded; the result is, to no decimals.
    assert_eq!(created.values["custom:twice"], json!(240.0));
    assert_eq!(created.values["custom:label"], json!("Deal x3"));
    assert_eq!(created.values["custom:big"], json!(false));
    assert_eq!(created.values["custom:region"], json!("emea"));
    assert_eq!(
        first_code(
            admin
                .update_record(
                    "custom:deal",
                    &created.public_id,
                    created.version,
                    &body(json!({"custom:total": 1})),
                    context(),
                )
                .await
                .expect_err("read only")
        ),
        "records.field_read_only"
    );
    let updated = admin
        .update_record(
            "custom:deal",
            &created.public_id,
            created.version,
            &body(json!({"custom:amount": 200})),
            context(),
        )
        .await
        .expect("update");
    assert_eq!(updated.values["custom:total"], json!(240.0));
    assert_eq!(updated.values["custom:big"], json!(true));

    // Lookups apply the reader's sharing: the member cannot see the account.
    let seen = member
        .get_record("custom:deal", &created.public_id, false)
        .await
        .expect("member reads the deal");
    assert_eq!(seen.values["custom:total"], json!(240.0));
    assert!(
        !seen.values.contains_key("custom:region"),
        "a hidden account gives a blank"
    );
    // A formula field read through a lookup is blank when the related
    // record is unavailable, even if its own expression is a constant.
    admin
        .create_field(
            "custom:account",
            formula("tier", "'gold'", FieldType::Text),
            context(),
        )
        .await
        .expect("constant formula");
    admin
        .create_field(
            "custom:deal",
            formula("tier", "custom:account.custom:tier", FieldType::Text),
            context(),
        )
        .await
        .expect("related formula");
    let read = admin
        .get_record("custom:deal", &created.public_id, false)
        .await
        .expect("admin reads the deal");
    assert_eq!(read.values["custom:tier"], json!("gold"));
    let hidden = member
        .get_record("custom:deal", &created.public_id, false)
        .await
        .expect("member reads the deal");
    assert!(
        !hidden.values.contains_key("custom:tier"),
        "a hidden account gives a blank, constant or not"
    );
    let orphan = admin
        .create_record(
            "custom:deal",
            &body(json!({"name": "Orphan", "custom:amount": 1, "custom:quantity": 1})),
            context(),
        )
        .await
        .expect("deal without an account");
    assert!(
        !orphan.values.contains_key("custom:tier"),
        "an empty lookup gives a blank"
    );
    admin
        .delete_record("custom:deal", &orphan.public_id, orphan.version, context())
        .await
        .expect("remove the orphan");
    let listed = member
        .list_records("custom:deal", service::ListRecords::default())
        .await
        .expect("list");
    assert_eq!(listed.records[0].values["custom:label"], json!("Deal x3"));

    // Queries select formula values. Formulas that read only the record are
    // stored, so queries can filter, sort and aggregate on them; formulas
    // that follow lookups are computed when read and cannot be filtered.
    let rows = admin
        .query(QueryRequest {
            q: "SELECT name, custom:total, custom:region FROM custom:deal".into(),
            ..QueryRequest::default()
        })
        .await
        .expect("query");
    assert_eq!(rows.rows[0]["custom:total"], json!(240.0));
    assert_eq!(rows.columns[1].column_type, "currency");
    let small = admin
        .create_record(
            "custom:deal",
            &body(json!({"name": "Small", "custom:amount": 5})),
            context(),
        )
        .await
        .expect("small deal");
    let query = |q: &str| {
        admin.query(QueryRequest {
            q: q.into(),
            ..QueryRequest::default()
        })
    };
    let names = |result: QueryResult| -> Vec<Value> {
        result
            .rows
            .into_iter()
            .map(|row| row["name"].clone())
            .collect()
    };
    assert_eq!(
        names(
            query("SELECT name FROM custom:deal WHERE custom:big = true")
                .await
                .expect("filter on a stored formula")
        ),
        [json!("Deal")]
    );
    assert_eq!(
        names(
            query("SELECT name FROM custom:deal WHERE custom:total < 100 ORDER BY custom:total")
                .await
                .expect("range and sort")
        ),
        [json!("Small")]
    );
    // Pages sorted on a stored formula continue from its value.
    let first = admin
        .query(QueryRequest {
            q: "SELECT name FROM custom:deal ORDER BY custom:total DESC".into(),
            page_size: Some(1),
            ..QueryRequest::default()
        })
        .await
        .expect("first page");
    assert_eq!(first.rows[0]["name"], json!("Deal"));
    let second = admin
        .query(QueryRequest {
            q: "SELECT name FROM custom:deal ORDER BY custom:total DESC".into(),
            page_size: Some(1),
            cursor: first.next_cursor.clone(),
            ..QueryRequest::default()
        })
        .await
        .expect("second page");
    assert_eq!(second.rows[0]["name"], json!("Small"));
    // Aggregates over formulas use the formula's result type.
    let grouped = query("SELECT custom:big, COUNT(id) n FROM custom:deal GROUP BY custom:big")
        .await
        .expect("group by a boolean formula");
    assert!(grouped
        .rows
        .iter()
        .all(|row| row["custom:big"].is_boolean()));
    let sum = query("SELECT SUM(custom:total) total FROM custom:deal")
        .await
        .expect("aggregate");
    assert_eq!(sum.rows[0]["total"], json!(246.0));
    match query("SELECT name FROM custom:deal WHERE custom:region = 'emea'").await {
        Err(ObjectError::Validation(issues)) => {
            assert_eq!(issues[0].code, "query.formula_not_filterable")
        }
        other => panic!("formulas through lookups are computed when read: {other:?}"),
    }
    admin
        .delete_record("custom:deal", &small.public_id, small.version, context())
        .await
        .expect("remove the small deal");

    // A formula cannot come back to itself, and one another formula uses
    // cannot be deleted while it does.
    let total_field = admin
        .describe("custom:deal")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:total")
        .expect("total");
    let cycle: UpdateField = serde_json::from_value(
        json!({"expected_version": total_field.version, "formula": "custom:twice / 2"}),
    )
    .expect("change");
    assert_eq!(
        first_code(
            admin
                .update_field("custom:deal", "custom:total", cycle, context())
                .await
                .expect_err("cycle")
        ),
        "objects.formula_invalid"
    );
    assert!(matches!(
        admin
            .delete_field(
                "custom:deal",
                "custom:total",
                total_field.version,
                context()
            )
            .await,
        Err(ObjectError::Conflict("objects.field_in_use"))
    ));
    let rule: CreateValidationRule = serde_json::from_value(json!({
        "name": "not_too_big", "condition": "custom:twice > 1000000",
        "message": "Too big"}))
    .expect("rule");
    admin
        .create_validation_rule("custom:deal", rule, context())
        .await
        .expect("rules can use formula fields");

    // Changing a formula changes every read.
    let label = admin
        .describe("custom:deal")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:label")
        .expect("label");
    let change: UpdateField = serde_json::from_value(
        json!({"expected_version": label.version, "formula": "UPPER(name)"}),
    )
    .expect("change");
    admin
        .update_field("custom:deal", "custom:label", change, context())
        .await
        .expect("new formula");
    let read = admin
        .get_record("custom:deal", &created.public_id, false)
        .await
        .expect("read");
    assert_eq!(read.values["custom:label"], json!("DEAL"));
    // Changing a formula restores its stored values for every record.
    assert_eq!(
        names(
            query("SELECT name FROM custom:deal WHERE custom:label = 'deal'")
                .await
                .expect("filter on the new formula")
        ),
        [json!("Deal")]
    );

    // A field a formula needs cannot be deleted; the formula field itself can.
    let quantity_free = admin
        .describe("custom:deal")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:amount")
        .expect("amount");
    assert!(matches!(
        admin
            .delete_field(
                "custom:deal",
                "custom:amount",
                quantity_free.version,
                context()
            )
            .await,
        Err(ObjectError::Conflict("objects.field_in_use"))
    ));
    let big = admin
        .describe("custom:deal")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:big")
        .expect("big");
    admin
        .delete_field("custom:deal", "custom:big", big.version, context())
        .await
        .expect("delete a formula field");
    let read = admin
        .get_record("custom:deal", &created.public_id, false)
        .await
        .expect("read");
    assert!(!read.values.contains_key("custom:big"));
    assert_eq!(read.values.get("custom:total"), Some(&Value::from(240.0)));
}
