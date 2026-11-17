//! A floating straightedge with native interaction and screen-space pen snapping.

use crate::{ink::Point, toolbar_preferences::ToolbarTheme};
use std::{
    cell::{Cell, RefCell},
    f32::consts::TAU,
    marker::PhantomData,
    mem::size_of,
    ptr,
    rc::Rc,
};
use tiny_skia::{
    FillRule, FilterQuality, GradientStop, LineCap, LineJoin, LinearGradient, Paint, Path,
    PathBuilder, PixmapMut, PixmapPaint, PixmapRef, SpreadMode, Stroke, Transform,
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{
            ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx, TOOLTIPS_CLASSW,
            TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW, TTM_POP, TTM_SETMAXTIPWIDTH,
            TTS_ALWAYSTIP, TTTOOLINFOW,
        },
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture, SetFocus},
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "StudyPen.Ruler";
const LENGTH: f32 = 640.0;
const THICKNESS: f32 = 72.0;
const PADDING: f32 = 10.0;
const SNAP_DISTANCE: f32 = 14.0;
const TEXT_SIZE_DIP: f32 = 16.0;
const ZERO_SNAP_ENTER_DEGREES: f32 = 1.5;
const ZERO_SNAP_RELEASE_DEGREES: f32 = 2.5;
const MK_SHIFT_FLAG: WPARAM = 0x0004;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HitTarget {
    Move,
    Rotate,
    Reset,
    Close,
}

/// Captures the chosen long edge at pen-down, independently of later ruler moves.
#[derive(Clone, Copy, Debug)]
pub struct RulerSnap {
    origin: Point,
    direction: Point,
}

impl RulerSnap {
    /// Projects onto the same infinite straight line for the whole gesture.
    pub fn project(self, point: POINT) -> POINT {
        let distance = (point.x as f32 - self.origin.x) * self.direction.x
            + (point.y as f32 - self.origin.y) * self.direction.y;
        POINT {
            x: (self.origin.x + self.direction.x * distance).round() as i32,
            y: (self.origin.y + self.direction.y * distance).round() as i32,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    center: Point,
    length: f32,
    thickness: f32,
    scale: f32,
    angle: f32,
}

impl Geometry {
    fn new(center: POINT, scale: f32, angle: f32, work: RECT) -> Self {
        let available = (work.right - work.left).min(work.bottom - work.top) as f32;
        let available = (available - PADDING * scale * 2.0).max(4.0);
        let thickness = (THICKNESS * scale).min(available * 0.25);
        let length = (LENGTH * scale).min(available - thickness);
        let mut geometry = Self {
            center: Point::new(center.x as f32, center.y as f32),
            length,
            thickness,
            scale,
            angle: angle.rem_euclid(TAU),
        };
        geometry.constrain(work);
        geometry
    }

    fn local(self, point: POINT) -> Point {
        let x = point.x as f32 - self.center.x;
        let y = point.y as f32 - self.center.y;
        let (sin, cos) = self.angle.sin_cos();
        Point::new(x * cos + y * sin, -x * sin + y * cos)
    }

    fn screen(self, x: f32, y: f32) -> Point {
        let (sin, cos) = self.angle.sin_cos();
        Point::new(
            self.center.x + x * cos - y * sin,
            self.center.y + x * sin + y * cos,
        )
    }

    fn bounds(self) -> RECT {
        let (sin, cos) = self.angle.sin_cos();
        let half_width = (cos.abs() * self.length + sin.abs() * self.thickness) * 0.5;
        let half_height = (sin.abs() * self.length + cos.abs() * self.thickness) * 0.5;
        let padding = PADDING * self.scale;
        RECT {
            left: (self.center.x - half_width - padding).floor() as i32,
            top: (self.center.y - half_height - padding).floor() as i32,
            right: (self.center.x + half_width + padding).ceil() as i32,
            bottom: (self.center.y + half_height + padding).ceil() as i32,
        }
    }

    fn constrain(&mut self, work: RECT) {
        let bounds = self.bounds();
        let x = bounds.left.clamp(
            work.left,
            (work.right - (bounds.right - bounds.left)).max(work.left),
        );
        let y = bounds.top.clamp(
            work.top,
            (work.bottom - (bounds.bottom - bounds.top)).max(work.top),
        );
        self.center.x += (x - bounds.left) as f32;
        self.center.y += (y - bounds.top) as f32;
    }

    fn contains(self, point: POINT) -> bool {
        let local = self.local(point);
        local.x.abs() <= self.length * 0.5 && local.y.abs() <= self.thickness * 0.5
    }

    fn close_hit(self, point: POINT) -> bool {
        let local = self.local(point);
        let x = self.length * 0.5 - self.close_inset();
        (local.x - x).abs() <= 13.0 * self.scale && local.y.abs() <= 13.0 * self.scale
    }

    fn close_inset(self) -> f32 {
        (22.0 * self.scale).min(self.length * 0.2)
    }

    fn rotation_bounds(self) -> [f32; 4] {
        let inset = (14.0 * self.scale).min(self.length * 0.06);
        let width = (106.0 * self.scale).min(self.length * 0.28);
        let height = (32.0 * self.scale).min(self.thickness * 0.55);
        [-self.length * 0.5 + inset, -height * 0.5, width, height]
    }

    fn angle_size(self) -> [f32; 2] {
        [
            (66.0 * self.scale).min(self.thickness * 0.9),
            (28.0 * self.scale).min(self.thickness * 0.45),
        ]
    }

    fn hit(self, point: POINT) -> Option<HitTarget> {
        if !self.contains(point) {
            return None;
        }
        if self.close_hit(point) {
            return Some(HitTarget::Close);
        }
        let [width, height] = self.angle_size();
        if (point.x as f32 - self.center.x).abs() <= width * 0.5
            && (point.y as f32 - self.center.y).abs() <= height * 0.5
        {
            return Some(HitTarget::Reset);
        }
        let point = self.local(point);
        let [left, top, width, height] = self.rotation_bounds();
        if (left..=left + width).contains(&point.x) && (top..=top + height).contains(&point.y) {
            Some(HitTarget::Rotate)
        } else {
            Some(HitTarget::Move)
        }
    }

    fn snap(self, point: POINT) -> Option<RulerSnap> {
        let local = self.local(point);
        let threshold = SNAP_DISTANCE * self.scale;
        if local.x.abs() > self.length * 0.5 + threshold {
            return None;
        }
        let edge_y = self.thickness * 0.5 * if local.y < 0.0 { -1.0 } else { 1.0 };
        if (local.y - edge_y).abs() > threshold {
            return None;
        }
        let (sin, cos) = self.angle.sin_cos();
        Some(RulerSnap {
            origin: self.screen(0.0, edge_y),
            direction: Point::new(cos, sin),
        })
    }
}

#[derive(Clone, Copy)]
enum Drag {
    Move {
        pointer: POINT,
        center: Point,
        reset_on_click: bool,
        moved: bool,
    },
    Rotate {
        center: Point,
        offset: f32,
        press: POINT,
        moved: bool,
    },
}

impl Drag {
    fn start(geometry: Geometry, pointer: POINT, target: HitTarget) -> Self {
        if target == HitTarget::Rotate {
            let bearing =
                (pointer.y as f32 - geometry.center.y).atan2(pointer.x as f32 - geometry.center.x);
            Self::Rotate {
                center: geometry.center,
                offset: geometry.angle - bearing,
                press: pointer,
                moved: false,
            }
        } else {
            Self::Move {
                pointer,
                center: geometry.center,
                reset_on_click: target == HitTarget::Reset,
                moved: false,
            }
        }
    }

    fn update(&mut self, mut geometry: Geometry, pointer: POINT, precise: bool) -> Geometry {
        match *self {
            Self::Move {
                pointer: start,
                center,
                reset_on_click,
                moved,
            } => {
                let moved = moved
                    || ((pointer.x - start.x) as f32).hypot((pointer.y - start.y) as f32)
                        >= 3.0 * geometry.scale;
                *self = Self::Move {
                    pointer: start,
                    center,
                    reset_on_click,
                    moved,
                };
                if !reset_on_click || moved {
                    geometry.center = Point::new(
                        center.x + (pointer.x - start.x) as f32,
                        center.y + (pointer.y - start.y) as f32,
                    );
                }
            }
            Self::Rotate {
                center,
                offset,
                press,
                moved,
            } => {
                let moved = moved
                    || ((pointer.x - press.x) as f32).hypot((pointer.y - press.y) as f32)
                        >= 3.0 * geometry.scale;
                *self = Self::Rotate {
                    center,
                    offset,
                    press,
                    moved,
                };
                let dx = pointer.x as f32 - center.x;
                let dy = pointer.y as f32 - center.y;
                if moved && dx.hypot(dy) >= 8.0 * geometry.scale {
                    geometry.angle = rotation_angle(dy.atan2(dx) + offset, geometry.angle, precise);
                }
                geometry.center = center;
            }
        }
        geometry
    }

    fn finish(mut self, geometry: Geometry, pointer: POINT, precise: bool) -> Geometry {
        let mut geometry = self.update(geometry, pointer, precise);
        if matches!(self, Self::Rotate { moved: false, .. }) {
            geometry.angle = rotation_angle(
                geometry.angle + 15.0_f32.to_radians(),
                geometry.angle,
                precise,
            );
        }
        if let Self::Move {
            center,
            reset_on_click: true,
            moved: false,
            ..
        } = self
        {
            geometry.center = center;
            geometry.angle = 0.0;
        }
        geometry
    }
}

struct State {
    geometry: Cell<Geometry>,
    theme: Cell<ToolbarTheme>,
    drag: Cell<Option<Drag>>,
    surface: RefCell<Option<Surface>>,
    text: RefCell<Option<TextCache>>,
    tips: Cell<HWND>,
}

impl State {
    unsafe fn paint(&self, window: HWND) -> Result<(), String> {
        let geometry = self.geometry.get();
        let bounds = geometry.bounds();
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        {
            // A DPI notification may reenter during native presentation. The
            // outer paint presents again below if that notification changes geometry.
            let Ok(mut surface) = self.surface.try_borrow_mut() else {
                return Ok(());
            };
            if surface
                .as_ref()
                .is_none_or(|surface| surface.width != width || surface.height != height)
            {
                *surface = Some(Surface::new(width, height)?);
            }
            let surface = surface.as_mut().unwrap();
            let Ok(mut text) = self.text.try_borrow_mut() else {
                return Ok(());
            };
            if text
                .as_ref()
                .is_none_or(|text| text.scale != geometry.scale)
            {
                *text = Some(TextCache::new(geometry.scale)?);
            }
            paint_surface(
                surface,
                text.as_mut().unwrap(),
                geometry,
                self.theme.get(),
                bounds,
            )?;
            SetWindowTextW(
                window,
                wide(&format!(
                    "Study Pen ruler — {}",
                    angle_label(geometry.angle)
                ))
                .as_ptr(),
            );
            let source = POINT::default();
            let destination = POINT {
                x: bounds.left,
                y: bounds.top,
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
                &destination,
                &size,
                surface.dc,
                &source,
                0,
                &blend,
                ULW_ALPHA,
            ) == 0
            {
                return Err(native_error("Could not display the ruler"));
            }
            self.update_tip_bounds(window, geometry, bounds, TTM_NEWTOOLRECTW);
        }
        if self.geometry.get() != geometry {
            self.paint(window)?;
        }
        Ok(())
    }

    unsafe fn hide(&self, window: HWND) {
        self.drag.set(None);
        if GetCapture() == window {
            ReleaseCapture();
        }
        ShowWindow(window, SW_HIDE);
        if !self.tips.get().is_null() {
            SendMessageW(self.tips.get(), TTM_POP, 0, 0);
        }
        *self.surface.borrow_mut() = None;
        *self.text.borrow_mut() = None;
    }

    unsafe fn fit_to_monitor(&self, scale: f32) {
        let previous = self.geometry.get();
        let center = POINT {
            x: previous.center.x.round() as i32,
            y: previous.center.y.round() as i32,
        };
        if let Some(work) = work_area(center) {
            let mut geometry = Geometry::new(center, scale, previous.angle, work);
            if self.drag.get().is_some() {
                geometry.center = previous.center;
            }
            self.geometry.set(geometry);
        }
    }

    unsafe fn create_tips(&self, window: HWND, instance: HINSTANCE) {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        });
        let tips = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            TOOLTIPS_CLASSW,
            ptr::null(),
            WS_POPUP | TTS_ALWAYSTIP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            window,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        self.tips.set(tips);
        let geometry = self.geometry.get();
        self.update_tip_bounds(window, geometry, geometry.bounds(), TTM_ADDTOOLW);
    }

    unsafe fn update_tip_bounds(
        &self,
        window: HWND,
        geometry: Geometry,
        bounds: RECT,
        message: u32,
    ) {
        if self.tips.get().is_null() {
            return;
        }
        let [width, height] = geometry.angle_size();
        let reset = RECT {
            left: (geometry.center.x - bounds.left as f32 - width * 0.5).floor() as i32,
            top: (geometry.center.y - bounds.top as f32 - height * 0.5).floor() as i32,
            right: (geometry.center.x - bounds.left as f32 + width * 0.5).ceil() as i32,
            bottom: (geometry.center.y - bounds.top as f32 + height * 0.5).ceil() as i32,
        };
        let [left, top, width, height] = geometry.rotation_bounds();
        let corners = [
            geometry.screen(left, top),
            geometry.screen(left + width, top),
            geometry.screen(left, top + height),
            geometry.screen(left + width, top + height),
        ];
        let rotation = RECT {
            left: (corners
                .iter()
                .map(|point| point.x)
                .fold(f32::INFINITY, f32::min)
                - bounds.left as f32)
                .floor() as i32,
            top: (corners
                .iter()
                .map(|point| point.y)
                .fold(f32::INFINITY, f32::min)
                - bounds.top as f32)
                .floor() as i32,
            right: (corners
                .iter()
                .map(|point| point.x)
                .fold(f32::NEG_INFINITY, f32::max)
                - bounds.left as f32)
                .ceil() as i32,
            bottom: (corners
                .iter()
                .map(|point| point.y)
                .fold(f32::NEG_INFINITY, f32::max)
                - bounds.top as f32)
                .ceil() as i32,
        };
        for (index, (rect, text)) in [
            (
                reset,
                windows_sys::w!("Click to reset to 0°. Drag to move."),
            ),
            (
                rotation,
                windows_sys::w!("Drag to rotate. Click for 15°. Hold Shift for precise angles."),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let tool = TTTOOLINFOW {
                // The V2 header also registers tools with unthemed Comctl32 v5.
                cbSize: std::mem::offset_of!(TTTOOLINFOW, lpReserved) as u32,
                uFlags: TTF_SUBCLASS,
                hwnd: window,
                uId: index + 1,
                rect,
                lpszText: text.cast_mut(),
                ..Default::default()
            };
            SendMessageW(
                self.tips.get(),
                message,
                0,
                &tool as *const TTTOOLINFOW as isize,
            );
        }
        SendMessageW(
            self.tips.get(),
            TTM_SETMAXTIPWIDTH,
            0,
            (280.0 * geometry.scale).round() as isize,
        );
    }
}

/// Owns the native window and is confined to its creating UI thread.
pub struct Ruler {
    window: HWND,
    state: Box<State>,
    _thread: PhantomData<Rc<()>>,
}

impl Ruler {
    pub fn new(instance: HINSTANCE, theme: ToolbarTheme) -> Result<Self, String> {
        unsafe {
            let instance = if instance.is_null() {
                GetModuleHandleW(ptr::null())
            } else {
                instance
            };
            let class_name = wide(CLASS);
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                hCursor: LoadCursorW(ptr::null_mut(), IDC_SIZEALL),
                ..Default::default()
            };
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(native_error("Could not register the ruler window"));
            }
            let state = Box::new(State {
                geometry: Cell::new(Geometry {
                    center: Point::new(0.0, 0.0),
                    length: LENGTH,
                    thickness: THICKNESS,
                    scale: 1.0,
                    angle: 0.0,
                }),
                theme: Cell::new(theme),
                drag: Cell::new(None),
                surface: RefCell::new(None),
                text: RefCell::new(None),
                tips: Cell::new(ptr::null_mut()),
            });
            let window = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name.as_ptr(),
                wide("Study Pen ruler").as_ptr(),
                WS_POPUP,
                0,
                0,
                1,
                1,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                (&raw const *state).cast(),
            );
            if window.is_null() {
                return Err(native_error("Could not create the ruler window"));
            }
            state.create_tips(window, instance);
            Ok(Self {
                window,
                state,
                _thread: PhantomData,
            })
        }
    }

    pub fn handle(&self) -> HWND {
        self.window
    }

    pub fn show_at(&self, center: POINT) -> Result<(), String> {
        unsafe {
            let work = work_area(center).ok_or("Could not find the ruler's monitor.")?;
            let mut geometry = self.state.geometry.get();
            geometry.center = Point::new(center.x as f32, center.y as f32);
            self.state.geometry.set(geometry);
            SetWindowPos(
                self.window,
                HWND_TOPMOST,
                center.x,
                center.y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
            let scale = GetDpiForWindow(self.window).max(96) as f32 / 96.0;
            self.state
                .geometry
                .set(Geometry::new(center, scale, geometry.angle, work));
            self.state.paint(self.window)?;
            ShowWindow(self.window, SW_SHOWNOACTIVATE);
            Ok(())
        }
    }

    pub fn hide(&self) {
        unsafe { self.state.hide(self.window) };
    }

    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.window) != 0 }
    }

    pub fn set_theme(&self, theme: ToolbarTheme) -> Result<(), String> {
        if self.state.theme.replace(theme) != theme && self.is_visible() {
            unsafe { self.state.paint(self.window)? };
        }
        Ok(())
    }

    pub fn snap(&self, point: POINT) -> Option<RulerSnap> {
        self.is_visible()
            .then(|| self.state.geometry.get().snap(point))
            .flatten()
    }
}

impl Drop for Ruler {
    fn drop(&mut self) {
        unsafe {
            if !self.state.tips.get().is_null() {
                DestroyWindow(self.state.tips.get());
            }
            if GetCapture() == self.window {
                ReleaseCapture();
            }
            DestroyWindow(self.window);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        return 1;
    }
    let Some(state) = (GetWindowLongPtrW(window, GWLP_USERDATA) as *const State).as_ref() else {
        return DefWindowProcW(window, message, wparam, lparam);
    };
    match message {
        WM_NCHITTEST => {
            let point = POINT {
                x: lparam as u16 as i16 as i32,
                y: (lparam >> 16) as u16 as i16 as i32,
            };
            if state.geometry.get().hit(point).is_some() {
                HTCLIENT as isize
            } else {
                HTTRANSPARENT as isize
            }
        }
        WM_SETCURSOR => {
            let mut pointer = POINT::default();
            GetCursorPos(&mut pointer);
            let cursor = if matches!(
                state.geometry.get().hit(pointer),
                Some(HitTarget::Rotate | HitTarget::Reset | HitTarget::Close)
            ) {
                IDC_HAND
            } else {
                IDC_SIZEALL
            };
            SetCursor(LoadCursorW(ptr::null_mut(), cursor));
            1
        }
        WM_LBUTTONDOWN => {
            let pointer = message_point(window, lparam);
            match state.geometry.get().hit(pointer) {
                Some(HitTarget::Close) => state.hide(window),
                Some(target) => {
                    SetFocus(window);
                    state
                        .drag
                        .set(Some(Drag::start(state.geometry.get(), pointer, target)));
                    SetCapture(window);
                }
                None => {}
            }
            0
        }
        WM_MOUSEMOVE => {
            if let Some(mut drag) = state.drag.get() {
                let mut pointer = POINT::default();
                GetCursorPos(&mut pointer);
                state.geometry.set(drag.update(
                    state.geometry.get(),
                    pointer,
                    wparam & MK_SHIFT_FLAG != 0,
                ));
                state.drag.set(Some(drag));
                let _ = state.paint(window);
            }
            0
        }
        WM_LBUTTONUP => {
            if let Some(drag) = state.drag.take() {
                let pointer = message_point(window, lparam);
                state.geometry.set(drag.finish(
                    state.geometry.get(),
                    pointer,
                    wparam & MK_SHIFT_FLAG != 0,
                ));
                ReleaseCapture();
                state.fit_to_monitor(state.geometry.get().scale);
                let _ = state.paint(window);
            }
            0
        }
        WM_CAPTURECHANGED => {
            state.drag.set(None);
            0
        }
        WM_MOUSEWHEEL => {
            if state.drag.get().is_some() {
                return 0;
            }
            let delta = (wparam >> 16) as u16 as i16;
            let mut geometry = state.geometry.get();
            geometry.angle = (geometry.angle
                + (delta as f32 / WHEEL_DELTA as f32) * 5.0_f32.to_radians())
            .rem_euclid(TAU);
            state.geometry.set(geometry);
            state.fit_to_monitor(geometry.scale);
            let _ = state.paint(window);
            0
        }
        WM_DPICHANGED => {
            let scale = (wparam & 0xffff).max(96) as f32 / 96.0;
            state.fit_to_monitor(scale);
            let _ = state.paint(window);
            0
        }
        WM_DISPLAYCHANGE => {
            state.fit_to_monitor(state.geometry.get().scale);
            if IsWindowVisible(window) != 0 {
                let _ = state.paint(window);
            }
            0
        }
        WM_CLOSE => {
            state.hide(window);
            0
        }
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            BeginPaint(window, &mut paint);
            EndPaint(window, &paint);
            0
        }
        WM_ERASEBKGND => 1,
        WM_NCDESTROY => {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            DefWindowProcW(window, message, wparam, lparam)
        }
        _ => DefWindowProcW(window, message, wparam, lparam),
    }
}

unsafe fn work_area(point: POINT) -> Option<RECT> {
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    (GetMonitorInfoW(monitor, &mut info) != 0).then_some(info.rcWork)
}

unsafe fn message_point(window: HWND, lparam: LPARAM) -> POINT {
    let mut point = POINT {
        x: lparam as u16 as i16 as i32,
        y: (lparam >> 16) as u16 as i16 as i32,
    };
    ClientToScreen(window, &mut point);
    point
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
        if !(1..=8192).contains(&width) || !(1..=8192).contains(&height) {
            return Err("The ruler exceeds the supported display size.".into());
        }
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return Err(native_error("Could not prepare the ruler surface"));
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
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, ptr::null_mut(), 0);
        if bitmap.is_null() || bits.is_null() {
            if !bitmap.is_null() {
                DeleteObject(bitmap);
            }
            DeleteDC(dc);
            return Err(native_error("Could not allocate the ruler bitmap"));
        }
        let previous = SelectObject(dc, bitmap);
        if previous.is_null() || previous as isize == -1 {
            DeleteObject(bitmap);
            DeleteDC(dc);
            return Err(native_error("Could not select the ruler bitmap"));
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

struct TextFont(HFONT);

impl TextFont {
    unsafe fn new(scale: f32, escapement: i32) -> Result<Self, String> {
        let font = CreateFontW(
            -(TEXT_SIZE_DIP * scale).round() as i32,
            0,
            escapement,
            escapement,
            FW_MEDIUM as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            DEFAULT_PITCH as u32,
            windows_sys::w!("Segoe UI"),
        );
        if font.is_null() {
            return Err(native_error("Could not create the ruler font"));
        }
        Ok(Self(font))
    }
}

impl Drop for TextFont {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.0) };
    }
}

struct TextCache {
    surface: Surface,
    upright_font: TextFont,
    rotated_font: Option<(i32, TextFont)>,
    scale: f32,
}

impl TextCache {
    unsafe fn new(scale: f32) -> Result<Self, String> {
        Ok(Self {
            surface: Surface::new((160.0 * scale).ceil() as i32, (72.0 * scale).ceil() as i32)?,
            upright_font: TextFont::new(scale, 0)?,
            rotated_font: None,
            scale,
        })
    }

    unsafe fn draw_angle(
        &mut self,
        pixels: &mut PixmapMut<'_>,
        angle: f32,
        center: Point,
        size: [f32; 2],
        color: [u8; 4],
    ) {
        let label = angle_label(angle);
        let number = label.trim_end_matches('°');
        let digits: Vec<u16> = number.encode_utf16().collect();
        let dc = self.surface.dc;
        let previous = SelectObject(dc, self.upright_font.0);
        let mut extent = SIZE::default();
        let measured = GetTextExtentPoint32W(dc, digits.as_ptr(), digits.len() as i32, &mut extent);
        SelectObject(dc, previous);
        let mark_space = 7.0 * self.scale;
        if measured == 0 || (extent.cx as f32 + mark_space) > size[0] {
            self.draw(pixels, &label, center, size, 0.0, color);
            return;
        }
        self.draw(
            pixels,
            number,
            Point::new(center.x - mark_space * 0.5, center.y),
            [size[0] - mark_space, size[1]],
            0.0,
            color,
        );
        // The hinted degree glyph can lose its counter at 96 DPI.
        let x = (center.x + extent.cx as f32 * 0.5 + self.scale).floor() + 0.5;
        let y = (center.y - 3.5 * self.scale).floor() + 0.5;
        if let Some(ring) = PathBuilder::from_circle(x, y, 2.0 * self.scale) {
            stroke_path(
                pixels,
                &ring,
                color,
                1.1 * self.scale,
                Transform::identity(),
            );
        }
    }

    unsafe fn draw(
        &mut self,
        pixels: &mut PixmapMut<'_>,
        label: &str,
        center: Point,
        size: [f32; 2],
        angle: f32,
        color: [u8; 4],
    ) {
        let escapement = (-angle.to_degrees() * 10.0).round() as i32;
        let font = if escapement == 0 {
            self.upright_font.0
        } else {
            if self
                .rotated_font
                .as_ref()
                .is_none_or(|(cached, _)| *cached != escapement)
            {
                let Ok(font) = TextFont::new(self.scale, escapement) else {
                    return;
                };
                self.rotated_font = Some((escapement, font));
            }
            self.rotated_font.as_ref().unwrap().1.0
        };
        let bytes = std::slice::from_raw_parts_mut(
            self.surface.bits,
            self.surface.width as usize * self.surface.height as usize * 4,
        );
        bytes.fill(0);
        let saved = SaveDC(self.surface.dc);
        if saved == 0 {
            return;
        }
        SelectObject(self.surface.dc, font);
        SetBkMode(self.surface.dc, TRANSPARENT as i32);
        SetTextColor(self.surface.dc, 0x00ff_ffff);
        SetTextAlign(self.surface.dc, TA_CENTER | TA_BASELINE);
        let value: Vec<u16> = label.encode_utf16().collect();
        let mut metrics = TEXTMETRICW::default();
        GetTextMetricsW(self.surface.dc, &mut metrics);
        let (sin, cos) = angle.sin_cos();
        let mask_center = Point::new(
            self.surface.width as f32 * 0.5,
            self.surface.height as f32 * 0.5,
        );
        let half_width = (cos.abs() * size[0] + sin.abs() * size[1]) * 0.5;
        let half_height = (sin.abs() * size[0] + cos.abs() * size[1]) * 0.5;
        IntersectClipRect(
            self.surface.dc,
            (mask_center.x - half_width).floor() as i32,
            (mask_center.y - half_height).floor() as i32,
            (mask_center.x + half_width).ceil() as i32,
            (mask_center.y + half_height).ceil() as i32,
        );
        let baseline = (metrics.tmAscent - metrics.tmDescent) as f32 * 0.5;
        TextOutW(
            self.surface.dc,
            (mask_center.x - sin * baseline).round() as i32,
            (mask_center.y + cos * baseline).round() as i32,
            value.as_ptr(),
            value.len() as i32,
        );
        RestoreDC(self.surface.dc, saved);
        GdiFlush();
        for pixel in bytes.as_chunks_mut::<4>().0 {
            let alpha =
                (u32::from(pixel[0].max(pixel[1]).max(pixel[2])) * u32::from(color[3]) + 127) / 255;
            for channel in 0..3 {
                pixel[channel] = ((u32::from(color[channel]) * alpha + 127) / 255) as u8;
            }
            pixel[3] = alpha as u8;
        }
        // GDI rasterizes the final angle and DPI. Copy that mask onto the same
        // physical pixel grid without rescaling or rotating the bitmap again.
        let transform = Transform::from_translate(
            (center.x - mask_center.x).round(),
            (center.y - mask_center.y).round(),
        );
        let image =
            PixmapRef::from_bytes(bytes, self.surface.width as u32, self.surface.height as u32)
                .unwrap();
        let paint = PixmapPaint {
            quality: FilterQuality::Nearest,
            ..Default::default()
        };
        pixels.draw_pixmap(0, 0, image, &paint, transform, None);
    }
}

fn angle_label(angle: f32) -> String {
    let degrees = (angle.rem_euclid(TAU).to_degrees() * 10.0).round() / 10.0;
    format!("{:.1}°", if degrees >= 360.0 { 0.0 } else { degrees })
}

fn rotation_angle(angle: f32, previous: f32, precise: bool) -> f32 {
    let angle = angle.rem_euclid(TAU);
    let distance = angle.min(TAU - angle).to_degrees();
    let threshold = if previous == 0.0 {
        ZERO_SNAP_RELEASE_DEGREES
    } else {
        ZERO_SNAP_ENTER_DEGREES
    };
    if !precise && distance <= threshold {
        0.0
    } else {
        angle
    }
}

fn rotation_icon(
    pixels: &mut PixmapMut<'_>,
    x: f32,
    radius: f32,
    color: [u8; 4],
    scale: f32,
    transform: Transform,
) {
    let curve = radius * 0.552_284_8;
    let mut path = PathBuilder::new();
    path.move_to(x - radius, 0.0);
    path.cubic_to(x - radius, -curve, x - curve, -radius, x, -radius);
    path.cubic_to(x + curve, -radius, x + radius, -curve, x + radius, 0.0);
    path.cubic_to(x + radius, curve, x + curve, radius, x, radius);
    path.move_to(x + radius * 0.48, radius * 0.55);
    path.line_to(x, radius);
    path.line_to(x + radius * 0.48, radius * 1.4);
    if let Some(path) = path.finish() {
        stroke_path(
            pixels,
            &path,
            color,
            (1.65 * scale).min(radius * 0.23),
            transform,
        );
    }
}

unsafe fn paint_surface(
    surface: &mut Surface,
    text: &mut TextCache,
    geometry: Geometry,
    theme: ToolbarTheme,
    bounds: RECT,
) -> Result<(), String> {
    let bytes = std::slice::from_raw_parts_mut(
        surface.bits,
        surface.width as usize * surface.height as usize * 4,
    );
    let mut pixels = PixmapMut::from_bytes(bytes, surface.width as u32, surface.height as u32)
        .ok_or("Could not access the ruler pixels.")?;
    pixels.fill(tiny_skia::Color::TRANSPARENT);
    let (sin, cos) = geometry.angle.sin_cos();
    let transform = Transform::from_row(
        cos,
        sin,
        -sin,
        cos,
        geometry.center.x - bounds.left as f32,
        geometry.center.y - bounds.top as f32,
    );
    let scale = geometry.scale;
    let left = -geometry.length * 0.5;
    let right = geometry.length * 0.5;
    let top = -geometry.thickness * 0.5;
    let bottom = geometry.thickness * 0.5;
    let radius = (8.0 * scale).min(geometry.thickness * 0.2);
    let body = rounded_path(left, top, right, bottom, radius)
        .ok_or("Could not draw the ruler outline.")?;
    for (width, alpha) in [(10.0, 8), (6.0, 10), (3.0, 14)] {
        stroke_path(
            &mut pixels,
            &body,
            [4, 12, 24, alpha],
            width * scale,
            transform,
        );
    }
    let (fill, border, ink) = match theme {
        ToolbarTheme::LiquidGlass => (
            [221, 232, 242, 114],
            [249, 252, 255, 194],
            [27, 39, 56, 220],
        ),
        ToolbarTheme::LiquidAir => (
            [165, 219, 255, 174],
            [213, 233, 255, 218],
            [22, 42, 78, 240],
        ),
    };
    let mut material = paint(fill);
    if theme == ToolbarTheme::LiquidAir {
        material.shader = LinearGradient::new(
            tiny_skia::Point::from_xy(left, top),
            tiny_skia::Point::from_xy(right, bottom),
            vec![
                GradientStop::new(0.0, tiny_skia::Color::from_rgba8(221, 247, 255, 174)),
                GradientStop::new(0.55, tiny_skia::Color::from_rgba8(165, 219, 255, 174)),
                GradientStop::new(1.0, tiny_skia::Color::from_rgba8(171, 185, 243, 174)),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        )
        .ok_or("Could not draw the ruler gradient.")?;
    }
    pixels.fill_path(&body, &material, FillRule::Winding, transform, None);
    stroke_path(&mut pixels, &body, border, 1.2 * scale, transform);
    let mut ticks = PathBuilder::new();
    let start = left + 20.0 * scale;
    let end = right - 50.0 * scale;
    let count = ((end - start) / (10.0 * scale)).floor().max(0.0) as usize;
    for index in 0..=count {
        let x = start + index as f32 * 10.0 * scale;
        let height = if index % 5 == 0 { 19.0 } else { 10.0 } * scale;
        ticks.move_to(x, top + 4.0 * scale);
        ticks.line_to(x, top + height);
        ticks.move_to(x, bottom - 4.0 * scale);
        ticks.line_to(x, bottom - height * 0.5);
    }
    if let Some(ticks) = ticks.finish() {
        stroke_path(&mut pixels, &ticks, ink, 1.0 * scale, transform);
    }
    let [rotation_x, rotation_y, rotation_width, rotation_height] = geometry.rotation_bounds();
    if let Some(control) = rounded_path(
        rotation_x,
        rotation_y,
        rotation_x + rotation_width,
        rotation_y + rotation_height,
        rotation_height * 0.5,
    ) {
        pixels.fill_path(
            &control,
            &paint([247, 251, 255, 170]),
            FillRule::Winding,
            transform,
            None,
        );
        stroke_path(
            &mut pixels,
            &control,
            [245, 251, 255, 182],
            scale,
            transform,
        );
    }
    let label_width = rotation_width - 44.0 * scale;
    let labeled = label_width >= 52.0 * scale;
    let icon_x = if labeled {
        rotation_x + 20.0 * scale
    } else {
        rotation_x + rotation_width * 0.5
    };
    rotation_icon(
        &mut pixels,
        icon_x,
        (rotation_height * 0.23).min(8.0 * scale),
        ink,
        scale,
        transform,
    );
    if labeled {
        let center = geometry.screen(rotation_x + 36.0 * scale + label_width * 0.5, 0.0);
        let center = Point::new(center.x - bounds.left as f32, center.y - bounds.top as f32);
        let readable_angle = if cos < 0.0 {
            geometry.angle - std::f32::consts::PI
        } else {
            geometry.angle
        };
        text.draw(
            &mut pixels,
            "Rotate",
            center,
            [label_width, 26.0 * scale],
            readable_angle,
            [15, 25, 44, 255],
        );
    }
    for x in [-47.0, -41.0] {
        for y in [-3.0, 3.0] {
            if let Some(dot) = PathBuilder::from_circle(x * scale, y * scale, 1.1 * scale) {
                pixels.fill_path(&dot, &paint(ink), FillRule::Winding, transform, None);
            }
        }
    }
    let [label_width, label_height] = geometry.angle_size();
    let center = Point::new(
        geometry.center.x - bounds.left as f32,
        geometry.center.y - bounds.top as f32,
    );
    let label_transform = Transform::from_translate(center.x, center.y);
    if let Some(label) = rounded_path(
        -label_width * 0.5,
        -label_height * 0.5,
        label_width * 0.5,
        label_height * 0.5,
        label_height * 0.5,
    ) {
        pixels.fill_path(
            &label,
            &paint([249, 252, 255, 220]),
            FillRule::Winding,
            label_transform,
            None,
        );
    }
    text.draw_angle(
        &mut pixels,
        geometry.angle,
        center,
        [label_width, label_height],
        [15, 25, 44, 255],
    );
    let close_x = right - geometry.close_inset();
    if let Some(circle) = PathBuilder::from_circle(close_x, 0.0, 12.0 * scale) {
        pixels.fill_path(
            &circle,
            &paint([255, 255, 255, 62]),
            FillRule::Winding,
            transform,
            None,
        );
    }
    let mut cross = PathBuilder::new();
    cross.move_to(close_x - 4.0 * scale, -4.0 * scale);
    cross.line_to(close_x + 4.0 * scale, 4.0 * scale);
    cross.move_to(close_x - 4.0 * scale, 4.0 * scale);
    cross.line_to(close_x + 4.0 * scale, -4.0 * scale);
    if let Some(cross) = cross.finish() {
        stroke_path(&mut pixels, &cross, ink, 1.5 * scale, transform);
    }
    for pixel in pixels.data_mut().as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(())
}

fn rounded_path(left: f32, top: f32, right: f32, bottom: f32, radius: f32) -> Option<Path> {
    let mut path = PathBuilder::new();
    path.move_to(left + radius, top);
    path.line_to(right - radius, top);
    path.quad_to(right, top, right, top + radius);
    path.line_to(right, bottom - radius);
    path.quad_to(right, bottom, right - radius, bottom);
    path.line_to(left + radius, bottom);
    path.quad_to(left, bottom, left, bottom - radius);
    path.line_to(left, top + radius);
    path.quad_to(left, top, left + radius, top);
    path.close();
    path.finish()
}

fn paint([red, green, blue, alpha]: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(red, green, blue, alpha);
    paint.anti_alias = true;
    paint
}

fn stroke_path(
    pixels: &mut PixmapMut<'_>,
    path: &Path,
    color: [u8; 4],
    width: f32,
    transform: Transform,
) {
    let stroke = Stroke {
        width,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Default::default()
    };
    pixels.stroke_path(path, &paint(color), &stroke, transform, None);
}

fn native_error(message: &str) -> String {
    format!("{message} (Windows error {}).", unsafe { GetLastError() })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(angle: f32, scale: f32) -> Geometry {
        Geometry::new(
            POINT { x: 500, y: 400 },
            scale,
            angle,
            RECT {
                left: -1920,
                top: -1080,
                right: 1920,
                bottom: 1080,
            },
        )
    }

    #[test]
    fn horizontal_snap_uses_nearest_long_edge_and_rejects_remote_points() {
        let ruler = geometry(0.0, 1.0);
        let top = ruler.snap(POINT { x: 600, y: 359 }).unwrap();
        let projected = top.project(POINT { x: 640, y: 490 });
        assert_eq!((projected.x, projected.y), (640, 364));
        let bottom = ruler.snap(POINT { x: 600, y: 441 }).unwrap();
        let projected = bottom.project(POINT { x: 620, y: 200 });
        assert_eq!((projected.x, projected.y), (620, 436));
        assert!(ruler.snap(POINT { x: 500, y: 400 }).is_none());
        assert!(ruler.snap(POINT { x: 900, y: 364 }).is_none());
        assert!(ruler.snap(POINT { x: 500, y: 349 }).is_none());
    }

    #[test]
    fn rotated_snap_and_snapshot_stay_on_the_selected_edge() {
        let mut ruler = geometry(45.0_f32.to_radians(), 1.0);
        let near_edge = ruler.screen(120.0, -40.0);
        let snap = ruler
            .snap(POINT {
                x: near_edge.x.round() as i32,
                y: near_edge.y.round() as i32,
            })
            .unwrap();
        ruler.center = Point::new(900.0, 900.0);
        ruler.angle = 0.0;
        assert!(
            ruler
                .snap(POINT {
                    x: near_edge.x.round() as i32,
                    y: near_edge.y.round() as i32,
                })
                .is_none()
        );
        let projected = snap.project(POINT { x: 700, y: 200 });
        let original = geometry(45.0_f32.to_radians(), 1.0);
        let local = original.local(projected);
        assert!((local.y + 36.0).abs() <= 0.8);
        let later = snap.project(POINT { x: 1700, y: -500 });
        assert!((original.local(later).y + 36.0).abs() <= 0.8);
    }

    #[test]
    fn snapping_distance_scales_with_the_display() {
        let ruler = geometry(0.0, 2.0);
        assert!(ruler.snap(POINT { x: 500, y: 300 }).is_some());
        assert!(ruler.snap(POINT { x: 500, y: 299 }).is_none());
    }

    #[test]
    fn fitted_rotations_stay_inside_negative_monitor_work_areas() {
        let work = RECT {
            left: -900,
            top: -480,
            right: -260,
            bottom: -20,
        };
        for scale in [1.0, 1.25, 2.0, 3.0] {
            for angle in [0.0_f32, 45.0, 90.0, 135.0] {
                let ruler =
                    Geometry::new(POINT { x: -890, y: -470 }, scale, angle.to_radians(), work);
                let bounds = ruler.bounds();
                assert!(bounds.left >= work.left && bounds.top >= work.top);
                assert!(bounds.right <= work.right && bounds.bottom <= work.bottom);
                assert!(ruler.length > ruler.thickness);
            }
        }
    }

    #[test]
    fn close_affordance_and_transparent_corners_use_rotated_geometry() {
        let ruler = geometry(45.0_f32.to_radians(), 1.0);
        let close = ruler.screen(ruler.length * 0.5 - ruler.close_inset(), 0.0);
        assert!(ruler.close_hit(POINT {
            x: close.x.round() as i32,
            y: close.y.round() as i32
        }));
        let bounds = ruler.bounds();
        assert!(!ruler.contains(POINT {
            x: bounds.left,
            y: bounds.top
        }));
        assert!(ruler.contains(POINT { x: 500, y: 400 }));
    }

    #[test]
    fn rotation_click_and_drag_generate_angles_without_moving_the_pivot() {
        let ruler = geometry(30.0_f32.to_radians(), 1.0);
        let [left, _, width, _] = ruler.rotation_bounds();
        let local_x = left + width * 0.5;
        let point = |geometry: Geometry| {
            let screen = geometry.screen(local_x, 0.0);
            POINT {
                x: screen.x.round() as i32,
                y: screen.y.round() as i32,
            }
        };
        let press = point(ruler);
        let mut click = Drag::start(ruler, press, HitTarget::Rotate);
        let unchanged = click.update(
            ruler,
            POINT {
                x: press.x + 1,
                y: press.y,
            },
            false,
        );
        assert_eq!(unchanged.angle, ruler.angle);
        let clicked = click.finish(unchanged, press, false);
        assert!((clicked.angle.to_degrees() - 45.0).abs() < 0.001);
        assert_eq!(clicked.center, ruler.center);

        let mut drag = Drag::start(ruler, press, HitTarget::Rotate);
        let target = Geometry {
            angle: 123.4_f32.to_radians(),
            ..ruler
        };
        let rotated = drag.update(ruler, point(target), false);
        assert!((rotated.angle.to_degrees() - 123.4).abs() < 0.25);
        assert_eq!(rotated.center, ruler.center);
        let center = POINT {
            x: ruler.center.x as i32,
            y: ruler.center.y as i32,
        };
        assert_eq!(drag.update(rotated, center, false).angle, rotated.angle);
        assert!((drag.finish(rotated, point(target), false).angle - rotated.angle).abs() < 0.001);
    }

    #[test]
    fn rotation_hit_target_stays_distinct_from_move_close_and_snap_at_common_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for angle in [0.0_f32, 17.0, 90.0, 215.0] {
                let ruler = geometry(angle.to_radians(), scale);
                let screen = |x, y| {
                    let point = ruler.screen(x, y);
                    POINT {
                        x: point.x.round() as i32,
                        y: point.y.round() as i32,
                    }
                };
                let [left, _, width, _] = ruler.rotation_bounds();
                assert_eq!(
                    ruler.hit(screen(left + width * 0.5, 0.0)),
                    Some(HitTarget::Rotate)
                );
                assert_eq!(
                    ruler.hit(screen(ruler.length * 0.5 - ruler.close_inset(), 0.0)),
                    Some(HitTarget::Close)
                );
                assert_eq!(ruler.hit(screen(0.0, 0.0)), Some(HitTarget::Reset));
                assert_eq!(ruler.hit(screen(80.0 * scale, 0.0)), Some(HitTarget::Move));
                let edge = screen(0.0, -ruler.thickness * 0.5 - 5.0 * scale);
                assert_eq!(ruler.hit(edge), None);
                assert!(ruler.snap(edge).is_some());
            }
        }
    }

    #[test]
    fn degree_readout_handles_wrapping_and_fractional_angles() {
        assert_eq!(angle_label(0.0), "0.0°");
        assert_eq!(angle_label(123.4_f32.to_radians()), "123.4°");
        assert_eq!(angle_label(-30.0_f32.to_radians()), "330.0°");
        assert_eq!(angle_label(359.96_f32.to_radians()), "0.0°");
        assert_eq!(angle_label(TAU), "0.0°");
    }

    #[test]
    fn horizontal_snap_has_a_release_margin_and_shift_preserves_nearby_angles() {
        for degrees in [1.4_f32, 358.6] {
            assert_eq!(
                rotation_angle(degrees.to_radians(), 20.0_f32.to_radians(), false),
                0.0
            );
        }
        assert_eq!(rotation_angle(2.4_f32.to_radians(), 0.0, false), 0.0);
        assert!(
            (rotation_angle(2.6_f32.to_radians(), 0.0, false).to_degrees() - 2.6).abs() < 0.001
        );
        assert!(
            (rotation_angle(1.8_f32.to_radians(), 2.6_f32.to_radians(), false).to_degrees() - 1.8)
                .abs()
                < 0.001
        );
        assert!((rotation_angle(0.5_f32.to_radians(), 0.0, true).to_degrees() - 0.5).abs() < 0.001);
        assert!(
            (rotation_angle(359.5_f32.to_radians(), 0.0, true).to_degrees() - 359.5).abs() < 0.001
        );
    }

    #[test]
    fn degree_click_resets_exactly_without_jitter_and_dragging_still_moves() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let ruler = geometry(23.0_f32.to_radians(), scale);
            let press = POINT {
                x: ruler.center.x as i32,
                y: ruler.center.y as i32,
            };
            let jitter = POINT {
                x: press.x + scale.round() as i32,
                y: press.y,
            };
            let mut click = Drag::start(ruler, press, HitTarget::Reset);
            let interim = click.update(ruler, jitter, false);
            assert_eq!(interim.center, ruler.center);
            let reset = click.finish(interim, jitter, false);
            assert_eq!(reset.center, ruler.center);
            assert_eq!(reset.angle, 0.0);
            let mut drag = Drag::start(ruler, press, HitTarget::Reset);
            let pulled = POINT {
                x: press.x + (20.0 * scale) as i32,
                y: press.y,
            };
            let moved = drag.update(ruler, pulled, false);
            assert_eq!(moved.angle, ruler.angle);
            assert!(moved.center.x > ruler.center.x);
            assert_eq!(drag.finish(moved, pulled, false).angle, ruler.angle);
        }
    }

    #[test]
    fn native_rotation_control_click_advances_fifteen_degrees_and_updates_readout() {
        let ruler = Ruler::new(ptr::null_mut(), ToolbarTheme::LiquidAir).unwrap();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let geometry = geometry(0.0, scale);
            ruler.state.geometry.set(geometry);
            let [left, _, width, _] = geometry.rotation_bounds();
            let point = geometry.screen(left + width * 0.5, 0.0);
            let mut point = POINT {
                x: point.x.round() as i32,
                y: point.y.round() as i32,
            };
            unsafe {
                ruler.state.paint(ruler.handle()).unwrap();
                ScreenToClient(ruler.handle(), &mut point);
                let position =
                    (u32::from(point.x as u16) | (u32::from(point.y as u16) << 16)) as isize;
                SendMessageW(ruler.handle(), WM_LBUTTONDOWN, 1, position);
                assert!(matches!(ruler.state.drag.get(), Some(Drag::Rotate { .. })));
                SendMessageW(ruler.handle(), WM_LBUTTONUP, 0, position);
                let mut title = [0_u16; 80];
                let length = GetWindowTextW(ruler.handle(), title.as_mut_ptr(), title.len() as i32);
                assert_eq!(
                    String::from_utf16_lossy(&title[..length as usize]),
                    "Study Pen ruler — 15.0°"
                );
                let angle_geometry = ruler.state.geometry.get();
                let mut center = POINT {
                    x: angle_geometry.center.x as i32,
                    y: angle_geometry.center.y as i32,
                };
                ScreenToClient(ruler.handle(), &mut center);
                let center =
                    (u32::from(center.x as u16) | (u32::from(center.y as u16) << 16)) as isize;
                SendMessageW(ruler.handle(), WM_LBUTTONDOWN, 1, center);
                SendMessageW(ruler.handle(), WM_LBUTTONUP, 0, center);
                assert_eq!(ruler.state.geometry.get().angle, 0.0);
                let length = GetWindowTextW(ruler.handle(), title.as_mut_ptr(), title.len() as i32);
                assert_eq!(
                    String::from_utf16_lossy(&title[..length as usize]),
                    "Study Pen ruler — 0.0°"
                );
            }
            assert_eq!(ruler.state.geometry.get().angle, 0.0);
            assert!(ruler.state.drag.get().is_none());
            assert!(!ruler.is_visible());
        }
    }

    #[test]
    fn native_shift_rotation_preserves_a_near_zero_angle_and_small_wheel_steps_accumulate() {
        let ruler = Ruler::new(ptr::null_mut(), ToolbarTheme::LiquidAir).unwrap();
        for precise in [false, true] {
            let initial = geometry(15.0_f32.to_radians(), 1.0);
            let [left, _, width, _] = initial.rotation_bounds();
            let local_x = left + width * 0.5;
            ruler.state.geometry.set(initial);
            unsafe {
                ruler.state.paint(ruler.handle()).unwrap();
                let position = |geometry: Geometry| {
                    let point = geometry.screen(local_x, 0.0);
                    let mut point = POINT {
                        x: point.x.round() as i32,
                        y: point.y.round() as i32,
                    };
                    ScreenToClient(ruler.handle(), &mut point);
                    (u32::from(point.x as u16) | (u32::from(point.y as u16) << 16)) as isize
                };
                SendMessageW(ruler.handle(), WM_LBUTTONDOWN, 1, position(initial));
                let target = Geometry {
                    angle: 0.5_f32.to_radians(),
                    ..initial
                };
                SendMessageW(
                    ruler.handle(),
                    WM_LBUTTONUP,
                    if precise { MK_SHIFT_FLAG } else { 0 },
                    position(target),
                );
            }
            let angle = ruler.state.geometry.get().angle.to_degrees();
            if precise {
                assert!((0.2..0.8).contains(&angle));
            } else {
                assert_eq!(angle, 0.0);
            }
        }
        let mut geometry = ruler.state.geometry.get();
        geometry.angle = 0.0;
        ruler.state.geometry.set(geometry);
        unsafe {
            for _ in 0..4 {
                SendMessageW(ruler.handle(), WM_MOUSEWHEEL, (15_u32 << 16) as usize, 0);
            }
        }
        assert!((ruler.state.geometry.get().angle.to_degrees() - 2.5).abs() < 0.001);
    }

    #[test]
    fn rotation_icon_and_native_degree_text_keep_antialiased_edges_at_common_dpi() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let side = (40.0 * scale) as u32;
            let mut icon = tiny_skia::Pixmap::new(side, side).unwrap();
            rotation_icon(
                &mut icon.as_mut(),
                0.0,
                8.0 * scale,
                [20, 40, 80, 255],
                scale,
                Transform::from_translate(20.0 * scale, 20.0 * scale),
            );
            let icon_coverage = icon
                .data()
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|pixel| (1..255).contains(&pixel[3]))
                .count();
            assert!(icon_coverage > 12, "rotation icon scale {scale}");
            let mut label =
                tiny_skia::Pixmap::new((80.0 * scale) as u32, (30.0 * scale) as u32).unwrap();
            unsafe {
                let mut text = TextCache::new(scale).unwrap();
                text.draw(
                    &mut label.as_mut(),
                    "34.5°",
                    Point::new(40.0 * scale, 15.0 * scale),
                    [70.0 * scale, 26.0 * scale],
                    0.0,
                    [20, 40, 80, 255],
                );
            }
            assert!(
                label
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|pixel| (1..255).contains(&pixel[3])),
                "angle text scale {scale}"
            );
        }
    }

    #[test]
    fn degree_readout_renders_identical_zeroes_and_an_open_degree_mark() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let width = (80.0 * scale) as usize;
            let height = (40.0 * scale) as usize;
            let mut label = tiny_skia::Pixmap::new(width as u32, height as u32).unwrap();
            let mut advance = SIZE::default();
            let mut zero = ABC::default();
            unsafe {
                let mut text = TextCache::new(scale).unwrap();
                let dc = text.surface.dc;
                let previous = SelectObject(dc, text.upright_font.0);
                let prefix = [u16::from(b'0'), u16::from(b'.')];
                let measured_advance =
                    GetTextExtentPoint32W(dc, prefix.as_ptr(), prefix.len() as i32, &mut advance);
                let measured_zero =
                    GetCharABCWidthsW(dc, u32::from(b'0'), u32::from(b'0'), &mut zero);
                SelectObject(dc, previous);
                assert_ne!(measured_advance, 0);
                assert_ne!(measured_zero, 0);
                assert!(zero.abcB > 0);
                text.draw_angle(
                    &mut label.as_mut(),
                    0.0,
                    Point::new(40.0 * scale, 20.0 * scale),
                    [57.6 * scale, 26.0 * scale],
                    [20, 40, 80, 255],
                );
            }
            let alpha_at = |x, y| label.data()[(y * width + x) * 4 + 3];
            let first = (0..width)
                .find(|&x| (0..height).any(|y| alpha_at(x, y) != 0))
                .expect("angle readout should contain visible glyphs");
            let second = first + advance.cx as usize;
            for offset in 0..zero.abcB as usize {
                for y in 0..height {
                    assert_eq!(
                        alpha_at(first + offset, y),
                        alpha_at(second + offset, y),
                        "zero coverage at scale {scale}"
                    );
                }
            }
            let filled_column = |x| (0..height).any(|y| alpha_at(x, y) != 0);
            let ring_right = (0..width).rfind(|x| filled_column(*x)).unwrap();
            let ring_left = (0..ring_right).rfind(|x| !filled_column(*x)).unwrap() + 1;
            assert!(ring_left > second + zero.abcB as usize - 1);
            assert!(ring_right - ring_left >= 3);
            let ring_x = (ring_left + ring_right) / 2;
            let ring_y = (16.5 * scale).floor() as usize;
            assert_eq!(alpha_at(ring_x, ring_y), 0);
            assert!((ring_left..ring_x).any(|x| alpha_at(x, ring_y) > 0));
            assert!((ring_x + 1..=ring_right).any(|x| alpha_at(x, ring_y) > 0));
        }
    }

    #[test]
    fn rotated_native_labels_retain_the_complete_word_and_restore_upright_state() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let mut text = unsafe { TextCache::new(scale).unwrap() };
            let render = |text: &mut TextCache, angle| {
                let mut pixels =
                    tiny_skia::Pixmap::new((120.0 * scale) as u32, (90.0 * scale) as u32).unwrap();
                unsafe {
                    text.draw(
                        &mut pixels.as_mut(),
                        "Rotate",
                        Point::new(60.0 * scale, 45.0 * scale),
                        [62.0 * scale, 28.0 * scale],
                        angle,
                        [15, 25, 44, 255],
                    );
                }
                pixels
            };
            let upright = render(&mut text, 0.0);
            let coverage = |pixels: &tiny_skia::Pixmap| {
                pixels
                    .data()
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|pixel| u64::from(pixel[3]))
                    .sum::<u64>()
            };
            let normal = coverage(&upright);
            assert!(normal > 0);
            for degrees in [-75.0_f32, -17.0, 17.0, 45.0, 90.0] {
                let rotated = render(&mut text, degrees.to_radians());
                assert!(
                    coverage(&rotated) > normal * 3 / 4,
                    "clipped Rotate at {degrees}°, scale {scale}"
                );
                let after = render(&mut text, 0.0);
                assert_eq!(after.data(), upright.data());
            }
        }
    }

    #[test]
    fn native_surfaces_keep_translucent_premultiplied_pixels_in_both_themes() {
        unsafe {
            for theme in ToolbarTheme::ALL {
                for scale in [1.0, 1.25, 1.5, 2.0] {
                    let geometry = geometry(17.0_f32.to_radians(), scale);
                    let bounds = geometry.bounds();
                    let mut surface =
                        Surface::new(bounds.right - bounds.left, bounds.bottom - bounds.top)
                            .unwrap();
                    let mut text = TextCache::new(geometry.scale).unwrap();
                    paint_surface(&mut surface, &mut text, geometry, theme, bounds).unwrap();
                    let bytes = std::slice::from_raw_parts(
                        surface.bits,
                        surface.width as usize * surface.height as usize * 4,
                    );
                    assert!(
                        bytes
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .any(|pixel| (1..255).contains(&pixel[3]))
                    );
                    assert!(bytes[..4].iter().all(|channel| *channel == 0));
                    for pixel in bytes.as_chunks::<4>().0 {
                        assert!(pixel[..3].iter().all(|channel| *channel <= pixel[3]));
                    }
                    if theme == ToolbarTheme::LiquidAir {
                        let sample = |x| {
                            let point = geometry.screen(x, 8.0 * scale);
                            let x = (point.x - bounds.left as f32).round() as usize;
                            let y = (point.y - bounds.top as f32).round() as usize;
                            let offset = (y * surface.width as usize + x) * 4;
                            &bytes[offset..offset + 4]
                        };
                        let left = sample(-geometry.length * 0.25);
                        let right = sample(geometry.length * 0.25);
                        assert!(left[0] > left[2] + 20 && right[0] > right[2] + 20);
                        assert!(left[1] > right[1] + 10);
                        assert!((129..255).contains(&left[3]));
                    }
                }
            }
        }
    }

    #[test]
    fn native_window_stays_hidden_and_allocates_no_bitmap_until_shown() {
        let first = Ruler::new(ptr::null_mut(), ToolbarTheme::LiquidGlass).unwrap();
        let second = Ruler::new(ptr::null_mut(), ToolbarTheme::LiquidAir).unwrap();
        assert!(!first.is_visible());
        assert!(!second.is_visible());
        unsafe {
            assert!(
                !first.state.tips.get().is_null(),
                "tooltip creation error {}",
                GetLastError()
            );
            assert_eq!(
                SendMessageW(
                    first.state.tips.get(),
                    windows_sys::Win32::UI::Controls::TTM_GETTOOLCOUNT,
                    0,
                    0
                ),
                2
            );
        }
        assert!(first.state.surface.borrow().is_none());
        first.set_theme(ToolbarTheme::LiquidAir).unwrap();
        assert!(first.state.surface.borrow().is_none());
        assert!(first.snap(POINT { x: 0, y: 0 }).is_none());
        first.state.geometry.set(geometry(0.0, 1.0));
        unsafe {
            first.state.paint(first.handle()).unwrap();
            assert!(!first.is_visible());
            assert!(first.state.surface.borrow().is_some());
            SendMessageW(first.handle(), WM_MOUSEWHEEL, (120_u32 << 16) as usize, 0);
        }
        assert!((first.state.geometry.get().angle.to_degrees() - 5.0).abs() < 0.001);
        assert!(!first.is_visible());
        first.hide();
        assert!(first.state.surface.borrow().is_none());
        let mut geometry = first.state.geometry.get();
        geometry.center = Point::new(-50000.0, -50000.0);
        first.state.geometry.set(geometry);
        first.state.drag.set(Some(Drag::Move {
            pointer: POINT::default(),
            center: geometry.center,
            reset_on_click: false,
            moved: false,
        }));
        unsafe { first.state.fit_to_monitor(1.25) };
        assert_eq!(first.state.geometry.get().center, geometry.center);
        first.state.drag.set(None);
        unsafe { first.state.fit_to_monitor(1.25) };
        assert_ne!(first.state.geometry.get().center, geometry.center);
    }
}
