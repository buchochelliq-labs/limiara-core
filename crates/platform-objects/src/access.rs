//! Who is calling and what they may do with each object (ADR 0015 section 4).
//!
//! System permissions come from the kernel RBAC; object permissions are rows
//! per role in the engine's own tables, because objects are created at
//! runtime. Record-level sharing narrows reads and writes further (ADR 0016).

use crate::field_security::{FieldAccess, FieldSecurity};
use crate::{
    AccessFlags, FieldDefinition, ObjectError, OBJECTS_MANAGE_PERMISSION,
    RECORDS_MODIFY_ALL_PERMISSION, RECORDS_VIEW_ALL_PERMISSION,
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
    /// The plugin whose installation this caller performs (ADR 0033 §2).
    installer: Option<String>,
    /// The platform itself, acting for no member (the recycle-bin purge).
    system: bool,
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
            installer: None,
            system: false,
        }
    }

    /// The platform itself in one workspace, acting for no member: the
    /// recycle-bin purge (ADR 0035 G2). It holds no permissions and no
    /// roles, so it can read nothing through the member paths; it is used
    /// only by engine jobs that work on the store directly. Its changes name
    /// no actor in history, audit or events. Built by the kernel's jobs,
    /// never from request input.
    pub fn system(tenant_id: Uuid) -> Self {
        Self {
            tenant_id,
            user_id: Uuid::nil(),
            system_permissions: BTreeSet::new(),
            role_ids: Vec::new(),
            reports: Vec::new(),
            installer: None,
            system: true,
        }
    }

    /// Who the evidence of this caller's changes names: the member, or
    /// nobody for the platform itself.
    pub fn actor(&self) -> Option<Uuid> {
        (!self.system).then_some(self.user_id)
    }

    /// The kernel installing, upgrading, suspending or resuming a plugin's
    /// declared objects in one workspace (ADR 0033 §2). It holds no member
    /// permissions: it may change only the definitions `plugin_id` owns,
    /// through the plugin installation entry points of the engine. `actor_id`
    /// is the member who changed the plugin's state, so evidence names who
    /// asked as well as which plugin acted. Built by the kernel after it has
    /// authorised that member's plugin transition; never from request input.
    pub fn plugin_installer(tenant_id: Uuid, actor_id: Uuid, plugin_id: impl Into<String>) -> Self {
        Self {
            tenant_id,
            user_id: actor_id,
            system_permissions: BTreeSet::new(),
            role_ids: Vec::new(),
            reports: Vec::new(),
            installer: Some(plugin_id.into()),
            system: false,
        }
    }

    /// The plugin this caller installs, if it is a plugin installer.
    pub fn installer(&self) -> Option<&str> {
        self.installer.as_deref()
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

    /// The caller's system permissions in the workspace.
    pub fn permissions(&self) -> &BTreeSet<String> {
        &self.system_permissions
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
    /// What the caller may do with each field (G5).
    fields: FieldSecurity,
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
            fields: FieldSecurity::for_caller(caller),
        }
    }

    /// Field-level security as this access applies it.
    pub fn fields(&self) -> &FieldSecurity {
        &self.fields
    }

    /// What the caller may do with one field (G5).
    pub fn field(&self, field: &FieldDefinition) -> FieldAccess {
        self.fields.field(field)
    }

    /// The same object access with every field readable and writable, for
    /// conditions an administrator defined that the engine evaluates for the
    /// caller, such as lookup filters: like validation rules, they read
    /// every field, and only their outcome reaches the caller.
    pub(crate) fn with_system_fields(&self) -> Self {
        Self {
            fields: FieldSecurity::system(),
            ..self.clone()
        }
    }

    /// Read access to every object and field, for the engine checking a
    /// query a plugin declares (ADR 0037 §5) against the workspace's
    /// definitions. Only the query's columns reach the caller, never rows.
    pub(crate) fn reading_everything() -> Self {
        Self {
            system: AccessFlags {
                read: true,
                view_all: true,
                ..AccessFlags::default()
            },
            fields: FieldSecurity::system(),
            ..Self::default()
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

    /// Whether a role grants the caller customising any object (G10).
    pub fn customises_any(&self) -> bool {
        self.by_object.values().any(|flags| flags.customise)
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
