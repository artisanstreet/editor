use super::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use aqua_core::Insets;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::Style,
};
pub struct Badge {
    theme: ArtisanTheme,
    label: String,
}
impl Badge {
    pub fn new(theme: ArtisanTheme, label: impl Into<String>) -> Self {
        Self {
            theme,
            label: label.into(),
        }
    }
    pub fn build(self) -> Element {
        let t = self.theme;
        let text = Text::new(self.label)
            .text_style(TextStyle {
                families: vec![FontFamily::Named(t.typography.sans.family.to_owned())],
                size_px: t.typography.label_text,
                line_height_px: t.spacing.steps(4.0),
                weight: FontWeight::MEDIUM,
                ..TextStyle::default()
            })
            .gpui_source_color(t.colors.text.to_reference_text_color())
            .build();
        UiBox::new()
            .height(t.density.badge_height)
            .radius(RadiusTokens::value(RadiusStep::X4l))
            .border((1.0, t.colors.line.to_paint()))
            .fill(t.colors.surface.to_paint())
            .clip(true)
            .style(
                Style::new()
                    .align(AlignItems::CENTER)
                    .justify(JustifyContent::CENTER)
                    .flex_shrink(0.0)
                    .padding_insets(Insets {
                        left: t.spacing.steps(2.0),
                        right: t.spacing.steps(2.0),
                        top: t.spacing.steps(0.5),
                        bottom: t.spacing.steps(0.5),
                    }),
            )
            .child(text)
            .build()
    }
}
