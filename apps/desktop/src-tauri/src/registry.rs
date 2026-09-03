//! Plugin and Output registries.
//!
//! These registries are responsible for constructing concrete plugin and
//! output instances based on configuration. The runtime delegates to them
//! so that it never needs to know about specific implementations.
//!
//! # Adding a New Plugin
//!
//! To add a new plugin (e.g. Spotify):
//!
//! 1. Implement the `Plugin` trait in a new crate (e.g. `plugins/spotify`).
//! 2. Add a field to `PluginConfig` in `presencehub-core` (e.g. `spotify: bool`).
//! 3. Add one entry in [`PluginRegistry::create_enabled_plugins`].
//!
//! The runtime does not change.

use presencehub_antigravity::AntigravityPlugin;
use presencehub_core::{
    output::{ConsoleOutput, Output},
    Config,
};
use presencehub_discord_output::DiscordOutput;
use presencehub_flstudio::FlStudioPlugin;
use presencehub_opencode::OpenCodePlugin;
use presencehub_plugin_host::Plugin;

// ---------------------------------------------------------------------------
// Plugin Registry
// ---------------------------------------------------------------------------

/// Constructs enabled plugins based on configuration.
///
/// The runtime calls [`create_enabled_plugins`] during startup and registers
/// the returned plugins with `PluginHost`. The runtime never constructs
/// plugins directly.
pub struct PluginRegistry;

impl PluginRegistry {
    /// Create all enabled plugins based on the given configuration.
    ///
    /// Returns a vector of `Box<dyn Plugin>` ready to be registered with
    /// `PluginHost`.
    ///
    /// # Adding a Plugin
    ///
    /// To add a new plugin, add a check here:
    ///
    /// ```rust,ignore
    /// if config.plugins.my_plugin {
    ///     plugins.push(Box::new(MyPlugin::new()));
    /// }
    /// ```
    #[allow(dead_code)]
    pub fn create_enabled_plugins(config: &Config) -> Vec<Box<dyn Plugin>> {
        let mut plugins: Vec<Box<dyn Plugin>> = Vec::new();

        if config.plugins.flstudio {
            plugins.push(Box::new(FlStudioPlugin::new()));
        }

        if config.plugins.antigravity {
            plugins.push(Box::new(AntigravityPlugin::new()));
        }

        if config.plugins.opencode {
            plugins.push(Box::new(OpenCodePlugin::new()));
        }

        // Future plugins go here:
        // if config.plugins.spotify { plugins.push(Box::new(SpotifyPlugin::new())); }
        // if config.plugins.vscode { plugins.push(Box::new(VSCodePlugin::new())); }

        plugins
    }

    /// Constructs all supported plugins regardless of initial enable state.
    ///
    /// The runtime registers all plugins with `PluginHost` and applies
    /// enable/disable flags so plugins can be toggled on and off dynamically
    /// at runtime without needing application restart.
    #[allow(dead_code)]
    pub fn create_all_plugins() -> Vec<Box<dyn Plugin>> {
        vec![
            Box::new(FlStudioPlugin::new()),
            Box::new(AntigravityPlugin::new()),
            Box::new(OpenCodePlugin::new()),
        ]
    }
}

// ---------------------------------------------------------------------------
// Output Registry
// ---------------------------------------------------------------------------

/// Constructs enabled outputs based on configuration.
///
/// The runtime calls [`create_enabled_outputs`] during startup and registers
/// the returned outputs with `PresenceEngine`. The runtime never constructs
/// outputs directly.
pub struct OutputRegistry;

impl OutputRegistry {
    /// Create all enabled outputs based on the given configuration.
    ///
    /// Returns a vector of `Box<dyn Output>` ready to be registered with
    /// `PresenceEngine`.
    ///
    /// # Adding an Output
    ///
    /// To add a new output, add a check here:
    ///
    /// ```rust,ignore
    /// if config.outputs.my_output {
    ///     outputs.push(Box::new(MyOutput::new()));
    /// }
    /// ```
    pub fn create_enabled_outputs(config: &Config) -> Vec<Box<dyn Output>> {
        let mut outputs: Vec<Box<dyn Output>> = Vec::new();

        if config.outputs.console {
            outputs.push(Box::new(ConsoleOutput::new()));
        }

        // Discord requires a valid application ID (a default or at least one
        // per-source entry).
        if config.outputs.discord
            && (config.outputs.discord_app_id > 0 || !config.outputs.discord_apps.is_empty())
        {
            outputs.push(Box::new(DiscordOutput::new(
                config.outputs.discord_app_id,
                config.outputs.discord_apps.clone(),
            )));
        }

        // Future outputs go here:
        // if config.outputs.slack { outputs.push(Box::new(SlackOutput::new())); }

        outputs
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use presencehub_core::Config;

    // -- PluginRegistry tests --------------------------------------------------

    #[test]
    fn plugin_registry_creates_flstudio_antigravity_and_opencode_when_enabled() {
        let config = Config::default(); // flstudio = true, antigravity = true, opencode = true by default
        let plugins = PluginRegistry::create_enabled_plugins(&config);
        assert_eq!(plugins.len(), 3);
        assert_eq!(plugins[0].metadata().name, "FL Studio");
        assert_eq!(plugins[1].metadata().name, "Antigravity");
        assert_eq!(plugins[2].metadata().name, "OpenCode");
    }

    #[test]
    fn plugin_registry_empty_when_all_plugins_disabled() {
        let config = Config {
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: false,
            },
            ..Config::default()
        };
        let plugins = PluginRegistry::create_enabled_plugins(&config);
        assert!(plugins.is_empty());
    }

    #[test]
    fn plugin_registry_creates_correct_plugin_types() {
        let config = Config::default();
        let plugins = PluginRegistry::create_enabled_plugins(&config);
        // Verify the plugins by checking metadata
        assert_eq!(plugins.len(), 3);
        assert_eq!(plugins[0].metadata().name, "FL Studio");
        assert_eq!(plugins[0].metadata().version, "0.1.0");
        assert_eq!(plugins[1].metadata().name, "Antigravity");
        assert_eq!(plugins[1].metadata().version, "0.1.0");
        assert_eq!(plugins[2].metadata().name, "OpenCode");
        assert_eq!(plugins[2].metadata().version, "0.1.0");
    }

    #[test]
    fn plugin_registry_antigravity_respects_config() {
        // Antigravity disabled but FL Studio enabled.
        let config = Config {
            plugins: presencehub_core::PluginConfig {
                flstudio: true,
                antigravity: false,
                opencode: false,
            },
            ..Config::default()
        };
        let plugins = PluginRegistry::create_enabled_plugins(&config);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].metadata().name, "FL Studio");

        // Only Antigravity enabled.
        let config = Config {
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: true,
                opencode: false,
            },
            ..Config::default()
        };
        let plugins = PluginRegistry::create_enabled_plugins(&config);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].metadata().name, "Antigravity");
    }

    #[test]
    fn plugin_registry_opencode_respects_config() {
        // OpenCode only enabled.
        let config = Config {
            plugins: presencehub_core::PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: true,
            },
            ..Config::default()
        };
        let plugins = PluginRegistry::create_enabled_plugins(&config);
        assert_eq!(plugins.len(), 1);
        assert_eq!(plugins[0].metadata().name, "OpenCode");
    }

    // -- OutputRegistry tests --------------------------------------------------

    #[test]
    fn output_registry_creates_console_when_enabled() {
        let config = Config::default(); // console = true by default
        let outputs = OutputRegistry::create_enabled_outputs(&config);
        assert_eq!(outputs.len(), 1);
    }

    #[test]
    fn output_registry_empty_when_all_disabled() {
        let config = Config {
            outputs: presencehub_core::OutputConfig {
                console: false,
                discord: false,
                discord_app_id: 0,
                discord_apps: std::collections::HashMap::new(),
            },
            ..Config::default()
        };
        let outputs = OutputRegistry::create_enabled_outputs(&config);
        assert!(outputs.is_empty());
    }

    #[test]
    fn output_registry_creates_discord_with_app_id() {
        let config = Config {
            outputs: presencehub_core::OutputConfig {
                console: true,
                discord: true,
                discord_app_id: 123456789,
                discord_apps: std::collections::HashMap::new(),
            },
            ..Config::default()
        };
        let outputs = OutputRegistry::create_enabled_outputs(&config);
        assert_eq!(outputs.len(), 2); // console + discord
    }

    #[test]
    fn output_registry_skips_discord_without_app_id() {
        // Discord enabled but app_id is 0 — should not create DiscordOutput
        let config = Config {
            outputs: presencehub_core::OutputConfig {
                console: true,
                discord: true,
                discord_app_id: 0,
                discord_apps: std::collections::HashMap::new(),
            },
            ..Config::default()
        };
        let outputs = OutputRegistry::create_enabled_outputs(&config);
        assert_eq!(outputs.len(), 1); // only console
    }

    #[test]
    fn output_registry_skips_discord_when_disabled() {
        let config = Config {
            outputs: presencehub_core::OutputConfig {
                console: true,
                discord: false,
                discord_app_id: 123456789,
                discord_apps: std::collections::HashMap::new(),
            },
            ..Config::default()
        };
        let outputs = OutputRegistry::create_enabled_outputs(&config);
        assert_eq!(outputs.len(), 1); // only console
    }

    #[test]
    fn output_registry_creates_discord_with_per_source_app_ids_only() {
        // Discord enabled with a zero default app id but per-source entries
        // must still create the output: each mapped source resolves its own
        // application ID.
        let config = Config {
            outputs: presencehub_core::OutputConfig {
                console: false,
                discord: true,
                discord_app_id: 0,
                discord_apps: std::collections::HashMap::from([(
                    "Antigravity".to_string(),
                    123456789,
                )]),
            },
            ..Config::default()
        };
        let outputs = OutputRegistry::create_enabled_outputs(&config);
        assert_eq!(outputs.len(), 1);
    }
}
