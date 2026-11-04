//! Settling a device replica's conflicts with the server (ADR 0024 §3), on
//! SQLite, which devices use: fields only one side changed merge on their
//! own, fields both changed are the member's to choose, and a delete wins
//! over an edit only once the member confirms. Here the server is another
//! workspace in the same store.

use super::push::{flush, methods, remote};
use super::{body, caller, context, field, object, service};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

const OBJECT: &str = "custom:account";

struct Sides {
    server: ObjectService,
    device: ObjectService,
}

impl Sides {
    async fn server_copy(&self, id: &str) -> RemoteRecord {
        remote(
            &self
                .server
                .get_record(OBJECT, id, true)
                .await
                .expect("server copy"),
        )
    }

    async fn local(&self, id: &str) -> Record {
        self.device
            .get_record(OBJECT, id, true)
            .await
            .expect("local copy")
    }

    async fn edit(&self, engine: &ObjectService, id: &str, values: serde_json::Value) {
        let current = engine.get_record(OBJECT, id, true).await.expect("record");
        engine
            .update_record(OBJECT, id, current.version, &body(values), context())
            .await
            .expect("edit");
    }

    /// A record both sides hold, as the server has it.
    async fn shared(&self, name: &str) -> String {
        let made = self
            .server
            .create_record(
                OBJECT,
                &body(json!({"name": name, "custom:tier": "gold", "custom:region": "north"})),
                context(),
            )
            .await
            .expect("server record");
        self.device
            .apply_remote(OBJECT, &remote(&made), None)
            .await
            .expect("pulled");
        made.public_id
    }

    /// Pushes the device's queue and settles nothing.
    async fn push(&self) -> PushReport {
        let batch = self.device.push_queue().await.expect("queue");
        flush(&self.device, &self.server, &batch).await
    }
}

pub async fn run(store: Arc<dyn ObjectStore>) {
    let member = Uuid::now_v7();
    let permissions = [OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION];
    let sides = Sides {
        server: service(&store, caller(Uuid::now_v7(), member, &permissions, &[])),
        device: service(&store, caller(Uuid::now_v7(), member, &permissions, &[]))
            .with_replica()
            .with_client_ids(),
    };
    for engine in [&sides.server, &sides.device] {
        engine
            .create_object(object("account", "Account"), context())
            .await
            .expect("account");
        for name in ["tier", "region"] {
            engine
                .create_field(OBJECT, field(name, FieldType::Text), context())
                .await
                .expect("field");
        }
    }

    // Only a replica's engine merges.
    let id = sides.shared("Acme").await;
    let copy = sides.server_copy(&id).await;
    assert!(matches!(
        sides.server.merge_remote(OBJECT, &copy).await,
        Err(ObjectError::PermissionDenied)
    ));

    // Fields only one side changed merge on their own: the device's edit
    // stays and pushes, and the server's arrives.
    sides
        .edit(&sides.device, &id, json!({"custom:tier": "silver"}))
        .await;
    sides
        .edit(&sides.server, &id, json!({"custom:region": "south"}))
        .await;
    let report = sides.push().await;
    assert_eq!(report.failed.len(), 1, "a version conflict: {report:?}");
    let server = sides.server_copy(&id).await;
    let merge = sides
        .device
        .merge_remote(OBJECT, &server)
        .await
        .expect("merge");
    assert!(merge.is_automatic(), "{merge:?}");
    assert_eq!(merge.from_server, vec!["custom:region".to_owned()]);
    let settled = sides
        .device
        .settle(OBJECT, &server, &Choices::default(), context())
        .await
        .expect("settle");
    assert_eq!(
        (
            &settled.values["custom:tier"],
            &settled.values["custom:region"]
        ),
        (&json!("silver"), &json!("south"))
    );
    let batch = sides.device.push_queue().await.expect("queue");
    assert_eq!(methods(&batch), vec![("update", id.as_str())]);
    let platform_objects::composite::CompositeOperation::Update {
        values,
        expected_version,
        ..
    } = &batch.operations[0].operation
    else {
        unreachable!()
    };
    assert_eq!(values, &body(json!({"custom:tier": "silver"})));
    assert_eq!(*expected_version, server.version);
    assert!(flush(&sides.device, &sides.server, &batch)
        .await
        .failed
        .is_empty());
    let both = sides.server_copy(&id).await;
    assert_eq!(
        (&both.values["custom:tier"], &both.values["custom:region"]),
        (&json!("silver"), &json!("south"))
    );
    assert!(sides.device.push_queue().await.expect("queue").is_empty());

    // Both changed a field to the same value: nothing to choose.
    sides
        .edit(&sides.device, &id, json!({"custom:tier": "bronze"}))
        .await;
    sides
        .edit(&sides.server, &id, json!({"custom:tier": "bronze"}))
        .await;
    let server = sides.server_copy(&id).await;
    let merge = sides
        .device
        .merge_remote(OBJECT, &server)
        .await
        .expect("merge");
    assert!(merge.is_automatic(), "{merge:?}");
    sides
        .device
        .settle(OBJECT, &server, &Choices::default(), context())
        .await
        .expect("settle");
    assert!(
        sides.device.push_queue().await.expect("queue").is_empty(),
        "the device already agrees with the server"
    );

    // Both changed a field differently: the member chooses. Without a
    // choice, nothing settles.
    sides
        .edit(&sides.device, &id, json!({"custom:tier": "device"}))
        .await;
    sides
        .edit(&sides.device, &id, json!({"custom:tier": "device, again"}))
        .await;
    sides
        .edit(&sides.server, &id, json!({"custom:tier": "server"}))
        .await;
    let server = sides.server_copy(&id).await;
    let merge = sides
        .device
        .merge_remote(OBJECT, &server)
        .await
        .expect("merge");
    assert_eq!(
        merge.conflicts,
        vec![FieldConflict {
            field: "custom:tier".into(),
            base: json!("bronze"),
            local: json!("device, again"),
            server: json!("server"),
        }],
        "the base is the server copy the member started from"
    );
    match sides
        .device
        .settle(OBJECT, &server, &Choices::default(), context())
        .await
    {
        Err(ObjectError::Validation(issues)) => {
            assert_eq!(
                (issues[0].field.as_str(), issues[0].code),
                ("custom:tier", "sync.choice_missing")
            )
        }
        other => panic!("a missing choice: {other:?}"),
    }
    let keep_server = Choices {
        fields: [("custom:tier".to_owned(), Side::Server)].into(),
        ..Choices::default()
    };
    let settled = sides
        .device
        .settle(OBJECT, &server, &keep_server, context())
        .await
        .expect("settle");
    assert_eq!(settled.values["custom:tier"], json!("server"));
    assert_eq!(settled.version, server.version);
    assert!(sides.device.push_queue().await.expect("queue").is_empty());

    // Choosing the device's value pushes it.
    sides
        .edit(&sides.device, &id, json!({"name": "Acme Ltd"}))
        .await;
    sides
        .edit(&sides.server, &id, json!({"name": "Acme Inc"}))
        .await;
    let server = sides.server_copy(&id).await;
    let keep_local = Choices {
        fields: [("name".to_owned(), Side::Local)].into(),
        ..Choices::default()
    };
    sides
        .device
        .settle(OBJECT, &server, &keep_local, context())
        .await
        .expect("settle");
    assert!(sides.push().await.failed.is_empty());
    assert_eq!(sides.server_copy(&id).await.name, "Acme Ltd");

    // An owner both sides changed is a conflict on `owner`.
    let colleague = Uuid::now_v7();
    let local = sides.local(&id).await;
    sides
        .device
        .change_owner(OBJECT, &id, colleague, local.version, context())
        .await
        .expect("transfer here");
    let on_server = sides.server_copy(&id).await;
    sides
        .server
        .change_owner(OBJECT, &id, Uuid::now_v7(), on_server.version, context())
        .await
        .expect("transfer there");
    let server = sides.server_copy(&id).await;
    let merge = sides
        .device
        .merge_remote(OBJECT, &server)
        .await
        .expect("merge");
    assert_eq!(
        merge
            .conflicts
            .iter()
            .map(|conflict| conflict.field.as_str())
            .collect::<Vec<_>>(),
        vec!["owner"]
    );
    let keep_mine = Choices {
        fields: [("owner".to_owned(), Side::Local)].into(),
        ..Choices::default()
    };
    sides
        .device
        .settle(OBJECT, &server, &keep_mine, context())
        .await
        .expect("settle");
    let batch = sides.device.push_queue().await.expect("queue");
    assert_eq!(methods(&batch), vec![("change_owner", id.as_str())]);
    flush(&sides.device, &sides.server, &batch).await;
    assert_eq!(sides.server_copy(&id).await.owner_id, Some(colleague));

    // The device deleted while the server edited: confirming keeps the
    // deletion, which then pushes.
    let deleted_here = sides.shared("Deleted here").await;
    let local = sides.local(&deleted_here).await;
    sides
        .device
        .delete_record(OBJECT, &deleted_here, local.version, context())
        .await
        .expect("delete here");
    sides
        .edit(
            &sides.server,
            &deleted_here,
            json!({"custom:tier": "edited"}),
        )
        .await;
    let server = sides.server_copy(&deleted_here).await;
    let merge = sides
        .device
        .merge_remote(OBJECT, &server)
        .await
        .expect("merge");
    assert_eq!(merge.deletion, Some(DeleteConflict::DeletedHere));
    assert!(matches!(
        sides
            .device
            .settle(OBJECT, &server, &Choices::default(), context())
            .await,
        Err(ObjectError::Validation(_))
    ));
    let confirm = Choices {
        keep_deletion: Some(true),
        ..Choices::default()
    };
    sides
        .device
        .settle(OBJECT, &server, &confirm, context())
        .await
        .expect("settle");
    let batch = sides.device.push_queue().await.expect("queue");
    assert_eq!(methods(&batch), vec![("delete", deleted_here.as_str())]);
    flush(&sides.device, &sides.server, &batch).await;
    assert!(sides.server_copy(&deleted_here).await.deleted_at.is_some());

    // Undoing the deletion takes the server's edit instead.
    let undone = sides.shared("Undone").await;
    let local = sides.local(&undone).await;
    sides
        .device
        .delete_record(OBJECT, &undone, local.version, context())
        .await
        .expect("delete here");
    sides
        .edit(&sides.server, &undone, json!({"custom:tier": "kept"}))
        .await;
    let server = sides.server_copy(&undone).await;
    let undo = Choices {
        keep_deletion: Some(false),
        ..Choices::default()
    };
    let settled = sides
        .device
        .settle(OBJECT, &server, &undo, context())
        .await
        .expect("settle");
    assert!(settled.deleted_at.is_none());
    assert_eq!(settled.values["custom:tier"], json!("kept"));
    assert!(sides.device.push_queue().await.expect("queue").is_empty());

    // The server deleted while the device edited: confirming takes the
    // deletion and drops the edit; undoing restores the record with it.
    for keep in [true, false] {
        let gone = sides.shared("Deleted on the server").await;
        sides
            .edit(&sides.device, &gone, json!({"custom:region": "west"}))
            .await;
        let on_server = sides.server_copy(&gone).await;
        sides
            .server
            .delete_record(OBJECT, &gone, on_server.version, context())
            .await
            .expect("delete there");
        let server = sides.server_copy(&gone).await;
        let merge = sides
            .device
            .merge_remote(OBJECT, &server)
            .await
            .expect("merge");
        assert_eq!(merge.deletion, Some(DeleteConflict::DeletedOnServer));
        let choice = Choices {
            keep_deletion: Some(keep),
            ..Choices::default()
        };
        let settled = sides
            .device
            .settle(OBJECT, &server, &choice, context())
            .await
            .expect("settle");
        assert_eq!(settled.deleted_at.is_some(), keep);
        if keep {
            assert!(sides.device.push_queue().await.expect("queue").is_empty());
            continue;
        }
        let batch = sides.device.push_queue().await.expect("queue");
        assert_eq!(methods(&batch), vec![("undelete", gone.as_str())]);
        flush(&sides.device, &sides.server, &batch).await;
        let batch = sides.device.push_queue().await.expect("queue");
        assert_eq!(methods(&batch), vec![("update", gone.as_str())]);
        flush(&sides.device, &sides.server, &batch).await;
        let restored = sides.server_copy(&gone).await;
        assert!(restored.deleted_at.is_none());
        assert_eq!(restored.values["custom:region"], json!("west"));
    }

    // A record made on the device has no server copy to merge with until
    // it is pushed.
    let made_here = sides
        .device
        .create_record(OBJECT, &body(json!({"name": "Made here"})), context())
        .await
        .expect("made here");
    sides
        .edit(
            &sides.device,
            &made_here.public_id,
            json!({"custom:tier": "new"}),
        )
        .await;
    assert!(sides.push().await.failed.is_empty());
    let pushed = sides.server_copy(&made_here.public_id).await;
    let merge = sides
        .device
        .merge_remote(OBJECT, &pushed)
        .await
        .expect("merge");
    assert_eq!(merge, Merge::default(), "nothing pending after the push");
}
