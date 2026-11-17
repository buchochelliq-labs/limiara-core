//! Field-level permissions on every backend (G5 of ADR 0035): a field's
//! read permission hides it from records, queries, search, history, the
//! change log, related lists, formulas and roll-ups; its write permission
//! refuses changes with `records.field_not_writable`; and a replica applies
//! both to the member exactly as the server does.

use super::{body, caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

const READ: &str = "hr.salary.read";
const WRITE: &str = "hr.salary.write";
const RATING_WRITE: &str = "hr.rating.write";

fn formula(name: &str, text: &str, kind: FieldType) -> CreateField {
    let mut definition = field(name, FieldType::Formula);
    definition.formula = Some(text.into());
    definition.formula_type = Some(kind);
    definition
}

fn change<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("change")
}

fn validation_codes(error: &ObjectError) -> Vec<(String, &'static str)> {
    match error {
        ObjectError::Validation(issues) => issues
            .iter()
            .map(|issue| (issue.field.clone(), issue.code))
            .collect(),
        other => panic!("expected a validation error, got {other:?}"),
    }
}

fn refused(error: &ObjectError) -> Vec<String> {
    match error {
        ObjectError::Refused(code, issues) => {
            assert_eq!(*code, FIELD_NOT_WRITABLE);
            assert!(issues.iter().all(|issue| issue.code == FIELD_NOT_WRITABLE));
            issues.iter().map(|issue| issue.field.clone()).collect()
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}

async fn query_error(service: &ObjectService, q: &str) -> &'static str {
    match service
        .query(QueryRequest {
            q: q.into(),
            ..QueryRequest::default()
        })
        .await
    {
        Err(ObjectError::Validation(issues)) => issues[0].code,
        other => panic!("{q}: expected a validation error, got {other:?}"),
    }
}

async fn rows(service: &ObjectService, q: &str) -> Vec<serde_json::Map<String, Value>> {
    service
        .query(QueryRequest {
            q: q.into(),
            ..QueryRequest::default()
        })
        .await
        .unwrap_or_else(|error| panic!("{q}: {error:?}"))
        .rows
}

fn keys(record: &Record) -> Vec<&str> {
    record.values.keys().map(String::as_str).collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    // A manager defines everything but holds no field permission.
    let manager = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let staff = Uuid::now_v7();
    let mut account = object("account", "Account");
    account.track_history = Some(true);
    manager
        .create_object(account, context())
        .await
        .expect("account");
    manager
        .create_object(object("deal", "Deal"), context())
        .await
        .expect("deal");

    // Setting permissions: ids are checked, and computed fields take a read
    // permission only.
    let mut bad = field("bad", FieldType::Text);
    bad.read_permission = Some("Not A Permission".into());
    let error = manager
        .create_field("custom:account", bad, context())
        .await
        .expect_err("bad id");
    assert_eq!(
        validation_codes(&error),
        [(
            "read_permission".to_owned(),
            "objects.field_permission_invalid"
        )]
    );
    let mut written = formula("written", "1", FieldType::Number);
    written.write_permission = Some(WRITE.into());
    let error = manager
        .create_field("custom:account", written, context())
        .await
        .expect_err("formula write permission");
    assert_eq!(
        validation_codes(&error),
        [(
            "write_permission".to_owned(),
            "objects.field_permission_invalid"
        )]
    );

    let mut region = field("region", FieldType::Text);
    region.searchable = true;
    region.indexed = true;
    region.track_history = true;
    let mut salary = field("salary", FieldType::Number);
    salary.indexed = true;
    salary.track_history = true;
    salary.read_permission = Some(READ.into());
    salary.write_permission = Some(WRITE.into());
    let mut secret = field("secret", FieldType::Text);
    secret.searchable = true;
    secret.read_permission = Some(READ.into());
    let mut rating = field("rating", FieldType::Text);
    rating.default_value = Some(json!("new"));
    rating.write_permission = Some(RATING_WRITE.into());
    let mut tagged = formula("tagged", "UPPER(name)", FieldType::Text);
    tagged.read_permission = Some(READ.into());
    for definition in [
        region,
        salary,
        secret,
        rating,
        formula("bonus", "custom:salary / 10", FieldType::Number),
        formula("shout", "UPPER(custom:region)", FieldType::Text),
        tagged,
        formula("retagged", "custom:tagged", FieldType::Text),
    ] {
        manager
            .create_field("custom:account", definition, context())
            .await
            .expect("account field");
    }
    let mut master = field("account", FieldType::MasterDetail);
    master.reference_object = Some("custom:account".into());
    let mut referrer = field("referrer", FieldType::Lookup);
    referrer.reference_object = Some("custom:account".into());
    referrer.read_permission = Some(READ.into());
    let mut amount = field("amount", FieldType::Currency);
    amount.scale = Some(2);
    amount.read_permission = Some(READ.into());
    for definition in [
        master,
        referrer,
        amount,
        formula(
            "account_salary",
            "custom:account.custom:salary",
            FieldType::Number,
        ),
        formula(
            "account_shout",
            "custom:account.custom:shout",
            FieldType::Text,
        ),
    ] {
        manager
            .create_field("custom:deal", definition, context())
            .await
            .expect("deal field");
    }
    let mut total = field("total", FieldType::Rollup);
    total.rollup = Some(CreateRollup {
        function: RollupFunction::Sum,
        object: "custom:deal".into(),
        relationship: "custom:account".into(),
        field: Some("custom:amount".into()),
    });
    let mut deals = field("deals", FieldType::Rollup);
    deals.rollup = Some(CreateRollup {
        function: RollupFunction::Count,
        object: "custom:deal".into(),
        relationship: "custom:account".into(),
        field: None,
    });
    for definition in [total, deals] {
        manager
            .create_field("custom:account", definition, context())
            .await
            .expect("roll-up");
    }
    for api_name in ["custom:account", "custom:deal"] {
        manager
            .set_object_permission(
                api_name,
                staff,
                // Modify all widens record access, never field access.
                AccessFlags {
                    create: true,
                    delete: true,
                    view_all: true,
                    modify_all: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("staff access");
    }
    let viewer = service(&store, caller(tenant, Uuid::now_v7(), &[], &[staff]));
    let reader = service(&store, caller(tenant, Uuid::now_v7(), &[READ], &[staff]));
    let writer = service(
        &store,
        caller(
            tenant,
            Uuid::now_v7(),
            &[READ, WRITE, RATING_WRITE],
            &[staff],
        ),
    );

    // --- Definitions ------------------------------------------------------
    let (described, _, access) = viewer
        .describe_with_field_access("custom:account")
        .await
        .expect("describe");
    let listed: Vec<&str> = described
        .fields
        .iter()
        .map(|field| field.api_name.as_str())
        .collect();
    assert_eq!(
        listed,
        [
            "custom:region",
            "custom:rating",
            "custom:shout",
            "custom:deals"
        ],
        "members see only the fields they can read"
    );
    assert_eq!(
        access["custom:rating"],
        FieldAccess {
            read: true,
            edit: false
        }
    );
    let (described, _, access) = manager
        .describe_with_field_access("custom:account")
        .await
        .expect("manager describe");
    assert_eq!(described.fields.len(), 10, "managers see every definition");
    for hidden in [
        "custom:salary",
        "custom:secret",
        "custom:bonus",
        "custom:tagged",
        "custom:retagged",
        "custom:total",
    ] {
        assert!(
            !access[hidden].read,
            "{hidden} is unreadable to the manager"
        );
    }
    let salary = described.field("custom:salary").expect("salary");
    assert_eq!(salary.read_permission.as_deref(), Some(READ));
    assert_eq!(salary.write_permission.as_deref(), Some(WRITE));

    // --- Writes -------------------------------------------------------------
    let acme = writer
        .create_record(
            "custom:account",
            &body(json!({"name": "Acme", "custom:region": "north",
                         "custom:salary": 100, "custom:secret": "zebra"})),
            context(),
        )
        .await
        .expect("writer creates");
    assert_eq!(acme.values["custom:rating"], json!("new"), "defaults apply");
    // A field the caller cannot read is refused as an unknown one, before
    // its value is looked at.
    let error = viewer
        .create_record(
            "custom:account",
            &body(json!({"name": "Hidden", "custom:salary": "not a number"})),
            context(),
        )
        .await
        .expect_err("unreadable");
    assert_eq!(
        validation_codes(&error),
        [("custom:salary".to_owned(), "records.field_unknown")]
    );
    let error = viewer
        .create_record(
            "custom:account",
            &body(json!({"name": "Rated", "custom:rating": "gold"})),
            context(),
        )
        .await
        .expect_err("unwritable on create");
    assert_eq!(refused(&error), ["custom:rating"]);
    let globex = viewer
        .create_record(
            "custom:account",
            &body(json!({"name": "Globex", "custom:region": "south"})),
            context(),
        )
        .await
        .expect("a create without the field keeps its default");
    assert_eq!(
        keys(&globex),
        [
            "custom:deals",
            "custom:rating",
            "custom:region",
            "custom:shout"
        ]
    );
    let error = reader
        .update_record(
            "custom:account",
            &acme.public_id,
            acme.version,
            &body(json!({"custom:salary": 200})),
            context(),
        )
        .await
        .expect_err("read but not write");
    assert_eq!(refused(&error), ["custom:salary"]);
    let unchanged = reader
        .update_record(
            "custom:account",
            &acme.public_id,
            acme.version,
            &body(json!({"custom:salary": 100, "custom:region": "north-east"})),
            context(),
        )
        .await
        .expect("an unchanged value is no change");
    let acme = writer
        .update_record(
            "custom:account",
            &acme.public_id,
            unchanged.version,
            &body(json!({"custom:salary": 150})),
            context(),
        )
        .await
        .expect("writer changes");
    assert_eq!(acme.values["custom:salary"].as_f64(), Some(150.0));
    assert_eq!(acme.values["custom:bonus"].as_f64(), Some(15.0));
    changes_hide_fields(&viewer, &reader, &acme.public_id).await;
    writer
        .create_record(
            "custom:deal",
            &body(json!({"name": "Big deal", "custom:account": acme.public_id,
                         "custom:referrer": acme.public_id, "custom:amount": 40})),
            context(),
        )
        .await
        .expect("deal");

    // --- Reads, formulas and roll-ups --------------------------------------
    let seen = viewer
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("viewer reads");
    assert_eq!(
        keys(&seen),
        [
            "custom:deals",
            "custom:rating",
            "custom:region",
            "custom:shout"
        ],
        "hidden fields, formulas over them and roll-ups of them are left out"
    );
    assert_eq!(seen.values["custom:shout"], json!("NORTH-EAST"));
    let read = reader
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("reader reads");
    for key in [
        "custom:salary",
        "custom:secret",
        "custom:bonus",
        "custom:tagged",
        "custom:retagged",
        "custom:total",
    ] {
        assert!(read.values.contains_key(key), "{key} reaches the reader");
    }
    assert_eq!(read.values["custom:total"].as_f64(), Some(40.0));
    let page = viewer
        .list_records("custom:deal", service::ListRecords::default())
        .await
        .expect("deals");
    assert_eq!(
        keys(&page.records[0]),
        ["custom:account", "custom:account_shout"],
        "a formula through a lookup to a hidden field is hidden"
    );
    let listed = viewer
        .list_records("custom:account", service::ListRecords::default())
        .await
        .expect("accounts");
    assert!(listed
        .records
        .iter()
        .all(|record| !record.values.contains_key("custom:salary")));
    let error = viewer
        .list_records(
            "custom:account",
            service::ListRecords {
                filters: vec![("custom:salary".into(), "150".into())],
                ..service::ListRecords::default()
            },
        )
        .await
        .expect_err("filter on a hidden field");
    assert_eq!(
        validation_codes(&error),
        [("custom:salary".to_owned(), "records.field_unknown")]
    );
    let filtered = reader
        .list_records(
            "custom:account",
            service::ListRecords {
                filters: vec![("custom:salary".into(), "150".into())],
                ..service::ListRecords::default()
            },
        )
        .await
        .expect("reader filters");
    assert_eq!(filtered.records.len(), 1);

    // --- Queries: a hidden field is an unknown one --------------------------
    for q in [
        "SELECT custom:salary FROM custom:account",
        "SELECT name FROM custom:account WHERE custom:salary > 1",
        "SELECT name FROM custom:account ORDER BY custom:salary",
        "SELECT custom:salary, COUNT(id) FROM custom:account GROUP BY custom:salary",
        "SELECT custom:bonus FROM custom:account",
        "SELECT custom:total FROM custom:account",
        "SELECT name, custom:account.custom:salary FROM custom:deal",
        "SELECT name FROM custom:deal WHERE custom:referrer = null",
    ] {
        assert_eq!(query_error(&viewer, q).await, "query.field_unknown", "{q}");
    }
    assert_eq!(
        rows(
            &viewer,
            "SELECT name, custom:account.custom:region FROM custom:deal"
        )
        .await[0]["custom:account.custom:region"],
        json!("north-east")
    );
    let found = rows(
        &reader,
        "SELECT name, custom:account.custom:salary FROM custom:deal \
         WHERE custom:account.custom:salary > 100",
    )
    .await;
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0]["custom:account.custom:salary"].as_f64(),
        Some(150.0)
    );

    // --- Search -------------------------------------------------------------
    let search = |q: &str| SearchRequest {
        q: q.into(),
        ..SearchRequest::default()
    };
    for member in [&viewer, &reader, &writer] {
        assert!(
            member
                .search(search("zebra"))
                .await
                .expect("search")
                .is_empty(),
            "a field with a read permission is never matched"
        );
    }
    let groups = viewer.search(search("acme")).await.expect("viewer search");
    assert_eq!(
        Value::Object(groups[0].records[0].fields.clone()),
        json!({"custom:region": "north-east"})
    );
    let groups = reader.search(search("acme")).await.expect("reader search");
    assert_eq!(
        Value::Object(groups[0].records[0].fields.clone()),
        json!({"custom:region": "north-east", "custom:secret": "zebra"})
    );

    // --- History and the change log ----------------------------------------
    let fields_of = |page: &HistoryPage| -> Vec<String> {
        page.entries
            .iter()
            .filter_map(|entry| entry.field.clone())
            .collect()
    };
    let (history, _) = viewer
        .record_history("custom:account", &acme.public_id, None, None)
        .await
        .expect("viewer history");
    assert_eq!(fields_of(&history), ["custom:region"]);
    let (history, _) = reader
        .record_history("custom:account", &acme.public_id, None, None)
        .await
        .expect("reader history");
    assert_eq!(fields_of(&history), ["custom:salary", "custom:region"]);
    // --- Related lists ------------------------------------------------------
    let lists = |lists: Vec<RelatedList>| -> Vec<String> {
        lists
            .into_iter()
            .map(|list| format!("{}/{}", list.relationship.object, list.relationship.field))
            .collect()
    };
    assert_eq!(
        lists(
            viewer
                .related_lists("custom:account", &acme.public_id)
                .await
                .expect("viewer lists")
        ),
        ["custom:deal/custom:account"]
    );
    assert_eq!(
        lists(
            reader
                .related_lists("custom:account", &acme.public_id)
                .await
                .expect("reader lists")
        ),
        ["custom:deal/custom:account", "custom:deal/custom:referrer"]
    );

    // --- Settings move with the metadata version ----------------------------
    let version = |page: ChangePage| page.metadata_version;
    let before = version(manager.list_changes(None, Some(1), None).await.expect("v"));
    let secret = manager
        .describe("custom:account")
        .await
        .expect("describe")
        .0
        .field("custom:secret")
        .cloned()
        .expect("secret");
    manager
        .update_field(
            "custom:account",
            "custom:secret",
            change(json!({"expected_version": secret.version, "read_permission": null})),
            context(),
        )
        .await
        .expect("clear the read permission");
    let after = version(manager.list_changes(None, Some(1), None).await.expect("v"));
    assert!(after > before, "the setting is metadata");
    let groups = viewer.search(search("zebra")).await.expect("reindexed");
    assert_eq!(
        groups.len(),
        1,
        "a field without a read permission is searched"
    );
    let seen = viewer
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("viewer reads");
    assert_eq!(seen.values["custom:secret"], json!("zebra"));

    quick_actions(&store, tenant).await;
}

/// Quick actions are offered only to members who can complete them: an
/// action that presets a field, or sets a reference field, the member may
/// not change is left out of the list, on the record and in the timeline.
/// Administrators who manage objects still list every action, and the save
/// stays refused for anyone who runs one regardless.
async fn quick_actions(store: &Arc<dyn ObjectStore>, tenant: Uuid) {
    let manager = service(
        store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let staff = Uuid::now_v7();
    for (name, label) in [("ticket", "Ticket"), ("line", "Line")] {
        manager
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    let mut grade = field("grade", FieldType::Text);
    grade.write_permission = Some(WRITE.into());
    let mut pay = field("pay", FieldType::Number);
    pay.read_permission = Some(READ.into());
    pay.write_permission = Some(WRITE.into());
    for definition in [field("note", FieldType::Text), grade, pay] {
        manager
            .create_field("custom:ticket", definition, context())
            .await
            .expect("ticket field");
    }
    let mut parent = field("ticket", FieldType::Lookup);
    parent.reference_object = Some("custom:ticket".into());
    parent.write_permission = Some(WRITE.into());
    let mut open_parent = field("about", FieldType::Lookup);
    open_parent.reference_object = Some("custom:ticket".into());
    for definition in [parent, open_parent] {
        manager
            .create_field("custom:line", definition, context())
            .await
            .expect("line field");
    }
    for api_name in ["custom:ticket", "custom:line"] {
        manager
            .set_object_permission(
                api_name,
                staff,
                AccessFlags {
                    create: true,
                    edit: true,
                    ..AccessFlags::default()
                },
                context(),
            )
            .await
            .expect("staff access");
    }
    for action in [
        json!({"name": "note", "label": "Note", "kind": "update", "fields": ["custom:note"]}),
        json!({"name": "grade", "label": "Grade", "kind": "update", "fields": ["custom:note"], "presets": {"custom:grade": "A"}}),
        json!({"name": "pay", "label": "Pay", "kind": "update", "fields": ["custom:note"], "presets": {"custom:pay": 1}}),
        // Asking for a read-only field is fine: the form shows it read-only.
        json!({"name": "review", "label": "Review", "kind": "update", "fields": ["custom:grade", "custom:note"]}),
        json!({"name": "add_line", "label": "Add line", "kind": "create", "target_object": "custom:line", "target_field": "custom:ticket", "fields": ["name"]}),
        json!({"name": "add_about", "label": "Add about", "kind": "create", "target_object": "custom:line", "target_field": "custom:about", "fields": ["name"]}),
    ] {
        manager
            .create_quick_action("custom:ticket", change(action), context())
            .await
            .expect("quick action");
    }
    let names = |actions: Vec<QuickAction>| -> Vec<String> {
        let mut names: Vec<String> = actions.into_iter().map(|action| action.name).collect();
        names.sort();
        names
    };
    let viewer = service(store, caller(tenant, Uuid::now_v7(), &[], &[staff]));
    let reader = service(store, caller(tenant, Uuid::now_v7(), &[READ], &[staff]));
    let writer = service(
        store,
        caller(tenant, Uuid::now_v7(), &[READ, WRITE], &[staff]),
    );
    let everything = ["add_about", "add_line", "grade", "note", "pay", "review"];
    assert_eq!(
        names(
            manager
                .quick_actions("custom:ticket")
                .await
                .expect("manager")
        ),
        everything,
        "administrators maintain every action"
    );
    assert_eq!(
        names(writer.quick_actions("custom:ticket").await.expect("writer")),
        everything
    );
    assert_eq!(
        names(viewer.quick_actions("custom:ticket").await.expect("viewer")),
        ["add_about", "note", "review"],
        "presets and references the viewer may not change hide their actions"
    );
    assert_eq!(
        names(reader.quick_actions("custom:ticket").await.expect("reader")),
        ["add_about", "note", "review"],
        "reading a preset field is not enough"
    );

    // The save an offered-but-stale action would make is still refused.
    let ticket = viewer
        .create_record("custom:ticket", &body(json!({"name": "T-1"})), context())
        .await
        .expect("ticket");
    let error = viewer
        .update_record(
            "custom:ticket",
            &ticket.public_id,
            ticket.version,
            &body(json!({"custom:grade": "A"})),
            context(),
        )
        .await
        .expect_err("grade is read-only");
    assert_eq!(refused(&error), ["custom:grade"]);
}

/// The change log names no field the reader cannot read; the entry for a
/// change to hidden fields only stays, naming nothing.
async fn changes_hide_fields(viewer: &ObjectService, reader: &ObjectService, public_id: &str) {
    let changed = |page: ChangePage| -> Vec<Vec<String>> {
        page.changes
            .into_iter()
            .filter(|(_, change)| change.public_id == public_id)
            .map(|(_, change)| change.fields)
            .collect()
    };
    let feed = changed(
        viewer
            .list_changes(None, Some(200), Some("custom:account"))
            .await
            .expect("viewer changes"),
    );
    assert!(
        feed.iter().flatten().all(|field| field != "custom:salary"
            && field != "custom:secret"
            && field != "custom:total"),
        "{feed:?}"
    );
    assert_eq!(
        feed.last().map(Vec::len),
        Some(0),
        "the change stays, naming no hidden field"
    );
    let feed = changed(
        reader
            .list_changes(None, Some(200), Some("custom:account"))
            .await
            .expect("reader changes"),
    );
    assert_eq!(feed.last(), Some(&vec!["custom:salary".to_owned()]));
}

/// A device's replica (SQLite only): definitions carry the permissions, and
/// the replica's engine hides and refuses fields for the member exactly as
/// the server does, even when a value reached it.
pub async fn replica(store: Arc<dyn ObjectStore>) {
    let server_tenant = Uuid::now_v7();
    let device_tenant = Uuid::now_v7();
    let admin = service(
        &store,
        caller(server_tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut salary = field("salary", FieldType::Number);
    salary.indexed = true;
    salary.read_permission = Some(READ.into());
    salary.write_permission = Some(WRITE.into());
    let mut rating = field("rating", FieldType::Text);
    rating.write_permission = Some(RATING_WRITE.into());
    for definition in [field("region", FieldType::Text), salary, rating] {
        admin
            .create_field("custom:account", definition, context())
            .await
            .expect("field");
    }
    let (described, flags) = admin.describe("custom:account").await.expect("describe");
    let remote_object = RemoteObject::from_definition(&described, flags, None);
    assert_eq!(
        remote_object.fields[1].read_permission.as_deref(),
        Some(READ)
    );

    let member = caller(
        device_tenant,
        Uuid::now_v7(),
        &[RECORDS_MODIFY_ALL_PERMISSION],
        &[],
    );
    let device = service(&store, member).with_replica().with_client_ids();
    device
        .apply_remote_definitions(&[remote_object])
        .await
        .expect("definitions");
    let local = service(
        &store,
        caller(device_tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    )
    .with_replica();
    let (stored, _) = local.describe("custom:account").await.expect("local");
    let stored_salary = stored.field("custom:salary").expect("salary");
    assert_eq!(stored_salary.read_permission.as_deref(), Some(READ));
    assert_eq!(stored_salary.write_permission.as_deref(), Some(WRITE));

    // A stale copy holding the value still never shows it.
    let key_prefix = described.key_prefix.clone();
    let public_id = platform_id::PublicId::new(&key_prefix, 7)
        .expect("id")
        .as_str()
        .to_owned();
    let at = chrono::Utc::now();
    let remote = RemoteRecord {
        public_id: public_id.clone(),
        name: "Acme".into(),
        owner_id: None,
        values: body(json!({"custom:region": "north", "custom:salary": 100})),
        version: 1,
        created_at: at,
        created_by: None,
        updated_at: at,
        updated_by: None,
        touched_at: at,
        deleted_at: None,
    };
    let applied = device
        .apply_remote("custom:account", &remote, None)
        .await
        .expect("apply");
    let Applied::Inserted(applied) = applied else {
        panic!("inserted");
    };
    assert_eq!(keys(&applied), ["custom:region"]);
    let read = device
        .get_record("custom:account", &public_id, false)
        .await
        .expect("read");
    assert_eq!(keys(&read), ["custom:region"]);
    assert_eq!(
        query_error(&device, "SELECT custom:salary FROM custom:account").await,
        "query.field_unknown"
    );
    let error = device
        .update_record(
            "custom:account",
            &public_id,
            1,
            &body(json!({"custom:salary": 5})),
            context(),
        )
        .await
        .expect_err("hidden");
    assert_eq!(
        validation_codes(&error),
        [("custom:salary".to_owned(), "records.field_unknown")]
    );
    let error = device
        .update_record(
            "custom:account",
            &public_id,
            1,
            &body(json!({"custom:rating": "gold"})),
            context(),
        )
        .await
        .expect_err("unwritable");
    assert_eq!(refused(&error), ["custom:rating"]);

    // The member's edits push only fields they may change.
    device
        .update_record(
            "custom:account",
            &public_id,
            1,
            &body(json!({"custom:region": "south"})),
            context(),
        )
        .await
        .expect("edit");
    let batch = device.push_queue().await.expect("queue");
    let pushed = serde_json::to_value(batch.request()).expect("request");
    let text = pushed.to_string();
    assert!(text.contains("custom:region"), "{text}");
    assert!(!text.contains("custom:salary"), "{text}");
    assert!(!text.contains("custom:rating"), "{text}");
}
