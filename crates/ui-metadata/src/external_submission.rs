//! Mediated anonymous submission for compiled public-form publications.

use crate::external_host::{CompiledExternalPage, ExternalPublicationRegistry, OpaqueSlug};
use plugin_contract::ui::HostProfile;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

const ABSOLUTE_BODY_LIMIT: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionFieldPolicy {
    pub required: bool,
    pub max_bytes: usize,
    pub retain_on_failure: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicSubmissionPolicy {
    pub action_id: String,
    pub allowed_origins: BTreeSet<String>,
    pub require_csrf: bool,
    pub max_body_bytes: usize,
    pub max_fields: usize,
    pub rate_window_ms: u64,
    pub rate_requests: u32,
    pub fields: BTreeMap<String, SubmissionFieldPolicy>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicSubmissionRequest {
    pub slug: OpaqueSlug,
    pub origin: Option<String>,
    pub csrf_cookie: Option<String>,
    pub csrf_header: Option<String>,
    pub idempotency_key: String,
    /// A one-way, deployment-keyed fingerprint. Raw IPs/device IDs are forbidden.
    pub client_fingerprint: String,
    pub body_bytes: usize,
    pub payload: BTreeMap<String, Value>,
    pub correlation_id: String,
    pub now_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmissionPublicCode {
    Unavailable,
    Invalid,
    TooLarge,
    RateLimited,
    IdempotencyConflict,
    VerificationRequired,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SafeRetryState {
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionReceipt {
    pub submission_id: String,
    pub correlation_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SubmissionAuditOutcome {
    Accepted,
    Rejected,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmissionAudit {
    pub tenant_id: Option<String>,
    pub correlation_id: String,
    pub action_id: Option<String>,
    pub outcome: SubmissionAuditOutcome,
    pub field_names: BTreeSet<String>,
    pub occurred_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionFailure {
    pub code: SubmissionPublicCode,
    pub correlation_id: String,
    pub retry: SafeRetryState,
    pub audit: SubmissionAudit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubmissionAccepted {
    pub receipt: SubmissionReceipt,
    pub replayed: bool,
    pub audit: SubmissionAudit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbuseDecision {
    Allow,
    Deny,
    Challenge,
}

pub struct AbuseContext<'a> {
    pub publication_hash: &'a str,
    pub client_fingerprint: &'a str,
    pub field_names: &'a BTreeSet<String>,
}

pub trait AbuseEvaluator: Send {
    fn assess(&mut self, context: &AbuseContext<'_>) -> AbuseDecision;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("submission execution failed")]
pub struct SubmissionExecutionError;

pub trait SubmissionExecutor: Send {
    fn execute(
        &mut self,
        tenant_id: &str,
        action_id: &str,
        payload: &BTreeMap<String, Value>,
        correlation_id: &str,
    ) -> Result<String, SubmissionExecutionError>;
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SubmissionPolicyError {
    #[error("submission policy requires a compiled public-form page")]
    NotPublicForm,
    #[error("submission action is not present in the compiled page")]
    UnknownAction,
    #[error("submission field {0} is not present in the compiled resources")]
    UnknownField(String),
    #[error("submission limits are invalid")]
    InvalidLimits,
    #[error("submission origin {0} is invalid")]
    InvalidOrigin(String),
}

#[derive(Debug, Clone)]
struct IdempotencyRecord {
    payload_hash: String,
    receipt: SubmissionReceipt,
}

#[derive(Debug, Clone, Copy)]
struct RateWindow {
    starts_at_ms: u64,
    count: u32,
}

#[derive(Debug, Default)]
pub struct PublicSubmissionGateway {
    policies: BTreeMap<String, PublicSubmissionPolicy>,
    idempotency: BTreeMap<(String, String), IdempotencyRecord>,
    rates: BTreeMap<(String, String), RateWindow>,
}

impl PublicSubmissionGateway {
    pub fn register_policy(
        &mut self,
        slug: &OpaqueSlug,
        compiled: &CompiledExternalPage,
        policy: PublicSubmissionPolicy,
    ) -> Result<(), SubmissionPolicyError> {
        if compiled.host != HostProfile::PublicForm {
            return Err(SubmissionPolicyError::NotPublicForm);
        }
        if !compiled
            .page
            .actions
            .iter()
            .any(|action| action.id == policy.action_id)
        {
            return Err(SubmissionPolicyError::UnknownAction);
        }
        if policy.max_body_bytes == 0
            || policy.max_body_bytes > ABSOLUTE_BODY_LIMIT
            || policy.max_fields == 0
            || policy.rate_window_ms == 0
            || policy.rate_requests == 0
        {
            return Err(SubmissionPolicyError::InvalidLimits);
        }
        let compiled_fields: BTreeSet<_> = compiled
            .resources
            .iter()
            .flat_map(|resource| resource.fields.iter().map(|field| field.api_name.as_str()))
            .collect();
        for (field, field_policy) in &policy.fields {
            if !compiled_fields.contains(field.as_str()) {
                return Err(SubmissionPolicyError::UnknownField(field.clone()));
            }
            if field_policy.max_bytes == 0 || field_policy.max_bytes > policy.max_body_bytes {
                return Err(SubmissionPolicyError::InvalidLimits);
            }
        }
        for origin in &policy.allowed_origins {
            if !valid_origin(origin) {
                return Err(SubmissionPolicyError::InvalidOrigin(origin.clone()));
            }
        }
        self.policies.insert(slug.digest(), policy);
        Ok(())
    }

    pub fn submit(
        &mut self,
        publications: &ExternalPublicationRegistry,
        request: PublicSubmissionRequest,
        minimum_generation: u64,
        abuse: &mut dyn AbuseEvaluator,
        executor: &mut dyn SubmissionExecutor,
    ) -> Result<SubmissionAccepted, Box<SubmissionFailure>> {
        let empty_retry = SafeRetryState {
            values: BTreeMap::new(),
        };
        if request.body_bytes > ABSOLUTE_BODY_LIMIT {
            return Err(failure(
                SubmissionPublicCode::TooLarge,
                &request,
                None,
                None,
                empty_retry,
            ));
        }
        if !valid_idempotency_key(&request.idempotency_key)
            || !valid_fingerprint(&request.client_fingerprint)
            || !valid_correlation_id(&request.correlation_id)
        {
            return Err(failure(
                SubmissionPublicCode::Invalid,
                &request,
                None,
                None,
                empty_retry,
            ));
        }
        let tenant_id = publications
            .public_grant(&request.slug, minimum_generation)
            .map_err(|_| {
                failure(
                    SubmissionPublicCode::Unavailable,
                    &request,
                    None,
                    None,
                    empty_retry.clone(),
                )
            })?;
        let publication_hash = request.slug.digest();
        let policy = self
            .policies
            .get(&publication_hash)
            .cloned()
            .ok_or_else(|| {
                failure(
                    SubmissionPublicCode::Unavailable,
                    &request,
                    None,
                    None,
                    empty_retry.clone(),
                )
            })?;
        let retry = safe_retry(&request.payload, &policy);
        if request.body_bytes > policy.max_body_bytes || request.payload.len() > policy.max_fields {
            return Err(failure(
                SubmissionPublicCode::TooLarge,
                &request,
                Some(tenant_id),
                Some(&policy.action_id),
                retry,
            ));
        }
        if !origin_allowed(request.origin.as_deref(), &policy)
            || (policy.require_csrf
                && !csrf_matches(
                    request.csrf_cookie.as_deref(),
                    request.csrf_header.as_deref(),
                ))
        {
            return Err(failure(
                SubmissionPublicCode::Unavailable,
                &request,
                Some(tenant_id),
                Some(&policy.action_id),
                retry,
            ));
        }
        if !payload_allowed(&request.payload, &policy) {
            return Err(failure(
                SubmissionPublicCode::Invalid,
                &request,
                Some(tenant_id),
                Some(&policy.action_id),
                retry,
            ));
        }
        let payload_hash = payload_hash(&request.payload);
        let idempotency_key = (publication_hash.clone(), request.idempotency_key.clone());
        if let Some(record) = self.idempotency.get(&idempotency_key) {
            if record.payload_hash == payload_hash {
                return Ok(SubmissionAccepted {
                    receipt: record.receipt.clone(),
                    replayed: true,
                    audit: audit(
                        &request,
                        Some(tenant_id),
                        Some(&policy.action_id),
                        SubmissionAuditOutcome::Accepted,
                    ),
                });
            }
            return Err(failure(
                SubmissionPublicCode::IdempotencyConflict,
                &request,
                Some(tenant_id),
                Some(&policy.action_id),
                retry,
            ));
        }
        if !self.take_rate_slot(&publication_hash, &request, &policy) {
            return Err(failure(
                SubmissionPublicCode::RateLimited,
                &request,
                Some(tenant_id),
                Some(&policy.action_id),
                retry,
            ));
        }
        let field_names = request.payload.keys().cloned().collect();
        match abuse.assess(&AbuseContext {
            publication_hash: &publication_hash,
            client_fingerprint: &request.client_fingerprint,
            field_names: &field_names,
        }) {
            AbuseDecision::Deny => {
                return Err(failure(
                    SubmissionPublicCode::Unavailable,
                    &request,
                    Some(tenant_id),
                    Some(&policy.action_id),
                    retry,
                ));
            }
            AbuseDecision::Challenge => {
                return Err(failure(
                    SubmissionPublicCode::VerificationRequired,
                    &request,
                    Some(tenant_id),
                    Some(&policy.action_id),
                    retry,
                ));
            }
            AbuseDecision::Allow => {}
        }
        let submission_id = executor
            .execute(
                tenant_id,
                &policy.action_id,
                &request.payload,
                &request.correlation_id,
            )
            .map_err(|_| {
                failure(
                    SubmissionPublicCode::Failed,
                    &request,
                    Some(tenant_id),
                    Some(&policy.action_id),
                    retry.clone(),
                )
            })?;
        let receipt = SubmissionReceipt {
            submission_id,
            correlation_id: request.correlation_id.clone(),
        };
        self.idempotency.insert(
            idempotency_key,
            IdempotencyRecord {
                payload_hash,
                receipt: receipt.clone(),
            },
        );
        Ok(SubmissionAccepted {
            receipt,
            replayed: false,
            audit: audit(
                &request,
                Some(tenant_id),
                Some(&policy.action_id),
                SubmissionAuditOutcome::Accepted,
            ),
        })
    }

    fn take_rate_slot(
        &mut self,
        publication_hash: &str,
        request: &PublicSubmissionRequest,
        policy: &PublicSubmissionPolicy,
    ) -> bool {
        let key = (
            publication_hash.to_owned(),
            request.client_fingerprint.clone(),
        );
        let window = self.rates.entry(key).or_insert(RateWindow {
            starts_at_ms: request.now_ms,
            count: 0,
        });
        if request.now_ms.saturating_sub(window.starts_at_ms) >= policy.rate_window_ms {
            *window = RateWindow {
                starts_at_ms: request.now_ms,
                count: 0,
            };
        }
        if window.count >= policy.rate_requests {
            return false;
        }
        window.count += 1;
        true
    }
}

fn payload_allowed(payload: &BTreeMap<String, Value>, policy: &PublicSubmissionPolicy) -> bool {
    if payload
        .keys()
        .any(|field| !policy.fields.contains_key(field))
    {
        return false;
    }
    for (field, field_policy) in &policy.fields {
        let value = payload.get(field);
        if field_policy.required && value.is_none_or(Value::is_null) {
            return false;
        }
        if value.is_some_and(|value| value_size(value) > field_policy.max_bytes) {
            return false;
        }
    }
    true
}

fn value_size(value: &Value) -> usize {
    serde_json::to_vec(value).map_or(usize::MAX, |value| value.len())
}

fn safe_retry(
    payload: &BTreeMap<String, Value>,
    policy: &PublicSubmissionPolicy,
) -> SafeRetryState {
    SafeRetryState {
        values: payload
            .iter()
            .filter(|(field, value)| {
                policy.fields.get(*field).is_some_and(|field_policy| {
                    field_policy.retain_on_failure && value_size(value) <= field_policy.max_bytes
                })
            })
            .map(|(field, value)| (field.clone(), value.clone()))
            .collect(),
    }
}

fn failure(
    code: SubmissionPublicCode,
    request: &PublicSubmissionRequest,
    tenant_id: Option<&str>,
    action_id: Option<&str>,
    retry: SafeRetryState,
) -> Box<SubmissionFailure> {
    Box::new(SubmissionFailure {
        code,
        correlation_id: request.correlation_id.clone(),
        retry,
        audit: audit(
            request,
            tenant_id,
            action_id,
            if code == SubmissionPublicCode::Failed {
                SubmissionAuditOutcome::Failed
            } else {
                SubmissionAuditOutcome::Rejected
            },
        ),
    })
}

fn audit(
    request: &PublicSubmissionRequest,
    tenant_id: Option<&str>,
    action_id: Option<&str>,
    outcome: SubmissionAuditOutcome,
) -> SubmissionAudit {
    SubmissionAudit {
        tenant_id: tenant_id.map(str::to_owned),
        correlation_id: request.correlation_id.clone(),
        action_id: action_id.map(str::to_owned),
        outcome,
        field_names: request.payload.keys().cloned().collect(),
        occurred_at_ms: request.now_ms,
    }
}

fn payload_hash(payload: &BTreeMap<String, Value>) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(payload).expect("submission payload serializes"))
    )
}

fn valid_idempotency_key(value: &str) -> bool {
    (16..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

fn valid_fingerprint(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_correlation_id(value: &str) -> bool {
    (16..=96).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_origin(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("https://") else {
        return false;
    };
    !rest.is_empty()
        && !rest.contains('/')
        && !rest.contains('?')
        && !rest.contains('#')
        && !rest.contains('@')
}

fn origin_allowed(origin: Option<&str>, policy: &PublicSubmissionPolicy) -> bool {
    origin.is_some_and(|origin| policy.allowed_origins.contains(origin))
}

fn csrf_matches(cookie: Option<&str>, header: Option<&str>) -> bool {
    match (cookie, header) {
        (Some(cookie), Some(header))
            if (32..=128).contains(&cookie.len()) && cookie.len() == header.len() =>
        {
            cookie
                .bytes()
                .zip(header.bytes())
                .fold(0_u8, |difference, (left, right)| {
                    difference | (left ^ right)
                })
                == 0
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        external_host::{
            CompiledExternalPage, ExternalPublicationError, ExternalPublicationRegistry,
        },
        ResolvedPage,
    };
    use plugin_contract::ui::{
        ActionDefinition, ActionKind, DataClassification, FieldDescriptor, FieldSource, FieldType,
        ResourceDescriptor,
    };

    fn field(name: &str) -> FieldDescriptor {
        FieldDescriptor {
            api_name: name.into(),
            label: name.into(),
            description: None,
            field_type: FieldType::Text,
            source: FieldSource::Standard,
            readable: true,
            writable: true,
            required: false,
            permission: None,
            classification: Some(DataClassification::Personal),
            options: Vec::new(),
            validation_schema: None,
            default_value: None,
            submit_when_hidden: false,
        }
    }

    fn compiled(host: HostProfile) -> CompiledExternalPage {
        CompiledExternalPage {
            host,
            policy_version: 1,
            page: ResolvedPage {
                page_id: "platform.forms.contact".into(),
                definition_version: 1,
                selected_variant: None,
                source_versions: BTreeMap::from([("platform.contacts".into(), "1.0.0".into())]),
                publication_versions: vec![1],
                resource: Some("platform.contacts.person".into()),
                data_sources: Vec::new(),
                actions: vec![ActionDefinition {
                    id: "submit-contact".into(),
                    kind: ActionKind::Submit,
                    target: "submit".into(),
                    parameters: BTreeMap::new(),
                    permission: None,
                }],
                root: plugin_contract::ui::UiNode {
                    id: "root".into(),
                    component: "platform.ui.form".into(),
                    props: BTreeMap::new(),
                    bindings: BTreeMap::new(),
                    conditions: BTreeMap::new(),
                    children: Vec::new(),
                    permission: None,
                    hosts: vec![HostProfile::PublicForm],
                },
                content_hash: "resolved".into(),
            },
            resources: vec![ResourceDescriptor {
                contract_version: 1,
                id: "platform.contacts.person".into(),
                definition_version: 1,
                owner_plugin: "platform.contacts".into(),
                label: "Person".into(),
                name_field: None,
                fields: vec![field("email"), field("message"), field("secret")],
                relationships: Vec::new(),
                operations: Vec::new(),
            }],
            dependency_plugins: BTreeSet::from(["platform.contacts".into()]),
            public_permissions: BTreeSet::new(),
            portal_permissions: BTreeSet::new(),
            content_hash: "a".repeat(64),
        }
    }

    fn policy() -> PublicSubmissionPolicy {
        PublicSubmissionPolicy {
            action_id: "submit-contact".into(),
            allowed_origins: BTreeSet::from(["https://forms.example.test".into()]),
            require_csrf: true,
            max_body_bytes: 4_096,
            max_fields: 3,
            rate_window_ms: 60_000,
            rate_requests: 2,
            fields: BTreeMap::from([
                (
                    "email".into(),
                    SubmissionFieldPolicy {
                        required: true,
                        max_bytes: 320,
                        retain_on_failure: true,
                    },
                ),
                (
                    "message".into(),
                    SubmissionFieldPolicy {
                        required: false,
                        max_bytes: 2_000,
                        retain_on_failure: true,
                    },
                ),
                (
                    "secret".into(),
                    SubmissionFieldPolicy {
                        required: false,
                        max_bytes: 128,
                        retain_on_failure: false,
                    },
                ),
            ]),
        }
    }

    fn slug() -> OpaqueSlug {
        OpaqueSlug::parse("S7m_t10aLmN3vW5uQ8zC2bD9fH4jK6sX").unwrap()
    }

    fn publication_registry(compiled: &CompiledExternalPage) -> ExternalPublicationRegistry {
        let mut registry = ExternalPublicationRegistry::default();
        registry
            .publish(
                &slug(),
                "tenant-a",
                None,
                compiled.clone(),
                3,
                BTreeSet::from(["platform.contacts".into()]),
            )
            .unwrap();
        registry
    }

    fn request() -> PublicSubmissionRequest {
        PublicSubmissionRequest {
            slug: slug(),
            origin: Some("https://forms.example.test".into()),
            csrf_cookie: Some("a".repeat(32)),
            csrf_header: Some("a".repeat(32)),
            idempotency_key: "request-0000000001".into(),
            client_fingerprint: "b".repeat(64),
            body_bytes: 128,
            payload: BTreeMap::from([
                ("email".into(), Value::String("person@example.test".into())),
                ("message".into(), Value::String("Please call me".into())),
                ("secret".into(), Value::String("do-not-retain".into())),
            ]),
            correlation_id: "correlation-000001".into(),
            now_ms: 1_000,
        }
    }

    struct Abuse(AbuseDecision);

    impl AbuseEvaluator for Abuse {
        fn assess(&mut self, _context: &AbuseContext<'_>) -> AbuseDecision {
            self.0
        }
    }

    #[derive(Default)]
    struct Executor {
        calls: Vec<(String, String, String)>,
        fail: bool,
    }

    impl SubmissionExecutor for Executor {
        fn execute(
            &mut self,
            tenant_id: &str,
            action_id: &str,
            _payload: &BTreeMap<String, Value>,
            correlation_id: &str,
        ) -> Result<String, SubmissionExecutionError> {
            self.calls
                .push((tenant_id.into(), action_id.into(), correlation_id.into()));
            if self.fail {
                Err(SubmissionExecutionError)
            } else {
                Ok(format!("submission-{}", self.calls.len()))
            }
        }
    }

    #[test]
    fn policy_registration_rejects_portal_unknown_fields_actions_limits_and_origins() {
        let mut gateway = PublicSubmissionGateway::default();
        assert_eq!(
            gateway.register_policy(&slug(), &compiled(HostProfile::Portal), policy()),
            Err(SubmissionPolicyError::NotPublicForm)
        );
        let public = compiled(HostProfile::PublicForm);

        let mut candidate = policy();
        candidate.action_id = "internal-action".into();
        assert_eq!(
            gateway.register_policy(&slug(), &public, candidate),
            Err(SubmissionPolicyError::UnknownAction)
        );

        let mut candidate = policy();
        candidate.fields.insert(
            "internal_score".into(),
            SubmissionFieldPolicy {
                required: false,
                max_bytes: 10,
                retain_on_failure: false,
            },
        );
        assert_eq!(
            gateway.register_policy(&slug(), &public, candidate),
            Err(SubmissionPolicyError::UnknownField("internal_score".into()))
        );

        let mut candidate = policy();
        candidate.max_body_bytes = ABSOLUTE_BODY_LIMIT + 1;
        assert_eq!(
            gateway.register_policy(&slug(), &public, candidate),
            Err(SubmissionPolicyError::InvalidLimits)
        );

        let mut candidate = policy();
        candidate.allowed_origins = BTreeSet::from(["https://user@evil.test".into()]);
        assert_eq!(
            gateway.register_policy(&slug(), &public, candidate),
            Err(SubmissionPolicyError::InvalidOrigin(
                "https://user@evil.test".into()
            ))
        );
    }

    #[test]
    fn accepted_submission_is_tenant_selected_audited_and_idempotently_replayed() {
        let public = compiled(HostProfile::PublicForm);
        let mut registry = publication_registry(&public);
        let mut gateway = PublicSubmissionGateway::default();
        gateway.register_policy(&slug(), &public, policy()).unwrap();
        let mut abuse = Abuse(AbuseDecision::Allow);
        let mut executor = Executor::default();

        let accepted = gateway
            .submit(&registry, request(), 3, &mut abuse, &mut executor)
            .unwrap();
        assert!(!accepted.replayed);
        assert_eq!(accepted.receipt.submission_id, "submission-1");
        assert_eq!(accepted.audit.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(accepted.audit.action_id.as_deref(), Some("submit-contact"));
        assert_eq!(executor.calls[0].0, "tenant-a");

        let replay = gateway
            .submit(&registry, request(), 3, &mut abuse, &mut executor)
            .unwrap();
        assert!(replay.replayed);
        assert_eq!(replay.receipt, accepted.receipt);
        assert_eq!(executor.calls.len(), 1);

        let mut conflict = request();
        conflict
            .payload
            .insert("message".into(), Value::String("Different".into()));
        let failure = gateway
            .submit(&registry, conflict, 3, &mut abuse, &mut executor)
            .unwrap_err();
        assert_eq!(failure.code, SubmissionPublicCode::IdempotencyConflict);
        assert_eq!(executor.calls.len(), 1);

        let tenant_b_slug = OpaqueSlug::parse("T8n_u21bMnO4wX6vR9aD3cE0gI5kL7tY").unwrap();
        gateway
            .register_policy(&tenant_b_slug, &public, policy())
            .unwrap();
        registry
            .publish(
                &tenant_b_slug,
                "tenant-a",
                None,
                public,
                3,
                BTreeSet::from(["platform.contacts".into()]),
            )
            .unwrap();
        let mut tenant_b_request = request();
        tenant_b_request.slug = tenant_b_slug;
        let tenant_b = gateway
            .submit(&registry, tenant_b_request, 3, &mut abuse, &mut executor)
            .unwrap();
        assert!(!tenant_b.replayed);
        assert_eq!(tenant_b.audit.tenant_id.as_deref(), Some("tenant-a"));
        assert_eq!(executor.calls.len(), 2);
    }

    #[test]
    fn unknown_revoked_origin_and_csrf_fail_without_enumerating_publication_state() {
        let public = compiled(HostProfile::PublicForm);
        let mut registry = publication_registry(&public);
        let mut gateway = PublicSubmissionGateway::default();
        gateway.register_policy(&slug(), &public, policy()).unwrap();
        let mut abuse = Abuse(AbuseDecision::Allow);
        let mut executor = Executor::default();

        let mut unknown = request();
        unknown.slug = OpaqueSlug::parse("U7m_t10aLmN3vW5uQ8zC2bD9fH4jK6sZ").unwrap();
        let unknown_failure = gateway
            .submit(&registry, unknown, 3, &mut abuse, &mut executor)
            .unwrap_err();
        registry.revoke(&slug()).unwrap();
        let revoked_failure = gateway
            .submit(&registry, request(), 3, &mut abuse, &mut executor)
            .unwrap_err();
        assert_eq!(unknown_failure.code, SubmissionPublicCode::Unavailable);
        assert_eq!(revoked_failure.code, SubmissionPublicCode::Unavailable);
        assert_eq!(unknown_failure.audit.tenant_id, None);
        assert_eq!(revoked_failure.audit.tenant_id, None);

        let registry = publication_registry(&public);
        let mut bad_origin = request();
        bad_origin.origin = Some("https://evil.test".into());
        assert_eq!(
            gateway
                .submit(&registry, bad_origin, 3, &mut abuse, &mut executor)
                .unwrap_err()
                .code,
            SubmissionPublicCode::Unavailable
        );
        let mut bad_csrf = request();
        bad_csrf.csrf_header = Some("z".repeat(32));
        assert_eq!(
            gateway
                .submit(&registry, bad_csrf, 3, &mut abuse, &mut executor)
                .unwrap_err()
                .code,
            SubmissionPublicCode::Unavailable
        );
        assert!(executor.calls.is_empty());
    }

    #[test]
    fn payload_rate_abuse_and_execution_failures_retain_only_safe_fields() {
        let public = compiled(HostProfile::PublicForm);
        let registry = publication_registry(&public);
        let mut limited_policy = policy();
        limited_policy.rate_requests = 1;
        let mut gateway = PublicSubmissionGateway::default();
        gateway
            .register_policy(&slug(), &public, limited_policy)
            .unwrap();
        let mut abuse = Abuse(AbuseDecision::Allow);
        let mut executor = Executor::default();

        let mut too_large = request();
        too_large.body_bytes = 5_000;
        let failure = gateway
            .submit(&registry, too_large, 3, &mut abuse, &mut executor)
            .unwrap_err();
        assert_eq!(failure.code, SubmissionPublicCode::TooLarge);
        assert!(failure.retry.values.contains_key("email"));
        assert!(!failure.retry.values.contains_key("secret"));

        gateway
            .submit(&registry, request(), 3, &mut abuse, &mut executor)
            .unwrap();
        let mut second = request();
        second.idempotency_key = "request-0000000002".into();
        assert_eq!(
            gateway
                .submit(&registry, second, 3, &mut abuse, &mut executor)
                .unwrap_err()
                .code,
            SubmissionPublicCode::RateLimited
        );

        let mut gateway = PublicSubmissionGateway::default();
        gateway.register_policy(&slug(), &public, policy()).unwrap();
        let mut challenge = Abuse(AbuseDecision::Challenge);
        assert_eq!(
            gateway
                .submit(&registry, request(), 3, &mut challenge, &mut executor)
                .unwrap_err()
                .code,
            SubmissionPublicCode::VerificationRequired
        );

        let mut gateway = PublicSubmissionGateway::default();
        gateway.register_policy(&slug(), &public, policy()).unwrap();
        let mut executor = Executor {
            fail: true,
            ..Executor::default()
        };
        let failure = gateway
            .submit(&registry, request(), 3, &mut abuse, &mut executor)
            .unwrap_err();
        assert_eq!(failure.code, SubmissionPublicCode::Failed);
        assert_eq!(failure.audit.outcome, SubmissionAuditOutcome::Failed);
        assert!(failure.retry.values.contains_key("message"));
        assert!(!failure.retry.values.contains_key("secret"));
        assert!(!failure
            .audit
            .field_names
            .iter()
            .any(|field| field.contains("do-not-retain")));
    }

    #[test]
    fn disabled_dependency_and_stale_generation_are_indistinguishable() {
        let public = compiled(HostProfile::PublicForm);
        let mut registry = publication_registry(&public);
        let mut gateway = PublicSubmissionGateway::default();
        gateway.register_policy(&slug(), &public, policy()).unwrap();
        let mut abuse = Abuse(AbuseDecision::Allow);
        let mut executor = Executor::default();
        let stale = gateway
            .submit(&registry, request(), 4, &mut abuse, &mut executor)
            .unwrap_err();
        registry.disable_plugin("platform.contacts");
        let disabled = gateway
            .submit(&registry, request(), 3, &mut abuse, &mut executor)
            .unwrap_err();
        assert_eq!(stale.code, SubmissionPublicCode::Unavailable);
        assert_eq!(disabled.code, SubmissionPublicCode::Unavailable);
        assert_eq!(
            registry.resolve(&slug(), HostProfile::PublicForm, 3),
            Err(ExternalPublicationError::Unavailable)
        );
    }
}
