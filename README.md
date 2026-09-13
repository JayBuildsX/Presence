# Presence

<div align="center">
  <h3>Unified, Multi-Application Discord Rich Presence for DAWs, IDEs & Creative Tools</h3>
  <p>Seamlessly broadcast what you are producing, coding, or creating without manual configuration.</p>
</div>

---

## Features

- **Multi-Application Tracking**: Supports multiple apps running simultaneously with intelligent window focus ownership.
- **Built-in First-Class Plugins**:
  - **FL Studio**: Dynamic project name detection, unsaved indicator (`*`), export/rendering status, and version detection.
  - **Antigravity**: Task-aware AI assistant status, active project tracking, and agent work indicators.
  - **OpenCode**: Real-time project tracking, active file name, and file-type icon resolution.
- **Custom Application Watchers**: Monitor *any* process (e.g. Blender, Photoshop, Ableton, Premiere Pro, Spotify) and display live window titles or custom status.
- **Streamer / Privacy Mode**: Instantly mask confidential project names and source paths from Discord.
- **Dynamic System Tray**: Hovering over the Windows taskbar tray icon shows what you're working on in real time.
- **Custom Global Shortcuts**: Control presence without opening the window (pause/resume, reconnect, prioritize, streamer mode).
- **Fast & Resilient**: Written in pure Rust with asynchronous IPC, non-blocking polling, zero UI freezing, and automatic Discord reconnection.

---

## Adding Custom Applications

Presence allows you to monitor any executable running on Windows via the **+ Add App** button in the dashboard.

When adding a custom app, you have two Discord broadcasting options:

### Option 1: Default Presence Broadcasting (Zero Setup)
Leave the **Custom Discord Application ID** field empty.
- **Header**: `Playing Presence`
- **Line 1 (Top Line)**: `{Your App Name} — {Activity / Window Title}`
- **Line 2 (Bottom Line)**: `{Your Secondary Details}` (Optional)
- Your app's display name will always be visible on Discord!

---

### Option 2: Custom Discord Application & Logo Art (Recommended)
If you want Discord to display your app's actual name in the profile header (e.g. `Playing Blender` or `Playing Ableton Live`) along with custom logo artwork:

#### Step 1: Create an Application in Discord Developer Portal
1. Open the [Discord Developer Portal](https://discord.com/developers/applications).
2. Click **New Application** in the top right.
3. Enter your desired application name (e.g. `Blender`, `Ableton Live`, `Photoshop`).
4. Copy the **Application ID** from the **General Information** page.

#### Step 2: Upload Rich Presence Art Assets
1. In the left navigation menu, go to **Rich Presence** → **Art Assets**.
2. Under **Rich Presence Assets**, click **Add Image(s)**.
3. Upload your app's logo/icon.
4. **Important**: Set the asset name / key to `logo` (or `app_logo`). Presence automatically searches for the `logo` asset key when querying Discord's Rich Presence API.
5. Click **Save Changes** at the bottom of the Discord Developer Portal.

#### Step 3: Connect It in Presence
1. In Presence, click **+ Add App** (or click the edit pencil icon on an existing custom app).
2. Select your running application from the **Pick from Running Applications** dropdown.
3. In the **Custom Discord Application ID** field, paste your copied **Application ID**.
4. Presence will instantly verify your application with Discord, verify the application title, and confirm that your `logo` asset was found.
5. Click **Add Application** / **Save Changes**.
6. When your app is active, Discord will display:
   - **Header**: `Playing <Your App Name>`
   - **Logo**: Your custom uploaded `logo` art asset (if no logo was uploaded, Presence avoids broken placeholder icons).
   - **Line 1 (Top Line)**: Active window title or primary activity.
   - **Line 2 (Bottom Line)**: Secondary details (if provided).

---

## Global Shortcuts

Configure custom key combinations in **Settings** → **Shortcuts**:

| Action | Default Shortcut | Description |
|---|---|---|
| **Pause / Resume** | `Ctrl + Shift + P` | Toggle presence broadcasting on or off |
| **Reconnect Discord** | `Ctrl + Shift + D` | Force-reconnect to Discord IPC pipe |
| **Streamer Mode** | `Ctrl + Shift + S` | Toggle privacy masking for project titles |
| **Prioritize App** | `Ctrl + Shift + 1` | Pin the current application's presence |

---

## Installation & Download

### Quick Download (No Dev Setup Required)
If you're looking to use Presence without building it from source:
1. Head over to the **[GitHub Releases](../../releases/latest)** page.
2. Download the latest Windows installer (`Presence_x64_en-US.msi` or setup `.exe`).
3. Run the installer and launch Presence from your Start menu or system tray.

---

## Development & Building

If you want to contribute or build from source:

### Prerequisites
- [Rust](https://rustup.rs/) (1.78 or later recommended)
- [Node.js](https://nodejs.org/) (v18 or later)
- Windows 10/11

### Setup & Run
```bash
# Clone the repository
git clone https://github.com/JayBuildsX/presence.git
cd presence

# Install frontend dependencies
npm install

# Run the desktop app in development mode
npm run tauri dev
```

### Building the Release Installer
```bash
npm run tauri build
```
The compiled standalone `.exe` and `.msi` installers will be generated in `src-tauri/target/release/bundle/`.

---

## Architecture

Presence is structured as a modular Rust workspace:
- `crates/core`: Core domain models (`Activity`, `RichPresence`, `PresenceEngine`), session tracking, configuration, and ownership policies.
- `crates/plugin-host`: Plugin container, dynamic registration, fault isolation, and process polling.
- `crates/discord-output`: Native asynchronous Discord IPC protocol implementation with zero external Discord SDK dependencies.
- `plugins/*`: First-class application integrations (`flstudio`, `antigravity`, `opencode`).
- `apps/desktop`: High-performance Tauri v2 desktop application with React, TypeScript, and modern dark-mode UI.

---

## License

MIT License. See [LICENSE](LICENSE) for details.
