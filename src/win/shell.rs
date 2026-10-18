//! Opening files and folders, delegated to Explorer, plus message boxes.
//!
//! Opening a file in-process (ShellExecute) loads much of the Windows shell
//! into the caller, and those DLLs never unload. Handing the job to a
//! short-lived explorer.exe keeps them out of this long-running process.

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::{io, ptr};

use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};

use super::wide;

/// Opens `path` the way a double-click would: in its associated app, or with
/// Windows asking which app to use if there is none.
pub fn open(path: &Path) -> io::Result<()> {
    Command::new("explorer.exe").arg(path).spawn().map(drop)
}

/// Opens an Explorer window with `path` selected.
pub fn reveal(path: &str) -> io::Result<()> {
    // Explorer parses its own command line and wants the path quoted after the comma.
    Command::new("explorer.exe")
        .raw_arg(format!("/select,\"{path}\""))
        .spawn()
        .map(drop)
}

pub fn error_box(title: &str, text: &str) {
    let (title, text) = (wide(title), wide(text));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            text.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        )
    };
}
