#![forbid(unsafe_code)]

pub mod activation;

use plugin_contract::{Manifest, PluginDistribution};
use regex::Regex;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};
use thiserror::Error;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LifecycleState {
    Discovered,
    Validated,
    Staged,
    Migrated,
    Registered,
    HealthChecked,
    Enabled,
    Disabled,
    Failed,
}

impl LifecycleState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Validated => "validated",
            Self::Staged => "staged",
            Self::Migrated => "migrated",
            Self::Registered => "registered",
            Self::HealthChecked => "health_checked",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Failed => "failed",
        }
    }

    pub fn transition(self, next: Self) -> Result<Self, RegistryError> {
        let allowed = matches!(
            (self, next),
            (Self::Discovered, Self::Validated)
                | (Self::Validated, Self::Staged)
                | (Self::Staged, Self::Migrated)
                | (Self::Migrated, Self::Registered)
                | (Self::Registered, Self::HealthChecked)
                | (Self::HealthChecked, Self::Enabled)
                | (Self::Enabled, Self::Disabled)
                | (Self::Disabled, Self::Enabled)
                | (Self::Failed, Self::Discovered)
        ) || (next == Self::Failed && self != Self::Enabled);
        allowed
            .then_some(next)
            .ok_or(RegistryError::InvalidTransition {
                from: self,
                to: next,
            })
    }
}

#[derive(Debug, Clone)]
pub struct RegistryPolicy {
    pub reserved_namespaces: Vec<String>,
    pub trusted_core_repositories: Vec<String>,
    pub trusted_provenance_hosts: Vec<String>,
    /// The deployment's storage backend; plugins must ship an adapter for it.
    pub storage_backend: plugin_contract::StorageBackend,
}

#[derive(Debug, Clone)]
pub struct VerifiedPlugin {
    pub manifest: Manifest,
    pub manifest_path: PathBuf,
    pub artifact_path: PathBuf,
    pub verified_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveryFailure {
    pub manifest_path: PathBuf,
    pub plugin_id: Option<String>,
    pub version: Option<String>,
    pub control: &'static str,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct DiscoveryReport {
    pub accepted: Vec<VerifiedPlugin>,
    pub failures: Vec<DiscoveryFailure>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RegistryError {
    #[error("duplicate plugin {0}")]
    Duplicate(String),
    #[error("missing dependency {plugin} requires {dependency}")]
    MissingDependency { plugin: String, dependency: String },
    #[error("dependency {dependency} version {actual} does not satisfy {plugin} requirement {requirement}")]
    IncompatibleDependency {
        plugin: String,
        dependency: String,
        actual: Version,
        requirement: String,
    },
    #[error("dependency cycle involving {0}")]
    Cycle(String),
    #[error("invalid lifecycle transition from {from:?} to {to:?}")]
    InvalidTransition {
        from: LifecycleState,
        to: LifecycleState,
    },
}

pub fn discover(manifest_paths: &[PathBuf], policy: &RegistryPolicy) -> DiscoveryReport {
    let mut report = DiscoveryReport::default();
    for path in manifest_paths {
        match discover_one(path, policy) {
            Ok(plugin) => report.accepted.push(plugin),
            Err(failure) => report.failures.push(failure),
        }
    }

    if report.accepted.is_empty() {
        return report;
    }
    match dependency_order(
        &report
            .accepted
            .iter()
            .map(|plugin| plugin.manifest.clone())
            .collect::<Vec<_>>(),
    ) {
        Ok(order) => {
            let positions: BTreeMap<_, _> = order
                .into_iter()
                .enumerate()
                .map(|(position, id)| (id, position))
                .collect();
            report
                .accepted
                .sort_by_key(|plugin| positions[&plugin.manifest.id]);
        }
        Err(error) => {
            report.failures.push(DiscoveryFailure {
                manifest_path: PathBuf::from("<dependency-graph>"),
                plugin_id: None,
                version: None,
                control: "plugin.dependency_graph",
                detail: error.to_string(),
            });
            report.accepted.clear();
        }
    }
    report
}

fn discover_one(path: &Path, policy: &RegistryPolicy) -> Result<VerifiedPlugin, DiscoveryFailure> {
    let contents = fs::read_to_string(path).map_err(|error| {
        failure(
            path,
            None,
            "plugin.manifest_read",
            format!("manifest cannot be read: {}", error.kind()),
        )
    })?;
    let manifest = Manifest::parse_toml(&contents)
        .map_err(|error| failure(path, None, "plugin.manifest", error.to_string()))?;
    let identity = Some((&manifest.id, manifest.version.to_string()));
    validate_policy(&manifest, policy)
        .map_err(|(control, detail)| failure(path, identity.clone(), control, detail))?;
    let artifact_path = resolve_artifact(path, &manifest)
        .map_err(|(control, detail)| failure(path, identity.clone(), control, detail))?;
    let verified_digest = verify_digest(&artifact_path, &manifest)
        .map_err(|(control, detail)| failure(path, identity, control, detail))?;
    Ok(VerifiedPlugin {
        manifest,
        manifest_path: path.to_owned(),
        artifact_path,
        verified_digest,
    })
}

fn validate_policy(
    manifest: &Manifest,
    policy: &RegistryPolicy,
) -> Result<(), (&'static str, String)> {
    if !manifest.supports_storage(policy.storage_backend) {
        return Err((
            "plugin.storage_backend",
            format!(
                "plugin {} does not support the {} storage backend",
                manifest.id,
                policy.storage_backend.as_str()
            ),
        ));
    }
    let reserved = policy.reserved_namespaces.iter().any(|namespace| {
        manifest.id == *namespace || manifest.id.starts_with(&format!("{namespace}."))
    });
    if reserved && manifest.supply_chain.distribution != Some(PluginDistribution::CoreOob) {
        return Err((
            "plugin.reserved_namespace",
            format!(
                "reserved plugin id {} requires core-oob distribution",
                manifest.id
            ),
        ));
    }
    if manifest.supply_chain.distribution != Some(PluginDistribution::CoreOob) {
        return Ok(());
    }

    let repository = manifest.supply_chain.source_repository.as_deref().ok_or((
        "plugin.provenance_repository",
        "source repository is required".into(),
    ))?;
    let repository_url = Url::parse(repository).map_err(|_| {
        (
            "plugin.provenance_repository",
            "source repository URL is malformed".into(),
        )
    })?;
    if repository_url.scheme() != "https"
        || !policy
            .trusted_core_repositories
            .iter()
            .any(|allowed| allowed.trim_end_matches('/') == repository.trim_end_matches('/'))
    {
        return Err((
            "plugin.provenance_repository",
            "source repository is not trusted by core policy".into(),
        ));
    }

    let revision = manifest
        .supply_chain
        .source_revision
        .as_deref()
        .unwrap_or_default();
    let revision_pattern = Regex::new(r"^[0-9a-f]{40}$").expect("valid revision regex");
    if !revision_pattern.is_match(revision) || revision.chars().all(|character| character == '0') {
        return Err((
            "plugin.provenance_revision",
            "source revision must be a non-zero lowercase 40-character commit id".into(),
        ));
    }

    let provenance = manifest
        .supply_chain
        .build_provenance
        .as_deref()
        .unwrap_or_default();
    let provenance_url = Url::parse(provenance).map_err(|_| {
        (
            "plugin.provenance_reference",
            "build provenance URL is malformed".into(),
        )
    })?;
    if provenance_url.scheme() != "https"
        || !provenance_url.host_str().is_some_and(|host| {
            policy
                .trusted_provenance_hosts
                .iter()
                .any(|allowed| allowed == host)
        })
        || !provenance_url.path().ends_with(".intoto.jsonl")
        || !provenance_url.path().contains(&manifest.id)
    {
        return Err((
            "plugin.provenance_reference",
            "build provenance reference violates host or path policy".into(),
        ));
    }
    Ok(())
}

fn resolve_artifact(
    manifest_path: &Path,
    manifest: &Manifest,
) -> Result<PathBuf, (&'static str, String)> {
    let relative = manifest
        .supply_chain
        .artifact_path
        .as_deref()
        .unwrap_or_default();
    let relative_path = Path::new(relative);
    if relative.is_empty()
        || relative_path.is_absolute()
        || relative.contains("..")
        || relative.contains('\\')
    {
        return Err((
            "plugin.artifact_path",
            "artifact path must be relative and cannot traverse directories".into(),
        ));
    }
    let root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    let canonical_root = root.canonicalize().map_err(|error| {
        (
            "plugin.artifact_path",
            format!("plugin root cannot be resolved: {}", error.kind()),
        )
    })?;
    let candidate = root.join(relative_path).canonicalize().map_err(|error| {
        (
            "plugin.artifact_path",
            format!("artifact cannot be read: {}", error.kind()),
        )
    })?;
    if !candidate.starts_with(canonical_root) || !candidate.is_file() {
        return Err((
            "plugin.artifact_path",
            "artifact must be a file within the plugin package".into(),
        ));
    }
    Ok(candidate)
}

fn verify_digest(
    artifact_path: &Path,
    manifest: &Manifest,
) -> Result<String, (&'static str, String)> {
    let declared = manifest
        .supply_chain
        .artifact_digest
        .as_deref()
        .unwrap_or_default();
    let Some(expected) = declared.strip_prefix("sha256:") else {
        return Err((
            "plugin.artifact_digest_algorithm",
            "only sha256 artifact digests are supported".into(),
        ));
    };
    let digest_pattern = Regex::new(r"^[0-9a-f]{64}$").expect("valid digest regex");
    if !digest_pattern.is_match(expected) {
        return Err((
            "plugin.artifact_digest_format",
            "artifact digest must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    let bytes = fs::read(artifact_path).map_err(|error| {
        (
            "plugin.artifact_read",
            format!("artifact cannot be read: {}", error.kind()),
        )
    })?;
    let actual = format!("{:x}", Sha256::digest(bytes));
    if actual != expected {
        return Err((
            "plugin.artifact_digest_mismatch",
            "artifact digest does not match the registered package".into(),
        ));
    }
    Ok(format!("sha256:{actual}"))
}

fn failure(
    path: &Path,
    identity: Option<(&String, String)>,
    control: &'static str,
    detail: String,
) -> DiscoveryFailure {
    DiscoveryFailure {
        manifest_path: path.to_owned(),
        plugin_id: identity.as_ref().map(|(id, _)| (*id).clone()),
        version: identity.map(|(_, version)| version),
        control,
        detail,
    }
}

pub fn dependency_order(manifests: &[Manifest]) -> Result<Vec<String>, RegistryError> {
    let mut map = BTreeMap::new();
    for manifest in manifests {
        if map.insert(manifest.id.clone(), manifest).is_some() {
            return Err(RegistryError::Duplicate(manifest.id.clone()));
        }
    }
    let mut temp = BTreeSet::new();
    let mut perm = BTreeSet::new();
    let mut out = Vec::new();
    fn visit(
        id: &str,
        map: &BTreeMap<String, &Manifest>,
        temp: &mut BTreeSet<String>,
        perm: &mut BTreeSet<String>,
        out: &mut Vec<String>,
    ) -> Result<(), RegistryError> {
        if perm.contains(id) {
            return Ok(());
        }
        if !temp.insert(id.to_string()) {
            return Err(RegistryError::Cycle(id.to_string()));
        }
        let manifest = map
            .get(id)
            .ok_or_else(|| RegistryError::MissingDependency {
                plugin: id.into(),
                dependency: id.into(),
            })?;
        for (dependency_id, dependency) in &manifest.dependencies {
            let actual =
                map.get(dependency_id)
                    .ok_or_else(|| RegistryError::MissingDependency {
                        plugin: id.into(),
                        dependency: dependency_id.clone(),
                    })?;
            if !dependency.version.matches(&actual.version) {
                return Err(RegistryError::IncompatibleDependency {
                    plugin: id.into(),
                    dependency: dependency_id.clone(),
                    actual: actual.version.clone(),
                    requirement: dependency.version.to_string(),
                });
            }
            visit(dependency_id, map, temp, perm, out)?;
        }
        temp.remove(id);
        perm.insert(id.to_string());
        out.push(id.to_string());
        Ok(())
    }
    for id in map.keys() {
        visit(id, &map, &mut temp, &mut perm, &mut out)?;
    }
    Ok(out)
}

pub fn platform_version() -> Version {
    Version::new(0, 1, 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contract::{DependencyDecl, DependencyReason};
    use semver::VersionReq;
    use std::sync::atomic::{AtomicU64, Ordering};

    static FIXTURE: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "limiara-plugin-registry-{}-{}",
            std::process::id(),
            FIXTURE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("artifact")).expect("create fixture");
        root
    }

    fn manifest(id: &str, digest: &str, distribution: &str) -> String {
        format!(
            r#"manifest_version = 1
id = "{id}"
name = "Fixture"
version = "0.1.0"
description = "fixture"
runtime = "native"
[compatibility]
platform = ">=0.1.0,<0.2.0"
[supply_chain]
distribution = "{distribution}"
source_repository = "https://github.com/buchochelliq-labs/limiara-core"
source_revision = "b02ea1b4ea400ca69ea7d3f70a2c06a15e553afb"
build_provenance = "https://github.com/buchochelliq-labs/limiara-core/attestations/{id}.intoto.jsonl"
artifact_digest = "sha256:{digest}"
artifact_path = "artifact/native.link"
signature = "https://example.invalid/plugin.sig"
sbom = "https://example.invalid/plugin.spdx.json"
"#
        )
    }

    fn policy() -> RegistryPolicy {
        RegistryPolicy {
            reserved_namespaces: vec!["platform".into()],
            trusted_core_repositories: vec![
                "https://github.com/buchochelliq-labs/limiara-core".into()
            ],
            trusted_provenance_hosts: vec!["github.com".into()],
            storage_backend: plugin_contract::StorageBackend::Postgres,
        }
    }

    fn graph_manifest(id: &str, version: &str) -> Manifest {
        Manifest::parse_toml(&format!(
            r#"manifest_version = 1
id = "{id}"
name = "Fixture"
version = "{version}"
description = "fixture"
runtime = "native"
[compatibility]
platform = ">=0.1.0,<0.2.0"
"#
        ))
        .expect("valid graph fixture")
    }

    fn depends_on(manifest: &mut Manifest, id: &str, version: &str) {
        manifest.dependencies.insert(
            id.into(),
            DependencyDecl {
                version: VersionReq::parse(version).expect("valid requirement"),
                reasons: vec![DependencyReason::Composition],
                description: None,
            },
        );
    }

    #[test]
    fn matching_digest_is_accepted_and_tampering_is_rejected_then_retry_succeeds() {
        let root = fixture();
        let artifact = b"linked-native:fixture@0.1.0\n";
        fs::write(root.join("artifact/native.link"), artifact).expect("write artifact");
        let digest = format!("{:x}", Sha256::digest(artifact));
        fs::write(
            root.join("plugin.toml"),
            manifest("platform.fixture", &digest, "core-oob"),
        )
        .expect("write manifest");
        let accepted = discover(&[root.join("plugin.toml")], &policy());
        assert_eq!(accepted.accepted.len(), 1);
        assert_eq!(
            accepted.accepted[0].verified_digest,
            format!("sha256:{digest}")
        );

        fs::write(root.join("artifact/native.link"), b"tampered").expect("tamper");
        let rejected = discover(&[root.join("plugin.toml")], &policy());
        assert_eq!(
            rejected.failures[0].control,
            "plugin.artifact_digest_mismatch"
        );
        fs::write(root.join("artifact/native.link"), artifact).expect("restore");
        assert_eq!(
            discover(&[root.join("plugin.toml")], &policy())
                .accepted
                .len(),
            1
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn plugins_without_an_adapter_for_the_storage_backend_are_rejected() {
        let root = fixture();
        let artifact = b"linked-native:fixture@0.1.0\n";
        fs::write(root.join("artifact/native.link"), artifact).expect("write artifact");
        let digest = format!("{:x}", Sha256::digest(artifact));
        fs::write(
            root.join("plugin.toml"),
            manifest("platform.fixture", &digest, "core-oob"),
        )
        .expect("write manifest");
        let sqlite = RegistryPolicy {
            storage_backend: plugin_contract::StorageBackend::Sqlite,
            ..policy()
        };
        let report = discover(&[root.join("plugin.toml")], &sqlite);
        assert!(report.accepted.is_empty());
        assert_eq!(report.failures[0].control, "plugin.storage_backend");
        assert_eq!(
            discover(&[root.join("plugin.toml")], &policy())
                .accepted
                .len(),
            1
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn reserved_identity_and_malformed_or_unsupported_digests_are_rejected() {
        let root = fixture();
        fs::write(root.join("artifact/native.link"), b"fixture").expect("artifact");
        fs::write(
            root.join("plugin.toml"),
            manifest("platform.impostor", &"0".repeat(64), "third-party"),
        )
        .expect("manifest");
        let reserved = discover(&[root.join("plugin.toml")], &policy());
        assert_eq!(reserved.failures[0].control, "plugin.reserved_namespace");

        let malformed = manifest("vendor.fixture", "nope", "third-party");
        fs::write(root.join("plugin.toml"), malformed).expect("manifest");
        assert_eq!(
            discover(&[root.join("plugin.toml")], &policy()).failures[0].control,
            "plugin.artifact_digest_format"
        );
        fs::write(
            root.join("plugin.toml"),
            manifest("vendor.fixture", &"0".repeat(64), "third-party")
                .replace("sha256:", "sha512:"),
        )
        .expect("manifest");
        assert_eq!(
            discover(&[root.join("plugin.toml")], &policy()).failures[0].control,
            "plugin.artifact_digest_algorithm"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn lifecycle_rejects_invalid_transitions_and_allows_retry() {
        assert_eq!(
            LifecycleState::Discovered.transition(LifecycleState::Validated),
            Ok(LifecycleState::Validated)
        );
        assert!(LifecycleState::Discovered
            .transition(LifecycleState::Enabled)
            .is_err());
        assert_eq!(
            LifecycleState::Failed.transition(LifecycleState::Discovered),
            Ok(LifecycleState::Discovered)
        );
        assert!(LifecycleState::Enabled
            .transition(LifecycleState::Failed)
            .is_err());
    }

    #[test]
    fn dependency_graph_rejects_duplicate_missing_incompatible_and_cyclic_plugins() {
        let first = graph_manifest("vendor.first", "1.0.0");
        assert!(matches!(
            dependency_order(&[first.clone(), first.clone()]),
            Err(RegistryError::Duplicate(id)) if id == "vendor.first"
        ));

        let mut missing = graph_manifest("vendor.consumer", "1.0.0");
        depends_on(&mut missing, "vendor.missing", "^1");
        assert!(matches!(
            dependency_order(&[missing]),
            Err(RegistryError::MissingDependency { dependency, .. }) if dependency == "vendor.missing"
        ));

        let actual = graph_manifest("vendor.provider", "2.0.0");
        let mut incompatible = graph_manifest("vendor.consumer", "1.0.0");
        depends_on(&mut incompatible, "vendor.provider", "^1");
        assert!(matches!(
            dependency_order(&[actual, incompatible]),
            Err(RegistryError::IncompatibleDependency { dependency, .. }) if dependency == "vendor.provider"
        ));

        let mut left = graph_manifest("vendor.left", "1.0.0");
        let mut right = graph_manifest("vendor.right", "1.0.0");
        depends_on(&mut left, "vendor.right", "^1");
        depends_on(&mut right, "vendor.left", "^1");
        assert!(matches!(
            dependency_order(&[left, right]),
            Err(RegistryError::Cycle(_))
        ));
    }
}
