//! The current-route context (ADR 0034 §9): which record or object the web
//! shell shows, for native plugins that tell the OS what is on screen.
//!
//! The web shell sends `POST /api/v1/workspaces/{id}/ui/route` with
//! `{route, object?, record_id?}` when it navigates, and only to a host
//! whose UI contributions say `route_context: true`; the server's never do.
//! `route` names the shell's route pattern, never the address, so search
//! terms and filters stay in the web view; `object` and `record_id` are ids,
//! never record content. The message is untrusted data: [`RouteMessage::parse`]
//! accepts only that shape, and the bridge then checks that the object is in
//! the replica and that the id is one of its record ids. Nothing in it is
//! ever run or rendered.
//!
//! Plugins read the latest route through a [`RouteSubscription`], which only
//! the shell's grant of `device.route_context` hands out. Only the latest
//! route matters, so a subscription holds one value: a plugin that is slow
//! sees the route as it is now, not every step on the way.

use platform_id::PublicId;
use platform_objects::naming::ApiName;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use uuid::Uuid;

/// The largest route message the bridge reads.
pub const MESSAGE_LIMIT: usize = 1024;

/// The web shell's route patterns, by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutePattern {
    /// The workspace's home page.
    Home,
    /// An object's records (`object`).
    Object,
    /// A new record of an object (`object`).
    NewRecord,
    /// One record (`object` and `record_id`).
    Record,
    /// Search results; the terms stay in the web view.
    Search,
    /// Settings pages.
    Settings,
    /// Any other page, such as a plugin's own.
    Other,
}

impl RoutePattern {
    fn takes_object(self) -> bool {
        matches!(self, Self::Object | Self::NewRecord | Self::Record)
    }

    fn takes_record(self) -> bool {
        self == Self::Record
    }
}

/// Why the bridge refused a route message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RouteError {
    /// Not the message's shape: not JSON, an unknown pattern or key, an
    /// object or id missing, present where the pattern takes none, or not
    /// well formed.
    #[error("{0}")]
    Invalid(&'static str),
    /// The object is not in the replica, or the member cannot read it.
    #[error("the object is not on this device")]
    ObjectNotFound,
    /// The record id is not one of the object's.
    #[error("the record id is not one of the object's")]
    RecordIdInvalid,
}

impl RouteError {
    /// The stable problem code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "route.invalid",
            Self::ObjectNotFound => "route.object_not_found",
            Self::RecordIdInvalid => "route.record_id_invalid",
        }
    }
}

/// A route message as the web shell sends it, checked for shape.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouteMessage {
    pub route: RoutePattern,
    #[serde(default)]
    pub object: Option<String>,
    #[serde(default)]
    pub record_id: Option<String>,
}

impl RouteMessage {
    /// Accepts exactly `{route, object?, record_id?}`: a known pattern, an
    /// object API name where the pattern takes one, and a well-formed
    /// record id where it takes one, and nothing else.
    pub fn parse(body: &[u8]) -> Result<Self, RouteError> {
        if body.len() > MESSAGE_LIMIT {
            return Err(RouteError::Invalid("the route message is too large"));
        }
        let message: Self = serde_json::from_slice(body).map_err(|_| {
            RouteError::Invalid("the route message is not {route, object?, record_id?}")
        })?;
        match (&message.object, message.route.takes_object()) {
            (Some(object), true) => {
                if ApiName::parse(object).is_none() {
                    return Err(RouteError::Invalid("the object is not an API name"));
                }
            }
            (None, true) => return Err(RouteError::Invalid("this route names an object")),
            (Some(_), false) => return Err(RouteError::Invalid("this route names no object")),
            (None, false) => {}
        }
        match (&message.record_id, message.route.takes_record()) {
            (Some(record_id), true) => {
                if PublicId::parse(record_id).is_err() {
                    return Err(RouteError::RecordIdInvalid);
                }
            }
            (None, true) => return Err(RouteError::Invalid("this route names a record")),
            (Some(_), false) => return Err(RouteError::Invalid("this route names no record")),
            (None, false) => {}
        }
        Ok(message)
    }

    /// Whether `record_id` belongs to an object with `key_prefix`.
    pub(crate) fn record_matches(&self, key_prefix: &str) -> bool {
        self.record_id.as_deref().is_none_or(|record_id| {
            PublicId::parse(record_id).is_ok_and(|id| id.prefix() == key_prefix)
        })
    }
}

/// The route the web shell shows, as plugins receive it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CurrentRoute {
    pub workspace: Uuid,
    pub route: RoutePattern,
    pub object: Option<String>,
    pub record_id: Option<String>,
}

/// The route context. The controller owns it; the bridge sets it, and
/// plugins read it through [`crate::capabilities::PluginGrant`].
///
/// The latest message the bridge received wins, not the latest it finished
/// checking: each message takes a [`RouteTicket`] as it arrives, and one
/// that a later message overtook while its object was being looked up is
/// dropped. A clear drops every message received before it.
#[derive(Clone)]
pub struct RouteContext {
    route: Arc<watch::Sender<Option<CurrentRoute>>>,
    /// The last ticket handed out.
    issued: Arc<AtomicU64>,
    /// The newest ticket applied, or the last issued at a clear. Changed
    /// only inside `send_if_modified`, under the watch's lock.
    applied: Arc<AtomicU64>,
}

impl Default for RouteContext {
    fn default() -> Self {
        Self {
            route: Arc::new(watch::Sender::new(None)),
            issued: Arc::new(AtomicU64::new(0)),
            applied: Arc::new(AtomicU64::new(0)),
        }
    }
}

/// When a route message arrived, relative to the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct RouteTicket(u64);

impl std::fmt::Debug for RouteContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("RouteContext")
    }
}

impl RouteContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Taken as a route message arrives, before anything is awaited.
    pub(crate) fn ticket(&self) -> RouteTicket {
        RouteTicket(self.issued.fetch_add(1, Ordering::SeqCst) + 1)
    }

    /// Sets the route a message received with `ticket` names, unless a
    /// message received after it was set already, or a clear came after it.
    /// Returns whether the route is now the message's.
    pub(crate) fn set(&self, ticket: RouteTicket, route: CurrentRoute) -> bool {
        let mut current_now = false;
        self.route.send_if_modified(|current| {
            if ticket.0 <= self.applied.load(Ordering::SeqCst) {
                return false;
            }
            self.applied.store(ticket.0, Ordering::SeqCst);
            current_now = true;
            if current.as_ref() == Some(&route) {
                return false;
            }
            *current = Some(route);
            true
        });
        current_now
    }

    /// Forgets the route, as when the workspace closes. A message received
    /// before it and still being checked is dropped.
    pub fn clear(&self) {
        self.route.send_if_modified(|current| {
            let issued = self.issued.load(Ordering::SeqCst);
            self.applied.fetch_max(issued, Ordering::SeqCst);
            current.take().is_some()
        });
    }

    /// The route now.
    pub fn current(&self) -> Option<CurrentRoute> {
        self.route.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> RouteSubscription {
        let mut receiver = self.route.subscribe();
        // The route as it is now counts as a change, so a new subscriber
        // hears it first.
        receiver.mark_changed();
        RouteSubscription(receiver)
    }
}

/// A plugin's view of the route context.
#[derive(Debug)]
pub struct RouteSubscription(watch::Receiver<Option<CurrentRoute>>);

impl RouteSubscription {
    /// The route now, without waiting.
    pub fn current(&self) -> Option<CurrentRoute> {
        self.0.borrow().clone()
    }

    /// Waits for the route to change and returns it: `None` when no
    /// workspace route is shown, such as after sign-out. `Err` once the
    /// context is gone with the app.
    pub async fn changed(&mut self) -> Result<Option<CurrentRoute>, RouteClosed> {
        self.0.changed().await.map_err(|_| RouteClosed)?;
        Ok(self.0.borrow_and_update().clone())
    }
}

/// The route context ended with the app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("the route context ended")]
pub struct RouteClosed;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: &str) -> Result<RouteMessage, RouteError> {
        RouteMessage::parse(body.as_bytes())
    }

    /// A well-formed record id of an object with `prefix`.
    fn id(prefix: &str) -> String {
        PublicId::new(prefix, 1).expect("id").as_str().to_owned()
    }

    #[test]
    fn a_route_message_is_only_its_shape() {
        let record = id("a01");
        assert_eq!(
            parse(&format!(
                r#"{{"route":"record","object":"custom:account","record_id":"{record}"}}"#
            )),
            Ok(RouteMessage {
                route: RoutePattern::Record,
                object: Some("custom:account".into()),
                record_id: Some(record.clone()),
            })
        );
        assert!(parse(r#"{"route":"object","object":"example.tracker:ticket"}"#).is_ok());
        assert!(parse(r#"{"route":"new_record","object":"custom:account"}"#).is_ok());
        assert!(parse(r#"{"route":"home"}"#).is_ok());
        assert!(parse(r#"{"route":"search","object":null,"record_id":null}"#).is_ok());

        let invalid = |body: &str| parse(body).map_err(|error| error.code());
        // Not the shape: not JSON, not an object, unknown patterns or keys.
        for body in [
            "",
            "null",
            "[]",
            r#""record""#,
            r#"{}"#,
            r#"{"route":"Record"}"#,
            r#"{"route":"/o/custom:account"}"#,
            r#"{"route":"home","path":"/o/custom:account?q=secret"}"#,
            r#"{"route":"home","name":"Acme"}"#,
            r#"{"route":1}"#,
            r#"{"route":"object","object":42}"#,
        ] {
            assert_eq!(invalid(body), Err("route.invalid"), "{body}");
        }
        // Objects and records only where the pattern takes them.
        for body in [
            r#"{"route":"record","object":"custom:account"}"#.to_owned(),
            format!(r#"{{"route":"record","record_id":"{record}"}}"#),
            r#"{"route":"object"}"#.to_owned(),
            r#"{"route":"home","object":"custom:account"}"#.to_owned(),
            format!(r#"{{"route":"object","object":"custom:account","record_id":"{record}"}}"#),
            format!(r#"{{"route":"search","record_id":"{record}"}}"#),
        ] {
            assert_eq!(invalid(&body), Err("route.invalid"), "{body}");
        }
        // Ids, never anything that only looks like one.
        for object in [
            "account",
            "Custom:Account",
            "custom:account<script>",
            "custom:",
            "custom:account.name",
        ] {
            let body = format!(r#"{{"route":"object","object":"{object}"}}"#);
            assert_eq!(invalid(&body), Err("route.invalid"), "{body}");
        }
        let mut wrong_checksum = record.clone();
        wrong_checksum.pop();
        wrong_checksum.push(if record.ends_with('0') { '1' } else { '0' });
        for record_id in [
            String::new(),
            "Acme Ltd".to_owned(),
            "a01".to_owned(),
            record.to_uppercase(),
            format!("../{record}"),
            wrong_checksum,
        ] {
            let body = format!(
                r#"{{"route":"record","object":"custom:account","record_id":"{record_id}"}}"#
            );
            assert_eq!(invalid(&body), Err("route.record_id_invalid"), "{body}");
        }
        let large = format!(r#"{{"route":"home","x":"{}"}}"#, "a".repeat(MESSAGE_LIMIT));
        assert_eq!(invalid(&large), Err("route.invalid"));
    }

    #[test]
    fn a_record_id_must_be_one_of_the_objects() {
        let message = parse(&format!(
            r#"{{"route":"record","object":"custom:account","record_id":"{}"}}"#,
            id("a01")
        ))
        .expect("message");
        assert!(message.record_matches("a01"));
        assert!(!message.record_matches("a02"));
        let home = parse(r#"{"route":"home"}"#).expect("home");
        assert!(home.record_matches("a01"));
    }

    #[tokio::test]
    async fn subscribers_see_the_latest_route() {
        let context = RouteContext::new();
        let workspace = Uuid::now_v7();
        let route = |object: &str| CurrentRoute {
            workspace,
            route: RoutePattern::Object,
            object: Some(object.into()),
            record_id: None,
        };
        context.set(context.ticket(), route("custom:account"));
        let mut subscription = context.subscribe();
        // A new subscriber hears the route as it is.
        assert_eq!(
            subscription.changed().await,
            Ok(Some(route("custom:account")))
        );
        // Several steps while it is busy: it sees where the shell is now.
        context.set(context.ticket(), route("custom:contact"));
        context.set(context.ticket(), route("custom:deal"));
        assert_eq!(subscription.changed().await, Ok(Some(route("custom:deal"))));
        // The same route again is no change.
        context.set(context.ticket(), route("custom:deal"));
        context.clear();
        assert_eq!(subscription.changed().await, Ok(None));
        assert_eq!(subscription.current(), None);
    }

    #[test]
    fn the_latest_message_received_wins_over_a_slower_earlier_one() {
        let context = RouteContext::new();
        let workspace = Uuid::now_v7();
        let route = |pattern: RoutePattern, object: Option<&str>| CurrentRoute {
            workspace,
            route: pattern,
            object: object.map(str::to_owned),
            record_id: None,
        };
        // A record page arrives and its object is looked up; home arrives
        // after it, needs no lookup, and is set first.
        let record = context.ticket();
        let home = context.ticket();
        assert!(context.set(home, route(RoutePattern::Home, None)));
        // The record page's lookup finishes: it was overtaken.
        assert!(!context.set(record, route(RoutePattern::Object, Some("custom:account"))));
        assert_eq!(context.current(), Some(route(RoutePattern::Home, None)));
        // The same route again is still current.
        assert!(context.set(context.ticket(), route(RoutePattern::Home, None)));

        // A message still being checked when the workspace closes is
        // dropped; one received after the clear is not.
        let pending = context.ticket();
        context.clear();
        assert!(!context.set(pending, route(RoutePattern::Search, None)));
        assert_eq!(context.current(), None);
        assert!(context.set(context.ticket(), route(RoutePattern::Search, None)));
        assert_eq!(context.current(), Some(route(RoutePattern::Search, None)));
    }
}
