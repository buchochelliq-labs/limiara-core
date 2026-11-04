//! The formula language (ADR 0021, #298).
//!
//! [`parse`] turns text into a syntax tree, [`check`] types it against
//! object metadata, and [`evaluate`] computes a value from a record through
//! an [`Env`]. Formulas have no loops, no I/O and bounded size, so
//! evaluation is linear in the formula's length; they never become SQL.

pub mod check;
pub mod parse;

use crate::query::plan::Standard;
use crate::{FieldType, Record};
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, SecondsFormat, Utc};
use parse::{BinaryOp, FormulaError, UnaryOp};
use serde_json::Value as Json;
use std::collections::BTreeSet;

/// Size limits (ADR 0021 §2).
pub mod limits {
    pub const LENGTH: usize = 3_900;
    pub const NODES: usize = 400;
    pub const DEPTH: usize = 40;
    pub const HOPS: usize = 3;
    /// Text values never grow past the long text limit.
    pub const TEXT: usize = crate::values::LONG_TEXT_LIMIT as usize;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Number,
    Text,
    Boolean,
    Date,
    DateTime,
    /// A multi-select picklist value; only `INCLUDES` and `ISBLANK` take it.
    Multi,
    /// The type of `NULL`, which matches any other.
    Null,
}

impl Type {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Number => "a number",
            Self::Text => "text",
            Self::Boolean => "true or false",
            Self::Date => "a date",
            Self::DateTime => "a date-time",
            Self::Multi => "a multi-select picklist",
            Self::Null => "NULL",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Number(f64),
    Text(String),
    Bool(bool),
    Date(NaiveDate),
    DateTime(DateTime<Utc>),
    Multi(Vec<String>),
}

impl Value {
    fn is_blank(&self) -> bool {
        match self {
            Self::Null => true,
            Self::Text(text) => text.is_empty(),
            Self::Multi(items) => items.is_empty(),
            _ => false,
        }
    }

    fn truthy(&self) -> bool {
        matches!(self, Self::Bool(true))
    }

    /// The JSON form of a formula result.
    pub fn to_json(&self) -> Json {
        match self {
            Self::Null => Json::Null,
            Self::Number(number) => {
                serde_json::Number::from_f64(*number).map_or(Json::Null, Json::Number)
            }
            Self::Text(text) => Json::String(text.clone()),
            Self::Bool(flag) => Json::Bool(*flag),
            Self::Date(date) => Json::String(date.format("%Y-%m-%d").to_string()),
            Self::DateTime(time) => Json::String(time.to_rfc3339_opts(SecondsFormat::Secs, true)),
            Self::Multi(items) => Json::Array(items.iter().cloned().map(Json::String).collect()),
        }
    }
}

/// The end of a field reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Leaf {
    Standard(Standard),
    Field {
        api_name: String,
        field_type: FieldType,
    },
}

/// A field of the record, or of a record its lookups lead to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldRef {
    /// Lookup fields followed, by API name, from the record.
    pub hops: Vec<String>,
    pub leaf: Leaf,
    pub ty: Type,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    If,
    Case,
    And,
    Or,
    Not,
    IsBlank,
    BlankValue,
    IsPickval,
    Includes,
    Len,
    Lower,
    Upper,
    Trim,
    Left,
    Right,
    Mid,
    Contains,
    Begins,
    Substitute,
    Text,
    Value,
    Abs,
    Floor,
    Ceiling,
    Round,
    Mod,
    Min,
    Max,
    Today,
    Now,
    Date,
    DateValue,
    Year,
    Month,
    Day,
    AddMonths,
    IsNew,
    IsChanged,
    PriorValue,
    Regex,
    IsNumber,
    Find,
    Lpad,
    Rpad,
    Sqrt,
    Exp,
    Ln,
    Log,
    Trunc,
    Weekday,
    DateTimeValue,
}

impl Function {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "IF" => Self::If,
            "CASE" => Self::Case,
            "AND" => Self::And,
            "OR" => Self::Or,
            "NOT" => Self::Not,
            "ISBLANK" => Self::IsBlank,
            "BLANKVALUE" | "NULLVALUE" => Self::BlankValue,
            "ISPICKVAL" => Self::IsPickval,
            "INCLUDES" => Self::Includes,
            "LEN" => Self::Len,
            "LOWER" => Self::Lower,
            "UPPER" => Self::Upper,
            "TRIM" => Self::Trim,
            "LEFT" => Self::Left,
            "RIGHT" => Self::Right,
            "MID" => Self::Mid,
            "CONTAINS" => Self::Contains,
            "BEGINS" => Self::Begins,
            "SUBSTITUTE" => Self::Substitute,
            "TEXT" => Self::Text,
            "VALUE" => Self::Value,
            "ABS" => Self::Abs,
            "FLOOR" => Self::Floor,
            "CEILING" => Self::Ceiling,
            "ROUND" => Self::Round,
            "MOD" => Self::Mod,
            "MIN" => Self::Min,
            "MAX" => Self::Max,
            "TODAY" => Self::Today,
            "NOW" => Self::Now,
            "DATE" => Self::Date,
            "DATEVALUE" => Self::DateValue,
            "YEAR" => Self::Year,
            "MONTH" => Self::Month,
            "DAY" => Self::Day,
            "ADDMONTHS" => Self::AddMonths,
            "ISNEW" => Self::IsNew,
            "ISCHANGED" => Self::IsChanged,
            "PRIORVALUE" => Self::PriorValue,
            "REGEX" => Self::Regex,
            "ISNUMBER" => Self::IsNumber,
            "FIND" => Self::Find,
            "LPAD" => Self::Lpad,
            "RPAD" => Self::Rpad,
            "SQRT" => Self::Sqrt,
            "EXP" => Self::Exp,
            "LN" => Self::Ln,
            "LOG" => Self::Log,
            "TRUNC" => Self::Trunc,
            "WEEKDAY" => Self::Weekday,
            "DATETIMEVALUE" => Self::DateTimeValue,
            _ => return None,
        })
    }
}

/// A `REGEX` pattern, compiled when the formula is checked. Matching is
/// linear in the text's length; patterns are compared by their text.
#[derive(Debug, Clone)]
pub struct Pattern(std::sync::Arc<regex::Regex>);

impl Pattern {
    /// Compiles a pattern that must match the whole text, as in Salesforce.
    pub fn new(pattern: &str) -> Result<Self, String> {
        regex::RegexBuilder::new(&format!("^(?:{pattern})$"))
            .size_limit(1 << 20)
            .dfa_size_limit(1 << 20)
            .build()
            .map(|regex| Self(std::sync::Arc::new(regex)))
            .map_err(|error| match error {
                regex::Error::CompiledTooBig(_) => "the pattern is too large".to_owned(),
                _ => "the pattern is not a valid regular expression".to_owned(),
            })
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_str() == other.0.as_str()
    }
}

/// A checked formula.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Const(Value),
    Field(FieldRef),
    Prior(FieldRef),
    Changed(FieldRef),
    Unary(UnaryOp, Box<Node>),
    Binary(BinaryOp, Box<Node>, Box<Node>),
    Call(Function, Vec<Node>),
    /// `REGEX(text, 'pattern')`.
    Matches(Box<Node>, Pattern),
}

/// A parsed and type-checked formula, ready to evaluate.
#[derive(Debug, Clone, PartialEq)]
pub struct Formula {
    pub node: Node,
    pub ty: Type,
}

impl Formula {
    /// Every chain of lookups the formula follows, shortest first, so an
    /// environment can load the related records before evaluating.
    pub fn hop_chains(&self) -> Vec<Vec<String>> {
        fn walk(node: &Node, out: &mut BTreeSet<Vec<String>>) {
            match node {
                Node::Const(_) => {}
                Node::Field(field) | Node::Prior(field) | Node::Changed(field) => {
                    for end in 1..=field.hops.len() {
                        out.insert(field.hops[..end].to_vec());
                    }
                }
                Node::Unary(_, operand) => walk(operand, out),
                Node::Binary(_, left, right) => {
                    walk(left, out);
                    walk(right, out);
                }
                Node::Call(_, arguments) => {
                    arguments.iter().for_each(|argument| walk(argument, out))
                }
                Node::Matches(subject, _) => walk(subject, out),
            }
        }
        let mut chains = BTreeSet::new();
        walk(&self.node, &mut chains);
        let mut chains: Vec<_> = chains.into_iter().collect();
        chains.sort_by_key(Vec::len);
        chains
    }
}

/// Parses and checks a formula, requiring its result to be `expected`
/// when given.
pub fn compile(
    text: &str,
    catalog: &check::Catalog<'_>,
    expected: Option<Type>,
) -> Result<Formula, FormulaError> {
    let expr = parse::parse(text)?;
    let (node, ty) = catalog.check(&expr)?;
    if let Some(expected) = expected {
        if ty != expected && ty != Type::Null {
            return Err(FormulaError::new(
                1,
                format!(
                    "the formula must give {}, not {}",
                    expected.describe(),
                    ty.describe()
                ),
            ));
        }
    }
    Ok(Formula { node, ty })
}

/// What a formula reads.
pub trait Env {
    fn value(&self, field: &FieldRef) -> Value;
    /// The field's value before this change; `NULL` for a new record.
    fn prior(&self, field: &FieldRef) -> Value;
    fn is_new(&self) -> bool;
    fn now(&self) -> DateTime<Utc>;
}

/// Why a formula has no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvalError(pub &'static str);

/// A record's value for a field, as a formula sees it.
pub fn record_value(record: &Record, leaf: &Leaf) -> Value {
    let user = |id: Option<uuid::Uuid>| id.map_or(Value::Null, |id| Value::Text(id.to_string()));
    match leaf {
        Leaf::Standard(standard) => match standard {
            Standard::Id => Value::Text(record.public_id.clone()),
            Standard::Name => Value::Text(record.name.clone()),
            Standard::Owner => user(record.owner_id),
            Standard::CreatedBy => user(record.created_by),
            Standard::UpdatedBy => user(record.updated_by),
            Standard::CreatedAt => Value::DateTime(record.created_at),
            Standard::UpdatedAt => Value::DateTime(record.updated_at),
            Standard::TouchedAt => Value::DateTime(record.touched_at),
            Standard::DeletedAt => record.deleted_at.map_or(Value::Null, Value::DateTime),
            Standard::Version => Value::Number(record.version as f64),
            Standard::IsDeleted => Value::Bool(record.deleted_at.is_some()),
        },
        Leaf::Field {
            api_name,
            field_type,
        } => json_value(record.values.get(api_name), *field_type),
    }
}

fn json_value(value: Option<&Json>, field_type: FieldType) -> Value {
    let Some(value) = value else {
        return Value::Null;
    };
    match field_type {
        FieldType::Number | FieldType::Currency | FieldType::Percent => value
            .as_f64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            .map_or(Value::Null, Value::Number),
        FieldType::Boolean => value.as_bool().map_or(Value::Null, Value::Bool),
        FieldType::Date => value
            .as_str()
            .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
            .map_or(Value::Null, Value::Date),
        FieldType::DateTime => value
            .as_str()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map_or(Value::Null, |time| {
                Value::DateTime(time.with_timezone(&Utc))
            }),
        FieldType::MultiPicklist => Value::Multi(
            value
                .as_array()
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Json::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default(),
        ),
        _ => value
            .as_str()
            .map_or(Value::Null, |text| Value::Text(text.to_owned())),
    }
}

fn text(value: String) -> Result<Value, EvalError> {
    if value.chars().count() > limits::TEXT {
        return Err(EvalError("text longer than the long text limit"));
    }
    Ok(Value::Text(value))
}

fn number(value: f64) -> Result<Value, EvalError> {
    if value.is_finite() {
        Ok(Value::Number(value))
    } else {
        Err(EvalError("a number out of range"))
    }
}

fn days(count: f64) -> Result<Duration, EvalError> {
    // Whole days for dates; fractions are dropped, as in Salesforce.
    let whole = count.trunc();
    if whole.abs() > 1_000_000.0 {
        return Err(EvalError("a date out of range"));
    }
    Ok(Duration::days(whole as i64))
}

fn as_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Number(number) if number.fract() == 0.0 && number.abs() < 1e15 => {
            format!("{}", *number as i64)
        }
        Value::Number(number) => number.to_string(),
        Value::Text(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Date(date) => date.format("%Y-%m-%d").to_string(),
        Value::DateTime(time) => time.to_rfc3339_opts(SecondsFormat::Secs, true),
        Value::Multi(items) => items.join(";"),
    }
}

fn compare(left: &Value, right: &Value) -> Option<std::cmp::Ordering> {
    match (left, right) {
        (Value::Number(left), Value::Number(right)) => left.partial_cmp(right),
        (Value::Text(left), Value::Text(right)) => Some(left.cmp(right)),
        (Value::Date(left), Value::Date(right)) => Some(left.cmp(right)),
        (Value::DateTime(left), Value::DateTime(right)) => Some(left.cmp(right)),
        _ => None,
    }
}

/// Evaluates a checked formula.
pub fn evaluate(formula: &Formula, env: &dyn Env) -> Result<Value, EvalError> {
    eval(&formula.node, env)
}

fn eval(node: &Node, env: &dyn Env) -> Result<Value, EvalError> {
    Ok(match node {
        Node::Const(value) => value.clone(),
        Node::Field(field) => env.value(field),
        Node::Prior(field) => {
            if env.is_new() {
                Value::Null
            } else {
                env.prior(field)
            }
        }
        Node::Changed(field) => Value::Bool(!env.is_new() && env.value(field) != env.prior(field)),
        Node::Unary(UnaryOp::Negate, operand) => match eval(operand, env)? {
            Value::Number(value) => Value::Number(-value),
            _ => Value::Null,
        },
        Node::Unary(UnaryOp::Not, operand) => Value::Bool(!eval(operand, env)?.truthy()),
        Node::Binary(BinaryOp::And, left, right) => {
            Value::Bool(eval(left, env)?.truthy() && eval(right, env)?.truthy())
        }
        Node::Binary(BinaryOp::Or, left, right) => {
            Value::Bool(eval(left, env)?.truthy() || eval(right, env)?.truthy())
        }
        Node::Binary(op, left, right) => binary(*op, eval(left, env)?, eval(right, env)?)?,
        Node::Call(function, arguments) => call(*function, arguments, env)?,
        Node::Matches(subject, pattern) => {
            Value::Bool(pattern.0.is_match(&as_text(&eval(subject, env)?)))
        }
    })
}

fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, EvalError> {
    use std::cmp::Ordering::*;
    Ok(match op {
        BinaryOp::Concat => text(as_text(&left) + &as_text(&right))?,
        BinaryOp::Eq => Value::Bool(left == right),
        BinaryOp::Ne => Value::Bool(left != right),
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => match compare(&left, &right) {
            None => Value::Null,
            Some(ordering) => Value::Bool(match op {
                BinaryOp::Lt => ordering == Less,
                BinaryOp::Le => ordering != Greater,
                BinaryOp::Gt => ordering == Greater,
                _ => ordering != Less,
            }),
        },
        _ => match (op, left, right) {
            (_, Value::Null, _) | (_, _, Value::Null) => Value::Null,
            (BinaryOp::Add, Value::Number(left), Value::Number(right)) => number(left + right)?,
            (BinaryOp::Subtract, Value::Number(left), Value::Number(right)) => {
                number(left - right)?
            }
            (BinaryOp::Multiply, Value::Number(left), Value::Number(right)) => {
                number(left * right)?
            }
            (BinaryOp::Divide, Value::Number(_), Value::Number(0.0)) => {
                return Err(EvalError("division by zero"))
            }
            (BinaryOp::Divide, Value::Number(left), Value::Number(right)) => number(left / right)?,
            (BinaryOp::Add, Value::Date(date), Value::Number(count))
            | (BinaryOp::Add, Value::Number(count), Value::Date(date)) => Value::Date(
                date.checked_add_signed(days(count)?)
                    .ok_or(EvalError("a date out of range"))?,
            ),
            (BinaryOp::Subtract, Value::Date(date), Value::Number(count)) => Value::Date(
                date.checked_sub_signed(days(count)?)
                    .ok_or(EvalError("a date out of range"))?,
            ),
            (BinaryOp::Subtract, Value::Date(left), Value::Date(right)) => {
                Value::Number((left - right).num_days() as f64)
            }
            (BinaryOp::Add, Value::DateTime(time), Value::Number(count))
            | (BinaryOp::Add, Value::Number(count), Value::DateTime(time)) => {
                Value::DateTime(shift(time, count)?)
            }
            (BinaryOp::Subtract, Value::DateTime(time), Value::Number(count)) => {
                Value::DateTime(shift(time, -count)?)
            }
            (BinaryOp::Subtract, Value::DateTime(left), Value::DateTime(right)) => {
                Value::Number((left - right).num_seconds() as f64 / 86_400.0)
            }
            _ => Value::Null,
        },
    })
}

fn shift(time: DateTime<Utc>, count: f64) -> Result<DateTime<Utc>, EvalError> {
    let seconds = count * 86_400.0;
    if !seconds.is_finite() || seconds.abs() > 1e11 {
        return Err(EvalError("a date out of range"));
    }
    time.checked_add_signed(Duration::seconds(seconds.round() as i64))
        .ok_or(EvalError("a date out of range"))
}

fn whole(value: f64) -> i64 {
    value.trunc().clamp(-1e12, 1e12) as i64
}

fn call(function: Function, arguments: &[Node], env: &dyn Env) -> Result<Value, EvalError> {
    let arg = |index: usize| eval(&arguments[index], env);
    let num = |value: &Value| match value {
        Value::Number(number) => Some(*number),
        _ => None,
    };
    let string = |value: Value| match value {
        Value::Text(text) => Some(text),
        _ => None,
    };
    Ok(match function {
        Function::If => {
            if arg(0)?.truthy() {
                arg(1)?
            } else {
                arg(2)?
            }
        }
        Function::Case => {
            let subject = arg(0)?;
            let last = arguments.len() - 1;
            for pair in (1..last).step_by(2) {
                if arg(pair)? == subject {
                    return arg(pair + 1);
                }
            }
            arg(last)?
        }
        Function::And => {
            for index in 0..arguments.len() {
                if !arg(index)?.truthy() {
                    return Ok(Value::Bool(false));
                }
            }
            Value::Bool(true)
        }
        Function::Or => {
            for index in 0..arguments.len() {
                if arg(index)?.truthy() {
                    return Ok(Value::Bool(true));
                }
            }
            Value::Bool(false)
        }
        Function::Not => Value::Bool(!arg(0)?.truthy()),
        Function::IsBlank => Value::Bool(arg(0)?.is_blank()),
        Function::BlankValue => {
            let value = arg(0)?;
            if value.is_blank() {
                arg(1)?
            } else {
                value
            }
        }
        Function::IsPickval => Value::Bool(matches!(
            (arg(0)?, arg(1)?),
            (Value::Text(left), Value::Text(right)) if left == right
        )),
        Function::Includes => Value::Bool(match (arg(0)?, arg(1)?) {
            (Value::Multi(items), Value::Text(item)) => items.contains(&item),
            _ => false,
        }),
        Function::Len => {
            Value::Number(string(arg(0)?).map_or(0, |text| text.chars().count()) as f64)
        }
        Function::Lower => {
            string(arg(0)?).map_or(Value::Null, |text| Value::Text(text.to_lowercase()))
        }
        Function::Upper => {
            string(arg(0)?).map_or(Value::Null, |text| Value::Text(text.to_uppercase()))
        }
        Function::Trim => {
            string(arg(0)?).map_or(Value::Null, |text| Value::Text(text.trim().to_owned()))
        }
        Function::Left | Function::Right => {
            let (Some(text), Some(count)) = (string(arg(0)?), num(&arg(1)?)) else {
                return Ok(Value::Null);
            };
            let count = whole(count).max(0) as usize;
            let length = text.chars().count();
            Value::Text(if function == Function::Left {
                text.chars().take(count).collect()
            } else {
                text.chars().skip(length.saturating_sub(count)).collect()
            })
        }
        Function::Mid => {
            let (Some(text), Some(start), Some(count)) =
                (string(arg(0)?), num(&arg(1)?), num(&arg(2)?))
            else {
                return Ok(Value::Null);
            };
            let start = whole(start).max(1) as usize - 1;
            Value::Text(
                text.chars()
                    .skip(start)
                    .take(whole(count).max(0) as usize)
                    .collect(),
            )
        }
        Function::Contains | Function::Begins => {
            let (Some(text), Some(part)) = (string(arg(0)?), string(arg(1)?)) else {
                return Ok(Value::Bool(false));
            };
            Value::Bool(if function == Function::Contains {
                text.contains(&part)
            } else {
                text.starts_with(&part)
            })
        }
        Function::Substitute => {
            let (Some(text), Some(old), new) = (string(arg(0)?), string(arg(1)?), arg(2)?) else {
                return Ok(Value::Null);
            };
            let new = as_text(&new);
            if old.is_empty() {
                return Ok(Value::Text(text));
            }
            // Check the size before building it.
            let count = text.matches(&old).count();
            let growth = new.chars().count().saturating_sub(old.chars().count());
            if text.chars().count() + count.saturating_mul(growth) > limits::TEXT {
                return Err(EvalError("text longer than the long text limit"));
            }
            Value::Text(text.replace(&old, &new))
        }
        Function::Text => Value::Text(as_text(&arg(0)?)),
        Function::Value => match string(arg(0)?) {
            None => Value::Null,
            Some(text) => number(
                text.trim()
                    .parse::<f64>()
                    .map_err(|_| EvalError("text that is not a number"))?,
            )?,
        },
        Function::Abs | Function::Floor | Function::Ceiling => match num(&arg(0)?) {
            None => Value::Null,
            Some(value) => Value::Number(match function {
                Function::Abs => value.abs(),
                Function::Floor => value.floor(),
                _ => value.ceil(),
            }),
        },
        Function::Round => {
            let (Some(value), Some(places)) = (num(&arg(0)?), num(&arg(1)?)) else {
                return Ok(Value::Null);
            };
            let factor = 10f64.powi(whole(places).clamp(-10, 10) as i32);
            number((value * factor).round() / factor)?
        }
        Function::Mod => {
            let (Some(value), Some(divisor)) = (num(&arg(0)?), num(&arg(1)?)) else {
                return Ok(Value::Null);
            };
            if divisor == 0.0 {
                return Err(EvalError("division by zero"));
            }
            number(value % divisor)?
        }
        Function::Min | Function::Max => {
            let mut best: Option<f64> = None;
            for index in 0..arguments.len() {
                let Some(value) = num(&arg(index)?) else {
                    return Ok(Value::Null);
                };
                best = Some(match best {
                    None => value,
                    Some(current) if function == Function::Min => current.min(value),
                    Some(current) => current.max(value),
                });
            }
            best.map_or(Value::Null, Value::Number)
        }
        Function::Today => Value::Date(env.now().date_naive()),
        Function::Now => Value::DateTime(env.now()),
        Function::Date => {
            let (Some(year), Some(month), Some(day)) =
                (num(&arg(0)?), num(&arg(1)?), num(&arg(2)?))
            else {
                return Ok(Value::Null);
            };
            let date = i32::try_from(whole(year)).ok().and_then(|year| {
                NaiveDate::from_ymd_opt(
                    year,
                    u32::try_from(whole(month)).ok()?,
                    u32::try_from(whole(day)).ok()?,
                )
            });
            Value::Date(date.ok_or(EvalError("an invalid date"))?)
        }
        Function::DateValue => match arg(0)? {
            Value::Null => Value::Null,
            Value::Date(date) => Value::Date(date),
            Value::DateTime(time) => Value::Date(time.date_naive()),
            Value::Text(text) => Value::Date(
                NaiveDate::parse_from_str(text.trim(), "%Y-%m-%d")
                    .map_err(|_| EvalError("an invalid date"))?,
            ),
            _ => Value::Null,
        },
        Function::Year | Function::Month | Function::Day => match arg(0)? {
            Value::Date(date) => Value::Number(f64::from(match function {
                Function::Year => date.year() as u32,
                Function::Month => date.month(),
                _ => date.day(),
            })),
            _ => Value::Null,
        },
        Function::AddMonths => {
            let (Value::Date(date), Some(count)) = (arg(0)?, num(&arg(1)?)) else {
                return Ok(Value::Null);
            };
            let count = whole(count);
            let months = Months::new(
                u32::try_from(count.unsigned_abs())
                    .map_err(|_| EvalError("a date out of range"))?,
            );
            Value::Date(
                if count >= 0 {
                    date.checked_add_months(months)
                } else {
                    date.checked_sub_months(months)
                }
                .ok_or(EvalError("a date out of range"))?,
            )
        }
        Function::IsNew => Value::Bool(env.is_new()),
        Function::IsChanged | Function::PriorValue | Function::Regex => Value::Null,
        Function::IsNumber => Value::Bool(
            string(arg(0)?)
                .is_some_and(|text| text.trim().parse::<f64>().is_ok_and(f64::is_finite)),
        ),
        Function::Find => {
            let (Some(part), Some(text)) = (string(arg(0)?), string(arg(1)?)) else {
                return Ok(Value::Number(0.0));
            };
            let start = match arguments.get(2) {
                Some(_) => match num(&arg(2)?) {
                    Some(start) => whole(start),
                    None => return Ok(Value::Number(0.0)),
                },
                None => 1,
            };
            if part.is_empty() || start < 1 {
                return Ok(Value::Number(0.0));
            }
            let chars: Vec<char> = text.chars().collect();
            let wanted: Vec<char> = part.chars().collect();
            let found = (start as usize - 1..chars.len())
                .find(|&at| chars[at..].starts_with(&wanted))
                .map_or(0, |at| at + 1);
            Value::Number(found as f64)
        }
        Function::Lpad | Function::Rpad => {
            let (Some(text), Some(length)) = (string(arg(0)?), num(&arg(1)?)) else {
                return Ok(Value::Null);
            };
            let pad = match arguments.get(2) {
                Some(_) => string(arg(2)?).unwrap_or_default(),
                None => " ".to_owned(),
            };
            let length = whole(length).max(0) as usize;
            if length > limits::TEXT {
                return Err(EvalError("text longer than the long text limit"));
            }
            let current = text.chars().count();
            if current >= length || pad.is_empty() {
                return Ok(Value::Text(text.chars().take(length).collect()));
            }
            let filler: String = pad.chars().cycle().take(length - current).collect();
            Value::Text(if function == Function::Lpad {
                filler + &text
            } else {
                text + &filler
            })
        }
        Function::Sqrt | Function::Exp | Function::Ln | Function::Log => {
            let Some(value) = num(&arg(0)?) else {
                return Ok(Value::Null);
            };
            let result = match function {
                Function::Sqrt if value < 0.0 => {
                    return Err(EvalError("the square root of a negative number"))
                }
                Function::Sqrt => value.sqrt(),
                Function::Exp => value.exp(),
                _ if value <= 0.0 => {
                    return Err(EvalError("the logarithm of zero or a negative number"))
                }
                Function::Ln => value.ln(),
                _ => value.log10(),
            };
            number(result)?
        }
        Function::Trunc => {
            let Some(value) = num(&arg(0)?) else {
                return Ok(Value::Null);
            };
            let places = match arguments.get(1) {
                Some(_) => match num(&arg(1)?) {
                    Some(places) => whole(places).clamp(-10, 10) as i32,
                    None => return Ok(Value::Null),
                },
                None => 0,
            };
            let factor = 10f64.powi(places);
            number((value * factor).trunc() / factor)?
        }
        Function::Weekday => match arg(0)? {
            Value::Date(date) => Value::Number(f64::from(date.weekday().number_from_sunday())),
            _ => Value::Null,
        },
        Function::DateTimeValue => match arg(0)? {
            Value::Null => Value::Null,
            Value::DateTime(time) => Value::DateTime(time),
            Value::Date(date) => Value::DateTime(
                date.and_hms_opt(0, 0, 0)
                    .ok_or(EvalError("an invalid date"))?
                    .and_utc(),
            ),
            Value::Text(text) => {
                let text = text.trim();
                let parsed = DateTime::parse_from_rfc3339(text)
                    .map(|time| time.with_timezone(&Utc))
                    .or_else(|_| {
                        chrono::NaiveDateTime::parse_from_str(text, "%Y-%m-%d %H:%M:%S")
                            .map(|time| time.and_utc())
                    })
                    .map_err(|_| EvalError("an invalid date-time"))?;
                Value::DateTime(parsed)
            }
            _ => Value::Null,
        },
    })
}

#[cfg(test)]
mod tests;
