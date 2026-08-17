# PresenceHub API Stability

> **Status:** Draft
> **Date:** 2026-07-31
> **Version:** v0.1.0

---

## Purpose

This document defines the public API surface of PresenceHub and the stability guarantees for each component.

The goal is to help downstream consumers (plugins, outputs, integrations) understand what is stable and what may change.

---

## Public API Surface

### `presencehub-core`

| Type / Module | Stability | Description |
|---|---|---|
| `presencehub_core::Activity` | **Stable** | Canonical activity model |
| `presencehub_core::ActivityTimestamps` | **Stable** | Temporal bounds for activities |
| `presencehub_core::Config` | **Stable** | Runtime configuration |
| `presencehub_core::ConfigError` | **Stable** | Configuration errors |
| `presencehub_core::ApplicationContext` | **Stable** | Application-scoped context |
| `presencehub_core::Core` | **Stable** | Core runtime |
| `presencehub_core::CoreError` | **Stable** | Core lifecycle errors |
| `presencehub_core::output::Output` | **Stable** | Output trait |
| `presencehub_core::output::OutputError` | **Stable** | Output errors |
| `presencehub_core::output::PresenceEngine` | **Stable** | Activity router |
| `presencehub_core::output::ConsoleOutput` | **Stable** | Development output |

**Not public (internal):**
- `presencehub_core::core` (module)
- `presencehub_core::config` (module)
- `presencehub_core::context` (module)
- `presencehub_core::activity` (module)
- `presencehub_core::output` (module)

Internal modules are not part of the public API even though they are accessible via `pub mod`. Only the re-exported types are stable.

### `presencehub-plugin-host`

| Type / Module | Stability | Description |
|---|---|---|
| `presencehub_plugin_host::Plugin` | **Stable** | Plugin trait |
| `presencehub_plugin_host::PluginMetadata` | **Stable** | Plugin metadata |
| `presencehub_plugin_host::PluginError` | **Stable** | Plugin errors |
| `presencehub_plugin_host::PluginHost` | **Stable** | Plugin lifecycle manager |

---

## Stability Definitions

- **Stable:** Will not break within the same major version (v0.x). Minor versions may add fields/methods but will not remove or rename existing ones.
- **Internal:** May change without notice. Do not depend on these directly.

---

## Versioning Policy

PresenceHub follows semantic versioning:

| Change Type | Version Bump | Examples |
|---|---|---|
| Bug fix | Patch (0.0.x) | Fix parsing edge case, fix test |
| New feature | Minor (0.x.0) | Add new output, add plugin |
| Breaking change | Major (x.0.0) | Remove trait method, rename type |

---

## Plugin Contract

Plugins must implement the `Plugin` trait:

```rust
pub trait Plugin: Send + Sync {
    fn metadata(&self) -> &PluginMetadata;
    fn init(&mut self) -> Result<(), PluginError>;
    fn shutdown(&mut self) -> Result<(), PluginError>;
}
```

This trait is stable. Adding new default methods is allowed without breaking existing plugins.

---

## Output Contract

Outputs must implement the `Output` trait:

```rust
pub trait Output: Send {
    fn publish(&mut self, activity: &Activity) -> Result<(), OutputError>;
}
```

This trait is stable. Adding new default methods is allowed without breaking existing outputs.

---

## Activity Contract

The `Activity` model is the primary data contract between plugins and outputs.

```rust
pub struct Activity {
    pub state: String,
    pub details: Option<String>,
    pub timestamps: Option<ActivityTimestamps>,
    pub metadata: HashMap<String, String>,
}
```

This struct is stable. New fields may be added with default values in future minor versions.

---

## Extension Points

| Extension Point | Mechanism | Stability |
|---|---|---|
| Application integration | `Plugin` trait | Stable |
| Output destination | `Output` trait | Stable |
| Activity metadata | `HashMap<String, String>` | Stable |

---

## Breaking Changes Policy

Breaking changes within v0.x will be minimized. When unavoidable, they will be documented in the release notes with migration instructions.

After v1.0.0, breaking changes will only occur in major versions.

---

## Deprecation Policy

Deprecated APIs will:
1. Emit a warning in the current minor version
2. Remain functional for at least one minor version
3. Be removed in the next major version

---