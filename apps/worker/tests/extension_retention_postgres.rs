//! Real-PostgreSQL acceptance for plugin removal and extension retention
//! (ADR 0012, #277): removal requires a disabled plugin, values survive the
//! retention window and can be exported, and after it the worker purges only
//! the removed plugin's namespace, only in its tenant, with audit evidence.
//! Run with `bash scripts/test-plugin-runtime.sh`.

use contacts_plugin::{
    extension_host::ContactsExtensionHost, ContactsAuthorization, ContactsMutationContext,
    ContactsService, CreatePerson, PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION,
    PERSON_RESOURCE,
};
use platform_db::{PluginCandidateRecord, PluginDesiredState, PluginLifecycleError};
use platform_extensions::ExtensionCatalog;
use plugin_contract::Manifest;
use plugin_runtime_native::NativeExtensionHost;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{collections::BTreeMap, sync::Arc};
use uuid::Uuid;
use worker::extension_retention::{purge_due_extensions, PurgePass};

const TENANT_A: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000001);
const TENANT_B: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000002);
const OPERATOR: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000104);
const LOYALTY: &str = "example.loyalty";

/// A second extending plugin whose values must survive Loyalty's purge. It is
/// never registered; the kernel catalog only needs its declaration.
const OTHER: &str = r#"
manifest_version = 1
id = "example.other"
name = "Other"
version = "0.1.0"
description = "Another extension"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[[permissions]]
id = "other.fields"
description = "Other fields"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "note"
label = "Note"
field_type = "text"
read_permission = "other.fields"
write_permission = "other.fields"
"#;

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is set"));
    PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect {variable}: {error}"))
}

fn contacts_manifest() -> Manifest {
    Manifest::parse_toml(include_str!("../../../plugins/contacts/plugin.toml")).expect("Contacts")
}

fn loyalty_manifest() -> Manifest {
    Manifest::parse_toml(include_str!(
        "../../../fixtures/plugins/loyalty/plugin.toml"
    ))
    .expect("Loyalty")
}

async fn register(app: &PgPool, manifest: &Manifest, path: &str) {
    let digest = manifest
        .supply_chain
        .artifact_digest
        .clone()
        .expect("fixture digest");
    platform_db::register_plugin_candidate(
        app,
        &PluginCandidateRecord {
            plugin_id: manifest.id.clone(),
            version: manifest.version.to_string(),
            manifest: serde_json::to_value(manifest).expect("manifest json"),
            manifest_path: path.into(),
            artifact_path: None,
            declared_digest: Some(digest.clone()),
            verified_digest: Some(digest),
            lifecycle_state: "health_checked".into(),
            failure_control: None,
            failure_detail: None,
        },
    )
    .await
    .expect("register candidate");
}

async fn transition(
    app: &PgPool,
    tenant_id: Uuid,
    plugin_id: &str,
    desired: PluginDesiredState,
) -> Result<(), PluginLifecycleError> {
    platform_db::transition_tenant_plugin(app, tenant_id, OPERATOR, plugin_id, desired, None)
        .await
        .map(|_| ())
}

/// A person in `tenant_id` carrying Loyalty and Other values.
async fn person_with_values(app: &PgPool, tenant_id: Uuid) -> String {
    let service = ContactsService::new(
        app.clone(),
        ContactsAuthorization::new(
            tenant_id,
            OPERATOR,
            [PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        ),
    );
    let context = || ContactsMutationContext::new(Uuid::now_v7());
    let person = service
        .create_person(
            CreatePerson {
                full_name: "Retention Ada".into(),
                email: None,
                telephone: None,
                organisation_id: None,
                tags: Vec::new(),
                custom_fields: BTreeMap::new(),
            },
            context(),
        )
        .await
        .expect("create person");
    let other = Manifest::parse_toml(OTHER).expect("Other");
    let catalog =
        ExtensionCatalog::from_manifests([&contacts_manifest(), &loyalty_manifest(), &other]);
    let patch = catalog
        .validate_patch(
            PERSON_RESOURCE,
            &BTreeMap::from([
                ("example.loyalty.tier".to_owned(), json!("gold")),
                ("example.other.note".to_owned(), json!("keep me")),
            ]),
            &|_: &str, _: &str| true,
        )
        .expect("kernel patch");
    service
        .update_person_extensions(&person.public_id, person.version, &patch, context())
        .await
        .expect("write extension values");
    person.public_id
}

async fn stored_extensions(migrator: &PgPool, tenant_id: Uuid, public_id: &str) -> Value {
    let mut tx = migrator.begin().await.expect("tx");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true), set_config('app.plugin_id', 'platform.contacts', true)")
        .bind(tenant_id.to_string())
        .execute(&mut *tx)
        .await
        .expect("context");
    let value: Value = sqlx::query_scalar(
        "SELECT extensions FROM p_contacts.people WHERE tenant_id = $1 AND public_id = $2",
    )
    .bind(tenant_id)
    .bind(public_id)
    .fetch_one(&mut *tx)
    .await
    .expect("stored extensions");
    tx.commit().await.expect("commit");
    value
}

/// `(updated_at, updated_by, touched_at, version)` for a person (ADR 0014).
async fn stamps(
    pool: &PgPool,
    tenant_id: Uuid,
    public_id: &str,
) -> (
    chrono::DateTime<chrono::Utc>,
    Option<Uuid>,
    chrono::DateTime<chrono::Utc>,
    i64,
) {
    let mut tx = pool.begin().await.expect("tx");
    sqlx::query("SELECT set_config('app.tenant_id', $1, true), set_config('app.plugin_id', 'platform.contacts', true)")
        .bind(tenant_id.to_string())
        .execute(&mut *tx)
        .await
        .expect("context");
    let stamps = sqlx::query_as(
        "SELECT updated_at, updated_by, touched_at, version FROM p_contacts.people \
         WHERE tenant_id = $1 AND public_id = $2",
    )
    .bind(tenant_id)
    .bind(public_id)
    .fetch_one(&mut *tx)
    .await
    .expect("stamps");
    tx.commit().await.expect("commit");
    stamps
}

async fn installation(migrator: &PgPool, tenant_id: Uuid) -> (String, bool, bool) {
    sqlx::query_as(
        "SELECT state, removed_at IS NOT NULL, extensions_purged_at IS NOT NULL \
         FROM tenant_plugin_installations WHERE tenant_id = $1 AND plugin_id = $2",
    )
    .bind(tenant_id)
    .bind(LOYALTY)
    .fetch_one(migrator)
    .await
    .expect("installation")
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn removed_extensions_are_retained_then_purged_per_tenant_and_namespace() {
    let app = pool("PLATFORM_TEST_DATABASE_URL").await;
    let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
    let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
    register(&app, &contacts_manifest(), "plugins/contacts/plugin.toml").await;
    register(
        &app,
        &loyalty_manifest(),
        "fixtures/plugins/loyalty/plugin.toml",
    )
    .await;
    for tenant in [TENANT_A, TENANT_B] {
        transition(
            &app,
            tenant,
            "platform.contacts",
            PluginDesiredState::Enabled,
        )
        .await
        .expect("enable contacts");
        transition(&app, tenant, LOYALTY, PluginDesiredState::Enabled)
            .await
            .expect("enable loyalty");
    }
    let person_a = person_with_values(&app, TENANT_A).await;
    let person_b = person_with_values(&app, TENANT_B).await;
    let hosts: Vec<Arc<dyn NativeExtensionHost>> =
        vec![Arc::new(ContactsExtensionHost::new(app.clone()))];

    // Removal requires a disabled plugin, and the refusal is audited.
    assert!(matches!(
        transition(&app, TENANT_A, LOYALTY, PluginDesiredState::Removed).await,
        Err(PluginLifecycleError::RemovalRequiresDisabled)
    ));
    for tenant in [TENANT_A, TENANT_B] {
        transition(&app, tenant, LOYALTY, PluginDesiredState::Disabled)
            .await
            .expect("disable loyalty");
        transition(&app, tenant, LOYALTY, PluginDesiredState::Removed)
            .await
            .expect("remove loyalty");
    }
    assert_eq!(
        installation(&migrator, TENANT_A).await,
        ("removed".into(), true, false)
    );

    // Inside the window nothing is purged and the values can be exported.
    assert_eq!(
        purge_due_extensions(&worker, &app, &hosts, 30)
            .await
            .expect("pass"),
        PurgePass::default()
    );
    let (exported, truncated) = hosts[0]
        .export_extension_namespace(TENANT_A, LOYALTY, 100)
        .await
        .expect("export");
    assert!(!truncated);
    let mine = exported
        .iter()
        .find(|record| record.record_id == person_a)
        .expect("exported person");
    assert_eq!(mine.resource, PERSON_RESOURCE);
    assert_eq!(mine.values["tier"], json!("gold"));
    // Public ids are numbered per tenant, so count records instead of
    // comparing ids across tenants: each tenant holds exactly one.
    assert_eq!(exported.len(), 1);

    // After the window only tenant A's Loyalty namespace is purged.
    sqlx::query(
        "UPDATE tenant_plugin_installations SET removed_at = now() - interval '31 days' \
         WHERE tenant_id = $1 AND plugin_id = $2",
    )
    .bind(TENANT_A)
    .bind(LOYALTY)
    .execute(&migrator)
    .await
    .expect("age removal");
    let before = stamps(&app, TENANT_A, &person_a).await;
    let pass = purge_due_extensions(&worker, &app, &hosts, 30)
        .await
        .expect("pass");
    assert_eq!(
        pass,
        PurgePass {
            purged: 1,
            deferred: 0
        }
    );
    // A purge is a system change: only touched_at moves (ADR 0014).
    let after = stamps(&app, TENANT_A, &person_a).await;
    assert_eq!(after.0, before.0, "updated_at is unchanged");
    assert_eq!(after.1, before.1, "updated_by is unchanged");
    assert_eq!(after.3, before.3, "version is unchanged");
    assert!(after.2 > before.2, "touched_at moves");
    let a = stored_extensions(&app, TENANT_A, &person_a).await;
    assert!(a.get(LOYALTY).is_none(), "{a}");
    assert_eq!(a["example.other"]["note"], json!("keep me"));
    let b = stored_extensions(&app, TENANT_B, &person_b).await;
    assert_eq!(b[LOYALTY]["tier"], json!("gold"));
    assert_eq!(
        installation(&migrator, TENANT_A).await,
        ("removed".into(), true, true)
    );
    let evidence: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE tenant_id = $1 AND action = 'plugin.extensions.purged' \
         AND target = 'plugin:example.loyalty' ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(TENANT_A)
    .fetch_one(&migrator)
    .await
    .expect("purge audit");
    assert!(evidence["records_by_host"]["platform.contacts"].as_u64() >= Some(1));
    assert!(!evidence.to_string().contains("gold"));

    // A second pass finds nothing; reinstalling clears the removal markers.
    assert_eq!(
        purge_due_extensions(&worker, &app, &hosts, 30)
            .await
            .expect("pass"),
        PurgePass::default()
    );
    transition(&app, TENANT_A, LOYALTY, PluginDesiredState::Enabled)
        .await
        .expect("reinstall");
    assert_eq!(
        installation(&migrator, TENANT_A).await,
        ("enabled".into(), false, false)
    );

    // A host that is not linked defers the purge instead of marking it done.
    sqlx::query(
        "UPDATE tenant_plugin_installations SET removed_at = now() - interval '31 days' \
         WHERE tenant_id = $1 AND plugin_id = $2",
    )
    .bind(TENANT_B)
    .bind(LOYALTY)
    .execute(&migrator)
    .await
    .expect("age removal");
    assert_eq!(
        purge_due_extensions(&worker, &app, &[], 30)
            .await
            .expect("pass"),
        PurgePass {
            purged: 0,
            deferred: 1
        }
    );
    assert_eq!(
        installation(&migrator, TENANT_B).await,
        ("removed".into(), true, false)
    );
}
