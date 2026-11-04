use crate::AppState;
use axum::{extract::State, http::StatusCode, Json};
use serde_json::{json, Value};
use std::time::Duration;

pub(crate) async fn live() -> Json<Value> {
    Json(json!({"status": "live"}))
}

pub(crate) async fn ready(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let timeout = Duration::from_millis(state.settings.platform.health.check_timeout_ms);
    let database = check_database(&state, timeout).await;
    let migrations = check_migrations(&state, timeout).await;
    let nats = check_nats(&state, timeout).await;
    let plugin_registry = check_plugin_registry(&state, timeout).await;
    let ready = database && migrations && nats && plugin_registry;
    (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if ready { "ready" } else { "not_ready" },
            "components": {
                "database": component(database),
                "migrations": component(migrations),
                "nats": component(nats),
                "plugin_registry": component(plugin_registry),
            }
        })),
    )
}

async fn check_database(state: &AppState, timeout: Duration) -> bool {
    let Some(pool) = state.pool.as_ref() else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            timeout,
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool)
        )
        .await,
        Ok(Ok(1))
    )
}

async fn check_migrations(state: &AppState, timeout: Duration) -> bool {
    let Some(pool) = state.pool.as_ref() else {
        return false;
    };
    let query = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
          SELECT 1 FROM plugin_migration_ledger
          WHERE tenant_id = '00000000-0000-0000-0000-000000000000'
            AND plugin_id = 'platform.core'
            AND migration_id = $1
        )
        "#,
    )
    .bind(platform_db::CORE_MIGRATION_SENTINEL)
    .fetch_one(pool);
    matches!(tokio::time::timeout(timeout, query).await, Ok(Ok(true)))
}

async fn check_plugin_registry(state: &AppState, timeout: Duration) -> bool {
    let Some(pool) = state.pool.as_ref() else {
        return false;
    };
    let query = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM plugin_catalogue WHERE latest_usable_version IS NOT NULL)",
    )
    .fetch_one(pool);
    matches!(tokio::time::timeout(timeout, query).await, Ok(Ok(true)))
}

async fn check_nats(state: &AppState, timeout: Duration) -> bool {
    let Ok(url) = openidconnect::url::Url::parse(&state.settings.platform.nats.url) else {
        return false;
    };
    let (Some(host), Some(port)) = (url.host_str(), url.port_or_known_default()) else {
        return false;
    };
    matches!(
        tokio::time::timeout(timeout, tokio::net::TcpStream::connect((host, port))).await,
        Ok(Ok(_))
    )
}

fn component(healthy: bool) -> Value {
    json!({"status": if healthy { "healthy" } else { "unavailable" }})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_are_structured_and_secret_free() {
        let value = component(false);
        assert_eq!(value, json!({"status": "unavailable"}));
        assert!(!value.to_string().contains("postgres://"));
    }
}
