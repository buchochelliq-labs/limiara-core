//! Saved reports on every backend (ADR 0027): definitions checked when
//! saved, seen by their owner or the workspace, and run as the viewer.

use super::{body, caller, codes, context, object, service};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn report(name: &str, query: &str) -> CreateReport {
    serde_json::from_value(json!({"name": name, "query": query})).expect("report")
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin_id = Uuid::now_v7();
    let admin = service(
        &store,
        caller(
            tenant,
            admin_id,
            &[
                OBJECTS_MANAGE_PERMISSION,
                RECORDS_MODIFY_ALL_PERMISSION,
                REPORTS_MANAGE_PERMISSION,
            ],
            &[],
        ),
    );
    let role = Uuid::now_v7();
    let ana_id = Uuid::now_v7();
    let ana = service(&store, caller(tenant, ana_id, &[], &[role]));
    let ben = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));

    let mut deal = object("deal", "Deal");
    deal.sharing_model = Some(SharingModel::Private);
    admin.create_object(deal, context()).await.expect("deal");
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
        .expect("access");
    for (who, name) in [
        (&ana, "Ana's deal"),
        (&ana, "Another"),
        (&ben, "Ben's deal"),
    ] {
        who.create_record("custom:deal", &body(json!({"name": name})), context())
            .await
            .expect("deal");
    }

    // A report's query must run for whoever saves it.
    let refused = ana
        .create_report(report("Broken", "SELECT nope FROM custom:deal"), context())
        .await
        .expect_err("bad query");
    assert!(codes(&refused)
        .iter()
        .all(|code| code.starts_with("query.")));
    let refused = ana
        .create_report(report(" ", "SELECT name FROM custom:deal"), context())
        .await
        .expect_err("no name");
    assert_eq!(codes(&refused), ["reports.name_invalid"]);
    let mut chart: CreateReport = serde_json::from_value(json!({
        "name": "By owner",
        "query": "SELECT owner, COUNT() FROM custom:deal GROUP BY owner",
        "chart": "bar",
        "label_column": "owner",
        "value_column": "owner",
    }))
    .expect("chart");
    let refused = ana
        .create_report(chart.clone(), context())
        .await
        .expect_err("text plotted");
    assert_eq!(codes(&refused), ["reports.value_column_invalid"]);
    chart.value_column = Some("count()".into());
    // Sharing a report with the workspace needs reports.manage.
    chart.visibility = ReportVisibility::Workspace;
    assert!(matches!(
        ana.create_report(chart.clone(), context()).await,
        Err(ObjectError::PermissionDenied)
    ));
    chart.visibility = ReportVisibility::Private;
    let mine = ana
        .create_report(chart, context())
        .await
        .expect("ana's report");
    assert_eq!(mine.chart, ReportChart::Bar);

    // Private reports are their owner's; workspace reports everyone's.
    let shared = admin
        .create_report(
            serde_json::from_value(json!({
                "name": "All deals",
                "query": "SELECT name FROM custom:deal ORDER BY name",
                "visibility": "workspace",
            }))
            .expect("shared"),
            context(),
        )
        .await
        .expect("shared report");
    let names = |reports: Vec<Report>| -> Vec<String> {
        reports.into_iter().map(|report| report.name).collect()
    };
    assert_eq!(
        names(ana.list_reports().await.expect("ana")),
        ["All deals", "By owner"]
    );
    assert_eq!(names(ben.list_reports().await.expect("ben")), ["All deals"]);
    assert_eq!(
        names(admin.list_reports().await.expect("admin")),
        ["All deals", "By owner"],
        "reports.manage sees private reports too"
    );
    assert!(matches!(
        ben.get_report(mine.id).await,
        Err(ObjectError::NotFound)
    ));

    // A shared report runs as its viewer: each sees only their rows.
    let rows = |result: QueryResult| -> Vec<serde_json::Value> {
        result
            .rows
            .into_iter()
            .map(|row| row["name"].clone())
            .collect()
    };
    let (_, result) = ana
        .run_report(shared.id, None, None)
        .await
        .expect("ana runs");
    assert_eq!(rows(result), [json!("Ana's deal"), json!("Another")]);
    let (_, result) = ben
        .run_report(shared.id, None, None)
        .await
        .expect("ben runs");
    assert_eq!(rows(result), [json!("Ben's deal")]);
    let (_, result) = admin
        .run_report(shared.id, None, None)
        .await
        .expect("admin runs");
    assert_eq!(result.rows.len(), 3);

    // Only the owner or reports.manage changes a report, at its version.
    let rename: UpdateReport =
        serde_json::from_value(json!({"expected_version": shared.version, "name": "Deals"}))
            .expect("rename");
    // Members see a shared report but only its owner or a manager may
    // change it, and the API tells them which.
    assert!(!ben.can_edit_report(&shared));
    assert!(admin.can_edit_report(&shared));
    assert!(matches!(
        ben.update_report(shared.id, rename.clone(), context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    let renamed = admin
        .update_report(shared.id, rename.clone(), context())
        .await
        .expect("rename");
    assert_eq!((renamed.name.as_str(), renamed.version), ("Deals", 2));
    assert!(matches!(
        admin.update_report(shared.id, rename, context()).await,
        Err(ObjectError::VersionConflict)
    ));
    let to_table: UpdateReport = serde_json::from_value(json!({
        "expected_version": mine.version,
        "chart": "table",
    }))
    .expect("table");
    let table = ana
        .update_report(mine.id, to_table, context())
        .await
        .expect("as a table");
    assert_eq!((table.label_column, table.value_column), (None, None));
    assert!(matches!(
        ben.delete_report(shared.id, renamed.version, context())
            .await,
        Err(ObjectError::PermissionDenied)
    ));
    ana.delete_report(mine.id, table.version, context())
        .await
        .expect("delete own");
    assert_eq!(names(ana.list_reports().await.expect("ana")), ["Deals"]);
    let _ = admin_id;
}

/// Concurrent creates cannot take a workspace past `reports::REPORTS_MAX`.
pub async fn limit(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(
            tenant,
            Uuid::now_v7(),
            &[
                OBJECTS_MANAGE_PERMISSION,
                RECORDS_MODIFY_ALL_PERMISSION,
                REPORTS_MANAGE_PERMISSION,
            ],
            &[],
        ),
    );
    admin
        .create_object(object("tally", "Tally"), context())
        .await
        .expect("object");
    let query = "SELECT name FROM custom:tally";
    for index in 1..reports::REPORTS_MAX {
        admin
            .create_report(report(&format!("Report {index}"), query), context())
            .await
            .expect("under the limit");
    }
    // Eight creates race for the last place, each on its own task.
    let racers: Vec<_> = (0..8)
        .map(|index| {
            let admin = admin.clone();
            tokio::spawn(async move {
                admin
                    .create_report(report(&format!("Last {index}"), query), context())
                    .await
            })
        })
        .collect();
    let mut made = 0;
    for racer in racers {
        match racer.await.expect("racer") {
            Ok(_) => made += 1,
            Err(error) => assert!(
                matches!(error, ObjectError::Conflict("reports.limit_reached")),
                "{error:?}"
            ),
        }
    }
    assert_eq!(made, 1, "exactly one create fills the last place");
    assert_eq!(
        admin.list_reports().await.expect("list").len(),
        reports::REPORTS_MAX
    );
}
