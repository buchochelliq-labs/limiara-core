//! Validation and normalisation of definitions and record values, and the
//! pivot rows derived from them (ADR 0015 section 2).
//!
//! Every value is normalised here before any backend sees it, so the stored
//! document is never free-form and every backend indexes the same values.

use crate::{
    CreateField, DeleteBehaviour, FieldDefinition, FieldType, IndexEntry, IndexValue,
    ObjectDefinition, PicklistOption, RecordIndex, ReferenceEntry, UniqueEntry, ValidationIssue,
};
use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use serde_json::{Number, Value};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const LABEL_MAX: usize = 80;
pub const DESCRIPTION_MAX: usize = 1000;
pub const NAME_MAX: usize = 255;
pub const TEXT_DEFAULT_MAX: i32 = 255;
pub const TEXT_LIMIT: i32 = 255;
pub const LONG_TEXT_DEFAULT_MAX: i32 = 32_768;
/// The longest email address a field may allow (RFC 5321's 64-character
/// local part, `@` and 255-character domain; ADR 0035 G8). Without a
/// declared `max_length`, an email field allows 255, as before.
pub const EMAIL_LIMIT: i32 = 320;
pub const LONG_TEXT_LIMIT: i32 = 131_072;
pub const OPTIONS_MAX: usize = 1000;
pub const FIELDS_PER_OBJECT_MAX: usize = 500;
/// Master-detail fields one object may have; two make a junction object.
pub const MASTERS_PER_OBJECT_MAX: usize = 2;
/// Roll-up summary fields one object may have, as in Salesforce.
pub const ROLLUPS_PER_OBJECT_MAX: usize = 25;
pub const SCALE_LIMIT: i32 = 8;
/// Objects one related-to field may point to (ADR 0033 §1).
pub const REFERENCE_OBJECTS_MAX: usize = 10;

/// The key names are searched and sorted by: Unicode lowercase, trimmed.
pub fn name_key(value: &str) -> String {
    value.trim().to_lowercase()
}

pub fn label(field: &str, value: &str, max: usize) -> Result<String, ValidationIssue> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max || value.chars().any(char::is_control) {
        return Err(ValidationIssue::new(
            field,
            "objects.label_invalid",
            format!("enter between 1 and {max} printable characters"),
        ));
    }
    Ok(value.to_owned())
}

pub fn description(value: Option<&str>) -> Result<Option<String>, ValidationIssue> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(value) if value.chars().count() <= DESCRIPTION_MAX => Ok(Some(value.to_owned())),
        Some(_) => Err(ValidationIssue::new(
            "description",
            "objects.description_too_long",
            format!("use at most {DESCRIPTION_MAX} characters"),
        )),
    }
}

/// The record name every engine record carries (the standard `name` field).
pub fn record_name(value: &Value) -> Result<String, ValidationIssue> {
    let issue = || {
        ValidationIssue::new(
            "name",
            "records.name_invalid",
            format!("enter a name of 1 to {NAME_MAX} characters"),
        )
    };
    let text = value.as_str().ok_or_else(issue)?.trim();
    if text.is_empty() || text.chars().count() > NAME_MAX || text.chars().any(char::is_control) {
        return Err(issue());
    }
    Ok(text.to_owned())
}

pub fn picklist_options(
    field_type: FieldType,
    options: &[PicklistOption],
) -> Result<Vec<PicklistOption>, ValidationIssue> {
    let picklist = matches!(field_type, FieldType::Picklist | FieldType::MultiPicklist);
    if !picklist {
        return if options.is_empty() {
            Ok(Vec::new())
        } else {
            Err(ValidationIssue::new(
                "options",
                "objects.options_not_allowed",
                "only picklist fields have options",
            ))
        };
    }
    let invalid = |detail: &str| ValidationIssue::new("options", "objects.options_invalid", detail);
    if options.is_empty() || options.len() > OPTIONS_MAX {
        return Err(invalid("a picklist needs between 1 and 1000 options"));
    }
    let mut seen = BTreeSet::new();
    let mut normalised = Vec::with_capacity(options.len());
    for option in options {
        let value = option.value.trim();
        if value.is_empty()
            || value.chars().count() > NAME_MAX
            || value.chars().any(char::is_control)
        {
            return Err(invalid(
                "each option value needs 1 to 255 printable characters",
            ));
        }
        if !seen.insert(value.to_lowercase()) {
            return Err(invalid("option values must be unique, ignoring case"));
        }
        let label = label("options", &option.label, NAME_MAX)?;
        if option.weight.is_some_and(|weight| weight > 100) {
            return Err(invalid("an option's weight is a percentage from 0 to 100"));
        }
        // A record holds several values of a multi-select picklist at once,
        // so its values cannot close it.
        if field_type == FieldType::MultiPicklist && option.outcome.is_closed() {
            return Err(invalid(
                "only a single-value picklist's options may have an outcome",
            ));
        }
        normalised.push(PicklistOption {
            value: value.to_owned(),
            label,
            active: option.active,
            default: option.default && option.active,
            outcome: option.outcome,
            weight: option.weight,
        });
    }
    if field_type == FieldType::Picklist
        && normalised.iter().filter(|option| option.default).count() > 1
    {
        return Err(invalid("a picklist has at most one default value"));
    }
    Ok(normalised)
}

/// The settings a new field may use, checked against its type.
pub struct FieldShape {
    pub max_length: Option<i32>,
    pub scale: Option<i32>,
    pub options: Vec<PicklistOption>,
    pub unique: bool,
    pub delete_behaviour: Option<DeleteBehaviour>,
    pub formula: Option<String>,
    pub formula_type: Option<FieldType>,
    /// A related-to field's allowlist, by API name.
    pub reference_objects: Vec<String>,
}

/// Checks the shape of a related-to field's allowlist: only related-to
/// fields have one, and it names 1 to 10 distinct objects. Whether they
/// exist is the service's to check. Returns the trimmed names.
/// A related-to field's allowlist: 1 to 10 distinct names, or 0 to 10 for a
/// plugin's own field (`plugin_owned`), whose targets the workspace's
/// administrators choose after the plugin installs it (ADR 0033 §2).
pub fn reference_objects(
    kind: FieldType,
    names: Option<&[String]>,
    plugin_owned: bool,
) -> Result<Vec<String>, ValidationIssue> {
    let invalid = |detail: &str| {
        ValidationIssue::new(
            "reference_objects",
            "objects.reference_objects_invalid",
            detail,
        )
    };
    let Some(names) = names else {
        return if kind == FieldType::RelatedTo {
            Err(invalid(
                "a related-to field needs the objects it may point to",
            ))
        } else {
            Ok(Vec::new())
        };
    };
    if kind != FieldType::RelatedTo {
        return Err(invalid("only related-to fields have a list of objects"));
    }
    let least = usize::from(!plugin_owned);
    if names.len() < least || names.len() > REFERENCE_OBJECTS_MAX {
        return Err(invalid(&format!(
            "choose between {least} and {REFERENCE_OBJECTS_MAX} objects"
        )));
    }
    let mut seen = BTreeSet::new();
    let mut trimmed = Vec::with_capacity(names.len());
    for name in names {
        let name = name.trim();
        if !seen.insert(name.to_owned()) {
            return Err(invalid("choose each object once"));
        }
        trimmed.push(name.to_owned());
    }
    Ok(trimmed)
}

/// The types a formula field may give.
pub const FORMULA_TYPES: [FieldType; 7] = [
    FieldType::Text,
    FieldType::Number,
    FieldType::Currency,
    FieldType::Percent,
    FieldType::Boolean,
    FieldType::Date,
    FieldType::DateTime,
];

pub fn field_shape(input: &CreateField) -> Result<FieldShape, Vec<ValidationIssue>> {
    field_shape_for(input, false)
}

/// [`field_shape`] for a field a plugin declares (`plugin_owned`), whose
/// related-to allowlist may start empty (ADR 0033 §2).
pub fn field_shape_for(
    input: &CreateField,
    plugin_owned: bool,
) -> Result<FieldShape, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    let kind = input.field_type;
    let max_length = match kind {
        FieldType::Text => Some(input.max_length.unwrap_or(TEXT_DEFAULT_MAX)),
        FieldType::LongText => Some(input.max_length.unwrap_or(LONG_TEXT_DEFAULT_MAX)),
        _ => input.max_length,
    };
    let limit = match kind {
        FieldType::Text => Some(TEXT_LIMIT),
        FieldType::LongText => Some(LONG_TEXT_LIMIT),
        FieldType::Email => Some(EMAIL_LIMIT),
        _ => None,
    };
    match (max_length, limit) {
        (Some(length), Some(limit)) if !(1..=limit).contains(&length) => {
            issues.push(ValidationIssue::new(
                "max_length",
                "objects.max_length_invalid",
                format!("use 1 to {limit}"),
            ))
        }
        (Some(_), None) => issues.push(ValidationIssue::new(
            "max_length",
            "objects.max_length_not_allowed",
            "only text and email fields have a maximum length",
        )),
        _ => {}
    }
    let scale = match kind {
        FieldType::Currency | FieldType::Percent => Some(input.scale.unwrap_or(2)),
        FieldType::Number => Some(input.scale.unwrap_or(0)),
        _ => input.scale,
    };
    match (scale, kind.is_numeric() || kind == FieldType::Formula) {
        (Some(scale), true) if !(0..=SCALE_LIMIT).contains(&scale) => {
            issues.push(ValidationIssue::new(
                "scale",
                "objects.scale_invalid",
                "use 0 to 8 decimal places",
            ))
        }
        (Some(_), false) => issues.push(ValidationIssue::new(
            "scale",
            "objects.scale_not_allowed",
            "only number, currency and percent fields have decimal places",
        )),
        _ => {}
    }
    let options = picklist_options(kind, &input.options).unwrap_or_else(|issue| {
        issues.push(issue);
        Vec::new()
    });
    if input.unique && !kind.may_be_unique() {
        issues.push(ValidationIssue::new(
            "unique",
            "objects.unique_not_allowed",
            "only text, email, phone, URL and number fields can be unique",
        ));
    }
    let delete_behaviour = match (kind, input.delete_behaviour) {
        (FieldType::Lookup, behaviour) => Some(behaviour.unwrap_or_default()),
        // A related-to value is always cleared when its target goes.
        (FieldType::RelatedTo, None | Some(DeleteBehaviour::Clear)) => Some(DeleteBehaviour::Clear),
        (FieldType::RelatedTo, Some(_)) => {
            issues.push(ValidationIssue::new(
                "delete_behaviour",
                "objects.delete_behaviour_not_allowed",
                "deleting the target of a related-to field always clears it",
            ));
            None
        }
        (FieldType::MasterDetail, Some(_)) => {
            issues.push(ValidationIssue::new(
                "delete_behaviour",
                "objects.delete_behaviour_not_allowed",
                "deleting a master always deletes its details",
            ));
            None
        }
        (_, Some(_)) => {
            issues.push(ValidationIssue::new(
                "delete_behaviour",
                "objects.delete_behaviour_not_allowed",
                "only lookup fields have a delete behaviour",
            ));
            None
        }
        (_, None) => None,
    };
    if kind.is_reference() != input.reference_object.is_some() {
        issues.push(ValidationIssue::new(
            "reference_object",
            "objects.reference_object_invalid",
            "a lookup needs the object it points to, and only lookups have one",
        ));
    }
    let reference_objects =
        reference_objects(kind, input.reference_objects.as_deref(), plugin_owned).unwrap_or_else(
            |issue| {
                issues.push(issue);
                Vec::new()
            },
        );
    if kind == FieldType::MasterDetail && (input.unique || input.default_value.is_some()) {
        issues.push(ValidationIssue::new(
            "field_type",
            "objects.master_detail_settings_invalid",
            "a master-detail field is never unique and has no default value",
        ));
    }
    if (kind == FieldType::Rollup) != input.rollup.is_some() {
        issues.push(ValidationIssue::new(
            "rollup",
            "objects.rollup_invalid",
            "a roll-up summary field needs a roll-up, and only those have one",
        ));
    }
    if kind == FieldType::Rollup
        && (input.required
            || input.unique
            || input.searchable
            || input.track_history
            || input.default_value.is_some())
    {
        issues.push(ValidationIssue::new(
            "field_type",
            "objects.rollup_settings_invalid",
            "roll-up summary fields cannot be required, unique, searchable, tracked or defaulted",
        ));
    }
    let formula = input
        .formula
        .as_ref()
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty());
    let formula_type = if kind == FieldType::Formula {
        if formula.is_none() {
            issues.push(ValidationIssue::new(
                "formula",
                "objects.formula_required",
                "a formula field needs a formula",
            ));
        }
        match input.formula_type {
            Some(result) if FORMULA_TYPES.contains(&result) => Some(result),
            _ => {
                issues.push(ValidationIssue::new(
                    "formula_type",
                    "objects.formula_type_invalid",
                    "choose text, number, currency, percent, boolean, date or date_time",
                ));
                None
            }
        }
    } else {
        if formula.is_some() || input.formula_type.is_some() {
            issues.push(ValidationIssue::new(
                "formula",
                "objects.formula_not_allowed",
                "only formula fields have a formula",
            ));
        }
        None
    };
    if kind == FieldType::Formula
        && (input.required
            || input.unique
            || input.indexed
            || input.searchable
            || input.track_history
            || input.default_value.is_some())
    {
        issues.push(ValidationIssue::new(
            "field_type",
            "objects.formula_settings_invalid",
            "formula fields cannot be required, unique, indexed, searchable, tracked or defaulted",
        ));
    }
    // A formula field's number of decimal places applies to its result.
    let scale = match (kind, formula_type) {
        (FieldType::Formula, Some(FieldType::Currency | FieldType::Percent)) => {
            Some(input.scale.unwrap_or(2).clamp(0, SCALE_LIMIT))
        }
        (FieldType::Formula, Some(FieldType::Number)) => {
            Some(input.scale.unwrap_or(0).clamp(0, SCALE_LIMIT))
        }
        (FieldType::Formula, _) => None,
        _ => scale,
    };
    if issues.is_empty() {
        Ok(FieldShape {
            max_length,
            scale,
            options,
            unique: input.unique,
            delete_behaviour,
            formula: if kind == FieldType::Formula {
                formula
            } else {
                None
            },
            formula_type,
            reference_objects,
        })
    } else {
        Err(issues)
    }
}

fn issue(
    field: &FieldDefinition,
    code: &'static str,
    detail: impl Into<String>,
) -> ValidationIssue {
    ValidationIssue::new(field.api_name.clone(), code, detail)
}

fn text_value(field: &FieldDefinition, value: &Value) -> Result<Option<String>, ValidationIssue> {
    let text = value
        .as_str()
        .ok_or_else(|| issue(field, "records.value_type", "enter text"))?
        .trim();
    if text.is_empty() {
        return Ok(None);
    }
    let max = field.max_length.unwrap_or(TEXT_DEFAULT_MAX);
    let length = i32::try_from(text.chars().count()).unwrap_or(i32::MAX);
    if length > max {
        return Err(issue(
            field,
            "records.value_too_long",
            format!("use at most {max} characters"),
        ));
    }
    let multiline = field.field_type == FieldType::LongText;
    if text
        .chars()
        .any(|c| c.is_control() && !(multiline && (c == '\n' || c == '\t')))
    {
        return Err(issue(
            field,
            "records.value_invalid",
            "remove control characters",
        ));
    }
    Ok(Some(text.to_owned()))
}

fn valid_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    // RFC 5321's limits on the parts; the field's `max_length` bounds the
    // whole address (255 unless the field allows up to 320, ADR 0035 G8).
    local.len() <= 64
        && domain.len() <= 255
        && !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !value
            .chars()
            .any(|c| c.is_whitespace() || c == '<' || c == '>')
        && !domain.contains('@')
}

fn valid_phone(value: &str) -> bool {
    let digits = value.chars().filter(char::is_ascii_digit).count();
    value.len() <= 40
        && digits >= 3
        && value
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '+' | '(' | ')' | '-' | '.'))
}

fn valid_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    value.len() <= 2048
        && (lower.starts_with("https://") || lower.starts_with("http://"))
        && lower.len() > lower.find("://").map_or(0, |index| index + 3)
        && !value.chars().any(char::is_whitespace)
}

/// Significant decimal places: trailing zeros do not count, so `1.0`, which
/// is how a normalised whole number reads back, has the scale of `1`.
fn decimal_places(number: &Number) -> usize {
    let text = number.to_string();
    if text.contains(['e', 'E']) {
        return usize::MAX;
    }
    text.split_once('.')
        .map_or(0, |(_, fraction)| fraction.trim_end_matches('0').len())
}

/// Normalises one raw value for a field. `None` means the value is empty and
/// the field should be cleared.
pub fn normalise(field: &FieldDefinition, value: &Value) -> Result<Option<Value>, ValidationIssue> {
    if field.field_type == FieldType::Formula {
        return Err(issue(
            field,
            "records.field_read_only",
            "this field is calculated from a formula",
        ));
    }
    if field.field_type == FieldType::Rollup {
        return Err(issue(
            field,
            "records.field_read_only",
            "this field is calculated from related records",
        ));
    }
    if value.is_null() {
        return Ok(None);
    }
    match field.field_type {
        FieldType::Text | FieldType::LongText => Ok(text_value(field, value)?.map(Value::String)),
        FieldType::Email => match text_value(field, value)? {
            Some(email) if valid_email(&email) => Ok(Some(Value::String(email))),
            Some(_) => Err(issue(
                field,
                "records.email_invalid",
                "enter an email address such as name@example.com",
            )),
            None => Ok(None),
        },
        FieldType::Phone => match text_value(field, value)? {
            Some(phone) if valid_phone(&phone) => Ok(Some(Value::String(phone))),
            Some(_) => Err(issue(
                field,
                "records.phone_invalid",
                "enter a phone number using digits, spaces and + ( ) - .",
            )),
            None => Ok(None),
        },
        FieldType::Url => match text_value(field, value)? {
            Some(url) if valid_url(&url) => Ok(Some(Value::String(url))),
            Some(_) => Err(issue(
                field,
                "records.url_invalid",
                "enter a web address starting with https:// or http://",
            )),
            None => Ok(None),
        },
        FieldType::Number | FieldType::Currency | FieldType::Percent => {
            let number = match value {
                Value::Number(number) => number.clone(),
                Value::String(text) if text.trim().is_empty() => return Ok(None),
                Value::String(text) => serde_json::from_str::<Number>(text.trim())
                    .map_err(|_| issue(field, "records.value_type", "enter a number"))?,
                _ => return Err(issue(field, "records.value_type", "enter a number")),
            };
            let as_float = number
                .as_f64()
                .filter(|value| value.is_finite() && value.abs() < 1e15)
                .ok_or_else(|| {
                    issue(
                        field,
                        "records.number_out_of_range",
                        "enter a number smaller than one quadrillion",
                    )
                })?;
            let scale = usize::try_from(field.scale.unwrap_or(0)).unwrap_or(0);
            if decimal_places(&number) > scale {
                return Err(issue(
                    field,
                    "records.number_scale",
                    format!("use at most {scale} decimal places"),
                ));
            }
            Ok(Some(Value::Number(
                Number::from_f64(as_float).unwrap_or(number),
            )))
        }
        FieldType::Boolean => value
            .as_bool()
            .map(|value| Some(Value::Bool(value)))
            .ok_or_else(|| issue(field, "records.value_type", "choose true or false")),
        FieldType::Date => {
            let text = value
                .as_str()
                .ok_or_else(|| issue(field, "records.value_type", "enter a date as YYYY-MM-DD"))?
                .trim();
            if text.is_empty() {
                return Ok(None);
            }
            NaiveDate::parse_from_str(text, "%Y-%m-%d")
                .map(|date| Some(Value::String(date.format("%Y-%m-%d").to_string())))
                .map_err(|_| issue(field, "records.date_invalid", "enter a date as YYYY-MM-DD"))
        }
        FieldType::DateTime => {
            let text = value
                .as_str()
                .ok_or_else(|| issue(field, "records.value_type", "enter a date and time"))?
                .trim();
            if text.is_empty() {
                return Ok(None);
            }
            DateTime::parse_from_rfc3339(text)
                .map(|time| {
                    Some(Value::String(
                        time.with_timezone(&Utc)
                            .to_rfc3339_opts(SecondsFormat::Micros, true),
                    ))
                })
                .map_err(|_| {
                    issue(
                        field,
                        "records.date_time_invalid",
                        "enter a date and time with a time zone, such as 2026-10-11T09:30:00Z",
                    )
                })
        }
        FieldType::Picklist => {
            let text = value
                .as_str()
                .ok_or_else(|| {
                    issue(
                        field,
                        "records.value_type",
                        "choose one of the field's values",
                    )
                })?
                .trim();
            if text.is_empty() {
                return Ok(None);
            }
            field
                .options
                .iter()
                .find(|option| option.active && option.value == text)
                .map(|option| Some(Value::String(option.value.clone())))
                .ok_or_else(|| {
                    issue(
                        field,
                        "records.picklist_value",
                        "choose one of the field's active values",
                    )
                })
        }
        FieldType::MultiPicklist => {
            let items = value.as_array().ok_or_else(|| {
                issue(
                    field,
                    "records.value_type",
                    "choose a list of the field's values",
                )
            })?;
            let mut chosen = BTreeSet::new();
            for item in items {
                let text = item.as_str().ok_or_else(|| {
                    issue(
                        field,
                        "records.value_type",
                        "choose a list of the field's values",
                    )
                })?;
                if !field
                    .options
                    .iter()
                    .any(|option| option.active && option.value == text)
                {
                    return Err(issue(
                        field,
                        "records.picklist_value",
                        "choose only the field's active values",
                    ));
                }
                chosen.insert(text.to_owned());
            }
            if chosen.is_empty() {
                return Ok(None);
            }
            // Keep the field's option order so equal choices store equally.
            let ordered: Vec<Value> = field
                .options
                .iter()
                .filter(|option| chosen.contains(&option.value))
                .map(|option| Value::String(option.value.clone()))
                .collect();
            Ok(Some(Value::Array(ordered)))
        }
        FieldType::TextList => text_list(field, value),
        FieldType::Lookup | FieldType::MasterDetail | FieldType::RelatedTo => {
            let text = value
                .as_str()
                .ok_or_else(|| issue(field, "records.value_type", "choose a record"))?
                .trim();
            if text.is_empty() {
                return Ok(None);
            }
            platform_id::PublicId::parse(text)
                .map(|_| Some(Value::String(text.to_owned())))
                .map_err(|_| {
                    issue(
                        field,
                        "records.reference_invalid",
                        "choose a record by its ID",
                    )
                })
        }
        // Refused above: formula and roll-up values are computed, never written.
        FieldType::Formula | FieldType::Rollup => Ok(None),
    }
}

/// A pending change to a record's values.
pub struct ValueChange {
    pub name: Option<String>,
    pub values: BTreeMap<String, Value>,
    /// API names whose value changed, for events and audit.
    pub changed: Vec<String>,
}

/// Applies a request body (`name` plus field API names) to existing values.
/// Unknown keys and invalid values are rejected together; required fields
/// must hold a value afterwards. On create, `existing` is empty and defaults
/// fill absent fields.
/// Checks that `name_from` names two or three distinct, active text fields
/// of `object`, with a separator of at most five printable characters
/// (ADR 0035 G17).
/// The longest processing purpose a field may declare (ADR 0035 G9).
pub const PURPOSE_LIMIT: usize = 4000;

/// Checks a field's governance declarations (ADR 0035 G9), with the rules
/// Contacts applies to its custom fields: a classified field that is not
/// public states its purpose, a sensitive one is declared encrypted, and a
/// retention period is a positive number of days.
pub fn check_governance(governance: &crate::FieldGovernance) -> Vec<ValidationIssue> {
    use crate::DataClassification;
    let mut issues = Vec::new();
    let purpose = governance
        .purpose
        .as_deref()
        .map(str::trim)
        .filter(|purpose| !purpose.is_empty());
    if governance.purpose.is_some() && purpose.is_none() {
        issues.push(ValidationIssue::new(
            "governance.purpose",
            "objects.purpose_invalid",
            "a purpose is not blank".to_owned(),
        ));
    }
    if purpose.is_some_and(|purpose| purpose.chars().count() > PURPOSE_LIMIT) {
        issues.push(ValidationIssue::new(
            "governance.purpose",
            "objects.purpose_invalid",
            format!("a purpose has at most {PURPOSE_LIMIT} characters"),
        ));
    }
    let classification = governance.data_classification;
    if classification.is_some_and(|class| class != DataClassification::Public) && purpose.is_none()
    {
        issues.push(ValidationIssue::new(
            "governance.purpose",
            "objects.purpose_required",
            "a field that is not public states the purpose of processing it".to_owned(),
        ));
    }
    if classification == Some(DataClassification::Sensitive) && !governance.encrypted {
        issues.push(ValidationIssue::new(
            "governance.encrypted",
            "objects.encryption_required",
            "a sensitive field is declared encrypted".to_owned(),
        ));
    }
    if governance.retention_days.is_some_and(|days| days < 1) {
        issues.push(ValidationIssue::new(
            "governance.retention_days",
            "objects.retention_invalid",
            "retention is a positive number of days".to_owned(),
        ));
    }
    issues
}

/// The declarations as stored: the purpose trimmed.
pub fn normalise_governance(mut governance: crate::FieldGovernance) -> crate::FieldGovernance {
    governance.purpose = governance.purpose.map(|purpose| purpose.trim().to_owned());
    governance
}

pub fn check_name_from(
    object: &ObjectDefinition,
    name_from: &crate::NameFrom,
) -> Result<(), ValidationIssue> {
    let invalid = |detail: &str| {
        ValidationIssue::new("name_from", "objects.name_from_invalid", detail.to_owned())
    };
    if !(2..=3).contains(&name_from.fields.len()) {
        return Err(invalid("name two or three fields"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for api_name in &name_from.fields {
        let text = object
            .field(api_name)
            .is_some_and(|field| field.is_active() && field.field_type == FieldType::Text);
        if !text {
            return Err(invalid("name text fields of this object"));
        }
        // The name is shown and searched wherever the record is, so it may
        // not carry a field some members cannot read (ADR 0035 G5).
        if object
            .field(api_name)
            .is_some_and(|field| field.read_permission.is_some())
        {
            return Err(invalid("name fields every reader of the record may read"));
        }
        if !seen.insert(api_name.as_str()) {
            return Err(invalid("name each field once"));
        }
    }
    if name_from.separator.chars().count() > 5 || name_from.separator.chars().any(char::is_control)
    {
        return Err(invalid("use a separator of at most 5 printable characters"));
    }
    Ok(())
}

pub fn apply_values(
    object: &ObjectDefinition,
    existing: &BTreeMap<String, Value>,
    body: &serde_json::Map<String, Value>,
    creating: bool,
) -> Result<ValueChange, Vec<ValidationIssue>> {
    let mut issues = Vec::new();
    // Values of fields out of service with their plugin (ADR 0035 G6) are
    // kept, unseen, for when it comes back; deleted fields' are dropped.
    let mut values: BTreeMap<String, Value> = existing
        .iter()
        .filter(|(key, _)| object.field(key).is_some() || object.suspended_fields.contains(*key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let mut changed = Vec::new();
    let mut name = None;
    for (key, raw) in body {
        if key == "name" && object.name_from.is_some() {
            issues.push(ValidationIssue::new(
                "name",
                "objects.name_composed",
                "the name is made from other fields; change those instead",
            ));
            continue;
        }
        if key == "name" {
            match record_name(raw) {
                Ok(value) => name = Some(value),
                Err(error) => issues.push(error),
            }
            continue;
        }
        let Some(field) = object.field(key) else {
            issues.push(ValidationIssue::new(
                key.clone(),
                "records.field_unknown",
                "this object has no such field",
            ));
            continue;
        };
        match normalise(field, raw) {
            Ok(Some(value)) => {
                if values.get(key) != Some(&value) {
                    changed.push(key.clone());
                }
                values.insert(key.clone(), value);
            }
            Ok(None) => {
                if values.remove(key).is_some() {
                    changed.push(key.clone());
                }
            }
            Err(error) => issues.push(error),
        }
    }
    if creating {
        if name.is_none() && object.name_from.is_none() {
            issues.push(ValidationIssue::new(
                "name",
                "records.name_required",
                "enter a name",
            ));
        }
        for field in &object.fields {
            if !field.is_active() || values.contains_key(&field.api_name) {
                continue;
            }
            let default = field.default_value.clone().or_else(|| {
                (field.field_type == FieldType::Picklist)
                    .then(|| field.options.iter().find(|option| option.default))
                    .flatten()
                    .map(|option| Value::String(option.value.clone()))
            });
            if let Some(default) = default {
                if let Ok(Some(value)) = normalise(field, &default) {
                    values.insert(field.api_name.clone(), value);
                    changed.push(field.api_name.clone());
                }
            }
        }
    }
    // A composed name follows its fields (ADR 0035 G17): written on create
    // and whenever one of them changes, and never left empty.
    if let Some(name_from) = &object.name_from {
        let touched = creating || name_from.fields.iter().any(|field| changed.contains(field));
        if touched {
            match name_from.compose(&values) {
                Some(composed) => name = Some(composed),
                None => issues.push(ValidationIssue::new(
                    name_from
                        .fields
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "name".into()),
                    "records.name_required",
                    "enter at least one of the fields the name is made from",
                )),
            }
        }
    }
    for field in &object.fields {
        if field.is_active() && field.required && !values.contains_key(&field.api_name) {
            issues.push(ValidationIssue::new(
                field.api_name.clone(),
                "records.value_required",
                format!("{} is required", field.label),
            ));
        }
    }
    if issues.is_empty() {
        changed.sort();
        changed.dedup();
        Ok(ValueChange {
            name,
            values,
            changed,
        })
    } else {
        Err(issues)
    }
}

fn number_key(value: f64) -> String {
    // Normalise -0.0 and integral floats so equal numbers share a key.
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{value}")
}

/// The most values a text list holds (ADR 0035 G7).
pub const TEXT_LIST_MAX_VALUES: usize = 50;
/// The longest value of a text list, in characters.
pub const TEXT_LIST_MAX_CHARS: usize = 80;

/// A text list's values (ADR 0035 G7): each trimmed, printable and at most
/// 80 characters; duplicates ignoring case keep their first spelling; at
/// most 50 remain. An empty list clears the field.
fn text_list(field: &FieldDefinition, value: &Value) -> Result<Option<Value>, ValidationIssue> {
    let list = || issue(field, "records.value_type", "enter a list of text values");
    let items = value.as_array().ok_or_else(list)?;
    let mut seen = BTreeSet::new();
    let mut kept = Vec::new();
    for item in items {
        let text = item.as_str().ok_or_else(list)?.trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().count() > TEXT_LIST_MAX_CHARS || text.chars().any(char::is_control) {
            return Err(issue(
                field,
                "records.text_list_value",
                format!("each value has 1 to {TEXT_LIST_MAX_CHARS} printable characters"),
            ));
        }
        if seen.insert(name_key(text)) {
            kept.push(Value::String(text.to_owned()));
        }
    }
    if kept.len() > TEXT_LIST_MAX_VALUES {
        return Err(issue(
            field,
            "records.text_list_too_long",
            format!("enter at most {TEXT_LIST_MAX_VALUES} distinct values"),
        ));
    }
    Ok((!kept.is_empty()).then_some(Value::Array(kept)))
}

/// The typed index value one stored value contributes, if the field is
/// indexed.
pub fn index_values(field: &FieldDefinition, value: &Value) -> Vec<IndexValue> {
    match (field.field_type, value) {
        (FieldType::MultiPicklist, Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|item| IndexValue::Text(item.to_owned()))
            .collect(),
        // Each value, without case (ADR 0035 G7).
        (FieldType::TextList, Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(|item| IndexValue::Text(name_key(item)))
            .collect(),
        (FieldType::Picklist, Value::String(text)) => vec![IndexValue::Text(text.clone())],
        (kind, Value::String(text)) if kind.is_textual() => vec![IndexValue::Text(name_key(text))],
        (kind, Value::Number(number)) if kind.is_numeric() => number
            .as_f64()
            .map(IndexValue::Number)
            .into_iter()
            .collect(),
        (FieldType::Boolean, Value::Bool(flag)) => vec![IndexValue::Text(flag.to_string())],
        (FieldType::Date, Value::String(text)) => NaiveDate::parse_from_str(text, "%Y-%m-%d")
            .ok()
            .and_then(|date| date.and_hms_opt(0, 0, 0))
            .map(|time| IndexValue::Time(time.and_utc()))
            .into_iter()
            .collect(),
        (FieldType::DateTime, Value::String(text)) => DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|time| IndexValue::Time(time.with_timezone(&Utc)))
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

/// The index value an equality filter on `field` compares with, from a
/// query-string value.
pub fn filter_value(field: &FieldDefinition, raw: &str) -> Result<IndexValue, ValidationIssue> {
    let value = match field.field_type {
        FieldType::Boolean => match raw {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => {
                return Err(issue(
                    field,
                    "records.filter_value",
                    "filter with true or false",
                ))
            }
        },
        kind if kind.is_numeric() => serde_json::from_str::<Number>(raw)
            .map(Value::Number)
            .map_err(|_| issue(field, "records.filter_value", "filter with a number"))?,
        FieldType::MultiPicklist | FieldType::TextList => {
            Value::Array(vec![Value::String(raw.to_owned())])
        }
        _ => Value::String(raw.to_owned()),
    };
    let normalised = normalise(field, &value)?
        .ok_or_else(|| issue(field, "records.filter_value", "filter with a value"))?;
    index_values(field, &normalised)
        .into_iter()
        .next()
        .ok_or_else(|| {
            issue(
                field,
                "records.filter_not_indexed",
                "this field cannot be filtered",
            )
        })
}

/// The pivot rows for a record. `references` maps lookup field ids to the
/// resolved target record ids.
pub fn record_index(
    object: &ObjectDefinition,
    name: &str,
    values: &BTreeMap<String, Value>,
    references: &BTreeMap<Uuid, Uuid>,
) -> RecordIndex {
    let mut index = RecordIndex {
        search: crate::search::search_terms(object, name, values),
        ..RecordIndex::default()
    };
    for field in object.fields.iter().filter(|field| field.is_active()) {
        let Some(value) = values.get(&field.api_name) else {
            continue;
        };
        if field.field_type.holds_reference() {
            if let Some(target_id) = references.get(&field.id) {
                index.references.push(ReferenceEntry {
                    field_id: field.id,
                    target_id: *target_id,
                });
            }
            continue;
        }
        if field.is_indexed() {
            // A roll-up summary is indexed by its result type.
            for value in index_values(&field.as_value_field(), value) {
                index.entries.push(IndexEntry {
                    field_id: field.id,
                    value,
                });
            }
        }
        if field.unique {
            let key = match index_values(field, value).into_iter().next() {
                Some(IndexValue::Text(text)) => Some(text),
                Some(IndexValue::Number(number)) => Some(number_key(number)),
                _ => None,
            };
            if let Some(value_key) = key {
                index.uniques.push(UniqueEntry {
                    field_id: field.id,
                    value_key,
                });
            }
        }
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefinitionStatus, Origin, SharingModel};
    use serde_json::json;

    #[test]
    fn trailing_zeros_are_not_decimal_places() {
        let places = |value: Value| match value {
            Value::Number(number) => decimal_places(&number),
            _ => usize::MAX,
        };
        assert_eq!(places(json!(1)), 0);
        assert_eq!(places(json!(1.0)), 0);
        assert_eq!(places(json!(2.5)), 1);
        assert_eq!(places(json!(2.50)), 1);
        assert_eq!(places(json!(0.125)), 3);
    }

    fn field(api: &str, kind: FieldType) -> FieldDefinition {
        FieldDefinition {
            id: Uuid::now_v7(),
            object_id: Uuid::nil(),
            api_name: api.into(),
            label: api.into(),
            description: None,
            field_type: kind,
            origin: Origin::Workspace,
            required: false,
            unique: false,
            indexed: false,
            searchable: false,
            track_history: false,
            max_length: kind.is_textual().then_some(255),
            scale: kind.is_numeric().then_some(2),
            options: if matches!(kind, FieldType::Picklist | FieldType::MultiPicklist) {
                vec![
                    PicklistOption {
                        outcome: Default::default(),
                        weight: None,
                        value: "Open".into(),
                        label: "Open".into(),
                        active: true,
                        default: true,
                    },
                    PicklistOption {
                        outcome: Default::default(),
                        weight: None,
                        value: "Won".into(),
                        label: "Won".into(),
                        active: true,
                        default: false,
                    },
                    PicklistOption {
                        outcome: Default::default(),
                        weight: None,
                        value: "Old".into(),
                        label: "Old".into(),
                        active: false,
                        default: false,
                    },
                ]
            } else {
                Vec::new()
            },
            default_value: None,
            reference_object_id: None,
            reference_object: None,
            reference_object_ids: Vec::new(),
            reference_objects: Vec::new(),
            delete_behaviour: None,
            formula: None,
            formula_type: None,
            rollup: None,
            lookup_filter: None,
            read_permission: None,
            write_permission: None,
            governance: Default::default(),
            position: 0,
            status: DefinitionStatus::Active,
            version: 1,
            created_at: Utc::now(),
            created_by: None,
            updated_at: Utc::now(),
            updated_by: None,
        }
    }

    fn object(fields: Vec<FieldDefinition>) -> ObjectDefinition {
        ObjectDefinition {
            path_field: None,
            name_from: None,
            id: Uuid::nil(),
            api_name: "custom:project".into(),
            label: "Project".into(),
            plural_label: "Projects".into(),
            description: None,
            origin: Origin::Workspace,
            owner_plugin: None,
            key_prefix: "000".into(),
            name_label: "Name".into(),
            sharing_model: SharingModel::ReadWrite,
            grant_hierarchy: true,
            track_history: false,
            searchable: true,
            in_navigation: true,
            status: DefinitionStatus::Active,
            suspended: false,
            dormant: false,
            resource: None,
            suspended_fields: Default::default(),
            version: 1,
            created_at: Utc::now(),
            created_by: None,
            updated_at: Utc::now(),
            updated_by: None,
            fields,
        }
    }

    /// Email fields may allow up to 320 characters (ADR 0035 G8); without a
    /// declared `max_length` they allow 255, as before.
    #[test]
    fn email_fields_allow_up_to_320_characters() {
        let shape = |max_length: Option<i32>| {
            let input: CreateField = serde_json::from_value(json!({
                "name": "email", "label": "Email", "field_type": "email",
                "max_length": max_length,
            }))
            .expect("input");
            field_shape_for(&input, false).map(|shape| shape.max_length)
        };
        assert_eq!(shape(None).expect("default"), None);
        assert_eq!(shape(Some(320)).expect("longest"), Some(320));
        for refused in [0, 321] {
            let issues = shape(Some(refused)).expect_err("out of range");
            assert_eq!(issues[0].code, "objects.max_length_invalid");
        }
        let input: CreateField = serde_json::from_value(json!({
            "name": "phone", "label": "Phone", "field_type": "phone", "max_length": 40,
        }))
        .expect("input");
        let Err(issues) = field_shape_for(&input, false) else {
            panic!("a phone field takes no max_length");
        };
        assert_eq!(issues[0].code, "objects.max_length_not_allowed");
        // A 300-character address: refused by default, accepted at 320.
        let label = "b".repeat(60);
        let long = format!(
            "{}@{label}.{label}.{label}.{}.example",
            "a".repeat(64),
            "c".repeat(44)
        );
        assert_eq!(long.chars().count(), 300);
        let mut email = field("custom:email", FieldType::Email);
        email.max_length = None;
        assert_eq!(
            normalise(&email, &json!(long)).expect_err("default").code,
            "records.value_too_long"
        );
        email.max_length = Some(EMAIL_LIMIT);
        assert_eq!(
            normalise(&email, &json!(long)).expect("320"),
            Some(json!(long))
        );
        // The parts keep RFC 5321's limits.
        let local = format!("{}@example.com", "a".repeat(65));
        assert_eq!(
            normalise(&email, &json!(local))
                .expect_err("local part")
                .code,
            "records.email_invalid"
        );
    }

    /// A composed name (ADR 0035 G17) joins the non-empty, trimmed parts in
    /// order and is cut to 255 characters.
    #[test]
    fn composed_names_join_their_parts() {
        let name_from = crate::NameFrom {
            fields: vec![
                "custom:first".into(),
                "custom:middle".into(),
                "custom:last".into(),
            ],
            separator: " ".into(),
        };
        let values = |pairs: &[(&str, Value)]| -> BTreeMap<String, Value> {
            pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect()
        };
        assert_eq!(
            name_from.compose(&values(&[
                ("custom:last", json!("Lovelace")),
                ("custom:first", json!(" Ada ")),
                ("custom:middle", json!("  ")),
            ])),
            Some("Ada Lovelace".into())
        );
        assert_eq!(name_from.compose(&values(&[])), None);
        assert_eq!(
            name_from.compose(&values(&[("custom:middle", json!(7))])),
            None,
            "only text counts"
        );
        let long = "x".repeat(200);
        let composed = name_from
            .compose(&values(&[
                ("custom:first", json!(long)),
                ("custom:last", json!(long)),
            ]))
            .expect("name");
        assert_eq!(composed.chars().count(), 255);
    }

    #[test]
    fn values_are_normalised_per_type() {
        let cases = [
            (FieldType::Text, json!("  hello "), Some(json!("hello"))),
            (FieldType::Text, json!("   "), None),
            (FieldType::Email, json!("a@b.co"), Some(json!("a@b.co"))),
            (
                FieldType::Url,
                json!("https://x.test/a"),
                Some(json!("https://x.test/a")),
            ),
            (FieldType::Number, json!("12.5"), Some(json!(12.5))),
            (FieldType::Boolean, json!(true), Some(json!(true))),
            (
                FieldType::Date,
                json!("2026-02-03"),
                Some(json!("2026-02-03")),
            ),
            (
                FieldType::DateTime,
                json!("2026-02-03T10:00:00+01:00"),
                Some(json!("2026-02-03T09:00:00.000000Z")),
            ),
            (FieldType::Picklist, json!("Won"), Some(json!("Won"))),
            (
                FieldType::MultiPicklist,
                json!(["Won", "Open", "Won"]),
                Some(json!(["Open", "Won"])),
            ),
            (FieldType::Lookup, json!("per000000000016a1b"), None),
        ];
        for (kind, raw, expected) in cases {
            let result = normalise(&field("custom:f", kind), &raw);
            match expected {
                Some(expected) => assert_eq!(result.expect("valid"), Some(expected), "{kind:?}"),
                None if kind == FieldType::Lookup => assert!(result.is_err(), "bad public id"),
                None => assert_eq!(result.expect("valid"), None, "{kind:?}"),
            }
        }
    }

    #[test]
    fn invalid_values_name_the_field_and_a_stable_code() {
        let cases = [
            (
                FieldType::Email,
                json!("not-an-email"),
                "records.email_invalid",
            ),
            (
                FieldType::Url,
                json!("javascript:alert(1)"),
                "records.url_invalid",
            ),
            (FieldType::Number, json!(1.234), "records.number_scale"),
            (FieldType::Number, json!(true), "records.value_type"),
            (FieldType::Date, json!("03/02/2026"), "records.date_invalid"),
            (FieldType::Picklist, json!("Old"), "records.picklist_value"),
            (FieldType::Phone, json!("call me"), "records.phone_invalid"),
        ];
        for (kind, raw, code) in cases {
            let error = normalise(&field("custom:f", kind), &raw).expect_err("invalid");
            assert_eq!(error.code, code, "{kind:?}");
            assert_eq!(error.field, "custom:f");
        }
    }

    #[test]
    fn create_applies_defaults_and_requires_name_and_required_fields() {
        let mut status = field("custom:status", FieldType::Picklist);
        status.required = true;
        let mut budget = field("custom:budget", FieldType::Currency);
        budget.required = true;
        let object = object(vec![status, budget]);
        let body = json!({"custom:nope": 1}).as_object().cloned().unwrap();
        let issues = apply_values(&object, &BTreeMap::new(), &body, true)
            .err()
            .unwrap();
        let codes: Vec<_> = issues.iter().map(|issue| issue.code).collect();
        assert!(codes.contains(&"records.field_unknown"));
        assert!(codes.contains(&"records.name_required"));
        assert!(codes.contains(&"records.value_required"), "{codes:?}");

        let body = json!({"name": "Apollo", "custom:budget": 10})
            .as_object()
            .cloned()
            .unwrap();
        let change = apply_values(&object, &BTreeMap::new(), &body, true).expect("valid");
        assert_eq!(change.name.as_deref(), Some("Apollo"));
        assert_eq!(
            change.values["custom:status"],
            json!("Open"),
            "picklist default"
        );
        assert_eq!(change.changed, ["custom:budget", "custom:status"]);
    }

    #[test]
    fn index_rows_cover_indexed_unique_and_lookup_fields() {
        let mut code = field("custom:code", FieldType::Text);
        code.unique = true;
        let status = field("custom:status", FieldType::Picklist);
        let mut owner = field("custom:account", FieldType::Lookup);
        owner.reference_object_id = Some(Uuid::nil());
        let plain = field("custom:notes", FieldType::LongText);
        let target = Uuid::now_v7();
        let object = object(vec![code.clone(), status.clone(), owner.clone(), plain]);
        let values = BTreeMap::from([
            ("custom:code".to_owned(), json!("AB-1")),
            ("custom:status".to_owned(), json!("Won")),
            ("custom:account".to_owned(), json!("per000000000016a1b")),
            ("custom:notes".to_owned(), json!("not indexed")),
        ]);
        let index = record_index(
            &object,
            "Name",
            &values,
            &BTreeMap::from([(owner.id, target)]),
        );
        assert_eq!(
            index.uniques,
            vec![UniqueEntry {
                field_id: code.id,
                value_key: "ab-1".into()
            }]
        );
        assert_eq!(
            index.references,
            vec![ReferenceEntry {
                field_id: owner.id,
                target_id: target
            }]
        );
        assert_eq!(
            index.entries.len(),
            2,
            "unique text and picklist; notes are not indexed"
        );
    }
}
