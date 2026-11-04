//! Plugin-contributed background work, governed by tenant plugin activation.
//!
//! Event consumers and scheduled jobs are registered from trusted native
//! plugins whose linked implementations exactly match their manifest
//! declarations. Every unit of work re-checks the committed tenant activation
//! inside its own RLS-bound tenant transaction and holds a share lock on the
//! installation until commit, so:
//!
//! - disabled, unresolved, or version/digest-mismatched installations never
//!   start new work;
//! - a disable waits for in-flight work to settle (drain) instead of racing it;
//! - deliveries for an inactive installation are acknowledged without effects
//!   or inbox markers, and job schedules pause with their checkpoints intact;
//! - an unreadable lifecycle store fails closed and is retried.

use async_nats::jetstream::{
    self,
    consumer::{pull, AckPolicy, DeliverPolicy},
    AckKind,
};
use async_trait::async_trait;
use futures_util::StreamExt;
use platform_events::EventEnvelope;
use platform_inbox::{
    consume_gated, BrokerDisposition, ConsumerId, ConsumptionError, ConsumptionGate,
    ConsumptionStatus, EventHandler, GateDecision, HandlerFailure, HandlerFailureKind,
    UnsupportedSchemaDisposition,
};
use platform_jobs::{ClaimedJob, JobGate, RunOutcome, ScheduledJob};
use plugin_registry::activation::{resolve_active_manifest, ActivationInputs};
use plugin_runtime_native::{verify_worker_plugin, ContributionError, NativeWorkerPlugin};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Transaction};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    num::NonZeroU32,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

/// Runtime knobs derived from the validated platform configuration.
#[derive(Debug, Clone)]
pub struct PluginRuntimeSettings {
    pub reconcile_interval: Duration,
    pub job_poll_interval: Duration,
    pub job_batch_size: NonZeroU32,
    pub job_lease: Duration,
    pub job_retry_base: Duration,
    pub job_retry_max: Duration,
    pub consumer_ack_wait: Duration,
    pub consumer_max_deliver: u32,
    pub consumer_retry_delay: Duration,
}

impl PluginRuntimeSettings {
    pub fn from_platform(settings: &platform_config::Settings) -> Self {
        let worker = &settings.platform.worker;
        Self {
            reconcile_interval: Duration::from_millis(worker.reconcile_interval_ms),
            job_poll_interval: Duration::from_millis(worker.job_poll_interval_ms),
            job_batch_size: NonZeroU32::new(worker.job_batch_size).unwrap_or(NonZeroU32::MIN),
            job_lease: Duration::from_secs(worker.job_lease_seconds),
            job_retry_base: Duration::from_millis(worker.job_retry_base_ms),
            job_retry_max: Duration::from_millis(worker.job_retry_max_ms),
            consumer_ack_wait: Duration::from_secs(worker.consumer_ack_wait_seconds),
            consumer_max_deliver: worker.consumer_max_deliver,
            consumer_retry_delay: Duration::from_millis(worker.consumer_retry_delay_ms),
        }
    }
}

/// The activation authority for one linked plugin.
///
/// Work is permitted only when the tenant's committed installation resolves to
/// an active manifest (the same rule the API uses) whose version and artifact
/// digest are exactly the ones linked into this worker binary. A worker built
/// from a different plugin version therefore cannot act on another version's
/// activation.
#[derive(Debug, Clone)]
pub struct ActivationGate {
    plugin_id: String,
    linked_version: String,
    linked_digest: Option<String>,
}

impl ActivationGate {
    pub fn for_manifest(manifest: &plugin_contract::Manifest) -> Self {
        Self {
            plugin_id: manifest.id.clone(),
            linked_version: manifest.version.to_string(),
            linked_digest: manifest.supply_chain.artifact_digest.clone(),
        }
    }

    async fn evaluate(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
    ) -> Result<Activation, sqlx::Error> {
        let Some(record) =
            platform_db::lock_tenant_plugin_activation(transaction, tenant_id, &self.plugin_id)
                .await?
        else {
            return Ok(Activation::Inactive);
        };
        let Some(active) = resolve_active_manifest(ActivationInputs {
            plugin_id: &record.plugin_id,
            installation_state: record.installation_state.as_deref(),
            active_version: record.active_version.as_deref(),
            manifest: record.manifest.as_ref(),
            lifecycle_state: record.lifecycle_state.as_deref(),
            verified_digest: record.verified_digest.as_deref(),
        }) else {
            return Ok(Activation::Inactive);
        };
        let linked = active.version.to_string() == self.linked_version
            && active.supply_chain.artifact_digest.is_some()
            && active.supply_chain.artifact_digest == self.linked_digest;
        Ok(if linked {
            Activation::Active
        } else {
            // The tenant is active on a version this binary does not link
            // (rolling deploy or pending upgrade). Another worker may serve it,
            // so this is never treated as a disable.
            Activation::VersionSkew
        })
    }
}

/// The activation outcome for one tenant and linked plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activation {
    Active,
    Inactive,
    VersionSkew,
}

#[async_trait]
impl ConsumptionGate for ActivationGate {
    async fn decide(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<GateDecision, sqlx::Error> {
        Ok(match self.evaluate(transaction, event.tenant_id).await? {
            Activation::Active => GateDecision::Permit,
            Activation::Inactive => GateDecision::Skip,
            Activation::VersionSkew => GateDecision::Defer,
        })
    }
}

#[async_trait]
impl JobGate for ActivationGate {
    async fn permits(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        tenant_id: Uuid,
    ) -> Result<bool, sqlx::Error> {
        Ok(self.evaluate(transaction, tenant_id).await? == Activation::Active)
    }
}

/// A verified event consumer contribution.
pub struct RegisteredConsumer {
    pub plugin_id: String,
    pub id: ConsumerId,
    pub events: BTreeSet<String>,
    handler: Arc<dyn EventHandler>,
    gate: ActivationGate,
}

impl RegisteredConsumer {
    /// The stable JetStream durable name. NATS names cannot contain `.`, and
    /// a character substitution could collide, so the name is a digest of the
    /// consumer identity; the identity itself is kept as the description.
    pub fn durable_name(&self) -> String {
        let digest = Sha256::digest(self.id.as_str().as_bytes());
        let mut name = String::from("plugin-consumer-");
        for byte in &digest[..16] {
            let _ = write!(name, "{byte:02x}");
        }
        name
    }

    pub fn filter_subjects(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| format!("platform.events.v1.tenant.*.{event}"))
            .collect()
    }
}

/// Rejects any event type the manifest did not declare for this consumer,
/// even if a broker filter misconfiguration delivered it.
struct DeclaredEventsHandler<'a> {
    events: &'a BTreeSet<String>,
    inner: &'a dyn EventHandler,
}

#[async_trait]
impl EventHandler for DeclaredEventsHandler<'_> {
    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<(), HandlerFailure> {
        if !self.events.contains(&event.event_type) {
            return Err(undeclared_event_failure());
        }
        self.inner.handle(transaction, event).await
    }
}

fn undeclared_event_failure() -> HandlerFailure {
    // A static, pattern-valid control code; covered by a unit test.
    HandlerFailure::poison("consumer.undeclared_event").expect("static control code is valid")
}

/// A verified scheduled job contribution.
pub struct RegisteredJob {
    pub plugin_id: String,
    pub id: String,
    pub interval_seconds: u32,
    job: Arc<dyn ScheduledJob>,
    gate: ActivationGate,
}

/// All background contributions linked into this worker.
#[derive(Default)]
pub struct WorkerContributions {
    pub consumers: Vec<Arc<RegisteredConsumer>>,
    pub jobs: Vec<Arc<RegisteredJob>>,
}

impl WorkerContributions {
    /// Verify and register linked plugins. Any mismatch between code and
    /// manifest is a startup error rather than silently skipped work.
    pub fn from_plugins(
        plugins: &[Arc<dyn NativeWorkerPlugin>],
    ) -> Result<Self, ContributionError> {
        let mut contributions = Self::default();
        let mut plugin_ids = BTreeSet::new();
        for plugin in plugins {
            verify_worker_plugin(plugin.as_ref())?;
            let manifest = plugin.manifest();
            if !plugin_ids.insert(manifest.id.clone()) {
                return Err(ContributionError::Duplicate {
                    plugin_id: manifest.id.clone(),
                    id: manifest.id.clone(),
                });
            }
            let gate = ActivationGate::for_manifest(manifest);
            let mut handlers = plugin
                .consumers()
                .into_iter()
                .map(|consumer| (consumer.id.as_str().to_owned(), consumer.handler))
                .collect::<BTreeMap<_, _>>();
            for declared in &manifest.worker.consumers {
                let (Some(handler), Ok(id)) = (
                    handlers.remove(&declared.id),
                    ConsumerId::parse(&declared.id),
                ) else {
                    return Err(ContributionError::Unimplemented {
                        plugin_id: manifest.id.clone(),
                        id: declared.id.clone(),
                    });
                };
                contributions.consumers.push(Arc::new(RegisteredConsumer {
                    plugin_id: manifest.id.clone(),
                    id,
                    events: declared.events.iter().cloned().collect(),
                    handler,
                    gate: gate.clone(),
                }));
            }
            let mut jobs = plugin
                .jobs()
                .into_iter()
                .map(|job| (job.id, job.job))
                .collect::<BTreeMap<_, _>>();
            for declared in &manifest.worker.jobs {
                let Some(job) = jobs.remove(&declared.id) else {
                    return Err(ContributionError::Unimplemented {
                        plugin_id: manifest.id.clone(),
                        id: declared.id.clone(),
                    });
                };
                contributions.jobs.push(Arc::new(RegisteredJob {
                    plugin_id: manifest.id.clone(),
                    id: declared.id.clone(),
                    interval_seconds: declared.interval_seconds,
                    job,
                    gate: gate.clone(),
                }));
            }
        }
        Ok(contributions)
    }

    pub fn is_empty(&self) -> bool {
        self.consumers.is_empty() && self.jobs.is_empty()
    }
}

/// Aggregate, tenant-redacted runtime counters.
#[derive(Debug, Default)]
pub struct PluginRuntimeMetrics {
    deliveries_applied: AtomicU64,
    deliveries_duplicate: AtomicU64,
    deliveries_skipped_inactive: AtomicU64,
    deliveries_retried: AtomicU64,
    deliveries_deferred: AtomicU64,
    deliveries_terminated: AtomicU64,
    consumer_restarts: AtomicU64,
    jobs_succeeded: AtomicU64,
    jobs_inactive: AtomicU64,
    jobs_lease_lost: AtomicU64,
    jobs_failed: AtomicU64,
    reconcile_failures: AtomicU64,
    reconcile_healthy: AtomicBool,
    reconcile_tick_epoch_seconds: AtomicU64,
}

impl PluginRuntimeMetrics {
    fn observe_reconcile(&self, healthy: bool) {
        self.reconcile_healthy.store(healthy, Ordering::Relaxed);
        if !healthy {
            self.reconcile_failures.fetch_add(1, Ordering::Relaxed);
        }
        self.reconcile_tick_epoch_seconds
            .store(epoch_seconds(), Ordering::Relaxed);
    }

    /// Ready when the last reconciliation succeeded recently enough.
    pub fn reconcile_ready(&self, stale_after: Duration) -> bool {
        let tick = self.reconcile_tick_epoch_seconds.load(Ordering::Relaxed);
        self.reconcile_healthy.load(Ordering::Relaxed)
            && epoch_seconds().saturating_sub(tick) <= stale_after.as_secs().max(1)
    }

    /// Prometheus text, following the worker's metrics contract: one
    /// label-free counter per outcome, with no tenant, workspace, event, or
    /// subject dimension.
    pub fn render(&self) -> String {
        let counters: [(&str, &AtomicU64); 12] = [
            (
                "limiara_plugin_deliveries_applied_total",
                &self.deliveries_applied,
            ),
            (
                "limiara_plugin_deliveries_duplicate_total",
                &self.deliveries_duplicate,
            ),
            (
                "limiara_plugin_deliveries_skipped_inactive_total",
                &self.deliveries_skipped_inactive,
            ),
            (
                "limiara_plugin_deliveries_retried_total",
                &self.deliveries_retried,
            ),
            (
                "limiara_plugin_deliveries_deferred_total",
                &self.deliveries_deferred,
            ),
            (
                "limiara_plugin_deliveries_terminated_total",
                &self.deliveries_terminated,
            ),
            (
                "limiara_plugin_job_runs_succeeded_total",
                &self.jobs_succeeded,
            ),
            (
                "limiara_plugin_job_runs_inactive_total",
                &self.jobs_inactive,
            ),
            (
                "limiara_plugin_job_runs_lease_lost_total",
                &self.jobs_lease_lost,
            ),
            ("limiara_plugin_job_runs_failed_total", &self.jobs_failed),
            (
                "limiara_plugin_reconcile_failures_total",
                &self.reconcile_failures,
            ),
            (
                "limiara_plugin_consumer_restarts_total",
                &self.consumer_restarts,
            ),
        ];
        let mut body = String::new();
        for (name, value) in counters {
            let _ = writeln!(
                body,
                "# TYPE {name} counter\n{name} {}",
                value.load(Ordering::Relaxed)
            );
        }
        body
    }
}

/// Record a successful reconciliation for runtimes with nothing to reconcile.
pub fn mark_reconciled(metrics: &PluginRuntimeMetrics) {
    metrics.observe_reconcile(true);
}

/// Process one broker delivery through the gated inbox and map the result to
/// a broker disposition.
///
/// Infrastructure failures (database or lifecycle-store outage) and version
/// skew are retried without limit, because neither says anything about the
/// event itself. A handler's retryable failure, or a schema this build cannot
/// read, becomes an explicit terminate after `max_deliver` attempts, so poison
/// handling is visible rather than an unbounded hot loop.
pub async fn process_delivery(
    tenant_pool: &PgPool,
    consumer: &RegisteredConsumer,
    subject: &str,
    payload: &[u8],
    delivered: u64,
    max_deliver: u32,
    metrics: &PluginRuntimeMetrics,
) -> BrokerDisposition {
    let handler = DeclaredEventsHandler {
        events: &consumer.events,
        inner: consumer.handler.as_ref(),
    };
    let result = consume_gated(
        tenant_pool,
        &consumer.plugin_id,
        &consumer.id,
        subject,
        payload,
        &consumer.gate,
        &handler,
    )
    .await;
    let (disposition, control) = match &result {
        Ok(status) => (status.disposition(), status.control()),
        Err(error) => (
            error.disposition(UnsupportedSchemaDisposition::Retry),
            error.control(),
        ),
    };
    let bounded_retry = matches!(
        &result,
        Err(ConsumptionError::Handler(failure)) if failure.kind() == HandlerFailureKind::Retryable
    ) || matches!(&result, Err(ConsumptionError::UnsupportedSchemaVersion));
    let disposition = match disposition {
        BrokerDisposition::Retry if bounded_retry && delivered >= u64::from(max_deliver) => {
            BrokerDisposition::Terminate
        }
        other => other,
    };
    match (&result, disposition) {
        (Ok(ConsumptionStatus::Applied), _) => &metrics.deliveries_applied,
        (Ok(ConsumptionStatus::Duplicate), _) => &metrics.deliveries_duplicate,
        (Ok(ConsumptionStatus::SkippedInactive), _) => &metrics.deliveries_skipped_inactive,
        (Err(ConsumptionError::Deferred), _) => &metrics.deliveries_deferred,
        (Err(_), BrokerDisposition::Retry) => &metrics.deliveries_retried,
        (Err(_), _) => &metrics.deliveries_terminated,
    }
    .fetch_add(1, Ordering::Relaxed);
    if let Err(error) = &result {
        log_consumption_error(consumer, error, control, disposition);
    }
    disposition
}

fn log_consumption_error(
    consumer: &RegisteredConsumer,
    error: &ConsumptionError,
    control: &str,
    disposition: BrokerDisposition,
) {
    if disposition == BrokerDisposition::Terminate {
        tracing::error!(
            consumer_id = %consumer.id,
            control,
            error_type = %std::any::type_name_of_val(error),
            "plugin consumer terminated a delivery"
        );
    } else {
        tracing::warn!(
            consumer_id = %consumer.id,
            control,
            "plugin consumer delivery will be retried"
        );
    }
}

/// Register schedules for every active tenant installation of every job.
pub async fn reconcile_jobs(
    worker_pool: &PgPool,
    jobs: &[Arc<RegisteredJob>],
    metrics: &PluginRuntimeMetrics,
) -> bool {
    let mut healthy = true;
    for job in jobs {
        match platform_jobs::ensure_schedules(
            worker_pool,
            &job.plugin_id,
            &job.id,
            job.interval_seconds,
        )
        .await
        {
            Ok(registered) if registered > 0 => {
                tracing::info!(job_id = %job.id, registered, "registered plugin job schedules");
            }
            Ok(_) => {}
            Err(error) => {
                healthy = false;
                tracing::warn!(job_id = %job.id, error = %error, "plugin job reconciliation failed");
            }
        }
    }
    metrics.observe_reconcile(healthy);
    healthy
}

/// Claim and run one bounded batch of due runs for a job.
pub async fn run_due_jobs(
    worker_pool: &PgPool,
    tenant_pool: &PgPool,
    job: &RegisteredJob,
    settings: &PluginRuntimeSettings,
    metrics: &PluginRuntimeMetrics,
) -> Result<usize, platform_jobs::JobStoreError> {
    // Claim one run at a time so every run gets its own token and a full
    // lease that starts when the run starts, never part-way through a batch.
    let mut ran = 0;
    while ran < usize::try_from(settings.job_batch_size.get()).unwrap_or(usize::MAX) {
        let claims = platform_jobs::claim(
            worker_pool,
            &job.plugin_id,
            &job.id,
            NonZeroU32::MIN,
            settings.job_lease,
        )
        .await?;
        let Some(claim) = claims.first() else {
            break;
        };
        let outcome = platform_jobs::run_claimed(
            tenant_pool,
            &job.plugin_id,
            &job.id,
            claim,
            &job.gate,
            job.job.as_ref(),
        )
        .await;
        settle_job_run(worker_pool, job, claim, outcome, settings, metrics).await;
        ran += 1;
    }
    Ok(ran)
}

async fn settle_job_run(
    worker_pool: &PgPool,
    job: &RegisteredJob,
    claim: &ClaimedJob,
    outcome: RunOutcome,
    settings: &PluginRuntimeSettings,
    metrics: &PluginRuntimeMetrics,
) {
    let settled = match outcome {
        RunOutcome::Succeeded => {
            metrics.jobs_succeeded.fetch_add(1, Ordering::Relaxed);
            return;
        }
        RunOutcome::LeaseLost => {
            metrics.jobs_lease_lost.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(job_id = %job.id, "plugin job lease expired before commit; effects rolled back");
            return;
        }
        RunOutcome::Inactive => {
            metrics.jobs_inactive.fetch_add(1, Ordering::Relaxed);
            platform_jobs::release(
                worker_pool,
                &job.plugin_id,
                &job.id,
                claim,
                settings.job_retry_base,
            )
            .await
        }
        RunOutcome::Failed(control) => {
            metrics.jobs_failed.fetch_add(1, Ordering::Relaxed);
            tracing::warn!(job_id = %job.id, control, "plugin job run failed");
            let retry = platform_jobs::retry_delay(
                settings.job_retry_base,
                settings.job_retry_max,
                claim.attempt_count,
            );
            platform_jobs::fail(worker_pool, &job.plugin_id, &job.id, claim, retry, control).await
        }
    };
    match settled {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(job_id = %job.id, control = outcome.control(), "plugin job claim was stale at settlement");
        }
        Err(error) => {
            // The lease expires on its own, so an unsettled claim is retried
            // after the lease window rather than lost.
            tracing::warn!(job_id = %job.id, error = %error, "plugin job settlement failed");
        }
    }
}

/// Run reconciliation and every job until the process stops.
pub async fn run_jobs(
    worker_pool: PgPool,
    tenant_pool: PgPool,
    jobs: Vec<Arc<RegisteredJob>>,
    settings: PluginRuntimeSettings,
    metrics: Arc<PluginRuntimeMetrics>,
) {
    // Reconciliation drives readiness, so it runs as its own future and a
    // slow job batch cannot make a healthy worker look stale.
    let reconcile = async {
        let mut ticks = tokio::time::interval(settings.reconcile_interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            reconcile_jobs(&worker_pool, &jobs, &metrics).await;
        }
    };
    let poll = async {
        let mut ticks = tokio::time::interval(settings.job_poll_interval);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticks.tick().await;
            for job in &jobs {
                if let Err(error) =
                    run_due_jobs(&worker_pool, &tenant_pool, job, &settings, &metrics).await
                {
                    tracing::warn!(job_id = %job.id, error = %error, "plugin job claim failed");
                }
            }
        }
    };
    tokio::join!(reconcile, poll);
}

/// Keep one consumer running for the life of the process. A panic in plugin
/// handler code ends only that task; the supervisor counts it and restarts
/// the consumer from its durable acknowledgement floor.
pub async fn supervise_consumer(
    nats_url: String,
    stream: String,
    tenant_pool: PgPool,
    consumer: Arc<RegisteredConsumer>,
    settings: PluginRuntimeSettings,
    metrics: Arc<PluginRuntimeMetrics>,
) {
    loop {
        let task = tokio::spawn(run_consumer(
            nats_url.clone(),
            stream.clone(),
            tenant_pool.clone(),
            consumer.clone(),
            settings.clone(),
            metrics.clone(),
        ));
        if let Err(error) = task.await {
            metrics.consumer_restarts.fetch_add(1, Ordering::Relaxed);
            tracing::error!(
                consumer_id = %consumer.id,
                panicked = error.is_panic(),
                "plugin consumer task stopped; restarting"
            );
        }
        tokio::time::sleep(settings.consumer_retry_delay).await;
    }
}

/// Run one durable JetStream consumer until the process stops, reconnecting
/// with a bounded delay after broker failures.
pub async fn run_consumer(
    nats_url: String,
    stream: String,
    tenant_pool: PgPool,
    consumer: Arc<RegisteredConsumer>,
    settings: PluginRuntimeSettings,
    metrics: Arc<PluginRuntimeMetrics>,
) {
    loop {
        if let Err(error) = consume_until_error(
            &nats_url,
            &stream,
            &tenant_pool,
            &consumer,
            &settings,
            &metrics,
        )
        .await
        {
            tracing::warn!(
                consumer_id = %consumer.id,
                error = %error,
                "plugin consumer disconnected; retrying"
            );
        }
        tokio::time::sleep(settings.consumer_retry_delay).await;
    }
}

async fn consume_until_error(
    nats_url: &str,
    stream: &str,
    tenant_pool: &PgPool,
    consumer: &RegisteredConsumer,
    settings: &PluginRuntimeSettings,
    metrics: &PluginRuntimeMetrics,
) -> anyhow::Result<()> {
    let client = async_nats::connect(nats_url).await?;
    let context = jetstream::new(client);
    let stream = context.get_stream(stream).await?;
    // Create-or-update keeps exactly one durable registration per consumer
    // identity across restarts and replicas.
    let durable = stream
        .create_consumer(pull::Config {
            durable_name: Some(consumer.durable_name()),
            description: Some(consumer.id.as_str().to_owned()),
            ack_policy: AckPolicy::Explicit,
            ack_wait: settings.consumer_ack_wait,
            // Unlimited broker redelivery: process_delivery decides when a
            // failure is poison, so an outage never silently drops events.
            max_deliver: -1,
            deliver_policy: DeliverPolicy::All,
            filter_subjects: consumer.filter_subjects(),
            ..Default::default()
        })
        .await?;
    tracing::info!(consumer_id = %consumer.id, "plugin consumer registered");
    let mut messages = durable.messages().await?;
    while let Some(message) = messages.next().await {
        let message = message?;
        let delivered = message.info().map(|info| info.delivered).unwrap_or(1);
        let delivered = u64::try_from(delivered).unwrap_or(1);
        let disposition = process_delivery(
            tenant_pool,
            consumer,
            message.subject.as_str(),
            &message.payload,
            delivered,
            settings.consumer_max_deliver,
            metrics,
        )
        .await;
        let ack = match disposition {
            BrokerDisposition::Acknowledge => AckKind::Ack,
            BrokerDisposition::Retry => AckKind::Nak(Some(platform_jobs::retry_delay(
                settings.consumer_retry_delay,
                settings.consumer_ack_wait,
                i32::try_from(delivered).unwrap_or(i32::MAX),
            ))),
            BrokerDisposition::Terminate => AckKind::Term,
        };
        // A lost acknowledgement causes redelivery, which the inbox dedupes.
        if let Err(error) = message.ack_with(ack).await {
            tracing::warn!(consumer_id = %consumer.id, error = %error, "plugin consumer acknowledgement failed");
        }
    }
    anyhow::bail!("consumer message stream ended")
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_runtime_native::{NativeConsumer, NativeJob};
    use serde_json::Value;

    pub(crate) const FIXTURE_MANIFEST: &str = r#"
manifest_version = 1
id = "platform.fixture"
name = "Fixture"
version = "0.1.0"
description = "Worker contribution fixture"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[supply_chain]
artifact_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000"

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

    struct Noop;

    #[async_trait]
    impl EventHandler for Noop {
        async fn handle(
            &self,
            _transaction: &mut Transaction<'_, Postgres>,
            _event: &EventEnvelope,
        ) -> Result<(), HandlerFailure> {
            Ok(())
        }
    }

    #[async_trait]
    impl ScheduledJob for Noop {
        async fn run(
            &self,
            _transaction: &mut Transaction<'_, Postgres>,
            claim: &ClaimedJob,
        ) -> Result<Value, platform_jobs::JobFailure> {
            Ok(claim.checkpoint.clone())
        }
    }

    struct Fixture {
        manifest: plugin_contract::Manifest,
        consumers: Vec<&'static str>,
        jobs: Vec<&'static str>,
    }

    impl NativeWorkerPlugin for Fixture {
        fn manifest(&self) -> &plugin_contract::Manifest {
            &self.manifest
        }

        fn consumers(&self) -> Vec<NativeConsumer> {
            self.consumers
                .iter()
                .map(|id| NativeConsumer {
                    id: ConsumerId::parse(id).unwrap(),
                    handler: Arc::new(Noop),
                })
                .collect()
        }

        fn jobs(&self) -> Vec<NativeJob> {
            self.jobs
                .iter()
                .map(|id| NativeJob {
                    id: (*id).to_owned(),
                    job: Arc::new(Noop),
                })
                .collect()
        }
    }

    fn fixture(
        consumers: Vec<&'static str>,
        jobs: Vec<&'static str>,
    ) -> Arc<dyn NativeWorkerPlugin> {
        Arc::new(Fixture {
            manifest: plugin_contract::Manifest::parse_toml(FIXTURE_MANIFEST).unwrap(),
            consumers,
            jobs,
        })
    }

    #[test]
    fn linked_contributions_must_match_manifest_declarations_exactly() {
        let contributions = WorkerContributions::from_plugins(&[fixture(
            vec!["platform.fixture.probe-v1"],
            vec!["platform.fixture.tick-v1"],
        )])
        .unwrap();
        assert_eq!(contributions.consumers.len(), 1);
        assert_eq!(contributions.jobs[0].interval_seconds, 60);
        let consumer = &contributions.consumers[0];
        assert_eq!(
            consumer.filter_subjects(),
            vec!["platform.events.v1.tenant.*.platform.contacts.person-created.v1"]
        );
        assert!(consumer.durable_name().starts_with("plugin-consumer-"));
        assert_eq!(consumer.durable_name().len(), "plugin-consumer-".len() + 32);
        assert!(!consumer.durable_name().contains('.'));

        let undeclared = WorkerContributions::from_plugins(&[fixture(
            vec!["platform.fixture.probe-v1", "platform.fixture.hidden-v1"],
            vec!["platform.fixture.tick-v1"],
        )]);
        assert!(matches!(
            undeclared,
            Err(ContributionError::Undeclared { .. })
        ));

        let missing = WorkerContributions::from_plugins(&[fixture(
            vec!["platform.fixture.probe-v1"],
            vec![],
        )]);
        assert!(matches!(
            missing,
            Err(ContributionError::Unimplemented { .. })
        ));

        let duplicate = WorkerContributions::from_plugins(&[fixture(
            vec!["platform.fixture.probe-v1", "platform.fixture.probe-v1"],
            vec!["platform.fixture.tick-v1"],
        )]);
        assert!(matches!(
            duplicate,
            Err(ContributionError::Duplicate { .. })
        ));

        let twice = WorkerContributions::from_plugins(&[
            fixture(
                vec!["platform.fixture.probe-v1"],
                vec!["platform.fixture.tick-v1"],
            ),
            fixture(
                vec!["platform.fixture.probe-v1"],
                vec!["platform.fixture.tick-v1"],
            ),
        ]);
        assert!(matches!(twice, Err(ContributionError::Duplicate { .. })));
    }

    #[test]
    fn undeclared_events_are_poison() {
        let failure = undeclared_event_failure();
        assert_eq!(failure.control(), "consumer.undeclared_event");
        assert_eq!(failure.kind(), platform_inbox::HandlerFailureKind::Poison);
    }

    #[test]
    fn metrics_render_bounded_outcome_labels_without_tenant_data() {
        let metrics = PluginRuntimeMetrics::default();
        metrics
            .deliveries_skipped_inactive
            .fetch_add(2, Ordering::Relaxed);
        metrics.jobs_inactive.fetch_add(1, Ordering::Relaxed);
        let body = metrics.render();
        assert!(body.contains("\nlimiara_plugin_deliveries_skipped_inactive_total 2\n"));
        assert!(body.contains("\nlimiara_plugin_job_runs_inactive_total 1\n"));
        // The same privacy rule the outbox acceptance applies to /metrics.
        for forbidden in ["tenant", "workspace", "event_type", "subject", "{"] {
            assert!(!body.to_lowercase().contains(forbidden), "{forbidden}");
        }
        assert!(!metrics.reconcile_ready(Duration::from_secs(60)));
        metrics.observe_reconcile(true);
        assert!(metrics.reconcile_ready(Duration::from_secs(60)));
        metrics.observe_reconcile(false);
        assert!(!metrics.reconcile_ready(Duration::from_secs(60)));
    }
}
