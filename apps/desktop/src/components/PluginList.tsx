import type { LiveState, PluginView } from "../types";
import { PluginIcon, StatusDot, pluginStatusText } from "./widgets";

interface PluginListProps {
  state: LiveState;
  pending: string | null;
  onToggle: (name: string, enabled: boolean) => void;
}

function statusDot(plugin: PluginView): string {
  if (!plugin.enabled) {
    return "dot-disabled";
  }
  return plugin.active ? "dot-live" : "dot-idle";
}

export default function PluginList({ state, pending, onToggle }: PluginListProps) {
  return (
    <section className="plugins-section" aria-label="Plugins">
      <div className="section-header">
        <h2 className="section-label">Supported Applications</h2>
        <span className="section-count">{state.plugins.filter((p) => p.enabled).length} Enabled</span>
      </div>
      <div className="plugin-list">
        {state.plugins.map((plugin) => {
          const isOwner = state.owner === plugin.name && plugin.active && !state.paused;
          const cardClass = [
            "plugin-card",
            isOwner ? "is-owner" : "",
            !plugin.enabled ? "is-disabled" : "",
            plugin.enabled && plugin.active ? "is-active" : "",
          ]
            .filter(Boolean)
            .join(" ");
          const busy = pending === plugin.name;

          return (
            <div className={cardClass} key={plugin.name}>
              <div className="plugin-left">
                <PluginIcon name={plugin.name} />
                <div className="plugin-info">
                  <div className="plugin-title-row">
                    <span className="plugin-name">{plugin.name}</span>
                    {isOwner && <span className="owner-badge">Broadcasting</span>}
                  </div>
                  <div className="plugin-status-row">
                    <StatusDot className={statusDot(plugin)} />
                    <span className="plugin-status">{pluginStatusText(plugin)}</span>
                  </div>
                </div>
              </div>
              <div className="plugin-right">
                <button
                  type="button"
                  role="switch"
                  aria-checked={plugin.enabled}
                  aria-label={`${plugin.enabled ? "Disable" : "Enable"} ${plugin.name}`}
                  className="toggle-switch"
                  disabled={busy}
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

