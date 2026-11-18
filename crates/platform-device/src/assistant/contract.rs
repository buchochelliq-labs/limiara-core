//! The `assistant.v1` contract (ADR 0034 §1 and §3): the requests a native
//! assistant plugin sends and the answers it gets, as serialisable types,
//! so the Swift and Kotlin halves are built and tested against the same
//! shapes. Later versions add operations without changing these.
//!
//! Every answer is inert data: record content comes back as values and
//! display text, never as markup or instructions.

use super::errors::AssistantError;
use chrono::{DateTime, Utc};
use platform_objects::{Assistant, FieldType, QuickActionKind};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use uuid::Uuid;

/// The contract version every request names and every answer carries.
pub const CONTRACT: &str = "assistant.v1";
/// Search results at most.
pub const SEARCH_MAX: usize = 20;
/// Records a `list` page holds at most.
pub const LIST_PAGE_MAX: u16 = 50;
/// Conditions a `list` takes at most.
pub const CONDITIONS_MAX: usize = 5;
/// The longest text a condition or search compares with.
pub const VALUE_TEXT_MAX: usize = 255;

/// One call from a native assistant plugin.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AssistantRequest {
    /// Always [`CONTRACT`].
    pub contract: String,
    /// The assistant that called the plugin, from the closed list. The
    /// bridge accepts only the one the shell built the plugin for.
    pub assistant: Assistant,
    /// Whether the device is unlocked, as the platform tells the plugin.
    /// The bridge refuses when it is not (§8), so a plugin's mistake fails
    /// closed.
    pub device_unlocked: bool,
    pub operation: Operation,
}

impl AssistantRequest {
    /// A request under [`CONTRACT`].
    pub fn new(assistant: Assistant, device_unlocked: bool, operation: Operation) -> Self {
        Self {
            contract: CONTRACT.to_owned(),
            assistant,
            device_unlocked,
            operation,
        }
    }
}

/// The operations (§3). `create`, `update` and `run_action` prepare a
/// change and answer with a [`Confirmation`]; only `commit`, with the
/// confirmation's token, makes it. Delete, owner changes, sharing, bulk and
/// metadata changes are not offered.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "name", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    /// The exposed objects, the fields the member can read, the offered
    /// quick actions and the timelines.
    Describe,
    /// Global search over the exposed objects, or one of them.
    Search {
        text: String,
        #[serde(default)]
        object: Option<String>,
    },
    /// One record's readable values.
    Get { object: String, record: String },
    /// Records of one object, with typed conditions and one sort.
    List {
        object: String,
        #[serde(default)]
        conditions: Vec<Condition>,
        #[serde(default)]
        sort: Option<Sort>,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// A record's Activity timeline, paged as the route pages it.
    Timeline {
        object: String,
        record: String,
        #[serde(default)]
        cursor: Option<String>,
    },
    /// The quick actions offered on an object, or on one of its records.
    Actions {
        object: String,
        #[serde(default)]
        record: Option<String>,
    },
    /// Prepares a quick action on a record: `values` fills the action's
    /// form fields; its presets are applied on top.
    RunAction {
        object: String,
        action: String,
        record: String,
        #[serde(default)]
        values: Map<String, Value>,
    },
    /// Prepares a new record.
    Create {
        object: String,
        values: Map<String, Value>,
    },
    /// Prepares a change to chosen fields of a record.
    Update {
        object: String,
        record: String,
        values: Map<String, Value>,
    },
    /// Makes a prepared change, after the member confirmed it.
    Commit { token: String },
    /// The record the app shows, for "this record", when its object is
    /// exposed and the member can read it.
    OnScreen,
}

/// A typed `list` condition, compiled into the query language (ADR 0019).
/// An assistant never sends query text.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "test", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
    Equals {
        field: String,
        value: Value,
    },
    NotEquals {
        field: String,
        value: Value,
    },
    /// Text that contains `value`, ignoring case.
    Contains {
        field: String,
        value: String,
    },
    /// Earlier than, or less than, `value`.
    Before {
        field: String,
        value: Value,
    },
    /// Later than, or more than, `value`.
    After {
        field: String,
        value: Value,
    },
    /// From `from` to `to`, both included.
    Between {
        field: String,
        from: Value,
        to: Value,
    },
    IsEmpty {
        field: String,
    },
}

impl Condition {
    pub fn field(&self) -> &str {
        match self {
            Self::Equals { field, .. }
            | Self::NotEquals { field, .. }
            | Self::Contains { field, .. }
            | Self::Before { field, .. }
            | Self::After { field, .. }
            | Self::Between { field, .. }
            | Self::IsEmpty { field } => field,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Sort {
    pub field: String,
    #[serde(default)]
    pub descending: bool,
}

/// The answer to one request.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AssistantAnswer {
    pub contract: &'static str,
    #[serde(flatten)]
    pub outcome: Outcome,
}

impl AssistantAnswer {
    pub fn ok(result: AssistantResult) -> Self {
        Self {
            contract: CONTRACT,
            outcome: Outcome::Result(result),
        }
    }

    pub fn error(error: AssistantError) -> Self {
        Self {
            contract: CONTRACT,
            outcome: Outcome::Error(error),
        }
    }

    /// The result, or the error.
    pub fn into_result(self) -> Result<AssistantResult, AssistantError> {
        match self.outcome {
            Outcome::Result(result) => Ok(result),
            Outcome::Error(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Result(AssistantResult),
    Error(AssistantError),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AssistantResult {
    Description(Description),
    SearchResults(SearchResults),
    Record(RecordView),
    Records(RecordPage),
    Timeline(TimelineView),
    Actions(ActionList),
    Confirmation(Confirmation),
    Committed(Committed),
    OnScreen(OnScreen),
}

/// What `describe` returns (§2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Description {
    pub metadata_version: i64,
    pub objects: Vec<DescribedObject>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DescribedObject {
    pub api_name: String,
    pub label: String,
    pub plural_label: String,
    /// The label of the record name.
    pub name_label: String,
    /// What the member may do, as the engine decides.
    pub can_create: bool,
    pub can_edit: bool,
    /// Whether the policy lets assistants write its records.
    pub writable: bool,
    /// Whether its records have an Activity timeline the member can read.
    pub timeline: bool,
    pub fields: Vec<DescribedField>,
    pub actions: Vec<DescribedAction>,
    pub metadata_version: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DescribedField {
    pub api_name: String,
    pub label: String,
    pub field_type: FieldType,
    pub required: bool,
    /// Computed by the engine (formulas, roll-ups) or not changeable by the
    /// member.
    pub read_only: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<DescribedOption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reference_object: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DescribedOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DescribedAction {
    /// The action's API name, unique on its object.
    pub name: String,
    pub label: String,
    pub kind: QuickActionKind,
    /// The object it is run on.
    pub object: String,
    /// The object it writes: the same object for an update, the child for
    /// a create.
    pub target_object: String,
    /// The form fields it asks for, of the target object.
    pub fields: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResults {
    pub results: Vec<RecordRef>,
}

/// A record by id and name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordRef {
    pub object: String,
    pub object_label: String,
    pub id: String,
    pub name: String,
}

/// One field's value, raw and as it reads.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldValue {
    pub field: String,
    pub label: String,
    pub field_type: FieldType,
    pub value: Value,
    pub display: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordView {
    pub object: String,
    pub object_label: String,
    pub id: String,
    pub name: String,
    pub version: i64,
    pub fields: Vec<FieldValue>,
    pub metadata_version: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RecordPage {
    pub object: String,
    pub records: Vec<RecordRef>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineEntry {
    pub object: String,
    pub object_label: String,
    pub id: String,
    pub name: String,
    pub date: Option<String>,
    pub open: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineView {
    pub open: Vec<TimelineEntry>,
    pub past: Vec<TimelineEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActionList {
    pub actions: Vec<DescribedAction>,
}

/// Which write a confirmation is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteKind {
    Create,
    Update,
    RunAction,
}

/// What the member confirms (§5): the object's label, the record's name
/// for a change to one, and each field that would change with its label
/// and new display value. The token commits exactly this.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Confirmation {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub write: WriteKind,
    pub object: String,
    pub object_label: String,
    /// The record an update or a quick action is about.
    pub record_name: Option<String>,
    /// The quick action's label.
    pub action_label: Option<String>,
    pub changes: Vec<FieldValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Committed {
    pub object: String,
    pub id: String,
    pub name: String,
    pub version: i64,
}

/// The record on screen, by id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OnScreen {
    pub workspace: Uuid,
    pub object: Option<String>,
    pub record: Option<String>,
}
