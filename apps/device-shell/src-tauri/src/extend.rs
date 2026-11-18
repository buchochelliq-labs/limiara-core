//! Native plugins an app links beyond the shell's own (ADR 0024 §6,
//! ADR 0034 §9).
//!
//! The shell's own plugins (`device-keystore`, `device-browser`,
//! `device-push` and, in debug builds, `device-route-probe`) are named in
//! [`crate::app`]. An app crate built on the shell can link more, without
//! the shell naming them: it builds the app with [`crate::app::AppBuilder`]
//! and adds each one as an [`ExtraPlugin`]:
//!
//! - the Tauri plugin itself, registered before the app starts, like the
//!   shell's own;
//! - its [`PluginDeclaration`]: the local capabilities it uses;
//! - a start function, called once the controller exists and has reopened
//!   what the device holds, with the [`PluginGrant`] the controller builds
//!   from that declaration.
//!
//! Nothing here widens what a plugin reaches. The grant holds exactly what
//! the declaration names, as for the shell's own plugins: an unknown
//! capability refuses the declaration, and a handle the plugin did not
//! declare is refused (`device.capability_not_granted`). A plugin holding a
//! capability still reads records only through the engine as the member.
//! The web view reaches none of it.

use platform_device::capabilities::{PluginDeclaration, PluginGrant};
use std::collections::BTreeSet;

/// The names of the shell's own plugins, which an extra plugin may not
/// take.
pub const SHELL_PLUGINS: &[&str] = &[
    device_keystore::NAME,
    device_browser::NAME,
    device_push::NAME,
    device_route_probe::NAME,
];

/// Why an extra plugin was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    /// The declaration names another plugin than the Tauri plugin it
    /// comes with, so its grant could not be told apart.
    NameMismatch { declared: String, plugin: String },
    /// One of the shell's own plugins has the name.
    Reserved(String),
    /// Another extra plugin has the name.
    Duplicate(String),
}

impl std::fmt::Display for ExtensionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NameMismatch { declared, plugin } => write!(
                formatter,
                "the declaration names `{declared}`, but the plugin is `{plugin}`"
            ),
            Self::Reserved(name) => write!(formatter, "`{name}` is one of the shell's own plugins"),
            Self::Duplicate(name) => write!(formatter, "the plugin `{name}` is added twice"),
        }
    }
}

impl std::error::Error for ExtensionError {}

impl ExtensionError {
    /// The stable error code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NameMismatch { .. } => "device.plugin_name_mismatch",
            Self::Reserved(_) => "device.plugin_reserved",
            Self::Duplicate(_) => "device.plugin_duplicate",
        }
    }
}

/// What a plugin's start function reports when it cannot start. The shell
/// logs it and goes on without the plugin.
pub type StartError = String;

/// A start function: called once, with the app and the plugin's grant.
pub type Start<A> = Box<dyn FnOnce(&A, PluginGrant) -> Result<(), StartError> + Send>;

/// A native plugin an app adds to the shell. `A` is what the start
/// function gets: the Tauri app in the shell, anything in tests.
pub struct ExtraPlugin<A> {
    declaration: PluginDeclaration,
    plugin: Box<dyn tauri::plugin::Plugin<tauri::Wry>>,
    start: Start<A>,
}

impl<A> std::fmt::Debug for ExtraPlugin<A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtraPlugin")
            .field("declaration", &self.declaration)
            .finish_non_exhaustive()
    }
}

impl<A> ExtraPlugin<A> {
    /// A plugin with its declaration and start function. The declaration
    /// must name the Tauri plugin, which must not be one of the shell's.
    pub fn new<P>(
        declaration: PluginDeclaration,
        plugin: P,
        start: impl FnOnce(&A, PluginGrant) -> Result<(), StartError> + Send + 'static,
    ) -> Result<Self, ExtensionError>
    where
        P: tauri::plugin::Plugin<tauri::Wry> + 'static,
    {
        let name = plugin.name();
        if declaration.plugin() != name {
            return Err(ExtensionError::NameMismatch {
                declared: declaration.plugin().to_owned(),
                plugin: name.to_owned(),
            });
        }
        if SHELL_PLUGINS.contains(&name) {
            return Err(ExtensionError::Reserved(name.to_owned()));
        }
        Ok(Self {
            declaration,
            plugin: Box::new(plugin),
            start: Box::new(start),
        })
    }

    pub fn declaration(&self) -> &PluginDeclaration {
        &self.declaration
    }

    pub fn name(&self) -> &str {
        self.declaration.plugin()
    }
}

/// The extra plugins of one app, each name once.
pub struct ExtraPlugins<A> {
    plugins: Vec<ExtraPlugin<A>>,
}

impl<A> Default for ExtraPlugins<A> {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
        }
    }
}

impl<A> std::fmt::Debug for ExtraPlugins<A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_list().entries(&self.plugins).finish()
    }
}

/// The plugins' start functions, once their Tauri plugins are registered.
pub struct PendingStarts<A> {
    starts: Vec<(PluginDeclaration, Start<A>)>,
}

impl<A> std::fmt::Debug for PendingStarts<A> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_list()
            .entries(self.starts.iter().map(|(declaration, _)| declaration))
            .finish()
    }
}

impl<A> ExtraPlugins<A> {
    /// Adds a plugin; a second plugin of the same name is refused.
    pub fn add(&mut self, plugin: ExtraPlugin<A>) -> Result<(), ExtensionError> {
        if self
            .plugins
            .iter()
            .any(|added| added.name() == plugin.name())
        {
            return Err(ExtensionError::Duplicate(plugin.name().to_owned()));
        }
        self.plugins.push(plugin);
        Ok(())
    }

    /// The plugins' names, in the order they were added.
    pub fn names(&self) -> Vec<&str> {
        self.plugins.iter().map(ExtraPlugin::name).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Hands each Tauri plugin to `register`, in the order they were
    /// added, and keeps their start functions for [`PendingStarts::start`].
    pub fn register(
        self,
        mut register: impl FnMut(Box<dyn tauri::plugin::Plugin<tauri::Wry>>),
    ) -> PendingStarts<A> {
        let mut starts = Vec::with_capacity(self.plugins.len());
        for plugin in self.plugins {
            register(plugin.plugin);
            starts.push((plugin.declaration, plugin.start));
        }
        PendingStarts { starts }
    }
}

impl<A> PendingStarts<A> {
    /// Starts every plugin with the grant `grant` builds from its
    /// declaration, in the order they were added. A plugin that cannot
    /// start does not stop the others; each outcome is returned by name.
    pub fn start(
        self,
        app: &A,
        grant: impl Fn(&PluginDeclaration) -> PluginGrant,
    ) -> Vec<(String, Result<(), StartError>)> {
        self.starts
            .into_iter()
            .map(|(declaration, start)| {
                let granted = grant(&declaration);
                (declaration.plugin().to_owned(), start(app, granted))
            })
            .collect()
    }

    /// The declared capabilities by plugin, for the shell's log.
    pub fn declared(&self) -> Vec<(String, BTreeSet<String>)> {
        self.starts
            .iter()
            .map(|(declaration, _)| {
                (
                    declaration.plugin().to_owned(),
                    declaration
                        .capabilities()
                        .iter()
                        .map(|capability| capability.as_str().to_owned())
                        .collect(),
                )
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform_device::capabilities::{LocalCapability, LocalServices};
    use std::sync::{Arc, Mutex};

    /// A plugin's name and the capabilities its grant held.
    type Held = (String, Vec<LocalCapability>);

    fn tauri_plugin(name: &'static str) -> tauri::plugin::TauriPlugin<tauri::Wry> {
        tauri::plugin::Builder::new(name).build()
    }

    fn declared(name: &str, capabilities: &[&str]) -> PluginDeclaration {
        PluginDeclaration::new(name, capabilities).expect("declaration")
    }

    #[test]
    fn a_declaration_names_its_own_plugin() {
        let refused = ExtraPlugin::<()>::new(
            declared("device-other", &[]),
            tauri_plugin("device-extra"),
            |_, _| Ok(()),
        )
        .expect_err("another name");
        assert_eq!(refused.code(), "device.plugin_name_mismatch");
        assert!(ExtraPlugin::<()>::new(
            declared("device-extra", &[]),
            tauri_plugin("device-extra"),
            |_, _| Ok(())
        )
        .is_ok());
    }

    #[test]
    fn the_shells_own_names_are_reserved() {
        for name in SHELL_PLUGINS {
            let refused =
                ExtraPlugin::<()>::new(declared(name, &[]), tauri_plugin(name), |_, _| Ok(()))
                    .expect_err(name);
            assert_eq!(refused.code(), "device.plugin_reserved");
        }
    }

    #[test]
    fn each_plugin_is_added_once() {
        let mut plugins = ExtraPlugins::<()>::default();
        let plugin = || {
            ExtraPlugin::new(
                declared("device-extra", &[]),
                tauri_plugin("device-extra"),
                |_, _| Ok(()),
            )
            .expect("plugin")
        };
        plugins.add(plugin()).expect("first");
        assert_eq!(
            plugins.add(plugin()).map_err(|error| error.code()),
            Err("device.plugin_duplicate")
        );
        assert_eq!(plugins.names(), vec!["device-extra"]);
    }

    /// Every plugin is registered with Tauri, then started with exactly
    /// the grant its declaration earns, in order; one that fails to start
    /// leaves the others running.
    #[test]
    fn plugins_start_with_exactly_what_they_declared() {
        let services = LocalServices::default();
        let seen: Arc<Mutex<Vec<Held>>> = Arc::default();
        let mut plugins = ExtraPlugins::<&'static str>::default();
        for (name, capabilities, fails) in [
            (
                "device-first",
                &["device.assistant", "device.route_context"][..],
                false,
            ),
            ("device-second", &[][..], true),
            ("device-third", &["device.replica_changes"][..], false),
        ] {
            let seen = seen.clone();
            plugins
                .add(
                    ExtraPlugin::new(
                        declared(name, capabilities),
                        tauri_plugin(name),
                        move |app: &&'static str, grant: PluginGrant| {
                            assert_eq!(*app, "app");
                            let held = LocalCapability::ALL
                                .into_iter()
                                .filter(|capability| grant.holds(*capability))
                                .collect();
                            seen.lock()
                                .expect("lock")
                                .push((grant.plugin().to_owned(), held));
                            if fails {
                                Err("not today".into())
                            } else {
                                Ok(())
                            }
                        },
                    )
                    .expect("plugin"),
                )
                .expect("added");
        }

        let mut registered = Vec::new();
        let pending = plugins.register(|plugin| registered.push(plugin.name()));
        assert_eq!(
            registered,
            vec!["device-first", "device-second", "device-third"]
        );
        assert_eq!(
            pending.declared()[0],
            (
                "device-first".to_owned(),
                BTreeSet::from([
                    "device.assistant".to_owned(),
                    "device.route_context".to_owned()
                ])
            )
        );

        let outcomes = pending.start(&"app", |declaration| services.grant(declaration));
        assert_eq!(
            outcomes,
            vec![
                ("device-first".to_owned(), Ok(())),
                ("device-second".to_owned(), Err("not today".to_owned())),
                ("device-third".to_owned(), Ok(())),
            ]
        );
        assert_eq!(
            *seen.lock().expect("lock"),
            vec![
                (
                    "device-first".to_owned(),
                    vec![LocalCapability::RouteContext, LocalCapability::Assistant]
                ),
                ("device-second".to_owned(), vec![]),
                (
                    "device-third".to_owned(),
                    vec![LocalCapability::ReplicaChanges]
                ),
            ]
        );
    }

    /// A grant from the shell refuses what the plugin did not declare,
    /// however it was added.
    #[test]
    fn an_extra_plugin_cannot_reach_what_it_did_not_declare() {
        let services = LocalServices::default();
        let mut plugins = ExtraPlugins::<()>::default();
        plugins
            .add(
                ExtraPlugin::new(
                    declared("device-extra", &["device.route_context"]),
                    tauri_plugin("device-extra"),
                    |_, grant| {
                        assert!(grant.route_context().is_ok());
                        let refused = grant.assistant().map(|_| ()).map_err(|error| error.code());
                        assert_eq!(refused, Err("device.capability_not_granted"));
                        assert!(grant.replica_changes().is_err());
                        Ok(())
                    },
                )
                .expect("plugin"),
            )
            .expect("added");
        let outcomes = plugins
            .register(|_| {})
            .start(&(), |declaration| services.grant(declaration));
        assert_eq!(outcomes, vec![("device-extra".to_owned(), Ok(()))]);
    }
}
