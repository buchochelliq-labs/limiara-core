//! Global search (ADR 0020, #305).
//!
//! Every record keeps the terms of its name and searchable fields in
//! `record_search_terms`, written with its pivots in the same transaction.
//! A search matches every query term as a prefix, scores matches by where
//! they were found, and applies each object's sharing in the SQL.

use crate::query::plan::Scope;
use crate::service::ObjectService;
use crate::{
    sharing, FieldType, ObjectDefinition, ObjectError, ObjectTx, Record, RecordStatus,
    ValidationIssue,
};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// Terms are cut to this many characters.
pub const TERM_LENGTH: usize = 40;
/// A record keeps at most this many distinct terms, name terms first.
pub const TERMS_PER_RECORD: usize = 500;
/// A query uses at most this many terms.
pub const QUERY_TERMS: usize = 8;
/// A query needs at least this many letters or digits in total.
pub const QUERY_MIN_CHARS: usize = 2;
pub const QUERY_MAX_LENGTH: usize = 200;
pub const PER_OBJECT_DEFAULT: u16 = 5;
pub const PER_OBJECT_MAX: u16 = 50;
/// The per-object limit when searching one object for a results page.
pub const ONE_OBJECT_MAX: u16 = 200;
/// Fields shown with each result.
const RESULT_FIELDS: usize = 3;

pub const NAME_WEIGHT: i16 = 3;
pub const FIELD_WEIGHT: i16 = 1;

/// One indexed term of a record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchTerm {
    pub term: String,
    pub weight: i16,
}

/// Lowercase terms of a value: runs of letters and digits.
pub fn tokens(value: &str) -> Vec<String> {
    value
        .to_lowercase()
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(|token| token.chars().take(TERM_LENGTH).collect())
        .collect()
}

/// The terms a record is found by: its name, then each searchable field.
pub fn search_terms(
    object: &ObjectDefinition,
    name: &str,
    values: &BTreeMap<String, Value>,
) -> Vec<SearchTerm> {
    let mut weights: BTreeMap<String, i16> = BTreeMap::new();
    let mut order = Vec::new();
    let mut add = |term: String, weight: i16, order: &mut Vec<String>| match weights.get_mut(&term)
    {
        Some(existing) => *existing = (*existing).max(weight),
        None if order.len() < TERMS_PER_RECORD => {
            weights.insert(term.clone(), weight);
            order.push(term);
        }
        None => {}
    };
    for term in tokens(name) {
        add(term, NAME_WEIGHT, &mut order);
    }
    for field in object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.searchable && field.field_type.is_textual())
    {
        let Some(text) = values.get(&field.api_name).and_then(Value::as_str) else {
            continue;
        };
        for term in tokens(text) {
            add(term, FIELD_WEIGHT, &mut order);
        }
        if field.field_type == FieldType::Phone {
            let digits: String = text.chars().filter(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                add(
                    digits.chars().take(TERM_LENGTH).collect(),
                    FIELD_WEIGHT,
                    &mut order,
                );
            }
        }
    }
    order
        .into_iter()
        .map(|term| SearchTerm {
            weight: weights[&term],
            term,
        })
        .collect()
}

/// A search across objects, resolved by the service.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchPlan {
    pub terms: Vec<String>,
    /// Each searchable, readable object with its visibility.
    pub scopes: Vec<Scope>,
    /// Records returned per object; the store returns one more where more
    /// exist.
    pub per_object: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchRequest {
    pub q: String,
    /// Search one object only.
    pub object: Option<String>,
    /// Records per object.
    pub limit: Option<u16>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub public_id: String,
    pub name: String,
    /// Searchable field values with content, in field order.
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchGroup {
    pub object: String,
    pub label: String,
    pub plural_label: String,
    pub records: Vec<SearchHit>,
    pub has_more: bool,
}

fn invalid(field: &str, code: &'static str, detail: impl Into<String>) -> ObjectError {
    ObjectError::Validation(vec![ValidationIssue::new(field, code, detail)])
}

/// The distinct query terms, longest first within the limit.
pub fn query_terms(q: &str) -> Result<Vec<String>, ObjectError> {
    if q.chars().count() > QUERY_MAX_LENGTH {
        return Err(invalid(
            "q",
            "search.too_long",
            format!("search with at most {QUERY_MAX_LENGTH} characters"),
        ));
    }
    let mut seen = BTreeSet::new();
    let terms: Vec<String> = tokens(q)
        .into_iter()
        .filter(|term| seen.insert(term.clone()))
        .take(QUERY_TERMS)
        .collect();
    if terms.iter().map(|term| term.chars().count()).sum::<usize>() < QUERY_MIN_CHARS {
        return Err(invalid(
            "q",
            "search.too_short",
            format!("search with at least {QUERY_MIN_CHARS} letters or digits"),
        ));
    }
    Ok(terms)
}

fn hit(object: &ObjectDefinition, record: Record) -> SearchHit {
    let fields = object
        .fields
        .iter()
        .filter(|field| field.is_active() && field.searchable)
        .filter_map(|field| {
            record
                .values
                .get(&field.api_name)
                .filter(|value| value.as_str().is_some_and(|text| !text.is_empty()))
                .map(|value| (field.api_name.clone(), value.clone()))
        })
        .take(RESULT_FIELDS)
        .collect();
    SearchHit {
        public_id: record.public_id,
        name: record.name,
        fields,
    }
}

impl ObjectService {
    /// Searches every searchable object the caller can read (ADR 0020).
    pub async fn search(&self, request: SearchRequest) -> Result<Vec<SearchGroup>, ObjectError> {
        let terms = query_terms(&request.q)?;
        let max = if request.object.is_some() {
            ONE_OBJECT_MAX
        } else {
            PER_OBJECT_MAX
        };
        let limit = request.limit.unwrap_or(PER_OBJECT_DEFAULT);
        if !(1..=max).contains(&limit) {
            return Err(invalid(
                "limit",
                "search.limit_invalid",
                format!("use 1 to {max}"),
            ));
        }
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let summaries = tx.list_objects().await?;
        let mut objects = BTreeMap::new();
        for summary in summaries {
            if !summary.is_active() || !summary.searchable {
                continue;
            }
            if request
                .object
                .as_deref()
                .is_some_and(|api_name| api_name != summary.api_name)
            {
                continue;
            }
            let flags = access.for_object(summary.id);
            if !flags.read {
                continue;
            }
            let Some(object) = tx.object_by_id(summary.id).await? else {
                continue;
            };
            let visibility = self.visibility_of(tx.as_mut(), &access, &object).await?;
            objects.insert(object.id, (object, visibility));
        }
        if let Some(api_name) = &request.object {
            if objects.is_empty() {
                return Err(invalid(
                    "object",
                    "search.object_unknown",
                    format!("there is no searchable object {api_name} you can read"),
                ));
            }
        }
        let groups = if objects.is_empty() {
            Vec::new()
        } else {
            let plan = SearchPlan {
                terms,
                scopes: objects
                    .values()
                    .map(|(object, visibility)| Scope {
                        object_id: object.id,
                        status: RecordStatus::Active,
                        visibility: visibility.clone(),
                        predicate: None,
                    })
                    .collect(),
                per_object: u32::from(limit),
            };
            group(tx.as_mut(), &objects, &plan).await?
        };
        tx.commit().await?;
        Ok(groups)
    }
}

/// Runs the plan and groups hits by object, best group first. Hits arrive
/// in rank order.
async fn group(
    tx: &mut dyn ObjectTx,
    objects: &BTreeMap<Uuid, (ObjectDefinition, Option<sharing::Visibility>)>,
    plan: &SearchPlan,
) -> Result<Vec<SearchGroup>, ObjectError> {
    let hits = tx.search(plan).await?;
    let mut order: Vec<Uuid> = Vec::new();
    let mut grouped: BTreeMap<Uuid, Vec<Record>> = BTreeMap::new();
    for (record, _score) in hits {
        if !grouped.contains_key(&record.object_id) {
            order.push(record.object_id);
        }
        grouped.entry(record.object_id).or_default().push(record);
    }
    let mut groups = Vec::new();
    for object_id in order {
        let (Some((object, _)), Some(mut records)) =
            (objects.get(&object_id), grouped.remove(&object_id))
        else {
            continue;
        };
        let has_more = records.len() > plan.per_object as usize;
        records.truncate(plan.per_object as usize);
        groups.push(SearchGroup {
            object: object.api_name.clone(),
            label: object.label.clone(),
            plural_label: object.plural_label.clone(),
            records: records
                .into_iter()
                .map(|record| hit(object, record))
                .collect(),
            has_more,
        });
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefinitionStatus, FieldDefinition, Origin, SharingModel};
    use chrono::Utc;
    use serde_json::json;

    fn field(api: &str, kind: FieldType, searchable: bool) -> FieldDefinition {
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
            searchable,
            track_history: false,
            max_length: None,
            scale: None,
            options: Vec::new(),
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
            id: Uuid::nil(),
            api_name: "custom:account".into(),
            label: "Account".into(),
            plural_label: "Accounts".into(),
            description: None,
            origin: Origin::Workspace,
            owner_plugin: None,
            key_prefix: "0a1".into(),
            name_label: "Name".into(),
            sharing_model: SharingModel::ReadWrite,
            grant_hierarchy: true,
            track_history: false,
            searchable: true,
            in_navigation: true,
            status: DefinitionStatus::Active,
            suspended: false,
            version: 1,
            created_at: Utc::now(),
            created_by: None,
            updated_at: Utc::now(),
            updated_by: None,
            fields,
        }
    }

    #[test]
    fn tokens_split_on_anything_but_letters_and_digits() {
        assert_eq!(
            tokens("  Ångström-Labs, Inc. #42 "),
            ["ångström", "labs", "inc", "42"]
        );
        assert_eq!(tokens("sales@acme.example"), ["sales", "acme", "example"]);
        assert_eq!(tokens(&"x".repeat(60))[0].chars().count(), TERM_LENGTH);
        assert!(tokens(" -- ").is_empty());
    }

    #[test]
    fn terms_come_from_the_name_and_searchable_fields_only() {
        let account = object(vec![
            field("custom:email", FieldType::Email, true),
            field("custom:phone", FieldType::Phone, true),
            field("custom:secret", FieldType::Text, false),
            field("custom:acme", FieldType::Text, true),
        ]);
        let values = BTreeMap::from([
            ("custom:email".to_owned(), json!("Sales@Acme.example")),
            ("custom:phone".to_owned(), json!("+44 7946 0958")),
            ("custom:secret".to_owned(), json!("hidden")),
            ("custom:acme".to_owned(), json!("acme")),
        ]);
        let terms = search_terms(&account, "Acme Industries", &values);
        let pairs: Vec<(&str, i16)> = terms
            .iter()
            .map(|term| (term.term.as_str(), term.weight))
            .collect();
        assert_eq!(
            pairs,
            [
                ("acme", NAME_WEIGHT),
                ("industries", NAME_WEIGHT),
                ("sales", FIELD_WEIGHT),
                ("example", FIELD_WEIGHT),
                ("44", FIELD_WEIGHT),
                ("7946", FIELD_WEIGHT),
                ("0958", FIELD_WEIGHT),
                ("4479460958", FIELD_WEIGHT),
            ],
            "the name's weight wins for terms found in both"
        );
    }

    #[test]
    fn records_keep_a_bounded_number_of_terms() {
        let account = object(vec![field("custom:notes", FieldType::LongText, true)]);
        let notes = (0..2000)
            .map(|index| format!("w{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let values = BTreeMap::from([("custom:notes".to_owned(), json!(notes))]);
        let terms = search_terms(&account, "Name", &values);
        assert_eq!(terms.len(), TERMS_PER_RECORD);
        assert_eq!(terms[0].term, "name");
    }

    #[test]
    fn queries_need_two_characters_and_keep_eight_distinct_terms() {
        assert_eq!(
            query_terms("Acme  acme ind").expect("terms"),
            ["acme", "ind"]
        );
        assert!(query_terms("a").is_err());
        assert!(query_terms("!!").is_err());
        assert_eq!(
            query_terms("a b c d e f g h i j").expect("terms").len(),
            QUERY_TERMS
        );
        assert!(query_terms(&"x".repeat(QUERY_MAX_LENGTH + 1)).is_err());
    }
}
