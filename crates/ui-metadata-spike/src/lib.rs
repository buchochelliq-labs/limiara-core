#![forbid(unsafe_code)]

//! Disposable executable proof for ADR 0005 and issue #112.
//!
//! The production contracts belong in `plugin-contract` after the spike is
//! accepted. This crate exists to make the proposed resolution and filtering
//! rules testable before that public API is committed.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostProfile {
    InternalWeb,
    Desktop,
    Portal,
    PublicForm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub component: String,
    #[serde(default)]
    pub props: BTreeMap<String, Value>,
    #[serde(default)]
    pub children: Vec<Node>,
    #[serde(default)]
    pub permission: Option<String>,
    #[serde(default)]
    pub hosts: Vec<HostProfile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PageDefinition {
    pub contract_version: u16,
    pub id: String,
    pub definition_version: u32,
    pub owner_plugin: String,
    pub root: Node,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PatchOperation {
    Inside,
    Before,
    After,
    Configure,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Contribution {
    pub plugin_id: String,
    /// Dependency/topological rank supplied by the plugin registry.
    pub dependency_order: u32,
    #[serde(default)]
    pub priority: i32,
    pub target_page: String,
    pub target_version: u32,
    pub target_node: String,
    pub operation: PatchOperation,
    #[serde(default)]
    pub node: Option<Node>,
    #[serde(default)]
    pub props: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveContext {
    pub host: HostProfile,
    pub permissions: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResolvedPage {
    pub id: String,
    pub definition_version: u32,
    pub sources: Vec<String>,
    pub root: Node,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ResolveError {
    #[error("unsupported UI contract version {0}")]
    ContractVersion(u16),
    #[error("contribution from {plugin} targets page {actual}, expected {expected}")]
    WrongPage {
        plugin: String,
        actual: String,
        expected: String,
    },
    #[error("contribution from {plugin} targets page version {actual}, expected {expected}")]
    WrongVersion {
        plugin: String,
        actual: u32,
        expected: u32,
    },
    #[error("target node {target} was not found for contribution from {plugin}")]
    MissingTarget { plugin: String, target: String },
    #[error("contribution from {plugin} requires a node for {operation:?}")]
    MissingNode {
        plugin: String,
        operation: PatchOperation,
    },
    #[error("duplicate node id {0}")]
    DuplicateNode(String),
    #[error("ambiguous writes to {target}.{property} by {first} and {second}")]
    AmbiguousWrite {
        target: String,
        property: String,
        first: String,
        second: String,
    },
    #[error("component {component} at node {node} is unavailable on host {host:?}")]
    UnsupportedHost {
        node: String,
        component: String,
        host: HostProfile,
    },
}

pub fn resolve_page(
    page: &PageDefinition,
    contributions: &[Contribution],
    context: &ResolveContext,
) -> Result<ResolvedPage, ResolveError> {
    if page.contract_version != 1 {
        return Err(ResolveError::ContractVersion(page.contract_version));
    }
    ensure_unique_ids(&page.root)?;

    let mut root = page.root.clone();
    let mut ordered = contributions.to_vec();
    ordered.sort_by(|left, right| {
        (
            left.dependency_order,
            left.priority,
            left.plugin_id.as_str(),
            left.target_node.as_str(),
        )
            .cmp(&(
                right.dependency_order,
                right.priority,
                right.plugin_id.as_str(),
                right.target_node.as_str(),
            ))
    });

    let mut property_writers: BTreeMap<(String, String), String> = BTreeMap::new();
    let mut sources = vec![page.owner_plugin.clone()];
    for contribution in ordered {
        validate_target(page, &contribution)?;
        apply_contribution(&mut root, &contribution, &mut property_writers)?;
        if !sources.contains(&contribution.plugin_id) {
            sources.push(contribution.plugin_id);
        }
    }

    ensure_unique_ids(&root)?;
    validate_host(&root, &context.host)?;
    let root = filter_permissions(root, &context.permissions).unwrap_or_else(|| Node {
        id: "empty-page".into(),
        component: "empty".into(),
        props: BTreeMap::new(),
        children: Vec::new(),
        permission: None,
        hosts: vec![context.host.clone()],
    });

    Ok(ResolvedPage {
        id: page.id.clone(),
        definition_version: page.definition_version,
        sources,
        root,
    })
}

fn validate_target(page: &PageDefinition, contribution: &Contribution) -> Result<(), ResolveError> {
    if contribution.target_page != page.id {
        return Err(ResolveError::WrongPage {
            plugin: contribution.plugin_id.clone(),
            actual: contribution.target_page.clone(),
            expected: page.id.clone(),
        });
    }
    if contribution.target_version != page.definition_version {
        return Err(ResolveError::WrongVersion {
            plugin: contribution.plugin_id.clone(),
            actual: contribution.target_version,
            expected: page.definition_version,
        });
    }
    Ok(())
}

fn apply_contribution(
    root: &mut Node,
    contribution: &Contribution,
    property_writers: &mut BTreeMap<(String, String), String>,
) -> Result<(), ResolveError> {
    match contribution.operation {
        PatchOperation::Configure => {
            let target = find_mut(root, &contribution.target_node).ok_or_else(|| {
                ResolveError::MissingTarget {
                    plugin: contribution.plugin_id.clone(),
                    target: contribution.target_node.clone(),
                }
            })?;
            for (property, value) in &contribution.props {
                let key = (contribution.target_node.clone(), property.clone());
                if let Some(first) = property_writers.insert(key, contribution.plugin_id.clone()) {
                    return Err(ResolveError::AmbiguousWrite {
                        target: contribution.target_node.clone(),
                        property: property.clone(),
                        first,
                        second: contribution.plugin_id.clone(),
                    });
                }
                target.props.insert(property.clone(), value.clone());
            }
            Ok(())
        }
        PatchOperation::Inside => {
            let node = required_node(contribution)?;
            let target = find_mut(root, &contribution.target_node).ok_or_else(|| {
                ResolveError::MissingTarget {
                    plugin: contribution.plugin_id.clone(),
                    target: contribution.target_node.clone(),
                }
            })?;
            target.children.push(node);
            Ok(())
        }
        PatchOperation::Before | PatchOperation::After => {
            let node = required_node(contribution)?;
            insert_sibling(
                root,
                &contribution.target_node,
                node,
                &contribution.operation,
            )
            .then_some(())
            .ok_or_else(|| ResolveError::MissingTarget {
                plugin: contribution.plugin_id.clone(),
                target: contribution.target_node.clone(),
            })
        }
    }
}

fn required_node(contribution: &Contribution) -> Result<Node, ResolveError> {
    contribution
        .node
        .clone()
        .ok_or_else(|| ResolveError::MissingNode {
            plugin: contribution.plugin_id.clone(),
            operation: contribution.operation.clone(),
        })
}

fn find_mut<'a>(node: &'a mut Node, id: &str) -> Option<&'a mut Node> {
    if node.id == id {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| find_mut(child, id))
}

fn insert_sibling(root: &mut Node, target: &str, node: Node, operation: &PatchOperation) -> bool {
    if let Some(index) = root.children.iter().position(|child| child.id == target) {
        let offset = usize::from(matches!(operation, PatchOperation::After));
        root.children.insert(index + offset, node);
        return true;
    }
    root.children
        .iter_mut()
        .any(|child| insert_sibling(child, target, node.clone(), operation))
}

fn ensure_unique_ids(root: &Node) -> Result<(), ResolveError> {
    fn visit(node: &Node, ids: &mut BTreeSet<String>) -> Result<(), ResolveError> {
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

fn validate_host(node: &Node, host: &HostProfile) -> Result<(), ResolveError> {
    if !node.hosts.is_empty() && !node.hosts.contains(host) {
        return Err(ResolveError::UnsupportedHost {
            node: node.id.clone(),
            component: node.component.clone(),
            host: host.clone(),
        });
    }
    for child in &node.children {
        validate_host(child, host)?;
    }
    Ok(())
}

fn filter_permissions(mut node: Node, permissions: &BTreeSet<String>) -> Option<Node> {
    if node
        .permission
        .as_ref()
        .is_some_and(|permission| !permissions.contains(permission))
    {
        return None;
    }
    node.children = node
        .children
        .into_iter()
        .filter_map(|child| filter_permissions(child, permissions))
        .collect();
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize)]
    struct PrototypeBundle {
        pages: Vec<PageDefinition>,
        contributions: Vec<Contribution>,
    }

    fn node(id: &str, component: &str) -> Node {
        Node {
            id: id.into(),
            component: component.into(),
            props: BTreeMap::new(),
            children: Vec::new(),
            permission: None,
            hosts: vec![HostProfile::InternalWeb],
        }
    }

    fn base_page() -> PageDefinition {
        let mut root = node("person-page", "page");
        root.children.push(node("main", "section"));
        root.children.push(node("sidebar", "slot"));
        PageDefinition {
            contract_version: 1,
            id: "platform.contacts.person.record".into(),
            definition_version: 3,
            owner_plugin: "platform.contacts".into(),
            root,
        }
    }

    fn context() -> ResolveContext {
        ResolveContext {
            host: HostProfile::InternalWeb,
            permissions: BTreeSet::from(["contacts.person.read-private".into()]),
        }
    }

    #[test]
    fn contributions_resolve_deterministically_and_filter_permissions() {
        let mut private = node("private-card", "card");
        private.permission = Some("contacts.person.read-private".into());
        let public = node("activity-card", "card");
        let contributions = vec![
            Contribution {
                plugin_id: "customer.activity".into(),
                dependency_order: 2,
                priority: 0,
                target_page: base_page().id,
                target_version: 3,
                target_node: "sidebar".into(),
                operation: PatchOperation::Inside,
                node: Some(public),
                props: BTreeMap::new(),
            },
            Contribution {
                plugin_id: "customer.private".into(),
                dependency_order: 1,
                priority: 0,
                target_page: "platform.contacts.person.record".into(),
                target_version: 3,
                target_node: "main".into(),
                operation: PatchOperation::Inside,
                node: Some(private),
                props: BTreeMap::new(),
            },
        ];

        let resolved = resolve_page(&base_page(), &contributions, &context()).unwrap();
        assert_eq!(
            resolved.sources,
            ["platform.contacts", "customer.private", "customer.activity"]
        );
        assert!(find(&resolved.root, "private-card"));

        let public_context = ResolveContext {
            permissions: BTreeSet::new(),
            ..context()
        };
        let filtered = resolve_page(&base_page(), &contributions, &public_context).unwrap();
        assert!(!find(&filtered.root, "private-card"));
        assert!(find(&filtered.root, "activity-card"));
    }

    #[test]
    fn ambiguous_property_writes_fail_with_both_plugin_ids() {
        let contribution = |plugin: &str| Contribution {
            plugin_id: plugin.into(),
            dependency_order: 1,
            priority: 0,
            target_page: base_page().id,
            target_version: 3,
            target_node: "main".into(),
            operation: PatchOperation::Configure,
            node: None,
            props: BTreeMap::from([("title".into(), json!(plugin))]),
        };
        let error = resolve_page(
            &base_page(),
            &[contribution("customer.a"), contribution("customer.b")],
            &context(),
        )
        .unwrap_err();
        assert_eq!(
            error,
            ResolveError::AmbiguousWrite {
                target: "main".into(),
                property: "title".into(),
                first: "customer.a".into(),
                second: "customer.b".into(),
            }
        );
    }

    #[test]
    fn incompatible_base_revision_reports_the_expected_version() {
        let contribution = Contribution {
            plugin_id: "tenant.patch".into(),
            dependency_order: 10,
            priority: 0,
            target_page: base_page().id,
            target_version: 2,
            target_node: "main".into(),
            operation: PatchOperation::Inside,
            node: Some(node("custom-field", "field")),
            props: BTreeMap::new(),
        };
        assert_eq!(
            resolve_page(&base_page(), &[contribution], &context()).unwrap_err(),
            ResolveError::WrongVersion {
                plugin: "tenant.patch".into(),
                actual: 2,
                expected: 3,
            }
        );
    }

    #[test]
    fn public_host_rejects_an_internal_component_before_rendering() {
        let context = ResolveContext {
            host: HostProfile::PublicForm,
            permissions: BTreeSet::new(),
        };
        let error = resolve_page(&base_page(), &[], &context).unwrap_err();
        assert!(matches!(error, ResolveError::UnsupportedHost { .. }));
    }

    #[test]
    fn published_prototype_bundle_is_executable_by_the_resolver() {
        let bundle: PrototypeBundle = serde_json::from_str(include_str!(
            "../../../docs/architecture/examples/metadata-ui/prototype-bundle.json"
        ))
        .expect("documented prototype bundle must match the executable spike types");
        let page = bundle
            .pages
            .iter()
            .find(|page| page.id == "platform.contacts.person.record")
            .expect("record page fixture");

        let resolved = resolve_page(page, &bundle.contributions, &context())
            .expect("documented contribution must resolve");
        assert!(find(&resolved.root, "activity-insights-card"));
    }

    fn find(node: &Node, id: &str) -> bool {
        node.id == id || node.children.iter().any(|child| find(child, id))
    }
}
