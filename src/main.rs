//! spike-detector: a tray app that watches for runaway background processes
//! (a node.exe spinning a core, an MSBuild node nobody needs any more) and
//! tells you about them. What to do is up to you.

// Release builds run without a console window; debug builds keep one for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod alerts;
mod app;
mod config;
mod detect;
mod history;
mod sampler;
mod tracker;
mod win;

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Shell::{NIN_BALLOONUSERCLICK, NIN_SELECT, NINF_KEY};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, PostQuitMessage, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_TIMER,
};

use app::{App, Flow};
use config::{Config, Paths};
use win::tray::WM_TRAY;

const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;

thread_local! {
    /// The app lives on the UI thread and is reached from the window procedure.
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn main() {
    let Some(_instance) = win::system::SingleInstance::acquire("Local\\spike-detector") else {
        return;
    };
    win::system::enable_per_monitor_dpi();
    win::system::enable_eco_qos();

    if let Err(message) = run() {
        win::shell::error_box("spike-detector", &message);
    }
    APP.with(|cell| drop(cell.borrow_mut().take()));
}

fn run() -> Result<(), String> {
    let paths = Paths::resolve().map_err(|e| format!("Could not create the data folder: {e}"))?;
    let config = Config::load_or_create(&paths.config)
        .map_err(|e| format!("{}:\n{e}", paths.config.display()))?;
    let hwnd = win::window::create_hidden("SpikeDetector", Some(wndproc))
        .map_err(|e| format!("Could not create the window: {e}"))?;
    APP.with(|cell| *cell.borrow_mut() = Some(App::new(hwnd, paths, config)));
    // Take the first snapshot now rather than one interval later.
    with_app(App::tick);
    win::window::run_message_loop();
    Ok(())
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            with_app(App::tick);
        }
        WM_TRAY => on_tray_event(hwnd, lparam),
        WM_CLOSE => quit(hwnd),
        // SAFETY: plain FFI call.
        WM_DESTROY => unsafe { PostQuitMessage(0) },
        _ if Some(msg) == win::window::taskbar_created_message() => {
            with_app(App::on_taskbar_created);
        }
        // SAFETY: forwarding the arguments we were given.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    0
}

fn on_tray_event(hwnd: HWND, lparam: LPARAM) {
    // With NOTIFYICON_VERSION_4 the event sits in the low word of lParam.
    let event = (lparam & 0xFFFF) as u32;
    if matches!(
        event,
        WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT | NIN_BALLOONUSERCLICK
    ) {
        show_menu(hwnd);
    }
}

fn show_menu(hwnd: HWND) {
    let Some(entries) = with_app(|app| app.menu()) else {
        return;
    };
    // The app must not stay borrowed while the menu is open: its modal loop
    // keeps dispatching timer ticks.
    let Some(command) = win::menu::show(hwnd, entries) else {
        return;
    };
    if let Some(Flow::Quit) = with_app(|app| app.execute(command)) {
        quit(hwnd);
    }
}

fn quit(hwnd: HWND) {
    // Drop the app first so the tray icon goes away while the window still exists.
    APP.with(|cell| {
        if let Ok(mut app) = cell.try_borrow_mut() {
            drop(app.take());
        }
    });
    // SAFETY: plain FFI call on our own window.
    unsafe { DestroyWindow(hwnd) };
}

/// Runs `f` on the app. Returns `None` if there is no app, or if it is
/// already borrowed further up the stack: modal loops (menus, message boxes)
/// dispatch messages re-entrantly, and skipping one tick is harmless.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        let mut app = cell.try_borrow_mut().ok()?;
        app.as_mut().map(f)
    })
}
