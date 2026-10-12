//! Compact native GPUI card primitive for the reached conversation surfaces.
//!
//! Ported from the audited legacy wrapper
//! (`modules/frontend/src/lib/components/ui/card/card.svelte`; INVENTORY §2
//! row 7) at the single configuration the product actually reaches: the
//! compact `data-size="sm"` Card/CardContent pair used by
//! `conversation-change` and `conversation-prompt`. The legacy root classes
//! `ring-foreground/10 bg-card text-card-foreground gap-(--card-spacing)
//! overflow-hidden rounded-2xl py-(--card-spacing) text-sm ring-1
//! [--card-spacing:--spacing(6)] … data-[size=sm]:[--card-spacing:--spacing(4)]
//! flex flex-col` therefore map onto exactly one recipe here: flex column,
//! 16 px gap, 16 px default vertical padding, an 18 px `--radius-2xl`
//! silhouette, mode-resolved `card`/`card-foreground` colors, 14 px shared
//! control typography, rectangular `overflow_hidden`, and the legacy 1 px
//! `ring-foreground/10` hairline carried as one zero-blur spread shadow.
//!
//! Deliberately absent because no reached call site uses them: the default
//! 24 px spacing step, the `Header`/`Title`/`Description`/`Footer`/`Action`
//! slots, a size enum, destructive/color variants, prose measure widths, and
//! the legacy first-image corner special cases. Callers resolve
//! [`CardStyle::resolve`] once for their mode and pass that one recipe value
//! to both [`compact_card`] and [`compact_card_content`], so the root and its
//! content band always share a single audited spacing step; further
//! [`gpui::Styled`] refinements chain onto the returned [`Div`]s and later
//! values win, keeping the audited tighter vertical paddings reachable
//! without new APIs.
//!
//! ## Known rendering limitations (pinned GPUI 0.2.2)
//!
//! - **Rounded clipping is not honored for children.** `overflow_hidden`
//!   installs a rectangular [`gpui::ContentMask`] built purely from the
//!   element bounds (`gpui-0.2.2/src/style.rs:552–589`,
//!   `overflow_mask`; applied at `elements/div.rs:1674/1825`). Corner radii
//!   round the painted background quad and the ring shadow, but children are
//!   clipped to the straight-edged rectangle, so content reaching the corners
//!   can overpaint the transparent 18 px corner area. Every reached call site
//!   keeps content inset, and the geometry tests below pin the insets that
//!   guarantee it; a rounded clip would need first-party masking work.
//! - **The legacy `ring-1` is a shadow, not a border.** GPUI shrinks the
//!   overflow clip rectangle inward by any opaque border widths
//!   (`style.rs:568–576`) and borders consume paint inside the bounds, while
//!   the CSS ring floats over the edge without affecting layout. The recipe
//!   reproduces the ring as one `BoxShadow` with zero offset, zero blur, and
//!   a 1 px spread in `foreground` at 10% alpha: layout-neutral, outside the
//!   clip computation, and following the rounded silhouette like the legacy
//!   ring did.

use gpui::{BoxShadow, Div, Hsla, Pixels, Styled, div, point, px};

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// Legacy `ring-foreground/10`: the shared foreground at 10% alpha
/// (`card.svelte` `ring-foreground/10`).
const RING_ALPHA: f32 = 0.10;

/// Legacy `ring-1`: a one-pixel hairline around the card.
const RING_WIDTH_PX: f32 = 1.0;

/// Paint and geometry values resolved for one compact card.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::button::ButtonStyle`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardStyle {
    /// Rounded-corner radius: the legacy `rounded-2xl` ramp step, 18 px.
    pub corner_radius: Pixels,
    /// Default vertical padding: the compact `--card-spacing` token, 16 px.
    pub vertical_padding: Pixels,
    /// Horizontal content-band padding: the same compact token, 16 px.
    pub content_horizontal_padding: Pixels,
    /// Flex-column gap between children: the same compact token, 16 px.
    pub vertical_gap: Pixels,
    /// Shared control typography: Tailwind `text-sm`, 14 px.
    pub text_size: Pixels,
    /// Card background (`--card`) resolved for the theme mode.
    pub background: Hsla,
    /// Card foreground text color (`--card-foreground`) for the theme mode.
    pub foreground: Hsla,
    /// Ring hairline color: the shared `--foreground` at 10% alpha.
    pub ring_color: Hsla,
    /// Ring hairline thickness, painted as shadow spread: 1 px.
    pub ring_spread: Pixels,
}

impl CardStyle {
    /// Resolves the exact reached compact recipe from shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let compact_spacing = theme.density.card_padding_compact;
        Self {
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            vertical_padding: compact_spacing,
            content_horizontal_padding: compact_spacing,
            vertical_gap: compact_spacing,
            text_size: theme.typography.control_text,
            background: theme.colors.card.to_paint(),
            foreground: theme.colors.card_foreground.to_paint(),
            ring_color: theme.colors.foreground.with_alpha(RING_ALPHA).to_paint(),
            ring_spread: px(RING_WIDTH_PX),
        }
    }

    /// Builds the legacy `ring-foreground/10` hairline as one layout-neutral
    /// zero-blur spread shadow (see the module-level limitation notes).
    #[must_use]
    pub fn ring(&self) -> BoxShadow {
        BoxShadow {
            color: self.ring_color,
            offset: point(px(0.0), px(0.0)),
            blur_radius: px(0.0),
            spread_radius: self.ring_spread,
            inset: false,
        }
    }
}

/// Returns the compact card root as a plain GPUI [`Div`].
///
/// The root consumes the root fields of the caller-resolved recipe: flex
/// column, gap, vertical padding, corner radius, colors, typography, and the
/// ring shadow. Callers add children and chain further [`gpui::Styled`]
/// refinements (later values override the recipe defaults).
#[must_use]
pub fn compact_card(style: CardStyle) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(style.vertical_gap)
        .py(style.vertical_padding)
        .rounded(style.corner_radius)
        .overflow_hidden()
        .bg(style.background)
        .text_color(style.foreground)
        .text_size(style.text_size)
        .shadow(vec![style.ring()])
}

/// Returns the compact card content band as a plain GPUI [`Div`].
///
/// The band consumes only [`CardStyle::content_horizontal_padding`] from the
/// same caller-resolved recipe (the legacy `px-(--card-spacing)`); vertical
/// rhythm belongs to the card root's gap.
#[must_use]
pub fn compact_card_content(style: CardStyle) -> Div {
    div().px(style.content_horizontal_padding)
}
