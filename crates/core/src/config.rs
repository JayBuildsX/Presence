//! Configuration loading and management.
//!
//! This module defines the [`Config`] struct and [`ConfigError`] type
//! used for loading and accessing runtime configuration.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

fn default_config_version() -> u32 {
    1
}

fn default_log_level() -> String {
    "info".to_string()
}

// ---------------------------------------------------------------------------
// Nested configuration sections
// ---------------------------------------------------------------------------

/// Runtime loop settings.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct RuntimeConfig {
    /// Polling interval in milliseconds.
    ///
    /// Values below [`Self::MIN_POLL_INTERVAL_MS`] are clamped up to the
    /// minimum when loaded so a misconfigured value cannot busy-spin the
    /// polling loop. (Values constructed in code are clamped again at the
    /// runtime boundary.)
    #[serde(deserialize_with = "deserialize_poll_interval_ms")]
    pub poll_interval_ms: u64,
}

impl RuntimeConfig {
    /// The smallest poll interval allowed, in milliseconds.
    ///
    /// A zero (or absurdly small) interval makes the runtime polling loop
    /// sleep for no meaningful time and busy-spin, wasting CPU. 250 ms is a
    /// safe floor: still far below the 1000 ms default for responsive
    /// updates, but bounded enough to prevent a busy loop.
    pub const MIN_POLL_INTERVAL_MS: u64 = 250;
}

/// Deserializes `poll_interval_ms`, clamping values below the safe minimum.
fn deserialize_poll_interval_ms<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = u64::deserialize(deserializer)?;
    Ok(value.max(RuntimeConfig::MIN_POLL_INTERVAL_MS))
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            poll_interval_ms: 1000,
        }
    }
}

/// Plugin enable/disable settings.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PluginConfig {
    /// Whether the FL Studio plugin is enabled.
    pub flstudio: bool,
    /// Whether the Antigravity plugin is enabled.
    pub antigravity: bool,
    /// Whether the OpenCode plugin is enabled.
    pub opencode: bool,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            flstudio: true,
            antigravity: true,
            opencode: true,
        }
    }
}

/// Output enable/disable settings.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Whether the console output is enabled.
    pub console: bool,
    /// Whether the Discord Rich Presence output is enabled.
    pub discord: bool,
    /// Discord application ID used when the displayed source has no entry in
    /// [`Self::discord_apps`]. A value of `0` (plus an empty
    /// [`Self::discord_apps`]) disables the Discord output.
    pub discord_app_id: u64,
    /// Optional per-source Discord application IDs, keyed by the engine
    /// source string (the plugin's metadata name, e.g. "Antigravity").
    /// Sources without an entry fall back to [`Self::discord_app_id`].
    #[serde(default)]
    pub discord_apps: HashMap<String, u64>,
}

/// How the Presence Engine chooses which active source owns the display.
///
/// The policy is generic and source-agnostic — it never refers to a
/// concrete application by name. When no supported application is active,
/// the outputs are cleared regardless of the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum OwnershipPolicy {
    /// The currently foreground/focused supported application owns the
    /// displayed presence. When no supported application is in the
    /// foreground, ownership falls back to deterministic registration order
    /// among the active sources.
    #[default]
    Foreground,
    /// Use deterministic source priority/order: the first-active source
    /// owns the display while it remains active.
    Fixed,
    /// The most recently active source owns the display.
    Recent,
}

impl std::str::FromStr for OwnershipPolicy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "foreground" => Ok(Self::Foreground),
            "fixed" => Ok(Self::Fixed),
            "recent" => Ok(Self::Recent),
            _ => Err(()),
        }
    }
}

impl<'de> serde::Deserialize<'de> for OwnershipPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Missing or unknown values fall back to the default policy safely,
        // so a typo in an existing configuration file never breaks startup.
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default())
    }
}

/// How the Presence Engine treats an unsupported application in the
/// foreground.
///
/// An "unsupported" foreground window is one that does not belong to any
/// registered plugin source (e.g. a browser, a file explorer, or the
/// desktop itself). The policy decides whether such a window may revoke the
/// presence shown by an active supported source. Like [`OwnershipPolicy`],
/// it is generic and source-agnostic — it never refers to a concrete
/// application by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub enum UnsupportedForegroundPolicy {
    /// Keep the currently displayed presence while it is still active. An
    /// unsupported foreground window never steals or clears the current
    /// owner; the owner is only replaced when it stops reporting activity
    /// (closes), at which point the next best active source is promoted.
    #[default]
    KeepLast,
}

impl std::str::FromStr for UnsupportedForegroundPolicy {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "keep_last" => Ok(Self::KeepLast),
            _ => Err(()),
        }
    }
}

impl<'de> serde::Deserialize<'de> for UnsupportedForegroundPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Missing or unknown values fall back to the default (`keep_last`)
        // so a typo in an existing configuration file never breaks startup.
        let raw = Option::<String>::deserialize(deserializer)?;
        Ok(raw
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or_default())
    }
}

/// Presence ownership settings.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct PresenceConfig {
    /// Which source owns the displayed presence when multiple are active.
    /// Defaults to [`OwnershipPolicy::Foreground`].
    pub ownership: OwnershipPolicy,
    /// How an unsupported foreground application affects the current owner.
    /// Defaults to [`UnsupportedForegroundPolicy::KeepLast`].
    pub unsupported_foreground: UnsupportedForegroundPolicy,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            console: true,
            discord: false,
            discord_app_id: 0,
            discord_apps: HashMap::new(),
        }
    }
}

/// Errors that can occur during configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Returned when the configuration file cannot be read.
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),

    /// Returned when the configuration file contains invalid TOML or
    /// does not match the expected schema.
    #[error("Failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Runtime configuration for PresenceHub.
///
/// # Fields
///
/// * `config_version` — Schema version for future migration support.
/// * `log_level` — Logging verbosity (e.g. "info", "debug", "warn").
/// * `runtime` — Runtime loop settings ([`RuntimeConfig`]).
/// * `plugins` — Plugin enable/disable settings ([`PluginConfig`]).
/// * `outputs` — Output enable/disable settings ([`OutputConfig`]).
/// * `presence` — Presence ownership settings ([`PresenceConfig`]).
///
/// # Example
///
/// ```toml
/// config_version = 1
/// log_level = "info"
///
/// [runtime]
/// poll_interval_ms = 1000
///
/// [plugins]
/// flstudio = true
///
/// [outputs]
/// console = true
/// discord = false
///
/// [presence]
/// ownership = "foreground"
/// unsupported_foreground = "keep_last"
/// ```
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Config {
    /// Schema version of this configuration file.
    #[serde(default = "default_config_version")]
    pub config_version: u32,

    /// Logging verbosity level.
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Runtime loop settings.
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// Plugin enable/disable settings.
    #[serde(default)]
    pub plugins: PluginConfig,

    /// Output enable/disable settings.
    #[serde(default)]
    pub outputs: OutputConfig,

    /// Presence ownership settings.
    #[serde(default)]
    pub presence: PresenceConfig,
}

impl Config {
    /// Loads configuration from a TOML file at the given path.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the file cannot be read.
    /// Returns [`ConfigError::Parse`] if the file contains invalid TOML
    /// or does not match the expected schema.
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use presencehub_core::Config;
    ///
    /// let config = Config::load("presencehub.toml").unwrap_or_default();
    /// ```
    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path.as_ref())?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            config_version: default_config_version(),
            log_level: default_log_level(),
            runtime: RuntimeConfig::default(),
            plugins: PluginConfig::default(),
            outputs: OutputConfig::default(),
            presence: PresenceConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn config_default_values() {
        let config = Config::default();
        assert_eq!(config.config_version, 1);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.runtime.poll_interval_ms, 1000);
        assert!(config.plugins.flstudio);
        assert!(config.plugins.antigravity);
        assert!(config.outputs.console);
        assert!(!config.outputs.discord);
        assert_eq!(config.outputs.discord_app_id, 0);
        assert!(config.outputs.discord_apps.is_empty());
    }

    #[test]
    fn config_deserializes_full_toml() {
        let toml_str = r#"
            config_version = 2
            log_level = "debug"

            [runtime]
            poll_interval_ms = 500

            [plugins]
            flstudio = false

            [outputs]
            console = true
            discord = true
            discord_app_id = 12345
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.config_version, 2);
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.runtime.poll_interval_ms, 500);
        assert!(!config.plugins.flstudio);
        assert!(config.outputs.console);
        assert!(config.outputs.discord);
        assert_eq!(config.outputs.discord_app_id, 12345);
    }

    #[test]
    fn config_discord_apps_defaults_to_empty_map() {
        // Backward compatibility: a config with only discord_app_id (no
        // [outputs.discord_apps] section) must deserialize to an empty map.
        let toml_str = r#"
            [outputs]
            console = true
            discord = true
            discord_app_id = 1533559059125637311
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.outputs.discord_app_id, 1533559059125637311);
        assert!(
            config.outputs.discord_apps.is_empty(),
            "absent [outputs.discord_apps] must default to an empty map"
        );
    }

    #[test]
    fn config_deserializes_discord_apps_per_source() {
        let toml_str = r#"
            [outputs]
            discord = true
            discord_app_id = 1533559059125637311

            [outputs.discord_apps]
            "Antigravity" = 200
            "FL Studio" = 300
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.outputs.discord_apps.get("Antigravity"), Some(&200));
        assert_eq!(config.outputs.discord_apps.get("FL Studio"), Some(&300));
        assert_eq!(config.outputs.discord_apps.len(), 2);
        // The default is retained alongside the per-source overrides.
        assert_eq!(config.outputs.discord_app_id, 1533559059125637311);
    }

    #[test]
    fn config_deserializes_full_toml_with_flstudio_enabled() {
        let toml_str = r#"
            [plugins]
            flstudio = true
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.plugins.flstudio);
    }

    #[test]
    fn config_deserializes_from_empty_toml() {
        let toml_str = "";
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.config_version, 1);
        assert_eq!(config.log_level, "info");
        assert_eq!(config.runtime.poll_interval_ms, 1000);
        assert!(config.plugins.flstudio);
        assert!(config.outputs.console);
    }

    #[test]
    fn config_deserializes_partial_toml() {
        let toml_str = r#"
            log_level = "warn"

            [runtime]
            poll_interval_ms = 250
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.config_version, 1);
        assert_eq!(config.log_level, "warn");
        assert_eq!(config.runtime.poll_interval_ms, 250);
        // Unspecified sections fall back to defaults
        assert!(config.plugins.flstudio);
        assert!(config.outputs.console);
    }

    #[test]
    fn config_deserializes_only_runtime_section() {
        let toml_str = r#"
            [runtime]
            poll_interval_ms = 5000
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.runtime.poll_interval_ms, 5000);
        assert!(config.plugins.flstudio);
        assert!(config.outputs.console);
    }

    #[test]
    fn config_clamps_poll_interval_zero_to_minimum() {
        // Regression: poll_interval_ms = 0 must not survive deserialization
        // or the runtime loop would busy-spin.
        let toml_str = r#"
            [runtime]
            poll_interval_ms = 0
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.runtime.poll_interval_ms,
            RuntimeConfig::MIN_POLL_INTERVAL_MS,
            "zero poll interval must be clamped up to the safe minimum"
        );
    }

    #[test]
    fn config_clamps_poll_interval_below_minimum() {
        // Excessively small values are clamped to the safe minimum too.
        let toml_str = r#"
            [runtime]
            poll_interval_ms = 5
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.runtime.poll_interval_ms,
            RuntimeConfig::MIN_POLL_INTERVAL_MS
        );
    }

    #[test]
    fn config_keeps_poll_interval_at_or_above_minimum() {
        // Values at or above the minimum are preserved unchanged.
        for value in [RuntimeConfig::MIN_POLL_INTERVAL_MS, 500, 1000] {
            let toml_str = format!("[runtime]\npoll_interval_ms = {}\n", value);
            let config: Config = toml::from_str(&toml_str).unwrap();
            assert_eq!(config.runtime.poll_interval_ms, value);
        }
    }

    #[test]
    fn runtime_config_min_poll_interval_is_sane() {
        // The minimum must be nonzero and well below the default, so it
        // actually prevents busy-looping without forcing an unresponsive
        // polling cadence.
        const { assert!(RuntimeConfig::MIN_POLL_INTERVAL_MS > 0) };
        assert!(RuntimeConfig::MIN_POLL_INTERVAL_MS < RuntimeConfig::default().poll_interval_ms);
    }

    #[test]
    fn config_deserializes_only_outputs_section() {
        let toml_str = r#"
            [outputs]
            discord = true
            discord_app_id = 999
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.outputs.discord);
        assert_eq!(config.outputs.discord_app_id, 999);
        assert!(config.outputs.console);
        assert!(config.plugins.flstudio);
        assert_eq!(config.runtime.poll_interval_ms, 1000);
    }

    #[test]
    fn config_serializes_and_deserializes() {
        let config = Config {
            config_version: 3,
            log_level: "trace".to_string(),
            runtime: RuntimeConfig {
                poll_interval_ms: 750,
            },
            plugins: PluginConfig {
                flstudio: false,
                antigravity: false,
                opencode: false,
            },
            outputs: OutputConfig {
                console: false,
                discord: true,
                discord_app_id: 424242,
                discord_apps: HashMap::new(),
            },
            presence: PresenceConfig::default(),
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.config_version, 3);
        assert_eq!(deserialized.log_level, "trace");
        assert_eq!(deserialized.runtime.poll_interval_ms, 750);
        assert!(!deserialized.plugins.flstudio);
        assert!(!deserialized.plugins.antigravity);
        assert!(!deserialized.outputs.console);
        assert!(deserialized.outputs.discord);
        assert_eq!(deserialized.outputs.discord_app_id, 424242);
    }

    #[test]
    fn config_load_from_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_presencehub_config.toml");
        let toml_str = r#"
            config_version = 2
            log_level = "debug"

            [runtime]
            poll_interval_ms = 300
        "#;
        std::fs::write(&path, toml_str).unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.config_version, 2);
        assert_eq!(config.log_level, "debug");
        assert_eq!(config.runtime.poll_interval_ms, 300);

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn config_load_from_nonexistent_file_returns_error() {
        let path = std::env::temp_dir().join("nonexistent_config.toml");
        let result = Config::load(&path);
        assert!(result.is_err());
    }

    #[test]
    fn config_load_from_invalid_toml_returns_error() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_invalid_config.toml");
        std::fs::write(&path, "invalid { toml [[[").unwrap();

        let result = Config::load(&path);
        assert!(result.is_err());

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn config_error_display() {
        let err = ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "file not found",
        ));
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn config_antigravity_defaults_enabled() {
        let config = Config::default();
        assert!(config.plugins.antigravity);
    }

    #[test]
    fn config_opencode_defaults_enabled() {
        let config = Config::default();
        assert!(config.plugins.opencode);
    }

    #[test]
    fn config_opencode_deserializes_explicitly() {
        let toml_str = r#"
            [plugins]
            flstudio = true
            antigravity = true
            opencode = false
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.plugins.opencode);
        assert!(config.plugins.flstudio);
        assert!(config.plugins.antigravity);
    }

    #[test]
    fn config_opencode_missing_in_toml_uses_default() {
        // An existing config file without the `opencode` key keeps the
        // default (enabled), so upgrading the schema does not silently
        // disable the new plugin.
        let toml_str = r#"
            [plugins]
            flstudio = true
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.plugins.opencode);
    }

    #[test]
    fn config_antigravity_deserializes_explicitly() {
        let toml_str = r#"
            [plugins]
            flstudio = true
            antigravity = false
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(!config.plugins.antigravity);
        assert!(config.plugins.flstudio);
    }

    #[test]
    fn config_antigravity_missing_in_toml_uses_default() {
        // An existing config file without the `antigravity` key keeps the default
        // (enabled), so upgrading the schema does not silently disable the
        // new plugin.
        let toml_str = r#"
            [plugins]
            flstudio = true
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert!(config.plugins.antigravity);
    }

    // -- Ownership policy -----------------------------------------------------

    #[test]
    fn ownership_defaults_to_foreground() {
        assert_eq!(
            Config::default().presence.ownership,
            OwnershipPolicy::Foreground
        );
        assert_eq!(OwnershipPolicy::default(), OwnershipPolicy::Foreground);
    }

    #[test]
    fn ownership_deserializes_fixed() {
        let toml_str = r#"
            [presence]
            ownership = "fixed"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.presence.ownership, OwnershipPolicy::Fixed);
    }

    #[test]
    fn ownership_deserializes_recent() {
        let toml_str = r#"
            [presence]
            ownership = "recent"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.presence.ownership, OwnershipPolicy::Recent);
    }

    #[test]
    fn ownership_is_case_insensitive() {
        assert_eq!(
            OwnershipPolicy::from_str("Foreground").unwrap(),
            OwnershipPolicy::Foreground
        );
        assert_eq!(
            OwnershipPolicy::from_str("FIXED").unwrap(),
            OwnershipPolicy::Fixed
        );
    }

    #[test]
    fn ownership_missing_in_toml_uses_default() {
        // Pre-existing config files without a [presence] section must keep
        // loading with the default (foreground) policy.
        let toml_str = r#"
            [runtime]
            poll_interval_ms = 500
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.presence.ownership, OwnershipPolicy::Foreground);
    }

    #[test]
    fn ownership_invalid_value_falls_back_safely() {
        // A typo in an existing config must not break startup; the default
        // policy is used instead.
        let toml_str = r#"
            [presence]
            ownership = "banana"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.presence.ownership, OwnershipPolicy::Foreground);
    }

    #[test]
    fn ownership_serializes_and_deserializes_round_trip() {
        let config = Config {
            presence: PresenceConfig {
                ownership: OwnershipPolicy::Recent,
                ..Default::default()
            },
            ..Config::default()
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.presence.ownership, OwnershipPolicy::Recent);
    }

    // -- Unsupported foreground policy -----------------------------------------

    #[test]
    fn unsupported_foreground_defaults_to_keep_last() {
        assert_eq!(
            Config::default().presence.unsupported_foreground,
            UnsupportedForegroundPolicy::KeepLast
        );
        assert_eq!(
            UnsupportedForegroundPolicy::default(),
            UnsupportedForegroundPolicy::KeepLast
        );
    }

    #[test]
    fn unsupported_foreground_deserializes_keep_last() {
        let toml_str = r#"
            [presence]
            unsupported_foreground = "keep_last"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.presence.unsupported_foreground,
            UnsupportedForegroundPolicy::KeepLast
        );
    }

    #[test]
    fn unsupported_foreground_missing_in_toml_uses_default() {
        // Pre-existing config files without the key must keep the default.
        let toml_str = r#"
            [presence]
            ownership = "fixed"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.presence.ownership, OwnershipPolicy::Fixed);
        assert_eq!(
            config.presence.unsupported_foreground,
            UnsupportedForegroundPolicy::KeepLast
        );
    }

    #[test]
    fn unsupported_foreground_invalid_value_falls_back_safely() {
        let toml_str = r#"
            [presence]
            unsupported_foreground = "revoke"
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.presence.unsupported_foreground,
            UnsupportedForegroundPolicy::KeepLast
        );
    }

    #[test]
    fn unsupported_foreground_is_case_insensitive() {
        use std::str::FromStr;
        assert_eq!(
            UnsupportedForegroundPolicy::from_str("keep_last").unwrap(),
            UnsupportedForegroundPolicy::KeepLast
        );
        assert_eq!(
            UnsupportedForegroundPolicy::from_str("KEEP_LAST").unwrap(),
            UnsupportedForegroundPolicy::KeepLast
        );
    }

    #[test]
    fn unsupported_foreground_serializes_round_trip() {
        let config = Config {
            presence: PresenceConfig {
                ownership: OwnershipPolicy::Foreground,
                unsupported_foreground: UnsupportedForegroundPolicy::KeepLast,
            },
            ..Config::default()
        };
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: Config = toml::from_str(&toml_str).unwrap();
        assert_eq!(
            deserialized.presence.unsupported_foreground,
            UnsupportedForegroundPolicy::KeepLast
        );
    }
}
