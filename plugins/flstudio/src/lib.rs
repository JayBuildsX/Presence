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
//! enumerated. The main title is the first visible window naming FL
//! Studio (falling back to the first titled window), matching
//! `zfi2/FL-Studio-Discord-RPC`.
//!
//! # Render Detection
//!
//! A visible FL Studio window that mentions a render without naming
//! FL Studio itself (e.g. the export progress dialog) marks the session
//! as rendering, in which case the presence state becomes
//! `"Rendering <project>"` instead of the project name. A project file
//! merely named like a render (e.g. `rendering.flp`) never triggers this
//! through its main title, since that title always names FL Studio.
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
        /// Every matching window title found during enumeration, in order.
        static FOUND_TITLES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
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

    /// Callback for EnumWindows. Collects every visible window owned by one
    /// of the FL Studio PIDs whose title is non-empty.
    unsafe extern "system" fn enum_callback(hwnd: HWND, _lparam: LPARAM) -> BOOL {
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

        FOUND_TITLES.with(|found| found.borrow_mut().push(title));
        TRUE // continue enumeration
    }

    /// All visible titled windows owned by the FL Studio processes, in
    /// enumeration order. Empty when FL Studio is not running.
    fn fl_window_titles() -> Vec<String> {
        unsafe {
            TARGET_PIDS.with(|targets| *targets.borrow_mut() = collect_fl_pids());
            if TARGET_PIDS.with(|targets| targets.borrow().is_empty()) {
                return Vec::new();
            }
            FOUND_TITLES.with(|found| found.borrow_mut().clear());
            EnumWindows(Some(enum_callback), 0);
            FOUND_TITLES.with(|found| found.borrow_mut().drain(..).collect())
        }
    }

    /// Find FL Studio's main window title: the first titled window whose
    /// title names FL Studio, falling back to the first titled window.
    /// Returns `None` when FL Studio is not running or has no titled window.
    pub fn find_flstudio_window_title() -> Option<String> {
        let mut titles = fl_window_titles();
        if titles.is_empty() {
            return None;
        }
        if let Some(pos) = titles.iter().position(|t| t.contains("FL Studio")) {
            return Some(titles.swap_remove(pos));
        }
        titles.into_iter().next()
    }

    /// Whether an FL Studio render/export dialog is currently visible.
    ///
    /// Matches any titled FL window satisfying [`is_render_dialog_title`].
    pub fn is_render_dialog_open() -> bool {
        fl_window_titles()
            .iter()
            .any(|title| super::is_render_dialog_title(title))
    }
}

/// Get the FL Studio window title, or None if not found.
pub fn get_flstudio_title() -> Option<String> {
    windows::find_flstudio_window_title()
}

/// Whether a window title looks like an FL Studio render/export dialog.
///
/// Matches titles mentioning a render without naming FL Studio itself
/// (e.g. the export progress dialog), so a project merely named like a
/// render (e.g. `rendering.flp - FL Studio 21`) never false-positives
/// through its main title.
pub fn is_render_dialog_title(title: &str) -> bool {
    title.to_lowercase().contains("render") && !title.contains("FL Studio")
}

/// The observed FL Studio window state for one poll.
pub struct FlWindow {
    /// Main window title (names FL Studio when available).
    pub title: String,
    /// Whether a render/export dialog is visible.
    pub rendering: bool,
}

/// Get FL Studio's current window state, or None if not found.
pub fn get_flstudio_state() -> Option<FlWindow> {
    let title = get_flstudio_title()?;
    Some(FlWindow {
        title,
        rendering: windows::is_render_dialog_open(),
    })
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
    /// it differs from the last emitted activity. `rendering` reports
    /// whether an FL Studio render/export dialog is visible. Split out of
    /// [`Plugin::poll`] so the deduplication state machine can be tested
    /// without a live FL Studio window.
    pub fn observe_title(
        &mut self,
        title: &str,
        rendering: bool,
    ) -> Result<Option<Activity>, PluginError> {
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

        let activity = build_activity(&parsed, start_time, rendering);

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
        let Some(window) = get_flstudio_state() else {
            // FL Studio is not running. Forget the previous activity so the
            // next identical activity is published again after the
            // application reopens.
            return self.application_not_found();
        };

        self.observe_title(&window.title, window.rendering)
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
/// - `state`: project name (e.g. "song.flp" or "song.flp*"), or "Empty project" when no project loaded.
///   While a render/export dialog is visible (`rendering`), the state becomes
///   "Rendering <project>" (or "Rendering..." with no project).
/// - `timestamps`: persistent start timestamp when session started
/// - `metadata`: "large_image" = "fl_studio_logo", "large_text" = details
pub fn build_activity(parsed: &ParsedTitle, start_timestamp: i64, rendering: bool) -> Activity {
    let details = match &parsed.version {
        Some(version) => format!("FL Studio {}", version),
        None => "FL Studio".to_string(),
    };

    let state = match &parsed.project {
        Some(project) => {
            let mut name = project.clone();
            if parsed.has_unsaved_changes {
                name.push('*');
            }
            if rendering {
                format!("Rendering {name}")
            } else {
                name
            }
        }
        None => {
            if rendering {
                "Rendering...".to_string()
            } else {
                "Empty project".to_string()
            }
        }
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
        let activity = build_activity(&parsed, 1700000000, false);
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
        let activity = build_activity(&parsed, 1700000000, false);
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
        let activity = build_activity(&parsed, 1700000000, false);
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
        let activity = build_activity(&parsed, 1700000000, false);
        assert_eq!(activity.state, "track.flp");
        assert_eq!(activity.details, Some("FL Studio 2025".to_string()));
        assert_eq!(activity.metadata.get("version"), Some(&"2025".to_string()));
    }

    #[test]
    fn build_activity_rendering_with_project() {
        let parsed = ParsedTitle {
            version: Some("21".to_string()),
            project: Some("song.flp".to_string()),
            has_unsaved_changes: false,
        };
        let activity = build_activity(&parsed, 1700000000, true);
        assert_eq!(activity.state, "Rendering song.flp");
        assert_eq!(activity.details, Some("FL Studio 21".to_string()));
        assert_eq!(
            activity.metadata.get("large_image"),
            Some(&"fl_studio_logo".to_string())
        );
    }

    #[test]
    fn build_activity_rendering_without_project() {
        let parsed = ParsedTitle {
            version: Some("21".to_string()),
            project: None,
            has_unsaved_changes: false,
        };
        let activity = build_activity(&parsed, 1700000000, true);
        assert_eq!(activity.state, "Rendering...");
        assert_eq!(activity.details, Some("FL Studio 21".to_string()));
    }

    #[test]
    fn render_dialog_title_matches_export_dialogs_only() {
        assert!(is_render_dialog_title("Rendering..."));
        assert!(is_render_dialog_title("RENDERING audio"));
        // Main titles always name FL Studio, even for render-named projects.
        assert!(!is_render_dialog_title("rendering.flp - FL Studio 21"));
        assert!(!is_render_dialog_title("song.flp - FL Studio 21"));
        assert!(!is_render_dialog_title("FL Studio 21"));
    }

    #[test]
    fn rendering_transition_publishes_new_activity() {
        // Starting a render must replace the editing activity, and finishing
        // it must publish the editing activity again.
        let mut plugin = FlStudioPlugin::new();

        let editing = plugin
            .observe_title("song.flp - FL Studio 21", false)
            .unwrap()
            .expect("editing should publish");
        assert_eq!(editing.state, "song.flp");

        let rendering = plugin
            .observe_title("song.flp - FL Studio 21", true)
            .unwrap()
            .expect("render start must publish");
        assert_eq!(rendering.state, "Rendering song.flp");

        // Unchanged render state is deduped.
        assert!(plugin
            .observe_title("song.flp - FL Studio 21", true)
            .unwrap()
            .is_none());

        let back = plugin
            .observe_title("song.flp - FL Studio 21", false)
            .unwrap()
            .expect("render end must publish");
        assert_eq!(back.state, "song.flp");
    }

    #[test]
    fn timestamp_persists_across_title_updates() {
        let mut plugin = FlStudioPlugin::new();
        let act1 = plugin
            .observe_title("song.flp - FL Studio 2025", false)
            .unwrap()
            .unwrap();
        let ts1 = act1.timestamps.unwrap().start;
        assert!(ts1.is_some());

        let act2 = plugin
            .observe_title("other.flp - FL Studio 2025", false)
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
        let first = plugin
            .observe_title("song.flp - FL Studio 2025", false)
            .unwrap();
        assert!(
            first.is_some(),
            "first observation should publish an activity"
        );

        // Poll again → suppressed by duplicate detection.
        let second = plugin
            .observe_title("song.flp - FL Studio 2025", false)
            .unwrap();
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
            .observe_title("song.flp - FL Studio 2025", false)
            .unwrap()
            .is_some());

        // Poll again → suppressed.
        assert!(plugin
            .observe_title("song.flp - FL Studio 2025", false)
            .unwrap()
            .is_none());

        // Close FL Studio → window not found → poll() forgets the previous
        // activity before the runtime ends the session.
        assert!(plugin.application_not_found().is_err());

        // Reopen Project A → must publish again, not be suppressed.
        let republished = plugin
            .observe_title("song.flp - FL Studio 2025", false)
            .unwrap();
        assert!(
            republished.is_some(),
            "activity must republish after application exit"
        );
    }
}
