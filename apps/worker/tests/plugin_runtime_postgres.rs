//! Real-PostgreSQL acceptance for plugin consumers and jobs under tenant
//! lifecycle changes. Run with `bash scripts/test-plugin-runtime.sh`; the
//! tests share one fixture plugin and must run with `--test-threads=1`.

use async_trait::async_trait;
use platform_events::EventEnvelope;
use platform_inbox::{BrokerDisposition, ConsumerId, EventHandler, HandlerFailure};
use platform_jobs::{ClaimedJob, JobFailure, RunOutcome, ScheduledJob};
use plugin_runtime_native::{NativeConsumer, NativeJob, NativeWorkerPlugin};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, Transaction};
use std::{
    num::NonZeroU32,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;
use uuid::Uuid;
use worker::plugin_runtime::{
    process_delivery, reconcile_jobs, run_due_jobs, ActivationGate, PluginRuntimeMetrics,
    PluginRuntimeSettings, RegisteredConsumer, RegisteredJob, WorkerContributions,
};

const TENANT_A: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000001);
const TENANT_B: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000002);
const OPERATOR: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000104);
const PLUGIN_ID: &str = "platform.fixture";
const CONSUMER_ID: &str = "platform.fixture.probe-v1";
const JOB_ID: &str = "platform.fixture.tick-v1";
const EVENT_TYPE: &str = "platform.contacts.person-created.v1";
const DIGEST: &str = "sha256:1111111111111111111111111111111111111111111111111111111111111111";

const MANIFEST: &str = r#"
manifest_version = 1
id = "platform.fixture"
name = "Fixture"
version = "0.1.0"
description = "Worker lifecycle acceptance fixture"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[supply_chain]
artifact_digest = "sha256:1111111111111111111111111111111111111111111111111111111111111111"

[entrypoints]
worker = true

[[events.consume]]
type = "platform.contacts.person-created.v1"

[capabilities]
event_consume = ["platform.contacts.person-created.v1"]

[[worker.consumers]]
id = "platform.fixture.probe-v1"
events = ["platform.contacts.person-created.v1"]

[[worker.jobs]]
id = "platform.fixture.tick-v1"
interval_seconds = 60
"#;

const FIXTURE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS worker_acceptance_effects (
  tenant_id uuid NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  kind text NOT NULL,
  reference text NOT NULL,
  created_at timestamptz NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, kind, reference)
);
ALTER TABLE worker_acceptance_effects ENABLE ROW LEVEL SECURITY;
ALTER TABLE worker_acceptance_effects FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS worker_acceptance_effects_tenant_isolation ON worker_acceptance_effects;
CREATE POLICY worker_acceptance_effects_tenant_isolation
ON worker_acceptance_effects
USING (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid)
WITH CHECK (tenant_id = nullif(current_setting('app.tenant_id', true), '')::uuid);
GRANT SELECT, INSERT ON worker_acceptance_effects TO platform_app;
"#;

fn control(failure: Result<HandlerFailure, platform_inbox::InvalidControlCode>) -> HandlerFailure {
    failure.expect("static acceptance control is valid")
}

/// Records one effect per event, optionally pausing mid-transaction so the
/// test can attempt a concurrent lifecycle transition.
#[derive(Default)]
struct ProbeConsumer {
    fail_retryable: AtomicBool,
    pause: AtomicBool,
    started: Notify,
    release: Notify,
}

#[async_trait]
impl EventHandler for ProbeConsumer {
    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<(), HandlerFailure> {
        sqlx::query(
            "INSERT INTO worker_acceptance_effects (tenant_id, kind, reference) VALUES ($1, 'event', $2)",
        )
        .bind(event.tenant_id)
        .bind(event.id.to_string())
        .execute(&mut **transaction)
        .await
        .map_err(|_| control(HandlerFailure::retryable("acceptance.effect_database")))?;
        if self.fail_retryable.load(Ordering::SeqCst) {
            return Err(control(HandlerFailure::retryable("acceptance.transient")));
        }
        if self.pause.load(Ordering::SeqCst) {
            self.started.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

/// Records one effect per committed run and advances a checkpoint.
#[derive(Default)]
struct ProbeJob {
    fail: AtomicBool,
}

#[async_trait]
impl ScheduledJob for ProbeJob {
    async fn run(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        claim: &ClaimedJob,
    ) -> Result<Value, JobFailure> {
        let runs = claim.checkpoint["runs"].as_i64().unwrap_or(0) + 1;
        sqlx::query(
            "INSERT INTO worker_acceptance_effects (tenant_id, kind, reference) VALUES ($1, 'job', $2)",
        )
        .bind(claim.tenant_id)
        .bind(format!("{}:{runs}", claim.claim_token))
        .execute(&mut **transaction)
        .await
        .map_err(|_| JobFailure::new("acceptance.effect_database").expect("valid control"))?;
        if self.fail.load(Ordering::SeqCst) {
            return Err(JobFailure::new("acceptance.job_failed").expect("valid control"));
        }
        Ok(json!({ "runs": runs }))
    }
}

struct FixturePlugin {
    manifest: plugin_contract::Manifest,
    consumer: Arc<ProbeConsumer>,
    job: Arc<ProbeJob>,
}

impl NativeWorkerPlugin for FixturePlugin {
    fn manifest(&self) -> &plugin_contract::Manifest {
        &self.manifest
    }

    fn consumers(&self) -> Vec<NativeConsumer> {
        vec![NativeConsumer {
            id: ConsumerId::parse(CONSUMER_ID).expect("valid consumer id"),
            handler: self.consumer.clone(),
        }]
    }

    fn jobs(&self) -> Vec<NativeJob> {
        vec![NativeJob {
            id: JOB_ID.into(),
            job: self.job.clone(),
        }]
    }
}

struct Harness {
    app: PgPool,
    worker: PgPool,
    migrator: PgPool,
    consumer_impl: Arc<ProbeConsumer>,
    job_impl: Arc<ProbeJob>,
    consumer: Arc<RegisteredConsumer>,
    job: Arc<RegisteredJob>,
    settings: PluginRuntimeSettings,
    metrics: PluginRuntimeMetrics,
}

async fn pool(variable: &str) -> PgPool {
    let url = std::env::var(variable).unwrap_or_else(|_| panic!("{variable} is set"));
    PgPoolOptions::new()
        .max_connections(8)
        .connect(&url)
        .await
        .unwrap_or_else(|error| panic!("connect {variable}: {error}"))
}

impl Harness {
    async fn new() -> Self {
        let app = pool("PLATFORM_TEST_DATABASE_URL").await;
        let worker = pool("PLATFORM_TEST_WORKER_DATABASE_URL").await;
        let migrator = pool("PLATFORM_TEST_MIGRATOR_DATABASE_URL").await;
        sqlx::raw_sql(FIXTURE_DDL)
            .execute(&migrator)
            .await
            .expect("create acceptance effect table");

        let manifest = plugin_contract::Manifest::parse_toml(MANIFEST).expect("fixture manifest");
        // Register through the same path the API uses at startup.
        platform_db::register_plugin_candidate(
            &app,
            &platform_db::PluginCandidateRecord {
                plugin_id: PLUGIN_ID.into(),
                version: manifest.version.to_string(),
                manifest: serde_json::to_value(&manifest).expect("manifest json"),
                manifest_path: "plugins/fixture/plugin.toml".into(),
                artifact_path: Some("plugins/fixture/artifact/native.link".into()),
                declared_digest: Some(DIGEST.into()),
                verified_digest: Some(DIGEST.into()),
                lifecycle_state: "health_checked".into(),
                failure_control: None,
                failure_detail: None,
            },
        )
        .await
        .expect("register fixture plugin");

        let consumer_impl = Arc::new(ProbeConsumer::default());
        let job_impl = Arc::new(ProbeJob::default());
        let plugin: Arc<dyn NativeWorkerPlugin> = Arc::new(FixturePlugin {
            manifest,
            consumer: consumer_impl.clone(),
            job: job_impl.clone(),
        });
        let contributions =
            WorkerContributions::from_plugins(&[plugin]).expect("verified contributions");
        let settings = PluginRuntimeSettings {
            reconcile_interval: Duration::from_secs(1),
            job_poll_interval: Duration::from_millis(100),
            job_batch_size: NonZeroU32::new(16).expect("non-zero"),
            job_lease: Duration::from_secs(30),
            job_retry_base: Duration::from_secs(60),
            job_retry_max: Duration::from_secs(600),
            consumer_ack_wait: Duration::from_secs(30),
            consumer_max_deliver: 3,
            consumer_retry_delay: Duration::from_millis(100),
        };
        Self {
            app,
            worker,
            migrator,
            consumer_impl,
            job_impl,
            consumer: contributions.consumers[0].clone(),
            job: contributions.jobs[0].clone(),
            settings,
            metrics: PluginRuntimeMetrics::default(),
        }
    }

    async fn transition(&self, tenant_id: Uuid, desired: platform_db::PluginDesiredState) {
        platform_db::transition_tenant_plugin(
            &self.app, tenant_id, OPERATOR, PLUGIN_ID, desired, None,
        )
        .await
        .expect("lifecycle transition");
    }

    async fn deliver(&self, event: &EventEnvelope, delivered: u64) -> BrokerDisposition {
        let subject = event.jetstream_subject().expect("subject");
        let payload = serde_json::to_vec(event).expect("payload");
        process_delivery(
            &self.app,
            &self.consumer,
            &subject,
            &payload,
            delivered,
            self.settings.consumer_max_deliver,
            &self.metrics,
        )
        .await
    }

    /// Count tenant-visible rows through the RLS-bound role.
    async fn count(&self, tenant_id: Uuid, sql: &str, reference: Option<String>) -> i64 {
        let mut tx = platform_db::begin_tenant_tx(&self.app, tenant_id)
            .await
            .expect("tenant tx");
        let query = sqlx::query_scalar::<_, i64>(sql);
        let query = match reference {
            Some(reference) => query.bind(reference),
            None => query,
        };
        let count = query.fetch_one(&mut *tx).await.expect("count");
        tx.commit().await.expect("commit count");
        count
    }

    async fn event_effects(&self, event: &EventEnvelope) -> i64 {
        self.count(
            event.tenant_id,
            "SELECT count(*) FROM worker_acceptance_effects WHERE kind = 'event' AND reference = $1",
            Some(event.id.to_string()),
        )
        .await
    }

    async fn markers(&self, event: &EventEnvelope) -> i64 {
        self.count(
            event.tenant_id,
            "SELECT count(*) FROM inbox WHERE consumer_id = 'platform.fixture.probe-v1' AND event_id = $1::uuid",
            Some(event.id.to_string()),
        )
        .await
    }

    async fn job_effects(&self, tenant_id: Uuid) -> i64 {
        self.count(
            tenant_id,
            "SELECT count(*) FROM worker_acceptance_effects WHERE kind = 'job'",
            None,
        )
        .await
    }

    async fn schedule(&self, tenant_id: Uuid) -> Option<(Value, i64, Option<String>, bool)> {
        let mut tx = platform_db::begin_plugin_tx(&self.app, tenant_id, PLUGIN_ID)
            .await
            .expect("tenant tx");
        let row = sqlx::query_as::<_, (Value, i64, Option<String>, bool)>(
            r#"
            SELECT checkpoint, run_count, last_error_code, claim_expires_at IS NOT NULL
            FROM plugin_job_schedules
            WHERE plugin_id = $1 AND job_id = $2
            "#,
        )
        .bind(PLUGIN_ID)
        .bind(JOB_ID)
        .fetch_optional(&mut *tx)
        .await
        .expect("read schedule");
        tx.commit().await.expect("commit schedule read");
        row
    }

    /// Make every schedule due without touching anything else.
    async fn make_due(&self) {
        sqlx::query("UPDATE plugin_job_schedules SET next_run_at = now() - interval '1 second' WHERE plugin_id = $1")
            .bind(PLUGIN_ID)
            .execute(&self.migrator)
            .await
            .expect("make schedules due");
    }

    async fn run_jobs(&self) -> usize {
        run_due_jobs(
            &self.worker,
            &self.app,
            &self.job,
            &self.settings,
            &self.metrics,
        )
        .await
        .expect("run due jobs")
    }

    async fn reset(&self) {
        for tenant in [TENANT_A, TENANT_B] {
            self.transition(tenant, platform_db::PluginDesiredState::Disabled)
                .await;
        }
        sqlx::query("DELETE FROM plugin_job_schedules WHERE plugin_id = $1")
            .bind(PLUGIN_ID)
            .execute(&self.migrator)
            .await
            .expect("reset schedules");
        // TRUNCATE by the owner clears the probe table across tenants; it is a
        // test-only fixture, so no runtime role can do this.
        sqlx::query("TRUNCATE worker_acceptance_effects")
            .execute(&self.migrator)
            .await
            .expect("reset effects");
    }
}

fn event(tenant_id: Uuid) -> EventEnvelope {
    EventEnvelope::with_context(
        EVENT_TYPE,
        "platform.contacts",
        tenant_id,
        None,
        Uuid::now_v7(),
        None,
        json!({"entity_id": "per_acceptance", "version": 1}),
    )
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn consumers_follow_tenant_lifecycle_without_losing_or_duplicating_effects() {
    let harness = Harness::new().await;
    harness.reset().await;
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;

    // Enabled: applied once; redelivery is an acknowledged duplicate.
    let first = event(TENANT_A);
    assert_eq!(
        harness.deliver(&first, 1).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(
        harness.deliver(&first, 2).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(harness.event_effects(&first).await, 1);
    assert_eq!(harness.markers(&first).await, 1);

    // Unresolved installation (tenant B never enabled): no effect, no marker.
    let other_tenant = event(TENANT_B);
    assert_eq!(
        harness.deliver(&other_tenant, 1).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(harness.event_effects(&other_tenant).await, 0);
    assert_eq!(harness.markers(&other_tenant).await, 0);

    // Disabled: skipped without effect or marker, prior evidence retained.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Disabled)
        .await;
    let while_disabled = event(TENANT_A);
    assert_eq!(
        harness.deliver(&while_disabled, 1).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(harness.event_effects(&while_disabled).await, 0);
    assert_eq!(harness.markers(&while_disabled).await, 0);
    assert_eq!(harness.event_effects(&first).await, 1);
    assert_eq!(harness.markers(&first).await, 1);

    // Re-enabled: a redelivered event applies once; history stays deduplicated.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    assert_eq!(
        harness.deliver(&while_disabled, 2).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(harness.event_effects(&while_disabled).await, 1);
    assert_eq!(
        harness.deliver(&first, 3).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(harness.event_effects(&first).await, 1);

    // A worker linked against a different artifact cannot act on this
    // activation, but it must not treat the skew as a disable: the delivery
    // stays retryable (without limit) for a worker that links the right build.
    let mut other_build =
        plugin_contract::Manifest::parse_toml(MANIFEST).expect("fixture manifest");
    other_build.supply_chain.artifact_digest = Some(format!("sha256:{}", "2".repeat(64)));
    let skewed_plugin: Arc<dyn NativeWorkerPlugin> = Arc::new(FixturePlugin {
        manifest: other_build,
        consumer: Arc::new(ProbeConsumer::default()),
        job: Arc::new(ProbeJob::default()),
    });
    let skewed = WorkerContributions::from_plugins(&[skewed_plugin]).expect("skewed build");
    let skewed_event = event(TENANT_A);
    let subject = skewed_event.jetstream_subject().expect("subject");
    let payload = serde_json::to_vec(&skewed_event).expect("payload");
    for delivered in [1, 50] {
        assert_eq!(
            process_delivery(
                &harness.app,
                &skewed.consumers[0],
                &subject,
                &payload,
                delivered,
                harness.settings.consumer_max_deliver,
                &harness.metrics,
            )
            .await,
            BrokerDisposition::Retry
        );
    }
    assert_eq!(harness.event_effects(&skewed_event).await, 0);
    assert_eq!(harness.markers(&skewed_event).await, 0);
    // The correctly linked consumer then applies it once.
    assert_eq!(
        harness.deliver(&skewed_event, 51).await,
        BrokerDisposition::Acknowledge
    );
    assert_eq!(harness.event_effects(&skewed_event).await, 1);

    // A retryable handler failure is bounded: retried, then explicitly
    // terminated at the delivery limit, with no partial effect committed.
    harness
        .consumer_impl
        .fail_retryable
        .store(true, Ordering::SeqCst);
    let flaky = event(TENANT_A);
    assert_eq!(harness.deliver(&flaky, 1).await, BrokerDisposition::Retry);
    assert_eq!(
        harness.deliver(&flaky, 3).await,
        BrokerDisposition::Terminate
    );
    harness
        .consumer_impl
        .fail_retryable
        .store(false, Ordering::SeqCst);
    assert_eq!(harness.event_effects(&flaky).await, 0);
    assert_eq!(harness.markers(&flaky).await, 0);

    // Disable during in-flight work: the transition waits for the handler's
    // transaction (drain) and the committed effect is kept.
    harness.consumer_impl.pause.store(true, Ordering::SeqCst);
    let in_flight = event(TENANT_A);
    let in_flight_delivery = {
        let started = harness.consumer_impl.started.notified();
        let delivery = harness.deliver(&in_flight, 1);
        tokio::pin!(delivery);
        tokio::select! {
            _ = &mut delivery => panic!("delivery finished before the handler paused"),
            _ = started => {}
        }
        let disable = harness.transition(TENANT_A, platform_db::PluginDesiredState::Disabled);
        tokio::pin!(disable);
        tokio::select! {
            _ = &mut disable => panic!("disable did not wait for in-flight work"),
            _ = tokio::time::sleep(Duration::from_millis(300)) => {}
        }
        harness.consumer_impl.pause.store(false, Ordering::SeqCst);
        harness.consumer_impl.release.notify_one();
        let (disposition, ()) = tokio::join!(delivery, disable);
        disposition
    };
    assert_eq!(in_flight_delivery, BrokerDisposition::Acknowledge);
    assert_eq!(harness.event_effects(&in_flight).await, 1);
    let after_drain = event(TENANT_A);
    harness.deliver(&after_drain, 1).await;
    assert_eq!(harness.event_effects(&after_drain).await, 0);

    // Lifecycle-store outage fails closed and is retried without limit: an
    // outage longer than the retry budget must not discard events.
    let unreachable = PgPoolOptions::new()
        .acquire_timeout(Duration::from_millis(200))
        .connect_lazy("postgres://platform_app:platform_app@127.0.0.1:1/platform")
        .expect("lazy pool");
    let outage = event(TENANT_A);
    let subject = outage.jetstream_subject().expect("subject");
    let payload = serde_json::to_vec(&outage).expect("payload");
    for (delivered, expected) in [
        (1, BrokerDisposition::Retry),
        (500, BrokerDisposition::Retry),
    ] {
        assert_eq!(
            process_delivery(
                &unreachable,
                &harness.consumer,
                &subject,
                &payload,
                delivered,
                harness.settings.consumer_max_deliver,
                &harness.metrics,
            )
            .await,
            expected
        );
    }
    assert_eq!(harness.event_effects(&outage).await, 0);

    let metrics = harness.metrics.render();
    assert!(
        metrics.contains("limiara_plugin_deliveries_skipped_inactive_total 3"),
        "{metrics}"
    );
    assert!(
        metrics.contains("limiara_plugin_deliveries_terminated_total 1"),
        "{metrics}"
    );
    assert!(
        metrics.contains("limiara_plugin_deliveries_deferred_total 2"),
        "{metrics}"
    );
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL fixture"]
async fn jobs_pause_on_disable_and_resume_once_with_their_checkpoint() {
    let harness = Harness::new().await;
    harness.reset().await;
    let jobs = std::slice::from_ref(&harness.job);

    // Only active installations receive a schedule; reconciliation is
    // idempotent across restarts.
    assert!(reconcile_jobs(&harness.worker, jobs, &harness.metrics).await);
    assert!(harness.schedule(TENANT_A).await.is_none());
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    assert!(reconcile_jobs(&harness.worker, jobs, &harness.metrics).await);
    assert!(reconcile_jobs(&harness.worker, jobs, &harness.metrics).await);
    assert!(harness.schedule(TENANT_A).await.is_some());
    assert!(harness.schedule(TENANT_B).await.is_none());

    // First run commits its effect, checkpoint, and next due time together.
    assert_eq!(harness.run_jobs().await, 1);
    assert_eq!(harness.run_jobs().await, 0, "not due again yet");
    let (checkpoint, run_count, _, claimed) = harness.schedule(TENANT_A).await.expect("schedule");
    assert_eq!(
        (checkpoint, run_count, claimed),
        (json!({"runs": 1}), 1, false)
    );
    assert_eq!(harness.job_effects(TENANT_A).await, 1);

    // Disabled: due work is not claimed, and nothing is deleted.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Disabled)
        .await;
    harness.make_due().await;
    assert!(reconcile_jobs(&harness.worker, jobs, &harness.metrics).await);
    assert_eq!(harness.run_jobs().await, 0);
    let (checkpoint, run_count, _, _) = harness.schedule(TENANT_A).await.expect("kept");
    assert_eq!((checkpoint, run_count), (json!({"runs": 1}), 1));

    // Claimed while enabled, disabled before execution: the run is refused in
    // its own transaction and the claim is released without consuming it.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    harness.make_due().await;
    let claims = platform_jobs::claim(
        &harness.worker,
        PLUGIN_ID,
        JOB_ID,
        NonZeroU32::MIN,
        Duration::from_secs(30),
    )
    .await
    .expect("claim");
    assert_eq!(claims.len(), 1);
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Disabled)
        .await;
    let gate = ActivationGate::for_manifest(
        &plugin_contract::Manifest::parse_toml(MANIFEST).expect("manifest"),
    );
    let outcome = platform_jobs::run_claimed(
        &harness.app,
        PLUGIN_ID,
        JOB_ID,
        &claims[0],
        &gate,
        harness.job_impl.as_ref(),
    )
    .await;
    assert_eq!(outcome, RunOutcome::Inactive);
    assert!(platform_jobs::release(
        &harness.worker,
        PLUGIN_ID,
        JOB_ID,
        &claims[0],
        harness.settings.job_retry_base,
    )
    .await
    .expect("release"));
    assert_eq!(harness.job_effects(TENANT_A).await, 1);
    // A released claim is deferred, so a schedule this worker cannot serve
    // never monopolises the claim queue.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    assert_eq!(harness.run_jobs().await, 0, "released run is deferred");
    harness.make_due().await;

    // An expired lease cannot commit effects, so a reclaimed run is never
    // applied twice.
    let claims = platform_jobs::claim(
        &harness.worker,
        PLUGIN_ID,
        JOB_ID,
        NonZeroU32::MIN,
        Duration::from_millis(1),
    )
    .await
    .expect("short claim");
    assert_eq!(claims.len(), 1);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let outcome = platform_jobs::run_claimed(
        &harness.app,
        PLUGIN_ID,
        JOB_ID,
        &claims[0],
        &gate,
        harness.job_impl.as_ref(),
    )
    .await;
    assert_eq!(outcome, RunOutcome::LeaseLost);
    assert_eq!(harness.job_effects(TENANT_A).await, 1);

    // A failing run rolls back its effects and records a bounded error code.
    harness.job_impl.fail.store(true, Ordering::SeqCst);
    assert_eq!(harness.run_jobs().await, 1);
    harness.job_impl.fail.store(false, Ordering::SeqCst);
    assert_eq!(harness.job_effects(TENANT_A).await, 1);
    let (_, run_count, error, claimed) = harness.schedule(TENANT_A).await.expect("schedule");
    assert_eq!(run_count, 1);
    assert_eq!(error.as_deref(), Some("acceptance.job_failed"));
    assert!(!claimed);
    assert_eq!(
        harness.run_jobs().await,
        0,
        "failure is retried after backoff"
    );

    // Re-enabled after all of the above: exactly one more run continues from
    // the committed checkpoint.
    harness.make_due().await;
    assert_eq!(harness.run_jobs().await, 1);
    let (checkpoint, run_count, error, _) = harness.schedule(TENANT_A).await.expect("schedule");
    assert_eq!(
        (checkpoint, run_count, error),
        (json!({"runs": 2}), 2, None)
    );
    assert_eq!(harness.job_effects(TENANT_A).await, 2);

    // The schedule is invisible to another tenant through the runtime role,
    // and claim tokens are never readable by it.
    assert!(harness.schedule(TENANT_B).await.is_none());
    let mut tx = platform_db::begin_plugin_tx(&harness.app, TENANT_A, PLUGIN_ID)
        .await
        .expect("plugin tx");
    assert!(sqlx::query("SELECT claim_token FROM plugin_job_schedules")
        .fetch_all(&mut *tx)
        .await
        .is_err());
    tx.rollback().await.expect("rollback");

    // Plugin execution context: the right tenant without the owning plugin's
    // context (kernel code, or another plugin) sees no schedule at all.
    for context in [None, Some("platform.contacts")] {
        let mut tx = match context {
            None => platform_db::begin_tenant_tx(&harness.app, TENANT_A).await,
            Some(plugin) => platform_db::begin_plugin_tx(&harness.app, TENANT_A, plugin).await,
        }
        .expect("tx");
        let visible: i64 =
            sqlx::query_scalar("SELECT count(*) FROM plugin_job_schedules WHERE plugin_id = $1")
                .bind(PLUGIN_ID)
                .fetch_one(&mut *tx)
                .await
                .expect("count schedules");
        assert_eq!(
            visible, 0,
            "{context:?} must not see the fixture plugin's schedules"
        );
        tx.rollback().await.expect("rollback");
    }
}

async fn wait_for(mut condition: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..100 {
        if condition().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

async fn publish(
    context: &async_nats::jetstream::Context,
    event: &EventEnvelope,
    message_id: &str,
) {
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", message_id);
    context
        .publish_with_headers(
            event.jetstream_subject().expect("subject"),
            headers,
            serde_json::to_vec(event).expect("payload").into(),
        )
        .await
        .expect("publish")
        .await
        .expect("broker acknowledgement");
}

#[tokio::test]
#[ignore = "requires scripts/test-plugin-runtime.sh PostgreSQL and NATS fixtures"]
async fn durable_broker_consumer_survives_restart_and_follows_lifecycle() {
    let Ok(nats_url) = std::env::var("PLATFORM_TEST_NATS_URL") else {
        panic!("PLATFORM_TEST_NATS_URL is set by the acceptance script");
    };
    let harness = Harness::new().await;
    harness.reset().await;
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Enabled)
        .await;
    let stream = format!("acceptance-{}", Uuid::now_v7().simple());
    // Create the stream through the production publisher contract.
    worker::outbox_publisher::JetStreamPublisher::connect(
        &nats_url,
        &stream,
        Duration::from_secs(60),
        64 * 1024 * 1024,
    )
    .await
    .expect("create event stream");
    let client = async_nats::connect(&nats_url).await.expect("connect nats");
    let context = async_nats::jetstream::new(client);
    let metrics = Arc::new(PluginRuntimeMetrics::default());
    let spawn = || {
        tokio::spawn(worker::plugin_runtime::run_consumer(
            nats_url.clone(),
            stream.clone(),
            harness.app.clone(),
            harness.consumer.clone(),
            harness.settings.clone(),
            metrics.clone(),
        ))
    };

    let mut running = spawn();
    let enabled = event(TENANT_A);
    let foreign = event(TENANT_B);
    publish(&context, &enabled, "enabled-1").await;
    publish(&context, &foreign, "foreign-1").await;
    // A second broker message with the same envelope models an outbox
    // re-publication after a lost acknowledgement.
    publish(&context, &enabled, "enabled-2").await;
    assert!(wait_for(async || harness.event_effects(&enabled).await == 1).await);
    assert!(
        wait_for(async || {
            let body = metrics.render();
            body.contains("limiara_plugin_deliveries_duplicate_total 1")
                && body.contains("limiara_plugin_deliveries_skipped_inactive_total 1")
        })
        .await,
        "{}",
        metrics.render()
    );
    assert_eq!(harness.event_effects(&foreign).await, 0);

    // Restart: the same durable registration resumes from its ack floor
    // without replaying acknowledged work into duplicate effects.
    running.abort();
    let _ = running.await;
    let after_restart = event(TENANT_A);
    publish(&context, &after_restart, "after-restart").await;
    running = spawn();
    assert!(wait_for(async || harness.event_effects(&after_restart).await == 1).await);
    assert_eq!(harness.event_effects(&enabled).await, 1);
    let mut broker_stream = context.get_stream(&stream).await.expect("stream");
    assert_eq!(
        broker_stream
            .info()
            .await
            .expect("stream info")
            .state
            .consumer_count,
        1
    );

    // Disabled mid-stream: later deliveries are acknowledged without effects.
    harness
        .transition(TENANT_A, platform_db::PluginDesiredState::Disabled)
        .await;
    let disabled = event(TENANT_A);
    publish(&context, &disabled, "disabled").await;
    assert!(
        wait_for(async || metrics
            .render()
            .contains("limiara_plugin_deliveries_skipped_inactive_total 2"))
        .await,
        "{}",
        metrics.render()
    );
    assert_eq!(harness.event_effects(&disabled).await, 0);
    assert_eq!(harness.markers(&disabled).await, 0);

    running.abort();
    let _ = running.await;
    context.delete_stream(&stream).await.expect("delete stream");
}
