import { invoke } from "@tauri-apps/api/core";
import type { CustomAppConfig, DiscordAppDetails, LiveState, RunningProcessView } from "./types";

export async function getState(): Promise<LiveState> {
  return invoke<LiveState>("get_state");
}

export async function setPluginEnabled(
  name: string,
  enabled: boolean,
): Promise<LiveState> {
  return invoke<LiveState>("set_plugin_enabled", { name, enabled });
}

export async function setPaused(paused: boolean): Promise<LiveState> {
  return invoke<LiveState>("set_paused", { paused });
}

export async function setPollInterval(intervalMs: number): Promise<LiveState> {
  return invoke<LiveState>("set_poll_interval", { intervalMs });
}

export async function minimizeWindow(): Promise<void> {
  return invoke<void>("minimize_window");
}

export async function toggleMaximizeWindow(): Promise<boolean> {
  return invoke<boolean>("toggle_maximize_window");
}

export async function isWindowMaximized(): Promise<boolean> {
  return invoke<boolean>("is_window_maximized");
}

export async function dragWindow(): Promise<void> {
  return invoke<void>("drag_window");
}

export async function closeWindow(): Promise<void> {
  return invoke<void>("close_window");
}

export async function reconnectDiscord(): Promise<LiveState> {
  return invoke<LiveState>("reconnect_discord");
}

export async function quitApp(): Promise<void> {
  return invoke<void>("quit_app");
}

export async function setPinnedSource(source: string | null): Promise<LiveState> {
  return invoke<LiveState>("set_pinned_source", { source });
}

export async function getAutostartStatus(): Promise<boolean> {
  return invoke<boolean>("get_autostart_status");
}

export async function setAutostart(enabled: boolean): Promise<boolean> {
  return invoke<boolean>("set_autostart", { enabled });
}

export async function registerCustomShortcut(
  action: string,
  oldShortcut: string | null,
  newShortcut: string,
): Promise<void> {
  return invoke<void>("register_custom_shortcut", {
    action,
    oldShortcut,
    newShortcut,
  });
}

export async function getRunningApplications(): Promise<RunningProcessView[]> {
  return invoke<RunningProcessView[]>("get_running_applications");
}

export async function addCustomApp(app: CustomAppConfig): Promise<LiveState> {
  return invoke<LiveState>("add_custom_app", { app });
}

export async function updateCustomApp(app: CustomAppConfig): Promise<LiveState> {
  return invoke<LiveState>("update_custom_app", { app });
}

export async function removeCustomApp(id: string): Promise<LiveState> {
  return invoke<LiveState>("remove_custom_app", { id });
}

export async function setStreamerMode(enabled: boolean): Promise<LiveState> {
  return invoke<LiveState>("set_streamer_mode", { enabled });
}

export async function inspectDiscordApp(appId: string): Promise<DiscordAppDetails> {
  return invoke<DiscordAppDetails>("inspect_discord_app", { appId });
}
