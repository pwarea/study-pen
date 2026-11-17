//! A blurred desktop snapshot captured before the settings window is shown.

use crate::toolbar_preferences::ToolbarTheme;
use std::{mem::size_of, ptr, slice};
use windows_sys::Win32::{
    Foundation::{POINT, RECT},
    Graphics::Gdi::*,
};

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmFlush() -> i32;
}

pub struct PanelMaterial {
    pixels: Vec<u32>,
    width: i32,
    height: i32,
}

impl PanelMaterial {
    /// Capture only while the panel is hidden, so the snapshot cannot contain
    /// the panel itself. The retained image stays fixed until the next capture.
    pub unsafe fn capture(bounds: RECT, theme: ToolbarTheme) -> Self {
        let full_width = i64::from(bounds.right) - i64::from(bounds.left);
        let full_height = i64::from(bounds.bottom) - i64::from(bounds.top);
        if full_width <= 0 || full_height <= 0 {
            return Self {
                pixels: vec![tint(0, 0.0, theme, false)],
                width: 1,
                height: 1,
            };
        }
        let reduction = 4.0_f64
            .max(full_width as f64 / 384.0)
            .max(full_height as f64 / 512.0);
        let width = (full_width as f64 / reduction).ceil().max(1.0) as i32;
        let height = (full_height as f64 / reduction).ceil().max(1.0) as i32;
        let mut pixels = vec![0; (width * height) as usize];
        let captured = full_width <= i64::from(i32::MAX)
            && full_height <= i64::from(i32::MAX)
            && capture_pixels(bounds, width, height, &mut pixels);
        if captured {
            let radius = ((width as f32 / 115.0) * 3.0).round().clamp(2.0, 12.0) as usize;
            let mut scratch = vec![0; pixels.len()];
            for _ in 0..2 {
                blur_horizontal(
                    &pixels,
                    &mut scratch,
                    width as usize,
                    height as usize,
                    radius,
                );
                blur_vertical(
                    &scratch,
                    &mut pixels,
                    width as usize,
                    height as usize,
                    radius,
                );
            }
        }
        for y in 0..height {
            let vertical = y as f32 / (height - 1).max(1) as f32;
            for x in 0..width {
                let index = (y * width + x) as usize;
                pixels[index] = tint(pixels[index], vertical, theme, captured);
            }
        }
        Self {
            pixels,
            width,
            height,
        }
    }

    /// Paint the material first, then draw the panel's labels and controls.
    pub unsafe fn paint(&self, dc: HDC, width: i32, height: i32) -> bool {
        if dc.is_null() || width <= 0 || height <= 0 {
            return false;
        }
        let info = bitmap_info(self.width, self.height);
        let previous_mode = SetStretchBltMode(dc, HALFTONE);
        let mut previous_origin = POINT::default();
        let changed_origin = SetBrushOrgEx(dc, 0, 0, &mut previous_origin) != 0;
        let result = StretchDIBits(
            dc,
            0,
            0,
            width,
            height,
            0,
            0,
            self.width,
            self.height,
            self.pixels.as_ptr().cast(),
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        if changed_origin {
            SetBrushOrgEx(dc, previous_origin.x, previous_origin.y, ptr::null_mut());
        }
        if previous_mode != 0 {
            SetStretchBltMode(dc, previous_mode);
        }
        result != 0 && result != -1
    }
}

fn bitmap_info(width: i32, height: i32) -> BITMAPINFO {
    BITMAPINFO {
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
    }
}

struct CaptureSurface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u32,
}

impl CaptureSurface {
    unsafe fn new(width: i32, height: i32) -> Option<Self> {
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let mut bits = ptr::null_mut();
        let bitmap = CreateDIBSection(
            dc,
            &bitmap_info(width, height),
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        );
        if bitmap.is_null() || bits.is_null() {
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
            bits: bits.cast(),
        })
    }
}

impl Drop for CaptureSurface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

unsafe fn capture_pixels(bounds: RECT, width: i32, height: i32, pixels: &mut [u32]) -> bool {
    // A caller may have just hidden the old panel to refresh its backdrop.
    // Wait for that visibility change to reach the compositor before sampling.
    if DwmFlush() < 0 {
        return false;
    }
    let Some(surface) = CaptureSurface::new(width, height) else {
        return false;
    };
    let screen = GetDC(ptr::null_mut());
    if screen.is_null() {
        return false;
    }
    SetStretchBltMode(surface.dc, HALFTONE);
    SetBrushOrgEx(surface.dc, 0, 0, ptr::null_mut());
    let copied = StretchBlt(
        surface.dc,
        0,
        0,
        width,
        height,
        screen,
        bounds.left,
        bounds.top,
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
        SRCCOPY,
    ) != 0;
    GdiFlush();
    ReleaseDC(ptr::null_mut(), screen);
    if copied {
        pixels.copy_from_slice(slice::from_raw_parts(surface.bits, pixels.len()));
    }
    copied
}

fn tint(pixel: u32, vertical: f32, theme: ToolbarTheme, captured: bool) -> u32 {
    let (top, bottom, opacity) = match theme {
        ToolbarTheme::LiquidGlass => ([31.0, 38.0, 45.0], [22.0, 27.0, 33.0], 0.84),
        ToolbarTheme::LiquidAir => ([48.0, 66.0, 101.0], [24.0, 34.0, 59.0], 0.82),
    };
    let opacity = if captured { opacity } else { 1.0 };
    let mut result = 0xff00_0000;
    for channel in 0..3 {
        let neutral = top[2 - channel] + (bottom[2 - channel] - top[2 - channel]) * vertical;
        let background = ((pixel >> (channel * 8)) & 255) as f32;
        let value = background + (neutral - background) * opacity;
        result |= (value.round().clamp(0.0, 255.0) as u32) << (channel * 8);
    }
    result
}

fn blur_horizontal(input: &[u32], output: &mut [u32], width: usize, height: usize, radius: usize) {
    let diameter = (radius * 2 + 1) as u32;
    for y in 0..height {
        let row = &input[y * width..(y + 1) * width];
        let mut sum = [0; 3];
        for sample in 0..=radius * 2 {
            add(&mut sum, row[sample.saturating_sub(radius).min(width - 1)]);
        }
        for x in 0..width {
            output[y * width + x] = average(sum, diameter);
            subtract(&mut sum, row[x.saturating_sub(radius)]);
            add(&mut sum, row[(x + radius + 1).min(width - 1)]);
        }
    }
}

fn blur_vertical(input: &[u32], output: &mut [u32], width: usize, height: usize, radius: usize) {
    let diameter = (radius * 2 + 1) as u32;
    for x in 0..width {
        let mut sum = [0; 3];
        for sample in 0..=radius * 2 {
            let y = sample.saturating_sub(radius).min(height - 1);
            add(&mut sum, input[y * width + x]);
        }
        for y in 0..height {
            output[y * width + x] = average(sum, diameter);
            subtract(&mut sum, input[y.saturating_sub(radius) * width + x]);
            add(
                &mut sum,
                input[(y + radius + 1).min(height - 1) * width + x],
            );
        }
    }
}

fn add(sum: &mut [u32; 3], pixel: u32) {
    for (channel, value) in sum.iter_mut().enumerate() {
        *value += (pixel >> (channel * 8)) & 255;
    }
}

fn subtract(sum: &mut [u32; 3], pixel: u32) {
    for (channel, value) in sum.iter_mut().enumerate() {
        *value -= (pixel >> (channel * 8)) & 255;
    }
}

fn average(sum: [u32; 3], diameter: u32) -> u32 {
    let [blue, green, red] = sum.map(|value| (value + diameter / 2) / diameter);
    0xff00_0000 | (red << 16) | (green << 8) | blue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_keeps_constant_colors_and_diffuses_a_thin_line() {
        let input = vec![0xff60_4020; 15 * 9];
        let mut scratch = vec![0; input.len()];
        let mut output = vec![0; input.len()];
        blur_horizontal(&input, &mut scratch, 15, 9, 3);
        blur_vertical(&scratch, &mut output, 15, 9, 3);
        assert_eq!(output, input);

        let mut line = vec![0xff00_0000; 15 * 9];
        for y in 0..9 {
            line[y * 15 + 7] = 0xffff_ffff;
        }
        blur_horizontal(&line, &mut scratch, 15, 9, 3);
        blur_vertical(&scratch, &mut output, 15, 9, 3);
        assert_eq!(output[4 * 15 + 7], output[4 * 15 + 6]);
        assert!((output[4 * 15 + 7] & 255) > 0);
        assert!((output[4 * 15 + 7] & 255) < 255);
        assert_eq!(output[4 * 15], 0xff00_0000);
    }

    #[test]
    fn single_pixel_blur_and_fallback_are_readable_and_opaque() {
        let mut scratch = [0];
        let mut output = [0];
        blur_horizontal(&[0xff12_3456], &mut scratch, 1, 1, 4);
        blur_vertical(&scratch, &mut output, 1, 1, 4);
        assert_eq!(output[0], 0xff12_3456);
        for theme in [ToolbarTheme::LiquidGlass, ToolbarTheme::LiquidAir] {
            for background in [0, 0x00ff_ffff] {
                let pixel = tint(background, 0.5, theme, true);
                assert_eq!(pixel >> 24, 255);
                assert!(((pixel >> 16) & 255) < 100);
            }
            assert_eq!(
                tint(0, 0.5, theme, false),
                tint(0x00ff_ffff, 0.5, theme, false)
            );
        }
        assert_ne!(
            tint(0, 0.0, ToolbarTheme::LiquidAir, false),
            tint(0, 1.0, ToolbarTheme::LiquidAir, false),
        );
    }

    #[test]
    fn native_snapshot_paint_scales_pixels_and_restores_the_dc_mode() {
        unsafe {
            let target = CaptureSurface::new(40, 30).unwrap();
            let previous = SetStretchBltMode(target.dc, COLORONCOLOR);
            let material = PanelMaterial {
                pixels: vec![0xff30_6090; 6],
                width: 3,
                height: 2,
            };
            assert!(material.paint(target.dc, 40, 30));
            assert_eq!(GetStretchBltMode(target.dc), COLORONCOLOR);
            GdiFlush();
            let pixels = slice::from_raw_parts(target.bits, 40 * 30);
            assert!(
                pixels
                    .iter()
                    .all(|pixel| pixel & 0x00ff_ffff == 0x0030_6090)
            );
            if previous != 0 {
                SetStretchBltMode(target.dc, previous);
            }
        }
    }
}
