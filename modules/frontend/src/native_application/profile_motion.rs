//! Retained, interruptible profile-menu motion state: the reading/action/loading
//! swap tween and the remaining-value meter tooltip tween.
//!
//! Extracted verbatim from `native_application.rs` during the phase-1 module
//! split; visibility was widened to `pub(super)` for parent-owned state.

/// Swap target for one refresh control: the reading at rest, the action
/// on hover or keyboard focus, the spinner while its refresh is in flight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RefreshSwapTarget {
    Reading,
    Action,
    Loading,
}

impl RefreshSwapTarget {
    /// Displayed endpoints per reading in [reading, action, spinner] order.
    pub(super) fn values(self) -> [f32; 3] {
        match self {
            Self::Reading => [1.0, 0.0, 0.0],
            Self::Action => [0.0, 1.0, 0.0],
            Self::Loading => [0.0, 0.0, 1.0],
        }
    }
}

/// Retained interruptible swap for one refresh control. Retargets always
/// start from the currently displayed values, so rapid hover/focus/refresh
/// changes reverse mid-flight exactly like the source transition. Opacity,
/// blur, and paint offset are each retained and interpolated, so a reversal
/// can never flip an offset sign mid-flight.
#[derive(Clone, Copy, Debug)]
pub(super) struct RefreshSwap {
    pub(super) from: [f32; 3],
    pub(super) displayed: [f32; 3],
    pub(super) to: [f32; 3],
    pub(super) off_from: [f32; 3],
    pub(super) off_displayed: [f32; 3],
    pub(super) off_to: [f32; 3],
    pub(super) started_ms: i64,
    pub(super) hovered: bool,
}

impl RefreshSwap {
    pub(super) fn resting() -> Self {
        Self {
            from: [1.0, 0.0, 0.0],
            displayed: [1.0, 0.0, 0.0],
            to: [1.0, 0.0, 0.0],
            off_from: [0.0, 4.0, 4.0],
            off_displayed: [0.0, 4.0, 4.0],
            off_to: [0.0, 4.0, 4.0],
            started_ms: 0,
            hovered: false,
        }
    }
}

/// Paint offsets per target: shown readings sit at zero; the hidden
/// reading exits upward while action and spinner rest below, matching the
/// source hidden frames. A reversal keeps interpolating its retained
/// offset, so the sign can never flip mid-flight.
pub(super) fn swap_offsets_for(to: [f32; 3]) -> [f32; 3] {
    [
        if to[0] >= 1.0 { 0.0 } else { -4.0 },
        if to[1] >= 1.0 { 0.0 } else { 4.0 },
        if to[2] >= 1.0 { 0.0 } else { 4.0 },
    ]
}
/// Width of one meter tick: fourteen full 72/14 pitches with a 2px
/// transparent tail inside every pitch including the last.
pub(super) const PROFILE_METER_TICK_PX: f32 = 72.0 / 14.0 - 2.0;
/// Shared remaining-value tween for meter tooltips: the first reading of
/// a menu-open session runs up from just short of its value, later rows
/// carry the displayed value across, all on the source 250ms smooth-out
/// curve (`MotionDuration::Fast` + `MotionCurve::SmoothOut`, matching
/// `--duration-fast` and `--ease-smooth-out`).
#[derive(Clone, Copy, Debug)]
pub(super) struct ProfileTipTween {
    pub(super) displayed: f64,
    pub(super) from: f64,
    pub(super) to: f64,
    pub(super) started_ms: i64,
    pub(super) seen: bool,
    pub(super) scheduled: bool,
}

impl Default for ProfileTipTween {
    fn default() -> Self {
        Self {
            displayed: 0.0,
            from: 0.0,
            to: 0.0,
            started_ms: 0,
            seen: false,
            scheduled: false,
        }
    }
}
/// Fixed vertical chrome inside the profile panel: header (avatar 32 +
/// vertical padding 32), two separators (1 + margins 8 each), and the action
/// section (container padding 8 + two 36px rows). The usage area scrolls
/// above this chrome under the viewport cap.
pub(super) const PROFILE_MENU_FIXED_CHROME_PX: f32 = 68.0 + 18.0 + 80.0;
