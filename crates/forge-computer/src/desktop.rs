//! What computer use needs from a machine, whatever its windowing system:
//! the agent's display, captures of it, input, its windows, launching, and
//! the clipboard. Every call blocks; the server runs them off the async
//! threads.

use crate::frame::Frame;
use crate::steps::Key;
use anyhow::Result;
use std::time::Duration;

/// A window, as the backend names it.
pub type WindowId = u64;

/// A rectangle in desktop pixels: the space every display shares, where
/// the primary display's top left is (0, 0).
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

/// A point in desktop pixels.
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

/// One input event, sent as the machine's own input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    MoveTo(Point),
    Button {
        button: Button,
        down: bool,
    },
    /// Wheel notches: positive `dy` scrolls down, positive `dx` right.
    Wheel {
        dx: i32,
        dy: i32,
    },
    Key {
        key: Key,
        down: bool,
    },
    /// One character typed as text, whatever the keyboard layout.
    Text(char),
}

/// A visible top-level window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: WindowId,
    pub title: String,
    /// The program's file name, such as `notepad.exe`.
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
    /// Restores it if minimized or maximized, then moves it here.
    Place(Rect),
}

pub trait Desktop: Send + Sync {
    /// The display agents use. Refused, saying what to install, when this
    /// machine has none.
    fn agent_display(&self) -> Result<Rect>;
    /// The user's main display, where a handoff shows a window.
    fn user_display(&self) -> Result<Rect>;
    /// The pixels of `area`.
    fn capture(&self, area: Rect) -> Result<Frame>;
    fn cursor(&self) -> Result<Point>;
    fn send(&self, input: &Input) -> Result<()>;
    /// How long since the user's own last input, leaving out what [`send`]
    /// sent.
    ///
    /// [`send`]: Desktop::send
    fn user_idle(&self) -> Duration;
    /// Changes on physical user input, never on our injected events.
    fn user_activity(&self) -> u64;
    /// The window with the keyboard focus.
    fn foreground(&self) -> Option<WindowId>;
    /// Every visible top-level window, frontmost first.
    fn windows(&self) -> Result<Vec<WindowInfo>>;
    fn window_rect(&self, id: WindowId) -> Result<Rect> {
        self.windows()?
            .into_iter()
            .find(|window| window.id == id)
            .map(|window| window.rect)
            .ok_or_else(|| anyhow::anyhow!("window no longer exists"))
    }
    fn change_window(&self, window: WindowId, change: WindowChange) -> Result<()>;
    /// Starts a program; its process id.
    fn launch(&self, command: &str, args: &[String], cwd: Option<&str>) -> Result<u32>;
    /// Every process as (id, parent id).
    fn processes(&self) -> Result<Vec<(u32, u32)>>;
    fn clipboard(&self) -> Result<Option<String>>;
    fn set_clipboard(&self, text: &str) -> Result<()>;
}
