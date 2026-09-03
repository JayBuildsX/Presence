//! Tauri commands bridging the GUI and the runtime.
//!
//! Every command locks the shared runtime, performs exactly one backend
//! operation, and returns a fresh [`LiveState`](crate::state::LiveState).
//! The frontend renders the returned state verbatim; on error it shows the
//! message and refreshes, so the UI can never permanently disagree with
//! the backend.

use tauri::State;

use crate::state::{AppState, LiveState};

/// Returns the current backend snapshot for one render pass.
#[tauri::command]
pub async fn get_state(state: State<'_, AppState>) -> Result<LiveState, String> {
    Ok(state.runtime.lock().await.snapshot())
}

/// Enables or disables a plugin in the backend and persists the toggle.
/// Returns the fresh snapshot.
#[tauri::command]
pub async fn set_plugin_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_plugin_enabled(&name, enabled)?;
    Ok(runtime.snapshot())
}

/// Pauses or resumes the whole runtime. Returns the fresh snapshot.
#[tauri::command]
pub async fn set_paused(state: State<'_, AppState>, paused: bool) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_paused(paused);
    Ok(runtime.snapshot())
}

/// Updates the polling interval in the backend and persists it. Returns the fresh snapshot.
#[tauri::command]
pub async fn set_poll_interval(
    state: State<'_, AppState>,
    interval_ms: u64,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_poll_interval_ms(interval_ms)?;
    Ok(runtime.snapshot())
}

/// Minimizes the application window.
#[tauri::command]
pub async fn minimize_window(window: tauri::Window) -> Result<(), String> {
    window.minimize().map_err(|e| e.to_string())
}

/// Starts dragging the application window.
#[tauri::command]
pub async fn drag_window(window: tauri::Window) -> Result<(), String> {
    window.start_dragging().map_err(|e| e.to_string())
}

/// Reconnects outputs (Discord) and republishes active rich presence.
#[tauri::command]
pub async fn reconnect_discord(state: State<'_, AppState>) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.reconnect_discord()?;
    Ok(runtime.snapshot())
}

/// Hides the application window to system tray.
#[tauri::command]
pub async fn close_window(window: tauri::Window) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// Explicitly terminates the application.
#[tauri::command]
pub async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

/// Manually pins or unpins a source as the primary presence.
#[tauri::command]
pub async fn set_pinned_source(
    state: State<'_, AppState>,
    source: Option<String>,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_pinned_source(source);
    Ok(runtime.snapshot())
}

/// Returns whether PresenceHub is set to auto-start with Windows.
#[tauri::command]
pub async fn get_autostart_status() -> Result<bool, String> {
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            "/v",
            "PresenceHub",
        ])
        .output()
        .map_err(|e| format!("Failed to query registry: {e}"))?;
    Ok(output.status.success())
}

/// Enables or disables auto-start with Windows (with --minimized flag).
#[tauri::command]
pub async fn set_autostart(enabled: bool) -> Result<bool, String> {
    if enabled {
        let exe_path =
            std::env::current_exe().map_err(|e| format!("Failed to get executable path: {e}"))?;
        let cmd_value = format!("\"{}\" --minimized", exe_path.to_string_lossy());
        let status = std::process::Command::new("reg")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "PresenceHub",
                "/t",
                "REG_SZ",
                "/d",
                &cmd_value,
                "/f",
            ])
            .status()
            .map_err(|e| format!("Failed to execute reg command: {e}"))?;
        if !status.success() {
            return Err("Failed to add registry entry for auto-start".to_string());
        }
    } else {
        let _ = std::process::Command::new("reg")
            .args([
                "delete",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
                "/v",
                "PresenceHub",
                "/f",
            ])
            .status();
    }
    get_autostart_status().await
}

/// Rebinds a global shortcut dynamically.
#[tauri::command]
pub async fn register_custom_shortcut(
    app: tauri::AppHandle,
    action: String,
    old_shortcut: Option<String>,
    new_shortcut: String,
) -> Result<(), String> {
    use crate::state::ShortcutMap;
    use tauri::Manager;
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let map = app.state::<ShortcutMap>();
    if let Some(old) = old_shortcut {
        if let Ok(sc) = old.parse::<Shortcut>() {
            let _ = app.global_shortcut().unregister(sc.clone());
            if let Ok(mut lock) = map.0.write() {
                lock.remove(&sc);
            }
        }
    }
    let sc = new_shortcut
        .parse::<Shortcut>()
        .map_err(|e| format!("Invalid shortcut: {e}"))?;
    app.global_shortcut()
        .register(sc.clone())
        .map_err(|e| format!("Failed to register shortcut: {e}"))?;
    if let Ok(mut lock) = map.0.write() {
        lock.insert(sc, action);
    }
    Ok(())
}
