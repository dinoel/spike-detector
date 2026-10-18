//! The few Windows calls the window needs beyond what eframe does.

use std::ptr;

use windows_sys::Win32::Foundation::{FILETIME, SYSTEMTIME};
use windows_sys::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow,
};

/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01 (Unix epoch).
const UNIX_EPOCH_IN_FILETIME_SECS: u64 = 11_644_473_600;

/// Brings the top-level window titled `title` to the front, restoring it if
/// minimized. Returns whether such a window exists.
pub fn bring_to_front(title: &str) -> bool {
    let title: Vec<u16> = title.encode_utf16().chain([0]).collect();
    // SAFETY: `title` is NUL-terminated and outlives the call; the window
    // handle is only passed back to the system.
    unsafe {
        let hwnd = FindWindowW(ptr::null(), title.as_ptr());
        if hwnd.is_null() {
            return false;
        }
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd);
    }
    true
}

/// `unix_secs` as local time, "2026-10-18 14:32:05".
pub fn local_time(unix_secs: u64) -> String {
    let ticks = unix_secs
        .saturating_add(UNIX_EPOCH_IN_FILETIME_SECS)
        .saturating_mul(10_000_000);
    let utc = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let (mut utc_parts, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    // SAFETY: all pointers refer to live locals; a null time zone means the current one.
    let ok = unsafe {
        FileTimeToSystemTime(&utc, &mut utc_parts) != 0
            && SystemTimeToTzSpecificLocalTime(ptr::null(), &utc_parts, &mut local) != 0
    };
    if !ok {
        return format!("@{unix_secs}");
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        local.wYear, local.wMonth, local.wDay, local.wHour, local.wMinute, local.wSecond
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_a_plausible_local_time() {
        // 2026-10-18 12:00:00 UTC; any time zone keeps it on the 18th or a neighbour.
        let text = local_time(1_792_324_800);
        assert_eq!(text.len(), "2026-10-18 12:00:00".len(), "{text}");
        assert!(text.starts_with("2026-10-1"), "{text}");
    }
}
