//! User settings, stored as a commented TOML file in the app's data folder.

use std::fmt;
use std::fs;
use std::io;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::Deserialize;

/// Written on first run. It doubles as documentation, so a test keeps it in
/// sync with [`Config::default`].
const TEMPLATE: &str = include_str!("../config.default.toml");

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub poll_interval_secs: u32,
    pub renotify_after_minutes: u32,
    /// Exe names never reported, compared case-insensitively.
    pub ignore: Vec<String>,
    pub cpu_hog: CpuHogConfig,
    pub idle_busy: IdleBusyConfig,
    pub orphan: OrphanConfig,
    pub swarm: SwarmConfig,
    pub mem_leak: MemLeakConfig,
    pub handle_leak: HandleLeakConfig,
    pub io_heavy: IoHeavyConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CpuHogConfig {
    pub enabled: bool,
    pub window_minutes: u32,
    /// Average load over the window, in percent of one core.
    pub min_core_percent: u32,
    pub skip_foreground: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IdleBusyConfig {
    pub enabled: bool,
    /// How long the user must have been away; also the averaging window.
    pub idle_minutes: u32,
    pub min_core_percent: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OrphanConfig {
    pub enabled: bool,
    pub min_minutes: u32,
    /// Exe names to watch, compared case-insensitively.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SwarmConfig {
    pub enabled: bool,
    pub min_instances: u32,
    /// Only instances alive at least this long are counted.
    pub min_minutes: u32,
    /// Exe names to watch, compared case-insensitively.
    pub names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MemLeakConfig {
    pub enabled: bool,
    pub window_minutes: u32,
    pub min_growth_mb: u32,
    /// Minimum coefficient of determination of the growth trend.
    pub min_r2: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HandleLeakConfig {
    pub enabled: bool,
    pub window_minutes: u32,
    pub min_handle_growth: u32,
    pub min_thread_growth: u32,
    /// Minimum coefficient of determination of the growth trend.
    pub min_r2: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct IoHeavyConfig {
    pub enabled: bool,
    pub window_minutes: u32,
    /// Average reads + writes over the window.
    pub min_mb_per_sec: u32,
    pub skip_foreground: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            poll_interval_secs: 30,
            renotify_after_minutes: 60,
            ignore: Vec::new(),
            cpu_hog: CpuHogConfig::default(),
            idle_busy: IdleBusyConfig::default(),
            orphan: OrphanConfig::default(),
            swarm: SwarmConfig::default(),
            mem_leak: MemLeakConfig::default(),
            handle_leak: HandleLeakConfig::default(),
            io_heavy: IoHeavyConfig::default(),
        }
    }
}

impl Default for CpuHogConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_minutes: 5,
            min_core_percent: 80,
            skip_foreground: true,
        }
    }
}

impl Default for IdleBusyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_minutes: 10,
            min_core_percent: 25,
        }
    }
}

impl Default for OrphanConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_minutes: 30,
            names: dev_tools(),
        }
    }
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            min_instances: 25,
            min_minutes: 10,
            names: dev_tools(),
        }
    }
}

impl Default for MemLeakConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_minutes: 30,
            min_growth_mb: 300,
            min_r2: 0.8,
        }
    }
}

impl Default for HandleLeakConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_minutes: 30,
            min_handle_growth: 2000,
            min_thread_growth: 100,
            min_r2: 0.8,
        }
    }
}

impl Default for IoHeavyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            window_minutes: 10,
            min_mb_per_sec: 10,
            skip_foreground: true,
        }
    }
}

/// Dev tools that tend to outlive their terminal or pile up.
fn dev_tools() -> Vec<String> {
    let names = [
        "node.exe",
        "MSBuild.exe",
        "dotnet.exe",
        "esbuild.exe",
        "python.exe",
        "java.exe",
    ];
    names.map(String::from).to_vec()
}

impl Config {
    /// Reads the config, first writing the commented template if there is no
    /// file yet.
    pub fn load_or_create(path: &Path) -> Result<Self, ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::write(path, TEMPLATE).map_err(ConfigError::Io)?;
                Self::parse(TEMPLATE)
            }
            Err(e) => Err(ConfigError::Io(e)),
        }
    }

    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let config: Self = toml::from_str(text).map_err(|e| ConfigError::Syntax(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn poll_interval(&self) -> Duration {
        Duration::from_secs(self.poll_interval_secs.into())
    }

    /// The furthest back any enabled detector looks; bounds per-process history.
    pub fn longest_window_ms(&self) -> u64 {
        let windows = [
            (self.cpu_hog.enabled, self.cpu_hog.window_minutes),
            (self.idle_busy.enabled, self.idle_busy.idle_minutes),
            (self.mem_leak.enabled, self.mem_leak.window_minutes),
            (self.handle_leak.enabled, self.handle_leak.window_minutes),
            (self.io_heavy.enabled, self.io_heavy.window_minutes),
        ];
        windows
            .into_iter()
            .filter(|&(enabled, _)| enabled)
            .map(|(_, minutes)| minutes_to_ms(minutes))
            .max()
            .unwrap_or(0)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let values = [
            ("poll_interval_secs", self.poll_interval_secs),
            ("renotify_after_minutes", self.renotify_after_minutes),
            ("cpu_hog.window_minutes", self.cpu_hog.window_minutes),
            ("cpu_hog.min_core_percent", self.cpu_hog.min_core_percent),
            ("idle_busy.idle_minutes", self.idle_busy.idle_minutes),
            (
                "idle_busy.min_core_percent",
                self.idle_busy.min_core_percent,
            ),
            ("orphan.min_minutes", self.orphan.min_minutes),
            ("swarm.min_instances", self.swarm.min_instances),
            ("swarm.min_minutes", self.swarm.min_minutes),
            ("mem_leak.window_minutes", self.mem_leak.window_minutes),
            ("mem_leak.min_growth_mb", self.mem_leak.min_growth_mb),
            (
                "handle_leak.window_minutes",
                self.handle_leak.window_minutes,
            ),
            (
                "handle_leak.min_handle_growth",
                self.handle_leak.min_handle_growth,
            ),
            (
                "handle_leak.min_thread_growth",
                self.handle_leak.min_thread_growth,
            ),
            ("io_heavy.window_minutes", self.io_heavy.window_minutes),
            ("io_heavy.min_mb_per_sec", self.io_heavy.min_mb_per_sec),
        ];
        for (name, value) in values {
            let range = limits(name);
            if !range.contains(&value) {
                return Err(ConfigError::Invalid(format!(
                    "{name} must be between {} and {}, got {value}",
                    range.start(),
                    range.end()
                )));
            }
        }
        let fractions = [
            ("mem_leak.min_r2", self.mem_leak.min_r2),
            ("handle_leak.min_r2", self.handle_leak.min_r2),
        ];
        for (name, value) in fractions {
            if !(0.0..=1.0).contains(&value) {
                return Err(ConfigError::Invalid(format!(
                    "{name} must be between 0 and 1, got {value}"
                )));
            }
        }
        Ok(())
    }
}

/// Allowed range of an integer setting, by its dotted name as written in the
/// file. Shared by validation and the settings window's input fields.
///
/// # Panics
/// If `name` isn't an integer setting: a typo in the caller.
pub fn limits(name: &str) -> RangeInclusive<u32> {
    match name {
        "poll_interval_secs" => 1..=3600,
        "renotify_after_minutes" => 1..=10_080,
        "cpu_hog.window_minutes" | "idle_busy.idle_minutes" | "io_heavy.window_minutes" => 1..=1440,
        "cpu_hog.min_core_percent" | "idle_busy.min_core_percent" => 1..=10_000,
        "orphan.min_minutes" | "swarm.min_minutes" => 0..=10_080,
        "swarm.min_instances" => 2..=10_000,
        "mem_leak.window_minutes" | "handle_leak.window_minutes" => 5..=1440,
        "mem_leak.min_growth_mb" => 1..=1_000_000,
        "handle_leak.min_handle_growth" => 1..=10_000_000,
        "handle_leak.min_thread_growth" | "io_heavy.min_mb_per_sec" => 1..=100_000,
        _ => panic!("no limits defined for setting `{name}`"),
    }
}

/// Adds `name` to the top-level `ignore` list in the config file. Only that
/// list is rewritten; comments and everything else stay as they are.
pub fn add_ignored(path: &Path, name: &str) -> Result<(), ConfigError> {
    let text = fs::read_to_string(path).map_err(ConfigError::Io)?;
    let updated = with_ignored(&text, name)?;
    if updated != text {
        fs::write(path, updated).map_err(ConfigError::Io)?;
    }
    Ok(())
}

fn with_ignored(text: &str, name: &str) -> Result<String, ConfigError> {
    /// Just the part of the file we edit, with its position in the text.
    #[derive(Deserialize)]
    struct IgnoreList {
        ignore: Option<toml::Spanned<Vec<String>>>,
    }
    let doc: IgnoreList = toml::from_str(text).map_err(|e| ConfigError::Syntax(e.to_string()))?;
    let Some(list) = doc.ignore else {
        // Top-level keys must come before the first [table].
        let at = text
            .match_indices('\n')
            .map(|(i, _)| i + 1)
            .chain([0])
            .filter(|&i| text[i..].starts_with('['))
            .min()
            .unwrap_or(text.len());
        let line = format!("ignore = {}\n", toml_array(&[name.to_owned()]));
        return Ok(format!("{}{line}{}", &text[..at], &text[at..]));
    };
    if list
        .get_ref()
        .iter()
        .any(|n| n.to_lowercase() == name.to_lowercase())
    {
        return Ok(text.to_owned());
    }
    let mut names = list.get_ref().clone();
    names.push(name.to_owned());
    let span = list.span();
    Ok(format!(
        "{}{}{}",
        &text[..span.start],
        toml_array(&names),
        &text[span.end..]
    ))
}

fn toml_array(names: &[String]) -> String {
    let quoted: Vec<String> = names
        .iter()
        .map(|n| format!("\"{}\"", n.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect();
    format!("[{}]", quoted.join(", "))
}

pub fn minutes_to_ms(minutes: u32) -> u64 {
    u64::from(minutes) * 60_000
}

/// Last-modified time of `path`, used to notice edits without a file watcher.
pub fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Syntax(String),
    Invalid(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "cannot read the file: {e}"),
            Self::Syntax(e) => write!(f, "syntax error: {e}"),
            Self::Invalid(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for ConfigError {}

/// Where the app keeps its files: `%LOCALAPPDATA%\spike-detector`.
pub struct Paths {
    pub config: PathBuf,
    pub history: PathBuf,
}

impl Paths {
    pub fn resolve() -> io::Result<Self> {
        let base = std::env::var_os("LOCALAPPDATA")
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "LOCALAPPDATA is not set"))?;
        let dir = PathBuf::from(base).join("spike-detector");
        fs::create_dir_all(&dir)?;
        Ok(Self {
            config: dir.join("config.toml"),
            history: dir.join("history.jsonl"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_matches_defaults() {
        assert_eq!(Config::parse(TEMPLATE).unwrap(), Config::default());
    }

    #[test]
    fn missing_keys_fall_back_to_defaults() {
        let config = Config::parse("[cpu_hog]\nwindow_minutes = 2\n").unwrap();
        assert_eq!(config.cpu_hog.window_minutes, 2);
        assert_eq!(
            config.cpu_hog.min_core_percent,
            CpuHogConfig::default().min_core_percent
        );
        assert_eq!(config.orphan, OrphanConfig::default());
    }

    #[test]
    fn typo_in_key_is_rejected() {
        let err = Config::parse("poll_interval_sec = 5\n").unwrap_err();
        assert!(matches!(err, ConfigError::Syntax(_)), "{err}");
    }

    #[test]
    fn out_of_range_value_is_rejected() {
        for text in [
            "poll_interval_secs = 0\n",
            "[swarm]\nmin_instances = 1\n",
            "[mem_leak]\nmin_r2 = 1.5\n",
            "[handle_leak]\nmin_r2 = -0.1\n",
        ] {
            let err = Config::parse(text).unwrap_err();
            assert!(matches!(err, ConfigError::Invalid(_)), "{text}: {err}");
        }
    }

    #[test]
    fn adding_ignored_name_keeps_the_rest_of_the_file() {
        let updated = with_ignored(TEMPLATE, "PING.EXE").unwrap();
        assert_eq!(Config::parse(&updated).unwrap().ignore, ["PING.EXE"]);
        let changed: Vec<(&str, &str)> = TEMPLATE
            .lines()
            .zip(updated.lines())
            .filter(|(before, after)| before != after)
            .collect();
        assert_eq!(changed, [("ignore = []", r#"ignore = ["PING.EXE"]"#)]);
        assert_eq!(TEMPLATE.lines().count(), updated.lines().count());
    }

    #[test]
    fn adding_appends_and_skips_duplicates() {
        let text = "ignore = [\n  \"a.exe\",  # noisy\n]\n\n[cpu_hog]\nwindow_minutes = 2\n";
        let updated = with_ignored(text, "b.exe").unwrap();
        let config = Config::parse(&updated).unwrap();
        assert_eq!(config.ignore, ["a.exe", "b.exe"]);
        assert_eq!(config.cpu_hog.window_minutes, 2);
        assert_eq!(with_ignored(&updated, "B.EXE").unwrap(), updated);
    }

    #[test]
    fn adding_creates_missing_list_before_first_table() {
        let text = "# settings\npoll_interval_secs = 20\n\n[orphan]\nmin_minutes = 5\n";
        let updated = with_ignored(text, "a.exe").unwrap();
        assert!(
            updated.starts_with("# settings\npoll_interval_secs = 20\n\n"),
            "{updated}"
        );
        let config = Config::parse(&updated).unwrap();
        assert_eq!(config.ignore, ["a.exe"]);
        assert_eq!(config.orphan.min_minutes, 5);
        assert_eq!(
            Config::parse(&with_ignored("", "a.exe").unwrap())
                .unwrap()
                .ignore,
            ["a.exe"]
        );
    }

    #[test]
    fn longest_window_ignores_disabled_detectors() {
        let mut config = Config::default();
        assert_eq!(config.longest_window_ms(), minutes_to_ms(30));
        config.mem_leak.enabled = false;
        config.handle_leak.enabled = false;
        assert_eq!(config.longest_window_ms(), minutes_to_ms(10));
        config.idle_busy.enabled = false;
        config.io_heavy.enabled = false;
        assert_eq!(config.longest_window_ms(), minutes_to_ms(5));
        config.cpu_hog.enabled = false;
        assert_eq!(config.longest_window_ms(), 0);
    }
}
