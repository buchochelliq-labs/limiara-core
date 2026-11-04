//! Saved reports (ADR 0027, #313): a record query (ADR 0019) with a name
//! and a way to show its rows. A report is run as whoever opens it, so
//! sharing a report shares the question, never the answers.

use crate::events::Change;
use crate::service::{MutationContext, ObjectService};
use crate::{ObjectError, QueryRequest, QueryResult, ValidationIssue, REPORTS_MANAGE_PERMISSION};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Reports one workspace may keep.
pub const REPORTS_MAX: usize = 1_000;
const NAME_MAX: usize = 80;
const DESCRIPTION_MAX: usize = 1_000;

/// How a report shows its rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportChart {
    /// The rows as a table.
    #[default]
    Table,
    Bar,
    Line,
    Pie,
    /// One number: the first row's value column.
    Metric,
}

impl ReportChart {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::Bar => "bar",
            Self::Line => "line",
            Self::Pie => "pie",
            Self::Metric => "metric",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "table" => Some(Self::Table),
            "bar" => Some(Self::Bar),
            "line" => Some(Self::Line),
            "pie" => Some(Self::Pie),
            "metric" => Some(Self::Metric),
            _ => None,
        }
    }

    /// Whether the chart plots a value column against a label column.
    fn needs_label(self) -> bool {
        matches!(self, Self::Bar | Self::Line | Self::Pie)
    }
}

/// Who sees a report in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReportVisibility {
    /// Its owner, and members who manage reports.
    #[default]
    Private,
    /// Every workspace member.
    Workspace,
}

impl ReportVisibility {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Workspace => "workspace",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "private" => Some(Self::Private),
            "workspace" => Some(Self::Workspace),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    /// The record query the report runs (ADR 0019).
    pub query: String,
    pub chart: ReportChart,
    /// The column a chart labels its values with.
    pub label_column: Option<String>,
    /// The column a chart or metric plots.
    pub value_column: Option<String>,
    pub visibility: ReportVisibility,
    pub owner_id: Uuid,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateReport {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub query: String,
    #[serde(default)]
    pub chart: ReportChart,
    #[serde(default)]
    pub label_column: Option<String>,
    #[serde(default)]
    pub value_column: Option<String>,
    #[serde(default)]
    pub visibility: ReportVisibility,
}

/// A partial change to a report; `None` keeps a value.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateReport {
    pub expected_version: i64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub chart: Option<ReportChart>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub label_column: Option<Option<String>>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub value_column: Option<Option<String>>,
    #[serde(default)]
    pub visibility: Option<ReportVisibility>,
}

/// The column types a chart or metric can plot.
const NUMERIC_COLUMNS: [&str; 4] = ["number", "currency", "percent", "integer"];

impl ObjectService {
    fn manages_reports(&self) -> bool {
        self.caller().has(REPORTS_MANAGE_PERMISSION)
    }

    fn sees(&self, report: &Report) -> bool {
        report.visibility == ReportVisibility::Workspace
            || report.owner_id == self.caller().user_id()
            || self.manages_reports()
    }

    /// Whether the caller may change or delete a report they see: its
    /// owner, or a member who manages reports.
    pub fn can_edit_report(&self, report: &Report) -> bool {
        report.owner_id == self.caller().user_id() || self.manages_reports()
    }

    /// The reports the caller sees, by name.
    pub async fn list_reports(&self) -> Result<Vec<Report>, ObjectError> {
        let mut tx = self.begin().await?;
        let mut reports: Vec<Report> = tx
            .reports()
            .await?
            .into_iter()
            .filter(|report| self.sees(report))
            .collect();
        tx.commit().await?;
        reports.sort_by_key(|report| report.name.to_lowercase());
        Ok(reports)
    }

    /// One report the caller sees; any other is missing.
    pub async fn get_report(&self, id: Uuid) -> Result<Report, ObjectError> {
        let mut tx = self.begin().await?;
        let report = tx
            .report(id)
            .await?
            .filter(|report| self.sees(report))
            .ok_or(ObjectError::NotFound)?;
        tx.commit().await?;
        Ok(report)
    }

    /// Runs a report as the caller: only rows they may see come back.
    pub async fn run_report(
        &self,
        id: Uuid,
        cursor: Option<String>,
        page_size: Option<u16>,
    ) -> Result<(Report, QueryResult), ObjectError> {
        let report = self.get_report(id).await?;
        let result = self
            .query(QueryRequest {
                q: report.query.clone(),
                cursor,
                page_size,
                include_deleted: false,
            })
            .await?;
        Ok((report, result))
    }

    pub async fn create_report(
        &self,
        input: CreateReport,
        context: MutationContext,
    ) -> Result<Report, ObjectError> {
        if input.visibility == ReportVisibility::Workspace && !self.manages_reports() {
            return Err(ObjectError::PermissionDenied);
        }
        let now = Utc::now();
        let mut report = Report {
            id: Uuid::now_v7(),
            name: input.name,
            description: input.description,
            query: input.query,
            chart: input.chart,
            label_column: input.label_column,
            value_column: input.value_column,
            visibility: input.visibility,
            owner_id: self.caller().user_id(),
            version: 1,
            created_at: now,
            created_by: Some(self.caller().user_id()),
            updated_at: now,
            updated_by: Some(self.caller().user_id()),
        };
        self.check_report(&mut report).await?;
        let mut tx = self.begin().await?;
        // Serialises concurrent creates so the count below stays true.
        tx.lock_metadata().await?;
        if tx.reports().await?.len() >= REPORTS_MAX {
            return Err(ObjectError::Conflict("reports.limit_reached"));
        }
        tx.insert_report(&report).await?;
        self.record_change(tx.as_mut(), context, Change::ReportCreated(&report))
            .await?;
        tx.commit().await?;
        Ok(report)
    }

    pub async fn update_report(
        &self,
        id: Uuid,
        input: UpdateReport,
        context: MutationContext,
    ) -> Result<Report, ObjectError> {
        let mut report = self.get_report(id).await?;
        if !self.can_edit_report(&report) {
            return Err(ObjectError::PermissionDenied);
        }
        if report.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        if input.visibility == Some(ReportVisibility::Workspace)
            && report.visibility != ReportVisibility::Workspace
            && !self.manages_reports()
        {
            return Err(ObjectError::PermissionDenied);
        }
        if let Some(name) = input.name {
            report.name = name;
        }
        if let Some(description) = input.description {
            report.description = description;
        }
        if let Some(query) = input.query {
            report.query = query;
        }
        if let Some(chart) = input.chart {
            report.chart = chart;
        }
        if let Some(column) = input.label_column {
            report.label_column = column;
        }
        if let Some(column) = input.value_column {
            report.value_column = column;
        }
        if let Some(visibility) = input.visibility {
            report.visibility = visibility;
        }
        self.check_report(&mut report).await?;
        report.version += 1;
        report.updated_at = Utc::now();
        report.updated_by = Some(self.caller().user_id());
        let mut tx = self.begin().await?;
        if !tx.update_report(&report, input.expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(tx.as_mut(), context, Change::ReportUpdated(&report))
            .await?;
        tx.commit().await?;
        Ok(report)
    }

    pub async fn delete_report(
        &self,
        id: Uuid,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<Report, ObjectError> {
        let report = self.get_report(id).await?;
        if !self.can_edit_report(&report) {
            return Err(ObjectError::PermissionDenied);
        }
        let mut tx = self.begin().await?;
        if !tx.delete_report(id, expected_version).await? {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(tx.as_mut(), context, Change::ReportDeleted(&report))
            .await?;
        tx.commit().await?;
        Ok(report)
    }

    /// Checks a report's settings, and that its query runs for the caller
    /// and has the columns its chart plots. Text is trimmed in place.
    async fn check_report(&self, report: &mut Report) -> Result<(), ObjectError> {
        let mut issues = Vec::new();
        report.name = report.name.trim().to_owned();
        if report.name.is_empty() || report.name.chars().count() > NAME_MAX {
            issues.push(ValidationIssue::new(
                "name",
                "reports.name_invalid",
                "name the report in 1 to 80 characters",
            ));
        }
        report.description = report
            .description
            .take()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        if report
            .description
            .as_ref()
            .is_some_and(|text| text.chars().count() > DESCRIPTION_MAX)
        {
            issues.push(ValidationIssue::new(
                "description",
                "reports.description_invalid",
                "describe the report in at most 1,000 characters",
            ));
        }
        report.query = report.query.trim().to_owned();
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        // The query must run for whoever saves it; its errors are the
        // query language's own, addressed to the query.
        let columns = match self
            .query(QueryRequest {
                q: report.query.clone(),
                cursor: None,
                page_size: Some(1),
                include_deleted: false,
            })
            .await
        {
            Ok(result) => result.columns,
            Err(ObjectError::Validation(found)) => {
                return Err(ObjectError::Validation(
                    found
                        .into_iter()
                        .map(|issue| ValidationIssue {
                            field: "query".into(),
                            ..issue
                        })
                        .collect(),
                ))
            }
            Err(other) => return Err(other),
        };
        let column = |key: &Option<String>| {
            key.as_ref()
                .and_then(|key| columns.iter().find(|column| &column.key == key))
        };
        if report.chart == ReportChart::Table {
            report.label_column = None;
            report.value_column = None;
            return Ok(());
        }
        match column(&report.value_column) {
            Some(found) if NUMERIC_COLUMNS.contains(&found.column_type) => {}
            _ => issues.push(ValidationIssue::new(
                "value_column",
                "reports.value_column_invalid",
                "choose a number column of the query to plot",
            )),
        }
        if report.chart.needs_label() {
            if column(&report.label_column).is_none() {
                issues.push(ValidationIssue::new(
                    "label_column",
                    "reports.label_column_invalid",
                    "choose a column of the query to label the values with",
                ));
            }
        } else {
            report.label_column = None;
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ObjectError::Validation(issues))
        }
    }
}
