//! Copies of the same dev tool pile up: watchers, test runners or build nodes
//! that keep getting spawned and never cleaned up. Reported once per exe name,
//! on its oldest instance.

use std::collections::{HashMap, HashSet};

use super::{Context, Detector, Finding, FindingKind, minutes};
use crate::config::{SwarmConfig, minutes_to_ms};
use crate::tracker::TrackedProcess;

pub struct Swarm {
    min_instances: usize,
    min_age_ms: u64,
    /// Lower-cased exe names.
    names: HashSet<String>,
}

impl Swarm {
    pub fn new(config: &SwarmConfig) -> Self {
        Self {
            min_instances: config.min_instances as usize,
            min_age_ms: minutes_to_ms(config.min_minutes),
            names: config.names.iter().map(|n| n.to_lowercase()).collect(),
        }
    }

    /// Short-lived instances (a parallel test run) don't make a swarm.
    fn counts(&self, process: &TrackedProcess) -> bool {
        self.names.contains(&process.name_lc)
            && process.latest.at_ms.saturating_sub(process.first_seen_ms) >= self.min_age_ms
    }
}

impl Detector for Swarm {
    fn scan(&self, processes: &[&TrackedProcess], _ctx: &Context, findings: &mut Vec<Finding>) {
        // Per name: how many instances count, and the oldest of them.
        let mut groups: HashMap<&str, (usize, &TrackedProcess)> = HashMap::new();
        for &process in processes.iter().filter(|p| self.counts(p)) {
            groups
                .entry(process.name_lc.as_str())
                .and_modify(|(count, oldest)| {
                    *count += 1;
                    if process.key.create_time < oldest.key.create_time {
                        *oldest = process;
                    }
                })
                .or_insert((1, process));
        }
        findings.extend(
            groups
                .into_values()
                .filter(|&(count, _)| count >= self.min_instances)
                .map(|(count, oldest)| Finding {
                    key: oldest.key,
                    kind: FindingKind::Swarm,
                    summary: format!(
                        "{count} instances running for over {} min",
                        minutes(self.min_age_ms)
                    ),
                }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::Tracker;
    use crate::tracker::testing::{Fake, feed};

    /// Runs `old` instances for 15 minutes, then adds `young` fresh ones.
    fn scan(name: &'static str, old: u32, young: u32) -> Vec<Finding> {
        let mut tracker = Tracker::new(0);
        let mut fakes: Vec<Fake> = (1..=old).map(|pid| Fake::new(pid, name)).collect();
        fakes.push(Fake::new(10_000, "explorer.exe"));
        feed(&mut tracker, 0, &fakes);
        fakes.extend((0..young).map(|i| Fake::new(5_000 + i, name)));
        feed(&mut tracker, minutes_to_ms(15), &fakes);

        let processes: Vec<&TrackedProcess> = tracker.processes().collect();
        let mut findings = Vec::new();
        Swarm::new(&SwarmConfig::default()).scan(&processes, &Context::default(), &mut findings);
        findings
    }

    #[test]
    fn long_lived_pile_is_reported_once_on_oldest() {
        let findings = scan("node.exe", 30, 0);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].kind, FindingKind::Swarm);
        assert_eq!(findings[0].key.pid, 1);
        assert!(
            findings[0].summary.starts_with("30 instances"),
            "{}",
            findings[0].summary
        );
    }

    #[test]
    fn short_lived_burst_does_not_count() {
        assert!(scan("node.exe", 10, 30).is_empty());
    }

    #[test]
    fn below_threshold_is_fine() {
        assert!(scan("node.exe", 24, 0).is_empty());
    }

    #[test]
    fn unwatched_names_are_ignored() {
        assert!(scan("svchost.exe", 90, 0).is_empty());
    }
}
