# Media Flyout

A compact Windows 10/11 media flyout built with Rust, Slint's software renderer,
and native Windows APIs. It runs quietly in the tray and opens with **Ctrl + Space**.
No account connection or Spotify Web API is required.

> **Known Windows startup issue:** After signing in to Windows, the configured
> shortcut has been reported to remain unresponsive until **Show / hide flyout**
> or **Reload settings** is selected in the tray. Use either action as a workaround.
> The root cause is unresolved; this has not been established as a Rust limitation.
> Hidden-launch desktop tests pass, but they do not reproduce a full Windows sign-in.
> There is no verified automatic fix yet.
> 
> **Known YouTube issue in Gecko browsers:** Live diagnostics captured Firefox and
> Zen reporting zero duration, position, and seek range to Windows while a video
> was playing, including after Zen's progress bar disappeared. These empty timelines
> cannot provide a progress bar. A separate flyout bug that hid valid progress
> whenever a status message appeared is fixed; missing browser data remains
> unresolved. See [diagnostic findings](docs/gecko-youtube-progress.md).

- Real GSMTC metadata, artwork, playback state, source, and optional progress.
- Previous, play/pause, next, and click/drag seeking that follow the player's capabilities.
- Automatic selection of playing media, manual session switching, and saved app preferences.
- Dark, light, and system themes; rounded corners; short optional transitions.
- Configurable shortcuts, screen position, monitor, margin, and dismissal behavior.
- Native tray menu, single-instance operation, and opt-in Windows startup.

The measured release used **22.94 MiB working set before first show** and
**25.73 MiB while visible** on the development machine. Hidden sampled CPU use
was about **0.01% of the machine**. See [measurement conditions and reproduction
instructions](docs/performance.md); these results are not guarantees.

## Tested applications
 - **Spotify**: Source shows up correctly as Spotify, everything works like it's supposed to.
 - **Tidal**: Source shows up correctly as Tidal, everything works like it's supposed to.
 - **Foobar2000**: Source shows up correctly as Foobar2000, everything works like it's supposed to.
 - **Youtube on Zen Browser (Gecko)**: Source shows up correctly as Zen, everything apart from progress bar works like it's supposed to. Currently looking into this issue.
 - **Youtube on Google Chrome (Chromium)**: Source shows up correctly as Google Chrome, everything works like it's supposed to, progress bar control works too. 

## Requirements

- Windows 10 version 1809 or newer, or Windows 11, x64.
- A player that publishes Windows System Media Transport Controls.
- The ordinary interactive Windows desktop and notification area.

Spotify was checked live. Browser players, foobar2000 configurations with GSMTC
support, and other compatible players use the same Windows interface; their
metadata and control support depend on the player. This application does not
install player extensions or access player accounts.

## Build and run

Install the current stable Rust MSVC toolchain and Visual Studio Build Tools with
**Desktop development with C++**, x64 tools, and a Windows SDK. Rust 1.92 is the
minimum required by the pinned Slint version; current stable Rust is recommended.

```powershell
git clone https://github.com/Spinelpersonal/media-flyout.git
cd media-flyout
cargo build --locked --release
.\target\release\media-flyout.exe
```

The executable starts hidden, without a console or permanent main window.
Press **Ctrl + Space** to show it and press again to dismiss it. It also dismisses
after ten seconds, on Escape, on the close button, or on an observed outside click.
Using a media control restarts the timeout. Dismissal leaves the background process
running. Media buttons remain clickable without taking focus from the current app.

Click the tray icon to toggle the flyout. Right-click it for settings, reload,
startup, preferences, and exit. If the shortcut is unavailable, the tray remains
usable and reports the conflict. Launching another instance shows the existing
window. Use the tray's **Exit** command to stop the application.

The portable executable can be placed in a permanent folder; installation and
administrator access are unnecessary. The C runtime is statically linked so a
separate Visual C++ redistributable is not required. Windows builds are unsigned. CI produces
binary and corresponding-source archives as build artifacts; no published GitHub
release or installer is required to build locally.

## Settings

The first launch creates `%APPDATA%\MediaFlyout\config.toml`. Choose **Open settings**
in the tray, edit the file in Notepad, save, then choose **Reload settings**.
[config.example.toml](config.example.toml) documents every option.

```toml
shortcut = "Ctrl+Space"
position = "top-center"
monitor = "active"
margin = 20
preferred_app = ""
remember_preferred_app = false
theme = "dark"
animations = true
show_progress = true
dismiss_after_seconds = 10
dismiss_on_outside_click = true
autostart = false
```

| Setting | Supported values |
| --- | --- |
| `shortcut` | Modifiers `Ctrl`, `Alt`, `Shift`, `Win` plus a letter, digit, named navigation key, or F1–F24 except Windows-reserved F12. Function keys also work alone. |
| `position` | `top-center`, `top-left`, `top-right`, `bottom-center`, `bottom-left`, `bottom-right` |
| `monitor` | `active` (foreground app), `cursor`, `primary` |
| `margin` | 0–256 logical pixels inside the monitor's taskbar-aware work area |
| `theme` | `dark`, `light`, `system` |
| `preferred_app` | Exact GSMTC application ID, such as `Spotify.exe`; empty for no preference |
| `dismiss_after_seconds` | 0–600; `0` disables the timeout |

The remaining options are booleans. Animations also respect Windows' client-area
animation setting. Unsupported, missing, or unreliable timelines are omitted.
Progress is estimated between Windows events and clamps to the duration. Click
the progress bar or drag it to seek forward or backward. Dragging previews the
target time and seeks when released, within the player's supported seek range.
Players without seeking support keep a read-only progress bar.

Invalid fields use their defaults with a warning; valid fields remain usable.
Malformed TOML is never overwritten, and a malformed reload retains the last
working configuration. Preference saves preserve comments and replace the file
atomically. A shortcut change registers the replacement before releasing the old
shortcut, so a conflict preserves the working registration. Settings reload on
request; there is no permanent file watcher or polling loop.

### Multiple media sessions

Click the source application name above the title to open the session selector.
The list supports scrolling and dragging. Choose a session or **Automatic**.
Different windows from the same source have separate session entries.

Source labels use readable player names and Windows' installed application names
when available, including browsers that report an installation-specific ID.
YouTube playing in Zen or Firefox is labeled with the browser's name; an installed
web app can use its own registered name. If an opaque ID cannot be resolved, the
label is **Media player**. `--diagnose` retains the original application ID for
troubleshooting and preferences.

Manual selection stays in effect until that session disappears or automatic mode
is selected. Automatic mode prioritizes a playing preferred app, Windows' current
playing session, then any playing session. A paused preference does not displace
playing media. When nothing is playing, preferences and Windows' current session
still guide selection.

Enable **Remember selected application** in the tray to persist the selected
source for future launches. **Clear application preference** clears the saved
preference and restores automatic selection. The preference is an application ID,
not a temporary session identifier.

### Start with Windows

Autostart is **disabled by default**. Explicitly enable **Start with Windows** in
the tray, or set `autostart = true` and reload. The application manages only its
own value at:

```text
HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run\MediaFlyout
```

The value contains quoted executable and configuration paths. Disable the option
before moving or deleting the executable. Changing the file to `false` and
reloading also removes the entry for that configuration. Other startup entries
are untouched. Windows can delay startup applications.

## Commands and diagnostics

```powershell
.\target\release\media-flyout.exe --show
.\target\release\media-flyout.exe --reload
.\target\release\media-flyout.exe --quit
.\target\release\media-flyout.exe --diagnose
.\target\release\media-flyout.exe --watch
.\target\release\media-flyout.exe --config .\my-settings.toml --show
.\target\release\media-flyout.exe --preview preview.bmp
```

`--diagnose` prints the first real media snapshot. `--watch` prints event-driven
updates until interrupted, including artwork arriving after metadata. `--play-pause`,
`--previous`, and `--next` send one command to the selected session. These commands
use the configured application preference; UI-only manual choices belong to the
running GUI process. `--help` and `--version` are also available.

`--preview` opens the real flyout briefly, saves its own Slint-rendered bitmap,
and exits. Close the existing instance first. Diagnostic text may need redirection
when launched from a shell that treats Windows GUI executables asynchronously.
For example: `media-flyout.exe --diagnose > media-status.txt`.

## How media detection works

Players publish their media state to Windows. The [GSMTC session manager](https://learn.microsoft.com/en-us/uwp/api/windows.media.control.globalsystemmediatransportcontrolssessionmanager)
exposes those sessions, their metadata, supported commands, and event notifications
to desktop applications. Media Flyout subscribes to those events rather than
polling the media API. Metadata, playback, and session changes update the reusable
flyout through a bounded worker queue and a latest-snapshot UI mailbox.

Artwork is decoded by Windows Imaging Component on a separate worker, resized
before pixel transfer, and checked against the selected track revision. There is
no GPU rendering framework or browser runtime. See [architecture and resource
ownership](docs/architecture.md) and [performance](docs/performance.md).

## Development and verification

```powershell
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --locked --release
```

The ordinary test suite covers selection priorities, same-app preferences,
configuration validation and safe edits, shortcut parsing, CLI arguments, timeline
validation/extrapolation and seek ranges, progress-bar pointer interactions,
startup quoting, native pixel blending, and screen geometry.

Two desktop integration suites are ignored by default because they require real
Windows services and an interactive shell. Close any running flyout before running
the lifecycle suite, and run its tests serially:

```powershell
cargo test --locked --test media_integration -- --ignored --test-threads=1
cargo test --locked --test lifecycle -- --ignored --test-threads=1
```

The media suite publishes silent test sessions and verifies real GSMTC discovery,
Unicode metadata events, play/pause/next/previous and seek delivery, timelines, unsupported
controls, switching between two sessions from the same application, wrapper
reconciliation, and removal. It does not send commands to personal media players.
The lifecycle suite verifies hidden startup, single-instance reuse, timeout,
configuration reload and positioning, shortcut conflicts/replacement, and cleanup.
It also sends an isolated **Ctrl + Alt + Shift + F24** shortcut before the first
show or reload, with animations enabled and disabled and `STARTF_USESHOWWINDOW`
set to `SW_HIDE`. Run this case against the release build to exercise the Windows
GUI executable, and release modifier keys before running it:

```powershell
cargo test --locked --release --test lifecycle shortcut_works_before_first_show -- --ignored --test-threads=1
```

Live Spotify metadata/artwork and application-rendered dark/light previews were
also checked. Physical shortcut presses, tray-menu clicks, Escape/outside-click
input, Windows logon, suspend/resume, and mixed-DPI monitor disconnection still
need manual hardware/desktop testing. The native UI automation helper was
unavailable during development, so those interactions are not claimed as tested.

GitHub Actions runs formatting, clippy, unit tests, and a release build on Windows.
It packages the executable, README, license, example settings, and a separate
corresponding-source archive with vendored dependencies. Keep both archives
available together when distributing the GPL build. Desktop-dependent tests are
run explicitly on a suitable local machine.

## Known limitations

- Player support and metadata accuracy depend on the provider's GSMTC implementation.
  Reopening the flyout refreshes sessions and metadata; resume reconnects the manager.
- Escape and outside clicks use 50 ms checks only while visible. Very short input
  may be missed. This preserves foreground focus without installing input hooks.
- Timelines can be incomplete or inaccurate. Unknown/live ranges are hidden;
  extrapolation cannot correct a provider that reports an incorrect position.
- Windows exposes no persistent public ID for otherwise identical sessions from
  the same app. If Windows republishes indistinguishable sessions, their
  source-relative order is used as a final identity fallback.
- Artwork over 8 MiB encoded, 8192 pixels in either dimension, or 16 million pixels
  is rejected. Missing/invalid artwork uses the built-in fallback.
- The minimal native backend does not expose a Windows UI Automation tree or full
  keyboard navigation. Buttons are intended for pointer use without taking focus.
- Windows security desktops and some exclusive full-screen applications can cover
  ordinary topmost tool windows. Builds are portable and unsigned.

## License

[GPL-3.0-only](LICENSE). Slint is used under its GPLv3 licensing option, and the
distributed application is GPLv3 as a whole. Its royalty-free/commercial options
are not assumed. See [Slint's licensing FAQ](https://slint.dev/faqs) and
[license choices](https://slint.dev/get-started).

Dependency licenses still apply; their license files are retained in the vendored
source archive. Redistributed binaries must be accompanied by the corresponding
source under the applicable GPLv3 distribution terms. For a source checkout,
`Cargo.lock` records the exact dependencies used to reproduce the build.
