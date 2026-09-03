//! Foreground window detection and generic source matching.
//!
//! The foreground-ownership policy needs to know which supported application
//! currently owns the OS foreground window. This module reads only that
//! identity (process base name + window class name) and maps it back to a
//! registered plugin source using the plugin-declared [`WindowIdentity`].
//!
//! Nothing here knows a concrete application name — the mapping is driven
//! entirely by plugin-provided capability information, so a new plugin with
//! a declared window identity works automatically.

use presencehub_plugin_host::WindowIdentity;

/// The OS foreground window's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForegroundWindow {
    /// Process base name, lowercased (e.g. `"fl64.exe"`).
    pub process_name: Option<String>,
    /// Window class name (e.g. `"TFruityLoopsMainForm"`).
    pub window_class: Option<String>,
}

/// Returns the foreground window's identity, or `None` when no window is
/// foreground or the platform does not expose one.
#[cfg(target_os = "windows")]
pub fn foreground_window() -> Option<ForegroundWindow> {
    let hwnd = unsafe { winapi::um::winuser::GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }

    // Window class name.
    let mut class_buf = [0u16; 256];
    let class_len = unsafe {
        winapi::um::winuser::GetClassNameW(hwnd, class_buf.as_mut_ptr(), class_buf.len() as i32)
    };
    let window_class = if class_len > 0 {
        Some(String::from_utf16_lossy(&class_buf[..class_len as usize]))
    } else {
        None
    };

    // Owning process. Querying the image name is strictly read-only: no
    // process memory is accessed and nothing is injected or modified.
    let mut pid: winapi::shared::minwindef::DWORD = 0;
    unsafe {
        winapi::um::winuser::GetWindowThreadProcessId(hwnd, &mut pid);
    }
    let process_name = if pid != 0 {
        process_base_name(pid)
    } else {
        None
    };

    Some(ForegroundWindow {
        process_name,
        window_class,
    })
}

/// Resolves a process id to its base name (lowercased), if permitted.
#[cfg(target_os = "windows")]
fn process_base_name(pid: winapi::shared::minwindef::DWORD) -> Option<String> {
    use winapi::shared::minwindef::{DWORD, FALSE};
    use winapi::um::errhandlingapi::GetLastError;
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::processthreadsapi::OpenProcess;
    use winapi::um::winbase::QueryFullProcessImageNameW;
    use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;

    unsafe {
        // PROCESS_QUERY_LIMITED_INFORMATION is sufficient for reading the
        // image name and is granted even to non-administrators.
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid);
        if handle.is_null() {
            return None;
        }

        let mut buf = [0u16; 260];
        let mut size = buf.len() as DWORD;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        let last_error = GetLastError();
        CloseHandle(handle);

        if ok == 0 || size == 0 {
            debug_assert_eq!(last_error, 0, "QueryFullProcessImageNameW failed");
            return None;
        }

        let path = String::from_utf16_lossy(&buf[..size as usize]);
        Some(base_name_of(&path))
    }
}

/// Returns the lowercased file name portion of a Windows path.
fn base_name_of(path: &str) -> String {
    path.rsplit(['\\', '/'])
        .next()
        .unwrap_or(path)
        .to_ascii_lowercase()
}

/// Whether a foreground window belongs to a plugin's declared identity.
///
/// A match is any declared process base name or window class name equal to
/// the window's (case-insensitive).
pub fn window_matches_identity(window: &ForegroundWindow, identity: &WindowIdentity) -> bool {
    if let Some(ref process) = window.process_name {
        let process = process.to_ascii_lowercase();
        if identity
            .process_names
            .iter()
            .any(|p| p.to_ascii_lowercase() == process)
        {
            return true;
        }
    }
    if let Some(ref class) = window.window_class {
        let class = class.to_ascii_lowercase();
        if identity
            .window_classes
            .iter()
            .any(|c| c.to_ascii_lowercase() == class)
        {
            return true;
        }
    }
    false
}

/// Resolves the foreground window to a registered source name, if any.
///
/// `sources` pairs each registered source name with its declared window
/// identity. The first source whose identity matches the foreground window
/// is returned. Purely generic: no concrete application is referenced.
pub fn resolve_foreground_source<'a>(
    window: &ForegroundWindow,
    sources: &[(&'a str, &'a WindowIdentity)],
) -> Option<&'a str> {
    sources
        .iter()
        .find(|(_, identity)| window_matches_identity(window, identity))
        .map(|(name, _)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_match_returns_none() {
        let window = ForegroundWindow {
            process_name: Some("chrome.exe".to_string()),
            window_class: Some("Chrome_WidgetWin_1".to_string()),
        };
        let identity = WindowIdentity::new(["fl64.exe"], ["TFruityLoopsMainForm"]);
        assert!(!window_matches_identity(&window, &identity));
    }

    #[test]
    fn matches_process_base_name_case_insensitively() {
        let window = ForegroundWindow {
            process_name: Some("FL64.EXE".to_string()),
            window_class: None,
        };
        let identity = WindowIdentity::new(["fl64.exe"], ["TFruityLoopsMainForm"]);
        assert!(window_matches_identity(&window, &identity));
    }

    #[test]
    fn matches_window_class() {
        let window = ForegroundWindow {
            process_name: None,
            window_class: Some("TFruityLoopsMainForm".to_string()),
        };
        let identity = WindowIdentity::new(["fl64.exe"], ["TFruityloopsmainform"]);
        assert!(window_matches_identity(&window, &identity));
    }

    #[test]
    fn resolves_first_matching_source() {
        let window = ForegroundWindow {
            process_name: Some("antigravity.exe".to_string()),
            window_class: Some("Chrome_WidgetWin_1".to_string()),
        };
        let fl = WindowIdentity::new(["fl64.exe"], ["TFruityLoopsMainForm"]);
        let antigravity = WindowIdentity::new(["antigravity.exe"], Vec::<String>::new());

        assert_eq!(
            resolve_foreground_source(
                &window,
                &[("FL Studio", &fl), ("Antigravity", &antigravity)]
            ),
            Some("Antigravity")
        );
    }

    #[test]
    fn unsupported_foreground_resolves_to_none() {
        let window = ForegroundWindow {
            process_name: Some("notepad.exe".to_string()),
            window_class: Some("Notepad".to_string()),
        };
        let fl = WindowIdentity::new(["fl64.exe"], ["TFruityLoopsMainForm"]);
        assert_eq!(
            resolve_foreground_source(&window, &[("FL Studio", &fl)]),
            None
        );
    }

    #[test]
    fn identity_with_no_patterns_never_matches() {
        let window = ForegroundWindow {
            process_name: Some("fl64.exe".to_string()),
            window_class: Some("TFruityLoopsMainForm".to_string()),
        };
        let empty = WindowIdentity::default();
        assert!(!window_matches_identity(&window, &empty));
    }

    #[test]
    fn base_name_extraction() {
        assert_eq!(
            base_name_of(r"C:\Users\HP\AppData\Local\Programs\Antigravity\antigravity.exe"),
            "antigravity.exe"
        );
        assert_eq!(base_name_of("fl64.exe"), "fl64.exe");
    }
}
