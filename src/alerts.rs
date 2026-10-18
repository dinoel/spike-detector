//! Turns each tick's raw findings into what the user sees: keeps the list of
//! what is wrong right now, decides what deserves a notification, and
//! remembers "don't tell me again" choices.

use std::collections::{HashMap, HashSet};

use crate::detect::{Finding, FindingKind};
use crate::sampler::ProcessKey;
use crate::tracker::Tracker;
use crate::win::process::ProcessDetails;

type AlertId = (ProcessKey, FindingKind);

#[derive(Debug, Clone)]
pub struct Alert {
    pub finding: Finding,
    pub name: String,
    pub details: ProcessDetails,
}

impl Alert {
    fn id(&self) -> AlertId {
        (self.finding.key, self.finding.kind)
    }
}

pub struct Alerts {
    active: Vec<Alert>,
    last_notified_ms: HashMap<AlertId, u64>,
    dismissed: HashSet<AlertId>,
    renotify_ms: u64,
}

impl Alerts {
    pub fn new(renotify_ms: u64) -> Self {
        Self {
            active: Vec::new(),
            last_notified_ms: HashMap::new(),
            dismissed: HashSet::new(),
            renotify_ms,
        }
    }

    pub fn set_renotify(&mut self, renotify_ms: u64) {
        self.renotify_ms = renotify_ms;
    }

    pub fn active(&self) -> &[Alert] {
        &self.active
    }

    /// Makes this tick's `findings` the active set. Returns the alerts to
    /// announce now: new ones, and lasting ones whose re-notify interval has
    /// passed. `describe` runs once per new alert, not on every tick.
    pub fn update(
        &mut self,
        now_ms: u64,
        findings: Vec<Finding>,
        tracker: &Tracker,
        mut describe: impl FnMut(ProcessKey) -> ProcessDetails,
    ) -> Vec<&Alert> {
        let mut previous = std::mem::take(&mut self.active);
        let mut announce = Vec::new();
        for finding in findings {
            let id = (finding.key, finding.kind);
            if self.dismissed.contains(&id) {
                continue;
            }
            let Some(process) = tracker.get(finding.key) else {
                continue;
            };
            let alert = match previous.iter().position(|a| a.id() == id) {
                Some(i) => Alert {
                    finding,
                    ..previous.swap_remove(i)
                },
                None => Alert {
                    name: process.name.clone(),
                    details: describe(finding.key),
                    finding,
                },
            };
            let due = self
                .last_notified_ms
                .get(&id)
                .is_none_or(|&at| now_ms.saturating_sub(at) >= self.renotify_ms);
            if due {
                self.last_notified_ms.insert(id, now_ms);
                announce.push(self.active.len());
            }
            self.active.push(alert);
        }
        // Per-process memory goes away with the process.
        self.last_notified_ms
            .retain(|(key, _), _| tracker.contains(*key));
        self.dismissed.retain(|(key, _)| tracker.contains(*key));
        announce.into_iter().map(|i| &self.active[i]).collect()
    }

    /// Hides one alert until its process exits.
    pub fn dismiss(&mut self, key: ProcessKey, kind: FindingKind) {
        self.dismissed.insert((key, kind));
        self.active.retain(|a| a.id() != (key, kind));
    }

    /// Drops alerts about a process the user has just dealt with.
    pub fn forget_process(&mut self, key: ProcessKey) {
        self.active.retain(|a| a.finding.key != key);
    }

    /// Drops alerts about every process with this lower-cased exe name.
    pub fn forget_name(&mut self, name_lc: &str) {
        self.active.retain(|a| a.name.to_lowercase() != name_lc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::testing::{Fake, feed};

    const RENOTIFY_MS: u64 = 60 * 60_000;

    fn cpu_finding(p: &Fake) -> Finding {
        Finding {
            key: p.key(),
            kind: FindingKind::CpuHog,
            summary: "busy".into(),
        }
    }

    fn announced(
        alerts: &mut Alerts,
        now_ms: u64,
        findings: Vec<Finding>,
        tracker: &Tracker,
    ) -> usize {
        alerts
            .update(now_ms, findings, tracker, |_| ProcessDetails::default())
            .len()
    }

    #[test]
    fn lasting_finding_is_announced_once_per_interval() {
        let mut tracker = Tracker::new(0);
        let p = Fake::new(10, "node.exe");
        feed(&mut tracker, 0, std::slice::from_ref(&p));
        let mut alerts = Alerts::new(RENOTIFY_MS);

        assert_eq!(
            announced(&mut alerts, 0, vec![cpu_finding(&p)], &tracker),
            1
        );
        assert_eq!(
            announced(&mut alerts, 10_000, vec![cpu_finding(&p)], &tracker),
            0
        );
        assert_eq!(alerts.active().len(), 1);
        assert_eq!(
            announced(&mut alerts, RENOTIFY_MS, vec![cpu_finding(&p)], &tracker),
            1
        );
    }

    #[test]
    fn details_are_fetched_once() {
        let mut tracker = Tracker::new(0);
        let p = Fake::new(10, "node.exe");
        feed(&mut tracker, 0, std::slice::from_ref(&p));
        let mut alerts = Alerts::new(RENOTIFY_MS);
        let mut calls = 0;
        for tick in 0..3 {
            alerts.update(tick * 10_000, vec![cpu_finding(&p)], &tracker, |_| {
                calls += 1;
                ProcessDetails::default()
            });
        }
        assert_eq!(calls, 1);
    }

    #[test]
    fn resolved_finding_leaves_active_list_but_not_cooldown() {
        let mut tracker = Tracker::new(0);
        let p = Fake::new(10, "node.exe");
        feed(&mut tracker, 0, std::slice::from_ref(&p));
        let mut alerts = Alerts::new(RENOTIFY_MS);

        announced(&mut alerts, 0, vec![cpu_finding(&p)], &tracker);
        announced(&mut alerts, 10_000, vec![], &tracker);
        assert!(alerts.active().is_empty());
        // Flapping back within the interval stays quiet.
        assert_eq!(
            announced(&mut alerts, 20_000, vec![cpu_finding(&p)], &tracker),
            0
        );
    }

    #[test]
    fn dismissed_until_process_exits() {
        let mut tracker = Tracker::new(0);
        let p = Fake::new(10, "node.exe");
        feed(&mut tracker, 0, std::slice::from_ref(&p));
        let mut alerts = Alerts::new(RENOTIFY_MS);

        announced(&mut alerts, 0, vec![cpu_finding(&p)], &tracker);
        alerts.dismiss(p.key(), FindingKind::CpuHog);
        assert!(alerts.active().is_empty());
        assert_eq!(
            announced(&mut alerts, RENOTIFY_MS, vec![cpu_finding(&p)], &tracker),
            0
        );
        assert!(alerts.active().is_empty());

        feed(&mut tracker, RENOTIFY_MS + 10_000, &[]);
        announced(&mut alerts, RENOTIFY_MS + 10_000, vec![], &tracker);
        assert!(alerts.dismissed.is_empty());
        assert!(alerts.last_notified_ms.is_empty());
    }

    #[test]
    fn forget_name_matches_case_insensitively() {
        let mut tracker = Tracker::new(0);
        let p = Fake::new(10, "MSBuild.exe");
        feed(&mut tracker, 0, std::slice::from_ref(&p));
        let mut alerts = Alerts::new(RENOTIFY_MS);
        announced(&mut alerts, 0, vec![cpu_finding(&p)], &tracker);
        alerts.forget_name("msbuild.exe");
        assert!(alerts.active().is_empty());
    }
}
