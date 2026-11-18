//! The `assistant.v1` contract file ([`platform_device::assistant::SCHEMA`])
//! and a small validator for the part of JSON Schema it uses: `type`,
//! `const`, `enum`, `properties`, `required`, `additionalProperties`,
//! `items`, `minItems`, `maxItems`, `minLength`, `maxLength`, `oneOf`,
//! `format` (`uuid` and `date-time`) and local `$ref`s. The harness checks
//! every request it sends and every answer and index update it receives
//! against it, so the file cannot drift from what the bridge speaks.

use serde_json::{Map, Value};

/// The parsed contract file.
#[derive(Debug, Clone)]
pub struct Contract {
    root: Value,
}

impl Contract {
    /// The contract the bridge ships.
    pub fn bundled() -> anyhow::Result<Self> {
        Self::parse(platform_device::assistant::SCHEMA)
    }

    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let root: Value = serde_json::from_str(text)?;
        anyhow::ensure!(root.get("$defs").is_some_and(Value::is_object), "no $defs");
        Ok(Self { root })
    }

    /// The definition named `name` under `$defs`.
    pub fn definition(&self, name: &str) -> Option<&Value> {
        self.root.get("$defs")?.get(name)
    }

    /// Whether `value` is a valid `$defs/{name}`; the error says where it
    /// is not.
    pub fn check(&self, name: &str, value: &Value) -> Result<(), String> {
        let schema = self
            .definition(name)
            .ok_or_else(|| format!("no definition {name}"))?;
        self.validate(schema, value, "")
    }

    fn resolve<'a>(&'a self, reference: &str) -> Result<&'a Value, String> {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| format!("only local references: {reference}"))?;
        self.root
            .pointer(pointer)
            .ok_or_else(|| format!("unresolved reference {reference}"))
    }

    fn validate(&self, schema: &Value, value: &Value, at: &str) -> Result<(), String> {
        let Some(schema) = schema.as_object() else {
            return Ok(());
        };
        if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
            self.validate(self.resolve(reference)?, value, at)?;
        }
        if let Some(expected) = schema.get("const") {
            if value != expected {
                return Err(format!("{at}: expected {expected}, got {value}"));
            }
        }
        if let Some(options) = schema.get("enum").and_then(Value::as_array) {
            if !options.contains(value) {
                return Err(format!("{at}: {value} is not one of {options:?}"));
            }
        }
        if let Some(types) = schema.get("type") {
            let allowed: Vec<&str> = match types {
                Value::String(one) => vec![one.as_str()],
                Value::Array(many) => many.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            if !allowed.iter().any(|kind| is_type(value, kind)) {
                return Err(format!("{at}: {value} is not {allowed:?}"));
            }
        }
        if let Some(format) = schema.get("format").and_then(Value::as_str) {
            if let Some(text) = value.as_str() {
                let ok = match format {
                    "uuid" => uuid::Uuid::parse_str(text).is_ok(),
                    "date-time" => chrono::DateTime::parse_from_rfc3339(text).is_ok(),
                    _ => true,
                };
                if !ok {
                    return Err(format!("{at}: {text} is not a {format}"));
                }
            }
        }
        if let Some(text) = value.as_str() {
            let length = text.chars().count();
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64) {
                if (length as u64) < min {
                    return Err(format!("{at}: shorter than {min}"));
                }
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64) {
                if (length as u64) > max {
                    return Err(format!("{at}: longer than {max}"));
                }
            }
        }
        if let Some(items) = value.as_array() {
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64) {
                if (items.len() as u64) < min {
                    return Err(format!("{at}: fewer than {min} items"));
                }
            }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64) {
                if (items.len() as u64) > max {
                    return Err(format!("{at}: more than {max} items"));
                }
            }
            if let Some(item) = schema.get("items") {
                for (index, element) in items.iter().enumerate() {
                    self.validate(item, element, &format!("{at}/{index}"))?;
                }
            }
        }
        if let Some(object) = value.as_object() {
            self.validate_object(schema, object, at)?;
        }
        if let Some(options) = schema.get("oneOf").and_then(Value::as_array) {
            let mut failures = Vec::new();
            let mut matched = 0;
            for option in options {
                match self.validate(option, value, at) {
                    Ok(()) => matched += 1,
                    Err(error) => failures.push(error),
                }
            }
            if matched != 1 {
                return Err(if matched == 0 {
                    format!("{at}: matches no option: {}", failures.join("; "))
                } else {
                    format!("{at}: matches {matched} options")
                });
            }
        }
        Ok(())
    }

    fn validate_object(
        &self,
        schema: &Map<String, Value>,
        object: &Map<String, Value>,
        at: &str,
    ) -> Result<(), String> {
        let properties = schema.get("properties").and_then(Value::as_object);
        for required in schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !object.contains_key(required) {
                return Err(format!("{at}: missing {required}"));
            }
        }
        for (key, element) in object {
            let path = format!("{at}/{key}");
            match properties.and_then(|properties| properties.get(key)) {
                Some(property) => self.validate(property, element, &path)?,
                None => match schema.get("additionalProperties") {
                    Some(Value::Bool(false)) => return Err(format!("{path}: not allowed")),
                    Some(other @ Value::Object(_)) => self.validate(other, element, &path)?,
                    _ => {}
                },
            }
        }
        Ok(())
    }
}

fn is_type(value: &Value, kind: &str) -> bool {
    match kind {
        "null" => value.is_null(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        _ => false,
    }
}

/// The `const` values a definition's `oneOf` lists at `property` (or the
/// values of its `enum`), such as every operation name.
pub fn listed(contract: &Contract, definition: &str, property: Option<&str>) -> Vec<String> {
    let Some(schema) = contract.definition(definition) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut take = |value: &Value| {
        if let Some(text) = value.get("const").and_then(Value::as_str) {
            out.push(text.to_owned());
        }
        for text in value
            .get("enum")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            out.push(text.to_owned());
        }
    };
    match property {
        None => {
            take(schema);
            for option in schema
                .get("oneOf")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                take(option);
            }
        }
        Some(property) => {
            for option in schema
                .get("oneOf")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let option = match option.get("$ref").and_then(Value::as_str) {
                    Some(reference) => contract.resolve(reference).unwrap_or(option),
                    None => option,
                };
                if let Some(value) = option.pointer(&format!("/properties/{property}")) {
                    take(value);
                }
            }
        }
    }
    out
}
