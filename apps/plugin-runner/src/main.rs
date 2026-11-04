#![forbid(unsafe_code)]

use axum::{extract::State, http::StatusCode, routing::get, Json, Router};
use plugin_runtime_wasm::{WasmLimits, WasmPluginRunner};
use serde_json::{json, Value};
use std::net::SocketAddr;

#[derive(Clone)]
struct RunnerState {
    registry_initialized: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    platform_observability::init("plugin-runner");
    let settings = platform_config::Settings::load()?;
    let runner = WasmPluginRunner::new(WasmLimits {
        fuel: 1_000_000,
        memory_bytes: 16 * 1024 * 1024,
        timeout_ms: 250,
    });
    tracing::info!(message = %runner.hello()?, "wasm plugin registry initialized");
    let bind: SocketAddr = settings.platform.services.plugin_runner_bind.parse()?;
    let app = Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .with_state(RunnerState {
            registry_initialized: true,
        });
    tracing::info!(%bind, "plugin runner started");
    axum::serve(tokio::net::TcpListener::bind(bind).await?, app).await?;
    Ok(())
}

async fn live() -> Json<Value> {
    Json(json!({"status": "live"}))
}

async fn ready(State(state): State<RunnerState>) -> (StatusCode, Json<Value>) {
    (
        if state.registry_initialized {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if state.registry_initialized { "ready" } else { "not_ready" },
            "components": {
                "plugin_registry": {
                    "status": if state.registry_initialized { "healthy" } else { "unavailable" }
                }
            }
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn readiness_tracks_registry_initialization() {
        let (status, _) = ready(State(RunnerState {
            registry_initialized: false,
        }))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }
}
