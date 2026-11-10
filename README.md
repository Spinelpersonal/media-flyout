# Media Flyout

A compact Windows 10/11 media controller written in Rust. Development follows the
phases in [prompt.md](prompt.md).

Running the executable starts it in the background. Press **Ctrl + Space** to
toggle the flyout, or use the tray icon. It dismisses after ten seconds, on Escape,
or on an observed outside click; closing it keeps the background process running.
Its native Win32 backend presents Slint's
software-rendered pixels directly through GDI, with no GPU renderer or extra
windowing framework. The window uses the foreground application's monitor and
does not take keyboard focus.

The event-driven GSMTC backend discovers Windows media sessions, follows metadata
and playback changes, and sends play/pause, previous, and next commands without
accounts or network services. A playing session takes priority over a stale,
paused Windows current session. All WinRT work runs on one dedicated worker;
asynchronous requests have a three-second timeout and subscriptions are released
on shutdown.

## Build and diagnose

Install stable Rust for `x86_64-pc-windows-msvc` and the Visual Studio C++ build
tools, then run:

```powershell
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
.\target\release\media-flyout.exe --diagnose
.\target\release\media-flyout.exe --watch
.\target\release\media-flyout.exe --show
.\target\release\media-flyout.exe --quit
.\target\release\media-flyout.exe --preview preview.bmp
```

`--play-pause`, `--previous`, and `--next` send a single command to the selected
session. Only players that publish Windows System Media Transport Controls are
discoverable. No media session is a normal, empty result.

`--preview` briefly opens the real flyout, saves its own Slint-rendered bitmap,
and exits. Artwork uses Windows Imaging Component, is decoded on the media
worker, and is reduced to at most 256 × 256 pixels before reaching the UI.

The shortcut uses `RegisterHotKey` with repeat suppression. If it is already in
use, the tray icon remains available. Escape and outside clicks are checked every
50 ms **only while visible**: this allows dismissal without taking focus or
installing a global input hook. Very short outside clicks may be missed. There
are no input checks or render timers while hidden. Media updates remain driven by
Windows events. The tray icon is restored if Explorer restarts, and a resume
notification reconnects the media manager.

The optional desktop lifecycle integration test starts a separate flyout and
checks hidden startup, window styles, reuse by a second launch, dismissal, and
exit. Close any running flyout first:

```powershell
cargo test --test lifecycle -- --ignored --test-threads=1
```

## Settings and sessions

Settings are created at `%APPDATA%\MediaFlyout\config.toml`. Use the tray's
**Open settings** command to edit them in Notepad, then **Reload settings** (or
`media-flyout.exe --reload`). [config.example.toml](config.example.toml) lists all
values. `--config <path>` selects an isolated file. Unknown/invalid fields produce
a warning and use defaults; malformed TOML is left intact. Reloading malformed
TOML retains the last working settings. Saves preserve comments and replace the
file atomically.

Click the application name above the title to choose a session, or choose
**Automatic** to follow playing media. Manual selection stays in effect until
that session closes. Automatic selection prioritizes a playing preferred app,
Windows' current playing session, then any playing session. Paused preferences
never displace playing media. Enable **Remember selected application** in the
tray to save the chosen GSMTC application ID across launches. **Clear application
preference** restores automatic selection.

The configuration supports six screen positions, foreground/cursor/primary
monitor selection, a DPI-scaled margin, dark/light/system themes, a customizable
shortcut, and dismissal time (`0` disables the timeout). A shortcut change is
registered before releasing the previous one, so a conflict retains the working
shortcut. There is no configuration watcher or permanent file polling.

**Start with Windows** is disabled by default. Enable it explicitly from the tray
or set `autostart = true` and reload. This adds only the current user's
`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\MediaFlyout` entry, containing
the quoted executable and configuration paths. Disable it through the same menu
or set it to `false` and reload before moving/deleting the executable. Other
startup entries are untouched. Windows may delay startup applications.

## License

GPL-3.0-only. Slint will be used under its GPLv3 option so the complete distributed
application and corresponding source use compatible terms. See
[Slint's licensing FAQ](https://slint.dev/faqs).
