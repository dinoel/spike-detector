//! A watched dev tool keeps running after its parent exited: the terminal or
//! IDE that started it is gone, but it is still here.

use std::collections::HashSet;

use super::{Context, Finding, FindingKind, ProcessDetector, minutes};
use crate::config::{OrphanConfig, minutes_to_ms};
use crate::tracker::TrackedProcess;

pub struct Orphan {
    min_ms: u64,
    /// Lower-cased exe names.
    names: HashSet<String>,
}

impl Orphan {
    pub fn new(config: &OrphanConfig) -> Self {
        Self {
            min_ms: minutes_to_ms(config.min_minutes),
            names: config.names.iter().map(|n| n.to_lowercase()).collect(),
        }
    }
}

impl ProcessDetector for Orphan {
    fn check(&self, process: &TrackedProcess, _ctx: &Context) -> Option<Finding> {
        let since = process.orphaned_since_ms?;
        if !self.names.contains(&process.name_lc) {
            return None;
        }
        let orphaned_for = process.latest.at_ms.saturating_sub(since);
        (orphaned_for >= self.min_ms).then(|| Finding {
            key: process.key,
            kind: FindingKind::Orphan,
            summary: format!(
                "running without its parent for {} min",
                minutes(orphaned_for)
            ),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Tracker;
    use crate::tracker::testing::{Fake, feed};

    /// Parent and child run together, then the parent exits and the child
    /// stays alone for `alone_ms`.
    fn orphan_for(child_name: &'static str, alone_ms: u64) -> Option<Finding> {
        let mut tracker = Tracker::new(0);
        let parent = Fake::new(1, "WindowsTerminal.exe");
        let child = Fake::new(2, child_name).child_of(1);
        feed(&mut tracker, 0, &[parent, child.clone()]);
        feed(&mut tracker, 10_000, std::slice::from_ref(&child));
        feed(
            &mut tracker,
            10_000 + alone_ms,
            std::slice::from_ref(&child),
        );
        let detector = Orphan::new(&OrphanConfig::default());
        detector.check(tracker.get(child.key()).unwrap(), &Context::default())
    }

    #[test]
    fn long_lived_orphan_is_reported() {
        let finding = orphan_for("node.exe", minutes_to_ms(31)).expect("no finding");
        assert_eq!(finding.kind, FindingKind::Orphan);
    }

    #[test]
    fn name_match_ignores_case() {
        assert!(orphan_for("msbuild.EXE", minutes_to_ms(31)).is_some());
    }

    #[test]
    fn recent_orphan_gets_grace_time() {
        assert_eq!(orphan_for("node.exe", minutes_to_ms(5)), None);
    }

    #[test]
    fn unwatched_names_are_ignored() {
        assert_eq!(orphan_for("explorer.exe", minutes_to_ms(120)), None);
    }
}
