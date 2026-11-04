//! Who is calling and what they may do with each object (ADR 0015 section 4).
//!
//! System permissions come from the kernel RBAC; object permissions are rows
//! per role in the engine's own tables, because objects are created at
//! runtime. Record-level sharing narrows reads and writes further (ADR 0016).

use crate::{
    AccessFlags, ObjectError, OBJECTS_MANAGE_PERMISSION, RECORDS_MODIFY_ALL_PERMISSION,
    RECORDS_VIEW_ALL_PERMISSION,
};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

/// The authenticated member and their authority in one workspace. Built by
/// the kernel after membership is verified; never from request input.
#[derive(Debug, Clone)]
pub struct ObjectCaller {
    tenant_id: Uuid,
    user_id: Uuid,
    system_permissions: BTreeSet<String>,
    role_ids: Vec<Uuid>,
    /// Members below the caller in the reporting line (ADR 0016).
    reports: Vec<Uuid>,
}

impl ObjectCaller {
    pub fn new(
        tenant_id: Uuid,
        user_id: Uuid,
        system_permissions: BTreeSet<String>,
        role_ids: Vec<Uuid>,
    ) -> Self {
        Self {
            tenant_id,
            user_id,
            system_permissions,
            role_ids,
            reports: Vec::new(),
        }
    }

    /// The members below the caller in the reporting line, resolved by the
    /// kernel from verified membership data.
    pub fn with_reports(mut self, mut reports: Vec<Uuid>) -> Self {
        reports.retain(|report| *report != self.user_id);
        reports.sort_unstable();
        reports.dedup();
        self.reports = reports;
        self
    }

    pub fn reports(&self) -> &[Uuid] {
        &self.reports
    }

    pub fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    pub fn user_id(&self) -> Uuid {
        self.user_id
    }

    pub fn role_ids(&self) -> &[Uuid] {
        &self.role_ids
    }

    pub fn has(&self, permission: &str) -> bool {
        self.system_permissions.contains(permission)
    }

    pub fn can_manage(&self) -> bool {
        self.has(OBJECTS_MANAGE_PERMISSION)
    }

    pub fn require_manage(&self) -> Result<(), ObjectError> {
        if self.can_manage() {
            Ok(())
        } else {
            Err(ObjectError::PermissionDenied)
        }
    }

    /// Access from system permissions alone.
    fn system_access(&self) -> AccessFlags {
        if self.has(RECORDS_MODIFY_ALL_PERMISSION) {
            AccessFlags::FULL
        } else if self.has(RECORDS_VIEW_ALL_PERMISSION) {
            AccessFlags {
                read: true,
                view_all: true,
                ..AccessFlags::default()
            }
        } else {
            AccessFlags::default()
        }
    }
}

/// The caller's effective access to every object, combining role grants with
/// system permissions.
#[derive(Debug, Clone, Default)]
pub struct ObjectAccess {
    system: AccessFlags,
    by_object: BTreeMap<Uuid, AccessFlags>,
    /// Each detail object's masters (ADR 0025).
    masters: BTreeMap<Uuid, Vec<Uuid>>,
}

impl ObjectAccess {
    pub fn resolve(caller: &ObjectCaller, grants: BTreeMap<Uuid, AccessFlags>) -> Self {
        Self {
            system: caller.system_access(),
            by_object: grants
                .into_iter()
                .map(|(object, flags)| (object, flags.normalised()))
                .collect(),
            masters: BTreeMap::new(),
        }
    }

    /// Detail objects are only accessible with read access to every one of
    /// their masters (ADR 0025). `links` are `(detail, master)` pairs.
    pub fn with_masters(mut self, links: impl IntoIterator<Item = (Uuid, Uuid)>) -> Self {
        for (detail, master) in links {
            self.masters.entry(detail).or_default().push(master);
        }
        self
    }

    fn own(&self, object_id: Uuid) -> AccessFlags {
        self.by_object
            .get(&object_id)
            .copied()
            .unwrap_or_default()
            .union(self.system)
    }

    pub fn for_object(&self, object_id: Uuid) -> AccessFlags {
        let flags = self.own(object_id);
        // Masters are never details themselves, so one level is enough.
        match self.masters.get(&object_id) {
            Some(masters) if masters.iter().any(|master| !self.own(*master).read) => {
                AccessFlags::default()
            }
            _ => flags,
        }
    }

    pub fn require(
        &self,
        object_id: Uuid,
        check: impl Fn(AccessFlags) -> bool,
    ) -> Result<AccessFlags, ObjectError> {
        let flags = self.for_object(object_id);
        if check(flags) {
            Ok(flags)
        } else {
            Err(ObjectError::PermissionDenied)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(permissions: &[&str]) -> ObjectCaller {
        ObjectCaller::new(
            Uuid::nil(),
            Uuid::nil(),
            permissions
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            Vec::new(),
        )
    }

    #[test]
    fn grants_imply_weaker_access_and_system_permissions_widen_it() {
        let object = Uuid::from_u128(1);
        let other = Uuid::from_u128(2);
        let grants = BTreeMap::from([(
            object,
            AccessFlags {
                delete: true,
                ..AccessFlags::default()
            },
        )]);
        let access = ObjectAccess::resolve(&caller(&[]), grants.clone());
        let flags = access.for_object(object);
        assert!(flags.read && flags.edit && flags.delete && !flags.create);
        assert_eq!(access.for_object(other), AccessFlags::default());

        let viewer = ObjectAccess::resolve(&caller(&[RECORDS_VIEW_ALL_PERMISSION]), grants.clone());
        assert!(viewer.for_object(other).read && !viewer.for_object(other).edit);
        let admin = ObjectAccess::resolve(&caller(&[RECORDS_MODIFY_ALL_PERMISSION]), grants);
        assert_eq!(admin.for_object(other), AccessFlags::FULL);
    }

    #[test]
    fn details_need_read_access_to_every_master() {
        let detail = Uuid::from_u128(1);
        let master = Uuid::from_u128(2);
        let other_master = Uuid::from_u128(3);
        let grants = BTreeMap::from([
            (detail, AccessFlags::FULL),
            (
                master,
                AccessFlags {
                    read: true,
                    ..AccessFlags::default()
                },
            ),
        ]);
        let access =
            ObjectAccess::resolve(&caller(&[]), grants.clone()).with_masters([(detail, master)]);
        assert_eq!(access.for_object(detail), AccessFlags::FULL);
        let junction = ObjectAccess::resolve(&caller(&[]), grants)
            .with_masters([(detail, master), (detail, other_master)]);
        assert_eq!(junction.for_object(detail), AccessFlags::default());
    }
}
