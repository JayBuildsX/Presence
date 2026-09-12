//! PresenceHub Plugin Host
//!
//! The Plugin Host owns registered plugins and coordinates their
//! lifecycle. It does not discover, load, or source plugins — those
//! responsibilities belong to future phases.
//!
//! # Responsibilities
//!
//! - Defining the [`Plugin`] trait
//! - Owning registered plugin instances
//! - Coordinating lifecycle (init, shutdown)
//! - Providing immutable access to registered plugins
//!
//! # Non-Responsibilities
//!
//! - Plugin discovery
//! - Filesystem scanning
//! - Dynamic library loading
//! - IPC or process management
//! - Sandboxing or crash recovery

use presencehub_core::activity::Activity;
use tracing::{debug, error, info};

// ---------------------------------------------------------------------------
// Plugin Metadata
// ---------------------------------------------------------------------------

/// Lightweight metadata describing a plugin.
///
/// Only fields with a real consumer today are included.
///
/// # Fields
///
/// * `name` — Human-readable plugin name (e.g. "FL Studio", "VS Code").
///   Used for identification, logging, and error reporting.
///
/// * `version` — Plugin version string (e.g. "1.0.0").
///   Used for future compatibility checks between plugins and the host.
#[derive(Debug, Clone, PartialEq)]
pub struct PluginMetadata {
    /// Human-readable plugin name.
    pub name: String,

    /// Plugin version string.
    pub version: String,
}

impl PluginMetadata {
    /// Creates a new `PluginMetadata` with the given name and version.
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            version: version.into(),
        }
    }
}

// ---------------------------------------------------------------------------
// Window Identity
// ---------------------------------------------------------------------------

/// Declared window identity for a plugin.
///
/// Lets generic foreground ownership map the OS foreground window to a
/// registered plugin source without the engine knowing anything about a
/// concrete application. A plugin declares the window class names and/or
/// process base names it presents; the ownership layer matches the
/// foreground window against these declarations.
///
/// Matching is always case-insensitive. Either vector may be empty — an
/// empty vector simply never matches.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WindowIdentity {
    /// Process base-name patterns (e.g. `"fl64.exe"`, `"antigravity.exe"`).
    pub process_names: Vec<String>,
    /// Window class-name patterns (e.g. `"TFruityLoopsMainForm"`).
    pub window_classes: Vec<String>,
}

impl WindowIdentity {
    /// Creates a new identity from process-name and window-class patterns.
    pub fn new(
        process_names: impl IntoIterator<Item = impl Into<String>>,
        window_classes: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            process_names: process_names.into_iter().map(Into::into).collect(),
            window_classes: window_classes.into_iter().map(Into::into).collect(),
        }
    }
}

// ---------------------------------------------------------------------------
// Plugin Error
// ---------------------------------------------------------------------------

/// Errors that can occur during plugin lifecycle operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    /// Returned when [`Plugin::init`] fails.
    #[error("Plugin initialization failed: {0}")]
    InitFailed(String),

    /// Returned when [`Plugin::shutdown`] fails.
    #[error("Plugin shutdown failed: {0}")]
    ShutdownFailed(String),

    /// Returned when [`Plugin::poll`] fails.
    #[error("Plugin poll failed: {0}")]
    PollFailed(String),
}

// ---------------------------------------------------------------------------
// Plugin Trait
// ---------------------------------------------------------------------------

/// A single application integration.
///
/// Each plugin represents one application that PresenceHub can monitor.
/// Examples include FL Studio, VS Code, and Spotify.
///
/// The trait is intentionally minimal. Every method has a clear
/// justification:
///
/// * [`metadata`](Plugin::metadata) — Provides identification. Without it,
///   the host cannot distinguish one plugin from another.
///
/// * [`init`](Plugin::init) — Prepares the plugin for operation.
///   A plugin must be able to initialize itself when the runtime starts.
///
/// * [`shutdown`](Plugin::shutdown) — Cleans up plugin resources.
///   A plugin must be able to release resources when the runtime stops.
///
/// * [`poll`](Plugin::poll) — Observes the application and produces
///   an Activity when its state has changed. Polling belongs to plugins
///   because only a plugin knows how to inspect its application.
///
/// # Object Safety
///
/// This trait is object-safe, allowing plugins to be stored as
/// `Box<dyn Plugin>`. This enables heterogeneous plugin storage
/// without generics.
///
/// # Thread Safety
///
/// Plugins must implement `Send + Sync` so that the host can
/// safely pass plugin references across threads in the future.
pub trait Plugin: Send + Sync {
    /// Returns the plugin's metadata.
    ///
    /// Metadata is immutable after construction. Returning a shared
    /// reference avoids unnecessary allocation or cloning.
    fn metadata(&self) -> &PluginMetadata;

    /// Returns the plugin's declared window identity, if any.
    ///
    /// This is used by the generic foreground-ownership layer to map the OS
    /// foreground window back to a registered plugin source. Plugins that
    /// do not expose a window (or that cannot be identified by window)
    /// return `None` (the default), in which case they are excluded from
    /// foreground ownership selection.
    fn window_identity(&self) -> Option<WindowIdentity> {
        None
    }

    /// Initializes the plugin.
    ///
    /// Called when the runtime starts. The plugin should prepare
    /// any resources needed for operation.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::InitFailed`] if initialization fails.
    /// The host will collect the error and continue with other plugins.
    fn init(&mut self) -> Result<(), PluginError>;

    /// Polls the plugin's application state.
    ///
    /// Called periodically by the runtime. Returns an [`Activity`]
    /// when the application state has changed, or `None` when the
    /// state is unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::PollFailed`] if the plugin cannot
    /// determine the application state. One plugin failing must
    /// never prevent other plugins from being polled.
    fn poll(&mut self) -> Result<Option<Activity>, PluginError>;

    /// Shuts down the plugin.
    ///
    /// Called when the runtime stops. The plugin should release
    /// any resources acquired during initialization or operation.
    ///
    /// # Errors
    ///
    /// Returns [`PluginError::ShutdownFailed`] if shutdown fails.
    /// The host will collect the error and continue with other plugins.
    fn shutdown(&mut self) -> Result<(), PluginError>;

    /// Resets the plugin's cached state and change detection.
    ///
    /// The default implementation calls [`shutdown`](Plugin::shutdown) followed by
    /// [`init`](Plugin::init). Plugins can override this to clear cached
    /// activities so the next poll immediately re-emits application state.
    fn reset(&mut self) {
        let _ = self.shutdown();
        let _ = self.init();
    }
}

// ---------------------------------------------------------------------------
// Plugin Host
// ---------------------------------------------------------------------------

/// Owns registered plugins and coordinates their lifecycle.
///
/// The Plugin Host is a runtime container. It does not discover,
/// load, or source plugins — those responsibilities belong to
/// future phases. Plugins are registered explicitly via
/// [`register`](PluginHost::register).
///
/// # Lifecycle
///
/// ```text
/// new() ──► register() ──► init_all() ──► (plugins running) ──► shutdown_all()
/// ```
///
/// # Fault Isolation
///
/// The host collects errors from individual plugins rather than
/// failing fast. This ensures that one plugin's failure does not
/// prevent other plugins from initializing or shutting down.
///
/// # Example
///
/// ```rust
/// use presencehub_plugin_host::{PluginHost, Plugin, PluginMetadata, PluginError};
///
/// struct MyPlugin {
///     metadata: PluginMetadata,
/// }
///
/// impl Plugin for MyPlugin {
///     fn metadata(&self) -> &PluginMetadata {
///         &self.metadata
///     }
///     fn init(&mut self) -> Result<(), PluginError> { Ok(()) }
///     fn poll(&mut self) -> Result<Option<presencehub_core::activity::Activity>, PluginError> { Ok(None) }
///     fn shutdown(&mut self) -> Result<(), PluginError> { Ok(()) }
/// }
///
/// let mut host = PluginHost::new();
/// host.register(Box::new(MyPlugin {
///     metadata: PluginMetadata::new("My Plugin", "1.0.0"),
/// }));
/// let errors = host.init_all();
/// assert!(errors.is_empty());
/// ```
pub struct PluginHost {
    /// The registered plugins, owned by the host.
    plugins: Vec<Box<dyn Plugin>>,
    /// Sources disabled at runtime (e.g. through the desktop GUI). Disabled
    /// plugins stay registered but are never polled.
    disabled: std::collections::HashSet<String>,
}

impl PluginHost {
    /// Creates a new, empty `PluginHost`.
    ///
    /// No plugins are registered. Use [`register`](PluginHost::register)
    /// to add plugins.
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            disabled: std::collections::HashSet::new(),
        }
    }

    /// Registers a plugin with the host.
    ///
    /// The plugin is owned by the host. It will be initialized when
    /// [`init_all`](PluginHost::init_all) is called and shut down when
    /// [`shutdown_all`](PluginHost::shutdown_all) is called.
    ///
    /// Registration does not call [`Plugin::init`]. Initialization is
    /// deferred until [`init_all`](PluginHost::init_all) is invoked.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) {
        let name = plugin.metadata().name.clone();
        self.plugins.push(plugin);
        info!(plugin = %name, "Plugin registered");
    }

    /// Removes a registered plugin by name, shutting it down if active.
    pub fn remove_plugin(&mut self, name: &str) {
        if let Some(pos) = self.plugins.iter().position(|p| p.metadata().name == name) {
            let mut plugin = self.plugins.remove(pos);
            let _ = plugin.shutdown();
            info!(plugin = %name, "Plugin unregistered");
        }
        self.disabled.remove(name);
    }

    /// Returns an immutable slice of all registered plugins.
    ///
    /// This allows the runtime to iterate over plugins without
    /// taking ownership or modifying them.
    pub fn plugins(&self) -> &[Box<dyn Plugin>] {
        &self.plugins
    }

    /// Returns a mutable slice of all registered plugins.
    pub fn plugins_mut(&mut self) -> &mut [Box<dyn Plugin>] {
        &mut self.plugins
    }

    /// Disables or re-enables a registered source.
    ///
    /// A disabled source stays registered but is skipped by
    /// [`poll_all`](PluginHost::poll_all): its `poll` is never called.
    /// Used by the desktop GUI toggle; the runtime additionally ends the
    /// source's engine session so the display falls back immediately.
    pub fn set_source_disabled(&mut self, source: &str, disabled: bool) {
        if disabled {
            if self.disabled.insert(source.to_string()) {
                if let Some(plugin) = self
                    .plugins
                    .iter_mut()
                    .find(|p| p.metadata().name == source)
                {
                    let _ = plugin.shutdown();
                }
            }
        } else if self.disabled.remove(source) {
            if let Some(plugin) = self
                .plugins
                .iter_mut()
                .find(|p| p.metadata().name == source)
            {
                plugin.reset();
            }
        }
    }

    /// Whether the given source is currently disabled.
    pub fn is_source_disabled(&self, source: &str) -> bool {
        self.disabled.contains(source)
    }

    /// Initializes all registered plugins.
    ///
    /// Calls [`Plugin::init`] on each plugin. Errors from individual
    /// plugins are collected and returned. The host always attempts
    /// to initialize every plugin, even if some fail.
    ///
    /// Returns a vector of `(plugin_name, error)` tuples for each
    /// plugin that failed to initialize. An empty vector means all
    /// plugins initialized successfully.
    pub fn init_all(&mut self) -> Vec<(String, PluginError)> {
        let mut errors = Vec::new();

        for plugin in self.plugins.iter_mut() {
            let name = plugin.metadata().name.clone();
            match plugin.init() {
                Ok(()) => {
                    info!(plugin = %name, "Plugin initialized");
                }
                Err(e) => {
                    error!(plugin = %name, error = %e, "Plugin initialization failed");
                    errors.push((name, e));
                }
            }
        }

        errors
    }

    /// Shuts down all registered plugins.
    ///
    /// Calls [`Plugin::shutdown`] on each plugin. Errors from
    /// individual plugins are collected and returned. The host
    /// always attempts to shut down every plugin, even if some fail.
    ///
    /// Returns a vector of `(plugin_name, error)` tuples for each
    /// plugin that failed to shut down. An empty vector means all
    /// plugins shut down successfully.
    pub fn shutdown_all(&mut self) -> Vec<(String, PluginError)> {
        let mut errors = Vec::new();

        for plugin in self.plugins.iter_mut() {
            let name = plugin.metadata().name.clone();
            match plugin.shutdown() {
                Ok(()) => {
                    info!(plugin = %name, "Plugin shut down");
                }
                Err(e) => {
                    error!(plugin = %name, error = %e, "Plugin shutdown failed");
                    errors.push((name, e));
                }
            }
        }

        errors
    }

    /// Polls all registered plugins in registration order.
    ///
    /// Calls [`Plugin::poll`] on each plugin. A failing plugin does
    /// not prevent other plugins from being polled. Sources disabled via
    /// [`set_source_disabled`](PluginHost::set_source_disabled) are
    /// skipped entirely: their `poll` is never called and they produce no
    /// result entry.
    ///
    /// Returns a vector of `(source, result)` pairs in registration
    /// order. `source` is the plugin's metadata name and identifies
    /// which plugin produced the result, so callers can route
    /// per-plugin sessions:
    /// - `Ok(Some(activity))` — the plugin produced a new activity
    /// - `Ok(None)` — the plugin's state is unchanged
    /// - `Err(error)` — the plugin failed to poll
    pub fn poll_all(&mut self) -> Vec<(String, Result<Option<Activity>, PluginError>)> {
        let mut results = Vec::new();
        for plugin in self.plugins.iter_mut() {
            let name = plugin.metadata().name.clone();
            if self.disabled.contains(&name) {
                debug!(plugin = %name, "PluginHost skipping disabled plugin");
                continue;
            }
            debug!(plugin = %name, "PluginHost polling plugin");
            let result = match plugin.poll() {
                Ok(Some(activity)) => {
                    debug!(
                        plugin = %name,
                        state = %activity.state,
                        details = ?activity.details,
                        metadata = ?activity.metadata,
                        "Plugin returned Ok(Some(Activity))"
                    );
                    (name, Ok(Some(activity)))
                }
                Ok(None) => {
                    debug!(plugin = %name, "Plugin returned Ok(None)");
                    (name, Ok(None))
                }
                Err(e) => {
                    debug!(plugin = %name, error = %e, "Plugin returned Err");
                    (name, Err(e))
                }
            };
            results.push(result);
        }
        results
    }
}

impl Default for PluginHost {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Mock plugins ----------------------------------------------------------

    /// A plugin that succeeds in all lifecycle operations.
    struct OkPlugin {
        metadata: PluginMetadata,
        init_called: bool,
        shutdown_called: bool,
    }

    impl OkPlugin {
        fn new(name: &str, version: &str) -> Self {
            Self {
                metadata: PluginMetadata::new(name, version),
                init_called: false,
                shutdown_called: false,
            }
        }
    }

    impl Plugin for OkPlugin {
        fn metadata(&self) -> &PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            self.init_called = true;
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            Ok(None)
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            self.shutdown_called = true;
            Ok(())
        }
    }

    /// A plugin that fails during initialization.
    struct FailingInitPlugin {
        metadata: PluginMetadata,
    }

    impl Plugin for FailingInitPlugin {
        fn metadata(&self) -> &PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            Err(PluginError::InitFailed("something went wrong".to_string()))
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            Ok(None)
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            Ok(())
        }
    }

    /// A plugin that fails during shutdown.
    struct FailingShutdownPlugin {
        metadata: PluginMetadata,
    }

    impl Plugin for FailingShutdownPlugin {
        fn metadata(&self) -> &PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            Ok(None)
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            Err(PluginError::ShutdownFailed(
                "something went wrong".to_string(),
            ))
        }
    }

    // -- PluginMetadata tests --------------------------------------------------

    #[test]
    fn plugin_metadata_new() {
        let meta = PluginMetadata::new("Test Plugin", "1.0.0");
        assert_eq!(meta.name, "Test Plugin");
        assert_eq!(meta.version, "1.0.0");
    }

    #[test]
    fn plugin_metadata_equality() {
        let a = PluginMetadata::new("Plugin", "1.0.0");
        let b = PluginMetadata::new("Plugin", "1.0.0");
        assert_eq!(a, b);
    }

    #[test]
    fn plugin_metadata_inequality() {
        let a = PluginMetadata::new("Plugin A", "1.0.0");
        let b = PluginMetadata::new("Plugin B", "2.0.0");
        assert_ne!(a, b);
    }

    // -- WindowIdentity tests -------------------------------------------------

    #[test]
    fn window_identity_defaults_to_empty() {
        let id = WindowIdentity::default();
        assert!(id.process_names.is_empty());
        assert!(id.window_classes.is_empty());
    }

    #[test]
    fn window_identity_constructs_from_iterators() {
        let id = WindowIdentity::new(
            ["fl64.exe".to_string(), "FL.exe".to_string()],
            ["TFruityLoopsMainForm".to_string()],
        );
        assert_eq!(id.process_names.len(), 2);
        assert_eq!(id.window_classes.len(), 1);
    }

    #[test]
    fn window_identity_is_equatable() {
        let a = WindowIdentity::new(["x.exe"], ["Class"]);
        let b = WindowIdentity::new(["x.exe"], ["Class"]);
        let c = WindowIdentity::new(["y.exe"], ["Class"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn plugin_window_identity_defaults_to_none() {
        // Plugins that do not declare a window identity default to None, so
        // the foreground-ownership layer simply excludes them.
        let plugin = OkPlugin::new("Headless", "1.0.0");
        assert!(plugin.window_identity().is_none());
    }

    // -- PluginError tests -----------------------------------------------------

    #[test]
    fn plugin_error_init_failed_display() {
        let err = PluginError::InitFailed("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }

    #[test]
    fn plugin_error_shutdown_failed_display() {
        let err = PluginError::ShutdownFailed("test error".to_string());
        assert!(err.to_string().contains("test error"));
    }

    // -- Plugin trait tests ----------------------------------------------------

    #[test]
    fn plugin_metadata_is_accessible() {
        let plugin = OkPlugin::new("My Plugin", "2.0.0");
        let meta = plugin.metadata();
        assert_eq!(meta.name, "My Plugin");
        assert_eq!(meta.version, "2.0.0");
    }

    #[test]
    fn plugin_init_is_called() {
        let mut plugin = OkPlugin::new("Test", "1.0.0");
        assert!(!plugin.init_called);
        plugin.init().unwrap();
        assert!(plugin.init_called);
    }

    #[test]
    fn plugin_shutdown_is_called() {
        let mut plugin = OkPlugin::new("Test", "1.0.0");
        assert!(!plugin.shutdown_called);
        plugin.shutdown().unwrap();
        assert!(plugin.shutdown_called);
    }

    #[test]
    fn plugin_trait_is_object_safe() {
        fn assert_object_safe(_: &dyn Plugin) {}
        let plugin = OkPlugin::new("Test", "1.0.0");
        assert_object_safe(&plugin);
    }

    // -- PluginHost tests ------------------------------------------------------

    #[test]
    fn new_creates_empty_host() {
        let host = PluginHost::new();
        assert!(host.plugins().is_empty());
    }

    #[test]
    fn register_adds_plugin() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("Test", "1.0.0")));
        assert_eq!(host.plugins().len(), 1);
        assert_eq!(host.plugins()[0].metadata().name, "Test");
    }

    #[test]
    fn register_multiple_plugins() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("A", "1.0.0")));
        host.register(Box::new(OkPlugin::new("B", "1.0.0")));
        host.register(Box::new(OkPlugin::new("C", "1.0.0")));
        assert_eq!(host.plugins().len(), 3);
    }

    #[test]
    fn init_all_initializes_all_plugins() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("A", "1.0.0")));
        host.register(Box::new(OkPlugin::new("B", "1.0.0")));

        let errors = host.init_all();
        assert!(errors.is_empty());
    }

    #[test]
    fn init_all_collects_errors() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("Good", "1.0.0")));
        host.register(Box::new(FailingInitPlugin {
            metadata: PluginMetadata::new("FailingInit", "1.0.0"),
        }));
        host.register(Box::new(OkPlugin::new("Also Good", "1.0.0")));

        let errors = host.init_all();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "FailingInit");
    }

    #[test]
    fn shutdown_all_shuts_down_all_plugins() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("A", "1.0.0")));
        host.register(Box::new(OkPlugin::new("B", "1.0.0")));
        host.init_all();

        let errors = host.shutdown_all();
        assert!(errors.is_empty());
    }

    #[test]
    fn shutdown_all_collects_errors() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("Good", "1.0.0")));
        host.register(Box::new(FailingShutdownPlugin {
            metadata: PluginMetadata::new("FailingShutdown", "1.0.0"),
        }));
        host.register(Box::new(OkPlugin::new("Also Good", "1.0.0")));
        host.init_all();

        let errors = host.shutdown_all();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].0, "FailingShutdown");
    }

    #[test]
    fn full_lifecycle() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("Test", "1.0.0")));

        let init_errors = host.init_all();
        assert!(init_errors.is_empty());

        let shutdown_errors = host.shutdown_all();
        assert!(shutdown_errors.is_empty());
    }

    #[test]
    fn plugins_returns_immutable_slice() {
        let mut host = PluginHost::new();
        host.register(Box::new(OkPlugin::new("A", "1.0.0")));
        host.register(Box::new(OkPlugin::new("B", "1.0.0")));

        let plugins: &[Box<dyn Plugin>] = host.plugins();
        assert_eq!(plugins.len(), 2);
        assert_eq!(plugins[0].metadata().name, "A");
        assert_eq!(plugins[1].metadata().name, "B");
    }

    #[test]
    fn default_creates_empty_host() {
        let host: PluginHost = Default::default();
        assert!(host.plugins().is_empty());
    }

    #[test]
    fn host_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<PluginHost>();
    }

    #[test]
    fn host_is_sync() {
        fn assert_sync<T: Sync>() {}
        assert_sync::<PluginHost>();
    }

    #[test]
    fn disabled_source_is_skipped_by_poll_all() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct CountingPlugin {
            metadata: PluginMetadata,
            polls: Arc<AtomicUsize>,
        }
        impl Plugin for CountingPlugin {
            fn metadata(&self) -> &PluginMetadata {
                &self.metadata
            }
            fn init(&mut self) -> Result<(), PluginError> {
                Ok(())
            }
            fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
                self.polls.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            }
            fn shutdown(&mut self) -> Result<(), PluginError> {
                Ok(())
            }
        }

        let polls_a = Arc::new(AtomicUsize::new(0));
        let polls_b = Arc::new(AtomicUsize::new(0));
        let mut host = PluginHost::new();
        host.register(Box::new(CountingPlugin {
            metadata: PluginMetadata::new("A", "1.0.0"),
            polls: polls_a.clone(),
        }));
        host.register(Box::new(CountingPlugin {
            metadata: PluginMetadata::new("B", "1.0.0"),
            polls: polls_b.clone(),
        }));

        host.set_source_disabled("A", true);
        assert!(host.is_source_disabled("A"));
        assert!(!host.is_source_disabled("B"));

        let results = host.poll_all();
        assert_eq!(results.len(), 1, "disabled source produces no entry");
        assert_eq!(results[0].0, "B");
        assert_eq!(
            polls_a.load(Ordering::SeqCst),
            0,
            "disabled poll never runs"
        );
        assert_eq!(polls_b.load(Ordering::SeqCst), 1);

        host.set_source_disabled("A", false);
        let results = host.poll_all();
        assert_eq!(results.len(), 2, "re-enabled source is polled again");
    }
}
