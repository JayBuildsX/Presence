import { invoke } from "@tauri-apps/api/core";
import type { LiveState } from "./types";

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



