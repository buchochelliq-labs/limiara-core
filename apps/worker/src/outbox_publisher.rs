#[cfg(test)]
use async_nats::jetstream::stream::{Republish, SubjectTransform};
use async_nats::jetstream::{
    self,
    message::PublishMessage,
    stream::{Config as StreamConfig, DiscardPolicy, RetentionPolicy, StorageType},
};
use async_trait::async_trait;
use chrono::Utc;
use platform_events::{
    EventContractError, EventEnvelope, EventSubjectError, JETSTREAM_V1_SUBJECTS,
};
use platform_outbox::{
    backlog, claim_batch, mark_failed, mark_published, mark_quarantined, ClaimedOutboxRecord,
    OutboxBacklog,
};
use sqlx::PgPool;
use std::{
    num::NonZeroU32,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::task::JoinSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishFailure {
    InvalidEnvelope,
    EnvelopeMismatch,
    InvalidSubject,
    BrokerUnavailable,
    BrokerRejected,
    BrokerTimeout,
    OversizedEnvelope,
    UnsupportedSchemaVersion,
}

impl PublishFailure {
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidEnvelope => "invalid_envelope",
            Self::EnvelopeMismatch => "envelope_mismatch",
            Self::InvalidSubject => "invalid_subject",
            Self::BrokerUnavailable => "broker_unavailable",
            Self::BrokerRejected => "broker_rejected",
            Self::BrokerTimeout => "broker_timeout",
            Self::OversizedEnvelope => "oversized_envelope",
            Self::UnsupportedSchemaVersion => "unsupported_schema_version",
        }
    }

    const fn retryable(self) -> bool {
        matches!(
            self,
            Self::BrokerUnavailable
                | Self::BrokerRejected
                | Self::BrokerTimeout
                | Self::UnsupportedSchemaVersion
        )
    }
}

#[derive(Debug, Clone)]
pub struct PreparedPublication {
    pub id: uuid::Uuid,
    pub deduplication_id: String,
    pub subject: String,
    pub payload: Vec<u8>,
}

pub fn prepare_publication(
    record: &ClaimedOutboxRecord,
) -> Result<PreparedPublication, PublishFailure> {
    let envelope: EventEnvelope = serde_json::from_value(record.envelope.clone())
        .map_err(|_| PublishFailure::InvalidEnvelope)?;
    if envelope.id != record.id
        || envelope.tenant_id != record.tenant_id
        || envelope.event_type != record.event_type
    {
        return Err(PublishFailure::EnvelopeMismatch);
    }
    let subject = envelope.jetstream_subject().map_err(subject_failure)?;
    // Canonicalise the known envelope so corrupt, unrecognised top-level keys
    // are never forwarded. The bounded payload remains plugin-owned.
    let payload = envelope.publication_bytes().map_err(|error| match error {
        EventContractError::Subject(error) => subject_failure(error),
        EventContractError::Serialization(_) => PublishFailure::InvalidEnvelope,
        EventContractError::OversizedEnvelope => PublishFailure::OversizedEnvelope,
    })?;
    Ok(PreparedPublication {
        id: record.id,
        deduplication_id: format!("{}:{}", record.tenant_id, record.id),
        subject,
        payload,
    })
}

fn subject_failure(error: EventSubjectError) -> PublishFailure {
    match error {
        EventSubjectError::UnsupportedSchemaVersion => PublishFailure::UnsupportedSchemaVersion,
        EventSubjectError::UnsafeEventType => PublishFailure::InvalidSubject,
    }
}

#[async_trait]
pub trait BrokerPublisher: Send + Sync {
    async fn publish(&self, publication: &PreparedPublication) -> Result<(), PublishFailure>;
}

#[derive(Clone)]
pub struct JetStreamPublisher {
    client: async_nats::Client,
    context: jetstream::Context,
    stream: String,
    duplicate_window: Duration,
    stream_max_bytes: i64,
}

impl JetStreamPublisher {
    pub async fn connect(
        url: &str,
        stream: &str,
        duplicate_window: Duration,
        stream_max_bytes: i64,
    ) -> anyhow::Result<Self> {
        let client = async_nats::connect(url).await?;
        let context = jetstream::new(client.clone());
        let handle = context
            .get_or_create_stream(StreamConfig {
                name: stream.to_owned(),
                description: Some("Limiara versioned domain events".into()),
                subjects: vec![JETSTREAM_V1_SUBJECTS.into()],
                storage: StorageType::File,
                retention: RetentionPolicy::Limits,
                discard: DiscardPolicy::New,
                duplicate_window,
                max_bytes: stream_max_bytes,
                ..Default::default()
            })
            .await?;
        let config = &handle.cached_info().config;
        anyhow::ensure!(
            stream_config_is_compatible(config, duplicate_window, stream_max_bytes),
            "configured JetStream stream is incompatible with the Limiara event contract"
        );
        Ok(Self {
            client,
            context,
            stream: stream.to_owned(),
            duplicate_window,
            stream_max_bytes,
        })
    }

    pub async fn ready(&self) -> bool {
        self.client.connection_state() == async_nats::connection::State::Connected
            && self
                .context
                .get_stream(&self.stream)
                .await
                .is_ok_and(|stream| {
                    stream_config_is_compatible(
                        &stream.cached_info().config,
                        self.duplicate_window,
                        self.stream_max_bytes,
                    )
                })
    }
}

fn stream_config_is_compatible(
    config: &StreamConfig,
    duplicate_window: Duration,
    stream_max_bytes: i64,
) -> bool {
    config.storage == StorageType::File
        && config.retention == RetentionPolicy::Limits
        && config.discard == DiscardPolicy::New
        && config.max_bytes == stream_max_bytes
        && config.duplicate_window == duplicate_window
        && config.subjects.as_slice() == [JETSTREAM_V1_SUBJECTS]
        && !config.no_ack
        && !config.sealed
        && config.republish.is_none()
        && config.subject_transform.is_none()
        && config.mirror.is_none()
        && config.sources.as_ref().is_none_or(Vec::is_empty)
}

#[async_trait]
impl BrokerPublisher for JetStreamPublisher {
    async fn publish(&self, publication: &PreparedPublication) -> Result<(), PublishFailure> {
        let message = PublishMessage::build()
            .payload(publication.payload.clone().into())
            .message_id(publication.deduplication_id.clone())
            .expected_stream(&self.stream);
        let acknowledgement = self
            .context
            .send_publish(publication.subject.clone(), message)
            .await
            .map_err(|_| PublishFailure::BrokerUnavailable)?;
        acknowledgement
            .await
            .map_err(|_| PublishFailure::BrokerRejected)?;
        Ok(())
    }
}

#[derive(Default)]
pub struct PublisherMetrics {
    publish_attempts: AtomicU64,
    publish_failures: AtomicU64,
    published: AtomicU64,
    publish_latency_micros: AtomicU64,
    publish_latency_count: AtomicU64,
    publisher_loop_healthy: AtomicBool,
    publisher_loop_tick_epoch_seconds: AtomicU64,
}

impl PublisherMetrics {
    fn observe_attempt(&self) {
        self.publish_attempts.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_failure(&self) {
        self.publish_failures.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_success(&self, latency: Duration) {
        self.published.fetch_add(1, Ordering::Relaxed);
        self.publish_latency_micros.fetch_add(
            u64::try_from(latency.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.publish_latency_count.fetch_add(1, Ordering::Relaxed);
    }

    fn observe_claim_result(&self, healthy: bool) {
        self.publisher_loop_healthy
            .store(healthy, Ordering::Relaxed);
        self.publisher_loop_tick_epoch_seconds.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            Ordering::Relaxed,
        );
    }

    pub fn publisher_loop_ready(&self, maximum_age: Duration) -> bool {
        if !self.publisher_loop_healthy.load(Ordering::Relaxed) {
            return false;
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        now.saturating_sub(
            self.publisher_loop_tick_epoch_seconds
                .load(Ordering::Relaxed),
        ) <= maximum_age.as_secs()
    }

    pub fn render(&self, current: &OutboxBacklog) -> String {
        let oldest_age = current
            .oldest_pending_at
            .map(|created_at| {
                (Utc::now() - created_at)
                    .to_std()
                    .unwrap_or_default()
                    .as_secs_f64()
            })
            .unwrap_or(0.0);
        let latency_seconds =
            self.publish_latency_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;
        format!(
            concat!(
                "# TYPE limiara_outbox_backlog gauge\n",
                "limiara_outbox_backlog {}\n",
                "# TYPE limiara_outbox_claimable gauge\n",
                "limiara_outbox_claimable {}\n",
                "# TYPE limiara_outbox_claimed gauge\n",
                "limiara_outbox_claimed {}\n",
                "# TYPE limiara_outbox_quarantined gauge\n",
                "limiara_outbox_quarantined {}\n",
                "# TYPE limiara_outbox_publish_attempts_total counter\n",
                "limiara_outbox_publish_attempts_total {}\n",
                "# TYPE limiara_outbox_publish_failures_total counter\n",
                "limiara_outbox_publish_failures_total {}\n",
                "# TYPE limiara_outbox_published_total counter\n",
                "limiara_outbox_published_total {}\n",
                "# TYPE limiara_outbox_oldest_pending_age_seconds gauge\n",
                "limiara_outbox_oldest_pending_age_seconds {:.6}\n",
                "# TYPE limiara_outbox_publish_latency_seconds summary\n",
                "limiara_outbox_publish_latency_seconds_sum {:.6}\n",
                "limiara_outbox_publish_latency_seconds_count {}\n"
            ),
            current.pending,
            current.claimable,
            current.claimed,
            current.quarantined,
            self.publish_attempts.load(Ordering::Relaxed),
            self.publish_failures.load(Ordering::Relaxed),
            self.published.load(Ordering::Relaxed),
            oldest_age,
            latency_seconds,
            self.publish_latency_count.load(Ordering::Relaxed),
        )
    }
}

#[derive(Clone)]
pub struct RuntimeSettings {
    pub batch_size: NonZeroU32,
    pub max_in_flight: usize,
    pub poll_interval: Duration,
    pub claim_lease: Duration,
    pub publish_timeout: Duration,
    pub database_timeout: Duration,
    pub retry_base: Duration,
    pub retry_max: Duration,
}

impl RuntimeSettings {
    pub fn from_platform(settings: &platform_config::Settings) -> Self {
        let outbox = &settings.platform.nats.outbox;
        let usable_database_connections = settings
            .platform
            .database
            .max_connections
            .saturating_sub(1)
            .max(1);
        Self {
            batch_size: NonZeroU32::new(outbox.batch_size)
                .expect("validated non-zero outbox batch size"),
            max_in_flight: outbox.max_in_flight.min(usable_database_connections) as usize,
            poll_interval: Duration::from_millis(outbox.poll_interval_ms),
            claim_lease: Duration::from_secs(outbox.claim_lease_seconds),
            publish_timeout: Duration::from_millis(outbox.publish_timeout_ms),
            database_timeout: Duration::from_millis(
                settings.platform.database.statement_timeout_ms,
            ),
            retry_base: Duration::from_millis(outbox.retry_base_ms),
            retry_max: Duration::from_millis(outbox.retry_max_ms),
        }
    }
}

pub async fn run(
    pool: PgPool,
    publisher: Arc<dyn BrokerPublisher>,
    settings: RuntimeSettings,
    metrics: Arc<PublisherMetrics>,
) {
    loop {
        match tokio::time::timeout(
            settings.database_timeout,
            claim_batch(&pool, settings.batch_size, settings.claim_lease),
        )
        .await
        {
            Ok(Ok(records)) if records.is_empty() => {
                metrics.observe_claim_result(true);
                tokio::time::sleep(settings.poll_interval).await;
            }
            Ok(Ok(records)) => {
                metrics.observe_claim_result(true);
                process_batch(
                    pool.clone(),
                    publisher.clone(),
                    settings.clone(),
                    metrics.clone(),
                    records,
                )
                .await;
            }
            Ok(Err(error)) => {
                metrics.observe_claim_result(false);
                tracing::error!(error = %error, "outbox claim failed");
                tokio::time::sleep(settings.retry_base).await;
            }
            Err(_) => {
                metrics.observe_claim_result(false);
                tracing::error!("outbox claim timed out");
                tokio::time::sleep(settings.retry_base).await;
            }
        }
    }
}

async fn process_batch(
    pool: PgPool,
    publisher: Arc<dyn BrokerPublisher>,
    settings: RuntimeSettings,
    metrics: Arc<PublisherMetrics>,
    records: Vec<ClaimedOutboxRecord>,
) {
    let mut records = records.into_iter();
    let mut tasks = JoinSet::new();
    loop {
        while tasks.len() < settings.max_in_flight {
            let Some(record) = records.next() else {
                break;
            };
            let pool = pool.clone();
            let publisher = publisher.clone();
            let settings = settings.clone();
            let metrics = metrics.clone();
            tasks.spawn(async move {
                process_record(&pool, publisher.as_ref(), &settings, &metrics, record).await;
            });
        }
        if tasks.is_empty() {
            break;
        }
        if let Some(Err(error)) = tasks.join_next().await {
            tracing::error!(error = %error, "outbox publication task failed");
        }
    }
}

async fn process_record(
    pool: &PgPool,
    publisher: &dyn BrokerPublisher,
    settings: &RuntimeSettings,
    metrics: &PublisherMetrics,
    record: ClaimedOutboxRecord,
) {
    let started = Instant::now();
    metrics.observe_attempt();
    let result = match prepare_publication(&record) {
        Ok(publication) => {
            match tokio::time::timeout(settings.publish_timeout, publisher.publish(&publication))
                .await
            {
                Ok(result) => result,
                Err(_) => Err(PublishFailure::BrokerTimeout),
            }
        }
        Err(error) => Err(error),
    };

    match result {
        Ok(()) => match tokio::time::timeout(
            settings.database_timeout,
            mark_published(pool, record.tenant_id, record.id, record.claim_token),
        )
        .await
        {
            Ok(Ok(true)) => {
                let latency = started.elapsed();
                metrics.observe_success(latency);
                tracing::info!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    publish_latency_ms = latency.as_millis(),
                    "outbox event acknowledged and finalised"
                );
            }
            Ok(Ok(false)) => tracing::warn!(
                event_id = %record.id,
                attempt = record.attempt_count,
                "acknowledged outbox event had a stale claim"
            ),
            Ok(Err(error)) => tracing::error!(
                event_id = %record.id,
                attempt = record.attempt_count,
                error = %error,
                "acknowledged outbox event could not be finalised"
            ),
            Err(_) => tracing::error!(
                event_id = %record.id,
                attempt = record.attempt_count,
                "acknowledged outbox event finalisation timed out"
            ),
        },
        Err(failure) if failure.retryable() => {
            metrics.observe_failure();
            let retry = retry_delay(settings, record.attempt_count);
            match tokio::time::timeout(
                settings.database_timeout,
                mark_failed(
                    pool,
                    record.tenant_id,
                    record.id,
                    record.claim_token,
                    retry,
                    failure.code(),
                ),
            )
            .await
            {
                Ok(Ok(true)) => tracing::warn!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    retry_delay_ms = retry.as_millis(),
                    "outbox publication will be retried"
                ),
                Ok(Ok(false)) => tracing::warn!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    "outbox publication failure had a stale claim"
                ),
                Ok(Err(error)) => tracing::error!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    error = %error,
                    "outbox publication failure could not be recorded"
                ),
                Err(_) => tracing::error!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    "outbox publication failure recording timed out"
                ),
            }
        }
        Err(failure) => {
            metrics.observe_failure();
            match tokio::time::timeout(
                settings.database_timeout,
                mark_quarantined(
                    pool,
                    record.tenant_id,
                    record.id,
                    record.claim_token,
                    failure.code(),
                ),
            )
            .await
            {
                Ok(Ok(true)) => tracing::error!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    "deterministically invalid outbox event was quarantined"
                ),
                Ok(Ok(false)) => tracing::warn!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    "outbox quarantine had a stale claim"
                ),
                Ok(Err(error)) => tracing::error!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    error = %error,
                    "outbox quarantine could not be recorded"
                ),
                Err(_) => tracing::error!(
                    event_id = %record.id,
                    attempt = record.attempt_count,
                    failure_code = failure.code(),
                    "outbox quarantine recording timed out"
                ),
            }
        }
    }
}

pub fn retry_delay(settings: &RuntimeSettings, attempt_count: i32) -> Duration {
    let exponent = u32::try_from(attempt_count.saturating_sub(1))
        .unwrap_or_default()
        .min(31);
    settings
        .retry_base
        .checked_mul(1_u32 << exponent)
        .unwrap_or(settings.retry_max)
        .min(settings.retry_max)
}

pub async fn render_metrics(pool: &PgPool, metrics: &PublisherMetrics) -> anyhow::Result<String> {
    Ok(metrics.render(&backlog(pool).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use tokio::sync::Notify;

    fn record() -> ClaimedOutboxRecord {
        let id = uuid::Uuid::from_u128(11);
        let tenant_id = uuid::Uuid::from_u128(22);
        ClaimedOutboxRecord {
            id,
            tenant_id,
            event_type: "platform.contacts.person-created.v1".into(),
            envelope: json!({
                "id": id,
                "event_type": "platform.contacts.person-created.v1",
                "schema_version": 1,
                "source_plugin": "platform.contacts",
                "tenant_id": tenant_id,
                "actor_id": null,
                "correlation_id": uuid::Uuid::from_u128(33),
                "causation_id": null,
                "occurred_at": "2026-07-20T12:00:00Z",
                "payload": {"entity_id": "per_example", "version": 1}
            }),
            created_at: Utc.with_ymd_and_hms(2026, 7, 20, 12, 0, 0).unwrap(),
            attempt_count: 1,
            claim_token: uuid::Uuid::from_u128(44),
            claim_expires_at: Utc.with_ymd_and_hms(2026, 7, 20, 12, 1, 0).unwrap(),
        }
    }

    fn settings() -> RuntimeSettings {
        RuntimeSettings {
            batch_size: NonZeroU32::new(32).unwrap(),
            max_in_flight: 8,
            poll_interval: Duration::from_millis(250),
            claim_lease: Duration::from_secs(60),
            publish_timeout: Duration::from_secs(5),
            database_timeout: Duration::from_secs(5),
            retry_base: Duration::from_secs(1),
            retry_max: Duration::from_secs(300),
        }
    }

    #[test]
    fn prepared_message_matches_the_stored_envelope_and_contract_subject() {
        let record = record();
        let publication = prepare_publication(&record).expect("valid publication");
        assert_eq!(publication.id, record.id);
        assert_eq!(
            publication.deduplication_id,
            format!("{}:{}", record.tenant_id, record.id)
        );
        assert_eq!(
            publication.subject,
            format!(
                "platform.events.v1.tenant.{}.platform.contacts.person-created.v1",
                record.tenant_id
            )
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&publication.payload).unwrap(),
            record.envelope
        );
    }

    #[test]
    fn broker_deduplication_identity_is_tenant_scoped() {
        let first = record();
        let mut second = first.clone();
        second.tenant_id = uuid::Uuid::from_u128(23);
        second.envelope["tenant_id"] = json!(second.tenant_id);

        let first_publication = prepare_publication(&first).expect("valid first publication");
        let second_publication = prepare_publication(&second).expect("valid second publication");

        assert_eq!(first_publication.id, second_publication.id);
        assert_ne!(
            first_publication.deduplication_id,
            second_publication.deduplication_id
        );
    }

    #[test]
    fn mismatched_storage_identity_is_rejected_before_publication() {
        let mut mismatched = record();
        mismatched.event_type = "platform.contacts.person-updated.v1".into();
        assert_eq!(
            prepare_publication(&mismatched).unwrap_err(),
            PublishFailure::EnvelopeMismatch
        );

        let mut wrong_tenant = record();
        wrong_tenant.tenant_id = uuid::Uuid::from_u128(999);
        assert_eq!(
            prepare_publication(&wrong_tenant).unwrap_err(),
            PublishFailure::EnvelopeMismatch
        );

        let mut wrong_id = record();
        wrong_id.id = uuid::Uuid::from_u128(1_000);
        assert_eq!(
            prepare_publication(&wrong_id).unwrap_err(),
            PublishFailure::EnvelopeMismatch
        );

        let mut future = record();
        future.envelope["schema_version"] = json!(2);
        let failure = prepare_publication(&future).unwrap_err();
        assert_eq!(failure, PublishFailure::UnsupportedSchemaVersion);
        assert!(failure.retryable());
    }

    #[test]
    fn publication_canonicalises_unknown_keys_and_rejects_oversized_payloads() {
        let mut record = record();
        record.envelope["secret"] = json!("must-not-cross-the-broker-boundary");
        let publication = prepare_publication(&record).expect("known envelope remains valid");
        let canonical: serde_json::Value = serde_json::from_slice(&publication.payload).unwrap();
        assert!(canonical.get("secret").is_none());

        record.envelope["payload"] =
            json!({"blob": "x".repeat(platform_events::MAX_EVENT_ENVELOPE_BYTES)});
        assert_eq!(
            prepare_publication(&record).unwrap_err(),
            PublishFailure::OversizedEnvelope
        );
    }

    #[test]
    fn retry_backoff_is_exponential_and_bounded() {
        let settings = settings();
        assert_eq!(retry_delay(&settings, 1), Duration::from_secs(1));
        assert_eq!(retry_delay(&settings, 4), Duration::from_secs(8));
        assert_eq!(retry_delay(&settings, 40), Duration::from_secs(300));
    }

    #[test]
    fn stream_compatibility_includes_storage_subject_and_deduplication_window() {
        let mut config = StreamConfig {
            subjects: vec![JETSTREAM_V1_SUBJECTS.into()],
            storage: StorageType::File,
            retention: RetentionPolicy::Limits,
            discard: DiscardPolicy::New,
            max_bytes: 1_073_741_824,
            duplicate_window: Duration::from_secs(600),
            ..Default::default()
        };
        assert!(stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
        config.duplicate_window = Duration::from_secs(120);
        assert!(!stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
        config.duplicate_window = Duration::from_secs(600);
        config.no_ack = true;
        assert!(!stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
        config.no_ack = false;
        config.sealed = true;
        assert!(!stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
        config.sealed = false;
        config.republish = Some(Republish {
            source: JETSTREAM_V1_SUBJECTS.into(),
            destination: "unsafe.republish.>".into(),
            headers_only: false,
        });
        assert!(!stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
        config.republish = None;
        config.subject_transform = Some(SubjectTransform {
            source: JETSTREAM_V1_SUBJECTS.into(),
            destination: "unsafe.transform.>".into(),
        });
        assert!(!stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
        config.subject_transform = None;
        config.max_bytes -= 1;
        assert!(!stream_config_is_compatible(
            &config,
            Duration::from_secs(600),
            1_073_741_824,
        ));
    }

    #[test]
    fn readiness_requires_a_recent_successful_claim_cycle() {
        let metrics = PublisherMetrics::default();
        assert!(!metrics.publisher_loop_ready(Duration::from_secs(60)));
        metrics.observe_claim_result(true);
        assert!(metrics.publisher_loop_ready(Duration::from_secs(60)));
        metrics.observe_claim_result(false);
        assert!(!metrics.publisher_loop_ready(Duration::from_secs(60)));
    }

    struct AckBarrier {
        release: Notify,
    }

    #[async_trait]
    impl BrokerPublisher for AckBarrier {
        async fn publish(&self, _: &PreparedPublication) -> Result<(), PublishFailure> {
            self.release.notified().await;
            Ok(())
        }
    }

    #[tokio::test]
    async fn publication_attempt_does_not_complete_before_broker_acknowledgement() {
        let broker = Arc::new(AckBarrier {
            release: Notify::new(),
        });
        let publication = prepare_publication(&record()).unwrap();
        let task_broker = broker.clone();
        let task = tokio::spawn(async move { task_broker.publish(&publication).await });
        tokio::task::yield_now().await;
        assert!(!task.is_finished());
        broker.release.notify_one();
        assert_eq!(task.await.unwrap(), Ok(()));
    }

    #[test]
    fn metrics_are_aggregate_and_do_not_expose_tenant_or_event_dimensions() {
        let metrics = PublisherMetrics::default();
        for _ in 0..3 {
            metrics.observe_attempt();
        }
        metrics.observe_failure();
        metrics.observe_success(Duration::from_millis(25));
        let body = metrics.render(&OutboxBacklog {
            pending: 2,
            claimable: 1,
            claimed: 1,
            quarantined: 1,
            oldest_pending_at: None,
        });
        assert!(body.contains("limiara_outbox_backlog 2"));
        assert!(body.contains("limiara_outbox_publish_attempts_total 3"));
        assert!(body.contains("limiara_outbox_publish_failures_total 1"));
        assert!(body.contains("limiara_outbox_quarantined 1"));
        assert!(body.contains("limiara_outbox_publish_latency_seconds_sum 0.025000"));
        assert!(!body.contains("tenant_id"));
        assert!(!body.contains("event_type"));
    }
}
