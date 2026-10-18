//! The settings and history window. The tray monitor opens it from its menu;
//! it lives only while the window is open, so the monitor itself never
//! carries a GUI toolkit.

// Release builds run without a console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config_file;
mod history;
mod os;
mod settings;

use std::time::Duration;

use eframe::egui;
use spike_detector::config::Paths;
use spike_detector::win::{shell, system};

use history::HistoryView;
use settings::Settings;

/// Window title; also how a second launch finds the open window.
const TITLE: &str = "spike-detector: history and settings";

/// How often the open window looks for changes made by the monitor (new
/// history) or the tray menu (the ignore list).
const REFRESH: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    History,
    Settings,
}

fn main() -> eframe::Result {
    let tab = if std::env::args().any(|arg| arg == "--settings") {
        Tab::Settings
    } else {
        Tab::History
    };
    // One window is enough: a second launch brings the first one forward.
    let Some(_instance) = system::SingleInstance::acquire(r"Local\spike-detector-ui") else {
        os::bring_to_front(TITLE);
        return Ok(());
    };
    // Someone is working with this window: full-speed cores, whatever the
    // monitor that started it runs on.
    system::set_eco_qos(false);
    let paths = match Paths::resolve() {
        Ok(paths) => paths,
        Err(e) => {
            shell::error_box(TITLE, &format!("Could not find the data folder: {e}"));
            return Ok(());
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([640.0, 360.0])
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../../assets/icon.png"))
                    .expect("the bundled icon is a valid PNG"),
            ),
        ..Default::default()
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(move |_| Ok(Box::new(Window::new(paths, tab)))),
    )
}

struct Window {
    tab: Tab,
    settings: Settings,
    history: HistoryView,
}

impl Window {
    fn new(paths: Paths, tab: Tab) -> Self {
        Self {
            tab,
            settings: Settings::load(paths.config),
            history: HistoryView::new(paths.history),
        }
    }
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.settings.refresh_from_disk();
        self.history.refresh_from_disk();
        ui.ctx().request_repaint_after(REFRESH);

        egui::Panel::top("tabs").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.tab, Tab::History, "History");
                let settings = if self.settings.is_dirty() {
                    "Settings •"
                } else {
                    "Settings"
                };
                ui.selectable_value(&mut self.tab, Tab::Settings, settings);
            });
        });
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::History => self.history.ui(ui, &self.settings),
            Tab::Settings => self.settings.ui(ui),
        });
    }
}
