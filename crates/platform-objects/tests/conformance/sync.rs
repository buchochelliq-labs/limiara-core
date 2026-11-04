//! Device sync on every backend (ADR 0024 §4): the revocation set names the
//! records a device holds that the member can no longer read.

use super::{body, caller, codes, context, object, service};
use platform_objects::*;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

fn manifest(entries: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
    entries
        .iter()
        .map(|(object, ids)| {
            (
                (*object).to_owned(),
                ids.iter().map(|id| (*id).to_owned()).collect(),
            )
        })
        .collect()
}

pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) {
    let admin = service(
        &store,
        caller(
            tenant,
            Uuid::now_v7(),
            &[OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION],
            &[],
        ),
    );
    let role = Uuid::now_v7();
    let ana_id = Uuid::now_v7();
    let ana = service(&store, caller(tenant, ana_id, &[], &[role]));
    let ben = service(&store, caller(tenant, Uuid::now_v7(), &[], &[role]));

    admin
        .create_object(object("account", "Account"), context())
        .await
        .expect("account");
    let mut contact = object("contact", "Contact");
    contact.sharing_model = Some(SharingModel::Private);
    admin
        .create_object(contact, context())
        .await
        .expect("contact");
    let member = AccessFlags {
        read: true,
        create: true,
        edit: true,
        ..AccessFlags::default()
    };
    for api in ["custom:account", "custom:contact"] {
        admin
            .set_object_permission(api, role, member, context())
            .await
            .expect("access");
    }

    let account = admin
        .create_record("custom:account", &body(json!({"name": "Acme"})), context())
        .await
        .expect("account record");
    let own = ana
        .create_record("custom:contact", &body(json!({"name": "Ana's"})), context())
        .await
        .expect("ana's contact");
    let shared = ben
        .create_record("custom:contact", &body(json!({"name": "Ben's"})), context())
        .await
        .expect("ben's contact");
    ben.share_record(
        "custom:contact",
        &shared.public_id,
        GranteeKind::User,
        ana_id,
        ShareAccess::Read,
        context(),
    )
    .await
    .expect("share");

    // Everything Ana holds and may read stays; ids she cannot read, unknown
    // ones and ones that are not ids at all come back, never as an error.
    let held = manifest(&[
        ("custom:account", &[account.public_id.as_str()]),
        (
            "custom:contact",
            &[
                own.public_id.as_str(),
                shared.public_id.as_str(),
                own.public_id.as_str(),
                "zzz000000000000",
                "it's') OR (true",
            ],
        ),
    ]);
    let revoked = ana.revocations(&held).await.expect("revocations");
    assert_eq!(
        revoked,
        manifest(&[("custom:contact", &["it's') OR (true", "zzz000000000000"])])
    );

    // An unshared record, a deleted one and an object Ana can no longer
    // read are all revoked; an object that does not exist is too.
    ben.unshare_record(
        "custom:contact",
        &shared.public_id,
        GranteeKind::User,
        ana_id,
        context(),
    )
    .await
    .expect("unshare");
    admin
        .delete_record("custom:contact", &own.public_id, own.version, context())
        .await
        .expect("delete");
    admin
        .set_object_permission("custom:account", role, AccessFlags::default(), context())
        .await
        .expect("no account access");
    let held = manifest(&[
        ("custom:account", &[account.public_id.as_str()]),
        (
            "custom:contact",
            &[own.public_id.as_str(), shared.public_id.as_str()],
        ),
        ("custom:gone", &["abc"]),
        ("custom:empty", &[]),
    ]);
    let revoked = ana.revocations(&held).await.expect("revocations");
    let mut lost = vec![own.public_id.clone(), shared.public_id.clone()];
    lost.sort();
    assert_eq!(revoked.len(), 3);
    assert_eq!(revoked["custom:account"], vec![account.public_id.clone()]);
    assert_eq!(revoked["custom:contact"], lost);
    assert_eq!(revoked["custom:gone"], vec!["abc".to_owned()]);

    // More than one request's worth of ids is refused.
    let many: Vec<String> = (0..=sync::MANIFEST_MAX).map(|n| format!("id{n}")).collect();
    let mut too_many = BTreeMap::new();
    too_many.insert("custom:contact".to_owned(), many);
    let error = ana.revocations(&too_many).await.expect_err("too many");
    assert_eq!(codes(&error), vec!["sync.manifest_too_large"]);

    // Ids are checked in chunks the query language accepts.
    let ids: Vec<String> = std::iter::once(shared.public_id.clone())
        .chain((0..450).map(|n| format!("x{n}")))
        .collect();
    let mut large = BTreeMap::new();
    large.insert("custom:contact".to_owned(), ids);
    let revoked = admin.revocations(&large).await.expect("large manifest");
    assert_eq!(revoked["custom:contact"].len(), 450);
    assert!(!revoked["custom:contact"].contains(&shared.public_id));
}
