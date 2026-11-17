//! Native settings views and antialiased toolbar icons in logical pixels.

use crate::{
    ink::{Color, Tool},
    panel_material::PanelMaterial,
    panel_primitives, toolbar_editor,
    toolbar_labels::{self, Label},
    toolbar_preferences::{ToolbarButton, ToolbarTheme},
};
use std::{cell::RefCell, mem::size_of};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};
use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
};

pub const PANEL_WIDTH: i32 = 460;
pub const PANEL_HEIGHT: i32 = 620;
pub const TAB_LEFT: [i32; 4] = [24, 129, 234, 339];
pub const TAB_WIDTH: i32 = 97;
pub const SHORTCUT_TOP: i32 = 124;
pub const SHORTCUT_STEP: i32 = 44;
pub const SHORTCUT_HEIGHT: i32 = 38;
pub const UPDATE_BUTTON_TOP: i32 = 234;
pub const UPDATE_BUTTON_BOTTOM: i32 = 270;
pub const UPDATE_CHECK_RIGHT: i32 = 222;
pub const UPDATE_ACTION_LEFT: i32 = 236;
const BUTTON_SIZE: i32 = 38;
const HANDLE_LENGTH: i32 = 45;
const BAR_THICKNESS: i32 = 60;

/// Layout geometry shared by native hit testing, docking, and painting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolbarLayout {
    #[default]
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ToolbarGeometry {
    pub layout: ToolbarLayout,
    pub labels: bool,
    buttons: [Option<ToolbarButton>; ToolbarButton::ALL.len()],
    count: usize,
}

impl ToolbarGeometry {
    pub fn new(layout: ToolbarLayout, buttons: &[ToolbarButton], labels: bool) -> Self {
        let count = buttons.len().min(ToolbarButton::ALL.len());
        let mut slots = [None; ToolbarButton::ALL.len()];
        for (slot, &button) in slots.iter_mut().zip(buttons) {
            *slot = Some(button);
        }
        Self {
            layout,
            labels,
            buttons: slots,
            count,
        }
    }

    #[cfg(test)]
    pub fn default_for(layout: ToolbarLayout) -> Self {
        Self::new(layout, &ToolbarButton::DEFAULT, false)
    }

    pub fn with_layout(self, layout: ToolbarLayout) -> Self {
        Self { layout, ..self }
    }

    pub fn button(self, index: usize) -> Option<ToolbarButton> {
        self.buttons.get(index).copied().flatten()
    }

    pub fn len(self) -> usize {
        self.count
    }

    fn track_start(self, index: usize) -> i32 {
        let group_gaps = 7 * [4, 6, 8].into_iter().filter(|&gap| index >= gap).count() as i32;
        51 + index as i32 * 40 + group_gaps
    }

    pub fn size(self) -> (i32, i32) {
        let length = self.track_start(self.count.saturating_sub(1)) + BUTTON_SIZE + 10;
        match self.layout {
            ToolbarLayout::Horizontal => (length, BAR_THICKNESS),
            ToolbarLayout::Vertical => (BAR_THICKNESS, length),
        }
    }

    pub fn handle_size(self) -> (i32, i32) {
        let (width, height) = self.size();
        match self.layout {
            ToolbarLayout::Horizontal => (HANDLE_LENGTH, height),
            ToolbarLayout::Vertical => (width, HANDLE_LENGTH),
        }
    }

    pub(crate) fn button_bounds(self, index: usize) -> Option<(i32, i32, i32, i32)> {
        self.button(index)?;
        let start = self.track_start(index);
        Some(match (self.layout, self.labels) {
            (ToolbarLayout::Horizontal, false) => (start, 11, BUTTON_SIZE, BUTTON_SIZE),
            (ToolbarLayout::Vertical, false) => (11, start, BUTTON_SIZE, BUTTON_SIZE),
            (ToolbarLayout::Horizontal, true) => (start, 8, BUTTON_SIZE, 44),
            (ToolbarLayout::Vertical, true) => (11, start, BUTTON_SIZE, BUTTON_SIZE),
        })
    }

    pub fn hit_test(self, x: i32, y: i32) -> Option<usize> {
        (0..self.count).find(|index| {
            let (left, top, width, height) = self.button_bounds(*index).unwrap();
            (left..left + width).contains(&x) && (top..top + height).contains(&y)
        })
    }

    pub fn is_drag_handle(self, x: i32, y: i32) -> bool {
        let (width, height) = self.handle_size();
        (0..width).contains(&x) && (0..height).contains(&y)
    }
}

fn toolbar_caption(button: ToolbarButton) -> &'static str {
    match button {
        ToolbarButton::ObjectEraser => "Object",
        ToolbarButton::Clear => "Clear",
        ToolbarButton::Hide => "Hide",
        ToolbarButton::Quit => "Exit",
        ToolbarButton::Fill => "Fill",
        _ => button.label(),
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

pub(crate) const BG: Color = Color::rgb(23, 28, 32);
pub(crate) const SURFACE: Color = Color::rgb(32, 39, 44);
pub(crate) const BORDER: Color = Color::rgb(55, 65, 72);
pub(crate) const TEXT: Color = Color::rgb(239, 242, 238);
pub(crate) const MUTED: Color = Color::rgb(163, 173, 182);
pub(crate) const ACCENT: Color = Color::rgb(198, 220, 248);
pub(crate) const SELECTED: Color = Color::rgb(43, 62, 87);

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
    pub checking_update: bool,
    pub can_check_updates: bool,
    pub update_message: String,
    pub update_action: Option<&'static str>,
    pub storage_message: String,
    pub status: String,
    pub pen_width: f32,
    pub eraser_diameter: f32,
    pub toolbar_theme: ToolbarTheme,
    pub toolbar_labels: bool,
    pub toolbar_buttons: Vec<ToolbarButton>,
    pub toolbar_edit_drag: Option<toolbar_editor::Drag>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ToolbarControls {
    pub geometry: ToolbarGeometry,
    pub theme: ToolbarTheme,
    pub interactive: bool,
    pub tool: Tool,
    pub primary: Color,
    pub ruler_visible: bool,
    pub hover: Option<usize>,
    pub dark: bool,
}

impl ToolbarControls {
    pub fn button(&self, index: usize) -> Option<ToolbarButton> {
        self.geometry.button(index)
    }

    pub fn selected(&self) -> Option<usize> {
        let selected = if self.interactive {
            match self.tool {
                Tool::Pen => ToolbarButton::Pen,
                Tool::Eraser => ToolbarButton::Eraser,
                Tool::ObjectEraser => ToolbarButton::ObjectEraser,
                Tool::Shape => ToolbarButton::Shapes,
                Tool::Fill => ToolbarButton::Fill,
            }
        } else {
            ToolbarButton::Cursor
        };
        (0..self.geometry.len()).find(|&index| self.button(index) == Some(selected))
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

pub(crate) struct Painter<'a> {
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

    pub(crate) fn fill(&self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        let brush = OwnedObject(unsafe { CreateSolidBrush(cref(color)) });
        unsafe {
            FillRect(self.dc, &self.rect(x, y, w, h), brush.0);
        }
    }

    // Keeping rectangle coordinates and paint attributes together makes these
    // immediate-mode view definitions easier to compare with their hit areas.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rounded(
        &self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        fill: Color,
        border: Color,
    ) {
        if unsafe {
            panel_primitives::rounded(
                self.dc,
                [x, y, w, h].map(|value| value * self.scale),
                radius * self.scale,
                fill,
                border,
                self.scale,
            )
        } {
            return;
        }
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
        if unsafe {
            panel_primitives::circle(
                self.dc,
                x * self.scale,
                y * self.scale,
                radius * self.scale,
                fill,
                border,
                width * self.scale,
            )
        } {
            return;
        }
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

    pub(crate) fn line<const N: usize>(&self, points: &[(f32, f32); N], color: Color, width: f32) {
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

    pub(crate) fn label(&self, value: &str, x: f32, y: f32, w: f32, font: usize, color: Color) {
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
    pub(crate) fn center(
        &self,
        value: &str,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        font: usize,
        color: Color,
    ) {
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

    fn bitmap(&self, bounds: RECT, data: &[u8]) {
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
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
                bounds.left,
                bounds.top,
                width,
                height,
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
    let ink = if state.dark {
        Color::rgb(250, 251, 254)
    } else {
        Color::rgb(24, 27, 34)
    };
    let geometry = state.geometry;
    let (handle_width, handle_height) = geometry.handle_size();
    let (grip_x, grip_y) = match geometry.layout {
        ToolbarLayout::Horizontal => (26.0, handle_height as f32 / 2.0),
        ToolbarLayout::Vertical => (handle_width as f32 / 2.0, 26.0),
    };
    let grip_paint = icon_paint(ink);
    for dx in [-3.0, 3.0] {
        for dy in [-5.0, 0.0, 5.0] {
            let x = ((grip_x + dx) * scale).floor() + 0.5;
            let y = ((grip_y + dy) * scale).floor() + 0.5;
            let circle = PathBuilder::from_circle(x, y, 1.25 * scale)?;
            pixmap.fill_path(
                &circle,
                &grip_paint,
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
    let mut labels = Vec::new();
    for index in 0..geometry.len() {
        let button = geometry.button(index)?;
        let (left, top, width, height) = geometry.button_bounds(index)?;
        let x = left as f32 + width as f32 / 2.0;
        let y = if geometry.labels {
            let (icon_y, label_y) = match geometry.layout {
                ToolbarLayout::Horizontal => (13.5, 27),
                ToolbarLayout::Vertical => (11.0, 22),
            };
            labels.push(Label {
                text: toolbar_caption(button),
                bounds: [left, top + label_y, width, 16],
            });
            top as f32 + icon_y
        } else {
            top as f32 + height as f32 / 2.0
        };
        let transform = Transform::from_row(
            19.0 * scale / 24.0,
            0.0,
            0.0,
            19.0 * scale / 24.0,
            (x - 9.5) * scale,
            (y - 9.5) * scale,
        );
        let path = toolbar_icon(button)?;
        let stroke = Stroke {
            width: if button == ToolbarButton::Quit {
                2.0
            } else {
                1.65
            },
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &icon_paint(ink), &stroke, transform, None);
        if button == ToolbarButton::Color {
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
    let mut pixels: Vec<u32> = pixmap
        .pixels()
        .iter()
        .map(|pixel| {
            u32::from(pixel.blue())
                | (u32::from(pixel.green()) << 8)
                | (u32::from(pixel.red()) << 16)
                | (u32::from(pixel.alpha()) << 24)
        })
        .collect();
    if geometry.labels {
        toolbar_labels::paint(&mut pixels, width, height, scale, ink, &labels);
    }
    Some(pixels)
}

fn icon_paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.r, color.g, color.b, 255);
    paint.anti_alias = true;
    paint
}

/// Toolbar icons share a 24-unit view box.
fn toolbar_icon(button: ToolbarButton) -> Option<Path> {
    use ToolbarButton::*;
    let mut path = PathBuilder::new();
    match button {
        Cursor => {
            path.move_to(5.0, 3.0);
            path.line_to(18.7, 12.7);
            path.line_to(11.9, 14.1);
            path.line_to(8.0, 19.9);
            path.close();
        }
        Pen => {
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
        Eraser => {
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
        ObjectEraser => {
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
        Undo => {
            path.move_to(9.0, 5.0);
            path.line_to(4.0, 10.0);
            path.line_to(9.0, 15.0);
            path.move_to(4.0, 10.0);
            path.line_to(14.7, 10.0);
            path.cubic_to(17.35, 10.0, 19.5, 12.15, 19.5, 14.8);
            path.cubic_to(19.5, 17.45, 17.35, 19.6, 14.7, 19.6);
            path.line_to(13.0, 19.6);
        }
        Clear => {
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
        Color => path.push_circle(12.0, 12.0, 8.3),
        Settings => {
            for (y, x) in [(6.0, 10.0), (12.0, 14.0), (18.0, 9.0)] {
                path.move_to(5.0, y);
                path.line_to(19.0, y);
                path.move_to(x, y - 2.0);
                path.line_to(x, y + 2.0);
            }
        }
        Hide => {
            path.move_to(9.2, 5.8);
            path.cubic_to(13.6, 4.1, 18.0, 6.5, 21.0, 12.0);
            path.quad_to(20.1, 13.6, 18.8, 14.9);
            path.move_to(5.5, 8.9);
            path.quad_to(4.0, 10.2, 3.0, 12.0);
            path.cubic_to(6.2, 17.8, 11.0, 19.9, 15.5, 17.5);
            path.move_to(12.0, 9.2);
            path.quad_to(14.8, 9.2, 14.8, 12.0);
            path.move_to(9.2, 12.0);
            path.quad_to(9.2, 14.8, 12.0, 14.8);
            path.move_to(3.7, 3.7);
            path.line_to(20.3, 20.3);
        }
        Quit => {
            path.move_to(5.8, 5.8);
            path.line_to(18.2, 18.2);
            path.move_to(18.2, 5.8);
            path.line_to(5.8, 18.2);
        }
        Shapes => {
            path.move_to(5.0, 3.5);
            path.line_to(14.0, 3.5);
            path.line_to(14.0, 12.5);
            path.line_to(5.0, 12.5);
            path.close();
            path.push_circle(15.5, 15.5, 5.0);
        }
        Ruler => {
            path.move_to(3.0, 7.4);
            path.line_to(21.0, 7.4);
            path.line_to(21.0, 16.6);
            path.line_to(3.0, 16.6);
            path.close();
            for x in [7.0, 10.3, 13.7, 17.0] {
                path.move_to(x, 7.4);
                path.line_to(x, if x == 10.3 || x == 13.7 { 10.4 } else { 12.0 });
            }
        }
        Fill => {
            path.move_to(7.0, 4.2);
            path.line_to(16.2, 13.4);
            path.line_to(10.5, 19.1);
            path.quad_to(9.5, 20.0, 8.5, 19.1);
            path.line_to(3.9, 14.5);
            path.quad_to(2.9, 13.5, 3.9, 12.5);
            path.line_to(11.1, 5.3);
            path.line_to(16.2, 10.4);
            path.line_to(16.2, 13.4);
            path.move_to(4.6, 13.2);
            path.line_to(15.6, 13.2);
            path.move_to(19.0, 14.5);
            path.quad_to(23.1, 19.5, 19.0, 20.3);
            path.quad_to(14.9, 19.5, 19.0, 14.5);
        }
    }
    path.finish()
}

/// Paints the fixed 460 by 620 DIP companion panel.
pub unsafe fn paint_panel(dc: HDC, scale: f32, state: &UiState, material: Option<&PanelMaterial>) {
    let fonts = unsafe { FontSet::new(FontSet::heights(scale)) };
    let p = unsafe { Painter::new(dc, scale.max(0.25), Some(&fonts)) };
    if !material.is_some_and(|material| {
        material.paint(dc, p.px(PANEL_WIDTH as f32), p.px(PANEL_HEIGHT as f32))
    }) {
        p.fill(0.0, 0.0, PANEL_WIDTH as f32, PANEL_HEIGHT as f32, BG);
    }
    p.line(&[(20.0, 1.0), (440.0, 1.0)], BORDER, 1.0);
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
    p.line(&[(424.0, 24.0), (436.0, 36.0)], MUTED, 1.5);
    p.line(&[(436.0, 24.0), (424.0, 36.0)], MUTED, 1.5);
    let tabs = ["Colors", "Shortcuts", "General", "Toolbar"];
    for (index, label) in tabs.into_iter().enumerate() {
        let x = TAB_LEFT[index] as f32;
        let active = state.tab == index;
        if active {
            p.rounded(x, 76.0, TAB_WIDTH as f32, 32.0, 8.0, SELECTED, BORDER);
        }
        p.center(
            label,
            x,
            76.0,
            TAB_WIDTH as f32,
            32.0,
            if active { 5 } else { 0 },
            if active { ACCENT } else { MUTED },
        );
    }
    match state.tab {
        1 => shortcuts(&p, state),
        2 => general(&p, state),
        3 => toolbar_editor::paint(
            &p,
            state.toolbar_theme,
            state.toolbar_labels,
            &state.toolbar_buttons,
            state.toolbar_edit_drag,
        ),
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

#[derive(Default)]
struct ColorGradients {
    plane_key: Option<(i32, i32, Color)>,
    plane: Vec<u8>,
    strip_size: (i32, i32),
    strip: Vec<u8>,
}

thread_local! {
    static COLOR_GRADIENTS: RefCell<ColorGradients> = RefCell::default();
}

pub fn clear_color_gradients() {
    COLOR_GRADIENTS.with(|cache| *cache.borrow_mut() = ColorGradients::default());
}

impl ColorGradients {
    fn paint(&mut self, p: &Painter<'_>, hue: Color) {
        let plane = p.rect(24.0, 202.0, 412.0, 170.0);
        let width = plane.right - plane.left;
        let height = plane.bottom - plane.top;
        let key = (width, height, hue);
        if self.plane_key != Some(key) {
            self.plane.resize(width as usize * height as usize * 4, 0);
            let colored = [hue.b, hue.g, hue.r].map(f32::from);
            for (y, row) in self.plane.chunks_exact_mut(width as usize * 4).enumerate() {
                let v = 1.0 - y as f32 / (height - 1).max(1) as f32;
                for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let s = x as f32 / (width - 1).max(1) as f32;
                    for (channel, component) in colored.into_iter().enumerate() {
                        pixel[channel] = ((255.0 + (component - 255.0) * s) * v).round() as u8;
                    }
                }
            }
            self.plane_key = Some(key);
        }
        p.bitmap(plane, &self.plane);

        let strip = p.rect(24.0, 388.0, 412.0, 16.0);
        let width = strip.right - strip.left;
        let height = strip.bottom - strip.top;
        if self.strip_size != (width, height) {
            let stride = width as usize * 4;
            self.strip.resize(stride * height as usize, 0);
            let (first, remaining) = self.strip.split_at_mut(stride);
            for (x, pixel) in first.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let color = hsv_to_rgb(x as f32 * 360.0 / (width - 1).max(1) as f32, 1.0, 1.0);
                pixel.copy_from_slice(&[color.b, color.g, color.r, 0]);
            }
            for row in remaining.chunks_exact_mut(stride) {
                row.copy_from_slice(first);
            }
            self.strip_size = (width, height);
        }
        p.bitmap(strip, &self.strip);
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
    let hue = hsv_to_rgb(state.hue, 1.0, 1.0);
    COLOR_GRADIENTS.with(|cache| cache.borrow_mut().paint(p, hue));
    let marker_x = 24.0 + state.saturation.clamp(0.0, 1.0) * 412.0;
    let marker_y = 202.0 + (1.0 - state.value.clamp(0.0, 1.0)) * 170.0;
    p.circle(marker_x, marker_y, 7.0, None, Color::rgb(20, 20, 20), 3.0);
    p.circle(marker_x, marker_y, 6.0, None, TEXT, 1.7);
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
        36.0,
        1,
        MUTED,
        DT_WORDBREAK,
    );
    let top = UPDATE_BUTTON_TOP as f32;
    let height = (UPDATE_BUTTON_BOTTOM - UPDATE_BUTTON_TOP) as f32;
    let check_width = (UPDATE_CHECK_RIGHT - 24) as f32;
    p.rounded(24.0, top, check_width, height, 8.0, SURFACE, BORDER);
    p.center(
        if state.checking_update {
            "Checking…"
        } else {
            "Check for updates"
        },
        24.0,
        top,
        check_width,
        height,
        0,
        if state.can_check_updates { TEXT } else { MUTED },
    );
    if let Some(action) = state.update_action {
        let left = UPDATE_ACTION_LEFT as f32;
        let width = 436.0 - left;
        p.rounded(left, top, width, height, 8.0, SURFACE, BORDER);
        p.center(action, left, top, width, height, 0, TEXT);
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
            geometry: ToolbarGeometry::default_for(layout),
            theme: ToolbarTheme::LiquidGlass,
            interactive: true,
            tool: Tool::Eraser,
            primary: Color::rgb(99, 102, 241),
            ruler_visible: false,
            hover: Some(7),
            dark,
        }
    }

    #[test]
    fn toolbar_raster_preserves_antialiased_icons_at_common_dpi_and_fitted_scales() {
        let scales = [96, 120, 144, 192].map(|dpi| dpi as f32 / 96.0);
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            let geometry = ToolbarGeometry::default_for(layout);
            for scale in scales.into_iter().chain([0.4]) {
                let (width, height) = geometry.size();
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
                    for index in 0..geometry.len() {
                        let (x, y, w, h) = geometry.button_bounds(index).unwrap();
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
    fn drag_grip_has_six_opaque_centers_at_native_dpi() {
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            let (width, height) = ToolbarGeometry::default_for(layout).size();
            let (x, y) = match layout {
                ToolbarLayout::Horizontal => (26, 30),
                ToolbarLayout::Vertical => (30, 26),
            };
            for dark in [false, true] {
                let state = toolbar_state(layout, dark);
                let pixels = toolbar_pixels(width as u32, height as u32, 1.0, &state).unwrap();
                let sample = |x, y| pixels[(y * width + x) as usize] >> 24;
                for dx in [-3, 3] {
                    for dy in [-5, 0, 5] {
                        assert_eq!(sample(x + dx, y + dy), 255);
                    }
                }
                assert_eq!(sample(x, y), 0);
            }
        }
    }

    #[test]
    fn near_black_primary_swatch_keeps_its_opaque_color_and_alpha() {
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            let geometry = ToolbarGeometry::default_for(layout);
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let (width, height) = geometry.size();
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
                let (x, y, width, height) = geometry.button_bounds(6).unwrap();
                let center = physical(y + height / 2) * physical(geometry.size().0)
                    + physical(x + width / 2);
                assert_eq!(pixels[center], 0xff01_0203);
            }
        }
    }

    #[test]
    fn both_toolbar_layouts_keep_every_button_inside_its_hit_bounds() {
        let horizontal = ToolbarGeometry::default_for(ToolbarLayout::Horizontal);
        let vertical = ToolbarGeometry::default_for(ToolbarLayout::Vertical);
        assert_eq!(horizontal.size(), (480, 60));
        assert_eq!(vertical.size(), (60, 480));
        for layout in [horizontal, vertical] {
            let (width, height) = layout.size();
            for index in 0..layout.len() {
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
            assert_eq!(horizontal.hit_test(gap, 30), None);
            assert_eq!(vertical.hit_test(30, gap), None);
        }
    }

    #[test]
    fn toolbar_drag_headers_exclude_buttons_and_window_exterior() {
        let horizontal = ToolbarGeometry::default_for(ToolbarLayout::Horizontal);
        assert_eq!(horizontal.handle_size(), (45, 60));
        assert!(horizontal.is_drag_handle(0, 0));
        assert!(horizontal.is_drag_handle(44, 59));
        assert!(!horizontal.is_drag_handle(45, 20));
        assert!(!horizontal.is_drag_handle(-1, 20));
        assert!(!horizontal.is_drag_handle(20, 60));
        let vertical = ToolbarGeometry::default_for(ToolbarLayout::Vertical);
        assert_eq!(vertical.handle_size(), (60, 45));
        assert!(vertical.is_drag_handle(0, 0));
        assert!(vertical.is_drag_handle(59, 44));
        assert!(!vertical.is_drag_handle(60, 44));
        assert!(!vertical.is_drag_handle(30, 45));
        assert!(!vertical.is_drag_handle(30, 51));
        assert_eq!(vertical.hit_test(30, 51), Some(0));
    }

    #[test]
    fn short_names_keep_the_same_outer_toolbar_size() {
        for layout in [ToolbarLayout::Horizontal, ToolbarLayout::Vertical] {
            for buttons in [&ToolbarButton::DEFAULT[..], &ToolbarButton::ALL[..]] {
                let plain = ToolbarGeometry::new(layout, buttons, false);
                let named = ToolbarGeometry::new(layout, buttons, true);
                assert_eq!(named.size(), plain.size());
                assert_eq!(named.handle_size(), plain.handle_size());
                for index in 0..named.len() {
                    let (x, y, w, h) = named.button_bounds(index).unwrap();
                    assert_eq!(named.hit_test(x + w / 2, y + h - 1), Some(index));
                }
            }
        }
        assert_eq!(toolbar_caption(ToolbarButton::ObjectEraser), "Object");
        assert_eq!(toolbar_caption(ToolbarButton::Quit), "Exit");
        assert_eq!(ToolbarButton::Quit.label(), "Quit Study Pen");
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
