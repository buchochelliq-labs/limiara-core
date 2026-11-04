//! UI metadata that a trusted native plugin compiles into the host.
//!
//! A native plugin's artifact digest covers its linked code, not loose files,
//! so its UI documents are embedded at compile time rather than read from the
//! plugin directory at runtime. [`verify_ui_plugin`] then requires the linked
//! documents to match the manifest's declarations exactly.

use plugin_contract::ui::{
    ComponentDefinition, ExtensionContribution, FragmentDefinition, PageDefinition,
    ResourceDescriptor, UiBundle, UiContractError,
};
use plugin_contract::{Manifest, UiFileReference};
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use thiserror::Error;

/// One embedded UI document and the manifest identifier it implements.
#[derive(Debug, Clone, Copy)]
pub struct NativeUiDocument {
    pub id: &'static str,
    pub json: &'static str,
}

/// Every UI document a native plugin links, grouped as the manifest declares.
#[derive(Debug, Clone, Copy, Default)]
pub struct NativeUiDocuments {
    pub resources: &'static [NativeUiDocument],
    pub pages: &'static [NativeUiDocument],
    pub fragments: &'static [NativeUiDocument],
    pub extensions: &'static [NativeUiDocument],
    pub components: &'static [NativeUiDocument],
}

/// A native plugin that contributes UI metadata to the host.
pub trait NativeUiPlugin: Send + Sync {
    fn manifest(&self) -> &Manifest;
    fn ui_documents(&self) -> NativeUiDocuments;
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum UiPackageError {
    #[error("plugin {plugin_id} {kind} {id} is not valid JSON: {message}")]
    Json {
        plugin_id: String,
        kind: &'static str,
        id: String,
        message: String,
    },
    #[error("plugin {plugin_id} {kind} document linked as {linked} declares id {actual}")]
    IdMismatch {
        plugin_id: String,
        kind: &'static str,
        linked: String,
        actual: String,
    },
    #[error("plugin {plugin_id} {kind} {id} declares owner {owner}")]
    Owner {
        plugin_id: String,
        kind: &'static str,
        id: String,
        owner: String,
    },
    #[error("plugin {plugin_id} links undeclared {kind} {id}")]
    Undeclared {
        plugin_id: String,
        kind: &'static str,
        id: String,
    },
    #[error("plugin {plugin_id} links {kind} {id} more than once")]
    Duplicate {
        plugin_id: String,
        kind: &'static str,
        id: String,
    },
    #[error("plugin {plugin_id} declares {kind} {id} but links no document for it")]
    Unimplemented {
        plugin_id: String,
        kind: &'static str,
        id: String,
    },
    #[error("plugin {plugin_id} UI bundle is invalid: {source}")]
    Contract {
        plugin_id: String,
        source: UiContractError,
    },
}

/// Parse a plugin's linked UI documents and check them against its manifest:
/// every declared document is linked exactly once, nothing undeclared is
/// linked, each document carries its declared id and the plugin as owner, and
/// the combined bundle passes contract validation.
pub fn verify_ui_plugin(plugin: &dyn NativeUiPlugin) -> Result<UiBundle, UiPackageError> {
    let manifest = plugin.manifest();
    let documents = plugin.ui_documents();
    let ui = &manifest.ui;
    let bundle = UiBundle {
        resources: parse_kind::<ResourceDescriptor>(
            manifest,
            "resource",
            &ui.resources,
            documents.resources,
            |document| (&document.id, &document.owner_plugin),
        )?,
        pages: parse_kind::<PageDefinition>(
            manifest,
            "page",
            &ui.pages,
            documents.pages,
            |document| (&document.id, &document.owner_plugin),
        )?,
        fragments: parse_kind::<FragmentDefinition>(
            manifest,
            "fragment",
            &ui.fragments,
            documents.fragments,
            |document| (&document.id, &document.owner_plugin),
        )?,
        extensions: parse_kind::<ExtensionContribution>(
            manifest,
            "extension",
            &ui.extensions,
            documents.extensions,
            |document| (&document.id, &document.owner_plugin),
        )?,
        components: parse_kind::<ComponentDefinition>(
            manifest,
            "component",
            &ui.components,
            documents.components,
            |document| (&document.id, &document.owner_plugin),
        )?,
    };
    bundle
        .validate()
        .map_err(|source| UiPackageError::Contract {
            plugin_id: manifest.id.clone(),
            source,
        })?;
    Ok(bundle)
}

fn parse_kind<T: DeserializeOwned>(
    manifest: &Manifest,
    kind: &'static str,
    declared: &[UiFileReference],
    linked: &[NativeUiDocument],
    identity: impl Fn(&T) -> (&String, &String),
) -> Result<Vec<T>, UiPackageError> {
    let plugin_id = &manifest.id;
    let declared_ids = declared
        .iter()
        .map(|reference| reference.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut seen = BTreeSet::new();
    let mut parsed = Vec::with_capacity(linked.len());
    for document in linked {
        if !declared_ids.contains(document.id) {
            return Err(UiPackageError::Undeclared {
                plugin_id: plugin_id.clone(),
                kind,
                id: document.id.to_owned(),
            });
        }
        if !seen.insert(document.id) {
            return Err(UiPackageError::Duplicate {
                plugin_id: plugin_id.clone(),
                kind,
                id: document.id.to_owned(),
            });
        }
        let value: T =
            serde_json::from_str(document.json).map_err(|error| UiPackageError::Json {
                plugin_id: plugin_id.clone(),
                kind,
                id: document.id.to_owned(),
                message: error.to_string(),
            })?;
        let (id, owner) = identity(&value);
        if id != document.id {
            return Err(UiPackageError::IdMismatch {
                plugin_id: plugin_id.clone(),
                kind,
                linked: document.id.to_owned(),
                actual: id.clone(),
            });
        }
        if owner != plugin_id {
            return Err(UiPackageError::Owner {
                plugin_id: plugin_id.clone(),
                kind,
                id: id.clone(),
                owner: owner.clone(),
            });
        }
        parsed.push(value);
    }
    if let Some(id) = declared_ids.iter().find(|id| !seen.contains(**id)) {
        return Err(UiPackageError::Unimplemented {
            plugin_id: plugin_id.clone(),
            kind,
            id: (*id).to_owned(),
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = include_str!("../../../fixtures/plugins/ui-conformance/plugin.toml");
    const RESOURCE: &str =
        include_str!("../../../fixtures/plugins/ui-conformance/record.resource.json");
    const PAGE: &str =
        include_str!("../../../fixtures/plugins/ui-conformance/record-form.page.json");
    const RESOURCE_ID: &str = "example.conformance.record";
    const PAGE_ID: &str = "example.conformance.record-form";

    struct Fixture {
        manifest: Manifest,
        documents: NativeUiDocuments,
    }

    impl NativeUiPlugin for Fixture {
        fn manifest(&self) -> &Manifest {
            &self.manifest
        }

        fn ui_documents(&self) -> NativeUiDocuments {
            self.documents
        }
    }

    fn fixture(
        resources: &'static [NativeUiDocument],
        pages: &'static [NativeUiDocument],
    ) -> Fixture {
        Fixture {
            manifest: Manifest::parse_toml(MANIFEST).expect("fixture manifest parses"),
            documents: NativeUiDocuments {
                resources,
                pages,
                ..NativeUiDocuments::default()
            },
        }
    }

    const GOOD_RESOURCES: &[NativeUiDocument] = &[NativeUiDocument {
        id: RESOURCE_ID,
        json: RESOURCE,
    }];
    const GOOD_PAGES: &[NativeUiDocument] = &[NativeUiDocument {
        id: PAGE_ID,
        json: PAGE,
    }];

    #[test]
    fn declared_documents_verify_into_a_bundle() {
        let bundle = verify_ui_plugin(&fixture(GOOD_RESOURCES, GOOD_PAGES)).expect("verifies");
        assert_eq!(bundle.resources.len(), 1);
        assert_eq!(bundle.pages.len(), 1);
    }

    #[test]
    fn a_declared_document_must_be_linked() {
        assert!(matches!(
            verify_ui_plugin(&fixture(GOOD_RESOURCES, &[])),
            Err(UiPackageError::Unimplemented { kind: "page", .. })
        ));
    }

    #[test]
    fn an_undeclared_document_is_rejected() {
        const PAGES: &[NativeUiDocument] = &[
            NativeUiDocument {
                id: PAGE_ID,
                json: PAGE,
            },
            NativeUiDocument {
                id: "example.conformance.smuggled",
                json: PAGE,
            },
        ];
        assert!(matches!(
            verify_ui_plugin(&fixture(GOOD_RESOURCES, PAGES)),
            Err(UiPackageError::Undeclared { kind: "page", .. })
        ));
    }

    #[test]
    fn a_document_linked_twice_is_rejected() {
        const PAGES: &[NativeUiDocument] = &[
            NativeUiDocument {
                id: PAGE_ID,
                json: PAGE,
            },
            NativeUiDocument {
                id: PAGE_ID,
                json: PAGE,
            },
        ];
        assert!(matches!(
            verify_ui_plugin(&fixture(GOOD_RESOURCES, PAGES)),
            Err(UiPackageError::Duplicate { kind: "page", .. })
        ));
    }

    #[test]
    fn a_document_must_carry_its_declared_id() {
        // The resource JSON linked under the page's declared id.
        const RESOURCES: &[NativeUiDocument] = &[NativeUiDocument {
            id: RESOURCE_ID,
            json: PAGE,
        }];
        assert!(verify_ui_plugin(&fixture(RESOURCES, GOOD_PAGES)).is_err());
        const PAGES: &[NativeUiDocument] = &[NativeUiDocument {
            id: PAGE_ID,
            json: r#"{"contract_version":1,"id":"example.conformance.other","definition_version":1,"owner_plugin":"example.conformance","hosts":["internal-web"],"root":{"id":"root","component":"platform.ui.page"}}"#,
        }];
        assert!(matches!(
            verify_ui_plugin(&fixture(GOOD_RESOURCES, PAGES)),
            Err(UiPackageError::IdMismatch { .. } | UiPackageError::Json { .. })
        ));
    }

    #[test]
    fn a_document_owned_by_another_plugin_is_rejected() {
        let foreign = Box::leak(
            RESOURCE
                .replace(
                    "\"owner_plugin\": \"example.conformance\"",
                    "\"owner_plugin\": \"example.other\"",
                )
                .into_boxed_str(),
        );
        assert_ne!(foreign, RESOURCE, "fixture owner field changed shape");
        let resources = Box::leak(Box::new([NativeUiDocument {
            id: RESOURCE_ID,
            json: foreign,
        }]));
        assert!(matches!(
            verify_ui_plugin(&fixture(resources, GOOD_PAGES)),
            Err(UiPackageError::Owner { .. })
        ));
    }

    #[test]
    fn malformed_json_is_rejected() {
        const PAGES: &[NativeUiDocument] = &[NativeUiDocument {
            id: PAGE_ID,
            json: "{",
        }];
        assert!(matches!(
            verify_ui_plugin(&fixture(GOOD_RESOURCES, PAGES)),
            Err(UiPackageError::Json { .. })
        ));
    }
}
