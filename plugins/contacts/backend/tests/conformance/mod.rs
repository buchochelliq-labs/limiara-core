//! One behavioural suite every Contacts store must pass (ADR 0013). It drives
//! the real `ContactsService`, so it checks what callers rely on, not SQL.

#![allow(dead_code)]

use contacts_plugin::store::ContactsStore;
use contacts_plugin::{
    ContactCustomFieldTarget, ContactCustomFieldType, ContactSort, ContactsAuthorization,
    ContactsError, ContactsMutationContext, ContactsService, CreateCustomFieldDefinition,
    CreateOrganisation, CreatePerson, DataClassification, ListRequest, RecordStatus, SortDirection,
    UpdateOrganisation, UpdatePerson, ORGANISATIONS_READ_PERMISSION,
    ORGANISATIONS_WRITE_PERMISSION, PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION,
    PERSON_RESOURCE, SETTINGS_MANAGE_PERMISSION,
};
use platform_extensions::ExtensionCatalog;
use plugin_contract::Manifest;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;
use uuid::Uuid;

const LOYALTY: &str = r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.fields"
description = "Loyalty fields"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }, { value = "silver", label = "Silver" }]
read_permission = "loyalty.fields"
write_permission = "loyalty.fields"
"#;

/// What the suite wrote, so callers can check backend-specific evidence.
pub struct Report {
    pub tenant_a: Uuid,
    pub tenant_b: Uuid,
    /// Successful mutations in workspace A, each of which must have left
    /// exactly one audit row and one outbox row.
    pub mutations_in_a: i64,
}

fn service(store: &Arc<dyn ContactsStore>, tenant_id: Uuid) -> ContactsService {
    member_service(store, tenant_id, Uuid::now_v7())
}

fn member_service(
    store: &Arc<dyn ContactsStore>,
    tenant_id: Uuid,
    member: Uuid,
) -> ContactsService {
    ContactsService::with_store(
        store.clone(),
        ContactsAuthorization::new(
            tenant_id,
            member,
            [
                PEOPLE_READ_PERMISSION,
                PEOPLE_WRITE_PERMISSION,
                ORGANISATIONS_READ_PERMISSION,
                ORGANISATIONS_WRITE_PERMISSION,
                SETTINGS_MANAGE_PERMISSION,
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ),
    )
}

fn context() -> ContactsMutationContext {
    ContactsMutationContext::new(Uuid::now_v7())
}

fn person(name: &str, email: Option<&str>) -> CreatePerson {
    CreatePerson {
        full_name: name.into(),
        email: email.map(str::to_owned),
        telephone: None,
        organisation_id: None,
        tags: vec!["vip".into()],
        custom_fields: BTreeMap::new(),
    }
}

fn update(version: i64, name: &str) -> UpdatePerson {
    UpdatePerson {
        expected_version: version,
        full_name: name.into(),
        email: Some("Ada@Example.org".into()),
        telephone: None,
        organisation_id: None,
        tags: Vec::new(),
        custom_fields: BTreeMap::new(),
    }
}

fn list(limit: u16) -> ListRequest {
    ListRequest {
        limit,
        ..ListRequest::default()
    }
}

pub async fn run(store: Arc<dyn ContactsStore>) -> Report {
    let tenant_a = Uuid::now_v7();
    let tenant_b = Uuid::now_v7();
    let a = service(&store, tenant_a);
    let b = service(&store, tenant_b);
    let mut mutations = 0;

    // Create, read and update with optimistic versions.
    let ada = a
        .create_person(person("Ada Lovelace", Some("Ada@Example.org")), context())
        .await
        .expect("create");
    mutations += 1;
    assert!(ada.public_id.starts_with("per"));
    assert_eq!(ada.version, 1);
    assert_eq!(ada.tags, ["vip"]);
    assert_eq!(
        a.get_person(&ada.public_id).await.expect("read").full_name,
        "Ada Lovelace"
    );
    let renamed = a
        .update_person(&ada.public_id, update(1, "Ada King"), context())
        .await
        .expect("update");
    mutations += 1;
    assert_eq!(renamed.version, 2);
    assert!(renamed.updated_at >= ada.updated_at);
    assert!(matches!(
        a.update_person(&ada.public_id, update(1, "Stale"), context())
            .await,
        Err(ContactsError::VersionConflict)
    ));
    assert!(matches!(
        a.get_person("per_unknown").await,
        Err(ContactsError::NotFound)
    ));

    // Tenant isolation: another workspace sees nothing and changes nothing.
    assert!(matches!(
        b.get_person(&ada.public_id).await,
        Err(ContactsError::NotFound)
    ));
    assert!(b.list_people(list(50)).await.expect("list").data.is_empty());
    assert!(matches!(
        b.update_person(&ada.public_id, update(2, "Intruder"), context())
            .await,
        Err(ContactsError::NotFound)
    ));
    let other = b
        .create_person(person("Grace Hopper", None), context())
        .await
        .expect("create in b");
    assert_eq!(
        other.public_id, ada.public_id,
        "public ids are numbered per workspace"
    );

    // Lists: search, email, sorting and keyset pagination with cursors.
    for name in [
        "Émile Zola",
        "bob Builder",
        "Charles Babbage",
        "Dorothy Vaughan",
    ] {
        a.create_person(person(name, None), context())
            .await
            .expect("seed");
        mutations += 1;
    }
    let first = a.list_people(list(2)).await.expect("page one");
    let names = |page: &contacts_plugin::Page<contacts_plugin::Person>| {
        page.data
            .iter()
            .map(|person| person.full_name.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&first), ["Ada King", "bob Builder"]);
    let cursor = first.next_cursor.clone().expect("more pages");
    let second = a
        .list_people(ListRequest {
            cursor: Some(cursor.clone()),
            ..list(2)
        })
        .await
        .expect("page two");
    assert_eq!(names(&second), ["Charles Babbage", "Dorothy Vaughan"]);
    let third = a
        .list_people(ListRequest {
            cursor: second.next_cursor.clone(),
            ..list(2)
        })
        .await
        .expect("page three");
    assert_eq!(
        names(&third),
        ["Émile Zola"],
        "non-ASCII names sort by their lowercase key"
    );
    assert!(third.next_cursor.is_none());
    assert!(matches!(
        a.list_people(ListRequest {
            cursor: Some(cursor.clone()),
            search: Some("ada".into()),
            ..list(2)
        })
        .await,
        Err(ContactsError::InvalidCursor)
    ));
    assert!(matches!(
        b.list_people(ListRequest {
            cursor: Some(cursor),
            ..list(2)
        })
        .await,
        Err(ContactsError::InvalidCursor)
    ));
    let newest = a
        .list_people(ListRequest {
            sort: ContactSort::CreatedAt,
            direction: SortDirection::Desc,
            ..list(1)
        })
        .await
        .expect("newest");
    assert_eq!(names(&newest), ["Dorothy Vaughan"]);
    let searched = a
        .list_people(ListRequest {
            search: Some("ÉMILE".into()),
            ..list(10)
        })
        .await
        .expect("search");
    assert_eq!(names(&searched), ["Émile Zola"]);
    let by_email = a
        .list_people(ListRequest {
            email: Some("ada@example.org".into()),
            ..list(10)
        })
        .await
        .expect("email");
    assert_eq!(names(&by_email), ["Ada King"]);

    // Archiving and status filters.
    let bob = a
        .list_people(ListRequest {
            search: Some("bob".into()),
            ..list(1)
        })
        .await
        .expect("bob")
        .data
        .remove(0);
    let archived = a
        .archive_person(&bob.public_id, bob.version, context())
        .await
        .expect("archive");
    mutations += 1;
    assert!(archived.archived_at.is_some());
    assert_eq!(a.list_people(list(50)).await.expect("active").data.len(), 4);
    let archived_list = a
        .list_people(ListRequest {
            status: RecordStatus::Archived,
            ..list(50)
        })
        .await
        .expect("archived");
    assert_eq!(names(&archived_list), ["bob Builder"]);
    assert!(matches!(
        a.update_person(&bob.public_id, update(archived.version, "Bob"), context())
            .await,
        Err(ContactsError::VersionConflict)
    ));

    // Organisations and the organisation filter.
    let org = a
        .create_organisation(
            CreateOrganisation {
                name: "Analytical Engines".into(),
                email: None,
                telephone: None,
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("organisation");
    mutations += 1;
    assert!(org.public_id.starts_with("org"));
    let linked = a
        .update_person(
            &ada.public_id,
            UpdatePerson {
                organisation_id: Some(org.public_id.clone()),
                ..update(2, "Ada King")
            },
            context(),
        )
        .await
        .expect("link");
    mutations += 1;
    assert_eq!(
        linked.organisation_public_id.as_deref(),
        Some(org.public_id.as_str())
    );
    let employees = a
        .list_people(ListRequest {
            organisation_id: Some(org.public_id.clone()),
            ..list(10)
        })
        .await
        .expect("employees");
    assert_eq!(names(&employees), ["Ada King"]);
    assert_eq!(
        a.list_organisations(list(10))
            .await
            .expect("orgs")
            .data
            .len(),
        1
    );

    // Custom field definitions and tenant-unique values.
    let definition = a
        .create_definition(
            CreateCustomFieldDefinition {
                applies_to: ContactCustomFieldTarget::Person,
                api_name: "badge_number__c".into(),
                label: "Badge number".into(),
                description: None,
                field_type: ContactCustomFieldType::Text,
                required: false,
                unique_per_tenant: true,
                indexed: true,
                options: Vec::new(),
                validation_schema: json!({}),
                data_classification: DataClassification::Internal,
                purpose: Some("Building access".into()),
                retention_days: None,
                encrypted: false,
            },
            context(),
        )
        .await
        .expect("definition");
    mutations += 1;
    assert!(definition.unique_per_tenant);
    assert!(matches!(
        a.create_definition(
            CreateCustomFieldDefinition {
                api_name: "badge_number__c".into(),
                ..definition_input()
            },
            context()
        )
        .await,
        Err(ContactsError::Validation(issues)) if issues[0].code == "custom_field.api_name_conflict"
    ));
    let mut badge = BTreeMap::new();
    badge.insert("badge_number__c".to_owned(), Value::String("B-7".into()));
    a.create_person(
        CreatePerson {
            custom_fields: badge.clone(),
            ..person("Katherine Johnson", None)
        },
        context(),
    )
    .await
    .expect("first badge");
    mutations += 1;
    assert!(matches!(
        a.create_person(
            CreatePerson {
                custom_fields: badge.clone(),
                ..person("Mary Jackson", None)
            },
            context()
        )
        .await,
        Err(ContactsError::Validation(issues)) if issues[0].code == "custom_field.not_unique"
    ));
    b.create_definition(definition_input(), context())
        .await
        .expect("definition in b");
    b.create_person(
        CreatePerson {
            custom_fields: badge,
            ..person("Mary Jackson", None)
        },
        context(),
    )
    .await
    .expect("uniqueness is per workspace");
    let disabled = a
        .disable_definition(definition.id, definition.version, context())
        .await
        .expect("disable");
    mutations += 1;
    assert!(disabled.disabled_at.is_some());
    assert_eq!(
        a.list_definitions(ContactCustomFieldTarget::Person)
            .await
            .expect("definitions")
            .len(),
        1
    );

    // Extension values: stored with the record, versioned and filterable.
    let contacts = Manifest::parse_toml(include_str!("../../../plugin.toml")).expect("Contacts");
    let loyalty = Manifest::parse_toml(LOYALTY).expect("Loyalty");
    let catalog = ExtensionCatalog::from_manifests([&contacts, &loyalty]);
    let allow = |_: &str, _: &str| true;
    let patch = catalog
        .validate_patch(
            PERSON_RESOURCE,
            &BTreeMap::from([("example.loyalty.tier".to_owned(), json!("gold"))]),
            &allow,
        )
        .expect("patch");
    let current = a.get_person(&ada.public_id).await.expect("current");
    let tiered = a
        .update_person_extensions(&ada.public_id, current.version, &patch, context())
        .await
        .expect("extension write");
    mutations += 1;
    assert_eq!(tiered.extensions["example.loyalty"]["tier"], json!("gold"));
    assert_eq!(tiered.version, current.version + 1);
    let filter = catalog
        .validate_filter(PERSON_RESOURCE, "example.loyalty.tier", "gold", &allow)
        .expect("filter");
    let gold = a
        .list_people(ListRequest {
            extension_filter: Some(filter),
            ..list(10)
        })
        .await
        .expect("filtered");
    assert_eq!(names(&gold), ["Ada King"]);
    let edited = a
        .update_person_with_extensions(
            &ada.public_id,
            UpdatePerson {
                organisation_id: Some(org.public_id.clone()),
                ..update(tiered.version, "Ada Lovelace")
            },
            Some(
                &catalog
                    .validate_patch(
                        PERSON_RESOURCE,
                        &BTreeMap::from([("example.loyalty.tier".to_owned(), json!("silver"))]),
                        &allow,
                    )
                    .expect("patch"),
            ),
            context(),
        )
        .await
        .expect("form save");
    mutations += 1;
    assert_eq!(edited.full_name, "Ada Lovelace");
    assert_eq!(
        edited.extensions["example.loyalty"]["tier"],
        json!("silver")
    );
    assert_eq!(edited.version, tiered.version + 1);

    Report {
        tenant_a,
        tenant_b,
        mutations_in_a: mutations,
    }
}

fn definition_input() -> CreateCustomFieldDefinition {
    CreateCustomFieldDefinition {
        applies_to: ContactCustomFieldTarget::Person,
        api_name: "badge_number__c".into(),
        label: "Badge number".into(),
        description: None,
        field_type: ContactCustomFieldType::Text,
        required: false,
        unique_per_tenant: true,
        indexed: false,
        options: Vec::new(),
        validation_schema: json!({}),
        data_classification: DataClassification::Internal,
        purpose: Some("Building access".into()),
        retention_days: None,
        encrypted: false,
    }
}

/// The standard record stamps (ADR 0014): who created and last changed a
/// record, and `touched_at` never earlier than `updated_at`. Runs in its own
/// workspace so `run`'s evidence counts are unaffected.
pub async fn standard_fields(store: Arc<dyn ContactsStore>) -> Uuid {
    let tenant = Uuid::now_v7();
    let ada = Uuid::now_v7();
    let grace = Uuid::now_v7();
    let as_ada = member_service(&store, tenant, ada);
    let as_grace = member_service(&store, tenant, grace);

    let created = as_ada
        .create_person(person("Stamp Person", None), context())
        .await
        .expect("create");
    assert_eq!(created.created_by, Some(ada));
    assert_eq!(created.updated_by, Some(ada));
    assert_eq!(created.touched_at, created.updated_at);

    let updated = as_grace
        .update_person(
            &created.public_id,
            update(created.version, "Stamp Person"),
            context(),
        )
        .await
        .expect("update");
    assert_eq!(updated.created_by, Some(ada), "the creator never changes");
    assert_eq!(updated.created_at, created.created_at);
    assert_eq!(updated.updated_by, Some(grace));
    assert!(updated.updated_at >= created.updated_at);
    assert!(updated.touched_at >= updated.updated_at);

    let read = as_ada.get_person(&created.public_id).await.expect("read");
    assert_eq!(
        read.updated_by,
        Some(grace),
        "stamps are stored, not echoed"
    );
    let listed = as_ada.list_people(list(10)).await.expect("list");
    assert_eq!(listed.data[0].created_by, Some(ada));

    let archived = as_ada
        .archive_person(&created.public_id, updated.version, context())
        .await
        .expect("archive");
    assert_eq!(
        archived.updated_by,
        Some(ada),
        "archiving is a member's edit"
    );
    assert!(archived.archived_at.is_some());

    let organisation = as_grace
        .create_organisation(
            CreateOrganisation {
                name: "Stamp Org".into(),
                email: None,
                telephone: None,
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("organisation");
    assert_eq!(organisation.created_by, Some(grace));
    let renamed = as_ada
        .update_organisation(
            &organisation.public_id,
            UpdateOrganisation {
                expected_version: organisation.version,
                name: "Stamp Org Renamed".into(),
                email: None,
                telephone: None,
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("organisation update");
    assert_eq!(renamed.created_by, Some(grace));
    assert_eq!(renamed.updated_by, Some(ada));
    assert!(renamed.touched_at >= renamed.updated_at);
    tenant
}

/// The recycle bin (ADR 0014, #290): a deleted record is hidden from reads,
/// lists and writes, visible with `include_deleted` and `status=deleted`, and
/// restorable by undelete, in the deleting workspace only.
pub async fn recycle_bin(store: Arc<dyn ContactsStore>) {
    let tenant = Uuid::now_v7();
    let member = Uuid::now_v7();
    let a = member_service(&store, tenant, member);
    let b = service(&store, Uuid::now_v7());

    let organisation = a
        .create_organisation(
            CreateOrganisation {
                name: "Bin Org".into(),
                email: None,
                telephone: None,
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("organisation");
    let mut linked = person("Linked Person", None);
    linked.organisation_id = Some(organisation.public_id.clone());
    let linked = a.create_person(linked, context()).await.expect("linked");
    let created = a
        .create_person(person("Bin Person", None), context())
        .await
        .expect("person");
    assert_eq!(created.deleted_at, None);

    let deleted = a
        .delete_person(&created.public_id, created.version, context())
        .await
        .expect("delete");
    assert!(deleted.deleted_at.is_some());
    assert_eq!(deleted.version, created.version + 1);
    assert_eq!(
        deleted.updated_by,
        Some(member),
        "deleting is a member's edit"
    );
    assert!(deleted.touched_at >= deleted.updated_at);

    // Hidden from ordinary reads, lists and writes.
    assert!(matches!(
        a.get_person(&created.public_id).await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        a.update_person(
            &created.public_id,
            update(deleted.version, "Bin Person"),
            context()
        )
        .await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        a.archive_person(&created.public_id, deleted.version, context())
            .await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        a.delete_person(&created.public_id, deleted.version, context())
            .await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        a.person_activity(&created.public_id).await,
        Err(ContactsError::NotFound)
    ));
    for status in [
        RecordStatus::Active,
        RecordStatus::All,
        RecordStatus::Archived,
    ] {
        let page = a
            .list_people(ListRequest { status, ..list(50) })
            .await
            .expect("list");
        assert!(
            page.data.iter().all(|p| p.public_id != created.public_id),
            "{status:?} must not list deleted records"
        );
    }

    // Visible on request, and in the recycle bin listing only.
    let read = a
        .read_person(&created.public_id, true)
        .await
        .expect("include_deleted read");
    assert_eq!(read.deleted_at, deleted.deleted_at);
    let bin = a
        .list_people(ListRequest {
            status: RecordStatus::Deleted,
            ..list(50)
        })
        .await
        .expect("recycle bin");
    assert_eq!(
        bin.data
            .iter()
            .map(|p| p.public_id.as_str())
            .collect::<Vec<_>>(),
        [created.public_id.as_str()]
    );

    // Another workspace can neither see nor restore it.
    assert!(matches!(
        b.read_person(&created.public_id, true).await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        b.undelete_person(&created.public_id, deleted.version, context())
            .await,
        Err(ContactsError::NotFound)
    ));

    // Undelete checks the version and restores the record.
    assert!(matches!(
        a.undelete_person(&created.public_id, created.version, context())
            .await,
        Err(ContactsError::VersionConflict)
    ));
    let restored = a
        .undelete_person(&created.public_id, deleted.version, context())
        .await
        .expect("undelete");
    assert_eq!(restored.deleted_at, None);
    assert_eq!(restored.version, deleted.version + 1);
    assert!(matches!(
        a.undelete_person(&created.public_id, restored.version, context())
            .await,
        Err(ContactsError::NotFound)
    ));
    a.get_person(&created.public_id)
        .await
        .expect("readable again");

    // A deleted organisation keeps existing links but accepts no new ones.
    let gone = a
        .delete_organisation(&organisation.public_id, organisation.version, context())
        .await
        .expect("delete organisation");
    assert!(gone.deleted_at.is_some());
    let still_linked = a.get_person(&linked.public_id).await.expect("linked");
    assert_eq!(
        still_linked.organisation_public_id.as_deref(),
        Some(organisation.public_id.as_str())
    );
    let mut new_link = person("New Link", None);
    new_link.organisation_id = Some(organisation.public_id.clone());
    assert!(a.create_person(new_link, context()).await.is_err());
    let back = a
        .undelete_organisation(&organisation.public_id, gone.version, context())
        .await
        .expect("undelete organisation");
    assert_eq!(back.deleted_at, None);
}
