//! Renders the floating toolbar against the desktop pixels directly beneath it.

use crate::{
    ink::Color,
    ui::{self, ToolbarControls, ToolbarLayout},
};
use std::{mem::size_of, ptr, slice};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, SIZE},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow},
};

const DARK_TINT_ALPHA: f32 = 0.56;
const LIGHT_TINT_ALPHA: f32 = 0.54;

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u32,
    len: usize,
}

impl Surface {
    unsafe fn new(width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let len = (width as usize).checked_mul(height as usize)?;
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return None;
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
        let mut pixels = ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, ptr::null_mut(), 0);
        if bitmap.is_null() || pixels.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            DeleteDC(dc);
            return None;
        }
        let previous = SelectObject(dc, bitmap);
        if previous.is_null() || previous as isize == -1 {
            DeleteObject(bitmap);
            DeleteDC(dc);
            return None;
        }
        Some(Self {
            dc,
            bitmap,
            previous,
            pixels: pixels.cast(),
            len,
        })
    }

    unsafe fn pixels(&mut self) -> &mut [u32] {
        slice::from_raw_parts_mut(self.pixels, self.len)
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

#[derive(Clone, Copy)]
struct Frame {
    bounds: RECT,
    scale: f32,
    controls: ToolbarControls,
    reflection: Option<POINT>,
}

impl PartialEq for Frame {
    fn eq(&self, other: &Self) -> bool {
        self.bounds.left == other.bounds.left
            && self.bounds.top == other.bounds.top
            && self.bounds.right == other.bounds.right
            && self.bounds.bottom == other.bounds.bottom
            && self.scale == other.scale
            && self.controls == other.controls
            && self.reflection.map(|p| (p.x, p.y)) == other.reflection.map(|p| (p.x, p.y))
    }
}

struct Optic {
    source_x: f32,
    source_y: f32,
    rim: f32,
    shade: f32,
    lens: f32,
    alpha: u8,
}

#[derive(Clone, Copy, PartialEq)]
struct GlyphKey {
    layout: ToolbarLayout,
    scale: f32,
    primary: Color,
    dark: bool,
}

impl GlyphKey {
    fn new(scale: f32, controls: &ToolbarControls) -> Self {
        Self {
            layout: controls.layout,
            scale,
            primary: controls.primary,
            dark: controls.dark,
        }
    }
}

struct Buffers {
    width: i32,
    height: i32,
    padding: i32,
    capture: Surface,
    glyphs: Vec<u32>,
    glyph_key: Option<GlyphKey>,
    output: Surface,
    optics: Vec<Optic>,
    last: Option<(Frame, u64)>,
    monitors: Vec<RECT>,
    copied_rects: Vec<RECT>,
}

impl Buffers {
    unsafe fn new(width: i32, height: i32, scale: f32) -> Option<Self> {
        let padding = (23.0 * scale).ceil().max(4.0) as i32;
        let capture_width = width + 2 * padding;
        let capture_height = height + 2 * padding;
        let capture = Surface::new(capture_width, capture_height)?;
        let output = Surface::new(width, height)?;
        let radius = width.min(height) as f32 * 0.5;
        let half_width = width as f32 * 0.5;
        let half_height = height as f32 * 0.5;
        let bevel = (14.0 * scale).min(radius * 0.49);
        let mut optics = Vec::with_capacity(output.len);
        for y in 0..height {
            for x in 0..width {
                let px = x as f32 + 0.5 - half_width;
                let py = y as f32 + 0.5 - half_height;
                let dx = px - px.clamp(radius - half_width, half_width - radius);
                let dy = py - py.clamp(radius - half_height, half_height - radius);
                let length = dx.hypot(dy);
                let depth = radius - length;
                let nx = dx / length.max(f32::EPSILON);
                let ny = dy / length.max(f32::EPSILON);
                let edge = 1.0 - smoothstep(depth / bevel);
                // Continue the outward ray past the window before sampling. The
                // display DC includes the toolbar itself within its bounds.
                let exit_x = distance_to_edge(x, width, nx);
                let exit_y = distance_to_edge(y, height, ny);
                let shift = exit_x.min(exit_y) + (2.0 + 17.0 * edge * edge) * scale;
                let sx = x as f32 + shift * nx + padding as f32;
                let sy = y as f32 + shift * ny + padding as f32;
                let direction = -0.59 * ny - 0.48 * nx;
                let rim = gaussian((depth / scale - 1.45) / 1.15)
                    * (0.19 + direction.max(0.0) * 0.72 + (-direction).max(0.0) * 0.21);
                let shade = gaussian((depth / scale - 4.5) / 2.75);
                optics.push(Optic {
                    source_x: sx,
                    source_y: sy,
                    rim,
                    shade,
                    lens: 0.68 * edge,
                    alpha: (255.0 * (depth + 0.5).clamp(0.0, 1.0)).round() as u8,
                });
            }
        }
        Some(Self {
            width,
            height,
            padding,
            capture,
            glyphs: Vec::new(),
            glyph_key: None,
            output,
            optics,
            last: None,
            monitors: Vec::with_capacity(4),
            copied_rects: Vec::with_capacity(8),
        })
    }

    unsafe fn sample_desktop(&mut self, bounds: RECT) -> bool {
        self.capture.pixels().fill(0);
        self.monitors.clear();
        self.copied_rects.clear();
        if EnumDisplayMonitors(
            ptr::null_mut(),
            ptr::null(),
            Some(collect_monitor),
            &mut self.monitors as *mut Vec<RECT> as LPARAM,
        ) == 0
        {
            return false;
        }
        let screen = GetDC(ptr::null_mut());
        if screen.is_null() {
            return false;
        }
        let padding = self.padding;
        let x0 = bounds.left - padding;
        let y0 = bounds.top - padding;
        let x1 = bounds.right + padding;
        let y1 = bounds.bottom + padding;
        for (l, t, r, b) in [
            (x0, y0, x1, bounds.top),
            (x0, bounds.bottom, x1, y1),
            (x0, bounds.top, bounds.left, bounds.bottom),
            (bounds.right, bounds.top, x1, bounds.bottom),
        ] {
            let strip = RECT {
                left: l,
                top: t,
                right: r,
                bottom: b,
            };
            for monitor in &self.monitors {
                let Some(rect) = intersection(strip, *monitor) else {
                    continue;
                };
                if BitBlt(
                    self.capture.dc,
                    rect.left - x0,
                    rect.top - y0,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    screen,
                    rect.left,
                    rect.top,
                    SRCCOPY,
                ) != 0
                {
                    self.copied_rects.push(rect);
                }
            }
        }
        GdiFlush();
        ReleaseDC(ptr::null_mut(), screen);
        let stride = (self.width + 2 * padding) as usize;
        let pixels = self.capture.pixels();
        for rect in &self.copied_rects {
            for row in (rect.top - y0) as usize..(rect.bottom - y0) as usize {
                let start = row * stride + (rect.left - x0) as usize;
                for pixel in &mut pixels[start..start + (rect.right - rect.left) as usize] {
                    *pixel |= 0xff00_0000;
                }
            }
        }
        !self.copied_rects.is_empty()
    }

    unsafe fn render(&mut self, window: HWND, mut frame: Frame) {
        let sampled = self.sample_desktop(frame.bounds);
        let input = self.capture.pixels();
        let hash = if sampled { fingerprint(input) } else { 0 };
        frame.controls.dark =
            !sampled || backdrop_is_dark(input, self.last.map(|(frame, _)| frame.controls.dark));
        if self.last == Some((frame, hash)) {
            return;
        }

        let dark = frame.controls.dark;
        let width = self.width;
        let height = self.height;
        let scale = frame.scale;
        let active = frame.controls.selected();
        let selected = frame.controls.layout.button_bounds(active);
        let hover = frame
            .controls
            .hover
            .filter(|&index| index != active)
            .and_then(|index| frame.controls.layout.button_bounds(index));
        let key = GlyphKey::new(scale, &frame.controls);
        if self.glyph_key != Some(key) {
            let Some(glyphs) =
                ui::toolbar_pixels(width as u32, height as u32, scale, &frame.controls)
                    .filter(|pixels| pixels.len() == self.output.len)
            else {
                return;
            };
            self.glyphs = glyphs;
            self.glyph_key = Some(key);
        }
        let glyphs = &self.glyphs;
        let output = self.output.pixels();
        for (offset, (target, optic)) in output.iter_mut().zip(&self.optics).enumerate() {
            let alpha = optic.alpha;
            if alpha == 0 {
                *target = 0;
                continue;
            }
            let glyph = glyphs[offset];
            if glyph >> 24 == 255 {
                *target = glyph;
                continue;
            }
            let x = (offset % width as usize) as i32;
            let y = (offset / width as usize) as i32;
            let sample = if optic.lens > 0.0 {
                sample_rim(
                    input,
                    width + self.padding * 2,
                    height + self.padding * 2,
                    optic.source_x,
                    optic.source_y,
                    (0.75 * scale).max(0.5),
                )
            } else {
                Sample::default()
            };
            let lens = optic.lens * sample.coverage;
            let reflection = frame.reflection.map_or(0.0, |point| {
                let dx = (x - point.x) as f32 / (56.0 * scale);
                let dy = (y - point.y) as f32 / (56.0 * scale);
                0.095 / (1.0 + 4.0 * (dx * dx + dy * dy))
            });
            let selected = chip_coverage(selected, x, y, scale);
            let hovered = chip_coverage(hover, x, y, scale);
            let glint = (optic.rim + reflection * (0.4 + optic.rim)).min(0.86);
            let tint_alpha = if !sampled {
                0.88
            } else if dark {
                DARK_TINT_ALPHA
            } else {
                LIGHT_TINT_ALPHA
            };
            let opacity = (1.0
                - (1.0 - tint_alpha)
                    * (1.0 - lens)
                    * (1.0 - glint)
                    * (1.0 - 0.24 * selected)
                    * (1.0 - 0.10 * hovered))
                * f32::from(alpha);
            let mut pixel = (opacity.round() as u32) << 24;
            for channel in 0..3 {
                let shift = channel * 8;
                let neutral = if dark {
                    [34.0, 29.0, 26.0]
                } else {
                    [251.0, 248.0, 245.0]
                };
                let tinted = mix(neutral[channel], sample.channels[channel], lens);
                let shaded = tinted * (1.0 - optic.shade * if dark { 0.31 } else { 0.17 });
                let glass = mix(shaded, 255.0, glint);
                let with_hover = mix(glass, if dark { 255.0 } else { 21.0 }, hovered * 0.09);
                let value = mix(with_hover, 255.0, selected * if dark { 0.23 } else { 0.46 });
                pixel |= ((value * opacity / 255.0).round() as u32) << shift;
            }
            *target = source_over(glyph, pixel);
        }

        let origin = POINT {
            x: frame.bounds.left,
            y: frame.bounds.top,
        };
        let size = SIZE {
            cx: width,
            cy: height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
            ..Default::default()
        };
        if UpdateLayeredWindow(
            window,
            ptr::null_mut(),
            &origin,
            &size,
            self.output.dc,
            &POINT::default(),
            0,
            &blend,
            ULW_ALPHA,
        ) != 0
        {
            self.last = Some((frame, hash));
        }
    }
}

#[derive(Default)]
pub struct Glass {
    buffers: Option<Buffers>,
    scale: f32,
}

impl Glass {
    pub unsafe fn render(
        &mut self,
        window: HWND,
        bounds: RECT,
        scale: f32,
        controls: ToolbarControls,
        reflection: Option<POINT>,
    ) {
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        if width <= 0 || height <= 0 {
            return;
        }
        if self.buffers.as_ref().is_none_or(|buffers| {
            buffers.width != width || buffers.height != height || self.scale != scale
        }) {
            self.buffers = Buffers::new(width, height, scale);
            self.scale = scale;
        }
        if let Some(buffers) = self.buffers.as_mut() {
            buffers.render(
                window,
                Frame {
                    bounds,
                    scale,
                    controls,
                    reflection,
                },
            );
        }
    }
}

fn mix(a: f32, b: f32, amount: f32) -> f32 {
    a + (b - a) * amount
}

fn gaussian(value: f32) -> f32 {
    (-value * value).exp()
}

fn smoothstep(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

unsafe extern "system" fn collect_monitor(
    _: HMONITOR,
    _: HDC,
    bounds: *mut RECT,
    data: LPARAM,
) -> i32 {
    if let Some(bounds) = bounds.as_ref() {
        (&mut *(data as *mut Vec<RECT>)).push(*bounds);
    }
    1
}

fn intersection(a: RECT, b: RECT) -> Option<RECT> {
    let rect = RECT {
        left: a.left.max(b.left),
        top: a.top.max(b.top),
        right: a.right.min(b.right),
        bottom: a.bottom.min(b.bottom),
    };
    (rect.left < rect.right && rect.top < rect.bottom).then_some(rect)
}

#[derive(Clone, Copy, Default)]
struct Sample {
    channels: [f32; 3],
    coverage: f32,
}

fn sample_bilinear(pixels: &[u32], width: i32, height: i32, x: f32, y: f32) -> Sample {
    if !x.is_finite()
        || !y.is_finite()
        || x <= -1.0
        || y <= -1.0
        || x >= width as f32
        || y >= height as f32
    {
        return Sample::default();
    }
    let left = x.floor() as i32;
    let top = y.floor() as i32;
    let fx = x - left as f32;
    let fy = y - top as f32;
    let mut sample = Sample::default();
    for (px, py, weight) in [
        (left, top, (1.0 - fx) * (1.0 - fy)),
        (left + 1, top, fx * (1.0 - fy)),
        (left, top + 1, (1.0 - fx) * fy),
        (left + 1, top + 1, fx * fy),
    ] {
        if px < 0 || py < 0 || px >= width || py >= height {
            continue;
        }
        let color = pixels[(py * width + px) as usize];
        let weight = weight * (color >> 24) as f32 / 255.0;
        sample.coverage += weight;
        for (channel, value) in sample.channels.iter_mut().enumerate() {
            *value += ((color >> (channel * 8)) & 255) as f32 * weight;
        }
    }
    if sample.coverage > 0.0 {
        for value in &mut sample.channels {
            *value /= sample.coverage;
        }
    }
    sample
}

fn sample_rim(pixels: &[u32], width: i32, height: i32, x: f32, y: f32, spread: f32) -> Sample {
    let mut result = Sample::default();
    for (sx, sy, weight) in [
        (x, y, 2.0),
        (x - spread, y, 1.0),
        (x + spread, y, 1.0),
        (x, y - spread, 1.0),
        (x, y + spread, 1.0),
    ] {
        let sample = sample_bilinear(pixels, width, height, sx, sy);
        let weight = weight * sample.coverage;
        result.coverage += weight;
        for (value, channel) in result.channels.iter_mut().zip(sample.channels) {
            *value += channel * weight;
        }
    }
    if result.coverage > 0.0 {
        for value in &mut result.channels {
            *value /= result.coverage;
        }
    }
    result.coverage /= 6.0;
    result
}

fn source_over(source: u32, destination: u32) -> u32 {
    let alpha = source >> 24;
    if alpha == 0 {
        return destination;
    }
    let remaining = 255 - alpha;
    let mut output = 0;
    for shift in [0, 8, 16, 24] {
        let source = (source >> shift) & 255;
        let destination = (destination >> shift) & 255;
        output |= (source + (destination * remaining + 127) / 255).min(255) << shift;
    }
    output
}

fn distance_to_edge(position: i32, extent: i32, normal: f32) -> f32 {
    if normal < 0.0 {
        (position as f32 + 1.0) / -normal
    } else if normal > 0.0 {
        (extent - position) as f32 / normal
    } else {
        f32::INFINITY
    }
}

fn fingerprint(pixels: &[u32]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, pixel| {
        (hash ^ u64::from(*pixel)).wrapping_mul(0x100_0000_01b3)
    })
}

fn backdrop_is_dark(pixels: &[u32], previous: Option<bool>) -> bool {
    let (sum, count) = pixels
        .iter()
        .step_by(13)
        .filter(|&&pixel| pixel >> 24 != 0)
        .fold((0_u64, 0_u64), |(sum, count), &color| {
            let luma = 21 * ((color >> 16) & 255) + 72 * ((color >> 8) & 255) + 7 * (color & 255);
            (sum + u64::from(luma), count + 1)
        });
    let threshold = match previous {
        Some(true) => 14_500,
        Some(false) => 11_500,
        None => 13_000,
    };
    count == 0 || sum < threshold * count
}

fn chip_coverage(bounds: Option<(i32, i32, i32, i32)>, x: i32, y: i32, scale: f32) -> f32 {
    let Some((left, top, width, height)) = bounds else {
        return 0.0;
    };
    let dx = x as f32 + 0.5 - (left as f32 + width as f32 * 0.5) * scale;
    let dy = y as f32 + 0.5 - (top as f32 + height as f32 * 0.5) * scale;
    let radius = width.min(height) as f32 * scale * 0.5;
    if dx.abs() > radius + 0.5 || dy.abs() > radius + 0.5 {
        return 0.0;
    }
    (radius + 0.5 - dx.hypot(dy)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ink::Tool;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, IsWindowVisible, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_POPUP,
    };

    struct HiddenWindow(HWND);

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.0) };
        }
    }

    #[test]
    fn premultiplied_source_over_preserves_partial_glyph_coverage() {
        assert_eq!(source_over(0, 0x8060_3020), 0x8060_3020);
        assert_eq!(source_over(0xff40_2010, 0x8060_3020), 0xff40_2010);
        assert_eq!(source_over(0x8040_2010, 0x8060_3020), 0xc070_3820);
        assert_eq!(source_over(0x8040_2010, 0), 0x8040_2010);
        for source in [0, 0x8040_2010, 0xffe0_d0c0] {
            for destination in [0, 0x8060_3020, 0xff20_4060] {
                let pixel = source_over(source, destination);
                let alpha = pixel >> 24;
                for shift in [0, 8, 16] {
                    assert!((pixel >> shift) & 255 <= alpha);
                }
            }
        }
    }

    #[test]
    fn bilinear_samples_interpolate_fractional_screen_positions() {
        let pixels = [0xffff_0000, 0xff00_ff00, 0xff00_00ff, 0xffff_ffff];
        let sample = sample_bilinear(&pixels, 2, 2, 0.5, 0.5);
        assert_eq!(sample.channels, [127.5; 3]);
        assert_eq!(sample.coverage, 1.0);
        let sample = sample_bilinear(&pixels, 2, 2, 0.0, 0.0);
        assert_eq!(sample.channels, [0.0, 0.0, 255.0]);
    }

    #[test]
    fn missing_capture_pixels_fade_the_lens_without_black_or_invalid_color() {
        let pixels = [0xffff_0000, 0x00ff_ffff, 0, 0];
        let sample = sample_bilinear(&pixels, 2, 2, 0.5, 0.5);
        assert_eq!(sample.coverage, 0.25);
        assert_eq!(sample.channels, [0.0, 0.0, 255.0]);
        assert_eq!(sample_bilinear(&pixels, 2, 2, 1.0, 1.0).coverage, 0.0);
        assert_eq!(sample_bilinear(&pixels, 2, 2, f32::NAN, 0.0).coverage, 0.0);
        assert_eq!(sample_bilinear(&pixels, 2, 2, -1.0, 0.0).coverage, 0.0);
    }

    #[test]
    fn small_rim_filter_softens_a_single_pixel_and_keeps_constant_colors() {
        let mut pixels = [0xff00_0000; 49];
        pixels[24] = 0xffff_ffff;
        let sample = sample_rim(&pixels, 7, 7, 3.0, 3.0, 1.0);
        assert_eq!(sample.coverage, 1.0);
        assert_eq!(sample.channels, [85.0; 3]);
        pixels.fill(0xffa0_6040);
        let sample = sample_rim(&pixels, 7, 7, 3.3, 3.6, 0.75);
        assert!((sample.coverage - 1.0).abs() < 0.00001);
        for (actual, expected) in sample.channels.into_iter().zip([64.0, 96.0, 160.0]) {
            assert!((actual - expected).abs() < 0.001);
        }
    }

    #[test]
    fn screen_intersections_reject_monitor_gaps_and_keep_negative_origins() {
        let first = RECT {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let second = RECT {
            left: 0,
            top: 300,
            right: 1920,
            bottom: 1380,
        };
        let gap = RECT {
            left: 0,
            top: 20,
            right: 200,
            bottom: 100,
        };
        assert!(intersection(gap, first).is_none());
        assert!(intersection(gap, second).is_none());
        let seam = RECT {
            left: -20,
            top: 400,
            right: 20,
            bottom: 420,
        };
        let left = intersection(seam, first).unwrap();
        let right = intersection(seam, second).unwrap();
        assert_eq!((left.left, left.right), (-20, 0));
        assert_eq!((right.left, right.right), (0, 20));
    }

    #[test]
    fn contrast_mode_has_hysteresis_near_the_original_midpoint() {
        let near_middle = [0xff82_8282; 52];
        assert!(!backdrop_is_dark(&near_middle, None));
        assert!(backdrop_is_dark(&near_middle, Some(true)));
        assert!(!backdrop_is_dark(&near_middle, Some(false)));
        assert!(!backdrop_is_dark(&[0xffb0_b0b0; 52], Some(true)));
        assert!(backdrop_is_dark(&[0xff40_4040; 52], Some(false)));
        assert!(backdrop_is_dark(&[0; 52], Some(false)));
    }

    #[test]
    fn center_tint_keeps_icons_visible_against_the_opposite_desktop_color() {
        fn luminance(channels: [f32; 3]) -> f32 {
            let linear = channels.map(|value| {
                let value = value / 255.0;
                if value <= 0.04045 {
                    value / 12.92
                } else {
                    ((value + 0.055) / 1.055).powf(2.4)
                }
            });
            linear[0] * 0.2126 + linear[1] * 0.7152 + linear[2] * 0.0722
        }
        for (glyph, neutral, opacity, desktop) in [
            (
                [250.0, 251.0, 254.0],
                [26.0, 29.0, 34.0],
                DARK_TINT_ALPHA,
                255.0,
            ),
            (
                [24.0, 27.0, 34.0],
                [245.0, 248.0, 251.0],
                LIGHT_TINT_ALPHA,
                0.0,
            ),
        ] {
            let background = neutral.map(|channel| mix(desktop, channel, opacity));
            let glyph = luminance(glyph);
            let background = luminance(background);
            let contrast = (glyph.max(background) + 0.05) / (glyph.min(background) + 0.05);
            assert!(
                contrast >= 3.0,
                "opposite desktop icon contrast was {contrast}"
            );
        }
    }

    #[test]
    fn glyph_cache_ignores_hover_and_tool_state_but_tracks_color_scale_and_layout() {
        let controls = ToolbarControls {
            layout: ToolbarLayout::Horizontal,
            interactive: false,
            tool: Tool::Pen,
            primary: Color::rgb(16, 32, 48),
            hover: None,
            dark: false,
        };
        let key = GlyphKey::new(1.0, &controls);
        let mut changed = controls;
        changed.hover = Some(8);
        changed.tool = Tool::Eraser;
        changed.interactive = true;
        assert!(key == GlyphKey::new(1.0, &changed));
        assert!(key != GlyphKey::new(1.5, &changed));
        changed.primary = Color::rgb(255, 255, 255);
        assert!(key != GlyphKey::new(1.0, &changed));
        changed = controls;
        changed.layout = ToolbarLayout::Vertical;
        assert!(key != GlyphKey::new(1.0, &changed));
        changed = controls;
        changed.dark = true;
        assert!(key != GlyphKey::new(1.0, &changed));
    }

    #[test]
    fn hidden_native_renderer_keeps_fractional_glyph_alpha_and_reuses_the_cache() {
        unsafe {
            let class: Vec<u16> = "Static\0".encode_utf16().collect();
            let window = HiddenWindow(CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW,
                class.as_ptr(),
                ptr::null(),
                WS_POPUP,
                -32000,
                -32000,
                480,
                60,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
            ));
            assert!(!window.0.is_null());
            let mut buffers = Buffers::new(480, 60, 1.0).unwrap();
            let mut frame = Frame {
                bounds: RECT {
                    left: -32000,
                    top: -32000,
                    right: -31520,
                    bottom: -31940,
                },
                scale: 1.0,
                controls: ToolbarControls {
                    layout: ToolbarLayout::Horizontal,
                    interactive: false,
                    tool: Tool::Pen,
                    primary: Color::rgb(99, 102, 241),
                    hover: None,
                    dark: false,
                },
                reflection: None,
            };
            buffers.render(window.0, frame);
            assert!(buffers.last.is_some());
            assert_eq!(IsWindowVisible(window.0), 0);
            assert!(
                buffers
                    .glyphs
                    .iter()
                    .any(|pixel| (1..255).contains(&(pixel >> 24)))
            );
            let cached = buffers.glyphs.as_ptr();
            let first = buffers.output.pixels().to_vec();
            assert_eq!(first[0], 0);
            for pixel in &first {
                let alpha = pixel >> 24;
                for shift in [0, 8, 16] {
                    assert!((pixel >> shift) & 255 <= alpha);
                }
            }
            frame.controls.hover = Some(5);
            frame.controls.interactive = true;
            frame.controls.tool = Tool::Eraser;
            buffers.render(window.0, frame);
            assert_eq!(buffers.glyphs.as_ptr(), cached);
            assert_ne!(buffers.output.pixels(), first);
            assert_eq!(IsWindowVisible(window.0), 0);
        }
    }
}
