//! Related lists and lookup filters (ADR 0028, #295). Both read records
//! through the query language as the caller, so sharing, object
//! permissions and the recycle bin apply exactly as they do to a query.

use crate::service::ObjectService;
use crate::{
    FieldDefinition, FieldType, ObjectAccess, ObjectDefinition, ObjectError, ObjectTx,
    QueryRequest, QueryResult, ValidationIssue,
};
use serde::Serialize;

/// The longest lookup filter a field keeps.
pub const LOOKUP_FILTER_MAX: usize = 1_000;
/// Related lists one record page shows.
pub const RELATED_LISTS_MAX: usize = 20;
/// Records each related list shows on the record page.
pub const RELATED_PREVIEW: u16 = 5;
/// Records one page of a related list or of lookup candidates holds.
pub const RELATED_PAGE: u16 = 50;
const SEARCH_MAX: usize = 80;

/// A lookup, master-detail or related-to field of another object that
/// points at an object: one related list on that object's records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Relationship {
    /// The object whose records point here.
    pub object: String,
    pub object_label: String,
    pub object_plural_label: String,
    /// The reference field on that object.
    pub field: String,
    pub field_label: String,
    pub master_detail: bool,
    /// The reference field's type: lookup, master-detail or related to.
    pub field_type: FieldType,
}

/// A record another record points to, as the caller may see it: its name,
/// and its object's API name so a client can link to it. A related-to
/// field's target may be of any of several objects (ADR 0033 §1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RelatedTarget {
    pub name: String,
    pub object: String,
}

/// The first records of one related list.
#[derive(Debug, Clone, PartialEq)]
pub struct RelatedList {
    pub relationship: Relationship,
    pub records: QueryResult,
}

/// Quotes text as a query-language string literal.
pub(crate) fn literal(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('\'');
    for character in text.chars() {
        if matches!(character, '\'' | '\\') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push('\'');
    quoted
}

/// Quotes text as a LIKE pattern literal that matches it anywhere: the
/// query language's `\%` and `\_` escapes keep its wildcards literal.
fn contains_pattern(text: &str) -> String {
    let mut quoted = String::from("'%");
    for character in text.chars() {
        if matches!(character, '\'' | '\\' | '%' | '_') {
            quoted.push('\\');
        }
        quoted.push(character);
    }
    quoted.push_str("%'");
    quoted
}

/// Trims a lookup filter and checks its shape: only reference fields take
/// one, and an empty filter is none.
pub(crate) fn lookup_filter(
    field_type: FieldType,
    filter: Option<&str>,
) -> Result<Option<String>, ValidationIssue> {
    let Some(filter) = filter.map(str::trim).filter(|filter| !filter.is_empty()) else {
        return Ok(None);
    };
    if field_type == FieldType::RelatedTo {
        // Its target may be any of several objects, and a filter is one
        // object's condition; per-object filters are a follow-up.
        return Err(ValidationIssue::new(
            "lookup_filter",
            "objects.lookup_filter_not_supported",
            "related-to fields do not take a lookup filter yet",
        ));
    }
    if !field_type.is_reference() {
        return Err(ValidationIssue::new(
            "lookup_filter",
            "objects.lookup_filter_invalid",
            "only lookup and master-detail fields take a lookup filter",
        ));
    }
    if filter.chars().count() > LOOKUP_FILTER_MAX {
        return Err(ValidationIssue::new(
            "lookup_filter",
            "objects.lookup_filter_invalid",
            format!("use at most {LOOKUP_FILTER_MAX} characters"),
        ));
    }
    Ok(Some(filter.to_owned()))
}

impl ObjectService {
    /// Checks that a lookup filter is one condition on the target object
    /// that runs for the caller, leaving room for the conditions the engine
    /// adds to it. Its problems are the query language's, addressed to
    /// `lookup_filter`.
    pub(crate) async fn check_lookup_filter(
        &self,
        tx: &mut dyn ObjectTx,
        target: &ObjectDefinition,
        filter: &str,
    ) -> Result<(), ObjectError> {
        if !crate::query::is_condition(filter) {
            return Err(ObjectError::invalid(
                "lookup_filter",
                "objects.lookup_filter_invalid",
                "write one condition, as after WHERE, without ORDER BY, GROUP BY or LIMIT",
            ));
        }
        let access = self.access(tx).await?;
        let request = QueryRequest {
            // The shape a filter is enforced in, with the record and the
            // picker's search, so it fits the query's limits when used.
            q: format!(
                "SELECT id FROM {} WHERE id = '0' AND name LIKE '%' AND ({filter})",
                target.api_name
            ),
            cursor: None,
            page_size: Some(1),
            include_deleted: false,
        };
        match self.query_in(tx, &access, &request).await {
            Ok(_) => Ok(()),
            Err(ObjectError::Validation(found)) => Err(ObjectError::Validation(
                found
                    .into_iter()
                    .map(|issue| ValidationIssue {
                        field: "lookup_filter".into(),
                        ..issue
                    })
                    .collect(),
            )),
            Err(other) => Err(other),
        }
    }

    /// Whether a target record the caller sees meets a field's lookup
    /// filter. A filter that no longer runs, after a field it names was
    /// deleted, refuses every new value until it is fixed.
    pub(crate) async fn meets_lookup_filter(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        field: &FieldDefinition,
        filter: &str,
        public_id: &str,
    ) -> Result<bool, ObjectError> {
        let target = field
            .reference_object
            .as_deref()
            .ok_or(ObjectError::Invariant)?;
        let request = QueryRequest {
            q: format!(
                "SELECT id FROM {target} WHERE id = {} AND ({filter})",
                literal(public_id)
            ),
            cursor: None,
            page_size: Some(1),
            include_deleted: false,
        };
        match self.query_in(tx, access, &request).await {
            Ok(result) => Ok(!result.rows.is_empty()),
            Err(ObjectError::Validation(_)) => Err(ObjectError::invalid(
                field.api_name.clone(),
                "records.lookup_filter_invalid",
                "this field's lookup filter no longer runs; ask an administrator to fix it",
            )),
            Err(other) => Err(other),
        }
    }

    /// The records a lookup field may point at, for its picker: the target
    /// records the caller sees that meet the field's filter, by name,
    /// optionally those whose name contains `search`.
    pub async fn lookup_candidates(
        &self,
        api_name: &str,
        field_api_name: &str,
        search: Option<&str>,
        cursor: Option<String>,
    ) -> Result<QueryResult, ObjectError> {
        self.reference_candidates(api_name, field_api_name, None, search, cursor)
            .await
    }

    /// The records a reference field may point at, for its picker. A
    /// related-to field names the object to list with `target_object`,
    /// one its allowlist names; a lookup takes none, or its own target.
    pub async fn reference_candidates(
        &self,
        api_name: &str,
        field_api_name: &str,
        target_object: Option<&str>,
        search: Option<&str>,
        cursor: Option<String>,
    ) -> Result<QueryResult, ObjectError> {
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        if !access.for_object(object.id).read {
            return Err(ObjectError::NotFound);
        }
        let field = object
            .field(field_api_name)
            .filter(|field| field.is_active() && field.field_type.holds_reference())
            .ok_or(ObjectError::NotFound)?;
        let not_allowed = || {
            ObjectError::invalid(
                "object",
                "records.reference_object_not_allowed",
                "choose one of the objects this field allows",
            )
        };
        let target = match (field.field_type, target_object) {
            (FieldType::RelatedTo, Some(target)) => field
                .reference_objects
                .iter()
                .find(|allowed| allowed.as_str() == target)
                .map(String::as_str)
                .ok_or_else(not_allowed)?,
            (FieldType::RelatedTo, None) => return Err(not_allowed()),
            (_, target) => {
                let own = field
                    .reference_object
                    .as_deref()
                    .ok_or(ObjectError::Invariant)?;
                if target.is_some_and(|target| target != own) {
                    return Err(not_allowed());
                }
                own
            }
        };
        let mut conditions = Vec::new();
        if let Some(filter) = &field.lookup_filter {
            conditions.push(format!("({filter})"));
        }
        if let Some(search) = search.map(str::trim).filter(|search| !search.is_empty()) {
            if search.chars().count() > SEARCH_MAX {
                return Err(ObjectError::invalid(
                    "search",
                    "records.search_invalid",
                    format!("search with at most {SEARCH_MAX} characters"),
                ));
            }
            conditions.push(format!("name LIKE {}", contains_pattern(search)));
        }
        let filter = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };
        let request = QueryRequest {
            q: format!("SELECT id, name FROM {target}{filter} ORDER BY name"),
            cursor,
            page_size: Some(RELATED_PAGE),
            include_deleted: false,
        };
        let result = match self.query_in(tx.as_mut(), &access, &request).await {
            Err(ObjectError::Validation(issues))
                if issues.iter().any(|issue| issue.field == "q") =>
            {
                // A target the caller cannot read, or a filter that no
                // longer runs, offers no records.
                return Err(ObjectError::invalid(
                    field.api_name.clone(),
                    "records.lookup_filter_invalid",
                    "this field's records cannot be listed; ask an administrator to check its lookup filter",
                ));
            }
            other => other?,
        };
        tx.commit().await?;
        Ok(result)
    }

    /// The lookup, master-detail and related-to fields of objects the
    /// caller reads that point at an object, by the pointing object's label.
    async fn relationships(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        object: &ObjectDefinition,
    ) -> Result<Vec<Relationship>, ObjectError> {
        let mut found = Vec::new();
        for field in tx.fields_referencing_object(object.id).await? {
            // A related-to field whose allowlist names the object gives
            // a list too (ADR 0033 §1); the store found only those.
            if !field.is_active() || !field.field_type.holds_reference() {
                continue;
            }
            let Some(child) = tx
                .object_by_id(field.object_id)
                .await?
                .filter(|child| child.is_active() && access.for_object(child.id).read)
            else {
                continue;
            };
            found.push(Relationship {
                object: child.api_name,
                object_label: child.label,
                object_plural_label: child.plural_label,
                field: field.api_name,
                field_label: field.label,
                master_detail: field.field_type == FieldType::MasterDetail,
                field_type: field.field_type,
            });
        }
        found.sort_by(|a, b| {
            (
                a.object_plural_label.to_lowercase(),
                a.field_label.to_lowercase(),
            )
                .cmp(&(
                    b.object_plural_label.to_lowercase(),
                    b.field_label.to_lowercase(),
                ))
        });
        found.truncate(RELATED_LISTS_MAX);
        Ok(found)
    }

    async fn related_page(
        &self,
        tx: &mut dyn ObjectTx,
        access: &ObjectAccess,
        relationship: &Relationship,
        public_id: &str,
        cursor: Option<String>,
        page_size: u16,
    ) -> Result<QueryResult, ObjectError> {
        let request = QueryRequest {
            q: format!(
                "SELECT id, name FROM {} WHERE {} = {} ORDER BY name",
                relationship.object,
                relationship.field,
                literal(public_id)
            ),
            cursor,
            page_size: Some(page_size),
            include_deleted: false,
        };
        self.query_in(tx, access, &request).await
    }

    /// The related lists of a record the caller sees: for each lookup or
    /// master-detail field pointing at its object, and each related-to field
    /// allowing it, the first records that point at it and that the caller
    /// may see.
    pub async fn related_lists(
        &self,
        api_name: &str,
        public_id: &str,
    ) -> Result<Vec<RelatedList>, ObjectError> {
        let record = self.get_record(api_name, public_id, false).await?;
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let mut lists = Vec::new();
        for relationship in self.relationships(tx.as_mut(), &access, &object).await? {
            let records = self
                .related_page(
                    tx.as_mut(),
                    &access,
                    &relationship,
                    &record.public_id,
                    None,
                    RELATED_PREVIEW,
                )
                .await?;
            lists.push(RelatedList {
                relationship,
                records,
            });
        }
        tx.commit().await?;
        Ok(lists)
    }

    /// One page of one related list of a record the caller sees.
    pub async fn related_records(
        &self,
        api_name: &str,
        public_id: &str,
        child_object: &str,
        field: &str,
        cursor: Option<String>,
    ) -> Result<RelatedList, ObjectError> {
        let record = self.get_record(api_name, public_id, false).await?;
        let mut tx = self.begin().await?;
        let access = self.access(tx.as_mut()).await?;
        let object = self.active_object(tx.as_mut(), api_name).await?;
        let relationship = self
            .relationships(tx.as_mut(), &access, &object)
            .await?
            .into_iter()
            .find(|found| found.object == child_object && found.field == field)
            .ok_or(ObjectError::NotFound)?;
        let records = self
            .related_page(
                tx.as_mut(),
                &access,
                &relationship,
                &record.public_id,
                cursor,
                RELATED_PAGE,
            )
            .await?;
        tx.commit().await?;
        Ok(RelatedList {
            relationship,
            records,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_literals_and_patterns() {
        assert_eq!(literal("it's \\ ok"), "'it\\'s \\\\ ok'");
        assert_eq!(contains_pattern("50%_o'ff"), "'%50\\%\\_o\\'ff%'");
    }

    #[test]
    fn null_clears_and_absent_keeps() {
        let cleared: crate::UpdateField =
            serde_json::from_str(r#"{"expected_version": 1, "lookup_filter": null}"#)
                .expect("cleared");
        assert_eq!(cleared.lookup_filter, Some(None));
        let kept: crate::UpdateField =
            serde_json::from_str(r#"{"expected_version": 1}"#).expect("kept");
        assert_eq!(kept.lookup_filter, None);
    }

    #[test]
    fn only_reference_fields_take_a_filter() {
        assert_eq!(
            lookup_filter(FieldType::Lookup, Some("  custom:active = true ")),
            Ok(Some("custom:active = true".into()))
        );
        assert_eq!(lookup_filter(FieldType::Lookup, Some("  ")), Ok(None));
        assert!(lookup_filter(FieldType::Text, Some("x = 1")).is_err());
        assert!(lookup_filter(FieldType::Lookup, Some(&"a".repeat(1_001))).is_err());
    }
}
