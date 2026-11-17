//! Cached grayscale text masks for the premultiplied BGRA toolbar surface.

use crate::ink::Color;
use std::{cell::RefCell, mem::size_of, ptr};
use windows_sys::Win32::{
    Foundation::{RECT, SIZE},
    Graphics::Gdi::*,
};

const FONT_SIZE_DIP: f32 = 11.0;
const MAX_MASK_PIXELS: usize = 1024 * 1024;
const MAX_CACHED_LABELS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub struct Label {
    pub text: &'static str,
    /// Logical x, y, width and height. Text is centered within this slot.
    pub bounds: [i32; 4],
}

thread_local! {
    static LABEL_CACHE: RefCell<Option<Cache>> = const { RefCell::new(None) };
}

/// Composites antialiased Segoe UI labels over existing premultiplied BGRA.
/// One small native scratch surface produces reusable masks; no HWND is needed.
/// Invalid dimensions and unavailable native resources leave pixels unchanged.
pub fn paint(
    pixels: &mut [u32],
    width: u32,
    height: u32,
    scale: f32,
    color: Color,
    labels: &[Label],
) {
    let Some(len) = (width as usize).checked_mul(height as usize) else {
        return;
    };
    if width == 0
        || height == 0
        || width > i32::MAX as u32
        || height > i32::MAX as u32
        || len > pixels.len()
        || !scale.is_finite()
        || !(0.0..=32.0).contains(&scale)
        || scale == 0.0
        || labels.is_empty()
    {
        return;
    }
    LABEL_CACHE.with(|cell| {
        let Ok(mut cached) = cell.try_borrow_mut() else {
            return;
        };
        if cached.as_ref().is_none_or(|cache| cache.scale != scale) {
            *cached = Cache::new(scale);
        }
        let Some(cache) = cached.as_mut() else {
            return;
        };
        for label in labels {
            if label.text.is_empty() || label.bounds[2] <= 0 || label.bounds[3] <= 0 {
                continue;
            }
            let Some(x) = scaled(label.bounds[0], scale) else {
                continue;
            };
            let Some(y) = scaled(label.bounds[1], scale) else {
                continue;
            };
            let Some(slot_width) = scaled(label.bounds[2], scale).filter(|value| *value > 0) else {
                continue;
            };
            let Some(slot_height) = scaled(label.bounds[3], scale).filter(|value| *value > 0)
            else {
                continue;
            };
            let Some(glyph) = cache.glyph(label.text, slot_width, slot_height) else {
                continue;
            };
            // An unexpectedly long string is allowed to extend symmetrically
            // beyond its slot instead of being silently clipped or shortened.
            let origin_x = i64::from(x) + i64::from(slot_width - glyph.width) / 2;
            let origin_y = i64::from(y) + i64::from(slot_height - glyph.height) / 2;
            for row in 0..glyph.height {
                let target_y = origin_y + i64::from(row);
                if !(0..i64::from(height)).contains(&target_y) {
                    continue;
                }
                for column in 0..glyph.width {
                    let target_x = origin_x + i64::from(column);
                    if !(0..i64::from(width)).contains(&target_x) {
                        continue;
                    }
                    let coverage = glyph.alpha[(row * glyph.width + column) as usize];
                    if coverage != 0 {
                        let offset = target_y as usize * width as usize + target_x as usize;
                        pixels[offset] = over(pixels[offset], color, coverage);
                    }
                }
            }
        }
    });
}

fn scaled(value: i32, scale: f32) -> Option<i32> {
    let value = (f64::from(value) * f64::from(scale)).round();
    (value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX)).then_some(value as i32)
}

fn over(destination: u32, color: Color, coverage: u8) -> u32 {
    let alpha = u32::from(coverage);
    let inverse = 255 - alpha;
    let mut result = (alpha + (((destination >> 24) * inverse + 127) / 255)) << 24;
    for (shift, channel) in [(0, color.b), (8, color.g), (16, color.r)] {
        let previous = (destination >> shift) & 255;
        result |= ((u32::from(channel) * alpha + previous * inverse + 127) / 255) << shift;
    }
    result
}

struct Glyph {
    text: &'static str,
    slot_width: i32,
    slot_height: i32,
    width: i32,
    height: i32,
    alpha: Vec<u8>,
}

struct Cache {
    scale: f32,
    font: HFONT,
    scratch: Surface,
    glyphs: Vec<Glyph>,
}

impl Cache {
    fn new(scale: f32) -> Option<Self> {
        let face: Vec<u16> = "Segoe UI".encode_utf16().chain(Some(0)).collect();
        let font = unsafe {
            CreateFontW(
                -((FONT_SIZE_DIP * scale).round() as i32).max(1),
                0,
                0,
                0,
                FW_MEDIUM as i32,
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
            return None;
        }
        let Some(scratch) = Surface::new(1, 1) else {
            unsafe { DeleteObject(font) };
            return None;
        };
        Some(Self {
            scale,
            font,
            scratch,
            glyphs: Vec::new(),
        })
    }

    fn glyph(&mut self, text: &'static str, slot_width: i32, slot_height: i32) -> Option<&Glyph> {
        if let Some(index) = self.glyphs.iter().position(|glyph| {
            glyph.text == text && glyph.slot_width == slot_width && glyph.slot_height == slot_height
        }) {
            return self.glyphs.get(index);
        }
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let length = i32::try_from(utf16.len()).ok()?;
        let mut extent = SIZE::default();
        let measured = unsafe {
            let previous = SelectObject(self.scratch.dc, self.font);
            if previous.is_null() || previous as isize == -1 {
                return None;
            }
            let measured =
                GetTextExtentPoint32W(self.scratch.dc, utf16.as_ptr(), length, &mut extent);
            SelectObject(self.scratch.dc, previous);
            measured
        };
        if measured == 0 {
            return None;
        }
        let width = slot_width.max(extent.cx.checked_add(4)?);
        let height = slot_height.max(extent.cy.checked_add(2)?);
        let count = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if count == 0 || count > MAX_MASK_PIXELS {
            return None;
        }
        if self.scratch.width < width || self.scratch.height < height {
            self.scratch = Surface::new(width, height)?;
        }
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        };
        unsafe {
            ptr::write_bytes(self.scratch.pixels, 0, self.scratch.len);
            let previous = SelectObject(self.scratch.dc, self.font);
            if previous.is_null() || previous as isize == -1 {
                return None;
            }
            SetBkMode(self.scratch.dc, TRANSPARENT as i32);
            SetTextColor(self.scratch.dc, 0x00ff_ffff);
            DrawTextW(
                self.scratch.dc,
                utf16.as_ptr(),
                length,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(self.scratch.dc, previous);
            GdiFlush();
        }
        let mut alpha = Vec::with_capacity(count);
        let source = unsafe { std::slice::from_raw_parts(self.scratch.pixels, self.scratch.len) };
        for row in 0..height {
            for column in 0..width {
                let pixel = source[(row * self.scratch.width + column) as usize];
                alpha.push(
                    (pixel & 255)
                        .max((pixel >> 8) & 255)
                        .max((pixel >> 16) & 255) as u8,
                );
            }
        }
        if self.glyphs.len() >= MAX_CACHED_LABELS {
            self.glyphs.clear();
        }
        self.glyphs.push(Glyph {
            text,
            slot_width,
            slot_height,
            width,
            height,
            alpha,
        });
        self.glyphs.last()
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.font) };
    }
}

struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
    pixels: *mut u32,
    len: usize,
    width: i32,
    height: i32,
}

impl Surface {
    fn new(width: i32, height: i32) -> Option<Self> {
        let len = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?;
        if width <= 0 || height <= 0 || len > MAX_MASK_PIXELS {
            return None;
        }
        unsafe {
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
            let bitmap =
                CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut pixels, ptr::null_mut(), 0);
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
                width,
                height,
            })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_coverage_preserves_premultiplied_bgra() {
        let color = Color::rgb(240, 100, 20);
        assert_eq!(over(0, color, 255), 0xfff0_6414);
        assert_eq!(over(0, color, 128), 0x8078_320a);
        assert_eq!(over(0x8040_2010, color, 0), 0x8040_2010);
        assert_eq!(over(0xff40_2010, color, 128) >> 24, 255);
    }

    #[test]
    fn native_labels_render_antialiased_complete_strings_and_reuse_masks() {
        let labels = [
            Label {
                text: "Object eraser",
                bounds: [5, 0, 96, 28],
            },
            Label {
                text: "Settings",
                bounds: [105, 0, 90, 28],
            },
        ];
        let mut first = vec![0; 200 * 32];
        paint(&mut first, 200, 32, 1.0, Color::rgb(240, 240, 240), &labels);
        assert!(first.iter().any(|pixel| pixel >> 24 == 255));
        assert!(first.iter().any(|pixel| (1..255).contains(&(pixel >> 24))));
        for pixel in &first {
            let alpha = pixel >> 24;
            assert!(
                pixel & 255 <= alpha && (pixel >> 8) & 255 <= alpha && (pixel >> 16) & 255 <= alpha
            );
        }
        let mut repeated = vec![0; 200 * 32];
        paint(
            &mut repeated,
            200,
            32,
            1.0,
            Color::rgb(240, 240, 240),
            &labels,
        );
        assert_eq!(first, repeated);
        LABEL_CACHE.with(|cell| assert_eq!(cell.borrow().as_ref().unwrap().glyphs.len(), 2));
    }

    #[test]
    fn toolbar_captions_stay_legible_without_overlapping_at_common_dpi() {
        let captions = [
            "Cursor", "Pen", "Eraser", "Object", "Undo", "Clear", "Color", "Settings", "Hide",
            "Exit", "Shapes", "Ruler", "Fill",
        ];
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut cache = Cache::new(scale).unwrap();
            let slot_width = scaled(38, scale).unwrap();
            let slot_height = scaled(16, scale).unwrap();
            let mut spans = Vec::new();
            for text in captions {
                let glyph = cache.glyph(text, slot_width, slot_height).unwrap();
                let visible = |column| {
                    (0..glyph.height)
                        .any(|row| glyph.alpha[(row * glyph.width + column) as usize] != 0)
                };
                let left = (0..glyph.width).find(|&x| visible(x)).unwrap();
                let right = (0..glyph.width).rfind(|&x| visible(x)).unwrap();
                let origin = (slot_width - glyph.width) / 2;
                spans.push((origin + left, origin + right));
                assert!(
                    glyph
                        .alpha
                        .chunks_exact(glyph.width as usize)
                        .filter(|row| { row.iter().any(|&coverage| coverage >= 128) })
                        .count()
                        >= scaled(8, scale).unwrap() as usize,
                    "{text} is too small at scale {scale}"
                );
            }
            let spacing = (40.0 * scale).floor() as i32;
            for (index, &(_, right)) in spans.iter().enumerate() {
                for (neighbor, &(left, _)) in spans.iter().enumerate() {
                    if index != neighbor {
                        assert!(
                            right + 1 < spacing + left,
                            "Captions {} and {} touch at scale {scale}",
                            captions[index],
                            captions[neighbor]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn label_clipping_and_invalid_inputs_preserve_buffer_bounds() {
        let labels = [Label {
            text: "Undo",
            bounds: [-20, -4, 90, 28],
        }];
        let mut pixels = vec![0xff10_2030; 64 * 32 + 8];
        paint(&mut pixels, 64, 32, 1.5, Color::rgb(240, 240, 240), &labels);
        assert_eq!(&pixels[64 * 32..], &[0xff10_2030; 8]);
        assert!(pixels[..64 * 32].iter().any(|pixel| *pixel != 0xff10_2030));
        let before = pixels.clone();
        for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            paint(&mut pixels, 64, 32, scale, Color::default(), &labels);
        }
        paint(&mut pixels, 65, 99, 1.0, Color::default(), &labels);
        assert_eq!(pixels, before);
    }
}
