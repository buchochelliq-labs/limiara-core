use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Person {
    #[serde(skip)]
    pub id: Uuid,
    #[serde(rename = "id")]
    pub public_id: String,
    #[serde(skip)]
    pub tenant_id: Uuid,
    pub full_name: String,
    pub email: Option<String>,
    pub telephone: Option<String>,
    #[serde(skip)]
    pub organisation_id: Option<Uuid>,
    #[serde(rename = "organisation_id")]
    pub organisation_public_id: Option<String>,
    pub tags: Vec<String>,
    pub custom_fields: BTreeMap<String, serde_json::Value>,
    /// Values other plugins store with this record. Never serialised as is:
    /// callers receive only what the kernel extension service projects.
    #[serde(skip)]
    pub extensions: platform_extensions::ExtensionValues,
    pub version: i64,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Standard record stamps (ADR 0014). Members are served by the API as
    /// user references the kernel resolves, never as raw identifiers.
    #[serde(skip)]
    pub created_by: Option<Uuid>,
    #[serde(skip)]
    pub updated_by: Option<Uuid>,
    pub touched_at: DateTime<Utc>,
    /// When the record moved to the recycle bin; the standard `is_deleted`
    /// field is derived from it (ADR 0014).
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Organisation {
    #[serde(skip)]
    pub id: Uuid,
    #[serde(rename = "id")]
    pub public_id: String,
    #[serde(skip)]
    pub tenant_id: Uuid,
    pub name: String,
    pub email: Option<String>,
    pub telephone: Option<String>,
    pub custom_fields: BTreeMap<String, serde_json::Value>,
    /// Values other plugins store with this record. Never serialised as is:
    /// callers receive only what the kernel extension service projects.
    #[serde(skip)]
    pub extensions: platform_extensions::ExtensionValues,
    pub version: i64,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Standard record stamps (ADR 0014). Members are served by the API as
    /// user references the kernel resolves, never as raw identifiers.
    #[serde(skip)]
    pub created_by: Option<Uuid>,
    #[serde(skip)]
    pub updated_by: Option<Uuid>,
    pub touched_at: DateTime<Utc>,
    /// When the record moved to the recycle bin; the standard `is_deleted`
    /// field is derived from it (ADR 0014).
    pub deleted_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePerson {
    pub full_name: String,
    pub email: Option<String>,
    pub telephone: Option<String>,
    pub organisation_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdatePerson {
    pub expected_version: i64,
    pub full_name: String,
    pub email: Option<String>,
    pub telephone: Option<String>,
    pub organisation_id: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateOrganisation {
    pub name: String,
    pub email: Option<String>,
    pub telephone: Option<String>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOrganisation {
    pub expected_version: i64,
    pub name: String,
    pub email: Option<String>,
    pub telephone: Option<String>,
    #[serde(default)]
    pub custom_fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContactCustomFieldTarget {
    Person,
    Organisation,
}

impl ContactCustomFieldTarget {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Organisation => "organisation",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ContactCustomFieldType {
    Text,
    Textarea,
    Email,
    Phone,
    Number,
    Boolean,
    Date,
    DateTime,
    Select,
    MultiSelect,
    Json,
}

impl ContactCustomFieldType {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Textarea => "textarea",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "datetime",
            Self::Select => "select",
            Self::MultiSelect => "multi_select",
            Self::Json => "json",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "text" => Self::Text,
            "textarea" => Self::Textarea,
            "email" => Self::Email,
            "phone" => Self::Phone,
            "number" => Self::Number,
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "datetime" => Self::DateTime,
            "select" => Self::Select,
            "multi_select" => Self::MultiSelect,
            "json" => Self::Json,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DataClassification {
    Public,
    Internal,
    Personal,
    Sensitive,
}

impl DataClassification {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Personal => "personal",
            Self::Sensitive => "sensitive",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "public" => Self::Public,
            "internal" => Self::Internal,
            "personal" => Self::Personal,
            "sensitive" => Self::Sensitive,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactCustomFieldDefinition {
    pub id: Uuid,
    #[serde(skip)]
    pub tenant_id: Uuid,
    pub applies_to: ContactCustomFieldTarget,
    pub api_name: String,
    pub label: String,
    pub description: Option<String>,
    pub field_type: ContactCustomFieldType,
    pub required: bool,
    pub unique_per_tenant: bool,
    pub indexed: bool,
    pub options: Vec<String>,
    pub validation_schema: serde_json::Value,
    pub data_classification: DataClassification,
    pub purpose: Option<String>,
    pub retention_days: Option<u32>,
    pub encrypted: bool,
    pub version: i64,
    pub disabled_at: Option<DateTime<Utc>>,
    pub archived_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ContactCustomFieldDefinition {
    pub fn is_active(&self) -> bool {
        self.disabled_at.is_none() && self.archived_at.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateCustomFieldDefinition {
    pub applies_to: ContactCustomFieldTarget,
    pub api_name: String,
    pub label: String,
    pub description: Option<String>,
    pub field_type: ContactCustomFieldType,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique_per_tenant: bool,
    #[serde(default)]
    pub indexed: bool,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default = "empty_object")]
    pub validation_schema: serde_json::Value,
    pub data_classification: DataClassification,
    pub purpose: Option<String>,
    pub retention_days: Option<u32>,
    #[serde(default)]
    pub encrypted: bool,
}

fn empty_object() -> serde_json::Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCustomFieldDefinition {
    pub expected_version: i64,
    pub label: String,
    pub description: Option<String>,
    pub required: bool,
    pub unique_per_tenant: bool,
    pub indexed: bool,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default = "empty_object")]
    pub validation_schema: serde_json::Value,
    pub data_classification: DataClassification,
    pub purpose: Option<String>,
    pub retention_days: Option<u32>,
    pub encrypted: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    #[default]
    Active,
    Archived,
    /// Active and archived records; never deleted ones.
    All,
    /// The recycle bin: deleted records only.
    Deleted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContactSort {
    #[default]
    Name,
    CreatedAt,
    UpdatedAt,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListRequest {
    pub search: Option<String>,
    pub email: Option<String>,
    pub organisation_id: Option<String>,
    #[serde(default)]
    pub status: RecordStatus,
    #[serde(default)]
    pub sort: ContactSort,
    #[serde(default)]
    pub direction: SortDirection,
    #[serde(default = "default_limit")]
    pub limit: u16,
    pub cursor: Option<String>,
    /// Equality filter on one extension field, validated by the kernel
    /// extension service. Set by the API layer, never parsed from a query.
    #[serde(skip)]
    pub extension_filter: Option<platform_extensions::ValidatedExtensionFilter>,
}

fn default_limit() -> u16 {
    50
}

impl Default for ListRequest {
    fn default() -> Self {
        Self {
            search: None,
            email: None,
            organisation_id: None,
            status: RecordStatus::Active,
            sort: ContactSort::Name,
            direction: SortDirection::Asc,
            limit: default_limit(),
            cursor: None,
            extension_filter: None,
        }
    }
}

/// One entry in a record's activity timeline. It identifies what happened and
/// who caused it, never the record's personal attributes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityEntry {
    pub event_id: Uuid,
    pub action: String,
    pub entity_version: i64,
    pub actor_id: Option<Uuid>,
    pub occurred_at: DateTime<Utc>,
}

/// Most recent timeline entries returned for one record.
pub const ACTIVITY_PAGE_LIMIT: i64 = 50;

#[derive(Debug, Clone, Serialize)]
pub struct Page<T> {
    pub data: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValidationIssue {
    pub field: String,
    pub code: String,
    pub detail: String,
}

impl ValidationIssue {
    pub(crate) fn new(
        field: impl Into<String>,
        code: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            field: field.into(),
            code: code.into(),
            detail: detail.into(),
        }
    }
}
