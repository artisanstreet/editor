//! Native tooltip presentation and timing policy for reached surfaces.
//!
//! Ported from the audited legacy wrapper
//! (`modules/frontend/src/lib/components/ui/tooltip/tooltip-content.svelte`;
//! INVENTORY §2 row 32) at the configuration the selected workflow actually
//! reaches: the default inverted pill rendering plain text
//! (`<TooltipContent>{send_blocked_reason}</TooltipContent>` in
//! `composer/controls.svelte`) plus the wrapper's caret decision. The recipe
//! maps the legacy class string onto native values exactly like the
//! neighboring primitives resolve theirs through [`crate::theme::ArtisanTheme`]:
//! a fitted, centered flex row with a 6 px child gap (`gap-1.5`), 12 px
//! horizontal and 6 px vertical padding (`px-3 py-1.5`), the 18 px
//! `--radius-2xl` ramp step (`rounded-2xl`), 12 px label typography on its
//! 16 px leading (`text-xs`), the `max-w-xs` fitted-content cap (the pinned
//! `--container-xs`, 20 rem), and the inverted palette resolved per mode
//! (`bg-foreground text-background`).
//!
//! Timing is policy, not duplicated constants: each lifecycle phase maps to
//! the authoritative [`MotionRecipe::TooltipIn`] / [`MotionRecipe::TooltipOut`]
//! recipes and resolves through a caller-supplied [`MotionPolicy`], so reduced
//! motion collapses both phases to [`MotionPlan::Immediate`] instead of
//! animating or waiting.
//!
//! ## Presentation policy and pinned GPUI behavior
//!
//! GPUI's built-in `tooltip` / `hoverable_tooltip` machinery owns hover
//! tracking, dismissal, and viewport flip/clamp placement (`window.rs`
//! `prepaint_tooltip` / `draw_roots`; `GPUI_CAPABILITIES` §2.12/§2.17).
//! Pinned GPUI 0.2.2 hardcodes a 500 ms show delay and a 500 ms hoverable
//! hide grace; neither attachment method exposes a delay setting. The
//! audited composer requests `delayDuration={0}`, which those built-ins
//! cannot preserve. That compatibility gap requires explicit later overlay
//! integration, not a configuration option on this presentation recipe.
//! The motion plans here neither schedule nor override GPUI's timers;
//! reduced motion resolves this module's plan only.
//!
//! ## Known representation limits (pinned GPUI 0.2.2)
//!
//! - **There is no inline layout.** As with the badge primitive, Taffy routes
//!   every element through flexbox, so the legacy `inline-flex` participates
//!   as an ordinary flex item: the bubble hugs its content but occupies its
//!   own line rather than flowing inline with sibling text.
//! - **There is no literal z-index.** The legacy `z-50` is carried by GPUI's
//!   explicit overlay selection after ordinary roots. Prompt, drag, and
//!   tooltip overlays are mutually exclusive with that priority order;
//!   no numeric stacking axis exists to set.
//! - **Transform origin is deferred.** Bits derives
//!   `--bits-tooltip-content-transform-origin` from the floating side; natively
//!   it follows from the anchor corner chosen by the later overlay
//!   integration, which owns anchored positioning.
//! - **The entrance scale is not encoded.** The two audited motion sources
//!   disagree (`zoom-in-95` on the wrapper's content classes versus
//!   `--tt-scale:0.98` in the transitions.dev tokens) and the authoritative
//!   recipes carry no scale token, so none is invented here; callers may
//!   refine the returned element if a decision lands later.
//! - **The caret cannot join its anchor yet.** [`TooltipArrow`] records the
//!   audited geometry, fill sharing, and default-on decision (with the glass
//!   opt-out expressed as [`None`]), but the rotated square only means
//!   something once an anchored surface positions the bubble; its per-side
//!   translation classes are placement policy and are intentionally not
//!   fabricated in this boundary.

use gpui::{Div, Hsla, ParentElement, Pixels, SharedString, Styled, div, px};

use crate::motion::{MotionPlan, MotionPolicy, MotionRecipe};
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// Tailwind `max-w-xs`: the pinned `--container-xs` token, `20rem`, i.e.
/// 320 px at the 16 px root (`tailwindcss@4.3.2 theme.css`).
const MAX_WIDTH_PX: f32 = 320.0;

/// Which half of the tooltip lifecycle a caller presents.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TooltipPhase {
    /// Reveal the bubble once its trigger has earned it.
    Entrance,
    /// Dismiss the bubble.
    Exit,
}

impl TooltipPhase {
    /// The authoritative semantic motion recipe bound to this phase.
    #[must_use]
    pub const fn recipe(self) -> MotionRecipe {
        match self {
            Self::Entrance => MotionRecipe::TooltipIn,
            Self::Exit => MotionRecipe::TooltipOut,
        }
    }

    /// Resolves this phase under an explicit motion policy.
    ///
    /// Full motion runs the phase's recipe unchanged, including the
    /// entrance's separate product delay; reduced motion resolves to
    /// [`MotionPlan::Immediate`], collapsing both the animation and the wait
    /// rather than shortening them.
    #[must_use]
    pub const fn plan(self, policy: MotionPolicy) -> MotionPlan {
        policy.resolve(self.recipe())
    }
}

/// Audited caret geometry for one tooltip presentation.
///
/// The legacy caret is a rotated square painted in the same solid inverted
/// fill as the bubble (`bg-foreground`), so this record deliberately carries
/// no color of its own. That solid-only join is also why layered/glass
/// surfaces opt out entirely (`tooltip-content.svelte` doc comment;
/// `model-selector/option-tooltip.svelte` drops the caret for its
/// shader-glass material).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooltipArrow {
    /// Square edge before rotation: `size-2.5`, 10 px.
    pub edge: Pixels,
    /// Corner softening: `rounded-xs`, the 4 px ramp step.
    pub corner_radius: Pixels,
    /// Rotation turning the square into a point: `rotate-45`.
    pub rotation_degrees: f32,
}

impl TooltipArrow {
    /// The default caret reached by the first workflow.
    #[must_use]
    pub const fn legacy() -> Self {
        Self {
            edge: px(10.0),
            corner_radius: RadiusTokens::value(RadiusStep::Xs),
            rotation_degrees: 45.0,
        }
    }
}

/// Paint and geometry values resolved for one default tooltip bubble.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::button::ButtonStyle`], [`crate::badge::BadgeStyle`],
/// and [`crate::card::CardStyle`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TooltipStyle {
    /// Corner radius: `rounded-2xl`, the 18 px ramp step.
    pub corner_radius: Pixels,
    /// Horizontal padding: `px-3`, 12 px.
    pub horizontal_padding: Pixels,
    /// Vertical padding: `py-1.5`, 6 px.
    pub vertical_padding: Pixels,
    /// Flex-row gap between children: `gap-1.5`, 6 px.
    pub child_gap: Pixels,
    /// Label typography: Tailwind `text-xs`, 12 px.
    pub text_size: Pixels,
    /// Label line height carried by `text-xs`: 16 px.
    pub line_height: Pixels,
    /// Fitted-content width cap: `max-w-xs`, 20 rem at the 16 px root.
    pub max_width: Pixels,
    /// Inverted fill (`bg-foreground`) resolved for the theme mode.
    pub background: Hsla,
    /// Label color over the inverted fill (`text-background`).
    pub foreground: Hsla,
    /// Caret decision: present by default with the audited geometry;
    /// layered/glass materials pass [`None`] like the audited opt-out.
    pub arrow: Option<TooltipArrow>,
}

impl TooltipStyle {
    /// Resolves the exact reached default recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        Self {
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            horizontal_padding: theme.spacing.steps(3.0),
            vertical_padding: theme.spacing.steps(1.5),
            child_gap: theme.spacing.steps(1.5),
            text_size: theme.typography.label_text,
            line_height: theme.spacing.steps(4.0),
            max_width: px(MAX_WIDTH_PX),
            background: theme.colors.foreground.to_paint(),
            foreground: theme.colors.background.to_paint(),
            arrow: Some(TooltipArrow::legacy()),
        }
    }
}

/// Returns the default tooltip bubble as a plain GPUI [`Div`] owning its
/// visible plain-text label.
///
/// Mirrors the reached `<TooltipContent>{send_blocked_reason}</TooltipContent>`
/// call shape: a centered flex row with the audited paddings, gap, corner
/// radius, inverted palette, and 12 px typography on its 16 px leading,
/// hugging its content up to the 320 px cap. GPUI's built-in `.tooltip(...)`
/// / `.hoverable_tooltip(...)` attachments impose the fixed delays described
/// above; preserving the composer's zero-delay trigger requires later
/// overlay integration. Callers can chain further [`gpui::Styled`]
/// refinements (later values override the recipe defaults).
#[must_use]
pub fn tooltip_content(style: TooltipStyle, label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(style.child_gap)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .rounded(style.corner_radius)
        .max_w(style.max_width)
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .line_height(style.line_height)
        .child(label.into())
}
