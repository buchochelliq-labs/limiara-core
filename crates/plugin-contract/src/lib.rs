#![forbid(unsafe_code)]
pub mod objects;
pub mod ui;

pub use objects::{
    ObjectDecl, ObjectDeleteBehaviour, ObjectFieldDecl, ObjectFieldOption, ObjectFieldType,
    ObjectSharingDecl,
};

use regex::Regex;
use semver::{Version, VersionReq};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Runtime {
    Native,
    Wasm,
    ExternalService,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compatibility {
    pub platform: VersionReq,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Entrypoints {
    pub api: Option<bool>,
    pub worker: Option<bool>,
    pub web: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionDecl {
    pub id: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventDecls {
    #[serde(default)]
    pub publish: Vec<EventType>,
    #[serde(default)]
    pub consume: Vec<EventType>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventType {
    #[serde(rename = "type")]
    pub event_type: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiDecl {
    #[serde(default)]
    pub navigation: Vec<NavigationItem>,
    #[serde(default)]
    pub resources: Vec<UiFileReference>,
    #[serde(default)]
    pub pages: Vec<UiFileReference>,
    #[serde(default)]
    pub fragments: Vec<UiFileReference>,
    #[serde(default)]
    pub extensions: Vec<UiFileReference>,
    #[serde(default)]
    pub components: Vec<UiFileReference>,
    /// Places in this plugin's pages where an administrator may put fields
    /// other plugins add to its resources (ADR 0012).
    #[serde(default)]
    pub field_zones: Vec<FieldZoneDecl>,
}

/// A named place on a host page that can hold extension fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldZoneDecl {
    /// Short id, unique per resource, for example `details`.
    pub id: String,
    /// The host resource whose extension fields may be placed here.
    pub resource: String,
    pub label: String,
    pub kind: FieldZoneKind,
    /// The page and node the zone's fields are rendered in.
    pub page: String,
    pub node: String,
    /// Fields an administrator has not arranged appear in the default zone
    /// of each kind; at most one per resource and kind.
    #[serde(default)]
    pub default: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum FieldZoneKind {
    /// Read-only field lists on a record page.
    Display,
    /// Editable fields on a record form.
    Form,
    /// Columns of a record list.
    List,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiFileReference {
    pub id: String,
    pub path: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationItem {
    pub id: String,
    pub label: String,
    pub route: String,
    pub icon: Option<String>,
    pub order: i32,
    #[serde(default)]
    pub permission: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Capabilities {
    #[serde(default)]
    pub database: bool,
    #[serde(default)]
    pub audit: bool,
    #[serde(default)]
    pub event_publish: Vec<String>,
    /// Event types the plugin may receive through declared worker consumers.
    #[serde(default)]
    pub event_consume: Vec<String>,
    #[serde(default)]
    pub outbound_http: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<String>,
    #[serde(default)]
    pub object_storage: bool,
}
/// Background work a plugin contributes to the worker.
///
/// Declarations are inert data: the worker runs only contributions that are
/// both declared here and linked as trusted native code, and only for tenants
/// whose installation is active.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkerDecl {
    #[serde(default)]
    pub consumers: Vec<ConsumerDecl>,
    #[serde(default)]
    pub jobs: Vec<JobDecl>,
}
/// A durable event consumer. The id is the stable inbox consumer identity and
/// must never be reused for different semantics.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConsumerDecl {
    pub id: String,
    pub events: Vec<String>,
}
/// A recurring tenant-scoped job with a fixed interval.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobDecl {
    pub id: String,
    pub interval_seconds: u32,
}
pub const MIN_JOB_INTERVAL_SECONDS: u32 = 60;
pub const MAX_JOB_INTERVAL_SECONDS: u32 = 7 * 24 * 60 * 60;
const MAX_CONTRIBUTION_ID_BYTES: usize = 100;
/// Storage backends a plugin's linked store adapters support (ADR 0013).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum StorageBackend {
    Postgres,
    Sqlite,
}

impl StorageBackend {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct StorageDecl {
    /// Backends the plugin ships store adapters for. Empty means the plugin
    /// predates ADR 0013 and supports PostgreSQL only.
    #[serde(default)]
    pub backends: Vec<StorageBackend>,
}

/// Additions this plugin makes to other plugins' records (ADR 0012).
///
/// Declarations are inert data. The kernel extension service decides at
/// runtime whether each field is active, and hosts store values only through
/// that service.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ExtensionsDecl {
    #[serde(default)]
    pub fields: Vec<ExtensionFieldDecl>,
}

/// One simple field added to a record resource owned by another plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionFieldDecl {
    /// The plugin that owns the target resource.
    pub host: String,
    /// The host resource the field is added to, for example
    /// `platform.contacts.person`.
    pub target: String,
    /// Short field key, unique per target within this plugin.
    pub key: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub field_type: ExtensionFieldType,
    #[serde(default)]
    pub options: Vec<ExtensionFieldOption>,
    /// Maximum characters for text fields; defaults to
    /// [`DEFAULT_EXTENSION_TEXT_LENGTH`].
    #[serde(default)]
    pub max_length: Option<u16>,
    /// Permission, declared by this plugin, that lets a user read the field.
    pub read_permission: String,
    /// Permission, declared by this plugin, that lets a user write the field.
    pub write_permission: String,
    #[serde(default)]
    pub classification: Option<ui::DataClassification>,
}

/// Value types an extension may store with a host record.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionFieldType {
    Text,
    Number,
    Boolean,
    Date,
    Choice,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtensionFieldOption {
    pub value: String,
    pub label: String,
}

pub const MAX_EXTENSION_FIELDS: usize = 100;
pub const DEFAULT_EXTENSION_TEXT_LENGTH: u16 = 500;
pub const MAX_EXTENSION_TEXT_LENGTH: u16 = 2000;
pub const MAX_EXTENSION_OPTIONS: usize = 50;
const MAX_EXTENSION_LABEL_CHARS: usize = 80;
const MAX_EXTENSION_DESCRIPTION_CHARS: usize = 300;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Entitlements {
    #[serde(default)]
    pub required: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PluginDistribution {
    CoreOob,
    FirstParty,
    ThirdParty,
    LocalDevelopment,
}
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SupplyChain {
    pub distribution: Option<PluginDistribution>,
    pub source_repository: Option<String>,
    pub source_revision: Option<String>,
    pub build_provenance: Option<String>,
    pub artifact_digest: Option<String>,
    pub artifact_path: Option<String>,
    pub signature: Option<String>,
    pub sbom: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyReason {
    Composition,
    Extension,
    EventContract,
    ReadModel,
    CapabilityProvider,
    MigrationOrder,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyDecl {
    pub version: VersionReq,
    #[serde(default)]
    pub reasons: Vec<DependencyReason>,
    pub description: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub manifest_version: u16,
    pub id: String,
    pub name: String,
    pub version: Version,
    pub description: String,
    pub runtime: Runtime,
    pub compatibility: Compatibility,
    #[serde(default)]
    pub entrypoints: Entrypoints,
    #[serde(default)]
    pub dependencies: BTreeMap<String, DependencyDecl>,
    #[serde(default)]
    pub permissions: Vec<PermissionDecl>,
    #[serde(default)]
    pub events: EventDecls,
    #[serde(default)]
    pub ui: UiDecl,
    #[serde(default)]
    pub capabilities: Capabilities,
    #[serde(default)]
    pub worker: WorkerDecl,
    #[serde(default)]
    pub extensions: ExtensionsDecl,
    /// Engine objects the plugin declares in its own namespace (ADR 0033
    /// §2), created or migrated in a workspace when it enables the plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<ObjectDecl>,
    #[serde(default)]
    pub storage: StorageDecl,
    #[serde(default)]
    pub entitlements: Entitlements,
    #[serde(default)]
    pub supply_chain: SupplyChain,
    pub configuration: Option<BTreeMap<String, String>>,
    pub checksum: Option<String>,
    pub signature: Option<String>,
    pub sbom: Option<String>,
}
#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("manifest parse failed: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported manifest version {0}")]
    Version(u16),
    #[error("invalid plugin id {0}")]
    Id(String),
    #[error("platform version {platform} incompatible with {requirement}")]
    Incompatible {
        platform: Version,
        requirement: VersionReq,
    },
    #[error("core out-of-box plugin {id} is missing supply-chain field {field}")]
    MissingCoreSupplyChainField { id: String, field: &'static str },
    #[error("invalid UI metadata reference {id}: {reason}")]
    InvalidUiReference { id: String, reason: String },
    #[error("invalid worker contribution {id}: {reason}")]
    InvalidWorkerContribution { id: String, reason: &'static str },
    #[error("invalid storage declaration: {0}")]
    InvalidStorage(&'static str),
    #[error("invalid field zone {resource}/{id}: {reason}")]
    InvalidFieldZone {
        resource: String,
        id: String,
        reason: &'static str,
    },
    #[error("invalid extension field {target}/{key}: {reason}")]
    InvalidExtensionField {
        target: String,
        key: String,
        reason: &'static str,
    },
    #[error("invalid object {object}: {reason}")]
    InvalidObject {
        object: String,
        reason: &'static str,
    },
    #[error("invalid object field {object}/{field}: {reason}")]
    InvalidObjectField {
        object: String,
        field: String,
        reason: &'static str,
    },
}
impl Manifest {
    pub fn parse_toml(input: &str) -> Result<Self, ManifestError> {
        let m: Manifest = toml::from_str(input)?;
        m.validate(&Version::new(0, 1, 0))?;
        Ok(m)
    }
    pub fn validate(&self, platform: &Version) -> Result<(), ManifestError> {
        if self.manifest_version != 1 {
            return Err(ManifestError::Version(self.manifest_version));
        }
        let re = Regex::new(r"^[a-z0-9]+(\.[a-z0-9-]+)+$").expect("valid regex");
        if !re.is_match(&self.id) {
            return Err(ManifestError::Id(self.id.clone()));
        }
        if !self.compatibility.platform.matches(platform) {
            return Err(ManifestError::Incompatible {
                platform: platform.clone(),
                requirement: self.compatibility.platform.clone(),
            });
        }
        if self.supply_chain.distribution == Some(PluginDistribution::CoreOob) {
            for (field, value) in [
                ("source_repository", &self.supply_chain.source_repository),
                ("source_revision", &self.supply_chain.source_revision),
                ("build_provenance", &self.supply_chain.build_provenance),
                ("artifact_digest", &self.supply_chain.artifact_digest),
                ("artifact_path", &self.supply_chain.artifact_path),
                ("signature", &self.supply_chain.signature),
                ("sbom", &self.supply_chain.sbom),
            ] {
                if value.as_deref().unwrap_or_default().is_empty() {
                    return Err(ManifestError::MissingCoreSupplyChainField {
                        id: self.id.clone(),
                        field,
                    });
                }
            }
        }
        validate_ui_references(&self.ui)?;
        self.validate_worker_contributions()?;
        self.validate_extension_fields()?;
        self.validate_objects()?;
        self.validate_field_zones()?;
        let mut backends = self.storage.backends.clone();
        backends.sort();
        backends.dedup();
        if backends.len() != self.storage.backends.len() {
            return Err(ManifestError::InvalidStorage("duplicate storage backend"));
        }
        Ok(())
    }

    /// Whether the plugin's linked store adapters support `backend`.
    pub fn supports_storage(&self, backend: StorageBackend) -> bool {
        if self.storage.backends.is_empty() {
            backend == StorageBackend::Postgres
        } else {
            self.storage.backends.contains(&backend)
        }
    }

    fn validate_objects(&self) -> Result<(), ManifestError> {
        objects::validate_objects(
            &self.id,
            &self.objects,
            self.dependencies.keys().map(String::as_str),
        )
        .map_err(|issue| match issue.field {
            Some(field) => ManifestError::InvalidObjectField {
                object: issue.object,
                field,
                reason: issue.reason,
            },
            None => ManifestError::InvalidObject {
                object: issue.object,
                reason: issue.reason,
            },
        })
    }

    fn validate_field_zones(&self) -> Result<(), ManifestError> {
        let zones = &self.ui.field_zones;
        let invalid = |zone: &FieldZoneDecl, reason| ManifestError::InvalidFieldZone {
            resource: zone.resource.clone(),
            id: zone.id.clone(),
            reason,
        };
        let id_pattern = Regex::new(r"^[a-z][a-z0-9-]{0,39}$").expect("valid regex");
        let node_pattern = Regex::new(r"^[a-z0-9][a-z0-9_.-]{0,99}$").expect("valid regex");
        let mut ids = std::collections::BTreeSet::new();
        let mut defaults = std::collections::BTreeSet::new();
        for zone in zones {
            if !id_pattern.is_match(&zone.id) {
                return Err(invalid(zone, "id must be a short lowercase identifier"));
            }
            if !self
                .ui
                .resources
                .iter()
                .any(|resource| resource.id == zone.resource)
            {
                return Err(invalid(zone, "resource must be declared by this plugin"));
            }
            if !self.ui.pages.iter().any(|page| page.id == zone.page) {
                return Err(invalid(zone, "page must be declared by this plugin"));
            }
            if !node_pattern.is_match(&zone.node) {
                return Err(invalid(zone, "node must be a page node id"));
            }
            let label = zone.label.trim();
            if label.is_empty()
                || label.chars().count() > MAX_EXTENSION_LABEL_CHARS
                || label.chars().any(char::is_control)
            {
                return Err(invalid(zone, "label must be 1 to 80 printable characters"));
            }
            if !ids.insert((zone.resource.as_str(), zone.id.as_str())) {
                return Err(invalid(zone, "duplicate zone id for this resource"));
            }
            if zone.default && !defaults.insert((zone.resource.as_str(), zone.kind)) {
                return Err(invalid(zone, "only one default zone per resource and kind"));
            }
        }
        Ok(())
    }

    fn validate_extension_fields(&self) -> Result<(), ManifestError> {
        let fields = &self.extensions.fields;
        if fields.is_empty() {
            return Ok(());
        }
        let invalid = |field: &ExtensionFieldDecl, reason| ManifestError::InvalidExtensionField {
            target: field.target.clone(),
            key: field.key.clone(),
            reason,
        };
        if fields.len() > MAX_EXTENSION_FIELDS {
            return Err(invalid(&fields[0], "too many extension fields"));
        }
        let plugin_id = Regex::new(r"^[a-z0-9]+(\.[a-z0-9-]+)+$").expect("valid regex");
        let key_pattern = Regex::new(r"^[a-z][a-z0-9_]{0,39}$").expect("valid regex");
        let option_pattern = Regex::new(r"^[a-z0-9][a-z0-9_-]{0,39}$").expect("valid regex");
        let declared_permissions = self
            .permissions
            .iter()
            .map(|permission| permission.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let mut seen = std::collections::BTreeSet::new();
        for field in fields {
            if !plugin_id.is_match(&field.host) {
                return Err(invalid(field, "host must be a plugin id"));
            }
            if field.host == self.id {
                return Err(invalid(field, "a plugin cannot extend its own resources"));
            }
            let extends_host = self
                .dependencies
                .get(&field.host)
                .is_some_and(|dependency| {
                    dependency.reasons.contains(&DependencyReason::Extension)
                });
            if !extends_host {
                return Err(invalid(
                    field,
                    "host must be a dependency declared with the extension reason",
                ));
            }
            let in_host_namespace = field
                .target
                .strip_prefix(field.host.as_str())
                .and_then(|suffix| suffix.strip_prefix('.'))
                .is_some_and(|suffix| {
                    !suffix.is_empty()
                        && suffix.len() <= MAX_CONTRIBUTION_ID_BYTES
                        && suffix.split('.').all(|token| {
                            !token.is_empty()
                                && token.bytes().all(|byte| {
                                    byte.is_ascii_lowercase()
                                        || byte.is_ascii_digit()
                                        || byte == b'-'
                                })
                        })
                });
            if !in_host_namespace {
                return Err(invalid(
                    field,
                    "target must be a resource id owned by the host",
                ));
            }
            if !key_pattern.is_match(&field.key) {
                return Err(invalid(
                    field,
                    "key must be a short lowercase identifier starting with a letter",
                ));
            }
            if !seen.insert((field.target.as_str(), field.key.as_str())) {
                return Err(invalid(field, "duplicate key for this target"));
            }
            let label = field.label.trim();
            if label.is_empty()
                || label.chars().count() > MAX_EXTENSION_LABEL_CHARS
                || label.chars().any(char::is_control)
            {
                return Err(invalid(field, "label must be 1 to 80 printable characters"));
            }
            if field.description.as_deref().is_some_and(|description| {
                description.chars().count() > MAX_EXTENSION_DESCRIPTION_CHARS
                    || description
                        .chars()
                        .any(|character| character.is_control() && character != '\n')
            }) {
                return Err(invalid(field, "description is too long or not printable"));
            }
            for permission in [&field.read_permission, &field.write_permission] {
                if !declared_permissions.contains(permission.as_str()) {
                    return Err(invalid(
                        field,
                        "read and write permissions must be declared by this plugin",
                    ));
                }
            }
            let is_choice = field.field_type == ExtensionFieldType::Choice;
            if is_choice
                && (field.options.is_empty() || field.options.len() > MAX_EXTENSION_OPTIONS)
            {
                return Err(invalid(field, "choice fields need 1 to 50 options"));
            }
            if !is_choice && !field.options.is_empty() {
                return Err(invalid(field, "only choice fields take options"));
            }
            let mut option_values = std::collections::BTreeSet::new();
            for option in &field.options {
                let label = option.label.trim();
                if !option_pattern.is_match(&option.value)
                    || !option_values.insert(option.value.as_str())
                    || label.is_empty()
                    || label.chars().count() > MAX_EXTENSION_LABEL_CHARS
                    || label.chars().any(char::is_control)
                {
                    return Err(invalid(
                        field,
                        "options need unique lowercase values and printable labels",
                    ));
                }
            }
            match (field.field_type, field.max_length) {
                (ExtensionFieldType::Text, Some(length))
                    if length == 0 || length > MAX_EXTENSION_TEXT_LENGTH =>
                {
                    return Err(invalid(field, "max_length must be between 1 and 2000"));
                }
                (ExtensionFieldType::Text, _) | (_, None) => {}
                (_, Some(_)) => {
                    return Err(invalid(field, "only text fields take max_length"));
                }
            }
        }
        Ok(())
    }

    fn validate_worker_contributions(&self) -> Result<(), ManifestError> {
        let worker = &self.worker;
        if worker.consumers.is_empty() && worker.jobs.is_empty() {
            return Ok(());
        }
        let invalid = |id: &str, reason| ManifestError::InvalidWorkerContribution {
            id: id.to_owned(),
            reason,
        };
        if self.entrypoints.worker != Some(true) {
            let id = worker
                .consumers
                .first()
                .map(|consumer| consumer.id.as_str())
                .or_else(|| worker.jobs.first().map(|job| job.id.as_str()))
                .unwrap_or_default();
            return Err(invalid(
                id,
                "worker contributions require entrypoints.worker",
            ));
        }
        let declared_consume = self
            .events
            .consume
            .iter()
            .map(|event| event.event_type.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let mut ids = std::collections::BTreeSet::new();
        let contribution_ids = worker
            .consumers
            .iter()
            .map(|consumer| consumer.id.as_str())
            .chain(worker.jobs.iter().map(|job| job.id.as_str()));
        for id in contribution_ids {
            if !self.owns_contribution_id(id) {
                return Err(invalid(
                    id,
                    "id must be a bounded lowercase identifier under the plugin id",
                ));
            }
            if !ids.insert(id) {
                return Err(invalid(id, "duplicate worker contribution id"));
            }
        }
        for consumer in &worker.consumers {
            if consumer.events.is_empty() {
                return Err(invalid(
                    &consumer.id,
                    "consumer must declare at least one event",
                ));
            }
            for event in &consumer.events {
                if !safe_event_type(event) {
                    return Err(invalid(
                        &consumer.id,
                        "consumed event types must be dotted lowercase identifiers without wildcards",
                    ));
                }
                if !declared_consume.contains(event.as_str())
                    || !self.capabilities.event_consume.contains(event)
                {
                    return Err(invalid(
                        &consumer.id,
                        "consumed events must be declared in events.consume and capabilities.event_consume",
                    ));
                }
            }
        }
        for job in &worker.jobs {
            if !(MIN_JOB_INTERVAL_SECONDS..=MAX_JOB_INTERVAL_SECONDS)
                .contains(&job.interval_seconds)
            {
                return Err(invalid(
                    &job.id,
                    "job interval is outside the supported range",
                ));
            }
        }
        Ok(())
    }

    /// Whether a contribution id is a safe identifier namespaced by this plugin.
    pub fn owns_contribution_id(&self, id: &str) -> bool {
        id.len() <= MAX_CONTRIBUTION_ID_BYTES
            && id
                .strip_prefix(self.id.as_str())
                .and_then(|suffix| suffix.strip_prefix('.'))
                .is_some_and(|suffix| {
                    suffix
                        .as_bytes()
                        .first()
                        .is_some_and(u8::is_ascii_lowercase)
                        && suffix
                            .as_bytes()
                            .last()
                            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                        && suffix.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'.' | b'-')
                        })
                })
    }
}

/// A broker-safe event type: at least two dotted tokens of lowercase letters,
/// digits, and hyphens. Rejects NATS wildcards, whitespace, and empty tokens.
fn safe_event_type(value: &str) -> bool {
    value.len() <= 200
        && value.split('.').count() >= 2
        && value.split('.').all(|token| {
            !token.is_empty()
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn validate_ui_references(ui: &UiDecl) -> Result<(), ManifestError> {
    let id_pattern = Regex::new(r"^[a-z0-9]+([.-][a-z0-9-]+)+$").expect("valid regex");
    for references in [
        &ui.resources,
        &ui.pages,
        &ui.fragments,
        &ui.extensions,
        &ui.components,
    ] {
        let mut ids = std::collections::BTreeSet::new();
        for reference in references {
            if !id_pattern.is_match(&reference.id) {
                return Err(ManifestError::InvalidUiReference {
                    id: reference.id.clone(),
                    reason: "id must be a stable dotted or namespaced lowercase identifier".into(),
                });
            }
            if !ids.insert(&reference.id) {
                return Err(ManifestError::InvalidUiReference {
                    id: reference.id.clone(),
                    reason: "duplicate id in the same metadata collection".into(),
                });
            }
            let path = std::path::Path::new(&reference.path);
            if path.is_absolute()
                || reference.path.contains("..")
                || reference.path.contains('\\')
                || !reference.path.ends_with(".json")
            {
                return Err(ManifestError::InvalidUiReference {
                    id: reference.id.clone(),
                    reason: "path must be a relative forward-slash JSON path without traversal"
                        .into(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Manifest, ManifestError};

    fn base_manifest(extra: &str) -> String {
        format!(
            r#"
manifest_version = 1
id = "platform.contacts"
name = "Contacts"
version = "0.1.0"
description = "People and organisations"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

{extra}
"#
        )
    }

    #[test]
    fn core_oob_plugins_require_complete_supply_chain_metadata() {
        let err = Manifest::parse_toml(&base_manifest(
            r#"
[supply_chain]
distribution = "core-oob"
"#,
        ))
        .expect_err("core plugins without provenance must be rejected");

        assert!(matches!(
            err,
            ManifestError::MissingCoreSupplyChainField {
                field: "source_repository",
                ..
            }
        ));
    }

    #[test]
    fn core_oob_plugins_accept_complete_supply_chain_metadata() {
        Manifest::parse_toml(&base_manifest(
            r#"
[supply_chain]
distribution = "core-oob"
source_repository = "https://example.invalid/limiara"
source_revision = "0123456789abcdef0123456789abcdef01234567"
build_provenance = "https://example.invalid/provenance/platform.contacts.intoto.jsonl"
artifact_digest = "sha256:0000000000000000000000000000000000000000000000000000000000000000"
artifact_path = "artifact/native.link"
signature = "https://example.invalid/signatures/platform.contacts.sig"
sbom = "https://example.invalid/sbom/platform.contacts.spdx.json"
"#,
        ))
        .expect("complete core plugin provenance should validate");
    }

    #[test]
    fn navigation_only_manifests_remain_compatible() {
        Manifest::parse_toml(&base_manifest(
            r#"
[[ui.navigation]]
id = "contacts"
label = "Contacts"
route = "/contacts"
order = 10
"#,
        ))
        .expect("existing navigation-only manifests remain valid");
    }

    const WORKER_FIXTURE: &str = r#"
[entrypoints]
worker = true

[[events.consume]]
type = "platform.contacts.person-created.v1"

[capabilities]
event_consume = ["platform.contacts.person-created.v1"]

[[worker.consumers]]
id = "platform.contacts.timeline-v1"
events = ["platform.contacts.person-created.v1"]

[[worker.jobs]]
id = "platform.contacts.digest-v1"
interval_seconds = 3600
"#;

    #[test]
    fn worker_contributions_are_namespaced_declared_and_bounded() {
        let manifest =
            Manifest::parse_toml(&base_manifest(WORKER_FIXTURE)).expect("valid worker fixture");
        assert_eq!(manifest.worker.consumers.len(), 1);
        assert_eq!(manifest.worker.jobs[0].interval_seconds, 3600);

        for (from, to) in [
            ("worker = true", "worker = false"),
            (
                "id = \"platform.contacts.timeline-v1\"",
                "id = \"platform.other.timeline-v1\"",
            ),
            (
                "id = \"platform.contacts.digest-v1\"",
                "id = \"platform.contacts.timeline-v1\"",
            ),
            (
                "id = \"platform.contacts.digest-v1\"",
                "id = \"platform.contacts.Digest\"",
            ),
            (
                "event_consume = [\"platform.contacts.person-created.v1\"]",
                "event_consume = []",
            ),
            ("interval_seconds = 3600", "interval_seconds = 1"),
            ("platform.contacts.person-created.v1", "platform.contacts.>"),
        ] {
            let fixture = WORKER_FIXTURE.replace(from, to);
            assert_ne!(fixture, WORKER_FIXTURE, "{from}");
            let error = Manifest::parse_toml(&base_manifest(&fixture))
                .expect_err("invalid worker contribution must be rejected");
            assert!(
                matches!(error, ManifestError::InvalidWorkerContribution { .. }),
                "{from}: {error}"
            );
        }
    }

    const EXTENSION_FIXTURE: &str = r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty tiers"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0,<0.2.0"
reasons = ["extension"]

[dependencies."example.billing"]
version = "^1.0.0"
reasons = ["extension", "event-contract"]

[[permissions]]
id = "loyalty.tier.read"
description = "Read loyalty tiers"
[[permissions]]
id = "loyalty.tier.write"
description = "Change loyalty tiers"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Loyalty tier"
field_type = "choice"
options = [{ value = "gold", label = "Gold" }, { value = "silver", label = "Silver" }]
read_permission = "loyalty.tier.read"
write_permission = "loyalty.tier.write"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.organisation"
key = "tier"
label = "Account tier"
field_type = "text"
max_length = 40
read_permission = "loyalty.tier.read"
write_permission = "loyalty.tier.write"

[[extensions.fields]]
host = "example.billing"
target = "example.billing.account"
key = "member_since"
label = "Member since"
field_type = "date"
read_permission = "loyalty.tier.read"
write_permission = "loyalty.tier.write"
"#;

    #[test]
    fn one_plugin_may_extend_several_hosts_and_resources() {
        let manifest = Manifest::parse_toml(EXTENSION_FIXTURE).expect("valid extension fixture");
        let targets = manifest
            .extensions
            .fields
            .iter()
            .map(|field| field.target.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            targets,
            [
                "platform.contacts.person",
                "platform.contacts.organisation",
                "example.billing.account"
            ]
        );
        assert_eq!(
            manifest.extensions.fields[0].field_type,
            super::ExtensionFieldType::Choice
        );
    }

    #[test]
    fn extension_fields_only_add_declared_namespaced_typed_values() {
        for (from, to) in [
            (
                "host = \"platform.contacts\"\ntarget = \"platform.contacts.person\"",
                "host = \"example.loyalty\"\ntarget = \"example.loyalty.person\"",
            ),
            (
                "reasons = [\"extension\"]",
                "reasons = [\"composition\"]",
            ),
            (
                "target = \"platform.contacts.person\"",
                "target = \"platform.other.person\"",
            ),
            (
                "target = \"platform.contacts.person\"",
                "target = \"platform.contacts\"",
            ),
            ("key = \"tier\"\nlabel = \"Loyalty", "key = \"Tier\"\nlabel = \"Loyalty"),
            ("label = \"Loyalty tier\"", "label = \"  \""),
            (
                "write_permission = \"loyalty.tier.write\"\n\n[[extensions.fields]]\nhost = \"platform.contacts\"\ntarget = \"platform.contacts.organisation\"",
                "write_permission = \"contacts.people.write\"\n\n[[extensions.fields]]\nhost = \"platform.contacts\"\ntarget = \"platform.contacts.organisation\"",
            ),
            (
                "options = [{ value = \"gold\", label = \"Gold\" }, { value = \"silver\", label = \"Silver\" }]",
                "options = []",
            ),
            (
                "options = [{ value = \"gold\", label = \"Gold\" }, { value = \"silver\", label = \"Silver\" }]",
                "options = [{ value = \"gold\", label = \"Gold\" }, { value = \"gold\", label = \"Again\" }]",
            ),
            ("max_length = 40", "max_length = 4000"),
            (
                "field_type = \"date\"",
                "field_type = \"date\"\nmax_length = 10",
            ),
            (
                "field_type = \"date\"",
                "field_type = \"date\"\noptions = [{ value = \"a\", label = \"A\" }]",
            ),
        ] {
            let fixture = EXTENSION_FIXTURE.replacen(from, to, 1);
            assert_ne!(fixture, EXTENSION_FIXTURE, "{from}");
            match Manifest::parse_toml(&fixture) {
                Err(ManifestError::InvalidExtensionField { .. }) => {}
                other => panic!("{from}: expected rejection, got {other:?}"),
            }
        }
    }

    #[test]
    fn duplicate_keys_per_target_are_rejected() {
        let fixture = EXTENSION_FIXTURE.replacen(
            "target = \"platform.contacts.organisation\"",
            "target = \"platform.contacts.person\"",
            1,
        );
        assert!(matches!(
            Manifest::parse_toml(&fixture),
            Err(ManifestError::InvalidExtensionField {
                reason: "duplicate key for this target",
                ..
            })
        ));
    }

    #[test]
    fn field_zones_name_own_resources_pages_and_one_default_per_kind() {
        let fixture = r#"
[[ui.resources]]
id = "platform.contacts.person"
path = "ui/resources/person.json"

[[ui.pages]]
id = "platform.contacts.person-detail"
path = "ui/pages/person-detail.json"

[[ui.field_zones]]
id = "details"
resource = "platform.contacts.person"
label = "Contact information"
kind = "display"
page = "platform.contacts.person-detail"
node = "person-identity-summary"
default = true

[[ui.field_zones]]
id = "highlights"
resource = "platform.contacts.person"
label = "Highlights"
kind = "display"
page = "platform.contacts.person-detail"
node = "person-highlights"
"#;
        let manifest = Manifest::parse_toml(&base_manifest(fixture)).expect("valid zones");
        assert_eq!(manifest.ui.field_zones.len(), 2);
        for (from, to) in [
            ("id = \"highlights\"", "id = \"details\""),
            ("id = \"highlights\"", "id = \"High lights\""),
            (
                "node = \"person-highlights\"",
                "node = \"person-highlights\"\ndefault = true",
            ),
            (
                "resource = \"platform.contacts.person\"\nlabel = \"Highlights\"",
                "resource = \"platform.other.thing\"\nlabel = \"Highlights\"",
            ),
            (
                "page = \"platform.contacts.person-detail\"\nnode = \"person-highlights\"",
                "page = \"platform.contacts.unknown\"\nnode = \"person-highlights\"",
            ),
            ("label = \"Highlights\"", "label = \"\""),
        ] {
            let changed = fixture.replacen(from, to, 1);
            assert_ne!(changed, fixture, "{from}");
            assert!(
                matches!(
                    Manifest::parse_toml(&base_manifest(&changed)),
                    Err(ManifestError::InvalidFieldZone { .. })
                ),
                "{from}"
            );
        }
    }

    #[test]
    fn storage_backends_default_to_postgres_and_reject_duplicates() {
        let legacy = Manifest::parse_toml(&base_manifest("")).expect("legacy manifest");
        assert!(legacy.supports_storage(super::StorageBackend::Postgres));
        assert!(!legacy.supports_storage(super::StorageBackend::Sqlite));
        let both = Manifest::parse_toml(&base_manifest(
            "[storage]\nbackends = [\"postgres\", \"sqlite\"]\n",
        ))
        .expect("declared backends");
        assert!(both.supports_storage(super::StorageBackend::Sqlite));
        assert!(matches!(
            Manifest::parse_toml(&base_manifest(
                "[storage]\nbackends = [\"sqlite\", \"sqlite\"]\n"
            )),
            Err(ManifestError::InvalidStorage(_))
        ));
        assert!(
            Manifest::parse_toml(&base_manifest("[storage]\nbackends = [\"oracle\"]\n")).is_err()
        );
    }

    #[test]
    fn ui_metadata_files_are_relative_json_and_unique() {
        Manifest::parse_toml(&base_manifest(
            r#"
[[ui.resources]]
id = "platform.contacts.person"
path = "ui/resources/person.json"

[[ui.pages]]
id = "platform.contacts.person-form"
path = "ui/pages/person-form.json"
"#,
        ))
        .expect("bounded file references are accepted");

        let error = Manifest::parse_toml(&base_manifest(
            r#"
[[ui.pages]]
id = "platform.contacts.person-form"
path = "../outside.json"
"#,
        ))
        .expect_err("metadata cannot escape the plugin artifact");
        assert!(matches!(error, ManifestError::InvalidUiReference { .. }));
    }

    #[test]
    fn the_tracker_fixture_declares_two_objects() {
        let manifest = Manifest::parse_toml(include_str!(
            "../../../fixtures/plugins/tracker/plugin.toml"
        ))
        .expect("fixture parses");
        assert_eq!(manifest.objects.len(), 2);
        assert_eq!(manifest.objects[1].fields.len(), 3);
        assert_eq!(
            manifest.objects[1].fields[2].reference_object.as_deref(),
            Some("example.tracker:milestone")
        );
        let round_trip: Manifest =
            serde_json::from_value(serde_json::to_value(&manifest).expect("json"))
                .expect("stored manifests read back");
        assert_eq!(round_trip.objects, manifest.objects);
    }

    fn objects_manifest(id: &str, objects: &str) -> Result<Manifest, ManifestError> {
        Manifest::parse_toml(&base_manifest(objects).replace("platform.contacts", id))
    }

    fn object_reason(result: Result<Manifest, ManifestError>) -> &'static str {
        match result {
            Err(ManifestError::InvalidObject { reason, .. })
            | Err(ManifestError::InvalidObjectField { reason, .. }) => reason,
            other => panic!("expected an object error, got {other:?}"),
        }
    }

    const TASK: &str = r#"
[[objects]]
name = "task"
label = "Task"
plural_label = "Tasks"
key_prefix = "tsk"
"#;

    #[test]
    fn declared_objects_are_validated_when_the_manifest_is_read() {
        objects_manifest("example.work", TASK).expect("a minimal object");
        assert!(object_reason(objects_manifest("example.my-work", TASK)).contains("namespace"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &TASK.replace("tsk", "1sk")
        ))
        .contains("key_prefix"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &TASK.replace("tsk", "Tsk")
        ))
        .contains("key_prefix"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &TASK.replace("\"task\"", "\"Task\"")
        ))
        .contains("name"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &format!("{TASK}{}", TASK.replace("tsk", "tsl"))
        ))
        .contains("duplicate object name"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &format!("{TASK}{}", TASK.replace("\"task\"", "\"chore\""))
        ))
        .contains("duplicate key prefix"));
        let many: String = (0..21)
            .map(|index| {
                TASK.replace("\"task\"", &format!("\"task{index}\""))
                    .replace("tsk", &format!("t{index:02}"))
            })
            .collect();
        assert!(object_reason(objects_manifest("example.work", &many)).contains("at most 20"));
        // Unknown keys are refused rather than ignored.
        assert!(matches!(
            objects_manifest("example.work", &format!("{TASK}formula = \"x\"\n")),
            Err(ManifestError::Parse(_))
        ));
    }

    #[test]
    fn declared_fields_use_engine_types_and_known_targets() {
        let with_field = |field: &str| format!("{TASK}\n[[objects.fields]]\n{field}\n");
        objects_manifest(
            "example.work",
            &with_field(
                r#"name = "parent"
label = "Parent"
field_type = "lookup"
reference_object = "example.work:task"
delete_behaviour = "restrict""#,
            ),
        )
        .expect("a lookup to its own object");
        assert!(matches!(
            objects_manifest(
                "example.work",
                &with_field("name = \"total\"\nlabel = \"Total\"\nfield_type = \"formula\"")
            ),
            Err(ManifestError::Parse(_))
        ));
        assert!(object_reason(objects_manifest(
            "example.work",
            &with_field(
                "name = \"owner\"\nlabel = \"Owner\"\nfield_type = \"lookup\"\nreference_object = \"custom:project\""
            )
        ))
        .contains("reference_object"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &with_field(
                "name = \"person\"\nlabel = \"Person\"\nfield_type = \"lookup\"\nreference_object = \"example.people:person\""
            )
        ))
        .contains("depends on"));
        let depending = format!(
            "[dependencies.\"example.people\"]\nversion = \">=0.1.0\"\nreasons = [\"composition\"]\n{}",
            with_field(
                "name = \"about\"\nlabel = \"About\"\nfield_type = \"related_to\"\nreference_objects = [\"example.people:person\", \"example.work:task\"]"
            )
        );
        objects_manifest("example.work", &depending).expect("a dependency's object");
        assert!(object_reason(objects_manifest(
            "example.work",
            &with_field("name = \"about\"\nlabel = \"About\"\nfield_type = \"related_to\"")
        ))
        .contains("1 to 10"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &with_field("name = \"stage\"\nlabel = \"Stage\"\nfield_type = \"picklist\"")
        ))
        .contains("options"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &with_field(
                "name = \"done\"\nlabel = \"Done\"\nfield_type = \"boolean\"\nunique = true"
            )
        ))
        .contains("unique"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &with_field(
                "name = \"note\"\nlabel = \"Note\"\nfield_type = \"text\"\nmax_length = 300"
            )
        ))
        .contains("max_length"));
        assert!(object_reason(objects_manifest(
            "example.work",
            &format!(
                "{}[[objects.fields]]\nname = \"note\"\nlabel = \"Again\"\nfield_type = \"text\"\n",
                with_field("name = \"note\"\nlabel = \"Note\"\nfield_type = \"text\"")
            )
        ))
        .contains("duplicate field name"));
    }
}
