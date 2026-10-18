//! The Settings tab: a form over config.toml. Saving rewrites only the values
//! that changed, and the monitor picks the file up on its next check.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use eframe::egui::{self, Color32, DragValue, RichText, TextEdit, Ui};
use spike_detector::config::{self, Config};
use spike_detector::win::shell;

use crate::config_file;

pub struct Settings {
    path: PathBuf,
    /// What the file says.
    saved: Config,
    /// What the form shows. List fields live in `lists` while being edited.
    draft: Config,
    lists: Lists,
    file_modified: Option<SystemTime>,
    status: Status,
}

/// List settings as editable text, one exe name per line.
#[derive(Default)]
struct Lists {
    ignore: String,
    orphan: String,
    swarm: String,
}

enum Status {
    Clean,
    Saved,
    /// Someone else changed the file while the form had unsaved edits.
    ChangedOnDisk,
    Error(String),
}

impl Settings {
    pub fn load(path: PathBuf) -> Self {
        let mut settings = Self {
            path,
            saved: Config::default(),
            draft: Config::default(),
            lists: Lists::default(),
            file_modified: None,
            status: Status::Clean,
        };
        settings.reload();
        settings
    }

    pub fn config_path(&self) -> &Path {
        &self.path
    }

    /// Lower-cased exe names the saved settings ignore.
    pub fn ignored(&self) -> HashSet<String> {
        self.saved.ignore.iter().map(|n| n.to_lowercase()).collect()
    }

    pub fn is_dirty(&self) -> bool {
        self.edited() != self.saved
    }

    /// Picks up edits made elsewhere (the tray's "Always ignore", a text
    /// editor), unless that would throw away unsaved changes in the form.
    pub fn refresh_from_disk(&mut self) {
        if config::modified(&self.path) == self.file_modified {
            return;
        }
        if self.is_dirty() {
            self.status = Status::ChangedOnDisk;
        } else {
            self.reload();
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.toolbar(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| self.form(ui));
    }

    fn reload(&mut self) {
        self.file_modified = config::modified(&self.path);
        match Config::load_or_create(&self.path) {
            Ok(config) => {
                self.saved = config.clone();
                self.set_draft(config);
                self.status = Status::Clean;
            }
            Err(e) => self.status = Status::Error(format!("{}: {e}", self.path.display())),
        }
    }

    fn set_draft(&mut self, config: Config) {
        self.lists = Lists {
            ignore: config.ignore.join("\n"),
            orphan: config.orphan.names.join("\n"),
            swarm: config.swarm.names.join("\n"),
        };
        self.draft = config;
    }

    /// The form's content as a config.
    fn edited(&self) -> Config {
        let mut config = self.draft.clone();
        config.ignore = names(&self.lists.ignore);
        config.orphan.names = names(&self.lists.orphan);
        config.swarm.names = names(&self.lists.swarm);
        config
    }

    fn save(&mut self) {
        let edited = self.edited();
        match write(&self.path, &edited) {
            Ok(()) => {
                self.file_modified = config::modified(&self.path);
                self.saved = edited.clone();
                self.set_draft(edited);
                self.status = Status::Saved;
            }
            Err(e) => self.status = Status::Error(e),
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let dirty = self.is_dirty();
        ui.horizontal(|ui| {
            if ui.add_enabled(dirty, egui::Button::new("Save")).clicked() {
                self.save();
            }
            if ui.add_enabled(dirty, egui::Button::new("Revert")).clicked() {
                self.set_draft(self.saved.clone());
                self.status = Status::Clean;
            }
            if matches!(self.status, Status::ChangedOnDisk)
                && ui.button("Reload from file").clicked()
            {
                self.reload();
            }
            if ui
                .button("Open file")
                .on_hover_text(self.path.display().to_string())
                .clicked()
                && let Err(e) = shell::open(&self.path)
            {
                self.status = Status::Error(e.to_string());
            }
            ui.add_space(12.0);
            let visuals = ui.visuals();
            match (&self.status, dirty) {
                (Status::Error(e), _) => {
                    ui.colored_label(visuals.error_fg_color, e);
                }
                (Status::ChangedOnDisk, _) => {
                    ui.colored_label(
                        visuals.warn_fg_color,
                        "The file changed on disk. Reload to see it, or save to overwrite it.",
                    );
                }
                (_, true) => {
                    ui.weak("Unsaved changes");
                }
                (Status::Saved, false) => {
                    ui.colored_label(
                        Color32::from_rgb(0x2e, 0xa0, 0x43),
                        "Saved. The monitor applies it on its next check.",
                    );
                }
                (Status::Clean, false) => {}
            }
        });
    }

    fn form(&mut self, ui: &mut Ui) {
        let lists = &mut self.lists;
        let d = &mut self.draft;

        section(
            ui,
            "General",
            None,
            "How often processes are checked, and which ones are never reported.",
            |ui| {
                number(
                    ui,
                    "Check every",
                    &mut d.poll_interval_secs,
                    "poll_interval_secs",
                    " s",
                );
                number(
                    ui,
                    "Repeat a reminder after",
                    &mut d.renotify_after_minutes,
                    "renotify_after_minutes",
                    " min",
                );
                names_field(ui, "Never report", &mut lists.ignore);
            },
        );

        let c = &mut d.cpu_hog;
        section(
            ui,
            "High CPU",
            Some(&mut c.enabled),
            "A process keeps a large share of a core busy for minutes.",
            |ui| {
                number(
                    ui,
                    "Averaging window",
                    &mut c.window_minutes,
                    "cpu_hog.window_minutes",
                    " min",
                );
                number(
                    ui,
                    "Threshold",
                    &mut c.min_core_percent,
                    "cpu_hog.min_core_percent",
                    " % of a core",
                );
                toggle(ui, "Skip the window in front", &mut c.skip_foreground);
            },
        );

        let c = &mut d.idle_busy;
        section(
            ui,
            "Busy while you were away",
            Some(&mut c.enabled),
            "No keyboard or mouse input for a while, yet a process in your session keeps working. Services are left out.",
            |ui| {
                number(
                    ui,
                    "Away for at least",
                    &mut c.idle_minutes,
                    "idle_busy.idle_minutes",
                    " min",
                );
                number(
                    ui,
                    "Threshold",
                    &mut c.min_core_percent,
                    "idle_busy.min_core_percent",
                    " % of a core",
                );
            },
        );

        let c = &mut d.orphan;
        section(
            ui,
            "Orphaned dev tools",
            Some(&mut c.enabled),
            "A dev tool still runs after its terminal or IDE is gone.",
            |ui| {
                number(
                    ui,
                    "Without a parent for",
                    &mut c.min_minutes,
                    "orphan.min_minutes",
                    " min",
                );
                names_field(ui, "Watched processes", &mut lists.orphan);
            },
        );

        let c = &mut d.swarm;
        section(
            ui,
            "Too many instances",
            Some(&mut c.enabled),
            "Copies of one dev tool pile up and nobody cleans them up.",
            |ui| {
                number(
                    ui,
                    "At least",
                    &mut c.min_instances,
                    "swarm.min_instances",
                    " instances",
                );
                number(
                    ui,
                    "Each alive for",
                    &mut c.min_minutes,
                    "swarm.min_minutes",
                    " min",
                );
                names_field(ui, "Watched processes", &mut lists.swarm);
            },
        );

        let c = &mut d.mem_leak;
        section(
            ui,
            "Memory leak",
            Some(&mut c.enabled),
            "Private memory grows steadily and never comes back down.",
            |ui| {
                number(
                    ui,
                    "Window",
                    &mut c.window_minutes,
                    "mem_leak.window_minutes",
                    " min",
                );
                number(
                    ui,
                    "Growth at least",
                    &mut c.min_growth_mb,
                    "mem_leak.min_growth_mb",
                    " MB",
                );
                fraction(ui, "Steadiness (R²)", &mut c.min_r2);
            },
        );

        let c = &mut d.handle_leak;
        section(
            ui,
            "Handle leak",
            Some(&mut c.enabled),
            "Handles or threads keep growing and are never released.",
            |ui| {
                number(
                    ui,
                    "Window",
                    &mut c.window_minutes,
                    "handle_leak.window_minutes",
                    " min",
                );
                number(
                    ui,
                    "Handle growth at least",
                    &mut c.min_handle_growth,
                    "handle_leak.min_handle_growth",
                    "",
                );
                number(
                    ui,
                    "Thread growth at least",
                    &mut c.min_thread_growth,
                    "handle_leak.min_thread_growth",
                    "",
                );
                fraction(ui, "Steadiness (R²)", &mut c.min_r2);
            },
        );

        let c = &mut d.io_heavy;
        section(
            ui,
            "Heavy file I/O",
            Some(&mut c.enabled),
            "A process pushes lots of data through its files for minutes. Reads from the file cache count too.",
            |ui| {
                number(
                    ui,
                    "Averaging window",
                    &mut c.window_minutes,
                    "io_heavy.window_minutes",
                    " min",
                );
                number(
                    ui,
                    "Threshold",
                    &mut c.min_mb_per_sec,
                    "io_heavy.min_mb_per_sec",
                    " MB/s",
                );
                toggle(ui, "Skip the window in front", &mut c.skip_foreground);
            },
        );
    }
}

/// Saves `config` into the file at `path`, after the same validation the
/// monitor applies, so nothing invalid ever reaches it.
fn write(path: &Path, config: &Config) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let updated = config_file::apply(&text, config)?;
    Config::parse(&updated).map_err(|e| e.to_string())?;
    fs::write(path, updated).map_err(|e| e.to_string())
}

/// Exe names from list text: one per line (commas work too), blanks dropped.
fn names(text: &str) -> Vec<String> {
    text.split(['\n', ','])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(String::from)
        .collect()
}

/// A titled block of settings; `enabled`, if given, greys out the fields when off.
fn section(
    ui: &mut Ui,
    title: &str,
    enabled: Option<&mut bool>,
    about: &str,
    fields: impl FnOnce(&mut Ui),
) {
    ui.add_space(10.0);
    let on = match enabled {
        Some(enabled) => {
            ui.checkbox(enabled, RichText::new(title).strong());
            *enabled
        }
        None => {
            ui.label(RichText::new(title).strong());
            true
        }
    };
    ui.indent(title, |ui| {
        ui.weak(about);
        ui.add_enabled_ui(on, |ui| {
            // The same minimum label width in every section lines the
            // values up into one column down the whole form.
            egui::Grid::new(title)
                .num_columns(2)
                .min_col_width(190.0)
                .spacing([24.0, 6.0])
                .show(ui, fields);
        });
    });
}

fn number(ui: &mut Ui, label: &str, value: &mut u32, setting: &str, suffix: &str) {
    ui.label(label);
    ui.add(
        DragValue::new(value)
            .range(config::limits(setting))
            .suffix(suffix),
    );
    ui.end_row();
}

fn fraction(ui: &mut Ui, label: &str, value: &mut f64) {
    ui.label(label)
        .on_hover_text("1.0 means growth along a perfectly straight line.");
    ui.add(
        DragValue::new(value)
            .range(0.0..=1.0)
            .speed(0.01)
            .fixed_decimals(2),
    );
    ui.end_row();
}

fn toggle(ui: &mut Ui, label: &str, value: &mut bool) {
    ui.label(label);
    ui.checkbox(value, "");
    ui.end_row();
}

fn names_field(ui: &mut Ui, label: &str, text: &mut String) {
    ui.label(label)
        .on_hover_text("One exe name per line, case doesn't matter.");
    ui.add(
        TextEdit::multiline(text)
            .desired_rows(3)
            .desired_width(280.0)
            .hint_text("e.g. node.exe"),
    );
    ui.end_row();
}

#[cfg(test)]
mod tests {
    use super::names;

    #[test]
    fn list_text_splits_on_lines_and_commas() {
        assert_eq!(
            names(" node.exe \n\nMSBuild.exe, dotnet.exe,\n"),
            ["node.exe", "MSBuild.exe", "dotnet.exe"]
        );
        assert!(names("  \n ").is_empty());
    }
}
