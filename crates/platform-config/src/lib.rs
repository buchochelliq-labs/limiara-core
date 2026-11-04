#![forbid(unsafe_code)]

use config::{builder::DefaultState, Config, ConfigBuilder, Environment, File};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

const DEFAULT_CONFIG_PATH: &str = "config/default.yml";

#[derive(Debug, Clone, Deserialize)]
pub struct Settings {
    pub platform: PlatformSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PlatformSettings {
    pub environment: String,
    pub public_base_url: String,
    pub database: DatabaseSettings,
    pub oidc: OidcSettings,
    pub nats: NatsSettings,
    pub sessions: SessionSettings,
    pub health: HealthSettings,
    #[serde(default)]
    pub plugins: PluginSettings,
    #[serde(default)]
    pub worker: WorkerSettings,
    pub services: ServiceSettings,
    #[serde(default)]
    pub external_hosts: ExternalHostSettings,
    pub storage: StorageSettings,
    #[serde(default)]
    pub deployment: DeploymentSettings,
    #[serde(default)]
    pub analytics: AnalyticsSettings,
    #[serde(default)]
    pub devices: DeviceSettings,
}

/// Device apps that sign members in through the system browser (ADR 0029).
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceSettings {
    /// Where the server may return a device app's sign-in, matched
    /// exactly: private-use schemes such as `com.example.app:/oauth` or
    /// loopback `http://127.0.0.1:<port>/…` (RFC 8252). Empty turns device
    /// sign-in off.
    #[serde(default)]
    pub redirect_uris: Vec<String>,
    /// How long a device may work without reaching the server before it
    /// locks its replica (ADR 0024 §4).
    #[serde(default = "default_offline_limit_seconds")]
    pub offline_limit_seconds: u64,
    /// Content-free "workspace changed" notifications (ADR 0030).
    #[serde(default)]
    pub push: PushSettings,
}

fn default_offline_limit_seconds() -> u64 {
    14 * 24 * 60 * 60
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            redirect_uris: Vec::new(),
            offline_limit_seconds: default_offline_limit_seconds(),
            push: PushSettings::default(),
        }
    }
}

/// The worker's push notification dispatcher (ADR 0030 §3). With neither
/// provider configured, no notifications are sent and devices rely on their
/// own catch-up schedule.
#[derive(Debug, Clone, Deserialize)]
pub struct PushSettings {
    /// The least time between two notifications to one device, so a burst
    /// of changes costs one notification.
    #[serde(default = "default_push_quiet_seconds")]
    pub quiet_seconds: u64,
    #[serde(default)]
    pub apns: Option<ApnsSettings>,
    #[serde(default)]
    pub fcm: Option<FcmSettings>,
}

fn default_push_quiet_seconds() -> u64 {
    180
}

impl Default for PushSettings {
    fn default() -> Self {
        Self {
            quiet_seconds: default_push_quiet_seconds(),
            apns: None,
            fcm: None,
        }
    }
}

/// Apple Push Notification service credentials. The signing key stays in
/// a file the worker reads at startup, never in configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct ApnsSettings {
    /// The `.p8` key file (PKCS#8 PEM).
    pub key_file: String,
    pub key_id: String,
    pub team_id: String,
    /// The app's bundle id.
    pub topic: String,
    /// Apple's development environment, for builds signed for development.
    #[serde(default)]
    pub sandbox: bool,
}

/// Firebase Cloud Messaging credentials.
#[derive(Debug, Clone, Deserialize)]
pub struct FcmSettings {
    pub project_id: String,
    /// The service account's JSON key file.
    pub service_account_file: String,
}

/// The transactional store backend (ADR 0013).
#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DatabaseBackend {
    #[default]
    Postgres,
    Sqlite,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DeploymentProfileSetting {
    SingleNode,
    /// The safe default: only backends that support several nodes pass.
    #[default]
    MultiNode,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeploymentSettings {
    #[serde(default)]
    pub profile: DeploymentProfileSetting,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AnalyticsBackendSetting {
    #[default]
    None,
    Duckdb,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnalyticsSettings {
    #[serde(default)]
    pub backend: AnalyticsBackendSetting,
    /// Directory for one DuckDB analytics file per workspace.
    #[serde(default)]
    pub duckdb_directory: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseSettings {
    #[serde(default)]
    pub backend: DatabaseBackend,
    /// PostgreSQL connection URL; unused by other backends.
    #[serde(default)]
    pub url: String,
    /// SQLite: directory holding one database file per workspace.
    #[serde(default)]
    pub sqlite_directory: Option<String>,
    pub max_connections: u32,
    pub statement_timeout_ms: u64,
    /// Longest a request waits for a pooled connection. Bounded so that a
    /// database outage fails requests closed promptly instead of holding
    /// them for the driver's 30-second default.
    #[serde(default = "default_acquire_timeout_ms")]
    pub acquire_timeout_ms: u64,
}

fn default_acquire_timeout_ms() -> u64 {
    3_000
}

#[derive(Debug, Clone, Deserialize)]
pub struct OidcSettings {
    pub issuer_url: String,
    pub client_id: String,
    #[serde(default)]
    pub client_secret: Option<String>,
    pub redirect_url: String,
    pub post_logout_redirect_url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NatsSettings {
    pub url: String,
    pub stream: String,
    #[serde(default)]
    pub outbox: OutboxPublisherSettings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OutboxPublisherSettings {
    pub batch_size: u32,
    pub max_in_flight: u32,
    pub poll_interval_ms: u64,
    pub claim_lease_seconds: u64,
    pub publish_timeout_ms: u64,
    pub retry_base_ms: u64,
    pub retry_max_ms: u64,
    pub duplicate_window_seconds: u64,
    pub stream_max_bytes: u64,
}

impl Default for OutboxPublisherSettings {
    fn default() -> Self {
        Self {
            batch_size: 32,
            max_in_flight: 8,
            poll_interval_ms: 250,
            claim_lease_seconds: 60,
            publish_timeout_ms: 5_000,
            retry_base_ms: 1_000,
            retry_max_ms: 300_000,
            duplicate_window_seconds: 600,
            stream_max_bytes: 1_073_741_824,
        }
    }
}

/// Plugin background work hosted by the worker process.
///
/// `tenant_database_url` names the RLS-bound runtime role used for tenant
/// transactions. The worker's own `platform.database.url` role keeps only the
/// bounded cross-tenant routines. When it is absent, plugin consumers and jobs
/// are not started and readiness reports the runtime as disabled.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WorkerSettings {
    pub tenant_database_url: Option<String>,
    pub reconcile_interval_ms: u64,
    pub job_poll_interval_ms: u64,
    pub job_batch_size: u32,
    pub job_lease_seconds: u64,
    pub job_retry_base_ms: u64,
    pub job_retry_max_ms: u64,
    pub consumer_ack_wait_seconds: u64,
    pub consumer_max_deliver: u32,
    pub consumer_retry_delay_ms: u64,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            tenant_database_url: None,
            reconcile_interval_ms: 5_000,
            job_poll_interval_ms: 1_000,
            job_batch_size: 16,
            job_lease_seconds: 300,
            job_retry_base_ms: 5_000,
            job_retry_max_ms: 3_600_000,
            consumer_ack_wait_seconds: 30,
            consumer_max_deliver: 10,
            consumer_retry_delay_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionSettings {
    pub cookie_name: String,
    pub ttl_seconds: u64,
    pub renewal_window_seconds: u64,
    pub cookie_secure: bool,
    pub same_site: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HealthSettings {
    pub check_timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginSettings {
    pub manifest_paths: Vec<String>,
    pub reserved_namespaces: Vec<String>,
    pub trusted_core_repositories: Vec<String>,
    pub trusted_provenance_hosts: Vec<String>,
}

impl Default for PluginSettings {
    fn default() -> Self {
        Self {
            manifest_paths: vec!["plugins/contacts/plugin.toml".into()],
            reserved_namespaces: vec!["platform".into()],
            trusted_core_repositories: vec!["https://github.com/buchochelliq-labs/limiara-core".into()],
            trusted_provenance_hosts: vec!["github.com".into()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceSettings {
    pub api_bind: String,
    pub worker_bind: String,
    pub plugin_runner_bind: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ExternalHostSettings {
    /// Deployment-local secret used only to derive non-reversible abuse keys.
    pub fingerprint_key: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageSettings {
    pub object: StoreProviderSettings,
    pub cache: StoreProviderSettings,
    pub graph: StoreProviderSettings,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StoreProviderSettings {
    pub provider: String,
    pub bucket: Option<String>,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration source error: {0}")]
    Source(String),
    #[error("configuration field `{0}` is required")]
    Missing(&'static str),
    #[error("configuration field `{0}` is invalid")]
    Invalid(&'static str),
}

impl Settings {
    /// Load required YAML defaults, an optional YAML override, then nested
    /// `PLATFORM__...` environment values (highest precedence).
    pub fn load() -> Result<Self, ConfigError> {
        let default_file = std::env::var_os("PLATFORM_DEFAULT_CONFIG_FILE")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
        let override_file = std::env::var_os("PLATFORM_CONFIG_FILE")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from);
        Self::load_from_paths(&default_file, override_file.as_deref())
    }

    pub fn load_from_paths(
        default_file: &Path,
        override_file: Option<&Path>,
    ) -> Result<Self, ConfigError> {
        let mut builder = Config::builder().add_source(File::from(default_file).required(true));
        if let Some(path) = override_file {
            builder = builder.add_source(File::from(path).required(true));
        }
        deserialize(builder.add_source(platform_environment("PLATFORM")))
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        required("platform.environment", &self.platform.environment)?;
        valid_http_url("platform.public_base_url", &self.platform.public_base_url)?;
        match self.platform.database.backend {
            DatabaseBackend::Postgres => {
                required("platform.database.url", &self.platform.database.url)?
            }
            DatabaseBackend::Sqlite => required(
                "platform.database.sqlite_directory",
                self.platform
                    .database
                    .sqlite_directory
                    .as_deref()
                    .unwrap_or_default(),
            )?,
        }
        if self.platform.analytics.backend == AnalyticsBackendSetting::Duckdb {
            required(
                "platform.analytics.duckdb_directory",
                self.platform
                    .analytics
                    .duckdb_directory
                    .as_deref()
                    .unwrap_or_default(),
            )?;
        }
        non_zero(
            "platform.database.max_connections",
            self.platform.database.max_connections,
        )?;
        non_zero(
            "platform.database.statement_timeout_ms",
            self.platform.database.statement_timeout_ms,
        )?;
        non_zero(
            "platform.database.acquire_timeout_ms",
            self.platform.database.acquire_timeout_ms,
        )?;
        valid_http_url("platform.oidc.issuer_url", &self.platform.oidc.issuer_url)?;
        required("platform.oidc.client_id", &self.platform.oidc.client_id)?;
        valid_http_url(
            "platform.oidc.redirect_url",
            &self.platform.oidc.redirect_url,
        )?;
        valid_http_url(
            "platform.oidc.post_logout_redirect_url",
            &self.platform.oidc.post_logout_redirect_url,
        )?;
        self.validate_worker()?;
        self.validate_devices()?;
        valid_nats_url(
            "platform.nats.url",
            &self.platform.nats.url,
            !matches!(
                self.platform.environment.to_ascii_lowercase().as_str(),
                "development" | "test" | "local"
            ),
        )?;
        required("platform.nats.stream", &self.platform.nats.stream)?;
        if !safe_nats_stream_name(&self.platform.nats.stream) {
            return Err(ConfigError::Invalid("platform.nats.stream"));
        }
        let outbox = &self.platform.nats.outbox;
        bounded_non_zero("platform.nats.outbox.batch_size", outbox.batch_size, 1_000)?;
        bounded_non_zero(
            "platform.nats.outbox.max_in_flight",
            outbox.max_in_flight,
            128,
        )?;
        if outbox.max_in_flight > outbox.batch_size {
            return Err(ConfigError::Invalid("platform.nats.outbox.max_in_flight"));
        }
        bounded_non_zero(
            "platform.nats.outbox.poll_interval_ms",
            outbox.poll_interval_ms,
            60_000,
        )?;
        bounded_non_zero(
            "platform.nats.outbox.claim_lease_seconds",
            outbox.claim_lease_seconds,
            3_600,
        )?;
        bounded_non_zero(
            "platform.nats.outbox.publish_timeout_ms",
            outbox.publish_timeout_ms,
            60_000,
        )?;
        bounded_non_zero(
            "platform.nats.outbox.retry_base_ms",
            outbox.retry_base_ms,
            86_400_000,
        )?;
        bounded_non_zero(
            "platform.nats.outbox.retry_max_ms",
            outbox.retry_max_ms,
            86_400_000,
        )?;
        bounded_non_zero(
            "platform.nats.outbox.duplicate_window_seconds",
            outbox.duplicate_window_seconds,
            86_400,
        )?;
        bounded_non_zero(
            "platform.nats.outbox.stream_max_bytes",
            outbox.stream_max_bytes,
            i64::MAX as u64,
        )?;
        if outbox.retry_base_ms > outbox.retry_max_ms {
            return Err(ConfigError::Invalid("platform.nats.outbox.retry_max_ms"));
        }
        let lease_ms = outbox.claim_lease_seconds.saturating_mul(1_000);
        let idle_cycle_ms = outbox
            .poll_interval_ms
            .saturating_add(self.platform.database.statement_timeout_ms)
            .saturating_add(1_000);
        if idle_cycle_ms >= lease_ms {
            return Err(ConfigError::Invalid(
                "platform.nats.outbox.poll_interval_ms",
            ));
        }
        let usable_database_connections = self
            .platform
            .database
            .max_connections
            .saturating_sub(1)
            .max(1);
        let effective_in_flight = outbox.max_in_flight.min(usable_database_connections);
        let publication_waves = outbox.batch_size.div_ceil(effective_in_flight);
        let required_lease_ms = outbox
            .publish_timeout_ms
            .saturating_add(self.platform.database.statement_timeout_ms)
            .saturating_mul(u64::from(publication_waves))
            .saturating_add(10_000);
        if lease_ms <= required_lease_ms {
            return Err(ConfigError::Invalid(
                "platform.nats.outbox.claim_lease_seconds",
            ));
        }
        required(
            "platform.sessions.cookie_name",
            &self.platform.sessions.cookie_name,
        )?;
        non_zero(
            "platform.sessions.ttl_seconds",
            self.platform.sessions.ttl_seconds,
        )?;
        if self.platform.sessions.renewal_window_seconds >= self.platform.sessions.ttl_seconds {
            return Err(ConfigError::Invalid(
                "platform.sessions.renewal_window_seconds",
            ));
        }
        if !matches!(
            self.platform
                .sessions
                .same_site
                .to_ascii_lowercase()
                .as_str(),
            "lax" | "strict" | "none"
        ) {
            return Err(ConfigError::Invalid("platform.sessions.same_site"));
        }
        if self
            .platform
            .sessions
            .same_site
            .eq_ignore_ascii_case("none")
            && !self.platform.sessions.cookie_secure
        {
            return Err(ConfigError::Invalid("platform.sessions.cookie_secure"));
        }
        if !matches!(
            self.platform.environment.to_ascii_lowercase().as_str(),
            "development" | "test" | "local"
        ) && !self.platform.sessions.cookie_secure
        {
            return Err(ConfigError::Invalid("platform.sessions.cookie_secure"));
        }
        non_zero(
            "platform.health.check_timeout_ms",
            self.platform.health.check_timeout_ms,
        )?;
        non_empty_list(
            "platform.plugins.manifest_paths",
            &self.platform.plugins.manifest_paths,
        )?;
        non_empty_list(
            "platform.plugins.reserved_namespaces",
            &self.platform.plugins.reserved_namespaces,
        )?;
        non_empty_list(
            "platform.plugins.trusted_core_repositories",
            &self.platform.plugins.trusted_core_repositories,
        )?;
        non_empty_list(
            "platform.plugins.trusted_provenance_hosts",
            &self.platform.plugins.trusted_provenance_hosts,
        )?;
        required(
            "platform.services.api_bind",
            &self.platform.services.api_bind,
        )?;
        required(
            "platform.services.worker_bind",
            &self.platform.services.worker_bind,
        )?;
        required(
            "platform.services.plugin_runner_bind",
            &self.platform.services.plugin_runner_bind,
        )?;
        if self
            .platform
            .external_hosts
            .fingerprint_key
            .as_ref()
            .is_some_and(|key| key.len() < 32)
        {
            return Err(ConfigError::Invalid(
                "platform.external_hosts.fingerprint_key",
            ));
        }
        Ok(())
    }
}

fn platform_environment(prefix: &str) -> Environment {
    Environment::with_prefix(prefix)
        .prefix_separator("__")
        .separator("__")
        .try_parsing(true)
        .list_separator(",")
        .with_list_parse_key("platform.devices.redirect_uris")
}

fn deserialize(builder: ConfigBuilder<DefaultState>) -> Result<Settings, ConfigError> {
    let settings: Settings = builder
        .build()
        .and_then(Config::try_deserialize)
        .map_err(|error| ConfigError::Source(error.to_string()))?;
    settings.validate()?;
    Ok(settings)
}

fn required(field: &'static str, value: &str) -> Result<(), ConfigError> {
    (!value.trim().is_empty())
        .then_some(())
        .ok_or(ConfigError::Missing(field))
}

impl Settings {
    fn validate_devices(&self) -> Result<(), ConfigError> {
        let devices = &self.platform.devices;
        if !devices
            .redirect_uris
            .iter()
            .all(|uri| valid_device_redirect_uri(uri))
        {
            return Err(ConfigError::Invalid("platform.devices.redirect_uris"));
        }
        bounded_non_zero(
            "platform.devices.offline_limit_seconds",
            devices.offline_limit_seconds,
            90 * 24 * 60 * 60,
        )?;
        bounded_non_zero(
            "platform.devices.push.quiet_seconds",
            devices.push.quiet_seconds,
            3_600,
        )?;
        if let Some(apns) = &devices.push.apns {
            required("platform.devices.push.apns.key_file", &apns.key_file)?;
            required("platform.devices.push.apns.key_id", &apns.key_id)?;
            required("platform.devices.push.apns.team_id", &apns.team_id)?;
            required("platform.devices.push.apns.topic", &apns.topic)?;
        }
        if let Some(fcm) = &devices.push.fcm {
            required("platform.devices.push.fcm.project_id", &fcm.project_id)?;
            required(
                "platform.devices.push.fcm.service_account_file",
                &fcm.service_account_file,
            )?;
        }
        Ok(())
    }

    fn validate_worker(&self) -> Result<(), ConfigError> {
        let worker = &self.platform.worker;
        if let Some(url) = &worker.tenant_database_url {
            required("platform.worker.tenant_database_url", url)?;
        }
        bounded_non_zero(
            "platform.worker.reconcile_interval_ms",
            worker.reconcile_interval_ms,
            300_000,
        )?;
        bounded_non_zero(
            "platform.worker.job_poll_interval_ms",
            worker.job_poll_interval_ms,
            60_000,
        )?;
        bounded_non_zero(
            "platform.worker.job_batch_size",
            worker.job_batch_size,
            1_000,
        )?;
        bounded_non_zero(
            "platform.worker.job_lease_seconds",
            worker.job_lease_seconds,
            3_600,
        )?;
        bounded_non_zero(
            "platform.worker.job_retry_base_ms",
            worker.job_retry_base_ms,
            86_400_000,
        )?;
        bounded_non_zero(
            "platform.worker.job_retry_max_ms",
            worker.job_retry_max_ms,
            86_400_000,
        )?;
        if worker.job_retry_base_ms > worker.job_retry_max_ms {
            return Err(ConfigError::Invalid("platform.worker.job_retry_max_ms"));
        }
        // A job transaction is bounded by the statement timeout; its lease must
        // outlive it so a live run is never reclaimed by another worker.
        if worker.job_lease_seconds.saturating_mul(1_000)
            <= self.platform.database.statement_timeout_ms
        {
            return Err(ConfigError::Invalid("platform.worker.job_lease_seconds"));
        }
        bounded_non_zero(
            "platform.worker.consumer_ack_wait_seconds",
            worker.consumer_ack_wait_seconds,
            3_600,
        )?;
        bounded_non_zero(
            "platform.worker.consumer_max_deliver",
            worker.consumer_max_deliver,
            1_000,
        )?;
        bounded_non_zero(
            "platform.worker.consumer_retry_delay_ms",
            worker.consumer_retry_delay_ms,
            3_600_000,
        )?;
        Ok(())
    }
}

fn safe_nats_stream_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn non_zero(field: &'static str, value: impl Into<u64>) -> Result<(), ConfigError> {
    (value.into() > 0)
        .then_some(())
        .ok_or(ConfigError::Invalid(field))
}

fn bounded_non_zero(
    field: &'static str,
    value: impl Into<u64>,
    maximum: u64,
) -> Result<(), ConfigError> {
    let value = value.into();
    (value > 0 && value <= maximum)
        .then_some(())
        .ok_or(ConfigError::Invalid(field))
}

fn non_empty_list(field: &'static str, values: &[String]) -> Result<(), ConfigError> {
    (!values.is_empty() && values.iter().all(|value| !value.trim().is_empty()))
        .then_some(())
        .ok_or(ConfigError::Invalid(field))
}

/// A native app redirect URI (RFC 8252 §7): a private-use scheme in
/// reverse-domain form, or plain HTTP to a loopback address. Web schemes to
/// other hosts are refused, since any site could receive the code.
pub fn valid_device_redirect_uri(value: &str) -> bool {
    let Ok(url) = url::Url::parse(value) else {
        return false;
    };
    if url.fragment().is_some() || value.len() > 512 {
        return false;
    }
    match url.scheme() {
        "http" => {
            matches!(
                url.host(),
                Some(url::Host::Ipv4(address)) if address.is_loopback()
            ) || matches!(url.host(), Some(url::Host::Ipv6(address)) if address.is_loopback())
        }
        "https" => false,
        scheme => scheme.contains('.'),
    }
}

fn valid_http_url(field: &'static str, value: &str) -> Result<(), ConfigError> {
    required(field, value)?;
    (value.starts_with("http://") || value.starts_with("https://"))
        .then_some(())
        .ok_or(ConfigError::Invalid(field))
}

fn valid_nats_url(
    field: &'static str,
    value: &str,
    secure_required: bool,
) -> Result<(), ConfigError> {
    required(field, value)?;
    let url = url::Url::parse(value).map_err(|_| ConfigError::Invalid(field))?;
    let supported = matches!(url.scheme(), "nats" | "tls") && url.host_str().is_some();
    let secure = url.scheme() == "tls";
    (supported && (!secure_required || secure))
        .then_some(())
        .ok_or(ConfigError::Invalid(field))
}

#[cfg(test)]
mod tests {
    use super::*;
    use config::FileFormat;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const DEFAULTS: &str = r#"
platform:
  environment: test
  public_base_url: http://localhost:8080
  database: { url: postgres://user:secret@localhost/db, max_connections: 10, statement_timeout_ms: 5000 }
  oidc:
    issuer_url: http://localhost:8081/realms/test
    client_id: web
    redirect_url: http://localhost:8080/auth/oidc/callback
    post_logout_redirect_url: http://localhost:3000/
  nats: { url: nats://localhost:4222, stream: events }
  sessions: { cookie_name: session, ttl_seconds: 3600, renewal_window_seconds: 900, cookie_secure: false, same_site: Lax }
  health: { check_timeout_ms: 500 }
  services: { api_bind: '127.0.0.1:8080', worker_bind: '127.0.0.1:8081', plugin_runner_bind: '127.0.0.1:8082' }
  storage:
    object: { provider: local, bucket: test }
    cache: { provider: memory }
    graph: { provider: none }
"#;

    fn test_builder(override_yaml: Option<&str>, prefix: &str) -> ConfigBuilder<DefaultState> {
        let mut builder = Config::builder().add_source(File::from_str(DEFAULTS, FileFormat::Yaml));
        if let Some(contents) = override_yaml {
            builder = builder.add_source(File::from_str(contents, FileFormat::Yaml));
        }
        builder.add_source(platform_environment(prefix))
    }

    #[test]
    fn device_redirect_uris_are_native_app_uris() {
        for valid in [
            "com.example.app:/oauth",
            "com.example.app:/oauth?x=1",
            "http://127.0.0.1:8123/callback",
            "http://[::1]/callback",
        ] {
            assert!(valid_device_redirect_uri(valid), "{valid}");
        }
        for invalid in [
            "https://example.com/callback",
            "http://localhost/callback",
            "http://example.com/callback",
            "myapp:/oauth",
            "com.example.app:/oauth#fragment",
            "not a uri",
        ] {
            assert!(!valid_device_redirect_uri(invalid), "{invalid}");
        }
    }

    #[test]
    fn device_redirect_uris_come_from_a_comma_list_in_the_environment() {
        let _guard = ENV_LOCK.lock().expect("environment test lock");
        const KEY: &str = "LIMIARA_DEVICE_TEST__PLATFORM__DEVICES__REDIRECT_URIS";
        std::env::set_var(KEY, "com.example.app:/oauth,http://127.0.0.1:9/cb");
        let settings =
            deserialize(test_builder(None, "LIMIARA_DEVICE_TEST")).expect("valid layered settings");
        std::env::remove_var(KEY);
        assert_eq!(
            settings.platform.devices.redirect_uris,
            vec!["com.example.app:/oauth", "http://127.0.0.1:9/cb"]
        );
        assert_eq!(settings.platform.devices.offline_limit_seconds, 1_209_600);
        settings.validate().expect("valid device settings");
    }

    #[test]
    fn yaml_override_then_environment_have_documented_precedence() {
        let _guard = ENV_LOCK.lock().expect("environment test lock");
        const KEY: &str = "LIMIARA_CONFIG_TEST__PLATFORM__DATABASE__MAX_CONNECTIONS";
        std::env::set_var(KEY, "30");
        let settings = deserialize(test_builder(
            Some("platform:\n  database:\n    max_connections: 20"),
            "LIMIARA_CONFIG_TEST",
        ))
        .expect("valid layered settings");
        std::env::remove_var(KEY);
        assert_eq!(settings.platform.database.max_connections, 30);
        assert_eq!(settings.platform.database.statement_timeout_ms, 5000);
    }

    #[test]
    fn missing_required_field_is_named_without_secret_values() {
        let settings: Settings = Config::builder()
            .add_source(File::from_str(DEFAULTS, FileFormat::Yaml))
            .add_source(File::from_str(
                "platform:\n  database:\n    url: ''",
                FileFormat::Yaml,
            ))
            .build()
            .expect("configuration builds")
            .try_deserialize()
            .expect("configuration shape");
        let error = settings.validate().expect_err("empty URL must fail");
        assert_eq!(
            error.to_string(),
            "configuration field `platform.database.url` is required"
        );
        assert!(!error.to_string().contains("secret"));
    }

    fn settings_with(yaml: &str) -> Settings {
        Config::builder()
            .add_source(File::from_str(DEFAULTS, FileFormat::Yaml))
            .add_source(File::from_str(yaml, FileFormat::Yaml))
            .build()
            .expect("configuration builds")
            .try_deserialize()
            .expect("configuration shape")
    }

    #[test]
    fn storage_backend_settings_require_their_locations() {
        let defaults = settings_with("platform: {}");
        assert_eq!(
            defaults.platform.database.backend,
            DatabaseBackend::Postgres
        );
        assert_eq!(
            defaults.platform.deployment.profile,
            DeploymentProfileSetting::MultiNode
        );
        assert_eq!(
            defaults.platform.analytics.backend,
            AnalyticsBackendSetting::None
        );

        let sqlite = settings_with("platform:\n  database:\n    backend: sqlite\n    url: ''");
        assert!(matches!(
            sqlite.validate(),
            Err(ConfigError::Missing("platform.database.sqlite_directory"))
        ));
        let sqlite = settings_with(
            "platform:\n  database:\n    backend: sqlite\n    url: ''\n    sqlite_directory: /var/lib/limiara",
        );
        sqlite.validate().expect("sqlite needs no postgres url");

        let duckdb = settings_with("platform:\n  analytics:\n    backend: duckdb");
        assert!(matches!(
            duckdb.validate(),
            Err(ConfigError::Missing("platform.analytics.duckdb_directory"))
        ));
    }

    #[test]
    fn invalid_session_policy_is_field_level() {
        let error = deserialize(test_builder(
            Some("platform:\n  sessions:\n    same_site: Sometimes"),
            "LIMIARA_UNUSED_TEST",
        ))
        .expect_err("invalid SameSite must fail");
        assert!(error.to_string().contains("platform.sessions.same_site"));
    }

    #[test]
    fn production_rejects_insecure_session_cookies() {
        let error = deserialize(test_builder(
            Some("platform:\n  environment: production\n  nats:\n    url: tls://localhost:4222"),
            "LIMIARA_UNUSED_PRODUCTION_TEST",
        ))
        .expect_err("production cookies must be secure");
        assert!(error
            .to_string()
            .contains("platform.sessions.cookie_secure"));
    }

    #[test]
    fn production_requires_an_encrypted_nats_transport() {
        let error = deserialize(test_builder(
            Some("platform:\n  environment: production\n  sessions:\n    cookie_secure: true"),
            "LIMIARA_UNUSED_PRODUCTION_NATS",
        ))
        .expect_err("production event transport must be encrypted");
        assert!(error.to_string().contains("platform.nats.url"));
        assert!(!error.to_string().contains("localhost:4222"));
    }

    #[test]
    fn outbox_defaults_are_bounded_and_invalid_concurrency_is_rejected() {
        let settings = deserialize(test_builder(None, "LIMIARA_UNUSED_OUTBOX_DEFAULTS"))
            .expect("valid defaults");
        assert_eq!(settings.platform.nats.outbox.batch_size, 32);
        assert_eq!(settings.platform.nats.outbox.max_in_flight, 8);
        assert_eq!(settings.platform.nats.outbox.retry_max_ms, 300_000);
        assert_eq!(
            settings.platform.nats.outbox.stream_max_bytes,
            1_073_741_824
        );

        let error = deserialize(test_builder(
            Some("platform:\n  nats:\n    outbox:\n      batch_size: 2\n      max_in_flight: 3"),
            "LIMIARA_UNUSED_OUTBOX_INVALID",
        ))
        .expect_err("in-flight limit cannot exceed a claim batch");
        assert!(error
            .to_string()
            .contains("platform.nats.outbox.max_in_flight"));

        let error = deserialize(test_builder(
            Some(
                "platform:\n  nats:\n    outbox:\n      claim_lease_seconds: 18446744073709551615",
            ),
            "LIMIARA_UNUSED_OUTBOX_OVERFLOW",
        ))
        .expect_err("durations must remain representable by the storage layer");
        assert!(error
            .to_string()
            .contains("platform.nats.outbox.claim_lease_seconds"));

        let error = deserialize(test_builder(
            Some("platform:\n  nats:\n    outbox:\n      batch_size: 100\n      max_in_flight: 1\n      claim_lease_seconds: 60"),
            "LIMIARA_UNUSED_OUTBOX_SHORT_LEASE",
        ))
        .expect_err("a claim must cover every bounded publication wave");
        assert!(error
            .to_string()
            .contains("platform.nats.outbox.claim_lease_seconds"));

        let error = deserialize(test_builder(
            Some("platform:\n  nats:\n    outbox:\n      batch_size: 1\n      max_in_flight: 1\n      publish_timeout_ms: 1\n      claim_lease_seconds: 16\n      poll_interval_ms: 16000"),
            "LIMIARA_UNUSED_OUTBOX_SLOW_POLL",
        ))
        .expect_err("the publisher heartbeat must refresh before its lease horizon");
        assert!(error
            .to_string()
            .contains("platform.nats.outbox.poll_interval_ms"));
    }

    #[test]
    fn worker_plugin_runtime_settings_default_safely_and_reject_unsafe_leases() {
        let settings = deserialize(test_builder(None, "LIMIARA_UNUSED_WORKER_DEFAULTS"))
            .expect("worker settings default");
        assert!(settings.platform.worker.tenant_database_url.is_none());
        assert_eq!(settings.platform.worker.job_lease_seconds, 300);

        let error = deserialize(test_builder(
            Some("platform:\n  worker:\n    job_lease_seconds: 5"),
            "LIMIARA_UNUSED_WORKER_SHORT_LEASE",
        ))
        .expect_err("a job lease must outlive the statement timeout");
        assert!(error
            .to_string()
            .contains("platform.worker.job_lease_seconds"));

        let error = deserialize(test_builder(
            Some("platform:\n  worker:\n    job_retry_base_ms: 10\n    job_retry_max_ms: 1"),
            "LIMIARA_UNUSED_WORKER_RETRY",
        ))
        .expect_err("retry base cannot exceed its cap");
        assert!(error
            .to_string()
            .contains("platform.worker.job_retry_max_ms"));
    }
}
