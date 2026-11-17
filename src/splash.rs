//! Native presentation for the startup update check.

use crate::identity;
use std::ptr;
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx},
};

pub const WIDTH: i32 = 360;
pub const HEIGHT: i32 = 148;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Checking,
    Downloading,
    Installing,
}

impl Stage {
    pub const fn status(self) -> &'static str {
        match self {
            Self::Checking => "Checking for updates…",
            Self::Downloading => "Downloading and verifying…",
            Self::Installing => "Installing update…",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Checking => 0,
            Self::Downloading => 1,
            Self::Installing => 2,
        }
    }
}

/// The caller owns the DC and clips the window to its rounded outline.
pub unsafe fn paint(dc: HDC, scale: f32, stage: Stage) {
    if dc.is_null() || !scale.is_finite() || scale <= 0.0 {
        return;
    }
    let saved = SaveDC(dc);
    if saved == 0 {
        return;
    }
    let px = |dip: i32| (dip as f32 * scale).round() as i32;
    let rect = |left, top, right, bottom| RECT {
        left: px(left),
        top: px(top),
        right: px(right),
        bottom: px(bottom),
    };

    let bounds = rect(0, 0, WIDTH, HEIGHT);
    let top = vertex(0, 0, [47, 61, 83]);
    let bottom = vertex(bounds.right, bounds.bottom, [23, 29, 44]);
    let vertices = [top, bottom];
    let mesh = GRADIENT_RECT {
        UpperLeft: 0,
        LowerRight: 1,
    };
    if GdiGradientFill(
        dc,
        vertices.as_ptr(),
        2,
        (&raw const mesh).cast(),
        1,
        GRADIENT_FILL_RECT_V,
    ) == 0
    {
        SetDCBrushColor(dc, rgb(31, 40, 57));
        FillRect(dc, &bounds, GetStockObject(DC_BRUSH));
    }

    let border = CreatePen(PS_SOLID, 1, rgb(117, 138, 165));
    if !border.is_null() {
        let old_pen = SelectObject(dc, border);
        SelectObject(dc, GetStockObject(NULL_BRUSH));
        RoundRect(dc, 0, 0, bounds.right, bounds.bottom, px(26), px(26));
        SelectObject(dc, old_pen);
        DeleteObject(border);
    }

    let icon_size = px(40);
    if let Some(icon) = identity::load_icon(icon_size) {
        DrawIconEx(
            dc,
            px(24),
            px(32),
            icon,
            icon_size,
            icon_size,
            0,
            ptr::null_mut(),
            DI_NORMAL,
        );
        DestroyIcon(icon);
    }

    SetBkMode(dc, TRANSPARENT as i32);
    draw_text(
        dc,
        "Study Pen",
        rect(80, 28, 336, 56),
        px(24).max(1),
        600,
        rgb(248, 250, 255),
    );
    draw_text(
        dc,
        stage.status(),
        rect(81, 59, 336, 83),
        px(15).max(1),
        400,
        rgb(204, 217, 236),
    );

    SelectObject(dc, GetStockObject(DC_BRUSH));
    SelectObject(dc, GetStockObject(NULL_PEN));
    // The three marks represent stages, independently of download progress.
    for index in 0..3 {
        let color = match index.cmp(&stage.index()) {
            std::cmp::Ordering::Less => rgb(101, 137, 183),
            std::cmp::Ordering::Equal => rgb(174, 210, 254),
            std::cmp::Ordering::Greater => rgb(63, 78, 105),
        };
        SetDCBrushColor(dc, color);
        let mark = rect(24 + 30 * index as i32, 118, 48 + 30 * index as i32, 122);
        RoundRect(
            dc,
            mark.left,
            mark.top,
            mark.right,
            mark.bottom,
            px(4),
            px(4),
        );
    }
    RestoreDC(dc, saved);
}

fn vertex(x: i32, y: i32, [red, green, blue]: [u8; 3]) -> TRIVERTEX {
    TRIVERTEX {
        x,
        y,
        Red: u16::from(red) << 8,
        Green: u16::from(green) << 8,
        Blue: u16::from(blue) << 8,
        Alpha: 0,
    }
}

const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    red as u32 | ((green as u32) << 8) | ((blue as u32) << 16)
}

unsafe fn draw_text(dc: HDC, value: &str, mut bounds: RECT, height: i32, weight: i32, color: u32) {
    let font = CreateFontW(
        -height,
        0,
        0,
        0,
        weight,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        windows_sys::w!("Segoe UI"),
    );
    let chosen = if font.is_null() {
        GetStockObject(DEFAULT_GUI_FONT)
    } else {
        font
    };
    let old = SelectObject(dc, chosen);
    SetTextColor(dc, color);
    let text: Vec<_> = value.encode_utf16().collect();
    DrawTextW(
        dc,
        text.as_ptr(),
        text.len() as i32,
        &mut bounds,
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX,
    );
    SelectObject(dc, old);
    if !font.is_null() {
        DeleteObject(font);
    }
}
