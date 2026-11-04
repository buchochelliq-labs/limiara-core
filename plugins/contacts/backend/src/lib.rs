#![forbid(unsafe_code)]

mod authorization;
pub mod events;
pub mod extension_host;
mod model;
mod repository;
mod service;
pub mod store;
pub mod ui;
mod validation;
pub mod worker;

pub use authorization::{
    ContactsAuthorization, ORGANISATIONS_READ_PERMISSION, ORGANISATIONS_WRITE_PERMISSION,
    PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION, SETTINGS_MANAGE_PERMISSION,
};
pub use model::*;
pub use service::{ContactsMutationContext, ContactsService};
pub use validation::{
    validate_custom_field_api_name, validate_custom_field_definition, validate_custom_fields,
    validate_list_request, validate_organisation, validate_person,
};

use thiserror::Error;

/// Host resource ids other plugins may extend (ADR 0012).
pub const PERSON_RESOURCE: &str = "platform.contacts.person";
pub const ORGANISATION_RESOURCE: &str = "platform.contacts.organisation";

#[derive(Debug, Error)]
pub enum ContactsError {
    #[error("validation failed")]
    Validation(Vec<ValidationIssue>),
    #[error("permission denied")]
    PermissionDenied,
    #[error("record not found")]
    NotFound,
    #[error("version conflict")]
    VersionConflict,
    #[error("cursor is invalid or does not belong to this query")]
    InvalidCursor,
    #[error("contacts storage is unavailable")]
    Database(#[from] sqlx::Error),
    #[error("contacts data invariant failed")]
    Invariant,
}

impl ContactsError {
    pub(crate) fn validation(
        field: impl Into<String>,
        code: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self::Validation(vec![ValidationIssue::new(field, code, detail)])
    }
}
