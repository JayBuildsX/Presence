# PresenceHub Architecture

> **Version:** Draft 1.0
> **Status:** Living Document

---

# Purpose

This document defines the high-level architecture of PresenceHub.

Rather than describing implementation details, it explains how the major components of the system interact, where responsibilities belong, and the architectural principles that guide every development decision.

The goal is to create a system that is lightweight, extensible, maintainable, and resilient as new plugins and outputs are introduced.

---

# Architectural Philosophy

PresenceHub follows a **layered, plugin-based architecture**.

The core is intentionally minimal. It acts as the coordinator of the application but contains no application-specific logic.

Every integration—whether FL Studio, VS Code, Spotify, or any future application—is implemented as a plugin.

The core never knows *what* an application is. It only knows that plugins produce standardized activities.

This separation allows the project to scale without increasing complexity inside the core.

---

# High-Level Architecture

```text
                   +-----------------------+
                   |     Desktop UI        |
                   |  (Tauri + React)      |
                   +-----------+-----------+
                               |
                               v
                   +-----------------------+
                   |         Core          |
                   +-----------+-----------+
                               |
              +----------------+----------------+
              |                                 |
              v                                 v
      +------------------+             +------------------+
      |  Plugin Manager  |             | Presence Engine  |
      +---------+--------+             +---------+--------+
                |                                |
      +---------+---------+                      |
      |                   |                      |
      v                   v                      v
+--------------+   +---------------+     +------------------+
| FL Studio    |   | VS Code       |     | Discord Output   |
| Plugin       |   | Plugin        |     |                  |
+--------------+   +---------------+     +------------------+
```

---

# Component Responsibilities

## Core

The Core is responsible for orchestrating the entire application.

It manages startup, shutdown, configuration loading, service initialization, and communication between internal components.

The Core **must never** contain logic related to individual applications.

### Responsibilities

* Application startup
* Service initialization
* Configuration management
* Dependency wiring
* Lifecycle coordination
* Graceful shutdown

### Non-Responsibilities

* Detecting applications
* Reading application data
* Communicating with Discord
* Managing plugin-specific logic

---

## Plugin Manager

The Plugin Manager is responsible for everything related to plugins.

It discovers plugins, validates them, manages their lifecycle, and isolates failures from the rest of the system.

It never interprets the information produced by plugins.

### Responsibilities

* Plugin discovery
* Loading plugins
* Starting plugins
* Stopping plugins
* Version compatibility checks
* Error isolation
* Plugin lifecycle management

### Design Decision

The Plugin Manager manages plugins—it does **not** process activities.

Keeping these responsibilities separate prevents unnecessary coupling.

---

## Presence Engine

The Presence Engine is the heart of the runtime.

It receives standardized activity information from plugins and routes it to every enabled output.

It does not know where an activity originated.

Whether an activity comes from FL Studio or another application is irrelevant.

### Responsibilities

* Receive activities
* Maintain current activity state
* Detect activity changes
* Forward updates to outputs
* Prevent unnecessary update spam

---

## Outputs

Outputs publish activities to external systems.

Examples include:

* Discord Rich Presence
* Local HTTP API
* WebSocket API
* OBS Overlay
* Future integrations

Outputs never communicate directly with plugins.

They only consume standardized activity objects produced by the Presence Engine.

---

# Activity Flow

PresenceHub follows a single-direction data flow.

```text
Application
      │
      ▼
Plugin
      │
      ▼
Activity
      │
      ▼
Presence Engine
      │
      ▼
Outputs
```

Each layer performs one responsibility before passing the activity to the next component.

This predictable flow makes the system easier to debug and extend.

---

# Plugin Architecture

Plugins are independent modules responsible for translating application-specific information into a standardized activity format.

Each plugin owns all knowledge about its target application.

For example:

The FL Studio plugin understands:

* Project names
* Tempo
* Playback state
* Timeline position

The Core understands none of these concepts.

It simply receives an activity object.

This allows entirely new applications to be supported without modifying the core.

---

# Architectural Principles

## Lightweight First

PresenceHub is designed to run continuously in the background.

Resource consumption should remain minimal.

Performance is treated as a feature rather than an optimization.

---

## Application Agnostic

The Core should never know anything about individual applications.

Supporting a new application should only require adding a new plugin.

---

## Loose Coupling

Components communicate through well-defined interfaces.

Every layer depends on abstractions rather than concrete implementations.

This allows components to evolve independently.

---

## Single Responsibility

Every component should perform one job well.

Core

↓

Plugin Manager

↓

Presence Engine

↓

Outputs

Responsibilities should never overlap unnecessarily.

---

## Fault Isolation

Plugins should never be capable of crashing the application.

If a plugin fails:

```text
Plugin

↓

Failure

↓

Plugin Disabled

↓

Core Continues Running
```

The rest of PresenceHub should continue operating normally.

---

## Extensibility

New functionality should primarily be introduced through:

* Plugins
* Outputs

The architecture should encourage extension rather than modification.

---

# Performance Goals

PresenceHub should remain lightweight under normal operation.

Initial design targets:

| Component              | Target   |
| ---------------------- | -------- |
| Plugin startup         | < 100 ms |
| Idle CPU usage         | < 0.1%   |
| Memory usage           | < 30 MB  |
| Discord update latency | < 50 ms  |

These values serve as engineering goals rather than strict limits and may evolve as the project matures.

---

# Repository Structure

```text
presencehub/

├── apps/
│   └── desktop/
│
├── crates/
│   ├── core/
│   ├── plugin-host/
│   └── discord-output/
│
├── packages/
│   ├── sdk/
│   └── ui/
│
├── plugins/
│   ├── flstudio/
│   ├── vscode/
│   └── spotify/
│
├── docs/
│
└── .github/
```

This structure separates the core runtime, desktop application, shared packages, plugins, and documentation into clearly defined boundaries.

---

# Technology Stack

| Layer         | Technology               |
| ------------- | ------------------------ |
| Core          | Rust                     |
| Desktop       | Tauri v2                 |
| UI            | React                    |
| Language      | TypeScript               |
| Plugin SDK    | TypeScript               |
| Styling       | Tailwind CSS + shadcn/ui |
| Configuration | TOML or JSON             |
| Logging       | Rust (`tracing`)         |

Each technology has been selected to support the project's goals of performance, maintainability, and accessibility for contributors.

---

# Future Evolution

PresenceHub is designed to grow through extensions rather than modifications.

As the ecosystem expands, the primary additions should be:

* New plugins
* New outputs
* Improvements to shared interfaces

The architecture should remain stable regardless of how many integrations are added.

A successful architecture is one where supporting the next application is no more difficult than supporting the previous one.

---

# Summary

PresenceHub is built around a simple idea:

> The Core coordinates. Plugins understand applications. Outputs communicate with external services.

By maintaining this separation of responsibilities, the project can remain lightweight, scalable, and easy to extend without sacrificing performance or maintainability.
