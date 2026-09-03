//! Antigravity Plugin for PresenceHub.
//!
//! Observes the Antigravity AI-assisted IDE and reports real-time agent activity
//! or workspace status to PresenceHub.
//!
//! Implemented following `A-Gift-Of-Flame/antigravity-rpc`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use presencehub_core::activity::{Activity, ActivityTimestamps};
use presencehub_plugin_host::{Plugin, PluginError, PluginMetadata, WindowIdentity};

pub mod detection;
pub mod state;

pub use detection::{default_brain_dir, find_active_conversation, ActiveConversation};
pub use state::{parse_task_md, AntigravityState};

/// The canonical application name for this plugin.
pub const APPLICATION_NAME: &str = "Antigravity";

/// Large image asset key on Discord.
pub const ASSET_LARGE_IMAGE: &str = "antigravity_logo";

/// Large image hover text on Discord.
pub const ASSET_LARGE_TEXT: &str = "Antigravity";

/// Small image asset key for active agent on Discord.
pub const ASSET_SMALL_IMAGE_AGENT: &str = "robot";

/// Small image hover text for active agent on Discord.
pub const ASSET_SMALL_TEXT_AGENT: &str = "Agent Working";

/// The Antigravity plugin.
pub struct AntigravityPlugin {
    metadata: PluginMetadata,
    last_activity: Option<Activity>,
    session_start_time: Option<i64>,
    current_conversation_id: Option<String>,
    custom_brain_dir: Option<PathBuf>,
}

impl AntigravityPlugin {
    /// Creates a new Antigravity plugin with default system brain directory resolution.
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata::new(APPLICATION_NAME, "0.1.0"),
            last_activity: None,
            session_start_time: None,
            current_conversation_id: None,
            custom_brain_dir: None,
        }
    }

    /// Creates a plugin with a custom brain directory (useful for testing).
    pub fn with_brain_dir(brain_dir: PathBuf) -> Self {
        Self {
            metadata: PluginMetadata::new(APPLICATION_NAME, "0.1.0"),
            last_activity: None,
            session_start_time: None,
            current_conversation_id: None,
            custom_brain_dir: Some(brain_dir),
        }
    }

    /// Returns the active brain directory.
    fn brain_dir(&self) -> Option<PathBuf> {
        self.custom_brain_dir
            .clone()
            .or_else(detection::default_brain_dir)
    }

    /// Handle application absence.
    ///
    /// Forgets the previous activity and resets the session start timestamp so
    /// a new session begins cleanly upon application restart.
    fn application_not_found(&mut self) -> Result<Option<Activity>, PluginError> {
        self.last_activity = None;
        self.session_start_time = None;
        self.current_conversation_id = None;
        Err(PluginError::PollFailed(
            "Antigravity is not active".to_string(),
        ))
    }

    /// Builds the Activity representation and applies deduplication.
    pub fn observe_state(
        &mut self,
        parsed_state: &AntigravityState,
    ) -> Result<Option<Activity>, PluginError> {
        // Prefer the real Antigravity process start time so the elapsed timer
        // counts since the application launched, not since PresenceHub
        // started tracking it. The stored session time is only a fallback
        // for when the process start cannot be read.
        let start_time = presencehub_core::process::process_start_unix(&["antigravity.exe"])
            .unwrap_or_else(|| match self.session_start_time {
                Some(ts) => ts,
                None => {
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64;
                    self.session_start_time = Some(now);
                    now
                }
            });

        let activity = build_activity(parsed_state, start_time);

        if self.last_activity.as_ref() != Some(&activity) {
            self.last_activity = Some(activity.clone());
            return Ok(Some(activity));
        }

        Ok(None)
    }
}

impl Default for AntigravityPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for AntigravityPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn window_identity(&self) -> Option<WindowIdentity> {
        Some(WindowIdentity::new(
            ["antigravity.exe".to_string()],
            Vec::<String>::new(),
        ))
    }

    fn init(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
        let is_running = detection::antigravity_running();
        let brain_dir = self.brain_dir();

        // Check if there is an active conversation within 5 minutes
        let active_conv = brain_dir.as_deref().and_then(|dir| {
            detection::find_active_conversation(dir, detection::DEFAULT_STALE_THRESHOLD)
        });

        // If the process is not running and no active conversation exists, Antigravity is inactive
        if !is_running && active_conv.is_none() {
            return self.application_not_found();
        }

        // A fresh conversation means the agent recently did something: parse
        // the plan file for titles when there is one, and always report
        // working. Freshness itself is the working signal; staleness falls
        // back to idle below.
        let state = if let Some(ref conv) = active_conv {
            self.current_conversation_id = Some(conv.conversation_id.clone());
            let mut state = match conv.task_file.as_ref() {
                Some(plan) => match std::fs::read_to_string(plan) {
                    Ok(content) => {
                        if plan
                            .file_name()
                            .is_some_and(|name| name == "implementation_plan.md")
                        {
                            state::parse_plan_md(&content)
                        } else {
                            state::parse_task_md(&content)
                        }
                    }
                    Err(_) => AntigravityState::default(),
                },
                None => AntigravityState::default(),
            };
            state.project_name = conv.project_name.clone();
            state.is_agent_working = true;
            state
        } else {
            self.current_conversation_id = None;
            AntigravityState::default()
        };

        self.observe_state(&state)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Builds a canonical [`Activity`] from [`AntigravityState`].
pub fn build_activity(state: &AntigravityState, start_timestamp: i64) -> Activity {
    let mut metadata = HashMap::new();
    metadata.insert("large_image".to_string(), ASSET_LARGE_IMAGE.to_string());
    metadata.insert("large_text".to_string(), ASSET_LARGE_TEXT.to_string());

    let (activity_state, details) = if state.is_agent_working {
        metadata.insert(
            "small_image".to_string(),
            ASSET_SMALL_IMAGE_AGENT.to_string(),
        );
        metadata.insert("small_text".to_string(), ASSET_SMALL_TEXT_AGENT.to_string());

        if let Some(ref project) = state.project_name {
            let st = state
                .active_task
                .as_ref()
                .or(state.task_name.as_ref())
                .cloned()
                .unwrap_or_else(|| "Agent Active".to_string());
            (st, Some(format!("Project: {}", project)))
        } else {
            let st = state
                .task_name
                .clone()
                .unwrap_or_else(|| "Agent Active".to_string());
            let mut dt = state
                .active_task
                .clone()
                .or_else(|| Some("Agent Working".to_string()));

            // Never allow state and details to show identical text
            if dt.as_ref() == Some(&st) {
                dt = state
                    .main_header
                    .clone()
                    .filter(|m| m != &st)
                    .or_else(|| Some("Agent Working".to_string()));
            }

            (st, dt)
        }
    } else {
        ("In Antigravity".to_string(), Some("Idle".to_string()))
    };

    Activity {
        state: activity_state,
        details,
        timestamps: Some(ActivityTimestamps {
            start: Some(start_timestamp),
            end: None,
        }),
        application: Some(APPLICATION_NAME.to_string()),
        metadata,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_metadata() {
        let plugin = AntigravityPlugin::new();
        assert_eq!(plugin.metadata().name, "Antigravity");
        assert_eq!(plugin.metadata().version, "0.1.0");
    }

    #[test]
    fn plugin_init_and_shutdown() {
        let mut plugin = AntigravityPlugin::new();
        assert!(plugin.init().is_ok());
        assert!(plugin.shutdown().is_ok());
    }

    #[test]
    fn window_identity_matches_antigravity_process() {
        let plugin = AntigravityPlugin::new();
        let identity = plugin.window_identity().unwrap();
        assert!(identity
            .process_names
            .contains(&"antigravity.exe".to_string()));
    }

    #[test]
    fn build_activity_agent_working() {
        let state = AntigravityState {
            main_header: Some("Goal".to_string()),
            section_header: Some("Feature Implementation".to_string()),
            active_task: Some("Add Antigravity plugin".to_string()),
            task_name: Some("Feature Implementation".to_string()),
            project_name: None,
            is_agent_working: true,
        };

        let activity = build_activity(&state, 1700000000);
        assert_eq!(activity.application.as_deref(), Some("Antigravity"));
        assert_eq!(activity.state, "Feature Implementation");
        assert_eq!(activity.details.as_deref(), Some("Add Antigravity plugin"));
        assert_eq!(activity.timestamps.unwrap().start, Some(1700000000));
        assert_eq!(
            activity.metadata.get("large_image"),
            Some(&"antigravity_logo".to_string())
        );
        assert_eq!(
            activity.metadata.get("small_image"),
            Some(&"robot".to_string())
        );
        assert_eq!(
            activity.metadata.get("small_text"),
            Some(&"Agent Working".to_string())
        );
    }

    #[test]
    fn build_activity_agent_idle() {
        let state = AntigravityState {
            main_header: Some("Goal".to_string()),
            section_header: None,
            active_task: None,
            task_name: None,
            project_name: None,
            is_agent_working: false,
        };

        let activity = build_activity(&state, 1700000000);
        assert_eq!(activity.application.as_deref(), Some("Antigravity"));
        assert_eq!(activity.state, "In Antigravity");
        assert_eq!(activity.details.as_deref(), Some("Idle"));
        assert_eq!(activity.timestamps.unwrap().start, Some(1700000000));
        assert_eq!(
            activity.metadata.get("large_image"),
            Some(&"antigravity_logo".to_string())
        );
        assert!(!activity.metadata.contains_key("small_image"));
    }

    #[test]
    fn timestamp_persists_across_task_updates() {
        let mut plugin = AntigravityPlugin::new();

        let state1 = AntigravityState {
            main_header: Some("Goal".to_string()),
            section_header: None,
            active_task: Some("Task 1".to_string()),
            task_name: Some("Goal".to_string()),
            project_name: None,
            is_agent_working: true,
        };

        let act1 = plugin.observe_state(&state1).unwrap().unwrap();
        let start_time1 = act1.timestamps.unwrap().start;
        assert!(start_time1.is_some());

        // Update to Task 2 in the same session
        let state2 = AntigravityState {
            main_header: Some("Goal".to_string()),
            section_header: None,
            active_task: Some("Task 2".to_string()),
            task_name: Some("Goal".to_string()),
            project_name: None,
            is_agent_working: true,
        };

        let act2 = plugin.observe_state(&state2).unwrap().unwrap();
        let start_time2 = act2.timestamps.unwrap().start;
        assert_eq!(start_time1, start_time2, "timestamp must be preserved");
    }

    #[test]
    fn plugin_suppresses_identical_activity_and_republishes_on_reopen() {
        let mut plugin = AntigravityPlugin::new();

        let state = AntigravityState {
            main_header: Some("Goal".to_string()),
            section_header: None,
            active_task: Some("Task 1".to_string()),
            task_name: Some("Goal".to_string()),
            project_name: None,
            is_agent_working: true,
        };

        let first = plugin.observe_state(&state).unwrap();
        assert!(first.is_some());

        // Same state -> suppressed
        let second = plugin.observe_state(&state).unwrap();
        assert!(second.is_none());

        // App closed / not found
        assert!(plugin.application_not_found().is_err());

        // Reopened -> emits again
        let reopened = plugin.observe_state(&state).unwrap();
        assert!(reopened.is_some());
    }

    #[test]
    fn build_activity_with_project_name() {
        let state = AntigravityState {
            main_header: Some("Refactor PresenceHUB".to_string()),
            section_header: Some("Verification Plan".to_string()),
            active_task: Some("Verification Plan".to_string()),
            task_name: Some("Verification Plan".to_string()),
            project_name: Some("PresenceHUB".to_string()),
            is_agent_working: true,
        };

        let activity = build_activity(&state, 1700000000);
        assert_eq!(activity.state, "Verification Plan");
        assert_eq!(activity.details.as_deref(), Some("Project: PresenceHUB"));
    }

    #[test]
    fn build_activity_never_duplicates_state_and_details() {
        let state = AntigravityState {
            main_header: Some("Overall Goal".to_string()),
            section_header: Some("Step 1".to_string()),
            active_task: Some("Step 1".to_string()),
            task_name: Some("Step 1".to_string()),
            project_name: None,
            is_agent_working: true,
        };

        let activity = build_activity(&state, 1700000000);
        assert_eq!(activity.state, "Step 1");
        assert_ne!(activity.details.as_deref(), Some("Step 1"));
        assert_eq!(activity.details.as_deref(), Some("Overall Goal"));
    }
}
