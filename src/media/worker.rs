use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use windows::{
    Foundation::TypedEventHandler,
    Media::Control::{
        GlobalSystemMediaTransportControlsSession as Session,
        GlobalSystemMediaTransportControlsSessionManager as Manager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
    },
    Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize},
    core::{Error, HRESULT, Result, RuntimeType},
};
use windows_future::{AsyncOperationCompletedHandler, IAsyncOperation};

use super::{Controls, PlaybackState, SessionInfo, Snapshot, Track, select_session};

const TOPOLOGY: u8 = 1;
const DATA: u8 = 2;

#[derive(Clone, Copy, Debug)]
pub enum Action {
    PlayPause,
    Previous,
    Next,
}

enum Command {
    Wake,
    Refresh,
    Reconnect,
    Select(Option<u64>),
    Prefer(Option<String>),
    Control(Action),
    Shutdown,
}

#[derive(Clone)]
struct Notifier {
    sender: SyncSender<Command>,
    pending: Arc<AtomicU8>,
}

impl Notifier {
    fn notify(&self, flags: u8) {
        self.pending.fetch_or(flags, Ordering::Release);
        // A full queue already guarantees a wakeup; pending flags are never lost.
        let _ = self.sender.try_send(Command::Wake);
    }
}

pub struct MediaService {
    notifier: Notifier,
    thread: Option<JoinHandle<()>>,
}

impl MediaService {
    pub fn start(on_snapshot: impl Fn(Snapshot) + Send + 'static) -> std::io::Result<Self> {
        Self::with_preference(None, on_snapshot)
    }

    pub fn with_preference(
        preferred: Option<String>,
        on_snapshot: impl Fn(Snapshot) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(16);
        let notifier = Notifier {
            sender,
            pending: Arc::new(AtomicU8::new(0)),
        };
        let worker_notifier = notifier.clone();
        let thread = thread::Builder::new().name("media".into()).spawn(move || {
            if let Err(error) = run(receiver, worker_notifier, preferred, &on_snapshot) {
                on_snapshot(Snapshot {
                    status: Some(format!("Media service unavailable: {error}")),
                    ..Snapshot::default()
                });
            }
        })?;
        Ok(Self {
            notifier,
            thread: Some(thread),
        })
    }

    pub fn control(&self, action: Action) {
        let _ = self.notifier.sender.try_send(Command::Control(action));
    }

    pub fn refresh(&self) {
        let _ = self.notifier.sender.try_send(Command::Refresh);
    }

    pub fn reconnect(&self) {
        let _ = self.notifier.sender.try_send(Command::Reconnect);
    }

    pub fn select(&self, id: Option<u64>) {
        let _ = self.notifier.sender.try_send(Command::Select(id));
    }
    pub fn prefer(&self, app: Option<String>) {
        let _ = self.notifier.sender.try_send(Command::Prefer(app));
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        let _ = self.notifier.sender.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// WinRT waits are bounded and run only on the media worker, never on the UI thread.
pub(super) fn wait<T: RuntimeType + 'static>(operation: IAsyncOperation<T>) -> Result<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    operation.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
        let _ = sender.try_send(());
        Ok(())
    }))?;
    if receiver.recv_timeout(Duration::from_secs(3)).is_err() {
        let _ = operation.Cancel();
        return Err(Error::new(
            HRESULT(0x800705B4u32 as i32),
            "Media operation timed out",
        ));
    }
    operation.GetResults()
}

struct Apartment;

impl Apartment {
    fn new() -> Result<Self> {
        // SAFETY: Called on a dedicated new worker thread. Drop balances initialization
        // on that same thread after every WinRT object has been released.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        Ok(Self)
    }
}

impl Drop for Apartment {
    fn drop(&mut self) {
        // SAFETY: Matches the successful RoInitialize on this thread.
        unsafe { RoUninitialize() };
    }
}

struct ManagerEvents {
    manager: Manager,
    sessions: Option<i64>,
    current: Option<i64>,
}

impl ManagerEvents {
    fn new(notifier: &Notifier) -> Result<Self> {
        let manager = wait(Manager::RequestAsync()?)?;
        let mut events = Self {
            manager,
            sessions: None,
            current: None,
        };
        let notify = notifier.clone();
        events.sessions = Some(events.manager.SessionsChanged(&TypedEventHandler::new(
            move |_, _| {
                notify.notify(TOPOLOGY);
                Ok(())
            },
        ))?);
        let notify = notifier.clone();
        events.current = Some(
            events
                .manager
                .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                    notify.notify(DATA);
                    Ok(())
                }))?,
        );
        Ok(events)
    }
}

impl Drop for ManagerEvents {
    fn drop(&mut self) {
        if let Some(token) = self.sessions {
            let _ = self.manager.RemoveSessionsChanged(token);
        }
        if let Some(token) = self.current {
            let _ = self.manager.RemoveCurrentSessionChanged(token);
        }
    }
}

struct WatchedSession {
    native: Session,
    id: u64,
    app_id: String,
    metadata_token: Option<i64>,
    playback_token: Option<i64>,
    timeline_token: Option<i64>,
    dirty: Arc<AtomicBool>,
    track: Track,
}

impl WatchedSession {
    fn new(native: Session, id: u64, notifier: &Notifier) -> Result<Self> {
        let app_id = native.SourceAppUserModelId()?.to_string_lossy();
        let mut session = Self {
            native,
            id,
            app_id,
            metadata_token: None,
            playback_token: None,
            timeline_token: None,
            dirty: Arc::new(AtomicBool::new(true)),
            track: Track::default(),
        };
        let notify = notifier.clone();
        let dirty = session.dirty.clone();
        session.metadata_token = Some(session.native.MediaPropertiesChanged(
            &TypedEventHandler::new(move |_, _| {
                dirty.store(true, Ordering::Release);
                notify.notify(DATA);
                Ok(())
            }),
        )?);
        let notify = notifier.clone();
        session.playback_token = Some(session.native.PlaybackInfoChanged(
            &TypedEventHandler::new(move |_, _| {
                notify.notify(DATA);
                Ok(())
            }),
        )?);
        let notify = notifier.clone();
        session.timeline_token = Some(session.native.TimelinePropertiesChanged(
            &TypedEventHandler::new(move |_, _| {
                notify.notify(DATA);
                Ok(())
            }),
        )?);
        Ok(session)
    }

    fn state(&self) -> PlaybackState {
        match self
            .native
            .GetPlaybackInfo()
            .and_then(|info| info.PlaybackStatus())
        {
            Ok(Status::Playing) => PlaybackState::Playing,
            Ok(Status::Paused) => PlaybackState::Paused,
            _ => PlaybackState::Stopped,
        }
    }

    fn read_track(&mut self) -> Result<()> {
        let properties = wait(self.native.TryGetMediaPropertiesAsync()?)?;
        self.track = Track {
            title: properties
                .Title()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default(),
            artist: properties
                .Artist()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default(),
            album: properties
                .AlbumTitle()
                .map(|s| s.to_string_lossy())
                .unwrap_or_default(),
            artwork: properties
                .Thumbnail()
                .ok()
                .and_then(|reference| super::artwork::load(reference).ok().flatten()),
        };
        Ok(())
    }

    fn controls(&self) -> Controls {
        let Ok(controls) = self
            .native
            .GetPlaybackInfo()
            .and_then(|info| info.Controls())
        else {
            return Controls::default();
        };
        let playing = self.state() == PlaybackState::Playing;
        Controls {
            play_pause: controls.IsPlayPauseToggleEnabled().unwrap_or(false)
                || if playing {
                    controls.IsPauseEnabled().unwrap_or(false)
                } else {
                    controls.IsPlayEnabled().unwrap_or(false)
                },
            previous: controls.IsPreviousEnabled().unwrap_or(false),
            next: controls.IsNextEnabled().unwrap_or(false),
        }
    }

    fn control(&self, action: Action) -> Result<bool> {
        let controls = self.native.GetPlaybackInfo()?.Controls()?;
        let operation = match action {
            Action::Previous if controls.IsPreviousEnabled()? => {
                self.native.TrySkipPreviousAsync()?
            }
            Action::Next if controls.IsNextEnabled()? => self.native.TrySkipNextAsync()?,
            Action::PlayPause if controls.IsPlayPauseToggleEnabled()? => {
                self.native.TryTogglePlayPauseAsync()?
            }
            Action::PlayPause
                if self.state() == PlaybackState::Playing && controls.IsPauseEnabled()? =>
            {
                self.native.TryPauseAsync()?
            }
            Action::PlayPause if controls.IsPlayEnabled()? => self.native.TryPlayAsync()?,
            _ => return Ok(false),
        };
        wait(operation)
    }
}

impl Drop for WatchedSession {
    fn drop(&mut self) {
        if let Some(token) = self.metadata_token {
            let _ = self.native.RemoveMediaPropertiesChanged(token);
        }
        if let Some(token) = self.playback_token {
            let _ = self.native.RemovePlaybackInfoChanged(token);
        }
        if let Some(token) = self.timeline_token {
            let _ = self.native.RemoveTimelinePropertiesChanged(token);
        }
    }
}

fn synchronize(
    manager: &Manager,
    sessions: &mut Vec<WatchedSession>,
    next_id: &mut u64,
    notifier: &Notifier,
) -> Result<()> {
    let native_sessions: Vec<_> = manager.GetSessions()?.into_iter().collect();
    sessions.retain(|s| native_sessions.contains(&s.native));
    for native in native_sessions {
        if !sessions.iter().any(|s| s.native == native) {
            *next_id += 1;
            if let Ok(session) = WatchedSession::new(native, *next_id, notifier) {
                sessions.push(session);
            }
        }
    }
    Ok(())
}

fn snapshot(
    manager: &Manager,
    sessions: &mut [WatchedSession],
    status: Option<String>,
    preferred: Option<&str>,
    manual: Option<u64>,
) -> Snapshot {
    let mut snapshot = Snapshot {
        status,
        ..Snapshot::default()
    };
    snapshot.sessions = sessions
        .iter()
        .map(|s| SessionInfo {
            id: s.id,
            app_id: s.app_id.clone(),
            state: s.state(),
        })
        .collect();
    let current = manager
        .GetCurrentSession()
        .ok()
        .and_then(|native| sessions.iter().find(|s| s.native == native).map(|s| s.id));
    if let Some(index) = select_session(&snapshot.sessions, current, preferred, manual) {
        let session = &mut sessions[index];
        snapshot.selected = Some(session.id);
        snapshot.manual = manual == Some(session.id);
        snapshot.state = snapshot.sessions[index].state;
        snapshot.controls = session.controls();
        if session.dirty.swap(false, Ordering::AcqRel)
            && let Err(error) = session.read_track()
        {
            session.track = Track::default();
            session.dirty.store(true, Ordering::Release);
            snapshot.status = Some(format!("Metadata unavailable: {error}"));
        }
        snapshot.track = session.track.clone();
    }
    for session in sessions {
        if Some(session.id) != snapshot.selected && session.track.artwork.take().is_some() {
            session.dirty.store(true, Ordering::Release);
        }
    }
    snapshot
}

fn run(
    receiver: Receiver<Command>,
    notifier: Notifier,
    mut preferred: Option<String>,
    on_snapshot: &impl Fn(Snapshot),
) -> Result<()> {
    let _apartment = Apartment::new()?;
    let mut events = ManagerEvents::new(&notifier).ok();
    let mut sessions = Vec::new();
    let mut next_id = 0;
    let mut manual = None;
    let mut last = if let Some(events) = &events {
        match synchronize(&events.manager, &mut sessions, &mut next_id, &notifier) {
            Ok(()) => snapshot(
                &events.manager,
                &mut sessions,
                None,
                preferred.as_deref(),
                manual,
            ),
            Err(error) => Snapshot {
                status: Some(format!("Sessions unavailable: {error}")),
                ..Snapshot::default()
            },
        }
    } else {
        Snapshot {
            status: Some(
                "Windows media service is unavailable. Reopen the flyout to retry.".into(),
            ),
            ..Snapshot::default()
        }
    };
    on_snapshot(last.clone());
    while let Ok(command) = receiver.recv() {
        let mut status = None;
        match command {
            Command::Shutdown => break,
            Command::Wake => {}
            Command::Select(id) => {
                manual = id.filter(|id| sessions.iter().any(|s| s.id == *id));
            }
            Command::Prefer(app) => {
                preferred = app;
            }
            Command::Refresh => {
                if events.is_none() {
                    events = ManagerEvents::new(&notifier).ok();
                }
                notifier.pending.fetch_or(TOPOLOGY, Ordering::Release);
            }
            Command::Reconnect => {
                sessions.clear();
                drop(events.take());
                events = ManagerEvents::new(&notifier).ok();
                notifier.pending.fetch_or(TOPOLOGY, Ordering::Release);
            }
            Command::Control(action) => {
                if let Some(session) = sessions.iter().find(|s| Some(s.id) == last.selected) {
                    status = match session.control(action) {
                        Ok(true) => None,
                        Ok(false) => Some("This player did not accept the control".into()),
                        Err(error) => Some(format!("Player unavailable: {error}")),
                    };
                }
            }
        }
        let flags = notifier.pending.swap(0, Ordering::AcqRel);
        let Some(events) = &events else {
            on_snapshot(last.clone());
            continue;
        };
        if flags & TOPOLOGY != 0
            && let Err(error) = synchronize(&events.manager, &mut sessions, &mut next_id, &notifier)
        {
            sessions.clear();
            status = Some(format!("Sessions unavailable: {error}"));
        }
        last = snapshot(
            &events.manager,
            &mut sessions,
            status,
            preferred.as_deref(),
            manual,
        );
        on_snapshot(last.clone());
    }
    Ok(())
}
