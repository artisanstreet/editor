//! A key as printed on the keyboard, beside the hint it triggers.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use gpui::{
    App, FontWeight, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div, px,
};

#[derive(IntoElement)]
pub struct Keycap {
    theme: ArtisanTheme,
    key: SharedString,
}

impl Keycap {
    pub fn new(theme: ArtisanTheme, key: impl Into<SharedString>) -> Self {
        Self {
            theme,
            key: key.into(),
        }
    }
}

impl RenderOnce for Keycap {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(18.0))
            .px(px(5.0))
            .rounded(RadiusTokens::value(RadiusStep::Xs))
            .border_1()
            .border_color(colors.line.to_paint())
            .text_size(TypographyTokens::size(TextStep::X2s))
            .line_height(px(16.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(colors.text_muted.to_paint())
            .whitespace_nowrap()
            .child(self.key)
    }
}
