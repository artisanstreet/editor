use super::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems,
    element::{Box as UiBox, Element, Text},
    style::Style,
};
pub struct Keycap {
    theme: ArtisanTheme,
    key: String,
}
impl Keycap {
    pub fn new(theme: ArtisanTheme, key: impl Into<String>) -> Self {
        Self {
            theme,
            key: key.into(),
        }
    }
    pub fn build(self) -> Element {
        let t = self.theme;
        let text = Text::new(self.key)
            .text_style(TextStyle {
                families: vec![FontFamily::Named(t.typography.sans.family.to_owned())],
                size_px: TypographyTokens::size(TextStep::X2s),
                line_height_px: 16.0,
                weight: FontWeight::MEDIUM,
                ..TextStyle::default()
            })
            .gpui_source_color(t.colors.text_muted.to_reference_text_color())
            .build();
        UiBox::new()
            .height(18.0)
            .radius(RadiusTokens::value(RadiusStep::Xs))
            .border((1.0, t.colors.line.to_paint()))
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .flex_shrink(0.0)
                    .padding_insets(Insets {
                        left: 5.0,
                        right: 5.0,
                        top: 0.0,
                        bottom: 0.0,
                    }),
            )
            .child(text)
            .build()
    }
}
