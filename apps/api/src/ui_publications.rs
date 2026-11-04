use crate::{problem, AppState};
use async_trait::async_trait;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use plugin_contract::ui::HostProfile;
use serde::{Deserialize, Serialize};
use std::{borrow::Cow, collections::BTreeMap};
use ui_metadata::{
    publication::{
        ImportPlan, PublicationActor, PublicationBundle, PublicationComparison, PublicationError,
        TenantPagePublications, MANAGE_PERMISSION,
    },
    PublishedPatch,
};
use ui_metadata_delivery::{CallerSafePageResponse, UiCaller, UiPageRequest};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum PublicationStoreError {
    #[error("publication store unavailable")]
    #[allow(dead_code, reason = "used by isolated API state fixtures")]
    Unavailable,
    #[error("publication state changed; expected revision {expected}, actual {actual:?}")]
    StaleRevision { expected: u64, actual: Option<u64> },
    #[error("publication store failed")]
    Internal,
}

#[async_trait]
pub trait PublicationStore: Send + Sync {
    async fn load(
        &self,
        tenant_id: Uuid,
        page_id: &str,
    ) -> Result<Option<TenantPagePublications>, PublicationStoreError>;

    async fn save(
        &self,
        state: &TenantPagePublications,
        expected_revision: u64,
    ) -> Result<(), PublicationStoreError>;
}

#[allow(dead_code, reason = "used by isolated API state fixtures")]
pub struct UnavailablePublicationStore;

#[async_trait]
impl PublicationStore for UnavailablePublicationStore {
    async fn load(
        &self,
        _tenant_id: Uuid,
        _page_id: &str,
    ) -> Result<Option<TenantPagePublications>, PublicationStoreError> {
        Err(PublicationStoreError::Unavailable)
    }

    async fn save(
        &self,
        _state: &TenantPagePublications,
        _expected_revision: u64,
    ) -> Result<(), PublicationStoreError> {
        Err(PublicationStoreError::Unavailable)
    }
}

pub struct PostgresPublicationStore(sqlx::PgPool);

impl PostgresPublicationStore {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self(pool)
    }
}

#[async_trait]
impl PublicationStore for PostgresPublicationStore {
    async fn load(
        &self,
        tenant_id: Uuid,
        page_id: &str,
    ) -> Result<Option<TenantPagePublications>, PublicationStoreError> {
        platform_db::load_ui_metadata_page_state(&self.0, tenant_id, page_id)
            .await
            .map_err(map_database_error)
    }

    async fn save(
        &self,
        state: &TenantPagePublications,
        expected_revision: u64,
    ) -> Result<(), PublicationStoreError> {
        platform_db::save_ui_metadata_page_state(&self.0, state, expected_revision)
            .await
            .map_err(map_database_error)
    }
}

fn map_database_error(error: platform_db::UiMetadataStateError) -> PublicationStoreError {
    match error {
        platform_db::UiMetadataStateError::StaleRevision { expected, actual } => {
            PublicationStoreError::StaleRevision { expected, actual }
        }
        platform_db::UiMetadataStateError::Database(_)
        | platform_db::UiMetadataStateError::Serialization(_) => PublicationStoreError::Internal,
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ResolveQuery {
    #[serde(default = "default_view")]
    view: String,
    #[serde(default = "default_locale")]
    locale: String,
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_mode")]
    mode: String,
    #[serde(default = "default_form_factor")]
    form_factor: String,
    audience: Option<String>,
    resource_subtype: Option<String>,
    #[serde(default)]
    record: BTreeMap<String, String>,
}

impl Default for ResolveQuery {
    fn default() -> Self {
        Self {
            view: default_view(),
            locale: default_locale(),
            host: default_host(),
            mode: default_mode(),
            form_factor: default_form_factor(),
            audience: None,
            resource_subtype: None,
            record: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct CompareQuery {
    from: u32,
    to: u32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct MutationRequest {
    expected_revision: u64,
    #[serde(flatten)]
    operation: MutationOperation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
enum MutationOperation {
    BeginDraft {
        #[serde(default)]
        base_source_versions: BTreeMap<String, String>,
    },
    UpdateDraft {
        expected_draft_version: u64,
        patches: Vec<PublishedPatch>,
    },
    Validate {
        #[serde(default)]
        context: ResolveQuery,
    },
    Publish {
        expected_draft_version: u64,
        #[serde(default)]
        context: ResolveQuery,
    },
    Rollback {
        target_version: u32,
        #[serde(default)]
        context: ResolveQuery,
    },
    Abandon {
        expected_draft_version: u64,
    },
    Import {
        bundle: PublicationBundle,
        #[serde(default)]
        dry_run: bool,
        #[serde(default)]
        context: ResolveQuery,
    },
    Export,
}

#[derive(Debug, Serialize)]
struct ManagementResponse {
    state: TenantPagePublications,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview: Option<CallerSafePageResponse>,
    #[serde(skip_serializing_if = "Option::is_none")]
    import_plan: Option<ImportPlan>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle: Option<PublicationBundle>,
}

pub(crate) async fn get(
    State(state): State<AppState>,
    caller: Option<Extension<UiCaller>>,
    Path((tenant_id, page_id)): Path<(Uuid, String)>,
) -> Response {
    let Some((_caller, _)) = authorized(caller, tenant_id) else {
        return unavailable_response();
    };
    match load_or_new(&state, tenant_id, &page_id).await {
        Ok(aggregate) => management_response(aggregate, None, None, None),
        Err(error) => store_error_response(error),
    }
}

pub(crate) async fn mutate(
    State(state): State<AppState>,
    principal: Option<Extension<crate::auth::WorkspacePrincipal>>,
    caller: Option<Extension<UiCaller>>,
    Path((tenant_id, page_id)): Path<(Uuid, String)>,
    Json(request): Json<MutationRequest>,
) -> Response {
    let Some((caller, actor)) = authorized(caller, tenant_id) else {
        return unavailable_response();
    };
    if let Err(response) = prime(&state, principal.as_deref()).await {
        return response;
    }
    let mut aggregate = match load_or_new(&state, tenant_id, &page_id).await {
        Ok(aggregate) => aggregate,
        Err(error) => return store_error_response(error),
    };
    if aggregate.revision != request.expected_revision {
        return publication_error_response(PublicationError::StaleRevision {
            expected: request.expected_revision,
            actual: aggregate.revision,
        });
    }
    let now = chrono::Utc::now();
    let previous_publication_generation = aggregate.publication_generation;
    let mut preview = None;
    let mut import_plan = None;
    let mut bundle = None;
    let should_save = match request.operation {
        MutationOperation::BeginDraft {
            base_source_versions,
        } => aggregate
            .begin_draft(&actor, request.expected_revision, base_source_versions, now)
            .map(|_| true),
        MutationOperation::UpdateDraft {
            expected_draft_version,
            patches,
        } => aggregate
            .update_draft(
                &actor,
                request.expected_revision,
                expected_draft_version,
                patches,
                now,
            )
            .map(|_| true),
        MutationOperation::Validate { context } => {
            let patches = aggregate
                .draft
                .as_ref()
                .ok_or(PublicationError::DraftMissing)
                .map(|draft| draft.patches.clone());
            patches.and_then(|patches| {
                preview = Some(resolve_preview(
                    &state, &caller, tenant_id, &page_id, &context, &patches,
                )?);
                aggregate.validate_draft_with(
                    &actor,
                    request.expected_revision,
                    |_| Ok(()),
                    now,
                )?;
                Ok(true)
            })
        }
        MutationOperation::Publish {
            expected_draft_version,
            context,
        } => aggregate
            .publish_with_validation(
                &actor,
                request.expected_revision,
                expected_draft_version,
                |patches| {
                    resolve_preview(&state, &caller, tenant_id, &page_id, &context, patches)
                        .map(|_| ())
                },
                now,
            )
            .map(|_| true),
        MutationOperation::Rollback {
            target_version,
            context,
        } => aggregate
            .rollback_with_validation(
                &actor,
                request.expected_revision,
                target_version,
                |patches| {
                    resolve_preview(&state, &caller, tenant_id, &page_id, &context, patches)
                        .map(|_| ())
                },
                now,
            )
            .map(|_| true),
        MutationOperation::Abandon {
            expected_draft_version,
        } => aggregate
            .abandon_draft(
                &actor,
                request.expected_revision,
                expected_draft_version,
                now,
            )
            .map(|_| true),
        MutationOperation::Import {
            bundle: incoming,
            dry_run,
            context,
        } => {
            let plan = aggregate.plan_import_with(&actor, &incoming, |patches| {
                resolve_preview(&state, &caller, tenant_id, &page_id, &context, patches).map(|_| ())
            });
            plan.and_then(|plan| {
                import_plan = Some(plan.clone());
                if dry_run {
                    Ok(false)
                } else if !plan.can_apply() {
                    Err(PublicationError::InvalidDefinition(
                        plan.conflicts.join("; "),
                    ))
                } else {
                    aggregate.import_as_draft_with_validation(
                        &actor,
                        request.expected_revision,
                        incoming,
                        |_| Ok(()),
                        now,
                    )?;
                    Ok(true)
                }
            })
        }
        MutationOperation::Export => aggregate.export_published(&actor, now).map(|value| {
            bundle = Some(value);
            true
        }),
    };
    let should_save = match should_save {
        Ok(should_save) => should_save,
        Err(error) => return publication_error_response(error),
    };
    if should_save {
        if let Err(error) = state
            .ui_publications
            .save(&aggregate, request.expected_revision)
            .await
        {
            return store_error_response(error);
        }
        if aggregate.publication_generation != previous_publication_generation {
            state
                .ui_delivery
                .invalidate_workspace(&tenant_id.to_string());
        }
    }
    management_response(aggregate, preview, import_plan, bundle)
}

pub(crate) async fn preview(
    State(state): State<AppState>,
    principal: Option<Extension<crate::auth::WorkspacePrincipal>>,
    caller: Option<Extension<UiCaller>>,
    Path((tenant_id, page_id)): Path<(Uuid, String)>,
    Query(query): Query<ResolveQuery>,
) -> Response {
    let Some((caller, _)) = authorized(caller, tenant_id) else {
        return unavailable_response();
    };
    if let Err(response) = prime(&state, principal.as_deref()).await {
        return response;
    }
    if query.view == "published" {
        let Some(request) = page_request(tenant_id, &page_id, &query) else {
            return publication_error_response(PublicationError::InvalidDefinition(
                "unsupported host".into(),
            ));
        };
        return match state.ui_delivery.deliver_page(&caller, &request) {
            Ok(delivery) => Json(delivery.body).into_response(),
            Err(_) => unavailable_response(),
        };
    }
    let aggregate = match load_or_new(&state, tenant_id, &page_id).await {
        Ok(aggregate) => aggregate,
        Err(error) => return store_error_response(error),
    };
    let Some(draft) = aggregate.draft else {
        return publication_error_response(PublicationError::DraftMissing);
    };
    match resolve_preview(&state, &caller, tenant_id, &page_id, &query, &draft.patches) {
        Ok(body) => ([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response(),
        Err(error) => publication_error_response(error),
    }
}

pub(crate) async fn compare(
    State(state): State<AppState>,
    caller: Option<Extension<UiCaller>>,
    Path((tenant_id, page_id)): Path<(Uuid, String)>,
    Query(query): Query<CompareQuery>,
) -> Response {
    let Some((_, actor)) = authorized(caller, tenant_id) else {
        return unavailable_response();
    };
    let aggregate = match load_or_new(&state, tenant_id, &page_id).await {
        Ok(aggregate) => aggregate,
        Err(error) => return store_error_response(error),
    };
    match aggregate.compare(&actor, query.from, query.to) {
        Ok(comparison) => Json(comparison).into_response(),
        Err(error) => publication_error_response(error),
    }
}

/// Resolution below reads the workspace's delivery snapshot; build it first.
async fn prime(
    state: &AppState,
    principal: Option<&crate::auth::WorkspacePrincipal>,
) -> Result<(), Response> {
    match principal {
        Some(principal) => crate::ui_delivery::prime_snapshot(state, principal).await,
        None if state.ui_snapshots.is_some() => Err(unavailable_response()),
        None => Ok(()),
    }
}

fn resolve_preview(
    state: &AppState,
    caller: &UiCaller,
    tenant_id: Uuid,
    page_id: &str,
    query: &ResolveQuery,
    patches: &[PublishedPatch],
) -> Result<CallerSafePageResponse, PublicationError> {
    state
        .ui_delivery
        .inspect_draft_page(
            caller,
            &page_request(tenant_id, page_id, query)
                .ok_or_else(|| PublicationError::InvalidDefinition("unsupported host".into()))?,
            patches,
        )
        .map_err(|error| PublicationError::InvalidDefinition(error.diagnostic))?
        .body
        .ok_or_else(|| PublicationError::InvalidDefinition("preview body unavailable".into()))
}

fn page_request(tenant_id: Uuid, page_id: &str, query: &ResolveQuery) -> Option<UiPageRequest> {
    Some(UiPageRequest {
        workspace_id: tenant_id.to_string(),
        page_id: page_id.into(),
        locale: query.locale.clone(),
        host: parse_host(&query.host)?,
        mode: query.mode.clone(),
        form_factor: query.form_factor.clone(),
        audience: query.audience.clone(),
        resource_subtype: query.resource_subtype.clone(),
        record_context: query.record.clone(),
        if_none_match: None,
    })
}

async fn load_or_new(
    state: &AppState,
    tenant_id: Uuid,
    page_id: &str,
) -> Result<TenantPagePublications, PublicationStoreError> {
    Ok(state
        .ui_publications
        .load(tenant_id, page_id)
        .await?
        .unwrap_or_else(|| TenantPagePublications::new(tenant_id, page_id)))
}

fn authorized(
    caller: Option<Extension<UiCaller>>,
    tenant_id: Uuid,
) -> Option<(UiCaller, PublicationActor)> {
    let Extension(caller) = caller?;
    if !caller.active_membership
        || caller.workspace_id != tenant_id.to_string()
        || !caller.permissions.contains(MANAGE_PERMISSION)
    {
        return None;
    }
    let actor_id = Uuid::parse_str(&caller.user_id).ok()?;
    let actor = PublicationActor {
        actor_id,
        permissions: caller.permissions.clone(),
    };
    Some((caller, actor))
}

fn management_response(
    aggregate: TenantPagePublications,
    preview: Option<CallerSafePageResponse>,
    import_plan: Option<ImportPlan>,
    bundle: Option<PublicationBundle>,
) -> Response {
    let revision = aggregate.revision;
    let mut response = Json(ManagementResponse {
        state: aggregate,
        preview,
        import_plan,
        bundle,
    })
    .into_response();
    if let Ok(value) = HeaderValue::from_str(&format!("\"ui-revision-{revision}\"")) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

fn publication_error_response(error: PublicationError) -> Response {
    let (status, code, detail): (StatusCode, &str, Cow<'static, str>) = match error {
        PublicationError::Denied => (
            StatusCode::NOT_FOUND,
            "ui.customisation_unavailable",
            Cow::Borrowed("The requested UI customisation is unavailable."),
        ),
        PublicationError::StaleRevision { .. } | PublicationError::StaleDraft { .. } => (
            StatusCode::PRECONDITION_FAILED,
            "ui.customisation_stale",
            Cow::Borrowed("The UI customisation changed. Reload it and retry."),
        ),
        PublicationError::InvalidDefinition(detail) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            "ui.customisation_invalid",
            Cow::Owned(detail),
        ),
        PublicationError::DraftExists
        | PublicationError::DraftMissing
        | PublicationError::PublicationMissing(_)
        | PublicationError::UnsupportedBundle(_)
        | PublicationError::WrongPage { .. } => (
            StatusCode::CONFLICT,
            "ui.customisation_conflict",
            Cow::Borrowed("The UI customisation operation conflicts with its current state."),
        ),
    };
    problem(status, code, detail.as_ref()).into_response()
}

fn store_error_response(error: PublicationStoreError) -> Response {
    match error {
        PublicationStoreError::StaleRevision { .. } => problem(
            StatusCode::PRECONDITION_FAILED,
            "ui.customisation_stale",
            "The UI customisation changed. Reload it and retry.",
        )
        .into_response(),
        PublicationStoreError::Unavailable | PublicationStoreError::Internal => problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "ui.customisation_unavailable",
            "UI customisation is temporarily unavailable.",
        )
        .into_response(),
    }
}

fn unavailable_response() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "ui.customisation_unavailable",
        "The requested UI customisation is unavailable.",
    )
    .into_response()
}

fn parse_host(value: &str) -> Option<HostProfile> {
    match value {
        "internal-web" => Some(HostProfile::InternalWeb),
        "desktop" => Some(HostProfile::Desktop),
        "portal" => Some(HostProfile::Portal),
        "public-form" => Some(HostProfile::PublicForm),
        _ => None,
    }
}

fn default_view() -> String {
    "draft".into()
}
fn default_locale() -> String {
    "en-GB".into()
}
fn default_host() -> String {
    "internal-web".into()
}
fn default_mode() -> String {
    "view".into()
}
fn default_form_factor() -> String {
    "wide".into()
}

// Keep externally returned contract types compile-checked.
const _: fn(PublicationComparison) = |_| {};

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contract::ui::{
        PageDefinition, PageKind, PatchOperation, UiBundle, UiNode, UI_CONTRACT_VERSION,
    };
    use std::{
        collections::BTreeSet,
        sync::{Arc, Mutex},
    };
    use ui_metadata::{DefinitionVersionRange, MetadataCatalogue, PluginUiPackage, PluginUiState};
    use ui_metadata_delivery::{
        NoopDeliveryObserver, SnapshotError, UiDeliveryService, UiSnapshotSource,
        UiWorkspaceSnapshot,
    };

    #[derive(Default)]
    struct MemoryStore(Mutex<BTreeMap<(Uuid, String), TenantPagePublications>>);

    #[async_trait]
    impl PublicationStore for MemoryStore {
        async fn load(
            &self,
            tenant_id: Uuid,
            page_id: &str,
        ) -> Result<Option<TenantPagePublications>, PublicationStoreError> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(&(tenant_id, page_id.into()))
                .cloned())
        }

        async fn save(
            &self,
            state: &TenantPagePublications,
            expected_revision: u64,
        ) -> Result<(), PublicationStoreError> {
            let mut values = self.0.lock().unwrap();
            let key = (state.tenant_id, state.page_id.clone());
            let actual = values.get(&key).map(|value| value.revision);
            if actual.unwrap_or(0) != expected_revision
                || (actual.is_none() && expected_revision != 0)
            {
                return Err(PublicationStoreError::StaleRevision {
                    expected: expected_revision,
                    actual,
                });
            }
            values.insert(key, state.clone());
            Ok(())
        }
    }

    struct StaticSource {
        tenant_id: String,
        snapshot: UiWorkspaceSnapshot,
    }

    impl UiSnapshotSource for StaticSource {
        fn snapshot(&self, workspace_id: &str) -> Result<UiWorkspaceSnapshot, SnapshotError> {
            if workspace_id != self.tenant_id {
                return Err(SnapshotError::Hidden);
            }
            Ok(self.snapshot.clone())
        }

        fn is_ready(&self) -> bool {
            true
        }
    }

    fn node() -> UiNode {
        UiNode {
            id: "page-root".into(),
            component: "platform.ui.page".into(),
            props: BTreeMap::from([("marker".into(), serde_json::json!("published"))]),
            bindings: BTreeMap::new(),
            conditions: BTreeMap::new(),
            children: Vec::new(),
            permission: None,
            hosts: vec![HostProfile::InternalWeb],
        }
    }

    fn catalogue() -> MetadataCatalogue {
        let mut catalogue = MetadataCatalogue::default();
        catalogue.register_host_component("platform.ui.page");
        catalogue
            .install(PluginUiPackage {
                plugin_id: "platform.sample".into(),
                plugin_version: "1.0.0".into(),
                dependencies: BTreeSet::new(),
                state: PluginUiState::Enabled,
                bundle: UiBundle {
                    pages: vec![PageDefinition {
                        contract_version: UI_CONTRACT_VERSION,
                        id: "platform.sample.record".into(),
                        definition_version: 1,
                        owner_plugin: "platform.sample".into(),
                        kind: PageKind::Record,
                        hosts: vec![HostProfile::InternalWeb],
                        resource: None,
                        data_sources: Vec::new(),
                        actions: Vec::new(),
                        variants: Vec::new(),
                        root: node(),
                    }],
                    ..UiBundle::default()
                },
            })
            .unwrap();
        catalogue
    }

    fn state(tenant_id: Uuid, store: Arc<MemoryStore>) -> AppState {
        let source = Arc::new(StaticSource {
            tenant_id: tenant_id.to_string(),
            snapshot: UiWorkspaceSnapshot {
                catalogue: Arc::new(catalogue()),
                resources: Vec::new(),
                tenant_patches: Vec::new(),
                catalogue_generation: 1,
                publication_generation: 0,
            },
        });
        AppState {
            ui_delivery: Arc::new(UiDeliveryService::new(
                source,
                Arc::new(NoopDeliveryObserver),
            )),
            ui_publications: store,
            ..AppState::default()
        }
    }

    fn caller(tenant_id: Uuid, allowed: bool) -> UiCaller {
        UiCaller {
            user_id: Uuid::now_v7().to_string(),
            workspace_id: tenant_id.to_string(),
            active_membership: true,
            membership_version: 1,
            roles: BTreeSet::new(),
            permissions: if allowed {
                BTreeSet::from([MANAGE_PERMISSION.into()])
            } else {
                BTreeSet::new()
            },
        }
    }

    fn patch(id: &str, marker: &str) -> PublishedPatch {
        PublishedPatch {
            id: id.into(),
            publication_version: 0,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange::exact(1),
            target_node: "page-root".into(),
            operation: PatchOperation::Configure,
            priority: 0,
            node: None,
            props: BTreeMap::from([("marker".into(), serde_json::json!(marker))]),
        }
    }

    async fn mutation(
        state: &AppState,
        caller: &UiCaller,
        tenant_id: Uuid,
        expected_revision: u64,
        operation: MutationOperation,
    ) -> Response {
        mutate(
            State(state.clone()),
            None,
            Some(Extension(caller.clone())),
            Path((tenant_id, "platform.sample.record".into())),
            Json(MutationRequest {
                expected_revision,
                operation,
            }),
        )
        .await
    }

    #[tokio::test]
    async fn management_lifecycle_persists_history_compare_rollback_and_audit() {
        let tenant_id = Uuid::now_v7();
        let store = Arc::new(MemoryStore::default());
        let state = state(tenant_id, store.clone());
        let caller = caller(tenant_id, true);

        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                0,
                MutationOperation::BeginDraft {
                    base_source_versions: BTreeMap::from([(
                        "platform.sample".into(),
                        "1.0.0".into(),
                    )]),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                1,
                MutationOperation::UpdateDraft {
                    expected_draft_version: 1,
                    patches: vec![patch("first", "first")],
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let inspector = preview(
            State(state.clone()),
            None,
            Some(Extension(caller.clone())),
            Path((tenant_id, "platform.sample.record".into())),
            Query(ResolveQuery::default()),
        )
        .await;
        assert_eq!(inspector.status(), StatusCode::OK);
        assert_eq!(
            inspector.headers()[header::CACHE_CONTROL],
            "private, no-store"
        );
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                2,
                MutationOperation::Validate {
                    context: ResolveQuery::default(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                3,
                MutationOperation::Publish {
                    expected_draft_version: 2,
                    context: ResolveQuery::default(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );

        for (revision, id) in [(4, "second")] {
            assert_eq!(
                mutation(
                    &state,
                    &caller,
                    tenant_id,
                    revision,
                    MutationOperation::BeginDraft {
                        base_source_versions: BTreeMap::new(),
                    },
                )
                .await
                .status(),
                StatusCode::OK
            );
            assert_eq!(
                mutation(
                    &state,
                    &caller,
                    tenant_id,
                    revision + 1,
                    MutationOperation::UpdateDraft {
                        expected_draft_version: 1,
                        patches: vec![patch(id, id)],
                    },
                )
                .await
                .status(),
                StatusCode::OK
            );
            assert_eq!(
                mutation(
                    &state,
                    &caller,
                    tenant_id,
                    revision + 2,
                    MutationOperation::Publish {
                        expected_draft_version: 2,
                        context: ResolveQuery::default(),
                    },
                )
                .await
                .status(),
                StatusCode::OK
            );
        }

        let comparison = compare(
            State(state.clone()),
            Some(Extension(caller.clone())),
            Path((tenant_id, "platform.sample.record".into())),
            Query(CompareQuery { from: 1, to: 2 }),
        )
        .await;
        assert_eq!(comparison.status(), StatusCode::OK);
        let comparison_body = axum::body::to_bytes(comparison.into_body(), 16 * 1024)
            .await
            .unwrap();
        let comparison_json: serde_json::Value = serde_json::from_slice(&comparison_body).unwrap();
        assert_eq!(comparison_json["added"], serde_json::json!(["second"]));
        assert_eq!(comparison_json["removed"], serde_json::json!(["first"]));
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                7,
                MutationOperation::Rollback {
                    target_version: 1,
                    context: ResolveQuery::default(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            mutation(&state, &caller, tenant_id, 8, MutationOperation::Export,)
                .await
                .status(),
            StatusCode::OK
        );

        let saved = store
            .load(tenant_id, "platform.sample.record")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(saved.publications.len(), 3);
        assert_eq!(saved.publications[2].rollback_of, Some(1));
        assert_eq!(saved.publication_generation, 3);
        assert!(saved
            .audit
            .iter()
            .any(|event| event.action.as_str() == "draft-validated"));
        assert_eq!(saved.audit.last().unwrap().action.as_str(), "exported");
    }

    #[tokio::test]
    async fn import_dry_run_is_non_mutating_and_apply_creates_only_a_draft() {
        let tenant_id = Uuid::now_v7();
        let store = Arc::new(MemoryStore::default());
        let state = state(tenant_id, store.clone());
        let caller = caller(tenant_id, true);
        let bundle = PublicationBundle {
            format_version: 1,
            page_id: "platform.sample.record".into(),
            base_source_versions: BTreeMap::new(),
            patches: vec![patch("imported", "imported")],
        };
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                0,
                MutationOperation::Import {
                    bundle: bundle.clone(),
                    dry_run: true,
                    context: ResolveQuery::default(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert!(store
            .load(tenant_id, "platform.sample.record")
            .await
            .unwrap()
            .is_none());
        let mut invalid_bundle = bundle.clone();
        invalid_bundle.patches[0].target_node = "missing-anchor".into();
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                0,
                MutationOperation::Import {
                    bundle: invalid_bundle,
                    dry_run: false,
                    context: ResolveQuery::default(),
                },
            )
            .await
            .status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
        assert!(store
            .load(tenant_id, "platform.sample.record")
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            mutation(
                &state,
                &caller,
                tenant_id,
                0,
                MutationOperation::Import {
                    bundle,
                    dry_run: false,
                    context: ResolveQuery::default(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        let saved = store
            .load(tenant_id, "platform.sample.record")
            .await
            .unwrap()
            .unwrap();
        assert!(saved.draft.is_some());
        assert!(saved.publications.is_empty());
    }

    #[tokio::test]
    async fn permissions_workspace_binding_and_stale_revisions_fail_closed() {
        let tenant_id = Uuid::now_v7();
        let other_tenant = Uuid::now_v7();
        let store = Arc::new(MemoryStore::default());
        let state = state(tenant_id, store.clone());
        let denied = caller(tenant_id, false);
        let wrong_workspace = caller(other_tenant, true);
        let operation = MutationOperation::BeginDraft {
            base_source_versions: BTreeMap::new(),
        };
        assert_eq!(
            mutation(&state, &denied, tenant_id, 0, operation)
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(
                State(state.clone()),
                Some(Extension(wrong_workspace)),
                Path((tenant_id, "platform.sample.record".into())),
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        let allowed = caller(tenant_id, true);
        assert_eq!(
            mutation(
                &state,
                &allowed,
                tenant_id,
                0,
                MutationOperation::BeginDraft {
                    base_source_versions: BTreeMap::new(),
                },
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            mutation(
                &state,
                &allowed,
                tenant_id,
                0,
                MutationOperation::Abandon {
                    expected_draft_version: 1,
                },
            )
            .await
            .status(),
            StatusCode::PRECONDITION_FAILED
        );
        assert!(store
            .load(other_tenant, "platform.sample.record")
            .await
            .unwrap()
            .is_none());
    }
}
