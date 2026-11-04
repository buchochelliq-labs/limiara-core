#![forbid(unsafe_code)]
//! A synced device's sync loop (ADR 0024 §3–4).
//!
//! The device's engine is a replica of one server workspace
//! (`ObjectService::with_replica`). [`SyncClient::sync_once`] brings it in
//! line with the server:
//!
//! 1. definitions, when the server's metadata version moved;
//! 2. revocations, after each reconnect and after definitions moved;
//! 3. push of the member's queued changes as composite requests, merging
//!    with the server's copy after a version conflict;
//! 4. pull of the change log from the stored position.
//!
//! The server is reached through a [`SyncTransport`]: [`HttpTransport`]
//! calls the workspace API with the device's own access token (ADR 0029).
//! Token refresh and storage stay with the app.

pub mod client;
pub mod error;
pub mod http;
pub mod transport;
pub mod wire;

pub use client::{PendingConflict, SyncClient, SyncReport};
pub use error::SyncError;
pub use http::{AccessToken, HttpTransport};
pub use transport::{
    ChangeEntry, ChangesPage, CompositeResult, EventStream, RecordsPage, StreamEvent, SyncTransport,
};
