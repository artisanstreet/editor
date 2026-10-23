//! Win32 calls stay here. Forge remains an ordinary interactive process.
use crate::{
    desktop::*,
    frame::Frame,
    steps::{Key, Named},
};
use anyhow::{Context, Result, bail, ensure};
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::{
            DataExchange::*, Diagnostics::ToolHelp::*, Memory::*,
            SystemInformation::GetTickCount64, Threading::*,
        },
        UI::{HiDpi::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    },
    core::{PCWSTR, PWSTR},
};

static ACTIVITY: AtomicU64 = AtomicU64::new(0);
static LAST_INPUT: AtomicU64 = AtomicU64::new(0);
static HOOKS: OnceLock<std::result::Result<(), String>> = OnceLock::new();
const MARKER: usize = 0x41525443;
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
fn hooks() -> Result<()> {
    let result = HOOKS.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        if let Err(error) = std::thread::Builder::new()
            .name("computer-user-input".into())
            .spawn(move || unsafe {
                physical_input();
                let mouse = match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), None, 0) {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let key = match SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), None, 0) {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = UnhookWindowsHookEx(mouse);
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };
                let _ = tx.send(Ok(()));
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).0 > 0 {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                let _ = UnhookWindowsHookEx(key);
                let _ = UnhookWindowsHookEx(mouse);
            })
        {
            return Err(error.to_string());
        }
        rx.recv_timeout(Duration::from_secs(3))
            .unwrap_or_else(|e| Err(e.to_string()))
    });
    result.clone().map_err(anyhow::Error::msg)
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
        width: (r.right - r.left).max(0) as u32,
        height: (r.bottom - r.top).max(0) as u32,
    }
}
fn hwnd(id: WindowId) -> HWND {
    HWND(id as usize as *mut _)
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
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
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
fn monitors() -> Result<Vec<Monitor>> {
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
    .ok()?;
    Ok(list)
}
pub struct WindowsDesktop {
    display: String,
}
impl WindowsDesktop {
    pub fn new() -> Result<Self> {
        let display=std::env::var("ARTISAN_COMPUTER_DISPLAY").context("set ARTISAN_COMPUTER_DISPLAY to the virtual monitor device name, such as \\\\.\\DISPLAY3, before starting Forge")?;
        let desktop = Self { display };
        desktop.agent_display()?;
        hooks()?;
        Ok(desktop)
    }
}
unsafe extern "system" fn window_callback(window: HWND, data: LPARAM) -> windows::core::BOOL {
    if !unsafe { IsWindowVisible(window) }.as_bool() {
        return true.into();
    }
    let mut bounds = RECT::default();
    if unsafe { GetWindowRect(window, &mut bounds) }.is_err() {
        return true.into();
    }
    let mut title =
        vec![0; (unsafe { GetWindowTextLengthW(window) }.max(0) as usize + 1).min(32768)];
    let length = unsafe { GetWindowTextW(window, &mut title) }.max(0) as usize;
    let mut pid = 0;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut pid));
    }
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .ok()
        .map(|handle| {
            let mut path = vec![0; 32768];
            let mut length = path.len() as u32;
            let result = unsafe {
                QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_WIN32,
                    PWSTR(path.as_mut_ptr()),
                    &mut length,
                )
            };
            let _ = unsafe { CloseHandle(handle) };
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
                let _ = DeleteObject(self.bitmap.into());
            }
            if !self.memory.is_invalid() {
                let _ = DeleteDC(self.memory);
            }
            ReleaseDC(None, self.screen);
        }
    }
}
struct Clipboard(HWND);
impl Clipboard {
    fn open() -> Result<Self> {
        // A real owner is necessary for SetClipboardData after EmptyClipboard.
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
        }?;
        for _ in 0..10 {
            if unsafe { OpenClipboard(Some(window)) }.is_ok() {
                return Ok(Self(window));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = unsafe { DestroyWindow(window) };
        bail!("clipboard is busy")
    }
}
impl Drop for Clipboard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
            let _ = DestroyWindow(self.0);
        }
    }
}

impl Desktop for WindowsDesktop {
    fn agent_display(&self) -> Result<Rect> {
        let display = monitors()?
            .into_iter()
            .find(|m| m.name.eq_ignore_ascii_case(&self.display))
            .context("configured agent display is disconnected")?;
        ensure!(
            !display.primary,
            "agent display must not be the primary monitor"
        );
        ensure!(
            display.rect.width > 0 && display.rect.height > 0,
            "agent display has no pixels"
        );
        Ok(display.rect)
    }
    fn user_display(&self) -> Result<Rect> {
        Ok(monitors()?
            .into_iter()
            .find(|m| m.primary)
            .context("no primary monitor")?
            .rect)
    }
    fn capture(&self, area: Rect) -> Result<Frame> {
        let _dpi = Dpi::enter();
        ensure!(
            area.width > 0
                && area.height > 0
                && u64::from(area.width) * u64::from(area.height) <= 64_000_000,
            "invalid capture size"
        );
        unsafe {
            let screen = GetDC(None);
            ensure!(!screen.is_invalid(), "GetDC failed");
            let mut capture = Capture {
                screen,
                memory: HDC::default(),
                bitmap: HBITMAP::default(),
                previous: HGDIOBJ::default(),
            };
            capture.memory = CreateCompatibleDC(Some(screen));
            ensure!(!capture.memory.is_invalid(), "CreateCompatibleDC failed");
            let mut info = BITMAPINFO::default();
            info.bmiHeader = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: area.width as i32,
                biHeight: -(area.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let mut pixels = std::ptr::null_mut();
            capture.bitmap =
                CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut pixels, None, 0)?;
            capture.previous = SelectObject(capture.memory, capture.bitmap.into());
            ensure!(!capture.previous.is_invalid(), "SelectObject failed");
            BitBlt(
                capture.memory,
                0,
                0,
                area.width as i32,
                area.height as i32,
                Some(screen),
                area.x,
                area.y,
                SRCCOPY | CAPTUREBLT,
            )?;
            GdiFlush().ok()?;
            let bgra = std::slice::from_raw_parts(
                pixels.cast::<u8>(),
                area.width as usize * area.height as usize * 4,
            );
            let mut rgb = Vec::with_capacity(area.width as usize * area.height as usize * 3);
            for pixel in bgra.chunks_exact(4) {
                rgb.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
            }
            Ok(Frame {
                width: area.width,
                height: area.height,
                rgb,
            })
        }
    }
    fn cursor(&self) -> Result<Point> {
        let _dpi = Dpi::enter();
        let mut p = POINT::default();
        unsafe { GetCursorPos(&mut p) }?;
        Ok(Point { x: p.x, y: p.y })
    }
    fn send(&self, input: &Input) -> Result<()> {
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
                ensure!(width > 1 && height > 1, "invalid virtual desktop bounds");
                vec![mouse(
                    MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                    0,
                    ((i64::from(p.x - x) * 65535) / i64::from(width - 1)) as i32,
                    ((i64::from(p.y - y) * 65535) / i64::from(height - 1)) as i32,
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
                    events.push(mouse(MOUSEEVENTF_WHEEL, (-dy * 120) as u32, 0, 0));
                }
                if dx != 0 {
                    events.push(mouse(MOUSEEVENTF_HWHEEL, (dx * 120) as u32, 0, 0));
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
        ensure!(
            unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) } as usize
                == events.len(),
            "Windows rejected input; use handoff for elevated windows or UAC"
        );
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
    fn window_rect(&self, id: WindowId) -> Result<Rect> {
        let _dpi = Dpi::enter();
        let mut bounds = RECT::default();
        unsafe { GetWindowRect(hwnd(id), &mut bounds) }?;
        Ok(rect(bounds))
    }
    fn windows(&self) -> Result<Vec<WindowInfo>> {
        let _dpi = Dpi::enter();
        let mut list = Vec::new();
        unsafe { EnumWindows(Some(window_callback), LPARAM((&raw mut list) as isize)) }?;
        Ok(list)
    }
    fn change_window(&self, window: WindowId, change: WindowChange) -> Result<()> {
        let _dpi = Dpi::enter();
        let window = hwnd(window);
        ensure!(
            unsafe { IsWindow(Some(window)) }.as_bool(),
            "window no longer exists"
        );
        unsafe {
            match change {
                WindowChange::Focus => {
                    if IsIconic(window).as_bool() {
                        let _ = ShowWindow(window, SW_RESTORE);
                    }
                    ensure!(
                        SetForegroundWindow(window).as_bool(),
                        "Windows refused focus; use handoff"
                    );
                }
                WindowChange::Close => PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0))?,
                WindowChange::Maximize => {
                    let _ = ShowWindow(window, SW_MAXIMIZE);
                }
                WindowChange::Place(r) => {
                    let _ = ShowWindow(window, SW_RESTORE);
                    SetWindowPos(
                        window,
                        None,
                        r.x,
                        r.y,
                        r.width as i32,
                        r.height as i32,
                        SWP_NOACTIVATE | SWP_NOZORDER,
                    )?;
                }
            }
        }
        Ok(())
    }
    fn launch(&self, command: &str, args: &[String], cwd: Option<&str>) -> Result<u32> {
        let mut process = std::process::Command::new(command);
        process
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let mut child = process.spawn().context("launch program")?;
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(pid)
    }
    fn processes(&self) -> Result<Vec<(u32, u32)>> {
        unsafe {
            let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)?;
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            let mut list = Vec::new();
            let mut next = Process32FirstW(snapshot, &mut entry);
            while next.is_ok() {
                list.push((entry.th32ProcessID, entry.th32ParentProcessID));
                next = Process32NextW(snapshot, &mut entry);
            }
            let _ = CloseHandle(snapshot);
            Ok(list)
        }
    }
    fn clipboard(&self) -> Result<Option<String>> {
        let _clipboard = Clipboard::open()?;
        unsafe {
            if IsClipboardFormatAvailable(13).is_err() {
                return Ok(None);
            }
            let handle = GetClipboardData(13)?;
            let global = HGLOBAL(handle.0);
            let ptr = GlobalLock(global).cast::<u16>();
            ensure!(!ptr.is_null(), "clipboard memory could not be locked");
            let length = GlobalSize(global) / 2;
            let slice = std::slice::from_raw_parts(ptr, length);
            let end = slice.iter().position(|u| *u == 0).unwrap_or(length);
            let text = String::from_utf16_lossy(&slice[..end]);
            let _ = GlobalUnlock(global);
            Ok(Some(text))
        }
    }
    fn set_clipboard(&self, text: &str) -> Result<()> {
        ensure!(
            text.len() <= 1024 * 1024 && !text.contains('\0'),
            "clipboard text must be at most 1 MiB and contain no NUL"
        );
        let _clipboard = Clipboard::open()?;
        let units: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, units.len() * 2)?;
            let ptr = GlobalLock(memory).cast::<u16>();
            if ptr.is_null() {
                let _ = GlobalFree(Some(memory));
                bail!("clipboard allocation failed");
            }
            std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
            let _ = GlobalUnlock(memory);
            if let Err(error) = EmptyClipboard()
                .and_then(|()| SetClipboardData(13, Some(HANDLE(memory.0))).map(|_| ()))
            {
                let _ = GlobalFree(Some(memory));
                return Err(error.into());
            }
            Ok(())
        }
    }
}
fn virtual_key(key: Key) -> Result<VIRTUAL_KEY> {
    use Named::*;
    Ok(match key {
        Key::Char(c) => {
            ensure!(c.is_ascii(), "use type for Unicode text");
            let scan = unsafe { VkKeyScanW(c as u16) };
            ensure!(scan != -1, "key is unavailable in this keyboard layout");
            VIRTUAL_KEY((scan as u16) & 0xff)
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
