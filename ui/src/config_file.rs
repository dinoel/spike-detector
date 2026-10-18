//! Writes edited settings back into the text of config.toml. Only values that
//! changed are touched; comments, order and the formatting of everything
//! else stay as the user left them.

use spike_detector::config::Config;
use toml_edit::{Array, DocumentMut, Item, TableLike, Value};

/// `text` with the values of `config` written in.
pub fn apply(text: &str, config: &Config) -> Result<String, String> {
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| e.to_string())?;

    let root = doc.as_table_mut();
    set(root, "poll_interval_secs", &config.poll_interval_secs);
    set(
        root,
        "renotify_after_minutes",
        &config.renotify_after_minutes,
    );
    set(root, "ignore", &config.ignore);

    let s = &config.cpu_hog;
    let t = section(&mut doc, "cpu_hog")?;
    set(t, "enabled", &s.enabled);
    set(t, "window_minutes", &s.window_minutes);
    set(t, "min_core_percent", &s.min_core_percent);
    set(t, "skip_foreground", &s.skip_foreground);

    let s = &config.idle_busy;
    let t = section(&mut doc, "idle_busy")?;
    set(t, "enabled", &s.enabled);
    set(t, "idle_minutes", &s.idle_minutes);
    set(t, "min_core_percent", &s.min_core_percent);

    let s = &config.orphan;
    let t = section(&mut doc, "orphan")?;
    set(t, "enabled", &s.enabled);
    set(t, "min_minutes", &s.min_minutes);
    set(t, "names", &s.names);

    let s = &config.swarm;
    let t = section(&mut doc, "swarm")?;
    set(t, "enabled", &s.enabled);
    set(t, "min_instances", &s.min_instances);
    set(t, "min_minutes", &s.min_minutes);
    set(t, "names", &s.names);

    let s = &config.mem_leak;
    let t = section(&mut doc, "mem_leak")?;
    set(t, "enabled", &s.enabled);
    set(t, "window_minutes", &s.window_minutes);
    set(t, "min_growth_mb", &s.min_growth_mb);
    set(t, "min_r2", &s.min_r2);

    let s = &config.handle_leak;
    let t = section(&mut doc, "handle_leak")?;
    set(t, "enabled", &s.enabled);
    set(t, "window_minutes", &s.window_minutes);
    set(t, "min_handle_growth", &s.min_handle_growth);
    set(t, "min_thread_growth", &s.min_thread_growth);
    set(t, "min_r2", &s.min_r2);

    let s = &config.io_heavy;
    let t = section(&mut doc, "io_heavy")?;
    set(t, "enabled", &s.enabled);
    set(t, "window_minutes", &s.window_minutes);
    set(t, "min_mb_per_sec", &s.min_mb_per_sec);
    set(t, "skip_foreground", &s.skip_foreground);

    Ok(doc.to_string())
}

/// The `[name]` table, created at the end of the file if it's missing.
fn section<'a>(doc: &'a mut DocumentMut, name: &str) -> Result<&'a mut dyn TableLike, String> {
    doc.entry(name)
        .or_insert(toml_edit::table())
        .as_table_like_mut()
        .ok_or_else(|| format!("`{name}` in the settings file is not a table"))
}

/// Writes `setting` under `key` unless the file already says the same.
/// A replaced value keeps its surrounding whitespace and trailing comment;
/// the key keeps the comment lines above it.
fn set(table: &mut dyn TableLike, key: &str, setting: &impl Setting) {
    match table.get_mut(key) {
        Some(Item::Value(old)) if setting.matches(old) => {}
        Some(Item::Value(old)) => {
            let decor = old.decor().clone();
            *old = setting.to_value();
            *old.decor_mut() = decor;
        }
        _ => {
            table.insert(key, Item::Value(setting.to_value()));
        }
    }
}

/// A Rust value that can be compared with and written as a TOML value.
trait Setting {
    fn to_value(&self) -> Value;
    fn matches(&self, value: &Value) -> bool;
}

impl Setting for u32 {
    fn to_value(&self) -> Value {
        Value::from(i64::from(*self))
    }
    fn matches(&self, value: &Value) -> bool {
        value.as_integer() == Some(i64::from(*self))
    }
}

impl Setting for bool {
    fn to_value(&self) -> Value {
        Value::from(*self)
    }
    fn matches(&self, value: &Value) -> bool {
        value.as_bool() == Some(*self)
    }
}

impl Setting for f64 {
    fn to_value(&self) -> Value {
        Value::from(*self)
    }
    fn matches(&self, value: &Value) -> bool {
        value.as_float() == Some(*self)
    }
}

impl Setting for Vec<String> {
    fn to_value(&self) -> Value {
        Value::Array(self.iter().map(String::as_str).collect::<Array>())
    }
    fn matches(&self, value: &Value) -> bool {
        value.as_array().is_some_and(|array| {
            array.len() == self.len()
                && array
                    .iter()
                    .zip(self)
                    .all(|(old, new)| old.as_str() == Some(new.as_str()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEMPLATE: &str = include_str!("../../config.default.toml");

    #[test]
    fn unchanged_settings_leave_the_file_byte_for_byte() {
        assert_eq!(apply(TEMPLATE, &Config::default()).unwrap(), TEMPLATE);
    }

    #[test]
    fn changed_values_are_written_and_comments_survive() {
        let mut config = Config {
            poll_interval_secs: 15,
            ignore: vec!["Teams.exe".into()],
            ..Config::default()
        };
        config.cpu_hog.enabled = false;
        config.mem_leak.min_r2 = 0.9;
        config.swarm.names = vec!["node.exe".into(), "cargo.exe".into()];

        let text = apply(TEMPLATE, &config).unwrap();
        assert_eq!(Config::parse(&text).unwrap(), config);
        for comment in TEMPLATE.lines().filter(|l| l.trim_start().starts_with('#')) {
            assert!(text.contains(comment), "lost comment: {comment}");
        }
        assert_eq!(TEMPLATE.lines().count(), text.lines().count());
    }

    #[test]
    fn missing_keys_and_sections_are_added() {
        let mut config = Config::default();
        config.io_heavy.window_minutes = 20;
        config.renotify_after_minutes = 30;
        let text = apply("# minimal\npoll_interval_secs = 30\n", &config).unwrap();
        assert!(text.starts_with("# minimal\n"), "{text}");
        assert_eq!(Config::parse(&text).unwrap(), config);
    }

    #[test]
    fn invalid_toml_is_reported() {
        assert!(apply("poll_interval_secs = = 3", &Config::default()).is_err());
    }
}
