//! Noninteractive loading skeleton primitive for the reached editor surfaces.
//!
//! The legacy wrapper is intentionally small: a plain `div` with
//! `bg-muted rounded-xl animate-pulse`. [`SkeletonStyle`] resolves the shared
//! muted paint and the pinned GPUI `rounded-xl` token; width and height stay
//! entirely with the caller. The constructor returns a styled element so
//! callers can continue chaining GPUI refinements, with later values winning
//! in the usual `Styled` order.
//!
//! Motion is explicit through [`crate::motion::MotionPolicy`]. Full motion
//! attaches GPUI's repeating two-second animation to that same `Div` and
//! samples the legacy `cubic-bezier(0.4, 0, 0.6, 1)` opacity curve in the
//! animator. GPUI's animation clock remains linear because its callback is
//! the established seam for a visual curve that is not one of the shared
//! clock easings. Reduced motion returns the opaque `Div` directly and never
//! constructs a GPUI animation.

use std::time::Duration;

use gpui::{Animation, AnimationExt, AnyElement, Div, Hsla, IntoElement, Pixels, Styled, div, px};

use crate::motion::MotionPolicy;
use crate::theme::ArtisanTheme;

const PULSE_DURATION: Duration = Duration::from_secs(2);
const PULSE_MIN_OPACITY: f32 = 0.5;
const PULSE_MAX_OPACITY: f32 = 1.0;
const ROUNDED_XL_PX: f32 = 12.0;
const PULSE_ANIMATION_ID: &str = "artisan-skeleton-pulse";

/// Paint and geometry values resolved for one loading skeleton.
///
/// Width and height are deliberately absent. Every reached caller supplies
/// those values, and callers may override any GPUI style value after
/// constructing the element.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkeletonStyle {
    /// Shared theme `--muted` paint.
    pub background: Hsla,
    /// The pinned GPUI `rounded-xl` token: 12 px.
    pub corner_radius: Pixels,
}

impl SkeletonStyle {
    /// Resolves the exact reached recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            background: theme.colors.muted.to_paint(),
            corner_radius: px(ROUNDED_XL_PX),
        }
    }
}

/// The full-motion animation specification for the legacy pulse.
///
/// This is a small copyable description rather than GPUI's runtime animation
/// object so reduced motion can carry no animation value at all until the
/// full-motion branch is selected.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct SkeletonAnimation;

impl SkeletonAnimation {
    /// Constructs the full-motion pulse specification.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// The legacy `animate-pulse` duration.
    #[must_use]
    pub const fn duration(self) -> Duration {
        PULSE_DURATION
    }

    /// Samples the legacy opacity at normalized iteration progress.
    ///
    /// The keyframe values are `1 → 0.5 → 1`, with the legacy cubic-bezier
    /// timing function applied independently to each half of the iteration.
    /// Inputs are bounded so the visual value never overshoots the CSS keyframe
    /// interval, including when a caller supplies a non-finite sample.
    #[must_use]
    pub fn opacity_at(self, progress: f32) -> f32 {
        if !progress.is_finite() {
            return PULSE_MAX_OPACITY;
        }

        let progress = progress.clamp(0.0, 1.0);
        let eased = if progress <= 0.5 {
            cubic_bezier(progress * 2.0)
        } else {
            cubic_bezier((progress - 0.5) * 2.0)
        };
        let opacity = if progress <= 0.5 {
            PULSE_MAX_OPACITY - (PULSE_MAX_OPACITY - PULSE_MIN_OPACITY) * eased
        } else {
            PULSE_MIN_OPACITY + (PULSE_MAX_OPACITY - PULSE_MIN_OPACITY) * eased
        };

        opacity.clamp(PULSE_MIN_OPACITY, PULSE_MAX_OPACITY)
    }

    /// Builds the repeating, linear-clock GPUI animation.
    #[must_use]
    pub fn gpui_animation(self) -> Animation {
        Animation::new(self.duration()).repeat()
    }
}

/// The explicit Skeleton motion decision, mirroring the shared motion
/// vocabulary while keeping this primitive's recipe local to its owned file.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SkeletonMotionPlan {
    /// Render the final opaque shape immediately.
    Immediate,
    /// Run the repeating pulse animation.
    Animate(SkeletonAnimation),
}

impl SkeletonMotionPlan {
    /// Resolves the Skeleton animation decision from the shared policy.
    #[must_use]
    pub const fn for_policy(policy: MotionPolicy) -> Self {
        match policy {
            MotionPolicy::Full => Self::Animate(SkeletonAnimation::new()),
            MotionPolicy::Reduced => Self::Immediate,
        }
    }

    /// Returns the full-motion specification, or no animation for reduced
    /// motion.
    #[must_use]
    pub const fn animation(self) -> Option<SkeletonAnimation> {
        match self {
            Self::Immediate => None,
            Self::Animate(animation) => Some(animation),
        }
    }

    /// Returns the opacity the plan presents at a normalized sample.
    #[must_use]
    pub fn opacity_at(self, progress: f32) -> f32 {
        match self {
            Self::Immediate => PULSE_MAX_OPACITY,
            Self::Animate(animation) => animation.opacity_at(progress),
        }
    }
}

/// A styled Skeleton element that resolves into the underlying plain GPUI
/// `Div` (or its GPUI animation element) only when it enters an element tree.
///
/// It owns no children, event handlers, focus behavior, or intrinsic size.
pub struct SkeletonElement {
    element: Div,
    motion: SkeletonMotionPlan,
}

impl Styled for SkeletonElement {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.element.style()
    }
}

impl IntoElement for SkeletonElement {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let Self { element, motion } = self;
        match motion {
            SkeletonMotionPlan::Immediate => element.into_any_element(),
            SkeletonMotionPlan::Animate(animation) => element
                .with_animation(
                    PULSE_ANIMATION_ID,
                    animation.gpui_animation(),
                    move |element, progress| element.opacity(animation.opacity_at(progress)),
                )
                .into_any_element(),
        }
    }
}

/// Returns the reached noninteractive Skeleton recipe.
///
/// The returned value is a single styled `Div` at render time. Callers own its
/// width and height and may chain any GPUI [`Styled`] refinement, including
/// later width/height values. [`MotionPolicy::Reduced`] takes the direct static
/// path; [`MotionPolicy::Full`] attaches the repeating pulse to the same
/// element without adding a layout wrapper.
#[must_use]
pub fn skeleton(style: SkeletonStyle, policy: MotionPolicy) -> SkeletonElement {
    SkeletonElement {
        element: div().rounded(style.corner_radius).bg(style.background),
        motion: SkeletonMotionPlan::for_policy(policy),
    }
}

/// Samples `cubic-bezier(0.4, 0, 0.6, 1)` at a normalized input.
fn cubic_bezier(progress: f32) -> f32 {
    let progress = f64::from(progress);
    let mut lower = 0.0_f64;
    let mut upper = 1.0_f64;

    // CSS timing functions solve the x axis before evaluating y. The x
    // control points are monotonic, so deterministic bisection avoids a
    // derivative singularity and keeps the visual result bounded.
    for _ in 0..48 {
        let t = lower.midpoint(upper);
        if cubic_axis(t, 0.4, 0.6) < progress {
            lower = t;
        } else {
            upper = t;
        }
    }

    let value = cubic_axis(lower.midpoint(upper), 0.0, 1.0);
    #[allow(clippy::cast_possible_truncation)]
    {
        value as f32
    }
}

fn cubic_axis(t: f64, first: f64, second: f64) -> f64 {
    let inverse = 1.0 - t;
    3.0 * inverse * inverse * t * first + 3.0 * inverse * t * t * second + t * t * t
}
