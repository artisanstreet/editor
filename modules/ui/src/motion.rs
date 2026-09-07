//! Typed motion policy and the small set of reachable Artisan motion recipes.
//!
//! Policy is explicit: callers choose [`MotionPolicy::Full`] or
//! [`MotionPolicy::Reduced`]. This module does not inspect the operating system,
//! retain global state, or infer a policy from a window. Reduced motion resolves
//! directly to [`MotionPlan::Immediate`]; it is never represented by a zero- or
//! one-millisecond animation.

use std::time::Duration;

use gpui::Animation;

/// The explicit motion preference supplied by a product boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionPolicy {
    /// Use the selected recipe's full animation.
    Full,
    /// Reveal the final state immediately.
    Reduced,
}

impl MotionPolicy {
    /// Resolves a semantic recipe under this explicit policy.
    #[must_use]
    pub const fn resolve(self, recipe: MotionRecipe) -> MotionPlan {
        match self {
            Self::Full => MotionPlan::Animate(recipe.animation()),
            Self::Reduced => MotionPlan::Immediate,
        }
    }
}

/// A policy decision that keeps immediate and animated presentation distinct.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MotionPlan {
    /// Present the recipe's final state without constructing an animation.
    Immediate,
    /// Run the supplied positive-duration animation specification.
    Animate(MotionAnimation),
}

impl MotionPlan {
    /// Returns the animation specification only for [`Self::Animate`].
    #[must_use]
    pub const fn animation(self) -> Option<MotionAnimation> {
        match self {
            Self::Immediate => None,
            Self::Animate(animation) => Some(animation),
        }
    }
}

/// Shared duration tokens retained from the reachable motion system.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionDuration {
    /// 150 milliseconds.
    Quick,
    /// 250 milliseconds.
    Fast,
    /// 350 milliseconds.
    Medium,
}

impl MotionDuration {
    /// Returns the exact duration represented by this token.
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        match self {
            Self::Quick => Duration::from_millis(150),
            Self::Fast => Duration::from_millis(250),
            Self::Medium => Duration::from_millis(350),
        }
    }
}

/// The selected visual easing curves, sampled independently from GPUI's clock.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionCurve {
    /// `cubic-bezier(0.22, 1, 0.36, 1)`.
    SmoothOut,
    /// `cubic-bezier(0, 0, 0.58, 1)`.
    EaseOut,
    /// `cubic-bezier(0.42, 0, 0.58, 1)`.
    EaseInOut,
    /// `cubic-bezier(0.34, 1.35, 0.64, 1)`; the only selected overshoot.
    CheckBob,
}

impl MotionCurve {
    /// Samples this CSS cubic-bezier at an input progress value.
    ///
    /// CSS timing functions map input progress through the curve's x axis, so
    /// this solves `x(t) = progress` before evaluating `y(t)`. Values outside
    /// the clock interval clamp to the exact endpoints.
    #[must_use]
    pub fn sample(self, progress: f64) -> f64 {
        if progress <= 0.0 {
            return 0.0;
        }
        if progress >= 1.0 {
            return 1.0;
        }

        let (x1, y1, x2, y2) = self.control_points();
        let target_x = progress;
        let mut lower = 0.0_f64;
        let mut upper = 1.0_f64;

        // Every selected x control point lies in [0, 1], making x(t)
        // monotonic. A fixed bisection count is deterministic and avoids the
        // unstable derivative edge cases of a Newton-only solver.
        for _ in 0..48 {
            let t = lower.midpoint(upper);
            if cubic_axis(t, x1, x2) < target_x {
                lower = t;
            } else {
                upper = t;
            }
        }

        cubic_axis(lower.midpoint(upper), y1, y2)
    }

    const fn control_points(self) -> (f64, f64, f64, f64) {
        match self {
            Self::SmoothOut => (0.22, 1.0, 0.36, 1.0),
            Self::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Self::EaseInOut => (0.42, 0.0, 0.58, 1.0),
            Self::CheckBob => (0.34, 1.35, 0.64, 1.0),
        }
    }
}

fn cubic_axis(t: f64, first: f64, second: f64) -> f64 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}

/// A positive-duration animation plus its separate delay and visual curve.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MotionAnimation {
    duration: Duration,
    delay: Duration,
    curve: MotionCurve,
}

impl MotionAnimation {
    const fn from_millis(duration_ms: u64, delay_ms: u64, curve: MotionCurve) -> Self {
        assert!(duration_ms > 0, "motion duration must be positive");
        Self {
            duration: Duration::from_millis(duration_ms),
            delay: Duration::from_millis(delay_ms),
            curve,
        }
    }

    /// Returns the active animation duration, excluding its delay.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// Returns the delay consumers apply before attaching the GPUI clock.
    #[must_use]
    pub const fn delay(self) -> Duration {
        self.delay
    }

    /// Returns the independently sampled visual curve.
    #[must_use]
    pub const fn curve(self) -> MotionCurve {
        self.curve
    }

    /// Creates the safe, linear `0..=1` GPUI clock for this animation.
    ///
    /// Visual easing is intentionally not installed in GPUI: the selected
    /// check-bob curve overshoots, while GPUI requires its easing callback to
    /// remain within `0..=1`. Consumers sample [`Self::curve`] separately.
    #[must_use]
    pub fn gpui_clock(self) -> Animation {
        Animation::new(self.duration)
    }
}

/// Reachable semantic motion recipes used by the first native UI slices.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MotionRecipe {
    /// Open a menu.
    MenuOpen,
    /// Close a menu.
    MenuClose,
    /// Reveal a tooltip after the product delay.
    TooltipIn,
    /// Hide a tooltip.
    TooltipOut,
    /// Swap one text value for another.
    TextSwap,
    /// Reveal a streamed word; reduced motion bypasses this entirely.
    StreamWord,
    /// Expand accordion content.
    AccordionExpand,
    /// Collapse accordion content.
    AccordionCollapse,
    /// Rotate the accordion chevron.
    AccordionChevron,
    /// Swap an icon.
    IconSwap,
    /// Show the success treatment.
    Success,
    /// Draw the success check path after its delay.
    CheckPath,
    /// Apply the distinct overshooting check bob.
    CheckBob,
}

impl MotionRecipe {
    /// Every reachable semantic recipe, in stable catalog order.
    pub const ALL: [Self; 13] = [
        Self::MenuOpen,
        Self::MenuClose,
        Self::TooltipIn,
        Self::TooltipOut,
        Self::TextSwap,
        Self::StreamWord,
        Self::AccordionExpand,
        Self::AccordionCollapse,
        Self::AccordionChevron,
        Self::IconSwap,
        Self::Success,
        Self::CheckPath,
        Self::CheckBob,
    ];

    const fn animation(self) -> MotionAnimation {
        match self {
            Self::MenuOpen => MotionAnimation::from_millis(250, 0, MotionCurve::SmoothOut),
            Self::MenuClose => MotionAnimation::from_millis(150, 0, MotionCurve::SmoothOut),
            Self::TooltipIn => MotionAnimation::from_millis(150, 80, MotionCurve::EaseOut),
            Self::TooltipOut => MotionAnimation::from_millis(50, 0, MotionCurve::EaseOut),
            Self::TextSwap => MotionAnimation::from_millis(150, 0, MotionCurve::EaseInOut),
            Self::StreamWord => MotionAnimation::from_millis(320, 0, MotionCurve::SmoothOut),
            Self::AccordionExpand | Self::AccordionCollapse | Self::AccordionChevron => {
                MotionAnimation::from_millis(250, 0, MotionCurve::SmoothOut)
            }
            Self::IconSwap => MotionAnimation::from_millis(250, 0, MotionCurve::EaseInOut),
            Self::Success => MotionAnimation::from_millis(500, 0, MotionCurve::SmoothOut),
            Self::CheckPath => MotionAnimation::from_millis(500, 80, MotionCurve::SmoothOut),
            Self::CheckBob => MotionAnimation::from_millis(500, 0, MotionCurve::CheckBob),
        }
    }
}
