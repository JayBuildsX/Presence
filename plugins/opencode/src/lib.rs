//! PresenceHub OpenCode plugin.
//!
//! Observes the OpenCode desktop application and produces canonical
//! PresenceHub [`Activity`] values.
//!
//! # How it works
//!
//! 1. **Detection** — On each poll, the plugin checks whether the
//!    `OpenCode.exe` process is running. This is the reliable "is it
//!    active" signal: OpenCode is an Electron desktop app with an
//!    internal sidecar server that is only present while the app is open.
//!
//! 2. **State** — When OpenCode is running, the plugin reads the
//!    `opencode.window.<uuid>.dat` state file from the user-data directory
//!    (`%APPDATA%\ai.opencode.desktop`). The window state carries the
//!    active session's workspace directory and session title.
//!
//! 3. **Activity** — The plugin builds a conservative Activity:
//!    - `state`: `"Coding"` when a project is open, `"Idle"` otherwise
//!    - `details`: `"Project: <name>"` using only the last path component
//!      of the workspace directory (never the full filesystem path)
//!    - `application`: `Some("OpenCode")`
//!    - `metadata`: may include `"project"` (human-readable name)
//!
//! # Privacy
//!
//! The plugin deliberately never exposes:
//! - Full filesystem paths
//! - Usernames or home directories
//! - Session IDs (implementation detail)
//! - Prompt contents or chat messages
//! - API keys or tokens

use presencehub_core::activity::Activity;
use presencehub_plugin_host::{Plugin, PluginError, PluginMetadata, WindowIdentity};

mod detection;
mod state;

pub use detection::{
    data_dir, find_window_state_file, opencode_running, project_name_from_path, read_window_state,
};
pub use state::{parse_window_state, OpenCodeState};

/// The canonical application name for this plugin.
pub const APPLICATION_NAME: &str = "OpenCode";

/// The OpenCode plugin.
///
/// # Example
///
/// ```rust
/// use presencehub_opencode::OpenCodePlugin;
/// use presencehub_plugin_host::Plugin;
///
/// let mut plugin = OpenCodePlugin::new();
/// assert_eq!(plugin.metadata().name, "OpenCode");
/// ```
pub struct OpenCodePlugin {
    /// Plugin metadata.
    metadata: PluginMetadata,
    /// The last emitted activity, used for change detection.
    last_activity: Option<Activity>,
}

impl OpenCodePlugin {
    /// Creates a new OpenCode plugin.
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata::new(APPLICATION_NAME, "0.1.0"),
            last_activity: None,
        }
    }

    /// Handle the OpenCode application being absent.
    ///
    /// Forgets the previously emitted activity so the next identical
    /// activity is published again when the application reopens. Returns
    /// the poll error the runtime uses to end the session.
    fn application_not_found(&mut self) -> Result<Option<Activity>, PluginError> {
        self.last_activity = None;
        Err(PluginError::PollFailed(
            "OpenCode is not running".to_string(),
        ))
    }

    /// Observe a snapshot of OpenCode state and apply change detection.
    ///
    /// Builds a canonical Activity from the observed state and emits it
    /// only when it differs from the last emitted activity. Split out of
    /// [`Plugin::poll`] so the deduplication state machine can be tested
    /// without a running OpenCode instance.
    fn observe_state(&mut self, state: &OpenCodeState) -> Result<Option<Activity>, PluginError> {
        let activity = build_activity(state);

        // Only emit if the activity changed.
        if self.last_activity.as_ref() != Some(&activity) {
            self.last_activity = Some(activity.clone());
            return Ok(Some(activity));
        }

        Ok(None)
    }
}

impl Default for OpenCodePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for OpenCodePlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn window_identity(&self) -> Option<WindowIdentity> {
        // The OpenCode.exe process is the reliable window identity. The
        // Electron window class names are not stable across versions, so we
        // match on the process base name only.
        Some(WindowIdentity::new(
            ["OpenCode.exe".to_string()],
            Vec::<String>::new(),
        ))
    }

    fn init(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
        // Check whether OpenCode is actually running. If not, treat this as
        // application not found: the runtime will end the presence session.
        if !detection::opencode_running() {
            return self.application_not_found();
        }

        // OpenCode is running; attempt to read its state. If the state file
        // cannot be read (e.g. transient file lock), we still want to show
        // that OpenCode is active, but we cannot get the project name.
        let state = detection::read_window_state()
            .and_then(|content| state::parse_window_state(&content))
            .unwrap_or_default();

        self.observe_state(&state)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Build a canonical Activity from parsed OpenCode state.
fn build_activity(state: &OpenCodeState) -> Activity {
    // Extract a safe, human-readable project name from the workspace
    // directory. Only the last path component is used; full paths are never
    // exposed.
    let project_name = state
        .directory
        .as_deref()
        .and_then(detection::project_name_from_path);

    let (activity_state, details) = match &project_name {
        Some(name) => ("Coding".to_string(), Some(format!("Project: {}", name))),
        None => ("Idle".to_string(), None),
    };

    let mut metadata = std::collections::HashMap::new();
    metadata.insert("application".to_string(), APPLICATION_NAME.to_string());
    if let Some(name) = &project_name {
        metadata.insert("project".to_string(), name.clone());
    }

    Activity {
        state: activity_state,
        details,
        timestamps: None,
        application: Some(APPLICATION_NAME.to_string()),
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_metadata() {
        let plugin = OpenCodePlugin::new();
        assert_eq!(plugin.metadata().name, "OpenCode");
        assert_eq!(plugin.metadata().version, "0.1.0");
    }

    #[test]
    fn application_identity_is_opencode() {
        let state = open_with_state();
        let activity = build_activity(&state);
        assert_eq!(activity.application.as_deref(), Some("OpenCode"));
    }

    #[test]
    fn plugin_init_and_shutdown() {
        let mut plugin = OpenCodePlugin::new();
        assert!(plugin.init().is_ok());
        assert!(plugin.shutdown().is_ok());
    }

    #[test]
    fn activity_with_project() {
        let state = open_with_state();
        let activity = build_activity(&state);
        assert_eq!(activity.state, "Coding");
        assert_eq!(activity.details, Some("Project: MyProject".to_string()));
        assert_eq!(
            activity.metadata.get("project"),
            Some(&"MyProject".to_string())
        );
    }

    #[test]
    fn activity_without_project() {
        let state = OpenCodeState::default();
        let activity = build_activity(&state);
        assert_eq!(activity.state, "Idle");
        assert!(activity.details.is_none());
        assert!(!activity.metadata.contains_key("project"));
    }

    #[test]
    fn full_path_is_not_exposed() {
        let state = open_with_state();
        let activity = build_activity(&state);
        let details = activity.details.unwrap();
        assert!(!details.contains("Users"));
        assert!(!details.contains("C:\\"));
        assert_eq!(details, "Project: MyProject");
    }

    // Helper to build a test OpenCode state.
    fn open_with_state() -> OpenCodeState {
        open_with_dir("C:\\Users\\HP\\Desktop\\MyProject")
    }

    fn open_with_dir(dir: &str) -> OpenCodeState {
        OpenCodeState {
            session_id: Some("ses_123".to_string()),
            directory: Some(dir.to_string()),
            title: Some("MyProject".to_string()),
        }
    }

    #[test]
    fn plugin_suppresses_identical_activity() {
        let mut plugin = OpenCodePlugin::new();
        let state = open_with_state();

        assert!(plugin.observe_state(&state).unwrap().is_some());
        // Identical poll -> no new activity.
        assert!(plugin.observe_state(&state).unwrap().is_none());
    }

    #[test]
    fn plugin_republishes_after_application_exit() {
        let mut plugin = OpenCodePlugin::new();
        let state = open_with_state();

        assert!(plugin.observe_state(&state).unwrap().is_some());
        assert!(plugin.observe_state(&state).unwrap().is_none());

        // Application exits.
        assert!(plugin.application_not_found().is_err());

        // Reopens -> activity is published again.
        assert!(plugin.observe_state(&state).unwrap().is_some());
    }

    #[test]
    fn plugin_handles_missing_state() {
        let mut plugin = OpenCodePlugin::new();
        let state = OpenCodeState::default();
        let activity = plugin
            .observe_state(&state)
            .unwrap()
            .expect("should publish");
        assert_eq!(activity.state, "Idle");
    }

    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenCodePlugin>();
    }
}
