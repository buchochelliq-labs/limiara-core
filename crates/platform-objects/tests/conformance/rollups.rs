//! Roll-up summary fields on every backend (ADR 0026): a master's count,
//! sum, minimum and maximum over its details, kept current through every
//! change to the details and readable, filterable and sortable like any
//! stored value.

use super::{body, caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn master_detail(name: &str, master: &str) -> CreateField {
    let mut definition = field(name, FieldType::MasterDetail);
    definition.reference_object = Some(master.into());
    definition
}

fn rollup(
    name: &str,
    function: RollupFunction,
    object: &str,
    relationship: &str,
    summarised: Option<&str>,
) -> CreateField {
    let mut definition = field(name, FieldType::Rollup);
    definition.rollup = Some(CreateRollup {
        function,
        object: object.into(),
        relationship: relationship.into(),
        field: summarised.map(str::to_owned),
    });
    definition
}

/// The account's roll-up values: lines, total, first shipment, biggest.
async fn summary(admin: &ObjectService, public_id: &str) -> [Value; 4] {
    let record = admin
        .get_record("custom:account", public_id, false)
        .await
        .expect("account");
    [
        "custom:lines",
        "custom:total",
        "custom:first_ship",
        "custom:biggest",
    ]
    .map(|api| record.values.get(api).cloned().unwrap_or(Value::Null))
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    for (name, label) in [("account", "Account"), ("line", "Line")] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    admin
        .create_field(
            "custom:line",
            master_detail("account", "custom:account"),
            context(),
        )
        .await
        .expect("master-detail");
    let mut amount = field("amount", FieldType::Currency);
    amount.scale = Some(2);
    for definition in [
        amount,
        field("shipped", FieldType::Date),
        field("note", FieldType::Text),
    ] {
        admin
            .create_field("custom:line", definition, context())
            .await
            .expect("line field");
    }

    // Roll-ups are checked against the detail object when created.
    for (definition, code) in [
        (
            rollup(
                "bad",
                RollupFunction::Sum,
                "custom:line",
                "custom:account",
                None,
            ),
            "objects.rollup_field_required",
        ),
        (
            rollup(
                "bad",
                RollupFunction::Count,
                "custom:line",
                "custom:account",
                Some("custom:amount"),
            ),
            "objects.rollup_field_not_allowed",
        ),
        (
            rollup(
                "bad",
                RollupFunction::Sum,
                "custom:line",
                "custom:account",
                Some("custom:shipped"),
            ),
            "objects.rollup_field_invalid",
        ),
        (
            rollup(
                "bad",
                RollupFunction::Count,
                "custom:line",
                "custom:note",
                None,
            ),
            "objects.rollup_relationship_invalid",
        ),
        (
            rollup(
                "bad",
                RollupFunction::Count,
                "custom:nowhere",
                "custom:account",
                None,
            ),
            "objects.rollup_object_unknown",
        ),
    ] {
        let refused = admin
            .create_field("custom:account", definition, context())
            .await
            .expect_err("invalid roll-up");
        assert_eq!(codes(&refused), [code]);
    }
    let refused = admin
        .create_field(
            "custom:line",
            rollup(
                "bad",
                RollupFunction::Count,
                "custom:line",
                "custom:account",
                None,
            ),
            context(),
        )
        .await
        .expect_err("a roll-up on the detail");
    assert_eq!(codes(&refused), ["objects.rollup_relationship_invalid"]);

    let acme = admin
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("acme");
    let anvils = admin
        .create_record(
            "custom:line",
            &body(json!({
                "name": "Anvils",
                "custom:account": acme.public_id,
                "custom:amount": 120.5,
                "custom:shipped": "2026-11-02",
            })),
            context(),
        )
        .await
        .expect("anvils");

    // A roll-up created later counts the details that already exist, and
    // indexes the field it summarises.
    let lines = admin
        .create_field(
            "custom:account",
            rollup(
                "lines",
                RollupFunction::Count,
                "custom:line",
                "custom:account",
                None,
            ),
            context(),
        )
        .await
        .expect("count");
    assert_eq!(lines.formula_type, Some(FieldType::Number));
    // Its settings stay those of a computed field.
    let required: UpdateField =
        serde_json::from_value(json!({"expected_version": lines.version, "required": true}))
            .expect("update");
    let refused = admin
        .update_field("custom:account", "custom:lines", required, context())
        .await
        .expect_err("required roll-up");
    assert_eq!(codes(&refused), ["objects.rollup_settings_invalid"]);
    let total = admin
        .create_field(
            "custom:account",
            rollup(
                "total",
                RollupFunction::Sum,
                "custom:line",
                "custom:account",
                Some("custom:amount"),
            ),
            context(),
        )
        .await
        .expect("sum");
    assert_eq!(
        (total.formula_type, total.scale),
        (Some(FieldType::Currency), Some(2))
    );
    let (line, _) = admin.describe("custom:line").await.expect("line");
    assert!(
        line.field("custom:amount").expect("amount").is_indexed(),
        "the summarised field is indexed"
    );
    for (name, function, summarised) in [
        ("first_ship", RollupFunction::Min, "custom:shipped"),
        ("biggest", RollupFunction::Max, "custom:amount"),
    ] {
        admin
            .create_field(
                "custom:account",
                rollup(
                    name,
                    function,
                    "custom:line",
                    "custom:account",
                    Some(summarised),
                ),
                context(),
            )
            .await
            .expect("min or max");
    }
    assert_eq!(
        summary(&admin, &acme.public_id).await,
        [json!(1), json!(120.5), json!("2026-11-02"), json!(120.5)]
    );

    // Every change to a detail keeps its master current.
    let rockets = admin
        .create_record(
            "custom:line",
            &body(json!({
                "name": "Rockets",
                "custom:account": acme.public_id,
                "custom:amount": 1000,
                "custom:shipped": "2026-10-20",
            })),
            context(),
        )
        .await
        .expect("rockets");
    assert_eq!(
        summary(&admin, &acme.public_id).await,
        [json!(2), json!(1120.5), json!("2026-10-20"), json!(1000)]
    );
    let rockets = admin
        .update_record(
            "custom:line",
            &rockets.public_id,
            rockets.version,
            &body(json!({"custom:amount": 10})),
            context(),
        )
        .await
        .expect("cheaper rockets");
    assert_eq!(
        summary(&admin, &acme.public_id).await,
        [json!(2), json!(130.5), json!("2026-10-20"), json!(120.5)]
    );
    let anvils_now = admin
        .get_record("custom:line", &anvils.public_id, false)
        .await
        .expect("anvils");
    let anvils_deleted = admin
        .delete_record(
            "custom:line",
            &anvils.public_id,
            anvils_now.version,
            context(),
        )
        .await
        .expect("delete anvils");
    assert_eq!(
        summary(&admin, &acme.public_id).await,
        [json!(1), json!(10), json!("2026-10-20"), json!(10)]
    );
    admin
        .undelete_record(
            "custom:line",
            &anvils.public_id,
            anvils_deleted.version,
            context(),
        )
        .await
        .expect("restore anvils");
    assert_eq!(
        summary(&admin, &acme.public_id).await,
        [json!(2), json!(130.5), json!("2026-10-20"), json!(120.5)]
    );
    // A detail change that leaves the summaries as they are leaves the
    // master alone.
    let before = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    admin
        .update_record(
            "custom:line",
            &rockets.public_id,
            rockets.version,
            &body(json!({"custom:note": "fragile"})),
            context(),
        )
        .await
        .expect("note");
    let after = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    assert_eq!(after.version, before.version);

    // Roll-ups are read-only, and queries filter and sort on them.
    let refused = admin
        .update_record(
            "custom:account",
            &acme.public_id,
            after.version,
            &body(json!({"custom:lines": 9})),
            context(),
        )
        .await
        .expect_err("written roll-up");
    assert_eq!(codes(&refused), ["records.field_read_only"]);
    let globex = admin
        .create_record(
            "custom:account",
            &body(json!({"name": "Globex"})),
            context(),
        )
        .await
        .expect("globex");
    assert_eq!(
        summary(&admin, &globex.public_id).await,
        [json!(0), json!(0), Value::Null, Value::Null],
        "a master without details counts and sums zero"
    );
    let rows = admin
        .query(QueryRequest {
            q: "SELECT name, custom:total FROM custom:account WHERE custom:lines > 0 \
                ORDER BY custom:total DESC"
                .into(),
            ..QueryRequest::default()
        })
        .await
        .expect("query");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0]["name"], json!("Acme"));
    assert_eq!(rows.columns[1].column_type, "currency");

    // Deleting the master takes its details and restoring it brings back
    // its summaries with them.
    let acme_now = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    let deleted = admin
        .delete_record(
            "custom:account",
            &acme.public_id,
            acme_now.version,
            context(),
        )
        .await
        .expect("delete acme");
    let restored = admin
        .undelete_record(
            "custom:account",
            &acme.public_id,
            deleted.version,
            context(),
        )
        .await
        .expect("restore acme");
    let reread = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    assert_eq!(
        restored.version, reread.version,
        "undelete returns the latest version"
    );
    assert_eq!(
        summary(&admin, &acme.public_id).await,
        [json!(2), json!(130.5), json!("2026-10-20"), json!(120.5)]
    );

    // A junction record leaving with one master leaves the other's count.
    admin
        .create_object(object("product", "Product"), context())
        .await
        .expect("product");
    admin
        .create_object(object("offer", "Offer"), context())
        .await
        .expect("offer");
    for (name, master) in [("account", "custom:account"), ("product", "custom:product")] {
        admin
            .create_field("custom:offer", master_detail(name, master), context())
            .await
            .expect("junction master");
    }
    admin
        .create_field(
            "custom:product",
            rollup(
                "offers",
                RollupFunction::Count,
                "custom:offer",
                "custom:product",
                None,
            ),
            context(),
        )
        .await
        .expect("offer count");
    let gadget = admin
        .create_record(
            "custom:product",
            &body(json!({"name": "Gadget"})),
            context(),
        )
        .await
        .expect("gadget");
    for account in [&acme.public_id, &globex.public_id] {
        admin
            .create_record(
                "custom:offer",
                &body(json!({
                    "name": "Offer",
                    "custom:account": account,
                    "custom:product": gadget.public_id,
                })),
                context(),
            )
            .await
            .expect("offer");
    }
    let offers = |record: Record| record.values.get("custom:offers").cloned();
    assert_eq!(
        offers(
            admin
                .get_record("custom:product", &gadget.public_id, false)
                .await
                .expect("gadget")
        ),
        Some(json!(2))
    );
    let globex_now = admin
        .get_record("custom:account", &globex.public_id, false)
        .await
        .expect("globex");
    let globex_deleted = admin
        .delete_record(
            "custom:account",
            &globex.public_id,
            globex_now.version,
            context(),
        )
        .await
        .expect("delete globex");
    assert_eq!(
        offers(
            admin
                .get_record("custom:product", &gadget.public_id, false)
                .await
                .expect("gadget")
        ),
        Some(json!(1))
    );
    admin
        .undelete_record(
            "custom:account",
            &globex.public_id,
            globex_deleted.version,
            context(),
        )
        .await
        .expect("restore globex");
    assert_eq!(
        offers(
            admin
                .get_record("custom:product", &gadget.public_id, false)
                .await
                .expect("gadget")
        ),
        Some(json!(2))
    );

    // What a roll-up reads stays while it exists.
    let (line, _) = admin.describe("custom:line").await.expect("line");
    let amount = line.field("custom:amount").expect("amount").clone();
    let refused = admin
        .delete_field("custom:line", "custom:amount", amount.version, context())
        .await
        .expect_err("summarised field");
    assert!(matches!(
        refused,
        ObjectError::Conflict("objects.field_in_use")
    ));
    let unindex: UpdateField =
        serde_json::from_value(json!({"expected_version": amount.version, "indexed": false}))
            .expect("update");
    let refused = admin
        .update_field("custom:line", "custom:amount", unindex, context())
        .await
        .expect_err("unindexed");
    assert_eq!(codes(&refused), ["objects.rollup_needs_index"]);
    let relationship = line.field("custom:account").expect("account").clone();
    let refused = admin
        .delete_field(
            "custom:line",
            "custom:account",
            relationship.version,
            context(),
        )
        .await
        .expect_err("relationship");
    assert!(matches!(
        refused,
        ObjectError::Conflict("objects.field_in_use")
    ));
    let refused = admin
        .delete_object("custom:line", line.version, context())
        .await
        .expect_err("detail object");
    assert!(matches!(
        refused,
        ObjectError::Conflict("objects.object_in_use")
    ));
}

/// A master inserted while a roll-up summary is being added to its object
/// is refused, rather than saved without the roll-up's starting value
/// (ADR 0026), on a store whose transactions interleave.
pub async fn locks(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    for (name, label) in [("account", "Account"), ("line", "Line")] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    admin
        .create_field(
            "custom:line",
            master_detail("account", "custom:account"),
            context(),
        )
        .await
        .expect("master-detail");
    let lines = admin
        .create_field(
            "custom:account",
            rollup(
                "lines",
                RollupFunction::Count,
                "custom:line",
                "custom:account",
                None,
            ),
            context(),
        )
        .await
        .expect("count");
    let mut adding = store.begin(tenant).await.expect("transaction");
    adding
        .lock_object(lines.object_id, true)
        .await
        .expect("lock");
    adding
        .insert_field(&FieldDefinition {
            id: Uuid::now_v7(),
            api_name: "custom:more_lines".into(),
            ..lines.clone()
        })
        .await
        .expect("field");
    let acme = body(json!({"name": "Acme"}));
    let (inserted, ()) = tokio::join!(
        admin.create_record("custom:account", &acme, context()),
        async {
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            adding.commit().await.expect("commit field");
        }
    );
    assert!(
        matches!(
            inserted,
            Err(ObjectError::Conflict("records.definition_changed"))
        ),
        "a master that would miss the new roll-up is refused: {inserted:?}"
    );
    let retried = admin
        .create_record("custom:account", &acme, context())
        .await
        .expect("retry");
    assert_eq!(retried.values.get("custom:more_lines"), Some(&json!(0)));
}
