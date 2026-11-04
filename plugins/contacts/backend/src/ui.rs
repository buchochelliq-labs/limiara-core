//! Contacts UI metadata, compiled into the host so it is covered by the same
//! trust as the linked code (see `plugin_runtime_native::verify_ui_plugin`).

use plugin_contract::{Manifest, ManifestError};
use plugin_runtime_native::{NativeUiDocument, NativeUiDocuments, NativeUiPlugin};

const MANIFEST: &str = include_str!("../../plugin.toml");

const RESOURCES: &[NativeUiDocument] = &[
    NativeUiDocument {
        id: "platform.contacts.person",
        json: include_str!("../../ui/resources/person.json"),
    },
    NativeUiDocument {
        id: "platform.contacts.organisation",
        json: include_str!("../../ui/resources/organisation.json"),
    },
];

const PAGES: &[NativeUiDocument] = &[
    NativeUiDocument {
        id: "platform.contacts.person-form",
        json: include_str!("../../ui/pages/person-form.json"),
    },
    NativeUiDocument {
        id: "platform.contacts.people-directory",
        json: include_str!("../../ui/pages/people-directory.json"),
    },
    NativeUiDocument {
        id: "platform.contacts.person-detail",
        json: include_str!("../../ui/pages/person-detail.json"),
    },
    NativeUiDocument {
        id: "platform.contacts.organisation-directory",
        json: include_str!("../../ui/pages/organisation-directory.json"),
    },
    NativeUiDocument {
        id: "platform.contacts.organisation-detail",
        json: include_str!("../../ui/pages/organisation-detail.json"),
    },
];

/// The Contacts UI plugin: its manifest plus the UI documents it links.
pub struct ContactsUiPlugin {
    manifest: Manifest,
}

impl ContactsUiPlugin {
    pub fn new() -> Result<Self, ManifestError> {
        Ok(Self {
            manifest: Manifest::parse_toml(MANIFEST)?,
        })
    }
}

impl NativeUiPlugin for ContactsUiPlugin {
    fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    fn ui_documents(&self) -> NativeUiDocuments {
        NativeUiDocuments {
            resources: RESOURCES,
            pages: PAGES,
            ..NativeUiDocuments::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_runtime_native::verify_ui_plugin;

    #[test]
    fn linked_ui_documents_match_the_manifest() {
        let plugin = ContactsUiPlugin::new().expect("Contacts manifest parses");
        let bundle = verify_ui_plugin(&plugin).expect("Contacts UI documents verify");
        assert_eq!(bundle.resources.len(), plugin.manifest().ui.resources.len());
        assert_eq!(bundle.pages.len(), plugin.manifest().ui.pages.len());
    }
}
