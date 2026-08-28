//! Pure thread-title selection policy.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/threads/title.ts`. The surrounding frontend
//! owns the defaults store and thread projection; this leaf only selects one
//! already-decoded title for presentation. It performs no protocol decoding,
//! persistence, rendering, or asynchronous work.
//!
//! The protocol currently recognizes `summary` and `latest_message`. The
//! local mode type also retains an unrecognized raw value so a newer protocol
//! mode can cross this boundary without being normalized or discarded. Every
//! mode other than `summary` deliberately falls back to the stored title.

#![allow(clippy::module_name_repetitions)]

use std::borrow::Borrow;

/// The reader's preference for naming a thread.
///
/// `Summary` and `LatestMessage` mirror the current protocol literals. An
/// unknown value represents a mode added by a newer protocol version and is
/// retained exactly so an adapter can inspect or re-emit it.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub enum ThreadTitleMode {
    /// Prefer a present harness-generated summary for an unlocked title.
    #[default]
    Summary,
    /// Always use the thread's stored title.
    LatestMessage,
    /// A future or otherwise unrecognized raw mode, preserved verbatim.
    Unknown(String),
}

impl ThreadTitleMode {
    /// The currently recognized modes in protocol order.
    pub const ALL: [Self; 2] = [Self::Summary, Self::LatestMessage];

    /// Parses one exact raw mode without trimming or case folding.
    ///
    /// Unknown and future values become [`Self::Unknown`] and retain every
    /// byte of the supplied UTF-8 string.
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "summary" => Self::Summary,
            "latest_message" => Self::LatestMessage,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Builds a mode from an owned raw value without cloning unknown input.
    ///
    /// Known literals are classified, while every other value is moved into
    /// [`Self::Unknown`] unchanged.
    #[must_use]
    pub fn from_owned(raw: String) -> Self {
        match raw.as_str() {
            "summary" => Self::Summary,
            "latest_message" => Self::LatestMessage,
            _ => Self::Unknown(raw),
        }
    }

    /// Returns the exact raw mode represented by this value.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Summary => "summary",
            Self::LatestMessage => "latest_message",
            Self::Unknown(raw) => raw,
        }
    }

    /// Returns the exact raw mode represented by this value.
    ///
    /// This string-oriented alias is useful to callers that do not need to
    /// distinguish a known mode from a retained future mode.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.as_raw()
    }

    /// Returns an owned raw mode, preserving unknown values exactly.
    #[must_use]
    pub fn into_raw(self) -> String {
        match self {
            Self::Summary => String::from("summary"),
            Self::LatestMessage => String::from("latest_message"),
            Self::Unknown(raw) => raw,
        }
    }

    /// Returns whether this is the exact `summary` mode.
    #[must_use]
    pub const fn is_summary(&self) -> bool {
        matches!(self, Self::Summary)
    }
}

impl From<&str> for ThreadTitleMode {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for ThreadTitleMode {
    fn from(raw: String) -> Self {
        Self::from_owned(raw)
    }
}

/// The title fields required by the pure display selector.
///
/// The fields borrow from the caller's already-decoded thread projection.
/// `summary_title` retains the protocol's presence distinction: `None` means
/// no generated summary was supplied, while `Some("")` is an explicitly
/// present empty summary and is therefore eligible for selection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadTitleInput<'a> {
    /// The harness-generated summary title, when the projection supplied one.
    pub summary_title: Option<&'a str>,
    /// The stored title, normally derived from the latest user message.
    pub title: &'a str,
    /// Whether a manual rename has locked the stored title.
    pub title_locked: bool,
}

impl<'a> ThreadTitleInput<'a> {
    /// Builds a borrowed selector input without copying either title.
    #[must_use]
    pub const fn new(summary_title: Option<&'a str>, title: &'a str, title_locked: bool) -> Self {
        Self {
            summary_title,
            title,
            title_locked,
        }
    }
}

/// Selects the title a thread surface should display.
///
/// Summary mode, an unlocked title, and a present summary select the summary;
/// every other combination selects the stored title. Presence is tested with
/// `Option`, not string content, so an explicitly empty summary wins when it
/// is eligible. The returned `&str` borrows from `input` and this function
/// never allocates.
///
/// The mode argument accepts either an owned [`ThreadTitleMode`] or a borrow
/// of one. An owned future mode can therefore be moved directly into this
/// function, while a mode retained by a caller can be passed by reference.
#[must_use]
pub fn thread_display_title(
    input: ThreadTitleInput<'_>,
    mode: impl Borrow<ThreadTitleMode>,
) -> &str {
    if input.title_locked {
        return input.title;
    }

    if !mode.borrow().is_summary() {
        return input.title;
    }

    input.summary_title.unwrap_or(input.title)
}
