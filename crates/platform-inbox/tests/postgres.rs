use async_trait::async_trait;
use platform_events::EventEnvelope;
use platform_inbox::{
    consume, BrokerDisposition, ConsumerId, ConsumptionError, ConsumptionStatus, EventHandler,
    HandlerFailure, UnsupportedSchemaDisposition,
};
use sqlx::{postgres::PgPoolOptions, PgPool, Postgres, Transaction};
use std::sync::atomic::{AtomicUsize, Ordering};
use uuid::Uuid;

const TENANT_A: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000001);
const ACCEPTANCE_PLUGIN: &str = "platform.acceptance";
const TENANT_B: Uuid = Uuid::from_u128(0x018f0000_0000_7000_8000_000000000002);

#[derive(Clone, Copy)]
enum HandlerResult {
    Success,
    Retry,
    Poison,
}

struct ProbeHandler {
    consumer_id: ConsumerId,
    result: HandlerResult,
    effect_tenant: Option<Uuid>,
    calls: AtomicUsize,
}

impl ProbeHandler {
    fn success(consumer_id: &'static str) -> Self {
        Self {
            consumer_id: ConsumerId::parse(consumer_id).expect("valid acceptance consumer ID"),
            result: HandlerResult::Success,
            effect_tenant: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn failing(consumer_id: &'static str, result: HandlerResult) -> Self {
        Self {
            consumer_id: ConsumerId::parse(consumer_id).expect("valid acceptance consumer ID"),
            result,
            effect_tenant: None,
            calls: AtomicUsize::new(0),
        }
    }

    fn wrong_tenant(consumer_id: &'static str, effect_tenant: Uuid) -> Self {
        Self {
            consumer_id: ConsumerId::parse(consumer_id).expect("valid acceptance consumer ID"),
            result: HandlerResult::Success,
            effect_tenant: Some(effect_tenant),
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl EventHandler for ProbeHandler {
    async fn handle(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        event: &EventEnvelope,
    ) -> Result<(), HandlerFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let effect_tenant = self.effect_tenant.unwrap_or(event.tenant_id);
        sqlx::query(
            r#"
            INSERT INTO inbox_acceptance_effects (tenant_id, consumer_id, event_id)
            VALUES ($1, $2, $3)
            "#,
        )
        .bind(effect_tenant)
        .bind(self.consumer_id.as_str())
        .bind(event.id)
        .execute(&mut **transaction)
        .await
        .map_err(|_| {
            HandlerFailure::retryable("acceptance.effect_database")
                .expect("static acceptance control is valid")
        })?;

        match self.result {
            HandlerResult::Success => Ok(()),
            HandlerResult::Retry => Err(HandlerFailure::retryable("acceptance.transient")
                .expect("static acceptance control is valid")),
            HandlerResult::Poison => Err(HandlerFailure::poison("acceptance.invalid_event")
                .expect("static acceptance control is valid")),
        }
    }
}

fn event(tenant_id: Uuid, id: Uuid) -> EventEnvelope {
    EventEnvelope {
        id,
        event_type: "platform.acceptance.changed.v1".into(),
        schema_version: 1,
        source_plugin: "platform.acceptance".into(),
        tenant_id,
        actor_id: None,
        correlation_id: Uuid::now_v7(),
        causation_id: None,
        occurred_at: chrono::Utc::now(),
        payload: serde_json::json!({"sequence": 1}),
    }
}

fn delivery(event: &EventEnvelope) -> (String, Vec<u8>) {
    (
        event.jetstream_subject().expect("acceptance subject"),
        serde_json::to_vec(event).expect("acceptance envelope"),
    )
}

async fn counts(pool: &PgPool, tenant_id: Uuid, consumer_id: &str, event_id: Uuid) -> (i64, i64) {
    let mut transaction = platform_db::begin_tenant_tx(pool, tenant_id)
        .await
        .expect("tenant transaction");
    let marker: i64 =
        sqlx::query_scalar("SELECT count(*) FROM inbox WHERE consumer_id = $1 AND event_id = $2")
            .bind(consumer_id)
            .bind(event_id)
            .fetch_one(&mut *transaction)
            .await
            .expect("count inbox markers");
    let effects: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbox_acceptance_effects WHERE consumer_id = $1 AND event_id = $2",
    )
    .bind(consumer_id)
    .bind(event_id)
    .fetch_one(&mut *transaction)
    .await
    .expect("count effects");
    transaction.commit().await.expect("commit count query");
    (marker, effects)
}

#[tokio::test]
#[ignore = "requires scripts/test-inbox-consumer.sh PostgreSQL fixture"]
async fn inbox_acceptance() {
    let database_url = std::env::var("PLATFORM_TEST_DATABASE_URL")
        .expect("PLATFORM_TEST_DATABASE_URL is set by the acceptance script");
    let pool = PgPoolOptions::new()
        .max_connections(8)
        .connect(&database_url)
        .await
        .expect("connect as platform_app");

    // A reusable application-role connection has no ambient tenant. Forced
    // RLS makes reads empty and rejects writes until begin_tenant_tx sets the
    // transaction-local context.
    let visible_without_context: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox")
        .fetch_one(&pool)
        .await
        .expect("missing tenant context fails closed for reads");
    assert_eq!(visible_without_context, 0);
    let insert_without_context =
        sqlx::query("INSERT INTO inbox (tenant_id, consumer_id, event_id) VALUES ($1, $2, $3)")
            .bind(TENANT_A)
            .bind("platform.acceptance.missing-context")
            .bind(Uuid::from_u128(0x018f3100_0000_7000_8000_000000000000))
            .execute(&pool)
            .await;
    assert!(insert_without_context.is_err());

    // Concurrent and sequential redelivery share one committed marker and one
    // effect. Discarding the first Applied result models a crash after commit
    // but before broker acknowledgement; the redelivery is a safe duplicate.
    let concurrent_event = event(
        TENANT_A,
        Uuid::from_u128(0x018f3100_0000_7000_8000_000000000001),
    );
    let (subject, payload) = delivery(&concurrent_event);
    let concurrent_handler = ProbeHandler::success("platform.acceptance.concurrent");
    let (left, right) = tokio::join!(
        consume(
            &pool,
            ACCEPTANCE_PLUGIN,
            &concurrent_handler.consumer_id,
            &subject,
            &payload,
            &concurrent_handler
        ),
        consume(
            &pool,
            ACCEPTANCE_PLUGIN,
            &concurrent_handler.consumer_id,
            &subject,
            &payload,
            &concurrent_handler
        )
    );
    let statuses = [left.unwrap(), right.unwrap()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == ConsumptionStatus::Applied)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == ConsumptionStatus::Duplicate)
            .count(),
        1
    );
    assert_eq!(concurrent_handler.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        counts(
            &pool,
            TENANT_A,
            concurrent_handler.consumer_id.as_str(),
            concurrent_event.id
        )
        .await,
        (1, 1)
    );
    assert_eq!(
        consume(
            &pool,
            ACCEPTANCE_PLUGIN,
            &concurrent_handler.consumer_id,
            &subject,
            &payload,
            &concurrent_handler
        )
        .await
        .unwrap(),
        ConsumptionStatus::Duplicate
    );
    assert_eq!(concurrent_handler.calls.load(Ordering::SeqCst), 1);

    // A retryable handler failure rolls back both its attempted effect and the
    // marker. The next delivery owns the marker and applies exactly once.
    let retry_event = event(
        TENANT_A,
        Uuid::from_u128(0x018f3100_0000_7000_8000_000000000002),
    );
    let (subject, payload) = delivery(&retry_event);
    let retry_handler = ProbeHandler::failing("platform.acceptance.retry", HandlerResult::Retry);
    let error = consume(
        &pool,
        ACCEPTANCE_PLUGIN,
        &retry_handler.consumer_id,
        &subject,
        &payload,
        &retry_handler,
    )
    .await
    .unwrap_err();
    assert_eq!(error.control(), "acceptance.transient");
    assert_eq!(
        error.disposition(UnsupportedSchemaDisposition::Terminate),
        BrokerDisposition::Retry
    );
    assert_eq!(
        counts(
            &pool,
            TENANT_A,
            retry_handler.consumer_id.as_str(),
            retry_event.id,
        )
        .await,
        (0, 0)
    );
    let recovered_handler = ProbeHandler::success("platform.acceptance.retry");
    assert_eq!(
        consume(
            &pool,
            ACCEPTANCE_PLUGIN,
            &recovered_handler.consumer_id,
            &subject,
            &payload,
            &recovered_handler
        )
        .await
        .unwrap(),
        ConsumptionStatus::Applied
    );
    assert_eq!(
        counts(
            &pool,
            TENANT_A,
            recovered_handler.consumer_id.as_str(),
            retry_event.id
        )
        .await,
        (1, 1)
    );

    // A deterministic handler rejection is terminal but still rolls back all
    // attempted database state.
    let poison_event = event(
        TENANT_A,
        Uuid::from_u128(0x018f3100_0000_7000_8000_000000000003),
    );
    let (subject, payload) = delivery(&poison_event);
    let poison_handler = ProbeHandler::failing("platform.acceptance.poison", HandlerResult::Poison);
    let error = consume(
        &pool,
        ACCEPTANCE_PLUGIN,
        &poison_handler.consumer_id,
        &subject,
        &payload,
        &poison_handler,
    )
    .await
    .unwrap_err();
    assert_eq!(error.control(), "acceptance.invalid_event");
    assert_eq!(
        error.disposition(UnsupportedSchemaDisposition::Retry),
        BrokerDisposition::Terminate
    );
    assert_eq!(
        counts(
            &pool,
            TENANT_A,
            poison_handler.consumer_id.as_str(),
            poison_event.id,
        )
        .await,
        (0, 0)
    );

    // Consumer identity is part of the deduplication key, so independent
    // logical consumers can each apply the same event once.
    let shared_event = event(
        TENANT_A,
        Uuid::from_u128(0x018f3100_0000_7000_8000_000000000004),
    );
    let (subject, payload) = delivery(&shared_event);
    for consumer_id in [
        "platform.acceptance.consumer-a",
        "platform.acceptance.consumer-b",
    ] {
        let handler = ProbeHandler::success(consumer_id);
        assert_eq!(
            consume(
                &pool,
                ACCEPTANCE_PLUGIN,
                &handler.consumer_id,
                &subject,
                &payload,
                &handler
            )
            .await
            .unwrap(),
            ConsumptionStatus::Applied
        );
        assert_eq!(
            counts(&pool, TENANT_A, consumer_id, shared_event.id).await,
            (1, 1)
        );
    }

    // The same logical key is independent across tenants, but an application
    // transaction scoped to tenant A cannot observe tenant B's known marker.
    let cross_tenant_id = Uuid::from_u128(0x018f3100_0000_7000_8000_000000000005);
    for tenant_id in [TENANT_A, TENANT_B] {
        let tenant_event = event(tenant_id, cross_tenant_id);
        let (subject, payload) = delivery(&tenant_event);
        let handler = ProbeHandler::success("platform.acceptance.tenant-key");
        assert_eq!(
            consume(
                &pool,
                ACCEPTANCE_PLUGIN,
                &handler.consumer_id,
                &subject,
                &payload,
                &handler
            )
            .await
            .unwrap(),
            ConsumptionStatus::Applied
        );
        assert_eq!(
            counts(
                &pool,
                tenant_id,
                handler.consumer_id.as_str(),
                cross_tenant_id,
            )
            .await,
            (1, 1)
        );
    }
    let mut tenant_a = platform_db::begin_tenant_tx(&pool, TENANT_A)
        .await
        .expect("tenant A transaction");
    let visible_b: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM inbox WHERE tenant_id = $1 AND consumer_id = $2 AND event_id = $3",
    )
    .bind(TENANT_B)
    .bind("platform.acceptance.tenant-key")
    .bind(cross_tenant_id)
    .fetch_one(&mut *tenant_a)
    .await
    .expect("known tenant B marker is hidden");
    assert_eq!(visible_b, 0);
    tenant_a.commit().await.expect("commit tenant probe");

    // Even a trusted handler mistake cannot persist a cross-tenant effect: the
    // fixture's forced RLS rejects it and the newly inserted marker rolls back.
    let wrong_tenant_event = event(
        TENANT_A,
        Uuid::from_u128(0x018f3100_0000_7000_8000_000000000006),
    );
    let (subject, payload) = delivery(&wrong_tenant_event);
    let wrong_tenant_handler =
        ProbeHandler::wrong_tenant("platform.acceptance.wrong-tenant", TENANT_B);
    let error = consume(
        &pool,
        ACCEPTANCE_PLUGIN,
        &wrong_tenant_handler.consumer_id,
        &subject,
        &payload,
        &wrong_tenant_handler,
    )
    .await
    .unwrap_err();
    assert_eq!(error.control(), "acceptance.effect_database");
    assert_eq!(
        error.disposition(UnsupportedSchemaDisposition::Terminate),
        BrokerDisposition::Retry
    );
    assert_eq!(
        counts(
            &pool,
            TENANT_A,
            wrong_tenant_handler.consumer_id.as_str(),
            wrong_tenant_event.id
        )
        .await,
        (0, 0)
    );

    // Invalid bytes and a subject/envelope tenant mismatch are terminated
    // before a tenant transaction or inbox marker can be created.
    let invalid_handler = ProbeHandler::success("platform.acceptance.invalid");
    let error = consume(
        &pool,
        ACCEPTANCE_PLUGIN,
        &invalid_handler.consumer_id,
        "platform.events.v1.tenant.invalid",
        b"not-json",
        &invalid_handler,
    )
    .await
    .unwrap_err();
    assert_eq!(error.control(), "consumer.invalid_envelope");
    assert_eq!(
        error.disposition(UnsupportedSchemaDisposition::Retry),
        BrokerDisposition::Terminate
    );
    assert_eq!(invalid_handler.calls.load(Ordering::SeqCst), 0);

    let mismatch_event = event(
        TENANT_A,
        Uuid::from_u128(0x018f3100_0000_7000_8000_000000000007),
    );
    let (_, payload) = delivery(&mismatch_event);
    let error = consume(
        &pool,
        ACCEPTANCE_PLUGIN,
        &invalid_handler.consumer_id,
        "platform.events.v1.tenant.018f0000-0000-7000-8000-000000000002.platform.acceptance.changed.v1",
        &payload,
        &invalid_handler,
    )
    .await
    .unwrap_err();
    assert_eq!(error.control(), "consumer.subject_mismatch");
    assert_eq!(
        error.disposition(UnsupportedSchemaDisposition::Retry),
        BrokerDisposition::Terminate
    );
    assert_eq!(
        counts(
            &pool,
            TENANT_A,
            invalid_handler.consumer_id.as_str(),
            mismatch_event.id
        )
        .await,
        (0, 0)
    );

    // The public error text stays bounded and never includes the payload or
    // database URL; callers log `control()` rather than customer event data.
    let display = ConsumptionError::SubjectMismatch.to_string();
    assert!(!display.contains("not-json"));
    assert!(!display.contains(&database_url));
}
