//! OpenCode state parsing.
//!
//! Parses the OpenCode window state file (`opencode.window.<uuid>.dat`)
//! to extract the active session's project directory and title.
//!
//! The window state file is a flat JSON object whose string values are
//! themselves JSON-encoded. The relevant keys are:
//!
//! * `tabs` — a JSON array of tab items. Each item may carry a `sessionId`
//!   from a `sidecar` server, identifying the active session.
//!
//! * `tabs.info` — a JSON object keyed by session identifiers (e.g.
//!   `"sidecar\n/server/c2lkZWNhcg/session/ses_..."`) whose values carry
//!   `title` (human-readable session title) and `directory` (the workspace
//!   directory path).
//!
//! This module is deliberately conservative: it returns `None` for any
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
}

/// Parse the raw window-state file content.
///
/// Returns `None` if the content cannot be parsed or contains no usable
/// session information. Malformed input never panics.
pub fn parse_window_state(content: &str) -> Option<OpenCodeState> {
    let root: Value = serde_json::from_str(content).ok()?;

    // Extract the active session ID from the `tabs` field.
    let session_id = extract_session_id(root.get("tabs")?);

    // Extract title and directory from the `tabs.info` field.
    let (title, directory) = extract_tab_info(root.get("tabs.info")?, &session_id);

    if title.is_none() && directory.is_none() {
        None
    } else {
        Some(OpenCodeState {
            session_id,
            directory,
            title,
        })
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
}
