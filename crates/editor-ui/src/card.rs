//! Compact cards own frame geometry; callers provide content or divided bands.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, App, BoxShadow, Div, Hsla, IntoElement, ParentElement as _, Pixels, RenderOnce,
    Styled as _, Window, div, point, px,
};

#[derive(IntoElement)]
pub struct Card {
    theme: ArtisanTheme,
    bands: Vec<AnyElement>,
    divided: bool,
}
impl Card {
    pub fn new(theme: ArtisanTheme, content: impl IntoElement) -> Self {
        Self {
            theme,
            bands: vec![content.into_any_element()],
            divided: false,
        }
    }
    pub fn bands(theme: ArtisanTheme, bands: Vec<AnyElement>) -> Self {
        Self {
            theme,
            bands,
            divided: true,
        }
    }
}
impl RenderOnce for Card {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let style = CardStyle::resolve(theme);
        let padding = style.content_horizontal_padding;
        let mut root = compact_card(style).w_full().py(px(0.0));
        if !self.divided {
            root = root.py(padding);
        }
        for (index, band) in self.bands.into_iter().enumerate() {
            let mut band = div().w_full().px(padding).child(band);
            if self.divided && index > 0 {
                band = band
                    .border_t_1()
                    .border_color(theme.colors.border.to_paint());
            }
            root = root.child(band);
        }
        root
    }
}

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
