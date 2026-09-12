import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./App.css";
import {
  addCustomApp,
  getState,
  reconnectDiscord,
  removeCustomApp,
  setPaused,
  setPinnedSource,
  setPluginEnabled,
  setPollInterval,
  setStreamerMode,
  updateCustomApp,
} from "./api";
import type { CustomAppConfig, LiveState } from "./types";
import CurrentPresence from "./components/CurrentPresence";
import CustomAppModal from "./components/CustomAppModal";
import Header from "./components/Header";
import PluginList from "./components/PluginList";
import Settings from "./components/Settings";
import Toast, { type ToastMessage } from "./components/Toast";

export default function App() {
  const [state, setState] = useState<LiveState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [pending, setPending] = useState<string | null>(null);
  const [activeOperations, setActiveOperations] = useState<Set<string>>(new Set());
  const [showSettings, setShowSettings] = useState(false);
  const [toast, setToast] = useState<ToastMessage | null>(null);
  const [isCustomAppModalOpen, setIsCustomAppModalOpen] = useState(false);
  const [editingCustomApp, setEditingCustomApp] = useState<CustomAppConfig | null>(null);

  const handleDismissToast = useCallback(() => {
    setToast(null);
  }, []);

  const showToast = useCallback(
    (text: string, type: "info" | "success" | "warning" = "info") => {
      setToast({ id: Date.now(), text, type });
    },
    [],
  );

  useEffect(() => {
    const unlisten = listen<{ text: string; type?: "info" | "success" | "warning" }>(
      "toast",
      (event) => {
        showToast(event.payload.text, event.payload.type);
      },
    );
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [showToast]);

  const refreshState = useCallback(async () => {
    try {
      const next = await getState();
      setState(next);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const pollIntervalRef = useRef<number>(1000);
  useEffect(() => {
    if (state?.poll_interval_ms) {
      pollIntervalRef.current = state.poll_interval_ms;
    }
  }, [state?.poll_interval_ms]);

  useEffect(() => {
    void refreshState();
    let timer: ReturnType<typeof setTimeout>;
    const scheduleNext = () => {
      timer = setTimeout(() => {
        void refreshState().finally(scheduleNext);
      }, pollIntervalRef.current);
    };
    scheduleNext();
    return () => clearTimeout(timer);
  }, [refreshState]);

  const debouncedTimersRef = useRef<Map<string, ReturnType<typeof setTimeout>>>(
    new Map(),
  );

  const scheduleDebouncedAction = useCallback(
    (
      key: string,
      actionFn: () => Promise<LiveState | void>,
      delayMs = 180,
    ) => {
      setActiveOperations((prev) => new Set(prev).add(key));

      const existingTimer = debouncedTimersRef.current.get(key);
      if (existingTimer) {
        clearTimeout(existingTimer);
      }

      const timer = setTimeout(async () => {
        debouncedTimersRef.current.delete(key);
        try {
          const next = await actionFn();
          if (next) {
            setState(next);
          } else {
            await refreshState();
          }
          setError(null);
        } catch (e) {
          setError(e instanceof Error ? e.message : String(e));
        } finally {
          setActiveOperations((prev) => {
            const nextSet = new Set(prev);
            nextSet.delete(key);
            return nextSet;
          });
        }
      }, delayMs);

      debouncedTimersRef.current.set(key, timer);
    },
    [refreshState],
  );

  async function runCommand(
    name: string,
    action: () => Promise<LiveState | void>,
  ) {
    setPending(name);
    try {
      const next = await action();
      if (next) {
        setState(next);
      } else {
        await refreshState();
      }
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setPending(null);
    }
  }

  function handleTogglePlugin(name: string, enabled: boolean) {
    setState((prev) => {
      if (!prev) return null;
      return {
        ...prev,
        plugins: prev.plugins.map((p) =>
          p.name === name ? { ...p, enabled } : p,
        ),
      };
    });
    showToast(enabled ? `${name} enabled` : `${name} disabled`, "info");
    scheduleDebouncedAction(`plugin:${name}`, () =>
      setPluginEnabled(name, enabled),
    );
  }

  function handleTogglePause() {
    if (!state) return;
    const nextPause = !state.paused;
    setState((prev) => (prev ? { ...prev, paused: nextPause } : null));
    showToast(nextPause ? "Broadcasting paused" : "Broadcasting resumed", "info");
    scheduleDebouncedAction("pause", () => setPaused(nextPause));
  }

  function handleSelectInterval(intervalMs: number) {
    setState((prev) => (prev ? { ...prev, poll_interval_ms: intervalMs } : null));
    scheduleDebouncedAction("interval", () => setPollInterval(intervalMs), 50);
  }

  function handleReconnectDiscord() {
    setActiveOperations((prev) => new Set(prev).add("reconnect"));
    void runCommand("reconnect", async () => {
      const res = await reconnectDiscord();
      if (res.discord_connected) {
        showToast("Discord reconnected successfully", "success");
      } else {
        showToast("Could not connect to Discord pipe", "warning");
      }
      return res;
    }).finally(() => {
      setActiveOperations((prev) => {
        const nextSet = new Set(prev);
        nextSet.delete("reconnect");
        return nextSet;
      });
    });
  }

  function handleTogglePriority(name: string) {
    if (!state) return;
    const nextPriority = state.pinned_source === name ? null : name;
    setState((prev) => (prev ? { ...prev, pinned_source: nextPriority } : null));
    showToast(
      nextPriority ? `★ Prioritized ${name}` : `Priority removed from ${name}`,
      "success",
    );
    scheduleDebouncedAction("priority", () => setPinnedSource(nextPriority));
  }

  function handleToggleStreamerMode() {
    if (!state) return;
    const nextStreamer = !state.streamer_mode;
    setState((prev) => (prev ? { ...prev, streamer_mode: nextStreamer } : null));
    showToast(
      nextStreamer ? "🛡️ Streamer Mode enabled" : "Streamer Mode disabled",
      nextStreamer ? "success" : "info",
    );
    scheduleDebouncedAction("streamer_mode", () => setStreamerMode(nextStreamer));
  }

  function handleOpenAddCustomApp() {
    setEditingCustomApp(null);
    setIsCustomAppModalOpen(true);
  }

  function handleOpenEditCustomApp(app: CustomAppConfig) {
    setEditingCustomApp(app);
    setIsCustomAppModalOpen(true);
  }

  async function handleSaveCustomApp(app: CustomAppConfig) {
    if (editingCustomApp) {
      const next = await updateCustomApp(app);
      setState(next);
      showToast(`Updated custom app: ${app.name}`, "success");
    } else {
      const next = await addCustomApp(app);
      setState(next);
      showToast(`Added custom app: ${app.name}`, "success");
    }
  }

  async function handleDeleteCustomApp(id: string, name: string) {
    try {
      const next = await removeCustomApp(id);
      setState(next);
      showToast(`Removed custom app: ${name}`, "info");
    } catch {
      showToast(`Failed to remove ${name}`, "warning");
    }
  }

  const isSyncing = activeOperations.size > 0;

  return (
    <div className="app">
      <Header
        state={
          state ?? {
            paused: false,
            poll_interval_ms: 1000,
            discord_connected: false,
            discord_error: null,
            owner: null,
            current: null,
            pinned_source: null,
            streamer_mode: false,
            custom_apps: [],
            plugins: [],
          }
        }
        busy={state === null || pending !== null}
        showSettings={showSettings}
        onTogglePause={handleTogglePause}
        onToggleSettings={() => setShowSettings(!showSettings)}
        onToggleStreamerMode={handleToggleStreamerMode}
        onReconnectDiscord={handleReconnectDiscord}
        isSyncing={isSyncing}
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
          <PluginList
            state={state}
            onToggle={handleTogglePlugin}
            onTogglePriority={handleTogglePriority}
            onAddCustomApp={handleOpenAddCustomApp}
            onEditCustomApp={handleOpenEditCustomApp}
            onDeleteCustomApp={handleDeleteCustomApp}
          />
          {showSettings && (
            <Settings
              state={state}
              pending={pending}
              onSelectInterval={handleSelectInterval}
              onToast={showToast}
            />
          )}
        </>
      )}
      <Toast toast={toast} onDismiss={handleDismissToast} />
      <CustomAppModal
        isOpen={isCustomAppModalOpen}
        initialApp={editingCustomApp}
        onSave={handleSaveCustomApp}
        onClose={() => setIsCustomAppModalOpen(false)}
      />
    </div>
  );
}
