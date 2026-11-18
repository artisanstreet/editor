use crate::frame::Frame;
use crate::steps::Key;
use std::time::Duration;

pub type WindowId = u64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    #[must_use]
    pub fn contains(&self, point: Point) -> bool {
        point.x >= self.x
            && point.y >= self.y
            && i64::from(point.x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(point.y) < i64::from(self.y) + i64::from(self.height)
    }

    #[must_use]
    pub fn center(&self) -> Point {
        Point {
            x: self
                .x
                .saturating_add(i32::try_from(self.width / 2).unwrap_or(i32::MAX)),
            y: self
                .y
                .saturating_add(i32::try_from(self.height / 2).unwrap_or(i32::MAX)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    MoveTo(Point),
    Button { button: Button, down: bool },
    Wheel { dx: i32, dy: i32 },
    Key { key: Key, down: bool },
    Text(char),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    pub process: String,
    pub pid: u32,
    pub rect: Rect,
    pub minimized: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowChange {
    Focus,
    Close,
    Maximize,
    Place(Rect),
}

#[derive(Debug, thiserror::Error)]
pub enum DisplayError {
    #[cfg(windows)]
    #[error("list the monitors")]
    Monitors(#[source] windows::core::Error),
    #[error("the agent display {display} is disconnected")]
    Disconnected { display: String },
    #[error("the agent display {display} must not be the primary monitor")]
    Primary { display: String },
    #[error("the agent display {display} has no pixels")]
    Empty { display: String },
    #[error("there is no primary monitor")]
    NoPrimary,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("capture {area:?}: the size is outside what a capture takes")]
    Size { area: Rect },
    #[error("capture {area:?}: {step} failed")]
    Failed { area: Rect, step: &'static str },
    #[cfg(windows)]
    #[error("capture {area:?}: {step}")]
    System {
        area: Rect,
        step: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("capture {area:?}: keep the pixels")]
    Frame {
        area: Rect,
        #[source]
        source: crate::frame::FrameError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum CursorError {
    #[cfg(windows)]
    #[error("read the cursor position")]
    System(#[source] windows::core::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("the system rejected {input:?}; use handoff for elevated windows or UAC")]
    Rejected { input: Input },
    #[error("the virtual desktop is {width}x{height}, too small to move the cursor in")]
    VirtualDesktop { width: i32, height: i32 },
    #[error("{key:?} is not a key; use type for Unicode text")]
    NotAKey { key: char },
    #[error("{key:?} is unavailable in this keyboard layout")]
    Unavailable { key: char },
}

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[cfg(windows)]
    #[error("{step}")]
    System {
        step: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("window {window} no longer exists")]
    Gone { window: WindowId },
    #[error("the system refused to focus window {window}; use handoff")]
    FocusRefused { window: WindowId },
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("launch {command}")]
    Spawn {
        command: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProcessesError {
    #[cfg(windows)]
    #[error("list the processes")]
    System(#[source] windows::core::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ClipboardError {
    #[cfg(windows)]
    #[error("{step}")]
    System {
        step: &'static str,
        #[source]
        source: windows::core::Error,
    },
    #[error("the clipboard is busy")]
    Busy,
    #[error("the clipboard memory could not be locked")]
    Lock,
    #[error("clipboard text must be at most 1 MiB and contain no NUL, not {bytes} bytes")]
    Text { bytes: usize },
}

pub trait Desktop: Send + Sync {
    fn agent_display(&self) -> Result<Rect, DisplayError>;
    fn user_display(&self) -> Result<Rect, DisplayError>;
    fn capture(&self, area: Rect) -> Result<Frame, CaptureError>;
    fn cursor(&self) -> Result<Point, CursorError>;
    fn send(&self, input: &Input) -> Result<(), InputError>;
    fn user_idle(&self) -> Duration;
    fn user_activity(&self) -> u64;
    fn foreground(&self) -> Option<WindowId>;
    fn windows(&self) -> Result<Vec<WindowInfo>, WindowError>;
    fn window_rect(&self, id: WindowId) -> Result<Rect, WindowError> {
        self.windows()?
            .into_iter()
            .find(|window| window.id == id)
            .map(|window| window.rect)
            .ok_or(WindowError::Gone { window: id })
    }
    fn change_window(&self, window: WindowId, change: WindowChange) -> Result<(), WindowError>;
    fn launch(&self, command: &str, args: &[String], cwd: Option<&str>)
    -> Result<u32, LaunchError>;
    fn processes(&self) -> Result<Vec<(u32, u32)>, ProcessesError>;
    fn clipboard(&self) -> Result<Option<String>, ClipboardError>;
    fn set_clipboard(&self, text: &str) -> Result<(), ClipboardError>;
}
