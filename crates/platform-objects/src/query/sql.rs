//! Renders query plans to SQL for each backend (ADR 0019 §4).
//!
//! One renderer serves PostgreSQL and SQLite; the dialect only changes
//! placeholders. Every value is a bound parameter and every name in the SQL
//! text is a fixed table, column or generated alias, never query input.
//! Conditions are `EXISTS` probes on tenant-first pivot indexes, hops are
//! nested `EXISTS` through `record_references`, and sort or group keys are
//! `LEFT JOIN`s on single-valued pivots.

use super::parse::Aggregate;
use super::plan::{
    AggregatePlan, Column, Hop, Keyset, Op, OutputRef, Param, PivotKind, Predicate, RecordPlan,
    Scope, Source, Test,
};
use crate::{RecordStatus, Visibility};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    Sqlite,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub sql: String,
    pub params: Vec<Param>,
}

struct Writer {
    dialect: Dialect,
    sql: String,
    params: Vec<Param>,
    aliases: usize,
}

impl Writer {
    fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            sql: String::new(),
            params: Vec::new(),
            aliases: 0,
        }
    }

    fn push(&mut self, text: &str) -> &mut Self {
        self.sql.push_str(text);
        self
    }

    fn bind(&mut self, param: Param) -> &mut Self {
        self.params.push(param);
        let placeholder = match self.dialect {
            Dialect::Postgres => format!("${}", self.params.len()),
            Dialect::Sqlite => format!("?{}", self.params.len()),
        };
        self.sql.push_str(&placeholder);
        self
    }

    fn bind_list(&mut self, params: impl IntoIterator<Item = Param>) -> &mut Self {
        for (index, param) in params.into_iter().enumerate() {
            if index > 0 {
                self.push(", ");
            }
            self.bind(param);
        }
        self
    }

    fn alias(&mut self, prefix: &str) -> String {
        self.aliases += 1;
        format!("{prefix}{}", self.aliases)
    }

    /// Runs `write` into a separate buffer and returns its text. Placeholders
    /// are numbered, so text can be assembled in any order.
    fn capture(&mut self, write: impl FnOnce(&mut Self)) -> String {
        let outer = std::mem::take(&mut self.sql);
        write(self);
        std::mem::replace(&mut self.sql, outer)
    }

    fn finish(self) -> Rendered {
        Rendered {
            sql: self.sql,
            params: self.params,
        }
    }
}

const RECORD_COLUMNS: &str =
    "r.id, r.object_id, r.public_id, r.name, r.owner_id, r.record_type_id, \
     r.data, r.version, r.created_at, r.created_by, r.updated_at, r.updated_by, r.touched_at, \
     r.archived_at, r.deleted_at";

fn column(alias: &str, column: Column) -> String {
    let name = match column {
        Column::PublicId => "public_id",
        Column::NameKey => "name_key",
        Column::Owner => "owner_id",
        Column::CreatedAt => "created_at",
        Column::CreatedBy => "created_by",
        Column::UpdatedAt => "updated_at",
        Column::UpdatedBy => "updated_by",
        Column::TouchedAt => "touched_at",
        Column::Version => "version",
        Column::DeletedAt => "deleted_at",
        Column::IsDeleted => {
            return format!(
                "CAST(CASE WHEN {alias}.deleted_at IS NULL THEN 0 ELSE 1 END AS BIGINT)"
            )
        }
    };
    format!("{alias}.{name}")
}

fn pivot_column(kind: PivotKind) -> &'static str {
    match kind {
        PivotKind::Text => "text_value",
        PivotKind::Number => "number_value",
        PivotKind::Time => "time_value",
    }
}

fn op(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Lt => "<",
        Op::Le => "<=",
        Op::Gt => ">",
        Op::Ge => ">=",
    }
}

fn visibility(writer: &mut Writer, alias: &str, visibility: &Visibility) {
    let Some(field_id) = visibility.master else {
        return owned_or_shared(writer, alias, visibility);
    };
    // A detail is visible when its master is (ADR 0025).
    let link = writer.alias("mr");
    let master = writer.alias("m");
    writer.push(&format!(
        "EXISTS (SELECT 1 FROM record_references {link} JOIN records {master} \
         ON {master}.tenant_id = {link}.tenant_id AND {master}.id = {link}.target_id \
         WHERE {link}.tenant_id = {alias}.tenant_id AND {link}.record_id = {alias}.id \
         AND {link}.field_id = "
    ));
    writer.bind(Param::Uuid(field_id));
    writer.push(" AND ");
    owned_or_shared(writer, &master, visibility);
    writer.push(")");
}

fn owned_or_shared(writer: &mut Writer, alias: &str, visibility: &Visibility) {
    let shares = writer.alias("s");
    writer.push(&format!("({alias}.owner_id IN ("));
    writer.bind_list(visibility.owners.iter().map(|owner| Param::Uuid(*owner)));
    writer.push(&format!(
        ") OR EXISTS (SELECT 1 FROM record_shares {shares} WHERE {shares}.tenant_id = {alias}.tenant_id \
         AND {shares}.record_id = {alias}.id AND (({shares}.grantee_kind = 'user' AND {shares}.grantee_id = "
    ));
    writer.bind(Param::Uuid(visibility.user_id));
    writer.push(")");
    if !visibility.role_ids.is_empty() {
        writer.push(&format!(
            " OR ({shares}.grantee_kind = 'role' AND {shares}.grantee_id IN ("
        ));
        writer.bind_list(visibility.role_ids.iter().map(|role| Param::Uuid(*role)));
        writer.push("))");
    }
    writer.push(")))");
}

/// The value expression of a test on one pivot or reference row.
fn condition_on(writer: &mut Writer, expression: &str, test: &Test) {
    match test {
        Test::Compare(_, comparison, param) => {
            writer.push(&format!(" AND {expression} {} ", op(*comparison)));
            writer.bind(param.clone());
        }
        Test::Range(_, start, end) => {
            writer.push(&format!(" AND {expression} >= "));
            writer.bind(start.clone());
            writer.push(&format!(" AND {expression} < "));
            writer.bind(end.clone());
        }
        Test::In(_, params) => {
            writer.push(&format!(" AND {expression} IN ("));
            writer.bind_list(params.iter().cloned());
            writer.push(")");
        }
        Test::Like(_, pattern) => {
            writer.push(&format!(" AND {expression} LIKE "));
            writer.bind(Param::Text(pattern.clone()));
            writer.push(" ESCAPE '\\'");
        }
        Test::HasValue(_) => {}
    }
}

fn test(writer: &mut Writer, alias: &str, test: &Test) {
    let source = match test {
        Test::Compare(source, ..)
        | Test::Range(source, ..)
        | Test::In(source, _)
        | Test::Like(source, _)
        | Test::HasValue(source) => *source,
    };
    match source {
        Source::Column(name) => {
            let expression = column(alias, name);
            writer.push(&format!("({expression} IS NOT NULL"));
            condition_on(writer, &expression, test);
            writer.push(")");
        }
        Source::Pivot { field_id, kind } => {
            let pivot = writer.alias("i");
            writer.push(&format!(
                "EXISTS (SELECT 1 FROM record_index {pivot} WHERE {pivot}.tenant_id = {alias}.tenant_id \
                 AND {pivot}.record_id = {alias}.id AND {pivot}.field_id = "
            ));
            writer.bind(Param::Uuid(field_id));
            let expression = format!("{pivot}.{}", pivot_column(kind));
            writer.push(&format!(" AND {expression} IS NOT NULL"));
            condition_on(writer, &expression, test);
            writer.push(")");
        }
        Source::Reference { field_id } => {
            let reference = writer.alias("f");
            let target = writer.alias("t");
            writer.push(&format!(
                "EXISTS (SELECT 1 FROM record_references {reference} JOIN records {target} \
                 ON {target}.tenant_id = {reference}.tenant_id AND {target}.id = {reference}.target_id \
                 WHERE {reference}.tenant_id = {alias}.tenant_id AND {reference}.record_id = {alias}.id \
                 AND {reference}.field_id = "
            ));
            writer.bind(Param::Uuid(field_id));
            condition_on(writer, &format!("{target}.public_id"), test);
            writer.push(")");
        }
    }
}

fn hops(writer: &mut Writer, alias: &str, through: &[Hop], leaf: &Test) {
    let Some((hop, rest)) = through.split_first() else {
        test(writer, alias, leaf);
        return;
    };
    let reference = writer.alias("f");
    let parent = writer.alias("p");
    writer.push(&format!(
        "EXISTS (SELECT 1 FROM record_references {reference} JOIN records {parent} \
         ON {parent}.tenant_id = {reference}.tenant_id AND {parent}.id = {reference}.target_id \
         WHERE {reference}.tenant_id = {alias}.tenant_id AND {reference}.record_id = {alias}.id \
         AND {reference}.field_id = "
    ));
    writer.bind(Param::Uuid(hop.field_id));
    writer.push(&format!(" AND {parent}.object_id = "));
    writer.bind(Param::Uuid(hop.object_id));
    writer.push(&format!(" AND {parent}.deleted_at IS NULL"));
    if let Some(rule) = &hop.visibility {
        writer.push(" AND ");
        visibility(writer, &parent, rule);
    }
    writer.push(" AND ");
    hops(writer, &parent, rest, leaf);
    writer.push(")");
}

fn predicate(writer: &mut Writer, predicate: &Predicate) {
    match predicate {
        Predicate::And(terms) | Predicate::Or(terms) => {
            let joiner = if matches!(predicate, Predicate::And(_)) {
                " AND "
            } else {
                " OR "
            };
            writer.push("(");
            for (index, term) in terms.iter().enumerate() {
                if index > 0 {
                    writer.push(joiner);
                }
                self::predicate(writer, term);
            }
            writer.push(")");
        }
        Predicate::Not(inner) => {
            writer.push("NOT (");
            self::predicate(writer, inner);
            writer.push(")");
        }
        Predicate::Test {
            hops: through,
            test,
        } => hops(writer, "r", through, test),
    }
}

fn scope(writer: &mut Writer, tenant_id: Uuid, scope: &Scope) {
    writer.push(" WHERE r.tenant_id = ");
    writer.bind(Param::Uuid(tenant_id));
    writer.push(" AND r.object_id = ");
    writer.bind(Param::Uuid(scope.object_id));
    writer.push(match scope.status {
        RecordStatus::Active => " AND r.deleted_at IS NULL",
        RecordStatus::Deleted => " AND r.deleted_at IS NOT NULL",
        RecordStatus::Any => "",
    });
    if let Some(rule) = &scope.visibility {
        writer.push(" AND ");
        visibility(writer, "r", rule);
    }
    if let Some(filter) = &scope.predicate {
        writer.push(" AND ");
        predicate(writer, filter);
    }
}

/// A sort or group key's expression, writing a `LEFT JOIN` for pivots and
/// references.
fn key_expression(writer: &mut Writer, source: Source) -> String {
    match source {
        Source::Column(name) => column("r", name),
        Source::Pivot { field_id, kind } => {
            let pivot = writer.alias("k");
            writer.push(&format!(
                " LEFT JOIN record_index {pivot} ON {pivot}.tenant_id = r.tenant_id \
                 AND {pivot}.record_id = r.id AND {pivot}.field_id = "
            ));
            writer.bind(Param::Uuid(field_id));
            format!("{pivot}.{}", pivot_column(kind))
        }
        Source::Reference { field_id } => {
            let reference = writer.alias("g");
            let target = writer.alias("gt");
            writer.push(&format!(
                " LEFT JOIN record_references {reference} ON {reference}.tenant_id = r.tenant_id \
                 AND {reference}.record_id = r.id AND {reference}.field_id = "
            ));
            writer.bind(Param::Uuid(field_id));
            writer.push(&format!(
                " LEFT JOIN records {target} ON {target}.tenant_id = {reference}.tenant_id \
                 AND {target}.id = {reference}.target_id"
            ));
            format!("{target}.public_id")
        }
    }
}

/// Writes the `LEFT JOIN`s for `sources` and returns them with each key's
/// expression.
fn keys(writer: &mut Writer, sources: &[Source]) -> (String, Vec<String>) {
    let mut expressions = Vec::new();
    let joins = writer.capture(|writer| {
        expressions = sources
            .iter()
            .map(|source| key_expression(writer, *source))
            .collect();
    });
    (joins, expressions)
}

/// Rows of the keyset after `after`: for keys k1..kn and the id,
/// `k1 after v1 OR (k1 = v1 AND (k2 after v2 OR ...))`, with records
/// without a value sorting last in both directions.
fn keyset(writer: &mut Writer, expressions: &[String], plan: &RecordPlan, after: &Keyset) {
    writer.push(" AND (");
    let mut alternatives = 0;
    for index in 0..=expressions.len() {
        let mut parts: Vec<(String, Option<Param>)> = Vec::new();
        for (position, expression) in expressions.iter().enumerate().take(index) {
            match after.values.get(position).cloned().flatten() {
                Some(value) => parts.push((format!("{expression} = "), Some(value))),
                None => parts.push((format!("{expression} IS NULL"), None)),
            }
        }
        let last = if index == expressions.len() {
            Some((
                "r.id > ".to_owned(),
                Some(Param::Uuid(after.id)),
                String::new(),
            ))
        } else {
            match after.values.get(index).cloned().flatten() {
                // Nothing sorts after a missing value except its ties.
                None => None,
                Some(value) => {
                    let comparison = if plan.sort[index].descending {
                        "<"
                    } else {
                        ">"
                    };
                    let expression = &expressions[index];
                    let tail = if plan.sort[index].source.nullable() {
                        format!(" OR {expression} IS NULL")
                    } else {
                        String::new()
                    };
                    Some((
                        format!("({expression} {comparison} "),
                        Some(value),
                        format!("{tail})"),
                    ))
                }
            }
        };
        let Some((head, value, tail)) = last else {
            continue;
        };
        if alternatives > 0 {
            writer.push(" OR ");
        }
        alternatives += 1;
        writer.push("(");
        for (text, value) in parts {
            writer.push(&text);
            if let Some(value) = value {
                writer.bind(value);
            }
            writer.push(" AND ");
        }
        writer.push(&head);
        if let Some(value) = value {
            writer.bind(value);
        }
        writer.push(&tail);
        writer.push(")");
    }
    writer.push(")");
}

fn order_term(expression: &str, descending: bool, nullable: bool) -> String {
    let direction = if descending { "DESC" } else { "ASC" };
    if nullable {
        format!("({expression} IS NULL) ASC, {expression} {direction}")
    } else {
        format!("{expression} {direction}")
    }
}

/// Records of one object in the plan's order.
pub fn records(dialect: Dialect, tenant_id: Uuid, plan: &RecordPlan) -> Rendered {
    let mut writer = Writer::new(dialect);
    let sources: Vec<Source> = plan.sort.iter().map(|key| key.source).collect();
    let (joins, expressions) = keys(&mut writer, &sources);
    writer.push(&format!("SELECT {RECORD_COLUMNS} FROM records r{joins}"));
    scope(&mut writer, tenant_id, &plan.scope);
    if let Some(after) = &plan.after {
        keyset(&mut writer, &expressions, plan, after);
    }
    let mut order: Vec<String> = plan
        .sort
        .iter()
        .zip(&expressions)
        .map(|(key, expression)| order_term(expression, key.descending, key.source.nullable()))
        .collect();
    order.push("r.id ASC".into());
    writer.push(&format!(" ORDER BY {} LIMIT ", order.join(", ")));
    writer.bind(Param::Int(i64::from(plan.fetch)));
    writer.finish()
}

/// The upper bound of a prefix range: every term starting with `prefix`
/// sorts before it in byte order, as both backends compare these terms.
fn prefix_end(prefix: &str) -> String {
    format!("{prefix}{}", char::MAX)
}

/// Global search (ADR 0020): records matching every term as a prefix,
/// scored, ranked per object, with each object's visibility applied. Rows
/// carry the record columns and `score`, best first, at most
/// `per_object + 1` per object.
pub fn search(dialect: Dialect, tenant_id: Uuid, plan: &crate::search::SearchPlan) -> Rendered {
    let mut writer = Writer::new(dialect);
    writer.push("WITH ");
    for (index, term) in plan.terms.iter().enumerate() {
        if index > 0 {
            writer.push(", ");
        }
        writer.push(&format!(
            "m{index} AS (SELECT record_id, MAX(weight + CASE WHEN term = "
        ));
        writer.bind(Param::Text(term.clone()));
        writer.push(" THEN 1 ELSE 0 END) AS s FROM record_search_terms WHERE tenant_id = ");
        writer.bind(Param::Uuid(tenant_id));
        writer.push(" AND term >= ");
        writer.bind(Param::Text(term.clone()));
        writer.push(" AND term < ");
        writer.bind(Param::Text(prefix_end(term)));
        writer.push(" GROUP BY record_id)");
    }
    let score = (0..plan.terms.len())
        .map(|index| format!("m{index}.s"))
        .collect::<Vec<_>>()
        .join(" + ");
    writer.push(&format!(
        ", ranked AS (SELECT {RECORD_COLUMNS}, r.name_key, CAST({score} AS BIGINT) AS score, \
         ROW_NUMBER() OVER (PARTITION BY r.object_id ORDER BY {score} DESC, r.name_key ASC, r.id ASC) AS place \
         FROM m0"
    ));
    for index in 1..plan.terms.len() {
        writer.push(&format!(
            " JOIN m{index} ON m{index}.record_id = m0.record_id"
        ));
    }
    writer.push(" JOIN records r ON r.tenant_id = ");
    writer.bind(Param::Uuid(tenant_id));
    writer.push(" AND r.id = m0.record_id WHERE r.deleted_at IS NULL AND (");
    for (index, scope) in plan.scopes.iter().enumerate() {
        if index > 0 {
            writer.push(" OR ");
        }
        writer.push("(r.object_id = ");
        writer.bind(Param::Uuid(scope.object_id));
        if let Some(rule) = &scope.visibility {
            writer.push(" AND ");
            visibility(&mut writer, "r", rule);
        }
        writer.push(")");
    }
    if plan.scopes.is_empty() {
        writer.push("1 = 0");
    }
    writer.push(
        ")) SELECT id, object_id, public_id, name, owner_id, record_type_id, data, version, \
         created_at, created_by, updated_at, updated_by, touched_at, archived_at, deleted_at, score \
         FROM ranked WHERE place <= ",
    );
    writer.bind(Param::Int(i64::from(plan.per_object) + 1));
    writer.push(" ORDER BY score DESC, name_key ASC, id ASC");
    writer.finish()
}

fn aggregate_expression(function: Aggregate, expression: Option<&str>) -> String {
    match (function, expression) {
        (Aggregate::Count, None) => "COUNT(*)".into(),
        (Aggregate::Count, Some(expression)) => format!("COUNT({expression})"),
        (Aggregate::Sum, Some(expression)) => format!("SUM({expression})"),
        (Aggregate::Avg, Some(expression)) => format!("AVG({expression})"),
        (Aggregate::Min, Some(expression)) => format!("MIN({expression})"),
        (Aggregate::Max, Some(expression)) => format!("MAX({expression})"),
        (_, None) => "COUNT(*)".into(),
    }
}

/// Groups and aggregates over one object's visible rows.
pub fn aggregate(dialect: Dialect, tenant_id: Uuid, plan: &AggregatePlan) -> Rendered {
    let mut writer = Writer::new(dialect);
    let mut sources: Vec<Source> = plan.groups.clone();
    sources.extend(
        plan.aggregates
            .iter()
            .filter_map(|aggregate| aggregate.source),
    );
    let (joins, expressions) = keys(&mut writer, &sources);
    let (groups, arguments) = expressions.split_at(plan.groups.len());
    let mut arguments = arguments.iter();
    let aggregates: Vec<String> = plan
        .aggregates
        .iter()
        .map(|aggregate| {
            let argument = aggregate.source.and_then(|_| arguments.next());
            aggregate_expression(aggregate.function, argument.map(String::as_str))
        })
        .collect();
    let outputs: Vec<&str> = groups
        .iter()
        .chain(aggregates.iter())
        .map(String::as_str)
        .collect();
    writer.push(&format!(
        "SELECT {} FROM records r{joins}",
        outputs.join(", ")
    ));
    scope(&mut writer, tenant_id, &plan.scope);
    if !groups.is_empty() {
        writer.push(&format!(" GROUP BY {}", groups.join(", ")));
    }
    let mut order: Vec<String> = plan
        .order
        .iter()
        .map(|(output, descending)| match output {
            OutputRef::Group(index) => {
                order_term(&groups[*index], *descending, plan.groups[*index].nullable())
            }
            OutputRef::Aggregate(index) => order_term(&aggregates[*index], *descending, true),
        })
        .collect();
    // Then every remaining group key, so the order is stable.
    for (index, group) in groups.iter().enumerate() {
        if !plan
            .order
            .iter()
            .any(|(output, _)| *output == OutputRef::Group(index))
        {
            order.push(order_term(group, false, plan.groups[index].nullable()));
        }
    }
    if !order.is_empty() {
        writer.push(&format!(" ORDER BY {}", order.join(", ")));
    }
    writer.push(" LIMIT ");
    writer.bind(Param::Int(i64::from(plan.fetch)));
    writer.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::plan::{AggregateExpr, SortKey};

    fn scope(predicate: Option<Predicate>) -> Scope {
        Scope {
            object_id: Uuid::from_u128(2),
            status: RecordStatus::Active,
            visibility: Some(Visibility {
                user_id: Uuid::from_u128(3),
                owners: vec![Uuid::from_u128(3)],
                role_ids: vec![Uuid::from_u128(4)],
                master: None,
            }),
            predicate,
        }
    }

    fn placeholders(rendered: &Rendered, sigil: char) -> Vec<usize> {
        rendered
            .sql
            .split(sigil)
            .skip(1)
            .map(|rest| {
                rest.chars()
                    .take_while(char::is_ascii_digit)
                    .collect::<String>()
                    .parse()
                    .expect("number")
            })
            .collect()
    }

    #[test]
    fn every_parameter_has_one_placeholder() {
        let plan = RecordPlan {
            scope: scope(Some(Predicate::Not(Box::new(Predicate::Test {
                hops: vec![Hop {
                    field_id: Uuid::from_u128(5),
                    object_id: Uuid::from_u128(6),
                    visibility: None,
                }],
                test: Test::Like(Source::Column(Column::NameKey), "a%".into()),
            })))),
            sort: vec![SortKey {
                source: Source::Pivot {
                    field_id: Uuid::from_u128(7),
                    kind: PivotKind::Number,
                },
                descending: true,
            }],
            after: Some(Keyset {
                values: vec![Some(Param::Number(4.0))],
                id: Uuid::from_u128(8),
            }),
            fetch: 11,
        };
        for (dialect, sigil) in [(Dialect::Postgres, '$'), (Dialect::Sqlite, '?')] {
            let rendered = records(dialect, Uuid::from_u128(1), &plan);
            let mut numbers = placeholders(&rendered, sigil);
            numbers.sort_unstable();
            assert_eq!(
                numbers,
                (1..=rendered.params.len()).collect::<Vec<_>>(),
                "{}",
                rendered.sql
            );
            // The join's field id binds first because the join comes first.
            assert_eq!(rendered.params[0], Param::Uuid(Uuid::from_u128(7)));
            assert_eq!(rendered.params[1], Param::Uuid(Uuid::from_u128(1)));
            assert_eq!(rendered.params.last(), Some(&Param::Int(11)));
            assert!(rendered
                .sql
                .contains("NOT (EXISTS (SELECT 1 FROM record_references"));
            assert!(rendered.sql.contains("LIKE"));
            assert!(rendered.sql.contains("ORDER BY (k"));
        }
    }

    #[test]
    fn aggregates_group_and_order_without_parameters_in_expressions() {
        let plan = AggregatePlan {
            scope: scope(None),
            groups: vec![Source::Reference {
                field_id: Uuid::from_u128(9),
            }],
            aggregates: vec![
                AggregateExpr {
                    function: Aggregate::Count,
                    source: None,
                },
                AggregateExpr {
                    function: Aggregate::Sum,
                    source: Some(Source::Pivot {
                        field_id: Uuid::from_u128(10),
                        kind: PivotKind::Number,
                    }),
                },
            ],
            order: vec![(OutputRef::Aggregate(1), true)],
            fetch: 2001,
        };
        let rendered = aggregate(Dialect::Postgres, Uuid::from_u128(1), &plan);
        let mut numbers = placeholders(&rendered, '$');
        numbers.sort_unstable();
        assert_eq!(
            numbers,
            (1..=rendered.params.len()).collect::<Vec<_>>(),
            "{}",
            rendered.sql
        );
        assert!(rendered.sql.starts_with("SELECT gt"), "{}", rendered.sql);
        assert!(rendered.sql.contains("COUNT(*), SUM(k"));
        assert!(rendered.sql.contains(" GROUP BY gt"));
        assert!(rendered.sql.contains("ORDER BY (SUM("));
    }
}
