//! Pure reader-attention decisions.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/browser/reader-attention.ts` for the decisions
//! that can be made from already-observed values. The host supplies focus,
//! visibility, and inspection state; this module only derives booleans from
//! them. Host access, DOM targets, and asynchronous behavior are outside this
//! boundary.

/// The document visibility states understood by the browser-facing boundary.
///
/// The browser currently reports `"visible"` and `"hidden"`, but the native
/// boundary must not discard a value introduced by a host or a future browser
/// state. [`Self::Unknown`] therefore keeps that raw value byte-for-byte.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum VisibilityState {
    /// The exact canonical `"visible"` state.
    Visible,
    /// The exact canonical `"hidden"` state.
    Hidden,
    /// Any other host value, preserved without casing or whitespace changes.
    Unknown(String),
}

impl VisibilityState {
    /// Parses a host visibility value without normalizing it.
    ///
    /// Only the exact lowercase literals `"visible"` and `"hidden"` are
    /// recognized. Every other value becomes [`Self::Unknown`] and remains
    /// available through [`Self::as_raw`].
    #[must_use]
    pub fn from_raw(raw: &str) -> Self {
        match raw {
            "visible" => Self::Visible,
            "hidden" => Self::Hidden,
            other => Self::Unknown(other.to_owned()),
        }
    }

    /// Whether this state is the exact canonical visible state.
    #[must_use]
    pub const fn is_visible(&self) -> bool {
        matches!(self, Self::Visible)
    }

    /// Returns the exact raw host value represented by this state.
    #[must_use]
    pub fn as_raw(&self) -> &str {
        match self {
            Self::Visible => "visible",
            Self::Hidden => "hidden",
            Self::Unknown(raw) => raw,
        }
    }
}

impl From<&str> for VisibilityState {
    fn from(raw: &str) -> Self {
        Self::from_raw(raw)
    }
}

impl From<String> for VisibilityState {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "visible" => Self::Visible,
            "hidden" => Self::Hidden,
            _ => Self::Unknown(raw),
        }
    }
}

/// Whether the reader is watching the document.
///
/// A reader is watching exactly when the document has focus and its raw
/// visibility value was the exact lowercase `"visible"` literal. Hidden and
/// unknown states are not watching, including values that differ only by
/// casing or surrounding whitespace.
#[must_use]
pub fn reader_is_watching(has_focus: bool, visibility_state: &VisibilityState) -> bool {
    has_focus && visibility_state.is_visible()
}

/// Convenience form of [`reader_is_watching`] for a raw host value.
#[must_use]
pub fn reader_is_watching_raw(has_focus: bool, raw_visibility_state: &str) -> bool {
    has_focus && raw_visibility_state == "visible"
}

/// Whether a root conversation read may be acknowledged.
///
/// Root reads can be acknowledged only while the reader is watching and the
/// reader is not inspecting an agent. This is a pure composition decision; it
/// does not perform or represent acknowledgement mutation.
#[must_use]
pub const fn reader_can_acknowledge_root_conversation(
    watching: bool,
    inspecting_agent: bool,
) -> bool {
    watching && !inspecting_agent
}
