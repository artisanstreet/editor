use super::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use aqua_core::Insets;
use aqua_ui::{
    element::{Box as UiBox, Element},
    style::{Length, Style},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardStyle {
    pub corner_radius: f32,
    pub vertical_padding: f32,
    pub content_horizontal_padding: f32,
    pub vertical_gap: f32,
    pub text_size: f32,
    pub background: aqua_core::Oklch,
    pub foreground: aqua_core::Oklch,
    pub source_foreground: aqua_core::GpuiTextColor,
    pub ring_color: aqua_core::Oklch,
    pub ring_spread: f32,
}
impl CardStyle {
    pub fn resolve(theme: ArtisanTheme) -> Self {
        let spacing = theme.density.card_padding_compact;
        Self {
            corner_radius: RadiusTokens::value(RadiusStep::X2l),
            vertical_padding: spacing,
            content_horizontal_padding: spacing,
            vertical_gap: spacing,
            text_size: theme.typography.control_text,
            background: theme.colors.surface.to_paint(),
            foreground: theme.colors.text.to_paint(),
            source_foreground: theme.colors.text.to_reference_text_color(),
            ring_color: theme.colors.line.to_paint(),
            ring_spread: 1.0,
        }
    }
    pub fn ring(&self) -> aqua::ShadowLayer {
        aqua::ShadowLayer {
            dx: 0.0,
            dy: 0.0,
            blur: 0.0,
            spread: self.ring_spread,
            inset: false,
            color: self.ring_color,
        }
    }
}
pub fn compact_card(style: CardStyle) -> UiBox {
    UiBox::new()
        .layout("column")
        .gap(style.vertical_gap)
        .radius(style.corner_radius)
        .fill(style.background)
        .clip(true)
        .style(
            Style::new()
                .padding_insets(Insets {
                    top: style.vertical_padding,
                    bottom: style.vertical_padding,
                    left: 0.0,
                    right: 0.0,
                })
                .font_size(style.text_size)
                .gpui_source_text_color(style.source_foreground)
                .shadows(vec![style.ring()]),
        )
}
pub fn compact_card_content(style: CardStyle) -> UiBox {
    UiBox::new().style(Style::new().padding_insets(Insets {
        left: style.content_horizontal_padding,
        right: style.content_horizontal_padding,
        top: 0.0,
        bottom: 0.0,
    }))
}
pub struct Card {
    theme: ArtisanTheme,
    bands: Vec<Element>,
    divided: bool,
}
impl Card {
    pub fn new(theme: ArtisanTheme, content: Element) -> Self {
        Self {
            theme,
            bands: vec![content],
            divided: false,
        }
    }
    pub const fn bands(theme: ArtisanTheme, bands: Vec<Element>) -> Self {
        Self {
            theme,
            bands,
            divided: true,
        }
    }
    pub fn build(self) -> Element {
        let style = CardStyle::resolve(self.theme);
        let mut root = compact_card(style).width(Length::Pct(100.0)).gap(0.0);
        if self.divided {
            root = root.style(Style::new().padding_insets(Insets::default()));
        }
        for (index, band) in self.bands.into_iter().enumerate() {
            let mut row = compact_card_content(style)
                .width(Length::Pct(100.0))
                .child(band);
            if self.divided && index > 0 {
                row = row.style(Style::new().border_edges(
                    Insets {
                        top: 1.0,
                        ..Insets::default()
                    },
                    self.theme.colors.line.to_paint(),
                ));
            }
            root = root.child(row.build());
        }
        root.build()
    }
}
