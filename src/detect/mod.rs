//! Anomaly heuristics. Each tick every detector looks at the watched
//! processes and reports what it finds as [`Finding`]s.

mod cpu;
mod handle_leak;
mod io_heavy;
mod mem_leak;
mod orphan;
mod swarm;
mod trend;

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::sampler::ProcessKey;
use crate::tracker::TrackedProcess;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingKind {
    CpuHog,
    IdleBusy,
    Orphan,
    Swarm,
    MemLeak,
    HandleLeak,
    IoHeavy,
}

impl FindingKind {
    pub const ALL: [Self; 7] = [
        Self::CpuHog,
        Self::IdleBusy,
        Self::Orphan,
        Self::Swarm,
        Self::MemLeak,
        Self::HandleLeak,
        Self::IoHeavy,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::CpuHog => "high CPU",
            Self::IdleBusy => "busy while you were away",
            Self::Orphan => "orphaned process",
            Self::Swarm => "too many instances",
            Self::MemLeak => "memory leak",
            Self::HandleLeak => "handle leak",
            Self::IoHeavy => "heavy file I/O",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    /// The process the finding is about; for a swarm, its oldest instance.
    pub key: ProcessKey,
    pub kind: FindingKind,
    /// Human-readable evidence, e.g. "97% of a core on average over 5 min".
    pub summary: String,
}

/// Machine-wide facts detectors may need, gathered once per tick.
#[derive(Debug, Default)]
pub struct Context {
    pub foreground_pid: Option<u32>,
    /// Time since the last keyboard or mouse input.
    pub user_idle_ms: Option<u64>,
    /// The interactive user's session (the app's own).
    pub user_session: Option<u32>,
}

/// Looks at all watched processes once per tick.
pub trait Detector {
    fn scan(&self, processes: &[&TrackedProcess], ctx: &Context, findings: &mut Vec<Finding>);
}

/// A detector that judges each process on its own, as most do.
pub trait ProcessDetector {
    fn check(&self, process: &TrackedProcess, ctx: &Context) -> Option<Finding>;
}

impl<T: ProcessDetector> Detector for T {
    fn scan(&self, processes: &[&TrackedProcess], ctx: &Context, findings: &mut Vec<Finding>) {
        findings.extend(processes.iter().filter_map(|p| self.check(p, ctx)));
    }
}

pub fn from_config(config: &Config) -> Vec<Box<dyn Detector>> {
    let mut detectors: Vec<Box<dyn Detector>> = Vec::new();
    if let Some(cpu) = cpu::CpuLoad::new(&config.cpu_hog, &config.idle_busy) {
        detectors.push(Box::new(cpu));
    }
    if config.orphan.enabled {
        detectors.push(Box::new(orphan::Orphan::new(&config.orphan)));
    }
    if config.swarm.enabled {
        detectors.push(Box::new(swarm::Swarm::new(&config.swarm)));
    }
    if config.mem_leak.enabled {
        detectors.push(Box::new(mem_leak::MemLeak::new(&config.mem_leak)));
    }
    if config.handle_leak.enabled {
        detectors.push(Box::new(handle_leak::HandleLeak::new(&config.handle_leak)));
    }
    if config.io_heavy.enabled {
        detectors.push(Box::new(io_heavy::IoHeavy::new(&config.io_heavy)));
    }
    detectors
}

/// Kernel-made pseudo processes (Idle, System, Registry, Memory Compression,
/// Secure System). Nothing to act on, and Idle's "CPU time" is idle time.
pub fn is_kernel_process(process: &TrackedProcess) -> bool {
    process.key.pid <= 4 || (process.parent_pid == 4 && !process.name_lc.ends_with(".exe"))
}

/// Whole minutes, rounded, for summaries.
fn minutes(ms: u64) -> u64 {
    (ms + 30_000) / 60_000
}
