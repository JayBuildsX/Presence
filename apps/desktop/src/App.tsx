import { useCallback, useEffect, useRef, useState } from "react";
import "./App.css";
import {
  getState,
  reconnectDiscord,
  setPaused,
  setPluginEnabled,
  setPollInterval,
} from "./api";
import type { LiveState } from "./types";
import CurrentPresence from "./components/CurrentPresence";
import Header from "./components/Header";
import PluginList from "./components/PluginList";
import Settings from "./components/Settings";

const REFRESH_INTERVAL_MS = 1000;

function App() {
  const [state, setState] = useState<LiveState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showSettings, setShowSettings] = useState(false);
  // Name of the plugin with an in-flight toggle, "pause", or "interval".
  const [pending, setPending] = useState<string | null>(null);
  const mounted = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await getState();
      if (mounted.current) {
        setState(next);
      }
    } catch (e) {
      if (mounted.current) {
        setError(`Could not reach backend: ${String(e)}`);
      }
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    void refresh();
    const timer = window.setInterval(() => {
      void refresh();
    }, REFRESH_INTERVAL_MS);
    return () => {
      mounted.current = false;
      window.clearInterval(timer);
    };
  }, [refresh]);

  async function runCommand<T>(key: string, command: () => Promise<T>) {
    setPending(key);
    setError(null);
    try {
      const next = (await command()) as unknown as LiveState;
      if (mounted.current) {
        setState(next);
      }
    } catch (e) {
      // Never leave the UI disagreeing with the backend: show the error
      // and reload the authoritative state.
      if (mounted.current) {
        setError(String(e));
      }
      await refresh();
    } finally {
      if (mounted.current) {
        setPending(null);
      }
    }
  }

  function handleTogglePlugin(name: string, enabled: boolean) {
    void runCommand(name, () => setPluginEnabled(name, enabled));
  }

  function handleTogglePause() {
    if (!state) {
      return;
    }
    void runCommand("pause", () => setPaused(!state.paused));
  }

  function handleSelectInterval(intervalMs: number) {
    void runCommand("interval", () => setPollInterval(intervalMs));
  }

  function handleReconnectDiscord() {
    void runCommand("reconnect", () => reconnectDiscord());
  }

  return (
    <div className="app">
      <Header
        state={
          state ?? {
            paused: false,
            poll_interval_ms: 1000,
            discord_connected: false,
            owner: null,
            current: null,
            plugins: [],
          }
        }
        busy={state === null || pending !== null}
        showSettings={showSettings}
        onTogglePause={handleTogglePause}
        onToggleSettings={() => setShowSettings(!showSettings)}
        onReconnectDiscord={handleReconnectDiscord}
      />
      {error && (
        <div className="error-bar" role="alert">
          {error}
        </div>
      )}
      {state === null ? (
        <div className="loading">Connecting to PresenceHub…</div>
      ) : (
        <>
          <CurrentPresence state={state} />
          <PluginList state={state} pending={pending} onToggle={handleTogglePlugin} />
          {showSettings && (
            <Settings
              state={state}
              pending={pending}
              onSelectInterval={handleSelectInterval}
            />
          )}
        </>
      )}
    </div>
  );
}

export default App;
