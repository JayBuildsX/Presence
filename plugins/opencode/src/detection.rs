//! OpenCode detection: process and state-file discovery.
//!
//! OpenCode is an Electron desktop application installed as
//! `@opencode-aidesktop` with user data at
//! `%APPDATA%\ai.opencode.desktop`.
//!
//! The plugin relies on two local signals:
//!
//! 1. **Process presence** — the `OpenCode.exe` process exists while the
//!    application is running. This is the canonical active/running check.
//!
//! 2. **Window state file** — OpenCode persists its current window state
//!    (active sessions, project directories, session titles) to a
//!    `opencode.window.<uuid>.dat` file in the user-data directory.
//!
//! The window state file is the authoritative source for the current
//! project/workspace and session title. It is written by OpenCode itself
//! and is stable while the application is running.

use std::path::{Path, PathBuf};

/// The OpenCode process base name on Windows.
pub const OPENCODE_PROCESS_NAME: &str = "OpenCode.exe";

/// The OpenCode user-data directory name under `%APPDATA%`.
pub const OPENCODE_DATA_DIR: &str = "ai.opencode.desktop";

/// Environment variable override for the OpenCode data directory (mainly for tests).
pub const DATA_DIR_ENV_VAR: &str = "PRESENCEHUB_OPENCODE_DATA_DIR";

/// The window state file prefix.
pub const WINDOW_STATE_PREFIX: &str = "opencode.window.";

/// The window state file suffix.
pub const WINDOW_STATE_SUFFIX: &str = ".dat";

/// Checks whether the OpenCode process is currently running.
///
/// On Windows this enumerates running processes and matches the base name
/// `OpenCode.exe`. The check is cheap and reliable: the process exists
/// exactly while the application is open.
#[cfg(windows)]
pub fn opencode_running() -> bool {
    use winapi::shared::minwindef::FALSE;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot.is_null() {
            return false;
        }

        let mut entry: winapi::um::tlhelp32::PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<winapi::um::tlhelp32::PROCESSENTRY32W>() as u32;

        let mut found = false;
        if Process32FirstW(snapshot, &mut entry) != FALSE {
            loop {
                // szExeFile is a fixed-size array; truncate at the first null
                // terminator before converting.
                let name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0)],
                );
                if name.eq_ignore_ascii_case(OPENCODE_PROCESS_NAME) {
                    found = true;
                    break;
                }
                if Process32NextW(snapshot, &mut entry) == FALSE {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        found
    }
}

/// Non-Windows fallback: no cheap way to verify, defer to state-file presence.
#[cfg(not(windows))]
pub fn opencode_running() -> bool {
    false
}

/// Resolves the OpenCode user-data directory.
///
/// Order of preference:
/// 1. `PRESENCEHUB_OPENCODE_DATA_DIR` environment variable (test/custom installs)
/// 2. `%APPDATA%\ai.opencode.desktop` on Windows
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(env_path) = std::env::var(DATA_DIR_ENV_VAR) {
        if !env_path.is_empty() {
            return Some(PathBuf::from(env_path));
        }
    }

    #[cfg(windows)]
    {
        if let Ok(appdata) = std::env::var("APPDATA") {
            let path = PathBuf::from(appdata).join(OPENCODE_DATA_DIR);
            if path.is_dir() {
                return Some(path);
            }
        }
    }

    None
}

/// Finds the OpenCode window state file.
///
/// Returns the path of the first `opencode.window.<uuid>.dat` file found in
/// the data directory. There is normally exactly one such file (one window).
pub fn find_window_state_file() -> Option<PathBuf> {
    let dir = data_dir()?;
    let entries = std::fs::read_dir(&dir).ok()?;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(WINDOW_STATE_PREFIX) && name.ends_with(WINDOW_STATE_SUFFIX) {
            return Some(entry.path());
        }
    }

    None
}

/// Reads the raw contents of the OpenCode window state file.
pub fn read_window_state() -> Option<String> {
    let path = find_window_state_file()?;
    std::fs::read_to_string(path).ok()
}

/// The workspace state file prefix.
pub const WORKSPACE_STATE_PREFIX: &str = "opencode.workspace.";

/// Finds all OpenCode workspace state files.
///
/// OpenCode persists per-project UI state (model selection, file views,
/// terminal layout) to `opencode.workspace.<slug>.dat` files in the data
/// directory. The slug encodes the project path, so discovery scans the
/// directory rather than deriving a name from a directory path.
pub fn workspace_state_files() -> Vec<PathBuf> {
    let Some(dir) = data_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(WORKSPACE_STATE_PREFIX) && name.ends_with(WINDOW_STATE_SUFFIX) {
            files.push(entry.path());
        }
    }
    files
}

/// Reads the viewed-file list for a session from the workspace state files.
///
/// Returns the first non-empty file-view found across all workspace files
/// for the given session, or `None` when no workspace file records one.
pub fn read_file_view(session_id: &str) -> Option<Vec<String>> {
    for path in workspace_state_files() {
        let content = std::fs::read_to_string(&path).ok()?;
        let files = crate::state::parse_file_view(&content, session_id);
        if !files.is_empty() {
            return Some(files);
        }
    }
    None
}

/// Extracts the human-readable project name from a filesystem path.
///
/// Returns the last path component (the directory name). This deliberately
/// avoids leaking the full filesystem path, usernames, or home directories.
pub fn project_name_from_path(path: &str) -> Option<String> {
    let path = Path::new(path);
    let name = path.file_name()?.to_string_lossy().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Creates a temporary data directory populated with fixture window and
    /// workspace state files, sets `PRESENCEHUB_OPENCODE_DATA_DIR` to it, and
    /// returns a guard that restores the environment and removes the
    /// directory on drop. The fixture mirrors the shapes OpenCode writes.
    struct DataDirGuard {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl DataDirGuard {
        fn new() -> Self {
            let lock = TEST_LOCK.lock().unwrap();
            let dir = std::env::temp_dir()
                .join(format!("presencehub_opencode_test_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();

            std::fs::write(
                dir.join("opencode.window.test.dat"),
                r#"{
                    "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_active\"}]",
                    "tabs.recent": "{\"key\":\"sidecar\\n/server/c2lkZWNhcg/session/ses_active\"}",
                    "tabs.info": "{\"sidecar\\n/server/c2lkZWNhcg/session/ses_active\":{\"title\":\"Working\",\"directory\":\"C:\\\\Users\\\\HP\\\\Desktop\\\\MyProject\"}}"
                }"#,
            )
            .unwrap();

            std::fs::write(
                dir.join("opencode.workspace.test.dat"),
                r#"{
                    "session:ses_active:file-view": "{\"file\":{\"src/main.rs\":{\"selectedLines\":{\"start\":1,\"end\":1}},\"src/app.ts\":{\"selectedLines\":{\"start\":5,\"end\":5}}}}",
                    "workspace:vcs": "{\"value\":{\"branch\":\"main\"}}"
                }"#,
            )
            .unwrap();

            std::env::set_var(DATA_DIR_ENV_VAR, &dir);
            DataDirGuard { dir, _lock: lock }
        }
    }

    impl Drop for DataDirGuard {
        fn drop(&mut self) {
            std::env::remove_var(DATA_DIR_ENV_VAR);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn project_name_from_full_path() {
        assert_eq!(
            project_name_from_path("C:\\Users\\HP\\Desktop\\PresenceHUB"),
            Some("PresenceHUB".to_string())
        );
    }

    #[test]
    fn project_name_from_relative_path() {
        assert_eq!(
            project_name_from_path("my-project/src"),
            Some("src".to_string())
        );
    }

    #[test]
    fn project_name_from_single_component() {
        assert_eq!(
            project_name_from_path("my-project"),
            Some("my-project".to_string())
        );
    }

    #[test]
    fn project_name_from_root() {
        assert_eq!(project_name_from_path("C:\\"), None);
    }

    #[test]
    fn project_name_from_empty() {
        assert_eq!(project_name_from_path(""), None);
    }

    #[test]
    fn project_name_from_trailing_slash() {
        // On Windows, a trailing separator is normalized by Path::file_name
        // and the last component is still extracted correctly.
        assert_eq!(
            project_name_from_path("C:\\Users\\HP\\Desktop\\"),
            Some("Desktop".to_string())
        );
    }

    #[test]
    fn project_name_does_not_leak_full_path() {
        let name = project_name_from_path("C:\\Users\\secretuser\\home\\ProjectX").unwrap();
        assert_eq!(name, "ProjectX");
        assert!(!name.contains("secretuser"));
        assert!(!name.contains("C:\\"));
    }

    #[test]
    fn end_to_end_reads_window_and_file_view_state() {
        let _guard = DataDirGuard::new();

        // Window state: active session and project.
        let raw = read_window_state().expect("window state should be readable");
        let state = crate::state::parse_window_state(&raw).expect("window state should parse");
        assert_eq!(state.session_id.as_deref(), Some("ses_active"));
        assert_eq!(
            state.directory.as_deref(),
            Some("C:\\Users\\HP\\Desktop\\MyProject")
        );

        // File view: the session's viewed files.
        let files = read_file_view("ses_active").expect("file view should be readable");
        assert!(files.contains(&"src/main.rs".to_string()));
        assert!(files.contains(&"src/app.ts".to_string()));
    }

    #[test]
    fn read_file_view_returns_none_for_unknown_session() {
        let _guard = DataDirGuard::new();
        assert!(read_file_view("ses_missing").is_none());
    }

    #[test]
    fn workspace_state_files_discovers_workspace_files_only() {
        let _guard = DataDirGuard::new();
        let files = workspace_state_files();
        assert!(files.iter().any(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(WORKSPACE_STATE_PREFIX))
                .unwrap_or(false)
        }));
        // The window file must not be reported as a workspace file.
        assert!(!files.iter().any(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with(WINDOW_STATE_PREFIX))
                .unwrap_or(false)
        }));
    }
}
