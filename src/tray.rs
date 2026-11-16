//! Native notification-area integration with no background polling once added.

use std::{mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::{HWND, POINT},
    Graphics::Gdi::*,
    UI::{Shell::*, WindowsAndMessaging::*},
};

pub const CALLBACK_MESSAGE: u32 = WM_APP + 17;
pub const RESTORE_MESSAGE: u32 = WM_APP + 18;
pub const COMMAND_MESSAGE: u32 = WM_APP + 19;
pub const COMMAND_TOGGLE: usize = 1;
pub const COMMAND_SETTINGS: usize = 2;
pub const COMMAND_QUIT: usize = 3;
const ICON_ID: u32 = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1;

pub struct Tray {
    window: HWND,
    icon: HICON,
    restart_message: u32,
    added: bool,
    legacy_callback: bool,
}

impl Tray {
    /// The caller owns the live notification callback window on this thread.
    pub unsafe fn new(window: HWND) -> Result<Self, String> {
        let icon = match crate::identity::load_icon(32) {
            Some(icon) => icon,
            None => create_icon()?,
        };
        let restart_message = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
        SendMessageW(window, WM_SETICON, ICON_SMALL as usize, icon as isize);
        SendMessageW(window, WM_SETICON, ICON_BIG as usize, icon as isize);
        let mut tray = Self {
            window,
            icon,
            restart_message,
            added: false,
            legacy_callback: false,
        };
        tray.install(true);
        Ok(tray)
    }

    pub fn restart_message(&self) -> u32 {
        self.restart_message
    }
    pub fn is_added(&self) -> bool {
        self.added
    }

    pub fn uses_legacy_callback(&self) -> bool {
        self.legacy_callback
    }

    fn data(&self, visible: bool) -> NOTIFYICONDATAW {
        let mut data = NOTIFYICONDATAW {
            cbSize: size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.window,
            uID: ICON_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: CALLBACK_MESSAGE,
            hIcon: self.icon,
            ..Default::default()
        };
        // Window/id identity lets an unsigned portable executable move folders
        // without leaving a GUID tied to an old installation path.
        let tip = if visible {
            "Study Pen"
        } else {
            "Study Pen - toolbar hidden"
        };
        for (target, value) in data.szTip.iter_mut().zip(tip.encode_utf16()) {
            *target = value;
        }
        data
    }

    pub unsafe fn install(&mut self, visible: bool) {
        let mut data = self.data(visible);
        self.added =
            Shell_NotifyIconW(NIM_ADD, &data) != 0 || Shell_NotifyIconW(NIM_MODIFY, &data) != 0;
        if self.added {
            data.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            self.legacy_callback = Shell_NotifyIconW(NIM_SETVERSION, &data) == 0;
            if self.legacy_callback {
                // Legacy shell callbacks carry the same low-word event code.
                data.Anonymous.uVersion = NOTIFYICON_VERSION;
                Shell_NotifyIconW(NIM_SETVERSION, &data);
            }
        }
    }

    pub unsafe fn update_visibility(&mut self, visible: bool) {
        if self.added && Shell_NotifyIconW(NIM_MODIFY, &self.data(visible)) == 0 {
            self.added = false;
        }
    }

    pub unsafe fn remove(&mut self) {
        Shell_NotifyIconW(NIM_DELETE, &self.data(true));
        self.added = false;
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            self.remove();
            DestroyIcon(self.icon);
        }
    }
}

/// Called without an application-state borrow: menus run a nested message loop.
pub unsafe fn callback(window: HWND, event: u32, visible: bool, legacy: bool) {
    match event {
        NIN_SELECT | NIN_KEYSELECT => {
            PostMessageW(window, COMMAND_MESSAGE, COMMAND_TOGGLE, 0);
        }
        WM_LBUTTONUP if legacy => {
            PostMessageW(window, COMMAND_MESSAGE, COMMAND_TOGGLE, 0);
        }
        WM_CONTEXTMENU | WM_RBUTTONUP => {
            let command = show_menu(window, visible);
            if command != 0 {
                PostMessageW(window, COMMAND_MESSAGE, command, 0);
            }
        }
        _ => {}
    }
}

unsafe fn show_menu(window: HWND, visible: bool) -> usize {
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return 0;
    }
    AppendMenuW(
        menu,
        MF_STRING,
        COMMAND_TOGGLE,
        wide(if visible {
            "Hide toolbar"
        } else {
            "Show toolbar"
        })
        .as_ptr(),
    );
    AppendMenuW(menu, MF_STRING, COMMAND_SETTINGS, wide("Settings").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
    AppendMenuW(
        menu,
        MF_STRING,
        COMMAND_QUIT,
        wide("Exit Study Pen").as_ptr(),
    );
    SetMenuDefaultItem(menu, COMMAND_TOGGLE as u32, 0);
    let mut position = POINT::default();
    GetCursorPos(&mut position);
    SetForegroundWindow(window);
    let command = TrackPopupMenuEx(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        position.x,
        position.y,
        window,
        ptr::null(),
    ) as usize;
    // This lets the shell dismiss the menu reliably when clicking elsewhere.
    PostMessageW(window, WM_NULL, 0, 0);
    DestroyMenu(menu);
    command
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

unsafe fn create_icon() -> Result<HICON, String> {
    use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};
    const SIZE: u32 = 32;
    let mut pixels = Pixmap::new(SIZE, SIZE).ok_or("Could not allocate the tray icon.")?;
    let mut paint = Paint::default();
    paint.set_color_rgba8(30, 26, 23, 255); // BGRA for the native bitmap.
    let mut disk = PathBuilder::new();
    disk.push_circle(16.0, 16.0, 15.0);
    pixels.fill_path(
        &disk.finish().unwrap(),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    paint.set_color_rgba8(211, 229, 201, 255);
    let mut pen = PathBuilder::new();
    pen.move_to(10.0, 21.0);
    pen.line_to(12.0, 15.0);
    pen.line_to(21.0, 6.0);
    pen.line_to(26.0, 11.0);
    pen.line_to(17.0, 20.0);
    pen.close();
    pixels.fill_path(
        &pen.finish().unwrap(),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    paint.set_color_rgba8(30, 26, 23, 255);
    let mut seam = PathBuilder::new();
    seam.move_to(19.0, 8.0);
    seam.line_to(24.0, 13.0);
    pixels.stroke_path(
        &seam.finish().unwrap(),
        &paint,
        &Stroke {
            width: 1.5,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
    paint.set_color_rgba8(211, 229, 201, 255);
    let mut baseline = PathBuilder::new();
    baseline.move_to(8.0, 26.0);
    baseline.line_to(24.0, 26.0);
    pixels.stroke_path(
        &baseline.finish().unwrap(),
        &paint,
        &Stroke {
            width: 2.0,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: SIZE as i32,
            biHeight: -(SIZE as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = ptr::null_mut();
    let bitmap = CreateDIBSection(
        ptr::null_mut(),
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        ptr::null_mut(),
        0,
    );
    if bitmap.is_null() || bits.is_null() {
        if !bitmap.is_null() {
            DeleteObject(bitmap);
        }
        return Err("Could not create the tray icon bitmap.".into());
    }
    ptr::copy_nonoverlapping(
        pixels.data().as_ptr(),
        bits.cast::<u8>(),
        pixels.data().len(),
    );
    let mask = CreateBitmap(SIZE as i32, SIZE as i32, 1, 1, ptr::null());
    if mask.is_null() {
        DeleteObject(bitmap);
        return Err("Could not create the tray icon mask.".into());
    }
    let icon = CreateIconIndirect(&ICONINFO {
        fIcon: 1,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: bitmap,
    });
    DeleteObject(mask);
    DeleteObject(bitmap);
    if icon.is_null() {
        Err("Could not create the tray icon.".into())
    } else {
        Ok(icon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::{Foundation::RECT, System::LibraryLoader::GetModuleHandleW};

    struct HiddenWindow {
        window: HWND,
        class_name: Vec<u16>,
    }

    impl HiddenWindow {
        fn new() -> Self {
            static NEXT_CLASS: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_CLASS.fetch_add(1, Ordering::Relaxed);
            let class_name = wide(&format!("StudyPen.Tray.Test.{id}"));
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
                    "Could not register the hidden tray test window"
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
                    panic!("Could not create the hidden tray test window");
                }
                Self { window, class_name }
            }
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                SendMessageW(self.window, WM_SETICON, ICON_SMALL as usize, 0);
                SendMessageW(self.window, WM_SETICON, ICON_BIG as usize, 0);
                DestroyWindow(self.window);
                UnregisterClassW(self.class_name.as_ptr(), GetModuleHandleW(ptr::null()));
            }
        }
    }

    fn queued_command(window: HWND) -> Option<usize> {
        let mut message = MSG::default();
        let found = unsafe {
            PeekMessageW(
                &mut message,
                window,
                COMMAND_MESSAGE,
                COMMAND_MESSAGE,
                PM_REMOVE,
            )
        };
        (found != 0).then_some(message.wParam)
    }

    fn shell_has_icon(identifier: &NOTIFYICONIDENTIFIER) -> bool {
        let mut bounds = RECT::default();
        unsafe { Shell_NotifyIconGetRect(identifier, &mut bounds) >= 0 }
    }

    #[test]
    fn callbacks_queue_one_toggle_for_the_negotiated_shell_version() {
        let hidden = HiddenWindow::new();
        assert_eq!(queued_command(hidden.window), None);
        unsafe {
            callback(hidden.window, WM_LBUTTONUP, true, false);
        }
        assert_eq!(
            queued_command(hidden.window),
            None,
            "Version4 must ignore raw mouse-up to avoid double toggle"
        );
        unsafe {
            callback(hidden.window, NIN_SELECT, true, false);
        }
        assert_eq!(queued_command(hidden.window), Some(COMMAND_TOGGLE));
        assert_eq!(queued_command(hidden.window), None);
        unsafe {
            callback(hidden.window, NIN_KEYSELECT, false, false);
        }
        assert_eq!(queued_command(hidden.window), Some(COMMAND_TOGGLE));
        assert_eq!(queued_command(hidden.window), None);
        unsafe {
            callback(hidden.window, WM_LBUTTONUP, false, true);
        }
        assert_eq!(queued_command(hidden.window), Some(COMMAND_TOGGLE));
        assert_eq!(queued_command(hidden.window), None);
        unsafe {
            callback(hidden.window, WM_MOUSEMOVE, false, true);
        }
        assert_eq!(queued_command(hidden.window), None);
        assert_eq!(unsafe { IsWindowVisible(hidden.window) }, 0);
    }

    #[test]
    fn native_shell_duplicate_install_remove_and_readd_preserve_icon_lifecycle() {
        if unsafe { GetShellWindow() }.is_null() {
            eprintln!("Skipping tray shell integration: no Explorer shell is available.");
            return;
        }
        let hidden = HiddenWindow::new();
        let mut tray =
            unsafe { Tray::new(hidden.window) }.expect("Could not create the test tray icon");
        if !tray.is_added() {
            eprintln!(
                "Skipping tray shell integration: this session does not accept notification icons."
            );
            return;
        }
        let identifier = NOTIFYICONIDENTIFIER {
            cbSize: size_of::<NOTIFYICONIDENTIFIER>() as u32,
            hWnd: hidden.window,
            uID: ICON_ID,
            ..Default::default()
        };
        let geometry_available = shell_has_icon(&identifier);
        if !geometry_available {
            eprintln!(
                "Tray registration succeeded; icon geometry is unavailable in this shell session."
            );
        }
        unsafe {
            tray.install(false);
            assert!(
                tray.is_added(),
                "Duplicate install must retain an existing registered icon"
            );
            tray.update_visibility(false);
            assert!(
                tray.is_added(),
                "Updating the tooltip must preserve registration"
            );
            if geometry_available {
                assert!(shell_has_icon(&identifier));
            }

            // Reproduce stale bookkeeping from a failed duplicate NIM_ADD:
            // removal must delete the shell icon even if added is false.
            tray.added = false;
            tray.remove();
            assert!(!tray.is_added());
            if geometry_available {
                assert!(!shell_has_icon(&identifier));
            }
            tray.remove(); // Removing twice is harmless.
            tray.install(true);
            assert!(
                tray.is_added(),
                "The same identity must be reusable after removal"
            );
            if geometry_available {
                assert!(shell_has_icon(&identifier));
            }
            tray.remove();
            tray.install(false);
            assert!(tray.is_added());
            assert_eq!(IsWindowVisible(hidden.window), 0);
        }
        drop(tray);
        if geometry_available {
            assert!(
                !shell_has_icon(&identifier),
                "Drop must remove the owned notification icon"
            );
        }
    }
}
