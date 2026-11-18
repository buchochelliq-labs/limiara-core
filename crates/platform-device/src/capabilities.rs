//! Local-only runtime capabilities (ADR 0024 §6, ADR 0034 §9): what a
//! native plugin may reach of the shell's own services. A plugin declares
//! them; the shell grants them when it builds the plugin, and hands it a
//! handle only for what it declared. Nothing is ambient: the change feed
//! and the route context have no other way in, and the web view has none
//! at all.
//!
//! These are not user permissions, which the engine checks for the member,
//! and they grant nothing in the workspace: a plugin holding one still
//! reads records only through the engine as the member. None of them needs
//! an OS permission.

use crate::assistant::confirm::{AssistantClock, SystemClock};
use crate::assistant::{AssistantBridge, AssistantService};
use crate::feed::{ChangeSubscription, ReplicaFeed};
use crate::route::{RouteContext, RouteSubscription};
use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

/// One local-only capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LocalCapability {
    /// `device.replica_changes`: subscribe to the replica change feed.
    ReplicaChanges,
    /// `device.route_context`: read the route the web shell shows.
    RouteContext,
    /// `device.assistant`: call the assistant bridge (ADR 0034), which
    /// consumes the feed and the route context itself.
    Assistant,
}

impl LocalCapability {
    pub const ALL: [Self; 3] = [Self::ReplicaChanges, Self::RouteContext, Self::Assistant];

    /// The capability's stable name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReplicaChanges => "device.replica_changes",
            Self::RouteContext => "device.route_context",
            Self::Assistant => "device.assistant",
        }
    }
}

impl fmt::Display for LocalCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for LocalCapability {
    type Err = CapabilityError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|capability| capability.as_str() == name)
            .ok_or_else(|| CapabilityError::Unknown(name.to_owned()))
    }
}

/// Why a declaration or a handle was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CapabilityError {
    /// The declaration names a capability the shell does not offer.
    #[error("`{0}` is not a local capability")]
    Unknown(String),
    /// The declaration names no plugin.
    #[error("a plugin declaration names its plugin")]
    PluginUnnamed,
    /// The plugin asked for a handle it did not declare.
    #[error("the plugin `{plugin}` did not declare `{capability}`")]
    NotGranted {
        plugin: String,
        capability: LocalCapability,
    },
}

impl CapabilityError {
    /// The stable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unknown(_) => "device.capability_unknown",
            Self::PluginUnnamed => "device.capability_invalid",
            Self::NotGranted { .. } => "device.capability_not_granted",
        }
    }
}

/// What a native plugin declares: its name and the local capabilities it
/// uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDeclaration {
    plugin: String,
    capabilities: BTreeSet<LocalCapability>,
}

impl PluginDeclaration {
    /// A declaration from the plugin's own list of capability names. An
    /// unknown name refuses the whole declaration, so a typo never leaves a
    /// plugin quietly without what it needs, and nothing new is granted by
    /// spelling.
    pub fn new(plugin: &str, capabilities: &[&str]) -> Result<Self, CapabilityError> {
        if plugin.trim().is_empty() {
            return Err(CapabilityError::PluginUnnamed);
        }
        Ok(Self {
            plugin: plugin.to_owned(),
            capabilities: capabilities
                .iter()
                .map(|name| name.parse())
                .collect::<Result<_, _>>()?,
        })
    }

    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    pub fn capabilities(&self) -> &BTreeSet<LocalCapability> {
        &self.capabilities
    }
}

/// The shell's services that local capabilities reach. The controller
/// owns them for the app's lifetime.
#[derive(Debug, Clone)]
pub struct LocalServices {
    feed: ReplicaFeed,
    routes: RouteContext,
    assistant: AssistantService,
}

impl Default for LocalServices {
    fn default() -> Self {
        Self::new(ReplicaFeed::default(), RouteContext::default())
    }
}

impl LocalServices {
    /// Services with no assistant: the desktop's.
    pub fn new(feed: ReplicaFeed, routes: RouteContext) -> Self {
        Self::with_assistant(feed, routes, None, std::sync::Arc::new(SystemClock))
    }

    /// Services on a platform with `assistant` (Siri on iOS, Gemini on
    /// Android): the assistant bridge accepts calls naming only it.
    /// `clock` times confirmations; the shell passes [`SystemClock`].
    pub fn with_assistant(
        feed: ReplicaFeed,
        routes: RouteContext,
        assistant: Option<platform_objects::Assistant>,
        clock: std::sync::Arc<dyn AssistantClock>,
    ) -> Self {
        let assistant = AssistantService::new(feed.clone(), routes.clone(), assistant, clock);
        Self {
            feed,
            routes,
            assistant,
        }
    }

    pub fn feed(&self) -> &ReplicaFeed {
        &self.feed
    }

    pub fn routes(&self) -> &RouteContext {
        &self.routes
    }

    /// The shell's side of the assistant bridge: opened and closed with
    /// the workspace, with the member's device setting.
    pub fn assistant(&self) -> &AssistantService {
        &self.assistant
    }

    /// The handles a plugin gets: exactly what it declared.
    pub fn grant(&self, declaration: &PluginDeclaration) -> PluginGrant {
        let declared = |capability| declaration.capabilities.contains(&capability);
        PluginGrant {
            plugin: declaration.plugin.clone(),
            feed: declared(LocalCapability::ReplicaChanges).then(|| self.feed.clone()),
            routes: declared(LocalCapability::RouteContext).then(|| self.routes.clone()),
            assistant: declared(LocalCapability::Assistant)
                .then(|| self.assistant.bridge_for(&declaration.plugin)),
        }
    }
}

/// What the shell granted one plugin. Each subscription is checked
/// against the declaration before it is handed out.
#[derive(Debug, Clone)]
pub struct PluginGrant {
    plugin: String,
    feed: Option<ReplicaFeed>,
    routes: Option<RouteContext>,
    assistant: Option<AssistantBridge>,
}

impl PluginGrant {
    pub fn plugin(&self) -> &str {
        &self.plugin
    }

    /// Whether the plugin holds `capability`.
    pub fn holds(&self, capability: LocalCapability) -> bool {
        match capability {
            LocalCapability::ReplicaChanges => self.feed.is_some(),
            LocalCapability::RouteContext => self.routes.is_some(),
            LocalCapability::Assistant => self.assistant.is_some(),
        }
    }

    /// The assistant bridge (`device.assistant`).
    pub fn assistant(&self) -> Result<AssistantBridge, CapabilityError> {
        self.assistant
            .clone()
            .ok_or_else(|| self.refused(LocalCapability::Assistant))
    }

    /// Subscribes to the replica change feed (`device.replica_changes`).
    pub fn replica_changes(&self) -> Result<ChangeSubscription, CapabilityError> {
        self.feed
            .as_ref()
            .map(ReplicaFeed::subscribe)
            .ok_or_else(|| self.refused(LocalCapability::ReplicaChanges))
    }

    /// Subscribes to the route context (`device.route_context`).
    pub fn route_context(&self) -> Result<RouteSubscription, CapabilityError> {
        self.routes
            .as_ref()
            .map(RouteContext::subscribe)
            .ok_or_else(|| self.refused(LocalCapability::RouteContext))
    }

    fn refused(&self, capability: LocalCapability) -> CapabilityError {
        CapabilityError::NotGranted {
            plugin: self.plugin.clone(),
            capability,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_have_stable_names() {
        for capability in LocalCapability::ALL {
            assert_eq!(capability.as_str().parse(), Ok(capability));
        }
        assert_eq!(
            LocalCapability::ReplicaChanges.as_str(),
            "device.replica_changes"
        );
        assert_eq!(
            LocalCapability::RouteContext.as_str(),
            "device.route_context"
        );
    }

    #[test]
    fn a_declaration_names_only_known_capabilities() {
        let declared = PluginDeclaration::new("device-route-probe", &["device.route_context"])
            .expect("declaration");
        assert_eq!(declared.plugin(), "device-route-probe");
        assert_eq!(
            declared.capabilities(),
            &BTreeSet::from([LocalCapability::RouteContext])
        );
        for unknown in ["device.camera", "assistant", "replica_changes", ""] {
            let error = PluginDeclaration::new("probe", &[unknown]).expect_err(unknown);
            assert_eq!(error.code(), "device.capability_unknown");
        }
        assert_eq!(
            PluginDeclaration::new(" ", &[]).map_err(|error| error.code()),
            Err("device.capability_invalid")
        );
    }

    #[test]
    fn a_plugin_gets_only_what_it_declared() {
        let services = LocalServices::default();
        let routes_only = services.grant(
            &PluginDeclaration::new("probe", &["device.route_context"]).expect("declaration"),
        );
        assert!(routes_only.route_context().is_ok());
        assert!(routes_only.holds(LocalCapability::RouteContext));
        assert!(!routes_only.holds(LocalCapability::ReplicaChanges));
        let refused = routes_only.replica_changes().expect_err("not declared");
        assert_eq!(refused.code(), "device.capability_not_granted");
        assert_eq!(
            refused,
            CapabilityError::NotGranted {
                plugin: "probe".into(),
                capability: LocalCapability::ReplicaChanges
            }
        );

        let nothing = services.grant(&PluginDeclaration::new("bare", &[]).expect("declaration"));
        assert!(nothing.replica_changes().is_err());
        assert!(nothing.route_context().is_err());
        assert_eq!(
            nothing
                .assistant()
                .map(|_| ())
                .map_err(|error| error.code()),
            Err("device.capability_not_granted")
        );
        let assistant = services
            .grant(&PluginDeclaration::new("siri", &["device.assistant"]).expect("declaration"));
        assert_eq!(
            assistant
                .assistant()
                .map(|bridge| bridge.plugin().to_owned()),
            Ok("siri".into())
        );
        assert!(assistant.replica_changes().is_err());

        let both = services.grant(
            &PluginDeclaration::new("index", &["device.replica_changes", "device.route_context"])
                .expect("declaration"),
        );
        assert!(both.replica_changes().is_ok());
        assert!(both.route_context().is_ok());
        assert!(!both.holds(LocalCapability::Assistant));
    }
}
