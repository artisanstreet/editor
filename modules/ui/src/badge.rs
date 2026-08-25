//! Noninteractive plain-text outline badge primitive for reached surfaces.
//!
//! Ported from the audited legacy wrapper
//! (`modules/frontend/src/lib/components/ui/badge/badge.svelte`; INVENTORY §2
//! row 5) at the single configuration the product actually reaches: the
//! noninteractive `variant="outline"` plain-text badge rendered by
//! `conversation-change` and `conversation-prompt`. The composed legacy class
//! set therefore maps onto exactly one recipe here: a fixed 20 px (`h-5`)
//! pill at the 26 px `--radius-4xl` ramp step, 8 px horizontal and 2 px
//! vertical padding (`px-2 py-0.5`), a 4 px child gap (`gap-1`), 12 px
//! medium text (`text-xs font-medium`) on the 16 px leading `text-xs`
//! carries, one line (`whitespace-nowrap`) clipped by `overflow-hidden`, a
//! fitted, non-growing, non-shrinking, centered flex layout (`inline-flex
//! w-fit shrink-0 items-center justify-center`), and the outline palette
//! resolved per mode: `border-border text-foreground` over `bg-surface-100`
//! in light presentation and `bg-surface-900` in dark.
//!
//! Deliberately absent because no reached call site uses them: every other
//! legacy variant (`default`/`secondary`/`destructive`/`ghost`/`link`), the
//! anchor/`href` link behavior, hover/focus/`aria-invalid` treatments, icon
//! slots, transitions, and wrapper-class strings. Callers resolve
//! [`BadgeStyle::resolve`] once for their mode and pass that one recipe value
//! together with the visible plain-text label to [`outline_badge`]; further
//! [`gpui::Styled`] refinements chain onto the returned [`Div`] and later
//! values win, keeping audited caller-side adjustments reachable without new
//! APIs.
//!
//! ## Known rendering limitations (pinned GPUI 0.2.2)
//!
//! - **There is no inline layout.** GPUI routes every element through Taffy
//!   flexbox, so the legacy `inline-flex` participates as an ordinary flex
//!   item: the badge still hugs its content and refuses to grow or shrink,
//!   but it occupies its own flex line rather than flowing inline with
//!   sibling text. Both reached call sites place the badge as the first
//!   child of an `items-center` row, where the two layouts coincide.
//! - **Rounded clipping is not honored for children.** `overflow_hidden`
//!   installs a rectangular [`gpui::ContentMask`] built purely from the
//!   element bounds (same engine behavior documented for the card
//!   primitive), so painted clipping honors the 20 px edges but not the
//!   26 px corners. Reached plain-text labels stay far inside the corners.

use gpui::{Div, FontWeight, Hsla, ParentElement, Pixels, SharedString, Styled, div};

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

/// Paint and geometry values resolved for one outline badge.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::button::ButtonStyle`] and [`crate::card::CardStyle`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BadgeStyle {
    /// Fixed control height: the legacy `h-5`, 20 px.
    pub height: Pixels,
    /// Horizontal padding: the legacy `px-2`, 8 px.
    pub horizontal_padding: Pixels,
    /// Vertical padding: the legacy `py-0.5`, 2 px.
    pub vertical_padding: Pixels,
    /// Flex-row gap between children: the legacy `gap-1`, 4 px.
    pub child_gap: Pixels,
    /// Rounded-pill radius: the legacy `rounded-4xl` ramp step, 26 px.
    pub corner_radius: Pixels,
    /// Label typography: Tailwind `text-xs`, 12 px.
    pub text_size: Pixels,
    /// Label line height: the leading `text-xs` carries, 16 px.
    pub line_height: Pixels,
    /// Badge background resolved for the theme mode: surface 100 in light
    /// presentation, surface 900 in dark.
    pub background: Hsla,
    /// Label color (`--foreground`) for the theme mode.
    pub foreground: Hsla,
    /// Hairline border color (`--border`) for the theme mode.
    pub border: Hsla,
}

impl BadgeStyle {
    /// Resolves the exact reached outline recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let surface = match theme.mode {
            ThemeMode::Light => SurfaceStep::S100,
            ThemeMode::Dark => SurfaceStep::S900,
        };

        Self {
            height: theme.density.badge_height,
            horizontal_padding: theme.spacing.steps(2.0),
            vertical_padding: theme.spacing.steps(0.5),
            child_gap: theme.spacing.steps(1.0),
            corner_radius: RadiusTokens::value(RadiusStep::X4l),
            text_size: theme.typography.label_text,
            line_height: theme.spacing.steps(4.0),
            background: theme.surfaces.value(surface).to_paint(),
            foreground: theme.colors.foreground.to_paint(),
            border: theme.colors.border.to_paint(),
        }
    }
}

/// Returns the outline badge as a plain GPUI [`Div`] owning its visible
/// plain-text label.
///
/// The element consumes the whole caller-resolved recipe: fixed height,
/// paddings, child gap, 26 px pill radius, one-pixel `--border` hairline,
/// mode-resolved surface background and `--foreground` label color, 12 px
/// medium text on a 16 px line, single-line no-wrap flow, and clipped
/// overflow. The layout is fitted, non-growing, and non-shrinking with
/// centered contents. Callers add further [`gpui::Styled`] refinements
/// (later values override the recipe defaults).
#[must_use]
pub fn outline_badge(style: BadgeStyle, label: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_row()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .h(style.height)
        .px(style.horizontal_padding)
        .py(style.vertical_padding)
        .gap(style.child_gap)
        .rounded(style.corner_radius)
        .border_1()
        .border_color(style.border)
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .font_weight(FontWeight::MEDIUM)
        .line_height(style.line_height)
        .whitespace_nowrap()
        .overflow_hidden()
        .child(label.into())
}
