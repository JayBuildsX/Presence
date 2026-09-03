//! FL Studio Plugin
//!
//! Production plugin that observes FL Studio and produces canonical
//! PresenceHub Activities by parsing the FL Studio window title.
//!
//! # Window Detection
//!
//! The main FL Studio window is identified by its **window class name**
//! (`TFruityLoopsMainForm`), which is a stable, application-set identifier
//! that does not change across FL Studio versions or window state changes.
//!
//! This is more reliable than title-based matching because:
//! - "Welcome to FL Studio" (class: `TWelcomeWizard`) is a popup, not the main window
//! - "Mixer - FL Studio", "Piano Roll - FL Studio" etc. are child windows with different classes
//! - The main window's class name is always `TFruityLoopsMainForm` regardless of project state

#![cfg(windows)]

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Dependencies (workspace)
// ---------------------------------------------------------------------------

use presencehub_core::activity::{Activity, ActivityTimestamps};
use presencehub_plugin_host::{Plugin, PluginError, PluginMetadata, WindowIdentity};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The window class name of FL Studio's main window.
///
/// This is a stable identifier set by FL Studio itself (Delphi/VCL class name)
/// and does not change between versions or projects. Popups like the Welcome
/// wizard use a different class name (`TWelcomeWizard`).
pub const FLSTUDIO_MAIN_WINDOW_CLASS: &str = "TFruityLoopsMainForm";

/// Large image asset key on Discord.
pub const ASSET_LARGE_IMAGE: &str = "fl_studio_logo";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors specific to the FL Studio plugin.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FlStudioError {
    /// FL Studio window was not found.
    #[error("FL Studio window not found")]
    WindowNotFound,

    /// The window title could not be parsed.
    #[error("Failed to parse FL Studio window title: {0}")]
    ParseError(String),
}

// ---------------------------------------------------------------------------
// Pure parser (platform-independent)
// ---------------------------------------------------------------------------

/// Parsed information from an FL Studio window title.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedTitle {
    /// FL Studio version string (e.g. "20", "21", "2025").
    pub version: Option<String>,
    /// Project filename (e.g. "song.flp"), or None if no project loaded.
    pub project: Option<String>,
    /// Whether the project has unsaved changes.
    pub has_unsaved_changes: bool,
}

/// Parse an FL Studio window title into structured data.
///
/// Supports both old and new title formats:
///
/// # Old Format
/// - `"FL Studio 20"` — no project loaded
/// - `"FL Studio 20 - [song.flp]"` — project loaded
/// - `"FL Studio 20 - [song.flp*]"` — project loaded, unsaved
///
/// # New Format (FL Studio 2025+)
/// - `"FL Studio 2025"` — no project loaded
/// - `"song.flp - FL Studio 2025"` — project loaded
/// - `"song.flp* - FL Studio 2025"` — project loaded, unsaved
pub fn parse_window_title(title: &str) -> Result<ParsedTitle, FlStudioError> {
    let version = extract_version(title);
    let project = extract_project(title);
    let has_unsaved_changes = has_unsaved_changes(title);

    Ok(ParsedTitle {
        version,
        project,
        has_unsaved_changes,
    })
}

/// Extract the version number from the title.
///
/// Finds "FL Studio " in the title and extracts the version token after it.
/// Works with both `"FL Studio 2025"` and `"song.flp - FL Studio 2025"`.
fn extract_version(title: &str) -> Option<String> {
    let prefix = "FL Studio ";
    let start = title.find(prefix)?;
    let rest = &title[start + prefix.len()..];
    // Version is the first token after "FL Studio "
    let end = rest.find(' ').unwrap_or(rest.len());
    let ver = &rest[..end];
    // Accept any non-empty string that starts with a digit
    if !ver.is_empty() && ver.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        Some(ver.to_string())
    } else {
        None
    }
}

/// Extract the project filename from the title.
///
/// Handles both formats:
/// - Old: `[song.flp]` or `[song.flp*]` — extract from brackets
/// - New: `song.flp - FL Studio` or `song.flp* - FL Studio` — extract from the prefix before " - FL Studio"
fn extract_project(title: &str) -> Option<String> {
    // Old format: "[ProjectName.flp]" or "[ProjectName.flp*]"
    if let Some(start) = title.find('[') {
        if let Some(end) = title.find(']') {
            if start < end {
                let mut project = title[start + 1..end].to_string();
                if project.ends_with('*') {
                    project.pop();
                }
                if !project.is_empty() {
                    return Some(project);
                }
            }
        }
    }

    // New format: "ProjectName.flp - FL Studio" or "ProjectName.flp* - FL Studio"
    if let Some(pos) = title.find(" - FL Studio") {
        let prefix = &title[..pos];
        let mut project = prefix.to_string();
        // Strip trailing asterisk (unsaved marker)
        if project.ends_with('*') {
            project.pop();
        }
        if !project.is_empty() && project.ends_with(".flp") {
            return Some(project);
        }
    }

    None
}

/// Check if the title indicates unsaved changes.
///
/// Handles both formats:
/// - Old: `"... - [song.flp*]"` — ends with `*]`
/// - New: `"song.flp* - FL Studio 2025"` — contains `* - FL Studio`
fn has_unsaved_changes(title: &str) -> bool {
    // Old format: ends with "*]"
    if title.ends_with("*]") {
        return true;
    }
    // New format: contains "* - FL Studio"
    if title.contains("* - FL Studio") {
        return true;
    }
    false
}

/// Check if a window title belongs to the main FL Studio window (not a popup).
///
/// This is a secondary check used alongside class name matching.
/// The title must contain "FL Studio" to be considered a candidate.
pub fn is_main_window_title(title: &str) -> bool {
    title.contains("FL Studio")
}

// ---------------------------------------------------------------------------
// Windows API (platform-specific)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use std::cell::Cell;
    use winapi::shared::minwindef::{BOOL, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::winuser::{
        EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// Information about a candidate FL Studio window.
    #[derive(Debug, Clone)]
    #[allow(dead_code)]
    struct WindowInfo {
        hwnd: HWND,
        title: String,
        class_name: String,
        process_id: u32,
    }

    thread_local! {
        static CANDIDATES: Cell<Option<Vec<WindowInfo>>> = const { Cell::new(None) };
    }

    /// Callback for EnumWindows. Collects all visible windows whose class name
    /// matches the FL Studio main window class (`TFruityLoopsMainForm`).
    unsafe extern "system" fn enum_callback(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        // Only consider visible windows
        if IsWindowVisible(hwnd) == 0 {
            return TRUE; // skip, continue
        }

        // Get window title
        let mut title_buf = [0u16; 4096];
        let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 4096);
        let title = if title_len > 0 {
            String::from_utf16_lossy(&title_buf[..title_len as usize])
        } else {
            String::new()
        };

        // Get class name
        let mut class_buf = [0u16; 256];
        let class_len = GetClassNameW(hwnd, class_buf.as_mut_ptr(), 256);
        let class_name = if class_len > 0 {
            String::from_utf16_lossy(&class_buf[..class_len as usize])
        } else {
            String::new()
        };

        // Get process ID
        let mut process_id: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut process_id);

        // Only collect windows whose class name matches the FL Studio main window class
        // and whose title contains "FL Studio" (secondary verification)
        if class_name == super::FLSTUDIO_MAIN_WINDOW_CLASS && super::is_main_window_title(&title) {
            CANDIDATES.with(|cell| {
                let mut candidates = cell.take().unwrap_or_default();
                candidates.push(WindowInfo {
                    hwnd,
                    title,
                    class_name,
                    process_id,
                });
                cell.set(Some(candidates));
            });
        }

        TRUE // continue enumeration
    }

    /// Find the main FL Studio window.
    ///
    /// Uses the window class name (`TFruityLoopsMainForm`) as the primary
    /// identifier, which is stable across FL Studio versions and project states.
    /// The title must also contain "FL Studio" as a secondary verification.
    ///
    /// Returns the HWND and title of the first matching window, or None if
    /// FL Studio is not running.
    pub fn find_flstudio_window() -> Option<(HWND, String)> {
        unsafe {
            CANDIDATES.set(None);
            EnumWindows(Some(enum_callback), 0);

            CANDIDATES.with(|cell| {
                let candidates = cell.take().unwrap_or_default();

                if candidates.is_empty() {
                    return None;
                }

                // Take the first candidate (there should only be one main window)
                Some((candidates[0].hwnd, candidates[0].title.clone()))
            })
        }
    }
}

/// Get the FL Studio window title, or None if not found.
pub fn get_flstudio_title() -> Option<String> {
    windows::find_flstudio_window().map(|(_hwnd, title)| title)
}

// ---------------------------------------------------------------------------
// Console Logger (temporary output for validation)
// ---------------------------------------------------------------------------

/// Print an Activity to the console in a human-readable format.
///
/// This is a temporary validation output. It does not know anything
/// about FL Studio. It only receives canonical Activities.
pub fn log_activity(activity: &Activity) {
    println!("--------------------------------------------------");
    println!("PresenceHub Activity");
    println!();
    if let Some(app) = activity.metadata.get("application") {
        println!("Application: {}", app);
    }
    println!("State: {}", activity.state);
    if let Some(ref details) = activity.details {
        println!("Details: {}", details);
    }
    println!();
    println!("Metadata");
    for (key, value) in &activity.metadata {
        println!("{} = {}", key, value);
    }
    println!("--------------------------------------------------");
}

// ---------------------------------------------------------------------------
// FL Studio Plugin
// ---------------------------------------------------------------------------

/// The FL Studio plugin.
///
/// Observes FL Studio by parsing its window title and producing
/// canonical PresenceHub Activities.
///
/// # Example
///
/// ```rust
/// use presencehub_flstudio::FlStudioPlugin;
/// use presencehub_plugin_host::Plugin;
///
/// let mut plugin = FlStudioPlugin::new();
/// let meta = plugin.metadata();
/// assert_eq!(meta.name, "FL Studio");
/// ```
pub struct FlStudioPlugin {
    /// Plugin metadata.
    metadata: PluginMetadata,
    /// The last emitted activity, used for change detection.
    last_activity: Option<Activity>,
    /// The start timestamp of the active FL Studio session.
    session_start_time: Option<i64>,
}

impl FlStudioPlugin {
    /// Creates a new FL Studio plugin.
    pub fn new() -> Self {
        Self {
            metadata: PluginMetadata::new("FL Studio", "0.1.0"),
            last_activity: None,
            session_start_time: None,
        }
    }

    /// Handle the application window being absent.
    ///
    /// Forgets the previously emitted activity and resets the session start
    /// timestamp so the next identical activity is published again when the
    /// application reopens. Returns the poll error the runtime uses to end
    /// the session.
    fn application_not_found(&mut self) -> Result<Option<Activity>, PluginError> {
        self.last_activity = None;
        self.session_start_time = None;
        Err(PluginError::PollFailed(
            "FL Studio window not found".to_string(),
        ))
    }

    /// Observe a window title and apply change detection.
    ///
    /// Builds a canonical Activity from the title and emits it only when
    /// it differs from the last emitted activity. Split out of
    /// [`Plugin::poll`] so the deduplication state machine can be tested
    /// without a live FL Studio window.
    pub fn observe_title(&mut self, title: &str) -> Result<Option<Activity>, PluginError> {
        let parsed =
            parse_window_title(title).map_err(|e| PluginError::PollFailed(e.to_string()))?;

        // Initialize persistent session start time if this is a newly active session
        let start_time = match self.session_start_time {
            Some(ts) => ts,
            None => {
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                self.session_start_time = Some(now);
                now
            }
        };

        let activity = build_activity(&parsed, start_time);

        // Only emit if the activity changed.
        if self.last_activity.as_ref() != Some(&activity) {
            self.last_activity = Some(activity.clone());
            return Ok(Some(activity));
        }

        Ok(None)
    }
}

impl Default for FlStudioPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for FlStudioPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn window_identity(&self) -> Option<WindowIdentity> {
        // The main window's class name is the stable identifier; the
        // process base names are a secondary fallback.
        Some(WindowIdentity::new(
            ["FL64.exe".to_string(), "FL.exe".to_string()],
            [FLSTUDIO_MAIN_WINDOW_CLASS.to_string()],
        ))
    }

    fn init(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
        // Poll FL Studio and produce a canonical Activity.
        // Errors are mapped to PluginError::PollFailed.
        let Some(title) = get_flstudio_title() else {
            // FL Studio is not running. Forget the previous activity so the
            // next identical activity is published again after the
            // application reopens.
            return self.application_not_found();
        };

        self.observe_title(&title)
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        Ok(())
    }
}

/// Build a canonical Activity from parsed FL Studio title data.
///
/// Follows `zfi2/FL-Studio-Discord-RPC`:
/// - `details`: "FL Studio <version>" (or "FL Studio" if version is unknown)
/// - `state`: project name (e.g. "song.flp" or "song.flp*"), or "Empty project" when no project loaded
/// - `timestamps`: persistent start timestamp when session started
/// - `metadata`: "large_image" = "fl_studio_logo", "large_text" = details
pub fn build_activity(parsed: &ParsedTitle, start_timestamp: i64) -> Activity {
    let details = match &parsed.version {
        Some(version) => format!("FL Studio {}", version),
        None => "FL Studio".to_string(),
    };

    let state = match &parsed.project {
        Some(project) => {
            if parsed.has_unsaved_changes {
                format!("{}*", project)
            } else {
                project.clone()
            }
        }
        None => "Empty project".to_string(),
    };

    let mut metadata = HashMap::new();
    metadata.insert("large_image".to_string(), ASSET_LARGE_IMAGE.to_string());
    metadata.insert("large_text".to_string(), details.clone());
    metadata.insert("application".to_string(), "FL Studio".to_string());
    if let Some(ref v) = parsed.version {
        metadata.insert("version".to_string(), v.clone());
    }
    if let Some(ref p) = parsed.project {
        metadata.insert("project".to_string(), p.clone());
    }
    metadata.insert(
        "unsaved".to_string(),
        if parsed.has_unsaved_changes {
            "true"
        } else {
            "false"
        }
        .to_string(),
    );

    Activity {
        state,
        details: Some(details),
        timestamps: Some(ActivityTimestamps {
            start: Some(start_timestamp),
            end: None,
        }),
        application: Some("FL Studio".to_string()),
        metadata,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Parser tests: Old Format ------------------------------------------------

    #[test]
    fn parse_title_old_format_with_project() {
        let parsed = parse_window_title("FL Studio 20 - [song.flp]").unwrap();
        assert_eq!(parsed.version, Some("20".to_string()));
        assert_eq!(parsed.project, Some("song.flp".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_old_format_with_project_unsaved() {
        let parsed = parse_window_title("FL Studio 20 - [song.flp*]").unwrap();
        assert_eq!(parsed.version, Some("20".to_string()));
        assert_eq!(parsed.project, Some("song.flp".to_string()));
        assert!(parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_old_format_no_project() {
        let parsed = parse_window_title("FL Studio 20").unwrap();
        assert_eq!(parsed.version, Some("20".to_string()));
        assert!(parsed.project.is_none());
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_old_format_version_21() {
        let parsed = parse_window_title("FL Studio 21 - [track.flp]").unwrap();
        assert_eq!(parsed.version, Some("21".to_string()));
        assert_eq!(parsed.project, Some("track.flp".to_string()));
    }

    #[test]
    fn parse_title_old_format_unknown_version() {
        let parsed = parse_window_title("FL Studio - [song.flp]").unwrap();
        assert!(parsed.version.is_none());
        assert_eq!(parsed.project, Some("song.flp".to_string()));
    }

    // -- Parser tests: New Format (FL Studio 2025+) ------------------------------

    #[test]
    fn parse_title_new_format_no_project() {
        let parsed = parse_window_title("FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert!(parsed.project.is_none());
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_new_format_with_project() {
        let parsed = parse_window_title("1409.flp - FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert_eq!(parsed.project, Some("1409.flp".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_new_format_with_project_unsaved() {
        let parsed = parse_window_title("1409.flp* - FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert_eq!(parsed.project, Some("1409.flp".to_string()));
        assert!(parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_new_format_different_project() {
        let parsed = parse_window_title("tras.flp - FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert_eq!(parsed.project, Some("tras.flp".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_new_format_unsaved_with_different_name() {
        let parsed = parse_window_title("my_song.flp* - FL Studio 2025").unwrap();
        assert_eq!(parsed.project, Some("my_song.flp".to_string()));
        assert!(parsed.has_unsaved_changes);
    }

    // -- extract_project tests -------------------------------------------------

    #[test]
    fn extract_project_old_format() {
        assert_eq!(
            extract_project("FL Studio 20 - [song.flp]"),
            Some("song.flp".to_string())
        );
    }

    #[test]
    fn extract_project_old_format_unsaved() {
        assert_eq!(
            extract_project("FL Studio 20 - [song.flp*]"),
            Some("song.flp".to_string())
        );
    }

    #[test]
    fn extract_project_new_format() {
        assert_eq!(
            extract_project("1409.flp - FL Studio 2025"),
            Some("1409.flp".to_string())
        );
    }

    #[test]
    fn extract_project_new_format_unsaved() {
        assert_eq!(
            extract_project("1409.flp* - FL Studio 2025"),
            Some("1409.flp".to_string())
        );
    }

    #[test]
    fn extract_project_no_project_old_format() {
        assert_eq!(extract_project("FL Studio 20"), None);
    }

    #[test]
    fn extract_project_no_project_new_format() {
        assert_eq!(extract_project("FL Studio 2025"), None);
    }

    // -- has_unsaved_changes tests --------------------------------------------

    #[test]
    fn unsaved_old_format() {
        assert!(has_unsaved_changes("FL Studio 20 - [song.flp*]"));
        assert!(!has_unsaved_changes("FL Studio 20 - [song.flp]"));
    }

    #[test]
    fn unsaved_new_format() {
        assert!(has_unsaved_changes("song.flp* - FL Studio 2025"));
        assert!(!has_unsaved_changes("song.flp - FL Studio 2025"));
    }

    // -- is_main_window_title tests --------------------------------------------

    #[test]
    fn is_main_window_title_old_format() {
        assert!(is_main_window_title("FL Studio 20"));
        assert!(is_main_window_title("FL Studio 20 - [song.flp]"));
    }

    #[test]
    fn is_main_window_title_new_format() {
        assert!(is_main_window_title("FL Studio 2025"));
        assert!(is_main_window_title("1409.flp - FL Studio 2025"));
        assert!(is_main_window_title("song.flp* - FL Studio 2025"));
    }

    #[test]
    fn is_main_window_title_accepts_fl_studio_in_title() {
        // is_main_window_title is a secondary check — it verifies the title
        // contains "FL Studio". Primary filtering by class name
        // (TFruityLoopsMainForm vs TWelcomeWizard) happens in enum_callback.
        assert!(is_main_window_title("FL Studio 2025"));
        assert!(is_main_window_title("1409.flp - FL Studio 2025"));
        // "Welcome to FL Studio" also contains "FL Studio" but has a different
        // class name (TWelcomeWizard), so it's excluded by the class name filter.
        assert!(is_main_window_title("Welcome to FL Studio"));
    }

    // -- Activity mapping tests ------------------------------------------------

    #[test]
    fn build_activity_with_project() {
        let parsed = ParsedTitle {
            version: Some("20".to_string()),
            project: Some("my_song.flp".to_string()),
            has_unsaved_changes: false,
        };
        let activity = build_activity(&parsed, 1700000000);
        assert_eq!(activity.state, "my_song.flp");
        assert_eq!(activity.details, Some("FL Studio 20".to_string()));
        assert_eq!(activity.application.as_deref(), Some("FL Studio"));
        assert_eq!(
            activity.metadata.get("large_image"),
            Some(&"fl_studio_logo".to_string())
        );
        assert_eq!(
            activity.metadata.get("large_text"),
            Some(&"FL Studio 20".to_string())
        );
        assert_eq!(activity.metadata.get("version"), Some(&"20".to_string()));
        assert_eq!(activity.metadata.get("unsaved"), Some(&"false".to_string()));
        assert_eq!(activity.timestamps.unwrap().start, Some(1700000000));
    }

    #[test]
    fn build_activity_with_unsaved_project() {
        let parsed = ParsedTitle {
            version: Some("21".to_string()),
            project: Some("my_song.flp".to_string()),
            has_unsaved_changes: true,
        };
        let activity = build_activity(&parsed, 1700000000);
        assert_eq!(activity.state, "my_song.flp*");
        assert_eq!(activity.details, Some("FL Studio 21".to_string()));
        assert_eq!(activity.metadata.get("version"), Some(&"21".to_string()));
        assert_eq!(activity.metadata.get("unsaved"), Some(&"true".to_string()));
    }

    #[test]
    fn build_activity_without_project() {
        let parsed = ParsedTitle {
            version: Some("20".to_string()),
            project: None,
            has_unsaved_changes: false,
        };
        let activity = build_activity(&parsed, 1700000000);
        assert_eq!(activity.state, "Empty project");
        assert_eq!(activity.details, Some("FL Studio 20".to_string()));
        assert_eq!(activity.application.as_deref(), Some("FL Studio"));
    }

    #[test]
    fn build_activity_with_2025_version() {
        let parsed = ParsedTitle {
            version: Some("2025".to_string()),
            project: Some("track.flp".to_string()),
            has_unsaved_changes: false,
        };
        let activity = build_activity(&parsed, 1700000000);
        assert_eq!(activity.state, "track.flp");
        assert_eq!(activity.details, Some("FL Studio 2025".to_string()));
        assert_eq!(activity.metadata.get("version"), Some(&"2025".to_string()));
    }

    #[test]
    fn timestamp_persists_across_title_updates() {
        let mut plugin = FlStudioPlugin::new();
        let act1 = plugin
            .observe_title("song.flp - FL Studio 2025")
            .unwrap()
            .unwrap();
        let ts1 = act1.timestamps.unwrap().start;
        assert!(ts1.is_some());

        let act2 = plugin
            .observe_title("other.flp - FL Studio 2025")
            .unwrap()
            .unwrap();
        let ts2 = act2.timestamps.unwrap().start;
        assert_eq!(
            ts1, ts2,
            "session start timestamp must not reset on title update"
        );
    }

    // -- Plugin tests ----------------------------------------------------------

    #[test]
    fn plugin_metadata() {
        let plugin = FlStudioPlugin::new();
        let meta = plugin.metadata();
        assert_eq!(meta.name, "FL Studio");
        assert_eq!(meta.version, "0.1.0");
    }

    #[test]
    fn plugin_init_and_shutdown() {
        let mut plugin = FlStudioPlugin::new();
        assert!(plugin.init().is_ok());
        assert!(plugin.shutdown().is_ok());
    }

    #[test]
    fn plugin_poll_returns_err_when_not_running() {
        let mut plugin = FlStudioPlugin::new();
        let result = Plugin::poll(&mut plugin);

        // This test is environment-dependent: if FL Studio is currently
        // running, poll() returns Ok(Some(...)) or Ok(None). If it is not
        // running, poll() returns Err(PollFailed). We only assert the
        // error case when FL Studio is not running.
        if get_flstudio_title().is_none() {
            assert!(
                result.is_err(),
                "poll should fail when FL Studio is not running"
            );
        }
        // If FL Studio IS running, the test passes trivially (we cannot
        // force it to close from here).
    }

    // Note: Runtime integration tests are in apps/desktop/src-tauri/src/main.rs

    // -- Plugin deduplication lifecycle --------------------------------------

    #[test]
    fn plugin_suppresses_identical_activity_while_running() {
        // Scenario 1: while FL Studio is open, an identical activity is
        // published once and then suppressed on subsequent polls.
        let mut plugin = FlStudioPlugin::new();

        // Open Project A → published.
        let first = plugin.observe_title("song.flp - FL Studio 2025").unwrap();
        assert!(
            first.is_some(),
            "first observation should publish an activity"
        );

        // Poll again → suppressed by duplicate detection.
        let second = plugin.observe_title("song.flp - FL Studio 2025").unwrap();
        assert!(second.is_none(), "identical activity should be suppressed");
    }

    #[test]
    fn plugin_republishes_activity_after_application_exit() {
        // Scenario 2: close FL Studio, then reopen the SAME project. The
        // plugin must forget the previous activity when the application
        // exits so the identical activity is published again.
        let mut plugin = FlStudioPlugin::new();

        // Open Project A → published.
        assert!(plugin
            .observe_title("song.flp - FL Studio 2025")
            .unwrap()
            .is_some());

        // Poll again → suppressed.
        assert!(plugin
            .observe_title("song.flp - FL Studio 2025")
            .unwrap()
            .is_none());

        // Close FL Studio → window not found → poll() forgets the previous
        // activity before the runtime ends the session.
        assert!(plugin.application_not_found().is_err());

        // Reopen Project A → must publish again, not be suppressed.
        let republished = plugin.observe_title("song.flp - FL Studio 2025").unwrap();
        assert!(
            republished.is_some(),
            "activity must republish after application exit"
        );
    }
}
