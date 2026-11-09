#![forbid(unsafe_code)]
//! The kernel object engine (ADR 0015).
//!
//! Objects and fields are metadata; records of every object live in one
//! generic store with standard fields as columns, custom values in a document
//! validated against field metadata, and typed pivots for indexes,
//! uniqueness and references. The engine is domain-neutral: it knows about
//! objects, fields and records, never about any business concept.
//!
//! - [`naming`] decides API names and public-ID key prefixes.
//! - [`model`] holds definitions, inputs and records.
//! - [`values`] validates and normalises record values and derives the
//!   pivot rows that index them.
//! - [`store`] is the storage port with PostgreSQL and SQLite adapters.
//! - [`quick_actions`] holds administrator-defined record shortcuts
//!   (ADR 0032).
//! - [`service`] authorises, validates and records evidence for every change.

pub mod access;
pub mod automation;
pub mod bulk;
pub mod composite;
pub mod events;
pub mod formula;
pub mod formula_fields;
pub mod model;
pub mod naming;
pub mod push;
pub mod query;
pub mod quick_actions;
pub mod related;
pub mod replica;
pub mod replica_definitions;
pub mod reports;
pub mod rules;
pub mod search;
pub mod service;
pub mod sharing;
pub mod store;
pub mod sync;
pub mod values;

pub use access::{ObjectAccess, ObjectCaller};
pub use model::*;
pub use push::{PushAnswer, PushBatch, PushOperation, PushReport};
pub use query::{QueryColumn, QueryRequest, QueryResult};
pub use related::{RelatedList, Relationship};
pub use replica::{
    Applied, Choices, DeleteConflict, FieldConflict, Lease, Merge, RemoteRecord, Side,
    TrustedClock, TrustedInstant,
};
pub use replica_definitions::{
    DefinitionsApplied, RemoteField, RemoteObject, RemoteRollup, RemoteRule,
};
pub use reports::{CreateReport, Report, ReportChart, ReportVisibility, UpdateReport};
pub use search::{SearchGroup, SearchHit, SearchRequest};
pub use service::{MutationContext, ObjectService};
pub use sharing::{GranteeKind, RecordRights, RecordShare, ShareAccess, Visibility};
pub use store::{ObjectStore, ObjectTx, RollupQuery};

use serde::Serialize;
use thiserror::Error;

/// The engine's execution context and event source (ADR 0010, ADR 0015).
pub const ENGINE_CONTEXT: &str = "platform.objects";

/// System permission to define objects, fields and object permissions.
pub const OBJECTS_MANAGE_PERMISSION: &str = "objects.manage";
/// System permission to read every record of every object.
pub const RECORDS_VIEW_ALL_PERMISSION: &str = "records.view_all";
/// System permission to read, change and delete every record.
pub const RECORDS_MODIFY_ALL_PERMISSION: &str = "records.modify_all";
/// Change the owner of records someone else owns (Salesforce "Transfer
/// Record"; ADR 0016).
pub const RECORDS_TRANSFER_PERMISSION: &str = "records.transfer";
/// Share reports with the workspace and change or delete anyone's
/// (ADR 0027).
pub const REPORTS_MANAGE_PERMISSION: &str = "reports.manage";

/// One problem with a request, addressed to a field where possible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationIssue {
    pub field: String,
    pub code: &'static str,
    pub detail: String,
}

impl ValidationIssue {
    pub fn new(field: impl Into<String>, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            code,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum ObjectError {
    #[error("not found")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("validation failed")]
    Validation(Vec<ValidationIssue>),
    #[error("version conflict")]
    VersionConflict,
    /// A state conflict other than a version, with a stable code such as
    /// `objects.duplicate_api_name` or `records.duplicate_value`.
    #[error("conflict: {0}")]
    Conflict(&'static str),
    #[error("database error")]
    Database(#[from] sqlx::Error),
    #[error("internal invariant violated")]
    Invariant,
}

impl ObjectError {
    pub fn invalid(
        field: impl Into<String>,
        code: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self::Validation(vec![ValidationIssue::new(field, code, detail)])
    }

    /// Whether a database error is a unique-constraint violation.
    pub fn is_unique_violation(error: &sqlx::Error) -> bool {
        error
            .as_database_error()
            .is_some_and(|error| error.is_unique_violation())
    }
}
