//! Workspace user records (ADR 0014): what the standard `created_by` and
//! `updated_by` record fields link to. Kernel-owned; plugins never read the
//! user directory.

use crate::{auth::WorkspacePrincipal, problem, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Serialize, PartialEq)]
pub(crate) struct UserRecord {
    id: Uuid,
    name: String,
    email: Option<String>,
    roles: Vec<String>,
    created_at: DateTime<Utc>,
    /// Who the member reports to (ADR 0016), named when still a member.
    manager: Option<Value>,
}

impl From<platform_db::WorkspaceMember> for UserRecord {
    fn from(member: platform_db::WorkspaceMember) -> Self {
        Self {
            id: member.id,
            name: member.display_name,
            email: member.email,
            roles: member.roles,
            created_at: member.created_at,
            manager: None,
        }
    }
}

/// One member of the caller's workspace. An unknown id, a malformed id and a
/// user outside the workspace all get the same `404`, so the route cannot be
/// used to discover users elsewhere.
pub(crate) async fn get_user(
    State(state): State<AppState>,
    Path((_workspace_id, user_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Ok(user_id) = Uuid::parse_str(&user_id) else {
        return not_found();
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    let tenant_id = principal.workspace.tenant_id;
    match platform_db::workspace_member(pool, tenant_id, user_id).await {
        Ok(Some(member)) => {
            let mut record = UserRecord::from(member);
            if let Ok(Some(manager)) =
                platform_db::reporting_manager(pool, tenant_id, user_id).await
            {
                let mut reference = user_reference(Some(manager));
                if let Ok(names) =
                    platform_db::member_display_names(pool, tenant_id, &[manager]).await
                {
                    if let (Some(name), Some(map)) =
                        (names.get(&manager), reference.as_object_mut())
                    {
                        map.insert("name".into(), json!(name));
                    }
                }
                record.manager = Some(reference);
            }
            (StatusCode::OK, Json(record)).into_response()
        }
        Ok(None) => not_found(),
        Err(error) => {
            tracing::error!(
                error_type = %std::any::type_name_of_val(&error),
                "workspace member lookup failed"
            );
            unavailable()
        }
    }
}

/// The workspace's members with who each reports to, for owner and share
/// pickers and the reporting line editor.
pub(crate) async fn list_users(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::workspace_members(pool, principal.workspace.tenant_id).await {
        Ok(members) => Json(json!({
            "data": members
                .into_iter()
                .map(|member| json!({
                    "id": member.id,
                    "name": member.display_name,
                    "manager_id": member.manager_user_id,
                }))
                .collect::<Vec<_>>(),
            "can_manage": principal.workspace.permissions.contains(MEMBERS_MANAGE_PERMISSION),
        }))
        .into_response(),
        Err(_) => unavailable(),
    }
}

/// The workspace's roles by name, so members can share records with
/// everyone holding a role (ADR 0016). Permissions are not disclosed.
pub(crate) async fn list_roles(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::workspace_roles(pool, principal.workspace.tenant_id).await {
        Ok(roles) => Json(json!({
            "data": roles
                .into_iter()
                .map(|(id, name)| json!({"id": id, "name": name}))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(_) => unavailable(),
    }
}

/// Set who workspace members report to (ADR 0016).
pub(crate) const MEMBERS_MANAGE_PERMISSION: &str = "members.manage";

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManagerChange {
    manager_id: Option<Uuid>,
}

/// Sets or clears who a member reports to; needs `members.manage`.
pub(crate) async fn set_manager(
    State(state): State<AppState>,
    Path((_workspace_id, user_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<tower_http::request_id::RequestId>,
    body: Result<Json<ManagerChange>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !principal
        .workspace
        .permissions
        .contains(MEMBERS_MANAGE_PERMISSION)
    {
        return problem(
            StatusCode::FORBIDDEN,
            "auth.permission_denied",
            "permission denied",
        )
        .into_response();
    }
    let Ok(user_id) = Uuid::parse_str(&user_id) else {
        return not_found();
    };
    let Ok(Json(change)) = body else {
        return problem(
            StatusCode::BAD_REQUEST,
            "validation.failed",
            "the request body is invalid",
        )
        .into_response();
    };
    let Some(pool) = state.pool.as_ref() else {
        return unavailable();
    };
    match platform_db::set_reporting_line(
        pool,
        principal.workspace.tenant_id,
        user_id,
        change.manager_id,
        principal.session.user.id,
        Some(crate::correlation_id(&request_id)),
    )
    .await
    {
        Ok(()) => Json(json!({"id": user_id, "manager_id": change.manager_id})).into_response(),
        Err(platform_db::ReportingLineError::NotMember) => not_found(),
        Err(platform_db::ReportingLineError::Cycle) => problem(
            StatusCode::CONFLICT,
            "members.reporting_cycle",
            "the manager already reports to this member",
        )
        .into_response(),
        Err(platform_db::ReportingLineError::Sqlx(_)) => unavailable(),
    }
}

fn not_found() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "resource.unavailable",
        "the requested resource is unavailable",
    )
    .into_response()
}

fn unavailable() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "platform.unavailable",
        "the service is temporarily unavailable",
    )
    .into_response()
}

/// Fills in member names for the standard `owner`, `created_by` and
/// `updated_by` fields of one record or a page of records (ADR 0014). Users
/// who are no longer members keep their id and no name; lookups never leave
/// the caller's workspace. The change has already committed, so a failed
/// lookup leaves references unnamed rather than failing the response.
pub(crate) async fn name_record_users(state: &AppState, tenant_id: Uuid, value: &mut Value) {
    let Some(pool) = &state.pool else {
        return;
    };
    let ids = referenced_users(value);
    match platform_db::member_display_names(pool, tenant_id, &ids).await {
        Ok(names) => name_users(value, &names),
        Err(error) => tracing::warn!(
            error_type = %std::any::type_name_of_val(&error),
            "member names unavailable for record response"
        ),
    }
}

pub(crate) use platform_objects_http::user_reference;

pub(crate) use platform_objects_http::{name_users, referenced_users};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_records_serialise_member_fields_only() {
        let record = UserRecord::from(platform_db::WorkspaceMember {
            id: Uuid::from_u128(7),
            display_name: "Grace Hopper".into(),
            email: Some("grace@example.org".into()),
            roles: vec!["Administrator".into()],
            created_at: DateTime::UNIX_EPOCH,
        });
        let value = serde_json::to_value(&record).expect("serialise");
        assert_eq!(value["name"], "Grace Hopper");
        assert_eq!(value["roles"], serde_json::json!(["Administrator"]));
        let keys: Vec<_> = value.as_object().expect("object").keys().cloned().collect();
        assert_eq!(
            keys,
            ["created_at", "email", "id", "manager", "name", "roles"]
        );
        assert!(value["manager"].is_null(), "no reporting line by default");
    }
}
