//! Native Windows shell. The message loop sleeps whenever there is no work.

use crate::{
    docking::{self, Bounds, Edge},
    glass::Glass,
    hotkeys, identity,
    ink::{Color, Ink, Point, Tool, eraser_diameter},
    install_update,
    overlay::{Rect, Renderer},
    radial::{self, WheelStage, WheelState},
    removal::{RemovalImage, RemovalPlan},
    settings::{Action, Hotkey, Settings},
    startup_update::{self, Launch},
    toolbar_drag::{DragGeometry, Point as DragPoint},
    toolbar_motion::DockAnimation,
    tray::{self, Tray},
    ui::{
        self, PANEL_HEIGHT, PANEL_WIDTH, SHORTCUT_HEIGHT, SHORTCUT_STEP, SHORTCUT_TOP,
        ToolbarControls, ToolbarLayout, UPDATE_BUTTON_BOTTOM, UPDATE_BUTTON_TOP, UiState,
    },
    updates::{self, ReleaseInfo, UpdateStatus},
};
use std::{
    cell::RefCell,
    ffi::c_void,
    mem::{size_of, zeroed},
    ptr,
    sync::mpsc,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{
        Controls::{
            EM_SETLIMITTEXT, EM_SETSEL, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX,
            InitCommonControlsEx, TOOLTIPS_CLASSW, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_NEWTOOLRECTW,
            TTM_POP, TTS_ALWAYSTIP, TTTOOLINFOW, WM_MOUSELEAVE,
        },
        HiDpi::*,
        Input::KeyboardAndMouse::*,
        Shell::ShellExecuteW,
        WindowsAndMessaging::*,
    },
};

const TOOLBAR_CLASS: &str = "StudyPen.Toolbar";
const PANEL_CLASS: &str = "StudyPen.Settings";
const OVERLAY_CLASS: &str = "StudyPen.Ink";
const RADIAL_CLASS: &str = "StudyPen.Radial";
const TIMER_ERASER: usize = 1;
const TIMER_UPDATES: usize = 2;
const TIMER_DAILY: usize = 3;
const TIMER_SAVE: usize = 4;
const TIMER_TRAY: usize = 5;
const TIMER_CAPTURE_RELEASE: usize = 6;
const TIMER_RADIAL: usize = 7;
const TIMER_DOCK: usize = 8;
const TIMER_GLASS: usize = 9;
const TIMER_INSTALL_UPDATE: usize = 10;
const DAILY_UPDATE_MS: u32 = 86_400_000;
const GLASS_REFRESH_MS: u32 = 100;
const REMOVE_REQUEST: u32 = WM_APP + 20;
const UPDATE_REQUEST: u32 = WM_APP + 21;
const RADIAL_DWELL: Duration = Duration::from_millis(90);
const UNAVAILABLE_SHORTCUTS: &str = "Shortcut unavailable. Select its amber row to change it.";
const EDIT_HEX: usize = 200;
const EDIT_BLUE: usize = 203;

thread_local! { static APP: RefCell<Option<App>> = const { RefCell::new(None) }; }

struct EraserHold {
    key: u32,
    start_y: i32,
    initial: f32,
    started: Instant,
    resizing: bool,
}

/// One selection per hold. The color page resets its dwell after recentering.
struct RadialHold {
    key: u32,
    center: POINT,
    scale: f32,
    state: WheelState,
    renderer: radial::Renderer,
    dwell: HoverDwell,
}

struct ToolbarTips {
    window: HWND,
    labels: [Vec<u16>; ui::TOOLBAR_TOOLTIPS.len()],
}

impl ToolbarTips {
    unsafe fn new(toolbar: HWND, instance: HINSTANCE, layout: ToolbarLayout, scale: f32) -> Self {
        InitCommonControlsEx(&INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        });
        let window = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_NOACTIVATE,
            TOOLTIPS_CLASSW,
            ptr::null(),
            WS_POPUP | TTS_ALWAYSTIP,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            toolbar,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        let mut tips = Self {
            window,
            labels: ui::TOOLBAR_TOOLTIPS.map(wide),
        };
        tips.set_bounds(toolbar, layout, scale, TTM_ADDTOOLW);
        tips
    }

    unsafe fn set_bounds(
        &mut self,
        toolbar: HWND,
        layout: ToolbarLayout,
        scale: f32,
        message: u32,
    ) {
        if self.window.is_null() {
            return;
        }
        let px = |value: i32| (value as f32 * scale).round() as i32;
        for (index, label) in self.labels.iter_mut().enumerate() {
            let Some((x, y, w, h)) = layout.button_bounds(index) else {
                continue;
            };
            let tool = TTTOOLINFOW {
                cbSize: size_of::<TTTOOLINFOW>() as u32,
                uFlags: TTF_SUBCLASS,
                hwnd: toolbar,
                uId: index + 1,
                rect: RECT {
                    left: px(x),
                    top: px(y),
                    right: px(x + w),
                    bottom: px(y + h),
                },
                lpszText: label.as_mut_ptr(),
                ..Default::default()
            };
            SendMessageW(
                self.window,
                message,
                0,
                &tool as *const TTTOOLINFOW as isize,
            );
        }
    }

    unsafe fn hide(&self) {
        if !self.window.is_null() {
            SendMessageW(self.window, TTM_POP, 0, 0);
        }
    }
}

impl Drop for ToolbarTips {
    fn drop(&mut self) {
        if !self.window.is_null() {
            unsafe { DestroyWindow(self.window) };
        }
    }
}

#[derive(Default)]
struct HoverDwell {
    candidate: Option<usize>,
    since: Option<Instant>,
}

impl HoverDwell {
    fn sample(&mut self, candidate: Option<usize>, now: Instant) -> Option<usize> {
        if self.candidate != candidate {
            self.candidate = candidate;
            self.since = candidate.map(|_| now);
            return None;
        }
        if candidate.is_some()
            && self
                .since
                .is_some_and(|since| now.duration_since(since) >= RADIAL_DWELL)
        {
            self.since = None;
            candidate
        } else {
            None
        }
    }
}

#[derive(Clone, Copy)]
enum PanelDrag {
    SaturationValue,
    Hue,
    PenWidth,
    EraserWidth,
}

struct App {
    toolbar: HWND,
    panel: HWND,
    overlay: HWND,
    radial_window: HWND,
    edits: [HWND; 4],
    edit_font: HFONT,
    edit_brush: HBRUSH,
    tray: Tray,
    ui_visible: bool,
    toolbar_layout: ToolbarLayout,
    toolbar_scale: f32,
    toolbar_drag: Option<DragGeometry>,
    toolbar_animation: Option<DockAnimation>,
    toolbar_clipped: bool,
    glass: Glass,
    toolbar_tips: ToolbarTips,
    toolbar_pointer: Option<POINT>,
    renderer: Option<Renderer>,
    settings: Settings,
    ink: Ink,
    scale: f32,
    panel_scale: f32,
    origin: POINT,
    interactive: bool,
    tool: Tool,
    last_point: Option<Point>,
    gesture_max_width: f32,
    hold: Option<EraserHold>,
    radial_hold: Option<RadialHold>,
    cursor: HCURSOR,
    tab: usize,
    editing_secondary: bool,
    hsv: (f32, f32, f32),
    capturing: Option<usize>,
    pending_shortcut_release: Option<u32>,
    hotkey_errors: [Option<String>; 9],
    hover: Option<usize>,
    panel_drag: Option<PanelDrag>,
    status: String,
    update_status: UpdateStatus,
    update_receiver: Option<mpsc::Receiver<UpdateStatus>>,
    update_install_receiver: Option<mpsc::Receiver<Result<(), String>>>,
    update_dialog_open: bool,
    update_error: Option<String>,
    syncing_edits: bool,
    removal_image: Result<RemovalImage, String>,
    removal_dialog_open: bool,
    removing: bool,
}

pub fn run() -> Result<(), String> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mutex = CreateMutexW(
            ptr::null(),
            0,
            wide("Local\\StudyPen.SingleInstance").as_ptr(),
        );
        if mutex.is_null() {
            return Err("Could not create the application instance.".into());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let existing = FindWindowW(wide(TOOLBAR_CLASS).as_ptr(), ptr::null());
            if !existing.is_null() {
                PostMessageW(existing, tray::RESTORE_MESSAGE, 0, 0);
            }
            CloseHandle(mutex);
            return Ok(());
        }
        let result = run_instance();
        CloseHandle(mutex);
        result
    }
}

unsafe fn run_instance() -> Result<(), String> {
    let removal_image = RemovalImage::capture();
    let identity_error = identity::set_process_identity().err();
    let instance = GetModuleHandleW(ptr::null());
    let mut settings = Settings::load();
    let (update_status, update_error, check_in_background) =
        match startup_update::run(&settings, instance) {
            Launch::BackgroundCheck => (UpdateStatus::Unconfigured, None, true),
            Launch::Open {
                status,
                install_error,
            } => (status, install_error, false),
            Launch::Exit => return Ok(()),
        };
    let shortcut_error = if cfg!(debug_assertions) {
        None
    } else {
        match identity::ensure_start_menu_shortcut() {
            Ok(identity::ShortcutStatus::NameOccupied) => {
                Some("A different shortcut already uses the Study Pen name.".into())
            }
            Ok(_) => None,
            Err(error) => Some(error),
        }
    };
    for name in [TOOLBAR_CLASS, PANEL_CLASS, OVERLAY_CLASS, RADIAL_CLASS] {
        let class_name = wide(name);
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            lpszClassName: class_name.as_ptr(),
            ..zeroed()
        };
        if RegisterClassW(&class) == 0 {
            return Err("Could not register the application windows.".into());
        }
    }
    let scale = (GetDpiForSystem() as f32 / 96.0).max(1.0);
    let px = |n: i32| (n as f32 * scale).round() as i32;
    let desktop_error = if cfg!(debug_assertions) || settings.desktop_shortcut_seen {
        None
    } else {
        match identity::ensure_desktop_shortcut() {
            Ok(status) => {
                settings.desktop_shortcut_seen = true;
                if status == identity::ShortcutStatus::NameOccupied {
                    Some(
                        "A different desktop item already uses the Study Pen shortcut name.".into(),
                    )
                } else {
                    None
                }
            }
            Err(error) => Some(error),
        }
    };
    let toolbar_layout = if settings.toolbar_vertical {
        ToolbarLayout::Vertical
    } else {
        ToolbarLayout::Horizontal
    };
    let (toolbar_width, toolbar_height) = toolbar_layout.size();
    let origin = POINT {
        x: GetSystemMetrics(SM_XVIRTUALSCREEN),
        y: GetSystemMetrics(SM_YVIRTUALSCREEN),
    };
    let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
    let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    let overlay = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        wide(OVERLAY_CLASS).as_ptr(),
        wide("Study Pen Ink").as_ptr(),
        WS_POPUP,
        origin.x,
        origin.y,
        width,
        height,
        ptr::null_mut(),
        ptr::null_mut(),
        instance,
        ptr::null(),
    );
    if overlay.is_null() {
        return Err("Could not create the ink window.".into());
    }
    let mut work: RECT = zeroed();
    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut _ as *mut c_void, 0);
    let (saved_x, saved_y) = settings.toolbar_position.unwrap_or((
        work.left + (work.right - work.left - px(toolbar_width)) / 2,
        work.bottom - px(110),
    ));
    let work = monitor_work_area(POINT {
        x: saved_x,
        y: saved_y,
    });
    let toolbar_scale = fitted_toolbar_scale(toolbar_layout, scale, work);
    let toolbar_px = |n: i32| (n as f32 * toolbar_scale).round() as i32;
    let (x, y) = constrain_toolbar_position(
        saved_x,
        saved_y,
        toolbar_px(toolbar_width),
        toolbar_px(toolbar_height),
        work,
    );
    let toolbar = CreateWindowExW(
        WS_EX_TOPMOST | WS_EX_APPWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
        wide(TOOLBAR_CLASS).as_ptr(),
        wide("Study Pen").as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN,
        x,
        y,
        toolbar_px(toolbar_width),
        toolbar_px(toolbar_height),
        ptr::null_mut(),
        ptr::null_mut(),
        instance,
        ptr::null(),
    );
    let panel = CreateWindowExW(
        WS_EX_TOPMOST,
        wide(PANEL_CLASS).as_ptr(),
        wide("Study Pen Settings").as_ptr(),
        WS_POPUP | WS_CLIPCHILDREN,
        x,
        (y - px(636)).max(work.top + px(16)),
        px(PANEL_WIDTH),
        px(PANEL_HEIGHT),
        toolbar,
        ptr::null_mut(),
        instance,
        ptr::null(),
    );
    if toolbar.is_null() || panel.is_null() {
        return Err("Could not create the controls.".into());
    }
    let radial_window = CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        wide(RADIAL_CLASS).as_ptr(),
        wide("Study Pen Radial Menu").as_ptr(),
        WS_POPUP,
        0,
        0,
        1,
        1,
        ptr::null_mut(),
        ptr::null_mut(),
        instance,
        ptr::null(),
    );
    if radial_window.is_null() {
        return Err("Could not create the radial menu window.".into());
    }
    rounded_region(panel, px(PANEL_WIDTH), px(PANEL_HEIGHT), px(20));
    let edit_font = create_edit_font(scale);
    let edits = std::array::from_fn(|index| {
        let x_positions = [24, 176, 264, 352];
        let widths = [128, 72, 72, 84];
        let style = WS_CHILD
            | WS_TABSTOP
            | ES_AUTOHSCROLL as u32
            | if index > 0 { ES_NUMBER as u32 } else { 0 };
        let edit = CreateWindowExW(
            0,
            wide("EDIT").as_ptr(),
            ptr::null(),
            style,
            px(x_positions[index]),
            px(448),
            px(widths[index]),
            px(32),
            panel,
            (EDIT_HEX + index) as HMENU,
            instance,
            ptr::null(),
        );
        SendMessageW(edit, WM_SETFONT, edit_font as usize, 0);
        SendMessageW(edit, EM_SETLIMITTEXT, if index == 0 { 7 } else { 3 }, 0);
        edit
    });
    let primary = Color::from_hex(&settings.primary_color).unwrap_or(Color::rgb(99, 102, 241));
    let mut tray = Tray::new(toolbar)?;
    let ui_visible = settings.toolbar_visible || !tray.is_added();
    settings.toolbar_visible = ui_visible;
    tray.update_visibility(ui_visible);
    let app = App {
        toolbar,
        panel,
        overlay,
        radial_window,
        edits,
        edit_font,
        edit_brush: CreateSolidBrush(rgb(34, 41, 46)),
        tray,
        ui_visible,
        toolbar_layout,
        toolbar_scale,
        toolbar_drag: None,
        toolbar_animation: None,
        toolbar_clipped: false,
        glass: Glass::default(),
        toolbar_tips: ToolbarTips::new(toolbar, instance, toolbar_layout, toolbar_scale),
        toolbar_pointer: None,
        renderer: None,
        settings,
        ink: Ink::new(),
        scale,
        panel_scale: scale,
        origin,
        interactive: false,
        tool: Tool::Pen,
        last_point: None,
        gesture_max_width: 0.0,
        hold: None,
        radial_hold: None,
        cursor: ptr::null_mut(),
        tab: 0,
        editing_secondary: false,
        hsv: ui::rgb_to_hsv(primary),
        capturing: None,
        pending_shortcut_release: None,
        hotkey_errors: std::array::from_fn(|_| None),
        hover: None,
        panel_drag: None,
        status: desktop_error
            .or(shortcut_error)
            .or(identity_error)
            .unwrap_or_default(),
        update_status,
        update_receiver: None,
        update_install_receiver: None,
        update_dialog_open: false,
        update_error,
        syncing_edits: false,
        removal_image,
        removal_dialog_open: false,
        removing: false,
    };
    APP.with(|cell| *cell.borrow_mut() = Some(app));
    APP.with(|cell| {
        let mut guard = cell.borrow_mut();
        let app = guard.as_mut().unwrap();
        app.restore_toolbar_geometry(Some((saved_x, saved_y)));
        app.register_hotkeys();
        app.sync_edits(None);
        app.render(None);
        if check_in_background {
            app.start_updates();
        } else if app.settings.check_for_updates
            && updates::repository(app.settings.github_repository.as_deref()).is_some()
        {
            SetTimer(toolbar, TIMER_DAILY, DAILY_UPDATE_MS, None);
        }
        app.ensure_tray();
        app.save();
        if ui_visible {
            app.paint_glass();
            SetTimer(toolbar, TIMER_GLASS, GLASS_REFRESH_MS, None);
        }
    });
    if ui_visible {
        ShowWindow(toolbar, SW_SHOWNOACTIVATE);
    }
    APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut().unwrap();
        if app.hotkey_errors.iter().any(Option::is_some) {
            app.open_panel(1);
        }
    });
    let mut message: MSG = zeroed();
    loop {
        let result = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
        if result <= 0 {
            break;
        }
        let consumed = APP.with(|cell| {
            let mut guard = cell.borrow_mut();
            let app = guard.as_mut().unwrap();
            if hotkeys::blocks_menu_beep(message.message, message.wParam) {
                true
            } else if matches!(message.message, WM_KEYDOWN | WM_SYSKEYDOWN) {
                app.key_down(message.wParam as u32)
            } else if matches!(message.message, WM_KEYUP | WM_SYSKEYUP) {
                app.key_up(message.wParam as u32);
                false
            } else {
                false
            }
        });
        if !consumed {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    APP.with(|cell| {
        cell.borrow_mut().take();
    });
    Ok(())
}

impl App {
    fn primary(&self) -> Color {
        Color::from_hex(&self.settings.primary_color).unwrap_or_default()
    }
    fn secondary(&self) -> Color {
        Color::from_hex(&self.settings.secondary_color).unwrap_or_default()
    }
    fn edited_color(&self) -> Color {
        if self.editing_secondary {
            self.secondary()
        } else {
            self.primary()
        }
    }
    fn ui_state(&self) -> UiState {
        UiState {
            primary: self.primary(),
            secondary: self.secondary(),
            tab: self.tab,
            editing_secondary: self.editing_secondary,
            hue: self.hsv.0,
            saturation: self.hsv.1,
            value: self.hsv.2,
            hotkeys: self.settings.hotkeys.map(|key| key.display()),
            hotkey_errors: self.hotkey_errors.clone(),
            capturing: self.capturing,
            check_for_updates: self.settings.check_for_updates,
            installing_update: self.update_install_receiver.is_some(),
            update_message: if self.update_install_receiver.is_some() {
                "Downloading and verifying the update…".into()
            } else if !self.settings.check_for_updates {
                "Automatic updates are off.".into()
            } else if self.update_receiver.is_some() {
                "Checking GitHub for updates…".into()
            } else if let Some(error) = &self.update_error {
                error.clone()
            } else {
                self.update_status.message()
            },
            update_action: self.available_update().map(|(release, installable)| {
                if installable {
                    "Install update"
                } else if release.asset.is_some() {
                    "Download update"
                } else {
                    "View release"
                }
            }),
            status: if self.status.is_empty() {
                self.settings.persistence_warning().unwrap_or_default()
            } else {
                self.status.clone()
            },
            storage_message: self.settings.persistence_description(),
            pen_width: self.settings.pen_width,
            eraser_diameter: self.settings.eraser_diameter,
        }
    }

    unsafe fn paint_glass(&mut self) {
        if !self.ui_visible {
            return;
        }
        let mut bounds = RECT::default();
        if GetWindowRect(self.toolbar, &mut bounds) == 0 {
            return;
        }
        let controls = ToolbarControls {
            layout: self.toolbar_layout,
            interactive: self.interactive,
            tool: self.tool,
            primary: self.primary(),
            hover: self.hover,
            dark: false,
        };
        self.glass.render(
            self.toolbar,
            bounds,
            self.toolbar_scale,
            controls,
            self.toolbar_pointer,
        );
    }

    unsafe fn redraw_ui(&self) {
        InvalidateRect(self.toolbar, ptr::null(), 0);
        if IsWindowVisible(self.panel) != 0 {
            InvalidateRect(self.panel, ptr::null(), 0);
        }
    }

    unsafe fn apply_ui_scale(&mut self, scale: f32) {
        if (self.scale - scale).abs() < 0.001 {
            return;
        }
        self.scale = scale;
        if IsWindowVisible(self.panel) == 0 {
            self.apply_panel_scale(scale);
        }
        self.update_cursor();
    }

    unsafe fn apply_panel_scale(&mut self, scale: f32) {
        if (self.panel_scale - scale).abs() < 0.001 {
            return;
        }
        self.panel_scale = scale;
        SetWindowPos(
            self.panel,
            ptr::null_mut(),
            0,
            0,
            self.px(PANEL_WIDTH),
            self.px(PANEL_HEIGHT),
            SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
        rounded_region(
            self.panel,
            self.px(PANEL_WIDTH),
            self.px(PANEL_HEIGHT),
            self.px(20),
        );
        let font = create_edit_font(scale);
        if !font.is_null() {
            for edit in self.edits {
                SendMessageW(edit, WM_SETFONT, font as usize, 1);
            }
            DeleteObject(self.edit_font);
            self.edit_font = font;
        }
        for (index, edit) in self.edits.into_iter().enumerate() {
            SetWindowPos(
                edit,
                ptr::null_mut(),
                self.px([24, 176, 264, 352][index]),
                self.px(448),
                self.px([128, 72, 72, 84][index]),
                self.px(32),
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    unsafe fn place_toolbar(&mut self, x: i32, y: i32, work: RECT) {
        let scale = fitted_toolbar_scale(self.toolbar_layout, self.scale, work);
        let (width, height) = self.toolbar_layout.size();
        let px = |n: i32| (n as f32 * scale).round() as i32;
        let (width, height) = (px(width), px(height));
        let (x, y) = constrain_toolbar_position(x, y, width, height, work);
        self.apply_toolbar_frame(
            self.toolbar_layout,
            scale,
            Bounds::new(x, y, x + width, y + height),
            None,
        );
    }

    unsafe fn place_horizontal_drag_preview(&mut self, proposal: Bounds, work: RECT) {
        let scale = fitted_toolbar_scale(ToolbarLayout::Horizontal, self.scale, work);
        let bounds = horizontal_drag_preview(proposal, work, self.scale);
        let clip = (bounds.right > work.right).then_some(Bounds::new(
            work.left,
            work.top,
            work.right,
            work.bottom,
        ));
        self.apply_toolbar_frame(ToolbarLayout::Horizontal, scale, bounds, clip);
    }

    unsafe fn apply_toolbar_frame(
        &mut self,
        layout: ToolbarLayout,
        scale: f32,
        bounds: Bounds,
        clip: Option<Bounds>,
    ) {
        let mut old = RECT::default();
        GetWindowRect(self.toolbar, &mut old);
        let width = bounds.right - bounds.left;
        let height = bounds.bottom - bounds.top;
        let changed = old.right - old.left != width
            || old.bottom - old.top != height
            || self.toolbar_layout != layout
            || (self.toolbar_scale - scale).abs() >= 0.001;
        self.toolbar_layout = layout;
        self.toolbar_scale = scale;
        let moved = old.left != bounds.left || old.top != bounds.top;
        if changed || moved {
            SetWindowPos(
                self.toolbar,
                HWND_TOPMOST,
                bounds.left,
                bounds.top,
                width,
                height,
                SWP_NOACTIVATE,
            );
        }
        if changed || clip.is_some() || self.toolbar_clipped {
            toolbar_region(self.toolbar, bounds, clip);
        }
        self.toolbar_clipped = clip.is_some();
        if changed {
            self.toolbar_tips
                .set_bounds(self.toolbar, layout, scale, TTM_NEWTOOLRECTW);
        }
        if changed || moved || clip.is_some() {
            self.paint_glass();
        }
    }

    unsafe fn remember_toolbar(&mut self) {
        if let Some(animation) = &self.toolbar_animation {
            let target = animation.target();
            self.settings.toolbar_position = Some((target.left, target.top));
            self.settings.toolbar_vertical = true;
            return;
        }
        let mut bounds = RECT::default();
        if GetWindowRect(self.toolbar, &mut bounds) != 0 {
            self.settings.toolbar_position = Some((bounds.left, bounds.top));
            self.settings.toolbar_vertical = self.toolbar_layout == ToolbarLayout::Vertical;
        }
    }

    unsafe fn restore_toolbar_geometry(&mut self, requested: Option<(i32, i32)>) {
        self.finish_toolbar_animation();
        let mut bounds = RECT::default();
        if GetWindowRect(self.toolbar, &mut bounds) == 0 {
            return;
        }
        let dpi = GetDpiForWindow(self.toolbar);
        if dpi != 0 {
            self.apply_ui_scale((dpi as f32 / 96.0).max(1.0));
        }
        // Startup may have temporarily clamped a system-DPI-sized window on
        // another monitor. Keep its saved intent until actual DPI is known.
        let (x, y) = requested.unwrap_or((bounds.left, bounds.top));
        let work = monitor_work_area(POINT { x, y });
        self.place_toolbar(x, y, work);
        self.remember_toolbar();
    }

    unsafe fn begin_toolbar_drag(&mut self, mut pointer: POINT) {
        self.finish_toolbar_animation();
        self.toolbar_tips.hide();
        self.toolbar_pointer = None;
        self.hover = None;
        self.close_radial();
        self.finish_gesture();
        self.stop_hold();
        if IsWindowVisible(self.panel) != 0 {
            self.close_panel();
        }
        let mut bounds = RECT::default();
        if ClientToScreen(self.toolbar, &mut pointer) == 0
            || GetWindowRect(self.toolbar, &mut bounds) == 0
        {
            return;
        }
        // Anchor to the queued button-down position, even if the physical
        // pointer has already moved before this message is processed.
        self.toolbar_drag = DragGeometry::begin(
            DragPoint::new(pointer.x, pointer.y),
            Bounds::new(bounds.left, bounds.top, bounds.right, bounds.bottom),
            self.toolbar_layout,
            self.toolbar_scale,
        );
        if self.toolbar_drag.is_none() {
            return;
        }
        self.hover = None;
        InvalidateRect(self.toolbar, ptr::null(), 0);
        // Explicit dragging activates this window so capture continues over
        // other applications. Toolbar buttons remain non-activating.
        SetForegroundWindow(self.toolbar);
        SetCapture(self.toolbar);
        if GetCapture() != self.toolbar {
            self.toolbar_drag = None;
        }
    }

    unsafe fn move_toolbar(&mut self, queued_pointer: Option<POINT>) {
        if self.toolbar_drag.is_none() {
            return;
        }
        let pointer = if let Some(pointer) = queued_pointer {
            pointer
        } else {
            let mut pointer = POINT::default();
            if GetCursorPos(&mut pointer) == 0 {
                self.finish_toolbar_drag();
                return;
            }
            pointer
        };
        let Some(handle) = self
            .toolbar_drag
            .and_then(|geometry| geometry.handle_position(DragPoint::new(pointer.x, pointer.y)))
        else {
            self.finish_toolbar_drag();
            return;
        };
        // A slide can separate the real cursor from the column. Monitor
        // selection follows the logical handle carried by the drag instead.
        let work = monitor_work_area(POINT {
            x: handle.x,
            y: handle.y,
        });
        let work_bounds = Bounds::new(work.left, work.top, work.right, work.bottom);
        if self
            .toolbar_animation
            .as_ref()
            .is_some_and(|animation| animation.work() != work_bounds)
        {
            self.finish_toolbar_animation_at(queued_pointer);
        }
        // Use one hypothetical horizontal footprint in both orientations.
        // The actual clipped, resized, or animated HWND never controls undocking.
        for _ in 0..2 {
            let Some(drag) = self.toolbar_drag else {
                return;
            };
            let horizontal_scale =
                fitted_toolbar_scale(ToolbarLayout::Horizontal, self.scale, work);
            let Some(proposal) =
                proposed_toolbar_bounds(ToolbarLayout::Horizontal, pointer, drag, horizontal_scale)
            else {
                self.finish_toolbar_drag();
                return;
            };
            let current = if self.toolbar_animation.is_some() {
                ToolbarLayout::Vertical
            } else {
                self.toolbar_layout
            };
            let desired = docking::choose_layout(current, proposal, work_bounds, self.scale);
            if current == ToolbarLayout::Vertical && desired == ToolbarLayout::Vertical {
                let pinned = self.toolbar_animation.as_ref().map(DockAnimation::edge);
                if pinned
                    .is_some_and(|edge| docking::nearest_edge(proposal, work_bounds) != Some(edge))
                {
                    // A coalesced mouse move can skip the entire in-bounds
                    // region and reach the opposite side in a single sample.
                    self.cancel_toolbar_animation();
                    self.toolbar_layout = ToolbarLayout::Vertical;
                }
            }
            if desired == ToolbarLayout::Horizontal {
                self.cancel_toolbar_animation();
                if current == ToolbarLayout::Horizontal {
                    self.place_horizontal_drag_preview(proposal, work);
                } else {
                    self.toolbar_layout = ToolbarLayout::Horizontal;
                    self.place_toolbar(proposal.left, proposal.top, work);
                }
                self.align_toolbar_drag(pointer, Some(proposal), work);
            } else if current == ToolbarLayout::Horizontal {
                let edge = docking::nearest_edge(proposal, work_bounds).unwrap_or(Edge::Left);
                // Continue from the visible overhang instead of snapping the
                // toolbar back inside before its slide-out animation.
                self.place_horizontal_drag_preview(proposal, work);
                let dpi = GetDpiForWindow(self.toolbar);
                let actual_scale = (dpi as f32 / 96.0).max(1.0);
                if dpi != 0 && (self.scale - actual_scale).abs() >= 0.001 {
                    self.apply_ui_scale(actual_scale);
                    continue;
                }
                let mut source = RECT::default();
                GetWindowRect(self.toolbar, &mut source);
                let Some(target) = docked_toolbar_bounds(edge, pointer, drag, self.scale, work)
                else {
                    self.finish_toolbar_drag();
                    return;
                };
                let vertical_scale =
                    fitted_toolbar_scale(ToolbarLayout::Vertical, self.scale, work);
                if let Some(geometry) = self.toolbar_drag.as_mut() {
                    // The slide separates the pointer from the visible column.
                    // Keep pointer deltas attached to its final logical target.
                    geometry.rebase(
                        DragPoint::new(pointer.x, pointer.y),
                        target,
                        ToolbarLayout::Vertical,
                        vertical_scale,
                    );
                }
                self.toolbar_animation = Some(DockAnimation::new(
                    Bounds::new(source.left, source.top, source.right, source.bottom),
                    target,
                    work_bounds,
                    edge,
                    Instant::now(),
                ));
                if SetTimer(self.toolbar, TIMER_DOCK, 10, None) == 0 {
                    self.finish_toolbar_animation_at(queued_pointer);
                }
                return;
            } else if self.toolbar_animation.is_some() {
                let scale = fitted_toolbar_scale(ToolbarLayout::Vertical, self.scale, work);
                let Some(proposed) =
                    proposed_toolbar_bounds(ToolbarLayout::Vertical, pointer, drag, scale)
                else {
                    self.finish_toolbar_drag();
                    return;
                };
                let target = clamped_toolbar_bounds(proposed, work);
                if target != proposed
                    && let Some(geometry) = self.toolbar_drag.as_mut()
                {
                    let rebased = seam_preserving_bounds(
                        proposed,
                        target,
                        work,
                        geometry.handle_position(DragPoint::new(pointer.x, pointer.y)),
                    );
                    geometry.rebase(
                        DragPoint::new(pointer.x, pointer.y),
                        rebased,
                        ToolbarLayout::Vertical,
                        scale,
                    );
                }
                self.toolbar_animation.as_mut().unwrap().retarget(target);
                self.poll_toolbar_animation(queued_pointer);
                return;
            } else {
                self.toolbar_layout = ToolbarLayout::Vertical;
                let scale = fitted_toolbar_scale(ToolbarLayout::Vertical, self.scale, work);
                let Some(bounds) =
                    proposed_toolbar_bounds(ToolbarLayout::Vertical, pointer, drag, scale)
                else {
                    self.finish_toolbar_drag();
                    return;
                };
                self.place_toolbar(bounds.left, bounds.top, work);
                self.align_toolbar_drag(pointer, Some(bounds), work);
            }
            let dpi = GetDpiForWindow(self.toolbar);
            let actual_scale = (dpi as f32 / 96.0).max(1.0);
            if dpi == 0 || (self.scale - actual_scale).abs() < 0.001 {
                break;
            }
            // A synchronous DPI notification during SetWindowPos cannot
            // reborrow App. Read back the actual DPI before placing again.
            self.apply_ui_scale(actual_scale);
        }
    }

    unsafe fn align_toolbar_drag(&mut self, pointer: POINT, expected: Option<Bounds>, work: RECT) {
        let Some(geometry) = self.toolbar_drag.as_mut() else {
            return;
        };
        let mut actual = RECT::default();
        if GetWindowRect(self.toolbar, &mut actual) == 0 {
            return;
        }
        let actual = Bounds::new(actual.left, actual.top, actual.right, actual.bottom);
        if expected != Some(actual) {
            let point = DragPoint::new(pointer.x, pointer.y);
            let proposed = expected.or_else(|| {
                geometry.proposed_bounds(point, self.toolbar_layout, self.toolbar_scale)
            });
            let rebased = proposed.map_or(actual, |proposed| {
                seam_preserving_bounds(proposed, actual, work, geometry.handle_position(point))
            });
            geometry.rebase(point, rebased, self.toolbar_layout, self.toolbar_scale);
        }
    }

    unsafe fn cancel_toolbar_animation(&mut self) {
        self.toolbar_animation = None;
        KillTimer(self.toolbar, TIMER_DOCK);
    }

    unsafe fn finish_toolbar_animation(&mut self) {
        self.finish_toolbar_animation_at(None);
    }

    unsafe fn finish_toolbar_animation_at(&mut self, queued_pointer: Option<POINT>) {
        let Some(animation) = self.toolbar_animation.take() else {
            return;
        };
        KillTimer(self.toolbar, TIMER_DOCK);
        let work = animation.work();
        let mut target = animation.target();
        let work_rect = RECT {
            left: work.left,
            top: work.top,
            right: work.right,
            bottom: work.bottom,
        };
        self.toolbar_layout = ToolbarLayout::Vertical;
        let mut pointer = queued_pointer.unwrap_or_default();
        let has_pointer = self.toolbar_drag.is_some()
            && (queued_pointer.is_some() || GetCursorPos(&mut pointer) != 0);
        for _ in 0..2 {
            if has_pointer {
                let scale = fitted_toolbar_scale(ToolbarLayout::Vertical, self.scale, work_rect);
                if let Some(bounds) = self.toolbar_drag.and_then(|geometry| {
                    proposed_toolbar_bounds(ToolbarLayout::Vertical, pointer, geometry, scale)
                }) {
                    target = bounds;
                }
            }
            self.place_toolbar(target.left, target.top, work_rect);
            let dpi = GetDpiForWindow(self.toolbar);
            let scale = (dpi as f32 / 96.0).max(1.0);
            if dpi == 0 || (self.scale - scale).abs() < 0.001 {
                break;
            }
            self.apply_ui_scale(scale);
        }
        if has_pointer {
            self.align_toolbar_drag(pointer, None, work_rect);
        }
        self.remember_toolbar();
        self.save_later();
    }

    unsafe fn poll_toolbar_animation(&mut self, queued_pointer: Option<POINT>) {
        let Some(animation) = self.toolbar_animation.as_ref() else {
            KillTimer(self.toolbar, TIMER_DOCK);
            return;
        };
        let frame = animation.frame(Instant::now());
        let work = animation.work();
        if frame.finished {
            self.finish_toolbar_animation_at(queued_pointer);
            // A held drag may have moved during the last animation frame.
            if self.toolbar_drag.is_some() {
                self.move_toolbar(queued_pointer);
            }
            return;
        }
        let work_rect = RECT {
            left: work.left,
            top: work.top,
            right: work.right,
            bottom: work.bottom,
        };
        let scale = fitted_toolbar_scale(frame.layout, self.scale, work_rect);
        self.apply_toolbar_frame(frame.layout, scale, frame.bounds, Some(work));
    }

    unsafe fn finish_toolbar_drag(&mut self) {
        if self.toolbar_drag.take().is_some() {
            if GetCapture() == self.toolbar {
                ReleaseCapture();
            }
            if self.toolbar_animation.is_none()
                && self.toolbar_layout == ToolbarLayout::Horizontal
                && self.toolbar_clipped
            {
                // Dropping before the docking threshold leaves a fully usable
                // horizontal toolbar, rather than saving partly hidden buttons.
                let mut bounds = RECT::default();
                if GetWindowRect(self.toolbar, &mut bounds) != 0 {
                    let center = POINT {
                        x: bounds.left + (bounds.right - bounds.left) / 2,
                        y: bounds.top + (bounds.bottom - bounds.top) / 2,
                    };
                    self.place_toolbar(bounds.left, bounds.top, monitor_work_area(center));
                }
            }
            self.remember_toolbar();
            self.save_later();
            self.redraw_ui();
        }
    }

    unsafe fn render(&mut self, dirty: Option<Rect>) {
        if !self.interactive && self.ink.strokes().is_empty() && self.ink.active_stroke().is_none()
        {
            ShowWindow(self.overlay, SW_HIDE);
            self.renderer = None;
            return;
        }
        let newly_shown = IsWindowVisible(self.overlay) == 0;
        let mut dirty = dirty;
        if self.renderer.is_none() {
            match Renderer::new(
                self.overlay,
                self.origin.x,
                self.origin.y,
                GetSystemMetrics(SM_CXVIRTUALSCREEN),
                GetSystemMetrics(SM_CYVIRTUALSCREEN),
            ) {
                Ok(renderer) => {
                    self.renderer = Some(renderer);
                    dirty = None;
                }
                Err(error) => {
                    self.status = error;
                    self.redraw_ui();
                    return;
                }
            }
        }
        if let Err(error) =
            self.renderer
                .as_mut()
                .unwrap()
                .render(&self.ink, self.interactive, dirty)
        {
            self.status = error;
            self.redraw_ui();
        }
        if newly_shown {
            ShowWindow(self.overlay, SW_SHOWNOACTIVATE);
            SetWindowPos(
                self.toolbar,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
            if IsWindowVisible(self.panel) != 0 {
                SetWindowPos(
                    self.panel,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    unsafe fn save_later(&self) {
        if !self.removing {
            SetTimer(self.toolbar, TIMER_SAVE, 350, None);
        }
    }
    unsafe fn save(&mut self) {
        KillTimer(self.toolbar, TIMER_SAVE);
        if self.removing {
            return;
        }
        if let Err(error) = self.settings.save() {
            self.status = error;
            self.redraw_ui();
        }
    }

    unsafe fn ensure_tray(&mut self) {
        if !self.tray.is_added() {
            SetTimer(self.toolbar, TIMER_TRAY, 2000, None);
        } else {
            KillTimer(self.toolbar, TIMER_TRAY);
        }
    }

    unsafe fn set_ui_visible(&mut self, visible: bool) {
        self.finish_toolbar_animation();
        self.finish_toolbar_drag();
        self.close_radial();
        self.finish_gesture();
        self.stop_hold();
        self.commit_edit();
        self.cancel_capture();
        self.panel_drag = None;
        ShowWindow(self.panel, SW_HIDE);
        self.ui_visible = visible;
        self.settings.toolbar_visible = visible;
        self.toolbar_pointer = None;
        self.hover = None;
        self.toolbar_tips.hide();
        if visible {
            self.paint_glass();
            SetTimer(self.toolbar, TIMER_GLASS, GLASS_REFRESH_MS, None);
        } else {
            KillTimer(self.toolbar, TIMER_GLASS);
            self.glass = Glass::default();
        }
        ShowWindow(
            self.toolbar,
            if visible { SW_SHOWNOACTIVATE } else { SW_HIDE },
        );
        if visible {
            SetWindowPos(
                self.toolbar,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        self.tray.update_visibility(visible);
        self.ensure_tray();
        self.save();
        self.redraw_ui();
    }

    unsafe fn request_quit(&mut self) {
        if self.update_install_receiver.is_some() {
            return;
        }
        self.finish_toolbar_animation();
        self.finish_toolbar_drag();
        self.close_radial();
        self.finish_gesture();
        self.stop_hold();
        self.commit_edit();
        self.cancel_capture();
        PostQuitMessage(0);
    }

    unsafe fn unregister_hotkeys(&self) {
        for action in Action::ALL {
            hotkeys::unregister(self.toolbar, action.index());
        }
    }

    unsafe fn register_hotkeys(&mut self) {
        self.register_hotkeys_except(None);
    }

    unsafe fn register_hotkeys_except(&mut self, keep: Option<usize>) {
        for action in Action::ALL {
            let index = action.index();
            if keep == Some(index) {
                continue;
            }
            hotkeys::unregister(self.toolbar, index);
            self.hotkey_errors[index] =
                hotkeys::register(self.toolbar, index, self.settings.hotkeys[index])
                    .err()
                    .map(hotkeys::registration_error);
        }
        if self.hotkey_errors.iter().any(Option::is_some) {
            self.status = UNAVAILABLE_SHORTCUTS.into();
        } else if self.status == UNAVAILABLE_SHORTCUTS {
            self.status.clear();
        }
    }

    unsafe fn finish_gesture(&mut self) {
        if self.ink.is_drawing() {
            self.ink.end();
            self.last_point = None;
            ReleaseCapture();
        }
    }

    unsafe fn select_tool(&mut self, tool: Tool, active: bool) {
        self.finish_gesture();
        self.stop_hold();
        let changed = self.interactive != active;
        self.interactive = active;
        self.tool = tool;
        let mut style = GetWindowLongPtrW(self.overlay, GWL_EXSTYLE) as u32;
        if active {
            style &= !WS_EX_TRANSPARENT;
        } else {
            style |= WS_EX_TRANSPARENT;
        }
        SetWindowLongPtrW(self.overlay, GWL_EXSTYLE, style as isize);
        if changed {
            self.render(None);
        }
        self.update_cursor();
        self.redraw_ui();
    }

    unsafe fn action(&mut self, action: Action, from_hotkey: bool) {
        self.finish_toolbar_drag();
        if action != Action::RadialMenu {
            self.close_radial();
        }
        match action {
            Action::TogglePen => {
                self.select_tool(Tool::Pen, !(self.interactive && self.tool == Tool::Pen))
            }
            Action::ObjectEraser => self.select_tool(Tool::ObjectEraser, true),
            Action::Eraser => {
                self.select_tool(Tool::Eraser, true);
                if from_hotkey {
                    let mut cursor = POINT::default();
                    GetCursorPos(&mut cursor);
                    self.hold = Some(EraserHold {
                        key: self.settings.hotkeys[Action::Eraser.index()].key,
                        start_y: cursor.y,
                        initial: self.settings.eraser_diameter,
                        started: Instant::now(),
                        resizing: false,
                    });
                    SetTimer(self.toolbar, TIMER_ERASER, 16, None);
                }
            }
            Action::Clear => {
                self.finish_gesture();
                self.ink.clear();
                self.render(None);
            }
            Action::Undo => {
                self.finish_gesture();
                if self.ink.can_undo() && self.ink.undo() {
                    self.render(None);
                }
            }
            Action::SwapColor => {
                self.finish_gesture();
                std::mem::swap(
                    &mut self.settings.primary_color,
                    &mut self.settings.secondary_color,
                );
                self.hsv = ui::rgb_to_hsv(self.edited_color());
                self.sync_edits(None);
                self.save_later();
                self.redraw_ui();
            }
            Action::ToggleUi => self.set_ui_visible(!self.ui_visible),
            Action::Quit => self.request_quit(),
            Action::RadialMenu => {
                if from_hotkey {
                    self.open_radial();
                }
            }
        }
    }

    unsafe fn open_radial(&mut self) {
        if self.radial_hold.is_some() {
            return;
        }
        let key = self.settings.hotkeys[Action::RadialMenu.index()].key;
        if key == 0 || GetAsyncKeyState(key as i32) >= 0 {
            return;
        }
        self.finish_gesture();
        self.stop_hold();
        self.commit_edit();
        if self.panel_drag.take().is_some() {
            ReleaseCapture();
        }
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) == 0 {
            return;
        }
        let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..zeroed()
        };
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            return;
        }
        let center = POINT {
            x: info.rcMonitor.left + (info.rcMonitor.right - info.rcMonitor.left) / 2,
            y: info.rcMonitor.top + (info.rcMonitor.bottom - info.rcMonitor.top) / 2,
        };
        // Resolve the monitor DPI without moving or activating the toolbar.
        SetWindowPos(
            self.radial_window,
            HWND_TOPMOST,
            center.x,
            center.y,
            1,
            1,
            SWP_NOACTIVATE,
        );
        let monitor_scale = (GetDpiForWindow(self.radial_window) as f32 / 96.0).max(1.0);
        // Keep the wheel fully visible even on small or heavily scaled displays.
        let fit = (info.rcMonitor.right - info.rcMonitor.left)
            .min(info.rcMonitor.bottom - info.rcMonitor.top) as f32
            / (radial::SIZE_DIP as f32 + 16.0);
        let scale = monitor_scale.min(fit).max(0.25);
        let renderer = match radial::Renderer::new(self.radial_window, center.x, center.y, scale) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.status = error;
                self.redraw_ui();
                return;
            }
        };
        self.radial_hold = Some(RadialHold {
            key,
            center,
            scale,
            state: WheelState {
                stage: WheelStage::Actions,
                hover: None,
                committed: None,
                primary: self.primary(),
                secondary: self.secondary(),
                interactive: self.interactive,
                tool: self.tool,
            },
            renderer,
            dwell: HoverDwell::default(),
        });
        if !self.render_radial() {
            return;
        }
        // Starting at the hub makes all four directions equally accessible,
        // and prevents an old pointer position from committing a selection.
        if !self.center_radial_pointer() {
            return;
        }
        SetCursor(LoadCursorW(ptr::null_mut(), IDC_ARROW));
        ShowWindow(self.radial_window, SW_SHOWNOACTIVATE);
        SetWindowPos(
            self.radial_window,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        if SetTimer(self.toolbar, TIMER_RADIAL, 16, None) == 0 {
            self.status = "Could not start the radial menu's release timer.".into();
            self.close_radial();
            self.redraw_ui();
        }
    }

    unsafe fn center_radial_pointer(&mut self) -> bool {
        let Some(hold) = self.radial_hold.as_ref() else {
            return false;
        };
        let mut actual = POINT::default();
        let centered = SetCursorPos(hold.center.x, hold.center.y) != 0
            && GetCursorPos(&mut actual) != 0
            && pointer_is_neutral(actual, hold.center, hold.scale);
        if !centered {
            // ClipCursor in another app can keep the pointer away from the
            // hub even when SetCursorPos succeeds. Never select from there.
            self.status = "The radial menu could not center the pointer.".into();
            self.close_radial();
            self.redraw_ui();
        }
        centered
    }

    unsafe fn render_radial(&mut self) -> bool {
        let result = self
            .radial_hold
            .as_mut()
            .map(|hold| hold.renderer.render(&hold.state));
        if let Some(Err(error)) = result {
            self.status = error;
            self.close_radial();
            self.redraw_ui();
            return false;
        }
        true
    }

    unsafe fn close_radial(&mut self) {
        KillTimer(self.toolbar, TIMER_RADIAL);
        if self.radial_hold.take().is_some() {
            ShowWindow(self.radial_window, SW_HIDE);
            self.update_cursor();
        }
    }

    unsafe fn poll_radial(&mut self) {
        let Some(hold) = self.radial_hold.as_ref() else {
            KillTimer(self.toolbar, TIMER_RADIAL);
            return;
        };
        if GetAsyncKeyState(hold.key as i32) >= 0 {
            self.close_radial();
            return;
        }
        if hold.state.committed.is_some() {
            return;
        }
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor) == 0 {
            self.close_radial();
            return;
        }
        let hold = self.radial_hold.as_mut().unwrap();
        let candidate = radial::hit_test(
            hold.state.stage,
            (cursor.x - hold.center.x) as f32 / hold.scale,
            (cursor.y - hold.center.y) as f32 / hold.scale,
        );
        let changed = hold.state.hover != candidate;
        hold.state.hover = candidate;
        let selected = hold.dwell.sample(candidate, Instant::now());
        if let Some(index) = selected {
            self.commit_radial(index);
        } else if changed {
            self.render_radial();
        }
    }

    unsafe fn commit_radial(&mut self, index: usize) {
        let Some(hold) = self.radial_hold.as_mut() else {
            return;
        };
        if hold.state.committed.is_some() {
            return;
        }
        if hold.state.stage == WheelStage::Actions && index == 3 {
            hold.state.stage = WheelStage::Colors;
            hold.state.hover = None;
            hold.dwell = HoverDwell::default();
            if !self.center_radial_pointer() {
                return;
            }
            self.render_radial();
            return;
        }
        let stage = hold.state.stage;
        let color = if stage == WheelStage::Colors {
            radial::colors(hold.state.primary, hold.state.secondary)
                .get(index)
                .copied()
        } else {
            None
        };
        hold.state.committed = Some(index);
        match stage {
            WheelStage::Actions => match index {
                // The wheel switches from any drawing tool back to the mouse.
                0 => self.select_tool(Tool::Pen, !self.interactive),
                1 => {
                    self.ink.clear();
                    self.render(None);
                }
                2 => self.select_tool(Tool::ObjectEraser, true),
                _ => {}
            },
            WheelStage::Colors => {
                if let Some(color) = color {
                    self.settings.primary_color = color.to_hex();
                    self.hsv = ui::rgb_to_hsv(self.edited_color());
                    self.sync_edits(None);
                    self.save_later();
                    self.redraw_ui();
                }
            }
        }
        // Ink activation can change topmost ordering; the held wheel stays above it.
        SetWindowPos(
            self.radial_window,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        SetCursor(LoadCursorW(ptr::null_mut(), IDC_ARROW));
        self.render_radial();
    }

    unsafe fn stop_hold(&mut self) {
        if self.hold.take().is_some() {
            KillTimer(self.toolbar, TIMER_ERASER);
            self.save_later();
        }
    }

    unsafe fn poll_hold(&mut self) {
        let Some(hold) = self.hold.as_mut() else {
            return;
        };
        if GetAsyncKeyState(hold.key as i32) >= 0 {
            self.stop_hold();
            return;
        }
        let mut cursor = POINT::default();
        GetCursorPos(&mut cursor);
        let delta = (cursor.y - hold.start_y) as f32 / self.scale;
        if hold.started.elapsed().as_millis() < 180 || (!hold.resizing && delta.abs() <= 4.0) {
            return;
        }
        hold.resizing = true;
        let diameter = eraser_diameter(hold.initial, delta);
        if (diameter - self.settings.eraser_diameter).abs() >= 0.25 {
            self.settings.eraser_diameter = diameter;
            self.update_cursor();
            self.redraw_ui();
        }
    }

    unsafe fn update_cursor(&mut self) {
        if self.tool != Tool::Pen && self.interactive {
            let diameter = if self.tool == Tool::Eraser {
                self.settings.eraser_diameter
            } else {
                22.0
            };
            let next = circle_cursor(diameter * self.scale);
            if !next.is_null() {
                SetCursor(next);
                if !self.cursor.is_null() {
                    DestroyCursor(self.cursor);
                }
                self.cursor = next;
            }
        } else {
            SetCursor(LoadCursorW(
                ptr::null_mut(),
                if self.interactive {
                    IDC_CROSS
                } else {
                    IDC_ARROW
                },
            ));
        }
    }

    unsafe fn pointer_down(&mut self, x: i32, y: i32) {
        // A key-up may precede the next timer tick. Do not drop a quick
        // tap-then-click gesture in that short interval.
        if self.hold.is_some() {
            self.poll_hold();
        }
        if !self.interactive || self.hold.is_some() || self.radial_hold.is_some() {
            return;
        }
        let point = Point::new(x as f32, y as f32);
        // Keep the pre-erase width for the entire gesture: the widest stroke
        // may disappear on the very first sample, but its old pixels still
        // need to be included in subsequent dirty rectangles.
        self.gesture_max_width = self
            .ink
            .strokes()
            .iter()
            .map(|stroke| stroke.width)
            .fold(0.0, f32::max);
        let width = match self.tool {
            Tool::Pen => self.settings.pen_width,
            Tool::Eraser => self.settings.eraser_diameter,
            Tool::ObjectEraser => 22.0,
        } * self.scale;
        let old_count = self.ink.strokes().len();
        let object_area = if self.tool == Tool::ObjectEraser {
            object_dirty_area(&self.ink, point, point, 11.0 * self.scale)
        } else {
            None
        };
        self.ink.begin(point, self.tool, self.primary(), width);
        self.last_point = Some(point);
        SetCapture(self.overlay);
        if self.tool == Tool::ObjectEraser {
            if old_count != self.ink.strokes().len() {
                self.render(object_area);
            }
        } else {
            self.render_pointer(point, point);
        }
    }

    unsafe fn pointer_move(&mut self, x: i32, y: i32) {
        let Some(last) = self.last_point else {
            return;
        };
        if !self.ink.is_drawing() {
            return;
        }
        let point = Point::new(x as f32, y as f32);
        let old_count = self.ink.strokes().len();
        let object_area = if self.tool == Tool::ObjectEraser {
            object_dirty_area(&self.ink, last, point, 11.0 * self.scale)
        } else {
            None
        };
        self.ink.extend(point);
        self.last_point = Some(point);
        if self.tool == Tool::ObjectEraser {
            if old_count != self.ink.strokes().len() {
                self.render(object_area);
            }
        } else {
            self.render_pointer(last, point);
        }
    }

    unsafe fn render_pointer(&mut self, previous: Point, point: Point) {
        if self.tool == Tool::ObjectEraser {
            self.render(None);
            return;
        }
        let radius = if self.tool == Tool::Pen {
            self.settings.pen_width * 0.5 + 3.0
        } else {
            self.settings.eraser_diameter * 0.5
        } * self.scale;
        let radius = if self.tool == Tool::Eraser {
            radius + self.gesture_max_width + 3.0
        } else {
            radius
        };
        self.render(Some(
            Rect::around(previous.x, previous.y, radius)
                .union(Rect::around(point.x, point.y, radius)),
        ));
    }

    unsafe fn open_panel(&mut self, tab: usize) {
        self.finish_toolbar_animation();
        self.finish_toolbar_drag();
        self.close_radial();
        if !self.ui_visible {
            self.set_ui_visible(true);
        }
        self.finish_gesture();
        self.stop_hold();
        self.cancel_capture();
        self.tab = tab;
        self.hsv = ui::rgb_to_hsv(self.edited_color());
        self.sync_edits(None);
        self.show_edits();
        let mut toolbar_rect = RECT::default();
        GetWindowRect(self.toolbar, &mut toolbar_rect);
        let monitor = MonitorFromWindow(self.toolbar, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..zeroed()
        };
        GetMonitorInfoW(monitor, &mut info);
        self.apply_panel_scale(fitted_surface_scale(
            (PANEL_WIDTH, PANEL_HEIGHT),
            self.scale,
            info.rcWork,
        ));
        let width = self.px(PANEL_WIDTH);
        let height = self.px(PANEL_HEIGHT);
        let preferred_x = if self.toolbar_layout == ToolbarLayout::Vertical {
            if toolbar_rect.left + (toolbar_rect.right - toolbar_rect.left) / 2
                < info.rcWork.left + (info.rcWork.right - info.rcWork.left) / 2
            {
                toolbar_rect.right + self.px(12)
            } else {
                toolbar_rect.left - width - self.px(12)
            }
        } else {
            toolbar_rect.right.saturating_sub(width)
        };
        let x = preferred_x.clamp(
            info.rcWork.left,
            (info.rcWork.right - width).max(info.rcWork.left),
        );
        let above = toolbar_rect.top - height - self.px(12);
        let y = if self.toolbar_layout == ToolbarLayout::Vertical {
            toolbar_rect.top
        } else if above >= info.rcWork.top {
            above
        } else {
            toolbar_rect.bottom + self.px(12)
        }
        .clamp(
            info.rcWork.top,
            (info.rcWork.bottom - height).max(info.rcWork.top),
        );
        SetWindowPos(
            self.panel,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_SHOWWINDOW,
        );
        SetForegroundWindow(self.panel);
        self.redraw_ui();
    }

    unsafe fn close_panel(&mut self) {
        self.commit_edit();
        self.cancel_capture();
        self.panel_drag = None;
        ShowWindow(self.panel, SW_HIDE);
        self.save();
    }

    fn px(&self, value: i32) -> i32 {
        (value as f32 * self.panel_scale).round() as i32
    }
    unsafe fn show_edits(&self) {
        for edit in self.edits {
            ShowWindow(edit, if self.tab == 0 { SW_SHOWNA } else { SW_HIDE });
        }
    }

    unsafe fn sync_edits(&mut self, skip: Option<usize>) {
        let color = self.edited_color();
        let values = [
            color.to_hex(),
            color.r.to_string(),
            color.g.to_string(),
            color.b.to_string(),
        ];
        self.syncing_edits = true;
        for (index, value) in values.iter().enumerate() {
            if skip != Some(index) {
                SetWindowTextW(self.edits[index], wide(value).as_ptr());
            }
        }
        self.syncing_edits = false;
    }

    unsafe fn commit_edit(&mut self) {
        let focus = GetFocus();
        if focus == self.edits[0] {
            if let Some(color) = Color::from_hex(&window_text(focus)) {
                self.set_color(color, false, None);
            } else {
                self.sync_edits(None);
            }
        } else if self.edits.contains(&focus) {
            self.sync_edits(None);
        }
    }

    unsafe fn set_color(&mut self, color: Color, preserve_hsv: bool, skip: Option<usize>) {
        if self.editing_secondary {
            self.settings.secondary_color = color.to_hex();
        } else {
            self.settings.primary_color = color.to_hex();
        }
        if !preserve_hsv {
            self.hsv = ui::rgb_to_hsv(color);
        }
        self.sync_edits(skip);
        self.save_later();
        self.redraw_ui();
    }

    unsafe fn edit_changed(&mut self, id: usize) {
        if self.syncing_edits || !(EDIT_HEX..=EDIT_BLUE).contains(&id) {
            return;
        }
        let index = id - EDIT_HEX;
        let text = window_text(self.edits[index]);
        let color = if index == 0 {
            // Wait for a complete 24-bit value while typing; three-digit hex is
            // accepted on focus loss so intermediate input does not jump.
            if text.trim_start_matches('#').len() == 6 {
                Color::from_hex(&text)
            } else {
                None
            }
        } else {
            text.parse::<u8>().ok().map(|value| {
                let mut color = self.edited_color();
                match index {
                    1 => color.r = value,
                    2 => color.g = value,
                    _ => color.b = value,
                }
                color
            })
        };
        if let Some(color) = color {
            self.set_color(color, false, Some(index));
        }
    }

    unsafe fn begin_capture(&mut self, index: usize) {
        self.close_radial();
        self.cancel_capture();
        self.unregister_hotkeys();
        self.clear_pending_shortcut();
        self.capturing = Some(index);
        self.status = "Press a shortcut. Backspace unbinds; Esc cancels.".into();
        SetFocus(self.panel);
        self.redraw_ui();
    }

    unsafe fn cancel_capture(&mut self) {
        if self.capturing.take().is_some() {
            self.status.clear();
            self.register_hotkeys();
            self.redraw_ui();
        }
    }

    unsafe fn clear_pending_shortcut(&mut self) {
        self.pending_shortcut_release = None;
        KillTimer(self.toolbar, TIMER_CAPTURE_RELEASE);
    }

    unsafe fn key_up(&mut self, key: u32) {
        if self
            .radial_hold
            .as_ref()
            .is_some_and(|hold| hold.key == key)
        {
            self.close_radial();
        }
        if self.pending_shortcut_release == Some(key) {
            self.clear_pending_shortcut();
        }
    }

    unsafe fn poll_shortcut_release(&mut self) {
        if self
            .pending_shortcut_release
            .is_none_or(|key| GetAsyncKeyState(key as i32) >= 0)
        {
            self.clear_pending_shortcut();
        }
    }

    unsafe fn key_down(&mut self, key: u32) -> bool {
        if self.toolbar_drag.is_some() && key == VK_ESCAPE as u32 {
            self.finish_toolbar_drag();
            return true;
        }
        if let Some(index) = self.capturing {
            if key == VK_ESCAPE as u32 {
                self.cancel_capture();
                return true;
            }
            if matches!(key, 0x10..=0x12 | 0x5b | 0x5c | 0xa0..=0xa5) {
                return true;
            }
            let modifiers = current_modifiers();
            let hotkey = if key == VK_BACK as u32 && modifiers == 0 {
                Hotkey::unbound()
            } else {
                Hotkey::new(modifiers, key)
            };
            if let Err(error) = self.settings.validate_hotkey(Action::ALL[index], hotkey) {
                self.status = error;
                self.redraw_ui();
                return true;
            }
            if let Err(code) = hotkeys::register(self.toolbar, index, hotkey) {
                self.status = hotkeys::registration_error(code);
                self.redraw_ui();
                return true;
            }
            // Keep the accepted registration held. Releasing it to register
            // again would let another process claim it in the meantime.
            self.settings.hotkeys[index] = hotkey;
            self.hotkey_errors[index] = None;
            self.capturing = None;
            // Saving a held shortcut must not immediately execute it (notably
            // Exit). Resume dispatch as soon as its primary key is released.
            self.pending_shortcut_release = Some(key);
            SetTimer(self.toolbar, TIMER_CAPTURE_RELEASE, 16, None);
            self.status = "Shortcut saved.".into();
            self.register_hotkeys_except(Some(index));
            self.save();
            self.redraw_ui();
            return true;
        }
        if IsWindowVisible(self.panel) != 0 && GetForegroundWindow() == self.panel {
            if key == VK_ESCAPE as u32 {
                self.close_panel();
                return true;
            }
            if key == VK_RETURN as u32 {
                self.commit_edit();
                SetFocus(self.panel);
                self.sync_edits(None);
                return true;
            }
            if key == VK_TAB as u32 && self.tab == 0 {
                self.commit_edit();
                let focus = GetFocus();
                let next = self
                    .edits
                    .iter()
                    .position(|edit| *edit == focus)
                    .map(|i| (i + 1) % 4)
                    .unwrap_or(0);
                SetFocus(self.edits[next]);
                SendMessageW(self.edits[next], EM_SETSEL, 0, -1);
                return true;
            }
        }
        false
    }

    unsafe fn toolbar_click(&mut self, x: i32, y: i32, client_point: POINT) {
        if let Some(index) = self.toolbar_layout.hit_test(x, y) {
            match index {
                0 => self.select_tool(Tool::Pen, false),
                1 => self.action(Action::TogglePen, false),
                2 => self.action(Action::Eraser, false),
                3 => self.action(Action::ObjectEraser, false),
                4 => self.action(Action::Undo, false),
                5 => self.action(Action::Clear, false),
                6 => self.open_panel(0),
                7 => {
                    if IsWindowVisible(self.panel) != 0 {
                        self.close_panel();
                    } else {
                        self.open_panel(1);
                    }
                }
                8 => self.action(Action::ToggleUi, false),
                9 => self.action(Action::Quit, false),
                _ => {}
            }
            return;
        }
        if self.toolbar_layout.is_drag_handle(x, y) {
            self.begin_toolbar_drag(client_point);
        }
    }

    unsafe fn panel_click(&mut self, x: i32, y: i32) {
        self.commit_edit();
        SetFocus(self.panel);
        if (416..444).contains(&x) && (16..44).contains(&y) {
            self.close_panel();
            return;
        }
        if (76..108).contains(&y) {
            for (tab, left) in [24, 164, 304].into_iter().enumerate() {
                if (left..left + 130).contains(&x) {
                    self.cancel_capture();
                    self.tab = tab;
                    self.show_edits();
                    self.redraw_ui();
                    return;
                }
            }
        }
        if y < 64 {
            ReleaseCapture();
            PostMessageW(self.panel, WM_NCLBUTTONDOWN, HTCAPTION as usize, 0);
            return;
        }
        match self.tab {
            0 => {
                if (124..180).contains(&y) && (24..436).contains(&x) {
                    self.editing_secondary = x >= 230;
                    self.hsv = ui::rgb_to_hsv(self.edited_color());
                    self.sync_edits(None);
                    self.redraw_ui();
                } else if (24..436).contains(&x) && (202..372).contains(&y) {
                    self.panel_drag = Some(PanelDrag::SaturationValue);
                } else if (24..436).contains(&x) && (382..412).contains(&y) {
                    self.panel_drag = Some(PanelDrag::Hue);
                } else if (510..542).contains(&y) {
                    for (index, color) in ui::PALETTE.into_iter().enumerate() {
                        if (x - (42 + 35 * index as i32)).abs() <= 15 {
                            self.set_color(color, false, None);
                            break;
                        }
                    }
                }
            }
            1 => {
                for index in 0..Action::ALL.len() as i32 {
                    let top = SHORTCUT_TOP + index * SHORTCUT_STEP;
                    if (230..436).contains(&x) && (top..top + SHORTCUT_HEIGHT).contains(&y) {
                        self.begin_capture(index as usize);
                        return;
                    }
                }
            }
            2 => {
                if (24..436).contains(&x) && (136..182).contains(&y) {
                    if self.update_install_receiver.is_some() {
                        return;
                    }
                    self.settings.check_for_updates = !self.settings.check_for_updates;
                    if self.settings.check_for_updates {
                        self.start_updates();
                    } else {
                        KillTimer(self.toolbar, TIMER_DAILY);
                        KillTimer(self.toolbar, TIMER_UPDATES);
                        self.update_receiver = None;
                    }
                    self.save_later();
                    self.redraw_ui();
                } else if (24..436).contains(&x) && (314..348).contains(&y) {
                    self.panel_drag = Some(PanelDrag::PenWidth);
                } else if (24..436).contains(&x) && (410..444).contains(&y) {
                    self.panel_drag = Some(PanelDrag::EraserWidth);
                } else if (24..220).contains(&x) && (528..568).contains(&y) {
                    self.request_quit();
                } else if (236..436).contains(&x) && (528..568).contains(&y) {
                    PostMessageW(self.panel, REMOVE_REQUEST, 0, 0);
                } else if (24..436).contains(&x)
                    && (UPDATE_BUTTON_TOP..UPDATE_BUTTON_BOTTOM).contains(&y)
                    && self.available_update().is_some()
                {
                    PostMessageW(self.panel, UPDATE_REQUEST, 0, 0);
                }
            }
            _ => {}
        }
        if self.panel_drag.is_some() {
            SetCapture(self.panel);
            self.panel_move(x, y);
        }
    }

    unsafe fn panel_move(&mut self, x: i32, y: i32) {
        let Some(drag) = self.panel_drag else {
            return;
        };
        let fraction = ((x - 24) as f32 / 412.0).clamp(0.0, 1.0);
        match drag {
            PanelDrag::SaturationValue => {
                self.hsv.1 = fraction;
                self.hsv.2 = 1.0 - ((y - 202) as f32 / 170.0).clamp(0.0, 1.0);
                self.set_color(
                    ui::hsv_to_rgb(self.hsv.0, self.hsv.1, self.hsv.2),
                    true,
                    None,
                );
            }
            PanelDrag::Hue => {
                self.hsv.0 = fraction * 360.0;
                self.set_color(
                    ui::hsv_to_rgb(self.hsv.0, self.hsv.1, self.hsv.2),
                    true,
                    None,
                );
            }
            PanelDrag::PenWidth => {
                self.settings.pen_width = (1.0 + fraction * 15.0).round();
                self.save_later();
                self.redraw_ui();
            }
            PanelDrag::EraserWidth => {
                self.settings.eraser_diameter = (6.0 + fraction * 234.0).round();
                if self.interactive && self.tool == Tool::Eraser {
                    self.update_cursor();
                }
                self.save_later();
                self.redraw_ui();
            }
        }
    }

    unsafe fn start_updates(&mut self) {
        if !self.settings.check_for_updates
            || self.update_receiver.is_some()
            || self.update_install_receiver.is_some()
        {
            return;
        }
        self.update_error = None;
        let Some(repository) = updates::repository(self.settings.github_repository.as_deref())
        else {
            self.update_status = UpdateStatus::Unconfigured;
            return;
        };
        if SetTimer(self.toolbar, TIMER_UPDATES, 500, None) == 0 {
            self.update_status = UpdateStatus::Failed("Could not start the update check.".into());
            self.redraw_ui();
            return;
        }
        let repository = repository.to_owned();
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new().spawn(move || {
            let _ = sender.send(updates::check(Some(&repository), env!("CARGO_PKG_VERSION")));
        }) {
            Ok(_) => {
                self.update_receiver = Some(receiver);
                SetTimer(self.toolbar, TIMER_DAILY, DAILY_UPDATE_MS, None);
            }
            Err(_) => {
                KillTimer(self.toolbar, TIMER_UPDATES);
                self.update_status =
                    UpdateStatus::Failed("Could not start the update check.".into());
            }
        }
        self.redraw_ui();
    }

    unsafe fn poll_updates(&mut self) {
        let Some(receiver) = &self.update_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(status) => {
                self.update_status = status;
                self.update_receiver = None;
                KillTimer(self.toolbar, TIMER_UPDATES);
                self.redraw_ui();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.update_receiver = None;
                KillTimer(self.toolbar, TIMER_UPDATES);
                self.update_status =
                    UpdateStatus::Failed("The update check did not finish.".into());
                self.redraw_ui();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    fn available_update(&self) -> Option<(&ReleaseInfo, bool)> {
        if !self.settings.check_for_updates
            || self.update_receiver.is_some()
            || self.update_install_receiver.is_some()
            || self.update_dialog_open
        {
            return None;
        }
        match &self.update_status {
            UpdateStatus::Available(release) => Some((
                release,
                release.installable() && self.update_error.is_none(),
            )),
            _ => None,
        }
    }

    unsafe fn start_update_install(&mut self, release: ReleaseInfo) {
        if !release.installable() {
            return;
        }
        let Some(asset) = release.asset else {
            return;
        };
        if SetTimer(self.toolbar, TIMER_INSTALL_UPDATE, 250, None) == 0 {
            self.update_error = Some("Could not start the update installer.".into());
            self.redraw_ui();
            return;
        }
        let (sender, receiver) = mpsc::channel();
        match std::thread::Builder::new()
            .name("study-pen-update".into())
            .spawn(move || {
                let result = install_update::stage(&asset, &release.version)
                    .and_then(install_update::StagedUpdate::start);
                let _ = sender.send(result);
            }) {
            Ok(_) => {
                self.update_error = None;
                self.update_install_receiver = Some(receiver);
            }
            Err(_) => {
                KillTimer(self.toolbar, TIMER_INSTALL_UPDATE);
                self.update_error = Some("Could not start the update installer.".into());
            }
        }
        self.redraw_ui();
    }

    unsafe fn poll_update_install(&mut self) {
        let Some(receiver) = &self.update_install_receiver else {
            return;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The update installer did not finish.".into())
            }
            Err(mpsc::TryRecvError::Empty) => return,
        };
        self.update_install_receiver = None;
        KillTimer(self.toolbar, TIMER_INSTALL_UPDATE);
        match result {
            Ok(()) => self.request_quit(),
            Err(error) => {
                self.update_error = Some(error);
                self.redraw_ui();
            }
        }
    }

    unsafe fn display_changed(&mut self) {
        self.finish_toolbar_animation();
        self.finish_toolbar_drag();
        self.close_radial();
        self.finish_gesture();
        self.stop_hold();
        self.origin = POINT {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
        };
        let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        // Release before reallocating to avoid holding two desktop bitmaps
        // during a resolution change, and retain lazy allocation while idle.
        self.renderer = None;
        SetWindowPos(
            self.overlay,
            HWND_TOPMOST,
            self.origin.x,
            self.origin.y,
            width,
            height,
            SWP_NOACTIVATE,
        );
        self.render(None);
        self.restore_toolbar_geometry(None);
        self.save_later();
        SetWindowPos(
            self.toolbar,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        if IsWindowVisible(self.panel) != 0 {
            SetWindowPos(
                self.panel,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        unsafe {
            self.finish_toolbar_animation();
            self.finish_toolbar_drag();
            self.close_radial();
            self.unregister_hotkeys();
            self.save();
            SetCursor(LoadCursorW(ptr::null_mut(), IDC_ARROW));
            if !self.cursor.is_null() {
                DestroyCursor(self.cursor);
            }
            self.tray.remove();
            DestroyWindow(self.panel);
            DestroyWindow(self.toolbar);
            DestroyWindow(self.overlay);
            DestroyWindow(self.radial_window);
            DeleteObject(self.edit_font);
            DeleteObject(self.edit_brush);
        }
    }
}

/// Keep native prompts and shell calls outside the application borrow.
unsafe fn request_update(panel: HWND) {
    let request = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        if panel != app.panel || app.removal_dialog_open || app.removing {
            return None;
        }
        let (release, installable) = app.available_update()?;
        let release = release.clone();
        app.update_dialog_open = installable;
        Some((release, installable))
    });
    let Some((release, installable)) = request else {
        return;
    };
    if !installable {
        let url = release
            .asset
            .as_ref()
            .map_or(&release.url, |asset| &asset.url);
        let result = ShellExecuteW(
            panel,
            wide("open").as_ptr(),
            wide(url).as_ptr(),
            ptr::null(),
            ptr::null(),
            SW_SHOWNORMAL,
        );
        if result as isize <= 32 {
            APP.with(|cell| {
                if let Some(app) = cell.borrow_mut().as_mut() {
                    app.update_error = Some("Could not open GitHub in the default browser.".into());
                    app.redraw_ui();
                }
            });
        }
        return;
    }

    let message = format!(
        "Install Study Pen {}?\n\nStudy Pen will download and verify the update, replace this portable copy, and restart. Your settings will be kept. On-screen ink will be cleared when the app closes.",
        release.version,
    );
    let approved = MessageBoxW(
        panel,
        wide(&message).as_ptr(),
        wide("Install Study Pen update").as_ptr(),
        MB_YESNO | MB_DEFBUTTON2 | MB_ICONQUESTION | MB_TOPMOST,
    ) == IDYES;
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            app.update_dialog_open = false;
            if approved && app.settings.check_for_updates {
                app.start_update_install(release);
            }
        }
    });
}

/// Native confirmation pumps messages, so it must run outside the App borrow.
unsafe fn confirm_removal(panel: HWND) {
    let executable = APP.with(|cell| {
        let mut state = cell.try_borrow_mut().ok()?;
        let app = state.as_mut()?;
        if panel != app.panel
            || app.removal_dialog_open
            || app.removing
            || app.update_dialog_open
            || app.update_install_receiver.is_some()
        {
            return None;
        }
        app.removal_dialog_open = true;
        app.finish_toolbar_animation();
        app.finish_toolbar_drag();
        app.close_radial();
        app.finish_gesture();
        app.stop_hold();
        Some(
            app.removal_image
                .as_ref()
                .map(|image| image.path().to_owned())
                .map_err(Clone::clone),
        )
    });
    let Some(executable) = executable else {
        return;
    };

    let approved = executable.as_ref().is_ok_and(|path| {
        let message = format!(
            "Remove this portable copy of Study Pen?\n\n{}\n\nStudy Pen will close and remove its executable, saved preferences and matching desktop and Start shortcuts. This cannot be undone.",
            path.display(),
        );
        MessageBoxW(
            panel,
            wide(&message).as_ptr(),
            wide("Remove Study Pen").as_ptr(),
            MB_YESNO | MB_DEFBUTTON2 | MB_ICONWARNING | MB_TOPMOST,
        ) == IDYES
    });

    let error = APP.with(|cell| {
        let mut state = cell.borrow_mut();
        let app = state.as_mut()?;
        app.removal_dialog_open = false;
        if let Err(error) = executable {
            return Some(error);
        }
        if !approved {
            return None;
        }
        let result = (|| {
            let image = app.removal_image.as_ref().map_err(Clone::clone)?;
            RemovalPlan::prepare(image, &app.settings)?.start()
        })();
        match result {
            Ok(()) => {
                // The cleanup has captured the files' identities. Do not
                // replace a settings file with an atomic save during shutdown.
                app.removing = true;
                app.request_quit();
                None
            }
            Err(error) => Some(error),
        }
    });
    if let Some(error) = error {
        MessageBoxW(
            panel,
            wide(&error).as_ptr(),
            wide("Remove Study Pen").as_ptr(),
            MB_OK | MB_ICONERROR | MB_TOPMOST,
        );
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // Capture release position before any nested native calls. Polling the
    // cursor later could include motion that happened after the button was up.
    let release_position = if message == WM_LBUTTONUP {
        let point = GetMessagePos();
        Some(POINT {
            x: point as u16 as i16 as i32,
            y: (point >> 16) as u16 as i16 as i32,
        })
    } else {
        None
    };
    // These floating windows have no menu bar. Handle unused Alt mnemonics
    // locally instead of invoking DefWindowProc's warning sound. Standard
    // commands such as SC_CLOSE (Alt+F4) are deliberately left intact.
    if hotkeys::blocks_menu_beep(message, wparam) {
        return 0;
    }
    if message == REMOVE_REQUEST {
        confirm_removal(hwnd);
        return 0;
    }
    if message == UPDATE_REQUEST {
        request_update(hwnd);
        return 0;
    }
    // Popup menus also run a nested message loop; never retain the application
    // borrow while a tray menu is open or shortcut messages would be skipped.
    if message == tray::CALLBACK_MESSAGE {
        let (visible, legacy) = APP
            .with(|cell| {
                cell.try_borrow().ok().and_then(|state| {
                    state
                        .as_ref()
                        .map(|app| (app.ui_visible, app.tray.uses_legacy_callback()))
                })
            })
            .unwrap_or((true, false));
        tray::callback(hwnd, (lparam as u32) & 0xffff, visible, legacy);
        return 0;
    }
    // Native title-bar dragging runs a nested message loop. Release all state
    // borrows before entering it so painting, DPI changes and final position
    // persistence continue to receive their messages.
    if message == WM_NCLBUTTONDOWN {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let mut handled = false;
    let result = APP.with(|cell| {
        // Synchronous child-control notifications can reenter the window proc.
        // They are default-handled while an application operation is in flight.
        let Ok(mut guard) = cell.try_borrow_mut() else {
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        let Some(app) = guard.as_mut() else {
            drop(guard);
            return DefWindowProcW(hwnd, message, wparam, lparam);
        };
        handled = true;
        if app.removal_dialog_open
            && matches!(
                message,
                WM_HOTKEY
                    | WM_LBUTTONDOWN
                    | WM_CLOSE
                    | tray::COMMAND_MESSAGE
                    | tray::RESTORE_MESSAGE
            )
        {
            return 0;
        }
        let x = (lparam as u16 as i16) as i32;
        let y = ((lparam >> 16) as u16 as i16) as i32;
        let coordinate_scale = if hwnd == app.toolbar {
            app.toolbar_scale
        } else if hwnd == app.panel {
            app.panel_scale
        } else {
            app.scale
        };
        let logical_x = (x as f32 / coordinate_scale) as i32;
        let logical_y = (y as f32 / coordinate_scale) as i32;
        if hwnd == app.toolbar
            && app.tray.restart_message() != 0
            && message == app.tray.restart_message()
        {
            app.tray.install(app.ui_visible);
            app.ensure_tray();
            return 0;
        }
        match message {
            tray::RESTORE_MESSAGE => {
                app.set_ui_visible(true);
                0
            }
            tray::COMMAND_MESSAGE => {
                match wparam {
                    tray::COMMAND_TOGGLE => app.action(Action::ToggleUi, false),
                    tray::COMMAND_SETTINGS => app.open_panel(1),
                    tray::COMMAND_QUIT => app.action(Action::Quit, false),
                    _ => {}
                }
                0
            }
            WM_NCHITTEST if hwnd == app.toolbar || hwnd == app.panel => {
                let mut point = POINT { x, y };
                ScreenToClient(hwnd, &mut point);
                let lx = point.x as f32 / coordinate_scale;
                let ly = point.y as f32 / coordinate_scale;
                let close_button = (416.0..444.0).contains(&lx) && (16.0..44.0).contains(&ly);
                let drag_panel = hwnd == app.panel && (0.0..64.0).contains(&ly) && !close_button;
                if drag_panel {
                    HTCAPTION as isize
                } else {
                    HTCLIENT as isize
                }
            }
            WM_ERASEBKGND => 1,
            WM_PAINT => {
                let mut paint: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut paint);
                if hwnd == app.toolbar {
                    app.paint_glass();
                } else if hwnd == app.panel {
                    // One small back buffer prevents GDI control flicker.
                    let mut bounds = RECT::default();
                    GetClientRect(hwnd, &mut bounds);
                    let memory = CreateCompatibleDC(dc);
                    let bitmap = CreateCompatibleBitmap(dc, bounds.right, bounds.bottom);
                    if !memory.is_null() && !bitmap.is_null() {
                        let old = SelectObject(memory, bitmap);
                        let state = app.ui_state();
                        ui::paint_panel(memory, app.panel_scale, &state);
                        BitBlt(dc, 0, 0, bounds.right, bounds.bottom, memory, 0, 0, SRCCOPY);
                        SelectObject(memory, old);
                    }
                    if !bitmap.is_null() {
                        DeleteObject(bitmap);
                    }
                    if !memory.is_null() {
                        DeleteDC(memory);
                    }
                }
                EndPaint(hwnd, &paint);
                0
            }
            WM_MOUSEACTIVATE
                if hwnd == app.toolbar || hwnd == app.overlay || hwnd == app.radial_window =>
            {
                MA_NOACTIVATE as isize
            }
            WM_SETCURSOR if hwnd == app.radial_window => {
                SetCursor(LoadCursorW(ptr::null_mut(), IDC_ARROW));
                1
            }
            WM_SETCURSOR if hwnd == app.overlay => {
                if app.interactive {
                    SetCursor(if app.tool == Tool::Pen || app.cursor.is_null() {
                        LoadCursorW(ptr::null_mut(), IDC_CROSS)
                    } else {
                        app.cursor
                    });
                    1
                } else {
                    drop(guard);
                    DefWindowProcW(hwnd, message, wparam, lparam)
                }
            }
            WM_LBUTTONDOWN => {
                if app.radial_hold.is_some() || app.toolbar_animation.is_some() {
                    return 0;
                }
                if hwnd == app.toolbar {
                    app.toolbar_click(logical_x, logical_y, POINT { x, y });
                } else if hwnd == app.panel {
                    app.panel_click(logical_x, logical_y);
                } else if hwnd == app.overlay {
                    app.pointer_down(x, y);
                }
                0
            }
            WM_MOUSEMOVE => {
                if hwnd == app.overlay {
                    app.pointer_move(x, y);
                } else if hwnd == app.panel {
                    app.panel_move(logical_x, logical_y);
                } else if hwnd == app.toolbar {
                    if app.toolbar_drag.is_some() {
                        app.move_toolbar(None);
                        return 0;
                    }
                    app.toolbar_pointer = Some(POINT { x, y });
                    let hover = app.toolbar_layout.hit_test(logical_x, logical_y);
                    if app.hover != hover {
                        app.hover = hover;
                        app.redraw_ui();
                    }
                    let mut track = TRACKMOUSEEVENT {
                        cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                        dwFlags: TME_LEAVE,
                        hwndTrack: hwnd,
                        dwHoverTime: 0,
                    };
                    TrackMouseEvent(&mut track);
                }
                0
            }
            WM_MOUSELEAVE if hwnd == app.toolbar => {
                app.hover = None;
                app.toolbar_pointer = None;
                app.redraw_ui();
                0
            }
            WM_LBUTTONUP => {
                if hwnd == app.toolbar && app.toolbar_drag.is_some() {
                    app.move_toolbar(release_position);
                    app.finish_toolbar_drag();
                }
                if hwnd == app.overlay {
                    app.pointer_move(x, y);
                    app.finish_gesture();
                }
                if hwnd == app.panel {
                    if app.panel_drag.take().is_some() {
                        ReleaseCapture();
                    }
                    app.save();
                }
                0
            }
            WM_CAPTURECHANGED => {
                if hwnd == app.toolbar {
                    app.finish_toolbar_drag();
                }
                if hwnd == app.overlay && app.ink.is_drawing() {
                    app.ink.end();
                    app.last_point = None;
                }
                if hwnd == app.panel {
                    app.panel_drag = None;
                }
                0
            }
            WM_CANCELMODE if hwnd == app.toolbar => {
                app.finish_toolbar_drag();
                0
            }
            WM_HOTKEY if hwnd == app.toolbar => {
                let index = wparam as i32 - hotkeys::ID_BASE;
                if (0..Action::ALL.len() as i32).contains(&index) && app.capturing.is_none() {
                    let index = index as usize;
                    let current = app.settings.hotkeys[index];
                    let modifiers = (lparam as u32) & 0x000f;
                    let key = ((lparam as u32) >> 16) & 0xffff;
                    if app.hotkey_errors[index].is_none()
                        && current.is_bound()
                        && app.pending_shortcut_release != Some(key)
                        && current.modifiers == modifiers
                        && current.key == key
                    {
                        app.action(Action::ALL[index], true);
                    }
                }
                0
            }
            WM_TIMER => {
                match wparam {
                    TIMER_ERASER => app.poll_hold(),
                    TIMER_UPDATES => app.poll_updates(),
                    TIMER_INSTALL_UPDATE => app.poll_update_install(),
                    TIMER_DAILY => app.start_updates(),
                    TIMER_SAVE => app.save(),
                    TIMER_TRAY => {
                        if !app.tray.is_added() {
                            app.tray.install(app.ui_visible);
                        }
                        app.ensure_tray();
                    }
                    TIMER_CAPTURE_RELEASE => app.poll_shortcut_release(),
                    TIMER_RADIAL => app.poll_radial(),
                    TIMER_DOCK => app.poll_toolbar_animation(None),
                    TIMER_GLASS => app.paint_glass(),
                    _ => {}
                }
                0
            }
            WM_COMMAND if hwnd == app.panel => {
                let id = wparam & 0xffff;
                let notification = ((wparam >> 16) & 0xffff) as u32;
                if notification == EN_CHANGE {
                    app.edit_changed(id);
                }
                if notification == EN_KILLFOCUS && (EDIT_HEX..=EDIT_BLUE).contains(&id) {
                    if id == EDIT_HEX {
                        if let Some(color) = Color::from_hex(&window_text(app.edits[0])) {
                            app.set_color(color, false, None);
                        } else {
                            app.sync_edits(None);
                        }
                    } else {
                        app.sync_edits(None);
                    }
                }
                0
            }
            WM_CTLCOLOREDIT => {
                SetTextColor(wparam as HDC, rgb(234, 239, 236));
                SetBkColor(wparam as HDC, rgb(34, 41, 46));
                app.edit_brush as isize
            }
            WM_KILLFOCUS if hwnd == app.panel => {
                let new_focus = wparam as HWND;
                if new_focus != app.panel && !app.edits.contains(&new_focus) {
                    app.cancel_capture();
                }
                drop(guard);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
            WM_EXITSIZEMOVE if hwnd == app.toolbar => {
                app.restore_toolbar_geometry(None);
                app.save_later();
                0
            }
            WM_DISPLAYCHANGE if hwnd == app.toolbar => {
                app.display_changed();
                0
            }
            WM_DPICHANGED if hwnd == app.toolbar || hwnd == app.panel => {
                if hwnd == app.toolbar && app.toolbar_animation.is_some() {
                    return 0;
                }
                let new_scale = (wparam as u16 as f32 / 96.0).max(1.0);
                if hwnd == app.panel {
                    let mut bounds = RECT::default();
                    GetWindowRect(hwnd, &mut bounds);
                    let suggested = if lparam != 0 {
                        *(lparam as *const RECT)
                    } else {
                        bounds
                    };
                    let work = monitor_work_area(POINT {
                        x: suggested.left,
                        y: suggested.top,
                    });
                    app.apply_panel_scale(fitted_surface_scale(
                        (PANEL_WIDTH, PANEL_HEIGHT),
                        new_scale,
                        work,
                    ));
                    let (width, height) = (app.px(PANEL_WIDTH), app.px(PANEL_HEIGHT));
                    let (x, y) = constrain_toolbar_position(
                        suggested.left,
                        suggested.top,
                        width,
                        height,
                        work,
                    );
                    SetWindowPos(
                        hwnd,
                        ptr::null_mut(),
                        x,
                        y,
                        width,
                        height,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                    app.redraw_ui();
                    return 0;
                }
                app.apply_ui_scale(new_scale);
                if app.toolbar_drag.is_some() {
                    app.move_toolbar(None);
                } else if hwnd == app.toolbar && lparam != 0 {
                    let suggested = *(lparam as *const RECT);
                    let work = monitor_work_area(POINT {
                        x: suggested.left,
                        y: suggested.top,
                    });
                    app.place_toolbar(suggested.left, suggested.top, work);
                    app.remember_toolbar();
                    app.save_later();
                } else {
                    app.restore_toolbar_geometry(None);
                }
                app.redraw_ui();
                0
            }
            WM_CLOSE => {
                if hwnd == app.radial_window {
                    app.close_radial();
                } else if hwnd == app.panel {
                    app.close_panel();
                } else {
                    app.request_quit();
                }
                0
            }
            WM_QUERYENDSESSION => {
                app.finish_toolbar_animation();
                app.finish_toolbar_drag();
                app.close_radial();
                app.commit_edit();
                app.stop_hold();
                app.save();
                1
            }
            WM_ENDSESSION => {
                if wparam != 0 {
                    app.request_quit();
                }
                0
            }
            WM_DESTROY => 0,
            _ => {
                // DefWindowProc can synchronously send WM_CLOSE (Alt+F4),
                // enter a system menu, or run a move loop. Those nested
                // messages must be able to borrow the application state.
                drop(guard);
                DefWindowProcW(hwnd, message, wparam, lparam)
            }
        }
    });
    // Paint drag/animation changes before the next input sample, after releasing
    // App's borrow. Painting while it is held would default-handle WM_PAINT and
    // validate the frame without drawing it, leaving stale pixels until release.
    if handled
        && matches!(
            message,
            WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP | WM_TIMER | WM_DPICHANGED
        )
    {
        UpdateWindow(hwnd);
    }
    result
}

fn proposed_toolbar_bounds(
    layout: ToolbarLayout,
    pointer: POINT,
    drag: DragGeometry,
    scale: f32,
) -> Option<Bounds> {
    drag.proposed_bounds(DragPoint::new(pointer.x, pointer.y), layout, scale)
}

fn docked_toolbar_bounds(
    edge: Edge,
    pointer: POINT,
    drag: DragGeometry,
    dpi_scale: f32,
    work: RECT,
) -> Option<Bounds> {
    let scale = fitted_toolbar_scale(ToolbarLayout::Vertical, dpi_scale, work);
    let proposed = proposed_toolbar_bounds(ToolbarLayout::Vertical, pointer, drag, scale)?;
    Some(vertical_edge_bounds(edge, proposed.top, dpi_scale, work))
}

fn clamped_toolbar_bounds(bounds: Bounds, work: RECT) -> Bounds {
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let (left, top) = constrain_toolbar_position(bounds.left, bounds.top, width, height, work);
    Bounds::new(left, top, left + width, top + height)
}

fn horizontal_drag_preview(proposal: Bounds, work: RECT, dpi_scale: f32) -> Bounds {
    let width = proposal.right - proposal.left;
    let height = proposal.bottom - proposal.top;
    let overhang = (docking::RIGHT_OVERHANG_DIP * f64::from(dpi_scale)).ceil() as i32;
    let left = proposal
        .left
        .clamp(work.left, (work.right - width + overhang).max(work.left));
    let top = proposal
        .top
        .clamp(work.top, (work.bottom - height).max(work.top));
    Bounds::new(left, top, left + width, top + height)
}

unsafe fn seam_preserving_bounds(
    proposed: Bounds,
    actual: Bounds,
    work: RECT,
    handle: Option<DragPoint>,
) -> Bounds {
    let Some(handle) = handle else {
        return actual;
    };
    let mut aligned = actual;
    // Preserve blocked intent at a shared monitor seam until the logical
    // handle crosses it. An ordinary outer-edge clamp still discards overshoot.
    if actual.left != proposed.left {
        let direction = if proposed.left < work.left { -1 } else { 1 };
        if has_neighbor_monitor(work, handle, direction, 0) {
            aligned.left = proposed.left;
            aligned.right = proposed.right;
        }
    }
    if actual.top != proposed.top {
        let direction = if proposed.top < work.top { -1 } else { 1 };
        if has_neighbor_monitor(work, handle, 0, direction) {
            aligned.top = proposed.top;
            aligned.bottom = proposed.bottom;
        }
    }
    aligned
}

unsafe fn has_neighbor_monitor(work: RECT, handle: DragPoint, dx: i32, dy: i32) -> bool {
    let inside = POINT {
        x: handle.x.clamp(work.left, (work.right - 1).max(work.left)),
        y: handle.y.clamp(work.top, (work.bottom - 1).max(work.top)),
    };
    let current = MonitorFromPoint(inside, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(current, &mut info) == 0 {
        return false;
    }
    let probe = POINT {
        x: if dx < 0 {
            info.rcMonitor.left.saturating_sub(1)
        } else if dx > 0 {
            info.rcMonitor.right
        } else {
            inside.x
        },
        y: if dy < 0 {
            info.rcMonitor.top.saturating_sub(1)
        } else if dy > 0 {
            info.rcMonitor.bottom
        } else {
            inside.y
        },
    };
    let neighbor = MonitorFromPoint(probe, MONITOR_DEFAULTTONULL);
    !neighbor.is_null() && neighbor != current
}

fn vertical_edge_bounds(edge: Edge, top: i32, dpi_scale: f32, work: RECT) -> Bounds {
    let scale = fitted_toolbar_scale(ToolbarLayout::Vertical, dpi_scale, work);
    let (width, height) = ToolbarLayout::Vertical.size();
    let (width, height) = (
        (width as f32 * scale).round() as i32,
        (height as f32 * scale).round() as i32,
    );
    let left = match edge {
        Edge::Left => work.left,
        Edge::Right => work.right - width,
    };
    let (left, top) = constrain_toolbar_position(left, top, width, height, work);
    Bounds::new(left, top, left + width, top + height)
}

unsafe fn toolbar_region(window: HWND, bounds: Bounds, clip: Option<Bounds>) {
    let Some(work) = clip else {
        SetWindowRgn(window, ptr::null_mut(), 0);
        return;
    };
    let width = bounds.right - bounds.left;
    let height = bounds.bottom - bounds.top;
    let left = (work.left - bounds.left).clamp(0, width);
    let top = (work.top - bounds.top).clamp(0, height);
    let right = (work.right - bounds.left).clamp(left, width);
    let bottom = (work.bottom - bounds.top).clamp(top, height);
    let region = CreateRectRgn(left, top, right, bottom);
    if region.is_null() {
        return;
    }
    if SetWindowRgn(window, region, 0) == 0 {
        DeleteObject(region);
    }
}

unsafe fn monitor_work_area(point: POINT) -> RECT {
    let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if GetMonitorInfoW(monitor, &mut info) != 0 {
        info.rcWork
    } else {
        let mut work = RECT::default();
        SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut _ as *mut c_void, 0);
        work
    }
}

fn fitted_toolbar_scale(layout: ToolbarLayout, dpi_scale: f32, work: RECT) -> f32 {
    fitted_surface_scale(layout.size(), dpi_scale, work)
}

fn fitted_surface_scale((width, height): (i32, i32), dpi_scale: f32, work: RECT) -> f32 {
    dpi_scale
        .min((work.right - work.left).max(1) as f32 / width as f32)
        .min((work.bottom - work.top).max(1) as f32 / height as f32)
        .max(0.25)
}

fn constrain_toolbar_position(x: i32, y: i32, width: i32, height: i32, work: RECT) -> (i32, i32) {
    (
        x.clamp(work.left, (work.right - width).max(work.left)),
        y.clamp(work.top, (work.bottom - height).max(work.top)),
    )
}

fn pointer_is_neutral(actual: POINT, center: POINT, scale: f32) -> bool {
    let dx = (actual.x as f64 - center.x as f64) / scale as f64;
    let dy = (actual.y as f64 - center.y as f64) / scale as f64;
    dx * dx + dy * dy < (radial::DEAD_ZONE as f64).powi(2)
}

fn object_dirty_area(ink: &Ink, from: Point, to: Point, radius: f32) -> Option<Rect> {
    let sweep =
        Rect::around(from.x, from.y, radius + 2.0).union(Rect::around(to.x, to.y, radius + 2.0));
    ink.strokes()
        .iter()
        .filter_map(|stroke| {
            let bounds = stroke.bounds();
            let rect = Rect::new(
                (bounds.min_x - 3.0).floor() as i32,
                (bounds.min_y - 3.0).floor() as i32,
                (bounds.max_x + 3.0).ceil() as i32 + 1,
                (bounds.max_y + 3.0).ceil() as i32 + 1,
            );
            (rect.left < sweep.right
                && rect.right > sweep.left
                && rect.top < sweep.bottom
                && rect.bottom > sweep.top)
                .then_some(rect)
        })
        .reduce(Rect::union)
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}
unsafe fn current_modifiers() -> u32 {
    // Capture the modifier state belonging to this queued key message rather
    // than a later physical state after a fast chord has already been released.
    let down = |key| GetKeyState(key as i32) < 0;
    u32::from(down(VK_MENU))
        | (u32::from(down(VK_CONTROL)) * 2)
        | (u32::from(down(VK_SHIFT)) * 4)
        | (u32::from(down(VK_LWIN) || down(VK_RWIN)) * 8)
}
unsafe fn rounded_region(hwnd: HWND, width: i32, height: i32, radius: i32) {
    let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
    if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0 {
        DeleteObject(region);
    }
}
unsafe fn window_text(hwnd: HWND) -> String {
    let mut buffer = [0u16; 64];
    let length = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..length.max(0) as usize])
}

unsafe fn create_edit_font(scale: f32) -> HFONT {
    CreateFontW(
        -(15.0 * scale).round() as i32,
        0,
        0,
        0,
        FW_NORMAL as i32,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        OUT_DEFAULT_PRECIS as u32,
        CLIP_DEFAULT_PRECIS as u32,
        CLEARTYPE_QUALITY as u32,
        DEFAULT_PITCH as u32,
        wide("Segoe UI").as_ptr(),
    )
}

unsafe fn circle_cursor(diameter: f32) -> HCURSOR {
    let diameter = diameter.clamp(6.0, 720.0);
    let size = (diameter.ceil() as u32 + 8).min(728);
    let Some(mut pixmap) = tiny_skia::Pixmap::new(size, size) else {
        return ptr::null_mut();
    };
    let mut path = tiny_skia::PathBuilder::new();
    path.push_circle(size as f32 * 0.5, size as f32 * 0.5, diameter * 0.5);
    let Some(path) = path.finish() else {
        return ptr::null_mut();
    };
    for (shade, width) in [(20, 3.0), (245, 1.2)] {
        let mut paint = tiny_skia::Paint::default();
        paint.set_color_rgba8(shade, shade, shade, 240);
        pixmap.stroke_path(
            &path,
            &paint,
            &tiny_skia::Stroke {
                width,
                ..Default::default()
            },
            tiny_skia::Transform::identity(),
            None,
        );
    }
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: size as i32,
            biHeight: -(size as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..zeroed()
        },
        ..zeroed()
    };
    let mut bits = ptr::null_mut();
    let color = CreateDIBSection(
        ptr::null_mut(),
        &info,
        DIB_RGB_COLORS,
        &mut bits,
        ptr::null_mut(),
        0,
    );
    if color.is_null() {
        return ptr::null_mut();
    }
    ptr::copy_nonoverlapping(pixmap.data().as_ptr(), bits as *mut u8, pixmap.data().len());
    let mask = CreateBitmap(size as i32, size as i32, 1, 1, ptr::null());
    if mask.is_null() {
        DeleteObject(color);
        return ptr::null_mut();
    }
    let icon = ICONINFO {
        fIcon: 0,
        xHotspot: size / 2,
        yHotspot: size / 2,
        hbmMask: mask,
        hbmColor: color,
    };
    let cursor = CreateIconIndirect(&icon);
    DeleteObject(mask);
    DeleteObject(color);
    cursor
}

pub fn show_error(message: &str) {
    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            wide(message).as_ptr(),
            wide("Study Pen").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_drag_preview_visibly_overhangs_before_docking_and_caps_fast_drags() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        let partial = Bounds::new(1314, 200, 1936, 276);
        assert_eq!(horizontal_drag_preview(partial, work, 1.0), partial);
        assert_eq!(
            docking::choose_layout(
                ToolbarLayout::Horizontal,
                partial,
                Bounds::new(0, 0, 1920, 1040),
                1.0
            ),
            ToolbarLayout::Horizontal
        );

        let far_out = Bounds::new(1600, 200, 2222, 276);
        let source = horizontal_drag_preview(far_out, work, 1.0);
        assert_eq!(source, Bounds::new(1330, 200, 1952, 276));
        assert_eq!(
            docking::choose_layout(
                ToolbarLayout::Horizontal,
                far_out,
                Bounds::new(0, 0, 1920, 1040),
                1.0
            ),
            ToolbarLayout::Vertical
        );
        // A release before docking restores every horizontal button on screen.
        assert_eq!(
            constrain_toolbar_position(partial.left, partial.top, 622, 76, work),
            (1298, 200)
        );
    }

    #[test]
    fn horizontal_preview_scales_its_overhang_and_keeps_vertical_bounds_visible() {
        let work = RECT {
            left: -3840,
            top: -100,
            right: 0,
            bottom: 2060,
        };
        assert_eq!(
            horizontal_drag_preview(Bounds::new(-1200, 2000, 44, 2152), work, 2.0),
            Bounds::new(-1200, 1908, 44, 2060)
        );
        assert_eq!(
            horizontal_drag_preview(Bounds::new(-1100, -200, 144, -48), work, 2.0),
            Bounds::new(-1180, -100, 64, 52)
        );
        assert_eq!(
            horizontal_drag_preview(Bounds::new(-3900, 0, -2656, 152), work, 2.0),
            Bounds::new(-3840, 0, -2596, 152)
        );
    }

    #[test]
    fn vertical_toolbar_fits_high_dpi_work_area_without_changing_brush_scale() {
        let work = RECT {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 900,
        };
        let dpi_scale = 2.0;
        let scale = fitted_toolbar_scale(ToolbarLayout::Vertical, dpi_scale, work);
        assert!(scale < dpi_scale);
        let (width, height) = ToolbarLayout::Vertical.size();
        assert!((height as f32 * scale).round() as i32 <= work.bottom);
        assert!((width as f32 * scale).round() as i32 <= work.right);
        assert_eq!(
            fitted_toolbar_scale(ToolbarLayout::Horizontal, dpi_scale, work),
            dpi_scale
        );
    }

    #[test]
    fn restored_toolbar_stays_in_its_monitor_work_area_after_rotation() {
        let work = RECT {
            left: -1920,
            top: -200,
            right: 0,
            bottom: 840,
        };
        assert_eq!(
            constrain_toolbar_position(-10, 800, 76, 622, work),
            (-76, 218)
        );
        assert_eq!(
            constrain_toolbar_position(-3000, -800, 622, 76, work),
            (-1920, -200)
        );
        assert_eq!(
            constrain_toolbar_position(-900, 200, 622, 76, work),
            (-900, 200)
        );
    }

    #[test]
    fn radial_hover_requires_stability_and_only_commits_once() {
        let start = Instant::now();
        let mut dwell = HoverDwell::default();
        assert_eq!(dwell.sample(Some(1), start), None);
        assert_eq!(dwell.sample(Some(1), start + RADIAL_DWELL / 2), None);
        assert_eq!(dwell.sample(Some(1), start + RADIAL_DWELL), Some(1));
        assert_eq!(dwell.sample(Some(1), start + RADIAL_DWELL * 10), None);
    }

    #[test]
    fn clipped_pointer_outside_the_neutral_hub_cannot_arm_the_wheel() {
        let center = POINT { x: -1920, y: 1080 };
        assert!(pointer_is_neutral(center, center, 1.0));
        assert!(pointer_is_neutral(POINT { x: -1880, y: 1080 }, center, 2.0));
        assert!(!pointer_is_neutral(
            POINT { x: -1820, y: 1080 },
            center,
            2.0
        ));
        assert!(!pointer_is_neutral(
            POINT {
                x: i32::MAX,
                y: i32::MIN
            },
            center,
            1.0
        ));
    }

    #[test]
    fn radial_hub_and_sector_changes_restart_selection_delay() {
        let start = Instant::now();
        let mut dwell = HoverDwell::default();
        assert_eq!(dwell.sample(Some(3), start), None);
        assert_eq!(dwell.sample(None, start + RADIAL_DWELL), None);
        assert_eq!(dwell.sample(Some(3), start + RADIAL_DWELL * 2), None);
        assert_eq!(dwell.sample(Some(0), start + RADIAL_DWELL * 3), None);
        assert_eq!(dwell.sample(Some(0), start + RADIAL_DWELL * 4), Some(0));
        // A new color page gets a fresh dwell, even at the same sector index.
        dwell = HoverDwell::default();
        assert_eq!(dwell.sample(Some(0), start + RADIAL_DWELL * 5), None);
    }

    #[test]
    fn shortcut_capture_reads_queued_modifier_state() {
        struct RestoreKeyboardState([u8; 256]);
        impl Drop for RestoreKeyboardState {
            fn drop(&mut self) {
                unsafe {
                    SetKeyboardState(self.0.as_ptr());
                }
            }
        }
        // SetKeyboardState changes only this test thread's message-queue state;
        // it does not press keys or change the user's physical keyboard state.
        unsafe {
            let mut original = [0u8; 256];
            assert_ne!(GetKeyboardState(original.as_mut_ptr()), 0);
            let _restore = RestoreKeyboardState(original);
            let mut queued = [0u8; 256];
            for key in [
                VK_CONTROL,
                VK_LCONTROL,
                VK_MENU,
                VK_LMENU,
                VK_SHIFT,
                VK_LSHIFT,
            ] {
                queued[key as usize] = 0x80;
            }
            assert_ne!(SetKeyboardState(queued.as_ptr()), 0);
            assert_eq!(current_modifiers(), 7);
            queued.fill(0);
            assert_ne!(SetKeyboardState(queued.as_ptr()), 0);
            assert_eq!(current_modifiers(), 0);
        }
    }

    #[test]
    fn object_erasure_repaints_whole_candidates_and_ignores_empty_space() {
        let mut ink = Ink::new();
        ink.begin(Point::new(20.0, 100.0), Tool::Pen, Color::default(), 12.0);
        ink.extend(Point::new(900.0, 100.0));
        ink.end();
        let point = Point::new(450.0, 100.0);
        let dirty = object_dirty_area(&ink, point, point, 11.0).unwrap();
        assert!(dirty.left < 14 && dirty.right > 906);
        assert!(dirty.top < 94 && dirty.bottom > 106);
        assert!(
            object_dirty_area(
                &ink,
                Point::new(500.0, 250.0),
                Point::new(600.0, 300.0),
                11.0
            )
            .is_none()
        );
    }
}
