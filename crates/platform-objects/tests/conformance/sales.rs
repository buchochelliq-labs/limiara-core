//! The Sales plugin's objects on every backend (ADR 0037 slice 2): installing
//! it creates Lead and Opportunity on their paths, with the stages' outcomes
//! and weights kept, and records move along the path through ordinary
//! updates, which the engine validates like any other.

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
    let won = admin
        .update_record(
            OPPORTUNITY,
            &moved.public_id,
            moved.version,
            &body(json!({STAGE: "closed_won"})),
            context(),
        )
        .await
        .expect("the deal closes");
    assert_eq!(won.values.get(STAGE), Some(&json!("closed_won")));

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
