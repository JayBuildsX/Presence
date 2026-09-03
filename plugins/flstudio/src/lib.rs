//! FL Studio Plugin
//!
//! Production plugin that observes FL Studio and produces canonical
//! PresenceHub Activities by reading the FL Studio window title.
//!
//! Implemented following `zfi2/FL-Studio-Discord-RPC`:
//!
//! # Window Detection
//!
//! FL Studio is found by process name (`FL64.exe` / `FL.exe`): all PIDs
//! owned by those processes are collected, then top-level windows are
//! enumerated and the first visible window belonging to one of those PIDs
//! with a non-empty title wins. No window class matching is involved.
//!
//! # Title Parsing
//!
//! The title is split on the first hyphen: the part before it is the
//! project name, the part after it is the application name
//! (`"song.flp - FL Studio 21"` → project `"song.flp"`, app
//! `"FL Studio 21"`). A title without a hyphen carries no project. A
//! trailing `*` on the project marks unsaved changes.

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
/// Following `zfi2/FL-Studio-Discord-RPC`, the title is split on the first
/// hyphen:
///
/// - `"song.flp - FL Studio 21"` — project `"song.flp"`, version `"21"`
/// - `"song.flp* - FL Studio 21"` — project `"song.flp"`, unsaved
/// - `"FL Studio 21"` — no project loaded
pub fn parse_window_title(title: &str) -> Result<ParsedTitle, FlStudioError> {
    match title.split_once('-') {
        Some((before, after)) => {
            let raw_project = before.trim();
            let (project, has_unsaved_changes) = match raw_project.strip_suffix('*') {
                Some(stripped) if !stripped.is_empty() => (Some(stripped.to_string()), true),
                _ if raw_project.is_empty() => (None, false),
                _ => (Some(raw_project.to_string()), false),
            };
            Ok(ParsedTitle {
                version: extract_version(after),
                project,
                has_unsaved_changes,
            })
        }
        None => Ok(ParsedTitle {
            version: extract_version(title),
            project: None,
            has_unsaved_changes: false,
        }),
    }
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

// ---------------------------------------------------------------------------
// Windows API (platform-specific)
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod windows {
    use std::cell::RefCell;
    use winapi::shared::minwindef::{BOOL, FALSE, LPARAM, TRUE};
    use winapi::shared::windef::HWND;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use winapi::um::winuser::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    };

    /// Process base names belonging to FL Studio.
    const FL_PROCESS_NAMES: &[&str] = &["FL64.exe", "FL.exe"];

    thread_local! {
        /// PIDs owned by the FL Studio processes for the current lookup.
        static TARGET_PIDS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
        /// First matching window title found during enumeration.
        static FOUND_TITLE: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    /// Collect the PIDs of all running FL Studio processes.
    fn collect_fl_pids() -> Vec<u32> {
        let mut pids = Vec::new();
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot.is_null() {
                return pids;
            }

            let mut entry: PROCESSENTRY32W = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

            if Process32FirstW(snapshot, &mut entry) != FALSE {
                loop {
                    let len = entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len());
                    let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
                    if FL_PROCESS_NAMES
                        .iter()
                        .any(|wanted| name.eq_ignore_ascii_case(wanted))
                    {
                        pids.push(entry.th32ProcessID);
                    }
                    if Process32NextW(snapshot, &mut entry) == FALSE {
                        break;
                    }
                }
            }

            CloseHandle(snapshot);
        }
        pids
    }

    /// Callback for EnumWindows. Takes the first visible window owned by one
    /// of the FL Studio PIDs whose title is non-empty, then stops.
    unsafe extern "system" fn enum_callback(hwnd: HWND, _lparam: LPARAM) -> BOOL {
        if FOUND_TITLE.with(|found| found.borrow().is_some()) {
            return FALSE; // already have a title, stop
        }
        if IsWindowVisible(hwnd) == 0 {
            return TRUE; // skip, continue
        }

        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        if !TARGET_PIDS.with(|targets| targets.borrow().contains(&pid)) {
            return TRUE; // not an FL Studio window, continue
        }

        let mut title_buf = [0u16; 4096];
        let title_len = GetWindowTextW(hwnd, title_buf.as_mut_ptr(), 4096);
        if title_len <= 0 {
            return TRUE; // untitled, continue
        }
        let title = String::from_utf16_lossy(&title_buf[..title_len as usize]);
        if title.is_empty() {
            return TRUE; // continue
        }

        FOUND_TITLE.with(|found| *found.borrow_mut() = Some(title));
        FALSE // stop enumeration
    }

    /// Find FL Studio's window title.
    ///
    /// Collects the PIDs of the `FL64.exe` / `FL.exe` processes, then takes
    /// the first visible window owned by one of them with a non-empty title.
    /// Returns `None` when FL Studio is not running or has no titled window.
    pub fn find_flstudio_window_title() -> Option<String> {
        unsafe {
            TARGET_PIDS.with(|targets| *targets.borrow_mut() = collect_fl_pids());
            if TARGET_PIDS.with(|targets| targets.borrow().is_empty()) {
                return None;
            }
            FOUND_TITLE.with(|found| *found.borrow_mut() = None);
            EnumWindows(Some(enum_callback), 0);
            FOUND_TITLE.with(|found| found.borrow_mut().take())
        }
    }
}

/// Get the FL Studio window title, or None if not found.
pub fn get_flstudio_title() -> Option<String> {
    windows::find_flstudio_window_title()
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

        // Prefer the real FL Studio process start time so the elapsed timer
        // counts since the application launched, not since PresenceHub
        // started tracking it. The stored session time is only a fallback
        // for when the process start cannot be read.
        let start_time = presencehub_core::process::process_start_unix(&["FL64.exe", "FL.exe"])
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
        self.last_activity = None;
        self.session_start_time = None;
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
        self.last_activity = None;
        self.session_start_time = None;
        Ok(())
    }

    fn reset(&mut self) {
        self.last_activity = None;
        self.session_start_time = None;
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

    // -- Parser tests: hyphen-split titles -------------------------------------

    #[test]
    fn parse_title_with_project() {
        let parsed = parse_window_title("song.flp - FL Studio 21").unwrap();
        assert_eq!(parsed.version, Some("21".to_string()));
        assert_eq!(parsed.project, Some("song.flp".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_with_project_unsaved() {
        let parsed = parse_window_title("song.flp* - FL Studio 21").unwrap();
        assert_eq!(parsed.version, Some("21".to_string()));
        assert_eq!(parsed.project, Some("song.flp".to_string()));
        assert!(parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_no_project() {
        let parsed = parse_window_title("FL Studio 21").unwrap();
        assert_eq!(parsed.version, Some("21".to_string()));
        assert!(parsed.project.is_none());
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_unknown_version_with_project() {
        let parsed = parse_window_title("song.flp - FL Studio").unwrap();
        assert!(parsed.version.is_none());
        assert_eq!(parsed.project, Some("song.flp".to_string()));
    }

    #[test]
    fn parse_title_splits_on_first_hyphen() {
        // Like the reference implementation, only the first hyphen separates
        // the project from the application name.
        let parsed = parse_window_title("my-song.flp - FL Studio 21").unwrap();
        assert_eq!(parsed.version, Some("21".to_string()));
        assert_eq!(parsed.project, Some("my".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_empty_project_before_hyphen() {
        let parsed = parse_window_title(" - FL Studio 21").unwrap();
        assert_eq!(parsed.version, Some("21".to_string()));
        assert!(parsed.project.is_none());
        assert!(!parsed.has_unsaved_changes);
    }

    // -- Parser tests: 2025 titles ------------------------------------------------

    #[test]
    fn parse_title_2025_no_project() {
        let parsed = parse_window_title("FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert!(parsed.project.is_none());
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_2025_with_project() {
        let parsed = parse_window_title("1409.flp - FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert_eq!(parsed.project, Some("1409.flp".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_2025_with_project_unsaved() {
        let parsed = parse_window_title("1409.flp* - FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert_eq!(parsed.project, Some("1409.flp".to_string()));
        assert!(parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_2025_different_project() {
        let parsed = parse_window_title("tras.flp - FL Studio 2025").unwrap();
        assert_eq!(parsed.version, Some("2025".to_string()));
        assert_eq!(parsed.project, Some("tras.flp".to_string()));
        assert!(!parsed.has_unsaved_changes);
    }

    #[test]
    fn parse_title_2025_unsaved_with_different_name() {
        let parsed = parse_window_title("my_song.flp* - FL Studio 2025").unwrap();
        assert_eq!(parsed.project, Some("my_song.flp".to_string()));
        assert!(parsed.has_unsaved_changes);
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
