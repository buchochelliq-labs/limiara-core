//! The Sales plugin's objects on every backend (ADR 0037 slice 2): installing
//! it creates Lead and Opportunity on their paths, with the stages' outcomes
//! and weights kept, and records move along the path through ordinary
//! updates, which the engine validates like any other. Probability follows
//! the stage through the plugin's starting automation, unless a save sets it.

use super::{body, caller, codes, context, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use plugin_contract::Manifest;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const PLUGIN: &str = "platform.sales";
const LEAD: &str = "platform.sales:lead";
const OPPORTUNITY: &str = "platform.sales:opportunity";
const STAGE: &str = "platform.sales:stage";
const CLOSE_DATE: &str = "platform.sales:close_date";
const AMOUNT: &str = "platform.sales:amount";
const PROBABILITY: &str = "platform.sales:probability";

fn probability(record: &Record) -> Option<f64> {
    record
        .values
        .get(PROBABILITY)
        .and_then(|value| value.as_f64())
}

pub fn sales() -> Manifest {
    Manifest::parse_toml(include_str!("../../../../plugins/sales/plugin.toml"))
        .expect("the Sales manifest parses")
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let member = Uuid::now_v7();
    let admin = service(&store, caller(tenant, member, &ADMIN_PERMISSIONS, &[]));
    let installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, PLUGIN),
    );

    // --- An automation the engine refuses fails the whole install ----------
    let mut broken = sales();
    broken.objects[1].automations[0].set[0].formula =
        "PICKLISTWEIGHT(platform.sales:amount)".into();
    let refused = installer
        .install_plugin_objects(&broken, context())
        .await
        .expect_err("a formula the engine refuses");
    let ObjectError::Validation(issues) = &refused else {
        panic!("expected a validation error, got {refused:?}");
    };
    assert!(
        issues.iter().all(|issue| issue
            .field
            .starts_with("opportunity/automations/probability_from_stage/")),
        "{issues:?}"
    );
    assert!(matches!(
        admin.describe(OPPORTUNITY).await,
        Err(ObjectError::NotFound)
    ));

    // --- Install: Lead and Opportunity, each on its path --------------------
    let installed = installer
        .install_plugin_objects(&sales(), context())
        .await
        .expect("install");
    assert_eq!(installed.created_objects, vec![LEAD, OPPORTUNITY]);
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");

    let (lead, _) = admin.describe(LEAD).await.expect("lead");
    assert_eq!(lead.path_field.as_deref(), Some("platform.sales:status"));
    assert_eq!(lead.sharing_model, SharingModel::Private);
    let (opportunity, _) = admin.describe(OPPORTUNITY).await.expect("opportunity");
    assert_eq!(opportunity.path_field.as_deref(), Some(STAGE));
    let stages: Vec<_> = opportunity
        .field(STAGE)
        .expect("stage")
        .options
        .iter()
        .map(|option| (option.value.as_str(), option.outcome, option.weight))
        .collect();
    assert_eq!(
        stages,
        [
            ("prospecting", OptionOutcome::Open, Some(10)),
            ("qualification", OptionOutcome::Open, Some(20)),
            ("needs_analysis", OptionOutcome::Open, Some(40)),
            ("proposal", OptionOutcome::Open, Some(60)),
            ("negotiation", OptionOutcome::Open, Some(80)),
            ("closed_won", OptionOutcome::Positive, Some(100)),
            ("closed_lost", OptionOutcome::Negative, Some(0)),
        ]
    );

    // --- The starting automation is a workspace automation ------------------
    let automations = admin.automations(OPPORTUNITY).await.expect("automations");
    assert_eq!(
        automations
            .iter()
            .map(|automation| (automation.name.as_str(), automation.active))
            .collect::<Vec<_>>(),
        [("probability_from_stage", true)]
    );

    // --- A deal starts on the default stage and moves along the path --------
    let refused = admin
        .create_record(
            OPPORTUNITY,
            &body(json!({"name": "Acme renewal"})),
            context(),
        )
        .await
        .expect_err("an opportunity needs a close date");
    assert!(!codes(&refused).is_empty(), "{refused:?}");
    let deal = admin
        .create_record(
            OPPORTUNITY,
            &body(json!({"name": "Acme renewal", CLOSE_DATE: "2026-12-31", AMOUNT: "12000.00"})),
            context(),
        )
        .await
        .expect("an opportunity");
    assert_eq!(deal.values.get(STAGE), Some(&json!("prospecting")));
    assert_eq!(
        probability(&deal),
        Some(10.0),
        "a new deal takes its stage's weight"
    );
    let moved = admin
        .update_record(
            OPPORTUNITY,
            &deal.public_id,
            deal.version,
            &body(json!({STAGE: "negotiation"})),
            context(),
        )
        .await
        .expect("the deal moves");
    assert_eq!(moved.values.get(STAGE), Some(&json!("negotiation")));
    assert_eq!(
        probability(&moved),
        Some(80.0),
        "a stage change sets it again"
    );
    let refused = admin
        .update_record(
            OPPORTUNITY,
            &moved.public_id,
            moved.version,
            &body(json!({STAGE: "signed"})),
            context(),
        )
        .await
        .expect_err("only the path's stages are values");
    assert!(!codes(&refused).is_empty(), "{refused:?}");
    // A member's own probability stands, in the save that sets it and after.
    let judged = admin
        .update_record(
            OPPORTUNITY,
            &moved.public_id,
            moved.version,
            &body(json!({STAGE: "proposal", PROBABILITY: 75})),
            context(),
        )
        .await
        .expect("a member sets the probability");
    assert_eq!(probability(&judged), Some(75.0));
    let renamed = admin
        .update_record(
            OPPORTUNITY,
            &judged.public_id,
            judged.version,
            &body(json!({"name": "Acme renewal 2027"})),
            context(),
        )
        .await
        .expect("an edit that leaves the stage");
    assert_eq!(probability(&renamed), Some(75.0));
    let won = admin
        .update_record(
            OPPORTUNITY,
            &renamed.public_id,
            renamed.version,
            &body(json!({STAGE: "closed_won"})),
            context(),
        )
        .await
        .expect("the deal closes");
    assert_eq!(won.values.get(STAGE), Some(&json!("closed_won")));
    assert_eq!(probability(&won), Some(100.0));
    let given = admin
        .create_record(
            OPPORTUNITY,
            &body(json!({"name": "Initech pilot", CLOSE_DATE: "2027-03-31", PROBABILITY: 5})),
            context(),
        )
        .await
        .expect("a deal given its own probability");
    assert_eq!(probability(&given), Some(5.0));

    // --- The automation is the workspace's: a deletion sticks ---------------
    admin
        .delete_automation(OPPORTUNITY, "probability_from_stage", 1, context())
        .await
        .expect("administrators may delete it");
    let again = installer
        .install_plugin_objects(&sales(), context())
        .await
        .expect("install again, as an upgrade does");
    assert!(again.created_objects.is_empty(), "{again:?}");
    assert!(
        admin
            .automations(OPPORTUNITY)
            .await
            .expect("automations")
            .is_empty(),
        "an upgrade never adds a starting automation back"
    );
    let moved = admin
        .update_record(
            OPPORTUNITY,
            &given.public_id,
            given.version,
            &body(json!({STAGE: "negotiation"})),
            context(),
        )
        .await
        .expect("a move without the automation");
    assert_eq!(probability(&moved), Some(5.0));

    // --- A lead starts New ---------------------------------------------------
    let prospect = admin
        .create_record(
            LEAD,
            &body(json!({"name": "Ada Lovelace", "platform.sales:company": "Analytical"})),
            context(),
        )
        .await
        .expect("a lead");
    assert_eq!(
        prospect.values.get("platform.sales:status"),
        Some(&json!("new"))
    );
}
