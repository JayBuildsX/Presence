//! PresenceHub desktop application.
//!
//! Entry point for the Tauri-based desktop application.
//! The runtime is initialized and started here.

mod commands;
mod custom_app;
mod foreground;
mod registry;
mod runtime;
mod state;

use presencehub_core::{Config, OwnershipPolicy, UnsupportedForegroundPolicy};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::{Emitter, Manager};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
use tracing::{info, warn};

use state::{AppState, ShortcutMap};

/// File name of the desktop configuration file.
const CONFIG_FILE_NAME: &str = "presencehub.toml";

/// Run the PresenceHub desktop application.
///
/// Loads configuration, starts the runtime, then opens the GUI window and
/// drives polling on a background task. The frontend talks to the backend
/// exclusively through Tauri commands (`get_state`, `set_plugin_enabled`,
/// `set_paused`); the backend remains authoritative for all state.
///
/// # Shutdown
///
/// Closing the window ends the Tauri event loop, after which the runtime
/// is shut down so every output (notably Discord) is cleared.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing subscriber for logging.
    // Honor `RUST_LOG` when set, otherwise default to INFO so that launching
    // the executable directly (e.g. double-clicking it) produces the same
    // diagnostics as a terminal launch with `RUST_LOG=info`, without
    // requiring the environment variable.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // 1. Locate config file (executable-adjacent, ancestor, or %APPDATA%).
    let config_path = resolve_config_path();
    info!(path = %config_path.display(), "Loading configuration");
    let mut config = Config::load(&config_path).unwrap_or_else(|e| {
        warn!(error = %e, "Invalid configuration file, falling back to desktop defaults");
        desktop_default_config()
    });

    // Ensure desktop Discord output is properly configured with app IDs even if
    // an older or partial configuration omitted them.
    if !config.outputs.discord || config.outputs.discord_apps.is_empty() {
        info!("Ensuring Discord output and default app IDs are active");
        config.outputs.discord = true;
        if config.outputs.discord_app_id == 0 {
            config.outputs.discord_app_id = 1533559059125637311;
        }
        if !config.outputs.discord_apps.contains_key("FL Studio") {
            config
                .outputs
                .discord_apps
                .insert("FL Studio".to_string(), 1192880494086455357);
        }
        if !config.outputs.discord_apps.contains_key("Antigravity") {
            config
                .outputs
                .discord_apps
                .insert("Antigravity".to_string(), 1543009205785591868);
        }
        if !config.outputs.discord_apps.contains_key("OpenCode") {
            config
                .outputs
                .discord_apps
                .insert("OpenCode".to_string(), 1273940066603106328);
        }
    }

    // 2. Start the runtime with loaded config.
    let mut runtime = runtime::Runtime::with_config(config);
    runtime.set_config_path(Some(config_path));
    if let Err(e) = runtime.start() {
        warn!(error = %e, "Failed to start runtime during initialization");
    }

    let state = AppState {
        runtime: Arc::new(tokio::sync::Mutex::new(runtime)),
    };

    // 3. Build and launch Tauri application.
    tauri::Builder::default()
        .manage(state)
        .manage(ShortcutMap::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_plugin_enabled,
            commands::set_paused,
            commands::set_poll_interval,
            commands::minimize_window,
            commands::toggle_maximize_window,
            commands::is_window_maximized,
            commands::drag_window,
            commands::close_window,
            commands::reconnect_discord,
            commands::quit_app,
            commands::set_pinned_source,
            commands::get_autostart_status,
            commands::set_autostart,
            commands::register_custom_shortcut,
            commands::get_running_applications,
            commands::add_custom_app,
            commands::update_custom_app,
            commands::remove_custom_app,
            commands::set_streamer_mode,
            commands::inspect_discord_app,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent real termination; hide to Windows system tray instead.
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .setup(|app| {
            // If launched with --minimized, hide to system tray on start
            if std::env::args().any(|a| a == "--minimized") {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.hide();
                }
            }

            // Setup System Tray
            let show_item = tauri::menu::MenuItem::with_id(
                app,
                "show",
                "Open Presence",
                true,
                None::<&str>,
            )?;
            let toggle_pause_item = tauri::menu::MenuItem::with_id(
                app,
                "toggle_pause",
                "Pause / Resume",
                true,
                None::<&str>,
            )?;
            let reconnect_item = tauri::menu::MenuItem::with_id(
                app,
                "reconnect",
                "Reconnect Discord",
                true,
                None::<&str>,
            )?;
            let quit_item = tauri::menu::MenuItem::with_id(
                app,
                "quit",
                "Quit Presence",
                true,
                None::<&str>,
            )?;
            let menu = tauri::menu::Menu::with_items(
                app,
                &[&show_item, &toggle_pause_item, &reconnect_item, &quit_item],
            )?;

            let icon = app
                .default_window_icon()
                .cloned()
                .expect("default window icon");

            let _tray = tauri::tray::TrayIconBuilder::with_id("main")
                .icon(icon)
                .tooltip("Presence - Starting...")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.unminimize();
                            let _ = window.set_focus();
                        }
                    }
                    "toggle_pause" => {
                        let state = app.state::<AppState>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let mut runtime = state.runtime.lock().await;
                            let paused = runtime.is_paused();
                            runtime.set_paused(!paused);
                        });
                    }
                    "reconnect" => {
                        let state = app.state::<AppState>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let mut runtime = state.runtime.lock().await;
                            let _ = runtime.reconnect_discord();
                        });
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        button_state: tauri::tray::MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let is_visible = window.is_visible().unwrap_or(false);
                            if is_visible {
                                let _ = window.hide();
                            } else {
                                let _ = window.show();
                                let _ = window.unminimize();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            // Setup Global Shortcuts
            let shortcut_map = app.state::<ShortcutMap>().inner().clone();
            let pause_shortcut: Shortcut = "CommandOrControl+Shift+P".parse().unwrap();
            let reconnect_shortcut: Shortcut = "CommandOrControl+Shift+D".parse().unwrap();
            let toggle_win_shortcut: Shortcut = "CommandOrControl+Shift+H".parse().unwrap();
            let streamer_shortcut: Shortcut = "CommandOrControl+Shift+S".parse().unwrap();

            if let Ok(mut lock) = shortcut_map.0.write() {
                lock.insert(pause_shortcut, "pause".to_string());
                lock.insert(reconnect_shortcut, "reconnect".to_string());
                lock.insert(toggle_win_shortcut, "toggle_window".to_string());
                lock.insert(streamer_shortcut, "streamer_mode".to_string());
            }

            let app_handle_for_shortcuts = app.handle().clone();
            let state_for_shortcuts = app.state::<AppState>().inner().clone();
            let map_for_handler = shortcut_map.clone();

            let shortcut_plugin = tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |_app, shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let action = map_for_handler
                            .0
                            .read()
                            .ok()
                            .and_then(|m| m.get(shortcut).cloned());
                        if let Some(action) = action {
                            match action.as_str() {
                                "pause" => {
                                    let state = state_for_shortcuts.clone();
                                    let app_handle = app_handle_for_shortcuts.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let mut runtime = state.runtime.lock().await;
                                        let paused = runtime.is_paused();
                                        let next = !paused;
                                        runtime.set_paused(next);
                                        let _ = app_handle.emit(
                                            "toast",
                                            serde_json::json!({
                                                "text": if next { "Broadcasting paused" } else { "Broadcasting resumed" },
                                                "type": "info"
                                            }),
                                        );
                                    });
                                }
                                "reconnect" => {
                                    let state = state_for_shortcuts.clone();
                                    let app_handle = app_handle_for_shortcuts.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let mut runtime = state.runtime.lock().await;
                                        let ok = runtime.reconnect_discord().unwrap_or(false);
                                        let _ = app_handle.emit(
                                            "toast",
                                            serde_json::json!({
                                                "text": if ok { "Discord reconnected successfully" } else { "Could not connect to Discord pipe" },
                                                "type": if ok { "success" } else { "warning" }
                                            }),
                                        );
                                    });
                                }
                                "streamer_mode" => {
                                    let state = state_for_shortcuts.clone();
                                    let app_handle = app_handle_for_shortcuts.clone();
                                    tauri::async_runtime::spawn(async move {
                                        let mut runtime = state.runtime.lock().await;
                                        let curr = runtime.snapshot().streamer_mode;
                                        let next = !curr;
                                        runtime.set_streamer_mode(next);
                                        let _ = app_handle.emit(
                                            "toast",
                                            serde_json::json!({
                                                "text": if next { "🛡️ Streamer Mode Enabled" } else { "Streamer Mode Disabled" },
                                                "type": if next { "success" } else { "info" }
                                            }),
                                        );
                                    });
                                }
                                "toggle_window" => {
                                    if let Some(window) =
                                        app_handle_for_shortcuts.get_webview_window("main")
                                    {
                                        let is_visible = window.is_visible().unwrap_or(false);
                                        if is_visible {
                                            let _ = window.hide();
                                        } else {
                                            let _ = window.show();
                                            let _ = window.unminimize();
                                            let _ = window.set_focus();
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                })
                .build();

            app.handle().plugin(shortcut_plugin)?;
            let _ = app.global_shortcut().register(pause_shortcut);
            let _ = app.global_shortcut().register(reconnect_shortcut);
            let _ = app.global_shortcut().register(toggle_win_shortcut);
            let _ = app.global_shortcut().register(streamer_shortcut);

            // Drive polling on a background task; each iteration locks the
            // runtime briefly so GUI commands interleave between polls.
            let state = app.state::<AppState>().inner().clone();
            let app_handle_tray = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    let (interval, tooltip) = {
                        let mut runtime = state.runtime.lock().await;
                        if !runtime.is_running() {
                            break;
                        }
                        runtime.poll_once();
                        let snapshot = runtime.snapshot();
                        let tooltip = if snapshot.paused {
                            "Presence: Paused".to_string()
                        } else if !snapshot.discord_connected {
                            "Presence: Discord Disconnected".to_string()
                        } else if let Some(ref current) = snapshot.current {
                            let mut text =
                                format!("Presence: {} — {}", current.source, current.state);
                            if text.len() > 60 {
                                text.truncate(57);
                                text.push_str("...");
                            }
                            text
                        } else {
                            "Presence: Watching (Idle)".to_string()
                        };
                        (runtime.poll_interval(), tooltip)
                    };

                    if let Some(tray) = app_handle_tray.tray_by_id("main") {
                        let _ = tray.set_tooltip(Some(tooltip));
                    }

                    tokio::time::sleep(interval).await;
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build Presence window")
        .run(|app, event| {
            // Synchronous shutdown on exit so Discord presence is cleared.
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.runtime.lock().await.shutdown();
                });
                info!("Presence exited");
            }
        });
}

/// Returns the full desktop production configuration with Discord enabled
/// and preconfigured application IDs for supported applications.
pub fn desktop_default_config() -> Config {
    let mut config = Config::default();
    config.runtime.poll_interval_ms = 500;
    config.plugins.flstudio = true;
    config.plugins.antigravity = true;
    config.plugins.opencode = true;
    config.outputs.console = true;
    config.outputs.discord = true;
    config.outputs.discord_app_id = 1533559059125637311;
    let mut discord_apps = HashMap::new();
    discord_apps.insert("OpenCode".to_string(), 1273940066603106328);
    discord_apps.insert("Antigravity".to_string(), 1543009205785591868);
    discord_apps.insert("FL Studio".to_string(), 1192880494086455357);
    config.outputs.discord_apps = discord_apps;
    config.presence.ownership = OwnershipPolicy::Foreground;
    config.presence.unsupported_foreground = UnsupportedForegroundPolicy::KeepLast;
    config
}

/// Resolve the configuration file path for the desktop application.
///
/// 1. `presencehub.toml` in the executable's directory or ancestors (portable / developer).
/// 2. `presencehub.toml` in the working directory or ancestors.
/// 3. `%APPDATA%\PresenceHub\presencehub.toml` (standard Windows installation).
///    If it does not exist, the directory is created and populated with the default
///    desktop configuration so user settings persist across sessions.
fn resolve_config_path() -> PathBuf {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.to_path_buf());
            candidates.extend(exe_dir.ancestors().skip(1).map(Path::to_path_buf));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.clone());
        candidates.extend(cwd.ancestors().skip(1).map(Path::to_path_buf));
    }

    if let Some(existing) = find_first_existing_config(candidates.iter().map(PathBuf::as_path)) {
        return existing;
    }

    let appdata_dir = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("USERPROFILE")
                .map(|p| PathBuf::from(p).join("AppData").join("Roaming"))
                .unwrap_or_else(|_| PathBuf::from("."))
        })
        .join("PresenceHub");

    let appdata_config = appdata_dir.join(CONFIG_FILE_NAME);
    if appdata_config.is_file() {
        return appdata_config;
    }

    if let Err(e) = std::fs::create_dir_all(&appdata_dir) {
        warn!(error = %e, "Failed to create AppData directory for PresenceHub");
    } else {
        let default_config = desktop_default_config();
        if let Err(e) = default_config.save(&appdata_config) {
            warn!(error = %e, "Failed to write default configuration to AppData");
        } else {
            info!(path = %appdata_config.display(), "Initialized default desktop configuration");
        }
    }

    appdata_config
}

/// Returns the first candidate directory (in order) that contains the
/// configuration file, joined with the file name.
///
/// Pure helper so the resolution order can be unit-tested with a fake
/// directory tree without touching the process environment.
fn find_first_existing_config<'a>(dirs: impl IntoIterator<Item = &'a Path>) -> Option<PathBuf> {
    dirs.into_iter()
        .map(|dir| dir.join(CONFIG_FILE_NAME))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Build a scratch directory tree under the system temp dir and return
    /// the root, so the resolution order is tested without touching the
    /// process working directory.
    fn scratch_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("ph_cfg_test_{name}"));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn touch(root: &Path, rel: &str) -> PathBuf {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "config_version = 1").unwrap();
        path
    }

    #[test]
    fn finds_config_next_to_executable() {
        let root = scratch_root("exe_adjacent");
        let expected = touch(&root, "bin/presencehub.toml");
        let exe_dir = root.join("bin");

        let found = find_first_existing_config([exe_dir.as_path()]);
        assert_eq!(found, Some(expected));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_config_in_executable_ancestor() {
        let root = scratch_root("exe_ancestor");
        let expected = touch(&root, "target/presencehub.toml");
        let exe_dir = root.join("target/debug");

        let candidates = [exe_dir.as_path(), exe_dir.parent().unwrap()];
        let found = find_first_existing_config(candidates);
        assert_eq!(found, Some(expected));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exe_adjacent_wins_over_cwd() {
        let root = scratch_root("precedence");
        let expected = touch(&root, "bin/presencehub.toml");
        touch(&root, "cwd/presencehub.toml");
        let exe_dir = root.join("bin");
        let cwd = root.join("cwd");

        let candidates = [exe_dir.as_path(), cwd.as_path()];
        let found = find_first_existing_config(candidates);
        assert_eq!(found, Some(expected));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nearest_ancestor_wins() {
        let root = scratch_root("nearest");
        let expected = touch(&root, "target/presencehub.toml");
        touch(&root, "presencehub.toml");
        let exe_dir = root.join("target/debug");

        let candidates = [exe_dir.as_path(), exe_dir.parent().unwrap()];
        let found = find_first_existing_config(candidates);
        assert_eq!(found, Some(expected));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn returns_none_when_no_config_exists() {
        let root = scratch_root("none");
        let exe_dir = root.join("bin");

        let found = find_first_existing_config([exe_dir.as_path()]);
        assert_eq!(found, None);
        let _ = fs::remove_dir_all(&root);
    }
}
