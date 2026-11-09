//! Type-checks a formula against object metadata (ADR 0021). The result
//! names every field by id-free references the evaluator can read, every
//! function by enum, and carries the formula's type.

use super::parse::{self, BinaryOp, Expr, ExprKind, FormulaError, UnaryOp};
use super::{limits, FieldRef, Function, Leaf, Node, Type, Value};
use crate::query::plan::Standard;
use crate::{FieldType, ObjectDefinition};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use uuid::Uuid;

/// Where a formula runs, which decides the functions it may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Part of a save: validation rule conditions and automation formulas,
    /// which may compare with the previous values.
    Save,
    /// A formula field: computed from current values only.
    Field,
}

pub(crate) fn field_type(kind: FieldType) -> Type {
    match kind {
        FieldType::Number | FieldType::Currency | FieldType::Percent => Type::Number,
        FieldType::Boolean => Type::Boolean,
        FieldType::Date => Type::Date,
        FieldType::DateTime => Type::DateTime,
        FieldType::MultiPicklist => Type::Multi,
        _ => Type::Text,
    }
}

/// Formulas cannot use a related-to field: its target may be a record of
/// any of several objects (ADR 0033 §1).
fn related_to_unsupported(name: &str, position: usize) -> FormulaError {
    FormulaError::new(
        position,
        format!("{name} is a related-to field, which formulas cannot use yet"),
    )
}

fn standard_type(standard: Standard) -> Type {
    match standard {
        Standard::CreatedAt | Standard::UpdatedAt | Standard::TouchedAt | Standard::DeletedAt => {
            Type::DateTime
        }
        Standard::Version => Type::Number,
        Standard::IsDeleted => Type::Boolean,
        _ => Type::Text,
    }
}

/// The type both sides share, letting `NULL` stand for any type.
pub(crate) fn unify(left: Type, right: Type) -> Option<Type> {
    match (left, right) {
        (Type::Null, other) | (other, Type::Null) => Some(other),
        (left, right) if left == right => Some(left),
        _ => None,
    }
}

/// The metadata a formula is checked against.
pub struct Catalog<'a> {
    pub base: &'a ObjectDefinition,
    /// Objects lookups may lead to, by id.
    pub objects: &'a BTreeMap<Uuid, ObjectDefinition>,
    pub purpose: Purpose,
    /// Formula fields being inlined, shared with the catalogs that check
    /// them.
    expansion: Rc<RefCell<Expansion>>,
}

/// The formula fields a check is inside of, against cycles, and the nodes
/// inlined formulas may still add.
struct Expansion {
    stack: Vec<Uuid>,
    budget: usize,
}

/// A formula field may use formula fields this many levels deep.
pub const INLINE_DEPTH: usize = 10;
/// Nodes the formula fields a formula uses may add together.
pub const INLINE_NODES: usize = 2_000;

/// The formula type a formula field's result type stands for.
pub fn result_type(formula_type: FieldType) -> Type {
    match formula_type {
        FieldType::Number | FieldType::Currency | FieldType::Percent => Type::Number,
        FieldType::Boolean => Type::Boolean,
        FieldType::Date => Type::Date,
        FieldType::DateTime => Type::DateTime,
        _ => Type::Text,
    }
}

/// What a path names: a stored field, or a formula field's checked
/// formula, inlined.
enum Reference {
    Field(FieldRef),
    Inlined(Node, Type),
}

fn count(node: &Node) -> usize {
    1 + match node {
        Node::Const(_) | Node::Field(_) | Node::Prior(_) | Node::Changed(_) => 0,
        Node::Unary(_, operand) => count(operand),
        Node::Binary(_, left, right) => count(left) + count(right),
        Node::Call(_, arguments) => arguments.iter().map(count).sum(),
        Node::Matches(subject, _) => count(subject),
    }
}

/// Moves a formula checked on a related object onto the record that
/// reaches it through `through`. `None` when that follows too many lookups.
fn rebase(node: Node, through: &[String]) -> Option<Node> {
    let field = |mut field: FieldRef| {
        let mut hops = through.to_vec();
        hops.append(&mut field.hops);
        (hops.len() <= limits::HOPS).then(|| {
            field.hops = hops;
            field
        })
    };
    Some(match node {
        Node::Const(value) => Node::Const(value),
        Node::Field(reference) => Node::Field(field(reference)?),
        Node::Prior(reference) => Node::Prior(field(reference)?),
        Node::Changed(reference) => Node::Changed(field(reference)?),
        Node::Unary(op, operand) => Node::Unary(op, Box::new(rebase(*operand, through)?)),
        Node::Binary(op, left, right) => Node::Binary(
            op,
            Box::new(rebase(*left, through)?),
            Box::new(rebase(*right, through)?),
        ),
        Node::Call(function, arguments) => Node::Call(
            function,
            arguments
                .into_iter()
                .map(|argument| rebase(argument, through))
                .collect::<Option<_>>()?,
        ),
        Node::Matches(subject, pattern) => {
            Node::Matches(Box::new(rebase(*subject, through)?), pattern)
        }
    })
}

/// A formula field read through lookups is blank when the related record
/// is: an empty lookup, one in the recycle bin, or one the reader cannot
/// see. Its expression alone could still give a value then, as a constant
/// or `BLANKVALUE` would, so it is wrapped in a check of the related id.
fn guard(node: Node, through: &[String]) -> Node {
    if through.is_empty() {
        return node;
    }
    let related = Node::Field(FieldRef {
        hops: through.to_vec(),
        leaf: Leaf::Standard(Standard::Id),
        ty: standard_type(Standard::Id),
    });
    Node::Call(
        Function::If,
        vec![
            Node::Call(Function::IsBlank, vec![related]),
            Node::Const(Value::Null),
            node,
        ],
    )
}

impl<'a> Catalog<'a> {
    pub fn new(
        base: &'a ObjectDefinition,
        objects: &'a BTreeMap<Uuid, ObjectDefinition>,
        purpose: Purpose,
    ) -> Self {
        Self {
            base,
            objects,
            purpose,
            expansion: Rc::new(RefCell::new(Expansion {
                stack: Vec::new(),
                budget: INLINE_NODES,
            })),
        }
    }

    /// A catalog for the new formula of the formula field `field`, so a
    /// formula that comes back to it through other formulas is a cycle.
    pub fn editing(
        base: &'a ObjectDefinition,
        objects: &'a BTreeMap<Uuid, ObjectDefinition>,
        field: Uuid,
    ) -> Self {
        let catalog = Self::new(base, objects, Purpose::Field);
        catalog.expansion.borrow_mut().stack.push(field);
        catalog
    }

    /// Checks the formula of a formula field reached through `through`.
    fn inline(
        &self,
        object: &ObjectDefinition,
        field: &crate::FieldDefinition,
        through: &[String],
        position: usize,
    ) -> Result<Reference, FormulaError> {
        let name = &field.api_name;
        let fail = |detail: String| FormulaError::new(position, detail);
        let (Some(text), Some(kind)) = (&field.formula, field.formula_type) else {
            return Err(fail(format!("{name} has no formula")));
        };
        {
            let mut expansion = self.expansion.borrow_mut();
            if expansion.stack.contains(&field.id) {
                return Err(fail(format!(
                    "{name} uses this formula, so using it would go round in a circle"
                )));
            }
            if expansion.stack.len() >= INLINE_DEPTH {
                return Err(fail(format!(
                    "formulas can use other formulas at most {INLINE_DEPTH} levels deep"
                )));
            }
            expansion.stack.push(field.id);
        }
        let budget_before = self.expansion.borrow().budget;
        let inner = Catalog {
            base: object,
            objects: self.objects,
            purpose: Purpose::Field,
            expansion: Rc::clone(&self.expansion),
        };
        let checked = parse::parse(text).and_then(|expr| inner.check(&expr));
        self.expansion.borrow_mut().stack.pop();
        let (node, ty) =
            checked.map_err(|error| fail(format!("{name} cannot be used: {}", error.message)))?;
        let expected = result_type(kind);
        if unify(ty, expected).is_none() {
            return Err(fail(format!(
                "{name} cannot be used: its formula no longer gives {}",
                expected.describe()
            )));
        }
        {
            // Formula fields this one uses were charged as they were
            // inlined; charge only the nodes this level adds, so the budget
            // bounds the final expansion rather than the sum of its parts.
            let mut expansion = self.expansion.borrow_mut();
            let nested = budget_before.saturating_sub(expansion.budget);
            let size = count(&node).saturating_sub(nested);
            if size > expansion.budget {
                return Err(fail(
                    "the formula fields it uses are too large together".into(),
                ));
            }
            expansion.budget -= size;
        }
        let node = rebase(node, through).ok_or_else(|| {
            fail(format!(
                "follow at most {} lookups, counting those of the formula fields it uses",
                limits::HOPS
            ))
        })?;
        Ok(Reference::Inlined(guard(node, through), expected))
    }

    fn path(&self, segments: &[String], position: usize) -> Result<Reference, FormulaError> {
        if segments.len() > limits::HOPS + 1 {
            return Err(FormulaError::new(
                position,
                format!("follow at most {} lookups", limits::HOPS),
            ));
        }
        let mut object = self.base;
        let (last, through) = segments
            .split_last()
            .ok_or_else(|| FormulaError::new(position, "expected a field"))?;
        for segment in through {
            let field = object.field(segment).ok_or_else(|| {
                FormulaError::new(
                    position,
                    format!("{} has no field {segment}", object.api_name),
                )
            })?;
            if field.field_type == FieldType::RelatedTo {
                return Err(related_to_unsupported(segment, position));
            }
            if !field.field_type.is_reference() {
                return Err(FormulaError::new(
                    position,
                    format!("{segment} is not a lookup, so it cannot be followed"),
                ));
            }
            object = field
                .reference_object_id
                .and_then(|id| self.objects.get(&id))
                .ok_or_else(|| {
                    FormulaError::new(position, format!("{segment} points to an unknown object"))
                })?;
        }
        let leaf = match Standard::parse(last) {
            Some(standard) => Leaf::Standard(standard),
            None => {
                let field = object.field(last).ok_or_else(|| {
                    FormulaError::new(position, format!("{} has no field {last}", object.api_name))
                })?;
                if field.field_type == FieldType::Formula {
                    return self.inline(object, field, through, position);
                }
                if field.field_type == FieldType::RelatedTo {
                    return Err(related_to_unsupported(last, position));
                }
                Leaf::Field {
                    api_name: field.api_name.clone(),
                    field_type: field.value_type(),
                }
            }
        };
        let ty = match &leaf {
            Leaf::Standard(standard) => standard_type(*standard),
            Leaf::Field { field_type, .. } => self::field_type(*field_type),
        };
        Ok(Reference::Field(FieldRef {
            hops: through.to_vec(),
            leaf,
            ty,
        }))
    }

    pub fn check(&self, expr: &Expr) -> Result<(Node, Type), FormulaError> {
        let position = expr.position;
        let mismatch = |what: String| FormulaError::new(position, what);
        Ok(match &expr.kind {
            ExprKind::Number(number) => (Node::Const(Value::Number(*number)), Type::Number),
            ExprKind::Text(text) => (Node::Const(Value::Text(text.clone())), Type::Text),
            ExprKind::Bool(flag) => (Node::Const(Value::Bool(*flag)), Type::Boolean),
            ExprKind::Null => (Node::Const(Value::Null), Type::Null),
            ExprKind::Path(segments) => match self.path(segments, position)? {
                Reference::Field(field) => {
                    let ty = field.ty;
                    (Node::Field(field), ty)
                }
                Reference::Inlined(node, ty) => (node, ty),
            },
            ExprKind::Unary(op, operand) => {
                let (node, ty) = self.check(operand)?;
                let expected = match op {
                    UnaryOp::Negate => Type::Number,
                    UnaryOp::Not => Type::Boolean,
                };
                if unify(ty, expected).is_none() {
                    return Err(mismatch(format!(
                        "{} needs {}, not {}",
                        if *op == UnaryOp::Negate { "-" } else { "!" },
                        expected.describe(),
                        ty.describe()
                    )));
                }
                (Node::Unary(*op, Box::new(node)), expected)
            }
            ExprKind::Binary(op, left, right) => {
                let (left_node, left_ty) = self.check(left)?;
                let (right_node, right_ty) = self.check(right)?;
                let ty = binary_type(*op, left_ty, right_ty).ok_or_else(|| {
                    mismatch(format!(
                        "'{}' cannot combine {} and {}",
                        op.symbol(),
                        left_ty.describe(),
                        right_ty.describe()
                    ))
                })?;
                (
                    Node::Binary(*op, Box::new(left_node), Box::new(right_node)),
                    ty,
                )
            }
            ExprKind::Call(name, arguments) => self.call(name, arguments, position)?,
        })
    }

    fn call(
        &self,
        name: &str,
        arguments: &[Expr],
        position: usize,
    ) -> Result<(Node, Type), FormulaError> {
        let function = Function::parse(name)
            .ok_or_else(|| FormulaError::new(position, format!("{name} is not a function")))?;
        let arity = |min: usize, max: Option<usize>| -> Result<(), FormulaError> {
            let count = arguments.len();
            if count < min || max.is_some_and(|max| count > max) {
                let expected = match max {
                    Some(max) if max == min => format!("{min}"),
                    Some(max) => format!("{min} to {max}"),
                    None => format!("at least {min}"),
                };
                return Err(FormulaError::new(
                    position,
                    format!("{name} takes {expected} arguments, not {count}"),
                ));
            }
            Ok(())
        };
        if matches!(
            function,
            Function::IsNew | Function::IsChanged | Function::PriorValue
        ) && self.purpose != Purpose::Save
        {
            return Err(FormulaError::new(
                position,
                format!("{name} can only be used in validation rules and automations"),
            ));
        }
        // Functions that read a field directly, not a value.
        if matches!(function, Function::IsChanged | Function::PriorValue) {
            arity(1, Some(1))?;
            let ExprKind::Path(segments) = &arguments[0].kind else {
                return Err(FormulaError::new(
                    arguments[0].position,
                    format!("{name} needs a field of this record"),
                ));
            };
            if segments.len() != 1 {
                return Err(FormulaError::new(
                    arguments[0].position,
                    format!("{name} needs a field of this record, not of a related record"),
                ));
            }
            let Reference::Field(field) = self.path(segments, arguments[0].position)? else {
                return Err(FormulaError::new(
                    arguments[0].position,
                    format!("{name} needs a stored field, not a formula field"),
                ));
            };
            let ty = if function == Function::IsChanged {
                Type::Boolean
            } else {
                field.ty
            };
            return Ok((
                if function == Function::IsChanged {
                    Node::Changed(field)
                } else {
                    Node::Prior(field)
                },
                ty,
            ));
        }
        if function == Function::Regex {
            arity(2, Some(2))?;
            let ExprKind::Text(pattern) = &arguments[1].kind else {
                return Err(FormulaError::new(
                    arguments[1].position,
                    "REGEX needs its pattern written as text in quotes",
                ));
            };
            let pattern = super::Pattern::new(pattern)
                .map_err(|detail| FormulaError::new(arguments[1].position, detail))?;
            let (subject, ty) = self.check(&arguments[0])?;
            if unify(ty, Type::Text).is_none() {
                return Err(FormulaError::new(
                    arguments[0].position,
                    format!("REGEX needs text here, not {}", ty.describe()),
                ));
            }
            return Ok((Node::Matches(Box::new(subject), pattern), Type::Boolean));
        }
        let mut nodes = Vec::new();
        let mut types = Vec::new();
        for argument in arguments {
            let (node, ty) = self.check(argument)?;
            nodes.push(node);
            types.push(ty);
        }
        let want = |index: usize, expected: Type| -> Result<(), FormulaError> {
            let actual = types[index];
            if unify(actual, expected).is_some() {
                Ok(())
            } else {
                Err(FormulaError::new(
                    arguments[index].position,
                    format!(
                        "{name} needs {} here, not {}",
                        expected.describe(),
                        actual.describe()
                    ),
                ))
            }
        };
        let same = |indices: &[usize]| -> Result<Type, FormulaError> {
            let mut ty = Type::Null;
            for index in indices {
                ty = unify(ty, types[*index]).ok_or_else(|| {
                    FormulaError::new(
                        arguments[*index].position,
                        format!(
                            "{name} needs values of one type; {} does not match {}",
                            types[*index].describe(),
                            ty.describe()
                        ),
                    )
                })?;
            }
            Ok(ty)
        };
        let ty = match function {
            Function::If => {
                arity(3, Some(3))?;
                want(0, Type::Boolean)?;
                same(&[1, 2])?
            }
            Function::Case => {
                arity(4, None)?;
                if !arguments.len().is_multiple_of(2) {
                    return Err(FormulaError::new(
                        position,
                        "CASE takes a value, pairs of match and result, then a default",
                    ));
                }
                let matches: Vec<usize> = (1..arguments.len() - 1).step_by(2).collect();
                let results: Vec<usize> = (2..arguments.len() - 1)
                    .step_by(2)
                    .chain([arguments.len() - 1])
                    .collect();
                let mut keys = vec![0];
                keys.extend(&matches);
                same(&keys)?;
                same(&results)?
            }
            Function::And | Function::Or => {
                arity(1, None)?;
                for index in 0..types.len() {
                    want(index, Type::Boolean)?;
                }
                Type::Boolean
            }
            Function::Not => {
                arity(1, Some(1))?;
                want(0, Type::Boolean)?;
                Type::Boolean
            }
            Function::IsBlank => {
                arity(1, Some(1))?;
                Type::Boolean
            }
            Function::BlankValue => {
                arity(2, Some(2))?;
                same(&[0, 1])?
            }
            Function::IsPickval | Function::Includes => {
                arity(2, Some(2))?;
                want(
                    0,
                    if function == Function::Includes {
                        Type::Multi
                    } else {
                        Type::Text
                    },
                )?;
                want(1, Type::Text)?;
                Type::Boolean
            }
            Function::Len => {
                arity(1, Some(1))?;
                want(0, Type::Text)?;
                Type::Number
            }
            Function::Lower | Function::Upper | Function::Trim => {
                arity(1, Some(1))?;
                want(0, Type::Text)?;
                Type::Text
            }
            Function::Left | Function::Right => {
                arity(2, Some(2))?;
                want(0, Type::Text)?;
                want(1, Type::Number)?;
                Type::Text
            }
            Function::Mid => {
                arity(3, Some(3))?;
                want(0, Type::Text)?;
                want(1, Type::Number)?;
                want(2, Type::Number)?;
                Type::Text
            }
            Function::Contains | Function::Begins => {
                arity(2, Some(2))?;
                want(0, Type::Text)?;
                want(1, Type::Text)?;
                Type::Boolean
            }
            Function::Substitute => {
                arity(3, Some(3))?;
                for index in 0..3 {
                    want(index, Type::Text)?;
                }
                Type::Text
            }
            Function::Text => {
                arity(1, Some(1))?;
                if types[0] == Type::Multi {
                    want(0, Type::Text)?;
                }
                Type::Text
            }
            Function::Value => {
                arity(1, Some(1))?;
                want(0, Type::Text)?;
                Type::Number
            }
            Function::Abs | Function::Floor | Function::Ceiling => {
                arity(1, Some(1))?;
                want(0, Type::Number)?;
                Type::Number
            }
            Function::Round | Function::Mod => {
                arity(2, Some(2))?;
                want(0, Type::Number)?;
                want(1, Type::Number)?;
                Type::Number
            }
            Function::Min | Function::Max => {
                arity(1, None)?;
                for index in 0..types.len() {
                    want(index, Type::Number)?;
                }
                Type::Number
            }
            Function::Today => {
                arity(0, Some(0))?;
                Type::Date
            }
            Function::Now => {
                arity(0, Some(0))?;
                Type::DateTime
            }
            Function::Date => {
                arity(3, Some(3))?;
                for index in 0..3 {
                    want(index, Type::Number)?;
                }
                Type::Date
            }
            Function::DateValue => {
                arity(1, Some(1))?;
                if !matches!(
                    types[0],
                    Type::DateTime | Type::Text | Type::Date | Type::Null
                ) {
                    want(0, Type::DateTime)?;
                }
                Type::Date
            }
            Function::Year | Function::Month | Function::Day => {
                arity(1, Some(1))?;
                want(0, Type::Date)?;
                Type::Number
            }
            Function::AddMonths => {
                arity(2, Some(2))?;
                want(0, Type::Date)?;
                want(1, Type::Number)?;
                Type::Date
            }
            Function::IsNew => {
                arity(0, Some(0))?;
                Type::Boolean
            }
            Function::IsChanged | Function::PriorValue | Function::Regex => {
                return Err(FormulaError::new(position, "unreachable"))
            }
            Function::IsNumber => {
                arity(1, Some(1))?;
                want(0, Type::Text)?;
                Type::Boolean
            }
            Function::Find => {
                arity(2, Some(3))?;
                want(0, Type::Text)?;
                want(1, Type::Text)?;
                if types.len() == 3 {
                    want(2, Type::Number)?;
                }
                Type::Number
            }
            Function::Lpad | Function::Rpad => {
                arity(2, Some(3))?;
                want(0, Type::Text)?;
                want(1, Type::Number)?;
                if types.len() == 3 {
                    want(2, Type::Text)?;
                }
                Type::Text
            }
            Function::Sqrt | Function::Exp | Function::Ln | Function::Log => {
                arity(1, Some(1))?;
                want(0, Type::Number)?;
                Type::Number
            }
            Function::Trunc => {
                arity(1, Some(2))?;
                want(0, Type::Number)?;
                if types.len() == 2 {
                    want(1, Type::Number)?;
                }
                Type::Number
            }
            Function::Weekday => {
                arity(1, Some(1))?;
                want(0, Type::Date)?;
                Type::Number
            }
            Function::DateTimeValue => {
                arity(1, Some(1))?;
                if !matches!(
                    types[0],
                    Type::DateTime | Type::Text | Type::Date | Type::Null
                ) {
                    want(0, Type::Text)?;
                }
                Type::DateTime
            }
        };
        Ok((Node::Call(function, nodes), ty))
    }
}

fn binary_type(op: BinaryOp, left: Type, right: Type) -> Option<Type> {
    use Type::*;
    match op {
        BinaryOp::And | BinaryOp::Or => {
            (unify(left, Boolean).is_some() && unify(right, Boolean).is_some()).then_some(Boolean)
        }
        BinaryOp::Concat => {
            (unify(left, Text).is_some() && unify(right, Text).is_some()).then_some(Text)
        }
        BinaryOp::Eq | BinaryOp::Ne => {
            let shared = unify(left, right)?;
            (shared != Multi).then_some(Boolean)
        }
        BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
            let shared = unify(left, right)?;
            matches!(shared, Number | Text | Date | DateTime | Null).then_some(Boolean)
        }
        BinaryOp::Multiply | BinaryOp::Divide => {
            (unify(left, Number).is_some() && unify(right, Number).is_some()).then_some(Number)
        }
        BinaryOp::Add => match (left, right) {
            (Date, Number | Null) | (Number, Date) => Some(Date),
            (DateTime, Number | Null) | (Number, DateTime) => Some(DateTime),
            _ => {
                (unify(left, Number).is_some() && unify(right, Number).is_some()).then_some(Number)
            }
        },
        BinaryOp::Subtract => match (left, right) {
            (Date, Number) => Some(Date),
            (DateTime, Number) => Some(DateTime),
            (Date, Date) | (DateTime, DateTime) => Some(Number),
            _ => {
                (unify(left, Number).is_some() && unify(right, Number).is_some()).then_some(Number)
            }
        },
    }
}
