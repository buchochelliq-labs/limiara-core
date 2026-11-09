//! Validation rules (ADR 0021, #298): metadata that blocks saves.
//!
//! Rules are formulas checked against metadata when saved and evaluated in
//! the save transaction, in system context, before anything is written.

use crate::events::Change;
use crate::formula::check::{Catalog, Purpose};
use crate::formula::{self, Env, FieldRef, Formula, Type, Value};
use crate::service::{MutationContext, ObjectService};
use crate::{
    CreateValidationRule, FieldType, ObjectDefinition, ObjectError, ObjectTx, Record,
    UpdateValidationRule, ValidationIssue, ValidationRule,
};
use chrono::{DateTime, Utc};
use std::collections::btree_map::Entry;
use std::collections::BTreeMap;
use uuid::Uuid;

/// Rules per object.
pub const RULES_PER_OBJECT_MAX: usize = 100;
const MESSAGE_MAX: usize = 255;
const DESCRIPTION_MAX: usize = 1000;

/// The objects a formula on `base` may reach through lookups, by id.
pub(crate) async fn formula_objects(
    tx: &mut dyn ObjectTx,
    base: &ObjectDefinition,
) -> Result<BTreeMap<Uuid, ObjectDefinition>, ObjectError> {
    let mut objects = BTreeMap::from([(base.id, base.clone())]);
    let mut frontier = vec![base.id];
    for _ in 0..formula::limits::HOPS {
        let mut next = Vec::new();
        for id in frontier {
            let targets: Vec<Uuid> = objects[&id]
                .fields
                .iter()
                .filter(|field| field.is_active() && field.field_type.is_reference())
                .filter_map(|field| field.reference_object_id)
                .collect();
            for target in targets {
                if objects.contains_key(&target) {
                    continue;
                }
                if let Some(object) = tx
                    .object_by_id(target)
                    .await?
                    .filter(ObjectDefinition::is_active)
                {
                    objects.insert(target, object);
                    next.push(target);
                }
            }
        }
        frontier = next;
    }
    Ok(objects)
}

fn compile_rule(
    condition: &str,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
) -> Result<Formula, formula::parse::FormulaError> {
    let catalog = Catalog::new(base, objects, Purpose::Save);
    formula::compile(condition, &catalog, Some(Type::Boolean))
}

/// Whether a rule's condition compiles against an object as defined here,
/// such as in a replica that leaves out lookups to objects the member
/// cannot read.
pub(crate) fn compiles(
    condition: &str,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
) -> bool {
    compile_rule(condition, base, objects).is_ok()
}

fn issue(field: &str, code: &'static str, detail: impl Into<String>) -> ValidationIssue {
    ValidationIssue::new(field, code, detail)
}

pub(crate) fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 63
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

/// Checks a rule's text and target field, compiling its condition.
fn check_rule(
    rule: &ValidationRule,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !valid_name(&rule.name) {
        issues.push(issue(
            "name",
            "rules.name_invalid",
            "use a lowercase letter, then lowercase letters, digits or underscores, up to 63",
        ));
    }
    if rule
        .description
        .as_ref()
        .is_some_and(|text| text.chars().count() > DESCRIPTION_MAX)
    {
        issues.push(issue(
            "description",
            "rules.description_invalid",
            format!("use at most {DESCRIPTION_MAX} characters"),
        ));
    }
    let length = rule.message.chars().count();
    if length == 0 || length > MESSAGE_MAX || rule.message.chars().any(char::is_control) {
        issues.push(issue(
            "message",
            "rules.message_invalid",
            format!("write a message of 1 to {MESSAGE_MAX} characters on one line"),
        ));
    }
    if let Some(field) = &rule.error_field {
        if field != "name" && base.field(field).is_none() {
            issues.push(issue(
                "error_field",
                "rules.field_unknown",
                format!("{} has no field {field}", base.api_name),
            ));
        }
    }
    if let Err(error) = compile_rule(&rule.condition, base, objects) {
        issues.push(issue(
            "condition",
            "rules.condition_invalid",
            format!("{} (at position {})", error.message, error.position),
        ));
    }
    issues
}

/// The validation rules, formula fields and automations in the workspace
/// whose formulas still compile, each with the object it belongs to, for
/// refusing changes that would break them.
pub(crate) async fn compiling_dependents(
    tx: &mut dyn ObjectTx,
) -> Result<BTreeMap<Uuid, Uuid>, ObjectError> {
    let mut working = BTreeMap::new();
    let mut catalogs: BTreeMap<Uuid, Option<BTreeMap<Uuid, ObjectDefinition>>> = BTreeMap::new();
    let mut object_ids: Vec<Uuid> = tx
        .list_objects()
        .await?
        .into_iter()
        .map(|object| object.id)
        .collect();
    let rules = tx.all_validation_rules().await?;
    object_ids.extend(rules.iter().map(|rule| rule.object_id));
    let automations = tx.all_automations().await?;
    object_ids.extend(automations.iter().map(|automation| automation.object_id));
    for object_id in object_ids {
        if let Entry::Vacant(entry) = catalogs.entry(object_id) {
            let catalog = match tx
                .object_by_id(object_id)
                .await?
                .filter(ObjectDefinition::is_active)
            {
                Some(object) => Some(formula_objects(tx, &object).await?),
                None => None,
            };
            entry.insert(catalog);
        }
    }
    // Only active rules run, so only they keep fields. A rule needs the
    // fields its condition reads and the field it shows its message on.
    for rule in rules.iter().filter(|rule| rule.active) {
        if let Some(Some(objects)) = catalogs.get(&rule.object_id) {
            let object = &objects[&rule.object_id];
            let target_exists = rule
                .error_field
                .as_deref()
                .is_none_or(|field| field == "name" || object.field(field).is_some());
            if target_exists && compile_rule(&rule.condition, object, objects).is_ok() {
                working.insert(rule.id, rule.object_id);
            }
        }
    }
    for (object_id, objects) in &catalogs {
        let Some(objects) = objects else { continue };
        let object = &objects[object_id];
        for field in object
            .fields
            .iter()
            .filter(|field| field.is_active() && field.field_type == FieldType::Formula)
        {
            let (Some(formula), Some(kind)) = (&field.formula, field.formula_type) else {
                continue;
            };
            if crate::formula_fields::compile_field(formula, kind, object, objects, Some(field.id))
                .is_ok()
            {
                working.insert(field.id, *object_id);
            }
        }
    }
    crate::automation::working_automations(tx, &automations, &catalogs, &mut working).await?;
    Ok(working)
}

/// What a rule reads: the record being saved, its previous version and
/// the records its lookups reach, loaded in system context.
pub(crate) struct RuleEnv<'a> {
    pub(crate) record: &'a Record,
    pub(crate) prior: Option<&'a Record>,
    pub(crate) related: &'a BTreeMap<Vec<String>, Option<Record>>,
    pub(crate) now: DateTime<Utc>,
}

impl Env for RuleEnv<'_> {
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

    fn prior(&self, field: &FieldRef) -> Value {
        self.prior.map_or(Value::Null, |record| {
            formula::record_value(record, &field.leaf)
        })
    }

    fn is_new(&self) -> bool {
        self.prior.is_none()
    }

    fn now(&self) -> DateTime<Utc> {
        self.now
    }
}

/// Loads, in system context, the active records a formula's lookup chains
/// reach from `record`.
pub(crate) async fn related_records(
    tx: &mut dyn ObjectTx,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
    record: &Record,
    chains: &[Vec<String>],
    loaded: &mut BTreeMap<Vec<String>, Option<Record>>,
) -> Result<(), ObjectError> {
    for chain in chains {
        if loaded.contains_key(chain) {
            continue;
        }
        let (hop, through) = chain.split_last().ok_or(ObjectError::Invariant)?;
        let parent = if through.is_empty() {
            Some(record.clone())
        } else {
            loaded.get(through).cloned().flatten()
        };
        let mut object = base;
        for segment in through {
            let target = object
                .field(segment)
                .and_then(|field| field.reference_object_id)
                .and_then(|id| objects.get(&id));
            match target {
                Some(target) => object = target,
                None => break,
            }
        }
        let target_object = object
            .field(hop)
            .and_then(|field| field.reference_object_id);
        let target = match (parent, target_object) {
            (Some(parent), Some(target_object)) => {
                match parent.values.get(hop).and_then(serde_json::Value::as_str) {
                    Some(public_id) => tx.record_by_public_id(public_id).await?.filter(|found| {
                        found.object_id == target_object && found.deleted_at.is_none()
                    }),
                    None => None,
                }
            }
            _ => None,
        };
        loaded.insert(chain.clone(), target);
    }
    Ok(())
}

impl ObjectService {
    /// Blocks the save when an active rule's condition is true (ADR 0021).
    /// `prior` is the stored record for updates, `None` for creates.
    pub(crate) async fn enforce_rules(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        prior: Option<&Record>,
        record: &Record,
    ) -> Result<(), ObjectError> {
        let rules: Vec<ValidationRule> = tx
            .validation_rules(object.id)
            .await?
            .into_iter()
            .filter(|rule| rule.active)
            .collect();
        if rules.is_empty() {
            return Ok(());
        }
        let objects = formula_objects(tx, object).await?;
        let mut related = BTreeMap::new();
        let now = Utc::now();
        let mut issues = Vec::new();
        for rule in &rules {
            let field = rule.error_field.as_deref().unwrap_or("record");
            let broken = || {
                issue(
                    field,
                    "records.validation_rule_error",
                    format!(
                        "the validation rule {} cannot be checked; ask an administrator to correct it",
                        rule.name
                    ),
                )
            };
            let Ok(formula) = compile_rule(&rule.condition, object, &objects) else {
                issues.push(broken());
                continue;
            };
            related_records(
                tx,
                object,
                &objects,
                record,
                &formula.hop_chains(),
                &mut related,
            )
            .await?;
            let env = RuleEnv {
                record,
                prior,
                related: &related,
                now,
            };
            match formula::evaluate(&formula, &env) {
                Ok(Value::Bool(true)) => issues.push(issue(
                    field,
                    "records.validation_rule",
                    rule.message.clone(),
                )),
                Ok(_) => {}
                Err(_) => issues.push(broken()),
            }
        }
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ObjectError::Validation(issues))
        }
    }

    /// An object's validation rules. Administrators (`objects.manage`) see
    /// every rule; anyone else who can read the object sees its active rules,
    /// which constrain their saves and which a synced device checks offline
    /// (ADR 0021, "Reading rules"). To anyone else the object is unknown.
    pub async fn validation_rules(
        &self,
        api_name: &str,
    ) -> Result<Vec<ValidationRule>, ObjectError> {
        let manage = self.caller().can_manage();
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if !manage {
            let access = self.access(tx.as_mut()).await?;
            if !access.for_object(object.id).read {
                return Err(ObjectError::NotFound);
            }
        }
        let mut rules = tx.validation_rules(object.id).await?;
        tx.commit().await?;
        if !manage {
            rules.retain(|rule| rule.active);
        }
        Ok(rules)
    }

    pub async fn create_validation_rule(
        &self,
        api_name: &str,
        input: CreateValidationRule,
        context: MutationContext,
    ) -> Result<ValidationRule, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if tx.validation_rules(object.id).await?.len() >= RULES_PER_OBJECT_MAX {
            return Err(ObjectError::invalid(
                "name",
                "rules.limit_reached",
                format!("an object can have at most {RULES_PER_OBJECT_MAX} validation rules"),
            ));
        }
        let now = Utc::now();
        let actor = Some(self.caller().user_id());
        let rule = ValidationRule {
            id: Uuid::now_v7(),
            object_id: object.id,
            name: input.name.trim().to_owned(),
            description: input
                .description
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty()),
            condition: input.condition,
            message: input.message.trim().to_owned(),
            error_field: input.error_field.filter(|field| !field.is_empty()),
            active: input.active.unwrap_or(true),
            version: 1,
            created_at: now,
            created_by: actor,
            updated_at: now,
            updated_by: actor,
        };
        let objects = formula_objects(tx.as_mut(), &object).await?;
        let issues = check_rule(&rule, &object, &objects);
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        tx.insert_validation_rule(&rule).await?;
        self.record_change(tx.as_mut(), context, Change::RuleCreated(&object, &rule))
            .await?;
        tx.commit().await?;
        Ok(rule)
    }

    pub async fn update_validation_rule(
        &self,
        api_name: &str,
        rule_name: &str,
        input: UpdateValidationRule,
        context: MutationContext,
    ) -> Result<ValidationRule, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        // Serialised with field deletion, which checks what depends on a field.
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let mut rule = tx
            .validation_rules(object.id)
            .await?
            .into_iter()
            .find(|rule| rule.name == rule_name)
            .ok_or(ObjectError::NotFound)?;
        if rule.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        if let Some(description) = input.description {
            rule.description = description
                .map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty());
        }
        if let Some(condition) = input.condition {
            rule.condition = condition;
        }
        if let Some(message) = input.message {
            rule.message = message.trim().to_owned();
        }
        if let Some(error_field) = input.error_field {
            rule.error_field = error_field.filter(|field| !field.is_empty());
        }
        if let Some(active) = input.active {
            rule.active = active;
        }
        let objects = formula_objects(tx.as_mut(), &object).await?;
        let issues = check_rule(&rule, &object, &objects);
        if !issues.is_empty() {
            return Err(ObjectError::Validation(issues));
        }
        rule.version += 1;
        rule.updated_at = Utc::now();
        rule.updated_by = Some(self.caller().user_id());
        if !tx
            .update_validation_rule(&rule, input.expected_version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(tx.as_mut(), context, Change::RuleUpdated(&object, &rule))
            .await?;
        tx.commit().await?;
        Ok(rule)
    }

    pub async fn delete_validation_rule(
        &self,
        api_name: &str,
        rule_name: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<ValidationRule, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let rule = tx
            .validation_rules(object.id)
            .await?
            .into_iter()
            .find(|rule| rule.name == rule_name)
            .ok_or(ObjectError::NotFound)?;
        if rule.version != expected_version
            || !tx.delete_validation_rule(rule.id, expected_version).await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(tx.as_mut(), context, Change::RuleDeleted(&object, &rule))
            .await?;
        tx.commit().await?;
        Ok(rule)
    }
}

#[cfg(test)]
mod tests {
    use super::valid_name;

    #[test]
    fn rule_names_are_short_lowercase_identifiers() {
        assert!(valid_name("amount_positive"));
        assert!(valid_name("r2"));
        for name in [
            "",
            "Amount",
            "2fast",
            "with-dash",
            "custom:x",
            &"a".repeat(64),
        ] {
            assert!(!valid_name(name), "{name}");
        }
    }
}
