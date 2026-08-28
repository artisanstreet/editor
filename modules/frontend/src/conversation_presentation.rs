//! Conversation work-session disclosure presentation.
//!
//! Pure port of `modules/frontend/src/lib/conversation/presentation.ts` and
//! the presentation boundary used by
//! `modules/frontend/src/routes/components/conversation-work-session.svelte`.
//!
//! The Svelte component keeps several related observations separate:
//! details may be defined without currently rendering visible children, live
//! work may make the header visible without making it controllable, and the
//! caller's open state is independent of whether the detail tree is mounted.
//! These helpers preserve those distinctions without performing rendering or
//! DOM observation themselves.

/// Inputs used to seed a work session's initial disclosure state.
///
/// The TypeScript contract accepts `has_details` and `unsuccessful` for the
/// call-site's complete session snapshot, but neither input affects the
/// initial state. Only currently working sessions start open.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkSessionInitialOpenInput {
    /// Whether the session has details at the time it is projected.
    ///
    /// This is intentionally not consulted by
    /// [`work_session_initially_open`].
    pub has_details: bool,
    /// Whether the session has an unsuccessful terminal outcome.
    ///
    /// This is intentionally not consulted by
    /// [`work_session_initially_open`].
    pub unsuccessful: bool,
    /// Whether the session is still live.
    pub working: bool,
}

/// Returns the initial open state for a work-session disclosure.
///
/// Settled history starts closed regardless of details or failure state; only
/// live work starts open. The unused-looking fields are part of the explicit
/// input contract and deliberately do not acquire inferred behavior here.
#[must_use]
pub const fn work_session_initially_open(input: WorkSessionInitialOpenInput) -> bool {
    input.working
}

/// Inputs used to derive the rendered disclosure presentation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkSessionDisclosureInput {
    /// Whether the details snippet is defined and can be mounted.
    pub details_defined: bool,
    /// Whether the observed detail tree currently has visible children.
    ///
    /// This is the controllability signal used by the Svelte header. It is
    /// distinct from [`Self::details_defined`], which describes the snippet's
    /// availability rather than its current rendered contents.
    pub has_visible_details: bool,
    /// The caller-owned open state of the disclosure.
    pub open: bool,
    /// Whether the work session is still live.
    pub working: bool,
}

/// Derived attributes and lifecycle state for a work-session disclosure.
///
/// `data_open` and `data_state` use `None` to represent the TypeScript
/// `undefined` values. This matters to Svelte attribute spreading: absent
/// attributes are different from emitting a false/closed value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkSessionDisclosureOutput {
    /// Whether the header can expose a collapse/expand control.
    pub can_collapse: bool,
    /// The `data-open` value when the session is visible, otherwise absent.
    pub data_open: Option<bool>,
    /// The exact `data-state` value (`"open"` or `"closed"`) when visible.
    pub data_state: Option<&'static str>,
    /// Whether the detail panel is hidden by the caller's open state.
    pub details_hidden: bool,
    /// Whether the defined detail tree remains mounted for this state.
    pub details_mounted: bool,
}

/// Derives the pure visual contract for a work-session disclosure.
///
/// Visible details make the header controllable. Live work makes the session
/// visible even before details are observed, but liveness alone never changes
/// the caller's open state. The detail tree stays mounted while working or
/// while open, provided its snippet is defined.
#[must_use]
pub const fn work_session_disclosure(
    input: WorkSessionDisclosureInput,
) -> WorkSessionDisclosureOutput {
    let can_collapse = input.has_visible_details;
    let visible = can_collapse || input.working;

    WorkSessionDisclosureOutput {
        can_collapse,
        data_open: if visible { Some(input.open) } else { None },
        data_state: if visible {
            Some(if input.open { "open" } else { "closed" })
        } else {
            None
        },
        details_hidden: !input.open,
        details_mounted: input.details_defined && (input.working || input.open),
    }
}
