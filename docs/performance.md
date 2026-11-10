# Performance

The targets are under 30 MB background RAM, under 50 MB while visible, and CPU
close to zero when idle. They are optimization targets, not limits or guarantees.

## Recorded release measurement

Measured on 2026-11-10 with Windows build `10.0.26300.0`, eight logical processors,
150% display scaling, Rust 1.99.1, Slint 1.18.1, and the MSVC x64 release profile.
Spotify provided real metadata and artwork; playback state was not held constant
during these interactive samples. Each sample lasted about 20 seconds after a
short warm-up. The visible sample enabled progress and
disabled auto-dismiss/outside-click dismissal to keep the window open.

| Scenario | Mean working set | Peak working set | Mean private bytes | CPU time | CPU, whole machine |
| --- | ---: | ---: | ---: | ---: | ---: |
| Hidden, before first show | 22.94 MiB | 22.98 MiB | 3.81 MiB | 0.0156 s | 0.0096% |
| Visible | 25.73 MiB | 25.87 MiB | 4.76 MiB | 0.1250 s | 0.0772% |
| Hidden after showing | 25.93 MiB | 26.12 MiB | 5.08 MiB | 0.0156 s | 0.0096% |

The visible sample used 0.6177% of one logical core. Total thread counts declined
from 16 to 13 as Windows retired thread-pool threads. There are three application
threads. The measured executable was approximately 6.94 MiB with a static C runtime. Startup and artwork
decoding peaks were not profiled in these steady-state samples.

These are short measurements from one machine and one media provider. A measured
zero does not prove that no CPU instructions ran: Windows' accumulated CPU-time
counter has finite resolution. Fonts, artwork codecs, DPI, sessions, Windows
versions, antivirus, and build settings can change the results. Memory is not
artificially trimmed with `EmptyWorkingSet`.

## Reproduce with PowerShell

Build with `cargo build --locked --release`, then start the executable. Use an
isolated file if desired:

```toml
shortcut = "Ctrl+Alt+Shift+F24"
dismiss_after_seconds = 0
dismiss_on_outside_click = false
show_progress = true
```

```powershell
Start-Process .\target\release\media-flyout.exe -ArgumentList '--config', '.\benchmark.toml'
Get-Process -Name media-flyout | Select-Object Id
.\tools\measure.ps1 -ProcessId 1234 -Seconds 60 -Csv idle.csv
```

Replace `1234` with the returned ID. Record hidden startup, visible playback,
paused playback, and hidden usage after several show/hide cycles. Use `--show`
to open the existing process. Restore `dismiss_after_seconds = 10` and reload
settings, or exit from the tray / `--quit`, when finished.

The script reads `WorkingSet64` (resident shared + private pages),
`PrivateMemorySize64` (private committed bytes), and the change in
`TotalProcessorTime`. It reports CPU both as a percentage of the whole machine
and of one logical core. Private committed bytes are different from the private
working set displayed by Task Manager's default Memory column.

## Windows tools

- **Task Manager → Details:** enable Working set (memory), Commit size, CPU time,
  Handles, and Threads. Compare CPU time across at least a minute of hidden
  playback. Account for Task Manager's own sampling interval.
- **Resource Monitor → Memory:** inspect Working Set, Shareable, and Private.
  These are resident-page measurements, rather than private committed bytes.
- **Process Explorer:** compare Working Set and Private Bytes, and inspect thread
  CPU time during hidden operation and repeated show/hide cycles.
- **Windows Performance Recorder / Analyzer:** use CPU Usage sampling when
  investigating startup, image decoding, or an unexpected recurring wakeup.

## Design choices

Only Slint's software renderer is compiled. Its retained buffer enables partial
rasterization, and the native BGR pixel format avoids a presentation copy. There
is no GPU rendering framework, browser runtime, permanent frame loop, config-file
polling, or media polling timer. Opaque surfaces keep rendering predictable.

The only recurring UI timers are visible input checks (20 Hz), visible advancing
progress (1 Hz), and brief transitions (about 120 ms at 16 ms intervals). Auto-dismiss
uses one deadline. Hidden idle operation blocks on native messages and worker
notifications. Album pixels and pending work are bounded; system font and renderer
caches are retained to keep reopening responsive.

Release builds use optimization level 3, thin LTO, one code-generation unit,
symbol stripping, a static C runtime, and abort-on-panic. Correctness and recoverable error handling
take priority over executable size.
