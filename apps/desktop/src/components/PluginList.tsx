import type { CustomAppConfig, LiveState, PluginView } from "../types";
import { PluginIcon, StatusDot, pluginStatusText } from "./widgets";

interface PluginListProps {
  state: LiveState;
  onToggle: (name: string, enabled: boolean) => void;
  onTogglePriority?: (name: string) => void;
  onAddCustomApp: () => void;
  onEditCustomApp: (app: CustomAppConfig) => void;
  onDeleteCustomApp: (id: string, name: string) => void;
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
  onAddCustomApp,
  onEditCustomApp,
  onDeleteCustomApp,
}: PluginListProps) {
  return (
    <section className="plugins-section" aria-label="Plugins">
      <div className="section-header">
        <div className="section-header-left">
          <h2 className="section-label">Supported Applications</h2>
          <span className="section-count">{state.plugins.filter((p) => p.enabled).length} Enabled</span>
        </div>
        <button
          type="button"
          className="add-app-button"
          onClick={onAddCustomApp}
          title="Add Custom Application"
        >
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5">
            <line x1="12" y1="5" x2="12" y2="19" />
            <line x1="5" y1="12" x2="19" y2="12" />
          </svg>
          <span>Add App</span>
        </button>
      </div>
      <div className="plugin-list">
        {state.plugins.map((plugin) => {
          const isOwner = state.owner === plugin.name && plugin.active && !state.paused;
          const isPrioritized = state.pinned_source === plugin.name;
          const customConfig = plugin.is_custom
            ? state.custom_apps?.find((c) => c.name === plugin.name)
            : undefined;

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
                    {plugin.is_custom && <span className="custom-app-badge">Custom</span>}
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
                {plugin.is_custom && customConfig && (
                  <div className="custom-card-actions">
                    <button
                      type="button"
                      className="card-action-btn"
                      onClick={() => onEditCustomApp(customConfig)}
                      title={`Edit ${plugin.name}`}
                      aria-label={`Edit ${plugin.name}`}
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
                      </svg>
                    </button>
                    <button
                      type="button"
                      className="card-action-btn is-delete"
                      onClick={() => onDeleteCustomApp(customConfig.id, plugin.name)}
                      title={`Delete ${plugin.name}`}
                      aria-label={`Delete ${plugin.name}`}
                    >
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                        <polyline points="3 6 5 6 21 6" />
                        <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                      </svg>
                    </button>
                  </div>
                )}
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

