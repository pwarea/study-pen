//! A retained, tiled raster surface for the native layered ink window.
//!
//! Only one desktop-sized bitmap is kept. A small scratch tile handles
//! antialiasing; pointer movement redraws only the changed rectangle.

use crate::ink::{Ink, Stroke};
use std::{ffi::c_void, marker::PhantomData, mem::size_of, ptr, rc::Rc};
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    Transform,
};
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT, SIZE},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
        CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
        HGDIOBJ, SelectObject,
    },
    UI::WindowsAndMessaging::{ULW_ALPHA, UPDATELAYEREDWINDOWINFO, UpdateLayeredWindowIndirect},
};

const TILE: i32 = 256;
const PADDING: i32 = 2;
const SCRATCH: u32 = (TILE + PADDING * 2) as u32;

/// An exclusive-right/bottom rectangle in overlay-local physical pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    pub fn around(x: f32, y: f32, radius: f32) -> Self {
        Self {
            left: (x - radius).floor() as i32,
            top: (y - radius).floor() as i32,
            right: (x + radius).ceil() as i32 + 1,
            bottom: (y + radius).ceil() as i32 + 1,
        }
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    fn intersects(self, other: Self) -> bool {
        self.left < other.right
            && self.right > other.left
            && self.top < other.bottom
            && self.bottom > other.top
    }

    fn clamp(self, width: i32, height: i32) -> Option<Self> {
        let result = Self {
            left: self.left.clamp(0, width),
            top: self.top.clamp(0, height),
            right: self.right.clamp(0, width),
            bottom: self.bottom.clamp(0, height),
        };
        (result.left < result.right && result.top < result.bottom).then_some(result)
    }
}

struct PreparedStroke {
    bounds: Rect,
    path: Path,
    paint: Paint<'static>,
    width: f32,
    dot: bool,
    fill: Option<Paint<'static>>,
    erasures: Vec<PreparedErase>,
}

struct PreparedErase {
    bounds: Rect,
    path: Path,
    width: f32,
    dot: bool,
}

impl PreparedStroke {
    fn prepare(stroke: &Stroke, dirty: Rect, optimize: bool) -> Option<Self> {
        let first = stroke.points.first()?;
        let cached = stroke.bounds();
        let bounds = Rect::new(
            (cached.min_x - 2.0).floor() as i32,
            (cached.min_y - 2.0).floor() as i32,
            (cached.max_x + 2.0).ceil() as i32 + 1,
            (cached.max_y + 2.0).ceil() as i32 + 1,
        );
        if !bounds.intersects(dirty) {
            return None;
        }
        let mut builder = PathBuilder::new();
        let dot = stroke.points.len() == 1;
        if dot {
            builder.push_circle(first.x, first.y, stroke.width * 0.5);
        } else if optimize && !stroke.closed {
            // Keep complete line segments rather than clipping their endpoints:
            // this preserves the exact joins and numerical geometry of the
            // original stroke. Extra radius and neighboring segments place any
            // new subpath caps safely outside the visible dirty rectangle.
            let margin = stroke.width * 0.5 + 3.0;
            let left = dirty.left as f32 - margin;
            let top = dirty.top as f32 - margin;
            let right = dirty.right as f32 + margin;
            let bottom = dirty.bottom as f32 + margin;
            let intersects = |index: usize| {
                let a = stroke.points[index];
                let b = stroke.points[index + 1];
                a.x.min(b.x) <= right
                    && a.x.max(b.x) >= left
                    && a.y.min(b.y) <= bottom
                    && a.y.max(b.y) >= top
            };
            let mut before = false;
            let mut current = intersects(0);
            let mut in_run = false;
            for index in 0..stroke.points.len() - 1 {
                let after = index + 2 < stroke.points.len() && intersects(index + 1);
                if before || current || after {
                    let a = stroke.points[index];
                    let b = stroke.points[index + 1];
                    if !in_run {
                        builder.move_to(a.x, a.y);
                    }
                    builder.line_to(b.x, b.y);
                    in_run = true;
                } else {
                    in_run = false;
                }
                before = current;
                current = after;
            }
        } else {
            builder.move_to(first.x, first.y);
            for point in &stroke.points[1..] {
                builder.line_to(point.x, point.y);
            }
            if stroke.closed {
                builder.close();
            }
        }
        let mut paint = Paint::default();
        // GDI expects BGRA; swapping here avoids a second full-screen buffer
        // or a channel conversion while copying the finished tile.
        paint.set_color_rgba8(stroke.color.b, stroke.color.g, stroke.color.r, 255);
        paint.anti_alias = true;
        let fill = stroke.fill.map(|color| {
            let mut paint = Paint::default();
            paint.set_color_rgba8(color.b, color.g, color.r, 255);
            paint.anti_alias = true;
            paint
        });
        let erasures = stroke
            .erasures
            .iter()
            .filter_map(|cut| {
                let bounds = Rect::around(cut.from.x, cut.from.y, cut.radius + 2.0)
                    .union(Rect::around(cut.to.x, cut.to.y, cut.radius + 2.0));
                if !bounds.intersects(dirty) {
                    return None;
                }
                let mut path = PathBuilder::new();
                let dot = cut.from == cut.to;
                if dot {
                    path.push_circle(cut.from.x, cut.from.y, cut.radius);
                } else {
                    path.move_to(cut.from.x, cut.from.y);
                    path.line_to(cut.to.x, cut.to.y);
                }
                Some(PreparedErase {
                    bounds,
                    path: path.finish()?,
                    width: cut.radius * 2.0,
                    dot,
                })
            })
            .collect();
        Some(Self {
            bounds,
            path: builder.finish()?,
            paint,
            width: stroke.width,
            dot,
            fill,
            erasures,
        })
    }

    fn paint(&self, target: &mut Pixmap, transform: Transform) {
        if let Some(fill) = &self.fill {
            target.fill_path(&self.path, fill, FillRule::Winding, transform, None);
        }
        paint_path(
            target,
            &self.path,
            &self.paint,
            self.width,
            self.dot,
            transform,
        );
    }
}

fn paint_path(
    target: &mut Pixmap,
    path: &Path,
    paint: &Paint<'_>,
    width: f32,
    dot: bool,
    transform: Transform,
) {
    if dot {
        target.fill_path(path, paint, FillRule::Winding, transform, None);
    } else {
        let style = tiny_skia::Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Default::default()
        };
        target.stroke_path(path, paint, &style, transform, None);
    }
}

/// A main-thread-only layered-window backing surface.
pub struct Renderer {
    hwnd: HWND,
    origin: POINT,
    width: i32,
    height: i32,
    dc: HDC,
    bitmap: HBITMAP,
    old_bitmap: HGDIOBJ,
    bits: *mut u8,
    scratch: Pixmap,
    object_scratch: Option<Pixmap>,
    interactive: Option<bool>,
    _main_thread: PhantomData<Rc<()>>,
}

impl Renderer {
    /// The caller must supply a live WS_EX_LAYERED window on this thread.
    pub unsafe fn new(
        hwnd: HWND,
        origin_x: i32,
        origin_y: i32,
        width: i32,
        height: i32,
    ) -> Result<Self, String> {
        let byte_count = usize::try_from(width)
            .ok()
            .and_then(|w| usize::try_from(height).ok().and_then(|h| w.checked_mul(h)))
            .and_then(|pixels| pixels.checked_mul(4))
            .filter(|bytes| *bytes > 0 && *bytes <= 1024 * 1024 * 1024)
            .ok_or_else(|| "The desktop is too large to allocate an ink surface.".to_owned())?;
        let scratch = Pixmap::new(SCRATCH, SCRATCH)
            .ok_or_else(|| "Could not allocate the ink render tile.".to_owned())?;
        let dc = unsafe { CreateCompatibleDC(ptr::null_mut()) };
        if dc.is_null() {
            return Err(native_error("Could not create the ink device context"));
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
        let mut bits: *mut c_void = ptr::null_mut();
        let bitmap =
            unsafe { CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0) };
        if bitmap.is_null() || bits.is_null() {
            let error = native_error("Could not allocate the ink surface");
            if !bitmap.is_null() {
                unsafe { DeleteObject(bitmap) };
            }
            unsafe { DeleteDC(dc) };
            return Err(error);
        }
        let old_bitmap = unsafe { SelectObject(dc, bitmap) };
        if old_bitmap.is_null() || old_bitmap as isize == -1 {
            let error = native_error("Could not select the ink surface");
            unsafe {
                DeleteObject(bitmap);
                DeleteDC(dc);
            }
            return Err(error);
        }
        unsafe { ptr::write_bytes(bits.cast::<u8>(), 0, byte_count) };
        Ok(Self {
            hwnd,
            origin: POINT {
                x: origin_x,
                y: origin_y,
            },
            width,
            height,
            dc,
            bitmap,
            old_bitmap,
            bits: bits.cast(),
            scratch,
            object_scratch: None,
            interactive: None,
            _main_thread: PhantomData,
        })
    }

    /// Repaints a region and presents its pixels. Mode changes redraw the full
    /// surface so ink-free pixels consistently capture or pass mouse input.
    pub fn render(
        &mut self,
        ink: &Ink,
        interactive: bool,
        dirty: Option<Rect>,
    ) -> Result<(), String> {
        let full_redraw = dirty.is_none() || self.interactive != Some(interactive);
        let full = Rect::new(0, 0, self.width, self.height);
        let Some(dirty) = (if full_redraw { Some(full) } else { dirty })
            .and_then(|rect| rect.clamp(self.width, self.height))
        else {
            return Ok(());
        };
        let prepared: Vec<_> = ink
            .strokes()
            .iter()
            .chain(ink.active_stroke())
            .filter_map(|stroke| PreparedStroke::prepare(stroke, dirty, !full_redraw))
            .collect();
        let background = tiny_skia::Color::from_rgba8(0, 0, 0, if interactive { 1 } else { 0 });
        // A stable tile origin also keeps antialias coverage identical between
        // full and partial updates, including very long sparse segments.
        let mut top = dirty.top / TILE * TILE;
        while top < dirty.bottom {
            let tile_height = TILE.min(self.height - top);
            let copy_top = top.max(dirty.top);
            let copy_bottom = (top + tile_height).min(dirty.bottom);
            let mut left = dirty.left / TILE * TILE;
            while left < dirty.right {
                let tile_width = TILE.min(self.width - left);
                let copy_left = left.max(dirty.left);
                let copy_right = (left + tile_width).min(dirty.right);
                let bounds = Rect::new(
                    left - PADDING,
                    top - PADDING,
                    left + tile_width + PADDING,
                    top + tile_height + PADDING,
                );
                self.scratch.fill(background);
                let transform =
                    Transform::from_translate((PADDING - left) as f32, (PADDING - top) as f32);
                for stroke in &prepared {
                    if !stroke.bounds.intersects(bounds) {
                        continue;
                    }
                    if stroke.erasures.is_empty() {
                        stroke.paint(&mut self.scratch, transform);
                        continue;
                    }
                    // Erasing a filled shape must reveal earlier scene items,
                    // rather than clearing their pixels from the shared tile.
                    if self.object_scratch.is_none() {
                        self.object_scratch =
                            Some(Pixmap::new(SCRATCH, SCRATCH).ok_or_else(|| {
                                "Could not allocate the shape render tile.".to_owned()
                            })?);
                    }
                    let object = self.object_scratch.as_mut().expect("The shape tile exists");
                    object.fill(tiny_skia::Color::TRANSPARENT);
                    stroke.paint(object, transform);
                    let clear = Paint {
                        blend_mode: BlendMode::Clear,
                        anti_alias: true,
                        ..Default::default()
                    };
                    for cut in &stroke.erasures {
                        if cut.bounds.intersects(bounds) {
                            paint_path(object, &cut.path, &clear, cut.width, cut.dot, transform);
                        }
                    }
                    self.scratch.draw_pixmap(
                        0,
                        0,
                        object.as_ref(),
                        &PixmapPaint::default(),
                        Transform::identity(),
                        None,
                    );
                }
                let source = self.scratch.data();
                for y in copy_top..copy_bottom {
                    let src_offset = (((y - top + PADDING) as usize * SCRATCH as usize)
                        + (copy_left - left + PADDING) as usize)
                        * 4;
                    let dst_offset = ((y as usize * self.width as usize) + copy_left as usize) * 4;
                    // DIB memory is owned by this renderer. The checked surface
                    // dimensions and clamped tile bounds keep each row in range.
                    unsafe {
                        ptr::copy_nonoverlapping(
                            source.as_ptr().add(src_offset),
                            self.bits.add(dst_offset),
                            (copy_right - copy_left) as usize * 4,
                        );
                    }
                }
                left += TILE;
            }
            top += TILE;
        }
        let source_origin = POINT { x: 0, y: 0 };
        let size = SIZE {
            cx: self.width,
            cy: self.height,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let native_dirty = RECT {
            left: dirty.left,
            top: dirty.top,
            right: dirty.right,
            bottom: dirty.bottom,
        };
        let update = UPDATELAYEREDWINDOWINFO {
            cbSize: size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: ptr::null_mut(),
            pptDst: &self.origin,
            psize: &size,
            hdcSrc: self.dc,
            pptSrc: &source_origin,
            crKey: 0,
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: if full_redraw {
                ptr::null()
            } else {
                &native_dirty
            },
        };
        if unsafe { UpdateLayeredWindowIndirect(self.hwnd, &update) } == 0 {
            return Err(native_error("Could not present the ink surface"));
        }
        self.interactive = Some(interactive);
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old_bitmap);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

fn native_error(message: &str) -> String {
    format!("{message}: {}", std::io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ink::{Color, Point, Tool};
    use windows_sys::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindowVisible, RegisterClassW,
            UnregisterClassW, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
            WS_POPUP,
        },
    };

    struct HiddenWindow {
        hwnd: HWND,
        class_name: Vec<u16>,
    }

    impl HiddenWindow {
        fn new() -> Self {
            static NEXT_CLASS: std::sync::atomic::AtomicUsize =
                std::sync::atomic::AtomicUsize::new(0);
            let id = NEXT_CLASS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let class_name: Vec<u16> = format!("StudyPen.Renderer.Test.{id}")
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
                    "{}",
                    native_error("Register test window")
                );
                let hwnd = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    class_name.as_ptr(),
                    ptr::null(),
                    WS_POPUP,
                    0,
                    0,
                    512,
                    256,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    instance,
                    ptr::null(),
                );
                if hwnd.is_null() {
                    UnregisterClassW(class_name.as_ptr(), instance);
                    panic!("{}", native_error("Create test window"));
                }
                Self { hwnd, class_name }
            }
        }
    }

    impl Drop for HiddenWindow {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.hwnd);
                UnregisterClassW(self.class_name.as_ptr(), GetModuleHandleW(ptr::null()));
            }
        }
    }

    fn pixel(renderer: &Renderer, x: usize, y: usize) -> [u8; 4] {
        assert!(x < renderer.width as usize && y < renderer.height as usize);
        let offset = (y * renderer.width as usize + x) * 4;
        unsafe {
            let source = std::slice::from_raw_parts(renderer.bits.add(offset), 4);
            [source[0], source[1], source[2], source[3]]
        }
    }

    #[test]
    fn dirty_rectangles_are_clamped_to_the_surface() {
        assert_eq!(
            Rect::new(-10, -20, 400, 300).clamp(100, 80),
            Some(Rect::new(0, 0, 100, 80))
        );
        assert_eq!(Rect::new(200, 0, 300, 30).clamp(100, 80), None);
        assert_eq!(Rect::new(5, 5, 5, 10).clamp(100, 80), None);
    }

    #[test]
    fn round_dot_keeps_its_bounds_and_bgra_color() {
        let mut ink = Ink::new();
        ink.begin(
            crate::ink::Point { x: 20.0, y: 30.0 },
            crate::ink::Tool::Pen,
            crate::ink::Color::rgb(250, 120, 50),
            6.0,
        );
        let stroke = ink.active_stroke().unwrap();
        let prepared = PreparedStroke::prepare(stroke, Rect::new(0, 0, 100, 100), true).unwrap();
        assert!(prepared.dot);
        assert!(prepared.bounds.intersects(Rect::new(18, 28, 22, 32)));
        let mut pixmap = Pixmap::new(100, 100).unwrap();
        pixmap.fill_path(
            &prepared.path,
            &prepared.paint,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        assert_eq!(
            &pixmap.data()[(30 * 100 + 20) * 4..][..4],
            &[50, 120, 250, 255]
        );
        assert!(PreparedStroke::prepare(stroke, Rect::new(60, 60, 80, 80), true).is_none());
    }

    #[test]
    fn hidden_native_surface_preserves_dirty_erase_undo_and_input_alpha() {
        let window = HiddenWindow::new();
        let mut renderer = unsafe { Renderer::new(window.hwnd, 0, 0, 512, 256) }.unwrap();
        let mut ink = Ink::new();
        ink.begin(
            Point::new(220.0, 128.0),
            Tool::Pen,
            Color::rgb(250, 120, 50),
            12.0,
        );
        ink.extend(Point::new(292.0, 128.0));
        ink.end();
        renderer.render(&ink, false, None).unwrap();
        assert_eq!(pixel(&renderer, 0, 0), [0, 0, 0, 0]);
        for x in [245, 255, 256, 257, 267] {
            assert_eq!(pixel(&renderer, x, 128), [50, 120, 250, 255]);
        }

        // A mode change must update even pixels outside the supplied dirty area.
        let dirty = Some(Rect::new(236, 108, 276, 160));
        renderer.render(&ink, true, dirty).unwrap();
        assert_eq!(pixel(&renderer, 0, 0), [0, 0, 0, 1]);

        ink.begin(
            Point::new(256.0, 128.0),
            Tool::Eraser,
            Color::default(),
            20.0,
        );
        ink.end();
        renderer.render(&ink, true, dirty).unwrap();
        for x in [255, 256, 257] {
            assert_eq!(pixel(&renderer, x, 128), [0, 0, 0, 1]);
        }
        // Unchanged pixels outside the partial update retain the original ink.
        assert_eq!(pixel(&renderer, 225, 128), [50, 120, 250, 255]);
        assert_eq!(pixel(&renderer, 285, 128), [50, 120, 250, 255]);

        assert!(ink.undo());
        renderer.render(&ink, true, dirty).unwrap();
        for x in [255, 256, 257] {
            assert_eq!(pixel(&renderer, x, 128), [50, 120, 250, 255]);
        }

        ink.clear();
        renderer.render(&ink, false, dirty).unwrap();
        for (x, y) in [(0, 0), (225, 128), (256, 128), (285, 128)] {
            assert_eq!(pixel(&renderer, x, y), [0, 0, 0, 0]);
        }
        assert!(ink.undo());
        renderer.render(&ink, false, None).unwrap();
        assert_eq!(pixel(&renderer, 256, 128), [50, 120, 250, 255]);
        assert_eq!(pixel(&renderer, 0, 0), [0, 0, 0, 0]);
        assert_eq!(unsafe { IsWindowVisible(window.hwnd) }, 0);
    }

    #[test]
    fn partial_paths_match_full_geometry_at_bends_caps_and_tile_edges() {
        let window = HiddenWindow::new();
        let mut renderer = unsafe { Renderer::new(window.hwnd, 0, 0, 512, 256) }.unwrap();
        let mut ink = Ink::new();
        ink.begin(
            Point::new(-3000.0, 128.0),
            Tool::Pen,
            Color::rgb(235, 90, 130),
            8.25,
        );
        for x in (-2996..4000).step_by(4) {
            ink.extend(Point::new(x as f32, 128.0 + (x as f32 / 48.0).sin() * 90.0));
        }
        ink.end();
        let long_stroke = &ink.strokes()[0];
        let dirty = Rect::new(240, 90, 280, 170);
        let full = PreparedStroke::prepare(long_stroke, dirty, false).unwrap();
        let partial = PreparedStroke::prepare(long_stroke, dirty, true).unwrap();
        assert!(
            partial.path.len() * 10 < full.path.len(),
            "A small dirty rectangle must not tessellate a full long stroke"
        );

        ink.begin(
            Point::new(18.0, 25.0),
            Tool::Pen,
            Color::rgb(90, 170, 235),
            17.5,
        );
        for point in [
            Point::new(494.0, 25.0),
            Point::new(494.0, 65.0),
            Point::new(18.0, 65.0),
            Point::new(18.0, 105.0),
            Point::new(494.0, 105.0),
            Point::new(494.0, 145.0),
            Point::new(18.0, 145.0),
            Point::new(18.0, 185.0),
            Point::new(494.0, 185.0),
            Point::new(494.0, 225.0),
            Point::new(18.0, 225.0),
        ] {
            ink.extend(point);
        }
        ink.end();

        // The segment crosses the canvas although both endpoints are far away.
        ink.begin(
            Point::new(-5000.0, -3000.0),
            Tool::Pen,
            Color::rgb(240, 220, 90),
            5.75,
        );
        ink.extend(Point::new(5500.0, 3300.0));
        ink.end();
        ink.begin(
            Point::new(60.0, 180.0),
            Tool::Pen,
            Color::rgb(80, 220, 150),
            35.25,
        );
        for point in [
            Point::new(246.0, 135.0),
            Point::new(258.0, 95.0),
            Point::new(262.0, 132.0),
            Point::new(440.0, 180.0),
        ] {
            ink.extend(point);
        }
        ink.end();
        ink.begin(
            Point::new(256.0, 244.0),
            Tool::Pen,
            Color::rgb(240, 240, 240),
            13.0,
        );
        ink.extend(Point::new(285.0, 242.0));
        ink.end();
        // Leave a tap active to cover the uncommitted-stroke path as well.
        ink.begin(
            Point::new(256.0, 128.0),
            Tool::Pen,
            Color::rgb(255, 125, 50),
            11.0,
        );

        // None explicitly uses complete original paths, independently of the
        // partial-run selection under test.
        renderer.render(&ink, true, None).unwrap();
        let reference =
            unsafe { std::slice::from_raw_parts(renderer.bits, 512 * 256 * 4).to_vec() };
        for dirty in [
            Rect::new(241, 112, 274, 145),
            Rect::new(0, 0, 80, 80),
            Rect::new(230, 80, 285, 160),
            Rect::new(255, 0, 257, 256),
            Rect::new(5, 125, 507, 130),
            Rect::new(350, 20, 489, 74),
            Rect::new(247, 236, 291, 255),
            Rect::new(95, 37, 390, 225),
            Rect::new(0, 0, 512, 256),
        ] {
            // Poison the destination first: a missing partial repaint cannot
            // accidentally pass by retaining the reference pixels.
            for y in dirty.top..dirty.bottom {
                let offset = (y as usize * 512 + dirty.left as usize) * 4;
                unsafe {
                    ptr::write_bytes(
                        renderer.bits.add(offset),
                        0xa5,
                        (dirty.right - dirty.left) as usize * 4,
                    );
                }
            }
            renderer.render(&ink, true, Some(dirty)).unwrap();
            for y in dirty.top..dirty.bottom {
                for x in dirty.left..dirty.right {
                    let offset = (y as usize * 512 + x as usize) * 4;
                    assert_eq!(
                        pixel(&renderer, x as usize, y as usize),
                        reference[offset..offset + 4],
                        "dirty {dirty:?}, pixel ({x}, {y})"
                    );
                }
            }
        }
        assert_eq!(unsafe { IsWindowVisible(window.hwnd) }, 0);
    }

    #[test]
    fn shape_preview_fill_and_pixel_holes_match_full_and_partial_repaints() {
        let window = HiddenWindow::new();
        let mut renderer = unsafe { Renderer::new(window.hwnd, 0, 0, 512, 256) }.unwrap();
        let mut ink = Ink::new();
        let outline = Color::rgb(25, 95, 230);
        let fill = Color::rgb(240, 90, 45);
        ink.begin_shape(
            Point::new(30.0, 30.0),
            crate::ink::ShapeKind::Rectangle,
            outline,
            6.0,
        );
        ink.extend(Point::new(480.0, 220.0));
        renderer.render(&ink, false, None).unwrap();
        assert_eq!(pixel(&renderer, 30, 100), [230, 95, 25, 255]);
        assert_eq!(pixel(&renderer, 100, 100), [0, 0, 0, 0]);
        ink.end();
        ink.fill_at(Point::new(100.0, 100.0), fill).unwrap();
        renderer.render(&ink, false, None).unwrap();
        assert_eq!(pixel(&renderer, 100, 100), [45, 90, 240, 255]);
        assert_eq!(pixel(&renderer, 30, 100), [230, 95, 25, 255]);

        ink.begin(Point::new(252.0, 128.0), Tool::Eraser, fill, 24.0);
        ink.extend(Point::new(290.0, 128.0));
        ink.end();
        let dirty = Rect::new(235, 110, 308, 146);
        renderer.render(&ink, false, Some(dirty)).unwrap();
        assert_eq!(pixel(&renderer, 256, 128), [0, 0, 0, 0]);
        assert_eq!(pixel(&renderer, 285, 128), [0, 0, 0, 0]);
        assert_eq!(pixel(&renderer, 320, 128), [45, 90, 240, 255]);
        assert_eq!(pixel(&renderer, 30, 100), [230, 95, 25, 255]);
        let partial = unsafe { std::slice::from_raw_parts(renderer.bits, 512 * 256 * 4).to_vec() };
        renderer.render(&ink, false, None).unwrap();
        let full = unsafe { std::slice::from_raw_parts(renderer.bits, 512 * 256 * 4) };
        assert_eq!(
            partial, full,
            "Shape cutouts must keep identical coverage across dirty tile boundaries"
        );

        assert!(ink.undo());
        renderer.render(&ink, false, Some(dirty)).unwrap();
        assert_eq!(pixel(&renderer, 256, 128), [45, 90, 240, 255]);
        assert!(ink.undo());
        renderer.render(&ink, false, None).unwrap();
        assert_eq!(pixel(&renderer, 100, 100), [0, 0, 0, 0]);
        assert_eq!(pixel(&renderer, 30, 100), [230, 95, 25, 255]);
        assert_eq!(unsafe { IsWindowVisible(window.hwnd) }, 0);
    }

    #[test]
    fn closed_shapes_keep_complete_fill_geometry_during_partial_preparation() {
        let mut ink = Ink::new();
        ink.begin_shape(
            Point::new(20.0, 20.0),
            crate::ink::ShapeKind::Ellipse,
            Color::rgb(200, 40, 80),
            4.0,
        );
        ink.extend(Point::new(480.0, 230.0));
        ink.end();
        ink.fill_at(Point::new(250.0, 120.0), Color::rgb(30, 210, 120))
            .unwrap();
        let dirty = Rect::new(245, 110, 265, 140);
        let stroke = &ink.strokes()[0];
        let full = PreparedStroke::prepare(stroke, dirty, false).unwrap();
        let partial = PreparedStroke::prepare(stroke, dirty, true).unwrap();
        assert_eq!(partial.path, full.path);
        let mut pixmap = Pixmap::new(512, 256).unwrap();
        partial.paint(&mut pixmap, Transform::identity());
        assert_eq!(
            &pixmap.data()[(120 * 512 + 250) * 4..][..4],
            &[120, 210, 30, 255]
        );
        assert_eq!(&pixmap.data()[(20 * 512 + 20) * 4..][..4], &[0, 0, 0, 0]);
    }
}
