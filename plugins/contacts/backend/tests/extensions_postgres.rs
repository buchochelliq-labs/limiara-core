//! Real-PostgreSQL acceptance for Contacts as an extension host (ADR 0012):
//! kernel-validated extension values are stored with the record under forced
//! RLS, versioned, audited and filterable, and never cross tenants. Run with
//! `bash scripts/test-plugin-runtime.sh`.

mod conformance;

use contacts_plugin::store::{ContactsStore, PostgresContactsStore};
use contacts_plugin::{
    ContactsAuthorization, ContactsError, ContactsMutationContext, ContactsService,
    CreateOrganisation, CreatePerson, ListRequest, ORGANISATIONS_READ_PERMISSION,
    ORGANISATIONS_WRITE_PERMISSION, ORGANISATION_RESOURCE, PEOPLE_READ_PERMISSION,
    PEOPLE_WRITE_PERMISSION, PERSON_RESOURCE,
};
use platform_extensions::{ExtensionCatalog, ValidatedExtensionPatch};
use plugin_contract::Manifest;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::collections::BTreeMap;
use uuid::Uuid;

const TENANT_A: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000001);
const TENANT_B: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000002);
const OPERATOR: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000104);

const LOYALTY: &str = r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty tiers"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.read"
description = "Read loyalty fields"
[[permissions]]
id = "loyalty.write"
description = "Change loyalty fields"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Loyalty tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }, { value = "silver", label = "Silver" }]
read_permission = "loyalty.read"
write_permission = "loyalty.write"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.organisation"
key = "account_code"
label = "Account code"
field_type = "text"
read_permission = "loyalty.read"
write_permission = "loyalty.write"
"#;

fn catalog() -> ExtensionCatalog {
    let contacts = Manifest::parse_toml(include_str!("../../plugin.toml")).expect("Contacts");
    let loyalty = Manifest::parse_toml(LOYALTY).expect("Loyalty");
    ExtensionCatalog::from_manifests([&contacts, &loyalty])
}

fn writer(plugin: &str, permission: &str) -> bool {
    plugin == "example.loyalty" && permission.starts_with("loyalty.")
}

fn patch(target: &str, entries: &[(&str, Value)]) -> ValidatedExtensionPatch {
    let values = entries
        .iter()
        .map(|(key, value)| ((*key).to_owned(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    catalog()
        .validate_patch(target, &values, &writer)
        .expect("kernel-valid patch")
}

async fn service(tenant_id: Uuid, permissions: &[&str]) -> ContactsService {
    let url = std::env::var("PLATFORM_TEST_DATABASE_URL").expect("PLATFORM_TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    ContactsService::new(
        pool,
        ContactsAuthorization::new(
            tenant_id,
            OPERATOR,
            permissions
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
        ),
    )
}

fn context() -> ContactsMutationContext {
    ContactsMutationContext::new(Uuid::now_v7())
}

fn person(name: &str) -> CreatePerson {
    CreatePerson {
        full_name: name.into(),
        email: None,
        telephone: None,
        organisation_id: None,
        tags: Vec::new(),
        custom_fields: BTreeMap::new(),
    }
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn extension_values_are_versioned_filtered_and_tenant_isolated() {
    let full = service(TENANT_A, &[PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION]).await;
    let reader = service(TENANT_A, &[PEOPLE_READ_PERMISSION]).await;
    let created = full
        .create_person(person("Extension Host Ada"), context())
        .await
        .expect("create");
    assert!(created.extensions.is_empty());

    // A user who can read people writes extension values the kernel validated.
    let updated = reader
        .update_person_extensions(
            &created.public_id,
            created.version,
            &patch(PERSON_RESOURCE, &[("example.loyalty.tier", json!("gold"))]),
            context(),
        )
        .await
        .expect("write extension values");
    assert_eq!(updated.version, created.version + 1);
    assert_eq!(updated.extensions["example.loyalty"]["tier"], json!("gold"));
    let reread = full
        .get_person(&created.public_id)
        .await
        .expect("read back");
    assert_eq!(reread.extensions, updated.extensions);

    // Stale versions conflict; nothing is written.
    assert!(matches!(
        reader
            .update_person_extensions(
                &created.public_id,
                created.version,
                &patch(
                    PERSON_RESOURCE,
                    &[("example.loyalty.tier", json!("silver"))]
                ),
                context(),
            )
            .await,
        Err(ContactsError::VersionConflict)
    ));

    // A patch validated for another resource is refused.
    assert!(matches!(
        reader
            .update_person_extensions(
                &created.public_id,
                updated.version,
                &patch(
                    ORGANISATION_RESOURCE,
                    &[("example.loyalty.account_code", json!("A1"))]
                ),
                context(),
            )
            .await,
        Err(ContactsError::Invariant)
    ));

    // Without host read permission the write is denied before storage.
    let stranger = service(TENANT_A, &[]).await;
    assert!(matches!(
        stranger
            .update_person_extensions(
                &created.public_id,
                updated.version,
                &patch(
                    PERSON_RESOURCE,
                    &[("example.loyalty.tier", json!("silver"))]
                ),
                context(),
            )
            .await,
        Err(ContactsError::PermissionDenied)
    ));

    // List filters use the kernel-validated containment document.
    let filter = catalog()
        .validate_filter(PERSON_RESOURCE, "example.loyalty.tier", "gold", &writer)
        .expect("filter");
    let gold = reader
        .list_people(ListRequest {
            extension_filter: Some(filter),
            limit: 200,
            ..ListRequest::default()
        })
        .await
        .expect("filtered list");
    assert!(gold
        .data
        .iter()
        .any(|item| item.public_id == created.public_id));
    assert!(gold
        .data
        .iter()
        .all(|item| item.extensions["example.loyalty"]["tier"] == json!("gold")));
    let silver = catalog()
        .validate_filter(PERSON_RESOURCE, "example.loyalty.tier", "silver", &writer)
        .expect("filter");
    let silver = reader
        .list_people(ListRequest {
            extension_filter: Some(silver),
            limit: 200,
            ..ListRequest::default()
        })
        .await
        .expect("filtered list");
    assert!(silver
        .data
        .iter()
        .all(|item| item.public_id != created.public_id));

    // Clearing removes the namespace entirely.
    let cleared = reader
        .update_person_extensions(
            &created.public_id,
            updated.version,
            &patch(PERSON_RESOURCE, &[("example.loyalty.tier", Value::Null)]),
            context(),
        )
        .await
        .expect("clear");
    assert!(cleared.extensions.is_empty());

    // Another tenant cannot see or write the record.
    let other = service(TENANT_B, &[PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION]).await;
    assert!(matches!(
        other.get_person(&created.public_id).await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        other
            .update_person_extensions(
                &created.public_id,
                cleared.version,
                &patch(PERSON_RESOURCE, &[("example.loyalty.tier", json!("gold"))]),
                context(),
            )
            .await,
        Err(ContactsError::NotFound)
    ));
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn organisations_host_extension_values_and_archived_records_refuse_them() {
    let full = service(
        TENANT_A,
        &[
            ORGANISATIONS_READ_PERMISSION,
            ORGANISATIONS_WRITE_PERMISSION,
        ],
    )
    .await;
    let organisation = full
        .create_organisation(
            CreateOrganisation {
                name: "Extension Host Org".into(),
                email: None,
                telephone: None,
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("create organisation");
    let updated = full
        .update_organisation_extensions(
            &organisation.public_id,
            organisation.version,
            &patch(
                ORGANISATION_RESOURCE,
                &[("example.loyalty.account_code", json!(" A-17 "))],
            ),
            context(),
        )
        .await
        .expect("write organisation extension");
    assert_eq!(
        updated.extensions["example.loyalty"]["account_code"],
        json!("A-17")
    );
    let archived = full
        .archive_organisation(&organisation.public_id, updated.version, context())
        .await
        .expect("archive");
    assert!(matches!(
        full.update_organisation_extensions(
            &organisation.public_id,
            archived.version,
            &patch(
                ORGANISATION_RESOURCE,
                &[("example.loyalty.account_code", json!("B"))]
            ),
            context(),
        )
        .await,
        Err(ContactsError::VersionConflict)
    ));
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_passes_the_contacts_conformance_suite() {
    let url = std::env::var("PLATFORM_TEST_DATABASE_URL").expect("PLATFORM_TEST_DATABASE_URL");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    let store: std::sync::Arc<dyn ContactsStore> =
        std::sync::Arc::new(PostgresContactsStore::new(pool));
    let report = conformance::run(store.clone()).await;
    conformance::standard_fields(store.clone()).await;
    conformance::recycle_bin(store).await;
    let migrator = PgPoolOptions::new()
        .max_connections(1)
        .connect(
            &std::env::var("PLATFORM_TEST_MIGRATOR_DATABASE_URL")
                .expect("PLATFORM_TEST_MIGRATOR_DATABASE_URL"),
        )
        .await
        .expect("connect migrator");
    let audit: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_log WHERE tenant_id = $1")
        .bind(report.tenant_a)
        .fetch_one(&migrator)
        .await
        .expect("audit");
    let outbox: i64 = sqlx::query_scalar("SELECT count(*) FROM outbox WHERE tenant_id = $1")
        .bind(report.tenant_a)
        .fetch_one(&migrator)
        .await
        .expect("outbox");
    assert_eq!(audit, report.mutations_in_a);
    assert_eq!(outbox, report.mutations_in_a);
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn postgres_store_exports_a_workspace_for_the_engine() {
    let connect = |name: &str| {
        let url = std::env::var(name).expect(name);
        async move {
            PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect")
        }
    };
    let pool = connect("PLATFORM_TEST_DATABASE_URL").await;
    let migrator = connect("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    let tenant = Uuid::now_v7();
    sqlx::query("INSERT INTO tenants (id, display_name) VALUES ($1, 'Contacts export')")
        .bind(tenant)
        .execute(&migrator)
        .await
        .expect("tenant");
    let store: std::sync::Arc<dyn ContactsStore> =
        std::sync::Arc::new(PostgresContactsStore::new(pool.clone()));
    let report = conformance::export(store.clone(), tenant).await;
    // Activity rows go in as the worker's projection would: in the
    // workspace and the Contacts execution context.
    let mut tx = pool.begin().await.expect("begin");
    sqlx::query(
        "SELECT set_config('app.tenant_id', $1, true), \
                set_config('app.plugin_id', 'platform.contacts', true)",
    )
    .bind(tenant.to_string())
    .execute(&mut *tx)
    .await
    .expect("context");
    let mut rows = vec![("per000000000000zzz".to_owned(), "created", 9)];
    for public_id in &report.people[..2] {
        // The later entry is written first.
        rows.push((public_id.clone(), "updated", 5));
        rows.push((public_id.clone(), "created", 1));
    }
    for (public_id, action, minute) in rows {
        sqlx::query(
            "INSERT INTO p_contacts.activity (tenant_id, event_id, entity_type, entity_public_id, \
               action, entity_version, actor_id, correlation_id, occurred_at) \
             VALUES ($1, $2, 'person', $3, $4, 1, NULL, $2, \
               '2026-01-01T00:00:00Z'::timestamptz + make_interval(mins => $5))",
        )
        .bind(tenant)
        .bind(Uuid::now_v7())
        .bind(public_id)
        .bind(action)
        .bind(minute)
        .execute(&mut *tx)
        .await
        .expect("activity");
    }
    tx.commit().await.expect("commit");
    conformance::export_activity(store, &report).await;
}
