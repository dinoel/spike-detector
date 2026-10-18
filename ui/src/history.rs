//! The History tab: every notification the monitor has shown, newest first,
//! with filters and a way to silence a noisy process for good.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use eframe::egui::{self, Label, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use spike_detector::FindingKind;
use spike_detector::config;
use spike_detector::history::{Entry, History};
use spike_detector::win::shell;

use crate::os;
use crate::settings::Settings;

const ROW_HEIGHT: f32 = 22.0;

pub struct HistoryView {
    log: History,
    path: PathBuf,
    /// Newest first.
    entries: Vec<Entry>,
    /// Indices into `entries` that pass the filters.
    visible: Vec<usize>,
    loaded_modified: Option<SystemTime>,
    kind: Option<FindingKind>,
    search: String,
    error: Option<String>,
}

impl HistoryView {
    pub fn new(path: PathBuf) -> Self {
        let mut view = Self {
            log: History::new(path.clone()),
            path,
            entries: Vec::new(),
            visible: Vec::new(),
            loaded_modified: None,
            kind: None,
            search: String::new(),
            error: None,
        };
        view.refresh_from_disk();
        view
    }

    /// Re-reads the log if the monitor has written to it since.
    pub fn refresh_from_disk(&mut self) {
        let modified = config::modified(&self.path);
        if modified == self.loaded_modified {
            return;
        }
        self.loaded_modified = modified;
        match self.log.entries() {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Could not read {}: {e}", self.path.display())),
        }
        self.refilter();
    }

    pub fn ui(&mut self, ui: &mut Ui, settings: &Settings) {
        self.toolbar(ui);
        self.frequent(ui);
        ui.separator();
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        if self.entries.is_empty() {
            ui.add_space(12.0);
            ui.weak("No notifications yet. Anomalies the monitor reports will show up here.");
            return;
        }
        if let Some(name) = self.table(ui, &settings.ignored())
            && let Err(e) = config::add_ignored(settings.config_path(), &name)
        {
            self.error = Some(format!("Could not save {name} to the settings: {e}"));
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let mut changed = false;
            egui::ComboBox::from_id_salt("kind")
                .selected_text(self.kind.map_or("All kinds", FindingKind::label))
                .show_ui(ui, |ui| {
                    changed |= ui
                        .selectable_value(&mut self.kind, None, "All kinds")
                        .changed();
                    for kind in FindingKind::ALL {
                        changed |= ui
                            .selectable_value(&mut self.kind, Some(kind), kind.label())
                            .changed();
                    }
                });
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.search)
                        .hint_text("Filter by process, details or command line")
                        .desired_width(300.0),
                )
                .changed();
            if changed {
                self.refilter();
            }
            ui.weak(format!("{} of {}", self.visible.len(), self.entries.len()));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open log file").clicked()
                    && let Err(e) = shell::open(&self.path)
                {
                    self.error = Some(e.to_string());
                }
            });
        });
    }

    /// The processes that show up most in the current view: the first
    /// candidates for a higher threshold or the ignore list.
    fn frequent(&mut self, ui: &mut Ui) {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for &i in &self.visible {
            *counts.entry(self.entries[i].name.as_str()).or_default() += 1;
        }
        let mut top: Vec<(&str, usize)> = counts.into_iter().filter(|&(_, n)| n > 1).collect();
        top.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        top.truncate(6);
        if top.is_empty() {
            return;
        }
        let mut picked = None;
        ui.horizontal_wrapped(|ui| {
            ui.weak("Most frequent:");
            for (name, count) in top {
                if ui
                    .small_button(format!("{name} ×{count}"))
                    .on_hover_text("Show only this process")
                    .clicked()
                {
                    picked = Some(name.to_owned());
                }
            }
        });
        if let Some(name) = picked {
            self.search = name;
            self.refilter();
        }
    }

    /// Draws the table; returns the exe name the user chose to always ignore.
    fn table(&self, ui: &mut Ui, ignored: &HashSet<String>) -> Option<String> {
        let mut ignore = None;
        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
            .column(Column::auto().at_least(130.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::initial(160.0).at_least(80.0).clip(true))
            .column(Column::auto().at_least(50.0))
            .column(Column::remainder().at_least(200.0).clip(true))
            // Fixed, so the details column can't push the button off the edge.
            .column(Column::exact(110.0))
            .header(ROW_HEIGHT, |mut header| {
                for title in ["Time", "Kind", "Process", "PID", "Details", ""] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.visible.len(), |mut row| {
                    let entry = &self.entries[self.visible[row.index()]];
                    row.col(|ui| {
                        ui.label(os::local_time(entry.unix_time));
                    });
                    row.col(|ui| {
                        ui.label(entry.kind.label());
                    });
                    row.col(|ui| {
                        ui.add(Label::new(&entry.name).truncate())
                            .on_hover_ui(|ui| process_details(ui, entry))
                            .context_menu(|ui| process_menu(ui, entry));
                    });
                    row.col(|ui| {
                        ui.label(entry.pid.to_string());
                    });
                    row.col(|ui| {
                        ui.add(Label::new(&entry.summary).truncate());
                    });
                    row.col(|ui| {
                        if ignored.contains(&entry.name.to_lowercase()) {
                            ui.weak("ignored");
                        } else if ui
                            .small_button("Always ignore")
                            .on_hover_text(format!("Never report {} again", entry.name))
                            .clicked()
                        {
                            ignore = Some(entry.name.clone());
                        }
                    });
                });
            });
        ignore
    }

    fn refilter(&mut self) {
        let needle = self.search.trim().to_lowercase();
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| self.kind.is_none_or(|kind| e.kind == kind))
            .filter(|(_, e)| needle.is_empty() || mentions(e, &needle))
            .map(|(i, _)| i)
            .collect();
    }
}

/// Whether any text of `entry` contains the lower-cased `needle`.
fn mentions(entry: &Entry, needle: &str) -> bool {
    [
        Some(entry.name.as_str()),
        Some(entry.summary.as_str()),
        entry.command_line.as_deref(),
        entry.path.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(|text| text.to_lowercase().contains(needle))
}

fn process_details(ui: &mut Ui, entry: &Entry) {
    ui.label(RichText::new(&entry.name).strong());
    ui.label(entry.path.as_deref().unwrap_or("Path unavailable"));
    ui.label(
        RichText::new(
            entry
                .command_line
                .as_deref()
                .unwrap_or("Command line unavailable"),
        )
        .monospace(),
    );
    ui.weak("Right-click for more");
}

fn process_menu(ui: &mut Ui, entry: &Entry) {
    if let Some(command_line) = &entry.command_line
        && ui.button("Copy command line").clicked()
    {
        ui.ctx().copy_text(command_line.clone());
        ui.close();
    }
    if let Some(path) = &entry.path
        && ui.button("Show in Explorer").clicked()
    {
        // Nothing useful to do if Explorer can't start.
        let _ = shell::reveal(path);
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, kind: FindingKind) -> Entry {
        Entry {
            unix_time: 0,
            kind,
            pid: 1,
            name: name.into(),
            summary: "100% of a core".into(),
            path: None,
            command_line: Some(format!("{name} --watch src")),
        }
    }

    #[test]
    fn filters_by_kind_and_text() {
        let mut view =
            HistoryView::new(std::env::temp_dir().join("spike-detector-ui-test-missing.jsonl"));
        view.entries = vec![
            entry("node.exe", FindingKind::CpuHog),
            entry("MSBuild.exe", FindingKind::Orphan),
            entry("node.exe", FindingKind::Orphan),
        ];
        view.refilter();
        assert_eq!(view.visible, [0, 1, 2]);

        view.kind = Some(FindingKind::Orphan);
        view.refilter();
        assert_eq!(view.visible, [1, 2]);

        view.search = "NODE".into();
        view.refilter();
        assert_eq!(view.visible, [2]);

        view.kind = None;
        view.search = "--watch".into();
        view.refilter();
        assert_eq!(view.visible, [0, 1, 2]);
    }
}
