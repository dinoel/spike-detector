//! The application proper: one pass of the monitoring pipeline per timer
//! tick, the tray menu, and what each menu command does.

use std::borrow::Cow;
use std::collections::HashSet;
use std::io;
use std::time::{Instant, SystemTime};

use windows_sys::Win32::Foundation::HWND;

use crate::alerts::{Alert, Alerts};
use crate::config::{self, Config, Paths};
use crate::detect::{self, Context, Detector, Finding, FindingKind};
use crate::history::History;
use crate::sampler::{ProcessKey, Sampler};
use crate::tracker::{TrackedProcess, Tracker};
use crate::win::menu::Entry;
use crate::win::process::{self, ProcessError};
use crate::win::tray::{Notice, Status, Tray};
use crate::win::{autostart, shell, system, window};

const TIMER_ID: usize = 1;

/// Longest command line shown in a menu or a notification.
const MAX_COMMAND_LINE_CHARS: usize = 120;

pub enum Command {
    Kill(ProcessKey),
    KillTree(ProcessKey),
    /// Every live process with this lower-cased exe name.
    KillAllNamed(String),
    Reveal(String),
    Dismiss(ProcessKey, FindingKind),
    /// Exe name as shown; saved to the config's `ignore` list.
    IgnoreName(String),
    TogglePause,
    ToggleAutostart,
    OpenConfig,
    OpenHistory,
    Exit,
}

/// What the window procedure should do after a command.
pub enum Flow {
    Continue,
    Quit,
}

pub struct App {
    hwnd: HWND,
    paths: Paths,
    config: Config,
    config_modified: Option<SystemTime>,
    started: Instant,
    sampler: Sampler,
    tracker: Tracker,
    detectors: Vec<Box<dyn Detector>>,
    /// Lower-cased exe names from the config's `ignore` list.
    ignored: HashSet<String>,
    alerts: Alerts,
    history: History,
    tray: Tray,
    paused: bool,
    self_pid: u32,
    user_session: Option<u32>,
}

impl App {
    pub fn new(hwnd: HWND, paths: Paths, config: Config) -> Self {
        window::set_timer(hwnd, TIMER_ID, config.poll_interval());
        Self {
            hwnd,
            config_modified: config::modified(&paths.config),
            history: History::new(paths.history.clone()),
            paths,
            started: Instant::now(),
            sampler: Sampler::new(),
            tracker: Tracker::new(config.longest_window_ms()),
            detectors: detect::from_config(&config),
            ignored: lowercase_set(&config.ignore),
            alerts: Alerts::new(config::minutes_to_ms(config.renotify_after_minutes)),
            tray: Tray::new(hwnd),
            paused: false,
            self_pid: std::process::id(),
            user_session: system::current_session_id(),
            config,
        }
    }

    /// One monitoring pass.
    pub fn tick(&mut self) {
        self.reload_config_if_changed();
        if self.paused {
            return;
        }
        let now_ms = u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX);
        match self.sampler.take() {
            Ok(snapshot) => self.tracker.update(now_ms, snapshot.processes()),
            Err(e) => {
                eprintln!("snapshot failed: {e}");
                return;
            }
        }
        let ctx = Context {
            foreground_pid: process::foreground_pid(),
            user_idle_ms: system::user_idle_ms(),
            user_session: self.user_session,
        };
        let findings = self.detect(&ctx);
        let fresh = self
            .alerts
            .update(now_ms, findings, &self.tracker, process::details);
        for alert in &fresh {
            if let Err(e) = self.history.append(alert) {
                eprintln!("history: {e}");
            }
        }
        announce(&self.tray, &fresh);
        self.refresh_status();
    }

    pub fn on_taskbar_created(&mut self) {
        self.tray.restore();
    }

    pub fn menu(&self) -> Vec<Entry<Command>> {
        let mut entries: Vec<_> = if self.alerts.active().is_empty() {
            vec![Entry::note("No anomalies found")]
        } else {
            self.alerts
                .active()
                .iter()
                .map(|a| self.alert_menu(a))
                .collect()
        };
        entries.extend([
            Entry::Separator,
            Entry::Action {
                label: "Pause".to_owned(),
                command: Command::TogglePause,
                checked: self.paused,
            },
            Entry::Action {
                label: "Start with Windows".to_owned(),
                command: Command::ToggleAutostart,
                checked: autostart::is_enabled(),
            },
            Entry::action("Settings…", Command::OpenConfig),
            Entry::action("History…", Command::OpenHistory),
            Entry::Separator,
            Entry::action("Exit", Command::Exit),
        ]);
        entries
    }

    pub fn execute(&mut self, command: Command) -> Flow {
        match command {
            Command::Kill(key) => self.kill(&[key]),
            Command::KillTree(key) => {
                let mut keys = vec![key];
                keys.extend(self.tracker.descendants(key));
                self.kill(&keys);
            }
            Command::KillAllNamed(name) => {
                let keys: Vec<ProcessKey> = self.instances_of(&name).map(|p| p.key).collect();
                self.kill(&keys);
            }
            Command::Reveal(path) => self.report("Could not open Explorer", shell::reveal(&path)),
            Command::Dismiss(key, kind) => self.alerts.dismiss(key, kind),
            Command::IgnoreName(name) => self.ignore(&name),
            Command::TogglePause => self.set_paused(!self.paused),
            Command::ToggleAutostart => self.report(
                "Could not change autostart",
                autostart::set_enabled(!autostart::is_enabled()),
            ),
            Command::OpenConfig => {
                self.report("Could not open settings", shell::open(&self.paths.config))
            }
            Command::OpenHistory if !self.paths.history.exists() => {
                self.tray
                    .show_balloon(Notice::Info, "History is empty", "No notifications yet.");
            }
            Command::OpenHistory => {
                self.report("Could not open history", shell::open(&self.paths.history))
            }
            Command::Exit => return Flow::Quit,
        }
        self.refresh_status();
        Flow::Continue
    }

    fn detect(&self, ctx: &Context) -> Vec<Finding> {
        let watched: Vec<&TrackedProcess> = self
            .tracker
            .processes()
            .filter(|p| self.is_watched(p))
            .collect();
        let mut findings = Vec::new();
        for detector in &self.detectors {
            detector.scan(&watched, ctx, &mut findings);
        }
        findings
    }

    fn instances_of(&self, name_lc: &str) -> impl Iterator<Item = &TrackedProcess> {
        self.tracker
            .processes()
            .filter(move |p| p.name_lc == name_lc)
    }

    fn is_watched(&self, process: &TrackedProcess) -> bool {
        process.key.pid != self.self_pid
            && !detect::is_kernel_process(process)
            && !self.ignored.contains(&process.name_lc)
    }

    fn alert_menu(&self, alert: &Alert) -> Entry<Command> {
        let key = alert.finding.key;
        let command_line = alert
            .details
            .command_line
            .as_deref()
            .map_or(Cow::Borrowed("Command line unavailable"), |c| {
                ellipsize(c, MAX_COMMAND_LINE_CHARS)
            });
        let name_lc = alert.name.to_lowercase();
        let mut entries = vec![
            Entry::note(alert.finding.summary.clone()),
            Entry::note(command_line),
            Entry::Separator,
        ];
        // A swarm is about every instance; its key is just the oldest one.
        let hide_label = if alert.finding.kind == FindingKind::Swarm {
            let count = self.instances_of(&name_lc).count();
            entries.push(Entry::action(
                format!("End all {count} instances"),
                Command::KillAllNamed(name_lc.clone()),
            ));
            "Hide this alert"
        } else {
            entries.push(Entry::action("End process", Command::Kill(key)));
            let children = self.tracker.descendants(key).len();
            if children > 0 {
                entries.push(Entry::action(
                    format!("End process tree (+{children} children)"),
                    Command::KillTree(key),
                ));
            }
            "Hide while this process runs"
        };
        if let Some(path) = &alert.details.path {
            entries.push(Entry::action(
                "Show in Explorer",
                Command::Reveal(path.clone()),
            ));
        }
        entries.extend([
            Entry::Separator,
            Entry::action(hide_label, Command::Dismiss(key, alert.finding.kind)),
            Entry::action(
                format!("Always ignore {}", alert.name),
                Command::IgnoreName(alert.name.clone()),
            ),
        ]);
        Entry::Submenu {
            label: format!(
                "{} (PID {}): {}",
                alert.name,
                key.pid,
                alert.finding.kind.label()
            ),
            entries,
        }
    }

    /// Stops reporting `name` right away and saves it to the config, which
    /// the next tick then reloads with the same list.
    fn ignore(&mut self, name: &str) {
        let name_lc = name.to_lowercase();
        self.alerts.forget_name(&name_lc);
        self.ignored.insert(name_lc);
        if let Err(e) = config::add_ignored(&self.paths.config, name) {
            self.tray.show_balloon(
                Notice::Info,
                &format!("Could not save {name} to settings"),
                &format!("{e}\nIt stays ignored until the app restarts."),
            );
        }
    }

    /// Terminates `keys` in order. Keeps going past failures and reports the
    /// first one.
    fn kill(&mut self, keys: &[ProcessKey]) {
        let mut failure = None;
        for &key in keys {
            match process::terminate(key) {
                Ok(()) | Err(ProcessError::Gone) => self.alerts.forget_process(key),
                Err(e) => {
                    failure.get_or_insert((key, e));
                }
            }
        }
        if let Some((key, error)) = failure {
            let name = self.tracker.get(key).map_or("process", |p| p.name.as_str());
            self.tray.show_balloon(
                Notice::Info,
                "Could not end process",
                &format!("{name} (PID {}): {error}", key.pid),
            );
        }
    }

    fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if paused {
            window::kill_timer(self.hwnd, TIMER_ID);
        } else {
            window::set_timer(self.hwnd, TIMER_ID, self.config.poll_interval());
        }
    }

    fn reload_config_if_changed(&mut self) {
        let modified = config::modified(&self.paths.config);
        if modified == self.config_modified {
            return;
        }
        self.config_modified = modified;
        match Config::load_or_create(&self.paths.config) {
            Ok(config) => self.apply_config(config),
            Err(e) => self.tray.show_balloon(
                Notice::Info,
                "Settings error",
                &format!("{e}\nKeeping the previous settings."),
            ),
        }
    }

    fn apply_config(&mut self, config: Config) {
        self.tracker.set_longest_window(config.longest_window_ms());
        self.detectors = detect::from_config(&config);
        self.ignored = lowercase_set(&config.ignore);
        self.alerts
            .set_renotify(config::minutes_to_ms(config.renotify_after_minutes));
        if !self.paused {
            window::set_timer(self.hwnd, TIMER_ID, config.poll_interval());
        }
        self.config = config;
    }

    fn refresh_status(&mut self) {
        let status = match self.alerts.active().len() {
            _ if self.paused => Status::Paused,
            0 => Status::Idle,
            n => Status::Alerting(n),
        };
        self.tray.set_status(status);
    }

    fn report(&self, what: &str, result: io::Result<()>) {
        if let Err(e) = result {
            self.tray.show_balloon(Notice::Info, what, &e.to_string());
        }
    }
}

/// One notification per tick: detailed for a single alert, a list for several.
fn announce(tray: &Tray, alerts: &[&Alert]) {
    match alerts {
        [] => {}
        [alert] => {
            let title = format!(
                "{} (PID {}): {}",
                alert.name,
                alert.finding.key.pid,
                alert.finding.kind.label()
            );
            let mut text = alert.finding.summary.clone();
            if let Some(command_line) = &alert.details.command_line {
                text.push('\n');
                text.push_str(&ellipsize(command_line, MAX_COMMAND_LINE_CHARS));
            }
            tray.show_balloon(Notice::Anomaly, &title, &text);
        }
        many => {
            let title = format!("{} anomalies found", many.len());
            let lines: Vec<String> = many
                .iter()
                .map(|a| format!("{}: {}", a.name, a.finding.kind.label()))
                .collect();
            tray.show_balloon(Notice::Anomaly, &title, &lines.join("\n"));
        }
    }
}

fn lowercase_set(names: &[String]) -> HashSet<String> {
    names.iter().map(|n| n.to_lowercase()).collect()
}

/// Shortens `s` to at most `max_chars` characters, marking the cut with "…".
fn ellipsize(s: &str, max_chars: usize) -> Cow<'_, str> {
    match s.char_indices().nth(max_chars) {
        None => Cow::Borrowed(s),
        Some(_) => {
            let cut = s
                .char_indices()
                .nth(max_chars.saturating_sub(1))
                .map_or(0, |(i, _)| i);
            Cow::Owned(format!("{}…", &s[..cut]))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ellipsize;

    #[test]
    fn ellipsize_counts_characters_not_bytes() {
        assert_eq!(ellipsize("äöü", 3), "äöü");
        assert_eq!(ellipsize("äöüßé", 3), "äö…");
        assert_eq!(ellipsize("", 3), "");
    }
}
