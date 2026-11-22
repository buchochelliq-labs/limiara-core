//! Text list fields on every backend (ADR 0035 G7): free-form tags, up to
//! 50 distinct values of up to 80 characters, kept in the spelling first
//! given and compared without case, always indexed per value so queries
//! filter them with `INCLUDES` and `EXCLUDES`, and readable by formulas.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const PERSON: &str = "custom:person";
const TAGS: &str = "custom:tags";

fn issues(error: ObjectError) -> Vec<(String, &'static str)> {
    match error {
        ObjectError::Validation(issues) => issues
            .into_iter()
            .map(|issue| (issue.field, issue.code))
            .collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

async fn names(admin: &ObjectService, q: &str) -> Vec<String> {
    let mut found: Vec<String> = admin
        .query(QueryRequest {
            q: q.to_owned(),
            cursor: None,
            page_size: None,
            include_deleted: false,
            include_archived: false,
        })
        .await
        .unwrap_or_else(|error| panic!("{q}: {error:?}"))
        .rows
        .iter()
        .map(|row| row["name"].as_str().unwrap_or_default().to_owned())
        .collect();
    found.sort();
    found
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    admin
        .create_object(object("person", "Person"), context())
        .await
        .expect("object");

    // --- The field: always indexed, never unique, no options or length ----
    let created = admin
        .create_field(PERSON, field("tags", FieldType::TextList), context())
        .await
        .expect("a text list");
    assert_eq!(created.field_type, FieldType::TextList);
    assert!(created.is_indexed(), "indexed per value");
    for refused in [
        {
            let mut unique = field("codes", FieldType::TextList);
            unique.unique = true;
            unique
        },
        {
            let mut long = field("codes", FieldType::TextList);
            long.max_length = Some(20);
            long
        },
    ] {
        admin
            .create_field(PERSON, refused, context())
            .await
            .expect_err("a text list is neither unique nor sized");
    }

    // --- Values: trimmed, first spelling kept, duplicates ignoring case ----
    let ada = admin
        .create_record(
            PERSON,
            &body(json!({"name": "Ada", TAGS: [" VIP ", "Speaker", "vip", "", "speaker"]})),
            context(),
        )
        .await
        .expect("tags");
    assert_eq!(ada.values[TAGS], json!(["VIP", "Speaker"]));
    let grace = admin
        .create_record(
            PERSON,
            &body(json!({"name": "Grace", TAGS: ["Navy", "speaker"]})),
            context(),
        )
        .await
        .expect("tags");
    admin
        .create_record(PERSON, &body(json!({"name": "Linus"})), context())
        .await
        .expect("no tags");
    // An empty list clears the field.
    let cleared = admin
        .update_record(
            PERSON,
            &grace.public_id,
            grace.version,
            &body(json!({TAGS: []})),
            context(),
        )
        .await
        .expect("cleared");
    assert!(!cleared.values.contains_key(TAGS));
    let grace = admin
        .update_record(
            PERSON,
            &grace.public_id,
            cleared.version,
            &body(json!({TAGS: ["Navy", "speaker"]})),
            context(),
        )
        .await
        .expect("tags again");

    // --- Limits ------------------------------------------------------------
    let fifty_one: Vec<Value> = (0..51).map(|n| json!(format!("tag {n}"))).collect();
    let fifty_same: Vec<Value> = (0..60)
        .map(|n| json!(if n % 2 == 0 { "a" } else { "A" }))
        .collect();
    for (values, code) in [
        (json!(fifty_one), "records.text_list_too_long"),
        (json!(["x".repeat(81)]), "records.text_list_value"),
        (json!(["tab\there"]), "records.text_list_value"),
        (json!("VIP"), "records.value_type"),
        (json!([1, 2]), "records.value_type"),
    ] {
        let found = issues(
            admin
                .create_record(
                    PERSON,
                    &body(json!({"name": "Bad", TAGS: values})),
                    context(),
                )
                .await
                .expect_err("refused"),
        );
        assert_eq!(found, [(TAGS.to_owned(), code)], "{values}");
    }
    // Sixty values that are one ignoring case are one value.
    let many = admin
        .create_record(
            PERSON,
            &body(json!({"name": "Many", TAGS: fifty_same})),
            context(),
        )
        .await
        .expect("duplicates collapse");
    assert_eq!(many.values[TAGS], json!(["a"]));
    let exactly: Vec<Value> = (0..50).map(|n| json!(format!("tag {n}"))).collect();
    admin
        .create_record(
            PERSON,
            &body(json!({"name": "Fifty", TAGS: exactly})),
            context(),
        )
        .await
        .expect("fifty distinct values");

    // --- Queries: INCLUDES and EXCLUDES, without case ----------------------
    assert_eq!(
        names(
            &admin,
            "SELECT name FROM custom:person WHERE custom:tags INCLUDES ('speaker')"
        )
        .await,
        ["Ada", "Grace"]
    );
    assert_eq!(
        names(
            &admin,
            "SELECT name FROM custom:person WHERE custom:tags INCLUDES ('vip', 'NAVY')"
        )
        .await,
        ["Ada", "Grace"]
    );
    assert_eq!(
        names(
            &admin,
            "SELECT name FROM custom:person WHERE custom:tags EXCLUDES ('VIP')"
        )
        .await,
        ["Fifty", "Grace", "Linus", "Many"]
    );
    assert_eq!(
        names(
            &admin,
            "SELECT name FROM custom:person WHERE custom:tags = null"
        )
        .await,
        ["Linus"]
    );
    admin
        .query(QueryRequest {
            q: "SELECT name FROM custom:person WHERE custom:tags = 'VIP'".into(),
            cursor: None,
            page_size: None,
            include_deleted: false,
            include_archived: false,
        })
        .await
        .expect_err("a list is filtered with INCLUDES");

    // --- Formulas read them, and INCLUDES compares without case ------------
    let mut speaker = field("is_speaker", FieldType::Formula);
    speaker.formula = Some("INCLUDES(custom:tags, 'SPEAKER')".into());
    speaker.formula_type = Some(FieldType::Boolean);
    admin
        .create_field(PERSON, speaker, context())
        .await
        .expect("a formula over tags");
    for (record, expected) in [(&ada, true), (&grace, true), (&many, false)] {
        let read = admin
            .get_record(PERSON, &record.public_id, false)
            .await
            .expect("record");
        assert_eq!(
            read.values["custom:is_speaker"],
            json!(expected),
            "{}",
            read.name
        );
    }
}
