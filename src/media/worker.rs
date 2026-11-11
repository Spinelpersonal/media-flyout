use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
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

use super::{Controls, PlaybackState, SessionInfo, Snapshot, Timeline, Track, select_session};

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
    Seek {
        session_id: u64,
        revision: u64,
        fraction: f64,
    },
    Shutdown,
}

#[derive(Clone)]
struct Notifier {
    sender: SyncSender<Command>,
    pending: Arc<AtomicU8>,
}

impl Notifier {
    fn notify(&self, flags: u8) {
        let previous = self.pending.fetch_or(flags, Ordering::Release);
        // A full queue already guarantees a wakeup; pending flags are never lost.
        if previous == 0 {
            let _ = self.sender.try_send(Command::Wake);
        }
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

    pub fn seek(&self, session_id: u64, revision: u64, fraction: f64) {
        let _ = self.notifier.sender.try_send(Command::Seek {
            session_id,
            revision,
            fraction,
        });
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
    wait_for(operation, Duration::from_secs(3))
}

pub(super) fn wait_for<T: RuntimeType + 'static>(
    operation: IAsyncOperation<T>,
    timeout: Duration,
) -> Result<T> {
    let (sender, receiver) = mpsc::sync_channel(1);
    operation.SetCompleted(&AsyncOperationCompletedHandler::new(move |_, _| {
        let _ = sender.try_send(());
        Ok(())
    }))?;
    if receiver.recv_timeout(timeout).is_err() {
        let _ = operation.Cancel();
        return Err(Error::new(
            HRESULT(0x800705B4u32 as i32),
            "Media operation timed out",
        ));
    }
    operation.GetResults()
}

pub(super) struct Apartment(std::marker::PhantomData<std::rc::Rc<()>>);

impl Apartment {
    pub(super) fn new() -> Result<Self> {
        // SAFETY: Called on a dedicated new worker thread. Drop balances initialization
        // on that same thread after every WinRT object has been released.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        Ok(Self(std::marker::PhantomData))
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
    thumbnail: Option<windows::Storage::Streams::IRandomAccessStreamReference>,
    revision: u64,
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
            thumbnail: None,
            revision: 0,
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
        let mut track = Track {
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
            artwork: None,
        };
        if track.title == self.track.title
            && track.artist == self.track.artist
            && track.album == self.track.album
        {
            track.artwork = self.track.artwork.clone();
        }
        self.track = track;
        self.thumbnail = properties.Thumbnail().ok();
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
            seek: controls.IsPlaybackPositionEnabled().unwrap_or(false),
        }
    }

    fn timeline(&self) -> Option<Timeline> {
        let timeline = self.native.GetTimelineProperties().ok()?;
        let playback = self.native.GetPlaybackInfo().ok()?;
        let rate = playback
            .PlaybackRate()
            .and_then(|r| r.Value())
            .unwrap_or(1.0);
        let updated = timeline.LastUpdatedTime().ok()?.UniversalTime;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos()
            / 100
            + 116_444_736_000_000_000;
        let age = i128::try_from(now).ok()?.checked_sub(i128::from(updated))?;
        let reliable_clock = (0..=864_000_000_000).contains(&age);
        Some(
            Timeline::from_ticks(
                timeline.StartTime().ok()?.Duration,
                timeline.EndTime().ok()?.Duration,
                timeline.Position().ok()?.Duration,
                rate,
                self.state() == PlaybackState::Playing && reliable_clock,
                if reliable_clock {
                    Duration::from_nanos(age as u64 * 100)
                } else {
                    Duration::ZERO
                },
            )?
            .with_seek_range(
                timeline.MinSeekTime().ok()?.Duration,
                timeline.MaxSeekTime().ok()?.Duration,
            ),
        )
    }

    fn seek(&self, fraction: f64) -> Result<bool> {
        if !self
            .native
            .GetPlaybackInfo()?
            .Controls()?
            .IsPlaybackPositionEnabled()?
        {
            return Ok(false);
        }
        let Some(position) = self
            .timeline()
            .and_then(|timeline| timeline.seek_ticks(fraction))
        else {
            return Ok(false);
        };
        wait(self.native.TryChangePlaybackPositionAsync(position)?)
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
            Action::PlayPause
                if self.state() != PlaybackState::Playing && controls.IsPlayEnabled()? =>
            {
                self.native.TryPlayAsync()?
            }
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
    let native: Vec<_> = manager
        .GetSessions()?
        .into_iter()
        .filter_map(|session| {
            session
                .SourceAppUserModelId()
                .ok()
                .map(|app| (session, app.to_string_lossy()))
        })
        .collect();
    let mut plan = vec![None; native.len()];
    let mut used = vec![false; sessions.len()];
    // Windows may replace wrappers for still-open sessions. Reconcile identity,
    // unique application IDs, then metadata before falling back to source order.
    for (i, (session, _)) in native.iter().enumerate() {
        if let Some(j) = sessions
            .iter()
            .enumerate()
            .position(|(j, old)| !used[j] && old.native == *session)
        {
            plan[i] = Some(j);
            used[j] = true;
        }
    }
    for (i, (_, app)) in native.iter().enumerate() {
        if plan[i].is_some() {
            continue;
        }
        let candidates: Vec<_> = sessions
            .iter()
            .enumerate()
            .filter(|(j, old)| !used[*j] && old.app_id == *app)
            .map(|(j, _)| j)
            .collect();
        if candidates.len() == 1 && native.iter().filter(|(_, other)| other == app).count() == 1 {
            plan[i] = Some(candidates[0]);
            used[candidates[0]] = true;
        }
    }
    let mut old_keys = vec![None; sessions.len()];
    for (i, (session, app)) in native.iter().enumerate() {
        if plan[i].is_some()
            || !sessions
                .iter()
                .enumerate()
                .any(|(j, old)| !used[j] && old.app_id == *app)
        {
            continue;
        }
        let Some(key) = metadata_key(session) else {
            continue;
        };
        for (j, old) in sessions.iter().enumerate() {
            if used[j] || old.app_id != *app {
                continue;
            }
            let old_key = old_keys[j].get_or_insert_with(|| {
                metadata_key(&old.native).unwrap_or_else(|| {
                    (
                        old.track.title.clone(),
                        old.track.artist.clone(),
                        old.track.album.clone(),
                    )
                })
            });
            if *old_key == key {
                plan[i] = Some(j);
                used[j] = true;
                break;
            }
        }
    }
    // There is no public persistent ID for indistinguishable sessions.
    for (i, (_, app)) in native.iter().enumerate() {
        if plan[i].is_none()
            && let Some(j) = sessions
                .iter()
                .enumerate()
                .position(|(j, old)| !used[j] && old.app_id == *app)
        {
            plan[i] = Some(j);
            used[j] = true;
        }
    }
    let mut old: Vec<_> = std::mem::take(sessions).into_iter().map(Some).collect();
    for (i, (native, _)) in native.into_iter().enumerate() {
        let existing = plan[i].and_then(|j| old[j].take());
        if existing.as_ref().is_some_and(|old| old.native == native) {
            if let Some(existing) = existing {
                sessions.push(existing);
            }
            continue;
        }
        let id = existing.as_ref().map(|s| s.id).unwrap_or_else(|| {
            *next_id += 1;
            *next_id
        });
        if let Ok(mut session) = WatchedSession::new(native, id, notifier) {
            session.revision = existing.as_ref().map_or(0, |s| s.revision.wrapping_add(1));
            if let Some(existing) = &existing {
                session.track = existing.track.clone();
            }
            sessions.push(session);
        }
    }
    Ok(())
}

fn metadata_key(session: &Session) -> Option<(String, String, String)> {
    let properties = wait(session.TryGetMediaPropertiesAsync().ok()?).ok()?;
    Some((
        properties.Title().ok()?.to_string_lossy(),
        properties.Artist().ok()?.to_string_lossy(),
        properties.AlbumTitle().ok()?.to_string_lossy(),
    ))
}

fn current_session(manager: &Manager, sessions: &[WatchedSession]) -> Option<u64> {
    let native = manager.GetCurrentSession().ok()?;
    if let Some(session) = sessions.iter().find(|s| s.native == native) {
        return Some(session.id);
    }
    let app = native.SourceAppUserModelId().ok()?.to_string_lossy();
    let mut matching = sessions.iter().filter(|s| s.app_id == app);
    let first = matching.next()?;
    if matching.next().is_none() {
        return Some(first.id);
    }
    let key = metadata_key(&native)?;
    sessions
        .iter()
        .filter(|s| s.app_id == app)
        .find(|s| metadata_key(&s.native).as_ref() == Some(&key))
        .map(|s| s.id)
}

fn snapshot(
    manager: &Manager,
    sessions: &mut [WatchedSession],
    status: Option<String>,
    preferred: Option<&str>,
    manual: Option<u64>,
    artwork: Option<&super::artwork::Worker>,
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
    let current = current_session(manager, sessions);
    if let Some(index) = select_session(&snapshot.sessions, current, preferred, manual) {
        let session = &mut sessions[index];
        snapshot.selected = Some(session.id);
        snapshot.manual = manual == Some(session.id);
        snapshot.state = snapshot.sessions[index].state;
        snapshot.controls = session.controls();
        snapshot.timeline = session.timeline();
        snapshot.controls.seek &= snapshot
            .timeline
            .as_ref()
            .is_some_and(|t| t.seek_ticks(0.0).is_some());
        if session.dirty.swap(false, Ordering::AcqRel) {
            session.revision = session.revision.wrapping_add(1);
            if let Err(error) = session.read_track() {
                session.track = Track::default();
                session.thumbnail = None;
                session.dirty.store(true, Ordering::Release);
                snapshot.status = Some(format!("Metadata unavailable: {error}"));
            }
        }
        snapshot.track = session.track.clone();
        snapshot.track_revision = session.revision;
        if let (Some(worker), Some(reference)) = (artwork, session.thumbnail.take())
            && let Ok(reference) = windows::core::AgileReference::new(&reference)
        {
            worker.request(super::artwork::Job {
                session_id: session.id,
                revision: session.revision,
                reference,
            });
        }
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
    let decoded = Arc::new(Mutex::new(None::<super::artwork::Decoded>));
    let result_slot = decoded.clone();
    let artwork_notify = notifier.clone();
    let artwork = super::artwork::Worker::start(move |result| {
        *result_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(result);
        artwork_notify.notify(DATA);
    })
    .ok();
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
                artwork.as_ref(),
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
                for session in &sessions {
                    session.dirty.store(true, Ordering::Release);
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
                } else {
                    status = Some("No media session is available".into());
                }
            }
            Command::Seek {
                session_id,
                revision,
                fraction,
            } => {
                // A drag belongs to the session and track visible when it began.
                // Ignore it if selection or metadata changed before dispatch.
                if let Some(session) = sessions.iter().find(|s| {
                    s.id == session_id
                        && Some(s.id) == last.selected
                        && s.revision == revision
                        && !s.dirty.load(Ordering::Acquire)
                }) {
                    status = match session.seek(fraction) {
                        Ok(true) => None,
                        Ok(false) => Some("This player did not accept seeking".into()),
                        Err(error) => Some(format!("Player unavailable: {error}")),
                    };
                }
            }
        }
        let flags = notifier.pending.swap(0, Ordering::AcqRel);
        if flags & TOPOLOGY != 0 {
            let result = events
                .as_ref()
                .map(|events| synchronize(&events.manager, &mut sessions, &mut next_id, &notifier));
            if let Some(Err(error)) = result {
                sessions.clear();
                drop(events.take());
                last = Snapshot {
                    status: Some(format!(
                        "Sessions unavailable: {error}. Reopen to reconnect."
                    )),
                    ..Snapshot::default()
                };
                on_snapshot(last.clone());
                continue;
            }
        }
        let Some(events) = &events else {
            on_snapshot(last.clone());
            continue;
        };
        if let Some(result) = decoded.lock().unwrap_or_else(|p| p.into_inner()).take()
            && let Some(session) = sessions
                .iter_mut()
                .find(|s| s.id == result.session_id && s.revision == result.revision)
        {
            session.track.artwork = result.image;
        }
        last = snapshot(
            &events.manager,
            &mut sessions,
            status,
            preferred.as_deref(),
            manual,
            artwork.as_ref(),
        );
        on_snapshot(last.clone());
    }
    Ok(())
}
