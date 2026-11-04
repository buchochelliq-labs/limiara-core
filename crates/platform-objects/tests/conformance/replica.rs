//! A synced device's replica (ADR 0024 §3), on SQLite, which devices use:
//! records the server sends are stored as the server has them, leave no
//! entry in the device's change log, and never overwrite the member's
//! unpushed edits.

use super::{body, caller, context, field, object, service};
use chrono::{Duration, SubsecRound, Utc};
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn remote(public_id: &str, name: &str, version: i64) -> RemoteRecord {
    // Stores keep microseconds, as the server's stamps have.
    let at = (Utc::now() - Duration::hours(1)).trunc_subsecs(6);
    RemoteRecord {
        public_id: public_id.into(),
        name: name.into(),
        owner_id: Some(Uuid::now_v7()),
        values: serde_json::Map::new(),
        version,
        created_at: at,
        created_by: None,
        updated_at: at,
        updated_by: None,
        touched_at: at,
        deleted_at: None,
    }
}

fn record(applied: Applied) -> Record {
    match applied {
        Applied::Inserted(record)
        | Applied::Updated(record)
        | Applied::Unchanged(record)
        | Applied::Conflict(record) => record,
    }
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let member = caller(
        tenant,
        Uuid::now_v7(),
        &[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION],
        &[],
    );
    let server_side = service(&store, member.clone());
    let replica = service(&store, member).with_replica();
    let account = replica
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut parent = field("parent", FieldType::Lookup);
    parent.reference_object = Some("custom:account".into());
    parent.indexed = true;
    replica
        .create_field("custom:account", parent, context())
        .await
        .expect("parent");
    let mut tier = field("tier", FieldType::Text);
    tier.indexed = true;
    replica
        .create_field("custom:account", tier, context())
        .await
        .expect("tier");
    let id = |n: u64| {
        platform_id::PublicId::new(&account.key_prefix, n)
            .expect("id")
            .as_str()
            .to_owned()
    };

    // Only a replica's engine stores server records.
    let error = server_side
        .apply_remote("custom:account", &remote("x", "X", 1), None)
        .await
        .expect_err("not a replica");
    assert!(matches!(error, ObjectError::PermissionDenied));

    let head = replica
        .list_changes(None, Some(200), None)
        .await
        .expect("changes")
        .head;

    // A child can arrive before its lookup's target; the target's arrival
    // indexes the lookup.
    let parent_id = id(501);
    let child_id = id(502);
    let mut child = remote(&child_id, "Child", 7);
    child.values = body(json!({
        "custom:parent": parent_id,
        "custom:tier": "gold",
        "custom:unknown": "dropped",
    }));
    assert!(matches!(
        replica
            .apply_remote("custom:account", &child, None)
            .await
            .expect("child first"),
        Applied::Inserted(_)
    ));
    let lookup = format!(
        "SELECT id FROM custom:account WHERE custom:parent = '{parent_id}' AND custom:tier = 'gold'"
    );
    let query = |q: String| QueryRequest {
        q,
        ..QueryRequest::default()
    };
    let before = replica.query(query(lookup.clone())).await.expect("query");
    assert!(before.rows.is_empty(), "no target yet: {:?}", before.rows);
    replica
        .apply_remote("custom:account", &remote(&parent_id, "Parent", 3), None)
        .await
        .expect("parent");
    let after = replica.query(query(lookup)).await.expect("query");
    assert_eq!(after.rows.len(), 1);
    assert_eq!(after.rows[0]["id"], json!(child_id));

    // The record is the server's: id, version, owner and stamps.
    let stored = replica
        .get_record("custom:account", &child_id, false)
        .await
        .expect("stored");
    assert_eq!(stored.version, 7);
    assert_eq!(stored.owner_id, child.owner_id);
    assert_eq!(stored.created_at, child.created_at);
    assert_eq!(stored.values["custom:tier"], json!("gold"));
    assert!(!stored.values.contains_key("custom:unknown"));
    assert_eq!(
        replica
            .remote_version("custom:account", &child_id)
            .await
            .expect("version"),
        Some(7)
    );

    // An older or equal version changes nothing; a newer one replaces it.
    let mut stale = child.clone();
    stale.version = 6;
    stale.name = "Stale".into();
    assert!(matches!(
        replica
            .apply_remote("custom:account", &stale, None)
            .await
            .expect("stale"),
        Applied::Unchanged(_)
    ));
    let mut newer = child.clone();
    newer.version = 8;
    newer.name = "Renamed".into();
    newer.deleted_at = Some(Utc::now());
    assert!(matches!(
        replica
            .apply_remote("custom:account", &newer, None)
            .await
            .expect("newer"),
        Applied::Updated(_)
    ));
    assert!(replica
        .get_record("custom:account", &child_id, false)
        .await
        .is_err());

    // None of it entered the change log.
    let changes = replica
        .list_changes(Some(head), Some(200), None)
        .await
        .expect("changes");
    assert!(changes.changes.is_empty(), "{:?}", changes.changes);

    // The member's edit is logged, and a pull never overwrites it.
    let edited = replica
        .update_record(
            "custom:account",
            &parent_id,
            3,
            &body(json!({"name": "Edited here"})),
            context(),
        )
        .await
        .expect("local edit");
    assert_eq!(edited.version, 4);
    let changes = replica
        .list_changes(Some(head), Some(200), None)
        .await
        .expect("changes");
    assert_eq!(changes.changes.len(), 1);
    let pulled = replica
        .apply_remote(
            "custom:account",
            &remote(&parent_id, "Changed on the server", 4),
            None,
        )
        .await
        .expect("pull over an edit");
    assert!(matches!(pulled, Applied::Conflict(_)));
    assert_eq!(record(pulled).name, "Edited here");

    // The server's answer to the push replaces the edit, even at a version
    // the local record already passed.
    let mut answer = remote(&parent_id, "Edited here", 4);
    answer.updated_by = Some(Uuid::now_v7());
    let confirmed = replica
        .apply_remote("custom:account", &answer, Some(edited.version))
        .await
        .expect("push confirmed");
    assert!(matches!(confirmed, Applied::Updated(_)));
    assert_eq!(record(confirmed).updated_by, answer.updated_by);
    assert!(matches!(
        replica
            .apply_remote("custom:account", &answer, None)
            .await
            .expect("same version"),
        Applied::Unchanged(_)
    ));

    // An edit made while a push was in flight is kept, and builds on the
    // server version the push produced.
    let again = replica
        .update_record(
            "custom:account",
            &parent_id,
            4,
            &body(json!({"name": "Edited again"})),
            context(),
        )
        .await
        .expect("second edit");
    let late = replica
        .apply_remote("custom:account", &remote(&parent_id, "Pushed", 5), Some(4))
        .await
        .expect("answer to the earlier push");
    assert!(matches!(late, Applied::Conflict(_)));
    assert_eq!(record(late).name, "Edited again");
    assert_eq!(again.version, 5);
    assert_eq!(
        replica
            .remote_version("custom:account", &parent_id)
            .await
            .expect("version"),
        Some(5)
    );

    // After-save automations run on the server, not on the replica.
    let automation: CreateAutomation = serde_json::from_value(json!({
        "name": "follow_up", "trigger": "create", "timing": "after_save",
        "condition": "name <> 'Follow-up'",
        "actions": [{"type": "create_record", "object": "custom:account",
                     "values": [{"field": "name", "formula": "'Follow-up'"}]}],
    }))
    .expect("automation");
    replica
        .create_automation("custom:account", automation, context())
        .await
        .expect("automation");
    replica
        .create_record(
            "custom:account",
            &body(json!({"name": "Made here"})),
            context(),
        )
        .await
        .expect("local create");
    let follow_ups = replica
        .query(query(
            "SELECT id FROM custom:account WHERE name = 'Follow-up'".into(),
        ))
        .await
        .expect("query");
    assert!(follow_ups.rows.is_empty(), "{:?}", follow_ups.rows);
    // The same save on a server's engine runs it.
    server_side
        .create_record(
            "custom:account",
            &body(json!({"name": "Made there"})),
            context(),
        )
        .await
        .expect("server create");
    let follow_ups = replica
        .query(query(
            "SELECT id FROM custom:account WHERE name = 'Follow-up'".into(),
        ))
        .await
        .expect("query");
    assert_eq!(follow_ups.rows.len(), 1);
}
