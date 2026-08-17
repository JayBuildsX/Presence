//! PresenceHub desktop application.
//!
//! Entry point for the Tauri-based desktop application.
//! The runtime is initialized and started here.

mod foreground;
mod registry;
mod runtime;

use presencehub_core::Config;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// File name of the desktop configuration file.
const CONFIG_FILE_NAME: &str = "presencehub.toml";

/// Run the PresenceHub desktop application.
///
/// This is the main entry point called from `main.rs`.
/// It loads configuration, initializes the runtime, and enters
/// the polling loop.
///
/// # Configuration
///
/// Configuration is resolved deterministically relative to the executable
/// (see [`resolve_config_path`]); if the file is missing or invalid, default
/// configuration is used.
///
/// # Shutdown
///
/// Press Ctrl+C to gracefully shut down the application.
/// The runtime will stop polling, shut down plugins, and exit.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing subscriber for logging.
    // Honor `RUST_LOG` when set, otherwise default to INFO so that launching
    // the executable directly (e.g. double-clicking it) produces the same
    // diagnostics as a terminal launch with `RUST_LOG=info`, without
    // requiring the environment variable.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(env_filter).init();
    info!("PresenceHub starting");

    // Load configuration (default if file missing or invalid)
    let config = match resolve_config_path() {
        Some(path) => {
            info!(config_path = %path.display(), "Loading configuration file");
            match Config::load(&path) {
                Ok(config) => config,
                Err(error) => {
                    warn!(config_path = %path.display(), error = %error, "Invalid configuration; using defaults");
                    Config::default()
                }
            }
        }
        None => {
            info!("No configuration file found; using defaults");
            Config::default()
        }
    };
    info!(
        poll_interval_ms = config.runtime.poll_interval_ms,
        flstudio = config.plugins.flstudio,
        league = config.plugins.league,
        opencode = config.plugins.opencode,
        console = config.outputs.console,
        discord = config.outputs.discord,
        discord_app_id = config.outputs.discord_app_id,
        discord_apps = ?config.outputs.discord_apps,
        "Configuration loaded"
    );

    // Create and configure the runtime
    let mut runtime = runtime::Runtime::with_config(config);

    // Start the runtime
    if let Err(e) = runtime.start() {
        eprintln!("Failed to start PresenceHub runtime: {}", e);
        return;
    }

    // Set up Ctrl+C handler for graceful shutdown
    let stop_signal = runtime.stop_signal();
    ctrlc::set_handler(move || {
        eprintln!("\nReceived Ctrl+C, shutting down...");
        stop_signal.store(false, std::sync::atomic::Ordering::SeqCst);
    })
    .ok();

    // Run the main polling loop (blocks until stopped)
    runtime.run();

    // Shutdown gracefully
    runtime.shutdown();
    info!("PresenceHub exited");
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
