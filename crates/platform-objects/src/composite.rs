//! Composite requests (ADR 0023): several record changes in one call,
//! optionally all or none in one transaction, with later operations naming
//! records earlier ones created.

use crate::automation::Run;
use crate::service::{MutationContext, ObjectService};
use crate::{ObjectDefinition, ObjectError, Record};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// Operations per composite request.
pub const COMPOSITE_MAX: usize = 25;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CompositeRequest {
    /// Run every operation in one transaction: if one fails, none is kept.
    #[serde(default)]
    pub all_or_none: bool,
    pub operations: Vec<CompositeOperation>,
}

/// One record change. `reference` names it for later operations, which can
/// use `"@{reference.id}"` as a whole string value, or as `id`, to mean the
/// public ID of the record it created or changed.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "method", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompositeOperation {
    Create {
        reference: String,
        object: String,
        values: Map<String, Value>,
    },
    Update {
        reference: String,
        object: String,
        id: String,
        expected_version: i64,
        values: Map<String, Value>,
    },
    Delete {
        reference: String,
        object: String,
        id: String,
        expected_version: i64,
    },
    /// Restores a record from the recycle bin.
    Undelete {
        reference: String,
        object: String,
        id: String,
        expected_version: i64,
    },
    /// Transfers a record to another member.
    ChangeOwner {
        reference: String,
        object: String,
        id: String,
        expected_version: i64,
        owner_id: uuid::Uuid,
    },
}

impl CompositeOperation {
    pub fn reference(&self) -> &str {
        match self {
            Self::Create { reference, .. }
            | Self::Update { reference, .. }
            | Self::Delete { reference, .. }
            | Self::Undelete { reference, .. }
            | Self::ChangeOwner { reference, .. } => reference,
        }
    }

    /// The API name of the object the operation changes.
    pub fn object(&self) -> &str {
        match self {
            Self::Create { object, .. }
            | Self::Update { object, .. }
            | Self::Delete { object, .. }
            | Self::Undelete { object, .. }
            | Self::ChangeOwner { object, .. } => object,
        }
    }
}

/// What became of one operation.
#[derive(Debug)]
pub enum CompositeOutcome {
    /// Created (`created`) or changed, as the caller now sees it.
    Done {
        record: Box<Record>,
        created: bool,
    },
    Failed(ObjectError),
    /// It succeeded, but a later operation failed, so it was rolled back.
    RolledBack,
    /// An earlier operation failed in an all-or-none request.
    NotRun,
}

fn invalid(field: impl Into<String>, code: &'static str, detail: impl Into<String>) -> ObjectError {
    ObjectError::invalid(field, code, detail)
}

/// Checks the request's shape before anything runs.
fn check(request: &CompositeRequest) -> Result<(), ObjectError> {
    let count = request.operations.len();
    if count == 0 || count > COMPOSITE_MAX {
        return Err(invalid(
            "operations",
            "composite.operations_invalid",
            format!("send 1 to {COMPOSITE_MAX} operations"),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for (index, operation) in request.operations.iter().enumerate() {
        let reference = operation.reference();
        let valid = reference
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && reference.len() <= 40
            && reference
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_');
        if !valid || !seen.insert(reference) {
            return Err(invalid(
                format!("operations[{index}].reference"),
                "composite.reference_invalid",
                "use a unique name of letters, digits and underscores, starting with a letter",
            ));
        }
    }
    Ok(())
}

/// Replaces whole-string `@{reference.id}` values with the public IDs of
/// earlier operations' records. An unknown or failed reference is an error.
fn resolve(value: &Value, done: &BTreeMap<String, Option<String>>) -> Result<Value, String> {
    let Some(text) = value.as_str() else {
        return Ok(value.clone());
    };
    let Some(reference) = text
        .strip_prefix("@{")
        .and_then(|rest| rest.strip_suffix(".id}"))
    else {
        return Ok(value.clone());
    };
    match done.get(reference) {
        Some(Some(id)) => Ok(Value::String(id.clone())),
        Some(None) => Err(format!("{reference} did not succeed")),
        None => Err(format!("{reference} is not an earlier operation")),
    }
}

fn resolve_values(
    values: &Map<String, Value>,
    done: &BTreeMap<String, Option<String>>,
) -> Result<Map<String, Value>, ObjectError> {
    values
        .iter()
        .map(|(key, value)| {
            resolve(value, done)
                .map(|value| (key.clone(), value))
                .map_err(|detail| invalid(key.clone(), "composite.reference_unknown", detail))
        })
        .collect()
}

fn resolve_id(id: &str, done: &BTreeMap<String, Option<String>>) -> Result<String, ObjectError> {
    match resolve(&Value::String(id.to_owned()), done) {
        Ok(Value::String(id)) => Ok(id),
        Ok(_) => Err(ObjectError::Invariant),
        Err(detail) => Err(invalid("id", "composite.reference_unknown", detail)),
    }
}

impl ObjectService {
    /// Runs a composite request (ADR 0023). Each operation gets the same
    /// authorisation, validation, automations and evidence as on its own.
    /// With `all_or_none`, they share one transaction and the first failure
    /// rolls back every change; otherwise each runs on its own and later
    /// operations still run after a failure.
    pub async fn composite(
        &self,
        request: &CompositeRequest,
        context: MutationContext,
    ) -> Result<Vec<(String, CompositeOutcome)>, ObjectError> {
        check(request)?;
        if request.all_or_none {
            self.composite_together(request, context).await
        } else {
            self.composite_apart(request, context).await
        }
    }

    async fn composite_apart(
        &self,
        request: &CompositeRequest,
        context: MutationContext,
    ) -> Result<Vec<(String, CompositeOutcome)>, ObjectError> {
        let mut done: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut outcomes = Vec::with_capacity(request.operations.len());
        for operation in &request.operations {
            let result = match operation {
                CompositeOperation::Create { object, values, .. } => {
                    match resolve_values(values, &done) {
                        Ok(values) => self
                            .create_record(object, &values, context)
                            .await
                            .map(|record| (record, true)),
                        Err(error) => Err(error),
                    }
                }
                CompositeOperation::Update {
                    object,
                    id,
                    expected_version,
                    values,
                    ..
                } => match (resolve_id(id, &done), resolve_values(values, &done)) {
                    (Ok(id), Ok(values)) => self
                        .update_record(object, &id, *expected_version, &values, context)
                        .await
                        .map(|record| (record, false)),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                },
                CompositeOperation::Delete {
                    object,
                    id,
                    expected_version,
                    ..
                } => match resolve_id(id, &done) {
                    Ok(id) => self
                        .delete_record(object, &id, *expected_version, context)
                        .await
                        .map(|record| (record, false)),
                    Err(error) => Err(error),
                },
                CompositeOperation::Undelete {
                    object,
                    id,
                    expected_version,
                    ..
                } => match resolve_id(id, &done) {
                    Ok(id) => self
                        .undelete_record(object, &id, *expected_version, context)
                        .await
                        .map(|record| (record, false)),
                    Err(error) => Err(error),
                },
                CompositeOperation::ChangeOwner {
                    object,
                    id,
                    expected_version,
                    owner_id,
                    ..
                } => match resolve_id(id, &done) {
                    Ok(id) => self
                        .change_owner(object, &id, *owner_id, *expected_version, context)
                        .await
                        .map(|record| (record, false)),
                    Err(error) => Err(error),
                },
            };
            let reference = operation.reference().to_owned();
            match result {
                Ok((record, created)) => {
                    done.insert(reference.clone(), Some(record.public_id.clone()));
                    outcomes.push((
                        reference,
                        CompositeOutcome::Done {
                            record: Box::new(record),
                            created,
                        },
                    ));
                }
                Err(error) => {
                    done.insert(reference.clone(), None);
                    outcomes.push((reference, CompositeOutcome::Failed(error)));
                }
            }
        }
        Ok(outcomes)
    }

    async fn composite_together(
        &self,
        request: &CompositeRequest,
        context: MutationContext,
    ) -> Result<Vec<(String, CompositeOutcome)>, ObjectError> {
        let mut run = Run::default();
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let mut done: BTreeMap<String, Option<String>> = BTreeMap::new();
        let mut changed: Vec<(String, ObjectDefinition, Record, bool)> = Vec::new();
        let mut failure = None;
        for (index, operation) in request.operations.iter().enumerate() {
            let result = match operation {
                CompositeOperation::Create { object, values, .. } => {
                    match resolve_values(values, &done) {
                        Ok(values) => self
                            .create_record_tx(
                                tx.as_mut(),
                                &access,
                                object,
                                &values,
                                context,
                                &mut run,
                            )
                            .await
                            .map(|found| (found, true)),
                        Err(error) => Err(error),
                    }
                }
                CompositeOperation::Update {
                    object,
                    id,
                    expected_version,
                    values,
                    ..
                } => match (resolve_id(id, &done), resolve_values(values, &done)) {
                    (Ok(id), Ok(values)) => self
                        .update_record_tx(
                            tx.as_mut(),
                            &access,
                            object,
                            &id,
                            *expected_version,
                            &values,
                            context,
                            &mut run,
                        )
                        .await
                        .map(|found| (found, false)),
                    (Err(error), _) | (_, Err(error)) => Err(error),
                },
                CompositeOperation::Delete {
                    object,
                    id,
                    expected_version,
                    ..
                } => match resolve_id(id, &done) {
                    Ok(id) => self
                        .delete_record_tx(
                            tx.as_mut(),
                            &access,
                            object,
                            &id,
                            *expected_version,
                            context,
                            &mut run,
                        )
                        .await
                        .map(|found| (found, false)),
                    Err(error) => Err(error),
                },
                CompositeOperation::Undelete {
                    object,
                    id,
                    expected_version,
                    ..
                } => match resolve_id(id, &done) {
                    Ok(id) => self
                        .undelete_record_tx(
                            tx.as_mut(),
                            &access,
                            object,
                            &id,
                            *expected_version,
                            context,
                            &mut run,
                        )
                        .await
                        .map(|found| (found, false)),
                    Err(error) => Err(error),
                },
                CompositeOperation::ChangeOwner {
                    object,
                    id,
                    expected_version,
                    owner_id,
                    ..
                } => match resolve_id(id, &done) {
                    Ok(id) => self
                        .change_owner_tx(
                            tx.as_mut(),
                            object,
                            &id,
                            *owner_id,
                            *expected_version,
                            context,
                            &mut run,
                        )
                        .await
                        .map(|found| (found, false)),
                    Err(error) => Err(error),
                },
            };
            match result {
                Ok(((object, record), created)) => {
                    done.insert(
                        operation.reference().to_owned(),
                        Some(record.public_id.clone()),
                    );
                    changed.push((operation.reference().to_owned(), object, record, created));
                }
                Err(error) => {
                    failure = Some((index, error));
                    break;
                }
            }
        }
        if let Some((failed_at, error)) = failure {
            // Nothing is kept; the transaction rolls back when dropped.
            drop(tx);
            // Runs that failed are kept, as for a single change.
            let mut error = self.keep_failed_runs(run, Err::<(), _>(error)).await.err();
            return Ok(request
                .operations
                .iter()
                .enumerate()
                .map(|(index, operation)| {
                    let outcome = if index < failed_at {
                        CompositeOutcome::RolledBack
                    } else if index == failed_at {
                        CompositeOutcome::Failed(error.take().unwrap_or(ObjectError::Invariant))
                    } else {
                        CompositeOutcome::NotRun
                    };
                    (operation.reference().to_owned(), outcome)
                })
                .collect());
        }
        tx.commit().await?;
        let mut outcomes = Vec::with_capacity(changed.len());
        for (reference, object, record, created) in changed {
            let record = if created {
                self.with_formula(&object, record).await?
            } else {
                self.with_formula(&object, Self::project(&object, record))
                    .await?
            };
            outcomes.push((
                reference,
                CompositeOutcome::Done {
                    record: Box::new(record),
                    created,
                },
            ));
        }
        Ok(outcomes)
    }
}

#[cfg(test)]
mod tests {
    use super::{check, resolve, CompositeRequest};
    use serde_json::json;
    use std::collections::BTreeMap;

    #[test]
    fn references_resolve_whole_values_only() {
        let done = BTreeMap::from([
            ("account".to_owned(), Some("0a1000000000000001".to_owned())),
            ("broken".to_owned(), None),
        ]);
        assert_eq!(
            resolve(&json!("@{account.id}"), &done),
            Ok(json!("0a1000000000000001"))
        );
        assert_eq!(
            resolve(&json!("see @{account.id}"), &done),
            Ok(json!("see @{account.id}"))
        );
        assert_eq!(resolve(&json!(5), &done), Ok(json!(5)));
        assert!(resolve(&json!("@{broken.id}"), &done).is_err());
        assert!(resolve(&json!("@{later.id}"), &done).is_err());
    }

    #[test]
    fn requests_are_bounded_and_references_unique() {
        let request = |operations: serde_json::Value| -> CompositeRequest {
            serde_json::from_value(json!({"operations": operations})).expect("request")
        };
        let create = |reference: &str| json!({"method": "create", "reference": reference, "object": "custom:a", "values": {"name": "x"}});
        assert!(check(&request(json!([create("a"), create("b")]))).is_ok());
        assert!(check(&request(json!([]))).is_err());
        assert!(check(&request(json!([create("a"), create("a")]))).is_err());
        assert!(check(&request(json!([create("1a")]))).is_err());
        let many: Vec<_> = (0..26).map(|index| create(&format!("r{index}"))).collect();
        assert!(check(&request(json!(many))).is_err());
    }
}
