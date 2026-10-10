//! The thread's goal, shown as a lighter strip that grows out of the card's
//! tab along its top edge.
//!
//! A goal belongs to the thread, not to a turn: the engine keeps working
//! until its condition holds, so the strip stays for as long as the goal
//! does. Motion follows the transitions-dev tokens:
//!
//! - Set, the strip grows out of the tab to the card's right edge (400 ms
//!   smooth-out, the panel-open recipe); over its last stretch it settles
//!   flush into the card's corner (see [`artisan_ui::tab_card`]). Its label
//!   rises in 250 ms after the growth starts (250 ms, 4 px, 2 px blur).
//! - A changed objective or state swaps the label in place (150 ms out,
//!   150 ms in, ease-in-out, 4 px, 2 px blur).
//! - Cleared, the label leaves first (150 ms) and the strip retracts into the
//!   tab (350 ms smooth-out, the panel-close recipe).
//!
//! Reduced motion places and removes the strip at once.
//!
//! The same strip carries a provider safety system's word on the current
//! run (a held response, a reroute, a refusal), ahead of the goal: it says
//! why the turn changed course, which the reader needs before anything else.

use std::time::{Duration, Instant};

use artisan_assets::AssetId;
use artisan_ui::asset_seam::asset_glyph;
use artisan_ui::motion::MotionCurve;
use gpui::{
    AnyElement, Filter, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};

use artisan_ui::theme::{ArtisanTheme, DesktopTheme, ThemeMode};

const GROW: Duration = Duration::from_millis(400);
const LABEL_DELAY: Duration = Duration::from_millis(250);
const LABEL_RISE: Duration = Duration::from_millis(250);
const SWAP_HALF: Duration = Duration::from_millis(150);
const LABEL_OUT: Duration = Duration::from_millis(150);
const RETRACT: Duration = Duration::from_millis(350);

/// Where the thread's goal stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ComposerGoalState {
    /// The engine is working toward it.
    Active,
    /// Paused, by the reader or by the engine waiting on a decision.
    Paused,
    /// The provider holds the response back while it runs a safety check.
    SafetyHolding,
    /// The provider moved the request to a more conservative model.
    SafetyRerouted,
    /// The provider refused to serve the request.
    SafetyRefused,
}

impl ComposerGoalState {
    const fn is_safety(self) -> bool {
        matches!(
            self,
            Self::SafetyHolding | Self::SafetyRerouted | Self::SafetyRefused
        )
    }
}

/// The goal the card shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ComposerGoal {
    pub(crate) objective: SharedString,
    pub(crate) state: ComposerGoalState,
    /// The provider's own explanation after the headline, when it gave one.
    pub(crate) detail: Option<SharedString>,
}

/// The strip's motion, retained by the composer across renders.
#[derive(Debug, Default)]
pub(crate) struct GoalStrip {
    /// The goal painted now; while retracting, the one leaving.
    shown: Option<ComposerGoal>,
    /// When the strip started growing, while it grows.
    grown_at: Option<Instant>,
    /// When the strip started retracting.
    leaving_at: Option<Instant>,
    /// The label being swapped out and when the swap started.
    swap: Option<(Instant, ComposerGoal)>,
}

/// One frame of the strip.
pub(crate) struct GoalStripFrame {
    /// How far the strip has grown out of the tab, eased.
    pub(crate) reveal: f32,
    /// The strip's content.
    pub(crate) content: AnyElement,
}

impl GoalStrip {
    /// Follows the thread's goal; returns whether anything changed.
    pub(crate) fn set(&mut self, goal: Option<ComposerGoal>, now: Instant, reduced: bool) -> bool {
        match (goal, self.shown.clone()) {
            (None, None) => false,
            (None, Some(_)) => {
                if self.leaving_at.is_some() {
                    return false;
                }
                if reduced {
                    *self = Self::default();
                } else {
                    self.leaving_at = Some(now);
                    self.swap = None;
                }
                true
            }
            (Some(goal), None) => {
                self.shown = Some(goal);
                self.grown_at = (!reduced).then_some(now);
                self.leaving_at = None;
                self.swap = None;
                true
            }
            (Some(goal), Some(shown)) => {
                if self.leaving_at.take().is_some() {
                    // Set again while retracting: grow back from here.
                    self.grown_at = (!reduced).then_some(now);
                    self.shown = Some(goal);
                    return true;
                }
                if goal == shown {
                    return false;
                }
                if !reduced {
                    self.swap = Some((now, shown));
                }
                self.shown = Some(goal);
                true
            }
        }
    }

    /// Paints this frame; `None` when there is no strip. `moving` reports
    /// whether the composer must render the next frame.
    pub(crate) fn frame(
        &mut self,
        now: Instant,
        theme: DesktopTheme,
        moving: &mut bool,
    ) -> Option<GoalStripFrame> {
        let goal = self.shown.clone()?;
        let mut reveal = 1.0;
        let mut label = 1.0;
        if let Some(leaving) = self.leaving_at {
            let elapsed = now.saturating_duration_since(leaving);
            label = 1.0 - progress(elapsed, Duration::ZERO, LABEL_OUT);
            reveal = 1.0
                - eased(
                    MotionCurve::SmoothOut,
                    progress(elapsed, LABEL_OUT, RETRACT),
                );
            if elapsed >= LABEL_OUT + RETRACT {
                *self = Self::default();
                return None;
            }
            *moving = true;
        } else if let Some(grown) = self.grown_at {
            let elapsed = now.saturating_duration_since(grown);
            reveal = eased(
                MotionCurve::SmoothOut,
                progress(elapsed, Duration::ZERO, GROW),
            );
            label = eased(
                MotionCurve::SmoothOut,
                progress(elapsed, LABEL_DELAY, LABEL_RISE),
            );
            if elapsed >= GROW.max(LABEL_DELAY + LABEL_RISE) {
                self.grown_at = None;
            } else {
                *moving = true;
            }
        }
        let content = match self.swap.clone() {
            Some((started, from)) => {
                let elapsed = now.saturating_duration_since(started);
                let swap = progress(elapsed, Duration::ZERO, SWAP_HALF * 2);
                if swap >= 1.0 {
                    self.swap = None;
                    goal_label(&goal, theme, label, 0.0)
                } else {
                    *moving = true;
                    if swap < 0.5 {
                        let out = eased(MotionCurve::EaseInOut, swap * 2.0);
                        goal_label(&from, theme, 1.0 - out, -4.0 * out)
                    } else {
                        let enter = eased(MotionCurve::EaseInOut, swap * 2.0 - 1.0);
                        goal_label(&goal, theme, enter, 4.0 * (1.0 - enter))
                    }
                }
            }
            None => goal_label(&goal, theme, label, 4.0 * (1.0 - label)),
        };
        Some(GoalStripFrame { reveal, content })
    }
}

/// The strip's label: what the goal is and where it stands.
fn goal_label(goal: &ComposerGoal, theme: DesktopTheme, opacity: f32, rise: f32) -> AnyElement {
    let state = match goal.state {
        ComposerGoalState::Paused => Some(SharedString::from("Paused")),
        ComposerGoalState::Active
        | ComposerGoalState::SafetyHolding
        | ComposerGoalState::SafetyRerouted
        | ComposerGoalState::SafetyRefused => goal.detail.clone(),
    };
    let (glyph, tone) = match goal.state {
        ComposerGoalState::Active | ComposerGoalState::Paused => {
            (AssetId::TABLER_TARGET_ARROW, theme.secondary)
        }
        // The composer paints dark, so the banner tones come from the dark
        // palette.
        ComposerGoalState::SafetyHolding | ComposerGoalState::SafetyRerouted => (
            AssetId::TABLER_SHIELD_LOCK,
            ArtisanTheme::for_mode(ThemeMode::Dark)
                .colors
                .banner_warning
                .to_paint(),
        ),
        ComposerGoalState::SafetyRefused => (
            AssetId::TABLER_SHIELD_LOCK,
            ArtisanTheme::for_mode(ThemeMode::Dark)
                .colors
                .banner_error
                .to_paint(),
        ),
    };
    let headline = if goal.state.is_safety() {
        tone
    } else {
        theme.foreground
    };
    let blur = (rise.abs() / 2.0).min(2.0);
    div()
        .relative()
        .top(px(rise))
        .h_full()
        .pl(px(18.0))
        .pr(px(14.0))
        .flex()
        .items_center()
        .gap(px(6.0))
        .text_size(px(13.0))
        .opacity(opacity)
        .when_some_blur(blur)
        // The goal's mark, sized and toned like the tab's folder; a safety
        // notice takes the shield in its banner tone.
        .child(
            div()
                .flex_shrink_0()
                .text_color(tone)
                .child(asset_glyph(glyph).size(px(15.0))),
        )
        // A goal's objective gives way to its state; a safety headline
        // stays whole and the provider's explanation gives way instead.
        .child({
            let objective = div()
                .truncate()
                .text_color(headline)
                .child(goal.objective.clone());
            if goal.state.is_safety() {
                objective.flex_shrink_0().max_w_full()
            } else {
                objective.min_w_0()
            }
        })
        .children(state.map(|state| {
            let state = div().truncate().text_color(theme.secondary).child(state);
            if goal.state.is_safety() {
                state.min_w_0()
            } else {
                state.flex_shrink_0()
            }
        }))
        .into_any_element()
}

/// Applies a blur only while there is one: a zero blur still costs a layer.
trait BlurWhile {
    fn when_some_blur(self, blur: f32) -> Self;
}

impl BlurWhile for gpui::Div {
    fn when_some_blur(self, blur: f32) -> Self {
        if blur > 0.0 {
            self.filter(vec![Filter::Blur(px(blur))])
        } else {
            self
        }
    }
}

fn progress(elapsed: Duration, delay: Duration, duration: Duration) -> f32 {
    let Some(run) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    (run.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

#[expect(clippy::cast_possible_truncation, reason = "progress stays in 0..=1")]
fn eased(curve: MotionCurve, value: f32) -> f32 {
    curve.sample(f64::from(value)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(objective: &str, state: ComposerGoalState) -> ComposerGoal {
        ComposerGoal {
            objective: SharedString::from(objective.to_owned()),
            state,
            detail: None,
        }
    }

    fn reveal(strip: &mut GoalStrip, at: Instant) -> Option<(f32, bool)> {
        let mut moving = false;
        strip
            .frame(at, DesktopTheme::neutral_dark(), &mut moving)
            .map(|frame| (frame.reveal, moving))
    }

    #[test]
    fn setting_a_goal_grows_the_strip_then_rests() {
        let start = Instant::now();
        let mut strip = GoalStrip::default();
        assert!(strip.set(
            Some(goal("tests pass", ComposerGoalState::Active)),
            start,
            false
        ));
        assert_eq!(reveal(&mut strip, start), Some((0.0, true)));
        let (mid, moving) = reveal(&mut strip, start + GROW / 2).expect("strip");
        assert!(
            mid > 0.5 && mid < 1.0 && moving,
            "smooth-out passes halfway early"
        );
        assert_eq!(reveal(&mut strip, start + GROW * 2), Some((1.0, false)));
    }

    #[test]
    fn clearing_retracts_after_the_label_leaves_then_removes_the_strip() {
        let start = Instant::now();
        let mut strip = GoalStrip::default();
        strip.set(
            Some(goal("tests pass", ComposerGoalState::Active)),
            start,
            true,
        );
        assert!(strip.set(None, start, false));
        assert_eq!(
            reveal(&mut strip, start + LABEL_OUT / 2).map(|r| r.0),
            Some(1.0)
        );
        let (retracting, _) = reveal(&mut strip, start + LABEL_OUT + RETRACT / 2).expect("strip");
        assert!(retracting < 1.0 && retracting > 0.0);
        assert_eq!(reveal(&mut strip, start + LABEL_OUT + RETRACT), None);
    }

    #[test]
    fn reduced_motion_places_and_removes_at_once() {
        let start = Instant::now();
        let mut strip = GoalStrip::default();
        strip.set(
            Some(goal("tests pass", ComposerGoalState::Active)),
            start,
            true,
        );
        assert_eq!(reveal(&mut strip, start), Some((1.0, false)));
        strip.set(None, start, true);
        assert_eq!(reveal(&mut strip, start), None);
    }

    #[test]
    fn a_state_change_swaps_the_label_without_regrowing() {
        let start = Instant::now();
        let mut strip = GoalStrip::default();
        strip.set(
            Some(goal("tests pass", ComposerGoalState::Active)),
            start,
            true,
        );
        assert!(strip.set(
            Some(goal("tests pass", ComposerGoalState::Paused)),
            start,
            false
        ));
        let (reveal_now, moving) = reveal(&mut strip, start + SWAP_HALF).expect("strip");
        assert_eq!(reveal_now, 1.0);
        assert!(moving, "the label is mid-swap");
        assert!(!strip.set(
            Some(goal("tests pass", ComposerGoalState::Paused)),
            start,
            false
        ));
    }
}
