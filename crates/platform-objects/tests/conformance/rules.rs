//! Validation rules on every backend (ADR 0021): checked when saved,
//! enforced on create, update and owner change, in system context, with
//! dependencies that keep their fields.

use super::{body, caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn rule(
    name: &str,
    condition: &str,
    message: &str,
    error_field: Option<&str>,
) -> CreateValidationRule {
    serde_json::from_value(json!({
        "name": name,
        "condition": condition,
        "message": message,
        "error_field": error_field,
    }))
    .expect("rule")
}

/// `(field, code, detail)` of a failed save.
fn issues(error: ObjectError) -> Vec<(String, &'static str, String)> {
    match error {
        ObjectError::Validation(issues) => issues
            .into_iter()
            .map(|issue| (issue.field, issue.code, issue.detail))
            .collect(),
        other => panic!("expected validation issues, got {other:?}"),
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin_id = Uuid::now_v7();
    let admin = service(&store, caller(tenant, admin_id, &ADMIN_PERMISSIONS, &[]));
    let member_role = Uuid::now_v7();
    let member_id = Uuid::now_v7();
    let member = service(&store, caller(tenant, member_id, &[], &[member_role]));

    admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut region = field("region", FieldType::Picklist);
    region.options = vec![option("emea", false), option("closed", false)];
    admin
        .create_field("custom:account", region, context())
        .await
        .expect("region");
    admin
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal");
    let mut stage = field("stage", FieldType::Picklist);
    stage.options = vec![option("open", true), option("won", false)];
    let mut account = field("account", FieldType::Lookup);
    account.reference_object = Some("custom:account".into());
    for definition in [
        stage,
        field("amount", FieldType::Currency),
        field("close", FieldType::Date),
        field("notes", FieldType::Text),
        account,
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
                member_role,
                AccessFlags {
                    create: true,
                    edit: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("member access");
    }

    // Rules are checked when saved.
    for (input, field, code) in [
        (
            rule("bad", "custom:amount >", "m", None),
            "condition",
            "rules.condition_invalid",
        ),
        (
            rule("bad", "custom:amount", "m", None),
            "condition",
            "rules.condition_invalid",
        ),
        (
            rule("bad", "custom:nope = 1", "m", None),
            "condition",
            "rules.condition_invalid",
        ),
        (
            rule("Bad Name", "TRUE", "m", None),
            "name",
            "rules.name_invalid",
        ),
        (
            rule("bad", "TRUE", "", None),
            "message",
            "rules.message_invalid",
        ),
        (
            rule("bad", "TRUE", "m", Some("custom:nope")),
            "error_field",
            "rules.field_unknown",
        ),
    ] {
        let found = issues(
            admin
                .create_validation_rule("custom:deal", input, context())
                .await
                .expect_err("invalid rule"),
        );
        assert!(
            found
                .iter()
                .any(|(name, found_code, _)| name == field && *found_code == code),
            "{field} {code} in {found:?}"
        );
    }
    let positioned = issues(
        admin
            .create_validation_rule(
                "custom:deal",
                rule("bad", "custom:amount >", "m", None),
                context(),
            )
            .await
            .expect_err("syntax"),
    );
    assert!(
        positioned[0].2.contains("(at position 16)"),
        "{positioned:?}"
    );
    assert!(matches!(
        member
            .create_validation_rule("custom:deal", rule("mine", "TRUE", "m", None), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    // Members who can read the object read its active rules (ADR 0021,
    // "Reading rules"); there are none yet.
    assert!(member
        .validation_rules("custom:deal")
        .await
        .expect("a reader lists rules")
        .is_empty());

    let positive = admin
        .create_validation_rule(
            "custom:deal",
            rule(
                "amount_positive",
                "custom:amount <= 0",
                "Amount must be positive",
                Some("custom:amount"),
            ),
            context(),
        )
        .await
        .expect("amount rule");
    assert_eq!(positive.version, 1);
    assert!(positive.active);
    for input in [
        rule(
            "won_needs_close",
            "ISPICKVAL(custom:stage, 'won') && ISBLANK(custom:close)",
            "Won deals need a close date",
            None,
        ),
        rule(
            "no_reopen",
            "ISCHANGED(custom:stage) && PRIORVALUE(custom:stage) = 'won'",
            "Won deals cannot be reopened",
            Some("custom:stage"),
        ),
        rule(
            "closed_region",
            "custom:account.custom:region = 'closed'",
            "The account's region is closed",
            Some("custom:account"),
        ),
        rule(
            "ratio",
            "custom:amount > 0 && 100 / (custom:amount - 1) > 1000",
            "Ratio too high",
            None,
        ),
    ] {
        admin
            .create_validation_rule("custom:deal", input, context())
            .await
            .expect("rule");
    }
    assert!(matches!(
        admin
            .create_validation_rule("custom:deal", rule("ratio", "TRUE", "m", None), context())
            .await,
        Err(ObjectError::Conflict("rules.duplicate_name"))
    ));
    let listed: Vec<String> = admin
        .validation_rules("custom:deal")
        .await
        .expect("list")
        .into_iter()
        .map(|rule| rule.name)
        .collect();
    assert_eq!(
        listed,
        [
            "amount_positive",
            "closed_region",
            "no_reopen",
            "ratio",
            "won_needs_close"
        ]
    );

    // Creates: every true condition is reported; blank values pass.
    let create = |value: Value| {
        let member = &member;
        async move {
            member
                .create_record("custom:deal", &body(value), context())
                .await
        }
    };
    let failed = issues(
        create(json!({"name": "Bad", "custom:amount": -5, "custom:stage": "won"}))
            .await
            .expect_err("two rules"),
    );
    assert_eq!(
        failed,
        [
            (
                "custom:amount".to_owned(),
                "records.validation_rule",
                "Amount must be positive".to_owned()
            ),
            (
                "record".to_owned(),
                "records.validation_rule",
                "Won deals need a close date".to_owned()
            ),
        ]
    );
    let deal = create(json!({"name": "Fine"}))
        .await
        .expect("blank amount passes");
    let won = create(json!({"name": "Won", "custom:stage": "won", "custom:close": "2026-10-01", "custom:amount": 50}))
        .await
        .expect("won deal");
    let broken = issues(
        create(json!({"name": "Ratio", "custom:amount": 1}))
            .await
            .expect_err("division by zero"),
    );
    assert_eq!(broken[0].1, "records.validation_rule_error");
    assert!(broken[0].2.contains("ratio"));

    // Updates compare with the stored record.
    let reopened = issues(
        member
            .update_record(
                "custom:deal",
                &won.public_id,
                won.version,
                &body(json!({"custom:stage": "open"})),
                context(),
            )
            .await
            .expect_err("reopen"),
    );
    assert_eq!(reopened[0].0, "custom:stage");
    member
        .update_record(
            "custom:deal",
            &won.public_id,
            won.version,
            &body(json!({"custom:notes": "kept won"})),
            context(),
        )
        .await
        .expect("an unrelated change passes");

    // Cross-object references read the lookup's target.
    let closed = admin
        .create_record(
            "custom:account",
            &body(json!({"name": "Closed Co", "custom:region": "closed"})),
            context(),
        )
        .await
        .expect("closed account");
    let refused = issues(
        member
            .update_record(
                "custom:deal",
                &deal.public_id,
                deal.version,
                &body(json!({"custom:account": closed.public_id})),
                context(),
            )
            .await
            .expect_err("closed region"),
    );
    assert_eq!(
        refused[0],
        (
            "custom:account".to_owned(),
            "records.validation_rule",
            "The account's region is closed".to_owned()
        )
    );

    // Inactive rules do not run; versions guard changes.
    let positive = admin
        .update_validation_rule(
            "custom:deal",
            "amount_positive",
            serde_json::from_value(json!({"expected_version": 1, "active": false}))
                .expect("change"),
            context(),
        )
        .await
        .expect("deactivate");
    assert_eq!(positive.version, 2);
    create(json!({"name": "Negative ok", "custom:amount": -1}))
        .await
        .expect("inactive rule");

    // Administrators see every rule; members who can read the object see
    // the active ones, which constrain their saves; to anyone else the
    // object is unknown.
    let names = |rules: Vec<ValidationRule>| -> Vec<String> {
        rules.into_iter().map(|rule| rule.name).collect()
    };
    let all = names(admin.validation_rules("custom:deal").await.expect("all"));
    let active = names(
        member
            .validation_rules("custom:deal")
            .await
            .expect("active"),
    );
    assert!(all.contains(&"amount_positive".to_owned()));
    assert!(
        !active.contains(&"amount_positive".to_owned()),
        "{active:?}"
    );
    assert!(!active.is_empty());
    assert!(active.iter().all(|name| all.contains(name)));
    let stranger = service(
        &store,
        caller(tenant, Uuid::now_v7(), &[], &[Uuid::now_v7()]),
    );
    assert!(matches!(
        stranger.validation_rules("custom:deal").await,
        Err(ObjectError::NotFound)
    ));
    assert!(matches!(
        admin
            .update_validation_rule(
                "custom:deal",
                "amount_positive",
                serde_json::from_value(json!({"expected_version": 1, "active": true}))
                    .expect("change"),
                context(),
            )
            .await,
        Err(ObjectError::VersionConflict)
    ));
    let invalid = admin
        .update_validation_rule(
            "custom:deal",
            "amount_positive",
            serde_json::from_value(
                json!({"expected_version": 2, "condition": "custom:amount + 'x'"}),
            )
            .expect("change"),
            context(),
        )
        .await
        .expect_err("invalid change");
    assert_eq!(issues(invalid)[0].1, "rules.condition_invalid");

    // Owner changes run rules too.
    let other = Uuid::now_v7();
    admin
        .create_validation_rule(
            "custom:deal",
            rule(
                "no_handover",
                &format!("ISCHANGED(owner) && owner = '{other}'"),
                "Not to that member",
                None,
            ),
            context(),
        )
        .await
        .expect("owner rule");
    let fine = admin
        .get_record("custom:deal", &deal.public_id, false)
        .await
        .expect("deal");
    let handover = admin
        .change_owner(
            "custom:deal",
            &fine.public_id,
            other,
            fine.version,
            context(),
        )
        .await
        .expect_err("owner rule");
    assert_eq!(issues(handover)[0].2, "Not to that member");

    // A field an active rule needs cannot be deleted until the rule goes.
    let close = admin
        .describe("custom:deal")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:close")
        .expect("close");
    assert!(matches!(
        admin
            .delete_field("custom:deal", "custom:close", close.version, context())
            .await,
        Err(ObjectError::Conflict("objects.field_in_use"))
    ));
    let region_field = admin
        .describe("custom:account")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:region")
        .expect("region");
    assert!(
        matches!(
            admin
                .delete_field(
                    "custom:account",
                    "custom:region",
                    region_field.version,
                    context()
                )
                .await,
            Err(ObjectError::Conflict("objects.field_in_use"))
        ),
        "a rule on another object counts too"
    );
    let rule = admin
        .validation_rules("custom:deal")
        .await
        .expect("rules")
        .into_iter()
        .find(|rule| rule.name == "won_needs_close")
        .expect("rule");
    admin
        .delete_validation_rule("custom:deal", "won_needs_close", rule.version, context())
        .await
        .expect("delete rule");
    admin
        .delete_field("custom:deal", "custom:close", close.version, context())
        .await
        .expect("delete the freed field");

    // Only active rules keep fields, and the field a rule shows its message
    // on counts as needed.
    for name in ["spare", "target"] {
        admin
            .create_field("custom:deal", field(name, FieldType::Text), context())
            .await
            .expect("field");
    }
    let mut dormant = self::rule("dormant", "ISBLANK(custom:spare)", "Spare is empty", None);
    dormant.active = Some(false);
    admin
        .create_validation_rule("custom:deal", dormant, context())
        .await
        .expect("inactive rule");
    admin
        .create_validation_rule(
            "custom:deal",
            self::rule(
                "targeted",
                "custom:amount < -1000",
                "Far too negative",
                Some("custom:target"),
            ),
            context(),
        )
        .await
        .expect("rule with an error target");
    let version_of = |name: &str| {
        let admin = &admin;
        let api_name = format!("custom:{name}");
        async move {
            admin
                .describe("custom:deal")
                .await
                .expect("describe")
                .0
                .fields
                .into_iter()
                .find(|field| field.api_name == api_name)
                .expect("field")
                .version
        }
    };
    admin
        .delete_field(
            "custom:deal",
            "custom:spare",
            version_of("spare").await,
            context(),
        )
        .await
        .expect("an inactive rule does not keep its field");
    assert!(matches!(
        admin
            .delete_field(
                "custom:deal",
                "custom:target",
                version_of("target").await,
                context()
            )
            .await,
        Err(ObjectError::Conflict("objects.field_in_use"))
    ));
}
