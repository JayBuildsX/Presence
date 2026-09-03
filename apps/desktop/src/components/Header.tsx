import type { LiveState } from "../types";
import { StatusDot, globalStatus } from "./widgets";
import { dragWindow, minimizeWindow, closeWindow } from "../api";

interface HeaderProps {
  state: LiveState;
  busy: boolean;
  showSettings: boolean;
  onTogglePause: () => void;
  onToggleSettings: () => void;
}

export default function Header({
  state,
  busy,
  showSettings,
  onTogglePause,
  onToggleSettings,
}: HeaderProps) {
  const status = globalStatus(state);

  const handleMouseDown = (e: React.MouseEvent) => {
    // Only drag on left click and when not clicking a button or interactive child
    if (e.button === 0 && (e.target as HTMLElement).closest("button") === null) {
      void dragWindow();
    }
  };

  const handleMinimize = () => {
    void minimizeWindow();
  };

  const handleClose = () => {
    void closeWindow();
  };

  return (
    <header className="header" data-tauri-drag-region onMouseDown={handleMouseDown}>
      <div className="brand">
        <div className="brand-mark" aria-hidden="true">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <polygon points="13 2 3 14 12 14 11 22 21 10 12 10 13 2" />
          </svg>
        </div>
        <div className="brand-text">
          <span className="brand-title">PresenceHub</span>
          <span className="brand-subtitle">Discord Activity</span>
        </div>
      </div>

      <div className="header-actions">
        <div className={`status-badge ${status.dot}-badge`}>
          <StatusDot className={status.dot} />
          <span className="status-label">{status.label}</span>
        </div>

        <button
          type="button"
          className={`action-button ${state.paused ? "is-paused" : ""}`}
          disabled={busy}
          onClick={onTogglePause}
          aria-pressed={state.paused}
          title={state.paused ? "Resume Activity" : "Pause Activity"}
        >
          {state.paused ? (
            <>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
                <polygon points="5 3 19 12 5 21 5 3" />
              </svg>
              <span>Resume</span>
            </>
          ) : (
            <>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor">
                <rect x="6" y="4" width="4" height="16" />
                <rect x="14" y="4" width="4" height="16" />
              </svg>
              <span>Pause</span>
            </>
          )}
        </button>

        <button
          type="button"
          className={`icon-button ${showSettings ? "is-active" : ""}`}
          onClick={onToggleSettings}
          title="Settings & Polling Rate"
          aria-label="Toggle Settings"
        >
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z" />
          </svg>
        </button>

        <div className="window-controls">
          <button
            type="button"
            className="win-btn win-min"
            onClick={handleMinimize}
            title="Minimize"
            aria-label="Minimize"
          >
            <svg width="10" height="1" viewBox="0 0 10 1" fill="currentColor">
              <rect width="10" height="1" />
            </svg>
          </button>
          <button
            type="button"
            className="win-btn win-close"
            onClick={handleClose}
            title="Close"
            aria-label="Close"
          >
            <svg width="9" height="9" viewBox="0 0 12 12" fill="none" stroke="currentColor" strokeWidth="1.8">
              <line x1="1" y1="1" x2="11" y2="11" />
              <line x1="11" y1="1" x2="1" y2="11" />
            </svg>
          </button>
        </div>
      </div>
    </header>
  );
}


