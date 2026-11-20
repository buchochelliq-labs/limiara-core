//! Reports a plugin declares (ADR 0037 §5).
//!
//! `[[reports]]` in `plugin.toml` describes saved reports (ADR 0027): a name,
//! a label, a record query (ADR 0019) and how to show its rows. The
//! declaration is inert data. When a workspace enables the plugin, the
//! kernel adds each report to the workspace as a report every member sees,
//! owned by the plugin, checking its query against the workspace's objects.
//! An upgrade changes only reports nobody has changed since, and never
//! deletes one; a report the workspace deleted is never added again.
//!
//! A report's query may read the plugin's own objects and its dependencies'.
//! The engine checks that when it adds the report, since the query language
//! is the engine's.

use serde::{Deserialize, Serialize};

/// Reports per plugin.
pub const MAX_PLUGIN_REPORTS: usize = 20;
/// Characters in a report's label, as in a workspace's own reports.
pub const MAX_REPORT_LABEL: usize = 80;
/// Characters in a report's description.
pub const MAX_REPORT_DESCRIPTION: usize = 1_000;
/// Characters in a report's query.
pub const MAX_REPORT_QUERY: usize = 10_000;
/// Characters in a chart column key.
pub const MAX_REPORT_COLUMN: usize = 200;

/// A report a plugin declares.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReportDecl {
    /// Unique per plugin: `[a-z][a-z0-9_]*`. It names the report across
    /// upgrades; it is never shown.
    pub name: String,
    /// The report's name in the workspace.
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A record query (ADR 0019), naming objects and fields by API name.
    pub query: String,
    #[serde(default)]
    pub chart: ReportChartDecl,
    /// The column a bar, line or pie chart labels its values with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label_column: Option<String>,
    /// The number column a chart or metric plots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_column: Option<String>,
}

/// How a report shows its rows.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReportChartDecl {
    #[default]
    Table,
    Bar,
    Line,
    Pie,
    Metric,
}

/// Why a report declaration was refused, naming the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportDeclIssue {
    pub report: String,
    pub reason: &'static str,
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 63
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !name.contains("__")
        && !name.ends_with('_')
}

/// Checks a plugin's report declarations.
pub fn validate_reports(reports: &[ReportDecl]) -> Result<(), ReportDeclIssue> {
    if reports.len() > MAX_PLUGIN_REPORTS {
        return Err(ReportDeclIssue {
            report: String::new(),
            reason: "a plugin may declare at most 20 reports",
        });
    }
    let mut names = std::collections::BTreeSet::new();
    for report in reports {
        let issue = |reason| ReportDeclIssue {
            report: report.name.clone(),
            reason,
        };
        if !valid_name(&report.name) {
            return Err(issue(
                "a report's name must be a lowercase letter followed by lowercase letters, digits or single underscores, up to 63",
            ));
        }
        if !names.insert(report.name.as_str()) {
            return Err(issue("duplicate report name"));
        }
        let label = report.label.trim();
        if label.is_empty() || label.chars().count() > MAX_REPORT_LABEL {
            return Err(issue("a report's label must have 1 to 80 characters"));
        }
        if report
            .description
            .as_deref()
            .is_some_and(|text| text.chars().count() > MAX_REPORT_DESCRIPTION)
        {
            return Err(issue(
                "a report's description must have at most 1000 characters",
            ));
        }
        let query = report.query.trim();
        if query.is_empty() || query.chars().count() > MAX_REPORT_QUERY {
            return Err(issue("a report's query must have 1 to 10000 characters"));
        }
        let column = |value: &Option<String>| {
            value.as_deref().is_some_and(|key| {
                !key.trim().is_empty() && key.chars().count() <= MAX_REPORT_COLUMN
            })
        };
        match report.chart {
            ReportChartDecl::Table => {
                if report.label_column.is_some() || report.value_column.is_some() {
                    return Err(issue("a table names no chart columns"));
                }
            }
            ReportChartDecl::Metric => {
                if !column(&report.value_column) || report.label_column.is_some() {
                    return Err(issue("a metric names its value_column and no label_column"));
                }
            }
            ReportChartDecl::Bar | ReportChartDecl::Line | ReportChartDecl::Pie => {
                if !column(&report.value_column) || !column(&report.label_column) {
                    return Err(issue("a chart names its label_column and value_column"));
                }
            }
        }
    }
    Ok(())
}
