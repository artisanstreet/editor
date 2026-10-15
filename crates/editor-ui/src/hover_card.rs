//! Shared glass frames and hover treatments. Placement belongs to the caller.

use gpui::prelude::FluentBuilder as _;
use std::ops::Range;

use gpui::{
    AnyElement, App, BoxShadow, Div, FontWeight, HighlightStyle, Hsla, IntoElement, ParentElement,
    Pixels, RenderOnce, SharedString, StyleRefinement, Styled, StyledText, Window, div, point, px,
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
pub(crate) fn hover_highlight_shadow(theme: &ArtisanTheme) -> Vec<BoxShadow> {
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
fn hover_fill<E: Styled>(element: E, theme: &ArtisanTheme, radius: Pixels) -> E {
    element
        .rounded(radius)
        .bg(hover_fill_gradient(*theme))
        .shadow(hover_highlight_shadow(theme))
}

/// Paints the hover fill inside a `.hover(…)` style refinement.
#[must_use]
pub(crate) fn hover_fill_style(
    style: StyleRefinement,
    theme: &ArtisanTheme,
    radius: Pixels,
) -> StyleRefinement {
    hover_fill(style, theme, radius)
}

enum GlassShape {
    Card,
    Navigator,
    Pill,
    Handle,
}

/// A glass frame with component-owned material and corner geometry.
#[derive(IntoElement)]
pub struct GlassCard {
    theme: ArtisanTheme,
    content: AnyElement,
    shape: GlassShape,
}
impl RenderOnce for GlassCard {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let (radius, fill) = match self.shape {
            GlassShape::Card => (RadiusTokens::value(RadiusStep::X2l), false),
            GlassShape::Navigator => (RadiusTokens::value(RadiusStep::Xl), false),
            GlassShape::Pill => (px(11.0), true),
            GlassShape::Handle => (px(16.0), true),
        };
        let strength = if matches!(self.shape, GlassShape::Navigator) {
            GlassStrength::Strong
        } else {
            GlassStrength::Quiet
        };
        let frame = glass_surface(&self.theme, radius, self.content, fill, strength);
        match self.shape {
            GlassShape::Card | GlassShape::Navigator => frame,
            GlassShape::Pill => frame.h(px(22.0)),
            GlassShape::Handle => frame.size(px(32.0)),
        }
    }
}

/// A floating glass card. Place and size it with an outer layout element.
#[must_use]
pub fn glass_hover_card(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Card,
    }
}

/// The transcript navigator's original 14px strong-glass face.
pub fn glass_navigator_card(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Navigator,
    }
}

/// A compact label over media, with one height and radius in every state.
#[must_use]
pub fn glass_pill(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Pill,
    }
}

/// A circular 32 px handle over media.
#[must_use]
pub fn glass_handle(theme: &ArtisanTheme, content: impl IntoElement) -> GlassCard {
    GlassCard {
        theme: *theme,
        content: content.into_any_element(),
        shape: GlassShape::Handle,
    }
}

fn glass_surface(
    theme: &ArtisanTheme,
    radius: Pixels,
    content: impl IntoElement,
    fill: bool,
    strength: GlassStrength,
) -> Div {
    div()
        .relative()
        .rounded(radius)
        .backdrop_blur(glass_blur_radius(strength))
        .bg(glass_foreground_base(theme))
        .border_1()
        .border_color(theme.colors.border.to_paint())
        .shadow(glass_card_shadows())
        .font_family(theme.typography.sans.family)
        .child(glass_material_layer(strength, radius))
        .child(glass_highlight_layer(strength, radius))
        .child(
            div()
                .relative()
                .when(fill, |element| element.size_full())
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

/// A lazily-created native tooltip view. GPUI owns delay, placement and dismissal.
pub fn tooltip(
    content: impl Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement + 'static,
    cx: &mut gpui::App,
) -> gpui::AnyView {
    use gpui::AppContext as _;
    cx.new(|_| Tooltip {
        content: Box::new(content),
    })
    .into()
}
struct Tooltip {
    content: Box<dyn Fn(&mut gpui::Window, &mut gpui::App) -> gpui::AnyElement>,
}
impl gpui::Render for Tooltip {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        (self.content)(window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, InteractiveElement as _, Render, TestAppContext};

    struct GlassGallery;
    impl Render for GlassGallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = ArtisanTheme::default();
            div()
                .flex()
                .items_start()
                .gap(px(8.0))
                .child(
                    div()
                        .debug_selector(|| "glass-handle-test".to_owned())
                        .child(glass_handle(&theme, div().size_full())),
                )
                .child(
                    div()
                        .w(px(80.0))
                        .debug_selector(|| "glass-pill-test".to_owned())
                        .child(glass_pill(&theme, div().size_full().child("Before"))),
                )
        }
    }

    #[gpui::test]
    fn media_glass_recipes_own_the_complete_outer_geometry(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| GlassGallery);
        let handle = cx.debug_bounds("glass-handle-test").expect("handle");
        let pill = cx.debug_bounds("glass-pill-test").expect("pill");
        assert_eq!(handle.size, gpui::size(px(32.0), px(32.0)));
        assert_eq!(pill.size, gpui::size(px(80.0), px(22.0)));
    }
}
