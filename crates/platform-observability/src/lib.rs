#![forbid(unsafe_code)]
pub fn init(service_name: &str) {
    let filter =
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into());
    let _ = tracing_subscriber::fmt()
        .json()
        .with_env_filter(filter)
        .with_target(true)
        .try_init();
    tracing::info!(service.name = service_name, "observability initialized");
}
