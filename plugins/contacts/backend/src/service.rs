use crate::authorization::{AuthorizedContacts, ContactsAuthorization, ContactsOperation};
use crate::events::{ContactsMutation, EvidenceContext};
use crate::model::*;
use crate::store::{Actor, ContactsStore, ContactsTx, PostgresContactsStore};
use crate::validation::{
    validate_custom_field_definition, validate_custom_field_definition_update,
    validate_custom_fields, validate_expected_version, validate_list_request,
    validate_organisation, validate_organisation_update, validate_person, validate_person_update,
};
use crate::{ContactsError, ORGANISATION_RESOURCE, PERSON_RESOURCE};
use platform_extensions::{ExtensionError, ValidatedExtensionPatch};
use platform_id::PublicId;
use serde_json::Value;
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone)]
pub struct ContactsService {
    store: Arc<dyn ContactsStore>,
    authorization: ContactsAuthorization,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContactsMutationContext {
    correlation_id: Uuid,
    causation_id: Option<Uuid>,
}

impl ContactsMutationContext {
    pub fn new(correlation_id: Uuid) -> Self {
        Self {
            correlation_id,
            causation_id: None,
        }
    }

    pub fn with_causation(correlation_id: Uuid, causation_id: Uuid) -> Self {
        Self {
            correlation_id,
            causation_id: Some(causation_id),
        }
    }
}

impl ContactsService {
    /// A service over the PostgreSQL store.
    pub fn new(pool: PgPool, authorization: ContactsAuthorization) -> Self {
        Self::with_store(Arc::new(PostgresContactsStore::new(pool)), authorization)
    }

    /// A service over any Contacts store (ADR 0013).
    pub fn with_store(store: Arc<dyn ContactsStore>, authorization: ContactsAuthorization) -> Self {
        Self {
            store,
            authorization,
        }
    }

    pub async fn create_person(
        &self,
        input: CreatePerson,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        self.create_person_with_extensions(input, None, context)
            .await
    }

    /// Creates a person and, in the same transaction, stores extension
    /// values the kernel extension service validated for this resource.
    pub async fn create_person_with_extensions(
        &self,
        input: CreatePerson,
        extensions: Option<&ValidatedExtensionPatch>,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        let authorized = self.authorize(ContactsOperation::CreatePerson)?;
        let tenant_id = authorized.tenant_id;
        validate_person(&input)?;
        let mut extension_values = platform_extensions::ExtensionValues::new();
        if let Some(patch) = extensions {
            require_patch_target(patch, PERSON_RESOURCE)?;
            patch
                .apply(&mut extension_values)
                .map_err(map_extension_error)?;
        }
        let mut tx = self.begin(tenant_id).await?;
        let definitions = tx
            .definitions(tenant_id, ContactCustomFieldTarget::Person)
            .await?;
        let custom_fields = validate_custom_fields(&input.custom_fields, &definitions, None)?;
        self.validate_unique_values(
            tx.as_mut(),
            tenant_id,
            ContactCustomFieldTarget::Person,
            &definitions,
            &custom_fields,
            None,
        )
        .await?;
        let organisation_id = self
            .resolve_organisation(tx.as_mut(), tenant_id, input.organisation_id.as_deref())
            .await?;
        let public_id = tx.next_public_id(tenant_id, "person", "per").await?;
        let person = tx
            .insert_person(
                tenant_id,
                &public_id,
                &input,
                organisation_id,
                &custom_fields,
                &extension_values,
            )
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::PersonCreated(&person),
        )
        .await?;
        tx.commit().await?;
        Ok(person)
    }

    pub async fn get_person(&self, public_id: &str) -> Result<Person, ContactsError> {
        self.read_person(public_id, false).await
    }

    /// A person, including one in the recycle bin when `include_deleted`.
    pub async fn read_person(
        &self,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Person, ContactsError> {
        let tenant_id = self.authorize(ContactsOperation::GetPerson)?.tenant_id;
        require_public_id(public_id, "per")?;
        let mut tx = self.begin(tenant_id).await?;
        let person = tx.get_person(tenant_id, public_id, include_deleted).await?;
        tx.commit().await?;
        Ok(person)
    }

    /// The most recent activity for a person, newest first. Unknown or
    /// foreign identifiers return the same not-found error as `get_person`.
    pub async fn person_activity(
        &self,
        public_id: &str,
    ) -> Result<Vec<ActivityEntry>, ContactsError> {
        let tenant_id = self.authorize(ContactsOperation::GetPerson)?.tenant_id;
        require_public_id(public_id, "per")?;
        let mut tx = self.begin(tenant_id).await?;
        tx.get_person(tenant_id, public_id, false).await?;
        let activity = tx.activity(tenant_id, "person", public_id).await?;
        tx.commit().await?;
        Ok(activity)
    }

    pub async fn update_person(
        &self,
        public_id: &str,
        input: UpdatePerson,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        self.update_person_with_extensions(public_id, input, None, context)
            .await
    }

    /// Updates a person and applies kernel-validated extension values in the
    /// same transaction and record version, so one form saves everything.
    pub async fn update_person_with_extensions(
        &self,
        public_id: &str,
        input: UpdatePerson,
        extensions: Option<&ValidatedExtensionPatch>,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        let authorized = self.authorize(ContactsOperation::UpdatePerson)?;
        if let Some(patch) = extensions {
            require_patch_target(patch, PERSON_RESOURCE)?;
        }
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "per")?;
        validate_person_update(&input)?;
        let mut tx = self.begin(tenant_id).await?;
        let current = tx.get_person(tenant_id, public_id, false).await?;
        let definitions = tx
            .definitions(tenant_id, ContactCustomFieldTarget::Person)
            .await?;
        let custom_fields = validate_custom_fields(
            &input.custom_fields,
            &definitions,
            Some(&current.custom_fields),
        )?;
        self.validate_unique_values(
            tx.as_mut(),
            tenant_id,
            ContactCustomFieldTarget::Person,
            &definitions,
            &custom_fields,
            Some(current.id),
        )
        .await?;
        let organisation_id = self
            .resolve_organisation(tx.as_mut(), tenant_id, input.organisation_id.as_deref())
            .await?;
        let mut extension_values = current.extensions.clone();
        if let Some(patch) = extensions {
            patch
                .apply(&mut extension_values)
                .map_err(map_extension_error)?;
        }
        let person = tx
            .update_person(
                tenant_id,
                public_id,
                &input,
                organisation_id,
                &custom_fields,
                &extension_values,
            )
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::PersonUpdated(&person),
        )
        .await?;
        tx.commit().await?;
        Ok(person)
    }

    pub async fn archive_person(
        &self,
        public_id: &str,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        let authorized = self.authorize(ContactsOperation::ArchivePerson)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "per")?;
        validate_expected_version(expected_version)?;
        let mut tx = self.begin(tenant_id).await?;
        let person = tx
            .archive_person(tenant_id, public_id, expected_version)
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::PersonArchived(&person),
        )
        .await?;
        tx.commit().await?;
        Ok(person)
    }

    /// Moves a person to the recycle bin (ADR 0014). It disappears from
    /// reads, lists and writes until it is undeleted or purged.
    pub async fn delete_person(
        &self,
        public_id: &str,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        self.set_person_deleted(public_id, expected_version, true, context)
            .await
    }

    /// Restores a person from the recycle bin.
    pub async fn undelete_person(
        &self,
        public_id: &str,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        self.set_person_deleted(public_id, expected_version, false, context)
            .await
    }

    async fn set_person_deleted(
        &self,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        let authorized = self.authorize(ContactsOperation::DeletePerson)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "per")?;
        validate_expected_version(expected_version)?;
        let mut tx = self.begin(tenant_id).await?;
        let person = tx
            .set_person_deleted(tenant_id, public_id, expected_version, deleted)
            .await?;
        let mutation = if deleted {
            ContactsMutation::PersonDeleted(&person)
        } else {
            ContactsMutation::PersonUndeleted(&person)
        };
        self.record_mutation(tx.as_mut(), authorized, context, mutation)
            .await?;
        tx.commit().await?;
        Ok(person)
    }

    pub async fn list_people(&self, request: ListRequest) -> Result<Page<Person>, ContactsError> {
        let tenant_id = self.authorize(ContactsOperation::ListPeople)?.tenant_id;
        validate_list_request(&request)?;
        if let Some(id) = request.organisation_id.as_deref() {
            require_public_id_input(id, "org", "organisation_id")?;
        }
        require_filter_target(&request, PERSON_RESOURCE)?;
        let mut tx = self.begin(tenant_id).await?;
        let page = tx.list_people(tenant_id, &request).await?;
        tx.commit().await?;
        Ok(page)
    }

    pub async fn create_organisation(
        &self,
        input: CreateOrganisation,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        let authorized = self.authorize(ContactsOperation::CreateOrganisation)?;
        let tenant_id = authorized.tenant_id;
        validate_organisation(&input)?;
        let mut tx = self.begin(tenant_id).await?;
        let definitions = tx
            .definitions(tenant_id, ContactCustomFieldTarget::Organisation)
            .await?;
        let custom_fields = validate_custom_fields(&input.custom_fields, &definitions, None)?;
        self.validate_unique_values(
            tx.as_mut(),
            tenant_id,
            ContactCustomFieldTarget::Organisation,
            &definitions,
            &custom_fields,
            None,
        )
        .await?;
        let public_id = tx.next_public_id(tenant_id, "organisation", "org").await?;
        let organisation = tx
            .insert_organisation(tenant_id, &public_id, &input, &custom_fields)
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::OrganisationCreated(&organisation),
        )
        .await?;
        tx.commit().await?;
        Ok(organisation)
    }

    pub async fn get_organisation(&self, public_id: &str) -> Result<Organisation, ContactsError> {
        self.read_organisation(public_id, false).await
    }

    /// An organisation, including one in the recycle bin when
    /// `include_deleted`.
    pub async fn read_organisation(
        &self,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Organisation, ContactsError> {
        let tenant_id = self
            .authorize(ContactsOperation::GetOrganisation)?
            .tenant_id;
        require_public_id(public_id, "org")?;
        let mut tx = self.begin(tenant_id).await?;
        let organisation = tx
            .get_organisation(tenant_id, public_id, include_deleted)
            .await?;
        tx.commit().await?;
        Ok(organisation)
    }

    /// The most recent activity for an organisation, newest first.
    pub async fn organisation_activity(
        &self,
        public_id: &str,
    ) -> Result<Vec<ActivityEntry>, ContactsError> {
        let tenant_id = self
            .authorize(ContactsOperation::GetOrganisation)?
            .tenant_id;
        require_public_id(public_id, "org")?;
        let mut tx = self.begin(tenant_id).await?;
        tx.get_organisation(tenant_id, public_id, false).await?;
        let activity = tx.activity(tenant_id, "organisation", public_id).await?;
        tx.commit().await?;
        Ok(activity)
    }

    pub async fn update_organisation(
        &self,
        public_id: &str,
        input: UpdateOrganisation,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        let authorized = self.authorize(ContactsOperation::UpdateOrganisation)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "org")?;
        validate_organisation_update(&input)?;
        let mut tx = self.begin(tenant_id).await?;
        let current = tx.get_organisation(tenant_id, public_id, false).await?;
        let definitions = tx
            .definitions(tenant_id, ContactCustomFieldTarget::Organisation)
            .await?;
        let custom_fields = validate_custom_fields(
            &input.custom_fields,
            &definitions,
            Some(&current.custom_fields),
        )?;
        self.validate_unique_values(
            tx.as_mut(),
            tenant_id,
            ContactCustomFieldTarget::Organisation,
            &definitions,
            &custom_fields,
            Some(current.id),
        )
        .await?;
        let organisation = tx
            .update_organisation(tenant_id, public_id, &input, &custom_fields)
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::OrganisationUpdated(&organisation),
        )
        .await?;
        tx.commit().await?;
        Ok(organisation)
    }

    pub async fn archive_organisation(
        &self,
        public_id: &str,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        let authorized = self.authorize(ContactsOperation::ArchiveOrganisation)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "org")?;
        validate_expected_version(expected_version)?;
        let mut tx = self.begin(tenant_id).await?;
        let organisation = tx
            .archive_organisation(tenant_id, public_id, expected_version)
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::OrganisationArchived(&organisation),
        )
        .await?;
        tx.commit().await?;
        Ok(organisation)
    }

    /// Moves an organisation to the recycle bin (ADR 0014). People keep
    /// their link until the organisation is purged; no new links are made.
    pub async fn delete_organisation(
        &self,
        public_id: &str,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        self.set_organisation_deleted(public_id, expected_version, true, context)
            .await
    }

    /// Restores an organisation from the recycle bin.
    pub async fn undelete_organisation(
        &self,
        public_id: &str,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        self.set_organisation_deleted(public_id, expected_version, false, context)
            .await
    }

    async fn set_organisation_deleted(
        &self,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        let authorized = self.authorize(ContactsOperation::DeleteOrganisation)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "org")?;
        validate_expected_version(expected_version)?;
        let mut tx = self.begin(tenant_id).await?;
        let organisation = tx
            .set_organisation_deleted(tenant_id, public_id, expected_version, deleted)
            .await?;
        let mutation = if deleted {
            ContactsMutation::OrganisationDeleted(&organisation)
        } else {
            ContactsMutation::OrganisationUndeleted(&organisation)
        };
        self.record_mutation(tx.as_mut(), authorized, context, mutation)
            .await?;
        tx.commit().await?;
        Ok(organisation)
    }

    pub async fn list_organisations(
        &self,
        request: ListRequest,
    ) -> Result<Page<Organisation>, ContactsError> {
        let tenant_id = self
            .authorize(ContactsOperation::ListOrganisations)?
            .tenant_id;
        validate_list_request(&request)?;
        require_filter_target(&request, ORGANISATION_RESOURCE)?;
        let mut tx = self.begin(tenant_id).await?;
        let page = tx.list_organisations(tenant_id, &request).await?;
        tx.commit().await?;
        Ok(page)
    }

    /// Applies kernel-validated extension values to a person.
    ///
    /// The caller must be able to read the person; the kernel extension
    /// service has already checked the extension's own write permission and
    /// validated every value against the active declarations.
    pub async fn update_person_extensions(
        &self,
        public_id: &str,
        expected_version: i64,
        patch: &ValidatedExtensionPatch,
        context: ContactsMutationContext,
    ) -> Result<Person, ContactsError> {
        let authorized = self.authorize(ContactsOperation::UpdatePersonExtensions)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "per")?;
        validate_expected_version(expected_version)?;
        require_patch_target(patch, PERSON_RESOURCE)?;
        let mut tx = self.begin(tenant_id).await?;
        let current = tx.get_person(tenant_id, public_id, false).await?;
        if current.archived_at.is_some() || current.version != expected_version {
            return Err(ContactsError::VersionConflict);
        }
        let mut extensions = current.extensions;
        patch.apply(&mut extensions).map_err(map_extension_error)?;
        let person = tx
            .update_person_extensions(tenant_id, public_id, expected_version, &extensions)
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::PersonUpdated(&person),
        )
        .await?;
        tx.commit().await?;
        Ok(person)
    }

    /// Applies kernel-validated extension values to an organisation.
    pub async fn update_organisation_extensions(
        &self,
        public_id: &str,
        expected_version: i64,
        patch: &ValidatedExtensionPatch,
        context: ContactsMutationContext,
    ) -> Result<Organisation, ContactsError> {
        let authorized = self.authorize(ContactsOperation::UpdateOrganisationExtensions)?;
        let tenant_id = authorized.tenant_id;
        require_public_id(public_id, "org")?;
        validate_expected_version(expected_version)?;
        require_patch_target(patch, ORGANISATION_RESOURCE)?;
        let mut tx = self.begin(tenant_id).await?;
        let current = tx.get_organisation(tenant_id, public_id, false).await?;
        if current.archived_at.is_some() || current.version != expected_version {
            return Err(ContactsError::VersionConflict);
        }
        let mut extensions = current.extensions;
        patch.apply(&mut extensions).map_err(map_extension_error)?;
        let organisation = tx
            .update_organisation_extensions(tenant_id, public_id, expected_version, &extensions)
            .await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::OrganisationUpdated(&organisation),
        )
        .await?;
        tx.commit().await?;
        Ok(organisation)
    }

    pub async fn create_definition(
        &self,
        input: CreateCustomFieldDefinition,
        context: ContactsMutationContext,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let authorized = self.authorize(ContactsOperation::CreateDefinition)?;
        let tenant_id = authorized.tenant_id;
        validate_custom_field_definition(&input)?;
        let mut tx = self.begin(tenant_id).await?;
        let definition = tx
            .insert_definition(tenant_id, &input)
            .await
            .map_err(map_definition_write_error)?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::CustomFieldDefinitionCreated(&definition),
        )
        .await?;
        tx.commit().await?;
        Ok(definition)
    }

    pub async fn list_definitions(
        &self,
        target: ContactCustomFieldTarget,
    ) -> Result<Vec<ContactCustomFieldDefinition>, ContactsError> {
        let tenant_id = self
            .authorize(ContactsOperation::ListDefinitions)?
            .tenant_id;
        let mut tx = self.begin(tenant_id).await?;
        let definitions = tx.definitions(tenant_id, target).await?;
        tx.commit().await?;
        Ok(definitions)
    }

    pub async fn get_definition(
        &self,
        id: Uuid,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let tenant_id = self.authorize(ContactsOperation::GetDefinition)?.tenant_id;
        let mut tx = self.begin(tenant_id).await?;
        let definition = tx.get_definition(tenant_id, id).await?;
        tx.commit().await?;
        Ok(definition)
    }

    pub async fn update_definition(
        &self,
        id: Uuid,
        input: UpdateCustomFieldDefinition,
        context: ContactsMutationContext,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let authorized = self.authorize(ContactsOperation::UpdateDefinition)?;
        let tenant_id = authorized.tenant_id;
        let mut tx = self.begin(tenant_id).await?;
        let existing = tx.get_definition(tenant_id, id).await?;
        validate_custom_field_definition_update(&existing, &input)?;
        let definition = tx.update_definition(tenant_id, id, &input).await?;
        self.record_mutation(
            tx.as_mut(),
            authorized,
            context,
            ContactsMutation::CustomFieldDefinitionUpdated(&definition),
        )
        .await?;
        tx.commit().await?;
        Ok(definition)
    }

    pub async fn disable_definition(
        &self,
        id: Uuid,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let authorized = self.authorize(ContactsOperation::DisableDefinition)?;
        self.set_definition_status(authorized, id, expected_version, false, context)
            .await
    }

    pub async fn archive_definition(
        &self,
        id: Uuid,
        expected_version: i64,
        context: ContactsMutationContext,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        let authorized = self.authorize(ContactsOperation::ArchiveDefinition)?;
        self.set_definition_status(authorized, id, expected_version, true, context)
            .await
    }

    async fn set_definition_status(
        &self,
        authorized: AuthorizedContacts,
        id: Uuid,
        expected_version: i64,
        archive: bool,
        context: ContactsMutationContext,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        validate_expected_version(expected_version)?;
        let tenant_id = authorized.tenant_id;
        let mut tx = self.begin(tenant_id).await?;
        let definition = tx
            .set_definition_status(tenant_id, id, expected_version, archive)
            .await?;
        let mutation = if archive {
            ContactsMutation::CustomFieldDefinitionArchived(&definition)
        } else {
            ContactsMutation::CustomFieldDefinitionDisabled(&definition)
        };
        self.record_mutation(tx.as_mut(), authorized, context, mutation)
            .await?;
        tx.commit().await?;
        Ok(definition)
    }

    async fn record_mutation(
        &self,
        tx: &mut dyn ContactsTx,
        authorized: AuthorizedContacts,
        context: ContactsMutationContext,
        mutation: ContactsMutation<'_>,
    ) -> Result<(), ContactsError> {
        let records = mutation
            .records(EvidenceContext::new(
                authorized.actor_id,
                context.correlation_id,
                context.causation_id,
            ))
            .map_err(|_| ContactsError::Invariant)?;
        tx.append_evidence(&records.audit, &records.event).await
    }

    async fn begin(&self, tenant_id: Uuid) -> Result<Box<dyn ContactsTx>, ContactsError> {
        self.store
            .begin(tenant_id, Actor::Member(self.authorization.actor_id()))
            .await
    }

    fn authorize(&self, operation: ContactsOperation) -> Result<AuthorizedContacts, ContactsError> {
        self.authorization.authorize(operation)
    }

    async fn resolve_organisation(
        &self,
        tx: &mut dyn ContactsTx,
        tenant_id: Uuid,
        public_id: Option<&str>,
    ) -> Result<Option<Uuid>, ContactsError> {
        let Some(public_id) = public_id else {
            return Ok(None);
        };
        require_public_id_input(public_id, "org", "organisation_id")?;
        tx.resolve_organisation_id(tenant_id, public_id)
            .await
            .map(Some)
    }

    async fn validate_unique_values(
        &self,
        tx: &mut dyn ContactsTx,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
        definitions: &[ContactCustomFieldDefinition],
        values: &BTreeMap<String, Value>,
        exclude_id: Option<Uuid>,
    ) -> Result<(), ContactsError> {
        for definition in definitions
            .iter()
            .filter(|definition| definition.is_active() && definition.unique_per_tenant)
        {
            let Some(value) = values.get(&definition.api_name) else {
                continue;
            };
            if tx
                .custom_value_exists(tenant_id, target, &definition.api_name, value, exclude_id)
                .await?
            {
                return Err(ContactsError::validation(
                    &definition.api_name,
                    "custom_field.not_unique",
                    format!(
                        "custom field {} must be unique in this workspace",
                        definition.api_name
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// Rejects a patch the kernel validated for a different resource.
fn require_patch_target(
    patch: &ValidatedExtensionPatch,
    resource: &str,
) -> Result<(), ContactsError> {
    if patch.target() == resource {
        Ok(())
    } else {
        Err(ContactsError::Invariant)
    }
}

fn require_filter_target(request: &ListRequest, resource: &str) -> Result<(), ContactsError> {
    match &request.extension_filter {
        Some(filter) if filter.target() != resource => Err(ContactsError::Invariant),
        _ => Ok(()),
    }
}

fn map_extension_error(error: ExtensionError) -> ContactsError {
    match error {
        ExtensionError::Invalid(issues) => ContactsError::Validation(
            issues
                .into_iter()
                .map(|issue| ValidationIssue::new(issue.field, issue.code, issue.detail))
                .collect(),
        ),
        ExtensionError::PermissionDenied => ContactsError::PermissionDenied,
        ExtensionError::Invariant => ContactsError::Invariant,
    }
}

fn require_public_id(public_id: &str, prefix: &str) -> Result<(), ContactsError> {
    let valid = PublicId::parse(public_id).is_ok_and(|parsed| parsed.prefix() == prefix);
    if valid {
        Ok(())
    } else {
        Err(ContactsError::NotFound)
    }
}

fn require_public_id_input(
    public_id: &str,
    prefix: &str,
    field: &str,
) -> Result<(), ContactsError> {
    if PublicId::parse(public_id).is_ok_and(|parsed| parsed.prefix() == prefix) {
        Ok(())
    } else {
        Err(ContactsError::validation(
            field,
            "public_id.invalid",
            format!("{field} must be a valid {prefix} public identifier"),
        ))
    }
}

fn map_definition_write_error(error: ContactsError) -> ContactsError {
    match error {
        ContactsError::Database(sqlx::Error::Database(database))
            if database.is_unique_violation() =>
        {
            ContactsError::validation(
                "api_name",
                "custom_field.api_name_conflict",
                "a custom field with this API name already exists for the target",
            )
        }
        error => error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::BTreeSet;

    #[tokio::test]
    async fn every_service_method_denies_before_validation_or_storage() {
        let service = denied_service();
        let id = Uuid::nil();

        assert_denied(service.create_person(invalid_person(), context()).await);
        assert_denied(service.get_person("invalid").await);
        assert_denied(
            service
                .update_person("invalid", invalid_person_update(), context())
                .await,
        );
        assert_denied(service.archive_person("invalid", 0, context()).await);
        assert_denied(service.list_people(invalid_list()).await);

        assert_denied(
            service
                .create_organisation(invalid_organisation(), context())
                .await,
        );
        assert_denied(service.get_organisation("invalid").await);
        assert_denied(
            service
                .update_organisation("invalid", invalid_organisation_update(), context())
                .await,
        );
        assert_denied(service.archive_organisation("invalid", 0, context()).await);
        assert_denied(service.list_organisations(invalid_list()).await);
        let patch = extension_patch();
        assert_denied(
            service
                .update_person_extensions("invalid", 0, &patch, context())
                .await,
        );
        assert_denied(
            service
                .update_organisation_extensions("invalid", 0, &patch, context())
                .await,
        );

        assert_denied(
            service
                .create_definition(invalid_definition(), context())
                .await,
        );
        assert_denied(
            service
                .list_definitions(ContactCustomFieldTarget::Person)
                .await,
        );
        assert_denied(service.get_definition(id).await);
        assert_denied(
            service
                .update_definition(id, invalid_definition_update(), context())
                .await,
        );
        assert_denied(service.disable_definition(id, 0, context()).await);
        assert_denied(service.archive_definition(id, 0, context()).await);
    }

    fn denied_service() -> ContactsService {
        let pool = PgPoolOptions::new()
            .connect_lazy("postgres://127.0.0.1:1/limiara")
            .expect("test connection string is valid");
        let authorization =
            ContactsAuthorization::new(Uuid::from_u128(1), Uuid::from_u128(2), BTreeSet::new());
        ContactsService::new(pool, authorization)
    }

    fn extension_patch() -> ValidatedExtensionPatch {
        let contacts = plugin_contract::Manifest::parse_toml(include_str!("../../plugin.toml"))
            .expect("Contacts manifest");
        let extension = plugin_contract::Manifest::parse_toml(
            r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.write"
description = "Change loyalty fields"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "vip"
label = "VIP"
field_type = "boolean"
read_permission = "loyalty.write"
write_permission = "loyalty.write"
"#,
        )
        .expect("extension manifest");
        platform_extensions::ExtensionCatalog::from_manifests([&contacts, &extension])
            .validate_patch(
                PERSON_RESOURCE,
                &BTreeMap::from([("example.loyalty.vip".to_owned(), Value::Bool(true))]),
                &|_: &str, _: &str| true,
            )
            .expect("valid patch")
    }

    fn context() -> ContactsMutationContext {
        ContactsMutationContext::new(Uuid::from_u128(3))
    }

    fn assert_denied<T>(result: Result<T, ContactsError>) {
        assert!(matches!(result, Err(ContactsError::PermissionDenied)));
    }

    fn invalid_person() -> CreatePerson {
        CreatePerson {
            full_name: String::new(),
            email: None,
            telephone: None,
            organisation_id: None,
            tags: Vec::new(),
            custom_fields: BTreeMap::new(),
        }
    }

    fn invalid_person_update() -> UpdatePerson {
        UpdatePerson {
            expected_version: 0,
            full_name: String::new(),
            email: None,
            telephone: None,
            organisation_id: None,
            tags: Vec::new(),
            custom_fields: BTreeMap::new(),
        }
    }

    fn invalid_organisation() -> CreateOrganisation {
        CreateOrganisation {
            name: String::new(),
            email: None,
            telephone: None,
            custom_fields: BTreeMap::new(),
        }
    }

    fn invalid_organisation_update() -> UpdateOrganisation {
        UpdateOrganisation {
            expected_version: 0,
            name: String::new(),
            email: None,
            telephone: None,
            custom_fields: BTreeMap::new(),
        }
    }

    fn invalid_list() -> ListRequest {
        ListRequest {
            limit: 0,
            ..ListRequest::default()
        }
    }

    fn invalid_definition() -> CreateCustomFieldDefinition {
        CreateCustomFieldDefinition {
            applies_to: ContactCustomFieldTarget::Person,
            api_name: "invalid".into(),
            label: String::new(),
            description: None,
            field_type: ContactCustomFieldType::Text,
            required: false,
            unique_per_tenant: false,
            indexed: false,
            options: Vec::new(),
            validation_schema: Value::Null,
            data_classification: DataClassification::Public,
            purpose: None,
            retention_days: None,
            encrypted: false,
        }
    }

    fn invalid_definition_update() -> UpdateCustomFieldDefinition {
        UpdateCustomFieldDefinition {
            expected_version: 0,
            label: String::new(),
            description: None,
            required: false,
            unique_per_tenant: false,
            indexed: false,
            options: Vec::new(),
            validation_schema: Value::Null,
            data_classification: DataClassification::Public,
            purpose: None,
            retention_days: None,
            encrypted: false,
        }
    }
}
