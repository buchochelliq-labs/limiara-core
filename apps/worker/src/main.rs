#![forbid(unsafe_code)]

use axum::{
    extract::State,
    http::{header::CONTENT_TYPE, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use worker::{
    outbox_publisher::{
        self, retry_delay, BrokerPublisher, JetStreamPublisher, PublisherMetrics, RuntimeSettings,
    },
    plugin_runtime::{self, PluginRuntimeMetrics, PluginRuntimeSettings, WorkerContributions},
};

type PublisherState = Arc<RwLock<Option<Arc<JetStreamPublisher>>>>;

#[derive(Clone)]
struct WorkerState {
    settings: Arc<platform_config::Settings>,
    pool: PgPool,
    publisher: PublisherState,
    metrics: Arc<PublisherMetrics>,
    plugin_runtime: PluginRuntimeHealth,
}

/// Readiness of plugin background work. With no linked contributions there is
/// nothing to run; with contributions but no tenant runtime pool the worker
/// fails closed and reports not ready instead of silently skipping work.
#[derive(Clone)]
enum PluginRuntimeHealth {
    NoContributions,
    Unconfigured,
    Running {
        metrics: Arc<PluginRuntimeMetrics>,
        stale_after: Duration,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    platform_observability::init("worker");
    let settings = Arc::new(platform_config::Settings::load()?);
    // Refuse storage selections the platform cannot make safe, before any
    // store is opened (ADR 0013).
    let storage = platform_storage::selection_from(&settings.platform);
    if let Err(error) = storage.validate() {
        tracing::error!(code = error.code(), %error, "storage selection refused");
        anyhow::bail!("storage selection refused: {error}");
    }
    let pool = platform_db::connect_lazy(
        &settings.platform.database.url,
        settings.platform.database.max_connections,
        Duration::from_millis(settings.platform.database.acquire_timeout_ms),
    )?;
    let recovered = platform_db::recover_plugin_transitions(&pool).await?;
    if recovered > 0 {
        tracing::warn!(recovered, "recovered interrupted plugin transitions");
    }
    let outbox_settings = RuntimeSettings::from_platform(&settings);
    let publisher = PublisherState::default();
    let publisher_metrics = Arc::new(PublisherMetrics::default());
    let bind: SocketAddr = settings.platform.services.worker_bind.parse()?;
    let listener = tokio::net::TcpListener::bind(bind).await?;
    tokio::spawn(connect_and_run_publisher(
        settings.clone(),
        pool.clone(),
        outbox_settings,
        publisher_metrics.clone(),
        publisher.clone(),
    ));

    let plugin_runtime = start_plugin_runtime(&settings, &pool, worker::linked_worker_plugins()?)?;
    start_automation_jobs(&settings, &pool)?;

    let state = WorkerState {
        settings,
        pool,
        publisher,
        metrics: publisher_metrics,
        plugin_runtime,
    };
    let app = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/metrics", get(metrics))
        .with_state(state);
    tracing::info!(%bind, "worker started");
    axum::serve(listener, app).await?;
    Ok(())
}

/// Runs queued background and scheduled automations (ADR 0022 §6), bulk
/// jobs (ADR 0023 §2) and device notifications (ADR 0030 §3) through the
/// RLS-bound tenant role. Without it, jobs wait in their workspaces and
/// devices sync on their own schedule.
fn start_automation_jobs(
    settings: &Arc<platform_config::Settings>,
    worker_pool: &PgPool,
) -> anyhow::Result<()> {
    let Some(tenant_url) = settings.platform.worker.tenant_database_url.as_deref() else {
        tracing::warn!(
            "platform.worker.tenant_database_url is not configured; background and scheduled automations, bulk jobs and device notifications will not run"
        );
        return Ok(());
    };
    let tenant_pool = platform_db::connect_lazy(
        tenant_url,
        settings.platform.database.max_connections,
        Duration::from_millis(settings.platform.database.acquire_timeout_ms),
    )?;
    tokio::spawn(worker::automation_jobs::run(
        worker_pool.clone(),
        tenant_pool.clone(),
        Duration::from_secs(5),
    ));
    tokio::spawn(worker::bulk_jobs::run(
        worker_pool.clone(),
        tenant_pool.clone(),
        Duration::from_secs(5),
    ));
    let push = &settings.platform.devices.push;
    let notifiers = worker::device_notifications::push_notifiers(push)?;
    if notifiers.is_empty() {
        tracing::info!("no push provider is configured; devices sync on their own schedule");
    } else {
        tokio::spawn(worker::device_notifications::run(
            worker_pool.clone(),
            tenant_pool,
            notifiers,
            Duration::from_secs(push.quiet_seconds),
            Duration::from_secs(5),
        ));
    }
    Ok(())
}

fn start_plugin_runtime(
    settings: &Arc<platform_config::Settings>,
    worker_pool: &PgPool,
    plugins: Vec<Arc<dyn plugin_runtime_native::NativeWorkerPlugin>>,
) -> anyhow::Result<PluginRuntimeHealth> {
    let contributions = WorkerContributions::from_plugins(&plugins)?;
    if contributions.is_empty() {
        return Ok(PluginRuntimeHealth::NoContributions);
    }
    let Some(tenant_url) = settings.platform.worker.tenant_database_url.as_deref() else {
        tracing::error!(
            "plugin worker contributions are linked but platform.worker.tenant_database_url is not configured"
        );
        return Ok(PluginRuntimeHealth::Unconfigured);
    };
    let tenant_pool = platform_db::connect_lazy(
        tenant_url,
        settings.platform.database.max_connections,
        Duration::from_millis(settings.platform.database.acquire_timeout_ms),
    )?;
    // Hourly is ample for a 30-day window; a pass is cheap when nothing is due.
    tokio::spawn(worker::extension_retention::run(
        worker_pool.clone(),
        tenant_pool.clone(),
        worker::linked_extension_hosts(&tenant_pool),
        Duration::from_secs(60 * 60),
    ));
    let runtime_settings = PluginRuntimeSettings::from_platform(settings);
    let metrics = Arc::new(PluginRuntimeMetrics::default());
    let mut stale_after = runtime_settings.reconcile_interval.saturating_mul(3);
    if !contributions.jobs.is_empty() {
        tokio::spawn(plugin_runtime::run_jobs(
            worker_pool.clone(),
            tenant_pool.clone(),
            contributions.jobs.clone(),
            runtime_settings.clone(),
            metrics.clone(),
        ));
    } else {
        // Consumers alone have no schedules to reconcile, so readiness does
        // not depend on a reconciliation heartbeat.
        plugin_runtime::mark_reconciled(&metrics);
        stale_after = Duration::MAX;
    }
    for consumer in contributions.consumers {
        tokio::spawn(plugin_runtime::supervise_consumer(
            settings.platform.nats.url.clone(),
            settings.platform.nats.stream.clone(),
            tenant_pool.clone(),
            consumer,
            runtime_settings.clone(),
            metrics.clone(),
        ));
    }
    Ok(PluginRuntimeHealth::Running {
        metrics,
        stale_after,
    })
}

async fn connect_and_run_publisher(
    settings: Arc<platform_config::Settings>,
    pool: PgPool,
    outbox_settings: RuntimeSettings,
    metrics: Arc<PublisherMetrics>,
    publisher_state: PublisherState,
) {
    let mut attempt = 1;
    loop {
        match JetStreamPublisher::connect(
            &settings.platform.nats.url,
            &settings.platform.nats.stream,
            Duration::from_secs(settings.platform.nats.outbox.duplicate_window_seconds),
            i64::try_from(settings.platform.nats.outbox.stream_max_bytes)
                .expect("validated JetStream byte limit"),
        )
        .await
        {
            Ok(publisher) => {
                let publisher = Arc::new(publisher);
                *publisher_state.write().await = Some(publisher.clone());
                tracing::info!("outbox publisher connected to JetStream");
                let broker: Arc<dyn BrokerPublisher> = publisher;
                outbox_publisher::run(
                    pool.clone(),
                    broker,
                    outbox_settings.clone(),
                    metrics.clone(),
                )
                .await;
                publisher_state.write().await.take();
                tracing::error!("outbox publisher loop stopped; reconnecting");
                attempt = 1;
            }
            Err(error) => {
                let delay = retry_delay(&outbox_settings, attempt);
                tracing::warn!(
                    error = %error,
                    retry_delay_ms = delay.as_millis(),
                    "outbox publisher startup failed; worker remains live and will retry"
                );
                attempt = attempt.saturating_add(1);
                tokio::time::sleep(delay).await;
            }
        }
    }
}

async fn live() -> Json<Value> {
    Json(json!({"status": "live"}))
}

async fn ready(State(state): State<WorkerState>) -> (StatusCode, Json<Value>) {
    let timeout = Duration::from_millis(state.settings.platform.health.check_timeout_ms);
    let database = matches!(
        tokio::time::timeout(
            timeout,
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(&state.pool)
        )
        .await,
        Ok(Ok(1))
    );
    let migrations = matches!(
        tokio::time::timeout(
            timeout,
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM plugin_migration_ledger WHERE tenant_id = '00000000-0000-0000-0000-000000000000' AND plugin_id = 'platform.core' AND migration_id = $1)"
            )
            .bind(platform_db::CORE_MIGRATION_SENTINEL)
            .fetch_one(&state.pool)
        )
        .await,
        Ok(Ok(true))
    );
    let nats = broker_ready(&state.publisher, timeout).await;
    let publisher_loop = state.metrics.publisher_loop_ready(Duration::from_secs(
        state.settings.platform.nats.outbox.claim_lease_seconds,
    ));
    let plugin_runtime = match &state.plugin_runtime {
        PluginRuntimeHealth::NoContributions => true,
        PluginRuntimeHealth::Unconfigured => false,
        PluginRuntimeHealth::Running {
            metrics,
            stale_after,
        } => metrics.reconcile_ready(*stale_after),
    };
    let is_ready = database && migrations && nats && publisher_loop && plugin_runtime;
    (
        if is_ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if is_ready { "ready" } else { "not_ready" },
            "components": {
                "database": component(database),
                "migrations": component(migrations),
                "nats": component(nats),
                "outbox_publisher": component(publisher_loop),
                "plugin_runtime": component(plugin_runtime),
            }
        })),
    )
}

async fn broker_ready(publisher_state: &PublisherState, timeout: Duration) -> bool {
    let publisher = publisher_state.read().await.clone();
    match publisher {
        Some(publisher) => matches!(
            tokio::time::timeout(timeout, publisher.ready()).await,
            Ok(true)
        ),
        None => false,
    }
}

async fn metrics(State(state): State<WorkerState>) -> Response {
    let timeout = Duration::from_millis(state.settings.platform.health.check_timeout_ms);
    match tokio::time::timeout(
        timeout,
        outbox_publisher::render_metrics(&state.pool, &state.metrics),
    )
    .await
    {
        Ok(Ok(mut body)) => {
            if let PluginRuntimeHealth::Running { metrics, .. } = &state.plugin_runtime {
                body.push_str(&metrics.render());
            }
            (
                StatusCode::OK,
                [(CONTENT_TYPE, "text/plain; version=0.0.4; charset=utf-8")],
                body,
            )
                .into_response()
        }
        Ok(Err(error)) => {
            tracing::warn!(error = %error, "outbox metrics query failed");
            (StatusCode::SERVICE_UNAVAILABLE, "metrics unavailable\n").into_response()
        }
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "metrics timeout\n").into_response(),
    }
}

fn component(healthy: bool) -> Value {
    json!({"status": if healthy { "healthy" } else { "unavailable" }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn disconnected_publisher_state_is_not_ready() {
        assert!(!broker_ready(&PublisherState::default(), Duration::from_millis(10)).await);
    }
}
