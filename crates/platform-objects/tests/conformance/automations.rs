//! Record-triggered automations on every backend (ADR 0022): checked when
//! saved, run in the save transaction in system context, and rolled back
//! with the change when anything fails.

use super::{body, caller, context, field, object, option, service, ADMIN_PERMISSIONS};
use platform_objects::service::ListRecords;
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn automation(value: Value) -> CreateAutomation {
    serde_json::from_value(value).expect("automation")
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

async fn names(service: &ObjectService, api_name: &str) -> Vec<String> {
    let mut names: Vec<String> = service
        .list_records(api_name, ListRecords::default())
        .await
        .expect("list")
        .records
        .into_iter()
        .map(|record| record.name)
        .collect();
    names.sort();
    names
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let role = Uuid::now_v7();
    let member_id = Uuid::now_v7();
    let member = service(&store, caller(tenant, member_id, &[], &[role]));

    // Accounts are private and the member has no access to accounts or
    // follow-ups at all: automations still write them, in system context.
    let mut account = object("account", "Account");
    account.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(account, context())
        .await
        .expect("account");
    admin
        .create_field(
            "custom:account",
            field("status", FieldType::Text),
            context(),
        )
        .await
        .expect("status");
    admin
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal");
    let mut stage = field("stage", FieldType::Picklist);
    stage.options = vec![option("open", true), option("won", false)];
    let mut amount = field("amount", FieldType::Currency);
    amount.scale = Some(2);
    let mut account_lookup = field("account", FieldType::Lookup);
    account_lookup.reference_object = Some("custom:account".into());
    for definition in [
        amount,
        stage,
        field("tier", FieldType::Text),
        field("won_on", FieldType::Date),
        account_lookup,
    ] {
        admin
            .create_field("custom:deal", definition, context())
            .await
            .expect("deal field");
    }
    let mut total = field("total", FieldType::Formula);
    total.formula = Some("custom:amount * 2".into());
    total.formula_type = Some(FieldType::Number);
    admin
        .create_field("custom:deal", total, context())
        .await
        .expect("formula field");
    admin
        .create_object(object("follow_up", "Follow-up"), context())
        .await
        .expect("follow-up");
    let mut deal_lookup = field("deal", FieldType::Lookup);
    deal_lookup.reference_object = Some("custom:deal".into());
    let mut due = field("due", FieldType::Date);
    due.required = true;
    for definition in [deal_lookup, due] {
        admin
            .create_field("custom:follow_up", definition, context())
            .await
            .expect("follow-up field");
    }
    admin
        .set_object_permission(
            "custom:deal",
            role,
            AccessFlags {
                create: true,
                edit: true,
                ..AccessFlags::default()
            },
            context(),
        )
        .await
        .expect("deal access");

    // Automations are checked when they are saved.
    for (input, path, code) in [
        (
            json!({"name": "bad", "trigger": "create", "timing": "before_save",
                   "actions": [{"type": "update_record",
                                "values": [{"field": "custom:tier", "formula": "custom:amount"}]}]}),
            "actions[0].values[0].formula",
            "automations.formula_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "before_save",
                   "actions": [{"type": "update_record",
                                "values": [{"field": "custom:total", "formula": "1"}]}]}),
            "actions[0].values[0].field",
            "automations.field_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "before_save",
                   "actions": [{"type": "create_record", "object": "custom:follow_up",
                                "values": [{"field": "name", "formula": "'x'"}]}]}),
            "actions",
            "automations.actions_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "after_save",
                   "actions": [{"type": "create_record", "object": "custom:follow_up",
                                "values": [{"field": "name", "formula": "'x'"}]}]}),
            "actions[0].values",
            "automations.value_required",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "after_save",
                   "actions": [{"type": "update_related", "lookup": "custom:tier",
                                "values": [{"field": "name", "formula": "'x'"}]}]}),
            "actions[0].lookup",
            "automations.lookup_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "after_save",
                   "actions": [{"type": "create_record", "object": "custom:nope",
                                "values": [{"field": "name", "formula": "'x'"}]}]}),
            "actions[0].object",
            "automations.object_unknown",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "before_save",
                   "condition": "custom:amount",
                   "actions": [{"type": "update_record",
                                "values": [{"field": "custom:tier", "formula": "'x'"}]}]}),
            "condition",
            "automations.condition_invalid",
        ),
        (
            json!({"name": "Bad", "trigger": "create", "timing": "before_save",
                   "actions": [{"type": "update_record",
                                "values": [{"field": "custom:tier", "formula": "'x'"}]}]}),
            "name",
            "automations.name_invalid",
        ),
    ] {
        let found = issues(
            admin
                .create_automation("custom:deal", automation(input), context())
                .await
                .expect_err("invalid automation"),
        );
        assert_eq!((found[0].0.as_str(), found[0].1), (path, code), "{found:?}");
    }

    // Before save: the record's own fields, on create and on update.
    admin
        .create_automation(
            "custom:deal",
            automation(json!({
                "name": "set_tier", "trigger": "create_or_update", "timing": "before_save",
                "actions": [{"type": "update_record", "values": [
                    {"field": "custom:tier",
                     "formula": "IF(custom:amount >= 1000, 'large', 'small')"}]}]})),
            context(),
        )
        .await
        .expect("set tier");
    admin
        .create_automation(
            "custom:deal",
            automation(json!({
                "name": "stamp_won", "trigger": "update", "timing": "before_save",
                "condition": "AND(ISCHANGED(custom:stage), ISPICKVAL(custom:stage, 'won'))",
                "actions": [{"type": "update_record", "values": [
                    {"field": "custom:won_on", "formula": "TODAY()"}]}]})),
            context(),
        )
        .await
        .expect("stamp won");
    // After save: a follow-up for each new deal, and the account marked as a
    // customer once a deal is won.
    admin
        .create_automation(
            "custom:deal",
            automation(json!({
                "name": "follow_up", "trigger": "create", "timing": "after_save",
                "actions": [{"type": "create_record", "object": "custom:follow_up", "values": [
                    {"field": "name", "formula": "'Follow up ' & name"},
                    {"field": "custom:deal", "formula": "id"},
                    {"field": "custom:due", "formula": "TODAY() + 7"}]}]})),
            context(),
        )
        .await
        .expect("follow up");
    let customer = admin
        .create_automation(
            "custom:deal",
            automation(json!({
                "name": "mark_customer", "trigger": "update", "timing": "after_save",
                "condition": "ISPICKVAL(custom:stage, 'won')",
                "actions": [{"type": "update_related", "lookup": "custom:account", "values": [
                    {"field": "custom:status", "formula": "'customer'"}]}]})),
            context(),
        )
        .await
        .expect("mark customer");
    assert!(matches!(
        admin
            .create_automation(
                "custom:deal",
                automation(json!({
                    "name": "set_tier", "trigger": "create", "timing": "before_save",
                    "actions": [{"type": "update_record", "values": [
                        {"field": "custom:tier", "formula": "'x'"}]}]})),
                context(),
            )
            .await,
        Err(ObjectError::Conflict("automations.duplicate_name"))
    ));
    assert_eq!(
        admin
            .automations("custom:deal")
            .await
            .expect("list")
            .into_iter()
            .map(|automation| automation.name)
            .collect::<Vec<_>>(),
        ["follow_up", "mark_customer", "set_tier", "stamp_won"]
    );

    let acme = admin
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("account");
    let deal = admin
        .create_record(
            "custom:deal",
            &body(json!({"name": "Big deal", "custom:amount": 1500,
                         "custom:account": acme.public_id})),
            context(),
        )
        .await
        .expect("deal");
    assert_eq!(deal.values["custom:tier"], json!("large"));
    assert_eq!(
        deal.values["custom:total"],
        json!(3000.0),
        "formulas see it"
    );
    let today = chrono::Utc::now().date_naive();
    let follow_ups = admin
        .list_records("custom:follow_up", ListRecords::default())
        .await
        .expect("follow-ups")
        .records;
    assert_eq!(follow_ups.len(), 1);
    assert_eq!(follow_ups[0].name, "Follow up Big deal");
    assert_eq!(follow_ups[0].values["custom:deal"], json!(deal.public_id));
    assert_eq!(
        follow_ups[0].values["custom:due"],
        json!((today + chrono::Days::new(7))
            .format("%Y-%m-%d")
            .to_string())
    );

    // The member cannot see the account or create follow-ups; winning the
    // deal still updates the account, and the before-save change is kept.
    let won = member
        .update_record(
            "custom:deal",
            &deal.public_id,
            deal.version,
            &body(json!({"custom:stage": "won", "custom:amount": 10})),
            context(),
        )
        .await
        .expect("member wins the deal");
    assert_eq!(won.values["custom:tier"], json!("small"));
    assert_eq!(
        won.values["custom:won_on"],
        json!(today.format("%Y-%m-%d").to_string())
    );
    let account = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("account");
    assert_eq!(account.values["custom:status"], json!("customer"));
    assert_eq!(account.version, 2);
    // A second save changes nothing on the account, so it is not written.
    let again = member
        .update_record(
            "custom:deal",
            &deal.public_id,
            won.version,
            &body(json!({"custom:amount": 20})),
            context(),
        )
        .await
        .expect("save again");
    assert_eq!(again.values["custom:won_on"], won.values["custom:won_on"]);
    assert_eq!(
        admin
            .get_record("custom:account", &acme.public_id, false)
            .await
            .expect("account")
            .version,
        2
    );

    // A failure anywhere rolls the whole change back.
    let block: CreateValidationRule = serde_json::from_value(json!({
        "name": "no_blocked", "condition": "CONTAINS(name, 'blocked')",
        "message": "No blocked follow-ups"}))
    .expect("rule");
    admin
        .create_validation_rule("custom:follow_up", block, context())
        .await
        .expect("rule");
    let found = issues(
        member
            .create_record(
                "custom:deal",
                &body(json!({"name": "blocked deal", "custom:amount": 5})),
                context(),
            )
            .await
            .expect_err("blocked by the follow-up's rule"),
    );
    assert_eq!(found[0].0, "record");
    assert_eq!(found[0].1, "records.automation_failed");
    assert!(
        found[0].2.contains("follow_up") && found[0].2.contains("No blocked follow-ups"),
        "{found:?}"
    );
    assert_eq!(names(&admin, "custom:deal").await, ["Big deal"]);
    assert_eq!(
        names(&admin, "custom:follow_up").await,
        ["Follow up Big deal"]
    );

    // Inactive automations do not run.
    let follow_up = admin
        .automations("custom:deal")
        .await
        .expect("list")
        .into_iter()
        .find(|automation| automation.name == "follow_up")
        .expect("follow_up");
    let off: UpdateAutomation =
        serde_json::from_value(json!({"expected_version": follow_up.version, "active": false}))
            .expect("update");
    let off = admin
        .update_automation("custom:deal", "follow_up", off, context())
        .await
        .expect("deactivate");
    assert_eq!(off.version, 2);
    admin
        .create_record(
            "custom:deal",
            &body(json!({"name": "blocked but quiet", "custom:amount": 5})),
            context(),
        )
        .await
        .expect("no follow-up, no block");
    assert_eq!(names(&admin, "custom:follow_up").await.len(), 1);

    // Every run that acted is kept, newest first, and so is a run whose
    // change rolled back, with the reason.
    let runs = admin
        .automation_runs("custom:deal", "set_tier", None, 50)
        .await
        .expect("runs")
        .0;
    assert!(runs.len() >= 3, "{runs:?}");
    // set_tier acted in the blocked change too; its work was undone, and
    // its run says so.
    let undone: Vec<&AutomationRun> = runs
        .iter()
        .filter(|run| run.outcome == RunOutcome::Failed)
        .collect();
    assert_eq!(undone.len(), 1, "{runs:?}");
    assert!(undone[0]
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("rolled back")));
    assert_eq!(undone[0].saves, 0);
    assert!(runs.windows(2).all(|pair| pair[0].ran_at >= pair[1].ran_at));
    let (first, next) = admin
        .automation_runs("custom:deal", "set_tier", None, 1)
        .await
        .expect("first page");
    let (second, _) = admin
        .automation_runs("custom:deal", "set_tier", next.as_deref(), 1)
        .await
        .expect("second page");
    assert_eq!((first[0].id, second[0].id), (runs[0].id, runs[1].id));
    let follow_runs = admin
        .automation_runs("custom:deal", "follow_up", None, 50)
        .await
        .expect("runs")
        .0;
    let failed: Vec<&AutomationRun> = follow_runs
        .iter()
        .filter(|run| run.outcome == RunOutcome::Failed)
        .collect();
    assert_eq!(failed.len(), 1, "{follow_runs:?}");
    assert!(failed[0]
        .detail
        .as_deref()
        .is_some_and(|detail| detail.contains("No blocked follow-ups")));
    let succeeded: Vec<&AutomationRun> = follow_runs
        .iter()
        .filter(|run| run.outcome == RunOutcome::Succeeded)
        .collect();
    assert_eq!(succeeded.len(), 1);
    assert_eq!(
        (
            succeeded[0].record.as_str(),
            succeeded[0].event,
            succeeded[0].saves
        ),
        (deal.public_id.as_str(), SaveEvent::Create, 1)
    );
    assert!(matches!(
        admin
            .automation_runs("custom:deal", "set_tier", None, 0)
            .await,
        Err(ObjectError::Validation(_))
    ));

    // Owner changes run update automations; deleting and restoring run
    // automations of their own, after saving only.
    let found = issues(
        admin
            .create_automation(
                "custom:deal",
                automation(json!({
                    "name": "bad", "trigger": "delete", "timing": "before_save",
                    "actions": [{"type": "update_record", "values": [
                        {"field": "custom:tier", "formula": "'x'"}]}]})),
                context(),
            )
            .await
            .expect_err("before delete"),
    );
    assert_eq!(found[0].1, "automations.trigger_invalid");
    for definition in [
        json!({"name": "zz_reassigned", "trigger": "update", "timing": "before_save",
               "condition": "ISCHANGED(owner)",
               "actions": [{"type": "update_record", "values": [
                   {"field": "custom:tier", "formula": "'reassigned'"}]}]}),
        json!({"name": "on_delete", "trigger": "delete", "timing": "after_save",
               "actions": [{"type": "update_related", "lookup": "custom:account", "values": [
                   {"field": "custom:status", "formula": "'lost ' & name"}]}]}),
        json!({"name": "on_restore", "trigger": "undelete", "timing": "after_save",
               "actions": [{"type": "update_related", "lookup": "custom:account", "values": [
                   {"field": "custom:status", "formula": "'back'"}]}]}),
    ] {
        admin
            .create_automation("custom:deal", automation(definition), context())
            .await
            .expect("lifecycle automation");
    }
    let current = admin
        .get_record("custom:deal", &deal.public_id, false)
        .await
        .expect("deal");
    let moved = admin
        .change_owner(
            "custom:deal",
            &deal.public_id,
            member_id,
            current.version,
            context(),
        )
        .await
        .expect("transfer");
    assert_eq!(moved.owner_id, Some(member_id));
    assert_eq!(moved.values["custom:tier"], json!("reassigned"));
    let status = |record: Record| record.values["custom:status"].clone();
    let deleted = admin
        .delete_record("custom:deal", &deal.public_id, moved.version, context())
        .await
        .expect("delete");
    assert_eq!(
        status(
            admin
                .get_record("custom:account", &acme.public_id, false)
                .await
                .expect("account")
        ),
        json!("lost Big deal")
    );
    admin
        .undelete_record("custom:deal", &deal.public_id, deleted.version, context())
        .await
        .expect("restore");
    assert_eq!(
        status(
            admin
                .get_record("custom:account", &acme.public_id, false)
                .await
                .expect("account")
        ),
        json!("back")
    );
    let delete_runs = admin
        .automation_runs("custom:deal", "on_delete", None, 10)
        .await
        .expect("runs")
        .0;
    assert_eq!(delete_runs.len(), 1);
    assert_eq!(delete_runs[0].event, SaveEvent::Delete);
    for name in ["zz_reassigned", "on_delete", "on_restore"] {
        let version = admin
            .automations("custom:deal")
            .await
            .expect("list")
            .into_iter()
            .find(|automation| automation.name == name)
            .expect("automation")
            .version;
        admin
            .delete_automation("custom:deal", name, version, context())
            .await
            .expect("tidy up");
    }

    // Automations that keep causing each other stop at the depth limit.
    admin
        .create_object(object("node", "Node"), context())
        .await
        .expect("node");
    let mut parent = field("parent", FieldType::Lookup);
    parent.reference_object = Some("custom:node".into());
    admin
        .create_field("custom:node", parent, context())
        .await
        .expect("parent");
    admin
        .create_automation(
            "custom:node",
            automation(json!({
                "name": "spawn", "trigger": "create", "timing": "after_save",
                "actions": [{"type": "create_record", "object": "custom:node", "values": [
                    {"field": "name", "formula": "'child of ' & name"},
                    {"field": "custom:parent", "formula": "id"}]}]})),
            context(),
        )
        .await
        .expect("spawn");
    let found = issues(
        admin
            .create_record("custom:node", &body(json!({"name": "root"})), context())
            .await
            .expect_err("endless"),
    );
    assert_eq!(found[0].1, "records.automation_failed");
    assert!(found[0].2.contains("levels deep"), "{found:?}");
    assert!(names(&admin, "custom:node").await.is_empty());

    // Fields an active automation needs cannot be deleted, on its own
    // object or on the objects it writes.
    for (api_name, field_name) in [
        ("custom:deal", "custom:tier"),
        ("custom:account", "custom:status"),
    ] {
        let definition = admin
            .describe(api_name)
            .await
            .expect("describe")
            .0
            .fields
            .into_iter()
            .find(|field| field.api_name == field_name)
            .expect("field");
        assert!(
            matches!(
                admin
                    .delete_field(api_name, field_name, definition.version, context())
                    .await,
                Err(ObjectError::Conflict("objects.field_in_use"))
            ),
            "{field_name}"
        );
    }
    admin
        .delete_automation("custom:deal", "mark_customer", customer.version, context())
        .await
        .expect("delete automation");
    let status = admin
        .describe("custom:account")
        .await
        .expect("describe")
        .0
        .fields
        .into_iter()
        .find(|field| field.api_name == "custom:status")
        .expect("status");
    admin
        .delete_field("custom:account", "custom:status", status.version, context())
        .await
        .expect("no longer needed");

    // An object an active automation creates records of cannot be deleted.
    let follow_up_object = admin
        .describe("custom:follow_up")
        .await
        .expect("describe")
        .0;
    let on: UpdateAutomation =
        serde_json::from_value(json!({"expected_version": off.version, "active": true}))
            .expect("update");
    admin
        .update_automation("custom:deal", "follow_up", on, context())
        .await
        .expect("reactivate");
    assert!(matches!(
        admin
            .delete_object("custom:follow_up", follow_up_object.version, context())
            .await,
        Err(ObjectError::Conflict("objects.object_in_use"))
    ));

    // Actions can have conditions of their own, and an automation can
    // publish a workspace-defined platform event.
    admin
        .create_object(object("customer", "Customer"), context())
        .await
        .expect("customer");
    admin
        .create_field("custom:customer", field("tier", FieldType::Text), context())
        .await
        .expect("tier");
    admin
        .create_object(object("order", "Order"), context())
        .await
        .expect("order");
    let mut buyer = field("buyer", FieldType::Lookup);
    buyer.reference_object = Some("custom:customer".into());
    for definition in [field("total", FieldType::Number), buyer] {
        admin
            .create_field("custom:order", definition, context())
            .await
            .expect("order field");
    }
    for (input, path, code) in [
        (
            json!({"name": "bad", "trigger": "create", "timing": "after_save",
                   "actions": [{"type": "publish_event", "event": "Order-Placed",
                                "values": [{"field": "total", "formula": "custom:total"}]}]}),
            "actions[0].event",
            "automations.event_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "after_save",
                   "actions": [{"type": "publish_event", "event": "order_placed",
                                "values": [{"field": "Total", "formula": "custom:total"}]}]}),
            "actions[0].values[0].field",
            "automations.field_invalid",
        ),
        (
            json!({"name": "bad", "trigger": "create", "timing": "after_save",
                   "actions": [{"type": "update_related", "lookup": "custom:buyer",
                                "when": "custom:total",
                                "values": [{"field": "custom:tier", "formula": "'x'"}]}]}),
            "actions[0].when",
            "automations.condition_invalid",
        ),
    ] {
        let found = issues(
            admin
                .create_automation("custom:order", automation(input), context())
                .await
                .expect_err("invalid action"),
        );
        assert_eq!((found[0].0.as_str(), found[0].1), (path, code), "{found:?}");
    }
    admin
        .create_automation(
            "custom:order",
            automation(json!({
                "name": "route", "trigger": "create", "timing": "after_save",
                "actions": [
                    {"type": "update_related", "lookup": "custom:buyer",
                     "when": "custom:total >= 1000",
                     "values": [{"field": "custom:tier", "formula": "'gold'"}]},
                    {"type": "update_related", "lookup": "custom:buyer",
                     "when": "custom:total < 1000",
                     "values": [{"field": "custom:tier", "formula": "'standard'"}]},
                    {"type": "publish_event", "event": "order_placed",
                     "values": [{"field": "total", "formula": "custom:total"},
                                {"field": "big", "formula": "custom:total >= 1000"}]}]})),
            context(),
        )
        .await
        .expect("route");
    let customer = admin
        .create_record("custom:customer", &body(json!({"name": "Ada"})), context())
        .await
        .expect("customer");
    for (total, tier) in [(50, "standard"), (5000, "gold")] {
        admin
            .create_record(
                "custom:order",
                &body(
                    json!({"name": format!("Order {total}"), "custom:total": total,
                             "custom:buyer": customer.public_id}),
                ),
                context(),
            )
            .await
            .expect("order");
        assert_eq!(
            admin
                .get_record("custom:customer", &customer.public_id, false)
                .await
                .expect("customer")
                .values["custom:tier"],
            json!(tier)
        );
    }
    let route_runs = admin
        .automation_runs("custom:order", "route", None, 10)
        .await
        .expect("runs")
        .0;
    assert_eq!(route_runs.len(), 2);
    assert!(
        route_runs.iter().all(|run| run.saves == 1),
        "one branch ran each time; events are not saves"
    );

    // Members cannot see or change automations.
    assert!(matches!(
        member.automations("custom:deal").await,
        Err(ObjectError::PermissionDenied)
    ));
}
