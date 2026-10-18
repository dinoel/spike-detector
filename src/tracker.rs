//! The process table: one entry per live process, with a short, coarse
//! history of samples and the facts that take more than one snapshot to know.

use std::collections::{HashMap, VecDeque};

use crate::sampler::{ProcessKey, RawProcess};

/// Spacing of history points. Detectors look back minutes, so a point every
/// 30 s is plenty and keeps per-process memory small.
pub const HISTORY_STEP_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Milliseconds since the app started.
    pub at_ms: u64,
    /// User + kernel time, 100 ns units.
    pub cpu_time: u64,
    pub private_bytes: u64,
    /// Cumulative bytes read and written; see [`RawProcess`].
    pub io_read: u64,
    pub io_write: u64,
    pub handles: u32,
    pub threads: u32,
}

impl Sample {
    fn new(at_ms: u64, raw: &RawProcess<'_>) -> Self {
        Self {
            at_ms,
            cpu_time: raw.cpu_time,
            private_bytes: raw.private_bytes,
            io_read: raw.io_read_bytes,
            io_write: raw.io_write_bytes,
            handles: raw.handle_count,
            threads: raw.thread_count,
        }
    }
}

#[derive(Debug)]
pub struct TrackedProcess {
    pub key: ProcessKey,
    pub parent_pid: u32,
    pub session_id: u32,
    pub name: String,
    /// `name` lower-cased, for case-insensitive matching against config lists.
    pub name_lc: String,
    /// When the tracker first saw the process (app clock, not its start time).
    pub first_seen_ms: u64,
    /// The most recent sample, updated every tick.
    pub latest: Sample,
    /// When the parent was first seen gone; `None` while it is alive.
    pub orphaned_since_ms: Option<u64>,
    history: VecDeque<Sample>,
    last_seen_generation: u64,
}

impl TrackedProcess {
    fn new(raw: &RawProcess<'_>, sample: Sample, generation: u64) -> Self {
        let name = String::from_utf16_lossy(raw.name);
        Self {
            key: raw.key,
            parent_pid: raw.parent_pid,
            session_id: raw.session_id,
            name_lc: name.to_lowercase(),
            name,
            first_seen_ms: sample.at_ms,
            latest: sample,
            orphaned_since_ms: None,
            history: VecDeque::from([sample]),
            last_seen_generation: generation,
        }
    }

    fn observe(&mut self, sample: Sample, generation: u64, history_len: usize) {
        self.latest = sample;
        self.last_seen_generation = generation;
        let due = self
            .history
            .back()
            .is_none_or(|last| sample.at_ms.saturating_sub(last.at_ms) >= HISTORY_STEP_MS);
        if due {
            while self.history.len() >= history_len {
                self.history.pop_front();
            }
            self.history.push_back(sample);
        }
    }

    /// History points, oldest first, roughly [`HISTORY_STEP_MS`] apart.
    pub fn history(&self) -> impl Iterator<Item = &Sample> + Clone {
        self.history.iter()
    }

    /// The newest history point taken at least `age_ms` before `latest`.
    fn sample_before(&self, age_ms: u64) -> Option<&Sample> {
        let cutoff = self.latest.at_ms.checked_sub(age_ms)?;
        self.history.iter().rev().find(|s| s.at_ms <= cutoff)
    }

    /// Where a look-back of `window_ms` starts, and how long it actually
    /// spans (a little over `window_ms`, given the history spacing). For
    /// rates computed from cumulative counters.
    pub fn window(&self, window_ms: u64) -> Option<(&Sample, u64)> {
        let start = self.sample_before(window_ms)?;
        let span_ms = self
            .latest
            .at_ms
            .checked_sub(start.at_ms)
            .filter(|&ms| ms > 0)?;
        Some((start, span_ms))
    }

    /// The parent must be older than the child; that rules out a recycled
    /// PID posing as the parent.
    fn is_parent_of(&self, child: &TrackedProcess) -> bool {
        self.key.pid == child.parent_pid && self.key.create_time < child.key.create_time
    }
}

#[derive(Debug)]
pub struct Tracker {
    processes: HashMap<u32, TrackedProcess>,
    generation: u64,
    history_len: usize,
}

impl Tracker {
    /// `longest_window_ms` is the furthest back any detector looks.
    pub fn new(longest_window_ms: u64) -> Self {
        let mut tracker = Self {
            processes: HashMap::new(),
            generation: 0,
            history_len: 0,
        };
        tracker.set_longest_window(longest_window_ms);
        tracker
    }

    pub fn set_longest_window(&mut self, longest_window_ms: u64) {
        // +2: a point at or before the window start, plus slack for timer jitter.
        let steps = usize::try_from(longest_window_ms / HISTORY_STEP_MS).unwrap_or(usize::MAX);
        self.history_len = steps.saturating_add(2);
        for process in self.processes.values_mut() {
            let excess = process.history.len().saturating_sub(self.history_len);
            process.history.drain(..excess);
        }
    }

    /// Ingests one snapshot taken at `now_ms`.
    pub fn update<'a>(&mut self, now_ms: u64, snapshot: impl IntoIterator<Item = RawProcess<'a>>) {
        self.generation += 1;
        let generation = self.generation;
        for raw in snapshot {
            let sample = Sample::new(now_ms, &raw);
            match self.processes.get_mut(&raw.key.pid) {
                Some(known) if known.key == raw.key => {
                    known.observe(sample, generation, self.history_len)
                }
                // A new process, or a recycled PID that now names a different one.
                _ => {
                    self.processes
                        .insert(raw.key.pid, TrackedProcess::new(&raw, sample, generation));
                }
            }
        }
        self.processes
            .retain(|_, process| process.last_seen_generation == generation);
        self.mark_orphans(now_ms);
    }

    pub fn get(&self, key: ProcessKey) -> Option<&TrackedProcess> {
        self.processes.get(&key.pid).filter(|p| p.key == key)
    }

    pub fn contains(&self, key: ProcessKey) -> bool {
        self.get(key).is_some()
    }

    pub fn processes(&self) -> impl Iterator<Item = &TrackedProcess> {
        self.processes.values()
    }

    /// Live descendants of `root`, each listed after its ancestors. Killing
    /// in this order takes out a supervisor before it can respawn a child.
    pub fn descendants(&self, root: ProcessKey) -> Vec<ProcessKey> {
        let Some(root) = self.get(root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        let mut pending = vec![root];
        while let Some(parent) = pending.pop() {
            for child in self.processes.values().filter(|c| parent.is_parent_of(c)) {
                found.push(child.key);
                pending.push(child);
            }
        }
        found
    }

    /// Stamps processes whose parent has disappeared since the last check.
    fn mark_orphans(&mut self, now_ms: u64) {
        let newly_orphaned: Vec<u32> = self
            .processes
            .values()
            .filter(|p| p.orphaned_since_ms.is_none() && !self.has_live_parent(p))
            .map(|p| p.key.pid)
            .collect();
        for pid in newly_orphaned {
            if let Some(process) = self.processes.get_mut(&pid) {
                process.orphaned_since_ms = Some(now_ms);
            }
        }
    }

    fn has_live_parent(&self, child: &TrackedProcess) -> bool {
        self.processes
            .get(&child.parent_pid)
            .is_some_and(|parent| parent.is_parent_of(child))
    }
}

#[cfg(test)]
pub mod testing {
    //! Builds snapshots from plain descriptions for detector and tracker tests.

    use super::Tracker;
    use crate::sampler::{ProcessKey, RawProcess};

    /// The interactive session fakes run in unless a test says otherwise.
    pub const USER_SESSION: u32 = 1;

    #[derive(Debug, Clone)]
    pub struct Fake {
        pub pid: u32,
        pub create_time: i64,
        pub parent_pid: u32,
        pub session_id: u32,
        pub name: &'static str,
        pub cpu_time: u64,
        pub private_bytes: u64,
        pub io_read: u64,
        pub io_write: u64,
        pub handles: u32,
        pub threads: u32,
    }

    impl Fake {
        /// Creation time grows with the PID, so lower PIDs are older.
        pub fn new(pid: u32, name: &'static str) -> Self {
            Self {
                pid,
                create_time: i64::from(pid) * 1000,
                parent_pid: 0,
                session_id: USER_SESSION,
                name,
                cpu_time: 0,
                private_bytes: 0,
                io_read: 0,
                io_write: 0,
                handles: 100,
                threads: 4,
            }
        }

        pub fn child_of(mut self, parent: u32) -> Self {
            self.parent_pid = parent;
            self
        }

        pub fn key(&self) -> ProcessKey {
            ProcessKey {
                pid: self.pid,
                create_time: self.create_time,
            }
        }
    }

    pub fn feed(tracker: &mut Tracker, now_ms: u64, processes: &[Fake]) {
        let names: Vec<Vec<u16>> = processes
            .iter()
            .map(|p| p.name.encode_utf16().collect())
            .collect();
        let raw = processes.iter().zip(&names).map(|(p, name)| RawProcess {
            key: p.key(),
            parent_pid: p.parent_pid,
            session_id: p.session_id,
            cpu_time: p.cpu_time,
            private_bytes: p.private_bytes,
            handle_count: p.handles,
            thread_count: p.threads,
            io_read_bytes: p.io_read,
            io_write_bytes: p.io_write,
            name,
        });
        tracker.update(now_ms, raw);
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{Fake, feed};
    use super::*;

    #[test]
    fn history_is_coarse_and_bounded() {
        let mut tracker = Tracker::new(5 * 60_000);
        let p = Fake::new(10, "a.exe");
        for tick in 0..120 {
            feed(&mut tracker, tick * 10_000, std::slice::from_ref(&p));
        }
        let tracked = tracker.get(p.key()).unwrap();
        let points: Vec<_> = tracked.history().collect();
        assert_eq!(points.len(), 5 * 2 + 2);
        assert!(
            points
                .windows(2)
                .all(|w| w[1].at_ms - w[0].at_ms >= HISTORY_STEP_MS)
        );
        assert_eq!(tracked.latest.at_ms, 119 * 10_000);
    }

    #[test]
    fn shrinking_the_window_trims_history() {
        let mut tracker = Tracker::new(30 * 60_000);
        let p = Fake::new(10, "a.exe");
        for tick in 0..200 {
            feed(&mut tracker, tick * 10_000, std::slice::from_ref(&p));
        }
        tracker.set_longest_window(60_000);
        assert_eq!(tracker.get(p.key()).unwrap().history().count(), 4);
    }

    #[test]
    fn first_seen_survives_history_trimming() {
        let mut tracker = Tracker::new(60_000);
        let p = Fake::new(10, "a.exe");
        for tick in 1..100 {
            feed(&mut tracker, tick * 10_000, std::slice::from_ref(&p));
        }
        let tracked = tracker.get(p.key()).unwrap();
        assert_eq!(tracked.first_seen_ms, 10_000);
        assert!(tracked.history().next().unwrap().at_ms > 10_000);
    }

    #[test]
    fn recycled_pid_starts_a_fresh_record() {
        let mut tracker = Tracker::new(60_000);
        let old = Fake::new(10, "old.exe");
        let new = Fake {
            create_time: old.create_time + 1,
            ..Fake::new(10, "new.exe")
        };
        feed(&mut tracker, 0, std::slice::from_ref(&old));
        feed(&mut tracker, 60_000, std::slice::from_ref(&new));
        assert!(!tracker.contains(old.key()));
        let tracked = tracker.get(new.key()).unwrap();
        assert_eq!(tracked.name, "new.exe");
        assert_eq!(tracked.history().count(), 1);
    }

    #[test]
    fn exited_processes_are_dropped() {
        let mut tracker = Tracker::new(60_000);
        let (a, b) = (Fake::new(1, "a.exe"), Fake::new(2, "b.exe"));
        feed(&mut tracker, 0, &[a.clone(), b.clone()]);
        feed(&mut tracker, 10_000, std::slice::from_ref(&a));
        assert!(tracker.contains(a.key()));
        assert!(!tracker.contains(b.key()));
    }

    #[test]
    fn orphan_is_stamped_when_parent_exits() {
        let mut tracker = Tracker::new(60_000);
        let parent = Fake::new(1, "cmd.exe");
        let child = Fake::new(2, "node.exe").child_of(1);
        feed(&mut tracker, 0, &[parent, child.clone()]);
        assert_eq!(tracker.get(child.key()).unwrap().orphaned_since_ms, None);
        feed(&mut tracker, 10_000, std::slice::from_ref(&child));
        feed(&mut tracker, 20_000, std::slice::from_ref(&child));
        assert_eq!(
            tracker.get(child.key()).unwrap().orphaned_since_ms,
            Some(10_000)
        );
    }

    #[test]
    fn newer_process_with_parents_pid_is_not_the_parent() {
        let mut tracker = Tracker::new(60_000);
        let child = Fake::new(5, "node.exe").child_of(3);
        let impostor = Fake {
            create_time: child.create_time + 1,
            ..Fake::new(3, "cmd.exe")
        };
        feed(&mut tracker, 0, &[child.clone(), impostor]);
        assert_eq!(tracker.get(child.key()).unwrap().orphaned_since_ms, Some(0));
    }

    #[test]
    fn descendants_come_after_their_ancestors() {
        let mut tracker = Tracker::new(60_000);
        let tree = [
            Fake::new(1, "npm.exe"),
            Fake::new(2, "node.exe").child_of(1),
            Fake::new(3, "esbuild.exe").child_of(2),
            Fake::new(4, "node.exe").child_of(1),
            Fake::new(9, "unrelated.exe"),
        ];
        feed(&mut tracker, 0, &tree);
        let pids: Vec<u32> = tracker
            .descendants(tree[0].key())
            .iter()
            .map(|k| k.pid)
            .collect();
        assert_eq!(pids.len(), 3);
        assert!(pids.contains(&4));
        let pos = |pid| pids.iter().position(|&p| p == pid).unwrap();
        assert!(pos(2) < pos(3));
        assert!(tracker.descendants(tree[4].key()).is_empty());
    }
}
