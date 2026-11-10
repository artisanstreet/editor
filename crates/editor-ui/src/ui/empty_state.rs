use super::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use aqua_core::Insets;
use aqua_ui::{
    AlignItems, JustifyContent,
    element::{Box as UiBox, Element, Text},
    style::{Length, Style},
};
#[derive(Clone, Copy, Default)]
pub enum EmptyStateVariant {
    #[default]
    Inline,
    Framed,
}
pub struct EmptyState {
    theme: ArtisanTheme,
    text: String,
    variant: EmptyStateVariant,
}
impl EmptyState {
    pub fn new(theme: ArtisanTheme, text: impl Into<String>) -> Self {
        Self {
            theme,
            text: text.into(),
            variant: EmptyStateVariant::Inline,
        }
    }
    pub const fn variant(mut self, variant: EmptyStateVariant) -> Self {
        self.variant = variant;
        self
    }
    pub fn build(self) -> Element {
        let mut root = UiBox::new()
            .width(Length::Pct(100.0))
            .style(
                Style::new()
                    .min_width(Length::Px(0.0))
                    .padding_insets(Insets {
                        top: 20.0,
                        bottom: 20.0,
                        left: 12.0,
                        right: 12.0,
                    }),
            )
            .child(
                Text::new(self.text)
                    .font_size(14.0)
                    .gpui_source_color(self.theme.colors.text_muted.to_reference_text_color())
                    .wrap(true)
                    .build(),
            );
        if matches!(self.variant, EmptyStateVariant::Framed) {
            root = root
                .style(
                    Style::new()
                        .min_height(Length::Px(72.0))
                        .align(AlignItems::CENTER)
                        .justify(JustifyContent::CENTER),
                )
                .radius(RadiusTokens::value(RadiusStep::Lg))
                .border((1.0, self.theme.colors.line.to_paint()));
        }
        root.build()
    }
}
