//! Compilation and opaque publication boundaries for portal and public-form UI.

use crate::ResolvedPage;
use plugin_contract::ui::{
    ActionKind, Binding, ConditionExpression, DataClassification, FieldDescriptor, HostProfile,
    ResourceDescriptor, UiNode,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalResourcePolicy {
    pub fields: BTreeSet<String>,
    pub operations: BTreeSet<String>,
    #[serde(default)]
    pub allow_personal_fields: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalDataSourcePolicy {
    pub resource: String,
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalActionPolicy {
    pub kind: ActionKind,
    pub target: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalHostPolicy {
    pub version: u32,
    pub host: HostProfile,
    pub components: BTreeSet<String>,
    pub resources: BTreeMap<String, ExternalResourcePolicy>,
    pub data_sources: BTreeMap<String, ExternalDataSourcePolicy>,
    pub actions: BTreeMap<String, ExternalActionPolicy>,
    pub fragments: BTreeSet<String>,
    pub context_paths: BTreeSet<String>,
    pub permissions: BTreeSet<String>,
    pub owner_plugins: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledExternalPage {
    pub host: HostProfile,
    pub policy_version: u32,
    pub page: ResolvedPage,
    pub resources: Vec<ResourceDescriptor>,
    pub dependency_plugins: BTreeSet<String>,
    /// Permissions explicitly approved as anonymous host capabilities.
    pub public_permissions: BTreeSet<String>,
    pub portal_permissions: BTreeSet<String>,
    pub content_hash: String,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExternalCompileError {
    #[error("host {0:?} is not an external host")]
    InternalHost(HostProfile),
    #[error("component {component} at node {node} is not allowed for {host:?}")]
    ComponentDenied {
        host: HostProfile,
        node: String,
        component: String,
    },
    #[error("node {node} is not enabled for host {host:?}")]
    NodeHostDenied { host: HostProfile, node: String },
    #[error("permission {permission} at {document} is not allowed for {host:?}")]
    PermissionDenied {
        host: HostProfile,
        document: String,
        permission: String,
    },
    #[error("context path {path} at {document} is not allowed for {host:?}")]
    ContextDenied {
        host: HostProfile,
        document: String,
        path: String,
    },
    #[error("field {field} on resource {resource} at {document} is not externally allowed")]
    FieldDenied {
        document: String,
        resource: String,
        field: String,
    },
    #[error(
        "field {field} on resource {resource} has forbidden classification {classification:?}"
    )]
    ClassificationDenied {
        resource: String,
        field: String,
        classification: DataClassification,
    },
    #[error("data source {0} is not externally allowed")]
    DataSourceDenied(String),
    #[error("data source {data_source} uses forbidden operation {resource}.{operation}")]
    DataSourceOperationDenied {
        data_source: String,
        resource: String,
        operation: String,
    },
    #[error("action {0} is not externally allowed")]
    ActionDenied(String),
    #[error("action {action} targets forbidden operation {target}")]
    ActionTargetDenied { action: String, target: String },
    #[error("action {action} has forbidden kind {kind:?} for {host:?}")]
    ActionKindDenied {
        host: HostProfile,
        action: String,
        kind: ActionKind,
    },
    #[error("fragment {fragment} at node {node} is not externally allowed")]
    FragmentDenied { node: String, fragment: String },
    #[error("plugin {0} is not allowed in the external dependency set")]
    PluginDenied(String),
    #[error("page resource {0} has no external policy")]
    ResourceDenied(String),
    #[error("external compilation hashing failed")]
    HashFailed,
}

pub fn compile_external_page(
    page: &ResolvedPage,
    resources: &[ResourceDescriptor],
    policy: &ExternalHostPolicy,
) -> Result<CompiledExternalPage, ExternalCompileError> {
    if !matches!(policy.host, HostProfile::Portal | HostProfile::PublicForm) {
        return Err(ExternalCompileError::InternalHost(policy.host.clone()));
    }
    let dependency_plugins: BTreeSet<_> = page.source_versions.keys().cloned().collect();
    if let Some(plugin) = dependency_plugins
        .iter()
        .find(|plugin| !policy.owner_plugins.contains(*plugin))
    {
        return Err(ExternalCompileError::PluginDenied(plugin.clone()));
    }
    if let Some(resource) = &page.resource {
        if !policy.resources.contains_key(resource) {
            return Err(ExternalCompileError::ResourceDenied(resource.clone()));
        }
    }
    let resources_by_id: BTreeMap<_, _> = resources
        .iter()
        .map(|resource| (resource.id.as_str(), resource))
        .collect();
    validate_data_sources(page, &resources_by_id, policy)?;
    validate_actions(page, policy)?;
    validate_node(&page.root, page.resource.as_deref(), policy)?;

    let mut safe_resources = Vec::new();
    for (resource_id, resource_policy) in &policy.resources {
        let descriptor = resources_by_id
            .get(resource_id.as_str())
            .ok_or_else(|| ExternalCompileError::ResourceDenied(resource_id.clone()))?;
        for field_name in &resource_policy.fields {
            let field = descriptor
                .fields
                .iter()
                .find(|field| &field.api_name == field_name)
                .ok_or_else(|| ExternalCompileError::FieldDenied {
                    document: "policy".into(),
                    resource: resource_id.clone(),
                    field: field_name.clone(),
                })?;
            validate_classification(resource_id, field, resource_policy)?;
        }
        let mut descriptor = (*descriptor).clone();
        descriptor
            .fields
            .retain(|field| resource_policy.fields.contains(&field.api_name));
        descriptor.operations.retain(|operation| {
            resource_policy.operations.contains(&operation.id)
                && operation.hosts.contains(&policy.host)
        });
        descriptor.relationships.clear();
        safe_resources.push(descriptor);
    }
    safe_resources.sort_by(|left, right| left.id.cmp(&right.id));
    let mut compiled = CompiledExternalPage {
        host: policy.host.clone(),
        policy_version: policy.version,
        page: page.clone(),
        resources: safe_resources,
        dependency_plugins,
        public_permissions: if policy.host == HostProfile::PublicForm {
            policy.permissions.clone()
        } else {
            BTreeSet::new()
        },
        portal_permissions: if policy.host == HostProfile::Portal {
            policy.permissions.clone()
        } else {
            BTreeSet::new()
        },
        content_hash: String::new(),
    };
    compiled.content_hash = stable_hash(&compiled)?;
    Ok(compiled)
}

fn validate_data_sources(
    page: &ResolvedPage,
    resources: &BTreeMap<&str, &ResourceDescriptor>,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    for data_source in &page.data_sources {
        let Some(data_source_policy) = policy.data_sources.get(&data_source.id) else {
            return Err(ExternalCompileError::DataSourceDenied(
                data_source.id.clone(),
            ));
        };
        if data_source_policy.resource != data_source.resource
            || data_source_policy.operation != data_source.operation
        {
            return Err(ExternalCompileError::DataSourceOperationDenied {
                data_source: data_source.id.clone(),
                resource: data_source.resource.clone(),
                operation: data_source.operation.clone(),
            });
        }
        let allowed = policy.resources.get(&data_source.resource);
        if !allowed.is_some_and(|resource| resource.operations.contains(&data_source.operation)) {
            return Err(ExternalCompileError::DataSourceOperationDenied {
                data_source: data_source.id.clone(),
                resource: data_source.resource.clone(),
                operation: data_source.operation.clone(),
            });
        }
        let descriptor = resources
            .get(data_source.resource.as_str())
            .ok_or_else(|| ExternalCompileError::ResourceDenied(data_source.resource.clone()))?;
        if !descriptor.operations.iter().any(|operation| {
            operation.id == data_source.operation && operation.hosts.contains(&policy.host)
        }) {
            return Err(ExternalCompileError::DataSourceOperationDenied {
                data_source: data_source.id.clone(),
                resource: data_source.resource.clone(),
                operation: data_source.operation.clone(),
            });
        }
        for binding in data_source.parameters.values() {
            validate_binding(
                binding,
                &format!("data-source:{}", data_source.id),
                page.resource.as_deref(),
                policy,
            )?;
        }
    }
    Ok(())
}

fn validate_actions(
    page: &ResolvedPage,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    for action in &page.actions {
        let Some(action_policy) = policy.actions.get(&action.id) else {
            return Err(ExternalCompileError::ActionDenied(action.id.clone()));
        };
        if action_policy.kind != action.kind {
            return Err(ExternalCompileError::ActionKindDenied {
                host: policy.host.clone(),
                action: action.id.clone(),
                kind: action.kind.clone(),
            });
        }
        if action_policy.target != action.target {
            return Err(ExternalCompileError::ActionTargetDenied {
                action: action.id.clone(),
                target: action.target.clone(),
            });
        }
        if policy.host == HostProfile::PublicForm
            && !matches!(
                action.kind,
                ActionKind::Submit | ActionKind::ResourceOperation
            )
        {
            return Err(ExternalCompileError::ActionKindDenied {
                host: policy.host.clone(),
                action: action.id.clone(),
                kind: action.kind.clone(),
            });
        }
        validate_permission(
            action.permission.as_deref(),
            &format!("action:{}", action.id),
            policy,
        )?;
        if action.kind == ActionKind::ResourceOperation
            && !operation_target_allowed(&action.target, policy)
        {
            return Err(ExternalCompileError::ActionTargetDenied {
                action: action.id.clone(),
                target: action.target.clone(),
            });
        }
        for binding in action.parameters.values() {
            validate_binding(
                binding,
                &format!("action:{}", action.id),
                page.resource.as_deref(),
                policy,
            )?;
        }
    }
    Ok(())
}

fn validate_node(
    node: &UiNode,
    page_resource: Option<&str>,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    if !policy.components.contains(&node.component) {
        return Err(ExternalCompileError::ComponentDenied {
            host: policy.host.clone(),
            node: node.id.clone(),
            component: node.component.clone(),
        });
    }
    if !node.hosts.contains(&policy.host) {
        return Err(ExternalCompileError::NodeHostDenied {
            host: policy.host.clone(),
            node: node.id.clone(),
        });
    }
    validate_permission(
        node.permission.as_deref(),
        &format!("node:{}", node.id),
        policy,
    )?;
    for binding in node.bindings.values() {
        validate_binding(binding, &format!("node:{}", node.id), page_resource, policy)?;
    }
    for condition in node.conditions.values() {
        validate_condition(
            condition,
            &format!("node:{}", node.id),
            page_resource,
            policy,
        )?;
    }
    if let Some(field) = node.props.get("field").and_then(|value| value.as_str()) {
        validate_field(field, &format!("node:{}", node.id), page_resource, policy)?;
    }
    if let Some(action) = node.props.get("action_id").and_then(|value| value.as_str()) {
        if !policy.actions.contains_key(action) {
            return Err(ExternalCompileError::ActionDenied(action.into()));
        }
    }
    if let Some(fragment) = node
        .props
        .get("fragment_id")
        .and_then(|value| value.as_str())
    {
        if !policy.fragments.contains(fragment) {
            return Err(ExternalCompileError::FragmentDenied {
                node: node.id.clone(),
                fragment: fragment.into(),
            });
        }
    }
    for child in &node.children {
        validate_node(child, page_resource, policy)?;
    }
    Ok(())
}

fn validate_condition(
    condition: &ConditionExpression,
    document: &str,
    page_resource: Option<&str>,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    match condition {
        ConditionExpression::And { args } | ConditionExpression::Or { args } => {
            for condition in args {
                validate_condition(condition, document, page_resource, policy)?;
            }
        }
        ConditionExpression::Not { arg } => {
            validate_condition(arg, document, page_resource, policy)?;
        }
        ConditionExpression::Equal { left, right }
        | ConditionExpression::NotEqual { left, right } => {
            validate_binding(left, document, page_resource, policy)?;
            validate_binding(right, document, page_resource, policy)?;
        }
        ConditionExpression::Exists { value } => {
            validate_binding(value, document, page_resource, policy)?;
        }
        ConditionExpression::In { value, set } => {
            validate_binding(value, document, page_resource, policy)?;
            for binding in set {
                validate_binding(binding, document, page_resource, policy)?;
            }
        }
        ConditionExpression::HasPermission { permission } => {
            validate_permission(Some(permission), document, policy)?;
        }
    }
    Ok(())
}

fn validate_binding(
    binding: &Binding,
    document: &str,
    page_resource: Option<&str>,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    match binding {
        Binding::Field { field } => validate_field(field, document, page_resource, policy),
        Binding::Context { path } => {
            if policy.context_paths.contains(path) {
                Ok(())
            } else {
                Err(ExternalCompileError::ContextDenied {
                    host: policy.host.clone(),
                    document: document.into(),
                    path: path.clone(),
                })
            }
        }
        Binding::DataSource { data_source, .. } => {
            if policy.data_sources.contains_key(data_source) {
                Ok(())
            } else {
                Err(ExternalCompileError::DataSourceDenied(data_source.clone()))
            }
        }
        Binding::Literal { .. } => Ok(()),
    }
}

fn validate_field(
    field: &str,
    document: &str,
    page_resource: Option<&str>,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    let resource = page_resource.unwrap_or("<none>");
    if policy
        .resources
        .get(resource)
        .is_some_and(|allowed| allowed.fields.contains(field))
    {
        Ok(())
    } else {
        Err(ExternalCompileError::FieldDenied {
            document: document.into(),
            resource: resource.into(),
            field: field.into(),
        })
    }
}

fn validate_permission(
    permission: Option<&str>,
    document: &str,
    policy: &ExternalHostPolicy,
) -> Result<(), ExternalCompileError> {
    let Some(permission) = permission else {
        return Ok(());
    };
    if policy.host == HostProfile::Portal && policy.permissions.contains(permission) {
        Ok(())
    } else {
        Err(ExternalCompileError::PermissionDenied {
            host: policy.host.clone(),
            document: document.into(),
            permission: permission.into(),
        })
    }
}

fn operation_target_allowed(target: &str, policy: &ExternalHostPolicy) -> bool {
    policy.resources.iter().any(|(resource, allowed)| {
        allowed
            .operations
            .iter()
            .any(|operation| target == format!("{resource}.{operation}") || target == operation)
    })
}

fn classification_allowed(
    classification: Option<&DataClassification>,
    allow_personal: bool,
) -> bool {
    matches!(classification, None | Some(DataClassification::Public))
        || (allow_personal && classification == Some(&DataClassification::Personal))
}

fn validate_classification(
    resource: &str,
    field: &FieldDescriptor,
    policy: &ExternalResourcePolicy,
) -> Result<(), ExternalCompileError> {
    if classification_allowed(field.classification.as_ref(), policy.allow_personal_fields) {
        Ok(())
    } else {
        Err(ExternalCompileError::ClassificationDenied {
            resource: resource.into(),
            field: field.api_name.clone(),
            classification: field
                .classification
                .clone()
                .unwrap_or(DataClassification::Internal),
        })
    }
}

fn stable_hash<T: Serialize>(value: &T) -> Result<String, ExternalCompileError> {
    serde_json::to_vec(value)
        .map(|bytes| format!("{:x}", Sha256::digest(bytes)))
        .map_err(|_| ExternalCompileError::HashFailed)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpaqueSlug(String);

impl Serialize for OpaqueSlug {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for OpaqueSlug {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SlugError {
    #[error("external publication slug is invalid")]
    Invalid,
}

impl OpaqueSlug {
    pub fn parse(value: impl Into<String>) -> Result<Self, SlugError> {
        let value = value.into();
        if (32..=96).contains(&value.len())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            Ok(Self(value))
        } else {
            Err(SlugError::Invalid)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the one-way lookup key used by external publication storage.
    /// The original capability slug must never be persisted.
    pub fn lookup_digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.0.as_bytes()))
    }

    pub(crate) fn digest(&self) -> String {
        self.lookup_digest()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalPublication {
    tenant_id: String,
    portal_audience: Option<String>,
    pub compiled: CompiledExternalPage,
    pub generation: u64,
    pub enabled_plugins: BTreeSet<String>,
    pub revoked: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ExternalPublicationRegistry {
    publications: BTreeMap<String, ExternalPublication>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ExternalPublicationError {
    #[error("external publication already exists")]
    Duplicate,
    #[error("external publication is unavailable")]
    Unavailable,
    #[error("external publication dependencies are not enabled")]
    DependenciesUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalPrincipal {
    pub subject: String,
    pub tenant_id: String,
    pub audience: String,
    pub permissions: BTreeSet<String>,
    pub session_version: u64,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalGrant {
    pub subject: String,
    pub tenant_id: String,
    pub audience: String,
    pub permissions: BTreeSet<String>,
    pub session_version: u64,
}

impl ExternalPublicationRegistry {
    pub fn publish(
        &mut self,
        slug: &OpaqueSlug,
        tenant_id: impl Into<String>,
        portal_audience: Option<String>,
        compiled: CompiledExternalPage,
        generation: u64,
        enabled_plugins: BTreeSet<String>,
    ) -> Result<(), ExternalPublicationError> {
        if !compiled
            .dependency_plugins
            .iter()
            .all(|plugin| enabled_plugins.contains(plugin))
        {
            return Err(ExternalPublicationError::DependenciesUnavailable);
        }
        let key = slug.digest();
        if self.publications.contains_key(&key) {
            return Err(ExternalPublicationError::Duplicate);
        }
        self.publications.insert(
            key,
            ExternalPublication {
                tenant_id: tenant_id.into(),
                portal_audience,
                compiled,
                generation,
                enabled_plugins,
                revoked: false,
            },
        );
        Ok(())
    }

    pub fn resolve(
        &self,
        slug: &OpaqueSlug,
        host: HostProfile,
        minimum_generation: u64,
    ) -> Result<&CompiledExternalPage, ExternalPublicationError> {
        self.publications
            .get(&slug.digest())
            .filter(|publication| {
                !publication.revoked
                    && publication.compiled.host == host
                    && publication.generation >= minimum_generation
                    && publication
                        .compiled
                        .dependency_plugins
                        .iter()
                        .all(|plugin| publication.enabled_plugins.contains(plugin))
            })
            .map(|publication| &publication.compiled)
            .ok_or(ExternalPublicationError::Unavailable)
    }

    pub fn revoke(&mut self, slug: &OpaqueSlug) -> Result<(), ExternalPublicationError> {
        let publication = self
            .publications
            .get_mut(&slug.digest())
            .ok_or(ExternalPublicationError::Unavailable)?;
        publication.revoked = true;
        Ok(())
    }

    pub fn authorize_portal(
        &self,
        slug: &OpaqueSlug,
        principal: &PortalPrincipal,
        minimum_generation: u64,
    ) -> Result<PortalGrant, ExternalPublicationError> {
        let publication = self
            .publication(slug, HostProfile::Portal, minimum_generation)
            .filter(|publication| {
                principal.active
                    && principal.session_version > 0
                    && !principal.subject.is_empty()
                    && publication.tenant_id == principal.tenant_id
                    && publication.portal_audience.as_deref() == Some(&principal.audience)
            })
            .ok_or(ExternalPublicationError::Unavailable)?;
        Ok(PortalGrant {
            subject: principal.subject.clone(),
            tenant_id: principal.tenant_id.clone(),
            audience: principal.audience.clone(),
            permissions: principal
                .permissions
                .intersection(&publication.compiled.portal_permissions)
                .cloned()
                .collect(),
            session_version: principal.session_version,
        })
    }

    pub fn disable_plugin(&mut self, plugin_id: &str) {
        for publication in self.publications.values_mut() {
            publication.enabled_plugins.remove(plugin_id);
        }
    }

    pub(crate) fn public_grant(
        &self,
        slug: &OpaqueSlug,
        minimum_generation: u64,
    ) -> Result<&str, ExternalPublicationError> {
        self.publication(slug, HostProfile::PublicForm, minimum_generation)
            .map(|publication| publication.tenant_id.as_str())
            .ok_or(ExternalPublicationError::Unavailable)
    }

    fn publication(
        &self,
        slug: &OpaqueSlug,
        host: HostProfile,
        minimum_generation: u64,
    ) -> Option<&ExternalPublication> {
        self.publications.get(&slug.digest()).filter(|publication| {
            !publication.revoked
                && publication.compiled.host == host
                && publication.generation >= minimum_generation
                && publication
                    .compiled
                    .dependency_plugins
                    .iter()
                    .all(|plugin| publication.enabled_plugins.contains(plugin))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contract::ui::{
        ActionDefinition, DataSourceDefinition, FieldSource, FieldType, LoadPolicy, OperationKind,
        ResourceOperation,
    };
    use serde_json::json;

    const PAGE_ID: &str = "platform.forms.contact";
    const RESOURCE_ID: &str = "platform.contacts.person";

    fn field(
        name: &str,
        classification: DataClassification,
    ) -> plugin_contract::ui::FieldDescriptor {
        plugin_contract::ui::FieldDescriptor {
            api_name: name.into(),
            label: name.into(),
            description: None,
            field_type: FieldType::Text,
            source: FieldSource::Standard,
            readable: true,
            writable: true,
            required: false,
            permission: None,
            classification: Some(classification),
            options: Vec::new(),
            validation_schema: None,
            default_value: None,
            submit_when_hidden: false,
        }
    }

    fn resource(host: HostProfile) -> ResourceDescriptor {
        ResourceDescriptor {
            contract_version: 1,
            id: RESOURCE_ID.into(),
            definition_version: 1,
            owner_plugin: "platform.contacts".into(),
            label: "Person".into(),
            name_field: None,
            fields: vec![
                field("email", DataClassification::Personal),
                field("source", DataClassification::Public),
                field("internal_score", DataClassification::Internal),
                field("secret", DataClassification::Sensitive),
            ],
            relationships: Vec::new(),
            operations: vec![
                ResourceOperation {
                    id: "create-public".into(),
                    kind: OperationKind::Create,
                    permission: "contacts.public.submit".into(),
                    concurrency: None,
                    hosts: vec![host],
                },
                ResourceOperation {
                    id: "internal-update".into(),
                    kind: OperationKind::Update,
                    permission: "contacts.people.write".into(),
                    concurrency: None,
                    hosts: vec![HostProfile::InternalWeb],
                },
            ],
        }
    }

    fn node(host: HostProfile) -> UiNode {
        UiNode {
            id: "page-root".into(),
            component: "platform.ui.form".into(),
            props: BTreeMap::from([("fragment_id".into(), json!("platform.fragments.consent"))]),
            bindings: BTreeMap::from([(
                "email".into(),
                Binding::Field {
                    field: "email".into(),
                },
            )]),
            conditions: BTreeMap::new(),
            children: vec![UiNode {
                id: "submit".into(),
                component: "platform.ui.action".into(),
                props: BTreeMap::from([("action_id".into(), json!("submit"))]),
                bindings: BTreeMap::new(),
                conditions: BTreeMap::new(),
                children: Vec::new(),
                permission: None,
                hosts: vec![host.clone()],
            }],
            permission: None,
            hosts: vec![host],
        }
    }

    fn page(host: HostProfile) -> ResolvedPage {
        ResolvedPage {
            page_id: PAGE_ID.into(),
            definition_version: 1,
            selected_variant: None,
            source_versions: BTreeMap::from([("platform.contacts".into(), "1.0.0".into())]),
            publication_versions: vec![1],
            resource: Some(RESOURCE_ID.into()),
            data_sources: vec![DataSourceDefinition {
                id: "defaults".into(),
                resource: RESOURCE_ID.into(),
                operation: "create-public".into(),
                parameters: BTreeMap::from([(
                    "source".into(),
                    Binding::Context {
                        path: "public.source".into(),
                    },
                )]),
                load: LoadPolicy::Explicit,
            }],
            actions: vec![ActionDefinition {
                id: "submit".into(),
                kind: ActionKind::Submit,
                target: "submit".into(),
                parameters: BTreeMap::new(),
                permission: None,
            }],
            root: node(host),
            content_hash: "resolved".into(),
        }
    }

    fn policy(host: HostProfile) -> ExternalHostPolicy {
        ExternalHostPolicy {
            version: 1,
            host,
            components: BTreeSet::from(["platform.ui.form".into(), "platform.ui.action".into()]),
            resources: BTreeMap::from([(
                RESOURCE_ID.into(),
                ExternalResourcePolicy {
                    fields: BTreeSet::from(["email".into(), "source".into()]),
                    operations: BTreeSet::from(["create-public".into()]),
                    allow_personal_fields: true,
                },
            )]),
            data_sources: BTreeMap::from([(
                "defaults".into(),
                ExternalDataSourcePolicy {
                    resource: RESOURCE_ID.into(),
                    operation: "create-public".into(),
                },
            )]),
            actions: BTreeMap::from([(
                "submit".into(),
                ExternalActionPolicy {
                    kind: ActionKind::Submit,
                    target: "submit".into(),
                },
            )]),
            fragments: BTreeSet::from(["platform.fragments.consent".into()]),
            context_paths: BTreeSet::from(["public.source".into()]),
            permissions: BTreeSet::new(),
            owner_plugins: BTreeSet::from(["platform.contacts".into()]),
        }
    }

    #[test]
    fn public_compilation_keeps_only_explicitly_allowed_surface() {
        let compiled = compile_external_page(
            &page(HostProfile::PublicForm),
            &[resource(HostProfile::PublicForm)],
            &policy(HostProfile::PublicForm),
        )
        .unwrap();
        assert_eq!(compiled.host, HostProfile::PublicForm);
        assert_eq!(
            compiled.resources[0]
                .fields
                .iter()
                .map(|field| field.api_name.as_str())
                .collect::<Vec<_>>(),
            vec!["email", "source"]
        );
        assert_eq!(compiled.resources[0].operations.len(), 1);
        assert!(compiled.resources[0].relationships.is_empty());
        assert_eq!(compiled.content_hash.len(), 64);
    }

    #[test]
    fn internal_components_fields_data_actions_fragments_and_context_fail_compilation() {
        let resources = [resource(HostProfile::PublicForm)];
        let base_page = page(HostProfile::PublicForm);
        let base_policy = policy(HostProfile::PublicForm);

        let mut candidate = base_page.clone();
        candidate.root.component = "platform.ui.internal-admin".into();
        assert!(matches!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::ComponentDenied { .. })
        ));

        let mut candidate = base_page.clone();
        candidate.root.bindings.insert(
            "secret".into(),
            Binding::Field {
                field: "secret".into(),
            },
        );
        assert!(matches!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::FieldDenied { .. })
        ));

        let mut candidate = base_page.clone();
        candidate.data_sources[0].id = "internal-query".into();
        assert_eq!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::DataSourceDenied(
                "internal-query".into()
            ))
        );

        let mut candidate = base_page.clone();
        candidate.data_sources[0].operation = "internal-update".into();
        assert!(matches!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::DataSourceOperationDenied { .. })
        ));

        let mut candidate = base_page.clone();
        candidate.actions[0].id = "internal-action".into();
        assert_eq!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::ActionDenied("internal-action".into()))
        );

        let mut candidate = base_page.clone();
        candidate.actions[0].target = "/internal/admin".into();
        assert!(matches!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::ActionTargetDenied { .. })
        ));

        let mut candidate = base_page.clone();
        candidate
            .root
            .props
            .insert("fragment_id".into(), json!("platform.fragments.internal"));
        assert!(matches!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::FragmentDenied { .. })
        ));

        let mut candidate = base_page;
        candidate.data_sources[0].parameters.insert(
            "source".into(),
            Binding::Context {
                path: "session.internal_token".into(),
            },
        );
        assert!(matches!(
            compile_external_page(&candidate, &resources, &base_policy),
            Err(ExternalCompileError::ContextDenied { .. })
        ));
    }

    #[test]
    fn classifications_permissions_plugins_and_host_escalation_fail_closed() {
        let mut internal_field_policy = policy(HostProfile::PublicForm);
        internal_field_policy
            .resources
            .get_mut(RESOURCE_ID)
            .unwrap()
            .fields = BTreeSet::from(["email".into(), "internal_score".into()]);
        assert!(matches!(
            compile_external_page(
                &page(HostProfile::PublicForm),
                &[resource(HostProfile::PublicForm)],
                &internal_field_policy
            ),
            Err(ExternalCompileError::ClassificationDenied { .. })
        ));

        let mut permission_page = page(HostProfile::PublicForm);
        permission_page.root.permission = Some("contacts.people.read".into());
        let mut permission_policy = policy(HostProfile::PublicForm);
        permission_policy
            .permissions
            .insert("contacts.people.read".into());
        assert!(matches!(
            compile_external_page(
                &permission_page,
                &[resource(HostProfile::PublicForm)],
                &permission_policy
            ),
            Err(ExternalCompileError::PermissionDenied { .. })
        ));

        let mut plugin_page = page(HostProfile::PublicForm);
        plugin_page
            .source_versions
            .insert("customer.untrusted".into(), "1.0.0".into());
        assert_eq!(
            compile_external_page(
                &plugin_page,
                &[resource(HostProfile::PublicForm)],
                &policy(HostProfile::PublicForm)
            ),
            Err(ExternalCompileError::PluginDenied(
                "customer.untrusted".into()
            ))
        );

        let mut wrong_host = page(HostProfile::PublicForm);
        wrong_host.root.hosts = vec![HostProfile::InternalWeb];
        assert!(matches!(
            compile_external_page(
                &wrong_host,
                &[resource(HostProfile::PublicForm)],
                &policy(HostProfile::PublicForm)
            ),
            Err(ExternalCompileError::NodeHostDenied { .. })
        ));
    }

    #[test]
    fn portal_allows_only_declared_portal_permissions_and_operations() {
        let mut portal_page = page(HostProfile::Portal);
        portal_page.root.permission = Some("portal.contact.submit".into());
        let mut portal_policy = policy(HostProfile::Portal);
        portal_policy
            .permissions
            .insert("portal.contact.submit".into());
        let compiled = compile_external_page(
            &portal_page,
            &[resource(HostProfile::Portal)],
            &portal_policy,
        )
        .unwrap();
        assert_eq!(compiled.host, HostProfile::Portal);

        portal_page.root.permission = Some("contacts.people.read".into());
        assert!(matches!(
            compile_external_page(
                &portal_page,
                &[resource(HostProfile::Portal)],
                &portal_policy
            ),
            Err(ExternalCompileError::PermissionDenied { .. })
        ));
    }

    #[test]
    fn opaque_registry_hides_lookup_state_and_invalidates_revocation_plugin_and_generation() {
        assert_eq!(OpaqueSlug::parse("short"), Err(SlugError::Invalid));
        let slug = OpaqueSlug::parse("K8f_z20sRmP4nY7wQ2aB9cD6eF1hJ3kL").unwrap();
        let unknown = OpaqueSlug::parse("Z9x_y21tSnQ5oZ8xR3bC0dE7fG2iK4mN").unwrap();
        let compiled = compile_external_page(
            &page(HostProfile::PublicForm),
            &[resource(HostProfile::PublicForm)],
            &policy(HostProfile::PublicForm),
        )
        .unwrap();
        let mut registry = ExternalPublicationRegistry::default();
        registry
            .publish(
                &slug,
                "tenant-a",
                None,
                compiled.clone(),
                4,
                BTreeSet::from(["platform.contacts".into()]),
            )
            .unwrap();
        assert_eq!(
            registry.resolve(&unknown, HostProfile::PublicForm, 4),
            Err(ExternalPublicationError::Unavailable)
        );
        assert_eq!(
            registry.resolve(&slug, HostProfile::Portal, 4),
            Err(ExternalPublicationError::Unavailable)
        );
        assert_eq!(
            registry.resolve(&slug, HostProfile::PublicForm, 5),
            Err(ExternalPublicationError::Unavailable)
        );
        assert_eq!(
            registry
                .resolve(&slug, HostProfile::PublicForm, 4)
                .unwrap()
                .content_hash,
            compiled.content_hash
        );
        registry.disable_plugin("platform.contacts");
        assert_eq!(
            registry.resolve(&slug, HostProfile::PublicForm, 4),
            Err(ExternalPublicationError::Unavailable)
        );

        let other_slug = OpaqueSlug::parse("A7e_x10rQlN3mW5uP8zC2bD9fH4jK6sT").unwrap();
        registry
            .publish(
                &other_slug,
                "tenant-a",
                None,
                compiled,
                5,
                BTreeSet::from(["platform.contacts".into()]),
            )
            .unwrap();
        registry.revoke(&other_slug).unwrap();
        assert_eq!(
            registry.resolve(&other_slug, HostProfile::PublicForm, 5),
            Err(ExternalPublicationError::Unavailable)
        );
    }

    #[test]
    fn portal_grants_bind_tenant_audience_session_and_allowlisted_permissions() {
        let slug = OpaqueSlug::parse("P7r_t10aLmN3vW5uQ8zC2bD9fH4jK6sX").unwrap();
        let mut portal_policy = policy(HostProfile::Portal);
        portal_policy.permissions =
            BTreeSet::from(["portal.contact.read".into(), "portal.contact.submit".into()]);
        let compiled = compile_external_page(
            &page(HostProfile::Portal),
            &[resource(HostProfile::Portal)],
            &portal_policy,
        )
        .unwrap();
        let mut registry = ExternalPublicationRegistry::default();
        registry
            .publish(
                &slug,
                "tenant-a",
                Some("limiara-portal".into()),
                compiled,
                3,
                BTreeSet::from(["platform.contacts".into()]),
            )
            .unwrap();
        let principal = PortalPrincipal {
            subject: "portal-user-1".into(),
            tenant_id: "tenant-a".into(),
            audience: "limiara-portal".into(),
            permissions: BTreeSet::from(["portal.contact.read".into(), "internal.admin".into()]),
            session_version: 4,
            active: true,
        };
        let grant = registry.authorize_portal(&slug, &principal, 3).unwrap();
        assert_eq!(grant.tenant_id, "tenant-a");
        assert_eq!(
            grant.permissions,
            BTreeSet::from(["portal.contact.read".into()])
        );

        for denied in [
            PortalPrincipal {
                tenant_id: "tenant-b".into(),
                ..principal.clone()
            },
            PortalPrincipal {
                audience: "internal-web".into(),
                ..principal.clone()
            },
            PortalPrincipal {
                session_version: 0,
                ..principal.clone()
            },
            PortalPrincipal {
                active: false,
                ..principal.clone()
            },
        ] {
            assert_eq!(
                registry.authorize_portal(&slug, &denied, 3),
                Err(ExternalPublicationError::Unavailable)
            );
        }
    }
}
