//! Small, antialiased native pen cursors with a stable drawing-tip hotspot.

use std::{mem::size_of, ptr};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, Stroke, Transform};
use windows_sys::Win32::{Graphics::Gdi::*, UI::WindowsAndMessaging::*};

/// Creates an owned cursor. `size` is the nib's length in logical pixels (15 is
/// the normal size at 96 DPI); `scale` converts it to physical screen pixels.
/// The caller must destroy a successful result with `DestroyCursor`.
/// Invalid input or a native allocation failure returns a null handle.
pub fn create(scale: f32, size: u8) -> HCURSOR {
    let Some(image) = rasterize(scale, size) else {
        return ptr::null_mut();
    };
    unsafe { native_cursor(&image) }
}

struct Image {
    pixels: Pixmap,
    hotspot: [u32; 2],
}

fn rasterize(scale: f32, size: u8) -> Option<Image> {
    if !scale.is_finite() || scale <= 0.0 || size == 0 {
        return None;
    }
    let scale = scale.clamp(0.5, 4.0);
    let length = f32::from(size.clamp(8, 32)) * scale;
    let width = length * 0.44;
    let padding = (2.0 * scale).ceil() as u32;
    let side =
        ((length + width * 0.5) * std::f32::consts::FRAC_1_SQRT_2).ceil() as u32 + padding * 2 + 2;
    let hotspot = [padding, side - padding - 1];
    let tip = [hotspot[0] as f32 + 0.5, hotspot[1] as f32 + 0.5];
    // Area sampling retains thin slit detail and smooth curves at 100% and
    // fractional display scales. This scratch image exists only while creating
    // a frame; the host caches the finished native cursor.
    const SAMPLES: u32 = 3;
    let mut artwork = Pixmap::new(side * SAMPLES, side * SAMPLES)?;
    draw_nib(
        &mut artwork,
        [tip[0] * SAMPLES as f32, tip[1] * SAMPLES as f32],
        length * SAMPLES as f32,
        scale * SAMPLES as f32,
    )?;
    let mut pixels = Pixmap::new(side, side)?;
    let source = artwork.data();
    let target = pixels.data_mut();
    for y in 0..side {
        for x in 0..side {
            let mut sum = [0_u32; 4];
            for row in 0..SAMPLES {
                for column in 0..SAMPLES {
                    let index = (((y * SAMPLES + row) * side * SAMPLES + x * SAMPLES + column) * 4)
                        as usize;
                    for channel in 0..4 {
                        sum[channel] += u32::from(source[index + channel]);
                    }
                }
            }
            let index = ((y * side + x) * 4) as usize;
            for channel in 0..4 {
                target[index + channel] =
                    ((sum[channel] + SAMPLES * SAMPLES / 2) / (SAMPLES * SAMPLES)) as u8;
            }
        }
    }
    Some(Image { pixels, hotspot })
}

fn draw_nib(pixels: &mut Pixmap, tip: [f32; 2], length: f32, scale: f32) -> Option<()> {
    let point = |along: f32, across: f32| {
        let unit = std::f32::consts::FRAC_1_SQRT_2;
        [
            tip[0] + (along + across) * length * unit,
            tip[1] + (across - along) * length * unit,
        ]
    };
    let mut path = PathBuilder::new();
    path.move_to(tip[0], tip[1]);
    let control = point(0.32, -0.04);
    let end = point(0.58, -0.22);
    path.quad_to(control[0], control[1], end[0], end[1]);
    let control = point(0.65, -0.25);
    let end = point(0.73, -0.20);
    path.quad_to(control[0], control[1], end[0], end[1]);
    let [x, y] = point(0.96, -0.12);
    path.line_to(x, y);
    let control = point(1.0, -0.11);
    let end = point(1.0, -0.07);
    path.quad_to(control[0], control[1], end[0], end[1]);
    let [x, y] = point(1.0, 0.07);
    path.line_to(x, y);
    let control = point(1.0, 0.11);
    let end = point(0.96, 0.12);
    path.quad_to(control[0], control[1], end[0], end[1]);
    let [x, y] = point(0.73, 0.20);
    path.line_to(x, y);
    let control = point(0.65, 0.25);
    let end = point(0.58, 0.22);
    path.quad_to(control[0], control[1], end[0], end[1]);
    let control = point(0.32, 0.04);
    path.quad_to(control[0], control[1], tip[0], tip[1]);
    path.close();
    let path = path.finish()?;

    // A narrow white halo surrounds a dark navy contour. The blue silhouette
    // remains readable on both white pages and dark application content.
    stroke(pixels, &path, [255, 255, 255, 255], 2.7 * scale);
    pixels.fill_path(
        &path,
        &paint([44, 122, 181, 255]),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    let mut facet = PathBuilder::new();
    let [x, y] = point(0.26, -0.01);
    facet.move_to(x, y);
    let control = point(0.49, -0.11);
    let end = point(0.63, -0.16);
    facet.quad_to(control[0], control[1], end[0], end[1]);
    let [x, y] = point(0.88, -0.08);
    facet.line_to(x, y);
    let [x, y] = point(0.65, -0.03);
    facet.line_to(x, y);
    facet.close();
    pixels.fill_path(
        &facet.finish()?,
        &paint([144, 219, 247, 230]),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    stroke(pixels, &path, [20, 43, 73, 255], 1.05 * scale);

    let mut slit = PathBuilder::new();
    let [x, y] = point(0.12, 0.0);
    slit.move_to(x, y);
    let hole = point(0.59, 0.0);
    slit.line_to(hole[0], hole[1]);
    stroke(pixels, &slit.finish()?, [231, 249, 255, 255], 0.9 * scale);
    pixels.fill_path(
        &PathBuilder::from_circle(hole[0], hole[1], 0.78 * scale)?,
        &paint([236, 250, 255, 255]),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    // Re-darken the terminal point after the slit: the native hotspot always
    // coincides with a visible nib tip rather than a transparent cutout.
    pixels.fill_path(
        &PathBuilder::from_circle(tip[0], tip[1], 0.76 * scale)?,
        &paint([20, 43, 73, 255]),
        FillRule::Winding,
        Transform::identity(),
        None,
    );
    Some(())
}

fn paint([r, g, b, a]: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    // Reversed channels let tiny-skia write premultiplied BGRA directly.
    paint.set_color_rgba8(b, g, r, a);
    paint.anti_alias = true;
    paint
}

fn stroke(pixels: &mut Pixmap, path: &Path, color: [u8; 4], width: f32) {
    pixels.stroke_path(
        path,
        &paint(color),
        &Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
}

struct Bitmap(HBITMAP);

impl Drop for Bitmap {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0) };
    }
}

unsafe fn native_cursor(image: &Image) -> HCURSOR {
    let side = image.pixels.width();
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: side as i32,
            biHeight: -(side as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = ptr::null_mut();
    let color = Bitmap(CreateDIBSection(
        ptr::null_mut(),
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        ptr::null_mut(),
        0,
    ));
    if color.0.is_null() || bits.is_null() {
        return ptr::null_mut();
    }
    // The native cursor ingestion path applies alpha to the supplied colors.
    // Convert the area-sampled premultiplied raster to straight BGRA here so
    // translucent edges are not darkened twice. Native DrawIconEx regression
    // tests verify the resulting cursor against source-over on both backdrops.
    let destination = std::slice::from_raw_parts_mut(bits.cast::<u8>(), image.pixels.data().len());
    for (source, destination) in image
        .pixels
        .data()
        .as_chunks::<4>()
        .0
        .iter()
        .zip(destination.as_chunks_mut::<4>().0)
    {
        let alpha = u32::from(source[3]);
        for channel in 0..3 {
            destination[channel] = (u32::from(source[channel]) * 255 + alpha / 2)
                .checked_div(alpha)
                .unwrap_or(0)
                .min(255) as u8;
        }
        destination[3] = source[3];
    }

    // A word-aligned monochrome AND mask also defines transparent pixels for
    // native consumers which fall back to the cursor's legacy mask.
    let stride = side.div_ceil(16) as usize * 2;
    let mut mask_bits = vec![0_u8; stride * side as usize];
    for y in 0..side as usize {
        for x in 0..side as usize {
            if image.pixels.data()[(y * side as usize + x) * 4 + 3] == 0 {
                mask_bits[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
    }
    let mask = Bitmap(CreateBitmap(
        side as i32,
        side as i32,
        1,
        1,
        mask_bits.as_ptr().cast(),
    ));
    if mask.0.is_null() {
        return ptr::null_mut();
    }
    // CreateIconIndirect copies both unselected bitmaps; RAII releases them.
    CreateIconIndirect(&ICONINFO {
        fIcon: 0,
        xHotspot: image.hotspot[0],
        yHotspot: image.hotspot[1],
        hbmMask: mask.0,
        hbmColor: color.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tip_stays_on_an_opaque_dark_pixel_and_edges_are_antialiased() {
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0, 4.0] {
            for size in [10, 12, 13, 14, 15, 16, 22, 28, 30] {
                let image = rasterize(scale, size).unwrap();
                let side = image.pixels.width() as usize;
                let tip = (image.hotspot[1] as usize * side + image.hotspot[0] as usize) * 4;
                let tip = &image.pixels.data()[tip..tip + 4];
                assert!(tip[3] >= 230 && tip[0] < 90, "scale={scale}, size={size}");
                let pixels = image.pixels.data().as_chunks::<4>().0;
                assert!(pixels.iter().any(|p| p[3] > 0 && p[3] < 255));
                assert!(pixels.iter().all(|p| p[..3].iter().all(|&c| c <= p[3])));
                assert!(pixels[..side].iter().all(|p| p[3] == 0));
                assert!(pixels[(side - 1) * side..].iter().all(|p| p[3] == 0));
                assert!(
                    (0..side).all(|y| {
                        pixels[y * side][3] == 0 && pixels[y * side + side - 1][3] == 0
                    })
                );
            }
        }
    }

    #[test]
    fn rejects_invalid_inputs_and_bounds_large_requests() {
        for scale in [f32::NAN, f32::INFINITY, -1.0, 0.0] {
            assert!(create(scale, 12).is_null());
        }
        assert!(create(1.0, 0).is_null());
        assert!(rasterize(99.0, 255).unwrap().pixels.width() <= 132);
    }

    #[test]
    fn native_cursor_owns_its_bitmaps_and_retains_tip_hotspot() {
        for (scale, size) in [(1.0, 15), (1.25, 15), (2.0, 15), (2.0, 30), (4.0, 30)] {
            let expected = rasterize(scale, size).unwrap();
            let cursor = create(scale, size);
            assert!(!cursor.is_null());
            unsafe {
                let mut info = ICONINFO::default();
                assert_ne!(GetIconInfo(cursor, &mut info), 0);
                let color = Bitmap(info.hbmColor);
                let _mask = Bitmap(info.hbmMask);
                assert_eq!(info.fIcon, 0);
                assert_eq!([info.xHotspot, info.yHotspot], expected.hotspot);
                let mut bitmap = BITMAP::default();
                assert_ne!(
                    GetObjectW(
                        color.0,
                        size_of::<BITMAP>() as i32,
                        (&mut bitmap as *mut BITMAP).cast()
                    ),
                    0
                );
                assert_eq!(bitmap.bmWidth, expected.pixels.width() as i32);
                assert_eq!(bitmap.bmHeight, expected.pixels.height() as i32);
                assert_ne!(DestroyCursor(cursor), 0);
            }
        }
    }

    struct Cursor(HCURSOR);

    impl Drop for Cursor {
        fn drop(&mut self) {
            unsafe { DestroyCursor(self.0) };
        }
    }

    struct Dc(HDC);

    impl Drop for Dc {
        fn drop(&mut self) {
            unsafe { DeleteDC(self.0) };
        }
    }

    struct Selection(HDC, HGDIOBJ);

    impl Drop for Selection {
        fn drop(&mut self) {
            unsafe { SelectObject(self.0, self.1) };
        }
    }

    // Renders the actual native cursor without displaying it or changing input.
    unsafe fn native_pixels(image: &Image, background: [u8; 3]) -> Vec<u8> {
        let cursor = Cursor(native_cursor(image));
        assert!(!cursor.0.is_null());
        let dc = Dc(CreateCompatibleDC(ptr::null_mut()));
        assert!(!dc.0.is_null());
        let side = image.pixels.width();
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: side as i32,
                biHeight: -(side as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = ptr::null_mut();
        let bitmap = Bitmap(CreateDIBSection(
            dc.0,
            &info,
            DIB_RGB_COLORS,
            &mut bits,
            ptr::null_mut(),
            0,
        ));
        assert!(!bitmap.0.is_null() && !bits.is_null());
        let previous = SelectObject(dc.0, bitmap.0);
        let _selection = Selection(dc.0, previous);
        let bytes = std::slice::from_raw_parts_mut(bits.cast::<u8>(), (side * side * 4) as usize);
        for pixel in bytes.as_chunks_mut::<4>().0 {
            pixel.copy_from_slice(&[background[0], background[1], background[2], 255]);
        }
        assert_ne!(
            DrawIconEx(
                dc.0,
                0,
                0,
                cursor.0,
                side as i32,
                side as i32,
                0,
                ptr::null_mut(),
                DI_NORMAL
            ),
            0
        );
        GdiFlush();
        bytes.to_vec()
    }

    #[test]
    fn native_nib_matches_premultiplied_composition_on_light_and_dark_content() {
        for scale in [1.0, 1.25, 2.0] {
            for size in [15, 30] {
                let image = rasterize(scale, size).unwrap();
                let blue_pixels = image
                    .pixels
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .filter(|p| p[3] > 230 && i16::from(p[0]) - i16::from(p[2]) > 70)
                    .count();
                assert!(
                    blue_pixels >= 7,
                    "defined blue nib: scale={scale}, size={size}"
                );
                for background in [[250, 250, 250], [30, 24, 18]] {
                    let native = unsafe { native_pixels(&image, background) };
                    for (source, result) in image
                        .pixels
                        .data()
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .zip(native.as_chunks::<4>().0)
                    {
                        for channel in 0..3 {
                            let expected = u32::from(source[channel])
                                + (u32::from(background[channel]) * (255 - u32::from(source[3]))
                                    + 127)
                                    / 255;
                            assert!(
                                (i32::from(result[channel]) - expected as i32).abs() <= 1,
                                "native alpha/color: scale={scale}, size={size}, source={source:?}, result={result:?}, background={background:?}, channel={channel}, expected={expected}"
                            );
                        }
                    }
                }
            }
        }
    }
}
