//! PresenceHub runtime bootstrap.
//!
//! Connects Core, PluginHost, PresenceEngine, and Outputs into a single
//! polling loop. This is the entry point for the application.
//!
//! # Lifecycle
//!
//! ```text
//! new() ──► start() ──► run() ──► shutdown() ──► (exit)
//! ```
//!
//! # Architecture
//!
//! The runtime owns only three components and coordinates them:
//!
//! ```text
//! Runtime
//!   ├── Core
//!   ├── PluginHost
//!   │   └── Plugins (created by PluginRegistry from Config)
//!   └── PresenceEngine
//!       └── Outputs (created by OutputRegistry from Config)
//! ```
//!
//! The runtime is configuration-driven and plugin-agnostic. It never
//! imports or constructs concrete plugin or output implementations.
//! All construction is delegated to [`PluginRegistry`] and [`OutputRegistry`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use presencehub_core::{activity::Activity, output::PresenceEngine, Config, Core};
use presencehub_plugin_host::{PluginError, PluginHost, WindowIdentity};
use tracing::{debug, info, warn};

use crate::foreground;
use crate::registry::{OutputRegistry, PluginRegistry};

// ---------------------------------------------------------------------------
// Runtime
// ---------------------------------------------------------------------------

/// The PresenceHub application runtime.
///
/// Owns Core, PluginHost, and PresenceEngine. Coordinates the flow:
/// Plugin → Activity → PresenceEngine → Outputs.
///
/// The runtime is configuration-driven. Plugins and outputs are created
/// by registries based on [`Config`], not hardcoded in the runtime.
pub struct Runtime {
    /// Core runtime lifecycle manager.
    core: Core,
    /// Plugin lifecycle and polling manager.
    host: PluginHost,
    /// Activity routing engine.
    engine: PresenceEngine,
    /// Whether the runtime is running (shared for Ctrl+C handling).
    running: Arc<AtomicBool>,
    /// Polling interval (derived from config).
    poll_interval: Duration,
    /// The configuration used to construct this runtime.
    config: Config,
    /// Global pause flag (GUI). While paused the loop sleeps without
    /// polling, and outputs were cleared on pause so nothing stale shows.
    /// Sessions and plugin state are preserved across the pause.
    paused: bool,
    /// Filesystem path of the loaded configuration, used to persist GUI
    /// plugin toggles. `None` when no config file was found.
    config_path: Option<PathBuf>,
    /// Whether the in-memory configuration has unsaved changes.
    config_dirty: bool,
    /// Last time the configuration was persisted (debounce baseline).
    last_save: Option<Instant>,
    /// Per-source poll outcome tracker (transition-based log warnings).
    /// Stored on the runtime so single poll iterations can run outside
    /// [`run`](Runtime::run) (e.g. from the GUI thread).
    poll_errors: PollErrorTracker,
}

// ---------------------------------------------------------------------------
// Poll error transition tracking
// ---------------------------------------------------------------------------

/// Tracks the last poll outcome per plugin so availability warnings are
/// emitted on *state transitions* rather than on every polling cycle.
///
/// Without this, a plugin whose window is absent (e.g. FL Studio not
/// running) emits a warning roughly once per poll interval, spamming the
/// log. With this tracker a warning is logged once when the plugin first
/// becomes unavailable, once more if the error message changes, and a
/// recovery is logged exactly once when the plugin becomes available again.
///
/// Sources are tracked independently so one plugin's failure can never
/// suppress another plugin's warnings.
#[derive(Default, Clone)]
struct PollErrorTracker {
    /// Most recent error message per source; a missing entry means the
    /// source was last seen healthy.
    last_errors: HashMap<String, String>,
}

impl PollErrorTracker {
    /// Record a healthy poll for `source`.
    ///
    /// Returns `true` when this is a recovery — the previous poll for the
    /// source had failed — so the caller can log the recovery exactly once.
    fn polled_ok(&mut self, source: &str) -> bool {
        self.last_errors.remove(source).is_some()
    }

    /// Record a failed poll for `source` with `message`.
    ///
    /// Returns `true` when the failure is new and should be logged: the
    /// source was previously healthy, or the error message changed. Returns
    /// `false` when the identical error persists unchanged, so the caller
    /// stays silent while the condition is ongoing.
    fn polled_err(&mut self, source: &str, message: &str) -> bool {
        match self.last_errors.get(source) {
            Some(previous) if previous == message => false,
            _ => {
                self.last_errors
                    .insert(source.to_string(), message.to_string());
                true
            }
        }
    }
}

impl Runtime {
    /// Create a new runtime with the given configuration.
    ///
    /// Plugins and outputs are not created until [`start`] is called.
    ///
    /// The poll interval is clamped to at least
    /// [`RuntimeConfig::MIN_POLL_INTERVAL_MS`], so a config constructed in
    /// code (bypassing TOML deserialization) with `poll_interval_ms = 0`
    /// cannot make the polling loop busy-spin.
    pub fn with_config(mut config: Config) -> Self {
        config.runtime.poll_interval_ms = config
            .runtime
            .poll_interval_ms
            .max(presencehub_core::RuntimeConfig::MIN_POLL_INTERVAL_MS);
        let poll_interval = Duration::from_millis(config.runtime.poll_interval_ms);
        Self {
            core: Core::with_config(config.clone()),
            host: PluginHost::new(),
            engine: PresenceEngine::new(),
            running: Arc::new(AtomicBool::new(false)),
            poll_interval,
            config,
            paused: false,
            config_path: None,
            config_dirty: false,
            last_save: None,
            poll_errors: PollErrorTracker::default(),
        }
    }

    /// Returns a clone of the running flag.
    ///
    /// This can be used to stop the runtime from another thread.
    #[allow(dead_code)]
    pub fn stop_signal(&self) -> Arc<AtomicBool> {
        self.running.clone()
    }

    /// Stop the runtime's polling loop.
    ///
    /// Sets the running flag to false, causing the `run()` loop to exit.
    /// Call `shutdown()` afterwards for cleanup.
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }

    /// Create a new runtime with default configuration.
    pub fn new() -> Self {
        Self::with_config(Config::default())
    }

    /// Returns a reference to the configuration.
    #[allow(dead_code)]
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Start the runtime.
    ///
    /// Initializes Core, creates and registers plugins via
    /// [`PluginRegistry`], creates and registers outputs via
    /// [`OutputRegistry`], then enters the polling loop.
    pub fn start(&mut self) -> Result<(), String> {
        info!("PresenceHub runtime starting");

        // 1. Start Core
        self.core
            .start()
            .map_err(|e| format!("Core start failed: {}", e))?;
        info!("Core started");

        // 2. Create enabled plugins via PluginRegistry
        let plugins = PluginRegistry::create_enabled_plugins(&self.config);
        info!("PluginRegistry created {} plugin(s)", plugins.len());

        // 3. Register plugins with PluginHost
        for plugin in plugins {
            self.host.register(plugin);
        }

        // 4. Initialize all plugins
        let init_errors = self.host.init_all();
        if !init_errors.is_empty() {
            for (name, err) in &init_errors {
                warn!("Plugin '{}' failed to initialize: {}", name, err);
            }
        }
        info!("Plugin initialization complete");

        // 4b. Apply config-driven disable flags so plugins disabled in
        // presencehub.toml are never polled, exactly like GUI toggles.
        for (source, enabled) in [
            ("FL Studio", self.config.plugins.flstudio),
            ("Antigravity", self.config.plugins.antigravity),
            ("OpenCode", self.config.plugins.opencode),
        ] {
            self.host.set_source_disabled(source, !enabled);
        }

        // 5. Create enabled outputs via OutputRegistry
        let outputs = OutputRegistry::create_enabled_outputs(&self.config);
        info!("OutputRegistry created {} output(s)", outputs.len());

        // 6. Register outputs with PresenceEngine
        for output in outputs {
            self.engine.register_output(output);
        }
        info!("Outputs registered");

        // 7. Apply the configured ownership policy
        self.engine.set_policy(self.config.presence.ownership);
        info!(policy = ?self.config.presence.ownership, "Ownership policy configured");

        // 8. Apply the unsupported-foreground policy
        self.engine
            .set_unsupported_foreground(self.config.presence.unsupported_foreground);
        info!(
            policy = ?self.config.presence.unsupported_foreground,
            "Unsupported-foreground policy configured"
        );

        self.running.store(true, Ordering::SeqCst);
        info!("PresenceHub runtime started");
        Ok(())
    }

    /// Run the main polling loop.
    ///
    /// Continuously runs single poll iterations through [`poll_once`](Runtime::poll_once).
    /// This is a blocking call. The desktop GUI drives [`poll_once`](Runtime::poll_once)
    /// directly from its background task instead.
    #[allow(dead_code)]
    pub fn run(&mut self) {
        if !self.running.load(Ordering::SeqCst) {
            warn!("Runtime not started, call start() before run()");
            return;
        }

        info!("Entering polling loop (interval: {:?})", self.poll_interval);

        while self.running.load(Ordering::SeqCst) {
            self.poll_once();
            std::thread::sleep(self.poll_interval);
        }

        info!("Polling loop exited");
    }

    /// Run a single poll iteration.
    ///
    /// Resolves the foreground source, polls every enabled plugin, and
    /// routes results to the PresenceEngine. While paused this is a no-op:
    /// nothing is polled and no session is touched, so resume continues
    /// exactly where the pause began. Extracted from [`run`](Runtime::run)
    /// so embedders (e.g. the GUI thread) can drive polling without taking
    /// over the blocking loop.
    pub fn poll_once(&mut self) {
        // Opportunistically flush debounced configuration writes, even
        // while paused (toggles still work when paused).
        if self.config_dirty {
            if let Err(e) = self.flush_config_if_due() {
                warn!(error = %e, "Deferred configuration persist failed");
            }
        }

        if self.paused {
            return;
        }

        // Take the tracker for the iteration so `handle_poll_result` keeps
        // its signature; it is restored before returning.
        let mut poll_errors = std::mem::take(&mut self.poll_errors);

        let plugin_count = self.host.plugins().len();
        debug!(plugins = plugin_count, "Poll iteration");

        // Resolve the OS foreground window to a registered source using
        // the plugins' declared window identities. Generic: the runtime
        // never names a concrete application.
        let foreground_sources: Vec<(String, WindowIdentity)> = self
            .host
            .plugins()
            .iter()
            .filter_map(|plugin| {
                let identity = plugin.window_identity()?;
                Some((plugin.metadata().name.clone(), identity))
            })
            .collect();

        let foreground_source = foreground::foreground_window().and_then(|window| {
            let sources: Vec<(&str, &WindowIdentity)> = foreground_sources
                .iter()
                .map(|(name, identity)| (name.as_str(), identity))
                .collect();
            foreground::resolve_foreground_source(&window, &sources).map(str::to_string)
        });

        let foreground_errors = self
            .engine
            .set_foreground_source(foreground_source.as_deref());
        if !foreground_errors.is_empty() {
            for (index, err) in &foreground_errors {
                warn!(
                    "Output {} failed to publish foreground change: {}",
                    index, err
                );
            }
        }

        // Poll every registered plugin through PluginHost.
        // Results are tagged with the plugin's source name so each
        // plugin's session can be routed independently. Sources disabled
        // through the GUI are skipped by PluginHost itself.
        for (source, result) in self.host.poll_all() {
            self.handle_poll_result(&mut poll_errors, source, result);
        }

        // Automatic output recovery: If an active presence exists but Discord
        // is disconnected (e.g. Discord was closed and restarted), attempt to
        // republish so the connection is restored automatically without user intervention.
        if !self.paused && !self.engine.any_output_connected() {
            if let (Some(source), Some(activity)) = (
                self.engine.displayed_source().map(str::to_owned),
                self.engine.current_activity().cloned(),
            ) {
                let _ = self.engine.publish_now(&source, &activity);
            }
        }

        self.poll_errors = poll_errors;
    }

    /// Attempt to reconnect outputs (Discord) and republish the active presence.
    pub fn reconnect_discord(&mut self) -> Result<bool, String> {
        if let (Some(source), Some(activity)) = (
            self.engine.displayed_source().map(str::to_owned),
            self.engine.current_activity().cloned(),
        ) {
            let _ = self.engine.publish_now(&source, &activity);
        } else {
            // Even if idle, poll once to check if any active plugin can publish
            let mut poll_errors = self.poll_errors.clone();
            for (source, result) in self.host.poll_all() {
                self.handle_poll_result(&mut poll_errors, source, result);
            }
            self.poll_errors = poll_errors;
        }
        Ok(self.engine.any_output_connected())
    }

    /// The polling interval between iterations.
    pub fn poll_interval(&self) -> Duration {
        self.poll_interval
    }

    /// Whether the runtime is globally paused (GUI).
    #[allow(dead_code)]
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// Whether the named plugin source is currently enabled.
    #[allow(dead_code)]
    pub fn is_plugin_enabled(&self, source: &str) -> bool {
        !self.host.is_source_disabled(source)
    }

    /// Builds the GUI view model from current backend state.
    ///
    /// Plugin rows come from the configuration (so disabled plugins are
    /// always listed); activity, ownership, and connection status come
    /// from the presence engine and its outputs. The frontend renders
    /// this verbatim and never reconstructs backend state.
    pub fn snapshot(&self) -> crate::state::LiveState {
        use crate::state::{LiveState, PluginView, PresenceView};

        let plugins = ["FL Studio", "Antigravity", "OpenCode"]
            .into_iter()
            .map(|name| {
                let enabled = match name {
                    "FL Studio" => self.config.plugins.flstudio,
                    "Antigravity" => self.config.plugins.antigravity,
                    "OpenCode" => self.config.plugins.opencode,
                    _ => false,
                } && !self.host.is_source_disabled(name);
                let summary = self
                    .engine
                    .source_activity(name)
                    .map(|activity| activity.state.clone());
                PluginView {
                    name: name.to_string(),
                    enabled,
                    active: self.engine.is_source_active(name),
                    summary,
                }
            })
            .collect();

        let owner = self.engine.displayed_source().map(str::to_owned);
        let current = owner.as_deref().and_then(|source| {
            self.engine.current_activity().map(|activity| PresenceView {
                source: source.to_string(),
                state: activity.state.clone(),
                details: activity.details.clone(),
            })
        });

        LiveState {
            paused: self.paused,
            poll_interval_ms: self.poll_interval.as_millis() as u64,
            discord_connected: self.engine.any_output_connected(),
            owner,
            current,
            plugins,
        }
    }

    /// Records where the configuration was loaded from, so GUI toggles can
    /// be persisted back to the same file.
    pub fn set_config_path(&mut self, path: Option<PathBuf>) {
        self.config_path = path;
    }

    /// Updates the polling cadence and persists it to configuration.
    pub fn set_poll_interval_ms(&mut self, interval_ms: u64) -> Result<(), String> {
        let clamped = interval_ms.max(presencehub_core::RuntimeConfig::MIN_POLL_INTERVAL_MS);
        self.poll_interval = Duration::from_millis(clamped);
        self.config.runtime.poll_interval_ms = clamped;
        info!(poll_interval_ms = clamped, "Updated polling interval");

        if let Some(path) = self.config_path.clone() {
            self.config
                .save(&path)
                .map_err(|e| format!("Failed to persist configuration: {e}"))?;
        }
        Ok(())
    }

    /// Enables or disables a plugin at runtime (GUI toggle).
    ///
    /// Disabling marks the source skipped so its `poll` is never called,
    /// and ends its engine session so the display falls back to the
    /// remaining plugins. Enabling clears the skip so the next poll detects
    /// the application immediately. The change is applied to the in-memory
    /// configuration and persisted to the config file when its path is
    /// known; a persistence failure is returned as an error string while
    /// the in-memory state stays applied.
    pub fn set_plugin_enabled(&mut self, source: &str, enabled: bool) -> Result<(), String> {
        match source {
            "FL Studio" => self.config.plugins.flstudio = enabled,
            "Antigravity" => self.config.plugins.antigravity = enabled,
            "OpenCode" => self.config.plugins.opencode = enabled,
            unknown => return Err(format!("Unknown plugin: {unknown}")),
        }

        // If the plugin was not registered at startup, instantiate and register it now.
        if enabled
            && !self
                .host
                .plugins()
                .iter()
                .any(|p| p.metadata().name == source)
        {
            let plugin: Box<dyn presencehub_plugin_host::Plugin> = match source {
                "FL Studio" => Box::new(presencehub_flstudio::FlStudioPlugin::new()),
                "Antigravity" => Box::new(presencehub_antigravity::AntigravityPlugin::new()),
                "OpenCode" => Box::new(presencehub_opencode::OpenCodePlugin::new()),
                _ => unreachable!(),
            };
            self.host.register(plugin);
            if let Some(p) = self
                .host
                .plugins_mut()
                .iter_mut()
                .find(|p| p.metadata().name == source)
            {
                let _ = p.init();
            }
        }

        self.host.set_source_disabled(source, !enabled);
        if enabled {
            self.poll_errors.polled_ok(source);
            info!(source = %source, "Plugin enabled");
        } else {
            self.engine.end_session(source);
            info!(source = %source, "Plugin disabled");
        }

        // Mark dirty and persist immediately when due; otherwise the write
        // is deferred to the poll loop so rapid toggles never queue disk
        // I/O behind the runtime lock.
        self.config_dirty = true;
        self.flush_config_if_due()?;
        Ok(())
    }

    /// Minimum interval between configuration file writes.
    ///
    /// Rapid GUI toggles mark the config dirty; the file is rewritten at
    /// most this often so disk I/O never piles up behind the runtime lock.
    const CONFIG_SAVE_DEBOUNCE: Duration = Duration::from_secs(2);

    /// Persists the configuration when a write is due.
    ///
    /// No-op unless the config is dirty and the debounce interval has
    /// elapsed since the last write. Failures leave the dirty flag set so
    /// a later flush retries.
    fn flush_config_if_due(&mut self) -> Result<(), String> {
        if !self.config_dirty {
            return Ok(());
        }
        let due = self
            .last_save
            .map(|at| at.elapsed() >= Self::CONFIG_SAVE_DEBOUNCE)
            .unwrap_or(true);
        if !due {
            return Ok(());
        }
        self.save_config_now()
    }

    /// Persists the configuration immediately, bypassing the debounce.
    ///
    /// No-op unless the config is dirty. Failures leave the dirty flag set.
    fn save_config_now(&mut self) -> Result<(), String> {
        if !self.config_dirty {
            return Ok(());
        }
        if let Some(path) = self.config_path.clone() {
            self.config
                .save(&path)
                .map_err(|e| format!("Failed to persist configuration: {e}"))?;
        }
        self.last_save = Some(Instant::now());
        self.config_dirty = false;
        Ok(())
    }

    /// Pauses or resumes the whole runtime (GUI control).
    ///
    /// Pausing clears every output so no stale presence remains, but keeps
    /// all sessions, plugin state, and configuration untouched — pause is
    /// not "disable every plugin". Resuming republishes the currently
    /// displayed activity so Discord recovers immediately with its original
    /// session timer intact.
    pub fn set_paused(&mut self, paused: bool) {
        if self.paused == paused {
            return;
        }
        self.paused = paused;
        if paused {
            info!("PresenceHub paused");
            self.engine.clear_outputs();
        } else {
            info!("PresenceHub resumed");
            if let (Some(source), Some(activity)) = (
                self.engine.displayed_source().map(str::to_owned),
                self.engine.current_activity().cloned(),
            ) {
                let _ = self.engine.publish_now(&source, &activity);
            }
        }
    }

    /// Handle a single plugin poll result.
    ///
    /// Availability warnings and recovery notices are deduplicated through
    /// [`PollErrorTracker`], so a persistent condition (e.g. FL Studio not
    /// running) is logged once on transition rather than every polling
    /// cycle. Each plugin's session is managed independently.
    ///
    /// Extracted from [`Runtime::run`] so the logging behavior can be
    /// verified in isolation.
    fn handle_poll_result(
        &mut self,
        poll_errors: &mut PollErrorTracker,
        source: String,
        result: Result<Option<Activity>, PluginError>,
    ) {
        match result {
            Ok(Some(activity)) => {
                if poll_errors.polled_ok(&source) {
                    info!(source = %source, "Plugin recovered");
                }
                debug!(
                    source = %source,
                    state = %activity.state,
                    details = ?activity.details,
                    metadata = ?activity.metadata,
                    "Runtime received Activity from plugin"
                );
                debug!(source = %source, "Runtime calling PresenceEngine::update");
                let errors = self.engine.update(&source, &activity);
                debug!(source = %source, errors = errors.len(), "PresenceEngine::update returned");
                if !errors.is_empty() {
                    for (index, err) in &errors {
                        warn!("Output {} failed to publish: {}", index, err);
                    }
                }
            }
            Ok(None) => {
                if poll_errors.polled_ok(&source) {
                    info!(source = %source, "Plugin recovered");
                }
                debug!(source = %source, "Plugin returned no activity change (Ok(None))");
            }
            Err(e) => {
                let message = e.to_string();
                // Plugin errored (e.g. FL Studio window not found). Log the
                // first occurrence of each distinct error per source, then go
                // silent until the error changes or the plugin recovers.
                if poll_errors.polled_err(&source, &message) {
                    warn!(source = %source, error = %message, "Plugin poll failed");
                }
                // A poll failure typically means the application closed.
                // End only this plugin's session so other plugins'
                // published presence is untouched, and the next
                // detection starts a fresh timer.
                self.engine.end_session(&source);
            }
        }
    }

    /// Returns whether the runtime is currently running.
    #[allow(dead_code)]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Shutdown the runtime gracefully.
    pub fn shutdown(&mut self) {
        info!("PresenceHub runtime shutting down");

        // 0. Flush any debounced configuration writes so toggles made
        // shortly before exit are still persisted. Best-effort: shutdown
        // always proceeds.
        if let Err(e) = self.save_config_now() {
            warn!(error = %e, "Failed to persist configuration on shutdown");
        }

        // 1. Stop polling
        self.running.store(false, Ordering::SeqCst);

        // 2. Clear the presence engine FIRST so every output (notably
        //    Discord) receives an explicit activity-clear before any
        //    potentially slow plugin shutdown runs or the process exits.
        //    Plugin shutdown can block, and Windows console-close force-
        //    terminates the process after a short grace period — so the
        //    Discord clear must happen as early as possible in shutdown.
        //    engine.clear() is best-effort and never fails, so shutdown
        //    always proceeds.
        info!("Clearing Discord presence before shutdown");
        self.engine.clear();

        // 3. Shutdown all plugins through PluginHost
        let shutdown_errors = self.host.shutdown_all();
        if !shutdown_errors.is_empty() {
            for (name, err) in &shutdown_errors {
                warn!("Plugin '{}' shutdown failed: {}", name, err);
            }
        }
        info!("Plugins shut down");

        // 4. Stop Core
        if let Err(e) = self.core.stop() {
            warn!("Core stop failed: {}", e);
        }
        info!("Core stopped");

        info!("PresenceHub runtime shutdown complete");
    }

    /// Returns a reference to the core.
    #[allow(dead_code)]
    pub fn core(&self) -> &Core {
        &self.core
    }

    /// Returns a reference to the plugin host.
    #[allow(dead_code)]
    pub fn host(&self) -> &PluginHost {
        &self.host
    }

    /// Returns a reference to the presence engine.
    #[allow(dead_code)]
    pub fn engine(&self) -> &PresenceEngine {
        &self.engine
    }
}

impl Default for Runtime {
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
    use presencehub_core::activity::Activity;
    use presencehub_plugin_host::PluginError;
    use std::collections::HashMap;

    /// A mock plugin that produces an activity on first poll, then None.
    struct MockActivityPlugin {
        metadata: presencehub_plugin_host::PluginMetadata,
        emitted: bool,
    }

    impl MockActivityPlugin {
        fn new(name: &str) -> Self {
            Self {
                metadata: presencehub_plugin_host::PluginMetadata::new(name, "1.0.0"),
                emitted: false,
            }
        }
    }

    impl presencehub_plugin_host::Plugin for MockActivityPlugin {
        fn metadata(&self) -> &presencehub_plugin_host::PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            self.emitted = false;
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            if !self.emitted {
                self.emitted = true;
                let activity = Activity {
                    state: "Editing".to_string(),
                    details: Some("song.flp".to_string()),
                    timestamps: None,
                    metadata: HashMap::new(),
                    application: None,
                };
                Ok(Some(activity))
            } else {
                Ok(None)
            }
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            self.emitted = false;
            Ok(())
        }

        fn reset(&mut self) {
            self.emitted = false;
        }
    }

    /// A mock plugin that always fails polling.
    struct FailingPollPlugin {
        metadata: presencehub_plugin_host::PluginMetadata,
    }

    impl presencehub_plugin_host::Plugin for FailingPollPlugin {
        fn metadata(&self) -> &presencehub_plugin_host::PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            Err(PluginError::PollFailed("intentional failure".to_string()))
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            Ok(())
        }
    }

    #[test]
    fn runtime_creation() {
        let runtime = Runtime::new();
        assert!(!runtime.is_running());
    }

    #[test]
    fn runtime_startup_with_defaults() {
        let mut runtime = Runtime::new();
        assert!(runtime.start().is_ok());
        assert!(runtime.is_running());
        runtime.shutdown();
        assert!(!runtime.is_running());
    }

    #[test]
    fn runtime_startup_with_custom_config() {
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 500,
            },
            ..Config::default()
        };
        let mut runtime = Runtime::with_config(config);
        assert!(runtime.start().is_ok());
        assert!(runtime.is_running());
        runtime.shutdown();
    }

    #[test]
    fn runtime_startup_with_empty_registries() {
        // All plugins and outputs disabled
        let config = Config {
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: false,
            },
            outputs: presencehub_core::OutputConfig {
                console: false,
                discord: false,
                discord_app_id: 0,
                discord_apps: std::collections::HashMap::new(),
            },
            ..Config::default()
        };
        let mut runtime = Runtime::with_config(config);
        assert!(runtime.start().is_ok());
        assert!(runtime.is_running());
        assert_eq!(runtime.host().plugins().len(), 0);
        runtime.shutdown();
    }

    #[test]
    fn runtime_shutdown_without_start() {
        let mut runtime = Runtime::new();
        runtime.shutdown(); // Should not panic
        assert!(!runtime.is_running());
    }

    #[test]
    fn runtime_shutdown_twice_is_safe() {
        // Calling shutdown() twice (e.g. an exit hook plus a Ctrl+C handler)
        // must not panic, error, or corrupt state.
        let mut runtime = Runtime::new();
        runtime.start().unwrap();
        runtime.shutdown();
        runtime.shutdown();
        assert!(!runtime.is_running());
    }

    #[test]
    fn runtime_run_without_start() {
        let mut runtime = Runtime::new();
        runtime.run(); // Should not panic, just log a warning
        assert!(!runtime.is_running());
    }

    #[test]
    fn runtime_shutdown_clears_engine() {
        let mut runtime = Runtime::new();
        runtime.start().unwrap();
        assert!(runtime.is_running());
        runtime.shutdown();
        assert!(!runtime.is_running());
        assert!(runtime.engine().current_activity().is_none());
    }

    #[test]
    fn runtime_owns_plugin_host() {
        let runtime = Runtime::new();
        assert_eq!(runtime.host().plugins().len(), 0);

        let mut runtime = Runtime::new();
        runtime.start().unwrap();
        // FL Studio, Antigravity, and OpenCode plugins should be registered via PluginRegistry
        assert_eq!(runtime.host().plugins().len(), 3);
        runtime.shutdown();
    }

    #[test]
    fn runtime_plugin_host_polls_multiple_plugins() {
        let mut host = PluginHost::new();
        host.register(Box::new(MockActivityPlugin::new("A")));
        host.register(Box::new(MockActivityPlugin::new("B")));

        let results = host.poll_all();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "A");
        assert!(results[0].1.as_ref().unwrap().is_some());
        assert_eq!(results[1].0, "B");
        assert!(results[1].1.as_ref().unwrap().is_some());

        let results = host.poll_all();
        assert_eq!(results.len(), 2);
        assert!(results[0].1.as_ref().unwrap().is_none());
        assert!(results[1].1.as_ref().unwrap().is_none());
    }

    #[test]
    fn runtime_plugin_poll_failure_isolation() {
        let mut host = PluginHost::new();
        host.register(Box::new(FailingPollPlugin {
            metadata: presencehub_plugin_host::PluginMetadata::new("Failing", "1.0.0"),
        }));
        host.register(Box::new(MockActivityPlugin::new("Working")));

        let results = host.poll_all();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "Failing");
        assert!(results[0].1.is_err(), "First plugin should fail");
        assert_eq!(results[1].0, "Working");
        assert!(
            results[1].1.as_ref().unwrap().is_some(),
            "Second plugin should produce activity"
        );
    }

    #[test]
    fn runtime_activity_routing() {
        use presencehub_core::output::ConsoleOutput;

        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Editing".to_string(),
            details: Some("song.flp".to_string()),
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let errors = engine.update("FL Studio", &activity);
        assert!(errors.is_empty());

        // The engine stamps the session start time, so the stored activity
        // carries a timestamp even though the plugin provided none.
        let current = engine.current_activity().unwrap();
        assert_eq!(current.state, "Editing");
        assert_eq!(current.details, Some("song.flp".to_string()));
        assert!(current.timestamps.as_ref().unwrap().start.is_some());
    }

    #[test]
    fn runtime_duplicate_suppression() {
        use presencehub_core::output::ConsoleOutput;

        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(ConsoleOutput::new()));

        let activity = Activity {
            state: "Idle".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        engine.update("A", &activity);
        let errors = engine.update("A", &activity);
        assert!(
            errors.is_empty(),
            "Duplicate activities should not be republished"
        );
    }

    #[test]
    fn runtime_output_failure_isolation() {
        use presencehub_core::output::Output;
        use presencehub_core::output::OutputError;

        struct FailingOutput;
        impl Output for FailingOutput {
            fn publish(&mut self, _source: &str, _activity: &Activity) -> Result<(), OutputError> {
                Err(OutputError::PublishFailed(
                    "intentional failure".to_string(),
                ))
            }
        }

        let mut engine = PresenceEngine::new();
        engine.register_output(Box::new(FailingOutput));
        engine.register_output(Box::new(presencehub_core::output::ConsoleOutput::new()));

        let activity = Activity {
            state: "Editing".to_string(),
            details: None,
            timestamps: None,
            metadata: HashMap::new(),
            application: None,
        };

        let errors = engine.update("A", &activity);
        assert_eq!(errors.len(), 1, "One output should fail");
        assert_eq!(errors[0].0, 0, "Failing output should be index 0");
    }

    #[test]
    fn runtime_default_poll_interval() {
        let runtime = Runtime::new();
        assert_eq!(runtime.poll_interval, Duration::from_millis(1000));
    }

    #[test]
    fn runtime_custom_poll_interval() {
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 5000,
            },
            ..Config::default()
        };
        let runtime = Runtime::with_config(config);
        assert_eq!(runtime.poll_interval, Duration::from_millis(5000));
    }

    #[test]
    fn runtime_clamps_zero_poll_interval_to_minimum() {
        // Regression: a poll_interval_ms of 0 (constructed in code, bypassing
        // TOML deserialization) must not make the polling loop busy-spin; the
        // runtime boundary clamps it to the safe minimum.
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 0,
            },
            ..Config::default()
        };
        let runtime = Runtime::with_config(config);
        assert_eq!(
            runtime.poll_interval,
            Duration::from_millis(presencehub_core::RuntimeConfig::MIN_POLL_INTERVAL_MS),
            "zero poll interval must be clamped to the safe minimum"
        );
        assert_eq!(
            runtime.config().runtime.poll_interval_ms,
            presencehub_core::RuntimeConfig::MIN_POLL_INTERVAL_MS,
            "the effective config must reflect the clamped interval"
        );
    }

    #[test]
    fn runtime_clamps_excessively_small_poll_interval_to_minimum() {
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 5,
            },
            ..Config::default()
        };
        let runtime = Runtime::with_config(config);
        assert_eq!(
            runtime.poll_interval,
            Duration::from_millis(presencehub_core::RuntimeConfig::MIN_POLL_INTERVAL_MS)
        );
    }

    #[test]
    fn runtime_config_accessible() {
        let runtime = Runtime::new();
        assert_eq!(runtime.config().runtime.poll_interval_ms, 1000);
        assert!(runtime.config().plugins.flstudio);
    }

    // -----------------------------------------------------------------------
    // Poll error transition logging
    // -----------------------------------------------------------------------

    /// Runs `body` under a tracing subscriber that captures formatted output.
    fn capture_logs(body: impl FnOnce()) -> String {
        use std::sync::{Arc, Mutex};

        struct CaptureWriter(Arc<Mutex<String>>);
        impl std::io::Write for CaptureWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0
                    .lock()
                    .unwrap()
                    .push_str(&String::from_utf8_lossy(buf));
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CaptureWriter {
            type Writer = CaptureWriter;
            fn make_writer(&'a self) -> Self::Writer {
                CaptureWriter(self.0.clone())
            }
        }

        let buf = Arc::new(Mutex::new(String::new()));
        let writer = CaptureWriter(buf.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(writer)
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .finish();

        tracing::subscriber::with_default(subscriber, body);
        let logs = buf.lock().unwrap().clone();
        logs
    }

    #[test]
    fn poll_error_tracker_unavailable_sequence_logs_once() {
        let mut tracker = PollErrorTracker::default();
        assert!(
            tracker.polled_err("FL Studio", "window not found"),
            "first failure logs"
        );
        assert!(
            !tracker.polled_err("FL Studio", "window not found"),
            "unchanged repeat failure is suppressed"
        );
        assert!(
            !tracker.polled_err("FL Studio", "window not found"),
            "still suppressed while condition persists"
        );
    }

    #[test]
    fn poll_error_tracker_unavailable_to_available_logs_recovery_once() {
        let mut tracker = PollErrorTracker::default();
        assert!(tracker.polled_err("FL Studio", "window not found"));
        assert!(tracker.polled_ok("FL Studio"), "recovery logged once");
        assert!(
            !tracker.polled_ok("FL Studio"),
            "a healthy source that stays healthy must not keep logging recovery"
        );
    }

    #[test]
    fn poll_error_tracker_available_sequence_no_recovery() {
        let mut tracker = PollErrorTracker::default();
        assert!(
            !tracker.polled_ok("FL Studio"),
            "healthy -> healthy is no transition"
        );
        assert!(!tracker.polled_ok("FL Studio"));
    }

    #[test]
    fn poll_error_tracker_available_to_unavailable_logs_one() {
        let mut tracker = PollErrorTracker::default();
        assert!(!tracker.polled_ok("FL Studio"));
        assert!(
            tracker.polled_err("FL Studio", "window not found"),
            "new failure logs"
        );
        assert!(!tracker.polled_err("FL Studio", "window not found"));
    }

    #[test]
    fn poll_error_tracker_changed_error_message_is_a_new_transition() {
        let mut tracker = PollErrorTracker::default();
        assert!(tracker.polled_err("FL Studio", "window not found"));
        assert!(!tracker.polled_err("FL Studio", "window not found"));
        assert!(
            tracker.polled_err("FL Studio", "unexpected exit"),
            "a changed error message is a new transition and must be logged"
        );
    }

    #[test]
    fn poll_error_tracker_per_source_isolation() {
        let mut tracker = PollErrorTracker::default();
        assert!(tracker.polled_err("FL Studio", "window not found"));
        assert!(!tracker.polled_err("FL Studio", "window not found"));
        assert!(
            tracker.polled_err("Antigravity", "client not running"),
            "one plugin's repeated failure must not suppress another plugin's warning"
        );
        assert!(
            tracker.polled_ok("FL Studio"),
            "recovery is tracked per source"
        );
    }

    #[test]
    fn runtime_unavailable_plugin_logs_warning_once_per_condition() {
        let mut runtime = Runtime::new();
        let err = || {
            Err(PluginError::PollFailed(
                "FL Studio window not found".to_string(),
            ))
        };

        let logs = capture_logs(|| {
            let mut tracker = PollErrorTracker::default();
            // unavailable (first occurrence) -> warn
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), err());
            // unavailable (unchanged) -> suppressed
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), err());
            // unavailable (unchanged) -> suppressed
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), err());
            // available -> recovery logged once
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), Ok(None));
            // available (still) -> no repeat recovery
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), Ok(None));
            // unavailable again -> one new warning
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), err());
            // unavailable (unchanged) -> suppressed
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), err());
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        let recoveries = logs
            .lines()
            .filter(|l| l.contains("Plugin recovered"))
            .count();
        assert_eq!(
            warnings, 2,
            "one warning per unavailable transition, not per poll; logs:\n{}",
            logs
        );
        assert_eq!(
            recoveries, 1,
            "recovery logged exactly once; logs:\n{}",
            logs
        );
    }

    #[test]
    fn runtime_unrelated_plugin_failure_not_suppressed() {
        let mut runtime = Runtime::new();
        let fl_err = || {
            Err(PluginError::PollFailed(
                "FL Studio window not found".to_string(),
            ))
        };
        let antigravity_err = || {
            Err(PluginError::PollFailed(
                "Antigravity client not running".to_string(),
            ))
        };

        let logs = capture_logs(|| {
            let mut tracker = PollErrorTracker::default();
            // FL Studio goes down (warn) and stays down (suppressed).
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), fl_err());
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), fl_err());
            // An unrelated plugin fails in the meantime; it must still warn.
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), antigravity_err());
            // FL Studio remains down; still suppressed.
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), fl_err());
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        assert_eq!(
            warnings, 2,
            "FL Studio suppressed, but the Antigravity failure must still be logged; logs:\n{}",
            logs
        );
    }

    #[test]
    fn runtime_antigravity_repeated_failure_logs_exactly_one_warning() {
        // The reported scenario: Antigravity is unavailable and every
        // polling cycle produces the identical "client not running" error.
        // The runtime must warn once and then stay silent while the condition
        // persists.
        let mut runtime = Runtime::new();
        let err = || {
            Err(PluginError::PollFailed(
                "Antigravity client not running".to_string(),
            ))
        };

        let logs = capture_logs(|| {
            let mut tracker = PollErrorTracker::default();
            for _ in 0..20 {
                runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), err());
            }
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        let recoveries = logs
            .lines()
            .filter(|l| l.contains("Plugin recovered"))
            .count();
        assert_eq!(
            warnings, 1,
            "repeated identical Antigravity failures must yield exactly ONE warning; logs:\n{}",
            logs
        );
        assert_eq!(
            recoveries, 0,
            "no recovery expected while Antigravity stays down; logs:\n{}",
            logs
        );
    }

    #[test]
    fn runtime_antigravity_recovery_sequence() {
        // One Antigravity warning -> silence -> one recovery -> silence -> one
        // warning when Antigravity comes back and then fails again.
        let mut runtime = Runtime::new();
        let err = || {
            Err(PluginError::PollFailed(
                "Antigravity client not running".to_string(),
            ))
        };

        let logs = capture_logs(|| {
            let mut tracker = PollErrorTracker::default();
            // unavailable (warn), unavailable (silent), unavailable (silent)
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), err());
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), err());
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), err());
            // available (recovery once), available (silent)
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), Ok(None));
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), Ok(None));
            // unavailable again (warn), unavailable (silent)
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), err());
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), err());
        });

        let mut events = Vec::new();
        for line in logs.lines() {
            if line.contains("Plugin poll failed") {
                events.push("warn");
            } else if line.contains("Plugin recovered") {
                events.push("recovery");
            }
        }
        assert_eq!(
            events,
            vec!["warn", "recovery", "warn"],
            "transition order must be warn, recovery, warn; logs:\n{}",
            logs
        );
    }

    #[test]
    fn runtime_antigravity_fl_plugin_isolation() {
        // Both FL Studio and Antigravity are unavailable, alternating
        // every cycle. Each plugin's first failure must warn independently;
        // neither suppresses the other's warning.
        let mut runtime = Runtime::new();
        let fl_err = || {
            Err(PluginError::PollFailed(
                "FL Studio window not found".to_string(),
            ))
        };
        let antigravity_err = || {
            Err(PluginError::PollFailed(
                "Antigravity client not running".to_string(),
            ))
        };

        let logs = capture_logs(|| {
            let mut tracker = PollErrorTracker::default();
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), fl_err());
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), antigravity_err());
            runtime.handle_poll_result(&mut tracker, "FL Studio".to_string(), fl_err());
            runtime.handle_poll_result(&mut tracker, "Antigravity".to_string(), antigravity_err());
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        assert_eq!(
            warnings, 2,
            "each plugin's first failure must be logged independently; logs:\n{}",
            logs
        );
    }

    /// A plugin that always fails its poll, like FL Studio or Antigravity
    /// when the app is not running. Emits the identical error on
    /// every poll.
    struct AlwaysFailingPlugin {
        metadata: presencehub_plugin_host::PluginMetadata,
        error: String,
    }

    impl AlwaysFailingPlugin {
        fn new(name: &str) -> Self {
            Self::with_error(name, "FL Studio window not found")
        }

        fn with_error(name: &str, error: &str) -> Self {
            Self {
                metadata: presencehub_plugin_host::PluginMetadata::new(name, "1.0.0"),
                error: error.to_string(),
            }
        }
    }

    impl presencehub_plugin_host::Plugin for AlwaysFailingPlugin {
        fn metadata(&self) -> &presencehub_plugin_host::PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            Err(PluginError::PollFailed(self.error.clone()))
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            Ok(())
        }
    }

    /// A plugin whose availability cycles over time: unavailable, then
    /// available, then unavailable again, staying unavailable thereafter.
    /// Used to drive the real polling loop through the full transition
    /// sequence without needing a live application.
    struct StateCyclingPlugin {
        metadata: presencehub_plugin_host::PluginMetadata,
        polls: usize,
    }

    impl StateCyclingPlugin {
        fn new(name: &str) -> Self {
            Self {
                metadata: presencehub_plugin_host::PluginMetadata::new(name, "1.0.0"),
                polls: 0,
            }
        }
    }

    impl presencehub_plugin_host::Plugin for StateCyclingPlugin {
        fn metadata(&self) -> &presencehub_plugin_host::PluginMetadata {
            &self.metadata
        }

        fn init(&mut self) -> Result<(), PluginError> {
            Ok(())
        }

        fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
            // 5 polls per phase. Enough phases run so the loop observes
            // unavailable -> available -> unavailable within the test window.
            let phase = self.polls / 5;
            self.polls += 1;
            match phase {
                0 => Err(PluginError::PollFailed(
                    "FL Studio window not found".to_string(),
                )),
                1 => Ok(None),
                _ => Err(PluginError::PollFailed(
                    "FL Studio window not found".to_string(),
                )),
            }
        }

        fn shutdown(&mut self) -> Result<(), PluginError> {
            Ok(())
        }
    }

    /// Drives the real [`Runtime::run`] polling loop (not the extracted
    /// handler) with a plugin that fails identically on every cycle and
    /// asserts that the runtime emits exactly one warning.
    #[test]
    fn runtime_poll_loop_logs_exactly_one_flstudio_warning() {
        // Regression test for the observed bug: with FL Studio unavailable,
        // the plugin returned an identical PollFailed every polling cycle and
        // the running application logged a warning every ~1s. Driving the real
        // polling loop across many cycles must produce exactly ONE warning.
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 5,
            },
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: false,
            },
            ..Config::default()
        };
        let mut runtime = Runtime::with_config(config);
        runtime
            .host
            .register(Box::new(AlwaysFailingPlugin::new("FL Studio")));
        runtime.running.store(true, Ordering::SeqCst);

        let stop = runtime.stop_signal();
        let logs = capture_logs(|| {
            // Stop the loop from a second thread after many cycles; the loop
            // itself runs on this thread so its logs are captured.
            let stopper = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                stop.store(false, Ordering::SeqCst);
            });
            runtime.run();
            stopper.join().unwrap();
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        let recoveries = logs
            .lines()
            .filter(|l| l.contains("Plugin recovered"))
            .count();
        assert_eq!(
            warnings, 1,
            "identical FL Studio failures across many polling cycles must yield exactly ONE warning; logs:\n{}",
            logs
        );
        assert_eq!(
            recoveries, 0,
            "no recovery expected while FL Studio stays down; logs:\n{}",
            logs
        );
    }

    /// Drives the real [`Runtime::run`] polling loop with an Antigravity-named
    /// plugin that fails identically on every cycle and asserts that the
    /// runtime emits exactly one warning.
    #[test]
    fn runtime_poll_loop_logs_exactly_one_antigravity_warning() {
        // The reported scenario driven through the real polling loop:
        // an Antigravity plugin returning the identical "client not running" error
        // every cycle must yield exactly ONE warning, not one per cycle.
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 5,
            },
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: false,
            },
            ..Config::default()
        };
        let mut runtime = Runtime::with_config(config);
        runtime
            .host
            .register(Box::new(AlwaysFailingPlugin::with_error(
                "Antigravity",
                "Antigravity client not running",
            )));
        runtime.running.store(true, Ordering::SeqCst);

        let stop = runtime.stop_signal();
        let logs = capture_logs(|| {
            let stopper = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(600));
                stop.store(false, Ordering::SeqCst);
            });
            runtime.run();
            stopper.join().unwrap();
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        let recoveries = logs
            .lines()
            .filter(|l| l.contains("Plugin recovered"))
            .count();
        assert_eq!(
            warnings, 1,
            "identical Antigravity failures across many polling cycles must yield exactly ONE warning; logs:\n{}",
            logs
        );
        assert_eq!(
            recoveries, 0,
            "no recovery expected while Antigravity stays down; logs:\n{}",
            logs
        );
    }

    /// Drives the real [`Runtime::run`] polling loop through the full
    /// availability sequence and asserts the transition-based logging:
    /// 1 warning -> silence -> 1 recovery -> silence -> 1 warning -> silence.
    #[test]
    fn runtime_poll_loop_full_transition_sequence() {
        let config = Config {
            runtime: presencehub_core::RuntimeConfig {
                poll_interval_ms: 5,
            },
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: false,
            },
            ..Config::default()
        };
        let mut runtime = Runtime::with_config(config);
        runtime
            .host
            .register(Box::new(StateCyclingPlugin::new("FL Studio")));
        runtime.running.store(true, Ordering::SeqCst);

        let stop = runtime.stop_signal();
        let logs = capture_logs(|| {
            // The runtime clamps the poll interval to the safe minimum
            // (250 ms), so reaching all transition phases needs at least
            // 11 cycles (~2.75 s); the 4 s window leaves room for the
            // foreground-window detection that runs each cycle.
            let stopper = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(4000));
                stop.store(false, Ordering::SeqCst);
            });
            runtime.run();
            stopper.join().unwrap();
        });

        let warnings = logs
            .lines()
            .filter(|l| l.contains("Plugin poll failed"))
            .count();
        let recoveries = logs
            .lines()
            .filter(|l| l.contains("Plugin recovered"))
            .count();
        assert_eq!(
            warnings, 2,
            "one warning per unavailable transition across the real poll loop; logs:\n{}",
            logs
        );
        assert_eq!(
            recoveries, 1,
            "one recovery logged across the real poll loop; logs:\n{}",
            logs
        );
    }

    // -----------------------------------------------------------------------
    // GUI controls: enable/disable and pause
    // -----------------------------------------------------------------------

    #[test]
    fn set_plugin_enabled_rejects_unknown_plugin() {
        let mut runtime = Runtime::new();
        assert!(runtime.set_plugin_enabled("League", true).is_err());
        assert!(runtime.set_plugin_enabled("", false).is_err());
    }

    #[test]
    fn disable_plugin_stops_polling_and_clears_session() {
        let mut runtime = Runtime::new();
        runtime
            .host
            .register(Box::new(MockActivityPlugin::new("FL Studio")));

        // First poll publishes.
        runtime.poll_once();
        assert!(runtime.engine.current_activity().is_some());

        // Disable: session ends immediately and future polls skip the plugin.
        assert!(runtime.set_plugin_enabled("FL Studio", false).is_ok());
        assert!(!runtime.is_plugin_enabled("FL Studio"));
        assert!(!runtime.config.plugins.flstudio);
        assert!(runtime.engine.current_activity().is_none());

        runtime.poll_once();
        assert!(runtime.engine.current_activity().is_none());

        // Re-enable: the next poll detects the application again.
        assert!(runtime.set_plugin_enabled("FL Studio", true).is_ok());
        assert!(runtime.is_plugin_enabled("FL Studio"));
        assert!(runtime.config.plugins.flstudio);

        runtime.poll_once();
        assert!(
            runtime.engine.current_activity().is_some(),
            "re-enabling a plugin must detect and publish its activity on the next poll"
        );
    }

    #[test]
    fn pause_skips_polling_but_preserves_session() {
        let mut runtime = Runtime::new();
        runtime
            .host
            .register(Box::new(MockActivityPlugin::new("A")));

        runtime.poll_once();
        assert!(runtime.engine.current_activity().is_some());
        assert!(!runtime.is_paused());

        // Pause: polling stops but the session survives.
        runtime.set_paused(true);
        assert!(runtime.is_paused());
        runtime.poll_once();
        assert!(
            runtime.engine.current_activity().is_some(),
            "pause must not end the session"
        );
        assert!(runtime.engine.session_started_at().is_some());

        // Resume republishes the preserved session.
        runtime.set_paused(false);
        assert!(!runtime.is_paused());
        assert!(runtime.engine.current_activity().is_some());
    }

    #[test]
    fn set_paused_is_idempotent() {
        let mut runtime = Runtime::new();
        runtime.set_paused(false);
        assert!(!runtime.is_paused());
        runtime.set_paused(true);
        runtime.set_paused(true);
        assert!(runtime.is_paused());
    }

    fn temp_config_path(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "presencehub_toggle_test_{tag}_{}.toml",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn config_writes_are_debounced_and_flushed_by_poll() {
        let path = temp_config_path("debounce");
        let mut runtime = Runtime::new();
        runtime.set_config_path(Some(path.clone()));

        // First toggle writes immediately (no previous write to debounce).
        runtime.set_plugin_enabled("FL Studio", false).unwrap();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("flstudio = false"));

        // A rapid second toggle applies in memory but defers the write.
        runtime.set_plugin_enabled("FL Studio", true).unwrap();
        assert!(runtime.config.plugins.flstudio);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("flstudio = false"),
            "rapid toggle must not rewrite the file immediately"
        );

        // Once the debounce interval elapses, the poll loop flushes it.
        runtime.last_save = Some(Instant::now() - Duration::from_secs(3));
        runtime.poll_once();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("flstudio = true"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn shutdown_flushes_debounced_config() {
        let path = temp_config_path("shutdown");
        let mut runtime = Runtime::new();
        runtime.set_config_path(Some(path.clone()));

        runtime.set_plugin_enabled("FL Studio", false).unwrap();
        runtime.set_plugin_enabled("FL Studio", true).unwrap();

        // The last toggle is still debounced (not yet on disk)...
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("flstudio = false"));

        // ...but shutdown flushes it.
        runtime.shutdown();
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("flstudio = true"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn reconnect_discord_republishes_active_session() {
        let mut runtime = Runtime::new();
        runtime
            .host
            .register(Box::new(MockActivityPlugin::new("FL Studio")));

        runtime.poll_once();
        assert!(runtime.engine.current_activity().is_some());

        // Reconnect discord should succeed and republish
        let result = runtime.reconnect_discord();
        assert!(result.is_ok());
        assert!(runtime.engine.current_activity().is_some());
    }
}
