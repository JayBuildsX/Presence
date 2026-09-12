import type { LiveState, PluginView } from "../types";

function StatusDot({ className }: { className: string }) {
  return <span className={`dot ${className}`} aria-hidden="true" />;
}

export function globalStatus(state: LiveState): {
  label: string;
  dot: string;
} {
  if (state.paused) {
    return { label: "Paused", dot: "dot-paused" };
  }
  if (!state.discord_connected) {
    return { label: "No Discord", dot: "dot-error" };
  }  if (state.current) {
    return { label: "Live", dot: "dot-live" };
  }
  return { label: "Watching", dot: "dot-idle" };
}

const APP_ICONS: Record<string, string> = {
  "FL Studio": "/flstudio.png",
  Antigravity: "/antigravity.png",
  OpenCode: "/opencode.png",
};

export function PluginIcon({ name }: { name: string }) {
  const iconSrc = APP_ICONS[name];

  if (iconSrc) {
    return (
      <span className={`plugin-brand-icon brand-${name.toLowerCase().replace(/\s+/g, "")}`}>
        <img
          src={iconSrc}
          alt={name}
          className="plugin-real-icon"
          loading="lazy"
        />
      </span>
    );
  }

  return (
    <span className={`plugin-brand-icon brand-${name.toLowerCase().replace(/\s+/g, "")}`}>
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
      >
        <circle cx="12" cy="12" r="6" />
      </svg>
    </span>
  );
}

export function pluginStatusText(plugin: PluginView): string {
  if (!plugin.enabled) {
    return "Disabled";
  }
  if (plugin.active) {
    return plugin.summary ?? "Active";
  }
  return "Not detected";
}

export { StatusDot };

