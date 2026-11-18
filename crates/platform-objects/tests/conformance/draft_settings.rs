//! Record draft settings on every backend (ADR 0036 §9): any member reads
//! the workspace's draft expiry, only administrators change it, a change is
//! versioned and moves the metadata version, and nothing about drafts
//! reaches the change log.

use super::{caller, context, service, ADMIN_PERMISSIONS};
use platform_objects::*;
use std::sync::Arc;
use uuid::Uuid;

fn change(expiry_days: i32, expected_version: i64) -> UpdateDraftSettings {
    UpdateDraftSettings {
        expiry_days,
        expected_version,
    }
}

/// Returns how many settings changes succeeded, each of which left one
/// audit entry and one event.
pub async fn run(store: Arc<dyn ObjectStore>, tenant: Uuid) -> i64 {
    let administrator = Uuid::now_v7();
    let admin = service(
        &store,
        caller(tenant, administrator, &ADMIN_PERMISSIONS, &[]),
    );
    let member = service(
        &store,
        caller(tenant, Uuid::now_v7(), &[], &[Uuid::now_v7()]),
    );

    // No row: the default, for any member.
    let default = member.draft_settings().await.expect("default");
    assert_eq!(default, DraftSettings::default());
    assert_eq!(default.expiry_days, 7);

    let start = admin
        .list_changes(None, Some(200), None)
        .await
        .expect("log");

    // Members read but do not change the expiry.
    assert!(matches!(
        member.update_draft_settings(change(30, 0), context()).await,
        Err(ObjectError::PermissionDenied)
    ));
    for days in [0, 91, -1] {
        match admin
            .update_draft_settings(change(days, 0), context())
            .await
        {
            Err(ObjectError::Validation(issues)) => {
                assert_eq!(issues.len(), 1);
                assert_eq!(issues[0].field, "expiry_days");
                assert_eq!(issues[0].code, "drafts.expiry_invalid");
            }
            other => panic!("{days} days: {other:?}"),
        }
    }
    assert!(matches!(
        admin.update_draft_settings(change(30, 3), context()).await,
        Err(ObjectError::VersionConflict)
    ));

    let first = admin
        .update_draft_settings(change(30, 0), context())
        .await
        .expect("first change");
    assert_eq!(first.expiry_days, 30);
    assert_eq!(first.version, 1);
    assert_eq!(first.updated_by, Some(administrator));
    assert!(first.updated_at.is_some());
    assert_eq!(member.draft_settings().await.expect("read"), first);

    // A second administrator who read the default cannot overwrite it.
    assert!(matches!(
        admin.update_draft_settings(change(14, 0), context()).await,
        Err(ObjectError::VersionConflict)
    ));
    for days in [DRAFT_EXPIRY_DAYS_MIN, DRAFT_EXPIRY_DAYS_MAX] {
        let version = admin.draft_settings().await.expect("read").version;
        let changed = admin
            .update_draft_settings(change(days, version), context())
            .await
            .expect("a bound");
        assert_eq!(changed.expiry_days, days);
        assert_eq!(changed.version, version + 1);
    }

    // The setting moves the metadata version and leaves no record change:
    // a draft setting is metadata, and drafts are never record changes.
    let after = admin
        .list_changes(Some(start.head), Some(200), None)
        .await
        .expect("log after");
    assert!(after.changes.is_empty(), "no record change");
    assert_eq!(after.head, start.head);
    assert_eq!(after.metadata_version, start.metadata_version + 3);

    // Other workspaces keep their own.
    let elsewhere = service(
        &store,
        caller(Uuid::now_v7(), Uuid::now_v7(), &ADMIN_PERMISSIONS, &[]),
    );
    assert_eq!(
        elsewhere.draft_settings().await.expect("another workspace"),
        DraftSettings::default()
    );
    3
}
