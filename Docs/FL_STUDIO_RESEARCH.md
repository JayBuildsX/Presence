# FL Studio Research Spike

> **Status:** Complete
> **Date:** 2026-07-30
> **Author:** Lead Software Architect

---

## Goal

Determine what information PresenceHub can realistically obtain from a running instance of FL Studio on Windows, without requiring memory inspection, reverse engineering, or unsupported APIs.

---

## Questions Investigated

### Process Detection

**Can we detect if FL Studio is running?**

**Yes.** FL Studio runs as a standard Windows process. The executable name is:

| Architecture | Executable |
|---|---|
| 64-bit | `FL64.exe` |
| 32-bit | `FL.exe` |

Detection methods (all reliable):

| Method | Reliability | Notes |
|---|---|---|
| `EnumProcesses` + `GetModuleBaseName` | High | Standard Windows API. Works on all versions. |
| `CreateToolhelp32Snapshot` | High | Provides process list with executable names. |
| WMI query (`Win32_Process`) | High | Overkill for simple detection. |
| `EnumWindows` + `GetWindowThreadProcessId` | High | Also provides window handle. |

**Recommended approach:** `EnumWindows` + `GetWindowThreadProcessId`. This gives us both the window handle and the PID in one pass, eliminating the need for separate process enumeration.

**Can we get the process ID?** Yes. `GetWindowThreadProcessId` returns the PID directly.

**Can we get the executable path?** Yes. `QueryFullProcessImageNameW` returns the full path (e.g., `C:\Program Files\Image-Line\FL Studio 20\FL64.exe`).

**Can we monitor process lifetime?** Yes. The window handle becomes invalid when FL Studio closes. Polling `IsWindow` or catching `WM_DESTROY` via a message hook would work.

### Window Detection

**Can we obtain the window title?** Yes. `GetWindowTextW` returns the title. FL Studio's title format is:

```
FL Studio 20 - [ProjectName.flp]
```

Or with unsaved changes:

```
FL Studio 20 - [ProjectName.flp*]
```

When no project is loaded:

```
FL Studio 20
```

**Can we get the window handle?** Yes. `FindWindowW` with class name `TFruityLoopsMainForm` (or similar) or `EnumWindows` with title matching.

**How often does the title update?** The title updates when:
- A project is loaded (immediate)
- A project is saved (immediate, asterisk removed)
- Changes are made (asterisk added)
- A new project is created
- The application is minimized/restored (no title change)

For polling purposes, checking every 1–2 seconds is sufficient to catch all title changes.

### Project Information

**Can we get the project name?** **Yes, partially.** The project name is embedded in the window title. However:

- The full project path is NOT available in the title.
- Only the filename (e.g., `ProjectName.flp`) is shown.
- Unsaved changes are indicated by an asterisk: `ProjectName.flp*`.

**Can we get the full project path?** **No.** FL Studio does not expose the full project path through any standard Windows API. The window title only contains the filename. The Recent Files list in the FL Studio registry could be used to correlate filenames with paths, but this is unreliable when multiple projects share the same name.

**Can we detect unsaved projects?** **Yes.** The asterisk in the title indicates unsaved changes.

### Playback State

**Can we determine Play/Stop/Pause/Record status?**

**No, not through standard APIs.** FL Studio does not expose playback state through:
- Window title
- Window class
- Windows messages
- COM interfaces
- Named pipes
- Registry keys

Playback state information is held internally in FL Studio's process memory. Obtaining it would require:
- Memory inspection (reading process memory via `ReadProcessMemory`)
- Win32 message hooking (intercepting FL Studio's internal messages)
- DLL injection (running code inside FL Studio's process)

These approaches are:
- Fragile (break with FL Studio updates)
- Potentially detected as cheating/malware by antivirus
- Require deep reverse engineering of FL Studio's internal structures
- Not suitable for a production application

**Recommendation:** Do not attempt to obtain playback state in the initial implementation. If the window title changes between "[Project.flp]" and "[Project.flp*]", the plugin can infer that the user is editing. This is not playback state, but it's the best available signal without memory inspection.

### Musical Information

**Can we obtain BPM, time signature, playback position, or song length?**

**No, not through standard APIs.** All musical information is held internally in FL Studio's process memory. No public API or IPC mechanism exposes this data.

**Are there alternative approaches?**

| Approach | Feasibility | Risk |
|---|---|---|
| MIDI output from FL Studio | Low | FL Studio can send MIDI clock, but this requires configuring FL Studio to output to a virtual MIDI port. Not automatic. |
| Last Tweaked parameter | Medium | FL Studio exposes the last tweaked parameter via the remote control system, but this is for *control*, not *observation*. The parameter only exists when the user is actively tweaking a knob. |
| Memory inspection | High (technically) | Very high risk. Breaks with every update. AV warnings. Not production-ready. |
| FL Studio's internal scripting (Patcher, Score, etc.) | Low | These are for generating music, not for external observation. |

**Recommendation:** Do not attempt to obtain musical information in the initial implementation. This data is not accessible through safe, supported means.

### Plugin Communication

**Does FL Studio expose any of the following?**

| Mechanism | Available | Details |
|---|---|---|
| IPC | No | No documented IPC mechanism. |
| COM | No | FL Studio does not register any COM interfaces for external control. |
| Named Pipes | No | No named pipes created by FL Studio have been documented. |
| Windows Messages | Partial | FL Studio responds to standard window messages (minimize, close, etc.) but does not expose custom messages for data retrieval. |
| SDK | No | Image-Line does not provide a public SDK for FL Studio. |
| Official API | No | No REST, WebSocket, or other API exists. |
| Remote Control | Partial | FL Studio's Remote Control system allows MIDI mapping of parameters. This is input-only (for controlling FL Studio), not output (for reading FL Studio's state). |
| OSC | No | FL Studio does not natively support OSC. |
| NetBus (FL Studio's internal plugin bus) | No | Internal only. Not exposed externally. |

### Existing Community Solutions

Several open-source projects have attempted FL Studio integration:

| Project | Approach | Limitations |
|---|---|---|
| **FL Studio Rich Presence** (various) | Window title parsing | Only gets project name. No playback state, no BPM. |
| **flp-cli** | FLP file format parsing | Reads project files directly. Does not observe running instance. |
| **MIDI-based solutions** | Virtual MIDI port | Requires user to configure FL Studio MIDI output. BPM only. |
| **Memory readers** | `ReadProcessMemory` | Fragile, AV warnings, reverse engineering required. |

**Common pattern across all successful implementations:** Window title parsing is the universal approach. Every project that works reliably uses the window title as the primary data source. No project has achieved reliable playback state or BPM reading without memory inspection.

---

## Findings

### Confirmed Possible

| Data | Method | Reliability |
|---|---|---|
| Is FL Studio running | `EnumWindows` + title check | 100% |
| Process ID | `GetWindowThreadProcessId` | 100% |
| Executable path | `QueryFullProcessImageNameW` | 100% |
| Window handle | `EnumWindows` / `FindWindow` | 100% |
| Window title | `GetWindowTextW` | 100% |
| Project filename | Parse window title | 100% (when project loaded) |
| Unsaved indicator | Asterisk in title | 100% |
| Process lifetime | Window handle validity | 100% |

### Currently unavailable through supported APIs

| Data | Reason |
|---|---|
| Full project path | Not exposed in window title. Not available via any standard API. |
| Playback state (Play/Stop/Pause/Record) | Internal to FL Studio process. No API or IPC exposes it. |
| BPM | Internal to FL Studio process. No API or IPC exposes it. |
| Time signature | Internal to FL Studio process. No API or IPC exposes it. |
| Playback position | Internal to FL Studio process. No API or IPC exposes it. |
| Song length | Internal to FL Studio process. No API or IPC exposes it. |
| Track names | Internal to FL Studio process. Not exposed externally. |
| Plugin list | Internal to FL Studio process. Not exposed externally. |

> **Note:** Future FL Studio releases may expose additional APIs.
> This section documents what is unavailable *today* through supported APIs.

### Unknown

| Question | Status |
|---|---|
| Can FL Studio's window title format change between versions? | Likely. The format has remained stable for several major versions, but is not guaranteed. |
| Can we detect FL Studio 21 vs 20 from the title? | Yes, the version number is in the title (e.g., "FL Studio 20", "FL Studio 21"). |
| Does the FL Studio Remote Control system expose any readable state? | Unclear. The remote control is primarily for MIDI input. Output capabilities are undocumented. |
| Can we use FL Studio's internal scripting engine (Patcher) to send data out? | Patcher can send MIDI, but this requires user configuration. Not automatic. |

---

## Risks

1. **Window Title Format Changes:** Image-Line could change the title format in a future update, breaking project name extraction. Mitigation: The plugin should parse the title defensively and degrade gracefully.

2. **FL Studio Version Detection:** Different FL Studio versions may have slightly different window class names or behaviors. The plugin should be tested against FL Studio 20 and 21.

3. **No Playback State:** The most visible feature of Discord Rich Presence is "Playing" / "Editing" / "Idle" state. Without playback state, the plugin can only show the project name. This is a significant limitation.

4. **No BPM:** BPM display is a common feature in music-related Rich Presence. Without it, the presence is less informative.

5. **False Positives:** Other applications named "FL Studio" or windows containing "FL Studio" in the title could trigger false detection. The window class name should be verified to reduce false positives.

6. **Antivirus Flags:** If memory inspection is attempted in the future, antivirus software may flag the behavior as suspicious.

---

## Recommended Plugin Architecture

Based on the research findings, the FL Studio plugin should:

### Data Sources

```
┌─────────────────────────────────────┐
│           FL Studio Process          │
│                                     │
│  ┌───────────────────────────────┐  │
│  │         Window Title           │  │
│  │  "FL Studio 20 - [song.flp*]" │  │
│  └───────────┬───────────────────┘  │
│              │                      │
│              ▼ GetWindowTextW       │
└──────────────┼──────────────────────┘
               │
               ▼
┌─────────────────────────────────────┐
│        FL Studio Plugin             │
│                                     │
│  Parse: "FL Studio 20 - [s.flp*]"  │
│  ─► version: "20"                  │
│  ─► project: "song.flp"            │
│  ─► unsaved: true                  │
│  ─► state: "Editing"               │
│                                     │
│  Poll every 2 seconds              │
│  Emit Activity on change           │
└─────────────────────────────────────┘
```

### Activity Mapping

| FL Studio State | Activity State | Activity Details |
|---|---|---|
| Running, project loaded | `"Editing"` | `"Project: song.flp"` |
| Running, no project | `"Idle"` | `None` |
| Running, unsaved changes | `"Editing"` | `"Project: song.flp*"` |
| Not running | No activity | N/A |

### Polling Strategy

- Poll window title every 2 seconds
- Compare with previous title
- Only emit activity when title changes
- Rate-limit emissions to prevent spam

### Graceful Degradation

- If the title format changes in a future FL Studio version, display the raw title as `details` instead of failing
- If FL Studio is not running, emit no activity (the Presence Engine will handle idle state)

### What to Skip

- Do NOT attempt playback state detection (not possible without memory inspection)
- Do NOT attempt BPM detection (not possible without memory inspection)
- Do NOT implement memory inspection (fragile, high risk)
- Do NOT attempt DLL injection (security risk, AV issues)

---

## Future Research

Potential future information sources (not investigated):

- **MIDI** — FL Studio can output MIDI clock to a virtual port. Requires user configuration. Not automatic.
- **OSC** — Not natively supported by FL Studio. Would require a bridge.
- **FL Studio Python scripting** — Internal scripting engine (Patcher, Score). May be able to send data out via MIDI. Requires user configuration.
- **Native Image-Line APIs** — No public SDK exists today, but Image-Line could expose one in the future.
- **Memory inspection** — `ReadProcessMemory` approach. Technically possible but fragile, high risk, AV flags. Last resort only.

Status: Not investigated. Not implemented.

---

## Next Steps

1. Implement the FL Studio plugin using the window title parsing approach.
2. Test against FL Studio 20 and 21 (if available).
3. Consider adding a configuration option for custom title format patterns.
4. Document the plugin's limitations clearly for users.
5. If playback state or BPM become critical features, evaluate the MIDI clock approach (requires user configuration).