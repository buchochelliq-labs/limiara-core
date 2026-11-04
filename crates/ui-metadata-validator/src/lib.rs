#![forbid(unsafe_code)]

use plugin_contract::ui::{
    ComponentDefinition, ExtensionContribution, FragmentDefinition, PageDefinition,
    ResourceDescriptor, UiBundle, UiContractError,
};
use plugin_contract::{Manifest, ManifestError, UiFileReference};
use serde_json::Error as JsonError;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationSummary {
    pub manifests: usize,
    pub resources: usize,
    pub pages: usize,
    pub fragments: usize,
    pub extensions: usize,
    pub components: usize,
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("cannot read manifest {path}: {source}")]
    ReadManifest {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("manifest {path} is invalid: {source}")]
    Manifest {
        path: PathBuf,
        source: Box<ManifestError>,
    },
    #[error("cannot read {kind} {id} from {path}: {source}")]
    ReadDocument {
        kind: &'static str,
        id: String,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{kind} {id} in {path} is not valid JSON: {source}")]
    Json {
        kind: &'static str,
        id: String,
        path: PathBuf,
        source: JsonError,
    },
    #[error("{kind} reference {expected} in {path} loaded document {actual}")]
    ReferenceMismatch {
        kind: &'static str,
        expected: String,
        actual: String,
        path: PathBuf,
    },
    #[error("{kind} {id} in {path} declares owner {actual}; expected manifest owner {expected}")]
    OwnerMismatch {
        kind: &'static str,
        id: String,
        expected: String,
        actual: String,
        path: PathBuf,
    },
    #[error("combined UI metadata contract validation failed: {0}")]
    Contract(#[from] UiContractError),
    #[error("at least one plugin manifest path is required")]
    MissingManifest,
}

pub fn validate_manifests(paths: &[PathBuf]) -> Result<ValidationSummary, ValidationError> {
    if paths.is_empty() {
        return Err(ValidationError::MissingManifest);
    }
    let mut bundle = UiBundle::default();
    for path in paths {
        load_manifest(path, &mut bundle)?;
    }
    bundle.validate()?;
    Ok(ValidationSummary {
        manifests: paths.len(),
        resources: bundle.resources.len(),
        pages: bundle.pages.len(),
        fragments: bundle.fragments.len(),
        extensions: bundle.extensions.len(),
        components: bundle.components.len(),
    })
}

fn load_manifest(path: &Path, bundle: &mut UiBundle) -> Result<(), ValidationError> {
    let encoded = fs::read_to_string(path).map_err(|source| ValidationError::ReadManifest {
        path: path.to_path_buf(),
        source,
    })?;
    let manifest = Manifest::parse_toml(&encoded).map_err(|source| ValidationError::Manifest {
        path: path.to_path_buf(),
        source: Box::new(source),
    })?;
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    load_documents(
        root,
        "resource",
        &manifest.ui.resources,
        &mut bundle.resources,
        |document: &ResourceDescriptor| &document.id,
        |document: &ResourceDescriptor| &document.owner_plugin,
        &manifest.id,
    )?;
    load_documents(
        root,
        "page",
        &manifest.ui.pages,
        &mut bundle.pages,
        |document: &PageDefinition| &document.id,
        |document: &PageDefinition| &document.owner_plugin,
        &manifest.id,
    )?;
    load_documents(
        root,
        "fragment",
        &manifest.ui.fragments,
        &mut bundle.fragments,
        |document: &FragmentDefinition| &document.id,
        |document: &FragmentDefinition| &document.owner_plugin,
        &manifest.id,
    )?;
    load_documents(
        root,
        "extension",
        &manifest.ui.extensions,
        &mut bundle.extensions,
        |document: &ExtensionContribution| &document.id,
        |document: &ExtensionContribution| &document.owner_plugin,
        &manifest.id,
    )?;
    load_documents(
        root,
        "component",
        &manifest.ui.components,
        &mut bundle.components,
        |document: &ComponentDefinition| &document.id,
        |document: &ComponentDefinition| &document.owner_plugin,
        &manifest.id,
    )?;
    Ok(())
}

fn load_documents<T, F, O>(
    root: &Path,
    kind: &'static str,
    references: &[UiFileReference],
    target: &mut Vec<T>,
    id: F,
    owner: O,
    expected_owner: &str,
) -> Result<(), ValidationError>
where
    T: serde::de::DeserializeOwned,
    F: Fn(&T) -> &String,
    O: Fn(&T) -> &String,
{
    for reference in references {
        let path = root.join(&reference.path);
        let encoded =
            fs::read_to_string(&path).map_err(|source| ValidationError::ReadDocument {
                kind,
                id: reference.id.clone(),
                path: path.clone(),
                source,
            })?;
        let document: T =
            serde_json::from_str(&encoded).map_err(|source| ValidationError::Json {
                kind,
                id: reference.id.clone(),
                path: path.clone(),
                source,
            })?;
        if id(&document) != &reference.id {
            return Err(ValidationError::ReferenceMismatch {
                kind,
                expected: reference.id.clone(),
                actual: id(&document).clone(),
                path,
            });
        }
        if owner(&document) != expected_owner {
            return Err(ValidationError::OwnerMismatch {
                kind,
                id: reference.id.clone(),
                expected: expected_owner.into(),
                actual: owner(&document).clone(),
                path,
            });
        }
        target.push(document);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn representative_plugin_fixture_is_valid() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/plugins/ui-conformance/plugin.toml");
        let summary = validate_manifests(&[manifest]).expect("fixture should validate");
        assert_eq!(
            summary,
            ValidationSummary {
                manifests: 1,
                resources: 1,
                pages: 1,
                fragments: 0,
                extensions: 0,
                components: 0,
            }
        );
    }

    #[test]
    fn rejects_documents_claiming_another_plugin_owner() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should follow epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("limiara-ui-validator-{unique}"));
        fs::create_dir_all(&root).expect("create test directory");
        fs::write(
            root.join("plugin.toml"),
            r#"manifest_version = 1
id = "example.owner"
name = "Owner test"
version = "0.1.0"
description = "Owner validation test"
runtime = "native"
[compatibility]
platform = ">=0.1.0,<0.2.0"
[[ui.resources]]
id = "example.owner.record"
path = "record.json"
"#,
        )
        .expect("write manifest");
        fs::write(
            root.join("record.json"),
            r#"{"contract_version":1,"id":"example.owner.record","definition_version":1,"owner_plugin":"example.attacker","label":"Record"}"#,
        )
        .expect("write resource");
        let error = validate_manifests(&[root.join("plugin.toml")])
            .expect_err("owner impersonation must fail");
        assert!(matches!(
            error,
            ValidationError::OwnerMismatch {
                expected,
                actual,
                ..
            } if expected == "example.owner" && actual == "example.attacker"
        ));
        fs::remove_dir_all(root).expect("remove test directory");
    }
}
