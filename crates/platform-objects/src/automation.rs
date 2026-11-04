//! Record-triggered automations (ADR 0022, #298): metadata that sets fields
//! before a record is written, or creates and updates records after it.
//!
//! Automations are formulas evaluated in the save transaction, in system
//! context. A before-save automation changes the record being saved; an
//! after-save automation saves other records through the same save path a
//! member's change takes, so their automations, validation rules, indexes,
//! evidence and history all apply. If anything fails, the whole change
//! rolls back.

use crate::events::Change;
use crate::formula::check::{field_type, Catalog, Purpose};
use crate::formula::{self, Formula, Type, Value};
use crate::rules::{formula_objects, related_records, RuleEnv};
use crate::service::{MutationContext, ObjectService};
use crate::values::apply_values;
use crate::{
    Assignment, Automation, AutomationAction, AutomationJob, AutomationRun, AutomationTiming,
    AutomationTrigger, CreateAutomation, FieldType, JobStatus, ObjectCaller, ObjectDefinition,
    ObjectError, ObjectStore, ObjectTx, Record, RunOutcome, SaveEvent, Schedule, UpdateAutomation,
    ValidationIssue,
};
use chrono::Utc;
use serde_json::Map;
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

/// Automations per object.
pub const AUTOMATIONS_PER_OBJECT_MAX: usize = 50;
/// Actions per after-save automation.
pub const ACTIONS_MAX: usize = 10;
/// Fields one action sets.
pub const ASSIGNMENTS_MAX: usize = 50;
/// How deep automations may cause each other to run.
pub const DEPTH_MAX: usize = 5;
/// Records automations may save in one change.
pub const SAVES_MAX: usize = 100;
/// Scheduled automations run at most a year before or after their time.
pub const SCHEDULE_OFFSET_MAX_MINUTES: i64 = 525_600;
const DESCRIPTION_MAX: usize = 1000;

/// What automations have done so far in one change.
#[derive(Debug, Default)]
pub(crate) struct Run {
    depth: usize,
    saves: usize,
    /// Jobs that led to this change, when it runs a queued job.
    job_depth: usize,
    /// The automation and record of the job this change runs: its own
    /// changes do not queue it again.
    job: Option<(Uuid, Uuid)>,
    /// Runs that failed, written once the change has rolled back.
    failed: Vec<AutomationRun>,
    /// Runs that acted, written in the change's transaction. If the change
    /// later rolls back they are lost with it, so they are kept here to be
    /// written afterwards as rolled back.
    acted: Vec<AutomationRun>,
}

impl Run {
    /// The runs to write after the change rolled back: those that failed,
    /// and those that acted but whose work was undone.
    fn rolled_back(self) -> Vec<AutomationRun> {
        let mut runs = self.failed;
        runs.extend(self.acted.into_iter().map(|mut run| {
            run.id = Uuid::now_v7();
            run.outcome = RunOutcome::Failed;
            run.saves = 0;
            run.detail = Some(
                "its changes were rolled back because a later step of the change failed".into(),
            );
            run
        }));
        runs
    }
}

fn issue(
    field: impl Into<String>,
    code: &'static str,
    detail: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue::new(field, code, detail)
}

fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|first| first.is_ascii_lowercase())
        && name.len() <= 63
        && chars.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
}

/// A save blocked by an automation.
fn failed(automation: &Automation, detail: impl std::fmt::Display) -> ObjectError {
    ObjectError::invalid(
        "record",
        "records.automation_failed",
        format!("automation {}: {detail}", automation.name),
    )
}

/// Validation issues as one line, for an automation's error.
fn describe(issues: &[ValidationIssue]) -> String {
    issues
        .iter()
        .map(|issue| {
            if issue.field == "record" {
                issue.detail.clone()
            } else {
                format!("{}: {}", issue.field, issue.detail)
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// The object each action writes: the saved record's own object, the
/// target of a lookup on it, or a named object. `None` when it is missing.
pub(crate) async fn action_targets(
    tx: &mut dyn ObjectTx,
    base: &ObjectDefinition,
    actions: &[AutomationAction],
) -> Result<Vec<Option<ObjectDefinition>>, ObjectError> {
    let mut targets = Vec::with_capacity(actions.len());
    for action in actions {
        let target = match action {
            // Events are about the saved record; their fields are free names.
            AutomationAction::UpdateRecord { .. } | AutomationAction::PublishEvent { .. } => {
                Some(base.clone())
            }
            AutomationAction::UpdateRelated { lookup, .. } => {
                match base
                    .field(lookup)
                    .filter(|field| field.field_type.is_reference())
                    .and_then(|field| field.reference_object_id)
                {
                    Some(id) => tx
                        .object_by_id(id)
                        .await?
                        .filter(ObjectDefinition::is_active),
                    None => None,
                }
            }
            AutomationAction::CreateRecord { object, .. } => tx
                .object_by_api_name(object)
                .await?
                .filter(ObjectDefinition::is_active),
        };
        targets.push(target);
    }
    Ok(targets)
}

/// One field an action sets.
pub(crate) struct CompiledAssignment {
    field: String,
    scale: Option<i32>,
    formula: Formula,
}

/// An action ready to run against the object it writes; for an event, the
/// automation's own object.
pub(crate) struct CompiledAction {
    target: ObjectDefinition,
    when: Option<Formula>,
    values: Vec<CompiledAssignment>,
}

/// An automation with every formula compiled.
pub(crate) struct Compiled {
    condition: Option<Formula>,
    actions: Vec<CompiledAction>,
}

impl Compiled {
    /// The lookup chains the automation's formulas read, parents first.
    fn hop_chains(&self) -> Vec<Vec<String>> {
        let mut chains: Vec<Vec<String>> = self
            .condition
            .iter()
            .chain(self.actions.iter().flat_map(|action| {
                action
                    .when
                    .iter()
                    .chain(action.values.iter().map(|value| &value.formula))
            }))
            .flat_map(Formula::hop_chains)
            .collect();
        chains.sort_by_key(Vec::len);
        chains.dedup();
        chains
    }
}

fn compile_save(
    text: &str,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
    expected: Type,
) -> Result<Formula, formula::parse::FormulaError> {
    compile_value(text, base, objects, Some(expected))
}

/// Compiles a formula of any type, or of `expected` when given.
fn compile_value(
    text: &str,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
    expected: Option<Type>,
) -> Result<Formula, formula::parse::FormulaError> {
    let catalog = Catalog::new(base, objects, Purpose::Save);
    formula::compile(text, &catalog, expected)
}

/// The largest published event payload, in bytes of JSON.
pub const EVENT_BYTES_MAX: usize = 64 * 1024;

/// The type a field takes and its decimal places, or why it cannot be set.
fn assignable(target: &ObjectDefinition, field: &str) -> Result<(Type, Option<i32>), String> {
    if field == "name" {
        return Ok((Type::Text, None));
    }
    let definition = target
        .field(field)
        .ok_or_else(|| format!("{} has no field {field}", target.api_name))?;
    if definition.field_type == FieldType::Formula {
        return Err(format!("{field} is a formula field; its value is computed"));
    }
    if definition.field_type == FieldType::Rollup {
        return Err(format!(
            "{field} is a roll-up summary field; its value is computed"
        ));
    }
    Ok((field_type(definition.field_type), definition.scale))
}

/// Compiles an automation against current metadata. Every problem is
/// reported, addressed to the part of the definition it concerns.
pub(crate) fn compile_automation(
    automation: &Automation,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
    targets: &[Option<ObjectDefinition>],
) -> Result<Compiled, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let condition = match automation.condition.as_deref() {
        Some(text) => match compile_save(text, base, objects, Type::Boolean) {
            Ok(formula) => Some(formula),
            Err(error) => {
                issues.push(issue(
                    "condition",
                    "automations.condition_invalid",
                    format!("{} (at position {})", error.message, error.position),
                ));
                None
            }
        },
        None => None,
    };
    let lifecycle = matches!(
        automation.trigger,
        AutomationTrigger::Delete | AutomationTrigger::Undelete
    );
    match automation.timing {
        AutomationTiming::BeforeSave if lifecycle => issues.push(issue(
            "trigger",
            "automations.trigger_invalid",
            "automations on delete or restore run after saving",
        )),
        AutomationTiming::Scheduled if lifecycle => issues.push(issue(
            "trigger",
            "automations.trigger_invalid",
            "scheduled automations run for created or updated records",
        )),
        _ => {}
    }
    match (&automation.schedule, automation.timing) {
        (None, AutomationTiming::Scheduled) => issues.push(issue(
            "schedule",
            "automations.schedule_invalid",
            "say when a scheduled automation runs",
        )),
        (Some(_), timing) if timing != AutomationTiming::Scheduled => issues.push(issue(
            "schedule",
            "automations.schedule_invalid",
            "only scheduled automations have a schedule",
        )),
        (Some(schedule), _) => {
            if !(-SCHEDULE_OFFSET_MAX_MINUTES..=SCHEDULE_OFFSET_MAX_MINUTES)
                .contains(&schedule.offset_minutes)
            {
                issues.push(issue(
                    "schedule.offset_minutes",
                    "automations.schedule_invalid",
                    "schedule at most a year before or after",
                ));
            }
            if let Some(from) = &schedule.from {
                let date = base.field(from).is_some_and(|field| {
                    matches!(field.field_type, FieldType::Date | FieldType::DateTime)
                });
                if !date {
                    issues.push(issue(
                        "schedule.from",
                        "automations.schedule_invalid",
                        format!("{} has no date or date-time field {from}", base.api_name),
                    ));
                }
            }
        }
        (None, _) => {}
    }
    let count = automation.actions.len();
    match automation.timing {
        AutomationTiming::BeforeSave => {
            if count != 1 || !matches!(automation.actions[0], AutomationAction::UpdateRecord { .. })
            {
                issues.push(issue(
                    "actions",
                    "automations.actions_invalid",
                    "a before-save automation has one action, which updates the record",
                ));
            }
        }
        AutomationTiming::Async | AutomationTiming::Scheduled => {
            if count == 0 || count > ACTIONS_MAX {
                issues.push(issue(
                    "actions",
                    "automations.actions_invalid",
                    format!("a background automation has 1 to {ACTIONS_MAX} actions"),
                ));
            }
        }
        AutomationTiming::AfterSave => {
            if count == 0 || count > ACTIONS_MAX {
                issues.push(issue(
                    "actions",
                    "automations.actions_invalid",
                    format!("an after-save automation has 1 to {ACTIONS_MAX} actions"),
                ));
            }
            if automation
                .actions
                .iter()
                .any(|action| matches!(action, AutomationAction::UpdateRecord { .. }))
            {
                issues.push(issue(
                    "actions",
                    "automations.actions_invalid",
                    "after saving, update related records or create records; \
                     set the record's own fields before it is saved",
                ));
            }
        }
    }
    let mut actions = Vec::with_capacity(count);
    for (index, (action, target)) in automation.actions.iter().zip(targets).enumerate() {
        let path = format!("actions[{index}]");
        let Some(target) = target else {
            issues.push(match action {
                AutomationAction::UpdateRelated { lookup, .. } => issue(
                    format!("{path}.lookup"),
                    "automations.lookup_invalid",
                    format!("{} has no lookup {lookup}", base.api_name),
                ),
                AutomationAction::CreateRecord { object, .. } => issue(
                    format!("{path}.object"),
                    "automations.object_unknown",
                    format!("there is no object {object}"),
                ),
                AutomationAction::UpdateRecord { .. } | AutomationAction::PublishEvent { .. } => {
                    issue(path, "automations.object_unknown", "the object is missing")
                }
            });
            continue;
        };
        let when = match action.when() {
            Some(text) => match compile_save(text, base, objects, Type::Boolean) {
                Ok(formula) => Some(formula),
                Err(error) => {
                    issues.push(issue(
                        format!("{path}.when"),
                        "automations.condition_invalid",
                        format!("{} (at position {})", error.message, error.position),
                    ));
                    None
                }
            },
            None => None,
        };
        let event = match action {
            AutomationAction::PublishEvent { event, .. } => {
                if !valid_name(event) {
                    issues.push(issue(
                        format!("{path}.event"),
                        "automations.event_invalid",
                        "name the event with a lowercase letter, then lowercase letters, digits or underscores, up to 63",
                    ));
                }
                true
            }
            _ => false,
        };
        let values = action.values();
        if values.is_empty() || values.len() > ASSIGNMENTS_MAX {
            issues.push(issue(
                format!("{path}.values"),
                "automations.values_invalid",
                format!("set 1 to {ASSIGNMENTS_MAX} fields"),
            ));
        }
        let mut seen = BTreeSet::new();
        let mut compiled = Vec::with_capacity(values.len());
        for (position, Assignment { field, formula }) in values.iter().enumerate() {
            let at = format!("{path}.values[{position}]");
            if !seen.insert(field.as_str()) {
                issues.push(issue(
                    format!("{at}.field"),
                    "automations.field_duplicate",
                    format!("{field} is set more than once"),
                ));
                continue;
            }
            let (ty, scale) = if event {
                if !valid_name(field) {
                    issues.push(issue(
                        format!("{at}.field"),
                        "automations.field_invalid",
                        "name event fields with a lowercase letter, then lowercase letters, digits or underscores",
                    ));
                    continue;
                }
                (None, None)
            } else {
                match assignable(target, field) {
                    Ok((ty, scale)) => (Some(ty), scale),
                    Err(detail) => {
                        issues.push(issue(
                            format!("{at}.field"),
                            "automations.field_invalid",
                            detail,
                        ));
                        continue;
                    }
                }
            };
            match compile_value(formula, base, objects, ty) {
                Ok(formula) => compiled.push(CompiledAssignment {
                    field: field.clone(),
                    scale,
                    formula,
                }),
                Err(error) => issues.push(issue(
                    format!("{at}.formula"),
                    "automations.formula_invalid",
                    format!("{} (at position {})", error.message, error.position),
                )),
            }
        }
        actions.push(CompiledAction {
            target: target.clone(),
            when,
            values: compiled,
        });
    }
    if issues.is_empty() {
        Ok(Compiled { condition, actions })
    } else {
        Err(issues)
    }
}

/// Everything wrong with an automation an administrator saves.
fn check_automation(
    automation: &Automation,
    base: &ObjectDefinition,
    objects: &BTreeMap<Uuid, ObjectDefinition>,
    targets: &[Option<ObjectDefinition>],
) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !valid_name(&automation.name) {
        issues.push(issue(
            "name",
            "automations.name_invalid",
            "use a lowercase letter, then lowercase letters, digits or underscores, up to 63",
        ));
    }
    if automation
        .description
        .as_ref()
        .is_some_and(|text| text.chars().count() > DESCRIPTION_MAX)
    {
        issues.push(issue(
            "description",
            "automations.description_invalid",
            format!("use at most {DESCRIPTION_MAX} characters"),
        ));
    }
    if let Err(found) = compile_automation(automation, base, objects, targets) {
        issues.extend(found);
    }
    // A created record needs a name and every required field without a
    // default, or each run would fail.
    for (index, (action, target)) in automation.actions.iter().zip(targets).enumerate() {
        let (AutomationAction::CreateRecord { values, .. }, Some(target)) = (action, target) else {
            continue;
        };
        let set: BTreeSet<&str> = values.iter().map(|value| value.field.as_str()).collect();
        let mut missing: Vec<&str> = target
            .fields
            .iter()
            .filter(|field| {
                field.is_active()
                    && field.required
                    && field.default_value.is_none()
                    && !(field.field_type == FieldType::Picklist
                        && field.options.iter().any(|option| option.default))
                    && !set.contains(field.api_name.as_str())
            })
            .map(|field| field.api_name.as_str())
            .collect();
        if !set.contains("name") {
            missing.insert(0, "name");
        }
        if !missing.is_empty() {
            issues.push(issue(
                format!("actions[{index}].values"),
                "automations.value_required",
                format!("set {} on the new record", missing.join(", ")),
            ));
        }
    }
    issues
}

/// A formula result as a field value, rounded to the field's decimal places.
fn assigned(value: Value, scale: Option<i32>) -> serde_json::Value {
    match (value, scale) {
        (Value::Number(number), Some(scale)) => {
            let factor = 10f64.powi(scale);
            Value::Number((number * factor).round() / factor).to_json()
        }
        (value, _) => value.to_json(),
    }
}

/// Inserts the active automations in the workspace that still compile into
/// `working`, with their object, for refusing metadata changes that break
/// them.
pub(crate) async fn working_automations(
    tx: &mut dyn ObjectTx,
    automations: &[Automation],
    catalogs: &BTreeMap<Uuid, Option<BTreeMap<Uuid, ObjectDefinition>>>,
    working: &mut BTreeMap<Uuid, Uuid>,
) -> Result<(), ObjectError> {
    for automation in automations.iter().filter(|automation| automation.active) {
        let Some(Some(objects)) = catalogs.get(&automation.object_id) else {
            continue;
        };
        let object = &objects[&automation.object_id];
        let targets = action_targets(tx, object, &automation.actions).await?;
        if compile_automation(automation, object, objects, &targets).is_ok() {
            working.insert(automation.id, automation.object_id);
        }
    }
    Ok(())
}

type Saving<'a> = Pin<Box<dyn Future<Output = Result<(), ObjectError>> + Send + 'a>>;

/// Runs are kept this long (ADR 0022 §5).
pub const RUN_RETENTION: chrono::TimeDelta = chrono::TimeDelta::days(30);

/// What one automation run did, before it is recorded.
struct Acted {
    saves: usize,
}

impl ObjectService {
    /// The active automations of `object` that `event` fires at `timing`.
    async fn firing(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        timing: AutomationTiming,
        event: SaveEvent,
    ) -> Result<Vec<Automation>, ObjectError> {
        Ok(tx
            .automations(object.id)
            .await?
            .into_iter()
            .filter(|automation| {
                automation.active && automation.timing == timing && automation.trigger.fires(event)
            })
            .collect())
    }

    /// Compiles an automation for a run; one that no longer compiles blocks
    /// the save, as a broken validation rule does.
    async fn compile_for_run(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        objects: &BTreeMap<Uuid, ObjectDefinition>,
        automation: &Automation,
    ) -> Result<Compiled, ObjectError> {
        let targets = action_targets(tx, object, &automation.actions).await?;
        compile_automation(automation, object, objects, &targets)
            .map_err(|_| failed(automation, "cannot run; ask an administrator to correct it"))
    }

    /// Evaluates an automation's condition and the values of each action on
    /// the saved record. `None` when the condition is not met.
    #[allow(clippy::too_many_arguments)]
    async fn evaluate(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        objects: &BTreeMap<Uuid, ObjectDefinition>,
        automation: &Automation,
        compiled: &Compiled,
        prior: Option<&Record>,
        record: &Record,
        check_condition: bool,
    ) -> Result<Option<Vec<Option<Map<String, serde_json::Value>>>>, ObjectError> {
        let mut related = BTreeMap::new();
        related_records(
            tx,
            object,
            objects,
            record,
            &compiled.hop_chains(),
            &mut related,
        )
        .await?;
        let env = RuleEnv {
            record,
            prior,
            related: &related,
            now: Utc::now(),
        };
        if let Some(condition) = compiled.condition.as_ref().filter(|_| check_condition) {
            match formula::evaluate(condition, &env) {
                Ok(Value::Bool(true)) => {}
                Ok(_) => return Ok(None),
                Err(_) => return Err(failed(automation, "the condition cannot be evaluated")),
            }
        }
        if compiled.actions.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let mut bodies = Vec::with_capacity(compiled.actions.len());
        for (index, action) in compiled.actions.iter().enumerate() {
            // An action with a condition of its own runs only when it holds.
            if let Some(when) = &action.when {
                match formula::evaluate(when, &env) {
                    Ok(Value::Bool(true)) => {}
                    Ok(_) => {
                        bodies.push(None);
                        continue;
                    }
                    Err(_) => {
                        return Err(failed(
                            automation,
                            format!("the condition of action {} cannot be evaluated", index + 1),
                        ))
                    }
                }
            }
            let mut body = Map::new();
            for value in &action.values {
                let result = formula::evaluate(&value.formula, &env).map_err(|_| {
                    failed(
                        automation,
                        format!("the value for {} cannot be evaluated", value.field),
                    )
                })?;
                body.insert(value.field.clone(), assigned(result, value.scale));
            }
            bodies.push(Some(body));
        }
        Ok(Some(bodies))
    }

    /// Records how a run ended: in the change's transaction when it acted,
    /// or for writing after the change rolls back when it failed.
    #[allow(clippy::too_many_arguments)]
    async fn record_run(
        &self,
        tx: &mut dyn ObjectTx,
        automation: &Automation,
        event: SaveEvent,
        record: &Record,
        context: MutationContext,
        run: &mut Run,
        result: Result<Option<Acted>, ObjectError>,
    ) -> Result<(), ObjectError> {
        let entry = |outcome: RunOutcome, detail: Option<String>, saves: usize| AutomationRun {
            id: Uuid::now_v7(),
            automation_id: automation.id,
            object_id: automation.object_id,
            record: record.public_id.clone(),
            event,
            outcome,
            detail,
            saves: i64::try_from(saves).unwrap_or(i64::MAX),
            ran_at: Utc::now(),
            ran_by: Some(self.caller().user_id()),
            correlation_id: Some(context.correlation_id),
        };
        match result {
            Ok(None) => Ok(()),
            Ok(Some(acted)) => {
                let entry = entry(RunOutcome::Succeeded, None, acted.saves);
                store_runs(tx, std::slice::from_ref(&entry)).await?;
                run.acted.push(entry);
                Ok(())
            }
            Err(error) => {
                if let ObjectError::Validation(issues) = &error {
                    let detail: String = describe(issues).chars().take(2000).collect();
                    run.failed.push(entry(RunOutcome::Failed, Some(detail), 0));
                }
                Err(error)
            }
        }
    }

    /// Runs before-save automations on a record about to be written,
    /// changing it in place and adding the fields they changed to
    /// `changed`. Returns the fields automations set, whose lookups are
    /// resolved in system context.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn before_save(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        prior: Option<&Record>,
        record: &mut Record,
        changed: &mut Vec<String>,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<BTreeSet<String>, ObjectError> {
        let mut trusted = BTreeSet::new();
        let event = if prior.is_some() {
            SaveEvent::Update
        } else {
            SaveEvent::Create
        };
        let automations = self
            .firing(tx, object, AutomationTiming::BeforeSave, event)
            .await?;
        if automations.is_empty() {
            return Ok(trusted);
        }
        let objects = formula_objects(tx, object).await?;
        for automation in &automations {
            let result = self
                .set_fields(
                    tx,
                    object,
                    &objects,
                    automation,
                    prior,
                    record,
                    changed,
                    &mut trusted,
                )
                .await;
            self.record_run(tx, automation, event, record, context, run, result)
                .await?;
        }
        Ok(trusted)
    }

    /// One before-save automation: `None` when its condition is not met.
    #[allow(clippy::too_many_arguments)]
    async fn set_fields(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        objects: &BTreeMap<Uuid, ObjectDefinition>,
        automation: &Automation,
        prior: Option<&Record>,
        record: &mut Record,
        changed: &mut Vec<String>,
        trusted: &mut BTreeSet<String>,
    ) -> Result<Option<Acted>, ObjectError> {
        let compiled = self
            .compile_for_run(tx, object, objects, automation)
            .await?;
        let Some(bodies) = self
            .evaluate(
                tx, object, objects, automation, &compiled, prior, record, true,
            )
            .await?
        else {
            return Ok(None);
        };
        for body in bodies.into_iter().flatten() {
            let change = apply_values(object, &record.values, &body, false).map_err(|issues| {
                failed(
                    automation,
                    format!("could not set the record's fields: {}", describe(&issues)),
                )
            })?;
            if let Some(name) = change.name {
                if name != record.name {
                    changed.push("name".into());
                }
                record.name = name;
            }
            record.values = change.values;
            changed.extend(change.changed);
            trusted.extend(body.keys().cloned());
        }
        Ok(Some(Acted { saves: 0 }))
    }

    /// Runs after-save automations once a record is written, deleted or
    /// restored. Their saves take the full save path in system context, one
    /// level deeper, and name the automation in their evidence.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn after_save<'a>(
        &'a self,
        tx: &'a mut dyn ObjectTx,
        object: &'a ObjectDefinition,
        event: SaveEvent,
        prior: Option<&'a Record>,
        record: &'a Record,
        context: MutationContext,
        run: &'a mut Run,
    ) -> Saving<'a> {
        Box::pin(async move {
            // After-save automations and background and scheduled work run
            // on the server in synced mode, so they run once (ADR 0024 §3).
            if self.is_replica() {
                return Ok(());
            }
            let automations = self
                .firing(tx, object, AutomationTiming::AfterSave, event)
                .await?;
            let background = self.background(tx, object, event).await?;
            if automations.is_empty() && background.is_empty() {
                return Ok(());
            }
            let objects = formula_objects(tx, object).await?;
            for automation in &automations {
                let result = self
                    .act(
                        tx, object, &objects, automation, prior, record, true, context, run,
                    )
                    .await;
                self.record_run(tx, automation, event, record, context, run, result)
                    .await?;
            }
            self.queue_background(
                tx,
                object,
                &objects,
                &background,
                event,
                prior,
                record,
                context,
                run,
            )
            .await
        })
    }

    /// The active background and scheduled automations `event` fires.
    async fn background(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        event: SaveEvent,
    ) -> Result<Vec<Automation>, ObjectError> {
        let mut background = self
            .firing(tx, object, AutomationTiming::Async, event)
            .await?;
        background.extend(
            self.firing(tx, object, AutomationTiming::Scheduled, event)
                .await?,
        );
        Ok(background)
    }

    /// Queues the jobs of background and scheduled automations whose
    /// condition holds, in the change's transaction (ADR 0022 §6). A
    /// scheduled automation whose condition no longer holds loses its
    /// pending job.
    #[allow(clippy::too_many_arguments)]
    async fn queue_background(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        objects: &BTreeMap<Uuid, ObjectDefinition>,
        background: &[Automation],
        event: SaveEvent,
        prior: Option<&Record>,
        record: &Record,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<(), ObjectError> {
        let now = Utc::now();
        for automation in background {
            if run.job == Some((automation.id, record.id)) {
                continue;
            }
            let mut compiled = self
                .compile_for_run(tx, object, objects, automation)
                .await?;
            // Only the condition is evaluated now; values are evaluated when
            // the job runs, on the record as it is then.
            compiled.actions.clear();
            let holds = self
                .evaluate(
                    tx, object, objects, automation, &compiled, prior, record, true,
                )
                .await?
                .is_some();
            let scheduled = automation.timing == AutomationTiming::Scheduled;
            let due_at = match (&automation.schedule, holds) {
                (_, false) => None,
                (None, true) => Some(now),
                (Some(schedule), true) => scheduled_time(schedule, object, record, now),
            };
            let Some(due_at) = due_at else {
                if scheduled {
                    tx.cancel_scheduled_job(automation.id, record.id, now)
                        .await?;
                }
                continue;
            };
            if run.job_depth >= DEPTH_MAX {
                // A chain of jobs queuing jobs ends here, visibly.
                store_runs(
                    tx,
                    &[AutomationRun {
                        id: Uuid::now_v7(),
                        automation_id: automation.id,
                        object_id: automation.object_id,
                        record: record.public_id.clone(),
                        event,
                        outcome: RunOutcome::Failed,
                        detail: Some(format!(
                            "automations queued jobs more than {DEPTH_MAX} levels deep"
                        )),
                        saves: 0,
                        ran_at: now,
                        ran_by: Some(self.caller().user_id()),
                        correlation_id: Some(context.correlation_id),
                    }],
                )
                .await?;
                continue;
            }
            tx.enqueue_job(&AutomationJob {
                id: Uuid::now_v7(),
                automation_id: automation.id,
                object_id: object.id,
                record_id: record.id,
                record: record.public_id.clone(),
                event,
                scheduled,
                due_at,
                status: JobStatus::Pending,
                attempts: 0,
                locked_until: None,
                last_error: None,
                depth: i32::try_from(run.job_depth + 1).unwrap_or(i32::MAX),
                actor_id: self.caller().user_id(),
                correlation_id: context.correlation_id,
                created_at: now,
                updated_at: now,
            })
            .await?;
        }
        Ok(())
    }

    /// One after-save automation: `None` when its condition is not met.
    /// Background jobs pass `check_condition: false`: their condition held
    /// when they were queued, and `ISCHANGED` has no meaning later.
    #[allow(clippy::too_many_arguments)]
    async fn act(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        objects: &BTreeMap<Uuid, ObjectDefinition>,
        automation: &Automation,
        prior: Option<&Record>,
        record: &Record,
        check_condition: bool,
        context: MutationContext,
        run: &mut Run,
    ) -> Result<Option<Acted>, ObjectError> {
        let compiled = self
            .compile_for_run(tx, object, objects, automation)
            .await?;
        let Some(bodies) = self
            .evaluate(
                tx,
                object,
                objects,
                automation,
                &compiled,
                prior,
                record,
                check_condition,
            )
            .await?
        else {
            return Ok(None);
        };
        let context = MutationContext {
            automation_id: Some(automation.id),
            ..context
        };
        let mut saves = 0;
        for ((action, compiled), body) in
            automation.actions.iter().zip(&compiled.actions).zip(bodies)
        {
            let Some(body) = body else {
                continue;
            };
            let target = &compiled.target;
            if let AutomationAction::PublishEvent { event, .. } = action {
                self.publish(tx, object, automation, event, record, body, context)
                    .await?;
                continue;
            }
            let current =
                match action {
                    AutomationAction::UpdateRelated { lookup, .. } => {
                        let Some(public_id) = record
                            .values
                            .get(lookup)
                            .and_then(serde_json::Value::as_str)
                        else {
                            continue;
                        };
                        match tx.record_by_public_id(public_id).await?.filter(|found| {
                            found.object_id == target.id && found.deleted_at.is_none()
                        }) {
                            Some(found) => Some(found),
                            None => continue,
                        }
                    }
                    AutomationAction::CreateRecord { .. }
                    | AutomationAction::PublishEvent { .. } => None,
                    // Background automations update the record they ran
                    // for, as it is now.
                    AutomationAction::UpdateRecord { .. } => match tx
                        .record_by_id(record.id)
                        .await?
                        .filter(|found| found.deleted_at.is_none())
                    {
                        Some(found) => Some(found),
                        None => continue,
                    },
                };
            run.saves += 1;
            if run.saves > SAVES_MAX {
                return Err(failed(
                    automation,
                    format!("automations saved more than {SAVES_MAX} records in one change"),
                ));
            }
            if run.depth >= DEPTH_MAX {
                return Err(failed(
                    automation,
                    format!("automations ran more than {DEPTH_MAX} levels deep"),
                ));
            }
            run.depth += 1;
            // An update that changes nothing is not written and keeps its
            // version; it does not count as a save.
            let prior_version = current.as_ref().map(|current| current.version);
            let saved = match current {
                Some(current) => {
                    self.update_existing(tx, None, target, current, &body, context, run)
                        .await
                }
                None => {
                    self.insert_new(tx, None, target, &body, context, run, None)
                        .await
                }
            };
            run.depth -= 1;
            let saved = saved.map_err(|error| match error {
                ObjectError::Validation(issues) => failed(
                    automation,
                    format!(
                        "could not save the {} record: {}",
                        target.label,
                        describe(&issues)
                    ),
                ),
                other => other,
            })?;
            if prior_version != Some(saved.version) {
                saves += 1;
            }
        }
        Ok(Some(Acted { saves }))
    }

    /// One attempt at a claimed job, in its own transaction.
    async fn execute_job(
        &self,
        job: &AutomationJob,
        now: chrono::DateTime<Utc>,
        run: &mut Run,
    ) -> Result<JobOutcome, ObjectError> {
        let mut tx = self.begin().await?;
        let automation = tx
            .automations(job.object_id)
            .await?
            .into_iter()
            .find(|automation| automation.id == job.automation_id && automation.active);
        let object = tx
            .object_by_id(job.object_id)
            .await?
            .filter(ObjectDefinition::is_active);
        let record = tx
            .record_by_id(job.record_id)
            .await?
            .filter(|record| job.event == SaveEvent::Delete || record.deleted_at.is_none());
        let (Some(automation), Some(object), Some(record)) = (automation, object, record) else {
            let mut job = job.clone();
            job.status = JobStatus::Cancelled;
            job.locked_until = None;
            job.last_error =
                Some("its automation was switched off or deleted, or its record is gone".into());
            job.updated_at = now;
            if !tx.update_job(&job, JobStatus::Running).await? {
                // Another worker reclaimed it after this claim lapsed.
                return Ok(JobOutcome::NotClaimed);
            }
            tx.commit().await?;
            return Ok(JobOutcome::Cancelled);
        };
        let objects = formula_objects(tx.as_mut(), &object).await?;
        let context = MutationContext {
            correlation_id: job.correlation_id,
            causation_id: Some(job.id),
            automation_id: None,
        };
        // The condition held when the job was queued and is not checked
        // again; the values are those of the record now.
        let result = self
            .act(
                tx.as_mut(),
                &object,
                &objects,
                &automation,
                Some(&record),
                &record,
                false,
                context,
                run,
            )
            .await;
        let acted = result?.map_or(0, |acted| acted.saves);
        let mut job = job.clone();
        job.status = JobStatus::Succeeded;
        job.locked_until = None;
        job.last_error = None;
        job.updated_at = now;
        // Only the claim that is still current may complete the job; a
        // worker whose claim lapsed rolls its changes back here, so the
        // automation never applies twice.
        if !tx.update_job(&job, JobStatus::Running).await? {
            return Ok(JobOutcome::NotClaimed);
        }
        store_runs(
            tx.as_mut(),
            &[AutomationRun {
                id: Uuid::now_v7(),
                automation_id: automation.id,
                object_id: automation.object_id,
                record: record.public_id.clone(),
                event: job.event,
                outcome: RunOutcome::Succeeded,
                detail: None,
                saves: i64::try_from(acted).unwrap_or(i64::MAX),
                ran_at: now,
                ran_by: Some(job.actor_id),
                correlation_id: Some(job.correlation_id),
            }],
        )
        .await?;
        tx.commit().await?;
        Ok(JobOutcome::Succeeded)
    }

    /// Records a failed attempt: the job waits for a retry, or fails for
    /// good after its last attempt, and the run says why.
    async fn job_failed(
        &self,
        mut job: AutomationJob,
        error: &ObjectError,
        now: chrono::DateTime<Utc>,
        run: Run,
    ) -> Result<JobOutcome, ObjectError> {
        let reason = match error {
            ObjectError::Validation(issues) => describe(issues),
            ObjectError::Conflict(code) => format!("the change conflicted ({code})"),
            ObjectError::VersionConflict => "a record changed while the job ran".to_owned(),
            _ => "the service was unavailable".to_owned(),
        };
        let detail: String = format!("attempt {}: {reason}", job.attempts)
            .chars()
            .take(2000)
            .collect();
        let outcome = if job.attempts >= JOB_ATTEMPTS_MAX {
            job.status = JobStatus::Failed;
            JobOutcome::Failed
        } else {
            job.status = JobStatus::Pending;
            job.due_at = now + retry_delay(job.attempts);
            JobOutcome::Retrying
        };
        job.locked_until = None;
        job.last_error = Some(detail.clone());
        job.updated_at = now;
        let mut runs = run.rolled_back();
        runs.push(AutomationRun {
            id: Uuid::now_v7(),
            automation_id: job.automation_id,
            object_id: job.object_id,
            record: job.record.clone(),
            event: job.event,
            outcome: RunOutcome::Failed,
            detail: Some(detail),
            saves: 0,
            ran_at: now,
            ran_by: Some(job.actor_id),
            correlation_id: Some(job.correlation_id),
        });
        let mut tx = self.begin().await?;
        if !tx.update_job(&job, JobStatus::Running).await? {
            return Ok(JobOutcome::NotClaimed);
        }
        store_runs(tx.as_mut(), &runs).await?;
        tx.commit().await?;
        Ok(outcome)
    }

    /// An automation's queued jobs, newest first (ADR 0022 §6).
    pub async fn automation_jobs(
        &self,
        api_name: &str,
        name: &str,
        limit: u32,
    ) -> Result<Vec<AutomationJob>, ObjectError> {
        self.caller().require_manage()?;
        if !(1..=200).contains(&limit) {
            return Err(ObjectError::invalid(
                "limit",
                "automations.limit_invalid",
                "ask for between 1 and 200 jobs",
            ));
        }
        let mut tx = self.begin().await?;
        let (_, automation) = self.named_automation(tx.as_mut(), api_name, name).await?;
        let jobs = tx.automation_jobs(automation.id, limit).await?;
        tx.commit().await?;
        Ok(jobs)
    }

    /// Retries a failed job now, with a fresh set of attempts, or cancels a
    /// pending one. Anything else is `automations.job_state`.
    pub async fn change_automation_job(
        &self,
        api_name: &str,
        name: &str,
        job_id: Uuid,
        retry: bool,
        context: MutationContext,
    ) -> Result<AutomationJob, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        let (object, automation) = self.named_automation(tx.as_mut(), api_name, name).await?;
        let mut job = tx
            .job(job_id)
            .await?
            .filter(|job| job.automation_id == automation.id)
            .ok_or(ObjectError::NotFound)?;
        let now = Utc::now();
        let from = job.status;
        match (retry, job.status) {
            (true, JobStatus::Failed) => {
                job.status = JobStatus::Pending;
                job.attempts = 0;
                job.due_at = now;
            }
            (false, JobStatus::Pending) => job.status = JobStatus::Cancelled,
            _ => return Err(ObjectError::Conflict("automations.job_state")),
        }
        job.updated_at = now;
        // A worker may have claimed it since it was read.
        if !tx.update_job(&job, from).await? {
            return Err(ObjectError::Conflict("automations.job_state"));
        }
        self.record_change(
            tx.as_mut(),
            context,
            if retry {
                Change::AutomationJobRetried(&object, &automation, &job)
            } else {
                Change::AutomationJobCancelled(&object, &automation, &job)
            },
        )
        .await?;
        tx.commit().await?;
        Ok(job)
    }

    async fn named_automation(
        &self,
        tx: &mut dyn ObjectTx,
        api_name: &str,
        name: &str,
    ) -> Result<(ObjectDefinition, Automation), ObjectError> {
        let object = self.active_object(tx, api_name).await?;
        let automation = tx
            .automations(object.id)
            .await?
            .into_iter()
            .find(|automation| automation.name == name)
            .ok_or(ObjectError::NotFound)?;
        Ok((object, automation))
    }

    /// Publishes a workspace-defined platform event in the change's
    /// transaction, through the outbox (ADR 0022 §7).
    #[allow(clippy::too_many_arguments)]
    async fn publish(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        automation: &Automation,
        name: &str,
        record: &Record,
        fields: Map<String, serde_json::Value>,
        context: MutationContext,
    ) -> Result<(), ObjectError> {
        let context = MutationContext {
            automation_id: Some(automation.id),
            ..context
        };
        let (audit, event) = crate::events::workspace_event(
            self.evidence(context),
            object,
            automation,
            name,
            record,
            fields,
        );
        // The cap applies to the whole payload delivered, not just the
        // fields the automation computed.
        let size = serde_json::to_vec(&event.payload).map_or(usize::MAX, |bytes| bytes.len());
        if size > EVENT_BYTES_MAX {
            return Err(failed(
                automation,
                format!(
                    "the event {name} is larger than {} KB",
                    EVENT_BYTES_MAX / 1024
                ),
            ));
        }
        tx.append_evidence(&audit, &event).await
    }

    /// Writes the runs a rolled-back change failed in, in a transaction of
    /// their own, and returns the change's result. Writing them is best
    /// effort: the change's own error is what the caller needs.
    pub(crate) async fn keep_failed_runs<T>(
        &self,
        run: Run,
        result: Result<T, ObjectError>,
    ) -> Result<T, ObjectError> {
        let runs = if result.is_err() {
            run.rolled_back()
        } else {
            Vec::new()
        };
        if !runs.is_empty() {
            if let Ok(mut tx) = self.begin().await {
                if store_runs(tx.as_mut(), &runs).await.is_ok() {
                    let _ = tx.commit().await;
                }
            }
        }
        result
    }

    /// A page of an automation's runs, newest first, and the cursor of the
    /// next page (ADR 0022 §5).
    pub async fn automation_runs(
        &self,
        api_name: &str,
        name: &str,
        cursor: Option<&str>,
        limit: u32,
    ) -> Result<(Vec<AutomationRun>, Option<String>), ObjectError> {
        self.caller().require_manage()?;
        if !(1..=200).contains(&limit) {
            return Err(ObjectError::invalid(
                "limit",
                "automations.limit_invalid",
                "ask for between 1 and 200 runs",
            ));
        }
        let before = cursor.map(decode_cursor).transpose()?;
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let automation = tx
            .automations(object.id)
            .await?
            .into_iter()
            .find(|automation| automation.name == name)
            .ok_or(ObjectError::NotFound)?;
        // Reading also enforces retention, so a workspace that stopped
        // recording runs does not keep showing old ones.
        tx.expire_automation_runs(Utc::now() - RUN_RETENTION)
            .await?;
        let mut runs = tx.automation_runs(automation.id, before, limit + 1).await?;
        tx.commit().await?;
        let next = if runs.len() > limit as usize {
            runs.truncate(limit as usize);
            runs.last().map(|run| encode_cursor(run.ran_at, run.id))
        } else {
            None
        };
        Ok((runs, next))
    }

    // --- Administration -------------------------------------------------

    pub async fn automations(&self, api_name: &str) -> Result<Vec<Automation>, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let automations = tx.automations(object.id).await?;
        tx.commit().await?;
        Ok(automations)
    }

    async fn checked(
        &self,
        tx: &mut dyn ObjectTx,
        object: &ObjectDefinition,
        automation: &Automation,
    ) -> Result<(), ObjectError> {
        let objects = formula_objects(tx, object).await?;
        let targets = action_targets(tx, object, &automation.actions).await?;
        let issues = check_automation(automation, object, &objects, &targets);
        if issues.is_empty() {
            Ok(())
        } else {
            Err(ObjectError::Validation(issues))
        }
    }

    pub async fn create_automation(
        &self,
        api_name: &str,
        input: CreateAutomation,
        context: MutationContext,
    ) -> Result<Automation, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        // Serialised with field deletion, which checks what depends on a field.
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if tx.automations(object.id).await?.len() >= AUTOMATIONS_PER_OBJECT_MAX {
            return Err(ObjectError::invalid(
                "name",
                "automations.limit_reached",
                format!("an object can have at most {AUTOMATIONS_PER_OBJECT_MAX} automations"),
            ));
        }
        let now = Utc::now();
        let actor = Some(self.caller().user_id());
        let automation = Automation {
            id: Uuid::now_v7(),
            object_id: object.id,
            name: input.name.trim().to_owned(),
            description: clean(input.description),
            trigger: input.trigger,
            timing: input.timing,
            condition: clean(input.condition),
            actions: input.actions,
            schedule: input.schedule,
            active: input.active.unwrap_or(true),
            version: 1,
            created_at: now,
            created_by: actor,
            updated_at: now,
            updated_by: actor,
        };
        self.checked(tx.as_mut(), &object, &automation).await?;
        tx.insert_automation(&automation).await?;
        self.record_change(
            tx.as_mut(),
            context,
            Change::AutomationCreated(&object, &automation),
        )
        .await?;
        tx.commit().await?;
        Ok(automation)
    }

    pub async fn update_automation(
        &self,
        api_name: &str,
        name: &str,
        input: UpdateAutomation,
        context: MutationContext,
    ) -> Result<Automation, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        tx.lock_metadata().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let mut automation = tx
            .automations(object.id)
            .await?
            .into_iter()
            .find(|automation| automation.name == name)
            .ok_or(ObjectError::NotFound)?;
        if automation.version != input.expected_version {
            return Err(ObjectError::VersionConflict);
        }
        if let Some(description) = input.description {
            automation.description = clean(description);
        }
        if let Some(trigger) = input.trigger {
            automation.trigger = trigger;
        }
        if let Some(condition) = input.condition {
            automation.condition = clean(condition);
        }
        if let Some(actions) = input.actions {
            automation.actions = actions;
        }
        if let Some(schedule) = input.schedule {
            automation.schedule = Some(schedule);
        }
        if let Some(active) = input.active {
            automation.active = active;
        }
        self.checked(tx.as_mut(), &object, &automation).await?;
        automation.version += 1;
        automation.updated_at = Utc::now();
        automation.updated_by = Some(self.caller().user_id());
        if !tx
            .update_automation(&automation, input.expected_version)
            .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::AutomationUpdated(&object, &automation),
        )
        .await?;
        tx.commit().await?;
        Ok(automation)
    }

    pub async fn delete_automation(
        &self,
        api_name: &str,
        name: &str,
        expected_version: i64,
        context: MutationContext,
    ) -> Result<Automation, ObjectError> {
        self.caller().require_manage()?;
        let mut tx = self.begin().await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let automation = tx
            .automations(object.id)
            .await?
            .into_iter()
            .find(|automation| automation.name == name)
            .ok_or(ObjectError::NotFound)?;
        if automation.version != expected_version
            || !tx
                .delete_automation(automation.id, expected_version)
                .await?
        {
            return Err(ObjectError::VersionConflict);
        }
        self.record_change(
            tx.as_mut(),
            context,
            Change::AutomationDeleted(&object, &automation),
        )
        .await?;
        tx.commit().await?;
        Ok(automation)
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct RunCursor {
    t: chrono::DateTime<Utc>,
    i: Uuid,
}

fn encode_cursor(ran_at: chrono::DateTime<Utc>, id: Uuid) -> String {
    use base64::Engine;
    let token = serde_json::to_vec(&RunCursor { t: ran_at, i: id }).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(token)
}

fn decode_cursor(cursor: &str) -> Result<(chrono::DateTime<Utc>, Uuid), ObjectError> {
    use base64::Engine;
    let invalid = || {
        ObjectError::invalid(
            "cursor",
            "automations.cursor_invalid",
            "start the list again",
        )
    };
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| invalid())?;
    let token: RunCursor = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    Ok((token.t, token.i))
}

/// When a scheduled automation's job is due for a record: `None` when its
/// date field is blank.
fn scheduled_time(
    schedule: &Schedule,
    object: &ObjectDefinition,
    record: &Record,
    now: chrono::DateTime<Utc>,
) -> Option<chrono::DateTime<Utc>> {
    let base = match &schedule.from {
        None => now,
        Some(from) => match formula::record_value(
            record,
            &formula::Leaf::Field {
                api_name: from.clone(),
                field_type: object.field(from)?.field_type,
            },
        ) {
            Value::Date(date) => date.and_hms_opt(0, 0, 0)?.and_utc(),
            Value::DateTime(time) => time,
            _ => return None,
        },
    };
    base.checked_add_signed(chrono::TimeDelta::try_minutes(schedule.offset_minutes)?)
}

/// Attempts a job gets before it fails for good.
pub const JOB_ATTEMPTS_MAX: i32 = 5;
/// How long a worker holds a job it claimed.
pub const JOB_LOCK: chrono::TimeDelta = chrono::TimeDelta::minutes(10);

/// The wait before retrying a job after `attempts` attempts.
pub fn retry_delay(attempts: i32) -> chrono::TimeDelta {
    match attempts {
        ..=1 => chrono::TimeDelta::minutes(1),
        2 => chrono::TimeDelta::minutes(5),
        3 => chrono::TimeDelta::minutes(30),
        _ => chrono::TimeDelta::hours(2),
    }
}

/// What running a job came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobOutcome {
    /// Not due, or another worker holds it.
    NotClaimed,
    Succeeded,
    /// No longer needed: its automation or record is gone.
    Cancelled,
    /// It failed and will be tried again.
    Retrying,
    /// It failed its last attempt.
    Failed,
}

/// Runs a queued automation job in its workspace (ADR 0022 §6): claims it,
/// then runs it in a transaction of its own as the member whose change
/// queued it, in system context. A failure is recorded in a third
/// transaction and the job is retried with backoff, up to
/// [`JOB_ATTEMPTS_MAX`] attempts.
pub async fn run_job(
    store: std::sync::Arc<dyn ObjectStore>,
    tenant_id: Uuid,
    job_id: Uuid,
    now: chrono::DateTime<Utc>,
) -> Result<JobOutcome, ObjectError> {
    let mut tx = store.begin(tenant_id).await?;
    let claimed = tx.claim_job(job_id, now, now + JOB_LOCK).await?;
    tx.commit().await?;
    let Some(job) = claimed else {
        return Ok(JobOutcome::NotClaimed);
    };
    let service = ObjectService::new(
        store,
        ObjectCaller::new(tenant_id, job.actor_id, Default::default(), Vec::new()),
    );
    let mut run = Run {
        job_depth: usize::try_from(job.depth).unwrap_or(DEPTH_MAX),
        job: Some((job.automation_id, job.record_id)),
        ..Run::default()
    };
    match service.execute_job(&job, now, &mut run).await {
        Ok(outcome) => Ok(outcome),
        Err(error) => service.job_failed(job, &error, now, run).await,
    }
}

/// Records runs, first expiring the workspace's runs older than
/// [`RUN_RETENTION`]. Every write expires, so retention holds however few
/// runs a workspace records; the delete is an index probe when nothing has
/// expired.
async fn store_runs(tx: &mut dyn ObjectTx, runs: &[AutomationRun]) -> Result<(), ObjectError> {
    tx.expire_automation_runs(Utc::now() - RUN_RETENTION)
        .await?;
    tx.insert_automation_runs(runs).await
}

/// Trimmed text, with blank text as `None`.
fn clean(text: Option<String>) -> Option<String> {
    text.map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{assigned, describe, valid_name};
    use crate::formula::Value;
    use crate::ValidationIssue;
    use serde_json::json;

    #[test]
    fn automation_names_are_short_lowercase_identifiers() {
        assert!(valid_name("set_region"));
        for name in ["", "Set", "1st", "a-b", &"a".repeat(64)] {
            assert!(!valid_name(name), "{name}");
        }
    }

    #[test]
    fn numbers_are_rounded_to_the_field_scale() {
        assert_eq!(assigned(Value::Number(1.23456), Some(2)), json!(1.23));
        assert_eq!(assigned(Value::Number(2.5), None), json!(2.5));
        assert_eq!(assigned(Value::Null, Some(2)), json!(null));
    }

    #[test]
    fn issues_read_as_one_line() {
        let issues = [
            ValidationIssue::new("custom:x", "records.value_required", "X is required"),
            ValidationIssue::new("record", "records.validation_rule", "Too big"),
        ];
        assert_eq!(describe(&issues), "custom:x: X is required; Too big");
    }
}
