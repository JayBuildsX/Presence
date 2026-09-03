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
    return { label: "Disconnected", dot: "dot-error" };
  }
  if (state.current) {
    return { label: "Live", dot: "dot-live" };
  }
  return { label: "Idle", dot: "dot-idle" };
}

export function PluginIcon({ name }: { name: string }) {
  // Abstract monochrome glyphs (GUI-only; unrelated to Discord assets).
  const paths: Record<string, ReactNode> = {
    "FL Studio": (
      <>
        <circle cx="7" cy="17" r="3.2" />
        <circle cx="17" cy="15" r="3.2" />
        <path d="M10 17V6l10-2v11" />
      </>
    ),
    Antigravity: (
      <>
        <circle cx="12" cy="12" r="3" />
        <ellipse cx="12" cy="12" rx="10" ry="4.2" />
      </>
    ),
    OpenCode: (
      <>
        <path d="M8 7l-5 5 5 5" />
        <path d="M16 7l5 5-5 5" />
      </>
    ),
  };
  return (
    <svg
      width="18"
      height="18"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      {paths[name] ?? <circle cx="12" cy="12" r="6" />}
    </svg>
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
