//! Start at logon through the per-user Run key. No admin rights needed, and
//! the user can still switch it off under Settings > Apps > Startup or in
//! Task Manager; that choice is respected.

use std::path::Path;
use std::{env, io, ptr};

use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_ROUTINE_FLAGS, REG_SZ, RRF_RT_REG_BINARY, RRF_RT_REG_SZ,
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
};

use super::wide;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
/// Where Settings and Task Manager mark Run entries the user turned off.
/// They don't delete the entry, so the Run key alone would lie.
const APPROVED_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const VALUE: &str = "spike-detector";

/// Whether this executable will start at the next logon.
pub fn is_enabled() -> bool {
    let Ok(exe) = env::current_exe() else {
        return false;
    };
    let registered = read_string(RUN_KEY, VALUE).is_some_and(|command| starts(&command, &exe));
    registered && !read_binary(APPROVED_KEY, VALUE).is_some_and(|data| marked_disabled(&data))
}

pub fn set_enabled(enabled: bool) -> io::Result<()> {
    if !enabled {
        return delete_value(RUN_KEY, VALUE);
    }
    let exe = env::current_exe()?;
    write_string(RUN_KEY, VALUE, &format!("\"{}\"", exe.display()))?;
    // An old "disabled" mark from Task Manager would keep the entry dormant.
    delete_value(APPROVED_KEY, VALUE)
}

/// Whether a Run command line (`"C:\path\app.exe" args…`) starts `exe`.
fn starts(command: &str, exe: &Path) -> bool {
    let program = match command.strip_prefix('"') {
        Some(rest) => rest.split('"').next(),
        None => command.split(' ').next(),
    };
    program.is_some_and(|p| p.to_lowercase() == exe.to_string_lossy().to_lowercase())
}

/// StartupApproved data opens with a flags byte, odd when the entry is
/// disabled (02/06 on, 03/07 off); a timestamp follows.
fn marked_disabled(data: &[u8]) -> bool {
    data.first().is_some_and(|flags| flags & 1 == 1)
}

fn read_string(key: &str, value: &str) -> Option<String> {
    let bytes = read(key, value, RRF_RT_REG_SZ)?;
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs
        .iter()
        .map(|&pair| u16::from_le_bytes(pair))
        .take_while(|&unit| unit != 0)
        .collect();
    Some(String::from_utf16_lossy(&units))
}

fn read_binary(key: &str, value: &str) -> Option<Vec<u8>> {
    read(key, value, RRF_RT_REG_BINARY)
}

/// Raw data of an HKCU value; `None` if it doesn't exist or has another type.
fn read(key: &str, value: &str, kind: REG_ROUTINE_FLAGS) -> Option<Vec<u8>> {
    let (key, value) = (wide(key), wide(value));
    let query = |data: *mut u8, size: &mut u32| {
        // SAFETY: names are NUL-terminated; `data` is null (size query) or
        // points to at least `size` bytes.
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                value.as_ptr(),
                kind,
                ptr::null_mut(),
                data.cast(),
                size,
            )
        }
    };
    let mut size = 0;
    if query(ptr::null_mut(), &mut size) != ERROR_SUCCESS {
        return None;
    }
    let mut data = vec![0u8; size as usize];
    if query(data.as_mut_ptr(), &mut size) != ERROR_SUCCESS {
        return None;
    }
    data.truncate(size as usize);
    Some(data)
}

/// Writes an HKCU string value, creating the key if needed.
fn write_string(key: &str, value: &str, text: &str) -> io::Result<()> {
    let (key, value, text) = (wide(key), wide(value), wide(text));
    let size = u32::try_from(size_of_val(text.as_slice())).map_err(io::Error::other)?;
    // SAFETY: all buffers are NUL-terminated and outlive the call; `size`
    // covers the text including its NUL.
    check(unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            REG_SZ,
            text.as_ptr().cast(),
            size,
        )
    })
}

/// Deletes an HKCU value. Deleting what isn't there counts as success.
fn delete_value(key: &str, value: &str) -> io::Result<()> {
    let (key, value) = (wide(key), wide(value));
    // SAFETY: both names are NUL-terminated and outlive the call.
    match unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr()) } {
        ERROR_FILE_NOT_FOUND => Ok(()),
        status => check(status),
    }
}

fn check(status: WIN32_ERROR) -> io::Result<()> {
    if status == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(status as i32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Registry::RegDeleteKeyW;

    #[test]
    fn recognises_own_command_line() {
        let exe = Path::new(r"C:\Tools\Spike Detector\spike-detector.exe");
        assert!(starts(
            r#""C:\Tools\Spike Detector\spike-detector.exe""#,
            exe
        ));
        assert!(starts(
            r#""c:\tools\spike detector\SPIKE-DETECTOR.EXE" --quiet"#,
            exe
        ));
        assert!(!starts(r#""C:\Other\spike-detector.exe""#, exe));
        assert!(starts(
            r"C:\Tools\app.exe --flag",
            Path::new(r"C:\Tools\app.exe")
        ));
        assert!(!starts("", exe));
    }

    #[test]
    fn reads_startup_approved_flags() {
        assert!(!marked_disabled(&[2, 0, 0, 0]));
        assert!(!marked_disabled(&[6, 0, 0, 0]));
        assert!(marked_disabled(&[3, 0, 0, 0]));
        assert!(marked_disabled(&[7, 0, 0, 0]));
        assert!(!marked_disabled(&[]));
    }

    /// Exercises the registry helpers on a throwaway key, never the real Run key.
    #[test]
    fn registry_round_trip() {
        let key = format!(r"Software\spike-detector-test-{}", std::process::id());
        assert_eq!(read_string(&key, "missing"), None);
        write_string(&key, "path", r#""C:\Tools\app.exe""#).unwrap();
        assert_eq!(
            read_string(&key, "path").as_deref(),
            Some(r#""C:\Tools\app.exe""#)
        );
        assert_eq!(
            read_binary(&key, "path"),
            None,
            "type mismatch must not read"
        );
        delete_value(&key, "path").unwrap();
        delete_value(&key, "path").unwrap();
        assert_eq!(read_string(&key, "path"), None);

        let key = wide(&key);
        // SAFETY: NUL-terminated name of the key this test created.
        assert_eq!(
            unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) },
            ERROR_SUCCESS
        );
    }
}
