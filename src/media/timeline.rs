use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct Timeline {
    pub duration: Duration,
    pub position: Duration,
    pub advancing: bool,
    rate: f64,
    observed: Instant,
    start_ticks: i64,
    seek_range: Option<(i64, i64)>,
}

impl Timeline {
    /// WinRT timelines use 100 ns ticks, and may start at a nonzero offset.
    pub fn from_ticks(
        start: i64,
        end: i64,
        position: i64,
        rate: f64,
        advancing: bool,
        age: Duration,
    ) -> Option<Self> {
        let duration = end.checked_sub(start)?;
        let position = position.checked_sub(start)?;
        if duration <= 0
            || duration > 7 * 24 * 3600 * 10_000_000
            || position < -50_000_000
            || position > duration.saturating_add(50_000_000)
        {
            return None;
        }
        let duration = Duration::from_nanos(duration as u64 * 100);
        let advancing = advancing && rate.is_finite() && rate > 0.0 && rate <= 16.0;
        let mut timeline = Self {
            duration,
            position: Duration::from_nanos(position.max(0) as u64 * 100).min(duration),
            advancing,
            rate: if advancing { rate } else { 0.0 },
            observed: Instant::now(),
            start_ticks: start,
            seek_range: None,
        };
        timeline.position = timeline.position_after(age);
        Some(timeline)
    }

    pub fn position_after(&self, elapsed: Duration) -> Duration {
        if !self.advancing {
            return self.position;
        }
        self.position
            .saturating_add(elapsed.min(self.duration).mul_f64(self.rate))
            .min(self.duration)
    }

    pub fn current_position(&self) -> Duration {
        self.position_after(self.observed.elapsed())
    }

    pub fn with_seek_range(mut self, minimum: i64, maximum: i64) -> Self {
        let end = self.start_ticks + (self.duration.as_nanos() / 100) as i64;
        let minimum = minimum.max(self.start_ticks);
        let maximum = maximum.min(end);
        self.seek_range = (minimum < maximum).then_some((minimum, maximum));
        self
    }

    /// Convert a bar position to an absolute WinRT position within the seekable range.
    pub fn seek_ticks(&self, fraction: f64) -> Option<i64> {
        if !fraction.is_finite() {
            return None;
        }
        let (minimum, maximum) = self.seek_range?;
        let offset =
            ((self.duration.as_nanos() / 100) as f64 * fraction.clamp(0.0, 1.0)).round() as i64;
        Some((self.start_ticks + offset).clamp(minimum, maximum))
    }

    pub fn seek_position(&self, fraction: f64) -> Option<Duration> {
        let ticks = self.seek_ticks(fraction)? - self.start_ticks;
        Some(Duration::from_nanos(ticks as u64 * 100))
    }
}

pub fn timestamp(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds >= 3600 {
        format!(
            "{}:{:02}:{:02}",
            seconds / 3600,
            seconds / 60 % 60,
            seconds % 60
        )
    } else {
        format!("{}:{:02}", seconds / 60, seconds % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accounts_for_nonzero_start_rate_and_stale_event() {
        let timeline = Timeline::from_ticks(
            10_000_000,
            110_000_000,
            30_000_000,
            2.0,
            true,
            Duration::from_secs(1),
        )
        .expect("timeline");
        assert_eq!(timeline.position, Duration::from_secs(4));
        assert_eq!(
            timeline.position_after(Duration::from_secs(1)),
            Duration::from_secs(6)
        );
        assert_eq!(
            timeline.position_after(Duration::from_secs(100)),
            Duration::from_secs(10)
        );
    }
    #[test]
    fn ignores_live_unknown_and_inaccurate_ranges() {
        assert!(Timeline::from_ticks(0, 0, 0, 1.0, true, Duration::ZERO).is_none());
        assert!(Timeline::from_ticks(i64::MIN, i64::MAX, 0, 1.0, true, Duration::ZERO).is_none());
        assert!(
            Timeline::from_ticks(0, 10_000_000, 100_000_000, 1.0, true, Duration::ZERO).is_none()
        );
        let paused = Timeline::from_ticks(
            0,
            100_000_000,
            20_000_000,
            1.0,
            false,
            Duration::from_secs(20),
        )
        .expect("paused");
        assert_eq!(
            paused.position_after(Duration::from_secs(20)),
            Duration::from_secs(2)
        );
        assert!(
            !Timeline::from_ticks(0, 100_000_000, 0, f64::NAN, true, Duration::ZERO)
                .expect("static")
                .advancing
        );
    }
    #[test]
    fn formats_long_playback_times() {
        assert_eq!(timestamp(Duration::from_secs(65)), "1:05");
        assert_eq!(timestamp(Duration::from_secs(3605)), "1:00:05");
    }

    #[test]
    fn seek_uses_absolute_ticks_and_clamps_to_provider_range() {
        let timeline = Timeline::from_ticks(
            10_000_000,
            110_000_000,
            30_000_000,
            1.0,
            false,
            Duration::ZERO,
        )
        .expect("timeline")
        .with_seek_range(30_000_000, 90_000_000);
        assert_eq!(timeline.seek_ticks(0.5), Some(60_000_000));
        assert_eq!(timeline.seek_position(0.5), Some(Duration::from_secs(5)));
        assert_eq!(timeline.seek_ticks(-0.5), Some(30_000_000));
        assert_eq!(timeline.seek_ticks(1.5), Some(90_000_000));
        assert_eq!(timeline.seek_ticks(f64::NAN), None);
        assert_eq!(timeline.seek_ticks(f64::INFINITY), None);
        let full = timeline.with_seek_range(i64::MIN, i64::MAX);
        assert_eq!(full.seek_ticks(0.0), Some(10_000_000));
        assert_eq!(full.seek_ticks(1.0), Some(110_000_000));
    }

    #[test]
    fn invalid_seek_ranges_leave_progress_available() {
        let timeline = Timeline::from_ticks(0, 100_000_000, 20_000_000, 1.0, false, Duration::ZERO)
            .expect("timeline");
        for (minimum, maximum) in [(0, 0), (80_000_000, 20_000_000), (110_000_000, 120_000_000)] {
            let timeline = timeline.clone().with_seek_range(minimum, maximum);
            assert_eq!(timeline.seek_ticks(0.5), None);
            assert_eq!(timeline.current_position(), Duration::from_secs(2));
        }
        let near_limit = Timeline::from_ticks(
            i64::MAX - 100_000_000,
            i64::MAX,
            i64::MAX - 50_000_000,
            1.0,
            false,
            Duration::ZERO,
        )
        .expect("timeline")
        .with_seek_range(i64::MIN, i64::MAX);
        assert_eq!(near_limit.seek_ticks(1.0), Some(i64::MAX));
    }
}
