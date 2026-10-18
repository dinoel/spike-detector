//! The notification-area icon: shows status, pops balloon notifications
//! (rendered as toasts on Windows 10/11) and reports clicks to our window.

use std::ptr;

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_LARGE_ICON, NIIF_USER,
    NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION, NOTIFY_ICON_DATA_FLAGS,
    NOTIFY_ICON_MESSAGE, NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, HICON, IDI_APPLICATION, IDI_WARNING, IMAGE_ICON, LR_SHARED, LoadIconW,
    LoadImageW, SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, WM_APP,
};
use windows_sys::core::PCWSTR;

use super::copy_to_fixed;

/// Message the shell posts to our window for icon events.
pub const WM_TRAY: u32 = WM_APP + 1;
/// Identifies our icon to the shell; we only ever have one.
const TRAY_UID: u32 = 1;

/// Icon resource IDs, as numbered in assets/app.rc.
const ICON_IDLE: u16 = 1;
const ICON_ALERT: u16 = 2;
const ICON_PAUSED: u16 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Alerting(usize),
    Paused,
}

/// What a notification is about; it picks the picture shown in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notice {
    /// An anomaly was found.
    Anomaly,
    /// Anything else the app has to say: errors, an empty history.
    Info,
}

impl Status {
    /// Our own icon for the state, or a stock one if the executable was
    /// built without resources.
    fn icon(self) -> HICON {
        let (resource, stock) = match self {
            Self::Idle => (ICON_IDLE, IDI_APPLICATION),
            Self::Alerting(_) => (ICON_ALERT, IDI_WARNING),
            Self::Paused => (ICON_PAUSED, IDI_APPLICATION),
        };
        own_icon(resource, IconSize::Small).unwrap_or_else(|| {
            // SAFETY: loads a shared stock icon, which must not be destroyed.
            unsafe { LoadIconW(ptr::null_mut(), stock) }
        })
    }

    fn tooltip(self) -> String {
        match self {
            Self::Idle => "spike-detector: all clear".to_owned(),
            Self::Alerting(1) => "spike-detector: 1 anomaly".to_owned(),
            Self::Alerting(n) => format!("spike-detector: {n} anomalies"),
            Self::Paused => "spike-detector: paused".to_owned(),
        }
    }
}

pub struct Tray {
    hwnd: HWND,
    status: Status,
}

impl Tray {
    /// Adds the icon. Failure isn't fatal: right after logon Explorer may not
    /// be up yet, and [`Tray::restore`] runs again once it is.
    pub fn new(hwnd: HWND) -> Self {
        let tray = Self {
            hwnd,
            status: Status::Idle,
        };
        tray.restore();
        tray
    }

    /// (Re)registers the icon, e.g. after Explorer restarted.
    pub fn restore(&self) {
        let mut data = self.data(NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP);
        if send(NIM_ADD, &data) {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            send(NIM_SETVERSION, &data);
        }
    }

    pub fn set_status(&mut self, status: Status) {
        if status != self.status {
            self.status = status;
            send(NIM_MODIFY, &self.data(NIF_ICON | NIF_TIP | NIF_SHOWTIP));
        }
    }

    /// Shows a notification (a toast on Windows 10/11) with our own large
    /// icon as its picture; stock warning/info icons are only a fallback.
    pub fn show_balloon(&self, notice: Notice, title: &str, text: &str) {
        let mut data = self.data(NIF_INFO);
        copy_to_fixed(&mut data.szInfoTitle, title);
        copy_to_fixed(&mut data.szInfo, text);
        let (resource, stock) = match notice {
            Notice::Anomaly => (ICON_ALERT, NIIF_WARNING),
            Notice::Info => (ICON_IDLE, NIIF_INFO),
        };
        data.dwInfoFlags = match own_icon(resource, IconSize::Large) {
            Some(icon) => {
                data.hBalloonIcon = icon;
                NIIF_USER | NIIF_LARGE_ICON
            }
            None => stock,
        };
        send(NIM_MODIFY, &data);
    }

    fn data(&self, flags: NOTIFY_ICON_DATA_FLAGS) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: TRAY_UID,
            uFlags: flags,
            uCallbackMessage: WM_TRAY,
            ..Default::default()
        };
        if flags & NIF_ICON != 0 {
            data.hIcon = self.status.icon();
        }
        if flags & NIF_TIP != 0 {
            copy_to_fixed(&mut data.szTip, &self.status.tooltip());
        }
        data
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        send(NIM_DELETE, &self.data(0));
    }
}

#[derive(Debug, Clone, Copy)]
enum IconSize {
    /// Tray size.
    Small,
    /// Notification picture size.
    Large,
}

/// One of our icon resources at the system's size for `size`. `None` if the
/// executable was built without resources (test binaries are).
fn own_icon(resource: u16, size: IconSize) -> Option<HICON> {
    let (cx, cy) = match size {
        IconSize::Small => (SM_CXSMICON, SM_CYSMICON),
        IconSize::Large => (SM_CXICON, SM_CYICON),
    };
    // SAFETY: plain FFI calls. The resource ID travels as a fake string
    // pointer (MAKEINTRESOURCE). Shared icons are cached by the system and
    // must not be destroyed.
    let icon = unsafe {
        LoadImageW(
            GetModuleHandleW(ptr::null()),
            resource as usize as PCWSTR,
            IMAGE_ICON,
            GetSystemMetrics(cx),
            GetSystemMetrics(cy),
            LR_SHARED,
        )
    };
    (!icon.is_null()).then_some(icon)
}

/// Sends one request to the shell. Failures are logged and otherwise
/// ignored: a missing icon must never take the monitor down.
fn send(message: NOTIFY_ICON_MESSAGE, data: &NOTIFYICONDATAW) -> bool {
    // SAFETY: `data` is fully initialized and `cbSize` is set.
    let ok = unsafe { Shell_NotifyIconW(message, data) } != 0;
    if !ok {
        eprintln!("Shell_NotifyIconW({message}) failed");
    }
    ok
}
