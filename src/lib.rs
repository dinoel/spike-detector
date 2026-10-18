//! spike-detector: watches for runaway background processes on Windows and
//! tells you about them.
//!
//! Everything lives in this library; two executables use it: the tray monitor
//! (`src/main.rs`) and the settings and history window (`ui/`). Keeping the
//! window separate means the monitor never loads a GUI toolkit. The public
//! API is what those two need; the rest stays internal.

pub mod config;
pub mod history;
pub mod monitor;
pub mod win;

mod alerts;
mod app;
mod detect;
mod sampler;
mod tracker;

pub use detect::FindingKind;
