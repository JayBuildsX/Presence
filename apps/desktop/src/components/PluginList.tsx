import type { LiveState, PluginView } from "../types";
import { PluginIcon, StatusDot, pluginStatusText } from "./widgets";

interface PluginListProps {
  state: LiveState;
  pending: string | null;
  onToggle: (name: string, enabled: boolean) => void;
}

function statusDot(plugin: PluginView): string {
  if (!plugin.enabled) {
    return "dot-idle";
  }
  return plugin.active ? "dot-live" : "dot-idle";
}

export default function PluginList({ state, pending, onToggle }: PluginListProps) {
  return (
    <section aria-label="Plugins">
      <h2 className="section-label">Plugins</h2>
      <div className="plugin-list">
        {state.plugins.map((plugin) => {
          const isOwner = state.owner === plugin.name && plugin.active;
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
              <span className="plugin-icon">
                <PluginIcon name={plugin.name} />
              </span>
              <div className="plugin-info">
                <span className="plugin-name">
                  <StatusDot className={statusDot(plugin)} />
                  {plugin.name}
                </span>
                <span className="plugin-status">{pluginStatusText(plugin)}</span>
              </div>
              <button
                type="button"
                role="switch"
                aria-checked={plugin.enabled}
                aria-label={`${plugin.enabled ? "Disable" : "Enable"} ${plugin.name}`}
                className="toggle"
                disabled={busy}
                onClick={() => onToggle(plugin.name, !plugin.enabled)}
              />
            </div>
          );
        })}
      </div>
    </section>
  );
}
