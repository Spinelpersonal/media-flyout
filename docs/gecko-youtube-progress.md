# Gecko / YouTube progress investigation

Live Windows GSMTC snapshots captured on 2026-11-13 reproduced missing timeline
data from Firefox 160.0 and Zen 1.24.1b (Gecko 159.0.1).

## Observations

The user left a regular YouTube video playing in Firefox with no progress bar.
After switching to Zen, the user reproduced the disappearing bar after using it
to seek and left that video playing. Read-only diagnostics queried each browser's
raw `GetTimelineProperties()` and `GetPlaybackInfo()` results outside the sandbox.

Both affected sessions reported:

| Windows property | Value |
| --- | --- |
| Playback status | Playing |
| Playback rate | 1.0 |
| Playback position enabled | true |
| Start time | 0 |
| End time | 0 |
| Position | 0 |
| Minimum seek time | 0 |
| Maximum seek time | 0 |

The timeline values are absolute 100 ns ticks. These are successful Windows API
reads with empty data, rather than a failed media-service connection. Metadata
and play/pause support were present. The resulting flyout snapshots had
`timeline: None`, `controls.seek: false`, and no status error.

In the later snapshot, Firefox was paused and had a valid duration and position.
This shows that availability can change within the same browser session; it
does not establish pausing as a reliable workaround.

Gecko's [`WindowsSMTCProvider::SetPositionState`](https://github.com/mozilla-firefox/firefox/blob/main/widget/windows/WindowsSMTCProvider.cpp)
sets the timeline's duration and position to zero when no position state is
available, then publishes that timeline to Windows. The captured values are
consistent with this path. They do not establish why YouTube/Gecko lost the
position state or whether all Gecko sites or versions are affected.

## Flyout fix and remaining limitation

The Slint UI separately required an empty status message before displaying
progress. A rejected seek, control error, or settings warning therefore replaced
even a valid timeline and prevented another seek until the status cleared. A
headless pointer regression test reproduced this independently of the browser.

Status messages now appear below valid progress, and unrelated status updates
no longer cancel a drag. The regression test verifies that seeking can be retried
with a rejection message present. The Windows media integration fixture also
passes discovery, playback controls, seeking, and session-switching checks.

This UI fix does not restore the all-zero timelines captured above. Without a
duration or seek range, the flyout cannot construct an accurate progress bar or
seek target. Keeping an old timeline indefinitely could show the wrong position
or reuse a range after media changes, so empty timelines remain omitted.

## Further diagnosis

Run `media-flyout.exe --diagnose` while the symptom is present to inspect the
selected source, normalized timeline, supported controls, and status. `--watch`
prints subsequent Windows media events. For upstream investigation, compare raw
`StartTime`, `EndTime`, `Position`, `MinSeekTime`, and `MaxSeekTime` before seeking,
after seeking, and after pausing/resuming or reopening the flyout.

Browser UI automation could not complete a live YouTube retest: the computer-use
tool stopped because URL-policy enforcement is unsupported for the Windows
Firefox browser. The live observations above used playback arranged by the user
and read-only Windows API diagnostics.
