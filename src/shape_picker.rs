//! Native shape selection with standard Windows menu navigation.

use crate::ink::ShapeKind;
use std::mem::size_of;
use windows_sys::Win32::{
    Foundation::{HWND, RECT},
    Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromRect},
    UI::WindowsAndMessaging::*,
};

struct Menu(HMENU);

impl Drop for Menu {
    fn drop(&mut self) {
        unsafe { DestroyMenu(self.0) };
    }
}

/// Bounds identify the shape button in physical screen coordinates. The caller
/// releases application-state borrows before entering the native menu loop.
pub unsafe fn pick(
    owner: HWND,
    bounds: RECT,
    vertical: bool,
    selected: ShapeKind,
) -> Option<ShapeKind> {
    if owner.is_null() || bounds.left >= bounds.right || bounds.top >= bounds.bottom {
        return None;
    }
    let menu = Menu(CreatePopupMenu());
    if menu.0.is_null() {
        return None;
    }
    for (index, shape) in ShapeKind::ALL.into_iter().enumerate() {
        let checked = if shape == selected {
            MF_CHECKED
        } else {
            MF_UNCHECKED
        };
        let text: Vec<u16> = shape.label().encode_utf16().chain(Some(0)).collect();
        if AppendMenuW(menu.0, MF_STRING | checked, index + 1, text.as_ptr()) == 0 {
            return None;
        }
    }

    let mut flags = TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_TOPALIGN;
    let (x, y) = if vertical {
        let monitor = MonitorFromRect(&bounds, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let work = if GetMonitorInfoW(monitor, &mut info) != 0 {
            info.rcWork
        } else {
            bounds
        };
        let center = i64::from(bounds.left) + i64::from(bounds.right);
        let work_center = i64::from(work.left) + i64::from(work.right);
        flags |= TPM_VERTICAL;
        if center <= work_center {
            flags |= TPM_LEFTALIGN;
            (bounds.right, bounds.top)
        } else {
            flags |= TPM_RIGHTALIGN;
            (bounds.left, bounds.top)
        }
    } else {
        flags |= TPM_LEFTALIGN | TPM_HORIZONTAL;
        (bounds.left, bounds.bottom)
    };
    let exclude = TPMPARAMS {
        cbSize: size_of::<TPMPARAMS>() as u32,
        rcExclude: bounds,
    };
    SetForegroundWindow(owner);
    let command = TrackPopupMenuEx(menu.0, flags, x, y, owner, &exclude);
    PostMessageW(owner, WM_NULL, 0, 0);
    let index = usize::try_from(command).ok()?.checked_sub(1)?;
    ShapeKind::ALL.get(index).copied()
}
