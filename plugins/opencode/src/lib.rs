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
//! 3. **File** — OpenCode does not persist an explicit "active file" flag.
//!    The plugin approximates the current file using the per-session
//!    file-view state in `opencode.workspace.<project>.dat`: a file that
//!    first appears in the active session's file-view since the last poll
//!    is treated as the file the user most recently opened/selected.
//!
//! 4. **Activity** — The plugin builds a conservative Activity:
//!    - `state`: `"Editing <file.ext>"` when a file is being viewed,
//!      `"Coding"` when only a project is open, `"Idle"` otherwise
//!    - `details`: `"Project: <name>"` using only the last path component
//!      of the workspace directory (never the full filesystem path)
//!    - `metadata`:
//!      - `large_image`/`large_text`: the file type's Discord application
//!        asset key and display name (e.g. `rust` / `"Rust"`), shown as the
//!        large image and its hover text. Icons are resolved by
//!        [`FileIconResolver`]: exact file name first (`Cargo.toml`,
//!        `package.json`, ...), then extension, then no icon. Keys must be
//!        uploaded to the Discord application; there are no external URLs.
//!      - `project`: human-readable project name
//!    - `application`: deliberately `None` so the application identity never
//!      overrides the file type's label in the large-image hover
//!
//! The Discord application (ID 1273940066603106328, the vsc-presence app)
//! hosts the icon assets: `large_image` is a short asset key, exactly like
//! `brkpoint/VSCode-Discord-RPC`. No small image is set for now; one will be
//! added once our own assets are uploaded to our own application.
//!
//! # Privacy
//!
//! The plugin deliberately never exposes:
//! - Full filesystem paths
//! - Usernames or home directories
//! - Session IDs (implementation detail)
//! - Prompt contents or chat messages
//! - API keys or tokens

use presencehub_core::activity::{Activity, ActivityTimestamps};
use presencehub_plugin_host::{Plugin, PluginError, PluginMetadata, WindowIdentity};

mod detection;
mod file_icons;
mod state;

pub use detection::{
    data_dir, find_window_state_file, opencode_running, project_name_from_path, read_file_view,
    read_window_state, workspace_state_files,
};
pub use file_icons::{file_name_from_path, FileIconResolver, IconResolution};
pub use state::{parse_file_view, parse_window_state, OpenCodeState};

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
    /// The session the file-view heuristic is tracking, if any.
    active_session: Option<String>,
    /// The files seen so far in the active session (insertion = discovery).
    viewed_files: Vec<String>,
    /// The current file the user is working on, per the file-view heuristic.
    current_file: Option<String>,
}

impl OpenCodePlugin {
    /// Creates a new OpenCode plugin.
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata::new(APPLICATION_NAME, "0.1.0"),
            last_activity: None,
            active_session: None,
            viewed_files: Vec::new(),
            current_file: None,
        }
    }

    /// Handle the OpenCode application being absent.
    ///
    /// Forgets the previously emitted activity so the next identical
    /// activity is published again when the application reopens, and resets
    /// the file-view heuristic so a fresh session starts clean. Returns
    /// the poll error the runtime uses to end the session.
    fn application_not_found(&mut self) -> Result<Option<Activity>, PluginError> {
        self.last_activity = None;
        self.active_session = None;
        self.viewed_files.clear();
        self.current_file = None;
        Err(PluginError::PollFailed(
            "OpenCode is not running".to_string(),
        ))
    }

    /// Track the currently active file using the file-view heuristic.
    ///
    /// OpenCode does not persist an explicit "active file" flag. The closest
    /// signal is the per-session file-view state: the set of files the user
    /// has viewed in a session. A file that appears for the first time since
    /// the last poll is treated as the file the user has most recently
    /// opened/selected. When the active session changes, tracking restarts.
    fn track_active_file(&mut self, state: &OpenCodeState) {
        let Some(session_id) = state.session_id.as_deref() else {
            return;
        };

        // Reset per-session tracking when the active session changes.
        if self.active_session.as_deref() != Some(session_id) {
            self.active_session = Some(session_id.to_string());
            self.viewed_files.clear();
            self.current_file = None;
        }

        // A transient read failure must not clear the current file.
        let Some(files) = detection::read_file_view(session_id) else {
            return;
        };

        if let Some(new_file) = advance_file_tracking(&mut self.viewed_files, &files) {
            self.current_file = Some(new_file);
        }
    }

    /// Observe a snapshot of OpenCode state and apply change detection.
    ///
    /// Builds a canonical Activity from the observed state and emits it
    /// only when it differs from the last emitted activity. Split out of
    /// [`Plugin::poll`] so the deduplication state machine can be tested
    /// without a running OpenCode instance.
    fn observe_state(
        &mut self,
        state: &OpenCodeState,
        file: Option<&str>,
    ) -> Result<Option<Activity>, PluginError> {
        let activity = build_activity(state, file);

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
        self.last_activity = None;
        self.current_file = None;
        self.viewed_files.clear();
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

        self.track_active_file(&state);

        let current_file = self.current_file.clone();
        self.observe_state(&state, current_file.as_deref())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        self.last_activity = None;
        self.current_file = None;
        self.viewed_files.clear();
        Ok(())
    }

    fn reset(&mut self) {
        self.last_activity = None;
        self.current_file = None;
        self.viewed_files.clear();
    }
}

/// Advance the file-view heuristic over a poll.
///
/// Appends any newly discovered viewed files to `viewed_files` and returns
/// the most recently discovered file, which becomes the current file.
/// Returns `None` when no new file appeared since the last poll. Between
/// polls the user normally opens files one at a time, so the returned file
/// is exact in the common case.
fn advance_file_tracking(viewed_files: &mut Vec<String>, files: &[String]) -> Option<String> {
    let mut new_files: Vec<String> = files
        .iter()
        .filter(|path| !viewed_files.contains(path))
        .cloned()
        .collect();

    if new_files.is_empty() {
        return None;
    }

    new_files.sort();
    viewed_files.extend(new_files.iter().cloned());
    new_files.last().cloned()
}

/// Build a canonical Activity from parsed OpenCode state and the current
/// file (if any).
///
/// When the user is viewing a file, the primary line is the file name with
/// its extension ("Editing main.rs"), the file type's asset key and label
/// drive the large image/hover, and no small image is set. The project name
/// never exposes a full filesystem path.
fn build_activity(state: &OpenCodeState, file: Option<&str>) -> Activity {
    // Extract a safe, human-readable project name from the workspace
    // directory. Only the last path component is used; full paths are never
    // exposed.
    let project_name = state
        .directory
        .as_deref()
        .and_then(detection::project_name_from_path);

    let file_name = file.and_then(file_icons::file_name_from_path);
    let resolution = file_name
        .as_deref()
        .map(|name| FILE_ICON_RESOLVER.resolve(name));

    let (activity_state, details) = match &file_name {
        Some(name) => (
            format!("Editing {}", name),
            Some(format!(
                "Project: {}",
                project_name.as_deref().unwrap_or("Unknown")
            )),
        ),
        None => match &project_name {
            Some(name) => ("Coding".to_string(), Some(format!("Project: {}", name))),
            None => ("Idle".to_string(), None),
        },
    };

    let mut metadata = std::collections::HashMap::new();

    // Large image = the file type's Discord application asset key, hover =
    // the file type name. The metadata map is built from scratch on every
    // poll, so a file whose icon cannot be resolved yields an activity
    // *without* `large_image`/`large_text`: the previous file's icon is
    // never carried over (no stale icons). No small image is set, matching
    // the reference method.
    if let Some(res) = resolution.as_ref() {
        if let (Some(key), Some(label)) = (res.image_key, res.label) {
            metadata.insert("large_image".to_string(), key.to_string());
            metadata.insert("large_text".to_string(), label.to_string());
        }
    }

    // `application` is deliberately left None so the application identity
    // does not override the file type's label in the large-image hover.
    if let Some(name) = &project_name {
        metadata.insert("project".to_string(), name.clone());
    }

    // Stamp the real OpenCode process start time so the elapsed timer counts
    // since the application launched, not since PresenceHub started tracking
    // it. When unavailable the engine falls back to its session timer.
    let timestamps =
        presencehub_core::process::process_start_unix(&[detection::OPENCODE_PROCESS_NAME]).map(
            |start| ActivityTimestamps {
                start: Some(start),
                end: None,
            },
        );

    Activity {
        state: activity_state,
        details,
        timestamps,
        application: None,
        metadata,
    }
}

/// The resolver used for every poll. Stateless and const-constructible.
const FILE_ICON_RESOLVER: FileIconResolver = FileIconResolver::new();

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
        // No small image is set (matching the reference method). The
        // `application` field must stay None so the application identity
        // never overrides the file type's label in the large-image hover.
        let state = open_with_state();
        let activity = build_activity(&state, None);
        assert_eq!(activity.application, None);
        assert!(!activity.metadata.contains_key("small_image"));
        assert!(!activity.metadata.contains_key("small_text"));
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
        let activity = build_activity(&state, None);
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
        let activity = build_activity(&state, None);
        assert_eq!(activity.state, "Idle");
        assert!(activity.details.is_none());
        assert!(!activity.metadata.contains_key("project"));
    }

    #[test]
    fn activity_with_file_shows_name_extension_and_language() {
        // The primary line is "Editing <file.ext>"; the large image uses the
        // file type's asset key and the hover shows the file type name. No
        // small image is set.
        let state = open_with_state();
        let activity = build_activity(&state, Some("crates/core/src/main.rs"));
        assert_eq!(activity.state, "Editing main.rs");
        assert_eq!(activity.details, Some("Project: MyProject".to_string()));
        assert_eq!(
            activity.metadata.get("large_image"),
            Some(&"rust".to_string())
        );
        assert_eq!(
            activity.metadata.get("large_text"),
            Some(&"Rust".to_string())
        );
        assert!(!activity.metadata.contains_key("small_image"));
        assert!(!activity.metadata.contains_key("small_text"));
    }

    #[test]
    fn activity_with_unknown_file_type_shows_name_without_icon() {
        // An unrecognized extension resolves to no icon, and the activity
        // must not carry the previous file's icon (never stale).
        let state = open_with_state();
        let activity = build_activity(&state, Some("src/notes.xyz"));
        assert_eq!(activity.state, "Editing notes.xyz");
        assert!(!activity.metadata.contains_key("large_image"));
        assert!(!activity.metadata.contains_key("large_text"));
    }

    #[test]
    fn files_without_a_hosted_key_carry_no_icon() {
        // Dockerfiles, lockfiles, and config formats without a hosted asset
        // key (TOML, YAML, SQL, SVG, ...) yield no large image.
        let state = open_with_state();
        for path in [
            "Dockerfile",
            "Makefile",
            "src/config.toml",
            "src/config.yaml",
            "src/query.sql",
            "src/image.svg",
            "src/notes.txt",
        ] {
            let activity = build_activity(&state, Some(path));
            assert!(
                !activity.metadata.contains_key("large_image"),
                "{path}: must have no large image"
            );
            assert!(!activity.metadata.contains_key("large_text"));
        }
    }

    #[test]
    fn icon_transitions_never_leave_a_stale_image() {
        // The regression the resolver must prevent: switching from lib.rs to
        // index.html replaces the Rust key, and a subsequent unsupported
        // file clears the large image entirely instead of keeping the old one.
        let state = open_with_state();
        let cases = [
            ("src/lib.rs", Some("rust")),
            ("src/index.html", Some("html")),
            ("src/main.ts", Some("typescript")),
            ("src/component.tsx", Some("tsx")),
            ("src/data.json", Some("json")),
            ("src/script.py", Some("python")),
            ("src/unknown.xyz", None),
        ];
        for (path, expected_key) in cases {
            let activity = build_activity(&state, Some(path));
            match expected_key {
                Some(key) => {
                    assert_eq!(
                        activity.metadata.get("large_image"),
                        Some(&key.to_string()),
                        "{path}: expected asset key {key}"
                    );
                }
                None => {
                    assert!(
                        !activity.metadata.contains_key("large_image"),
                        "{path}: stale large image must be cleared"
                    );
                    assert!(!activity.metadata.contains_key("large_text"));
                }
            }
        }
    }

    #[test]
    fn consecutive_files_never_reuse_the_previous_icon() {
        let state = open_with_state();
        let rust = build_activity(&state, Some("src/lib.rs"));
        let html = build_activity(&state, Some("src/index.html"));
        assert_ne!(
            rust.metadata.get("large_image"),
            html.metadata.get("large_image"),
            "each file type must carry its own icon"
        );
    }

    #[test]
    fn full_path_is_not_exposed() {
        let state = open_with_state();
        let activity = build_activity(&state, None);
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
            files: Vec::new(),
        }
    }

    #[test]
    fn plugin_suppresses_identical_activity() {
        let mut plugin = OpenCodePlugin::new();
        let state = open_with_state();

        assert!(plugin.observe_state(&state, None).unwrap().is_some());
        // Identical poll -> no new activity.
        assert!(plugin.observe_state(&state, None).unwrap().is_none());
    }

    #[test]
    fn plugin_republishes_after_application_exit() {
        let mut plugin = OpenCodePlugin::new();
        let state = open_with_state();

        assert!(plugin.observe_state(&state, None).unwrap().is_some());
        assert!(plugin.observe_state(&state, None).unwrap().is_none());

        // Application exits.
        assert!(plugin.application_not_found().is_err());

        // Reopens -> activity is published again.
        assert!(plugin.observe_state(&state, None).unwrap().is_some());
    }

    #[test]
    fn plugin_republishes_when_file_changes() {
        let mut plugin = OpenCodePlugin::new();
        let state = open_with_state();

        assert!(plugin
            .observe_state(&state, Some("src/main.rs"))
            .unwrap()
            .is_some());
        // Same file -> deduped.
        assert!(plugin
            .observe_state(&state, Some("src/main.rs"))
            .unwrap()
            .is_none());
        // Different file -> published again.
        assert!(plugin
            .observe_state(&state, Some("src/app.ts"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn plugin_handles_missing_state() {
        let mut plugin = OpenCodePlugin::new();
        let state = OpenCodeState::default();
        let activity = plugin
            .observe_state(&state, None)
            .unwrap()
            .expect("should publish");
        assert_eq!(activity.state, "Idle");
    }

    // -- File-view heuristic ----------------------------------------------------

    #[test]
    fn advance_tracking_picks_newly_discovered_file() {
        let mut viewed = Vec::new();
        let files = vec!["a.rs".to_string(), "b.rs".to_string()];
        let current = advance_file_tracking(&mut viewed, &files);
        assert_eq!(current.as_deref(), Some("b.rs"));
        assert_eq!(viewed.len(), 2);
    }

    #[test]
    fn advance_tracking_returns_none_without_new_files() {
        let mut viewed = vec!["a.rs".to_string(), "b.rs".to_string()];
        let files = vec!["a.rs".to_string(), "b.rs".to_string()];
        let current = advance_file_tracking(&mut viewed, &files);
        assert!(current.is_none());
        assert_eq!(viewed.len(), 2);
    }

    #[test]
    fn advance_tracking_appends_only_new_files() {
        let mut viewed = vec!["a.rs".to_string()];
        let files = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
        let current = advance_file_tracking(&mut viewed, &files);
        assert_eq!(current.as_deref(), Some("c.rs"));
        assert_eq!(
            viewed,
            vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()]
        );
    }

    #[test]
    fn send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<OpenCodePlugin>();
    }
}
