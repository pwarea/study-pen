//! Native settings views and antialiased toolbar icons in logical pixels.

use crate::ink::{Color, Tool};
use std::mem::size_of;
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};
use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
};

pub const TOOLBAR_WIDTH: i32 = 480;
pub const TOOLBAR_HEIGHT: i32 = 60;
pub const PANEL_WIDTH: i32 = 460;
pub const PANEL_HEIGHT: i32 = 620;
pub const SHORTCUT_TOP: i32 = 124;
pub const SHORTCUT_STEP: i32 = 44;
pub const SHORTCUT_HEIGHT: i32 = 38;
pub const UPDATE_BUTTON_TOP: i32 = 234;
pub const UPDATE_BUTTON_BOTTOM: i32 = 270;
const TOOL_BUTTON_COUNT: usize = 10;
const BUTTON_STARTS: [i32; TOOL_BUTTON_COUNT] = [51, 91, 131, 171, 218, 258, 305, 345, 392, 432];
const BUTTON_SIZE: i32 = 38;
const BUTTON_INSET: i32 = (TOOLBAR_HEIGHT - BUTTON_SIZE) / 2;
const HANDLE_LENGTH: i32 = 45;

pub const TOOLBAR_TOOLTIPS: [&str; TOOL_BUTTON_COUNT] = [
    "Cursor",
    "Pen",
    "Eraser",
    "Object eraser",
    "Undo",
    "Clear screen",
    "Color",
    "Settings",
    "Hide",
    "Exit",
];

/// Layout geometry shared by native hit testing, docking, and painting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarLayout {
    #[default]
    Horizontal,
    Vertical,
}

impl ToolbarLayout {
    pub const fn size(self) -> (i32, i32) {
        match self {
            Self::Horizontal => (TOOLBAR_WIDTH, TOOLBAR_HEIGHT),
            Self::Vertical => (TOOLBAR_HEIGHT, TOOLBAR_WIDTH),
        }
    }

    pub const fn handle_size(self) -> (i32, i32) {
        match self {
            Self::Horizontal => (HANDLE_LENGTH, TOOLBAR_HEIGHT),
            Self::Vertical => (TOOLBAR_HEIGHT, HANDLE_LENGTH),
        }
    }

    pub(crate) fn button_bounds(self, index: usize) -> Option<(i32, i32, i32, i32)> {
        let start = *BUTTON_STARTS.get(index)?;
        Some(match self {
            Self::Horizontal => (start, BUTTON_INSET, BUTTON_SIZE, BUTTON_SIZE),
            Self::Vertical => (BUTTON_INSET, start, BUTTON_SIZE, BUTTON_SIZE),
        })
    }

    pub fn hit_test(self, x: i32, y: i32) -> Option<usize> {
        (0..TOOL_BUTTON_COUNT).find(|index| {
            let (left, top, width, height) = self.button_bounds(*index).unwrap();
            (left..left + width).contains(&x) && (top..top + height).contains(&y)
        })
    }

    pub fn is_drag_handle(self, x: i32, y: i32) -> bool {
        let (width, height) = self.handle_size();
        (0..width).contains(&x) && (0..height).contains(&y)
    }
}

pub const PALETTE: [Color; 12] = [
    Color::rgb(244, 246, 245),
    Color::rgb(27, 31, 35),
    Color::rgb(239, 83, 80),
    Color::rgb(255, 148, 77),
    Color::rgb(250, 211, 91),
    Color::rgb(158, 211, 136),
    Color::rgb(67, 191, 163),
    Color::rgb(71, 195, 224),
    Color::rgb(91, 148, 245),
    Color::rgb(155, 132, 235),
    Color::rgb(226, 130, 199),
    Color::rgb(153, 163, 170),
];

const BG: Color = Color::rgb(23, 28, 32);
const SURFACE: Color = Color::rgb(32, 39, 44);
const BORDER: Color = Color::rgb(55, 65, 72);
const TEXT: Color = Color::rgb(239, 242, 238);
const MUTED: Color = Color::rgb(151, 163, 168);
const ACCENT: Color = Color::rgb(200, 222, 187);
const SELECTED: Color = Color::rgb(50, 64, 53);

pub struct UiState {
    pub primary: Color,
    pub secondary: Color,
    pub tab: usize,
    pub editing_secondary: bool,
    /// Hue is measured in degrees; saturation and value are in 0..=1.
    pub hue: f32,
    pub saturation: f32,
    pub value: f32,
    pub hotkeys: [String; 9],
    pub hotkey_errors: [Option<String>; 9],
    pub capturing: Option<usize>,
    pub check_for_updates: bool,
    pub installing_update: bool,
    pub update_message: String,
    pub update_action: Option<&'static str>,
    pub storage_message: String,
    pub status: String,
    pub pen_width: f32,
    pub eraser_diameter: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ToolbarControls {
    pub layout: ToolbarLayout,
    pub interactive: bool,
    pub tool: Tool,
    pub primary: Color,
    pub hover: Option<usize>,
    pub dark: bool,
}

impl ToolbarControls {
    pub fn selected(&self) -> usize {
        if !self.interactive {
            return 0;
        }
        match self.tool {
            Tool::Pen => 1,
            Tool::Eraser => 2,
            Tool::ObjectEraser => 3,
        }
    }
}

/// Converts an opaque sRGB color into hue degrees, saturation, and value.
pub fn rgb_to_hsv(color: Color) -> (f32, f32, f32) {
    let r = color.r as f32 / 255.0;
    let g = color.g as f32 / 255.0;
    let b = color.b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let h = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        ((g - b) / delta).rem_euclid(6.0) * 60.0
    } else if max == g {
        ((b - r) / delta + 2.0) * 60.0
    } else {
        ((r - g) / delta + 4.0) * 60.0
    };
    (
        h,
        if max <= f32::EPSILON {
            0.0
        } else {
            delta / max
        },
        max,
    )
}

pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color {
    let h = if h.is_finite() {
        h.rem_euclid(360.0) / 60.0
    } else {
        0.0
    };
    let s = if s.is_finite() {
        s.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let v = if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let c = v * s;
    let x = c * (1.0 - (h.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match h as u8 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    Color::rgb(
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    )
}

struct OwnedObject(HGDIOBJ);
impl Drop for OwnedObject {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

const FONT_SPECS: [(f32, i32); 6] = [
    (13.0, 400),
    (11.0, 400),
    (9.0, 400),
    (16.0, 600),
    (22.0, 600),
    (13.0, 600),
];

struct FontSet {
    objects: [OwnedObject; 6],
}

impl FontSet {
    fn heights(scale: f32) -> [i32; 6] {
        FONT_SPECS.map(|(size, _)| -(size * scale.max(0.25)).round() as i32)
    }

    unsafe fn new(heights: [i32; 6]) -> Self {
        let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();
        Self {
            objects: std::array::from_fn(|index| {
                OwnedObject(unsafe {
                    CreateFontW(
                        heights[index],
                        0,
                        0,
                        0,
                        FONT_SPECS[index].1,
                        0,
                        0,
                        0,
                        DEFAULT_CHARSET as u32,
                        OUT_DEFAULT_PRECIS as u32,
                        CLIP_DEFAULT_PRECIS as u32,
                        CLEARTYPE_QUALITY as u32,
                        DEFAULT_PITCH as u32,
                        face.as_ptr(),
                    )
                })
            }),
        }
    }
}

struct Painter<'a> {
    dc: HDC,
    scale: f32,
    saved: i32,
    fonts: Option<&'a FontSet>,
}

impl<'a> Painter<'a> {
    unsafe fn new(dc: HDC, scale: f32, fonts: Option<&'a FontSet>) -> Self {
        let saved = unsafe { SaveDC(dc) };
        unsafe {
            SetBkMode(dc, TRANSPARENT as i32);
        }
        Self {
            dc,
            scale,
            saved,
            fonts,
        }
    }

    fn px(&self, value: f32) -> i32 {
        (value * self.scale).round() as i32
    }

    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> RECT {
        RECT {
            left: self.px(x),
            top: self.px(y),
            right: self.px(x + w),
            bottom: self.px(y + h),
        }
    }

    fn fill(&self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let brush = OwnedObject(unsafe { CreateSolidBrush(cref(color)) });
        unsafe {
            FillRect(self.dc, &self.rect(x, y, w, h), brush.0);
        }
    }

    // Keeping rectangle coordinates and paint attributes together makes these
    // immediate-mode view definitions easier to compare with their hit areas.
    #[allow(clippy::too_many_arguments)]
    fn rounded(&self, x: f32, y: f32, w: f32, h: f32, radius: f32, fill: Color, border: Color) {
        let brush = OwnedObject(unsafe { CreateSolidBrush(cref(fill)) });
        let pen = OwnedObject(unsafe { CreatePen(PS_SOLID, self.px(1.0).max(1), cref(border)) });
        unsafe {
            let old_brush = SelectObject(self.dc, brush.0);
            let old_pen = SelectObject(self.dc, pen.0);
            RoundRect(
                self.dc,
                self.px(x),
                self.px(y),
                self.px(x + w),
                self.px(y + h),
                self.px(radius * 2.0),
                self.px(radius * 2.0),
            );
            SelectObject(self.dc, old_pen);
            SelectObject(self.dc, old_brush);
        }
    }

    fn circle(&self, x: f32, y: f32, radius: f32, fill: Option<Color>, border: Color, width: f32) {
        let brush = fill.map(|color| OwnedObject(unsafe { CreateSolidBrush(cref(color)) }));
        let pen = OwnedObject(unsafe { CreatePen(PS_SOLID, self.px(width).max(1), cref(border)) });
        unsafe {
            let old_brush = SelectObject(
                self.dc,
                brush
                    .as_ref()
                    .map_or(GetStockObject(NULL_BRUSH), |brush| brush.0),
            );
            let old_pen = SelectObject(self.dc, pen.0);
            Ellipse(
                self.dc,
                self.px(x - radius),
                self.px(y - radius),
                self.px(x + radius),
                self.px(y + radius),
            );
            SelectObject(self.dc, old_pen);
            SelectObject(self.dc, old_brush);
        }
    }

    fn line<const N: usize>(&self, points: &[(f32, f32); N], color: Color, width: f32) {
        let pen = OwnedObject(unsafe { CreatePen(PS_SOLID, self.px(width).max(1), cref(color)) });
        let points = points.map(|(x, y)| POINT {
            x: self.px(x),
            y: self.px(y),
        });
        unsafe {
            let old = SelectObject(self.dc, pen.0);
            Polyline(self.dc, points.as_ptr(), points.len() as i32);
            SelectObject(self.dc, old);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text(
        &self,
        value: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        font: usize,
        color: Color,
        flags: u32,
    ) {
        let text: Vec<u16> = value.encode_utf16().collect();
        let mut rect = self.rect(x, y, w, h);
        unsafe {
            let fonts = self.fonts.expect("Text painting requires a panel font set");
            let old_font = SelectObject(self.dc, fonts.objects[font].0);
            let old_color = SetTextColor(self.dc, cref(color));
            DrawTextW(
                self.dc,
                text.as_ptr(),
                text.len() as i32,
                &mut rect,
                flags | DT_NOPREFIX,
            );
            SetTextColor(self.dc, old_color);
            SelectObject(self.dc, old_font);
        }
    }

    fn label(&self, value: &str, x: f32, y: f32, w: f32, font: usize, color: Color) {
        self.text(
            value,
            x,
            y,
            w,
            24.0,
            font,
            color,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn center(&self, value: &str, x: f32, y: f32, w: f32, h: f32, font: usize, color: Color) {
        self.text(
            value,
            x,
            y,
            w,
            h,
            font,
            color,
            DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_END_ELLIPSIS,
        );
    }

    fn bitmap(&self, x: f32, y: f32, width: i32, height: i32, data: &[u8]) {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            StretchDIBits(
                self.dc,
                self.px(x),
                self.px(y),
                self.px(width as f32),
                self.px(height as f32),
                0,
                0,
                width,
                height,
                data.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
                SRCCOPY,
            );
        }
    }
}

impl Drop for Painter<'_> {
    fn drop(&mut self) {
        if self.saved != 0 {
            unsafe {
                RestoreDC(self.dc, self.saved);
            }
        }
    }
}

fn cref(color: Color) -> u32 {
    color.r as u32 | ((color.g as u32) << 8) | ((color.b as u32) << 16)
}

/// Produces premultiplied BGRA pixels at the window's actual physical size.
pub fn toolbar_pixels(
    width: u32,
    height: u32,
    scale: f32,
    state: &ToolbarControls,
) -> Option<Vec<u32>> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let mut pixmap = Pixmap::new(width, height)?;
    let (ink, grip) = if state.dark {
        (Color::rgb(250, 251, 254), Color::rgb(193, 200, 215))
    } else {
        (Color::rgb(24, 27, 34), Color::rgb(66, 74, 88))
    };
    let (grip_x, grip_y) = match state.layout {
        ToolbarLayout::Horizontal => (26.0, 30.0),
        ToolbarLayout::Vertical => (30.0, 26.0),
    };
    for dx in [-2.5, 2.5] {
        for dy in [-4.0, 0.0, 4.0] {
            let circle = PathBuilder::from_circle(grip_x + dx, grip_y + dy, 0.72)?;
            pixmap.fill_path(
                &circle,
                &icon_paint(grip),
                FillRule::Winding,
                Transform::from_scale(scale, scale),
                None,
            );
        }
    }
    for index in 0..TOOL_BUTTON_COUNT {
        let (left, top, width, height) = state.layout.button_bounds(index).unwrap();
        let x = left as f32 + width as f32 / 2.0;
        let y = top as f32 + height as f32 / 2.0;
        let transform = Transform::from_row(
            19.0 * scale / 24.0,
            0.0,
            0.0,
            19.0 * scale / 24.0,
            (x - 9.5) * scale,
            (y - 9.5) * scale,
        );
        let path = toolbar_icon(index)?;
        let stroke = Stroke {
            width: 1.65,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &icon_paint(ink), &stroke, transform, None);
        if index == 6 {
            let swatch = PathBuilder::from_circle(12.0, 12.0, 5.7)?;
            pixmap.fill_path(
                &swatch,
                &icon_paint(state.primary),
                FillRule::Winding,
                transform,
                None,
            );
        }
    }
    Some(
        pixmap
            .pixels()
            .iter()
            .map(|pixel| {
                u32::from(pixel.blue())
                    | (u32::from(pixel.green()) << 8)
                    | (u32::from(pixel.red()) << 16)
                    | (u32::from(pixel.alpha()) << 24)
            })
            .collect(),
    )
}

fn icon_paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, 255);
    paint.anti_alias = true;
    paint
}

/// The 24-unit paths match the reviewed toolbar preview.
fn toolbar_icon(index: usize) -> Option<Path> {
    let mut path = PathBuilder::new();
    match index {
        0 => {
            path.move_to(5.0, 3.0);
            path.line_to(18.7, 12.7);
            path.line_to(11.9, 14.1);
            path.line_to(8.0, 19.9);
            path.close();
        }
        1 => {
            path.move_to(4.5, 19.5);
            path.line_to(5.6, 13.9);
            path.line_to(16.0, 3.6);
            path.cubic_to(16.61, 2.99, 17.59, 2.99, 18.2, 3.6);
            path.line_to(20.4, 5.8);
            path.cubic_to(21.01, 6.41, 21.01, 7.39, 20.4, 8.0);
            path.line_to(10.0, 18.4);
            path.close();
            path.move_to(13.7, 5.9);
            path.line_to(18.2, 10.4);
            path.move_to(12.5, 21.0);
            path.line_to(19.5, 21.0);
        }
        2 => {
            path.move_to(3.8, 14.0);
            path.line_to(12.7, 4.8);
            path.cubic_to(13.46, 4.01, 14.74, 3.96, 15.5, 4.7);
            path.line_to(19.6, 8.5);
            path.cubic_to(20.41, 9.25, 20.46, 10.54, 19.7, 11.3);
            path.line_to(12.3, 19.2);
            path.line_to(8.4, 19.2);
            path.line_to(3.9, 16.2);
            path.cubic_to(3.18, 15.73, 3.13, 14.65, 3.8, 14.0);
            path.close();
            path.move_to(8.8, 9.0);
            path.line_to(15.7, 15.4);
            path.move_to(12.3, 19.2);
            path.line_to(21.0, 19.2);
        }
        3 => {
            path.move_to(7.0, 3.0);
            path.line_to(4.0, 3.0);
            path.line_to(4.0, 6.0);
            path.move_to(17.0, 3.0);
            path.line_to(20.0, 3.0);
            path.line_to(20.0, 6.0);
            path.move_to(20.0, 17.0);
            path.line_to(20.0, 20.0);
            path.line_to(17.0, 20.0);
            path.move_to(4.0, 17.0);
            path.line_to(4.0, 20.0);
            path.line_to(7.0, 20.0);
            path.move_to(7.2, 12.5);
            path.line_to(13.0, 6.5);
            path.line_to(17.3, 10.7);
            path.line_to(11.9, 16.5);
            path.line_to(10.0, 16.5);
            path.close();
            path.move_to(9.3, 10.3);
            path.line_to(13.8, 14.6);
        }
        4 => {
            path.move_to(9.0, 5.0);
            path.line_to(4.0, 10.0);
            path.line_to(9.0, 15.0);
            path.move_to(4.0, 10.0);
            path.line_to(14.7, 10.0);
            path.cubic_to(17.35, 10.0, 19.5, 12.15, 19.5, 14.8);
            path.cubic_to(19.5, 17.45, 17.35, 19.6, 14.7, 19.6);
            path.line_to(13.0, 19.6);
        }
        5 => {
            path.move_to(5.0, 6.0);
            path.line_to(19.0, 6.0);
            path.move_to(9.0, 6.0);
            path.line_to(9.0, 4.0);
            path.line_to(15.0, 4.0);
            path.line_to(15.0, 6.0);
            path.move_to(17.6, 8.5);
            path.line_to(16.7, 19.5);
            path.line_to(7.3, 19.5);
            path.line_to(6.4, 8.5);
            path.move_to(10.0, 10.0);
            path.line_to(10.0, 16.0);
            path.move_to(14.0, 10.0);
            path.line_to(14.0, 16.0);
        }
        6 => path.push_circle(12.0, 12.0, 8.3),
        7 => {
            for (y, x) in [(6.0, 10.0), (12.0, 14.0), (18.0, 9.0)] {
                path.move_to(5.0, y);
                path.line_to(19.0, y);
                path.move_to(x, y - 2.0);
                path.line_to(x, y + 2.0);
            }
        }
        8 => {
            path.move_to(4.5, 5.0);
            path.line_to(19.5, 5.0);
            path.line_to(19.5, 18.0);
            path.line_to(4.5, 18.0);
            path.close();
            path.move_to(8.0, 14.5);
            path.line_to(16.0, 14.5);
        }
        9 => {
            path.move_to(12.0, 5.0);
            path.line_to(5.5, 5.0);
            path.line_to(5.5, 19.0);
            path.line_to(12.0, 19.0);
            path.move_to(13.5, 12.0);
            path.line_to(19.5, 12.0);
            path.move_to(16.5, 9.0);
            path.line_to(19.5, 12.0);
            path.line_to(16.5, 15.0);
        }
        _ => return None,
    }
    path.finish()
}

/// Paints the fixed 460 by 620 DIP companion panel.
pub unsafe fn paint_panel(dc: HDC, scale: f32, state: &UiState) {
    let fonts = unsafe { FontSet::new(FontSet::heights(scale)) };
    let p = unsafe { Painter::new(dc, scale.max(0.25), Some(&fonts)) };
    p.fill(0.0, 0.0, PANEL_WIDTH as f32, PANEL_HEIGHT as f32, BG);
    p.rounded(
        0.0,
        0.0,
        PANEL_WIDTH as f32,
        PANEL_HEIGHT as f32,
        14.0,
        BG,
        BORDER,
    );
    p.text(
        "Study Pen",
        24.0,
        19.0,
        280.0,
        31.0,
        4,
        TEXT,
        DT_SINGLELINE | DT_VCENTER,
    );
    p.label(
        "A little ink. A clearer thought.",
        24.0,
        47.0,
        330.0,
        1,
        MUTED,
    );
    p.line(&[(424.0, 24.0), (436.0, 36.0)], MUTED, 1.5);
    p.line(&[(436.0, 24.0), (424.0, 36.0)], MUTED, 1.5);
    let tabs = ["Colors", "Shortcuts", "General"];
    for (index, label) in tabs.into_iter().enumerate() {
        let x = 24.0 + index as f32 * 140.0;
        let active = state.tab == index;
        p.rounded(
            x,
            76.0,
            130.0,
            32.0,
            7.0,
            if active { SELECTED } else { BG },
            if active { SELECTED } else { BG },
        );
        p.center(
            label,
            x,
            76.0,
            130.0,
            32.0,
            if active { 5 } else { 0 },
            if active { ACCENT } else { MUTED },
        );
    }
    match state.tab {
        1 => shortcuts(&p, state),
        2 => general(&p, state),
        _ => colors(&p, state),
    }
    if !state.status.is_empty() {
        p.label(
            &state.status,
            24.0,
            586.0,
            412.0,
            1,
            Color::rgb(244, 187, 137),
        );
    }
}

fn swatch(p: &Painter<'_>, x: f32, label: &str, color: Color, selected: bool) {
    p.rounded(
        x,
        124.0,
        196.0,
        56.0,
        9.0,
        SURFACE,
        if selected { ACCENT } else { BORDER },
    );
    p.circle(x + 26.0, 152.0, 13.0, Some(color), BORDER, 1.0);
    p.label(
        label,
        x + 50.0,
        129.0,
        134.0,
        2,
        if selected { ACCENT } else { MUTED },
    );
    p.label(&color.to_hex(), x + 50.0, 149.0, 134.0, 5, TEXT);
    if selected {
        p.circle(x + 179.0, 137.0, 2.2, Some(ACCENT), ACCENT, 1.0);
    }
}

fn colors(p: &Painter<'_>, state: &UiState) {
    swatch(p, 24.0, "PRIMARY", state.primary, !state.editing_secondary);
    swatch(
        p,
        240.0,
        "SECONDARY",
        state.secondary,
        state.editing_secondary,
    );
    // The field is a true continuous HSV plane, without a fixed color table.
    let hue = hsv_to_rgb(state.hue, 1.0, 1.0);
    let mut plane = vec![0u8; 412 * 170 * 4];
    for y in 0..170 {
        let v = 1.0 - y as f32 / 169.0;
        for x in 0..412 {
            let s = x as f32 / 411.0;
            let offset = (y * 412 + x) * 4;
            for (channel, source) in [hue.b, hue.g, hue.r].into_iter().enumerate() {
                plane[offset + channel] =
                    ((255.0 * (1.0 - s) + source as f32 * s) * v).round() as u8;
            }
        }
    }
    p.bitmap(24.0, 202.0, 412, 170, &plane);
    let marker_x = 24.0 + state.saturation.clamp(0.0, 1.0) * 412.0;
    let marker_y = 202.0 + (1.0 - state.value.clamp(0.0, 1.0)) * 170.0;
    p.circle(marker_x, marker_y, 7.0, None, Color::rgb(20, 20, 20), 3.0);
    p.circle(marker_x, marker_y, 6.0, None, TEXT, 1.7);
    let mut hue_strip = vec![0u8; 412 * 16 * 4];
    for x in 0..412 {
        let color = hsv_to_rgb(x as f32 / 411.0 * 360.0, 1.0, 1.0);
        for y in 0..16 {
            let offset = (y * 412 + x) * 4;
            hue_strip[offset] = color.b;
            hue_strip[offset + 1] = color.g;
            hue_strip[offset + 2] = color.r;
        }
    }
    p.bitmap(24.0, 388.0, 412, 16, &hue_strip);
    let hue_x = 24.0 + state.hue.clamp(0.0, 360.0) / 360.0 * 412.0;
    p.rounded(hue_x - 3.0, 384.0, 6.0, 24.0, 3.0, TEXT, BG);
    for (label, x, width) in [
        ("HEX", 24.0, 128.0),
        ("R", 176.0, 72.0),
        ("G", 264.0, 72.0),
        ("B", 352.0, 84.0),
    ] {
        p.label(label, x, 420.0, width, 2, MUTED);
        p.rounded(x, 448.0, width, 32.0, 5.0, SURFACE, BORDER);
    }
    p.label("QUICK COLORS", 24.0, 491.0, 300.0, 2, MUTED);
    let current = if state.editing_secondary {
        state.secondary
    } else {
        state.primary
    };
    for (index, color) in PALETTE.into_iter().enumerate() {
        let x = 42.0 + index as f32 * 35.0;
        if current == color {
            p.circle(x, 526.0, 15.5, None, ACCENT, 1.3);
        }
        p.circle(x, 526.0, 11.5, Some(color), BORDER, 1.0);
    }
    p.label(
        "Changes are saved automatically.",
        24.0,
        559.0,
        412.0,
        1,
        MUTED,
    );
}

fn shortcuts(p: &Painter<'_>, state: &UiState) {
    let labels = [
        "Toggle pen",
        "Clear screen",
        "Object eraser",
        "Undo",
        "Normal eraser",
        "Change color",
        "Hide / show toolbar",
        "Exit Study Pen",
        "Radial menu",
    ];
    let hints = [
        "Draw or return to your apps",
        "Remove all visible ink",
        "Erase an entire stroke",
        "Undo the last gesture",
        "Hold and move to resize",
        "Swap primary and secondary",
        "Works while the toolbar is hidden",
        "Save settings and close",
        "Hold to choose a tool or color",
    ];
    for (index, label) in labels.into_iter().enumerate() {
        let y = (SHORTCUT_TOP + index as i32 * SHORTCUT_STEP) as f32;
        p.text(
            label,
            24.0,
            y + 1.0,
            198.0,
            18.0,
            5,
            TEXT,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        p.text(
            hints[index],
            24.0,
            y + 21.0,
            198.0,
            17.0,
            1,
            MUTED,
            DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
        );
        let capture = state.capturing == Some(index);
        let unavailable = state.hotkey_errors[index].is_some() && !capture;
        let warning = Color::rgb(244, 187, 137);
        p.rounded(
            230.0,
            y,
            206.0,
            SHORTCUT_HEIGHT as f32,
            7.0,
            if capture { SELECTED } else { SURFACE },
            if capture {
                ACCENT
            } else if unavailable {
                warning
            } else {
                BORDER
            },
        );
        if unavailable {
            p.center(&state.hotkeys[index], 238.0, y + 2.0, 190.0, 22.0, 0, TEXT);
            p.center("Unavailable", 238.0, y + 24.0, 190.0, 13.0, 2, warning);
        } else if capture {
            p.center("Press a shortcut…", 238.0, y + 1.0, 190.0, 22.0, 0, ACCENT);
            p.center(
                "Backspace to unbind",
                238.0,
                y + 23.0,
                190.0,
                13.0,
                2,
                MUTED,
            );
        } else {
            p.center(
                &state.hotkeys[index],
                238.0,
                y,
                190.0,
                SHORTCUT_HEIGHT as f32,
                0,
                TEXT,
            );
        }
    }
    p.text("Hold the eraser shortcut, then move up to grow\nor down to shrink. Release to keep the size.",
        24.0, 548.0, 412.0, 38.0, 1, MUTED, DT_WORDBREAK);
}

fn slider(p: &Painter<'_>, y: f32, t: f32) {
    let x = 24.0 + 412.0 * t.clamp(0.0, 1.0);
    p.rounded(24.0, y - 2.0, 412.0, 4.0, 2.0, BORDER, BORDER);
    if x > 25.0 {
        p.rounded(24.0, y - 2.0, x - 24.0, 4.0, 2.0, ACCENT, ACCENT);
    }
    p.circle(x, y, 7.0, Some(ACCENT), BG, 1.5);
}

fn general(p: &Painter<'_>, state: &UiState) {
    let enabled = state.check_for_updates && !state.installing_update;
    p.rounded(
        24.0,
        146.0,
        20.0,
        20.0,
        4.0,
        if enabled { ACCENT } else { SURFACE },
        if enabled { ACCENT } else { BORDER },
    );
    if state.check_for_updates {
        p.line(
            &[(28.0, 156.0), (32.0, 160.0), (40.0, 151.0)],
            if enabled { BG } else { MUTED },
            1.8,
        );
    }
    p.label(
        "Automatic updates at startup",
        58.0,
        143.0,
        378.0,
        5,
        if state.installing_update { MUTED } else { TEXT },
    );
    p.text(
        &state.update_message,
        24.0,
        196.0,
        412.0,
        if state.update_action.is_some() {
            36.0
        } else {
            69.0
        },
        1,
        MUTED,
        DT_WORDBREAK,
    );
    if let Some(action) = state.update_action {
        let top = UPDATE_BUTTON_TOP as f32;
        let height = (UPDATE_BUTTON_BOTTOM - UPDATE_BUTTON_TOP) as f32;
        p.rounded(24.0, top, 412.0, height, 8.0, SURFACE, BORDER);
        p.center(action, 24.0, top, 412.0, height, 0, TEXT);
    }
    p.line(&[(24.0, 278.0), (436.0, 278.0)], BORDER, 1.0);
    p.label("Pen width", 24.0, 288.0, 250.0, 5, TEXT);
    p.text(
        &format!("{:.1} px", state.pen_width),
        300.0,
        288.0,
        136.0,
        24.0,
        0,
        ACCENT,
        DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
    );
    slider(p, 330.0, (state.pen_width - 1.0) / 15.0);
    p.label("Eraser diameter", 24.0, 384.0, 250.0, 5, TEXT);
    p.text(
        &format!("{:.0} px", state.eraser_diameter),
        300.0,
        384.0,
        136.0,
        24.0,
        0,
        ACCENT,
        DT_SINGLELINE | DT_VCENTER | DT_RIGHT,
    );
    slider(p, 426.0, (state.eraser_diameter - 6.0) / 234.0);
    p.text(
        &state.storage_message,
        24.0,
        466.0,
        412.0,
        42.0,
        1,
        MUTED,
        DT_WORDBREAK,
    );
    p.rounded(24.0, 528.0, 196.0, 40.0, 8.0, SURFACE, BORDER);
    p.center(
        "Exit Study Pen",
        24.0,
        528.0,
        196.0,
        40.0,
        0,
        if state.installing_update { MUTED } else { TEXT },
    );
    let remove_text = if state.installing_update {
        MUTED
    } else {
        Color::rgb(238, 170, 170)
    };
    p.rounded(
        236.0,
        528.0,
        200.0,
        40.0,
        8.0,
        SURFACE,
        Color::rgb(102, 65, 69),
    );
    p.center(
        "Remove Study Pen…",
        236.0,
        528.0,
        200.0,
        40.0,
        0,
        remove_text,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolbar_state(layout: ToolbarLayout, dark: bool) -> ToolbarControls {
        ToolbarControls {
            layout,
            interactive: true,
            tool: Tool::Eraser,
            primary: Color::rgb(99, 102, 241),
            hover: Some(7),
            dark,
        }
    }

    #[test]
    fn toolbar_raster_preserves_antialiased_icons_at_common_dpi_and_fitted_scales() {
        let scales = [96, 120, 144, 192].map(|dpi| dpi as f32 / 96.0);
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            for scale in scales.into_iter().chain([0.4]) {
                let (width, height) = layout.size();
                let physical = |dip: i32| (dip as f32 * scale).round() as usize;
                let width = physical(width);
                let height = physical(height);
                for dark in [false, true] {
                    let pixels = toolbar_pixels(
                        width as u32,
                        height as u32,
                        scale,
                        &toolbar_state(layout, dark),
                    )
                    .expect("rasterize the actual toolbar controls");
                    assert_eq!(pixels.len(), width * height);
                    assert_eq!(pixels[0], 0);
                    assert_eq!(pixels[width * height - 1], 0);
                    for pixel in &pixels {
                        let [blue, green, red, alpha] = pixel.to_le_bytes();
                        assert!(blue <= alpha && green <= alpha && red <= alpha);
                    }
                    for index in 0..TOOL_BUTTON_COUNT {
                        let (x, y, w, h) = layout.button_bounds(index).unwrap();
                        let mut coverage = [false; 256];
                        for row in physical(y)..physical(y + h) {
                            for column in physical(x)..physical(x + w) {
                                let alpha = (pixels[row * width + column] >> 24) as usize;
                                if alpha != 0 {
                                    coverage[alpha] = true;
                                }
                            }
                        }
                        assert!(
                            coverage.iter().filter(|present| **present).count() >= 3,
                            "Missing antialiased edge coverage for {layout:?} icon {index}, scale {scale}, dark {dark}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn near_black_primary_swatch_keeps_its_opaque_color_and_alpha() {
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let (width, height) = layout.size();
                let physical = |dip: i32| (dip as f32 * scale).round() as usize;
                let mut state = toolbar_state(layout, false);
                state.primary = Color::rgb(1, 2, 3);
                let pixels = toolbar_pixels(
                    physical(width) as u32,
                    physical(height) as u32,
                    scale,
                    &state,
                )
                .unwrap();
                let (x, y, width, height) = layout.button_bounds(6).unwrap();
                let center =
                    physical(y + height / 2) * physical(layout.size().0) + physical(x + width / 2);
                assert_eq!(pixels[center], 0xff01_0203);
            }
        }
    }

    #[test]
    fn both_toolbar_layouts_keep_every_button_inside_its_hit_bounds() {
        assert_eq!(ToolbarLayout::Horizontal.size(), (480, 60));
        assert_eq!(ToolbarLayout::Vertical.size(), (60, 480));
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            let (width, height) = layout.size();
            for index in 0..TOOL_BUTTON_COUNT {
                let (left, top, button_width, button_height) = layout.button_bounds(index).unwrap();
                assert!(
                    left >= 0
                        && top >= 0
                        && left + button_width <= width
                        && top + button_height <= height
                );
                assert_eq!(
                    layout.hit_test(left + button_width / 2, top + button_height / 2),
                    Some(index)
                );
                assert_eq!(layout.hit_test(left, top), Some(index));
                assert_eq!(
                    layout.hit_test(left + button_width - 1, top + button_height - 1),
                    Some(index)
                );
                assert_eq!(
                    layout.hit_test(left + button_width, top + button_height / 2),
                    None
                );
                assert!(!layout.is_drag_handle(left + button_width / 2, top + button_height / 2));
            }
        }
        for gap in [48, 213, 300, 387, 475] {
            assert_eq!(ToolbarLayout::Horizontal.hit_test(gap, 30), None);
            assert_eq!(ToolbarLayout::Vertical.hit_test(30, gap), None);
        }
    }

    #[test]
    fn toolbar_drag_headers_exclude_buttons_and_window_exterior() {
        let horizontal = ToolbarLayout::Horizontal;
        assert_eq!(horizontal.handle_size(), (45, 60));
        assert!(horizontal.is_drag_handle(0, 0));
        assert!(horizontal.is_drag_handle(44, 59));
        assert!(!horizontal.is_drag_handle(45, 20));
        assert!(!horizontal.is_drag_handle(-1, 20));
        assert!(!horizontal.is_drag_handle(20, 60));
        let vertical = ToolbarLayout::Vertical;
        assert_eq!(vertical.handle_size(), (60, 45));
        assert!(vertical.is_drag_handle(0, 0));
        assert!(vertical.is_drag_handle(59, 44));
        assert!(!vertical.is_drag_handle(60, 44));
        assert!(!vertical.is_drag_handle(30, 45));
        assert!(!vertical.is_drag_handle(30, BUTTON_STARTS[0]));
        assert_eq!(vertical.hit_test(30, BUTTON_STARTS[0]), Some(0));
    }

    #[test]
    fn hsv_round_trips_rgb_and_quick_colors() {
        for color in PALETTE.into_iter().chain([
            Color::rgb(0, 0, 0),
            Color::rgb(255, 255, 255),
            Color::rgb(1, 129, 254),
        ]) {
            let (h, s, v) = rgb_to_hsv(color);
            assert_eq!(hsv_to_rgb(h, s, v), color);
        }
    }

    #[test]
    fn hue_wraps_and_achromatic_values_stay_valid() {
        assert_eq!(hsv_to_rgb(0.0, 1.0, 1.0), Color::rgb(255, 0, 0));
        assert_eq!(hsv_to_rgb(360.0, 1.0, 1.0), Color::rgb(255, 0, 0));
        assert_eq!(hsv_to_rgb(-120.0, 1.0, 1.0), Color::rgb(0, 0, 255));
        assert_eq!(hsv_to_rgb(90.0, 0.0, 0.5), Color::rgb(128, 128, 128));
    }
}
