use crate::model::*;
use crate::ContactsError;
use chrono::{DateTime, NaiveDate};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub fn validate_custom_field_api_name(api_name: &str) -> Result<(), ContactsError> {
    let valid = api_name.ends_with("__c")
        && api_name.len() <= 63
        && api_name.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '_'
        })
        && api_name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_lowercase())
        && !api_name.starts_with('_')
        && !api_name.contains("___");
    valid.then_some(()).ok_or_else(|| {
        ContactsError::validation(
            "api_name",
            "custom_field.api_name_invalid",
            "custom field API names must be lowercase snake_case, at most 63 characters, and end with __c",
        )
    })
}

pub fn validate_person(input: &CreatePerson) -> Result<(), ContactsError> {
    validate_contact_fields(&input.full_name, &input.email, "full_name")
}

pub fn validate_person_update(input: &UpdatePerson) -> Result<(), ContactsError> {
    validate_expected_version(input.expected_version)?;
    validate_contact_fields(&input.full_name, &input.email, "full_name")
}

pub fn validate_organisation(input: &CreateOrganisation) -> Result<(), ContactsError> {
    validate_contact_fields(&input.name, &input.email, "name")
}

pub fn validate_organisation_update(input: &UpdateOrganisation) -> Result<(), ContactsError> {
    validate_expected_version(input.expected_version)?;
    validate_contact_fields(&input.name, &input.email, "name")
}

pub fn validate_expected_version(version: i64) -> Result<(), ContactsError> {
    (version > 0).then_some(()).ok_or_else(|| {
        ContactsError::validation(
            "expected_version",
            "record.version_invalid",
            "expected_version must be greater than zero",
        )
    })
}

fn validate_contact_fields(
    name: &str,
    email: &Option<String>,
    name_field: &str,
) -> Result<(), ContactsError> {
    if name.trim().is_empty() || name.chars().count() > 240 {
        return Err(ContactsError::validation(
            name_field,
            "record.name_invalid",
            format!("{name_field} must contain between 1 and 240 characters"),
        ));
    }
    if email
        .as_deref()
        .is_some_and(|value| !valid_email(value) || value.chars().count() > 320)
    {
        return Err(ContactsError::validation(
            "email",
            "record.email_invalid",
            "email must be a valid address",
        ));
    }
    Ok(())
}

fn valid_email(value: &str) -> bool {
    let trimmed = value.trim();
    let Some((local, domain)) = trimmed.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

pub fn validate_custom_field_definition(
    definition: &CreateCustomFieldDefinition,
) -> Result<(), ContactsError> {
    validate_definition_parts(
        &definition.api_name,
        &definition.label,
        definition.field_type,
        &definition.options,
        &definition.validation_schema,
        definition.data_classification,
        definition.purpose.as_deref(),
        definition.retention_days,
        definition.encrypted,
    )
}

pub fn validate_custom_field_definition_update(
    existing: &ContactCustomFieldDefinition,
    update: &UpdateCustomFieldDefinition,
) -> Result<(), ContactsError> {
    validate_expected_version(update.expected_version)?;
    validate_definition_parts(
        &existing.api_name,
        &update.label,
        existing.field_type,
        &update.options,
        &update.validation_schema,
        update.data_classification,
        update.purpose.as_deref(),
        update.retention_days,
        update.encrypted,
    )
}

#[allow(clippy::too_many_arguments)]
fn validate_definition_parts(
    api_name: &str,
    label: &str,
    field_type: ContactCustomFieldType,
    options: &[String],
    validation_schema: &Value,
    classification: DataClassification,
    purpose: Option<&str>,
    retention_days: Option<u32>,
    encrypted: bool,
) -> Result<(), ContactsError> {
    validate_custom_field_api_name(api_name)?;
    if label.trim().is_empty() || label.chars().count() > 120 {
        return Err(ContactsError::validation(
            "label",
            "custom_field.label_invalid",
            "label must contain between 1 and 120 characters",
        ));
    }
    let option_field = matches!(
        field_type,
        ContactCustomFieldType::Select | ContactCustomFieldType::MultiSelect
    );
    let unique_options = options.iter().collect::<BTreeSet<_>>().len() == options.len();
    if option_field
        && (options.is_empty()
            || !unique_options
            || options.iter().any(|value| value.trim().is_empty()))
    {
        return Err(ContactsError::validation(
            "options",
            "custom_field.options_invalid",
            "select fields require non-empty unique options",
        ));
    }
    if !option_field && !options.is_empty() {
        return Err(ContactsError::validation(
            "options",
            "custom_field.options_not_supported",
            "options are supported only for select and multi_select fields",
        ));
    }
    validate_schema(field_type, validation_schema)?;
    if !matches!(classification, DataClassification::Public)
        && purpose.is_none_or(|value| value.trim().is_empty())
    {
        return Err(ContactsError::validation(
            "purpose",
            "custom_field.purpose_required",
            "non-public custom fields require a processing purpose",
        ));
    }
    if matches!(classification, DataClassification::Sensitive) && !encrypted {
        return Err(ContactsError::validation(
            "encrypted",
            "custom_field.encryption_required",
            "sensitive custom fields must declare encryption",
        ));
    }
    if retention_days.is_some_and(|days| days == 0 || days > i32::MAX as u32) {
        return Err(ContactsError::validation(
            "retention_days",
            "custom_field.retention_invalid",
            "retention_days must be between 1 and 2147483647",
        ));
    }
    Ok(())
}

fn validate_schema(
    field_type: ContactCustomFieldType,
    schema: &Value,
) -> Result<(), ContactsError> {
    let Some(object) = schema.as_object() else {
        return Err(ContactsError::validation(
            "validation_schema",
            "custom_field.schema_invalid",
            "validation_schema must be an object",
        ));
    };
    let allowed: BTreeSet<&str> = match field_type {
        ContactCustomFieldType::Text
        | ContactCustomFieldType::Textarea
        | ContactCustomFieldType::Email
        | ContactCustomFieldType::Phone => ["min_length", "max_length"].into_iter().collect(),
        ContactCustomFieldType::Number => ["minimum", "maximum"].into_iter().collect(),
        _ => BTreeSet::new(),
    };
    if let Some(key) = object.keys().find(|key| !allowed.contains(key.as_str())) {
        return Err(ContactsError::validation(
            "validation_schema",
            "custom_field.schema_keyword_unsupported",
            format!("validation keyword {key} is not supported for this field type"),
        ));
    }
    for key in ["min_length", "max_length"] {
        if object
            .get(key)
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(ContactsError::validation(
                "validation_schema",
                "custom_field.schema_invalid",
                format!("{key} must be a non-negative integer"),
            ));
        }
    }
    for key in ["minimum", "maximum"] {
        if object
            .get(key)
            .is_some_and(|value| value.as_f64().is_none())
        {
            return Err(ContactsError::validation(
                "validation_schema",
                "custom_field.schema_invalid",
                format!("{key} must be numeric"),
            ));
        }
    }
    Ok(())
}

pub fn validate_custom_fields(
    values: &BTreeMap<String, Value>,
    definitions: &[ContactCustomFieldDefinition],
    existing: Option<&BTreeMap<String, Value>>,
) -> Result<BTreeMap<String, Value>, ContactsError> {
    let definitions_by_name: BTreeMap<_, _> = definitions
        .iter()
        .map(|definition| (definition.api_name.as_str(), definition))
        .collect();
    let mut merged = values.clone();
    for (key, value) in values {
        validate_custom_field_api_name(key)?;
        let Some(definition) = definitions_by_name.get(key.as_str()) else {
            return Err(ContactsError::validation(
                key,
                "custom_field.undefined",
                format!("custom field {key} is not defined for this workspace"),
            ));
        };
        if !definition.is_active() {
            let unchanged = existing.and_then(|stored| stored.get(key)) == Some(value);
            if !unchanged {
                return Err(ContactsError::validation(
                    key,
                    "custom_field.inactive",
                    format!("custom field {key} is disabled or archived and cannot be written"),
                ));
            }
        } else {
            validate_value(definition, value)?;
        }
    }
    if let Some(existing) = existing {
        for definition in definitions
            .iter()
            .filter(|definition| !definition.is_active())
        {
            if let Some(value) = existing.get(&definition.api_name) {
                merged
                    .entry(definition.api_name.clone())
                    .or_insert_with(|| value.clone());
            }
        }
    }
    for definition in definitions
        .iter()
        .filter(|definition| definition.is_active() && definition.required)
    {
        if merged.get(&definition.api_name).is_none_or(Value::is_null) {
            return Err(ContactsError::validation(
                &definition.api_name,
                "custom_field.required",
                format!("custom field {} is required", definition.api_name),
            ));
        }
    }
    Ok(merged)
}

fn validate_value(
    definition: &ContactCustomFieldDefinition,
    value: &Value,
) -> Result<(), ContactsError> {
    let valid_type = match definition.field_type {
        ContactCustomFieldType::Text
        | ContactCustomFieldType::Textarea
        | ContactCustomFieldType::Email
        | ContactCustomFieldType::Phone
        | ContactCustomFieldType::Select
        | ContactCustomFieldType::Date
        | ContactCustomFieldType::DateTime => value.is_string(),
        ContactCustomFieldType::Number => value.is_number(),
        ContactCustomFieldType::Boolean => value.is_boolean(),
        ContactCustomFieldType::MultiSelect => value.is_array(),
        ContactCustomFieldType::Json => !value.is_null(),
    };
    if !valid_type {
        return Err(value_error(
            definition,
            "custom_field.type_mismatch",
            "value has the wrong type",
        ));
    }
    match definition.field_type {
        ContactCustomFieldType::Email if !valid_email(value.as_str().unwrap_or_default()) => {
            return Err(value_error(
                definition,
                "custom_field.email_invalid",
                "value must be a valid email address",
            ));
        }
        ContactCustomFieldType::Date
            if NaiveDate::parse_from_str(value.as_str().unwrap_or_default(), "%Y-%m-%d")
                .is_err() =>
        {
            return Err(value_error(
                definition,
                "custom_field.date_invalid",
                "value must use YYYY-MM-DD",
            ));
        }
        ContactCustomFieldType::DateTime
            if DateTime::parse_from_rfc3339(value.as_str().unwrap_or_default()).is_err() =>
        {
            return Err(value_error(
                definition,
                "custom_field.datetime_invalid",
                "value must be RFC 3339",
            ));
        }
        ContactCustomFieldType::Select => {
            let selected = value.as_str().unwrap_or_default();
            if !definition.options.iter().any(|option| option == selected) {
                return Err(value_error(
                    definition,
                    "custom_field.option_invalid",
                    "value is not an allowed option",
                ));
            }
        }
        ContactCustomFieldType::MultiSelect => {
            let selected = value.as_array().expect("type checked");
            if selected.iter().any(|item| {
                item.as_str()
                    .is_none_or(|item| !definition.options.iter().any(|option| option == item))
            }) {
                return Err(value_error(
                    definition,
                    "custom_field.option_invalid",
                    "every value must be an allowed option",
                ));
            }
        }
        _ => {}
    }
    apply_schema(definition, value)
}

fn apply_schema(
    definition: &ContactCustomFieldDefinition,
    value: &Value,
) -> Result<(), ContactsError> {
    let schema = definition.validation_schema.as_object();
    if let (Some(text), Some(schema)) = (value.as_str(), schema) {
        let length = text.chars().count() as u64;
        if schema
            .get("min_length")
            .and_then(Value::as_u64)
            .is_some_and(|minimum| length < minimum)
            || schema
                .get("max_length")
                .and_then(Value::as_u64)
                .is_some_and(|maximum| length > maximum)
        {
            return Err(value_error(
                definition,
                "custom_field.length_invalid",
                "value length violates the field definition",
            ));
        }
    }
    if let (Some(number), Some(schema)) = (value.as_f64(), schema) {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|minimum| number < minimum)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|maximum| number > maximum)
        {
            return Err(value_error(
                definition,
                "custom_field.range_invalid",
                "value is outside the allowed range",
            ));
        }
    }
    Ok(())
}

fn value_error(
    definition: &ContactCustomFieldDefinition,
    code: &str,
    detail: &str,
) -> ContactsError {
    ContactsError::validation(&definition.api_name, code, detail)
}

pub fn validate_list_request(request: &ListRequest) -> Result<(), ContactsError> {
    if !(1..=200).contains(&request.limit) {
        return Err(ContactsError::validation(
            "limit",
            "query.limit_invalid",
            "limit must be between 1 and 200",
        ));
    }
    if request
        .search
        .as_deref()
        .is_some_and(|value| value.chars().count() > 200)
    {
        return Err(ContactsError::validation(
            "search",
            "query.search_too_long",
            "search cannot exceed 200 characters",
        ));
    }
    if request
        .email
        .as_deref()
        .is_some_and(|value| !valid_email(value))
    {
        return Err(ContactsError::validation(
            "email",
            "query.email_invalid",
            "email filter must be a valid address",
        ));
    }
    if request
        .cursor
        .as_deref()
        .is_some_and(|value| value.len() != 43)
    {
        return Err(ContactsError::InvalidCursor);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use serde_json::json;
    use uuid::Uuid;

    fn definition(field_type: ContactCustomFieldType) -> ContactCustomFieldDefinition {
        ContactCustomFieldDefinition {
            id: Uuid::now_v7(),
            tenant_id: Uuid::now_v7(),
            applies_to: ContactCustomFieldTarget::Person,
            api_name: "fixture__c".into(),
            label: "Fixture".into(),
            description: None,
            field_type,
            required: true,
            unique_per_tenant: false,
            indexed: false,
            options: vec![],
            validation_schema: json!({}),
            data_classification: DataClassification::Public,
            purpose: None,
            retention_days: None,
            encrypted: false,
            version: 1,
            disabled_at: None,
            archived_at: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn validates_every_supported_custom_field_type() {
        let cases = [
            (ContactCustomFieldType::Text, json!("text")),
            (ContactCustomFieldType::Textarea, json!("long text")),
            (ContactCustomFieldType::Email, json!("a@example.test")),
            (ContactCustomFieldType::Phone, json!("+44 20 1234")),
            (ContactCustomFieldType::Number, json!(42.5)),
            (ContactCustomFieldType::Boolean, json!(true)),
            (ContactCustomFieldType::Date, json!("2026-07-17")),
            (
                ContactCustomFieldType::DateTime,
                json!("2026-07-17T12:00:00Z"),
            ),
            (ContactCustomFieldType::Json, json!({"safe": true})),
        ];
        for (field_type, value) in cases {
            let definition = definition(field_type);
            let values = BTreeMap::from([("fixture__c".into(), value)]);
            assert!(validate_custom_fields(&values, &[definition], None).is_ok());
        }
        for field_type in [
            ContactCustomFieldType::Select,
            ContactCustomFieldType::MultiSelect,
        ] {
            let mut definition = definition(field_type);
            definition.options = vec!["one".into(), "two".into()];
            let value = if field_type == ContactCustomFieldType::Select {
                json!("one")
            } else {
                json!(["one", "two"])
            };
            let values = BTreeMap::from([("fixture__c".into(), value)]);
            assert!(validate_custom_fields(&values, &[definition], None).is_ok());
        }
    }

    #[test]
    fn rejects_type_option_required_schema_and_inactive_writes() {
        let mut definition = definition(ContactCustomFieldType::Number);
        definition.validation_schema = json!({"minimum": 10, "maximum": 20});
        let wrong = BTreeMap::from([("fixture__c".into(), json!("12"))]);
        assert!(matches!(
            validate_custom_fields(&wrong, &[definition.clone()], None),
            Err(ContactsError::Validation(_))
        ));
        assert!(matches!(
            validate_custom_fields(&BTreeMap::new(), &[definition.clone()], None),
            Err(ContactsError::Validation(_))
        ));
        let outside = BTreeMap::from([("fixture__c".into(), json!(30))]);
        assert!(matches!(
            validate_custom_fields(&outside, &[definition.clone()], None),
            Err(ContactsError::Validation(_))
        ));
        definition.disabled_at = Some(Utc::now());
        assert!(matches!(
            validate_custom_fields(&outside, &[definition], None),
            Err(ContactsError::Validation(_))
        ));
    }

    #[test]
    fn sensitive_definitions_require_purpose_and_encryption() {
        let input = CreateCustomFieldDefinition {
            applies_to: ContactCustomFieldTarget::Person,
            api_name: "medical_notes__c".into(),
            label: "Medical notes".into(),
            description: None,
            field_type: ContactCustomFieldType::Textarea,
            required: false,
            unique_per_tenant: false,
            indexed: false,
            options: vec![],
            validation_schema: json!({}),
            data_classification: DataClassification::Sensitive,
            purpose: None,
            retention_days: Some(365),
            encrypted: false,
        };
        assert!(matches!(
            validate_custom_field_definition(&input),
            Err(ContactsError::Validation(_))
        ));
    }
}
