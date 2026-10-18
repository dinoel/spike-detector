//! Process-wide setup (single instance, DPI awareness, power throttling) and
//! session-wide facts (user idle time, session ID).

use std::os::windows::io::{FromRawHandle, OwnedHandle};
use std::ptr;

use windows_sys::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
use windows_sys::Win32::System::SystemInformation::GetTickCount;
use windows_sys::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, GetCurrentProcessId, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
    PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
    ProcessPowerThrottling, SetProcessInformation,
};
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

use super::wide;

/// A named mutex held for the lifetime of the process.
pub struct SingleInstance {
    _mutex: OwnedHandle,
}

impl SingleInstance {
    /// `None` if another instance already holds the mutex (or it can't be created).
    pub fn acquire(name: &str) -> Option<Self> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and outlives the call; GetLastError
        // is read right after, before anything can overwrite it.
        let (raw, already_exists) = unsafe {
            let raw = CreateMutexW(ptr::null(), 0, name.as_ptr());
            (raw, GetLastError() == ERROR_ALREADY_EXISTS)
        };
        if raw.is_null() {
            return None;
        }
        // SAFETY: `raw` is a fresh handle that nothing else owns.
        let mutex = unsafe { OwnedHandle::from_raw_handle(raw) };
        (!already_exists).then_some(Self { _mutex: mutex })
    }
}

/// Opts in or out of EcoQoS, where Windows may run the process on efficiency
/// cores at low clocks. Right for the monitor (tiny, latency-insensitive
/// work), wrong for a window someone is interacting with.
pub fn set_eco_qos(enabled: bool) {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: if enabled {
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED
        } else {
            0
        },
    };
    // SAFETY: `state` is a valid struct of the stated size. Best effort:
    // older Windows versions reject it, which is fine.
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            ptr::from_ref(&state).cast(),
            size_of_val(&state) as u32,
        );
    }
}

/// Crisp tray menus on high-DPI screens. Best effort.
pub fn enable_per_monitor_dpi() {
    // SAFETY: plain FFI call with a predefined constant.
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

/// Time since the last keyboard or mouse input in this session.
pub fn user_idle_ms() -> Option<u64> {
    let mut info = LASTINPUTINFO {
        cbSize: size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: `info` is a live out-struct with `cbSize` set.
    if unsafe { GetLastInputInfo(&mut info) } == 0 {
        return None;
    }
    // Both are 32-bit tick counts; wrapping subtraction survives the 49-day rollover.
    // SAFETY: plain FFI call.
    let now = unsafe { GetTickCount() };
    Some(u64::from(now.wrapping_sub(info.dwTime)))
}

/// The session this process runs in: the interactive user's.
pub fn current_session_id() -> Option<u32> {
    let mut session = 0;
    // SAFETY: `session` is a live out-parameter.
    let ok = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) };
    (ok != 0).then_some(session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_and_idle_time_are_available() {
        assert!(current_session_id().is_some());
        assert!(user_idle_ms().is_some());
    }
}
