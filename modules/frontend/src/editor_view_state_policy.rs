//! Pure save/restore policy for an editor's selection and vertical scroll.
//!
//! The browser adapter stores the main `CodeMirror` selection's `anchor` and
//! `head` together with `scrollDOM.scrollTop`. This module owns that value
//! without owning a document, editor surface, or renderer. Restoration
//! returns a new value for a caller to apply; it never mutates the supplied
//! document text.
//!
//! Positions are UTF-8 byte offsets. A position inside a scalar is rounded
//! down to the scalar's starting boundary, and a position beyond the document
//! is rounded down to `document.len()`. Rounding down is deterministic and
//! keeps both endpoints valid for Rust string slicing.
//!
//! `CodeMirror`'s `scrollDOM` is an ordinary downward-scrolling element. Its
//! browser boundary accepts finite non-negative CSS-pixel positions; a
//! negative requested position is clamped to zero, and CSSOM normalizes
//! `NaN`, positive infinity, and negative infinity to zero. This policy
//! rejects those invalid values when saving because they cannot be a valid
//! snapshot, and applies the same zero normalization when restoring an
//! untrusted value. Every finite non-negative value, including one larger
//! than the current scrollable extent, is retained. The platform adapter
//! performs the layout-dependent maximum-scroll clamp; this dependency-free
//! leaf intentionally has no viewport geometry and therefore does not invent
//! a maximum.

/// An owned snapshot of one editor surface's main selection and scroll.
///
/// `anchor` and `head` are UTF-8 byte offsets. Their validity is relative to
/// the document being restored and is checked by [`restore_view_state`]. A
/// state produced by [`save_view_state`] has a finite, non-negative
/// `scroll_top`; the public constructor also permits raw values so an adapter
/// can represent an opaque legacy payload before restoration normalizes it.
#[must_use = "an editor view state should be applied or retained"]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorViewState {
    /// The selection's fixed endpoint, in UTF-8 bytes.
    pub anchor: usize,
    /// The selection's moving endpoint, in UTF-8 bytes.
    pub head: usize,
    /// The vertical scroll position in CSS pixels.
    pub scroll_top: f64,
}

impl EditorViewState {
    /// Constructs a snapshot while preserving every supplied field exactly.
    ///
    /// This is a raw value constructor for adapter boundaries. Use
    /// [`save_view_state`] for a snapshot from a current surface, which
    /// accepts only the browser getter's finite, non-negative scroll domain.
    pub const fn new(anchor: usize, head: usize, scroll_top: f64) -> Self {
        Self {
            anchor,
            head,
            scroll_top,
        }
    }
}

/// The representation received from an adapter before restoration.
///
/// `Missing` models the service's absent optional state. `Opaque` models a
/// present payload belonging to another adapter or an incompatible version.
/// Both are deliberate no-op inputs; neither is decoded with a panic-prone
/// cast. `Owned` carries this policy's typed snapshot by value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EditorViewStateInput {
    /// No previous snapshot exists for the document.
    Missing,
    /// A snapshot exists but is not compatible with this policy.
    Opaque,
    /// A snapshot owned by this policy.
    Owned(EditorViewState),
}

impl EditorViewStateInput {
    /// Creates the explicit missing-state input.
    #[must_use]
    pub const fn missing() -> Self {
        Self::Missing
    }

    /// Creates the explicit opaque/incompatible-state input.
    #[must_use]
    pub const fn opaque() -> Self {
        Self::Opaque
    }

    /// Wraps an owned snapshot for restoration.
    #[must_use]
    pub const fn owned(state: EditorViewState) -> Self {
        Self::Owned(state)
    }
}

impl From<Option<EditorViewState>> for EditorViewStateInput {
    fn from(state: Option<EditorViewState>) -> Self {
        state.map_or(Self::Missing, Self::Owned)
    }
}

/// The explicit outcome of trying to restore a view snapshot.
#[must_use = "a restore outcome should be applied or intentionally ignored"]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RestoreViewStateResult {
    /// No state was available or the supplied state was incompatible.
    NoOp,
    /// A compatible state was normalized for `document` and can be applied.
    Applied(EditorViewState),
}

impl RestoreViewStateResult {
    /// Returns whether restoration intentionally performed no operation.
    #[must_use]
    pub const fn is_no_op(self) -> bool {
        matches!(self, Self::NoOp)
    }

    /// Returns the normalized state when restoration was applied.
    #[must_use]
    pub const fn state(self) -> Option<EditorViewState> {
        match self {
            Self::NoOp => None,
            Self::Applied(state) => Some(state),
        }
    }
}

/// Saves the exact selection and a browser-valid vertical scroll position.
///
/// The selection endpoints are not clamped here: a current editor selection
/// is already relative to its current document, and preserving it exactly is
/// the save boundary's purpose. A negative or non-finite scroll value returns
/// `None` because it is outside the ordinary `CodeMirror` `scrollDOM` getter
/// domain. Finite non-negative values, including oversized values, are kept
/// unchanged.
#[must_use]
pub fn save_view_state(anchor: usize, head: usize, scroll_top: f64) -> Option<EditorViewState> {
    if scroll_top.is_finite() && scroll_top >= 0.0 {
        Some(EditorViewState::new(anchor, head, scroll_top))
    } else {
        None
    }
}

/// Restores a compatible snapshot against `document` without changing text.
///
/// Anchor and head are clamped independently to valid UTF-8 byte boundaries,
/// so a reversed selection remains reversed and a caret remains a caret.
/// Scroll is restored independently of selection: invalid negative or
/// non-finite input becomes `0.0`, while every finite non-negative value is
/// retained for the eventual renderer to clamp to its layout-dependent
/// maximum. [`EditorViewStateInput::Missing`] and
/// [`EditorViewStateInput::Opaque`] return [`RestoreViewStateResult::NoOp`].
pub fn restore_view_state(document: &str, input: EditorViewStateInput) -> RestoreViewStateResult {
    let EditorViewStateInput::Owned(state) = input else {
        return RestoreViewStateResult::NoOp;
    };

    RestoreViewStateResult::Applied(EditorViewState::new(
        clamp_position(document, state.anchor),
        clamp_position(document, state.head),
        normalize_scroll_top(state.scroll_top),
    ))
}

/// Clamps a UTF-8 byte position to a scalar boundary in `document`.
///
/// Positions beyond the document length become the length. Positions inside a
/// multibyte scalar become that scalar's starting byte, never a split point.
#[must_use]
pub fn clamp_position(document: &str, position: usize) -> usize {
    let mut position = position.min(document.len());
    while !document.is_char_boundary(position) {
        position -= 1;
    }
    position
}

/// Applies the browser boundary's deterministic scroll normalization.
///
/// The CSSOM setter maps every non-finite value to zero, and an ordinary
/// downward `scrollDOM` maps negative requests to zero. Finite non-negative
/// values are returned unchanged, including values larger than the current
/// scrollable extent because this policy has no layout information.
#[must_use]
pub fn normalize_scroll_top(scroll_top: f64) -> f64 {
    if scroll_top.is_finite() && scroll_top >= 0.0 {
        scroll_top
    } else {
        0.0
    }
}
