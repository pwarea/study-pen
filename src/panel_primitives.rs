//! Antialiased native controls rendered through one small reusable BGRA tile.

use crate::ink::Color;
use std::{cell::RefCell, mem::size_of, ptr, slice};
use tiny_skia::{FillRule, Paint, Path, PathBuilder, PixmapMut, Stroke, Transform};
use windows_sys::Win32::Graphics::Gdi::*;

const TILE_WIDTH: u32 = 192;
const TILE_HEIGHT: u32 = 96;

thread_local! {
    static TILE: RefCell<Option<Tile>> = const { RefCell::new(None) };
}

/// Bounds and stroke width are physical pixels. Fractional coordinates retain
/// the same center when Windows uses a fractional display scale.
pub unsafe fn rounded(
    dc: HDC,
    bounds: [f32; 4],
    radius: f32,
    fill: Color,
    border: Color,
    border_width: f32,
) -> bool {
    let [x, y, width, height] = bounds;
    if !bounds.into_iter().all(f32::is_finite)
        || !radius.is_finite()
        || !border_width.is_finite()
        || width <= 0.0
        || height <= 0.0
    {
        return false;
    }
    let border_width = border_width.clamp(0.0, width.min(height) * 0.5);
    let inset = border_width * 0.5;
    let radius = (radius - inset).clamp(0.0, (width.min(height) - border_width) * 0.5);
    let Some(path) = rounded_path(
        x + inset,
        y + inset,
        width - border_width,
        height - border_width,
        radius,
    ) else {
        return false;
    };
    paint(dc, &path, Some(fill), border, border_width)
}

pub unsafe fn circle(
    dc: HDC,
    center_x: f32,
    center_y: f32,
    radius: f32,
    fill: Option<Color>,
    border: Color,
    border_width: f32,
) -> bool {
    if ![center_x, center_y, radius, border_width]
        .into_iter()
        .all(f32::is_finite)
        || radius <= 0.0
    {
        return false;
    }
    let border_width = border_width.clamp(0.0, radius);
    let Some(path) = PathBuilder::from_circle(center_x, center_y, radius - border_width * 0.5)
    else {
        return false;
    };
    paint(dc, &path, fill, border, border_width)
}

fn rounded_path(x: f32, y: f32, width: f32, height: f32, radius: f32) -> Option<Path> {
    let right = x + width;
    let bottom = y + height;
    let curve = radius * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x + radius, y);
    path.line_to(right - radius, y);
    path.cubic_to(
        right - radius + curve,
        y,
        right,
        y + radius - curve,
        right,
        y + radius,
    );
    path.line_to(right, bottom - radius);
    path.cubic_to(
        right,
        bottom - radius + curve,
        right - radius + curve,
        bottom,
        right - radius,
        bottom,
    );
    path.line_to(x + radius, bottom);
    path.cubic_to(
        x + radius - curve,
        bottom,
        x,
        bottom - radius + curve,
        x,
        bottom - radius,
    );
    path.line_to(x, y + radius);
    path.cubic_to(x, y + radius - curve, x + radius - curve, y, x + radius, y);
    path.close();
    path.finish()
}

fn color_paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    // Tiny-skia writes RGBA bytes. Reversed paint channels produce the BGRA
    // byte order GDI expects without an extra copy or conversion pass.
    paint.set_color_rgba8(color.b, color.g, color.r, 255);
    paint.anti_alias = true;
    paint
}

unsafe fn paint(
    dc: HDC,
    path: &Path,
    fill: Option<Color>,
    border: Color,
    border_width: f32,
) -> bool {
    if dc.is_null() {
        return false;
    }
    let bounds = path.bounds();
    let margin = border_width * 0.5 + 1.0;
    let left = (bounds.left() - margin).floor();
    let top = (bounds.top() - margin).floor();
    let right = (bounds.right() + margin).ceil();
    let bottom = (bounds.bottom() + margin).ceil();
    if left < i32::MIN as f32
        || top < i32::MIN as f32
        || right >= i32::MAX as f32
        || bottom >= i32::MAX as f32
    {
        return false;
    }
    TILE.with(|cache| {
        let Ok(mut cache) = cache.try_borrow_mut() else {
            return false;
        };
        if cache.is_none() {
            *cache = Tile::new();
        }
        let Some(tile) = cache.as_mut() else {
            return false;
        };
        let fill = fill.map(color_paint);
        let border = color_paint(border);
        let stroke = Stroke {
            width: border_width,
            ..Default::default()
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
            ..Default::default()
        };
        for y in (top as i32..bottom as i32).step_by(TILE_HEIGHT as usize) {
            for x in (left as i32..right as i32).step_by(TILE_WIDTH as usize) {
                // Flush any earlier GDI reads before reusing the DIB's bytes.
                GdiFlush();
                let data =
                    slice::from_raw_parts_mut(tile.bits, (TILE_WIDTH * TILE_HEIGHT * 4) as usize);
                data.fill(0);
                let Some(mut pixels) = PixmapMut::from_bytes(data, TILE_WIDTH, TILE_HEIGHT) else {
                    return false;
                };
                let transform = Transform::from_translate(-(x as f32), -(y as f32));
                if let Some(fill) = &fill {
                    pixels.fill_path(path, fill, FillRule::Winding, transform, None);
                }
                if border_width > 0.0 {
                    pixels.stroke_path(path, &border, &stroke, transform, None);
                }
                let width = (right as i32 - x).min(TILE_WIDTH as i32);
                let height = (bottom as i32 - y).min(TILE_HEIGHT as i32);
                if GdiAlphaBlend(dc, x, y, width, height, tile.dc, 0, 0, width, height, blend) == 0
                {
                    return false;
                }
            }
        }
        true
    })
}

struct Tile {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    bits: *mut u8,
}

impl Tile {
    unsafe fn new() -> Option<Self> {
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return None;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: TILE_WIDTH as i32,
                biHeight: -(TILE_HEIGHT as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = ptr::null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
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

impl Drop for Tile {
    fn drop(&mut self) {
        unsafe {
            GdiFlush();
            SelectObject(self.dc, self.previous);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_circle_blends_fractional_edges_without_losing_the_background() {
        unsafe {
            let target = Tile::new().unwrap();
            slice::from_raw_parts_mut(
                target.bits.cast::<u32>(),
                (TILE_WIDTH * TILE_HEIGHT) as usize,
            )
            .fill(0xff00_0000);
            assert!(circle(
                target.dc,
                31.5,
                31.5,
                12.5,
                Some(Color::rgb(230, 80, 30)),
                Color::rgb(230, 80, 30),
                1.25,
            ));
            GdiFlush();
            let pixels = slice::from_raw_parts(
                target.bits.cast::<u32>(),
                (TILE_WIDTH * TILE_HEIGHT) as usize,
            );
            assert_eq!(pixels[0] & 0x00ff_ffff, 0);
            let center = pixels[31 * TILE_WIDTH as usize + 31] & 0x00ff_ffff;
            assert_eq!(center, 0x00e6_501e);
            assert!(pixels.iter().any(|pixel| {
                let red = (pixel >> 16) & 255;
                red > 0 && red < 230
            }));
        }
    }

    #[test]
    fn tiled_rounded_rectangle_has_a_continuous_fill_across_tile_boundaries() {
        unsafe {
            // The destination is wider than a scratch tile, so paint it in two
            // calls translated into separate retained test surfaces.
            let left = Tile::new().unwrap();
            let right = Tile::new().unwrap();
            let color = Color::rgb(30, 90, 180);
            assert!(rounded(
                left.dc,
                [0.0, 0.0, 380.0, 70.0],
                15.0,
                color,
                color,
                1.0
            ));
            assert!(rounded(
                right.dc,
                [-192.0, 0.0, 380.0, 70.0],
                15.0,
                color,
                color,
                1.0
            ));
            GdiFlush();
            let first =
                slice::from_raw_parts(left.bits.cast::<u32>(), (TILE_WIDTH * TILE_HEIGHT) as usize);
            let second = slice::from_raw_parts(
                right.bits.cast::<u32>(),
                (TILE_WIDTH * TILE_HEIGHT) as usize,
            );
            for y in 3..67 {
                assert_eq!(
                    first[y * TILE_WIDTH as usize + 191] & 0x00ff_ffff,
                    0x001e_5ab4
                );
                assert_eq!(second[y * TILE_WIDTH as usize] & 0x00ff_ffff, 0x001e_5ab4);
            }
        }
    }
}
