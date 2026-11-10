#![cfg(windows)]

use media_flyout::media::{Action, MediaService, PlaybackState, Snapshot};
use std::{
    sync::mpsc::{self, Receiver},
    time::{Duration, Instant},
};
use windows::{
    Foundation::{TimeSpan, TypedEventHandler},
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls as Controls,
        SystemMediaTransportControlsButton as Button,
        SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    },
    Win32::{
        Foundation::HWND,
        System::WinRT::{
            ISystemMediaTransportControlsInterop, RO_INIT_MULTITHREADED, RoInitialize,
            RoUninitialize,
        },
        UI::{
            Shell::SetCurrentProcessExplicitAppUserModelID,
            WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_OVERLAPPED},
        },
    },
    core::{HSTRING, Result, factory, w},
};

struct Apartment;
impl Apartment {
    fn new() -> Result<Self> {
        // SAFETY: This test thread is initialized once and balanced by Drop.
        unsafe {
            RoInitialize(RO_INIT_MULTITHREADED)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: All publisher objects are released before this apartment guard.
        unsafe {
            RoUninitialize();
        }
    }
}

struct Publisher {
    controls: Controls,
    token: i64,
    hwnd: HWND,
    commands: Receiver<Button>,
}

impl Publisher {
    fn new() -> Result<Self> {
        // SAFETY: This creates a hidden top-level window owned by this test process.
        // It receives no user input and is destroyed after the SMTC instance is disabled.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                w!("Media Flyout test publisher"),
                WS_OVERLAPPED,
                0,
                0,
                1,
                1,
                None,
                None,
                None,
                None,
            )?
        };
        let interop: ISystemMediaTransportControlsInterop =
            factory::<Controls, ISystemMediaTransportControlsInterop>()?;
        // SAFETY: The HWND is a live top-level window belonging to the calling process.
        let controls: Controls = unsafe { interop.GetForWindow(hwnd)? };
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        let (sender, commands) = mpsc::channel();
        let native = controls.clone();
        let token = controls.ButtonPressed(&TypedEventHandler::<
            Controls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, arguments| {
            let Some(arguments) = arguments.as_ref() else {
                return Ok(());
            };
            let button = arguments.Button()?;
            match button {
                Button::Play => native.SetPlaybackStatus(MediaPlaybackStatus::Playing)?,
                Button::Pause => native.SetPlaybackStatus(MediaPlaybackStatus::Paused)?,
                _ => {}
            }
            let _ = sender.send(button);
            Ok(())
        }))?;
        controls.SetIsEnabled(true)?;
        controls.SetPlaybackStatus(MediaPlaybackStatus::Playing)?;
        let publisher = Self {
            controls,
            token,
            hwnd,
            commands,
        };
        publisher.track("Fixture track α")?;
        publisher.timeline()?;
        Ok(publisher)
    }

    fn track(&self, title: &str) -> Result<()> {
        let updater = self.controls.DisplayUpdater()?;
        updater.SetType(MediaPlaybackType::Music)?;
        let music = updater.MusicProperties()?;
        music.SetTitle(&HSTRING::from(title))?;
        music.SetArtist(&HSTRING::from("Fixture artist"))?;
        music.SetAlbumTitle(&HSTRING::from("Fixture album"))?;
        updater.Update()
    }

    fn timeline(&self) -> Result<()> {
        let timeline = SystemMediaTransportControlsTimelineProperties::new()?;
        timeline.SetStartTime(TimeSpan { Duration: 0 })?;
        timeline.SetEndTime(TimeSpan {
            Duration: 180 * 10_000_000,
        })?;
        timeline.SetPosition(TimeSpan {
            Duration: 30 * 10_000_000,
        })?;
        timeline.SetMinSeekTime(TimeSpan { Duration: 0 })?;
        timeline.SetMaxSeekTime(TimeSpan {
            Duration: 180 * 10_000_000,
        })?;
        self.controls.UpdateTimelineProperties(&timeline)
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = self.controls.RemoveButtonPressed(self.token);
        let _ = self.controls.SetIsEnabled(false);
        // SAFETY: This test owns the hidden window and destroys it exactly once.
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

fn receive(receiver: &Receiver<Snapshot>, predicate: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut description = String::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let snapshot = receiver.recv_timeout(remaining).unwrap_or_else(|error| {
            panic!("Missing media event: {error}; last snapshot: {description}")
        });
        if predicate(&snapshot) {
            return snapshot;
        }
        description = format!("{snapshot:?}");
    }
}

#[test]
#[ignore = "Requires interactive Windows GSMTC services; publishes a silent fixture session"]
fn real_gsmtc_events_controls_metadata_timeline_and_disappearance() -> Result<()> {
    let _apartment = Apartment::new()?;
    // SAFETY: This sets only the test process's app identity, before its hidden HWND.
    unsafe {
        SetCurrentProcessExplicitAppUserModelID(w!("MediaFlyout.TestPublisher"))?;
    }
    let publisher = Publisher::new()?;
    let (sender, receiver) = mpsc::channel();
    let service = MediaService::start(move |snapshot| {
        let _ = sender.send(snapshot);
    })
    .expect("media worker");
    let snapshot = receive(&receiver, |s| {
        s.sessions
            .iter()
            .any(|s| s.app_id.contains("TestPublisher") || s.app_id.contains("media_integration"))
    });
    let id = snapshot
        .sessions
        .iter()
        .find(|s| s.app_id.contains("TestPublisher") || s.app_id.contains("media_integration"))
        .expect("fixture session")
        .id;
    service.select(Some(id));
    let snapshot = receive(&receiver, |s| {
        s.selected == Some(id) && s.track.title == "Fixture track α"
    });
    assert_eq!(snapshot.track.artist, "Fixture artist");
    assert_eq!(snapshot.track.album, "Fixture album");
    assert!(
        snapshot.track.artwork.is_none(),
        "missing artwork is normal"
    );
    assert!(snapshot.controls.play_pause && snapshot.controls.previous && snapshot.controls.next);
    assert_eq!(
        snapshot.timeline.as_ref().expect("timeline").duration,
        Duration::from_secs(180)
    );

    service.control(Action::PlayPause);
    assert_eq!(
        publisher
            .commands
            .recv_timeout(Duration::from_secs(5))
            .expect("pause callback"),
        Button::Pause
    );
    receive(&receiver, |s| {
        s.selected == Some(id) && s.state == PlaybackState::Paused
    });
    service.control(Action::PlayPause);
    assert_eq!(
        publisher
            .commands
            .recv_timeout(Duration::from_secs(5))
            .expect("play callback"),
        Button::Play
    );
    receive(&receiver, |s| {
        s.selected == Some(id) && s.state == PlaybackState::Playing
    });
    service.control(Action::Next);
    assert_eq!(
        publisher
            .commands
            .recv_timeout(Duration::from_secs(5))
            .expect("next callback"),
        Button::Next
    );
    service.control(Action::Previous);
    assert_eq!(
        publisher
            .commands
            .recv_timeout(Duration::from_secs(5))
            .expect("previous callback"),
        Button::Previous
    );

    publisher.track("Fixture changed β")?;
    receive(&receiver, |s| {
        s.selected == Some(id) && s.track.title == "Fixture changed β"
    });
    publisher.controls.SetIsNextEnabled(false)?;
    receive(&receiver, |s| s.selected == Some(id) && !s.controls.next);
    service.control(Action::Next);
    receive(&receiver, |s| s.status.is_some());
    assert!(
        publisher
            .commands
            .recv_timeout(Duration::from_millis(250))
            .is_err(),
        "unsupported controls are not sent"
    );
    let secondary = Publisher::new()?;
    secondary.track("Second window session")?;
    let snapshot = receive(&receiver, |s| {
        s.sessions
            .iter()
            .filter(|s| {
                s.app_id.contains("TestPublisher") || s.app_id.contains("media_integration")
            })
            .count()
            >= 2
    });
    let second_id = snapshot
        .sessions
        .iter()
        .find(|s| {
            s.id != id
                && (s.app_id.contains("TestPublisher") || s.app_id.contains("media_integration"))
        })
        .expect("second session")
        .id;
    service.select(Some(second_id));
    receive(&receiver, |s| {
        s.selected == Some(second_id) && s.track.title == "Second window session"
    });
    service.select(Some(id));
    receive(&receiver, |s| {
        s.selected == Some(id) && s.track.title == "Fixture changed β"
    });
    drop(secondary);
    receive(&receiver, |s| {
        s.selected == Some(id) && s.sessions.iter().all(|s| s.id != second_id)
    });
    drop(publisher);
    receive(&receiver, |s| s.sessions.iter().all(|s| s.id != id));
    drop(service);
    Ok(())
}
