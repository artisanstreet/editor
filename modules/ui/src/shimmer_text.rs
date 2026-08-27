//! Native, theme-aware travelling-band text for loading and status surfaces.
//!
//! GPUI 0.2.2 exposes styled text runs, but it does not expose a continuous
//! gradient or a per-glyph paint callback. The animated path therefore rebuilds
//! [`gpui::StyledText`] with highlight ranges for each character on every GPUI
//! animation frame. This is an intentional segmented-glyph treatment: the
//! stable character positions make the phase and palette decisions testable,
//! while the text itself remains one measured GPUI text element and keeps its
//! normal wrapping behavior.
//!
//! Motion is explicit through [`crate::motion::MotionPolicy`]. Inactive and
//! reduced-motion states return the same readable text without constructing a
//! GPUI animation, so neither state requests animation frames.

use std::{convert::TryFrom, ops::Range, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, Div, HighlightStyle, Hsla, IntoElement, ParentElement,
    SharedString, Styled, StyledText, div,
};

use crate::motion::MotionPolicy;
use crate::theme::ArtisanTheme;

/// The named color treatments reached by the legacy `ShimmerText` wrapper.
///
/// The native port maps these names to the audited semantic theme tokens rather
/// than inventing a second color ramp. Several names intentionally share a
/// semantic token because the native theme records meaning instead of every
/// Tailwind shade individually.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum ShimmerTextVariant {
    /// Theme foreground text.
    #[default]
    Default,
    /// Theme secondary foreground text.
    Secondary,
    /// Theme destructive foreground text.
    Destructive,
    /// Theme error treatment.
    Red,
    /// Theme information treatment.
    Blue,
    /// Theme success treatment.
    Green,
    /// Theme warning treatment.
    Yellow,
    /// Theme question-from treatment.
    Purple,
    /// Theme question-to treatment.
    Pink,
    /// Theme favorite treatment.
    Orange,
    /// Theme unread treatment.
    Cyan,
    /// Theme information treatment.
    Indigo,
    /// Theme question-from treatment.
    Violet,
    /// Theme question-to treatment.
    Rose,
    /// Theme favorite treatment.
    Amber,
    /// Theme success treatment.
    Lime,
    /// Theme success treatment.
    Emerald,
    /// Theme unread treatment.
    Sky,
    /// Theme muted foreground text.
    Slate,
    /// Theme question-from treatment.
    Fuchsia,
}

impl ShimmerTextVariant {
    /// Every public treatment in the audited legacy catalog, in legacy order.
    pub const ALL: [Self; 20] = [
        Self::Default,
        Self::Secondary,
        Self::Destructive,
        Self::Red,
        Self::Blue,
        Self::Green,
        Self::Yellow,
        Self::Purple,
        Self::Pink,
        Self::Orange,
        Self::Cyan,
        Self::Indigo,
        Self::Violet,
        Self::Rose,
        Self::Amber,
        Self::Lime,
        Self::Emerald,
        Self::Sky,
        Self::Slate,
        Self::Fuchsia,
    ];

    /// Resolves this named treatment through the shared theme palette.
    #[must_use]
    pub fn resolve(self, theme: ArtisanTheme) -> ShimmerTextStyle {
        let foreground = match self {
            Self::Default => theme.colors.foreground,
            Self::Secondary => theme.colors.secondary_foreground,
            Self::Destructive => theme.colors.destructive,
            Self::Red => theme.colors.banner_error,
            Self::Blue | Self::Indigo => theme.colors.banner_info,
            Self::Green | Self::Lime | Self::Emerald => theme.colors.banner_success,
            Self::Yellow => theme.colors.banner_warning,
            Self::Purple | Self::Violet | Self::Fuchsia => theme.colors.question_from,
            Self::Pink | Self::Rose => theme.colors.question_to,
            Self::Orange | Self::Amber => theme.colors.favorite,
            Self::Cyan | Self::Sky => theme.colors.unread,
            Self::Slate => theme.colors.muted_foreground,
        }
        .to_paint();

        ShimmerTextStyle {
            foreground,
            highlight: theme.colors.highlight.to_paint(),
        }
    }
}

/// Alias emphasizing that the variants are color treatments when configuring
/// a component.
pub type ShimmerTextColor = ShimmerTextVariant;

/// Foreground and travelling-band colors resolved for one theme and variant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShimmerTextStyle {
    /// The settled text color.
    pub foreground: Hsla,
    /// The theme-aware color used by highlighted glyph runs.
    pub highlight: Hsla,
}

impl ShimmerTextStyle {
    /// Resolves a named variant through the shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, variant: ShimmerTextVariant) -> Self {
        variant.resolve(theme)
    }
}

/// Alias for callers that describe the resolved colors as a palette.
pub type ShimmerPalette = ShimmerTextStyle;

/// Default active animation duration, matching the audited three-second CSS
/// treatment.
pub const DEFAULT_DURATION: Duration = Duration::from_secs(3);

/// Default initial animation delay, matching the audited 1.5-second treatment.
pub const DEFAULT_DELAY: Duration = Duration::from_millis(1_500);

/// Default travelling-band spread as a percentage of the text width.
pub const DEFAULT_SPREAD: f32 = 50.0;

const MIN_ANIMATION_DURATION: Duration = Duration::from_millis(1);
const MAX_SPREAD: f32 = 100.0;
const SHIMMER_ANIMATION_ID: &str = "artisan-shimmer-text";

/// Timing and band-width inputs for a [`ShimmerText`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShimmerTiming {
    duration: Duration,
    delay: Duration,
    spread: f32,
}

impl Default for ShimmerTiming {
    fn default() -> Self {
        Self {
            duration: DEFAULT_DURATION,
            delay: DEFAULT_DELAY,
            spread: DEFAULT_SPREAD,
        }
    }
}

impl ShimmerTiming {
    /// Builds timing from typed GPUI-compatible durations and a percentage.
    ///
    /// A zero duration is raised to one millisecond so a full-motion component
    /// can never hand GPUI a clock that divides by zero. Spread is bounded to
    /// `0..=100`; non-finite spread values become zero.
    #[must_use]
    pub fn new(duration: Duration, delay: Duration, spread: f32) -> Self {
        Self {
            duration: nonzero_duration(duration),
            delay,
            spread: normalize_spread(spread),
        }
    }

    /// Builds timing from the legacy seconds-based inputs.
    #[must_use]
    pub fn from_seconds(duration: f32, delay: f32, spread: f32) -> Self {
        Self::new(
            seconds_to_duration(duration, MIN_ANIMATION_DURATION),
            seconds_to_duration(delay, Duration::ZERO),
            spread,
        )
    }

    /// Returns the active band-travel duration, excluding the delay.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.duration
    }

    /// Returns the delay before a band cycle begins.
    #[must_use]
    pub const fn delay(self) -> Duration {
        self.delay
    }

    /// Returns the bounded spread percentage.
    #[must_use]
    pub const fn spread(self) -> f32 {
        self.spread
    }

    /// Returns the duration of the native repeating clock, including its delay.
    #[must_use]
    pub fn cycle_duration(self) -> Duration {
        self.duration.saturating_add(self.delay)
    }

    /// Returns the deterministic phase for elapsed time.
    #[must_use]
    pub fn phase_at(self, elapsed: Duration) -> f32 {
        phase_at(elapsed, self.duration, self.delay)
    }

    /// Returns a copy with a new active duration.
    #[must_use]
    pub fn with_duration(self, duration: Duration) -> Self {
        Self::new(duration, self.delay, self.spread)
    }

    /// Returns a copy with a new delay.
    #[must_use]
    pub fn with_delay(self, delay: Duration) -> Self {
        Self::new(self.duration, delay, self.spread)
    }

    /// Returns a copy with a new bounded spread percentage.
    #[must_use]
    pub fn with_spread(self, spread: f32) -> Self {
        Self::new(self.duration, self.delay, spread)
    }
}

/// Calculates a travelling-band phase in `0..1`, including initial delay and
/// wrapping after each active duration.
///
/// The delay is applied once to the supplied elapsed timeline, matching the
/// legacy animation's initial CSS delay. The GPUI repeating clock uses the
/// separate [`ShimmerAnimation::phase_for_progress`] adapter, which includes
/// the delay in its repeat period because GPUI's pinned animation API exposes
/// one repeating duration rather than a distinct initial-delay hook.
#[must_use]
pub fn phase_at(elapsed: Duration, duration: Duration, delay: Duration) -> f32 {
    if duration.is_zero() || elapsed <= delay {
        return 0.0;
    }

    phase_from_seconds(
        elapsed.as_secs_f32(),
        duration.as_secs_f32(),
        delay.as_secs_f32(),
    )
}

/// A normalized, linear GPUI animation specification for a shimmer band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShimmerAnimation {
    timing: ShimmerTiming,
}

impl ShimmerAnimation {
    /// Creates an animation specification from bounded timing.
    #[must_use]
    pub const fn new(timing: ShimmerTiming) -> Self {
        Self { timing }
    }

    /// Returns the active travel duration.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.timing.duration()
    }

    /// Returns the initial/repeated cycle delay.
    #[must_use]
    pub const fn delay(self) -> Duration {
        self.timing.delay()
    }

    /// Returns the complete native repeating-clock duration.
    #[must_use]
    pub fn cycle_duration(self) -> Duration {
        self.timing.cycle_duration()
    }

    /// Returns the configured spread percentage.
    #[must_use]
    pub const fn spread(self) -> f32 {
        self.timing.spread()
    }

    /// Adapts a GPUI clock progress value to the delayed, wrapping shimmer
    /// phase used by the segmented glyph decisions.
    #[must_use]
    pub fn phase_for_progress(self, progress: f32) -> f32 {
        let progress = if progress.is_finite() {
            progress.clamp(0.0, 1.0)
        } else {
            0.0
        };

        phase_from_seconds(
            self.cycle_duration().as_secs_f32() * progress,
            self.duration().as_secs_f32(),
            self.delay().as_secs_f32(),
        )
    }

    /// Returns the repeating linear GPUI clock used by the visual element.
    #[must_use]
    pub fn gpui_animation(self) -> Animation {
        Animation::new(self.cycle_duration()).repeat()
    }
}

/// Explicit motion outcome for a shimmer component.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShimmerMotionPlan {
    /// Render settled text without an animation wrapper.
    Immediate,
    /// Render the text with a repeating native band animation.
    Animate(ShimmerAnimation),
}

impl ShimmerMotionPlan {
    /// Resolves a full/reduced policy for an active component.
    #[must_use]
    pub fn for_policy(policy: MotionPolicy, timing: ShimmerTiming) -> Self {
        Self::for_active_policy(policy, timing, true)
    }

    /// Resolves policy and activity together.
    #[must_use]
    pub fn for_active_policy(policy: MotionPolicy, timing: ShimmerTiming, active: bool) -> Self {
        if active && policy == MotionPolicy::Full {
            Self::Animate(ShimmerAnimation::new(timing))
        } else {
            Self::Immediate
        }
    }

    /// Returns the animation only for the active full-motion path.
    #[must_use]
    pub const fn animation(self) -> Option<ShimmerAnimation> {
        match self {
            Self::Immediate => None,
            Self::Animate(animation) => Some(animation),
        }
    }

    /// Returns whether this plan requests animation frames when rendered.
    #[must_use]
    pub const fn is_animating(self) -> bool {
        matches!(self, Self::Animate(_))
    }
}

/// The visual decision for one character-sized text segment.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ShimmerSegmentStyle {
    /// Use the settled foreground color.
    Base,
    /// Use the theme-aware band highlight color.
    Highlight,
}

/// One stable byte range and normalized character position in a shimmer text.
#[derive(Clone, Debug, PartialEq)]
pub struct ShimmerSegment {
    /// UTF-8 byte range for the character-sized segment.
    pub range: Range<usize>,
    /// Character-order position in `0..=1`, before GPUI glyph measurement.
    pub position: f32,
    /// The color decision for this phase.
    pub style: ShimmerSegmentStyle,
}

/// Returns per-character segment decisions for a normalized phase and spread.
///
/// GPUI's styled text API works in UTF-8 byte ranges, so each Unicode scalar
/// value becomes one valid range. The visual band travels from beyond the
/// trailing edge to beyond the leading edge; a zero spread is deliberately
/// settled and highlights no ranges.
#[must_use]
pub fn segments_for(content: &str, phase: f32, spread: f32) -> Vec<ShimmerSegment> {
    let starts: Vec<usize> = content.char_indices().map(|(start, _)| start).collect();
    let count = starts.len();
    if count == 0 {
        return Vec::new();
    }

    let phase = normalize_phase(phase);
    let width = normalize_spread(spread) / MAX_SPREAD;
    let center = 1.0 + width - phase * (1.0 + 2.0 * width);
    let half_width = width / 2.0;
    let denominator = count.saturating_sub(1);

    starts
        .iter()
        .copied()
        .enumerate()
        .map(|(index, start)| {
            let end = starts.get(index + 1).copied().unwrap_or(content.len());
            let position = index
                .saturating_mul(usize::from(u16::MAX))
                .checked_div(denominator)
                .map_or(0.5, |fixed_point| {
                    let fixed_point = u16::try_from(fixed_point).unwrap_or(u16::MAX);
                    f32::from(fixed_point) / f32::from(u16::MAX)
                });
            let style = if width > 0.0 && (position - center).abs() < half_width {
                ShimmerSegmentStyle::Highlight
            } else {
                ShimmerSegmentStyle::Base
            };

            ShimmerSegment {
                range: start..end,
                position,
                style,
            }
        })
        .collect()
}

/// Returns the highlighted UTF-8 ranges for a phase and spread.
#[must_use]
pub fn highlighted_ranges(content: &str, phase: f32, spread: f32) -> Vec<Range<usize>> {
    segments_for(content, phase, spread)
        .into_iter()
        .filter(|segment| segment.style == ShimmerSegmentStyle::Highlight)
        .map(|segment| segment.range)
        .collect()
}

/// Semantic state retained for a future accessibility layer.
///
/// GPUI 0.2.2 does not provide a platform accessibility tree. This value keeps
/// the intended label and activity state available to a future integration
/// without claiming that native accessibility is currently emitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShimmerSemanticState {
    /// The status label intended for assistive presentation.
    pub label: SharedString,
    /// Whether the product considers the status active.
    pub active: bool,
}

/// A reusable native GPUI shimmer text element.
///
/// The component keeps one text element mounted while `active` changes. Its
/// settled path is readable and its reduced-motion path is immediate; callers
/// supply the explicit [`MotionPolicy`] at construction time.
pub struct ShimmerText {
    element: Div,
    content: SharedString,
    theme: ArtisanTheme,
    variant: ShimmerTextVariant,
    timing: ShimmerTiming,
    active: bool,
    motion: MotionPolicy,
    semantic_label: SharedString,
}

impl ShimmerText {
    /// Creates a default-variant shimmer text with the audited timing values.
    #[must_use]
    pub fn new(
        content: impl Into<SharedString>,
        theme: ArtisanTheme,
        motion: MotionPolicy,
    ) -> Self {
        let content = content.into();
        let variant = ShimmerTextVariant::default();
        let style = variant.resolve(theme);
        Self {
            element: div().text_color(style.foreground),
            semantic_label: content.clone(),
            content,
            theme,
            variant,
            timing: ShimmerTiming::default(),
            active: true,
            motion,
        }
    }

    /// Returns the text's stable content.
    #[must_use]
    pub fn content(&self) -> &str {
        self.content.as_ref()
    }

    /// Returns the selected color treatment.
    #[must_use]
    pub const fn selected_variant(&self) -> ShimmerTextVariant {
        self.variant
    }

    /// Returns the resolved theme-aware foreground/highlight colors.
    #[must_use]
    pub fn visual_style(&self) -> ShimmerTextStyle {
        self.variant.resolve(self.theme)
    }

    /// Returns the resolved theme-aware palette.
    #[must_use]
    pub fn palette(&self) -> ShimmerPalette {
        self.visual_style()
    }

    /// Returns whether the travelling band is active.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Returns the explicit motion policy supplied by the caller.
    #[must_use]
    pub const fn policy(&self) -> MotionPolicy {
        self.motion
    }

    /// Returns all timing inputs.
    #[must_use]
    pub const fn timing_value(&self) -> ShimmerTiming {
        self.timing
    }

    /// Returns the active travel duration.
    #[must_use]
    pub const fn duration_value(&self) -> Duration {
        self.timing.duration()
    }

    /// Returns the configured delay.
    #[must_use]
    pub const fn delay_value(&self) -> Duration {
        self.timing.delay()
    }

    /// Returns the bounded spread percentage.
    #[must_use]
    pub const fn spread_value(&self) -> f32 {
        self.timing.spread()
    }

    /// Returns the retained semantic status label.
    #[must_use]
    pub fn semantic_status_label(&self) -> &str {
        self.semantic_label.as_ref()
    }

    /// Returns the retained label and product activity state.
    #[must_use]
    pub fn semantic_state(&self) -> ShimmerSemanticState {
        ShimmerSemanticState {
            label: self.semantic_label.clone(),
            active: self.active,
        }
    }

    /// Returns the policy/activity decision used by the renderer.
    #[must_use]
    pub fn motion_plan(&self) -> ShimmerMotionPlan {
        ShimmerMotionPlan::for_active_policy(self.motion, self.timing, self.active)
    }

    /// Sets the named color treatment.
    #[must_use]
    pub fn variant(mut self, variant: ShimmerTextVariant) -> Self {
        self.variant = variant;
        let foreground = self.visual_style().foreground;
        self.element = self.element.text_color(foreground);
        self
    }

    /// Alias for [`Self::variant`].
    #[must_use]
    pub fn color(self, color: ShimmerTextColor) -> Self {
        self.variant(color)
    }

    /// Sets whether the travelling band is active while retaining content.
    #[must_use]
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Sets the complete timing configuration.
    #[must_use]
    pub fn timing(mut self, timing: ShimmerTiming) -> Self {
        self.timing = timing;
        self
    }

    /// Sets the active duration.
    #[must_use]
    pub fn duration(self, duration: Duration) -> Self {
        let timing = self.timing.with_duration(duration);
        self.timing(timing)
    }

    /// Sets the active duration from seconds.
    #[must_use]
    pub fn duration_seconds(self, duration: f32) -> Self {
        let timing = self
            .timing
            .with_duration(seconds_to_duration(duration, MIN_ANIMATION_DURATION));
        self.timing(timing)
    }

    /// Sets the band delay.
    #[must_use]
    pub fn delay(self, delay: Duration) -> Self {
        let timing = self.timing.with_delay(delay);
        self.timing(timing)
    }

    /// Sets the band delay from seconds.
    #[must_use]
    pub fn delay_seconds(self, delay: f32) -> Self {
        let timing = self
            .timing
            .with_delay(seconds_to_duration(delay, Duration::ZERO));
        self.timing(timing)
    }

    /// Sets the bounded band spread percentage.
    #[must_use]
    pub fn spread(self, spread: f32) -> Self {
        let timing = self.timing.with_spread(spread);
        self.timing(timing)
    }

    /// Sets the explicit motion policy.
    #[must_use]
    pub fn motion_policy(mut self, motion: MotionPolicy) -> Self {
        self.motion = motion;
        self
    }

    /// Sets the retained semantic status label.
    #[must_use]
    pub fn semantic_label(mut self, label: impl Into<SharedString>) -> Self {
        self.semantic_label = label.into();
        self
    }

    /// Alias for [`Self::semantic_label`].
    #[must_use]
    pub fn status_label(self, label: impl Into<SharedString>) -> Self {
        self.semantic_label(label)
    }
}

impl Styled for ShimmerText {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        self.element.style()
    }
}

impl IntoElement for ShimmerText {
    type Element = AnyElement;

    fn into_element(self) -> Self::Element {
        let Self {
            element,
            content,
            theme,
            variant,
            timing,
            active,
            motion,
            ..
        } = self;
        let palette = variant.resolve(theme);
        let plan = ShimmerMotionPlan::for_active_policy(motion, timing, active);

        let text = match plan {
            ShimmerMotionPlan::Immediate => StyledText::new(content).into_any_element(),
            ShimmerMotionPlan::Animate(animation) => {
                let initial = styled_text_for_phase(
                    content.clone(),
                    palette,
                    animation.phase_for_progress(0.0),
                    timing.spread(),
                );
                initial
                    .with_animation(
                        SHIMMER_ANIMATION_ID,
                        animation.gpui_animation(),
                        move |_, progress| {
                            styled_text_for_phase(
                                content.clone(),
                                palette,
                                animation.phase_for_progress(progress),
                                timing.spread(),
                            )
                        },
                    )
                    .into_any_element()
            }
        };

        element.child(text).into_any_element()
    }
}

/// Returns a default-variant [`ShimmerText`] component.
#[must_use]
pub fn shimmer_text(
    content: impl Into<SharedString>,
    theme: ArtisanTheme,
    motion: MotionPolicy,
) -> ShimmerText {
    ShimmerText::new(content, theme, motion)
}

fn styled_text_for_phase(
    content: SharedString,
    palette: ShimmerTextStyle,
    phase: f32,
    spread: f32,
) -> StyledText {
    let highlights = highlighted_ranges(content.as_ref(), phase, spread)
        .into_iter()
        .map(|range| (range, HighlightStyle::from(palette.highlight)));
    StyledText::new(content).with_highlights(highlights)
}

fn phase_from_seconds(elapsed: f32, duration: f32, delay: f32) -> f32 {
    if duration <= 0.0 || elapsed <= delay {
        0.0
    } else {
        ((elapsed - delay) / duration).rem_euclid(1.0)
    }
}

fn normalize_phase(phase: f32) -> f32 {
    if phase.is_finite() {
        phase.rem_euclid(1.0)
    } else {
        0.0
    }
}

fn normalize_spread(spread: f32) -> f32 {
    if spread.is_finite() {
        spread.clamp(0.0, MAX_SPREAD)
    } else {
        0.0
    }
}

fn nonzero_duration(duration: Duration) -> Duration {
    if duration.is_zero() {
        MIN_ANIMATION_DURATION
    } else {
        duration
    }
}

fn seconds_to_duration(seconds: f32, zero_value: Duration) -> Duration {
    if seconds.is_nan() || seconds.is_sign_negative() {
        zero_value
    } else if seconds.is_infinite() {
        Duration::MAX
    } else {
        Duration::try_from_secs_f32(seconds).unwrap_or(Duration::MAX)
    }
}
