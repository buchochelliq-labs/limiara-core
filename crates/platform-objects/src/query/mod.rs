//! The record query language (ADR 0019, #304).
//!
//! [`parse`] turns text into a syntax tree, [`plan`] resolves it against
//! metadata and the caller's access into a backend-neutral plan, and [`sql`]
//! renders plans for each store. [`ObjectService::query`] runs a query in one
//! workspace transaction and formats the rows.

pub mod parse;
pub mod plan;
pub mod sql;

use crate::service::ObjectService;
use crate::values::{index_values, name_key};
use crate::{
    FieldType, ObjectAccess, ObjectDefinition, ObjectError, ObjectTx, Record, RecordStatus,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use plan::{
    error, AggregateColumn, Catalog, Cell, CellKind, Column, Keyset, Leaf, OutputRef, Param,
    Predicate, RecordPlan, Resolved, Scope, SelectHop, Selection, SortKey, Source, Standard, Test,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Cost limits (ADR 0019 §5).
pub mod limits {
    pub const QUERY_LENGTH: usize = 10_000;
    pub const SELECT_ITEMS: usize = 100;
    pub const CONDITIONS: usize = 50;
    pub const IN_VALUES: usize = 200;
    pub const RELATIONSHIP_HOPS: usize = 3;
    pub const RELATIONSHIP_PATHS: usize = 10;
    pub const ORDER_KEYS: usize = 3;
    pub const GROUP_KEYS: usize = 3;
    pub const RECORD_LIMIT: u64 = 50_000;
    pub const GROUPS: u32 = 2_000;
    pub const PAGE_SIZE_DEFAULT: u16 = 200;
    pub const PAGE_SIZE_MAX: u16 = 200;
    /// PostgreSQL statement timeout for query statements.
    pub const STATEMENT_TIMEOUT_MS: u32 = 10_000;
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryRequest {
    pub q: String,
    pub cursor: Option<String>,
    pub page_size: Option<u16>,
    /// Include records in the recycle bin.
    pub include_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueryColumn {
    pub key: String,
    #[serde(rename = "type")]
    pub column_type: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QueryResult {
    pub object: String,
    pub columns: Vec<QueryColumn>,
    pub rows: Vec<Map<String, Value>>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct CursorToken {
    /// Binds the cursor to the query text and flags.
    h: String,
    v: Vec<Value>,
    i: Uuid,
    /// Rows `LIMIT` still allows.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    r: Option<u64>,
}

fn fingerprint(request: &QueryRequest) -> String {
    let mut digest = Sha256::new();
    digest.update(request.q.as_bytes());
    digest.update([0, u8::from(request.include_deleted)]);
    digest
        .finalize()
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn invalid_cursor() -> ObjectError {
    ObjectError::invalid(
        "cursor",
        "query.cursor_invalid",
        "this cursor belongs to another query or has been altered; run the query again",
    )
}

/// Truncated to microseconds, the precision every backend stores.
fn micros(time: DateTime<Utc>) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(time.timestamp_micros()).unwrap_or(time)
}

fn time_json(time: DateTime<Utc>) -> Value {
    Value::String(time.to_rfc3339_opts(SecondsFormat::Micros, true))
}

fn number_json(number: f64) -> Value {
    serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number)
}

/// The sort value a record has for one key, as the store compares it.
fn sort_value(record: &Record, leaf: &Leaf) -> Option<Param> {
    match leaf {
        Leaf::Standard(standard) => match standard {
            Standard::Id => Some(Param::Text(record.public_id.clone())),
            Standard::Name => Some(Param::Text(name_key(&record.name))),
            Standard::CreatedAt => Some(Param::Time(micros(record.created_at))),
            Standard::UpdatedAt => Some(Param::Time(micros(record.updated_at))),
            Standard::TouchedAt => Some(Param::Time(micros(record.touched_at))),
            Standard::DeletedAt => record.deleted_at.map(|time| Param::Time(micros(time))),
            Standard::Version => Some(Param::Int(record.version)),
            Standard::IsDeleted => Some(Param::Int(i64::from(record.deleted_at.is_some()))),
            Standard::Owner | Standard::CreatedBy | Standard::UpdatedBy => None,
        },
        Leaf::Field(field) => record
            .values
            .get(&field.api_name)
            .and_then(|value| index_values(&indexed_as(field), value).into_iter().next())
            .map(|value| match value {
                crate::IndexValue::Text(text) => Param::Text(text),
                crate::IndexValue::Number(number) => Param::Number(number),
                crate::IndexValue::Time(time) => Param::Time(micros(time)),
            }),
    }
}

/// A field as its index stores it: a stored formula or a roll-up summary
/// by its result type.
fn indexed_as(field: &crate::FieldDefinition) -> std::borrow::Cow<'_, crate::FieldDefinition> {
    field.as_value_field()
}

fn param_json(param: Option<Param>) -> Value {
    match param {
        None => Value::Null,
        Some(Param::Text(text)) => Value::String(text),
        Some(Param::Number(number)) => number_json(number),
        Some(Param::Int(number)) => Value::from(number),
        Some(Param::Time(time)) => time_json(time),
        Some(Param::Uuid(id)) => Value::String(id.to_string()),
    }
}

fn param_from_json(value: &Value, kind: CellKind) -> Result<Option<Param>, ObjectError> {
    if value.is_null() {
        return Ok(None);
    }
    let param = match kind {
        CellKind::Text => value.as_str().map(|text| Param::Text(text.to_owned())),
        CellKind::Number => value.as_f64().map(Param::Number),
        CellKind::Int => value.as_i64().map(Param::Int),
        CellKind::Time => value
            .as_str()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|time| Param::Time(time.with_timezone(&Utc))),
        CellKind::Uuid => value
            .as_str()
            .and_then(|text| Uuid::parse_str(text).ok())
            .map(Param::Uuid),
    };
    param.map(Some).ok_or_else(invalid_cursor)
}

fn standard_json(record: &Record, standard: Standard) -> Value {
    let user = |id: Option<Uuid>| id.map_or(Value::Null, |id| Value::String(id.to_string()));
    match standard {
        Standard::Id => Value::String(record.public_id.clone()),
        Standard::Name => Value::String(record.name.clone()),
        Standard::Owner => user(record.owner_id),
        Standard::CreatedAt => time_json(record.created_at),
        Standard::CreatedBy => user(record.created_by),
        Standard::UpdatedAt => time_json(record.updated_at),
        Standard::UpdatedBy => user(record.updated_by),
        Standard::TouchedAt => time_json(record.touched_at),
        Standard::Version => Value::from(record.version),
        Standard::IsDeleted => Value::Bool(record.deleted_at.is_some()),
        Standard::DeletedAt => record.deleted_at.map_or(Value::Null, time_json),
    }
}

fn leaf_json(record: &Record, leaf: &Leaf) -> Value {
    match leaf {
        Leaf::Standard(standard) => standard_json(record, *standard),
        Leaf::Field(field) => record
            .values
            .get(&field.api_name)
            .cloned()
            .unwrap_or(Value::Null),
    }
}

fn cell_json(cell: Cell, leaf: Option<&Leaf>) -> Value {
    let field_type = match leaf {
        Some(Leaf::Field(field)) => Some(indexed_as(field).field_type),
        _ => None,
    };
    match cell {
        Cell::Null => Value::Null,
        Cell::Text(text) if field_type == Some(FieldType::Boolean) => Value::Bool(text == "true"),
        Cell::Text(text) => Value::String(text),
        Cell::Number(number) => number_json(number),
        Cell::Int(number) if matches!(leaf, Some(Leaf::Standard(Standard::IsDeleted))) => {
            Value::Bool(number != 0)
        }
        Cell::Int(number) => Value::from(number),
        Cell::Time(time) if field_type == Some(FieldType::Date) => {
            Value::String(time.date_naive().format("%Y-%m-%d").to_string())
        }
        Cell::Time(time) => time_json(time),
        Cell::Uuid(id) => Value::String(id.to_string()),
    }
}

/// Every path a query names, for loading relationship targets.
fn paths(query: &parse::Query) -> Vec<&parse::Path> {
    fn walk<'q>(expr: &'q parse::Expr, out: &mut Vec<&'q parse::Path>) {
        match expr {
            parse::Expr::And(terms) | parse::Expr::Or(terms) => {
                terms.iter().for_each(|term| walk(term, out))
            }
            parse::Expr::Not(inner) => walk(inner, out),
            parse::Expr::Condition(condition) => out.push(&condition.path),
        }
    }
    let mut out: Vec<&parse::Path> = query
        .select
        .iter()
        .filter_map(|item| match item {
            parse::SelectItem::Path { path, .. } => Some(path),
            parse::SelectItem::Aggregate { .. } => None,
        })
        .collect();
    if let Some(filter) = &query.filter {
        walk(filter, &mut out);
    }
    out
}

/// Visible, active records of one object by public ID, in batches.
async fn visible_by_public_id(
    tx: &mut dyn ObjectTx,
    hop: &SelectHop,
    ids: Vec<String>,
) -> Result<Vec<Record>, ObjectError> {
    let mut found = Vec::new();
    for chunk in ids.chunks(limits::IN_VALUES) {
        let plan = RecordPlan {
            scope: Scope {
                object_id: hop.object_id,
                status: RecordStatus::Active,
                visibility: hop.visibility.clone(),
                predicate: Some(Predicate::Test {
                    hops: Vec::new(),
                    test: Test::In(
                        Source::Column(Column::PublicId),
                        chunk.iter().cloned().map(Param::Text).collect(),
                    ),
                }),
            },
            sort: vec![SortKey {
                source: Source::Column(Column::PublicId),
                descending: false,
            }],
            after: None,
            // At most IN_VALUES per chunk.
            fetch: chunk.len() as u32,
        };
        found.extend(tx.query_records(&plan).await?);
    }
    Ok(found)
}

/// Values of selected relationship paths, per row; `None` where a target is
/// missing or hidden.
/// Selected lookup targets, with their formula fields computed.
struct Related<'a> {
    service: &'a ObjectService,
    access: &'a crate::ObjectAccess,
    targets: &'a BTreeMap<Uuid, (ObjectDefinition, Option<crate::Visibility>)>,
}

async fn related_values(
    tx: &mut dyn ObjectTx,
    related: &Related<'_>,
    selections: &[Selection],
    records: &[Record],
) -> Result<BTreeMap<String, Vec<Value>>, ObjectError> {
    let mut cache: BTreeMap<(Uuid, String), Option<Record>> = BTreeMap::new();
    let mut values = BTreeMap::new();
    for selection in selections
        .iter()
        .filter(|selection| !selection.hops.is_empty())
    {
        let mut current: Vec<Option<Record>> = records.iter().cloned().map(Some).collect();
        for hop in &selection.hops {
            let ids: Vec<Option<String>> = current
                .iter()
                .map(|record| {
                    record
                        .as_ref()
                        .and_then(|record| record.values.get(&hop.field_api_name))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            let mut missing: Vec<String> = ids
                .iter()
                .flatten()
                .filter(|id| !cache.contains_key(&(hop.object_id, (*id).clone())))
                .cloned()
                .collect();
            missing.sort();
            missing.dedup();
            if !missing.is_empty() {
                let mut found = visible_by_public_id(tx, hop, missing.clone()).await?;
                if let Some((object, _)) = related.targets.get(&hop.object_id) {
                    related
                        .service
                        .compute_formulas(tx, related.access, object, &mut found)
                        .await?;
                }
                for id in missing {
                    cache.insert((hop.object_id, id), None);
                }
                for record in found {
                    cache.insert((hop.object_id, record.public_id.clone()), Some(record));
                }
            }
            current = ids
                .into_iter()
                .map(|id| id.and_then(|id| cache.get(&(hop.object_id, id)).cloned().flatten()))
                .collect();
        }
        values.insert(
            selection.key.clone(),
            current
                .iter()
                .map(|record| {
                    record
                        .as_ref()
                        .map_or(Value::Null, |record| leaf_json(record, &selection.leaf))
                })
                .collect(),
        );
    }
    Ok(values)
}

/// Whether text is one condition, as written after WHERE, and nothing
/// more: a lookup filter (ADR 0028) is wrapped into a query, so it must not
/// carry its own ordering, grouping or limit.
pub(crate) fn is_condition(condition: &str) -> bool {
    parse::parse(&format!("SELECT id FROM custom:x WHERE ({condition})")).is_ok_and(|query| {
        query.filter.is_some()
            && query.group_by.is_empty()
            && query.order_by.is_empty()
            && query.limit.is_none()
    })
}

impl ObjectService {
    /// Runs a query as the caller (ADR 0019).
    pub async fn query(&self, request: QueryRequest) -> Result<QueryResult, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let result = self.query_in(tx.as_mut(), &access, &request).await?;
        tx.commit().await?;
        Ok(result)
    }

    /// Runs a query as the caller inside a transaction already open, with
    /// the caller's access: lookup filters and related lists (ADR 0028)
    /// read records as the caller would through `query`.
    pub(crate) async fn query_in(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        request: &QueryRequest,
    ) -> Result<QueryResult, ObjectError> {
        if request.q.chars().count() > limits::QUERY_LENGTH {
            return Err(error(
                "query.too_complex",
                limits::QUERY_LENGTH,
                format!("use at most {} characters", limits::QUERY_LENGTH),
            ));
        }
        let page_size = request.page_size.unwrap_or(limits::PAGE_SIZE_DEFAULT);
        if !(1..=limits::PAGE_SIZE_MAX).contains(&page_size) {
            return Err(ObjectError::invalid(
                "page_size",
                "query.page_size_invalid",
                format!("use 1 to {}", limits::PAGE_SIZE_MAX),
            ));
        }
        let query = parse::parse(&request.q)
            .map_err(|problem| error("query.syntax", problem.position, problem.message))?;
        let unknown_object = || {
            error(
                "query.object_unknown",
                query.from.position,
                format!("there is no object {} you can read", query.from.text()),
            )
        };
        if query.from.segments.len() != 1 {
            return Err(unknown_object());
        }
        let base = tx
            .object_by_api_name(&query.from.segments[0])
            .await?
            .filter(ObjectDefinition::is_active)
            .ok_or_else(unknown_object)?;
        let base_flags = access.for_object(base.id);
        if !base_flags.read {
            return Err(unknown_object());
        }
        // Load every object a relationship path passes through.
        let mut targets = BTreeMap::new();
        for path in paths(&query) {
            let mut object = base.clone();
            for segment in &path.segments[..path.segments.len().saturating_sub(1)] {
                let Some(target_id) = object
                    .field(segment)
                    .filter(|field| field.field_type.is_reference())
                    .and_then(|field| field.reference_object_id)
                else {
                    break;
                };
                if let Entry::Vacant(entry) = targets.entry(target_id) {
                    let Some(target) = tx
                        .object_by_id(target_id)
                        .await?
                        .filter(ObjectDefinition::is_active)
                    else {
                        break;
                    };
                    let flags = access.for_object(target.id);
                    if !flags.read {
                        break;
                    }
                    let visibility = self.visibility_of(&mut *tx, access, &target).await?;
                    entry.insert((target, visibility));
                }
                object = targets[&target_id].0.clone();
            }
        }
        let mut stored_formulas = crate::formula_fields::stored_formula_ids(&base);
        for (target, _) in targets.values() {
            stored_formulas.extend(crate::formula_fields::stored_formula_ids(target));
        }
        // Formula fields created before their values were stored have no
        // index rows yet: build them once, before the query relies on them.
        for object in std::iter::once(&base).chain(targets.values().map(|(target, _)| target)) {
            if !crate::formula_fields::stored_formula_ids(object).is_empty()
                && !tx.formulas_indexed(object.id).await?
            {
                crate::service::reindex(&mut *tx, object).await?;
            }
        }
        let catalog = Catalog {
            base: &base,
            base_visibility: self.visibility_of(&mut *tx, access, &base).await?,
            targets: &targets,
            now: Utc::now(),
            include_deleted: request.include_deleted,
            stored_formulas: &stored_formulas,
        };
        let resolved = plan::resolve(&query, &catalog)?;
        let result = match resolved {
            Resolved::Records {
                selections,
                scope,
                sort,
                sort_leaves,
                limit,
            } => {
                self.run_record_query(
                    &mut *tx,
                    &Related {
                        service: self,
                        access,
                        targets: &targets,
                    },
                    request,
                    &base,
                    page_size,
                    selections,
                    scope,
                    sort,
                    sort_leaves,
                    limit,
                )
                .await?
            }
            Resolved::Aggregate {
                columns,
                plan,
                explicit_limit,
            } => {
                let rows = tx.query_aggregate(&plan).await?;
                if !explicit_limit && rows.len() > limits::GROUPS as usize {
                    return Err(ObjectError::invalid(
                        "q",
                        "query.too_many_groups",
                        format!(
                            "the query has more than {} groups; filter it or add a LIMIT",
                            limits::GROUPS
                        ),
                    ));
                }
                aggregate_result(&base, columns, plan.groups.len(), rows)
            }
        };
        Ok(result)
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_record_query(
        &self,
        tx: &mut dyn ObjectTx,
        related: &Related<'_>,
        request: &QueryRequest,
        base: &ObjectDefinition,
        page_size: u16,
        selections: Vec<Selection>,
        scope: Scope,
        sort: Vec<SortKey>,
        sort_leaves: Vec<Leaf>,
        limit: Option<u64>,
    ) -> Result<QueryResult, ObjectError> {
        let fingerprint = fingerprint(request);
        let (after, remaining) = match request.cursor.as_deref() {
            None | Some("") => (None, limit),
            Some(token) => {
                let bytes = URL_SAFE_NO_PAD
                    .decode(token)
                    .map_err(|_| invalid_cursor())?;
                let token: CursorToken =
                    serde_json::from_slice(&bytes).map_err(|_| invalid_cursor())?;
                if token.h != fingerprint || token.v.len() != sort.len() {
                    return Err(invalid_cursor());
                }
                let values = token
                    .v
                    .iter()
                    .zip(&sort)
                    .map(|(value, key)| param_from_json(value, key.source.kind()))
                    .collect::<Result<Vec<_>, _>>()?;
                (
                    Some(Keyset {
                        values,
                        id: token.i,
                    }),
                    token.r,
                )
            }
        };
        let page = remaining.map_or(u64::from(page_size), |remaining| {
            remaining.min(u64::from(page_size))
        });
        let plan = RecordPlan {
            scope,
            sort,
            after,
            // At most PAGE_SIZE_MAX + 1.
            fetch: page as u32 + 1,
        };
        let mut records = tx.query_records(&plan).await?;
        let more = records.len() as u64 > page;
        records.truncate(page as usize);
        let remaining = remaining.map(|remaining| remaining - records.len() as u64);
        // Formula values are computed first, so a cursor sorted on a stored
        // formula records its value rather than a blank.
        self.compute_formulas(tx, related.access, base, &mut records)
            .await?;
        let next_cursor = match records.last() {
            Some(last) if more && remaining != Some(0) => {
                let token = CursorToken {
                    h: fingerprint,
                    v: sort_leaves
                        .iter()
                        .map(|leaf| param_json(sort_value(last, leaf)))
                        .collect(),
                    i: last.id,
                    r: remaining,
                };
                Some(
                    URL_SAFE_NO_PAD
                        .encode(serde_json::to_vec(&token).map_err(|_| ObjectError::Invariant)?),
                )
            }
            _ => None,
        };
        let related = related_values(tx, related, &selections, &records).await?;
        let rows = records
            .iter()
            .enumerate()
            .map(|(index, record)| {
                let mut row = Map::new();
                row.insert("id".into(), Value::String(record.public_id.clone()));
                for selection in &selections {
                    let value = if selection.hops.is_empty() {
                        leaf_json(record, &selection.leaf)
                    } else {
                        related
                            .get(&selection.key)
                            .and_then(|values| values.get(index))
                            .cloned()
                            .unwrap_or(Value::Null)
                    };
                    row.insert(selection.key.clone(), value);
                }
                row
            })
            .collect();
        Ok(QueryResult {
            object: base.api_name.clone(),
            columns: selections
                .iter()
                .map(|selection| QueryColumn {
                    key: selection.key.clone(),
                    column_type: selection.leaf.column_type(),
                })
                .collect(),
            rows,
            next_cursor,
        })
    }
}

fn aggregate_result(
    base: &ObjectDefinition,
    columns: Vec<AggregateColumn>,
    groups: usize,
    rows: Vec<Vec<Cell>>,
) -> QueryResult {
    let rows = rows
        .into_iter()
        .map(|cells| {
            let mut row = Map::new();
            for column in &columns {
                let index = match column.output {
                    OutputRef::Group(index) => index,
                    OutputRef::Aggregate(index) => groups + index,
                };
                let cell = cells.get(index).cloned().unwrap_or(Cell::Null);
                row.insert(column.key.clone(), cell_json(cell, column.leaf.as_ref()));
            }
            row
        })
        .collect();
    QueryResult {
        object: base.api_name.clone(),
        columns: columns
            .iter()
            .map(|column| QueryColumn {
                key: column.key.clone(),
                column_type: column.column_type,
            })
            .collect(),
        rows,
        next_cursor: None,
    }
}
