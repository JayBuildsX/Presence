import type { LiveState } from "../types";
import { PluginIcon } from "./widgets";

export default function CurrentPresence({ state }: { state: LiveState }) {
  const isLive = Boolean(state.current);

  return (
    <section className="presence-section" aria-label="Current presence">
      <div className="section-header">
        <h2 className="section-label">Active Presence</h2>
        {isLive && (
          <span className="live-indicator-tag">
            <span className="pulse-dot" /> Broadcasting to Discord
          </span>
        )}
      </div>
      <div className={`presence-card ${isLive ? "is-live" : "is-empty"}`}>
        {state.current ? (
          <div className="presence-live-layout">
            <div className="presence-icon-col">
              <PluginIcon name={state.current.source} />
            </div>
            <div className="presence-info-col">
              <div className="presence-source-row">
                <span className="presence-source-name">{state.current.source}</span>
                <span className="presence-source-badge">Active</span>
              </div>
              <div className="presence-state-text">{state.current.state}</div>
              {state.current.details && (
                <div className="presence-details-text">{state.current.details}</div>
              )}
            </div>
          </div>
        ) : (
          <div className="presence-empty-layout">
            <div className="empty-icon-wrap">
              <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
                <circle cx="12" cy="12" r="10" />
                <path d="M12 6v6l4 2" />
              </svg>
            </div>
            <div className="empty-text-wrap">
              <div className="empty-title">
                {state.paused ? "Presence is Paused" : "No Active Application Detected"}
              </div>
              <div className="empty-description">
                {state.paused
                  ? "Click Resume in the top right to start broadcasting your activity."
                  : "Launch FL Studio, Antigravity, or OpenCode to share your status."}
              </div>
            </div>
          </div>
        )}
      </div>
    </section>
  );
}


