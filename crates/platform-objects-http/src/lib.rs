#![forbid(unsafe_code)]
//! The object engine's HTTP routes (ADR 0015), shared by every host that
//! serves them (ADR 0024 §5): the workspace API over PostgreSQL, and the
//! local command bridge over a device's replica. The routes, JSON shapes and
//! problem codes are the same on both; a host supplies only the engine for a
//! verified caller ([`ObjectsHost`]) and inserts that caller ([`Caller`]) as a
//! request extension after its own authentication.

mod handlers;

use async_trait::async_trait;
use axum::extract::DefaultBodyLimit;
use axum::response::Response;
use axum::routing::{get, patch, post};
use axum::Router;
use platform_objects::ObjectService;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use tower_http::request_id::RequestId;
use uuid::Uuid;

pub use handlers::ui_contributions;

/// The verified member a request runs as, which the host inserts as a request
/// extension once it has authenticated the request and checked membership.
#[derive(Debug, Clone)]
pub struct Caller {
    pub tenant_id: Uuid,
    pub user_id: Uuid,
    /// The member's system permissions in the workspace.
    pub permissions: BTreeSet<String>,
}

/// Where the routes find the engine for a caller.
#[async_trait]
pub trait ObjectsHost: Clone + Send + Sync + 'static {
    /// The engine for this caller, with their roles and reporting line, or
    /// the problem response to return when it is unavailable.
    async fn engine(&self, caller: &Caller) -> Result<ObjectService, Response>;

    /// Fills in the display names of the members a response references;
    /// a host without a member directory leaves them as they are.
    async fn name_users(&self, tenant_id: Uuid, value: &mut Value);

    /// The workspace's roles by id, with their names.
    async fn workspace_roles(&self, tenant_id: Uuid) -> Result<BTreeMap<Uuid, String>, Response>;

    /// Display names for these members, as far as the host knows them.
    async fn member_names(&self, tenant_id: Uuid, users: &[Uuid]) -> BTreeMap<Uuid, String>;

    /// Whether the user is a member of the workspace.
    async fn is_member(&self, tenant_id: Uuid, user_id: Uuid) -> Result<bool, Response>;
}

/// The caller's request id as an audit correlation id; a fresh id when it
/// is not a UUID.
pub fn correlation_id(request_id: &RequestId) -> Uuid {
    request_id
        .header_value()
        .to_str()
        .ok()
        .and_then(|value| Uuid::parse_str(value).ok())
        .unwrap_or_else(Uuid::now_v7)
}

/// A reference to a member, with their display name filled in later by
/// [`ObjectsHost::name_users`].
pub fn user_reference(user: Option<Uuid>) -> Value {
    match user {
        Some(id) => json!({ "id": id, "name": Value::Null }),
        None => Value::Null,
    }
}

/// References named in records, pages of records and record history
/// entries (`changed_by`, and `old_user` / `new_user` for owner changes),
/// and in the device list (`member`, `revoked_by`).
const USER_FIELDS: [&str; 8] = [
    "owner",
    "created_by",
    "updated_by",
    "changed_by",
    "old_user",
    "new_user",
    "member",
    "revoked_by",
];

fn record_values(value: &mut Value) -> Vec<&mut Value> {
    if value.get("data").is_some_and(Value::is_array) {
        return value
            .get_mut("data")
            .and_then(Value::as_array_mut)
            .map(|page| page.iter_mut().collect())
            .unwrap_or_default();
    }
    vec![value]
}

/// The members referenced by a record, a page of records or a history
/// page, sorted and without duplicates.
pub fn referenced_users(value: &Value) -> Vec<Uuid> {
    let records: Vec<&Value> = match value.get("data").and_then(Value::as_array) {
        Some(page) => page.iter().collect(),
        None => vec![value],
    };
    let mut ids: Vec<Uuid> = records
        .into_iter()
        .flat_map(|record| {
            USER_FIELDS
                .iter()
                .filter_map(|field| record.get(*field)?.get("id")?.as_str()?.parse().ok())
        })
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// Fills in the names this host knows for the referenced members.
pub fn name_users(value: &mut Value, names: &BTreeMap<Uuid, String>) {
    for record in record_values(value) {
        for field in USER_FIELDS {
            let Some(reference) = record.get_mut(field).and_then(Value::as_object_mut) else {
                continue;
            };
            let name = reference
                .get("id")
                .and_then(Value::as_str)
                .and_then(|id| id.parse::<Uuid>().ok())
                .and_then(|id| names.get(&id));
            if let Some(name) = name {
                reference.insert("name".into(), json!(name));
            }
        }
    }
}

/// Every object engine route, for the host to place behind its own
/// authentication, which must insert the [`Caller`] and a request id.
pub fn routes<H: ObjectsHost>() -> Router<H> {
    Router::new()
        .route(
            "/api/v1/workspaces/:workspace_id/objects",
            get(handlers::list_objects::<H>).post(handlers::create_object::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object",
            get(handlers::describe_object::<H>)
                .patch(handlers::update_object::<H>)
                .delete(handlers::delete_object::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/fields",
            post(handlers::create_field::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/fields/:field",
            patch(handlers::update_field::<H>).delete(handlers::delete_field::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/validation-rules",
            get(handlers::list_validation_rules::<H>).post(handlers::create_validation_rule::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/validation-rules/:rule",
            patch(handlers::update_validation_rule::<H>).delete(handlers::delete_validation_rule::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/quick-actions",
            get(handlers::list_quick_actions::<H>).post(handlers::create_quick_action::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/quick-actions/:action",
            patch(handlers::update_quick_action::<H>).delete(handlers::delete_quick_action::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/reports",
            get(handlers::list_reports::<H>).post(handlers::create_report::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/reports/:report",
            get(handlers::run_report::<H>)
                .patch(handlers::update_report::<H>)
                .delete(handlers::delete_report::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/composite",
            post(handlers::composite::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/sync/revocations",
            post(handlers::revocations::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/bulk",
            post(handlers::submit_bulk_job::<H>).layer(DefaultBodyLimit::max(16 * 1_048_576)),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/bulk/:job",
            get(handlers::bulk_job::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/bulk/:job/results",
            get(handlers::bulk_job_results::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/bulk/:job/abort",
            post(handlers::abort_bulk_job::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/automations",
            get(handlers::list_automations::<H>).post(handlers::create_automation::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/automations/:automation",
            patch(handlers::update_automation::<H>).delete(handlers::delete_automation::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/automations/:automation/runs",
            get(handlers::list_automation_runs::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/automations/:automation/jobs",
            get(handlers::list_automation_jobs::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/automations/:automation/jobs/:job/retry",
            post(handlers::retry_automation_job::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/automations/:automation/jobs/:job/cancel",
            post(handlers::cancel_automation_job::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/permissions",
            get(handlers::object_permissions::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/permissions/:role_id",
            axum::routing::put(handlers::set_object_permission::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records",
            get(handlers::list_records::<H>).post(handlers::create_record::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record",
            get(handlers::get_record::<H>)
                .patch(handlers::update_record::<H>)
                .delete(handlers::delete_record::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/undelete",
            post(handlers::undelete_record::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/search",
            get(handlers::search::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/query",
            get(handlers::run_query::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/changes",
            get(handlers::list_changes::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/changes/stream",
            get(handlers::stream_changes::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/related",
            get(handlers::related_lists::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/related/:child/:field",
            get(handlers::related_records::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/fields/:field/candidates",
            get(handlers::lookup_candidates::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/history",
            get(handlers::record_history::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/sharing",
            get(handlers::record_sharing::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/sharing/:kind/:grantee",
            axum::routing::put(handlers::put_share::<H>).delete(handlers::delete_share::<H>),
        )
        .route(
            "/api/v1/workspaces/:workspace_id/objects/:object/records/:record/owner",
            post(handlers::change_owner::<H>),
        )
}
