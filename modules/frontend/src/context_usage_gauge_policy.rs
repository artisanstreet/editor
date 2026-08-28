//! Dependency-free presentation policy for the context-window gauge.
//!
//! This is the native counterpart of
//! `routes/components/context-usage-gauge.svelte`. The caller supplies the
//! already-computed screen-reader description, percentage, compaction
//! percentage, optional reporting-model name, and window capacity. This leaf
//! only assembles the values and presentation facts consumed by a later
//! renderer; it does not calculate usage, resolve context descriptions, or
//! render a control or floating surface.
//!
//! The gauge is an independent sibling control beside the model picker. Its
//! ring and details children receive separate adapter inputs so neither child
//! has to know about the other or recompute a context fact. The screen-reader
//! description is represented as a persistent, visually-hidden node because
//! the visual hover card is mounted only while open.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::time::Duration;

/// The stable id shared by the trigger's `aria-describedby` and the
/// persistent screen-reader description.
pub const CONTEXT_USAGE_DESCRIPTION_ID: &str = "context-usage-details";

/// The exact hover-card width utility reached by the Svelte component.
pub const CONTEXT_USAGE_WIDTH_CLASS: &str = "w-72";

/// The exact viewport-aware max-width utility reached by the Svelte
/// component.
pub const CONTEXT_USAGE_MAX_WIDTH_CLASS: &str = "max-w-[min(20rem,calc(100vw-2rem))]";

/// The hover-card delay before opening.
pub const CONTEXT_USAGE_OPEN_DELAY: Duration = Duration::ZERO;

/// The hover-card delay before closing.
pub const CONTEXT_USAGE_CLOSE_DELAY: Duration = Duration::from_millis(120);

/// The hover-card separation from its trigger in logical pixels.
pub const CONTEXT_USAGE_SIDE_OFFSET_PX: u16 = 8;

/// The side on which the gauge's hover card prefers to appear.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HoverCardSide {
    /// Place the card above the trigger.
    Top,
    /// Place the card to the right of the trigger.
    Right,
    /// Place the card below the trigger.
    Bottom,
    /// Place the card to the left of the trigger.
    Left,
}

/// The alignment of hover-card content along the trigger's non-side axis.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HoverCardAlign {
    /// Align the card's leading edge with the trigger's leading edge.
    Start,
    /// Center the card on the trigger.
    Center,
    /// Align the card's trailing edge with the trigger's trailing edge.
    End,
}

/// Typed preferred placement for the context gauge hover card.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HoverCardPlacement {
    /// Preferred side before any host collision handling.
    pub side: HoverCardSide,
    /// Alignment along the preferred side.
    pub align: HoverCardAlign,
    /// Separation from the trigger on the side axis, in logical pixels.
    pub side_offset_px: u16,
}

impl HoverCardPlacement {
    /// The placement reached by `side="top" align="start" sideOffset={8}`.
    pub const CONTEXT_USAGE: Self = Self {
        side: HoverCardSide::Top,
        align: HoverCardAlign::Start,
        side_offset_px: CONTEXT_USAGE_SIDE_OFFSET_PX,
    };

    /// Creates a preferred placement without collision resolution.
    #[must_use]
    pub const fn new(side: HoverCardSide, align: HoverCardAlign, side_offset_px: u16) -> Self {
        Self {
            side,
            align,
            side_offset_px,
        }
    }
}

/// The fixed width utility's rem-based intent.
///
/// `w-72` is an 18 rem Tailwind width token. The token, rather than a
/// hard-coded pixel conversion, is retained because the legacy component
/// expresses this width in rems.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HoverCardWidthIntent {
    /// The preferred width in rem units.
    pub preferred_rem: u16,
}

impl HoverCardWidthIntent {
    /// The width intent reached by the context usage card.
    pub const CONTEXT_USAGE: Self = Self { preferred_rem: 18 };

    /// Returns the exact utility class represented by this intent.
    #[must_use]
    pub const fn utility_class(self) -> &'static str {
        CONTEXT_USAGE_WIDTH_CLASS
    }
}

/// The viewport-aware max-width intent from the legacy utility class.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HoverCardMaxWidthIntent {
    /// The maximum width cap in rem units.
    pub maximum_rem: u16,
    /// The total horizontal viewport inset in rem units.
    pub viewport_inset_rem: u16,
}

impl HoverCardMaxWidthIntent {
    /// The max-width intent reached by the context usage card.
    pub const CONTEXT_USAGE: Self = Self {
        maximum_rem: 20,
        viewport_inset_rem: 2,
    };

    /// Returns the exact utility class represented by this intent.
    #[must_use]
    pub const fn utility_class(self) -> &'static str {
        CONTEXT_USAGE_MAX_WIDTH_CLASS
    }
}

/// Material used by the visual hover-card content.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HoverCardMaterial {
    /// The shared `ShaderGlassSurface` material.
    ShaderGlassSurface,
}

impl HoverCardMaterial {
    /// Returns whether this is the shared glass-surface material.
    #[must_use]
    pub const fn is_glass_surface(self) -> bool {
        matches!(self, Self::ShaderGlassSurface)
    }
}

/// Complete typed presentation facts for the gauge's hover card.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HoverCardPresentation {
    /// Delay before the card opens.
    pub open_delay: Duration,
    /// Delay before the card closes.
    pub close_delay: Duration,
    /// Preferred side, alignment, and trigger separation.
    pub placement: HoverCardPlacement,
    /// Preferred fixed width intent (`w-72`).
    pub width: HoverCardWidthIntent,
    /// Viewport-aware max-width intent.
    pub max_width: HoverCardMaxWidthIntent,
    /// Shared material used inside the transparent content shell.
    pub material: HoverCardMaterial,
}

impl HoverCardPresentation {
    /// The exact presentation recipe reached by the context gauge.
    pub const CONTEXT_USAGE: Self = Self {
        open_delay: CONTEXT_USAGE_OPEN_DELAY,
        close_delay: CONTEXT_USAGE_CLOSE_DELAY,
        placement: HoverCardPlacement::CONTEXT_USAGE,
        width: HoverCardWidthIntent::CONTEXT_USAGE,
        max_width: HoverCardMaxWidthIntent::CONTEXT_USAGE,
        material: HoverCardMaterial::ShaderGlassSurface,
    };
}

impl Default for HoverCardPresentation {
    fn default() -> Self {
        Self::CONTEXT_USAGE
    }
}

/// Ownership relationship between the gauge and the neighboring model
/// picker.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum GaugeControlOwnership {
    /// The gauge owns its own control and is a sibling of the picker control.
    IndependentSibling,
}

impl GaugeControlOwnership {
    /// Returns whether the gauge can own its hover interaction independently.
    #[must_use]
    pub const fn is_independent_sibling(self) -> bool {
        matches!(self, Self::IndependentSibling)
    }
}

/// The already-computed values the eventual ring adapter consumes.
///
/// Values are deliberately copied without clamping or tone calculation. The
/// context-usage tone and percentage policies own those decisions upstream.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextUsageRingInput {
    /// Engine-defined compaction threshold percentage.
    pub compaction_percent: f64,
    /// Already-computed context-window fullness percentage.
    pub percent: f64,
}

/// The already-computed values the eventual details adapter consumes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextUsageDetailsInput<'a> {
    /// Reporting model name, preserving `None` and supplied empty names.
    pub model_name: Option<&'a str>,
    /// Already-computed context-window fullness percentage.
    pub percent: f64,
    /// Context-window capacity in tokens.
    pub window_tokens: f64,
}

/// The persistent screen-reader description mounted beside the trigger.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenReaderDescription<'a> {
    /// Stable id referenced by the trigger's `aria-describedby` fact.
    pub id: &'static str,
    /// Caller-computed description text, retained without rewriting.
    pub text: &'a str,
    /// Whether the description remains mounted while the visual card is
    /// closed.
    pub always_present: bool,
    /// Whether the description is intended for screen readers rather than
    /// visual display (`sr-only`).
    pub visually_hidden: bool,
}

/// Accessibility and ownership facts for the gauge's trigger.
///
/// This describes a control's semantics without representing a button
/// element. A later renderer owns the actual control construction.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextUsageGaugeTrigger {
    /// Exact accessible label, including the rounded raw percentage.
    pub aria_label: String,
    /// Raw percentage after JavaScript `Math.round` semantics, before any
    /// visual fill clamping owned by another policy.
    pub label_percent: f64,
    /// Stable id for the persistent screen-reader description.
    pub aria_described_by: &'static str,
    /// Independent sibling-control ownership fact.
    pub ownership: GaugeControlOwnership,
}

/// Inputs required at this projection boundary.
///
/// `description` is intentionally borrowed and already computed. This type
/// does not accept a usage aggregate, prompt tokens, or model policy because
/// those facts belong to adjacent context modules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ContextUsageGaugeInput<'a> {
    /// Caller-computed accessible context-window description.
    pub description: &'a str,
    /// Already-computed context-window fullness percentage.
    pub percent: f64,
    /// Already-computed engine compaction threshold percentage.
    pub compaction_percent: f64,
    /// Optional reporting model name to forward to the details adapter.
    pub model_name: Option<&'a str>,
    /// Context-window capacity to forward to the details adapter.
    pub window_tokens: f64,
}

impl<'a> ContextUsageGaugeInput<'a> {
    /// Creates a complete gauge projection input without deriving any
    /// context-usage value.
    #[must_use]
    pub const fn new(
        description: &'a str,
        percent: f64,
        compaction_percent: f64,
        model_name: Option<&'a str>,
        window_tokens: f64,
    ) -> Self {
        Self {
            description,
            percent,
            compaction_percent,
            model_name,
            window_tokens,
        }
    }
}

/// Complete deterministic projection for one context usage gauge.
#[derive(Clone, Debug, PartialEq)]
pub struct ContextUsageGaugeProjection<'a> {
    /// Trigger semantics and the rounded accessible label.
    pub trigger: ContextUsageGaugeTrigger,
    /// Persistent screen-reader-only description.
    pub description: ScreenReaderDescription<'a>,
    /// Exact input pair for the ring adapter.
    pub ring: ContextUsageRingInput,
    /// Exact input tuple for the details adapter.
    pub details: ContextUsageDetailsInput<'a>,
    /// Typed hover-card timing, placement, sizing, and material facts.
    pub hover_card: HoverCardPresentation,
}

/// Projects the deterministic context gauge without rendering UI.
///
/// The input values are forwarded to their respective adapters unchanged.
/// The only derived value is the trigger's accessible label, which mirrors
/// ``aria-label={`Context window ${Math.round(percent)}% full`}`` from the
/// Svelte component. The persistent description intentionally has no visual
/// card-open condition.
#[must_use]
pub fn project_context_usage_gauge(
    input: ContextUsageGaugeInput<'_>,
) -> ContextUsageGaugeProjection<'_> {
    let label_percent = round_like_javascript_math_round(input.percent);

    ContextUsageGaugeProjection {
        trigger: ContextUsageGaugeTrigger {
            aria_label: format!(
                "Context window {}% full",
                format_rounded_percent(label_percent)
            ),
            label_percent,
            aria_described_by: CONTEXT_USAGE_DESCRIPTION_ID,
            ownership: GaugeControlOwnership::IndependentSibling,
        },
        description: ScreenReaderDescription {
            id: CONTEXT_USAGE_DESCRIPTION_ID,
            text: input.description,
            always_present: true,
            visually_hidden: true,
        },
        ring: ContextUsageRingInput {
            compaction_percent: input.compaction_percent,
            percent: input.percent,
        },
        details: ContextUsageDetailsInput {
            model_name: input.model_name,
            percent: input.percent,
            window_tokens: input.window_tokens,
        },
        hover_card: HoverCardPresentation::CONTEXT_USAGE,
    }
}

/// Alias naming the projection after the component's context-gauge role.
#[must_use]
pub fn context_usage_gauge(input: ContextUsageGaugeInput<'_>) -> ContextUsageGaugeProjection<'_> {
    project_context_usage_gauge(input)
}

/// Alias naming the projection boundary explicitly for native callers.
#[must_use]
pub fn context_usage_gauge_projection(
    input: ContextUsageGaugeInput<'_>,
) -> ContextUsageGaugeProjection<'_> {
    project_context_usage_gauge(input)
}

/// Rounds a percentage with JavaScript `Math.round` semantics.
///
/// The legacy caller supplies finite percentages. Non-finite values are
/// retained as non-finite label facts so this boundary never panics or turns
/// them into a plausible usage reading. String formatting below uses the
/// JavaScript spellings for those values.
#[must_use]
pub fn round_like_javascript_math_round(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }

    let lower = value.floor();
    if value - lower >= 0.5 {
        lower + 1.0
    } else {
        lower
    }
}

fn format_rounded_percent(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value == f64::INFINITY {
        return "Infinity".to_owned();
    }
    if value == f64::NEG_INFINITY {
        return "-Infinity".to_owned();
    }
    if value == 0.0 {
        // JavaScript string interpolation renders both signed zero values as
        // `0`, including Math.round(-0.5).
        return "0".to_owned();
    }

    format!("{value:.0}")
}
