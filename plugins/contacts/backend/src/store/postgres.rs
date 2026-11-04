//! The PostgreSQL Contacts store: tables in the `p_contacts` schema under
//! forced row-level security and the Contacts plugin context.

use super::{Actor, ContactsStore, ContactsTx};
use crate::events::CONTACTS_PLUGIN_ID;
use crate::model::*;
use crate::{repository, ContactsError};
use async_trait::async_trait;
use platform_audit::AuditEvent;
use platform_db::{begin_plugin_tx, DbError};
use platform_events::EventEnvelope;
use platform_extensions::ExtensionValues;
use serde_json::Value;
use sqlx::{PgPool, Postgres, Transaction};
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Clone)]
pub struct PostgresContactsStore {
    pool: PgPool,
}

impl PostgresContactsStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ContactsStore for PostgresContactsStore {
    async fn begin(
        &self,
        tenant_id: Uuid,
        actor: Actor,
    ) -> Result<Box<dyn ContactsTx>, ContactsError> {
        // Every Contacts transaction runs in the Contacts plugin context, the
        // only context the restrictive policies on p_contacts tables admit.
        let tx = begin_plugin_tx(&self.pool, tenant_id, CONTACTS_PLUGIN_ID)
            .await
            .map_err(|DbError::Sqlx(error)| ContactsError::Database(error))?;
        Ok(Box::new(PostgresContactsTx { tx, actor }))
    }
}

struct PostgresContactsTx {
    tx: Transaction<'static, Postgres>,
    actor: Actor,
}

#[async_trait]
impl ContactsTx for PostgresContactsTx {
    async fn next_public_id(
        &mut self,
        tenant_id: Uuid,
        construct: &str,
        prefix: &str,
    ) -> Result<String, ContactsError> {
        repository::next_public_id(&mut self.tx, tenant_id, construct, prefix).await
    }

    async fn resolve_organisation_id(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
    ) -> Result<Uuid, ContactsError> {
        repository::resolve_organisation_id(&mut self.tx, tenant_id, public_id).await
    }

    async fn insert_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &CreatePerson,
        organisation_id: Option<Uuid>,
        custom_fields: &BTreeMap<String, Value>,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError> {
        repository::insert_person(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            input,
            organisation_id,
            custom_fields,
            extensions,
        )
        .await
    }

    async fn get_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Person, ContactsError> {
        repository::get_person(&mut self.tx, tenant_id, public_id, include_deleted).await
    }

    async fn set_person_deleted(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
    ) -> Result<Person, ContactsError> {
        repository::set_person_deleted(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            expected_version,
            deleted,
        )
        .await
    }

    async fn update_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &UpdatePerson,
        organisation_id: Option<Uuid>,
        custom_fields: &BTreeMap<String, Value>,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError> {
        repository::update_person(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            input,
            organisation_id,
            custom_fields,
            extensions,
        )
        .await
    }

    async fn archive_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
    ) -> Result<Person, ContactsError> {
        repository::archive_person(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            expected_version,
        )
        .await
    }

    async fn update_person_extensions(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError> {
        repository::update_person_extensions(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            expected_version,
            extensions,
        )
        .await
    }

    async fn list_people(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
    ) -> Result<Page<Person>, ContactsError> {
        repository::list_people(&mut self.tx, tenant_id, request).await
    }

    async fn insert_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &CreateOrganisation,
        custom_fields: &BTreeMap<String, Value>,
    ) -> Result<Organisation, ContactsError> {
        repository::insert_organisation(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            input,
            custom_fields,
        )
        .await
    }

    async fn get_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Organisation, ContactsError> {
        repository::get_organisation(&mut self.tx, tenant_id, public_id, include_deleted).await
    }

    async fn set_organisation_deleted(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
    ) -> Result<Organisation, ContactsError> {
        repository::set_organisation_deleted(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            expected_version,
            deleted,
        )
        .await
    }

    async fn update_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &UpdateOrganisation,
        custom_fields: &BTreeMap<String, Value>,
    ) -> Result<Organisation, ContactsError> {
        repository::update_organisation(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            input,
            custom_fields,
        )
        .await
    }

    async fn archive_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
    ) -> Result<Organisation, ContactsError> {
        repository::archive_organisation(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            expected_version,
        )
        .await
    }

    async fn update_organisation_extensions(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        extensions: &ExtensionValues,
    ) -> Result<Organisation, ContactsError> {
        repository::update_organisation_extensions(
            &mut self.tx,
            self.actor.member_id(),
            tenant_id,
            public_id,
            expected_version,
            extensions,
        )
        .await
    }

    async fn list_organisations(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
    ) -> Result<Page<Organisation>, ContactsError> {
        repository::list_organisations(&mut self.tx, tenant_id, request).await
    }

    async fn activity(
        &mut self,
        tenant_id: Uuid,
        entity_type: &str,
        public_id: &str,
    ) -> Result<Vec<ActivityEntry>, ContactsError> {
        repository::activity(&mut self.tx, tenant_id, entity_type, public_id).await
    }

    async fn definitions(
        &mut self,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
    ) -> Result<Vec<ContactCustomFieldDefinition>, ContactsError> {
        repository::definitions(&mut self.tx, tenant_id, target).await
    }

    async fn get_definition(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        repository::get_definition(&mut self.tx, tenant_id, id).await
    }

    async fn insert_definition(
        &mut self,
        tenant_id: Uuid,
        input: &CreateCustomFieldDefinition,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        repository::insert_definition(&mut self.tx, tenant_id, input).await
    }

    async fn update_definition(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
        input: &UpdateCustomFieldDefinition,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        repository::update_definition(&mut self.tx, tenant_id, id, input).await
    }

    async fn set_definition_status(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
        expected_version: i64,
        archive: bool,
    ) -> Result<ContactCustomFieldDefinition, ContactsError> {
        repository::set_definition_status(&mut self.tx, tenant_id, id, expected_version, archive)
            .await
    }

    async fn custom_value_exists(
        &mut self,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
        api_name: &str,
        value: &Value,
        exclude_id: Option<Uuid>,
    ) -> Result<bool, ContactsError> {
        repository::custom_value_exists(
            &mut self.tx,
            tenant_id,
            target,
            api_name,
            value,
            exclude_id,
        )
        .await
    }

    async fn append_evidence(
        &mut self,
        audit: &AuditEvent,
        event: &EventEnvelope,
    ) -> Result<(), ContactsError> {
        platform_audit::append_in_transaction(&mut self.tx, audit).await?;
        platform_outbox::append_in_transaction(&mut self.tx, event).await?;
        Ok(())
    }

    async fn commit(self: Box<Self>) -> Result<(), ContactsError> {
        self.tx.commit().await?;
        Ok(())
    }
}
