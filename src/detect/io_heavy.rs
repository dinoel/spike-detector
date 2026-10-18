//! A process keeps pushing lots of data through its own handles for minutes:
//! a runaway logger, a build stuck copying files, a watcher re-reading a tree.
//!
//! This is file I/O as Windows attributes it to the process (Task Manager's
//! "I/O read/write bytes"), so reads served from the file cache count too.
//! Per-process disk counters exist, but only with elevation.

use super::{Context, Finding, FindingKind, ProcessDetector, minutes};
use crate::config::{IoHeavyConfig, minutes_to_ms};
use crate::tracker::TrackedProcess;

const BYTES_PER_MB: f64 = 1024.0 * 1024.0;

pub struct IoHeavy {
    window_ms: u64,
    min_bytes_per_sec: f64,
    skip_foreground: bool,
}

impl IoHeavy {
    pub fn new(config: &IoHeavyConfig) -> Self {
        Self {
            window_ms: minutes_to_ms(config.window_minutes),
            min_bytes_per_sec: f64::from(config.min_mb_per_sec) * BYTES_PER_MB,
            skip_foreground: config.skip_foreground,
        }
    }
}

impl ProcessDetector for IoHeavy {
    fn check(&self, process: &TrackedProcess, ctx: &Context) -> Option<Finding> {
        if self.skip_foreground && ctx.foreground_pid == Some(process.key.pid) {
            return None;
        }
        let (start, span_ms) = process.window(self.window_ms)?;
        let read = process.latest.io_read.saturating_sub(start.io_read);
        let written = process.latest.io_write.saturating_sub(start.io_write);
        let per_sec = (read + written) as f64 / (span_ms as f64 / 1000.0);
        if per_sec < self.min_bytes_per_sec {
            return None;
        }
        let volumes: Vec<String> = [(read, "read"), (written, "written")]
            .into_iter()
            .filter(|&(bytes, _)| bytes > 0)
            .map(|(bytes, what)| format!("{} {what}", size(bytes)))
            .collect();
        Some(Finding {
            key: process.key,
            kind: FindingKind::IoHeavy,
            summary: format!(
                "{:.0} MB/s for {} min: {}",
                per_sec / BYTES_PER_MB,
                minutes(span_ms),
                volumes.join(", ")
            ),
        })
    }
}

fn size(bytes: u64) -> String {
    let mb = bytes as f64 / BYTES_PER_MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Tracker;
    use crate::tracker::testing::{Fake, feed};

    const TICK_MS: u64 = 10_000;
    const MB: u64 = 1024 * 1024;

    /// Feeds a process for 12 minutes; `rate(ms)` gives (read, write) MB/s at each moment.
    fn run(rate: impl Fn(u64) -> (u64, u64), ctx: &Context) -> Option<Finding> {
        let config = IoHeavyConfig::default();
        let mut tracker = Tracker::new(minutes_to_ms(config.window_minutes));
        let mut p = Fake::new(100, "node.exe");
        for at in (0..=minutes_to_ms(12)).step_by(TICK_MS as usize) {
            feed(&mut tracker, at, std::slice::from_ref(&p));
            let (read, write) = rate(at);
            p.io_read += read * MB * TICK_MS / 1000;
            p.io_write += write * MB * TICK_MS / 1000;
        }
        IoHeavy::new(&config).check(tracker.get(p.key()).unwrap(), ctx)
    }

    #[test]
    fn sustained_io_is_reported() {
        let finding = run(|_| (30, 5), &Context::default()).expect("no finding");
        assert_eq!(finding.kind, FindingKind::IoHeavy);
        assert!(
            finding.summary.starts_with("35 MB/s"),
            "{}",
            finding.summary
        );
        assert!(
            finding.summary.contains("read") && finding.summary.contains("GB"),
            "{}",
            finding.summary
        );
    }

    #[test]
    fn zero_side_is_left_out() {
        let finding = run(|_| (30, 0), &Context::default()).expect("no finding");
        assert!(!finding.summary.contains("written"), "{}", finding.summary);
    }

    #[test]
    fn light_io_is_fine() {
        assert_eq!(run(|_| (2, 1), &Context::default()), None);
    }

    #[test]
    fn short_burst_is_fine() {
        // 100 MB/s for 30 seconds, quiet otherwise: 3 GB, 5 MB/s over the 10-minute window.
        let burst = |ms| {
            if (minutes_to_ms(5)..minutes_to_ms(5) + 30_000).contains(&ms) {
                (100, 0)
            } else {
                (0, 0)
            }
        };
        assert_eq!(run(burst, &Context::default()), None);
    }

    #[test]
    fn foreground_process_is_skipped() {
        let ctx = Context {
            foreground_pid: Some(100),
            ..Context::default()
        };
        assert_eq!(run(|_| (30, 5), &ctx), None);
    }

    #[test]
    fn sizes_read_naturally() {
        assert_eq!(size(512 * MB), "512 MB");
        assert_eq!(size(3 * 1024 * MB + 512 * MB), "3.5 GB");
    }
}
