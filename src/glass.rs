//! Renders the floating toolbar against the desktop pixels directly beneath it.

use crate::ui::{self, ToolbarControls};
use std::{mem::size_of, ptr, slice};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT, SIZE},
    Graphics::Gdi::*,
    UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN, ULW_ALPHA, UpdateLayeredWindow,
    },
};

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u32,
    len: usize,
}

impl Surface {
    unsafe fn new(width: i32, height: i32) -> Option<Self> {
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
    source: usize,
    rim: f32,
    shade: f32,
    lens: f32,
    alpha: u8,
}

struct Buffers {
    width: i32,
    height: i32,
    padding: i32,
    capture: Surface,
    controls: Surface,
    output: Surface,
    optics: Vec<Optic>,
    last: Option<(Frame, u64)>,
}

impl Buffers {
    unsafe fn new(width: i32, height: i32, scale: f32) -> Option<Self> {
        let padding = (23.0 * scale).ceil().max(4.0) as i32;
        let capture_width = width + 2 * padding;
        let capture_height = height + 2 * padding;
        let capture = Surface::new(capture_width, capture_height)?;
        let controls = Surface::new(width, height)?;
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
                let sx = (x as f32 + shift * nx).round() as i32 + padding;
                let sy = (y as f32 + shift * ny).round() as i32 + padding;
                let direction = -0.59 * ny - 0.48 * nx;
                let rim = gaussian((depth / scale - 1.45) / 1.15)
                    * (0.19 + direction.max(0.0) * 0.72 + (-direction).max(0.0) * 0.21);
                let shade = gaussian((depth / scale - 4.5) / 2.75);
                optics.push(Optic {
                    source: index(capture_width, capture_height, sx, sy),
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
            controls,
            output,
            optics,
            last: None,
        })
    }

    unsafe fn sample_desktop(&mut self, bounds: RECT) -> bool {
        let left = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let top = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let right = left + GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let bottom = top + GetSystemMetrics(SM_CYVIRTUALSCREEN);
        let screen = GetDC(ptr::null_mut());
        if screen.is_null() {
            return false;
        }
        self.capture.pixels().fill(0);
        let padding = self.padding;
        let x0 = bounds.left - padding;
        let y0 = bounds.top - padding;
        let x1 = bounds.right + padding;
        let y1 = bounds.bottom + padding;
        let mut copied = false;
        for (l, t, r, b) in [
            (x0, y0, x1, bounds.top),
            (x0, bounds.bottom, x1, y1),
            (x0, bounds.top, bounds.left, bounds.bottom),
            (bounds.right, bounds.top, x1, bounds.bottom),
        ] {
            let (l, t, r, b) = (l.max(left), t.max(top), r.min(right), b.min(bottom));
            if l >= r || t >= b {
                continue;
            }
            if BitBlt(
                self.capture.dc,
                l - x0,
                t - y0,
                r - l,
                b - t,
                screen,
                l,
                t,
                SRCCOPY,
            ) != 0
            {
                GdiFlush();
                let stride = (self.width + 2 * padding) as usize;
                let pixels = self.capture.pixels();
                for row in (t - y0) as usize..(b - y0) as usize {
                    let start = row * stride + (l - x0) as usize;
                    for pixel in &mut pixels[start..start + (r - l) as usize] {
                        *pixel |= 0xff00_0000;
                    }
                }
                copied = true;
            }
        }
        ReleaseDC(ptr::null_mut(), screen);
        copied
    }

    unsafe fn render(&mut self, window: HWND, mut frame: Frame) {
        let sampled = self.sample_desktop(frame.bounds);
        let input = self.capture.pixels();
        let hash = if sampled { fingerprint(input) } else { 0 };
        frame.controls.dark = !sampled || backdrop_is_dark(input);
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
        let primary = frame.controls.primary;
        let primary_bgr =
            (u32::from(primary.r) << 16) | (u32::from(primary.g) << 8) | u32::from(primary.b);
        let glyph_background = if primary_bgr == 0x0001_0203 {
            0x0002_0304
        } else {
            0x0001_0203
        };
        self.controls.pixels().fill(glyph_background);
        ui::paint_toolbar_controls(self.controls.dc, scale, &frame.controls);
        GdiFlush();
        let glyphs = self.controls.pixels();
        let output = self.output.pixels();
        for (offset, (target, optic)) in output.iter_mut().zip(&self.optics).enumerate() {
            let alpha = optic.alpha;
            if alpha == 0 {
                *target = 0;
                continue;
            }
            let glyph = glyphs[offset] & 0x00ff_ffff;
            if glyph != glyph_background {
                *target = glyph | 0xff00_0000;
                continue;
            }
            let x = (offset % width as usize) as i32;
            let y = (offset / width as usize) as i32;
            let center = input[optic.source];
            let lens = if center >> 24 == 0 { 0.0 } else { optic.lens };
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
                0.37
            } else {
                0.30
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
                let refracted = ((center >> shift) & 255) as f32;
                let neutral = if dark {
                    [34.0, 29.0, 26.0]
                } else {
                    [251.0, 248.0, 245.0]
                };
                let tinted = mix(neutral[channel], refracted, lens);
                let shaded = tinted * (1.0 - optic.shade * if dark { 0.31 } else { 0.17 });
                let glass = mix(shaded, 255.0, glint);
                let with_hover = mix(glass, if dark { 255.0 } else { 21.0 }, hovered * 0.09);
                let value = mix(with_hover, 255.0, selected * if dark { 0.23 } else { 0.46 });
                pixel |= ((value * opacity / 255.0).round() as u32) << shift;
            }
            *target = pixel;
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

fn index(width: i32, height: i32, x: i32, y: i32) -> usize {
    (y.clamp(0, height - 1) * width + x.clamp(0, width - 1)) as usize
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

fn backdrop_is_dark(pixels: &[u32]) -> bool {
    let (sum, count) = pixels
        .iter()
        .step_by(13)
        .filter(|&&pixel| pixel >> 24 != 0)
        .fold((0_u64, 0_u64), |(sum, count), &color| {
            let luma = 21 * ((color >> 16) & 255) + 72 * ((color >> 8) & 255) + 7 * (color & 255);
            (sum + u64::from(luma), count + 1)
        });
    count == 0 || sum < 13_000 * count
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
