//! Small native radial palette. Coordinates in hit tests are logical pixels.

use crate::{
    ink::{Color, Tool},
    ui::PALETTE,
};
use std::{
    f32::consts::{FRAC_PI_2, FRAC_PI_4, PI, TAU},
    mem::size_of,
    ptr,
};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Stroke, Transform};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT, SIZE},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow},
};

pub const SIZE_DIP: i32 = 336;
pub const DEAD_ZONE: f32 = 48.0;
pub const ACTION_INNER_RADIUS: f32 = 56.0;
pub const COLOR_INNER_RADIUS: f32 = 60.0;
pub const ACTION_RADIUS: f32 = 142.0;
pub const COLOR_RADIUS: f32 = 148.0;
pub const COLOR_COUNT: usize = 14;
const CENTER: f32 = SIZE_DIP as f32 * 0.5;
const BG: Color = Color::rgb(23, 28, 32);
const BORDER: Color = Color::rgb(59, 69, 76);
const TEXT: Color = Color::rgb(239, 242, 238);
const MUTED: Color = Color::rgb(151, 163, 168);
const ACCENT: Color = Color::rgb(200, 222, 187);
const ACTION_GAP: f32 = 0.025;
const COLOR_GAP: f32 = 0.012;
const ACTION_ANGLES: [f32; 4] = [PI, 0.0, -FRAC_PI_2, FRAC_PI_2];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WheelStage {
    Actions,
    Colors,
}

#[derive(Clone, Copy, Debug)]
pub struct WheelState {
    pub stage: WheelStage,
    pub hover: Option<usize>,
    pub committed: Option<usize>,
    pub primary: Color,
    pub secondary: Color,
    pub interactive: bool,
    pub tool: Tool,
}

/// Action indices are left=0, right=1, top=2, bottom=3. Color indices start
/// at the top and proceed clockwise, centered on each of fourteen sectors.
pub fn hit_test(stage: WheelStage, dx: f32, dy: f32) -> Option<usize> {
    if !dx.is_finite() || !dy.is_finite() {
        return None;
    }
    let distance = dx * dx + dy * dy;
    let (inner, outer) = match stage {
        WheelStage::Actions => (ACTION_INNER_RADIUS.max(DEAD_ZONE), ACTION_RADIUS),
        WheelStage::Colors => (COLOR_INNER_RADIUS.max(DEAD_ZONE), COLOR_RADIUS),
    };
    if !(inner * inner..=outer * outer).contains(&distance) {
        return None;
    }
    let angle = dy.atan2(dx);
    let (index, center, half_width) = match stage {
        WheelStage::Actions => {
            let index = if dy.abs() >= dx.abs() {
                if dy < 0.0 { 2 } else { 3 }
            } else if dx < 0.0 {
                0
            } else {
                1
            };
            (index, ACTION_ANGLES[index], FRAC_PI_4 - ACTION_GAP)
        }
        WheelStage::Colors => {
            let step = TAU / COLOR_COUNT as f32;
            let relative = (angle + FRAC_PI_2).rem_euclid(TAU);
            let index = ((relative / step + 0.5).floor() as usize) % COLOR_COUNT;
            (
                index,
                -FRAC_PI_2 + index as f32 * step,
                step * 0.5 - COLOR_GAP,
            )
        }
    };
    let delta = (angle - center + PI).rem_euclid(TAU) - PI;
    (delta.abs() <= half_width).then_some(index)
}

/// Presets retain stable indices; the saved primary and secondary slots follow.
pub fn colors(primary: Color, secondary: Color) -> [Color; COLOR_COUNT] {
    let mut result = [primary; COLOR_COUNT];
    result[..PALETTE.len()].copy_from_slice(&PALETTE);
    result[12] = primary;
    result[13] = secondary;
    result
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Surface {
    unsafe fn new(width: i32, height: i32) -> Result<Self, String> {
        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        if dc.is_null() {
            return Err(native_error("Could not create the radial device context"));
        }
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
        let mut bits = ptr::null_mut();
        let bitmap =
            unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            let error = native_error("Could not allocate the radial surface");
            unsafe {
                if !bitmap.is_null() {
                    DeleteObject(bitmap);
                }
                DeleteDC(dc);
            }
            return Err(error);
        }
        let previous = unsafe { SelectObject(dc, bitmap) };
        if previous.is_null() || previous as isize == -1 {
            let error = native_error("Could not select the radial surface");
            unsafe {
                DeleteObject(bitmap);
                DeleteDC(dc);
            }
            return Err(error);
        }
        Ok(Self {
            dc,
            bitmap,
            previous,
            bits: bits.cast(),
            width,
            height,
        })
    }

    fn byte_count(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
    unsafe fn clear(&mut self) {
        unsafe {
            ptr::write_bytes(self.bits, 0, self.byte_count());
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

struct Font(HFONT);
impl Font {
    unsafe fn new(size: f32, weight: i32, scale: f32) -> Result<Self, String> {
        let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();
        let font = unsafe {
            CreateFontW(
                -(size * scale).round() as i32,
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
                ANTIALIASED_QUALITY as u32,
                DEFAULT_PITCH as u32,
                face.as_ptr(),
            )
        };
        if font.is_null() {
            Err(native_error("Could not create the radial font"))
        } else {
            Ok(Self(font))
        }
    }
}
impl Drop for Font {
    fn drop(&mut self) {
        unsafe {
            DeleteObject(self.0);
        }
    }
}

/// Owns one small BGRA bitmap and a small grayscale text scratch surface.
/// The host owns the layered HWND and decides when to show or hide it.
pub struct Renderer {
    window: HWND,
    origin: POINT,
    scale: f32,
    surface: Surface,
    text_surface: Surface,
    fonts: [Font; 3],
}

impl Renderer {
    pub unsafe fn new(
        window: HWND,
        center_x: i32,
        center_y: i32,
        scale: f32,
    ) -> Result<Self, String> {
        let size = (SIZE_DIP as f32 * scale).round() as i32;
        if !scale.is_finite() || scale <= 0.0 || !(1..=4096).contains(&size) {
            return Err("The radial menu scale is outside the supported range.".into());
        }
        Ok(Self {
            window,
            origin: POINT {
                x: center_x - size / 2,
                y: center_y - size / 2,
            },
            scale,
            surface: unsafe { Surface::new(size, size)? },
            text_surface: unsafe {
                Surface::new((192.0 * scale).ceil() as i32, (64.0 * scale).ceil() as i32)?
            },
            fonts: unsafe {
                [
                    Font::new(9.0, 400, scale)?,
                    Font::new(11.0, 400, scale)?,
                    Font::new(12.0, 600, scale)?,
                ]
            },
        })
    }

    pub fn render(&mut self, state: &WheelState) -> Result<(), String> {
        {
            let width = self.surface.width as u32;
            let height = self.surface.height as u32;
            let data = unsafe {
                std::slice::from_raw_parts_mut(self.surface.bits, self.surface.byte_count())
            };
            let mut pixels = PixmapMut::from_bytes(data, width, height)
                .ok_or("Could not access the radial surface.")?;
            pixels.fill(tiny_skia::Color::TRANSPARENT);
            let transform = Transform::from_scale(self.scale, self.scale);
            circle(
                &mut pixels,
                CENTER,
                CENTER,
                157.0,
                Color::rgb(0, 0, 0),
                32,
                transform,
            );
            match state.stage {
                WheelStage::Actions => {
                    for (index, angle) in ACTION_ANGLES.into_iter().enumerate() {
                        let selected = state.committed == Some(index) || state.hover == Some(index);
                        sector(
                            &mut pixels,
                            ACTION_INNER_RADIUS,
                            ACTION_RADIUS,
                            angle - FRAC_PI_4 + ACTION_GAP,
                            angle + FRAC_PI_4 - ACTION_GAP,
                            if selected { Color::rgb(50, 64, 53) } else { BG },
                            if selected { ACCENT } else { BORDER },
                            if selected { 1.8 } else { 1.0 },
                            transform,
                        );
                    }
                    action_icons(&mut pixels, state, transform);
                }
                WheelStage::Colors => {
                    let step = TAU / COLOR_COUNT as f32;
                    let current = state.committed.unwrap_or(12);
                    for (index, color) in colors(state.primary, state.secondary)
                        .into_iter()
                        .enumerate()
                    {
                        let angle = -FRAC_PI_2 + index as f32 * step;
                        let active = index == current;
                        let hover = state.hover == Some(index);
                        sector(
                            &mut pixels,
                            COLOR_INNER_RADIUS,
                            COLOR_RADIUS,
                            angle - step * 0.5 + COLOR_GAP,
                            angle + step * 0.5 - COLOR_GAP,
                            color,
                            if active || hover { TEXT } else { BORDER },
                            if active {
                                3.0
                            } else if hover {
                                2.0
                            } else {
                                0.8
                            },
                            transform,
                        );
                    }
                }
            }
            circle(&mut pixels, CENTER, CENTER, 46.0, BG, 250, transform);
        }
        match state.stage {
            WheelStage::Actions => {
                self.label(
                    if state.interactive { "Mouse" } else { "Pen" },
                    18.0,
                    169.0,
                    104.0,
                    24.0,
                    1,
                    TEXT,
                );
                self.label("Clear", 214.0, 169.0, 104.0, 24.0, 1, TEXT);
                self.label("Object eraser", 108.0, 76.0, 120.0, 24.0, 1, TEXT);
                self.label("Colors", 116.0, 266.0, 104.0, 24.0, 1, TEXT);
            }
            WheelStage::Colors => {
                let step = TAU / COLOR_COUNT as f32;
                for (index, label) in [(12, "P"), (13, "S")] {
                    let angle = -FRAC_PI_2 + index as f32 * step;
                    let color = colors(state.primary, state.secondary)[index];
                    let brightness =
                        color.r as f32 * 0.213 + color.g as f32 * 0.715 + color.b as f32 * 0.072;
                    self.label(
                        label,
                        CENTER + angle.cos() * 106.0 - 14.0,
                        CENTER + angle.sin() * 106.0 - 12.0,
                        28.0,
                        24.0,
                        2,
                        if brightness > 145.0 { BG } else { TEXT },
                    );
                }
            }
        }
        self.label(
            if state.committed.is_some() {
                "Selected"
            } else {
                "Select"
            },
            CENTER - 44.0,
            CENTER - 20.0,
            88.0,
            24.0,
            2,
            if state.committed.is_some() {
                ACCENT
            } else {
                TEXT
            },
        );
        self.label(
            "Release to close",
            CENTER - 45.0,
            CENTER + 3.0,
            90.0,
            21.0,
            0,
            MUTED,
        );
        let source = POINT::default();
        let size = SIZE {
            cx: self.surface.width,
            cy: self.surface.height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        if unsafe {
            UpdateLayeredWindow(
                self.window,
                ptr::null_mut(),
                &self.origin,
                &size,
                self.surface.dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            )
        } == 0
        {
            return Err(native_error("Could not present the radial menu"));
        }
        Ok(())
    }

    // GDI text is rendered into a grayscale mask, then composed explicitly.
    // Drawing GDI text directly into a layered bitmap would erase its alpha.
    #[allow(clippy::too_many_arguments)]
    fn label(
        &mut self,
        value: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        font: usize,
        color: Color,
    ) {
        let px = |n: f32| (n * self.scale).round() as i32;
        let width = px(width).min(self.text_surface.width).max(0);
        let height = px(height).min(self.text_surface.height).max(0);
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        let text: Vec<u16> = value.encode_utf16().collect();
        unsafe {
            self.text_surface.clear();
            let previous = SelectObject(self.text_surface.dc, self.fonts[font].0);
            SetBkMode(self.text_surface.dc, TRANSPARENT as i32);
            SetTextColor(self.text_surface.dc, 0x00ff_ffff);
            DrawTextW(
                self.text_surface.dc,
                text.as_ptr(),
                text.len() as i32,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(self.text_surface.dc, previous);
            GdiFlush();
            let mask =
                std::slice::from_raw_parts(self.text_surface.bits, self.text_surface.byte_count());
            let target =
                std::slice::from_raw_parts_mut(self.surface.bits, self.surface.byte_count());
            let origin_x = px(x);
            let origin_y = px(y);
            for row in 0..height {
                let dest_y = origin_y + row;
                if !(0..self.surface.height).contains(&dest_y) {
                    continue;
                }
                for column in 0..width {
                    let dest_x = origin_x + column;
                    if !(0..self.surface.width).contains(&dest_x) {
                        continue;
                    }
                    let offset = ((row * self.text_surface.width + column) * 4) as usize;
                    let coverage = mask[offset].max(mask[offset + 1]).max(mask[offset + 2]) as u32;
                    if coverage == 0 {
                        continue;
                    }
                    let dest = ((dest_y * self.surface.width + dest_x) * 4) as usize;
                    let inverse = 255 - coverage;
                    for (channel, source) in [color.b, color.g, color.r].into_iter().enumerate() {
                        target[dest + channel] = ((source as u32 * coverage
                            + target[dest + channel] as u32 * inverse
                            + 127)
                            / 255) as u8;
                    }
                    target[dest + 3] =
                        (coverage + (target[dest + 3] as u32 * inverse + 127) / 255) as u8;
                }
            }
        }
    }
}

fn paint(color: Color, alpha: u8) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.b, color.g, color.r, alpha);
    paint.anti_alias = true;
    paint
}

fn circle(
    pixels: &mut PixmapMut<'_>,
    x: f32,
    y: f32,
    radius: f32,
    color: Color,
    alpha: u8,
    transform: Transform,
) {
    let mut path = PathBuilder::new();
    path.push_circle(x, y, radius);
    if let Some(path) = path.finish() {
        pixels.fill_path(
            &path,
            &paint(color, alpha),
            FillRule::Winding,
            transform,
            None,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn sector(
    pixels: &mut PixmapMut<'_>,
    inner: f32,
    outer: f32,
    start: f32,
    end: f32,
    color: Color,
    border: Color,
    width: f32,
    transform: Transform,
) {
    let steps = ((end - start).abs() * outer / 4.0).ceil().max(2.0) as usize;
    let mut path = PathBuilder::new();
    path.move_to(CENTER + start.cos() * inner, CENTER + start.sin() * inner);
    for step in 0..=steps {
        let angle = start + (end - start) * step as f32 / steps as f32;
        path.line_to(CENTER + angle.cos() * outer, CENTER + angle.sin() * outer);
    }
    for step in (0..=steps).rev() {
        let angle = start + (end - start) * step as f32 / steps as f32;
        path.line_to(CENTER + angle.cos() * inner, CENTER + angle.sin() * inner);
    }
    path.close();
    if let Some(path) = path.finish() {
        pixels.fill_path(
            &path,
            &paint(color, 255),
            FillRule::Winding,
            transform,
            None,
        );
        pixels.stroke_path(
            &path,
            &paint(border, 255),
            &Stroke {
                width,
                line_join: LineJoin::Round,
                ..Default::default()
            },
            transform,
            None,
        );
    }
}

fn line(
    pixels: &mut PixmapMut<'_>,
    points: &[(f32, f32)],
    color: Color,
    width: f32,
    transform: Transform,
) {
    let Some(first) = points.first() else {
        return;
    };
    let mut path = PathBuilder::new();
    path.move_to(first.0, first.1);
    for &(x, y) in &points[1..] {
        path.line_to(x, y);
    }
    if let Some(path) = path.finish() {
        pixels.stroke_path(
            &path,
            &paint(color, 255),
            &Stroke {
                width,
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Default::default()
            },
            transform,
            None,
        );
    }
}

fn action_icons(pixels: &mut PixmapMut<'_>, state: &WheelState, transform: Transform) {
    if state.interactive {
        line(
            pixels,
            &[
                (62.0, 141.0),
                (63.0, 162.0),
                (68.0, 157.0),
                (74.0, 167.0),
                (78.0, 164.0),
                (72.0, 154.0),
                (80.0, 153.0),
                (62.0, 141.0),
            ],
            TEXT,
            1.8,
            transform,
        );
    } else {
        line(
            pixels,
            &[
                (59.0, 164.0),
                (62.0, 156.0),
                (76.0, 142.0),
                (82.0, 148.0),
                (68.0, 162.0),
                (59.0, 164.0),
            ],
            TEXT,
            1.8,
            transform,
        );
        line(
            pixels,
            &[(72.0, 146.0), (78.0, 152.0)],
            TEXT,
            1.5,
            transform,
        );
    }
    line(
        pixels,
        &[
            (257.0, 146.0),
            (259.0, 166.0),
            (273.0, 166.0),
            (275.0, 146.0),
        ],
        TEXT,
        1.8,
        transform,
    );
    line(
        pixels,
        &[(254.0, 143.0), (278.0, 143.0)],
        TEXT,
        1.8,
        transform,
    );
    line(
        pixels,
        &[
            (262.0, 143.0),
            (262.0, 138.0),
            (270.0, 138.0),
            (270.0, 143.0),
        ],
        TEXT,
        1.8,
        transform,
    );
    for x in [263.0, 269.0] {
        line(pixels, &[(x, 150.0), (x, 162.0)], TEXT, 1.4, transform);
    }
    let object_color = if state.interactive && state.tool == Tool::ObjectEraser {
        ACCENT
    } else {
        TEXT
    };
    line(
        pixels,
        &[
            (155.0, 64.0),
            (169.0, 49.0),
            (181.0, 60.0),
            (168.0, 73.0),
            (163.0, 73.0),
            (155.0, 64.0),
        ],
        object_color,
        1.8,
        transform,
    );
    line(
        pixels,
        &[(162.0, 57.0), (174.0, 68.0)],
        object_color,
        1.5,
        transform,
    );
    for (x, y) in [(153.0, 49.0), (183.0, 48.0), (184.0, 74.0), (152.0, 76.0)] {
        circle(pixels, x, y, 1.5, object_color, 255, transform);
    }
    circle(pixels, 175.0, 253.0, 10.0, state.secondary, 255, transform);
    circle(pixels, 161.0, 246.0, 10.0, TEXT, 255, transform);
    circle(pixels, 161.0, 246.0, 8.5, state.primary, 255, transform);
}

fn native_error(message: &str) -> String {
    format!("{message}: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use windows_sys::Win32::{System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};

    struct HiddenWindow {
        window: HWND,
        class_name: Vec<u16>,
    }

    impl HiddenWindow {
        fn new() -> Self {
            static NEXT_CLASS: AtomicUsize = AtomicUsize::new(0);
            let id = NEXT_CLASS.fetch_add(1, Ordering::Relaxed);
            let class_name: Vec<u16> = format!("StudyPen.Radial.Test.{id}")
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
                    "Could not register the radial test window"
                );
                let window = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST,
                    class_name.as_ptr(),
                    ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    SIZE_DIP,
                    SIZE_DIP,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    instance,
                    ptr::null(),
                );
                if window.is_null() {
                    UnregisterClassW(class_name.as_ptr(), instance);
                    panic!("Could not create the radial test window");
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

    fn state() -> WheelState {
        WheelState {
            stage: WheelStage::Actions,
            hover: Some(2),
            committed: None,
            primary: Color::rgb(99, 102, 241),
            secondary: Color::rgb(244, 63, 94),
            interactive: false,
            tool: Tool::Pen,
        }
    }

    fn bytes(renderer: &Renderer) -> &[u8] {
        unsafe { std::slice::from_raw_parts(renderer.surface.bits, renderer.surface.byte_count()) }
    }

    fn pixel(renderer: &Renderer, x: usize, y: usize) -> [u8; 4] {
        let offset = (y * renderer.surface.width as usize + x) * 4;
        bytes(renderer)[offset..offset + 4].try_into().unwrap()
    }

    #[test]
    fn cardinal_actions_and_dead_zone_have_predictable_geometry() {
        assert_eq!(hit_test(WheelStage::Actions, -100.0, 0.0), Some(0));
        assert_eq!(hit_test(WheelStage::Actions, 100.0, 0.0), Some(1));
        assert_eq!(hit_test(WheelStage::Actions, 0.0, -100.0), Some(2));
        assert_eq!(hit_test(WheelStage::Actions, 0.0, 100.0), Some(3));
        assert_eq!(hit_test(WheelStage::Actions, 0.0, 0.0), None);
        assert_eq!(hit_test(WheelStage::Actions, DEAD_ZONE - 0.1, 0.0), None);
        assert_eq!(hit_test(WheelStage::Actions, DEAD_ZONE, 0.0), None);
        assert_eq!(
            hit_test(WheelStage::Actions, ACTION_INNER_RADIUS, 0.0),
            Some(1)
        );
        assert_eq!(hit_test(WheelStage::Actions, ACTION_RADIUS, 0.0), Some(1));
        assert_eq!(
            hit_test(WheelStage::Actions, ACTION_RADIUS + 0.1, 0.0),
            None
        );
        assert_eq!(
            hit_test(WheelStage::Colors, 0.0, -COLOR_INNER_RADIUS),
            Some(0)
        );
        assert_eq!(
            hit_test(WheelStage::Colors, 0.0, -COLOR_INNER_RADIUS + 0.1),
            None
        );
        assert_eq!(hit_test(WheelStage::Colors, 0.0, -COLOR_RADIUS), Some(0));
        assert_eq!(hit_test(WheelStage::Actions, 80.0, -80.0), None);
        let color_gap = -FRAC_PI_2 + TAU / COLOR_COUNT as f32 * 0.5;
        assert_eq!(
            hit_test(
                WheelStage::Colors,
                color_gap.cos() * 100.0,
                color_gap.sin() * 100.0
            ),
            None
        );
        assert_eq!(hit_test(WheelStage::Actions, COLOR_RADIUS + 0.1, 0.0), None);
        assert_eq!(hit_test(WheelStage::Actions, f32::NAN, 100.0), None);
        assert_eq!(hit_test(WheelStage::Actions, f32::INFINITY, 0.0), None);
    }

    #[test]
    fn color_sectors_wrap_clockwise_and_keep_saved_slots_stable() {
        for index in 0..COLOR_COUNT {
            let angle = -FRAC_PI_2 + index as f32 * TAU / COLOR_COUNT as f32;
            assert_eq!(
                hit_test(WheelStage::Colors, angle.cos() * 100.0, angle.sin() * 100.0),
                Some(index)
            );
        }
        let primary = Color::rgb(10, 20, 30);
        let secondary = Color::rgb(120, 130, 140);
        let choices = colors(primary, secondary);
        assert_eq!(&choices[..12], &PALETTE);
        assert_eq!(choices[12], primary);
        assert_eq!(choices[13], secondary);
    }

    #[test]
    fn hidden_native_renderer_preserves_transparency_and_text_alpha() {
        let window = HiddenWindow::new();
        let mut renderer = unsafe { Renderer::new(window.window, 168, 168, 1.0) }.unwrap();
        let mut state = state();
        renderer.render(&state).unwrap();
        assert_eq!(pixel(&renderer, 0, 0), [0, 0, 0, 0]);
        assert_eq!(pixel(&renderer, 70, 168)[3], 255);
        // Every center pixel remains opaque enough after native text masking;
        // GDI text must never punch transparent holes in the layered bitmap.
        for y in 140..196 {
            for x in 140..196 {
                assert!(pixel(&renderer, x, y)[3] >= 245);
            }
        }
        assert!(
            (145..188).any(|y| (140..196).any(|x| {
                let pixel = pixel(&renderer, x, y);
                pixel[0] > 180 && pixel[1] > 180 && pixel[2] > 180
            })),
            "The central text must produce visible light glyphs"
        );
        state.stage = WheelStage::Colors;
        state.hover = Some(3);
        state.committed = Some(3);
        renderer.render(&state).unwrap();
        for pixel in bytes(&renderer).as_chunks::<4>().0 {
            assert!(
                pixel[0] <= pixel[3] && pixel[1] <= pixel[3] && pixel[2] <= pixel[3],
                "All BGRA colors must remain premultiplied"
            );
        }
        assert_eq!(pixel(&renderer, 0, 0)[3], 0);
        assert_eq!(unsafe { IsWindowVisible(window.window) }, 0);
    }

    #[test]
    #[ignore = "Shows Actions25s then Colors25s for visual inspection; run explicitly"]
    fn preview_radial_on_this_desktop() {
        let window = HiddenWindow::new();
        let title: Vec<u16> = "Study Pen Radial Preview"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        unsafe {
            // Give the opt-in preview a discoverable capture surface. The
            // application's real wheel remains a no-activate tool window.
            SetWindowTextW(window.window, title.as_ptr());
            let style = GetWindowLongPtrW(window.window, GWL_EXSTYLE) as u32;
            SetWindowLongPtrW(
                window.window,
                GWL_EXSTYLE,
                ((style & !WS_EX_TOOLWINDOW) | WS_EX_APPWINDOW) as isize,
            );
        }
        let center_x = unsafe { GetSystemMetrics(SM_CXSCREEN) } / 2;
        let center_y = unsafe { GetSystemMetrics(SM_CYSCREEN) } / 2;
        let mut renderer =
            unsafe { Renderer::new(window.window, center_x, center_y, 1.0) }.unwrap();
        let mut state = state();
        renderer.render(&state).unwrap();
        unsafe {
            ShowWindow(window.window, SW_SHOWNOACTIVATE);
        }
        let start = std::time::Instant::now();
        let mut colors_shown = false;
        while start.elapsed().as_secs_f32() < 50.0 {
            if !colors_shown && start.elapsed().as_secs_f32() >= 25.0 {
                state.stage = WheelStage::Colors;
                state.hover = Some(3);
                state.committed = Some(3);
                renderer.render(&state).unwrap();
                colors_shown = true;
            }
            let mut message = MSG::default();
            unsafe {
                while PeekMessageW(&mut message, window.window, 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}
