import { useEffect, useState, useCallback } from "react";
import type { LiveState } from "../types";
import { getAutostartStatus, setAutostart, registerCustomShortcut } from "../api";

interface SettingsProps {
  state: LiveState;
  pending: string | null;
  onSelectInterval: (ms: number) => void;
  onToast?: (text: string, type?: "info" | "success" | "warning") => void;
}

const INTERVAL_PRESETS = [
  { ms: 500, label: "500 ms", desc: "High Responsiveness" },
  { ms: 1000, label: "1.0 s", desc: "Recommended" },
  { ms: 2000, label: "2.0 s", desc: "Balanced" },
  { ms: 5000, label: "5.0 s", desc: "Power Saver" },
];

interface ShortcutConfig {
  id: "pause" | "reconnect" | "toggle_window" | "streamer_mode";
  action: string;
  desc: string;
  keys: string[];
  defaultKeys: string[];
}

const DEFAULT_SHORTCUTS: ShortcutConfig[] = [
  {
    id: "pause",
    action: "Toggle Pause / Resume",
    desc: "Instantly pauses or resumes Discord Rich Presence",
    keys: ["Ctrl", "Shift", "P"],
    defaultKeys: ["Ctrl", "Shift", "P"],
  },
  {
    id: "reconnect",
    action: "Reconnect Discord",
    desc: "Probes and re-establishes Discord IPC connection",
    keys: ["Ctrl", "Shift", "D"],
    defaultKeys: ["Ctrl", "Shift", "D"],
  },
  {
    id: "toggle_window",
    action: "Show / Hide Window",
    desc: "Toggles Presence between screen and system tray",
    keys: ["Ctrl", "Shift", "H"],
    defaultKeys: ["Ctrl", "Shift", "H"],
  },
  {
    id: "streamer_mode",
    action: "Toggle Streamer Mode",
    desc: "Masks private project names and file paths",
    keys: ["Ctrl", "Shift", "S"],
    defaultKeys: ["Ctrl", "Shift", "S"],
  },
];

function toTauriFormat(keys: string[]): string {
  return keys
    .map((k) => (k === "Ctrl" ? "CommandOrControl" : k))
    .join("+");
}

export default function Settings({
  state,
  pending,
  onSelectInterval,
  onToast,
}: SettingsProps) {
  const currentMs = state.poll_interval_ms;
  const [activeTab, setActiveTab] = useState<"general" | "shortcuts">("general");
  const [autostart, setAutostartState] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);

  const [shortcuts, setShortcuts] = useState<ShortcutConfig[]>(() => {
    try {
      const saved = localStorage.getItem("presencehub_shortcuts");
      if (saved) {
        const parsed = JSON.parse(saved) as Record<string, string[]>;
        return DEFAULT_SHORTCUTS.map((item) => ({
          ...item,
          keys: parsed[item.id] ?? item.defaultKeys,
        }));
      }
    } catch {
      // Ignored
    }
    return DEFAULT_SHORTCUTS;
  });

  const [recordingId, setRecordingId] = useState<string | null>(null);

  useEffect(() => {
    void getAutostartStatus().then(setAutostartState).catch(() => {});
  }, []);

  const handleToggleAutostart = async () => {
    setAutostartBusy(true);
    try {
      const next = await setAutostart(!autostart);
      setAutostartState(next);
      onToast?.(
        next ? "Launch with Windows enabled" : "Launch with Windows disabled",
        "success",
      );
    } catch {
      onToast?.("Failed to update startup configuration", "warning");
    } finally {
      setAutostartBusy(false);
    }
  };

  const handleKeyDown = useCallback(
    async (e: KeyboardEvent) => {
      if (!recordingId) return;
      e.preventDefault();
      e.stopPropagation();

      if (e.key === "Escape") {
        setRecordingId(null);
        return;
      }

      // Ignore bare modifiers
      if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) {
        return;
      }

      const modifiers: string[] = [];
      if (e.ctrlKey) modifiers.push("Ctrl");
      if (e.altKey) modifiers.push("Alt");
      if (e.shiftKey) modifiers.push("Shift");

      const keyName =
        e.key.length === 1 ? e.key.toUpperCase() : e.key;

      if (modifiers.length === 0 && !keyName.startsWith("F")) {
        onToast?.("Include at least one modifier key (Ctrl / Alt / Shift)", "warning");
        return;
      }

      const newKeys = [...modifiers, keyName];
      const target = shortcuts.find((s) => s.id === recordingId);
      if (!target) return;

      const oldTauri = toTauriFormat(target.keys);
      const newTauri = toTauriFormat(newKeys);

      try {
        await registerCustomShortcut(target.id, oldTauri, newTauri);
        const next = shortcuts.map((s) =>
          s.id === recordingId ? { ...s, keys: newKeys } : s,
        );
        setShortcuts(next);
        const mapToSave = Object.fromEntries(next.map((s) => [s.id, s.keys]));
        localStorage.setItem("presencehub_shortcuts", JSON.stringify(mapToSave));
        onToast?.(`Shortcut updated to ${newKeys.join(" + ")}`, "success");
      } catch (err) {
        onToast?.(`Failed to bind shortcut: ${String(err)}`, "warning");
      } finally {
        setRecordingId(null);
      }
    },
    [recordingId, shortcuts, onToast],
  );

  useEffect(() => {
    if (!recordingId) return;
    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [recordingId, handleKeyDown]);

  const handleResetDefaults = async () => {
    for (const sc of shortcuts) {
      const currentTauri = toTauriFormat(sc.keys);
      const defaultTauri = toTauriFormat(sc.defaultKeys);
      if (currentTauri !== defaultTauri) {
        try {
          await registerCustomShortcut(sc.id, currentTauri, defaultTauri);
        } catch {
          // Ignored
        }
      }
    }
    const reset = shortcuts.map((s) => ({ ...s, keys: s.defaultKeys }));
    setShortcuts(reset);
    localStorage.removeItem("presencehub_shortcuts");
    setRecordingId(null);
    onToast?.("Shortcuts reset to defaults", "info");
  };

  return (
    <section className="settings-section" aria-label="Settings">
      <div className="section-header">
        <div className="settings-tabs">
          <button
            type="button"
            className={`settings-tab-btn ${activeTab === "general" ? "is-active" : ""}`}
            onClick={() => {
              setRecordingId(null);
              setActiveTab("general");
            }}
          >
            Preferences
          </button>
          <button
            type="button"
            className={`settings-tab-btn ${activeTab === "shortcuts" ? "is-active" : ""}`}
            onClick={() => setActiveTab("shortcuts")}
          >
            Global Shortcuts
          </button>
        </div>
        {activeTab === "general" && (
          <span className="section-count">{currentMs} ms cycle</span>
        )}
      </div>

      {activeTab === "general" ? (
        <div className="settings-card">
          <div className="settings-row">
            <div className="settings-info">
              <span className="settings-title">Polling Frequency</span>
              <span className="settings-desc">
                How often Presence checks active apps and updates Discord
              </span>
            </div>
          </div>

          <div className="interval-presets">
            {INTERVAL_PRESETS.map((preset) => {
              const isSelected = currentMs === preset.ms;
              const busy = pending === "interval";
              return (
                <button
                  key={preset.ms}
                  type="button"
                  className={`preset-btn ${isSelected ? "is-selected" : ""}`}
                  disabled={busy}
                  onClick={() => {
                    onSelectInterval(preset.ms);
                    onToast?.(`Detection rate set to ${preset.label}`, "info");
                  }}
                >
                  <span className="preset-label">{preset.label}</span>
                  <span className="preset-sub">{preset.desc}</span>
                </button>
              );
            })}
          </div>

          <div className="settings-divider" />

          <div className="settings-row">
            <div className="settings-info">
              <span className="settings-title">Launch with Windows</span>
              <span className="settings-desc">
                Start silently minimized in the system tray when logging in
              </span>
            </div>
            <button
              type="button"
              role="switch"
              aria-checked={autostart}
              aria-label="Toggle Launch with Windows"
              className="toggle-switch"
              disabled={autostartBusy}
              onClick={handleToggleAutostart}
            >
              <span className="toggle-slider" />
            </button>
          </div>
        </div>
      ) : (
        <div className="settings-card">
          <div className="shortcuts-list">
            {shortcuts.map((sc) => {
              const isRecording = recordingId === sc.id;
              return (
                <div className="shortcut-row" key={sc.id}>
                  <div className="shortcut-info">
                    <span className="shortcut-title">{sc.action}</span>
                    <span className="shortcut-desc">
                      {isRecording
                        ? "Press desired key combination (or Esc to cancel)..."
                        : sc.desc}
                    </span>
                  </div>
                  <div className="shortcut-actions">
                    <button
                      type="button"
                      className={`shortcut-keys-btn ${isRecording ? "is-recording" : ""}`}
                      onClick={() =>
                        setRecordingId(isRecording ? null : sc.id)
                      }
                      title="Click to change shortcut"
                    >
                      {isRecording ? (
                        <span className="recording-text">Press keys…</span>
                      ) : (
                        <div className="shortcut-keys">
                          {sc.keys.map((k, idx) => (
                            <span key={k} className="key-wrap">
                              <kbd className="key-badge">{k}</kbd>
                              {idx < sc.keys.length - 1 && (
                                <span className="key-plus">+</span>
                              )}
                            </span>
                          ))}
                        </div>
                      )}
                    </button>
                  </div>
                </div>
              );
            })}
          </div>
          <div className="settings-divider" />
          <div className="shortcuts-footer">
            <span className="shortcuts-tip">
              Click any hotkey to record a new key combination.
            </span>
            <button
              type="button"
              className="shortcut-reset-btn"
              onClick={handleResetDefaults}
            >
              Reset Defaults
            </button>
          </div>
        </div>
      )}
    </section>
  );
}

