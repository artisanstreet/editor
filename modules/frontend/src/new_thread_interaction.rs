//! Pure pre-creation interaction policies for a new thread.
//!
//! This is the dependency-free portion of
//! `modules/frontend/src/lib/root/new-thread-draft.ts`. It preserves the
//! stable draft-key mapping and the browser-style unmodified primary-click
//! guard while deliberately leaving Effect services and durable
//! submit/reset/retry orchestration to a later boundary.

/// Returns the composer slot used by a new-thread surface.
///
/// An absent workspace uses the shared root draft slot. A present workspace
/// is inserted verbatim, including an empty string or any special characters;
/// this policy performs no trimming, escaping, case conversion, or other
/// normalization.
#[must_use]
pub fn new_thread_draft_key(workspace_id: Option<&str>) -> String {
    match workspace_id {
        None => "draft:new-thread".to_owned(),
        Some(workspace_id) => format!("draft:{workspace_id}"),
    }
}

/// The browser activation fields needed by a new-thread link guard.
///
/// `button` follows the DOM `MouseEvent.button` numeric shape. The policy
/// intentionally checks equality with zero rather than treating only the
/// usual nonnegative button values as valid, so negative values fail closed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(clippy::struct_excessive_bools)]
pub struct NewThreadActivation {
    /// Whether the Alt modifier was held.
    pub alt_key: bool,
    /// The DOM button number; zero is the primary button.
    pub button: i32,
    /// Whether the Ctrl modifier was held.
    pub ctrl_key: bool,
    /// Whether the Meta modifier was held.
    pub meta_key: bool,
    /// Whether the Shift modifier was held.
    pub shift_key: bool,
}

/// Returns whether an activation is exactly an unmodified primary click.
///
/// Modified activations remain available to the browser's normal link
/// handling. Only button zero with all four modifier flags false is consumed
/// by the new-thread surface.
#[must_use]
pub const fn is_unmodified_primary_activation(event: NewThreadActivation) -> bool {
    event.button == 0 && !event.alt_key && !event.ctrl_key && !event.meta_key && !event.shift_key
}
