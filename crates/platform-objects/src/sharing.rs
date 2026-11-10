//! Record ownership and sharing (ADR 0016).
//!
//! Access to one record is the union of view all / modify all, ownership
//! (the caller or anyone below them in the reporting line), the object's
//! organisation-wide default and manual shares, always capped by the
//! caller's object permissions. Visibility is evaluated at query time by the
//! store, so nothing is materialised and nothing needs recalculating.

use crate::{
    AccessFlags, ObjectCaller, ObjectDefinition, SharingModel, RECORDS_TRANSFER_PERMISSION,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a manual share grants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareAccess {
    Read,
    Edit,
}

impl ShareAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Edit => "edit",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "edit" => Some(Self::Edit),
            _ => None,
        }
    }
}

/// Who a manual share is for: one member, or every holder of a role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GranteeKind {
    User,
    Role,
}

impl GranteeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Role => "role",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "role" => Some(Self::Role),
            _ => None,
        }
    }
}

/// A manual share of one record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RecordShare {
    pub grantee_kind: GranteeKind,
    pub grantee_id: Uuid,
    pub access: ShareAccess,
    pub created_at: DateTime<Utc>,
    pub created_by: Option<Uuid>,
}

/// The rows a caller may see when the object's default does not already
/// show them everything: records owned by `owners`, or shared with `user_id`
/// or one of `role_ids`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Visibility {
    pub user_id: Uuid,
    pub owners: Vec<Uuid>,
    pub role_ids: Vec<Uuid>,
    /// For a detail object (ADR 0025): the master-detail field whose master
    /// the rule above applies to, instead of the row itself.
    pub master: Option<Uuid>,
}

/// What the caller may do with one record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RecordRights {
    pub read: bool,
    pub edit: bool,
    pub delete: bool,
    /// Grant and revoke manual shares.
    pub share: bool,
    /// Change the owner.
    pub transfer: bool,
}

/// Whether the caller owns a record directly or through their reports.
/// A record without an owner belongs to no one.
pub fn owns(caller: &ObjectCaller, object: &ObjectDefinition, owner_id: Option<Uuid>) -> bool {
    owner_id.is_some_and(|owner| {
        owner == caller.user_id()
            || (object.grant_hierarchy && caller.reports().binary_search(&owner).is_ok())
    })
}

/// The visibility predicate for lists, or `None` when every record of the
/// object is visible to the caller.
pub fn visibility(
    caller: &ObjectCaller,
    flags: AccessFlags,
    object: &ObjectDefinition,
) -> Option<Visibility> {
    if flags.view_all || object.sharing_model != SharingModel::Private {
        return None;
    }
    let mut owners = vec![caller.user_id()];
    if object.grant_hierarchy {
        owners.extend_from_slice(caller.reports());
    }
    owners.sort_unstable();
    owners.dedup();
    Some(Visibility {
        user_id: caller.user_id(),
        owners,
        role_ids: caller.role_ids().to_vec(),
        master: None,
    })
}

/// The visibility predicate of a detail object whose primary master-detail
/// field is `field_id`: a detail is visible when its master is (ADR 0025).
/// `master_flags` and `master` are the master object's.
pub fn detail_visibility(
    caller: &ObjectCaller,
    flags: AccessFlags,
    field_id: Uuid,
    master_flags: AccessFlags,
    master: &ObjectDefinition,
) -> Option<Visibility> {
    if flags.view_all {
        return None;
    }
    visibility(caller, master_flags, master).map(|rule| Visibility {
        master: Some(field_id),
        ..rule
    })
}

/// What the caller may do with a detail record, from their rights on its
/// master (ADR 0025): reading needs read on the master, and creating,
/// editing and deleting need edit on it. Details have no shares and no
/// owner of their own to transfer.
pub fn detail_rights(flags: AccessFlags, master: RecordRights) -> RecordRights {
    let read = flags.read && (flags.view_all || master.read);
    let edit = read && flags.edit && (flags.modify_all || master.edit);
    let delete = read && flags.delete && (flags.modify_all || master.edit);
    RecordRights {
        read,
        edit,
        delete,
        share: false,
        transfer: false,
    }
}

/// What the caller may do with one record, given the strongest manual share
/// they hold on it.
pub fn rights(
    caller: &ObjectCaller,
    flags: AccessFlags,
    object: &ObjectDefinition,
    owner_id: Option<Uuid>,
    share: Option<ShareAccess>,
) -> RecordRights {
    let owned = owns(caller, object, owner_id);
    let read = flags.read
        && (flags.view_all
            || owned
            || object.sharing_model != SharingModel::Private
            || share.is_some());
    let edit = read
        && flags.edit
        && (flags.modify_all
            || owned
            || object.sharing_model == SharingModel::ReadWrite
            || share == Some(ShareAccess::Edit));
    let delete = read && flags.delete && (flags.modify_all || owned);
    let share_right = read && (flags.modify_all || owned);
    let transfer = edit
        && (flags.modify_all
            || owner_id == Some(caller.user_id())
            || caller.has(RECORDS_TRANSFER_PERMISSION));
    RecordRights {
        read,
        edit,
        delete,
        share: share_right,
        transfer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefinitionStatus, Origin};
    use std::collections::BTreeSet;

    fn object(sharing_model: SharingModel, grant_hierarchy: bool) -> ObjectDefinition {
        ObjectDefinition {
            id: Uuid::nil(),
            api_name: "custom:deal".into(),
            label: "Deal".into(),
            plural_label: "Deals".into(),
            description: None,
            origin: Origin::Workspace,
            owner_plugin: None,
            key_prefix: "000".into(),
            name_label: "Deal name".into(),
            sharing_model,
            grant_hierarchy,
            track_history: false,
            searchable: true,
            in_navigation: true,
            status: DefinitionStatus::Active,
            suspended: false,
            version: 1,
            created_at: Utc::now(),
            created_by: None,
            updated_at: Utc::now(),
            updated_by: None,
            fields: Vec::new(),
        }
    }

    const ME: Uuid = Uuid::from_u128(1);
    const REPORT: Uuid = Uuid::from_u128(2);
    const OTHER: Uuid = Uuid::from_u128(3);

    fn caller(permissions: &[&str]) -> ObjectCaller {
        ObjectCaller::new(
            Uuid::nil(),
            ME,
            permissions
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<BTreeSet<_>>(),
            vec![Uuid::from_u128(9)],
        )
        .with_reports(vec![REPORT, ME])
    }

    const MEMBER: AccessFlags = AccessFlags {
        read: true,
        create: true,
        edit: true,
        delete: true,
        view_all: false,
        modify_all: false,
    };

    #[test]
    fn private_records_belong_to_owners_their_managers_and_grantees() {
        let deal = object(SharingModel::Private, true);
        let caller = caller(&[]);
        let own = rights(&caller, MEMBER, &deal, Some(ME), None);
        assert!(own.read && own.edit && own.delete && own.share && own.transfer);
        let report = rights(&caller, MEMBER, &deal, Some(REPORT), None);
        assert!(report.read && report.edit && report.delete && report.share);
        assert!(
            !report.transfer,
            "moving a report's record needs records.transfer"
        );
        assert_eq!(
            rights(&caller, MEMBER, &deal, Some(OTHER), None),
            RecordRights::default()
        );
        let shared = rights(&caller, MEMBER, &deal, Some(OTHER), Some(ShareAccess::Read));
        assert!(shared.read && !shared.edit && !shared.delete && !shared.share);
        let editable = rights(&caller, MEMBER, &deal, Some(OTHER), Some(ShareAccess::Edit));
        assert!(
            editable.edit && !editable.delete,
            "an edit share never grants delete"
        );
        let visibility = visibility(&caller, MEMBER, &deal).expect("private");
        assert_eq!(visibility.owners, vec![ME, REPORT]);
    }

    #[test]
    fn the_hierarchy_can_be_switched_off_per_object() {
        let deal = object(SharingModel::Private, false);
        let caller = caller(&[]);
        assert!(!rights(&caller, MEMBER, &deal, Some(REPORT), None).read);
        assert_eq!(
            visibility(&caller, MEMBER, &deal).expect("private").owners,
            vec![ME]
        );
    }

    #[test]
    fn defaults_open_records_and_object_permissions_cap_them() {
        let caller = caller(&[]);
        let read_only = rights(
            &caller,
            MEMBER,
            &object(SharingModel::Read, true),
            Some(OTHER),
            None,
        );
        assert!(read_only.read && !read_only.edit);
        let open = rights(
            &caller,
            MEMBER,
            &object(SharingModel::ReadWrite, true),
            Some(OTHER),
            None,
        );
        assert!(open.read && open.edit && !open.delete && !open.share && !open.transfer);
        let reader = AccessFlags {
            read: true,
            ..AccessFlags::default()
        };
        let capped = rights(
            &caller,
            reader,
            &object(SharingModel::ReadWrite, true),
            Some(ME),
            Some(ShareAccess::Edit),
        );
        assert!(
            capped.read && !capped.edit && !capped.delete,
            "object permissions cap sharing"
        );
        assert!(visibility(&caller, MEMBER, &object(SharingModel::Read, true)).is_none());
    }

    #[test]
    fn details_follow_their_master() {
        let master = RecordRights {
            read: true,
            edit: false,
            delete: false,
            share: true,
            transfer: true,
        };
        let reader = detail_rights(MEMBER, master);
        assert!(reader.read && !reader.edit && !reader.delete);
        assert!(!reader.share && !reader.transfer, "details have no shares");
        let editor = detail_rights(
            MEMBER,
            RecordRights {
                edit: true,
                ..master
            },
        );
        assert!(
            editor.edit && editor.delete,
            "edit on the master deletes details"
        );
        assert_eq!(
            detail_rights(MEMBER, RecordRights::default()),
            RecordRights::default()
        );
        let admin = detail_rights(AccessFlags::FULL, RecordRights::default());
        assert!(admin.read && admin.edit && admin.delete);

        let deal = object(SharingModel::Private, true);
        let field = Uuid::from_u128(7);
        let rule =
            detail_visibility(&caller(&[]), MEMBER, field, MEMBER, &deal).expect("private master");
        assert_eq!(rule.master, Some(field));
        assert_eq!(rule.owners, vec![ME, REPORT]);
        assert!(detail_visibility(
            &caller(&[]),
            MEMBER,
            field,
            MEMBER,
            &object(SharingModel::Read, true)
        )
        .is_none());
    }

    #[test]
    fn view_all_and_modify_all_bypass_sharing() {
        let deal = object(SharingModel::Private, true);
        let caller = caller(&[]);
        let viewer = AccessFlags {
            view_all: true,
            ..MEMBER
        };
        let seen = rights(&caller, viewer, &deal, Some(OTHER), None);
        assert!(seen.read && !seen.edit);
        assert!(visibility(&caller, viewer, &deal).is_none());
        let full = rights(&caller, AccessFlags::FULL, &deal, Some(OTHER), None);
        assert!(full.read && full.edit && full.delete && full.share && full.transfer);
        let transferrer = super::rights(
            &self::caller(&[RECORDS_TRANSFER_PERMISSION]),
            MEMBER,
            &deal,
            Some(OTHER),
            Some(ShareAccess::Edit),
        );
        assert!(
            transferrer.transfer && !transferrer.share,
            "transfer permission moves records the caller can edit"
        );
    }
}
