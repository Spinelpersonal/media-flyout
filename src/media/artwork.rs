use std::{
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use windows::{
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat,
        BitmapTransform, ColorManagementMode, ExifOrientationMode,
    },
    Storage::Streams::IRandomAccessStreamReference,
    core::{AgileReference, Result},
};

use super::{
    Artwork,
    worker::{Apartment, wait_for},
};

pub(super) struct Job {
    pub session_id: u64,
    pub revision: u64,
    pub reference: AgileReference<IRandomAccessStreamReference>,
}
pub(super) struct Decoded {
    pub session_id: u64,
    pub revision: u64,
    pub image: Option<Arc<Artwork>>,
}

#[derive(Default)]
struct Queue {
    job: Option<Job>,
    stopping: bool,
}

/// A single sleeping decoder with one replaceable pending job. Slow thumbnail
/// providers cannot delay media controls, and old track images cannot overwrite new ones.
pub(super) struct Worker {
    shared: Arc<(Mutex<Queue>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(on_decoded: impl Fn(Decoded) + Send + 'static) -> std::io::Result<Self> {
        let shared = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let queue = shared.clone();
        let thread = thread::Builder::new()
            .name("artwork".into())
            .spawn(move || {
                let Ok(_apartment) = Apartment::new() else {
                    return;
                };
                loop {
                    let job = {
                        let (lock, ready) = &*queue;
                        let mut state = lock.lock().unwrap_or_else(|p| p.into_inner());
                        while state.job.is_none() && !state.stopping {
                            state = ready.wait(state).unwrap_or_else(|p| p.into_inner());
                        }
                        if state.stopping {
                            break;
                        }
                        state.job.take()
                    };
                    if let Some(job) = job {
                        let image = job
                            .reference
                            .resolve()
                            .ok()
                            .and_then(|reference| load(reference).ok().flatten());
                        if !queue.0.lock().unwrap_or_else(|p| p.into_inner()).stopping {
                            on_decoded(Decoded {
                                session_id: job.session_id,
                                revision: job.revision,
                                image,
                            });
                        }
                    }
                }
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn request(&self, job: Job) {
        self.shared.0.lock().unwrap_or_else(|p| p.into_inner()).job = Some(job);
        self.shared.1.notify_one();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        {
            let mut state = self.shared.0.lock().unwrap_or_else(|p| p.into_inner());
            state.stopping = true;
            state.job = None;
        }
        self.shared.1.notify_one();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Decode through Windows Imaging Component and scale before transferring pixels.
/// Only the selected session is decoded; the retained image is at most 256 KiB.
pub(super) fn load(reference: IRandomAccessStreamReference) -> Result<Option<Arc<Artwork>>> {
    let deadline = Instant::now() + Duration::from_secs(3);
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let stream = wait_for(reference.OpenReadAsync()?, remaining())?;
    if stream.Size()? > 8 * 1024 * 1024 {
        return Ok(None);
    }
    let decoder = wait_for(BitmapDecoder::CreateAsync(&stream)?, remaining())?;
    let (width, height) = (decoder.PixelWidth()?, decoder.PixelHeight()?);
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || u64::from(width) * u64::from(height) > 16_000_000
    {
        return Ok(None);
    }
    let ratio = (256.0 / f64::from(width.max(height))).min(1.0);
    let width = (f64::from(width) * ratio).round().max(1.0) as u32;
    let height = (f64::from(height) * ratio).round().max(1.0) as u32;
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(width)?;
    transform.SetScaledHeight(height)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?;
    let data = wait_for(
        decoder.GetPixelDataTransformedAsync(
            BitmapPixelFormat::Rgba8,
            BitmapAlphaMode::Straight,
            &transform,
            ExifOrientationMode::IgnoreExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?,
        remaining(),
    )?;
    let rgba = data.DetachPixelData()?.to_vec();
    if rgba.len() != width as usize * height as usize * 4 {
        return Ok(None);
    }
    Ok(Some(Arc::new(Artwork {
        width,
        height,
        rgba,
    })))
}
