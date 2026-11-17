//! Native presentation for the startup update check.

use crate::{identity, ink::Color, panel_primitives};
use std::ptr;
use windows_sys::Win32::{
    Foundation::RECT,
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{DI_NORMAL, DestroyIcon, DrawIconEx},
};

pub const WIDTH: i32 = 360;
pub const HEIGHT: i32 = 136;
pub const ACTION: (i32, i32, i32, i32) = (222, 96, 114, 28);

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
    fill_background(dc, bounds, 0.0, HEIGHT as f32);

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
        0,
    );
    draw_text(
        dc,
        stage.status(),
        rect(81, 59, 336, 83),
        px(15).max(1),
        400,
        rgb(204, 217, 236),
        0,
    );

    RestoreDC(dc, saved);
}

pub unsafe fn paint_action(
    dc: HDC,
    bounds: RECT,
    scale: f32,
    label: &str,
    pressed: bool,
    focused: bool,
) {
    let saved = SaveDC(dc);
    if saved == 0 {
        return;
    }
    fill_background(dc, bounds, ACTION.1 as f32, (ACTION.1 + ACTION.3) as f32);
    let inset = scale;
    let width = (bounds.right - bounds.left) as f32 - 2.0 * inset;
    let height = (bounds.bottom - bounds.top) as f32 - 2.0 * inset;
    let fill = if pressed {
        Color::rgb(41, 58, 82)
    } else if focused {
        Color::rgb(72, 92, 122)
    } else {
        Color::rgb(57, 73, 99)
    };
    panel_primitives::rounded(
        dc,
        [
            bounds.left as f32 + inset,
            bounds.top as f32 + inset,
            width,
            height,
        ],
        height / 2.0,
        fill,
        fill,
        0.0,
    );
    SetBkMode(dc, TRANSPARENT as i32);
    draw_text(
        dc,
        label,
        bounds,
        (13.0 * scale).round().max(1.0) as i32,
        500,
        rgb(241, 247, 255),
        DT_CENTER,
    );
    RestoreDC(dc, saved);
}

unsafe fn fill_background(dc: HDC, bounds: RECT, from_y: f32, to_y: f32) {
    let shade = |y: f32| -> [u8; 3] {
        let progress = y / HEIGHT as f32;
        let top = [47.0, 61.0, 83.0];
        let bottom = [23.0, 29.0, 44.0];
        std::array::from_fn(|index| {
            (top[index] + (bottom[index] - top[index]) * progress).round() as u8
        })
    };
    let top = shade(from_y);
    let bottom = shade(to_y);
    let vertices = [
        vertex(bounds.left, bounds.top, top),
        vertex(bounds.right, bounds.bottom, bottom),
    ];
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
        SetDCBrushColor(dc, rgb(top[0], top[1], top[2]));
        FillRect(dc, &bounds, GetStockObject(DC_BRUSH));
    }
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

unsafe fn draw_text(
    dc: HDC,
    value: &str,
    mut bounds: RECT,
    height: i32,
    weight: i32,
    color: u32,
    alignment: u32,
) {
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
        DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS | DT_NOPREFIX | alignment,
    );
    SelectObject(dc, old);
    if !font.is_null() {
        DeleteObject(font);
    }
}
