import type { ReactNode } from "react";
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
    return { label: "Discord Disconnected", dot: "dot-error" };
  }
  if (state.current) {
    return { label: "Rich Presence Live", dot: "dot-live" };
  }
  return { label: "Watching Applications", dot: "dot-idle" };
}

export function PluginIcon({ name }: { name: string }) {
  const icons: Record<string, ReactNode> = {
    "FL Studio": (
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <path d="M9 18V5l12-2v13" />
        <circle cx="6" cy="18" r="3" />
        <circle cx="18" cy="16" r="3" />
      </svg>
    ),
    Antigravity: (
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <circle cx="12" cy="12" r="3" />
        <path d="M12 2v2" />
        <path d="M12 20v2" />
        <path d="m4.93 4.93 1.41 1.41" />
        <path d="m17.66 17.66 1.41 1.41" />
        <path d="M2 12h2" />
        <path d="M20 12h2" />
        <path d="m6.34 17.66-1.41 1.41" />
        <path d="m19.07 4.93-1.41 1.41" />
      </svg>
    ),
    OpenCode: (
      <svg
        width="20"
        height="20"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        strokeLinecap="round"
        strokeLinejoin="round"
      >
        <polyline points="16 18 22 12 16 6" />
        <polyline points="8 6 2 12 8 18" />
        <line x1="14" y1="4" x2="10" y2="20" />
      </svg>
    ),
  };

  return (
    <span className={`plugin-brand-icon brand-${name.toLowerCase().replace(/\s+/g, "")}`}>
      {icons[name] ?? (
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
      )}
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

