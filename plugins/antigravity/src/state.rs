//! Antigravity state parsing.
//!
//! Parses Antigravity's `task.md` markdown files to extract the agent's
//! active task, current section heading, and overall goal heading.
//!
//! Follows the specification established in `A-Gift-Of-Flame/antigravity-rpc`.

/// Structured representation of Antigravity's detected agent activity.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AntigravityState {
    /// The main goal header (`# Header`).
    pub main_header: Option<String>,
    /// The current section header (`## Section`).
    pub section_header: Option<String>,
    /// The active in-progress task extracted from the first `[/]` line.
    pub active_task: Option<String>,
    /// The human-readable task/section name (section header if available, else main header).
    pub task_name: Option<String>,
    /// Whether an agent is actively working on a task.
    pub is_agent_working: bool,
}

/// Parses the contents of a `task.md` file into [`AntigravityState`].
///
/// Sequential line parser:
/// 1. `# <Header>` sets the main header and resets the section header.
/// 2. `## <Header>` sets the current section header.
/// 3. Any line containing `[/]` marks an in-progress active task.
///    The task text is extracted (ignoring HTML comments `<!-- ... -->`).
///    The task name is set to the nearest `##` section header, or the `#` main header.
///    Parsing stops after finding the first active task.
pub fn parse_task_md(content: &str) -> AntigravityState {
    let mut current_main_header: Option<String> = None;
    let mut current_section_header: Option<String> = None;
    let mut active_task: Option<String> = None;
    let mut task_name: Option<String> = None;
    let mut is_agent_working = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if let Some(rest) = trimmed.strip_prefix("# ") {
            current_main_header = Some(rest.trim().to_string());
            current_section_header = None;
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            current_section_header = Some(rest.trim().to_string());
        } else if trimmed.contains("[/]") {
            if let Some(task_text) = extract_in_progress_task(trimmed) {
                active_task = Some(task_text);
                task_name = current_section_header
                    .clone()
                    .or_else(|| current_main_header.clone());
                is_agent_working = true;
                break;
            }
        }
    }

    AntigravityState {
        main_header: current_main_header,
        section_header: current_section_header,
        active_task,
        task_name,
        is_agent_working,
    }
}

/// Extracts the task description from a line containing `[/]`.
///
/// Handles markdown formats such as:
/// - `- [/] Implement conversation detection <!-- id: 0 -->` -> `"Implement conversation detection"`
/// - `* [/] Build AST parser` -> `"Build AST parser"`
/// - `[/] Refactoring extension` -> `"Refactoring extension"`
fn extract_in_progress_task(line: &str) -> Option<String> {
    let idx = line.find("[/]")?;
    let after = &line[idx + 3..];
    let without_comment = match after.find("<!--") {
        Some(comment_start) => &after[..comment_start],
        None => after,
    };
    let cleaned = without_comment.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_content_returns_default() {
        let state = parse_task_md("");
        assert_eq!(state, AntigravityState::default());
        assert!(!state.is_agent_working);
    }

    #[test]
    fn parse_with_main_header_and_active_task() {
        let content = r#"
# Implement Discord Presence

- [/] Implement conversation detection
- [ ] Add unit tests
"#;
        let state = parse_task_md(content);
        assert_eq!(
            state.main_header.as_deref(),
            Some("Implement Discord Presence")
        );
        assert_eq!(state.section_header, None);
        assert_eq!(
            state.active_task.as_deref(),
            Some("Implement conversation detection")
        );
        assert_eq!(
            state.task_name.as_deref(),
            Some("Implement Discord Presence")
        );
        assert!(state.is_agent_working);
    }

    #[test]
    fn parse_with_section_header_and_active_task() {
        let content = r#"
# Core Refactoring

## Dynamic Conversation Tracking

- [x] Create directory watcher
- [/] Parse active task from task.md <!-- id: 123 -->
- [ ] Connect to Discord

## Next Section
- [ ] Other task
"#;
        let state = parse_task_md(content);
        assert_eq!(state.main_header.as_deref(), Some("Core Refactoring"));
        assert_eq!(
            state.section_header.as_deref(),
            Some("Dynamic Conversation Tracking")
        );
        assert_eq!(
            state.active_task.as_deref(),
            Some("Parse active task from task.md")
        );
        assert_eq!(
            state.task_name.as_deref(),
            Some("Dynamic Conversation Tracking")
        );
        assert!(state.is_agent_working);
    }

    #[test]
    fn section_header_resets_on_new_main_header() {
        let content = r#"
# First Goal
## First Section
- [x] Done

# Second Goal
- [/] Active task in second goal without section
"#;
        let state = parse_task_md(content);
        assert_eq!(state.main_header.as_deref(), Some("Second Goal"));
        assert_eq!(state.section_header, None);
        assert_eq!(
            state.active_task.as_deref(),
            Some("Active task in second goal without section")
        );
        assert_eq!(state.task_name.as_deref(), Some("Second Goal"));
        assert!(state.is_agent_working);
    }

    #[test]
    fn parse_when_all_tasks_completed_or_pending() {
        let content = r#"
# Done Goal
## Sub Section
- [x] Task 1
- [x] Task 2
- [ ] Task 3
"#;
        let state = parse_task_md(content);
        assert_eq!(state.main_header.as_deref(), Some("Done Goal"));
        assert_eq!(state.section_header.as_deref(), Some("Sub Section"));
        assert_eq!(state.active_task, None);
        assert_eq!(state.task_name, None);
        assert!(!state.is_agent_working);
    }

    #[test]
    fn extract_in_progress_task_strips_html_comments() {
        assert_eq!(
            extract_in_progress_task("- [/] Write docs <!-- comment -->"),
            Some("Write docs".to_string())
        );
        assert_eq!(
            extract_in_progress_task("[/] Simple task"),
            Some("Simple task".to_string())
        );
        assert_eq!(
            extract_in_progress_task("- [/] <!-- only comment -->"),
            None
        );
    }
}
