//! CPU load, judged by two rules with one verdict per process:
//! - hog: a process keeps a big share of a core busy for minutes;
//! - idle-busy: while the user is away, a process in their session keeps a
//!   smaller share busy. Nobody is watching, so nothing should work that hard.
//!
//! A process that breaks both rules is reported once, as a hog.

use super::{Context, Finding, FindingKind, ProcessDetector, minutes};
use crate::config::{CpuHogConfig, IdleBusyConfig, minutes_to_ms};
use crate::tracker::TrackedProcess;

pub struct CpuLoad {
    hog: Option<HogRule>,
    idle: Option<IdleRule>,
}

struct HogRule {
    window_ms: u64,
    min_cores: f64,
    skip_foreground: bool,
}

struct IdleRule {
    window_ms: u64,
    min_cores: f64,
}

impl CpuLoad {
    /// `None` when both rules are disabled.
    pub fn new(hog: &CpuHogConfig, idle: &IdleBusyConfig) -> Option<Self> {
        let load = Self {
            hog: hog.enabled.then(|| HogRule {
                window_ms: minutes_to_ms(hog.window_minutes),
                min_cores: f64::from(hog.min_core_percent) / 100.0,
                skip_foreground: hog.skip_foreground,
            }),
            idle: idle.enabled.then(|| IdleRule {
                window_ms: minutes_to_ms(idle.idle_minutes),
                min_cores: f64::from(idle.min_core_percent) / 100.0,
            }),
        };
        (load.hog.is_some() || load.idle.is_some()).then_some(load)
    }
}

impl ProcessDetector for CpuLoad {
    fn check(&self, process: &TrackedProcess, ctx: &Context) -> Option<Finding> {
        let hog = self.hog.as_ref().and_then(|rule| rule.check(process, ctx));
        hog.or_else(|| self.idle.as_ref().and_then(|rule| rule.check(process, ctx)))
    }
}

impl HogRule {
    fn check(&self, process: &TrackedProcess, ctx: &Context) -> Option<Finding> {
        if self.skip_foreground && ctx.foreground_pid == Some(process.key.pid) {
            return None;
        }
        let (cores, span_ms) = average_cores(process, self.window_ms)?;
        (cores >= self.min_cores).then(|| Finding {
            key: process.key,
            kind: FindingKind::CpuHog,
            summary: format!(
                "{:.0}% of a core on average over {} min",
                cores * 100.0,
                minutes(span_ms)
            ),
        })
    }
}

impl IdleRule {
    fn check(&self, process: &TrackedProcess, ctx: &Context) -> Option<Finding> {
        // The whole window must fall inside the user's absence.
        if ctx.user_idle_ms? < self.window_ms {
            return None;
        }
        // Session 0 holds services, and Windows schedules its own maintenance
        // for exactly these idle periods.
        if Some(process.session_id) != ctx.user_session {
            return None;
        }
        // No input, yet someone may be watching: a full-screen video, say.
        if ctx.foreground_pid == Some(process.key.pid) {
            return None;
        }
        let (cores, span_ms) = average_cores(process, self.window_ms)?;
        (cores >= self.min_cores).then(|| Finding {
            key: process.key,
            kind: FindingKind::IdleBusy,
            summary: format!(
                "{:.0}% of a core while you were away ({} min)",
                cores * 100.0,
                minutes(span_ms)
            ),
        })
    }
}

/// Average CPU use over the window, in cores (1.0 = one core fully busy),
/// and the span it was measured over.
fn average_cores(process: &TrackedProcess, window_ms: u64) -> Option<(f64, u64)> {
    let (start, span_ms) = process.window(window_ms)?;
    let cpu_100ns = process.latest.cpu_time.saturating_sub(start.cpu_time);
    Some((cpu_100ns as f64 / (span_ms as f64 * 10_000.0), span_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Tracker;
    use crate::tracker::testing::{Fake, USER_SESSION, feed};

    const TICK_MS: u64 = 10_000;

    /// Runs `p` at `cores` load for `duration_ms`, one snapshot per tick.
    fn run(mut p: Fake, cores: f64, duration_ms: u64) -> (Tracker, Fake) {
        let mut tracker = Tracker::new(minutes_to_ms(15));
        let cpu_per_tick = (cores * TICK_MS as f64 * 10_000.0) as u64;
        for at in (0..=duration_ms).step_by(TICK_MS as usize) {
            feed(&mut tracker, at, std::slice::from_ref(&p));
            p.cpu_time += cpu_per_tick;
        }
        (tracker, p)
    }

    fn check(tracker: &Tracker, p: &Fake, ctx: &Context) -> Option<Finding> {
        let detector = CpuLoad::new(&CpuHogConfig::default(), &IdleBusyConfig::default()).unwrap();
        detector.check(tracker.get(p.key()).unwrap(), ctx)
    }

    fn user_away_for(minutes: u32) -> Context {
        Context {
            user_idle_ms: Some(minutes_to_ms(minutes)),
            user_session: Some(USER_SESSION),
            ..Context::default()
        }
    }

    fn node() -> Fake {
        Fake::new(100, "node.exe")
    }

    #[test]
    fn sustained_load_is_reported() {
        let (tracker, p) = run(node(), 0.95, minutes_to_ms(6));
        let finding = check(&tracker, &p, &Context::default()).expect("no finding");
        assert_eq!(finding.kind, FindingKind::CpuHog);
        assert!(finding.summary.starts_with("95%"), "{}", finding.summary);
    }

    #[test]
    fn moderate_load_is_fine_while_user_is_here() {
        let (tracker, p) = run(node(), 0.5, minutes_to_ms(12));
        assert_eq!(check(&tracker, &p, &Context::default()), None);
    }

    #[test]
    fn short_burst_is_not_judged_yet() {
        let (tracker, p) = run(node(), 1.0, minutes_to_ms(4));
        assert_eq!(check(&tracker, &p, &Context::default()), None);
    }

    #[test]
    fn foreground_process_is_skipped() {
        let (tracker, p) = run(node(), 1.0, minutes_to_ms(6));
        let ctx = Context {
            foreground_pid: Some(p.pid),
            ..Context::default()
        };
        assert_eq!(check(&tracker, &p, &ctx), None);
    }

    #[test]
    fn moderate_load_while_user_is_away_is_reported() {
        let (tracker, p) = run(node(), 0.4, minutes_to_ms(12));
        let finding = check(&tracker, &p, &user_away_for(11)).expect("no finding");
        assert_eq!(finding.kind, FindingKind::IdleBusy);
        assert!(finding.summary.starts_with("40%"), "{}", finding.summary);
    }

    #[test]
    fn short_absence_is_not_enough() {
        let (tracker, p) = run(node(), 0.4, minutes_to_ms(12));
        assert_eq!(check(&tracker, &p, &user_away_for(5)), None);
    }

    #[test]
    fn services_may_work_while_user_is_away() {
        let service = Fake {
            session_id: 0,
            ..Fake::new(100, "SearchIndexer.exe")
        };
        let (tracker, p) = run(service, 0.4, minutes_to_ms(12));
        assert_eq!(check(&tracker, &p, &user_away_for(11)), None);
    }

    #[test]
    fn hog_while_away_is_reported_once_as_hog() {
        let (tracker, p) = run(node(), 1.0, minutes_to_ms(12));
        let finding = check(&tracker, &p, &user_away_for(11)).expect("no finding");
        assert_eq!(finding.kind, FindingKind::CpuHog);
    }
}
