//! Pure policy for identifying and activating a new-thread draft.
//!
//! This is the dependency-free Rust equivalent of
//! `modules/frontend/src/lib/root/new-thread-draft.ts`. It owns only the
//! stable draft-key derivation and the browser-neutral activation predicate.
//! Draft storage, thread creation, retry handling, navigation, and any other
//! lifecycle effects remain with the caller that integrates this boundary.

/// Returns the stable composer key for a new-thread surface.
///
/// An absent workspace uses the one root draft slot. A present workspace is
/// appended exactly as supplied, including an empty string, whitespace,
/// Unicode, or any other characters; this function performs no normalization.
#[must_use]
pub fn new_thread_draft_key(workspace_id: Option<&str>) -> String {
    match workspace_id {
        None => String::from("draft:new-thread"),
        Some(workspace_id) => {
            let mut key = String::with_capacity("draft:".len() + workspace_id.len());
            key.push_str("draft:");
            key.push_str(workspace_id);
            key
        }
    }
}

/// The activation details needed by the new-thread link policy.
///
/// This is deliberately a small, browser-neutral projection rather than an
/// event or UI-framework type. A later lifecycle integration can translate
/// its event into this input without making this pure policy own prevention,
/// navigation, persistence, or thread creation.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NewThreadActivation {
    /// Whether the Alt/Option modifier was held.
    pub alt_key: bool,
    /// The numeric button identifier; zero is the primary button.
    pub button: u8,
    /// Whether the Control modifier was held.
    pub ctrl_key: bool,
    /// Whether the Meta/Command modifier was held.
    pub meta_key: bool,
    /// Whether the Shift modifier was held.
    pub shift_key: bool,
}

impl NewThreadActivation {
    /// Creates an activation projection from the button and modifier state.
    #[allow(clippy::fn_params_excessive_bools)]
    #[must_use]
    pub const fn new(
        button: u8,
        alt_key: bool,
        ctrl_key: bool,
        meta_key: bool,
        shift_key: bool,
    ) -> Self {
        Self {
            alt_key,
            button,
            ctrl_key,
            meta_key,
            shift_key,
        }
    }
}

/// Returns whether the activation is an unmodified primary-button action.
///
/// Only button zero with every modifier released is accepted. Modified
/// activations remain available to the embedding surface for its normal link
/// behavior; this predicate performs no event handling of its own.
#[must_use]
pub const fn is_unmodified_primary_activation(input: NewThreadActivation) -> bool {
    input.button == 0 && !input.alt_key && !input.ctrl_key && !input.meta_key && !input.shift_key
}
