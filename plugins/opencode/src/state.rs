//! OpenCode state parsing.
//!
//! Parses the OpenCode window state file (`opencode.window.<uuid>.dat`)
//! to extract the active session's project directory and title.
//!
//! The window state file is a flat JSON object whose string values are
//! themselves JSON-encoded. The relevant keys are:
//!
//! * `tabs.recent` — a JSON object whose `key` ends with
//!   `/session/<id>`, identifying the most recently active session.
//!   Falls back to `tabs` when absent.
//!
//! * `tabs` — a JSON array of tab items. Each item may carry a `sessionId`
//!   from a `sidecar` server, identifying a session.
//!
//! * `tabs.info` — a JSON object keyed by session identifiers (e.g.
//!   `"sidecar\n/server/c2lkZWNhcg/session/ses_..."`) whose values carry
//!   `title` (human-readable session title) and `directory` (the workspace
//!   directory path).
//!
//! This module also parses the per-session file-view state persisted in the
//! `opencode.workspace.<project>.dat` files: the `session:<id>:file-view`
//! key holds a JSON object of the files that have been viewed in that
//! session. This is the closest persisted signal for "the file the user is
//! currently working on" — OpenCode does not store an explicit active-file
//! flag.
//!
//! This module is deliberately conservative: it returns `None`/empty for any
//! malformed input rather than panicking.

use serde_json::Value;

/// Parsed OpenCode session state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OpenCodeState {
    /// The active session ID (implementation detail, never exposed publicly).
    pub session_id: Option<String>,
    /// The workspace directory path as reported by OpenCode.
    pub directory: Option<String>,
    /// The session title as reported by OpenCode.
    pub title: Option<String>,
    /// The files viewed in the active session (from file-view state).
    pub files: Vec<String>,
}

/// Parse the raw window-state file content.
///
/// Returns `None` if the content cannot be parsed or contains no usable
/// session information. Malformed input never panics.
pub fn parse_window_state(content: &str) -> Option<OpenCodeState> {
    let root: Value = serde_json::from_str(content).ok()?;

    // The active session is `tabs.recent` when present; otherwise fall back
    // to the first session tab.
    let session_id = root
        .get("tabs.recent")
        .and_then(extract_recent_session_id)
        .or_else(|| root.get("tabs").and_then(extract_session_id));

    // Extract title and directory from the `tabs.info` field.
    let (title, directory) = extract_tab_info(root.get("tabs.info")?, &session_id);

    if title.is_none() && directory.is_none() {
        None
    } else {
        Some(OpenCodeState {
            session_id,
            directory,
            title,
            files: Vec::new(),
        })
    }
}

/// Extracts the active session ID from the `tabs.recent` value.
///
/// The value is a JSON-encoded string containing an object with a `key`
/// ending in `/session/<id>`.
fn extract_recent_session_id(tabs_recent: &Value) -> Option<String> {
    let raw = tabs_recent.as_str()?;
    let inner: Value = serde_json::from_str(raw).ok()?;
    let key = inner.get("key")?.as_str()?;
    session_id_from_key(key)
}

/// Extracts the trailing session ID from a `.../session/<id>` key.
fn session_id_from_key(key: &str) -> Option<String> {
    let id = key.rsplit("/session/").next()?;
    if id.is_empty() {
        None
    } else {
        Some(id.to_string())
    }
}

/// Extracts the active session ID from the `tabs` JSON array string.
///
/// The `tabs` value is a JSON-encoded string containing an array of tab
/// objects. The first `type: "session"` tab carries the active session ID.
fn extract_session_id(tabs: &Value) -> Option<String> {
    let tabs_str = tabs.as_str()?;
    let tabs_json: Value = serde_json::from_str(tabs_str).ok()?;
    let tabs_array = tabs_json.as_array()?;

    for tab in tabs_array {
        if let Some(id) = tab.get("sessionId").and_then(Value::as_str) {
            return Some(id.to_string());
        }
    }

    None
}

/// Extracts title and directory for the active session from `tabs.info`.
///
/// The `tabs.info` value is a JSON-encoded string containing an object
/// mapping session identifiers to `{title, directory}` objects. We match
/// the key that ends with `/session/{id}` for the active session, or fall
/// back to the first entry when the session cannot be matched.
fn extract_tab_info(info: &Value, session_id: &Option<String>) -> (Option<String>, Option<String>) {
    let Some(info_str) = info.as_str() else {
        return (None, None);
    };
    let Ok(info_json) = serde_json::from_str::<Value>(info_str) else {
        return (None, None);
    };
    let Some(info_map) = info_json.as_object() else {
        return (None, None);
    };

    // Look for the entry matching the active session ID; otherwise use the
    // first entry as a fallback.
    let target_suffix = session_id.as_ref().map(|id| format!("/session/{}", id));

    let selected = if let Some(suffix) = &target_suffix {
        info_map
            .iter()
            .find(|(key, _)| key.ends_with(suffix))
            .map(|(_, v)| v)
    } else {
        None
    }
    .or_else(|| info_map.values().next());

    let Some(selected) = selected else {
        return (None, None);
    };

    (
        selected
            .get("title")
            .and_then(Value::as_str)
            .map(String::from),
        selected
            .get("directory")
            .and_then(Value::as_str)
            .map(String::from),
    )
}

/// Parses the viewed-file list for a session from a workspace state file.
///
/// A workspace file is a flat JSON object whose values are themselves
/// JSON-encoded. The `session:<id>:file-view` key holds a JSON object
/// shaped `{"file": { "<path>": {...}, ... }}`; the object keys are the
/// paths the user has viewed in that session. The paths are returned in
/// sorted order for determinism (OpenCode does not persist view order).
///
/// Returns an empty vector when the session has no recorded file-view.
pub fn parse_file_view(content: &str, session_id: &str) -> Vec<String> {
    let root: Value = match serde_json::from_str(content) {
        Ok(root) => root,
        Err(_) => return Vec::new(),
    };

    let key = format!("session:{}:file-view", session_id);
    let Some(raw) = root.get(&key).and_then(Value::as_str) else {
        return Vec::new();
    };
    let Ok(inner) = serde_json::from_str::<Value>(raw) else {
        return Vec::new();
    };
    let Some(files) = inner.get("file").and_then(Value::as_object) else {
        return Vec::new();
    };

    let mut paths: Vec<String> = files.keys().cloned().collect();
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A realistic window-state file fixture matching the shape OpenCode
    /// writes on disk.
    fn valid_window_state() -> &'static str {
        r#"{
            "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_ff0126264ffeYEwEyQOaXyAsi5\"}]",
            "tabs.info": "{\"sidecar\\n/server/c2lkZWNhcg/session/ses_ff0126264ffeYEwEyQOaXyAsi5\":{\"title\":\"QTR codebase architecture review\",\"directory\":\"C:\\\\Users\\\\HP\\\\Desktop\\\\QTR\"}}"
        }"#
    }

    #[test]
    fn parses_window_state_with_project() {
        let state = parse_window_state(valid_window_state()).unwrap();
        assert_eq!(
            state.session_id.as_deref(),
            Some("ses_ff0126264ffeYEwEyQOaXyAsi5")
        );
        assert_eq!(
            state.directory.as_deref(),
            Some("C:\\Users\\HP\\Desktop\\QTR")
        );
        assert_eq!(
            state.title.as_deref(),
            Some("QTR codebase architecture review")
        );
    }

    #[test]
    fn malformed_content_returns_none() {
        assert!(parse_window_state("not json").is_none());
        assert!(parse_window_state("").is_none());
    }

    #[test]
    fn empty_object_returns_none() {
        assert!(parse_window_state("{}").is_none());
    }

    #[test]
    fn missing_tabs_returns_none() {
        let content = r#"{"foo": "bar"}"#;
        assert!(parse_window_state(content).is_none());
    }

    #[test]
    fn malformed_tabs_returns_none() {
        let content = r#"{"tabs": "not-a-json-array"}"#;
        assert!(parse_window_state(content).is_none());
    }

    #[test]
    fn missing_tab_info_returns_none() {
        let content = r#"{
            "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_123\"}]"
        }"#;
        assert!(parse_window_state(content).is_none());
    }

    #[test]
    fn fallback_uses_first_entry_when_session_mismatches() {
        // If the tabs.info key does not match the active session ID, the
        // parser falls back to the first entry rather than returning None.
        let content = r#"{
            "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_other\"}]",
            "tabs.info": "{\"sidecar\\n/server/c2lkZQ==/session/ses_someone\":{\"title\":\"Fallback Project\",\"directory\":\"C:\\\\Projects\\\\Fallback\"}}"
        }"#;
        let state = parse_window_state(content).unwrap();
        assert_eq!(state.title.as_deref(), Some("Fallback Project"));
        assert_eq!(state.directory.as_deref(), Some("C:\\Projects\\Fallback"));
    }

    #[test]
    fn no_title_with_directory_is_still_valid() {
        let content = r#"{
            "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_123\"}]",
            "tabs.info": "{\"sidecar:server/session/ses_123\":{\"directory\":\"C:\\\\Projects\\\\X\"}}"
        }"#;
        let state = parse_window_state(content).unwrap();
        assert!(state.title.is_none());
        assert_eq!(state.directory.as_deref(), Some("C:\\Projects\\X"));
    }

    #[test]
    fn recent_session_takes_precedence_over_tabs() {
        // tabs.recent identifies the most recently active session; the
        // parser must prefer it over the first entry in `tabs`.
        let content = r#"{
            "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_old\"}]",
            "tabs.recent": "{\"key\":\"sidecar\\n/server/c2lkZWNhcg/session/ses_active\"}",
            "tabs.info": "{\"sidecar\\n/server/c2lkZWNhcg/session/ses_old\":{\"title\":\"Old\",\"directory\":\"C:\\\\Old\"},\"sidecar\\n/server/c2lkZWNhcg/session/ses_active\":{\"title\":\"Active\",\"directory\":\"C:\\\\Active\"}}"
        }"#;
        let state = parse_window_state(content).unwrap();
        assert_eq!(state.session_id.as_deref(), Some("ses_active"));
        assert_eq!(state.title.as_deref(), Some("Active"));
        assert_eq!(state.directory.as_deref(), Some("C:\\Active"));
    }

    #[test]
    fn recent_session_falls_back_to_tabs_when_malformed() {
        let content = r#"{
            "tabs": "[{\"type\":\"session\",\"server\":\"sidecar\",\"sessionId\":\"ses_tab\"}]",
            "tabs.recent": "not-json",
            "tabs.info": "{\"sidecar/server/session/ses_tab\":{\"title\":\"From Tabs\",\"directory\":\"C:\\\\Tabs\"}}"
        }"#;
        let state = parse_window_state(content).unwrap();
        assert_eq!(state.session_id.as_deref(), Some("ses_tab"));
        assert_eq!(state.title.as_deref(), Some("From Tabs"));
    }

    #[test]
    fn parses_file_view_for_session() {
        let content = r#"{
            "session:ses_abc:file-view": "{\"file\":{\"plugins/flstudio/src/lib.rs\":{\"selectedLines\":{\"start\":16,\"end\":16}},\"presencehub.toml\":{\"selectedLines\":{\"start\":1,\"end\":11}}}}",
            "workspace:vcs": "{\"value\":{\"branch\":\"main\"}}"
        }"#;
        let files = parse_file_view(content, "ses_abc");
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"plugins/flstudio/src/lib.rs".to_string()));
        assert!(files.contains(&"presencehub.toml".to_string()));
    }

    #[test]
    fn file_view_missing_for_session_returns_empty() {
        let content = r#"{
            "session:ses_other:file-view": "{\"file\":{\"a.rs\":{}}}",
            "workspace:vcs": "{\"value\":{\"branch\":\"main\"}}"
        }"#;
        assert!(parse_file_view(content, "ses_abc").is_empty());
    }

    #[test]
    fn file_view_malformed_returns_empty() {
        assert!(parse_file_view("not json", "ses_abc").is_empty());
        let content = r#"{"session:ses_abc:file-view": "not-json"}"#;
        assert!(parse_file_view(content, "ses_abc").is_empty());
        let content = r#"{"session:ses_abc:file-view": "{\"file\":\"oops\"}"}"#;
        assert!(parse_file_view(content, "ses_abc").is_empty());
    }
}
