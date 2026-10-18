//! Queries and actions on a single process. Every call takes a
//! [`ProcessKey`] and verifies the creation time, so a recycled PID can never
//! redirect it to an unrelated process.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::{fmt, io, ptr};

use windows_sys::Wdk::System::Threading::{
    NtQueryInformationProcess, ProcessCommandLineInformation,
};
use windows_sys::Win32::Foundation::{
    ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, FILETIME, UNICODE_STRING,
};
use windows_sys::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, QueryFullProcessImageNameW,
    TerminateProcess,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

use super::{as_bytes, utf16_in};
use crate::sampler::ProcessKey;

/// What can be learned about a process beyond the snapshot. Fields are
/// `None` when access is denied: elevated processes, other users' processes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessDetails {
    pub path: Option<String>,
    pub command_line: Option<String>,
}

#[derive(Debug)]
pub enum ProcessError {
    /// The process has exited, or its PID now belongs to another process.
    Gone,
    AccessDenied,
    Os(io::Error),
}

impl From<io::Error> for ProcessError {
    fn from(e: io::Error) -> Self {
        match e.raw_os_error().and_then(|code| u32::try_from(code).ok()) {
            Some(ERROR_ACCESS_DENIED) => Self::AccessDenied,
            // OpenProcess reports a PID that no longer exists this way.
            Some(ERROR_INVALID_PARAMETER) => Self::Gone,
            _ => Self::Os(e),
        }
    }
}

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Gone => f.write_str("the process has already exited"),
            Self::AccessDenied => {
                f.write_str("access denied: the process runs elevated or as another user")
            }
            Self::Os(e) => write!(f, "{e}"),
        }
    }
}

pub fn details(key: ProcessKey) -> ProcessDetails {
    match open(key, PROCESS_QUERY_LIMITED_INFORMATION) {
        Ok(handle) => ProcessDetails {
            path: image_path(&handle),
            command_line: command_line(&handle),
        },
        Err(_) => ProcessDetails::default(),
    }
}

pub fn terminate(key: ProcessKey) -> Result<(), ProcessError> {
    let handle = open(key, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION)?;
    // SAFETY: `handle` is valid and was opened with PROCESS_TERMINATE.
    if unsafe { TerminateProcess(handle.as_raw_handle(), 1) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

/// PID of the process that owns the foreground window, if any.
pub fn foreground_pid() -> Option<u32> {
    // SAFETY: plain FFI calls; a null window is handled.
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            return None;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, &mut pid);
        (pid != 0).then_some(pid)
    }
}

/// Opens the process and checks it is still the instance `key` names.
fn open(key: ProcessKey, access: PROCESS_ACCESS_RIGHTS) -> Result<OwnedHandle, ProcessError> {
    // SAFETY: plain FFI call; failure is reported as a null handle.
    let raw = unsafe { OpenProcess(access, 0, key.pid) };
    if raw.is_null() {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: `raw` is a fresh handle that nothing else owns.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    if creation_time(&handle)? != key.create_time {
        return Err(ProcessError::Gone);
    }
    Ok(handle)
}

fn creation_time(handle: &OwnedHandle) -> io::Result<i64> {
    let mut created = FILETIME::default();
    let (mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: all out-pointers refer to live locals.
    let ok = unsafe {
        GetProcessTimes(
            handle.as_raw_handle(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((i64::from(created.dwHighDateTime) << 32) | i64::from(created.dwLowDateTime))
}

fn image_path(handle: &OwnedHandle) -> Option<String> {
    // Longest path Windows supports, so one call always suffices.
    let mut buf = vec![0u16; 32 * 1024];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` holds `len` units and outlives the call.
    let ok = unsafe {
        QueryFullProcessImageNameW(
            handle.as_raw_handle(),
            PROCESS_NAME_WIN32,
            buf.as_mut_ptr(),
            &mut len,
        )
    };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

fn command_line(handle: &OwnedHandle) -> Option<String> {
    let query = |buf: &mut [u64], needed: &mut u32| {
        let len = u32::try_from(size_of_val(buf)).unwrap_or(u32::MAX);
        // SAFETY: the pointer and length describe `buf`.
        unsafe {
            NtQueryInformationProcess(
                handle.as_raw_handle(),
                ProcessCommandLineInformation,
                buf.as_mut_ptr().cast(),
                len,
                needed,
            )
        }
    };
    // The first call only reports the size; its status is expected to fail.
    let mut needed = 0;
    query(&mut [], &mut needed);
    if needed == 0 {
        return None;
    }
    let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
    if query(&mut buf, &mut needed) < 0 {
        return None;
    }
    // The result is a UNICODE_STRING header pointing at text right after it.
    let bytes = as_bytes(&buf);
    let header = bytes.get(..size_of::<UNICODE_STRING>())?;
    // SAFETY: `header` spans a whole UNICODE_STRING of plain data.
    let text: UNICODE_STRING = unsafe { ptr::read_unaligned(header.as_ptr().cast()) };
    utf16_in(bytes, &text).map(String::from_utf16_lossy)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sampler::Sampler;
    use std::process::{Command, Stdio};

    fn key_of(pid: u32) -> ProcessKey {
        let snapshot = Sampler::new().take().unwrap();
        snapshot
            .processes()
            .find(|p| p.key.pid == pid)
            .map(|p| p.key)
            .expect("process not in snapshot")
    }

    #[test]
    fn details_of_own_process() {
        let details = details(key_of(std::process::id()));
        let path = details.path.expect("no path");
        assert!(path.to_lowercase().ends_with(".exe"), "{path}");
        let exe = std::env::current_exe().unwrap();
        assert!(
            details
                .command_line
                .expect("no command line")
                .contains(exe.file_stem().unwrap().to_str().unwrap())
        );
    }

    #[test]
    fn stale_key_is_refused_before_anything_happens() {
        let stale = ProcessKey {
            pid: std::process::id(),
            create_time: 1,
        };
        assert!(matches!(terminate(stale), Err(ProcessError::Gone)));
        assert_eq!(details(stale), ProcessDetails::default());
    }

    #[test]
    fn terminates_a_child_process() {
        let mut child = Command::new("ping.exe")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        terminate(key_of(child.id())).unwrap();
        assert_eq!(child.wait().unwrap().code(), Some(1));
    }
}
