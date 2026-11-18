//! What the assistant bridge reads, always through [`Bridge::handle`] as the
//! signed-in member: the assistant policy, the objects and fields the
//! member can read, quick actions and records. Every check the engine makes
//! for the web shell applies, and the policy is applied after it, only
//! ever narrowing.

use super::errors::{AssistantCode, AssistantError, Issue, Problem};
use crate::Bridge;
use platform_objects::{FieldType, QuickActionKind};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OptionMeta {
    pub value: String,
    pub label: String,
}

/// A field as the member sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FieldMeta {
    pub api_name: String,
    pub label: String,
    pub field_type: FieldType,
    pub required: bool,
    /// Whether the query language can filter and sort on it (ADR 0019 §2).
    pub indexed: bool,
    pub read: bool,
    pub edit: bool,
    pub options: Vec<OptionMeta>,
    pub reference_object: Option<String>,
    pub scale: Option<u32>,
    pub formula_type: Option<FieldType>,
}

impl FieldMeta {
    pub fn computed(&self) -> bool {
        matches!(self.field_type, FieldType::Formula | FieldType::Rollup)
    }
}

/// An object as the member sees it, with only the fields they can read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ObjectMeta {
    pub api_name: String,
    pub label: String,
    pub plural_label: String,
    pub name_label: String,
    pub can_create: bool,
    pub can_edit: bool,
    pub fields: Vec<FieldMeta>,
}

impl ObjectMeta {
    pub fn field(&self, api_name: &str) -> Option<&FieldMeta> {
        self.fields.iter().find(|field| field.api_name == api_name)
    }

    /// A field's label, the name label for `name`, the object's label for
    /// a record-wide issue, and the API name otherwise.
    pub fn label_of(&self, api_name: &str) -> String {
        match api_name {
            "name" => self.name_label.clone(),
            "record" | "" => self.label.clone(),
            other => self
                .field(other)
                .map_or_else(|| other.to_owned(), |field| field.label.clone()),
        }
    }
}

/// One object's assistant policy, as the replica's definitions carry it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub(crate) struct ObjectPolicy {
    #[serde(default)]
    pub exposed: bool,
    #[serde(default)]
    pub writable: bool,
    #[serde(default)]
    pub indexable: bool,
    #[serde(default)]
    pub excluded_fields: Vec<String>,
    #[serde(default)]
    pub offered_actions: Vec<String>,
}

/// The workspace's assistant policy, by object, for the objects the member
/// can read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PolicyMeta {
    pub enabled: bool,
    pub objects: BTreeMap<String, ObjectPolicy>,
}

impl PolicyMeta {
    pub fn exposed(&self, object: &str) -> bool {
        self.enabled
            && self
                .objects
                .get(object)
                .is_some_and(|policy| policy.exposed)
    }

    pub fn writable(&self, object: &str) -> bool {
        self.exposed(object)
            && self
                .objects
                .get(object)
                .is_some_and(|policy| policy.writable)
    }

    pub fn indexable(&self, object: &str) -> bool {
        self.exposed(object)
            && self
                .objects
                .get(object)
                .is_some_and(|policy| policy.indexable)
    }

    pub fn offers(&self, object: &str, action: &str) -> bool {
        self.exposed(object)
            && self
                .objects
                .get(object)
                .is_some_and(|policy| policy.offered_actions.iter().any(|name| name == action))
    }
}

/// A quick action as the member sees it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct ActionMeta {
    pub name: String,
    pub label: String,
    pub kind: QuickActionKind,
    #[serde(default)]
    pub target_object: Option<String>,
    #[serde(default)]
    pub target_field: Option<String>,
    #[serde(default)]
    pub fields: Vec<String>,
    #[serde(default)]
    pub presets: Map<String, Value>,
}

impl ActionMeta {
    /// The object the action writes.
    pub fn writes<'a>(&'a self, object: &'a str) -> Option<&'a str> {
        match self.kind {
            QuickActionKind::Update => Some(object),
            QuickActionKind::Create => self.target_object.as_deref(),
        }
    }
}

/// Reads and writes through one bridge.
pub(crate) struct Reader<'a> {
    pub bridge: &'a Bridge,
    pub workspace: Uuid,
}

#[derive(Deserialize)]
struct AccessBody {
    #[serde(default)]
    read: bool,
    #[serde(default)]
    create: bool,
    #[serde(default)]
    edit: bool,
}

#[derive(Deserialize)]
struct FieldAccessBody {
    #[serde(default)]
    read: bool,
    #[serde(default)]
    edit: bool,
}

#[derive(Deserialize)]
struct OptionBody {
    value: String,
    label: String,
    #[serde(default = "yes")]
    active: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
struct FieldBody {
    api_name: String,
    label: String,
    field_type: FieldType,
    #[serde(default)]
    required: bool,
    #[serde(default)]
    indexed: bool,
    #[serde(default)]
    options: Vec<OptionBody>,
    #[serde(default)]
    reference_object: Option<String>,
    #[serde(default)]
    scale: Option<i64>,
    #[serde(default)]
    formula_type: Option<FieldType>,
    #[serde(default)]
    access: Option<FieldAccessBody>,
}

#[derive(Deserialize)]
struct ObjectBody {
    api_name: String,
    label: String,
    plural_label: String,
    name_label: String,
    access: AccessBody,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

#[derive(Deserialize)]
struct ObjectsBody {
    data: Vec<ObjectBody>,
}

#[derive(Deserialize)]
struct PolicyEntryBody {
    object: String,
    #[serde(flatten)]
    policy: ObjectPolicy,
}

#[derive(Deserialize)]
struct PolicyBody {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    objects: Vec<PolicyEntryBody>,
}

#[derive(Deserialize)]
struct ActionsBody {
    data: Vec<ActionMeta>,
}

#[derive(Deserialize)]
struct ProblemBody {
    #[serde(default)]
    code: String,
    #[serde(default)]
    errors: Vec<Issue>,
}

/// Percent-encodes a query string from pairs.
pub(crate) fn query_string(pairs: &[(&str, &str)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in pairs {
        serializer.append_pair(key, value);
    }
    serializer.finish()
}

/// Percent-encodes one path segment.
pub(crate) fn segment(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

pub(crate) fn problem_of(status: u16, body: &[u8]) -> Problem {
    let parsed: ProblemBody = serde_json::from_slice(body).unwrap_or(ProblemBody {
        code: String::new(),
        errors: Vec::new(),
    });
    Problem {
        status,
        code: parsed.code,
        errors: parsed.errors,
    }
}

/// An unavailable state answered while reading metadata: the lease, the
/// key, the workspace. Anything else is `failed`.
pub(crate) fn state_error(problem: &Problem) -> AssistantError {
    super::errors::from_problem(problem, &|field| field.to_owned())
}

impl Reader<'_> {
    fn path(&self, rest: &str) -> String {
        format!("/api/v1/workspaces/{}{rest}", self.workspace)
    }

    /// One request through the command bridge.
    pub async fn call(
        &self,
        method: &str,
        rest: &str,
        body: Option<&Value>,
    ) -> Result<Value, Problem> {
        let mut request = http::Request::builder().method(method).uri(self.path(rest));
        let bytes = match body {
            Some(body) => {
                request = request.header(http::header::CONTENT_TYPE, "application/json");
                serde_json::to_vec(body).unwrap_or_default()
            }
            None => Vec::new(),
        };
        let Ok(request) = request.body(bytes) else {
            return Err(Problem {
                status: 400,
                code: "request.invalid".into(),
                errors: Vec::new(),
            });
        };
        let response = self.bridge.handle(request).await;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(problem_of(status, response.body()));
        }
        if response.body().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(response.body()).map_err(|_| Problem {
            status: 502,
            code: "platform.unavailable".into(),
            errors: Vec::new(),
        })
    }

    pub async fn get(&self, rest: &str) -> Result<Value, Problem> {
        self.call("GET", rest, None).await
    }

    /// The assistant policy in the replica's definitions.
    pub async fn policy(&self) -> Result<PolicyMeta, AssistantError> {
        let body = self
            .get("/objects/assistant-policy")
            .await
            .map_err(|problem| state_error(&problem))?;
        let parsed: PolicyBody =
            serde_json::from_value(body).map_err(|_| AssistantError::new(AssistantCode::Failed))?;
        Ok(PolicyMeta {
            enabled: parsed.enabled,
            objects: parsed
                .objects
                .into_iter()
                .map(|entry| (entry.object, entry.policy))
                .collect(),
        })
    }

    /// The objects the member can read, without fields.
    pub async fn objects(&self) -> Result<Vec<ObjectMeta>, AssistantError> {
        let body = self
            .get("/objects")
            .await
            .map_err(|problem| state_error(&problem))?;
        let parsed: ObjectsBody =
            serde_json::from_value(body).map_err(|_| AssistantError::new(AssistantCode::Failed))?;
        Ok(parsed
            .data
            .into_iter()
            .filter(|object| object.access.read)
            .map(object_meta)
            .collect())
    }

    /// One object the member can read, with the fields they can read.
    /// `None` when they cannot read it or it does not exist.
    pub async fn object(&self, api_name: &str) -> Result<Option<ObjectMeta>, AssistantError> {
        match self.get(&format!("/objects/{}", segment(api_name))).await {
            Ok(body) => {
                let parsed: ObjectBody = serde_json::from_value(body)
                    .map_err(|_| AssistantError::new(AssistantCode::Failed))?;
                Ok(parsed.access.read.then(|| object_meta(parsed)))
            }
            Err(problem) if problem.status == 404 => Ok(None),
            Err(problem) => Err(state_error(&problem)),
        }
    }

    /// An object's quick actions the member can complete.
    pub async fn actions(&self, api_name: &str) -> Result<Vec<ActionMeta>, AssistantError> {
        match self
            .get(&format!("/objects/{}/quick-actions", segment(api_name)))
            .await
        {
            Ok(body) => serde_json::from_value::<ActionsBody>(body)
                .map(|parsed| parsed.data)
                .map_err(|_| AssistantError::new(AssistantCode::Failed)),
            Err(problem) if problem.status == 404 => Ok(Vec::new()),
            Err(problem) => Err(state_error(&problem)),
        }
    }

    /// One record as the member may read it: `None` when it does not
    /// exist or they cannot read it, which are the same.
    pub async fn record(
        &self,
        object: &str,
        record: &str,
    ) -> Result<Option<Map<String, Value>>, AssistantError> {
        match self
            .get(&format!(
                "/objects/{}/records/{}",
                segment(object),
                segment(record)
            ))
            .await
        {
            Ok(Value::Object(map)) => Ok(Some(map)),
            Ok(_) => Err(AssistantError::new(AssistantCode::Failed)),
            Err(problem) if problem.status == 404 || problem.status == 400 => Ok(None),
            Err(problem) => Err(state_error(&problem)),
        }
    }
}

fn object_meta(body: ObjectBody) -> ObjectMeta {
    ObjectMeta {
        api_name: body.api_name,
        label: body.label,
        plural_label: body.plural_label,
        name_label: body.name_label,
        can_create: body.access.create,
        can_edit: body.access.edit,
        fields: body
            .fields
            .into_iter()
            .filter_map(|field| {
                // Without an access entry the route gave no field rights:
                // the field is left out rather than assumed readable.
                let access = field.access?;
                access.read.then(|| FieldMeta {
                    api_name: field.api_name,
                    label: field.label,
                    field_type: field.field_type,
                    required: field.required,
                    indexed: field.indexed,
                    read: access.read,
                    edit: access.edit,
                    options: field
                        .options
                        .into_iter()
                        .filter(|option| option.active)
                        .map(|option| OptionMeta {
                            value: option.value,
                            label: option.label,
                        })
                        .collect(),
                    reference_object: field.reference_object,
                    scale: field.scale.and_then(|scale| u32::try_from(scale).ok()),
                    formula_type: field.formula_type,
                })
            })
            .collect(),
    }
}
