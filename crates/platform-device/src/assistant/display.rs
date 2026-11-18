//! Display values (ADR 0034 §3): every value the bridge returns is
//! formatted once, here, so iOS and Android say the same thing. The raw
//! value travels with it, typed.

use chrono::{DateTime, NaiveDate, Utc};
use platform_objects::FieldType;
use serde_json::Value;

use super::model::FieldMeta;

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

fn date_text(date: NaiveDate) -> String {
    use chrono::Datelike;
    let month = MONTHS
        .get(usize::try_from(date.month0()).unwrap_or(0))
        .copied()
        .unwrap_or_default();
    format!("{} {month} {}", date.day(), date.year())
}

fn number_text(value: &Value, scale: Option<u32>) -> String {
    let Some(number) = value.as_f64() else {
        return text_of(value);
    };
    match scale {
        Some(scale) => format!("{number:.prec$}", prec = scale as usize),
        None if value.is_i64() || value.is_u64() => value.to_string(),
        None => format!("{number}"),
    }
}

fn text_of(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(true) => "Yes".into(),
        Value::Bool(false) => "No".into(),
        other => other.to_string(),
    }
}

/// How a value of `field` reads. `related` names the records lookups
/// point at, as the record routes give them; a target the member cannot
/// read has no name and reads as nothing.
pub(crate) fn display(
    field: &FieldMeta,
    value: &Value,
    related: &serde_json::Map<String, Value>,
) -> String {
    if value.is_null() {
        return String::new();
    }
    let kind = match field.field_type {
        FieldType::Formula => field.formula_type.unwrap_or(FieldType::Text),
        other => other,
    };
    match kind {
        FieldType::Boolean => match value.as_bool() {
            Some(true) => "Yes".into(),
            Some(false) => "No".into(),
            None => text_of(value),
        },
        FieldType::Number | FieldType::Rollup => number_text(value, field.scale),
        FieldType::Currency => number_text(value, Some(field.scale.unwrap_or(2))),
        FieldType::Percent => format!("{}%", number_text(value, field.scale)),
        FieldType::Date => value
            .as_str()
            .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
            .map(date_text)
            .unwrap_or_else(|| text_of(value)),
        FieldType::DateTime => value
            .as_str()
            .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
            .map(|time| {
                let time = time.with_timezone(&Utc);
                format!(
                    "{}, {} UTC",
                    date_text(time.date_naive()),
                    time.format("%H:%M")
                )
            })
            .unwrap_or_else(|| text_of(value)),
        FieldType::Picklist => value
            .as_str()
            .map(|chosen| option_label(field, chosen))
            .unwrap_or_else(|| text_of(value)),
        FieldType::MultiPicklist => match value.as_array() {
            Some(values) => values
                .iter()
                .filter_map(Value::as_str)
                .map(|chosen| option_label(field, chosen))
                .collect::<Vec<_>>()
                .join(", "),
            None => text_of(value),
        },
        FieldType::Lookup | FieldType::MasterDetail | FieldType::RelatedTo => value
            .as_str()
            .and_then(|id| related.get(id))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_default(),
        _ => text_of(value),
    }
}

fn option_label(field: &FieldMeta, chosen: &str) -> String {
    field
        .options
        .iter()
        .find(|option| option.value == chosen)
        .map_or_else(|| chosen.to_owned(), |option| option.label.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::model::OptionMeta;
    use serde_json::json;

    fn field(field_type: FieldType) -> FieldMeta {
        FieldMeta {
            api_name: "custom:x".into(),
            label: "X".into(),
            field_type,
            required: false,
            indexed: false,
            read: true,
            edit: true,
            options: vec![OptionMeta {
                value: "won".into(),
                label: "Closed won".into(),
            }],
            reference_object: None,
            scale: None,
            formula_type: None,
        }
    }

    #[test]
    fn values_read_the_same_everywhere() {
        let none = serde_json::Map::new();
        assert_eq!(
            display(&field(FieldType::Boolean), &json!(true), &none),
            "Yes"
        );
        assert_eq!(
            display(&field(FieldType::Date), &json!("2026-11-18"), &none),
            "18 November 2026"
        );
        assert_eq!(
            display(
                &field(FieldType::DateTime),
                &json!("2026-11-18T09:05:00Z"),
                &none
            ),
            "18 November 2026, 09:05 UTC"
        );
        assert_eq!(
            display(&field(FieldType::Currency), &json!(12.5), &none),
            "12.50"
        );
        assert_eq!(display(&field(FieldType::Number), &json!(7), &none), "7");
        assert_eq!(
            display(&field(FieldType::Percent), &json!(40), &none),
            "40%"
        );
        assert_eq!(
            display(&field(FieldType::Picklist), &json!("won"), &none),
            "Closed won"
        );
        assert_eq!(
            display(
                &field(FieldType::MultiPicklist),
                &json!(["won", "lost"]),
                &none
            ),
            "Closed won, lost"
        );
        let related = json!({"a01-1": "Acme"});
        let related = related.as_object().expect("map");
        assert_eq!(
            display(&field(FieldType::Lookup), &json!("a01-1"), related),
            "Acme"
        );
        assert_eq!(
            display(&field(FieldType::Lookup), &json!("a01-2"), related),
            ""
        );
        assert_eq!(display(&field(FieldType::Text), &Value::Null, related), "");
    }
}
