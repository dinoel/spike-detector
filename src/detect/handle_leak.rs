//! Handle or thread count climbs steadily across the whole window. Healthy
//! processes hover around a stable count; a leak only ever goes up.

use super::trend::Trend;
use super::{Context, Finding, FindingKind, ProcessDetector, minutes};
use crate::config::{HandleLeakConfig, minutes_to_ms};
use crate::tracker::{Sample, TrackedProcess};

pub struct HandleLeak {
    window_ms: u64,
    min_handle_growth: f64,
    min_thread_growth: f64,
    min_r2: f64,
}

impl HandleLeak {
    pub fn new(config: &HandleLeakConfig) -> Self {
        Self {
            window_ms: minutes_to_ms(config.window_minutes),
            min_handle_growth: f64::from(config.min_handle_growth),
            min_thread_growth: f64::from(config.min_thread_growth),
            min_r2: config.min_r2,
        }
    }

    /// Evidence that `metric` leaks, e.g. "+2400 handles (now 5100)".
    fn leak(
        &self,
        process: &TrackedProcess,
        what: &str,
        min_growth: f64,
        metric: fn(&Sample) -> u32,
    ) -> Option<String> {
        let trend = Trend::of(process, self.window_ms, |s| f64::from(metric(s)))?;
        trend.is_steady_rise(min_growth, self.min_r2).then(|| {
            format!(
                "+{:.0} {what} (now {})",
                trend.growth,
                metric(&process.latest)
            )
        })
    }
}

impl ProcessDetector for HandleLeak {
    fn check(&self, process: &TrackedProcess, _ctx: &Context) -> Option<Finding> {
        let leaks: Vec<String> = [
            self.leak(process, "handles", self.min_handle_growth, |s| s.handles),
            self.leak(process, "threads", self.min_thread_growth, |s| s.threads),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!leaks.is_empty()).then(|| Finding {
            key: process.key,
            kind: FindingKind::HandleLeak,
            summary: format!("{} in {} min", leaks.join(", "), minutes(self.window_ms)),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Tracker;
    use crate::tracker::testing::{Fake, feed};

    const TICK_MS: u64 = 10_000;

    /// Feeds a process for 35 minutes; `grow(ms, fake)` sets its counts.
    fn run(grow: impl Fn(u64, &mut Fake)) -> Option<Finding> {
        let config = HandleLeakConfig::default();
        let mut tracker = Tracker::new(minutes_to_ms(config.window_minutes));
        let mut p = Fake::new(100, "service.exe");
        for at in (0..=minutes_to_ms(35)).step_by(TICK_MS as usize) {
            grow(at, &mut p);
            feed(&mut tracker, at, std::slice::from_ref(&p));
        }
        HandleLeak::new(&config).check(tracker.get(p.key()).unwrap(), &Context::default())
    }

    /// `per_minute` units per minute on top of `base`.
    fn linear(base: u32, per_minute: u64, ms: u64) -> u32 {
        base + u32::try_from(ms * per_minute / 60_000).unwrap()
    }

    #[test]
    fn handle_leak_is_reported() {
        // 100 handles a minute: 3000 over the window.
        let finding = run(|ms, p| p.handles = linear(500, 100, ms)).expect("no finding");
        assert_eq!(finding.kind, FindingKind::HandleLeak);
        assert!(
            finding.summary.starts_with("+3000 handles"),
            "{}",
            finding.summary
        );
        assert!(!finding.summary.contains("threads"), "{}", finding.summary);
    }

    #[test]
    fn thread_leak_is_reported() {
        // 5 threads a minute: 150 over the window.
        let finding = run(|ms, p| p.threads = linear(10, 5, ms)).expect("no finding");
        assert!(
            finding.summary.starts_with("+150 threads"),
            "{}",
            finding.summary
        );
    }

    #[test]
    fn both_leaks_share_one_finding() {
        let finding = run(|ms, p| {
            p.handles = linear(500, 100, ms);
            p.threads = linear(10, 5, ms);
        })
        .expect("no finding");
        assert!(
            finding.summary.contains("handles") && finding.summary.contains("threads"),
            "{}",
            finding.summary
        );
    }

    #[test]
    fn busy_but_stable_process_is_fine() {
        // Swings by 1500 handles every 3 minutes, but no upward trend.
        assert_eq!(
            run(|ms, p| p.handles = 2000 + if (ms / 180_000) % 2 == 0 { 0 } else { 1500 }),
            None
        );
    }
}
