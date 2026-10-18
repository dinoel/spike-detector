//! Small, safe wrappers over the Win32 APIs the app uses.

pub mod autostart;
pub mod menu;
pub mod process;
pub mod shell;
pub mod system;
pub mod tray;
pub mod window;

use std::ffi::OsStr;
use std::iter;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::UNICODE_STRING;

/// NUL-terminated UTF-16 copy of `s`, for `*W` APIs.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(iter::once(0)).collect()
}

/// Copies `s` into a fixed-size, NUL-terminated UTF-16 field of a Win32
/// struct, cutting it short with an ellipsis if it doesn't fit.
pub fn copy_to_fixed(dst: &mut [u16], s: &str) {
    const ELLIPSIS: u16 = 0x2026;
    // One unit is always reserved for the terminating NUL.
    let Some(capacity) = dst.len().checked_sub(1) else {
        return;
    };
    let units: Vec<u16> = s.encode_utf16().collect();
    let len = if units.len() <= capacity {
        dst[..units.len()].copy_from_slice(&units);
        units.len()
    } else if capacity == 0 {
        0
    } else {
        let mut keep = capacity - 1;
        // Never split a surrogate pair.
        if keep > 0 && (0xD800..0xDC00).contains(&units[keep - 1]) {
            keep -= 1;
        }
        dst[..keep].copy_from_slice(&units[..keep]);
        dst[keep] = ELLIPSIS;
        keep + 1
    };
    dst[len] = 0;
}

/// Byte view of a `u64` buffer; NT query buffers are kept as `u64`s so the
/// structs inside them are suitably aligned.
pub fn as_bytes(words: &[u64]) -> &[u8] {
    // SAFETY: any initialized `u64` is also valid as bytes, and the length
    // covers exactly the same memory.
    unsafe { std::slice::from_raw_parts(words.as_ptr().cast(), std::mem::size_of_val(words)) }
}

/// Text of a `UNICODE_STRING` that an NT query placed inside `buf`.
/// Returns `None` if it points anywhere else or is malformed.
pub fn utf16_in<'a>(buf: &'a [u8], s: &UNICODE_STRING) -> Option<&'a [u16]> {
    if s.Length == 0 {
        return Some(&[]);
    }
    let start = (s.Buffer as usize).checked_sub(buf.as_ptr() as usize)?;
    let bytes = buf.get(start..start.checked_add(usize::from(s.Length))?)?;
    // SAFETY: every bit pattern is a valid u16; misalignment is caught below.
    let (head, units, tail) = unsafe { bytes.align_to::<u16>() };
    (head.is_empty() && tail.is_empty()).then_some(units)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed<const N: usize>(s: &str) -> String {
        let mut buf = [0xFFFFu16; N];
        copy_to_fixed(&mut buf, s);
        let end = buf
            .iter()
            .position(|&u| u == 0)
            .expect("not NUL-terminated");
        String::from_utf16(&buf[..end]).unwrap()
    }

    #[test]
    fn copy_to_fixed_fits_or_ellipsizes() {
        assert_eq!(fixed::<8>("short"), "short");
        assert_eq!(fixed::<8>("exactly"), "exactly");
        assert_eq!(fixed::<8>("too long text"), "too lo…");
        assert_eq!(fixed::<1>("x"), "");
    }

    #[test]
    fn copy_to_fixed_keeps_surrogate_pairs_whole() {
        // "ab😀cd": the emoji is a surrogate pair at units 2..4.
        assert_eq!(fixed::<5>("ab😀cd"), "ab…");
    }

    #[test]
    fn utf16_in_rejects_pointers_outside_buffer() {
        let words = [0u64; 4];
        let buf = as_bytes(&words);
        let inside = UNICODE_STRING {
            Length: 4,
            MaximumLength: 4,
            Buffer: buf[8..].as_ptr().cast_mut().cast(),
        };
        assert_eq!(utf16_in(buf, &inside), Some(&[0u16, 0][..]));
        let outside = UNICODE_STRING {
            Buffer: buf[30..].as_ptr().cast_mut().cast(),
            ..inside
        };
        assert_eq!(utf16_in(buf, &outside), None);
        let odd = UNICODE_STRING {
            Buffer: buf[9..].as_ptr().cast_mut().cast(),
            ..inside
        };
        assert_eq!(utf16_in(buf, &odd), None);
    }
}
