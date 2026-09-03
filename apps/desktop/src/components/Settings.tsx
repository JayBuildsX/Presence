import { useEffect, useState } from "react";
import type { LiveState } from "../types";
import { getAutostartStatus, setAutostart } from "../api";

interface SettingsProps {
  state: LiveState;
  pending: string | null;
  onSelectInterval: (ms: number) => void;
}

const INTERVAL_PRESETS = [
  { ms: 500, label: "500 ms", desc: "High Responsiveness" },
  { ms: 1000, label: "1.0 s", desc: "Recommended" },
  { ms: 2000, label: "2.0 s", desc: "Balanced" },
  { ms: 5000, label: "5.0 s", desc: "Power Saver" },
];

export default function Settings({ state, pending, onSelectInterval }: SettingsProps) {
  const currentMs = state.poll_interval_ms;
  const [autostart, setAutostartState] = useState(false);
  const [autostartBusy, setAutostartBusy] = useState(false);

  useEffect(() => {
    void getAutostartStatus().then(setAutostartState).catch(() => {});
  }, []);

  const handleToggleAutostart = async () => {
    setAutostartBusy(true);
    try {
      const next = await setAutostart(!autostart);
      setAutostartState(next);
    } catch {
      // Ignored
    } finally {
      setAutostartBusy(false);
    }
  };

  return (
    <section className="settings-section" aria-label="Settings">
      <div className="section-header">
        <h2 className="section-label">Preferences & System</h2>
        <span className="section-count">{currentMs} ms cycle</span>
      </div>

      <div className="settings-card">
        <div className="settings-row">
          <div className="settings-info">
            <span className="settings-title">Polling Frequency</span>
            <span className="settings-desc">
              How often PresenceHub checks active apps and updates Discord
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
                onClick={() => onSelectInterval(preset.ms)}
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
    </section>
  );
}
