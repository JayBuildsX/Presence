// Backend view model (mirrors LiveState in src-tauri/src/state.rs).
// The frontend renders this verbatim and never reconstructs backend state.

export interface CustomAppConfig {
  id: string;
  name: string;
  process_name: string;
  state: string;
  details: string | null;
  discord_app_id: string;
  logo_asset?: string | null;
  enabled: boolean;
}

export interface DiscordAppDetails {
  id: string;
  name: string;
  logo_asset: string | null;
}

export interface RunningProcessView {
  name: string;
  process_name: string;
  window_title: string;
}

export interface PluginView {
  name: string;
  enabled: boolean;
  active: boolean;
  summary: string | null;
  is_custom: boolean;
}

export interface PresenceView {
  source: string;
  state: string;
  details: string | null;
}

export interface LiveState {
  paused: boolean;
  poll_interval_ms: number;
  discord_connected: boolean;
  discord_error: string | null;
  owner: string | null;
  current: PresenceView | null;
  pinned_source: string | null;
  streamer_mode: boolean;
  custom_apps: CustomAppConfig[];
  plugins: PluginView[];
}

