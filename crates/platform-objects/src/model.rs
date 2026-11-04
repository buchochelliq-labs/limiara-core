//! Object and field definitions, their inputs, and records.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Who defined an object or field (ADR 0015). Kernel standard fields are not
/// stored as definitions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Workspace,
    Plugin,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Plugin => "plugin",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "workspace" => Some(Self::Workspace),
            "plugin" => Some(Self::Plugin),
            _ => None,
        }
    }
}

/// The organisation-wide default for records of an object (sharing, #286).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SharingModel {
    Private,
    Read,
    #[default]
    ReadWrite,
}

impl SharingModel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Private => "private",
            Self::Read => "read",
            Self::ReadWrite => "read_write",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "private" => Some(Self::Private),
            "read" => Some(Self::Read),
            "read_write" => Some(Self::ReadWrite),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionStatus {
    Active,
    Deleted,
}

impl DefinitionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Deleted => "deleted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "deleted" => Some(Self::Deleted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldType {
    Text,
    LongText,
    Email,
    Phone,
    Url,
    Number,
    Currency,
    Percent,
    Boolean,
    Date,
    DateTime,
    Picklist,
    MultiPicklist,
    Lookup,
    /// Computed from a formula when read (ADR 0021).
    Formula,
    /// A required lookup to the record's master: the master controls the
    /// record's sharing, and deleting or restoring the master deletes or
    /// restores its details with it.
    MasterDetail,
    /// A summary of the master's detail records, kept on the master
    /// (ADR 0026).
    Rollup,
}

impl FieldType {
    pub const ALL: [FieldType; 17] = [
        Self::Text,
        Self::LongText,
        Self::Email,
        Self::Phone,
        Self::Url,
        Self::Number,
        Self::Currency,
        Self::Percent,
        Self::Boolean,
        Self::Date,
        Self::DateTime,
        Self::Picklist,
        Self::MultiPicklist,
        Self::Lookup,
        Self::Formula,
        Self::MasterDetail,
        Self::Rollup,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::LongText => "long_text",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Url => "url",
            Self::Number => "number",
            Self::Currency => "currency",
            Self::Percent => "percent",
            Self::Boolean => "boolean",
            Self::Date => "date",
            Self::DateTime => "date_time",
            Self::Picklist => "picklist",
            Self::MultiPicklist => "multi_picklist",
            Self::Lookup => "lookup",
            Self::Formula => "formula",
            Self::MasterDetail => "master_detail",
            Self::Rollup => "rollup",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.as_str() == value)
    }

    /// Whether the field points to another record: a lookup or a
    /// master-detail field.
    pub fn is_reference(self) -> bool {
        matches!(self, Self::Lookup | Self::MasterDetail)
    }

    pub fn is_textual(self) -> bool {
        matches!(
            self,
            Self::Text | Self::LongText | Self::Email | Self::Phone | Self::Url
        )
    }

    pub fn is_numeric(self) -> bool {
        matches!(self, Self::Number | Self::Currency | Self::Percent)
    }

    /// Whether values of this type may be declared unique.
    pub fn may_be_unique(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Email | Self::Phone | Self::Url | Self::Number
        )
    }

    /// Picklists are always indexed, as Salesforce filters on them by default.
    pub fn always_indexed(self) -> bool {
        matches!(self, Self::Picklist | Self::MultiPicklist)
    }
}

/// What happens to a lookup when the record it points to is deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DeleteBehaviour {
    /// The lookup is cleared on every referencing record.
    #[default]
    Clear,
    /// Deleting the target is refused while records point to it.
    Restrict,
}

/// How a roll-up summary field summarises the detail records (ADR 0026).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollupFunction {
    Count,
    Sum,
    Min,
    Max,
}

impl RollupFunction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Sum => "sum",
            Self::Min => "min",
            Self::Max => "max",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "count" => Some(Self::Count),
            "sum" => Some(Self::Sum),
            "min" => Some(Self::Min),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// What a roll-up summary field summarises: the detail records reached
/// through a master-detail field, and the detail field it aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Rollup {
    pub function: RollupFunction,
    /// The master-detail field on the detail object.
    #[serde(skip)]
    pub relationship_id: Uuid,
    /// The summarised detail field; none for a count.
    #[serde(skip)]
    pub field_id: Option<Uuid>,
    /// API names, resolved for callers.
    pub object: String,
    pub relationship: String,
    pub field: Option<String>,
}

/// The roll-up of a new roll-up summary field, by API names.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRollup {
    pub function: RollupFunction,
    /// The detail object.
    pub object: String,
    /// Its master-detail field pointing to this object.
    pub relationship: String,
    /// The detail field to sum, or to take the minimum or maximum of.
    #[serde(default)]
    pub field: Option<String>,
}

impl DeleteBehaviour {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Clear => "clear",
            Self::Restrict => "restrict",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "clear" => Some(Self::Clear),
            "restrict" => Some(Self::Restrict),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PicklistOption {
    pub value: String,
    pub label: String,
    #[serde(default = "active_by_default")]
    pub active: bool,
    #[serde(default)]
    pub default: bool,
}

fn active_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldDefinition {
    pub id: Uuid,
    pub object_id: Uuid,
    pub api_name: String,
    pub label: String,
    pub description: Option<String>,
    pub field_type: FieldType,
    pub origin: Origin,
    pub required: bool,
    pub unique: bool,
    pub indexed: bool,
    pub searchable: bool,
    /// Whether changes are kept in record history (ADR 0017).
    pub track_history: bool,
    pub max_length: Option<i32>,
    pub scale: Option<i32>,
    pub options: Vec<PicklistOption>,
    pub default_value: Option<Value>,
    #[serde(skip)]
    pub reference_object_id: Option<Uuid>,
    /// API name of the object a lookup points to, resolved for callers.
    pub reference_object: Option<String>,
    pub delete_behaviour: Option<DeleteBehaviour>,
    /// A query-language condition on the target object that a lookup's
    /// new values must meet (ADR 0028).
    pub lookup_filter: Option<String>,
    /// The formula of a formula field (ADR 0021).
    pub formula: Option<String>,
    /// The type a formula or roll-up summary field gives.
    pub formula_type: Option<FieldType>,
    /// What a roll-up summary field summarises (ADR 0026).
    pub rollup: Option<Rollup>,
    pub position: i32,
    pub status: DefinitionStatus,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

impl FieldDefinition {
    /// Whether values are written to the typed index pivot. Roll-up
    /// summaries always are, so they can be filtered and sorted on.
    pub fn is_indexed(&self) -> bool {
        if self.field_type == FieldType::Rollup {
            return true;
        }
        !matches!(
            self.field_type,
            FieldType::Lookup | FieldType::MasterDetail | FieldType::Formula
        ) && (self.indexed || self.unique || self.field_type.always_indexed())
    }

    pub fn is_active(&self) -> bool {
        self.status == DefinitionStatus::Active
    }

    /// The type of the values a field holds: a formula or roll-up summary
    /// by its result type.
    pub fn value_type(&self) -> FieldType {
        match self.field_type {
            FieldType::Formula | FieldType::Rollup => {
                self.formula_type.unwrap_or(FieldType::Number)
            }
            other => other,
        }
    }

    /// The field as its values are validated and indexed: a formula or
    /// roll-up summary as a field of its result type.
    pub fn as_value_field(&self) -> std::borrow::Cow<'_, FieldDefinition> {
        if matches!(self.field_type, FieldType::Formula | FieldType::Rollup) {
            let mut result = self.clone();
            result.field_type = self.value_type();
            std::borrow::Cow::Owned(result)
        } else {
            std::borrow::Cow::Borrowed(self)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ObjectDefinition {
    pub id: Uuid,
    pub api_name: String,
    pub label: String,
    pub plural_label: String,
    pub description: Option<String>,
    pub origin: Origin,
    pub owner_plugin: Option<String>,
    pub key_prefix: String,
    pub name_label: String,
    pub sharing_model: SharingModel,
    /// Whether managers see and edit what their reports own (ADR 0016).
    pub grant_hierarchy: bool,
    /// Whether record history keeps lifecycle, name and owner changes and
    /// those of tracked fields (ADR 0017).
    pub track_history: bool,
    pub searchable: bool,
    pub in_navigation: bool,
    pub status: DefinitionStatus,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
    /// Active fields in display order; empty in listings.
    pub fields: Vec<FieldDefinition>,
}

impl ObjectDefinition {
    pub fn is_active(&self) -> bool {
        self.status == DefinitionStatus::Active
    }

    pub fn field(&self, api_name: &str) -> Option<&FieldDefinition> {
        self.fields
            .iter()
            .find(|field| field.api_name == api_name && field.is_active())
    }

    /// The active master-detail fields, the first created first.
    pub fn masters(&self) -> Vec<&FieldDefinition> {
        let mut masters: Vec<&FieldDefinition> = self
            .fields
            .iter()
            .filter(|field| field.is_active() && field.field_type == FieldType::MasterDetail)
            .collect();
        masters.sort_by_key(|field| field.id);
        masters
    }

    /// The master-detail field that controls the record's sharing: the
    /// first one created (ADR 0025).
    pub fn primary_master(&self) -> Option<&FieldDefinition> {
        self.masters().into_iter().next()
    }
}

/// A new workspace object. `name` is the short name; the engine qualifies it
/// as `custom:<name>`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateObject {
    pub name: String,
    pub label: String,
    pub plural_label: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub name_label: Option<String>,
    #[serde(default)]
    pub sharing_model: Option<SharingModel>,
    #[serde(default)]
    pub grant_hierarchy: Option<bool>,
    #[serde(default)]
    pub track_history: Option<bool>,
    #[serde(default)]
    pub searchable: Option<bool>,
    #[serde(default)]
    pub in_navigation: Option<bool>,
}

/// A partial change to an object definition.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateObject {
    pub expected_version: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub plural_label: Option<String>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub name_label: Option<String>,
    #[serde(default)]
    pub sharing_model: Option<SharingModel>,
    #[serde(default)]
    pub grant_hierarchy: Option<bool>,
    #[serde(default)]
    pub track_history: Option<bool>,
    #[serde(default)]
    pub searchable: Option<bool>,
    #[serde(default)]
    pub in_navigation: Option<bool>,
}

/// Reads an update field that may be left out (keep the value), `null`
/// (clear it) or a value (set it). Serde reads `null` as "left out" for a
/// plain `Option<Option<_>>`, so clearing needs this.
pub(crate) fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// A new workspace field. `name` is the short name; the engine qualifies it
/// as `custom:<name>`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateField {
    pub name: String,
    pub label: String,
    pub field_type: FieldType,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub unique: bool,
    #[serde(default)]
    pub indexed: bool,
    #[serde(default)]
    pub searchable: bool,
    #[serde(default)]
    pub track_history: bool,
    #[serde(default)]
    pub max_length: Option<i32>,
    #[serde(default)]
    pub scale: Option<i32>,
    #[serde(default)]
    pub options: Vec<PicklistOption>,
    #[serde(default)]
    pub default_value: Option<Value>,
    /// The API name of the object a lookup points to.
    #[serde(default)]
    pub reference_object: Option<String>,
    #[serde(default)]
    pub delete_behaviour: Option<DeleteBehaviour>,
    /// A condition on the target object that a lookup's values must meet,
    /// in the query language's WHERE syntax (ADR 0028).
    #[serde(default)]
    pub lookup_filter: Option<String>,
    /// The formula of a formula field.
    #[serde(default)]
    pub formula: Option<String>,
    /// The type a formula field gives: text, number, currency, percent,
    /// boolean, date or date_time.
    #[serde(default)]
    pub formula_type: Option<FieldType>,
    /// What a roll-up summary field summarises.
    #[serde(default)]
    pub rollup: Option<CreateRollup>,
}

/// A partial change to a field. The type, uniqueness and lookup target are
/// fixed once a field exists.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateField {
    pub expected_version: i64,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub required: Option<bool>,
    #[serde(default)]
    pub indexed: Option<bool>,
    #[serde(default)]
    pub searchable: Option<bool>,
    #[serde(default)]
    pub track_history: Option<bool>,
    #[serde(default)]
    pub max_length: Option<i32>,
    #[serde(default)]
    pub options: Option<Vec<PicklistOption>>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub default_value: Option<Option<Value>>,
    #[serde(default)]
    pub position: Option<i32>,
    /// A new formula for a formula field.
    #[serde(default)]
    pub formula: Option<String>,
    /// A new lookup filter, or `null` to remove it (ADR 0028).
    #[serde(default, deserialize_with = "crate::model::present")]
    pub lookup_filter: Option<Option<String>>,
}

/// Effective access to one object, from object permissions and system
/// permissions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessFlags {
    #[serde(default)]
    pub read: bool,
    #[serde(default)]
    pub create: bool,
    #[serde(default)]
    pub edit: bool,
    #[serde(default)]
    pub delete: bool,
    #[serde(default)]
    pub view_all: bool,
    #[serde(default)]
    pub modify_all: bool,
}

impl AccessFlags {
    pub const FULL: Self = Self {
        read: true,
        create: true,
        edit: true,
        delete: true,
        view_all: true,
        modify_all: true,
    };

    /// Each grant implies the weaker ones, as in Salesforce.
    pub fn normalised(self) -> Self {
        let modify_all = self.modify_all;
        let view_all = self.view_all || modify_all;
        let delete = self.delete || modify_all;
        let edit = self.edit || delete;
        let create = self.create;
        let read = self.read || create || edit || view_all;
        Self {
            read,
            create,
            edit,
            delete,
            view_all,
            modify_all,
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            read: self.read || other.read,
            create: self.create || other.create,
            edit: self.edit || other.edit,
            delete: self.delete || other.delete,
            view_all: self.view_all || other.view_all,
            modify_all: self.modify_all || other.modify_all,
        }
        .normalised()
    }
}

/// An object permission row for one role.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RolePermission {
    pub role_id: Uuid,
    pub access: AccessFlags,
}

/// A record of any engine object. Standard fields are typed; custom values
/// are keyed by field API name and already normalised.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub id: Uuid,
    pub object_id: Uuid,
    pub public_id: String,
    pub name: String,
    pub owner_id: Option<Uuid>,
    pub record_type_id: Option<Uuid>,
    pub values: BTreeMap<String, Value>,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
    pub touched_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
}

/// One typed value in the index pivot.
#[derive(Debug, Clone, PartialEq)]
pub enum IndexValue {
    Text(String),
    Number(f64),
    Time(DateTime<Utc>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct IndexEntry {
    pub field_id: Uuid,
    pub value: IndexValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniqueEntry {
    pub field_id: Uuid,
    pub value_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReferenceEntry {
    pub field_id: Uuid,
    pub target_id: Uuid,
}

/// The pivot rows that index one record.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RecordIndex {
    pub entries: Vec<IndexEntry>,
    pub uniques: Vec<UniqueEntry>,
    pub references: Vec<ReferenceEntry>,
    /// Global search terms (ADR 0020).
    pub search: Vec<crate::search::SearchTerm>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    #[default]
    Active,
    /// The recycle bin.
    Deleted,
    /// Both, for sync clients: deleted records are tombstones (#287).
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordSort {
    #[default]
    Name,
    CreatedAt,
    UpdatedAt,
    /// The standard stamp that moves on every change (ADR 0014, #287).
    TouchedAt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    #[default]
    Asc,
    Desc,
}

/// An equality filter on an indexed field.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordFilter {
    pub field_id: Uuid,
    pub value: IndexValue,
}

/// A keyset position: the sort value and id of the last record returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordCursor {
    pub sort_value: String,
    pub id: Uuid,
}

/// A resolved list request for one object.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecordQuery {
    pub status: RecordStatus,
    pub sort: RecordSort,
    pub direction: SortDirection,
    pub limit: u16,
    pub after: Option<RecordCursor>,
    pub filters: Vec<RecordFilter>,
    pub owner: Option<Uuid>,
    /// A lowercase substring of the record name.
    pub name_contains: Option<String>,
    /// Sharing: only these rows are visible; `None` shows every row.
    pub visibility: Option<crate::Visibility>,
    /// Only records touched strictly after this moment (#287).
    pub touched_after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordPage {
    pub records: Vec<Record>,
    pub next_cursor: Option<String>,
}

/// What a history entry records (ADR 0017).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryKind {
    Created,
    Changed,
    Deleted,
    Undeleted,
}

impl HistoryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Changed => "changed",
            Self::Deleted => "deleted",
            Self::Undeleted => "undeleted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "created" => Some(Self::Created),
            "changed" => Some(Self::Changed),
            "deleted" => Some(Self::Deleted),
            "undeleted" => Some(Self::Undeleted),
            _ => None,
        }
    }
}

/// One tracked change to one record. `field` is `name`, `owner` or a field
/// API name for `Changed`, and absent otherwise.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HistoryEntry {
    pub id: Uuid,
    pub record_id: Uuid,
    pub object_id: Uuid,
    pub kind: HistoryKind,
    pub field: Option<String>,
    pub old_value: Option<Value>,
    pub new_value: Option<Value>,
    pub changed_at: DateTime<Utc>,
    pub changed_by: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
}

/// One page of a record's history, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub next_cursor: Option<String>,
}

/// What happened to a record, in the change log (ADR 0018).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Created,
    Updated,
    Deleted,
    Undeleted,
}

impl ChangeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Updated => "updated",
            Self::Deleted => "deleted",
            Self::Undeleted => "undeleted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "created" => Some(Self::Created),
            "updated" => Some(Self::Updated),
            "deleted" => Some(Self::Deleted),
            "undeleted" => Some(Self::Undeleted),
            _ => None,
        }
    }
}

/// One entry of a workspace's change log: which record changed and which
/// fields, never their values. `position` is gap-free per workspace and in
/// commit order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordChange {
    pub position: i64,
    pub record_id: Uuid,
    pub object_id: Uuid,
    pub public_id: String,
    pub kind: ChangeKind,
    pub fields: Vec<String>,
    pub changed_at: DateTime<Utc>,
    pub changed_by: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
}

/// The server version a replica record last took (ADR 0024 §3), and
/// whether the server had it in the recycle bin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteVersion {
    pub version: i64,
    pub deleted: bool,
    /// The device's change-log position the local copy took the server's
    /// at: the member's changes after it are what the record still has to
    /// send.
    pub through: i64,
}

/// Where a replica's sync stands (ADR 0024 §3).
/// What a replica remembers for its offline lease (ADR 0024 §4): the limit
/// the server set and when the device last reached the server.
/// The workspace's members and roles as a replica last took them from the
/// server (ADR 0024, "Offline directory"): display names only, never
/// permissions, so a device can name the people and roles its records
/// mention while offline.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicaDirectory {
    pub members: std::collections::BTreeMap<uuid::Uuid, String>,
    pub roles: std::collections::BTreeMap<uuid::Uuid, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplicaLease {
    pub offline_limit_seconds: Option<i64>,
    pub last_contact_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The latest device time the replica has seen, so turning the clock
    /// back cannot reopen a lease that has run out.
    pub clock_seen_at: Option<chrono::DateTime<chrono::Utc>>,
    /// The trusted clock's reading at the last contact, when the app has
    /// one (ADR 0024, "Trusted elapsed time").
    pub contact_trusted: Option<crate::TrustedInstant>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncPositions {
    /// The device's own change log is pushed up to here: every entry at or
    /// before it has nothing left to send.
    pub pushed: i64,
    /// The server's change log position the replica has pulled to, and
    /// the workspace metadata version it last applied. `None` before the
    /// first pull.
    pub pulled: Option<i64>,
    pub metadata_version: Option<i64>,
}

/// One page of the change log, visible to the caller.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangePage {
    /// Changes the caller can see, oldest first, with the object API name.
    pub changes: Vec<(String, RecordChange)>,
    /// Resume after this position; it advances past changes the caller
    /// cannot see, so a page can be empty while the log moves on.
    pub next_after: i64,
    /// The newest position in the workspace's log.
    pub head: i64,
    /// The workspace metadata version when the page was read (ADR 0024 §3).
    /// When it moves, a synced device fetches definitions again before it
    /// applies the page's records.
    pub metadata_version: i64,
}

/// A validation rule (ADR 0021): when its condition is true, a save is
/// blocked with its message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationRule {
    pub id: Uuid,
    pub object_id: Uuid,
    /// Unique per object: `[a-z][a-z0-9_]*`.
    pub name: String,
    pub description: Option<String>,
    /// A boolean formula describing the error.
    pub condition: String,
    pub message: String,
    /// The field the message is shown on; `None` shows it on the record.
    pub error_field: Option<String>,
    pub active: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateValidationRule {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub condition: String,
    pub message: String,
    #[serde(default)]
    pub error_field: Option<String>,
    #[serde(default)]
    pub active: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateValidationRule {
    pub expected_version: i64,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub error_field: Option<Option<String>>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// When a record-triggered automation runs (ADR 0022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTrigger {
    Create,
    Update,
    CreateOrUpdate,
    /// A record moved to the recycle bin; after saving only.
    Delete,
    /// A record restored from the recycle bin; after saving only.
    Undelete,
}

/// What happened to the record an automation runs for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SaveEvent {
    Create,
    /// Field changes and owner changes.
    Update,
    Delete,
    Undelete,
}

impl SaveEvent {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Undelete => "undelete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            "undelete" => Some(Self::Undelete),
            _ => None,
        }
    }
}

impl AutomationTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::CreateOrUpdate => "create_or_update",
            Self::Delete => "delete",
            Self::Undelete => "undelete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "update" => Some(Self::Update),
            "create_or_update" => Some(Self::CreateOrUpdate),
            "delete" => Some(Self::Delete),
            "undelete" => Some(Self::Undelete),
            _ => None,
        }
    }

    /// Whether an event fires it.
    pub fn fires(self, event: SaveEvent) -> bool {
        matches!(
            (self, event),
            (Self::Create | Self::CreateOrUpdate, SaveEvent::Create)
                | (Self::Update | Self::CreateOrUpdate, SaveEvent::Update)
                | (Self::Delete, SaveEvent::Delete)
                | (Self::Undelete, SaveEvent::Undelete)
        )
    }
}

/// Whether an automation changes the record before it is written or acts
/// on other records after it is written (ADR 0022).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationTiming {
    BeforeSave,
    AfterSave,
    /// After the change commits, in the background, with retries
    /// (ADR 0022 §6).
    Async,
    /// At a time set by `schedule`, in the background, with retries.
    Scheduled,
}

impl AutomationTiming {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BeforeSave => "before_save",
            Self::AfterSave => "after_save",
            Self::Async => "async",
            Self::Scheduled => "scheduled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "before_save" => Some(Self::BeforeSave),
            "after_save" => Some(Self::AfterSave),
            "async" => Some(Self::Async),
            "scheduled" => Some(Self::Scheduled),
            _ => None,
        }
    }

    /// Whether it runs in the background, from a queued job.
    pub fn is_background(self) -> bool {
        matches!(self, Self::Async | Self::Scheduled)
    }
}

/// When a scheduled automation runs: `offset_minutes` after (or, when
/// negative, before) the value of the date or date-time field `from`, or
/// after the change when `from` is `None` (ADR 0022 §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Schedule {
    pub offset_minutes: i64,
    #[serde(default)]
    pub from: Option<String>,
}

/// Sets a field to the value of a formula evaluated on the record that
/// triggered the automation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    /// `name` or a field API name on the record being written.
    pub field: String,
    pub formula: String,
}

/// What an automation does (ADR 0022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AutomationAction {
    /// Sets fields on the record being saved (before save), or on the
    /// record a background automation ran for.
    UpdateRecord {
        /// Runs the action only when this boolean formula holds (ADR 0022 §7).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
        values: Vec<Assignment>,
    },
    /// After save: updates the record a lookup on the saved record points to.
    UpdateRelated {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
        lookup: String,
        values: Vec<Assignment>,
    },
    /// After save: creates a record of an object.
    CreateRecord {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
        object: String,
        values: Vec<Assignment>,
    },
    /// After save: publishes a workspace-defined platform event whose fields
    /// are the values of formulas (ADR 0022 §7).
    PublishEvent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        when: Option<String>,
        /// `[a-z][a-z0-9_]*`, at most 63 characters.
        event: String,
        /// Each `field` names a field of the event.
        values: Vec<Assignment>,
    },
}

impl AutomationAction {
    pub fn values(&self) -> &[Assignment] {
        match self {
            Self::UpdateRecord { values, .. }
            | Self::UpdateRelated { values, .. }
            | Self::CreateRecord { values, .. }
            | Self::PublishEvent { values, .. } => values,
        }
    }

    pub fn when(&self) -> Option<&str> {
        match self {
            Self::UpdateRecord { when, .. }
            | Self::UpdateRelated { when, .. }
            | Self::CreateRecord { when, .. }
            | Self::PublishEvent { when, .. } => when.as_deref(),
        }
    }
}

/// A record-triggered automation (ADR 0022).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Automation {
    pub id: Uuid,
    pub object_id: Uuid,
    /// Unique per object: `[a-z][a-z0-9_]*`. Automations run in name order.
    pub name: String,
    pub description: Option<String>,
    pub trigger: AutomationTrigger,
    pub timing: AutomationTiming,
    /// A boolean formula; the automation runs only when it is true. `None`
    /// runs it on every save its trigger covers.
    pub condition: Option<String>,
    pub actions: Vec<AutomationAction>,
    /// Scheduled automations only.
    pub schedule: Option<Schedule>,
    pub active: bool,
    pub version: i64,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<Uuid>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateAutomation {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub trigger: AutomationTrigger,
    pub timing: AutomationTiming,
    #[serde(default)]
    pub condition: Option<String>,
    pub actions: Vec<AutomationAction>,
    #[serde(default)]
    pub schedule: Option<Schedule>,
    #[serde(default)]
    pub active: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateAutomation {
    pub expected_version: i64,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub description: Option<Option<String>>,
    #[serde(default)]
    pub trigger: Option<AutomationTrigger>,
    #[serde(default, deserialize_with = "crate::model::present")]
    pub condition: Option<Option<String>>,
    #[serde(default)]
    pub actions: Option<Vec<AutomationAction>>,
    #[serde(default)]
    pub schedule: Option<Schedule>,
    #[serde(default)]
    pub active: Option<bool>,
}

/// Whether an automation run finished its work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcome {
    Succeeded,
    /// The change it ran for was rolled back.
    Failed,
}

impl RunOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// One time an automation's condition held and it acted (ADR 0022 §5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutomationRun {
    pub id: Uuid,
    pub automation_id: Uuid,
    pub object_id: Uuid,
    /// The public ID of the record it ran for.
    pub record: String,
    pub event: SaveEvent,
    pub outcome: RunOutcome,
    /// Why it failed; administrator-facing, no record values.
    pub detail: Option<String>,
    /// Records its actions saved.
    pub saves: i64,
    pub ran_at: DateTime<Utc>,
    pub ran_by: Option<Uuid>,
    pub correlation_id: Option<Uuid>,
}

/// Where a queued automation job is (ADR 0022 §6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting until `due_at`, or for a retry.
    Pending,
    /// Claimed by a worker until `locked_until`.
    Running,
    Succeeded,
    /// Out of attempts; an administrator can retry it.
    Failed,
    /// Cancelled by an administrator, or no longer needed.
    Cancelled,
}

impl JobStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "running" => Some(Self::Running),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

/// A background or scheduled automation run, queued with the change that
/// fired it (ADR 0022 §6).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AutomationJob {
    pub id: Uuid,
    pub automation_id: Uuid,
    pub object_id: Uuid,
    pub record_id: Uuid,
    /// The record's public ID.
    pub record: String,
    pub event: SaveEvent,
    /// Scheduled jobs: one pending job per automation and record.
    pub scheduled: bool,
    pub due_at: DateTime<Utc>,
    pub status: JobStatus,
    pub attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    /// How many queued jobs led to this one, against endless chains.
    pub depth: i32,
    /// The member whose change queued it; its changes are theirs.
    pub actor_id: Uuid,
    pub correlation_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What a bulk job does to each row (ADR 0023 §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkOperation {
    Insert,
    Update,
    Delete,
}

impl BulkOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Insert => "insert",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "insert" => Some(Self::Insert),
            "update" => Some(Self::Update),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkStatus {
    /// Waiting for the worker, or between batches.
    Queued,
    /// A worker holds it until `locked_until`.
    Running,
    /// Every row was processed.
    Completed,
    /// Stopped by its submitter or an administrator; unprocessed rows stay
    /// pending.
    Aborted,
}

impl BulkStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Aborted => "aborted",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "completed" => Some(Self::Completed),
            "aborted" => Some(Self::Aborted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BulkRowStatus {
    Pending,
    Succeeded,
    Failed,
}

impl BulkRowStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "succeeded" => Some(Self::Succeeded),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

/// A bulk job (ADR 0023 §2): rows applied in the background, each as a
/// change of its own, with the submitting member's access.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BulkJob {
    pub id: Uuid,
    /// The object's API name.
    pub object: String,
    pub operation: BulkOperation,
    pub status: BulkStatus,
    pub total: i64,
    pub succeeded: i64,
    pub failed: i64,
    /// The member who submitted it. Rows run with their access as it is
    /// when each batch runs.
    pub actor_id: Uuid,
    pub correlation_id: Uuid,
    pub locked_until: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// One row of a bulk job and what became of it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BulkRow {
    pub job_id: Uuid,
    /// Its position in the submitted rows, from 0.
    pub index: i32,
    pub input: Value,
    pub status: BulkRowStatus,
    /// The record it created or changed.
    pub record: Option<String>,
    /// The problem, as the single-record route would report it.
    pub errors: Option<Value>,
}
