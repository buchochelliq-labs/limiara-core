//! The Activity timeline (ADR 0033 §4): the records of other objects that
//! point at a record, in two groups, "open and upcoming" and "past".
//!
//! The kernel knows nothing about tasks, calls or events. A plugin object
//! declares itself a *timeline source* in its manifest
//! (`[objects.timeline]`, see `plugin_contract::objects`): the lookup or
//! related-to fields that reach the viewed record, the date that orders the
//! list, and when a record is open. Installing the plugin keeps the
//! declaration per workspace as a [`TimelineSource`]; dropping it from the
//! manifest drops it.
//!
//! - **As the viewer.** The timeline reads records through the query
//!   language as the caller, like related lists (ADR 0028): the viewer must
//!   see the record itself, and sees only the records sharing lets them.
//!   Sources whose object is suspended (plugin disabled) or that the viewer
//!   cannot read contribute nothing.
//! - **Which fields.** A source is followed through each of its `through`
//!   fields that can point at the record's object: a lookup to it, a
//!   related-to field whose allowlist names it, or a related-to field that
//!   still holds a value to one of its records after the object left the
//!   allowlist (values outlive that, ADR 0033 §1).
//! - **Ordering.** Open records come soonest date first, then those without
//!   a date, oldest first. Past records come newest first by their date, or
//!   by when they were created if they have none; a date counts from
//!   midnight UTC. Ties keep each source's own order.
//! - **Paging.** The open group holds at most [`TIMELINE_OPEN_MAX`] records
//!   and comes only with the first page; the past group pages
//!   [`TIMELINE_PAGE`] at a time. Its cursor holds one query cursor per
//!   source and kind, so paging merges the sources without reading any of
//!   them twice.

use crate::related::literal;
use crate::service::ObjectService;
use crate::{
    FieldDefinition, FieldType, ObjectAccess, ObjectDefinition, ObjectError, ObjectTx,
    QueryRequest, QueryResult, QuickActionKind, TimelineOpen, TimelineSource,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, NaiveDate, Utc};
use plugin_contract::{ObjectDecl, TimelineOpenDecl};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Open records the timeline lists.
pub const TIMELINE_OPEN_MAX: u16 = 50;
/// Past records per page.
pub const TIMELINE_PAGE: u16 = 20;
/// Picklist values an item carries as display text.
pub const TIMELINE_DETAILS_MAX: usize = 3;
/// A cursor that is not one this timeline issued.
pub const TIMELINE_CURSOR_INVALID: &str = "timeline.cursor_invalid";

/// A record's Activity timeline, as the viewer may see it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Timeline {
    /// Sources that can point at this record and the viewer can read. The
    /// web page shows no Activity section when there are none.
    pub sources: usize,
    /// Open and upcoming records, soonest first; on the first page only.
    pub open: Vec<TimelineItem>,
    /// Whether more open records exist than the timeline lists.
    pub open_more: bool,
    /// One page of past records, newest first.
    pub past: Vec<TimelineItem>,
    /// The cursor of the next page of past records.
    pub next_cursor: Option<String>,
    /// The names of the record's object's create quick actions that make
    /// records of a timeline source: the ones the Activity section offers.
    pub actions: Vec<String>,
}

/// One record on a timeline.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineItem {
    pub object: String,
    pub object_label: String,
    pub id: String,
    pub name: String,
    /// The value of `date_field`: a date or an RFC 3339 date-time.
    pub date: Option<String>,
    pub date_field: String,
    pub date_type: FieldType,
    pub created_at: Value,
    pub open: bool,
    /// The `through` field that points at the viewed record.
    pub through: String,
    /// Up to three of the source's picklist values, as their labels.
    pub details: Vec<TimelineDetail>,
}

/// A picklist value an item shows, as text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TimelineDetail {
    pub field: String,
    pub label: String,
    pub value: String,
}

/// A source as the viewer may follow it to one record.
struct Followed {
    object: ObjectDefinition,
    through: Vec<FieldDefinition>,
    date: FieldDefinition,
    open: Option<OpenRule>,
    details: Vec<FieldDefinition>,
}

enum OpenRule {
    Equals(Box<FieldDefinition>, String),
    Upcoming,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    OpenDated,
    OpenUndated,
    PastDated,
    PastUndated,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Self::OpenDated => "open",
            Self::OpenUndated => "open_undated",
            Self::PastDated => "past",
            Self::PastUndated => "past_undated",
        }
    }

    fn is_open(self) -> bool {
        matches!(self, Self::OpenDated | Self::OpenUndated)
    }
}

/// Where each past stream stands: absent once read to its end, `None`
/// before its first record, or its query's cursor.
#[derive(Serialize, Deserialize, Default)]
struct PastCursor {
    s: BTreeMap<String, Option<String>>,
}

fn invalid_cursor() -> ObjectError {
    ObjectError::invalid(
        "cursor",
        TIMELINE_CURSOR_INVALID,
        "this cursor belongs to another timeline or has expired; reload the timeline",
    )
}

fn stream_key(object: &ObjectDefinition, kind: Kind) -> String {
    format!("{}/{}", object.id, kind.name())
}

/// When a row sorts: its date from midnight UTC, or its creation.
fn sort_instant(row: &Map<String, Value>, key: &str) -> Option<DateTime<Utc>> {
    let text = row.get(key)?.as_str()?;
    if let Ok(time) = DateTime::parse_from_rfc3339(text) {
        return Some(time.with_timezone(&Utc));
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d")
        .ok()
        .and_then(|date| date.and_hms_opt(0, 0, 0))
        .map(|time| time.and_utc())
}

impl Followed {
    /// The query for one kind of this source's records pointing at
    /// `public_id`, or `None` when the kind cannot occur.
    fn query(&self, kind: Kind, public_id: &str) -> Option<String> {
        let id = literal(public_id);
        let through = self
            .through
            .iter()
            .map(|field| format!("{} = {id}", field.api_name))
            .collect::<Vec<_>>()
            .join(" OR ");
        let date = &self.date.api_name;
        let open = match (&self.open, kind.is_open()) {
            (Some(OpenRule::Equals(field, value)), true) => {
                format!(" AND {} = {}", field.api_name, literal(value))
            }
            (Some(OpenRule::Equals(field, value)), false) => {
                format!(" AND {} != {}", field.api_name, literal(value))
            }
            (Some(OpenRule::Upcoming), true) => format!(" AND {date} >= TODAY"),
            (Some(OpenRule::Upcoming), false) => String::new(),
            (None, true) => return None,
            (None, false) => String::new(),
        };
        let (presence, order) = match kind {
            Kind::OpenDated => (format!("{date} != null"), format!("{date} ASC")),
            Kind::OpenUndated => {
                // An upcoming record has a date by definition.
                if matches!(self.open, Some(OpenRule::Upcoming)) {
                    return None;
                }
                (format!("{date} = null"), "created_at ASC".to_owned())
            }
            Kind::PastDated => {
                let presence = if matches!(self.open, Some(OpenRule::Upcoming)) {
                    format!("{date} < TODAY")
                } else {
                    format!("{date} != null")
                };
                (presence, format!("{date} DESC"))
            }
            Kind::PastUndated => (format!("{date} = null"), "created_at DESC".to_owned()),
        };
        let mut columns = vec!["name".to_owned(), "created_at".to_owned(), date.clone()];
        for field in self.through.iter().chain(&self.details) {
            if !columns.contains(&field.api_name) {
                columns.push(field.api_name.clone());
            }
        }
        Some(format!(
            "SELECT {} FROM {} WHERE ({through}){open} AND {presence} ORDER BY {order}",
            columns.join(", "),
            self.object.api_name,
        ))
    }

    fn item(&self, row: &Map<String, Value>, open: bool, public_id: &str) -> TimelineItem {
        let text = |key: &str| row.get(key).and_then(Value::as_str).map(str::to_owned);
        let through = self
            .through
            .iter()
            .find(|field| row.get(&field.api_name).and_then(Value::as_str) == Some(public_id))
            .or_else(|| self.through.first())
            .map(|field| field.api_name.clone())
            .unwrap_or_default();
        let details = self
            .details
            .iter()
            .filter_map(|field| {
                let value = row.get(&field.api_name)?.as_str()?;
                let label = field
                    .options
                    .iter()
                    .find(|option| option.value == value)
                    .map_or(value, |option| option.label.as_str());
                Some(TimelineDetail {
                    field: field.api_name.clone(),
                    label: field.label.clone(),
                    value: label.to_owned(),
                })
            })
            .collect();
        TimelineItem {
            object: self.object.api_name.clone(),
            object_label: self.object.label.clone(),
            id: text("id").unwrap_or_default(),
            name: text("name").unwrap_or_default(),
            date: text(&self.date.api_name),
            date_field: self.date.api_name.clone(),
            date_type: self.date.field_type,
            created_at: row.get("created_at").cloned().unwrap_or(Value::Null),
            open,
            through,
            details,
        }
    }

    /// The key a row sorts by in its group.
    fn instant(&self, kind: Kind, row: &Map<String, Value>) -> Option<DateTime<Utc>> {
        match kind {
            Kind::OpenDated | Kind::PastDated => sort_instant(row, &self.date.api_name),
            Kind::OpenUndated | Kind::PastUndated => sort_instant(row, "created_at"),
        }
    }
}

/// A row read from one stream, with what orders it.
struct Read {
    source: usize,
    kind: Kind,
    position: usize,
    instant: Option<DateTime<Utc>>,
    row: Map<String, Value>,
}

impl ObjectService {
    /// Brings the timeline source `object` keeps in line with its
    /// declaration, in the installation's transaction: a declaration is
    /// kept or replaced, and a dropped one goes.
    pub(crate) async fn put_plugin_timeline(
        &self,
        tx: &mut dyn ObjectTx,
        plugin_id: &str,
        object: &ObjectDefinition,
        declared: &ObjectDecl,
    ) -> Result<(), ObjectError> {
        let existing = tx
            .timeline_sources()
            .await?
            .into_iter()
            .find(|source| source.object_id == object.id);
        let Some(timeline) = &declared.timeline else {
            if existing.is_some() {
                tx.delete_timeline_source(object.id).await?;
            }
            return Ok(());
        };
        let field = |short: &str| {
            object
                .field(&format!("{plugin_id}:{short}"))
                .map(|field| field.id)
                .ok_or(ObjectError::Invariant)
        };
        let through = timeline
            .through
            .iter()
            .map(|short| field(short))
            .collect::<Result<Vec<_>, _>>()?;
        let open = match &timeline.open {
            None => None,
            Some(TimelineOpenDecl::Keyword(_)) => Some(TimelineOpen::Upcoming),
            Some(TimelineOpenDecl::Field {
                field: short,
                equals,
            }) => Some(TimelineOpen::FieldEquals {
                field_id: field(short)?,
                value: equals.clone(),
            }),
        };
        let now = Utc::now();
        let source = TimelineSource {
            object_id: object.id,
            owner_plugin: plugin_id.to_owned(),
            through,
            date_field_id: field(&timeline.date)?,
            open,
            created_at: existing.as_ref().map_or(now, |known| known.created_at),
            updated_at: now,
        };
        let unchanged = existing.is_some_and(|known| {
            known.through == source.through
                && known.date_field_id == source.date_field_id
                && known.open == source.open
                && known.owner_plugin == source.owner_plugin
        });
        if !unchanged {
            tx.put_timeline_source(&source).await?;
        }
        Ok(())
    }

    /// The sources the viewer can follow to records of `target`.
    async fn followed_sources(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        target: &ObjectDefinition,
    ) -> Result<Vec<Followed>, ObjectError> {
        let sources = tx.timeline_sources().await?;
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        // Related-to fields keep values to an object taken out of their
        // allowlist; those values still reach the record.
        let holding: BTreeSet<Uuid> = tx
            .related_to_fields_holding(target.id)
            .await?
            .into_iter()
            .map(|field| field.id)
            .collect();
        let mut followed = Vec::new();
        for source in sources {
            let Some(object) = tx
                .object_by_id(source.object_id)
                .await?
                .filter(|object| object.is_active() && access.for_object(object.id).read)
            else {
                continue;
            };
            let by_id = |id: Uuid| {
                object
                    .fields
                    .iter()
                    .find(|field| field.id == id && field.is_active())
                    .cloned()
            };
            let through: Vec<_> = source
                .through
                .iter()
                .filter_map(|id| by_id(*id))
                .filter(|field| match field.field_type {
                    FieldType::Lookup | FieldType::MasterDetail => {
                        field.reference_object_id == Some(target.id)
                    }
                    FieldType::RelatedTo => {
                        field.reference_object_ids.contains(&target.id)
                            || holding.contains(&field.id)
                    }
                    _ => false,
                })
                .collect();
            let Some(date) = by_id(source.date_field_id)
                .filter(|field| matches!(field.field_type, FieldType::Date | FieldType::DateTime))
            else {
                continue;
            };
            if through.is_empty() {
                continue;
            }
            let open = match &source.open {
                None => None,
                Some(TimelineOpen::Upcoming) => Some(OpenRule::Upcoming),
                Some(TimelineOpen::FieldEquals { field_id, value }) => {
                    by_id(*field_id).map(|field| OpenRule::Equals(Box::new(field), value.clone()))
                }
            };
            let mut details: Vec<_> = object
                .fields
                .iter()
                .filter(|field| field.is_active() && field.field_type == FieldType::Picklist)
                .cloned()
                .collect();
            details.sort_by_key(|field| field.position);
            details.truncate(TIMELINE_DETAILS_MAX);
            followed.push(Followed {
                object,
                through,
                date,
                open,
                details,
            });
        }
        followed.sort_by(|a, b| a.object.api_name.cmp(&b.object.api_name));
        Ok(followed)
    }

    /// Runs one stream's query as the viewer. A source whose query no
    /// longer runs is left out rather than failing the timeline; a cursor
    /// that does not fit its query is the caller's mistake.
    async fn timeline_page(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        q: String,
        cursor: Option<String>,
        page_size: u16,
    ) -> Result<Option<QueryResult>, ObjectError> {
        let request = QueryRequest {
            q,
            cursor,
            page_size: Some(page_size),
            include_deleted: false,
        };
        match self.query_in(tx, access, &request).await {
            Ok(result) => Ok(Some(result)),
            Err(ObjectError::Validation(issues))
                if issues.iter().any(|issue| issue.field == "cursor") =>
            {
                Err(invalid_cursor())
            }
            Err(ObjectError::Validation(issues)) => {
                tracing::warn!(
                    query = %request.q,
                    code = issues.first().map_or("", |issue| issue.code),
                    "timeline source skipped"
                );
                Ok(None)
            }
            Err(other) => Err(other),
        }
    }

    /// The Activity timeline of a record the viewer sees: open records on
    /// the first page, and a page of past records after `cursor`.
    pub async fn timeline(
        &self,
        api_name: &str,
        public_id: &str,
        cursor: Option<&str>,
    ) -> Result<Timeline, ObjectError> {
        let record = self.get_record(api_name, public_id, false).await?;
        let state = match cursor.filter(|cursor| !cursor.is_empty()) {
            None => None,
            Some(token) => {
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| invalid_cursor())?;
                Some(serde_json::from_slice::<PastCursor>(&bytes).map_err(|_| invalid_cursor())?)
            }
        };
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let sources = self.followed_sources(tx.as_mut(), &access, &object).await?;
        let mut timeline = Timeline {
            sources: sources.len(),
            open: Vec::new(),
            open_more: false,
            past: Vec::new(),
            next_cursor: None,
            actions: Vec::new(),
        };
        if sources.is_empty() {
            tx.commit().await?;
            return Ok(timeline);
        }
        let in_timeline: BTreeSet<Uuid> = sources.iter().map(|source| source.object.id).collect();
        let mut actions = tx.quick_actions(object.id).await?;
        actions.sort_by(|a, b| (a.position, &a.name).cmp(&(b.position, &b.name)));
        timeline.actions = actions
            .into_iter()
            .filter(|action| {
                action.kind == QuickActionKind::Create
                    && action
                        .target_object_id
                        .is_some_and(|target| in_timeline.contains(&target))
            })
            .map(|action| action.name)
            .collect();

        if state.is_none() {
            let (open, more) = self
                .open_group(tx.as_mut(), &access, &sources, &record.public_id)
                .await?;
            timeline.open = open;
            timeline.open_more = more;
        }
        let state = state.unwrap_or_else(|| PastCursor {
            s: sources
                .iter()
                .flat_map(|source| {
                    [Kind::PastDated, Kind::PastUndated]
                        .into_iter()
                        .map(|kind| (stream_key(&source.object, kind), None))
                })
                .collect(),
        });
        let (past, next) = self
            .past_page(tx.as_mut(), &access, &sources, &record.public_id, state)
            .await?;
        timeline.past = past;
        timeline.next_cursor = next;
        tx.commit().await?;
        Ok(timeline)
    }

    async fn open_group(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        sources: &[Followed],
        public_id: &str,
    ) -> Result<(Vec<TimelineItem>, bool), ObjectError> {
        let mut read = Vec::new();
        let mut more = false;
        for (index, source) in sources.iter().enumerate() {
            for kind in [Kind::OpenDated, Kind::OpenUndated] {
                let Some(q) = source.query(kind, public_id) else {
                    continue;
                };
                let Some(result) = self
                    .timeline_page(tx, access, q, None, TIMELINE_OPEN_MAX)
                    .await?
                else {
                    continue;
                };
                more |= result.next_cursor.is_some();
                for (position, row) in result.rows.into_iter().enumerate() {
                    read.push(Read {
                        source: index,
                        kind,
                        position,
                        instant: source.instant(kind, &row),
                        row,
                    });
                }
            }
        }
        // Dated first, soonest first; then undated, oldest first.
        read.sort_by(|a, b| {
            (a.kind != Kind::OpenDated)
                .cmp(&(b.kind != Kind::OpenDated))
                .then(a.instant.cmp(&b.instant))
                .then(a.source.cmp(&b.source))
                .then(a.position.cmp(&b.position))
        });
        let limit = usize::from(TIMELINE_OPEN_MAX);
        more |= read.len() > limit;
        read.truncate(limit);
        let items = read
            .iter()
            .map(|entry| sources[entry.source].item(&entry.row, true, public_id))
            .collect();
        Ok((items, more))
    }

    async fn past_page(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        sources: &[Followed],
        public_id: &str,
        state: PastCursor,
    ) -> Result<(Vec<TimelineItem>, Option<String>), ObjectError> {
        // Each live stream: its key, query, starting cursor, rows read and
        // where its query ends.
        struct Stream {
            key: String,
            q: String,
            from: Option<String>,
            read: usize,
            next: Option<String>,
        }
        let mut streams = Vec::new();
        let mut read = Vec::new();
        for (index, source) in sources.iter().enumerate() {
            for kind in [Kind::PastDated, Kind::PastUndated] {
                let key = stream_key(&source.object, kind);
                let Some(from) = state.s.get(&key) else {
                    continue;
                };
                let Some(q) = source.query(kind, public_id) else {
                    continue;
                };
                let Some(result) = self
                    .timeline_page(tx, access, q.clone(), from.clone(), TIMELINE_PAGE)
                    .await?
                else {
                    continue;
                };
                let stream = streams.len();
                streams.push(Stream {
                    key,
                    q,
                    from: from.clone(),
                    read: result.rows.len(),
                    next: result.next_cursor,
                });
                for (position, row) in result.rows.into_iter().enumerate() {
                    read.push((
                        stream,
                        Read {
                            source: index,
                            kind,
                            position,
                            instant: source.instant(kind, &row),
                            row,
                        },
                    ));
                }
            }
        }
        // Newest first; ties keep each stream's own order, so every stream
        // gives up a prefix of what it read.
        read.sort_by(|(a_stream, a), (b_stream, b)| {
            b.instant
                .cmp(&a.instant)
                .then(a_stream.cmp(b_stream))
                .then(a.position.cmp(&b.position))
        });
        read.truncate(usize::from(TIMELINE_PAGE));
        let mut taken = vec![0_usize; streams.len()];
        for (stream, _) in &read {
            taken[*stream] += 1;
        }
        let mut next = PastCursor::default();
        for (stream, taken) in streams.iter().zip(taken) {
            let position = if taken == stream.read {
                // Everything read was shown: on from where the query ends.
                stream.next.clone().map(Some)
            } else if taken == 0 {
                Some(stream.from.clone())
            } else {
                // The stream's own cursor after the last row shown: the
                // same query, as many rows as were taken.
                let page_size = u16::try_from(taken).map_err(|_| ObjectError::Invariant)?;
                self.timeline_page(tx, access, stream.q.clone(), stream.from.clone(), page_size)
                    .await?
                    .and_then(|result| result.next_cursor)
                    .map(Some)
            };
            if let Some(position) = position {
                next.s.insert(stream.key.clone(), position);
            }
        }
        let items = read
            .iter()
            .map(|(_, entry)| sources[entry.source].item(&entry.row, false, public_id))
            .collect();
        let cursor = if next.s.is_empty() {
            None
        } else {
            Some(
                URL_SAFE_NO_PAD
                    .encode(serde_json::to_vec(&next).map_err(|_| ObjectError::Invariant)?),
            )
        };
        Ok((items, cursor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_sort_from_midnight_utc() {
        let mut row = Map::new();
        row.insert("d".into(), Value::String("2026-11-10".into()));
        row.insert(
            "t".into(),
            Value::String("2026-11-10T09:30:00.000000Z".into()),
        );
        row.insert("n".into(), Value::Null);
        let date = sort_instant(&row, "d").expect("date");
        let time = sort_instant(&row, "t").expect("time");
        assert!(date < time);
        assert_eq!(date.to_rfc3339(), "2026-11-10T00:00:00+00:00");
        assert_eq!(sort_instant(&row, "n"), None);
        assert_eq!(sort_instant(&row, "missing"), None);
    }

    #[test]
    fn a_stale_cursor_is_refused() {
        assert!(URL_SAFE_NO_PAD.decode("not base64!").is_err());
        assert!(serde_json::from_slice::<PastCursor>(b"[]").is_err());
        let round: PastCursor =
            serde_json::from_slice(br#"{"s":{"a/past":null,"b/past":"x"}}"#).expect("cursor");
        assert_eq!(round.s.len(), 2);
    }
}
