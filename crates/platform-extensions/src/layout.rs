//! Where extension fields appear on host pages (ADR 0012).
//!
//! Hosts declare field zones in their manifests. An administrator arranges
//! extension fields into those zones, at any position among the host's own
//! fields, or leaves them out. Fields nobody has arranged yet appear in the
//! default zone of each kind, so a newly enabled extension is visible without
//! setup and looks like any other field of the record.

use crate::{ExtensionCatalog, ExtensionError, ExtensionIssue};
use plugin_contract::{FieldZoneKind, Manifest};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Largest position an administrator may give; beyond the zone's length a
/// field simply goes last.
pub const MAX_POSITION: u32 = 1000;
/// Most placements one layout may hold.
pub const MAX_PLACEMENTS: usize = 500;

/// A zone of a host page, as declared by the host plugin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FieldZone {
    pub id: String,
    pub label: String,
    pub kind: FieldZoneKind,
    pub page: String,
    pub node: String,
    pub default: bool,
}

/// The zones the active host of `resource` declares.
pub fn zones_for<'a>(
    manifests: impl IntoIterator<Item = &'a Manifest>,
    resource: &str,
) -> Vec<FieldZone> {
    manifests
        .into_iter()
        .filter(|manifest| manifest.ui.resources.iter().any(|item| item.id == resource))
        .flat_map(|manifest| manifest.ui.field_zones.iter())
        .filter(|zone| zone.resource == resource)
        .map(|zone| FieldZone {
            id: zone.id.clone(),
            label: zone.label.trim().to_owned(),
            kind: zone.kind,
            page: zone.page.clone(),
            node: zone.node.clone(),
            default: zone.default,
        })
        .collect()
}

/// One field shown in one zone. `position` counts every field in the zone,
/// the host's included, so a field can sit between two host fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldPlacement {
    pub field_id: String,
    pub zone: String,
    pub position: u32,
}

/// An administrator's arrangement for one resource. `arranged` lists every
/// field the administrator has decided about; a field in `arranged` with no
/// placement is deliberately hidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FieldLayout {
    #[serde(default)]
    pub arranged: Vec<String>,
    #[serde(default)]
    pub placements: Vec<FieldPlacement>,
}

impl ExtensionCatalog {
    /// Validates an administrator's layout for `resource` against the active
    /// fields and the host's zones. Ids are normalised and duplicates refused.
    pub fn validate_layout(
        &self,
        resource: &str,
        zones: &[FieldZone],
        layout: &FieldLayout,
    ) -> Result<FieldLayout, ExtensionError> {
        let mut issues = Vec::new();
        let known = self
            .fields_for(resource)
            .map(|field| field.id())
            .collect::<BTreeSet<_>>();
        if layout.placements.len() > MAX_PLACEMENTS || layout.arranged.len() > MAX_PLACEMENTS {
            return Err(ExtensionError::Invalid(vec![ExtensionIssue::new(
                "placements",
                "layout.too_large",
                "the layout holds too many placements",
            )]));
        }
        let mut arranged = BTreeSet::new();
        for id in &layout.arranged {
            if !known.contains(id) {
                issues.push(ExtensionIssue::new(
                    id,
                    "layout.field_unknown",
                    "this field is not available on this resource",
                ));
            }
            arranged.insert(id.clone());
        }
        let mut seen = BTreeSet::new();
        for placement in &layout.placements {
            if !known.contains(&placement.field_id) {
                issues.push(ExtensionIssue::new(
                    &placement.field_id,
                    "layout.field_unknown",
                    "this field is not available on this resource",
                ));
            }
            if !zones.iter().any(|zone| zone.id == placement.zone) {
                issues.push(ExtensionIssue::new(
                    &placement.field_id,
                    "layout.zone_unknown",
                    "this place does not exist on the page",
                ));
            }
            if placement.position > MAX_POSITION {
                issues.push(ExtensionIssue::new(
                    &placement.field_id,
                    "layout.position_invalid",
                    "the position is out of range",
                ));
            }
            if !seen.insert((placement.field_id.as_str(), placement.zone.as_str())) {
                issues.push(ExtensionIssue::new(
                    &placement.field_id,
                    "layout.duplicate",
                    "a field can appear once in each place",
                ));
            }
            arranged.insert(placement.field_id.clone());
        }
        if !issues.is_empty() {
            return Err(ExtensionError::Invalid(issues));
        }
        let mut placements = layout.placements.clone();
        placements.sort_by(|left, right| {
            left.zone
                .cmp(&right.zone)
                .then(left.position.cmp(&right.position))
                .then(left.field_id.cmp(&right.field_id))
        });
        Ok(FieldLayout {
            arranged: arranged.into_iter().collect(),
            placements,
        })
    }

    /// The placements to render for `resource`: the stored layout for
    /// arranged fields that are still active, plus default-zone placements
    /// for every active field nobody has arranged.
    pub fn effective_placements(
        &self,
        resource: &str,
        zones: &[FieldZone],
        layout: Option<&FieldLayout>,
    ) -> Vec<FieldPlacement> {
        let active = self
            .fields_for(resource)
            .map(|field| field.id())
            .collect::<Vec<_>>();
        let arranged = layout
            .map(|layout| layout.arranged.iter().cloned().collect::<BTreeSet<_>>())
            .unwrap_or_default();
        let mut placements = layout
            .map(|layout| {
                layout
                    .placements
                    .iter()
                    .filter(|placement| {
                        active.contains(&placement.field_id)
                            && zones.iter().any(|zone| zone.id == placement.zone)
                    })
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for zone in zones.iter().filter(|zone| zone.default) {
            for id in active.iter().filter(|id| !arranged.contains(*id)) {
                placements.push(FieldPlacement {
                    field_id: id.clone(),
                    zone: zone.id.clone(),
                    position: MAX_POSITION,
                });
            }
        }
        placements
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contract::Manifest;

    fn manifests() -> Vec<Manifest> {
        let host = Manifest::parse_toml(
            r#"
manifest_version = 1
id = "platform.contacts"
name = "Contacts"
version = "0.1.0"
description = "Host"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[[ui.resources]]
id = "platform.contacts.person"
path = "ui/resources/person.json"

[[ui.pages]]
id = "platform.contacts.person-detail"
path = "ui/pages/person-detail.json"

[[ui.field_zones]]
id = "details"
resource = "platform.contacts.person"
label = "Details"
kind = "display"
page = "platform.contacts.person-detail"
node = "person-details"
default = true

[[ui.field_zones]]
id = "highlights"
resource = "platform.contacts.person"
label = "Highlights"
kind = "display"
page = "platform.contacts.person-detail"
node = "person-highlights"

[[ui.field_zones]]
id = "form"
resource = "platform.contacts.person"
label = "Form"
kind = "form"
page = "platform.contacts.person-detail"
node = "person-form"
default = true
"#,
        )
        .expect("host");
        let extension = Manifest::parse_toml(
            r#"
manifest_version = 1
id = "example.loyalty"
name = "Loyalty"
version = "0.1.0"
description = "Loyalty"
runtime = "native"

[compatibility]
platform = ">=0.1.0,<0.2.0"

[dependencies."platform.contacts"]
version = ">=0.1.0"
reasons = ["extension"]

[[permissions]]
id = "loyalty.read"
description = "Read"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "tier"
label = "Tier"
field_type = "text"
read_permission = "loyalty.read"
write_permission = "loyalty.read"

[[extensions.fields]]
host = "platform.contacts"
target = "platform.contacts.person"
key = "points"
label = "Points"
field_type = "number"
read_permission = "loyalty.read"
write_permission = "loyalty.read"
"#,
        )
        .expect("extension");
        vec![host, extension]
    }

    const PERSON: &str = "platform.contacts.person";

    fn placement(field: &str, zone: &str, position: u32) -> FieldPlacement {
        FieldPlacement {
            field_id: field.into(),
            zone: zone.into(),
            position,
        }
    }

    #[test]
    fn zones_come_from_the_host_of_the_resource() {
        let manifests = manifests();
        let zones = zones_for(&manifests, PERSON);
        assert_eq!(
            zones
                .iter()
                .map(|zone| zone.id.as_str())
                .collect::<Vec<_>>(),
            ["details", "highlights", "form"]
        );
        assert!(zones_for(&manifests, "platform.contacts.unknown").is_empty());
    }

    #[test]
    fn unarranged_fields_land_in_every_default_zone() {
        let manifests = manifests();
        let catalog = ExtensionCatalog::from_manifests(&manifests);
        let zones = zones_for(&manifests, PERSON);
        let placements = catalog.effective_placements(PERSON, &zones, None);
        assert_eq!(
            placements,
            [
                placement("example.loyalty.tier", "details", MAX_POSITION),
                placement("example.loyalty.points", "details", MAX_POSITION),
                placement("example.loyalty.tier", "form", MAX_POSITION),
                placement("example.loyalty.points", "form", MAX_POSITION),
            ]
        );
    }

    #[test]
    fn arranged_fields_follow_the_layout_and_may_be_hidden() {
        let manifests = manifests();
        let catalog = ExtensionCatalog::from_manifests(&manifests);
        let zones = zones_for(&manifests, PERSON);
        let layout = catalog
            .validate_layout(
                PERSON,
                &zones,
                &FieldLayout {
                    arranged: vec!["example.loyalty.points".into()],
                    placements: vec![placement("example.loyalty.tier", "highlights", 1)],
                },
            )
            .expect("valid layout");
        assert_eq!(
            layout.arranged,
            ["example.loyalty.points", "example.loyalty.tier"]
        );
        // Tier sits second in the highlights; points is hidden everywhere.
        assert_eq!(
            catalog.effective_placements(PERSON, &zones, Some(&layout)),
            [placement("example.loyalty.tier", "highlights", 1)]
        );
    }

    #[test]
    fn layouts_name_active_fields_existing_zones_and_bounded_positions() {
        let manifests = manifests();
        let catalog = ExtensionCatalog::from_manifests(&manifests);
        let zones = zones_for(&manifests, PERSON);
        for (layout, code) in [
            (
                FieldLayout {
                    arranged: vec![],
                    placements: vec![placement("example.loyalty.gone", "details", 0)],
                },
                "layout.field_unknown",
            ),
            (
                FieldLayout {
                    arranged: vec![],
                    placements: vec![placement("example.loyalty.tier", "sidebar", 0)],
                },
                "layout.zone_unknown",
            ),
            (
                FieldLayout {
                    arranged: vec![],
                    placements: vec![placement("example.loyalty.tier", "details", 5000)],
                },
                "layout.position_invalid",
            ),
            (
                FieldLayout {
                    arranged: vec![],
                    placements: vec![
                        placement("example.loyalty.tier", "details", 0),
                        placement("example.loyalty.tier", "details", 2),
                    ],
                },
                "layout.duplicate",
            ),
        ] {
            assert!(
                matches!(
                    catalog.validate_layout(PERSON, &zones, &layout),
                    Err(ExtensionError::Invalid(issues)) if issues[0].code == code
                ),
                "{code}"
            );
        }
    }

    #[test]
    fn fields_of_inactive_extensions_drop_out_of_a_stored_layout() {
        let manifests = manifests();
        let zones = zones_for(&manifests, PERSON);
        let layout = FieldLayout {
            arranged: vec!["example.loyalty.tier".into()],
            placements: vec![placement("example.loyalty.tier", "highlights", 0)],
        };
        let host_only = ExtensionCatalog::from_manifests(&manifests[..1]);
        assert!(host_only
            .effective_placements(PERSON, &zones, Some(&layout))
            .is_empty());
    }
}
