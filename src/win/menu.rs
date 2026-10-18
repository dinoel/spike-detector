//! Popup menus built from plain data. The picked entry's command comes back
//! directly, so callers never deal with numeric menu IDs.

use std::{mem, ptr};

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MENU_ITEM_FLAGS, MF_CHECKED,
    MF_GRAYED, MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, WM_NULL,
};

use super::wide;

pub enum Entry<C> {
    Action {
        label: String,
        command: C,
        checked: bool,
    },
    /// A greyed-out line of information.
    Note(String),
    Submenu {
        label: String,
        entries: Vec<Entry<C>>,
    },
    Separator,
}

impl<C> Entry<C> {
    pub fn action(label: impl Into<String>, command: C) -> Self {
        Self::Action {
            label: label.into(),
            command,
            checked: false,
        }
    }

    pub fn note(label: impl Into<String>) -> Self {
        Self::Note(label.into())
    }
}

/// Shows `entries` as a popup menu at the cursor and returns the command the
/// user picked, if any. Runs a modal loop that keeps dispatching messages.
pub fn show<C>(hwnd: HWND, entries: Vec<Entry<C>>) -> Option<C> {
    let menu = Menu::new()?;
    let mut commands = Vec::new();
    menu.append_all(entries, &mut commands);
    let mut cursor = POINT::default();
    // SAFETY: plain FFI calls on a window and a menu we own.
    let picked = unsafe {
        GetCursorPos(&mut cursor);
        // Without this the menu doesn't close when the user clicks elsewhere.
        SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(
            menu.0,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            cursor.x,
            cursor.y,
            0,
            hwnd,
            ptr::null(),
        );
        PostMessageW(hwnd, WM_NULL, 0, 0);
        picked
    };
    // IDs are 1-based positions in `commands`; 0 means nothing was picked.
    let index = usize::try_from(picked).ok()?.checked_sub(1)?;
    commands.into_iter().nth(index)
}

struct Menu(HMENU);

impl Menu {
    fn new() -> Option<Self> {
        // SAFETY: plain FFI call; failure is a null handle.
        let handle = unsafe { CreatePopupMenu() };
        (!handle.is_null()).then_some(Self(handle))
    }

    fn append_all<C>(&self, entries: Vec<Entry<C>>, commands: &mut Vec<C>) {
        for entry in entries {
            match entry {
                Entry::Action {
                    label,
                    command,
                    checked,
                } => {
                    commands.push(command);
                    let flags = if checked {
                        MF_STRING | MF_CHECKED
                    } else {
                        MF_STRING
                    };
                    self.append(flags, commands.len(), &label);
                }
                Entry::Note(label) => {
                    self.append(MF_STRING | MF_GRAYED, 0, &label);
                }
                Entry::Submenu { label, entries } => {
                    let Some(submenu) = Menu::new() else { continue };
                    submenu.append_all(entries, commands);
                    // Once attached, the parent owns the submenu and destroys it.
                    if self.append(MF_POPUP, submenu.0 as usize, &label) {
                        mem::forget(submenu);
                    }
                }
                Entry::Separator => {
                    // SAFETY: separators take no text.
                    unsafe { AppendMenuW(self.0, MF_SEPARATOR, 0, ptr::null()) };
                }
            }
        }
    }

    fn append(&self, flags: MENU_ITEM_FLAGS, id: usize, label: &str) -> bool {
        // Menu text treats `&` as a mnemonic marker and `\t` as a column break.
        let text = wide(label.replace('&', "&&").replace(['\t', '\r', '\n'], " "));
        // SAFETY: `text` is NUL-terminated and outlives the call.
        unsafe { AppendMenuW(self.0, flags, id, text.as_ptr()) != 0 }
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        // SAFETY: we own the menu; this also destroys attached submenus.
        unsafe { DestroyMenu(self.0) };
    }
}
