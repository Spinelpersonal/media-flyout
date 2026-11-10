# Media Flyout

A compact Windows 10/11 media controller written in Rust. Development follows the
phases in [prompt.md](prompt.md).

Running the executable without arguments opens the Slint flyout, with album art,
metadata, and playback controls. Its native Win32 backend presents Slint's
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
.\target\release\media-flyout.exe --preview preview.bmp
```

`--play-pause`, `--previous`, and `--next` send a single command to the selected
session. Only players that publish Windows System Media Transport Controls are
discoverable. No media session is a normal, empty result.

`--preview` briefly opens the real flyout, saves its own Slint-rendered bitmap,
and exits. Artwork uses Windows Imaging Component, is decoded on the media
worker, and is reduced to at most 256 × 256 pixels before reaching the UI.

## License

GPL-3.0-only. Slint will be used under its GPLv3 option so the complete distributed
application and corresponding source use compatible terms. See
[Slint's licensing FAQ](https://slint.dev/faqs).
