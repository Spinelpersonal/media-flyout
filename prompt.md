# Project: Lightweight Windows Media Flyout (Rust + Slint)

You are an experienced Rust developer specializing in Windows desktop applications, native Windows APIs, performance optimization, and lightweight graphical interfaces.

I want you to build a complete, production-quality, open-source Windows application that displays a customizable media flyout when the user presses a global keyboard shortcut.

The application should be extremely lightweight, responsive, visually polished, and suitable for publishing on GitHub.

Prioritize clean architecture, low resource consumption, maintainability, and reliability.

## 1. Technology Stack

Use the following technologies:

- **Language:** Rust (latest stable release)
- **UI Framework:** Slint
- **Rendering:** Slint software renderer, avoiding unnecessary GPU usage
- **Windows Integration:** `windows` crate (windows-rs)
- **Media API:** Windows Global System Media Transport Controls (GSMTC)
- **Target OS:** Windows 10 and Windows 11, x64

Use the latest compatible stable versions of the required dependencies, and verify their APIs before implementing them.

Avoid Electron, Tauri, WebView2, Chromium, browser-based UIs, or other heavy frameworks.

Use native Windows APIs whenever practical.

Keep external dependencies minimal.

## 2. Main Functionality

### Media Detection

The application must automatically detect the currently playing media through Windows GSMTC.

It should support any compatible Windows media session, including:

- Spotify
- foobar2000
- Browser media playback (Firefox, Chrome, etc.)
- Other compatible Windows media players

Do not use Spotify's Web API or require users to connect their Spotify accounts.

Retrieve the following information where available:

- Track title
- Artist name
- Album title
- Album artwork
- Playback state
- Playback duration
- Current playback position
- Source application

Update the displayed information automatically when the song changes, media is paused, or the active media session changes.

Use event-based notifications rather than aggressive polling.

Handle missing metadata, missing artwork, unsupported controls, and unavailable media sessions gracefully.

### Multiple Media Sessions

Windows may expose multiple media sessions simultaneously.

Implement logic that:

1. Prioritizes the currently active or playing media session.
2. Uses Windows' current-session preference when appropriate.
3. Allows the user to manually select another available media session.
4. Remembers the preferred application if configured.
5. Handles applications closing, opening, or becoming unavailable.

Avoid accidentally displaying an inactive browser session when Spotify or foobar2000 is actively playing.

### Media Controls

Provide the following controls within the flyout:

- Previous track
- Play/Pause
- Next track

Use GSMTC playback-control methods.

Only enable controls supported by the selected session.

Optionally display a progress bar and playback timestamps when reliable timeline information is available.

Do not implement volume controls or audio visualization in the initial version.

## 3. Flyout Interface

Create a modern, minimalistic media flyout with Slint.

The visual design should be inspired by modern Windows 11 interfaces, but should have its own identity rather than copying Windows' native media flyout.

The flyout should contain:

- Album artwork on the left
- Track title
- Artist name
- Album name, when available
- Previous, Play/Pause, and Next buttons
- Optional subtle progress bar
- A discreet indicator of the source application

### Design Requirements

- Dark theme by default
- Rounded corners
- Compact dimensions
- Clean typography
- Appropriate spacing
- Subtle hover and pressed states
- Support for long track titles and artist names
- Smooth, lightweight opening and closing transitions
- Proper scaling on high-DPI displays
- No unnecessary visual clutter

Aim for a polished application that could be used daily.

Use lightweight animations that do not require continuous rendering when the flyout is hidden.

Avoid expensive effects such as elaborate GPU shaders.

If native Windows acrylic or blur effects can be added without significantly increasing complexity or resource consumption, consider implementing them as an optional feature. Otherwise, prioritize a clean opaque or subtly translucent interface.

## 4. Global Keyboard Shortcut

The flyout should be accessible from anywhere in Windows using a global keyboard shortcut.

Default shortcut:

`Ctrl + Space`

Use the native Win32 `RegisterHotKey` API, or another equally lightweight and reliable method.

Requirements:

- Works while other applications are focused.
- Opens the flyout when pressed.
- Closes the flyout when pressed again or after 10 seconds.
- Allows the shortcut to be customized through a configuration file.
- Handles shortcut-registration failures gracefully.
- Unregisters the shortcut correctly when the application exits.

Do not use a permanent keyboard hook if a simpler Windows API works.

## 5. Window Behavior

The application should run primarily in the background.

The flyout should:

- Appear at the top of the screen in the middle.
- Remain above ordinary application windows while visible.
- Support multiple monitors.
- Position itself correctly with different taskbar placements and DPI scaling.
- Close when the user clicks outside it, where practical.
- Close when Escape is pressed.
- Reuse the same window instead of constantly creating new windows.

Consider allowing future configuration of its position.

Avoid unnecessarily stealing keyboard focus from the currently active application, while ensuring the media controls remain clickable and functional.

## 6. System Tray Integration

Implement a lightweight system tray icon.

The tray menu should provide:

- Show/Hide flyout
- Settings or configuration
- Exit application

The application must not require a permanently visible main window.

Closing the flyout should not terminate the background process.

Allow users to enable or disable starting the application automatically with Windows.

Autostart must be disabled by default and require explicit user action.

## 7. Performance Requirements

**Resource efficiency is one of the highest priorities of this project.**

The application is intended to remain running throughout the day on a Windows laptop.

### Memory Usage

Aim for:

- **Idle/background RAM:** preferably under 30 MB
- **Flyout visible:** preferably under 50 MB
- **Avoid exceeding 100 MB** during normal usage, if realistically achievable

These are optimization targets, not absolute requirements. Measure actual usage rather than claiming these numbers are guaranteed.

Avoid unnecessary memory allocations, repeated loading of assets, or large retained image buffers.

Keep the number of threads, timers, and dependencies small.

### CPU Usage

When idle, CPU utilization should be close to zero.

Use Windows event subscriptions and callbacks to react to:

- Media metadata changes
- Playback state changes
- Media session changes

Avoid continuously polling the media API.

If polling is unavoidable for a particular feature, explain why and use an appropriately low frequency.

### Rendering

Configure Slint to use its software renderer.

Investigate partial rendering and other optimizations supported by the current version of Slint.

Do not continuously redraw the flyout while it is hidden.

Cache album artwork when appropriate, and resize oversized images before displaying them.

### Build Optimization

Configure a proper Rust release profile with suitable optimizations, such as:

- Link-time optimization
- Reduced code generation units
- Symbol stripping
- An appropriate optimization level

Prioritize runtime performance and low memory consumption over achieving the absolute smallest executable.

Do not compromise correctness merely to save a few megabytes.

Document how to benchmark the application using Windows tools, including working-set memory, private memory, and idle CPU usage.

## 8. Application Architecture

Use a clean, modular Rust project structure.

Suggested modules:

- `main.rs` — application initialization and lifecycle
- `media/` — Windows GSMTC integration, session management, and playback controls
- `ui/` — Slint components and flyout presentation
- `hotkey/` — global keyboard-shortcut registration
- `tray/` — system tray integration
- `config/` — persistent user settings
- `platform/` — Windows-specific windowing and operating-system integration

You may adjust the structure if a different organization is simpler or more maintainable.

Clearly separate the media logic, native Windows integration, and user interface.

Use asynchronous operations appropriately and avoid blocking the UI thread.

Respect Rust's ownership model and thread-safety requirements.

Ensure that Windows callbacks and background operations communicate with the Slint event loop safely.

Properly unregister event handlers, release resources, and handle shutdown.

Do not use `unsafe` unless necessary for native interoperability. Keep unavoidable unsafe code isolated and document its safety assumptions.

## 9. Configuration

Use a lightweight configuration file, such as TOML.

Store it in the appropriate Windows user configuration directory.

Include settings for:

- Global shortcut
- Flyout position
- Preferred media application
- Theme
- Autostart
- Animation enable/disable
- Flyout auto-dismiss behavior

Use sensible defaults when the configuration file does not exist.

Handle invalid configuration values gracefully.

Do not implement an elaborate graphical settings window in the first version unless it adds minimal complexity.

## 10. Reliability and Edge Cases

Handle the following situations:

- No media player is running.
- A player is running but no track is selected.
- Multiple players are active.
- The active player closes unexpectedly.
- The player does not expose album artwork.
- The track changes while the flyout is open.
- Metadata is temporarily unavailable.
- Media controls are unsupported.
- The global shortcut is already registered by another program.
- The system is suspended and later resumed.
- The user disconnects an external monitor.
- A player reports incomplete or inaccurate timeline information.

The application should never crash simply because a media session disappears or stops responding.

## 11. GitHub and Open-Source Requirements

Prepare the project for public distribution.

Include:

- A comprehensive `README.md`
- Clear setup and compilation instructions
- A `.gitignore`
- A configuration example
- Instructions for building a release executable
- A list of known limitations
- A short explanation of how GSMTC works
- Instructions for running the application at startup

Review Slint's licensing requirements before choosing a project license.

Do not assume that every permissive license is compatible with the chosen Slint licensing option.

Structure the project so it can eventually support GitHub Actions builds and downloadable releases.

## 12. Development Process

Build the application incrementally.

### Phase 1 — Media Integration

Create a minimal working Rust application that:

1. Connects to Windows GSMTC.
2. Detects the current media session.
3. Retrieves title, artist, and playback state.
4. Responds to song changes.
5. Performs play/pause and track-skipping operations.

Ensure this works before building the graphical interface.

### Phase 2 — Basic Flyout

Implement the Slint interface with:

- Album artwork
- Title and artist
- Playback controls
- Basic window positioning

Connect the interface to real media data.

### Phase 3 — Hotkey and Background Operation

Implement:

- Global keyboard shortcut
- Flyout visibility management
- System tray
- Clean application shutdown

### Phase 4 — Multiple Sessions and Configuration

Implement:

- Session switching
- Application preferences
- TOML configuration
- Optional autostart

### Phase 5 — Polish and Optimization

Improve:

- Animations
- High-DPI scaling
- Multi-monitor behavior
- Memory usage
- CPU usage
- Error handling

Benchmark the release build and investigate unnecessary overhead.

## 13. Coding Standards

Follow idiomatic Rust practices.

Requirements:

- Readable and maintainable code
- Clear module boundaries
- Strong error handling
- Minimal unnecessary dependencies
- Avoid `unwrap()` or `expect()` in recoverable runtime scenarios
- No placeholder functionality presented as complete
- No unfinished TODOs in core features
- No unnecessary abstraction layers
- No excessive logging in release builds
- Avoid memory leaks, busy loops, and unmanaged event subscriptions

Use `cargo fmt`, `cargo clippy`, and `cargo test`.

Add meaningful unit tests for logic that can be tested independently of Windows media APIs.

## 14. Expected Deliverables

The final project should provide:

1. A compilable Windows Rust application.
2. Working media integration through GSMTC.
3. A functional Slint media flyout.
4. Global keyboard-shortcut support.
5. Playback controls.
6. A system tray icon.
7. Basic configuration.
8. A GitHub-ready repository structure.
9. A README explaining installation and development.
10. Documented performance targets and benchmarking instructions.

### Important Instructions

**Actually implement the application. Do not stop after writing an architecture proposal or pseudocode.**

Start by inspecting the current project directory and creating the necessary project files.

Before using external APIs, verify their current documentation and make sure the chosen crate features and APIs are compatible.

After implementing each phase, run the available compilation, linting, and testing commands. Fix issues before continuing.

If execution or testing is unavailable in the current environment, explicitly state what could not be verified instead of pretending that the project was tested.

Favor the simplest working native implementation over complex architecture.

If an implementation decision would significantly increase memory usage or complexity, prefer the lighter option and explain the tradeoff.

The ultimate goal is a **beautiful, highly responsive Windows media flyout that runs quietly in the background with extremely low RAM and CPU usage**, suitable for daily personal use and public GitHub distribution.
