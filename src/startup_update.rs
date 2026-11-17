//! Update progress and installation at startup or from Settings.

use crate::{
    install_update,
    settings::Settings,
    splash::{self, Stage},
    updates::{self, ReleaseInfo, UpdateStatus},
};
use std::{
    cell::Cell,
    mem::size_of,
    ptr,
    sync::mpsc::{self, Receiver, TryRecvError},
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    UI::{
        HiDpi::GetDpiForWindow,
        Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE},
        WindowsAndMessaging::*,
    },
};

const CLASS: &str = "StudyPen.Startup";
const POLL_TIMER: usize = 1;
const SKIP_BUTTON: usize = 2;
const CHECK_LIMIT: Duration = Duration::from_secs(10);
const SPLASH_MINIMUM: Duration = Duration::from_millis(350);
const DOWNLOAD_MINIMUM: Duration = Duration::from_millis(1200);
pub const INSTALL_POLL_MS: u32 = 50;
const RESTART_ARG: &str = "--after-update";

pub enum Launch {
    BackgroundCheck,
    Open {
        status: UpdateStatus,
        install_error: Option<String>,
    },
    Exit,
}

impl Launch {
    fn open(status: UpdateStatus) -> Self {
        Self::Open {
            status,
            install_error: None,
        }
    }
}

pub unsafe fn run(settings: &Settings, instance: HINSTANCE) -> Launch {
    if !settings.check_for_updates {
        return Launch::open(UpdateStatus::Unconfigured);
    }
    let Some(repository) = updates::repository(settings.github_repository.as_deref()) else {
        return Launch::open(UpdateStatus::Unconfigured);
    };
    if std::env::args_os().any(|arg| arg == RESTART_ARG) {
        return Launch::BackgroundCheck;
    }
    let Some(window) = Window::new(instance, ptr::null_mut()) else {
        return Launch::BackgroundCheck;
    };
    if SetTimer(window.handle, POLL_TIMER, INSTALL_POLL_MS, None) == 0 {
        return Launch::BackgroundCheck;
    }
    window.show();

    let repository = repository.to_owned();
    let now = Instant::now();
    let result = match worker(move || updates::check(Some(&repository), env!("CARGO_PKG_VERSION")))
    {
        Ok(receiver) => window.wait(&receiver, now + SPLASH_MINIMUM, Some(now + CHECK_LIMIT)),
        Err(_) => Wait::Failed,
    };
    let status = match result {
        Wait::Ready(status) => status,
        Wait::Skipped => UpdateStatus::Failed("Update check skipped for this launch.".into()),
        Wait::TimedOut => UpdateStatus::Failed("The startup update check timed out.".into()),
        Wait::Failed => UpdateStatus::Failed("Could not complete the startup update check.".into()),
        Wait::Quit => return Launch::Exit,
    };
    let UpdateStatus::Available(release) = &status else {
        return Launch::open(status);
    };
    if !release.installable() {
        return Launch::open(status);
    }
    let Some(asset) = release.asset.clone() else {
        return Launch::open(status);
    };
    let version = release.version.clone();
    window.set_stage(Stage::Downloading);
    let result = match worker(move || install_update::stage(&asset, &version)) {
        Ok(receiver) => window.wait(&receiver, Instant::now() + DOWNLOAD_MINIMUM, None),
        Err(_) => Wait::Failed,
    };
    let install = match result {
        Wait::Ready(Ok(staged)) => staged,
        Wait::Ready(Err(error)) => {
            return Launch::Open {
                status,
                install_error: Some(error),
            };
        }
        Wait::Skipped => return Launch::open(status),
        Wait::Quit => return Launch::Exit,
        Wait::TimedOut | Wait::Failed => {
            return Launch::Open {
                status,
                install_error: Some("Could not prepare the startup update.".into()),
            };
        }
    };

    // Only this phase arms replacement. Skipping during download drops the
    // staging result without ever handing it off to the installation helper.
    window.set_stage(Stage::Installing);
    let result = match worker(move || install.start()) {
        Ok(receiver) => window.wait(&receiver, Instant::now(), None),
        Err(_) => Wait::Failed,
    };
    match result {
        Wait::Ready(Ok(())) | Wait::Quit => Launch::Exit,
        Wait::Ready(Err(error)) => Launch::Open {
            status,
            install_error: Some(error),
        },
        _ => Launch::Open {
            status,
            install_error: Some("Could not start the update installer.".into()),
        },
    }
}

fn worker<T: Send + 'static>(
    task: impl FnOnce() -> T + Send + 'static,
) -> std::io::Result<Receiver<T>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("study-pen-update".into())
        .spawn(move || {
            let _ = sender.send(task());
        })?;
    Ok(receiver)
}

enum InstallWork {
    Downloading(
        Receiver<Result<install_update::StagedUpdate, String>>,
        Instant,
    ),
    Installing(Receiver<Result<(), String>>),
}

pub enum InstallResult {
    Canceled,
    Finished(Result<(), String>),
}

pub struct Installation {
    window: Window,
    work: InstallWork,
}

impl Installation {
    pub unsafe fn begin(
        instance: HINSTANCE,
        owner: HWND,
        release: ReleaseInfo,
    ) -> Result<Self, String> {
        if !release.installable() {
            return Err("The update package could not be verified.".into());
        }
        let asset = release.asset.ok_or("The release has no update package.")?;
        let window = Window::new(instance, owner).ok_or("Could not open the update screen.")?;
        window.set_stage(Stage::Downloading);
        let receiver = worker(move || install_update::stage(&asset, &release.version))
            .map_err(|_| "Could not start the update download.")?;
        window.show();
        Ok(Self {
            window,
            work: InstallWork::Downloading(receiver, Instant::now() + DOWNLOAD_MINIMUM),
        })
    }

    pub fn stage(&self) -> Stage {
        self.window.state.stage.get()
    }

    pub unsafe fn handle_escape(&self, message: &MSG) -> bool {
        if message.message == WM_KEYDOWN
            && message.wParam == VK_ESCAPE as usize
            && (message.hwnd == self.window.handle || message.hwnd == self.window.button)
        {
            SendMessageW(self.window.handle, WM_CLOSE, 0, 0);
            return true;
        }
        false
    }

    pub unsafe fn poll(&mut self) -> Option<InstallResult> {
        if self.window.state.skipped.get()
            || (self.window.state.ending_session.get()
                && matches!(self.work, InstallWork::Downloading(..)))
        {
            return Some(InstallResult::Canceled);
        }
        match &self.work {
            InstallWork::Downloading(receiver, ready_at) => {
                if Instant::now() < *ready_at {
                    return None;
                }
                let staged = match receive_install_result(receiver)? {
                    Ok(staged) => staged,
                    Err(error) => return Some(InstallResult::Finished(Err(error))),
                };
                self.window.set_stage(Stage::Installing);
                match worker(move || staged.start()) {
                    Ok(receiver) => {
                        self.work = InstallWork::Installing(receiver);
                        None
                    }
                    Err(_) => Some(InstallResult::Finished(Err(
                        "Could not start the update installer.".into(),
                    ))),
                }
            }
            InstallWork::Installing(receiver) => {
                receive_install_result(receiver).map(InstallResult::Finished)
            }
        }
    }
}

fn receive_install_result<T>(receiver: &Receiver<Result<T, String>>) -> Option<Result<T, String>> {
    match receiver.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err("The update worker did not finish.".into())),
    }
}

enum Wait<T> {
    Ready(T),
    Skipped,
    TimedOut,
    Failed,
    Quit,
}

struct WindowState {
    scale: Cell<f32>,
    stage: Cell<Stage>,
    skipped: Cell<bool>,
    ending_session: Cell<bool>,
}

struct Window {
    handle: HWND,
    button: HWND,
    state: Box<WindowState>,
}

impl Window {
    unsafe fn new(instance: HINSTANCE, owner: HWND) -> Option<Self> {
        let name = wide(CLASS);
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: name.as_ptr(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            ..Default::default()
        };
        if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            return None;
        }
        let mut state = Box::new(WindowState {
            scale: Cell::new(1.0),
            stage: Cell::new(Stage::Checking),
            skipped: Cell::new(false),
            ending_session: Cell::new(false),
        });
        let monitor = if owner.is_null() {
            let mut cursor = POINT::default();
            GetCursorPos(&mut cursor);
            MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(owner, MONITOR_DEFAULTTONEAREST)
        };
        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if GetMonitorInfoW(monitor, &mut monitor_info) == 0 {
            return None;
        }
        let work = monitor_info.rcWork;
        let work_width = work.right - work.left;
        let work_height = work.bottom - work.top;
        let center_x = work.left + work_width / 2;
        let center_y = work.top + work_height / 2;
        let handle = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            name.as_ptr(),
            wide("Study Pen — Checking for updates").as_ptr(),
            WS_POPUP | WS_CLIPCHILDREN,
            center_x,
            center_y,
            1,
            1,
            owner,
            ptr::null_mut(),
            instance,
            (&raw mut *state).cast(),
        );
        if handle.is_null() {
            return None;
        }
        let mut window = Self {
            handle,
            button: ptr::null_mut(),
            state,
        };
        let scale = (GetDpiForWindow(handle).max(96) as f32 / 96.0)
            .min(work_width as f32 / splash::WIDTH as f32)
            .min(work_height as f32 / splash::HEIGHT as f32);
        window.state.scale.set(scale);
        let px = |value: i32| (value as f32 * scale).round() as i32;
        let width = px(splash::WIDTH);
        let height = px(splash::HEIGHT);
        if SetWindowPos(
            handle,
            HWND_TOPMOST,
            center_x - width / 2,
            center_y - height / 2,
            width,
            height,
            SWP_NOACTIVATE,
        ) == 0
        {
            return None;
        }
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, px(26), px(26));
        if !region.is_null() && SetWindowRgn(handle, region, 1) == 0 {
            DeleteObject(region);
        }
        window.button = CreateWindowExW(
            0,
            wide("BUTTON").as_ptr(),
            wide(if owner.is_null() {
                "Skip for now"
            } else {
                "Cancel"
            })
            .as_ptr(),
            WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON as u32,
            px(222),
            px(106),
            px(114),
            px(28),
            handle,
            SKIP_BUTTON as HMENU,
            instance,
            ptr::null_mut(),
        );
        if window.button.is_null() {
            return None;
        }
        SendMessageW(
            window.button,
            WM_SETFONT,
            GetStockObject(DEFAULT_GUI_FONT) as usize,
            0,
        );
        Some(window)
    }

    unsafe fn show(&self) {
        ShowWindow(self.handle, SW_SHOW);
        SetFocus(self.button);
        UpdateWindow(self.handle);
    }

    unsafe fn set_stage(&self, stage: Stage) {
        self.state.stage.set(stage);
        SetWindowTextW(
            self.handle,
            wide(&format!("Study Pen — {}", stage.status())).as_ptr(),
        );
        if stage == Stage::Installing {
            ShowWindow(self.button, SW_HIDE);
        }
        InvalidateRect(self.handle, ptr::null(), 0);
        UpdateWindow(self.handle);
    }

    unsafe fn wait<T>(
        &self,
        receiver: &Receiver<T>,
        not_before: Instant,
        deadline: Option<Instant>,
    ) -> Wait<T> {
        loop {
            if self.state.ending_session.get() {
                return Wait::Quit;
            }
            if self.state.skipped.get() {
                return Wait::Skipped;
            }
            let now = Instant::now();
            if now >= not_before {
                match receiver.try_recv() {
                    Ok(result) => return Wait::Ready(result),
                    Err(TryRecvError::Disconnected) => return Wait::Failed,
                    Err(TryRecvError::Empty) => {}
                }
            }
            if deadline.is_some_and(|limit| now >= limit) {
                return Wait::TimedOut;
            }
            let mut message = MSG::default();
            let received = GetMessageW(&mut message, ptr::null_mut(), 0, 0);
            if received == 0 {
                return Wait::Quit;
            }
            if received == -1 {
                return Wait::Failed;
            }
            if message.message == WM_KEYDOWN && message.wParam == VK_ESCAPE as usize {
                SendMessageW(self.handle, WM_CLOSE, 0, 0);
            } else {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            KillTimer(self.handle, POLL_TIMER);
            DestroyWindow(self.handle);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return 1;
    }
    let Some(state) = (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const WindowState).as_ref()
    else {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    };
    match message {
        WM_PAINT => {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut paint);
            splash::paint(dc, state.scale.get(), state.stage.get());
            EndPaint(hwnd, &paint);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CLOSE => {
            if state.stage.get() != Stage::Installing {
                state.skipped.set(true);
            }
            0
        }
        WM_COMMAND if wparam == SKIP_BUTTON => {
            if state.stage.get() != Stage::Installing {
                state.skipped.set(true);
            }
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            state.ending_session.set(wparam != 0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;

    #[test]
    fn splash_can_skip_download_but_not_the_installer_handoff() {
        unsafe {
            let window = Window::new(GetModuleHandleW(ptr::null()), ptr::null_mut())
                .expect("create startup window");
            window.set_stage(Stage::Downloading);
            SendMessageW(
                window.handle,
                WM_COMMAND,
                SKIP_BUTTON,
                window.button as isize,
            );
            let (sender, receiver) = mpsc::channel();
            sender.send(()).unwrap();
            assert!(matches!(
                window.wait(&receiver, Instant::now(), None),
                Wait::Skipped
            ));

            window.state.skipped.set(false);
            window.set_stage(Stage::Installing);
            SendMessageW(
                window.handle,
                WM_COMMAND,
                SKIP_BUTTON,
                window.button as isize,
            );
            SendMessageW(window.handle, WM_CLOSE, 0, 0);
            assert!(matches!(
                window.wait(&receiver, Instant::now(), None),
                Wait::Ready(())
            ));
        }
    }

    #[test]
    fn manual_install_waits_for_the_splash_and_can_cancel_before_handoff() {
        unsafe {
            let instance = GetModuleHandleW(ptr::null());
            let startup = Window::new(instance, ptr::null_mut()).unwrap();
            drop(startup);
            let window = Window::new(instance, ptr::null_mut()).unwrap();
            window.set_stage(Stage::Downloading);
            let (sender, receiver) = mpsc::channel();
            sender.send(Err("Download failed.".into())).unwrap();
            let mut install = Installation {
                window,
                work: InstallWork::Downloading(receiver, Instant::now() + DOWNLOAD_MINIMUM),
            };
            assert!(install.poll().is_none());
            let key = MSG {
                hwnd: install.window.button,
                message: WM_KEYDOWN,
                wParam: VK_ESCAPE as usize,
                ..Default::default()
            };
            assert!(install.handle_escape(&key));
            assert!(matches!(install.poll(), Some(InstallResult::Canceled)));
        }
    }
}
