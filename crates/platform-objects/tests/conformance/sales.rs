//! The Sales plugin's objects on every backend (ADR 0037 slice 2): installing
//! it creates Lead and Opportunity on their paths, with the stages' outcomes
//! and weights kept, and records move along the path through ordinary
//! updates, which the engine validates like any other. Its reports (ADR
//! 0037 §5) are added once the plugin is enabled, upgraded only while
//! nobody changed them, never added back once deleted, and hidden while the
//! plugin is not enabled.

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

    // --- Reports: added once the objects are in service --------------------
    let added = installer
        .put_plugin_reports(&sales(), context())
        .await
        .expect("the plugin's reports");
    assert!(added.skipped.is_empty(), "{:?}", added.skipped);
    assert_eq!(
        added.created,
        [
            "open_pipeline_by_stage",
            "open_pipeline_by_forecast_category",
            "won_this_year",
            "leads_by_source"
        ]
    );
    assert!(added.skipped.is_empty(), "{:?}", added.skipped);
    let reports = admin.list_reports().await.expect("reports");
    let names: Vec<_> = reports.iter().map(|report| report.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Leads by source",
            "Open pipeline by forecast category",
            "Open pipeline by stage",
            "Won this year"
        ]
    );
    for report in &reports {
        assert_eq!(report.owner_plugin.as_deref(), Some(PLUGIN));
        assert_eq!(report.visibility, ReportVisibility::Workspace);
        admin
            .run_report(report.id, None, None)
            .await
            .unwrap_or_else(|error| panic!("{} runs: {error:?}", report.name));
    }
    let report = |name: &str| {
        reports
            .iter()
            .find(|report| report.plugin_report.as_deref() == Some(name))
            .cloned()
            .expect("report")
    };
    let (_, won) = admin
        .run_report(report("won_this_year").id, None, None)
        .await
        .expect("won this year");
    assert_eq!(
        won.rows[0].get("amount").and_then(|value| value.as_f64()),
        Some(12000.0)
    );
    // Any member sees them: they are the workspace's reports.
    let reader = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));
    assert_eq!(reader.list_reports().await.expect("reports").len(), 4);
    // Enabling again changes nothing.
    let again = installer
        .put_plugin_reports(&sales(), context())
        .await
        .expect("idempotent");
    assert_eq!(again, Default::default());

    // --- An upgrade changes only the reports nobody changed -----------------
    let edited = admin
        .update_report(
            report("leads_by_source").id,
            serde_json::from_value(
                json!({"expected_version": 1, "name": "Where our leads come from"}),
            )
            .expect("change"),
            context(),
        )
        .await
        .expect("an administrator renames a plugin report");
    let mut upgrade = sales();
    for decl in &mut upgrade.reports {
        decl.label = format!("{} (v2)", decl.label);
    }
    let upgraded = installer
        .put_plugin_reports(&upgrade, context())
        .await
        .expect("upgrade");
    assert_eq!(upgraded.updated.len(), 3, "{upgraded:?}");
    assert!(!upgraded.updated.contains(&"leads_by_source".to_owned()));
    let kept = admin.get_report(edited.id).await.expect("kept");
    assert_eq!(kept.name, "Where our leads come from");
    assert_eq!(
        admin
            .get_report(report("won_this_year").id)
            .await
            .expect("won")
            .name,
        "Won this year (v2)"
    );

    // --- A deleted report stays deleted -------------------------------------
    let stage_report = admin
        .get_report(report("open_pipeline_by_stage").id)
        .await
        .expect("by stage");
    admin
        .delete_report(stage_report.id, stage_report.version, context())
        .await
        .expect("delete a plugin report");
    let after = installer
        .put_plugin_reports(&sales(), context())
        .await
        .expect("enable again");
    assert!(
        !after.created.contains(&"open_pipeline_by_stage".to_owned()),
        "{after:?}"
    );
    assert_eq!(admin.list_reports().await.expect("reports").len(), 3);

    // --- Hidden while the plugin is not enabled ------------------------------
    installer
        .suspend_plugin_objects(PLUGIN, context())
        .await
        .expect("suspend");
    assert!(admin.list_reports().await.expect("reports").is_empty());
    assert!(matches!(
        admin.get_report(edited.id).await,
        Err(ObjectError::NotFound)
    ));
    installer
        .resume_plugin_objects(PLUGIN, context())
        .await
        .expect("resume");
    assert_eq!(admin.list_reports().await.expect("reports").len(), 3);

    // --- Reports that cannot be added are skipped, not fatal ----------------
    let mut odd = sales();
    odd.reports.truncate(1);
    odd.reports[0].name = "foreign".into();
    odd.reports[0].query = "SELECT name FROM platform.activities:task".into();
    odd.reports[0].chart = plugin_contract::ReportChartDecl::Table;
    odd.reports[0].label_column = None;
    odd.reports[0].value_column = None;
    let mut broken = odd.reports[0].clone();
    broken.name = "broken".into();
    broken.query = "SELECT platform.sales:nothing FROM platform.sales:lead".into();
    odd.reports.push(broken);
    let skipped = installer
        .put_plugin_reports(&odd, context())
        .await
        .expect("skips, not fails");
    assert_eq!(
        skipped.skipped,
        [
            (
                "foreign".to_owned(),
                "reports.plugin_report_foreign_object".to_owned()
            ),
            ("broken".to_owned(), "query.field_unknown".to_owned()),
        ]
    );
    assert!(skipped.created.is_empty());

    // --- A lookup may not lead a report outside its dependencies -----------
    let dependency = |id: &str| json!({id: {"version": ">=0.1.0"}});
    let plugin = |id: &str, dependencies: serde_json::Value, objects: serde_json::Value| {
        let mut manifest = sales();
        manifest.id = id.into();
        manifest.dependencies = serde_json::from_value(dependencies).expect("dependencies");
        manifest.objects = serde_json::from_value(objects).expect("objects");
        manifest.reports = Vec::new();
        manifest
    };
    let third = plugin(
        "example.third",
        json!({}),
        json!([{"name": "region", "label": "Region", "plural_label": "Regions", "key_prefix": "rgn"}]),
    );
    let middle = plugin(
        "example.middle",
        dependency("example.third"),
        json!([{"name": "account", "label": "Account", "plural_label": "Accounts", "key_prefix": "acc",
                "fields": [{"name": "region", "label": "Region", "field_type": "lookup",
                            "reference_object": "example.third:region"}]}]),
    );
    for manifest in [&third, &middle] {
        let installer = service(
            &store,
            ObjectCaller::plugin_installer(tenant, member, manifest.id.clone()),
        );
        installer
            .install_plugin_objects(manifest, context())
            .await
            .expect("install");
        installer
            .resume_plugin_objects(&manifest.id, context())
            .await
            .expect("resume");
    }
    let mut reporting = plugin("example.reporting", dependency("example.middle"), json!([]));
    reporting.reports = vec![plugin_contract::ReportDecl {
        name: "accounts_by_region".into(),
        label: "Accounts by region".into(),
        description: None,
        query: "SELECT name, example.middle:region.name FROM example.middle:account".into(),
        chart: plugin_contract::ReportChartDecl::Table,
        label_column: None,
        value_column: None,
    }];
    let reporting_installer = service(
        &store,
        ObjectCaller::plugin_installer(tenant, member, "example.reporting"),
    );
    let refused = reporting_installer
        .put_plugin_reports(&reporting, context())
        .await
        .expect("skips, not fails");
    assert_eq!(
        refused.skipped,
        [(
            "accounts_by_region".to_owned(),
            "reports.plugin_report_foreign_object".to_owned()
        )],
        "a lookup into a plugin the report does not depend on"
    );
    reporting.dependencies = serde_json::from_value(
        json!({"example.middle": {"version": ">=0.1.0"}, "example.third": {"version": ">=0.1.0"}}),
    )
    .expect("dependencies");
    let allowed = reporting_installer
        .put_plugin_reports(&reporting, context())
        .await
        .expect("allowed once declared");
    assert_eq!(allowed.created, ["accounts_by_region"]);

    // --- Only the plugin's installer adds them ------------------------------
    assert!(matches!(
        admin.put_plugin_reports(&sales(), context()).await,
        Err(ObjectError::PermissionDenied)
    ));

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
