//! The handlers for metadata-driven objects (ADR 0015): object and field
//! definitions, object permissions, and records of every object.
//!
//! The host verifies the caller and supplies the engine; the engine decides
//! everything else. Responses keep stable problem codes.

use crate::{Caller, ObjectsHost};
use axum::{
    extract::{rejection::QueryRejection, Path, Query, RawQuery, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use platform_objects::composite::{CompositeOperation, CompositeOutcome, CompositeRequest};
use platform_objects::service::ListRecords;
use platform_objects::{
    AccessFlags, Automation, CreateAutomation, CreateField, CreateObject, CreateValidationRule,
    FieldDefinition, GranteeKind, MutationContext, ObjectDefinition, ObjectError, ObjectService,
    Record, RecordSort, RecordStatus, ShareAccess, SortDirection, UpdateAutomation, UpdateField,
    UpdateObject, UpdateValidationRule, ValidationIssue, ValidationRule,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use tower_http::request_id::RequestId;
use uuid::Uuid;

#[derive(Serialize)]
struct Problem {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    status: u16,
    code: &'static str,
    detail: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<ValidationIssue>,
}

fn problem(
    status: StatusCode,
    code: &'static str,
    detail: &'static str,
    errors: Vec<ValidationIssue>,
) -> Response {
    (
        status,
        Json(Problem {
            kind: "about:blank",
            title: status.canonical_reason().unwrap_or("error"),
            status: status.as_u16(),
            code,
            detail,
            errors,
        }),
    )
        .into_response()
}

/// The status and problem an engine error maps to.
fn error_problem(
    error: ObjectError,
) -> (StatusCode, &'static str, &'static str, Vec<ValidationIssue>) {
    match error {
        ObjectError::NotFound => (
            StatusCode::NOT_FOUND,
            "resource.unavailable",
            "the requested resource is unavailable",
            Vec::new(),
        ),
        ObjectError::PermissionDenied => (
            StatusCode::FORBIDDEN,
            "auth.permission_denied",
            "permission denied",
            Vec::new(),
        ),
        ObjectError::Validation(errors) => (
            StatusCode::BAD_REQUEST,
            "validation.failed",
            "one or more values are invalid",
            errors,
        ),
        ObjectError::VersionConflict => (
            StatusCode::CONFLICT,
            "records.version_conflict",
            "the item changed since it was read",
            Vec::new(),
        ),
        ObjectError::Conflict("changes.position_expired") => (
            StatusCode::GONE,
            "changes.position_expired",
            "the change log no longer holds that position; resynchronise with touched_after",
            Vec::new(),
        ),
        ObjectError::Conflict("query.timeout") => (
            StatusCode::SERVICE_UNAVAILABLE,
            "query.timeout",
            "the query took too long; filter on more selective indexed fields",
            Vec::new(),
        ),
        ObjectError::Conflict(code) => (
            StatusCode::CONFLICT,
            code,
            "the change conflicts with the current state",
            Vec::new(),
        ),
        ObjectError::Database(error) => {
            tracing::error!(
                error_type = %std::any::type_name_of_val(&error),
                "object engine storage failed"
            );
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "platform.unavailable",
                "the service is temporarily unavailable",
                Vec::new(),
            )
        }
        ObjectError::Invariant => {
            tracing::error!("object engine invariant violated");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "platform.unavailable",
                "the service is temporarily unavailable",
                Vec::new(),
            )
        }
    }
}

fn error_response(error: ObjectError) -> Response {
    let (status, code, detail, errors) = error_problem(error);
    problem(status, code, detail, errors)
}

/// A problem body without the response around it, for multi-part results.
fn problem_value(error: ObjectError) -> (StatusCode, Value) {
    let (status, code, detail, errors) = error_problem(error);
    let body = serde_json::to_value(Problem {
        kind: "about:blank",
        title: status.canonical_reason().unwrap_or("error"),
        status: status.as_u16(),
        code,
        detail,
        errors,
    })
    .unwrap_or(Value::Null);
    (status, body)
}

fn invalid_query() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "validation.failed",
        "one or more values are invalid",
        vec![ValidationIssue::new(
            "query",
            "query.invalid",
            "one or more query parameters are invalid",
        )],
    )
}

fn context(request_id: &RequestId) -> MutationContext {
    MutationContext::new(crate::correlation_id(request_id))
}

/// The engine service for this request's verified member.
async fn service<H: ObjectsHost>(state: &H, caller: &Caller) -> Result<ObjectService, Response> {
    state.engine(caller).await
}

/// Navigation contributions for engine objects the caller can read and an
/// administrator placed in navigation, plus the engine's system permissions
/// the caller holds. The shell discovers both from the UI contribution
/// endpoint like any feature's (ADR 0015); when the engine is unavailable
/// the workspace simply shows no object entries.
pub async fn ui_contributions<H: ObjectsHost>(
    state: &H,
    caller: &Caller,
) -> (Vec<Value>, Vec<String>) {
    let permissions = [
        platform_objects::OBJECTS_MANAGE_PERMISSION,
        platform_objects::RECORDS_VIEW_ALL_PERMISSION,
        platform_objects::RECORDS_MODIFY_ALL_PERMISSION,
        platform_objects::RECORDS_TRANSFER_PERMISSION,
        platform_objects::REPORTS_MANAGE_PERMISSION,
    ]
    .into_iter()
    .filter(|permission| caller.permissions.contains(*permission))
    .map(str::to_owned)
    .collect();
    let Ok(service) = service(state, caller).await else {
        return (Vec::new(), permissions);
    };
    let objects = match service.list_objects().await {
        Ok(objects) => objects,
        Err(_) => {
            tracing::warn!("object navigation unavailable");
            return (Vec::new(), permissions);
        }
    };
    // Reports run over the objects the caller can read (ADR 0027).
    let reports = json!({
        "id": "reports",
        "label": "Reports",
        "route": "/reports",
        "icon": "reports",
        "order": REPORTS_NAVIGATION_ORDER,
    });
    let navigation = objects
        .iter()
        .filter(|(object, access)| object.in_navigation && access.read)
        .enumerate()
        .map(|(position, (object, _))| {
            json!({
                "id": format!("objects.{}", object.api_name),
                "label": object.plural_label,
                "route": format!("/o/{}", object.api_name),
                "icon": "cube",
                "order": OBJECT_NAVIGATION_ORDER + i32::try_from(position).unwrap_or(0),
            })
        })
        .chain(std::iter::once(reports))
        .collect();
    (navigation, permissions)
}

/// Engine objects follow feature navigation.
const OBJECT_NAVIGATION_ORDER: i32 = 1000;
/// Reports follow the objects they report on.
const REPORTS_NAVIGATION_ORDER: i32 = 5000;

// --- Serialisation -------------------------------------------------------

fn field_json(field: &FieldDefinition) -> Value {
    json!({
        "api_name": field.api_name,
        "label": field.label,
        "description": field.description,
        "field_type": field.field_type,
        "origin": field.origin,
        "required": field.required,
        "unique": field.unique,
        "indexed": field.is_indexed(),
        "searchable": field.searchable,
        "track_history": field.track_history,
        "max_length": field.max_length,
        "scale": field.scale,
        "options": field.options,
        "default_value": field.default_value,
        "reference_object": field.reference_object,
        "delete_behaviour": field.delete_behaviour,
        "lookup_filter": field.lookup_filter,
        "formula": field.formula,
        "formula_type": field.formula_type,
        "rollup": field.rollup,
        "position": field.position,
        "version": field.version,
    })
}

fn object_json(object: &ObjectDefinition, access: Option<AccessFlags>, with_fields: bool) -> Value {
    let mut value = json!({
        "api_name": object.api_name,
        "label": object.label,
        "plural_label": object.plural_label,
        "description": object.description,
        "origin": object.origin,
        "owner_plugin": object.owner_plugin,
        "key_prefix": object.key_prefix,
        "name_label": object.name_label,
        "sharing_model": object.sharing_model,
        "grant_hierarchy": object.grant_hierarchy,
        "track_history": object.track_history,
        "searchable": object.searchable,
        "in_navigation": object.in_navigation,
        "version": object.version,
        "created_at": object.created_at,
        "updated_at": object.updated_at,
    });
    if let (Some(access), Some(map)) = (access, value.as_object_mut()) {
        map.insert("access".into(), json!(access));
    }
    if let (true, Some(map)) = (with_fields, value.as_object_mut()) {
        map.insert(
            "fields".into(),
            Value::Array(object.fields.iter().map(field_json).collect()),
        );
    }
    value
}

/// A record as callers see it: the standard fields of ADR 0014 followed by
/// custom values keyed by field API name.
/// Records as callers see them, with the names of lookup targets the caller
/// can read under `related` (target id to name).
async fn records_json(service: &ObjectService, object: &str, records: &[Record]) -> Vec<Value> {
    let names = match service.related_names(object, records).await {
        Ok(names) => names,
        Err(_) => {
            tracing::warn!("lookup names unavailable");
            BTreeMap::new()
        }
    };
    records
        .iter()
        .map(|record| record_json(object, record, &names))
        .collect()
}

async fn one_record_json(service: &ObjectService, object: &str, record: &Record) -> Value {
    records_json(service, object, std::slice::from_ref(record))
        .await
        .pop()
        .unwrap_or(Value::Null)
}

fn record_json(object: &str, record: &Record, names: &BTreeMap<String, String>) -> Value {
    let mut value = json!({
        "id": record.public_id,
        "object": object,
        "name": record.name,
        "owner": crate::user_reference(record.owner_id),
        "created_at": record.created_at,
        "created_by": crate::user_reference(record.created_by),
        "updated_at": record.updated_at,
        "updated_by": crate::user_reference(record.updated_by),
        "touched_at": record.touched_at,
        "version": record.version,
        "archived_at": record.archived_at,
        "is_deleted": record.deleted_at.is_some(),
        "deleted_at": record.deleted_at,
    });
    if let Some(map) = value.as_object_mut() {
        let mut related = Map::new();
        for (key, field_value) in &record.values {
            if let Some((id, name)) = field_value.as_str().and_then(|id| names.get_key_value(id)) {
                related.insert(id.clone(), json!(name));
            }
            map.insert(key.clone(), field_value.clone());
        }
        map.insert("related".into(), Value::Object(related));
    }
    value
}

async fn respond_records<H: ObjectsHost>(
    state: &H,
    caller: &Caller,
    status: StatusCode,
    mut value: Value,
) -> Response {
    state.name_users(caller.tenant_id, &mut value).await;
    (status, Json(value)).into_response()
}

// --- Metadata routes -------------------------------------------------------

pub async fn list_objects<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list_objects().await {
        Ok(objects) => Json(json!({
            "data": objects
                .iter()
                .map(|(object, access)| object_json(object, Some(*access), false))
                .collect::<Vec<_>>(),
            "can_manage": service.caller().can_manage(),
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn create_object<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<CreateObject>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.create_object(input, context(&request_id)).await {
        Ok(object) => (StatusCode::CREATED, Json(object_json(&object, None, true))).into_response(),
        Err(error) => error_response(error),
    }
}

fn invalid_body() -> Response {
    problem(
        StatusCode::BAD_REQUEST,
        "validation.failed",
        "one or more values are invalid",
        vec![ValidationIssue::new(
            "body",
            "request.invalid",
            "the request body is invalid",
        )],
    )
}

pub async fn describe_object<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.describe(&object).await {
        Ok((object, access)) => Json(object_json(&object, Some(access), true)).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn update_object<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<UpdateObject>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .update_object(&object, input, context(&request_id))
        .await
    {
        Ok(object) => Json(object_json(&object, None, true)).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize)]
pub(crate) struct ExpectedVersion {
    expected_version: i64,
}

pub async fn delete_object<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .delete_object(&object, version.expected_version, context(&request_id))
        .await
    {
        Ok(object) => Json(object_json(&object, None, false)).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn create_field<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<CreateField>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .create_field(&object, input, context(&request_id))
        .await
    {
        Ok(field) => (StatusCode::CREATED, Json(field_json(&field))).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn update_field<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, field)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<UpdateField>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .update_field(&object, &field, input, context(&request_id))
        .await
    {
        Ok(field) => Json(field_json(&field)).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn delete_field<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, field)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .delete_field(
            &object,
            &field,
            version.expected_version,
            context(&request_id),
        )
        .await
    {
        Ok(field) => Json(field_json(&field)).into_response(),
        Err(error) => error_response(error),
    }
}

// --- Validation rules (ADR 0021) --------------------------------------------

fn rule_json(rule: &ValidationRule) -> Value {
    json!({
        "name": rule.name,
        "description": rule.description,
        "condition": rule.condition,
        "message": rule.message,
        "error_field": rule.error_field,
        "active": rule.active,
        "version": rule.version,
        "created_at": rule.created_at,
        "created_by": crate::user_reference(rule.created_by),
        "updated_at": rule.updated_at,
        "updated_by": crate::user_reference(rule.updated_by),
    })
}

pub async fn list_validation_rules<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.validation_rules(&object).await {
        Ok(rules) => {
            Json(json!({"data": rules.iter().map(rule_json).collect::<Vec<_>>()})).into_response()
        }
        Err(error) => error_response(error),
    }
}

pub async fn create_validation_rule<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<CreateValidationRule>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .create_validation_rule(&object, input, context(&request_id))
        .await
    {
        Ok(rule) => (StatusCode::CREATED, Json(rule_json(&rule))).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn update_validation_rule<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, rule)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<UpdateValidationRule>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .update_validation_rule(&object, &rule, input, context(&request_id))
        .await
    {
        Ok(rule) => Json(rule_json(&rule)).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn delete_validation_rule<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, rule)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .delete_validation_rule(
            &object,
            &rule,
            version.expected_version,
            context(&request_id),
        )
        .await
    {
        Ok(rule) => Json(rule_json(&rule)).into_response(),
        Err(error) => error_response(error),
    }
}

// --- Reports (ADR 0027) ------------------------------------------------------

fn report_json(report: &platform_objects::Report, can_edit: bool) -> Value {
    json!({
        "id": report.id,
        "name": report.name,
        "description": report.description,
        "query": report.query,
        "chart": report.chart,
        "label_column": report.label_column,
        "value_column": report.value_column,
        "visibility": report.visibility,
        "owner": crate::user_reference(Some(report.owner_id)),
        "version": report.version,
        "created_at": report.created_at,
        "created_by": crate::user_reference(report.created_by),
        "updated_at": report.updated_at,
        "updated_by": crate::user_reference(report.updated_by),
        "can_edit": can_edit,
    })
}

/// `GET /reports`: the reports the caller sees.
pub async fn list_reports<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list_reports().await {
        Ok(reports) => {
            let mut value = json!({
                "data": reports
                    .iter()
                    .map(|report| report_json(report, service.can_edit_report(report)))
                    .collect::<Vec<_>>(),
                "can_manage": caller
                    .permissions
                    .contains(platform_objects::REPORTS_MANAGE_PERMISSION),
            });
            state.name_users(caller.tenant_id, &mut value).await;
            Json(value).into_response()
        }
        Err(error) => error_response(error),
    }
}

pub async fn create_report<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<platform_objects::CreateReport>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.create_report(input, context(&request_id)).await {
        Ok(report) => {
            let mut value = report_json(&report, true);
            state.name_users(caller.tenant_id, &mut value).await;
            (StatusCode::CREATED, Json(value)).into_response()
        }
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReportRunParams {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    page_size: Option<u16>,
}

/// `GET /reports/{id}`: the report and its rows, run as the caller.
pub async fn run_report<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, id)): Path<(Uuid, Uuid)>,
    Extension(caller): Extension<Caller>,
    params: Result<Query<ReportRunParams>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .run_report(id, params.cursor, params.page_size)
        .await
    {
        Ok((report, result)) => {
            let mut value = report_json(&report, service.can_edit_report(&report));
            value["result"] = json!({
                "object": result.object,
                "columns": result.columns,
                "data": result.rows,
                "next_cursor": result.next_cursor,
            });
            state.name_users(caller.tenant_id, &mut value).await;
            Json(value).into_response()
        }
        Err(error) => error_response(error),
    }
}

pub async fn update_report<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, id)): Path<(Uuid, Uuid)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<platform_objects::UpdateReport>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.update_report(id, input, context(&request_id)).await {
        Ok(report) => {
            let mut value = report_json(&report, true);
            state.name_users(caller.tenant_id, &mut value).await;
            Json(value).into_response()
        }
        Err(error) => error_response(error),
    }
}

pub async fn delete_report<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, id)): Path<(Uuid, Uuid)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .delete_report(id, version.expected_version, context(&request_id))
        .await
    {
        Ok(report) => Json(report_json(&report, true)).into_response(),
        Err(error) => error_response(error),
    }
}

// --- Automations (ADR 0022) ------------------------------------------------

fn automation_json(automation: &Automation) -> Value {
    json!({
        "name": automation.name,
        "description": automation.description,
        "trigger": automation.trigger,
        "timing": automation.timing,
        "condition": automation.condition,
        "actions": automation.actions,
        "schedule": automation.schedule,
        "active": automation.active,
        "version": automation.version,
        "created_at": automation.created_at,
        "created_by": crate::user_reference(automation.created_by),
        "updated_at": automation.updated_at,
        "updated_by": crate::user_reference(automation.updated_by),
    })
}

pub async fn list_automations<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.automations(&object).await {
        Ok(automations) => Json(json!({
            "data": automations.iter().map(automation_json).collect::<Vec<_>>()
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn create_automation<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<CreateAutomation>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .create_automation(&object, input, context(&request_id))
        .await
    {
        Ok(automation) => (StatusCode::CREATED, Json(automation_json(&automation))).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn update_automation<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, automation)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<UpdateAutomation>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .update_automation(&object, &automation, input, context(&request_id))
        .await
    {
        Ok(automation) => Json(automation_json(&automation)).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn delete_automation<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, automation)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .delete_automation(
            &object,
            &automation,
            version.expected_version,
            context(&request_id),
        )
        .await
    {
        Ok(automation) => Json(automation_json(&automation)).into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunsQuery {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

fn run_json(run: &platform_objects::AutomationRun) -> Value {
    json!({
        "id": run.id,
        "record": run.record,
        "event": run.event,
        "outcome": run.outcome,
        "detail": run.detail,
        "saves": run.saves,
        "ran_at": run.ran_at,
        "ran_by": crate::user_reference(run.ran_by),
        "correlation_id": run.correlation_id,
    })
}

/// An automation's runs, newest first (ADR 0022 §5).
pub async fn list_automation_runs<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, automation)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<RunsQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .automation_runs(
            &object,
            &automation,
            query.cursor.as_deref(),
            query.limit.unwrap_or(50),
        )
        .await
    {
        Ok((runs, next)) => Json(json!({
            "data": runs.iter().map(run_json).collect::<Vec<_>>(),
            "next_cursor": next,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

fn job_json(job: &platform_objects::AutomationJob) -> Value {
    json!({
        "id": job.id,
        "record": job.record,
        "event": job.event,
        "scheduled": job.scheduled,
        "due_at": job.due_at,
        "status": job.status,
        "attempts": job.attempts,
        "last_error": job.last_error,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
    })
}

/// An automation's queued jobs, newest first (ADR 0022 §6).
pub async fn list_automation_jobs<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, automation)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<RunsQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    if query.cursor.is_some() {
        return invalid_query();
    }
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .automation_jobs(&object, &automation, query.limit.unwrap_or(50))
        .await
    {
        Ok(jobs) => {
            Json(json!({"data": jobs.iter().map(job_json).collect::<Vec<_>>()})).into_response()
        }
        Err(error) => error_response(error),
    }
}

async fn change_job<H: ObjectsHost>(
    state: H,
    (object, automation, job): (String, String, Uuid),
    caller: Caller,
    request_id: RequestId,
    retry: bool,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .change_automation_job(&object, &automation, job, retry, context(&request_id))
        .await
    {
        Ok(job) => Json(job_json(&job)).into_response(),
        Err(error) => error_response(error),
    }
}

/// Retries a failed job now (ADR 0022 §6).
pub async fn retry_automation_job<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, automation, job)): Path<(Uuid, String, String, Uuid)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    change_job(state, (object, automation, job), caller, request_id, true).await
}

/// Cancels a pending job (ADR 0022 §6).
pub async fn cancel_automation_job<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, automation, job)): Path<(Uuid, String, String, Uuid)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    change_job(state, (object, automation, job), caller, request_id, false).await
}

// --- Composite requests (ADR 0023) ------------------------------------------

/// Several record changes in one call, optionally all or none.
pub async fn composite<H: ObjectsHost>(
    State(state): State<H>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<CompositeRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    // A record goes only to a member of the workspace, as on the owner
    // route.
    for (index, operation) in request.operations.iter().enumerate() {
        let CompositeOperation::ChangeOwner { owner_id, .. } = operation else {
            continue;
        };
        match grantee_exists(&state, caller.tenant_id, GranteeKind::User, *owner_id).await {
            Ok(true) => {}
            Ok(false) => {
                return error_response(ObjectError::invalid(
                    format!("operations[{index}].owner_id"),
                    "records.owner_unknown",
                    "choose a member of this workspace",
                ))
            }
            Err(response) => return response,
        }
    }
    let outcomes = match service.composite(&request, context(&request_id)).await {
        Ok(outcomes) => outcomes,
        Err(error) => return error_response(error),
    };
    let mut results = Vec::with_capacity(outcomes.len());
    for (operation, (reference, outcome)) in request.operations.iter().zip(outcomes) {
        let object = operation.object();
        results.push(match outcome {
            CompositeOutcome::Done { record, created } => json!({
                "reference": reference,
                "status": if created { 201 } else { 200 },
                "body": one_record_json(&service, object, &record).await,
            }),
            CompositeOutcome::Failed(error) => {
                let (status, body) = problem_value(error);
                json!({"reference": reference, "status": status.as_u16(), "body": body})
            }
            CompositeOutcome::RolledBack => {
                json!({"reference": reference, "status": 409, "body": {"code": "composite.rolled_back"}})
            }
            CompositeOutcome::NotRun => {
                json!({"reference": reference, "status": 424, "body": {"code": "composite.not_run"}})
            }
        });
    }
    respond_records(
        &state,
        &caller,
        StatusCode::OK,
        json!({"all_or_none": request.all_or_none, "results": results}),
    )
    .await
}

// --- Device sync (ADR 0024 §4) ----------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevocationsRequest {
    /// Public record IDs the device holds, by object API name.
    records: BTreeMap<String, Vec<String>>,
}

/// `POST /sync/revocations`: of the records a device holds, those the
/// member can no longer read. The device deletes them from its replica.
pub async fn revocations<H: ObjectsHost>(
    State(state): State<H>,
    Extension(caller): Extension<Caller>,
    body: Result<Json<RevocationsRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.revocations(&request.records).await {
        Ok(revoked) => Json(json!({"revoked": revoked})).into_response(),
        Err(error) => error_response(error),
    }
}

// --- Bulk jobs (ADR 0023 §2) ------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkRequest {
    object: String,
    operation: platform_objects::BulkOperation,
    rows: Vec<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BulkRowsQuery {
    status: Option<platform_objects::BulkRowStatus>,
    after: Option<i32>,
    limit: Option<u32>,
}

fn bulk_json(job: &platform_objects::BulkJob) -> Value {
    json!({
        "id": job.id,
        "object": job.object,
        "operation": job.operation,
        "status": job.status,
        "total": job.total,
        "succeeded": job.succeeded,
        "failed": job.failed,
        "created_by": crate::user_reference(Some(job.actor_id)),
        "correlation_id": job.correlation_id,
        "created_at": job.created_at,
        "updated_at": job.updated_at,
        "completed_at": job.completed_at,
    })
}

async fn respond_bulk<H: ObjectsHost>(
    state: &H,
    caller: &Caller,
    status: StatusCode,
    job: &platform_objects::BulkJob,
) -> Response {
    respond_records(state, caller, status, bulk_json(job)).await
}

/// Queues a bulk job; the worker applies its rows in the background.
pub async fn submit_bulk_job<H: ObjectsHost>(
    State(state): State<H>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<BulkRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(request)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .submit_bulk_job(
            &request.object,
            request.operation,
            &request.rows,
            context(&request_id),
        )
        .await
    {
        Ok(job) => respond_bulk(&state, &caller, StatusCode::ACCEPTED, &job).await,
        Err(error) => error_response(error),
    }
}

/// A bulk job's progress.
pub async fn bulk_job<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, job)): Path<(Uuid, Uuid)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.bulk_job(job).await {
        Ok(job) => respond_bulk(&state, &caller, StatusCode::OK, &job).await,
        Err(error) => error_response(error),
    }
}

/// A bulk job's row results in order, a page at a time.
pub async fn bulk_job_results<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, job)): Path<(Uuid, Uuid)>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<BulkRowsQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    let limit = query.limit.unwrap_or(200);
    match service
        .bulk_job_rows(job, query.status, query.after, limit)
        .await
    {
        Ok(rows) => {
            let next = (rows.len() == usize::try_from(limit).unwrap_or(usize::MAX))
                .then(|| rows.last().map(|row| row.index))
                .flatten();
            Json(json!({
                "data": rows
                    .iter()
                    .map(|row| json!({
                        "index": row.index,
                        "status": row.status,
                        "record": row.record,
                        "errors": row.errors,
                    }))
                    .collect::<Vec<_>>(),
                "next_after": next,
            }))
            .into_response()
        }
        Err(error) => error_response(error),
    }
}

/// Stops a queued or running bulk job.
pub async fn abort_bulk_job<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, job)): Path<(Uuid, Uuid)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.abort_bulk_job(job, context(&request_id)).await {
        Ok(job) => respond_bulk(&state, &caller, StatusCode::OK, &job).await,
        Err(error) => error_response(error),
    }
}

pub async fn object_permissions<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    let granted = match service.object_permissions(&object).await {
        Ok(granted) => granted,
        Err(error) => return error_response(error),
    };
    let roles = match state.workspace_roles(caller.tenant_id).await {
        Ok(roles) => roles,
        Err(response) => return response,
    };
    // Every workspace role, with its grant or no access.
    let data: Vec<Value> = roles
        .iter()
        .map(|(role_id, name)| {
            let access = granted
                .iter()
                .find(|permission| permission.role_id == *role_id)
                .map(|permission| permission.access)
                .unwrap_or_default();
            json!({"role_id": role_id, "role_name": name, "access": access})
        })
        .collect();
    Json(json!({"data": data})).into_response()
}

pub async fn set_object_permission<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, role_id)): Path<(Uuid, String, Uuid)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<AccessFlags>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(access)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    if !service.caller().can_manage() {
        return error_response(ObjectError::PermissionDenied);
    }
    match state.workspace_roles(caller.tenant_id).await {
        Ok(roles) if roles.contains_key(&role_id) => {}
        Ok(_) => return error_response(ObjectError::NotFound),
        Err(response) => return response,
    }
    match service
        .set_object_permission(&object, role_id, access, context(&request_id))
        .await
    {
        Ok(permission) => Json(json!({"role_id": permission.role_id, "access": permission.access}))
            .into_response(),
        Err(error) => error_response(error),
    }
}

// --- Record routes ---------------------------------------------------------

/// Parses `status`, `sort`, `direction`, `limit`, `cursor`, `search`,
/// `owner=me` and `filter[<field>]=<value>` from the raw query string;
/// `None` means the query is invalid.
fn list_request(raw: Option<&str>, me: Uuid) -> Option<ListRecords> {
    let mut request = ListRecords::default();
    let pairs: Vec<(String, String)> = match raw {
        Some(raw) => serde_urlencoded_pairs(raw)?,
        None => Vec::new(),
    };
    for (key, value) in pairs {
        match key.as_str() {
            "status" => {
                request.status = match value.as_str() {
                    "active" => RecordStatus::Active,
                    "deleted" => RecordStatus::Deleted,
                    "any" => RecordStatus::Any,
                    _ => return None,
                }
            }
            "sort" => {
                request.sort = match value.as_str() {
                    "name" => RecordSort::Name,
                    "created_at" => RecordSort::CreatedAt,
                    "updated_at" => RecordSort::UpdatedAt,
                    "touched_at" => RecordSort::TouchedAt,
                    _ => return None,
                }
            }
            "direction" => {
                request.direction = match value.as_str() {
                    "asc" => SortDirection::Asc,
                    "desc" => SortDirection::Desc,
                    _ => return None,
                }
            }
            "limit" => request.limit = Some(value.parse().ok()?),
            "cursor" => request.cursor = Some(value),
            "search" => request.search = Some(value),
            "touched_after" => {
                request.touched_after = Some(
                    chrono::DateTime::parse_from_rfc3339(&value)
                        .ok()?
                        .with_timezone(&chrono::Utc),
                )
            }
            "owner" if value == "me" => request.owner = Some(me),
            other => match other
                .strip_prefix("filter[")
                .and_then(|rest| rest.strip_suffix(']'))
            {
                Some(field) if request.filters.len() < 10 => {
                    request.filters.push((field.to_owned(), value));
                }
                _ => return None,
            },
        }
    }
    Some(request)
}

/// Decodes `application/x-www-form-urlencoded` pairs without a new
/// dependency: `+` is a space and `%XX` an escaped byte.
fn serde_urlencoded_pairs(raw: &str) -> Option<Vec<(String, String)>> {
    fn decode(part: &str) -> Option<String> {
        let mut bytes = Vec::with_capacity(part.len());
        let mut input = part.bytes();
        while let Some(byte) = input.next() {
            match byte {
                b'+' => bytes.push(b' '),
                b'%' => {
                    let high = (input.next()? as char).to_digit(16)?;
                    let low = (input.next()? as char).to_digit(16)?;
                    bytes.push(u8::try_from(high * 16 + low).ok()?);
                }
                other => bytes.push(other),
            }
        }
        String::from_utf8(bytes).ok()
    }
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Some((decode(key)?, decode(value)?))
        })
        .collect()
}

pub async fn list_records<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    RawQuery(raw): RawQuery,
) -> Response {
    let Some(request) = list_request(raw.as_deref(), caller.user_id) else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.list_records(&object, request).await {
        Ok(page) => {
            let value = json!({
                "data": records_json(&service, &object, &page.records).await,
                "next_cursor": page.next_cursor,
            });
            respond_records(&state, &caller, StatusCode::OK, value).await
        }
        Err(error) => error_response(error),
    }
}

fn record_body(
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Option<Map<String, Value>> {
    match body {
        Ok(Json(Value::Object(map))) => Some(map),
        _ => None,
    }
}

pub async fn create_record<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object)): Path<(Uuid, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(body) = record_body(body) else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .create_record(&object, &body, context(&request_id))
        .await
    {
        Ok(record) => {
            respond_records(
                &state,
                &caller,
                StatusCode::CREATED,
                one_record_json(&service, &object, &record).await,
            )
            .await
        }
        Err(error) => error_response(error),
    }
}

#[derive(Deserialize, Default)]
pub(crate) struct ReadQuery {
    #[serde(default)]
    include_deleted: bool,
}

pub async fn get_record<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    read: Result<Query<ReadQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(read)) = read else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .get_record_with_rights(&object, &record, read.include_deleted)
        .await
    {
        Ok((found, rights)) => {
            let mut value = one_record_json(&service, &object, &found).await;
            // What the caller may do with this record (ADR 0016), so pages
            // offer only the actions that will succeed.
            if let Some(map) = value.as_object_mut() {
                map.insert("access".into(), json!(rights));
            }
            respond_records(&state, &caller, StatusCode::OK, value).await
        }
        Err(error) => error_response(error),
    }
}

// --- Change data capture (ADR 0018) ---------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SearchParams {
    q: String,
    #[serde(default)]
    object: Option<String>,
    #[serde(default)]
    limit: Option<u16>,
}

/// `GET /search`: global search across searchable objects (ADR 0020).
pub async fn search<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
    params: Result<Query<SearchParams>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    let request = platform_objects::SearchRequest {
        q: params.q,
        object: params.object,
        limit: params.limit,
    };
    match service.search(request).await {
        Ok(groups) => Json(json!({
            "data": groups
                .iter()
                .map(|group| json!({
                    "object": group.object,
                    "label": group.label,
                    "plural_label": group.plural_label,
                    "records": group
                        .records
                        .iter()
                        .map(|record| json!({
                            "id": record.public_id,
                            "name": record.name,
                            "fields": record.fields,
                        }))
                        .collect::<Vec<_>>(),
                    "has_more": group.has_more,
                }))
                .collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryParams {
    q: String,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    page_size: Option<u16>,
    #[serde(default)]
    include_deleted: bool,
}

/// `GET /query`: runs a record query as the caller (ADR 0019).
pub async fn run_query<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
    params: Result<Query<QueryParams>, QueryRejection>,
) -> Response {
    let Ok(Query(params)) = params else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    let request = platform_objects::QueryRequest {
        q: params.q,
        cursor: params.cursor,
        page_size: params.page_size,
        include_deleted: params.include_deleted,
    };
    match service.query(request).await {
        Ok(result) => Json(json!({
            "object": result.object,
            "columns": result.columns,
            "data": result.rows,
            "next_cursor": result.next_cursor,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChangesQuery {
    #[serde(default)]
    after: Option<i64>,
    #[serde(default)]
    limit: Option<u16>,
    #[serde(default)]
    object: Option<String>,
}

fn change_json(object: &str, change: &platform_objects::RecordChange) -> Value {
    json!({
        "position": change.position,
        "object": object,
        "record": change.public_id,
        "kind": change.kind,
        "fields": change.fields,
        "changed_at": change.changed_at,
        "changed_by": change.changed_by,
        "correlation_id": change.correlation_id,
    })
}

/// `GET /objects/changes`: one page of the workspace's change log after a
/// position, with only changes to records the caller can see.
pub async fn list_changes<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<ChangesQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .list_changes(query.after, query.limit, query.object.as_deref())
        .await
    {
        Ok(page) => Json(json!({
            "data": page.changes.iter().map(|(object, change)| change_json(object, change)).collect::<Vec<_>>(),
            "next_after": page.next_after,
            "head": page.head,
            "metadata_version": page.metadata_version,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

/// How often an idle stream looks for new changes.
const STREAM_POLL: std::time::Duration = std::time::Duration::from_secs(1);
/// A stream ends after this long; the client reconnects with
/// `Last-Event-ID`, so its session and membership are checked again.
const STREAM_LIFETIME: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// `GET /objects/changes/stream`: server-sent events for the change log,
/// resuming after `after` or the `Last-Event-ID` header. Each event's id is
/// its position; a position that has expired ends the stream with an
/// `expired` event. Visibility is checked per change with the caller's
/// current access, which the stream re-resolves on every poll.
pub async fn stream_changes<H: ObjectsHost>(
    State(state): State<H>,
    Path(_workspace): Path<Uuid>,
    Extension(caller): Extension<Caller>,
    headers: axum::http::HeaderMap,
    query: Result<Query<ChangesQuery>, QueryRejection>,
) -> Response {
    use axum::response::sse::{Event, KeepAlive, Sse};
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let resume = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i64>().ok());
    let after = resume.or(query.after);
    // Validate the starting point before committing to a stream.
    let first = match service(&state, &caller).await {
        Ok(service) => match service
            .list_changes(after, Some(1), query.object.as_deref())
            .await
        {
            Ok(_) => service,
            Err(error) => return error_response(error),
        },
        Err(response) => return response,
    };
    drop(first);
    struct Cursor<H> {
        state: H,
        caller: Caller,
        object: Option<String>,
        after: Option<i64>,
        /// The metadata version last sent; a `metadata` event announces
        /// each move, starting with the first page (ADR 0024 §3).
        metadata: Option<i64>,
        pending: std::collections::VecDeque<Event>,
        done: bool,
        deadline: tokio::time::Instant,
    }
    let cursor = Cursor {
        state,
        caller,
        object: query.object,
        after,
        metadata: None,
        pending: std::collections::VecDeque::new(),
        done: false,
        deadline: tokio::time::Instant::now() + STREAM_LIFETIME,
    };
    let events = futures_util::stream::unfold(cursor, |mut cursor| async move {
        loop {
            if let Some(event) = cursor.pending.pop_front() {
                return Some((Ok::<Event, std::convert::Infallible>(event), cursor));
            }
            if cursor.done || tokio::time::Instant::now() >= cursor.deadline {
                return None;
            }
            let page = match service(&cursor.state, &cursor.caller).await {
                Ok(service) => {
                    service
                        .list_changes(cursor.after, Some(200), cursor.object.as_deref())
                        .await
                }
                Err(_) => {
                    tokio::time::sleep(STREAM_POLL).await;
                    continue;
                }
            };
            match page {
                Ok(page) => {
                    let moved = Some(page.next_after) != cursor.after;
                    // Definitions first: a device refreshes them before it
                    // applies records made under them.
                    if cursor.metadata != Some(page.metadata_version) {
                        cursor.metadata = Some(page.metadata_version);
                        cursor.pending.push_back(
                            Event::default().event("metadata").data(
                                json!({"metadata_version": page.metadata_version}).to_string(),
                            ),
                        );
                    }
                    for (object, change) in &page.changes {
                        cursor.pending.push_back(
                            Event::default()
                                .id(change.position.to_string())
                                .event("change")
                                .data(change_json(object, change).to_string()),
                        );
                    }
                    cursor.after = Some(page.next_after);
                    if !moved {
                        tokio::time::sleep(STREAM_POLL).await;
                    }
                }
                Err(ObjectError::Conflict("changes.position_expired")) => {
                    cursor.done = true;
                    cursor.pending.push_back(
                        Event::default()
                            .event("expired")
                            .data(json!({"code": "changes.position_expired"}).to_string()),
                    );
                }
                Err(ObjectError::NotFound | ObjectError::PermissionDenied) => {
                    // Access was withdrawn while streaming.
                    cursor.done = true;
                }
                Err(_) => tokio::time::sleep(STREAM_POLL).await,
            }
        }
    });
    Sse::new(events)
        .keep_alive(KeepAlive::default())
        .into_response()
}

// --- History (ADR 0017) ---------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HistoryQuery {
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    limit: Option<u16>,
}

fn history_json(entry: &platform_objects::HistoryEntry) -> Value {
    let mut value = json!({
        "id": entry.id,
        "kind": entry.kind,
        "field": entry.field,
        "old_value": entry.old_value,
        "new_value": entry.new_value,
        "changed_at": entry.changed_at,
        "changed_by": crate::user_reference(entry.changed_by),
    });
    // Owner changes also carry member references, named like `owner`.
    if entry.field.as_deref() == Some("owner") {
        let member = |value: &Option<Value>| {
            crate::user_reference(
                value
                    .as_ref()
                    .and_then(Value::as_str)
                    .and_then(|id| Uuid::parse_str(id).ok()),
            )
        };
        if let Some(map) = value.as_object_mut() {
            map.insert("old_user".into(), member(&entry.old_value));
            map.insert("new_user".into(), member(&entry.new_value));
        }
    }
    value
}

/// `GET .../records/{id}/history`: tracked changes, newest first.
pub async fn record_history<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<HistoryQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .record_history(&object, &record, query.cursor.as_deref(), query.limit)
        .await
    {
        Ok((page, names)) => {
            let value = json!({
                "data": page.entries.iter().map(history_json).collect::<Vec<_>>(),
                "related": names,
                "next_cursor": page.next_cursor,
            });
            respond_records(&state, &caller, StatusCode::OK, value).await
        }
        Err(error) => error_response(error),
    }
}

// --- Related lists and lookup candidates (ADR 0028) -------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PageQuery {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CandidatesQuery {
    #[serde(default)]
    search: Option<String>,
    #[serde(default)]
    cursor: Option<String>,
}

fn related_json(list: &platform_objects::RelatedList) -> Value {
    json!({
        "object": list.relationship.object,
        "object_label": list.relationship.object_label,
        "object_plural_label": list.relationship.object_plural_label,
        "field": list.relationship.field,
        "field_label": list.relationship.field_label,
        "master_detail": list.relationship.master_detail,
        "data": list.records.rows,
        "next_cursor": list.records.next_cursor,
    })
}

/// `GET .../records/{id}/related`: the first records of each related list.
pub async fn related_lists<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service.related_lists(&object, &record).await {
        Ok(lists) => Json(json!({
            "data": lists.iter().map(related_json).collect::<Vec<_>>(),
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

/// `GET .../records/{id}/related/{object}/{field}`: one page of one list.
pub async fn related_records<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record, child, field)): Path<(Uuid, String, String, String, String)>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .related_records(&object, &record, &child, &field, query.cursor)
        .await
    {
        Ok(list) => Json(related_json(&list)).into_response(),
        Err(error) => error_response(error),
    }
}

/// `GET .../fields/{field}/candidates`: the records a lookup may point at,
/// within its lookup filter.
pub async fn lookup_candidates<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, field)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    query: Result<Query<CandidatesQuery>, QueryRejection>,
) -> Response {
    let Ok(Query(query)) = query else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .lookup_candidates(&object, &field, query.search.as_deref(), query.cursor)
        .await
    {
        Ok(result) => Json(json!({
            "data": result.rows,
            "next_cursor": result.next_cursor,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

// --- Sharing and ownership (ADR 0016) -------------------------------------

fn share_json(share: &platform_objects::RecordShare, names: &BTreeMap<Uuid, String>) -> Value {
    json!({
        "grantee_kind": share.grantee_kind,
        "grantee_id": share.grantee_id,
        "grantee_name": names.get(&share.grantee_id),
        "access": share.access,
        "created_at": share.created_at,
        "created_by": crate::user_reference(share.created_by),
    })
}

/// The record's owner, the caller's rights and its manual shares, with
/// member and role names.
pub async fn record_sharing<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
) -> Response {
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    let (found, rights, shares) = match service.record_sharing(&object, &record).await {
        Ok(result) => result,
        Err(error) => return error_response(error),
    };
    let tenant_id = caller.tenant_id;
    let users: Vec<Uuid> = shares
        .iter()
        .filter(|share| share.grantee_kind == GranteeKind::User)
        .map(|share| share.grantee_id)
        .collect();
    let mut names = state.member_names(tenant_id, &users).await;
    if let Ok(roles) = state.workspace_roles(tenant_id).await {
        names.extend(roles);
    }
    let value = json!({
        "record": found.public_id,
        "owner": crate::user_reference(found.owner_id),
        "version": found.version,
        "access": rights,
        "shares": shares.iter().map(|share| share_json(share, &names)).collect::<Vec<_>>(),
    });
    respond_records(&state, &caller, StatusCode::OK, value).await
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ShareBody {
    access: ShareAccess,
}

/// Whether a grantee exists in the caller's workspace; unknown ones are
/// indistinguishable from foreign ones.
async fn grantee_exists<H: ObjectsHost>(
    state: &H,
    tenant_id: Uuid,
    kind: GranteeKind,
    grantee: Uuid,
) -> Result<bool, Response> {
    match kind {
        GranteeKind::User => state.is_member(tenant_id, grantee).await,
        GranteeKind::Role => state
            .workspace_roles(tenant_id)
            .await
            .map(|roles| roles.contains_key(&grantee)),
    }
}

fn grantee_path(kind: &str, grantee: &str) -> Option<(GranteeKind, Uuid)> {
    Some((GranteeKind::parse(kind)?, Uuid::parse_str(grantee).ok()?))
}

/// `PUT .../sharing/{user|role}/{id}` with `{"access": "read"|"edit"}`.
pub async fn put_share<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record, kind, grantee)): Path<(Uuid, String, String, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<ShareBody>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some((kind, grantee)) = grantee_path(&kind, &grantee) else {
        return error_response(ObjectError::NotFound);
    };
    let Ok(Json(body)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match grantee_exists(&state, caller.tenant_id, kind, grantee).await {
        Ok(true) => {}
        Ok(false) => {
            return error_response(ObjectError::invalid(
                "grantee_id",
                "records.grantee_unknown",
                "choose a member or role of this workspace",
            ))
        }
        Err(response) => return response,
    }
    match service
        .share_record(
            &object,
            &record,
            kind,
            grantee,
            body.access,
            context(&request_id),
        )
        .await
    {
        Ok(share) => Json(share_json(&share, &BTreeMap::new())).into_response(),
        Err(error) => error_response(error),
    }
}

pub async fn delete_share<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record, kind, grantee)): Path<(Uuid, String, String, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
) -> Response {
    let Some((kind, grantee)) = grantee_path(&kind, &grantee) else {
        return error_response(ObjectError::NotFound);
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .unshare_record(&object, &record, kind, grantee, context(&request_id))
        .await
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => error_response(error),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnerChange {
    owner_id: Uuid,
    expected_version: i64,
}

/// `POST .../records/{id}/owner`: transfers the record to another member.
pub async fn change_owner<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<OwnerChange>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(change)) = body else {
        return invalid_body();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match grantee_exists(&state, caller.tenant_id, GranteeKind::User, change.owner_id).await {
        Ok(true) => {}
        Ok(false) => {
            return error_response(ObjectError::invalid(
                "owner_id",
                "records.owner_unknown",
                "choose a member of this workspace",
            ))
        }
        Err(response) => return response,
    }
    match service
        .change_owner(
            &object,
            &record,
            change.owner_id,
            change.expected_version,
            context(&request_id),
        )
        .await
    {
        Ok(moved) => {
            let value = one_record_json(&service, &object, &moved).await;
            respond_records(&state, &caller, StatusCode::OK, value).await
        }
        Err(error) => error_response(error),
    }
}

/// `PATCH`: `expected_version` plus the values to change; `null` clears.
pub async fn update_record<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    body: Result<Json<Value>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Some(mut body) = record_body(body) else {
        return invalid_body();
    };
    let Some(expected_version) = body
        .remove("expected_version")
        .and_then(|value| value.as_i64())
    else {
        return problem(
            StatusCode::BAD_REQUEST,
            "validation.failed",
            "one or more values are invalid",
            vec![ValidationIssue::new(
                "expected_version",
                "records.expected_version_required",
                "send the version you read",
            )],
        );
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .update_record(
            &object,
            &record,
            expected_version,
            &body,
            context(&request_id),
        )
        .await
    {
        Ok(updated) => {
            respond_records(
                &state,
                &caller,
                StatusCode::OK,
                one_record_json(&service, &object, &updated).await,
            )
            .await
        }
        Err(error) => error_response(error),
    }
}

pub async fn delete_record<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .delete_record(
            &object,
            &record,
            version.expected_version,
            context(&request_id),
        )
        .await
    {
        Ok(deleted) => {
            respond_records(
                &state,
                &caller,
                StatusCode::OK,
                one_record_json(&service, &object, &deleted).await,
            )
            .await
        }
        Err(error) => error_response(error),
    }
}

pub async fn undelete_record<H: ObjectsHost>(
    State(state): State<H>,
    Path((_workspace, object, record)): Path<(Uuid, String, String)>,
    Extension(caller): Extension<Caller>,
    Extension(request_id): Extension<RequestId>,
    version: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Ok(Query(version)) = version else {
        return invalid_query();
    };
    let service = match service(&state, &caller).await {
        Ok(service) => service,
        Err(response) => return response,
    };
    match service
        .undelete_record(
            &object,
            &record,
            version.expected_version,
            context(&request_id),
        )
        .await
    {
        Ok(restored) => {
            respond_records(
                &state,
                &caller,
                StatusCode::OK,
                one_record_json(&service, &object, &restored).await,
            )
            .await
        }
        Err(error) => error_response(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_queries_parse_filters_and_reject_unknown_keys() {
        let me = Uuid::from_u128(9);
        let request = list_request(
            Some("status=deleted&sort=updated_at&direction=desc&limit=5&search=apollo+one&owner=me&filter%5Bcustom%3Astatus%5D=Won"),
            me,
        )
        .expect("valid");
        assert_eq!(request.status, RecordStatus::Deleted);
        assert_eq!(request.sort, RecordSort::UpdatedAt);
        assert_eq!(request.direction, SortDirection::Desc);
        assert_eq!(request.limit, Some(5));
        assert_eq!(request.search.as_deref(), Some("apollo one"));
        assert_eq!(request.owner, Some(me));
        assert_eq!(
            request.filters,
            vec![("custom:status".to_owned(), "Won".to_owned())]
        );
        let sync = list_request(
            Some("status=any&sort=touched_at&touched_after=2026-10-11T20%3A04%3A19.26Z"),
            me,
        )
        .expect("sync query");
        assert_eq!(sync.status, RecordStatus::Any);
        assert_eq!(sync.sort, RecordSort::TouchedAt);
        assert_eq!(
            sync.touched_after.map(|time| time.to_rfc3339()),
            Some("2026-10-11T20:04:19.260+00:00".to_owned())
        );
        for bad in [
            "touched_after=yesterday",
            "nope=1",
            "status=archived",
            "limit=x",
            "owner=someone",
            "filter[x",
        ] {
            assert!(list_request(Some(bad), me).is_none(), "{bad}");
        }
    }

    #[test]
    fn records_serialise_standard_fields_then_custom_values() {
        let now = chrono::Utc::now();
        let record = Record {
            id: Uuid::nil(),
            object_id: Uuid::nil(),
            public_id: "000000000000001abc".into(),
            name: "Apollo".into(),
            owner_id: Some(Uuid::from_u128(1)),
            record_type_id: None,
            values: [("custom:status".to_owned(), json!("Won"))]
                .into_iter()
                .collect(),
            version: 2,
            created_at: now,
            created_by: Some(Uuid::from_u128(1)),
            updated_at: now,
            updated_by: Some(Uuid::from_u128(2)),
            touched_at: now,
            archived_at: None,
            deleted_at: None,
        };
        let value = record_json("custom:project", &record, &BTreeMap::new());
        assert_eq!(value["object"], "custom:project");
        assert_eq!(value["owner"]["id"], json!(Uuid::from_u128(1)));
        assert_eq!(value["custom:status"], "Won");
        assert_eq!(value["is_deleted"], false);
        assert!(
            value.get("data").is_none(),
            "custom values are flat, not nested"
        );
    }
}
