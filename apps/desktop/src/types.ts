// Backend view model (mirrors LiveState in src-tauri/src/state.rs).
// The frontend renders this verbatim and never reconstructs backend state.

export interface PluginView {
  name: string;
  enabled: boolean;
  active: boolean;
  summary: string | null;
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
  owner: string | null;
  current: PresenceView | null;
  plugins: PluginView[];
}
