use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub const UI_CONTRACT_VERSION: u16 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum HostProfile {
    InternalWeb,
    Desktop,
    Portal,
    PublicForm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FieldType {
    Text,
    LongText,
    Email,
    Phone,
    Number,
    Boolean,
    Date,
    DateTime,
    Choice,
    MultiChoice,
    Reference,
    Money,
    Json,
    /// A workspace member, served as `{ "id", "name" }` (ADR 0014).
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FieldSource {
    Standard,
    TenantCustom,
    /// A field another plugin adds to this resource (ADR 0012). Served by the
    /// kernel extension service, never declared in a host resource document.
    Extension,
    /// A standard field the kernel defines on every record resource
    /// (ADR 0014). Never declared in a resource document.
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DataClassification {
    Public,
    Internal,
    Personal,
    Sensitive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldDescriptor {
    pub api_name: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub field_type: FieldType,
    pub source: FieldSource,
    #[serde(default)]
    pub readable: bool,
    #[serde(default)]
    pub writable: bool,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub classification: Option<DataClassification>,
    #[serde(default)]
    pub options: Vec<FieldOption>,
    #[serde(default)]
    pub validation_schema: Option<Value>,
    #[serde(default)]
    pub default_value: Option<Value>,
    #[serde(default)]
    pub submit_when_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelationshipDescriptor {
    pub id: String,
    pub target_resource: String,
    pub cardinality: RelationshipCardinality,
    #[serde(default)]
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RelationshipCardinality {
    One,
    Many,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OperationKind {
    Read,
    List,
    Create,
    Update,
    Archive,
    Command,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ConcurrencyMode {
    None,
    Etag,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceOperation {
    pub id: String,
    pub kind: OperationKind,
    pub permission: String,
    #[serde(default)]
    pub concurrency: Option<ConcurrencyMode>,
    #[serde(default)]
    pub hosts: Vec<HostProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceDescriptor {
    pub contract_version: u16,
    pub id: String,
    pub definition_version: u32,
    pub owner_plugin: String,
    pub label: String,
    /// The declared field that is this record's display name (ADR 0014).
    /// Required on record resources.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_field: Option<String>,
    #[serde(default)]
    pub fields: Vec<FieldDescriptor>,
    #[serde(default)]
    pub relationships: Vec<RelationshipDescriptor>,
    #[serde(default)]
    pub operations: Vec<ResourceOperation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "source", rename_all = "kebab-case")]
pub enum Binding {
    Field { field: String },
    Context { path: String },
    Literal { value: Value },
    DataSource { data_source: String, path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum ConditionExpression {
    And { args: Vec<ConditionExpression> },
    Or { args: Vec<ConditionExpression> },
    Not { arg: Box<ConditionExpression> },
    Equal { left: Binding, right: Binding },
    NotEqual { left: Binding, right: Binding },
    Exists { value: Binding },
    In { value: Binding, set: Vec<Binding> },
    HasPermission { permission: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum ConditionalState {
    Visible,
    Enabled,
    ReadOnly,
    Required,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiNode {
    pub id: String,
    pub component: String,
    #[serde(default)]
    pub props: BTreeMap<String, Value>,
    #[serde(default)]
    pub bindings: BTreeMap<String, Binding>,
    #[serde(default)]
    pub conditions: BTreeMap<ConditionalState, ConditionExpression>,
    #[serde(default)]
    pub children: Vec<UiNode>,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub hosts: Vec<HostProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PageKind {
    Record,
    RecordForm,
    Collection,
    Kanban,
    Calendar,
    Timeline,
    Dashboard,
    Wizard,
    Settings,
    Portal,
    PublicForm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DataSourceDefinition {
    pub id: String,
    pub resource: String,
    pub operation: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, Binding>,
    #[serde(default)]
    pub load: LoadPolicy,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LoadPolicy {
    #[default]
    Eager,
    Explicit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ActionKind {
    Navigate,
    Submit,
    Overlay,
    ResourceOperation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ActionDefinition {
    pub id: String,
    pub kind: ActionKind,
    pub target: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, Binding>,
    #[serde(default)]
    pub permission: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageVariant {
    pub id: String,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub condition: Option<ConditionExpression>,
    pub root: UiNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageDefinition {
    pub contract_version: u16,
    pub id: String,
    pub definition_version: u32,
    pub owner_plugin: String,
    pub kind: PageKind,
    pub hosts: Vec<HostProfile>,
    #[serde(default)]
    pub resource: Option<String>,
    #[serde(default)]
    pub data_sources: Vec<DataSourceDefinition>,
    #[serde(default)]
    pub actions: Vec<ActionDefinition>,
    #[serde(default)]
    pub variants: Vec<PageVariant>,
    pub root: UiNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FragmentDefinition {
    pub contract_version: u16,
    pub id: String,
    pub definition_version: u32,
    pub owner_plugin: String,
    pub hosts: Vec<HostProfile>,
    pub root: UiNode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentSlot {
    pub id: String,
    #[serde(default)]
    pub accepts: Vec<String>,
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ComponentDefinition {
    pub contract_version: u16,
    pub id: String,
    pub definition_version: u32,
    pub owner_plugin: String,
    pub props_schema: Value,
    #[serde(default)]
    pub slots: Vec<ComponentSlot>,
    pub hosts: Vec<HostProfile>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    pub accessibility: AccessibilityContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessibilityContract {
    pub semantics: String,
    #[serde(default)]
    pub keyboard: Vec<String>,
    #[serde(default)]
    pub required_label_prop: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PatchOperation {
    Inside,
    Before,
    After,
    Move,
    Configure,
    Replace,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionContribution {
    pub contract_version: u16,
    pub id: String,
    pub owner_plugin: String,
    pub target_page: String,
    pub target_version: u32,
    pub target_node: String,
    pub operation: PatchOperation,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub node: Option<UiNode>,
    #[serde(default)]
    pub props: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiBundle {
    #[serde(default)]
    pub resources: Vec<ResourceDescriptor>,
    #[serde(default)]
    pub pages: Vec<PageDefinition>,
    #[serde(default)]
    pub fragments: Vec<FragmentDefinition>,
    #[serde(default)]
    pub components: Vec<ComponentDefinition>,
    #[serde(default)]
    pub extensions: Vec<ExtensionContribution>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UiContractError {
    #[error("unsupported UI contract version {actual} in {document}; expected {expected}")]
    ContractVersion {
        document: String,
        actual: u16,
        expected: u16,
    },
    #[error("invalid stable identifier {id} in {document}")]
    InvalidId { document: String, id: String },
    #[error("duplicate {kind} identifier {id}")]
    DuplicateId { kind: &'static str, id: String },
    #[error("dangling {kind} reference {target} in {document}")]
    DanglingReference {
        document: String,
        kind: &'static str,
        target: String,
    },
    #[error("duplicate node identifier {id} in {document}")]
    DuplicateNode { document: String, id: String },
    #[error("custom field {field} in {resource} must end in __c")]
    CustomFieldSuffix { resource: String, field: String },
    #[error("field {field} in {resource} has invalid API-name syntax")]
    InvalidFieldName { resource: String, field: String },
    #[error("choice field {field} in {resource} requires options")]
    MissingFieldOptions { resource: String, field: String },
    #[error("node {node} in {page} supports a host not enabled by the page")]
    HostEscalation { page: String, node: String },
    #[error("document {document} must enable at least one host profile")]
    EmptyHosts { document: String },
    #[error("field node {node} in {page} references unknown field {field}")]
    UnknownField {
        page: String,
        node: String,
        field: String,
    },
    #[error("extension {extension} has an invalid payload for {operation:?}")]
    InvalidPatchPayload {
        extension: String,
        operation: PatchOperation,
    },
    #[error("condition in {document} contains an empty {operator} expression")]
    EmptyCondition {
        document: String,
        operator: &'static str,
    },
    #[error("binding in {document} references unknown {kind} {target}")]
    InvalidBinding {
        document: String,
        kind: &'static str,
        target: String,
    },
    #[error("data source dependency cycle in {document}: {cycle}")]
    DataSourceCycle { document: String, cycle: String },
    #[error("field {field} in {resource} is a standard record field the kernel defines")]
    ReservedFieldName { resource: String, field: String },
    #[error("record resource {resource} must name a required text field as name_field")]
    InvalidNameField { resource: String },
}

impl UiBundle {
    pub fn validate(&self) -> Result<(), UiContractError> {
        collect_ids("resource", self.resources.iter().map(|item| &item.id))?;
        let resources: BTreeMap<_, _> = self
            .resources
            .iter()
            .map(|resource| (resource.id.clone(), resource))
            .collect();
        let pages = collect_ids("page", self.pages.iter().map(|item| &item.id))?;
        collect_ids("fragment", self.fragments.iter().map(|item| &item.id))?;
        collect_ids("component", self.components.iter().map(|item| &item.id))?;
        collect_ids("extension", self.extensions.iter().map(|item| &item.id))?;

        for resource in &self.resources {
            resource.validate()?;
            for relationship in &resource.relationships {
                require_reference(
                    &resource.id,
                    "relationship resource",
                    &relationship.target_resource,
                    &resources.keys().cloned().collect(),
                )?;
            }
        }
        for page in &self.pages {
            page.validate(&resources)?;
        }
        for fragment in &self.fragments {
            validate_version(&fragment.id, fragment.contract_version)?;
            validate_id(&fragment.id, &fragment.id)?;
            require_hosts(&fragment.id, &fragment.hosts)?;
            validate_node_tree(&fragment.id, &fragment.root, &fragment.hosts)?;
        }
        for component in &self.components {
            validate_version(&component.id, component.contract_version)?;
            validate_id(&component.id, &component.id)?;
            require_hosts(&component.id, &component.hosts)?;
            collect_ids(
                "component slot",
                component.slots.iter().map(|slot| &slot.id),
            )?;
        }
        for extension in &self.extensions {
            extension.validate(&pages, &self.pages)?;
        }
        Ok(())
    }
}

impl ResourceDescriptor {
    pub fn validate(&self) -> Result<(), UiContractError> {
        validate_version(&self.id, self.contract_version)?;
        validate_id(&self.id, &self.id)?;
        validate_id(&self.id, &self.owner_plugin)?;
        collect_ids("field", self.fields.iter().map(|field| &field.api_name))?;
        collect_ids(
            "relationship",
            self.relationships.iter().map(|item| &item.id),
        )?;
        collect_ids("operation", self.operations.iter().map(|item| &item.id))?;
        let field_pattern = Regex::new(r"^[a-z][a-z0-9_]*$").expect("valid regex");
        for field in &self.fields {
            if !field_pattern.is_match(&field.api_name) {
                return Err(UiContractError::InvalidFieldName {
                    resource: self.id.clone(),
                    field: field.api_name.clone(),
                });
            }
            if field.source == FieldSource::TenantCustom && !field.api_name.ends_with("__c") {
                return Err(UiContractError::CustomFieldSuffix {
                    resource: self.id.clone(),
                    field: field.api_name.clone(),
                });
            }
            if matches!(field.field_type, FieldType::Choice | FieldType::MultiChoice)
                && field.options.is_empty()
            {
                return Err(UiContractError::MissingFieldOptions {
                    resource: self.id.clone(),
                    field: field.api_name.clone(),
                });
            }
            let names_record = self.name_field.as_deref() == Some(STANDARD_NAME_FIELD)
                && field.api_name == STANDARD_NAME_FIELD;
            if field.source == FieldSource::System
                || (is_standard_field(&field.api_name) && !names_record)
            {
                return Err(UiContractError::ReservedFieldName {
                    resource: self.id.clone(),
                    field: field.api_name.clone(),
                });
            }
        }
        if self.is_record_resource() || self.name_field.is_some() {
            let name_field = self
                .name_field
                .as_deref()
                .and_then(|name| self.fields.iter().find(|field| field.api_name == name));
            if !name_field.is_some_and(|field| {
                field.field_type == FieldType::Text
                    && field.required
                    && field.source == FieldSource::Standard
            }) {
                return Err(UiContractError::InvalidNameField {
                    resource: self.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// Whether callers read records of this resource, which gives it the
    /// standard record fields (ADR 0014).
    pub fn is_record_resource(&self) -> bool {
        self.operations
            .iter()
            .any(|operation| matches!(operation.kind, OperationKind::Read | OperationKind::List))
    }

    /// Whether `api_name` names a field of this resource, including the
    /// standard record fields.
    pub fn has_field(&self, api_name: &str) -> bool {
        self.fields.iter().any(|field| field.api_name == api_name)
            || (self.is_record_resource() && is_standard_field(api_name))
    }

    /// The declared fields followed by the standard record fields the
    /// resource does not declare itself (only `name` may be declared).
    pub fn fields_with_standard(&self) -> Vec<FieldDescriptor> {
        let mut fields = self.fields.clone();
        if self.is_record_resource() {
            fields.extend(
                standard_fields().into_iter().filter(|standard| {
                    !self.fields.iter().any(|f| f.api_name == standard.api_name)
                }),
            );
        }
        fields
    }
}

/// The standard field that holds a record's display name.
pub const STANDARD_NAME_FIELD: &str = "name";

/// API names of the standard record fields, in display order (ADR 0014).
pub const STANDARD_FIELD_NAMES: [&str; 11] = [
    "id",
    STANDARD_NAME_FIELD,
    "created_at",
    "created_by",
    "updated_at",
    "updated_by",
    "touched_at",
    "version",
    "archived_at",
    "is_deleted",
    "deleted_at",
];

pub fn is_standard_field(api_name: &str) -> bool {
    STANDARD_FIELD_NAMES.contains(&api_name)
}

/// The fields the kernel defines on every record resource (ADR 0014). They
/// are read-only to every caller.
pub fn standard_fields() -> Vec<FieldDescriptor> {
    let field =
        |api_name: &str, label: &str, description: &str, field_type: FieldType| FieldDescriptor {
            api_name: api_name.into(),
            label: label.into(),
            description: Some(description.into()),
            field_type,
            source: FieldSource::System,
            readable: true,
            writable: false,
            required: false,
            permission: None,
            classification: Some(DataClassification::Internal),
            options: Vec::new(),
            validation_schema: None,
            default_value: None,
            submit_when_hidden: false,
        };
    vec![
        field(
            "id",
            "Record ID",
            "Stable identifier for this record",
            FieldType::Text,
        ),
        field(
            STANDARD_NAME_FIELD,
            "Name",
            "The name shown for this record",
            FieldType::Text,
        ),
        field(
            "created_at",
            "Created",
            "When the record was created",
            FieldType::DateTime,
        ),
        field(
            "created_by",
            "Created by",
            "Who created the record",
            FieldType::User,
        ),
        field(
            "updated_at",
            "Last modified",
            "The last change a person made",
            FieldType::DateTime,
        ),
        field(
            "updated_by",
            "Last modified by",
            "Who made the last change",
            FieldType::User,
        ),
        field(
            "touched_at",
            "Last touched",
            "The last change of any kind, including automation and system jobs",
            FieldType::DateTime,
        ),
        field(
            "version",
            "Version",
            "Saved record version",
            FieldType::Number,
        ),
        field(
            "archived_at",
            "Archived",
            "When the record was archived",
            FieldType::DateTime,
        ),
        field(
            "is_deleted",
            "Deleted",
            "Whether the record is in the recycle bin",
            FieldType::Boolean,
        ),
        field(
            "deleted_at",
            "Deleted on",
            "When the record was moved to the recycle bin",
            FieldType::DateTime,
        ),
    ]
}

impl PageDefinition {
    pub fn validate(
        &self,
        resources: &BTreeMap<String, &ResourceDescriptor>,
    ) -> Result<(), UiContractError> {
        validate_version(&self.id, self.contract_version)?;
        validate_id(&self.id, &self.id)?;
        validate_id(&self.id, &self.owner_plugin)?;
        require_hosts(&self.id, &self.hosts)?;
        if let Some(resource) = &self.resource {
            require_reference(
                &self.id,
                "resource",
                resource,
                &resources.keys().cloned().collect(),
            )?;
        }
        collect_ids("data source", self.data_sources.iter().map(|item| &item.id))?;
        collect_ids("action", self.actions.iter().map(|item| &item.id))?;
        collect_ids("variant", self.variants.iter().map(|item| &item.id))?;
        for data_source in &self.data_sources {
            let resource = resources.get(&data_source.resource).ok_or_else(|| {
                UiContractError::DanglingReference {
                    document: self.id.clone(),
                    kind: "resource",
                    target: data_source.resource.clone(),
                }
            })?;
            if !resource
                .operations
                .iter()
                .any(|operation| operation.id == data_source.operation)
            {
                return Err(UiContractError::DanglingReference {
                    document: self.id.clone(),
                    kind: "resource operation",
                    target: format!("{}.{}", data_source.resource, data_source.operation),
                });
            }
            for binding in data_source.parameters.values() {
                validate_binding(
                    &self.id,
                    binding,
                    self.resource
                        .as_ref()
                        .and_then(|id| resources.get(id).copied()),
                    &self.data_sources,
                )?;
            }
        }
        validate_data_source_cycles(&self.id, &self.data_sources)?;
        for action in &self.actions {
            if action.kind == ActionKind::ResourceOperation {
                let (resource_id, operation_id) =
                    action.target.rsplit_once('.').ok_or_else(|| {
                        UiContractError::DanglingReference {
                            document: self.id.clone(),
                            kind: "resource operation",
                            target: action.target.clone(),
                        }
                    })?;
                let resource = resources.get(resource_id).ok_or_else(|| {
                    UiContractError::DanglingReference {
                        document: self.id.clone(),
                        kind: "resource operation",
                        target: action.target.clone(),
                    }
                })?;
                if !resource
                    .operations
                    .iter()
                    .any(|operation| operation.id == operation_id)
                {
                    return Err(UiContractError::DanglingReference {
                        document: self.id.clone(),
                        kind: "resource operation",
                        target: action.target.clone(),
                    });
                }
            }
            for binding in action.parameters.values() {
                validate_binding(
                    &self.id,
                    binding,
                    self.resource
                        .as_ref()
                        .and_then(|id| resources.get(id).copied()),
                    &self.data_sources,
                )?;
            }
        }
        validate_node_tree(&self.id, &self.root, &self.hosts)?;
        if let Some(resource_id) = &self.resource {
            validate_node_bindings(
                &self.id,
                &self.root,
                Some(resources[resource_id]),
                &self.data_sources,
            )?;
        } else {
            validate_node_bindings(&self.id, &self.root, None, &self.data_sources)?;
        }
        for variant in &self.variants {
            if let Some(condition) = &variant.condition {
                validate_condition(&self.id, condition)?;
            }
            validate_node_tree(&self.id, &variant.root, &self.hosts)?;
            if let Some(resource_id) = &self.resource {
                validate_node_bindings(
                    &self.id,
                    &variant.root,
                    Some(resources[resource_id]),
                    &self.data_sources,
                )?;
            } else {
                validate_node_bindings(&self.id, &variant.root, None, &self.data_sources)?;
            }
        }
        Ok(())
    }
}

impl ExtensionContribution {
    fn validate(
        &self,
        pages: &BTreeSet<String>,
        page_definitions: &[PageDefinition],
    ) -> Result<(), UiContractError> {
        validate_version(&self.id, self.contract_version)?;
        validate_id(&self.id, &self.id)?;
        validate_id(&self.id, &self.owner_plugin)?;
        require_reference(&self.id, "page", &self.target_page, pages)?;
        let page = page_definitions
            .iter()
            .find(|page| page.id == self.target_page)
            .expect("page id set and definitions agree");
        if page.definition_version != self.target_version
            || !node_ids(&page.root).contains(&self.target_node)
        {
            return Err(UiContractError::DanglingReference {
                document: self.id.clone(),
                kind: "page version or node",
                target: format!(
                    "{}@{}#{}",
                    self.target_page, self.target_version, self.target_node
                ),
            });
        }
        let valid_payload = match self.operation {
            PatchOperation::Inside
            | PatchOperation::Before
            | PatchOperation::After
            | PatchOperation::Replace => self.node.is_some() && self.props.is_empty(),
            PatchOperation::Configure => self.node.is_none() && !self.props.is_empty(),
            PatchOperation::Move => self.node.is_none() && self.props.contains_key("destination"),
        };
        if !valid_payload {
            return Err(UiContractError::InvalidPatchPayload {
                extension: self.id.clone(),
                operation: self.operation.clone(),
            });
        }
        if let Some(node) = &self.node {
            validate_node_tree(&self.id, node, &page.hosts)?;
        }
        Ok(())
    }
}

fn validate_version(document: &str, actual: u16) -> Result<(), UiContractError> {
    if actual != UI_CONTRACT_VERSION {
        return Err(UiContractError::ContractVersion {
            document: document.into(),
            actual,
            expected: UI_CONTRACT_VERSION,
        });
    }
    Ok(())
}

fn validate_id(document: &str, id: &str) -> Result<(), UiContractError> {
    let pattern = Regex::new(r"^[a-z0-9]+([.-][a-z0-9-]+)+$").expect("valid regex");
    if !pattern.is_match(id) {
        return Err(UiContractError::InvalidId {
            document: document.into(),
            id: id.into(),
        });
    }
    Ok(())
}

fn collect_ids<'a>(
    kind: &'static str,
    ids: impl Iterator<Item = &'a String>,
) -> Result<BTreeSet<String>, UiContractError> {
    let mut collected = BTreeSet::new();
    for id in ids {
        if !collected.insert(id.clone()) {
            return Err(UiContractError::DuplicateId {
                kind,
                id: id.clone(),
            });
        }
    }
    Ok(collected)
}

fn require_reference(
    document: &str,
    kind: &'static str,
    target: &str,
    known: &BTreeSet<String>,
) -> Result<(), UiContractError> {
    if !known.contains(target) {
        return Err(UiContractError::DanglingReference {
            document: document.into(),
            kind,
            target: target.into(),
        });
    }
    Ok(())
}

fn validate_node_tree(
    document: &str,
    root: &UiNode,
    document_hosts: &[HostProfile],
) -> Result<(), UiContractError> {
    let mut ids = BTreeSet::new();
    fn visit(
        document: &str,
        node: &UiNode,
        document_hosts: &BTreeSet<HostProfile>,
        ids: &mut BTreeSet<String>,
    ) -> Result<(), UiContractError> {
        let node_pattern = Regex::new(r"^[a-z][a-z0-9-]*$").expect("valid regex");
        if !node_pattern.is_match(&node.id) {
            return Err(UiContractError::InvalidId {
                document: document.into(),
                id: node.id.clone(),
            });
        }
        validate_id(document, &node.component)?;
        if !ids.insert(node.id.clone()) {
            return Err(UiContractError::DuplicateNode {
                document: document.into(),
                id: node.id.clone(),
            });
        }
        if node.hosts.iter().any(|host| !document_hosts.contains(host)) {
            return Err(UiContractError::HostEscalation {
                page: document.into(),
                node: node.id.clone(),
            });
        }
        for condition in node.conditions.values() {
            validate_condition(document, condition)?;
        }
        for child in &node.children {
            visit(document, child, document_hosts, ids)?;
        }
        Ok(())
    }
    visit(
        document,
        root,
        &document_hosts.iter().cloned().collect(),
        &mut ids,
    )
}

fn require_hosts(document: &str, hosts: &[HostProfile]) -> Result<(), UiContractError> {
    if hosts.is_empty() {
        return Err(UiContractError::EmptyHosts {
            document: document.into(),
        });
    }
    Ok(())
}

fn validate_node_bindings(
    document: &str,
    node: &UiNode,
    resource: Option<&ResourceDescriptor>,
    data_sources: &[DataSourceDefinition],
) -> Result<(), UiContractError> {
    if node.component.ends_with(".field") || node.component.contains(".field.") {
        if let Some(field) = node.props.get("field").and_then(Value::as_str) {
            require_field(document, &node.id, field, resource)?;
        }
    }
    for binding in node.bindings.values() {
        validate_binding(document, binding, resource, data_sources)?;
    }
    for condition in node.conditions.values() {
        validate_condition_bindings(document, condition, resource, data_sources)?;
    }
    for child in &node.children {
        validate_node_bindings(document, child, resource, data_sources)?;
    }
    Ok(())
}

fn require_field(
    page: &str,
    node: &str,
    field: &str,
    resource: Option<&ResourceDescriptor>,
) -> Result<(), UiContractError> {
    if resource.is_some_and(|resource| resource.has_field(field)) {
        return Ok(());
    }
    Err(UiContractError::UnknownField {
        page: page.into(),
        node: node.into(),
        field: field.into(),
    })
}

fn validate_binding(
    document: &str,
    binding: &Binding,
    resource: Option<&ResourceDescriptor>,
    data_sources: &[DataSourceDefinition],
) -> Result<(), UiContractError> {
    match binding {
        Binding::Field { field } => {
            if resource.is_some_and(|resource| resource.has_field(field)) {
                Ok(())
            } else {
                Err(UiContractError::InvalidBinding {
                    document: document.into(),
                    kind: "field",
                    target: field.clone(),
                })
            }
        }
        Binding::DataSource { data_source, .. } => {
            if data_sources.iter().any(|source| source.id == *data_source) {
                Ok(())
            } else {
                Err(UiContractError::InvalidBinding {
                    document: document.into(),
                    kind: "data source",
                    target: data_source.clone(),
                })
            }
        }
        Binding::Context { .. } | Binding::Literal { .. } => Ok(()),
    }
}

fn validate_condition_bindings(
    document: &str,
    condition: &ConditionExpression,
    resource: Option<&ResourceDescriptor>,
    data_sources: &[DataSourceDefinition],
) -> Result<(), UiContractError> {
    match condition {
        ConditionExpression::And { args } | ConditionExpression::Or { args } => {
            for argument in args {
                validate_condition_bindings(document, argument, resource, data_sources)?;
            }
        }
        ConditionExpression::Not { arg } => {
            validate_condition_bindings(document, arg, resource, data_sources)?;
        }
        ConditionExpression::Equal { left, right }
        | ConditionExpression::NotEqual { left, right } => {
            validate_binding(document, left, resource, data_sources)?;
            validate_binding(document, right, resource, data_sources)?;
        }
        ConditionExpression::Exists { value } => {
            validate_binding(document, value, resource, data_sources)?;
        }
        ConditionExpression::In { value, set } => {
            validate_binding(document, value, resource, data_sources)?;
            for binding in set {
                validate_binding(document, binding, resource, data_sources)?;
            }
        }
        ConditionExpression::HasPermission { .. } => {}
    }
    Ok(())
}

fn validate_data_source_cycles(
    document: &str,
    data_sources: &[DataSourceDefinition],
) -> Result<(), UiContractError> {
    let dependencies: BTreeMap<&str, BTreeSet<&str>> = data_sources
        .iter()
        .map(|source| {
            let dependencies = source
                .parameters
                .values()
                .filter_map(|binding| match binding {
                    Binding::DataSource { data_source, .. } => Some(data_source.as_str()),
                    _ => None,
                })
                .collect();
            (source.id.as_str(), dependencies)
        })
        .collect();
    fn visit<'a>(
        current: &'a str,
        dependencies: &BTreeMap<&'a str, BTreeSet<&'a str>>,
        active: &mut Vec<&'a str>,
        complete: &mut BTreeSet<&'a str>,
        document: &str,
    ) -> Result<(), UiContractError> {
        if let Some(position) = active.iter().position(|candidate| *candidate == current) {
            let mut cycle = active[position..].to_vec();
            cycle.push(current);
            return Err(UiContractError::DataSourceCycle {
                document: document.into(),
                cycle: cycle.join(" -> "),
            });
        }
        if complete.contains(current) {
            return Ok(());
        }
        active.push(current);
        if let Some(next) = dependencies.get(current) {
            for dependency in next {
                visit(dependency, dependencies, active, complete, document)?;
            }
        }
        active.pop();
        complete.insert(current);
        Ok(())
    }
    let mut complete = BTreeSet::new();
    for source in data_sources {
        visit(
            &source.id,
            &dependencies,
            &mut Vec::new(),
            &mut complete,
            document,
        )?;
    }
    Ok(())
}

fn validate_condition(
    document: &str,
    condition: &ConditionExpression,
) -> Result<(), UiContractError> {
    match condition {
        ConditionExpression::And { args } | ConditionExpression::Or { args } => {
            if args.is_empty() {
                return Err(UiContractError::EmptyCondition {
                    document: document.into(),
                    operator: "and/or",
                });
            }
            for arg in args {
                validate_condition(document, arg)?;
            }
        }
        ConditionExpression::Not { arg } => validate_condition(document, arg)?,
        _ => {}
    }
    Ok(())
}

fn node_ids(root: &UiNode) -> BTreeSet<String> {
    let mut ids = BTreeSet::new();
    fn visit(node: &UiNode, ids: &mut BTreeSet<String>) {
        ids.insert(node.id.clone());
        for child in &node.children {
            visit(child, ids);
        }
    }
    visit(root, &mut ids);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_bundle() -> UiBundle {
        serde_json::from_str(include_str!("../fixtures/ui/bundle.json"))
            .expect("golden UI bundle must deserialize")
    }

    #[test]
    fn golden_bundle_round_trips_and_validates() {
        let bundle = fixture_bundle();
        bundle.validate().expect("golden bundle must be valid");
        let encoded = serde_json::to_string(&bundle).expect("serialize bundle");
        let decoded: UiBundle = serde_json::from_str(&encoded).expect("deserialize bundle");
        assert_eq!(decoded, bundle);
    }

    #[test]
    fn rejects_unknown_contract_versions() {
        let mut bundle = fixture_bundle();
        bundle.pages[0].contract_version = 99;
        assert!(matches!(
            bundle.validate(),
            Err(UiContractError::ContractVersion { actual: 99, .. })
        ));
    }

    #[test]
    fn rejects_invalid_and_cyclic_runtime_bindings() {
        let mut invalid = fixture_bundle();
        invalid.pages[0].root.bindings.insert(
            "missing".into(),
            Binding::Field {
                field: "not_a_field".into(),
            },
        );
        assert!(matches!(
            invalid.validate(),
            Err(UiContractError::InvalidBinding {
                kind: "field",
                target,
                ..
            }) if target == "not_a_field"
        ));

        let mut cyclic = fixture_bundle();
        cyclic.pages[0].data_sources[0].parameters.insert(
            "cycle".into(),
            Binding::DataSource {
                data_source: "person-record".into(),
                path: "id".into(),
            },
        );
        assert!(matches!(
            cyclic.validate(),
            Err(UiContractError::DataSourceCycle { cycle, .. })
                if cycle == "person-record -> person-record"
        ));
    }

    #[test]
    fn record_resources_get_kernel_standard_fields() {
        let bundle = fixture_bundle();
        let person = &bundle.resources[0];
        assert!(person.is_record_resource());
        let fields = person.fields_with_standard();
        for name in STANDARD_FIELD_NAMES {
            let field = fields
                .iter()
                .find(|field| field.api_name == name)
                .expect("standard field present");
            assert!(!field.writable || field.source == FieldSource::Standard);
        }
        assert!(person.has_field("touched_at"));
        assert!(person.has_field("created_by"));
        // An organisation's own `name` is the standard name field.
        let organisation = &bundle.resources[1];
        let names: Vec<_> = organisation
            .fields_with_standard()
            .into_iter()
            .filter(|field| field.api_name == STANDARD_NAME_FIELD)
            .collect();
        assert_eq!(names.len(), 1);
        assert_eq!(names[0].source, FieldSource::Standard);
    }

    #[test]
    fn standard_fields_match_the_published_definition() {
        // The web app and integrators read this file; it must never drift
        // from the kernel's definition.
        let published: Vec<FieldDescriptor> = serde_json::from_str(include_str!(
            "../../../docs/architecture/standard-record-fields.json"
        ))
        .expect("published standard fields parse");
        assert_eq!(published, standard_fields());
        let names: Vec<_> = published
            .iter()
            .map(|field| field.api_name.as_str())
            .collect();
        assert_eq!(names, STANDARD_FIELD_NAMES);
    }

    #[test]
    fn resources_cannot_redeclare_standard_fields() {
        for reserved in ["created_by", "updated_at", "touched_at", "version", "id"] {
            let mut bundle = fixture_bundle();
            let mut field = bundle.resources[0].fields[0].clone();
            field.api_name = reserved.into();
            field.required = false;
            bundle.resources[0].fields.push(field);
            assert!(
                matches!(
                    bundle.validate(),
                    Err(UiContractError::ReservedFieldName { field, .. }) if field == reserved
                ),
                "{reserved} must be reserved"
            );
        }
        // `name` is reserved unless it is the resource's name field.
        let mut bundle = fixture_bundle();
        let mut field = bundle.resources[0].fields[0].clone();
        field.api_name = STANDARD_NAME_FIELD.into();
        bundle.resources[0].fields.push(field);
        assert!(matches!(
            bundle.validate(),
            Err(UiContractError::ReservedFieldName { .. })
        ));
        let mut system = fixture_bundle();
        system.resources[0].fields[1].source = FieldSource::System;
        assert!(matches!(
            system.validate(),
            Err(UiContractError::ReservedFieldName { .. })
        ));
    }

    #[test]
    fn record_resources_name_a_required_text_field() {
        let mut missing = fixture_bundle();
        missing.resources[0].name_field = None;
        assert!(matches!(
            missing.validate(),
            Err(UiContractError::InvalidNameField { .. })
        ));
        let mut unknown = fixture_bundle();
        unknown.resources[0].name_field = Some("nickname".into());
        assert!(matches!(
            unknown.validate(),
            Err(UiContractError::InvalidNameField { .. })
        ));
        let mut optional = fixture_bundle();
        optional.resources[0].fields[0].required = false;
        assert!(matches!(
            optional.validate(),
            Err(UiContractError::InvalidNameField { .. })
        ));
    }

    #[test]
    fn pages_may_bind_standard_fields_of_record_resources() {
        let mut bundle = fixture_bundle();
        bundle.pages[0].root.bindings.insert(
            "stamp".into(),
            Binding::Field {
                field: "touched_at".into(),
            },
        );
        bundle.validate().expect("standard fields are bindable");
    }

    #[test]
    fn unsupported_condition_operators_do_not_deserialize() {
        let encoded = r#"{"op":"execute-script","value":{"source":"literal","value":true}}"#;
        assert!(serde_json::from_str::<ConditionExpression>(encoded).is_err());
    }

    #[test]
    fn rejects_duplicate_nodes_and_dangling_resources() {
        let mut duplicate = fixture_bundle();
        let repeated = duplicate.pages[0].root.children[0].clone();
        duplicate.pages[0].root.children.push(repeated);
        assert!(matches!(
            duplicate.validate(),
            Err(UiContractError::DuplicateNode { .. })
        ));

        let mut dangling = fixture_bundle();
        dangling.pages[0].resource = Some("missing.resource".into());
        assert!(matches!(
            dangling.validate(),
            Err(UiContractError::DanglingReference {
                kind: "resource",
                ..
            })
        ));
    }

    #[test]
    fn rejects_custom_fields_without_the_explicit_suffix() {
        let mut bundle = fixture_bundle();
        bundle.resources[0]
            .fields
            .iter_mut()
            .find(|field| field.source == FieldSource::TenantCustom)
            .expect("custom field fixture")
            .api_name = "service_tier".into();
        assert!(matches!(
            bundle.validate(),
            Err(UiContractError::CustomFieldSuffix { .. })
        ));
    }

    #[test]
    fn rejects_nodes_that_escalate_the_page_host_profile() {
        let mut bundle = fixture_bundle();
        bundle.pages[0].root.children[0].hosts = vec![HostProfile::PublicForm];
        assert!(matches!(
            bundle.validate(),
            Err(UiContractError::HostEscalation { .. })
        ));
    }

    #[test]
    fn rejects_invalid_patch_payloads_and_missing_targets() {
        let mut invalid = fixture_bundle();
        invalid.extensions[0].node = None;
        assert!(matches!(
            invalid.validate(),
            Err(UiContractError::InvalidPatchPayload { .. })
        ));

        let mut missing = fixture_bundle();
        missing.extensions[0].target_node = "missing-slot".into();
        assert!(matches!(
            missing.validate(),
            Err(UiContractError::DanglingReference {
                kind: "page version or node",
                ..
            })
        ));
    }

    #[test]
    fn rejects_unknown_fields_relationships_and_operations() {
        let mut field = fixture_bundle();
        field.pages[0].root.children[0].children[0]
            .props
            .insert("field".into(), Value::String("missing_field".into()));
        assert!(matches!(
            field.validate(),
            Err(UiContractError::UnknownField { .. })
        ));

        let mut relationship = fixture_bundle();
        relationship.resources[0].relationships[0].target_resource = "missing.resource".into();
        assert!(matches!(
            relationship.validate(),
            Err(UiContractError::DanglingReference {
                kind: "relationship resource",
                ..
            })
        ));

        let mut operation = fixture_bundle();
        operation.pages[0].data_sources[0].operation = "missing".into();
        assert!(matches!(
            operation.validate(),
            Err(UiContractError::DanglingReference {
                kind: "resource operation",
                ..
            })
        ));
    }

    #[test]
    fn published_schema_documents_are_valid_json() {
        for schema in [
            include_str!("../../../docs/architecture/plugin-manifest.schema.json"),
            include_str!("../../../docs/architecture/ui-metadata.schema.json"),
        ] {
            let document: Value = serde_json::from_str(schema).expect("schema must be valid JSON");
            assert_eq!(
                document.get("$schema").and_then(Value::as_str),
                Some("https://json-schema.org/draft/2020-12/schema")
            );
        }
    }
}
