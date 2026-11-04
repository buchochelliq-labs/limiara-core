use crate::extensions::{catalog, CallerAccess};
use crate::users::user_reference;
use crate::{auth::WorkspacePrincipal, AppState};
use axum::{
    extract::{rejection::QueryRejection, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use contacts_plugin::{
    ContactCustomFieldTarget, ContactsAuthorization, ContactsError, ContactsMutationContext,
    ContactsService, CreateCustomFieldDefinition, CreateOrganisation, CreatePerson, ListRequest,
    Organisation, Page, Person, UpdateCustomFieldDefinition, UpdateOrganisation, UpdatePerson,
    ValidationIssue, ORGANISATION_RESOURCE, PERSON_RESOURCE,
};
use platform_extensions::{ExtensionCatalog, ExtensionError, ExtensionValues};
use serde::{Deserialize, Serialize};
use serde_json::json;
use serde_json::Value;
use std::collections::BTreeMap;
use tower_http::request_id::RequestId;
use uuid::Uuid;

#[derive(Serialize)]
struct ContactsProblem {
    #[serde(rename = "type")]
    kind: &'static str,
    title: &'static str,
    status: u16,
    code: &'static str,
    detail: &'static str,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    errors: Vec<ValidationIssue>,
}

#[derive(Deserialize)]
pub(crate) struct ExpectedVersion {
    expected_version: i64,
}

/// `?include_deleted=true` reads a record in the recycle bin, as Salesforce's
/// `queryAll` does (ADR 0014).
#[derive(Deserialize, Default)]
pub(crate) struct ReadQuery {
    #[serde(default)]
    include_deleted: bool,
}

/// Optional equality filter on one extension field of a list.
#[derive(Deserialize)]
pub(crate) struct ExtensionFilterQuery {
    extension_field: Option<String>,
    extension_value: Option<String>,
}

/// A write to extension values on one record.
#[derive(Deserialize)]
pub(crate) struct ExtensionValuesInput {
    expected_version: i64,
    values: BTreeMap<String, Value>,
}

/// What a record response needs beyond the record's own serialisation: its
/// stored extension values and the standard fields the model keeps private.
struct StandardValues<'a> {
    extensions: &'a ExtensionValues,
    name: &'a str,
    created_by: Option<Uuid>,
    updated_by: Option<Uuid>,
    deleted: bool,
}

impl<'a> StandardValues<'a> {
    fn person(person: &'a Person) -> Self {
        Self {
            extensions: &person.extensions,
            name: &person.full_name,
            created_by: person.created_by,
            updated_by: person.updated_by,
            deleted: person.deleted_at.is_some(),
        }
    }

    fn organisation(organisation: &'a Organisation) -> Self {
        Self {
            extensions: &organisation.extensions,
            name: &organisation.name,
            created_by: organisation.created_by,
            updated_by: organisation.updated_by,
            deleted: organisation.deleted_at.is_some(),
        }
    }
}

/// Fills in member names for the standard `created_by` and `updated_by`
/// fields of one record or a page of records (ADR 0014). Users who are no
/// longer members keep their id and no name; lookups never leave the
/// caller's workspace.
async fn records(
    state: &AppState,
    principal: &WorkspacePrincipal,
    status: StatusCode,
    value: Result<Value, ContactsError>,
) -> Response {
    let mut value = match value {
        Ok(value) => value,
        Err(error) => return error_response(error),
    };
    crate::users::name_record_users(state, principal.workspace.tenant_id, &mut value).await;
    (status, Json(value)).into_response()
}

/// Serialises Contacts records with only the extension values the caller may
/// read from active extensions, as projected by the kernel service.
struct RecordView<'a> {
    catalog: ExtensionCatalog,
    access: CallerAccess<'a>,
}

impl<'a> RecordView<'a> {
    fn new(principal: &'a WorkspacePrincipal) -> Self {
        Self::for_activation(&principal.activation)
    }

    fn for_activation(activation: &'a crate::plugins::PluginActivationProjection) -> Self {
        Self {
            catalog: catalog(activation),
            access: CallerAccess(activation),
        }
    }

    fn record<T: Serialize>(
        &self,
        target: &str,
        record: &T,
        standard: StandardValues<'_>,
    ) -> Result<Value, ContactsError> {
        let mut value = serde_json::to_value(record).map_err(|_| ContactsError::Invariant)?;
        let projected = self
            .catalog
            .project(target, standard.extensions, &self.access);
        let object = value.as_object_mut().ok_or(ContactsError::Invariant)?;
        object.insert(
            "extensions".into(),
            serde_json::to_value(projected).map_err(|_| ContactsError::Invariant)?,
        );
        // Standard record fields (ADR 0014). Members carry only their id
        // here; `records` fills in names from workspace membership.
        object.insert("name".into(), json!(standard.name));
        object.insert("is_deleted".into(), json!(standard.deleted));
        object.insert("created_by".into(), user_reference(standard.created_by));
        object.insert("updated_by".into(), user_reference(standard.updated_by));
        Ok(value)
    }

    fn person(&self, person: Result<Person, ContactsError>) -> Result<Value, ContactsError> {
        let person = person?;
        self.record(PERSON_RESOURCE, &person, StandardValues::person(&person))
    }

    fn organisation(
        &self,
        organisation: Result<Organisation, ContactsError>,
    ) -> Result<Value, ContactsError> {
        let organisation = organisation?;
        self.record(
            ORGANISATION_RESOURCE,
            &organisation,
            StandardValues::organisation(&organisation),
        )
    }

    fn people(&self, page: Result<Page<Person>, ContactsError>) -> Result<Value, ContactsError> {
        let page = page?;
        let data = page
            .data
            .iter()
            .map(|person| self.record(PERSON_RESOURCE, person, StandardValues::person(person)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({ "data": data, "next_cursor": page.next_cursor }))
    }

    fn organisations(
        &self,
        page: Result<Page<Organisation>, ContactsError>,
    ) -> Result<Value, ContactsError> {
        let page = page?;
        let data = page
            .data
            .iter()
            .map(|organisation| {
                self.record(
                    ORGANISATION_RESOURCE,
                    organisation,
                    StandardValues::organisation(organisation),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({ "data": data, "next_cursor": page.next_cursor }))
    }

    /// Splits a record body into the host's input and the kernel-validated
    /// patch for its optional `extensions` map, so a form saves host fields
    /// and extension fields in one request and one transaction.
    fn split_body<T: serde::de::DeserializeOwned>(
        &self,
        target: &str,
        mut body: Value,
    ) -> Result<(T, Option<platform_extensions::ValidatedExtensionPatch>), ContactsError> {
        let extensions = body
            .as_object_mut()
            .and_then(|object| object.remove("extensions"));
        let input = serde_json::from_value::<T>(body).map_err(|_| {
            ContactsError::Validation(vec![ValidationIssue {
                field: "body".into(),
                code: "request.invalid".into(),
                detail: "the request body is invalid".into(),
            }])
        })?;
        let patch = match extensions {
            None | Some(Value::Null) => None,
            Some(Value::Object(values)) if values.is_empty() => None,
            Some(Value::Object(values)) => Some(
                self.catalog
                    .validate_patch(target, &values.into_iter().collect(), &self.access)
                    .map_err(extension_error)?,
            ),
            Some(_) => {
                return Err(ContactsError::Validation(vec![ValidationIssue {
                    field: "extensions".into(),
                    code: "extension.values_invalid".into(),
                    detail: "extensions must be an object of field values".into(),
                }]))
            }
        };
        Ok((input, patch))
    }

    /// Adds a kernel-validated extension filter to a list request.
    fn filter(
        &self,
        target: &str,
        request: &mut ListRequest,
        query: Result<Query<ExtensionFilterQuery>, QueryRejection>,
    ) -> Result<(), ContactsError> {
        let query = query_value(query)?;
        match (query.extension_field, query.extension_value) {
            (None, None) => Ok(()),
            (Some(field), Some(value)) => {
                request.extension_filter = Some(
                    self.catalog
                        .validate_filter(target, &field, &value, &self.access)
                        .map_err(extension_error)?,
                );
                Ok(())
            }
            _ => Err(ContactsError::Validation(vec![ValidationIssue {
                field: "extension_field".into(),
                code: "extension.filter_incomplete".into(),
                detail: "extension_field and extension_value must be supplied together".into(),
            }])),
        }
    }
}

fn extension_error(error: ExtensionError) -> ContactsError {
    match error {
        ExtensionError::Invalid(issues) => ContactsError::Validation(
            issues
                .into_iter()
                .map(|issue| ValidationIssue {
                    field: issue.field,
                    code: issue.code.into(),
                    detail: issue.detail.into(),
                })
                .collect(),
        ),
        ExtensionError::PermissionDenied => ContactsError::PermissionDenied,
        ExtensionError::Invariant => ContactsError::Invariant,
    }
}

pub(crate) async fn list_people(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    request: Result<Query<ListRequest>, QueryRejection>,
    extension_filter: Result<Query<ExtensionFilterQuery>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let mut request = match query_value(request) {
        Ok(request) => request,
        Err(error) => return error_response(error),
    };
    let view = RecordView::new(&principal);
    if let Err(error) = view.filter(PERSON_RESOURCE, &mut request, extension_filter) {
        return error_response(error);
    }
    records(
        &state,
        &principal,
        StatusCode::OK,
        view.people(service.list_people(request).await),
    )
    .await
}

pub(crate) async fn create_person(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(body): Json<Value>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let view = RecordView::new(&principal);
    let (input, patch) = match view.split_body::<CreatePerson>(PERSON_RESOURCE, body) {
        Ok(parts) => parts,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::CREATED,
        view.person(
            service
                .create_person_with_extensions(input, patch.as_ref(), mutation_context(&request_id))
                .await,
        ),
    )
    .await
}

pub(crate) async fn get_person(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    read: Result<Query<ReadQuery>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let read = match query_value(read) {
        Ok(read) => read,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal)
            .person(service.read_person(&public_id, read.include_deleted).await),
    )
    .await
}

pub(crate) async fn person_activity(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(
        StatusCode::OK,
        service
            .person_activity(&public_id)
            .await
            .map(|data| serde_json::json!({ "data": data })),
    )
}

pub(crate) async fn update_person(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(body): Json<Value>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let view = RecordView::new(&principal);
    let (input, patch) = match view.split_body::<UpdatePerson>(PERSON_RESOURCE, body) {
        Ok(parts) => parts,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        view.person(
            service
                .update_person_with_extensions(
                    &public_id,
                    input,
                    patch.as_ref(),
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

pub(crate) async fn archive_person(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let input = match query_value(input) {
        Ok(input) => input,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).person(
            service
                .archive_person(
                    &public_id,
                    input.expected_version,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

/// Moves the person to the recycle bin (ADR 0014).
pub(crate) async fn delete_person(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let input = match query_value(input) {
        Ok(input) => input,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).person(
            service
                .delete_person(
                    &public_id,
                    input.expected_version,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

/// Restores the person from the recycle bin.
pub(crate) async fn undelete_person(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let input = match query_value(input) {
        Ok(input) => input,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).person(
            service
                .undelete_person(
                    &public_id,
                    input.expected_version,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

pub(crate) async fn list_organisations(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    request: Result<Query<ListRequest>, QueryRejection>,
    extension_filter: Result<Query<ExtensionFilterQuery>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let mut request = match query_value(request) {
        Ok(request) => request,
        Err(error) => return error_response(error),
    };
    let view = RecordView::new(&principal);
    if let Err(error) = view.filter(ORGANISATION_RESOURCE, &mut request, extension_filter) {
        return error_response(error);
    }
    records(
        &state,
        &principal,
        StatusCode::OK,
        view.organisations(service.list_organisations(request).await),
    )
    .await
}

pub(crate) async fn create_organisation(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<CreateOrganisation>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    records(
        &state,
        &principal,
        StatusCode::CREATED,
        RecordView::new(&principal).organisation(
            service
                .create_organisation(input, mutation_context(&request_id))
                .await,
        ),
    )
    .await
}

pub(crate) async fn get_organisation(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    read: Result<Query<ReadQuery>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let read = match query_value(read) {
        Ok(read) => read,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).organisation(
            service
                .read_organisation(&public_id, read.include_deleted)
                .await,
        ),
    )
    .await
}

pub(crate) async fn organisation_activity(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(
        StatusCode::OK,
        service
            .organisation_activity(&public_id)
            .await
            .map(|data| serde_json::json!({ "data": data })),
    )
}

pub(crate) async fn update_organisation(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<UpdateOrganisation>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).organisation(
            service
                .update_organisation(&public_id, input, mutation_context(&request_id))
                .await,
        ),
    )
    .await
}

pub(crate) async fn archive_organisation(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let input = match query_value(input) {
        Ok(input) => input,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).organisation(
            service
                .archive_organisation(
                    &public_id,
                    input.expected_version,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

/// Moves the organisation to the recycle bin (ADR 0014).
pub(crate) async fn delete_organisation(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let input = match query_value(input) {
        Ok(input) => input,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).organisation(
            service
                .delete_organisation(
                    &public_id,
                    input.expected_version,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

/// Restores the organisation from the recycle bin.
pub(crate) async fn undelete_organisation(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    input: Result<Query<ExpectedVersion>, QueryRejection>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let input = match query_value(input) {
        Ok(input) => input,
        Err(error) => return error_response(error),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        RecordView::new(&principal).organisation(
            service
                .undelete_organisation(
                    &public_id,
                    input.expected_version,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

/// `PATCH .../contacts/people/{id}/extensions`: writes extension values the
/// kernel validates against the active declarations and the caller's
/// extension permissions.
pub(crate) async fn update_person_extensions(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ExtensionValuesInput>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let view = RecordView::new(&principal);
    let patch = match view
        .catalog
        .validate_patch(PERSON_RESOURCE, &input.values, &view.access)
    {
        Ok(patch) => patch,
        Err(error) => return error_response(extension_error(error)),
    };
    records(
        &state,
        &principal,
        StatusCode::OK,
        view.person(
            service
                .update_person_extensions(
                    &public_id,
                    input.expected_version,
                    &patch,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

/// `PATCH .../contacts/organisations/{id}/extensions`.
pub(crate) async fn update_organisation_extensions(
    State(state): State<AppState>,
    Path((_workspace_id, public_id)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ExtensionValuesInput>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let view = RecordView::new(&principal);
    let patch =
        match view
            .catalog
            .validate_patch(ORGANISATION_RESOURCE, &input.values, &view.access)
        {
            Ok(patch) => patch,
            Err(error) => return error_response(extension_error(error)),
        };
    records(
        &state,
        &principal,
        StatusCode::OK,
        view.organisation(
            service
                .update_organisation_extensions(
                    &public_id,
                    input.expected_version,
                    &patch,
                    mutation_context(&request_id),
                )
                .await,
        ),
    )
    .await
}

pub(crate) async fn list_definitions(
    State(state): State<AppState>,
    Path((_workspace_id, target)): Path<(Uuid, String)>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let target = match parse_target(&target) {
        Ok(target) => target,
        Err(error) => return error_response(error),
    };
    result(StatusCode::OK, service.list_definitions(target).await)
}

pub(crate) async fn create_definition(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<CreateCustomFieldDefinition>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(
        StatusCode::CREATED,
        service
            .create_definition(input, mutation_context(&request_id))
            .await,
    )
}

pub(crate) async fn get_definition(
    State(state): State<AppState>,
    Path((_workspace_id, id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(StatusCode::OK, service.get_definition(id).await)
}

pub(crate) async fn update_definition(
    State(state): State<AppState>,
    Path((_workspace_id, id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<UpdateCustomFieldDefinition>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(
        StatusCode::OK,
        service
            .update_definition(id, input, mutation_context(&request_id))
            .await,
    )
}

pub(crate) async fn disable_definition(
    State(state): State<AppState>,
    Path((_workspace_id, id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ExpectedVersion>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(
        StatusCode::OK,
        service
            .disable_definition(id, input.expected_version, mutation_context(&request_id))
            .await,
    )
}

pub(crate) async fn archive_definition(
    State(state): State<AppState>,
    Path((_workspace_id, id)): Path<(Uuid, Uuid)>,
    Extension(principal): Extension<WorkspacePrincipal>,
    Extension(request_id): Extension<RequestId>,
    Json(input): Json<ExpectedVersion>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    result(
        StatusCode::OK,
        service
            .archive_definition(id, input.expected_version, mutation_context(&request_id))
            .await,
    )
}

pub(crate) async fn odata_people(
    State(state): State<AppState>,
    Path(_workspace_id): Path<Uuid>,
    Extension(principal): Extension<WorkspacePrincipal>,
) -> Response {
    let Some(service) = service(&state, &principal).await else {
        return plugin_disabled();
    };
    let workspace_id = principal.workspace.tenant_id;
    match service.list_people(ListRequest::default()).await {
        Ok(page) => Json(json!({
            "@odata.context": format!("/api/v1/workspaces/{workspace_id}/odata/$metadata#People"),
            "value": page.data,
        }))
        .into_response(),
        Err(error) => error_response(error),
    }
}

async fn service(state: &AppState, principal: &WorkspacePrincipal) -> Option<ContactsService> {
    let workspace_id = principal.workspace.tenant_id;
    let permissions = principal
        .activation
        .permissions_for("platform.contacts")?
        .clone();
    let authorization =
        ContactsAuthorization::new(workspace_id, principal.session.user.id, permissions);
    state
        .pool
        .clone()
        .map(|pool| ContactsService::new(pool, authorization))
}

fn mutation_context(request_id: &RequestId) -> ContactsMutationContext {
    ContactsMutationContext::new(crate::correlation_id(request_id))
}

fn parse_target(value: &str) -> Result<ContactCustomFieldTarget, ContactsError> {
    match value {
        "person" => Ok(ContactCustomFieldTarget::Person),
        "organisation" => Ok(ContactCustomFieldTarget::Organisation),
        _ => Err(ContactsError::Validation(vec![ValidationIssue {
            field: "target".into(),
            code: "custom_field.target_invalid".into(),
            detail: "target must be person or organisation".into(),
        }])),
    }
}

fn query_value<T>(query: Result<Query<T>, QueryRejection>) -> Result<T, ContactsError> {
    query.map(|Query(value)| value).map_err(|_| {
        ContactsError::Validation(vec![ValidationIssue {
            field: "query".into(),
            code: "query.invalid".into(),
            detail: "one or more query parameters are invalid".into(),
        }])
    })
}

fn result<T: Serialize>(status: StatusCode, value: Result<T, ContactsError>) -> Response {
    match value {
        Ok(value) => (status, Json(value)).into_response(),
        Err(error) => error_response(error),
    }
}

fn plugin_disabled() -> Response {
    problem(
        StatusCode::NOT_FOUND,
        "resource.unavailable",
        "the requested resource is unavailable",
        Vec::new(),
    )
}

fn error_response(error: ContactsError) -> Response {
    match error {
        ContactsError::Validation(errors) => problem(
            StatusCode::BAD_REQUEST,
            "validation.failed",
            "one or more values are invalid",
            errors,
        ),
        ContactsError::PermissionDenied => problem(
            StatusCode::FORBIDDEN,
            "auth.permission_denied",
            "permission denied",
            Vec::new(),
        ),
        ContactsError::NotFound => problem(
            StatusCode::NOT_FOUND,
            "contacts.record_not_found",
            "the contact record was not found",
            Vec::new(),
        ),
        ContactsError::VersionConflict => problem(
            StatusCode::CONFLICT,
            "contacts.version_conflict",
            "the record changed since it was read",
            Vec::new(),
        ),
        ContactsError::InvalidCursor => problem(
            StatusCode::BAD_REQUEST,
            "contacts.cursor_invalid",
            "the cursor is invalid for this workspace or query",
            Vec::new(),
        ),
        ContactsError::Database(error) => {
            tracing::error!(error = %error, "contacts database operation failed");
            unavailable()
        }
        ContactsError::Invariant => {
            tracing::error!("contacts data invariant failed");
            unavailable()
        }
    }
}

fn unavailable() -> Response {
    problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "contacts.unavailable",
        "contacts are temporarily unavailable",
        Vec::new(),
    )
}

fn problem(
    status: StatusCode,
    code: &'static str,
    detail: &'static str,
    errors: Vec<ValidationIssue>,
) -> Response {
    (
        status,
        Json(ContactsProblem {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensions::tests::projection;
    use crate::users::{name_users, referenced_users};

    fn person() -> Person {
        let now = chrono::Utc::now();
        Person {
            id: Uuid::nil(),
            public_id: "per_test".into(),
            tenant_id: Uuid::nil(),
            full_name: "Ada".into(),
            email: None,
            telephone: None,
            organisation_id: None,
            organisation_public_id: None,
            tags: Vec::new(),
            custom_fields: BTreeMap::new(),
            extensions: serde_json::from_value(json!({
                "example.loyalty": {"tier": "gold", "retired": "x"},
                "example.removed": {"secret": "hidden"}
            }))
            .expect("stored values"),
            version: 3,
            archived_at: None,
            created_at: now,
            updated_at: now,
            created_by: Some(Uuid::from_u128(1)),
            updated_by: Some(Uuid::from_u128(2)),
            touched_at: now,
            deleted_at: None,
        }
    }

    #[test]
    fn responses_carry_standard_record_fields() {
        let reader = projection(true, &["loyalty.read"]);
        let mut value = RecordView::for_activation(&reader)
            .person(Ok(person()))
            .expect("person");
        assert_eq!(value["name"], json!("Ada"));
        assert_eq!(value["is_deleted"], json!(false));
        assert_eq!(value["deleted_at"], json!(null));
        assert_eq!(value["version"], json!(3));
        assert!(value["touched_at"].is_string());
        assert_eq!(
            referenced_users(&value),
            vec![Uuid::from_u128(1), Uuid::from_u128(2)]
        );
        // Only members get a name; a former member keeps their id.
        name_users(
            &mut value,
            &BTreeMap::from([(Uuid::from_u128(1), "Grace".to_owned())]),
        );
        assert_eq!(
            value["created_by"],
            json!({"id": Uuid::from_u128(1), "name": "Grace"})
        );
        assert_eq!(
            value["updated_by"],
            json!({"id": Uuid::from_u128(2), "name": null})
        );
        let mut page = json!({"data": [value.clone(), value], "next_cursor": null});
        assert_eq!(referenced_users(&page).len(), 2);
        name_users(
            &mut page,
            &BTreeMap::from([(Uuid::from_u128(2), "Alan".to_owned())]),
        );
        assert_eq!(page["data"][1]["updated_by"]["name"], json!("Alan"));
    }

    #[test]
    fn responses_carry_only_projected_extension_values() {
        let reader = projection(true, &["loyalty.read"]);
        let value = RecordView::for_activation(&reader)
            .person(Ok(person()))
            .expect("projected person");
        assert_eq!(value["extensions"], json!({"example.loyalty.tier": "gold"}));
        assert_eq!(value["id"], json!("per_test"));

        let stranger = projection(true, &[]);
        let value = RecordView::for_activation(&stranger)
            .person(Ok(person()))
            .expect("projected person");
        assert_eq!(value["extensions"], json!({}));

        let disabled = projection(false, &["loyalty.read"]);
        let page = RecordView::for_activation(&disabled)
            .people(Ok(Page {
                data: vec![person()],
                next_cursor: None,
            }))
            .expect("projected page");
        assert_eq!(page["data"][0]["extensions"], json!({}));
    }

    #[test]
    fn list_filters_are_validated_and_complete() {
        let reader = projection(true, &["loyalty.read"]);
        let view = RecordView::for_activation(&reader);
        let mut request = ListRequest::default();
        view.filter(
            PERSON_RESOURCE,
            &mut request,
            Ok(Query(ExtensionFilterQuery {
                extension_field: Some("example.loyalty.tier".into()),
                extension_value: Some("gold".into()),
            })),
        )
        .expect("valid filter");
        assert!(request.extension_filter.is_some());

        for (field, value) in [
            (Some("example.loyalty.tier"), None),
            (Some("example.loyalty.tier"), Some("platinum")),
            (Some("example.loyalty.unknown"), Some("gold")),
        ] {
            let mut request = ListRequest::default();
            assert!(matches!(
                view.filter(
                    PERSON_RESOURCE,
                    &mut request,
                    Ok(Query(ExtensionFilterQuery {
                        extension_field: field.map(str::to_owned),
                        extension_value: value.map(str::to_owned),
                    })),
                ),
                Err(ContactsError::Validation(_))
            ));
        }
    }
}
