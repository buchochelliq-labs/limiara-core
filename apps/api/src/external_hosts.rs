use crate::{problem, AppState};
use async_trait::async_trait;
use axum::{
    body::Bytes,
    extract::{ConnectInfo, Path, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use ui_metadata::{
    external_host::{ExternalPublicationRegistry, OpaqueSlug, PortalPrincipal},
    external_submission::{
        AbuseEvaluator, PublicSubmissionGateway, PublicSubmissionRequest, SubmissionAccepted,
        SubmissionAudit, SubmissionExecutionError, SubmissionExecutor, SubmissionFailure,
        SubmissionPublicCode,
    },
};
use ui_metadata_delivery::{deliver_external_page, CallerSafePageResponse};

const MINIMUM_EXTERNAL_GENERATION: u64 = 1;
const CSRF_COOKIE: &str = "limiara_csrf";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExternalHostUnavailable;

#[async_trait]
pub(crate) trait ExternalHostService: Send + Sync {
    async fn public_form(
        &self,
        slug: &OpaqueSlug,
    ) -> Result<CallerSafePageResponse, ExternalHostUnavailable>;

    async fn portal(
        &self,
        slug: &OpaqueSlug,
        principal: &PortalPrincipal,
    ) -> Result<CallerSafePageResponse, ExternalHostUnavailable>;

    async fn submit(
        &self,
        request: PublicSubmissionRequest,
    ) -> Result<SubmissionAccepted, Box<SubmissionFailure>>;
}

#[async_trait]
trait ExternalPublicationSource: Send + Sync {
    async fn load(
        &self,
        slug_digest: &str,
    ) -> Result<Option<platform_db::ExternalPublicationRecord>, ExternalHostUnavailable>;
}

struct PostgresExternalPublicationSource(sqlx::PgPool);

#[async_trait]
impl ExternalPublicationSource for PostgresExternalPublicationSource {
    async fn load(
        &self,
        slug_digest: &str,
    ) -> Result<Option<platform_db::ExternalPublicationRecord>, ExternalHostUnavailable> {
        platform_db::load_external_publication(&self.0, slug_digest)
            .await
            .map_err(|_| ExternalHostUnavailable)
    }
}

#[allow(dead_code, reason = "used by isolated API state fixtures")]
struct EmptyExternalPublicationSource;

#[async_trait]
impl ExternalPublicationSource for EmptyExternalPublicationSource {
    async fn load(
        &self,
        _slug_digest: &str,
    ) -> Result<Option<platform_db::ExternalPublicationRecord>, ExternalHostUnavailable> {
        Ok(None)
    }
}

pub(crate) trait ExternalAuditSink: Send + Sync {
    fn record(&self, audit: &SubmissionAudit);
}

pub(crate) struct DomainExternalHostService {
    source: Arc<dyn ExternalPublicationSource>,
    submissions: Mutex<PublicSubmissionGateway>,
    abuse: Mutex<Box<dyn AbuseEvaluator>>,
    executor: Mutex<Box<dyn SubmissionExecutor>>,
    audit: Arc<dyn ExternalAuditSink>,
    minimum_generation: u64,
}

impl DomainExternalHostService {
    fn new(
        source: Arc<dyn ExternalPublicationSource>,
        submissions: PublicSubmissionGateway,
        abuse: Box<dyn AbuseEvaluator>,
        executor: Box<dyn SubmissionExecutor>,
        audit: Arc<dyn ExternalAuditSink>,
    ) -> Self {
        Self {
            source,
            submissions: Mutex::new(submissions),
            abuse: Mutex::new(abuse),
            executor: Mutex::new(executor),
            audit,
            minimum_generation: MINIMUM_EXTERNAL_GENERATION,
        }
    }

    #[allow(dead_code, reason = "used by isolated API state fixtures")]
    pub(crate) fn unavailable() -> Arc<dyn ExternalHostService> {
        Arc::new(Self::new(
            Arc::new(EmptyExternalPublicationSource),
            PublicSubmissionGateway::default(),
            Box::new(AllowAbuse),
            Box::new(UnavailableExecutor),
            Arc::new(NoopAudit),
        ))
    }

    pub(crate) fn postgres(pool: sqlx::PgPool) -> Arc<dyn ExternalHostService> {
        Arc::new(Self::new(
            Arc::new(PostgresExternalPublicationSource(pool)),
            PublicSubmissionGateway::default(),
            Box::new(AllowAbuse),
            Box::new(UnavailableExecutor),
            Arc::new(NoopAudit),
        ))
    }

    async fn publication(
        &self,
        slug: &OpaqueSlug,
    ) -> Result<
        (
            ExternalPublicationRegistry,
            Option<ui_metadata::external_submission::PublicSubmissionPolicy>,
        ),
        ExternalHostUnavailable,
    > {
        let record = self
            .source
            .load(&slug.lookup_digest())
            .await?
            .ok_or(ExternalHostUnavailable)?;
        let mut publications = ExternalPublicationRegistry::default();
        publications
            .publish(
                slug,
                record.tenant_id.to_string(),
                record.portal_audience,
                record.compiled,
                record.generation,
                record.enabled_plugins,
            )
            .map_err(|_| ExternalHostUnavailable)?;
        if record.revoked {
            publications
                .revoke(slug)
                .map_err(|_| ExternalHostUnavailable)?;
        }
        Ok((publications, record.submission_policy))
    }
}

struct AllowAbuse;

impl AbuseEvaluator for AllowAbuse {
    fn assess(
        &mut self,
        _context: &ui_metadata::external_submission::AbuseContext<'_>,
    ) -> ui_metadata::external_submission::AbuseDecision {
        ui_metadata::external_submission::AbuseDecision::Allow
    }
}

struct UnavailableExecutor;

impl SubmissionExecutor for UnavailableExecutor {
    fn execute(
        &mut self,
        _tenant_id: &str,
        _action_id: &str,
        _payload: &BTreeMap<String, Value>,
        _correlation_id: &str,
    ) -> Result<String, SubmissionExecutionError> {
        Err(SubmissionExecutionError)
    }
}

struct NoopAudit;

impl ExternalAuditSink for NoopAudit {
    fn record(&self, _audit: &SubmissionAudit) {}
}

#[async_trait]
impl ExternalHostService for DomainExternalHostService {
    async fn public_form(
        &self,
        slug: &OpaqueSlug,
    ) -> Result<CallerSafePageResponse, ExternalHostUnavailable> {
        let (publications, _) = self.publication(slug).await?;
        let compiled = publications
            .resolve(
                slug,
                plugin_contract::ui::HostProfile::PublicForm,
                self.minimum_generation,
            )
            .map_err(|_| ExternalHostUnavailable)?;
        deliver_external_page(compiled, &compiled.public_permissions)
            .map_err(|_| ExternalHostUnavailable)
    }

    async fn portal(
        &self,
        slug: &OpaqueSlug,
        principal: &PortalPrincipal,
    ) -> Result<CallerSafePageResponse, ExternalHostUnavailable> {
        let (publications, _) = self.publication(slug).await?;
        let grant = publications
            .authorize_portal(slug, principal, self.minimum_generation)
            .map_err(|_| ExternalHostUnavailable)?;
        let compiled = publications
            .resolve(
                slug,
                plugin_contract::ui::HostProfile::Portal,
                self.minimum_generation,
            )
            .map_err(|_| ExternalHostUnavailable)?;
        deliver_external_page(compiled, &grant.permissions).map_err(|_| ExternalHostUnavailable)
    }

    async fn submit(
        &self,
        request: PublicSubmissionRequest,
    ) -> Result<SubmissionAccepted, Box<SubmissionFailure>> {
        let result = async {
            let (publications, policy) = self
                .publication(&request.slug)
                .await
                .map_err(|_| unavailable(&request))?;
            let mut submissions = self.submissions.lock().map_err(|_| unavailable(&request))?;
            submissions
                .register_policy(
                    &request.slug,
                    publications
                        .resolve(
                            &request.slug,
                            plugin_contract::ui::HostProfile::PublicForm,
                            self.minimum_generation,
                        )
                        .map_err(|_| unavailable(&request))?,
                    policy.ok_or_else(|| unavailable(&request))?,
                )
                .map_err(|_| unavailable(&request))?;
            let mut abuse = self.abuse.lock().map_err(|_| unavailable(&request))?;
            let mut executor = self.executor.lock().map_err(|_| unavailable(&request))?;
            submissions.submit(
                &publications,
                request.clone(),
                self.minimum_generation,
                abuse.as_mut(),
                executor.as_mut(),
            )
        }
        .await;
        match &result {
            Ok(accepted) => self.audit.record(&accepted.audit),
            Err(failure) => self.audit.record(&failure.audit),
        }
        result
    }
}

#[cfg(test)]
pub(crate) struct UnavailableExternalHostService;

#[cfg(test)]
#[async_trait]
impl ExternalHostService for UnavailableExternalHostService {
    async fn public_form(
        &self,
        _slug: &OpaqueSlug,
    ) -> Result<CallerSafePageResponse, ExternalHostUnavailable> {
        Err(ExternalHostUnavailable)
    }

    async fn portal(
        &self,
        _slug: &OpaqueSlug,
        _principal: &PortalPrincipal,
    ) -> Result<CallerSafePageResponse, ExternalHostUnavailable> {
        Err(ExternalHostUnavailable)
    }

    async fn submit(
        &self,
        request: PublicSubmissionRequest,
    ) -> Result<SubmissionAccepted, Box<SubmissionFailure>> {
        Err(unavailable(&request))
    }
}

/// Session data produced only after cookie, expiry, revocation, tenant membership,
/// audience, and permission verification by the identity boundary (#15-#17).
#[derive(Debug, Clone)]
pub(crate) struct VerifiedPortalSession {
    subject: String,
    tenant_id: String,
    audience: String,
    permissions: BTreeSet<String>,
    session_version: u64,
    active: bool,
}

impl VerifiedPortalSession {
    /// Called by the server-side identity adapter only after it has verified the
    /// durable session and derived its tenant-scoped permission set.
    #[cfg(test)]
    fn from_verified_principal(principal: PortalPrincipal) -> Self {
        Self {
            subject: principal.subject,
            tenant_id: principal.tenant_id,
            audience: principal.audience,
            permissions: principal.permissions,
            session_version: principal.session_version,
            active: principal.active,
        }
    }
}

pub(crate) async fn portal_principal_boundary(mut request: Request, next: Next) -> Response {
    if let Some(verified) = request.extensions_mut().remove::<VerifiedPortalSession>() {
        request.extensions_mut().insert(PortalPrincipal {
            subject: verified.subject,
            tenant_id: verified.tenant_id,
            audience: verified.audience,
            permissions: verified.permissions,
            session_version: verified.session_version,
            active: verified.active,
        });
    }
    next.run(request).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExternalClientFingerprint(pub String);

pub(crate) async fn fingerprint(
    State(state): State<AppState>,
    connect: Option<ConnectInfo<SocketAddr>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let (Some(key), Some(ConnectInfo(peer))) = (&state.external_fingerprint_key, connect) {
        let input = serde_json::json!([key.as_ref(), peer.ip().to_string()]);
        if let Ok(bytes) = serde_json::to_vec(&input) {
            request
                .extensions_mut()
                .insert(ExternalClientFingerprint(format!(
                    "{:x}",
                    Sha256::digest(bytes)
                )));
        }
    }
    next.run(request).await
}

pub(crate) async fn secure_headers(request: Request, next: Next) -> Response {
    secure_external_response(next.run(request).await)
}

#[derive(Debug, Deserialize)]
struct SubmissionBody {
    #[serde(default)]
    payload: BTreeMap<String, Value>,
}

#[derive(Debug, Serialize)]
struct AcceptedBody {
    submission_id: String,
    correlation_id: String,
    replayed: bool,
}

#[derive(Debug, Serialize)]
struct FailureBody {
    code: SubmissionPublicCode,
    correlation_id: String,
    retry: ui_metadata::external_submission::SafeRetryState,
}

pub(crate) async fn public_form(
    State(state): State<AppState>,
    Path(raw_slug): Path<String>,
) -> Response {
    let response = OpaqueSlug::parse(raw_slug)
        .ok()
        .map(|slug| async move { state.external_hosts.public_form(&slug).await.ok() });
    let response = match response {
        Some(response) => response.await,
        None => None,
    }
    .map_or_else(page_unavailable_response, |page| {
        (StatusCode::OK, Json(page)).into_response()
    });
    secure_external_response(response)
}

pub(crate) async fn portal(
    State(state): State<AppState>,
    principal: Option<Extension<PortalPrincipal>>,
    Path(raw_slug): Path<String>,
) -> Response {
    let response = principal.and_then(|Extension(principal)| {
        OpaqueSlug::parse(raw_slug)
            .ok()
            .map(|slug| async move { state.external_hosts.portal(&slug, &principal).await.ok() })
    });
    let response = match response {
        Some(response) => response.await,
        None => None,
    }
    .map_or_else(page_unavailable_response, |page| {
        (StatusCode::OK, Json(page)).into_response()
    });
    secure_external_response(response)
}

pub(crate) async fn submit(
    State(state): State<AppState>,
    fingerprint: Option<Extension<ExternalClientFingerprint>>,
    Path(raw_slug): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let correlation_id = header_string(&headers, "x-request-id").unwrap_or_default();
    let Some(Extension(fingerprint)) = fingerprint else {
        return secure_external_response(page_unavailable_response());
    };
    let Ok(slug) = OpaqueSlug::parse(raw_slug) else {
        return secure_external_response(page_unavailable_response());
    };
    if !is_json(&headers) {
        return secure_external_response(invalid_submission_response(correlation_id));
    }
    let Ok(input) = serde_json::from_slice::<SubmissionBody>(&body) else {
        return secure_external_response(invalid_submission_response(correlation_id));
    };
    let request = PublicSubmissionRequest {
        slug,
        origin: header_string(&headers, header::ORIGIN.as_str()),
        csrf_cookie: csrf_cookie(&headers),
        csrf_header: header_string(&headers, "x-csrf-token"),
        idempotency_key: header_string(&headers, "idempotency-key").unwrap_or_default(),
        client_fingerprint: fingerprint.0,
        body_bytes: body.len(),
        payload: input.payload,
        correlation_id,
        now_ms: now_ms(),
    };
    let response = match state.external_hosts.submit(request).await {
        Ok(accepted) => (
            StatusCode::ACCEPTED,
            Json(AcceptedBody {
                submission_id: accepted.receipt.submission_id,
                correlation_id: accepted.receipt.correlation_id,
                replayed: accepted.replayed,
            }),
        )
            .into_response(),
        Err(failure) => submission_failure_response(*failure),
    };
    secure_external_response(response)
}

fn unavailable(request: &PublicSubmissionRequest) -> Box<SubmissionFailure> {
    Box::new(SubmissionFailure {
        code: SubmissionPublicCode::Unavailable,
        correlation_id: request.correlation_id.clone(),
        retry: ui_metadata::external_submission::SafeRetryState {
            values: BTreeMap::new(),
        },
        audit: SubmissionAudit {
            tenant_id: None,
            correlation_id: request.correlation_id.clone(),
            action_id: None,
            outcome: ui_metadata::external_submission::SubmissionAuditOutcome::Failed,
            field_names: BTreeSet::new(),
            occurred_at_ms: request.now_ms,
        },
    })
}

fn submission_failure_response(failure: SubmissionFailure) -> Response {
    let status = match failure.code {
        SubmissionPublicCode::Unavailable => StatusCode::NOT_FOUND,
        SubmissionPublicCode::Invalid => StatusCode::BAD_REQUEST,
        SubmissionPublicCode::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        SubmissionPublicCode::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        SubmissionPublicCode::IdempotencyConflict => StatusCode::CONFLICT,
        SubmissionPublicCode::VerificationRequired => StatusCode::FORBIDDEN,
        SubmissionPublicCode::Failed => StatusCode::SERVICE_UNAVAILABLE,
    };
    (
        status,
        Json(FailureBody {
            code: failure.code,
            correlation_id: failure.correlation_id,
            retry: failure.retry,
        }),
    )
        .into_response()
}

fn invalid_submission_response(correlation_id: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(FailureBody {
            code: SubmissionPublicCode::Invalid,
            correlation_id,
            retry: ui_metadata::external_submission::SafeRetryState {
                values: BTreeMap::new(),
            },
        }),
    )
        .into_response()
}

fn page_unavailable_response() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "external.page_unavailable",
        "The requested page is unavailable.",
    )
    .into_response()
}

fn secure_external_response(mut response: Response) -> Response {
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store, max-age=0"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("x-content-type-options"),
        HeaderValue::from_static("nosniff"),
    );
    response.headers_mut().insert(
        header::HeaderName::from_static("referrer-policy"),
        HeaderValue::from_static("no-referrer"),
    );
    response
}

fn is_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|media_type| media_type.trim() == "application/json")
        })
}

fn csrf_cookie(headers: &HeaderMap) -> Option<String> {
    let cookies = header_string(headers, header::COOKIE.as_str())?;
    cookies.split(';').find_map(|cookie| {
        let (name, value) = cookie.trim().split_once('=')?;
        (name == CSRF_COOKIE).then(|| value.to_owned())
    })
}

fn header_string(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::{http::Request as HttpRequest, routing::get, Router};
    use plugin_contract::ui::{
        ActionDefinition, ActionKind, ConcurrencyMode, DataClassification, FieldDescriptor,
        FieldSource, FieldType, HostProfile, OperationKind, ResourceDescriptor, ResourceOperation,
        UiNode,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;
    use ui_metadata::{
        external_host::CompiledExternalPage,
        external_submission::{
            AbuseContext, AbuseDecision, PublicSubmissionPolicy, SubmissionExecutionError,
            SubmissionFieldPolicy,
        },
        ResolvedPage,
    };

    struct AllowAbuse;

    impl AbuseEvaluator for AllowAbuse {
        fn assess(&mut self, _context: &AbuseContext<'_>) -> AbuseDecision {
            AbuseDecision::Allow
        }
    }

    struct Executor(Arc<AtomicUsize>);

    impl SubmissionExecutor for Executor {
        fn execute(
            &mut self,
            _tenant_id: &str,
            _action_id: &str,
            _payload: &BTreeMap<String, Value>,
            _correlation_id: &str,
        ) -> Result<String, SubmissionExecutionError> {
            let next = self.0.fetch_add(1, Ordering::SeqCst) + 1;
            Ok(format!("submission-{next}"))
        }
    }

    #[derive(Default)]
    struct Audit(Mutex<Vec<SubmissionAudit>>);

    impl ExternalAuditSink for Audit {
        fn record(&self, audit: &SubmissionAudit) {
            self.0.lock().unwrap().push(audit.clone());
        }
    }

    struct TestSource {
        digest: String,
        record: Mutex<Option<platform_db::ExternalPublicationRecord>>,
    }

    #[async_trait]
    impl ExternalPublicationSource for TestSource {
        async fn load(
            &self,
            slug_digest: &str,
        ) -> Result<Option<platform_db::ExternalPublicationRecord>, ExternalHostUnavailable>
        {
            if slug_digest != self.digest {
                return Ok(None);
            }
            Ok(self.record.lock().unwrap().clone())
        }
    }

    fn slug() -> OpaqueSlug {
        OpaqueSlug::parse("S7m_t10aLmN3vW5uQ8zC2bD9fH4jK6sX").unwrap()
    }

    fn tenant_id() -> uuid::Uuid {
        uuid::Uuid::parse_str("018f0000-0000-7000-8000-000000000001").unwrap()
    }

    fn compiled(host: HostProfile) -> CompiledExternalPage {
        let permission = match host {
            HostProfile::PublicForm => "public.submit",
            HostProfile::Portal => "portal.read",
            _ => unreachable!(),
        };
        CompiledExternalPage {
            host: host.clone(),
            policy_version: 1,
            page: ResolvedPage {
                page_id: "platform.forms.contact".into(),
                definition_version: 1,
                selected_variant: None,
                source_versions: BTreeMap::new(),
                publication_versions: Vec::new(),
                resource: Some("platform.contacts.person".into()),
                data_sources: Vec::new(),
                actions: vec![ActionDefinition {
                    id: "submit-contact".into(),
                    kind: ActionKind::ResourceOperation,
                    target: "platform.contacts.person.create".into(),
                    parameters: BTreeMap::new(),
                    permission: Some(permission.into()),
                }],
                root: UiNode {
                    id: "root".into(),
                    component: "form".into(),
                    props: BTreeMap::new(),
                    bindings: BTreeMap::new(),
                    conditions: BTreeMap::new(),
                    children: vec![UiNode {
                        id: "email".into(),
                        component: "text-input".into(),
                        props: BTreeMap::from([("field".into(), Value::String("email".into()))]),
                        bindings: BTreeMap::new(),
                        conditions: BTreeMap::new(),
                        children: Vec::new(),
                        permission: Some(permission.into()),
                        hosts: vec![host.clone()],
                    }],
                    permission: None,
                    hosts: vec![host.clone()],
                },
                content_hash: "page".into(),
            },
            resources: vec![ResourceDescriptor {
                contract_version: 1,
                id: "platform.contacts.person".into(),
                definition_version: 1,
                owner_plugin: "platform.contacts".into(),
                label: "Person".into(),
                name_field: None,
                fields: vec![FieldDescriptor {
                    api_name: "email".into(),
                    label: "Email".into(),
                    description: None,
                    field_type: FieldType::Email,
                    source: FieldSource::Standard,
                    readable: true,
                    writable: true,
                    required: true,
                    permission: Some(permission.into()),
                    classification: Some(DataClassification::Personal),
                    options: Vec::new(),
                    validation_schema: None,
                    default_value: None,
                    submit_when_hidden: false,
                }],
                relationships: Vec::new(),
                operations: vec![ResourceOperation {
                    id: "create".into(),
                    kind: OperationKind::Create,
                    permission: permission.into(),
                    concurrency: Some(ConcurrencyMode::None),
                    hosts: vec![host.clone()],
                }],
            }],
            dependency_plugins: BTreeSet::from(["platform.contacts".into()]),
            public_permissions: if host == HostProfile::PublicForm {
                BTreeSet::from([permission.into()])
            } else {
                BTreeSet::new()
            },
            portal_permissions: if host == HostProfile::Portal {
                BTreeSet::from([permission.into()])
            } else {
                BTreeSet::new()
            },
            content_hash: format!("{host:?}-hash"),
        }
    }

    fn service(
        host: HostProfile,
    ) -> (
        Arc<DomainExternalHostService>,
        Arc<TestSource>,
        Arc<Audit>,
        Arc<AtomicUsize>,
    ) {
        let compiled = compiled(host.clone());
        let submission_policy = (host == HostProfile::PublicForm).then(|| PublicSubmissionPolicy {
            action_id: "submit-contact".into(),
            allowed_origins: BTreeSet::from(["https://forms.example.test".into()]),
            require_csrf: true,
            max_body_bytes: 4096,
            max_fields: 2,
            rate_window_ms: 60_000,
            rate_requests: 5,
            fields: BTreeMap::from([(
                "email".into(),
                SubmissionFieldPolicy {
                    required: true,
                    max_bytes: 320,
                    retain_on_failure: true,
                },
            )]),
        });
        let source = Arc::new(TestSource {
            digest: slug().lookup_digest(),
            record: Mutex::new(Some(platform_db::ExternalPublicationRecord {
                tenant_id: tenant_id(),
                portal_audience: (host == HostProfile::Portal).then(|| "customers".into()),
                compiled,
                generation: 1,
                enabled_plugins: BTreeSet::from(["platform.contacts".into()]),
                revoked: false,
                submission_policy,
            })),
        });
        let audit = Arc::new(Audit::default());
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(DomainExternalHostService::new(
                source.clone(),
                PublicSubmissionGateway::default(),
                Box::new(AllowAbuse),
                Box::new(Executor(calls.clone())),
                audit.clone(),
            )),
            source,
            audit,
            calls,
        )
    }

    fn state(service: Arc<dyn ExternalHostService>) -> AppState {
        AppState {
            external_hosts: service,
            ..AppState::default()
        }
    }

    fn submission_headers(origin: &str, csrf: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        headers.insert("x-csrf-token", HeaderValue::from_str(csrf).unwrap());
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=x; {CSRF_COOKIE}={csrf}")).unwrap(),
        );
        headers.insert(
            "idempotency-key",
            HeaderValue::from_static("request-0000000001"),
        );
        headers.insert(
            "x-request-id",
            HeaderValue::from_static("request-correlation-1"),
        );
        headers
    }

    #[tokio::test]
    async fn malformed_unknown_and_missing_portal_identity_are_non_enumerating() {
        let state = state(Arc::new(UnavailableExternalHostService));
        let malformed = public_form(State(state.clone()), Path("short".into())).await;
        let unknown = public_form(State(state.clone()), Path(slug().as_str().into())).await;
        let portal = portal(State(state), None, Path(slug().as_str().into())).await;
        let malformed_body = to_bytes(malformed.into_body(), usize::MAX).await.unwrap();
        let unknown_body = to_bytes(unknown.into_body(), usize::MAX).await.unwrap();
        let portal_body = to_bytes(portal.into_body(), usize::MAX).await.unwrap();
        assert_eq!(malformed_body, unknown_body);
        assert_eq!(unknown_body, portal_body);
    }

    #[tokio::test]
    async fn portal_principal_boundary_ignores_headers_and_accepts_only_verified_session() {
        async fn principal_status(principal: Option<Extension<PortalPrincipal>>) -> StatusCode {
            if principal.is_some() {
                StatusCode::OK
            } else {
                StatusCode::UNAUTHORIZED
            }
        }

        let untrusted = Router::new()
            .route("/", get(principal_status))
            .layer(axum::middleware::from_fn(portal_principal_boundary))
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .header("x-portal-subject", "attacker")
                    .header("x-portal-tenant", tenant_id().to_string())
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(untrusted.status(), StatusCode::UNAUTHORIZED);

        let verified = VerifiedPortalSession::from_verified_principal(PortalPrincipal {
            subject: "customer-1".into(),
            tenant_id: tenant_id().to_string(),
            audience: "customers".into(),
            permissions: BTreeSet::from(["portal.read".into()]),
            session_version: 1,
            active: true,
        });
        let trusted = Router::new()
            .route("/", get(principal_status))
            .layer(axum::middleware::from_fn(portal_principal_boundary))
            .layer(Extension(verified))
            .oneshot(
                HttpRequest::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(trusted.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn public_and_portal_delivery_are_permission_filtered_and_no_store() {
        let (public_service, public_source, _, _) = service(HostProfile::PublicForm);
        let public_state = state(public_service.clone());
        let public = public_form(State(public_state.clone()), Path(slug().as_str().into())).await;
        assert_eq!(public.status(), StatusCode::OK);
        assert_eq!(
            public.headers()[header::CACHE_CONTROL],
            "no-store, max-age=0"
        );
        let restarted_service = Arc::new(DomainExternalHostService::new(
            public_source.clone(),
            PublicSubmissionGateway::default(),
            Box::new(AllowAbuse),
            Box::new(UnavailableExecutor),
            Arc::new(NoopAudit),
        ));
        let after_restart = public_form(
            State(state(restarted_service)),
            Path(slug().as_str().into()),
        )
        .await;
        assert_eq!(after_restart.status(), StatusCode::OK);
        public_source
            .record
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .enabled_plugins
            .remove("platform.contacts");
        let disabled = public_form(State(public_state.clone()), Path(slug().as_str().into())).await;
        assert_eq!(disabled.status(), StatusCode::NOT_FOUND);
        public_source
            .record
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .enabled_plugins
            .insert("platform.contacts".into());
        public_source
            .record
            .lock()
            .unwrap()
            .as_mut()
            .unwrap()
            .revoked = true;
        let revoked = public_form(State(public_state), Path(slug().as_str().into())).await;
        assert_eq!(revoked.status(), StatusCode::NOT_FOUND);

        let (portal_service, _, _, _) = service(HostProfile::Portal);
        let principal = PortalPrincipal {
            subject: "customer-1".into(),
            tenant_id: tenant_id().to_string(),
            audience: "customers".into(),
            permissions: BTreeSet::from(["portal.read".into(), "internal.admin".into()]),
            session_version: 1,
            active: true,
        };
        let portal_response = portal(
            State(state(portal_service.clone())),
            Some(Extension(principal.clone())),
            Path(slug().as_str().into()),
        )
        .await;
        assert_eq!(portal_response.status(), StatusCode::OK);
        let mut wrong_tenant = principal;
        wrong_tenant.tenant_id = "tenant-b".into();
        let denied = portal(
            State(state(portal_service)),
            Some(Extension(wrong_tenant)),
            Path(slug().as_str().into()),
        )
        .await;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn submission_transport_enforces_origin_csrf_replay_and_redacts_audit() {
        let (service, _, audit, calls) = service(HostProfile::PublicForm);
        let state = state(service);
        let body = Bytes::from_static(br#"{"payload":{"email":"person@example.test"}}"#);
        let fingerprint = Some(Extension(ExternalClientFingerprint("b".repeat(64))));
        let accepted = submit(
            State(state.clone()),
            fingerprint.clone(),
            Path(slug().as_str().into()),
            submission_headers("https://forms.example.test", &"a".repeat(32)),
            body.clone(),
        )
        .await;
        assert_eq!(accepted.status(), StatusCode::ACCEPTED);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let replay = submit(
            State(state.clone()),
            fingerprint.clone(),
            Path(slug().as_str().into()),
            submission_headers("https://forms.example.test", &"a".repeat(32)),
            body.clone(),
        )
        .await;
        assert_eq!(replay.status(), StatusCode::ACCEPTED);
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let denied = submit(
            State(state.clone()),
            fingerprint,
            Path(slug().as_str().into()),
            submission_headers("https://evil.test", &"a".repeat(32)),
            body,
        )
        .await;
        assert_eq!(denied.status(), StatusCode::NOT_FOUND);
        let denied_body = to_bytes(denied.into_body(), usize::MAX).await.unwrap();
        assert!(!String::from_utf8_lossy(&denied_body).contains("tenant-a"));

        let mut bad_csrf = submission_headers("https://forms.example.test", &"a".repeat(32));
        bad_csrf.insert(
            "x-csrf-token",
            HeaderValue::from_str(&"z".repeat(32)).unwrap(),
        );
        let csrf_denied = submit(
            State(state.clone()),
            Some(Extension(ExternalClientFingerprint("c".repeat(64)))),
            Path(slug().as_str().into()),
            bad_csrf,
            Bytes::from_static(br#"{"payload":{"email":"other@example.test"}}"#),
        )
        .await;
        assert_eq!(csrf_denied.status(), StatusCode::NOT_FOUND);

        let oversized = serde_json::to_vec(&serde_json::json!({
            "payload": {"email": "x".repeat(5_000)}
        }))
        .unwrap();
        let too_large = submit(
            State(state),
            Some(Extension(ExternalClientFingerprint("d".repeat(64)))),
            Path(slug().as_str().into()),
            submission_headers("https://forms.example.test", &"a".repeat(32)),
            Bytes::from(oversized),
        )
        .await;
        assert_eq!(too_large.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(audit.0.lock().unwrap().len(), 5);
    }

    #[tokio::test]
    async fn untrusted_or_malformed_submission_transport_fails_closed() {
        let (service, _, _, _) = service(HostProfile::PublicForm);
        let state = state(service);
        let missing_fingerprint = submit(
            State(state.clone()),
            None,
            Path(slug().as_str().into()),
            submission_headers("https://forms.example.test", &"a".repeat(32)),
            Bytes::from_static(br#"{"payload":{}}"#),
        )
        .await;
        assert_eq!(missing_fingerprint.status(), StatusCode::NOT_FOUND);
        let mut headers = submission_headers("https://forms.example.test", &"a".repeat(32));
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        let wrong_type = submit(
            State(state),
            Some(Extension(ExternalClientFingerprint("b".repeat(64)))),
            Path(slug().as_str().into()),
            headers,
            Bytes::from_static(br#"{"payload":{}}"#),
        )
        .await;
        assert_eq!(wrong_type.status(), StatusCode::BAD_REQUEST);
    }
}
