import type { LiveState, PluginView } from "../types";
import { PluginIcon, StatusDot, pluginStatusText } from "./widgets";

interface PluginListProps {
  state: LiveState;
  onToggle: (name: string, enabled: boolean) => void;
  onTogglePriority?: (name: string) => void;
}

function statusDot(plugin: PluginView): string {
  if (!plugin.enabled) {
    return "dot-disabled";
  }
  return plugin.active ? "dot-live" : "dot-idle";
}

export default function PluginList({
  state,
  onToggle,
  onTogglePriority,
}: PluginListProps) {
  return (
    <section className="plugins-section" aria-label="Plugins">
      <div className="section-header">
        <h2 className="section-label">Supported Applications</h2>
        <span className="section-count">{state.plugins.filter((p) => p.enabled).length} Enabled</span>
      </div>
      <div className="plugin-list">
        {state.plugins.map((plugin) => {
          const isOwner = state.owner === plugin.name && plugin.active && !state.paused;
          const isPrioritized = state.pinned_source === plugin.name;
          const cardClass = [
            "plugin-card",
            isOwner ? "is-owner" : "",
            isPrioritized ? "is-prioritized-card" : "",
            !plugin.enabled ? "is-disabled" : "",
            plugin.enabled && plugin.active ? "is-active" : "",
          ]
            .filter(Boolean)
            .join(" ");

          return (
            <div className={cardClass} key={plugin.name}>
              <div className="plugin-left">
                <PluginIcon name={plugin.name} />
                <div className="plugin-info">
                  <div className="plugin-title-row">
                    <span className="plugin-name">{plugin.name}</span>
                    {isPrioritized && <span className="priority-badge">★ Priority</span>}
                    {isOwner && <span className="owner-badge">Broadcasting</span>}
                  </div>
                  <div className="plugin-status-row">
                    <StatusDot className={statusDot(plugin)} />
                    <span className="plugin-status">{pluginStatusText(plugin)}</span>
                  </div>
                </div>
              </div>
              <div className="plugin-right">
                {plugin.enabled && onTogglePriority && (
                  <button
                    type="button"
                    className={`priority-btn ${isPrioritized ? "is-prioritized" : ""}`}
                    onClick={() => onTogglePriority(plugin.name)}
                    title={
                      isPrioritized
                        ? "Prioritized application (Click to remove priority)"
                        : `Set ${plugin.name} as priority application`
                    }
                    aria-label={`Prioritize ${plugin.name}`}
                  >
                    <svg
                      width="13"
                      height="13"
                      viewBox="0 0 24 24"
                      fill={isPrioritized ? "currentColor" : "none"}
                      stroke="currentColor"
                      strokeWidth="2"
                      strokeLinecap="round"
                      strokeLinejoin="round"
                    >
                      <polygon points="12 2 15.09 8.26 22 9.27 17 14.14 18.18 21.02 12 17.77 5.82 21.02 7 14.14 2 9.27 8.91 8.26 12 2" />
                    </svg>
                  </button>
                )}
                <button
                  type="button"
                  role="switch"
                  aria-checked={plugin.enabled}
                  aria-label={`${plugin.enabled ? "Disable" : "Enable"} ${plugin.name}`}
                  className="toggle-switch"
                  onClick={() => onToggle(plugin.name, !plugin.enabled)}
                >
                  <span className="toggle-slider" />
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </section>
  );
}

