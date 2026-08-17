# PresenceHub Plugin System

> **Version:** Draft 1.0
> **Status:** Living Document

---

# Purpose

This document defines the plugin architecture used by PresenceHub.

Plugins are the primary extension mechanism of the platform.

Every application integration—from FL Studio to VS Code—is implemented as a plugin.

The Core never contains application-specific logic.

---

# Design Philosophy

PresenceHub follows one simple rule:

> **Everything application-specific belongs inside a plugin.**

Plugins are responsible for understanding an application.

The Core is responsible for coordinating plugins.

This separation keeps the architecture clean, maintainable, and scalable.

---

# What Is a Plugin?

A plugin is an independent module that observes a single application and translates its state into a standardized PresenceHub Activity.

For example:

```text
FL Studio
        │
        ▼
FL Studio Plugin
        │
        ▼
Standard Activity
```

The Core never sees FL Studio's internal data.

It only receives a standardized activity object.

---

# Plugin Responsibilities

Every plugin is responsible for:

* Detecting whether its application is running.
* Gathering application-specific information.
* Converting that information into a PresenceHub Activity.
* Updating the Presence Engine whenever the activity changes.
* Cleaning up its own resources when stopped.

Plugins should not communicate directly with outputs.

Plugins should not modify other plugins.

Plugins should remain completely independent.

---

# Plugin Lifecycle

Every plugin follows the same lifecycle.

```text
Discovered

↓

Loaded

↓

Initialized

↓

Running

↓

Stopped

↓

Unloaded
```

Each stage has a single purpose.

### Discovered

The Plugin Manager finds the plugin.

No code is executed.

---

### Loaded

The plugin is loaded into memory.

Configuration is validated.

---

### Initialized

Resources required by the plugin are prepared.

Examples:

* Opening file watchers
* Registering process listeners
* Initializing timers

---

### Running

The plugin actively monitors its target application.

Whenever the application's state changes, the plugin emits a new activity.

---

### Stopped

Monitoring ends.

Resources are released.

---

### Unloaded

The plugin is removed from memory.

---

# Plugin Isolation

Plugins are isolated from one another.

A failure inside one plugin must never affect:

* The Core
* The UI
* Other plugins
* Outputs

If a plugin encounters an unrecoverable error:

```text
Plugin

↓

Error

↓

Plugin Disabled

↓

Logged

↓

Core Continues Running
```

Fault isolation is a core design principle.

---

# Activity Generation

Plugins never communicate directly with Discord or any other output.

Instead, plugins produce standardized activities.

```text
Application

↓

Plugin

↓

Activity

↓

Presence Engine

↓

Outputs
```

This allows the same activity to be published through multiple outputs simultaneously.

---

# Plugin Interface

Every plugin should expose a common lifecycle interface.

Conceptually, every plugin provides the following capabilities:

* Metadata
* Initialization
* Start
* Stop
* Cleanup

The exact implementation is defined by the SDK.

This document intentionally focuses on architectural behavior rather than implementation details.

---

# Plugin Metadata

Every plugin should provide basic information describing itself.

Typical metadata includes:

* Plugin name
* Plugin ID
* Version
* Author
* Description
* Compatible SDK version

This information allows the Plugin Manager to validate compatibility before loading the plugin.

---

# Configuration

Plugins manage their own configuration.

The Core is responsible for providing configuration access, but never interprets plugin-specific settings.

Examples:

FL Studio Plugin

* Enable project name
* Enable BPM
* Show playback state

VS Code Plugin

* Show workspace
* Show language
* Show Git branch

Each plugin defines only the settings it requires.

---

# Performance Expectations

Plugins are expected to remain lightweight.

General engineering targets include:

* Fast initialization
* Minimal idle CPU usage
* Efficient polling or event handling
* No unnecessary allocations
* Avoid duplicate activity updates

Plugins should only emit activities when meaningful changes occur.

Repeatedly publishing identical activities should be avoided.

---

# Design Principles

## One Plugin, One Application

Each plugin should focus on a single application.

Avoid combining multiple unrelated integrations into one plugin.

---

## Independent Development

Plugins should be developed independently.

Adding or updating one plugin should not require changes to another.

---

## Minimal Core Knowledge

The Core should never know:

* FL Studio projects
* Spotify tracks
* VS Code workspaces
* Blender scenes

The Core only understands activities.

---

## Stable Contracts

Plugins communicate with the Core exclusively through the SDK.

The SDK acts as the contract between plugins and the runtime.

As long as that contract remains stable, plugins remain compatible.

---

# Future Capabilities

The plugin system is intentionally designed to support future enhancements such as:

* Hot plugin loading
* Plugin marketplace
* Plugin updates
* Plugin permissions
* Plugin signing
* Sandboxing

These features are outside the scope of the initial release but should remain compatible with the overall architecture.

---

# Summary

PresenceHub is built around a simple principle:

> Plugins understand applications. The Core understands plugins.

By enforcing this separation, PresenceHub remains lightweight, extensible, and capable of supporting new applications without requiring architectural changes.
