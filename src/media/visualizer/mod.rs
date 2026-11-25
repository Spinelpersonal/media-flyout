//! Default render-endpoint loopback. All WASAPI interfaces stay on the capture thread.
mod spectrum;

pub(crate) use self::spectrum::BARS;
use self::spectrum::{Levels, Spectrum};
use super::worker::Apartment;
use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use windows::{
    Win32::{
        Foundation::E_INVALIDARG,
        Media::{
            Audio::*,
            KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE},
            Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT},
        },
        System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree},
    },
    core::Result,
};

const FRAME: Duration = Duration::from_millis(33);
const RETRY: Duration = Duration::from_secs(2);

pub(crate) struct Visualizer {
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
    latest: Arc<Mutex<Option<Levels>>>,
}

impl Visualizer {
    pub(crate) fn start(notify: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (stop, receiver) = mpsc::channel();
        let latest = Arc::new(Mutex::new(None));
        let slot = latest.clone();
        let worker = thread::Builder::new()
            .name("audio spectrum".into())
            .spawn(move || {
                let mut previous = [0; BARS];
                let mut publish = |levels| {
                    if levels != previous {
                        previous = levels;
                        let pending = slot
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .replace(levels);
                        if pending.is_none() {
                            notify();
                        }
                    }
                };
                while capture(&receiver, &mut publish).is_err() {
                    publish([0; BARS]);
                    if stopped(&receiver, RETRY) {
                        break;
                    }
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
            latest,
        })
    }

    pub(crate) fn take_levels(&self) -> Option<Levels> {
        self.latest.lock().unwrap_or_else(|p| p.into_inner()).take()
    }
}

impl Drop for Visualizer {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn stopped(stop: &Receiver<()>, timeout: Duration) -> bool {
    stop.recv_timeout(timeout) != Err(RecvTimeoutError::Timeout)
}

fn capture(stop: &Receiver<()>, publish: &mut impl FnMut(Levels)) -> Result<()> {
    let _apartment = Apartment::new()?;
    // SAFETY: COM is initialized on this worker and all interfaces drop before its apartment.
    let devices: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    loop {
        // SAFETY: devices belongs to this apartment. Loopback requires a render endpoint.
        let device = unsafe { devices.GetDefaultAudioEndpoint(eRender, eMultimedia)? };
        let id = endpoint_id(&device)?;
        let (stream, mut spectrum) = Stream::open(&device)?;
        let mut checked = Instant::now();
        loop {
            if stopped(stop, FRAME) {
                return Ok(());
            }
            if checked.elapsed() >= Duration::from_secs(1) {
                // SAFETY: Same initialized apartment and render role used to open this stream.
                let default = unsafe { devices.GetDefaultAudioEndpoint(eRender, eMultimedia)? };
                if endpoint_id(&default)? != id {
                    break;
                }
                checked = Instant::now();
            }
            stream.read(&mut spectrum)?;
            publish(spectrum.levels());
        }
        publish([0; BARS]);
    }
}

fn endpoint_id(device: &IMMDevice) -> Result<Vec<u16>> {
    // SAFETY: GetId transfers one null-terminated COM allocation to us.
    unsafe {
        let id = device.GetId()?;
        let owned = id.as_wide().to_vec();
        CoTaskMemFree(Some(id.0.cast()));
        Ok(owned)
    }
}

#[derive(Clone, Copy)]
enum Sample {
    Float,
    Pcm,
}

struct Format {
    sample: Sample,
    width: usize,
    stride: usize,
    rate: u32,
}

impl Format {
    /// The pointer must be a live format allocation returned by GetMixFormat.
    unsafe fn from_mix(ptr: *const WAVEFORMATEX) -> Result<Self> {
        // SAFETY: Caller supplies a complete base descriptor and its cbSize extension.
        let base = unsafe { *ptr };
        let tag = match u32::from(base.wFormatTag) {
            WAVE_FORMAT_EXTENSIBLE if base.cbSize >= 22 => {
                // SAFETY: The extension is at least the 22 bytes required for SubFormat.
                let sub = unsafe { (*ptr.cast::<WAVEFORMATEXTENSIBLE>()).SubFormat };
                if sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT {
                    WAVE_FORMAT_IEEE_FLOAT
                } else if sub == KSDATAFORMAT_SUBTYPE_PCM {
                    WAVE_FORMAT_PCM
                } else {
                    return Err(E_INVALIDARG.into());
                }
            }
            tag => tag,
        };
        let sample = match (tag, base.wBitsPerSample) {
            (WAVE_FORMAT_IEEE_FLOAT, 32) => Sample::Float,
            (WAVE_FORMAT_PCM, 8 | 16 | 24 | 32) => Sample::Pcm,
            _ => return Err(E_INVALIDARG.into()),
        };
        let width = usize::from(base.wBitsPerSample / 8);
        let stride = usize::from(base.nBlockAlign);
        let rate = base.nSamplesPerSec;
        if stride == 0
            || stride != width * usize::from(base.nChannels)
            || !(8000..=768_000).contains(&rate)
        {
            return Err(E_INVALIDARG.into());
        }
        Ok(Self {
            sample,
            width,
            stride,
            rate,
        })
    }

    fn mono(&self, frame: &[u8]) -> f32 {
        let mut sum = 0.0;
        for channel in frame.chunks_exact(self.width) {
            sum += match (self.sample, channel) {
                (Sample::Float, &[a, b, c, d]) => f32::from_le_bytes([a, b, c, d]),
                (Sample::Pcm, &[a]) => (f32::from(a) - 128.0) / 128.0,
                (Sample::Pcm, &[a, b]) => f32::from(i16::from_le_bytes([a, b])) / 32768.0,
                (Sample::Pcm, &[a, b, c]) => i32::from_le_bytes([0, a, b, c]) as f32 / 2147483648.0,
                (Sample::Pcm, &[a, b, c, d]) => {
                    i32::from_le_bytes([a, b, c, d]) as f32 / 2147483648.0
                }
                _ => unreachable!("validated mix format"),
            };
        }
        sum / (self.stride / self.width) as f32
    }
}

struct Stream {
    packets: IAudioCaptureClient,
    client: IAudioClient,
    format: Format,
}

impl Stream {
    fn open(device: &IMMDevice) -> Result<(Self, Spectrum)> {
        // SAFETY: Device and client are local to this apartment. Initialize copies the mix
        // format, whose COM allocation is freed on both successful and failed initialization.
        unsafe {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let ptr = client.GetMixFormat()?;
            let format = Format::from_mix(ptr).and_then(|format| {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    AUDCLNT_STREAMFLAGS_LOOPBACK,
                    1_000_000,
                    0,
                    ptr,
                    None,
                )?;
                Ok(format)
            });
            CoTaskMemFree(Some(ptr.cast()));
            let format = format?;
            let spectrum = Spectrum::new(format.rate);
            let packets = client.GetService()?;
            client.Start()?;
            Ok((
                Self {
                    packets,
                    client,
                    format,
                },
                spectrum,
            ))
        }
    }

    fn read(&self, spectrum: &mut Spectrum) -> Result<()> {
        // SAFETY: Successful GetBuffer exposes frames * block alignment bytes until the
        // matching ReleaseBuffer on this thread. Silent packets may have a null pointer.
        unsafe {
            while self.packets.GetNextPacketSize()? != 0 {
                let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0, 0);
                self.packets
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                    spectrum.clear_samples();
                }
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    for _ in 0..frames {
                        spectrum.push(0.0);
                    }
                } else {
                    let bytes =
                        std::slice::from_raw_parts(data, frames as usize * self.format.stride);
                    for frame in bytes.chunks_exact(self.format.stride) {
                        spectrum.push(self.format.mono(frame));
                    }
                }
                self.packets.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        // SAFETY: This stream was started on the same worker. Invalidation can make Stop fail.
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_endpoint_formats_and_decodes_interleaved_samples() {
        let mut mix = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE as u16,
                cbSize: 22,
                nChannels: 2,
                nSamplesPerSec: 48_000,
                wBitsPerSample: 32,
                nBlockAlign: 8,
                ..Default::default()
            },
            SubFormat: KSDATAFORMAT_SUBTYPE_IEEE_FLOAT,
            ..Default::default()
        };
        // SAFETY: The test owns the complete packed mix descriptor for every call.
        unsafe {
            let float = Format::from_mix(&raw const mix.Format).unwrap();
            assert_eq!(float.mono(&[0, 0, 128, 63, 0, 0, 0, 191]), 0.25);
            mix.SubFormat = KSDATAFORMAT_SUBTYPE_PCM;
            mix.Format.wBitsPerSample = 24;
            mix.Format.nBlockAlign = 6;
            let pcm = Format::from_mix(&raw const mix.Format).unwrap();
            assert_eq!(pcm.mono(&[0, 0, 128, 0, 0, 64]), -0.25);
            mix.Format.nBlockAlign = 8;
            assert!(Format::from_mix(&raw const mix.Format).is_err());
            mix.Format.nBlockAlign = 6;
            mix.Format.cbSize = 0;
            assert!(Format::from_mix(&raw const mix.Format).is_err());
        }
    }
}
