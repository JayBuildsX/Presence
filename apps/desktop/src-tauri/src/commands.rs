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
///
/// Returns the fresh snapshot on success. When still disconnected, returns
/// the recorded failure reason (e.g. no IPC pipe) so a machine where
/// Discord looks open but unreachable gets an answer instead of silence.
#[tauri::command]
pub async fn reconnect_discord(state: State<'_, AppState>) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    let connected = runtime.reconnect_discord().unwrap_or(false);
    let snapshot = runtime.snapshot();
    if connected {
        Ok(snapshot)
    } else {
        Err(snapshot
            .discord_error
            .unwrap_or_else(|| "Discord is not reachable".to_string()))
    }
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
            let _ = app.global_shortcut().unregister(sc);
            if let Ok(mut lock) = map.0.write() {
                lock.remove(&sc);
            }
        }
    }
    let sc = new_shortcut
        .parse::<Shortcut>()
        .map_err(|e| format!("Invalid shortcut: {e}"))?;
    app.global_shortcut()
        .register(sc)
        .map_err(|e| format!("Failed to register shortcut: {e}"))?;
    if let Ok(mut lock) = map.0.write() {
        lock.insert(sc, action);
    }
    Ok(())
}

/// Returns currently running top-level desktop applications.
#[tauri::command]
pub async fn get_running_applications() -> Result<Vec<crate::foreground::RunningProcessView>, String>
{
    Ok(crate::foreground::enumerate_running_applications())
}

/// Adds a custom process watcher application.
#[tauri::command]
pub async fn add_custom_app(
    state: State<'_, AppState>,
    app: presencehub_core::CustomAppConfig,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.add_custom_app(app)?;
    Ok(runtime.snapshot())
}

/// Updates an existing custom process watcher application.
#[tauri::command]
pub async fn update_custom_app(
    state: State<'_, AppState>,
    app: presencehub_core::CustomAppConfig,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.update_custom_app(app)?;
    Ok(runtime.snapshot())
}

/// Removes a custom process watcher application.
#[tauri::command]
pub async fn remove_custom_app(
    state: State<'_, AppState>,
    id: String,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.remove_custom_app(&id)?;
    Ok(runtime.snapshot())
}

/// Toggles Streamer / Privacy Mode.
#[tauri::command]
pub async fn set_streamer_mode(
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<LiveState, String> {
    let mut runtime = state.runtime.lock().await;
    runtime.set_streamer_mode(enabled);
    Ok(runtime.snapshot())
}

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiscordAppDetails {
    pub id: String,
    pub name: String,
    pub logo_asset: Option<String>,
}

async fn fetch_curl(url: &str) -> Result<String, String> {
    let url = url.to_string();
    tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new("curl.exe");
        cmd.args(["-s", "-m", "4", &url]);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let output = cmd
            .output()
            .map_err(|e| format!("Failed to run curl: {}", e))?;
        if !output.status.success() {
            return Err("Discord request failed".to_string());
        }
        String::from_utf8(output.stdout).map_err(|e| format!("Invalid UTF-8 response: {}", e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Inspects a Discord application by ID, retrieving its application name and any registered logo assets.
#[tauri::command]
pub async fn inspect_discord_app(app_id: String) -> Result<DiscordAppDetails, String> {
    let trimmed = app_id.trim();
    if trimmed.is_empty() {
        return Err("Application ID cannot be empty".to_string());
    }

    let parsed_id: u64 = trimmed
        .parse()
        .map_err(|_| "Application ID must be a numeric snowflake ID (e.g. 1533559059125637311)".to_string())?;

    if parsed_id == 0 {
        return Err("Application ID must be greater than 0".to_string());
    }

    let rpc_url = format!("https://discord.com/api/v10/applications/{}/rpc", parsed_id);
    let rpc_body = fetch_curl(&rpc_url).await?;
    let rpc_json: serde_json::Value = serde_json::from_str(&rpc_body)
        .map_err(|e| format!("Invalid JSON from Discord RPC: {}", e))?;

    let name = rpc_json
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            if let Some(msg) = rpc_json.get("message").and_then(|v| v.as_str()) {
                if msg.eq_ignore_ascii_case("unknown application") {
                    "Discord error: Unknown Application. Please make sure this is the Application ID from General Information in Discord Developer Portal (not the Public Key).".to_string()
                } else {
                    format!("Discord error: {}", msg)
                }
            } else {
                "Application not found or inaccessible on Discord".to_string()
            }
        })?
        .to_string();

    let assets_url = format!(
        "https://discord.com/api/v10/oauth2/applications/{}/assets",
        parsed_id
    );
    let logo_asset = match fetch_curl(&assets_url).await {
        Ok(assets_body) => {
            if let Ok(serde_json::Value::Array(assets)) = serde_json::from_str(&assets_body) {
                let mut found = None;
                // 1. Search for asset named "logo"
                for asset in &assets {
                    if let Some(asset_name) = asset.get("name").and_then(|v| v.as_str()) {
                        if asset_name.eq_ignore_ascii_case("logo") {
                            found = Some(asset_name.to_string());
                            break;
                        }
                    }
                }
                // 2. Search for asset named "app_logo" or "icon"
                if found.is_none() {
                    for asset in &assets {
                        if let Some(asset_name) = asset.get("name").and_then(|v| v.as_str()) {
                            if asset_name.eq_ignore_ascii_case("app_logo")
                                || asset_name.eq_ignore_ascii_case("icon")
                            {
                                found = Some(asset_name.to_string());
                                break;
                            }
                        }
                    }
                }
                // 3. Fallback to first available asset if any exist
                if found.is_none() {
                    if let Some(first) = assets
                        .first()
                        .and_then(|a| a.get("name"))
                        .and_then(|v| v.as_str())
                    {
                        found = Some(first.to_string());
                    }
                }
                found
            } else {
                None
            }
        }
        Err(_) => None,
    };

    Ok(DiscordAppDetails {
        id: parsed_id.to_string(),
        name,
        logo_asset,
    })
}
