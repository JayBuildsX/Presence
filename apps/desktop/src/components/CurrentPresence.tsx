import type { LiveState } from "../types";
import { StatusDot } from "./widgets";

export default function CurrentPresence({ state }: { state: LiveState }) {
  return (
    <section aria-label="Current presence">
      <h2 className="section-label">Current presence</h2>
      <div className="presence">
        {state.current ? (
          <>
            <div className="presence-owner">
              <StatusDot className="dot-live" />
              <span>{state.current.source}</span>
            </div>
            <div className="presence-state">{state.current.state}</div>
            {state.current.details && (
              <div className="presence-details">{state.current.details}</div>
            )}
          </>
        ) : (
          <div className="presence-empty">
            {state.paused
              ? "Paused — resume to publish presence."
              : "Nothing to show. Start a watched application."}
          </div>
        )}
      </div>
    </section>
  );
}
