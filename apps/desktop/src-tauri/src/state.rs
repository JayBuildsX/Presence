//! GUI live-state snapshot.
//!
//! The view model the desktop GUI renders. It is built exclusively from
//! backend state ([`Runtime::snapshot`](crate::runtime::Runtime::snapshot)):
//! plugin enablement from the configuration, activity and ownership from
//! the [`PresenceEngine`](presencehub_core::output::PresenceEngine), and
//! connection status from the outputs. The frontend never reconstructs
//! any of this itself.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tauri_plugin_global_shortcut::Shortcut;

/// Shared GUI state: the runtime behind an async mutex.
///
/// Commands lock it briefly per call; the background polling task locks it
/// once per poll iteration.
#[derive(Clone)]
pub struct AppState {
    pub runtime: Arc<tokio::sync::Mutex<crate::runtime::Runtime>>,
}

/// Shared action mapping for dynamically registered global shortcuts.
#[derive(Clone, Default)]
pub struct ShortcutMap(pub Arc<RwLock<HashMap<Shortcut, String>>>);

/// Per-plugin row in the GUI.
#[derive(Debug, Clone, Serialize)]
pub struct PluginView {
    /// Engine source name, e.g. `"FL Studio"`.
    pub name: String,
    /// Whether the plugin is enabled (configuration + runtime toggle).
    pub enabled: bool,
    /// Whether the plugin currently has an active session.
    pub active: bool,
    /// Short activity summary (the activity's state line), if active.
    pub summary: Option<String>,
}

/// The activity currently published to Discord.
#[derive(Debug, Clone, Serialize)]
pub struct PresenceView {
    /// Engine source owning the display.
    pub source: String,
    /// Primary line, e.g. `"Editing lib.rs"`.
    pub state: String,
    /// Secondary line, e.g. `"Project: PresenceHUB"`.
    pub details: Option<String>,
}

/// Everything the GUI needs for one render pass.
#[derive(Debug, Clone, Serialize)]
pub struct LiveState {
    /// Global pause flag.
    pub paused: bool,
    /// Current polling interval in milliseconds.
    pub poll_interval_ms: u64,
    /// Whether any output currently reports a live connection.
    pub discord_connected: bool,
    /// Source owning the display, if any.
    pub owner: Option<String>,
    /// Currently published activity, if any.
    pub current: Option<PresenceView>,
    /// Manually pinned source overriding foreground window switching.
    pub pinned_source: Option<String>,
    /// One row per known plugin, in display order.
    pub plugins: Vec<PluginView>,
}
