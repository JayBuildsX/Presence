import type { LiveState } from "../types";
import { StatusDot, globalStatus } from "./widgets";

interface HeaderProps {
  state: LiveState;
  busy: boolean;
  onTogglePause: () => void;
}

export default function Header({ state, busy, onTogglePause }: HeaderProps) {
  const status = globalStatus(state);
  return (
    <header className="header">
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
      </div>
    </header>
  );
}

