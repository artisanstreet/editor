use crate::{
    desktop::{
        Button, CaptureError, ClipboardError, CursorError, Desktop, DisplayError, Input,
        InputError, LaunchError, Point, ProcessesError, Rect, WindowChange, WindowError, WindowId,
        WindowInfo,
    },
    frame::Frame,
    steps::{Key, Named},
};
use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
        },
        Graphics::Gdi::{
            BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleDC,
            CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, EnumDisplayMonitors,
            GdiFlush, GetDC, GetMonitorInfoW, HBITMAP, HDC, HGDIOBJ, HMONITOR, MONITORINFOEXW,
            ReleaseDC, SRCCOPY, SelectObject,
        },
        System::{
            DataExchange::{
                CloseClipboard, EmptyClipboard, GetClipboardData, IsClipboardFormatAvailable,
                OpenClipboard, SetClipboardData,
            },
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            SystemInformation::GetTickCount64,
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                QueryFullProcessImageNameW,
            },
        },
        UI::{
            HiDpi::{
                DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
                SetThreadDpiAwarenessContext,
            },
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS,
                KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
                MOUSE_EVENT_FLAGS, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
                MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
                MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
                MOUSEEVENTF_WHEEL, MOUSEINPUT, SendInput, VIRTUAL_KEY, VK_APPS, VK_BACK,
                VK_CAPITAL, VK_CONTROL, VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_HOME,
                VK_INSERT, VK_LEFT, VK_LWIN, VK_MENU, VK_NEXT, VK_NUMLOCK, VK_PAUSE, VK_PRIOR,
                VK_RETURN, VK_RIGHT, VK_SCROLL, VK_SHIFT, VK_SNAPSHOT, VK_SPACE, VK_TAB, VK_UP,
                VkKeyScanW,
            },
            WindowsAndMessaging::{
                CallNextHookEx, CreateWindowExW, DestroyWindow, DispatchMessageW, EnumWindows,
                GetCursorPos, GetForegroundWindow, GetMessageW, GetSystemMetrics, GetWindowRect,
                GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, HWND_MESSAGE,
                IsIconic, IsWindow, IsWindowVisible, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT,
                PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
                SM_YVIRTUALSCREEN, SW_MAXIMIZE, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER,
                SetForegroundWindow, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage,
                UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE,
                WM_CLOSE,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

static ACTIVITY: AtomicU64 = AtomicU64::new(0);
static LAST_INPUT: AtomicU64 = AtomicU64::new(0);
static HOOKS: OnceLock<Result<(), Arc<HookError>>> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub(crate) enum HookError {
    #[error("start the user input thread")]
    Thread(#[source] std::io::Error),
    #[error("install the {hook} hook")]
    Install {
        hook: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("wait for the input hooks")]
    NoAnswer(#[source] std::sync::mpsc::RecvTimeoutError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StartError {
    #[error(
        "set ARTISAN_COMPUTER_DISPLAY to the virtual monitor device name, such as \\\\.\\DISPLAY3, before starting Forge"
    )]
    Unconfigured(#[source] std::env::VarError),
    #[error("check the agent display")]
    Display(#[source] DisplayError),
    #[error("watch for the user's own input")]
    Hooks(#[source] Arc<HookError>),
}
const MARKER: usize = 0x4152_5443;
fn physical_input() {
    LAST_INPUT.store(unsafe { GetTickCount64() }, Ordering::SeqCst);
    ACTIVITY.fetch_add(1, Ordering::SeqCst);
}
unsafe extern "system" fn mouse_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = unsafe { &*(l.0 as *const MSLLHOOKSTRUCT) };
        if event.dwExtraInfo != MARKER {
            physical_input();
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}
unsafe extern "system" fn key_hook(code: i32, w: WPARAM, l: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = unsafe { &*(l.0 as *const KBDLLHOOKSTRUCT) };
        if event.dwExtraInfo != MARKER {
            physical_input();
        }
    }
    unsafe { CallNextHookEx(None, code, w, l) }
}
fn release(what: &'static str, result: windows::core::Result<()>) {
    if let Err(error) = result
        && error.code().is_err()
    {
        tracing::debug!("release {what}: {error}");
    }
}
fn hooks() -> Result<(), Arc<HookError>> {
    let result = HOOKS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("computer-user-input".into())
            .spawn(move || unsafe {
                physical_input();
                let mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) {
                    Ok(h) => h,
                    Err(source) => {
                        let _requester_gave_up = tx.send(Err(HookError::Install {
                            hook: "mouse",
                            source,
                        }));
                        return;
                    }
                };
                let key = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), None, 0) {
                    Ok(h) => h,
                    Err(source) => {
                        release("the mouse hook", UnhookWindowsHookEx(mouse));
                        let _requester_gave_up = tx.send(Err(HookError::Install {
                            hook: "keyboard",
                            source,
                        }));
                        return;
                    }
                };
                let _requester_gave_up = tx.send(Ok(()));
                let mut message = MSG::default();
                while GetMessageW(&raw mut message, None, 0, 0).0 > 0 {
                    let _produced_a_character = TranslateMessage(&raw const message);
                    DispatchMessageW(&raw const message);
                }
                release("the keyboard hook", UnhookWindowsHookEx(key));
                release("the mouse hook", UnhookWindowsHookEx(mouse));
            })
        {
            return Err(Arc::new(HookError::Thread(error)));
        }
        rx.recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|error| Err(HookError::NoAnswer(error)))
            .map_err(Arc::new)
    });
    result.clone()
}
struct Dpi(DPI_AWARENESS_CONTEXT);
impl Dpi {
    fn enter() -> Self {
        Self(unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) })
    }
}
impl Drop for Dpi {
    fn drop(&mut self) {
        unsafe {
            SetThreadDpiAwarenessContext(self.0);
        }
    }
}
fn rect(r: RECT) -> Rect {
    Rect {
        x: r.left,
        y: r.top,
        width: (r.right - r.left).max(0).cast_unsigned(),
        height: (r.bottom - r.top).max(0).cast_unsigned(),
    }
}
const PATH_UNITS: u32 = 32768;
fn struct_size<T>() -> u32 {
    u32::try_from(size_of::<T>()).expect("invariant: Win32 structs are far smaller than 4 GiB")
}
fn absolute(offset: i32, extent: i32) -> i32 {
    let scaled = (i64::from(offset) * 65535 / i64::from(extent - 1)).clamp(0, 65535);
    i32::try_from(scaled).expect("invariant: clamped to the absolute input range")
}
fn hwnd(id: WindowId) -> HWND {
    HWND(usize::try_from(id).expect("invariant: window ids are pointer-sized handles") as *mut _)
}
fn window_id(window: HWND) -> WindowId {
    window.0 as usize as u64
}
struct Monitor {
    name: String,
    rect: Rect,
    primary: bool,
}
unsafe extern "system" fn monitor_callback(
    monitor: HMONITOR,
    _: HDC,
    _: *mut RECT,
    data: LPARAM,
) -> windows::core::BOOL {
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = struct_size::<MONITORINFOEXW>();
    if unsafe { GetMonitorInfoW(monitor, &raw mut info.monitorInfo) }.as_bool() {
        let end = info
            .szDevice
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(info.szDevice.len());
        unsafe { &mut *(data.0 as *mut Vec<Monitor>) }.push(Monitor {
            name: String::from_utf16_lossy(&info.szDevice[..end]),
            rect: rect(info.monitorInfo.rcMonitor),
            primary: info.monitorInfo.dwFlags & 1 != 0,
        });
    }
    true.into()
}
fn monitors() -> Result<Vec<Monitor>, DisplayError> {
    let _dpi = Dpi::enter();
    let mut list = Vec::new();
    unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(monitor_callback),
            LPARAM((&raw mut list) as isize),
        )
    }
    .ok()
    .map_err(DisplayError::Monitors)?;
    Ok(list)
}
pub(crate) struct WindowsDesktop {
    display: String,
}
impl WindowsDesktop {
    pub(crate) fn new() -> Result<Self, StartError> {
        let display =
            std::env::var("ARTISAN_COMPUTER_DISPLAY").map_err(StartError::Unconfigured)?;
        let desktop = Self { display };
        desktop.agent_display().map_err(StartError::Display)?;
        hooks().map_err(StartError::Hooks)?;
        Ok(desktop)
    }
}
unsafe extern "system" fn window_callback(window: HWND, data: LPARAM) -> windows::core::BOOL {
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return true.into();
    }
    let mut bounds = RECT::default();
    if unsafe { GetWindowRect(window, &raw mut bounds) }.is_err() {
        return true.into();
    }
    let mut title = vec![
        0;
        (usize::try_from(unsafe { GetWindowTextLengthW(window) }).unwrap_or(0) + 1)
            .min(32768)
    ];
    let length = usize::try_from(unsafe { GetWindowTextW(window, &mut title) }).unwrap_or(0);
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, Some(&raw mut pid));
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .ok()
        .map(|handle| {
            let mut length = PATH_UNITS;
            let mut path = vec![0; PATH_UNITS as usize];
            let result = unsafe {
                QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &raw mut length,
                )
            };
            release("the process handle", unsafe { CloseHandle(handle) });
            if result.is_ok() {
                String::from_utf16_lossy(&path[..length as usize])
                    .rsplit('\\')
                    .next()
                    .unwrap_or("")
                    .to_owned()
            } else {
                String::new()
            }
        })
        .unwrap_or_default();
    unsafe { &mut *(data.0 as *mut Vec<WindowInfo>) }.push(WindowInfo {
        id: window_id(window),
        title: String::from_utf16_lossy(&title[..length]),
        process,
        pid,
        rect: rect(bounds),
        minimized: unsafe { IsIconic(window) }.as_bool(),
    });
    true.into()
}
struct Capture {
    screen: HDC,
    memory: HDC,
    bitmap: HBITMAP,
    previous: HGDIOBJ,
}
impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            if !self.previous.is_invalid() {
                SelectObject(self.memory, self.previous);
            }
            if !self.bitmap.is_invalid() {
                release("the capture bitmap", DeleteObject(self.bitmap.into()).ok());
            }
            if !self.memory.is_invalid() {
                release("the capture memory context", DeleteDC(self.memory).ok());
            }
            ReleaseDC(None, self.screen);
        }
    }
}
struct Clipboard(HWND);
impl Clipboard {
    fn open() -> Result<Self, ClipboardError> {
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                windows::core::w!("STATIC"),
                PCWSTR::null(),
                WINDOW_STYLE::default(),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                None,
                None,
            )
        }
        .map_err(|source| ClipboardError::System {
            step: "create the clipboard owner window",
            source,
        })?;
        for _ in 0..10 {
            if unsafe { OpenClipboard(Some(window)) }.is_ok() {
                return Ok(Self(window));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        release("the clipboard owner window", unsafe {
            DestroyWindow(window)
        });
        Err(ClipboardError::Busy)
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            release("the clipboard", CloseClipboard());
            release("the clipboard owner window", DestroyWindow(self.0));
        }
    }
}

impl Desktop for WindowsDesktop {
    fn agent_display(&self) -> Result<Rect, DisplayError> {
        let display = monitors()?
            .into_iter()
            .find(|m| m.name.eq_ignore_ascii_case(&self.display))
            .ok_or_else(|| DisplayError::Disconnected {
                display: self.display.clone(),
            })?;
        if display.primary {
            return Err(DisplayError::Primary {
                display: self.display.clone(),
            });
        }
        if display.rect.width == 0 || display.rect.height == 0 {
            return Err(DisplayError::Empty {
                display: self.display.clone(),
            });
        }
        Ok(display.rect)
    }
    fn user_display(&self) -> Result<Rect, DisplayError> {
        Ok(monitors()?
            .into_iter()
            .find(|m| m.primary)
            .ok_or(DisplayError::NoPrimary)?
            .rect)
    }
    fn capture(&self, area: Rect) -> Result<Frame, CaptureError> {
        let _dpi = Dpi::enter();
        if area.width == 0
            || area.height == 0
            || u64::from(area.width) * u64::from(area.height) > 64_000_000
        {
            return Err(CaptureError::Size { area });
        }
        let failed = |step| CaptureError::Failed { area, step };
        let system = |step| move |source| CaptureError::System { area, step, source };
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(failed("GetDC"));
            }
            let mut capture = Capture {
                screen,
                memory: HDC::default(),
                bitmap: HBITMAP::default(),
                previous: HGDIOBJ::default(),
            };
            capture.memory = CreateCompatibleDC(Some(screen));
            if capture.memory.is_invalid() {
                return Err(failed("CreateCompatibleDC"));
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: struct_size::<BITMAPINFOHEADER>(),
                    biWidth: area.width.cast_signed(),
                    biHeight: -area.height.cast_signed(),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = std::ptr::null_mut();
            capture.bitmap = CreateDIBSection(
                Some(screen),
                &raw const info,
                DIB_RGB_COLORS,
                &raw mut pixels,
                None,
                0,
            )
            .map_err(system("CreateDIBSection"))?;
            capture.previous = SelectObject(capture.memory, capture.bitmap.into());
            if capture.previous.is_invalid() {
                return Err(failed("SelectObject"));
            }
            BitBlt(
                capture.memory,
                0,
                0,
                area.width.cast_signed(),
                area.height.cast_signed(),
                Some(screen),
                area.x,
                area.y,
                SRCCOPY | CAPTUREBLT,
            )
            .map_err(system("BitBlt"))?;
            GdiFlush().ok().map_err(system("GdiFlush"))?;
            let bgra = std::slice::from_raw_parts(
                pixels.cast::<u8>(),
                area.width as usize * area.height as usize * 4,
            );
            let pixels = bgra
                .as_chunks::<4>()
                .0
                .iter()
                .map(|[blue, green, red, _]| [*red, *green, *blue])
                .collect();
            Frame::new(area.width, area.height, pixels)
                .map_err(|source| CaptureError::Frame { area, source })
        }
    }
    fn cursor(&self) -> Result<Point, CursorError> {
        let _dpi = Dpi::enter();
        let mut p = POINT::default();
        unsafe { GetCursorPos(&raw mut p) }.map_err(CursorError::System)?;
        Ok(Point { x: p.x, y: p.y })
    }
    fn send(&self, input: &Input) -> Result<(), InputError> {
        let _dpi = Dpi::enter();
        let mouse = |flags: MOUSE_EVENT_FLAGS, data: u32, dx: i32, dy: i32| INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: data,
                    dwFlags: flags,
                    dwExtraInfo: MARKER,
                    ..Default::default()
                },
            },
        };
        let keyboard = |vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    dwExtraInfo: MARKER,
                    ..Default::default()
                },
            },
        };
        let events = match *input {
            Input::MoveTo(p) => {
                let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
                let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
                let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
                let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
                if width <= 1 || height <= 1 {
                    return Err(InputError::VirtualDesktop { width, height });
                }
                vec![mouse(
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    0,
                    absolute(p.x - x, width),
                    absolute(p.y - y, height),
                )]
            }
            Input::Button { button, down } => vec![mouse(
                match (button, down) {
                    (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
                    (Button::Left, false) => MOUSEEVENTF_LEFTUP,
                    (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                    (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
                    (Button::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                    (Button::Middle, false) => MOUSEEVENTF_MIDDLEUP,
                },
                0,
                0,
                0,
            )],
            Input::Wheel { dx, dy } => {
                let mut events = Vec::new();
                if dy != 0 {
                    events.push(mouse(MOUSEEVENTF_WHEEL, (-dy * 120).cast_unsigned(), 0, 0));
                }
                if dx != 0 {
                    events.push(mouse(MOUSEEVENTF_HWHEEL, (dx * 120).cast_unsigned(), 0, 0));
                }
                events
            }
            Input::Key { key, down } => {
                let vk = virtual_key(key)?;
                let mut flags = if down {
                    KEYBD_EVENT_FLAGS::default()
                } else {
                    KEYEVENTF_KEYUP
                };
                if matches!(vk.0, 0x21..=0x28 | 0x2d | 0x2e | 0x5b | 0x5d | 0x6f | 0x90) {
                    flags |= KEYEVENTF_EXTENDEDKEY;
                }
                vec![keyboard(vk, 0, flags)]
            }
            Input::Text(c) => {
                let mut units = [0; 2];
                c.encode_utf16(&mut units)
                    .iter()
                    .flat_map(|u| {
                        [
                            keyboard(VIRTUAL_KEY(0), *u, KEYEVENTF_UNICODE),
                            keyboard(VIRTUAL_KEY(0), *u, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                        ]
                    })
                    .collect()
            }
        };
        if unsafe { SendInput(&events, struct_size::<INPUT>().cast_signed()) } as usize
            != events.len()
        {
            return Err(InputError::Rejected { input: *input });
        }
        Ok(())
    }
    fn user_idle(&self) -> Duration {
        if (1..=254).any(|key| unsafe { GetAsyncKeyState(key) } < 0) {
            return Duration::ZERO;
        }
        Duration::from_millis(
            unsafe { GetTickCount64() }.saturating_sub(LAST_INPUT.load(Ordering::SeqCst)),
        )
    }
    fn user_activity(&self) -> u64 {
        ACTIVITY.load(Ordering::SeqCst)
    }
    fn foreground(&self) -> Option<WindowId> {
        let window = unsafe { GetForegroundWindow() };
        (!window.is_invalid()).then(|| window_id(window))
    }
    fn window_rect(&self, id: WindowId) -> Result<Rect, WindowError> {
        let _dpi = Dpi::enter();
        let mut bounds = RECT::default();
        unsafe { GetWindowRect(hwnd(id), &raw mut bounds) }.map_err(|source| {
            WindowError::System {
                step: "read the window bounds",
                source,
            }
        })?;
        Ok(rect(bounds))
    }
    fn windows(&self) -> Result<Vec<WindowInfo>, WindowError> {
        let _dpi = Dpi::enter();
        let mut list = Vec::new();
        unsafe { EnumWindows(Some(window_callback), LPARAM((&raw mut list) as isize)) }.map_err(
            |source| WindowError::System {
                step: "list the windows",
                source,
            },
        )?;
        Ok(list)
    }
    fn change_window(&self, window: WindowId, change: WindowChange) -> Result<(), WindowError> {
        let _dpi = Dpi::enter();
        let id = window;
        let window = hwnd(id);
        if !unsafe { IsWindow(Some(window)) }.as_bool() {
            return Err(WindowError::Gone { window: id });
        }
        unsafe {
            match change {
                WindowChange::Focus => {
                    if IsIconic(window).as_bool() {
                        let _was_visible = ShowWindow(window, SW_RESTORE);
                    }
                    if !SetForegroundWindow(window).as_bool() {
                        return Err(WindowError::FocusRefused { window: id });
                    }
                }
                WindowChange::Close => PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0))
                    .map_err(|source| WindowError::System {
                    step: "ask the window to close",
                    source,
                })?,
                WindowChange::Maximize => {
                    let _was_visible = ShowWindow(window, SW_MAXIMIZE);
                }
                WindowChange::Place(r) => {
                    let _was_visible = ShowWindow(window, SW_RESTORE);
                    SetWindowPos(
                        window,
                        None,
                        r.x,
                        r.y,
                        r.width.cast_signed(),
                        r.height.cast_signed(),
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )
                    .map_err(|source| WindowError::System {
                        step: "place the window",
                        source,
                    })?;
                }
            }
        }
        Ok(())
    }
    fn launch(
        &self,
        command: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> Result<u32, LaunchError> {
        let mut process = std::process::Command::new(command);
        process
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let mut child = process.spawn().map_err(|source| LaunchError::Spawn {
            command: command.to_owned(),
            source,
        })?;
        let pid = child.id();
        std::thread::spawn(move || {
            if let Err(error) = child.wait() {
                tracing::debug!("reap launched process {pid}: {error}");
            }
        });
        Ok(pid)
    }
    fn processes(&self) -> Result<Vec<(u32, u32)>, ProcessesError> {
        unsafe {
            let snapshot =
                CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(ProcessesError::System)?;
            let mut entry = PROCESSENTRY32W {
                dwSize: struct_size::<PROCESSENTRY32W>(),
                ..Default::default()
            };
            let mut list = Vec::new();
            let mut next = Process32FirstW(snapshot, &raw mut entry);
            while next.is_ok() {
                list.push((entry.th32ProcessID, entry.th32ParentProcessID));
                next = Process32NextW(snapshot, &raw mut entry);
            }
            release("the process snapshot", CloseHandle(snapshot));
            Ok(list)
        }
    }
    fn clipboard(&self) -> Result<Option<String>, ClipboardError> {
        let _clipboard = Clipboard::open()?;
        unsafe {
            if IsClipboardFormatAvailable(13).is_err() {
                return Ok(None);
            }
            let handle = GetClipboardData(13).map_err(|source| ClipboardError::System {
                step: "read the clipboard text",
                source,
            })?;
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global).cast::<u16>();
            if ptr.is_null() {
                return Err(ClipboardError::Lock);
            }
            let length = GlobalSize(global) / 2;
            let slice = std::slice::from_raw_parts(ptr, length);
            let end = slice.iter().position(|u| *u == 0).unwrap_or(length);
            let text = String::from_utf16_lossy(&slice[..end]);
            release("the clipboard text", GlobalUnlock(global));
            Ok(Some(text))
        }
    }
    fn set_clipboard(&self, text: &str) -> Result<(), ClipboardError> {
        if text.len() > 1024 * 1024 || text.contains('\0') {
            return Err(ClipboardError::Text { bytes: text.len() });
        }
        let _clipboard = Clipboard::open()?;
        let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2).map_err(|source| {
                ClipboardError::System {
                    step: "allocate the clipboard text",
                    source,
                }
            })?;
            let ptr = GlobalLock(memory).cast::<u16>();
            if ptr.is_null() {
                release(
                    "the new clipboard text",
                    GlobalFree(Some(memory)).map(|_| ()),
                );
                return Err(ClipboardError::Lock);
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
            release("the new clipboard text", GlobalUnlock(memory));
            if let Err(error) = EmptyClipboard()
                .and_then(|()| SetClipboardData(13, Some(HANDLE(memory.0))).map(|_| ()))
            {
                release(
                    "the new clipboard text",
                    GlobalFree(Some(memory)).map(|_| ()),
                );
                return Err(ClipboardError::System {
                    step: "replace the clipboard text",
                    source: error,
                });
            }
            Ok(())
        }
    }
}
fn virtual_key(key: Key) -> Result<VIRTUAL_KEY, InputError> {
    use Named::{
        Alt, Backspace, CapsLock, Ctrl, Delete, Down, End, Enter, Escape, F, Home, Insert, Left,
        Menu, NumLock, PageDown, PageUp, Pause, PrintScreen, Right, ScrollLock, Shift, Space,
        Super, Tab, Up,
    };
    Ok(match key {
        Key::Char(c) => {
            if !c.is_ascii() {
                return Err(InputError::NotAKey { key: c });
            }
            let scan = unsafe { VkKeyScanW(c as u16) };
            if scan == -1 {
                return Err(InputError::Unavailable { key: c });
            }
            VIRTUAL_KEY(scan.cast_unsigned() & 0xff)
        }
        Key::Named(key) => match key {
            Ctrl => VK_CONTROL,
            Shift => VK_SHIFT,
            Alt => VK_MENU,
            Super => VK_LWIN,
            Enter => VK_RETURN,
            Tab => VK_TAB,
            Escape => VK_ESCAPE,
            Backspace => VK_BACK,
            Delete => VK_DELETE,
            Insert => VK_INSERT,
            Home => VK_HOME,
            End => VK_END,
            PageUp => VK_PRIOR,
            PageDown => VK_NEXT,
            Up => VK_UP,
            Down => VK_DOWN,
            Left => VK_LEFT,
            Right => VK_RIGHT,
            Space => VK_SPACE,
            CapsLock => VK_CAPITAL,
            NumLock => VK_NUMLOCK,
            ScrollLock => VK_SCROLL,
            PrintScreen => VK_SNAPSHOT,
            Pause => VK_PAUSE,
            Menu => VK_APPS,
            F(n) => VIRTUAL_KEY(VK_F1.0 + u16::from(n) - 1),
        },
    })
}
