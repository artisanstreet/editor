//! The app's two hover treatments, shared so every surface matches.
//!
//! - **Hover fill**: what sits under a hovered row, cell, or bar — the
//!   rounded `hover_fill` gradient with a hairline inset highlight, exactly
//!   as the sliding pill in the pickers and sidebar paints it.
//! - **Glass hover card**: a floating card that appears on hover — the quiet
//!   glass material of the context window card, with its text hierarchy: a
//!   semibold title in the popover foreground, muted prose, and the facts in
//!   the prose lifted to the foreground.

use std::ops::Range;

use gpui::{
    BoxShadow, Div, FontWeight, HighlightStyle, Hsla, IntoElement, ParentElement, Pixels,
    SharedString, StyleRefinement, Styled, StyledText, div, point, px,
};

use crate::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::gradient::hover_fill_gradient;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};

/// Line height of hover card prose.
pub const HOVER_CARD_LINE_HEIGHT_PX: f32 = 19.0;

/// The hairline inset ring that edges the hover fill.
#[must_use]
pub fn hover_highlight_shadow(theme: &ArtisanTheme) -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: theme.colors.foreground.with_alpha(0.13).to_paint(),
        offset: point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(0.5),
        inset: true,
    }]
}

/// Paints the hover fill on an element that is hovered now.
#[must_use]
pub fn hover_fill<E: Styled>(element: E, theme: &ArtisanTheme, radius: Pixels) -> E {
    element
        .rounded(radius)
        .bg(hover_fill_gradient(*theme))
        .shadow(hover_highlight_shadow(theme))
}

/// Paints the hover fill inside a `.hover(…)` style refinement.
#[must_use]
pub fn hover_fill_style(
    style: StyleRefinement,
    theme: &ArtisanTheme,
    radius: Pixels,
) -> StyleRefinement {
    hover_fill(style, theme, radius)
}

/// The corner radius of a hover card.
#[must_use]
pub fn hover_card_radius() -> Pixels {
    RadiusTokens::value(RadiusStep::X2l)
}

/// A floating glass card around `content`. Size and position are the
/// caller's.
#[must_use]
pub fn glass_hover_card(theme: &ArtisanTheme, content: impl IntoElement) -> Div {
    glass_surface(theme, hover_card_radius(), content)
}

/// The hover card's quiet glass at any `radius`: pills and handles that
/// float over content share the card's material.
#[must_use]
pub fn glass_surface(theme: &ArtisanTheme, radius: Pixels, content: impl IntoElement) -> Div {
    div()
        .relative()
        .rounded(radius)
        .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
        .bg(glass_foreground_base(theme))
        .border_1()
        .border_color(theme.colors.border.to_paint())
        .shadow(glass_card_shadows())
        .font_family(theme.typography.sans.family)
        .child(glass_material_layer(GlassStrength::Quiet, radius))
        .child(glass_highlight_layer(GlassStrength::Quiet, radius))
        .child(
            div()
                .relative()
                .text_color(theme.colors.popover_foreground.to_paint())
                .child(content),
        )
}

/// The card title: semibold, control-sized, in the popover foreground.
#[must_use]
pub fn hover_card_title(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme.typography.control_text)
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// Muted prose with the `facts` byte ranges lifted to the foreground.
#[must_use]
pub fn hover_card_text(
    theme: &ArtisanTheme,
    text: impl Into<SharedString>,
    facts: &[Range<usize>],
) -> Div {
    let text = text.into();
    let foreground = HighlightStyle {
        color: Some(theme.colors.foreground.to_paint()),
        ..HighlightStyle::default()
    };
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = facts
        .iter()
        .filter(|range| range.start < range.end && range.end <= text.len())
        .filter(|range| text.is_char_boundary(range.start) && text.is_char_boundary(range.end))
        .cloned()
        .map(|range| (range, foreground))
        .collect();
    highlights.sort_by_key(|(range, _)| range.start);
    highlights.dedup_by(|later, earlier| later.0.start < earlier.0.end);
    div()
        .text_size(theme.typography.label_text)
        .line_height(px(HOVER_CARD_LINE_HEIGHT_PX))
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(StyledText::new(text).with_highlights(highlights))
}

/// One fact row: an optional colour key, a muted label, and the value in
/// the foreground.
#[must_use]
pub fn hover_card_row(
    theme: &ArtisanTheme,
    key: Option<Hsla>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.0))
        .whitespace_nowrap()
        .text_size(theme.typography.label_text)
        .line_height(px(HOVER_CARD_LINE_HEIGHT_PX))
        .children(key.map(|color| {
            div()
                .flex_shrink_0()
                .size(px(8.0))
                .rounded(px(4.0))
                .bg(color)
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(label.into()),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_color(theme.colors.foreground.to_paint())
                .child(value.into()),
        )
}
