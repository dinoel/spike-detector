//! Private memory climbs steadily across the whole window: a near-straight
//! upward line, not the sawtooth of a healthy garbage-collected heap.

use super::trend::Trend;
use super::{Context, Finding, FindingKind, ProcessDetector, minutes};
use crate::config::{MemLeakConfig, minutes_to_ms};
use crate::tracker::TrackedProcess;

const BYTES_PER_MB: f64 = 1024.0 * 1024.0;

pub struct MemLeak {
    window_ms: u64,
    min_growth_bytes: f64,
    min_r2: f64,
}

impl MemLeak {
    pub fn new(config: &MemLeakConfig) -> Self {
        Self {
            window_ms: minutes_to_ms(config.window_minutes),
            min_growth_bytes: f64::from(config.min_growth_mb) * BYTES_PER_MB,
            min_r2: config.min_r2,
        }
    }
}

impl ProcessDetector for MemLeak {
    fn check(&self, process: &TrackedProcess, _ctx: &Context) -> Option<Finding> {
        let trend = Trend::of(process, self.window_ms, |s| s.private_bytes as f64)?;
        trend
            .is_steady_rise(self.min_growth_bytes, self.min_r2)
            .then(|| Finding {
                key: process.key,
                kind: FindingKind::MemLeak,
                summary: format!(
                    "+{:.0} MB in {} min, now {:.0} MB",
                    trend.growth / BYTES_PER_MB,
                    minutes(self.window_ms),
                    process.latest.private_bytes as f64 / BYTES_PER_MB
                ),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Tracker;
    use crate::tracker::testing::{Fake, feed};

    const TICK_MS: u64 = 10_000;
    const MB: u64 = 1024 * 1024;

    /// Feeds a process whose private bytes follow `memory_at(ms)`.
    fn run(duration_ms: u64, memory_at: impl Fn(u64) -> u64) -> Option<Finding> {
        let config = MemLeakConfig::default();
        let mut tracker = Tracker::new(minutes_to_ms(config.window_minutes));
        let mut p = Fake::new(100, "node.exe");
        for at in (0..=duration_ms).step_by(TICK_MS as usize) {
            p.private_bytes = memory_at(at);
            feed(&mut tracker, at, std::slice::from_ref(&p));
        }
        MemLeak::new(&config).check(tracker.get(p.key()).unwrap(), &Context::default())
    }

    #[test]
    fn steady_growth_is_reported() {
        // 20 MB per minute: 600 MB over the 30-minute window.
        let finding =
            run(minutes_to_ms(35), |ms| 200 * MB + ms * 20 * MB / 60_000).expect("no finding");
        assert_eq!(finding.kind, FindingKind::MemLeak);
        assert!(
            finding.summary.starts_with("+600 MB"),
            "{}",
            finding.summary
        );
    }

    #[test]
    fn slow_growth_is_fine() {
        // 2 MB per minute: only 60 MB over the window.
        assert_eq!(
            run(minutes_to_ms(35), |ms| 200 * MB + ms * 2 * MB / 60_000),
            None
        );
    }

    #[test]
    fn gc_sawtooth_is_fine() {
        // Climbs 400 MB over 5 minutes, then drops back: big swings, no trend.
        let sawtooth = |ms: u64| 200 * MB + (ms % minutes_to_ms(5)) * 400 * MB / minutes_to_ms(5);
        assert_eq!(run(minutes_to_ms(60), sawtooth), None);
    }

    #[test]
    fn not_judged_before_window_is_covered() {
        assert_eq!(run(minutes_to_ms(20), |ms| ms * 100 * MB / 60_000), None);
    }
}
