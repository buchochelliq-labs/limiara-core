//! Master-detail relationships on every backend (ADR 0025): details follow
//! their master's sharing, stay with their master, and go to and come back
//! from the recycle bin with it.

use super::{body, caller, codes, context, field, object, service, ADMIN_PERMISSIONS};
use platform_objects::service::ListRecords;
use platform_objects::*;
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

fn master_detail(name: &str, master: &str) -> CreateField {
    let mut definition = field(name, FieldType::MasterDetail);
    definition.reference_object = Some(master.into());
    definition
}

async fn list(who: &ObjectService) -> Result<RecordPage, ObjectError> {
    who.list_records("custom:line", ListRecords::default())
        .await
}

fn names(page: RecordPage) -> Vec<String> {
    let mut names: Vec<String> = page.records.into_iter().map(|record| record.name).collect();
    names.sort();
    names
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    let role = Uuid::now_v7();
    let ana_id = Uuid::now_v7();
    let ana = service(&store, caller(tenant, ana_id, &[], &[role]));
    let ben_id = Uuid::now_v7();
    let ben = service(&store, caller(tenant, ben_id, &[], &[role]));
    let outsider_role = Uuid::now_v7();
    let outsider = service(
        &store,
        caller(tenant, Uuid::now_v7(), &[], &[outsider_role]),
    );

    let mut account = object("account", "Account");
    account.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(account, context())
        .await
        .expect("account");
    admin
        .create_object(object("line", "Line"), context())
        .await
        .expect("line");
    let member = AccessFlags {
        create: true,
        edit: true,
        delete: true,
        ..AccessFlags::default()
    };
    for api_name in ["custom:account", "custom:line"] {
        admin
            .set_object_permission(api_name, role, member, context())
            .await
            .expect("member access");
    }
    // The outsider may use lines but cannot read accounts.
    admin
        .set_object_permission("custom:line", outsider_role, member, context())
        .await
        .expect("outsider access");

    // A master-detail field is always required, never cascades another way
    // and never points to its own object.
    let mut restricted = master_detail("account", "custom:account");
    restricted.delete_behaviour = Some(DeleteBehaviour::Restrict);
    let refused = admin
        .create_field("custom:line", restricted, context())
        .await
        .expect_err("delete behaviour");
    assert_eq!(codes(&refused), ["objects.delete_behaviour_not_allowed"]);
    let refused = admin
        .create_field(
            "custom:line",
            master_detail("itself", "custom:line"),
            context(),
        )
        .await
        .expect_err("own master");
    assert_eq!(codes(&refused), ["objects.master_detail_invalid"]);
    let link = admin
        .create_field(
            "custom:line",
            master_detail("account", "custom:account"),
            context(),
        )
        .await
        .expect("master-detail");
    assert!(link.required, "a detail always has a master");
    let optional: UpdateField =
        serde_json::from_value(json!({"expected_version": link.version, "required": false}))
            .expect("update");
    let refused = admin
        .update_field("custom:line", &link.api_name, optional, context())
        .await
        .expect_err("optional master");
    assert_eq!(codes(&refused), ["objects.master_detail_settings_invalid"]);

    // One level only: a master cannot become a detail, nor a detail a master.
    admin
        .create_object(object("region", "Region"), context())
        .await
        .expect("region");
    let refused = admin
        .create_field(
            "custom:account",
            master_detail("region", "custom:region"),
            context(),
        )
        .await
        .expect_err("master as detail");
    assert_eq!(codes(&refused), ["objects.master_detail_invalid"]);
    let refused = admin
        .create_field(
            "custom:region",
            master_detail("line", "custom:line"),
            context(),
        )
        .await
        .expect_err("detail as master");
    assert_eq!(codes(&refused), ["objects.master_detail_invalid"]);

    let acme = ana
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("acme");
    let globex = ben
        .create_record(
            "custom:account",
            &body(json!({"name": "Globex"})),
            context(),
        )
        .await
        .expect("globex");
    // An object with records cannot get a master: they would have none.
    admin
        .create_object(object("contract", "Contract"), context())
        .await
        .expect("contract");
    admin
        .create_record("custom:contract", &body(json!({"name": "Old"})), context())
        .await
        .expect("contract record");
    let refused = admin
        .create_field(
            "custom:contract",
            master_detail("account", "custom:account"),
            context(),
        )
        .await
        .expect_err("records without a master");
    assert!(matches!(
        refused,
        ObjectError::Conflict("objects.master_detail_has_records")
    ));

    // Adding a detail needs a master the member can edit.
    let refused = ana
        .create_record("custom:line", &body(json!({"name": "Orphan"})), context())
        .await
        .expect_err("no master");
    assert_eq!(codes(&refused), ["records.value_required"]);
    let refused = ana
        .create_record(
            "custom:line",
            &body(json!({"name": "Sneaky", "custom:account": globex.public_id})),
            context(),
        )
        .await
        .expect_err("invisible master");
    assert_eq!(codes(&refused), ["records.reference_not_found"]);
    ben.share_record(
        "custom:account",
        &globex.public_id,
        GranteeKind::User,
        ana_id,
        ShareAccess::Read,
        context(),
    )
    .await
    .expect("read share");
    let refused = ana
        .create_record(
            "custom:line",
            &body(json!({"name": "Sneaky", "custom:account": globex.public_id})),
            context(),
        )
        .await
        .expect_err("read-only master");
    assert_eq!(codes(&refused), ["records.master_read_only"]);
    let anvils = ana
        .create_record(
            "custom:line",
            &body(json!({"name": "Anvils", "custom:account": acme.public_id})),
            context(),
        )
        .await
        .expect("anvils");
    let rockets = ana
        .create_record(
            "custom:line",
            &body(json!({"name": "Rockets", "custom:account": acme.public_id})),
            context(),
        )
        .await
        .expect("rockets");
    let widgets = ben
        .create_record(
            "custom:line",
            &body(json!({"name": "Widgets", "custom:account": globex.public_id})),
            context(),
        )
        .await
        .expect("widgets");

    // Details follow their master: in lists, queries, search and on their
    // own, whoever created them.
    assert_eq!(
        names(list(&ana).await.expect("ana")),
        ["Anvils", "Rockets", "Widgets"]
    );
    assert_eq!(names(list(&ben).await.expect("ben")), ["Widgets"]);
    let rows = ben
        .query(QueryRequest {
            q: "SELECT name FROM custom:line".into(),
            ..QueryRequest::default()
        })
        .await
        .expect("query");
    assert_eq!(rows.rows.len(), 1);
    assert_eq!(rows.rows[0]["name"], json!("Widgets"));
    let hits = ben
        .search(SearchRequest {
            q: "rockets".into(),
            object: None,
            limit: None,
        })
        .await
        .expect("search");
    assert!(hits.iter().all(|group| group.records.is_empty()));
    assert!(matches!(
        ben.get_record("custom:line", &anvils.public_id, false)
            .await,
        Err(ObjectError::NotFound)
    ));
    let (_, rights) = ana
        .get_record_with_rights("custom:line", &widgets.public_id, false)
        .await
        .expect("read through the master's share");
    assert!(rights.read && !rights.edit && !rights.share && !rights.transfer);
    let refused = ana
        .update_record(
            "custom:line",
            &widgets.public_id,
            widgets.version,
            &body(json!({"name": "Mine now"})),
            context(),
        )
        .await
        .expect_err("read-only detail");
    assert!(matches!(refused, ObjectError::PermissionDenied));
    // Details have no shares of their own.
    let refused = ana
        .share_record(
            "custom:line",
            &anvils.public_id,
            GranteeKind::User,
            ben_id,
            ShareAccess::Read,
            context(),
        )
        .await
        .expect_err("detail share");
    assert!(matches!(refused, ObjectError::PermissionDenied));
    // Without read access to the master object there are no details at all.
    assert!(matches!(list(&outsider).await, Err(ObjectError::NotFound)));

    // A detail stays with its master.
    let refused = ana
        .update_record(
            "custom:line",
            &anvils.public_id,
            anvils.version,
            &body(json!({"custom:account": globex.public_id})),
            context(),
        )
        .await
        .expect_err("reparent");
    assert_eq!(codes(&refused), ["records.master_locked"]);

    // Transferring the master moves its details with it.
    let current = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    admin
        .change_owner(
            "custom:account",
            &acme.public_id,
            ben_id,
            current.version,
            context(),
        )
        .await
        .expect("transfer");
    assert_eq!(
        names(list(&ben).await.expect("ben after transfer")),
        ["Anvils", "Rockets", "Widgets"]
    );
    assert_eq!(
        names(list(&ana).await.expect("ana after transfer")),
        ["Widgets"]
    );

    // Deleting the master deletes its details; a detail deleted on its own
    // before stays in the recycle bin when the master comes back.
    let rockets = ben
        .delete_record(
            "custom:line",
            &rockets.public_id,
            rockets.version,
            context(),
        )
        .await
        .expect("delete one detail");
    let acme = admin
        .get_record("custom:account", &acme.public_id, false)
        .await
        .expect("acme");
    let deleted = ben
        .delete_record("custom:account", &acme.public_id, acme.version, context())
        .await
        .expect("delete master");
    assert_eq!(names(list(&ben).await.expect("after delete")), ["Widgets"]);
    let trashed = admin
        .get_record("custom:line", &anvils.public_id, true)
        .await
        .expect("anvils in the bin");
    assert!(trashed.deleted_at.is_some());
    let refused = admin
        .undelete_record("custom:line", &anvils.public_id, trashed.version, context())
        .await
        .expect_err("detail without its master");
    assert!(matches!(
        refused,
        ObjectError::Conflict("records.master_deleted")
    ));
    ben.undelete_record(
        "custom:account",
        &acme.public_id,
        deleted.version,
        context(),
    )
    .await
    .expect("restore master");
    assert_eq!(
        names(list(&ben).await.expect("after restore")),
        ["Anvils", "Widgets"]
    );
    ben.undelete_record(
        "custom:line",
        &rockets.public_id,
        rockets.version,
        context(),
    )
    .await
    .expect("restore the detail deleted on its own");

    // Two masters make a junction object; its sharing follows the first,
    // and it goes to the bin with either master.
    admin
        .create_object(object("product", "Product"), context())
        .await
        .expect("product");
    admin
        .set_object_permission("custom:product", role, member, context())
        .await
        .expect("product access");
    admin
        .create_object(object("offer", "Offer"), context())
        .await
        .expect("offer");
    admin
        .set_object_permission("custom:offer", role, member, context())
        .await
        .expect("offer access");
    for (name, master) in [("account", "custom:account"), ("product", "custom:product")] {
        admin
            .create_field("custom:offer", master_detail(name, master), context())
            .await
            .expect("junction master");
    }
    let refused = admin
        .create_field(
            "custom:offer",
            master_detail("region", "custom:region"),
            context(),
        )
        .await
        .expect_err("third master");
    assert!(matches!(
        refused,
        ObjectError::Conflict("objects.master_detail_limit_reached")
    ));
    let gadget = ben
        .create_record(
            "custom:product",
            &body(json!({"name": "Gadget"})),
            context(),
        )
        .await
        .expect("gadget");
    ben.create_record(
        "custom:offer",
        &body(json!({
            "name": "Gadget for Globex",
            "custom:account": globex.public_id,
            "custom:product": gadget.public_id,
        })),
        context(),
    )
    .await
    .expect("offer");
    assert_eq!(
        names(
            ana.list_records("custom:offer", ListRecords::default())
                .await
                .expect("offers through the account share")
        ),
        ["Gadget for Globex"]
    );
    let gadget = ben
        .delete_record(
            "custom:product",
            &gadget.public_id,
            gadget.version,
            context(),
        )
        .await
        .expect("delete product");
    assert!(ben
        .list_records("custom:offer", ListRecords::default())
        .await
        .expect("offers")
        .records
        .is_empty());
    // With both masters in the bin, the junction record comes back only
    // when the last of them does.
    let globex_now = ben
        .get_record("custom:account", &globex.public_id, false)
        .await
        .expect("globex");
    let globex_deleted = ben
        .delete_record(
            "custom:account",
            &globex.public_id,
            globex_now.version,
            context(),
        )
        .await
        .expect("delete account too");
    ben.undelete_record(
        "custom:product",
        &gadget.public_id,
        gadget.version,
        context(),
    )
    .await
    .expect("restore product");
    assert!(ben
        .list_records("custom:offer", ListRecords::default())
        .await
        .expect("offers")
        .records
        .is_empty());
    ben.undelete_record(
        "custom:account",
        &globex.public_id,
        globex_deleted.version,
        context(),
    )
    .await
    .expect("restore account");
    assert_eq!(
        names(
            ben.list_records("custom:offer", ListRecords::default())
                .await
                .expect("offers back")
        ),
        ["Gadget for Globex"]
    );
}

/// Master-detail stays consistent when changes interleave (ADR 0025), on a
/// store whose transactions can: a record insert that waited for a
/// master-detail field being added is refused instead of saved without a
/// master, and a new detail that waited for its master's deletion is
/// refused instead of left active under a deleted master.
pub async fn locks(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(tenant, Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    for (name, label) in [("account", "Account"), ("line", "Line"), ("item", "Item")] {
        admin
            .create_object(object(name, label), context())
            .await
            .expect("object");
    }
    admin
        .create_field(
            "custom:item",
            master_detail("account", "custom:account"),
            context(),
        )
        .await
        .expect("master-detail");
    let template = admin
        .describe("custom:item")
        .await
        .expect("item")
        .0
        .primary_master()
        .cloned()
        .expect("item master");
    let line = admin.describe("custom:line").await.expect("line").0;
    let pause = std::time::Duration::from_millis(400);

    // A master-detail field commits while a line insert waits for it.
    let mut adding = store.begin(tenant).await.expect("transaction");
    adding.lock_object(line.id, true).await.expect("lock");
    adding
        .insert_field(&FieldDefinition {
            id: Uuid::now_v7(),
            object_id: line.id,
            ..template.clone()
        })
        .await
        .expect("field");
    let early = body(json!({"name": "Early"}));
    let (inserted, ()) = tokio::join!(
        admin.create_record("custom:line", &early, context()),
        async {
            tokio::time::sleep(pause).await;
            adding.commit().await.expect("commit field");
        }
    );
    assert!(
        matches!(
            inserted,
            Err(ObjectError::Conflict("records.definition_changed"))
        ),
        "a record that would have no master is refused: {inserted:?}"
    );

    // The master is deleted while a new detail waits for it.
    let acme = admin
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("acme");
    let mut deleting = store.begin(tenant).await.expect("transaction");
    let mut master = deleting
        .record_by_public_id(&acme.public_id)
        .await
        .expect("read")
        .expect("acme");
    deleting.lock_record(master.id, true).await.expect("lock");
    let version = master.version;
    master.version += 1;
    master.deleted_at = Some(chrono::Utc::now());
    assert!(deleting
        .update_record(&master, version)
        .await
        .expect("delete"));
    let detail = body(json!({"name": "Bolts", "custom:account": acme.public_id}));
    let (created, ()) = tokio::join!(
        admin.create_record("custom:item", &detail, context()),
        async {
            tokio::time::sleep(pause).await;
            deleting.commit().await.expect("commit delete");
        }
    );
    match created {
        Err(error) => assert_eq!(codes(&error), ["records.reference_not_found"]),
        Ok(record) => panic!("a detail of a deleted master was saved: {record:?}"),
    }
}
