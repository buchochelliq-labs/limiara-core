//! The Contacts store port (ADR 0013).
//!
//! `ContactsService` owns validation, authorisation and evidence; a store
//! only persists. Each supported backend implements [`ContactsStore`], and
//! the same conformance suite runs against every implementation.

use crate::model::*;
use crate::ContactsError;
use async_trait::async_trait;
use platform_audit::AuditEvent;
use platform_events::EventEnvelope;
use platform_extensions::ExtensionValues;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

pub mod postgres;
pub mod sqlite;

pub use postgres::PostgresContactsStore;
pub use sqlite::SqliteContactsStore;

/// Who a unit of work acts for, which decides the standard record stamps
/// (ADR 0014).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Actor {
    /// A workspace member, from the authenticated session. Creating a record
    /// sets `created_by`; every write sets `updated_at`, `updated_by` and
    /// `touched_at`.
    Member(Uuid),
    /// The platform itself, such as a retention purge. Writes move only
    /// `touched_at`.
    System,
}

impl Actor {
    pub fn member_id(self) -> Option<Uuid> {
        match self {
            Self::Member(id) => Some(id),
            Self::System => None,
        }
    }
}

/// Opens tenant-scoped units of work.
#[async_trait]
pub trait ContactsStore: Send + Sync {
    /// A write transaction for one workspace, acting for `actor`. Domain
    /// rows, audit evidence and the outbox event written through it commit
    /// together or not at all.
    async fn begin(
        &self,
        tenant_id: Uuid,
        actor: Actor,
    ) -> Result<Box<dyn ContactsTx>, ContactsError>;
}

/// One workspace transaction. Every method is scoped to `tenant_id`, and
/// every record write stamps the standard fields for the transaction's actor.
#[async_trait]
pub trait ContactsTx: Send {
    async fn next_public_id(
        &mut self,
        tenant_id: Uuid,
        construct: &str,
        prefix: &str,
    ) -> Result<String, ContactsError>;
    async fn resolve_organisation_id(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
    ) -> Result<Uuid, ContactsError>;

    async fn insert_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &CreatePerson,
        organisation_id: Option<Uuid>,
        custom_fields: &BTreeMap<String, Value>,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError>;
    /// A person outside the recycle bin, or any person with
    /// `include_deleted`.
    async fn get_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Person, ContactsError>;
    async fn update_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &UpdatePerson,
        organisation_id: Option<Uuid>,
        custom_fields: &BTreeMap<String, Value>,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError>;
    async fn archive_person(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
    ) -> Result<Person, ContactsError>;
    /// Moves a person into (`deleted`) or out of the recycle bin. Ordinary
    /// reads and writes treat a deleted record as not found.
    async fn set_person_deleted(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
    ) -> Result<Person, ContactsError>;
    async fn update_person_extensions(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        extensions: &ExtensionValues,
    ) -> Result<Person, ContactsError>;
    async fn list_people(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
    ) -> Result<Page<Person>, ContactsError>;

    async fn insert_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &CreateOrganisation,
        custom_fields: &BTreeMap<String, Value>,
    ) -> Result<Organisation, ContactsError>;
    async fn get_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        include_deleted: bool,
    ) -> Result<Organisation, ContactsError>;
    async fn update_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        input: &UpdateOrganisation,
        custom_fields: &BTreeMap<String, Value>,
    ) -> Result<Organisation, ContactsError>;
    async fn archive_organisation(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
    ) -> Result<Organisation, ContactsError>;
    async fn set_organisation_deleted(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        deleted: bool,
    ) -> Result<Organisation, ContactsError>;
    async fn update_organisation_extensions(
        &mut self,
        tenant_id: Uuid,
        public_id: &str,
        expected_version: i64,
        extensions: &ExtensionValues,
    ) -> Result<Organisation, ContactsError>;
    async fn list_organisations(
        &mut self,
        tenant_id: Uuid,
        request: &ListRequest,
    ) -> Result<Page<Organisation>, ContactsError>;

    async fn activity(
        &mut self,
        tenant_id: Uuid,
        entity_type: &str,
        public_id: &str,
    ) -> Result<Vec<ActivityEntry>, ContactsError>;

    async fn definitions(
        &mut self,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
    ) -> Result<Vec<ContactCustomFieldDefinition>, ContactsError>;
    async fn get_definition(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
    ) -> Result<ContactCustomFieldDefinition, ContactsError>;
    async fn insert_definition(
        &mut self,
        tenant_id: Uuid,
        input: &CreateCustomFieldDefinition,
    ) -> Result<ContactCustomFieldDefinition, ContactsError>;
    async fn update_definition(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
        input: &UpdateCustomFieldDefinition,
    ) -> Result<ContactCustomFieldDefinition, ContactsError>;
    async fn set_definition_status(
        &mut self,
        tenant_id: Uuid,
        id: Uuid,
        expected_version: i64,
        archive: bool,
    ) -> Result<ContactCustomFieldDefinition, ContactsError>;
    /// Whether another record already holds `value` for a unique custom
    /// field. Implementations serialise the check with the following write.
    async fn custom_value_exists(
        &mut self,
        tenant_id: Uuid,
        target: ContactCustomFieldTarget,
        api_name: &str,
        value: &Value,
        exclude_id: Option<Uuid>,
    ) -> Result<bool, ContactsError>;

    /// Appends the audit entry and the outbox event for this mutation.
    async fn append_evidence(
        &mut self,
        audit: &AuditEvent,
        event: &EventEnvelope,
    ) -> Result<(), ContactsError>;
    // --- Moving a workspace to the object engine (ADR 0035 §2) ----------

    /// Up to `limit` people in any state (archived and in the recycle bin
    /// too), with an internal id after `after`, in id order.
    async fn export_people(
        &mut self,
        tenant_id: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Person>, ContactsError>;
    /// Up to `limit` organisations in any state, after `after`, in id order.
    async fn export_organisations(
        &mut self,
        tenant_id: Uuid,
        after: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<Organisation>, ContactsError>;
    /// Every activity entry of these records of `entity_type`, oldest first.
    async fn export_activity(
        &mut self,
        tenant_id: Uuid,
        entity_type: &str,
        public_ids: &[String],
    ) -> Result<Vec<ExportedActivity>, ContactsError>;
    /// The last public-id sequence issued for `construct` (the counter
    /// table's `next_value`, which holds the value last handed out); 0
    /// when none was.
    async fn public_id_counter(
        &mut self,
        tenant_id: Uuid,
        construct: &str,
    ) -> Result<i64, ContactsError>;
    /// The workspace's migration state; `None` before it starts.
    async fn engine_migration(
        &mut self,
        tenant_id: Uuid,
    ) -> Result<Option<EngineMigration>, ContactsError>;
    /// Writes the migration state: creates it when `expected_version` is
    /// `None`, otherwise replaces the version read, or answers
    /// `VersionConflict`. Returns the state as stored.
    async fn save_engine_migration(
        &mut self,
        tenant_id: Uuid,
        state: &EngineMigration,
        expected_version: Option<i64>,
    ) -> Result<EngineMigration, ContactsError>;

    async fn commit(self: Box<Self>) -> Result<(), ContactsError>;
}
