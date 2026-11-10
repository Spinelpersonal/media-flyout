# Media Flyout

A compact Windows 10/11 media controller written in Rust. Development follows the
phases in [prompt.md](prompt.md).

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
```

`--play-pause`, `--previous`, and `--next` send a single command to the selected
session. Only players that publish Windows System Media Transport Controls are
discoverable. No media session is a normal, empty result.

## License

GPL-3.0-only. Slint will be used under its GPLv3 option so the complete distributed
application and corresponding source use compatible terms. See
[Slint's licensing FAQ](https://slint.dev/faqs).
