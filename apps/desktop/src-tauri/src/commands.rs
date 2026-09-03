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
