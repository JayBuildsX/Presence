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
        <span className="brand-mark" aria-hidden="true">
          P
        </span>
        PresenceHub
      </div>
      <div className="header-status">
        <StatusDot className={status.dot} />
        <span>{status.label}</span>
      </div>
      <button
        type="button"
        className="pause-button"
        disabled={busy}
        onClick={onTogglePause}
        aria-pressed={state.paused}
      >
        {state.paused ? "Resume" : "Pause"}
      </button>
    </header>
  );
}
