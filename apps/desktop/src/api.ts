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
