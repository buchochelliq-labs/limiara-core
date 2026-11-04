#![forbid(unsafe_code)]

//! Permission-filtered delivery, conditional requests, and cache isolation for
//! resolved metadata UI pages.

use plugin_contract::ui::{
    ActionDefinition, ActionKind, Binding, ConcurrencyMode, ConditionExpression, ConditionalState,
    DataClassification, FieldDescriptor, FieldOption, FieldSource, FieldType, HostProfile,
    OperationKind, RelationshipCardinality, ResourceDescriptor, UiNode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Instant,
};
use thiserror::Error;
use ui_metadata::{
    external_host::CompiledExternalPage, publication::MANAGE_PERMISSION, MetadataCatalogue,
    PublishedPatch, ResolveContext, ResolveError, ResolvedPage,
};

/// Apply the same permission and reference filtering used by internal delivery
/// to an already restricted external publication.
pub fn deliver_external_page(
    compiled: &CompiledExternalPage,
    granted_permissions: &BTreeSet<String>,
) -> Result<CallerSafePageResponse, UiDeliveryError> {
    let approved_permissions = match compiled.host {
        HostProfile::PublicForm => &compiled.public_permissions,
        HostProfile::Portal => &compiled.portal_permissions,
        HostProfile::InternalWeb | HostProfile::Desktop => return Err(page_unavailable()),
    };
    if !granted_permissions.is_subset(approved_permissions) {
        return Err(page_unavailable());
    }
    let caller = UiCaller {
        user_id: "external-host".into(),
        workspace_id: "external-publication".into(),
        active_membership: true,
        membership_version: 1,
        roles: BTreeSet::new(),
        permissions: granted_permissions.clone(),
    };
    let context = ResolveContext {
        host: compiled.host.clone(),
        mode: "view".into(),
        form_factor: "wide".into(),
        audience: None,
        roles: BTreeSet::new(),
        resource_subtype: None,
        locale: "en-GB".into(),
        permissions: granted_permissions.clone(),
        values: BTreeMap::new(),
    };
    filter_page(
        compiled.page.clone(),
        &compiled.resources,
        &caller,
        &context,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiCaller {
    pub user_id: String,
    pub workspace_id: String,
    pub active_membership: bool,
    pub membership_version: u64,
    pub roles: BTreeSet<String>,
    pub permissions: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiPageRequest {
    pub workspace_id: String,
    pub page_id: String,
    pub locale: String,
    pub host: HostProfile,
    pub mode: String,
    pub form_factor: String,
    pub audience: Option<String>,
    pub resource_subtype: Option<String>,
    pub record_context: BTreeMap<String, String>,
    pub if_none_match: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiResourceRequest {
    pub workspace_id: String,
    pub resource_id: String,
    pub host: HostProfile,
    pub locale: String,
    pub if_none_match: Option<String>,
}

#[derive(Debug, Clone)]
pub struct UiWorkspaceSnapshot {
    pub catalogue: Arc<MetadataCatalogue>,
    pub resources: Vec<ResourceDescriptor>,
    pub tenant_patches: Vec<PublishedPatch>,
    pub catalogue_generation: u64,
    pub publication_generation: u64,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SnapshotError {
    #[error("workspace UI snapshot is hidden")]
    Hidden,
    #[error("workspace UI snapshot is incompatible")]
    Incompatible,
    #[error("workspace UI snapshot is conflicted")]
    Conflict,
    #[error("workspace UI snapshot is temporarily unavailable")]
    TemporarilyUnavailable,
}

pub trait UiSnapshotSource: Send + Sync {
    fn snapshot(&self, workspace_id: &str) -> Result<UiWorkspaceSnapshot, SnapshotError>;
    fn is_ready(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UiFailureCategory {
    PageUnavailable,
    Incompatible,
    Conflict,
    TemporarilyUnavailable,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("UI delivery failed: {category:?}")]
pub struct UiDeliveryError {
    pub category: UiFailureCategory,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("UI draft inspection failed: {diagnostic}")]
pub struct UiInspectionError {
    pub category: UiFailureCategory,
    pub diagnostic: String,
}

impl UiDeliveryError {
    pub fn code(&self) -> &'static str {
        match self.category {
            UiFailureCategory::PageUnavailable => "ui.page_unavailable",
            UiFailureCategory::Incompatible => "ui.incompatible",
            UiFailureCategory::Conflict => "ui.conflicted",
            UiFailureCategory::TemporarilyUnavailable => "ui.temporarily_unavailable",
        }
    }

    pub fn public_detail(&self) -> &'static str {
        match self.category {
            UiFailureCategory::PageUnavailable => "The requested UI is unavailable.",
            UiFailureCategory::Incompatible => "The requested UI version is unavailable.",
            UiFailureCategory::Conflict => "The requested UI cannot currently be resolved.",
            UiFailureCategory::TemporarilyUnavailable => "UI delivery is temporarily unavailable.",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryStatus {
    Fresh,
    NotModified,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Delivery<T> {
    pub status: DeliveryStatus,
    pub body: Option<T>,
    pub etag: String,
    pub cache_control: &'static str,
    pub cache_hit: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallerSafePageResponse {
    pub page: ResolvedPage,
    pub resources: Vec<CallerSafeResourceDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallerSafeResourceResponse {
    pub resource: CallerSafeResourceDescriptor,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallerSafeResourceDescriptor {
    pub id: String,
    pub definition_version: u32,
    pub owner_plugin: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_field: Option<String>,
    pub fields: Vec<CallerSafeFieldDescriptor>,
    pub relationships: Vec<CallerSafeRelationshipDescriptor>,
    pub operations: Vec<CallerSafeOperationDescriptor>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CallerSafeFieldDescriptor {
    pub api_name: String,
    pub label: String,
    pub description: Option<String>,
    pub field_type: FieldType,
    pub source: FieldSource,
    pub readable: bool,
    pub writable: bool,
    pub required: bool,
    pub classification: Option<DataClassification>,
    pub options: Vec<FieldOption>,
    pub validation_schema: Option<Value>,
    pub default_value: Option<Value>,
    pub submit_when_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerSafeRelationshipDescriptor {
    pub id: String,
    pub target_resource: String,
    pub cardinality: RelationshipCardinality,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerSafeOperationDescriptor {
    pub id: String,
    pub kind: OperationKind,
    pub concurrency: Option<ConcurrencyMode>,
    pub hosts: Vec<HostProfile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryKind {
    Page,
    Resource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryObservation {
    pub kind: DeliveryKind,
    pub outcome: Result<DeliveryStatus, UiFailureCategory>,
    pub cache_hit: bool,
    pub definition_version: Option<u32>,
    pub elapsed_micros: u64,
}

pub trait DeliveryObserver: Send + Sync {
    fn record(&self, observation: DeliveryObservation);
}

#[derive(Default)]
pub struct NoopDeliveryObserver;

impl DeliveryObserver for NoopDeliveryObserver {
    fn record(&self, _observation: DeliveryObservation) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UiDeliveryReadiness {
    pub ready: bool,
    pub cache_entries: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum CacheTarget {
    Page(String),
    Resource(String),
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct CacheKey {
    workspace_id: String,
    target: CacheTarget,
    security_fingerprint: String,
    locale: String,
    host: HostProfile,
    mode: String,
    form_factor: String,
    audience: Option<String>,
    resource_subtype: Option<String>,
    record_context: BTreeMap<String, String>,
    catalogue_generation: u64,
    publication_generation: u64,
}

#[derive(Debug, Clone)]
enum CachedPayload {
    Page(Box<CallerSafePageResponse>),
    Resource(Box<CallerSafeResourceResponse>),
}

#[derive(Debug, Clone)]
struct CacheEntry {
    etag: String,
    payload: CachedPayload,
}

pub struct UiDeliveryService {
    source: Arc<dyn UiSnapshotSource>,
    observer: Arc<dyn DeliveryObserver>,
    cache: Mutex<BTreeMap<CacheKey, CacheEntry>>,
}

const PRIVATE_REVALIDATE: &str = "private, max-age=0, must-revalidate";
const PRIVATE_NO_STORE: &str = "private, no-store";

impl UiDeliveryService {
    pub fn new(source: Arc<dyn UiSnapshotSource>, observer: Arc<dyn DeliveryObserver>) -> Self {
        Self {
            source,
            observer,
            cache: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn deliver_page(
        &self,
        caller: &UiCaller,
        request: &UiPageRequest,
    ) -> Result<Delivery<CallerSafePageResponse>, UiDeliveryError> {
        let started = Instant::now();
        let result = self.deliver_page_inner(caller, request);
        self.observe(
            DeliveryKind::Page,
            &result,
            started,
            result
                .as_ref()
                .ok()
                .and_then(|delivery| delivery.body.as_ref())
                .map(|body| body.page.definition_version),
        );
        result
    }

    pub fn deliver_resource(
        &self,
        caller: &UiCaller,
        request: &UiResourceRequest,
    ) -> Result<Delivery<CallerSafeResourceResponse>, UiDeliveryError> {
        let started = Instant::now();
        let result = self.deliver_resource_inner(caller, request);
        self.observe(
            DeliveryKind::Resource,
            &result,
            started,
            result
                .as_ref()
                .ok()
                .and_then(|delivery| delivery.body.as_ref())
                .map(|body| body.resource.definition_version),
        );
        result
    }

    /// Resolve an authorized caller's draft without reading or writing the
    /// published response cache. Conditional request headers are intentionally
    /// ignored because a draft is private mutable state.
    pub fn preview_page(
        &self,
        caller: &UiCaller,
        request: &UiPageRequest,
        draft_patches: &[PublishedPatch],
    ) -> Result<Delivery<CallerSafePageResponse>, UiDeliveryError> {
        self.inspect_draft_page(caller, request, draft_patches)
            .map_err(|error| UiDeliveryError {
                category: error.category,
            })
    }

    /// Administrator-facing draft inspection. Unlike ordinary delivery this
    /// retains exact resolver diagnostics after authorization has succeeded.
    pub fn inspect_draft_page(
        &self,
        caller: &UiCaller,
        request: &UiPageRequest,
        draft_patches: &[PublishedPatch],
    ) -> Result<Delivery<CallerSafePageResponse>, UiInspectionError> {
        require_active_membership(caller, &request.workspace_id)
            .map_err(inspection_from_delivery)?;
        if !caller.permissions.contains(MANAGE_PERMISSION) {
            return Err(inspection_unavailable());
        }
        let snapshot = self
            .source
            .snapshot(&request.workspace_id)
            .map_err(|error| inspection_from_delivery(map_snapshot_error(error)))?;
        let context = page_context(caller, request);
        let resolved = snapshot
            .catalogue
            .resolve(&request.page_id, &context, draft_patches, &[])
            .map_err(|error| {
                let diagnostic = error.to_string();
                UiInspectionError {
                    category: map_resolve_error(error).category,
                    diagnostic,
                }
            })?;
        let body = filter_page(resolved, &snapshot.resources, caller, &context)
            .map_err(inspection_from_delivery)?;
        let etag = make_etag(
            &body,
            &security_fingerprint(caller),
            snapshot.catalogue_generation,
            snapshot.publication_generation,
        )
        .map_err(inspection_from_delivery)?;
        Ok(Delivery {
            status: DeliveryStatus::Fresh,
            body: Some(body),
            etag,
            cache_control: PRIVATE_NO_STORE,
            cache_hit: false,
        })
    }

    pub fn invalidate_workspace(&self, workspace_id: &str) {
        self.cache
            .lock()
            .expect("UI delivery cache lock poisoned")
            .retain(|key, _| key.workspace_id != workspace_id);
    }

    pub fn readiness(&self) -> UiDeliveryReadiness {
        UiDeliveryReadiness {
            ready: self.source.is_ready(),
            cache_entries: self
                .cache
                .lock()
                .expect("UI delivery cache lock poisoned")
                .len(),
        }
    }

    fn deliver_page_inner(
        &self,
        caller: &UiCaller,
        request: &UiPageRequest,
    ) -> Result<Delivery<CallerSafePageResponse>, UiDeliveryError> {
        require_active_membership(caller, &request.workspace_id)?;
        let snapshot = self
            .source
            .snapshot(&request.workspace_id)
            .map_err(map_snapshot_error)?;
        let fingerprint = security_fingerprint(caller);
        let key = CacheKey {
            workspace_id: request.workspace_id.clone(),
            target: CacheTarget::Page(request.page_id.clone()),
            security_fingerprint: fingerprint.clone(),
            locale: request.locale.clone(),
            host: request.host.clone(),
            mode: request.mode.clone(),
            form_factor: request.form_factor.clone(),
            audience: request.audience.clone(),
            resource_subtype: request.resource_subtype.clone(),
            record_context: request.record_context.clone(),
            catalogue_generation: snapshot.catalogue_generation,
            publication_generation: snapshot.publication_generation,
        };
        if let Some(entry) = self.cache_entry(&key) {
            if let CachedPayload::Page(body) = entry.payload {
                return Ok(delivery_from_cache(
                    *body,
                    entry.etag,
                    request.if_none_match.as_deref(),
                ));
            }
        }

        let context = page_context(caller, request);
        let resolved = snapshot
            .catalogue
            .resolve(&request.page_id, &context, &snapshot.tenant_patches, &[])
            .map_err(map_resolve_error)?;
        let body = filter_page(resolved, &snapshot.resources, caller, &context)?;
        let etag = make_etag(
            &body,
            &fingerprint,
            snapshot.catalogue_generation,
            snapshot.publication_generation,
        )?;
        self.insert_cache(
            key,
            CacheEntry {
                etag: etag.clone(),
                payload: CachedPayload::Page(Box::new(body.clone())),
            },
        );
        Ok(delivery_fresh_or_not_modified(
            body,
            etag,
            request.if_none_match.as_deref(),
            false,
        ))
    }

    fn deliver_resource_inner(
        &self,
        caller: &UiCaller,
        request: &UiResourceRequest,
    ) -> Result<Delivery<CallerSafeResourceResponse>, UiDeliveryError> {
        require_active_membership(caller, &request.workspace_id)?;
        let snapshot = self
            .source
            .snapshot(&request.workspace_id)
            .map_err(map_snapshot_error)?;
        let fingerprint = security_fingerprint(caller);
        let key = CacheKey {
            workspace_id: request.workspace_id.clone(),
            target: CacheTarget::Resource(request.resource_id.clone()),
            security_fingerprint: fingerprint.clone(),
            locale: request.locale.clone(),
            host: request.host.clone(),
            mode: String::new(),
            form_factor: String::new(),
            audience: None,
            resource_subtype: None,
            record_context: BTreeMap::new(),
            catalogue_generation: snapshot.catalogue_generation,
            publication_generation: snapshot.publication_generation,
        };
        if let Some(entry) = self.cache_entry(&key) {
            if let CachedPayload::Resource(body) = entry.payload {
                return Ok(delivery_from_cache(
                    *body,
                    entry.etag,
                    request.if_none_match.as_deref(),
                ));
            }
        }
        let resource = snapshot
            .resources
            .iter()
            .find(|resource| resource.id == request.resource_id)
            .and_then(|resource| safe_resource(resource, caller, &request.host))
            .ok_or_else(page_unavailable)?;
        let body = CallerSafeResourceResponse { resource };
        let etag = make_etag(
            &body,
            &fingerprint,
            snapshot.catalogue_generation,
            snapshot.publication_generation,
        )?;
        self.insert_cache(
            key,
            CacheEntry {
                etag: etag.clone(),
                payload: CachedPayload::Resource(Box::new(body.clone())),
            },
        );
        Ok(delivery_fresh_or_not_modified(
            body,
            etag,
            request.if_none_match.as_deref(),
            false,
        ))
    }

    fn cache_entry(&self, key: &CacheKey) -> Option<CacheEntry> {
        self.cache
            .lock()
            .expect("UI delivery cache lock poisoned")
            .get(key)
            .cloned()
    }

    fn insert_cache(&self, key: CacheKey, entry: CacheEntry) {
        self.cache
            .lock()
            .expect("UI delivery cache lock poisoned")
            .insert(key, entry);
    }

    fn observe<T>(
        &self,
        kind: DeliveryKind,
        result: &Result<Delivery<T>, UiDeliveryError>,
        started: Instant,
        definition_version: Option<u32>,
    ) {
        self.observer.record(DeliveryObservation {
            kind,
            outcome: result
                .as_ref()
                .map(|delivery| delivery.status)
                .map_err(|error| error.category),
            cache_hit: result.as_ref().is_ok_and(|delivery| delivery.cache_hit),
            definition_version,
            elapsed_micros: started.elapsed().as_micros().try_into().unwrap_or(u64::MAX),
        });
    }
}

fn inspection_unavailable() -> UiInspectionError {
    inspection_from_delivery(page_unavailable())
}

fn inspection_from_delivery(error: UiDeliveryError) -> UiInspectionError {
    UiInspectionError {
        category: error.category,
        diagnostic: error.public_detail().into(),
    }
}

fn page_context(caller: &UiCaller, request: &UiPageRequest) -> ResolveContext {
    ResolveContext {
        host: request.host.clone(),
        mode: request.mode.clone(),
        form_factor: request.form_factor.clone(),
        audience: request.audience.clone(),
        roles: caller.roles.clone(),
        resource_subtype: request.resource_subtype.clone(),
        locale: request.locale.clone(),
        permissions: caller.permissions.clone(),
        values: request
            .record_context
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    }
}

fn filter_page(
    mut page: ResolvedPage,
    resources: &[ResourceDescriptor],
    caller: &UiCaller,
    context: &ResolveContext,
) -> Result<CallerSafePageResponse, UiDeliveryError> {
    let safe_resources: BTreeMap<_, _> = resources
        .iter()
        .filter_map(|resource| {
            safe_resource(resource, caller, &context.host).map(|safe| (safe.id.clone(), safe))
        })
        .collect();
    if page
        .resource
        .as_ref()
        .is_some_and(|resource| !safe_resources.contains_key(resource))
    {
        return Err(page_unavailable());
    }

    let allowed_operations: BTreeMap<String, BTreeSet<String>> = safe_resources
        .iter()
        .map(|(id, resource)| {
            (
                id.clone(),
                resource
                    .operations
                    .iter()
                    .map(|operation| operation.id.clone())
                    .collect(),
            )
        })
        .collect();
    page.data_sources.retain(|data_source| {
        allowed_operations
            .get(&data_source.resource)
            .is_some_and(|operations| operations.contains(&data_source.operation))
    });
    let allowed_data_sources: BTreeSet<_> = page
        .data_sources
        .iter()
        .map(|data_source| data_source.id.clone())
        .collect();
    page.actions
        .retain(|action| action_allowed(action, caller, &allowed_operations));
    for action in &mut page.actions {
        action.permission = None;
    }
    let allowed_actions: BTreeSet<_> = page
        .actions
        .iter()
        .map(|action| action.id.clone())
        .collect();
    let allowed_fields: BTreeSet<_> = page
        .resource
        .as_ref()
        .and_then(|resource| safe_resources.get(resource))
        .map(|resource| {
            resource
                .fields
                .iter()
                .map(|field| field.api_name.clone())
                .collect()
        })
        .unwrap_or_default();
    page.root = filter_node(
        page.root,
        caller,
        &allowed_fields,
        &allowed_data_sources,
        &allowed_actions,
    )
    .ok_or_else(page_unavailable)?;

    let referenced_resources: BTreeSet<_> = page
        .resource
        .iter()
        .cloned()
        .chain(
            page.data_sources
                .iter()
                .map(|data_source| data_source.resource.clone()),
        )
        .collect();
    let resources = referenced_resources
        .iter()
        .filter_map(|resource| safe_resources.get(resource).cloned())
        .collect();
    page.content_hash.clear();
    page.content_hash = content_hash(&page)?;
    Ok(CallerSafePageResponse { page, resources })
}

fn safe_resource(
    resource: &ResourceDescriptor,
    caller: &UiCaller,
    host: &HostProfile,
) -> Option<CallerSafeResourceDescriptor> {
    let operations: Vec<_> = resource
        .operations
        .iter()
        .filter(|operation| {
            caller.permissions.contains(&operation.permission)
                && (operation.hosts.is_empty() || operation.hosts.contains(host))
        })
        .map(|operation| CallerSafeOperationDescriptor {
            id: operation.id.clone(),
            kind: operation.kind.clone(),
            concurrency: operation.concurrency.clone(),
            hosts: operation.hosts.clone(),
        })
        .collect();
    if operations.is_empty() {
        return None;
    }
    let fields = resource
        .fields_with_standard()
        .iter()
        .filter(|field| permission_allowed(field.permission.as_deref(), caller))
        .map(safe_field)
        .collect();
    let relationships = resource
        .relationships
        .iter()
        .filter(|relationship| permission_allowed(relationship.permission.as_deref(), caller))
        .map(|relationship| CallerSafeRelationshipDescriptor {
            id: relationship.id.clone(),
            target_resource: relationship.target_resource.clone(),
            cardinality: relationship.cardinality.clone(),
        })
        .collect();
    Some(CallerSafeResourceDescriptor {
        id: resource.id.clone(),
        definition_version: resource.definition_version,
        owner_plugin: resource.owner_plugin.clone(),
        label: resource.label.clone(),
        name_field: resource.name_field.clone(),
        fields,
        relationships,
        operations,
    })
}

fn safe_field(field: &FieldDescriptor) -> CallerSafeFieldDescriptor {
    CallerSafeFieldDescriptor {
        api_name: field.api_name.clone(),
        label: field.label.clone(),
        description: field.description.clone(),
        field_type: field.field_type.clone(),
        source: field.source.clone(),
        readable: field.readable,
        writable: field.writable,
        required: field.required,
        classification: field.classification.clone(),
        options: field.options.clone(),
        validation_schema: field.validation_schema.clone(),
        default_value: field.default_value.clone(),
        submit_when_hidden: field.submit_when_hidden,
    }
}

fn action_allowed(
    action: &ActionDefinition,
    caller: &UiCaller,
    allowed_operations: &BTreeMap<String, BTreeSet<String>>,
) -> bool {
    if !permission_allowed(action.permission.as_deref(), caller) {
        return false;
    }
    if action.kind != ActionKind::ResourceOperation {
        return true;
    }
    action
        .target
        .rsplit_once('.')
        .is_some_and(|(resource, operation)| {
            allowed_operations
                .get(resource)
                .is_some_and(|operations| operations.contains(operation))
        })
}

fn filter_node(
    mut node: UiNode,
    caller: &UiCaller,
    allowed_fields: &BTreeSet<String>,
    allowed_data_sources: &BTreeSet<String>,
    allowed_actions: &BTreeSet<String>,
) -> Option<UiNode> {
    if !permission_allowed(node.permission.as_deref(), caller) {
        return None;
    }
    if node
        .props
        .get("field")
        .and_then(Value::as_str)
        .is_some_and(|field| !allowed_fields.contains(field))
        || ["action", "action_id"]
            .iter()
            .filter_map(|key| node.props.get(*key).and_then(Value::as_str))
            .any(|action| !allowed_actions.contains(action))
        || node
            .bindings
            .values()
            .any(|binding| !binding_allowed(binding, allowed_fields, allowed_data_sources))
    {
        return None;
    }

    let conditions = std::mem::take(&mut node.conditions);
    for (state, condition) in conditions {
        let sanitized =
            sanitize_condition(condition, caller, allowed_fields, allowed_data_sources)?;
        match (state, sanitized) {
            (ConditionalState::Visible, SanitizedCondition::Known(false)) => return None,
            (ConditionalState::Visible, SanitizedCondition::Known(true)) => {}
            (ConditionalState::Enabled, SanitizedCondition::Known(false)) => {
                node.props.insert("disabled".into(), Value::Bool(true));
            }
            (ConditionalState::ReadOnly, SanitizedCondition::Known(true)) => {
                node.props.insert("read_only".into(), Value::Bool(true));
            }
            (ConditionalState::Required, SanitizedCondition::Known(true)) => {
                node.props.insert("required".into(), Value::Bool(true));
            }
            (_, SanitizedCondition::Known(_)) => {}
            (state, SanitizedCondition::Expression(condition)) => {
                node.conditions.insert(state, condition);
            }
        }
    }
    node.permission = None;
    node.children = node
        .children
        .into_iter()
        .filter_map(|child| {
            filter_node(
                child,
                caller,
                allowed_fields,
                allowed_data_sources,
                allowed_actions,
            )
        })
        .collect();
    Some(node)
}

enum SanitizedCondition {
    Known(bool),
    Expression(ConditionExpression),
}

fn sanitize_condition(
    condition: ConditionExpression,
    caller: &UiCaller,
    allowed_fields: &BTreeSet<String>,
    allowed_data_sources: &BTreeSet<String>,
) -> Option<SanitizedCondition> {
    match condition {
        ConditionExpression::HasPermission { permission } => Some(SanitizedCondition::Known(
            caller.permissions.contains(&permission),
        )),
        ConditionExpression::And { args } => {
            sanitize_many(args, true, caller, allowed_fields, allowed_data_sources)
        }
        ConditionExpression::Or { args } => {
            sanitize_many(args, false, caller, allowed_fields, allowed_data_sources)
        }
        ConditionExpression::Not { arg } => {
            match sanitize_condition(*arg, caller, allowed_fields, allowed_data_sources)? {
                SanitizedCondition::Known(value) => Some(SanitizedCondition::Known(!value)),
                SanitizedCondition::Expression(expression) => {
                    Some(SanitizedCondition::Expression(ConditionExpression::Not {
                        arg: Box::new(expression),
                    }))
                }
            }
        }
        condition => {
            let allowed = condition_bindings(&condition)
                .all(|binding| binding_allowed(binding, allowed_fields, allowed_data_sources));
            allowed.then_some(SanitizedCondition::Expression(condition))
        }
    }
}

fn sanitize_many(
    args: Vec<ConditionExpression>,
    is_and: bool,
    caller: &UiCaller,
    allowed_fields: &BTreeSet<String>,
    allowed_data_sources: &BTreeSet<String>,
) -> Option<SanitizedCondition> {
    let mut expressions = Vec::new();
    for argument in args {
        match sanitize_condition(argument, caller, allowed_fields, allowed_data_sources)? {
            SanitizedCondition::Known(value) if is_and && !value => {
                return Some(SanitizedCondition::Known(false));
            }
            SanitizedCondition::Known(value) if !is_and && value => {
                return Some(SanitizedCondition::Known(true));
            }
            SanitizedCondition::Known(_) => {}
            SanitizedCondition::Expression(expression) => expressions.push(expression),
        }
    }
    match expressions.len() {
        0 => Some(SanitizedCondition::Known(is_and)),
        1 => Some(SanitizedCondition::Expression(expressions.remove(0))),
        _ if is_and => Some(SanitizedCondition::Expression(ConditionExpression::And {
            args: expressions,
        })),
        _ => Some(SanitizedCondition::Expression(ConditionExpression::Or {
            args: expressions,
        })),
    }
}

fn condition_bindings(condition: &ConditionExpression) -> Box<dyn Iterator<Item = &Binding> + '_> {
    match condition {
        ConditionExpression::Equal { left, right }
        | ConditionExpression::NotEqual { left, right } => Box::new([left, right].into_iter()),
        ConditionExpression::Exists { value } | ConditionExpression::In { value, .. } => {
            let mut bindings = vec![value];
            if let ConditionExpression::In { set, .. } = condition {
                bindings.extend(set.iter());
            }
            Box::new(bindings.into_iter())
        }
        _ => Box::new(std::iter::empty()),
    }
}

fn binding_allowed(
    binding: &Binding,
    allowed_fields: &BTreeSet<String>,
    allowed_data_sources: &BTreeSet<String>,
) -> bool {
    match binding {
        Binding::Field { field } => allowed_fields.contains(field),
        Binding::DataSource { data_source, .. } => allowed_data_sources.contains(data_source),
        Binding::Context { .. } | Binding::Literal { .. } => true,
    }
}

fn permission_allowed(permission: Option<&str>, caller: &UiCaller) -> bool {
    permission.is_none_or(|permission| caller.permissions.contains(permission))
}

fn require_active_membership(caller: &UiCaller, workspace_id: &str) -> Result<(), UiDeliveryError> {
    (caller.active_membership && caller.workspace_id == workspace_id)
        .then_some(())
        .ok_or_else(page_unavailable)
}

fn map_snapshot_error(error: SnapshotError) -> UiDeliveryError {
    UiDeliveryError {
        category: match error {
            SnapshotError::Hidden => UiFailureCategory::PageUnavailable,
            SnapshotError::Incompatible => UiFailureCategory::Incompatible,
            SnapshotError::Conflict => UiFailureCategory::Conflict,
            SnapshotError::TemporarilyUnavailable => UiFailureCategory::TemporarilyUnavailable,
        },
    }
}

fn map_resolve_error(error: ResolveError) -> UiDeliveryError {
    UiDeliveryError {
        category: match error {
            ResolveError::PageUnavailable(_) | ResolveError::UnsupportedHost { .. } => {
                UiFailureCategory::PageUnavailable
            }
            ResolveError::IncompatibleVersion { .. } | ResolveError::InvalidVersionRange(_) => {
                UiFailureCategory::Incompatible
            }
            ResolveError::HashSerialization(_) => UiFailureCategory::TemporarilyUnavailable,
            _ => UiFailureCategory::Conflict,
        },
    }
}

fn page_unavailable() -> UiDeliveryError {
    UiDeliveryError {
        category: UiFailureCategory::PageUnavailable,
    }
}

fn security_fingerprint(caller: &UiCaller) -> String {
    let input = serde_json::json!({
        "user": caller.user_id,
        "workspace": caller.workspace_id,
        "membership_version": caller.membership_version,
        "roles": caller.roles,
        "permissions": caller.permissions,
    });
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&input).expect("security fingerprint serializes"))
    )
}

fn content_hash<T: Serialize>(value: &T) -> Result<String, UiDeliveryError> {
    let bytes = serde_json::to_vec(value).map_err(|_| UiDeliveryError {
        category: UiFailureCategory::TemporarilyUnavailable,
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn make_etag<T: Serialize>(
    value: &T,
    fingerprint: &str,
    catalogue_generation: u64,
    publication_generation: u64,
) -> Result<String, UiDeliveryError> {
    let input = serde_json::json!({
        "payload": value,
        "security": fingerprint,
        "catalogue_generation": catalogue_generation,
        "publication_generation": publication_generation,
    });
    Ok(format!("\"ui-{}\"", content_hash(&input)?))
}

fn delivery_from_cache<T: Clone>(
    body: T,
    etag: String,
    if_none_match: Option<&str>,
) -> Delivery<T> {
    delivery_fresh_or_not_modified(body, etag, if_none_match, true)
}

fn delivery_fresh_or_not_modified<T>(
    body: T,
    etag: String,
    if_none_match: Option<&str>,
    cache_hit: bool,
) -> Delivery<T> {
    let not_modified = if_none_match == Some(etag.as_str());
    Delivery {
        status: if not_modified {
            DeliveryStatus::NotModified
        } else {
            DeliveryStatus::Fresh
        },
        body: (!not_modified).then_some(body),
        etag,
        cache_control: PRIVATE_REVALIDATE,
        cache_hit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contract::ui::{
        ActionDefinition, ActionKind, ConditionExpression, DataSourceDefinition, FieldSource,
        LoadPolicy, PageDefinition, PageKind, PageVariant, ResourceOperation, UiBundle,
        UI_CONTRACT_VERSION,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use ui_metadata::{DefinitionVersionRange, MetadataCatalogue, PluginUiPackage, PluginUiState};

    #[derive(Default)]
    struct MutableSource {
        snapshot: Mutex<Option<UiWorkspaceSnapshot>>,
        error: Mutex<Option<SnapshotError>>,
        ready: AtomicBool,
    }

    impl MutableSource {
        fn new(snapshot: UiWorkspaceSnapshot) -> Self {
            Self {
                snapshot: Mutex::new(Some(snapshot)),
                error: Mutex::new(None),
                ready: AtomicBool::new(true),
            }
        }

        fn replace(&self, snapshot: UiWorkspaceSnapshot) {
            *self.snapshot.lock().unwrap() = Some(snapshot);
        }

        fn fail_with(&self, error: SnapshotError) {
            *self.error.lock().unwrap() = Some(error);
        }
    }

    impl UiSnapshotSource for MutableSource {
        fn snapshot(&self, _workspace_id: &str) -> Result<UiWorkspaceSnapshot, SnapshotError> {
            if let Some(error) = self.error.lock().unwrap().clone() {
                return Err(error);
            }
            self.snapshot
                .lock()
                .unwrap()
                .clone()
                .ok_or(SnapshotError::TemporarilyUnavailable)
        }

        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::Relaxed)
        }
    }

    #[derive(Default)]
    struct RecordingObserver(Mutex<Vec<DeliveryObservation>>);

    impl DeliveryObserver for RecordingObserver {
        fn record(&self, observation: DeliveryObservation) {
            self.0.lock().unwrap().push(observation);
        }
    }

    fn field(api_name: &str, permission: Option<&str>) -> FieldDescriptor {
        FieldDescriptor {
            api_name: api_name.into(),
            label: api_name.replace('_', " "),
            description: None,
            field_type: FieldType::Text,
            source: if api_name.ends_with("__c") {
                FieldSource::TenantCustom
            } else {
                FieldSource::Standard
            },
            readable: true,
            writable: true,
            required: api_name == "first_name" || api_name == "name",
            permission: permission.map(Into::into),
            classification: Some(DataClassification::Personal),
            options: Vec::new(),
            validation_schema: None,
            default_value: None,
            submit_when_hidden: false,
        }
    }

    fn operation(id: &str, permission: &str) -> ResourceOperation {
        ResourceOperation {
            id: id.into(),
            kind: if id == "read" {
                OperationKind::Read
            } else {
                OperationKind::Update
            },
            permission: permission.into(),
            concurrency: None,
            hosts: vec![HostProfile::InternalWeb, HostProfile::Desktop],
        }
    }

    fn resources() -> Vec<ResourceDescriptor> {
        vec![
            ResourceDescriptor {
                contract_version: UI_CONTRACT_VERSION,
                id: "platform.contacts.person".into(),
                definition_version: 1,
                owner_plugin: "platform.contacts".into(),
                label: "Person".into(),
                name_field: Some("first_name".into()),
                fields: vec![
                    field("first_name", None),
                    field("service_tier__c", Some("contacts.person.read-custom")),
                ],
                relationships: vec![plugin_contract::ui::RelationshipDescriptor {
                    id: "organisation".into(),
                    target_resource: "platform.contacts.organisation".into(),
                    cardinality: RelationshipCardinality::One,
                    permission: Some("contacts.organisation.read".into()),
                }],
                operations: vec![
                    operation("read", "contacts.person.read"),
                    operation("update", "contacts.person.update"),
                ],
            },
            ResourceDescriptor {
                contract_version: UI_CONTRACT_VERSION,
                id: "platform.contacts.organisation".into(),
                definition_version: 1,
                owner_plugin: "platform.contacts".into(),
                label: "Organisation".into(),
                name_field: Some("name".into()),
                fields: vec![field("name", None)],
                relationships: Vec::new(),
                operations: vec![operation("read", "contacts.organisation.read")],
            },
        ]
    }

    fn node(id: &str, component: &str, marker: &str) -> UiNode {
        UiNode {
            id: id.into(),
            component: component.into(),
            props: BTreeMap::from([("marker".into(), Value::String(marker.into()))]),
            bindings: BTreeMap::new(),
            conditions: BTreeMap::new(),
            children: Vec::new(),
            permission: None,
            hosts: vec![HostProfile::InternalWeb, HostProfile::Desktop],
        }
    }

    fn root(marker: &str) -> UiNode {
        let mut root = node("page-root", "platform.ui.page", marker);
        let mut first_name = node("first-name", "platform.ui.field", marker);
        first_name
            .props
            .insert("field".into(), Value::String("first_name".into()));
        let mut custom = node("custom-field", "platform.ui.field", marker);
        custom
            .props
            .insert("field".into(), Value::String("service_tier__c".into()));
        custom.permission = Some("contacts.person.read-custom".into());
        let mut permission_condition = node("permission-panel", "platform.ui.section", marker);
        permission_condition.conditions.insert(
            ConditionalState::Visible,
            ConditionExpression::HasPermission {
                permission: "contacts.person.read-custom".into(),
            },
        );
        let mut action = node("save-action", "platform.ui.action", marker);
        action
            .props
            .insert("action_id".into(), Value::String("save-person".into()));
        root.children = vec![first_name, custom, permission_condition, action];
        root
    }

    fn equals_context(path: &str, value: Value) -> ConditionExpression {
        ConditionExpression::Equal {
            left: Binding::Context { path: path.into() },
            right: Binding::Literal { value },
        }
    }

    fn page() -> PageDefinition {
        PageDefinition {
            contract_version: UI_CONTRACT_VERSION,
            id: "platform.contacts.person-page".into(),
            definition_version: 4,
            owner_plugin: "platform.contacts".into(),
            kind: PageKind::Record,
            hosts: vec![HostProfile::InternalWeb, HostProfile::Desktop],
            resource: Some("platform.contacts.person".into()),
            data_sources: vec![DataSourceDefinition {
                id: "person-record".into(),
                resource: "platform.contacts.person".into(),
                operation: "read".into(),
                parameters: BTreeMap::new(),
                load: LoadPolicy::Eager,
            }],
            actions: vec![ActionDefinition {
                id: "save-person".into(),
                kind: ActionKind::ResourceOperation,
                target: "platform.contacts.person.update".into(),
                parameters: BTreeMap::new(),
                permission: Some("contacts.person.update".into()),
            }],
            variants: vec![
                PageVariant {
                    id: "fr-locale".into(),
                    priority: 30,
                    condition: Some(equals_context("locale", Value::String("fr-FR".into()))),
                    root: root("fr"),
                },
                PageVariant {
                    id: "admin-role".into(),
                    priority: 20,
                    condition: Some(equals_context("role.admin", Value::Bool(true))),
                    root: root("admin"),
                },
                PageVariant {
                    id: "compact-form".into(),
                    priority: 10,
                    condition: Some(equals_context(
                        "form-factor",
                        Value::String("compact".into()),
                    )),
                    root: root("compact"),
                },
            ],
            root: root("default"),
        }
    }

    fn catalogue(enabled: bool) -> MetadataCatalogue {
        let resources = resources();
        let mut catalogue = MetadataCatalogue::default();
        for component in [
            "platform.ui.page",
            "platform.ui.field",
            "platform.ui.section",
            "platform.ui.action",
        ] {
            catalogue.register_host_component(component);
        }
        catalogue
            .install(PluginUiPackage {
                plugin_id: "platform.contacts".into(),
                plugin_version: "1.4.0".into(),
                dependencies: BTreeSet::new(),
                state: if enabled {
                    PluginUiState::Enabled
                } else {
                    PluginUiState::Disabled
                },
                bundle: UiBundle {
                    resources,
                    pages: vec![page()],
                    ..UiBundle::default()
                },
            })
            .unwrap();
        catalogue
    }

    fn snapshot(
        enabled: bool,
        catalogue_generation: u64,
        publication_generation: u64,
    ) -> UiWorkspaceSnapshot {
        UiWorkspaceSnapshot {
            catalogue: Arc::new(catalogue(enabled)),
            resources: resources(),
            tenant_patches: Vec::new(),
            catalogue_generation,
            publication_generation,
        }
    }

    fn caller(permissions: &[&str]) -> UiCaller {
        UiCaller {
            user_id: "user-a".into(),
            workspace_id: "workspace-a".into(),
            active_membership: true,
            membership_version: 1,
            roles: BTreeSet::new(),
            permissions: permissions
                .iter()
                .map(|permission| (*permission).into())
                .collect(),
        }
    }

    fn request() -> UiPageRequest {
        UiPageRequest {
            workspace_id: "workspace-a".into(),
            page_id: "platform.contacts.person-page".into(),
            locale: "en-GB".into(),
            host: HostProfile::InternalWeb,
            mode: "view".into(),
            form_factor: "wide".into(),
            audience: Some("internal".into()),
            resource_subtype: None,
            record_context: BTreeMap::from([("route.person_id".into(), "record-1".into())]),
            if_none_match: None,
        }
    }

    fn service() -> (
        Arc<MutableSource>,
        Arc<RecordingObserver>,
        UiDeliveryService,
    ) {
        let source = Arc::new(MutableSource::new(snapshot(true, 1, 1)));
        let observer = Arc::new(RecordingObserver::default());
        let service = UiDeliveryService::new(source.clone(), observer.clone());
        (source, observer, service)
    }

    #[test]
    fn forbidden_fields_actions_relationships_and_permissions_never_serialize() {
        let (_, _, service) = service();
        let response = service
            .deliver_page(&caller(&["contacts.person.read"]), &request())
            .unwrap()
            .body
            .unwrap();
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("first_name"));
        for forbidden in [
            "service_tier__c",
            "save-person",
            "contacts.person.update",
            "contacts.person.read-custom",
            "contacts.organisation.read",
            "organisation",
            "permission-panel",
        ] {
            assert!(!json.contains(forbidden), "leaked {forbidden}: {json}");
        }
    }

    #[test]
    fn permission_contexts_never_share_cache_entries_or_etags() {
        let (_, _, service) = service();
        let basic = service
            .deliver_page(&caller(&["contacts.person.read"]), &request())
            .unwrap();
        let mut privileged = caller(&[
            "contacts.person.read",
            "contacts.person.update",
            "contacts.person.read-custom",
        ]);
        privileged.user_id = "user-b".into();
        let rich = service.deliver_page(&privileged, &request()).unwrap();
        assert_ne!(basic.etag, rich.etag);
        assert!(!basic.cache_hit);
        assert!(!rich.cache_hit);
        assert!(serde_json::to_string(&rich.body.unwrap())
            .unwrap()
            .contains("service_tier__c"));
    }

    #[test]
    fn conditional_requests_only_match_the_same_authorized_snapshot() {
        let (_, _, service) = service();
        let basic_caller = caller(&["contacts.person.read"]);
        let first = service.deliver_page(&basic_caller, &request()).unwrap();
        let mut conditional = request();
        conditional.if_none_match = Some(first.etag.clone());
        let second = service.deliver_page(&basic_caller, &conditional).unwrap();
        assert_eq!(second.status, DeliveryStatus::NotModified);
        assert!(second.body.is_none());
        assert!(second.cache_hit);

        let mut other = caller(&["contacts.person.read"]);
        other.user_id = "user-b".into();
        let cross_user = service.deliver_page(&other, &conditional).unwrap();
        assert_eq!(cross_user.status, DeliveryStatus::Fresh);
        assert_ne!(cross_user.etag, first.etag);
    }

    #[test]
    fn draft_preview_is_no_store_and_cannot_poison_published_cache() {
        let (_, _, service) = service();
        let mut admin = caller(&["contacts.person.read", MANAGE_PERMISSION]);
        admin.user_id = "metadata-admin".into();
        let draft = PublishedPatch {
            id: "tenant.preview.marker".into(),
            publication_version: 0,
            target_page: "platform.contacts.person-page".into(),
            compatible_versions: DefinitionVersionRange::exact(4),
            target_node: "page-root".into(),
            operation: plugin_contract::ui::PatchOperation::Configure,
            priority: 0,
            node: None,
            props: BTreeMap::from([("marker".into(), Value::String("draft-preview".into()))]),
        };

        let preview = service.preview_page(&admin, &request(), &[draft]).unwrap();
        assert_eq!(preview.cache_control, PRIVATE_NO_STORE);
        assert!(!preview.cache_hit);
        assert_eq!(preview.status, DeliveryStatus::Fresh);
        assert_eq!(
            preview.body.unwrap().page.root.props["marker"],
            Value::String("draft-preview".into())
        );
        assert_eq!(service.readiness().cache_entries, 0);

        let published = service.deliver_page(&admin, &request()).unwrap();
        assert_eq!(
            published.body.unwrap().page.root.props["marker"],
            Value::String("default".into())
        );
        assert_eq!(service.readiness().cache_entries, 1);

        let ordinary = caller(&["contacts.person.read"]);
        assert_eq!(
            service
                .preview_page(&ordinary, &request(), &[])
                .unwrap_err(),
            page_unavailable()
        );
        assert_eq!(service.readiness().cache_entries, 1);

        let invalid = PublishedPatch {
            id: "tenant.preview.invalid".into(),
            publication_version: 0,
            target_page: "platform.contacts.person-page".into(),
            compatible_versions: DefinitionVersionRange::exact(4),
            target_node: "missing-anchor".into(),
            operation: plugin_contract::ui::PatchOperation::Configure,
            priority: 0,
            node: None,
            props: BTreeMap::from([("marker".into(), Value::String("invalid".into()))]),
        };
        let diagnostic = service
            .inspect_draft_page(&admin, &request(), &[invalid])
            .unwrap_err();
        assert!(diagnostic
            .diagnostic
            .contains("missing anchor missing-anchor"));

        let mut wrong_workspace = admin.clone();
        wrong_workspace.workspace_id = "workspace-b".into();
        assert_eq!(
            service
                .preview_page(&wrong_workspace, &request(), &[])
                .unwrap_err()
                .category,
            UiFailureCategory::PageUnavailable
        );
    }

    #[test]
    fn generations_and_explicit_invalidation_retire_cached_snapshots() {
        let (source, _, service) = service();
        let first = service
            .deliver_page(&caller(&["contacts.person.read"]), &request())
            .unwrap();
        assert_eq!(service.readiness().cache_entries, 1);
        source.replace(snapshot(true, 2, 3));
        let next = service
            .deliver_page(&caller(&["contacts.person.read"]), &request())
            .unwrap();
        assert_ne!(first.etag, next.etag);
        assert!(!next.cache_hit);
        service.invalidate_workspace("workspace-a");
        assert_eq!(service.readiness().cache_entries, 0);
    }

    #[test]
    fn disabled_missing_and_denied_pages_share_a_non_enumerating_error() {
        let (source, _, service) = service();
        let mut denied = caller(&["contacts.person.read"]);
        denied.active_membership = false;
        let denied_error = service.deliver_page(&denied, &request()).unwrap_err();

        source.replace(snapshot(false, 2, 1));
        let disabled_error = service
            .deliver_page(&caller(&["contacts.person.read"]), &request())
            .unwrap_err();

        source.replace(snapshot(true, 3, 1));
        let mut missing_request = request();
        missing_request.page_id = "platform.unknown.page".into();
        let missing_error = service
            .deliver_page(&caller(&["contacts.person.read"]), &missing_request)
            .unwrap_err();
        assert_eq!(denied_error, disabled_error);
        assert_eq!(disabled_error, missing_error);
        assert_eq!(missing_error.code(), "ui.page_unavailable");
        assert!(!missing_error.public_detail().contains("platform"));
    }

    #[test]
    fn locale_form_factor_role_host_and_lifecycle_change_resolution_context() {
        let (_, _, service) = service();
        let basic = caller(&["contacts.person.read"]);
        let default = service
            .deliver_page(&basic, &request())
            .unwrap()
            .body
            .unwrap();
        assert_eq!(
            default.page.root.props["marker"],
            Value::String("default".into())
        );

        let mut french_request = request();
        french_request.locale = "fr-FR".into();
        let french = service
            .deliver_page(&basic, &french_request)
            .unwrap()
            .body
            .unwrap();
        assert_eq!(french.page.root.props["marker"], Value::String("fr".into()));

        let mut compact_request = request();
        compact_request.form_factor = "compact".into();
        let compact = service
            .deliver_page(&basic, &compact_request)
            .unwrap()
            .body
            .unwrap();
        assert_eq!(
            compact.page.root.props["marker"],
            Value::String("compact".into())
        );

        let mut admin = basic.clone();
        admin.roles.insert("admin".into());
        let admin_page = service
            .deliver_page(&admin, &request())
            .unwrap()
            .body
            .unwrap();
        assert_eq!(
            admin_page.page.root.props["marker"],
            Value::String("admin".into())
        );

        let mut public_request = request();
        public_request.host = HostProfile::PublicForm;
        assert_eq!(
            service
                .deliver_page(&basic, &public_request)
                .unwrap_err()
                .category,
            UiFailureCategory::PageUnavailable
        );
    }

    #[test]
    fn resource_endpoint_filters_and_caches_the_same_security_contract() {
        let (_, _, service) = service();
        let resource_request = UiResourceRequest {
            workspace_id: "workspace-a".into(),
            resource_id: "platform.contacts.person".into(),
            host: HostProfile::InternalWeb,
            locale: "en-GB".into(),
            if_none_match: None,
        };
        let first = service
            .deliver_resource(&caller(&["contacts.person.read"]), &resource_request)
            .unwrap();
        let json = serde_json::to_string(&first.body).unwrap();
        assert!(json.contains("first_name"));
        assert!(!json.contains("service_tier__c"));
        // Every record resource carries the kernel's standard fields (ADR 0014).
        let body = serde_json::to_value(&first.body).unwrap();
        let fields: Vec<_> = body["resource"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| (field["api_name"].clone(), field["source"].clone()))
            .collect();
        for name in plugin_contract::ui::STANDARD_FIELD_NAMES {
            assert!(
                fields.contains(&(serde_json::json!(name), serde_json::json!("system"))),
                "{name} missing from {fields:?}"
            );
        }
        assert_eq!(body["resource"]["name_field"], "first_name");
        let second = service
            .deliver_resource(&caller(&["contacts.person.read"]), &resource_request)
            .unwrap();
        assert!(second.cache_hit);
    }

    #[test]
    fn readiness_and_observations_expose_no_tenant_or_permission_metadata() {
        let (source, observer, service) = service();
        service
            .deliver_page(&caller(&["contacts.person.read"]), &request())
            .unwrap();
        let observation = observer.0.lock().unwrap()[0].clone();
        assert_eq!(observation.kind, DeliveryKind::Page);
        assert_eq!(observation.definition_version, Some(4));
        assert!(service.readiness().ready);
        source.ready.store(false, Ordering::Relaxed);
        let readiness = serde_json::to_string(&service.readiness()).unwrap();
        assert_eq!(readiness, r#"{"ready":false,"cache_entries":1}"#);
        assert!(!readiness.contains("workspace"));
        assert!(!readiness.contains("contacts"));
    }

    #[test]
    fn snapshot_failures_have_stable_redacted_categories() {
        let (source, _, service) = service();
        for (source_error, expected) in [
            (SnapshotError::Hidden, UiFailureCategory::PageUnavailable),
            (SnapshotError::Incompatible, UiFailureCategory::Incompatible),
            (SnapshotError::Conflict, UiFailureCategory::Conflict),
            (
                SnapshotError::TemporarilyUnavailable,
                UiFailureCategory::TemporarilyUnavailable,
            ),
        ] {
            source.fail_with(source_error);
            assert_eq!(
                service
                    .deliver_page(&caller(&["contacts.person.read"]), &request())
                    .unwrap_err()
                    .category,
                expected
            );
        }
    }
}
