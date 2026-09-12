//! Generic custom application plugin watcher.
//!
//! Allows users to add any application (e.g. Blender, Photoshop, Ableton)
//! without writing Rust code. It inspects process presence and foreground window
//! state to publish Rich Presence.

use presencehub_core::activity::{Activity, ActivityTimestamps};
use presencehub_core::CustomAppConfig;
use presencehub_plugin_host::{Plugin, PluginError, PluginMetadata, WindowIdentity};
use std::collections::HashMap;

pub struct CustomAppPlugin {
    config: CustomAppConfig,
    metadata: PluginMetadata,
    window_identity: WindowIdentity,
    session_start: Option<u64>,
}

impl CustomAppPlugin {
    pub fn new(config: CustomAppConfig) -> Self {
        let metadata = PluginMetadata::new(&config.name, "1.0.0");
        let window_identity = WindowIdentity::new(
            [config.process_name.to_ascii_lowercase()],
            Vec::<String>::new(),
        );
        Self {
            config,
            metadata,
            window_identity,
            session_start: None,
        }
    }

    #[allow(dead_code)]
    pub fn id(&self) -> &str {
        &self.config.id
    }

    #[allow(dead_code)]
    pub fn config(&self) -> &CustomAppConfig {
        &self.config
    }

    #[allow(dead_code)]
    pub fn update_config(&mut self, config: CustomAppConfig) {
        self.metadata = PluginMetadata::new(&config.name, "1.0.0");
        self.window_identity = WindowIdentity::new(
            [config.process_name.to_ascii_lowercase()],
            Vec::<String>::new(),
        );
        self.config = config;
    }
}

impl Plugin for CustomAppPlugin {
    fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    fn window_identity(&self) -> Option<WindowIdentity> {
        Some(self.window_identity.clone())
    }

    fn init(&mut self) -> Result<(), PluginError> {
        Ok(())
    }

    fn shutdown(&mut self) -> Result<(), PluginError> {
        self.session_start = None;
        Ok(())
    }

    fn poll(&mut self) -> Result<Option<Activity>, PluginError> {
        if !self.config.enabled {
            self.session_start = None;
            return Ok(None);
        }

        let is_running = is_process_running(&self.config.process_name);
        if !is_running {
            self.session_start = None;
            return Ok(None);
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let start_time = *self.session_start.get_or_insert(now);

        let mut primary_text = if self.config.state.trim().is_empty() {
            "Active".to_string()
        } else {
            self.config.state.clone()
        };

        // If the process is currently in the foreground, attempt to extract the window title
        let window = crate::foreground::foreground_window();
        if let Some(ref fg) = window {
            if crate::foreground::window_matches_identity(fg, &self.window_identity) {
                if let Some(title) = crate::foreground::get_foreground_window_title() {
                    let trimmed = title.trim();
                    if !trimmed.is_empty() && !trimmed.eq_ignore_ascii_case(&self.config.name) {
                        primary_text = trimmed.to_string();
                    }
                }
            }
        }

        // Determine Line 1 (primary activity / display name) and Line 2 (secondary details).
        // In Discord IPC: Line 1 renders as `details` and Line 2 renders as `state`.
        let line1 = if self.config.discord_app_id == 0 {
            if primary_text.eq_ignore_ascii_case("Active")
                || primary_text.eq_ignore_ascii_case(&self.config.name)
            {
                self.config.name.clone()
            } else {
                format!("{} — {}", self.config.name, primary_text)
            }
        } else {
            primary_text
        };

        let line2 = self
            .config
            .details
            .as_deref()
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        let (state, details) = match line2 {
            Some(secondary) => (secondary.to_string(), Some(line1)),
            None => (line1, None),
        };

        let mut metadata = HashMap::new();
        metadata.insert("application".to_string(), self.config.name.clone());

        if let Some(ref logo) = self.config.logo_asset {
            let trimmed = logo.trim();
            if !trimmed.is_empty() {
                metadata.insert("large_image".to_string(), trimmed.to_string());
            } else {
                metadata.insert("no_large_image".to_string(), "true".to_string());
            }
        } else {
            metadata.insert("no_large_image".to_string(), "true".to_string());
        }

        Ok(Some(Activity {
            state,
            details,
            timestamps: Some(ActivityTimestamps {
                start: Some(start_time as i64),
                end: None,
            }),
            application: Some(self.config.name.clone()),
            metadata,
        }))
    }
}

/// Checks if a process with the specified base name is currently running.
#[cfg(target_os = "windows")]
pub fn is_process_running(process_name: &str) -> bool {
    use winapi::um::handleapi::CloseHandle;
    use winapi::um::tlhelp32::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };

    let target = process_name.to_ascii_lowercase();
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == winapi::um::handleapi::INVALID_HANDLE_VALUE {
            return false;
        }

        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;

        if Process32FirstW(snapshot, &mut entry) != 0 {
            loop {
                let null_pos = entry
                    .szExeFile
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(entry.szExeFile.len());
                let exe_name =
                    String::from_utf16_lossy(&entry.szExeFile[..null_pos]).to_ascii_lowercase();
                if exe_name == target {
                    CloseHandle(snapshot);
                    return true;
                }
                if Process32NextW(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }

        CloseHandle(snapshot);
    }
    false
}

#[cfg(not(target_os = "windows"))]
pub fn is_process_running(_process_name: &str) -> bool {
    false
}
