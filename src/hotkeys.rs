//! Native global shortcut registration and local menu-message filtering.
//!
//! This module never installs keyboard hooks or changes system sound settings.

use crate::settings::Hotkey;
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_HOTKEY_ALREADY_REGISTERED, ERROR_INVALID_PARAMETER,
        ERROR_INVALID_WINDOW_HANDLE, GetLastError, HWND,
    },
    UI::{
        Input::KeyboardAndMouse::{MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey},
        WindowsAndMessaging::{SC_KEYMENU, WM_SYSCHAR, WM_SYSCOMMAND},
    },
};

pub const ID_BASE: i32 = 100;

fn native_id(index: usize) -> Option<i32> {
    i32::try_from(index)
        .ok()
        .and_then(|index| ID_BASE.checked_add(index))
        .filter(|id| *id <= 0xbfff)
}

/// Registers a shortcut on the calling thread's live window. A null window
/// registers a thread shortcut instead. The caller owns its later removal.
pub unsafe fn register(window: HWND, index: usize, hotkey: Hotkey) -> Result<(), u32> {
    if !hotkey.is_bound() {
        return Ok(());
    }
    let id = native_id(index).ok_or(ERROR_INVALID_PARAMETER)?;
    if unsafe { RegisterHotKey(window, id, hotkey.modifiers | MOD_NOREPEAT, hotkey.key) } != 0 {
        Ok(())
    } else {
        Err(unsafe { GetLastError() })
    }
}

pub unsafe fn unregister(window: HWND, index: usize) {
    if let Some(id) = native_id(index) {
        unsafe {
            UnregisterHotKey(window, id);
        }
    }
}

pub fn registration_error(code: u32) -> String {
    match code {
        ERROR_HOTKEY_ALREADY_REGISTERED => {
            "This shortcut is already registered by Windows or another application.".into()
        }
        ERROR_ACCESS_DENIED => {
            "Windows denied permission to register this shortcut (error 5).".into()
        }
        ERROR_INVALID_PARAMETER => {
            "Windows rejected this shortcut's registration parameters (error 87).".into()
        }
        ERROR_INVALID_WINDOW_HANDLE => {
            "The shortcut's application window is unavailable (error 1400).".into()
        }
        _ => format!("Windows could not register this shortcut (error {code})."),
    }
}

/// Prevents the app's menu-less native windows from requesting unused menu
/// activation. Call only for Study Pen windows, before default processing.
pub fn blocks_menu_beep(message: u32, wparam: usize) -> bool {
    message == WM_SYSCHAR || (message == WM_SYSCOMMAND && (wparam & 0xfff0) == SC_KEYMENU as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{Action, Settings};
    use std::{
        ptr,
        sync::atomic::{AtomicUsize, Ordering},
    };
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_SHIFT, VK_F4, VK_F24},
            WindowsAndMessaging::*,
        },
    };

    struct HiddenWindow {
        window: HWND,
        class_name: Vec<u16>,
    }

    impl HiddenWindow {
        fn new() -> Self {
            static NEXT_CLASS: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_CLASS.fetch_add(1, Ordering::Relaxed);
            let class_name: Vec<u16> = format!("StudyPen.Hotkeys.Test.{id}")
                .encode_utf16()
                .chain(Some(0))
                .collect();
            unsafe {
                let instance = GetModuleHandleW(ptr::null());
                let class = WNDCLASSW {
                    lpfnWndProc: Some(DefWindowProcW),
                    hInstance: instance,
                    lpszClassName: class_name.as_ptr(),
                    ..Default::default()
                };
                assert_ne!(
                    RegisterClassW(&class),
                    0,
                    "Could not register the hidden shortcut test window"
                );
                let window = CreateWindowExW(
                    WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    1,
                    1,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    instance,
                    ptr::null(),
                );
                if window.is_null() {
                    UnregisterClassW(class_name.as_ptr(), instance);
                    panic!("Could not create the hidden shortcut test window");
                }
                Self { window, class_name }
            }
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.window);
                UnregisterClassW(self.class_name.as_ptr(), GetModuleHandleW(ptr::null()));
            }
        }
    }

    struct Registration {
        window: HWND,
        index: usize,
    }

    impl Registration {
        fn bind(window: HWND, index: usize, hotkey: Hotkey) -> Result<Self, u32> {
            unsafe {
                register(window, index, hotkey)?;
            }
            Ok(Self { window, index })
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            unsafe {
                unregister(self.window, self.index);
            }
        }
    }

    #[test]
    fn unbound_shortcuts_do_not_request_native_registration() {
        // Even an invalid owner/id is harmless when there is no binding.
        assert!(unsafe { register(ptr::null_mut(), usize::MAX, Hotkey::unbound()) }.is_ok());
    }

    #[test]
    fn local_menu_filter_preserves_close_and_system_keystrokes() {
        assert!(blocks_menu_beep(WM_SYSCHAR, b'a' as usize));
        assert!(blocks_menu_beep(WM_SYSCOMMAND, SC_KEYMENU as usize));
        assert!(blocks_menu_beep(
            WM_SYSCOMMAND,
            SC_KEYMENU as usize | 0x000f
        ));
        assert!(!blocks_menu_beep(WM_SYSCOMMAND, SC_CLOSE as usize));
        assert!(!blocks_menu_beep(WM_SYSCOMMAND, SC_MOVE as usize));
        assert!(!blocks_menu_beep(WM_SYSKEYDOWN, VK_F4 as usize));
        assert!(!blocks_menu_beep(WM_SYSKEYUP, 0x12));
        assert!(!blocks_menu_beep(WM_CHAR, b'a' as usize));
    }

    #[test]
    fn registration_errors_distinguish_conflicts_from_native_failures() {
        assert!(
            registration_error(ERROR_HOTKEY_ALREADY_REGISTERED).contains("another application")
        );
        assert!(registration_error(ERROR_ACCESS_DENIED).contains("permission"));
        assert!(registration_error(12345).contains("12345"));
        assert_eq!(native_id(0), Some(ID_BASE));
        assert_eq!(native_id(usize::MAX), None);
    }

    #[test]
    fn native_collision_preserves_owner_and_release_allows_registration() {
        let first_window = HiddenWindow::new();
        let second_window = HiddenWindow::new();
        let hotkey = Hotkey::new(MOD_CONTROL | MOD_SHIFT | MOD_ALT, VK_F24 as u32);
        let first = match Registration::bind(first_window.window, 0, hotkey) {
            Ok(registration) => registration,
            Err(ERROR_HOTKEY_ALREADY_REGISTERED) => {
                eprintln!(
                    "Skipping native shortcut collision test: the desktop already owns the test chord: {}",
                    registration_error(ERROR_HOTKEY_ALREADY_REGISTERED)
                );
                return;
            }
            Err(code) => panic!(
                "Could not register the native test shortcut: {}",
                registration_error(code)
            ),
        };
        let failed = Registration::bind(second_window.window, 1, hotkey);
        assert!(
            matches!(failed, Err(ERROR_HOTKEY_ALREADY_REGISTERED)),
            "A second owner must not replace an existing global shortcut"
        );
        let still_owned = Registration::bind(second_window.window, 2, hotkey);
        assert!(matches!(still_owned, Err(ERROR_HOTKEY_ALREADY_REGISTERED)));
        drop(first);
        let second = Registration::bind(second_window.window, 1, hotkey)
            .expect("The shortcut should become available after its owner releases it");
        assert_eq!(unsafe { IsWindowVisible(first_window.window) }, 0);
        assert_eq!(unsafe { IsWindowVisible(second_window.window) }, 0);
        drop(second);
    }

    #[test]
    #[ignore = "Checks this desktop's external global shortcut availability; run explicitly"]
    fn probe_default_registration_on_this_desktop() {
        let window = HiddenWindow::new();
        let settings = Settings::default();
        let mut registrations = Vec::new();
        let mut failures = Vec::new();
        for action in Action::ALL {
            let index = action.index();
            let hotkey = settings.hotkeys[index];
            if !hotkey.is_bound() {
                println!("{}: unbound (no registration)", action.label());
                continue;
            }
            match Registration::bind(window.window, index, hotkey) {
                Ok(registration) => {
                    println!("{} [{}]: registered", action.label(), hotkey.display());
                    registrations.push(registration);
                }
                Err(code) => {
                    let error = format!(
                        "{} [{}]: {}",
                        action.label(),
                        hotkey.display(),
                        registration_error(code)
                    );
                    println!("{error}");
                    failures.push(error);
                }
            }
        }
        assert!(
            failures.is_empty(),
            "Default shortcut registration failed: {}",
            failures.join("; ")
        );
    }
}
