//! Governed tenant draft and publication state for metadata UI definitions.

use crate::{MetadataCatalogue, PublishedPatch, ResolveContext, ResolveError, ResolvedPage};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;
use uuid::Uuid;

/// Permission to manage tenant UI customisations. It is the id seeded by core
/// migration 0004 and required by the API route boundary for the same routes.
pub const MANAGE_PERMISSION: &str = "ui.metadata.manage";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationActor {
    pub actor_id: Uuid,
    pub permissions: BTreeSet<String>,
}

impl PublicationActor {
    fn can_manage(&self) -> bool {
        self.permissions.contains(MANAGE_PERMISSION)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantDraft {
    pub id: Uuid,
    pub version: u64,
    pub base_source_versions: BTreeMap<String, String>,
    pub patches: Vec<PublishedPatch>,
    pub created_by: Uuid,
    pub updated_by: Uuid,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPublication {
    pub version: u32,
    pub source_draft_id: Option<Uuid>,
    pub base_source_versions: BTreeMap<String, String>,
    pub patches: Vec<PublishedPatch>,
    pub published_by: Uuid,
    pub published_at: DateTime<Utc>,
    pub rollback_of: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublicationAction {
    DraftCreated,
    DraftUpdated,
    DraftValidated,
    DraftAbandoned,
    Published,
    RolledBack,
    Imported,
    Exported,
}

impl PublicationAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DraftCreated => "draft-created",
            Self::DraftUpdated => "draft-updated",
            Self::DraftValidated => "draft-validated",
            Self::DraftAbandoned => "draft-abandoned",
            Self::Published => "published",
            Self::RolledBack => "rolled-back",
            Self::Imported => "imported",
            Self::Exported => "exported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicationAuditRecord {
    pub id: Uuid,
    pub actor_id: Uuid,
    pub action: PublicationAction,
    pub revision: u64,
    pub draft_id: Option<Uuid>,
    pub publication_version: Option<u32>,
    pub occurred_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublicationBundle {
    pub format_version: u32,
    pub page_id: String,
    pub base_source_versions: BTreeMap<String, String>,
    pub patches: Vec<PublishedPatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportPlan {
    pub page_id: String,
    pub patch_count: usize,
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicationComparison {
    pub from_version: u32,
    pub to_version: u32,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

impl ImportPlan {
    pub fn can_apply(&self) -> bool {
        self.conflicts.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TenantPagePublications {
    pub tenant_id: Uuid,
    pub page_id: String,
    pub revision: u64,
    pub publication_generation: u64,
    pub draft: Option<TenantDraft>,
    pub publications: Vec<TenantPublication>,
    pub audit: Vec<PublicationAuditRecord>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PublicationError {
    #[error("metadata publication permission denied")]
    Denied,
    #[error("publication state changed; expected revision {expected}, actual {actual}")]
    StaleRevision { expected: u64, actual: u64 },
    #[error("draft already exists")]
    DraftExists,
    #[error("draft does not exist")]
    DraftMissing,
    #[error("draft changed; expected version {expected}, actual {actual}")]
    StaleDraft { expected: u64, actual: u64 },
    #[error("publication {0} does not exist")]
    PublicationMissing(u32),
    #[error("bundle format {0} is unsupported")]
    UnsupportedBundle(u32),
    #[error("bundle targets page {actual}, expected {expected}")]
    WrongPage { expected: String, actual: String },
    #[error("definition cannot be resolved: {0}")]
    InvalidDefinition(String),
}

impl TenantPagePublications {
    pub fn new(tenant_id: Uuid, page_id: impl Into<String>) -> Self {
        Self {
            tenant_id,
            page_id: page_id.into(),
            revision: 0,
            publication_generation: 0,
            draft: None,
            publications: Vec::new(),
            audit: Vec::new(),
        }
    }

    pub fn begin_draft(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        base_source_versions: BTreeMap<String, String>,
        now: DateTime<Utc>,
    ) -> Result<&TenantDraft, PublicationError> {
        self.authorize(actor)?;
        self.check_revision(expected_revision)?;
        if self.draft.is_some() {
            return Err(PublicationError::DraftExists);
        }
        let draft = TenantDraft {
            id: Uuid::now_v7(),
            version: 1,
            base_source_versions,
            patches: Vec::new(),
            created_by: actor.actor_id,
            updated_by: actor.actor_id,
            created_at: now,
            updated_at: now,
        };
        let draft_id = draft.id;
        self.draft = Some(draft);
        self.bump();
        self.record(
            actor,
            PublicationAction::DraftCreated,
            Some(draft_id),
            None,
            now,
        );
        Ok(self.draft.as_ref().expect("draft was inserted"))
    }

    pub fn update_draft(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        expected_draft_version: u64,
        mut patches: Vec<PublishedPatch>,
        now: DateTime<Utc>,
    ) -> Result<&TenantDraft, PublicationError> {
        self.authorize(actor)?;
        self.check_revision(expected_revision)?;
        let draft = self.draft.as_mut().ok_or(PublicationError::DraftMissing)?;
        if draft.version != expected_draft_version {
            return Err(PublicationError::StaleDraft {
                expected: expected_draft_version,
                actual: draft.version,
            });
        }
        for patch in &mut patches {
            patch.publication_version = 0;
        }
        draft.version += 1;
        draft.patches = patches;
        draft.updated_by = actor.actor_id;
        draft.updated_at = now;
        let draft_id = draft.id;
        self.bump();
        self.record(
            actor,
            PublicationAction::DraftUpdated,
            Some(draft_id),
            None,
            now,
        );
        Ok(self.draft.as_ref().expect("draft still exists"))
    }

    pub fn preview(
        &self,
        actor: &PublicationActor,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
    ) -> Result<ResolvedPage, PublicationError> {
        self.authorize(actor)?;
        let draft = self.draft.as_ref().ok_or(PublicationError::DraftMissing)?;
        resolve(catalogue, &self.page_id, context, &draft.patches)
    }

    pub fn validate_draft(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
        now: DateTime<Utc>,
    ) -> Result<ResolvedPage, PublicationError> {
        let mut resolved = None;
        let page_id = self.page_id.clone();
        self.validate_draft_with(
            actor,
            expected_revision,
            |patches| {
                let page = resolve(catalogue, &page_id, context, patches)?;
                resolved = Some(page);
                Ok(())
            },
            now,
        )?;
        Ok(resolved.expect("successful validation produced a page"))
    }

    pub fn validate_draft_with(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        validate: impl FnOnce(&[PublishedPatch]) -> Result<(), PublicationError>,
        now: DateTime<Utc>,
    ) -> Result<(), PublicationError> {
        self.authorize(actor)?;
        self.check_revision(expected_revision)?;
        let draft_id = self
            .draft
            .as_ref()
            .ok_or(PublicationError::DraftMissing)?
            .id;
        validate(&self.draft.as_ref().expect("draft was checked").patches)?;
        self.bump();
        self.record(
            actor,
            PublicationAction::DraftValidated,
            Some(draft_id),
            None,
            now,
        );
        Ok(())
    }

    pub fn publish(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        expected_draft_version: u64,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
        now: DateTime<Utc>,
    ) -> Result<&TenantPublication, PublicationError> {
        let page_id = self.page_id.clone();
        self.publish_with_validation(
            actor,
            expected_revision,
            expected_draft_version,
            |patches| resolve(catalogue, &page_id, context, patches).map(|_| ()),
            now,
        )
    }

    pub fn publish_with_validation(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        expected_draft_version: u64,
        validate: impl FnOnce(&[PublishedPatch]) -> Result<(), PublicationError>,
        now: DateTime<Utc>,
    ) -> Result<&TenantPublication, PublicationError> {
        self.authorize(actor)?;
        self.check_revision(expected_revision)?;
        let draft = self.draft.as_ref().ok_or(PublicationError::DraftMissing)?;
        if draft.version != expected_draft_version {
            return Err(PublicationError::StaleDraft {
                expected: expected_draft_version,
                actual: draft.version,
            });
        }
        validate(&draft.patches)?;
        let draft = self.draft.take().expect("draft was checked");
        let version = self.next_publication_version();
        let mut patches = draft.patches;
        for patch in &mut patches {
            patch.publication_version = version;
        }
        self.publications.push(TenantPublication {
            version,
            source_draft_id: Some(draft.id),
            base_source_versions: draft.base_source_versions,
            patches,
            published_by: actor.actor_id,
            published_at: now,
            rollback_of: None,
        });
        self.bump_publication();
        self.record(
            actor,
            PublicationAction::Published,
            Some(draft.id),
            Some(version),
            now,
        );
        Ok(self.publications.last().expect("publication was inserted"))
    }

    pub fn rollback(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        target_version: u32,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
        now: DateTime<Utc>,
    ) -> Result<&TenantPublication, PublicationError> {
        let page_id = self.page_id.clone();
        self.rollback_with_validation(
            actor,
            expected_revision,
            target_version,
            |patches| resolve(catalogue, &page_id, context, patches).map(|_| ()),
            now,
        )
    }

    pub fn rollback_with_validation(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        target_version: u32,
        validate: impl FnOnce(&[PublishedPatch]) -> Result<(), PublicationError>,
        now: DateTime<Utc>,
    ) -> Result<&TenantPublication, PublicationError> {
        self.authorize(actor)?;
        self.check_revision(expected_revision)?;
        let target = self
            .publications
            .iter()
            .find(|publication| publication.version == target_version)
            .cloned()
            .ok_or(PublicationError::PublicationMissing(target_version))?;
        validate(&target.patches)?;
        let version = self.next_publication_version();
        let mut patches = target.patches;
        for patch in &mut patches {
            patch.publication_version = version;
        }
        self.publications.push(TenantPublication {
            version,
            source_draft_id: None,
            base_source_versions: target.base_source_versions,
            patches,
            published_by: actor.actor_id,
            published_at: now,
            rollback_of: Some(target_version),
        });
        self.bump_publication();
        self.record(
            actor,
            PublicationAction::RolledBack,
            None,
            Some(version),
            now,
        );
        Ok(self.publications.last().expect("rollback was inserted"))
    }

    pub fn abandon_draft(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        expected_draft_version: u64,
        now: DateTime<Utc>,
    ) -> Result<(), PublicationError> {
        self.authorize(actor)?;
        self.check_revision(expected_revision)?;
        let draft = self.draft.as_ref().ok_or(PublicationError::DraftMissing)?;
        if draft.version != expected_draft_version {
            return Err(PublicationError::StaleDraft {
                expected: expected_draft_version,
                actual: draft.version,
            });
        }
        let draft_id = draft.id;
        self.draft = None;
        self.bump();
        self.record(
            actor,
            PublicationAction::DraftAbandoned,
            Some(draft_id),
            None,
            now,
        );
        Ok(())
    }

    pub fn published_patches(&self) -> &[PublishedPatch] {
        self.publications
            .last()
            .map_or(&[], |publication| publication.patches.as_slice())
    }

    pub fn export_published(
        &mut self,
        actor: &PublicationActor,
        now: DateTime<Utc>,
    ) -> Result<PublicationBundle, PublicationError> {
        self.authorize(actor)?;
        let publication = self
            .publications
            .last()
            .ok_or(PublicationError::PublicationMissing(0))?;
        let bundle = PublicationBundle {
            format_version: 1,
            page_id: self.page_id.clone(),
            base_source_versions: publication.base_source_versions.clone(),
            patches: publication.patches.clone(),
        };
        let publication_version = publication.version;
        self.bump();
        self.record(
            actor,
            PublicationAction::Exported,
            None,
            Some(publication_version),
            now,
        );
        Ok(bundle)
    }

    pub fn compare(
        &self,
        actor: &PublicationActor,
        from_version: u32,
        to_version: u32,
    ) -> Result<PublicationComparison, PublicationError> {
        self.authorize(actor)?;
        let from = self
            .publications
            .iter()
            .find(|item| item.version == from_version)
            .ok_or(PublicationError::PublicationMissing(from_version))?;
        let to = self
            .publications
            .iter()
            .find(|item| item.version == to_version)
            .ok_or(PublicationError::PublicationMissing(to_version))?;
        let from_by_id: BTreeMap<_, _> = from
            .patches
            .iter()
            .map(|patch| (patch.id.as_str(), patch))
            .collect();
        let to_by_id: BTreeMap<_, _> = to
            .patches
            .iter()
            .map(|patch| (patch.id.as_str(), patch))
            .collect();
        Ok(PublicationComparison {
            from_version,
            to_version,
            added: to_by_id
                .keys()
                .filter(|id| !from_by_id.contains_key(**id))
                .map(|id| (*id).to_owned())
                .collect(),
            removed: from_by_id
                .keys()
                .filter(|id| !to_by_id.contains_key(**id))
                .map(|id| (*id).to_owned())
                .collect(),
            changed: from_by_id
                .iter()
                .filter(|(id, patch)| {
                    to_by_id
                        .get(**id)
                        .is_some_and(|candidate| !same_patch_content(candidate, patch))
                })
                .map(|(id, _)| (**id).to_owned())
                .collect(),
        })
    }

    pub fn plan_import(
        &self,
        actor: &PublicationActor,
        bundle: &PublicationBundle,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
    ) -> Result<ImportPlan, PublicationError> {
        let page_id = self.page_id.clone();
        self.plan_import_with(actor, bundle, |patches| {
            resolve(catalogue, &page_id, context, patches).map(|_| ())
        })
    }

    pub fn plan_import_with(
        &self,
        actor: &PublicationActor,
        bundle: &PublicationBundle,
        validate: impl FnOnce(&[PublishedPatch]) -> Result<(), PublicationError>,
    ) -> Result<ImportPlan, PublicationError> {
        self.authorize(actor)?;
        if bundle.format_version != 1 {
            return Err(PublicationError::UnsupportedBundle(bundle.format_version));
        }
        if bundle.page_id != self.page_id {
            return Err(PublicationError::WrongPage {
                expected: self.page_id.clone(),
                actual: bundle.page_id.clone(),
            });
        }
        let conflicts = validate(&bundle.patches)
            .err()
            .map_or_else(Vec::new, |error| vec![error.to_string()]);
        Ok(ImportPlan {
            page_id: self.page_id.clone(),
            patch_count: bundle.patches.len(),
            conflicts,
        })
    }

    pub fn import_as_draft(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        bundle: PublicationBundle,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
        now: DateTime<Utc>,
    ) -> Result<&TenantDraft, PublicationError> {
        let page_id = self.page_id.clone();
        self.import_as_draft_with_validation(
            actor,
            expected_revision,
            bundle,
            |patches| resolve(catalogue, &page_id, context, patches).map(|_| ()),
            now,
        )
    }

    pub fn import_as_draft_with_validation(
        &mut self,
        actor: &PublicationActor,
        expected_revision: u64,
        bundle: PublicationBundle,
        validate: impl FnOnce(&[PublishedPatch]) -> Result<(), PublicationError>,
        now: DateTime<Utc>,
    ) -> Result<&TenantDraft, PublicationError> {
        self.check_revision(expected_revision)?;
        let plan = self.plan_import_with(actor, &bundle, validate)?;
        if !plan.can_apply() {
            return Err(PublicationError::InvalidDefinition(
                plan.conflicts.join("; "),
            ));
        }
        if self.draft.is_some() {
            return Err(PublicationError::DraftExists);
        }
        let mut patches = bundle.patches;
        for patch in &mut patches {
            patch.publication_version = 0;
        }
        let draft = TenantDraft {
            id: Uuid::now_v7(),
            version: 1,
            base_source_versions: bundle.base_source_versions,
            patches,
            created_by: actor.actor_id,
            updated_by: actor.actor_id,
            created_at: now,
            updated_at: now,
        };
        let draft_id = draft.id;
        self.draft = Some(draft);
        self.bump();
        self.record(
            actor,
            PublicationAction::Imported,
            Some(draft_id),
            None,
            now,
        );
        Ok(self.draft.as_ref().expect("imported draft was inserted"))
    }

    pub fn validate_current_publication(
        &self,
        catalogue: &MetadataCatalogue,
        context: &ResolveContext,
    ) -> Result<ResolvedPage, PublicationError> {
        resolve(catalogue, &self.page_id, context, self.published_patches())
    }

    fn authorize(&self, actor: &PublicationActor) -> Result<(), PublicationError> {
        if actor.can_manage() {
            Ok(())
        } else {
            Err(PublicationError::Denied)
        }
    }

    fn check_revision(&self, expected: u64) -> Result<(), PublicationError> {
        if self.revision == expected {
            Ok(())
        } else {
            Err(PublicationError::StaleRevision {
                expected,
                actual: self.revision,
            })
        }
    }

    fn next_publication_version(&self) -> u32 {
        self.publications.last().map_or(1, |item| item.version + 1)
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    fn bump_publication(&mut self) {
        self.bump();
        self.publication_generation += 1;
    }

    fn record(
        &mut self,
        actor: &PublicationActor,
        action: PublicationAction,
        draft_id: Option<Uuid>,
        publication_version: Option<u32>,
        now: DateTime<Utc>,
    ) {
        self.audit.push(PublicationAuditRecord {
            id: Uuid::now_v7(),
            actor_id: actor.actor_id,
            action,
            revision: self.revision,
            draft_id,
            publication_version,
            occurred_at: now,
        });
    }
}

fn same_patch_content(left: &PublishedPatch, right: &PublishedPatch) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.publication_version = 0;
    right.publication_version = 0;
    left == right
}

fn resolve(
    catalogue: &MetadataCatalogue,
    page_id: &str,
    context: &ResolveContext,
    patches: &[PublishedPatch],
) -> Result<ResolvedPage, PublicationError> {
    catalogue
        .resolve(page_id, context, patches, &[])
        .map_err(|error: ResolveError| PublicationError::InvalidDefinition(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DefinitionVersionRange, PluginUiPackage, PluginUiState};
    use plugin_contract::ui::{
        HostProfile, PageDefinition, PageKind, PatchOperation, UiBundle, UiNode,
        UI_CONTRACT_VERSION,
    };
    use std::collections::BTreeMap;

    fn node(id: &str, component: &str, children: Vec<UiNode>) -> UiNode {
        UiNode {
            id: id.into(),
            component: component.into(),
            props: BTreeMap::new(),
            bindings: BTreeMap::new(),
            conditions: BTreeMap::new(),
            children,
            permission: None,
            hosts: vec![HostProfile::InternalWeb],
        }
    }

    fn catalogue(definition_version: u32) -> MetadataCatalogue {
        let mut catalogue = MetadataCatalogue::default();
        for component in ["platform.ui.page", "platform.ui.slot", "platform.ui.card"] {
            catalogue.register_host_component(component);
        }
        catalogue
            .install(PluginUiPackage {
                plugin_id: "platform.sample".into(),
                plugin_version: format!("{definition_version}.0.0"),
                dependencies: BTreeSet::new(),
                state: PluginUiState::Enabled,
                bundle: UiBundle {
                    pages: vec![PageDefinition {
                        contract_version: UI_CONTRACT_VERSION,
                        id: "platform.sample.record".into(),
                        definition_version,
                        owner_plugin: "platform.sample".into(),
                        kind: PageKind::Record,
                        hosts: vec![HostProfile::InternalWeb],
                        resource: None,
                        data_sources: Vec::new(),
                        actions: Vec::new(),
                        variants: Vec::new(),
                        root: node(
                            "page-root",
                            "platform.ui.page",
                            vec![node("main", "platform.ui.slot", Vec::new())],
                        ),
                    }],
                    ..UiBundle::default()
                },
            })
            .unwrap();
        catalogue
    }

    fn context() -> ResolveContext {
        ResolveContext {
            host: HostProfile::InternalWeb,
            mode: "view".into(),
            form_factor: "wide".into(),
            audience: None,
            roles: BTreeSet::new(),
            resource_subtype: None,
            locale: "en-GB".into(),
            permissions: BTreeSet::new(),
            values: BTreeMap::new(),
        }
    }

    fn actor(allowed: bool) -> PublicationActor {
        PublicationActor {
            actor_id: Uuid::now_v7(),
            permissions: if allowed {
                BTreeSet::from([MANAGE_PERMISSION.into()])
            } else {
                BTreeSet::new()
            },
        }
    }

    fn patch(id: &str, target: &str, version: u32) -> PublishedPatch {
        PublishedPatch {
            id: id.into(),
            publication_version: 99,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange::exact(version),
            target_node: target.into(),
            operation: PatchOperation::Inside,
            priority: 0,
            node: Some(node(id, "platform.ui.card", Vec::new())),
            props: BTreeMap::new(),
        }
    }

    fn aggregate() -> TenantPagePublications {
        TenantPagePublications::new(Uuid::now_v7(), "platform.sample.record")
    }

    fn timestamp() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-07-16T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn draft_preview_is_authorized_and_invisible_to_published_delivery() {
        let mut state = aggregate();
        let admin = actor(true);
        let denied = actor(false);
        assert_eq!(
            state.begin_draft(&denied, 0, BTreeMap::new(), timestamp()),
            Err(PublicationError::Denied)
        );
        state
            .begin_draft(
                &admin,
                0,
                BTreeMap::from([("platform.sample".into(), "3.0.0".into())]),
                timestamp(),
            )
            .unwrap();
        state
            .update_draft(
                &admin,
                1,
                1,
                vec![patch("draft-card", "main", 3)],
                timestamp(),
            )
            .unwrap();

        assert!(state.published_patches().is_empty());
        let preview = state.preview(&admin, &catalogue(3), &context()).unwrap();
        assert_eq!(preview.root.children[0].children[0].id, "draft-card");
        assert_eq!(
            state.preview(&denied, &catalogue(3), &context()),
            Err(PublicationError::Denied)
        );
    }

    #[test]
    fn optimistic_concurrency_rejects_stale_aggregate_and_draft_writes() {
        let mut state = aggregate();
        let admin = actor(true);
        state
            .begin_draft(&admin, 0, BTreeMap::new(), timestamp())
            .unwrap();
        assert_eq!(
            state.update_draft(&admin, 0, 1, Vec::new(), timestamp()),
            Err(PublicationError::StaleRevision {
                expected: 0,
                actual: 1
            })
        );
        assert_eq!(
            state.update_draft(&admin, 1, 9, Vec::new(), timestamp()),
            Err(PublicationError::StaleDraft {
                expected: 9,
                actual: 1
            })
        );
        assert_eq!(state.revision, 1);
        assert_eq!(state.draft.as_ref().unwrap().version, 1);
    }

    #[test]
    fn publish_is_validated_immutable_and_advances_cache_generation() {
        let mut state = aggregate();
        let admin = actor(true);
        state
            .begin_draft(&admin, 0, BTreeMap::new(), timestamp())
            .unwrap();
        state
            .update_draft(
                &admin,
                1,
                1,
                vec![patch("published-card", "main", 3)],
                timestamp(),
            )
            .unwrap();
        let publication = state
            .publish(&admin, 2, 2, &catalogue(3), &context(), timestamp())
            .unwrap()
            .clone();
        assert_eq!(publication.version, 1);
        assert_eq!(publication.patches[0].publication_version, 1);
        assert!(state.draft.is_none());
        assert_eq!(state.publication_generation, 1);

        let encoded = serde_json::to_string(&state).unwrap();
        let restored: TenantPagePublications = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored, state);
        assert_eq!(restored.published_patches()[0].id, "published-card");
    }

    #[test]
    fn invalid_publish_preserves_the_draft_and_last_known_good_publication() {
        let mut state = aggregate();
        let admin = actor(true);
        state
            .begin_draft(&admin, 0, BTreeMap::new(), timestamp())
            .unwrap();
        state
            .update_draft(&admin, 1, 1, vec![patch("good", "main", 3)], timestamp())
            .unwrap();
        state
            .publish(&admin, 2, 2, &catalogue(3), &context(), timestamp())
            .unwrap();
        state
            .begin_draft(&admin, 3, BTreeMap::new(), timestamp())
            .unwrap();
        state
            .update_draft(
                &admin,
                4,
                1,
                vec![patch("bad", "removed-anchor", 3)],
                timestamp(),
            )
            .unwrap();
        let result = state.publish(&admin, 5, 2, &catalogue(3), &context(), timestamp());
        assert!(matches!(
            result,
            Err(PublicationError::InvalidDefinition(_))
        ));
        assert_eq!(state.publications.len(), 1);
        assert_eq!(state.published_patches()[0].id, "good");
        assert_eq!(state.draft.as_ref().unwrap().patches[0].id, "bad");
        assert_eq!(state.publication_generation, 1);
    }

    #[test]
    fn rollback_creates_a_new_audited_publication_without_mutating_history() {
        let mut state = aggregate();
        let admin = actor(true);
        for id in ["first", "second"] {
            let revision = state.revision;
            state
                .begin_draft(&admin, revision, BTreeMap::new(), timestamp())
                .unwrap();
            let revision = state.revision;
            state
                .update_draft(&admin, revision, 1, vec![patch(id, "main", 3)], timestamp())
                .unwrap();
            let revision = state.revision;
            state
                .publish(&admin, revision, 2, &catalogue(3), &context(), timestamp())
                .unwrap();
        }
        let first_before = state.publications[0].clone();
        let comparison = state.compare(&admin, 1, 2).unwrap();
        assert_eq!(comparison.added, vec!["second"]);
        assert_eq!(comparison.removed, vec!["first"]);
        assert!(comparison.changed.is_empty());
        let revision = state.revision;
        let rollback = state
            .rollback(&admin, revision, 1, &catalogue(3), &context(), timestamp())
            .unwrap();
        assert_eq!(rollback.version, 3);
        assert_eq!(rollback.rollback_of, Some(1));
        assert_eq!(rollback.patches[0].id, "first");
        assert_eq!(state.publications[0], first_before);
        assert_eq!(state.publication_generation, 3);
        let rollback_comparison = state.compare(&admin, 1, 3).unwrap();
        assert!(rollback_comparison.added.is_empty());
        assert!(rollback_comparison.removed.is_empty());
        assert!(rollback_comparison.changed.is_empty());
        assert_eq!(
            state.audit.last().unwrap().action,
            PublicationAction::RolledBack
        );
    }

    #[test]
    fn import_dry_run_reports_exact_conflicts_and_is_atomic() {
        let mut state = aggregate();
        let admin = actor(true);
        let invalid = PublicationBundle {
            format_version: 1,
            page_id: state.page_id.clone(),
            base_source_versions: BTreeMap::new(),
            patches: vec![patch("imported", "missing", 3)],
        };
        let plan = state
            .plan_import(&admin, &invalid, &catalogue(3), &context())
            .unwrap();
        assert!(!plan.can_apply());
        assert!(plan.conflicts[0].contains("missing anchor"));
        let before = state.clone();
        assert!(matches!(
            state.import_as_draft(&admin, 0, invalid, &catalogue(3), &context(), timestamp()),
            Err(PublicationError::InvalidDefinition(_))
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn incompatible_plugin_upgrade_reports_conflict_without_replacing_publication() {
        let mut state = aggregate();
        let admin = actor(true);
        state
            .begin_draft(&admin, 0, BTreeMap::new(), timestamp())
            .unwrap();
        state
            .update_draft(&admin, 1, 1, vec![patch("card", "main", 3)], timestamp())
            .unwrap();
        state
            .publish(&admin, 2, 2, &catalogue(3), &context(), timestamp())
            .unwrap();
        let before = state.clone();
        let error = state
            .validate_current_publication(&catalogue(4), &context())
            .unwrap_err();
        assert!(error.to_string().contains("incompatible page version 4"));
        assert_eq!(state, before);
    }
}
