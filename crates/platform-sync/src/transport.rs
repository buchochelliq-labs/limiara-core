//! What the sync loop needs from the server: the workspace API routes a
//! device calls (docs/api/objects.md), with JSON bodies as the API renders
//! them. [`crate::wire`] turns those into engine types.

use crate::SyncError;
use async_trait::async_trait;
use futures_util::stream::BoxStream;
use platform_objects::composite::CompositeRequest;
use serde_json::Value;
use std::collections::BTreeMap;

/// One entry of `GET /objects/changes`. Entries carry no values: the record
/// is read through the record API, which applies sharing.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeEntry {
    pub position: i64,
    /// The object's API name.
    pub object: String,
    /// The record's public id.
    pub record: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChangesPage {
    pub entries: Vec<ChangeEntry>,
    /// Where to resume; it moves past entries the member cannot see too.
    pub next_after: i64,
    pub head: i64,
    pub metadata_version: i64,
}

/// One event of `GET /objects/changes/stream`.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// A change, with the same fields as a page entry.
    Change(ChangeEntry),
    /// The workspace metadata version: sent before the first changes and
    /// whenever definitions move.
    Metadata(i64),
    /// The position the stream resumed from has expired; the stream ends.
    Expired,
}

/// The events of one stream, until the server ends it (after 15 minutes)
/// or the connection drops.
pub type EventStream = BoxStream<'static, Result<StreamEvent, SyncError>>;

/// One operation's result in a composite response.
#[derive(Debug, Clone, PartialEq)]
pub struct CompositeResult {
    pub reference: String,
    pub status: u16,
    /// The record, or the problem the single-record route would return.
    pub body: Value,
}

/// A page of `status=any&sort=touched_at` records, tombstones included.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordsPage {
    pub records: Vec<Value>,
    pub next_cursor: Option<String>,
}

/// Every request must be safe to drop part-way: a background sync cuts
/// off the request in flight when its time is up ([`crate::SyncClient::wake`]).
#[async_trait]
pub trait SyncTransport: Send + Sync {
    /// `GET /objects`: the objects the member can read, each with
    /// `api_name` and `access`.
    async fn objects(&self) -> Result<Vec<Value>, SyncError>;

    /// `GET /objects/{api_name}`: the object with its active fields, or
    /// `None` when it is gone or no longer readable.
    async fn describe(&self, api_name: &str) -> Result<Option<Value>, SyncError>;

    /// `GET /objects/{api_name}/validation-rules`: the object's active
    /// rules, which every member who can read it may read, or `None` when the
    /// object is gone or no longer readable.
    async fn validation_rules(&self, api_name: &str) -> Result<Option<Vec<Value>>, SyncError>;

    /// `POST /composite`, with each operation's result in order.
    async fn composite(
        &self,
        request: &CompositeRequest,
    ) -> Result<Vec<CompositeResult>, SyncError>;

    /// `GET /objects/{api_name}/records/{id}?include_deleted=true`, or
    /// `None` when the member cannot read it.
    async fn record(&self, api_name: &str, public_id: &str) -> Result<Option<Value>, SyncError>;

    /// `GET /objects/changes?after=&limit=`. Without `after`, the page
    /// starts at the oldest change the server keeps, so it never expires;
    /// a position older than that is [`SyncError::PositionExpired`].
    async fn changes(&self, after: Option<i64>, limit: u16) -> Result<ChangesPage, SyncError>;

    /// `GET /objects/changes/stream?after=`, held open while the app is in
    /// the foreground. A position older than the server keeps is
    /// [`SyncError::PositionExpired`] or an [`StreamEvent::Expired`] event.
    async fn stream(&self, after: Option<i64>) -> Result<EventStream, SyncError>;

    /// `GET /objects/{api_name}/records?status=any&sort=touched_at`, from
    /// `cursor`.
    async fn records_page(
        &self,
        api_name: &str,
        cursor: Option<&str>,
    ) -> Result<RecordsPage, SyncError>;

    /// `GET /users` and `GET /roles`: the workspace's members and roles with
    /// their names, which every member may read.
    async fn directory(&self) -> Result<platform_objects::ReplicaDirectory, SyncError>;

    /// `POST /sync/revocations`: of the records named, by object, those the
    /// member can no longer read. At most 1,000 ids a request.
    async fn revocations(
        &self,
        records: &BTreeMap<String, Vec<String>>,
    ) -> Result<BTreeMap<String, Vec<String>>, SyncError>;
}
