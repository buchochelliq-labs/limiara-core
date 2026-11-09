//! The workspace API's JSON as engine types. The shapes are those of
//! `crates/platform-objects-http/src/handlers.rs` (`record_json`, `object_json`, `field_json`,
//! `rule_json`) and docs/api/objects.md.

use crate::transport::{ChangeEntry, ChangesPage};
use crate::SyncError;
use chrono::{DateTime, Utc};
use platform_objects::{
    AccessFlags, DeleteBehaviour, FieldType, Origin, PicklistOption, RemoteField, RemoteObject,
    RemoteRecord, RemoteRollup, RemoteRule, RollupFunction, SharingModel,
};
use serde::Deserialize;
use serde_json::{Map, Value};
use uuid::Uuid;

/// Keys of a record that are not field values: the standard fields of ADR
/// 0014, reference names and objects, and the caller's rights.
const RECORD_KEYS: [&str; 16] = [
    "id",
    "object",
    "name",
    "owner",
    "created_at",
    "created_by",
    "updated_at",
    "updated_by",
    "touched_at",
    "version",
    "archived_at",
    "is_deleted",
    "deleted_at",
    "related",
    "related_objects",
    "access",
];

/// A record as the record API returns it.
pub fn remote_record(value: &Value) -> Result<RemoteRecord, SyncError> {
    let map = value
        .as_object()
        .ok_or_else(|| SyncError::protocol("a record is an object"))?;
    let values: Map<String, Value> = map
        .iter()
        .filter(|(key, _)| !RECORD_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    Ok(RemoteRecord {
        public_id: text(map, "id")?,
        name: text(map, "name")?,
        owner_id: user(map, "owner")?,
        values,
        version: map
            .get("version")
            .and_then(Value::as_i64)
            .ok_or_else(|| SyncError::protocol("a record has a version"))?,
        created_at: time(map, "created_at")?,
        created_by: user(map, "created_by")?,
        updated_at: time(map, "updated_at")?,
        updated_by: user(map, "updated_by")?,
        touched_at: time(map, "touched_at")?,
        deleted_at: match map.get("deleted_at") {
            None | Some(Value::Null) => None,
            Some(_) => Some(time(map, "deleted_at")?),
        },
    })
}

fn text(map: &Map<String, Value>, key: &str) -> Result<String, SyncError> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| SyncError::protocol(format!("a record has `{key}`")))
}

fn time(map: &Map<String, Value>, key: &str) -> Result<DateTime<Utc>, SyncError> {
    map.get(key)
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
        .ok_or_else(|| SyncError::protocol(format!("`{key}` is an RFC 3339 time")))
}

/// A user reference, `{"id": …, "name": …}`, or null.
fn user(map: &Map<String, Value>, key: &str) -> Result<Option<Uuid>, SyncError> {
    match map.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(reference) => reference
            .get("id")
            .and_then(Value::as_str)
            .and_then(|id| Uuid::parse_str(id).ok())
            .map(Some)
            .ok_or_else(|| SyncError::protocol(format!("`{key}` is a user reference"))),
    }
}

#[derive(Deserialize)]
struct ObjectBody {
    api_name: String,
    label: String,
    plural_label: String,
    #[serde(default)]
    description: Option<String>,
    origin: Origin,
    #[serde(default)]
    owner_plugin: Option<String>,
    key_prefix: String,
    name_label: String,
    sharing_model: SharingModel,
    grant_hierarchy: bool,
    track_history: bool,
    searchable: bool,
    in_navigation: bool,
    version: i64,
    #[serde(default)]
    access: Option<AccessFlags>,
    #[serde(default)]
    fields: Vec<FieldBody>,
}

#[derive(Deserialize)]
struct FieldBody {
    api_name: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    field_type: FieldType,
    origin: Origin,
    required: bool,
    unique: bool,
    indexed: bool,
    searchable: bool,
    track_history: bool,
    #[serde(default)]
    max_length: Option<i32>,
    #[serde(default)]
    scale: Option<i32>,
    #[serde(default)]
    options: Vec<PicklistOption>,
    #[serde(default)]
    default_value: Option<Value>,
    #[serde(default)]
    reference_object: Option<String>,
    #[serde(default)]
    reference_objects: Option<Vec<String>>,
    #[serde(default)]
    delete_behaviour: Option<DeleteBehaviour>,
    #[serde(default)]
    lookup_filter: Option<String>,
    #[serde(default)]
    formula: Option<String>,
    #[serde(default)]
    formula_type: Option<FieldType>,
    #[serde(default)]
    rollup: Option<RollupBody>,
    position: i32,
    version: i64,
}

#[derive(Deserialize)]
struct RollupBody {
    function: RollupFunction,
    object: String,
    relationship: String,
    #[serde(default)]
    field: Option<String>,
}

#[derive(Deserialize)]
struct RuleBody {
    name: String,
    #[serde(default)]
    description: Option<String>,
    condition: String,
    message: String,
    #[serde(default)]
    error_field: Option<String>,
    active: bool,
    version: i64,
}

/// An object as `GET /objects/{api_name}` describes it, with its rules
/// when the server shows them.
pub fn remote_object(
    described: &Value,
    rules: Option<&[Value]>,
) -> Result<RemoteObject, SyncError> {
    let body: ObjectBody = serde_json::from_value(described.clone())
        .map_err(|error| SyncError::protocol(format!("an object description: {error}")))?;
    let rules = match rules {
        Some(rules) => Some(
            rules
                .iter()
                .map(|rule| {
                    serde_json::from_value::<RuleBody>(rule.clone())
                        .map(|rule| RemoteRule {
                            name: rule.name,
                            description: rule.description,
                            condition: rule.condition,
                            message: rule.message,
                            error_field: rule.error_field,
                            active: rule.active,
                            version: rule.version,
                        })
                        .map_err(|error| SyncError::protocol(format!("a validation rule: {error}")))
                })
                .collect::<Result<Vec<_>, _>>()?,
        ),
        None => None,
    };
    Ok(RemoteObject {
        api_name: body.api_name,
        label: body.label,
        plural_label: body.plural_label,
        description: body.description,
        origin: body.origin,
        owner_plugin: body.owner_plugin,
        key_prefix: body.key_prefix,
        name_label: body.name_label,
        sharing_model: body.sharing_model,
        grant_hierarchy: body.grant_hierarchy,
        track_history: body.track_history,
        searchable: body.searchable,
        in_navigation: body.in_navigation,
        version: body.version,
        access: body.access.unwrap_or_default(),
        fields: body
            .fields
            .into_iter()
            .map(|field| RemoteField {
                api_name: field.api_name,
                label: field.label,
                description: field.description,
                field_type: field.field_type,
                origin: field.origin,
                required: field.required,
                unique: field.unique,
                indexed: field.indexed,
                searchable: field.searchable,
                track_history: field.track_history,
                max_length: field.max_length,
                scale: field.scale,
                options: field.options,
                default_value: field.default_value,
                reference_object: field.reference_object,
                reference_objects: field.reference_objects.unwrap_or_default(),
                delete_behaviour: field.delete_behaviour,
                lookup_filter: field.lookup_filter,
                formula: field.formula,
                formula_type: field.formula_type,
                rollup: field.rollup.map(|rollup| RemoteRollup {
                    function: rollup.function,
                    object: rollup.object,
                    relationship: rollup.relationship,
                    field: rollup.field,
                }),
                position: field.position,
                version: field.version,
            })
            .collect(),
        rules,
    })
}

#[derive(Deserialize)]
struct ChangesBody {
    data: Vec<EntryBody>,
    next_after: i64,
    head: i64,
    metadata_version: i64,
}

#[derive(Deserialize)]
struct EntryBody {
    position: i64,
    object: String,
    record: String,
    kind: String,
}

/// One change entry, as a page or a stream event carries it.
pub fn change_entry(value: &Value) -> Result<ChangeEntry, SyncError> {
    let entry: EntryBody = serde_json::from_value(value.clone())
        .map_err(|error| SyncError::protocol(format!("a change: {error}")))?;
    Ok(ChangeEntry {
        position: entry.position,
        object: entry.object,
        record: entry.record,
        kind: entry.kind,
    })
}

/// A page of `GET /objects/changes`.
pub fn changes_page(value: &Value) -> Result<ChangesPage, SyncError> {
    let body: ChangesBody = serde_json::from_value(value.clone())
        .map_err(|error| SyncError::protocol(format!("a change page: {error}")))?;
    Ok(ChangesPage {
        entries: body
            .data
            .into_iter()
            .map(|entry| ChangeEntry {
                position: entry.position,
                object: entry.object,
                record: entry.record,
                kind: entry.kind,
            })
            .collect(),
        next_after: body.next_after,
        head: body.head,
        metadata_version: body.metadata_version,
    })
}

/// The stable `code` of a problem body, or the HTTP reason when it has none.
pub fn problem_code(body: &Value) -> String {
    body.get("code")
        .and_then(Value::as_str)
        .unwrap_or("platform.unexpected")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_split_standard_fields_from_values() {
        let record = remote_record(&json!({
            "id": "a0B000000000001AAA",
            "object": "custom:account",
            "name": "Acme",
            "owner": {"id": "0190a6f2-27f8-7c55-a2a9-8e6a1e0f4b10", "name": null},
            "created_at": "2026-11-01T10:00:00Z",
            "created_by": null,
            "updated_at": "2026-11-01T10:00:00Z",
            "updated_by": {"id": "0190a6f2-27f8-7c55-a2a9-8e6a1e0f4b10", "name": null},
            "touched_at": "2026-11-01T10:00:00Z",
            "version": 3,
            "archived_at": null,
            "is_deleted": true,
            "deleted_at": "2026-11-01T11:00:00Z",
            "custom:tier": "gold",
            "custom:parent": "a0B000000000002AAA",
            "related": {"a0B000000000002AAA": "Parent"},
            "access": {"read": true}
        }))
        .expect("record");
        assert_eq!(record.public_id, "a0B000000000001AAA");
        assert_eq!(record.version, 3);
        assert!(record.owner_id.is_some() && record.created_by.is_none());
        assert!(record.deleted_at.is_some());
        assert_eq!(
            record.values.keys().collect::<Vec<_>>(),
            vec!["custom:parent", "custom:tier"]
        );
    }

    #[test]
    fn malformed_records_are_protocol_errors() {
        assert!(matches!(
            remote_record(&json!({"id": "x", "name": "y"})),
            Err(SyncError::Protocol(_))
        ));
        assert!(matches!(
            remote_record(&json!([])),
            Err(SyncError::Protocol(_))
        ));
    }
}
