use std::f32::consts::TAU;

pub(crate) const BARS: usize = 24;
const SAMPLES: usize = 2048;
pub(super) type Levels = [u8; BARS];

pub(super) struct Spectrum {
    history: [f32; SAMPLES],
    cursor: usize,
    window: [f32; SAMPLES],
    edges: [usize; BARS + 1],
    envelope: [f32; BARS],
    has_signal: bool,
}

impl Spectrum {
    pub(super) fn new(sample_rate: u32) -> Self {
        let bin_hz = sample_rate as f32 / SAMPLES as f32;
        let high = (sample_rate as f32 * 0.5).min(18_000.0);
        let mut last = 0;
        let edges = std::array::from_fn(|i| {
            let hz = 40.0 * (high / 40.0).powf(i as f32 / BARS as f32);
            last = ((hz / bin_hz).round() as usize).clamp(last + 1, SAMPLES / 2 - (BARS - i));
            last
        });
        Self {
            history: [0.0; SAMPLES],
            cursor: 0,
            window: std::array::from_fn(|i| 0.5 - 0.5 * (TAU * i as f32 / SAMPLES as f32).cos()),
            edges,
            envelope: [0.0; BARS],
            has_signal: false,
        }
    }

    pub(super) fn clear_samples(&mut self) {
        self.history.fill(0.0);
        self.has_signal = false;
    }

    pub(super) fn push(&mut self, sample: f32) {
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.history[self.cursor] = sample;
        self.cursor = (self.cursor + 1) % SAMPLES;
        self.has_signal |= sample.abs() > 0.00001;
    }

    pub(super) fn levels(&mut self) -> Levels {
        let mut targets = [0.0; BARS];
        if self.has_signal {
            let mut samples =
                std::array::from_fn(|i| self.history[(self.cursor + i) % SAMPLES] * self.window[i]);
            let bins = microfft::real::rfft_2048(&mut samples);
            for (target, range) in targets.iter_mut().zip(self.edges.windows(2)) {
                let power = bins[range[0]..range[1]]
                    .iter()
                    .map(|b| b.norm_sqr())
                    .fold(0.0f32, f32::max);
                // Hann coherent gain is 1/2. Map -60..0 dBFS to the bar height.
                let normalized = power * (4.0 / SAMPLES as f32).powi(2);
                *target = (1.0 + normalized.max(1e-6).log10() / 6.0).clamp(0.0, 1.0);
            }
        } else {
            // WASAPI can stop delivering packets altogether when nothing is playing.
            self.history.fill(0.0);
        }
        self.has_signal = false;
        std::array::from_fn(|i| {
            let level = &mut self.envelope[i];
            let smoothing = if targets[i] > *level { 0.78 } else { 0.24 };
            *level += (targets[i] - *level) * smoothing;
            (*level * 20.0).round() as u8
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_land_in_their_frequency_bands_and_silence_decays() {
        for rate in [44_100, 48_000, 96_000] {
            for frequency in [220.0, 3000.0, 12_000.0] {
                let mut spectrum = Spectrum::new(rate);
                for i in 0..SAMPLES {
                    spectrum.push((TAU * frequency * i as f32 / rate as f32).sin());
                }
                let bin = (frequency * SAMPLES as f32 / rate as f32).round() as usize;
                let band = spectrum
                    .edges
                    .windows(2)
                    .position(|e| (e[0]..e[1]).contains(&bin))
                    .unwrap();
                let levels = spectrum.levels();
                assert_eq!(levels[band], *levels.iter().max().unwrap());
                assert!(levels[band] > 12);
                assert!((1..levels[band]).contains(&spectrum.levels()[band]));
                for _ in 0..30 {
                    spectrum.levels();
                }
                assert_eq!(spectrum.levels(), [0; BARS]);
            }
        }
    }
}
