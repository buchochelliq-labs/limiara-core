//! Reports a plugin declares (ADR 0037 §5).
//!
//! A plugin's manifest may declare saved reports (`[[reports]]`, see
//! `plugin_contract::reports`). When a workspace enables the plugin, the
//! kernel calls [`ObjectService::put_plugin_reports`] as the plugin's
//! installer, once its objects are in service:
//!
//! - **Added as workspace reports.** Each declared report becomes a report
//!   every member sees (`visibility = workspace`), owned by the member who
//!   enabled the plugin and naming the plugin as `owner_plugin`. Its query
//!   is checked against the workspace's definitions, inside the engine, as
//!   an administrator's report is; only its columns are used.
//! - **Upgrades respect the workspace.** A report still at the version the
//!   plugin last installed (`plugin_version`) is brought in line with the
//!   manifest. One the workspace changed is left alone, and one it deleted
//!   is never added again. Reports a new version no longer declares are
//!   kept.
//! - **A report that cannot be added is skipped.** A query that reads
//!   another plugin's objects, directly or through a lookup, or that the
//!   engine refuses, is skipped and
//!   audited as `reports.plugin_report.skipped` with its reason; enabling
//!   the plugin goes ahead.
//! - **Suspended with the plugin.** While the plugin is not enabled, its
//!   reports are hidden (`suspended`), and they come back unchanged.

use crate::access::ObjectAccess;
use crate::events::Change;
use crate::reports::{
    check_columns, query_issues, tidy_report, Report, ReportChart, ReportVisibility,
};
use crate::service::{MutationContext, ObjectService};
use crate::store::ObjectTx;
use crate::{ObjectError, QueryRequest};
use chrono::Utc;
use platform_audit::AuditEvent;
use plugin_contract::{Manifest, ReportChartDecl, ReportDecl};
use serde::Serialize;
use serde_json::json;
use uuid::Uuid;

/// Audited when a declared report cannot be added or upgraded.
pub const PLUGIN_REPORT_SKIPPED: &str = "reports.plugin_report.skipped";
/// A report's query reads an object, directly or through a lookup, that is
/// neither the plugin's nor one of its dependencies'.
pub const PLUGIN_REPORT_FOREIGN: &str = "reports.plugin_report_foreign_object";

/// What adding a plugin's reports changed, by manifest name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PluginReportsInstalled {
    pub created: Vec<String>,
    pub updated: Vec<String>,
    /// `(name, reason code)` for each report skipped.
    pub skipped: Vec<(String, String)>,
}

fn chart(decl: ReportChartDecl) -> ReportChart {
    match decl {
        ReportChartDecl::Table => ReportChart::Table,
        ReportChartDecl::Bar => ReportChart::Bar,
        ReportChartDecl::Line => ReportChart::Line,
        ReportChartDecl::Pie => ReportChart::Pie,
        ReportChartDecl::Metric => ReportChart::Metric,
    }
}

/// Whether two reports show the same thing, ignoring who changed them when.
fn same_content(a: &Report, b: &Report) -> bool {
    a.name == b.name
        && a.description == b.description
        && a.query == b.query
        && a.chart == b.chart
        && a.label_column == b.label_column
        && a.value_column == b.value_column
        && a.visibility == b.visibility
}

impl ObjectService {
    /// Adds or upgrades the reports `manifest` declares in the caller's
    /// workspace and brings the plugin's reports back into service. Only a
    /// plugin installer for `manifest.id` may. Idempotent.
    pub async fn put_plugin_reports(
        &self,
        manifest: &Manifest,
        context: MutationContext,
    ) -> Result<PluginReportsInstalled, ObjectError> {
        if self.caller().installer() != Some(manifest.id.as_str()) {
            return Err(ObjectError::PermissionDenied);
        }
        let plugin_id = manifest.id.as_str();
        plugin_contract::reports::validate_reports(&manifest.reports).map_err(|issue| {
            ObjectError::invalid(
                format!("reports/{}", issue.report),
                crate::plugin_objects::PLUGIN_OBJECTS_INVALID,
                issue.reason,
            )
        })?;
        let mut installed = PluginReportsInstalled::default();
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        tx.set_plugin_reports_suspended(plugin_id, false).await?;
        let existing: Vec<Report> = tx
            .reports()
            .await?
            .into_iter()
            .filter(|report| report.owner_plugin.as_deref() == Some(plugin_id))
            .collect();
        let mut count = tx.reports().await?.len();
        let access = ObjectAccess::reading_everything();
        for decl in &manifest.reports {
            if tx.plugin_report_removed(plugin_id, &decl.name).await? {
                continue;
            }
            let current = existing
                .iter()
                .find(|report| report.plugin_report.as_deref() == Some(decl.name.as_str()));
            // The workspace changed it since the plugin last did: theirs now.
            if current.is_some_and(|report| Some(report.version) != report.plugin_version) {
                continue;
            }
            let mut report = self.declared_report(plugin_id, decl, current);
            if current.is_some_and(|current| same_content(current, &report)) {
                continue;
            }
            if current.is_none() && count >= crate::reports::REPORTS_MAX {
                self.skip_plugin_report(
                    tx.as_mut(),
                    plugin_id,
                    decl,
                    "reports.limit_reached",
                    context,
                )
                .await?;
                installed
                    .skipped
                    .push((decl.name.clone(), "reports.limit_reached".into()));
                continue;
            }
            if let Err(reason) = self
                .check_plugin_report(tx.as_mut(), manifest, &access, &mut report)
                .await?
            {
                self.skip_plugin_report(tx.as_mut(), plugin_id, decl, reason, context)
                    .await?;
                installed.skipped.push((decl.name.clone(), reason.into()));
                continue;
            }
            match current {
                None => {
                    tx.insert_report(&report).await?;
                    self.record_change(tx.as_mut(), context, Change::ReportCreated(&report))
                        .await?;
                    count += 1;
                    installed.created.push(decl.name.clone());
                }
                Some(current) => {
                    if !tx.update_report(&report, current.version).await? {
                        return Err(ObjectError::VersionConflict);
                    }
                    self.record_change(tx.as_mut(), context, Change::ReportUpdated(&report))
                        .await?;
                    installed.updated.push(decl.name.clone());
                }
            }
        }
        tx.commit().await?;
        Ok(installed)
    }

    /// The report a declaration describes: new, or the next version of the
    /// plugin's current one.
    fn declared_report(
        &self,
        plugin_id: &str,
        decl: &ReportDecl,
        current: Option<&Report>,
    ) -> Report {
        let now = Utc::now();
        let actor = Some(self.caller().user_id());
        let version = current.map_or(1, |report| report.version + 1);
        Report {
            id: current.map_or_else(Uuid::now_v7, |report| report.id),
            name: decl.label.clone(),
            description: decl.description.clone(),
            query: decl.query.clone(),
            chart: chart(decl.chart),
            label_column: decl.label_column.clone(),
            value_column: decl.value_column.clone(),
            visibility: ReportVisibility::Workspace,
            owner_id: current.map_or(self.caller().user_id(), |report| report.owner_id),
            version,
            created_at: current.map_or(now, |report| report.created_at),
            created_by: current.map_or(actor, |report| report.created_by),
            updated_at: now,
            updated_by: actor,
            owner_plugin: Some(plugin_id.to_owned()),
            plugin_report: Some(decl.name.clone()),
            plugin_version: Some(version),
            suspended: false,
        }
    }

    /// Checks a declared report as an administrator's would be checked,
    /// with its query reading only the plugin's and its dependencies'
    /// objects. `Err` carries the reason it is skipped.
    async fn check_plugin_report(
        &self,
        tx: &mut dyn ObjectTx,
        manifest: &Manifest,
        access: &ObjectAccess,
        report: &mut Report,
    ) -> Result<Result<(), &'static str>, ObjectError> {
        let first_code = |error: ObjectError| match error {
            ObjectError::Validation(issues) => {
                Ok(issues.first().map_or("reports.invalid", |issue| issue.code))
            }
            other => Err(other),
        };
        if let Err(error) = tidy_report(report) {
            return first_code(error).map(Err);
        }
        let Ok(query) = crate::query::parse::parse(&report.query) else {
            return Ok(Err("query.syntax"));
        };
        // Every object the query reads, through its lookups too, must be the
        // plugin's or a dependency's.
        let allowed = |api_name: &str| {
            api_name.split_once(':').is_some_and(|(namespace, _)| {
                namespace == manifest.id || manifest.dependencies.contains_key(namespace)
            })
        };
        let Some(base) = query.from.segments.first() else {
            return Ok(Err("query.object_unknown"));
        };
        if !allowed(base) {
            return Ok(Err(PLUGIN_REPORT_FOREIGN));
        }
        let Some(base) = tx.object_by_api_name(base).await? else {
            return Ok(Err("query.object_unknown"));
        };
        for path in crate::query::every_path(&query) {
            let mut object = base.clone();
            for segment in &path.segments[..path.segments.len().saturating_sub(1)] {
                let Some(target_id) = object
                    .field(segment)
                    .filter(|field| field.field_type.is_reference())
                    .and_then(|field| field.reference_object_id)
                else {
                    break;
                };
                let Some(target) = tx.object_by_id(target_id).await? else {
                    break;
                };
                if !allowed(&target.api_name) {
                    return Ok(Err(PLUGIN_REPORT_FOREIGN));
                }
                object = target;
            }
        }
        let columns = match self
            .query_in(
                tx,
                access,
                &QueryRequest {
                    q: report.query.clone(),
                    cursor: None,
                    page_size: Some(1),
                    include_deleted: false,
                    include_archived: false,
                },
            )
            .await
        {
            Ok(result) => result.columns,
            Err(error) => return first_code(query_issues(error)).map(Err),
        };
        match check_columns(report, &columns) {
            Ok(()) => Ok(Ok(())),
            Err(error) => first_code(error).map(Err),
        }
    }

    async fn skip_plugin_report(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        decl: &ReportDecl,
        reason: &str,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        tracing::warn!(plugin_id, report = %decl.name, reason, "plugin report skipped");
        let evidence = self.evidence(context);
        let audit = AuditEvent::with_context(
            Some(evidence.tenant_id),
            evidence.actor_id,
            evidence.correlation_id,
            evidence.causation_id,
            PLUGIN_REPORT_SKIPPED,
            format!("report:{plugin_id}/{}", decl.name),
            json!({"plugin": plugin_id, "report": decl.name, "reason": reason}),
        );
        tx.append_audit(&audit).await
    }
}
