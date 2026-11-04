//! Resolves a parsed query against object metadata into a backend-neutral
//! plan (ADR 0019 §4). Every name becomes a field id or a standard column,
//! every literal a typed parameter, every relative date a range, and every
//! relationship hop carries the target object's visibility. Stores render
//! the plan to SQL; nothing from the query text reaches SQL text.

use super::limits;
use super::parse::{
    Aggregate, AggregateCall, Comparison, Condition, Expr, Literal, Operator, OrderTarget, Path,
    Query, RelativeDate, SelectItem,
};
use crate::values::name_key;
use crate::{FieldDefinition, FieldType, ObjectDefinition, ObjectError, RecordStatus, Visibility};
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// A bound parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Text(String),
    Number(f64),
    Int(i64),
    Time(DateTime<Utc>),
    Uuid(Uuid),
}

/// A standard column of `records` (ADR 0014).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    PublicId,
    /// The lowercase name key names compare and sort by.
    NameKey,
    Owner,
    CreatedAt,
    CreatedBy,
    UpdatedAt,
    UpdatedBy,
    TouchedAt,
    Version,
    DeletedAt,
    /// `deleted_at IS NOT NULL` as 0 or 1, for grouping and sorting.
    IsDeleted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PivotKind {
    Text,
    Number,
    Time,
}

/// Where a value comes from on one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Column(Column),
    /// A typed `record_index` row of an indexed field.
    Pivot {
        field_id: Uuid,
        kind: PivotKind,
    },
    /// A lookup's `record_references` row; compared by the target's public ID.
    Reference {
        field_id: Uuid,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A positive test: false for a record without a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Test {
    Compare(Source, Op, Param),
    /// `start <= value < end`.
    Range(Source, Param, Param),
    In(Source, Vec<Param>),
    /// A LIKE pattern with `\` as the escape character.
    Like(Source, String),
    HasValue(Source),
}

/// One relationship hop: through a lookup to a visible, active target.
#[derive(Debug, Clone, PartialEq)]
pub struct Hop {
    pub field_id: Uuid,
    pub object_id: Uuid,
    /// `None` when every record of the target object is visible.
    pub visibility: Option<Visibility>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Predicate {
    And(Vec<Predicate>),
    Or(Vec<Predicate>),
    Not(Box<Predicate>),
    Test { hops: Vec<Hop>, test: Test },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SortKey {
    pub source: Source,
    pub descending: bool,
}

/// What every query shares: one object's rows, as the caller sees them.
#[derive(Debug, Clone, PartialEq)]
pub struct Scope {
    pub object_id: Uuid,
    pub status: RecordStatus,
    pub visibility: Option<Visibility>,
    pub predicate: Option<Predicate>,
}

/// A keyset position: the sort values of the last row, then its id.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyset {
    pub values: Vec<Option<Param>>,
    pub id: Uuid,
}

/// Records in order. The store returns up to `fetch` rows.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordPlan {
    pub scope: Scope,
    pub sort: Vec<SortKey>,
    pub after: Option<Keyset>,
    pub fetch: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AggregateExpr {
    pub function: Aggregate,
    pub source: Option<Source>,
}

/// A result column of an aggregate query: a group key or an aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRef {
    Group(usize),
    Aggregate(usize),
}

/// The type a store decodes a result cell as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellKind {
    Text,
    Number,
    Int,
    Time,
    Uuid,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Text(String),
    Number(f64),
    Int(i64),
    Time(DateTime<Utc>),
    Uuid(Uuid),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AggregatePlan {
    pub scope: Scope,
    pub groups: Vec<Source>,
    pub aggregates: Vec<AggregateExpr>,
    pub order: Vec<(OutputRef, bool)>,
    pub fetch: u32,
}

impl Column {
    pub fn kind(self) -> CellKind {
        match self {
            Self::PublicId | Self::NameKey => CellKind::Text,
            Self::Owner | Self::CreatedBy | Self::UpdatedBy => CellKind::Uuid,
            Self::CreatedAt | Self::UpdatedAt | Self::TouchedAt | Self::DeletedAt => CellKind::Time,
            Self::Version | Self::IsDeleted => CellKind::Int,
        }
    }

    pub fn nullable(self) -> bool {
        matches!(
            self,
            Self::Owner | Self::CreatedBy | Self::UpdatedBy | Self::DeletedAt
        )
    }
}

impl Source {
    pub fn kind(self) -> CellKind {
        match self {
            Self::Column(column) => column.kind(),
            Self::Pivot { kind, .. } => match kind {
                PivotKind::Text => CellKind::Text,
                PivotKind::Number => CellKind::Number,
                PivotKind::Time => CellKind::Time,
            },
            Self::Reference { .. } => CellKind::Text,
        }
    }

    pub fn nullable(self) -> bool {
        match self {
            Self::Column(column) => column.nullable(),
            Self::Pivot { .. } | Self::Reference { .. } => true,
        }
    }
}

impl AggregateExpr {
    pub fn kind(self) -> CellKind {
        match (self.function, self.source) {
            (Aggregate::Count, _) => CellKind::Int,
            (Aggregate::Sum | Aggregate::Avg, _) => CellKind::Number,
            (_, Some(source)) => source.kind(),
            (_, None) => CellKind::Int,
        }
    }
}

impl AggregatePlan {
    /// The kind of each result column, groups first.
    pub fn output_kinds(&self) -> Vec<CellKind> {
        self.groups
            .iter()
            .map(|source| source.kind())
            .chain(self.aggregates.iter().map(|aggregate| aggregate.kind()))
            .collect()
    }
}

/// A standard field (ADR 0014) as the language names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standard {
    Id,
    Name,
    Owner,
    CreatedAt,
    CreatedBy,
    UpdatedAt,
    UpdatedBy,
    TouchedAt,
    Version,
    IsDeleted,
    DeletedAt,
}

impl Standard {
    pub const ALL: [Standard; 11] = [
        Self::Id,
        Self::Name,
        Self::Owner,
        Self::CreatedAt,
        Self::CreatedBy,
        Self::UpdatedAt,
        Self::UpdatedBy,
        Self::TouchedAt,
        Self::Version,
        Self::IsDeleted,
        Self::DeletedAt,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Name => "name",
            Self::Owner => "owner",
            Self::CreatedAt => "created_at",
            Self::CreatedBy => "created_by",
            Self::UpdatedAt => "updated_at",
            Self::UpdatedBy => "updated_by",
            Self::TouchedAt => "touched_at",
            Self::Version => "version",
            Self::IsDeleted => "is_deleted",
            Self::DeletedAt => "deleted_at",
        }
    }

    pub(crate) fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|standard| standard.as_str() == name)
    }

    /// The column type reported to clients.
    pub fn column_type(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Name => "text",
            Self::Owner | Self::CreatedBy | Self::UpdatedBy => "user",
            Self::CreatedAt | Self::UpdatedAt | Self::TouchedAt | Self::DeletedAt => "date_time",
            Self::Version => "number",
            Self::IsDeleted => "boolean",
        }
    }

    pub fn column(self) -> Column {
        match self {
            Self::Id => Column::PublicId,
            Self::Name => Column::NameKey,
            Self::Owner => Column::Owner,
            Self::CreatedAt => Column::CreatedAt,
            Self::CreatedBy => Column::CreatedBy,
            Self::UpdatedAt => Column::UpdatedAt,
            Self::UpdatedBy => Column::UpdatedBy,
            Self::TouchedAt => Column::TouchedAt,
            Self::Version => Column::Version,
            Self::IsDeleted => Column::IsDeleted,
            Self::DeletedAt => Column::DeletedAt,
        }
    }
}

/// The end of a path: a standard field or a custom field.
#[derive(Debug, Clone, PartialEq)]
pub enum Leaf {
    Standard(Standard),
    Field(Box<FieldDefinition>),
}

impl Leaf {
    pub fn column_type(&self) -> &'static str {
        match self {
            Self::Standard(standard) => standard.column_type(),
            Self::Field(field) => field.formula_type.unwrap_or(field.field_type).as_str(),
        }
    }
}

/// A selected relationship hop, for resolving values after the query.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectHop {
    pub field_api_name: String,
    pub object_id: Uuid,
    pub visibility: Option<Visibility>,
}

/// One selected value of a record query.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    pub key: String,
    pub hops: Vec<SelectHop>,
    pub leaf: Leaf,
}

/// One result column of an aggregate query.
#[derive(Debug, Clone, PartialEq)]
pub struct AggregateColumn {
    pub key: String,
    pub column_type: &'static str,
    pub output: OutputRef,
    /// The field a group or MIN/MAX value belongs to, for formatting.
    pub leaf: Option<Leaf>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Records {
        selections: Vec<Selection>,
        scope: Scope,
        sort: Vec<SortKey>,
        /// The field behind each sort key, to compute cursor values.
        sort_leaves: Vec<Leaf>,
        limit: Option<u64>,
    },
    Aggregate {
        columns: Vec<AggregateColumn>,
        plan: AggregatePlan,
        explicit_limit: bool,
    },
}

/// What the resolver may consult.
pub struct Catalog<'a> {
    pub base: &'a ObjectDefinition,
    /// The base object's visibility; `None` shows every record.
    pub base_visibility: Option<Visibility>,
    /// Lookup targets the caller may read, with their visibility.
    pub targets: &'a BTreeMap<Uuid, (ObjectDefinition, Option<Visibility>)>,
    pub now: DateTime<Utc>,
    pub include_deleted: bool,
    /// Formula fields of the base object and targets whose values are
    /// stored, so they can be filtered, sorted and grouped (ADR 0021 §6).
    pub stored_formulas: &'a BTreeSet<Uuid>,
}

pub(crate) fn error(
    code: &'static str,
    position: usize,
    detail: impl std::fmt::Display,
) -> ObjectError {
    ObjectError::invalid("q", code, format!("{detail} (at position {position})"))
}

fn too_complex(position: usize, detail: impl std::fmt::Display) -> ObjectError {
    error("query.too_complex", position, detail)
}

/// A resolved path: relationship hops, then a leaf.
struct ResolvedPath<'c> {
    hops: Vec<(
        &'c FieldDefinition,
        &'c ObjectDefinition,
        Option<Visibility>,
    )>,
    leaf: Leaf,
}

impl<'a> Catalog<'a> {
    fn path(&self, path: &Path) -> Result<ResolvedPath<'_>, ObjectError> {
        if path.segments.len() > limits::RELATIONSHIP_HOPS + 1 {
            return Err(too_complex(
                path.position,
                format!("follow at most {} relationships", limits::RELATIONSHIP_HOPS),
            ));
        }
        let mut object: &ObjectDefinition = self.base;
        let mut hops = Vec::new();
        let (last, through) = path
            .segments
            .split_last()
            .ok_or_else(|| error("query.syntax", path.position, "expected a name"))?;
        for segment in through {
            let Some(field) = object.field(segment) else {
                return Err(error(
                    "query.field_unknown",
                    path.position,
                    format!("{} has no field {segment}", object.api_name),
                ));
            };
            if !field.field_type.is_reference() {
                return Err(error(
                    "query.relationship_unknown",
                    path.position,
                    format!("{segment} is not a lookup, so it cannot be followed"),
                ));
            }
            let Some((target, visibility)) = field
                .reference_object_id
                .and_then(|id| self.targets.get(&id))
            else {
                return Err(error(
                    "query.relationship_unknown",
                    path.position,
                    format!("{segment} points to an object you cannot read"),
                ));
            };
            hops.push((field, target, visibility.clone()));
            object = target;
        }
        let leaf = match Standard::parse(last) {
            Some(standard) => Leaf::Standard(standard),
            None => Leaf::Field(Box::new(object.field(last).cloned().ok_or_else(|| {
                error(
                    "query.field_unknown",
                    path.position,
                    format!("{} has no field {last}", object.api_name),
                )
            })?)),
        };
        Ok(ResolvedPath { hops, leaf })
    }

    fn base_path(&self, path: &Path, clause: &str) -> Result<Leaf, ObjectError> {
        if path.segments.len() > 1 {
            return Err(error(
                "query.relationship_unsupported",
                path.position,
                format!("{clause} cannot follow relationships yet"),
            ));
        }
        Ok(self.path(path)?.leaf)
    }
}

/// How a leaf compares, filters and sorts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Category {
    /// Case-insensitive text through its lowercase key.
    Text,
    /// Exact text: picklists and public IDs.
    Exact,
    Number,
    Int,
    Time,
    /// Booleans, indexed as `true` / `false`.
    Bool,
    /// `is_deleted`.
    Deleted,
    User,
    Reference,
    Multi,
}

impl Catalog<'_> {
    fn category(
        &self,
        leaf: &Leaf,
        position: usize,
        clause: &str,
    ) -> Result<(Category, Source), ObjectError> {
        category(leaf, position, clause, self.stored_formulas)
    }
}

fn category(
    leaf: &Leaf,
    position: usize,
    clause: &str,
    stored_formulas: &BTreeSet<Uuid>,
) -> Result<(Category, Source), ObjectError> {
    Ok(match leaf {
        Leaf::Standard(standard) => {
            let category = match standard {
                Standard::Id => Category::Exact,
                Standard::Name => Category::Text,
                Standard::Owner | Standard::CreatedBy | Standard::UpdatedBy => Category::User,
                Standard::CreatedAt
                | Standard::UpdatedAt
                | Standard::TouchedAt
                | Standard::DeletedAt => Category::Time,
                Standard::Version => Category::Int,
                Standard::IsDeleted => Category::Deleted,
            };
            (category, Source::Column(standard.column()))
        }
        Leaf::Field(field) => {
            if field.field_type.is_reference() {
                return Ok((
                    Category::Reference,
                    Source::Reference { field_id: field.id },
                ));
            }
            let field_type = if field.field_type == FieldType::Formula {
                if !stored_formulas.contains(&field.id) {
                    return Err(error(
                        "query.formula_not_filterable",
                        position,
                        format!(
                            "{} follows lookups or uses TODAY or NOW, so it is computed when read and cannot be used in {clause}",
                            field.api_name
                        ),
                    ));
                }
                field.formula_type.unwrap_or(FieldType::Text)
            } else if !field.is_indexed() {
                return Err(error(
                    "query.field_not_indexed",
                    position,
                    format!(
                        "{} is not indexed, so it cannot be used in {clause}; index it in the Object Manager",
                        field.api_name
                    ),
                ));
            } else {
                field.value_type()
            };
            let (category, kind) = match field_type {
                FieldType::Picklist => (Category::Exact, PivotKind::Text),
                FieldType::MultiPicklist => (Category::Multi, PivotKind::Text),
                FieldType::Boolean => (Category::Bool, PivotKind::Text),
                FieldType::Date | FieldType::DateTime => (Category::Time, PivotKind::Time),
                kind if kind.is_numeric() => (Category::Number, PivotKind::Number),
                _ => (Category::Text, PivotKind::Text),
            };
            (
                category,
                Source::Pivot {
                    field_id: field.id,
                    kind,
                },
            )
        }
    })
}

fn leaf_name(leaf: &Leaf) -> &str {
    match leaf {
        Leaf::Standard(standard) => standard.as_str(),
        Leaf::Field(field) => &field.api_name,
    }
}

/// A value to compare with: a point, or a period for dates.
enum Value {
    Point(Param),
    Period(Param, Param),
}

fn start_of(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(0, 0, 0)
        .map(|time| time.and_utc())
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

fn period(relative: RelativeDate, now: DateTime<Utc>) -> (DateTime<Utc>, DateTime<Utc>) {
    let today = now.date_naive();
    let day = |offset: i64| start_of(today) + Duration::days(offset);
    let monday = today - Duration::days(i64::from(today.weekday().num_days_from_monday()));
    let week = |offset: i64| start_of(monday) + Duration::weeks(offset);
    let month = |offset: i32| {
        let index = today.year() * 12 + today.month0() as i32 + offset;
        NaiveDate::from_ymd_opt(index.div_euclid(12), index.rem_euclid(12) as u32 + 1, 1)
            .map(start_of)
            .unwrap_or(DateTime::<Utc>::MAX_UTC)
    };
    match relative {
        RelativeDate::Today => (day(0), day(1)),
        RelativeDate::Yesterday => (day(-1), day(0)),
        RelativeDate::Tomorrow => (day(1), day(2)),
        RelativeDate::ThisWeek => (week(0), week(1)),
        RelativeDate::LastWeek => (week(-1), week(0)),
        RelativeDate::NextWeek => (week(1), week(2)),
        RelativeDate::ThisMonth => (month(0), month(1)),
        RelativeDate::LastMonth => (month(-1), month(0)),
        RelativeDate::NextMonth => (month(1), month(2)),
        RelativeDate::ThisYear => (
            month(-(today.month0() as i32)),
            month(12 - today.month0() as i32),
        ),
        RelativeDate::LastNDays(days) => (day(1 - i64::from(days)), day(1)),
        RelativeDate::NextNDays(days) => (day(1), day(1 + i64::from(days))),
    }
}

impl<'a> Catalog<'a> {
    fn value(
        &self,
        category: Category,
        leaf: &Leaf,
        literal: &Literal,
        position: usize,
    ) -> Result<Value, ObjectError> {
        let mismatch = || {
            let needs = match category {
                Category::Text | Category::Exact | Category::Reference | Category::Multi => "text",
                Category::Number | Category::Int => "a number",
                Category::Time => "a date or date-time",
                Category::Bool | Category::Deleted => "true or false",
                Category::User => "a user ID",
            };
            error(
                "query.value_invalid",
                position,
                format!(
                    "{} needs {needs}, not {}",
                    leaf_name(leaf),
                    literal.describe()
                ),
            )
        };
        Ok(Value::Point(match (category, literal) {
            (Category::Text, Literal::Text { value, .. }) => Param::Text(name_key(value)),
            (Category::Exact, Literal::Text { value, .. })
                if matches!(leaf, Leaf::Standard(Standard::Id)) =>
            {
                Param::Text(value.trim().to_ascii_lowercase())
            }
            (Category::Exact | Category::Multi, Literal::Text { value, .. }) => {
                Param::Text(value.clone())
            }
            (Category::Reference, Literal::Text { value, .. }) => {
                Param::Text(value.trim().to_ascii_lowercase())
            }
            (Category::Number, Literal::Number(number)) => Param::Number(*number),
            (Category::Int, Literal::Number(number)) if number.fract() == 0.0 => {
                // Whole and finite, checked by the lexer and the guard.
                Param::Int(*number as i64)
            }
            (Category::Bool, Literal::Bool(flag)) => Param::Text(flag.to_string()),
            (Category::User, Literal::Text { value, .. }) => {
                Param::Uuid(Uuid::parse_str(value.trim()).map_err(|_| mismatch())?)
            }
            (Category::Time, Literal::DateTime(time)) => Param::Time(*time),
            (Category::Time, Literal::Date(date)) => {
                let start = start_of(*date);
                return Ok(Value::Period(
                    Param::Time(start),
                    Param::Time(start + Duration::days(1)),
                ));
            }
            (Category::Time, Literal::Relative(relative)) => {
                let (start, end) = period(*relative, self.now);
                return Ok(Value::Period(Param::Time(start), Param::Time(end)));
            }
            _ => return Err(mismatch()),
        }))
    }

    fn condition(
        &self,
        condition: &Condition,
        count: &mut usize,
    ) -> Result<Predicate, ObjectError> {
        *count += 1;
        if *count > limits::CONDITIONS {
            return Err(too_complex(
                condition.position,
                format!("use at most {} conditions", limits::CONDITIONS),
            ));
        }
        let position = condition.position;
        let resolved = self.path(&condition.path)?;
        let hops: Vec<Hop> = resolved
            .hops
            .iter()
            .map(|(field, object, visibility)| Hop {
                field_id: field.id,
                object_id: object.id,
                visibility: visibility.clone(),
            })
            .collect();
        let leaf = &resolved.leaf;
        let (category, source) = self.category(leaf, position, "WHERE")?;
        let test = |test: Test| Predicate::Test {
            hops: hops.clone(),
            test,
        };
        let not = |predicate: Predicate| Predicate::Not(Box::new(predicate));
        let operator_invalid = |what: &str| {
            error(
                "query.operator_invalid",
                position,
                format!("{} cannot be compared with {what}", leaf_name(leaf)),
            )
        };
        let source = if category == Category::Deleted {
            Source::Column(Column::DeletedAt)
        } else {
            source
        };
        let equal = |value: Value| match value {
            Value::Point(param) => test(Test::Compare(source, Op::Eq, param)),
            Value::Period(start, end) => test(Test::Range(source, start, end)),
        };
        Ok(match &condition.operator {
            Operator::Compare(comparison, Literal::Null) => match comparison {
                Comparison::Eq => not(test(Test::HasValue(source))),
                Comparison::Ne => test(Test::HasValue(source)),
                _ => return Err(operator_invalid("null except by = and !=")),
            },
            Operator::Compare(comparison, literal) => {
                if category == Category::Deleted {
                    let Literal::Bool(deleted) = literal else {
                        return Err(error(
                            "query.value_invalid",
                            position,
                            format!("is_deleted needs true or false, not {}", literal.describe()),
                        ));
                    };
                    let positive = match comparison {
                        Comparison::Eq => *deleted,
                        Comparison::Ne => !*deleted,
                        _ => return Err(operator_invalid("<, <=, > or >=")),
                    };
                    let has = test(Test::HasValue(source));
                    return Ok(if positive { has } else { not(has) });
                }
                if category == Category::Multi {
                    return Err(operator_invalid("= or !=; use INCLUDES or EXCLUDES"));
                }
                let ordered = !matches!(
                    category,
                    Category::Bool | Category::User | Category::Reference
                );
                if !matches!(comparison, Comparison::Eq | Comparison::Ne) && !ordered {
                    return Err(operator_invalid("<, <=, > or >="));
                }
                let value = self.value(category, leaf, literal, position)?;
                match (comparison, value) {
                    (Comparison::Eq, value) => equal(value),
                    (Comparison::Ne, value) => not(equal(value)),
                    (comparison, Value::Point(param)) => {
                        let op = match comparison {
                            Comparison::Lt => Op::Lt,
                            Comparison::Le => Op::Le,
                            Comparison::Gt => Op::Gt,
                            _ => Op::Ge,
                        };
                        test(Test::Compare(source, op, param))
                    }
                    (comparison, Value::Period(start, end)) => match comparison {
                        Comparison::Lt => test(Test::Compare(source, Op::Lt, start)),
                        Comparison::Le => test(Test::Compare(source, Op::Lt, end)),
                        Comparison::Gt => test(Test::Compare(source, Op::Ge, end)),
                        _ => test(Test::Compare(source, Op::Ge, start)),
                    },
                }
            }
            Operator::In { negated, values } => {
                if matches!(
                    category,
                    Category::Deleted | Category::Bool | Category::Multi
                ) {
                    return Err(operator_invalid("IN"));
                }
                if values.len() > limits::IN_VALUES {
                    return Err(too_complex(
                        position,
                        format!("use at most {} values in IN", limits::IN_VALUES),
                    ));
                }
                let mut points = Vec::new();
                let mut periods = Vec::new();
                for literal in values {
                    if *literal == Literal::Null {
                        return Err(operator_invalid("null in IN; use = null"));
                    }
                    match self.value(category, leaf, literal, position)? {
                        Value::Point(param) => points.push(param),
                        period => periods.push(equal(period)),
                    }
                }
                let mut alternatives = periods;
                if !points.is_empty() {
                    alternatives.insert(0, test(Test::In(source, points)));
                }
                let positive = if alternatives.len() == 1 {
                    alternatives.remove(0)
                } else {
                    Predicate::Or(alternatives)
                };
                if *negated {
                    not(positive)
                } else {
                    positive
                }
            }
            Operator::Like { negated, pattern } => {
                if category != Category::Text {
                    return Err(operator_invalid("LIKE, which needs a text field"));
                }
                let Literal::Text { pattern, .. } = pattern else {
                    return Err(error(
                        "query.value_invalid",
                        position,
                        "LIKE needs a text pattern",
                    ));
                };
                let positive = test(Test::Like(source, pattern.to_lowercase()));
                if *negated {
                    not(positive)
                } else {
                    positive
                }
            }
            Operator::Includes { negated, values } => {
                if category != Category::Multi {
                    return Err(operator_invalid(
                        "INCLUDES or EXCLUDES, which need a multi-select picklist",
                    ));
                }
                if values.len() > limits::IN_VALUES {
                    return Err(too_complex(
                        position,
                        format!("use at most {} values", limits::IN_VALUES),
                    ));
                }
                let mut params = Vec::new();
                for literal in values {
                    match self.value(category, leaf, literal, position)? {
                        Value::Point(param) => params.push(param),
                        Value::Period(..) => return Err(operator_invalid("a date")),
                    }
                }
                let positive = test(Test::In(source, params));
                if *negated {
                    not(positive)
                } else {
                    positive
                }
            }
        })
    }

    fn expr(&self, expr: &Expr, count: &mut usize) -> Result<Predicate, ObjectError> {
        Ok(match expr {
            Expr::And(terms) => Predicate::And(
                terms
                    .iter()
                    .map(|term| self.expr(term, count))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Or(terms) => Predicate::Or(
                terms
                    .iter()
                    .map(|term| self.expr(term, count))
                    .collect::<Result<_, _>>()?,
            ),
            Expr::Not(inner) => Predicate::Not(Box::new(self.expr(inner, count)?)),
            Expr::Condition(condition) => self.condition(condition, count)?,
        })
    }

    fn sort_source(
        &self,
        leaf: &Leaf,
        position: usize,
        clause: &str,
    ) -> Result<Source, ObjectError> {
        let refuse = |what: &str| {
            error(
                "query.sort_invalid",
                position,
                format!("{} cannot be used in {clause}: {what}", leaf_name(leaf)),
            )
        };
        let (category, source) = self.category(leaf, position, clause)?;
        match category {
            Category::Multi => Err(refuse("multi-select picklists hold several values")),
            Category::Reference if clause == "ORDER BY" => Err(refuse(
                "sort by a field of the target, such as its name, once that is supported",
            )),
            Category::User if clause == "ORDER BY" => Err(refuse("user IDs have no useful order")),
            _ => match leaf {
                Leaf::Field(field) if field.field_type == FieldType::LongText => {
                    Err(refuse("long text cannot be sorted or grouped"))
                }
                _ => Ok(source),
            },
        }
    }

    fn scope(&self, query: &Query) -> Result<Scope, ObjectError> {
        let predicate = match &query.filter {
            Some(filter) => Some(self.expr(filter, &mut 0)?),
            None => None,
        };
        Ok(Scope {
            object_id: self.base.id,
            status: if self.include_deleted {
                RecordStatus::Any
            } else {
                RecordStatus::Active
            },
            visibility: self.base_visibility.clone(),
            predicate,
        })
    }
}

fn select_key(item: &SelectItem) -> String {
    match item {
        SelectItem::Path { path, alias } => alias.clone().unwrap_or_else(|| path.text()),
        SelectItem::Aggregate { call, alias } => alias.clone().unwrap_or_else(|| call.text()),
    }
}

fn item_position(item: &SelectItem) -> usize {
    match item {
        SelectItem::Path { path, .. } => path.position,
        SelectItem::Aggregate { call, .. } => call.position,
    }
}

/// Resolves a parsed query.
pub fn resolve(query: &Query, catalog: &Catalog<'_>) -> Result<Resolved, ObjectError> {
    if query.select.len() > limits::SELECT_ITEMS {
        return Err(too_complex(
            1,
            format!("select at most {} items", limits::SELECT_ITEMS),
        ));
    }
    let mut keys = BTreeSet::new();
    for item in &query.select {
        let key = select_key(item);
        if !keys.insert(key.clone()) {
            return Err(error(
                "query.select_duplicate",
                item_position(item),
                format!("{key} is selected twice; give one an alias"),
            ));
        }
    }
    // Selected and filtered relationships both cost joins.
    fn filtered<'q>(expr: &'q Expr, out: &mut Vec<&'q Path>) {
        match expr {
            Expr::And(terms) | Expr::Or(terms) => terms.iter().for_each(|term| filtered(term, out)),
            Expr::Not(inner) => filtered(inner, out),
            Expr::Condition(condition) => out.push(&condition.path),
        }
    }
    let mut paths: Vec<&Path> = query
        .select
        .iter()
        .filter_map(|item| match item {
            SelectItem::Path { path, .. } => Some(path),
            SelectItem::Aggregate { .. } => None,
        })
        .collect();
    if let Some(filter) = &query.filter {
        filtered(filter, &mut paths);
    }
    let relationships: BTreeSet<String> = paths
        .iter()
        .filter(|path| path.segments.len() > 1)
        .map(|path| path.segments[..path.segments.len() - 1].join("."))
        .collect();
    if relationships.len() > limits::RELATIONSHIP_PATHS {
        return Err(too_complex(
            1,
            format!("use at most {} relationships", limits::RELATIONSHIP_PATHS),
        ));
    }
    let aggregate = !query.group_by.is_empty()
        || query
            .select
            .iter()
            .any(|item| matches!(item, SelectItem::Aggregate { .. }));
    let scope = catalog.scope(query)?;
    if aggregate {
        resolve_aggregate(query, catalog, scope)
    } else {
        resolve_records(query, catalog, scope)
    }
}

fn resolve_records(
    query: &Query,
    catalog: &Catalog<'_>,
    scope: Scope,
) -> Result<Resolved, ObjectError> {
    let mut selections = Vec::new();
    for item in &query.select {
        let SelectItem::Path { path, alias } = item else {
            continue;
        };
        let key = alias.clone().unwrap_or_else(|| path.text());
        if key == "id" && path.text() != "id" {
            return Err(error(
                "query.select_duplicate",
                path.position,
                "id is reserved for the record's ID; choose another alias",
            ));
        }
        let resolved = catalog.path(path)?;
        selections.push(Selection {
            key,
            hops: resolved
                .hops
                .iter()
                .map(|(field, object, visibility)| SelectHop {
                    field_api_name: field.api_name.clone(),
                    object_id: object.id,
                    visibility: visibility.clone(),
                })
                .collect(),
            leaf: resolved.leaf,
        });
    }
    if query.order_by.len() > limits::ORDER_KEYS {
        return Err(too_complex(
            1,
            format!("sort by at most {} fields", limits::ORDER_KEYS),
        ));
    }
    let mut sort = Vec::new();
    let mut sort_leaves = Vec::new();
    for item in &query.order_by {
        let path = match &item.target {
            OrderTarget::Path(path) => path,
            OrderTarget::Aggregate(call) => {
                return Err(error(
                    "query.sort_invalid",
                    call.position,
                    "only aggregate queries can sort by an aggregate",
                ))
            }
        };
        // A single name may be an alias of a selected field.
        let aliased = (path.segments.len() == 1)
            .then(|| {
                query.select.iter().find_map(|selected| match selected {
                    SelectItem::Path {
                        path: selected,
                        alias: Some(alias),
                    } if *alias == path.segments[0] => Some(selected),
                    _ => None,
                })
            })
            .flatten();
        let path = aliased.unwrap_or(path);
        let leaf = catalog.base_path(path, "ORDER BY")?;
        let source = catalog.sort_source(&leaf, path.position, "ORDER BY")?;
        sort.push(SortKey {
            source,
            descending: item.descending,
        });
        sort_leaves.push(leaf);
    }
    if sort.is_empty() {
        sort.push(SortKey {
            source: Source::Column(Column::NameKey),
            descending: false,
        });
        sort_leaves.push(Leaf::Standard(Standard::Name));
    }
    let limit = match query.limit {
        Some((limit, position)) if limit > limits::RECORD_LIMIT => {
            return Err(too_complex(
                position,
                format!("LIMIT can be at most {}", limits::RECORD_LIMIT),
            ))
        }
        Some((limit, _)) => Some(limit),
        None => None,
    };
    Ok(Resolved::Records {
        selections,
        scope,
        sort,
        sort_leaves,
        limit,
    })
}

fn resolve_aggregate(
    query: &Query,
    catalog: &Catalog<'_>,
    scope: Scope,
) -> Result<Resolved, ObjectError> {
    if query.group_by.len() > limits::GROUP_KEYS {
        return Err(too_complex(
            1,
            format!("group by at most {} fields", limits::GROUP_KEYS),
        ));
    }
    let mut groups = Vec::new();
    let mut group_leaves = Vec::new();
    let mut group_texts = Vec::new();
    for path in &query.group_by {
        let leaf = catalog.base_path(path, "GROUP BY")?;
        let source = catalog.sort_source(&leaf, path.position, "GROUP BY")?;
        if groups.contains(&source) {
            return Err(error(
                "query.select_duplicate",
                path.position,
                format!("{} is grouped twice", path.text()),
            ));
        }
        groups.push(source);
        group_leaves.push(leaf);
        group_texts.push(path.text());
    }
    let mut aggregates = Vec::new();
    let mut aggregate_texts = Vec::new();
    let mut columns = Vec::new();
    for item in &query.select {
        let key = select_key(item);
        match item {
            SelectItem::Path { path, .. } => {
                let Some(index) = group_texts.iter().position(|text| *text == path.text()) else {
                    return Err(error(
                        "query.group_by_required",
                        path.position,
                        format!("{} must be grouped or aggregated", path.text()),
                    ));
                };
                columns.push(AggregateColumn {
                    key,
                    column_type: group_leaves[index].column_type(),
                    output: OutputRef::Group(index),
                    leaf: Some(group_leaves[index].clone()),
                });
            }
            SelectItem::Aggregate { call, .. } => {
                let (expr, leaf) = aggregate_expr(catalog, call)?;
                let column_type = match (call.function, &leaf) {
                    (Aggregate::Count, _) => "number",
                    (Aggregate::Sum | Aggregate::Avg, Some(leaf)) => match leaf.column_type() {
                        "currency" => "currency",
                        "percent" => "percent",
                        _ => "number",
                    },
                    (_, Some(leaf)) => leaf.column_type(),
                    (_, None) => "number",
                };
                columns.push(AggregateColumn {
                    key,
                    column_type,
                    output: OutputRef::Aggregate(aggregates.len()),
                    leaf: if matches!(call.function, Aggregate::Min | Aggregate::Max) {
                        leaf
                    } else {
                        None
                    },
                });
                aggregates.push(expr);
                aggregate_texts.push(call.text());
            }
        }
    }
    if query.order_by.len() > limits::ORDER_KEYS {
        return Err(too_complex(
            1,
            format!("sort by at most {} keys", limits::ORDER_KEYS),
        ));
    }
    let mut order = Vec::new();
    for item in &query.order_by {
        let output = match &item.target {
            OrderTarget::Aggregate(call) => aggregate_texts
                .iter()
                .position(|text| *text == call.text())
                .map(OutputRef::Aggregate)
                .ok_or_else(|| {
                    error(
                        "query.sort_invalid",
                        call.position,
                        format!("select {} to sort by it", call.text()),
                    )
                })?,
            OrderTarget::Path(path) => {
                let text = path.text();
                let by_alias = columns
                    .iter()
                    .find(|column| column.key == text)
                    .map(|column| column.output);
                match by_alias.or_else(|| {
                    group_texts
                        .iter()
                        .position(|group| *group == text)
                        .map(OutputRef::Group)
                }) {
                    Some(output) => output,
                    None => {
                        return Err(error(
                            "query.sort_invalid",
                            path.position,
                            format!("{text} must be grouped to sort by it"),
                        ))
                    }
                }
            }
        };
        order.push((output, item.descending));
    }
    let (fetch, explicit_limit) = match query.limit {
        Some((limit, position)) if limit > u64::from(limits::GROUPS) => {
            return Err(too_complex(
                position,
                format!("aggregate queries return at most {} groups", limits::GROUPS),
            ))
        }
        // At most `GROUPS`, checked above.
        Some((limit, _)) => (limit as u32, true),
        None => (limits::GROUPS + 1, false),
    };
    Ok(Resolved::Aggregate {
        columns,
        plan: AggregatePlan {
            scope,
            groups,
            aggregates,
            order,
            fetch,
        },
        explicit_limit,
    })
}

fn aggregate_expr(
    catalog: &Catalog<'_>,
    call: &AggregateCall,
) -> Result<(AggregateExpr, Option<Leaf>), ObjectError> {
    let Some(path) = &call.argument else {
        return Ok((
            AggregateExpr {
                function: Aggregate::Count,
                source: None,
            },
            None,
        ));
    };
    let leaf = catalog.base_path(path, "aggregates")?;
    let (category, source) = catalog.category(&leaf, path.position, "aggregates")?;
    let refuse = |what: &str| {
        error(
            "query.aggregate_invalid",
            call.position,
            format!(
                "{} cannot be applied to {}: {what}",
                call.function.as_str().to_uppercase(),
                leaf_name(&leaf)
            ),
        )
    };
    match call.function {
        Aggregate::Count if category == Category::Multi => {
            return Err(refuse("multi-select picklists hold several values"))
        }
        Aggregate::Count => {}
        Aggregate::Sum | Aggregate::Avg if category == Category::Number => {}
        Aggregate::Sum | Aggregate::Avg => return Err(refuse("it needs a number field")),
        Aggregate::Min | Aggregate::Max
            if matches!(
                category,
                Category::Number
                    | Category::Int
                    | Category::Time
                    | Category::Text
                    | Category::Exact
            ) => {}
        Aggregate::Min | Aggregate::Max => return Err(refuse("its values have no order")),
    }
    Ok((
        AggregateExpr {
            function: call.function,
            source: Some(source),
        },
        Some(leaf),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::parse::parse;
    use crate::{DefinitionStatus, Origin, SharingModel};

    fn field(
        object: &ObjectDefinition,
        api: &str,
        kind: FieldType,
        indexed: bool,
    ) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            object_id: object.id,
            api_name: api.into(),
            label: api.into(),
            description: None,
            field_type: kind,
            origin: Origin::Workspace,
            required: false,
            unique: false,
            indexed,
            searchable: false,
            track_history: false,
            max_length: None,
            scale: None,
            options: Vec::new(),
            default_value: None,
            reference_object_id: None,
            reference_object: None,
            delete_behaviour: None,
            formula: None,
            formula_type: None,
            rollup: None,
            lookup_filter: None,
            position: 0,
            status: DefinitionStatus::Active,
            version: 1,
            created_at: Utc::now(),
            created_by: None,
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    fn object(api: &str) -> ObjectDefinition {
        ObjectDefinition {
            id: Uuid::now_v7(),
            api_name: api.into(),
            label: api.into(),
            plural_label: api.into(),
            description: None,
            origin: Origin::Workspace,
            owner_plugin: None,
            key_prefix: "0a1".into(),
            name_label: "Name".into(),
            sharing_model: SharingModel::Private,
            grant_hierarchy: false,
            track_history: false,
            searchable: true,
            in_navigation: true,
            status: DefinitionStatus::Active,
            version: 1,
            created_at: Utc::now(),
            created_by: None,
            updated_at: Utc::now(),
            updated_by: None,
            fields: Vec::new(),
        }
    }

    struct Fixture {
        deal: ObjectDefinition,
        targets: BTreeMap<Uuid, (ObjectDefinition, Option<Visibility>)>,
    }

    fn fixture() -> Fixture {
        let mut account = object("custom:account");
        account
            .fields
            .push(field(&account, "custom:region", FieldType::Picklist, false));
        let mut deal = object("custom:deal");
        let mut lookup = field(&deal, "custom:account", FieldType::Lookup, false);
        lookup.reference_object_id = Some(account.id);
        deal.fields = vec![
            field(&deal, "custom:amount", FieldType::Currency, true),
            field(&deal, "custom:notes", FieldType::LongText, false),
            field(&deal, "custom:close", FieldType::Date, true),
            field(&deal, "custom:won", FieldType::Boolean, true),
            field(&deal, "custom:tags", FieldType::MultiPicklist, false),
            field(&deal, "custom:email", FieldType::Email, true),
            lookup,
        ];
        let visibility = Visibility {
            user_id: Uuid::from_u128(1),
            owners: vec![Uuid::from_u128(1)],
            role_ids: Vec::new(),
            master: None,
        };
        let targets = BTreeMap::from([(account.id, (account, Some(visibility)))]);
        Fixture { deal, targets }
    }

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-14T15:00:00Z")
            .expect("time")
            .with_timezone(&Utc)
    }

    fn run(fixture: &Fixture, text: &str) -> Result<Resolved, ObjectError> {
        let catalog = Catalog {
            base: &fixture.deal,
            base_visibility: None,
            targets: &fixture.targets,
            now: now(),
            include_deleted: false,
            stored_formulas: &BTreeSet::new(),
        };
        resolve(&parse(text).expect("parses"), &catalog)
    }

    fn code(result: Result<Resolved, ObjectError>) -> &'static str {
        match result {
            Err(ObjectError::Validation(issues)) => issues[0].code,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    fn predicate(resolved: Resolved) -> Predicate {
        match resolved {
            Resolved::Records { scope, .. }
            | Resolved::Aggregate {
                plan: AggregatePlan { scope, .. },
                ..
            } => scope.predicate.expect("predicate"),
        }
    }

    fn time(text: &str) -> Param {
        Param::Time(
            DateTime::parse_from_rfc3339(text)
                .expect("time")
                .with_timezone(&Utc),
        )
    }

    #[test]
    fn literals_become_typed_parameters_and_dates_become_ranges() {
        let fixture = fixture();
        let amount = fixture.deal.field("custom:amount").expect("amount").id;
        let close = fixture.deal.field("custom:close").expect("close").id;
        let resolved = run(
            &fixture,
            "SELECT name FROM custom:deal WHERE custom:amount >= 10 AND custom:close = THIS_WEEK \
             AND name = '  Big ' AND custom:close > LAST_N_DAYS:3 AND created_at <= 2026-10-01",
        )
        .expect("resolves");
        let Predicate::And(terms) = predicate(resolved) else {
            panic!("and")
        };
        let tests: Vec<Test> = terms
            .into_iter()
            .map(|term| match term {
                Predicate::Test { hops, test } if hops.is_empty() => test,
                other => panic!("{other:?}"),
            })
            .collect();
        let amount = Source::Pivot {
            field_id: amount,
            kind: PivotKind::Number,
        };
        let close = Source::Pivot {
            field_id: close,
            kind: PivotKind::Time,
        };
        assert_eq!(tests[0], Test::Compare(amount, Op::Ge, Param::Number(10.0)));
        assert_eq!(
            tests[1],
            Test::Range(
                close,
                time("2026-10-12T00:00:00Z"),
                time("2026-10-19T00:00:00Z")
            )
        );
        assert_eq!(
            tests[2],
            Test::Compare(
                Source::Column(Column::NameKey),
                Op::Eq,
                Param::Text("big".into())
            )
        );
        // "After the last three days" starts tomorrow.
        assert_eq!(
            tests[3],
            Test::Compare(close, Op::Ge, time("2026-10-15T00:00:00Z"))
        );
        assert_eq!(
            tests[4],
            Test::Compare(
                Source::Column(Column::CreatedAt),
                Op::Lt,
                time("2026-10-02T00:00:00Z")
            )
        );
    }

    #[test]
    fn periods_cover_months_years_and_n_days() {
        let now = now();
        let day = |text: &str| match time(text) {
            Param::Time(time) => time,
            _ => unreachable!(),
        };
        assert_eq!(
            period(RelativeDate::LastMonth, now),
            (day("2026-09-01T00:00:00Z"), day("2026-10-01T00:00:00Z"))
        );
        assert_eq!(
            period(RelativeDate::ThisYear, now),
            (day("2026-01-01T00:00:00Z"), day("2027-01-01T00:00:00Z"))
        );
        assert_eq!(
            period(RelativeDate::LastNDays(7), now),
            (day("2026-10-08T00:00:00Z"), day("2026-10-15T00:00:00Z"))
        );
        assert_eq!(
            period(RelativeDate::NextNDays(1), now),
            (day("2026-10-15T00:00:00Z"), day("2026-10-16T00:00:00Z"))
        );
        let december = day("2026-12-31T23:00:00Z");
        assert_eq!(
            period(RelativeDate::NextMonth, december),
            (day("2027-01-01T00:00:00Z"), day("2027-02-01T00:00:00Z"))
        );
    }

    #[test]
    fn negations_and_nulls_are_two_valued() {
        let fixture = fixture();
        let resolved = run(
            &fixture,
            "SELECT id FROM custom:deal WHERE custom:amount != 3 AND owner = null AND is_deleted = false",
        )
        .expect("resolves");
        let Predicate::And(terms) = predicate(resolved) else {
            panic!("and")
        };
        assert!(terms.iter().all(|term| matches!(term, Predicate::Not(_))));
    }

    #[test]
    fn relationship_conditions_carry_the_target_visibility() {
        let fixture = fixture();
        let resolved = run(
            &fixture,
            "SELECT custom:account.name FROM custom:deal WHERE custom:account.custom:region = 'emea'",
        )
        .expect("resolves");
        let Predicate::Test { hops, .. } = predicate(resolved.clone()) else {
            panic!("test")
        };
        assert_eq!(hops.len(), 1);
        assert!(hops[0].visibility.is_some());
        let Resolved::Records { selections, .. } = resolved else {
            panic!("records")
        };
        assert_eq!(selections[0].key, "custom:account.name");
        assert_eq!(selections[0].hops[0].field_api_name, "custom:account");
    }

    #[test]
    fn misuse_has_stable_codes() {
        let fixture = fixture();
        for (text, expected) in [
            (
                "SELECT name FROM custom:deal WHERE custom:notes = 'x'",
                "query.field_not_indexed",
            ),
            (
                "SELECT name FROM custom:deal WHERE custom:missing = 1",
                "query.field_unknown",
            ),
            (
                "SELECT name FROM custom:deal WHERE custom:amount = 'ten'",
                "query.value_invalid",
            ),
            (
                "SELECT name FROM custom:deal WHERE owner = 'nobody'",
                "query.value_invalid",
            ),
            (
                "SELECT name FROM custom:deal WHERE custom:amount LIKE 'x%'",
                "query.operator_invalid",
            ),
            (
                "SELECT name FROM custom:deal WHERE custom:won > true",
                "query.operator_invalid",
            ),
            (
                "SELECT name FROM custom:deal WHERE custom:amount.name = 'x'",
                "query.relationship_unknown",
            ),
            (
                "SELECT name FROM custom:deal ORDER BY custom:account",
                "query.sort_invalid",
            ),
            (
                "SELECT name FROM custom:deal ORDER BY custom:account.name",
                "query.relationship_unsupported",
            ),
            (
                "SELECT name, COUNT() FROM custom:deal",
                "query.group_by_required",
            ),
            (
                "SELECT SUM(custom:email) FROM custom:deal",
                "query.aggregate_invalid",
            ),
            (
                "SELECT name, name FROM custom:deal",
                "query.select_duplicate",
            ),
            (
                "SELECT name FROM custom:deal LIMIT 50001",
                "query.too_complex",
            ),
            (
                "SELECT COUNT() FROM custom:deal LIMIT 2001",
                "query.too_complex",
            ),
            (
                "SELECT name FROM custom:deal WHERE custom:tags = 'a'",
                "query.operator_invalid",
            ),
            (
                "SELECT name FROM custom:deal WHERE is_deleted = 1",
                "query.value_invalid",
            ),
        ] {
            assert_eq!(code(run(&fixture, text)), expected, "{text}");
        }
    }

    #[test]
    fn aggregates_resolve_groups_outputs_and_order() {
        let fixture = fixture();
        let resolved = run(
            &fixture,
            "SELECT custom:won, COUNT() n, SUM(custom:amount) FROM custom:deal \
             GROUP BY custom:won ORDER BY n DESC, custom:won",
        )
        .expect("resolves");
        let Resolved::Aggregate {
            columns,
            plan,
            explicit_limit,
        } = resolved
        else {
            panic!("aggregate")
        };
        assert!(!explicit_limit);
        assert_eq!(
            columns
                .iter()
                .map(|column| column.key.as_str())
                .collect::<Vec<_>>(),
            ["custom:won", "n", "sum(custom:amount)"]
        );
        assert_eq!(columns[2].column_type, "currency");
        assert_eq!(
            plan.order,
            vec![
                (OutputRef::Aggregate(0), true),
                (OutputRef::Group(0), false)
            ]
        );
        assert_eq!(
            plan.output_kinds(),
            vec![CellKind::Text, CellKind::Int, CellKind::Number]
        );
    }

    #[test]
    fn relationship_paths_in_conditions_count_towards_the_limit() {
        let mut fixture = fixture();
        let account = fixture
            .deal
            .field("custom:account")
            .cloned()
            .expect("lookup");
        for index in 0..limits::RELATIONSHIP_PATHS {
            let mut lookup = account.clone();
            lookup.id = Uuid::now_v7();
            lookup.api_name = format!("custom:account{index}");
            fixture.deal.fields.push(lookup);
        }
        let conditions = (0..=limits::RELATIONSHIP_PATHS)
            .map(|index| {
                let lookup = if index == limits::RELATIONSHIP_PATHS {
                    "custom:account".to_owned()
                } else {
                    format!("custom:account{index}")
                };
                format!("{lookup}.name = 'x'")
            })
            .collect::<Vec<_>>()
            .join(" OR ");
        assert_eq!(
            code(run(
                &fixture,
                &format!("SELECT name FROM custom:deal WHERE {conditions}")
            )),
            "query.too_complex"
        );
    }

    #[test]
    fn condition_limits_are_enforced() {
        let fixture = fixture();
        let many = (0..=limits::CONDITIONS)
            .map(|index| format!("custom:amount = {index}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        assert_eq!(
            code(run(
                &fixture,
                &format!("SELECT id FROM custom:deal WHERE {many}")
            )),
            "query.too_complex"
        );
    }
}
