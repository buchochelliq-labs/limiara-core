//! The single tenant activation rule shared by every runtime surface.
//!
//! API routes, UI contributions, event consumers, and scheduled jobs must all
//! agree on whether a tenant may use a plugin. Keeping the rule here prevents
//! the worker and the API from drifting into different interpretations of the
//! same persisted lifecycle state.

use plugin_contract::Manifest;

/// The persisted installation and registry inputs for one tenant plugin.
///
/// Every field is optional because the projection is a left join: missing
/// inputs are treated as inactive rather than as an error.
#[derive(Debug, Clone, Copy)]
pub struct ActivationInputs<'a> {
    pub plugin_id: &'a str,
    pub installation_state: Option<&'a str>,
    pub active_version: Option<&'a str>,
    pub manifest: Option<&'a serde_json::Value>,
    pub lifecycle_state: Option<&'a str>,
    pub verified_digest: Option<&'a str>,
}

/// Resolve the manifest of an active tenant installation, or `None`.
///
/// An installation is active only when it is `enabled`, its exact installed
/// version is still a `health_checked` registry candidate, the stored manifest
/// parses, and the manifest identity, version, and declared artifact digest
/// match the verified registry record. Anything else fails closed.
pub fn resolve_active_manifest(inputs: ActivationInputs<'_>) -> Option<Manifest> {
    if inputs.installation_state != Some("enabled")
        || inputs.lifecycle_state != Some("health_checked")
    {
        return None;
    }
    let verified_digest = inputs
        .verified_digest
        .filter(|digest| is_verified_digest(digest))?;
    let version = inputs.active_version?;
    let manifest = serde_json::from_value::<Manifest>(inputs.manifest?.clone()).ok()?;
    (manifest.id == inputs.plugin_id
        && manifest.version.to_string() == version
        && manifest.supply_chain.artifact_digest.as_deref() == Some(verified_digest))
    .then_some(manifest)
}

/// Whether a value is a canonical lowercase `sha256:` digest.
pub fn is_verified_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIGEST: &str = "sha256:0000000000000000000000000000000000000000000000000000000000000000";

    fn manifest() -> serde_json::Value {
        let manifest = Manifest::parse_toml(&format!(
            r#"
manifest_version = 1
id = "platform.example"
name = "Example"
version = "0.1.0"
description = "Example"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[supply_chain]
artifact_digest = "{DIGEST}"
"#
        ))
        .unwrap();
        serde_json::to_value(manifest).unwrap()
    }

    fn inputs(manifest: &serde_json::Value) -> ActivationInputs<'_> {
        ActivationInputs {
            plugin_id: "platform.example",
            installation_state: Some("enabled"),
            active_version: Some("0.1.0"),
            manifest: Some(manifest),
            lifecycle_state: Some("health_checked"),
            verified_digest: Some(DIGEST),
        }
    }

    #[test]
    fn enabled_verified_installation_is_active() {
        let manifest = manifest();
        assert!(resolve_active_manifest(inputs(&manifest)).is_some());
    }

    #[test]
    fn every_missing_or_mismatched_input_fails_closed() {
        let manifest = manifest();
        let other_digest = format!("sha256:{}", "1".repeat(64));
        let cases = [
            ActivationInputs {
                installation_state: Some("disabled"),
                ..inputs(&manifest)
            },
            ActivationInputs {
                installation_state: Some("disabling"),
                ..inputs(&manifest)
            },
            ActivationInputs {
                installation_state: None,
                ..inputs(&manifest)
            },
            ActivationInputs {
                lifecycle_state: Some("failed"),
                ..inputs(&manifest)
            },
            ActivationInputs {
                active_version: Some("0.2.0"),
                ..inputs(&manifest)
            },
            ActivationInputs {
                verified_digest: Some(&other_digest),
                ..inputs(&manifest)
            },
            ActivationInputs {
                verified_digest: Some("sha256:ABC"),
                ..inputs(&manifest)
            },
            ActivationInputs {
                plugin_id: "platform.other",
                ..inputs(&manifest)
            },
            ActivationInputs {
                manifest: None,
                ..inputs(&manifest)
            },
        ];
        for case in cases {
            assert!(resolve_active_manifest(case).is_none(), "{case:?}");
        }
    }
}
