//! The tray monitor: watches for runaway background processes (a node.exe
//! spinning a core, an MSBuild node nobody needs any more) and tells you
//! about them. What to do is up to you.

// Release builds run without a console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    spike_detector::monitor::run();
}
