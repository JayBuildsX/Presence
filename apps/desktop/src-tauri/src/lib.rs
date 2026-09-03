//! PresenceHub desktop application.
//!
//! Entry point for the Tauri-based desktop application.
//! The runtime is initialized and started here.

mod commands;
mod foreground;
mod registry;
mod runtime;
mod state;

use presencehub_core::Config;
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

    // 1. Locate config file (executable-adjacent or ancestor-adjacent).
    let config_path = resolve_config_path();
    let config = match &config_path {
        Some(path) => {
            info!(path = %path.display(), "Loading configuration");
            Config::load(path).unwrap_or_else(|e| {
                warn!(error = %e, "Invalid configuration file, falling back to defaults");
                Config::default()
            })
        }
        None => {
            info!("No configuration file found, using defaults");
            Config::default()
        }
    };

    // 2. Start the runtime with loaded config.
    let mut runtime = runtime::Runtime::with_config(config);
    runtime.set_config_path(config_path);
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
            commands::drag_window,
            commands::close_window,
            commands::reconnect_discord,
            commands::quit_app,
            commands::set_pinned_source,
            commands::get_autostart_status,
            commands::set_autostart,
            commands::register_custom_shortcut,
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
                "Open PresenceHub",
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
                "Quit PresenceHub",
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
                .tooltip("PresenceHub - Starting...")
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

            if let Ok(mut lock) = shortcut_map.0.write() {
                lock.insert(pause_shortcut.clone(), "pause".to_string());
                lock.insert(reconnect_shortcut.clone(), "reconnect".to_string());
                lock.insert(toggle_win_shortcut.clone(), "toggle_window".to_string());
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
                            "PresenceHub: Paused".to_string()
                        } else if !snapshot.discord_connected {
                            "PresenceHub: Discord Disconnected".to_string()
                        } else if let Some(ref current) = snapshot.current {
                            let mut text =
                                format!("PresenceHub: {} — {}", current.source, current.state);
                            if text.len() > 60 {
                                text.truncate(57);
                                text.push_str("...");
                            }
                            text
                        } else {
                            "PresenceHub: Watching (Idle)".to_string()
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
        .expect("failed to build PresenceHub window")
        .run(|app, event| {
            // Synchronous shutdown on exit so Discord presence is cleared.
            if let tauri::RunEvent::Exit = event {
                let state = app.state::<AppState>();
                tauri::async_runtime::block_on(async {
                    state.runtime.lock().await.shutdown();
                });
                info!("PresenceHub exited");
            }
        });
}

/// Resolve the configuration file path for the desktop application.
///
/// The config is resolved deterministically so the application behaves the
/// same whether it is launched from a terminal or by double-clicking the
/// executable:
///
/// 1. `presencehub.toml` in the executable's directory (portable layout:
///    config shipped next to the exe).
/// 2. `presencehub.toml` in the executable's directory ancestors (source-tree
///    layout: `target\debug\presencehub-desktop.exe` finds a config at the
///    project root).
/// 3. `presencehub.toml` in the current working directory and its ancestors.
///    (Terminal launches, including `cargo run`, use the project root as the
///    working directory, so this keeps the existing development workflow.)
///
/// Order matters: an explicit exe-adjacent config wins over one resolved from
/// the working directory, and the nearest ancestor wins. Explorer launches a
/// process with the working directory set to `C:\Windows\system32`, so a
/// purely working-directory-relative lookup would silently miss the config;
/// the exe-relative search guarantees a deterministic result regardless of
/// how the application was started.
///
/// If none of the candidates exist, `None` is returned and the caller falls
/// back to default configuration (existing missing-config semantics).
fn resolve_config_path() -> Option<PathBuf> {
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

    find_first_existing_config(candidates.iter().map(PathBuf::as_path))
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
