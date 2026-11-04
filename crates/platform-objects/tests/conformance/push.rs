//! A synced device's push queue (ADR 0024 §3), on SQLite, which devices
//! use: the member's own edits push as composite requests to the server,
//! the answers replace the replica's records, and the queue empties. Here
//! the server is another workspace in the same store.

use super::{body, caller, context, field, object, service};
use platform_objects::composite::{CompositeOperation, CompositeOutcome};
use platform_objects::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

pub fn remote(record: &Record) -> RemoteRecord {
    RemoteRecord {
        public_id: record.public_id.clone(),
        name: record.name.clone(),
        owner_id: record.owner_id,
        values: record
            .values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        version: record.version,
        created_at: record.created_at,
        created_by: record.created_by,
        updated_at: record.updated_at,
        updated_by: record.updated_by,
        touched_at: record.touched_at,
        deleted_at: record.deleted_at,
    }
}

fn code(error: &ObjectError) -> String {
    match error {
        ObjectError::VersionConflict => "records.version_conflict".into(),
        ObjectError::Conflict(code) => (*code).into(),
        ObjectError::Validation(issues) => issues
            .first()
            .map_or("validation.failed", |issue| issue.code)
            .into(),
        other => format!("{other:?}"),
    }
}

/// Sends a batch to the server and gives the device the answers.
pub async fn flush(
    device: &ObjectService,
    server: &ObjectService,
    batch: &PushBatch,
) -> PushReport {
    let outcomes = server
        .composite(&batch.request(), context())
        .await
        .expect("composite");
    let answers: BTreeMap<String, PushAnswer> = outcomes
        .into_iter()
        .map(|(reference, outcome)| {
            let answer = match outcome {
                CompositeOutcome::Done { record, .. } => PushAnswer::Done(remote(&record)),
                CompositeOutcome::Failed(error) => PushAnswer::Failed {
                    status: 409,
                    code: code(&error),
                },
                other => panic!("operations run apart: {other:?}"),
            };
            (reference, answer)
        })
        .collect();
    device
        .take_push_answers(batch, &answers, context())
        .await
        .expect("answers")
}

pub fn methods(batch: &PushBatch) -> Vec<(&'static str, &str)> {
    batch
        .operations
        .iter()
        .map(|pushed| {
            let method = match pushed.operation {
                CompositeOperation::Create { .. } => "create",
                CompositeOperation::Update { .. } => "update",
                CompositeOperation::Delete { .. } => "delete",
                CompositeOperation::Undelete { .. } => "undelete",
                CompositeOperation::ChangeOwner { .. } => "change_owner",
            };
            (method, pushed.public_id.as_str())
        })
        .collect()
}

async fn local_head(device: &ObjectService) -> i64 {
    device
        .list_changes(None, Some(200), None)
        .await
        .expect("changes")
        .head
}

pub async fn run(store: Arc<dyn ObjectStore>) {
    let member = Uuid::now_v7();
    let permissions = [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION];
    let server = service(&store, caller(Uuid::now_v7(), member, &permissions, &[]));
    let device = service(&store, caller(Uuid::now_v7(), member, &permissions, &[]))
        .with_replica()
        .with_client_ids();
    // The device holds the server's definitions.
    for engine in [&server, &device] {
        engine
            .create_object(object("account", "Account"), context())
            .await
            .expect("account");
        let mut parent = field("parent", FieldType::Lookup);
        parent.reference_object = Some("custom:account".into());
        engine
            .create_field("custom:account", parent, context())
            .await
            .expect("parent");
        engine
            .create_field("custom:account", field("tier", FieldType::Text), context())
            .await
            .expect("tier");
    }

    // Only a replica's engine has a push queue.
    assert!(matches!(
        server.push_queue().await,
        Err(ObjectError::PermissionDenied)
    ));
    assert!(device.push_queue().await.expect("queue").is_empty());
    assert_eq!(
        device.sync_positions().await.expect("positions"),
        SyncPositions::default()
    );

    // Two records made offline point at each other. The first one's lookup
    // waits for the second to reach the server.
    let first = device
        .create_record(
            "custom:account",
            &body(json!({"name": "First", "custom:tier": "gold"})),
            context(),
        )
        .await
        .expect("first");
    let second = device
        .create_record(
            "custom:account",
            &body(json!({"name": "Second", "custom:parent": first.public_id})),
            context(),
        )
        .await
        .expect("second");
    let first = device
        .update_record(
            "custom:account",
            &first.public_id,
            first.version,
            &body(json!({"custom:parent": second.public_id})),
            context(),
        )
        .await
        .expect("first points at second");
    // Made and discarded offline: the server never hears of it.
    let scrap = device
        .create_record("custom:account", &body(json!({"name": "Scrap"})), context())
        .await
        .expect("scrap");
    device
        .delete_record("custom:account", &scrap.public_id, scrap.version, context())
        .await
        .expect("discard");

    let batch = device.push_queue().await.expect("queue");
    assert_eq!(
        methods(&batch),
        vec![
            ("create", first.public_id.as_str()),
            ("create", second.public_id.as_str())
        ]
    );
    let CompositeOperation::Create { values, .. } = &batch.operations[0].operation else {
        unreachable!()
    };
    assert_eq!(values["id"], json!(first.public_id));
    assert_eq!(values["custom:tier"], json!("gold"));
    assert!(!values.contains_key("custom:parent"), "{values:?}");
    assert_eq!(
        batch.operations[0].held_back["custom:parent"],
        json!(second.public_id)
    );
    let report = flush(&device, &server, &batch).await;
    assert_eq!(report.stored.len(), 2, "{report:?}");
    assert!(report.failed.is_empty() && report.edited_since.is_empty());

    // The held-back lookup goes next, against the server's version.
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(methods(&batch), vec![("update", first.public_id.as_str())]);
    let CompositeOperation::Update {
        expected_version,
        values,
        ..
    } = &batch.operations[0].operation
    else {
        unreachable!()
    };
    let on_server = server
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("on server");
    assert_eq!(*expected_version, on_server.version);
    assert_eq!(values["custom:parent"], json!(second.public_id));
    flush(&device, &server, &batch).await;
    let on_server = server
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("on server");
    assert_eq!(on_server.values["custom:parent"], json!(second.public_id));
    assert!(device.push_queue().await.expect("queue").is_empty());
    let positions = device.sync_positions().await.expect("positions");
    assert_eq!(positions.pushed, local_head(&device).await);
    let local = device
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("local");
    assert_eq!(
        local.version, on_server.version,
        "the replica took the answer"
    );

    // An edit made while a push is out is kept, and goes next time.
    let local = device
        .update_record(
            "custom:account",
            &first.public_id,
            local.version,
            &body(json!({"custom:tier": "silver"})),
            context(),
        )
        .await
        .expect("edit");
    let batch = device.push_queue().await.expect("queue");
    let CompositeOperation::Update { values, .. } = &batch.operations[0].operation else {
        unreachable!()
    };
    assert_eq!(
        values,
        &body(json!({"custom:tier": "silver"})),
        "only changed fields"
    );
    device
        .update_record(
            "custom:account",
            &first.public_id,
            local.version,
            &body(json!({"custom:tier": null})),
            context(),
        )
        .await
        .expect("edit while out");
    let report = flush(&device, &server, &batch).await;
    assert_eq!(report.edited_since, vec![first.public_id.clone()]);
    let local = device
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("local");
    assert!(!local.values.contains_key("custom:tier"), "the edit stays");
    let batch = device.push_queue().await.expect("queue");
    let CompositeOperation::Update { values, .. } = &batch.operations[0].operation else {
        unreachable!()
    };
    assert_eq!(values["custom:tier"], json!(null), "a cleared field clears");
    assert!(flush(&device, &server, &batch).await.failed.is_empty());
    let on_server = server
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("on server");
    assert!(!on_server.values.contains_key("custom:tier"));

    // An owner change pushes as its own operation. Made with other edits,
    // it is held back and goes next time.
    let colleague = Uuid::now_v7();
    let local = device
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("local");
    device
        .change_owner(
            "custom:account",
            &first.public_id,
            colleague,
            local.version,
            context(),
        )
        .await
        .expect("transfer");
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(
        methods(&batch),
        vec![("change_owner", first.public_id.as_str())]
    );
    assert!(flush(&device, &server, &batch).await.failed.is_empty());
    let on_server = server
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("on server");
    assert_eq!(on_server.owner_id, Some(colleague));
    assert!(device.push_queue().await.expect("queue").is_empty());
    let local = device
        .update_record(
            "custom:account",
            &first.public_id,
            on_server.version,
            &body(json!({"custom:tier": "bronze"})),
            context(),
        )
        .await
        .expect("edit");
    device
        .change_owner(
            "custom:account",
            &first.public_id,
            member,
            local.version,
            context(),
        )
        .await
        .expect("transfer back");
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(methods(&batch), vec![("update", first.public_id.as_str())]);
    assert_eq!(batch.operations[0].held_owner, Some(member));
    assert!(flush(&device, &server, &batch).await.failed.is_empty());
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(
        methods(&batch),
        vec![("change_owner", first.public_id.as_str())]
    );
    assert!(flush(&device, &server, &batch).await.failed.is_empty());
    let on_server = server
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("on server");
    assert_eq!(
        (on_server.owner_id, on_server.values["custom:tier"].clone()),
        (Some(member), json!("bronze"))
    );
    assert!(device.push_queue().await.expect("queue").is_empty());
    // Transferred away and back before it went: the server finds nothing
    // to change, and the device takes its copy.
    let local = device
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("local");
    let local = device
        .change_owner(
            "custom:account",
            &first.public_id,
            colleague,
            local.version,
            context(),
        )
        .await
        .expect("away");
    device
        .change_owner(
            "custom:account",
            &first.public_id,
            member,
            local.version,
            context(),
        )
        .await
        .expect("back");
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(
        methods(&batch),
        vec![("change_owner", first.public_id.as_str())]
    );
    flush(&device, &server, &batch).await;
    assert!(device.push_queue().await.expect("queue").is_empty());
    let local = device
        .get_record("custom:account", &first.public_id, true)
        .await
        .expect("local");
    assert_eq!(local.version, on_server.version);

    // Deleting and restoring push as the server's own operations. The
    // delete clears the lookup that points at the record, which pushes
    // first, so the server sees the same changes in the same order.
    let local = device
        .get_record("custom:account", &second.public_id, true)
        .await
        .expect("second");
    device
        .delete_record(
            "custom:account",
            &second.public_id,
            local.version,
            context(),
        )
        .await
        .expect("delete");
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(
        methods(&batch),
        vec![
            ("update", first.public_id.as_str()),
            ("delete", second.public_id.as_str())
        ]
    );
    let CompositeOperation::Update { values, .. } = &batch.operations[0].operation else {
        unreachable!()
    };
    assert_eq!(values["custom:parent"], json!(null));
    assert!(flush(&device, &server, &batch).await.failed.is_empty());
    let deleted = server
        .get_record("custom:account", &second.public_id, true)
        .await
        .expect("in the recycle bin");
    assert!(deleted.deleted_at.is_some());
    let local = device
        .get_record("custom:account", &second.public_id, true)
        .await
        .expect("local");
    device
        .undelete_record(
            "custom:account",
            &second.public_id,
            local.version,
            context(),
        )
        .await
        .expect("restore");
    let batch = device.push_queue().await.expect("queue");
    assert_eq!(
        methods(&batch),
        vec![("undelete", second.public_id.as_str())]
    );
    flush(&device, &server, &batch).await;
    let restored = server
        .get_record("custom:account", &second.public_id, true)
        .await
        .expect("restored");
    assert!(restored.deleted_at.is_none());
    assert!(device.push_queue().await.expect("queue").is_empty());

    // A change someone made on the server first fails the push; the edit
    // stays queued for the member to settle.
    server
        .update_record(
            "custom:account",
            &second.public_id,
            restored.version,
            &body(json!({"name": "Renamed on the server"})),
            context(),
        )
        .await
        .expect("server edit");
    let local = device
        .get_record("custom:account", &second.public_id, true)
        .await
        .expect("local");
    device
        .update_record(
            "custom:account",
            &second.public_id,
            local.version,
            &body(json!({"name": "Renamed on the device"})),
            context(),
        )
        .await
        .expect("device edit");
    let batch = device.push_queue().await.expect("queue");
    let report = flush(&device, &server, &batch).await;
    assert_eq!(
        report.failed,
        vec![(
            "custom:account".to_owned(),
            second.public_id.clone(),
            "records.version_conflict".to_owned()
        )]
    );
    assert_eq!(
        methods(&device.push_queue().await.expect("queue")),
        vec![("update", second.public_id.as_str())],
        "a failed edit stays queued"
    );
    let unanswered = device
        .take_push_answers(&batch, &BTreeMap::new(), context())
        .await
        .expect("no answers");
    assert_eq!(unanswered.unanswered, vec![second.public_id.clone()]);

    // A record whose changes run past what one push reads sends every
    // field, so a change named only later is not lost.
    let long = device
        .create_record("custom:account", &body(json!({"name": "Long"})), context())
        .await
        .expect("long");
    flush(&device, &server, &device.push_queue().await.expect("queue")).await;
    let mut long = device
        .get_record("custom:account", &long.public_id, true)
        .await
        .expect("long");
    for round in 0..1_000 {
        long = device
            .update_record(
                "custom:account",
                &long.public_id,
                long.version,
                &body(json!({"custom:tier": format!("t{round}")})),
                context(),
            )
            .await
            .expect("edit");
    }
    device
        .update_record(
            "custom:account",
            &long.public_id,
            long.version,
            &body(json!({"custom:parent": first.public_id})),
            context(),
        )
        .await
        .expect("point at first last");
    let batch = device.push_queue().await.expect("queue");
    let pushed = batch
        .operations
        .iter()
        .find(|pushed| pushed.public_id == long.public_id)
        .expect("queued");
    let CompositeOperation::Update { values, .. } = &pushed.operation else {
        unreachable!()
    };
    assert_eq!(values["custom:parent"], json!(first.public_id));
    assert_eq!(values["custom:tier"], json!("t999"));
    flush(&device, &server, &batch).await;
    let on_server = server
        .get_record("custom:account", &long.public_id, true)
        .await
        .expect("on server");
    assert_eq!(on_server.values["custom:parent"], json!(first.public_id));

    // The pull position is stored and never moves back.
    let positions = device.record_pull(40, 3).await.expect("pull");
    assert_eq!(
        (positions.pulled, positions.metadata_version),
        (Some(40), Some(3))
    );
    let positions = device.record_pull(12, 2).await.expect("older pull");
    assert_eq!(
        (positions.pulled, positions.metadata_version),
        (Some(40), Some(3))
    );
    assert!(server.record_pull(1, 1).await.is_err());
}
