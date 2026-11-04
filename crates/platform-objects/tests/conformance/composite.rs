//! Composite requests on every backend (ADR 0023): references between
//! operations, independent operations, and all-or-none transactions.

use super::{caller, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::composite::{CompositeOutcome, CompositeRequest};
use platform_objects::service::ListRecords;
use platform_objects::*;
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

fn request(value: Value) -> CompositeRequest {
    serde_json::from_value(value).expect("request")
}

fn summary(outcomes: &[(String, CompositeOutcome)]) -> Vec<String> {
    outcomes
        .iter()
        .map(|(reference, outcome)| {
            let state = match outcome {
                CompositeOutcome::Done { created: true, .. } => "created".to_owned(),
                CompositeOutcome::Done { .. } => "changed".to_owned(),
                CompositeOutcome::Failed(ObjectError::Validation(issues)) => {
                    issues[0].code.to_owned()
                }
                CompositeOutcome::Failed(other) => format!("{other:?}"),
                CompositeOutcome::RolledBack => "rolled_back".to_owned(),
                CompositeOutcome::NotRun => "not_run".to_owned(),
            };
            format!("{reference}:{state}")
        })
        .collect()
}

async fn count(admin: &ObjectService, api_name: &str) -> usize {
    admin
        .list_records(api_name, ListRecords::default())
        .await
        .expect("list")
        .records
        .len()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    admin
        .create_object(object("company", "Company"), context())
        .await
        .expect("company");
    admin
        .create_object(object("person", "Person"), context())
        .await
        .expect("person");
    let mut employer = field("employer", FieldType::Lookup);
    employer.reference_object = Some("custom:company".into());
    let mut age = field("age", FieldType::Number);
    age.scale = Some(0);
    for definition in [employer, age] {
        admin
            .create_field("custom:person", definition, context())
            .await
            .expect("person field");
    }

    // Independent operations: later ones use earlier records, and a failure
    // does not stop the rest.
    let outcomes = admin
        .composite(
            &request(json!({"operations": [
                {"method": "create", "reference": "acme", "object": "custom:company",
                 "values": {"name": "Acme"}},
                {"method": "create", "reference": "ada", "object": "custom:person",
                 "values": {"name": "Ada", "custom:employer": "@{acme.id}"}},
                {"method": "create", "reference": "bad", "object": "custom:person",
                 "values": {"name": "Bad", "custom:age": "old"}},
                {"method": "create", "reference": "orphan", "object": "custom:person",
                 "values": {"name": "Orphan", "custom:employer": "@{bad.id}"}},
                {"method": "update", "reference": "ada_again", "object": "custom:person",
                 "id": "@{ada.id}", "expected_version": 1, "values": {"custom:age": 36}},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(
        summary(&outcomes),
        [
            "acme:created",
            "ada:created",
            "bad:records.value_type",
            "orphan:composite.reference_unknown",
            "ada_again:changed",
        ]
    );
    let (CompositeOutcome::Done { record: acme, .. }, CompositeOutcome::Done { record: ada, .. }) =
        (&outcomes[0].1, &outcomes[4].1)
    else {
        panic!("done");
    };
    assert_eq!(ada.values["custom:employer"], json!(acme.public_id));
    assert_eq!(ada.values["custom:age"], json!(36.0));
    assert_eq!(ada.version, 2);

    // All or none: one failure rolls every change back.
    let before = (
        count(&admin, "custom:company").await,
        count(&admin, "custom:person").await,
    );
    let outcomes = admin
        .composite(
            &request(json!({"all_or_none": true, "operations": [
                {"method": "create", "reference": "globex", "object": "custom:company",
                 "values": {"name": "Globex"}},
                {"method": "create", "reference": "hank", "object": "custom:person",
                 "values": {"name": "Hank", "custom:employer": "@{globex.id}"}},
                {"method": "create", "reference": "bad", "object": "custom:person",
                 "values": {"custom:age": 3}},
                {"method": "create", "reference": "later", "object": "custom:person",
                 "values": {"name": "Later"}},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(
        summary(&outcomes),
        [
            "globex:rolled_back",
            "hank:rolled_back",
            "bad:records.name_required",
            "later:not_run"
        ]
    );
    assert_eq!(
        (
            count(&admin, "custom:company").await,
            count(&admin, "custom:person").await
        ),
        before,
        "nothing was kept"
    );
    let outcomes = admin
        .composite(
            &request(json!({"all_or_none": true, "operations": [
                {"method": "create", "reference": "initech", "object": "custom:company",
                 "values": {"name": "Initech"}},
                {"method": "create", "reference": "peter", "object": "custom:person",
                 "values": {"name": "Peter", "custom:employer": "@{initech.id}"}},
                {"method": "delete", "reference": "gone", "object": "custom:person",
                 "id": "@{peter.id}", "expected_version": 1},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(
        summary(&outcomes),
        ["initech:created", "peter:created", "gone:changed"]
    );
    assert_eq!(count(&admin, "custom:company").await, before.0 + 1);

    // A record in the recycle bin comes back with `undelete`, on its own or
    // with other operations in one transaction.
    let peter = match &outcomes[1].1 {
        CompositeOutcome::Done { record, .. } => record.public_id.clone(),
        other => panic!("peter: {other:?}"),
    };
    let outcomes = admin
        .composite(
            &request(json!({"operations": [
                {"method": "undelete", "reference": "back", "object": "custom:person",
                 "id": peter, "expected_version": 2},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(summary(&outcomes), ["back:changed"]);
    let outcomes = admin
        .composite(
            &request(json!({"all_or_none": true, "operations": [
                {"method": "delete", "reference": "out", "object": "custom:person",
                 "id": peter, "expected_version": 3},
                {"method": "undelete", "reference": "in", "object": "custom:person",
                 "id": peter, "expected_version": 4},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(summary(&outcomes), ["out:changed", "in:changed"]);
    let restored = admin
        .get_record("custom:person", &peter, false)
        .await
        .expect("restored");
    assert!(restored.deleted_at.is_none());
    assert_eq!(restored.version, 5);

    // `change_owner` transfers a record, on its own or with other
    // operations in one transaction.
    let colleague = Uuid::now_v7();
    let outcomes = admin
        .composite(
            &request(json!({"all_or_none": true, "operations": [
                {"method": "update", "reference": "renamed", "object": "custom:person",
                 "id": peter, "expected_version": 5, "values": {"name": "Peter G."}},
                {"method": "change_owner", "reference": "moved", "object": "custom:person",
                 "id": peter, "expected_version": 6, "owner_id": colleague},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(summary(&outcomes), ["renamed:changed", "moved:changed"]);
    let moved = admin
        .get_record("custom:person", &peter, false)
        .await
        .expect("moved");
    assert_eq!(
        (moved.name.as_str(), moved.owner_id),
        ("Peter G.", Some(colleague))
    );
    let outcomes = admin
        .composite(
            &request(json!({"operations": [
                {"method": "change_owner", "reference": "stale", "object": "custom:person",
                 "id": peter, "expected_version": 6, "owner_id": colleague},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(summary(&outcomes), ["stale:VersionConflict"]);

    // Each operation has the caller's own permissions.
    let member = service(&store, caller(tenant, Uuid::now_v7(), &[], &[]));
    let outcomes = member
        .composite(
            &request(json!({"operations": [
                {"method": "create", "reference": "nope", "object": "custom:company",
                 "values": {"name": "Nope"}},
            ]})),
            context(),
        )
        .await
        .expect("composite");
    assert_eq!(summary(&outcomes), ["nope:NotFound"]);

    // Requests are bounded.
    let many: Vec<Value> = (0..26)
        .map(|index| {
            json!({"method": "create", "reference": format!("r{index}"),
                   "object": "custom:company", "values": {"name": "x"}})
        })
        .collect();
    match admin
        .composite(&request(json!({"operations": many})), context())
        .await
    {
        Err(ObjectError::Validation(issues)) => {
            assert_eq!(issues[0].code, "composite.operations_invalid")
        }
        other => panic!("too many operations: {other:?}"),
    }
}
