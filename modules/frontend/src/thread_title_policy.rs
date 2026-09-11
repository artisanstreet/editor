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

/// The placeholder title native task creation writes before any user text
/// exists.
///
/// `native_transport_service` creates every new task with this exact title.
/// The reference's live refiner replaces it with the latest user text as soon
/// as one arrives; a display surface that has the message evidence and not yet
/// the refined listing applies the same replacement through
/// [`refined_thread_title`].
pub const UNNAMED_THREAD_TITLE: &str = "New task";

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

/// Applies the reference refiner's stored-title rule to evidence the caller
/// already holds.
///
/// The reference's live refiner derives the stored title from the latest user
/// text while the title is unlocked
/// (`modules/backend/src/threads/thread-metadata-refiner.ts`). While a native
/// thread still carries the creation placeholder and no refreshed listing has
/// arrived, the latest user message *is* that stored title for display
/// purposes. Any other stored title — a refined title, an import, or a future
/// manual rename — is returned untouched, so this never overrides real
/// metadata. Blank evidence is ignored rather than replacing the placeholder
/// with an empty label.
#[must_use]
pub fn refined_thread_title<'a>(
    stored_title: &'a str,
    latest_user_text: Option<&'a str>,
) -> &'a str {
    match latest_user_text {
        Some(latest) if stored_title == UNNAMED_THREAD_TITLE => {
            let trimmed = latest.trim();
            if trimmed.is_empty() {
                stored_title
            } else {
                trimmed
            }
        }
        _ => stored_title,
    }
}

#[cfg(test)]
mod tests {
    use super::{ThreadTitleInput, ThreadTitleMode, refined_thread_title, thread_display_title};

    #[test]
    fn summary_wins_for_an_unlocked_title() {
        let input = ThreadTitleInput::new(Some("Generated summary"), "Latest user message", false);

        assert_eq!(
            thread_display_title(input, ThreadTitleMode::Summary),
            "Generated summary"
        );
    }

    #[test]
    fn a_manual_lock_wins_over_the_summary() {
        let input = ThreadTitleInput::new(Some("Generated summary"), "My renamed thread", true);

        assert_eq!(
            thread_display_title(input, ThreadTitleMode::Summary),
            "My renamed thread"
        );
    }

    #[test]
    fn an_absent_summary_falls_back_to_the_stored_title() {
        let input = ThreadTitleInput::new(None, "Latest user message", false);

        assert_eq!(
            thread_display_title(input, ThreadTitleMode::Summary),
            "Latest user message"
        );
    }

    #[test]
    fn creation_placeholder_refines_to_the_latest_user_message() {
        assert_eq!(
            refined_thread_title("New task", Some("  Fix the header title  ")),
            "Fix the header title"
        );
        assert_eq!(
            refined_thread_title("New task", Some("")),
            "New task",
            "blank evidence must not replace the placeholder with nothing"
        );
        assert_eq!(refined_thread_title("New task", None), "New task");
    }

    #[test]
    fn real_stored_titles_are_never_overridden_by_message_evidence() {
        assert_eq!(
            refined_thread_title("Ship the port", Some("A later unrelated message")),
            "Ship the port"
        );
    }
}
