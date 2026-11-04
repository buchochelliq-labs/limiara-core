//! Formula fields (ADR 0021): values computed from a formula whenever a
//! record is read, never stored. Lookups apply the reader's sharing, so a
//! target the reader cannot see is blank, never its values.

use crate::access::ObjectAccess;
use crate::formula::check::{Catalog, Purpose};
use crate::formula::{self, Env, FieldRef, Formula, Value};
use crate::rules::formula_objects;
use crate::service::ObjectService;
use crate::{
    FieldDefinition, FieldType, ObjectDefinition, ObjectError, ObjectTx, Record, ValidationIssue,
};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use uuid::Uuid;

pub(crate) use crate::formula::check::result_type;

/// Compiles a formula field's formula. `field` is the formula field's own
/// id when it exists, so a formula that leads back to it is a cycle.
pub(crate) fn compile_field(
    formula: &str,
    formula_type: FieldType,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
    field: Option<Uuid>,
) -> Result<Formula, formula::parse::FormulaError> {
    let catalog = match field {
        Some(field) => Catalog::editing(base, objects, field),
        None => Catalog::new(base, objects, Purpose::Field),
    };
    formula::compile(formula, &catalog, Some(result_type(formula_type)))
}

/// Checks a formula field's formula against the object it belongs to.
pub(crate) async fn check_formula(
    tx: &mut dyn ObjectTx,
    object: &ObjectDefinition,
    formula: &str,
    formula_type: FieldType,
    field: Option<Uuid>,
) -> Result<(), ObjectError> {
    let objects = formula_objects(tx, object).await?;
    compile_field(formula, formula_type, object, &objects, field).map_err(|error| {
        ObjectError::Validation(vec![ValidationIssue::new(
            "formula",
            "objects.formula_invalid",
            format!("{} (at position {})", error.message, error.position),
        )])
    })?;
    Ok(())
}

struct ReadEnv<'a> {
    record: &'a Record,
    related: &'a BTreeMap<Vec<String>, Option<Record>>,
    now: DateTime<Utc>,
}

impl Env for ReadEnv<'_> {
    fn value(&self, field: &FieldRef) -> Value {
        let source = if field.hops.is_empty() {
            Some(self.record)
        } else {
            self.related.get(&field.hops).and_then(Option::as_ref)
        };
        source.map_or(Value::Null, |record| {
            formula::record_value(record, &field.leaf)
        })
    }

    fn prior(&self, _field: &FieldRef) -> Value {
        Value::Null
    }

    fn is_new(&self) -> bool {
        false
    }

    fn now(&self) -> DateTime<Utc> {
        self.now
    }
}

pub(crate) fn has_formulas(object: &ObjectDefinition) -> bool {
    object
        .fields
        .iter()
        .any(|field| field.is_active() && field.field_type == FieldType::Formula)
}

/// Rounds number results to the field's decimal places.
fn finish(field: &FieldDefinition, value: Value) -> serde_json::Value {
    match (value, field.scale) {
        (Value::Number(number), Some(scale)) => {
            let factor = 10f64.powi(scale);
            Value::Number((number * factor).round() / factor).to_json()
        }
        // Empty text is blank, as in Salesforce.
        (Value::Text(text), _) if text.is_empty() => serde_json::Value::Null,
        (value, _) => value.to_json(),
    }
}

impl ObjectService {
    /// Computes formula fields for records the caller has just read, in a
    /// read transaction of its own.
    pub(crate) async fn with_formulas(
        &self,
        object: &ObjectDefinition,
        mut records: Vec<Record>,
    ) -> Result<Vec<Record>, ObjectError> {
        if !has_formulas(object) || records.is_empty() {
            return Ok(records);
        }
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        self.compute_formulas(tx.as_mut(), &access, object, &mut records)
            .await?;
        tx.commit().await?;
        Ok(records)
    }

    /// Computes formula fields in the caller's transaction.
    pub(crate) async fn compute_formulas(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
        records: &mut [Record],
    ) -> Result<(), ObjectError> {
        let fields: Vec<&FieldDefinition> = object
            .fields
            .iter()
            .filter(|field| field.is_active() && field.field_type == FieldType::Formula)
            .collect();
        if fields.is_empty() || records.is_empty() {
            return Ok(());
        }
        let objects = formula_objects(tx, object).await?;
        let compiled: Vec<(&FieldDefinition, Option<Formula>)> = fields
            .iter()
            .map(|field| {
                let formula = match (&field.formula, field.formula_type) {
                    (Some(text), Some(kind)) => {
                        compile_field(text, kind, object, &objects, Some(field.id)).ok()
                    }
                    _ => None,
                };
                (*field, formula)
            })
            .collect();
        let mut chains: Vec<Vec<String>> = compiled
            .iter()
            .filter_map(|(_, formula)| formula.as_ref())
            .flat_map(Formula::hop_chains)
            .collect();
        chains.sort_by_key(Vec::len);
        chains.dedup();
        let now = Utc::now();
        let mut visible: BTreeMap<String, Option<Record>> = BTreeMap::new();
        let mut definitions = BTreeMap::new();
        for record in records.iter_mut() {
            let related = self
                .visible_related(
                    tx,
                    access,
                    object,
                    &objects,
                    record,
                    &chains,
                    &mut visible,
                    &mut definitions,
                )
                .await?;
            let env = ReadEnv {
                record,
                related: &related,
                now,
            };
            let values: Vec<(String, serde_json::Value)> = compiled
                .iter()
                .map(|(field, formula)| {
                    let value = formula
                        .as_ref()
                        .and_then(|formula| formula::evaluate(formula, &env).ok())
                        .unwrap_or(Value::Null);
                    (field.api_name.clone(), finish(field, value))
                })
                .collect();
            for (api_name, value) in values {
                if value.is_null() {
                    record.values.remove(&api_name);
                } else {
                    record.values.insert(api_name, value);
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn with_formula(
        &self,
        object: &ObjectDefinition,
        record: Record,
    ) -> Result<Record, ObjectError> {
        self.with_formulas(object, vec![record])
            .await?
            .pop()
            .ok_or(ObjectError::Invariant)
    }

    /// The records each chain of lookups reaches from `record`, as the
    /// caller sees them: hidden or deleted targets are `None`.
    #[allow(clippy::too_many_arguments)]
    async fn visible_related(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        base: &ObjectDefinition,
        objects: &BTreeMap<Uuid, ObjectDefinition>,
        record: &Record,
        chains: &[Vec<String>],
        visible: &mut BTreeMap<String, Option<Record>>,
        definitions: &mut BTreeMap<Uuid, Option<ObjectDefinition>>,
    ) -> Result<BTreeMap<Vec<String>, Option<Record>>, ObjectError> {
        let mut related: BTreeMap<Vec<String>, Option<Record>> = BTreeMap::new();
        for chain in chains {
            let Some((hop, through)) = chain.split_last() else {
                continue;
            };
            let parent = if through.is_empty() {
                Some(record.clone())
            } else {
                related.get(through).cloned().flatten()
            };
            let mut object = base;
            for segment in through {
                match object
                    .field(segment)
                    .and_then(|field| field.reference_object_id)
                    .and_then(|id| objects.get(&id))
                {
                    Some(target) => object = target,
                    None => break,
                }
            }
            let target_object = object
                .field(hop)
                .and_then(|field| field.reference_object_id);
            let public_id = parent.as_ref().and_then(|parent| {
                parent
                    .values
                    .get(hop)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            });
            let target = match (public_id, target_object) {
                (Some(public_id), Some(target_object)) => {
                    if !visible.contains_key(&public_id) {
                        let found = match tx.record_by_public_id(&public_id).await? {
                            Some(found)
                                if self.can_see(tx, access, definitions, &found).await? =>
                            {
                                Some(found)
                            }
                            _ => None,
                        };
                        visible.insert(public_id.clone(), found);
                    }
                    visible
                        .get(&public_id)
                        .cloned()
                        .flatten()
                        .filter(|found| found.object_id == target_object)
                }
                _ => None,
            };
            related.insert(chain.clone(), target);
        }
        Ok(related)
    }
}

/// A formula field whose value depends only on the record's own stored
/// fields, so it is stored with the record's index rows and queries can
/// filter, sort and group on it (ADR 0021 §6).
pub(crate) struct StoredFormula {
    field: FieldDefinition,
    formula: Formula,
}

/// Whether a checked formula reads only this record's fields, its ID, name
/// and creation stamps: values that change only when the record is saved.
fn reads_only_the_record(node: &formula::Node) -> bool {
    use crate::query::plan::Standard;
    use formula::{Function, Leaf, Node};
    match node {
        Node::Const(_) => true,
        Node::Field(field) | Node::Prior(field) | Node::Changed(field) => {
            field.hops.is_empty()
                && match &field.leaf {
                    Leaf::Field { .. } => true,
                    Leaf::Standard(standard) => matches!(
                        standard,
                        Standard::Id | Standard::Name | Standard::CreatedAt | Standard::CreatedBy
                    ),
                }
        }
        Node::Unary(_, operand) => reads_only_the_record(operand),
        Node::Binary(_, left, right) => reads_only_the_record(left) && reads_only_the_record(right),
        Node::Call(Function::Today | Function::Now, _) => false,
        Node::Call(_, arguments) => arguments.iter().all(reads_only_the_record),
        Node::Matches(subject, _) => reads_only_the_record(subject),
    }
}

/// The object's formula fields whose values can be stored.
pub(crate) fn stored_formulas(object: &ObjectDefinition) -> Vec<StoredFormula> {
    // Only the object itself: a formula that follows a lookup cannot compile.
    let objects = BTreeMap::from([(object.id, object.clone())]);
    object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.field_type == FieldType::Formula)
        .filter_map(|field| {
            let (Some(text), Some(kind)) = (&field.formula, field.formula_type) else {
                return None;
            };
            let formula = compile_field(text, kind, object, &objects, Some(field.id)).ok()?;
            reads_only_the_record(&formula.node).then(|| StoredFormula {
                field: field.clone(),
                formula,
            })
        })
        .collect()
}

/// Ids of the object's formula fields whose values are stored.
pub(crate) fn stored_formula_ids(object: &ObjectDefinition) -> std::collections::BTreeSet<Uuid> {
    stored_formulas(object)
        .into_iter()
        .map(|stored| stored.field.id)
        .collect()
}

/// Adds the stored formula values of a record to its index rows.
pub(crate) fn index_formulas(
    stored: &[StoredFormula],
    record: &Record,
    index: &mut crate::RecordIndex,
) {
    let related = BTreeMap::new();
    let env = ReadEnv {
        record,
        related: &related,
        now: Utc::now(),
    };
    for StoredFormula { field, formula } in stored {
        let Ok(value) = formula::evaluate(formula, &env) else {
            continue;
        };
        let value = finish(field, value);
        let kind = field.formula_type.unwrap_or(FieldType::Text);
        let entry = match (kind, &value) {
            (FieldType::Boolean, serde_json::Value::Bool(flag)) => {
                Some(crate::IndexValue::Text(flag.to_string()))
            }
            (kind, serde_json::Value::Number(number)) if kind.is_numeric() => {
                number.as_f64().map(crate::IndexValue::Number)
            }
            (FieldType::Date, serde_json::Value::String(text)) => {
                chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d")
                    .ok()
                    .and_then(|date| date.and_hms_opt(0, 0, 0))
                    .map(|time| crate::IndexValue::Time(time.and_utc()))
            }
            (FieldType::DateTime, serde_json::Value::String(text)) => {
                DateTime::parse_from_rfc3339(text)
                    .ok()
                    .map(|time| crate::IndexValue::Time(time.with_timezone(&Utc)))
            }
            (_, serde_json::Value::String(text)) => {
                Some(crate::IndexValue::Text(crate::values::name_key(text)))
            }
            _ => None,
        };
        if let Some(value) = entry {
            index.entries.push(crate::IndexEntry {
                field_id: field.id,
                value,
            });
        }
    }
}
