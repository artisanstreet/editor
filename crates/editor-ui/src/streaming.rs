//! How a streaming reply appears: this device's choice, read at render time.
//!
//! The choice lives in a main-thread cell rather than a GPUI global so that
//! the markdown renderer, which draws without an `App`, can read it too.

use std::cell::Cell;

use gpui::App;

/// The pace and shape of a reply's reveal while it streams.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum StreamingMode {
    /// Words are paced at a capped rate and swept in letter runs; lines
    /// show once their layout is final and blocks wait until they close.
    #[default]
    Smooth,
    /// Every complete word shows as soon as it arrives, each fading in
    /// whole; lines still wait for their final layout and blocks for
    /// their closing fence.
    Swift,
    /// Text draws the moment it arrives, the growing word included, and
    /// an unfinished block draws as its source until it closes.
    Instant,
}

impl StreamingMode {
    pub const ALL: [Self; 3] = [Self::Smooth, Self::Swift, Self::Instant];

    /// The value stored in `settings.json`.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Smooth => "smooth",
            Self::Swift => "swift",
            Self::Instant => "instant",
        }
    }

    /// The mode stored as `key`; anything else reads as the default.
    #[must_use]
    pub fn from_key(key: Option<&str>) -> Self {
        Self::ALL
            .into_iter()
            .find(|mode| Some(mode.key()) == key)
            .unwrap_or_default()
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Smooth => "Smooth",
            Self::Swift => "Swift",
            Self::Instant => "Instant",
        }
    }

    /// The mode this application uses.
    #[must_use]
    pub fn current() -> Self {
        CURRENT.with(Cell::get)
    }
}

thread_local! {
    pub(crate) static CURRENT: Cell<StreamingMode> = const { Cell::new(StreamingMode::Smooth) };
}

/// Installs `mode` and redraws every window when it changes.
pub fn set_mode(mode: StreamingMode, cx: &mut App) {
    if CURRENT.with(|current| current.replace(mode)) != mode {
        cx.refresh_windows();
    }
}
