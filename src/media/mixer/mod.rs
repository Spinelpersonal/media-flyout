//! Application volume on render sessions. All COM objects stay on the visible flyout's worker.
mod identity;
#[cfg(test)]
mod tests;

use super::worker::Apartment;
use identity::{Source, take_string};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::{S_FALSE, S_OK},
        Media::Audio::*,
        System::Com::{CLSCTX_ALL, CoCreateInstance},
    },
    core::{Interface, Result},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Volume {
    pub instance: String,
    pub percent: i32,
    pub muted: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum Change {
    Set(i32),
    Step(i32),
    ToggleMute,
}

struct Request {
    instance: String,
    change: Change,
}

struct Session {
    instance: String,
    volume: ISimpleAudioVolume,
}

impl Session {
    fn read(&self) -> Result<Volume> {
        // SAFETY: The interface belongs to a single verified render session in this apartment.
        unsafe {
            Ok(Volume {
                instance: self.instance.clone(),
                percent: (self.volume.GetMasterVolume()? * 100.0).round() as i32,
                muted: self.volume.GetMute()?.as_bool(),
            })
        }
    }

    fn apply(&self, request: &Request) -> Result<()> {
        if self.instance != request.instance {
            return Ok(());
        }
        // SAFETY: ISimpleAudioVolume controls only this session. A null event context is valid.
        unsafe {
            let level = match request.change {
                Change::ToggleMute => {
                    return self
                        .volume
                        .SetMute(!self.volume.GetMute()?.as_bool(), std::ptr::null());
                }
                Change::Set(percent) => percent as f32 / 100.0,
                Change::Step(delta) => self.volume.GetMasterVolume()? + delta as f32 / 100.0,
            };
            self.volume
                .SetMasterVolume(level.clamp(0.0, 1.0), std::ptr::null())
        }
    }
}

fn find_session(source: &Source<'_>) -> Result<Option<Session>> {
    let mut matches = Vec::new();
    // SAFETY: All interfaces are local to an initialized MTA. No capture streams or
    // endpoint-volume interfaces are opened. The vtable call preserves the success
    // status that distinguishes a single owner from AUDCLNT_S_NO_SINGLE_PROCESS.
    unsafe {
        let devices: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let outputs = devices.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for i in 0..outputs.GetCount()? {
            let device = outputs.Item(i)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for j in 0..sessions.GetCount()? {
                let control: IAudioSessionControl2 = sessions.GetSession(j)?.cast()?;
                let state = control.GetState()?;
                if state == AudioSessionStateExpired || control.IsSystemSoundsSession() != S_FALSE {
                    continue;
                }
                let mut pid = 0;
                let status = (control.vtable().GetProcessId)(control.as_raw(), &mut pid);
                if status != S_OK || pid == 0 || !source.owns(pid) {
                    continue;
                }
                matches.push((state == AudioSessionStateActive, control));
            }
        }
        // A stopped stream can remain listed on the previous output after a move.
        let active = matches.iter().any(|(active, _)| *active);
        let mut candidates = matches
            .into_iter()
            .filter(|(running, _)| *running == active);
        let Some((_, control)) = candidates.next() else {
            return Ok(None);
        };
        if candidates.next().is_some() {
            return Ok(None);
        }
        let Some(instance) =
            take_string(control.GetSessionInstanceIdentifier()?).filter(|id| !id.is_empty())
        else {
            return Ok(None);
        };
        Ok(Some(Session {
            instance,
            volume: control.cast()?,
        }))
    }
}

pub(crate) struct Mixer {
    pub selection: (u64, String),
    sender: Option<SyncSender<Request>>,
    stopped: Arc<AtomicBool>,
    latest: Arc<Mutex<Option<Volume>>>,
    thread: Option<JoinHandle<()>>,
}

impl Mixer {
    pub fn start(
        selection: (u64, String),
        notify: impl Fn() + Send + 'static,
    ) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Request>(8);
        let stopped = Arc::new(AtomicBool::new(false));
        let cancelled = stopped.clone();
        let latest = Arc::new(Mutex::new(None));
        let published = latest.clone();
        let id = selection.1.clone();
        let thread = thread::Builder::new()
            .name("application volume".into())
            .spawn(move || {
                let Ok(_apartment) = Apartment::new() else {
                    return;
                };
                let mut session = None;
                let mut refresh = Instant::now();
                let mut request = None;
                while !cancelled.load(Ordering::Acquire) {
                    if request.is_some() || Instant::now() >= refresh {
                        session = find_session(&Source::new(&id)).ok().flatten();
                        refresh = Instant::now() + Duration::from_secs(1);
                    }
                    if let (Some(session), Some(request)) = (&session, request.take())
                        && !cancelled.load(Ordering::Acquire)
                    {
                        let _ = session.apply(&request);
                    }
                    let current = session.as_ref().and_then(|s| s.read().ok());
                    let mut slot = published.lock().unwrap_or_else(|p| p.into_inner());
                    let changed = *slot != current;
                    if changed {
                        *slot = current;
                    }
                    drop(slot);
                    if changed {
                        notify();
                    }
                    let mut wait = refresh.saturating_duration_since(Instant::now());
                    if session.is_some() {
                        wait = wait.min(Duration::from_millis(250));
                    }
                    match receiver.recv_timeout(wait) {
                        Ok(next) => request = Some(next),
                        Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })?;
        Ok(Self {
            selection,
            sender: Some(sender),
            stopped,
            latest,
            thread: Some(thread),
        })
    }

    pub fn state(&self) -> Option<Volume> {
        self.latest
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn change(&self, instance: String, change: Change) {
        if let Some(sender) = &self.sender {
            let _ = sender.try_send(Request { instance, change });
        }
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
