use crate::ContactsError;
use platform_permissions::{PolicyEngine, Principal, RbacPolicyEngine};
use std::collections::BTreeSet;
use uuid::Uuid;

pub const PEOPLE_READ_PERMISSION: &str = "contacts.people.read";
pub const PEOPLE_WRITE_PERMISSION: &str = "contacts.people.write";
pub const ORGANISATIONS_READ_PERMISSION: &str = "contacts.organisations.read";
pub const ORGANISATIONS_WRITE_PERMISSION: &str = "contacts.organisations.write";
pub const SETTINGS_MANAGE_PERMISSION: &str = "contacts.settings.manage";

/// Membership-derived authority for one Contacts workspace.
///
/// The fields stay private so service callers must construct and pass an
/// explicit context rather than supplying an unscoped tenant identifier to
/// individual operations.
#[derive(Clone, Debug)]
pub struct ContactsAuthorization {
    tenant_id: Uuid,
    actor_id: Uuid,
    principal: Principal,
}

impl ContactsAuthorization {
    pub fn new(tenant_id: Uuid, actor_id: Uuid, permissions: BTreeSet<String>) -> Self {
        Self {
            tenant_id,
            actor_id,
            principal: Principal {
                user_id: actor_id.to_string(),
                permissions,
            },
        }
    }

    /// The member every write in this context is stamped with (ADR 0014).
    pub(crate) fn actor_id(&self) -> Uuid {
        self.actor_id
    }

    pub(crate) fn authorize(
        &self,
        operation: ContactsOperation,
    ) -> Result<AuthorizedContacts, ContactsError> {
        RbacPolicyEngine
            .require(&self.principal, operation.required_permission())
            .map(|()| AuthorizedContacts {
                tenant_id: self.tenant_id,
                actor_id: self.actor_id,
            })
            .map_err(|_| ContactsError::PermissionDenied)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AuthorizedContacts {
    pub(crate) tenant_id: Uuid,
    pub(crate) actor_id: Uuid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ContactsOperation {
    ListPeople,
    CreatePerson,
    GetPerson,
    UpdatePerson,
    ArchivePerson,
    /// Moving into or out of the recycle bin (ADR 0014).
    DeletePerson,
    UpdatePersonExtensions,
    ListOrganisations,
    CreateOrganisation,
    GetOrganisation,
    UpdateOrganisation,
    ArchiveOrganisation,
    DeleteOrganisation,
    UpdateOrganisationExtensions,
    ListDefinitions,
    CreateDefinition,
    GetDefinition,
    UpdateDefinition,
    DisableDefinition,
    ArchiveDefinition,
}

impl ContactsOperation {
    pub(crate) const fn required_permission(self) -> &'static str {
        match self {
            // Extension values carry their own write permission, checked by
            // the kernel extension service; the host requires only that the
            // caller can see the record.
            Self::ListPeople | Self::GetPerson | Self::UpdatePersonExtensions => {
                PEOPLE_READ_PERMISSION
            }
            Self::CreatePerson | Self::UpdatePerson | Self::ArchivePerson | Self::DeletePerson => {
                PEOPLE_WRITE_PERMISSION
            }
            Self::ListOrganisations
            | Self::GetOrganisation
            | Self::UpdateOrganisationExtensions => ORGANISATIONS_READ_PERMISSION,
            Self::CreateOrganisation
            | Self::UpdateOrganisation
            | Self::ArchiveOrganisation
            | Self::DeleteOrganisation => ORGANISATIONS_WRITE_PERMISSION,
            Self::ListDefinitions
            | Self::CreateDefinition
            | Self::GetDefinition
            | Self::UpdateDefinition
            | Self::DisableDefinition
            | Self::ArchiveDefinition => SETTINGS_MANAGE_PERMISSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_service_operation_maps_to_a_declared_manifest_permission() {
        let cases = [
            (ContactsOperation::ListPeople, PEOPLE_READ_PERMISSION),
            (ContactsOperation::CreatePerson, PEOPLE_WRITE_PERMISSION),
            (ContactsOperation::GetPerson, PEOPLE_READ_PERMISSION),
            (ContactsOperation::UpdatePerson, PEOPLE_WRITE_PERMISSION),
            (ContactsOperation::ArchivePerson, PEOPLE_WRITE_PERMISSION),
            (ContactsOperation::DeletePerson, PEOPLE_WRITE_PERMISSION),
            (
                ContactsOperation::DeleteOrganisation,
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                ContactsOperation::UpdatePersonExtensions,
                PEOPLE_READ_PERMISSION,
            ),
            (
                ContactsOperation::UpdateOrganisationExtensions,
                ORGANISATIONS_READ_PERMISSION,
            ),
            (
                ContactsOperation::ListOrganisations,
                ORGANISATIONS_READ_PERMISSION,
            ),
            (
                ContactsOperation::CreateOrganisation,
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                ContactsOperation::GetOrganisation,
                ORGANISATIONS_READ_PERMISSION,
            ),
            (
                ContactsOperation::UpdateOrganisation,
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                ContactsOperation::ArchiveOrganisation,
                ORGANISATIONS_WRITE_PERMISSION,
            ),
            (
                ContactsOperation::ListDefinitions,
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                ContactsOperation::CreateDefinition,
                SETTINGS_MANAGE_PERMISSION,
            ),
            (ContactsOperation::GetDefinition, SETTINGS_MANAGE_PERMISSION),
            (
                ContactsOperation::UpdateDefinition,
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                ContactsOperation::DisableDefinition,
                SETTINGS_MANAGE_PERMISSION,
            ),
            (
                ContactsOperation::ArchiveDefinition,
                SETTINGS_MANAGE_PERMISSION,
            ),
        ];
        let manifest = include_str!("../../plugin.toml");

        for (operation, permission) in cases {
            assert_eq!(operation.required_permission(), permission);
            assert!(
                manifest.contains(&format!("id = \"{permission}\"")),
                "{permission} must be declared by the Contacts manifest"
            );
        }
    }

    #[test]
    fn authorization_is_exact_and_tenant_scoped() {
        let tenant_id = Uuid::from_u128(1);
        let authorization = ContactsAuthorization::new(
            tenant_id,
            Uuid::from_u128(2),
            BTreeSet::from([PEOPLE_READ_PERMISSION.to_string()]),
        );

        assert!(matches!(
            authorization.authorize(ContactsOperation::GetPerson),
            Ok(authorized) if authorized.tenant_id == tenant_id
                && authorized.actor_id == Uuid::from_u128(2)
        ));
        assert!(matches!(
            authorization.authorize(ContactsOperation::CreatePerson),
            Err(ContactsError::PermissionDenied)
        ));
        assert!(matches!(
            ContactsAuthorization::new(tenant_id, Uuid::from_u128(2), BTreeSet::new(),)
                .authorize(ContactsOperation::GetPerson),
            Err(ContactsError::PermissionDenied)
        ));
    }
}
