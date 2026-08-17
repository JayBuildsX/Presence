/**
 * PresenceHub Plugin SDK
 *
 * @packageDocumentation
 *
 * This package provides the TypeScript types and interfaces for
 * developing PresenceHub plugins. It mirrors the Rust Core concepts
 * to provide API consistency across languages.
 *
 * ## Usage
 *
 * ```typescript
 * import type { Plugin, PluginMetadata, Activity } from "@presencehub/sdk";
 *
 * class MyPlugin implements Plugin {
 *   readonly metadata: PluginMetadata = {
 *     name: "My Plugin",
 *     version: "1.0.0",
 *   };
 *
 *   init(): void {
 *     // Prepare plugin resources
 *   }
 *
 *   shutdown(): void {
 *     // Release plugin resources
 *   }
 * }
 * ```
 */

// ---------------------------------------------------------------------------
// Activity Types
// ---------------------------------------------------------------------------

/**
 * Temporal bounds for an {@link Activity}.
 *
 * Both fields are optional because an activity may have only a start
 * time (e.g. "started editing 5 minutes ago") or no temporal bounds
 * at all (e.g. "idle").
 *
 * @remarks
 * Mirrors the Rust `ActivityTimestamps` struct in `presencehub-core`.
 */
export interface ActivityTimestamps {
  /** Unix timestamp in seconds when the activity started. */
  start?: number;
  /** Unix timestamp in seconds when the activity is expected to end. */
  end?: number;
}

/**
 * The canonical representation of a user's current activity.
 *
 * This is the shared data contract between all plugins and outputs.
 * Every plugin produces {@link Activity} values. Every output consumes them.
 *
 * @remarks
 * Mirrors the Rust `Activity` struct in `presencehub-core::activity`.
 *
 * The model is intentionally application-agnostic. It knows nothing
 * about FL Studio, VS Code, Discord, or any other application.
 * Plugins express application-specific information through the `state`
 * field and the `metadata` map.
 */
export interface Activity {
  /**
   * What the user is currently doing.
   *
   * Examples: "Playing", "Editing", "Idle", "Presenting", "Listening".
   *
   * This is the core semantic of the activity. It is always required
   * because an activity without a state is meaningless.
   */
  state: string;

  /**
   * Optional human-readable description of the activity.
   *
   * Provides additional context beyond the state. For example, a
   * "Playing" state might have details like "Level 5 - Forest Zone".
   *
   * This is optional because not all activities need description.
   * An "Idle" state, for instance, is self-evident.
   */
  details?: string;

  /**
   * Optional temporal bounds for the activity.
   *
   * When present, outputs can display elapsed or remaining time.
   * This is optional because not all activities have a defined
   * duration (e.g. "Idle" has no meaningful start time).
   */
  timestamps?: ActivityTimestamps;

  /**
   * Extensible key-value metadata.
   *
   * Plugins use this to attach application-specific data without
   * modifying the Activity model. The keys and values are
   * application-defined strings.
   *
   * Defaults to an empty object.
   */
  metadata: Record<string, string>;
}

// ---------------------------------------------------------------------------
// Plugin Types
// ---------------------------------------------------------------------------

/**
 * Lightweight metadata describing a plugin.
 *
 * @remarks
 * Mirrors the Rust `PluginMetadata` struct in `presencehub-plugin-host`.
 *
 * Only fields with a real consumer today are included:
 * - `name` — Used for identification, logging, and error reporting.
 * - `version` — Used for future compatibility checks.
 */
export interface PluginMetadata {
  /** Human-readable plugin name (e.g. "FL Studio", "VS Code"). */
  name: string;

  /** Plugin version string (e.g. "1.0.0"). */
  version: string;
}

/**
 * A single application integration.
 *
 * Each plugin represents one application that PresenceHub can monitor.
 * Examples include FL Studio, VS Code, and Spotify.
 *
 * @remarks
 * Mirrors the Rust `Plugin` trait in `presencehub-plugin-host`.
 *
 * The interface is intentionally minimal. Every method has a clear
 * justification:
 *
 * - `metadata` — Provides identification. Without it, the host cannot
 *   distinguish one plugin from another.
 *
 * - `init()` — Prepares the plugin for operation. A plugin must be
 *   able to initialize itself when the runtime starts.
 *
 * - `shutdown()` — Cleans up plugin resources. A plugin must be able
 *   to release resources when the runtime stops.
 *
 * @example
 * ```typescript
 * class MyPlugin implements Plugin {
 *   readonly metadata: PluginMetadata = {
 *     name: "My Plugin",
 *     version: "1.0.0",
 *   };
 *
 *   init(): void {
 *     console.log("Plugin initialized");
 *   }
 *
 *   shutdown(): void {
 *     console.log("Plugin shut down");
 *   }
 * }
 * ```
 */
export interface Plugin {
  /**
   * The plugin's metadata.
   *
   * Metadata is immutable after construction. Making it `readonly`
   * prevents accidental modification after initialization.
   */
  readonly metadata: PluginMetadata;

  /**
   * Initializes the plugin.
   *
   * Called when the runtime starts. The plugin should prepare
   * any resources needed for operation.
   */
  init(): void;

  /**
   * Shuts down the plugin.
   *
   * Called when the runtime stops. The plugin should release
   * any resources acquired during initialization or operation.
   */
  shutdown(): void;
}