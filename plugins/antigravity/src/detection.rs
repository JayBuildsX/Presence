//! Antigravity discovery and process detection.
//!
//! Locates the Antigravity `brain/` directory, discovers conversation folders,
//! and determines the most recently modified plan file within the stale threshold.
//! Also checks whether the Antigravity application process is running.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Default stale threshold: 5 minutes (same as reference implementation).
pub const DEFAULT_STALE_THRESHOLD: Duration = Duration::from_secs(5 * 60);

/// Plan file names the agent writes, newest format first.
///
/// Current Antigravity writes `implementation_plan.md`; older versions wrote
/// `task.md`. The freshest file of either kind marks the active conversation.
pub const PLAN_FILENAMES: &[&str] = &["implementation_plan.md", "task.md"];

/// Resolves the default Antigravity brain directory on Windows/cross-platform.
///
/// Looks in `%USERPROFILE%\.gemini\antigravity\brain` or `$HOME/.gemini/antigravity/brain`.
pub fn default_brain_dir() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    let brain = home.join(".gemini").join("antigravity").join("brain");
    if brain.is_dir() {
        Some(brain)
    } else {
        None
    }
}

/// Checks if a directory name looks like a conversation UUID (`8-4-4-4-12` hex digits).
pub fn is_conversation_uuid(name: &str) -> bool {
    if name.len() != 36 {
        return false;
    }
    let bytes = name.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 => {
                if b != b'-' {
                    return false;
                }
            }
            _ => {
                if !b.is_ascii_hexdigit() {
                    return false;
                }
            }
        }
    }
    true
}

/// Information about the most recent active conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveConversation {
    /// The conversation UUID directory name.
    pub conversation_id: String,
    /// Newest plan file for title parsing (`implementation_plan.md` or
    /// legacy `task.md`), if the conversation has one. Activity alone
    /// (transcripts, logs) still marks the conversation active.
    pub task_file: Option<PathBuf>,
    /// Newest activity anywhere inside the conversation directory.
    pub modified: SystemTime,
}

/// Finds the most recently active conversation in the brain directory.
///
/// Each conversation folder is scanned recursively: transcripts, logs,
/// messages, task files, and plan files all count as agent activity. The
/// conversation with the freshest file wins, provided it is within
/// `stale_threshold`. A separate plan-file lookup feeds title parsing.
pub fn find_active_conversation(
    brain_dir: &Path,
    stale_threshold: Duration,
) -> Option<ActiveConversation> {
    let entries = std::fs::read_dir(brain_dir).ok()?;
    let mut newest: Option<ActiveConversation> = None;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(dir_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        if !is_conversation_uuid(dir_name) {
            continue;
        }

        // Newest plan file at the conversation root, for title parsing.
        let task_file = PLAN_FILENAMES
            .iter()
            .filter_map(|name| {
                let candidate = path.join(name);
                std::fs::metadata(&candidate).ok().map(|_| candidate)
            })
            .next();
        // Newest activity anywhere inside the conversation: plan files,
        // transcripts, logs, messages, uploads — any write means the
        // agent (or the user with it) recently did something.
        let Some(mtime) = newest_mtime_recursive(&path) else {
            continue;
        };
        if let Some(ref current) = newest {
            if mtime > current.modified {
                newest = Some(ActiveConversation {
                    conversation_id: dir_name.to_string(),
                    task_file,
                    modified: mtime,
                });
            }
        } else {
            newest = Some(ActiveConversation {
                conversation_id: dir_name.to_string(),
                task_file,
                modified: mtime,
            });
        }
    }

    let active = newest?;

    // Check stale threshold against current system time
    if let Ok(elapsed) = SystemTime::now().duration_since(active.modified) {
        if elapsed > stale_threshold {
            return None;
        }
    }

    Some(active)
}

/// Newest file modification time anywhere under `dir` (recursive).
///
/// Returns `None` when the tree holds no readable files.
fn newest_mtime_recursive(dir: &Path) -> Option<SystemTime> {
    let mut newest: Option<SystemTime> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if let Ok(metadata) = std::fs::metadata(&path) {
                if let Ok(mtime) = metadata.modified() {
                    newest = Some(newest.map_or(mtime, |best| best.max(mtime)));
                }
            }
        }
    }
    newest
}

/// Checks whether the Antigravity desktop application process is running.
#[cfg(windows)]
pub fn antigravity_running() -> bool {
    use winapi::um::handleapi::{CloseHandle, INVALID_HANDLE_VALUE};
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let exe_len = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe_name = String::from_utf16_lossy(&entry.szExeFile[..exe_len]);
                if exe_name.eq_ignore_ascii_case("antigravity.exe") {
                    CloseHandle(snapshot);
                    return true;
                }

                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
        false
    }
}

#[cfg(not(windows))]
pub fn antigravity_running() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_conversation_uuids() {
        assert!(is_conversation_uuid("7604be42-4bbe-4026-9212-7e0a6709cc3f"));
        assert!(is_conversation_uuid("12345678-1234-1234-1234-123456789abc"));
        assert!(!is_conversation_uuid("not-a-uuid"));
        assert!(!is_conversation_uuid("7604be42-4bbe-4026-9212-7e0a6709cc3")); // short
        assert!(!is_conversation_uuid(
            "7604be42_4bbe_4026_9212_7e0a6709cc3f"
        )); // underscores
    }

    #[test]
    fn finds_active_conversation_in_temp_dir() {
        let temp = std::env::temp_dir().join("presencehub_test_brain");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let uuid1 = "11111111-1111-1111-1111-111111111111";
        let uuid2 = "22222222-2222-2222-2222-222222222222";
        let dir1 = temp.join(uuid1);
        let dir2 = temp.join(uuid2);

        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();

        let task1 = dir1.join("task.md");
        let task2 = dir2.join("task.md");

        std::fs::write(&task1, "# Task 1\n- [x] Done").unwrap();
        // Give a tiny delay so task2 has a later mtime
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(&task2, "# Task 2\n- [/] Working").unwrap();

        let active = find_active_conversation(&temp, Duration::from_secs(60)).unwrap();
        assert_eq!(active.conversation_id, uuid2);
        assert_eq!(active.task_file, Some(task2));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn ignores_stale_conversation() {
        let temp = std::env::temp_dir().join("presencehub_test_stale_brain");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let uuid1 = "33333333-3333-3333-3333-333333333333";
        let dir1 = temp.join(uuid1);
        std::fs::create_dir_all(&dir1).unwrap();
        let task1 = dir1.join("task.md");
        std::fs::write(&task1, "# Task 1\n- [/] Working").unwrap();

        // Stale threshold 0 secs -> immediately considered stale
        let active = find_active_conversation(&temp, Duration::from_secs(0));
        assert!(active.is_none());

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn finds_implementation_plan_as_active_conversation() {
        let temp = std::env::temp_dir().join("presencehub_test_plan_brain");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let uuid = "44444444-4444-4444-4444-444444444444";
        let dir = temp.join(uuid);
        std::fs::create_dir_all(&dir).unwrap();
        let plan = dir.join("implementation_plan.md");
        std::fs::write(&plan, "# Some Plan\n\n## Proposed Changes\n").unwrap();

        let active = find_active_conversation(&temp, Duration::from_secs(60)).unwrap();
        assert_eq!(active.conversation_id, uuid);
        assert_eq!(active.task_file, Some(plan));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn newest_plan_file_wins_across_conversations_and_formats() {
        let temp = std::env::temp_dir().join("presencehub_test_mixed_brain");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let uuid_old = "55555555-5555-5555-5555-555555555555";
        let dir_old = temp.join(uuid_old);
        std::fs::create_dir_all(&dir_old).unwrap();
        std::fs::write(dir_old.join("task.md"), "# Old task\n").unwrap();

        // Tiny delay so the plan file is strictly newer.
        std::thread::sleep(Duration::from_millis(50));

        let uuid_new = "66666666-6666-6666-6666-666666666666";
        let dir_new = temp.join(uuid_new);
        std::fs::create_dir_all(&dir_new).unwrap();
        let plan = dir_new.join("implementation_plan.md");
        std::fs::write(&plan, "# New plan\n").unwrap();

        let active = find_active_conversation(&temp, Duration::from_secs(60)).unwrap();
        assert_eq!(active.conversation_id, uuid_new);
        assert_eq!(active.task_file, Some(plan));

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn transcript_activity_without_plan_file_is_still_active() {
        // Conversations whose agent only wrote transcripts/logs (no plan
        // file yet) still mark the agent as working.
        let temp = std::env::temp_dir().join("presencehub_test_transcript_brain");
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let uuid = "77777777-7777-7777-7777-777777777777";
        let logs = temp.join(uuid).join(".system_generated").join("logs");
        std::fs::create_dir_all(&logs).unwrap();
        std::fs::write(logs.join("transcript.jsonl"), "{}\n").unwrap();

        let active = find_active_conversation(&temp, Duration::from_secs(60)).unwrap();
        assert_eq!(active.conversation_id, uuid);
        assert_eq!(active.task_file, None);

        let _ = std::fs::remove_dir_all(&temp);
    }
}
