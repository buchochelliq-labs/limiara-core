#![forbid(unsafe_code)]

//! Domain-neutral catalogue and deterministic resolver for metadata-driven UI.

pub mod external_host;
pub mod external_submission;
pub mod publication;

use plugin_contract::ui::{
    ActionDefinition, Binding, ConditionExpression, DataSourceDefinition, ExtensionContribution,
    HostProfile, PageDefinition, PatchOperation, UiBundle, UiContractError, UiNode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginUiState {
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginUiPackage {
    pub plugin_id: String,
    pub plugin_version: String,
    #[serde(default)]
    pub dependencies: BTreeSet<String>,
    pub state: PluginUiState,
    pub bundle: UiBundle,
}

#[derive(Debug, Default, Clone)]
pub struct MetadataCatalogue {
    packages: BTreeMap<String, PluginUiPackage>,
    host_components: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveContext {
    pub host: HostProfile,
    pub mode: String,
    pub form_factor: String,
    pub audience: Option<String>,
    pub roles: BTreeSet<String>,
    pub resource_subtype: Option<String>,
    pub locale: String,
    pub permissions: BTreeSet<String>,
    pub values: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefinitionVersionRange {
    pub min_inclusive: u32,
    pub max_exclusive: Option<u32>,
}

impl DefinitionVersionRange {
    pub fn exact(version: u32) -> Self {
        Self {
            min_inclusive: version,
            max_exclusive: version.checked_add(1),
        }
    }

    pub fn contains(self, version: u32) -> bool {
        version >= self.min_inclusive
            && self
                .max_exclusive
                .is_none_or(|max_exclusive| version < max_exclusive)
    }

    pub fn is_valid(self) -> bool {
        self.max_exclusive
            .is_none_or(|max_exclusive| max_exclusive > self.min_inclusive)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PublishedPatch {
    pub id: String,
    pub publication_version: u32,
    pub target_page: String,
    pub compatible_versions: DefinitionVersionRange,
    pub target_node: String,
    pub operation: PatchOperation,
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub node: Option<UiNode>,
    #[serde(default)]
    pub props: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UserPreferenceKey {
    Collapsed,
    Density,
    Sort,
    ViewMode,
}

impl UserPreferenceKey {
    fn property(self) -> &'static str {
        match self {
            Self::Collapsed => "collapsed",
            Self::Density => "density",
            Self::Sort => "sort",
            Self::ViewMode => "viewMode",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPreference {
    pub target_node: String,
    pub key: UserPreferenceKey,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedPage {
    pub page_id: String,
    pub definition_version: u32,
    pub selected_variant: Option<String>,
    pub source_versions: BTreeMap<String, String>,
    pub publication_versions: Vec<u32>,
    pub resource: Option<String>,
    pub data_sources: Vec<DataSourceDefinition>,
    pub actions: Vec<ActionDefinition>,
    pub root: UiNode,
    pub content_hash: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CatalogueError {
    #[error("plugin {plugin} owns document {document} as {actual_owner}")]
    WrongOwner {
        plugin: String,
        document: String,
        actual_owner: String,
    },
    #[error("plugin {0} is already installed")]
    DuplicatePlugin(String),
    #[error("plugin {0} is not installed")]
    UnknownPlugin(String),
    #[error("component {component} at node {node} in {document} is not registered")]
    UnknownComponent {
        document: String,
        node: String,
        component: String,
    },
    #[error(transparent)]
    InvalidBundle(#[from] UiContractError),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("page {0} is not available from an enabled plugin")]
    PageUnavailable(String),
    #[error("page {page} does not support host {host:?}")]
    UnsupportedHost { page: String, host: HostProfile },
    #[error("enabled plugin {plugin} is missing enabled dependency {dependency}")]
    MissingDependency { plugin: String, dependency: String },
    #[error("plugin dependency cycle involving {0}")]
    DependencyCycle(String),
    #[error("contribution {contribution} targets incompatible page version {actual}")]
    IncompatibleVersion { contribution: String, actual: u32 },
    #[error("contribution {0} has an invalid compatible version range")]
    InvalidVersionRange(String),
    #[error("contribution {contribution} targets missing anchor {target} on page {page}")]
    MissingAnchor {
        contribution: String,
        page: String,
        target: String,
    },
    #[error("contribution {contribution} has an invalid payload for {operation:?}")]
    InvalidPatch {
        contribution: String,
        operation: PatchOperation,
    },
    #[error("restricted replacement {contribution} must preserve target node id {target}")]
    UnsafeReplacement {
        contribution: String,
        target: String,
    },
    #[error("move {contribution} would place {target} inside its own subtree at {destination}")]
    MoveCycle {
        contribution: String,
        target: String,
        destination: String,
    },
    #[error("duplicate node id {0} after composition")]
    DuplicateNode(String),
    #[error("ambiguous writes to {target}.{property} by {first} and {second}")]
    AmbiguousWrite {
        target: String,
        property: String,
        first: String,
        second: String,
    },
    #[error("failed to serialize resolved page for hashing: {0}")]
    HashSerialization(String),
}

impl MetadataCatalogue {
    pub fn register_host_component(&mut self, component_id: impl Into<String>) {
        self.host_components.insert(component_id.into());
    }

    pub fn install(&mut self, package: PluginUiPackage) -> Result<(), CatalogueError> {
        if self.packages.contains_key(&package.plugin_id) {
            return Err(CatalogueError::DuplicatePlugin(package.plugin_id));
        }
        validate_ownership(&package)?;

        let plugin_id = package.plugin_id.clone();
        self.packages.insert(plugin_id.clone(), package);
        let bundle = self.combined_bundle();
        if let Err(error) = bundle.validate() {
            self.packages.remove(&plugin_id);
            return Err(error.into());
        }
        if let Err(error) = validate_component_references(&bundle, &self.host_components) {
            self.packages.remove(&plugin_id);
            return Err(error);
        }
        Ok(())
    }

    pub fn set_state(
        &mut self,
        plugin_id: &str,
        state: PluginUiState,
    ) -> Result<(), CatalogueError> {
        let package = self
            .packages
            .get_mut(plugin_id)
            .ok_or_else(|| CatalogueError::UnknownPlugin(plugin_id.into()))?;
        package.state = state;
        Ok(())
    }

    pub fn resolve(
        &self,
        page_id: &str,
        context: &ResolveContext,
        tenant_patches: &[PublishedPatch],
        user_preferences: &[UserPreference],
    ) -> Result<ResolvedPage, ResolveError> {
        let order = self.dependency_order()?;
        let (owner, page) = self
            .packages
            .values()
            .filter(|package| package.state == PluginUiState::Enabled)
            .find_map(|package| {
                package
                    .bundle
                    .pages
                    .iter()
                    .find(|page| page.id == page_id)
                    .map(|page| (package, page))
            })
            .ok_or_else(|| ResolveError::PageUnavailable(page_id.into()))?;

        if !page.hosts.contains(&context.host) {
            return Err(ResolveError::UnsupportedHost {
                page: page.id.clone(),
                host: context.host.clone(),
            });
        }

        let (mut root, selected_variant) = select_root(page, context);
        let mut source_plugins = BTreeSet::from([owner.plugin_id.clone()]);

        let rank: BTreeMap<_, _> = order
            .iter()
            .enumerate()
            .map(|(index, plugin)| (plugin.clone(), index))
            .collect();
        let mut plugin_patches = Vec::new();
        for package in self
            .packages
            .values()
            .filter(|package| package.state == PluginUiState::Enabled)
        {
            for contribution in package
                .bundle
                .extensions
                .iter()
                .filter(|contribution| contribution.target_page == page_id)
            {
                plugin_patches.push(RuntimePatch::from_extension(
                    contribution,
                    rank[&package.plugin_id],
                ));
                source_plugins.insert(package.plugin_id.clone());
            }
        }
        let mut pending: Vec<_> = source_plugins.iter().cloned().collect();
        while let Some(plugin_id) = pending.pop() {
            for dependency in &self.packages[&plugin_id].dependencies {
                if source_plugins.insert(dependency.clone()) {
                    pending.push(dependency.clone());
                }
            }
        }
        let source_versions = source_plugins
            .into_iter()
            .map(|plugin_id| {
                let version = self.packages[&plugin_id].plugin_version.clone();
                (plugin_id, version)
            })
            .collect();
        plugin_patches.sort_by(RuntimePatch::compare);

        let mut property_writers = BTreeMap::new();
        for patch in &plugin_patches {
            apply_patch(
                &mut root,
                page.definition_version,
                patch,
                &mut property_writers,
            )?;
        }

        let mut published: Vec<_> = tenant_patches
            .iter()
            .filter(|patch| patch.target_page == page_id)
            .map(RuntimePatch::from_published)
            .collect();
        published.sort_by(RuntimePatch::compare);
        property_writers.clear();
        for patch in &published {
            apply_patch(
                &mut root,
                page.definition_version,
                patch,
                &mut property_writers,
            )?;
        }

        let mut preferences = user_preferences.to_vec();
        preferences.sort_by(|left, right| {
            (&left.target_node, left.key).cmp(&(&right.target_node, right.key))
        });
        let mut preference_writers = BTreeSet::new();
        for preference in &preferences {
            let key = (preference.target_node.clone(), preference.key);
            if !preference_writers.insert(key) {
                return Err(ResolveError::AmbiguousWrite {
                    target: preference.target_node.clone(),
                    property: preference.key.property().into(),
                    first: "user-preferences".into(),
                    second: "user-preferences".into(),
                });
            }
            let target = find_mut(&mut root, &preference.target_node).ok_or_else(|| {
                ResolveError::MissingAnchor {
                    contribution: "user-preferences".into(),
                    page: page_id.into(),
                    target: preference.target_node.clone(),
                }
            })?;
            target
                .props
                .insert(preference.key.property().into(), preference.value.clone());
        }

        ensure_unique_nodes(&root)?;
        let publication_versions = tenant_patches
            .iter()
            .filter(|patch| patch.target_page == page_id)
            .map(|patch| patch.publication_version)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut resolved = ResolvedPage {
            page_id: page.id.clone(),
            definition_version: page.definition_version,
            selected_variant,
            source_versions,
            publication_versions,
            resource: page.resource.clone(),
            data_sources: page.data_sources.clone(),
            actions: page.actions.clone(),
            root,
            content_hash: String::new(),
        };
        resolved.content_hash = stable_hash(&resolved)?;
        Ok(resolved)
    }

    fn combined_bundle(&self) -> UiBundle {
        let mut bundle = UiBundle::default();
        for package in self.packages.values() {
            bundle.resources.extend(package.bundle.resources.clone());
            bundle.pages.extend(package.bundle.pages.clone());
            bundle.fragments.extend(package.bundle.fragments.clone());
            bundle.components.extend(package.bundle.components.clone());
            bundle.extensions.extend(package.bundle.extensions.clone());
        }
        bundle
    }

    fn dependency_order(&self) -> Result<Vec<String>, ResolveError> {
        let enabled: BTreeMap<_, _> = self
            .packages
            .iter()
            .filter(|(_, package)| package.state == PluginUiState::Enabled)
            .collect();
        let mut temporary = BTreeSet::new();
        let mut permanent = BTreeSet::new();
        let mut output = Vec::new();

        fn visit(
            plugin_id: &str,
            enabled: &BTreeMap<&String, &PluginUiPackage>,
            temporary: &mut BTreeSet<String>,
            permanent: &mut BTreeSet<String>,
            output: &mut Vec<String>,
        ) -> Result<(), ResolveError> {
            if permanent.contains(plugin_id) {
                return Ok(());
            }
            if !temporary.insert(plugin_id.into()) {
                return Err(ResolveError::DependencyCycle(plugin_id.into()));
            }
            let package = enabled[&plugin_id.to_string()];
            for dependency in &package.dependencies {
                if !enabled.contains_key(dependency) {
                    return Err(ResolveError::MissingDependency {
                        plugin: plugin_id.into(),
                        dependency: dependency.clone(),
                    });
                }
                visit(dependency, enabled, temporary, permanent, output)?;
            }
            temporary.remove(plugin_id);
            permanent.insert(plugin_id.into());
            output.push(plugin_id.into());
            Ok(())
        }

        for plugin_id in enabled.keys() {
            visit(
                plugin_id,
                &enabled,
                &mut temporary,
                &mut permanent,
                &mut output,
            )?;
        }
        Ok(output)
    }
}

fn validate_ownership(package: &PluginUiPackage) -> Result<(), CatalogueError> {
    let owners = package
        .bundle
        .resources
        .iter()
        .map(|item| (&item.id, &item.owner_plugin))
        .chain(
            package
                .bundle
                .pages
                .iter()
                .map(|item| (&item.id, &item.owner_plugin)),
        )
        .chain(
            package
                .bundle
                .fragments
                .iter()
                .map(|item| (&item.id, &item.owner_plugin)),
        )
        .chain(
            package
                .bundle
                .components
                .iter()
                .map(|item| (&item.id, &item.owner_plugin)),
        )
        .chain(
            package
                .bundle
                .extensions
                .iter()
                .map(|item| (&item.id, &item.owner_plugin)),
        );
    for (document, owner) in owners {
        if owner != &package.plugin_id {
            return Err(CatalogueError::WrongOwner {
                plugin: package.plugin_id.clone(),
                document: document.clone(),
                actual_owner: owner.clone(),
            });
        }
    }
    Ok(())
}

fn validate_component_references(
    bundle: &UiBundle,
    host_components: &BTreeSet<String>,
) -> Result<(), CatalogueError> {
    let known: BTreeSet<_> = host_components
        .iter()
        .cloned()
        .chain(
            bundle
                .components
                .iter()
                .map(|component| component.id.clone()),
        )
        .collect();
    fn visit(
        document: &str,
        node: &UiNode,
        known: &BTreeSet<String>,
    ) -> Result<(), CatalogueError> {
        if !known.contains(&node.component) {
            return Err(CatalogueError::UnknownComponent {
                document: document.into(),
                node: node.id.clone(),
                component: node.component.clone(),
            });
        }
        for child in &node.children {
            visit(document, child, known)?;
        }
        Ok(())
    }
    for page in &bundle.pages {
        visit(&page.id, &page.root, &known)?;
        for variant in &page.variants {
            visit(&page.id, &variant.root, &known)?;
        }
    }
    for fragment in &bundle.fragments {
        visit(&fragment.id, &fragment.root, &known)?;
    }
    for extension in &bundle.extensions {
        if let Some(node) = &extension.node {
            visit(&extension.id, node, &known)?;
        }
    }
    Ok(())
}

fn select_root(page: &PageDefinition, context: &ResolveContext) -> (UiNode, Option<String>) {
    let mut variants: Vec<_> = page
        .variants
        .iter()
        .filter(|variant| {
            variant
                .condition
                .as_ref()
                .is_none_or(|condition| evaluate_condition(condition, context))
        })
        .collect();
    variants.sort_by(|left, right| {
        right
            .priority
            .cmp(&left.priority)
            .then_with(|| left.id.cmp(&right.id))
    });
    variants.first().map_or_else(
        || (page.root.clone(), None),
        |variant| (variant.root.clone(), Some(variant.id.clone())),
    )
}

pub fn evaluate_condition(condition: &ConditionExpression, context: &ResolveContext) -> bool {
    match condition {
        ConditionExpression::And { args } => args
            .iter()
            .all(|condition| evaluate_condition(condition, context)),
        ConditionExpression::Or { args } => args
            .iter()
            .any(|condition| evaluate_condition(condition, context)),
        ConditionExpression::Not { arg } => !evaluate_condition(arg, context),
        ConditionExpression::Equal { left, right } => {
            evaluate_binding(left, context) == evaluate_binding(right, context)
        }
        ConditionExpression::NotEqual { left, right } => {
            evaluate_binding(left, context) != evaluate_binding(right, context)
        }
        ConditionExpression::Exists { value } => {
            evaluate_binding(value, context).is_some_and(|value| !value.is_null())
        }
        ConditionExpression::In { value, set } => {
            let value = evaluate_binding(value, context);
            set.iter()
                .any(|candidate| evaluate_binding(candidate, context) == value)
        }
        ConditionExpression::HasPermission { permission } => {
            context.permissions.contains(permission)
        }
    }
}

fn evaluate_binding(binding: &Binding, context: &ResolveContext) -> Option<Value> {
    match binding {
        Binding::Literal { value } => Some(value.clone()),
        Binding::Context { path } => match path.as_str() {
            "host" => Some(Value::String(host_name(&context.host).into())),
            "mode" => Some(Value::String(context.mode.clone())),
            "form-factor" => Some(Value::String(context.form_factor.clone())),
            "audience" => context.audience.clone().map(Value::String),
            "resource-subtype" => context.resource_subtype.clone().map(Value::String),
            "locale" => Some(Value::String(context.locale.clone())),
            path if path.starts_with("role.") => Some(Value::Bool(
                context.roles.contains(path.trim_start_matches("role.")),
            )),
            path => context.values.get(path).cloned(),
        },
        Binding::Field { field } => context.values.get(&format!("field.{field}")).cloned(),
        Binding::DataSource { data_source, path } => context
            .values
            .get(&format!("data-source.{data_source}.{path}"))
            .cloned(),
    }
}

fn host_name(host: &HostProfile) -> &'static str {
    match host {
        HostProfile::InternalWeb => "internal-web",
        HostProfile::Desktop => "desktop",
        HostProfile::Portal => "portal",
        HostProfile::PublicForm => "public-form",
    }
}

#[derive(Debug, Clone)]
struct RuntimePatch {
    id: String,
    order: usize,
    priority: i32,
    compatible_versions: DefinitionVersionRange,
    target_page: String,
    target_node: String,
    operation: PatchOperation,
    node: Option<UiNode>,
    props: BTreeMap<String, Value>,
}

impl RuntimePatch {
    fn from_extension(extension: &ExtensionContribution, order: usize) -> Self {
        Self {
            id: extension.id.clone(),
            order,
            priority: extension.priority,
            compatible_versions: DefinitionVersionRange::exact(extension.target_version),
            target_page: extension.target_page.clone(),
            target_node: extension.target_node.clone(),
            operation: extension.operation.clone(),
            node: extension.node.clone(),
            props: extension.props.clone(),
        }
    }

    fn from_published(patch: &PublishedPatch) -> Self {
        Self {
            id: patch.id.clone(),
            order: 0,
            priority: patch.priority,
            compatible_versions: patch.compatible_versions,
            target_page: patch.target_page.clone(),
            target_node: patch.target_node.clone(),
            operation: patch.operation.clone(),
            node: patch.node.clone(),
            props: patch.props.clone(),
        }
    }

    fn compare(left: &Self, right: &Self) -> std::cmp::Ordering {
        (left.order, left.priority, left.id.as_str()).cmp(&(
            right.order,
            right.priority,
            right.id.as_str(),
        ))
    }
}

fn apply_patch(
    root: &mut UiNode,
    page_version: u32,
    patch: &RuntimePatch,
    property_writers: &mut BTreeMap<(String, String), String>,
) -> Result<(), ResolveError> {
    if !patch.compatible_versions.is_valid() {
        return Err(ResolveError::InvalidVersionRange(patch.id.clone()));
    }
    if !patch.compatible_versions.contains(page_version) {
        return Err(ResolveError::IncompatibleVersion {
            contribution: patch.id.clone(),
            actual: page_version,
        });
    }

    match patch.operation {
        PatchOperation::Inside => {
            let node = required_node(patch)?;
            find_mut(root, &patch.target_node)
                .ok_or_else(|| missing_anchor(patch))?
                .children
                .push(node);
        }
        PatchOperation::Before | PatchOperation::After => {
            let node = required_node(patch)?;
            if !insert_sibling(root, &patch.target_node, node, &patch.operation) {
                return Err(missing_anchor(patch));
            }
        }
        PatchOperation::Configure => {
            if patch.props.is_empty() || patch.node.is_some() {
                return Err(invalid_patch(patch));
            }
            let target = find_mut(root, &patch.target_node).ok_or_else(|| missing_anchor(patch))?;
            for (property, value) in &patch.props {
                let key = (patch.target_node.clone(), property.clone());
                if let Some(first) = property_writers.insert(key, patch.id.clone()) {
                    return Err(ResolveError::AmbiguousWrite {
                        target: patch.target_node.clone(),
                        property: property.clone(),
                        first,
                        second: patch.id.clone(),
                    });
                }
                target.props.insert(property.clone(), value.clone());
            }
        }
        PatchOperation::Replace => {
            let node = required_node(patch)?;
            if node.id != patch.target_node || root.id == patch.target_node {
                return Err(ResolveError::UnsafeReplacement {
                    contribution: patch.id.clone(),
                    target: patch.target_node.clone(),
                });
            }
            if !replace_child(root, &patch.target_node, node) {
                return Err(missing_anchor(patch));
            }
        }
        PatchOperation::Move => {
            let destination = patch
                .props
                .get("destination")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid_patch(patch))?;
            if root.id == patch.target_node {
                return Err(invalid_patch(patch));
            }
            let moving =
                remove_child(root, &patch.target_node).ok_or_else(|| missing_anchor(patch))?;
            if contains_node(&moving, destination) {
                return Err(ResolveError::MoveCycle {
                    contribution: patch.id.clone(),
                    target: patch.target_node.clone(),
                    destination: destination.into(),
                });
            }
            find_mut(root, destination)
                .ok_or_else(|| ResolveError::MissingAnchor {
                    contribution: patch.id.clone(),
                    page: patch.target_page.clone(),
                    target: destination.into(),
                })?
                .children
                .push(moving);
        }
    }
    ensure_unique_nodes(root)
}

fn required_node(patch: &RuntimePatch) -> Result<UiNode, ResolveError> {
    if !patch.props.is_empty() {
        return Err(invalid_patch(patch));
    }
    patch.node.clone().ok_or_else(|| invalid_patch(patch))
}

fn invalid_patch(patch: &RuntimePatch) -> ResolveError {
    ResolveError::InvalidPatch {
        contribution: patch.id.clone(),
        operation: patch.operation.clone(),
    }
}

fn missing_anchor(patch: &RuntimePatch) -> ResolveError {
    ResolveError::MissingAnchor {
        contribution: patch.id.clone(),
        page: patch.target_page.clone(),
        target: patch.target_node.clone(),
    }
}

fn find_mut<'a>(node: &'a mut UiNode, id: &str) -> Option<&'a mut UiNode> {
    if node.id == id {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| find_mut(child, id))
}

fn contains_node(node: &UiNode, id: &str) -> bool {
    node.id == id || node.children.iter().any(|child| contains_node(child, id))
}

fn insert_sibling(
    root: &mut UiNode,
    target: &str,
    node: UiNode,
    operation: &PatchOperation,
) -> bool {
    if let Some(index) = root.children.iter().position(|child| child.id == target) {
        let offset = usize::from(matches!(operation, PatchOperation::After));
        root.children.insert(index + offset, node);
        return true;
    }
    root.children
        .iter_mut()
        .any(|child| insert_sibling(child, target, node.clone(), operation))
}

fn replace_child(root: &mut UiNode, target: &str, node: UiNode) -> bool {
    if let Some(index) = root.children.iter().position(|child| child.id == target) {
        root.children[index] = node;
        return true;
    }
    root.children
        .iter_mut()
        .any(|child| replace_child(child, target, node.clone()))
}

fn remove_child(root: &mut UiNode, target: &str) -> Option<UiNode> {
    if let Some(index) = root.children.iter().position(|child| child.id == target) {
        return Some(root.children.remove(index));
    }
    root.children
        .iter_mut()
        .find_map(|child| remove_child(child, target))
}

fn ensure_unique_nodes(root: &UiNode) -> Result<(), ResolveError> {
    fn visit(node: &UiNode, ids: &mut BTreeSet<String>) -> Result<(), ResolveError> {
        if !ids.insert(node.id.clone()) {
            return Err(ResolveError::DuplicateNode(node.id.clone()));
        }
        for child in &node.children {
            visit(child, ids)?;
        }
        Ok(())
    }
    visit(root, &mut BTreeSet::new())
}

fn stable_hash(resolved: &ResolvedPage) -> Result<String, ResolveError> {
    let bytes = serde_json::to_vec(resolved)
        .map_err(|error| ResolveError::HashSerialization(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_contract::ui::{PageKind, PageVariant, UI_CONTRACT_VERSION};
    use serde_json::json;

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

    fn page() -> PageDefinition {
        PageDefinition {
            contract_version: UI_CONTRACT_VERSION,
            id: "platform.sample.record".into(),
            definition_version: 3,
            owner_plugin: "platform.sample".into(),
            kind: PageKind::Record,
            hosts: vec![HostProfile::InternalWeb],
            resource: None,
            data_sources: Vec::new(),
            actions: Vec::new(),
            variants: vec![PageVariant {
                id: "platform.sample.record.compact".into(),
                priority: 10,
                condition: Some(ConditionExpression::Equal {
                    left: Binding::Context {
                        path: "form-factor".into(),
                    },
                    right: Binding::Literal {
                        value: json!("compact"),
                    },
                }),
                root: node(
                    "page-root",
                    "platform.ui.page",
                    vec![
                        node("main", "platform.ui.slot", Vec::new()),
                        node("sidebar", "platform.ui.slot", Vec::new()),
                    ],
                ),
            }],
            root: node(
                "page-root",
                "platform.ui.page",
                vec![
                    node("main", "platform.ui.slot", Vec::new()),
                    node("sidebar", "platform.ui.slot", Vec::new()),
                ],
            ),
        }
    }

    fn extension(id: &str, owner: &str, target: &str, child: &str) -> ExtensionContribution {
        ExtensionContribution {
            contract_version: UI_CONTRACT_VERSION,
            id: id.into(),
            owner_plugin: owner.into(),
            target_page: "platform.sample.record".into(),
            target_version: 3,
            target_node: target.into(),
            operation: PatchOperation::Inside,
            priority: 0,
            node: Some(node(child, "platform.ui.card", Vec::new())),
            props: BTreeMap::new(),
        }
    }

    fn package(
        id: &str,
        dependencies: &[&str],
        pages: Vec<PageDefinition>,
        extensions: Vec<ExtensionContribution>,
    ) -> PluginUiPackage {
        PluginUiPackage {
            plugin_id: id.into(),
            plugin_version: "1.0.0".into(),
            dependencies: dependencies.iter().map(|item| (*item).into()).collect(),
            state: PluginUiState::Enabled,
            bundle: UiBundle {
                pages,
                extensions,
                ..UiBundle::default()
            },
        }
    }

    fn context() -> ResolveContext {
        ResolveContext {
            host: HostProfile::InternalWeb,
            mode: "view".into(),
            form_factor: "wide".into(),
            audience: Some("internal".into()),
            roles: BTreeSet::from(["admin".into()]),
            resource_subtype: None,
            locale: "en-GB".into(),
            permissions: BTreeSet::new(),
            values: BTreeMap::new(),
        }
    }

    #[test]
    fn approved_condition_subset_matches_client_truth_table() {
        let mut context = context();
        context.permissions.insert("records.edit".into());
        context
            .values
            .insert("field.status".into(), Value::String("active".into()));
        context.values.insert(
            "data-source.account.tier".into(),
            Value::String("priority".into()),
        );
        let cases = [
            (
                ConditionExpression::Equal {
                    left: Binding::Field {
                        field: "status".into(),
                    },
                    right: Binding::Literal {
                        value: Value::String("active".into()),
                    },
                },
                true,
            ),
            (
                ConditionExpression::In {
                    value: Binding::DataSource {
                        data_source: "account".into(),
                        path: "tier".into(),
                    },
                    set: vec![Binding::Literal {
                        value: Value::String("priority".into()),
                    }],
                },
                true,
            ),
            (
                ConditionExpression::And {
                    args: vec![
                        ConditionExpression::HasPermission {
                            permission: "records.edit".into(),
                        },
                        ConditionExpression::Not {
                            arg: Box::new(ConditionExpression::Exists {
                                value: Binding::Field {
                                    field: "missing".into(),
                                },
                            }),
                        },
                    ],
                },
                true,
            ),
        ];
        for (condition, expected) in cases {
            assert_eq!(evaluate_condition(&condition, &context), expected);
        }
    }

    fn catalogue(reverse: bool) -> MetadataCatalogue {
        let base = package("platform.sample", &[], vec![page()], Vec::new());
        let first = package(
            "customer.first",
            &["platform.sample"],
            Vec::new(),
            vec![extension(
                "customer.first.card",
                "customer.first",
                "main",
                "first-card",
            )],
        );
        let second = package(
            "customer.second",
            &["customer.first"],
            Vec::new(),
            vec![extension(
                "customer.second.card",
                "customer.second",
                "sidebar",
                "second-card",
            )],
        );
        let mut catalogue = MetadataCatalogue::default();
        for component in [
            "platform.ui.page",
            "platform.ui.slot",
            "platform.ui.card",
            "platform.ui.summary-card",
        ] {
            catalogue.register_host_component(component);
        }
        let packages = if reverse {
            vec![base, second, first]
        } else {
            vec![base, first, second]
        };
        for package in packages {
            catalogue.install(package).unwrap();
        }
        catalogue
    }

    #[test]
    fn resolution_is_stable_and_dependency_ordered() {
        let normal = catalogue(false)
            .resolve("platform.sample.record", &context(), &[], &[])
            .unwrap();
        let reverse = catalogue(true)
            .resolve("platform.sample.record", &context(), &[], &[])
            .unwrap();
        assert_eq!(normal, reverse);
        assert_eq!(normal.content_hash.len(), 64);
        assert!(contains_node(&normal.root, "first-card"));
        assert!(contains_node(&normal.root, "second-card"));
    }

    #[test]
    fn resolved_output_matches_the_golden_snapshot() {
        let mut resolved = catalogue(false)
            .resolve("platform.sample.record", &context(), &[], &[])
            .unwrap();
        resolved.content_hash.clear();
        let expected: ResolvedPage =
            serde_json::from_str(include_str!("../fixtures/golden-resolved-page.json")).unwrap();
        assert_eq!(resolved, expected);
    }

    #[test]
    fn catalogue_rejects_unregistered_components() {
        let mut catalogue = MetadataCatalogue::default();
        assert!(matches!(
            catalogue.install(package(
                "platform.sample",
                &[],
                vec![page()],
                Vec::new()
            )),
            Err(CatalogueError::UnknownComponent { component, .. })
                if component == "platform.ui.page"
        ));
    }

    #[test]
    fn variants_use_safe_context_conditions() {
        let mut compact = context();
        compact.form_factor = "compact".into();
        let resolved = catalogue(false)
            .resolve("platform.sample.record", &compact, &[], &[])
            .unwrap();
        assert_eq!(
            resolved.selected_variant.as_deref(),
            Some("platform.sample.record.compact")
        );
        assert!(contains_node(&resolved.root, "main"));
    }

    #[test]
    fn tenant_patches_compose_and_survive_compatible_revision() {
        let patches = [PublishedPatch {
            id: "tenant.layout.sidebar".into(),
            publication_version: 7,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange {
                min_inclusive: 2,
                max_exclusive: Some(5),
            },
            target_node: "sidebar".into(),
            operation: PatchOperation::Inside,
            priority: 0,
            node: Some(node("tenant-card", "platform.ui.card", Vec::new())),
            props: BTreeMap::new(),
        }];
        let resolved = catalogue(false)
            .resolve("platform.sample.record", &context(), &patches, &[])
            .unwrap();
        assert!(contains_node(&resolved.root, "tenant-card"));
        assert_eq!(resolved.publication_versions, [7]);
    }

    #[test]
    fn tenant_and_preference_input_order_does_not_change_the_snapshot() {
        let patch = |id: &str, target: &str, child: &str, publication_version| PublishedPatch {
            id: id.into(),
            publication_version,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange::exact(3),
            target_node: target.into(),
            operation: PatchOperation::Inside,
            priority: 0,
            node: Some(node(child, "platform.ui.card", Vec::new())),
            props: BTreeMap::new(),
        };
        let patches = vec![
            patch("tenant.z", "sidebar", "tenant-z", 11),
            patch("tenant.a", "main", "tenant-a", 10),
        ];
        let preferences = vec![
            UserPreference {
                target_node: "sidebar".into(),
                key: UserPreferenceKey::Collapsed,
                value: json!(true),
            },
            UserPreference {
                target_node: "main".into(),
                key: UserPreferenceKey::Density,
                value: json!("compact"),
            },
        ];
        let mut reversed_patches = patches.clone();
        reversed_patches.reverse();
        let mut reversed_preferences = preferences.clone();
        reversed_preferences.reverse();

        let catalogue = catalogue(false);
        let first = catalogue
            .resolve("platform.sample.record", &context(), &patches, &preferences)
            .unwrap();
        let second = catalogue
            .resolve(
                "platform.sample.record",
                &context(),
                &reversed_patches,
                &reversed_preferences,
            )
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.publication_versions, [10, 11]);
    }

    #[test]
    fn before_after_and_incompatible_versions_are_checked() {
        let sibling = |id: &str, operation, priority| PublishedPatch {
            id: id.into(),
            publication_version: 12,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange::exact(3),
            target_node: "main".into(),
            operation,
            priority,
            node: Some(node(id, "platform.ui.card", Vec::new())),
            props: BTreeMap::new(),
        };
        let catalogue = catalogue(false);
        let resolved = catalogue
            .resolve(
                "platform.sample.record",
                &context(),
                &[
                    sibling("before-main", PatchOperation::Before, 0),
                    sibling("after-main", PatchOperation::After, 1),
                ],
                &[],
            )
            .unwrap();
        let ids: Vec<_> = resolved
            .root
            .children
            .iter()
            .map(|node| node.id.as_str())
            .collect();
        assert_eq!(ids, ["before-main", "main", "after-main", "sidebar"]);

        let incompatible = PublishedPatch {
            compatible_versions: DefinitionVersionRange {
                min_inclusive: 1,
                max_exclusive: Some(3),
            },
            ..sibling("old-layout", PatchOperation::Inside, 0)
        };
        assert!(matches!(
            catalogue.resolve("platform.sample.record", &context(), &[incompatible], &[]),
            Err(ResolveError::IncompatibleVersion { actual: 3, .. })
        ));

        let malformed = PublishedPatch {
            compatible_versions: DefinitionVersionRange {
                min_inclusive: 3,
                max_exclusive: Some(3),
            },
            ..sibling("malformed-range", PatchOperation::Inside, 0)
        };
        assert!(matches!(
            catalogue.resolve(
                "platform.sample.record",
                &context(),
                &[malformed],
                &[]
            ),
            Err(ResolveError::InvalidVersionRange(id)) if id == "malformed-range"
        ));
    }

    #[test]
    fn ambiguous_writes_fail_without_mutating_a_prior_snapshot() {
        let catalogue = catalogue(false);
        let stable = catalogue
            .resolve("platform.sample.record", &context(), &[], &[])
            .unwrap();
        let configure = |id: &str| PublishedPatch {
            id: id.into(),
            publication_version: 8,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange::exact(3),
            target_node: "main".into(),
            operation: PatchOperation::Configure,
            priority: 0,
            node: None,
            props: BTreeMap::from([("title".into(), json!(id))]),
        };
        assert!(matches!(
            catalogue.resolve(
                "platform.sample.record",
                &context(),
                &[configure("tenant.a"), configure("tenant.b")],
                &[]
            ),
            Err(ResolveError::AmbiguousWrite { .. })
        ));
        assert_eq!(
            stable,
            catalogue
                .resolve("platform.sample.record", &context(), &[], &[])
                .unwrap()
        );
    }

    #[test]
    fn missing_upgrade_anchor_is_actionable_and_rollback_input_still_resolves() {
        let catalogue = catalogue(false);
        let valid = PublishedPatch {
            id: "tenant.valid".into(),
            publication_version: 4,
            target_page: "platform.sample.record".into(),
            compatible_versions: DefinitionVersionRange::exact(3),
            target_node: "main".into(),
            operation: PatchOperation::Inside,
            priority: 0,
            node: Some(node("valid-card", "platform.ui.card", Vec::new())),
            props: BTreeMap::new(),
        };
        let mut broken = valid.clone();
        broken.id = "tenant.broken".into();
        broken.publication_version = 5;
        broken.target_node = "removed-anchor".into();
        assert!(matches!(
            catalogue.resolve("platform.sample.record", &context(), &[broken], &[]),
            Err(ResolveError::MissingAnchor { target, .. }) if target == "removed-anchor"
        ));
        assert!(catalogue
            .resolve("platform.sample.record", &context(), &[valid], &[])
            .is_ok());
    }

    #[test]
    fn disabling_plugins_removes_owned_pages_and_contributions() {
        let mut catalogue = catalogue(false);
        catalogue
            .set_state("customer.first", PluginUiState::Disabled)
            .unwrap();
        assert!(matches!(
            catalogue.resolve("platform.sample.record", &context(), &[], &[]),
            Err(ResolveError::MissingDependency { plugin, dependency })
                if plugin == "customer.second" && dependency == "customer.first"
        ));
        catalogue
            .set_state("customer.second", PluginUiState::Disabled)
            .unwrap();
        let resolved = catalogue
            .resolve("platform.sample.record", &context(), &[], &[])
            .unwrap();
        assert!(!contains_node(&resolved.root, "first-card"));

        catalogue
            .set_state("platform.sample", PluginUiState::Disabled)
            .unwrap();
        assert!(matches!(
            catalogue.resolve("platform.sample.record", &context(), &[], &[]),
            Err(ResolveError::PageUnavailable(_))
        ));
    }

    #[test]
    fn dependency_cycles_are_structured_errors() {
        let mut catalogue = catalogue(false);
        catalogue
            .packages
            .get_mut("platform.sample")
            .unwrap()
            .dependencies
            .insert("customer.second".into());
        assert!(matches!(
            catalogue.resolve("platform.sample.record", &context(), &[], &[]),
            Err(ResolveError::DependencyCycle(_))
        ));
    }

    #[test]
    fn move_replace_and_user_preferences_are_bounded() {
        let catalogue = catalogue(false);
        let patches = [
            PublishedPatch {
                id: "tenant.move".into(),
                publication_version: 9,
                target_page: "platform.sample.record".into(),
                compatible_versions: DefinitionVersionRange::exact(3),
                target_node: "first-card".into(),
                operation: PatchOperation::Move,
                priority: 0,
                node: None,
                props: BTreeMap::from([("destination".into(), json!("sidebar"))]),
            },
            PublishedPatch {
                id: "tenant.replace".into(),
                publication_version: 9,
                target_page: "platform.sample.record".into(),
                compatible_versions: DefinitionVersionRange::exact(3),
                target_node: "second-card".into(),
                operation: PatchOperation::Replace,
                priority: 1,
                node: Some(node("second-card", "platform.ui.summary-card", Vec::new())),
                props: BTreeMap::new(),
            },
        ];
        let preferences = [UserPreference {
            target_node: "sidebar".into(),
            key: UserPreferenceKey::Collapsed,
            value: json!(true),
        }];
        let resolved = catalogue
            .resolve("platform.sample.record", &context(), &patches, &preferences)
            .unwrap();
        let sidebar = resolved
            .root
            .children
            .iter()
            .find(|node| node.id == "sidebar")
            .unwrap();
        assert_eq!(sidebar.props.get("collapsed"), Some(&json!(true)));
        assert!(contains_node(sidebar, "first-card"));
    }
}
