//! The hidden window that owns the tray icon and receives timer ticks, and
//! the message loop that drives it.

use std::sync::OnceLock;
use std::time::Duration;
use std::{io, ptr};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DispatchMessageW, GetMessageW, KillTimer, MSG, RegisterClassExW,
    RegisterWindowMessageW, SetCoalescableTimer, TranslateMessage, WNDCLASSEXW, WNDPROC,
    WS_OVERLAPPED,
};

use super::wide;

/// Creates a top-level window that is never shown. Deliberately not a
/// message-only window: those miss the `TaskbarCreated` broadcast.
pub fn create_hidden(class_name: &str, wndproc: WNDPROC) -> io::Result<HWND> {
    let class_name = wide(class_name);
    // SAFETY: plain FFI calls; every pointer refers to live data.
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: wndproc,
            hInstance: instance,
            lpszClassName: class_name.as_ptr(),
            ..Default::default()
        };
        if RegisterClassExW(&class) == 0 {
            return Err(io::Error::last_os_error());
        }
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err(io::Error::last_os_error());
        }
        Ok(hwnd)
    }
}

/// Pumps messages until `WM_QUIT`.
pub fn run_message_loop() {
    let mut msg = MSG::default();
    // SAFETY: standard message loop; GetMessageW returns 0 on WM_QUIT and -1 on error.
    unsafe {
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// The message Explorer broadcasts when the taskbar is (re)created.
pub fn taskbar_created_message() -> Option<u32> {
    static ID: OnceLock<u32> = OnceLock::new();
    let id = *ID.get_or_init(|| {
        let name = wide("TaskbarCreated");
        // SAFETY: `name` is NUL-terminated and outlives the call.
        unsafe { RegisterWindowMessageW(name.as_ptr()) }
    });
    (id != 0).then_some(id)
}

/// Starts, or restarts with a new interval, a periodic timer. Windows may
/// delay a tick by up to a quarter of the interval to batch wake-ups with
/// other timers, which saves power and doesn't matter here.
pub fn set_timer(hwnd: HWND, id: usize, interval: Duration) {
    let interval_ms = u32::try_from(interval.as_millis()).unwrap_or(u32::MAX);
    // SAFETY: plain FFI call on our own window.
    unsafe { SetCoalescableTimer(hwnd, id, interval_ms, None, interval_ms / 4) };
}

pub fn kill_timer(hwnd: HWND, id: usize) {
    // SAFETY: plain FFI call on our own window.
    unsafe { KillTimer(hwnd, id) };
}
