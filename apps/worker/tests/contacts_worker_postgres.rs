//! Real-PostgreSQL acceptance for the Contacts worker contributions: the
//! activity-timeline consumer and the list-cursor cleanup job, driven through
//! production paths (Contacts service writes, outbox envelopes, the gated
//! consumer, and leased tenant job runs). Run with
//! `bash scripts/test-plugin-runtime.sh`.

use contacts_plugin::{
    worker::{ACTIVITY_TIMELINE_CONSUMER_ID, LIST_CURSOR_CLEANUP_JOB_ID, RECYCLE_BIN_PURGE_JOB_ID},
    ContactsAuthorization, ContactsError, ContactsMutationContext, ContactsService,
    CreateOrganisation, CreatePerson, UpdatePerson, ORGANISATIONS_READ_PERMISSION,
    ORGANISATIONS_WRITE_PERMISSION, PEOPLE_READ_PERMISSION, PEOPLE_WRITE_PERMISSION,
};
use platform_events::EventEnvelope;
use platform_inbox::BrokerDisposition;
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{num::NonZeroU32, sync::Arc, time::Duration};
use uuid::Uuid;
use worker::plugin_runtime::{
    process_delivery, reconcile_jobs, run_due_jobs, PluginRuntimeMetrics, PluginRuntimeSettings,
    RegisteredConsumer, RegisteredJob, WorkerContributions,
};

const TENANT_A: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000001);
const TENANT_B: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000002);
const OPERATOR: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000104);
const CONTACTS: &str = "platform.contacts";

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is set"));
    PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect {variable}: {error}"))
}

struct Harness {
    app: PgPool,
    worker: PgPool,
    migrator: PgPool,
    consumer: Arc<RegisteredConsumer>,
    job: Arc<RegisteredJob>,
    purge_job: Arc<RegisteredJob>,
    settings: PluginRuntimeSettings,
    metrics: PluginRuntimeMetrics,
}

impl Harness {
    async fn new() -> Self {
        let app = pool("PLATFORM_TEST_DATABASE_URL").await;
        let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
        let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
        let plugins = worker::linked_worker_plugins().expect("linked plugins");
        let manifest = plugins[0].manifest().clone();
        let digest = manifest
            .supply_chain
            .artifact_digest
            .clone()
            .expect("Contacts declares its artifact digest");
        // Register the linked Contacts build through the API's startup path.
        platform_db::register_plugin_candidate(
            &app,
            &platform_db::PluginCandidateRecord {
                plugin_id: CONTACTS.into(),
                version: manifest.version.to_string(),
                manifest: serde_json::to_value(&manifest).expect("manifest json"),
                manifest_path: "plugins/contacts/plugin.toml".into(),
                artifact_path: Some("plugins/contacts/artifact/native.link".into()),
                declared_digest: Some(digest.clone()),
                verified_digest: Some(digest),
                lifecycle_state: "health_checked".into(),
                failure_control: None,
                failure_detail: None,
            },
        )
        .await
        .expect("register Contacts");
        let contributions =
            WorkerContributions::from_plugins(&plugins).expect("verified contributions");
        let consumer = contributions
            .consumers
            .iter()
            .find(|consumer| consumer.id.as_str() == ACTIVITY_TIMELINE_CONSUMER_ID)
            .expect("timeline consumer")
            .clone();
        let job = contributions
            .jobs
            .iter()
            .find(|job| job.id == LIST_CURSOR_CLEANUP_JOB_ID)
            .expect("cleanup job")
            .clone();
        let purge_job = contributions
            .jobs
            .iter()
            .find(|job| job.id == RECYCLE_BIN_PURGE_JOB_ID)
            .expect("recycle bin purge job")
            .clone();
        Self {
            app,
            worker,
            migrator,
            consumer,
            job,
            purge_job,
            settings: PluginRuntimeSettings {
                reconcile_interval: Duration::from_secs(1),
                job_poll_interval: Duration::from_millis(100),
                job_batch_size: NonZeroU32::new(16).expect("non-zero"),
                job_lease: Duration::from_secs(30),
                job_retry_base: Duration::from_secs(60),
                job_retry_max: Duration::from_secs(600),
                consumer_ack_wait: Duration::from_secs(30),
                consumer_max_deliver: 3,
                consumer_retry_delay: Duration::from_millis(100),
            },
            metrics: PluginRuntimeMetrics::default(),
        }
    }

    fn service(&self, tenant_id: Uuid) -> ContactsService {
        ContactsService::new(
            self.app.clone(),
            ContactsAuthorization::new(
                tenant_id,
                OPERATOR,
                [
                    PEOPLE_READ_PERMISSION,
                    PEOPLE_WRITE_PERMISSION,
                    ORGANISATIONS_READ_PERMISSION,
                    ORGANISATIONS_WRITE_PERMISSION,
                ]
                .into_iter()
                .map(str::to_owned)
                .collect(),
            ),
        )
    }

    async fn transition(&self, tenant_id: Uuid, desired: platform_db::PluginDesiredState) {
        platform_db::transition_tenant_plugin(
            &self.app, tenant_id, OPERATOR, CONTACTS, desired, None,
        )
        .await
        .expect("lifecycle transition");
    }

    /// The newest committed outbox envelope of a type, as the publisher would
    /// put it on the broker.
    async fn latest_event(&self, tenant_id: Uuid, event_type: &str) -> EventEnvelope {
        let mut tx = self.migrator.begin().await.expect("migrator tx");
        let envelope: Value = sqlx::query_scalar(
            "SELECT envelope FROM outbox WHERE tenant_id = $1 AND event_type = $2 ORDER BY created_at DESC, id DESC LIMIT 1",
        )
        .bind(tenant_id)
        .bind(event_type)
        .fetch_one(&mut *tx)
        .await
        .expect("outbox envelope");
        tx.commit().await.expect("commit");
        serde_json::from_value(envelope).expect("valid envelope")
    }

    async fn deliver(&self, event: &EventEnvelope) -> BrokerDisposition {
        process_delivery(
            &self.app,
            &self.consumer,
            &event.jetstream_subject().expect("subject"),
            &serde_json::to_vec(event).expect("payload"),
            1,
            self.settings.consumer_max_deliver,
            &self.metrics,
        )
        .await
    }

    async fn insert_cursor(&self, tenant_id: Uuid, expired: bool) -> String {
        let token_hash = format!("{:064x}", Uuid::now_v7().as_u128());
        let mut tx = self.migrator.begin().await.expect("migrator tx");
        sqlx::query("SELECT set_config('app.tenant_id', $1, true)")
            .bind(tenant_id.to_string())
            .execute(&mut *tx)
            .await
            .expect("tenant context");
        sqlx::query(
            r#"
            INSERT INTO p_contacts.list_cursors
              (token_hash, tenant_id, resource, query_hash, sort_value, record_id, expires_at)
            VALUES ($1, $2, 'people', $3, 'sort', $4,
                    CASE WHEN $5 THEN now() - interval '1 minute' ELSE now() + interval '1 hour' END)
            "#,
        )
        .bind(&token_hash)
        .bind(tenant_id)
        .bind("0".repeat(64))
        .bind(Uuid::now_v7())
        .bind(expired)
        .execute(&mut *tx)
        .await
        .expect("insert cursor");
        tx.commit().await.expect("commit");
        token_hash
    }

    async fn cursor_exists(&self, tenant_id: Uuid, token_hash: &str) -> bool {
        let mut tx = platform_db::begin_plugin_tx(&self.app, tenant_id, CONTACTS)
            .await
            .expect("tenant tx");
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM p_contacts.list_cursors WHERE token_hash = $1)",
        )
        .bind(token_hash)
        .fetch_one(&mut *tx)
        .await
        .expect("cursor query");
        tx.commit().await.expect("commit");
        exists
    }

    async fn make_due(&self) {
        sqlx::query("UPDATE plugin_job_schedules SET next_run_at = now() - interval '1 second' WHERE plugin_id = $1")
            .bind(CONTACTS)
            .execute(&self.migrator)
            .await
            .expect("make due");
    }

    async fn checkpoint(&self, tenant_id: Uuid) -> Option<Value> {
        let mut tx = platform_db::begin_plugin_tx(&self.app, tenant_id, CONTACTS)
            .await
            .expect("tenant tx");
        let checkpoint = sqlx::query_scalar(
            "SELECT checkpoint FROM plugin_job_schedules WHERE plugin_id = $1 AND job_id = $2",
        )
        .bind(CONTACTS)
        .bind(LIST_CURSOR_CLEANUP_JOB_ID)
        .fetch_optional(&mut *tx)
        .await
        .expect("checkpoint");
        tx.commit().await.expect("commit");
        checkpoint
    }
}

/// Whether a statement failed specifically with a row-level-security refusal
/// (SQLSTATE 42501), rather than an unrelated constraint.
fn rls_violation<T>(result: &Result<T, sqlx::Error>) -> Option<bool> {
    let error = result.as_ref().err()?;
    Some(error.as_database_error()?.code().as_deref() == Some("42501"))
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn contacts_activity_timeline_follows_events_and_lifecycle() {
    let harness = Harness::new().await;
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    let service = harness.service(TENANT_A);
    let context = ContactsMutationContext::new(Uuid::now_v7());

    let person = service
        .create_person(
            CreatePerson {
                full_name: "Timeline Person".into(),
                email: None,
                telephone: None,
                organisation_id: None,
                tags: Vec::new(),
                custom_fields: Default::default(),
            },
            context,
        )
        .await
        .expect("create person");
    let created = harness
        .latest_event(TENANT_A, "platform.contacts.person-created.v1")
        .await;
    assert_eq!(
        harness.deliver(&created).await,
        BrokerDisposition::Acknowledge
    );
    // Redelivery is deduplicated.
    assert_eq!(
        harness.deliver(&created).await,
        BrokerDisposition::Acknowledge
    );

    let updated_person = service
        .update_person(
            &person.public_id,
            UpdatePerson {
                expected_version: person.version,
                full_name: "Timeline Person Renamed".into(),
                email: None,
                telephone: None,
                organisation_id: None,
                tags: Vec::new(),
                custom_fields: Default::default(),
            },
            context,
        )
        .await
        .expect("update person");
    let updated = harness
        .latest_event(TENANT_A, "platform.contacts.person-updated.v1")
        .await;
    assert_eq!(
        harness.deliver(&updated).await,
        BrokerDisposition::Acknowledge
    );

    let activity = service
        .person_activity(&person.public_id)
        .await
        .expect("activity");
    assert_eq!(
        activity
            .iter()
            .map(|entry| (entry.action.as_str(), entry.entity_version))
            .collect::<Vec<_>>(),
        vec![
            ("updated", updated_person.version),
            ("created", person.version)
        ]
    );
    assert_eq!(activity[0].event_id, updated.id);
    assert_eq!(activity[0].actor_id, Some(OPERATOR));

    // Disabled: the archive event is acknowledged without a timeline entry,
    // and the existing history is retained.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Disabled)
        .await;
    service
        .archive_person(&person.public_id, updated_person.version, context)
        .await
        .expect("archive person");
    let archived = harness
        .latest_event(TENANT_A, "platform.contacts.person-archived.v1")
        .await;
    assert_eq!(
        harness.deliver(&archived).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(
        service
            .person_activity(&person.public_id)
            .await
            .unwrap()
            .len(),
        2
    );

    // Re-enabled: a redelivery of the skipped event is applied once.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    assert_eq!(
        harness.deliver(&archived).await,
        BrokerDisposition::Acknowledge
    );
    let activity = service.person_activity(&person.public_id).await.unwrap();
    assert_eq!(activity.len(), 3);
    assert_eq!(activity[0].action, "archived");

    // Another workspace cannot see the timeline or the record.
    assert!(matches!(
        harness
            .service(TENANT_B)
            .person_activity(&person.public_id)
            .await,
        Err(contacts_plugin::ContactsError::NotFound)
    ));

    // Plugin execution context: in the right workspace, kernel code (no
    // plugin context) and another plugin's context can neither read Contacts
    // rows nor write them, nor publish events as Contacts.
    for context in [None, Some("platform.fixture")] {
        let mut tx = match context {
            None => platform_db::begin_tenant_tx(&harness.app, TENANT_A).await,
            Some(plugin) => platform_db::begin_plugin_tx(&harness.app, TENANT_A, plugin).await,
        }
        .expect("tx");
        for table in [
            "p_contacts.people",
            "p_contacts.activity",
            "p_contacts.list_cursors",
        ] {
            let visible: i64 = sqlx::query_scalar(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&mut *tx)
                .await
                .expect("count");
            assert_eq!(visible, 0, "{context:?} must not read {table}");
        }
        tx.rollback().await.expect("rollback");

        let mut tx = match context {
            None => platform_db::begin_tenant_tx(&harness.app, TENANT_A).await,
            Some(plugin) => platform_db::begin_plugin_tx(&harness.app, TENANT_A, plugin).await,
        }
        .expect("tx");
        let forged_write = sqlx::query(
            "INSERT INTO p_contacts.people (tenant_id, id, public_id, full_name) VALUES ($1, $2, $3, 'Forged')",
        )
        .bind(TENANT_A)
        .bind(Uuid::now_v7())
        .bind(platform_id::PublicId::new("per", 999_999).expect("public id").as_str().to_owned())
        .execute(&mut *tx)
        .await;
        assert_eq!(
            rls_violation(&forged_write),
            Some(true),
            "{context:?} must be refused by RLS when writing Contacts tables"
        );
        tx.rollback().await.expect("rollback");

        let mut tx = match context {
            None => platform_db::begin_tenant_tx(&harness.app, TENANT_A).await,
            Some(plugin) => platform_db::begin_plugin_tx(&harness.app, TENANT_A, plugin).await,
        }
        .expect("tx");
        let forged_event = sqlx::query(
            "INSERT INTO outbox (id, tenant_id, event_type, envelope) VALUES ($1, $2, 'platform.contacts.person-created.v1', jsonb_build_object('source_plugin', 'platform.contacts'))",
        )
        .bind(Uuid::now_v7())
        .bind(TENANT_A)
        .execute(&mut *tx)
        .await;
        assert_eq!(
            rls_violation(&forged_event),
            Some(true),
            "{context:?} must be refused by RLS when publishing as Contacts"
        );
        tx.rollback().await.expect("rollback");
    }

    // A poisoned payload is terminated, never half-applied.
    let mut forged = updated.clone();
    forged.id = Uuid::now_v7();
    forged.payload = json!({"entity_id": "not-a-public-id", "version": 1});
    assert_eq!(harness.deliver(&forged).await, BrokerDisposition::Terminate);
    assert_eq!(
        service
            .person_activity(&person.public_id)
            .await
            .unwrap()
            .len(),
        3
    );
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn contacts_cursor_cleanup_runs_only_for_active_tenants() {
    let harness = Harness::new().await;
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    harness
        .transition(TENANT_B, platform_db::PluginDesiredState::Disabled)
        .await;
    let expired_a = harness.insert_cursor(TENANT_A, true).await;
    let live_a = harness.insert_cursor(TENANT_A, false).await;
    let expired_b = harness.insert_cursor(TENANT_B, true).await;

    let jobs = std::slice::from_ref(&harness.job);
    assert!(reconcile_jobs(&harness.worker, jobs, &harness.metrics).await);
    harness.make_due().await;
    let ran = run_due_jobs(
        &harness.worker,
        &harness.app,
        &harness.job,
        &harness.settings,
        &harness.metrics,
    )
    .await
    .expect("run cleanup");
    assert_eq!(ran, 1, "only the active tenant has a runnable schedule");

    assert!(!harness.cursor_exists(TENANT_A, &expired_a).await);
    assert!(harness.cursor_exists(TENANT_A, &live_a).await);
    assert!(
        harness.cursor_exists(TENANT_B, &expired_b).await,
        "a disabled tenant's data is untouched"
    );
    let checkpoint = harness.checkpoint(TENANT_A).await.expect("checkpoint");
    assert!(checkpoint["deleted_total"].as_u64().unwrap_or(0) >= 1);
    assert!(harness.checkpoint(TENANT_B).await.is_none());
}

/// Moves a record's deletion time back, as if it had sat in the recycle bin
/// for `days`.
async fn age_deletion(pool: &PgPool, tenant_id: Uuid, table: &str, public_id: &str, days: i32) {
    let mut tx = platform_db::begin_plugin_tx(pool, tenant_id, CONTACTS)
        .await
        .expect("plugin tx");
    sqlx::query(&format!(
        "UPDATE p_contacts.{table} SET deleted_at = now() - make_interval(days => $3) \
         WHERE tenant_id = $1 AND public_id = $2"
    ))
    .bind(tenant_id)
    .bind(public_id)
    .bind(days)
    .execute(&mut *tx)
    .await
    .expect("age deletion");
    tx.commit().await.expect("commit");
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn contacts_recycle_bin_purge_removes_only_expired_deletions() {
    let harness = Harness::new().await;
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    let service = harness.service(TENANT_A);
    let context = || ContactsMutationContext::new(Uuid::now_v7());
    let person = |name: &str, organisation_id: Option<String>| CreatePerson {
        full_name: name.into(),
        email: None,
        telephone: None,
        organisation_id,
        tags: Vec::new(),
        custom_fields: Default::default(),
    };
    let organisation = service
        .create_organisation(
            CreateOrganisation {
                name: "Purged Org".into(),
                email: None,
                telephone: None,
                custom_fields: Default::default(),
            },
            context(),
        )
        .await
        .expect("organisation");
    let linked = service
        .create_person(
            person("Linked", Some(organisation.public_id.clone())),
            context(),
        )
        .await
        .expect("linked person");
    let expired = service
        .create_person(person("Expired", None), context())
        .await
        .expect("expired person");
    let recent = service
        .create_person(person("Recent", None), context())
        .await
        .expect("recent person");
    service
        .delete_organisation(&organisation.public_id, organisation.version, context())
        .await
        .expect("delete organisation");
    service
        .delete_person(&expired.public_id, expired.version, context())
        .await
        .expect("delete expired");
    service
        .delete_person(&recent.public_id, recent.version, context())
        .await
        .expect("delete recent");
    age_deletion(
        &harness.app,
        TENANT_A,
        "organisations",
        &organisation.public_id,
        16,
    )
    .await;
    age_deletion(&harness.app, TENANT_A, "people", &expired.public_id, 16).await;
    let linked_before = service.get_person(&linked.public_id).await.expect("linked");

    let jobs = std::slice::from_ref(&harness.purge_job);
    assert!(reconcile_jobs(&harness.worker, jobs, &harness.metrics).await);
    harness.make_due().await;
    run_due_jobs(
        &harness.worker,
        &harness.app,
        &harness.purge_job,
        &harness.settings,
        &harness.metrics,
    )
    .await
    .expect("run purge");

    // Past the retention window: gone for good, even with include_deleted.
    assert!(matches!(
        service.read_person(&expired.public_id, true).await,
        Err(ContactsError::NotFound)
    ));
    assert!(matches!(
        service
            .read_organisation(&organisation.public_id, true)
            .await,
        Err(ContactsError::NotFound)
    ));
    // Inside the window: still in the recycle bin.
    let kept = service
        .read_person(&recent.public_id, true)
        .await
        .expect("still in the bin");
    assert!(kept.deleted_at.is_some());
    // Losing a purged organisation is a system change: only touched_at moves.
    let linked_after = service.get_person(&linked.public_id).await.expect("linked");
    assert_eq!(linked_after.organisation_public_id, None);
    assert_eq!(linked_after.version, linked_before.version);
    assert_eq!(linked_after.updated_at, linked_before.updated_at);
    assert!(linked_after.touched_at > linked_before.touched_at);

    let evidence: Value = sqlx::query_scalar(
        "SELECT metadata FROM audit_log WHERE tenant_id = $1 AND action = 'contacts.recycle-bin.purged' \
         ORDER BY occurred_at DESC LIMIT 1",
    )
    .bind(TENANT_A)
    .fetch_one(&harness.migrator)
    .await
    .expect("purge audit");
    assert_eq!(
        evidence,
        json!({"people": 1, "organisations": 1, "people_unlinked": 1, "retention_days": 15})
    );
}
